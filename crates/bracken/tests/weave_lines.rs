//! `bracken weave`, lines and decisions: `integrate` of a given tree, `head`,
//! `log`, `status`, `diff` and `verify --at` (docs/design/weave.md, section 3;
//! build step 5); and line policies, proposals, merges and resolutions:
//! `line create`, `line policy`, `line merge`, `propose`, `integrate
//! --proposals` and `resolve` (step 6). Every test uses throwaway Grounds: never this repository's
//! `.bracken/`, and never a real Weave Ground.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use bracken_tree::ContentHash;
use bracken_tree::format::{DECISION_MEDIA_TYPE, Decision, Guardrails, LINE_MEDIA_TYPE, Line, Record, VERSION};
use serde_json::{Value, json};
use tempfile::TempDir;

const AS: &str = "weave-integrator";

/// An owner decision's id, as the coordination Ground would hold it. The binary checks only its shape;
/// scripts/weave looks it up.
const OD: &str = "0123456789abcdef";

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
        owner_decision: Some("decision 0123456789ab".into()),
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

impl Weave {
    fn id_of(&self, args: &[&str], field: &str) -> String {
        self.json(args)[field].as_str().unwrap_or_else(|| panic!("{args:?}: no {field}")).to_owned()
    }

    /// Stages the whole work tree.
    fn stage(&self) {
        self.git(&["add", "-A"]);
    }
}

fn hash(bytes: &[u8]) -> String {
    ContentHash::of(bytes).to_string()
}

#[test]
fn a_line_is_created_and_changed_only_with_an_owner_decision_and_by_its_author() {
    let w = Weave::new();
    let first = w.integrate_on(&w.ground, AS);
    // A version without an owner decision would count for nothing, so the binary asks for one too.
    let err = w.fails(&["weave", "line", "create", "dev", "--as", AS]);
    assert!(err.contains("--owner-decision"), "{err}");
    for cited in [" ", "TODO", "0123", "0123456789AB"] {
        let err = w.fails(&["weave", "line", "create", "dev", "--owner-decision", cited, "--as", AS]);
        assert!(err.contains("12 to 64 lowercase hex digits"), "{cited:?}: {err}");
    }

    // A dry run shows the policy and writes nothing.
    let before = w.contributions();
    let dry = w.json(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS, "--dry-run", "--json"]);
    assert_eq!((dry["line"].clone(), dry["name"].as_str()), (Value::Null, Some("dev")));
    assert_eq!(dry["integrators"].as_array().map(Vec::len), Some(1), "the author alone");
    let text = w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS, "--dry-run"]);
    assert!(text.starts_with("line dev (dry run, nothing written)"), "{text}");
    assert_eq!(w.contributions(), before);

    let out = w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS]);
    let dev = out.trim().strip_prefix("line ").and_then(|l| l.strip_suffix(" (dev)")).expect("labelled").to_owned();
    let head = w.json(&["weave", "head", "--line", "dev", "--json"]);
    assert_eq!((head["line"].as_str(), head["heads"].as_array().map(Vec::len)), (Some(dev.as_str()), Some(0)));

    // Only the line's author changes its policy.
    w.ok(&["id", "new", "bob"]);
    let before = w.contributions();
    let err =
        w.fails(&["weave", "line", "policy", "dev", "--integrator", "bob", "--owner-decision", OD, "--as", "bob"]);
    assert!(err.contains("only the line's author"), "{err}");
    assert_eq!(w.contributions(), before);
    let changed = w.json(&[
        "weave",
        "line",
        "policy",
        "dev",
        "--integrator",
        "bob",
        "--check",
        "test=cargo test --workspace",
        "--owner-decision",
        OD,
        "--as",
        AS,
        "--json",
    ]);
    assert_eq!(changed["line"], dev);
    assert_eq!(changed["relations"][0]["supersedes"], dev);
    assert_eq!(w.contributions(), before + 2, "a version and its supersedes relation");

    // Under the new version bob integrates on dev: a line with no decision starts from another's head.
    let merged = w.json(&["weave", "line", "merge", "main", "--into", "dev", "--as", "bob", "--json"]);
    assert_eq!(merged["parents"], json!([first]));
    assert_eq!(w.heads_on(&w.ground), [first.as_str()], "main hasn't moved");
    let err = w.fails(&["weave", "line", "merge", "main", "--into", "dev", "--as", "bob"]);
    assert!(err.contains("nothing to merge"), "{err}");
    assert_eq!(w.ok(&["weave", "check"]).trim(), "no problems");

    // A version written by hand without an owner decision is listed by check and counts for nothing.
    let alice = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    let rogue = Line {
        v: VERSION,
        name: "rogue".into(),
        integrators: vec![alice.parse().expect("a key")],
        validators: vec![],
        goal_setters: vec![],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: None,
    };
    let file = w.tmp.path().join("rogue.cbor");
    fs::write(&file, rogue.encode()).expect("write the policy");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", LINE_MEDIA_TYPE, "--as", AS]);
    let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let check = w.run_on(&w.ground, &["weave", "check", "--json"]);
    assert!(!check.status.success());
    let check: Value = serde_json::from_slice(&check.stdout).expect("JSON");
    assert_eq!(check["problems"][0]["problem"], "no_owner_decision");
    assert_eq!(check["problems"][0]["id"], id);
    let err = w.fails(&["weave", "head", "--line", "rogue"]);
    assert!(err.contains("no line is named"), "{err}");
}

#[test]
fn proposals_merge_in_order_and_a_conflict_waits_for_a_resolution() {
    let w = Weave::new();
    let base = w.integrate_on(&w.ground, AS);

    // A proposal adding b.txt; a dry run writes nothing.
    w.commit("b.txt", b"b\n");
    let before = w.contributions();
    let dry = w.json(&["weave", "propose", "--dry-run", "--json"]);
    assert_eq!(dry["files"], json!([{"path": "b.txt", "change": "added"}]));
    assert_eq!(dry["base"], base);
    assert_eq!(w.contributions(), before);
    let out = w.ok(&["weave", "propose", "--summary", "add b", "--as", AS]);
    let p1 = out.lines().last().and_then(|l| l.strip_prefix("proposal ")).expect("the proposal last").to_owned();
    let err = w.fails(&["weave", "propose", "--from-git", "HEAD~1", "--as", AS]);
    assert!(err.contains("there is nothing to propose"), "{err}");

    // Two proposals from the same base, changing a.txt each its own way.
    w.git(&["rm", "-q", "b.txt"]);
    w.write("a.txt", b"two\n");
    w.stage();
    let p2 = w.id_of(&["weave", "propose", "--index", "--base", &base, "--as", AS, "--json"], "proposal");
    w.write("a.txt", b"three\n");
    w.stage();
    let p3 = w.id_of(&["weave", "propose", "--index", "--as", AS, "--json"], "proposal");

    // The first two merge cleanly, in order.
    let d = w.json(&["weave", "integrate", "--proposals", &p1, &p2, "--as", AS, "--json"]);
    assert_eq!(d["proposals"], json!([p1, p2]));
    assert_eq!(d["resolutions"], json!([]));
    let d = d["decision"].as_str().expect("a decision").to_owned();
    let diff = w.json(&["weave", "diff", &base, &d, "--json"]);
    let changed: Vec<(&str, &str)> = diff["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["path"].as_str().expect("path"), f["change"].as_str().expect("change")))
        .collect();
    assert_eq!(changed, [("a.txt", "modified"), ("b.txt", "added")]);

    // The third changed a.txt too, from its base: a conflict, listed, and nothing is written.
    let before = w.contributions();
    let out = w.run_on(&w.ground, &["weave", "integrate", "--proposals", &p3, "--as", AS, "--json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("1 conflicts have no resolution"));
    let listed: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let conflict = &listed["conflicts"][0];
    assert_eq!(conflict["path"], "a.txt");
    assert_eq!(conflict["base"]["hash"], hash(b"one\n"));
    assert_eq!(conflict["into"]["hash"], hash(b"two\n"));
    assert_eq!(conflict["from"]["hash"], hash(b"three\n"));
    let text = w.run_on(&w.ground, &["weave", "integrate", "--proposals", &p3, "--as", AS]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("conflict a.txt: ") && text.contains("weave resolve <path> --line"), "{text}");
    assert_eq!(w.contributions(), before);

    // resolve rebuilds the same merge from the same selector; --dry-run writes nothing.
    let dry = w.json(&["weave", "resolve", "a.txt", "--proposals", &p3, "--take", "from", "--dry-run", "--json"]);
    assert_eq!(dry["result"]["hash"], hash(b"three\n"));
    assert_eq!(w.contributions(), before);
    let r = w.id_of(
        &["weave", "resolve", "a.txt", "--proposals", &p3, "--take", "from", "--as", AS, "--json"],
        "resolution",
    );
    // Anyone may add another resolution, but a merge applies on its own only an integrator's.
    w.ok(&["id", "new", "eve"]);
    let offered = w.id_of(
        &["weave", "resolve", "a.txt", "--proposals", &p3, "--take", "into", "--as", "eve", "--json"],
        "resolution",
    );
    assert_ne!(offered, r);
    let err = w.fails(&["weave", "resolve", "b.txt", "--proposals", &p3, "--take", "into", "--as", AS]);
    assert!(err.contains("no conflict at \"b.txt\""), "{err}");

    let done = w.json(&["weave", "integrate", "--proposals", &p3, "--as", AS, "--json"]);
    assert_eq!(done["resolutions"], json!([r]));
    assert_eq!(done["used"][0]["author_name"], AS);
    assert_eq!(done["used"][0]["result"]["hash"], hash(b"three\n"));
    assert_eq!(done["parents"], json!([d]));
    let log = w.json(&["weave", "log", "--json"]);
    assert_eq!(log["decisions"][0]["proposals"], json!([p3]));
    assert_eq!(log["decisions"][0]["resolutions"], json!([r]));
    let diff = w.json(&["weave", "diff", &d, done["decision"].as_str().expect("id"), "--json"]);
    assert_eq!(diff["files"][0]["after"]["hash"], hash(b"three\n"));

    // Records named by the wrong kind are refused.
    let err = w.fails(&["weave", "integrate", "--proposals", &d, "--as", AS]);
    assert!(err.contains("is a decision, not a proposal"), "{err}");
}

/// Two lines, main and dev, where dev branched from main's first decision; each then moves.
fn branched(w: &Weave) -> (String, String) {
    w.integrate_on(&w.ground, AS);
    w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS]);
    w.ok(&["weave", "line", "merge", "main", "--into", "dev", "--as", AS]);
    w.commit("a.txt", b"two\n");
    let m2 = w.integrate_on(&w.ground, AS);
    // dev makes a.txt executable, without changing its bytes.
    use std::os::unix::fs::PermissionsExt;
    w.write("a.txt", b"one\n");
    fs::set_permissions(w.repo.join("a.txt"), fs::Permissions::from_mode(0o755)).expect("chmod");
    w.stage();
    w.git(&["commit", "-q", "-m", "exec"]);
    let d2 = w.id_of(&["weave", "integrate", "--line", "dev", "--as", AS, "--json"], "decision");
    (m2, d2)
}

#[test]
fn line_merge_lists_a_mode_conflict_and_a_resolution_settles_it() {
    let w = Weave::new();
    let (m2, d2) = branched(&w);

    // The content change on main against the mode change on dev is a conflict.
    let before = w.contributions();
    let out = w.run_on(&w.ground, &["weave", "line", "merge", "dev", "--into", "main", "--dry-run", "--json"]);
    assert!(!out.status.success());
    let listed: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let c = &listed["conflicts"][0];
    assert_eq!(
        (c["path"].as_str(), c["into"]["mode"].as_str(), c["from"]["mode"].as_str()),
        (Some("a.txt"), Some("100644"), Some("100755"))
    );
    assert_eq!(c["into"]["hash"], hash(b"two\n"));
    assert_eq!(w.contributions(), before);
    let err = w.fails(&["weave", "line", "merge", "dev", "--into", "main", "--as", AS]);
    assert!(err.contains("1 conflicts have no resolution, so nothing was written"), "{err}");
    assert_eq!(w.contributions(), before);

    // --base only chooses among merge bases; with one, it must be that one.
    let err = w.fails(&["weave", "line", "merge", "dev", "--into", "main", "--base", &d2, "--dry-run"]);
    assert!(err.contains("isn't a merge base of the heads"), "{err}");

    // A resolution by someone who isn't an integrator of main is only offered.
    w.ok(&["id", "new", "eve"]);
    let offered = w.id_of(
        &["weave", "resolve", "a.txt", "--into", "main", "--from", "dev", "--take", "from", "--as", "eve", "--json"],
        "resolution",
    );
    let out = w.run_on(&w.ground, &["weave", "line", "merge", "dev", "--into", "main", "--dry-run", "--json"]);
    assert!(!out.status.success(), "eve's resolution isn't applied");
    let listed: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(listed["conflicts"][0]["offered"][0]["resolution"], offered);
    assert_eq!(listed["conflicts"][0]["offered"][0]["author_name"], "eve");
    let text = w.run_on(&w.ground, &["weave", "line", "merge", "dev", "--into", "main", "--dry-run"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&format!("offered: resolution {offered} by eve")), "{text}");
    // An integrator adopts it by writing their own, which keeps its result.
    let adopted = w.json(&[
        "weave",
        "resolve",
        "a.txt",
        "--into",
        "main",
        "--from",
        "dev",
        "--take",
        &offered,
        "--dry-run",
        "--json",
    ]);
    assert_eq!(adopted["result"], json!({"hash": hash(b"one\n"), "mode": "100755"}));
    let err = w.fails(&["weave", "resolve", "a.txt", "--into", "main", "--from", "dev", "--take", &m2]);
    assert!(err.contains("is a decision, not a resolution"), "{err}");

    // Keep both: main's bytes, dev's mode.
    let r = w.id_of(
        &[
            "weave",
            "resolve",
            "a.txt",
            "--into",
            "main",
            "--from",
            "dev",
            "--take",
            "into",
            "--mode",
            "executable",
            "--as",
            AS,
            "--json",
        ],
        "resolution",
    );
    let dry = w.json(&["weave", "line", "merge", "dev", "--into", "main", "--dry-run", "--json"]);
    assert_eq!((dry["resolutions"].clone(), dry["decision"].clone()), (json!([r]), Value::Null));
    let text = w.ok(&["weave", "line", "merge", "dev", "--into", "main", "--dry-run"]);
    assert!(text.contains(&format!("uses resolution {r} by {AS}: a.txt {} 100755", hash(b"two\n"))), "{text}");
    let merged = w.json(&["weave", "line", "merge", "dev", "--into", "main", "--note", "dev in", "--as", AS, "--json"]);
    assert_eq!(merged["used"][0]["resolution"], r);
    let mut parents = vec![m2.clone(), d2];
    parents.sort();
    assert_eq!(merged["parents"], json!(parents));
    assert_eq!(merged["resolutions"], json!([r]));
    let diff = w.json(&["weave", "diff", &m2, merged["decision"].as_str().expect("id"), "--json"]);
    assert_eq!(
        diff["files"],
        json!([{"path": "a.txt", "change": "mode changed",
        "before": {"hash": hash(b"two\n"), "mode": "100644"}, "after": {"hash": hash(b"two\n"), "mode": "100755"},
        "not_held": []}])
    );
    assert_eq!(w.heads_on(&w.ground), [merged["decision"].as_str().expect("id")]);
}

#[test]
fn resolve_file_refuses_secrets_and_files_outside_the_checkout() {
    let w = Weave::new();
    branched(&w);
    let pem = w.repo.join("pem.txt");
    // Written so this file holds no header itself: importing this repository would refuse it.
    fs::write(&pem, format!("-----BEGIN {}PRIVATE KEY-----\nMIIB\n", "RSA ")).expect("write");
    let key = w.repo.join("conf.json");
    fs::write(&key, format!("{{\"secret_key\": \"{}\"}}\n", "ab".repeat(32))).expect("write");
    let env = w.repo.join(".env");
    fs::write(&env, "TOKEN=1\n").expect("write");
    let outside = w.tmp.path().join("merged.txt");
    fs::write(&outside, "merged\n").expect("write");
    let link = w.repo.join("link.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let git = w.repo.join(".git/description");
    let before = w.contributions();
    for (file, expected) in [
        (&pem, "PEM private-key header"),
        (&key, "secret_key field"),
        (&env, ".env"),
        (&outside, "outside the checkout"),
        (&link, "outside the checkout"),
        (&git, "inside a .git directory"),
    ] {
        let file = file.to_str().expect("UTF-8");
        let args = ["weave", "resolve", "a.txt", "--into", "main", "--from", "dev", "--file", file, "--as", AS];
        let err = w.fails(&args);
        assert!(err.contains(expected), "{file}: expected {expected:?} in {err}");
        let mut dry = args.to_vec();
        dry.push("--dry-run");
        assert!(w.fails(&dry).contains(expected), "{file}: a dry run refuses too");
    }
    assert_eq!(w.contributions(), before);

    // A file inside the checkout is taken, with its executable bit unless --mode says otherwise.
    let good = w.repo.join("merged.txt");
    fs::write(&good, "merged\n").expect("write");
    let args = ["weave", "resolve", "a.txt", "--into", "main", "--from", "dev", "--file", "merged.txt"];
    let dry = w.json(&[&args[..], &["--dry-run", "--json"]].concat());
    assert_eq!(dry["result"], json!({"hash": hash(b"merged\n"), "mode": "100644"}));
    assert_eq!(dry["file_new"], true);
    let out = w.ok(&[&args[..], &["--mode", "executable", "--as", AS]].concat());
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(lines[0], format!("file {} a.txt", hash(b"merged\n")));
    assert!(lines[1].starts_with("resolution "));
    assert_eq!(w.contributions(), before + 2);
    w.ok(&["weave", "line", "merge", "dev", "--into", "main", "--as", AS]);
}

#[test]
fn a_criss_cross_has_two_merge_bases_and_needs_base() {
    let w = Weave::new();
    let (m2, d2) = branched(&w);
    // A second replica, whose integrator the dev line's author adds.
    let bundle = w.tmp.path().join("bundle");
    w.ok(&["export", bundle.to_str().expect("UTF-8")]);
    let other = w.tmp.path().join("other");
    w.ok_on(&other, &["join", bundle.to_str().expect("UTF-8")]);
    let bob = w.ok_on(&other, &["id", "new", "bob"]).trim().to_owned();
    w.ok(&["weave", "line", "policy", "dev", "--integrator", &bob, "--owner-decision", OD, "--as", AS]);
    w.sync(&w.ground, &other);

    // Each merges the other line at the same time.
    w.ok(&[
        "weave",
        "resolve",
        "a.txt",
        "--into",
        "main",
        "--from",
        "dev",
        "--take",
        "into",
        "--mode",
        "executable",
        "--as",
        AS,
    ]);
    w.sync(&w.ground, &other);
    let m3 = w.id_of(&["weave", "line", "merge", "dev", "--into", "main", "--as", AS, "--json"], "decision");
    let d3 = w.json_on(&other, &["weave", "line", "merge", "main", "--into", "dev", "--as", "bob", "--json"]);
    let d3 = d3["decision"].as_str().expect("a decision").to_owned();
    w.sync(&other, &w.ground);

    let mut bases = vec![m2.clone(), d2.clone()];
    bases.sort();
    let err = w.fails(&["weave", "line", "merge", "dev", "--into", "main", "--dry-run"]);
    assert!(err.contains("2 merge bases (a criss-cross)"), "{err}");
    for b in &bases {
        assert!(err.contains(&b[..12]), "{err}");
    }
    let err = w.fails(&["weave", "resolve", "a.txt", "--into", "main", "--from", "dev", "--delete"]);
    assert!(err.contains("criss-cross"), "{err}");
    let err = w.fails(&["weave", "line", "merge", "dev", "--into", "main", "--base", &m3, "--dry-run"]);
    assert!(err.contains("isn't a merge base of the heads"), "{err}");
    let dry = w.json(&["weave", "line", "merge", "dev", "--into", "main", "--base", &m2, "--dry-run", "--json"]);
    assert_eq!(dry["merge"]["base"], m2);
    let mut parents = vec![m3, d3];
    parents.sort();
    assert_eq!(dry["parents"], json!(parents));
}

/// Finding 90de879d935c: the head and two proposals all change a.txt, so integrating both conflicts at
/// a.txt twice, in successive steps. resolve finds the first conflict there that no resolution settles.
#[test]
fn resolve_reaches_a_later_steps_conflict_at_the_same_path() {
    let w = Weave::new();
    let base = w.integrate_on(&w.ground, AS);
    w.write("a.txt", b"x\n");
    w.stage();
    let p1 = w.id_of(&["weave", "propose", "--index", "--as", AS, "--json"], "proposal");
    w.write("a.txt", b"y\n");
    w.stage();
    let p2 = w.id_of(&["weave", "propose", "--index", "--base", &base, "--as", AS, "--json"], "proposal");
    w.commit("a.txt", b"h\n");
    w.integrate_on(&w.ground, AS);

    let take = |side: &str| -> String {
        w.id_of(
            &["weave", "resolve", "a.txt", "--proposals", &p1, &p2, "--take", side, "--as", AS, "--json"],
            "resolution",
        )
    };
    // Step 1: the head's h against P1's x, from one.
    let dry = w.json(&["weave", "resolve", "a.txt", "--proposals", &p1, &p2, "--take", "from", "--dry-run", "--json"]);
    assert_eq!(dry["merging"], format!("proposal {}", &p1[..12]));
    let r1 = take("from");
    // Step 2: then x against P2's y, from one.
    let dry = w.json(&["weave", "resolve", "a.txt", "--proposals", &p1, &p2, "--take", "from", "--dry-run", "--json"]);
    assert_eq!(dry["merging"], format!("proposal {}", &p2[..12]));
    assert_eq!(
        dry["sides"],
        json!([{"hash": hash(b"x\n"), "mode": "100644"}, {"hash": hash(b"y\n"), "mode": "100644"}])
    );
    let r2 = take("from");

    let done = w.json(&["weave", "integrate", "--proposals", &p1, &p2, "--as", AS, "--json"]);
    let mut used = vec![r1, r2];
    used.sort();
    assert_eq!(done["resolutions"], json!(used));
    let diff = w.json(&["weave", "diff", &base, done["decision"].as_str().expect("id"), "--json"]);
    assert_eq!(diff["files"][0]["after"]["hash"], hash(b"y\n"));
}

/// Finding 4eead6290997: on a diverged chain, line policy starts only from the version --from names, and
/// supersedes every current one.
#[test]
fn line_policy_on_a_diverged_chain_needs_from() {
    let w = Weave::new();
    w.ok(&["id", "new", "bob"]);
    let dev = w.id_of(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS, "--json"], "line");
    let v1 = w.id_of(
        &["weave", "line", "policy", "dev", "--integrator", "bob", "--owner-decision", OD, "--as", AS, "--json"],
        "version",
    );
    // A second successor of the first version, written by hand, diverges the chain.
    let alice = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    let v2 = Line {
        v: VERSION,
        name: "dev".into(),
        integrators: vec![alice.parse().expect("a key")],
        validators: vec![alice.parse().expect("a key")],
        goal_setters: vec![alice.parse().expect("a key")],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: Some(OD.into()),
    };
    let file = w.tmp.path().join("v2.cbor");
    fs::write(&file, v2.encode()).expect("write the policy");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", LINE_MEDIA_TYPE, "--as", AS]);
    let v2 = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    w.ok(&["relate", &v2, "supersedes", &dev, "--as", AS]);

    let before = w.contributions();
    let err = w.fails(&["weave", "line", "policy", "dev", "--owner-decision", OD, "--as", AS]);
    assert!(err.contains("has diverged into 2") && err.contains("--from"), "{err}");
    assert!(err.contains(&v1[..12]) && err.contains(&v2[..12]), "{err}");
    let err = w.fails(&["weave", "line", "policy", "dev", "--from", &dev, "--owner-decision", OD, "--as", AS]);
    assert!(err.contains("isn't a current version"), "{err}");
    assert_eq!(w.contributions(), before);

    let settled =
        w.json(&["weave", "line", "policy", "dev", "--from", &v1, "--owner-decision", OD, "--as", AS, "--json"]);
    let mut superseded: Vec<&str> = settled["relations"]
        .as_array()
        .expect("relations")
        .iter()
        .map(|r| r["supersedes"].as_str().expect("id"))
        .collect();
    superseded.sort();
    let mut expected = vec![v1.as_str(), v2.as_str()];
    expected.sort();
    assert_eq!(superseded, expected);
    // v1's integrators carried on: bob can integrate on dev.
    w.ok(&["weave", "integrate", "--as", AS]);
    w.ok(&["weave", "line", "merge", "main", "--into", "dev", "--as", "bob"]);
}

// Rule 5 at integrate: until --answer (Weave step 7, PR 4), an objection in the window stops both
// forms of integrate before anything is written, and the refusal names the objection, its basis and
// the rule.
#[test]
fn integrate_refuses_while_an_objection_is_in_the_window() {
    let w = Weave::new();
    let first = w.json(&["weave", "integrate", "--as", AS, "--json"]);
    let decision = first["decision"].as_str().expect("a decision");
    let claim = w.ok(&["claim", "this decision breaks the build", "--as", AS]);
    let claim = claim.split_whitespace().nth(1).expect("an id");
    let objection: Value =
        serde_json::from_str(&w.ok(&["relate", claim, "contradicts", decision, "--as", AS, "--json"])).expect("JSON");
    let objection = objection["id"].as_str().expect("an id").to_owned();
    w.commit("b.txt", b"two\n");
    let proposal = w.ok(&["weave", "propose", "--as", AS]);
    let proposal = proposal.lines().find_map(|l| l.strip_prefix("proposal ")).expect("a proposal").to_owned();
    let before = w.contributions();
    for args in [&["weave", "integrate", "--as", AS][..], &["weave", "integrate", "--proposals", &proposal, "--as", AS]]
    {
        let err = w.fails(args);
        assert!(
            err.contains(&format!(
                "it doesn't answer the discussion objection {objection}, nor does a decision in its window"
            )),
            "{args:?}: {err}"
        );
        assert!(err.contains("(rule 5:") && err.contains("--answer"), "{args:?}: {err}");
        assert_eq!(w.contributions(), before, "{args:?} wrote something");
    }
}

impl Weave {
    /// Runs a command that must succeed with `BRACKEN_WEAVE_STATS` set, and returns each build's
    /// (decisions judged, questions asked), in order.
    fn stats(&self, args: &[&str]) -> Vec<(usize, u64)> {
        let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(&self.repo)
            .env("BRACKEN_DIR", &self.ground)
            .env("BRACKEN_WEAVE_STATS", "1")
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("bracken runs");
        let stderr = String::from_utf8(out.stderr).expect("UTF-8");
        assert!(out.status.success(), "bracken {args:?} failed: {stderr}");
        let builds: Vec<(usize, u64)> = stderr
            .lines()
            .filter_map(|l| l.strip_prefix("weave stats: "))
            .map(|l| {
                let (judged, rest) = l.split_once(" decisions judged, ").expect("judged");
                let questions = rest.strip_suffix(" questions").expect("questions");
                (judged.parse().expect("a count"), questions.parse().expect("a count"))
            })
            .collect();
        assert!(!builds.is_empty(), "bracken {args:?} printed no stats: {stderr}");
        builds
    }
}

/// Task a4d1a06a1072: a line anyone starts, here `stray`, whose moves are written by hand and each
/// objected to and left unanswered, costs a command that doesn't name it nothing: reading main, checking
/// the Ground, merging main into another line and moving main judge what they judged before it existed.
/// Naming the line still judges all of it.
#[test]
fn a_line_nobody_names_costs_main_nothing_and_naming_it_judges_it() {
    const N: usize = 40;
    let w = Weave::new();
    let first = w.json(&["weave", "integrate", "--as", AS, "--json"]);
    let (first, manifest) = (first["decision"].as_str().expect("an id"), first["manifest"].as_str().expect("a hash"));
    w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS]);
    let out = w.ok(&["weave", "line", "create", "stray", "--owner-decision", OD, "--as", AS]);
    let stray_line = out.trim().strip_prefix("line ").and_then(|l| l.strip_suffix(" (stray)")).expect("labelled");
    let reads: [&[&str]; 3] = [
        &["weave", "head"],
        &["weave", "check"],
        &["weave", "line", "merge", "main", "--into", "dev", "--dry-run", "--as", AS],
    ];
    let before: Vec<_> = reads.iter().map(|args| w.stats(args)).collect();

    let file = w.tmp.path().join("decision.cbor");
    let mut stray: Vec<String> = Vec::new();
    for i in 0..N {
        let parent = stray.last().map_or(first, String::as_str);
        let decision = Decision {
            v: VERSION,
            line: stray_line.parse().expect("an id"),
            manifest: manifest.parse().expect("a hash"),
            parents: vec![parent.parse().expect("an id")],
            proposals: vec![],
            breakage: vec![],
            answers: vec![],
            resolutions: vec![],
            goals: vec![],
            source: None,
            note: format!("stray {i}"),
        };
        fs::write(&file, decision.encode()).expect("write the decision");
        let out = w.ok(&[
            "artifact",
            file.to_str().expect("UTF-8"),
            "--name",
            "decision",
            "--media-type",
            DECISION_MEDIA_TYPE,
            "--as",
            AS,
        ]);
        let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
        if i + 1 < N {
            let claim = w.ok(&["claim", "this move is wrong", "--as", AS]);
            let claim = claim.split_whitespace().nth(1).expect("an id");
            w.ok(&["relate", claim, "contradicts", &id, "--as", AS]);
        }
        stray.push(id);
    }

    for (args, before) in reads.iter().zip(&before) {
        assert_eq!(&w.stats(args), before, "bracken {args:?} pays for the stray line");
    }
    w.commit("a.txt", b"two\n");
    for (judged, questions) in w.stats(&["weave", "integrate", "--as", AS]) {
        assert!(judged <= 2 && questions == 0, "integrate judged {judged} decisions, asking {questions} questions");
    }

    // Named, by its name or by one of its moves, the stray line is judged whole, every unanswered
    // objection asked about in each later move's window.
    for args in [&["weave", "log", "--line", "stray"][..], &["weave", "status", "--at", &stray[N - 1]]] {
        for (judged, questions) in w.stats(args) {
            assert!(judged > N, "bracken {args:?} judged {judged} decisions");
            assert!(questions >= (N * (N - 1) / 2) as u64, "bracken {args:?} asked {questions} questions");
        }
    }
    let head = w.json(&["weave", "head", "--line", "stray", "--json"]);
    assert_eq!(head["heads"][0]["decision"], stray[0].as_str(), "only the first move qualifies");
}
