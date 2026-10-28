//! The interface between the core and a mode (design sections 2 and 3).
//!
//! The core runs agents, relays their tool calls, syncs copies with the hub
//! and logs everything. A mode supplies the tools agents see and a [`Store`]:
//! one agent's working copy, or the mode's hub. A store runs inside a worker
//! process and touches only its own directory. Data between stores moves as
//! bytes the driver carries: the hub's [`Store::export`] seeds each copy, and
//! each sync moves only what the other side is missing, as the mode's
//! [`SyncKind`] says: one session ([`Store::sync_client`] on the copy,
//! [`Store::sync_server`] on the hub, relayed through the driver's sockets), or
//! four transfers ([`Store::push`], [`Store::receive`], [`Store::send`],
//! [`Store::fetch`]), each validated by its receiver.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool as agents see it in `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// What a store needs to start: its own directory and the binaries it may run.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// The store's own directory: the only place it reads or writes.
    pub dir: PathBuf,
    /// The `bracken` binary, by absolute path.
    pub bracken: PathBuf,
    /// The `git` binary, by absolute path (Git mode).
    pub git: PathBuf,
    /// The identity it signs as: an agent's name, or `setter` for the hub.
    pub identity: String,
    /// Tests only: record every command git starts in the store's `git-trace.log`.
    pub trace: bool,
}

/// One agent's working copy, or the mode's hub.
pub trait Store {
    /// Creates the hub (with the task stated in it), or, given the hub's export, an agent's copy.
    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String>;
    /// Everything another store may need, as bytes.
    fn export(&mut self) -> Result<Vec<u8>, String>;
    /// Applies another store's export. It must validate what it receives.
    fn import(&mut self, bytes: &[u8]) -> Result<Value, String>;
    /// A copy: syncs with the hub over `socket`, reached through `relay shim --socket` (the
    /// `bracken-experiment` binary), so the session's bytes pass through the driver.
    fn sync_client(&mut self, _socket: &Path, _relay: &Path) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// The hub: serves one sync session on `socket` for `session.agent`, within its quotas.
    fn sync_server(&mut self, _socket: &Path, _session: &Session) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// A copy, in a mode that syncs by [`Transfer`]s: what it sends the hub (a sync's first half).
    fn push(&mut self) -> Result<Transfer, String> {
        Err(not_this_mode())
    }
    /// The hub: takes one agent's push. It must validate what it receives.
    fn receive(&mut self, _session: &Session, _push: Transfer) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// The hub: what it sends the agent back, given what the agent's push said it has.
    fn send(&mut self, _session: &Session, _have: &Value) -> Result<Transfer, String> {
        Err(not_this_mode())
    }
    /// A copy: takes the hub's answer. It must validate what it receives.
    fn fetch(&mut self, _answer: Transfer) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// A copy: checks a call to one of the mode's hub tools and returns its arguments as checked,
    /// so agent values are parsed in the agent's own worker first.
    fn validate(&mut self, _name: &str, _args: &Value) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// The hub: runs one of the mode's hub tools for `session.name`, with arguments its copy checked.
    fn hub_tool(&mut self, _session: &Session, _name: &str, _args: &Value) -> Result<Value, String> {
        Err(not_this_mode())
    }
    /// Runs one of the mode's tools for the agent that owns this copy.
    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String>;
    /// The hub only: states the change requests, as the mode states them.
    fn release_changes(&mut self) -> Result<Value, String>;
    /// The hub only: the integrated library, as `(module, text)` pairs.
    fn integrated(&mut self) -> Result<Vec<(String, String)>, String>;
    /// What the measures need to know about a tool call that just succeeded, for the event log only
    /// (never the agent): `writes`, the `[function, definition]` pairs it created or changed;
    /// `blocks`, the definitions a verdict against them stops; `conflict`, a merge that stopped.
    fn record(&mut self, _tool: &str, _args: &Value, _result: &Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

fn not_this_mode() -> String {
    "not in this mode".into()
}

/// One half of a sync in a mode that syncs by transfers: what the sender says, and its bytes.
#[derive(Debug, Clone, Default)]
pub struct Transfer {
    pub meta: Value,
    pub bytes: Vec<u8>,
}

/// How a mode's copies sync with its hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    /// One session both ways ([`Store::sync_client`], [`Store::sync_server`]).
    Session,
    /// Push, receive, send, fetch ([`Store::push`] and the rest).
    Transfers,
}

/// What the hub may take in one agent's sync, and whose it is.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    /// The agent's index: each agent's intake is counted on its own.
    pub agent: usize,
    /// The agent's name, set by the harness.
    pub name: String,
    /// Every copy's public key: the hub keeps contributions only by these.
    pub peers: Vec<String>,
    /// The most contributions, and bytes of them, this agent may add over the run.
    pub quota_contributions: u64,
    pub quota_bytes: u64,
    /// A sync session's deadline in seconds; 0 means the mode's own (tests shorten it).
    #[serde(default)]
    pub sync_seconds: u32,
}

/// A mode, as the driver and the workers know it.
pub trait Mode: Sync {
    fn name(&self) -> &'static str;
    /// The mode's tools, without the core's own (`sync`, `notes_read`, `notes_write`).
    fn tools(&self) -> Vec<ToolSpec>;
    /// Which of [`Mode::tools`] the hub serves ([`Store::hub_tool`]) rather than the agent's copy.
    fn hub_tools(&self) -> &'static [&'static str] {
        &[]
    }
    fn sync_kind(&self) -> SyncKind {
        SyncKind::Session
    }
    fn open(&self, config: StoreConfig) -> Box<dyn Store>;
}

/// The mode named `name`.
pub fn mode_named(name: &str) -> Option<&'static dyn Mode> {
    match name {
        "bracken" => Some(&crate::bracken_mode::BrackenMode),
        "crdt" => Some(&crate::crdt_mode::CrdtMode),
        "git" => Some(&crate::git_mode::GitMode),
        _ => None,
    }
}

/// Waits for `child`, feeding it `stdin` from another thread while its output is read, so a
/// child that writes as it reads (`git cat-file --batch`) can never fill both pipes and wait
/// on its parent for ever (finding `0521992b7de5`).
pub fn communicate(mut child: std::process::Child, stdin: Option<&[u8]>) -> std::io::Result<std::process::Output> {
    let pipe = child.stdin.take();
    std::thread::scope(|scope| {
        let feeder = scope.spawn(move || {
            if let (Some(bytes), Some(mut pipe)) = (stdin, pipe) {
                use std::io::Write;
                // A child that exits without reading everything is not an error here; its status says.
                let _ = pipe.write_all(bytes);
            }
        });
        let out = child.wait_with_output();
        let _ = feeder.join();
        out
    })
}

/// Whether `path` is inside `dir`, after resolving both: a store's guard on the files it writes.
pub fn is_within(path: &Path, dir: &Path) -> bool {
    match (std::fs::canonicalize(path.parent().unwrap_or(path)), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}
