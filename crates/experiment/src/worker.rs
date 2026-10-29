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
use crate::mode::{Session, Store, StoreConfig, Transfer, mode_under};
use crate::sandbox::Sandbox;

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
    /// For the event log only: what [`Store::record`] says about a tool call.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub record: Value,
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
    let mode = mode_under(mode, config.accept_rule).ok_or_else(|| format!("no mode {mode}"))?;
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
        "sync_server" => Reply::from(socket(&req.args).and_then(|s| store.sync_server(&s, &session(&req.args)?))),
        "push" => match store.push() {
            Ok(t) => Reply { ok: true, result: t.meta, bytes: Some(hex::encode(t.bytes)), ..Reply::default() },
            Err(e) => Reply::from(Err(e)),
        },
        "receive" => Reply::from(session(&req.args).and_then(|s| {
            store.receive(&s, Transfer { meta: req.args["meta"].clone(), bytes: bytes.unwrap_or_default() })
        })),
        "send" => match session(&req.args).and_then(|s| store.send(&s, &req.args["have"])) {
            Ok(t) => Reply { ok: true, result: t.meta, bytes: Some(hex::encode(t.bytes)), ..Reply::default() },
            Err(e) => Reply::from(Err(e)),
        },
        "fetch" => {
            Reply::from(store.fetch(Transfer { meta: req.args["meta"].clone(), bytes: bytes.unwrap_or_default() }))
        }
        "validate" => Reply::from(store.validate(&req.tool.unwrap_or_default(), &req.args)),
        "hub_tool" => {
            let name = req.tool.unwrap_or_default();
            let args = &req.args["args"];
            let result = session(&req.args).and_then(|s| store.hub_tool(&s, &name, args));
            recorded(store, &name, args, result)
        }
        "release" => Reply::from(store.release_changes()),
        "integrated" => Reply::from(store.integrated().map(|m| json!(m))),
        "tool" => {
            let name = req.tool.unwrap_or_default();
            match name.as_str() {
                "notes_read" => Reply::from(notes_read(notes, &req.args)),
                "notes_write" => Reply::from(notes_write(notes, &req.args)),
                _ => {
                    let result = store.tool(&name, &req.args);
                    recorded(store, &name, &req.args, result)
                }
            }
        }
        op => Reply::from(Err(format!("unknown op {}", crate::args::short(op)))),
    }
}

/// A tool's reply with the store's record of it. A record that fails is an error for the driver, never the agent.
fn recorded(store: &mut dyn Store, tool: &str, args: &Value, result: Result<Value, String>) -> Reply {
    match result {
        Ok(v) => {
            let record = store.record(tool, args, &v).unwrap_or_else(|e| json!({"error": e}));
            Reply { ok: true, result: v, record, ..Reply::default() }
        }
        Err(e) => Reply::from(Err(e)),
    }
}

/// The session a hub op is for, from the driver.
fn session(args: &Value) -> Result<Session, String> {
    let session: Session = serde_json::from_value(args["session"].clone()).map_err(|e| e.to_string())?;
    if !session.peers.iter().all(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("peers are public keys".into());
    }
    Ok(session)
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
    /// Set once a reply couldn't be read whole: the worker is killed, and every later call fails.
    failed: bool,
}

impl Worker {
    /// Starts `bin worker` for `mode` on `config`, with an empty environment, in the store's directory.
    pub fn spawn(bin: &Path, mode: &str, config: &StoreConfig) -> Result<Worker, String> {
        Worker::spawn_in(bin, mode, config, None)
    }

    /// The same, under `sandbox` if given (the tools or hub profile).
    pub fn spawn_in(bin: &Path, mode: &str, config: &StoreConfig, sandbox: Option<&Sandbox>) -> Result<Worker, String> {
        std::fs::create_dir_all(&config.dir).map_err(|e| format!("creating {}: {e}", config.dir.display()))?;
        let mut command = match sandbox {
            Some(s) => s.command(bin),
            None => Command::new(bin),
        };
        let child = command
            .arg("worker")
            .args(["--mode", mode, "--identity", &config.identity])
            .arg("--store")
            .arg(&config.dir)
            .arg("--bracken")
            .arg(&config.bracken)
            .arg("--git")
            .arg(&config.git)
            .args(["--task", config.task.name()])
            .args(config.trace.then_some("--trace"))
            .args(["--accept-rule", config.accept_rule.name()])
            .env_clear()
            .current_dir(&config.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting a worker: {e}"))?;
        Worker::from_child(child, MAX_LINE)
    }

    /// A worker over an already started `child`, whose reply lines may be at most `max` bytes.
    pub(crate) fn from_child(mut child: Child, max: usize) -> Result<Worker, String> {
        let stdin = child.stdin.take().ok_or("no worker stdin")?;
        let stdout = Lines::new(BufReader::new(child.stdout.take().ok_or("no worker stdout")?), max);
        Ok(Worker { child, stdin, stdout, failed: false })
    }

    /// Sends one request and waits for its reply. A reply that can't be read whole (over the line
    /// limit, cut, not UTF-8) leaves the rest of it in the pipe, so the worker is killed and
    /// fails every later call rather than answer one call with another's reply (finding `7d119d53922f`).
    pub fn call(&mut self, req: &Request) -> Result<Reply, String> {
        if self.failed {
            return Err("the worker failed earlier and was stopped".into());
        }
        let text = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{text}").and_then(|()| self.stdin.flush()).map_err(|e| format!("worker gone: {e}"))?;
        let mut line = String::new();
        let read = self.stdout.next_line(&mut line);
        let reply = match read {
            Ok(true) => serde_json::from_str(&line).map_err(|e| format!("bad reply from a worker: {e}")),
            Ok(false) => Err("the worker exited".into()),
            Err(e) => Err(e),
        };
        if reply.is_err() {
            self.failed = true;
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        reply
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_over_the_line_limit_stops_the_worker_for_good() {
        // `cat` answers each request with the request itself: a 16-byte limit is passed at once.
        let child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("cat");
        let mut w = Worker::from_child(child, 16).expect("a worker");
        let req = Request { op: "a request longer than sixteen bytes".into(), ..Request::default() };
        assert!(w.call(&req).is_err_and(|e| e.contains("a line over")));
        // The rest of that reply is still in the pipe: no later call may read it as its own.
        let short = Request { op: "x".into(), ..Request::default() };
        assert!(w.call(&short).is_err_and(|e| e.contains("stopped")));
        assert!(w.child.try_wait().expect("waited").is_some(), "the child was killed");
    }
}
