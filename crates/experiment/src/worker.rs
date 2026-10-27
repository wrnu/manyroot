//! Worker processes: each holds one [`Store`] and answers the driver's
//! requests, one JSON line each way over its standard input and output.
//!
//! A worker gets only its own store's directory and the binaries it runs, a
//! cleared environment, and no path to the event log or any other store.
//! Bytes travel hex-encoded inside the JSON.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::args::{Args, MAX_NOTES, check_text};
use crate::mode::{Store, StoreConfig, mode_named};

/// The largest request or reply line a worker accepts: the hub's first export, which seeds each copy, fits easily.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Value,
    /// Hex.
    #[serde(default)]
    pub bytes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Option<String>,
    /// Hex.
    #[serde(default)]
    pub bytes: Option<String>,
}

impl Reply {
    fn from(result: Result<Value, String>) -> Reply {
        match result {
            Ok(result) => Reply { ok: true, result, ..Reply::default() },
            Err(e) => Reply { ok: false, error: Some(e), ..Reply::default() },
        }
    }
}

/// Serves requests on standard input until it closes. Called by `bracken-experiment worker`.
pub fn serve(mode: &str, config: StoreConfig) -> Result<(), String> {
    if std::env::vars_os().next().is_some() {
        return Err("a worker must start with an empty environment".into());
    }
    let mode = mode_named(mode).ok_or_else(|| format!("no mode {mode}"))?;
    let notes = config.dir.join("notes.txt");
    let mut store = mode.open(config);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut line = String::new();
    let mut input = Lines::new(stdin.lock(), MAX_LINE);
    while input.next_line(&mut line)? {
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(store.as_mut(), &notes, req),
            Err(e) => Reply::from(Err(format!("bad request: {e}"))),
        };
        let text = serde_json::to_string(&reply).map_err(|e| e.to_string())?;
        writeln!(out, "{text}").and_then(|()| out.flush()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn handle(store: &mut dyn Store, notes: &Path, req: Request) -> Reply {
    let bytes = match req.bytes.as_deref().map(hex::decode).transpose() {
        Ok(b) => b,
        Err(e) => return Reply::from(Err(format!("bad bytes: {e}"))),
    };
    match req.op.as_str() {
        "setup" => Reply::from(store.setup(bytes.as_deref())),
        "export" => match store.export() {
            Ok(b) => Reply { ok: true, bytes: Some(hex::encode(b)), ..Reply::default() },
            Err(e) => Reply::from(Err(e)),
        },
        "import" => Reply::from(bytes.ok_or_else(|| "import needs bytes".to_string()).and_then(|b| store.import(&b))),
        "sync_client" => Reply::from(socket(&req.args).and_then(|s| {
            let me = std::env::current_exe().map_err(|e| format!("finding the relay: {e}"))?;
            store.sync_client(&s, &me)
        })),
        "sync_server" => Reply::from(socket(&req.args).and_then(|s| {
            let peers: Vec<String> = serde_json::from_value(req.args["peers"].clone()).map_err(|e| e.to_string())?;
            if !peers.iter().all(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit())) {
                return Err("peers are public keys".into());
            }
            store.sync_server(&s, &peers)
        })),
        "release" => Reply::from(store.release_changes()),
        "integrated" => Reply::from(store.integrated().map(|m| json!(m))),
        "tool" => {
            let name = req.tool.unwrap_or_default();
            Reply::from(match name.as_str() {
                "notes_read" => notes_read(notes, &req.args),
                "notes_write" => notes_write(notes, &req.args),
                _ => store.tool(&name, &req.args),
            })
        }
        op => Reply::from(Err(format!("unknown op {}", crate::args::short(op)))),
    }
}

/// The socket a sync op names: the driver's, never an agent's choice.
fn socket(args: &Value) -> Result<std::path::PathBuf, String> {
    args["socket"].as_str().map(std::path::PathBuf::from).ok_or_else(|| "a sync op needs a socket".into())
}

fn notes_read(notes: &Path, args: &Value) -> Result<Value, String> {
    Args::new(args, &[])?;
    let text = match std::fs::read_to_string(notes) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("reading notes: {e}")),
    };
    Ok(json!({ "notes": text }))
}

fn notes_write(notes: &Path, args: &Value) -> Result<Value, String> {
    let text = Args::new(args, &["text"])?.text("text")?;
    if text.chars().count() > MAX_NOTES {
        return Err(format!("notes hold at most {MAX_NOTES} characters"));
    }
    check_text(text, MAX_NOTES * 4)?;
    std::fs::write(notes, text).map_err(|e| format!("writing notes: {e}"))?;
    Ok(json!({ "saved": text.chars().count() }))
}

/// Reads lines of at most `max` bytes.
pub struct Lines<R> {
    reader: R,
    max: usize,
}

impl<R: BufRead> Lines<R> {
    pub fn new(reader: R, max: usize) -> Lines<R> {
        Lines { reader, max }
    }

    /// The next line into `line`, without its newline; false at the end.
    pub fn next_line(&mut self, line: &mut String) -> Result<bool, String> {
        line.clear();
        let n = Read::take(&mut self.reader, self.max as u64 + 1).read_line(line).map_err(|e| e.to_string())?;
        if n > self.max {
            return Err(format!("a line over {} bytes", self.max));
        }
        if line.ends_with('\n') {
            line.pop();
        }
        Ok(n > 0)
    }
}

/// A running worker, from the driver's side.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Worker {
    /// Starts `bin worker` for `mode` on `config`, with an empty environment, in the store's directory.
    pub fn spawn(bin: &Path, mode: &str, config: &StoreConfig) -> Result<Worker, String> {
        std::fs::create_dir_all(&config.dir).map_err(|e| format!("creating {}: {e}", config.dir.display()))?;
        let mut child = Command::new(bin)
            .arg("worker")
            .args(["--mode", mode, "--identity", &config.identity])
            .arg("--store")
            .arg(&config.dir)
            .arg("--bracken")
            .arg(&config.bracken)
            .env_clear()
            .current_dir(&config.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting a worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("no worker stdin")?;
        let stdout = Lines::new(BufReader::new(child.stdout.take().ok_or("no worker stdout")?), MAX_LINE);
        Ok(Worker { child, stdin, stdout })
    }

    /// Sends one request and waits for its reply.
    pub fn call(&mut self, req: &Request) -> Result<Reply, String> {
        let text = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{text}").and_then(|()| self.stdin.flush()).map_err(|e| format!("worker gone: {e}"))?;
        let mut line = String::new();
        if !self.stdout.next_line(&mut line)? {
            return Err("the worker exited".into());
        }
        serde_json::from_str(&line).map_err(|e| format!("bad reply from a worker: {e}"))
    }

    /// The same, turning a failed reply into an error.
    pub fn ok(&mut self, req: &Request) -> Result<Reply, String> {
        let reply = self.call(req)?;
        if reply.ok { Ok(reply) } else { Err(reply.error.unwrap_or_else(|| "the worker failed".into())) }
    }

    pub fn bytes(reply: &Reply) -> Result<Vec<u8>, String> {
        hex::decode(reply.bytes.as_deref().unwrap_or_default()).map_err(|e| e.to_string())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
