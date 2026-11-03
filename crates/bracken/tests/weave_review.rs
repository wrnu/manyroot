//! `bracken weave green`, `weave review`, and `integrate --answer`, `--breakage` and `--goal`
//! (docs/design/weave.md, sections 2 and 3; build step 7), end to end through the binary. Every test
//! uses a throwaway Ground: never this repository's `.bracken/`, and never a real Weave Ground.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use bracken_tree::ContentHash;
use bracken_tree::check::{Exit, RecordedTail};
use bracken_tree::results::details;
use serde_json::Value;
use tempfile::TempDir;

const AS: &str = "weave-integrator";
const VAL: &str = "val";
const OD: &str = "0123456789abcdef";

struct Weave {
    _tmp: TempDir,
    ground: PathBuf,
    repo: PathBuf,
}

impl Weave {
    /// A Weave Ground with a validator `val` and one check, `test`; a git repository with one commit; and
    /// a first decision on it. Breakage up to two entries, two moves old, and never under `secrets`.
    fn new() -> Weave {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let (ground, repo) = (tmp.path().join("g"), tmp.path().join("repo"));
        fs::create_dir(&repo).expect("mkdir repo");
        let w = Weave { _tmp: tmp, ground, repo };
        w.git(&["init", "-q", "-b", "main"]);
        w.ok(&["weave", "init", "--as", AS]);
        w.ok(&["id", "new", VAL]);
        w.ok(&[
            "weave",
            "line",
            "policy",
            "main",
            "--validator",
            VAL,
            "--check",
            "test=true",
            "--max-known",
            "2",
            "--max-age",
            "2",
            "--protected",
            "secrets",
            "--owner-decision",
            OD,
            "--as",
            AS,
        ]);
        w.commit("a.txt", b"one\n");
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

    /// Runs a command that must fail, and returns its stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
        String::from_utf8(out.stderr).expect("UTF-8")
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).expect("JSON")
    }

    /// The id a write printed with --json.
    fn id(&self, args: &[&str]) -> String {
        let out = self.json(args);
        let id = out.get("id").or_else(|| out["records"][0].get("id")).unwrap_or(&out["contributions"][0]["id"]);
        id.as_str().unwrap_or_else(|| panic!("an id in {out}")).to_owned()
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

    fn commit(&self, path: &str, bytes: &[u8]) {
        fs::write(self.repo.join(path), bytes).expect("write");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "c"]);
    }

    /// `integrate` of git's HEAD, with `extra` flags: (decision, manifest hash, manifest artifact).
    fn integrate(&self, extra: &[&str]) -> (String, String, String) {
        let mut args = vec!["weave", "integrate", "--as", AS, "--json"];
        args.extend(extra);
        let out = self.json(&args);
        let s = |k: &str| out[k].as_str().unwrap_or_else(|| panic!("{k} in {out}")).to_owned();
        // The manifest artifact, if this integrate wrote it.
        (s("decision"), s("manifest"), out["manifest_id"].as_str().unwrap_or_default().to_owned())
    }

    /// A result for `test` by `val` on a manifest artifact.
    fn result(&self, method: &str, manifest: (&str, &str), passed: bool, attempt: u64) -> String {
        let hash: ContentHash = manifest.0.parse().expect("a hash");
        let exit = Exit::Code(if passed { 0 } else { 1 });
        let details = details(&hash, attempt, &["true".to_owned()], &exit, &RecordedTail::Text("out".into()));
        let verdict = if passed { "pass" } else { "fail" };
        self.id(&["evidence", manifest.1, verdict, "--method", method, "--details", &details, "--as", VAL, "--json"])
    }

    fn method(&self) -> String {
        let green = self.json(&["weave", "green", "--json"]);
        green["heads"][0]["checks"][0]["method"].as_str().expect("the check's method").to_owned()
    }
}

/// `weave green` names the green decision, and per check every counted result with its attempt,
/// including a fail a later attempt replaced; text lines are labelled.
#[test]
fn green_shows_every_attempt() {
    let w = Weave::new();
    let (d1, hash, artifact) = w.integrate(&[]);
    let method = w.method();
    let none = w.json(&["weave", "green", "--json"]);
    assert_eq!(none["state"], "none");
    let fail = w.result(&method, (&hash, &artifact), false, 1);
    let pass = w.result(&method, (&hash, &artifact), true, 2);
    let green = w.json(&["weave", "green", "--json"]);
    assert_eq!(green["state"], "one", "{green}");
    assert_eq!(green["green"][0], d1.as_str());
    let check = &green["heads"][0]["checks"][0];
    assert_eq!(check["passes"][0]["result"], pass.as_str());
    assert_eq!(check["passes"][0]["attempt"], 2);
    assert_eq!(check["replaced"][0]["result"], fail.as_str(), "{check}");
    assert_eq!(check["replaced"][0]["attempt"], 1);
    let text = w.ok(&["weave", "green"]);
    assert!(text.starts_with(&format!("green {d1}\n")), "{text}");
    assert!(text.contains("attempt 2") && text.contains("fails a later attempt replaced"), "{text}");
}

/// `weave green --manifest`: the counted results on any manifest the replica holds, not only a head's, as
/// scripts/merge reads a candidate's before git merges (plan cc496dcee0e3 step 1b). A validator's latest
/// counts, a fail it replaced is listed, the integrator's own result is left out, and a manifest the
/// replica doesn't hold is refused.
#[test]
fn green_on_a_manifest_counts_its_results_as_for_a_head() {
    let w = Weave::new();
    w.integrate(&[]);
    let method = w.method();
    // A candidate: imported, never integrated.
    w.commit("b.txt", b"two\n");
    let imported = w.json(&["weave", "import", "--as", AS, "--json"]);
    let hash = imported["manifest"].as_str().unwrap_or_else(|| panic!("a manifest in {imported}")).to_owned();
    let artifact = imported["manifest_id"].as_str().unwrap_or_else(|| panic!("its id in {imported}")).to_owned();
    let on = |w: &Weave| w.json(&["weave", "green", "--manifest", &hash, "--json"]);

    let none = on(&w);
    assert_eq!(none["green"], false, "{none}");
    assert_eq!(none["manifest"], hash.as_str());
    assert_eq!(none["checks"][0]["check"], "test");
    assert_eq!(none["checks"][0]["passes"], serde_json::json!([]));

    // The integrator's own pass doesn't count.
    let details = details(
        &hash.parse().expect("a hash"),
        1,
        &["true".to_owned()],
        &Exit::Code(0),
        &RecordedTail::Text("out".into()),
    );
    w.ok(&["evidence", &artifact, "pass", "--method", &method, "--details", &details, "--as", AS]);
    assert_eq!(on(&w)["green"], false);

    let fail = w.result(&method, (&hash, &artifact), false, 1);
    assert_eq!(on(&w)["checks"][0]["fails"][0]["result"], fail.as_str());
    let pass = w.result(&method, (&hash, &artifact), true, 2);
    let green = on(&w);
    assert_eq!(green["green"], true, "{green}");
    assert_eq!(green["checks"][0]["passes"][0]["result"], pass.as_str());
    assert_eq!(green["checks"][0]["replaced"][0]["result"], fail.as_str());
    assert!(green["version"].as_str().is_some_and(|v| v.len() == 64), "{green}");
    let text = w.ok(&["weave", "green", "--manifest", &hash]);
    assert!(text.starts_with(&format!("manifest {hash} under line ")) && text.contains(": green\n"), "{text}");

    // A manifest this replica doesn't hold.
    let missing = format!("blake3:{}", "0".repeat(64));
    let err = w.fails(&["weave", "green", "--manifest", &missing]);
    assert!(err.contains("manifest"), "{err}");
}

/// An evidence-backed objection to the head is open, with the dispositions its basis allows; integrate
/// refuses an answer its basis doesn't allow, naming rule 5, and accepts an allowed one.
#[test]
fn an_answer_is_refused_for_its_basis_and_accepted_when_allowed() {
    let w = Weave::new();
    let (d1, hash, artifact) = w.integrate(&[]);
    let method = w.method();
    let fail = w.result(&method, (&hash, &artifact), false, 1);
    let objection = w.id(&["relate", &fail, "contradicts", &d1, "--note", "test fails", "--as", VAL, "--json"]);
    let review = w.json(&["weave", "review", "--json"]);
    let open = &review["open"][0];
    assert_eq!(open["objection"], objection.as_str(), "{review}");
    assert_eq!(open["basis"]["kind"], "evidence-backed");
    assert_eq!(open["allowed"], serde_json::json!(["fixed", "fell_back", "accepted_breakage"]));
    let text = w.ok(&["weave", "review"]);
    assert!(text.contains(&format!("  open {objection} by {VAL}")), "{text}");
    w.commit("a.txt", b"two\n");
    let unanswered = w.fails(&["weave", "integrate", "--as", AS]);
    assert!(unanswered.contains("rule 5") && unanswered.contains(&objection), "{unanswered}");
    let answer = format!("{objection}=overridden:not convinced");
    let refused = w.fails(&["weave", "integrate", "--as", AS, "--answer", &answer]);
    assert!(refused.contains("rule 5") && refused.contains("overridden"), "{refused}");
    let answer = format!("{objection}=fixed:a.txt is two now");
    let (d2, _, _) = w.integrate(&["--answer", &answer]);
    let log = w.json(&["weave", "log", "--json"]);
    let head =
        log["decisions"].as_array().expect("decisions").iter().find(|d| d["decision"] == d2.as_str()).expect("d2");
    assert_eq!(head["answers"][0]["disposition"], "fixed", "{head}");
    let review = w.json(&["weave", "review", "--json"]);
    assert_eq!(review["open"], serde_json::json!([]), "{review}");
}

/// One answer naming the head answers every discussion objection to it (decision 8cf20fe7669a).
#[test]
fn one_answer_naming_the_target_answers_every_objection_to_it() {
    let w = Weave::new();
    let (d1, _, _) = w.integrate(&[]);
    for i in 0..5 {
        let why = w.id(&["claim", &format!("I don't like it ({i})"), "--as", VAL, "--json"]);
        w.ok(&["relate", &why, "contradicts", &d1, "--as", VAL]);
    }
    let review = w.json(&["weave", "review", "--json"]);
    assert_eq!(review["open"].as_array().expect("open").len(), 5);
    w.commit("a.txt", b"two\n");
    let answer = format!("{d1}=overridden:discussed, and we go ahead");
    w.integrate(&["--answer", &answer]);
    let review = w.json(&["weave", "review", "--json"]);
    assert_eq!(review["open"], serde_json::json!([]), "{review}");
}

/// Breakage naming a protected path is refused (rule 6); breakage within the guardrails is recorded.
#[test]
fn breakage_is_held_to_the_guardrails() {
    let w = Weave::new();
    w.integrate(&[]);
    w.commit("a.txt", b"two\n");
    let refused = w.fails(&["weave", "integrate", "--as", AS, "--breakage", "test:secrets/key"]);
    assert!(refused.contains("rule 6") && refused.contains("secrets/key"), "{refused}");
    let (d2, _, _) = w.integrate(&["--breakage", "test:a.txt"]);
    let log = w.json(&["weave", "log", "--json"]);
    let head =
        log["decisions"].as_array().expect("decisions").iter().find(|d| d["decision"] == d2.as_str()).expect("d2");
    assert_eq!(head["breakage"][0]["check"], "test");
    assert_eq!(head["breakage"][0]["paths"][0], "a.txt");
    let bad = w.fails(&["weave", "integrate", "--as", AS, "--goal", &d2]);
    assert!(bad.contains("weave-goal"), "{bad}");
}

/// Finding 889d899a57b8: --answer refuses an id that is neither an objection nor what objections
/// target, before anything is written.
#[test]
fn an_answer_naming_anything_else_is_refused_before_any_write() {
    let w = Weave::new();
    w.integrate(&[]);
    let claim = w.id(&["claim", "just a claim", "--as", VAL, "--json"]);
    w.commit("a.txt", b"two\n");
    let before = w.json(&["log", "--json", "--limit", "100000"])["contributions"].as_array().expect("a list").len();
    let answer = format!("{claim}=overridden:x");
    let refused = w.fails(&["weave", "integrate", "--as", AS, "--answer", &answer]);
    assert!(refused.contains("neither an objection") && refused.contains("nothing was written"), "{refused}");
    let after = w.json(&["log", "--json", "--limit", "100000"])["contributions"].as_array().expect("a list").len();
    assert_eq!(before, after, "nothing was written");
}

/// Decision 4afee4855944: --answer naming the line answers every objection in the window, to any number
/// of decisions, in one reference.
#[test]
fn one_answer_naming_the_line_answers_every_objection_in_the_window() {
    let w = Weave::new();
    let (d1, _, _) = w.integrate(&[]);
    let line = w.json(&["weave", "head", "--json"])["line"].as_str().expect("the line").to_owned();
    w.commit("a.txt", b"two\n");
    let (d2, _, _) = w.integrate(&[]);
    for d in [&d1, &d2] {
        let why = w.id(&["claim", "I don't like it", "--as", VAL, "--json"]);
        w.ok(&["relate", &why, "contradicts", d, "--as", VAL]);
    }
    assert_eq!(w.json(&["weave", "review", "--json"])["open"].as_array().expect("open").len(), 2);
    w.commit("a.txt", b"three\n");
    let answer = format!("{line}=overridden:discussed, and we go ahead");
    w.integrate(&["--answer", &answer]);
    let review = w.json(&["weave", "review", "--json"]);
    assert_eq!(review["open"], serde_json::json!([]), "{review}");
}
