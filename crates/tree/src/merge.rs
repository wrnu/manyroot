//! Per-file merges, conflicts and resolutions (docs/design/weave.md, section 2
//! *Merge base* and *Per-file merge*, and section 1 *Proposals and
//! resolutions*).
//!
//! A file's value is its [`FileValue`] (content hash and mode), or absent.
//! Comparing a base `B` with sides `X` and `Y`: equal sides are taken; a side
//! equal to `B` gives way to the other; anything else is a conflict, keyed on
//! `(path, B, {X, Y})` with full file values, mode included. A resolution with
//! exactly that key settles it, the smallest id first. A rename is a delete
//! plus an add.

use std::collections::{BTreeMap, BTreeSet};

use manyroot::ContributionId;

use crate::format::{Entry, FileValue, Manifest, Resolution};
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

/// A merged tree, with the resolutions it used, ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub manifest: Manifest,
    pub used: Vec<ContributionId>,
}

/// Settles every conflict of `merged` with the resolution of its key that has the
/// smallest id. `size` gives a held file version's size, for a resolution's result.
pub fn resolve(
    merged: &Merged,
    resolutions: &Resolutions,
    size: impl Fn(&crate::ContentHash) -> Option<u64>,
) -> Result<Resolved, MergeError> {
    let mut entries = merged.settled.clone();
    let mut used = Vec::new();
    let mut unresolved = Vec::new();
    for conflict in &merged.conflicts {
        let Some((id, resolution)) = resolutions.for_key(&conflict.key).first() else {
            unresolved.push(conflict.clone());
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
    Ok(Resolved { manifest: Manifest::new(entries.into_values().collect()), used })
}
