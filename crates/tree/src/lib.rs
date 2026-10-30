//! # bracken-tree
//!
//! Bracken's code layer on Manyroot's Weave: source trees as manifests of
//! content-hashed files, and the lines, decisions, proposals and resolutions
//! that move them (docs/design/weave.md). manyroot assumes nothing about code or
//! files; this crate knows about files and paths.
//!
//! | Module     | Holds                                                         |
//! |------------|---------------------------------------------------------------|
//! | [`hash`]   | Content hashes, labelled `blake3:` in text                    |
//! | [`path`]   | Path and symlink-target rules, folding and collisions         |
//! | [`format`] | Every Weave record's payload format, in canonical CBOR        |
//!
//! Every record is a plain manyroot contribution, so nothing here changes the
//! protocol.

pub mod format;
pub mod hash;
pub mod path;

pub use format::{
    Answer, Breakage, Change, ChangeEntry, CheckDefinition, Decision, Disposition, Entry, FileValue, FormatError,
    Guardrails, Line, Manifest, Marker, Mode, Proposal, Record, Resolution, Source,
};
pub use hash::ContentHash;
pub use path::{Collision, PathError};
