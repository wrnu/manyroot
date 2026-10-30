//! Every path, folding, collision and symlink-target rule, positive and negative
//! (docs/design/weave.md, *Unit: a file, by path*, and build step 1).

use bracken_tree::path::{find_collision, fold, validate_path, validate_symlink_target};
use bracken_tree::{Collision, PathError};

#[test]
fn ordinary_paths_are_valid() {
    for path in [
        "a",
        "Cargo.toml",
        "crates/tree/src/lib.rs",
        ".gitignore",
        ".github/workflows/ci.yml",
        ".gitattributes",
        "docs/über.md",
        "a b/c d",
        "...x",
        "x.git",
        "git~",
        "gitx~1",
        "bracken/notes.md",
        "a.bracken",
        // Rule 8's blank set is White_Space only, so these are not blank.
        "a\u{1c}",
        "\u{1c}",
    ] {
        assert_eq!(validate_path(path), Ok(()), "{path:?}");
    }
}

#[test]
fn malformed_paths_are_refused() {
    let cases: &[(&str, PathError)] = &[
        ("", PathError::Empty),
        ("/a", PathError::Absolute),
        ("a/", PathError::TrailingSlash),
        ("a\0b", PathError::Nul),
        ("a//b", PathError::BlankComponent),
        ("a\\b", PathError::Backslash),
    ];
    for (path, error) in cases {
        assert_eq!(validate_path(path).as_ref(), Err(error), "{path:?}");
    }
}

#[test]
fn blank_components_are_refused() {
    // Empty, White_Space only (rule 8's characters), and components that fold to
    // nothing because they hold only default-ignorable code points.
    for path in [
        " /a",
        "a/ ",
        "a/\t/b",
        "a/\u{3000}",
        "\u{a0}",
        "a/\u{2028}/b",
        "a/\u{200b}",
        "\u{feff}/a",
        "a/\u{200d}\u{200c}",
    ] {
        assert_eq!(validate_path(path), Err(PathError::BlankComponent), "{path:?}");
    }
}

#[test]
fn dot_components_are_refused() {
    for path in [".", "..", "./a", "a/./b", "a/../b", "a/..", "...", ". .", ".. ", "a/.\u{200b}."] {
        assert!(matches!(validate_path(path), Err(PathError::DotComponent(_))), "{path:?}");
    }
}

#[test]
fn git_and_bracken_names_are_refused_after_folding() {
    for path in [
        ".git",
        ".git/config",
        "a/.git/HEAD",
        ".GIT",
        ".Git/x",
        // protectHFS: ignorable code points inside the name.
        ".g\u{200c}it",
        "\u{feff}.git",
        ".gi\u{200d}t/x",
        // protectNTFS: trailing dots and spaces, and 8.3 short names.
        ".git.",
        ".git ",
        ".git. .",
        "git~1",
        "GIT~2/x",
        "a/Git~10",
        // Any .bracken name, at any depth, in any case.
        ".bracken",
        ".bracken/objects/x",
        ".bracken-team",
        ".bracken-bin",
        "x/.Bracken/y",
        ".BRACKEN",
        ".brac\u{200b}ken",
        "BRACKE~1",
        "bracke~2/x",
    ] {
        assert!(matches!(validate_path(path), Err(PathError::Reserved(_))), "{path:?}");
    }
}

#[test]
fn folding_is_the_canonical_caseless_match_without_ignorables() {
    // NFC and NFD spellings of one name.
    assert_eq!(fold("caf\u{e9}"), fold("cafe\u{301}"));
    // Full case folding, not just lowercasing.
    assert_eq!(fold("README.md"), fold("readme.MD"));
    assert_eq!(fold("stra\u{df}e"), fold("STRASSE"));
    assert_eq!(fold("\u{212a}elvin"), fold("kelvin"));
    assert_eq!(fold("\u{3c3}"), fold("\u{3c2}"));
    // Default-ignorable code points are dropped.
    assert_eq!(fold("a\u{200d}b"), "ab");
    assert_eq!(fold("a\u{feff}b\u{00ad}"), "ab");
    // Different names stay different.
    assert_ne!(fold("a"), fold("b"));
    assert_ne!(fold("e"), fold("\u{e9}"));
}

#[test]
fn equal_paths_after_folding_collide() {
    let cases = [
        ["caf\u{e9}.txt", "cafe\u{301}.txt"],
        ["README", "readme"],
        ["src/Main.rs", "src/main.rs"],
        ["Docs/a", "docs/A"],
        ["stra\u{df}e", "STRASSE"],
        ["a\u{200d}b", "ab"],
    ];
    for [a, b] in cases {
        assert_eq!(find_collision([a, b]), Some(Collision::Equal(a.to_owned(), b.to_owned())), "{a:?} {b:?}");
    }
}

#[test]
fn a_file_that_is_a_directory_of_another_path_collides() {
    assert_eq!(find_collision(["a", "a/b"]), Some(Collision::Prefix("a".to_owned(), "a/b".to_owned())));
    assert_eq!(find_collision(["a/b", "a"]), Some(Collision::Prefix("a".to_owned(), "a/b".to_owned())));
    assert_eq!(find_collision(["H/x", "h"]), Some(Collision::Prefix("h".to_owned(), "H/x".to_owned())));
    assert_eq!(find_collision(["h", "H/x"]), Some(Collision::Prefix("h".to_owned(), "H/x".to_owned())));
    assert_eq!(
        find_collision(["caf\u{e9}", "cafe\u{301}/x/y"]),
        Some(Collision::Prefix("caf\u{e9}".to_owned(), "cafe\u{301}/x/y".to_owned()))
    );
}

#[test]
fn paths_that_only_share_a_directory_do_not_collide() {
    assert_eq!(find_collision(["H/x", "h/y"]), None);
    assert_eq!(find_collision(["a", "ab", "a.b", "a-b/c"]), None);
    assert_eq!(find_collision(["a/b", "a/c", "a/d/e"]), None);
    assert_eq!(find_collision(["e", "\u{e9}"]), None);
}

#[test]
fn symlink_targets_point_only_down() {
    for target in ["target", "dir/file", "a b/c", "docs/über.md"] {
        assert_eq!(validate_symlink_target(target.as_bytes()), Ok(()), "{target:?}");
    }
    let refused: &[(&[u8], PathError)] = &[
        (b"", PathError::Empty),
        (b"/etc/passwd", PathError::Absolute),
        (b"dir/", PathError::TrailingSlash),
        (b"a\0b", PathError::Nul),
        (b"\xff\xfe", PathError::NotUtf8),
        (b"a//b", PathError::BlankComponent),
    ];
    for (target, error) in refused {
        assert_eq!(validate_symlink_target(target).as_ref(), Err(error), "{target:?}");
    }
    for target in ["..", "../x", "a/../../b", "./x", "a/."] {
        assert!(matches!(validate_symlink_target(target.as_bytes()), Err(PathError::DotComponent(_))), "{target:?}");
    }
    for target in [".git/config", "x/.GIT", ".bracken/identities/x", "git~1"] {
        assert!(matches!(validate_symlink_target(target.as_bytes()), Err(PathError::Reserved(_))), "{target:?}");
    }
}

// Findings 6f00787d66b9 and 044cd093a6bf: Windows reads a name before ':' as a
// drive, whatever the name (git's CVE-2019-1351), and one after it as a stream.
// So ':' is refused in every component, of paths and targets alike.
#[test]
fn colons_are_refused_in_paths_and_targets() {
    for path in [
        "C:/Windows/System32",
        "C:Windows",
        "c:",
        "z:/a",
        "C:\u{200b}x",
        "1:/x",
        "\u{e9}:/x",
        "\u{3b1}:x",
        "CC:/x",
        "a/b:c",
        "a/C:x",
        ".git:x",
        ".git::$INDEX_ALLOCATION",
        ".bracken:x",
        "notes:2026",
        ":",
    ] {
        assert_eq!(validate_path(path), Err(PathError::Colon), "{path:?}");
        assert_eq!(validate_symlink_target(path.as_bytes()), Err(PathError::Colon), "{path:?}");
    }
}

// Finding d65749f2b45d: a path's length is bounded before anything is folded.
#[test]
fn paths_and_components_are_bounded() {
    use bracken_tree::path::{MAX_COMPONENT_BYTES, MAX_PATH_BYTES};
    let component = "c".repeat(MAX_COMPONENT_BYTES);
    assert_eq!(validate_path(&component), Ok(()));
    let long_component = "c".repeat(MAX_COMPONENT_BYTES + 1);
    assert_eq!(validate_path(&long_component), Err(PathError::ComponentTooLong(MAX_COMPONENT_BYTES + 1)));
    assert_eq!(
        validate_symlink_target(format!("a/{long_component}").as_bytes()),
        Err(PathError::ComponentTooLong(MAX_COMPONENT_BYTES + 1))
    );

    let longest = format!("{}b", "a/".repeat((MAX_PATH_BYTES - 1) / 2));
    assert_eq!(longest.len(), MAX_PATH_BYTES - 1);
    assert_eq!(validate_path(&longest), Ok(()));
    assert_eq!(validate_path(&format!("{longest}b")), Ok(()));
    assert_eq!(validate_path(&format!("{longest}bb")), Err(PathError::TooLong(MAX_PATH_BYTES + 1)));

    // The finding's counterexample: 40,000 components, refused for its length.
    let deep = vec!["a"; 40_000].join("/");
    assert_eq!(validate_path(&deep), Err(PathError::TooLong(deep.len())));
}

// Finding d65749f2b45d: collisions among many deep paths are found without
// copying every directory prefix.
#[test]
fn collisions_among_deep_paths_are_found() {
    let dir = vec!["d"; 2_000].join("/");
    let mut paths: Vec<String> = (0..2_000).map(|i| format!("{dir}/{i}")).collect();
    assert_eq!(find_collision(paths.iter().map(String::as_str)), None);
    // A file that is a directory of every other path, in another case.
    let file = vec!["D"; 1_000].join("/");
    paths.push(file.clone());
    match find_collision(paths.iter().map(String::as_str)) {
        Some(Collision::Prefix(found, under)) => {
            assert_eq!(found, file);
            assert!(under.starts_with(&dir));
        }
        other => panic!("expected a prefix collision, got {other:?}"),
    }
}

// Finding cc6570efb144: folding normalizes again after case folding.
#[test]
fn canonically_equivalent_folds_collide() {
    // U+01F0 (j with caron) folds to j + U+030C; with U+0323 that must reorder.
    let a = "x\u{1f0}\u{323}";
    let b = "xJ\u{323}\u{30c}";
    assert_eq!(fold(a), fold(b));
    assert_eq!(find_collision([a, b]), Some(Collision::Equal(a.to_owned(), b.to_owned())));
    let under = format!("{b}/f");
    assert_eq!(find_collision([a, under.as_str()]), Some(Collision::Prefix(a.to_owned(), under.clone())));
    // Composed and decomposed spellings, in either case.
    assert_eq!(fold("\u{1e0c}\u{307}"), fold("d\u{323}\u{307}"));
    assert_eq!(fold("\u{1e0a}\u{323}"), fold("D\u{307}\u{323}"));
}

// Finding cc6570efb144: a mark after `.bracken` doesn't hide the prefix.
#[test]
fn a_bracken_prefix_with_a_combining_mark_is_refused() {
    for path in [".bracken\u{300}", ".bracken\u{301}-bin", "x/.bracken\u{327}team", ".BRACKEN\u{301}", ".brackeñ"] {
        assert!(matches!(validate_path(path), Err(PathError::Reserved(_))), "{path:?}");
    }
}
