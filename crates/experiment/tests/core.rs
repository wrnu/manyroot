//! The core and Bracken mode end to end (docs/design/phase1-experiment.md,
//! sections 2 and 3): the driver, its workers, the shim and the mock backend.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::Value;

use bracken_experiment::driver::{
    Backend, CALLS_PER_TURN, MAX_MCP_LINE, RunConfig, TURN_LIMIT, git_binary, run, sync_session,
};
use bracken_experiment::grader::{HiddenCases, grade};
use bracken_experiment::lang::Library;
use bracken_experiment::mock::{Client, MockBackend};
use bracken_experiment::mode::{Session, StoreConfig};
use bracken_experiment::task::{SPECS, Version};
use bracken_experiment::worker::{Request, Worker};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn bracken() -> PathBuf {
    let b = bin().with_file_name("bracken");
    assert!(b.exists(), "build bracken first: cargo build -p bracken");
    b
}

fn events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("the event log")
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

#[test]
fn a_mock_run_goes_end_to_end_through_the_shim() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig {
        root: dir.path().join("run"),
        mode: "bracken",
        agents: 3,
        turns: 3,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox: false,
    };
    let outcome = run(&cfg, &MockBackend { seed: 3, wrong_rate: 0.0, shim: Some(bin()) }).expect("the run");

    // The change requests went out once 40% of the 9 turns were used.
    assert_eq!(outcome.released_after, Some(4));
    let log = events(&outcome.events);
    let seqs: Vec<u64> = log.iter().map(|e| e["seq"].as_u64().expect("seq")).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "one writer, no gaps");
    for agent in ["agent-0", "agent-1", "agent-2"] {
        let calls = log.iter().filter(|e| e["agent"] == agent && e["event"] == "tool_call").count();
        let results = log.iter().filter(|e| e["agent"] == agent && e["event"] == "tool_result").count();
        assert!(calls >= 3 * 8 && calls == results, "{agent}: {calls} calls, {results} results");
        assert_eq!(log.iter().filter(|e| e["agent"] == agent && e["event"] == "turn_end").count(), 3);
    }
    assert!(
        log.iter().filter(|e| e["event"] == "tool_result").all(|e| e["data"]["ok"] == true),
        "every call succeeded"
    );
    assert_eq!(log.iter().filter(|e| e["event"] == "changes_released").count(), 1);

    // The integrated library holds only functions the rule picked, and the mock wrote only correct programs.
    let library = Library::parse(outcome.modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
    assert!(library.broken.is_empty() && library.duplicates.is_empty());
    let present: Vec<&str> = SPECS.iter().filter(|s| library.get(s.name).is_some()).map(|s| s.name).collect();
    assert!(!present.is_empty(), "something was accepted");
    let grades = grade(&library, Version::all(), &HiddenCases::new(1));
    for g in grades.iter().filter(|g| g.present) {
        assert!(g.standalone, "{} is a correct program", g.name);
    }
}

/// Every file under `dir` with its size and modification time.
fn tree(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).expect("readable").flatten() {
            let meta = e.metadata().expect("metadata");
            if meta.is_dir() {
                todo.push(e.path());
            } else {
                out.insert(e.path(), (meta.len(), meta.modified().expect("mtime")));
            }
        }
    }
    out
}

fn request(op: &str) -> Request {
    Request { op: op.into(), ..Request::default() }
}

#[test]
fn each_worker_writes_only_its_own_store() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = |name: &str, identity: &str| StoreConfig {
        dir: dir.path().join(name),
        bracken: bracken(),
        git: git_binary().expect("git"),
        identity: identity.into(),
        trace: false,
    };
    let mut hub = Worker::spawn(&bin(), "bracken", &config("hub", "setter")).expect("hub");
    hub.ok(&request("setup")).expect("hub setup");
    let seed = Worker::bytes(&hub.ok(&request("export")).expect("export")).expect("bytes");
    let mut a = Worker::spawn(&bin(), "bracken", &config("a", "agent-a")).expect("a");
    let mut b = Worker::spawn(&bin(), "bracken", &config("b", "agent-b")).expect("b");
    for w in [&mut a, &mut b] {
        w.ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })
            .expect("copy setup");
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    let before = |n: &str| tree(&dir.path().join(n));
    let (hub0, a0, b0) = (before("hub"), before("a"), before("b"));

    // A tool call changes only its own copy.
    let claim = serde_json::json!({"statement": "a claim", "tags": ["note"]});
    a.ok(&Request { op: "tool".into(), tool: Some("claim".into()), args: claim, ..Request::default() }).expect("claim");
    a.ok(&Request {
        op: "tool".into(),
        tool: Some("notes_write".into()),
        args: serde_json::json!({"text": "n"}),
        ..Request::default()
    })
    .expect("notes");
    assert_ne!(before("a"), a0);
    assert_eq!(before("b"), b0);
    assert_eq!(before("hub"), hub0);

    // Sync carries bytes: the copy exports, the hub imports. Each writes only itself.
    let a1 = before("a");
    let up = Worker::bytes(&a.ok(&request("export")).expect("export")).expect("bytes");
    hub.ok(&Request { op: "import".into(), bytes: Some(hex::encode(&up)), ..Request::default() }).expect("import");
    assert_ne!(before("hub"), hub0);
    assert_eq!(before("a"), a1);
    assert_eq!(before("b"), b0);

    // A copy refuses a bundle that isn't one, and stays as it was.
    let b1 = before("b");
    assert!(
        b.ok(&Request { op: "import".into(), bytes: Some(hex::encode(b"gitdir: /etc")), ..Request::default() })
            .is_err()
    );
    assert_eq!(before("b"), b1);
}

#[test]
fn the_shim_and_workers_refuse_to_start_with_any_environment() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let shim = Command::new(bin())
        .args(["shim", "--socket"])
        .arg(dir.path().join("none.sock"))
        .env_clear()
        .env("ANTHROPIC_AUTH_TOKEN", "fake-not-a-secret")
        .stdin(Stdio::null())
        .output()
        .expect("the shim runs");
    assert!(!shim.status.success());
    assert!(String::from_utf8_lossy(&shim.stderr).contains("empty environment"));
    let worker = Command::new(bin())
        .args(["worker", "--mode", "bracken", "--identity", "x", "--store"])
        .arg(dir.path())
        .arg("--bracken")
        .arg(bracken())
        .args(["--git", "/nonexistent/git"])
        .env_clear()
        .env("HOME", "/")
        .stdin(Stdio::null())
        .output()
        .expect("the worker runs");
    assert!(!worker.status.success());
    assert!(String::from_utf8_lossy(&worker.stderr).contains("empty environment"));
}

#[test]
fn a_run_refuses_a_directory_that_exists() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig {
        root: dir.path().to_path_buf(),
        mode: "bracken",
        agents: 1,
        turns: 1,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox: false,
    };
    let err = run(&cfg, &MockBackend { seed: 1, wrong_rate: 0.0, shim: None }).err().expect("refused");
    assert!(err.contains("already exists"));
}

/// Each turn, one claim; agent 0 instead sends an MCP line over the driver's cap, if `hostile`.
struct Claiming {
    hostile: bool,
    /// Times the driver closed agent 0's connection after the over-long line.
    closed: std::sync::atomic::AtomicUsize,
}

impl Claiming {
    fn new(hostile: bool) -> Claiming {
        Claiming { hostile, closed: Default::default() }
    }
}

impl Backend for Claiming {
    fn turn(&self, agent: usize, _name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        if self.hostile && agent == 0 {
            use std::io::{Read, Write};
            let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
            let _ = s.write_all(&vec![b'x'; MAX_MCP_LINE + 10]);
            s.set_read_timeout(Some(std::time::Duration::from_secs(10))).map_err(|e| e.to_string())?;
            if matches!(s.read(&mut [0u8; 1]), Ok(0)) {
                self.closed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            return Err("sent an over-long line".into());
        }
        let mut c = Client::connect(socket, None)?;
        c.tool("claim", serde_json::json!({"statement": format!("turn {turn}"), "tags": ["note"]}))?;
        Ok(())
    }
}

fn config(dir: &Path, agents: usize, turns: u32) -> RunConfig {
    RunConfig {
        root: dir.join("run"),
        mode: "bracken",
        agents,
        turns,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox: false,
    }
}

#[test]
fn sync_moves_only_what_is_missing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut hub = Worker::spawn(
        &bin(),
        "bracken",
        &StoreConfig {
            dir: dir.path().join("hub"),
            bracken: bracken(),
            git: git_binary().expect("git"),
            identity: "setter".into(),
            trace: false,
        },
    )
    .expect("hub");
    hub.ok(&request("setup")).expect("hub setup");
    let whole = Worker::bytes(&hub.ok(&request("export")).expect("export")).expect("bytes").len() as u64;

    let outcome = run(&config(dir.path(), 2, 4), &Claiming::new(false)).expect("the run");
    let log = events(&outcome.events);
    let moved = |agent: &str| -> Vec<u64> {
        log.iter()
            .filter(|e| e["event"] == "sync" && e["agent"] == agent)
            .map(|s| {
                s["data"]["up"]["bytes"].as_u64().expect("up") + s["data"]["down"]["bytes"].as_u64().expect("down")
            })
            .collect()
    };
    let (a0, a1) = (moved("agent-0"), moved("agent-1"));
    assert_eq!((a0.len(), a1.len()), (8, 8), "two syncs a turn");
    // A whole-Ground transfer would move at least the first Ground each time, and grow with it.
    assert!(a0.iter().chain(&a1).all(|&m| m < whole), "{a0:?} {a1:?}: the first Ground is {whole} bytes");
    assert!(a0[0] * 20 < whole, "nothing new moves next to nothing: {a0:?}");
    // Most syncs carry a claim or two; only those with the change requests carry more.
    let small = a0.iter().chain(&a1).filter(|&&m| m * 4 < whole).count();
    assert!(small >= 10, "{a0:?} {a1:?}: the first Ground is {whole} bytes");
    assert!(log.iter().all(|e| e["event"] != "turn_error"));
}

#[test]
fn one_agents_failure_stays_in_its_own_turn() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let backend = Claiming::new(true);
    let outcome = run(&config(dir.path(), 3, 2), &backend).expect("the run goes on");
    assert_eq!(backend.closed.load(std::sync::atomic::Ordering::SeqCst), 2, "the driver hung up on the over-long line");
    let log = events(&outcome.events);
    let count = |agent: &str, event: &str| log.iter().filter(|e| e["agent"] == agent && e["event"] == event).count();
    assert_eq!(count("agent-0", "turn_error"), 2, "each of agent 0's turns ended in an error");
    assert!(log.iter().filter(|e| e["event"] == "turn_error").all(|e| e["data"]["step"] == "play"));
    for agent in ["agent-0", "agent-1", "agent-2"] {
        assert_eq!(count(agent, "turn_end"), 2, "{agent} had all its turns");
    }
    for agent in ["agent-1", "agent-2"] {
        assert_eq!(count(agent, "turn_error"), 0);
        assert_eq!(count(agent, "tool_result"), 2, "{agent}'s calls went through");
    }
    assert_eq!(log.iter().filter(|e| e["event"] == "changes_released").count(), 1);
}

#[test]
fn a_crdt_mode_mock_run_goes_end_to_end_through_the_shim() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig {
        root: dir.path().join("run"),
        mode: "crdt",
        agents: 3,
        turns: 3,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox: false,
    };
    let outcome = run(&cfg, &MockBackend { seed: 5, wrong_rate: 0.0, shim: Some(bin()) }).expect("the run");
    assert_eq!(outcome.released_after, Some(4));
    let log = events(&outcome.events);
    assert!(!log.iter().any(|e| e["event"] == "turn_error"), "no turn failed: {log:?}");
    for agent in ["agent-0", "agent-1", "agent-2"] {
        assert_eq!(log.iter().filter(|e| e["agent"] == agent && e["event"] == "turn_end").count(), 3);
        // A sync at the start and end of every turn, and the mock's own.
        assert_eq!(log.iter().filter(|e| e["agent"] == agent && e["event"] == "sync").count(), 9, "{agent}");
    }
    // Every agent's edits reached the hub. Agents overlap on functions, and a later definition
    // replaces an earlier one, so there's one line per function written.
    let defs: Vec<&str> =
        outcome.modules.iter().flat_map(|(_, text)| text.lines()).filter(|l| l.starts_with("def ")).collect();
    let names: std::collections::BTreeSet<&str> = defs.iter().filter_map(|l| l.split_whitespace().nth(1)).collect();
    assert!(names.len() >= 4 && names.len() == defs.len(), "{defs:?}");
    // The hub's copy of the document is the only store any other store's data came from.
    let hub = dir.path().join("run/hub/doc.automerge");
    assert!(hub.exists());
    let library = Library::parse(outcome.modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
    let grades = grade(&library, Version::all(), &HiddenCases::new(1));
    assert_eq!(grades.len(), SPECS.len());
}

#[test]
fn one_agent_spending_its_hub_quota_leaves_anothers_sync_alone() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = |name: &str, identity: &str| StoreConfig {
        dir: dir.path().join(name),
        bracken: bracken(),
        git: git_binary().expect("git"),
        identity: identity.into(),
        trace: false,
    };
    let sockets = dir.path().join("s");
    std::fs::create_dir(&sockets).expect("sockets");
    let mut hub = Worker::spawn(&bin(), "bracken", &config("hub", "setter")).expect("hub");
    hub.ok(&request("setup")).expect("hub setup");
    let seed = Worker::bytes(&hub.ok(&request("export")).expect("export")).expect("bytes");
    let (mut copies, mut peers) = (Vec::new(), Vec::new());
    for name in ["a", "b"] {
        let mut w = Worker::spawn(&bin(), "bracken", &config(name, name)).expect("copy");
        let reply = w
            .ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })
            .expect("copy setup");
        peers.push(reply.result["key"].as_str().expect("a key").to_string());
        copies.push(w);
    }
    let claims = |w: &mut Worker, n: usize| {
        for k in 0..n {
            let args = serde_json::json!({"statement": format!("claim {k}"), "tags": ["note"]});
            w.ok(&Request { op: "tool".into(), tool: Some("claim".into()), args, ..Request::default() })
                .expect("claim");
        }
    };
    let session = |agent: usize| Session {
        agent,
        name: format!("agent-{agent}"),
        peers: peers.clone(),
        quota_contributions: 3,
        quota_bytes: 1 << 20,
        sync_seconds: 0,
    };
    // Agent a adds more than its quota: its sync is refused.
    claims(&mut copies[0], 5);
    assert!(sync_session(&mut hub, &mut copies[0], &sockets, &session(0)).is_err());
    // Agent b, within its own quota, still syncs, and the hub keeps its claims.
    claims(&mut copies[1], 3);
    sync_session(&mut hub, &mut copies[1], &sockets, &session(1)).expect("b's sync");
    let log = hub
        .ok(&Request {
            op: "tool".into(),
            tool: Some("log".into()),
            args: serde_json::json!({"tag": "note"}),
            ..Request::default()
        })
        .expect("the hub's log")
        .result;
    let by_b = log.as_array().expect("a list").iter().filter(|e| e["author"] == peers[1].as_str()).count();
    assert_eq!(by_b, 3);
}

/// Calls `notes_read` until refused, then once more.
struct Greedy;

impl Backend for Greedy {
    fn turn(&self, _agent: usize, _name: &str, _turn: u32, socket: &Path) -> Result<(), String> {
        let mut c = Client::connect(socket, None)?;
        let mut ok = 0;
        while c.tool("notes_read", serde_json::json!({})).is_ok() {
            ok += 1;
        }
        assert!(c.tool("notes_read", serde_json::json!({})).is_err());
        assert_eq!(ok, 3, "three calls a turn");
        Ok(())
    }
}

#[test]
fn the_driver_refuses_calls_past_the_turns_cap() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig { calls_per_turn: 3, ..config(dir.path(), 1, 2) };
    let outcome = run(&cfg, &Greedy).expect("the run");
    let log = events(&outcome.events);
    assert_eq!(log.iter().filter(|e| e["event"] == "tool_call").count(), 6);
    assert_eq!(log.iter().filter(|e| e["event"] == "call_refused").count(), 4);
    assert!(log.iter().all(|e| e["event"] != "turn_error"));
}

/// Every program git started in the stores under `dir`, by their traces, is git itself.
fn only_git_started(dir: &Path) -> usize {
    let mut programs = 0;
    for (path, _) in tree(dir).into_iter().filter(|(p, _)| p.ends_with("git-trace.log")) {
        let trace = std::fs::read_to_string(&path).expect("a trace");
        for cmd in trace.lines().filter_map(|l| l.split_once("trace: start_command: ")).map(|(_, c)| c) {
            let program = cmd.split_whitespace().next().unwrap_or_default();
            assert!(program.ends_with("/git-core/git"), "{}: git started {program}", path.display());
            programs += 1;
        }
    }
    programs
}

#[test]
fn a_git_mode_mock_run_goes_end_to_end_through_the_shim() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig { mode: "git", trace: true, ..config(dir.path(), 3, 4) };
    let outcome = run(&cfg, &MockBackend { seed: 3, wrong_rate: 0.0, shim: Some(bin()) }).expect("the run");
    let log = events(&outcome.events);
    let seqs: Vec<u64> = log.iter().map(|e| e["seq"].as_u64().expect("seq")).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "one writer, no gaps");
    assert!(log.iter().all(|e| e["event"] != "turn_error"), "no turn failed");
    assert_eq!(log.iter().filter(|e| e["event"] == "changes_released").count(), 1);
    for agent in ["agent-0", "agent-1", "agent-2"] {
        assert_eq!(log.iter().filter(|e| e["agent"] == agent && e["event"] == "turn_end").count(), 4);
    }
    // Calls fail only where two agents race to review or merge the same PR.
    for e in log.iter().filter(|e| e["event"] == "tool_result" && e["data"]["ok"] == false) {
        assert!(
            ["`pr_merge`", "`pr_review`", "`pr_open`"].contains(&e["data"]["tool"].as_str().unwrap_or_default()),
            "{e}"
        );
    }
    // Merged work is on main, correct, and every function once.
    let library = Library::parse(outcome.modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
    assert!(library.broken.is_empty() && library.duplicates.is_empty());
    // Main keeps what merged before the change requests, unlike a superseded Bracken spec: each
    // function is correct as of the original contract or the changed one.
    let (before, after) = (
        grade(&library, Version::ORIGINAL, &HiddenCases::new(1)),
        grade(&library, Version::all(), &HiddenCases::new(1)),
    );
    assert!(after.iter().filter(|g| g.present).count() >= 2, "PRs were merged");
    for (b, a) in before.iter().zip(&after).filter(|(_, a)| a.present) {
        assert!(b.standalone || a.standalone, "{} is a correct program", a.name);
    }
    // No worker's git started anything but git.
    assert!(only_git_started(&dir.path().join("run")) > 0);
}

fn git_store(dir: &Path, name: &str) -> StoreConfig {
    StoreConfig {
        dir: dir.join(name),
        bracken: bracken(),
        git: git_binary().expect("git"),
        identity: name.into(),
        trace: true,
    }
}

#[test]
fn each_git_worker_writes_only_its_own_store_and_the_hub_takes_only_bundles() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut hub = Worker::spawn(&bin(), "git", &git_store(dir.path(), "hub")).expect("hub");
    hub.ok(&request("setup")).expect("hub setup");
    let seed = Worker::bytes(&hub.ok(&request("export")).expect("export")).expect("bytes");
    let mut a = Worker::spawn(&bin(), "git", &git_store(dir.path(), "agent-a")).expect("a");
    let mut b = Worker::spawn(&bin(), "git", &git_store(dir.path(), "agent-b")).expect("b");
    for w in [&mut a, &mut b] {
        w.ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })
            .expect("copy setup");
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    let now = |n: &str| tree(&dir.path().join(n));
    let (hub0, b0) = (now("hub"), now("agent-b"));
    let tool = |w: &mut Worker, name: &str, args: Value| {
        w.ok(&Request { op: "tool".into(), tool: Some(name.into()), args, ..Request::default() }).expect(name)
    };
    tool(&mut a, "switch", serde_json::json!({"branch": "abs", "create": true}));
    tool(&mut a, "edit", serde_json::json!({"file": "base.stk", "start": 1, "end": 0, "text": "def abs 1 : a"}));
    tool(&mut a, "commit", serde_json::json!({"message": "abs"}));
    assert_eq!(now("hub"), hub0);
    assert_eq!(now("agent-b"), b0);

    // A sync's halves: each side writes only itself.
    let session = Session { agent: 0, name: "agent-a".into(), quota_bytes: 1 << 20, ..Session::default() };
    let a1 = now("agent-a");
    let push = a.ok(&request("push")).expect("push");
    let args = serde_json::json!({"session": session, "meta": push.result});
    hub.ok(&Request { op: "receive".into(), args, bytes: push.bytes, ..Request::default() }).expect("receive");
    assert_ne!(now("hub"), hub0);
    assert_eq!(now("agent-b"), b0);
    let pushed = now("agent-a");
    assert!(pushed.keys().all(|p| a1.contains_key(p) || p.starts_with(dir.path().join("agent-a/files"))));

    // A gitfile, or anything that isn't a bundle, sent as a push is refused, and git isn't started.
    let hub1 = now("hub");
    let gitfile = format!("gitdir: {}\n", dir.path().join("agent-a/work/.git").display());
    for bytes in [gitfile.into_bytes(), b"PACK\0\0\0\x02".to_vec(), b"#!/bin/sh\nexit 0\n".to_vec()] {
        let args = serde_json::json!({"session": session, "meta": {"branches": {}}});
        let reply =
            hub.call(&Request { op: "receive".into(), args, bytes: Some(hex::encode(&bytes)), ..Request::default() });
        let reply = reply.expect("a reply");
        assert!(!reply.ok && reply.error.is_some_and(|e| e.contains("not a git bundle")));
    }
    // Only the quota record changed (refused pushes count); git didn't run, so its trace is unchanged.
    let records = dir.path().join("hub/received.json");
    let without = |t: BTreeMap<PathBuf, (u64, SystemTime)>| {
        t.into_iter().filter(|(p, _)| *p != records).collect::<BTreeMap<_, _>>()
    };
    assert_eq!(without(now("hub")), without(hub1));
    assert!(std::fs::read_to_string(&records).expect("records").contains("agent-a"));
    assert!(only_git_started(dir.path()) > 0);
}

/// Connects, then says nothing until the driver hangs up.
struct Silent;

impl Backend for Silent {
    fn turn(&self, _agent: usize, _name: &str, _turn: u32, socket: &Path) -> Result<(), String> {
        use std::io::Read;
        let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
        let _ = s.read(&mut [0u8; 1]);
        Ok(())
    }
}

#[test]
fn a_silent_agent_ends_at_the_turns_deadline() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig { turn_limit: std::time::Duration::from_secs(2), ..config(dir.path(), 1, 2) };
    let started = std::time::Instant::now();
    let outcome = run(&cfg, &Silent).expect("the run ends");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    let log = events(&outcome.events);
    let errors = log.iter().filter(|e| e["event"] == "turn_error" && e["data"]["step"] == "play").count();
    assert_eq!(errors, 2, "each turn ended at its deadline");
    assert_eq!(log.iter().filter(|e| e["event"] == "turn_end").count(), 2);
}

/// Sends pings and never reads a reply, until the driver hangs up.
struct Deaf;

impl Backend for Deaf {
    fn turn(&self, _agent: usize, _name: &str, _turn: u32, socket: &Path) -> Result<(), String> {
        use std::io::Write;
        let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| e.to_string())?;
        let ping = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n";
        while s.write_all(ping).is_ok() {}
        Ok(())
    }
}

#[test]
fn an_agent_that_stops_reading_ends_at_the_turns_deadline() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let cfg = RunConfig { turn_limit: std::time::Duration::from_secs(3), ..config(dir.path(), 1, 1) };
    let started = std::time::Instant::now();
    let outcome = run(&cfg, &Deaf).expect("the run ends");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    let log = events(&outcome.events);
    assert_eq!(log.iter().filter(|e| e["event"] == "turn_error" && e["data"]["step"] == "play").count(), 1);
}
