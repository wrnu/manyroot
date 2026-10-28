//! The agent driver (docs/design/phase1-experiment.md, build step 6): the
//! sandbox profiles, the forwarding proxy, the gate, token accounting and
//! caps, process groups and transcripts. Every test uses the fake `claude`
//! (`experiment-fake-claude`) and fake routing values; no model is called and
//! no real credential is read. The one test that runs the real Claude Code
//! binary is ignored by default (`real_claude_code_reaches_init_and_first_request`).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use bracken_experiment::claude::{Caps, ClaudeBackend, ClaudeConfig, Expect, Pin, Routing, Tokens};
use bracken_experiment::driver::{Backend, CALLS_PER_TURN, RunConfig, TURN_LIMIT, core_tools, git_binary, run};
use bracken_experiment::mode::mode_named;
use bracken_experiment::sandbox::{Git, Sandbox, WorkerPaths, resolved};

const FAKE_AUTH: &str = "fake-auth-value-not-a-secret-0123456789";
const HOST: &str = "llm.fake.invalid";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn fake() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_experiment-fake-claude"))
}

fn bracken() -> PathBuf {
    let b = bin().with_file_name("bracken");
    assert!(b.exists(), "build bracken first: cargo build -p bracken");
    b
}

/// A run of `mode` in `root`, workers sandboxed if `sandbox`.
fn run_config(root: PathBuf, mode: &'static str, agents: usize, turns: u32, sandbox: bool) -> RunConfig {
    RunConfig {
        root,
        mode,
        agents,
        turns,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox,
    }
}

fn routing() -> Routing {
    Routing::new(vec![
        ("ANTHROPIC_BASE_URL".into(), format!("https://{HOST}")),
        ("ANTHROPIC_AUTH_TOKEN".into(), FAKE_AUTH.into()),
    ])
    .expect("routing")
}

fn tools() -> Vec<String> {
    let mode = mode_named("bracken").expect("Bracken mode");
    mode.tools().into_iter().chain(core_tools()).map(|t| t.name.to_string()).collect()
}

/// Caps small enough for tests to reach.
fn caps() -> Caps {
    Caps { turn_time: Duration::from_secs(20), turn_budget_usd: None, ..Caps::pilot() }
}

/// For tests that a cap ends a turn or run early: the fake sleeps this long (10 minutes, in its
/// milliseconds) under a turn cap as long, so nothing else ends it, and the test allows
/// [`EARLY`], generous for a loaded machine and still far short of the sleep.
const SLEEP: &str = "sleep 600000";
const LONG_TURN: Duration = Duration::from_secs(600);
const EARLY: Duration = Duration::from_secs(60);

/// A local stand-in for the LLM proxy: records the first bytes of every connection.
struct Endpoint {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Endpoint {
    fn start() -> Endpoint {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&seen);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(EARLY));
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                buf.truncate(n);
                s.lock().expect("lock").push(buf);
                // Kept open, as a server mid-handshake would be.
                std::mem::forget(stream);
            }
        });
        Endpoint { addr, seen }
    }

    fn connections(&self) -> Vec<Vec<u8>> {
        self.seen.lock().expect("lock").clone()
    }
}

struct Setup {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    endpoint: Endpoint,
}

impl Setup {
    fn new() -> Setup {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = std::fs::canonicalize(tmp.path()).expect("resolved");
        Setup { _tmp: tmp, dir, endpoint: Endpoint::start() }
    }

    fn backend(&self, script: &str, caps: Caps) -> ClaudeBackend {
        self.backend_with(script, caps, Pin::of(&fake(), "2.1.311").expect("pin"))
    }

    fn backend_with(&self, script: &str, caps: Caps, pin: Pin) -> ClaudeBackend {
        ClaudeBackend::new(ClaudeConfig {
            pin,
            expect: Expect::pinned_2_1_311("claude-opus-5-5"),
            caps,
            routing: routing(),
            dir: self.dir.join("claude"),
            shim: bin(),
            prompt: script.to_string(),
            tools: tools(),
            upstream: Some(self.endpoint.addr),
        })
        .expect("the backend")
    }

    fn run(&self, backend: &ClaudeBackend, agents: usize, turns: u32) -> Result<PathBuf, String> {
        let cfg = run_config(self.dir.join("run"), "bracken", agents, turns, true);
        run(&cfg, backend).map(|o| o.events)
    }

    fn transcript(&self, agent: &str, turn: u32) -> Option<String> {
        std::fs::read_to_string(self.dir.join(format!("claude/transcripts/{agent}-{turn}.jsonl"))).ok()
    }

    fn usage(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("claude/usage.jsonl")).expect("usage.jsonl");
        text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
    }

    /// The probe report in a transcript.
    fn probe(&self, agent: &str, turn: u32) -> Value {
        let t = self.transcript(agent, turn).expect("a transcript");
        let line = t.lines().find(|l| l.contains("\"probe\"")).expect("a probe report");
        serde_json::from_str::<Value>(line).expect("JSON")["results"].clone()
    }
}

fn events(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).expect("events");
    text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

fn assert_verdicts(report: &Value, want: &[(&str, &str)]) {
    for (key, verdict) in want {
        assert_eq!(report[*key], *verdict, "{key} in {report:#}");
    }
}

// ---- The agent profile ----

#[test]
fn the_agent_profile_refuses_each_forbidden_access() {
    let s = Setup::new();
    let other_port = TcpListener::bind("127.0.0.1:0").expect("bind");
    let other = other_port.local_addr().expect("addr");
    let run = s.dir.join("run");
    let own = s.dir.join("claude/agents/agent-0");
    let repo = resolved(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("the repository");
    let h = home();
    let spec = json!({
        "read": [
            h.join(".zshrc"), h.clone(), h.join(".ssh"), h.join("Library/Keychains"), repo.join("Cargo.toml"),
            run.join("agents/agent-1"), run.join("agents/agent-0"), run.join("hub"), run.join("events.jsonl"),
            s.dir.join("claude/usage.jsonl"), own.join("home"), s.dir.join("claude/turn/agent-0/prompt.md"),
        ],
        "write": [
            h.join(".experiment-probe"), run.join("hub/probe"), run.join("agents/agent-0/probe"),
            s.dir.join("claude/turn/agent-0/probe"), own.join("home/probe"),
        ],
        "exec": ["/bin/sh", "/bin/bash", "/usr/bin/true", bracken()],
        "tcp": [other.to_string()],
        "proxy": [format!("{HOST}:443"), "example.com:443", format!("{HOST}:8443")],
        "mach": ["com.apple.SecurityServer", "com.apple.securityd.xpc"],
    });
    let backend = s.backend(&format!("probe {spec}\nresult 10"), caps());
    s.run(&backend, 2, 1).expect("the run");
    let r = s.probe("agent-0", 1);
    let p = |pre: &str, path: &Path| format!("{pre}:{}", path.display());
    let mut want = vec![
        (p("read", &h.join(".zshrc")), "denied"),
        (p("read", &h), "denied"),
        (p("read", &h.join(".ssh")), "denied"),
        (p("read", &h.join("Library/Keychains")), "denied"),
        (p("read", &repo.join("Cargo.toml")), "denied"),
        (p("read", &run.join("agents/agent-1")), "denied"),
        (p("read", &run.join("agents/agent-0")), "denied"),
        (p("read", &run.join("hub")), "denied"),
        (p("read", &run.join("events.jsonl")), "denied"),
        (p("read", &s.dir.join("claude/usage.jsonl")), "denied"),
        (p("write", &h.join(".experiment-probe")), "denied"),
        (p("write", &run.join("hub/probe")), "denied"),
        (p("write", &run.join("agents/agent-0/probe")), "denied"),
        (p("write", &s.dir.join("claude/turn/agent-0/probe")), "denied"),
        ("exec:/bin/sh".into(), "denied"),
        ("exec:/bin/bash".into(), "denied"),
        ("exec:/usr/bin/true".into(), "denied"),
        (p("exec", &bracken()), "denied"),
        (format!("tcp:{other}"), "denied"),
        ("proxy:example.com:443".into(), "denied"),
        (format!("proxy:{HOST}:8443"), "denied"),
        ("mach:com.apple.SecurityServer".into(), "denied"),
        ("mach:com.apple.securityd.xpc".into(), "denied"),
        // What it must still do: read its own and the turn's files, write its own, reach the LLM proxy.
        (p("read", &own.join("home")), "allowed"),
        (p("read", &s.dir.join("claude/turn/agent-0/prompt.md")), "allowed"),
        (p("write", &own.join("home/probe")), "allowed"),
        (format!("proxy:{HOST}:443"), "allowed"),
    ];
    want.sort();
    let want: Vec<(&str, &str)> = want.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_verdicts(&r, &want);
    // Only the allowed CONNECT reached the stand-in for the LLM proxy.
    let attempts = backend.proxy_attempts();
    assert!(attempts.iter().any(|a| a.allowed && a.target == format!("{HOST}:443")), "{attempts:?}");
    assert!(attempts.iter().filter(|a| a.allowed).all(|a| a.target == format!("{HOST}:443")), "{attempts:?}");
}

// ---- The tools and hub profiles ----

/// Runs the fake's probe under `sandbox`, standing in for the worker binary.
fn probe_under(sandbox: &Sandbox, spec: &Value) -> Value {
    let out = sandbox
        .command(&fake())
        .arg("probe")
        .arg(spec.to_string())
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("the probe");
    assert!(out.status.success(), "probe: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice::<Value>(&out.stdout).expect("JSON")["results"].clone()
}

struct Stores {
    _tmp: tempfile::TempDir,
    hub: PathBuf,
    copy: PathBuf,
    other: PathBuf,
    sockets: PathBuf,
}

fn stores() -> Stores {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("resolved");
    let (hub, copy, other, sockets) =
        (root.join("hub"), root.join("agents/agent-0"), root.join("agents/agent-1"), root.join("sockets"));
    for d in [&hub, &copy, &other, &sockets] {
        std::fs::create_dir_all(d).expect("mkdir");
        std::fs::write(d.join("file"), "x").expect("write");
    }
    Stores { _tmp: tmp, hub, copy, other, sockets }
}

#[test]
fn the_tools_profile_holds_one_copy_and_runs_no_shell() {
    let st = stores();
    let fake = fake();
    let sockets = [st.sockets.join("c0.sock")];
    let sandbox = Sandbox::tools(&WorkerPaths {
        store: &st.copy,
        worker: &fake,
        bracken: &bracken(),
        git: None,
        sockets: &sockets,
    })
    .expect("profile");
    let h = home();
    let spec = json!({
        "read": [st.copy.join("file"), st.other.join("file"), st.hub.join("file"), h.join(".zshrc"), h.join(".ssh")],
        "write": [st.copy.join("new"), st.hub.join("new"), st.other.join("new"), h.join(".experiment-probe")],
        "exec": ["/bin/sh", "/bin/bash", "/usr/bin/env", bracken()],
        "unix": [st.sockets.join("h0.sock")],
        "mach": ["com.apple.SecurityServer"],
    });
    let r = probe_under(&sandbox, &spec);
    let p = |pre: &str, path: &Path| format!("{pre}:{}", path.display());
    let want = [
        (p("read", &st.copy.join("file")), "allowed"),
        (p("write", &st.copy.join("new")), "allowed"),
        (p("exec", &bracken()), "allowed"),
        (p("read", &st.other.join("file")), "denied"),
        (p("read", &st.hub.join("file")), "denied"),
        (p("read", &h.join(".zshrc")), "denied"),
        (p("read", &h.join(".ssh")), "denied"),
        (p("write", &st.hub.join("new")), "denied"),
        (p("write", &st.other.join("new")), "denied"),
        (p("write", &h.join(".experiment-probe")), "denied"),
        ("exec:/bin/sh".into(), "denied"),
        ("exec:/bin/bash".into(), "denied"),
        ("exec:/usr/bin/env".into(), "denied"),
        (p("unix", &st.sockets.join("h0.sock")), "denied"),
        ("mach:com.apple.SecurityServer".into(), "denied"),
    ];
    let want: Vec<(&str, &str)> = want.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_verdicts(&r, &want);
}

#[test]
fn the_hub_profile_writes_no_copy_and_runs_no_shell() {
    let st = stores();
    let fake = fake();
    let sockets = [st.sockets.join("h0.sock"), st.sockets.join("h1.sock")];
    let sandbox =
        Sandbox::hub(&WorkerPaths { store: &st.hub, worker: &fake, bracken: &bracken(), git: None, sockets: &sockets })
            .expect("profile");
    let spec = json!({
        "read": [st.hub.join("file"), st.copy.join("file"), st.other.join("file")],
        "write": [st.hub.join("new"), st.copy.join("new"), st.other.join("new")],
        "exec": ["/bin/sh", "/bin/bash"],
        "unix": [st.sockets.join("c0.sock")],
    });
    let r = probe_under(&sandbox, &spec);
    let p = |pre: &str, path: &Path| format!("{pre}:{}", path.display());
    let want = [
        (p("read", &st.hub.join("file")), "allowed"),
        (p("write", &st.hub.join("new")), "allowed"),
        (p("read", &st.copy.join("file")), "denied"),
        (p("read", &st.other.join("file")), "denied"),
        (p("write", &st.copy.join("new")), "denied"),
        (p("write", &st.other.join("new")), "denied"),
        ("exec:/bin/sh".into(), "denied"),
        ("exec:/bin/bash".into(), "denied"),
        (p("unix", &st.sockets.join("c0.sock")), "denied"),
    ];
    let want: Vec<(&str, &str)> = want.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_verdicts(&r, &want);
}

/// A mock run of `mode` with every worker sandboxed: each agent's work goes up to the hub and the other's comes back.
fn syncs_both_ways_sandboxed(mode: &'static str) {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let cfg = run_config(tmp.path().join("run"), mode, 2, 2, true);
    let backend = bracken_experiment::mock::MockBackend { seed: 5, wrong_rate: 0.0, shim: Some(bin()) };
    let outcome = run(&cfg, &backend).unwrap_or_else(|e| panic!("{mode}: the run: {e}"));
    let log = events(&outcome.events);
    assert!(!log.iter().any(|e| e["event"] == "turn_error"), "{mode}: no turn failed: {log:?}");
    let syncs: Vec<&Value> = log.iter().filter(|e| e["event"] == "sync").collect();
    assert!(syncs.len() >= 8, "{mode}: {} syncs", syncs.len());
    // Each agent's work went up to the hub, and the other's came back down: the second turn's syncs carry more
    // than the first's.
    let bytes = |agent: &str, way: &str| -> Vec<u64> {
        syncs.iter().filter(|e| e["agent"] == agent).filter_map(|e| e["data"][way]["bytes"].as_u64()).collect()
    };
    for agent in ["agent-0", "agent-1"] {
        for way in ["up", "down"] {
            let b = bytes(agent, way);
            assert!(b.iter().max() > b.first(), "{mode}: {agent} {way}: {b:?}");
        }
    }
}

#[test]
fn bracken_mode_syncs_both_ways_with_every_worker_sandboxed() {
    syncs_both_ways_sandboxed("bracken");
}

#[test]
fn git_mode_syncs_both_ways_with_every_worker_sandboxed() {
    syncs_both_ways_sandboxed("git");
}

#[test]
fn crdt_mode_syncs_both_ways_with_every_worker_sandboxed() {
    syncs_both_ways_sandboxed("crdt");
}

#[test]
fn git_modes_worker_profiles_run_git_and_still_no_shell() {
    let st = stores();
    let fake = fake();
    let binary = git_binary().expect("git");
    let git = Git { exec_path: binary.parent().expect("an exec path").to_path_buf(), binary: binary.clone() };
    let helper = git.exec_path.join("git-upload-pack");
    for hub in [false, true] {
        let store = if hub { &st.hub } else { &st.copy };
        let paths = WorkerPaths { store, worker: &fake, bracken: &bracken(), git: Some(&git), sockets: &[] };
        let sandbox = if hub { Sandbox::hub(&paths) } else { Sandbox::tools(&paths) }.expect("profile");
        let spec = json!({"exec": [binary, helper, "/bin/sh", "/bin/bash"], "write": [st.other.join("new")]});
        let r = probe_under(&sandbox, &spec);
        let want = [
            (format!("exec:{}", binary.display()), "allowed"),
            (format!("exec:{}", helper.display()), "allowed"),
            ("exec:/bin/sh".to_string(), "denied"),
            ("exec:/bin/bash".to_string(), "denied"),
            (format!("write:{}", st.other.join("new").display()), "denied"),
        ];
        let want: Vec<(&str, &str)> = want.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_verdicts(&r, &want);
    }
}

// ---- The gate ----

fn refused_by_gate(script: &str) -> String {
    let s = Setup::new();
    let backend = s.backend(script, caps());
    let err = s.run(&backend, 1, 2).expect_err("the gate stops the run");
    assert!(err.contains("the gate refused"), "{err}");
    assert!(backend.halted().is_some());
    assert!(s.transcript("agent-0", 2).is_none(), "no second turn started");
    err
}

#[test]
fn the_gate_passes_the_planned_init() {
    let s = Setup::new();
    let backend = s.backend("result 10", caps());
    s.run(&backend, 1, 1).expect("the run");
    assert!(s.usage().iter().any(|r| r["gate"] == true));
}

#[test]
fn the_gate_refuses_each_deviation() {
    let cases = [
        ("claude_code_version", "\"2.1.312\""),
        ("model", "\"claude-haiku-5-5\""),
        ("permissionMode", "\"bypassPermissions\""),
        ("apiKeySource", "\"ANTHROPIC_API_KEY\""),
        ("tools", "[\"Bash\"]"),
        ("skills", "[\"deep-research\"]"),
        ("plugins", "[{\"name\": \"owner-plugin\"}]"),
        (
            "mcp_servers",
            "[{\"name\": \"experiment\", \"status\": \"connected\"}, {\"name\": \"owner\", \"status\": \"connected\"}]",
        ),
        ("memory_paths", "{\"auto\": \"/Users/someone/.claude/projects/x/memory/\"}"),
        ("subtype", "\"status\""),
    ];
    for (key, value) in cases {
        let err = refused_by_gate(&format!("init {key} {value}\nresult 10"));
        assert!(!err.is_empty(), "{key}");
    }
    // An added tool, beside the mode's, is refused too.
    let mut with_extra: Vec<String> = tools().iter().map(|t| format!("\"mcp__experiment__{t}\"")).collect();
    with_extra.push("\"WebFetch\"".into());
    refused_by_gate(&format!("init tools [{}]\nresult 10", with_extra.join(",")));
}

#[test]
fn a_changed_turn_configuration_or_binary_stops_the_run() {
    // The turn configuration: changed between turns.
    let s = Setup::new();
    let backend = s.backend("result 10", caps());
    s.run(&backend, 1, 1).expect("the first run");
    let prompt = s.dir.join("claude/turn/agent-0/prompt.md");
    let mut perms = std::fs::metadata(&prompt).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o600);
    std::fs::set_permissions(&prompt, perms).expect("chmod");
    std::fs::write(&prompt, "something else").expect("write");
    let socket = s.dir.join("run/sockets/a0.sock");
    let err = backend.turn(0, "agent-0", 2, &socket).expect_err("refused");
    assert!(err.contains("prompt.md changed"), "{err}");
    assert!(backend.halted().is_some());

    // The binary: changed after it was pinned.
    let s = Setup::new();
    let copy = s.dir.join("claude-copy");
    std::fs::copy(fake(), &copy).expect("copy");
    let backend = s.backend_with("result 10", caps(), Pin::of(&copy, "2.1.311").expect("pin"));
    std::fs::OpenOptions::new().append(true).open(&copy).expect("open").write_all(b"\0").expect("append");
    let err = s.run(&backend, 1, 1).expect_err("refused");
    assert!(err.contains("binary changed"), "{err}");
}

// ---- Token accounting and caps ----

fn turn_error_for(s: &Setup, events_path: &Path) -> usize {
    let _ = s;
    events(events_path).iter().filter(|e| e["event"] == "turn_error").count()
}

#[test]
fn output_comes_from_the_result_and_repeated_blocks_count_once() {
    let s = Setup::new();
    // Three content blocks of one message repeat its usage; the stream says output 1, the result 5000.
    let backend = s.backend("say 3\nsay\nresult 5000", caps());
    s.run(&backend, 1, 1).expect("the run");
    let (mine, run) = backend.totals("agent-0");
    assert_eq!(mine, Tokens { input: 2000, cache_read: 0, cache_write: 0, output: 5000 });
    assert_eq!(run, mine);
    let rows = s.usage();
    assert_eq!(rows.iter().filter(|r| r["message"].is_string()).count(), 4, "every assistant event is recorded");
    assert!(rows.iter().any(|r| r["output"] == 5000 && r["ended"] == "success"));
}

#[test]
fn tool_calls_past_the_cap_end_the_turn() {
    let s = Setup::new();
    let script: String = (0..30).map(|_| "call notes_read {}\nsleep 100\n").collect::<String>() + "result 10";
    let backend = s.backend(&script, Caps { calls_per_turn: 3, ..caps() });
    let events_path = s.run(&backend, 1, 1).expect("the run goes on");
    assert_eq!(turn_error_for(&s, &events_path), 1);
    let calls = events(&events_path).iter().filter(|e| e["event"] == "tool_call").count();
    // The driver's own refusal of extra calls is serve()'s (PR #26); here the process ends just past the cap.
    assert!(calls <= 5, "{calls} calls reached the driver");
    let (mine, _) = backend.totals("agent-0");
    assert_eq!(mine.output, caps().turn_output, "a turn without a result counts at its output cap");

    // At the cap exactly, the turn runs to its result.
    let s = Setup::new();
    let backend = s.backend(
        "call notes_read {}\ncall notes_read {}\ncall notes_read {}\nresult 10",
        Caps { calls_per_turn: 3, ..caps() },
    );
    let events_path = s.run(&backend, 1, 1).expect("the run");
    assert_eq!(turn_error_for(&s, &events_path), 0, "the cap-th call is allowed");
    assert_eq!(backend.totals("agent-0").0.output, 10);
}

/// A result event with input `input` (the stream counted 1000 per message) and output `output`.
fn result_with(input: u64, output: u64) -> String {
    format!(
        r#"result-json {{"type":"result","subtype":"success","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"modelUsage":{{"claude-opus-5-5":{{"inputTokens":{input},"outputTokens":{output},"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}}}}}"#
    )
}

#[test]
fn the_input_gap_stops_the_run_just_over_10_percent_and_its_result_is_counted() {
    // 9% over the stream's 1000: counted, the run goes on.
    let s = Setup::new();
    let backend = s.backend(&format!("say\n{}", result_with(1090, 20)), caps());
    s.run(&backend, 1, 1).expect("a 9% gap is allowed");
    assert_eq!(backend.totals("agent-0").0, Tokens { input: 1090, output: 20, ..Tokens::default() });

    // Over 10% of the larger figure (120 of 1120): the run stops, and the result's output is still counted, not the stream's stub.
    let s = Setup::new();
    let backend = s.backend(&format!("say\n{}", result_with(1120, 20000)), caps());
    let err = s.run(&backend, 1, 1).expect_err("an 11% gap stops the run");
    assert!(err.contains("10%"), "{err}");
    assert_eq!(backend.totals("agent-0").0, Tokens { input: 1120, output: 20000, ..Tokens::default() });
}

#[test]
fn a_halted_turn_is_charged_at_its_output_cap_with_a_closing_row() {
    let s = Setup::new();
    let backend = s.backend("say\nsay\nsay\nsleep 10000\nresult 10", Caps { run_tokens: 2500, ..caps() });
    assert!(s.run(&backend, 1, 1).is_err());
    assert_eq!(backend.totals("agent-0").0.output, caps().turn_output, "charged at its cap, not the stream's stub");
    let rows = s.usage();
    let closing = rows.iter().find(|r| r["output_counted_at"].is_u64()).expect("a closing row");
    assert_eq!(closing["output_counted_at"], caps().turn_output);
    assert!(closing["ended"].as_str().is_some_and(|e| e.contains("Halt")), "{closing}");
}

#[test]
fn a_result_less_turn_whose_stream_counted_more_records_what_it_counted() {
    let s = Setup::new();
    let usage = r#"{"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 31000}"#;
    let backend = s.backend(&format!("usage {usage}\nsay\nexit 3"), caps());
    s.run(&backend, 1, 1).expect("the run goes on");
    assert_eq!(backend.totals("agent-0").0.output, 31000);
    assert!(s.usage().iter().any(|r| r["output_counted_at"] == 31000));
}

#[test]
fn a_tool_call_without_an_id_or_an_overflowing_figure_stops_the_run() {
    let idless = r#"line {"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1},"content":[{"type":"tool_use","name":"mcp__experiment__log","input":{}}]}}"#;
    let s = Setup::new();
    let backend = s.backend(&format!("{idless}\nresult 10"), caps());
    let err = s.run(&backend, 1, 1).expect_err("stopped");
    assert!(err.contains("without an id"), "{err}");

    // A figure near u64::MAX saturates the totals at the top, so the run's cap trips instead of wrapping.
    let s = Setup::new();
    let usage = r#"{"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 18446744073709550616, "output_tokens": 1}"#;
    let backend = s.backend(&format!("usage {usage}\nsay\nresult 10"), caps());
    let err = s.run(&backend, 1, 1).expect_err("stopped");
    assert!(err.contains("token cap"), "{err}");

    // With the caps at the top, two such figures would wrap to a small total; they saturate instead.
    let s = Setup::new();
    let top = Caps { run_tokens: u64::MAX, agent_tokens: u64::MAX, context: u64::MAX, ..caps() };
    let half = r#"{"input_tokens": 1000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 9223372036854775808, "output_tokens": 1}"#;
    let backend = s.backend(&format!("usage {half}\nsay\nsay\nresult 10"), top);
    assert!(s.run(&backend, 1, 1).is_err(), "the saturated total meets the cap");
    assert_eq!(backend.totals("agent-0").1.total(), u64::MAX);
}

#[test]
fn context_past_the_cap_ends_the_turn() {
    let s = Setup::new();
    let usage = r#"{"input_tokens": 2000, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 160000, "output_tokens": 1}"#;
    let backend = s.backend(&format!("usage {usage}\nsay\nsleep 3000\nresult 10"), caps());
    let events_path = s.run(&backend, 1, 1).expect("the run goes on");
    assert_eq!(turn_error_for(&s, &events_path), 1);
    assert!(s.usage().iter().any(|r| r["ended"].as_str().is_some_and(|e| e.contains("context"))));
}

#[test]
fn the_turn_time_cap_ends_a_quiet_turn() {
    let s = Setup::new();
    let backend = s.backend(&format!("say\n{SLEEP}\nresult 10"), Caps { turn_time: Duration::from_secs(2), ..caps() });
    let start = Instant::now();
    let events_path = s.run(&backend, 1, 1).expect("the run goes on");
    assert!(start.elapsed() < EARLY, "ended at the cap, not the sleep");
    assert_eq!(turn_error_for(&s, &events_path), 1);
    assert_eq!(backend.totals("agent-0").0.output, caps().turn_output);
}

#[test]
fn an_agents_token_cap_ends_its_turns_and_the_run_cap_stops_the_run() {
    let s = Setup::new();
    let backend = s.backend("say\nresult 600", Caps { agent_output: 1000, ..caps() });
    let events_path = s.run(&backend, 1, 3).expect("the run goes on without the agent");
    // Turns 1 and 2 ran to their results (600, then 1200 >= 1000); turn 3 never started.
    assert_eq!(backend.totals("agent-0").0.output, 1200);
    assert!(s.transcript("agent-0", 3).is_none());
    assert_eq!(turn_error_for(&s, &events_path), 1);

    let s = Setup::new();
    let backend = s.backend("say\nresult 600", Caps { run_output: 1000, ..caps() });
    let err = s.run(&backend, 2, 3).expect_err("the run stops");
    assert!(err.contains("token cap"), "{err}");
    assert!(backend.totals("x").1.output <= 2400, "no agent went on long after the cap");
}

#[test]
fn token_caps_act_while_a_turn_streams() {
    // The agent's cap: its turn ends at the third message, before the result.
    let s = Setup::new();
    let script = format!("say\nsay\nsay\n{SLEEP}\nresult 10");
    let backend = s.backend(&script, Caps { agent_tokens: 2500, turn_time: LONG_TURN, ..caps() });
    let start = Instant::now();
    s.run(&backend, 1, 1).expect("the run goes on");
    assert!(start.elapsed() < EARLY, "ended at the cap");
    assert!(s.usage().iter().any(|r| r["ended"].as_str().is_some_and(|e| e.contains("agent tokens"))));

    // The run's cap: the run stops mid-turn.
    let s = Setup::new();
    let backend = s.backend(&script, Caps { run_tokens: 2500, turn_time: LONG_TURN, ..caps() });
    let start = Instant::now();
    let err = s.run(&backend, 1, 1).expect_err("the run stops");
    assert!(err.contains("token cap") && start.elapsed() < EARLY, "{err}");
}

#[test]
fn the_run_time_cap_stops_the_run() {
    let s = Setup::new();
    let caps = Caps { run_time: Duration::from_secs(2), turn_time: LONG_TURN, ..caps() };
    let backend = s.backend(&format!("say\n{SLEEP}\nresult 10"), caps);
    let start = Instant::now();
    let err = s.run(&backend, 2, 2).expect_err("the run stops");
    assert!(err.contains("time cap") && start.elapsed() < EARLY, "{err}");
}

/// A backend that halts the run in its first turn.
struct Halting(Mutex<u32>);

impl Backend for Halting {
    fn turn(&self, _: usize, _: &str, _: u32, _: &Path) -> Result<(), String> {
        *self.0.lock().expect("lock") += 1;
        Ok(())
    }

    fn halted(&self) -> Option<String> {
        (*self.0.lock().expect("lock") > 0).then(|| "a test halt".to_string())
    }
}

#[test]
fn a_halted_backend_starts_no_further_turn_and_the_run_fails() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let cfg = run_config(tmp.path().join("run"), "bracken", 1, 3, false);
    let backend = Halting(Mutex::new(0));
    let err = run(&cfg, &backend).err().expect("the run fails");
    assert!(err.contains("a test halt"), "{err}");
    assert_eq!(*backend.0.lock().expect("lock"), 1);
}

#[test]
fn missing_or_unreadable_usage_stops_the_run() {
    let cases = [
        ("an assistant message without usage", r#"line {"type":"assistant","message":{"id":"m1","content":[]}}"#.to_string() + "\nresult 10"),
        ("usage that isn't a number", r#"usage {"input_tokens": "lots", "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 1}"#.to_string() + "\nsay\nresult 10"),
        ("a renamed field", r#"usage {"inputTokens": 5, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 1}"#.to_string() + "\nsay\nresult 10"),
        ("a result without usage", r#"say
result-json {"type":"result","subtype":"success","modelUsage":{"claude-opus-5-5":{"inputTokens":1000,"outputTokens":5,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}}"#.to_string()),
        ("a result without modelUsage", r#"say
result-json {"type":"result","subtype":"success","usage":{"input_tokens":1000,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#.to_string()),
        ("a result whose input is 50% over the stream's", r#"say
result-json {"type":"result","subtype":"success","usage":{"input_tokens":1500,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"modelUsage":{"claude-opus-5-5":{"inputTokens":1500,"outputTokens":5,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}}"#.to_string()),
        ("a line that isn't JSON", "say\nline not json\nresult 10".to_string()),
    ];
    for (what, script) in cases {
        let s = Setup::new();
        let backend = s.backend(&script, caps());
        assert!(s.run(&backend, 1, 2).is_err(), "{what} stops the run");
        assert!(s.transcript("agent-0", 2).is_none(), "{what}: no further turn");
    }
}

#[test]
fn a_turn_that_ends_without_a_result_counts_at_its_output_cap() {
    let s = Setup::new();
    let backend = s.backend("say\nexit 3", caps());
    let events_path = s.run(&backend, 1, 1).expect("the run goes on");
    assert_eq!(turn_error_for(&s, &events_path), 1);
    assert_eq!(backend.totals("agent-0").0, Tokens { input: 1000, output: caps().turn_output, ..Tokens::default() });
}

// ---- Transcripts ----

#[test]
fn transcripts_are_kept_without_the_auth_value_and_a_leak_stops_the_run() {
    // Claude Code holds the auth value: a turn that prints it stops the run, and the line isn't kept.
    let s = Setup::new();
    let backend =
        s.backend(&format!("env ANTHROPIC_AUTH_TOKEN\n{SLEEP}\nresult 10"), Caps { turn_time: LONG_TURN, ..caps() });
    let start = Instant::now();
    let err = s.run(&backend, 1, 2).expect_err("the run stops");
    assert!(err.contains("auth value"), "{err}");
    assert!(start.elapsed() < EARLY, "stopped at the line, not at the turn's end");
    let t = s.transcript("agent-0", 1).expect("kept, without the line");
    assert!(t.contains("\"init\"") && !t.contains(FAKE_AUTH));
    assert!(s.transcript("agent-0", 2).is_none());

    // On standard error: the turn's transcript is removed.
    let s = Setup::new();
    let backend = s.backend("say\nstderr-env ANTHROPIC_AUTH_TOKEN\nresult 10", caps());
    assert!(s.run(&backend, 1, 1).is_err());
    assert!(s.transcript("agent-0", 1).is_none(), "removed");

    // A clean turn keeps its transcript, which holds the stream and nothing of the value.
    let s = Setup::new();
    let backend = s.backend("env HTTPS_PROXY\nsay\nresult 10", caps());
    s.run(&backend, 1, 1).expect("the run");
    let t = s.transcript("agent-0", 1).expect("kept");
    assert!(t.contains("\"init\"") && t.contains("\"result\"") && !t.contains(FAKE_AUTH));
    assert!(t.contains(&format!("http://127.0.0.1:{}", backend.proxy_port())), "Claude Code is sent to the proxy");
}

// ---- Process groups ----

fn alive(pid: u32) -> bool {
    Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn processes_mentioning(text: &str) -> String {
    let out = Command::new("/usr/bin/pgrep").args(["-fl", text]).output().expect("pgrep");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn killing_the_driver_mid_turn_leaves_no_agent_process_and_no_request() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = std::fs::canonicalize(tmp.path()).expect("resolved").join("exp");
    let log = dir.join("claude/agents/agent-0/home/loop.log");
    let prompt = tmp.path().join("prompt.md");
    std::fs::write(&prompt, format!("env HTTPS_PROXY\nconnect-loop {HOST}:443 {}\n", log.display())).expect("prompt");
    let settings = tmp.path().join("settings.json");
    let env = json!({"env": {"ANTHROPIC_BASE_URL": format!("https://{HOST}"), "ANTHROPIC_AUTH_TOKEN": FAKE_AUTH}});
    std::fs::write(&settings, env.to_string()).expect("settings");
    let mut driver = Command::new(bin())
        .arg("run")
        .arg("--dir")
        .arg(&dir)
        .args(["--agents", "1", "--turns", "1", "--claude-version", "2.1.311"])
        .arg("--claude")
        .arg(fake())
        .arg("--settings")
        .arg(&settings)
        .arg("--prompt-file")
        .arg(&prompt)
        .arg("--bracken")
        .arg(bracken())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the driver");
    let lines = |p: &Path| std::fs::read_to_string(p).map(|t| t.lines().count()).unwrap_or(0);
    let start = Instant::now();
    while lines(&log) < 3 {
        assert!(start.elapsed() < Duration::from_secs(60), "the fake never reached the proxy");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: u32 = std::fs::read_to_string(log.with_extension("pid")).expect("pid").trim().parse().expect("a pid");
    let transcript = std::fs::read_to_string(dir.join("claude/transcripts/agent-0-1.jsonl")).expect("transcript");
    let port: u16 = transcript
        .split("http://127.0.0.1:")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .and_then(|p| p.parse().ok())
        .expect("the proxy's port");

    driver.kill().expect("killing the driver");
    driver.wait().expect("reaped");
    let start = Instant::now();
    while alive(pid) || !processes_mentioning(&dir.display().to_string()).is_empty() {
        assert!(start.elapsed() < EARLY, "left running: {}", processes_mentioning(&dir.display().to_string()));
        std::thread::sleep(Duration::from_millis(50));
    }
    let after = lines(&log);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lines(&log), after, "no further request after the driver died");
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "the proxy is gone with the driver");
}

// ---- The forwarding proxy and routing ----

fn connect_through(port: u16, head: &str) -> (String, std::net::TcpStream) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(EARLY)).expect("timeout");
    s.write_all(head.as_bytes()).expect("write");
    let mut buf = [0u8; 128];
    let n = s.read(&mut buf).unwrap_or(0);
    (String::from_utf8_lossy(&buf[..n]).into_owned(), s)
}

#[test]
fn the_proxy_tunnels_only_to_the_routing_host() {
    use bracken_experiment::proxy::{Proxy, Route};
    let endpoint = Endpoint::start();
    let route = Route { host: HOST.into(), port: 443, connect: Some(endpoint.addr) };
    let proxy = Proxy::start(route, Duration::from_secs(120)).expect("proxy");
    let port = proxy.port();

    let (reply, mut tunnel) = connect_through(port, "CONNECT LLM.fake.invalid:443 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    tunnel.write_all(b"client hello").expect("write");
    let start = Instant::now();
    while endpoint.connections().is_empty() {
        assert!(start.elapsed() < EARLY);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(endpoint.connections(), vec![b"client hello".to_vec()]);

    for head in [
        "CONNECT example.com:443 HTTP/1.1\r\n\r\n",
        "CONNECT llm.fake.invalid:80 HTTP/1.1\r\n\r\n",
        "CONNECT llm.fake.invalid.example.com:443 HTTP/1.1\r\n\r\n",
        "GET http://llm.fake.invalid/ HTTP/1.1\r\n\r\n",
        "CONNECT llm.fake.invalid:443 SPDY\r\n\r\n",
    ] {
        let (reply, _) = connect_through(port, head);
        assert!(reply.starts_with("HTTP/1.1 403"), "{head:?}: {reply}");
    }
    assert_eq!(endpoint.connections().len(), 1, "nothing refused reached the endpoint");
    let attempts = proxy.attempts();
    assert_eq!(attempts.iter().filter(|a| a.allowed).count(), 0, "the tunnel is still open");
    assert_eq!(attempts.len(), 5);

    // Dropping the proxy closes the open tunnel.
    drop(proxy);
    let mut buf = [0u8; 8];
    assert_eq!(tunnel.read(&mut buf).expect("closed, not timed out"), 0, "closed");
}

#[test]
fn routing_takes_only_the_routing_entries_and_never_shows_the_value() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let write = |v: Value| {
        let p = tmp.path().join("settings.json");
        std::fs::write(&p, v.to_string()).expect("write");
        p
    };
    let ok = json!({"env": {"ANTHROPIC_BASE_URL": "https://llm.fake.invalid:8443/v1", "ANTHROPIC_AUTH_TOKEN": FAKE_AUTH,
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY": "1"}, "hooks": {"Stop": []}});
    let r = Routing::from_settings(&write(ok)).expect("routing");
    assert_eq!((r.host.as_str(), r.port), (HOST, 8443));
    assert!(!format!("{r:?}").contains(FAKE_AUTH));
    for bad in [
        json!({"env": {"ANTHROPIC_BASE_URL": "https://h.invalid", "ANTHROPIC_AUTH_TOKEN": FAKE_AUTH, "NODE_OPTIONS": "x"}}),
        json!({"env": {"ANTHROPIC_BASE_URL": "http://h.invalid", "ANTHROPIC_AUTH_TOKEN": FAKE_AUTH}}),
        json!({"env": {"ANTHROPIC_BASE_URL": "https://h.invalid"}}),
        json!({"env": {"ANTHROPIC_AUTH_TOKEN": FAKE_AUTH}}),
        json!({"env": {"ANTHROPIC_BASE_URL": "https://h invalid", "ANTHROPIC_AUTH_TOKEN": FAKE_AUTH}}),
        json!({}),
    ] {
        let err = Routing::from_settings(&write(bad.clone())).expect_err("refused");
        assert!(!err.contains(FAKE_AUTH), "{err}");
    }
}

// ---- The real binary ----

/// The pinned real Claude Code binary under the final agent profile, with a throwaway home and
/// config directory and a fake auth value, through the forwarding proxy to a local stand-in for
/// the LLM proxy: run to its init event and its first request. No model is called: the stand-in
/// only reads the TLS ClientHello. Run with `EXPERIMENT_CLAUDE=<versioned binary> cargo test -p
/// bracken-experiment --test agent real_claude -- --ignored`.
#[test]
#[ignore = "runs the real Claude Code binary; set EXPERIMENT_CLAUDE"]
fn real_claude_code_reaches_init_and_first_request() {
    let claude = PathBuf::from(std::env::var("EXPERIMENT_CLAUDE").expect("EXPERIMENT_CLAUDE"));
    let version = claude.parent().and_then(|d| d.file_name()).and_then(|v| v.to_str()).expect("a versioned path");
    let s = Setup::new();
    let backend = s.backend_with(
        "You are a test. Reply with one word.",
        Caps { turn_time: Duration::from_secs(25), turn_budget_usd: Some(0.5), ..caps() },
        Pin::of(&claude, version).expect("pin"),
    );
    let _ = s.run(&backend, 1, 1).expect("the run");
    let t = s.transcript("agent-0", 1).expect("a transcript");
    let init: Value = serde_json::from_str(t.lines().next().expect("a line")).expect("JSON");
    println!(
        "init: tools {} mcp_servers {} apiKeySource {} version {}",
        init["tools"], init["mcp_servers"], init["apiKeySource"], init["claude_code_version"]
    );
    assert!(s.usage().iter().any(|r| r["gate"] == true), "the gate passed");
    assert!(!t.contains(FAKE_AUTH), "no auth value in the transcript");
    let attempts = backend.proxy_attempts();
    println!("proxy attempts: {attempts:?}");
    assert!(!attempts.is_empty() && attempts.iter().all(|a| a.allowed && a.target == format!("{HOST}:443")));
    let connections = s.endpoint.connections();
    println!("connections at the stand-in: {}", connections.len());
    let hello = connections.first().expect("the first request reached the stand-in");
    assert_eq!(hello.first(), Some(&0x16), "a TLS handshake");
    assert!(hello.windows(HOST.len()).any(|w| w == HOST.as_bytes()), "its SNI names the routing host");
    assert!(!hello.windows(FAKE_AUTH.len()).any(|w| w == FAKE_AUTH.as_bytes()));
    let rows = s.usage();
    println!("usage rows: {rows:?}");
}
