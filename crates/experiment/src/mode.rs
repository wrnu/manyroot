//! The interface between the core and a mode (design sections 2 and 3).
//!
//! The core runs agents, relays their tool calls, syncs copies with the hub
//! and logs everything. A mode supplies the tools agents see and a [`Store`]:
//! one agent's working copy, or the mode's hub. A store runs inside a worker
//! process and touches only its own directory. Data between stores moves as
//! bytes the driver carries: the hub's [`Store::export`] seeds each copy, and
//! each sync is one session, [`Store::sync_client`] on the copy and
//! [`Store::sync_server`] on the hub, relayed through the driver's sockets, in
//! which only what the other side is missing moves.

use std::path::{Path, PathBuf};

use serde::Serialize;
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
    /// The identity it signs as: an agent's name, or `setter` for the hub.
    pub identity: String,
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
    fn sync_client(&mut self, socket: &Path, relay: &Path) -> Result<Value, String>;
    /// The hub: serves one sync session on `socket`, keeping contributions only by `peers` (public keys).
    fn sync_server(&mut self, socket: &Path, peers: &[String]) -> Result<Value, String>;
    /// Runs one of the mode's tools for the agent that owns this copy.
    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String>;
    /// The hub only: states the change requests, as the mode states them.
    fn release_changes(&mut self) -> Result<Value, String>;
    /// The hub only: the integrated library, as `(module, text)` pairs.
    fn integrated(&mut self) -> Result<Vec<(String, String)>, String>;
}

/// A mode, as the driver and the workers know it.
pub trait Mode: Sync {
    fn name(&self) -> &'static str;
    /// The mode's tools, without the core's own (`sync`, `notes_read`, `notes_write`).
    fn tools(&self) -> Vec<ToolSpec>;
    fn open(&self, config: StoreConfig) -> Box<dyn Store>;
}

/// The mode named `name`.
pub fn mode_named(name: &str) -> Option<&'static dyn Mode> {
    match name {
        "bracken" => Some(&crate::bracken_mode::BrackenMode),
        _ => None,
    }
}

/// Whether `path` is inside `dir`, after resolving both: a store's guard on the files it writes.
pub fn is_within(path: &Path, dir: &Path) -> bool {
    match (std::fs::canonicalize(path.parent().unwrap_or(path)), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}
