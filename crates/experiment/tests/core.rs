//! The core and Bracken mode end to end (docs/design/phase1-experiment.md,
//! sections 2 and 3): the driver, its workers, the shim and the mock backend.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::Value;

use bracken_experiment::driver::{Backend, MAX_MCP_LINE, RunConfig, run};
use bracken_experiment::grader::{HiddenCases, grade};
use bracken_experiment::lang::Library;
use bracken_experiment::mock::{Client, MockBackend};
use bracken_experiment::mode::StoreConfig;
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
        identity: identity.into(),
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
    RunConfig { root: dir.join("run"), mode: "bracken", agents, turns, worker_bin: bin(), bracken: bracken() }
}

#[test]
fn sync_moves_only_what_is_missing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut hub = Worker::spawn(
        &bin(),
        "bracken",
        &StoreConfig { dir: dir.path().join("hub"), bracken: bracken(), identity: "setter".into() },
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
