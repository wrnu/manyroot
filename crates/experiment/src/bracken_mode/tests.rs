//! Unit tests: the argument rules and their placement, and the materializer.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::task::{Change, Task};

/// Values an agent might try to smuggle into a command line.
fn hostile(canary: &str) -> Vec<String> {
    vec![
        "-x".into(),
        format!("--receive-pack=touch {canary}/pwned"),
        "--upload-pack=x".into(),
        format!("--output={canary}/out"),
        format!("--details-file={canary}/../../etc/passwd"),
        "--exec=x".into(),
        "-C".into(),
        format!("--dir={canary}"),
        "--as=setter".into(),
        "..".into(),
        "/".into(),
        "/etc/passwd".into(),
        "a\0b".into(),
        "a\nb".into(),
        "x".repeat(20_000),
        "ａｂｓ".into(),
    ]
}

/// Each tool's agent-chosen string slots, with a valid value for the others.
fn slots() -> Vec<(&'static str, Value, &'static str)> {
    let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    vec![
        ("log", json!({"kind": "claim"}), "kind"),
        ("log", json!({"tag": "spec"}), "tag"),
        ("show", json!({"id": id}), "id"),
        ("cat", json!({"id": id}), "id"),
        ("check", json!({"id": id}), "id"),
        ("claim", json!({"statement": "s"}), "statement"),
        ("claim", json!({"statement": "s", "tags": ["t"]}), "tags"),
        ("claim", json!({"statement": "s", "supersedes": [id]}), "supersedes"),
        ("artifact", json!({"function": "abs", "content": "def abs 1 : a"}), "function"),
        ("artifact", json!({"function": "abs", "content": "def abs 1 : a"}), "content"),
        ("relate", json!({"from": id, "kind": "refines", "to": id}), "from"),
        ("relate", json!({"from": id, "kind": "refines", "to": id}), "kind"),
        ("relate", json!({"from": id, "kind": "refines", "to": id}), "to"),
        ("relate", json!({"from": id, "kind": "refines", "to": id, "note": "n"}), "note"),
        ("evidence", json!({"target": id, "verdict": "pass", "method": "m"}), "target"),
        ("evidence", json!({"target": id, "verdict": "pass", "method": "m"}), "verdict"),
        ("evidence", json!({"target": id, "verdict": "pass", "method": "m"}), "method"),
        ("evidence", json!({"target": id, "verdict": "pass", "method": "m", "details": "d"}), "details"),
        ("evidence", json!({"target": id, "verdict": "fail", "method": "m", "depends_on": [id]}), "depends_on"),
        ("retract", json!({"target": id}), "target"),
        ("retract", json!({"target": id, "reason": "r"}), "reason"),
    ]
}

fn with_value(args: &Value, slot: &str, v: &str) -> Value {
    let mut args = args.clone();
    let value = if matches!(slot, "tags" | "supersedes" | "depends_on") { json!([v]) } else { json!(v) };
    args[slot] = value;
    args
}

/// Free text is allowed to hold these values: it is data, never an option.
fn is_free_text(tool: &str, slot: &str) -> bool {
    matches!(
        (tool, slot),
        ("claim", "statement") | ("artifact", "content") | (_, "note" | "method" | "details" | "reason")
    )
}

fn store(dir: PathBuf, bracken: PathBuf) -> BrackenStore {
    store_as(dir, bracken, "me", AcceptRule::OnePass)
}

fn store_as(dir: PathBuf, bracken: PathBuf, identity: &str, accept_rule: AcceptRule) -> BrackenStore {
    store_for(dir, bracken, identity, accept_rule, Task::Phase1)
}

fn store_for(dir: PathBuf, bracken: PathBuf, identity: &str, accept_rule: AcceptRule, task: Task) -> BrackenStore {
    let git = PathBuf::from("/nonexistent/git");
    let limit = crate::driver::ResultLimit::default();
    let config = StoreConfig { task, dir, bracken, git, identity: identity.into(), trace: false, accept_rule, limit };
    BrackenStore { config, runs: 0, files: 0 }
}

/// A stand-in that ignores bracken's arguments and writes forever, written by a child `sh` so this
/// process never holds it open for writing: a parallel test's fork would inherit that descriptor until
/// its exec, and spawning the file meanwhile fails on Linux with ETXTBSY, "Text file busy", an error
/// that doesn't say "deadline" (task 1b273bf9a56c; the same as aa2ac61f8dd4).
///
/// A script, not `yes` itself: GNU `yes` would refuse bracken's options and exit at once.
fn endless_writer(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("endless");
    let written = Command::new("sh")
        .args(["-c", "printf '#!/bin/sh\\nexec /usr/bin/yes\\n' >\"$0\" && chmod 755 \"$0\""])
        .arg(&path)
        .status()
        .expect("sh");
    assert!(written.success(), "writing the stand-in");
    path
}

/// A hub stating the task, and agents a and b joined to it, each in its own directory under `root`.
fn ground_of(root: &std::path::Path, rule: AcceptRule) -> (BrackenStore, BrackenStore, BrackenStore) {
    ground_for(root, rule, Task::Phase1)
}

fn ground_for(root: &std::path::Path, rule: AcceptRule, task: Task) -> (BrackenStore, BrackenStore, BrackenStore) {
    let at = |name: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("a store dir");
        dir
    };
    let mut hub = store_for(at("hub"), bracken_bin(), "setter", rule, task);
    hub.setup(None).expect("the hub");
    let bundle = hub.export().expect("the hub's bundle");
    let join = |name: &str| {
        let mut s = store_for(at(name), bracken_bin(), name, rule, task);
        s.setup(Some(&bundle)).expect("a copy");
        s
    };
    let (a, b) = (join("a"), join("b"));
    (hub, a, b)
}

/// What `from` has, now in `to` too.
fn carry(from: &mut BrackenStore, to: &mut BrackenStore) {
    let bytes = from.export().expect("a bundle");
    to.import(&bytes).expect("import");
}

fn output(v: Result<Value, String>) -> String {
    v.expect("the tool")["output"].as_str().expect("text output").to_string()
}

/// A new contribution by `s` through `tool`; its full id.
fn made(s: &mut BrackenStore, tool: &str, args: Value) -> String {
    output(s.tool(tool, &args)).trim().to_string()
}

/// An implementation of `function` by `s`, related to its current spec claim; its id.
fn implement_in(s: &mut BrackenStore, function: &str, program: &str) -> String {
    let id = made(s, "artifact", json!({"function": function, "content": format!("def {function} 1 : {program}")}));
    let spec = s.state().expect("state").spec_claim(function).expect("a spec claim");
    s.tool("relate", &json!({"from": id, "kind": "implements", "to": spec})).expect("relate");
    id
}

/// The status line of implementation `id`, trimmed, as the status tool prints it.
fn status_of(s: &mut BrackenStore, id: &str) -> Option<String> {
    let text = output(s.tool("status", &json!({})));
    text.lines().find(|l| l.contains(&id[..8])).map(|l| l.trim().to_string())
}

fn accepted_of(s: &mut BrackenStore, id: &str) -> Option<String> {
    let rows = s.tool("accepted", &json!({})).expect("accepted");
    rows.as_array()?.iter().find(|r| r["id"] == id).map(|r| r["status"].as_str().unwrap_or_default().to_string())
}

#[test]
fn every_tool_slot_is_in_the_table() {
    let tools: Vec<&str> = BrackenMode::ONE_PASS.tools(Task::Phase1).iter().map(|t| t.name).collect();
    for t in &tools {
        assert!(
            slots().iter().any(|(name, _, _)| name == t)
                || matches!(*t, "heads" | "conflicts" | "accepted" | "status" | "news"),
            "{t}"
        );
    }
    for t in BrackenMode::ONE_PASS.tools(Task::Phase1) {
        let props = t.input_schema["properties"].as_object().expect("properties");
        for key in props.keys() {
            let typed_string = props[key]["type"] == "string" || props[key]["type"] == "array";
            assert!(!typed_string || slots().iter().any(|(n, _, s)| *n == t.name && s == key), "{}.{key}", t.name);
        }
    }
}

#[test]
fn hostile_arguments_are_refused_before_any_process_starts() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let canary = dir.path().join("canary");
    let mut s = store(dir.path().to_path_buf(), PathBuf::from("/nonexistent/bracken"));
    for (tool, args, slot) in slots() {
        for v in hostile(&canary.to_string_lossy()) {
            if is_free_text(tool, slot) && v.len() <= crate::args::MAX_TEXT && !v.contains('\0') {
                continue;
            }
            let result = s.tool(tool, &with_value(&args, slot, &v));
            assert!(result.is_err(), "{tool}.{slot} accepted {:?}", &v[..v.len().min(40)]);
        }
    }
    assert_eq!(s.runs, 0, "a refused call starts no process");
    assert!(!canary.exists());
}

/// The bracken binary cargo built next to the test binary.
fn bracken_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary");
    let bin = exe.parent().and_then(|d| d.parent()).expect("target/<profile>").join("bracken");
    assert!(bin.exists(), "build bracken first: cargo build -p bracken");
    bin
}

#[test]
fn placement_alone_keeps_hostile_values_from_being_options() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let canary = dir.path().join("canary");
    std::fs::create_dir(&canary).expect("canary dir");
    let replica = dir.path().join("copy");
    std::fs::create_dir(&replica).expect("store dir");
    let mut s = store(replica.clone(), bracken_bin());
    s.run(&["init".into(), "--name=t".into()], None).expect("init");
    let key = String::from_utf8(s.run(&["id".into(), "new".into(), "--".into(), "me".into()], None).expect("id"))
        .expect("utf8")
        .trim()
        .to_string();
    for (tool, args, slot) in slots().into_iter().filter(|(t, _, _)| *t != "check") {
        for v in hostile(&canary.to_string_lossy()).into_iter().filter(|v| !v.contains('\0')) {
            let args = with_value(&args, slot, &v);
            let plan = plan(tool, &Args::unchecked(&args).expect("an object"), "me", Task::Phase1).expect("a plan");
            // Every value is after `--` or inside one `--option=value` argument.
            let dashdash = plan.argv.iter().position(|a| a == "--");
            for (i, a) in plan.argv.iter().enumerate() {
                let after = dashdash.is_some_and(|d| i > d);
                assert!(after || a.starts_with("--") || i == 0, "{tool}.{slot}: {a:?} isn't placed safely");
                if !after && i > 0 {
                    assert!(a.contains('=') || ["--json", "--current", "--details-file=-", "--"].contains(&a.as_str()));
                }
            }
            let result = s.execute(plan);
            if let Err(e) = &result {
                assert!(!e.contains("unexpected argument") && !e.contains("unrecognized"), "{tool}.{slot}: {e}");
            }
        }
    }
    let log = s.json(&["log", "--json"]).expect("log");
    assert!(log.as_array().expect("a list").iter().all(|e| e["author"] == key.as_str()), "nobody signed as another");
    let canary_files: Vec<_> = std::fs::read_dir(&canary).expect("canary").collect();
    assert!(canary_files.is_empty(), "nothing was written through a hostile value: {canary_files:?}");
}

// The materializer.

struct Ground {
    rows: Vec<Value>,
    evidence: usize,
}

impl Ground {
    fn new() -> Ground {
        let mut g = Ground { rows: Vec::new(), evidence: 0 };
        g.push("spec-abs", "claim", "setter", json!({"statement": "s", "tags": ["spec", "abs"]}));
        g.push("spec-sq", "claim", "setter", json!({"statement": "s", "tags": ["spec", "sq"]}));
        g
    }

    fn push(&mut self, id: &str, kind: &str, author: &str, body: Value) {
        self.rows.push(json!({"id": id, "kind": kind, "author": author, "body": body}));
    }

    /// An artifact for `function` holding `text`, related by its author to the function's spec claim.
    fn artifact(&mut self, id: &str, author: &str, function: &str, text: &str) {
        self.push(id, "artifact", author, json!({"name": function, "text": text}));
        let rel = json!({"kind": "implements", "from": id, "to": format!("spec-{function}")});
        self.push(&format!("rel-{id}"), "relation", author, rel);
    }

    /// An implementation of `function` by `program`, with a pass from each of `passes` validators.
    fn implement(&mut self, id: &str, author: &str, function: &str, program: &str, passes: usize) {
        self.artifact(id, author, function, &format!("def {function} 1 : {program}\n"));
        for v in 0..passes {
            self.verdict(id, &format!("validator-{v}"), "pass");
        }
    }

    fn verdict(&mut self, target: &str, validator: &str, verdict: &str) {
        self.evidence += 1;
        let body = json!({"target": target, "verdict": verdict, "method": "m", "details": ""});
        self.push(&format!("ev-{}", self.evidence), "evidence", validator, body);
    }

    /// The contribution `id` retracted: `log --current` leaves it out.
    fn retract(&mut self, id: &str) {
        self.rows.retain(|r| r["id"] != id);
    }

    fn snapshot(&self) -> Snapshot {
        self.snapshot_under(AcceptRule::OnePass)
    }

    fn snapshot_under(&self, rule: AcceptRule) -> Snapshot {
        Snapshot::new(&json!(self.rows), "setter", rule, Task::Phase1).expect("a snapshot")
    }
}

/// Rivals: a1 with one pass, then a2 and a3 with two each; s1 and s2 with one each.
fn rivals() -> Ground {
    let mut g = Ground::new();
    g.implement("a1", "x", "abs", "a", 1);
    g.implement("a2", "y", "abs", "a 0 < a neg a ?", 2);
    g.implement("a3", "z", "abs", "a a", 2);
    g.implement("s1", "x", "sq", "a a *", 1);
    g.implement("s2", "y", "sq", "a dup *", 1);
    g
}

#[test]
fn rivals_with_passes_resolve_by_causal_order_alone() {
    let out = rivals().snapshot().integrated();
    assert_eq!(out["abs"], "def abs 1 : a\n", "the earliest with a pass, not the one with most");
    assert_eq!(out["sq"], "def sq 1 : a a *\n");
}

#[test]
fn phase1s_rule_takes_most_passes_then_earliest() {
    let out = rivals().snapshot_under(AcceptRule::MostPasses).integrated();
    assert_eq!(out["abs"], "def abs 1 : a 0 < a neg a ?\n", "most passes, then the earlier of a2 and a3");
    assert_eq!(out["sq"], "def sq 1 : a a *\n", "a tie goes to the earliest");
}

#[test]
fn one_independent_pass_accepts_and_a_later_independent_fail_takes_it_back() {
    let mut g = Ground::new();
    g.implement("a1", "x", "abs", "a 0 < a neg a ?", 0);
    assert!(g.snapshot().integrated().is_empty(), "no pass yet");
    // The author's own pass doesn't count.
    g.verdict("a1", "x", "pass");
    assert!(g.snapshot().integrated().is_empty(), "the author's own pass");
    // One pass from someone else accepts it.
    g.verdict("a1", "validator-0", "pass");
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
    // A later independent fail takes it back, even beside the pass.
    g.verdict("a1", "validator-1", "fail");
    assert!(g.snapshot().integrated().is_empty(), "un-accepted by the fail");
    // A later rival with a pass counts while a1 is out; a1 counts again, as the earlier, once that
    // validator retracts its fail (decision `d036bab456a9`).
    g.implement("a2", "y", "abs", "a", 1);
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a\n");
    let fail = format!("ev-{}", g.evidence - 1);
    g.retract(&fail);
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
}

/// Decision `d036bab456a9`: under the one-pass rule, as in Bracken's default policy, a validator's
/// later pass doesn't lift its fail; retracting the fail does. Phase 1's rule takes the latest verdict.
#[test]
fn a_fail_then_a_pass_stays_out_under_one_pass_and_counts_under_phase1s_rule() {
    let mut g = Ground::new();
    g.implement("a1", "x", "abs", "a 0 < a neg a ?", 0);
    g.verdict("a1", "validator-0", "fail");
    let fail = format!("ev-{}", g.evidence);
    g.verdict("a1", "validator-0", "pass");
    assert!(g.snapshot().integrated().is_empty(), "the fail still counts");
    assert_eq!(g.snapshot_under(AcceptRule::MostPasses).integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
    g.retract(&fail);
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n", "the retracted fail is lifted");
    // The author's own fail never counts, under either rule.
    g.verdict("a1", "x", "fail");
    assert_eq!(g.snapshot().integrated().len(), 1);
    assert_eq!(g.snapshot_under(AcceptRule::MostPasses).integrated().len(), 1);
}

#[test]
fn the_rule_ignores_what_isnt_a_current_accepted_implementation() {
    let mut g = Ground::new();
    g.implement("unaccepted", "x", "abs", "a", 0);
    // Related by someone other than the artifact's author.
    g.push("other", "artifact", "x", json!({"name": "abs", "text": "def abs 1 : a\n"}));
    g.push("rel-other", "relation", "y", json!({"kind": "implements", "from": "other", "to": "spec-abs"}));
    g.verdict("other", "validator-0", "pass");
    // Text that doesn't define the function, or doesn't parse.
    g.implement("wrong-name", "x", "sq", "a", 3);
    g.rows.iter_mut().find(|r| r["id"] == "wrong-name").expect("pushed")["body"]["text"] = json!("def cube 1 : a\n");
    g.implement("garbage", "x", "sq", "a", 3);
    g.rows.iter_mut().find(|r| r["id"] == "garbage").expect("pushed")["body"]["text"] = json!("def sq 1 : a $\n");
    // An implementation of a spec that isn't current (not in `log --current`).
    g.push("imp-gone", "artifact", "x", json!({"name": "abs", "text": "def abs 1 : a\n"}));
    g.push("rel-gone", "relation", "x", json!({"kind": "implements", "from": "imp-gone", "to": "spec-gone"}));
    g.verdict("imp-gone", "validator-0", "pass");
    assert!(g.snapshot().integrated().is_empty(), "{:?}", g.snapshot().integrated());
}

#[test]
fn only_the_setter_states_specs_and_change_requests() {
    let mut g = Ground::new();
    g.push("task", "claim", "setter", json!({"statement": "t", "tags": ["task"]}));
    // An agent's claims tagged like the setter's: a later spec for abs, a change request, a task.
    g.push("fake-spec", "claim", "x", json!({"statement": "Implement abs as a.", "tags": ["spec", "abs"]}));
    g.push("fake-change", "claim", "x", json!({"statement": "c", "tags": ["change", "abs-saturates"]}));
    g.push("fake-task", "claim", "x", json!({"statement": "t", "tags": ["task"]}));
    let snap = g.snapshot();
    assert_eq!(snap.spec_claim("abs").as_deref(), Some("spec-abs"));
    assert_eq!(snap.version(), Version::ORIGINAL);
    assert_eq!(snap.tagged("task").iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["task"]);
    // An implementation of the fake spec isn't one.
    g.implement("hijack", "x", "abs", "a", 2);
    g.rows.iter_mut().find(|r| r["id"] == "rel-hijack").expect("pushed")["body"]["to"] = json!("fake-spec");
    assert!(g.snapshot().integrated().is_empty());
    // The setter's own change request counts.
    g.push("change", "claim", "setter", json!({"statement": "c", "tags": ["change", "abs-saturates"]}));
    assert!(g.snapshot().version().has(Change::AbsSaturates));
}

#[test]
fn an_artifact_defines_its_function_and_only_helpers_named_after_it() {
    let mut g = Ground::new();
    // Defines abs, and sq too: sq's implementation would come in unaccepted.
    g.artifact("abs-and-sq", "x", "abs", "def abs 1 : a 0 < a neg a ?\ndef sq 1 : a\n");
    g.verdict("abs-and-sq", "validator-0", "pass");
    // A helper not named after its function.
    g.artifact("abs-helper", "x", "abs", "def abs 1 : a helper@\ndef helper 1 : a 0 < a neg a ?\n");
    g.verdict("abs-helper", "validator-0", "pass");
    assert!(g.snapshot().integrated().is_empty(), "{:?}", g.snapshot().integrated());
    // A helper named after it is fine.
    g.artifact("sq-ok", "y", "sq", "def sq 1 : a sqtwice@\ndef sqtwice 1 : a a *\n");
    g.verdict("sq-ok", "validator-0", "pass");
    assert_eq!(g.snapshot().integrated().keys().copied().collect::<Vec<_>>(), ["sq"]);
    assert!(
        artifact_problem(Task::Phase1, "abs", "def abs 1 : a\ndef absx 1 : a\ndef absx 1 : a\n").is_some(),
        "a helper twice"
    );
    // So helpers never collide across functions, no spec name starts another.
    for s in Task::Phase1.specs() {
        for t in Task::Phase1.specs().iter().filter(|t| t.name != s.name) {
            assert!(!t.name.starts_with(s.name), "{} starts {}", s.name, t.name);
        }
    }
}

#[test]
fn each_validator_counts_once_with_its_latest_verdict_and_never_the_author() {
    let mut g = Ground::new();
    // One validator passing three times is one pass: a2's two validators win.
    g.implement("a1", "x", "abs", "a", 0);
    for _ in 0..3 {
        g.verdict("a1", "validator-0", "pass");
    }
    g.implement("a2", "y", "abs", "a 0 < a neg a ?", 2);
    // Pass counts show only under Phase 1's rule, which ranks by them.
    let phase1 = |g: &Ground| g.snapshot_under(AcceptRule::MostPasses).integrated();
    assert_eq!(phase1(&g)["abs"], "def abs 1 : a 0 < a neg a ?\n");
    // A validator's later fail replaces its pass, and a counted fail keeps it out.
    g.verdict("a2", "validator-1", "fail");
    assert_eq!(phase1(&g)["abs"], "def abs 1 : a\n");
    // Its later pass replaces the fail again.
    g.verdict("a2", "validator-1", "pass");
    assert_eq!(phase1(&g)["abs"], "def abs 1 : a 0 < a neg a ?\n");
    // The author's own verdicts never count, pass or fail.
    g.implement("s1", "x", "sq", "a a *", 0);
    g.verdict("s1", "x", "pass");
    assert!(!g.snapshot().integrated().contains_key("sq"));
    g.verdict("s1", "validator-0", "pass");
    g.verdict("s1", "x", "fail");
    assert_eq!(g.snapshot().integrated()["sq"], "def sq 1 : a a *\n");
}

#[test]
fn one_counted_fail_keeps_an_implementation_out() {
    // Decision 5e8df6996a2e: two passes and another validator's fail is missing, not two passes against one.
    let mut g = Ground::new();
    g.implement("a1", "x", "abs", "a 0 < a neg a ?", 2);
    g.verdict("a1", "validator-2", "fail");
    assert!(g.snapshot().integrated().is_empty(), "{:?}", g.snapshot().integrated());
    // Under Phase 1's rule the same validator's later pass lifts its fail: three passes. Under the
    // one-pass rule it doesn't (decision `d036bab456a9`).
    g.verdict("a1", "validator-2", "pass");
    assert_eq!(g.snapshot_under(AcceptRule::MostPasses).integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
    assert!(g.snapshot().integrated().is_empty());
}

#[test]
fn one_artifact_cant_break_its_module() {
    let padded = |n: usize| {
        let head = "def abs 1 : a 0 < a neg a ?\n#";
        format!("{head}{}\n", "x".repeat(n - head.len() - 1))
    };
    assert!(artifact_problem(Task::Phase1, "abs", &padded(MAX_ARTIFACT_BYTES)).is_none());
    assert!(artifact_problem(Task::Phase1, "abs", &padded(MAX_ARTIFACT_BYTES + 1)).is_some());
    let mut g = Ground::new();
    g.artifact("big", "x", "abs", &padded(MAX_ARTIFACT_BYTES + 1));
    g.verdict("big", "validator-0", "pass");
    assert!(g.snapshot().integrated().is_empty());
    // Every module, each function's artifact at the largest size, still parses.
    let mut programs = Programs::new();
    for s in Task::Phase1.specs() {
        let head = format!("def {} {} : a\n#", s.name, s.arity);
        programs.insert(s.name, format!("{head}{}", "x".repeat(MAX_ARTIFACT_BYTES - head.len())));
    }
    assert!(programs.values().all(|t| t.len() == MAX_ARTIFACT_BYTES));
    let modules = modules_of(Task::Phase1, &programs);
    let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
    assert!(library.broken.is_empty(), "{:?}", library.broken);
    assert!(Task::Phase1.specs().iter().all(|s| library.get(s.name).is_some()));
}

#[test]
fn the_working_library_falls_back_to_mine_then_anyones_latest() {
    let mut g = Ground::new();
    g.implement("abs-other-old", "y", "abs", "a", 0);
    g.implement("abs-mine", "me", "abs", "a neg", 0);
    g.implement("abs-other-new", "y", "abs", "a a +", 0);
    g.implement("sq-other-old", "y", "sq", "a", 0);
    g.implement("sq-other-new", "z", "sq", "a a *", 0);
    let snap = g.snapshot();
    let working = snap.working("me");
    assert_eq!(working["abs"], "def abs 1 : a neg\n", "my own latest beats a later one by someone else");
    assert_eq!(working["sq"], "def sq 1 : a a *\n", "then anyone's latest");
    assert!(snap.integrated().is_empty(), "integration keeps the strict rule");
    g.implement("abs-accepted", "y", "abs", "a 0 < a neg a ?", 1);
    assert_eq!(g.snapshot().working("me")["abs"], "def abs 1 : a 0 < a neg a ?\n", "the rule's pick comes first");
}

#[test]
fn the_artifact_tool_refuses_what_cant_be_an_implementation() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut s = store(dir.path().to_path_buf(), PathBuf::from("/nonexistent/bracken"));
    for content in [
        "def abs 1 : a\ndef sq 1 : a\n".to_string(),
        "def abs 1 : a helper@\ndef helper 1 : a\n".to_string(),
        "def sq 1 : a\n".to_string(),
        format!("def abs 1 : a\n#{}\n", "x".repeat(MAX_ARTIFACT_BYTES)),
    ] {
        let result = s.tool("artifact", &json!({"function": "abs", "content": content}));
        assert!(result.is_err(), "{content:.40}");
    }
    assert_eq!(s.runs, 0, "a refused artifact starts no process");
}

#[test]
fn a_sync_session_ends_at_its_deadline_whatever_bracken_does() {
    // `endless_writer` stands in for a bracken that never stops writing, and the peer never reads.
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let mut s = store(dir.path().to_path_buf(), endless_writer(dir.path()));
    let session = Session { sync_seconds: 1, ..Session::default() };
    let (tx, rx) = std::sync::mpsc::channel();
    let held = std::thread::spawn(move || {
        let (peer, _) = listener.accept().expect("accept");
        rx.recv().ok();
        drop(peer);
    });
    let started = std::time::Instant::now();
    let result = s.sync_server(&socket, &session);
    assert!(result.as_ref().is_err_and(|e| e.contains("deadline")), "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    tx.send(()).ok();
    held.join().expect("the peer");
}

#[test]
fn a_sync_session_that_never_ends_by_itself_is_stopped_at_its_deadline() {
    // `endless_writer` never stops writing and this peer never stops reading, so only the deadline ends it.
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let mut s = store(dir.path().to_path_buf(), endless_writer(dir.path()));
    let session = Session { sync_seconds: 1, ..Session::default() };
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let (mut peer, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 65536];
        while peer.read(&mut buf).is_ok_and(|n| n > 0) {}
    });
    let started = std::time::Instant::now();
    let result = s.sync_server(&socket, &session);
    assert!(result.as_ref().is_err_and(|e| e.contains("deadline")), "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    reader.join().expect("the peer");
}

/// The `accepted` tool shows what counts, by the same rule: a fail the same validator later followed with
/// a pass still keeps it out (decision `d036bab456a9`), and an artifact related only by someone else, or
/// to no spec, doesn't count (finding `6792b444895d`).
#[test]
fn the_accepted_tool_shows_exactly_what_counts() {
    let mut g = Ground::new();
    g.implement("x-abs", "x", "abs", "a", 0);
    g.verdict("x-abs", "validator-0", "fail");
    g.verdict("x-abs", "validator-0", "pass");
    g.push("y-abs", "artifact", "y", json!({"name": "abs", "text": "def abs 1 : a\n"}));
    g.push("rel-z", "relation", "z", json!({"kind": "implements", "from": "y-abs", "to": "spec-abs"}));
    g.verdict("y-abs", "validator-1", "pass");
    // A later rival by w with a pass, and an implementation by v with none.
    g.implement("w-abs", "w", "abs", "a a", 1);
    g.implement("v-sq", "v", "sq", "a a *", 0);
    let counted = g.snapshot().counted();
    let rows: Vec<(&str, &str, &str)> = counted
        .as_array()
        .expect("a list")
        .iter()
        .map(|r| {
            (r["function"].as_str().unwrap_or(""), r["id"].as_str().unwrap_or(""), r["status"].as_str().unwrap_or(""))
        })
        .collect();
    assert_eq!(
        rows,
        [("abs", "x-abs", "waiting"), ("abs", "w-abs", "accepted"), ("sq", "v-sq", "waiting")],
        "{counted}"
    );
    assert!(counted.as_array().expect("a list").iter().all(|r| r.get("passes").is_none()), "no pass counts");
    let integrated = g.snapshot().integrated();
    assert_eq!(integrated["abs"], "def abs 1 : a a\n", "the same pick as the integrated library");
    assert_eq!(integrated.len(), 1);
}

/// Task `541b6f3db130` and decision `d036bab456a9`: Bracken's status view and the harness's accepted
/// tool agree in the usual case and on fails; the task's own rules (here: only the author's own
/// relation counts) are where they differ, and the accepted tool is what counts.
#[test]
fn the_status_view_agrees_with_accepted_and_differs_only_by_the_tasks_rules() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    let abs = implement_in(&mut a, "abs", "a 0 < a neg a ?");
    let sq = implement_in(&mut a, "sq", "a a *");
    carry(&mut a, &mut b);
    assert!(status_of(&mut b, &abs).expect("listed").starts_with("waiting"));
    assert_eq!(accepted_of(&mut b, &abs).as_deref(), Some("waiting"));
    // The usual case: one pass from someone else.
    b.tool("evidence", &json!({"target": &abs[..8], "verdict": "pass", "method": "ran it"})).expect("a pass");
    assert!(status_of(&mut b, &abs).expect("listed").starts_with("accepted"));
    assert_eq!(accepted_of(&mut b, &abs).as_deref(), Some("accepted"));
    // A fail, then a pass from the same validator: still out, in both.
    let fail = made(&mut b, "evidence", json!({"target": sq, "verdict": "fail", "method": "ran it"}));
    b.tool("evidence", &json!({"target": sq, "verdict": "pass", "method": "ran it again"})).expect("a pass");
    assert!(status_of(&mut b, &sq).expect("listed").starts_with("rejected"));
    assert_eq!(accepted_of(&mut b, &sq).as_deref(), Some("waiting"));
    // Retracting the fail lifts it, in both.
    b.tool("retract", &json!({"target": &fail[..10], "reason": "I was wrong"})).expect("retract");
    assert!(status_of(&mut b, &sq).expect("listed").starts_with("accepted"));
    assert_eq!(accepted_of(&mut b, &sq).as_deref(), Some("accepted"));
    // A task rule: an implementation related by someone other than its author is accepted in
    // Bracken's view, and doesn't count.
    let inc = made(&mut a, "artifact", json!({"function": "inc", "content": "def inc 1 : a 1 +"}));
    carry(&mut a, &mut b);
    let spec = b.state().expect("state").spec_claim("inc").expect("a spec");
    b.tool("relate", &json!({"from": inc, "kind": "implements", "to": spec})).expect("relate");
    b.tool("evidence", &json!({"target": inc, "verdict": "pass", "method": "ran it"})).expect("a pass");
    assert!(status_of(&mut b, &inc).expect("listed").starts_with("accepted"));
    assert_eq!(accepted_of(&mut b, &inc), None, "not an implementation the task counts");
}

/// The setter's `depends_on` relations, from its claims to its claims.
fn setters_depends_on(s: &mut BrackenStore) -> Vec<(String, String)> {
    let state = s.snapshot().expect("state");
    let log = s.json(&["log", "--json", "--current", "--kind=relation"]).expect("log");
    let mut out: Vec<(String, String)> = log
        .as_array()
        .expect("a list")
        .iter()
        .filter(|r| r["body"]["kind"] == "depends_on" && r["author"].as_str() == Some(state.setter.as_str()))
        .map(|r| {
            (r["body"]["from"].as_str().unwrap_or("").to_string(), r["body"]["to"].as_str().unwrap_or("").to_string())
        })
        .collect();
    out.sort();
    out
}

/// Decision `2b91a9541aba`: in Phase 2 the setter states each spec's dependencies (what it is defined in
/// terms of and its contract partners) and renews them at each release; Phase 1's setter states none.
/// An implementation passed before a wave changed a spec its own spec depends on needs a recheck: out of
/// the accepted tool's count and the integrated library, shown as such in status, until a new pass.
#[test]
fn acceptance_follows_the_waves_through_the_setters_dependencies() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (mut hub1, _, _) = ground_of(&dir.path().join("p1"), AcceptRule::OnePass);
    assert!(setters_depends_on(&mut hub1).is_empty(), "Phase 1's task reads as its runs did");

    let t = Task::Phase2;
    let (mut hub, mut a, mut b) = ground_for(&dir.path().join("p2"), AcceptRule::OnePass, t);
    let edges: usize = t.specs().iter().map(|s| spec_dependencies(t, s.name).len()).sum();
    assert_eq!(setters_depends_on(&mut hub).len(), edges);
    assert_eq!(edges, 139, "99 calls and 40 contract partners");
    let state = hub.snapshot().expect("state");
    let (toward, sign) = (state.spec_claim("toward").expect("spec"), state.spec_claim("sign").expect("spec"));
    assert!(setters_depends_on(&mut hub).contains(&(toward.clone(), sign.clone())), "toward is defined by sign");

    let program = crate::candidates::correct_programs(t, Version::ORIGINAL)["toward"];
    let id = made(&mut a, "artifact", json!({"function": "toward", "content": format!("def toward 2 : {program}")}));
    a.tool("relate", &json!({"from": id, "kind": "implements", "to": toward})).expect("relate");
    carry(&mut a, &mut b);
    b.tool("evidence", &json!({"target": id, "verdict": "pass", "method": "ran it"})).expect("a pass");
    assert_eq!(accepted_of(&mut b, &id).as_deref(), Some("accepted"));

    // Wave 1 changes sign: the setter's new sign spec, and toward's spec now depending on it too.
    assert!(t.wave_changes(1).any(|c| c.target == "sign"));
    hub.release_changes(1).expect("wave 1");
    let new_sign = hub.snapshot().expect("state").spec_claim("sign").expect("spec");
    assert_ne!(new_sign, sign);
    assert!(setters_depends_on(&mut hub).contains(&(toward.clone(), new_sign)), "renewed at the release");
    carry(&mut hub, &mut b);
    assert_eq!(accepted_of(&mut b, &id).as_deref(), Some("recheck"));
    assert!(!b.state().expect("state").integrated().contains_key("toward"), "out of the integrated library");
    assert!(status_of(&mut b, &id).expect("listed").starts_with("recheck"));
    let shown = b.tool("show", &json!({"id": id})).expect("show").to_string();
    assert!(shown.contains("needs_recheck"), "{shown}");

    // A pass that has seen the change clears it.
    b.tool("evidence", &json!({"target": id, "verdict": "pass", "method": "ran it under wave 1"})).expect("a pass");
    assert_eq!(accepted_of(&mut b, &id).as_deref(), Some("accepted"));
}

/// Owner decision f3405c284180, change B: in Phase 2, check can record its result as the caller's evidence
/// in the same call, through the evidence tool's own rules: a pass that counts, a fail that keeps an
/// implementation out and is recorded as blocking it. Phase 1's check is as its runs were given it.
#[test]
fn check_records_its_result_as_evidence_in_the_same_call() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    let abs = implement_in(&mut a, "abs", "a 0 < a neg a ?");
    let sq = implement_in(&mut a, "sq", "a a +");
    carry(&mut a, &mut b);

    let plain = b.tool("check", &json!({"id": &abs[..8]})).expect("a check");
    assert!(plain.get("evidence").is_none(), "without record, nothing is recorded: {plain}");
    assert_eq!(accepted_of(&mut b, &abs).as_deref(), Some("waiting"));

    let args = json!({"id": &abs[..8], "record": true});
    let passed = b.tool("check", &args).expect("a check");
    assert_eq!(
        (passed["ok"].as_bool(), passed["evidence"]["verdict"].as_str()),
        (Some(true), Some("pass")),
        "{passed}"
    );
    let id = passed["evidence"]["id"].as_str().expect("the evidence's id").to_string();
    let shown = b.tool("show", &json!({"id": id})).expect("the evidence");
    assert_eq!(
        (shown["body"]["method"].as_str(), shown["body"]["target"].as_str()),
        (Some("check"), Some(abs.as_str()))
    );
    assert_eq!(shown["body"]["details"], passed["details"]);
    assert_eq!(accepted_of(&mut b, &abs).as_deref(), Some("accepted"), "an independent pass counts");
    assert!(b.record("check", &args, &passed).expect("a record").is_null(), "a pass blocks nothing");

    let args = json!({"id": &sq[..8], "record": true});
    let failed = b.tool("check", &args).expect("a check");
    assert_eq!(
        (failed["ok"].as_bool(), failed["evidence"]["verdict"].as_str()),
        (Some(false), Some("fail")),
        "{failed}"
    );
    assert!(status_of(&mut b, &sq).expect("listed").starts_with("rejected"));
    let record = b.record("check", &args, &failed).expect("a record");
    assert_eq!(record["blocks"][0][0], "sq", "a recorded fail blocks, as the evidence tool's does: {record}");

    assert!(b.tool("check", &json!({"id": &sq[..8], "record": "yes"})).is_err(), "record is true or false");
    let (_hub, mut p, _) = ground_of(&dir.path().join("phase1"), AcceptRule::MostPasses);
    let old = implement_in(&mut p, "abs", "a 0 < a neg a ?");
    let err = p.tool("check", &json!({"id": old, "record": true})).expect_err("Phase 1 has no record");
    assert!(err.contains("unknown argument"), "{err}");
    let props = |mode: BrackenMode, task: Task| {
        let tools = mode.tools(task);
        let check = tools.iter().find(|t| t.name == "check").expect("a check tool");
        check.input_schema["properties"].as_object().expect("properties").keys().cloned().collect::<Vec<_>>()
    };
    assert_eq!(props(BrackenMode::PHASE1, Task::Phase1), ["id"]);
    assert_eq!(props(BrackenMode::ONE_PASS, Task::Phase1), ["id", "record"]);
}

/// Each agent's `news` starts with everything, then shows only what arrived since its own last call.
#[test]
fn news_shows_what_arrived_since_each_agents_last_call() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    let first = output(a.tool("news", &json!({})));
    assert!(first.lines().count() > Task::Phase1.specs().len() * 2 && first.contains("[task]"), "{first}");
    assert_eq!(output(a.tool("news", &json!({}))), "Nothing new since your last news call.");
    let id = made(&mut a, "claim", json!({"statement": "I take abs", "tags": ["note"]}));
    let news = output(a.tool("news", &json!({})));
    assert_eq!(news.lines().count(), 1, "{news}");
    assert!(news.starts_with(&id[..8]) && news.contains("I take abs"), "{news}");
    // b's cursor is its own: everything, then the agent's claim once it arrives.
    let b_first = output(b.tool("news", &json!({})));
    assert_eq!(b_first.lines().count(), first.lines().count());
    carry(&mut a, &mut b);
    let b_news = output(b.tool("news", &json!({})));
    assert!(b_news.contains("I take abs") && b_news.lines().count() == 1, "{b_news}");
    assert!(BrackenMode::PHASE1.tools(Task::Phase1).iter().all(|t| t.name != "news" && t.name != "status"));
}

/// A page that would pass the result bound stops short of it, and the next call goes on from there.
#[test]
fn news_pages_below_the_result_bound_and_loses_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, _b) = ground_of(dir.path(), AcceptRule::OnePass);
    output(a.tool("news", &json!({})));
    for i in 0..320 {
        let statement = format!("note {i:03} {}", "x".repeat(180));
        a.tool("claim", &json!({"statement": statement, "tags": ["note"]})).expect("a claim");
    }
    let page = output(a.tool("news", &json!({})));
    assert!(page.chars().count() < crate::driver::MAX_RESULT_CHARS, "{} chars", page.chars().count());
    let (lines, tail) = page.rsplit_once('\n').expect("a tail");
    assert!(tail.ends_with("more arrived: call news again for them."), "{tail}");
    let rest = output(a.tool("news", &json!({})));
    assert!(!rest.contains("more arrived"));
    let seen: Vec<&str> = lines.lines().chain(rest.lines()).collect();
    assert_eq!(seen.len(), 320, "every claim once");
    for (i, l) in seen.iter().enumerate() {
        assert!(l.contains(&format!("note {i:03} ")), "in order: {l}");
    }
    assert_eq!(output(a.tool("news", &json!({}))), "Nothing new since your last news call.");
}

/// Under Pi, a news page stops short of Pi's bound in bytes, and the pages still lose nothing (findings
/// `868ff1726b88`): Claude Code's pages are as before, above.
#[test]
fn under_pi_news_pages_below_pis_bound_and_loses_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, _b) = ground_of(dir.path(), AcceptRule::OnePass);
    a.config.limit = crate::driver::ResultLimit::Pi;
    output(a.tool("news", &json!({})));
    for i in 0..320 {
        // Two-byte characters, so a page counted in characters would be over the bound in bytes.
        let statement = format!("note {i:03} {}", "é".repeat(90));
        a.tool("claim", &json!({"statement": statement, "tags": ["note"]})).expect("a claim");
    }
    let mut pages = Vec::new();
    loop {
        let result = a.tool("news", &json!({})).expect("news");
        let sent = result.to_string().len();
        assert!(sent < crate::driver::MAX_RESULT_BYTES, "a page of {sent} bytes as sent would be cut");
        let page = result["output"].as_str().expect("text").to_string();
        match page.rsplit_once('\n') {
            Some((lines, tail)) if tail.ends_with("more arrived: call news again for them.") => {
                pages.push(lines.to_string())
            }
            _ => {
                pages.push(page);
                break;
            }
        }
    }
    assert!(pages.len() > 2, "{} pages", pages.len());
    let seen: Vec<&str> = pages.iter().flat_map(|p| p.lines()).collect();
    assert_eq!(seen.len(), 320, "every claim once");
    for (i, l) in seen.iter().enumerate() {
        assert!(l.contains(&format!("note {i:03} ")), "in order: {l}");
    }
}

#[test]
fn a_short_id_resolves_and_an_ambiguous_or_unknown_one_is_refused() {
    let ids = ["abcdef0123456789", "abcdef0199999999", "1234567890abcdef"];
    let resolve = |p: &str, min| resolve_id(p, ids.iter().copied(), min);
    assert_eq!(resolve("12345678", 8).as_deref(), Ok("1234567890abcdef"));
    assert!(resolve("abcdef01", 8).expect_err("ambiguous").contains("2 contributions"));
    assert_eq!(resolve("abcdef012", 8).as_deref(), Ok("abcdef0123456789"), "more digits settle it");
    assert!(resolve("1234567", 8).expect_err("too short").contains("at least 8"));
    assert!(resolve("ffffffff", 8).expect_err("unknown").contains("no contribution"));
    assert_eq!(
        resolve("12345678", 12).expect_err("Phase 1's rule"),
        "`12345678` is not an id (12 to 64 lowercase hex digits)"
    );

    // Through the tools, in a real replica.
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, _b) = ground_of(dir.path(), AcceptRule::OnePass);
    let spec = a.state().expect("state").spec_claim("abs").expect("a spec");
    let shown = a.tool("show", &json!({"id": &spec[..8]})).expect("show by 8 digits");
    assert_eq!(shown["id"], spec.as_str());
    assert!(a.tool("show", &json!({"id": &spec[..7]})).is_err(), "7 digits");
    let mine = made(&mut a, "claim", json!({"statement": "first", "tags": ["note"]}));
    let second = output(a.tool("claim", &json!({"statement": "second", "supersedes": [&mine[..9]]})));
    // What the agent sees of a write is what bracken printed before it labelled its ids (task
    // d4e958fd6719): each id it wrote, one per line, the claim first, then its supersedes relation.
    let lines: Vec<&str> = second.lines().collect();
    assert!(second.ends_with('\n') && lines.len() == 2, "{second:?}");
    assert!(lines.iter().all(|l| l.len() == 64 && l.bytes().all(|b| b.is_ascii_hexdigit())), "{second:?}");
    let relation = a.tool("show", &json!({"id": lines[1]})).expect("the relation");
    assert_eq!((relation["body"]["from"].as_str(), relation["body"]["to"].as_str()), (Some(lines[0]), Some(&*mine)));
    // Phase 1's tools take 12 digits or more, and have no status or news.
    let phase1 = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut p, _b) = ground_of(phase1.path(), AcceptRule::MostPasses);
    let spec = p.state().expect("state").spec_claim("abs").expect("a spec");
    assert!(p.tool("show", &json!({"id": &spec[..8]})).expect_err("8 digits").contains("12 to 64"));
    assert!(p.tool("show", &json!({"id": &spec[..12]})).is_ok());
    assert!(p.tool("status", &json!({})).is_err() && p.tool("news", &json!({})).is_err());
}

/// The next `news` pages, until nothing is new: their entry lines.
fn all_news(s: &mut BrackenStore) -> (Vec<String>, usize) {
    let (mut seen, mut pages) = (Vec::new(), 0);
    loop {
        let result = s.tool("news", &json!({})).expect("news");
        let sent = result.to_string().chars().count();
        assert!(sent < crate::driver::MAX_RESULT_CHARS, "a page of {sent} characters as sent would be cut");
        let page = result["output"].as_str().expect("text").to_string();
        if page.starts_with("Nothing new") {
            return (seen, pages);
        }
        pages += 1;
        assert!(pages <= 12, "paging never ends: {} lines so far", seen.len());
        seen.extend(page.lines().filter(|l| !l.contains("more arrived")).map(str::to_string));
    }
}

/// Findings `f8b022bbf5fd` and `36c77161816c`: a cut page keeps what the old cursor covered, so two
/// agents' concurrent work, part of it seen before, is listed once each across three or more pages.
#[test]
fn concurrent_work_across_pages_is_listed_once() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    output(a.tool("news", &json!({})));
    let note = |s: &mut BrackenStore, who: &str, i: usize| {
        let statement = format!("{who} note {i:03} {}", "x".repeat(180));
        s.tool("claim", &json!({"statement": statement, "tags": ["note"]})).expect("a claim");
    };
    // a's first notes, read; then more of a's, and b's, concurrently, unread.
    for i in 0..40 {
        note(&mut a, "a", i);
    }
    let (read, _) = all_news(&mut a);
    assert_eq!(read.len(), 40);
    for i in 40..100 {
        note(&mut a, "a", i);
    }
    for i in 0..520 {
        note(&mut b, "b", i);
    }
    carry(&mut b, &mut a);
    let (seen, pages) = all_news(&mut a);
    assert!(pages >= 3, "{pages} pages");
    assert_eq!(seen.len(), 580, "60 of a's and 520 of b's, each once");
    for (who, range) in [("a", 40..100), ("b", 0..520)] {
        for i in range {
            let n = seen.iter().filter(|l| l.contains(&format!(" {who} note {i:03} "))).count();
            assert_eq!(n, 1, "{who} note {i}");
        }
    }
    assert!(!seen.iter().any(|l| l.contains(" a note 000 ")), "seen before the cut, not listed again");
}

/// A page of quotes doubles when escaped; it is measured as sent, so the result bound never cuts it.
#[test]
fn a_news_page_of_quotes_fits_as_sent() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, _b) = ground_of(dir.path(), AcceptRule::OnePass);
    output(a.tool("news", &json!({})));
    for i in 0..300 {
        let statement = format!("q{i:03} {}", "\"".repeat(180));
        a.tool("claim", &json!({"statement": statement, "tags": ["note"]})).expect("a claim");
    }
    let (seen, pages) = all_news(&mut a);
    assert!(pages >= 2 && seen.len() == 300, "{pages} pages, {} lines", seen.len());
}

#[test]
fn an_unreadable_news_position_is_said() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, _b) = ground_of(dir.path(), AcceptRule::OnePass);
    std::fs::create_dir(a.config.dir.join("news-cursor.txt")).expect("a directory where the file goes");
    let err = a.tool("news", &json!({})).expect_err("refused");
    assert!(err.contains("couldn't be read"), "{err}");
}

/// Task `f1c8478d1012`: the status tool is bracken's queue from where the agent stands, and the evidence
/// tool's depends_on, each id the agent's own choice, is what status shows blocked, then ready.
#[test]
fn status_is_the_agents_queue_and_a_fail_can_name_what_it_needs() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    let abs = implement_in(&mut a, "abs", "a 0 < a neg a ?");
    let sq = implement_in(&mut a, "sq", "a a *");
    carry(&mut a, &mut b);
    let text = output(b.tool("status", &json!({})));
    let at = |text: &str, needle: &str| {
        text.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{text}"))
    };
    assert!(at(&text, "waiting for your pass:") < at(&text, &abs[..8]), "{text}");
    assert!(text.chars().count() < crate::driver::MAX_RESULT_CHARS);

    let out = output(b.tool(
        "evidence",
        &json!({"target": &sq[..8], "verdict": "fail", "method": "ran it", "depends_on": [&abs[..8]]}),
    ));
    assert_eq!(out.lines().count(), 2, "the evidence, then its depends_on relation: {out}");
    let line = status_of(&mut b, &sq).expect("listed");
    assert!(line.starts_with("blocked") && line.contains(&format!("needs {}", &abs[..8])), "{line}");
    b.tool("evidence", &json!({"target": &abs[..8], "verdict": "pass", "method": "ran it"})).expect("a pass");
    assert!(status_of(&mut b, &sq).expect("listed").starts_with("ready"));
    // The author sees their own work first.
    carry(&mut b, &mut a);
    let text = output(a.tool("status", &json!({})));
    assert!(text.starts_with("yours:\nready "), "{text}");

    // The ids are checked like every other id; a pass with depends_on is bracken's to refuse.
    let refused =
        b.tool("evidence", &json!({"target": sq, "verdict": "fail", "method": "m", "depends_on": ["ffffffff"]}));
    assert!(refused.expect_err("unknown").contains("no contribution"));
    let refused = b.tool("evidence", &json!({"target": sq, "verdict": "pass", "method": "m", "depends_on": [abs]}));
    assert!(refused.expect_err("a pass").contains("goes with a fail"));
    // Phase 1's evidence tool is as its runs were given it.
    let phase1 = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut p, _b) = ground_of(phase1.path(), AcceptRule::MostPasses);
    let spec = p.state().expect("state").spec_claim("abs").expect("a spec");
    let args = json!({"target": spec, "verdict": "fail", "method": "m", "depends_on": [spec]});
    assert!(p.tool("evidence", &args).is_err());
    let tools = BrackenMode::PHASE1.tools(Task::Phase1);
    let evidence = tools.iter().find(|t| t.name == "evidence").expect("evidence");
    assert!(evidence.input_schema["properties"].get("depends_on").is_none());
}

/// News puts what concerns the agent first, when it fits one page.
#[test]
fn news_puts_what_concerns_the_agent_first() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_hub, mut a, mut b) = ground_of(dir.path(), AcceptRule::OnePass);
    let abs = implement_in(&mut a, "abs", "a 0 < a neg a ?");
    carry(&mut a, &mut b);
    output(a.tool("news", &json!({})));
    b.tool("claim", &json!({"statement": "b's own aside", "tags": ["note"]})).expect("a claim");
    let verdict = made(&mut b, "evidence", json!({"target": abs, "verdict": "pass", "method": "ran it"}));
    carry(&mut b, &mut a);
    let news = output(a.tool("news", &json!({})));
    let lines: Vec<&str> = news.lines().collect();
    assert_eq!(lines.len(), 4, "{news}");
    assert!(lines[0] == "for you:" && lines[1].starts_with(&verdict[..8]) && lines[2] == "everything else:", "{news}");
    assert!(lines[3].contains("b's own aside"), "{news}");
}
