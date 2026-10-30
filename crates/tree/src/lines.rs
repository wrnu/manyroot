//! Lines and decisions (docs/design/weave.md, section 1 *Lines* and *Decisions*,
//! and section 2): each line's policy versions, which decisions qualify under
//! rules 1 to 4, and each line's heads.
//!
//! Every view here is derived from the Ground in causal order, ties broken by
//! the smallest id. None reads `timestamp_ms`. A reference inside a payload
//! counts only if it is a causal ancestor of the record holding it (the
//! ancestor rule), and whether a decision qualifies reads only its causal
//! ancestors, so a later contribution never changes it: which version of the
//! line's policy is in force, and whether its line is a line's first artifact,
//! are both judged among them. The exceptions are retractions, which are read
//! replica-wide: a decision's own (rule 1), and those of a policy version or of
//! the supersedes relation that chains it (note 5786efbb7f8d, reading 6).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use manyroot::{AgentId, Body, ContributionId, Graph, RelationKind};

use crate::format::{DECISION_MEDIA_TYPE, Decision, LINE_MEDIA_TYPE, Line, Record};
use crate::ground::Index;
use crate::hash::ContentHash;

/// One version of a line's policy: a valid, non-retracted line artifact.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: ContributionId,
    pub author: AgentId,
    pub policy: Line,
}

/// A decision artifact whose payload is valid.
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub id: ContributionId,
    pub author: AgentId,
    pub decision: Decision,
}

/// Why a decision doesn't qualify: the first rule it breaks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unqualified {
    /// Rule 1.
    #[error("it is retracted")]
    Retracted,
    #[error("its line {0} isn't the first artifact of a line")]
    NotALine(ContributionId),
    #[error("its line {0} isn't a causal ancestor of it")]
    LineNotAnAncestor(ContributionId),
    /// No single policy is in force: the chain diverged among its ancestors.
    #[error("the line's policy has diverged into {0:?}, so no policy is in force; the line's author settles them")]
    PolicyDiverged(Vec<ContributionId>),
    /// Rule 2.
    #[error("its author {author} isn't an integrator under the policy in force, {policy}")]
    NotAnIntegrator { author: AgentId, policy: ContributionId },
    /// Rule 3.
    #[error("its parent {0} isn't a causal ancestor of it")]
    ParentNotAnAncestor(ContributionId),
    #[error("its parent {0} isn't a qualifying decision")]
    ParentUnqualified(ContributionId),
    /// Rule 4.
    #[error("its manifest {0} isn't a valid manifest held here")]
    ManifestNotHeld(ContentHash),
    #[error("its manifest {hash} isn't complete here: {missing} files are missing, or held at another size")]
    ManifestIncomplete { hash: ContentHash, missing: usize },
}

/// A qualifying decision's line and the policy version in force for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qualified {
    pub line: ContributionId,
    pub policy: ContributionId,
}

/// One replica's lines and decisions.
#[derive(Debug, Default)]
pub struct Lines {
    versions: BTreeMap<ContributionId, Version>,
    /// For each version, the versions that supersede it, with the relation that says so.
    successors: BTreeMap<ContributionId, Vec<(ContributionId, ContributionId)>>,
    /// For each version, the versions it supersedes, with the relation that says so.
    predecessors: BTreeMap<ContributionId, Vec<(ContributionId, ContributionId)>>,
    /// Every line a decision can name: versions that supersede no other, and any
    /// version a qualifying decision was judged to be the first artifact of before
    /// it superseded one. [`Lines::lines`] and the default line take only the first kind.
    firsts: BTreeSet<ContributionId>,
    decisions: BTreeMap<ContributionId, DecisionRecord>,
    judged: BTreeMap<ContributionId, Result<Qualified, Unqualified>>,
    /// Decisions in causal order, parents first, ties by the smallest id.
    order: Vec<ContributionId>,
    default_line: Option<ContributionId>,
}

impl Lines {
    pub fn build(g: &Graph, ix: &Index) -> Lines {
        let mut lines = Lines::default();
        let order = g.topo_order();
        for id in &order {
            let c = g.get(id).expect("listed by the graph");
            let Body::Artifact { media_type, content, .. } = c.body() else { continue };
            match media_type.as_str() {
                LINE_MEDIA_TYPE if !g.is_retracted(id) => {
                    if let Ok(policy) = Line::decode(content) {
                        lines.versions.insert(*id, Version { id: *id, author: c.author(), policy });
                    }
                }
                DECISION_MEDIA_TYPE => {
                    if let Ok(decision) = Decision::decode(content) {
                        lines.decisions.insert(*id, DecisionRecord { id: *id, author: c.author(), decision });
                        lines.order.push(*id);
                    }
                }
                _ => {}
            }
        }

        // A policy change is a new version by the line's author that supersedes the
        // old one, in a relation the old one's author signed.
        for (id, old) in &lines.versions {
            for r in g.referrers(id) {
                let Body::Relation { from, kind: RelationKind::Supersedes, to, .. } = r.body() else { continue };
                let Some(new) = lines.versions.get(from) else { continue };
                if to == id && r.author() == old.author && new.author == old.author && !g.is_retracted(&r.id()) {
                    lines.successors.entry(*id).or_default().push((r.id(), *from));
                    lines.predecessors.entry(*from).or_default().push((r.id(), *id));
                }
            }
        }
        // Parents come before children in `order`, so each parent is judged first.
        for id in lines.order.clone() {
            let record = &lines.decisions[&id];
            let verdict = if g.is_retracted(&id) {
                Err(Unqualified::Retracted)
            } else {
                lines.judge(ix, record.author, &record.decision, |a| g.is_ancestor(a, &id))
            };
            lines.judged.insert(id, verdict);
        }
        // A line stays a line for its decisions' sake, even if its author later has it supersede
        // another version.
        let lines_used: Vec<ContributionId> =
            lines.judged.values().filter_map(|v| v.as_ref().ok()).map(|q| q.line).collect();
        lines.firsts = lines.versions.keys().filter(|id| !lines.predecessors.contains_key(id)).copied().collect();
        lines.firsts.extend(lines_used);

        if let Some(marker) = ix.counted_marker() {
            let author = g.get(&marker).expect("indexed from the graph").author();
            let main = lines.lines().find(|v| v.author == author && v.policy.name == "main").map(|v| v.id);
            lines.default_line = main;
        }
        lines
    }

    /// Judges a decision by rules 2 to 4, given which contributions are its causal
    /// ancestors.
    fn judge(
        &self,
        ix: &Index,
        author: AgentId,
        d: &Decision,
        before: impl Fn(&ContributionId) -> bool,
    ) -> Result<Qualified, Unqualified> {
        if !self.versions.contains_key(&d.line) {
            return Err(Unqualified::NotALine(d.line));
        }
        if !before(&d.line) {
            return Err(Unqualified::LineNotAnAncestor(d.line));
        }
        // A first artifact supersedes no version, among the decision's ancestors.
        if self.predecessors.get(&d.line).into_iter().flatten().any(|(relation, _)| before(relation)) {
            return Err(Unqualified::NotALine(d.line));
        }
        let policy = self.policy_in_force(&d.line, &before)?;
        if !self.versions[&policy].policy.integrators.contains(&author) {
            return Err(Unqualified::NotAnIntegrator { author, policy });
        }
        for parent in &d.parents {
            if !before(parent) {
                return Err(Unqualified::ParentNotAnAncestor(*parent));
            }
            if !matches!(self.judged.get(parent), Some(Ok(_))) {
                return Err(Unqualified::ParentUnqualified(*parent));
            }
        }
        let Some((_, manifest)) = ix.manifest(&d.manifest) else {
            return Err(Unqualified::ManifestNotHeld(d.manifest));
        };
        let missing = ix.missing(manifest).len();
        if missing > 0 {
            return Err(Unqualified::ManifestIncomplete { hash: d.manifest, missing });
        }
        Ok(Qualified { line: d.line, policy })
    }

    /// The newest version of `line`'s chain among the contributions `before` holds:
    /// the one no other version there supersedes.
    fn policy_in_force(
        &self,
        line: &ContributionId,
        before: &impl Fn(&ContributionId) -> bool,
    ) -> Result<ContributionId, Unqualified> {
        let mut newest = Vec::new();
        let mut seen = HashSet::from([*line]);
        let mut stack = vec![*line];
        while let Some(v) = stack.pop() {
            let mut superseded = false;
            for (relation, next) in self.successors.get(&v).into_iter().flatten() {
                // The relation lists the new version as a parent, so it is enough to ask about it.
                if before(relation) {
                    superseded = true;
                    if seen.insert(*next) {
                        stack.push(*next);
                    }
                }
            }
            if !superseded {
                newest.push(v);
            }
        }
        newest.sort();
        match newest.as_slice() {
            [one] => Ok(*one),
            _ => Err(Unqualified::PolicyDiverged(newest)),
        }
    }

    /// Whether a decision `author` would write now, listing every current head of
    /// the Ground as a parent, would qualify. Its manifest must already be held.
    pub fn judge_new(&self, g: &Graph, ix: &Index, author: AgentId, d: &Decision) -> Result<Qualified, Unqualified> {
        self.judge(ix, author, d, |id| g.contains(id))
    }

    /// The line's first artifact, if `id` names one: a version that supersedes none,
    /// or one a qualifying decision was judged a line by, before it superseded one.
    pub fn line(&self, id: &ContributionId) -> Option<&Version> {
        self.firsts.contains(id).then(|| &self.versions[id])
    }

    /// Every current line, by the id of its first artifact: the versions that
    /// supersede no version in this replica. A version that has since joined another
    /// line's chain isn't listed, though its old decisions keep their judgement
    /// (finding fdd17cbdc816).
    pub fn lines(&self) -> impl Iterator<Item = &Version> {
        self.firsts.iter().filter(|id| !self.predecessors.contains_key(id)).map(|id| &self.versions[id])
    }

    /// Any version of any line's policy.
    pub fn version(&self, id: &ContributionId) -> Option<&Version> {
        self.versions.get(id)
    }

    /// The current line named `main` by the counted marker's author, with the smallest id.
    pub fn default_line(&self) -> Option<ContributionId> {
        self.default_line
    }

    pub fn decision(&self, id: &ContributionId) -> Option<&DecisionRecord> {
        self.decisions.get(id)
    }

    /// Whether a decision qualifies, and if not, why.
    pub fn verdict(&self, id: &ContributionId) -> Option<&Result<Qualified, Unqualified>> {
        self.judged.get(id)
    }

    fn on_line<'a>(&'a self, line: &'a ContributionId) -> impl Iterator<Item = &'a ContributionId> + 'a {
        self.order.iter().filter(move |id| matches!(&self.judged[*id], Ok(q) if q.line == *line))
    }

    /// The line's heads: its qualifying decisions with no qualifying descendant on
    /// the line, ascending. More than one means the line is forked.
    pub fn heads(&self, line: &ContributionId) -> Vec<ContributionId> {
        let mine: Vec<ContributionId> = self.on_line(line).copied().collect();
        // Every strict ancestor of a decision on the line, through any line. Rule 3
        // makes every parent of a qualifying decision qualify.
        let mut below: HashSet<ContributionId> = HashSet::new();
        let mut stack: Vec<ContributionId> =
            mine.iter().flat_map(|id| self.decisions[id].decision.parents.iter().copied()).collect();
        while let Some(id) = stack.pop() {
            if below.insert(id) {
                stack.extend(self.decisions[&id].decision.parents.iter().copied());
            }
        }
        let mut heads: Vec<ContributionId> = mine.into_iter().filter(|id| !below.contains(id)).collect();
        heads.sort();
        heads
    }

    /// The line's qualifying decisions, newest first in causal order, ties by the
    /// smallest id. With `since`, only those that aren't `since` or in its causal past.
    pub fn log(&self, g: &Graph, line: &ContributionId, since: Option<&ContributionId>) -> Vec<&DecisionRecord> {
        let past = since.map(|s| causal_past(g, s));
        let mine: HashSet<&ContributionId> = self.on_line(line).collect();
        newest_first(g)
            .into_iter()
            .filter(|id| mine.contains(id))
            .filter(|id| past.as_ref().is_none_or(|p| !p.contains(id) && Some(id) != since))
            .map(|id| &self.decisions[&id])
            .collect()
    }
}

/// Every strict causal ancestor of `id`.
fn causal_past(g: &Graph, id: &ContributionId) -> HashSet<ContributionId> {
    let mut past = HashSet::new();
    let mut stack: Vec<ContributionId> = g.get(id).map(|c| c.parents().to_vec()).unwrap_or_default();
    while let Some(p) = stack.pop() {
        if past.insert(p) {
            stack.extend_from_slice(g.get(&p).expect("parents are held").parents());
        }
    }
    past
}

/// Every contribution, children before parents, the smallest id first among those
/// ready: the reverse of a causal order, never reading timestamps.
pub fn newest_first(g: &Graph) -> Vec<ContributionId> {
    let mut waiting: HashMap<ContributionId, usize> = g.iter().map(|c| (c.id(), g.children(&c.id()).len())).collect();
    let mut ready: BTreeSet<ContributionId> = waiting.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
    let mut out = Vec::with_capacity(waiting.len());
    while let Some(id) = ready.pop_first() {
        out.push(id);
        for p in g.get(&id).expect("held").parents() {
            let n = waiting.get_mut(p).expect("parents are held");
            *n -= 1;
            if *n == 0 {
                ready.insert(*p);
            }
        }
    }
    out
}
