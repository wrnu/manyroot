use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    std::fs::write(path, text).expect("a file");
}

/// A valid manifest: two items at the start, and a change at half the run revising `add`.
fn manifest() -> Value {
    json!({
        "format": 1,
        "name": "tiny",
        "python": "3.12",
        "package": "tiny",
        "spec": "spec.md",
        "seed": "seed",
        "items": [
            {"id": "add", "kind": "feature", "title": "Add", "text": "items/add.md", "release": {"at": "start"}},
            {"id": "fix-sign", "kind": "fix", "title": "Sign", "text": "items/fix-sign.md", "release": {"at": "start"}},
            {"id": "wider", "kind": "change", "title": "Wider", "text": "items/wider.md",
             "release": {"after_share": 0.5}, "revises": ["add"]},
        ],
        "hidden": {"id": "tiny@1", "digest": "0".repeat(64)},
    })
}

/// A project in `dir` with `manifest`, its spec, items and a seed with one public test.
fn project_in(dir: &Path, manifest: &Value) -> PathBuf {
    let p = dir.join("tiny");
    write(&p.join("project.json"), &manifest.to_string());
    write(&p.join("spec.md"), "# tiny\n");
    for id in ["add", "fix-sign", "wider"] {
        write(&p.join(format!("items/{id}.md")), "an issue\n");
    }
    write(&p.join("seed/tiny/__init__.py"), "");
    write(&p.join("seed/tests/test_tiny.py"), "import unittest\n");
    p
}

/// A hidden manifest for `tiny` with `groups`.
fn hidden(groups: Value) -> Value {
    json!({"format": 1, "project": "tiny", "groups": groups})
}

/// A store in `dir` holding `tiny@1` with the manifest `hidden`, two test files and a golden package, and the
/// set's digest.
fn store_in(dir: &Path, hidden: &Value) -> (PathBuf, String) {
    let store = dir.join("store");
    let set = store.join("tiny@1");
    write(&set.join("hidden.json"), &hidden.to_string());
    for file in ["tests/test_a.py", "tests/test_b.py"] {
        write(&set.join(file), "import unittest\n");
    }
    write(&set.join("tests/data.txt"), "data\n");
    write(&set.join("golden/tiny/__init__.py"), "");
    write(&set.join("golden-before/tiny/__init__.py"), "");
    let d = digest(&set).expect("a digest");
    (store, d)
}

/// `manifest()` with the hidden set's digest `d`.
fn manifest_for(d: &str) -> Value {
    let mut m = manifest();
    m["hidden"]["digest"] = json!(d);
    m
}

/// A named change to a hidden set on disk.
type Edit = (&'static str, fn(&Path));

fn groups() -> Value {
    json!([
        {"item": "add", "file": "tests/test_a.py", "applies": "before-change"},
        {"item": "add", "file": "tests/test_b.py", "applies": "after-change"},
        {"item": "fix-sign", "file": "tests/test_a.py", "applies": "always"},
    ])
}

#[test]
fn a_valid_project_loads_with_its_items_and_releases() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let p = Project::load(&project_in(tmp.path(), &manifest())).expect("loads");
    assert_eq!((p.name.as_str(), p.python.as_str(), p.package.as_str()), ("tiny", "3.12", "tiny"));
    assert_eq!(p.limits, Limits::default());
    let ids = |share| p.released(share).map(|i| i.id.as_str()).collect::<Vec<_>>();
    assert_eq!(ids(0.0), ["add", "fix-sign"]);
    assert_eq!(ids(0.49), ["add", "fix-sign"]);
    assert_eq!(ids(0.5), ["add", "fix-sign", "wider"]);
    assert_eq!(p.revised_by("add").map(|c| c.id.as_str()), Some("wider"));
    assert_eq!(p.revised_by("fix-sign"), None);
    assert_eq!(p.public_test_files().expect("tests"), ["tests/test_tiny.py"]);
    assert_eq!(p.seed_files().expect("seed"), ["tests/test_tiny.py", "tiny/__init__.py"]);
}

/// Each mistake in a manifest is refused, saying what is wrong.
#[test]
fn every_mistake_in_a_manifest_is_refused() {
    // A field by its path of keys (an array's by its index as a key), made if missing.
    fn at<'a>(m: &'a mut Value, path: &str) -> &'a mut Value {
        path.split('/').fold(m, |v, key| match key.parse::<usize>() {
            Ok(i) => &mut v[i],
            Err(_) => &mut v[key],
        })
    }
    let cases = [
        ("format 2", "format", json!(2)),
        ("isn't lowercase letters", "name", json!("Tiny")),
        ("isn't a version", "python", json!("2.7")),
        ("isn't a lowercase Python identifier", "package", json!("Tiny")),
        ("isn't a plain relative path", "spec", json!("../spec.md")),
        ("isn't a plain relative path", "spec", json!("/etc/hosts")),
        ("isn't a plain relative path", "seed", json!("./seed")),
        ("no package directory", "package", json!("other")),
        ("no file items/none.md", "items/0/text", json!("items/none.md")),
        ("used twice", "items/1/id", json!("add")),
        ("has no title", "items/1/title", json!(" ")),
        ("isn't a change", "items/1/revises", json!(["add"])),
        ("released at the start", "items/2/release", json!({"at": "start"})),
        ("which isn't an item", "items/2/revises", json!(["none"])),
        ("which is a change", "items/2/revises", json!(["wider"])),
        ("0 < x < 1", "items/2/release", json!({"after_share": 1.0})),
        ("0 < x < 1", "items/2/release", json!({"after_share": 0.0})),
        ("0 < x < 1", "items/2/release", json!({"at": "start", "after_share": 0.5})),
        ("0 < x < 1", "items/2/release", json!({"at": "later"})),
        ("unknown field", "items/0/owner", json!("x")),
        ("unknown field", "extra", json!(1)),
        ("no items", "items", json!([])),
        ("limits must", "limits", json!({"cpu_seconds": 0})),
        ("limits must", "limits", json!({"memory_mb": 100_000})),
        ("limits must", "limits", json!({"output_bytes": 0})),
        ("limits must", "limits", json!({"wall_seconds": 3601})),
        ("limits must", "limits", json!({"scratch_mb": 0})),
        ("unknown field", "limits", json!({"cpu": 1})),
        ("isn't a name", "hidden/id", json!("..")),
        ("isn't a name", "hidden/id", json!("a/b")),
        ("64 lowercase hex", "hidden/digest", json!("abc")),
        ("isn't a directory in the seed", "public_tests", json!("items")),
    ];
    for (says, path, value) in cases {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let mut m = manifest();
        *at(&mut m, path) = value;
        let err = Project::load(&project_in(tmp.path(), &m)).expect_err(says);
        assert!(err.contains(says), "{path}: expected {says:?}: {err}");
    }
}

#[test]
fn a_project_with_no_public_test_or_two_changes_on_one_item_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let p = project_in(tmp.path(), &manifest());
    std::fs::rename(p.join("seed/tests/test_tiny.py"), p.join("seed/tests/helper.py")).expect("renamed");
    assert!(Project::load(&p).expect_err("no test").contains("no test_*.py"));

    let tmp = tempfile::tempdir().expect("a temp dir");
    let mut m = manifest();
    m["items"][1] = json!({"id": "again", "kind": "change", "title": "Again", "text": "items/fix-sign.md",
                           "release": {"after_share": 0.7}, "revises": ["add"]});
    let err = Project::load(&project_in(tmp.path(), &m)).expect_err("two changes");
    assert!(err.contains("revised by both"), "{err}");
}

#[test]
fn a_symlink_or_too_many_files_anywhere_in_a_project_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let p = project_in(tmp.path(), &manifest());
    std::os::unix::fs::symlink("/etc/hosts", p.join("seed/tiny/hosts")).expect("a symlink");
    assert!(Project::load(&p).expect_err("a symlink").contains("symlink"));

    let tmp = tempfile::tempdir().expect("a temp dir");
    let p = project_in(tmp.path(), &manifest());
    for i in 0..MAX_FILES {
        std::fs::write(p.join(format!("seed/tiny/m{i}.py")), "").expect("a file");
    }
    assert!(Project::load(&p).expect_err("too many").contains("more than a project needs"));
}

#[test]
fn a_hidden_set_opens_only_with_the_projects_digest_and_says_only_counts() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (store, d) = store_in(tmp.path(), &hidden(groups()));
    let p = Project::load(&project_in(tmp.path(), &manifest_for(&d))).expect("loads");
    let set = HiddenSet::open(&p, &store).expect("opens");
    assert_eq!(
        set.summary(&p),
        StoreSummary {
            groups: 3,
            test_files: 2,
            golden_files: 1,
            golden_before_files: 1,
            items_without_tests: vec!["wider".into()]
        }
    );
    assert_eq!(set.file("tests/test_a.py"), Some(&b"import unittest\n"[..]), "the bytes it read and digested");
    let applying = |share| set.groups.iter().filter(|g| set.applies(&p, g, share)).map(|g| &g.file).collect::<Vec<_>>();
    assert_eq!(
        applying(0.2),
        ["tests/test_a.py", "tests/test_a.py"],
        "before the change: add's old tests and fix-sign's"
    );
    assert_eq!(applying(0.5), ["tests/test_b.py", "tests/test_a.py"], "after it: add's revised tests and fix-sign's");

    std::fs::write(store.join("tiny@1/tests/test_b.py"), "import unittest  # changed\n").expect("changed");
    let err = HiddenSet::open(&p, &store).expect_err("another digest");
    assert!(err.contains("not the project's"), "{err}");
}

/// Finding 0bc2906f1868: the digest changes with every file's name and content and with every directory, an
/// empty one too.
#[test]
fn the_digest_covers_every_name_content_and_directory() {
    let edits: [Edit; 6] = [
        ("a renamed file", |s| std::fs::rename(s.join("tests/test_b.py"), s.join("tests/test_c.py")).expect("renamed")),
        ("an added empty file", |s| write(&s.join("tests/empty.py"), "")),
        ("an added golden file", |s| write(&s.join("golden/tiny/more.py"), "x = 1\n")),
        ("a changed manifest", |s| write(&s.join("hidden.json"), &hidden(json!([])).to_string())),
        ("an added empty directory", |s| std::fs::create_dir(s.join("golden/tiny/sub")).expect("a directory")),
        ("a removed empty file", |s| std::fs::remove_file(s.join("golden/tiny/__init__.py")).expect("removed")),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (what, edit) in edits {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let (store, before) = store_in(tmp.path(), &hidden(groups()));
        seen.insert(before.clone());
        edit(&store.join("tiny@1"));
        let after = digest(&store.join("tiny@1")).expect("a digest");
        assert_ne!(after, before, "{what}");
        assert!(seen.insert(after), "{what} gave another edit's digest");
    }
    assert_eq!(seen.len(), 7, "every store before an edit has the same digest, and every edit its own");
}

/// Each mistake in a hidden set is refused, and no error names anything in the set (finding 76e88fa2ae94).
#[test]
fn every_mistake_in_a_hidden_set_is_refused_naming_nothing_in_it() {
    let group = |item: &str, file: &str, applies: &str| json!([{"item": item, "file": file, "applies": applies}]);
    let cases = [
        ("names no item", hidden(group("secret-item", "tests/test_a.py", "always"))),
        ("under tests/", hidden(group("add", "hidden.json", "always"))),
        ("under tests/", hidden(group("add", "tests/test_secret_missing.py", "always"))),
        ("under tests/", hidden(group("add", "tests/data.txt", "always"))),
        ("under tests/", hidden(group("add", "tests/../tests/test_a.py", "always"))),
        ("no change revises", hidden(group("fix-sign", "tests/test_a.py", "after-change"))),
        ("isn't a valid manifest", hidden(group("add", "tests/test_a.py", "secret-sometimes"))),
        ("isn't a valid manifest", json!({"format": 1, "project": "tiny", "groups": [], "secret_field": 1})),
        ("format 2", json!({"format": 2, "project": "tiny", "groups": []})),
        ("another project", json!({"format": 1, "project": "secret-other", "groups": []})),
    ];
    for (says, manifest) in cases {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let (store, d) = store_in(tmp.path(), &manifest);
        let p = Project::load(&project_in(tmp.path(), &manifest_for(&d))).expect("loads");
        let err = HiddenSet::open(&p, &store).expect_err(says);
        assert!(err.contains(says), "expected {says:?}: {err}");
        assert!(!err.contains("secret") && !err.contains("test_a") && !err.contains("data.txt"), "{err}");
    }
}

/// A symlink, a special file or a repository inside the set is refused, naming nothing in it.
#[test]
fn a_symlink_or_repository_inside_a_hidden_set_is_refused_naming_nothing_in_it() {
    let edits: [Edit; 5] = [
        ("a symlink", |s| {
            std::os::unix::fs::symlink("test_a.py", s.join("tests/test_secret_link.py")).expect("a link")
        }),
        ("a .git", |s| write(&s.join("golden/.git/HEAD"), "ref: refs/heads/secret\n")),
        // Finding 043504ed3df3: git on macOS finds a repository in .GIT too.
        ("a .git", |s| write(&s.join("golden/secret/.GIT/HEAD"), "ref: refs/heads/secret\n")),
        // Finding 81b5698a985c: a hard link puts a hidden file wherever its other link is, the project included.
        ("a hard link", |s| {
            let elsewhere = s.parent().and_then(Path::parent).expect("the store's parent").join("secret_linked.py");
            std::fs::hard_link(s.join("tests/test_a.py"), elsewhere).expect("a hard link");
        }),
        ("a symlink", |s| {
            let fifo = std::process::Command::new("mkfifo").arg(s.join("tests/secret_fifo")).status();
            assert!(fifo.expect("mkfifo").success());
        }),
    ];
    for (says, edit) in edits {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let (store, _) = store_in(tmp.path(), &hidden(groups()));
        edit(&store.join("tiny@1"));
        let err = digest(&store.join("tiny@1")).expect_err(says);
        assert!(err.contains(says) && !err.contains("secret"), "{err}");
        let p = Project::load(&project_in(tmp.path(), &manifest())).expect("loads");
        let err = HiddenSet::open(&p, &store).expect_err(says);
        assert!(err.contains(says) && !err.contains("secret"), "{err}");
    }
}

/// Finding 567a72cbfa5e: the set itself is checked, not only the store: a set that is a symlink, wherever it
/// points, or a repository of its own, or a store in the project or a git checkout, is refused.
#[test]
fn the_set_must_be_a_directory_outside_the_project_and_every_repository() {
    let git_init = |dir: &Path| {
        let init = std::process::Command::new("git").arg("init").arg("-q").arg(dir).status();
        assert!(init.expect("git").success());
    };
    // (A) A set that is a symlink into a git checkout.
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (elsewhere, d) = store_in(&tmp.path().join("repo"), &hidden(groups()));
    git_init(&tmp.path().join("repo"));
    let store = tmp.path().join("store");
    std::fs::create_dir(&store).expect("a store");
    std::os::unix::fs::symlink(elsewhere.join("tiny@1"), store.join("tiny@1")).expect("a link");
    let p = Project::load(&project_in(&tmp.path().join("p"), &manifest_for(&d))).expect("loads");
    assert!(HiddenSet::open(&p, &store).expect_err("a link").contains("is a symlink"));

    // (B) A set that is a symlink into the project, where agents read.
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (inside, d) = store_in(&tmp.path().join("tiny/notes"), &hidden(groups()));
    let p = Project::load(&project_in(tmp.path(), &manifest_for(&d))).expect("loads");
    let store = tmp.path().join("store");
    std::fs::create_dir(&store).expect("a store");
    std::os::unix::fs::symlink(inside.join("tiny@1"), store.join("tiny@1")).expect("a link");
    assert!(HiddenSet::open(&p, &store).expect_err("a link").contains("is a symlink"));

    // (C) A set that is a repository of its own.
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (store, _) = store_in(tmp.path(), &hidden(groups()));
    git_init(&store.join("tiny@1"));
    let p = Project::load(&project_in(&tmp.path().join("p"), &manifest())).expect("loads");
    let err = HiddenSet::open(&p, &store).expect_err("a repository");
    assert!(err.contains("git checkout") || err.contains(".git"), "{err}");

    // A store inside the project, and a store in a git checkout.
    let tmp = tempfile::tempdir().expect("a temp dir");
    let p = project_in(tmp.path(), &manifest());
    let (store, _) = store_in(&p, &hidden(groups()));
    let project = Project::load(&p).expect("loads");
    assert!(HiddenSet::open(&project, &store).expect_err("inside").contains("overlap"));

    let tmp = tempfile::tempdir().expect("a temp dir");
    let (store, d) = store_in(&tmp.path().join("repo"), &hidden(groups()));
    git_init(&tmp.path().join("repo"));
    let project = Project::load(&project_in(&tmp.path().join("elsewhere"), &manifest_for(&d))).expect("loads");
    assert!(HiddenSet::open(&project, &store).expect_err("in a checkout").contains("git checkout"));
}

#[test]
fn a_set_without_its_golden_package_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (store, _) = store_in(tmp.path(), &hidden(groups()));
    std::fs::remove_dir_all(store.join("tiny@1/golden/tiny")).expect("removed");
    let d = digest(&store.join("tiny@1")).expect("a digest");
    let project = Project::load(&project_in(tmp.path(), &manifest_for(&d))).expect("loads");
    assert!(HiddenSet::open(&project, &store).expect_err("no golden").contains("golden"));
}

/// Finding 6951053f24c1: a hidden set's Debug says counts and the digest, never a file's name or bytes, so a
/// failed expect_err or assertion can't print the set.
#[test]
fn a_hidden_sets_debug_holds_no_file_name_or_content() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (store, _) = store_in(tmp.path(), &hidden(groups()));
    write(&store.join("tiny@1/tests/test_secret_name.py"), "SECRET-CONTENT = 1\n");
    let d = digest(&store.join("tiny@1")).expect("a digest");
    let p = Project::load(&project_in(tmp.path(), &manifest_for(&d))).expect("loads");
    let set = HiddenSet::open(&p, &store).expect("opens");
    let shown = format!("{set:?} {set:#?}");
    assert!(shown.contains("groups: 3") && shown.contains(&d), "{shown}");
    for planted in ["secret", "SECRET", "test_a", "import unittest", "golden", "tiny@1/"] {
        assert!(!shown.contains(planted), "{planted:?} in {shown}");
    }
}
