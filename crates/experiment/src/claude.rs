//! The Claude Code backend (design sections 3 and 5): plays each agent turn
//! with one headless Claude Code process under the agent profile, and reads
//! its event stream as it arrives.
//!
//! - **Started as section 3 says**: the pinned binary (path and blake3 checked
//!   before every turn), fixed arguments, a turn configuration the agent
//!   profile can read but not write (checked before every turn), and an
//!   environment of exactly the agent's own directories, a minimal `PATH`,
//!   `HTTPS_PROXY` at the driver's forwarding proxy, non-essential traffic off,
//!   and the owner's routing entries.
//! - **The gate**: the first event must be `init`, showing exactly what the
//!   plan expects; anything else ends the turn and the run.
//! - **Token accounting**: every assistant message's usage and the tools it
//!   asked for, and the `result` event's usage, go to `usage.jsonl`.
//! - **Caps, failing closed**: tool calls, context and time per turn end the
//!   turn; an agent's tokens end its turns; the run's tokens, usage that can't
//!   be read, or a stream and result that disagree on input by more than 10%
//!   stop the run. Output is known only from the `result` event (the stream's
//!   figure is a lower bound), so a turn without one is counted at
//!   [`Caps::turn_output`].
//! - **Process groups**: Claude Code runs in a group led by a guard process,
//!   killed on every exit path; if the driver dies, the guard sees its pipe
//!   close and kills the group itself.
//! - **Transcripts**: each turn's event stream, checked for the auth value
//!   before it is kept.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::driver::Backend;
use crate::proxy::{Attempt, Proxy, Route};
use crate::sandbox::{AgentPaths, Sandbox, resolved};
use crate::worker::Lines;

/// The only entries the owner's settings may hold under `env`; any other stops the run before it starts.
pub const ROUTING_KEYS: [&str; 3] =
    ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"];
/// The run's own caps (design section 5): stops a plan expects, which the driver logs by name.
pub const RUN_TOKEN_CAP: &str = "the run's token cap is reached";
pub const RUN_TIME_CAP: &str = "the run's time cap is reached";

/// The message that starts each turn; the plan hashes it.
pub const TURN_MESSAGE: &str = "This is turn {turn} of yours. You are {name}.";

/// The MCP server name the shim is registered under: its tools are `mcp__experiment__<tool>`.
pub const SERVER: &str = "experiment";
/// The longest event line read from Claude Code.
const MAX_EVENT: usize = 8 * 1024 * 1024;
/// How long standard error may take to close once the group is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// How often a waiting turn looks at whether the run has stopped.
const POLL: Duration = Duration::from_millis(100);

/// The owner's routing entries, held in memory only. Its `Debug` shows the host, never a value.
pub struct Routing {
    pub host: String,
    pub port: u16,
    env: Vec<(String, String)>,
}

impl std::fmt::Debug for Routing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Routing {{ host: {}, port: {} }}", self.host, self.port)
    }
}

impl Routing {
    /// The routing entries of a Claude Code `settings.json`. Errors never quote a value.
    pub fn from_settings(path: &Path) -> Result<Routing, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let settings: Value = serde_json::from_str(&text).map_err(|_| format!("{} is not JSON", path.display()))?;
        let env = settings["env"].as_object().ok_or("the settings hold no env entries")?;
        let mut entries = Vec::new();
        for (k, v) in env {
            let v = v.as_str().ok_or_else(|| format!("{k} is not a string"))?;
            entries.push((k.clone(), v.to_string()));
        }
        Routing::new(entries)
    }

    /// Routing from entries; `ANTHROPIC_BASE_URL` (https) and `ANTHROPIC_AUTH_TOKEN` are required.
    pub fn new(env: Vec<(String, String)>) -> Result<Routing, String> {
        if let Some((k, _)) = env.iter().find(|(k, _)| !ROUTING_KEYS.contains(&k.as_str())) {
            return Err(format!("the settings' env holds {}, outside the routing entries", crate::args::short(k)));
        }
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let token = get("ANTHROPIC_AUTH_TOKEN").ok_or("no ANTHROPIC_AUTH_TOKEN")?;
        if token.len() < 8 {
            return Err("ANTHROPIC_AUTH_TOKEN is too short to check transcripts for".into());
        }
        let url = get("ANTHROPIC_BASE_URL").ok_or("no ANTHROPIC_BASE_URL")?;
        let rest = url.strip_prefix("https://").ok_or("ANTHROPIC_BASE_URL is not https")?;
        let authority = rest.split('/').next().unwrap_or_default();
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().map_err(|_| "ANTHROPIC_BASE_URL has a bad port")?),
            None => (authority, 443),
        };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
            return Err("ANTHROPIC_BASE_URL has a bad host".into());
        }
        Ok(Routing { host: host.to_ascii_lowercase(), port, env })
    }

    fn secret(&self) -> &str {
        self.env.iter().find(|(k, _)| k == "ANTHROPIC_AUTH_TOKEN").map(|(_, v)| v.as_str()).unwrap_or_default()
    }
}

/// The pinned Claude Code binary.
#[derive(Debug, Clone)]
pub struct Pin {
    /// Its real path: the versioned binary, not a package manager's `latest` link.
    pub path: PathBuf,
    /// blake3 of its bytes, hex.
    pub blake3: String,
    /// The `claude_code_version` its `init` event must show.
    pub version: String,
}

impl Pin {
    /// Pins the binary at `path` as it is now.
    pub fn of(path: &Path, version: &str) -> Result<Pin, String> {
        let path = std::fs::canonicalize(path).map_err(|e| format!("resolving {}: {e}", path.display()))?;
        Ok(Pin { blake3: file_hash(&path)?, path, version: version.into() })
    }

    fn check(&self) -> Result<(), String> {
        let real = std::fs::canonicalize(&self.path).map_err(|e| format!("the pinned binary: {e}"))?;
        if real != self.path || file_hash(&real)? != self.blake3 {
            return Err("the pinned Claude Code binary changed".into());
        }
        Ok(())
    }
}

fn file_hash(path: &Path) -> Result<String, String> {
    let mut h = blake3::Hasher::new();
    let mut f = File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    std::io::copy(&mut f, &mut h).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(h.finalize().to_hex().to_string())
}

/// What the `init` event must show, from the plan.
#[derive(Debug, Clone)]
pub struct Expect {
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    /// Where Claude Code says its credential came from: `none` when the auth token is in the environment.
    pub api_key_source: String,
    /// The bundled skills and plugins of the pinned version.
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
}

impl Expect {
    /// What Claude Code 2.1.311 shows with the turn configuration (note 24aa40160488).
    pub fn pinned_2_1_311(model: &str) -> Expect {
        let skills = [
            "deep-research",
            "dataviz",
            "update-config",
            "verify",
            "debug",
            "code-review",
            "simplify",
            "batch",
            "fewer-permission-prompts",
            "doctor",
            "loop",
            "claude-api",
            "workflow-authoring",
            "run",
            "run-skill-generator",
            "plugin-authoring",
        ];
        Expect {
            model: model.into(),
            effort: "medium".into(),
            permission_mode: "dontAsk".into(),
            api_key_source: "none".into(),
            skills: skills.iter().map(|s| s.to_string()).collect(),
            plugins: vec!["cc-plugin-agents-md".into(), "cc-plugin-plugin-authoring".into()],
        }
    }
}

/// Section 5's caps.
#[derive(Debug, Clone)]
pub struct Caps {
    /// Tool calls asked for in one turn; the process ends past it.
    pub calls_per_turn: u32,
    /// Context of one model call (input, cache reads and cache writes).
    pub context: u64,
    pub turn_time: Duration,
    /// The output a turn without a readable `result` event is counted at.
    pub turn_output: u64,
    /// `--max-budget-usd`: Claude Code's own stop, on its API-equivalent estimate, which sees output as it grows.
    pub turn_budget_usd: Option<f64>,
    pub agent_tokens: u64,
    pub agent_output: u64,
    pub run_tokens: u64,
    pub run_output: u64,
    /// Wall-clock time from the backend's start; past it the run stops.
    pub run_time: Duration,
}

impl Caps {
    /// The pilot's caps (design section 5).
    pub fn pilot() -> Caps {
        Caps {
            calls_per_turn: 25,
            context: 150_000,
            turn_time: Duration::from_secs(600),
            turn_output: 30_000,
            // About twice a turn's expected use at the pinned model's price (design section 5).
            turn_budget_usd: Some(1.0),
            agent_tokens: 3_000_000,
            agent_output: 180_000,
            run_tokens: 75_000_000,
            run_output: 5_000_000,
            run_time: Duration::from_secs(3 * 3600),
        }
    }
}

/// Everything the backend needs, fixed before the run.
pub struct ClaudeConfig {
    pub pin: Pin,
    pub expect: Expect,
    pub caps: Caps,
    pub routing: Routing,
    /// The backend's own directory, outside every store: agents' Claude Code directories, turn
    /// configurations, transcripts and `usage.jsonl`. Must not exist yet.
    pub dir: PathBuf,
    /// The `bracken-experiment` binary: the shim and the guard.
    pub shim: PathBuf,
    /// The base system prompt.
    pub prompt: String,
    /// The mode's tool names and the core's, as the driver serves them.
    pub tools: Vec<String>,
    /// Tests only: where the forwarding proxy connects instead of resolving the routing host.
    pub upstream: Option<std::net::SocketAddr>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Tokens {
    // Sums saturate: a figure too large to add pins the total at the top, so the caps trip.
    fn input_side(&self) -> u64 {
        self.input.saturating_add(self.cache_read).saturating_add(self.cache_write)
    }

    pub fn total(&self) -> u64 {
        self.input_side().saturating_add(self.output)
    }

    fn add(&mut self, o: &Tokens) {
        self.input = self.input.saturating_add(o.input);
        self.cache_read = self.cache_read.saturating_add(o.cache_read);
        self.cache_write = self.cache_write.saturating_add(o.cache_write);
        self.output = self.output.saturating_add(o.output);
    }

    /// What `self` has beyond `o`, field by field.
    fn beyond(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
            output: self.output.saturating_sub(o.output),
        }
    }

    fn max(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.max(o.input),
            cache_read: self.cache_read.max(o.cache_read),
            cache_write: self.cache_write.max(o.cache_write),
            output: self.output.max(o.output),
        }
    }

    /// A usage record, every field required: `snake` for the API's names, else Claude Code's camelCase.
    fn parse(v: &Value, snake: bool) -> Result<Tokens, String> {
        let names = if snake {
            ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"]
        } else {
            ["inputTokens", "cacheReadInputTokens", "cacheCreationInputTokens", "outputTokens"]
        };
        let get = |k: &str| v.get(k).and_then(Value::as_u64).ok_or_else(|| format!("usage without a number for {k}"));
        Ok(Tokens {
            input: get(names[0])?,
            cache_read: get(names[1])?,
            cache_write: get(names[2])?,
            output: get(names[3])?,
        })
    }
}

#[derive(Default)]
struct Totals {
    agents: BTreeMap<String, Tokens>,
    run: Tokens,
}

/// How a turn ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Result,
    /// A turn or agent cap: the process was ended.
    Cap(&'static str),
    /// The stream ended without a result.
    NoResult,
    /// The run must stop.
    Halt(String),
}

pub struct ClaudeBackend {
    cfg: ClaudeConfig,
    proxy: Proxy,
    totals: Mutex<Totals>,
    halt: Mutex<Option<String>>,
    usage: Mutex<File>,
    started: Instant,
}

impl ClaudeBackend {
    pub fn new(cfg: ClaudeConfig) -> Result<ClaudeBackend, String> {
        if cfg.dir.exists() {
            return Err(format!("{} already exists", cfg.dir.display()));
        }
        for sub in ["", "agents", "turn", "transcripts"] {
            make_dir(&cfg.dir.join(sub))?;
        }
        cfg.pin.check()?;
        let route = Route { host: cfg.routing.host.clone(), port: cfg.routing.port, connect: cfg.upstream };
        let proxy = Proxy::start(route, cfg.caps.turn_time)?;
        let usage = private_file(&cfg.dir.join("usage.jsonl"))?;
        Ok(ClaudeBackend {
            cfg,
            proxy,
            totals: Mutex::new(Totals::default()),
            halt: Mutex::new(None),
            usage: Mutex::new(usage),
            started: Instant::now(),
        })
    }

    /// Everything the forwarding proxy was asked so far.
    pub fn proxy_attempts(&self) -> Vec<Attempt> {
        self.proxy.attempts()
    }

    pub fn proxy_port(&self) -> u16 {
        self.proxy.port()
    }

    /// An agent's counted tokens, and the run's.
    pub fn totals(&self, agent: &str) -> (Tokens, Tokens) {
        let t = self.totals.lock().unwrap_or_else(|e| e.into_inner());
        (t.agents.get(agent).copied().unwrap_or_default(), t.run)
    }

    fn stop(&self, why: &str) {
        self.halt.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| why.to_string());
    }

    fn record(&self, row: Value) {
        let failed = writeln!(self.usage.lock().unwrap_or_else(|e| e.into_inner()), "{row}").is_err();
        if failed {
            self.stop("writing usage.jsonl failed");
        }
    }

    /// Adds `t` to the agent's and the run's totals, and says whether a cap is reached.
    fn count(&self, agent: &str, t: &Tokens) -> Option<End> {
        let caps = &self.cfg.caps;
        let mut totals = self.totals.lock().unwrap_or_else(|e| e.into_inner());
        totals.run.add(t);
        let run = totals.run;
        let mine = totals.agents.entry(agent.to_string()).or_default();
        mine.add(t);
        if run.total() >= caps.run_tokens || run.output >= caps.run_output {
            return Some(End::Halt(RUN_TOKEN_CAP.into()));
        }
        (mine.total() >= caps.agent_tokens || mine.output >= caps.agent_output).then_some(End::Cap("agent tokens"))
    }

    /// The turn configuration for an agent whose socket is `socket`: (file name, bytes).
    fn turn_files(&self, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        let mcp = json!({"mcpServers": {SERVER: {
            "type": "stdio",
            "command": "/usr/bin/env",
            "args": ["-i", self.cfg.shim, "shim", "--socket", socket],
        }}});
        let settings = json!({"permissions": {"allow": [format!("mcp__{SERVER}")], "defaultMode": "dontAsk"}});
        Ok(vec![
            ("prompt.md", self.cfg.prompt.as_bytes().to_vec()),
            ("mcp.json", serde_json::to_vec_pretty(&mcp).map_err(|e| e.to_string())?),
            ("settings.json", serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?),
        ])
    }

    /// Writes the agent's turn configuration if it isn't there yet, and checks it is exactly the plan's.
    fn turn_config(&self, name: &str, socket: &Path) -> Result<PathBuf, String> {
        let dir = self.cfg.dir.join("turn").join(name);
        let files = self.turn_files(socket)?;
        if !dir.exists() {
            make_dir(&dir)?;
            for (file, bytes) in &files {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o400)
                    .open(dir.join(file))
                    .map_err(|e| format!("writing {file}: {e}"))?;
                f.write_all(bytes).map_err(|e| format!("writing {file}: {e}"))?;
            }
        }
        for (file, bytes) in &files {
            let now = std::fs::read(dir.join(file)).map_err(|e| format!("reading {file}: {e}"))?;
            if blake3::hash(&now) != blake3::hash(bytes) {
                return Err(format!("the turn configuration's {file} changed"));
            }
        }
        Ok(dir)
    }

    fn own(&self, name: &str) -> PathBuf {
        self.cfg.dir.join("agents").join(name)
    }

    fn command(&self, name: &str, turn_dir: &Path, socket: &Path, message: &str) -> Result<Command, String> {
        let own = self.own(name);
        let (home, config, tmp) = (own.join("home"), own.join("config"), own.join("tmp"));
        for d in [&own, &home, &config, &tmp] {
            make_dir(d)?;
        }
        let pin = &self.cfg.pin;
        let install = pin.path.parent().ok_or("the pinned binary has no directory")?;
        let sandbox = Sandbox::agent(&AgentPaths {
            claude: &pin.path,
            install,
            own: &own,
            turn: turn_dir,
            shim: &self.cfg.shim,
            socket,
            proxy_port: self.proxy.port(),
        })?;
        let mut cmd = sandbox.command(&pin.path);
        cmd.arg("-p").arg(message);
        cmd.args(["--model", &self.cfg.expect.model, "--effort", &self.cfg.expect.effort]);
        cmd.arg("--system-prompt-file").arg(turn_dir.join("prompt.md"));
        cmd.args(["--tools", "", "--strict-mcp-config", "--mcp-config"]).arg(turn_dir.join("mcp.json"));
        cmd.args(["--setting-sources", "", "--settings"]).arg(turn_dir.join("settings.json"));
        cmd.args(["--permission-mode", &self.cfg.expect.permission_mode, "--no-session-persistence"]);
        cmd.args(["--output-format", "stream-json", "--verbose"]);
        if let Some(usd) = self.cfg.caps.turn_budget_usd {
            cmd.arg("--max-budget-usd").arg(format!("{usd}"));
        }
        // Claude Code writes /tmp/claude-<uid> unless CLAUDE_CODE_TMPDIR says otherwise (note 24aa40160488).
        let tmp = resolved(&tmp)?;
        cmd.env_clear()
            .env("HOME", resolved(&home)?)
            .env("CLAUDE_CONFIG_DIR", resolved(&config)?)
            .env("TMPDIR", &tmp)
            .env("CLAUDE_CODE_TMPDIR", &tmp)
            .env("PATH", "/usr/bin:/bin")
            .env("HTTPS_PROXY", format!("http://127.0.0.1:{}", self.proxy.port()))
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .envs(self.cfg.routing.env.iter().map(|(k, v)| (k, v)))
            .current_dir(resolved(&home)?)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }

    /// The gate: the `init` event must show exactly the plan.
    fn gate(&self, init: &Value, own: &Path) -> Result<(), String> {
        let e = &self.cfg.expect;
        let field = |k: &str| init.get(k).cloned().unwrap_or(Value::Null);
        let eq = |k: &str, want: &str| {
            if field(k).as_str() == Some(want) { Ok(()) } else { Err(format!("the init event's {k} isn't {want}")) }
        };
        if field("type") != "system" || field("subtype") != "init" {
            return Err("the first event isn't init".into());
        }
        eq("claude_code_version", &self.cfg.pin.version)?;
        eq("model", &e.model)?;
        eq("permissionMode", &e.permission_mode)?;
        eq("apiKeySource", &e.api_key_source)?;
        let set = |k: &str, pick: fn(&Value) -> Option<String>| -> Result<BTreeSet<String>, String> {
            let items = init.get(k).and_then(Value::as_array).ok_or_else(|| format!("the init event has no {k}"))?;
            items.iter().map(|v| pick(v).ok_or_else(|| format!("a bad entry in {k}"))).collect()
        };
        let want: BTreeSet<String> = self.cfg.tools.iter().map(|t| format!("mcp__{SERVER}__{t}")).collect();
        if set("tools", |v| v.as_str().map(String::from))? != want {
            return Err("the init event's tools aren't exactly the mode's".into());
        }
        let servers = set("mcp_servers", |v| Some(format!("{}:{}", v["name"].as_str()?, v["status"].as_str()?)))?;
        if servers != BTreeSet::from([format!("{SERVER}:connected")]) {
            return Err("the init event's MCP servers aren't exactly the shim".into());
        }
        if set("skills", |v| v.as_str().map(String::from))? != e.skills.iter().cloned().collect() {
            return Err("the init event's skills aren't the bundled ones".into());
        }
        if set("plugins", |v| v["name"].as_str().map(String::from))? != e.plugins.iter().cloned().collect() {
            return Err("the init event's plugins aren't the bundled ones".into());
        }
        let memory = init["memory_paths"]["auto"].as_str().ok_or("the init event has no memory path")?;
        if !Path::new(memory).starts_with(resolved(own)?) || memory.contains("..") {
            return Err("the init event's memory path is outside the agent's directory".into());
        }
        Ok(())
    }

    /// Plays one turn and says how it ended.
    fn play(&self, name: &str, turn: u32, socket: &Path) -> Result<End, String> {
        let turn_dir =
            self.cfg.pin.check().and_then(|()| self.turn_config(name, socket)).map_err(|e| self.halting(e))?;
        let message = TURN_MESSAGE.replace("{turn}", &turn.to_string()).replace("{name}", name);
        let mut cmd = self.command(name, &turn_dir, socket, &message)?;
        let path = self.cfg.dir.join("transcripts").join(format!("{name}-{turn}.jsonl"));
        let mut transcript = Transcript::create(&path, self.cfg.routing.secret())?;
        let group = Group::start(&self.cfg.shim, &mut cmd)?;
        let (lines, stderr) = (read_lines(group.stdout), read_stderr(group.stderr));
        let end = self.watch(name, turn, &lines, &mut transcript);
        drop(group.guard); // kills the group
        let err = stderr.recv_timeout(EXIT_GRACE).unwrap_or_default();
        let end = match transcript.finish(&err) {
            Err(e) => End::Halt(e),
            Ok(()) => end,
        };
        if let End::Halt(why) = &end {
            self.stop(why);
        }
        Ok(end)
    }

    fn halting(&self, why: String) -> String {
        self.stop(&why);
        why
    }

    /// Reads the event stream until the turn ends.
    fn watch(&self, name: &str, turn: u32, lines: &Receiver<Result<String, String>>, out: &mut Transcript) -> End {
        let caps = &self.cfg.caps;
        let deadline = Instant::now() + caps.turn_time;
        let mut gated = false;
        // Per message id, what has been counted: each content block repeats its message's usage.
        let mut messages: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut tool_uses: BTreeSet<String> = BTreeSet::new();
        // Whether a result event's figures were counted.
        let mut settled = false;
        let ended = loop {
            if let Some(why) = self.halted() {
                break End::Halt(why);
            }
            let now = Instant::now();
            if now >= self.started + caps.run_time {
                break End::Halt(RUN_TIME_CAP.into());
            }
            if now >= deadline {
                break End::Cap("turn time");
            }
            let line = match lines.recv_timeout(POLL.min(deadline - now)) {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => break End::Halt(format!("reading the event stream: {e}")),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break End::NoResult,
            };
            if let Err(e) = out.line(&line) {
                break End::Halt(e);
            }
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                break End::Halt("an event that isn't JSON".into());
            };
            if !gated {
                let verdict = self.gate(&event, &self.own(name));
                self.record(json!({"agent": name, "turn": turn, "gate": verdict.is_ok()}));
                if let Err(e) = verdict {
                    break End::Halt(format!("the gate refused {name}'s turn {turn}: {e}"));
                }
                gated = true;
                continue;
            }
            match event["type"].as_str() {
                Some("assistant") => {
                    let m = &event["message"];
                    let (Some(id), Ok(usage)) = (m["id"].as_str(), Tokens::parse(&m["usage"], true)) else {
                        break End::Halt("an assistant message without readable usage".into());
                    };
                    let blocks = m["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "tool_use");
                    let tools: Option<Vec<(&str, &str)>> = blocks
                        .map(|c| {
                            Some((c["id"].as_str().filter(|i| !i.is_empty())?, c["name"].as_str().unwrap_or_default()))
                        })
                        .collect();
                    let Some(tools) = tools else {
                        break End::Halt("a tool call without an id".into());
                    };
                    let seen = messages.entry(id.to_string()).or_default();
                    let grown = usage.beyond(seen);
                    seen.add(&grown);
                    self.record(json!({
                        "agent": name, "turn": turn, "message": crate::args::short(id), "model": m["model"],
                        "input": usage.input, "cache_read": usage.cache_read, "cache_write": usage.cache_write,
                        "output_at_least": usage.output, "tools": tools.iter().map(|(_, n)| n).collect::<Vec<_>>(),
                    }));
                    tool_uses.extend(tools.iter().map(|(id, _)| id.to_string()));
                    if let Some(end) = self.count(name, &grown) {
                        break end;
                    }
                    if usage.input_side() >= caps.context {
                        break End::Cap("context");
                    }
                    if tool_uses.len() > caps.calls_per_turn as usize {
                        break End::Cap("tool calls");
                    }
                }
                Some("result") => {
                    let streamed = messages.values().fold(Tokens::default(), |mut a, t| {
                        a.add(t);
                        a
                    });
                    break self.settle(name, turn, &event, &streamed, &mut settled);
                }
                _ => {}
            }
        };
        if !gated && !matches!(ended, End::Halt(_)) {
            return End::Halt(format!("{name}'s turn {turn} ended before its init event"));
        }
        if ended != End::Result && !settled {
            // No result counted, however the turn ended: its output counts at the larger of its cap
            // and what the stream already counted, and the row says what was counted.
            let streamed: u64 = messages.values().map(|t| t.output).fold(0, u64::saturating_add);
            let rest = Tokens { output: caps.turn_output.saturating_sub(streamed), ..Tokens::default() };
            let at = caps.turn_output.max(streamed);
            self.record(json!({"agent": name, "turn": turn, "ended": format!("{ended:?}"), "output_counted_at": at}));
            if let (Some(End::Halt(why)), false) = (self.count(name, &rest), matches!(ended, End::Halt(_))) {
                return End::Halt(why);
            }
        }
        ended
    }

    /// The result event: counts the larger of it and the stream, and stops the run if its usage is
    /// missing or the two disagree on input by more than 10%.
    fn settle(&self, name: &str, turn: u32, event: &Value, streamed: &Tokens, settled: &mut bool) -> End {
        let usage = match Tokens::parse(&event["usage"], true) {
            Ok(u) => u,
            Err(e) => return End::Halt(format!("the result event: {e}")),
        };
        let Some(models) = event["modelUsage"].as_object().filter(|m| !m.is_empty()) else {
            return End::Halt("the result event has no modelUsage".into());
        };
        let mut by_model = Tokens::default();
        for (model, v) in models {
            match Tokens::parse(v, false) {
                Ok(t) => by_model.add(&t),
                Err(e) => return End::Halt(format!("the result's usage for {}: {e}", crate::args::short(model))),
            }
        }
        let result = usage.max(&by_model);
        let (a, b) = (result.input_side(), streamed.input_side());
        let (low, high) = (a.min(b), a.max(b));
        let gap = high > 0 && (high - low).saturating_mul(10) > high;
        let counted = result.max(streamed);
        *settled = true;
        self.record(json!({
            "agent": name, "turn": turn, "ended": event["subtype"], "input": counted.input,
            "cache_read": counted.cache_read, "cache_write": counted.cache_write, "output": counted.output,
            "cost_usd": event["total_cost_usd"], "duration_ms": event["duration_ms"],
        }));
        // The stream's figures are already in the totals: add only the rest. A gap is counted, then stops the run.
        let capped = self.count(name, &counted.beyond(streamed));
        if gap {
            return End::Halt(format!("{name}'s turn {turn}: the stream and the result disagree on input by over 10%"));
        }
        match capped {
            Some(End::Halt(why)) => End::Halt(why),
            _ => End::Result,
        }
    }
}

impl Backend for ClaudeBackend {
    fn turn(&self, _agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        if let Some(why) = self.halted() {
            return Err(why);
        }
        let (mine, run) = self.totals(name);
        let caps = &self.cfg.caps;
        if run.total() >= caps.run_tokens || run.output >= caps.run_output {
            return Err(self.halting(RUN_TOKEN_CAP.into()));
        }
        if self.started.elapsed() >= caps.run_time {
            return Err(self.halting(RUN_TIME_CAP.into()));
        }
        if mine.total() >= caps.agent_tokens || mine.output >= caps.agent_output {
            return Err(format!("{name} has spent its tokens"));
        }
        match self.play(name, turn, socket)? {
            End::Result => Ok(()),
            End::Cap(cap) => Err(format!("{name}'s turn {turn} ended at its {cap} cap")),
            End::NoResult => Err(format!("{name}'s turn {turn} ended without a result")),
            End::Halt(why) => Err(why),
        }
    }

    fn halted(&self) -> Option<String> {
        self.halt.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn make_dir(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("creating {}: {e}", path.display()))
}

fn private_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("creating {}: {e}", path.display()))
}

/// Claude Code's process group, led by a guard.
struct Group {
    guard: Guard,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
}

/// Dropping it kills the whole group.
struct Guard {
    leader: Child,
    claude: Child,
}

impl Group {
    /// Starts `bin guard` as a new group's leader, then `cmd` in that group.
    fn start(bin: &Path, cmd: &mut Command) -> Result<Group, String> {
        let leader = Command::new(bin)
            .arg("guard")
            .env_clear()
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting the guard: {e}"))?;
        let pgid = leader.id() as i32;
        let mut claude = match cmd.process_group(pgid).spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = kill_group(pgid);
                return Err(format!("starting Claude Code: {e}"));
            }
        };
        let (Some(stdout), Some(stderr)) = (claude.stdout.take(), claude.stderr.take()) else {
            let _ = kill_group(pgid);
            return Err("Claude Code has no pipes".into());
        };
        Ok(Group { guard: Guard { leader, claude }, stdout, stderr })
    }
}

fn kill_group(pgid: i32) -> Result<(), String> {
    let pid = rustix::process::Pid::from_raw(pgid).ok_or("no group")?;
    rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).map_err(|e| e.to_string())
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = kill_group(self.leader.id() as i32);
        let _ = self.claude.kill();
        let _ = self.claude.wait();
        let _ = self.leader.kill();
        let _ = self.leader.wait();
    }
}

/// `bracken-experiment guard`: waits for its standard input to close (the driver ended or died),
/// then kills its own process group, which holds Claude Code and everything it started.
pub fn guard() -> Result<(), String> {
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
    rustix::process::kill_process_group(rustix::process::getpgrp(), rustix::process::Signal::KILL)
        .map_err(|e| e.to_string())
}

/// The event stream's lines, as they arrive.
fn read_lines(stdout: std::process::ChildStdout) -> Receiver<Result<String, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut lines = Lines::new(BufReader::new(stdout), MAX_EVENT);
        let mut line = String::new();
        loop {
            let next = lines.next_line(&mut line);
            let more = matches!(next, Ok(true));
            let sent = match next {
                Ok(true) => tx.send(Ok(line.clone())),
                Ok(false) => Ok(()),
                Err(e) => tx.send(Err(e)),
            };
            if !more || sent.is_err() {
                return;
            }
        }
    });
    rx
}

/// At most 64 KiB of standard error, once it closes.
fn read_stderr(stderr: std::process::ChildStderr) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// A turn's transcript: written as the stream arrives, refusing any line holding the auth value,
/// and checked again whole before it is kept.
struct Transcript<'a> {
    path: PathBuf,
    file: File,
    secret: &'a str,
}

impl<'a> Transcript<'a> {
    fn create(path: &Path, secret: &'a str) -> Result<Transcript<'a>, String> {
        Ok(Transcript { path: path.to_path_buf(), file: private_file(path)?, secret })
    }

    fn line(&mut self, line: &str) -> Result<(), String> {
        if line.contains(self.secret) {
            return Err("the event stream held the auth value".into());
        }
        writeln!(self.file, "{line}").map_err(|e| format!("writing the transcript: {e}"))
    }

    /// Keeps the transcript, with `stderr` beside it, only if neither holds the auth value; else removes it.
    fn finish(mut self, stderr: &[u8]) -> Result<(), String> {
        let written = self.file.flush().and_then(|()| std::fs::read(&self.path));
        let leaked = |b: &[u8]| b.windows(self.secret.len()).any(|w| w == self.secret.as_bytes());
        if !written.is_ok_and(|b| !leaked(&b) && !leaked(stderr)) {
            let _ = std::fs::remove_file(&self.path);
            return Err("a transcript held the auth value, or couldn't be read back; it was removed".into());
        }
        let err_path = self.path.with_extension("stderr");
        private_file(&err_path)?.write_all(stderr).map_err(|e| format!("writing {}: {e}", err_path.display()))
    }
}
