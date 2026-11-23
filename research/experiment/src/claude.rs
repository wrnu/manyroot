//! The Claude Code harness (design sections 3 and 5): plays each agent turn
//! with one headless Claude Code process, behind the harness-neutral
//! [`crate::backend`], which builds and sandboxes the process and does the
//! checks, accounting, caps, process groups and transcripts.
//!
//! - **Started as section 3 says**: the pinned binary, fixed arguments, a turn
//!   configuration (the system prompt, the shim's MCP entry and the settings),
//!   and beyond the backend's base environment only Claude Code's
//!   configuration and temporary directories inside the agent's own,
//!   non-essential traffic off ([`ENV_NAMES`]), and the run's sign-in
//!   ([`SignIn`]): the owner's gateway routing entries, or the owner's
//!   subscription token alone.
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
use crate::sandbox::{Profile, resolved};

/// The only entries the owner's settings may hold under `env`; any other stops the run before it starts.
pub const ROUTING_KEYS: [&str; 3] =
    ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"];

/// The environment entries Claude Code adds to the backend's base whatever its sign-in: its own directories and
/// non-essential traffic off. The sign-in adds its own ([`SignIn::names`]).
pub const ENV_NAMES: [&str; 3] =
    ["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_TMPDIR", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"];

/// The one entry a subscription sign-in sets: the long-lived token `claude setup-token` prints, which Claude Code
/// reads from here and uses non-interactively (checked on 2.1.329: no login prompt, `apiKeySource` `none`, and its
/// only connection is to [`SUBSCRIPTION_HOST`]).
pub const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Where a subscription sign-in's model calls go: Claude Code's own default API host.
pub const SUBSCRIPTION_HOST: &str = "api.anthropic.com";

/// The largest token file read: a token is about a hundred bytes.
const MAX_TOKEN_FILE: u64 = 4096;

/// The shortest subscription token taken.
const MIN_TOKEN: usize = 20;

/// The owner's subscription token, held in memory only. Its `Debug` shows nothing of it.
pub struct Subscription {
    token: String,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Subscription")
    }
}

impl Subscription {
    /// The token in `path` (`--claude-token-file`): one line, the token alone, in a file only its owner may read.
    /// Read once, into memory; nothing writes the file. Errors never quote it.
    pub fn from_file(path: &Path) -> Result<Subscription, String> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let shown = path.display();
        // Opened once, never through a symbolic link, never waiting on a FIFO, and checked by what was opened, so
        // nothing swapped in between a check and the read passes (finding 46569298796f).
        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC;
        let file =
            std::fs::OpenOptions::new().read(true).custom_flags(flags.bits() as i32).open(path).map_err(|e| match e
                .raw_os_error()
            {
                Some(code) if code == rustix::io::Errno::LOOP.raw_os_error() => {
                    format!("{shown} is a symbolic link: give the token file itself")
                }
                _ => format!("reading {shown}: {e}"),
            })?;
        let meta = file.metadata().map_err(|e| format!("reading {shown}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{shown} isn't a file"));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(format!("{shown} may be read by others than its owner: chmod 600 it"));
        }
        let too_long = || format!("{shown} is over {MAX_TOKEN_FILE} bytes, too long to hold a token alone");
        if meta.len() > MAX_TOKEN_FILE {
            return Err(too_long());
        }
        // At most one byte past the bound, whatever the file grows to as it's read.
        let mut bytes = Vec::new();
        file.take(MAX_TOKEN_FILE + 1).read_to_end(&mut bytes).map_err(|e| format!("reading {shown}: {e}"))?;
        if bytes.len() as u64 > MAX_TOKEN_FILE {
            return Err(too_long());
        }
        let text = String::from_utf8(bytes).map_err(|_| format!("{shown} isn't text"))?;
        Subscription::new(text.trim().to_string()).map_err(|e| format!("{shown}: {e}"))
    }

    /// A token as given: at least [`MIN_TOKEN`] bytes, each of A-Z, a-z, 0-9 and `._+/=-`, so JSON writes it as it is
    /// and the transcript check finds it as it is (finding de508082cfc6). Errors never quote it.
    pub fn new(token: String) -> Result<Subscription, String> {
        if token.len() < MIN_TOKEN {
            return Err(format!("the token is under {MIN_TOKEN} bytes"));
        }
        if !token.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+/=-".contains(&b)) {
            return Err("the token holds a character outside A-Z, a-z, 0-9 and ._+/=-, so the run can't check \
                        transcripts for it: give it alone"
                .into());
        }
        Ok(Subscription { token })
    }
}

/// How Claude Code signs in for a run's model calls: through the owner's gateway, by its routing entries
/// (`--settings`), or with the owner's subscription token (`--claude-token-file`). The run's record and so its
/// plan say which ([`SignIn::kind`]), never a value.
#[derive(Debug)]
pub enum SignIn {
    Gateway(Routing),
    Subscription(Subscription),
}

impl SignIn {
    /// Which sign-in, as the plan records it.
    pub fn kind(&self) -> &'static str {
        match self {
            SignIn::Gateway(_) => "gateway",
            SignIn::Subscription(_) => "subscription",
        }
    }

    /// The environment entries it sets, by name: the routing entries, or the token's one.
    pub fn names(&self) -> &'static [&'static str] {
        match self {
            SignIn::Gateway(_) => &ROUTING_KEYS,
            SignIn::Subscription(_) => std::slice::from_ref(&OAUTH_TOKEN),
        }
    }

    /// The entries it sets, with their values: for the turn's environment alone.
    fn env(&self) -> Vec<(&str, &str)> {
        match self {
            SignIn::Gateway(r) => r.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
            SignIn::Subscription(s) => vec![(OAUTH_TOKEN, s.token.as_str())],
        }
    }

    /// Where its model calls go.
    fn host(&self) -> (String, u16) {
        match self {
            SignIn::Gateway(r) => (r.host.clone(), r.port),
            SignIn::Subscription(_) => (SUBSCRIPTION_HOST.into(), 443),
        }
    }

    /// What no transcript or standard error may hold.
    fn secret(&self) -> &str {
        match self {
            SignIn::Gateway(r) => r.secret(),
            SignIn::Subscription(s) => &s.token,
        }
    }
}

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
        let (host, port) = https_host(url, "ANTHROPIC_BASE_URL")?;
        Ok(Routing { host, port, env })
    }

    fn secret(&self) -> &str {
        self.get("ANTHROPIC_AUTH_TOKEN").unwrap_or_default()
    }

    /// One routing entry's value, for a harness in this crate.
    pub(crate) fn get(&self, k: &str) -> Option<&str> {
        self.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// The host (lowercased) and port of `url`, which must be https; `what` names it in errors, which never
/// quote it.
pub(crate) fn https_host(url: &str, what: &str) -> Result<(String, u16), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| format!("{what} is not https"))?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("{what} has a bad port"))?),
        None => (authority, 443),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return Err(format!("{what} has a bad host"));
    }
    Ok((host.to_ascii_lowercase(), port))
}

/// What the `init` event must show, from the plan: whatever the version, the start event must show the plan's model,
/// permission mode and credential source (decision `2e583e378924`).
#[derive(Debug, Clone)]
pub struct Expect {
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    /// Where Claude Code says its credential came from: `none` when the auth token is in the environment.
    pub api_key_source: String,
}

impl Expect {
    /// The plan's start event for `model`, at medium effort, in the driver's turn configuration.
    pub fn new(model: &str) -> Expect {
        Expect {
            model: model.into(),
            effort: "medium".into(),
            permission_mode: "dontAsk".into(),
            api_key_source: "none".into(),
        }
    }
}

/// What a start event showed that varies with the version: the version, and its skills and plugins by name.
#[derive(Debug, Clone, PartialEq)]
pub struct StartEvent {
    pub version: String,
    pub skills: BTreeSet<String>,
    pub plugins: BTreeSet<String>,
}

impl StartEvent {
    /// As a turn's gate record keeps it.
    pub fn record(&self) -> Value {
        json!({"version": self.version, "skills": self.skills, "plugins": self.plugins})
    }

    /// A later turn's start event must show its run's first turn's skills and plugins (finding `aaa686b72100`): a
    /// different version may show others, but not partway through a run.
    pub fn same_as(&self, first: &StartEvent) -> Result<(), String> {
        for (k, mine, theirs) in [("skills", &self.skills, &first.skills), ("plugins", &self.plugins, &first.plugins)] {
            if mine != theirs {
                return Err(format!("the init event's {k} aren't the run's first turn's"));
            }
        }
        Ok(())
    }
}

/// The gate (decision `2e583e378924`): what the runner relies on in Claude Code's `init` event, checked field by
/// field whatever the version, each refusal naming its field. It must be the init event of the pinned `version`,
/// with the plan's model, permission mode and credential source; offer exactly the mode's `tools` through exactly
/// the shim; list its skills, and its plugins all builtin, so nothing of the owner's own config reached the agent;
/// run in the agent's `home`; and keep its memory inside the agent's own directory `own`. What it showed that varies
/// with the version, it returns.
pub fn check_start_event(
    init: &Value,
    e: &Expect,
    version: &str,
    tools: &[String],
    home: &Path,
    own: &Path,
) -> Result<StartEvent, String> {
    let has = |k: &str| init.get(k).filter(|v| !v.is_null()).ok_or_else(|| format!("the init event has no {k}"));
    let eq = |k: &str, want: &str| {
        if has(k)?.as_str() == Some(want) { Ok(()) } else { Err(format!("the init event's {k} isn't {want}")) }
    };
    if init.get("type").and_then(Value::as_str) != Some("system")
        || init.get("subtype").and_then(Value::as_str) != Some("init")
    {
        return Err("the first event isn't init (its type and subtype)".into());
    }
    eq("claude_code_version", version)?;
    eq("model", &e.model)?;
    eq("permissionMode", &e.permission_mode)?;
    eq("apiKeySource", &e.api_key_source)?;
    let list = |k: &str| has(k)?.as_array().ok_or_else(|| format!("the init event's {k} isn't a list"));
    let set = |k: &str, pick: fn(&Value) -> Option<String>| -> Result<BTreeSet<String>, String> {
        list(k)?.iter().map(|v| pick(v).ok_or_else(|| format!("a bad entry in the init event's {k}"))).collect()
    };
    let want: BTreeSet<String> = tools.iter().map(|t| format!("mcp__{SERVER}__{t}")).collect();
    if set("tools", |v| v.as_str().map(String::from))? != want {
        return Err("the init event's tools aren't exactly the mode's".into());
    }
    let servers = set("mcp_servers", |v| Some(format!("{}:{}", v["name"].as_str()?, v["status"].as_str()?)))?;
    if servers != BTreeSet::from([format!("{SERVER}:connected")]) {
        return Err("the init event's mcp_servers aren't exactly the shim".into());
    }
    let skills = set("skills", |v| v.as_str().map(String::from))?;
    let plugins = set("plugins", |v| v["name"].as_str().map(String::from))?;
    // Each plugin's own path, exactly (finding `6f43bdd61405`).
    if let Some(p) = list("plugins")?.iter().find(|p| p["path"].as_str() != Some("builtin")) {
        let name = p["name"].as_str().unwrap_or_default();
        return Err(format!("the init event's plugins hold {}, whose path isn't builtin", crate::args::short(name)));
    }
    if has("cwd")?.as_str().map(PathBuf::from) != Some(resolved(home)?) {
        return Err("the init event's cwd isn't the agent's home".into());
    }
    let memory = has("memory_paths")?.get("auto").and_then(Value::as_str);
    let memory = memory.ok_or("the init event's memory_paths has no auto")?;
    if !Path::new(memory).starts_with(resolved(own)?) || memory.contains("..") {
        return Err("the init event's memory_paths.auto is outside the agent's directory".into());
    }
    Ok(StartEvent { version: version.into(), skills, plugins })
}

/// The efforts `--effort` takes for Claude Code.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Everything the backend needs, fixed before the run.
pub struct ClaudeConfig {
    pub pin: Pin,
    pub expect: Expect,
    pub caps: Caps,
    pub sign_in: SignIn,
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
        let ClaudeConfig { pin, expect, caps, sign_in, dir, shim, prompt, turns, tools, upstream } = cfg;
        let backend = BackendConfig { caps: caps.clone(), dir, shim: shim.clone(), turns };
        let names = ENV_NAMES.iter().chain(sign_in.names()).copied().collect();
        let first = std::sync::Mutex::new(None);
        let harness = ClaudeCode { pin, expect, caps, sign_in, names, shim, prompt, tools, upstream, first };
        HarnessBackend::start(harness, backend)
    }
}

/// The Claude Code [`Harness`], from a [`ClaudeConfig`].
pub struct ClaudeCode {
    pin: Pin,
    expect: Expect,
    caps: Caps,
    sign_in: SignIn,
    /// [`ENV_NAMES`] and the sign-in's.
    names: Vec<&'static str>,
    shim: PathBuf,
    prompt: String,
    tools: Vec<String>,
    upstream: Option<std::net::SocketAddr>,
    /// What the run's first gated turn's start event showed: every later turn's must show the same (finding
    /// `aaa686b72100`).
    first: std::sync::Mutex<Option<StartEvent>>,
}

impl ClaudeCode {
    /// What the run's first gated turn's start event showed, if a turn has been gated.
    pub fn start_event(&self) -> Option<StartEvent> {
        self.first.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

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

    /// The gate: the `init` event must show the plan ([`check_start_event`]), and what the run's first turn's showed;
    /// the turn's gate record keeps what it showed.
    fn gate(&self, init: &Value, turn: &Turn) -> Result<Value, String> {
        let seen = check_start_event(init, &self.expect, turn.version, &self.tools, turn.home, turn.own)?;
        let mut first = self.first.lock().unwrap_or_else(|e| e.into_inner());
        match &*first {
            Some(f) => seen.same_as(f)?,
            None => *first = Some(seen.clone()),
        }
        Ok(seen.record())
    }

    /// An assistant message: its id, model, usage and tool calls.
    fn message(m: &Value) -> Result<Event, String> {
        // Each refusal names the field it found missing or changed (decision `2e583e378924`, finding
        // `0c108f394b12`): what the run records and counts must be there, as it reads it.
        let id = m["id"].as_str().filter(|i| !i.is_empty()).ok_or("an assistant message without an id")?;
        let model = m["model"].as_str().ok_or("an assistant message without a model")?;
        let usage = tokens(&m["usage"], true).map_err(|e| format!("an assistant message's {e}"))?;
        let content = m["content"].as_array().ok_or("an assistant message whose content isn't a list")?;
        let blocks = content.iter().filter(|c| c["type"] == "tool_use");
        let tools: Result<Vec<(String, String)>, String> = blocks
            .map(|c| {
                let id = c["id"].as_str().filter(|i| !i.is_empty()).ok_or("a tool call without an id")?;
                let name = c["name"].as_str().ok_or("a tool call without a name")?;
                Ok((id.to_string(), name.to_string()))
            })
            .collect();
        let tools = tools?;
        // Claude Code refuses a name it doesn't hold itself; nothing here halts on one, as before (answer
        // `98c18ce63bfc`).
        Ok(Event::Message { id: id.to_string(), model: json!(model), usage, tools, unknown: Vec::new() })
    }

    /// The result event: the larger of its `usage` and its `modelUsage`, both required.
    fn result(event: &Value) -> Result<Event, String> {
        let usage = tokens(&event["usage"], true).map_err(|e| format!("the result event: {e}"))?;
        let ended = event["subtype"].as_str().ok_or("the result event has no subtype")?;
        let models = event["modelUsage"].as_object().filter(|m| !m.is_empty());
        let models = models.ok_or("the result event has no modelUsage")?;
        let mut by_model = Tokens::default();
        for (model, v) in models {
            let t =
                tokens(v, false).map_err(|e| format!("the result's usage for {}: {e}", crate::args::short(model)))?;
            by_model.add(&t);
        }
        Ok(Event::Result {
            usage: Some(usage.max(&by_model)),
            ended: json!(ended),
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
        let (host, port) = self.sign_in.host();
        Route { host, port, connect: self.upstream }
    }

    fn secret(&self) -> &str {
        self.sign_in.secret()
    }

    fn pin(&self) -> &Pin {
        &self.pin
    }

    fn profile(&self) -> Profile {
        Profile::Agent
    }

    fn about(&self) -> Value {
        json!({"result": "the harness's", "sign_in": self.sign_in.kind()})
    }

    fn env_names(&self) -> &[&str] {
        &self.names
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
        // The sign-in's entries, in the environment alone: never an argument or a file the run writes.
        for (k, v) in self.sign_in.env() {
            l.env(k, v);
        }
        Ok(l)
    }

    fn event(&self, line: &str, first: bool, turn: &Turn) -> Result<Event, String> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err("an event that isn't JSON".into());
        };
        if first {
            return Ok(Event::Init(self.gate(&event, turn)));
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
