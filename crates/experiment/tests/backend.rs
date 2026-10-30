//! The agent backend through its harness interface, without Claude Code: a mock harness plays each turn
//! with `/bin/sh`, writing events in a line format of its own, so what the backend does (the gate, token
//! accounting, caps, process groups, transcripts, halting) is checked apart from any one harness.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use bracken_experiment::backend::{BackendConfig, Caps, Event, Harness, HarnessBackend, RUN_TOKEN_CAP, Tokens, Turn};
use bracken_experiment::driver::Backend;
use bracken_experiment::proxy::Route;

const SECRET: &str = "mock-secret-not-a-real-one-0123";

/// A harness whose turn is `script`, run by `/bin/sh` with the turn message as `$1`. Its events, a line each:
/// `init ok` or `init <why not>`; `call <id> <input> <output> [<call id>:<tool>...]`; `result <input> <output>`;
/// anything else is other.
struct Mock {
    script: String,
    checks: AtomicU32,
    refuse: AtomicBool,
}

fn number(word: Option<&str>) -> Result<u64, String> {
    word.and_then(|w| w.parse().ok()).ok_or_else(|| "an unreadable event".to_string())
}

impl Harness for Mock {
    fn name(&self) -> &str {
        "the mock harness"
    }

    fn route(&self) -> Route {
        Route { host: "llm.mock.invalid".into(), port: 443, connect: None }
    }

    fn secret(&self) -> &str {
        SECRET
    }

    fn setup(&self) -> Result<(), String> {
        Ok(())
    }

    fn check(&self, _turn: &Turn) -> Result<(), String> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.refuse.load(Ordering::SeqCst) { Err("the mock's binary changed".into()) } else { Ok(()) }
    }

    fn command(&self, turn: &Turn) -> Result<Command, String> {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &self.script, "sh", turn.message])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }

    fn event(&self, line: &str, first: bool, _turn: &Turn) -> Result<Event, String> {
        let mut words = line.split(' ');
        let kind = words.next().unwrap_or_default();
        if first {
            let why: Vec<&str> = words.collect();
            return Ok(match (kind, why.as_slice()) {
                ("init", ["ok"]) => Event::Init(Ok(())),
                ("init", why) => Event::Init(Err(why.join(" "))),
                _ => Event::Other,
            });
        }
        match kind {
            "call" => {
                let id = words.next().ok_or("an unreadable event")?.to_string();
                let usage = Tokens { input: number(words.next())?, output: number(words.next())?, ..Tokens::default() };
                let tools = words
                    .map(|w| w.split_once(':').map(|(i, n)| (i.into(), n.into())).ok_or("an unreadable event"))
                    .collect::<Result<_, _>>()?;
                Ok(Event::Message { id, model: json!("mock"), usage, tools })
            }
            "result" => {
                let usage = Tokens { input: number(words.next())?, output: number(words.next())?, ..Tokens::default() };
                Ok(Event::Result { usage, ended: json!("success"), cost_usd: json!(0.5), duration_ms: json!(7) })
            }
            _ => Ok(Event::Other),
        }
    }
}

struct Setup {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    backend: HarnessBackend<Mock>,
}

impl Setup {
    fn new(script: &str, caps: Caps) -> Setup {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = std::fs::canonicalize(tmp.path()).expect("resolved").join("backend");
        let cfg = BackendConfig {
            caps,
            dir: dir.clone(),
            guard: PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment")),
            turns: 4,
        };
        let mock = Mock { script: script.into(), checks: AtomicU32::new(0), refuse: AtomicBool::new(false) };
        let backend = HarnessBackend::start(mock, cfg).expect("the backend");
        Setup { _tmp: tmp, dir, backend }
    }

    /// Plays agent-0's turn `turn`. The mock never talks to the driver, so no socket is needed.
    fn turn(&self, turn: u32) -> Result<(), String> {
        self.backend.turn(0, "agent-0", turn, Path::new("/nonexistent/socket"))
    }

    fn usage(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("usage.jsonl")).expect("usage.jsonl");
        text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
    }

    fn transcript(&self, turn: u32) -> Option<String> {
        std::fs::read_to_string(self.dir.join(format!("transcripts/agent-0-{turn}.jsonl"))).ok()
    }
}

fn caps() -> Caps {
    Caps { turn_time: Duration::from_secs(20), ..Caps::pilot() }
}

#[test]
fn a_turn_is_started_gated_counted_and_kept() {
    let script = "echo init ok; echo \"note $1\"; echo call m1 1000 5 t1:notes_read; \
                  echo call m1 1000 9 t1:notes_read; echo call m2 1200 20; echo result 2200 40; echo warn >&2";
    let s = Setup::new(script, caps());
    s.turn(1).expect("the turn");
    assert_eq!(s.backend.harness().checks.load(Ordering::SeqCst), 1, "checked before the turn");
    let rows = s.usage();
    assert_eq!(rows[0], json!({"agent": "agent-0", "turn": 1, "gate": true}));
    let calls: Vec<_> = rows.iter().filter(|r| r["message"].is_string()).collect();
    assert_eq!(calls.len(), 3, "every call event is recorded: {rows:?}");
    assert_eq!(calls[0]["tools"], json!(["notes_read"]));
    let settled = rows.last().expect("a settled row");
    assert_eq!(
        (&settled["ended"], &settled["input"], &settled["output"]),
        (&json!("success"), &json!(2200), &json!(40))
    );
    assert_eq!(settled["cost_usd"], 0.5);
    let (mine, run) = s.backend.totals("agent-0");
    assert_eq!((mine.input, mine.output), (2200, 40), "a call's repeated usage counts once; the result's output wins");
    assert_eq!(mine, run);
    let transcript = s.transcript(1).expect("a transcript");
    assert!(transcript.contains("note This is your turn 1 of 4. You are agent-0."), "{transcript}");
    let stderr = std::fs::read_to_string(s.dir.join("transcripts/agent-0-1.stderr")).expect("stderr");
    assert_eq!(stderr, "warn\n");
    assert!(s.backend.halted().is_none());
}

#[test]
fn a_refused_gate_stops_the_run() {
    let s = Setup::new("echo init the model is wrong; echo result 1 1", caps());
    let err = s.turn(1).expect_err("refused");
    assert_eq!(err, "the gate refused agent-0's turn 1: the model is wrong");
    assert_eq!(s.usage()[0]["gate"], false);
    assert_eq!(s.backend.halted(), Some(err.clone()));
    assert_eq!(s.turn(2), Err(err), "no further turn");
    assert!(s.transcript(2).is_none());
}

#[test]
fn a_first_event_that_isnt_init_stops_the_run() {
    let s = Setup::new("echo call m1 1 1; echo result 1 1", caps());
    let err = s.turn(1).expect_err("refused");
    assert!(err.contains("the first event isn't init"), "{err}");
    assert!(s.backend.halted().is_some());
}

#[test]
fn a_turn_without_a_result_counts_at_its_output_cap() {
    let s = Setup::new("echo init ok; echo call m1 100 50", caps());
    assert_eq!(s.turn(1), Err("agent-0's turn 1 ended without a result".into()));
    assert_eq!(s.backend.totals("agent-0").0.output, caps().turn_output);
    assert!(s.usage().iter().any(|r| r["output_counted_at"] == caps().turn_output));
    assert!(s.backend.halted().is_none(), "only the turn ends");
}

#[test]
fn an_unreadable_event_stops_the_run() {
    let s = Setup::new("echo init ok; echo call m1 lots 1", caps());
    assert_eq!(s.turn(1), Err("an unreadable event".into()));
    assert!(s.backend.halted().is_some());
}

#[test]
fn the_secret_is_never_kept() {
    let s = Setup::new(&format!("echo init ok; echo {SECRET}; echo result 1 1"), caps());
    let err = s.turn(1).expect_err("stopped");
    assert_eq!(err, "the event stream held the auth value");
    assert!(!s.transcript(1).expect("the lines before it").contains(SECRET));
    assert!(s.backend.halted().is_some());

    let s = Setup::new(&format!("echo init ok; echo {SECRET} >&2; echo result 1 1"), caps());
    let err = s.turn(1).expect_err("stopped");
    assert!(err.starts_with("a transcript held the auth value"), "{err}");
    assert!(s.transcript(1).is_none(), "removed");
}

#[test]
fn a_failed_check_stops_the_run_before_the_turn_starts() {
    let s = Setup::new("echo init ok; echo result 1 1", caps());
    s.backend.harness().refuse.store(true, Ordering::SeqCst);
    assert_eq!(s.turn(1), Err("the mock's binary changed".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
    assert!(s.backend.halted().is_some());
}

#[test]
fn the_tool_call_cap_gives_grace_then_ends_the_turn() {
    let caps = Caps { calls_per_turn: 1, cap_grace_calls: 1, ..caps() };
    let s =
        Setup::new("echo init ok; echo call m1 10 1 a:x b:y; echo call m2 10 1; echo call m3 10 1; sleep 600", caps);
    assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its tool calls cap".into()));
    assert!(s.usage().iter().any(|r| r["past_tool_call_cap"] == 2));
}

#[test]
fn a_cap_kills_the_harness_and_everything_it_started() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let pid_file = tmp.path().join("pid");
    let script = format!("sleep 600 & echo $! > {}; echo init ok; echo call m1 200000 1; wait", pid_file.display());
    let s = Setup::new(&script, caps());
    let start = Instant::now();
    assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its context cap".into()));
    assert!(start.elapsed() < Duration::from_secs(60), "{:?}", start.elapsed());
    let pid = std::fs::read_to_string(&pid_file).expect("the pid");
    let alive = || {
        let kill = Command::new("/bin/kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status();
        kill.expect("kill").success()
    };
    let gone = Instant::now() + Duration::from_secs(30);
    while alive() && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(), "the sleep the harness started was killed with its group");
}

#[test]
fn spent_tokens_end_an_agents_turns_and_the_runs_stop_the_run() {
    let script = "echo init ok; echo call m1 60 50; sleep 600";
    let s = Setup::new(script, Caps { agent_tokens: 100, ..caps() });
    assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its agent tokens cap".into()));
    assert_eq!(s.turn(2), Err("agent-0 has spent its tokens".into()));
    assert!(s.backend.halted().is_none(), "the run goes on");

    let s = Setup::new(script, Caps { run_tokens: 100, ..caps() });
    assert_eq!(s.turn(1), Err(RUN_TOKEN_CAP.into()));
    assert_eq!(s.backend.halted(), Some(RUN_TOKEN_CAP.into()));
}
