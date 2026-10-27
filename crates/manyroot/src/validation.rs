//! Proof: deciding which contributions to treat as accepted.
//!
//! Acceptance is never replicated. Every replica holds the same evidence, and
//! each reader applies its own [`Policy`] to it, so two communities can share
//! a Ground and still disagree about what is established. The graph
//! synchronizes evidence; it does not enforce consensus.

use std::collections::{BTreeSet, HashMap, HashSet};

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
}

impl Default for Policy {
    fn default() -> Self {
        Self { min_passes: 1, validators: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Meets the policy and nothing accepted replaces it.
    Accepted,
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

    /// Status before supersession is taken into account.
    fn base(&self, graph: &Graph, id: &ContributionId) -> Option<(Status, Vec<ContributionId>, Vec<ContributionId>)> {
        let subject = graph.get(id).filter(|c| c.body().is_subject())?;
        let (mut passes, mut fails) = (Vec::new(), Vec::new());
        let mut pass_authors = HashSet::new();
        for r in graph.referrers(id) {
            let Body::Evidence { target, verdict, .. } = r.body() else { continue };
            if target != id || !self.counts(subject.author(), r.author()) || graph.is_retracted(&r.id()) {
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
        let status = if graph.is_retracted(id) {
            Status::Retracted
        } else if !fails.is_empty() {
            Status::Rejected
        } else if pass_authors.len() >= self.min_passes {
            Status::Accepted
        } else {
            Status::Pending
        };
        Some((status, passes, fails))
    }

    /// Assesses one claim or artifact. Returns `None` for other kinds.
    pub fn assess(&self, graph: &Graph, id: &ContributionId) -> Option<Assessment> {
        let (status, passes, fails) = self.base(graph, id)?;
        let mut superseded_by = Vec::new();
        if status == Status::Accepted {
            superseded_by =
                supersessors(graph, id, |s| self.base(graph, s).is_some_and(|(st, ..)| st == Status::Accepted));
        }
        let status = if superseded_by.is_empty() { status } else { Status::Superseded };
        Some(Assessment { id: *id, status, passes, fails, superseded_by })
    }

    /// The accepted layer: what this policy treats as established, and where
    /// established contributions conflict.
    pub fn evaluate(&self, graph: &Graph) -> Evaluation {
        let order = graph.topo_order();
        let accepted_base: HashSet<ContributionId> = order
            .iter()
            .filter(|id| self.base(graph, id).is_some_and(|(st, ..)| st == Status::Accepted))
            .copied()
            .collect();
        let accepted = order
            .iter()
            .filter(|id| accepted_base.contains(id))
            .filter_map(|id| self.assess(graph, id))
            .filter(|a| a.status == Status::Accepted)
            .collect();
        Evaluation { accepted, conflicts: conflicts(graph, |id| accepted_base.contains(id)) }
    }
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
/// only counts when its author also wrote the superseding contribution, and
/// it has not been retracted.
fn supersessors(graph: &Graph, target: &ContributionId, live: impl Fn(&ContributionId) -> bool) -> Vec<ContributionId> {
    let mut out: Vec<ContributionId> = graph
        .referrers(target)
        .filter_map(|r| match r.body() {
            Body::Relation { from, kind: RelationKind::Supersedes, to, .. }
                if to == target
                    && graph.get(from).is_some_and(|f| f.author() == r.author())
                    && !graph.is_retracted(&r.id())
                    && live(from) =>
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
