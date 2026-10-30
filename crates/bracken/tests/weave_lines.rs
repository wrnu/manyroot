//! `bracken weave`, lines and decisions: `integrate` of a given tree, `head`,
//! `log`, `status`, `diff` and `verify --at` (docs/design/weave.md, section 3;
//! build step 5). Every test uses throwaway Grounds: never this repository's
//! `.bracken/`, and never a real Weave Ground.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use bracken_tree::format::{Guardrails, LINE_MEDIA_TYPE, Line, Record, VERSION};
use serde_json::Value;
use tempfile::TempDir;

const AS: &str = "weave-integrator";

struct Weave {
    tmp: TempDir,
    ground: PathBuf,
    repo: PathBuf,
}

impl Weave {
    /// A Weave Ground at `<tmp>/g`, made by `weave init --as weave-integrator`, and
    /// a git repository at `<tmp>/repo` with one commit.
    fn new() -> Weave {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let ground = tmp.path().join("g");
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).expect("mkdir repo");
        let w = Weave { tmp, ground, repo };
        w.git(&["init", "-q", "-b", "main"]);
        w.ok(&["weave", "init", "--as", AS]);
        w.commit("a.txt", b"one\n");
        w
    }

    fn run_on(&self, ground: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(&self.repo)
            .env("BRACKEN_DIR", ground)
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("bracken runs")
    }

    fn ok_on(&self, ground: &Path, args: &[&str]) -> String {
        let out = self.run_on(ground, args);
        assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    }

    fn ok(&self, args: &[&str]) -> String {
        self.ok_on(&self.ground, args)
    }

    /// Runs a command that must fail, and returns its stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run_on(&self.ground, args);
        assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
        String::from_utf8(out.stderr).expect("UTF-8")
    }

    fn json_on(&self, ground: &Path, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok_on(ground, args)).expect("JSON")
    }

    fn json(&self, args: &[&str]) -> Value {
        self.json_on(&self.ground, args)
    }

    fn contributions(&self) -> usize {
        let log = self.json(&["log", "--json", "--limit", "100000"]);
        log["contributions"].as_array().expect("a list of contributions").len()
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
        fs::write(self.repo.join(path), bytes).expect("write");
    }

    fn commit(&self, path: &str, bytes: &[u8]) {
        self.write(path, bytes);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "c"]);
    }

    /// `integrate` on `ground` as `who`; returns the decision's id.
    fn integrate_on(&self, ground: &Path, who: &str) -> String {
        let out = self.json_on(ground, &["weave", "integrate", "--as", who, "--json"]);
        out["decision"].as_str().expect("a decision").to_owned()
    }

    fn heads_on(&self, ground: &Path) -> Vec<String> {
        let head = self.json_on(ground, &["weave", "head", "--json"]);
        head["heads"]
            .as_array()
            .expect("heads")
            .iter()
            .map(|h| h["decision"].as_str().expect("id").to_owned())
            .collect()
    }

    /// Copies every contribution of `from` into `to`.
    fn sync(&self, from: &Path, to: &Path) {
        let bundle = self.tmp.path().join("bundle");
        self.ok_on(from, &["export", bundle.to_str().expect("UTF-8")]);
        self.ok_on(to, &["import", bundle.to_str().expect("UTF-8")]);
    }
}

#[test]
fn integrate_records_a_tree_and_moves_the_head() {
    let w = Weave::new();
    let before = w.contributions();
    let out = w.ok(&["weave", "integrate", "--as", AS, "--note", "first"]);
    let lines: Vec<&str> = out.lines().collect();
    // The labelled lines, in their fixed order.
    assert_eq!(lines.len(), 3, "{out}");
    let manifest = lines[0].strip_prefix("manifest ").expect("the manifest first");
    assert!(manifest.starts_with("blake3:"), "{out}");
    assert!(lines[1].starts_with("file blake3:") && lines[1].ends_with(" a.txt"), "{out}");
    let first = lines[2].strip_prefix("decision ").expect("the decision last");
    assert_eq!(w.contributions(), before + 3, "a file version, a manifest and a decision");

    let head = w.json(&["weave", "head", "--json"]);
    assert_eq!(head["forked"], false);
    assert_eq!(head["name"], "main");
    assert_eq!(head["heads"][0]["decision"], first);
    assert_eq!(head["heads"][0]["manifest"], manifest);
    let text = w.ok(&["weave", "head"]);
    assert!(text.contains(&format!("head {first} manifest {manifest}")), "{text}");

    // The decision names the commit it mirrors, in full.
    let sha = w.git(&["rev-parse", "HEAD"]);
    let log = w.json(&["weave", "log", "--json"]);
    assert_eq!(log["decisions"][0]["source"]["git"], sha.trim());
    assert_eq!(log["decisions"][0]["note"], "first");
    assert_eq!(log["decisions"][0]["author_name"], AS);
    assert_eq!(log["more"], false);
    assert!(w.ok(&["weave", "verify", "--git", "HEAD", "--at", first]).starts_with("equal: "));
    assert!(w.ok(&["weave", "verify", "--git", "HEAD"]).starts_with("equal: "), "the head by default");
    assert_eq!(w.ok(&["weave", "status"]).lines().last(), Some("no differences"));

    // Status sees a staged change in the index, and HEAD once it is committed.
    w.write("a.txt", b"two\n");
    w.write("b.txt", b"new\n");
    w.git(&["add", "-A"]);
    let status = w.json(&["weave", "status", "--index", "--json"]);
    let files: Vec<(&str, &str)> = status["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["path"].as_str().expect("path"), f["change"].as_str().expect("change")))
        .collect();
    assert_eq!(files, [("a.txt", "modified"), ("b.txt", "added")]);
    assert_eq!(status["at"], first);
    assert_eq!(w.ok(&["weave", "status"]).lines().last(), Some("no differences"), "HEAD is unchanged");
    w.git(&["commit", "-q", "-m", "c2"]);

    let second = w.integrate_on(&w.ground, AS);
    assert_eq!(w.heads_on(&w.ground), [second.as_str()]);
    let log = w.json(&["weave", "log", "--json"]);
    assert_eq!(log["decisions"][0]["parents"][0], first, "the new decision lists the old head");
    assert!(w.ok(&["weave", "verify", "--git", "HEAD~1", "--at", first]).starts_with("equal: "));
    assert!(!w.run_on(&w.ground, &["weave", "verify", "--git", "HEAD~1", "--at", &second]).status.success());

    // diff takes decisions as well as manifests.
    let diff = w.ok(&["weave", "diff", first, &second]);
    assert!(diff.contains("M a.txt") && diff.contains("A b.txt") && diff.contains("-one") && diff.contains("+two"));
    let by_hash = w.ok(&["weave", "diff", manifest, &second]);
    assert_eq!(diff, by_hash);

    // A held manifest can be recorded as it is: nothing but the decision is written.
    let before = w.contributions();
    let out = w.ok(&["weave", "integrate", "--as", AS, "--manifest", manifest]);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert_eq!(w.contributions(), before + 1);
    assert!(w.ok(&["weave", "verify", "--git", "HEAD~1"]).starts_with("equal: "));
}

#[test]
fn integrate_refuses_what_wouldnt_qualify_and_writes_nothing() {
    let w = Weave::new();
    w.ok(&["id", "new", "stranger"]);
    let before = w.contributions();
    let err = w.fails(&["weave", "integrate", "--as", "stranger"]);
    assert!(err.contains("isn't an integrator"), "{err}");
    assert!(err.contains("nothing was written"), "{err}");
    assert_eq!(w.contributions(), before);

    // Without a marker, nothing is written either.
    let plain = w.tmp.path().join("plain");
    w.ok_on(&plain, &["init"]);
    w.ok_on(&plain, &["id", "new", AS]);
    let out = w.run_on(&plain, &["weave", "integrate", "--as", AS]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no Weave marker"));

    // Records named by the wrong kind are refused, naming the kind.
    let id = w.integrate_on(&w.ground, AS);
    let manifest = w.json(&["weave", "head", "--json"])["heads"][0]["manifest"].as_str().expect("hash").to_owned();
    let err = w.fails(&["weave", "verify", "--git", "HEAD", "--at", &manifest]);
    assert!(err.contains("is a content hash; a decision is named by its id"), "{err}");
    let err = w.fails(&["weave", "head", "--line", &id]);
    assert!(err.contains("is a decision, not a line"), "{err}");
    let err = w.fails(&["weave", "log", "--since", &manifest[7..]]);
    assert!(err.contains("--since"), "{err}");
    let err = w.fails(&["weave", "integrate", "--as", AS, "--manifest", &manifest[7..]]);
    assert!(err.contains("blake3:"), "{err}");
}

#[test]
fn two_integrators_fork_the_line_and_a_merge_settles_it() {
    let w = Weave::new();
    let first = w.integrate_on(&w.ground, AS);

    // A second replica, with its own integrator.
    let bundle = w.tmp.path().join("bundle");
    w.ok(&["export", bundle.to_str().expect("UTF-8")]);
    let other = w.tmp.path().join("other");
    w.ok_on(&other, &["join", bundle.to_str().expect("UTF-8")]);
    let bob = w.ok_on(&other, &["id", "new", "bob"]).trim().to_owned();

    // The line's author adds bob as an integrator: a new policy version that supersedes the first.
    let head = w.json(&["weave", "head", "--json"]);
    let line = head["line"].as_str().expect("the line").to_owned();
    let alice = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    let policy = Line {
        v: VERSION,
        name: "main".into(),
        integrators: vec![alice.parse().expect("a key"), bob.parse().expect("a key")],
        validators: vec![],
        goal_setters: vec![alice.parse().expect("a key")],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: None,
    };
    let file = w.tmp.path().join("policy.cbor");
    fs::write(&file, policy.encode()).expect("write the policy");
    let out = w.ok(&[
        "artifact",
        file.to_str().expect("UTF-8"),
        "--name",
        "main",
        "--media-type",
        LINE_MEDIA_TYPE,
        "--as",
        AS,
    ]);
    let version = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    w.ok(&["relate", &version, "supersedes", &line, "--as", AS]);
    w.sync(&w.ground, &other);

    // Each moves the line without seeing the other's move.
    w.commit("a.txt", b"alice\n");
    let a = w.integrate_on(&w.ground, AS);
    w.commit("a.txt", b"bob\n");
    let b = w.integrate_on(&other, "bob");
    w.sync(&w.ground, &other);
    w.sync(&other, &w.ground);

    let mut expected = vec![a.clone(), b.clone()];
    expected.sort();
    for ground in [&w.ground, &other] {
        assert_eq!(w.heads_on(ground), expected, "every replica shows the same heads, ascending");
        assert_eq!(w.json_on(ground, &["weave", "head", "--json"])["forked"], true);
    }
    assert!(w.ok(&["weave", "head"]).contains("forked, 2 heads"));
    let err = w.fails(&["weave", "verify", "--git", "HEAD"]);
    assert!(err.contains("is forked"), "{err}");
    let err = w.fails(&["weave", "status"]);
    assert!(err.contains("is forked"), "{err}");
    assert!(w.ok(&["weave", "status", "--at", &a]).contains("modified a.txt"));

    // An integrate lists every head, so it settles the fork.
    w.commit("a.txt", b"merged\n");
    let merge = w.json(&["weave", "integrate", "--as", AS, "--json"]);
    assert_eq!(merge["parents"], serde_json::json!(expected));
    let merge = merge["decision"].as_str().expect("a decision").to_owned();
    assert_eq!(w.heads_on(&w.ground), [merge.as_str()]);
    w.sync(&w.ground, &other);
    assert_eq!(w.heads_on(&other), [merge]);
    let log = w.json(&["weave", "log", "--json"]);
    assert_eq!(log["decisions"].as_array().expect("decisions").len(), 4);
    assert_eq!(log["decisions"][3]["decision"], first);
}

#[test]
fn log_prints_twenty_decisions_unless_told_otherwise() {
    let w = Weave::new();
    let manifest = {
        let out = w.json(&["weave", "integrate", "--as", AS, "--json"]);
        out["manifest"].as_str().expect("a hash").to_owned()
    };
    for _ in 0..24 {
        w.ok(&["weave", "integrate", "--as", AS, "--manifest", &manifest]);
    }
    let text = w.ok(&["weave", "log"]);
    assert_eq!(text.lines().filter(|l| l.starts_with("decision ")).count(), 20);
    assert!(text.ends_with("(5 more; raise --limit, or read on with --since)\n"), "{text}");
    assert!(!text.contains("one\n"), "no payload is printed");
    let json = w.json(&["weave", "log", "--json"]);
    assert_eq!(json["decisions"].as_array().expect("decisions").len(), 20);
    assert_eq!(json["more"], true);
    let all = w.json(&["weave", "log", "--json", "--limit", "100"]);
    let all = all["decisions"].as_array().expect("decisions");
    assert_eq!(all.len(), 25);
    // --since reads on from a cursor: what is newer than the tenth newest.
    let cursor = all[9]["decision"].as_str().expect("an id");
    let since = w.json(&["weave", "log", "--json", "--since", cursor]);
    let since: Vec<&Value> = since["decisions"].as_array().expect("decisions").iter().collect();
    assert_eq!(since.len(), 9);
    assert_eq!(since[0]["decision"], all[0]["decision"]);
}
