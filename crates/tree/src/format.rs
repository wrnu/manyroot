//! Weave's payload formats (docs/design/weave.md, *The records*).
//!
//! Every Weave record is an existing contribution kind: an artifact with one of
//! the media types below, whose payload is either a file's raw bytes or one of
//! the canonical CBOR records here. A record is valid only if decoding then
//! re-encoding it gives the same bytes (maps with text keys in the order listed,
//! integers in their shortest form, byte strings as major type 2), it starts with
//! `"v": 1`, and it keeps its own rules. An invalid payload doesn't make the
//! contribution invalid: Weave's views ignore it.
//!
//! These formats are pinned by golden vectors (`tests/format_guard.rs`).
//! Changing one is reviewed as protocol class.

use std::collections::BTreeMap;

use manyroot::{AgentId, ContributionId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::hash::ContentHash;
use crate::path::{self, Collision, PathCache, PathError};

/// The format version every record starts with.
pub const VERSION: u64 = 1;

pub const FILE_MEDIA_TYPE: &str = "application/vnd.bracken.weave.file";
pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.bracken.weave.manifest+cbor";
pub const MARKER_MEDIA_TYPE: &str = "application/vnd.bracken.weave.ground+cbor";
pub const LINE_MEDIA_TYPE: &str = "application/vnd.bracken.weave.line+cbor";
pub const DECISION_MEDIA_TYPE: &str = "application/vnd.bracken.weave.decision+cbor";
pub const PROPOSAL_MEDIA_TYPE: &str = "application/vnd.bracken.weave.proposal+cbor";
pub const RESOLUTION_MEDIA_TYPE: &str = "application/vnd.bracken.weave.resolution+cbor";

/// The prefix every Weave media type starts with.
pub const MEDIA_TYPE_PREFIX: &str = "application/vnd.bracken.weave";

/// The tag a goal claim carries.
pub const GOAL_TAG: &str = "weave-goal";

/// The most contributions one decision may refer to: its line, parents, proposals,
/// answered objections, resolutions and goals together. Every reference is checked
/// against the ancestor rule on each open, so an unbounded list would let one
/// decision make every open slow (finding 1873f4834edd).
pub const MAX_REFERENCES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("malformed payload: {0}")]
    Malformed(String),
    #[error("payload is not in canonical encoding")]
    NotCanonical,
    #[error("payload has format version {0}; this build reads version {VERSION}")]
    Version(u64),
    #[error("invalid path {path:?}: {error}")]
    Path { path: String, error: PathError },
    #[error("manifest paths collide: {0}")]
    Collision(Collision),
    #[error("manifest entries are not in strictly ascending path order at {0:?}")]
    EntryOrder(String),
    #[error("{0} is blank")]
    Blank(&'static str),
    #[error("check {0:?} has an empty argv")]
    EmptyArgv(String),
    #[error("{0} are not strictly ascending")]
    Order(&'static str),
    #[error("the decision refers to {0} contributions, over the limit of {MAX_REFERENCES}")]
    TooManyReferences(usize),
}

/// Canonical CBOR of any value: what every record and hashed list is written as.
pub fn to_cbor<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).expect("writing to a Vec cannot fail");
    bytes
}

/// Decodes canonical CBOR: exactly one value, with no trailing bytes and no
/// other encoding of the same value.
pub fn from_cbor<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T, FormatError> {
    let value: T = ciborium::from_reader(bytes).map_err(|e| FormatError::Malformed(e.to_string()))?;
    if to_cbor(&value) != bytes {
        return Err(FormatError::NotCanonical);
    }
    Ok(value)
}

/// A Weave record with a CBOR payload.
pub trait Record: Serialize + DeserializeOwned {
    const MEDIA_TYPE: &'static str;

    /// The format version the record carries.
    fn version(&self) -> u64;

    /// The record's own rules, beyond canonical encoding and the version.
    fn check(&self) -> Result<(), FormatError>;

    /// The artifact name the record is written under.
    fn artifact_name(&self) -> &str;

    fn validate(&self) -> Result<(), FormatError> {
        if self.version() != VERSION {
            return Err(FormatError::Version(self.version()));
        }
        self.check()
    }

    fn encode(&self) -> Vec<u8> {
        to_cbor(self)
    }

    /// Decodes a payload and checks it is valid.
    fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let record: Self = from_cbor(bytes)?;
        record.validate()?;
        Ok(record)
    }

    fn content_hash(&self) -> ContentHash {
        ContentHash::of(&self.encode())
    }
}

fn valid_path(path: &str) -> Result<(), FormatError> {
    path::validate_path(path).map_err(|error| FormatError::Path { path: path.to_owned(), error })
}

fn not_blank(text: &str, what: &'static str) -> Result<(), FormatError> {
    if text.trim().is_empty() { Err(FormatError::Blank(what)) } else { Ok(()) }
}

fn strictly_ascending<T: Ord>(items: &[T], what: &'static str) -> Result<(), FormatError> {
    if items.windows(2).all(|w| w[0] < w[1]) { Ok(()) } else { Err(FormatError::Order(what)) }
}

/// A file's mode, as git stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    /// `0o100644`
    File,
    /// `0o100755`
    Executable,
    /// `0o120000`: the file's bytes are the target.
    Symlink,
}

impl Mode {
    pub fn bits(self) -> u32 {
        match self {
            Mode::File => 0o100644,
            Mode::Executable => 0o100755,
            Mode::Symlink => 0o120000,
        }
    }

    pub fn from_bits(bits: u32) -> Option<Mode> {
        match bits {
            0o100644 => Some(Mode::File),
            0o100755 => Some(Mode::Executable),
            0o120000 => Some(Mode::Symlink),
            _ => None,
        }
    }
}

impl Serialize for Mode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.bits())
    }
}

impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Mode, D::Error> {
        let bits = u32::deserialize(deserializer)?;
        Mode::from_bits(bits).ok_or_else(|| serde::de::Error::custom(format!("unknown mode {bits:o}")))
    }
}

/// A file's value: its content hash and mode, `[hash, mode]`. Merges,
/// resolutions and manifests compare file values, never bare hashes. Values
/// are ordered by hash bytes, then mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileValue {
    pub hash: ContentHash,
    pub mode: Mode,
}

impl Serialize for FileValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.hash, self.mode).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FileValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<FileValue, D::Error> {
        let (hash, mode) = <(ContentHash, Mode)>::deserialize(deserializer)?;
        Ok(FileValue { hash, mode })
    }
}

/// One file of a manifest: `[path, mode, hash, size]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub mode: Mode,
    pub hash: ContentHash,
    pub size: u64,
}

impl Entry {
    pub fn value(&self) -> FileValue {
        FileValue { hash: self.hash, mode: self.mode }
    }
}

impl Serialize for Entry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.path, self.mode, self.hash, self.size).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Entry, D::Error> {
        let (path, mode, hash, size) = <(String, Mode, ContentHash, u64)>::deserialize(deserializer)?;
        Ok(Entry { path, mode, hash, size })
    }
}

/// A tree: `{v, entries}`. Entries are in strictly ascending order of path
/// bytes, every path is valid, and no two paths collide after folding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u64,
    pub entries: Vec<Entry>,
}

impl Manifest {
    /// [`Record::decode`], with the paths checked through `paths`, so a path seen
    /// in another manifest isn't checked or folded again.
    pub fn decode_with(bytes: &[u8], paths: &mut PathCache) -> Result<Manifest, FormatError> {
        let manifest: Manifest = from_cbor(bytes)?;
        if manifest.v != VERSION {
            return Err(FormatError::Version(manifest.v));
        }
        manifest.check_with(paths)?;
        Ok(manifest)
    }

    fn check_with(&self, paths: &mut PathCache) -> Result<(), FormatError> {
        for pair in self.entries.windows(2) {
            if pair[0].path.as_bytes() >= pair[1].path.as_bytes() {
                return Err(FormatError::EntryOrder(pair[1].path.clone()));
            }
        }
        for entry in &self.entries {
            paths.validate(&entry.path).map_err(|error| FormatError::Path { path: entry.path.clone(), error })?;
        }
        match paths.find_collision(self.entries.iter().map(|e| e.path.as_str())) {
            Some(collision) => Err(FormatError::Collision(collision)),
            None => Ok(()),
        }
    }

    /// A manifest of `entries` in any order; they are sorted by path bytes.
    pub fn new(mut entries: Vec<Entry>) -> Manifest {
        entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        Manifest { v: VERSION, entries }
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.binary_search_by(|e| e.path.as_bytes().cmp(path.as_bytes())).ok().map(|i| &self.entries[i])
    }
}

impl Record for Manifest {
    const MEDIA_TYPE: &'static str = MANIFEST_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        self.check_with(&mut PathCache::new())
    }

    fn artifact_name(&self) -> &str {
        "manifest"
    }
}

/// The Weave marker: `{v, purpose, in_place}`. `in_place` names, at the join
/// only, the claim tagged `decision` that allowed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub v: u64,
    pub purpose: String,
    pub in_place: Option<ContributionId>,
}

impl Record for Marker {
    const MEDIA_TYPE: &'static str = MARKER_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        Ok(())
    }

    fn artifact_name(&self) -> &str {
        "weave"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guardrails {
    pub max_known: u64,
    pub max_age: u64,
    /// Path prefixes no breakage entry may name.
    pub protected: Vec<String>,
}

/// One version of a line's policy: the line artifact's payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub v: u64,
    pub name: String,
    pub integrators: Vec<AgentId>,
    pub validators: Vec<AgentId>,
    pub goal_setters: Vec<AgentId>,
    /// A check's name, never blank, to the argv that runs it.
    pub checks: BTreeMap<String, Vec<String>>,
    pub guardrails: Guardrails,
    /// The id, as text, of the owner's decision claim in the coordination Ground
    /// that allows this version. Required by `scripts/weave` before stage 2.
    pub owner_decision: Option<String>,
}

impl Line {
    /// The method a result for the named check carries under this policy, if the
    /// policy has that check.
    pub fn check_method(&self, name: &str) -> Option<String> {
        self.checks.get(name).map(|argv| CheckDefinition { name, argv }.method())
    }
}

impl Record for Line {
    const MEDIA_TYPE: &'static str = LINE_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        not_blank(&self.name, "the line's name")?;
        for (name, argv) in &self.checks {
            not_blank(name, "a check's name")?;
            if argv.is_empty() {
                return Err(FormatError::EmptyArgv(name.clone()));
            }
        }
        for prefix in &self.guardrails.protected {
            valid_path(prefix)?;
        }
        if let Some(decision) = &self.owner_decision {
            not_blank(decision, "owner_decision")?;
        }
        Ok(())
    }

    fn artifact_name(&self) -> &str {
        &self.name
    }
}

/// Known breakage a decision accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakage {
    /// A check's name.
    pub check: String,
    pub detail: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Fixed,
    FellBack,
    AcceptedBreakage,
    Overridden,
}

/// How the integrator dealt with an objection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub objection: ContributionId,
    pub disposition: Disposition,
    pub reason: String,
}

/// The git commit a decision mirrors, while dogfooding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub git: String,
}

/// A decision moves a line: the commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub v: u64,
    pub line: ContributionId,
    pub manifest: ContentHash,
    /// Decisions, strictly ascending.
    pub parents: Vec<ContributionId>,
    pub proposals: Vec<ContributionId>,
    pub breakage: Vec<Breakage>,
    pub answers: Vec<Answer>,
    pub resolutions: Vec<ContributionId>,
    pub goals: Vec<ContributionId>,
    pub source: Option<Source>,
    pub note: String,
}

impl Record for Decision {
    const MEDIA_TYPE: &'static str = DECISION_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        let references = 1
            + self.parents.len()
            + self.proposals.len()
            + self.answers.len()
            + self.resolutions.len()
            + self.goals.len();
        if references > MAX_REFERENCES {
            return Err(FormatError::TooManyReferences(references));
        }
        strictly_ascending(&self.parents, "a decision's parents")?;
        for breakage in &self.breakage {
            not_blank(&breakage.check, "a breakage entry's check")?;
            for path in &breakage.paths {
                valid_path(path)?;
            }
        }
        for answer in &self.answers {
            not_blank(&answer.reason, "an answer's reason")?;
        }
        Ok(())
    }

    fn artifact_name(&self) -> &str {
        "decision"
    }
}

/// A request to integrate a tree onto a line. `change` is the content hash of
/// the per-file change from `base`'s manifest to `manifest` ([`Change`]); views
/// recompute it, and a proposal whose `change` doesn't match is invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub v: u64,
    pub line: ContributionId,
    pub base: ContributionId,
    pub manifest: ContentHash,
    pub change: ContentHash,
    pub summary: String,
}

impl Record for Proposal {
    const MEDIA_TYPE: &'static str = PROPOSAL_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        Ok(())
    }

    fn artifact_name(&self) -> &str {
        "proposal"
    }
}

/// Settles one file conflict. `base` is the common ancestor's value (null if
/// the file was added on both sides); `sides` are the two sides' values in
/// strictly ascending order, null (a deleted side) first; `result` is the
/// settled value, or null to delete the file. Resolutions are keyed on
/// `(path, base, sides)`, full file values included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub v: u64,
    pub path: String,
    pub base: Option<FileValue>,
    pub sides: [Option<FileValue>; 2],
    pub result: Option<FileValue>,
}

impl Resolution {
    /// A resolution, with the two sides put in order.
    pub fn new(
        path: String,
        base: Option<FileValue>,
        mut sides: [Option<FileValue>; 2],
        result: Option<FileValue>,
    ) -> Resolution {
        sides.sort();
        Resolution { v: VERSION, path, base, sides, result }
    }
}

impl Record for Resolution {
    const MEDIA_TYPE: &'static str = RESOLUTION_MEDIA_TYPE;

    fn version(&self) -> u64 {
        self.v
    }

    fn check(&self) -> Result<(), FormatError> {
        valid_path(&self.path)?;
        strictly_ascending(&self.sides, "a resolution's sides")
    }

    fn artifact_name(&self) -> &str {
        "resolution"
    }
}

/// One path of a change: `[path, before, after]`, null where the file is absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeEntry {
    pub path: String,
    pub before: Option<FileValue>,
    pub after: Option<FileValue>,
}

impl Serialize for ChangeEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.path, self.before, self.after).serialize(serializer)
    }
}

/// The per-file difference between two manifests: the paths that differ, in
/// ascending order of path bytes. Its hash is a proposal's `change`, so a rebase
/// that leaves what the proposal does to every file unchanged keeps its reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    entries: Vec<ChangeEntry>,
}

impl Change {
    pub fn between(base: &Manifest, new: &Manifest) -> Change {
        let mut values: BTreeMap<&[u8], (Option<FileValue>, Option<FileValue>)> = BTreeMap::new();
        for entry in &base.entries {
            values.entry(entry.path.as_bytes()).or_default().0 = Some(entry.value());
        }
        for entry in &new.entries {
            values.entry(entry.path.as_bytes()).or_default().1 = Some(entry.value());
        }
        let entries = values
            .into_iter()
            .filter(|(_, (before, after))| before != after)
            .map(|(path, (before, after))| ChangeEntry {
                path: String::from_utf8(path.to_vec()).expect("manifest paths are strings"),
                before,
                after,
            })
            .collect();
        Change { entries }
    }

    pub fn entries(&self) -> &[ChangeEntry] {
        &self.entries
    }

    /// The content hash of the list's canonical CBOR.
    pub fn hash(&self) -> ContentHash {
        ContentHash::of(&to_cbor(&self.entries))
    }
}

/// A named check and the argv a policy gives it.
#[derive(Clone, Copy, Debug)]
pub struct CheckDefinition<'a> {
    pub name: &'a str,
    pub argv: &'a [String],
}

impl CheckDefinition<'_> {
    /// The content hash of the canonical CBOR of `[name, argv]`.
    pub fn hash(&self) -> ContentHash {
        ContentHash::of(&to_cbor(&(self.name, self.argv)))
    }

    /// The method every result for this check carries: `<name> blake3:<definition hash>`.
    /// A result counts for a policy only if its method equals the policy's.
    pub fn method(&self) -> String {
        format!("{} {}", self.name, self.hash())
    }
}

/// Splits a result's method into the check's name and its definition hash.
pub fn parse_method(method: &str) -> Option<(&str, ContentHash)> {
    let (name, hash) = method.rsplit_once(' ')?;
    if name.trim().is_empty() {
        return None;
    }
    Some((name, hash.parse().ok()?))
}
