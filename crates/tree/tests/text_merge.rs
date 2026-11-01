//! The line-level merge of text files (task 7cbd0a0ce7d0, design 68b57f9aed6d): its cases. git's
//! `merge-file` as an oracle, the pinned cases where it differs, and determinism are in format_guard.rs
//! (answer 55e0a67c68a2 (3)).

use bracken_tree::text_merge::{Labels, MAX_DIFF_LINES, MAX_TEXT_BYTES, conflicts, is_text, markers, merge};

fn m(base: &str, ours: &str, theirs: &str) -> Option<String> {
    merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes()).map(|b| String::from_utf8(b).expect("text in"))
}

#[test]
fn line_cases() {
    let base = "a\nb\nc\nd\ne\n";
    // Disjoint hunks, one line apart or more, merge.
    assert_eq!(m(base, "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n").as_deref(), Some("A\nb\nc\nd\nE\n"));
    assert_eq!(m(base, "A\nb\nc\nd\ne\n", "a\nb\nC\nd\ne\n").as_deref(), Some("A\nb\nC\nd\ne\n"));
    // Adjacent lines conflict, as in git.
    assert_eq!(m(base, "A\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n"), None);
    // The same change on both sides is taken once; the same line changed two ways conflicts.
    assert_eq!(m(base, "a\nB\nc\nd\ne\n", "a\nB\nc\nd\ne\n").as_deref(), Some("a\nB\nc\nd\ne\n"));
    assert_eq!(m(base, "a\nB\nc\nd\ne\n", "a\nX\nc\nd\ne\n"), None);
    // Insertions at one point: equal ones merge, different ones conflict.
    assert_eq!(m(base, "a\nb\nN\nc\nd\ne\n", "a\nb\nN\nc\nd\ne\n").as_deref(), Some("a\nb\nN\nc\nd\ne\n"));
    assert_eq!(m(base, "a\nb\nN\nc\nd\ne\n", "a\nb\nM\nc\nd\ne\n"), None);
    // Start and end of the file.
    assert_eq!(m(base, "0\na\nb\nc\nd\ne\n", "a\nb\nc\nd\ne\nf\n").as_deref(), Some("0\na\nb\nc\nd\ne\nf\n"));
    // A deletion on one side, an edit far from it on the other.
    assert_eq!(m(base, "a\nc\nd\ne\n", "a\nb\nc\nd\nE\n").as_deref(), Some("a\nc\nd\nE\n"));
    // No final newline: a line of its own, so adding one on one side merges with an edit far away.
    assert_eq!(m("a\nb\nc\nd", "a\nb\nc\nd\n", "A\nb\nc\nd").as_deref(), Some("A\nb\nc\nd\n"));
    // CRLF: bytes like any other.
    assert_eq!(
        m("a\r\nb\r\nc\r\nd\r\n", "A\r\nb\r\nc\r\nd\r\n", "a\r\nb\r\nc\r\nD\r\n").as_deref(),
        Some("A\r\nb\r\nc\r\nD\r\n")
    );
    assert_eq!(m("a\nb\n", "a\r\nb\n", "a\nb\r\n"), None);
    // An empty file on a side, or as the base.
    assert_eq!(m(base, "", base).as_deref(), Some(""));
    assert_eq!(m("", "x\n", "y\n"), None);
    // Where it conflicts: base line ranges, from 0.
    assert_eq!(
        conflicts(base.as_bytes(), b"A\nb\nc\nd\ne\n", b"a\nB\nc\nd\ne\n"),
        Some(vec![std::ops::Range { start: 0, end: 2 }])
    );
    assert_eq!(conflicts(base.as_bytes(), b"A\nb\nc\nd\ne\n", b"a\nb\nc\nd\nE\n"), Some(vec![]));
}

#[test]
fn what_counts_as_text() {
    assert!(is_text(b"plain\n") && is_text(b"") && is_text("caf\u{e9}\n".as_bytes()));
    assert!(!is_text(b"caf\xe9\n"), "Latin-1 isn't UTF-8");
    assert!(!is_text(b"a\0b\n"), "a NUL anywhere");
    let mut big = vec![b'a'; MAX_TEXT_BYTES];
    assert!(is_text(&big));
    big.push(b'\n');
    assert!(!is_text(&big), "over 8 MiB");
}

/// `n` distinct lines, each starting with `tag`.
fn numbered(tag: &str, n: usize) -> String {
    (0..n).map(|i| format!("{tag}{i}\n")).collect()
}

/// Finding 933f535c9476, answer 47a1c1088fd6: a diff's lines are capped once the common first and last
/// lines are set aside, so a big file with small edits merges, and a big rewrite has no line merge.
#[test]
fn a_diff_over_the_line_cap_has_no_line_merge() {
    // Head and tail lines both sides keep; line 0 is changed on one side, a gap line apart from the
    // middle the other side rewrites.
    let file = |first: &str, middle: &str| format!("{first}\ngap\n{middle}tail\n");
    for (n, merges) in [(MAX_DIFF_LINES, true), (MAX_DIFF_LINES + 1, false)] {
        let base = file("h", &numbered("b", n));
        let theirs = file("H", &numbered("b", n));
        // The rewritten middle: n base lines and n of its own left after the common ones.
        let ours = file("h", &numbered("o", n));
        let got = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes());
        assert_eq!(got.is_some(), merges, "{n} lines");
        assert_eq!(conflicts(base.as_bytes(), ours.as_bytes(), theirs.as_bytes()).is_some(), merges, "{n} lines");
        // The same either way round.
        assert_eq!(merge(base.as_bytes(), theirs.as_bytes(), ours.as_bytes()).is_some(), merges, "{n} lines");
        // Over the cap on the side only: n new lines inserted, none of the base's left.
        let inserted = file("h", &format!("{}{}", numbered("b", n), numbered("i", n)));
        let side_only = merge(base.as_bytes(), inserted.as_bytes(), theirs.as_bytes());
        assert_eq!(side_only.is_some(), merges, "{n} lines inserted");
    }
    // A file far over the cap, with edits near its two ends only, merges.
    let base = numbered("x", 4 * MAX_DIFF_LINES);
    let ours = base.replacen("x1\n", "X1\n", 1);
    let theirs = format!("{base}y\n");
    let both = merge(base.as_bytes(), ours.as_bytes(), theirs.as_bytes()).expect("merges");
    assert_eq!(String::from_utf8(both).expect("text"), format!("{ours}y\n"));
    // Two sides equal, or one unchanged, need no diff, so no cap.
    let rewrite = numbered("o", MAX_DIFF_LINES + 1);
    assert_eq!(merge(base.as_bytes(), rewrite.as_bytes(), base.as_bytes()).as_deref(), Some(rewrite.as_bytes()));
}

fn marked(base: &str, ours: &str, theirs: &str) -> String {
    let labels = Labels { ours: "into", base: "base", theirs: "from" };
    String::from_utf8(markers(base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), &labels).expect("within the cap"))
        .expect("text in")
}

#[test]
fn markers_hold_each_conflicting_region_and_the_merge_around_it() {
    let base = "a\nb\nc\nd\ne\nf\ng\n";
    // Line 2 changed two ways, and line 6 on one side only, which merges around them.
    assert_eq!(
        marked(base, "a\nB\nc\nd\ne\nf\ng\n", "a\nX\nc\nd\ne\nF\ng\n"),
        "a\n<<<<<<< into\nB\n||||||| base\nb\n=======\nX\n>>>>>>> from\nc\nd\ne\nF\ng\n"
    );
    // Two regions; an insertion on both sides has an empty base section.
    assert_eq!(
        marked(base, "A\nb\nc\nd\nN\ne\nf\ng\n", "Z\nb\nc\nd\nM\ne\nf\ng\n"),
        "<<<<<<< into\nA\n||||||| base\na\n=======\nZ\n>>>>>>> from\nb\nc\nd\n\
         <<<<<<< into\nN\n||||||| base\n=======\nM\n>>>>>>> from\ne\nf\ng\n"
    );
    // A deletion against an edit: the deleting side's section is empty.
    assert_eq!(
        marked("a\nb\nc\n", "a\nc\n", "a\nB\nc\n"),
        "a\n<<<<<<< into\n||||||| base\nb\n=======\nB\n>>>>>>> from\nc\n"
    );
    // A last line without a line end gets one before the next marker.
    assert_eq!(marked("a", "b", "c"), "<<<<<<< into\nb\n||||||| base\na\n=======\nc\n>>>>>>> from\n");
    // With no conflict, it is the merge.
    let (ours, theirs) = ("A\nb\nc\nd\ne\nf\ng\n", "a\nb\nc\nd\ne\nf\nG\n");
    assert_eq!(Some(marked(base, ours, theirs)), m(base, ours, theirs));
    assert_eq!(marked(base, base, base), base);
}
