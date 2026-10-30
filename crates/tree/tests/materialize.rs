//! Safe materialization (docs/design/weave.md, *Writing a tree out*, build step 2).

#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use bracken_tree::import::{Source, plan};
use bracken_tree::materialize::{MaterializeError, materialize};
use bracken_tree::secrets::KnownKeys;
use bracken_tree::{ContentHash, Entry, FormatError, Manifest, Mode};
use common::Repo;

/// A manifest of (path, mode, bytes), and its files by hash.
fn tree(files: &[(&str, Mode, &[u8])]) -> (Manifest, BTreeMap<ContentHash, Vec<u8>>) {
    let entries = files
        .iter()
        .map(|(path, mode, bytes)| Entry {
            path: (*path).to_owned(),
            mode: *mode,
            hash: ContentHash::of(bytes),
            size: bytes.len() as u64,
        })
        .collect();
    let store = files.iter().map(|(_, _, bytes)| (ContentHash::of(bytes), bytes.to_vec())).collect();
    (Manifest::new(entries), store)
}

fn write(manifest: &Manifest, store: &BTreeMap<ContentHash, Vec<u8>>, target: &Path) -> Result<(), MaterializeError> {
    materialize(manifest, |h| store.get(h).cloned(), target)
}

fn is_empty(dir: &Path) -> bool {
    fs::read_dir(dir).expect("read the target").next().is_none()
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("stat").permissions().mode() & 0o7777
}

#[test]
fn an_imported_tree_round_trips() {
    let repo = Repo::new();
    repo.write("README.md", b"hello\n");
    repo.write("src/deep/er/lib.rs", b"fn f() {}\n");
    repo.executable("scripts/run", b"#!/bin/sh\necho hi\n");
    repo.symlink("latest", "src/deep");
    repo.symlink("src/link", "deep/er/lib.rs");
    repo.write("empty", b"");
    repo.commit_all();
    let plan = plan(repo.path(), &Source::Rev("HEAD".to_owned()), &KnownKeys::new()).expect("plan HEAD");
    let manifest = plan.manifest().expect("nothing refused");
    let store: BTreeMap<_, _> = plan.files.iter().map(|f| (f.hash, f.bytes.clone())).collect();

    let target = tempfile::tempdir().expect("a target");
    write(&manifest, &store, target.path()).expect("materialize");
    for entry in &manifest.entries {
        let on_disk = target.path().join(&entry.path);
        match entry.mode {
            Mode::Symlink => {
                let link = fs::read_link(&on_disk).expect("a symlink");
                assert_eq!(link.to_str().map(str::as_bytes), Some(&store[&entry.hash][..]));
            }
            mode => {
                assert_eq!(fs::read(&on_disk).expect("read"), store[&entry.hash], "{}", entry.path);
                assert_eq!(mode_of(&on_disk), if mode == Mode::Executable { 0o755 } else { 0o644 }, "{}", entry.path);
            }
        }
    }
    assert_eq!(mode_of(&target.path().join("src/deep")), 0o755);
}

#[test]
fn counterexample_1_of_d5112c023747_is_refused_and_nothing_written() {
    // A symlink and a directory that a case-insensitive file system treats as one.
    for (link, target, file) in
        [("h", ".git/hooks", "H/post-checkout"), ("b", ".bracken/objects", "B/ab/cd"), ("h", "sub", "H/x")]
    {
        let (manifest, store) =
            tree(&[(link, Mode::Symlink, target.as_bytes()), (file, Mode::Executable, b"#!/bin/sh\n")]);
        let dir = tempfile::tempdir().expect("a target");
        let result = write(&manifest, &store, dir.path());
        assert!(matches!(result, Err(MaterializeError::Invalid(FormatError::Collision(_)))), "{result:?}");
        assert!(is_empty(dir.path()));
    }
    // And in NFC and NFD spellings of one name.
    let (manifest, store) = tree(&[("caf\u{e9}", Mode::Symlink, b"sub"), ("cafe\u{301}/f", Mode::File, b"x")]);
    let dir = tempfile::tempdir().expect("a target");
    assert!(matches!(write(&manifest, &store, dir.path()), Err(MaterializeError::Invalid(FormatError::Collision(_)))));
    assert!(is_empty(dir.path()));
}

#[test]
fn counterexample_2_of_d5112c023747_is_refused_and_nothing_written() {
    let (manifest, store) =
        tree(&[("s", Mode::Symlink, b"."), ("x/f", Mode::File, b"x"), ("t", Mode::Symlink, b"s/x/../..")]);
    let dir = tempfile::tempdir().expect("a target");
    let result = write(&manifest, &store, dir.path());
    assert!(matches!(result, Err(MaterializeError::Target { .. })), "{result:?}");
    assert!(is_empty(dir.path()));
    for bad in ["..", "../x", "/etc", ".git/hooks", ".bracken-bin/bracken", "C:x", "a/"] {
        let (manifest, store) = tree(&[("link", Mode::Symlink, bad.as_bytes())]);
        let dir = tempfile::tempdir().expect("a target");
        assert!(matches!(write(&manifest, &store, dir.path()), Err(MaterializeError::Target { .. })), "{bad:?}");
        assert!(is_empty(dir.path()));
    }
}

#[test]
fn symlinks_are_written_last() {
    // `a` sorts before `b/f`; writing it first would still be safe, but symlinks
    // come after every file and directory.
    let (manifest, store) = tree(&[("a", Mode::Symlink, b"b"), ("b/f", Mode::File, b"x"), ("c", Mode::File, b"y")]);
    let dir = tempfile::tempdir().expect("a target");
    write(&manifest, &store, dir.path()).expect("materialize");
    assert_eq!(fs::read_link(dir.path().join("a")).expect("a symlink"), Path::new("b"));
    assert!(fs::symlink_metadata(dir.path().join("b")).expect("stat").is_dir());
}

#[test]
fn the_target_must_be_an_empty_directory_reached_without_a_symlink() {
    let (manifest, store) = tree(&[("f", Mode::File, b"x")]);
    let dir = tempfile::tempdir().expect("a target");
    fs::write(dir.path().join("existing"), b"x").expect("write");
    assert!(matches!(write(&manifest, &store, dir.path()), Err(MaterializeError::NotEmpty(_))));

    let outer = tempfile::tempdir().expect("a directory");
    let real = outer.path().join("real");
    fs::create_dir(&real).expect("mkdir");
    let link = outer.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert!(matches!(write(&manifest, &store, &link), Err(MaterializeError::Io { .. })));
    assert!(is_empty(&real));
}

#[test]
fn missing_or_wrong_files_are_refused_before_anything_is_written() {
    let (manifest, mut store) = tree(&[("a", Mode::File, b"a"), ("b", Mode::File, b"b")]);
    let dir = tempfile::tempdir().expect("a target");
    store.remove(&ContentHash::of(b"b"));
    assert!(matches!(write(&manifest, &store, dir.path()), Err(MaterializeError::Missing { path, .. }) if path == "b"));
    assert!(is_empty(dir.path()));
    store.insert(ContentHash::of(b"b"), b"not b".to_vec());
    assert!(
        matches!(write(&manifest, &store, dir.path()), Err(MaterializeError::Mismatch { path, .. }) if path == "b")
    );
    assert!(is_empty(dir.path()));
}

/// Materializes a small tree into `WEAVE_TEST_TARGET`. Run only by
/// `modes_are_exact_whatever_the_umask`, in a child process with its own umask.
#[test]
#[ignore = "run by modes_are_exact_whatever_the_umask"]
fn umask_child() {
    let Some(target) = std::env::var_os("WEAVE_TEST_TARGET") else { return };
    let (manifest, store) =
        tree(&[("d/f", Mode::File, b"f"), ("d/x", Mode::Executable, b"x"), ("g", Mode::File, b"g")]);
    write(&manifest, &store, Path::new(&target)).expect("materialize");
}

#[test]
fn modes_are_exact_whatever_the_umask() {
    let exe = std::env::current_exe().expect("the test binary");
    for umask in ["077", "000", "027"] {
        let dir = tempfile::tempdir().expect("a target");
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("umask {umask} && exec \"$0\" --ignored --exact umask_child --test-threads=1 -q"))
            .arg(&exe)
            .env("WEAVE_TEST_TARGET", dir.path())
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run the child");
        assert!(status.success(), "the child failed under umask {umask}");
        assert_eq!(mode_of(&dir.path().join("d/f")), 0o644, "umask {umask}");
        assert_eq!(mode_of(&dir.path().join("d/x")), 0o755, "umask {umask}");
        assert_eq!(mode_of(&dir.path().join("g")), 0o644, "umask {umask}");
        assert_eq!(mode_of(&dir.path().join("d")), 0o755, "umask {umask}");
    }
}
