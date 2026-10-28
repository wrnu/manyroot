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

use crate::mode::{Mode, Session, StoreConfig, SyncKind, ToolSpec, mode_named};
use crate::sandbox::{Git, Sandbox, WorkerPaths};
use crate::worker::{Lines, Request, Worker};

/// Turns used, as a share of all turns, before the change requests are released (design section 1).
pub const RELEASE_AT: f64 = 0.4;
/// Tool calls an agent may make in one turn (design section 5); the driver refuses the next.
pub const CALLS_PER_TURN: u32 = 25;
/// The most contributions one tool call can add in any mode: a Bracken claim superseding 8 of its own adds 9.
pub const CONTRIBUTIONS_PER_CALL: u64 = 9;
/// The most bytes one contribution can hold: the largest free text and its envelope.
pub const BYTES_PER_CONTRIBUTION: u64 = crate::args::MAX_TEXT as u64 + 4096;
/// The longest MCP message the driver reads from an agent.
pub const MAX_MCP_LINE: usize = 256 * 1024;
/// How long the driver waits for both ends of a sync session to connect.
const SYNC_CONNECT: Duration = Duration::from_secs(30);
/// The longest a sync session's relay runs: past bracken's own 60 s session deadline.
const SYNC_RELAY: Duration = Duration::from_secs(75);
/// The longest one turn may take (design section 5): the driver stops serving it then.
pub const TURN_LIMIT: Duration = Duration::from_secs(600);

pub struct RunConfig {
    /// Must not exist yet.
    pub root: PathBuf,
    pub mode: &'static str,
    pub agents: usize,
    pub turns: u32,
    /// The `bracken-experiment` binary, for workers.
    pub worker_bin: PathBuf,
    pub bracken: PathBuf,
    /// Run each worker under its `sandbox-exec` profile: the tools profile for a copy, the hub profile for the hub.
    pub sandbox: bool,
    /// The real `git` binary, by absolute path (see [`git_binary`]).
    pub git: PathBuf,
    /// Tests only: workers record every command git starts.
    pub trace: bool,
    /// Tool calls an agent may make per turn: [`CALLS_PER_TURN`] in a real run.
    pub calls_per_turn: u32,
    /// The longest a turn may take: [`TURN_LIMIT`] in a real run.
    pub turn_limit: Duration,
}

/// The real `git` binary: the one in the exec path of the `git` on `PATH`, so
/// a launcher such as macOS's `/usr/bin/git` (which starts `xcrun`) is never run by a worker.
pub fn git_binary() -> Result<PathBuf, String> {
    let out = std::process::Command::new("git").arg("--exec-path").output().map_err(|e| format!("finding git: {e}"))?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::canonicalize(Path::new(&dir).join("git")).map_err(|e| format!("no git in {dir}: {e}"))
}

impl RunConfig {
    /// What one agent's sync sessions may add to the hub over the whole run: more than any agent within
    /// the caps can make, so only a run past them reaches it, and then only for that agent.
    pub fn quota(&self) -> (u64, u64) {
        let contributions = u64::from(self.turns) * u64::from(self.calls_per_turn) * CONTRIBUTIONS_PER_CALL;
        (contributions, contributions * BYTES_PER_CONTRIBUTION)
    }
}

/// What plays an agent's turn: the mock backend now, Claude Code from step 6.
pub trait Backend: Sync {
    /// Plays one turn, talking MCP to the driver over `socket`.
    fn turn(&self, agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String>;

    /// Why the whole run must stop, once it must (a failed gate, a cap breach): no agent starts another turn.
    fn halted(&self) -> Option<String> {
        None
    }
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
    mode: &'static dyn Mode,
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
    let store = |dir: PathBuf, identity: &str| StoreConfig {
        dir,
        bracken: cfg.bracken.clone(),
        git: cfg.git.clone(),
        identity: identity.into(),
        trace: cfg.trace,
    };

    let sockets = root.join("sockets");
    // Only Git mode's workers may run git: its binary and the helpers in its exec path.
    let git = match (mode.name(), cfg.git.parent()) {
        ("git", Some(exec_path)) => Some(Git { binary: cfg.git.clone(), exec_path: exec_path.to_path_buf() }),
        ("git", None) => return Err("git's binary has no directory".into()),
        _ => None,
    };
    let profile = |dir: &Path, sockets: &[PathBuf], hub: bool| -> Result<Option<Sandbox>, String> {
        if !cfg.sandbox {
            return Ok(None);
        }
        make_dir(dir)?;
        let paths =
            WorkerPaths { store: dir, worker: &cfg.worker_bin, bracken: &cfg.bracken, git: git.as_ref(), sockets };
        if hub { Sandbox::hub(&paths).map(Some) } else { Sandbox::tools(&paths).map(Some) }
    };
    let hub_sockets: Vec<PathBuf> = (0..cfg.agents).map(|i| sockets.join(format!("h{i}.sock"))).collect();
    let hub_profile = profile(&root.join("hub"), &hub_sockets, true)?;
    let hub_store = store(root.join("hub"), "setter");
    let mut hub = Worker::spawn_in(&cfg.worker_bin, mode.name(), &hub_store, hub_profile.as_ref())?;
    hub.ok(&Request { op: "setup".into(), ..Request::default() })?;
    let seed = Worker::bytes(&hub.ok(&Request { op: "export".into(), ..Request::default() })?)?;
    let names: Vec<String> = (0..cfg.agents).map(|i| format!("agent-{i}")).collect();
    let (mut copies, mut peers) = (Vec::new(), Vec::new());
    for (i, name) in names.iter().enumerate() {
        let dir = root.join("agents").join(name);
        let copy_profile = profile(&dir, &[sockets.join(format!("c{i}.sock"))], false)?;
        let mut w = Worker::spawn_in(&cfg.worker_bin, mode.name(), &store(dir, name), copy_profile.as_ref())?;
        let reply = w.ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })?;
        if let Some(key) = reply.result["key"].as_str() {
            peers.push(key.to_string());
        }
        copies.push(Mutex::new(w));
    }
    events.log(None, "setup", json!({"mode": mode.name(), "agents": cfg.agents, "turns": cfg.turns}));

    let mut tools = mode.tools();
    tools.extend(core_tools());
    let run = Run {
        cfg,
        mode,
        events,
        hub: Mutex::new(hub),
        copies,
        names,
        peers,
        sockets,
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
    if let Some(why) = backend.halted() {
        run.events.log(None, "run_stopped", json!({"bytes": why.len(), "digest": digest(why.as_bytes())}));
        return Err(format!("the run stopped: {why}"));
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
pub fn core_tools() -> Vec<ToolSpec> {
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
            if backend.halted().is_some() {
                break;
            }
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

    /// What the hub takes from agent `i`, and as whom.
    fn session(&self, i: usize) -> Session {
        let (quota_contributions, quota_bytes) = self.cfg.quota();
        Session {
            agent: i,
            name: self.names[i].clone(),
            peers: self.peers.clone(),
            quota_contributions,
            quota_bytes,
            sync_seconds: 0,
        }
    }

    /// One sync between copy `i` and the hub, as the mode syncs ([`sync_session`], [`sync_transfers`]), logged.
    fn sync(&self, i: usize) -> Result<Value, String> {
        let session = self.session(i);
        let summary = match self.mode.sync_kind() {
            SyncKind::Session => sync_session(&mut self.hub(), &mut self.copy(i), &self.sockets, &session)?,
            SyncKind::Transfers => sync_transfers(&mut self.hub(), &mut self.copy(i), &session)?,
        };
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
        let deadline = Instant::now() + self.cfg.turn_limit;
        let result = std::thread::scope(|scope| {
            let player = scope.spawn(|| backend.turn(i, &name, turn, socket));
            let served = loop {
                match listener.accept() {
                    Ok((stream, _)) => break self.serve(i, turn, stream, deadline),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if player.is_finished() {
                            break Ok(());
                        }
                        if Instant::now() > deadline {
                            // Step 6 ends the agent's process here; the mock's thread is waited for.
                            break Err("no connection before the turn's deadline".into());
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

    /// MCP over newline-delimited JSON-RPC, for one connection, until it closes or `deadline`
    /// passes (finding `05bee0cd4fc4`), when the turn ends with an error.
    fn serve(&self, i: usize, turn: u32, stream: UnixStream, deadline: Instant) -> Result<(), String> {
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut out = stream.try_clone().map_err(|e| e.to_string())?;
        let mut lines = Lines::new(BufReader::new(crate::shim::Timed { stream, deadline }), MAX_MCP_LINE);
        let mut line = String::new();
        let name = &self.names[i];
        let mut calls = 0;
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
                "tools/call" if calls >= self.cfg.calls_per_turn => {
                    self.events.log(Some(name), "call_refused", json!({"turn": turn, "calls": calls}));
                    let text = "this turn's tool calls are spent";
                    json!({"result": {"content": [{"type": "text", "text": text}], "isError": true}})
                }
                "tools/call" => {
                    calls += 1;
                    json!({"result": self.call(i, turn, &msg["params"])})
                }
                "ping" => json!({"result": {}}),
                _ => json!({"error": {"code": -32601, "message": "method not found"}}),
            };
            let mut reply = reply;
            reply["jsonrpc"] = json!("2.0");
            reply["id"] = id;
            // A client that stops reading holds a reply for at most the turn's time (finding `a8cd5b213d6f`).
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the turn's time is spent".into());
            }
            // macOS refuses a timeout (EINVAL) once the peer has gone; the write then fails at once anyway.
            let _ = out.set_write_timeout(Some(left));
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
        } else if self.mode.hub_tools().contains(&tool.as_str()) {
            // The agent's own worker checks the values; the hub runs the checked form, as this agent.
            let req = Request { op: "validate".into(), tool: Some(tool.clone()), args, ..Request::default() };
            self.copy(i).ok(&req).map(|r| r.result).and_then(|checked| {
                let args = json!({"session": self.session(i), "args": checked});
                let req = Request { op: "hub_tool".into(), tool: Some(tool), args, ..Request::default() };
                self.hub().ok(&req).map(|r| r.result)
            })
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

/// One sync session between `copy` and `hub`, both ways, its bytes relayed
/// through the driver over two sockets in `sockets`: only what each side is
/// missing moves. Its summary: the bytes and digest each way.
pub fn sync_session(hub: &mut Worker, copy: &mut Worker, sockets: &Path, session: &Session) -> Result<Value, String> {
    let i = session.agent;
    let end = |side: &str| -> Result<(PathBuf, UnixListener), String> {
        let path = sockets.join(format!("{side}{i}.sock"));
        let _ = std::fs::remove_file(&path);
        let l = UnixListener::bind(&path).map_err(|e| format!("binding {}: {e}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok((path, l))
    };
    let (copy_path, copy_end) = end("c")?;
    let (hub_path, hub_end) = end("h")?;
    let request = |op: &str, path: &Path| Request {
        op: op.into(),
        args: json!({"socket": path, "session": session}),
        ..Request::default()
    };
    let (server, client) = (request("sync_server", &hub_path), request("sync_client", &copy_path));
    let (server, client) = (&server, &client);
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
    Ok(json!({"up": {"bytes": up, "digest": up_digest}, "down": {"bytes": down, "digest": down_digest}}))
}

/// One sync by transfers: the copy's push to the hub, then the hub's answer
/// to the copy, each as bytes the driver carries and each receiver checks.
pub fn sync_transfers(hub: &mut Worker, copy: &mut Worker, session: &Session) -> Result<Value, String> {
    let up = copy.ok(&Request { op: "push".into(), ..Request::default() })?;
    let up_bytes = Worker::bytes(&up)?;
    let have = up.result["have"].clone();
    let args = json!({"session": session, "meta": up.result});
    hub.ok(&Request { op: "receive".into(), args, bytes: up.bytes, ..Request::default() })?;
    let down =
        hub.ok(&Request { op: "send".into(), args: json!({"session": session, "have": have}), ..Request::default() })?;
    let down_bytes = Worker::bytes(&down)?;
    let args = json!({"meta": down.result});
    copy.ok(&Request { op: "fetch".into(), args, bytes: down.bytes, ..Request::default() })?;
    Ok(json!({
        "up": {"bytes": up_bytes.len(), "digest": digest(&up_bytes)},
        "down": {"bytes": down_bytes.len(), "digest": digest(&down_bytes)},
        "skipped_prs": down.result["skipped"],
    }))
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
    let deadline = Instant::now() + SYNC_RELAY;
    let one_way = |from: &UnixStream, to: &UnixStream| -> Result<Relayed, String> {
        from.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut hashing = Hashing { to, hasher: blake3::Hasher::new() };
        let mut timed = crate::shim::Timed { stream: from.try_clone().map_err(|e| e.to_string())?, deadline };
        let n = crate::shim::relay(&mut timed, &mut hashing);
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
