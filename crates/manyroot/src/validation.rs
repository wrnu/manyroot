//! Proof: deciding which contributions to treat as accepted.
//!
//! Acceptance is never replicated. Every replica holds the same evidence, and
//! each reader applies its own [`Policy`] to it, so two communities can share
//! a Ground and still disagree about what is established. The graph
//! synchronizes evidence; it does not enforce consensus.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use serde::Serialize;

use crate::contribution::{Body, RelationKind, Verdict};
use crate::graph::Graph;
use crate::id::{AgentId, ContributionId};

/// Rules for turning evidence into acceptance.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Distinct independent validators that must report a pass.
    pub min_passes: usize,
    /// Whose evidence counts. `None` means any agent. An author's evidence
    /// about their own contribution never counts.
    pub validators: Option<BTreeSet<AgentId>>,
    /// Acceptance follows changes: an accepted subject whose dependencies
    /// changed after its passes is [`Status::NeedsRecheck`] instead. Off by
    /// default, so every status reads as without it.
    pub follow_changes: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self { min_passes: 1, validators: None, follow_changes: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Meets the policy and nothing accepted replaces it.
    Accepted,
    /// Would be accepted, but something it depends on was superseded or
    /// retracted after its passes. Only under [`Policy::follow_changes`], and
    /// never in the accepted layer.
    NeedsRecheck,
    /// Meets the policy, but an accepted contribution supersedes it.
    Superseded,
    /// Not enough independent passing evidence yet.
    Pending,
    /// A counted validator reported a failure.
    Rejected,
    /// Withdrawn by its author.
    Retracted,
}

/// The validation result for one claim or artifact.
#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub id: ContributionId,
    pub status: Status,
    /// Counted passing evidence.
    pub passes: Vec<ContributionId>,
    /// Counted failing evidence.
    pub fails: Vec<ContributionId>,
    /// Accepted contributions that supersede this one.
    pub superseded_by: Vec<ContributionId>,
    /// For [`Status::NeedsRecheck`]: the supersessions and retractions in its
    /// dependency closure that too few of its passes had seen.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<ContributionId>,
}

/// A place where the graph holds incompatible positions at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Conflict {
    /// Several current contributions each supersede `target`, and none of
    /// them supersedes the others.
    Divergence { target: ContributionId, successors: Vec<ContributionId> },
    /// A relation says `a` contradicts `b`, and both are current.
    Contradiction { relation: ContributionId, a: ContributionId, b: ContributionId },
}

#[derive(Debug, Clone, Serialize)]
pub struct Evaluation {
    /// Accepted contributions, in causal order.
    pub accepted: Vec<Assessment>,
    pub conflicts: Vec<Conflict>,
}

impl Policy {
    fn counts(&self, subject_author: AgentId, evidence_author: AgentId) -> bool {
        evidence_author != subject_author && self.validators.as_ref().is_none_or(|v| v.contains(&evidence_author))
    }

    /// Assesses one claim or artifact. Returns `None` for other kinds.
    pub fn assess(&self, graph: &Graph, id: &ContributionId) -> Option<Assessment> {
        self.assessor(graph).assess(id)
    }

    /// The accepted layer: what this policy treats as established, and where
    /// established contributions conflict.
    pub fn evaluate(&self, graph: &Graph) -> Evaluation {
        self.assessor(graph).evaluate()
    }

    /// Assesses many contributions of `graph`, sharing the work between them.
    /// Use one to assess a whole graph: under [`Policy::follow_changes`] each
    /// subject asks which changes its passes had seen, and the assessor
    /// answers for all of them in one sweep of the graph.
    pub fn assessor<'a>(&'a self, graph: &'a Graph) -> Assessor<'a> {
        Assessor { policy: self, graph, bases: RefCell::default(), seen: OnceCell::new() }
    }
}

/// Assessments under one [`Policy`] of one [`Graph`]; see [`Policy::assessor`].
pub struct Assessor<'a> {
    policy: &'a Policy,
    graph: &'a Graph,
    bases: RefCell<HashMap<ContributionId, Option<Rc<Base>>>>,
    seen: OnceCell<Seen>,
}

impl Assessor<'_> {
    /// Status before supersession is taken into account, and the changes that
    /// make it [`Status::NeedsRecheck`].
    fn base(&self, id: &ContributionId) -> Option<Rc<Base>> {
        if let Some(b) = self.bases.borrow().get(id) {
            return b.clone();
        }
        let b = self.compute_base(id).map(Rc::new);
        self.bases.borrow_mut().insert(*id, b.clone());
        b
    }

    fn compute_base(&self, id: &ContributionId) -> Option<Base> {
        let (graph, policy) = (self.graph, self.policy);
        let subject = graph.get(id).filter(|c| c.body().is_subject())?;
        let (mut passes, mut fails) = (Vec::new(), Vec::new());
        let mut pass_authors = HashSet::new();
        for r in graph.referrers(id) {
            let Body::Evidence { target, verdict, .. } = r.body() else { continue };
            if target != id || !policy.counts(subject.author(), r.author()) || graph.is_retracted(&r.id()) {
                continue;
            }
            match verdict {
                Verdict::Pass => {
                    pass_authors.insert(r.author());
                    passes.push(r.id());
                }
                Verdict::Fail => fails.push(r.id()),
            }
        }
        let mut status = if graph.is_retracted(id) {
            Status::Retracted
        } else if !fails.is_empty() {
            Status::Rejected
        } else if pass_authors.len() >= policy.min_passes {
            Status::Accepted
        } else {
            Status::Pending
        };
        let mut unseen = Vec::new();
        if policy.follow_changes && status == Status::Accepted {
            let seen = self.seen.get_or_init(|| Seen::sweep(graph));
            let changes = seen.closure_changes(id);
            if changes.iter().any(|w| *w != 0) {
                // Per pass author: whether one of their passes had seen every change, and which changes any had.
                let mut by_author: HashMap<AgentId, (bool, Vec<u64>)> = HashMap::new();
                for e in &passes {
                    let had = seen.by_pass.get(e).expect("every pass is swept");
                    let author = graph.get(e).expect("counted evidence is in the graph").author();
                    let entry = by_author.entry(author).or_insert_with(|| (false, vec![0; changes.len()]));
                    entry.0 |= changes.iter().zip(had).all(|(c, h)| c & !h == 0);
                    entry.1.iter_mut().zip(had).for_each(|(a, h)| *a |= h);
                }
                // Distinct authors of passes that had seen every change: those still speak for it.
                if by_author.values().filter(|(all, _)| *all).count() < policy.min_passes {
                    status = Status::NeedsRecheck;
                    // The changes too few authors had seen; all of them if each was seen by enough, but not together.
                    let ids = || (0..seen.all.len()).filter(|i| bit(&changes, *i));
                    let seen_by = |i: usize| by_author.values().filter(|(_, had)| bit(had, i)).count();
                    unseen = ids().filter(|i| seen_by(*i) < policy.min_passes).map(|i| seen.all[i]).collect();
                    if unseen.is_empty() {
                        unseen = ids().map(|i| seen.all[i]).collect();
                    }
                }
            }
        }
        Some(Base { status, passes, fails, changes: unseen })
    }

    /// Assesses one claim or artifact. Returns `None` for other kinds.
    pub fn assess(&self, id: &ContributionId) -> Option<Assessment> {
        let base = self.base(id)?;
        let mut superseded_by = Vec::new();
        // A replacement that needs a recheck still replaces: the version it retired doesn't come back, and
        // a superseded version is superseded, not work to recheck.
        let stands = |s: Status| matches!(s, Status::Accepted | Status::NeedsRecheck);
        if stands(base.status) {
            superseded_by = supersessors(self.graph, id, |s| self.base(s).is_some_and(|b| stands(b.status)));
        }
        let Base { status, passes, fails, changes } = Base::clone(&base);
        let (status, changes) = if superseded_by.is_empty() { (status, changes) } else { (Status::Superseded, vec![]) };
        Some(Assessment { id: *id, status, passes, fails, superseded_by, changes })
    }

    /// The accepted layer: what the policy treats as established, and where
    /// established contributions conflict.
    pub fn evaluate(&self) -> Evaluation {
        let order = self.graph.topo_order();
        let accepted_base: HashSet<ContributionId> =
            order.iter().filter(|id| self.base(id).is_some_and(|b| b.status == Status::Accepted)).copied().collect();
        let accepted = order
            .iter()
            .filter(|id| accepted_base.contains(id))
            .filter_map(|id| self.assess(id))
            .filter(|a| a.status == Status::Accepted)
            .collect();
        Evaluation { accepted, conflicts: conflicts(self.graph, |id| accepted_base.contains(id)) }
    }
}

/// What following changes needs to know of a graph, worked out once per
/// assessor so that no subject repeats it. Sets of changes are bitsets over
/// `all`.
///
/// Asking `Graph::is_ancestor` for each pair of pass and change walks a pass's
/// whole history once per change, and walking each subject's closure repeats
/// the walk for every subject that shares a dependency; anyone can add passes,
/// changes and subjects (finding `73252cef945e`). Instead:
/// - one sweep in causal order carries, for 64 changes at a time, the changes
///   each contribution has seen, and keeps them for the passes:
///   `O(N · C / 64)` for `N` contributions and `C` changes;
/// - one walk of the dependency relations' strongly connected components,
///   successors first, gives the changes each node reaches, so a subject's
///   closure is the union over its direct dependencies.
struct Seen {
    all: Vec<ContributionId>,
    direct: HashMap<ContributionId, Vec<ContributionId>>,
    own: HashMap<ContributionId, Vec<u64>>,
    reach: HashMap<ContributionId, Rc<Vec<u64>>>,
    by_pass: HashMap<ContributionId, Vec<u64>>,
}

impl Seen {
    fn sweep(graph: &Graph) -> Self {
        let order = graph.topo_order();
        let nonempty = |(id, v): (&ContributionId, Vec<ContributionId>)| (!v.is_empty()).then_some((*id, v));
        let direct: HashMap<_, _> =
            order.iter().map(|n| (n, direct_dependencies(graph, n))).filter_map(nonempty).collect();
        let changes: HashMap<_, _> = order.iter().map(|d| (d, changes(graph, d))).filter_map(nonempty).collect();
        let mut all: Vec<ContributionId> = changes.values().flatten().copied().collect();
        all.sort();
        all.dedup();
        let words = all.len().div_ceil(64);
        let index: HashMap<ContributionId, usize> = all.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let own: HashMap<ContributionId, Vec<u64>> = changes
            .iter()
            .map(|(d, cs)| {
                let mut bits = vec![0; words];
                cs.iter().for_each(|c| bits[index[c] / 64] |= 1 << (index[c] % 64));
                (*d, bits)
            })
            .collect();
        let mut by_pass: HashMap<ContributionId, Vec<u64>> = graph
            .iter()
            .filter(|c| matches!(c.body(), Body::Evidence { verdict: Verdict::Pass, .. }))
            .map(|c| (c.id(), vec![0; words]))
            .collect();
        for (word, chunk) in all.chunks(64).enumerate() {
            let bits: HashMap<ContributionId, u64> = chunk.iter().enumerate().map(|(i, c)| (*c, 1 << i)).collect();
            // What each contribution has seen of this chunk, kept only where it is something.
            let mut seen: HashMap<ContributionId, u64> = HashMap::new();
            for id in &order {
                let parents = graph.get(id).expect("the order lists the graph").parents();
                let mask = parents
                    .iter()
                    .fold(0, |m, p| m | seen.get(p).copied().unwrap_or(0) | bits.get(p).copied().unwrap_or(0));
                if mask != 0 {
                    seen.insert(*id, mask);
                }
            }
            for (e, w) in &mut by_pass {
                w[word] = seen.get(e).copied().unwrap_or(0);
            }
        }
        let reach = reach(&direct, &own, words);
        Seen { all, direct, own, reach, by_pass }
    }

    /// The changes in `x`'s dependency closure: what its direct dependencies
    /// reach. `x`'s own changes aren't among them, even on a cycle back to `x`,
    /// as [`dependencies`] doesn't count `x`.
    fn closure_changes(&self, x: &ContributionId) -> Vec<u64> {
        let mut bits = vec![0; self.all.len().div_ceil(64)];
        for d in self.direct.get(x).into_iter().flatten() {
            if let Some(r) = self.reach.get(d) {
                bits.iter_mut().zip(r.iter()).for_each(|(b, r)| *b |= r);
            }
        }
        if let Some(own) = self.own.get(x) {
            bits.iter_mut().zip(own).for_each(|(b, o)| *b &= !o);
        }
        bits
    }
}

/// For each node of the dependency relations `direct`, the changes it reaches,
/// its own included. Tarjan's algorithm finds the strongly connected
/// components successors first, so each component's set is its members' own
/// changes and its successors' sets, and the members of a cycle share one.
fn reach(
    direct: &HashMap<ContributionId, Vec<ContributionId>>,
    own: &HashMap<ContributionId, Vec<u64>>,
    words: usize,
) -> HashMap<ContributionId, Rc<Vec<u64>>> {
    let none = Vec::new();
    let next = |v: &ContributionId| direct.get(v).unwrap_or(&none);
    let mut out: HashMap<ContributionId, Rc<Vec<u64>>> = HashMap::new();
    // Each node's visit number, and the lowest number it reaches on the stack.
    let (mut number, mut low): (HashMap<ContributionId, usize>, HashMap<ContributionId, usize>) = Default::default();
    let (mut stack, mut on_stack) = (Vec::new(), HashSet::new());
    let mut roots: Vec<&ContributionId> = direct.keys().collect();
    roots.sort();
    for root in roots {
        if number.contains_key(root) {
            continue;
        }
        let mut calls: Vec<(ContributionId, usize)> = Vec::new();
        let mut enter = Some(*root);
        loop {
            if let Some(v) = enter.take() {
                let n = number.len();
                number.insert(v, n);
                low.insert(v, n);
                stack.push(v);
                on_stack.insert(v);
                calls.push((v, 0));
            }
            let Some(&(v, i)) = calls.last() else { break };
            if let Some(w) = next(&v).get(i) {
                calls.last_mut().expect("just read").1 += 1;
                if !number.contains_key(w) {
                    enter = Some(*w);
                } else if on_stack.contains(w) {
                    low.insert(v, low[&v].min(number[w]));
                }
                continue;
            }
            calls.pop();
            if let Some(&(u, _)) = calls.last() {
                low.insert(u, low[&u].min(low[&v]));
            }
            if low[&v] == number[&v] {
                let mut members = Vec::new();
                loop {
                    let w = stack.pop().expect("v is on the stack");
                    on_stack.remove(&w);
                    members.push(w);
                    if w == v {
                        break;
                    }
                }
                let mut bits = vec![0; words];
                for m in &members {
                    let theirs = own.get(m).into_iter().chain(next(m).iter().filter_map(|s| out.get(s).map(|r| &**r)));
                    theirs.for_each(|o| bits.iter_mut().zip(o).for_each(|(b, o)| *b |= o));
                }
                let bits = Rc::new(bits);
                members.into_iter().for_each(|m| drop(out.insert(m, bits.clone())));
            }
        }
    }
    out
}

fn bit(bits: &[u64], i: usize) -> bool {
    bits[i / 64] >> (i % 64) & 1 == 1
}

/// A subject's status before supersession, its counted evidence, and what makes it
/// [`Status::NeedsRecheck`].
#[derive(Clone)]
struct Base {
    status: Status,
    passes: Vec<ContributionId>,
    fails: Vec<ContributionId>,
    changes: Vec<ContributionId>,
}

/// The dependency closure of `x`, not counting `x`: what it reaches through
/// non-retracted `depends_on`, `implements` and `refines` relations, from `from`
/// to `to`, that count (decision `2b91a9541aba`, security note `9769ab37127d`).
/// At every node `n` of the walk, an `implements` relation counts only if `n`'s
/// author wrote it, and a `depends_on` or `refines` relation only if its author
/// is `n`'s author or the author of a claim `n` implements through such an
/// `implements`. So no one else can make a node depend on anything, or stand in
/// a chain they didn't write.
pub fn dependencies(graph: &Graph, x: &ContributionId) -> Vec<ContributionId> {
    let mut seen: HashSet<ContributionId> = HashSet::from([*x]);
    let mut out = Vec::new();
    let mut stack = vec![*x];
    while let Some(n) = stack.pop() {
        for d in direct_dependencies(graph, &n) {
            if seen.insert(d) {
                out.push(d);
                stack.push(d);
            }
        }
    }
    out.sort();
    out
}

/// The dependencies of `n` its counting relations name directly.
fn direct_dependencies(graph: &Graph, n: &ContributionId) -> Vec<ContributionId> {
    let Some(author) = graph.get(n).map(|c| c.author()) else { return Vec::new() };
    let out_of = |kind: RelationKind| {
        graph.referrers(n).filter_map(move |r| match r.body() {
            Body::Relation { from, kind: k, to, .. } if from == n && *k == kind && !graph.is_retracted(&r.id()) => {
                Some((r.author(), *to))
            }
            _ => None,
        })
    };
    let implements: Vec<ContributionId> =
        out_of(RelationKind::Implements).filter(|(a, _)| *a == author).map(|(_, to)| to).collect();
    let mut trusted: HashSet<AgentId> = implements.iter().filter_map(|c| graph.get(c)).map(|c| c.author()).collect();
    trusted.insert(author);
    let mut out = implements;
    for kind in [RelationKind::DependsOn, RelationKind::Refines] {
        out.extend(out_of(kind).filter(|(a, _)| trusted.contains(a)).map(|(_, to)| to));
    }
    out
}

/// What changed `d`: the non-retracted `supersedes` relations by `d`'s author
/// whose replacement is live, and the retractions that withdraw `d`.
fn changes(graph: &Graph, d: &ContributionId) -> Vec<ContributionId> {
    let Some(author) = graph.get(d).map(|c| c.author()) else { return Vec::new() };
    let retracted = graph.is_retracted(d);
    graph
        .referrers(d)
        .filter(|r| !graph.is_retracted(&r.id()))
        .filter(|r| match r.body() {
            Body::Relation { from, kind: RelationKind::Supersedes, to, .. } => {
                to == d && r.author() == author && open(graph, from)
            }
            Body::Retract { target, .. } => retracted && target == d,
            _ => false,
        })
        .map(|r| r.id())
        .collect()
}

/// The exploration layer's conflicts: everything not retracted is in play.
pub fn open_conflicts(graph: &Graph) -> Vec<Conflict> {
    conflicts(graph, |id| open(graph, id))
}

/// Live subjects in the exploration layer that supersede `target`. A subject
/// is current there when it is live and this is empty.
pub fn open_supersessors(graph: &Graph, target: &ContributionId) -> Vec<ContributionId> {
    supersessors(graph, target, |id| open(graph, id))
}

/// Whether `id` is live in the exploration layer: a subject not retracted.
fn open(graph: &Graph, id: &ContributionId) -> bool {
    graph.get(id).is_some_and(|c| c.body().is_subject()) && !graph.is_retracted(id)
}

/// Contributions in `live` that supersede `target`. A supersedes relation
/// only counts when its author also wrote `target`, so only a contribution's
/// own author can retire it, whoever wrote the replacement; and only while
/// the relation is not retracted.
fn supersessors(graph: &Graph, target: &ContributionId, live: impl Fn(&ContributionId) -> bool) -> Vec<ContributionId> {
    let Some(target_author) = graph.get(target).map(|t| t.author()) else { return Vec::new() };
    let mut out: Vec<ContributionId> = graph
        .referrers(target)
        .filter_map(|r| match r.body() {
            Body::Relation { from, kind: RelationKind::Supersedes, to, .. }
                if to == target && r.author() == target_author && !graph.is_retracted(&r.id()) && live(from) =>
            {
                Some(*from)
            }
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn conflicts(graph: &Graph, live: impl Fn(&ContributionId) -> bool) -> Vec<Conflict> {
    let order = graph.topo_order();
    let mut successors: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
    for id in &order {
        let s = supersessors(graph, id, &live);
        if !s.is_empty() {
            successors.insert(*id, s);
        }
    }
    let current = |id: &ContributionId| live(id) && !successors.contains_key(id);

    let mut out = Vec::new();
    for id in &order {
        if let Some(s) = successors.get(id) {
            let heads: Vec<ContributionId> = s.iter().filter(|x| current(x)).copied().collect();
            if heads.len() > 1 {
                out.push(Conflict::Divergence { target: *id, successors: heads });
            }
        }
        if let Some(Body::Relation { from, kind: RelationKind::Contradicts, to, .. }) = graph.get(id).map(|c| c.body())
            && !graph.is_retracted(id)
            && current(from)
            && current(to)
        {
            out.push(Conflict::Contradiction { relation: *id, a: *from, b: *to });
        }
    }
    out
}
