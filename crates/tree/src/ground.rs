//! Weave's records in a Ground (docs/design/weave.md, sections 1 and 3): file
//! versions and manifests by content hash, the marker that counts, what each
//! contribution is, and what `weave check` lists.
//!
//! Retracted contributions are left out of every view. An invalid payload makes
//! no contribution invalid: Weave just ignores it, and [`Index::problems`] lists
//! it.

use std::collections::{BTreeMap, BTreeSet};

use manyroot::{Body, Contribution, ContributionId, Graph};

use crate::format::{
    DECISION_MEDIA_TYPE, Decision, FILE_MEDIA_TYPE, FormatError, LINE_MEDIA_TYPE, Line, MANIFEST_MEDIA_TYPE,
    MARKER_MEDIA_TYPE, MEDIA_TYPE_PREFIX, Manifest, Marker, PROPOSAL_MEDIA_TYPE, Proposal, RESOLUTION_MEDIA_TYPE,
    Record, Resolution,
};
use crate::hash::ContentHash;
use crate::merge::Resolutions;

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
    /// File versions by content hash: artifact ids, ascending.
    files: BTreeMap<ContentHash, Vec<ContributionId>>,
    /// Each held file version's size, by content hash: the same bytes under every id.
    sizes: BTreeMap<ContentHash, u64>,
    /// Valid manifests by content hash: artifact ids, ascending, and the manifest.
    manifests: BTreeMap<ContentHash, (Vec<ContributionId>, Manifest)>,
    /// Valid markers, ascending.
    markers: Vec<ContributionId>,
    /// Valid resolutions, by key.
    resolutions: Resolutions,
    /// The marker that counts.
    counted: Option<ContributionId>,
    problems: Vec<Problem>,
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
        for id in ids {
            let c = g.get(&id).expect("listed by the graph");
            let Body::Artifact { media_type, content, .. } = c.body() else { continue };
            if !media_type.starts_with(MEDIA_TYPE_PREFIX) || g.is_retracted(&id) {
                continue;
            }
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
                match Manifest::decode(content) {
                    Ok(manifest) => {
                        index.manifests.insert(hash, (vec![id], manifest));
                    }
                    Err(error) => {
                        index.problems.push(Problem::InvalidPayload { id, media_type: media_type.clone(), error })
                    }
                }
                continue;
            }
            match references(media_type, content) {
                Err(error) => {
                    index.problems.push(Problem::InvalidPayload { id, media_type: media_type.clone(), error });
                    continue;
                }
                Ok(refs) => {
                    for reference in refs {
                        if !g.is_ancestor(&reference, &id) {
                            index.problems.push(Problem::NotAnAncestor { id, reference });
                        }
                    }
                }
            }
            if media_type == MARKER_MEDIA_TYPE {
                index.markers.push(id);
            }
            if media_type == RESOLUTION_MEDIA_TYPE {
                index.resolutions.insert(id, Resolution::decode(content).expect("decoded above"));
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
        index.counted =
            index.markers.iter().find(|m| !index.markers.iter().any(|other| g.is_ancestor(other, m))).copied();
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
