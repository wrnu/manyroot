//! The driver: sets up a run, runs every agent's turns at once, relays each
//! tool call from the agent's socket to its tool worker, carries sync data
//! between workers, and alone writes the event log (design section 3).
//!
//! The run directory holds `hub/` and one `agents/<name>/` store per agent,
//! each written only by its own worker; `sockets/`, where each turn's
//! connection and each sync session's two ends arrive; and `events.jsonl`,
//! written only here.
//!
//! One agent's failure (a sync, a broken connection, its backend's error) is
//! logged and ends only that agent's turn; the run goes on.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::mode::{StoreConfig, ToolSpec, mode_named};
use crate::worker::{Lines, Request, Worker};

/// Turns used, as a share of all turns, before the change requests are released (design section 1).
pub const RELEASE_AT: f64 = 0.4;
/// The longest MCP message the driver reads from an agent.
pub const MAX_MCP_LINE: usize = 256 * 1024;
/// How long the driver waits for both ends of a sync session to connect.
const SYNC_CONNECT: Duration = Duration::from_secs(30);

pub struct RunConfig {
    /// Must not exist yet.
    pub root: PathBuf,
    pub mode: &'static str,
    pub agents: usize,
    pub turns: u32,
    /// The `bracken-experiment` binary, for workers.
    pub worker_bin: PathBuf,
    pub bracken: PathBuf,
}

/// What plays an agent's turn: the mock backend now, Claude Code from step 6.
pub trait Backend: Sync {
    /// Plays one turn, talking MCP to the driver over `socket`.
    fn turn(&self, agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String>;
}

pub struct Outcome {
    /// The hub's integrated library at the end.
    pub modules: Vec<(String, String)>,
    pub events: PathBuf,
    /// The number of turns used when the change requests went out.
    pub released_after: Option<u32>,
}

/// The run's record. Only the driver holds it.
pub struct EventLog {
    file: Mutex<File>,
    start: Instant,
    seq: AtomicU64,
    /// A record failed to write; the run fails at the end rather than report from a gap.
    failed: AtomicBool,
}

impl EventLog {
    fn create(path: &Path) -> Result<EventLog, String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
        Ok(EventLog {
            file: Mutex::new(file),
            start: Instant::now(),
            seq: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        })
    }

    pub fn log(&self, agent: Option<&str>, event: &str, data: Value) {
        // The sequence number and time are taken under the lock, so the file is in their order.
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let line = json!({
            "seq": self.seq.fetch_add(1, Ordering::SeqCst),
            "ms": self.start.elapsed().as_millis() as u64,
            "agent": agent,
            "event": event,
            "data": data,
        });
        if writeln!(file, "{line}").is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..16].to_string()
}

fn make_dir(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("creating {}: {e}", path.display()))
}

struct Run<'a> {
    cfg: &'a RunConfig,
    events: EventLog,
    hub: Mutex<Worker>,
    copies: Vec<Mutex<Worker>>,
    names: Vec<String>,
    /// Each copy's public key: the contributions the hub keeps.
    peers: Vec<String>,
    sockets: PathBuf,
    tools: Vec<ToolSpec>,
    turns_done: AtomicU32,
    released_after: Mutex<Option<u32>>,
}

/// Runs a whole mode with `backend` playing every agent.
pub fn run(cfg: &RunConfig, backend: &dyn Backend) -> Result<Outcome, String> {
    let mode = mode_named(cfg.mode).ok_or_else(|| format!("no mode {}", cfg.mode))?;
    if cfg.root.exists() {
        return Err(format!("{} already exists", cfg.root.display()));
    }
    make_dir(&cfg.root)?;
    let root = std::fs::canonicalize(&cfg.root).map_err(|e| e.to_string())?;
    for sub in ["agents", "sockets"] {
        make_dir(&root.join(sub))?;
    }
    let events = EventLog::create(&root.join("events.jsonl"))?;
    let store =
        |dir: PathBuf, identity: &str| StoreConfig { dir, bracken: cfg.bracken.clone(), identity: identity.into() };

    let mut hub = Worker::spawn(&cfg.worker_bin, mode.name(), &store(root.join("hub"), "setter"))?;
    hub.ok(&Request { op: "setup".into(), ..Request::default() })?;
    let seed = Worker::bytes(&hub.ok(&Request { op: "export".into(), ..Request::default() })?)?;
    let names: Vec<String> = (0..cfg.agents).map(|i| format!("agent-{i}")).collect();
    let (mut copies, mut peers) = (Vec::new(), Vec::new());
    for name in &names {
        let mut w = Worker::spawn(&cfg.worker_bin, mode.name(), &store(root.join("agents").join(name), name))?;
        let reply = w.ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })?;
        peers.push(reply.result["key"].as_str().ok_or("a copy's setup gave no key")?.to_string());
        copies.push(Mutex::new(w));
    }
    events.log(None, "setup", json!({"mode": mode.name(), "agents": cfg.agents, "turns": cfg.turns}));

    let mut tools = mode.tools();
    tools.extend(core_tools());
    let run = Run {
        cfg,
        events,
        hub: Mutex::new(hub),
        copies,
        names,
        peers,
        sockets: root.join("sockets"),
        tools,
        turns_done: AtomicU32::new(0),
        released_after: Mutex::new(None),
    };
    let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..cfg.agents)
            .map(|i| {
                let (run, root) = (&run, &root);
                scope.spawn(move || run.agent(i, root, backend))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("an agent thread panicked".into()))).collect()
    });
    for r in results {
        r?;
    }
    let reply = run.hub().ok(&Request { op: "integrated".into(), ..Request::default() })?;
    let modules: Vec<(String, String)> = serde_json::from_value(reply.result).map_err(|e| e.to_string())?;
    run.events.log(None, "integrated", json!({"modules": modules.iter().map(|(m, t)| json!({"module": m, "bytes": t.len(), "digest": digest(t.as_bytes())})).collect::<Vec<_>>()}));
    if run.events.failed.load(Ordering::SeqCst) {
        return Err("writing the event log failed".into());
    }
    let released_after = *run.released_after.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Outcome { modules, events: root.join("events.jsonl"), released_after })
}

/// Tools every mode has, which the core serves.
fn core_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "sync",
            description: "Exchange what's new with the hub, both ways",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: "notes_read",
            description: "Read your private notes, kept between turns",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: "notes_write",
            description: "Replace your private notes (at most 4000 characters)",
            input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"], "additionalProperties": false}),
        },
    ]
}

impl Run<'_> {
    fn hub(&self) -> std::sync::MutexGuard<'_, Worker> {
        self.hub.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn copy(&self, i: usize) -> std::sync::MutexGuard<'_, Worker> {
        self.copies[i].lock().unwrap_or_else(|e| e.into_inner())
    }

    fn agent(&self, i: usize, root: &Path, backend: &dyn Backend) -> Result<(), String> {
        let name = &self.names[i];
        let socket = root.join("sockets").join(format!("a{i}.sock"));
        for turn in 1..=self.cfg.turns {
            self.events.log(Some(name), "turn_start", json!({"turn": turn}));
            let played = self.sync(i).and_then(|_| self.serve_turn(i, turn, &socket, backend));
            let ended = self.sync(i);
            for (step, r) in [("play", played.err()), ("sync", ended.err())] {
                if let Some(e) = r {
                    // Sizes and digests only: an error can quote what the agent sent.
                    let data = json!({"turn": turn, "step": step, "bytes": e.len(), "digest": digest(e.as_bytes())});
                    self.events.log(Some(name), "turn_error", data);
                }
            }
            self.events.log(Some(name), "turn_end", json!({"turn": turn}));
            let done = self.turns_done.fetch_add(1, Ordering::SeqCst) + 1;
            self.maybe_release(done)?;
        }
        Ok(())
    }

    fn maybe_release(&self, done: u32) -> Result<(), String> {
        let total = self.cfg.turns * self.cfg.agents as u32;
        let mut released = self.released_after.lock().unwrap_or_else(|e| e.into_inner());
        if released.is_none() && f64::from(done) >= RELEASE_AT * f64::from(total) {
            let reply = self.hub().ok(&Request { op: "release".into(), ..Request::default() })?;
            self.events.log(None, "changes_released", json!({"turns_done": done, "result": reply.result}));
            *released = Some(done);
        }
        Ok(())
    }

    /// One sync session between copy `i` and the hub, both ways, its bytes
    /// relayed through the driver: only what each side is missing moves.
    fn sync(&self, i: usize) -> Result<Value, String> {
        let mut hub = self.hub();
        let mut copy = self.copy(i);
        let end = |side: &str| -> Result<(PathBuf, UnixListener), String> {
            let path = self.sockets.join(format!("{side}{i}.sock"));
            let _ = std::fs::remove_file(&path);
            let l = UnixListener::bind(&path).map_err(|e| format!("binding {}: {e}", path.display()))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
            l.set_nonblocking(true).map_err(|e| e.to_string())?;
            Ok((path, l))
        };
        let (copy_path, copy_end) = end("c")?;
        let (hub_path, hub_end) = end("h")?;
        let session = |op: &str, path: &Path| Request {
            op: op.into(),
            args: json!({"socket": path, "peers": self.peers}),
            ..Request::default()
        };
        let (server, client) = (session("sync_server", &hub_path), session("sync_client", &copy_path));
        let (hub, copy, server, client) = (&mut *hub, &mut *copy, &server, &client);
        let result = std::thread::scope(|scope| {
            let served = scope.spawn(move || hub.ok(server));
            let synced = scope.spawn(move || copy.ok(client));
            // The listeners go with the relay, so a side left waiting sees its connection close.
            let relayed = relay_session(copy_end, hub_end, || served.is_finished() || synced.is_finished());
            let served = served.join().unwrap_or_else(|_| Err("the hub's session panicked".into()));
            let synced = synced.join().unwrap_or_else(|_| Err("the copy's session panicked".into()));
            served.and(synced).and(relayed)
        });
        let _ = std::fs::remove_file(&copy_path);
        let _ = std::fs::remove_file(&hub_path);
        let ((up, up_digest), (down, down_digest)) = result?;
        let summary = json!({"up": {"bytes": up, "digest": up_digest}, "down": {"bytes": down, "digest": down_digest}});
        self.events.log(Some(&self.names[i]), "sync", summary.clone());
        Ok(summary)
    }

    /// Opens the agent's socket, lets the backend play its turn on it, and serves
    /// that one connection until it closes.
    fn serve_turn(&self, i: usize, turn: u32, socket: &Path, backend: &dyn Backend) -> Result<(), String> {
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket).map_err(|e| format!("binding {}: {e}", socket.display()))?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let name = self.names[i].clone();
        let result = std::thread::scope(|scope| {
            let player = scope.spawn(|| backend.turn(i, &name, turn, socket));
            let served = loop {
                match listener.accept() {
                    Ok((stream, _)) => break self.serve(i, turn, stream),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if player.is_finished() {
                            break Ok(());
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => break Err(format!("accepting on {}: {e}", socket.display())),
                }
            };
            let played = player.join().unwrap_or_else(|_| Err("the backend panicked".into()));
            served.and(played)
        });
        let _ = std::fs::remove_file(socket);
        result
    }

    /// MCP over newline-delimited JSON-RPC, for one connection.
    fn serve(&self, i: usize, turn: u32, stream: UnixStream) -> Result<(), String> {
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut out = stream.try_clone().map_err(|e| e.to_string())?;
        let mut lines = Lines::new(BufReader::new(stream), MAX_MCP_LINE);
        let mut line = String::new();
        let name = &self.names[i];
        while lines.next_line(&mut line)? {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                self.events.log(Some(name), "bad_message", json!({"turn": turn, "bytes": line.len()}));
                continue;
            };
            let Some(id) = msg.get("id").cloned() else { continue };
            let reply = match msg["method"].as_str().unwrap_or_default() {
                "initialize" => json!({"result": {
                    "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "experiment", "version": env!("CARGO_PKG_VERSION")},
                }}),
                "tools/list" => json!({"result": {"tools": self.tools}}),
                "tools/call" => json!({"result": self.call(i, turn, &msg["params"])}),
                "ping" => json!({"result": {}}),
                _ => json!({"error": {"code": -32601, "message": "method not found"}}),
            };
            let mut reply = reply;
            reply["jsonrpc"] = json!("2.0");
            reply["id"] = id;
            writeln!(out, "{reply}").and_then(|()| out.flush()).map_err(|e| format!("replying: {e}"))?;
        }
        Ok(())
    }

    fn call(&self, i: usize, turn: u32, params: &Value) -> Value {
        let tool = params["name"].as_str().unwrap_or_default().to_string();
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let shown = crate::args::short(&tool);
        let args_text = args.to_string();
        let name = &self.names[i];
        self.events.log(
            Some(name),
            "tool_call",
            json!({"turn": turn, "tool": shown, "args_bytes": args_text.len(), "args_digest": digest(args_text.as_bytes())}),
        );
        let result = if tool == "sync" {
            crate::args::Args::new(&args, &[]).and_then(|_| self.sync(i))
        } else {
            self.copy(i)
                .ok(&Request { op: "tool".into(), tool: Some(tool), args, ..Request::default() })
                .map(|r| r.result)
        };
        let (text, is_error) = match &result {
            Ok(v) => (v.to_string(), false),
            Err(e) => (e.clone(), true),
        };
        self.events.log(
            Some(name),
            "tool_result",
            json!({"turn": turn, "tool": shown, "ok": !is_error, "bytes": text.len(), "digest": digest(text.as_bytes())}),
        );
        json!({"content": [{"type": "text", "text": text}], "isError": is_error})
    }
}

/// Bytes relayed one way, and their digest.
type Relayed = (u64, String);

/// Accepts the copy's end and the hub's end of one session and relays bytes
/// between them until both sides close; gives up if `gone()` (a side ended
/// first) or neither connects in time.
fn relay_session(
    copy_end: UnixListener,
    hub_end: UnixListener,
    gone: impl Fn() -> bool,
) -> Result<(Relayed, Relayed), String> {
    let deadline = Instant::now() + SYNC_CONNECT;
    let (mut copy, mut hub) = (None, None);
    while copy.is_none() || hub.is_none() {
        for (listener, slot) in [(&copy_end, &mut copy), (&hub_end, &mut hub)] {
            if slot.is_none() {
                match listener.accept() {
                    Ok((s, _)) => *slot = Some(s),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(format!("accepting a sync session: {e}")),
                }
            }
        }
        if copy.is_none() || hub.is_none() {
            if gone() || Instant::now() > deadline {
                return Err("a sync session's side didn't connect".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let (copy, hub) = (copy.expect("accepted above"), hub.expect("accepted above"));
    let one_way = |from: &UnixStream, to: &UnixStream| -> Result<Relayed, String> {
        from.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut hashing = Hashing { to, hasher: blake3::Hasher::new() };
        let n = crate::shim::relay(&mut &*from, &mut hashing);
        let _ = to.shutdown(std::net::Shutdown::Write);
        let n = n?;
        Ok((n, hashing.hasher.finalize().to_hex()[..16].to_string()))
    };
    std::thread::scope(|scope| {
        let up = scope.spawn(|| one_way(&copy, &hub));
        let down = one_way(&hub, &copy);
        let up = up.join().unwrap_or_else(|_| Err("the relay panicked".into()));
        Ok((up?, down?))
    })
}

/// Writes through to a stream, hashing what passes.
struct Hashing<'a> {
    to: &'a UnixStream,
    hasher: blake3::Hasher,
}

impl Write for Hashing<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = (&mut &*self.to).write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&mut &*self.to).flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_log_is_written_in_sequence_order() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("events.jsonl");
        let log = EventLog::create(&path).expect("the log");
        std::thread::scope(|scope| {
            for t in 0..8 {
                let log = &log;
                scope.spawn(move || {
                    for n in 0..500 {
                        log.log(Some("agent"), "e", json!({"t": t, "n": n}));
                    }
                });
            }
        });
        let text = std::fs::read_to_string(&path).expect("written");
        let rows: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect();
        assert_eq!(rows.len(), 4000);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r["seq"], i as u64, "line {i} is out of order");
        }
        assert!(rows.windows(2).all(|w| w[0]["ms"].as_u64() <= w[1]["ms"].as_u64()));
    }
}
