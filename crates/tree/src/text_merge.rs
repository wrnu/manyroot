//! The line-level three-way merge of a text file (docs/design/weave.md, *Per-file merge*; task
//! `7cbd0a0ce7d0`, design `68b57f9aed6d`).
//!
//! It is Weave's own deterministic rule, modelled on git's `merge-file` but not byte for byte git's: the
//! histogram diff can align repeated lines differently, so in rare cases one merges where the other
//! conflicts (answer `55e0a67c68a2`; the format guard lists the known cases). It gives a merge or
//! nothing: a conflict anywhere in the file leaves the whole file a conflict, so the shape of a conflict
//! region never matters.
//!
//! - A line ends after each `\n`. A last line without one is a line of its own, so `x` and `x\n`
//!   differ. Nothing is normalised: whitespace, case and line endings are bytes like any other.
//! - Each side is diffed against the base with the histogram algorithm (git's `merge-ort` default), its
//!   hunks slid as far down as they go, as git's `xdl_change_compact` does without the indent
//!   heuristic.
//! - The hunks of both sides are taken in base order. Hunks whose base ranges overlap or touch, one
//!   ending where the next starts, form one region, the gaps between them included. A region holding
//!   hunks of one side only takes that side. One holding both is clean only if both sides give the
//!   same lines for it. So edits to adjacent lines conflict, and the same edit on both sides doesn't.
//!
//! - Each diff is bounded, since the histogram diff is quadratic in lines that occur once: after
//!   their common first and last lines are set aside, the base and the side may each have at most
//!   [`MAX_DIFF_LINES`] lines left. Over that, there is no line merge, and the file is a conflict, as a
//!   file that isn't text is (finding `933f535c9476`, answer `47a1c1088fd6`). It counts lines, never time.
//!
//! The result is a pure function of the three byte strings. The format guard pins it with golden
//! vectors, and imara-diff is pinned exactly, so any change to either is reviewed.

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

/// The largest file that merges line by line: half the 16 MiB contribution limit, which bounds the
/// diff's worst case.
pub const MAX_TEXT_BYTES: usize = 8 << 20;

/// The most lines the base and a side may each have left once their common first and last lines are set
/// aside: the bound on one diff's cost.
pub const MAX_DIFF_LINES: usize = 10_000;

/// Whether `bytes` merge line by line: at most [`MAX_TEXT_BYTES`], valid UTF-8, and no NUL byte.
/// Stricter than git, which looks for a NUL in the first 8,000 bytes only: where the two differ, Weave
/// conflicts and git may merge, never the other way round.
pub fn is_text(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_TEXT_BYTES && !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// One side's change: base lines `base` replaced by the side's lines `lines`.
struct Change<'a> {
    base: Range<u32>,
    lines: Vec<&'a [u8]>,
    side: usize,
}

/// The changes that turn `base` into `side`, in base order.
fn changes<'a>(base: &'a [u8], side_bytes: &'a [u8], side: usize) -> Vec<Change<'a>> {
    let input = InternedInput::new(base, side_bytes);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_no_heuristic(&input);
    diff.hunks()
        .map(|h| Change {
            base: h.before,
            lines: input.after[h.after.start as usize..h.after.end as usize]
                .iter()
                .map(|&t| input.interner[t])
                .collect(),
            side,
        })
        .collect()
}

/// Whether the diff of `base` against `side` is within [`MAX_DIFF_LINES`].
fn within_cap(base: &[&[u8]], side: &[u8]) -> bool {
    let side = lines(side);
    let prefix = base.iter().zip(&side).take_while(|(a, b)| a == b).count();
    let suffix = base[prefix..].iter().rev().zip(side[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    base.len() - prefix - suffix <= MAX_DIFF_LINES && side.len() - prefix - suffix <= MAX_DIFF_LINES
}

/// The lines of `base`, each with its line end.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&b| b == b'\n').collect()
}

/// What `changes`, all of one side, make of base lines `range`.
fn apply<'a>(base: &[&'a [u8]], range: Range<u32>, changes: &[&Change<'a>]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut at = range.start;
    for c in changes {
        out.extend_from_slice(&base[at as usize..c.base.start as usize]);
        out.extend_from_slice(&c.lines);
        at = c.base.end;
    }
    out.extend_from_slice(&base[at as usize..range.end as usize]);
    out
}

/// The merge of `ours` and `theirs` against `base`, or `None` for a conflict, or a diff over
/// [`MAX_DIFF_LINES`]. Callers check [`is_text`] first; this merges whatever it is given.
pub fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    let (out, conflicts) = walk(base, ours, theirs, None)?;
    conflicts.is_empty().then_some(out)
}

/// The base line ranges where `ours` and `theirs` conflict, counting lines from 0, in order: empty when
/// they merge. A range is empty where both sides insert at one point. `None` for a diff over
/// [`MAX_DIFF_LINES`], which has no line merge.
pub fn conflicts(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<Range<u32>>> {
    Some(walk(base, ours, theirs, None)?.1)
}

/// The labels of [`markers`]' three sections: ours, the base, theirs.
pub struct Labels<'a> {
    pub ours: &'a str,
    pub base: &'a str,
    pub theirs: &'a str,
}

/// The merge with each conflicting region written out between diff3-style markers, for a person to
/// edit: `<<<<<<< ours`, our lines, `||||||| base`, the base's, `=======`, theirs, `>>>>>>> theirs`.
/// A section whose last line has no line end gets a `\n` before the next marker. `None` for a diff over
/// [`MAX_DIFF_LINES`]. An editing aid only: what it gives never enters a tree.
pub fn markers(base: &[u8], ours: &[u8], theirs: &[u8], labels: &Labels) -> Option<Vec<u8>> {
    Some(walk(base, ours, theirs, Some(labels))?.0)
}

/// Appends `lines`, then a line end if the last of them has none.
fn section(out: &mut Vec<u8>, lines: &[&[u8]]) {
    for line in lines {
        out.extend_from_slice(line);
    }
    if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
        out.push(b'\n');
    }
}

/// The merge and every conflicting region's base range, or `None` for a diff over [`MAX_DIFF_LINES`]. In
/// a conflicting region it takes ours, or with `labels` writes all three sides between markers.
fn walk(base: &[u8], ours: &[u8], theirs: &[u8], labels: Option<&Labels>) -> Option<(Vec<u8>, Vec<Range<u32>>)> {
    if ours == theirs || theirs == base {
        return Some((ours.to_vec(), Vec::new()));
    }
    if ours == base {
        return Some((theirs.to_vec(), Vec::new()));
    }
    let mut conflicts = Vec::new();
    let base_lines = lines(base);
    if !(within_cap(&base_lines, ours) && within_cap(&base_lines, theirs)) {
        return None;
    }
    let mut all = changes(base, ours, 0);
    all.extend(changes(base, theirs, 1));
    // Base order; at one start, a pure insertion first, then by side, so the walk is the same every time.
    all.sort_by_key(|c| (c.base.start, c.base.end, c.side));
    let mut out: Vec<u8> = Vec::with_capacity(ours.len().max(theirs.len()));
    let mut at = 0u32;
    let mut i = 0;
    while i < all.len() {
        // A region: every change whose base range overlaps or touches the ones before it.
        let mut end = all[i].base.end;
        let mut j = i + 1;
        while j < all.len() && all[j].base.start <= end {
            end = end.max(all[j].base.end);
            j += 1;
        }
        let region = &all[i..j];
        let start = region[0].base.start;
        for line in &base_lines[at as usize..start as usize] {
            out.extend_from_slice(line);
        }
        let side = |s: usize| -> Vec<&Change> { region.iter().filter(|c| c.side == s).collect() };
        let (mine, yours) = (side(0), side(1));
        let taken = if yours.is_empty() {
            apply(&base_lines, start..end, &mine)
        } else if mine.is_empty() {
            apply(&base_lines, start..end, &yours)
        } else {
            let (a, b) = (apply(&base_lines, start..end, &mine), apply(&base_lines, start..end, &yours));
            if a != b {
                conflicts.push(start..end);
                if let Some(l) = labels {
                    out.extend_from_slice(format!("<<<<<<< {}\n", l.ours).as_bytes());
                    section(&mut out, &a);
                    out.extend_from_slice(format!("||||||| {}\n", l.base).as_bytes());
                    section(&mut out, &base_lines[start as usize..end as usize]);
                    out.extend_from_slice(b"=======\n");
                    section(&mut out, &b);
                    out.extend_from_slice(format!(">>>>>>> {}\n", l.theirs).as_bytes());
                    at = end;
                    i = j;
                    continue;
                }
            }
            a
        };
        for line in taken {
            out.extend_from_slice(line);
        }
        at = end;
        i = j;
    }
    for line in &base_lines[at as usize..] {
        out.extend_from_slice(line);
    }
    Some((out, conflicts))
}
