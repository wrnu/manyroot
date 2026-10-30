//! Import from git (docs/design/weave.md, *What import takes*, build step 2).

#![cfg(unix)]

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use bracken_tree::import::{Exclusion, ImportError, MAX_FILE_BYTES, Plan, Refusal, Source, plan};
use bracken_tree::secrets::{KnownKeys, SecretRule};
use bracken_tree::{ContentHash, Mode, PathError};
use common::Repo;

fn head() -> Source {
    Source::Rev("HEAD".to_owned())
}

fn paths(plan: &Plan) -> Vec<&str> {
    plan.entries.iter().map(|e| e.path.as_str()).collect()
}

fn refused_paths(plan: &Plan) -> BTreeSet<String> {
    plan.refused
        .iter()
        .map(|r| match r {
            Refusal::Secret { path, .. }
            | Refusal::Path { path, .. }
            | Refusal::Target { path, .. }
            | Refusal::TooLarge { path, .. }
            | Refusal::Mode { path, .. } => path.clone(),
            other => format!("{other}"),
        })
        .collect()
}

#[test]
fn this_repository_imports_cleanly() {
    let this_repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let plan = plan(this_repo, &head(), &KnownKeys::new()).expect("git can list HEAD");
    assert_eq!(plan.refused, [], "this repository must import cleanly");
    let manifest = plan.manifest().expect("nothing refused");
    assert!(manifest.get("Cargo.toml").is_some());
    assert!(manifest.get("crates/tree/src/lib.rs").is_some());
    // The coordination Ground is tracked, and excluded.
    assert!(plan.excluded.iter().any(|(p, why)| p.starts_with(".bracken/") && *why == Exclusion::BrackenRoot));
    assert!(manifest.entries.iter().all(|e| !e.path.starts_with(".bracken")));
}

#[test]
fn untracked_ignored_and_unstaged_content_is_never_seen() {
    let repo = Repo::new();
    repo.write(".gitignore", b"*.log\nsecrets/\n");
    repo.write("a.txt", b"committed\n");
    repo.commit_all();
    // Untracked and ignored files, which would be refused if import saw them.
    // Built at runtime, so this file holds no header.
    let pem = format!("-----BEGIN {}PRIVATE KEY-----\n", "OPENSSH ");
    let pem = pem.as_bytes();
    repo.write("id_rsa", pem);
    repo.write(".env", b"TOKEN=x\n");
    repo.write("debug.log", pem);
    repo.write("secrets/key.txt", pem);
    // A change that isn't staged, and a new file that is.
    repo.write("a.txt", b"changed but not staged\n");
    repo.write("b.txt", b"staged\n");
    repo.git(&["add", "b.txt"]);

    let committed = plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD");
    assert_eq!(committed.refused, []);
    assert_eq!(paths(&committed), [".gitignore", "a.txt"]);
    assert_eq!(committed.files.iter().find(|f| f.name == "a.txt").map(|f| &f.bytes[..]), Some(&b"committed\n"[..]));

    let staged = plan(repo.path(), &Source::Index, &KnownKeys::new()).expect("plan the index");
    assert_eq!(staged.refused, []);
    assert_eq!(paths(&staged), [".gitignore", "a.txt", "b.txt"]);
    assert_eq!(staged.files.iter().find(|f| f.name == "a.txt").map(|f| &f.bytes[..]), Some(&b"committed\n"[..]));

    // From a subdirectory too, paths are from the root.
    std::fs::create_dir_all(repo.file("sub")).expect("mkdir");
    let from_sub = plan(&repo.file("sub"), &head(), &KnownKeys::new()).expect("plan from a subdirectory");
    assert_eq!(paths(&from_sub), [".gitignore", "a.txt"]);
}

#[test]
fn every_deny_list_entry_and_key_pattern_is_refused() {
    let repo = Repo::new();
    repo.write("ok.txt", b"fine, and mentions secret_key\n");
    let deny_listed = [
        "config/identities/manager.json",
        ".env",
        "app/.env.production",
        "tls/server.pem",
        "tls/server.key",
        "tls/store.p12",
        "tls/store.pfx",
        "id_rsa",
        "home/id_dsa",
        "id_ecdsa",
        "keys/id_ed25519.pub",
    ];
    for path in deny_listed {
        repo.write(path, b"anything\n");
    }
    repo.write("notes/pem.txt", format!("x\n-----BEGIN {}PRIVATE KEY-----\nMIIE\n", "RSA ").as_bytes());
    repo.write("notes/identity.json", format!("{{\"secret_key\": \"{}\"}}\n", "ab".repeat(32)).as_bytes());

    // A raw hex key, and a raw binary one, from a real identities/ directory.
    let ground = tempfile::tempdir().expect("a temporary Ground");
    let identities = ground.path().join("identities");
    std::fs::create_dir(&identities).expect("mkdir identities");
    let identity = manyroot::Identity::generate("weave-integrator").expect("an identity");
    identity.save(&identities.join("weave-integrator.json")).expect("save the identity");
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(identities.join("weave-integrator.json")).expect("read it"))
            .expect("JSON");
    let hex = file["secret_key"].as_str().expect("a secret key").to_owned();
    repo.write("notes/raw-hex.txt", format!("key {hex}\n").as_bytes());
    let raw: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex")).collect();
    repo.write("notes/raw.bin", &raw);
    repo.commit_all();

    let mut known = KnownKeys::new();
    known.add_identities_dir(&identities).expect("read the identities");
    known.add_identities_dir(&ground.path().join("missing")).expect("a missing directory adds nothing");
    assert_eq!(known.len(), 1);

    let plan = plan(repo.path(), &head(), &known).expect("plan HEAD");
    assert!(plan.manifest().is_none(), "one refusal refuses the import");
    let mut expected: BTreeSet<String> = deny_listed.iter().map(|p| (*p).to_owned()).collect();
    expected.extend(["notes/pem.txt", "notes/identity.json", "notes/raw-hex.txt", "notes/raw.bin"].map(String::from));
    assert_eq!(refused_paths(&plan), expected);
    let rule_of = |path: &str| {
        plan.refused.iter().find_map(|r| match r {
            Refusal::Secret { path: p, rule } if p == path => Some(rule.clone()),
            _ => None,
        })
    };
    assert_eq!(rule_of("notes/pem.txt"), Some(SecretRule::PemPrivateKey));
    assert_eq!(rule_of("notes/identity.json"), Some(SecretRule::SecretKeyField));
    let by_identity = Some(SecretRule::IdentityKey { identity: "weave-integrator".to_owned() });
    assert_eq!(rule_of("notes/raw-hex.txt"), by_identity);
    assert_eq!(rule_of("notes/raw.bin"), by_identity);
    // Refusals never show the key.
    for refusal in &plan.refused {
        assert!(!refusal.to_string().contains(&hex), "{refusal}");
    }
}

#[test]
fn bracken_root_entries_are_excluded_and_nested_ones_refused() {
    let repo = Repo::new();
    repo.write("a", b"a\n");
    repo.write(".bracken/objects/ab/cd", b"object\n");
    repo.write(".bracken-team/token", b"t\n");
    repo.write(".bracken-bin/bracken", b"bin\n");
    repo.write("docs/.bracken-notes/x", b"x\n");
    repo.write("deep/er/.Bracken/y", b"y\n");
    repo.commit_all();

    let plan = plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD");
    let excluded: Vec<&str> = plan.excluded.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(excluded, [".bracken-bin/bracken", ".bracken-team/token", ".bracken/objects/ab/cd"]);
    assert_eq!(
        refused_paths(&plan),
        BTreeSet::from(["deep/er/.Bracken/y".to_owned(), "docs/.bracken-notes/x".to_owned()])
    );
    assert!(plan.refused.iter().all(|r| matches!(r, Refusal::Path { error: PathError::Reserved(_), .. })));
    assert!(plan.manifest().is_none());
}

#[test]
fn file_versions_are_deduplicated() {
    let repo = Repo::new();
    repo.write("a", b"same\n");
    repo.write("dir/b", b"same\n");
    repo.executable("c", b"same\n");
    repo.write("d", b"other\n");
    repo.commit_all();

    let plan = plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD");
    let manifest = plan.manifest().expect("nothing refused");
    assert_eq!(manifest.entries.len(), 4);
    assert_eq!(manifest.get("c").map(|e| e.mode), Some(Mode::Executable));
    let names: Vec<&str> = plan.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["a", "d"], "one file version per content hash, named by its first path");
    let held = ContentHash::of(b"same\n");
    let new: Vec<&str> = plan.new_files(|h| *h == held).map(|f| f.name.as_str()).collect();
    assert_eq!(new, ["d"], "only what the Ground lacks is new");
}

#[test]
fn a_file_over_the_limit_is_refused() {
    let repo = Repo::new();
    repo.write("small", b"x\n");
    repo.write("big.bin", &vec![0u8; MAX_FILE_BYTES + 1]);
    repo.commit_all();
    let plan = plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD");
    assert_eq!(plan.refused, [Refusal::TooLarge { path: "big.bin".to_owned(), size: MAX_FILE_BYTES as u64 + 1 }]);
    assert!(plan.files.iter().all(|f| f.name != "big.bin"));
}

#[test]
fn symlinks_are_imported_and_bad_targets_refused() {
    let repo = Repo::new();
    repo.write("dir/file", b"x\n");
    repo.symlink("link", "dir/file");
    repo.symlink("up", "../outside");
    repo.symlink("abs", "/etc/passwd");
    repo.symlink("dot", ".");
    repo.symlink("git", ".git/hooks");
    repo.commit_all();

    let plan = plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD");
    let link = plan.entries.iter().find(|e| e.path == "link").expect("the good symlink is imported");
    assert_eq!(link.mode, Mode::Symlink);
    assert_eq!(link.hash, ContentHash::of(b"dir/file"));
    assert_eq!(refused_paths(&plan), BTreeSet::from(["abs", "dot", "git", "up"].map(String::from)));
    assert!(plan.refused.iter().all(|r| matches!(r, Refusal::Target { .. })));
}

#[test]
fn colliding_paths_in_git_are_refused() {
    // The index can hold paths this file system can't.
    let repo = Repo::new();
    repo.stage_blob("README", b"a\n");
    repo.stage_blob("readme", b"b\n");
    repo.stage_blob("h", b"c\n");
    repo.stage_blob("H/post-checkout", b"d\n");
    let plan = plan(repo.path(), &Source::Index, &KnownKeys::new()).expect("plan the index");
    assert!(matches!(plan.refused.as_slice(), [Refusal::Collision(_)]), "{:?}", plan.refused);
    assert!(plan.manifest().is_none());
}

#[test]
fn bad_revisions_and_unmerged_indexes_are_errors() {
    let repo = Repo::new();
    repo.write("f", b"base\n");
    repo.commit_all();
    for rev in ["no-such-rev", "-x", "", "HEAD:f"] {
        assert!(
            matches!(plan(repo.path(), &Source::Rev(rev.to_owned()), &KnownKeys::new()), Err(ImportError::BadRev(_))),
            "{rev:?}"
        );
    }

    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.write("f", b"other\n");
    repo.commit_all();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f", b"main\n");
    repo.commit_all();
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "merge", "-q", "other"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    assert!(!merge.status.success(), "the merge conflicts");
    assert!(matches!(plan(repo.path(), &Source::Index, &KnownKeys::new()), Err(ImportError::Unmerged(p)) if p == "f"));
    // HEAD is unaffected by the index.
    assert_eq!(plan(repo.path(), &head(), &KnownKeys::new()).expect("plan HEAD").refused, []);
}

#[test]
fn submodules_are_excluded() {
    let repo = Repo::new();
    repo.write("a", b"a\n");
    repo.git(&["add", "a"]);
    let oid = repo.git(&["hash-object", "a"]).trim().to_owned();
    // A gitlink entry, as a submodule leaves in the index.
    repo.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{oid},vendor/lib")]);
    let plan = plan(repo.path(), &Source::Index, &KnownKeys::new()).expect("plan the index");
    assert_eq!(plan.excluded, [("vendor/lib".to_owned(), Exclusion::Submodule)]);
    assert_eq!(paths(&plan), ["a"]);
}

// Finding a2964a2a2023: --from-git takes a commit's tree, never a subtree.
#[test]
fn only_a_commit_is_a_revision() {
    let repo = Repo::new();
    repo.write("tmp/identities/backup", b"key\n");
    repo.write(".bracken-bin/bk", b"bin\n");
    repo.write("a", b"a\n");
    repo.commit_all();
    let tree = repo.git(&["rev-parse", "HEAD:tmp/identities"]).trim().to_owned();
    let bin = repo.git(&["rev-parse", "HEAD:.bracken-bin"]).trim().to_owned();
    let root = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_owned();
    repo.git(&["tag", "tree-tag", &tree]);
    repo.git(&["tag", "-a", "-m", "annotated", "annotated-tree-tag", &bin]);
    repo.git(&["tag", "-a", "-m", "annotated", "release", "HEAD"]);
    for rev in [tree.as_str(), bin.as_str(), root.as_str(), "tree-tag", "annotated-tree-tag", "HEAD:tmp/identities"] {
        let result = plan(repo.path(), &Source::Rev(rev.to_owned()), &KnownKeys::new());
        assert!(matches!(result, Err(ImportError::BadRev(_))), "{rev:?}: {result:?}");
    }
    // A commit, by name, id or annotated tag, still works.
    let commit = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    for rev in ["HEAD", commit.as_str(), "main", "release"] {
        let planned = plan(repo.path(), &Source::Rev(rev.to_owned()), &KnownKeys::new()).expect("a commit");
        assert!(
            planned
                .refused
                .iter()
                .any(|r| matches!(r, Refusal::Secret { path, .. } if path == "tmp/identities/backup"))
        );
    }
}

/// Plans `WEAVE_TEST_REPO`'s index with whatever GIT_* environment the parent
/// gave it, and checks it read that repository only. Run only by
/// `the_callers_git_environment_and_fsmonitor_are_ignored`.
#[test]
#[ignore = "run by the_callers_git_environment_and_fsmonitor_are_ignored"]
fn git_environment_child() {
    let Some(repo) = std::env::var_os("WEAVE_TEST_REPO") else { return };
    let staged = plan(Path::new(&repo), &Source::Index, &KnownKeys::new()).expect("plan the index");
    assert_eq!(paths(&staged), ["mine"]);
    let committed = plan(Path::new(&repo), &head(), &KnownKeys::new()).expect("plan HEAD");
    assert_eq!(paths(&committed), ["mine"]);
}

// Finding 9152cdb0a532: import reads the repository at `repo` and nothing else,
// and runs no configured command.
#[test]
fn the_callers_git_environment_and_fsmonitor_are_ignored() {
    let mine = Repo::new();
    mine.write("mine", b"mine\n");
    mine.commit_all();
    let other = Repo::new();
    other.write("theirs", b"theirs\n");
    other.commit_all();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let marker = scratch.path().join("ran");
    let hook = scratch.path().join("fsmonitor");
    std::fs::write(&hook, format!("#!/bin/sh\necho > '{}'\n", marker.display())).expect("write the hook");
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    // In the repository's own config too.
    mine.git(&["config", "core.fsmonitor", hook.to_str().expect("UTF-8")]);

    let exe = std::env::current_exe().expect("the test binary");
    let status = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "git_environment_child", "--test-threads=1", "-q"])
        .env("WEAVE_TEST_REPO", mine.path())
        .env("GIT_DIR", other.path().join(".git"))
        .env("GIT_COMMON_DIR", other.path().join(".git"))
        .env("GIT_WORK_TREE", other.path())
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", &hook)
        .env("GIT_CONFIG_PARAMETERS", format!("'core.fsmonitor'='{}'", hook.display()))
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run the child");
    assert!(status.success(), "the child read another repository");
    assert!(!marker.exists(), "a configured fsmonitor ran");
}

// Security's note on PR #98: the key scan doesn't follow a symlink in
// identities/, reads only regular files, and refuses one too large to be an
// identity.
#[test]
fn the_key_scan_refuses_symlinks_and_oversized_identity_files() {
    let ground = tempfile::tempdir().expect("a temporary Ground");
    let elsewhere = ground.path().join("elsewhere.json");
    std::fs::write(&elsewhere, format!("{{\"secret_key\": \"{}\"}}\n", "cd".repeat(32))).expect("write");

    let linked = ground.path().join("linked");
    std::fs::create_dir(&linked).expect("mkdir");
    std::os::unix::fs::symlink(&elsewhere, linked.join("a.json")).expect("symlink");
    let error = KnownKeys::new().add_identities_dir(&linked).expect_err("a symlink is refused");
    assert!(error.to_string().contains("a.json"), "{error}");

    let large = ground.path().join("large");
    std::fs::create_dir(&large).expect("mkdir");
    let mut big = format!("{{\"secret_key\": \"{}\", \"pad\": \"", "cd".repeat(32)).into_bytes();
    big.resize(bracken_tree::secrets::MAX_IDENTITY_FILE_BYTES as usize + 1, b'x');
    std::fs::write(large.join("b.json"), &big).expect("write");
    let error = KnownKeys::new().add_identities_dir(&large).expect_err("an oversized file is refused");
    assert!(error.to_string().contains("too large"), "{error}");

    let dir = ground.path().join("dir");
    std::fs::create_dir_all(dir.join("c.json")).expect("mkdir");
    let error = KnownKeys::new().add_identities_dir(&dir).expect_err("a directory is refused");
    assert!(error.to_string().contains("not a regular file"), "{error}");
}

/// Finding 0bbc434b2588: resolve --file reads through one open that follows no symlink, so a symlink
/// anywhere on the path, a FIFO, or a file over the limit is refused, and the bytes are the file's.
#[cfg(unix)]
#[test]
fn read_beneath_follows_no_symlink_and_reads_one_file() {
    use bracken_tree::import::{ReadError, read_beneath};
    use std::os::unix::fs::{PermissionsExt, symlink};

    let tmp = tempfile::tempdir().expect("a temporary directory");
    let root = tmp.path().join("root");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(root.join("dir")).expect("mkdir");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("f"), b"outside\n").expect("write");
    std::fs::write(root.join("dir/f"), b"inside\n").expect("write");
    std::fs::set_permissions(root.join("dir/f"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    symlink(&outside, root.join("linked")).expect("symlink");
    symlink(outside.join("f"), root.join("dir/link")).expect("symlink");
    assert!(std::process::Command::new("mkfifo").arg(root.join("fifo")).status().expect("mkfifo").success());

    let read = read_beneath(&root, "dir/f").expect("a regular file");
    assert_eq!((read.bytes.as_slice(), read.executable), (b"inside\n".as_slice(), true));
    for path in ["linked/f", "dir/link"] {
        assert!(matches!(read_beneath(&root, path), Err(ReadError::Open { .. })), "{path}");
    }
    assert!(matches!(read_beneath(&root, "fifo"), Err(ReadError::NotAFile(_))));
    assert!(matches!(read_beneath(&root, "dir"), Err(ReadError::NotAFile(_))));
    let big = std::fs::File::create(root.join("big")).expect("create");
    big.set_len(bracken_tree::import::MAX_FILE_BYTES as u64 + 1).expect("grow");
    assert!(matches!(read_beneath(&root, "big"), Err(ReadError::TooLarge { .. })));
}
