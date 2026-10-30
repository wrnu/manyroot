//! The Pi harness (task 68675575fd58; design note e19ff6fc8285, answers 8064daa71ddf and
//! cc34f6275877): plays each agent turn with one Pi process in JSON mode, behind the harness-neutral
//! [`crate::backend`], as [`crate::claude`] plays them with Claude Code.
//!
//! - **What runs**: the node of a prefix `scripts/pi-prefix` built, with Pi's CLI file as its first
//!   argument, never the `pi` link, whose `#!` the sandbox would refuse (gotcha e5274831d152). The pin
//!   is the whole prefix ([`Pin::tree`]); the profile is [`Profile::Node`].
//! - **Its configuration**: the turn configuration, read-only and checked before every turn: the system
//!   prompt, settings, the provider's base URL and the shim as the one MCP server, its tools declared to
//!   the model (`exposure: direct`). No generated value starts with `!`, which Pi would run as a command
//!   (security note b8bbcf81bc17 (c)), and no key or header is written at all: the credential is in the
//!   environment, as Claude Code's is. Pi writes beside what it reads (a lock beside `settings.json`, its
//!   credential store), so its agent directory (`PI_CODING_AGENT_DIR`) is made new in the agent's own
//!   before every turn, holding links to each checked file: Pi reads exactly the checked bytes, the
//!   sandbox lets nothing write through the links, and nothing it writes outlives the turn.
//! - **The gate**: Pi's first event, its session header, shows no model or tools, so it passes only as
//!   a version-3 header whose `cwd` is the agent's home; what it can't show is fixed by the pin and the
//!   checked configuration, every assistant message from another provider or model stops the run, and
//!   only the shim's tools are allowed (`--tools mcp__experiment__*`).
//! - **Its result**: Pi gives no total of its own, so the turn's result is the stream's sum, which the
//!   backend records as such and doesn't check against the stream (answer cc34f6275877 (2)). Pi's
//!   compaction is off, and one beginning stops the run: a summary call reports its usage only in
//!   `compaction_end`, so its tokens would escape the totals and the caps (finding ac5a014447a1).
//! - **The run's record** says all this ([`Harness::about`]): where results come from, compaction off,
//!   and whether a test CA was trusted (finding 695de67678dd).
//! - **Its provider** ([`PiCredential`]): `anthropic`, routed by the owner's Claude Code settings, or
//!   `openrouter` at Pi's own base URL for it, with the key from a Pi `auth.json` (task 19228e53a342,
//!   note ea4e2f77d70e): a stored API key, or the key Pi's OpenRouter login holds as `access` (decision
//!   467e1c8dbc09, note 9c39dc45be07), which Pi sends exactly as it sends an API key. Either way the forwarding proxy reaches only that host, and the key reaches Pi
//!   only in the one environment entry Pi reads for the provider.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::backend::{BackendConfig, Caps, Event, Harness, HarnessBackend, Launch, Pin, Tokens, Turn, make_dir};
use crate::claude::{Routing, SERVER, https_host};
use crate::proxy::Route;
use crate::sandbox::{Profile, resolved};

/// The environment entries Pi adds to the backend's base, before its provider's key: its agent and
/// session directories and network activity of its own off. Nothing else reaches it but the key's one
/// entry ([`PROVIDERS`]) and, for a stub provider in tests only, `NODE_EXTRA_CA_CERTS` (note
/// 5f9353037f2f).
pub const ENV_NAMES: [&str; 5] =
    ["PI_CODING_AGENT_DIR", "PI_CODING_AGENT_SESSION_DIR", "PI_OFFLINE", "PI_SKIP_VERSION_CHECK", "PI_TELEMETRY"];

/// The providers a run may name, each with the one environment entry Pi reads its key from.
pub const PROVIDERS: [(&str, &str); 2] = [("anthropic", "ANTHROPIC_AUTH_TOKEN"), ("openrouter", "OPENROUTER_API_KEY")];

/// OpenRouter's API, Pi 1.4.0's base URL for its `openrouter` provider.
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// How much longer than the run's time cap an OpenRouter login must stay valid, so that one expiring
/// during the run is refused at the start too (decision 467e1c8dbc09).
pub const LOGIN_MARGIN: Duration = Duration::from_secs(30 * 60);

/// A Pi `auth.json`, as far as the run reads it: the `openrouter` entry's type, and its stored key or
/// its login's `access` and `expires`. The parser skips everything else, the login's `refresh` and
/// every other provider included, without keeping it.
#[derive(serde::Deserialize)]
struct AuthFile {
    openrouter: Option<AuthEntry>,
}

#[derive(serde::Deserialize)]
struct AuthEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
    key: Option<String>,
    access: Option<String>,
    /// Milliseconds since the epoch, as Pi writes it.
    expires: Option<serde_json::Number>,
}

/// The session header version Pi 1.4.0 writes.
pub const HEADER_VERSION: u64 = 3;

/// Pi's CLI file in a prefix `scripts/pi-prefix` built.
pub const CLI: &str = "pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js";

/// The model every assistant message must come from, and the thinking level asked for.
#[derive(Debug, Clone)]
pub struct PiExpect {
    pub provider: String,
    pub model: String,
    pub thinking: String,
}

/// Everything the backend needs, fixed before the run.
pub struct PiConfig {
    /// A prefix `scripts/pi-prefix` built: node, and Pi's packages under `pi/`.
    pub prefix: PathBuf,
    /// The Pi version the prefix must hold.
    pub version: String,
    pub expect: PiExpect,
    pub caps: Caps,
    /// Where model calls go, and their key; for [`PiExpect::provider`].
    pub credential: PiCredential,
    /// The backend's own directory, as [`crate::claude::ClaudeConfig::dir`]. Must not exist yet.
    pub dir: PathBuf,
    /// The `bracken-experiment` binary: the shim and the guard.
    pub shim: PathBuf,
    pub prompt: String,
    pub turns: u32,
    /// The mode's tool names and the core's, as the driver serves them.
    pub tools: Vec<String>,
    /// Tests only: where the forwarding proxy connects instead of resolving the routing host.
    pub upstream: Option<std::net::SocketAddr>,
}

/// Where a provider's model calls go and the key they carry, held in memory only. Its `Debug` shows the
/// provider and host, never the key, and no error quotes it.
pub struct PiCredential {
    provider: &'static str,
    key_name: &'static str,
    base_url: String,
    host: String,
    port: u16,
    key: String,
}

impl std::fmt::Debug for PiCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PiCredential {{ provider: {}, host: {}, port: {} }}", self.provider, self.host, self.port)
    }
}

impl PiCredential {
    /// `provider`'s calls to `base_url` (https), carrying `key`, which must be at least 8 bytes for the
    /// transcripts to be checked for it.
    pub fn new(provider: &str, base_url: &str, key: String) -> Result<PiCredential, String> {
        let (provider, key_name) = PROVIDERS
            .into_iter()
            .find(|(p, _)| *p == provider)
            .ok_or_else(|| format!("Pi's provider {} isn't one a run may use", crate::args::short(provider)))?;
        let (host, port) = https_host(base_url, &format!("{provider}'s base URL"))?;
        if key.len() < 8 {
            return Err(format!("{provider}'s key is too short to check transcripts for"));
        }
        Ok(PiCredential { provider, key_name, base_url: base_url.to_string(), host, port, key })
    }

    /// Anthropic, routed by the owner's Claude Code settings.
    pub fn anthropic(routing: &Routing) -> Result<PiCredential, String> {
        let url = routing.get("ANTHROPIC_BASE_URL").ok_or("no ANTHROPIC_BASE_URL")?;
        let key = routing.get("ANTHROPIC_AUTH_TOKEN").ok_or("no ANTHROPIC_AUTH_TOKEN")?;
        PiCredential::new("anthropic", url, key.to_string())
    }

    /// OpenRouter at Pi's own base URL for it ([`OPENROUTER_BASE_URL`]), with the key in the
    /// `openrouter` entry of the Pi `auth.json` at `path`: a stored API key (`type: api_key`), or the
    /// key Pi's OpenRouter login holds (`type: oauth`), its `access`, used as it is. Pi sends either as
    /// the same bearer (note 9c39dc45be07). A login must stay valid until `until`; one that won't is
    /// refused, never refreshed, and nothing writes the file (decision 467e1c8dbc09). A key Pi would
    /// run or expand (`!…`, `$…`), or one with a character a transcript could hold escaped, is refused
    /// (finding b85965153a9b). No error quotes a value.
    pub fn openrouter(path: &Path, until: SystemTime) -> Result<PiCredential, String> {
        let p = path.display();
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {p}: {e}"))?;
        // A fixed message: the parser's own can quote a value.
        let auth: AuthFile = serde_json::from_str(&text)
            .map_err(|_| format!("{p} isn't a Pi auth.json with a readable openrouter entry"))?;
        let entry = auth.openrouter.ok_or_else(|| format!("{p} has no openrouter entry"))?;
        let key = match entry.kind.as_deref() {
            Some("api_key") => entry.key.ok_or_else(|| format!("{p}'s openrouter entry has no key"))?,
            Some("oauth") => {
                let access = entry.access.filter(|a| !a.is_empty());
                let access = access.ok_or_else(|| format!("{p}'s OpenRouter login holds no access key"))?;
                let expires = entry.expires.and_then(|n| n.as_f64());
                let expires =
                    expires.ok_or_else(|| format!("{p}'s OpenRouter login says nothing of when it expires"))?;
                let until = until.duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis() as f64;
                if expires < until {
                    return Err(format!(
                        "{p}'s OpenRouter login has expired or would expire before the run could end: log in \
                         to Pi again (/login openrouter), then start the run again"
                    ));
                }
                access
            }
            _ => return Err(format!("{p}'s openrouter entry is neither an API key nor an OpenRouter login")),
        };
        if key.starts_with('!') {
            return Err(format!("{p}'s openrouter key is a command, which the run won't run"));
        }
        if !key.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
            return Err(format!(
                "{p}'s openrouter key holds a character outside A-Z, a-z, 0-9, '.', '_' and '-' ('$', say, which \
                 Pi would expand), so the run can't check transcripts for it"
            ));
        }
        PiCredential::new("openrouter", OPENROUTER_BASE_URL, key)
    }

    pub fn provider(&self) -> &str {
        self.provider
    }
}

/// The backend with Pi playing each turn.
pub type PiBackend = HarnessBackend<Pi>;

impl HarnessBackend<Pi> {
    pub fn new(cfg: PiConfig) -> Result<PiBackend, String> {
        Self::with(cfg, None)
    }

    /// Tests only: as [`HarnessBackend::<Pi>::new`], and node also trusts the CA in `ca` (PEM), for a
    /// stub provider. A real run never has it (answer cc34f6275877 (3)); hidden from the docs, and the
    /// run's record says `test_ca: true` (finding 695de67678dd).
    #[doc(hidden)]
    pub fn with_test_ca(cfg: PiConfig, ca: Vec<u8>) -> Result<PiBackend, String> {
        Self::with(cfg, Some(ca))
    }

    fn with(cfg: PiConfig, test_ca: Option<Vec<u8>>) -> Result<PiBackend, String> {
        let PiConfig { prefix, version, expect, caps, credential, dir, shim, prompt, turns, tools, upstream } = cfg;
        if expect.provider != credential.provider {
            return Err(format!(
                "the plan's provider is {}, but the key is {}'s",
                crate::args::short(&expect.provider),
                credential.provider
            ));
        }
        let pin = Pin::tree(&prefix.join("bin/node"), &prefix, &version)?;
        let cli = resolved(&prefix.join(CLI))?;
        let package = cli.parent().and_then(Path::parent).and_then(Path::parent).ok_or("Pi's CLI has no package")?;
        let found = package_version(&package.join("package.json"))?;
        if found != version {
            return Err(format!("the prefix holds Pi {}, not {}", crate::args::short(&found), version));
        }
        let backend = BackendConfig { caps, dir, shim: shim.clone(), turns };
        let env_names = env_names(&credential, test_ca.is_some());
        let pi = Pi { pin, cli, expect, credential, env_names, shim, prompt, tools, upstream, test_ca };
        HarnessBackend::start(pi, backend)
    }
}

/// What Pi's turns may add to the backend's base: [`ENV_NAMES`], the credential's one entry and, with a
/// test CA, `NODE_EXTRA_CA_CERTS`.
fn env_names(credential: &PiCredential, test_ca: bool) -> Vec<&'static str> {
    ENV_NAMES.into_iter().chain([credential.key_name]).chain(test_ca.then_some("NODE_EXTRA_CA_CERTS")).collect()
}

fn package_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("{} isn't JSON", path.display()))?;
    v["version"].as_str().map(String::from).ok_or_else(|| format!("{} has no version", path.display()))
}

/// The Pi [`Harness`], from a [`PiConfig`].
pub struct Pi {
    pin: Pin,
    cli: PathBuf,
    expect: PiExpect,
    credential: PiCredential,
    env_names: Vec<&'static str>,
    shim: PathBuf,
    prompt: String,
    tools: Vec<String>,
    upstream: Option<std::net::SocketAddr>,
    test_ca: Option<Vec<u8>>,
}

impl Pi {
    /// The turn configuration, Pi's agent directory, for an agent whose socket is `socket`.
    fn turn_files(&self, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        let mcp = json!({"mcpServers": {SERVER: {
            "command": "/usr/bin/env",
            "args": ["-i", self.shim, "shim", "--socket", socket],
            "exposure": "direct",
        }}});
        // The base URL alone: for OpenRouter it's Pi's own, written so the checked file says where calls go.
        let models = json!({"providers": {self.credential.provider: {"baseUrl": self.credential.base_url}}});
        let json = |v: &Value| -> Result<Vec<u8>, String> {
            if let Some(s) = commands(v) {
                return Err(format!("the Pi configuration holds {}, which Pi would run", crate::args::short(&s)));
            }
            serde_json::to_vec_pretty(v).map_err(|e| e.to_string())
        };
        let mut files = vec![
            ("prompt.md", self.prompt.as_bytes().to_vec()),
            ("settings.json", json(&json!({"compaction": {"enabled": false}}))?),
            ("models.json", json(&models)?),
            ("mcp.json", json(&mcp)?),
        ];
        if let Some(ca) = &self.test_ca {
            files.push(("test-ca.pem", ca.clone()));
        }
        Ok(files)
    }

    /// The gate: the session header, version 3, in the agent's home.
    fn gate(header: &Value, turn: &Turn) -> Result<(), String> {
        if header["type"] != "session" {
            return Err("the first event isn't the session header".into());
        }
        if header["version"].as_u64() != Some(HEADER_VERSION) {
            return Err(format!("the session header's version isn't {HEADER_VERSION}"));
        }
        if header["cwd"].as_str().map(PathBuf::from) != Some(resolved(turn.home)?) {
            return Err("the session header's cwd isn't the agent's home".into());
        }
        Ok(())
    }

    /// A finished assistant message: from the plan's model, with its usage and tool calls.
    fn message(&self, m: &Value) -> Result<Event, String> {
        let (provider, model) = (m["provider"].as_str(), m["model"].as_str());
        if provider != Some(self.expect.provider.as_str()) || model != Some(self.expect.model.as_str()) {
            return Err(format!(
                "an assistant message from {}/{}, not the plan's {}/{}",
                crate::args::short(provider.unwrap_or("?")),
                crate::args::short(model.unwrap_or("?")),
                self.expect.provider,
                self.expect.model
            ));
        }
        let u = &m["usage"];
        let get = |k: &str| u.get(k).and_then(Value::as_u64).ok_or_else(|| format!("usage without a number for {k}"));
        let usage = Tokens {
            input: get("input")?,
            cache_read: get("cacheRead")?,
            cache_write: get("cacheWrite")?,
            output: get("output")?,
        };
        // Pi's own id for the response, else its timestamp: unique within a turn either way.
        let id = match (m["responseId"].as_str(), m["timestamp"].as_u64()) {
            (Some(id), _) if !id.is_empty() => id.to_string(),
            (_, Some(at)) => format!("at-{at}"),
            _ => return Err("an assistant message with neither a response id nor a timestamp".into()),
        };
        let calls = m["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "toolCall");
        let tools: Option<Vec<(String, String)>> = calls
            .map(|c| {
                let id = c["id"].as_str().filter(|i| !i.is_empty())?;
                Some((id.to_string(), c["name"].as_str().unwrap_or_default().to_string()))
            })
            .collect();
        let tools = tools.ok_or("a tool call without an id")?;
        // The header can't show the tools Pi declared, so each call shows it: only the shim's.
        let shim =
            |n: &str| n.strip_prefix(&format!("mcp__{SERVER}__")).is_some_and(|t| self.tools.iter().any(|x| x == t));
        if let Some((_, n)) = tools.iter().find(|(_, n)| !shim(n)) {
            return Err(format!("a call to {}, which isn't one of the shim's tools", crate::args::short(n)));
        }
        Ok(Event::Message { id, model: m["model"].clone(), usage, tools })
    }
}

/// Pi's agent directory for a turn: `own/pi-agent`, made new, holding a link to each file of the turn
/// configuration `config`, resolved.
fn agent_dir(own: &Path, config: &Path) -> Result<PathBuf, String> {
    let agent = own.join("pi-agent");
    if std::fs::symlink_metadata(&agent).is_ok() {
        std::fs::remove_dir_all(&agent).map_err(|e| format!("clearing {}: {e}", agent.display()))?;
    }
    make_dir(&agent)?;
    for entry in std::fs::read_dir(config).map_err(|e| format!("reading {}: {e}", config.display()))? {
        let name = entry.map_err(|e| format!("reading {}: {e}", config.display()))?.file_name();
        std::os::unix::fs::symlink(config.join(&name), agent.join(&name))
            .map_err(|e| format!("linking {}: {e}", agent.join(&name).display()))?;
    }
    resolved(&agent)
}

/// The first string in `v` that starts with `!`, which Pi runs as a command where it reads a key or a
/// header.
fn commands(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if s.starts_with('!') => Some(s.clone()),
        Value::Array(items) => items.iter().find_map(commands),
        Value::Object(m) => m.iter().find_map(|(k, v)| if k.starts_with('!') { Some(k.clone()) } else { commands(v) }),
        _ => None,
    }
}

impl Harness for Pi {
    fn name(&self) -> &str {
        "Pi"
    }

    fn route(&self) -> Route {
        Route { host: self.credential.host.clone(), port: self.credential.port, connect: self.upstream }
    }

    fn secret(&self) -> &str {
        &self.credential.key
    }

    fn pin(&self) -> &Pin {
        &self.pin
    }

    fn profile(&self) -> Profile {
        Profile::Node
    }

    fn about(&self) -> Value {
        json!({
            "result": "the stream's sum",
            "compaction": "off",
            "test_ca": self.test_ca.is_some(),
            "provider": self.credential.provider,
            "host": self.credential.host,
            "key_entry": self.credential.key_name,
        })
    }

    fn env_names(&self) -> &[&str] {
        &self.env_names
    }

    fn config(&self, _name: &str, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        self.turn_files(socket)
    }

    fn launch(&self, turn: &Turn) -> Result<Launch, String> {
        let sessions = turn.own.join("pi-sessions");
        make_dir(&sessions)?;
        let agent = agent_dir(turn.own, turn.config)?;
        let mut l = Launch::default();
        l.arg(&self.cli).args(["--mode", "json"]);
        l.arg("--model").arg(format!("{}/{}", self.expect.provider, self.expect.model));
        l.args(["--thinking", &self.expect.thinking]);
        l.arg("--system-prompt").arg(turn.config.join("prompt.md"));
        l.arg("--tools").arg(format!("mcp__{SERVER}__*"));
        // No extension but the built-in MCP support, which is one: --no-extensions alone turns MCP off.
        l.args(["--no-session", "--no-extensions", "-e", "builtin:mcp"]);
        l.args(["--no-skills", "--no-prompt-templates", "--no-context-files"]);
        l.args(["--no-themes", "--no-approve", "--offline"]);
        l.arg(turn.message);
        l.env("PI_CODING_AGENT_DIR", agent)
            .env("PI_CODING_AGENT_SESSION_DIR", resolved(&sessions)?)
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_TELEMETRY", "0")
            .env(self.credential.key_name, self.secret());
        if self.test_ca.is_some() {
            l.env("NODE_EXTRA_CA_CERTS", turn.config.join("test-ca.pem"));
        }
        Ok(l)
    }

    fn event(&self, line: &str, first: bool, turn: &Turn) -> Result<Event, String> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Err("an event that isn't JSON".into());
        };
        if first {
            return Ok(Event::Init(Pi::gate(&event, turn)));
        }
        match event["type"].as_str() {
            Some("message_end") if event["message"]["role"] == "assistant" => self.message(&event["message"]),
            Some("compaction_start") => Err(format!(
                "Pi began compacting ({}), which the plan turns off: its summary call's tokens would escape the totals",
                crate::args::short(event["reason"].as_str().unwrap_or("?"))
            )),
            Some("agent_settled") => Ok(Event::Result {
                usage: None,
                ended: json!(if event["aborted"] == true { "aborted" } else { "settled" }),
                cost_usd: Value::Null,
                duration_ms: Value::Null,
            }),
            _ => Ok(Event::Other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "fake-auth-value-not-a-secret-0123456789";

    fn pi(test_ca: Option<Vec<u8>>) -> Pi {
        let routing = Routing::new(vec![
            ("ANTHROPIC_BASE_URL".into(), "https://llm.stub.invalid".into()),
            ("ANTHROPIC_AUTH_TOKEN".into(), TOKEN.into()),
        ])
        .expect("routing");
        with_credential(PiCredential::anthropic(&routing).expect("a credential"), "claude-opus-5-5", test_ca)
    }

    fn with_credential(credential: PiCredential, model: &str, test_ca: Option<Vec<u8>>) -> Pi {
        let exe = std::env::current_exe().expect("the test binary");
        Pi {
            pin: Pin::of(&exe, "1.4.0").expect("a pin"),
            cli: exe.clone(),
            expect: PiExpect { provider: credential.provider.into(), model: model.into(), thinking: "high".into() },
            env_names: env_names(&credential, test_ca.is_some()),
            credential,
            shim: PathBuf::from("/bin/shim"),
            prompt: "the prompt".into(),
            tools: vec!["notes_read".into(), "claim".into()],
            upstream: None,
            test_ca,
        }
    }

    /// A turn whose directories are under `dir`, made.
    fn with_turn<T>(f: impl FnOnce(&Turn) -> T) -> T {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = resolved(tmp.path()).expect("resolved");
        let (own, config) = (dir.join("own"), dir.join("turn"));
        let (home, tmp_dir) = (own.join("home"), own.join("tmp"));
        for d in [&home, &tmp_dir, &config] {
            std::fs::create_dir_all(d).expect("a dir");
        }
        std::fs::write(config.join("settings.json"), "{}").expect("a file");
        let socket = dir.join("agent.sock");
        f(&Turn {
            name: "agent-0",
            turn: 1,
            message: "go",
            socket: &socket,
            own: &own,
            home: &home,
            tmp: &tmp_dir,
            config: &config,
            proxy_port: 1,
            version: "1.4.0",
        })
    }

    fn assistant(provider: &str, model: &str, content: Value) -> String {
        json!({"type": "message_end", "message": {
            "role": "assistant", "provider": provider, "model": model, "responseId": "msg_1",
            "timestamp": 7, "stopReason": "toolUse", "content": content,
            "usage": {"input": 10, "output": 3, "cacheRead": 40, "cacheWrite": 5, "totalTokens": 58},
        }})
        .to_string()
    }

    #[test]
    fn the_gate_passes_only_a_version_3_session_header_in_the_agents_home() {
        let p = pi(None);
        with_turn(|t| {
            let header = |kind: &str, version: u64, cwd: &Path| {
                json!({"type": kind, "version": version, "id": "x", "timestamp": "t", "cwd": cwd}).to_string()
            };
            let gate = |line: String| match p.event(&line, true, t) {
                Ok(Event::Init(v)) => v,
                other => panic!("not an init: {other:?}"),
            };
            assert_eq!(gate(header("session", 3, t.home)), Ok(()));
            assert_eq!(gate(header("agent_start", 3, t.home)), Err("the first event isn't the session header".into()));
            assert_eq!(gate(header("session", 4, t.home)), Err("the session header's version isn't 3".into()));
            assert_eq!(
                gate(header("session", 3, t.own)),
                Err("the session header's cwd isn't the agent's home".into())
            );
        });
    }

    #[test]
    fn an_assistant_message_is_counted_and_any_other_model_or_tool_stops_the_run() {
        let p = pi(None);
        with_turn(|t| {
            let calls = json!([
                {"type": "text", "text": "hi"},
                {"type": "toolCall", "id": "c1", "name": "mcp__experiment__notes_read", "arguments": {}},
                {"type": "toolCall", "id": "c2", "name": "mcp__experiment__claim", "arguments": {}},
            ]);
            let Ok(Event::Message { id, usage, tools, .. }) =
                p.event(&assistant("anthropic", "claude-opus-5-5", calls), false, t)
            else {
                panic!("not a message");
            };
            assert_eq!(id, "msg_1");
            assert_eq!(usage, Tokens { input: 10, cache_read: 40, cache_write: 5, output: 3 });
            let names: Vec<&str> = tools.iter().map(|(_, n)| n.as_str()).collect();
            assert_eq!(names, ["mcp__experiment__notes_read", "mcp__experiment__claim"]);

            let err = |line: String| p.event(&line, false, t).expect_err("refused");
            assert!(
                err(assistant("openai", "claude-opus-5-5", json!([])))
                    .starts_with("an assistant message from `openai`/")
            );
            assert!(
                err(assistant("anthropic", "claude-sonnet-5-5", json!([])))
                    .contains("not the plan's anthropic/claude-opus-5-5")
            );
            for name in ["bash", "mcp__other__notes_read", "mcp__experiment__rm", "__pi_deferred_placeholder__"] {
                let call = json!([{"type": "toolCall", "id": "c1", "name": name, "arguments": {}}]);
                let e = err(assistant("anthropic", "claude-opus-5-5", call));
                assert!(e.ends_with("which isn't one of the shim's tools"), "{name}: {e}");
            }
        });
    }

    #[test]
    fn a_message_without_a_response_id_is_known_by_its_timestamp_and_settling_is_a_result_of_the_streams_sum() {
        let p = pi(None);
        with_turn(|t| {
            let mut m: Value =
                serde_json::from_str(&assistant("anthropic", "claude-opus-5-5", json!([]))).expect("JSON");
            m["message"]["responseId"] = Value::Null;
            let Ok(Event::Message { id, .. }) = p.event(&m.to_string(), false, t) else { panic!("not a message") };
            assert_eq!(id, "at-7");
            let user = json!({"type": "message_end", "message": {"role": "user", "content": "x"}}).to_string();
            assert!(matches!(p.event(&user, false, t), Ok(Event::Other)));
            for (aborted, ended) in [(false, "settled"), (true, "aborted")] {
                let line = json!({"type": "agent_settled", "aborted": aborted}).to_string();
                let Ok(Event::Result { usage, ended: e, .. }) = p.event(&line, false, t) else {
                    panic!("not a result")
                };
                assert_eq!((usage, e), (None, json!(ended)));
            }
        });
    }

    #[test]
    fn the_configuration_writes_no_key_or_command_and_only_a_test_carries_a_ca() {
        let real = pi(None);
        let files = real.turn_files(Path::new("/s.sock")).expect("files");
        let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["prompt.md", "settings.json", "models.json", "mcp.json"]);
        let all: String = files.iter().map(|(_, b)| String::from_utf8_lossy(b).to_string()).collect();
        for word in ["apiKey", "headers", TOKEN, "\"!"] {
            assert!(!all.contains(word), "{word} in the configuration");
        }
        let mcp: Value = serde_json::from_slice(&files[3].1).expect("JSON");
        assert_eq!(mcp["mcpServers"]["experiment"]["exposure"], "direct");
        assert_eq!(mcp["mcpServers"]["experiment"]["command"], "/usr/bin/env");
        assert!(!real.env_names().contains(&"NODE_EXTRA_CA_CERTS"));

        let test = pi(Some(b"a CA".to_vec()));
        assert!(test.env_names().contains(&"NODE_EXTRA_CA_CERTS"));
        assert!(test.turn_files(Path::new("/s.sock")).expect("files").iter().any(|(n, _)| *n == "test-ca.pem"));

        assert_eq!(commands(&json!({"a": ["x", {"b": "!rm -rf ~"}]})), Some("!rm -rf ~".into()));
        assert_eq!(commands(&json!({"!k": "v"})), Some("!k".into()));
        assert_eq!(commands(&json!({"a": "x!"})), None);
        let mut bad = pi(None);
        bad.credential.base_url = "!curl evil".into();
        assert_eq!(
            bad.turn_files(Path::new("/s.sock")).err(),
            Some("the Pi configuration holds `!curl evil`, which Pi would run".into())
        );
    }

    #[test]
    fn compaction_is_off_one_beginning_stops_the_run_and_the_record_says_whether_a_ca_is_trusted() {
        let p = pi(None);
        let files = p.turn_files(Path::new("/s.sock")).expect("files");
        let settings: Value = serde_json::from_slice(&files[1].1).expect("JSON");
        assert_eq!(settings, json!({"compaction": {"enabled": false}}));
        with_turn(|t| {
            let line = json!({"type": "compaction_start", "reason": "overflow"}).to_string();
            let err = p.event(&line, false, t).expect_err("refused");
            assert!(err.starts_with("Pi began compacting (`overflow`), which the plan turns off"), "{err}");
        });
        assert_eq!(
            p.about(),
            json!({
                "result": "the stream's sum", "compaction": "off", "test_ca": false,
                "provider": "anthropic", "host": "llm.stub.invalid", "key_entry": "ANTHROPIC_AUTH_TOKEN",
            })
        );
        assert_eq!(pi(Some(b"a CA".to_vec())).about()["test_ca"], true);
    }

    /// A Pi auth.json in a temp dir holding `openrouter`, for [`PiCredential::openrouter`].
    fn auth(dir: &Path, openrouter: Value) -> PathBuf {
        let path = dir.join("auth.json");
        std::fs::write(&path, json!({"anthropic": {"type": "oauth"}, "openrouter": openrouter}).to_string())
            .expect("auth.json");
        path
    }

    #[test]
    fn openrouter_takes_its_key_from_a_pi_auth_file_and_goes_to_pis_own_url() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let c =
            PiCredential::openrouter(&auth(tmp.path(), json!({"type": "api_key", "key": TOKEN})), SystemTime::now())
                .expect("a credential");
        assert_eq!((c.provider(), c.host.as_str(), c.port), ("openrouter", "openrouter.ai", 443));
        assert!(!format!("{c:?}").contains(TOKEN), "its Debug never shows the key");

        let p = with_credential(c, "deepseek/deepseek-v4.1-flash", None);
        let route = p.route();
        assert_eq!((route.host.as_str(), route.port), ("openrouter.ai", 443));
        assert_eq!(p.secret(), TOKEN);
        assert!(p.env_names().contains(&"OPENROUTER_API_KEY") && !p.env_names().contains(&"ANTHROPIC_AUTH_TOKEN"));
        let files = p.turn_files(Path::new("/s.sock")).expect("files");
        let models: Value = serde_json::from_slice(&files[2].1).expect("JSON");
        assert_eq!(models, json!({"providers": {"openrouter": {"baseUrl": OPENROUTER_BASE_URL}}}));
        assert_eq!(p.about()["key_entry"], "OPENROUTER_API_KEY");
        with_turn(|t| {
            let shown = format!("{:?}", p.launch(t).expect("a launch"));
            assert!(shown.contains("openrouter/deepseek/deepseek-v4.1-flash") && shown.contains("OPENROUTER_API_KEY"));
            assert!(!shown.contains("ANTHROPIC_AUTH_TOKEN"), "{shown}");
            let line = assistant("openrouter", "deepseek/deepseek-v4.1-flash", json!([]));
            assert!(matches!(p.event(&line, false, t), Ok(Event::Message { .. })));
            let other = assistant("anthropic", "deepseek/deepseek-v4.1-flash", json!([]));
            assert!(p.event(&other, false, t).is_err(), "another provider stops the run");
        });
    }

    /// Milliseconds since the epoch at `t`, as Pi writes `expires`.
    fn ms(t: SystemTime) -> u64 {
        t.duration_since(UNIX_EPOCH).expect("after the epoch").as_millis() as u64
    }

    #[test]
    fn an_openrouter_login_gives_its_access_key_as_it_is_while_it_lasts_past_the_run() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let until = SystemTime::now() + Duration::from_secs(5400);
        // As Pi's OpenRouter login writes it: a permanent key, no refresh, and the largest safe expiry.
        let login = json!({"type": "oauth", "access": TOKEN, "refresh": "", "expires": 9007199254740991_u64});
        let c = PiCredential::openrouter(&auth(tmp.path(), login), until).expect("a credential");
        assert_eq!((c.provider(), c.key_name, c.key.as_str()), ("openrouter", "OPENROUTER_API_KEY", TOKEN));
        let p = with_credential(c, "deepseek/deepseek-v4.1-flash", None);
        assert_eq!(p.secret(), TOKEN, "the access key goes to Pi exactly as a stored key would");

        // A login lasting just past `until` serves; one ending just before it, or already over, doesn't.
        let refresh = "refresh-value-never-read-0123456789";
        let lasting = |expires: u64| json!({"type": "oauth", "access": TOKEN, "refresh": refresh, "expires": expires});
        let after = ms(until) + 1_000;
        assert!(PiCredential::openrouter(&auth(tmp.path(), lasting(after)), until).is_ok());
        for expires in [ms(until) - 1_000, ms(SystemTime::now()) - 60_000] {
            let e = PiCredential::openrouter(&auth(tmp.path(), lasting(expires)), until).expect_err("refused");
            assert!(e.contains("log in to Pi again (/login openrouter)"), "{e}");
            for value in [TOKEN, refresh, &expires.to_string()] {
                assert!(!e.contains(value), "the error quotes a value: {e}");
            }
        }
    }

    #[test]
    fn a_pi_auth_file_without_a_plain_openrouter_key_is_refused_and_never_quoted() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let now = SystemTime::now();
        let refused = |entry: Value| PiCredential::openrouter(&auth(tmp.path(), entry), now).expect_err("refused");
        let path = tmp.path().join("auth.json").display().to_string();
        let later = ms(now) + 3_600_000;
        let cases = [
            (Value::Null, "has no openrouter entry"),
            (json!({"type": "device", "key": TOKEN}), "neither an API key nor an OpenRouter login"),
            (json!({"key": TOKEN}), "neither an API key nor an OpenRouter login"),
            (json!({"type": "api_key"}), "openrouter entry has no key"),
            (json!({"type": "oauth", "expires": later}), "login holds no access key"),
            (json!({"type": "oauth", "access": "", "expires": later}), "login holds no access key"),
            (json!({"type": "oauth", "access": TOKEN}), "says nothing of when it expires"),
            (json!({"type": "oauth", "access": TOKEN, "expires": "never"}), "readable openrouter entry"),
            (json!({"type": "api_key", "key": 7}), "readable openrouter entry"),
            (json!(TOKEN), "readable openrouter entry"),
            (json!({"type": "api_key", "key": format!("!security find-generic-password {TOKEN}")}), "is a command"),
            (json!({"type": "oauth", "access": format!("!{TOKEN}"), "expires": later}), "is a command"),
            (json!({"type": "api_key", "key": "short"}), "too short"),
        ];
        // Pi expands $NAME and ${NAME}, and a transcript holds a quote or a backslash escaped.
        let odd = [
            "$OR_KEY",
            "${OR_KEY}",
            &format!("{TOKEN}$"),
            &format!("{TOKEN}\""),
            &format!("{TOKEN}\\"),
            &format!("{TOKEN} x"),
        ];
        let odd = odd.iter().map(|k| (json!({"type": "api_key", "key": k}), "holds a character outside"));
        let odd_login =
            (json!({"type": "oauth", "access": format!("${TOKEN}"), "expires": later}), "holds a character outside");
        for (entry, says) in cases.into_iter().chain(odd).chain([odd_login]) {
            let e = refused(entry.clone());
            assert!(e.contains(says), "{entry}: {e}");
            assert!(!e.contains(TOKEN) && !e.contains("OR_KEY"), "{entry}: the error quotes the key: {e}");
            assert!(says == "too short" || e.contains(&path), "{e}");
        }
        let missing = PiCredential::openrouter(&tmp.path().join("none.json"), now).expect_err("no file");
        assert!(missing.starts_with("reading "), "{missing}");
        let e = PiCredential::new("openai", OPENROUTER_BASE_URL, TOKEN.into()).expect_err("not a provider");
        assert_eq!(e, "Pi's provider `openai` isn't one a run may use");
        let e = PiCredential::new("openrouter", "http://openrouter.ai/api/v1", TOKEN.into()).expect_err("not https");
        assert_eq!(e, "openrouter's base URL is not https");
    }

    #[test]
    fn a_key_for_another_provider_than_the_plans_is_refused_before_anything_starts() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let credential =
            PiCredential::new("anthropic", "https://llm.stub.invalid", TOKEN.into()).expect("a credential");
        let cfg = PiConfig {
            prefix: tmp.path().join("no-prefix"),
            version: "1.4.0".into(),
            expect: PiExpect { provider: "openrouter".into(), model: "m".into(), thinking: "high".into() },
            caps: Caps::pilot(),
            credential,
            dir: tmp.path().join("pi"),
            shim: PathBuf::from("/bin/shim"),
            prompt: "p".into(),
            turns: 1,
            tools: vec![],
            upstream: None,
        };
        let err = PiBackend::new(cfg).err().expect("refused");
        assert_eq!(err, "the plan's provider is `openrouter`, but the key is anthropic's");
        assert!(!tmp.path().join("pi").exists(), "nothing started");
    }

    #[test]
    fn each_turn_gets_a_new_agent_directory_of_links_to_the_checked_files() {
        let p = pi(None);
        with_turn(|t| {
            let launch = p.launch(t).expect("a launch");
            let agent = resolved(&t.own.join("pi-agent")).expect("the agent dir");
            let link = std::fs::read_link(agent.join("settings.json")).expect("a link");
            assert_eq!(link, t.config.join("settings.json"));
            std::fs::write(agent.join("auth.json"), "{}").expect("Pi writes beside it");
            p.launch(t).expect("the next turn's launch");
            assert!(!agent.join("auth.json").exists(), "nothing Pi wrote outlives its turn");
            let shown = format!("{launch:?}");
            assert!(shown.contains("builtin:mcp") && shown.contains("--no-extensions") && shown.contains("--offline"));
            assert!(
                shown.contains(&format!("{:?}", agent.as_os_str())),
                "PI_CODING_AGENT_DIR is the new directory: {shown}"
            );
        });
    }
}
