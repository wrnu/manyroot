//! The Claude Code harness (design sections 3 and 5): plays each agent turn
//! with one headless Claude Code process, behind the harness-neutral
//! [`crate::backend`], which builds and sandboxes the process and does the
//! checks, accounting, caps, process groups and transcripts.
//!
//! - **Started as section 3 says**: the pinned binary, fixed arguments, a turn
//!   configuration (the system prompt, the shim's MCP entry and the settings),
//!   and beyond the backend's base environment only Claude Code's
//!   configuration and temporary directories inside the agent's own,
//!   non-essential traffic off, and the owner's routing entries ([`ENV_NAMES`]).
//! - **The gate**: the first event must be `init`, showing exactly what the
//!   plan expects.
//! - **Its event stream** (`stream-json`): every assistant message's usage and
//!   the tools it asked for, and the `result` event's usage, the larger of its
//!   `usage` and its `modelUsage`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backend::{BackendConfig, Event, Harness, HarnessBackend, Launch, Turn, make_dir};
// Re-exported where the tests and the binaries have always found them.
pub use crate::backend::{
    Caps, ModeTokenCap, Pin, REPLICATE_TOKENS, RUN_TIME_CAP, RUN_TOKEN_CAP, TURN_MESSAGE, Tokens, guard,
};
use crate::proxy::Route;
use crate::sandbox::resolved;

/// The only entries the owner's settings may hold under `env`; any other stops the run before it starts.
pub const ROUTING_KEYS: [&str; 3] =
    ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"];

/// The environment entries Claude Code adds to the backend's base: its own directories, non-essential
/// traffic off, and the routing entries.
pub const ENV_NAMES: [&str; 6] = [
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_TMPDIR",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    ROUTING_KEYS[0],
    ROUTING_KEYS[1],
    ROUTING_KEYS[2],
];

/// The MCP server name the shim is registered under: its tools are `mcp__experiment__<tool>`.
pub const SERVER: &str = "experiment";

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
    /// The system prompt: [`crate::prompts::system_prompt`] in a real run.
    pub prompt: String,
    /// Each agent's turns in the run, for the turn message.
    pub turns: u32,
    /// The mode's tool names and the core's, as the driver serves them.
    pub tools: Vec<String>,
    /// Tests only: where the forwarding proxy connects instead of resolving the routing host.
    pub upstream: Option<std::net::SocketAddr>,
}

/// The backend with Claude Code playing each turn.
pub type ClaudeBackend = HarnessBackend<ClaudeCode>;

impl HarnessBackend<ClaudeCode> {
    pub fn new(cfg: ClaudeConfig) -> Result<ClaudeBackend, String> {
        let ClaudeConfig { pin, expect, caps, routing, dir, shim, prompt, turns, tools, upstream } = cfg;
        let backend = BackendConfig { caps: caps.clone(), dir, shim: shim.clone(), turns };
        HarnessBackend::start(ClaudeCode { pin, expect, caps, routing, shim, prompt, tools, upstream }, backend)
    }
}

/// The Claude Code [`Harness`], from a [`ClaudeConfig`].
pub struct ClaudeCode {
    pin: Pin,
    expect: Expect,
    caps: Caps,
    routing: Routing,
    shim: PathBuf,
    prompt: String,
    tools: Vec<String>,
    upstream: Option<std::net::SocketAddr>,
}

impl ClaudeCode {
    /// The turn configuration for an agent whose socket is `socket`: (file name, bytes).
    fn turn_files(&self, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        let mcp = json!({"mcpServers": {SERVER: {
            "type": "stdio",
            "command": "/usr/bin/env",
            "args": ["-i", self.shim, "shim", "--socket", socket],
        }}});
        let settings = json!({"permissions": {"allow": [format!("mcp__{SERVER}")], "defaultMode": "dontAsk"}});
        Ok(vec![
            ("prompt.md", self.prompt.as_bytes().to_vec()),
            ("mcp.json", serde_json::to_vec_pretty(&mcp).map_err(|e| e.to_string())?),
            ("settings.json", serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?),
        ])
    }

    /// The gate: the `init` event must show exactly the plan.
    fn gate(&self, init: &Value, own: &Path) -> Result<(), String> {
        let e = &self.expect;
        let field = |k: &str| init.get(k).cloned().unwrap_or(Value::Null);
        let eq = |k: &str, want: &str| {
            if field(k).as_str() == Some(want) { Ok(()) } else { Err(format!("the init event's {k} isn't {want}")) }
        };
        if field("type") != "system" || field("subtype") != "init" {
            return Err("the first event isn't init".into());
        }
        eq("claude_code_version", &self.pin.version)?;
        eq("model", &e.model)?;
        eq("permissionMode", &e.permission_mode)?;
        eq("apiKeySource", &e.api_key_source)?;
        let set = |k: &str, pick: fn(&Value) -> Option<String>| -> Result<BTreeSet<String>, String> {
            let items = init.get(k).and_then(Value::as_array).ok_or_else(|| format!("the init event has no {k}"))?;
            items.iter().map(|v| pick(v).ok_or_else(|| format!("a bad entry in {k}"))).collect()
        };
        let want: BTreeSet<String> = self.tools.iter().map(|t| format!("mcp__{SERVER}__{t}")).collect();
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

    /// An assistant message: its id, model, usage and tool calls.
    fn message(m: &Value) -> Result<Event, String> {
        let (Some(id), Ok(usage)) = (m["id"].as_str(), tokens(&m["usage"], true)) else {
            return Err("an assistant message without readable usage".into());
        };
        let blocks = m["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "tool_use");
        let tools: Option<Vec<(String, String)>> = blocks
            .map(|c| {
                let id = c["id"].as_str().filter(|i| !i.is_empty())?;
                Some((id.to_string(), c["name"].as_str().unwrap_or_default().to_string()))
            })
            .collect();
        let tools = tools.ok_or("a tool call without an id")?;
        Ok(Event::Message { id: id.to_string(), model: m["model"].clone(), usage, tools })
    }

    /// The result event: the larger of its `usage` and its `modelUsage`, both required.
    fn result(event: &Value) -> Result<Event, String> {
        let usage = tokens(&event["usage"], true).map_err(|e| format!("the result event: {e}"))?;
        let models = event["modelUsage"].as_object().filter(|m| !m.is_empty());
        let models = models.ok_or("the result event has no modelUsage")?;
        let mut by_model = Tokens::default();
        for (model, v) in models {
            let t =
                tokens(v, false).map_err(|e| format!("the result's usage for {}: {e}", crate::args::short(model)))?;
            by_model.add(&t);
        }
        Ok(Event::Result {
            usage: usage.max(&by_model),
            ended: event["subtype"].clone(),
            cost_usd: event["total_cost_usd"].clone(),
            duration_ms: event["duration_ms"].clone(),
        })
    }
}

impl Harness for ClaudeCode {
    fn name(&self) -> &str {
        "Claude Code"
    }

    fn route(&self) -> Route {
        Route { host: self.routing.host.clone(), port: self.routing.port, connect: self.upstream }
    }

    fn secret(&self) -> &str {
        self.routing.secret()
    }

    fn pin(&self) -> &Pin {
        &self.pin
    }

    fn env_names(&self) -> &[&str] {
        &ENV_NAMES
    }

    fn config(&self, _name: &str, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        self.turn_files(socket)
    }

    fn launch(&self, turn: &Turn) -> Result<Launch, String> {
        let config = turn.own.join("config");
        make_dir(&config)?;
        let mut l = Launch::default();
        l.arg("-p").arg(turn.message);
        l.args(["--model", &self.expect.model, "--effort", &self.expect.effort]);
        l.arg("--system-prompt-file").arg(turn.config.join("prompt.md"));
        l.args(["--tools", "", "--strict-mcp-config", "--mcp-config"]).arg(turn.config.join("mcp.json"));
        l.args(["--setting-sources", "", "--settings"]).arg(turn.config.join("settings.json"));
        l.args(["--permission-mode", &self.expect.permission_mode, "--no-session-persistence"]);
        l.args(["--output-format", "stream-json", "--verbose"]);
        if let Some(usd) = self.caps.turn_budget_usd {
            l.arg("--max-budget-usd").arg(format!("{usd}"));
        }
        // Claude Code writes /tmp/claude-<uid> unless CLAUDE_CODE_TMPDIR says otherwise (note 24aa40160488).
        l.env("CLAUDE_CONFIG_DIR", resolved(&config)?)
            .env("CLAUDE_CODE_TMPDIR", turn.tmp)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        for (k, v) in &self.routing.env {
            l.env(k, v);
        }
        Ok(l)
    }

    fn event(&self, line: &str, first: bool, turn: &Turn) -> Result<Event, String> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err("an event that isn't JSON".into());
        };
        if first {
            return Ok(Event::Init(self.gate(&event, turn.own)));
        }
        match event["type"].as_str() {
            Some("assistant") => ClaudeCode::message(&event["message"]),
            Some("result") => ClaudeCode::result(&event),
            _ => Ok(Event::Other),
        }
    }
}

/// A usage record, every field required: `snake` for the API's names, else Claude Code's camelCase.
fn tokens(v: &Value, snake: bool) -> Result<Tokens, String> {
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
