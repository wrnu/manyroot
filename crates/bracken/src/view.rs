//! `bracken view`: a read-only web view of one replica, served on 127.0.0.1 only.
//!
//! The page and its scripts are embedded in the binary. The JSON API is the shapes of
//! docs/design/visualizer.md section 1, built from the same library calls and field names as the
//! CLI's `--json` forms. Nothing here writes: the replica is read with `Store::open` (and re-read
//! when its object count changes), and every endpoint is a GET. Agents' names come from the public
//! `agents/` directory only, never from `identities/`, so no private key is ever in this process.
//!
//! Ground text is untrusted. It goes out only as JSON string values, with `<`, `>`, `&`, U+2028 and
//! U+2029 escaped as `\u` sequences (the same strings once parsed), under a Content-Security-Policy
//! that allows no inline script and no eval. The page inserts it with `textContent` only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use manyroot::validation::open_conflicts;
use manyroot::{AgentEntry, Store};
use manyroot::{
    AgentId, Assessment, Body, Conflict, Contribution, ContributionId, Graph, Policy, RelationKind, Status,
};
use serde_json::{Map, Value, json};

use crate::serve::{linger, read_head_bytes, reason_phrase};
use crate::wire::{Refused, Timed};
use crate::{
    CurrentView, Names, contribution_json, intent_json, intent_until, listing, live_intents, now_ms, show_json,
};

/// Every response's security headers. The CSP is the design's (section 1.7): nothing loads from
/// anywhere but this server, no inline script or style, no eval, no framing.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                       connect-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'; \
                       frame-ancestors 'none'";

/// The embedded page: path, media type, content.
const ASSETS: &[(&str, &str, &[u8])] = &[
    ("/", "text/html; charset=utf-8", include_bytes!("../assets/view/index.html")),
    ("/app.js", "text/javascript; charset=utf-8", include_bytes!("../assets/view/app.js")),
    ("/style.css", "text/css; charset=utf-8", include_bytes!("../assets/view/style.css")),
];

/// The most connections served at once, and of those, event streams; more are refused.
const MAX_CONNECTIONS: usize = 64;
const MAX_STREAMS: usize = 16;
const MAX_HEADERS: usize = 32;
/// The longest a request's head may take to arrive, and a response's write.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the replica is checked for change, and how often an idle event stream is pinged.
const POLL: Duration = Duration::from_secs(1);
const PING: Duration = Duration::from_secs(15);
/// An artifact's text in `/api/contribution`, a tag in `/api/graph`, and a statement in the short
/// entries of `/api/root` are cut to these.
const TEXT_BYTES: usize = 64 * 1024;
const TAG_CHARS: usize = 64;
const SHORT_CHARS: usize = 200;
/// The fewest digits an id in a path may have.
const PATH_ID_DIGITS: usize = 8;
/// How many `?as_of=` graphs are kept.
const PAST_GRAPHS: usize = 8;

/// One reading of the replica, with what every request needs from it computed once.
struct Snapshot {
    store: Store,
    names: Names,
    order: Vec<ContributionId>,
    /// The longest causal path from a root to each contribution.
    depth: HashMap<ContributionId, u64>,
    /// Every author, in order of first contribution when first seen; later authors are appended,
    /// so an author's index stays the same while the server runs.
    agents: Vec<AgentId>,
    agent_index: HashMap<AgentId, usize>,
    /// Subjects whose status isn't pending with no counted evidence (section 1.3).
    statuses: BTreeMap<ContributionId, Assessment>,
    conflicts: Vec<Conflict>,
    /// What changes when the replica does: its object and agent entry counts.
    signature: (usize, usize),
}

impl Snapshot {
    /// `signature` must be taken before `store` was opened: something written while it was being
    /// read then changes the signature again, and the next poll reads the replica again.
    fn new(store: Store, policy: &Policy, previous: Option<&Snapshot>, signature: (usize, usize)) -> Result<Snapshot> {
        let names = public_names(&store)?;
        let g = store.graph();
        let order = g.topo_order();
        let mut depth: HashMap<ContributionId, u64> = HashMap::with_capacity(order.len());
        for id in &order {
            let parents = g.get(id).expect("ordered ids exist").parents();
            let d = parents.iter().map(|p| depth[p] + 1).max().unwrap_or(0);
            depth.insert(*id, d);
        }
        let mut agents: Vec<AgentId> = previous.map_or_else(Vec::new, |p| p.agents.clone());
        let mut agent_index: HashMap<AgentId, usize> = agents.iter().enumerate().map(|(i, a)| (*a, i)).collect();
        for id in &order {
            let author = g.get(id).expect("ordered ids exist").author();
            if let std::collections::hash_map::Entry::Vacant(e) = agent_index.entry(author) {
                e.insert(agents.len());
                agents.push(author);
            }
        }
        let statuses = order
            .iter()
            .filter_map(|id| policy.assess(g, id))
            .filter(|a| a.status != Status::Pending || !a.passes.is_empty() || !a.fails.is_empty())
            .map(|a| (a.id, a))
            .collect();
        let conflicts = policy.evaluate(g).conflicts;
        Ok(Snapshot { store, names, order, depth, agents, agent_index, statuses, conflicts, signature })
    }

    fn graph(&self) -> &Graph {
        self.store.graph()
    }
}

/// Agents' names from the public directory, `agents/*.json`. Unlike `Names::load` (which goes
/// through `Store::agents`), this never reads `identities/`, where the private keys are.
pub fn public_names(store: &Store) -> Result<Names> {
    let dir = store.agents_dir();
    let mut names = HashMap::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Names(names)),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let path = entry.with_context(|| format!("reading {}", dir.display()))?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let entry: AgentEntry = serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        names.insert(entry.agent, entry.name);
    }
    Ok(Names(names))
}

/// The replica's object files and agent entries, counted. The Ground is append-only, so a change
/// in either means something arrived.
fn signature(dir: &Path) -> Result<(usize, usize)> {
    let count = |d: PathBuf| -> Result<usize> {
        match std::fs::read_dir(&d) {
            Ok(entries) => {
                Ok(entries.filter_map(|e| e.ok()).filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e).with_context(|| format!("reading {}", d.display())),
        }
    };
    let mut objects = 0;
    if let Ok(shards) = std::fs::read_dir(dir.join("objects")) {
        for shard in shards.filter_map(|s| s.ok()).filter(|s| s.path().is_dir()) {
            objects += count(shard.path())?;
        }
    }
    Ok((objects, count(dir.join("agents"))?))
}

/// The replica as a cursor saw it, and its causal order.
type Past = (Graph, Vec<ContributionId>);

struct Viewer {
    dir: PathBuf,
    port: u16,
    policy: Policy,
    /// The latest snapshot and its generation, which the watcher bumps; streams wait on `changed`.
    current: Mutex<(u64, Arc<Snapshot>)>,
    changed: Condvar,
    connections: AtomicUsize,
    streams: AtomicUsize,
    /// The graphs `?as_of=` built last, by the hash of the set they hold.
    past: Mutex<Vec<(blake3::Hash, Arc<Past>)>>,
    /// For tests only (`BRACKEN_VIEW_REREAD_DELAY_MS`): how long a re-read waits after reading the
    /// replica before it's used, so a test can write while one is under way.
    reread_delay: Duration,
}

/// Listens on 127.0.0.1 only, at `port` (0 for any free port), prints the URL on stdout, and serves
/// the replica in `dir` until the process ends.
pub fn serve(dir: &Path, policy: Policy, port: u16) -> Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).context("listening on 127.0.0.1")?;
    let port = listener.local_addr()?.port();
    let dir = dir.to_owned();
    let first = signature(&dir)?;
    let snapshot = Snapshot::new(Store::open(&dir)?, &policy, None, first)?;
    let reread_delay = std::env::var("BRACKEN_VIEW_REREAD_DELAY_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(Duration::ZERO, Duration::from_millis);
    let viewer = Arc::new(Viewer {
        dir,
        port,
        policy,
        current: Mutex::new((0, Arc::new(snapshot))),
        changed: Condvar::new(),
        connections: AtomicUsize::new(0),
        streams: AtomicUsize::new(0),
        past: Mutex::new(Vec::new()),
        reread_delay,
    });
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "view at http://127.0.0.1:{port}/")?;
    stdout.flush()?;
    drop(stdout);
    let watcher = Arc::clone(&viewer);
    std::thread::spawn(move || watcher.watch());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if viewer.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            viewer.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(&stream, &error(503, "too many connections"));
            continue;
        }
        let viewer = Arc::clone(&viewer);
        std::thread::spawn(move || {
            // Given back even if the handler panics.
            let _slot = Release(&viewer.connections);
            viewer.connection(&stream);
            linger(&stream);
        });
    }
    Ok(())
}

/// A response with a body of known length.
struct Response {
    status: u16,
    media_type: &'static str,
    body: Vec<u8>,
}

fn json_response(v: &Value) -> Response {
    Response { status: 200, media_type: "application/json", body: json_bytes(v) }
}

fn error(status: u16, message: impl Into<String>) -> Response {
    Response { status, media_type: "application/json", body: json_bytes(&json!({"error": message.into()})) }
}

/// JSON with `<`, `>`, `&`, U+2028 and U+2029 written as `\u` escapes. Outside strings JSON has none
/// of them, so this changes no value; it only keeps markup out of the bytes, should anything ever
/// read them as HTML.
fn json_bytes(v: &Value) -> Vec<u8> {
    let text = serde_json::to_string(v).expect("values serialize");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.into_bytes()
}

/// The headers every response carries, whatever its status. There are no CORS headers, so no other
/// origin can read a response.
fn security_headers() -> String {
    format!(
        "Content-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Cache-Control: no-store\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Frame-Options: DENY\r\n"
    )
}

fn respond(stream: &TcpStream, r: &Response) -> io::Result<()> {
    let mut out = Timed::new(stream, Instant::now() + IO_TIMEOUT, None);
    let allow = if r.status == 405 { "Allow: GET\r\n" } else { "" };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{allow}{}Connection: close\r\n\r\n",
        r.status,
        reason_phrase(r.status),
        r.media_type,
        r.body.len(),
        security_headers()
    );
    out.write_all(head.as_bytes())?;
    out.write_all(&r.body)?;
    out.flush()
}

/// A request's method, path and query, and its Host header.
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    host: Option<String>,
}

fn read_request(stream: &TcpStream) -> Result<Request, Refused> {
    let (bytes, _) = read_head_bytes(&mut Timed::new(stream, Instant::now() + HEAD_TIMEOUT, None))?;
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(&bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) => return Err(Refused::new(400, "incomplete request head")),
        Err(httparse::Error::TooManyHeaders) => return Err(Refused::new(431, "too many headers")),
        Err(e) => return Err(Refused::new(400, format!("bad request: {e}"))),
    }
    let mut host = None;
    for h in req.headers.iter().filter(|h| h.name.eq_ignore_ascii_case("host")) {
        if host.replace(String::from_utf8_lossy(h.value).into_owned()).is_some() {
            return Err(Refused::new(400, "more than one Host"));
        }
    }
    let target = req.path.unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut parameters = HashMap::new();
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let k = percent_decode(k)?;
        if parameters.insert(k.clone(), percent_decode(v)?).is_some() {
            return Err(Refused::new(400, format!("the query gives {k} more than once")));
        }
    }
    let query = parameters;
    Ok(Request { method: req.method.unwrap_or_default().to_owned(), path: path.to_owned(), query, host })
}

fn percent_decode(s: &str) -> Result<String, Refused> {
    let bad = || Refused::new(400, "bad percent-encoding in the query");
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'%' => {
                let hex = [bytes.next().ok_or_else(bad)?, bytes.next().ok_or_else(bad)?];
                let hex = std::str::from_utf8(&hex).map_err(|_| bad())?;
                out.push(u8::from_str_radix(hex, 16).map_err(|_| bad())?);
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
    }
    String::from_utf8(out).map_err(|_| bad())
}

impl Viewer {
    fn snapshot(&self) -> (u64, Arc<Snapshot>) {
        let current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        (current.0, Arc::clone(&current.1))
    }

    /// Re-reads the replica whenever its signature changes, and wakes the event streams. A replica
    /// that fails to read (a write half done elsewhere, say) is tried again at the next poll.
    fn watch(&self) {
        loop {
            std::thread::sleep(POLL);
            let (_, last) = self.snapshot();
            // Counted before the replica is read (finding 31f5a44fc779): a write during the read
            // changes the count again, so the next poll reads again rather than missing it.
            let Ok(now) = signature(&self.dir) else { continue };
            if now == last.signature {
                continue;
            }
            let next = Store::open(&self.dir).map_err(anyhow::Error::from).and_then(|store| {
                std::thread::sleep(self.reread_delay);
                Snapshot::new(store, &self.policy, Some(&last), now)
            });
            match next {
                Ok(next) => {
                    let mut current = self.current.lock().unwrap_or_else(|p| p.into_inner());
                    *current = (current.0 + 1, Arc::new(next));
                    self.changed.notify_all();
                }
                Err(e) => eprintln!("bracken view: re-reading the replica: {e:#}"),
            }
        }
    }

    fn connection(&self, stream: &TcpStream) {
        let request = match read_request(stream) {
            Ok(r) => r,
            Err(refused) => {
                let _ = respond(stream, &error(refused.status, refused.reason));
                return;
            }
        };
        let ours = [format!("127.0.0.1:{}", self.port), format!("localhost:{}", self.port)];
        // Against DNS rebinding: a page from another site that resolves its own name to 127.0.0.1
        // sends its own name as the Host.
        if !request.host.as_ref().is_some_and(|h| ours.contains(&h.to_ascii_lowercase())) {
            let _ = respond(stream, &error(403, format!("this view answers only to http://{}/", ours[0])));
            return;
        }
        if request.method != "GET" {
            let _ = respond(stream, &error(405, "this view is read-only: only GET is served"));
            return;
        }
        // A parameter is never ignored: one a page doesn't take is refused, as is a bad value.
        let takes: &[&str] = match request.path.as_str() {
            "/api/graph" | "/api/contributions" => &["since", "limit"],
            "/api/events" => &["since"],
            path if path.starts_with("/api/root/") => &["as_of"],
            _ => &[],
        };
        if let Some(k) = request.query.keys().find(|k| !takes.contains(&k.as_str())) {
            let _ = respond(stream, &error(400, format!("{} takes no parameter {k:?}", request.path)));
            return;
        }
        if request.query.get("as_of").is_some_and(|c| c.split(',').all(|p| p.trim().is_empty())) {
            let _ = respond(stream, &error(400, "as_of needs a cursor: the ids of the heads to answer as of"));
            return;
        }
        if request.path == "/api/events" {
            self.events(stream, request.query.get("since").map(String::as_str));
            return;
        }
        let _ = respond(stream, &self.route(&request));
    }

    fn route(&self, request: &Request) -> Response {
        if let Some((_, media_type, body)) = ASSETS.iter().find(|(p, ..)| *p == request.path) {
            return Response { status: 200, media_type, body: body.to_vec() };
        }
        let (_, snap) = self.snapshot();
        let since = request.query.get("since").map(String::as_str);
        let limit = match request.query.get("limit").map(|l| l.parse::<usize>()) {
            None => None,
            Some(Ok(n)) if n > 0 => Some(n),
            Some(_) => return error(400, "limit is a whole number, at least 1"),
        };
        let result = match request.path.as_str() {
            "/api/ground" => Ok(self.ground_json(&snap)),
            "/api/graph" => self.page(&snap, since, limit).map(|p| graph_json(&snap, &p)),
            "/api/contributions" => self.page(&snap, since, limit).map(|p| contributions_json(&snap, &p)),
            "/api/status" => Ok(self.status_json(&snap)),
            "/api/conflicts" => Ok(json!(open_conflicts(snap.graph()))),
            "/api/intents" => Ok(intents_json(&snap)),
            path => match (path.strip_prefix("/api/contribution/"), path.strip_prefix("/api/root/")) {
                (Some(id), _) => self.contribution_json(&snap, id),
                (_, Some(id)) => self.root_json(&snap, id, request.query.get("as_of").map(String::as_str)),
                _ => Err(error(404, "no such page")),
            },
        };
        match result {
            Ok(v) => json_response(&v),
            Err(r) => r,
        }
    }

    fn policy_json(&self) -> Value {
        json!({"min_passes": self.policy.min_passes, "validators": self.policy.validators.iter().flatten().collect::<Vec<_>>()})
    }

    /// Section 1.1.
    fn ground_json(&self, snap: &Snapshot) -> Value {
        let g = snap.graph();
        let mut stats: HashMap<AgentId, (u64, u64, u64)> = HashMap::new();
        for c in g.iter() {
            let t = c.content().timestamp_ms;
            let s = stats.entry(c.author()).or_insert((0, t, t));
            *s = (s.0 + 1, s.1.min(t), s.2.max(t));
        }
        let agents: Vec<Value> = snap
            .agents
            .iter()
            .map(|a| {
                let (n, first, last) = stats.get(a).copied().unwrap_or_default();
                json!({"agent": a, "name": snap.names.0.get(a), "contributions": n, "first_ms": first, "last_ms": last})
            })
            .collect();
        let mut heads: Vec<ContributionId> = g.heads().collect();
        heads.sort();
        json!({
            "ground": snap.store.info().ground,
            "name": snap.store.info().name,
            "now_ms": now_ms(),
            "contributions": g.len(),
            "heads": heads,
            "cursor": listing::cursor(g),
            "policy": self.policy_json(),
            "agents": agents,
        })
    }

    /// What's new since a cursor, in causal order, at most `limit` of it.
    fn page(&self, snap: &Snapshot, since: Option<&str>, limit: Option<usize>) -> Result<Page, Response> {
        let g = snap.graph();
        let seen = since.map(|c| listing::seen(g, c)).transpose().map_err(|e| error(400, format!("{e:#}")))?;
        let fresh: Vec<ContributionId> =
            snap.order.iter().filter(|id| seen.as_ref().is_none_or(|s| !s.contains(id))).copied().collect();
        let more = limit.is_some_and(|n| fresh.len() > n);
        let ids = fresh[..limit.map_or(fresh.len(), |n| n.min(fresh.len()))].to_vec();
        let cursor = if more { cursor_after(g, seen.unwrap_or_default(), &ids) } else { listing::cursor(g) };
        Ok(Page { ids, cursor, more })
    }

    /// Section 1.3.
    fn status_json(&self, snap: &Snapshot) -> Value {
        let statuses: Map<String, Value> = snap
            .statuses
            .iter()
            .map(|(id, a)| {
                let v = json!({
                    "status": a.status,
                    "passes": a.passes.len(),
                    "fails": a.fails.len(),
                    "superseded_by": a.superseded_by,
                });
                (id.to_hex(), v)
            })
            .collect();
        json!({"policy": self.policy_json(), "statuses": statuses, "conflicts": snap.conflicts})
    }

    /// Section 1.4: `show --json`, with an artifact's text cut to [`TEXT_BYTES`].
    fn contribution_json(&self, snap: &Snapshot, prefix: &str) -> Result<Value, Response> {
        let g = snap.graph();
        let id = resolve(g, prefix)?;
        let c = g.get(&id).expect("resolved ids exist");
        let mut v = show_json(g, &snap.names, &self.policy, c, now_ms());
        if let Some(text) = v["body"]["text"].as_str()
            && text.len() > TEXT_BYTES
        {
            let cut = cut_bytes(text, TEXT_BYTES).to_owned();
            v["body"]["text"] = json!(cut);
            v["body"]["text_truncated"] = json!(true);
        }
        Ok(v)
    }

    /// Section 1.5: one claim's (or artifact's) life.
    ///
    /// With `as_of`, it's worked out over only what that cursor has seen, in a graph of just those
    /// contributions, and "now" (for intents' expiry) is the latest time among them.
    fn root_json(&self, snap: &Snapshot, prefix: &str, as_of: Option<&str>) -> Result<Value, Response> {
        let id = resolve(snap.graph(), prefix)?;
        let past = as_of.map(|cursor| self.as_of(snap, cursor)).transpose()?;
        let (g, order, now) = match &past {
            Some(past) => {
                let latest = past.0.iter().map(|c| c.content().timestamp_ms).max().unwrap_or(0);
                (&past.0, past.1.clone(), latest)
            }
            None => (snap.graph(), snap.order.clone(), now_ms()),
        };
        let Some(c) = g.get(&id) else {
            return Err(error(404, format!("{} isn't in the replica at that cursor", id.short())));
        };
        if !c.body().is_subject() {
            return Err(error(404, format!("{} is a {}, not a claim or artifact", id.short(), c.body().kind())));
        }
        let view = CurrentView::new(g, &order);
        let names = &snap.names;
        let short = |id: &ContributionId| short_json(g, names, &self.policy, id);
        let relations = |kind: RelationKind| {
            g.referrers(&id).filter_map(move |r| match r.body() {
                Body::Relation { from, kind: k, to, .. } if *k == kind => Some((r, *from, *to)),
                _ => None,
            })
        };
        // A relation about this claim stands unless its author retracted it or its other end is hidden.
        // (Not "current": a supersession that counts hides the claim it replaces, and with it the
        // relations about that claim, which are this view's whole subject.)
        let stands =
            |r: &Contribution, other: &ContributionId| !g.is_retracted(&r.id()) && !view.hidden.contains(other);

        let mut claim = slim_json(g, names, c);
        claim["assessment"] = json!(self.policy.assess(g, &id));
        let replaces: Vec<Value> = relations(RelationKind::Supersedes)
            .filter(|(r, f, _)| !g.is_retracted(&r.id()) && *f == id)
            .map(|(_, _, t)| short(&t))
            .collect();
        // A supersession counts only when its relation's author wrote this claim and the successor is a
        // subject its author hasn't retracted, as manyroot's supersessors decide (docs/protocol.md).
        let replaced_by: Vec<Value> = relations(RelationKind::Supersedes)
            .filter(|(r, _, t)| !g.is_retracted(&r.id()) && *t == id)
            .map(|(r, f, _)| {
                let mut v = short(&f);
                let live = g.get(&f).is_some_and(|f| f.body().is_subject()) && !g.is_retracted(&f);
                v["counts"] = json!(r.author() == c.author() && live);
                v
            })
            .collect();
        let implementations: Vec<Value> = relations(RelationKind::Implements)
            .filter(|(_, _, t)| *t == id)
            .filter_map(|(r, from, _)| {
                let artifact = g.get(&from)?;
                let assessment = self.policy.assess(g, &from);
                Some(json!({
                    "artifact": slim_json(g, names, artifact),
                    "relation": r.id(),
                    "relation_retracted": g.is_retracted(&r.id()),
                    "evidence": evidence_json(g, names, &from, assessment.as_ref()),
                    "assessment": assessment,
                }))
            })
            .collect();
        let assessment = self.policy.assess(g, &id);
        let refinements: Vec<Value> = relations(RelationKind::Refines)
            .filter(|(r, f, t)| {
                stands(r, f)
                    && *t == id
                    && g.get(f).is_some_and(|f| f.author() != r.author() || intent_until(f).is_none())
            })
            .map(|(_, f, _)| short(&f))
            .collect();
        let mut rivals: Vec<Value> = relations(RelationKind::Contradicts)
            .filter(|(r, f, t)| (*f == id || *t == id) && f != t && stands(r, if *f == id { t } else { f }))
            .map(|(r, f, t)| {
                let mut v = short(if f == id { &t } else { &f });
                v["how"] = json!("contradicts");
                v["relation"] = json!(r.id());
                v
            })
            .collect();
        for conflict in open_conflicts(g) {
            let Conflict::Divergence { target, successors } = conflict else { continue };
            if !successors.contains(&id) {
                continue;
            }
            for s in successors.iter().filter(|s| **s != id) {
                let target_author = g.get(&target).map(|t| t.author());
                // The supersession that counts: by the target's own author.
                let relation = g.referrers(&target).find(|r| {
                    !g.is_retracted(&r.id())
                        && Some(r.author()) == target_author
                        && matches!(r.body(), Body::Relation { from, kind: RelationKind::Supersedes, to, .. } if from == s && *to == target)
                });
                let mut v = short(s);
                v["how"] = json!("divergent");
                v["relation"] = json!(relation.map(|r| r.id()));
                rivals.push(v);
            }
        }
        Ok(json!({
            "now_ms": now,
            "claim": claim,
            "lineage": {"replaces": replaces, "replaced_by": replaced_by},
            "implementations": implementations,
            "evidence": evidence_json(g, names, &id, assessment.as_ref()),
            "intents": all_intents_json(g, names, &view, &id, now),
            "refinements": refinements,
            "rivals": rivals,
        }))
    }

    /// The replica as a cursor saw it: a graph of only the cursor's ids and their ancestors, and its
    /// causal order. Building one checks every contribution again, so the last few are kept: a
    /// cursor's graph never changes, since a cursor names its ancestors and the Ground only grows.
    fn as_of(&self, snap: &Snapshot, cursor: &str) -> Result<Arc<Past>, Response> {
        let g = snap.graph();
        let seen = listing::seen(g, cursor).map_err(|e| error(400, format!("as_of: {e:#}")))?;
        let mut key: Vec<ContributionId> = seen.iter().copied().collect();
        key.sort();
        let key = blake3::hash(&key.iter().flat_map(|id| id.to_hex().into_bytes()).collect::<Vec<u8>>());
        // The lock is held only to look up and to insert, so a cached cursor never waits behind a build.
        let cached = {
            let cache = self.past.lock().unwrap_or_else(|p| p.into_inner());
            cache.iter().find(|(k, _)| *k == key).map(|(_, past)| Arc::clone(past))
        };
        if let Some(past) = cached {
            return Ok(past);
        }
        let mut past = Graph::new(snap.store.info().ground);
        let report = past.import(seen.iter().map(|id| g.get(id).expect("seen ids exist").clone()));
        if !report.rejected.is_empty() || !report.unresolved.is_empty() {
            return Err(error(500, "the replica at that cursor didn't rebuild"));
        }
        let order = past.topo_order();
        let past = Arc::new((past, order));
        let mut cache = self.past.lock().unwrap_or_else(|p| p.into_inner());
        if cache.iter().any(|(k, _)| *k == key) {
            return Ok(past);
        }
        if cache.len() >= PAST_GRAPHS {
            cache.remove(0);
        }
        cache.push((key, Arc::clone(&past)));
        Ok(past)
    }

    /// Section 1.6: a `contributions` event whenever the replica gains anything, and a `ping` every
    /// [`PING`]. With `since`, the first event carries what arrived after that cursor, and its
    /// `status_changed` is `null`: the stream can't know what the page saw, so it refetches.
    fn events(&self, stream: &TcpStream, since: Option<&str>) {
        if self.streams.fetch_add(1, Ordering::SeqCst) >= MAX_STREAMS {
            self.streams.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(stream, &error(503, "too many event streams"));
            return;
        }
        let _slot = Release(&self.streams);
        let (mut generation, mut last) = self.snapshot();
        let mut cursor = listing::cursor(last.graph());
        let mut first = None;
        if let Some(since) = since {
            let page = match self.page(&last, Some(since), None) {
                Ok(p) => p,
                Err(r) => {
                    let _ = respond(stream, &r);
                    return;
                }
            };
            if !page.ids.is_empty() {
                first = Some(json!({"graph": graph_json(&last, &page), "status_changed": null}));
            }
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{}Connection: close\r\n\r\n",
            security_headers()
        );
        let send = |text: &[u8]| -> io::Result<()> {
            let mut out = Timed::new(stream, Instant::now() + IO_TIMEOUT, None);
            out.write_all(text)?;
            out.flush()
        };
        if send(head.as_bytes()).is_err() {
            return;
        }
        if let Some(v) = first
            && send(&event("contributions", &v)).is_err()
        {
            return;
        }
        loop {
            let next = {
                let current = self.current.lock().unwrap_or_else(|p| p.into_inner());
                let (current, _) = self
                    .changed
                    .wait_timeout_while(current, PING, |c| c.0 == generation)
                    .unwrap_or_else(|p| p.into_inner());
                (current.0 != generation).then(|| (current.0, Arc::clone(&current.1)))
            };
            let text = match next {
                None => event("ping", &json!({"now_ms": now_ms()})),
                Some((g, snap)) => {
                    // The cursor's ids are in every later snapshot: the Ground only grows.
                    let page = self.page(&snap, Some(&cursor), None).unwrap_or_else(|_| Page {
                        ids: Vec::new(),
                        cursor: listing::cursor(snap.graph()),
                        more: false,
                    });
                    let changed = status_changes(&last, &snap);
                    generation = g;
                    cursor = listing::cursor(snap.graph());
                    let text = (!page.ids.is_empty() || !changed.is_empty()).then(|| {
                        event("contributions", &json!({"graph": graph_json(&snap, &page), "status_changed": changed}))
                    });
                    last = snap;
                    match text {
                        Some(t) => t,
                        None => continue,
                    }
                }
            };
            if send(&text).is_err() {
                return;
            }
        }
    }
}

/// Gives back a counted slot when dropped.
struct Release<'a>(&'a AtomicUsize);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn event(name: &str, data: &Value) -> Vec<u8> {
    let mut out = format!("event: {name}\ndata: ").into_bytes();
    out.extend(json_bytes(data));
    out.extend_from_slice(b"\n\n");
    out
}

/// Subjects whose status differs between two snapshots, with the new one. A subject missing from a
/// snapshot's statuses is pending.
fn status_changes(old: &Snapshot, new: &Snapshot) -> Map<String, Value> {
    let status = |s: &Snapshot, id: &ContributionId| s.statuses.get(id).map_or(Status::Pending, |a| a.status);
    old.statuses
        .keys()
        .chain(new.statuses.keys())
        .filter(|id| status(old, id) != status(new, id))
        .map(|id| (id.to_hex(), json!(status(new, id))))
        .collect()
}

/// One page of contributions, in causal order, and the cursor after it.
struct Page {
    ids: Vec<ContributionId>,
    cursor: String,
    more: bool,
}

/// The cursor after `page`, which follows everything `seen` holds in causal order: the heads of the
/// two together. A page that ends early covers a set closed under ancestry (a page's parents are in
/// it or seen), so its heads are a cursor `log --since` reads the same way.
fn cursor_after(g: &Graph, mut seen: HashSet<ContributionId>, page: &[ContributionId]) -> String {
    seen.extend(page.iter().copied());
    let parents: HashSet<ContributionId> =
        seen.iter().flat_map(|id| g.get(id).expect("seen ids exist").parents().iter().copied()).collect();
    let mut heads: Vec<String> = seen.iter().filter(|id| !parents.contains(id)).map(|id| id.to_hex()).collect();
    heads.sort();
    heads.join(",")
}

/// Positions in a columnar response: the response's own ids first, then `refs`, ids from earlier
/// responses (section 1.2).
struct Positions<'a> {
    index: HashMap<ContributionId, usize>,
    refs: Vec<&'a ContributionId>,
    ref_index: HashMap<ContributionId, usize>,
}

impl<'a> Positions<'a> {
    fn of(&mut self, id: &'a ContributionId) -> usize {
        if let Some(i) = self.index.get(id) {
            return *i;
        }
        let base = self.index.len();
        let next = self.refs.len();
        *self.ref_index.entry(*id).or_insert_with(|| {
            self.refs.push(id);
            next
        }) + base
    }
}

/// Section 1.2, for one page.
fn graph_json(snap: &Snapshot, page: &Page) -> Value {
    let g = snap.graph();
    let contributions: Vec<&Contribution> = page.ids.iter().map(|id| g.get(id).expect("paged ids exist")).collect();
    let mut at = Positions {
        index: page.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect(),
        refs: Vec::new(),
        ref_index: HashMap::new(),
    };
    let parents: Vec<Vec<usize>> =
        contributions.iter().map(|c| c.parents().iter().map(|p| at.of(p)).collect()).collect();
    // What's retracted now, of this page and of what this page's retractions target.
    let mut retracted: Vec<usize> = Vec::new();
    for c in &contributions {
        if g.is_retracted(&c.id()) {
            retracted.push(at.index[&c.id()]);
        }
        if let Body::Retract { target, .. } = c.body()
            && g.is_retracted(target)
        {
            retracted.push(at.of(target));
        }
    }
    retracted.sort();
    retracted.dedup();
    let tags: Vec<Vec<String>> = contributions
        .iter()
        .map(|c| match c.body() {
            Body::Claim { tags, .. } => tags.iter().map(|t| t.chars().take(TAG_CHARS).collect()).collect(),
            _ => Vec::new(),
        })
        .collect();
    json!({
        "cursor": page.cursor,
        "more": page.more,
        "ids": page.ids,
        "kind": contributions.iter().map(|c| c.body().kind()).collect::<Vec<_>>(),
        "author": contributions.iter().map(|c| snap.agent_index[&c.author()]).collect::<Vec<_>>(),
        "parents": parents,
        "time": contributions.iter().map(|c| c.content().timestamp_ms).collect::<Vec<_>>(),
        "depth": page.ids.iter().map(|id| snap.depth[id]).collect::<Vec<_>>(),
        "tags": tags,
        "refs": at.refs,
        "retracted": retracted,
    })
}

/// `log --since --json` for one page, with no artifact's text: that's in `/api/contribution`.
fn contributions_json(snap: &Snapshot, page: &Page) -> Value {
    let g = snap.graph();
    let rows: Vec<Value> =
        page.ids.iter().map(|id| slim_json(g, &snap.names, g.get(id).expect("paged ids exist"))).collect();
    json!({"contributions": rows, "cursor": page.cursor, "more": page.more})
}

/// A contribution as `log --json` prints it, without an artifact's text.
fn slim_json(g: &Graph, names: &Names, c: &Contribution) -> Value {
    let mut v = contribution_json(g, names, c);
    if let Some(body) = v["body"].as_object_mut() {
        body.remove("text");
    }
    v
}

/// A short entry (section 1.5): id, first line of the statement or the name, author, time, status.
fn short_json(g: &Graph, names: &Names, policy: &Policy, id: &ContributionId) -> Value {
    let Some(c) = g.get(id) else { return json!({"id": id}) };
    let text = match c.body() {
        Body::Claim { statement, .. } => statement.as_str(),
        Body::Artifact { name, .. } => name.as_str(),
        _ => "",
    };
    let first: String = text.lines().next().unwrap_or_default().chars().take(SHORT_CHARS).collect();
    json!({
        "id": id,
        "statement": first,
        "author_name": names.0.get(&c.author()),
        "timestamp_ms": c.content().timestamp_ms,
        "status": policy.assess(g, id).map(|a| a.status),
    })
}

/// The evidence on `target`, each with whether the policy counted it.
fn evidence_json(g: &Graph, names: &Names, target: &ContributionId, assessment: Option<&Assessment>) -> Vec<Value> {
    let counted = |id: &ContributionId| assessment.is_some_and(|a| a.passes.contains(id) || a.fails.contains(id));
    g.referrers(target)
        .filter_map(|e| match e.body() {
            Body::Evidence { target: t, verdict, method, .. } if t == target => Some(json!({
                "id": e.id(),
                "verdict": verdict,
                "method": method,
                "author": e.author(),
                "author_name": names.0.get(&e.author()),
                "timestamp_ms": e.content().timestamp_ms,
                "counted": counted(&e.id()),
                "retracted": g.is_retracted(&e.id()),
            })),
            _ => None,
        })
        .collect()
}

/// Every intent on `target`, live or not, with how and when each ended (section 1.5): `implemented`,
/// `expired`, `retracted`, or `superseded` (the intent or its target, by its own author). The same
/// rules as `live_intents`: an intent is live exactly when `ended_by` is null.
fn all_intents_json(g: &Graph, names: &Names, view: &CurrentView, target: &ContributionId, now: u64) -> Vec<Value> {
    // When a contribution was retracted: its earliest retraction that stands.
    let retracted_at = |id: &ContributionId| {
        g.referrers(id)
            .filter(|r| matches!(r.body(), Body::Retract { target, .. } if target == id) && !g.is_retracted(&r.id()))
            .map(|r| r.content().timestamp_ms)
            .min()
    };
    // When a contribution was superseded: its earliest supersession that counts.
    let superseded_at = |id: &ContributionId| {
        let author = g.get(id)?.author();
        g.referrers(id)
            .filter(|r| {
                matches!(r.body(), Body::Relation { kind: RelationKind::Supersedes, to, .. } if to == id)
                    && r.author() == author
                    && !g.is_retracted(&r.id())
            })
            .map(|r| r.content().timestamp_ms)
            .min()
    };
    let mut seen = HashSet::new();
    let mut intents: Vec<(u64, ContributionId, Value)> = Vec::new();
    for r in g.referrers(target) {
        let Body::Relation { from, kind: RelationKind::Refines, to, .. } = r.body() else { continue };
        let Some(claim) = g.get(from).filter(|c| to == target && c.author() == r.author()) else { continue };
        let Some((_, until_ms)) = intent_until(claim) else { continue };
        if !seen.insert(claim.id()) {
            continue;
        }
        let delivered = g
            .referrers(target)
            .filter(|d| {
                matches!(d.body(), Body::Relation { kind: RelationKind::Implements, to, .. } if to == target)
                    && d.author() == claim.author()
                    && !g.is_retracted(&d.id())
                    && !g.is_ancestor(&d.id(), &claim.id())
            })
            .map(|d| d.content().timestamp_ms)
            .min();
        // Hidden (so not live) when the intent, its relation or its target is retracted, or when the
        // intent or its target is superseded by its own author.
        let hidden = view.hidden.contains(&claim.id()) || view.hidden.contains(&r.id());
        let retracted = [claim.id(), r.id(), *target].iter().filter_map(retracted_at).min();
        let (ended_by, ended_ms) = if hidden && retracted.is_some() {
            (Some("retracted"), retracted)
        } else if hidden {
            (Some("superseded"), [claim.id(), *target].iter().filter_map(superseded_at).min())
        } else if let Some(at) = delivered {
            (Some("implemented"), Some(at))
        } else if until_ms <= now {
            (Some("expired"), Some(until_ms))
        } else {
            (None, None)
        };
        let v = json!({
            "id": claim.id(),
            "author": claim.author(),
            "author_name": names.0.get(&claim.author()),
            "from_ms": claim.content().timestamp_ms,
            "until_ms": until_ms,
            "ended_ms": ended_ms,
            "ended_by": ended_by,
        });
        intents.push((claim.content().timestamp_ms, claim.id(), v));
    }
    intents.sort_by_key(|(t, id, _)| (*t, *id));
    intents.into_iter().map(|(.., v)| v).collect()
}

/// Every live intent in the replica, by target, in causal order of the targets.
fn intents_json(snap: &Snapshot) -> Value {
    let g = snap.graph();
    let view = CurrentView::new(g, &snap.order);
    let now = now_ms();
    let rows: Vec<Value> = snap
        .order
        .iter()
        .filter(|id| !view.hidden.contains(id))
        .filter_map(|id| {
            let live = live_intents(g, &view, id, now);
            (!live.is_empty()).then(|| {
                let intents: Vec<Value> = live.iter().map(|i| intent_json(g, &snap.names, i, now)).collect();
                json!({"target": id, "intents": intents})
            })
        })
        .collect();
    json!(rows)
}

/// The id a path names: a unique prefix of at least [`PATH_ID_DIGITS`] digits (section 1.4), or a 404
/// with the CLI's message.
fn resolve(g: &Graph, prefix: &str) -> Result<ContributionId, Response> {
    if prefix.len() < PATH_ID_DIGITS {
        return Err(error(404, format!("use at least {PATH_ID_DIGITS} hex characters")));
    }
    g.resolve_prefix(prefix).map_err(|e| error(404, e.kind.to_string()))
}

/// The longest prefix of `s` of at most `max` bytes that ends on a character boundary.
fn cut_bytes(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_bytes_escapes_markup_without_changing_the_value() {
        let v = json!({"s": "</script><b>&\u{2028}\u{2029}\"\n"});
        let bytes = json_bytes(&v);
        let text = std::str::from_utf8(&bytes).expect("utf-8");
        for bad in ["<", ">", "&", "\u{2028}", "\u{2029}"] {
            assert!(!text.contains(bad), "{text}");
        }
        assert_eq!(serde_json::from_slice::<Value>(&bytes).expect("still JSON"), v);
    }

    #[test]
    fn cut_bytes_ends_on_a_character_boundary() {
        assert_eq!(cut_bytes("aé", 2), "a");
        assert_eq!(cut_bytes("aé", 3), "aé");
        assert_eq!(cut_bytes("ab", 9), "ab");
    }

    #[test]
    fn percent_decoding_reads_commas_and_refuses_bad_escapes() {
        assert_eq!(percent_decode("ab%2Ccd+e").ok().as_deref(), Some("ab,cd e"));
        assert!(percent_decode("%2").is_err() && percent_decode("%zz").is_err() && percent_decode("%ff").is_err());
    }
}
