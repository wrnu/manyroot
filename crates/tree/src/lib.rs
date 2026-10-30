//! # bracken-tree
//!
//! Bracken's code layer on Manyroot's Weave: source trees as manifests of
//! content-hashed files, and the lines, decisions, proposals and resolutions
//! that move them (docs/design/weave.md). manyroot assumes nothing about code or
//! files; this crate knows about files and paths.
//!
//! | Module     | Holds                                                         |
//! |------------|---------------------------------------------------------------|
//! | [`ancestry`] | The ancestor rule, answered without a walk per question |
//! | [`check`]  | The check runner: a named check in a fresh directory, under the sandbox |
//! | [`ground`] | Weave's records in a Ground: an index, kinds, and what check lists |
//! | [`hash`]   | Content hashes, labelled `blake3:` in text                    |
//! | [`path`]   | Path and symlink-target rules, folding and collisions         |
//! | [`results`] | Check results, counted under a policy, and green             |
//! | [`reviews`] | Goals, and reviews of decisions, proposals and manifests     |
//! | [`diff`]   | Differences between manifests, and unified text diffs          |
//! | [`format`] | Every Weave record's payload format, in canonical CBOR        |
//! | [`import`] | Import from git's view of a tree, with exclusions and refusals |
//! | [`lines`]  | Lines and their policies, qualifying decisions, heads and forks |
//! | [`materialize`] | Writing a manifest out without following a symlink     |
//! | [`merge`]  | Per-file merges, conflicts and their resolutions             |
//! | [`objections`] | Objections, their basis and answers, guardrails, open objections |
//! | [`secrets`]| The deny-list and the key-material scan import refuses by      |
//!
//! Every record is a plain manyroot contribution, so nothing here changes the
//! protocol.

pub mod ancestry;
pub mod check;
pub mod diff;
pub mod format;
pub mod ground;
pub mod hash;
pub mod import;
pub mod lines;
pub mod materialize;
pub mod merge;
pub mod objections;
pub mod path;
pub mod results;
pub mod reviews;
pub mod secrets;

pub use format::{
    Answer, Breakage, Change, ChangeEntry, CheckDefinition, Decision, Disposition, Entry, FileValue, FormatError,
    Guardrails, Line, Manifest, Marker, Mode, Proposal, Record, Resolution, Source,
};
pub use hash::ContentHash;
pub use path::{Collision, PathError};
