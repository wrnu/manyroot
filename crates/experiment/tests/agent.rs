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

use bracken_experiment::claude::{Caps, ClaudeBackend, ClaudeConfig, Expect, Pin, RUN_TIME_CAP, Routing, Tokens};
use bracken_experiment::driver::{
    Backend, CALLS_PER_TURN, MAX_RESULT_CHARS, RunConfig, TURN_LIMIT, core_tools, git_binary, run,
};
use bracken_experiment::mode::mode_named;
use bracken_experiment::plan::{PlanInputs, report};
use bracken_experiment::proxy::Attempt;
use bracken_experiment::sandbox::{Git, Sandbox, WorkerPaths, resolved};
use bracken_experiment::task::Task;

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
        plan: PlanInputs::default(),
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
    tools_of("bracken")
}

fn tools_of(mode: &str) -> Vec<String> {
    let mode = mode_named(mode).expect("a mode");
    mode.tools(Task::Phase1).into_iter().chain(core_tools()).map(|t| t.name.to_string()).collect()
}

/// Caps small enough for tests to reach.
fn caps() -> Caps {
    Caps { turn_time: Duration::from_secs(20), turn_budget_usd: None, ..Caps::pilot() }
}

/// For tests that a cap ends a turn or run early: the fake sleeps this long (10 minutes, in its
/// milliseconds) under a turn cap as long, so nothing else ends it, and the test allows
/// [`EARLY`], generous for a loaded machine (several cargo builds at once) and still half the sleep.
const SLEEP: &str = "sleep 600000";
const LONG_TURN: Duration = Duration::from_secs(600);
const EARLY: Duration = Duration::from_secs(300);

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
        self.backend_in("bracken", script, caps, pin)
    }

    fn backend_in(&self, mode: &str, script: &str, caps: Caps, pin: Pin) -> ClaudeBackend {
        ClaudeBackend::new(ClaudeConfig {
            pin,
            expect: Expect::pinned_2_1_311("claude-opus-5-5"),
            caps,
            routing: routing(),
            dir: self.dir.join("claude"),
            shim: bin(),
            prompt: script.to_string(),
            turns: 8,
            tools: tools_of(mode),
            upstream: Some(self.endpoint.addr),
        })
        .expect("the backend")
    }

    fn run(&self, backend: &ClaudeBackend, agents: usize, turns: u32) -> Result<PathBuf, String> {
        let cfg = run_config(self.dir.join("run"), "bracken", agents, turns, true);
        run(&cfg, backend).map(|o| o.events)
    }

    /// A run of one agent whose driver refuses tool calls past `calls` a turn, as the backend's caps should match.
    fn run_calls(&self, backend: &ClaudeBackend, calls: u32) -> Result<PathBuf, String> {
        self.run_calls_for(backend, calls, 1)
    }

    fn run_calls_for(&self, backend: &ClaudeBackend, calls: u32, turns: u32) -> Result<PathBuf, String> {
        let cfg = RunConfig { calls_per_turn: calls, ..run_config(self.dir.join("run"), "bracken", 1, turns, true) };
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
    // Only the allowed CONNECT reached the stand-in for the LLM proxy. The proxy records a tunnel
    // once it has closed, which can be just after the probe has exited.
    let start = Instant::now();
    let tunnelled = |a: &Attempt| a.allowed && a.target == format!("{HOST}:443");
    while !backend.proxy_attempts().iter().any(tunnelled) {
        assert!(start.elapsed() < EARLY, "{:?}", backend.proxy_attempts());
        std::thread::sleep(Duration::from_millis(10));
    }
    let attempts = backend.proxy_attempts();
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
    let backend = bracken_experiment::mock::MockBackend {
        task: Task::Phase1,
        seed: 5,
        wrong_rate: 0.0,
        shim: Some(bin()),
        usage: None,
        ..bracken_experiment::mock::MockBackend::default()
    };
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
    let git = Git::of(&binary).expect("git's exec path");
    // The exec path, not the binary's own folder: a tool beside the binary that isn't a git helper stays out.
    // Where the folder holds nothing else (Homebrew, most Linux), there is nothing to refuse.
    let beside = std::fs::read_dir(binary.parent().expect("a folder"))
        .expect("the binary's folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("git");
            !name.starts_with("git") && p.is_file() && !p.starts_with(&git.exec_path)
        });
    let helper = git.exec_path.join("git-upload-pack");
    for hub in [false, true] {
        let store = if hub { &st.hub } else { &st.copy };
        let paths = WorkerPaths { store, worker: &fake, bracken: &bracken(), git: Some(&git), sockets: &[] };
        let sandbox = if hub { Sandbox::hub(&paths) } else { Sandbox::tools(&paths) }.expect("profile");
        let mut exec = vec![binary.clone(), helper.clone(), "/bin/sh".into(), "/bin/bash".into()];
        exec.extend(beside.clone());
        let spec = json!({"exec": exec, "write": [st.other.join("new")]});
        let r = probe_under(&sandbox, &spec);
        let mut want = vec![
            (format!("exec:{}", binary.display()), "allowed"),
            (format!("exec:{}", helper.display()), "allowed"),
            ("exec:/bin/sh".to_string(), "denied"),
            ("exec:/bin/bash".to_string(), "denied"),
            (format!("write:{}", st.other.join("new").display()), "denied"),
        ];
        want.extend(beside.iter().map(|b| (format!("exec:{}", b.display()), "denied")));
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
fn the_report_reads_the_backends_usage_rows() {
    let s = Setup::new();
    let backend = s.backend("call notes_read {}\ncall sync {}\nresult 4000", caps());
    s.run(&backend, 1, 1).expect("the run");
    let (mine, _) = backend.totals("agent-0");
    let m = report(&s.dir.join("run"), Some(&s.dir.join("claude/usage.jsonl"))).expect("the report").measures;
    assert_eq!(m.tokens, mine.input + mine.cache_read + mine.cache_write + mine.output);
    assert_eq!(m.output_tokens, 4000);
    // Claude Code names the tools mcp__experiment__<tool>: the sync still counts as integrating.
    assert!(m.overhead.integrate_tokens.is_some_and(|t| t > 0.0), "{:?}", m.overhead);
}

/// Counts of `kind` events in the run's log.
fn count(events_path: &Path, kind: &str) -> usize {
    events(events_path).iter().filter(|e| e["event"] == kind).count()
}

/// `n` tool calls, `pause` milliseconds apart.
fn calls(n: usize, pause: u64) -> String {
    (0..n).map(|_| format!("call notes_read {{}}\nsleep {pause}\n")).collect()
}

#[test]
fn a_turn_that_stops_after_the_refusal_is_counted_from_its_result() {
    let s = Setup::new();
    let backend = s.backend(&(calls(4, 0) + "say\nresult 10"), Caps { calls_per_turn: 3, ..caps() });
    let events_path = s.run_calls(&backend, 3).expect("the run");
    assert_eq!(turn_error_for(&s, &events_path), 0, "the turn ended itself");
    assert_eq!(count(&events_path, "tool_call"), 3, "the 4th call never reached a tool");
    assert_eq!(count(&events_path, "call_refused"), 1);
    let transcript = s.transcript("agent-0", 1).expect("a transcript");
    assert!(transcript.contains("tool calls are used up"), "the agent was told to finish");
    assert_eq!(backend.totals("agent-0").0.output, 10, "real output, not the ceiling");
    let rows = s.usage();
    assert!(rows.iter().any(|r| r["past_tool_call_cap"] == 4), "a row says the turn passed its cap");
    assert!(rows.iter().any(|r| r["output"] == 10 && r["ended"] == "success"));

    // At the cap exactly, nothing is refused.
    let s = Setup::new();
    let backend = s.backend(&(calls(3, 0) + "result 10"), Caps { calls_per_turn: 3, ..caps() });
    let events_path = s.run_calls(&backend, 3).expect("the run");
    assert_eq!(turn_error_for(&s, &events_path), 0, "the cap-th call is allowed");
    assert_eq!(count(&events_path, "call_refused"), 0);
    assert!(!s.usage().iter().any(|r| r["past_tool_call_cap"].is_u64()));
    assert_eq!(backend.totals("agent-0").0.output, 10);
}

/// Three runs, one per mode, stay within the owner's go for the pilot (decision `ea053c2c4d48`).
#[test]
fn the_pilots_run_caps_are_a_third_of_its_go_each() {
    let c = Caps::pilot();
    assert_eq!((c.run_tokens, c.run_output, c.run_time), (25_000_000, 1_666_666, Duration::from_secs(3600)));
    assert!(
        3 * c.run_tokens <= 75_000_000
            && 3 * c.run_output <= 5_000_000
            && 3 * c.run_time <= Duration::from_secs(3 * 3600)
    );
    assert_eq!((c.agent_tokens, c.agent_output, c.turn_output), (3_000_000, 180_000, 30_000), "unchanged");
}

/// Report `aba12882829a` point 6: at the 30,000 ceiling, the per-agent cap of 180,000 output tokens
/// stopped an agent after 6 of its 8 turns. Turns that reach the tool-call cap and end themselves
/// count their real output, so the agent gets all 8; the per-agent cap is unchanged.
#[test]
fn an_agent_whose_turns_end_softly_at_the_cap_gets_all_its_turns() {
    assert_eq!((caps().agent_output, caps().turn_output), (180_000, 30_000), "the pilot's figures");
    let s = Setup::new();
    let backend = s.backend(&(calls(4, 0) + "result 2000"), Caps { calls_per_turn: 3, ..caps() });
    let events_path = s.run_calls_for(&backend, 3, 8).expect("the run");
    assert_eq!(turn_error_for(&s, &events_path), 0);
    assert_eq!(count(&events_path, "call_refused"), 8, "every turn passed the cap");
    let ended = s.usage().iter().filter(|r| r["ended"] == "success").count();
    assert_eq!(ended, 8, "all 8 turns ran to their result");
    assert_eq!(backend.totals("agent-0").0.output, 8 * 2000);

    // The same turns, hard-stopped after the grace: each counts at the ceiling, and the agent stops after 6.
    let s = Setup::new();
    let grace = Caps { calls_per_turn: 3, cap_grace: Duration::from_millis(500), ..caps() };
    let backend = s.backend(&(calls(4, 0) + SLEEP + "\nresult 2000"), grace);
    let events_path = s.run_calls_for(&backend, 3, 8).expect("the run goes on");
    assert_eq!(s.usage().iter().filter(|r| r["output_counted_at"] == 30_000).count(), 6, "6 turns ran");
    assert_eq!(turn_error_for(&s, &events_path), 8, "6 hard stops and 2 turns refused for spent tokens");
    assert!(s.transcript("agent-0", 7).is_none());
    assert_eq!(backend.totals("agent-0").0.output, 180_000);
}

/// Decision `74d00180c642`: past the cap, one notes_write still runs and is saved; a second, and
/// any other tool, is refused.
#[test]
fn one_notes_write_runs_past_the_cap_and_no_other_call() {
    let s = Setup::new();
    let script = calls(3, 0)
        + "call notes_write {\"text\":\"kept past the cap\"}\n"
        + "call notes_write {\"text\":\"a second note\"}\n"
        + "call notes_read {}\n"
        + "result 10";
    let backend = s.backend(&script, Caps { calls_per_turn: 3, ..caps() });
    let events_path = s.run_calls_for(&backend, 3, 2).expect("the run");
    assert_eq!(turn_error_for(&s, &events_path), 0, "each turn ended itself");
    let log = events(&events_path);
    let tools = |turn: u64| -> Vec<String> {
        log.iter()
            .filter(|e| e["event"] == "tool_call" && e["data"]["turn"] == turn)
            .map(|e| e["data"]["tool"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    for turn in [1, 2] {
        assert_eq!(tools(turn), ["`notes_read`", "`notes_read`", "`notes_read`", "`notes_write`"], "turn {turn}");
    }
    assert_eq!(count(&events_path, "call_refused"), 4, "the second notes_write and the notes_read, each turn");
    let first = s.transcript("agent-0", 1).expect("a transcript");
    assert_eq!(first.matches("tool calls are used up").count(), 2, "both refusals told the agent");
    // The note written past the cap is the one the next turn reads.
    let second = s.transcript("agent-0", 2).expect("a transcript");
    let read = |note: &str| format!(r#"{{\"notes\":\"{note}\"}}"#);
    assert!(second.contains(&read("kept past the cap")) && !second.contains(&read("a second note")), "{second}");
    assert_eq!(backend.totals("agent-0").0.output, 20, "real output, counted from each turn's result");
}

#[test]
fn bounded_cuts_at_the_limit_in_characters_and_says_so() {
    use bracken_experiment::driver::bounded;
    let at = "é".repeat(MAX_RESULT_CHARS);
    assert_eq!(bounded(at.clone()), (at.clone(), None), "at the limit, untouched");
    let (text, cut) = bounded(at.clone() + "ab");
    assert_eq!(cut, Some(MAX_RESULT_CHARS + 2));
    assert!(text.starts_with(&at), "the first {MAX_RESULT_CHARS} characters, whole");
    let note = &text[at.len()..];
    assert!(note.starts_with("\n[Cut: this result has 40002 characters") && note.contains("narrow the call"), "{note}");
    assert!(note.chars().count() < 300);
}

/// A script that makes one result over [`MAX_RESULT_CHARS`] in `mode`, then a result event.
fn oversized(mode: &str) -> String {
    let big = |n: usize| "x".repeat(n);
    let moves = match mode {
        "bracken" => (0..3)
            .map(|i| format!("call claim {}\n", json!({"statement": format!("{i} {}", big(15_000)), "tags": ["note"]})))
            .chain(["call log {}\n".to_string()])
            .collect::<String>(),
        _ => (0..3)
            .map(|_| format!("call edit {}\n", json!({"file": "shape.stk", "start": 1, "end": 0, "text": big(15_000)})))
            .chain(["call read {\"file\":\"shape.stk\"}\n".to_string()])
            .collect::<String>(),
    };
    moves + "result 10"
}

#[test]
fn an_oversized_result_comes_back_bounded_with_a_note_in_every_mode() {
    for mode in ["bracken", "git", "crdt"] {
        let s = Setup::new();
        let backend = s.backend_in(mode, &oversized(mode), caps(), Pin::of(&fake(), "2.1.311").expect("pin"));
        let cfg = run_config(s.dir.join("run"), mode, 1, 1, true);
        let events_path = run(&cfg, &backend).expect("the run").events;
        assert_eq!(turn_error_for(&s, &events_path), 0, "{mode}");
        let log = events(&events_path);
        let results: Vec<&Value> = log.iter().filter(|e| e["event"] == "tool_result").map(|e| &e["data"]).collect();
        assert!(results[..3].iter().all(|r| r["ok"] == true && r["cut_from"].is_null()), "{mode}: {results:?}");
        let last = results[3];
        let cut = last["cut_from"].as_u64().expect("the read was cut") as usize;
        assert!(cut > MAX_RESULT_CHARS, "{mode}");
        let shown = last["bytes"].as_u64().expect("bytes") as usize;
        assert!(shown < MAX_RESULT_CHARS + 400, "{mode}: {shown} bytes shown");
        let transcript = s.transcript("agent-0", 1).expect("a transcript");
        let note = format!("[Cut: this result has {cut} characters and only the first {MAX_RESULT_CHARS} are shown.");
        assert!(transcript.contains(&note), "{mode}: the agent is told, and how to narrow the call");
    }
}

#[test]
fn a_turn_that_ignores_the_refusal_is_ended_after_the_grace_at_the_ceiling() {
    let s = Setup::new();
    let grace = Caps { calls_per_turn: 3, cap_grace: Duration::from_secs(2), turn_time: LONG_TURN, ..caps() };
    let backend = s.backend(&(calls(4, 0) + SLEEP + "\nresult 10"), grace);
    let start = Instant::now();
    let events_path = s.run_calls(&backend, 3).expect("the run goes on");
    assert!(start.elapsed() < EARLY, "the grace, not the turn's time, ended it");
    assert_eq!(turn_error_for(&s, &events_path), 1);
    assert_eq!(count(&events_path, "tool_call"), 3);
    assert_eq!(backend.totals("agent-0").0.output, caps().turn_output, "a turn without a result counts at the ceiling");
    assert!(s.usage().iter().any(|r| r["output_counted_at"] == caps().turn_output));
}

#[test]
fn a_turn_that_keeps_calling_is_ended_after_the_grace_calls() {
    let s = Setup::new();
    let grace = Caps { calls_per_turn: 3, cap_grace: LONG_TURN, cap_grace_calls: 3, turn_time: LONG_TURN, ..caps() };
    let backend = s.backend(&(calls(30, 100) + "result 10"), grace);
    let start = Instant::now();
    let events_path = s.run_calls(&backend, 3).expect("the run goes on");
    assert!(start.elapsed() < EARLY);
    assert_eq!(turn_error_for(&s, &events_path), 1);
    assert_eq!(count(&events_path, "tool_call"), 3, "no call past the cap reached a tool");
    // The call that crossed the cap and the three grace calls were refused; the next one ended the process,
    // perhaps after its own refusal.
    let refused = count(&events_path, "call_refused");
    assert!((4..=5).contains(&refused), "{refused} refused");
    assert_eq!(backend.totals("agent-0").0.output, caps().turn_output);
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
    // The cap must run out while a turn streams, so the check during a turn is the one that acts,
    // not the check before each turn. On a loaded machine the run's setup alone can take longer
    // than the cap, and then the check before the first turn stops the run, also rightly; so try
    // a longer cap until a turn has started before it runs out.
    let mut acted = None;
    for secs in [10, 30, 90] {
        let s = Setup::new();
        let caps = Caps { run_time: Duration::from_secs(secs), turn_time: LONG_TURN, ..caps() };
        let backend = s.backend(&format!("say\n{SLEEP}\nresult 10"), caps.clone());
        let start = Instant::now();
        let err = s.run(&backend, 2, 2).expect_err("the run stops");
        assert!(err.contains("time cap") && start.elapsed() < caps.run_time + EARLY, "{err}");
        if s.transcript("agent-0", 1).is_some() {
            acted = Some((s, caps));
            break;
        }
    }
    let (s, caps) = acted.expect("the cap acted while a turn ran, not only before one started");

    // The driver names the cap, so the report measures the run if the plan expects that stop, and not otherwise.
    let usage = s.dir.join("claude/usage.jsonl");
    let log = events(&s.dir.join("run/events.jsonl"));
    assert_eq!(
        log.last().map(|e| (e["event"].clone(), e["data"]["cap"].clone())),
        Some(("run_stopped".into(), RUN_TIME_CAP.into()))
    );
    let refused = report(&s.dir.join("run"), Some(&usage)).expect_err("not expected");
    assert!(refused.contains("not by a stop the plan expects"), "{refused}");
    let s = Setup::new();
    let backend = s.backend(&format!("say\n{SLEEP}\nresult 10"), caps);
    let expected = PlanInputs { expected_stops: vec![RUN_TIME_CAP.into()], ..PlanInputs::default() };
    let cfg = RunConfig { plan: expected, ..run_config(s.dir.join("run"), "bracken", 2, 2, true) };
    assert!(run(&cfg, &backend).is_err(), "the run stops");
    let m = report(&s.dir.join("run"), Some(&s.dir.join("claude/usage.jsonl"))).expect("measured").measures;
    assert_eq!(m.stopped.as_deref(), Some(RUN_TIME_CAP));
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
        .args(["--mode", "bracken", "--agents", "1", "--turns", "1", "--claude-version", "2.1.311"])
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
    // The driver writes the transcript as it reads the stream, so the env line may lag the fake.
    let port = || -> Option<u16> {
        let transcript = std::fs::read_to_string(dir.join("claude/transcripts/agent-0-1.jsonl")).ok()?;
        transcript.split("http://127.0.0.1:").nth(1)?.split('"').next()?.parse().ok()
    };
    while port().is_none() {
        assert!(start.elapsed() < Duration::from_secs(60), "no proxy port in the transcript");
        std::thread::sleep(Duration::from_millis(50));
    }
    let port = port().expect("the proxy's port");

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

/// Task `f0afdabe05b5`: a refusal is counted before its client sees the 403, so a count read right
/// after the reply never misses it.
#[test]
fn a_refusal_is_counted_before_its_client_sees_it() {
    use bracken_experiment::proxy::{Proxy, Route};
    let route = Route { host: HOST.into(), port: 443, connect: None };
    let proxy = Proxy::start(route, Duration::from_secs(120)).expect("proxy");
    for i in 1..=50 {
        let (reply, _) = connect_through(proxy.port(), "CONNECT example.com:443 HTTP/1.1\r\n\r\n");
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert_eq!(proxy.attempts().len(), i, "refusal {i} was counted before its reply");
    }
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
    // A tunnel is recorded once it closes, and the stand-in records a connection once it has read it.
    let start = Instant::now();
    while backend.proxy_attempts().is_empty() || s.endpoint.connections().is_empty() {
        assert!(start.elapsed() < EARLY, "no tunnel or no connection recorded");
        std::thread::sleep(Duration::from_millis(10));
    }
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
