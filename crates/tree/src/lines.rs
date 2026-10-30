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

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use manyroot::{AgentId, Body, ContributionId, Graph, RelationKind};

use crate::ancestry::{self, Stats};
use crate::format::{
    Change, DECISION_MEDIA_TYPE, Decision, LINE_MEDIA_TYPE, Line, PROPOSAL_MEDIA_TYPE, Proposal, Record,
};
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

/// A proposal artifact whose payload decodes, and whether it is valid.
#[derive(Debug, Clone)]
pub struct ProposalRecord {
    pub id: ContributionId,
    pub author: AgentId,
    pub proposal: Proposal,
    pub verdict: Result<(), InvalidProposal>,
}

/// Why a proposal is invalid: views ignore it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidProposal {
    #[error("its line {0} isn't a line's first artifact in its causal past")]
    NotALine(ContributionId),
    #[error("its base {0} isn't a qualifying decision on its line, in its causal past")]
    Base(ContributionId),
    #[error("its manifest {0} isn't a valid manifest held here")]
    ManifestNotHeld(ContentHash),
    #[error("its change {given} isn't the change from its base's manifest to its own, {computed}")]
    Change { given: ContentHash, computed: ContentHash },
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
    proposals: BTreeMap<ContributionId, ProposalRecord>,
    judged: BTreeMap<ContributionId, Result<Qualified, Unqualified>>,
    /// Decisions in causal order, parents first, ties by the smallest id.
    order: Vec<ContributionId>,
    default_line: Option<ContributionId>,
    /// Policy versions ignored for want of an owner decision.
    ignored: Vec<ContributionId>,
    /// What answering the chain questions did.
    stats: Stats,
}

impl Lines {
    pub fn build(g: &Graph, ix: &Index) -> Lines {
        Lines::build_with(g, ix, false)
    }

    /// [`Lines::build`], walking the graph for every question instead of answering from
    /// the index: the reference the index is tested against.
    #[doc(hidden)]
    pub fn build_by_walking(g: &Graph, ix: &Index) -> Lines {
        Lines::build_with(g, ix, true)
    }

    fn build_with(g: &Graph, ix: &Index, walking: bool) -> Lines {
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
                PROPOSAL_MEDIA_TYPE if !g.is_retracted(id) => {
                    if let Ok(proposal) = Proposal::decode(content) {
                        let record = ProposalRecord { id: *id, author: c.author(), proposal, verdict: Ok(()) };
                        lines.proposals.insert(*id, record);
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
        // A policy version cites the owner's decision; only the main line weave init writes, a first
        // artifact named main by the counted marker's author, may carry none (finding 12643881ac59).
        // The rest count for nothing, and weave check lists them.
        let marker_author = ix.counted_marker().and_then(|m| g.get(&m)).map(|c| c.author());
        let ignored: BTreeSet<ContributionId> = lines
            .versions
            .values()
            .filter(|v| {
                v.policy.owner_decision.is_none()
                    && !(Some(v.author) == marker_author
                        && v.policy.name == "main"
                        && !lines.predecessors.contains_key(&v.id))
            })
            .map(|v| v.id)
            .collect();
        lines.versions.retain(|id, _| !ignored.contains(id));
        for links in [&mut lines.successors, &mut lines.predecessors] {
            links.retain(|id, _| !ignored.contains(id));
            for list in links.values_mut() {
                list.retain(|(_, other)| !ignored.contains(other));
            }
            links.retain(|_, list| !list.is_empty());
        }
        lines.ignored = ignored.into_iter().collect();
        let mut chains = if walking { HashMap::new() } else { lines.ask_chains(g) };
        let missed = Cell::new(0);
        // A chain verdict nothing asked for: a bug, so walk, and count it (answer b2dbc4f5e810).
        let walk = |lines: &Lines, line: &ContributionId, id: &ContributionId, policy: bool| {
            if !walking {
                if cfg!(debug_assertions) {
                    panic!("{id} didn't ask about {line}'s chain");
                }
                missed.set(missed.get() + 1);
            }
            lines.chain(line, policy, |a| g.is_ancestor(a, id))
        };
        let is_ancestor = |a: &ContributionId, b: &ContributionId| match walking {
            true => g.is_ancestor(a, b),
            false => ix.is_ancestor(g, a, b),
        };
        // Parents come before children in `order`, so each parent is judged first.
        for id in lines.order.clone() {
            let record = &lines.decisions[&id];
            let verdict = if g.is_retracted(&id) {
                Err(Unqualified::Retracted)
            } else {
                let line = &record.decision.line;
                let chain = || chains.remove(&id).unwrap_or_else(|| walk(&lines, line, &id, true));
                lines.judge(ix, record.author, &record.decision, |a| is_ancestor(a, &id), chain)
            };
            lines.judged.insert(id, verdict);
        }
        // A line stays a line for its decisions' sake, even if its author later has it supersede
        // another version.
        let lines_used: Vec<ContributionId> =
            lines.judged.values().filter_map(|v| v.as_ref().ok()).map(|q| q.line).collect();
        lines.firsts = lines.versions.keys().filter(|id| !lines.predecessors.contains_key(id)).copied().collect();
        lines.firsts.extend(lines_used);

        // A proposal's base is a qualifying decision on its line, and its change recomputes.
        let verdicts: Vec<(ContributionId, Result<(), InvalidProposal>)> = lines
            .proposals
            .values()
            .map(|r| {
                let p = &r.proposal;
                let before = |a: &ContributionId| is_ancestor(a, &r.id);
                let verdict = if !lines.versions.contains_key(&p.line)
                    || !before(&p.line)
                    || chains.remove(&r.id).unwrap_or_else(|| walk(&lines, &p.line, &r.id, false)).is_err()
                {
                    Err(InvalidProposal::NotALine(p.line))
                } else if !before(&p.base) || !matches!(lines.judged.get(&p.base), Some(Ok(q)) if q.line == p.line) {
                    Err(InvalidProposal::Base(p.base))
                } else {
                    let base = ix.manifest(&lines.decisions[&p.base].decision.manifest).map(|(_, m)| m);
                    match (base, ix.manifest(&p.manifest).map(|(_, m)| m)) {
                        (_, None) => Err(InvalidProposal::ManifestNotHeld(p.manifest)),
                        (None, _) => Err(InvalidProposal::Base(p.base)),
                        (Some(base), Some(new)) => {
                            let computed = Change::between(base, new).hash();
                            if computed == p.change {
                                Ok(())
                            } else {
                                Err(InvalidProposal::Change { given: p.change, computed })
                            }
                        }
                    }
                };
                (r.id, verdict)
            })
            .collect();
        for (id, verdict) in verdicts {
            lines.proposals.get_mut(&id).expect("listed").verdict = verdict;
        }

        if let Some(marker) = ix.counted_marker() {
            let author = g.get(&marker).expect("indexed from the graph").author();
            let main = lines.lines().find(|v| v.author == author && v.policy.name == "main").map(|v| v.id);
            lines.default_line = main;
        }
        lines.stats.fallbacks += missed.get();
        lines
    }

    /// Judges a decision by rules 2 to 4, given which of the contributions it names are
    /// its causal ancestors, and what it finds of its line's chain ([`Lines::chain`]).
    fn judge(
        &self,
        ix: &Index,
        author: AgentId,
        d: &Decision,
        before: impl Fn(&ContributionId) -> bool,
        chain: impl FnOnce() -> Result<ContributionId, Unqualified>,
    ) -> Result<Qualified, Unqualified> {
        if !self.versions.contains_key(&d.line) {
            return Err(Unqualified::NotALine(d.line));
        }
        if !before(&d.line) {
            return Err(Unqualified::LineNotAnAncestor(d.line));
        }
        let policy = chain()?;
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

    /// What a record finds of `line`'s chain, given which of its supersedes relations are
    /// among the record's causal ancestors: `NotALine` if the line supersedes a version
    /// there, as a first artifact supersedes none; otherwise the policy in force, or with
    /// `policy` false, just the line.
    fn chain(
        &self,
        line: &ContributionId,
        policy: bool,
        before: impl Fn(&ContributionId) -> bool,
    ) -> Result<ContributionId, Unqualified> {
        if self.predecessors.get(line).into_iter().flatten().any(|(relation, _)| before(relation)) {
            return Err(Unqualified::NotALine(*line));
        }
        if policy { self.policy_in_force(line, &before) } else { Ok(*line) }
    }

    /// Every chain question, asked by the records that ask it (design note e93d47ad8a20):
    /// each decision and proposal naming a version asks about every relation in that
    /// version's component, the versions the relations Lines counts join, whichever way.
    /// [`Lines::chain`] only follows those relations from the line, so it asks nothing
    /// else, and as the components come from Lines' own relations, after it drops
    /// versions, no rule about which versions are first artifacts is guessed. Each
    /// relation is in one component, so stored once. Returns each record's chain verdict.
    fn ask_chains(&mut self, g: &Graph) -> HashMap<ContributionId, Result<ContributionId, Unqualified>> {
        fn find(root: &mut HashMap<ContributionId, ContributionId>, v: ContributionId) -> ContributionId {
            let mut r = v;
            while let Some(up) = root.get(&r).copied().filter(|up| *up != r) {
                r = up;
            }
            let mut v = v;
            while v != r {
                v = root.insert(v, r).unwrap_or(r);
            }
            r
        }
        let mut root: HashMap<ContributionId, ContributionId> = HashMap::new();
        for (old, links) in &self.successors {
            for (_, new) in links {
                let (a, b) = (find(&mut root, *old), find(&mut root, *new));
                if a != b {
                    root.insert(a.max(b), a.min(b));
                }
            }
        }
        // The first list is empty: the chain of a version no relation joins.
        let mut lists: Vec<Vec<ContributionId>> = vec![Vec::new()];
        let mut list_of: HashMap<ContributionId, usize> = HashMap::new();
        for (old, links) in &self.successors {
            let component = find(&mut root, *old);
            let next = lists.len();
            let at = *list_of.entry(component).or_insert(next);
            if at == next {
                lists.push(Vec::new());
            }
            lists[at].extend(links.iter().map(|(relation, _)| *relation));
        }
        for list in &mut lists {
            list.sort_unstable();
            list.shrink_to_fit();
        }
        // The askers, the line each names, and whether it needs the policy in force (a decision) or only
        // whether its line is a first artifact (a proposal).
        let mut askers: Vec<(ContributionId, usize)> = Vec::new();
        let mut asks: Vec<(ContributionId, bool)> = Vec::new();
        let (successors, predecessors) = (&self.successors, &self.predecessors);
        let mut list_for =
            |line: &ContributionId| match successors.contains_key(line) || predecessors.contains_key(line) {
                true => list_of[&find(&mut root, *line)],
                false => 0,
            };
        for d in self.decisions.values() {
            if self.versions.contains_key(&d.decision.line) && !g.is_retracted(&d.id) {
                askers.push((d.id, list_for(&d.decision.line)));
                asks.push((d.decision.line, true));
            }
        }
        for p in self.proposals.values() {
            if self.versions.contains_key(&p.proposal.line) {
                askers.push((p.id, list_for(&p.proposal.line)));
                asks.push((p.proposal.line, false));
            }
        }
        let missed = Cell::new(0);
        let mut chains = HashMap::new();
        let stats = ancestry::rounds(g, &lists, &askers, |k, answers| {
            let (asker, (line, policy)) = (askers[k].0, asks[k]);
            let before = |a: &ContributionId| {
                let answer = answers.get(a);
                debug_assert!(answer.is_some(), "{asker} didn't ask about {a}");
                answer.unwrap_or_else(|| {
                    missed.set(missed.get() + 1);
                    g.is_ancestor(a, &asker)
                })
            };
            chains.insert(asker, self.chain(&line, policy, before));
        });
        self.stats = Stats { fallbacks: missed.get(), ..stats };
        chains
    }

    /// What answering the chain questions did, and how many fell back to a walk.
    #[doc(hidden)]
    pub fn stats(&self) -> Stats {
        self.stats
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
        self.judge(ix, author, d, |id| g.contains(id), || self.chain(&d.line, true, |id| g.contains(id)))
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

    /// Valid line artifacts that count for nothing, because they cite no owner decision and aren't
    /// the main line weave init writes; ascending.
    pub fn ignored_versions(&self) -> &[ContributionId] {
        &self.ignored
    }

    /// The line's current policy: the version of its chain no other in the replica supersedes. Several
    /// if the chain diverged.
    pub fn current_versions(&self, line: &ContributionId) -> Vec<ContributionId> {
        match self.policy_in_force(line, &|_: &ContributionId| true) {
            Ok(one) => vec![one],
            Err(Unqualified::PolicyDiverged(many)) => many,
            Err(_) => vec![],
        }
    }

    /// The current line named `main` by the counted marker's author, with the smallest id.
    pub fn default_line(&self) -> Option<ContributionId> {
        self.default_line
    }

    pub fn decision(&self, id: &ContributionId) -> Option<&DecisionRecord> {
        self.decisions.get(id)
    }

    /// A proposal, valid or not.
    pub fn proposal(&self, id: &ContributionId) -> Option<&ProposalRecord> {
        self.proposals.get(id)
    }

    /// The valid proposals on a line, ascending.
    pub fn proposals_on<'a>(&'a self, line: &'a ContributionId) -> impl Iterator<Item = &'a ProposalRecord> + 'a {
        self.proposals.values().filter(move |r| r.verdict.is_ok() && r.proposal.line == *line)
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

impl Lines {
    /// `id` and every decision it descends from through decision parents.
    pub fn decision_ancestry(&self, id: &ContributionId) -> HashSet<ContributionId> {
        let mut seen = HashSet::new();
        let mut stack = vec![*id];
        while let Some(d) = stack.pop() {
            if seen.insert(d)
                && let Some(record) = self.decisions.get(&d)
            {
                stack.extend(record.decision.parents.iter().copied());
            }
        }
        seen
    }

    /// The merge bases of two decisions: their common ancestors in the decision
    /// graph (each counting as its own ancestor) that aren't ancestors of another
    /// common ancestor, ascending. In v0 a merge needs exactly one: several is a
    /// criss-cross, and none means the two share no history.
    pub fn merge_bases(&self, a: &ContributionId, b: &ContributionId) -> Vec<ContributionId> {
        let (ours, theirs) = (self.decision_ancestry(a), self.decision_ancestry(b));
        let common: BTreeSet<ContributionId> = ours.intersection(&theirs).copied().collect();
        // Every strict ancestor of a common ancestor, in one walk from their parents.
        let mut below: HashSet<ContributionId> = HashSet::new();
        let mut stack: Vec<ContributionId> = common
            .iter()
            .filter_map(|c| self.decisions.get(c))
            .flat_map(|r| r.decision.parents.iter().copied())
            .collect();
        while let Some(d) = stack.pop() {
            if below.insert(d)
                && let Some(record) = self.decisions.get(&d)
            {
                stack.extend(record.decision.parents.iter().copied());
            }
        }
        common.into_iter().filter(|c| !below.contains(c)).collect()
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
