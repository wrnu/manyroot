//! The agent backend through its harness interface, without Claude Code: a mock harness plays each turn
//! with `/bin/bash`, pinned and under the agent sandbox profile like any harness, writing events in a line
//! format of its own, so what the backend does (the process it builds, the pin and configuration checks,
//! the gate, token accounting, caps, process groups, transcripts, halting) is checked apart from any one
//! harness.

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use bracken_experiment::backend::{
    BackendConfig, Caps, Event, Harness, HarnessBackend, Launch, Pin, RUN_TIME_CAP, RUN_TOKEN_CAP, Tokens, Turn,
};
use bracken_experiment::driver::Backend;
use bracken_experiment::proxy::Route;

const SECRET: &str = "mock-secret-not-a-real-one-0123";

/// A harness whose turn is `script`, run by `/bin/bash` with the turn message as `$1`. Its events, a line
/// each: `init ok` or `init <why not>`; `call <id> <input> <output> [<call id>:<tool>...]`;
/// `result <input> <output>`; anything else is other.
struct Mock {
    script: String,
    pin: Pin,
    secret: String,
    /// What it declares, and what it sets each turn.
    names: Vec<&'static str>,
    env: Vec<(&'static str, String)>,
}

impl Mock {
    fn new(script: &str) -> Mock {
        Mock {
            script: script.into(),
            pin: Pin::of(Path::new("/bin/bash"), "mock").expect("a pin"),
            secret: SECRET.into(),
            names: vec!["MOCK_SET", "MOCK_CONF"],
            env: vec![("MOCK_SET", "yes".into())],
        }
    }
}

fn number(word: Option<&str>) -> Result<u64, String> {
    word.and_then(|w| w.parse().ok()).ok_or_else(|| "an unreadable event".to_string())
}

impl Harness for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn route(&self) -> Route {
        Route { host: "llm.mock.invalid".into(), port: 443, connect: None }
    }

    fn secret(&self) -> &str {
        &self.secret
    }

    fn pin(&self) -> &Pin {
        &self.pin
    }

    fn env_names(&self) -> &[&str] {
        &self.names
    }

    fn config(&self, name: &str, _socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        Ok(vec![("mock.conf", format!("conf for {name}").into_bytes())])
    }

    fn launch(&self, turn: &Turn) -> Result<Launch, String> {
        let mut l = Launch::default();
        l.args(["-c", &self.script, "bash", turn.message]);
        for (k, v) in &self.env {
            l.env(k, v);
        }
        l.env("MOCK_CONF", turn.config);
        Ok(l)
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
    socket: PathBuf,
    backend: HarnessBackend<Mock>,
}

fn backend_config(root: &Path, caps: Caps) -> BackendConfig {
    BackendConfig {
        caps,
        dir: root.join("backend"),
        shim: PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment")),
        turns: 4,
    }
}

impl Setup {
    fn new(script: &str, caps: Caps) -> Setup {
        Setup::with(Mock::new(script), caps)
    }

    fn with(mock: Mock, caps: Caps) -> Setup {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let cfg = backend_config(&root, caps);
        let dir = cfg.dir.clone();
        let backend = HarnessBackend::start(mock, cfg).expect("the backend");
        Setup { _tmp: tmp, dir, socket: root.join("agent.sock"), backend }
    }

    /// Plays agent-0's turn `turn`. The mock never talks to the driver, so its socket needn't exist.
    fn turn(&self, turn: u32) -> Result<(), String> {
        self.backend.turn(0, "agent-0", turn, &self.socket)
    }

    fn usage(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("usage.jsonl")).expect("usage.jsonl");
        text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
    }

    fn transcript(&self, turn: u32) -> Option<String> {
        std::fs::read_to_string(self.dir.join(format!("transcripts/agent-0-{turn}.jsonl"))).ok()
    }

    /// The lines of a turn's transcript that start with `word `, without it.
    fn said(&self, turn: u32, word: &str) -> Vec<String> {
        let t = self.transcript(turn).expect("a transcript");
        t.lines().filter_map(|l| l.strip_prefix(&format!("{word} ")).map(String::from)).collect()
    }
}

fn caps() -> Caps {
    Caps { turn_time: Duration::from_secs(20), ..Caps::pilot() }
}

/// A loop that runs until its process group is killed, without starting a program (the sandbox lets the
/// mock start only its pinned bash).
const FOREVER: &str = "while :; do :; done";

/// Says what standard input is, `stdin null` or `stdin other`, from what fd 0 is rather than whether it
/// delivers data: an inherited stdin that sends nothing would look empty too (finding c758214ceca6).
const STDIN_CHECK: &str = "if [[ /dev/stdin -ef /dev/null ]]; then echo \"stdin null\"; else echo \"stdin other\"; fi";

#[test]
fn the_stdin_check_tells_null_from_an_inherited_stdin() {
    // The control: the same check, unsandboxed, says other for a pipe nothing is written to, as an
    // inherited stdin would be without .stdin(Stdio::null()), and null for null.
    for (stdin, want) in [(Stdio::piped(), "stdin other\n"), (Stdio::null(), "stdin null\n")] {
        let out = Command::new("/bin/bash").args(["-c", STDIN_CHECK]).stdin(stdin).output().expect("bash");
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
    }
}

#[test]
fn an_entry_the_harness_didnt_declare_stops_the_run_before_the_turn_starts() {
    let mock = Mock { env: vec![("MOCK_SET", "yes".into()), ("SNEAKY", "1".into())], ..Mock::new("echo init ok") };
    let s = Setup::with(mock, caps());
    assert_eq!(s.turn(1), Err("mock set `SNEAKY`, an environment entry it didn't declare".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
    assert!(s.backend.halted().is_some());
}

#[test]
fn a_harness_may_not_declare_the_backends_own_entries() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    for name in ["HOME", "PATH", "TMPDIR", "HTTPS_PROXY"] {
        let mock = Mock { names: vec![name], env: vec![], ..Mock::new("echo init ok") };
        let cfg = backend_config(tmp.path(), caps());
        let err = HarnessBackend::start(mock, cfg).err().expect("refused");
        assert_eq!(err, format!("mock declares {name}, which the backend sets"));
    }
    assert!(!tmp.path().join("backend").exists(), "refused before anything was made");
}

#[test]
fn a_secret_under_8_bytes_is_refused_at_the_start() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    for secret in ["", "a", "1234567"] {
        let mock = Mock { secret: secret.into(), ..Mock::new("echo init ok") };
        let err = HarnessBackend::start(mock, backend_config(tmp.path(), caps())).err().expect("refused");
        assert!(err.contains(&format!("{} bytes, under 8", secret.len())), "{err}");
    }
    let mock = Mock { secret: "12345678".into(), ..Mock::new("echo init ok") };
    HarnessBackend::start(mock, backend_config(tmp.path(), caps())).expect("8 bytes is enough");
}

#[test]
fn a_changed_pin_stops_the_run_before_the_turn_starts() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let program = tmp.path().join("program");
    std::fs::write(&program, "one").expect("a program");
    let mock = Mock { pin: Pin::of(&program, "mock").expect("a pin"), ..Mock::new("echo init ok") };
    let s = Setup::with(mock, caps());
    std::fs::write(&program, "two").expect("changed");
    assert_eq!(s.turn(1), Err("the pinned mock binary changed".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
    assert!(s.backend.halted().is_some());
}

/// The tests that play a turn, which the backend runs under the agent profile: they need macOS, for
/// sandbox-exec. Linux ignores them, and `scripts/test-table --split` runs them on the Mac by this
/// module's name (owner decision `01a2d668b67e`).
mod macos_only {
    use super::*;

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_turn_is_started_gated_counted_and_kept() {
        let script = "echo init ok; echo \"note $1\"; echo call m1 1000 5 t1:notes_read; \
                      echo call m1 1000 9 t1:notes_read; echo call m2 1200 20; echo result 2200 40; echo warn >&2";
        let s = Setup::new(script, caps());
        s.turn(1).expect("the turn");
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
        assert_eq!(
            (mine.input, mine.output),
            (2200, 40),
            "a call's repeated usage counts once; the result's output wins"
        );
        assert_eq!(mine, run);
        assert_eq!(s.said(1, "note"), ["This is your turn 1 of 4. You are agent-0. Start by reading your notes."]);
        let stderr = std::fs::read_to_string(s.dir.join("transcripts/agent-0-1.stderr")).expect("stderr");
        assert_eq!(stderr, "warn\n");
        assert!(s.backend.halted().is_none());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_backend_builds_the_environment_and_the_harness_adds_only_what_it_declared() {
        let script = format!(
            "echo init ok; for n in $(compgen -e); do echo \"name $n\"; done; \
             echo \"home $HOME\"; echo \"tmp $TMPDIR\"; echo \"proxy $HTTPS_PROXY\"; echo \"path $PATH\"; \
             echo \"set $MOCK_SET\"; {STDIN_CHECK}; echo result 0 1"
        );
        let s = Setup::new(&script, caps());
        s.turn(1).expect("the turn");
        // bash sets PWD, SHLVL and _ itself, and OLDPWD where it has one.
        let shell = ["PWD", "SHLVL", "_", "OLDPWD"];
        let names: BTreeSet<String> = s.said(1, "name").into_iter().filter(|n| !shell.contains(&n.as_str())).collect();
        let want: BTreeSet<String> =
            ["HOME", "TMPDIR", "PATH", "HTTPS_PROXY", "MOCK_SET", "MOCK_CONF"].iter().map(|n| n.to_string()).collect();
        assert_eq!(names, want, "nothing of the driver's environment, only the base and the declared entries");
        let own = s.dir.join("agents/agent-0");
        assert_eq!(s.said(1, "home"), [own.join("home").display().to_string()]);
        assert_eq!(s.said(1, "tmp"), [own.join("tmp").display().to_string()]);
        assert_eq!(s.said(1, "proxy"), [format!("http://127.0.0.1:{}", s.backend.proxy_port())]);
        assert_eq!(s.said(1, "path"), ["/usr/bin:/bin"]);
        assert_eq!(s.said(1, "set"), ["yes"]);
        assert_eq!(s.said(1, "stdin"), ["null"], "standard input is /dev/null, not the driver's");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_secret_of_8_bytes_plays_a_turn() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let mock = Mock { secret: "12345678".into(), ..Mock::new("echo init ok; echo result 0 1") };
        let backend = HarnessBackend::start(mock, backend_config(tmp.path(), caps())).expect("8 bytes is enough");
        backend.turn(0, "agent-0", 1, &tmp.path().join("agent.sock")).expect("the turn");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_turn_runs_in_the_agent_sandbox() {
        let script = "echo init ok; \
                      if /usr/bin/true 2>/dev/null; then echo exec allowed; else echo exec refused; fi; \
                      if (echo x > \"$MOCK_CONF/new\") 2>/dev/null; then echo write allowed; else echo write refused; fi; \
                      if (echo x > \"$HOME/new\") 2>/dev/null; then echo own allowed; else echo own refused; fi; \
                      read -r conf < \"$MOCK_CONF/mock.conf\"; echo \"conf $conf\"; echo result 0 1";
        // Without the sandbox, the same attempts work (gotcha fd7742fa71ce: show the refusal is the sandbox's).
        let tmp = tempfile::tempdir().expect("a temp dir");
        std::fs::write(tmp.path().join("mock.conf"), "conf").expect("a conf");
        let out = Command::new("/bin/bash")
            .args(["-c", script])
            .env("MOCK_CONF", tmp.path())
            .env("HOME", tmp.path())
            .stdin(Stdio::null())
            .output()
            .expect("bash");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("exec allowed") && text.contains("write allowed"), "{text}");

        let s = Setup::new(script, caps());
        s.turn(1).expect("the turn");
        assert_eq!(s.said(1, "exec"), ["refused"], "only the pinned program and the shim may start");
        assert_eq!(s.said(1, "write"), ["refused"], "the turn configuration is read-only");
        assert_eq!(s.said(1, "own"), ["allowed"], "the agent's own directory is writable");
        assert_eq!(s.said(1, "conf"), ["conf for agent-0"], "the configuration is readable");
        let conf = s.dir.join("turn/agent-0/mock.conf");
        let mode = std::fs::metadata(&conf).expect("the conf").permissions().mode();
        assert_eq!(mode & 0o777, 0o400);
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_changed_configuration_stops_the_run_before_the_turn_starts() {
        let s = Setup::new("echo init ok; echo result 0 1", caps());
        s.turn(1).expect("the first turn");
        let conf = s.dir.join("turn/agent-0/mock.conf");
        std::fs::set_permissions(&conf, std::fs::Permissions::from_mode(0o600)).expect("writable");
        std::fs::write(&conf, "changed").expect("changed");
        assert_eq!(s.turn(2), Err("the turn configuration's mock.conf changed".into()));
        assert!(s.transcript(2).is_none(), "nothing started");
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_refused_gate_stops_the_run() {
        let s = Setup::new("echo init the model is wrong; echo result 0 1", caps());
        let err = s.turn(1).expect_err("refused");
        assert_eq!(err, "the gate refused agent-0's turn 1: the model is wrong");
        assert_eq!(s.usage()[0]["gate"], false);
        assert_eq!(s.backend.halted(), Some(err.clone()));
        assert_eq!(s.turn(2), Err(err), "no further turn");
        assert!(s.transcript(2).is_none());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_first_event_that_isnt_init_stops_the_run() {
        let s = Setup::new("echo call m1 1 1; echo result 0 1", caps());
        let err = s.turn(1).expect_err("refused");
        assert!(err.contains("the first event isn't init"), "{err}");
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_turn_without_a_result_counts_at_its_output_cap() {
        let s = Setup::new("echo init ok; echo call m1 100 50", caps());
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended without a result".into()));
        assert_eq!(s.backend.totals("agent-0").0.output, caps().turn_output);
        assert!(s.usage().iter().any(|r| r["output_counted_at"] == caps().turn_output));
        assert!(s.backend.halted().is_none(), "only the turn ends");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn an_unreadable_event_stops_the_run() {
        let s = Setup::new("echo init ok; echo call m1 lots 1", caps());
        assert_eq!(s.turn(1), Err("an unreadable event".into()));
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_stream_and_result_that_disagree_by_over_10_percent_stop_the_run() {
        // The gap is measured against the larger figure: 112 of 1112 is over a tenth, 111 of 1111 isn't.
        let s = Setup::new("echo init ok; echo call m1 1000 1; echo result 1112 1", caps());
        let err = s.turn(1).expect_err("stopped");
        assert_eq!(err, "agent-0's turn 1: the stream and the result disagree on input by over 10%");
        assert_eq!(s.backend.totals("agent-0").0.input, 1112, "the larger is counted before the stop");
        assert!(s.backend.halted().is_some());

        let s = Setup::new("echo init ok; echo call m1 1000 1; echo result 1111 1", caps());
        s.turn(1).expect("just within");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_secret_is_never_kept() {
        let s = Setup::new(&format!("echo init ok; echo {SECRET}; echo result 0 1"), caps());
        let err = s.turn(1).expect_err("stopped");
        assert_eq!(err, "the event stream held the auth value");
        assert!(!s.transcript(1).expect("the lines before it").contains(SECRET));
        assert!(s.backend.halted().is_some());

        let s = Setup::new(&format!("echo init ok; echo {SECRET} >&2; echo result 0 1"), caps());
        let err = s.turn(1).expect_err("stopped");
        assert!(err.starts_with("a transcript held the auth value"), "{err}");
        assert!(s.transcript(1).is_none(), "removed");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_tool_call_cap_gives_grace_then_ends_the_turn() {
        let caps = Caps { calls_per_turn: 1, cap_grace_calls: 1, ..caps() };
        let script =
            format!("echo init ok; echo call m1 10 1 a:x b:y; echo call m2 10 1; echo call m3 10 1; {FOREVER}");
        let s = Setup::new(&script, caps);
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its tool calls cap".into()));
        assert!(s.usage().iter().any(|r| r["past_tool_call_cap"] == 2));
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_cap_kills_the_harness_and_everything_it_started() {
        let script = format!("({FOREVER}) & echo $! > \"$HOME/pid\"; echo init ok; echo call m1 200000 1; wait");
        let s = Setup::new(&script, caps());
        let start = Instant::now();
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its context cap".into()));
        assert!(start.elapsed() < Duration::from_secs(60), "{:?}", start.elapsed());
        let pid = std::fs::read_to_string(s.dir.join("agents/agent-0/home/pid")).expect("the pid");
        let alive = || {
            let kill = Command::new("/bin/kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status();
            kill.expect("kill").success()
        };
        let gone = Instant::now() + Duration::from_secs(30);
        while alive() && Instant::now() < gone {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(), "the loop the harness started was killed with its group");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn spent_tokens_end_an_agents_turns_and_the_runs_stop_the_run() {
        let script = format!("echo init ok; echo call m1 60 50; {FOREVER}");
        let s = Setup::new(&script, Caps { agent_tokens: 100, ..caps() });
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its agent tokens cap".into()));
        assert_eq!(s.turn(2), Err("agent-0 has spent its tokens".into()));
        assert!(s.backend.halted().is_none(), "the run goes on");

        let s = Setup::new(&script, Caps { run_tokens: 100, ..caps() });
        assert_eq!(s.turn(1), Err(RUN_TOKEN_CAP.into()));
        assert_eq!(s.backend.halted(), Some(RUN_TOKEN_CAP.into()));
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_run_time_cap_stops_a_turn_and_the_run() {
        let s = Setup::new(&format!("echo init ok; {FOREVER}"), Caps { run_time: Duration::from_secs(2), ..caps() });
        let start = Instant::now();
        assert_eq!(s.turn(1), Err(RUN_TIME_CAP.into()));
        assert!(start.elapsed() < Duration::from_secs(60), "{:?}", start.elapsed());
        assert_eq!(s.backend.halted(), Some(RUN_TIME_CAP.into()));
        assert_eq!(s.turn(2), Err(RUN_TIME_CAP.into()));
        assert!(s.transcript(2).is_none());
    }
}
