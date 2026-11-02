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
use std::mem::size_of;
use std::sync::Arc;

use manyroot::{AgentId, Body, ContributionId, Graph, RelationKind};

use crate::ancestry::{self, Ancestry, Stats};
use crate::format::{
    Change, DECISION_MEDIA_TYPE, Decision, Disposition, LINE_MEDIA_TYPE, Line, PROPOSAL_MEDIA_TYPE, Proposal, Record,
};
use crate::ground::Index;
use crate::hash::ContentHash;
use crate::objections::{Obligations, Open};
use crate::results::answer;

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
    /// No single policy is in force: the chain diverged among its ancestors, into
    /// `count` versions. `versions` holds the first [`DIVERGED_SHOWN`] of them, ascending,
    /// shared by every decision that found the same ones (task a90865cb60db);
    /// [`Lines::current_versions`] lists them all.
    #[error(
        "the line's policy has diverged into {count} versions, among them {versions:?}, so no policy is in force; \
         the line's author settles them"
    )]
    PolicyDiverged { versions: Arc<[ContributionId]>, count: usize },
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
    /// Rule 5.
    #[error("it doesn't answer the {basis} objection {objection}, nor does a decision in its window")]
    Unanswered { objection: ContributionId, basis: &'static str },
    #[error("the {basis} objection {objection} is answered only {}, which its basis doesn't allow", disposition.as_str())]
    AnswerNotAllowed { objection: ContributionId, disposition: Disposition, basis: &'static str },
    /// Rule 6.
    #[error("its breakage for {check} names the protected path {path}")]
    ProtectedPath { check: String, path: String },
    #[error("its breakage has {count} entries, more than the {max} its policy allows, and doesn't reduce its parents'")]
    TooMuchBreakage { count: usize, max: u64 },
    #[error(
        "its breakage for {check} is {age} line moves old, more than the {max} its policy allows, and doesn't reduce its parents'"
    )]
    BreakageTooOld { check: String, age: u64, max: u64 },
}

/// The most versions a [`Unqualified::PolicyDiverged`] verdict holds. Each decision whose
/// past saw a different divergence keeps its own list, so a line's author could otherwise
/// make D decisions keep up to D x K versions (task a90865cb60db).
pub const DIVERGED_SHOWN: usize = 8;

/// A qualifying decision's line and the policy version in force for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qualified {
    pub line: ContributionId,
    pub policy: ContributionId,
}

/// The lines a build judges: the ones asked about and the default line, by first artifact, and every
/// version of their chains.
#[derive(Debug, Default)]
struct Scope {
    lines: BTreeSet<ContributionId>,
    versions: BTreeSet<ContributionId>,
}

/// Picks the lines a build judges, given what is known before any is judged.
type Pick<'a> = Box<dyn FnOnce(&Lines) -> Vec<ContributionId> + 'a>;

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
    /// Retracted proposals' payloads: a review of one still names its line and change.
    retracted_proposals: BTreeMap<ContributionId, Proposal>,
    /// The payloads of proposals on lines this build doesn't judge: still what a review or a use names.
    unjudged_proposals: BTreeMap<ContributionId, Proposal>,
    /// The lines this build judges, with the versions of their chains, if not every line
    /// ([`Lines::build_for`]).
    scope: Option<Scope>,
    judged: BTreeMap<ContributionId, Result<Qualified, Unqualified>>,
    /// The policy version in force for each decision whose line's chain gives one, qualifying or not.
    in_force: HashMap<ContributionId, ContributionId>,
    /// Decisions in causal order, parents first, ties by the smallest id.
    order: Vec<ContributionId>,
    default_line: Option<ContributionId>,
    /// Policy versions ignored for want of an owner decision.
    ignored: Vec<ContributionId>,
    /// Line artifacts, not retracted, that don't decode with this build and would be a policy change if
    /// they did: a version a newer build wrote, such as one with a field this build doesn't know
    /// (finding 5d50422b9e11), superseding a known version as a policy change does. Their lines read as
    /// if they weren't there, so the commands that judge lines say so. One that would change no line
    /// here is left to `weave check` (finding e89cf76e9ea2).
    unreadable: Vec<ContributionId>,
    /// What answering the chain questions did.
    stats: Stats,
    /// Every objection, and what rules 5 and 6 read of each decision.
    obligations: Obligations,
    /// What answering rule 5's questions of each decision's past did.
    obligation_stats: Stats,
}

impl Lines {
    /// Every line and decision in the replica, each judged.
    pub fn build(g: &Graph, ix: &Index) -> Lines {
        Lines::build_with(g, ix, false, None)
    }

    /// [`Lines::build`], walking the graph for every question instead of answering from
    /// the index: the reference the index is tested against.
    #[doc(hidden)]
    pub fn build_by_walking(g: &Graph, ix: &Index) -> Lines {
        Lines::build_with(g, ix, true, None)
    }

    /// The lines `pick` names, and the default line, judged (task a4d1a06a1072): their decisions, the
    /// decisions those descend from on any line, and their proposals. `pick` sees every version, line
    /// and decoded record, and the default line, before anything is judged. A decision's verdict reads
    /// only its decision-ancestors and its own line's policy, so each judged here is judged as
    /// [`Lines::build`] judges it. Any other decision has no verdict, any other proposal is known only
    /// by its payload, and the validators and current policies [`crate::results::Checks`] reads are
    /// only these lines'. So a line nobody asks about costs its records' decoding and no more.
    pub fn build_for<'a>(g: &Graph, ix: &Index, pick: impl FnOnce(&Lines) -> Vec<ContributionId> + 'a) -> Lines {
        Lines::build_with(g, ix, false, Some(Box::new(pick)))
    }

    /// [`Lines::build_for`], walking the graph for every question.
    #[doc(hidden)]
    pub fn build_for_by_walking<'a>(
        g: &Graph,
        ix: &Index,
        pick: impl FnOnce(&Lines) -> Vec<ContributionId> + 'a,
    ) -> Lines {
        Lines::build_with(g, ix, true, Some(Box::new(pick)))
    }

    /// How many decisions this build judged.
    #[doc(hidden)]
    pub fn judged_count(&self) -> usize {
        self.judged.len()
    }

    /// Whether this build judges `line`: every line, for [`Lines::build`].
    pub fn judges(&self, line: &ContributionId) -> bool {
        self.scope.as_ref().is_none_or(|s| s.lines.contains(line))
    }

    /// The policy versions of the lines this build judges, in id order.
    pub fn judged_versions(&self) -> impl Iterator<Item = &Version> {
        self.versions.values().filter(|v| self.scope.as_ref().is_none_or(|s| s.versions.contains(&v.id)))
    }

    /// The default line: the line named main by the counted marker's author, among [`Lines::lines`].
    fn find_default_line(&self, g: &Graph, ix: &Index) -> Option<ContributionId> {
        let author = g.get(&ix.counted_marker()?).expect("indexed from the graph").author();
        self.lines().find(|v| v.author == author && v.policy.name == "main").map(|v| v.id)
    }

    fn build_with(g: &Graph, ix: &Index, walking: bool, pick: Option<Pick<'_>>) -> Lines {
        let mut lines = Lines::default();
        let mut undecoded = Vec::new();
        let order = g.topo_order();
        for id in &order {
            let c = g.get(id).expect("listed by the graph");
            let Body::Artifact { media_type, content, .. } = c.body() else { continue };
            match media_type.as_str() {
                LINE_MEDIA_TYPE if !g.is_retracted(id) => match Line::decode(content) {
                    Ok(policy) => {
                        lines.versions.insert(*id, Version { id: *id, author: c.author(), policy });
                    }
                    Err(_) => undecoded.push(*id),
                },
                PROPOSAL_MEDIA_TYPE => {
                    if let Ok(proposal) = Proposal::decode(content) {
                        if g.is_retracted(id) {
                            lines.retracted_proposals.insert(*id, proposal);
                        } else {
                            let record = ProposalRecord { id: *id, author: c.author(), proposal, verdict: Ok(()) };
                            lines.proposals.insert(*id, record);
                        }
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
        // An undecodable artifact counts only where it would displace a version, by the rule above: it
        // supersedes a counted version of its own author's, in a relation that author signed.
        lines.unreadable = undecoded
            .into_iter()
            .filter(|id| {
                let author = g.get(id).expect("listed by the graph").author();
                g.referrers(id).any(|r| {
                    matches!(r.body(), Body::Relation { from, kind: RelationKind::Supersedes, to, .. }
                        if from == id
                            && lines.versions.get(to).is_some_and(|old| old.author == author)
                            && r.author() == author
                            && !g.is_retracted(&r.id()))
                })
            })
            .collect();
        if let Some(pick) = pick {
            lines.restrict(g, ix, pick);
        }
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
        // Rules 5 and 6: each decision's window, answers and used objections from the decision graph, then
        // every question they ask of each decision's past, under the policy in force for it, at once
        // (decision on split note dc467163233c).
        // Only a decision that may qualify is worked out: one that is retracted, on no line, by no
        // integrator under the policy in force for it, or with a parent that may not, fails rule 1, 2 or
        // 3 whatever it must answer, and so does every descendant (finding e4f57cd207b6). Those rules
        // are judged as before; this only leaves out what they make moot.
        let mut may_qualify: HashSet<ContributionId> = HashSet::new();
        for id in &lines.order {
            let record = &lines.decisions[id];
            let d = &record.decision;
            if g.is_retracted(id) || !lines.versions.contains_key(&d.line) {
                continue;
            }
            let walked;
            let chain = match walking {
                true => {
                    walked = lines.chain(&d.line, true, |a| g.is_ancestor(a, id));
                    Some(&walked)
                }
                false => chains.get(id),
            };
            let integrates = match chain {
                Some(Ok(policy)) => lines.versions[policy].policy.integrators.contains(&record.author),
                Some(Err(_)) => false,
                None => true,
            };
            if integrates && d.parents.iter().all(|p| may_qualify.contains(p)) {
                may_qualify.insert(*id);
            }
        }
        let mut obligations = Obligations::new(ix, &lines, &lines.order, may_qualify);
        // What answers settle is asked first, as the windows depend on it. Answers naming the line name
        // the window, so settling then takes two passes: the first leaves them out, and its windows, which
        // hold the second's, give what they ask (decision 4afee4855944).
        let first_asks = if walking { HashMap::new() } else { obligations.settle_asks(ix.retractions(), false) };
        let blanket = Ancestry::build(g, &first_asks, &[]);
        drop(first_asks);
        let blanket_missed = Cell::new(0);
        let two_passes = obligations.has_line_answers();
        let mut covered = |o: &ContributionId, a: &ContributionId| match walking {
            true => g.is_ancestor(o, a),
            false => answer(&blanket, &blanket_missed, g, o, a),
        };
        obligations.propagate(ix, &lines, &lines.order, ix.retractions(), false, &mut covered);
        let line_wide = match (two_passes, walking) {
            (true, false) => Ancestry::build(g, &obligations.settle_asks(ix.retractions(), true), &[]),
            _ => Ancestry::default(),
        };
        if two_passes {
            let mut covered = |o: &ContributionId, a: &ContributionId| match walking {
                true => g.is_ancestor(o, a),
                false => match line_wide.is_ancestor(o, a) {
                    Some(answer) => answer,
                    None => answer(&blanket, &blanket_missed, g, o, a),
                },
            };
            obligations.propagate(ix, &lines, &lines.order, ix.retractions(), true, &mut covered);
        }
        let mut refs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        if !walking {
            for (id, chain) in &chains {
                let (Ok(policy), Some(d)) = (chain, lines.decisions.get(id)) else { continue };
                let (mut past, mut epoch) = (Vec::new(), Vec::new());
                obligations.asks(id, &d.decision.line, ix.retractions(), &mut past, &mut epoch);
                if !past.is_empty() {
                    refs.insert(*id, past);
                }
                for r in epoch {
                    refs.entry(r).or_default().push(*policy);
                }
            }
        }
        let pasts = Ancestry::build(g, &refs, &[]);
        drop(refs);
        let rule_5_missed = Cell::new(0);
        let in_past_of = |a: &ContributionId, b: &ContributionId| match walking {
            true => g.is_ancestor(a, b),
            false => answer(&pasts, &rule_5_missed, g, a, b),
        };
        // Parents come before children in `order`, so each parent is judged first.
        for id in lines.order.clone() {
            let record = &lines.decisions[&id];
            let verdict = if g.is_retracted(&id) {
                Err(Unqualified::Retracted)
            } else {
                let line = &record.decision.line;
                let in_force = Cell::new(None);
                let chain = || {
                    let chain = chains.remove(&id).unwrap_or_else(|| walk(&lines, line, &id, true));
                    in_force.set(chain.as_ref().ok().copied());
                    chain
                };
                let verdict = lines.judge(ix, record.author, &record.decision, |a| is_ancestor(a, &id), chain);
                if let Some(policy) = in_force.get() {
                    lines.in_force.insert(id, policy);
                }
                verdict.and_then(|q| {
                    let policy = &lines.versions[&q.policy].policy;
                    let reads = obligations.of(&id);
                    let mut in_past = |x: &ContributionId| in_past_of(x, &id);
                    let mut in_epoch = |r: &ContributionId| in_past_of(&q.policy, r);
                    obligations.rule_5(
                        &lines,
                        ix.retractions(),
                        &record.author,
                        &record.decision,
                        policy,
                        &reads,
                        &mut in_past,
                        &mut in_epoch,
                    )?;
                    obligations.rule_6(&lines, &record.decision, policy, &reads.ages)?;
                    Ok(q)
                })
            };
            lines.judged.insert(id, verdict);
            obligations.judged(&lines, &id);
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

        lines.default_line = lines.find_default_line(g, ix);
        lines.stats.fallbacks += missed.get();
        let (first, second, then) = (blanket.stats(), line_wide.stats(), pasts.stats());
        lines.obligation_stats = Stats {
            passes: first.passes + second.passes + then.passes,
            ors: first.ors + second.ors + then.ors,
            questions: first.questions + second.questions + then.questions,
            bytes: first.bytes.max(then.bytes),
            fallbacks: rule_5_missed.get() + blanket_missed.get(),
            ..then
        };
        lines.obligations = obligations;
        lines
    }

    /// Leaves only what [`Lines::build_for`] judges: the lines `pick` names and the default line, each
    /// with the versions of its chain; the decisions on them and every decision those descend from; and
    /// the proposals on them. As nothing is judged yet, the first artifacts are those that supersede no
    /// version, as [`Lines::lines`] and the default line take them.
    fn restrict(&mut self, g: &Graph, ix: &Index, pick: Pick<'_>) {
        self.firsts = self.versions.keys().filter(|id| !self.predecessors.contains_key(id)).copied().collect();
        self.default_line = self.find_default_line(g, ix);
        let mut scope = Scope { lines: pick(self).into_iter().collect(), versions: BTreeSet::new() };
        scope.lines.extend(self.default_line);
        let mut stack: Vec<ContributionId> = scope.lines.iter().copied().collect();
        while let Some(v) = stack.pop() {
            if self.versions.contains_key(&v) && scope.versions.insert(v) {
                stack.extend(self.successors.get(&v).into_iter().flatten().map(|(_, next)| *next));
            }
        }
        let mut judging: HashSet<ContributionId> = HashSet::new();
        let mut stack: Vec<ContributionId> =
            self.decisions.values().filter(|d| scope.lines.contains(&d.decision.line)).map(|d| d.id).collect();
        while let Some(id) = stack.pop() {
            if let Some(d) = self.decisions.get(&id)
                && judging.insert(id)
            {
                stack.extend(d.decision.parents.iter().copied());
            }
        }
        self.order.retain(|id| judging.contains(id));
        let (judged, rest): (BTreeMap<_, _>, BTreeMap<_, _>) =
            std::mem::take(&mut self.proposals).into_iter().partition(|(_, r)| scope.lines.contains(&r.proposal.line));
        self.proposals = judged;
        self.unjudged_proposals = rest.into_iter().map(|(id, r)| (id, r.proposal)).collect();
        self.scope = Some(scope);
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
        for d in self.order.iter().map(|id| &self.decisions[id]) {
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
        askers.shrink_to_fit();
        asks.shrink_to_fit();
        let missed = Cell::new(0);
        let mut chains = HashMap::new();
        // Each distinct list of diverged versions, kept once and shared by every verdict naming it.
        let mut diverged: HashSet<Arc<[ContributionId]>> = HashSet::new();
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
            let mut verdict = self.chain(&line, policy, before);
            if let Err(Unqualified::PolicyDiverged { versions, .. }) = &mut verdict {
                match diverged.get(&versions[..]) {
                    Some(shared) => *versions = shared.clone(),
                    None => {
                        diverged.insert(versions.clone());
                    }
                }
            }
            chains.insert(asker, verdict);
        });
        // What the chain questions hold until the records are judged, counted by hand from each structure's
        // own element type: the lists, the askers, a verdict each, and each distinct list of diverged
        // versions once (task a90865cb60db).
        fn kept<T>(v: &Vec<T>) -> usize {
            size_of::<Vec<T>>() + v.capacity() * size_of::<T>()
        }
        fn entries<K, V>(m: &HashMap<K, V>) -> usize {
            m.len() * size_of::<(K, V)>()
        }
        let bytes = lists.iter().map(kept).sum::<usize>()
            + kept(&askers)
            + kept(&asks)
            + entries(&chains)
            + diverged
                .iter()
                .map(|d| size_of::<Arc<[ContributionId]>>() + 2 * size_of::<usize>() + size_of_val(&d[..]))
                .sum::<usize>();
        self.stats = Stats { fallbacks: missed.get(), bytes, ..stats };
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
        let newest = self.newest(line, before);
        match newest.as_slice() {
            [one] => Ok(*one),
            _ => Err(Unqualified::PolicyDiverged {
                versions: newest.iter().take(DIVERGED_SHOWN).copied().collect(),
                count: newest.len(),
            }),
        }
    }

    /// The versions of `line`'s chain among the contributions `before` holds that no
    /// other version there supersedes, ascending.
    fn newest(&self, line: &ContributionId, before: &impl Fn(&ContributionId) -> bool) -> Vec<ContributionId> {
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
        newest
    }

    /// Whether a decision `author` would write now, listing every current head of
    /// the Ground as a parent, would qualify. Its manifest must already be held.
    pub fn judge_new(&self, g: &Graph, ix: &Index, author: AgentId, d: &Decision) -> Result<Qualified, Unqualified> {
        let q = self.judge(ix, author, d, |id| g.contains(id), || self.chain(&d.line, true, |id| g.contains(id)))?;
        // Its past is the whole replica, and retraction is read now; the epoch is asked of the index.
        let policy = &self.versions[&q.policy].policy;
        let reads = self.obligations.of_new(ix, self, d);
        let candidates: Vec<(bool, u32)> =
            reads.window.iter().map(|at| (false, *at)).chain(reads.uses.iter().map(|at| (true, *at))).collect();
        let epochs = self.epochs(g, &candidates, &q.policy);
        let missed = Cell::new(0);
        let mut in_epoch = |r: &ContributionId| answer(&epochs, &missed, g, &q.policy, r);
        let mut in_past = |x: &ContributionId| g.contains(x);
        let (retractions, obligations) = (ix.retractions(), &self.obligations);
        obligations.rule_5(self, retractions, &author, d, policy, &reads, &mut in_past, &mut in_epoch)?;
        obligations.rule_6(self, d, policy, &reads.ages)?;
        Ok(q)
    }

    /// Whether `version` is an ancestor of each result an objection in `candidates` comes from, asked of
    /// the ancestor index.
    fn epochs(&self, g: &Graph, candidates: &[(bool, u32)], version: &ContributionId) -> Ancestry {
        let refs: HashMap<ContributionId, Vec<ContributionId>> =
            self.obligations.basis_results(candidates).into_iter().map(|r| (r, vec![*version])).collect();
        Ancestry::build(g, &refs, &[])
    }

    /// The objections the next move on `line` must answer (weave.md section 2, *Open objections*): those
    /// in the window of a decision with every head as a parent that no answer allows, judged under the
    /// line's current policy over the whole replica, each with the dispositions its basis allows. `None`
    /// if the line has no current policy.
    pub fn open_objections(&self, g: &Graph, ix: &Index, line: &ContributionId) -> Option<Vec<Open>> {
        let [version] = self.current_versions(line)[..] else { return None };
        let policy = &self.versions[&version].policy;
        let candidates = self.obligations.next_move(self, line, &self.heads(line));
        let epochs = self.epochs(g, &candidates, &version);
        let missed = Cell::new(0);
        let mut in_epoch = |r: &ContributionId| answer(&epochs, &missed, g, &version, r);
        let retractions = ix.retractions();
        Some(self.obligations.open(self, retractions, line, policy, &candidates, None, &mut in_epoch))
    }

    /// The objections a decision using `proposal` unchanged on its line would have to answer, to any
    /// proposal with its line and change or to its manifest, that no decision on the line has answered
    /// as their basis allows, under the line's current policy over the whole replica. Results by any
    /// validator count, as the integrator isn't known. `None` if `proposal` isn't one, or its line has
    /// no current policy.
    pub fn proposal_objections(&self, g: &Graph, ix: &Index, proposal: &ContributionId) -> Option<Vec<Open>> {
        let line = self.proposal_named(proposal)?.line;
        let [version] = self.current_versions(&line)[..] else { return None };
        let policy = &self.versions[&version].policy;
        let candidates = self.obligations.of_proposal(self, proposal, &self.heads(&line));
        let epochs = self.epochs(g, &candidates, &version);
        let missed = Cell::new(0);
        let mut in_epoch = |r: &ContributionId| answer(&epochs, &missed, g, &version, r);
        let retractions = ix.retractions();
        Some(self.obligations.open(self, retractions, &line, policy, &candidates, None, &mut in_epoch))
    }

    /// Every objection in the replica whose target resolves.
    pub fn objections(&self) -> &Obligations {
        &self.obligations
    }

    /// What answering rule 5's questions of each decision's past did, and how many fell back to a walk.
    #[doc(hidden)]
    pub fn obligation_stats(&self) -> Stats {
        self.obligation_stats
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

    /// Every version of every line's policy that counts, in id order.
    pub fn versions(&self) -> impl Iterator<Item = &Version> {
        self.versions.values()
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

    /// Line artifacts, not retracted, that this build can't decode, in causal order: while there are
    /// any, a line may be read under an older version of its policy than the one in force.
    pub fn unreadable_versions(&self) -> &[ContributionId] {
        &self.unreadable
    }

    /// The line's current policy: the version of its chain no other in the replica supersedes. Several
    /// if the chain diverged.
    pub fn current_versions(&self, line: &ContributionId) -> Vec<ContributionId> {
        match self.versions.contains_key(line) {
            true => self.newest(line, &|_: &ContributionId| true),
            false => vec![],
        }
    }

    /// The current line named `main` by the counted marker's author, with the smallest id.
    pub fn default_line(&self) -> Option<ContributionId> {
        self.default_line
    }

    pub fn decision(&self, id: &ContributionId) -> Option<&DecisionRecord> {
        self.decisions.get(id)
    }

    /// A proposal, valid or not, not retracted.
    pub fn proposal(&self, id: &ContributionId) -> Option<&ProposalRecord> {
        self.proposals.get(id)
    }

    /// The payload of a proposal artifact that decodes, retracted or not, valid or not: what a review of
    /// it names, by its line and change (finding 06bbc6a34636).
    pub fn proposal_named(&self, id: &ContributionId) -> Option<&Proposal> {
        self.proposals
            .get(id)
            .map(|r| &r.proposal)
            .or_else(|| self.retracted_proposals.get(id))
            .or_else(|| self.unjudged_proposals.get(id))
    }

    /// The valid proposals on a line, ascending.
    pub fn proposals_on<'a>(&'a self, line: &'a ContributionId) -> impl Iterator<Item = &'a ProposalRecord> + 'a {
        self.proposals.values().filter(move |r| r.verdict.is_ok() && r.proposal.line == *line)
    }

    /// The policy version in force for a decision, qualifying or not, if its line's chain gives one.
    pub fn policy_in_force_for(&self, decision: &ContributionId) -> Option<&Version> {
        self.in_force.get(decision).map(|v| &self.versions[v])
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
