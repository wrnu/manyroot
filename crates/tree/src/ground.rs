//! Weave's records in a Ground (docs/design/weave.md, sections 1 and 3): file
//! versions and manifests by content hash, the marker that counts, what each
//! contribution is, and what `weave check` lists.
//!
//! Retracted records are left out of every view, except file versions and
//! manifests: they are content, named by hash, so they are held retracted or not.
//! An invalid payload makes no contribution invalid: Weave just ignores it, and
//! [`Index::problems`] lists it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};

use manyroot::{AgentId, Body, Contribution, ContributionId, Graph, RelationKind, Verdict};

use crate::ancestry::{Ancestry, Stats};
use crate::format::{
    DECISION_MEDIA_TYPE, Decision, FILE_MEDIA_TYPE, FormatError, GOAL_TAG, LINE_MEDIA_TYPE, Line, MANIFEST_MEDIA_TYPE,
    MARKER_MEDIA_TYPE, MEDIA_TYPE_PREFIX, Manifest, Marker, PROPOSAL_MEDIA_TYPE, Proposal, RESOLUTION_MEDIA_TYPE,
    Record, Resolution,
};
use crate::hash::ContentHash;
use crate::merge::Resolutions;
use crate::path::PathCache;
use crate::results::{CheckResult, parse_details};

/// Something `weave check` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A Weave payload that doesn't decode, or breaks its record's rules.
    InvalidPayload { id: ContributionId, media_type: String, error: FormatError },
    /// A reference inside a payload that isn't a causal ancestor of its record.
    NotAnAncestor { id: ContributionId, reference: ContributionId },
    /// A manifest whose files this replica doesn't all hold, at the sizes it names.
    Incomplete { id: ContributionId, hash: ContentHash, missing: Vec<String> },
    /// A valid marker other than the one that counts: written after it, or concurrent with it.
    ExtraMarker { id: ContributionId, counted: ContributionId },
}

/// An index of one replica's Weave records.
#[derive(Debug, Default)]
pub struct Index {
    /// File versions by content hash: artifact ids, retracted or not, ascending.
    files: BTreeMap<ContentHash, Vec<ContributionId>>,
    /// Each held file version's size, by content hash: the same bytes under every id.
    sizes: BTreeMap<ContentHash, u64>,
    /// Valid manifests by content hash: artifact ids, retracted or not, ascending, and the manifest.
    manifests: BTreeMap<ContentHash, (Vec<ContributionId>, Manifest)>,
    /// Valid markers, ascending.
    markers: Vec<ContributionId>,
    /// Valid resolutions, by key.
    resolutions: Resolutions,
    /// The marker that counts.
    counted: Option<ContributionId>,
    problems: Vec<Problem>,
    /// The answers to every record's references, and each marker's bit.
    ancestry: Ancestry,
    /// Questions [`Index::is_ancestor`] answered by walking, because nothing asked them at build.
    fallbacks: AtomicUsize,
    /// Check results: evidence on a valid manifest artifact whose method names a check, in id order.
    results: Vec<CheckResult>,
    /// Reviews: `supports` and `contradicts` relations, retracted or not, in id order. Which ones review
    /// a Weave record, and which retractions count, is for the views to say.
    reviews: Vec<Review>,
    /// Goals: `refines` relations from a claim tagged `weave-goal`, retracted or not.
    goals: Vec<GoalLink>,
    /// Each manifest artifact's content hash, retracted or not, if the manifest is valid.
    manifest_of: HashMap<ContributionId, ContentHash>,
    /// Every retraction, by its target.
    retractions: Retractions,
}

/// Every retraction in a replica, standing or not, by its target. A contribution is retracted if a
/// retraction of it isn't itself retracted (docs/protocol.md, *Retraction*), so restoring one by
/// retracting its retraction works here too (finding 1c584afefc50). The view of now is
/// [`Graph::is_retracted`]; [`Retractions::within`] reads the same rule within one causal past.
#[derive(Debug, Default)]
pub struct Retractions {
    of: HashMap<ContributionId, Vec<ContributionId>>,
}

impl Retractions {
    /// The retractions below `id`: of it, of those, and so on, in no set order. Judging `id` within a
    /// past asks only whether these are in it.
    pub fn below(&self, id: &ContributionId) -> Vec<ContributionId> {
        let mut below: Vec<ContributionId> = self.of.get(id).cloned().unwrap_or_default();
        let mut at = 0;
        while let Some(r) = below.get(at).copied() {
            below.extend(self.of.get(&r).into_iter().flatten());
            at += 1;
        }
        below
    }

    /// Whether `id` is retracted within a causal past: a retraction of it is in that past, and isn't
    /// itself retracted there. `in_past` is asked about retractions below `id`, and only those. A
    /// retraction's descendants are all later than it, so a past holding one holds what it retracts;
    /// no recursion, so a long chain of retractions can't overflow the stack.
    pub fn within(&self, id: &ContributionId, mut in_past: impl FnMut(&ContributionId) -> bool) -> bool {
        // The retractions in the past below `id`, each with the index of what it retracts, breadth
        // first: each comes after its target, so the reverse decides a retraction before its target.
        let mut found: Vec<(ContributionId, usize)> = vec![(*id, 0)];
        let mut at = 0;
        while let Some((x, _)) = found.get(at).copied() {
            for r in self.of.get(&x).into_iter().flatten() {
                if in_past(r) {
                    found.push((*r, at));
                }
            }
            at += 1;
        }
        let mut retracted = vec![false; found.len()];
        for at in (1..found.len()).rev() {
            if !retracted[at] {
                retracted[found[at].1] = true;
            }
        }
        retracted[0]
    }
}

/// A `supports` or `contradicts` relation: agreement, or an objection, with its reason in `note`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub id: ContributionId,
    pub author: AgentId,
    pub objection: bool,
    pub from: ContributionId,
    pub to: ContributionId,
    pub note: String,
    /// Retracted now, by the protocol's rule. Within a causal past, [`Retractions::within`] judges it
    /// (finding 140fde48d768).
    pub retracted: bool,
}

/// A goal claim's `refines` relation to what it refines, a line's first artifact if it sets a goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalLink {
    /// The `refines` relation.
    pub id: ContributionId,
    /// The claim tagged `weave-goal`.
    pub goal: ContributionId,
    /// The goal claim's author.
    pub author: AgentId,
    /// The `refines` relation's author: a goal binds only through a link a goal setter wrote (task
    /// 8ff95e0b756f).
    pub linker: AgentId,
    pub to: ContributionId,
    /// The relation or the goal claim is retracted now, by the protocol's rule.
    pub retracted: bool,
}

impl GoalLink {
    /// Whether the relation or the goal claim is retracted within a causal past (finding 140fde48d768).
    pub fn retracted_within(
        &self,
        retractions: &Retractions,
        mut in_past: impl FnMut(&ContributionId) -> bool,
    ) -> bool {
        retractions.within(&self.id, &mut in_past) || retractions.within(&self.goal, &mut in_past)
    }
}

/// The ContributionIds a record's payload refers to, which must be its causal
/// ancestors (the ancestor rule).
fn references(media_type: &str, content: &[u8]) -> Result<Vec<ContributionId>, FormatError> {
    Ok(match media_type {
        MARKER_MEDIA_TYPE => Marker::decode(content)?.in_place.into_iter().collect(),
        LINE_MEDIA_TYPE => {
            Line::decode(content)?;
            Vec::new()
        }
        DECISION_MEDIA_TYPE => {
            let d = Decision::decode(content)?;
            let mut refs = vec![d.line];
            refs.extend(d.parents);
            refs.extend(d.proposals);
            refs.extend(d.answers.iter().map(|a| a.objection));
            refs.extend(d.resolutions);
            refs.extend(d.goals);
            refs
        }
        PROPOSAL_MEDIA_TYPE => {
            let p = Proposal::decode(content)?;
            vec![p.line, p.base]
        }
        RESOLUTION_MEDIA_TYPE => {
            Resolution::decode(content)?;
            Vec::new()
        }
        _ => return Err(FormatError::Malformed(format!("unknown Weave media type {media_type:?}"))),
    })
}

impl Index {
    pub fn build(g: &Graph) -> Index {
        let mut index = Index::default();
        let mut ids: Vec<ContributionId> = g.iter().map(|c| c.id()).collect();
        ids.sort();
        let mut paths = PathCache::new();
        // Each record's problem, or its references, in id order; the references are checked once every
        // record's are known.
        let mut found: Vec<Result<ContributionId, Problem>> = Vec::new();
        let mut refs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        // Evidence, each with its target; kept as results once the valid manifests are known.
        let mut evidence: Vec<(ContributionId, ContributionId)> = Vec::new();
        for id in ids {
            let c = g.get(&id).expect("listed by the graph");
            match c.body() {
                Body::Evidence { target, .. } => evidence.push((id, *target)),
                Body::Retract { target, .. } => index.retractions.of.entry(*target).or_default().push(id),
                Body::Relation {
                    kind: kind @ (RelationKind::Supports | RelationKind::Contradicts),
                    from,
                    to,
                    note,
                } => {
                    let objection = *kind == RelationKind::Contradicts;
                    let (from, to, note) = (*from, *to, note.clone());
                    let retracted = g.is_retracted(&id);
                    index.reviews.push(Review { id, author: c.author(), objection, from, to, note, retracted });
                }
                Body::Relation { kind: RelationKind::Refines, from, to, .. } => {
                    if let Some(goal) = g.get(from)
                        && matches!(goal.body(), Body::Claim { tags, .. } if tags.iter().any(|t| t == GOAL_TAG))
                    {
                        let retracted = g.is_retracted(&id) || g.is_retracted(from);
                        let (author, linker) = (goal.author(), c.author());
                        index.goals.push(GoalLink { id, goal: *from, author, linker, to: *to, retracted });
                    }
                }
                _ => {}
            }
            let Body::Artifact { media_type, content, .. } = c.body() else { continue };
            if !media_type.starts_with(MEDIA_TYPE_PREFIX) {
                continue;
            }
            // File versions and manifests are content, named by hash: retracting one artifact retracts
            // that artifact, not its content, which other artifacts may carry and decisions name. So they
            // are held retracted or not, and a later retraction never changes what a decision's manifest
            // resolves to (answer 2c28892e9701; v0 has no content takedown).
            let retracted = g.is_retracted(&id);
            if media_type == FILE_MEDIA_TYPE {
                let hash = ContentHash::of(content);
                index.files.entry(hash).or_default().push(id);
                index.sizes.insert(hash, content.len() as u64);
                continue;
            }
            // A manifest refers to no contribution, and is decoded once per content hash: the same
            // bytes are equally valid, or invalid, under every id.
            if media_type == MANIFEST_MEDIA_TYPE {
                let hash = ContentHash::of(content);
                if let Some((ids, _)) = index.manifests.get_mut(&hash) {
                    ids.push(id);
                    continue;
                }
                match Manifest::decode_with(content, &mut paths) {
                    Ok(manifest) => {
                        index.manifests.insert(hash, (vec![id], manifest));
                    }
                    Err(error) if !retracted => {
                        found.push(Err(Problem::InvalidPayload { id, media_type: media_type.clone(), error }))
                    }
                    Err(_) => {}
                }
                continue;
            }
            if retracted {
                continue;
            }
            match references(media_type, content) {
                Err(error) => {
                    found.push(Err(Problem::InvalidPayload { id, media_type: media_type.clone(), error }));
                    continue;
                }
                Ok(named) => {
                    refs.insert(id, named);
                    found.push(Ok(id));
                }
            }
            if media_type == MARKER_MEDIA_TYPE {
                index.markers.push(id);
            }
            if media_type == RESOLUTION_MEDIA_TYPE {
                index.resolutions.insert(id, Resolution::decode(content).expect("decoded above"));
            }
        }
        // Every manifest artifact names its manifest, retracted or not: results and reviews on it attach
        // to the manifest, so retracting one artifact clears none of them (findings 348e506fce4c and
        // 06bbc6a34636).
        index.manifest_of =
            index.manifests.iter().flat_map(|(hash, (ids, _))| ids.iter().map(|id| (*id, *hash))).collect();
        for (id, target) in evidence {
            let Some(manifest) = index.manifest_of.get(&target).copied() else { continue };
            let c = g.get(&id).expect("listed by the graph");
            let Body::Evidence { verdict, method, details, .. } = c.body() else { continue };
            if crate::format::parse_method(method).is_none() {
                continue;
            }
            index.results.push(CheckResult {
                id,
                author: c.author(),
                manifest,
                method: method.clone(),
                passed: *verdict == Verdict::Pass,
                attempt: parse_details(details).filter(|(m, _)| *m == manifest).map(|(_, attempt)| attempt),
                retracted: g.is_retracted(&id),
            });
        }
        index.ancestry = Ancestry::build(g, &refs, &index.markers);
        for record in found {
            match record {
                Err(problem) => index.problems.push(problem),
                Ok(id) => {
                    for reference in &refs[&id] {
                        if !index.is_ancestor(g, reference, &id) {
                            index.problems.push(Problem::NotAnAncestor { id, reference: *reference });
                        }
                    }
                }
            }
        }
        for (hash, (ids, manifest)) in &index.manifests {
            let missing = index.missing(manifest);
            if !missing.is_empty() {
                index.problems.push(Problem::Incomplete { id: ids[0], hash: *hash, missing });
            }
        }
        // Decision 049d02449a94: the marker that counts has no other valid marker in its causal past,
        // so one written after it has been seen never takes over. Among concurrent ones, the smallest id.
        index.counted = index.markers.iter().find(|m| index.ancestry.below_a_marker(m) == Some(false)).copied();
        if let Some(counted) = index.counted {
            for id in index.markers.iter().filter(|id| **id != counted) {
                index.problems.push(Problem::ExtraMarker { id: *id, counted });
            }
        }
        index
    }

    /// The marker that counts: the valid, non-retracted marker with no other such
    /// marker among its causal ancestors, the smallest id if several are concurrent.
    pub fn counted_marker(&self) -> Option<ContributionId> {
        self.counted
    }

    /// Whether `a` is a strict causal ancestor of `b`, where `b` is a valid record that names `a`: the
    /// answer built at open. Anything else is a missed question, a bug, answered by walking the graph
    /// and counted (answer b2dbc4f5e810).
    pub(crate) fn is_ancestor(&self, g: &Graph, a: &ContributionId, b: &ContributionId) -> bool {
        if !g.contains(a) {
            return false;
        }
        let answer = self.ancestry.is_ancestor(a, b);
        debug_assert!(answer.is_some(), "{b} didn't name {a} when the index was built");
        answer.unwrap_or_else(|| {
            self.fallbacks.fetch_add(1, Ordering::Relaxed);
            g.is_ancestor(a, b)
        })
    }

    /// What answering the references and markers did, and how many questions fell back to a walk since.
    #[doc(hidden)]
    pub fn ancestry_stats(&self) -> Stats {
        Stats { fallbacks: self.fallbacks.load(Ordering::Relaxed), ..self.ancestry.stats() }
    }

    /// Every check result, in id order: evidence on a valid manifest artifact whose method names a
    /// check, retracted or not.
    pub fn check_results(&self) -> &[CheckResult] {
        &self.results
    }

    /// Every `supports` and `contradicts` relation, retracted or not, in id order.
    pub fn reviews(&self) -> &[Review] {
        &self.reviews
    }

    /// Every `refines` relation from a goal claim, retracted or not, in id order.
    pub fn goal_links(&self) -> &[GoalLink] {
        &self.goals
    }

    /// The content hash a manifest artifact names, retracted or not, if the manifest is valid: what a
    /// result or a review on the artifact attaches to.
    pub fn manifest_named(&self, id: &ContributionId) -> Option<ContentHash> {
        self.manifest_of.get(id).copied()
    }

    /// Every retraction in the replica, by its target.
    pub fn retractions(&self) -> &Retractions {
        &self.retractions
    }

    /// Whether this replica holds a file version with this content hash.
    pub fn holds_file(&self, hash: &ContentHash) -> bool {
        self.files.contains_key(hash)
    }

    /// A file version's bytes, from the artifact with the smallest id.
    pub fn file<'g>(&self, g: &'g Graph, hash: &ContentHash) -> Option<&'g [u8]> {
        let id = self.files.get(hash)?.first()?;
        match g.get(id)?.body() {
            Body::Artifact { content, .. } => Some(content),
            _ => None,
        }
    }

    /// A valid manifest with this content hash, and its artifact with the smallest id.
    pub fn manifest(&self, hash: &ContentHash) -> Option<(ContributionId, &Manifest)> {
        self.manifests.get(hash).map(|(ids, m)| (ids[0], m))
    }

    /// The paths of `manifest` whose files this replica doesn't hold with the size
    /// the entry names. A manifest is complete when this is empty.
    pub fn missing(&self, manifest: &Manifest) -> Vec<String> {
        manifest.entries.iter().filter(|e| self.sizes.get(&e.hash) != Some(&e.size)).map(|e| e.path.clone()).collect()
    }

    /// What kind of Weave record has this content hash: `"manifest"` or `"file"`.
    pub fn hash_kind(&self, hash: &ContentHash) -> Option<&'static str> {
        if self.manifests.contains_key(hash) {
            Some("manifest")
        } else if self.files.contains_key(hash) {
            Some("file")
        } else {
            None
        }
    }

    /// Every valid resolution, by the key of the conflict it settles.
    pub fn resolutions(&self) -> &Resolutions {
        &self.resolutions
    }

    /// A held file version's size.
    pub fn file_size(&self, hash: &ContentHash) -> Option<u64> {
        self.sizes.get(hash).copied()
    }

    /// Every problem `weave check` lists, in the order found.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// Every content hash a file version is held for.
    pub fn file_hashes(&self) -> BTreeSet<ContentHash> {
        self.files.keys().copied().collect()
    }
}

/// What a contribution is, in the words commands use when refusing an id of the
/// wrong kind: a Weave record by name ("manifest", "file version", "decision"),
/// otherwise its body kind, with a relation's kind or an artifact's media type.
pub fn kind_of(c: &Contribution) -> String {
    match c.body() {
        Body::Artifact { media_type, .. } => match media_type.as_str() {
            FILE_MEDIA_TYPE => "file version".to_owned(),
            MANIFEST_MEDIA_TYPE => "manifest".to_owned(),
            MARKER_MEDIA_TYPE => "Weave marker".to_owned(),
            LINE_MEDIA_TYPE => "line".to_owned(),
            DECISION_MEDIA_TYPE => "decision".to_owned(),
            PROPOSAL_MEDIA_TYPE => "proposal".to_owned(),
            RESOLUTION_MEDIA_TYPE => "resolution".to_owned(),
            other => format!("artifact ({other})"),
        },
        Body::Relation { kind, .. } => format!("relation ({})", kind.as_str()),
        body => body.kind().as_str().to_owned(),
    }
}
