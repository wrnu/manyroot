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
    #[error("executor for {0:?}, which isn't one of the line's checks")]
    ExecutorWithoutCheck(String),
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

/// What a payload's top-level map holds under `v` and `line`, read as generic CBOR whatever its other
/// fields are, so a reader can say what a record it can't decode carries (docs/design/weave-freeze.md, G8).
/// This never makes a payload valid: [`Record::decode`] alone says that.
///
/// It reads what a generic [`ciborium::Value`] of the payload would hold (the first entry with each text
/// key; nothing if the payload isn't one well-formed map), but builds no value for the other entries: it
/// walks past them, so its cost is linear in the payload's bytes and its memory bounded by its longest
/// string, never a value tree many times the payload's size (finding 3832a3c04d2d). Its bounds are
/// ciborium's own, the same as the generic read's: 256 levels of nesting, past which the payload holds
/// nothing here, and strings read through a 4 KiB buffer or, when longer, one buffer of their own length.
/// So it gives the same answer as the generic read for every payload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PayloadHead {
    /// The top-level `v`, if it is an unsigned integer.
    pub version: Option<u64>,
    /// The top-level `line`, if it is a byte string of an id's length.
    pub line: Option<ContributionId>,
}

impl PayloadHead {
    /// Reads a payload's head, once: callers that need both fields keep the result.
    pub fn read(bytes: &[u8]) -> PayloadHead {
        ciborium::from_reader::<Head, _>(bytes).map(|h| h.0).unwrap_or_default()
    }

    /// Whether the payload this is the head of may decode in this build: its `v` is one this build reads.
    /// Only such a payload is ever valid, as [`Record::validate`] refuses every other version and a valid
    /// payload's head always reads its `v`. So a reader that asks this before decoding a payload as its
    /// type changes no record's validity, but pays that decode, which builds every field a payload holds at
    /// many times its size, only for the version it reads (finding 8b99e0d6d326). A payload of another
    /// version is refused naming it, whatever its fields (docs/design/weave-freeze.md, section 3).
    pub fn readable(&self) -> Result<(), FormatError> {
        match self.version {
            Some(VERSION) => Ok(()),
            Some(v) => Err(FormatError::Version(v)),
            None => Err(FormatError::Malformed("the payload's top-level map holds no format version `v`".into())),
        }
    }

    /// [`Record::decode`] of the payload this is the head of, only if it is [`PayloadHead::readable`].
    pub fn decode<R: Record>(&self, bytes: &[u8]) -> Result<R, FormatError> {
        self.readable()?;
        R::decode(bytes)
    }
}

/// [`Record::decode`], reading the payload's head first, so a payload of a version this build doesn't read
/// costs about its own size whatever its fields ([`PayloadHead::readable`]). What any reader that decodes
/// every Weave record of the Ground, as each command does, decodes them with.
pub fn decode_bounded<R: Record>(bytes: &[u8]) -> Result<R, FormatError> {
    PayloadHead::read(bytes).decode(bytes)
}

/// [`PayloadHead`], as the top-level item; anything but a map is an error, which reads as no head.
struct Head(PayloadHead);

impl<'de> Deserialize<'de> for Head {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(HeadVisitor)
    }
}

struct HeadVisitor;

impl<'de> serde::de::Visitor<'de> for HeadVisitor {
    type Value = Head;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a map")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Head, A::Error> {
        let mut head = PayloadHead::default();
        let (mut v, mut line) = (false, false);
        while let Some(key) = map.next_key::<Item>()? {
            let value: Item = map.next_value()?;
            match key {
                Item::V if !v => {
                    v = true;
                    head.version = match value {
                        Item::Unsigned(n) => Some(n),
                        _ => None,
                    };
                }
                Item::Line if !line => {
                    line = true;
                    head.line = match value {
                        Item::Id(id) => Some(id),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
        Ok(Head(head))
    }
}

/// One CBOR item, kept only as far as [`PayloadHead`] needs it. Visiting one accepts exactly what a generic
/// [`ciborium::Value`] accepts, through the same calls, and keeps nothing of arrays, maps and tags but
/// their well-formedness.
enum Item {
    /// An integer a `u64` holds.
    Unsigned(u64),
    /// A byte string of an id's length.
    Id(ContributionId),
    /// The text `v`.
    V,
    /// The text `line`.
    Line,
    Other,
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ItemVisitor)
    }
}

struct ItemVisitor;

impl ItemVisitor {
    fn text(s: &str) -> Item {
        match s {
            "v" => Item::V,
            "line" => Item::Line,
            _ => Item::Other,
        }
    }

    fn bytes(b: &[u8]) -> Item {
        b.try_into().map(|b| Item::Id(ContributionId(b))).unwrap_or(Item::Other)
    }

    fn integer<T: TryInto<u64>>(n: T) -> Item {
        n.try_into().map(Item::Unsigned).unwrap_or(Item::Other)
    }
}

impl<'de> serde::de::Visitor<'de> for ItemVisitor {
    type Value = Item;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a valid CBOR item")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Item, E> {
        Ok(Self::integer(n))
    }

    fn visit_i128<E: serde::de::Error>(self, n: i128) -> Result<Item, E> {
        Ok(Self::integer(n))
    }

    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Item, E> {
        Ok(Item::Unsigned(n))
    }

    fn visit_u128<E: serde::de::Error>(self, n: u128) -> Result<Item, E> {
        Ok(Self::integer(n))
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_char<E: serde::de::Error>(self, c: char) -> Result<Item, E> {
        Ok(Self::text(c.encode_utf8(&mut [0; 4])))
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Item, E> {
        Ok(Self::text(s))
    }

    fn visit_bytes<E: serde::de::Error>(self, b: &[u8]) -> Result<Item, E> {
        Ok(Self::bytes(b))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Item, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<Item, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Item, A::Error> {
        while seq.next_element::<Item>()?.is_some() {}
        Ok(Item::Other)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Item, A::Error> {
        while map.next_entry::<Item, Item>()?.is_some() {}
        Ok(Item::Other)
    }

    /// A tag, as ciborium hands it to a generic value: a variant whose content is the tag and the item.
    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, data: A) -> Result<Item, A::Error> {
        use serde::de::VariantAccess;

        struct Tagged;

        impl<'de> serde::de::Visitor<'de> for Tagged {
            type Value = Item;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a tag and an item")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Item, A::Error> {
                seq.next_element::<u64>()?.ok_or_else(|| serde::de::Error::custom("expected tag"))?;
                seq.next_element::<Item>()?.ok_or_else(|| serde::de::Error::custom("expected val"))?;
                Ok(Item::Other)
            }
        }

        let (_, variant): (String, _) = data.variant()?;
        variant.tuple_variant(2, Tagged)
    }
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
    /// A check that runs outside the sandboxed runner, by its name (one of `checks`), to how it runs
    /// (plan cc496dcee0e3, section 2). Absent from the payload when empty, so a line without one encodes
    /// as before; an older build, which doesn't know the field, refuses a line that has one, since it
    /// wouldn't re-encode the same.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub executors: BTreeMap<String, Executor>,
    pub guardrails: Guardrails,
    /// The id, as text, of the owner's decision claim in the coordination Ground
    /// that allows this version. Required by `scripts/weave` before stage 2.
    pub owner_decision: Option<String>,
}

impl Line {
    /// The method a result for the named check carries under this policy, if the
    /// policy has that check.
    pub fn check_method(&self, name: &str) -> Option<String> {
        self.definition(name).map(|d| d.method())
    }

    /// The named check's definition under this policy: its argv and, if it has one, its executor.
    pub fn definition<'a>(&'a self, name: &'a str) -> Option<CheckDefinition<'a>> {
        let argv = self.checks.get(name)?;
        Some(CheckDefinition { name, argv, executor: self.executors.get(name) })
    }

    /// Every check's definition, in name order.
    pub fn definitions(&self) -> impl Iterator<Item = CheckDefinition<'_>> {
        self.checks.iter().map(|(name, argv)| CheckDefinition { name, argv, executor: self.executors.get(name) })
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
        for (name, executor) in &self.executors {
            if !self.checks.contains_key(name) {
                return Err(FormatError::ExecutorWithoutCheck(name.clone()));
            }
            valid_path(&executor.script)?;
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

impl Disposition {
    /// The name the payload and the CLI use: `fixed`, `fell_back`, `accepted_breakage`, `overridden`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Disposition::Fixed => "fixed",
            Disposition::FellBack => "fell_back",
            Disposition::AcceptedBreakage => "accepted_breakage",
            Disposition::Overridden => "overridden",
        }
    }
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

/// Where a check runs, when not in the sandboxed runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    /// In a remote box, from a bundle of the manifest's commit.
    Box,
    /// On the validator's Mac, unsandboxed, in a checkout of the manifest's commit.
    Mac,
}

impl ExecutorKind {
    /// The name the payload and the CLI use: `box` or `mac`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutorKind::Box => "box",
            ExecutorKind::Mac => "mac",
        }
    }
}

/// How a check runs outside the sandboxed runner: the runner script that runs it, a path inside the
/// validator's runner directory (the main checkout), pinned by its content hash, so the script that
/// judges the run is the one the policy names, never the tree's own copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executor {
    pub kind: ExecutorKind,
    pub script: String,
    pub script_hash: ContentHash,
}

/// A named check, the argv a policy gives it and, if it runs outside the sandbox, its executor.
#[derive(Clone, Copy, Debug)]
pub struct CheckDefinition<'a> {
    pub name: &'a str,
    pub argv: &'a [String],
    pub executor: Option<&'a Executor>,
}

impl CheckDefinition<'_> {
    /// The content hash of the canonical CBOR of `[name, argv]`, or of `[name, argv, executor]` for a
    /// check with an executor: the kind, the script and its hash are part of what a result is for.
    pub fn hash(&self) -> ContentHash {
        match self.executor {
            None => ContentHash::of(&to_cbor(&(self.name, self.argv))),
            Some(e) => ContentHash::of(&to_cbor(&(self.name, self.argv, e))),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ciborium::Value;

    /// The generic read [`PayloadHead`] replaced: the payload as a whole [`Value`], then its fields.
    fn generic(bytes: &[u8]) -> PayloadHead {
        let Ok(Value::Map(entries)) = ciborium::from_reader::<Value, _>(bytes) else { return PayloadHead::default() };
        let field = |key: &str| entries.iter().find(|(k, _)| matches!(k, Value::Text(t) if t == key)).map(|(_, v)| v);
        PayloadHead {
            version: match field("v") {
                Some(Value::Integer(v)) => u64::try_from(*v).ok(),
                _ => None,
            },
            line: match field("line") {
                Some(Value::Bytes(b)) => b.as_slice().try_into().ok().map(ContributionId),
                _ => None,
            },
        }
    }

    fn agree(bytes: &[u8]) -> PayloadHead {
        let head = PayloadHead::read(bytes);
        assert_eq!(head, generic(bytes), "on {bytes:02x?}");
        head
    }

    fn map(entries: Vec<(Value, Value)>) -> Vec<u8> {
        to_cbor(&Value::Map(entries))
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    /// A map of `depth` nested one-entry maps, the innermost holding `v: 2`.
    fn nested(depth: usize) -> Vec<u8> {
        let mut bytes = [0xa1, 0x61, b'x'].repeat(depth);
        bytes.extend(map(vec![(text("v"), Value::Integer(2.into()))]));
        bytes
    }

    #[test]
    fn the_head_reads_what_the_generic_value_held() {
        let id = Value::Bytes(vec![9; 32]);
        let two = || Value::Integer(2.into());
        let head = agree(&map(vec![(text("x"), Value::Array(vec![])), (text("v"), two()), (text("line"), id.clone())]));
        assert_eq!(head, PayloadHead { version: Some(2), line: Some(ContributionId([9; 32])) });
        let cases: Vec<Vec<u8>> = vec![
            // The first of duplicate keys counts, whatever the others hold.
            map(vec![(text("v"), text("2")), (text("v"), two())]),
            map(vec![(text("v"), two()), (text("v"), Value::Integer(3.into())), (text("line"), id.clone())]),
            // Keys that aren't plain text never match.
            map(vec![(Value::Bytes(b"v".to_vec()), two()), (Value::Tag(1, Box::new(text("line"))), id.clone())]),
            map(vec![(Value::Array(vec![text("v")]), two()), (Value::Map(vec![(text("v"), two())]), id.clone())]),
            // Values of the wrong kind or size.
            map(vec![(text("v"), Value::Integer((-2).into())), (text("line"), Value::Bytes(vec![1; 31]))]),
            map(vec![(text("v"), Value::Float(2.0)), (text("line"), Value::Bytes(vec![1; 33]))]),
            map(vec![(text("v"), Value::Tag(7, Box::new(two()))), (text("line"), Value::Tag(7, Box::new(id.clone())))]),
            map(vec![(text("v"), Value::Null), (text("line"), Value::Bool(true))]),
            map(vec![(text("v"), Value::Integer(u64::MAX.into())), (text("line"), id.clone())]),
            // Long strings, read past the scratch buffer.
            map(vec![(text(&"k".repeat(5000)), Value::Bytes(vec![0; 5000])), (text("v"), two())]),
            map(vec![(text("v"), two()), (text("line"), Value::Bytes(vec![0; 70_000]))]),
            // Anything but a map holds no head.
            to_cbor(&Value::Array(vec![text("v"), two()])),
            to_cbor(&Value::Tag(55799, Box::new(Value::Map(vec![(text("v"), two())])))),
            to_cbor(&two()),
            vec![],
        ];
        for bytes in cases {
            agree(&bytes);
        }
        let raw: &[&[u8]] = &[
            // A bignum v reads as its integer, as the generic value read it.
            &[0xa1, 0x61, b'v', 0xc2, 0x41, 0x02],
            &[0xa1, 0x61, b'v', 0xc2, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            &[0xa1, 0x61, b'v', 0xc3, 0x41, 0x02],
            // Indefinite lengths, for the map, a key and the line.
            &[0xbf, 0x61, b'v', 0x02, 0xff],
            &[0xbf, 0x7f, 0x61, b'v', 0xff, 0x02, 0xff],
            &[
                0xa1, 0x64, b'l', b'i', b'n', b'e', 0x5f, 0x50, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0x50,
                2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 0xff,
            ],
            // A malformed item anywhere spoils the whole head: invalid UTF-8, an unknown simple value, a stray
            // break, a truncation.
            &[0xa2, 0x61, b'v', 0x02, 0x61, b'x', 0x61, 0xff],
            &[0xa2, 0x61, b'v', 0x02, 0x61, b'x', 0xf0],
            &[0xa2, 0x61, b'v', 0x02, 0x61, b'x', 0xff],
            &[0xa2, 0x61, b'v', 0x02, 0x61, b'x'],
            // Trailing bytes after the map are not read, by either.
            &[0xa1, 0x61, b'v', 0x02, 0xff, 0xff],
        ];
        for bytes in raw {
            agree(bytes);
        }
        assert_eq!(agree(&[0xa1, 0x61, b'v', 0xc2, 0x41, 0x02]).version, Some(2));
        assert_eq!(agree(&[0xa1, 0x61, b'v', 0x02, 0xff, 0xff]).version, Some(2));
    }

    #[test]
    fn the_nesting_bound_is_the_generic_reads() {
        // ciborium's 256 levels: the innermost map is the 256th at depth 255.
        assert_eq!(agree(&nested(255)), PayloadHead::default());
        for depth in [0, 1, 200, 250, 253, 254, 255, 256, 257, 1000] {
            agree(&nested(depth));
        }
        // Past the bound, nothing is read, so the record holds no head, as before.
        let deep = [map(vec![(text("v"), Value::Integer(2.into()))]), nested(1000)].concat();
        let mut too_deep = vec![0xa2];
        too_deep.extend(&deep[1..deep.len() - nested(1000).len()]);
        too_deep.extend([0x61, b'x']);
        too_deep.extend(nested(1000));
        assert_eq!(agree(&too_deep), PayloadHead::default());
        let mut shallow = vec![0xa2];
        shallow.extend(&deep[1..deep.len() - nested(1000).len()]);
        shallow.extend([0x61, b'x']);
        shallow.extend(nested(100));
        assert_eq!(agree(&shallow).version, Some(2));
    }

    #[test]
    fn mutated_payloads_read_the_same_head_as_the_generic_value() {
        let decision = map(vec![
            (text("v"), Value::Integer(2.into())),
            (text("line"), Value::Bytes((0..32).collect())),
            (text("x"), Value::Array(vec![Value::Tag(2, Box::new(Value::Bytes(vec![1, 2]))), Value::Float(1.5)])),
            (text("y"), Value::Map(vec![(Value::Integer(1.into()), Value::Text("é".into()))])),
        ]);
        // A fixed xorshift, so a failure reproduces.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let mut bytes = decision.clone();
            for _ in 0..1 + next() % 3 {
                let at = (next() as usize) % bytes.len();
                match next() % 3 {
                    0 => bytes[at] = next() as u8,
                    1 => bytes.truncate(at.max(1)),
                    _ => bytes.insert(at, next() as u8),
                }
            }
            agree(&bytes);
        }
    }
}
