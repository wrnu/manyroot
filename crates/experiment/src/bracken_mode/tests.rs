//! Unit tests: the argument rules and their placement, and the materializer.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::task::Change;

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
        ("retract", json!({"target": id}), "target"),
        ("retract", json!({"target": id, "reason": "r"}), "reason"),
    ]
}

fn with_value(args: &Value, slot: &str, v: &str) -> Value {
    let mut args = args.clone();
    let value = if matches!(slot, "tags" | "supersedes") { json!([v]) } else { json!(v) };
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
    let config =
        StoreConfig { dir, bracken, git: PathBuf::from("/nonexistent/git"), identity: "me".into(), trace: false };
    BrackenStore { config, runs: 0, files: 0 }
}

#[test]
fn every_tool_slot_is_in_the_table() {
    let tools: Vec<&str> = BrackenMode.tools().iter().map(|t| t.name).collect();
    for t in &tools {
        assert!(
            slots().iter().any(|(name, _, _)| name == t) || matches!(*t, "heads" | "conflicts" | "accepted"),
            "{t}"
        );
    }
    for t in BrackenMode.tools() {
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
            let plan = plan(tool, &Args::unchecked(&args).expect("an object"), "me").expect("a plan");
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

    fn snapshot(&self) -> Snapshot {
        Snapshot::new(&json!(self.rows), "setter").expect("a snapshot")
    }
}

#[test]
fn the_rule_takes_most_passes_then_earliest() {
    let mut g = Ground::new();
    g.implement("a1", "x", "abs", "a", 1);
    g.implement("a2", "y", "abs", "a 0 < a neg a ?", 2);
    g.implement("a3", "z", "abs", "a a", 2);
    g.implement("s1", "x", "sq", "a a *", 1);
    g.implement("s2", "y", "sq", "a dup *", 1);
    let out = g.snapshot().integrated();
    assert_eq!(out["abs"], "def abs 1 : a 0 < a neg a ?\n", "most passes, then the earlier of a2 and a3");
    assert_eq!(out["sq"], "def sq 1 : a a *\n", "a tie goes to the earliest");
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
    assert!(artifact_problem("abs", "def abs 1 : a\ndef absx 1 : a\ndef absx 1 : a\n").is_some(), "a helper twice");
    // So helpers never collide across functions, no spec name starts another.
    for s in SPECS {
        for t in SPECS.iter().filter(|t| t.name != s.name) {
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
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
    // A validator's later fail replaces its pass, and a counted fail keeps it out.
    g.verdict("a2", "validator-1", "fail");
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a\n");
    // Its later pass replaces the fail again.
    g.verdict("a2", "validator-1", "pass");
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
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
    // The same validator's later pass lifts its fail: three passes.
    g.verdict("a1", "validator-2", "pass");
    assert_eq!(g.snapshot().integrated()["abs"], "def abs 1 : a 0 < a neg a ?\n");
}

#[test]
fn one_artifact_cant_break_its_module() {
    let padded = |n: usize| {
        let head = "def abs 1 : a 0 < a neg a ?\n#";
        format!("{head}{}\n", "x".repeat(n - head.len() - 1))
    };
    assert!(artifact_problem("abs", &padded(MAX_ARTIFACT_BYTES)).is_none());
    assert!(artifact_problem("abs", &padded(MAX_ARTIFACT_BYTES + 1)).is_some());
    let mut g = Ground::new();
    g.artifact("big", "x", "abs", &padded(MAX_ARTIFACT_BYTES + 1));
    g.verdict("big", "validator-0", "pass");
    assert!(g.snapshot().integrated().is_empty());
    // Every module, each function's artifact at the largest size, still parses.
    let mut programs = Programs::new();
    for s in SPECS {
        let head = format!("def {} {} : a\n#", s.name, s.arity);
        programs.insert(s.name, format!("{head}{}", "x".repeat(MAX_ARTIFACT_BYTES - head.len())));
    }
    assert!(programs.values().all(|t| t.len() == MAX_ARTIFACT_BYTES));
    let modules = modules_of(&programs);
    let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
    assert!(library.broken.is_empty(), "{:?}", library.broken);
    assert!(SPECS.iter().all(|s| library.get(s.name).is_some()));
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
    // `yes` stands in for a bracken that never stops writing, and the peer never reads.
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let mut s = store(dir.path().to_path_buf(), PathBuf::from("/usr/bin/yes"));
    let session = Session { sync_seconds: 1, ..Session::default() };
    let (tx, rx) = std::sync::mpsc::channel();
    let held = std::thread::spawn(move || {
        let (peer, _) = listener.accept().expect("accept");
        rx.recv().ok();
        drop(peer);
    });
    let started = std::time::Instant::now();
    let result = s.sync_server(&socket, &session);
    assert!(result.is_err_and(|e| e.contains("deadline")));
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    tx.send(()).ok();
    held.join().expect("the peer");
}

#[test]
fn a_sync_session_that_never_ends_by_itself_is_stopped_at_its_deadline() {
    // `yes` never stops writing and this peer never stops reading, so only the deadline ends it.
    let dir = tempfile::tempdir().expect("a temp dir");
    let socket = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let mut s = store(dir.path().to_path_buf(), PathBuf::from("/usr/bin/yes"));
    let session = Session { sync_seconds: 1, ..Session::default() };
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let (mut peer, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 65536];
        while peer.read(&mut buf).is_ok_and(|n| n > 0) {}
    });
    let started = std::time::Instant::now();
    assert!(s.sync_server(&socket, &session).is_err_and(|e| e.contains("deadline")));
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    reader.join().expect("the peer");
}
