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

/// Something `weave check` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A Weave payload that doesn't decode, or breaks its record's rules.
    InvalidPayload { id: ContributionId, media_type: String, error: FormatError },
    /// A reference inside a payload that isn't a causal ancestor of its record.
    NotAnAncestor { id: ContributionId, reference: ContributionId },
    /// A manifest whose files this replica doesn't all hold.
    Incomplete { id: ContributionId, hash: ContentHash, missing: Vec<String> },
    /// A valid marker past the one that counts.
    ExtraMarker { id: ContributionId, counted: ContributionId },
}

/// An index of one replica's Weave records.
#[derive(Debug, Default)]
pub struct Index {
    /// File versions by content hash: artifact ids, ascending.
    files: BTreeMap<ContentHash, Vec<ContributionId>>,
    /// Valid manifests by content hash: artifact ids, ascending, and the manifest.
    manifests: BTreeMap<ContentHash, (Vec<ContributionId>, Manifest)>,
    /// Valid markers, ascending.
    markers: Vec<ContributionId>,
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
        MANIFEST_MEDIA_TYPE => {
            Manifest::decode(content)?;
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
                index.files.entry(ContentHash::of(content)).or_default().push(id);
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
            match media_type.as_str() {
                MANIFEST_MEDIA_TYPE => {
                    let manifest = Manifest::decode(content).expect("decoded above");
                    index
                        .manifests
                        .entry(ContentHash::of(content))
                        .or_insert_with(|| (Vec::new(), manifest))
                        .0
                        .push(id);
                }
                MARKER_MEDIA_TYPE => index.markers.push(id),
                _ => {}
            }
        }
        for (hash, (ids, manifest)) in &index.manifests {
            let missing = index.missing(manifest);
            if !missing.is_empty() {
                index.problems.push(Problem::Incomplete { id: ids[0], hash: *hash, missing });
            }
        }
        if let Some((counted, rest)) = index.markers.split_first() {
            for id in rest {
                index.problems.push(Problem::ExtraMarker { id: *id, counted: *counted });
            }
        }
        index
    }

    /// The valid, non-retracted marker with the smallest id: the one that counts.
    pub fn counted_marker(&self) -> Option<ContributionId> {
        self.markers.first().copied()
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

    /// The paths of `manifest` whose files this replica doesn't hold. A manifest is
    /// complete when this is empty.
    pub fn missing(&self, manifest: &Manifest) -> Vec<String> {
        manifest.entries.iter().filter(|e| !self.holds_file(&e.hash)).map(|e| e.path.clone()).collect()
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
