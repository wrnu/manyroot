//! `bracken weave`, manifests only (docs/design/weave.md, section 3; build step 3).
//! Every test uses a throwaway Ground: never this repository's `.bracken/`, and
//! never a real Weave Ground.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct Weave {
    _tmp: TempDir,
    ground: PathBuf,
    repo: PathBuf,
}

const AS: &str = "weave-integrator";

impl Weave {
    /// A throwaway Ground at `<tmp>/g`, and an empty git repository at `<tmp>/repo`.
    fn new() -> Weave {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let ground = tmp.path().join("g");
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).expect("mkdir repo");
        let w = Weave { _tmp: tmp, ground, repo };
        w.git(&["init", "-q", "-b", "main"]);
        w
    }

    fn initialized() -> Weave {
        let w = Weave::new();
        w.ok(&["weave", "init", "--as", AS]);
        w
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(&self.repo)
            .env("BRACKEN_DIR", &self.ground)
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("bracken runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    }

    /// Runs a command that must fail, and returns its stderr and stdout.
    fn fails(&self, args: &[&str]) -> (String, String) {
        let out = self.run(args);
        assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
        (String::from_utf8(out.stderr).expect("UTF-8"), String::from_utf8(out.stdout).expect("UTF-8"))
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).expect("JSON")
    }

    fn contributions(&self) -> usize {
        self.json(&["log", "--json", "--limit", "100000"]).as_array().map_or(0, Vec::len)
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let file = self.repo.join(path);
        fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
        fs::write(file, bytes).expect("write");
    }

    fn commit(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "c"]);
    }

    /// Imports HEAD and returns the manifest's labelled hash.
    fn import(&self) -> String {
        let out = self.ok(&["weave", "import", "--as", AS]);
        let first = out.lines().next().expect("a manifest line");
        first.strip_prefix("manifest ").expect("labelled").split(' ').next().expect("a hash").to_owned()
    }
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[test]
fn init_writes_a_marker_and_a_main_line_and_keeps_keys_private() {
    let w = Weave::new();
    let out = w.ok(&["weave", "init", "--as", AS]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    let marker = lines[0].strip_prefix("marker ").expect("a labelled marker line");
    let line = lines[1].strip_prefix("line ").and_then(|l| l.strip_suffix(" (main)")).expect("a labelled line");
    assert!(hex64(marker) && hex64(line), "{out}");
    let mode = fs::metadata(w.ground.join("identities")).expect("identities").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert_eq!(w.ok(&["weave", "check"]).trim(), "no problems");
    assert_eq!(w.json(&["weave", "check", "--json"])["marker"], marker);

    // Not twice: --dir must be new.
    let (err, _) = w.fails(&["weave", "init", "--as", AS]);
    assert!(err.contains("already exists"), "{err}");

    // --json names its fields.
    let other = Weave::new();
    let json = other.json(&["weave", "init", "--as", AS, "--json"]);
    assert!(json["marker"].as_str().is_some_and(hex64) && json["line"].as_str().is_some_and(hex64), "{json}");
}

#[test]
fn init_refuses_inside_a_git_checkout() {
    let w = Weave::new();
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["weave", "init", "--as", AS])
        .env("BRACKEN_DIR", w.repo.join("g"))
        .output()
        .expect("bracken runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the git checkout"));
    assert!(!w.repo.join("g").exists(), "nothing was created");
}

// Finding f2ebc53cf69a: a symlinked parent doesn't hide the checkout it points into.
#[test]
fn init_refuses_a_checkout_reached_through_a_symlinked_parent() {
    let w = Weave::new();
    fs::create_dir(w.repo.join("sub")).expect("mkdir sub");
    let link = w._tmp.path().join("link");
    std::os::unix::fs::symlink(w.repo.join("sub"), &link).expect("symlink");
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["weave", "init", "--as", AS])
        .env("BRACKEN_DIR", link.join("g"))
        .output()
        .expect("bracken runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the git checkout"));
    assert!(!w.repo.join("sub/g").exists(), "nothing was created");
}

// Finding 0c6a42a8d926, security's reproduction: a `..` after a directory that
// doesn't exist yet would resolve, once created, through a symlink into a repo.
#[test]
fn init_refuses_dot_dot_after_a_missing_directory() {
    let w = Weave::new();
    fs::create_dir(w.repo.join("sub")).expect("mkdir sub");
    let outside = w._tmp.path().join("outside");
    fs::create_dir_all(outside.join("E")).expect("mkdir outside/E");
    std::os::unix::fs::symlink(w.repo.join("sub"), outside.join("S")).expect("symlink");
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .arg("--dir")
        .arg(outside.join("E/a/../../S/g5"))
        .args(["weave", "init", "--as", AS])
        .env_remove("BRACKEN_DIR")
        .output()
        .expect("bracken runs");
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("needs the parent of --dir to exist"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!w.repo.join("sub/g5").exists(), "no Ground in the repository");
    assert!(!outside.join("E/a").exists(), "nothing was created");
}

/// Runs `bracken --dir <dir> weave init` and returns its stderr; it must fail.
fn init_fails(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .arg("--dir")
        .arg(dir)
        .args(["weave", "init", "--as", AS])
        .env_remove("BRACKEN_DIR")
        .output()
        .expect("bracken runs");
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8(out.stderr).expect("UTF-8")
}

// Finding 03e2743c0e78, security's reproduction: an `identities` symlink already
// inside --dir would receive the key. --dir must be new, so it's refused.
#[test]
fn init_refuses_an_existing_dir_with_an_identities_symlink() {
    let w = Weave::new();
    fs::create_dir(w.repo.join("keys")).expect("mkdir keys");
    let outside = w._tmp.path().join("outside/P");
    fs::create_dir_all(&outside).expect("mkdir outside/P");
    std::os::unix::fs::symlink(w.repo.join("keys"), outside.join("identities")).expect("symlink");
    let err = init_fails(&outside);
    assert!(err.contains("already exists"), "{err}");
    assert!(fs::read_dir(w.repo.join("keys")).expect("read keys").next().is_none(), "no key in the checkout");
}

// Decision 7529dd229d6f: --dir must not exist yet, even empty, and its parent must.
#[test]
fn init_refuses_an_existing_empty_dir_and_a_missing_parent() {
    let w = Weave::new();
    let empty = w._tmp.path().join("empty");
    fs::create_dir(&empty).expect("mkdir empty");
    let err = init_fails(&empty);
    assert!(err.contains("already exists"), "{err}");
    assert!(fs::read_dir(&empty).expect("read").next().is_none(), "nothing was written");

    let missing = w._tmp.path().join("missing/g");
    let err = init_fails(&missing);
    assert!(err.contains("needs the parent of --dir to exist"), "{err}");
    assert!(!w._tmp.path().join("missing").exists(), "nothing was created");

    // Finding 01a2de3d8cfb: a path with no final name, such as `X/..`, is refused.
    let err = init_fails(&empty.join(".."));
    assert!(err.contains("doesn't end in a directory name"), "{err}");
    assert!(fs::read_dir(&empty).expect("read").next().is_none(), "nothing was written");
}

#[test]
fn writes_are_refused_without_a_weave_marker() {
    let w = Weave::new();
    w.write("a", b"a\n");
    w.commit();
    w.ok(&["init"]);
    w.ok(&["id", "new", AS]);
    let before = w.contributions();
    let (err, _) = w.fails(&["weave", "import", "--as", AS]);
    assert!(err.contains("no Weave marker"), "{err}");
    assert_eq!(w.contributions(), before, "nothing was written");
    // A dry run writes nothing, so it needs no marker.
    w.ok(&["weave", "import", "--dry-run"]);
}

#[test]
fn import_prints_one_labelled_line_per_record_and_writes_only_new_files() {
    let w = Weave::initialized();
    w.write("a.txt", b"a\n");
    w.write("dir/b.txt", b"b\n");
    w.write("dir/same.txt", b"a\n");
    w.commit();
    let out = w.ok(&["weave", "import", "--as", AS]);
    let lines: Vec<&str> = out.lines().collect();
    let manifest = lines[0].strip_prefix("manifest blake3:").expect("the manifest first");
    assert!(hex64(manifest), "{out}");
    let files: Vec<&str> = lines[1..].iter().map(|l| l.strip_prefix("file blake3:").expect("then files")).collect();
    let paths: Vec<&str> = files.iter().map(|f| f.split_once(' ').expect("hash and path").1).collect();
    assert_eq!(paths, ["a.txt", "dir/b.txt"], "one file version per content hash");

    // The same tree again: nothing new.
    // No `manifest` line, since no manifest was written.
    assert_eq!(
        w.ok(&["weave", "import", "--as", AS]).trim(),
        format!("held blake3:{manifest}: this manifest was already held, so none was written")
    );

    // One file changes: only it is new.
    w.write("dir/b.txt", b"b2\n");
    w.commit();
    let json = w.json(&["weave", "import", "--as", AS, "--json"]);
    assert!(json["manifest"].as_str().is_some_and(|m| m.starts_with("blake3:")), "{json}");
    assert!(json["manifest_id"].as_str().is_some_and(hex64), "{json}");
    let files = json["files"].as_array().expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "dir/b.txt");
    assert!(files[0]["hash"].as_str().is_some_and(|h| h.starts_with("blake3:")));
    assert!(files[0]["id"].as_str().is_some_and(hex64));
}

#[test]
fn a_dry_run_lists_everything_and_writes_nothing() {
    let w = Weave::initialized();
    w.write("a.txt", b"a\n");
    w.write(".bracken/objects/x", b"x\n");
    w.commit();
    let before = w.contributions();
    let out = w.ok(&["weave", "import", "--dry-run"]);
    assert!(out.lines().any(|l| l.starts_with("file blake3:") && l.ends_with(" 100644 2 a.txt (new)")), "{out}");
    assert!(out.lines().any(|l| l == "excluded .bracken/objects/x (a .bracken root entry)"), "{out}");
    assert!(out.lines().last().is_some_and(|l| l.contains("dry run, nothing written")), "{out}");
    let json = w.json(&["weave", "import", "--dry-run", "--json"]);
    assert_eq!(json["files"][0]["path"], "a.txt");
    assert_eq!(json["files"][0]["new"], true);
    assert_eq!(json["excluded"][0]["path"], ".bracken/objects/x");
    assert_eq!(w.contributions(), before);
}

#[test]
fn a_secret_refuses_the_whole_import() {
    let w = Weave::initialized();
    w.write("a.txt", b"a\n");
    w.write("config/.env", b"TOKEN=x\n");
    w.commit();
    let before = w.contributions();
    let (err, out) = w.fails(&["weave", "import", "--as", AS]);
    assert!(err.contains("nothing was written"), "{err}");
    assert!(out.contains("refused \"config/.env\" looks like a secret"), "{out}");
    assert_eq!(w.contributions(), before);
    let out = w.run(&["weave", "import", "--as", AS, "--json"]);
    assert!(!out.status.success());
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(json["refused"].as_array().map(Vec::len), Some(1));
}

#[test]
fn checkout_writes_the_tree_and_refuses_ids_of_other_kinds() {
    let w = Weave::initialized();
    w.write("a.txt", b"a\n");
    w.write("bin/run", b"#!/bin/sh\n");
    fs::set_permissions(w.repo.join("bin/run"), fs::Permissions::from_mode(0o755)).expect("chmod");
    std::os::unix::fs::symlink("a.txt", w.repo.join("link")).expect("symlink");
    w.commit();
    let manifest = w.import();

    let target = w._tmp.path().join("out");
    fs::create_dir(&target).expect("mkdir");
    let out = w.ok(&["weave", "checkout", "--manifest", &manifest, target.to_str().expect("UTF-8")]);
    assert_eq!(out.trim(), format!("checkout {manifest} {}", target.display()));
    assert_eq!(fs::read(target.join("a.txt")).expect("read"), b"a\n");
    assert_eq!(fs::metadata(target.join("bin/run")).expect("stat").permissions().mode() & 0o777, 0o755);
    assert_eq!(fs::read_link(target.join("link")).expect("a symlink"), Path::new("a.txt"));
    // Not into a directory that holds anything.
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &manifest, target.to_str().expect("UTF-8")]);
    assert!(err.contains("not an empty directory"), "{err}");

    let fresh = |name: &str| {
        let dir = w._tmp.path().join(name);
        fs::create_dir(&dir).expect("mkdir");
        dir.to_str().expect("UTF-8").to_owned()
    };
    // A bare hex value is a contribution id: refused, naming its kind.
    let marker = w.json(&["weave", "check", "--json"])["marker"].as_str().expect("a marker").to_owned();
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &marker, &fresh("t1")]);
    assert!(err.contains("of a Weave marker, not a manifest's content hash; give blake3:"), "{err}");
    w.ok(&["id", "new", "someone"]);
    let claim = w.ok(&["claim", "a thought", "--as", "someone"]);
    let claim = claim.trim().strip_prefix("claim ").expect("labelled");
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &claim[..12], &fresh("t2")]);
    assert!(err.contains("of a claim, not a manifest's content hash"), "{err}");
    // The bare hex of the manifest's own hash.
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &manifest["blake3:".len()..], &fresh("t3")]);
    assert!(err.contains("blake3:<64 hex digits>"), "{err}");
    // A content hash of something that isn't a manifest.
    let file = format!("blake3:{}", blake3::hash(b"a\n").to_hex());
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &file, &fresh("t4")]);
    assert!(err.contains("is a file, not a manifest"), "{err}");
    let unknown = format!("blake3:{}", "0".repeat(64));
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &unknown, &fresh("t5")]);
    assert!(err.contains("holds no manifest"), "{err}");
}

#[test]
fn diff_lists_each_changed_file_with_a_unified_diff() {
    let w = Weave::initialized();
    w.write("keep.txt", b"k\n");
    w.write("edit.txt", b"one\ntwo\nthree\n");
    w.write("gone.txt", b"g\n");
    w.write("tool", b"#!/bin/sh\n");
    w.commit();
    let a = w.import();
    w.write("edit.txt", b"one\nTWO\nthree\n");
    fs::remove_file(w.repo.join("gone.txt")).expect("delete");
    w.write("new.txt", b"n\n");
    fs::set_permissions(w.repo.join("tool"), fs::Permissions::from_mode(0o755)).expect("chmod");
    w.commit();
    let b = w.import();

    let out = w.ok(&["weave", "diff", &a, &b]);
    assert_eq!(
        out,
        "M edit.txt\n--- a/edit.txt\n+++ b/edit.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n\
         D gone.txt\nA new.txt\nT tool (100644 -> 100755)\n"
    );
    let json = w.json(&["weave", "diff", &a, &b, "--json"]);
    assert_eq!(json["a"], a.as_str());
    let changes: Vec<(&str, &str)> = json["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["path"].as_str().expect("path"), f["change"].as_str().expect("change")))
        .collect();
    assert_eq!(
        changes,
        [("edit.txt", "modified"), ("gone.txt", "deleted"), ("new.txt", "added"), ("tool", "mode changed")]
    );
    assert_eq!(json["files"][3]["after"]["mode"], "100755");
}

#[test]
fn verify_compares_with_this_repositorys_own_git_tree() {
    let w = Weave::initialized();
    let this_repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let this = this_repo.to_str().expect("UTF-8");
    let out = w.ok(&["weave", "import", "--repo", this, "--as", AS]);
    let manifest = out.lines().next().and_then(|l| l.strip_prefix("manifest ")).expect("a manifest").to_owned();
    let out = w.ok(&["weave", "verify", "--repo", this, "--git", "HEAD", "--manifest", &manifest]);
    assert_eq!(out.trim(), format!("equal: {manifest} is git's HEAD"));
    let json = w.json(&["weave", "verify", "--repo", this, "--git", "HEAD", "--manifest", &manifest, "--json"]);
    assert_eq!(json["equal"], true);

    // Against another tree, it differs and says how.
    w.write("only-here.txt", b"x\n");
    w.commit();
    let out = w.run(&["weave", "verify", "--git", "HEAD", "--manifest", &manifest, "--json"]);
    assert!(!out.status.success(), "a difference exits non-zero");
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(json["equal"], false);
    let differences = json["differences"].as_array().expect("differences");
    assert!(differences.iter().any(|d| d["difference"] == "only_in_git" && d["path"] == "only-here.txt"));
    assert!(differences.iter().any(|d| d["difference"] == "only_in_manifest" && d["path"] == "Cargo.toml"));
}

#[test]
fn check_lists_invalid_payloads_incomplete_manifests_and_extra_markers() {
    let w = Weave::initialized();
    let junk = w._tmp.path().join("junk");
    fs::write(&junk, b"not CBOR").expect("write");
    let junk = junk.to_str().expect("UTF-8");
    w.ok(&[
        "artifact",
        junk,
        "--name",
        "manifest",
        "--media-type",
        "application/vnd.bracken.weave.manifest+cbor",
        "--as",
        AS,
    ]);
    // A valid manifest naming a file this Ground doesn't hold.
    let entry = bracken_tree::Entry {
        path: "missing.txt".to_owned(),
        mode: bracken_tree::Mode::File,
        hash: bracken_tree::ContentHash::of(b"missing"),
        size: 7,
    };
    let manifest = bracken_tree::Manifest::new(vec![entry]);
    let bytes = w._tmp.path().join("manifest.cbor");
    fs::write(&bytes, bracken_tree::Record::encode(&manifest)).expect("write");
    let bytes = bytes.to_str().expect("UTF-8");
    w.ok(&[
        "artifact",
        bytes,
        "--name",
        "manifest",
        "--media-type",
        "application/vnd.bracken.weave.manifest+cbor",
        "--as",
        AS,
    ]);
    // A second marker.
    let marker = bracken_tree::Marker { v: 1, purpose: "again".to_owned(), in_place: None };
    let path = w._tmp.path().join("marker.cbor");
    fs::write(&path, bracken_tree::Record::encode(&marker)).expect("write");
    w.ok(&[
        "artifact",
        path.to_str().expect("UTF-8"),
        "--name",
        "weave",
        "--media-type",
        "application/vnd.bracken.weave.ground+cbor",
        "--as",
        AS,
    ]);

    let out = w.run(&["weave", "check", "--json"]);
    assert!(!out.status.success(), "problems exit non-zero in JSON too");
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let kinds: Vec<&str> =
        json["problems"].as_array().expect("problems").iter().map(|p| p["problem"].as_str().expect("named")).collect();
    let mut sorted = kinds.clone();
    sorted.sort();
    assert_eq!(sorted, ["extra_marker", "incomplete", "invalid_payload"], "{json}");
    let incomplete =
        json["problems"].as_array().expect("problems").iter().find(|p| p["problem"] == "incomplete").expect("one");
    assert_eq!(incomplete["missing"][0], "missing.txt");
    let (_, text) = w.fails(&["weave", "check"]);
    assert_eq!(text.lines().count(), 3, "{text}");
    // An incomplete manifest isn't checked out.
    let hash = incomplete["manifest"].as_str().expect("a hash").to_owned();
    let target = w._tmp.path().join("out");
    fs::create_dir(&target).expect("mkdir");
    let (err, _) = w.fails(&["weave", "checkout", "--manifest", &hash, target.to_str().expect("UTF-8")]);
    assert!(err.contains("isn't complete here"), "{err}");
}

/// Note fefde0fa66f2 item 3: past the envelope margin, an import still refuses
/// before writing anything, rather than stopping part way.
#[test]
fn an_import_over_the_contribution_limit_writes_nothing() {
    let w = Weave::initialized();
    // 2,000 concurrent heads: each next write would list them all as parents,
    // about 68 KB, more than the 64 KiB margin.
    {
        let mut store = manyroot::Store::open(&w.ground).expect("open the throwaway Ground");
        let who = store.identity(AS).expect("the identity");
        let ground = store.info().ground;
        for i in 0..2_000 {
            let body = manyroot::Body::Claim { statement: format!("head {i}"), tags: vec![] };
            store.add(manyroot::Contribution::create(ground, &who, [], 1, body)).expect("add a root");
        }
    }
    w.write("big.bin", &vec![7u8; bracken_tree::import::MAX_FILE_BYTES]);
    w.commit();
    let before = w.contributions();
    let (err, _) = w.fails(&["weave", "import", "--as", AS]);
    assert!(err.contains("over the 16777216-byte limit; nothing was written"), "{err}");
    assert_eq!(w.contributions(), before);
}

/// Finding 8d33998966ab: `verify` and `check` print names from git and the Ground
/// with control characters escaped.
#[test]
fn verify_and_check_escape_control_characters() {
    let w = Weave::initialized();
    w.write("a.txt", b"a\n");
    w.commit();
    let manifest = w.import();
    let name = format!("bell{}{}[31m.txt", '\u{7}', '\u{1b}');
    w.write(&name, b"b\n");
    w.commit();
    let (_, out) = w.fails(&["weave", "verify", "--git", "HEAD", "--manifest", &manifest]);
    assert!(out.contains("bell"), "{out}");
    assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{out:?}");

    let junk = w._tmp.path().join("junk");
    fs::write(&junk, b"x").expect("write");
    let media_type = format!("application/vnd.bracken.weave.{}[31mred", '\u{1b}');
    w.ok(&["artifact", junk.to_str().expect("UTF-8"), "--name", "j", "--media-type", &media_type, "--as", AS]);
    let (_, out) = w.fails(&["weave", "check"]);
    assert!(out.contains("red"), "{out}");
    assert!(!out.contains('\u{1b}'), "{out:?}");
}
