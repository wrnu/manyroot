//! Differences between two manifests, file by file, and a unified diff of a text
//! file's two versions (docs/design/weave.md, section 3, `weave diff`).

use crate::format::{Change, FileValue, Manifest, Mode};

/// How a file differs between two manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Added,
    Deleted,
    /// Its bytes changed (its mode may have too).
    Modified,
    /// Only its mode changed.
    ModeChanged,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Deleted => "deleted",
            Kind::Modified => "modified",
            Kind::ModeChanged => "mode changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub kind: Kind,
    pub before: Option<FileValue>,
    pub after: Option<FileValue>,
}

/// Every path that differs from `a` to `b`, in path order.
pub fn manifests(a: &Manifest, b: &Manifest) -> Vec<FileDiff> {
    Change::between(a, b)
        .entries()
        .iter()
        .map(|e| {
            let kind = match (e.before, e.after) {
                (None, _) => Kind::Added,
                (_, None) => Kind::Deleted,
                (Some(x), Some(y)) if x.hash == y.hash => Kind::ModeChanged,
                _ => Kind::Modified,
            };
            FileDiff { path: e.path.clone(), kind, before: e.before, after: e.after }
        })
        .collect()
}

/// Whether a file's bytes are shown as text: a regular file, UTF-8, with no NUL.
pub fn is_text(mode: Mode, bytes: &[u8]) -> bool {
    mode != Mode::Symlink && !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The largest product of the two files' line counts diffed line by line; past
/// it, [`unified`] says only that the files differ.
pub const MAX_DIFF_CELLS: usize = 16_000_000;

enum Op<'a> {
    Same(&'a str),
    Del(&'a str),
    Ins(&'a str),
}

/// A unified diff from `old` to `new` with three lines of context, under
/// `--- a/<path>` and `+++ b/<path>` headers. `None` if they're too large to diff.
pub fn unified(path: &str, old: &str, new: &str) -> Option<String> {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    if a.len().saturating_mul(b.len()) > MAX_DIFF_CELLS {
        return None;
    }
    // Longest common subsequence, by suffix lengths.
    let width = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push(Op::Same(a[i]));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
            // Deletions before insertions, as git shows them.
            ops.push(Op::Del(a[i]));
            i += 1;
        } else {
            ops.push(Op::Ins(b[j]));
            j += 1;
        }
    }

    const CONTEXT: usize = 3;
    let changed: Vec<usize> =
        ops.iter().enumerate().filter(|(_, op)| !matches!(op, Op::Same(_))).map(|(k, _)| k).collect();
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let mut k = 0;
    while k < changed.len() {
        // A hunk: changes no more than 2 * CONTEXT unchanged lines apart.
        let start = changed[k].saturating_sub(CONTEXT);
        let mut last = changed[k];
        while k + 1 < changed.len() && changed[k + 1] - last <= 2 * CONTEXT {
            k += 1;
            last = changed[k];
        }
        k += 1;
        let end = (last + CONTEXT + 1).min(ops.len());
        // Line numbers at the hunk's start.
        let (mut old_line, mut new_line) = (1, 1);
        for op in &ops[..start] {
            match op {
                Op::Same(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                Op::Del(_) => old_line += 1,
                Op::Ins(_) => new_line += 1,
            }
        }
        let old_len = ops[start..end].iter().filter(|op| !matches!(op, Op::Ins(_))).count();
        let new_len = ops[start..end].iter().filter(|op| !matches!(op, Op::Del(_))).count();
        let at = |line: usize, len: usize| if len == 0 { line - 1 } else { line };
        out.push_str(&format!("@@ -{},{old_len} +{},{new_len} @@\n", at(old_line, old_len), at(new_line, new_len)));
        for op in &ops[start..end] {
            let (sign, line) = match op {
                Op::Same(l) => (' ', *l),
                Op::Del(l) => ('-', *l),
                Op::Ins(l) => ('+', *l),
            };
            out.push(sign);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Entry;
    use crate::hash::ContentHash;

    fn entry(path: &str, bytes: &[u8], mode: Mode) -> Entry {
        Entry { path: path.to_owned(), mode, hash: ContentHash::of(bytes), size: bytes.len() as u64 }
    }

    #[test]
    fn file_changes_are_classified() {
        let a = Manifest::new(vec![
            entry("same", b"s", Mode::File),
            entry("gone", b"g", Mode::File),
            entry("edit", b"1", Mode::File),
            entry("chmod", b"c", Mode::File),
        ]);
        let b = Manifest::new(vec![
            entry("same", b"s", Mode::File),
            entry("new", b"n", Mode::File),
            entry("edit", b"2", Mode::File),
            entry("chmod", b"c", Mode::Executable),
        ]);
        let diffs = manifests(&a, &b);
        let got: Vec<(&str, Kind)> = diffs.iter().map(|d| (d.path.as_str(), d.kind)).collect();
        assert_eq!(
            got,
            [("chmod", Kind::ModeChanged), ("edit", Kind::Modified), ("gone", Kind::Deleted), ("new", Kind::Added)]
        );
    }

    #[test]
    fn unified_diffs_match_git() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nb\nC\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let diff = unified("x.txt", old, new).expect("small enough");
        assert_eq!(
            diff,
            "--- a/x.txt\n+++ b/x.txt\n@@ -1,6 +1,6 @@\n a\n b\n-c\n+C\n d\n e\n f\n@@ -8,3 +8,4 @@\n h\n i\n j\n+k\n"
        );
        assert_eq!(unified("e", "", "x\n").expect("small"), "--- a/e\n+++ b/e\n@@ -0,0 +1,1 @@\n+x\n");
        assert_eq!(
            unified("n", "x", "y").expect("small"),
            "--- a/n\n+++ b/n\n@@ -1,1 +1,1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n"
        );
        assert_eq!(unified("same", "a\n", "a\n").expect("small"), "--- a/same\n+++ b/same\n");
    }

    #[test]
    fn binary_and_symlinks_are_not_text() {
        assert!(is_text(Mode::File, b"hello\n"));
        assert!(!is_text(Mode::File, b"a\0b"));
        assert!(!is_text(Mode::Executable, b"\xff"));
        assert!(!is_text(Mode::Symlink, b"target"));
    }
}
