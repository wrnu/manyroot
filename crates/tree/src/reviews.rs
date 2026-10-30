//! Goals and reviews (docs/design/weave.md, section 1 *Goals, checks and
//! objections*).
//!
//! A goal is a claim tagged `weave-goal` that `refines` a line's first artifact,
//! and binds the line under a policy whose `goal_setters` include its author and
//! the author of that `refines` link, so no one else can bind a setter's goal by
//! linking it (task 8ff95e0b756f). A
//! review is a `supports` (agreement) or `contradicts` (objection) relation to a
//! decision, a manifest artifact, or a proposal. One to a manifest artifact
//! reviews every decision and proposal with that manifest hash; one to a proposal
//! reviews every proposal on the same line with the same `change`, so it survives
//! a rebase that leaves the change as it was, and not one that changes it.
//!
//! A review's target resolves to what it names whether or not the target is
//! retracted: a decision by its id, a manifest artifact by its manifest hash, a
//! proposal by its line and change. So retracting the artifact a review targets,
//! while the manifest or the change stands through another, clears nothing
//! (finding 06bbc6a34636). Whether the review itself, or a goal, is retracted
//! follows the protocol's rule: now by [`Graph::is_retracted`](manyroot::Graph::is_retracted),
//! within a causal past by [`Retractions::within`](crate::ground::Retractions::within).

use std::collections::HashMap;

use manyroot::{AgentId, ContributionId};

use crate::format::Line;
use crate::ground::{Index, Review};
use crate::hash::ContentHash;
use crate::lines::Lines;

/// A goal set on a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    /// The claim tagged `weave-goal`.
    pub id: ContributionId,
    pub author: AgentId,
    /// Its `refines` relation to the line's first artifact.
    pub link: ContributionId,
    /// The relation's author.
    pub linker: AgentId,
}

impl Goal {
    /// Whether the goal binds its line under `policy`: its author, and its link's, are goal setters.
    pub fn binds(&self, policy: &Line) -> bool {
        policy.goal_setters.contains(&self.author) && policy.goal_setters.contains(&self.linker)
    }
}

/// The goals set on a line, by the line's first artifact, in id order of their relations, leaving out
/// those retracted now: the view of now. Judging a past decision reads [`Index::goal_links`] with
/// [`crate::ground::GoalLink::retracted_within`] instead, within that decision's past.
pub fn goals(ix: &Index, line: &ContributionId) -> Vec<Goal> {
    ix.goal_links()
        .iter()
        .filter(|link| link.to == *line && !link.retracted)
        .map(|link| Goal { id: link.goal, author: link.author, link: link.id, linker: link.linker })
        .collect()
}

/// What a review targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A decision, retracted or not.
    Decision(ContributionId),
    /// A manifest artifact, retracted or not, with its manifest's content hash.
    Manifest(ContributionId, ContentHash),
    /// A proposal, retracted or not, valid or not, with its line and change.
    Proposal { id: ContributionId, line: ContributionId, change: ContentHash },
}

impl Target {
    /// What a review's `to` names: a decision that decodes, a proposal that decodes, or an artifact of a
    /// valid manifest, each retracted or not. `None` for anything else.
    pub fn of(ix: &Index, lines: &Lines, to: &ContributionId) -> Option<Target> {
        if lines.decision(to).is_some() {
            Some(Target::Decision(*to))
        } else if let Some(p) = lines.proposal_named(to) {
            Some(Target::Proposal { id: *to, line: p.line, change: p.change })
        } else {
            ix.manifest_named(to).map(|hash| Target::Manifest(*to, hash))
        }
    }
}

/// Every review of a Weave decision, manifest artifact or proposal in a replica, by what it targets.
#[derive(Debug, Default)]
pub struct Reviews {
    reviews: Vec<Review>,
    targets: Vec<Target>,
    /// Reviews, by index, of each decision.
    of_decision: HashMap<ContributionId, Vec<usize>>,
    /// Reviews of each manifest hash, through any of its artifacts.
    of_manifest: HashMap<ContentHash, Vec<usize>>,
    /// Reviews of each proposal's line and change, through any proposal with them.
    of_change: HashMap<(ContributionId, ContentHash), Vec<usize>>,
}

impl Reviews {
    pub fn build(ix: &Index, lines: &Lines) -> Reviews {
        let mut reviews = Reviews::default();
        // The view of now: reviews retracted now are left out, whatever their target is. Judging a past
        // decision reads [`Index::reviews`] with [`Target::of`] and [`Retractions::within`] instead,
        // within that decision's past.
        //
        // [`Retractions::within`]: crate::ground::Retractions::within
        for review in ix.reviews().iter().filter(|r| !r.retracted) {
            let Some(target) = Target::of(ix, lines, &review.to) else { continue };
            let at = reviews.reviews.len();
            match target {
                Target::Decision(d) => reviews.of_decision.entry(d).or_default().push(at),
                Target::Manifest(_, hash) => reviews.of_manifest.entry(hash).or_default().push(at),
                Target::Proposal { line, change, .. } => reviews.of_change.entry((line, change)).or_default().push(at),
            }
            reviews.reviews.push(review.clone());
            reviews.targets.push(target);
        }
        reviews
    }

    fn collect(&self, lists: &[Option<&Vec<usize>>]) -> Vec<(&Review, Target)> {
        let mut at: Vec<usize> = lists.iter().flatten().flat_map(|l| l.iter().copied()).collect();
        at.sort_unstable();
        at.dedup();
        at.into_iter().map(|i| (&self.reviews[i], self.targets[i])).collect()
    }

    /// The reviews of a decision, retracted or not: of the decision itself, and of any manifest artifact
    /// with its manifest hash; in id order.
    pub fn of_decision(&self, lines: &Lines, decision: &ContributionId) -> Vec<(&Review, Target)> {
        let Some(d) = lines.decision(decision) else { return vec![] };
        self.collect(&[self.of_decision.get(decision), self.of_manifest.get(&d.decision.manifest)])
    }

    /// The reviews of a proposal, retracted or not: of any proposal on the same line with the same
    /// change, retracted or not, so they carry over a rebase that leaves the change as it was, and of any
    /// manifest artifact with its manifest hash; in id order.
    pub fn of_proposal(&self, lines: &Lines, proposal: &ContributionId) -> Vec<(&Review, Target)> {
        let Some(p) = lines.proposal_named(proposal) else { return vec![] };
        self.collect(&[self.of_change.get(&(p.line, p.change)), self.of_manifest.get(&p.manifest)])
    }

    /// The reviews of a manifest, through any of its artifacts; in id order.
    pub fn of_manifest(&self, manifest: &ContentHash) -> Vec<(&Review, Target)> {
        self.collect(&[self.of_manifest.get(manifest)])
    }
}
