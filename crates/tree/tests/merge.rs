//! Per-file merges with file values, conflicts, and resolutions and their reuse
//! (docs/design/weave.md, section 2 *Per-file merge*; build step 6).

use bracken_tree::ContentHash;
use bracken_tree::format::{Entry, FileValue, Manifest, Mode, Resolution};
use bracken_tree::merge::{self, Key, MergeError, Resolutions};
use manyroot::ContributionId;

const F: Mode = Mode::File;
const X: Mode = Mode::Executable;
const L: Mode = Mode::Symlink;

fn entry(path: &str, bytes: &[u8], mode: Mode) -> Entry {
    Entry { path: path.into(), mode, hash: ContentHash::of(bytes), size: bytes.len() as u64 }
}

fn value(bytes: &[u8], mode: Mode) -> Option<FileValue> {
    Some(FileValue { hash: ContentHash::of(bytes), mode })
}

/// A tree of one file `p`, or an empty tree.
fn one(file: Option<(&[u8], Mode)>) -> Manifest {
    Manifest::new(file.map(|(bytes, mode)| entry("p", bytes, mode)).into_iter().collect())
}

fn id(n: u8) -> ContributionId {
    ContributionId([n; 32])
}

/// Every case of the per-file merge, on one path: (base, ours, theirs) and what it gives, a value or a
/// conflict.
#[test]
fn each_merge_case_takes_a_side_or_conflicts() {
    type Side<'a> = Option<(&'a [u8], Mode)>;
    let a: Side = Some((b"a", F));
    let b: Side = Some((b"b", F));
    let c: Side = Some((b"c", F));
    let a_exec: Side = Some((b"a", X));
    let b_exec: Side = Some((b"b", X));
    let a_link: Side = Some((b"a", L));
    // (case, base, ours, theirs, Ok(result) or Err(()) for a conflict)
    type Case<'a> = (&'a str, Side<'a>, Side<'a>, Side<'a>, Result<Side<'a>, ()>);
    let cases: [Case; 18] = [
        ("unchanged", a, a, a, Ok(a)),
        ("ours changed", a, b, a, Ok(b)),
        ("theirs changed", a, a, b, Ok(b)),
        ("both changed alike", a, b, b, Ok(b)),
        ("both changed differently", a, b, c, Err(())),
        ("added by ours", None, a, None, Ok(a)),
        ("added by theirs", None, None, a, Ok(a)),
        ("added alike on both", None, a, a, Ok(a)),
        ("added differently on both", None, a, b, Err(())),
        ("added with the same bytes and another mode", None, a, a_exec, Err(())),
        ("ours deleted, theirs unchanged", a, None, a, Ok(None)),
        ("deleted on both", a, None, None, Ok(None)),
        ("deleted against changed", a, None, b, Err(())),
        ("changed against deleted", a, b, None, Err(())),
        ("ours made it executable", a, a_exec, a, Ok(a_exec)),
        ("a mode change against a content change", a, a_exec, b, Err(())),
        ("two different mode changes", a, a_exec, a_link, Err(())),
        ("both changed bytes and mode alike", a, b_exec, b_exec, Ok(b_exec)),
    ];
    for (case, base, ours, theirs, want) in cases {
        let merged = merge::three_way(&one(base), &one(ours), &one(theirs));
        match want {
            Ok(result) => {
                assert!(merged.conflicts.is_empty(), "{case}: {:?}", merged.conflicts);
                let got = merged.settled.get("p").map(Entry::value);
                assert_eq!(got, result.and_then(|(bytes, mode)| value(bytes, mode)), "{case}");
            }
            Err(()) => {
                assert_eq!(merged.conflicts.len(), 1, "{case}");
                let conflict = &merged.conflicts[0];
                let v = |s: Side| s.and_then(|(bytes, mode)| value(bytes, mode));
                assert_eq!(conflict.key, Key::new("p".into(), v(base), [v(ours), v(theirs)]), "{case}");
                assert_eq!((conflict.ours, conflict.theirs), (v(ours), v(theirs)), "{case}");
                assert!(conflict.key.sides[0] <= conflict.key.sides[1], "{case}: sides are sorted");
            }
        }
    }
    // A mode-only change merged with a content change on another path: both apply.
    let base = Manifest::new(vec![entry("x", b"x", F), entry("y", b"y", F)]);
    let ours = Manifest::new(vec![entry("x", b"x", X), entry("y", b"y", F)]);
    let theirs = Manifest::new(vec![entry("x", b"x", F), entry("y", b"y2", F)]);
    let merged = merge::three_way(&base, &ours, &theirs);
    assert!(merged.conflicts.is_empty());
    assert_eq!(merged.settled["x"].mode, X);
    assert_eq!(merged.settled["y"].hash, ContentHash::of(b"y2"));
}

#[test]
fn a_resolution_is_reused_only_for_exactly_its_key() {
    let (base, ours, theirs) = (one(Some((b"a", F))), one(Some((b"b", F))), one(Some((b"c", F))));
    let merged = merge::three_way(&base, &ours, &theirs);
    let key = merged.conflicts[0].key.clone();
    let size = |h: &ContentHash| {
        [b"b".as_slice(), b"c", b"bc"].iter().find(|b| ContentHash::of(b) == *h).map(|b| b.len() as u64)
    };

    // No resolution: unresolved.
    let none = Resolutions::default();
    assert!(matches!(merge::resolve(&merged, &none, size), Err(MergeError::Unresolved(c)) if c.len() == 1));

    // Resolutions for nearly the same conflict don't apply: another base, a side with another mode,
    // another path.
    let mut near = Resolutions::default();
    let result = value(b"bc", F);
    near.insert(id(1), Resolution::new("p".into(), value(b"z", F), [value(b"b", F), value(b"c", F)], result));
    near.insert(id(2), Resolution::new("p".into(), value(b"a", F), [value(b"b", X), value(b"c", F)], result));
    near.insert(id(3), Resolution::new("q".into(), value(b"a", F), [value(b"b", F), value(b"c", F)], result));
    assert!(matches!(merge::resolve(&merged, &near, size), Err(MergeError::Unresolved(_))));

    // The exact key, given with its sides in either order, settles it; of two, the smallest id is used.
    let mut exact = near.clone();
    exact.insert(id(9), Resolution::new("p".into(), value(b"a", F), [value(b"c", F), value(b"b", F)], value(b"c", F)));
    exact.insert(id(5), Resolution::new("p".into(), value(b"a", F), [value(b"b", F), value(b"c", F)], result));
    assert_eq!(exact.for_key(&key).len(), 2);
    let resolved = merge::resolve(&merged, &exact, size).expect("resolved");
    assert_eq!(resolved.used, [id(5)]);
    assert_eq!(resolved.manifest.entries, [entry("p", b"bc", F)]);

    // A resolution to delete the file deletes it.
    let mut delete = Resolutions::default();
    delete.insert(id(4), Resolution::new("p".into(), value(b"a", F), [value(b"b", F), value(b"c", F)], None));
    let resolved = merge::resolve(&merged, &delete, size).expect("resolved");
    assert!(resolved.manifest.entries.is_empty());
    assert_eq!(resolved.used, [id(4)]);

    // A result whose file version isn't held can't be used.
    let mut unheld = Resolutions::default();
    unheld
        .insert(id(6), Resolution::new("p".into(), value(b"a", F), [value(b"b", F), value(b"c", F)], value(b"new", F)));
    assert!(matches!(merge::resolve(&merged, &unheld, size), Err(MergeError::ResultNotHeld { .. })));
}

#[test]
fn delete_against_change_is_a_conflict_a_resolution_can_settle_either_way() {
    let base = one(Some((b"a", F)));
    let (deleted, changed) = (one(None), one(Some((b"b", F))));
    for (ours, theirs) in [(&deleted, &changed), (&changed, &deleted)] {
        let merged = merge::three_way(&base, ours, theirs);
        assert_eq!(merged.conflicts.len(), 1);
        let key = &merged.conflicts[0].key;
        assert_eq!(key.sides, [None, value(b"b", F)], "the deleted side is absent, and sorts first");
        for (result, want) in [(None, vec![]), (value(b"b", F), vec![entry("p", b"b", F)])] {
            let mut r = Resolutions::default();
            r.insert(id(1), Resolution::new("p".into(), value(b"a", F), [None, value(b"b", F)], result));
            let size = |h: &ContentHash| (*h == ContentHash::of(b"b")).then_some(1);
            assert_eq!(merge::resolve(&merged, &r, size).expect("resolved").manifest.entries, want);
        }
    }
}

#[test]
fn a_merge_whose_paths_collide_is_refused() {
    // Each side adds a path the other's folds onto.
    let base = Manifest::new(vec![]);
    let ours = Manifest::new(vec![entry("Read.md", b"a", F)]);
    let theirs = Manifest::new(vec![entry("read.md", b"b", F)]);
    let merged = merge::three_way(&base, &ours, &theirs);
    assert!(merged.conflicts.is_empty());
    assert!(matches!(merge::resolve(&merged, &Resolutions::default(), |_| None), Err(MergeError::Collision(_))));
    // A file on one side, a directory of the same name on the other.
    let ours = Manifest::new(vec![entry("d", b"a", F)]);
    let theirs = Manifest::new(vec![entry("d/x", b"b", F)]);
    let merged = merge::three_way(&base, &ours, &theirs);
    assert!(matches!(merge::resolve(&merged, &Resolutions::default(), |_| None), Err(MergeError::Collision(_))));
}

/// Bytes by hash, for the line merge: every version these tests make.
fn held<'a>(versions: &'a [&'a [u8]]) -> impl Fn(&ContentHash) -> Option<&'a [u8]> {
    move |h| versions.iter().copied().find(|b| ContentHash::of(b) == *h)
}

/// The line merge (task 7cbd0a0ce7d0): each case of the table, with text a line merge could join.
/// Present, regular, text versions on all three sides merge line by line where their lines merge;
/// everything else is a per-file conflict, as before.
#[test]
fn a_conflict_merges_line_by_line_only_for_three_text_files() {
    type Side<'a> = Option<(&'a [u8], Mode)>;
    let base: &[u8] = b"1\n2\n3\n4\n5\n";
    let top: &[u8] = b"one\n2\n3\n4\n5\n";
    let bottom: &[u8] = b"1\n2\n3\n4\nfive\n";
    let both: &[u8] = b"one\n2\n3\n4\nfive\n";
    let near: &[u8] = b"1\ntwo\n3\n4\n5\n";
    let latin1: &[u8] = b"1\n2\n3\n4\n5\xe9\n";
    let latin1_top: &[u8] = b"one\n2\n3\n4\n5\xe9\n";
    let latin1_bottom: &[u8] = b"1\n2\n3\n4\nfive\xe9\n";
    let nul: &[u8] = b"1\n2\n3\n4\n5\0\n";
    let nul_top: &[u8] = b"one\n2\n3\n4\n5\0\n";
    let nul_bottom: &[u8] = b"1\n2\n3\n4\nfive\0\n";
    let versions = [base, top, bottom, both, near, latin1, latin1_top, latin1_bottom, nul, nul_top, nul_bottom];
    type Case<'a> = (&'a str, Side<'a>, Side<'a>, Side<'a>, Result<Side<'a>, ()>);
    let cases: [Case; 11] = [
        ("lines apart", Some((base, F)), Some((top, F)), Some((bottom, F)), Ok(Some((both, F)))),
        ("adjacent lines", Some((base, F)), Some((top, F)), Some((near, F)), Err(())),
        (
            "a mode change against a content change",
            Some((base, F)),
            Some((base, X)),
            Some((bottom, F)),
            Ok(Some((bottom, X))),
        ),
        ("a mode change and lines apart", Some((base, F)), Some((top, X)), Some((bottom, F)), Ok(Some((both, X)))),
        ("two mode changes", Some((base, F)), Some((top, X)), Some((bottom, L)), Err(())),
        ("a symlink", Some((base, L)), Some((top, L)), Some((bottom, L)), Err(())),
        ("added on both", None, Some((top, F)), Some((bottom, F)), Err(())),
        ("deleted against changed", Some((base, F)), None, Some((bottom, F)), Err(())),
        ("changed against deleted", Some((base, F)), Some((top, F)), None, Err(())),
        ("not UTF-8", Some((latin1, F)), Some((latin1_top, F)), Some((latin1_bottom, F)), Err(())),
        ("a NUL", Some((nul, F)), Some((nul_top, F)), Some((nul_bottom, F)), Err(())),
    ];
    let size = |h: &ContentHash| versions.iter().find(|b| ContentHash::of(b) == *h).map(|b| b.len() as u64);
    for (case, b, x, y, want) in &cases {
        let merged = merge::three_way(&one(*b), &one(*x), &one(*y));
        assert_eq!(merged.conflicts.len(), 1, "{case}: a per-file conflict first");
        let got = merge::resolve_lines(&merged, &Resolutions::default(), size, held(&versions));
        match want {
            Ok(Some((bytes, mode))) => {
                let resolved = got.unwrap_or_else(|e| panic!("{case}: {e}"));
                assert_eq!(resolved.manifest.entries, [entry("p", bytes, *mode)], "{case}");
                assert_eq!(resolved.merged_lines.len(), 1, "{case}");
                assert_eq!(resolved.merged_lines[0].bytes, *bytes, "{case}");
                assert!(resolved.used.is_empty(), "{case}: no resolution");
            }
            Ok(None) => unreachable!(),
            Err(()) => assert!(matches!(got, Err(MergeError::Unresolved(c)) if c.len() == 1), "{case}"),
        }
        // Without the bytes, nothing merges line by line: the plain resolve, as before.
        assert!(matches!(merge::resolve(&merged, &Resolutions::default(), size), Err(MergeError::Unresolved(_))));
    }
}

/// A resolution comes before the line merge: an integrator's resolution of the key settles the file even
/// where the lines would merge, so a merge recorded before line merging computes the same tree.
#[test]
fn a_resolution_comes_before_the_line_merge() {
    let (base, top, bottom): (&[u8], &[u8], &[u8]) = (b"1\n2\n3\n4\n5\n", b"one\n2\n3\n4\n5\n", b"1\n2\n3\n4\nfive\n");
    let chosen: &[u8] = b"chosen\n";
    let versions = [base, top, bottom, chosen];
    let size = |h: &ContentHash| versions.iter().find(|b| ContentHash::of(b) == *h).map(|b| b.len() as u64);
    let merged = merge::three_way(&one(Some((base, F))), &one(Some((top, F))), &one(Some((bottom, F))));
    let mut resolutions = Resolutions::default();
    resolutions.insert(
        id(9),
        Resolution {
            v: 0,
            path: "p".into(),
            base: value(base, F),
            sides: merged.conflicts[0].key.sides,
            result: value(chosen, F),
        },
    );
    let resolved = merge::resolve_lines(&merged, &resolutions, size, held(&versions)).expect("resolved");
    assert_eq!(resolved.manifest.entries, [entry("p", chosen, F)]);
    assert_eq!(resolved.used, [id(9)]);
    assert!(resolved.merged_lines.is_empty());
}

/// Where a text conflict's lines conflict, counted from 1; none for a file that isn't text.
#[test]
fn text_conflicts_name_the_lines() {
    let (base, top, near): (&[u8], &[u8], &[u8]) = (b"1\n2\n3\n", b"one\n2\n3\n", b"1\ntwo\n3\n");
    let versions = [base, top, near];
    let merged = merge::three_way(&one(Some((base, F))), &one(Some((top, F))), &one(Some((near, F))));
    assert_eq!(merge::text_conflicts(&merged.conflicts[0], held(&versions)), Some(vec![(1, 2)]));
    let labels = bracken_tree::text_merge::Labels { ours: "into", base: "base", theirs: "from" };
    let marked = merge::conflict_markers(&merged.conflicts[0], held(&versions), &labels).expect("text");
    assert_eq!(marked, b"<<<<<<< into\none\n2\n||||||| base\n1\n2\n=======\n1\ntwo\n>>>>>>> from\n3\n");
    // Neither for symlinks, nor for versions this replica doesn't hold.
    let merged = merge::three_way(&one(Some((base, L))), &one(Some((top, L))), &one(Some((near, L))));
    assert_eq!(merge::text_conflicts(&merged.conflicts[0], held(&versions)), None);
    assert_eq!(merge::conflict_markers(&merged.conflicts[0], held(&versions), &labels), None);
    let merged = merge::three_way(&one(Some((base, F))), &one(Some((top, F))), &one(Some((near, F))));
    assert_eq!(merge::conflict_markers(&merged.conflicts[0], held(&[base, top]), &labels), None);
}

/// Finding d189e282cdee: each side may be 8 MiB, so a clean merge can be bigger than one file version
/// may be. It stays a per-file conflict, rather than failing when the tree is written.
#[test]
fn a_line_merge_too_big_to_write_stays_a_conflict() {
    let base: &[u8] = b"a\nb\nc\n";
    let line = [b'x'; 1023].iter().chain(b"\n").copied().collect::<Vec<u8>>();
    let big: Vec<u8> = line.repeat(8 * 1024 - 1);
    let ours = [&big[..], base].concat();
    let theirs = [base, &big[..]].concat();
    assert!(bracken_tree::text_merge::is_text(&ours) && bracken_tree::text_merge::is_text(&theirs));
    assert!(ours.len() + theirs.len() - base.len() > bracken_tree::import::MAX_FILE_BYTES);
    let versions = [base, &ours[..], &theirs[..]];
    let size = |h: &ContentHash| versions.iter().find(|b| ContentHash::of(b) == *h).map(|b| b.len() as u64);
    let merged = merge::three_way(&one(Some((base, F))), &one(Some((&ours, F))), &one(Some((&theirs, F))));
    let got = merge::resolve_lines(&merged, &Resolutions::default(), size, held(&versions));
    assert!(matches!(got, Err(MergeError::Unresolved(ref c)) if c.len() == 1), "{got:?}");
    // Its lines merge, so the listing names no conflicting region.
    assert_eq!(merge::text_conflicts(&merged.conflicts[0], held(&versions)), Some(vec![]));
}

/// Finding 933f535c9476: over the diff cap, the listing names no lines either, as for a file that isn't
/// text.
#[test]
fn over_the_diff_cap_the_listing_names_no_lines() {
    let n = bracken_tree::text_merge::MAX_DIFF_LINES + 1;
    let middle = |tag: &str| -> String { (0..n).map(|i| format!("{tag}{i}\n")).collect() };
    let base = format!("h\ngap\n{}", middle("b"));
    let (ours, theirs) = (format!("h\ngap\n{}", middle("o")), format!("H\ngap\n{}", middle("b")));
    let versions = [base.as_bytes(), ours.as_bytes(), theirs.as_bytes()];
    let merged = merge::three_way(
        &one(Some((base.as_bytes(), F))),
        &one(Some((ours.as_bytes(), F))),
        &one(Some((theirs.as_bytes(), F))),
    );
    assert_eq!(merge::text_conflicts(&merged.conflicts[0], held(&versions)), None);
    let size = |h: &ContentHash| versions.iter().find(|b| ContentHash::of(b) == *h).map(|b| b.len() as u64);
    assert!(merge::resolve_lines(&merged, &Resolutions::default(), size, held(&versions)).is_err());
}
