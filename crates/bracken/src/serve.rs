//! `bracken serve`: one replica answering sync sessions, over HTTP on 127.0.0.1 or over stdio.
//!
//! The server runs step 1's [`Session`] unchanged and adds local policy around it: it stores only
//! contributions whose author is on its allowlist, within per-author and per-credential quotas
//! and a total storage cap, bounds each session's time and received bytes ([`Bounds`]), and
//! appends each contribution it stores for the first time to its first-seen log. Over HTTP each
//! request carries a credential (a bearer token, checked against blake3 hashes); over stdio the
//! transport (SSH) has authenticated already, and the credential is logged as `stdio`.
//!
//! The server must be the only writer of its replica while it runs: it reads the replica once,
//! at startup.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use manyroot::sync::{Message, Rejected};
use manyroot::{AgentId, Contribution, ContributionId, Graph, GroundId, ImportReport, Store};
use serde::{Deserialize, Serialize};

use crate::wire::{Bounds, Refused, Replica, Side, Timed, run_stream, write_frames};

/// What a server accepts, and how much.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Authors whose contributions this server stores. `None`: it stores nothing (pull only).
    pub allow: Option<HashSet<AgentId>>,
    /// Per author key and per credential, over [`QUOTA_WINDOW`].
    pub quota_contributions: u64,
    pub quota_bytes: u64,
    /// The most bytes of contributions the replica may hold.
    pub store_cap: u64,
    pub bounds: Bounds,
    /// Over HTTP, the longest wait between a session's requests.
    pub idle: Duration,
    pub max_sessions: usize,
}

/// The window quotas count over.
pub const QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// The most connections served at once; more are refused.
const MAX_CONNECTIONS: usize = 64;
/// The longest an HTTP request's head may be, and how many headers it may have.
const MAX_HEAD: usize = 16 * 1024;
const MAX_HEADERS: usize = 32;
/// The longest a request's head may take to arrive, in all. Until then a connection is
/// unauthenticated, so this bounds how long any local process can hold a connection slot.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest a response may take to be written; and, in a body, the longest wait for the next
/// bytes. A body as a whole must arrive before the session's deadline.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// One line of the first-seen log.
#[derive(Debug, Serialize, Deserialize)]
pub struct Seen {
    pub id: ContributionId,
    pub first_seen_ms: u64,
    pub credential: String,
    pub author: AgentId,
    pub bytes: u64,
}

/// A quota's use over the window: when each contribution came, and its size.
#[derive(Default)]
struct Usage {
    entries: VecDeque<(u64, u64)>,
    count: u64,
    bytes: u64,
}

impl Usage {
    fn add(&mut self, at_ms: u64, bytes: u64) {
        self.entries.push_back((at_ms, bytes));
        self.count += 1;
        self.bytes += bytes;
    }

    fn expire(&mut self, now_ms: u64) {
        let window = QUOTA_WINDOW.as_millis() as u64;
        while let Some(&(at, bytes)) = self.entries.front() {
            if at + window > now_ms {
                break;
            }
            self.entries.pop_front();
            self.count -= 1;
            self.bytes -= bytes;
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Quota {
    Author(AgentId),
    Credential(String),
}

impl Quota {
    fn whose(&self) -> &'static str {
        match self {
            Quota::Author(_) => "author's",
            Quota::Credential(_) => "credential's",
        }
    }
}

/// The replica and everything that decides what goes into it. One per server, behind a lock.
pub struct Intake {
    store: Store,
    policy: Policy,
    log: Option<FirstSeen>,
    usage: HashMap<Quota, Usage>,
    store_bytes: u64,
    /// Set when the first-seen log can't be written; the server then stores nothing more.
    broken: Option<String>,
}

struct FirstSeen {
    file: File,
    seen: HashSet<ContributionId>,
}

impl Intake {
    /// Opens the first-seen log (`first_seen`, required to store anything) and replays its last
    /// [`QUOTA_WINDOW`] into the quotas, so a restart doesn't reset them.
    pub fn new(store: Store, policy: Policy, first_seen: Option<&Path>) -> Result<Intake> {
        if policy.allow.is_some() && first_seen.is_none() {
            bail!("a server that accepts contributions needs --first-seen");
        }
        let store_bytes = store.graph().iter().map(|c| c.encode().len() as u64).sum();
        let mut intake = Intake { store, policy, log: None, usage: HashMap::new(), store_bytes, broken: None };
        if let Some(path) = first_seen {
            intake.log = Some(intake.replay(path)?);
        }
        Ok(intake)
    }

    fn replay(&mut self, path: &Path) -> Result<FirstSeen> {
        let mut seen = HashSet::new();
        match fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() => bail!("{} isn't a regular file (or is a symlink)", path.display()),
            Ok(_) => {
                let now = now_ms();
                let f = File::open(path).with_context(|| format!("reading {}", path.display()))?;
                for (n, line) in BufReader::new(f).lines().enumerate() {
                    let line = line.with_context(|| format!("reading {}", path.display()))?;
                    let s: Seen = serde_json::from_str(&line)
                        .with_context(|| format!("{}:{}: not a first-seen entry", path.display(), n + 1))?;
                    seen.insert(s.id);
                    if s.first_seen_ms + QUOTA_WINDOW.as_millis() as u64 > now {
                        self.usage.entry(Quota::Author(s.author)).or_default().add(s.first_seen_ms, s.bytes);
                        self.usage.entry(Quota::Credential(s.credential)).or_default().add(s.first_seen_ms, s.bytes);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(FirstSeen { file, seen })
    }

    pub fn ground(&self) -> GroundId {
        self.store.info().ground
    }

    /// Why policy refuses a contribution this replica doesn't hold, given what the batch so far
    /// would add (`pending`), or `None` to accept it.
    fn refusal(&self, c: &Contribution, keys: &[Quota; 2], pending: &HashMap<Quota, (u64, u64)>) -> Option<String> {
        if let Some(why) = &self.broken {
            return Some(format!("this server can't store anything now: {why}"));
        }
        let Some(allow) = &self.policy.allow else {
            return Some("this server accepts no contributions".into());
        };
        if !allow.contains(&c.author()) {
            return Some(format!("author {} isn't on this server's allowlist", c.author()));
        }
        let bytes = c.encode().len() as u64;
        // One batch has one credential, so its pending bytes are the whole batch's.
        let pending_store = pending.get(&keys[1]).map_or(0, |p| p.1);
        if self.store_bytes + pending_store + bytes > self.policy.store_cap {
            return Some("this server's storage cap is reached".into());
        }
        keys.iter().find_map(|k| {
            let (count, used) = self.usage.get(k).map_or((0, 0), |u| (u.count, u.bytes));
            let (n, b) = pending.get(k).copied().unwrap_or_default();
            (count + n + 1 > self.policy.quota_contributions || used + b + bytes > self.policy.quota_bytes)
                .then(|| format!("the {} quota for the last 24 hours is spent", k.whose()))
        })
    }

    /// Refuses what policy refuses, imports the rest, and logs and counts what's new. If a write
    /// fails partway, what was stored before it is still logged and counted, and the server then
    /// stores nothing more.
    fn accept(&mut self, credential: &str, batch: Vec<Contribution>) -> Result<(ImportReport, Vec<Rejected>)> {
        let now = now_ms();
        for usage in self.usage.values_mut() {
            usage.expire(now);
        }
        let mut refused = Vec::new();
        let mut passed = Vec::new();
        let mut fresh = Vec::new();
        // What this batch would add to each quota so far, so one batch can't overrun one.
        let mut pending: HashMap<Quota, (u64, u64)> = HashMap::new();
        for c in batch {
            if self.store.graph().contains(&c.id()) {
                passed.push(c);
                continue;
            }
            let keys = [Quota::Author(c.author()), Quota::Credential(credential.to_owned())];
            if let Some(reason) = self.refusal(&c, &keys, &pending) {
                refused.push(Rejected { id: c.id(), reason });
                continue;
            }
            let bytes = c.encode().len() as u64;
            for k in keys {
                let p = pending.entry(k).or_default();
                p.0 += 1;
                p.1 += bytes;
            }
            fresh.push(c.id());
            passed.push(c);
        }
        let sizes: HashMap<ContributionId, (AgentId, u64)> =
            passed.iter().map(|c| (c.id(), (c.author(), c.encode().len() as u64))).collect();
        let (report, failed) = match self.store.import(passed) {
            Ok(report) => (report, None),
            Err(e) => {
                // The graph adds each contribution once it's written, so it holds exactly those.
                let added = fresh.into_iter().filter(|id| self.store.graph().contains(id)).collect();
                (ImportReport { added, ..ImportReport::default() }, Some(e))
            }
        };
        let mut lines = Vec::new();
        for id in &report.added {
            let (author, bytes) = sizes[id];
            self.store_bytes += bytes;
            self.usage.entry(Quota::Author(author)).or_default().add(now, bytes);
            self.usage.entry(Quota::Credential(credential.to_owned())).or_default().add(now, bytes);
            let log = self.log.as_mut().expect("only a server with a first-seen log accepts contributions");
            if log.seen.insert(*id) {
                let seen = Seen { id: *id, first_seen_ms: now, credential: credential.to_owned(), author, bytes };
                lines.extend(serde_json::to_vec(&seen).expect("entries serialize"));
                lines.push(b'\n');
            }
        }
        if !lines.is_empty() {
            let log = self.log.as_mut().expect("lines come from a server with a log");
            if let Err(e) = log.file.write_all(&lines).and_then(|()| log.file.sync_data()) {
                self.broken = Some(format!("writing the first-seen log: {e}"));
                return Err(e).context("writing the first-seen log");
            }
        }
        if let Some(e) = failed {
            self.broken = Some("a write to its replica failed".into());
            return Err(e.into());
        }
        Ok((report, refused))
    }
}

/// The intake as one session's replica, under one credential.
struct Locked<'a> {
    intake: MutexGuard<'a, Intake>,
    credential: &'a str,
}

impl Replica for Locked<'_> {
    fn graph(&self) -> &Graph {
        self.intake.store.graph()
    }

    fn import(&mut self, batch: Vec<Contribution>) -> Result<(ImportReport, Vec<Rejected>)> {
        self.intake.accept(self.credential, batch)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a lock is a bug, but the state it guards stays usable: every write to
    // the replica is an atomic rename, and the first-seen log is written after the import.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Refuses a configuration file that is a symlink, not a regular file, or writable by anyone.
pub fn check_config_file(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if m.file_type().is_symlink() {
        bail!("{} is a symlink; give the file itself", path.display());
    }
    if !m.is_file() {
        bail!("{} isn't a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o002 != 0 {
            bail!("{} is writable by anyone", path.display());
        }
    }
    Ok(())
}

/// Reads an allowlist: one author key in hex per line, optionally followed by a name; `#` starts
/// a comment.
pub fn read_allowlist(path: &Path) -> Result<HashSet<AgentId>> {
    check_config_file(path)?;
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut keys = HashSet::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some(key) = line.split_whitespace().next() else { continue };
        let key: AgentId = key.parse().with_context(|| format!("{}:{}: not an author key", path.display(), n + 1))?;
        keys.insert(key);
    }
    Ok(keys)
}

/// HTTP credentials: a name and the blake3 hash of its token, so the file holds no secret.
pub struct Tokens(Vec<(String, blake3::Hash)>);

impl Tokens {
    /// Reads lines of `<name> <blake3 hex of the token>`; `#` starts a comment.
    pub fn read(path: &Path) -> Result<Tokens> {
        check_config_file(path)?;
        let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut tokens: Vec<(String, blake3::Hash)> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let at = || format!("{}:{}", path.display(), n + 1);
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(name), Some(hash), None) = (parts.next(), parts.next(), parts.next()) else {
                bail!("{}: expected '<name> <blake3 hex of the token>'", at());
            };
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                bail!("{}: a credential name is letters, digits, '.', '_' and '-'", at());
            }
            if tokens.iter().any(|(n, _)| n == name) {
                bail!("{}: credential {name:?} is listed twice", at());
            }
            let hash = blake3::Hash::from_hex(hash).with_context(|| format!("{}: not a blake3 hash", at()))?;
            tokens.push((name.to_owned(), hash));
        }
        if tokens.is_empty() {
            bail!("{} lists no credentials", path.display());
        }
        Ok(Tokens(tokens))
    }

    /// The credential a token belongs to. Compares every hash, each in constant time.
    fn check(&self, token: &[u8]) -> Option<&str> {
        let hash = blake3::hash(token);
        let mut found = None;
        for (name, h) in &self.0 {
            if *h == hash {
                found = Some(name.as_str());
            }
        }
        found
    }
}

/// Serves one session on stdin and stdout, then returns. At the deadline the process ends, once
/// no import is in progress.
pub fn serve_stdio(intake: Intake) -> Result<()> {
    let (ground, bounds) = (intake.ground(), intake.policy.bounds);
    let intake = Arc::new(Mutex::new(intake));
    let mut side = Side::open(ground, bounds);
    let watchdog = Arc::clone(&intake);
    std::thread::spawn(move || {
        std::thread::sleep(bounds.deadline);
        let _held = lock(&watchdog);
        eprintln!("bracken serve: {}", Refused::deadline());
        std::process::exit(1);
    });
    let opening = side.session.start(lock(&intake).store.graph());
    let (mut input, mut output) = (io::stdin().lock(), io::stdout().lock());
    run_stream(&mut side, opening, &mut input, &mut output, |side, frame| {
        side.handle(&mut Locked { intake: lock(&intake), credential: "stdio" }, frame)
    })
}

/// A session served over HTTP, between requests.
struct HttpSession {
    side: Side,
    credential: String,
    last_seen: Instant,
}

struct Server {
    intake: Mutex<Intake>,
    tokens: Tokens,
    ground: GroundId,
    policy: Policy,
    sessions: Mutex<HashMap<[u8; 16], Arc<Mutex<HttpSession>>>>,
    /// Opens under way, which hold a session slot before their session exists.
    opening: AtomicUsize,
    connections: AtomicUsize,
}

/// A session slot held by an open under way, released when dropped.
struct Reservation<'a>(&'a AtomicUsize);

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Listens on 127.0.0.1:`port` (0 for any free port), prints the URL on stdout, and serves until
/// the process ends. Each connection carries one request.
pub fn serve_http(intake: Intake, tokens: Tokens, port: u16) -> Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).context("listening on 127.0.0.1")?;
    let (ground, policy) = (intake.ground(), intake.policy.clone());
    let server = Arc::new(Server {
        intake: Mutex::new(intake),
        tokens,
        ground,
        policy,
        sessions: Mutex::new(HashMap::new()),
        opening: AtomicUsize::new(0),
        connections: AtomicUsize::new(0),
    });
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "listening on http://{}", listener.local_addr()?)?;
    stdout.flush()?;
    drop(stdout);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if server.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            server.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(&stream, Err(Refused::new(503, "too many connections")));
            continue;
        }
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            let response = server.request(&stream);
            if let Err(refused) = &response {
                eprintln!("bracken serve: refused a request: {} {}", refused.status, refused.reason);
            }
            let _ = respond(&stream, response);
            linger(&stream);
            server.connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

/// Closes a connection after its response without losing the response: closing a socket with
/// unread input (a pipelined request, an unread body) resets it, and the peer may then drop the
/// response. So this half-closes, and discards a bounded amount of input first.
fn linger(stream: &TcpStream) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut input = Timed::new(stream, Instant::now() + Duration::from_secs(1), None);
    let _ = io::copy(&mut (&mut input).take(1 << 20), &mut io::sink());
}

/// A successful response: the server's messages, and the session id when it opened one.
struct Reply {
    messages: Vec<Message>,
    session: Option<[u8; 16]>,
}

fn respond(stream: &TcpStream, response: Result<Reply, Refused>) -> io::Result<()> {
    let mut stream = Timed::new(stream, Instant::now() + IO_TIMEOUT, None);
    let (status, mut extra, body) = match response {
        Ok(reply) => {
            let mut body = Vec::new();
            write_frames(&mut body, &reply.messages)?;
            let extra = reply.session.map(|id| format!("X-Bracken-Session: {}\r\n", hex(&id))).unwrap_or_default();
            (200, extra, body)
        }
        Err(refused) => (refused.status, String::new(), format!("{}\n", refused.reason).into_bytes()),
    };
    let kind = if status == 200 { "application/octet-stream" } else { "text/plain; charset=utf-8" };
    extra.push_str(&format!("Content-Type: {kind}\r\n"));
    let head = format!(
        "HTTP/1.1 {status} {}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        reason_phrase(status),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// An HTTP request's head, parsed.
struct Head {
    method: String,
    path: String,
    authorization: Option<Vec<u8>>,
    content_length: Option<u64>,
}

/// Reads an HTTP head of at most [`MAX_HEAD`] bytes, ending in a blank line. Returns it and the
/// bytes read past it, which start the body.
pub fn read_head_bytes(stream: &mut impl Read) -> Result<(Vec<u8>, Vec<u8>), Refused> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(i + 4);
            return Ok((buf, rest));
        }
        if buf.len() >= MAX_HEAD {
            return Err(Refused::new(431, "head too large"));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(Refused::new(400, "connection closed inside the head")),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Refused::new(408, "the head didn't arrive in time")),
        }
    }
}

/// A Content-Length value: digits only.
pub fn parse_length(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value)
        .ok()
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
}

fn read_head(stream: &mut impl Read) -> Result<(Head, Vec<u8>), Refused> {
    let (bytes, rest) = read_head_bytes(stream)?;
    if bytes.len() > MAX_HEAD {
        return Err(Refused::new(431, "head too large"));
    }
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(&bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => return Err(Refused::new(400, "incomplete request head")),
        Err(httparse::Error::TooManyHeaders) => return Err(Refused::new(431, "too many headers")),
        Err(e) => return Err(Refused::new(400, format!("bad request: {e}"))),
    }
    let mut authorization = None;
    let mut content_length = None;
    for h in req.headers.iter() {
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Refused::new(501, "Transfer-Encoding isn't supported; send Content-Length"));
        }
        if h.name.eq_ignore_ascii_case("content-length") {
            let n = parse_length(h.value).ok_or_else(|| Refused::new(400, "bad Content-Length"))?;
            if content_length.replace(n).is_some() {
                return Err(Refused::new(400, "more than one Content-Length"));
            }
        }
        if h.name.eq_ignore_ascii_case("authorization") && authorization.replace(h.value.to_vec()).is_some() {
            return Err(Refused::new(400, "more than one Authorization"));
        }
    }
    let head = Head {
        method: req.method.unwrap_or_default().to_owned(),
        path: req.path.unwrap_or_default().to_owned(),
        authorization,
        content_length,
    };
    Ok((head, rest))
}

/// The session id in a path's last segment.
fn parse_session_id(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut id = [0u8; 16];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(id)
}

impl Server {
    fn request(&self, stream: &TcpStream) -> Result<Reply, Refused> {
        let (head, early) = read_head(&mut Timed::new(stream, Instant::now() + HEAD_TIMEOUT, None))?;
        if head.method != "POST" {
            return Err(Refused::new(405, "only POST is served"));
        }
        let length = head.content_length.ok_or_else(|| Refused::new(411, "Content-Length is required"))?;
        let credential = head
            .authorization
            .as_deref()
            .and_then(|v| v.strip_prefix(b"Bearer "))
            .and_then(|token| self.tokens.check(token))
            .ok_or_else(|| Refused::new(401, "a valid bearer token is required"))?
            .to_owned();
        let prefix = format!("/v1/{}/sessions", self.ground);
        let rest = head.path.strip_prefix(&prefix).ok_or_else(|| Refused::new(404, "no such ground here"))?;
        let body = Body { stream, early, length };
        if rest.is_empty() {
            return self.open(&credential, body);
        }
        let id =
            rest.strip_prefix('/').and_then(parse_session_id).ok_or_else(|| Refused::new(404, "no such session"))?;
        Ok(Reply { messages: self.round(id, &credential, body)?, session: None })
    }

    /// Drops sessions past their deadline or idle too long, then reserves a slot for a new one if
    /// the sessions and the opens under way leave room. Both happen under the sessions lock, so
    /// concurrent opens can't all pass the check.
    fn reserve(&self) -> Option<Reservation<'_>> {
        let mut sessions = lock(&self.sessions);
        sessions.retain(|_, s| match s.try_lock() {
            Ok(s) => s.side.time_left().is_ok() && s.last_seen.elapsed() < self.policy.idle,
            Err(_) => true, // in use by a request, which its deadline bounds
        });
        if sessions.len() + self.opening.load(Ordering::SeqCst) >= self.policy.max_sessions {
            return None;
        }
        self.opening.fetch_add(1, Ordering::SeqCst);
        Some(Reservation(&self.opening))
    }

    fn open(&self, credential: &str, body: Body) -> Result<Reply, Refused> {
        let Some(_slot) = self.reserve() else {
            return Err(Refused::new(429, "too many sessions; try again later"));
        };
        let mut side = Side::open(self.ground, self.policy.bounds);
        let mut messages = side.session.start(lock(&self.intake).store.graph());
        messages.extend(self.frames(&mut side, credential, body)?);
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|e| Refused::new(500, format!("no randomness for a session id: {e}")))?;
        if !side.session.is_done() {
            let session = HttpSession { side, credential: credential.to_owned(), last_seen: Instant::now() };
            lock(&self.sessions).insert(id, Arc::new(Mutex::new(session)));
        }
        Ok(Reply { messages, session: Some(id) })
    }

    fn round(&self, id: [u8; 16], credential: &str, body: Body) -> Result<Vec<Message>, Refused> {
        let not_found = || Refused::new(404, "no such session");
        let session = lock(&self.sessions).get(&id).cloned().ok_or_else(not_found)?;
        let Ok(mut s) = session.try_lock() else {
            return Err(Refused::new(409, "the session is busy with another request"));
        };
        if s.credential != credential {
            return Err(not_found());
        }
        if s.last_seen.elapsed() >= self.policy.idle {
            lock(&self.sessions).remove(&id);
            return Err(Refused::new(408, "the session was idle too long"));
        }
        let result = self.frames(&mut s.side, credential, body);
        s.last_seen = Instant::now();
        if result.is_err() || s.side.session.is_done() {
            lock(&self.sessions).remove(&id);
        }
        result
    }

    /// Runs every frame in a request body through the session, and returns the replies. The body
    /// must fit what's left of the session's budget, and arrive before its deadline.
    fn frames(&self, side: &mut Side, credential: &str, body: Body) -> Result<Vec<Message>, Refused> {
        if body.length > side.budget_left() {
            return Err(Refused::new(413, "the request is larger than what's left of the session's budget"));
        }
        side.time_left()?;
        let stream = Timed::new(body.stream, side.deadline(), Some(IO_TIMEOUT));
        let mut reader = Cursor::new(body.early).chain(stream).take(body.length);
        let mut replies: Vec<Message> = Vec::new();
        while let Some(frame) = side.read_frame(&mut reader).map_err(refusal)? {
            let mut replica = Locked { intake: lock(&self.intake), credential };
            replies.extend(side.handle(&mut replica, &frame).map_err(refusal)?);
            // A response carries at most one batch, so its size is bounded by the batch limit. A
            // client that follows the protocol never asks for more: it answers each batch it
            // receives with one result, and it receives at most one per response. Only results
            // pipelined ahead of their batches would.
            if replies.iter().filter(|m| matches!(m, Message::Batch { .. })).count() > 1 {
                return Err(Refused::protocol("a request may release only one batch"));
            }
        }
        if reader.limit() > 0 {
            return Err(Refused::new(400, "the body is shorter than its Content-Length"));
        }
        Ok(replies)
    }
}

/// A request's body: the bytes read with its head, then the rest of the stream, up to its length.
struct Body<'a> {
    stream: &'a TcpStream,
    early: Vec<u8>,
    length: u64,
}

/// The refusal behind an error, or an internal error.
fn refusal(e: anyhow::Error) -> Refused {
    match e.downcast::<Refused>() {
        Ok(r) => r,
        Err(e) => {
            // The details name the server's files; they stay in its own log.
            eprintln!("bracken serve: {e:#}");
            Refused::new(500, "the server failed to store the batch; the session is over")
        }
    }
}
