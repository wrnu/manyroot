//! Per-file merges, conflicts and resolutions (docs/design/weave.md, section 2
//! *Merge base* and *Per-file merge*, and section 1 *Proposals and
//! resolutions*).
//!
//! A file's value is its [`FileValue`] (content hash and mode), or absent.
//! Comparing a base `B` with sides `X` and `Y`: equal sides are taken; a side
//! equal to `B` gives way to the other; anything else is a conflict, keyed on
//! `(path, B, {X, Y})` with full file values, mode included. A resolution with
//! exactly that key settles it, the smallest id first. Without one, a conflict
//! whose three versions are all text files merges line by line
//! ([`crate::text_merge`]; task 7cbd0a0ce7d0), if that merge is clean. A rename
//! is a delete plus an add.

use std::collections::{BTreeMap, BTreeSet};

use manyroot::ContributionId;

use crate::ContentHash;
use crate::format::{Entry, FileValue, Manifest, Mode, Resolution};
use crate::import::FileVersion;
use crate::path::{self, Collision};

/// One file conflict: what a resolution is keyed on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub path: String,
    pub base: Option<FileValue>,
    /// The two sides' values, in ascending order (absent first).
    pub sides: [Option<FileValue>; 2],
}

impl Key {
    pub fn new(path: String, base: Option<FileValue>, mut sides: [Option<FileValue>; 2]) -> Key {
        sides.sort();
        Key { path, base, sides }
    }

    pub fn of(resolution: &Resolution) -> Key {
        Key { path: resolution.path.clone(), base: resolution.base, sides: resolution.sides }
    }
}

/// A conflict as a merge meets it: its key, and which side is which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: Key,
    /// The value on the side being merged into (a line's head).
    pub ours: Option<FileValue>,
    /// The value on the side merged in (another line's head, or a proposal).
    pub theirs: Option<FileValue>,
}

/// What a three-way merge of three trees gives before resolutions: the files it
/// settled, by path, and the conflicts, in path order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Merged {
    pub settled: BTreeMap<String, Entry>,
    pub conflicts: Vec<Conflict>,
}

/// Merges `theirs` into `ours` against `base`, file by file.
pub fn three_way(base: &Manifest, ours: &Manifest, theirs: &Manifest) -> Merged {
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for m in [base, ours, theirs] {
        paths.extend(m.entries.iter().map(|e| e.path.as_str()));
    }
    let mut merged = Merged::default();
    for path in paths {
        let (b, x, y) = (base.get(path), ours.get(path), theirs.get(path));
        let value = |e: Option<&Entry>| e.map(Entry::value);
        let (bv, xv, yv) = (value(b), value(x), value(y));
        let taken = if xv == yv {
            Some(x)
        } else if xv == bv {
            Some(y)
        } else if yv == bv {
            Some(x)
        } else {
            None
        };
        match taken {
            Some(Some(entry)) => {
                merged.settled.insert(path.to_owned(), entry.clone());
            }
            Some(None) => {}
            None => {
                merged.conflicts.push(Conflict { key: Key::new(path.to_owned(), bv, [xv, yv]), ours: xv, theirs: yv })
            }
        }
    }
    merged
}

/// Valid resolutions, by key: ids ascending, each with its payload.
#[derive(Debug, Clone, Default)]
pub struct Resolutions {
    by_key: BTreeMap<Key, Vec<(ContributionId, Resolution)>>,
}

impl Resolutions {
    pub fn insert(&mut self, id: ContributionId, resolution: Resolution) {
        let ids = self.by_key.entry(Key::of(&resolution)).or_default();
        ids.push((id, resolution));
        ids.sort_by_key(|(id, _)| *id);
    }

    /// The resolutions with exactly this key, the smallest id first.
    pub fn for_key(&self, key: &Key) -> &[(ContributionId, Resolution)] {
        self.by_key.get(key).map_or(&[], Vec::as_slice)
    }

    /// Only the resolutions `keep` takes, by id: a merge applies on its own only those
    /// an integrator of the target line wrote (decision 91c12b4da2ad).
    pub fn only(&self, keep: impl Fn(&ContributionId) -> bool) -> Resolutions {
        let mut kept = Resolutions::default();
        for (id, resolution) in self.by_key.values().flatten() {
            if keep(id) {
                kept.insert(*id, resolution.clone());
            }
        }
        kept
    }
}

/// Why a merged tree can't be a manifest.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    #[error("{} conflicts have no resolution, starting with {:?}", .0.len(), .0[0].key.path)]
    Unresolved(Vec<Conflict>),
    #[error("resolution {id} settles {path:?} with {hash}, a file version this replica doesn't hold")]
    ResultNotHeld { id: ContributionId, path: String, hash: crate::ContentHash },
    #[error("the merged tree's paths collide: {0}")]
    Collision(Collision),
}

/// A merged tree, with the resolutions it used, ascending, and the paths it merged
/// line by line, with the file versions that merge made, in path order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub manifest: Manifest,
    pub used: Vec<ContributionId>,
    /// The file versions the line merge made: the tree holds them, the replica may not.
    pub merged_lines: Vec<FileVersion>,
}

/// Settles every conflict of `merged` with the resolution of its key that has the
/// smallest id. `size` gives a held file version's size, for a resolution's result.
/// No conflict merges line by line: see [`resolve_lines`].
pub fn resolve(
    merged: &Merged,
    resolutions: &Resolutions,
    size: impl Fn(&ContentHash) -> Option<u64>,
) -> Result<Resolved, MergeError> {
    resolve_lines(merged, resolutions, size, |_| None)
}

/// The mode of a file whose modes are `base`, `ours` and `theirs`, merged as values are: equal sides
/// are taken, and a side equal to the base gives way to the other. `None` if they changed both ways.
pub fn merge_mode(base: Mode, ours: Mode, theirs: Mode) -> Option<Mode> {
    if ours == theirs || theirs == base {
        Some(ours)
    } else if ours == base {
        Some(theirs)
    } else {
        None
    }
}

/// The line merge of one conflict, if it is clean: its entry and its bytes. Only a conflict whose three
/// versions are all present, regular files (not symlinks), held, and text merges, and only if its modes
/// merge too.
fn merge_lines<'a>(conflict: &Conflict, content: &impl Fn(&ContentHash) -> Option<&'a [u8]>) -> Option<FileVersion> {
    let (b, x, y) = (conflict.key.base?, conflict.ours?, conflict.theirs?);
    merge_mode(b.mode, x.mode, y.mode)?;
    let (bb, xb, yb) = text_sides(conflict, content)?;
    let bytes = crate::text_merge::merge(bb, xb, yb)?;
    // A merge too big to write as one file version stays a conflict (finding d189e282cdee).
    if bytes.len() > crate::import::MAX_FILE_BYTES {
        return None;
    }
    Some(FileVersion { hash: ContentHash::of(&bytes), name: conflict.key.path.clone(), bytes })
}

/// Where a conflict's text conflicts, as base line ranges counted from 1, inclusive at both ends:
/// `None` unless its three versions are present, held, regular files and text, with each side's diff
/// within [`crate::text_merge::MAX_DIFF_LINES`], so it could merge line by line. Empty if its lines
/// merge, and only its mode, or the merge's size, stops it.
pub fn text_conflicts<'a>(
    conflict: &Conflict,
    content: impl Fn(&ContentHash) -> Option<&'a [u8]>,
) -> Option<Vec<(u32, u32)>> {
    let (bb, xb, yb) = text_sides(conflict, content)?;
    Some(crate::text_merge::conflicts(bb, xb, yb)?.into_iter().map(|r| (r.start + 1, r.end.max(r.start + 1))).collect())
}

/// For a conflict whose base and sides are all text, as [`text_conflicts`] asks, the merge with each
/// conflicting region between markers ([`crate::text_merge::markers`]); `None` for any other conflict.
pub fn conflict_markers<'a>(
    conflict: &Conflict,
    content: impl Fn(&ContentHash) -> Option<&'a [u8]>,
    labels: &crate::text_merge::Labels,
) -> Option<Vec<u8>> {
    let (bb, xb, yb) = text_sides(conflict, content)?;
    crate::text_merge::markers(bb, xb, yb, labels)
}

/// The bytes of a conflict's base and sides, when all three are present, none is a symlink, `content`
/// gives each, and each is text.
fn text_sides<'a>(
    conflict: &Conflict,
    content: impl Fn(&ContentHash) -> Option<&'a [u8]>,
) -> Option<(&'a [u8], &'a [u8], &'a [u8])> {
    let (b, x, y) = (conflict.key.base?, conflict.ours?, conflict.theirs?);
    if [b, x, y].iter().any(|v| v.mode == Mode::Symlink) {
        return None;
    }
    let (bb, xb, yb) = (content(&b.hash)?, content(&x.hash)?, content(&y.hash)?);
    [bb, xb, yb].iter().all(|bytes| crate::text_merge::is_text(bytes)).then_some((bb, xb, yb))
}

/// As [`resolve`], and then a conflict no resolution settles merges line by line if it can, with
/// `content` giving a held file version's bytes. A resolution comes first, so every merge recorded
/// before line merging computes the same tree, and an integrator can overrule a clean textual merge
/// (design 68b57f9aed6d, answer c8a511445626).
pub fn resolve_lines<'a>(
    merged: &Merged,
    resolutions: &Resolutions,
    size: impl Fn(&ContentHash) -> Option<u64>,
    content: impl Fn(&ContentHash) -> Option<&'a [u8]>,
) -> Result<Resolved, MergeError> {
    let mut entries = merged.settled.clone();
    let mut used = Vec::new();
    let mut unresolved = Vec::new();
    let mut merged_lines = Vec::new();
    for conflict in &merged.conflicts {
        let Some((id, resolution)) = resolutions.for_key(&conflict.key).first() else {
            match line_merged(conflict, &content) {
                Some((entry, version)) => {
                    entries.insert(conflict.key.path.clone(), entry);
                    merged_lines.push(version);
                }
                None => unresolved.push(conflict.clone()),
            }
            continue;
        };
        if let Some(value) = resolution.result {
            let size = size(&value.hash).ok_or_else(|| MergeError::ResultNotHeld {
                id: *id,
                path: conflict.key.path.clone(),
                hash: value.hash,
            })?;
            entries.insert(
                conflict.key.path.clone(),
                Entry { path: conflict.key.path.clone(), mode: value.mode, hash: value.hash, size },
            );
        }
        used.push(*id);
    }
    if !unresolved.is_empty() {
        return Err(MergeError::Unresolved(unresolved));
    }
    if let Some(collision) = path::find_collision(entries.keys().map(String::as_str)) {
        return Err(MergeError::Collision(collision));
    }
    used.sort();
    used.dedup();
    Ok(Resolved { manifest: Manifest::new(entries.into_values().collect()), used, merged_lines })
}

/// [`merge_lines`]'s entry, with its mode merged, and its file version.
fn line_merged<'a>(
    conflict: &Conflict,
    content: &impl Fn(&ContentHash) -> Option<&'a [u8]>,
) -> Option<(Entry, FileVersion)> {
    let version = merge_lines(conflict, content)?;
    let (b, x, y) = (conflict.key.base?, conflict.ours?, conflict.theirs?);
    let mode = merge_mode(b.mode, x.mode, y.mode)?;
    let entry = Entry { path: conflict.key.path.clone(), mode, hash: version.hash, size: version.bytes.len() as u64 };
    Some((entry, version))
}
