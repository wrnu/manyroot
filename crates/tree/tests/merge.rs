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
