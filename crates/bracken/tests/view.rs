//! `bracken view`, end to end on 127.0.0.1 against throwaway Grounds: loopback only, read-only,
//! hostile text JSON-escaped under the CSP on every response, paging and the cursor, the shapes of
//! docs/design/visualizer.md section 1, and the event stream.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_bracken");
/// A file to contribute as an artifact: small on every platform (a debug binary is over the 16 MiB
/// contribution limit on Linux).
const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; \
                   font-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const HOSTILE: &str = "</script><img src=x onerror=alert(1)> & \u{1b}[2K\r\n\u{2028}forged \"quoted\"";

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN).args(args).env("BRACKEN_DIR", dir).env_remove("BRACKEN_AGENT").output().unwrap();
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn json(dir: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&ok(dir, args)).unwrap()
}

/// A Ground with alice, bob and val.
fn ground() -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep().join("g");
    ok(&dir, &["init", "--name", "viewed"]);
    for who in ["alice", "bob", "val"] {
        ok(&dir, &["id", "new", who]);
    }
    dir
}

/// A running `bracken view`, stopped when dropped.
struct View {
    child: Child,
    port: u16,
    /// Where it listens, as the URL it printed has it: `127.0.0.1:<port>`, `[::1]:<port>`.
    at: String,
}

impl Drop for View {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn view(dir: &Path) -> View {
    view_with(dir, &[])
}

fn view_with(dir: &Path, env: &[(&str, &str)]) -> View {
    view_args(dir, &[], env).0
}

/// A view started with `args`, and the first line it wrote to stderr (empty if none came within a second).
fn view_args(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (View, String) {
    let mut child = Command::new(BIN)
        .arg("view")
        .args(args)
        .envs(env.iter().copied())
        .env("BRACKEN_DIR", dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    let _ = BufReader::new(child.stdout.take().unwrap()).read_line(&mut line);
    let at = line.trim().strip_prefix("view at http://").and_then(|p| p.strip_suffix('/')).map(str::to_owned);
    let Some((at, port)) = at.and_then(|at| Some((at.clone(), at.rsplit(':').next()?.parse::<u16>().ok()?))) else {
        // It didn't start: stop whatever was launched before failing, so no process outlives the test.
        let _ = child.kill();
        let _ = child.wait();
        panic!("bracken view {args:?} didn't start: {line:?}");
    };
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut first = String::new();
        let _ = BufReader::new(stderr).read_line(&mut first);
        let _ = tx.send(first);
    });
    let warning = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
    (View { child, port, at }, warning)
}

struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&self.body)))
    }
}

impl View {
    fn request(&self, method: &str, path: &str, host: &str) -> Reply {
        let mut s = TcpStream::connect(self.at.as_str()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        parse(&raw)
    }

    fn get(&self, path: &str) -> Reply {
        let reply = self.request("GET", path, &format!("127.0.0.1:{}", self.port));
        assert_eq!(reply.status, 200, "{path}: {}", String::from_utf8_lossy(&reply.body));
        reply
    }

    fn api(&self, path: &str) -> Value {
        self.get(path).json()
    }

    /// Opens the event stream and returns it once its head has arrived.
    fn events(&self, query: &str) -> BufReader<TcpStream> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "GET /api/events{query} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", self.port).unwrap();
        let mut r = BufReader::new(s);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            head.push_str(&line);
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        let reply = parse(head.as_bytes());
        assert_eq!(reply.status, 200, "{head}");
        assert_eq!(reply.headers["content-type"], "text/event-stream");
        assert_security_headers("/api/events", &reply);
        r
    }
}

fn parse(raw: &[u8]) -> Reply {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
    let head = String::from_utf8(raw[..end].to_vec()).unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers =
        lines.filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned())).collect();
    Reply { status, headers, body: raw.get(end + 4..).unwrap_or_default().to_vec() }
}

fn assert_security_headers(what: &str, r: &Reply) {
    assert_eq!(r.headers.get("content-security-policy").map(String::as_str), Some(CSP), "{what}: {:?}", r.headers);
    assert_eq!(r.headers.get("x-content-type-options").map(String::as_str), Some("nosniff"), "{what}");
    assert!(!r.headers.keys().any(|k| k.starts_with("access-control-")), "{what} has CORS: {:?}", r.headers);
}

/// Reads events until one named `name` arrives, and returns its data.
fn next_event(r: &mut BufReader<TcpStream>, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut current = None;
    while Instant::now() < deadline {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap() == 0 {
            panic!("the stream ended before a {name} event");
        }
        if let Some(n) = line.strip_prefix("event: ") {
            current = Some(n.trim().to_owned());
        } else if let Some(data) = line.strip_prefix("data: ")
            && current.as_deref() == Some(name)
        {
            return serde_json::from_str(data).unwrap();
        }
    }
    panic!("no {name} event in time");
}

/// Every file under `dir`, with its bytes.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn view_serves_on_127_0_0_1_only() {
    let dir = ground();
    // There's no option to listen anywhere else.
    let out = Command::new(BIN).args(["view", "--address", "0.0.0.0"]).env("BRACKEN_DIR", &dir).output().unwrap();
    assert!(!out.status.success(), "an --address option exists");
    let v = view(&dir);
    assert_eq!(v.get("/").headers["content-type"], "text/html; charset=utf-8");
    // Not reachable on this machine's own non-loopback address, where it has one. (Connecting a UDP
    // socket sends nothing; it only picks the address a route would use.)
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    if let Ok(own) = udp.connect("192.0.2.1:9").and_then(|()| udp.local_addr()).map(|a| a.ip())
        && !own.is_loopback()
        && !own.is_unspecified()
    {
        let reached = TcpStream::connect_timeout(&(own, v.port).into(), Duration::from_secs(2));
        assert!(reached.is_err(), "reachable on {own}");
    }
    // A page served from another name that resolves to 127.0.0.1 (DNS rebinding) is refused.
    for host in ["evil.example:80", &format!("evil.example:{}", v.port), "127.0.0.1:1", ""] {
        let r = v.request("GET", "/api/ground", host);
        assert_eq!(r.status, 403, "Host {host:?}");
        assert_security_headers("a refused Host", &r);
    }
    assert_eq!(v.request("GET", "/api/ground", &format!("localhost:{}", v.port)).status, 200);
}

#[test]
fn every_endpoint_is_read_only_and_carries_the_csp() {
    let dir = ground();
    let claim = ok(&dir, &["claim", HOSTILE, "--tag", HOSTILE, "--as", "alice"]);
    std::fs::write(dir.join("../f"), HOSTILE).unwrap();
    let art = ok(
        &dir,
        &["artifact", dir.join("../f").to_str().unwrap(), "--name", HOSTILE, "--implements", &claim, "--as", "bob"],
    );
    let art = art.lines().next().unwrap().to_owned();
    ok(&dir, &["evidence", &art, "pass", "--method", HOSTILE, "--details", HOSTILE, "--as", "val"]);
    ok(&dir, &["intend", &claim, "--as", "bob"]);
    let before = files(&dir);
    let log = ok(&dir, &["log", "--json"]);

    let v = view(&dir);
    let gets = [
        "/".to_owned(),
        "/loom.js".to_owned(),
        "/root.html".to_owned(),
        "/trust.html".to_owned(),
        "/api/trust".to_owned(),
        "/tension.html".to_owned(),
        "/runs.html".to_owned(),
        "/api/tension".to_owned(),
        format!("/api/tension?as_of={claim}"),
        "/loom.css".to_owned(),
        "/api/ground".to_owned(),
        "/api/graph".to_owned(),
        "/api/graph?since=&limit=1".to_owned(),
        "/api/contributions".to_owned(),
        "/api/status".to_owned(),
        "/api/conflicts".to_owned(),
        "/api/intents".to_owned(),
        format!("/api/contribution/{claim}"),
        format!("/api/contribution/{art}"),
        format!("/api/root/{claim}"),
        "/api/nothing".to_owned(),
        "/api/contribution/ffffffffffff".to_owned(),
        "/api/graph?since=ffffffffffff".to_owned(),
        "/api/graph?limit=0".to_owned(),
    ];
    for path in &gets {
        let r = v.request("GET", path, &format!("127.0.0.1:{}", v.port));
        assert_security_headers(path, &r);
        if path.starts_with("/api/") {
            assert_eq!(r.headers["content-type"], "application/json", "{path}");
            // Hostile text is JSON data only: no markup in the bytes, the text intact once parsed.
            for markup in *b"<>&" {
                assert!(!r.body.contains(&markup), "{path} sent a raw {:?}", markup as char);
            }
            r.json();
        }
    }
    let shown = v.api(&format!("/api/contribution/{claim}"));
    assert_eq!(shown["body"]["statement"], HOSTILE);
    assert_eq!(shown["body"]["tags"][0], HOSTILE);
    let listed = v.api("/api/contributions");
    assert!(listed["contributions"].as_array().unwrap().iter().any(|c| c["body"]["name"] == HOSTILE));
    // Only GET: nothing else is served, whatever the path.
    for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"] {
        for path in ["/api/ground", "/api/contributions", "/"] {
            let r = v.request(method, path, &format!("127.0.0.1:{}", v.port));
            assert_eq!(r.status, 405, "{method} {path}");
            assert_security_headers(&format!("{method} {path}"), &r);
        }
    }
    let mut events = v.events("?since=");
    next_event(&mut events, "contributions");
    drop(events);
    std::thread::sleep(Duration::from_millis(1500)); // a watcher poll or so
    drop(v);
    assert_eq!(files(&dir), before, "a session of every request changed the replica");
    assert_eq!(ok(&dir, &["log", "--json"]), log);
}

#[test]
fn paging_and_the_cursor_round_trip() {
    let dir = ground();
    let mut last = ok(&dir, &["claim", "root", "--as", "alice"]);
    for n in 0..6 {
        let who = ["alice", "bob"][n % 2];
        let next = ok(&dir, &["claim", &format!("c{n}"), "--as", who]);
        ok(&dir, &["relate", &next, "refines", &last, "--as", who]);
        last = next;
    }
    let all: Vec<String> = json(&dir, &["log", "--json"])
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect();
    let everything = json(&dir, &["log", "--since", "", "--json"]);
    let v = view(&dir);

    for endpoint in ["/api/contributions", "/api/graph"] {
        let (mut cursor, mut seen, mut known): (String, Vec<String>, Vec<String>) = (String::new(), vec![], vec![]);
        loop {
            let page = v.api(&format!("{endpoint}?since={cursor}&limit=3"));
            let ids: Vec<String> = if endpoint == "/api/graph" {
                let ids: Vec<String> =
                    page["ids"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_owned()).collect();
                let refs: Vec<&str> = page["refs"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
                // Parents index this page's ids, then refs, which earlier pages already gave.
                for (i, parents) in page["parents"].as_array().unwrap().iter().enumerate() {
                    for p in parents.as_array().unwrap() {
                        let p = p.as_u64().unwrap() as usize;
                        let parent = if p < ids.len() { ids[p].as_str() } else { refs[p - ids.len()] };
                        assert!(p < i || known.iter().any(|k| k == parent), "{endpoint}: parent {p} of {i}");
                    }
                }
                for r in &refs {
                    assert!(known.iter().any(|k| k == r), "ref {r} not seen before");
                }
                ids
            } else {
                page["contributions"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect()
            };
            assert!(ids.len() <= 3);
            known.extend(ids.iter().cloned());
            seen.extend(ids);
            cursor = page["cursor"].as_str().unwrap().to_owned();
            // The CLI reads the same cursor: what it lists after it is exactly what's left.
            let rest: Vec<String> = json(&dir, &["log", "--since", &cursor, "--json"])["contributions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(rest, all[seen.len()..], "{endpoint} after {}", seen.len());
            if !page["more"].as_bool().unwrap() {
                break;
            }
        }
        assert_eq!(seen, all, "{endpoint} pages, in causal order");
        assert_eq!(cursor, everything["cursor"].as_str().unwrap(), "{endpoint}: the last cursor is log's");
        let after = v.api(&format!("{endpoint}?since={cursor}"));
        assert_eq!(after["more"], false);
        assert_eq!(after.get("ids").or(after.get("contributions")).unwrap(), &serde_json::json!([]));
    }
    // Commas may come percent-encoded, as the page's encodeURIComponent writes them.
    let page = v.api("/api/contributions?limit=4");
    let encoded = page["cursor"].as_str().unwrap().replace(',', "%2C");
    assert_eq!(
        v.api(&format!("/api/contributions?since={encoded}"))["contributions"].as_array().unwrap().len(),
        all.len() - 4
    );
}

#[test]
fn the_api_has_the_design_shapes() {
    let dir = ground();
    let claim = ok(&dir, &["claim", "spec: parse dates\nsecond line", "--tag", "spec", "--as", "alice"]);
    let rival = ok(&dir, &["claim", "spec: never parse dates", "--as", "bob"]);
    let contra = ok(&dir, &["relate", &rival, "contradicts", &claim, "--as", "bob"]);
    let big = dir.join("../big");
    std::fs::write(&big, "é".repeat(40_000)).unwrap(); // 80,000 bytes of text
    let out = ok(&dir, &["artifact", big.to_str().unwrap(), "--name", "big.rs", "--implements", &claim, "--as", "bob"]);
    let (art, relation) = out.split_once('\n').unwrap();
    let pass = ok(&dir, &["evidence", art, "pass", "--method", "cargo test", "--as", "val"]);
    let fail = ok(&dir, &["evidence", &rival, "fail", "--method", "review", "--as", "val"]);
    ok(&dir, &["evidence", art, "pass", "--method", "self", "--as", "bob"]);
    let live = ok(&dir, &["intend", &claim, "--as", "alice"]);
    let (live, live_relation) = live.split_once('\n').unwrap();
    let done = ok(&dir, &["intend", &claim, "--as", "bob"]).lines().next().unwrap().to_owned();
    ok(&dir, &["relate", art, "implements", &claim, "--as", "bob"]);
    let gone = ok(&dir, &["intend", &claim, "--as", "val"]).lines().next().unwrap().to_owned();
    let cut = ok(&dir, &["retract", &gone, "--as", "val"]);
    let refinement = ok(&dir, &["claim", "spec: parse ISO dates", "--as", "val"]);
    let refines = ok(&dir, &["relate", &refinement, "refines", &claim, "--as", "val"]);
    let v = view(&dir);

    let ground = v.api("/api/ground");
    assert_eq!(ground["name"], "viewed");
    assert_eq!(ground["policy"], serde_json::json!({"min_passes": 1, "validators": []}));
    let agents = ground["agents"].as_array().unwrap();
    let names: Vec<&str> = agents.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["alice", "bob", "val"], "in order of first contribution");
    let total: u64 = agents.iter().map(|a| a["contributions"].as_u64().unwrap()).sum();
    assert_eq!(total, ground["contributions"].as_u64().unwrap());
    assert!(agents.iter().all(|a| a["first_ms"].as_u64() <= a["last_ms"].as_u64()));
    assert_eq!(ground["cursor"], json(&dir, &["log", "--since", "", "--json"])["cursor"]);

    let graph = v.api("/api/graph");
    let ids: Vec<&str> = graph["ids"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
    let at = |id: &str| ids.iter().position(|i| *i == id).unwrap();
    assert_eq!(graph["refs"], serde_json::json!([]));
    assert_eq!(graph["kind"][at(&contra)], "relation");
    assert_eq!(graph["author"][at(&claim)], 0);
    assert_eq!(graph["author"][at(&pass)], 2);
    assert_eq!(graph["tags"][at(&claim)], serde_json::json!(["spec"]));
    assert_eq!(graph["depth"][at(ids[0])], 0);
    assert!(graph["retracted"].as_array().unwrap().contains(&serde_json::json!(at(&gone))));
    assert_eq!(graph["failed"], serde_json::json!([at(&fail)]), "failing evidence, so the page draws it");
    assert_eq!(graph["retracts"], serde_json::json!([[at(&cut), at(&gone)]]), "each retraction and its target");
    for (i, parents) in graph["parents"].as_array().unwrap().iter().enumerate() {
        for p in parents.as_array().unwrap() {
            assert!((p.as_u64().unwrap() as usize) < i, "parents come first");
            assert!(graph["depth"][i].as_u64() > graph["depth"][p.as_u64().unwrap() as usize].as_u64());
        }
    }

    let status = v.api("/api/status");
    assert_eq!(status["statuses"][art]["status"], "accepted");
    assert_eq!(status["statuses"][art]["passes"], 1, "bob's own pass doesn't count");
    assert!(status["statuses"].get(&claim).is_none(), "pending with no evidence");
    assert_eq!(status["conflicts"], json(&dir, &["accepted", "--json"])["conflicts"]);

    let shown = v.api(&format!("/api/contribution/{}", &art[..10]));
    assert_eq!(shown["body"]["text_truncated"], true);
    assert_eq!(shown["body"]["text"].as_str().unwrap().len(), 64 * 1024);
    let r = v.request("GET", "/api/contribution/ffffffffffff", &format!("127.0.0.1:{}", v.port));
    assert_eq!((r.status, r.json()), (404, serde_json::json!({"error": "no matching contribution"})));

    let root = v.api(&format!("/api/root/{claim}"));
    assert_eq!(root["claim"]["id"], claim.as_str());
    let imp = &root["implementations"][0];
    assert_eq!(root["implementations"].as_array().unwrap().len(), 2, "both implements relations");
    assert_eq!(imp["artifact"]["id"], art);
    assert!(imp["artifact"]["body"].get("text").is_none());
    let relations: Vec<&str> =
        root["implementations"].as_array().unwrap().iter().map(|i| i["relation"].as_str().unwrap()).collect();
    assert!(relations.contains(&relation), "{relations:?}");
    assert_eq!(imp["assessment"]["status"], "accepted");
    let counted: Vec<(String, bool)> = imp["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["author_name"].as_str().unwrap().to_owned(), e["counted"].as_bool().unwrap()))
        .collect();
    assert!(counted.contains(&("val".into(), true)) && counted.contains(&("bob".into(), false)), "{counted:?}");
    let intents: BTreeMap<&str, (&Value, &Value)> = root["intents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["author_name"].as_str().unwrap(), (&i["ended_by"], &i["id"])))
        .collect();
    assert_eq!(intents["alice"], (&Value::Null, &serde_json::json!(live)));
    assert_eq!(intents["bob"], (&serde_json::json!("implemented"), &serde_json::json!(done)));
    assert_eq!(intents["val"].0, "retracted");
    let refinements: Vec<&str> =
        root["refinements"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(refinements, [refinement.as_str()], "intents aren't refinements");
    // Each carries its refines relation, so a replay stop can bring it in with the claim.
    assert_eq!(root["refinements"][0]["relation"], refines.as_str());
    let alice = root["intents"].as_array().unwrap().iter().find(|i| i["author_name"] == "alice").unwrap();
    assert_eq!(alice["relation"], live_relation);
    assert_eq!(root["rivals"][0]["id"], rival.as_str());
    assert_eq!(root["rivals"][0]["how"], "contradicts");
    assert_eq!(root["rivals"][0]["relation"], contra.as_str());
    let claim_short = v.api(&format!("/api/root/{rival}"))["rivals"][0]["statement"].clone();
    assert_eq!(claim_short, "spec: parse dates", "short entries are the first line");

    let intents = v.api("/api/intents");
    assert_eq!(intents[0]["target"], claim.as_str());
    assert_eq!(intents[0]["intents"][0]["id"], live);
}

#[test]
fn events_announce_what_arrives() {
    let dir = ground();
    let claim = ok(&dir, &["claim", "spec", "--as", "alice"]);
    let art = ok(&dir, &["artifact", FILE, "--name", "bin", "--implements", &claim, "--as", "bob"]);
    let art = art.lines().next().unwrap().to_owned();
    let v = view(&dir);
    let cursor = v.api("/api/ground")["cursor"].as_str().unwrap().to_owned();
    let mut events = v.events("");
    let pass = ok(&dir, &["evidence", &art, "pass", "--method", "m", "--as", "val"]);
    let e = next_event(&mut events, "contributions");
    assert_eq!(e["graph"]["ids"], serde_json::json!([pass]));
    // Its parents are from before, so they come by id in refs.
    let refs = e["graph"]["refs"].as_array().unwrap();
    assert!(!refs.is_empty());
    for p in e["graph"]["parents"][0].as_array().unwrap() {
        assert!(p.as_u64().unwrap() >= 1 && (p.as_u64().unwrap() as usize) <= refs.len(), "{e}");
    }
    assert_eq!(e["status_changed"], serde_json::json!({art.clone(): "accepted"}));
    // A page reconnecting with its old cursor gets what it missed first.
    let mut again = v.events(&format!("?since={cursor}"));
    let e = next_event(&mut again, "contributions");
    assert_eq!(e["graph"]["ids"], serde_json::json!([pass]));
    assert_eq!(e["status_changed"], Value::Null);
}

/// `/api/root`'s lineage keeps supersessions that count (which hide the claim they replace) and marks
/// those that don't, every short entry carries its time, and `?as_of=` answers as the replica stood
/// at a cursor: assessments, counted evidence and intents' ends worked out over only what it saw.
#[test]
fn root_lineage_and_as_of() {
    let dir = ground();
    let old = ok(&dir, &["claim", "spec v1", "--as", "alice"]);
    let new = ok(&dir, &["claim", "spec v2", "--supersedes", &old, "--as", "alice"]).lines().next().unwrap().to_owned();
    let other = ok(&dir, &["claim", "spec v2, bob's", "--as", "bob"]);
    ok(&dir, &["relate", &other, "supersedes", &old, "--as", "bob"]);

    let spec = ok(&dir, &["claim", "spec: parse dates", "--as", "alice"]);
    let cursor = |v: &View| v.api("/api/ground")["cursor"].as_str().unwrap().to_owned();
    let v = view(&dir);
    let before = cursor(&v);
    // The intent's cursor is its refines relation: the claim alone isn't an intent yet.
    let intent = ok(&dir, &["intend", &spec, "--as", "bob"]).lines().nth(1).unwrap().to_owned();
    let out = ok(&dir, &["artifact", FILE, "--name", "bin", "--implements", &spec, "--as", "bob"]);
    let art = out.lines().next().unwrap().to_owned();
    let pass = ok(&dir, &["evidence", &art, "pass", "--method", "m", "--as", "val"]);
    drop(v);
    let v = view(&dir);

    let root = v.api(&format!("/api/root/{old}"));
    let by: BTreeMap<&str, (bool, u64)> = root["lineage"]["replaced_by"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["id"].as_str().unwrap(), (e["counts"].as_bool().unwrap(), e["timestamp_ms"].as_u64().unwrap())))
        .collect();
    assert_eq!(by.keys().copied().collect::<Vec<_>>().len(), 2, "{root}");
    assert!(by[new.as_str()].0 && !by[other.as_str()].0, "only alice's own supersession counts: {by:?}");
    assert!(by.values().all(|(_, t)| *t > 0));
    let replaces = v.api(&format!("/api/root/{new}"))["lineage"]["replaces"].clone();
    assert_eq!(replaces[0]["id"], old.as_str());
    assert_eq!(replaces[0]["statement"], "spec v1");

    // The life of spec, by cursor: before the intent, with it, delivered, accepted.
    let at = |cursor: &str| v.api(&format!("/api/root/{spec}?as_of={}", cursor.replace(',', "%2C")));
    let ended = |root: &Value| root["intents"][0]["ended_by"].clone();
    let r = at(&before);
    assert_eq!((r["intents"].as_array().unwrap().len(), r["implementations"].as_array().unwrap().len()), (0, 0));
    let r = at(&intent);
    assert_eq!(r["intents"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!((ended(&r), r["implementations"].as_array().unwrap().len()), (Value::Null, 0), "{r}");
    let delivered = json(&dir, &["log", "--json"]).as_array().unwrap().iter().rev().nth(1).unwrap()["id"].clone();
    let r = at(delivered.as_str().unwrap());
    assert_eq!(r["intents"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(ended(&r), "implemented");
    assert_eq!(r["implementations"][0]["assessment"]["status"], "pending");
    assert_eq!(r["implementations"][0]["evidence"], serde_json::json!([]));
    let r = at(&pass);
    assert_eq!(r["implementations"][0]["assessment"]["status"], "accepted");
    assert_eq!(r["implementations"][0]["evidence"][0]["counted"], true);
    let mut now = v.api(&format!("/api/root/{spec}"));
    let mut then = r.clone();
    now["now_ms"] = Value::Null;
    then["now_ms"] = Value::Null;
    assert_eq!(then, now, "as of the latest cursor is now");
    // Before it existed, and a cursor this replica doesn't know.
    let unknown = format!("127.0.0.1:{}", v.port);
    let r = v.request("GET", &format!("/api/root/{spec}?as_of={old}"), &unknown);
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
    let r = v.request("GET", &format!("/api/root/{spec}?as_of=ffffffffffff"), &unknown);
    assert_eq!(r.status, 400);
    assert_security_headers("as_of", &r);
}

/// An unknown or malformed `since` or `as_of` is a 400, never ignored (security b455229a7512), and so is a
/// parameter a page doesn't take, or one given twice.
#[test]
fn bad_parameters_are_refused_not_ignored() {
    let dir = ground();
    let spec = ok(&dir, &["claim", "spec", "--as", "alice"]);
    let v = view(&dir);
    let host = format!("127.0.0.1:{}", v.port);
    let bad_cursors = ["ffffffffffff", "zz", "abc", &spec[..3], &format!("{spec},ffffffffffff"), "%zz"];
    let mut refused = Vec::new();
    for c in bad_cursors {
        refused.push(format!("/api/graph?since={c}"));
        refused.push(format!("/api/contributions?since={c}"));
        refused.push(format!("/api/events?since={c}"));
        refused.push(format!("/api/root/{spec}?as_of={c}"));
    }
    refused.extend([
        format!("/api/root/{spec}?as_of="),
        format!("/api/root/{spec}?as_of=,"),
        format!("/api/root/{spec}?since={spec}"),
        format!("/api/contribution/{spec}?as_of={spec}"),
        format!("/api/graph?since={spec}&since={spec}"),
        "/api/graph?limit=-1".to_owned(),
        "/api/graph?limit=x".to_owned(),
        "/api/graph?cursor=x".to_owned(),
        "/api/status?as_of=x".to_owned(),
        "/api/ground?since=".to_owned(),
        "/api/events?as_of=x".to_owned(),
    ]);
    for path in &refused {
        let r = v.request("GET", path, &host);
        assert_eq!(r.status, 400, "{path}: {}", String::from_utf8_lossy(&r.body));
        assert!(r.json()["error"].is_string(), "{path}");
        assert_security_headers(path, &r);
    }
    for path in [format!("/api/contribution/{}", &spec[..7]), format!("/api/root/{}", &spec[..7])] {
        assert_eq!(v.request("GET", &path, &host).status, 404, "{path}");
    }
    // What's well formed still works: an empty since is everything, as for log --since.
    for path in
        ["/api/graph?since=", &format!("/api/root/{spec}?as_of={spec}"), &format!("/api/graph?since={}", &spec[..8])]
    {
        assert_eq!(v.request("GET", path, &host).status, 200, "{path}");
    }
}

/// The replica's count is taken before it's read (finding 31f5a44fc779): a write while the server
/// re-reads still shows. The server waits 1.5 s after each read here, so the second write lands
/// inside a re-read, deterministically.
#[test]
fn a_write_during_a_reread_still_shows() {
    let dir = ground();
    let v = view_with(&dir, &[("BRACKEN_VIEW_REREAD_DELAY_MS", "1500")]);
    for round in 0..2 {
        ok(&dir, &["claim", &format!("a{round}"), "--as", "alice"]);
        // The watcher polls each second, so by now it has seen the first write and is mid re-read.
        std::thread::sleep(Duration::from_millis(1200));
        ok(&dir, &["claim", &format!("b{round}"), "--as", "bob"]);
        let want = json(&dir, &["log", "--json"]).as_array().unwrap().len() as u64;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let have = v.api("/api/ground")["contributions"].as_u64().unwrap();
            if have == want {
                break;
            }
            assert!(Instant::now() < deadline, "round {round}: the view stayed at {have} of {want}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Every asset is served with the CSP and its media type: each file the binary embeds (the page, its
/// scripts and styles, the fonts and their licence), and everything the page and its styles name.
#[test]
fn every_asset_is_served_with_the_csp() {
    let dir = ground();
    let v = view(&dir);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/view");
    let mut paths = vec!["/".to_owned()];
    for (sub, prefix) in [("", "/"), ("fonts", "/fonts/")] {
        for e in std::fs::read_dir(root.join(sub)).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_str().unwrap().to_owned();
            if p.is_file() && name != "index.html" {
                paths.push(format!("{prefix}{name}"));
            }
        }
    }
    // What the pages and their styles load, so nothing they ask for is missing.
    let mut named: Vec<String> = Vec::new();
    for page in ["/", "/root.html", "/trust.html", "/tension.html", "/runs.html"] {
        let page = String::from_utf8(v.get(page).body).unwrap();
        for attr in ["src=\"", "href=\""] {
            for part in page.split(attr).skip(1) {
                named.push(part.split('"').next().unwrap().to_owned());
            }
        }
    }
    for css in ["/visualizer.css", "/loom.css", "/root.css", "/trust.css", "/tension.css", "/runs.css"] {
        let text = String::from_utf8(v.get(css).body).unwrap();
        for part in text.split("url(\"").skip(1) {
            named.push(part.split('"').next().unwrap().to_owned());
        }
    }
    named.retain(|n| !n.starts_with("data:"));
    assert!(named.len() >= 7, "{named:?}");
    for n in &named {
        assert!(n.starts_with('/') && !n.starts_with("//"), "{n} isn't the view's own");
        assert!(paths.contains(n), "{n} is named but not embedded");
    }
    let media = |p: &str| match p.rsplit('.').next().unwrap() {
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "text/html; charset=utf-8",
    };
    for p in &paths {
        let r = v.get(p);
        assert_security_headers(p, &r);
        assert_eq!(r.headers["content-type"], media(p), "{p}");
        assert!(!r.body.is_empty(), "{p}");
    }
}

/// Two agent entries that give one name to different keys: neither key is shown by that name, so one
/// agent can't pass as another, and the name a key also has elsewhere still shows.
#[test]
fn a_name_two_keys_claim_names_neither() {
    let dir = ground();
    ok(&dir, &["claim", "spec", "--as", "alice"]);
    ok(&dir, &["claim", "spec too", "--as", "val"]);
    let val = std::fs::read_to_string(dir.join("agents/val.json")).unwrap();
    let val: Value = serde_json::from_str(&val).unwrap();
    let forged = serde_json::json!({"name": "alice", "agent": val["agent"]});
    std::fs::write(dir.join("agents/mallory.json"), forged.to_string()).unwrap();
    let v = view(&dir);
    let names: Vec<Value> =
        v.api("/api/ground")["agents"].as_array().unwrap().iter().map(|a| a["name"].clone()).collect();
    assert_eq!(names, [Value::Null, serde_json::json!("val")]);
}

/// `/api/trust` counts as the CLI does: for every subject, `show --json`'s evidence that stands (by its
/// author, on the subject's author) and the assessment's counted passes and fails, summed per pair.
#[test]
fn trust_agrees_with_show() {
    let dir = ground();
    ok(&dir, &["id", "new", "maya"]);
    let claim = ok(&dir, &["claim", "spec", "--as", "alice"]);
    let other = ok(&dir, &["claim", "another spec", "--as", "bob"]);
    std::fs::write(dir.join("../f"), "fn main() {}").unwrap();
    let art = ok(
        &dir,
        &["artifact", dir.join("../f").to_str().unwrap(), "--name", "main.rs", "--implements", &claim, "--as", "bob"],
    );
    let art = art.lines().next().unwrap().to_owned();
    ok(&dir, &["evidence", &art, "pass", "--method", "cargo test", "--as", "val"]);
    ok(&dir, &["evidence", &art, "pass", "--method", "self", "--as", "bob"]);
    ok(&dir, &["evidence", &art, "fail", "--method", "review", "--as", "maya"]);
    ok(&dir, &["evidence", &claim, "pass", "--method", "acceptance", "--as", "val"]);
    ok(&dir, &["evidence", &other, "pass", "--method", "read it", "--as", "alice"]);
    let withdrawn = ok(&dir, &["evidence", &other, "fail", "--method", "too hasty", "--as", "maya"]);
    ok(&dir, &["retract", &withdrawn, "--as", "maya"]);
    // Evidence on a relation counts toward the relation's author.
    let link = ok(&dir, &["relate", &other, "refines", &claim, "--as", "alice"]);
    ok(&dir, &["evidence", link.lines().next().unwrap(), "pass", "--method", "read both", "--as", "maya"]);
    let v = view(&dir);

    let agents: Vec<String> = v.api("/api/ground")["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["agent"].as_str().unwrap().to_owned())
        .collect();
    let mut want: BTreeMap<(String, String), [u64; 4]> = BTreeMap::new();
    for c in json(&dir, &["log", "--json"]).as_array().unwrap() {
        if !matches!(c["kind"].as_str(), Some("claim" | "artifact" | "relation")) {
            continue;
        }
        let shown = json(&dir, &["show", c["id"].as_str().unwrap(), "--json"]);
        // A subject `show --json` gives no assessment for counts nothing.
        let counted = |list: &str, id: &Value| shown["assessment"][list].as_array().is_some_and(|l| l.contains(id));
        for r in shown["referrers"].as_array().unwrap() {
            if r["kind"] != "evidence" || r["retracted"] == true || r["body"]["target"] != c["id"] {
                continue;
            }
            let e = want
                .entry((r["author"].as_str().unwrap().to_owned(), c["author"].as_str().unwrap().to_owned()))
                .or_default();
            if r["body"]["verdict"] == "pass" {
                e[0] += 1;
                e[2] += u64::from(counted("passes", &r["id"]));
            } else {
                e[1] += 1;
                e[3] += u64::from(counted("fails", &r["id"]));
            }
        }
    }
    let trust = v.api("/api/trust");
    let have: BTreeMap<(String, String), [u64; 4]> = trust["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let n = |k: &str| e[k].as_u64().unwrap();
            let who = |k: &str| agents[e[k].as_u64().unwrap() as usize].clone();
            ((who("from"), who("to")), [n("passes"), n("fails"), n("counted_passes"), n("counted_fails")])
        })
        .collect();
    assert_eq!(have, want);
    assert!(want.values().any(|e| e[0] > e[2]), "the sample has an uncounted pass (bob's own)");
    assert!(want.values().any(|e| e[3] > 0) && want.len() >= 4, "{want:?}");
}

/// The Tension map's springs and divergences are what `conflicts --json` gives, and every end's
/// assessment is `show --json`'s: a finding against a pr, its triage pass, the supersession that clears
/// it, a withdrawn contradiction, and a divergence. As of before the supersession, the first is open.
#[test]
fn tension_agrees_with_conflicts_and_show() {
    let dir = ground();
    ok(&dir, &["id", "new", "maya"]);
    let first = |out: String| out.lines().next().unwrap().to_owned();
    let pr1 = ok(&dir, &["claim", "PR #1 parse @ aaa", "--tag", "pr", "--as", "alice"]);
    let f1 = ok(&dir, &["claim", "finding: parse drops the last line", "--tag", "finding", "--as", "bob"]);
    let early = ok(&dir, &["relate", &f1, "contradicts", &pr1, "--as", "bob"]);
    ok(&dir, &["evidence", &f1, "pass", "--method", "triage", "--as", "maya"]);
    let pr2 = ok(&dir, &["claim", "PR #1 parse @ bbb", "--tag", "pr", "--as", "alice"]);
    ok(&dir, &["relate", &pr2, "supersedes", &pr1, "--as", "alice"]);
    let f2 = ok(&dir, &["claim", "finding: parse is slow", "--tag", "finding", "--as", "val"]);
    let open = ok(&dir, &["relate", &f2, "contradicts", &pr2, "--as", "val"]);
    let f3 = ok(&dir, &["claim", "finding: parse panics", "--as", "bob"]);
    let withdrawn = ok(&dir, &["relate", &f3, "contradicts", &pr2, "--as", "bob"]);
    ok(&dir, &["retract", &withdrawn, "--as", "bob"]);
    let t = ok(&dir, &["claim", "the plan", "--as", "maya"]);
    for who in ["x", "y"] {
        let s = first(ok(&dir, &["claim", &format!("the plan, {who}"), "--as", "maya"]));
        ok(&dir, &["relate", &s, "supersedes", &t, "--as", "maya"]);
    }
    let v = view(&dir);

    let tension = v.api("/api/tension");
    let springs = tension["springs"].as_array().unwrap();
    let by_relation = |id: &str| springs.iter().find(|s| s["relation"] == id).unwrap();
    let conflicts = json(&dir, &["conflicts", "--json"]);
    let mut want_open: Vec<&str> = conflicts
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["type"] == "contradiction")
        .map(|c| c["relation"].as_str().unwrap())
        .collect();
    let mut have_open: Vec<&str> =
        springs.iter().filter(|s| s["open"] == true).map(|s| s["relation"].as_str().unwrap()).collect();
    want_open.sort();
    have_open.sort();
    assert_eq!(have_open, want_open);
    assert_eq!(have_open, [open.as_str()]);
    assert_eq!(springs.len(), 3, "every contradicts relation, open or not");
    for s in springs {
        for end in ["from", "to"] {
            let id = s[end]["id"].as_str().unwrap();
            assert_eq!(s[end]["assessment"], json(&dir, &["show", id, "--json"])["assessment"], "{id}");
        }
    }
    let cleared = by_relation(&early);
    assert_eq!(cleared["to"]["replaced_by"][0]["id"], pr2.as_str(), "the supersession clears it");
    let triage = &cleared["from"]["evidence"][0];
    assert_eq!((triage["method"].as_str(), triage["counted"].as_bool()), (Some("triage"), Some(true)));
    assert_eq!(by_relation(&withdrawn)["retracted"], true);
    let divergences: Vec<(Value, Vec<Value>)> = tension["divergences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (d["target"]["id"].clone(), d["successors"].as_array().unwrap().iter().map(|s| s["id"].clone()).collect())
        })
        .collect();
    let want: Vec<(Value, Vec<Value>)> = conflicts
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["type"] == "divergence")
        .map(|c| (c["target"].clone(), c["successors"].as_array().unwrap().clone()))
        .collect();
    assert_eq!((divergences.len(), &divergences), (1, &want));

    // As the replica stood when the first finding's contradiction arrived, it was open.
    let then = v.api(&format!("/api/tension?as_of={early}"));
    let springs = then["springs"].as_array().unwrap();
    assert_eq!(springs.len(), 1);
    assert_eq!(
        (springs[0]["open"].as_bool(), springs[0]["to"]["replaced_by"].as_array().map(Vec::len)),
        (Some(true), Some(0))
    );
}

/// A key published under two names shows by the one the CLI picks, so the view and `log --json` agree.
#[test]
fn a_key_with_two_names_shows_as_the_cli_shows_it() {
    let dir = ground();
    ok(&dir, &["claim", "spec", "--as", "bob"]);
    let bob = std::fs::read_to_string(dir.join("agents/bob.json")).unwrap();
    let bob: Value = serde_json::from_str(&bob).unwrap();
    std::fs::write(dir.join("agents/zed.json"), serde_json::json!({"name": "zed", "agent": bob["agent"]}).to_string())
        .unwrap();
    let cli = json(&dir, &["log", "--json"])[0]["author_name"].clone();
    let v = view(&dir);
    assert_eq!(v.api("/api/ground")["agents"][0]["name"], cli);
    assert_eq!(cli, "zed");
}

/// `BRACKEN_VIEW_REREAD_DELAY_MS` is capped: a huge value still lets a write show within a few polls.
#[test]
fn the_reread_delay_is_capped() {
    let dir = ground();
    let v = view_with(&dir, &[("BRACKEN_VIEW_REREAD_DELAY_MS", "600000")]);
    ok(&dir, &["claim", "spec", "--as", "alice"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while v.api("/api/ground")["contributions"] != 1 {
        assert!(Instant::now() < deadline, "the write didn't show: the delay isn't capped");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Names come from the public agent directory: the view starts and names every agent with
/// `identities/`, where the private keys are, unreadable.
#[cfg(unix)]
#[test]
fn the_view_never_reads_private_keys() {
    use std::os::unix::fs::PermissionsExt;
    let dir = ground();
    ok(&dir, &["claim", "spec", "--as", "alice"]);
    let identities = dir.join("identities");
    std::fs::set_permissions(&identities, std::fs::Permissions::from_mode(0o000)).unwrap();
    let names: Vec<String> = {
        let v = view(&dir);
        v.api("/api/ground")["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect()
    };
    std::fs::set_permissions(&identities, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(names, ["alice"]);
}

/// Paging while the Ground grows: every page's cursor picks up after it, so nothing repeats, nothing
/// is missed, and every contribution comes after its parents.
#[test]
fn paging_while_the_ground_grows() {
    let dir = ground();
    for n in 0..4 {
        ok(&dir, &["claim", &format!("seed {n}"), "--as", "alice"]);
    }
    let v = view(&dir);
    let count = |v: &View| v.api("/api/ground")["contributions"].as_u64().unwrap();
    let mut cursor = String::new();
    let mut seen: Vec<String> = Vec::new();
    for n in 0..8 {
        let page = v.api(&format!("/api/graph?since={}&limit=2", cursor.replace(',', "%2C")));
        let ids: Vec<String> = page["ids"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_owned()).collect();
        let refs: Vec<&str> = page["refs"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
        for (i, parents) in page["parents"].as_array().unwrap().iter().enumerate() {
            for p in parents.as_array().unwrap() {
                let p = p.as_u64().unwrap() as usize;
                let parent = if p < ids.len() { ids[p].as_str() } else { refs[p - ids.len()] };
                assert!(p < i || seen.iter().any(|s| s == parent), "page {n}: {parent} came after its child");
            }
        }
        seen.extend(ids);
        cursor = page["cursor"].as_str().unwrap().to_owned();
        // Something new arrives between pages, from either author.
        let want = count(&v) + 1;
        ok(&dir, &["claim", &format!("grown {n}"), "--as", ["alice", "bob"][n % 2]]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while count(&v) < want {
            assert!(Instant::now() < deadline, "the view didn't pick up a write");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    loop {
        let page = v.api(&format!("/api/graph?since={}&limit=2", cursor.replace(',', "%2C")));
        seen.extend(page["ids"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_owned()));
        cursor = page["cursor"].as_str().unwrap().to_owned();
        if !page["more"].as_bool().unwrap() {
            break;
        }
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a contribution came twice");
    let mut all: Vec<String> = json(&dir, &["log", "--json"])
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect();
    all.sort();
    assert_eq!(unique, all, "every contribution, once");
}

/// Root follows the protocol on what counts: an intent on a target its author superseded ended as
/// `superseded`, not `retracted`; a retracted successor's supersession doesn't count; and a divergent
/// rival's relation is the supersession that counts, not someone else's.
#[test]
fn root_ends_and_counts_follow_the_protocol() {
    let dir = ground();
    let target = ok(&dir, &["claim", "spec v1", "--as", "alice"]);
    ok(&dir, &["intend", &target, "--as", "bob"]);
    let gone = ok(&dir, &["claim", "spec v1.5", "--supersedes", &target, "--as", "alice"]);
    let gone = gone.lines().next().unwrap().to_owned();
    ok(&dir, &["retract", &gone, "--as", "alice"]);
    let x = ok(&dir, &["claim", "x", "--as", "alice"]);
    let y1 = ok(&dir, &["claim", "y1", "--supersedes", &x, "--as", "alice"]).lines().next().unwrap().to_owned();
    let y2 = ok(&dir, &["claim", "y2", "--as", "alice"]);
    // bob's supersession of x comes first and doesn't count; alice's does.
    let bobs = ok(&dir, &["relate", &y2, "supersedes", &x, "--as", "bob"]);
    let alices = ok(&dir, &["relate", &y2, "supersedes", &x, "--as", "alice"]);
    let v = view(&dir);

    let root = v.api(&format!("/api/root/{target}"));
    let by = root["lineage"]["replaced_by"].as_array().unwrap();
    let gone_entry = by.iter().find(|e| e["id"] == gone.as_str()).unwrap_or_else(|| panic!("{root}"));
    assert_eq!(gone_entry["counts"], false, "a retracted successor doesn't count");
    assert_eq!(root["intents"][0]["ended_by"], Value::Null, "the supersession was retracted, so it's live");

    let real =
        ok(&dir, &["claim", "spec v2", "--supersedes", &target, "--as", "alice"]).lines().next().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    let root = loop {
        let root = v.api(&format!("/api/root/{target}"));
        if root["lineage"]["replaced_by"].as_array().unwrap().iter().any(|e| e["id"] == real.as_str()) {
            break root;
        }
        assert!(Instant::now() < deadline, "the view didn't pick up the supersession");
        std::thread::sleep(Duration::from_millis(100));
    };
    let intent = &root["intents"][0];
    assert_eq!(intent["ended_by"], "superseded", "{intent}");
    assert!(intent["ended_ms"].as_u64().is_some(), "{intent}");

    let rivals = v.api(&format!("/api/root/{y1}"))["rivals"].clone();
    let rival =
        rivals.as_array().unwrap().iter().find(|r| r["id"] == y2.as_str()).unwrap_or_else(|| panic!("{rivals}"));
    assert_eq!(rival["how"], "divergent");
    assert_eq!(rival["relation"], alices.as_str(), "not bob's {bobs}");
}

/// Starts `bracken view` with `args` and returns its stderr, failing if it starts serving instead of
/// refusing.
fn view_refuses(dir: &Path, args: &[&str]) -> String {
    let mut child = Command::new(BIN)
        .arg("view")
        .args(args)
        .env("BRACKEN_DIR", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("bracken view {args:?} started serving");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success(), "{args:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn view_refuses_with(dir: &Path, args: &[&str], vars: &[(&str, &str)]) -> String {
    let out = Command::new(BIN)
        .arg("view")
        .args(args)
        .envs(vars.iter().copied())
        .env("BRACKEN_DIR", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A mock experiment run in `dir`: its plan, an event log that logged it at setup (with a snapshot of
/// module text the view must never show), and the report `bracken-experiment report --json` would
/// write. Returns the report's measures.
fn mock_run(dir: &Path, mode: &str, turns: u32) -> Value {
    std::fs::create_dir_all(dir).unwrap();
    let plan =
        serde_json::json!({"inputs": {"task": "phase2"}, "mode": mode, "agents": 3, "turns": turns, "waves": [2, 4]});
    let plan = serde_json::to_vec_pretty(&plan).unwrap();
    std::fs::write(dir.join("plan.json"), &plan).unwrap();
    let hash = blake3::hash(&plan).to_hex().to_string();
    let setup =
        serde_json::json!({"seq": 0, "ms": 1, "agent": null, "event": "setup", "data": {"plan": hash, "mode": mode}});
    let snapshot = serde_json::json!({"seq": 1, "ms": 2, "agent": null, "event": "checkpoint",
        "data": {"turn": 1, "modules": [["base", "def never_shown(x): return x"]]}});
    std::fs::write(dir.join("events.jsonl"), format!("{setup}\n{snapshot}\n")).unwrap();
    let turns: Vec<Value> = (1..=turns)
        .map(|t| serde_json::json!({"turn": t, "waves": u32::from(t > 2) + u32::from(t > 4), "correct": t, "standalone": t}))
        .collect();
    let measures = serde_json::json!({
        "mode": mode, "stopped": null, "tokens": 1000, "output_tokens": 100, "snapshots": 4,
        "checkpoints": {
            "turns": turns, "correct": 3, "total": 90, "correct_from": {}, "median_turn_correct": 2.5,
            "recovery": [{"wave": 1, "functions": 3, "checkpoints": 1}, {"wave": 2, "functions": 4, "checkpoints": null}],
            "breakage": {"after_wave": 1, "other": 0}, "tokens_per_correct": 1.5
        }
    });
    let report = serde_json::json!([{"dir": dir.display().to_string(), "plan": hash, "measures": measures}]);
    std::fs::write(dir.join("report.json"), report.to_string()).unwrap();
    measures
}

/// `--runs` serves each run's report measures and its plan's waves, nothing else: not the run's
/// directory, not its events, no file. One mode's run, a real run's `run/` directory, or a directory
/// of mode runs to compare; and without `--runs` there's no run to serve.
#[test]
fn runs_show_only_the_report() {
    let dir = ground();
    let base = tempfile::tempdir().unwrap().keep();
    let cmp = base.join("cmp");
    let want: Vec<Value> = ["bracken", "crdt", "git"].iter().map(|m| mock_run(&cmp.join(m), m, 6)).collect();
    let (v, _) = view_args(&dir, &["--runs", cmp.to_str().unwrap()], &[]);
    let r = v.get("/api/runs");
    let text = String::from_utf8_lossy(&r.body).into_owned();
    let runs = r.json()["runs"].as_array().unwrap().clone();
    let measures: Vec<&Value> = runs.iter().map(|r| &r["measures"]).collect();
    assert_eq!(measures, want.iter().collect::<Vec<_>>());
    assert!(runs.iter().all(|r| r["waves"] == serde_json::json!([2, 4])));
    let names: Vec<&str> = runs.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["bracken", "crdt", "git"]);
    assert!(!text.contains(base.to_str().unwrap()) && !text.contains("never_shown"), "{text}");
    for raw in ["/plan.json", "/report.json", "/events.jsonl", "/git/report.json"] {
        assert_eq!(v.request("GET", raw, &format!("127.0.0.1:{}", v.port)).status, 404, "{raw}");
    }
    drop(v);

    let (one, _) = view_args(&dir, &["--runs", cmp.join("git").to_str().unwrap()], &[]);
    assert_eq!(one.api("/api/runs")["runs"].as_array().unwrap().len(), 1);
    drop(one);
    let real = base.join("opus-git-20261029T0000Z");
    let measures = mock_run(&real.join("run"), "git", 3);
    let (run, _) = view_args(&dir, &["--runs", real.to_str().unwrap()], &[]);
    let runs = run.api("/api/runs");
    assert_eq!(runs["runs"][0]["name"], "opus-git-20261029T0000Z");
    assert_eq!(runs["runs"][0]["measures"], measures);
    drop(run);

    let none = view(&dir);
    let r = none.request("GET", "/api/runs", &format!("127.0.0.1:{}", none.port));
    assert_eq!((r.status, r.json()["error"].as_str()), (404, Some("this view was started without --runs")));
}

/// `--runs` refuses what it can't show faithfully, before it listens: a run with no report (saying how
/// to make one), a report for another plan, a plan the run didn't log, a symlink, runs that don't
/// compare, two runs of one mode, a home directory, nothing to show, and any address off loopback.
#[test]
fn runs_refuse_what_they_cant_show() {
    let dir = ground();
    let base = tempfile::tempdir().unwrap().keep();
    let refuses = |runs: &Path, extra: &[&str], vars: &[(&str, &str)], why: &str| {
        let mut args = vec!["--runs", runs.to_str().unwrap()];
        args.extend_from_slice(extra);
        let err = view_refuses_with(&dir, &args, vars);
        assert!(err.contains(why), "{runs:?} {extra:?}: {err}");
    };
    let no_report = base.join("no-report");
    mock_run(&no_report, "git", 6);
    std::fs::remove_file(no_report.join("report.json")).unwrap();
    refuses(&no_report, &[], &[], "bracken-experiment report --json");

    let other_plan = base.join("other-plan");
    mock_run(&other_plan, "git", 6);
    std::fs::write(other_plan.join("plan.json"), "{}").unwrap();
    refuses(&other_plan, &[], &[], "made for another plan");

    let not_logged = base.join("not-logged");
    mock_run(&not_logged, "git", 6);
    std::fs::write(not_logged.join("events.jsonl"), "{\"event\":\"setup\",\"data\":{\"plan\":\"0\"}}\n").unwrap();
    refuses(&not_logged, &[], &[], "isn't the plan the run logged");

    let linked = base.join("linked");
    mock_run(&linked.join("bracken"), "bracken", 6);
    std::os::unix::fs::symlink(base.join("other-plan"), linked.join("git")).unwrap();
    refuses(&linked, &[], &[], "is a symlink");
    let linked_file = base.join("linked-file");
    mock_run(&linked_file, "git", 6);
    std::fs::rename(linked_file.join("report.json"), base.join("elsewhere.json")).unwrap();
    std::os::unix::fs::symlink(base.join("elsewhere.json"), linked_file.join("report.json")).unwrap();
    refuses(&linked_file, &[], &[], "is a symlink");

    let unlike = base.join("unlike");
    mock_run(&unlike.join("bracken"), "bracken", 6);
    mock_run(&unlike.join("git"), "git", 5);
    refuses(&unlike, &[], &[], "don't compare");
    let twice = base.join("twice");
    mock_run(&twice.join("a"), "git", 6);
    mock_run(&twice.join("b"), "git", 6);
    refuses(&twice, &[], &[], "one run per mode");

    let home = base.join("homes").join("someone");
    std::fs::create_dir_all(&home).unwrap();
    refuses(&base.join("homes"), &[], &[("HOME", home.to_str().unwrap())], "home directory");
    refuses(&home, &[], &[("HOME", home.to_str().unwrap())], "home directory");
    refuses(&base.join("homes"), &[], &[], "holds no experiment run");

    let good = base.join("good");
    mock_run(&good, "git", 6);
    for bind in ["10.0.0.1", "192.168.1.10"] {
        refuses(&good, &["--bind", bind], &[], "loopback only");
    }
    // A loopback listener beside one off it is still refused: the run would reach that network too.
    refuses(&good, &["--bind", "127.0.0.1", "--bind", "100.74.240.73"], &[], "loopback only");
}

/// `--bind` names exactly one address (decision 917816c72975): never every interface, multicast or
/// broadcast, and never a --host that isn't a plain name.
#[test]
fn bind_refuses_unspecified_multicast_and_broadcast() {
    let dir = ground();
    // The IPv4-mapped forms too (finding e20de255980d): checked as the IPv4 address they are.
    for (ip, shown) in [
        ("0.0.0.0", "0.0.0.0"),
        ("::", "::"),
        ("224.0.0.1", "224.0.0.1"),
        ("239.255.255.250", "239.255.255.250"),
        ("ff02::1", "ff02::1"),
        ("255.255.255.255", "255.255.255.255"),
        ("::ffff:0.0.0.0", "0.0.0.0"),
        ("::ffff:0:0", "0.0.0.0"),
        ("::ffff:224.0.0.1", "224.0.0.1"),
        ("::ffff:255.255.255.255", "255.255.255.255"),
    ] {
        let err = view_refuses(&dir, &["--bind", ip]);
        assert!(err.contains(&format!("--bind {shown}")), "{ip}: {err}");
    }
    for name in ["", "evil.example:80", "a b", "-x.example", "x..example", "http://x"] {
        let err = view_refuses(&dir, &[&format!("--host={name}")]);
        assert!(err.contains("isn't a host name"), "{name:?}: {err}");
    }
    assert!(view_refuses(&dir, &["--bind", "not-an-ip"]).contains("invalid"));
}

/// Without --bind nothing changes: 127.0.0.1 and localhost only, no warning, and a name nobody gave is
/// refused.
#[test]
fn the_default_is_unchanged() {
    let dir = ground();
    let (v, warning) = view_args(&dir, &[], &[]);
    assert!(v.at.starts_with("127.0.0.1:"), "{}", v.at);
    assert_eq!(warning, "", "no warning on loopback");
    for (host, status) in [
        (format!("127.0.0.1:{}", v.port), 200),
        (format!("localhost:{}", v.port), 200),
        (format!("warren-mbp.jx.rs:{}", v.port), 403),
        (format!("100.74.240.73:{}", v.port), 403),
    ] {
        assert_eq!(v.request("GET", "/api/ground", &host).status, status, "{host}");
    }
}

/// Bound with --host, the view answers to its address and the names given, any case, at its port,
/// and refuses every other Host with a 403 that still carries the headers.
#[test]
fn bound_with_names_answers_only_to_them() {
    let dir = ground();
    let (v, _) =
        view_args(&dir, &["--bind", "127.0.0.1", "--host", "warren-mbp.jx.rs", "--host", "Other.Example"], &[]);
    let p = v.port;
    for (host, status) in [
        (format!("warren-mbp.jx.rs:{p}"), 200),
        (format!("WARREN-MBP.jx.rs:{p}"), 200),
        (format!("other.example:{p}"), 200),
        (format!("127.0.0.1:{p}"), 200),
        (format!("warren-mbp.jx.rs:{}", p + 1), 403),
        ("warren-mbp.jx.rs".to_owned(), 403),
        (format!("warren-mbp.jx.rs.evil.example:{p}"), 403),
        (format!("evil.warren-mbp.jx.rs:{p}"), 403),
        (format!("evil.example:{p}"), 403),
        (String::new(), 403),
    ] {
        let r = v.request("GET", "/api/ground", &host);
        assert_eq!(r.status, status, "Host {host:?}");
        assert_security_headers(&format!("Host {host:?}"), &r);
    }
    let r = v.request("POST", "/api/ground", &format!("warren-mbp.jx.rs:{p}"));
    assert_eq!(r.status, 405, "still GET only");
}

/// Bound to another loopback address, IPv6's, where the machine has it: it listens there, answers to
/// `[::1]:<port>` and localhost, not to 127.0.0.1, and doesn't warn. A mapped loopback address is bound
/// as 127.0.0.1 and doesn't warn either. (Off loopback, the Host set and the warning are unit tests in
/// view.rs; the real-interface run is `bound_to_this_machines_address`, opt-in.)
#[test]
fn bound_elsewhere_listens_and_answers_only_there() {
    let dir = ground();
    if let Ok((v, warning)) = std::panic::catch_unwind(|| view_args(&dir, &["--bind", "::1"], &[])) {
        assert!(v.at.starts_with("[::1]:"), "{}", v.at);
        assert_eq!(warning, "", "::1 is loopback");
        assert_eq!(v.request("GET", "/api/ground", &format!("[::1]:{}", v.port)).status, 200);
        assert_eq!(v.request("GET", "/api/ground", &format!("localhost:{}", v.port)).status, 200);
        assert_eq!(v.request("GET", "/api/ground", &format!("127.0.0.1:{}", v.port)).status, 403);
    }
    let (v, warning) = view_args(&dir, &["--bind", "::ffff:127.0.0.1"], &[]);
    assert!(v.at.starts_with("127.0.0.1:"), "{}", v.at);
    assert_eq!(warning, "", "a mapped loopback address is loopback");
}

/// Bound to this machine's own network address: it warns, answers there, and isn't on 127.0.0.1. It
/// listens on a real interface while it runs, so it's opt-in: `cargo test -- --ignored`.
#[test]
#[ignore = "listens on this machine's network address; run with --ignored"]
fn bound_to_this_machines_address() {
    let dir = ground();
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let Ok(own) = udp.connect("192.0.2.1:9").and_then(|()| udp.local_addr()).map(|a| a.ip()) else { return };
    if own.is_loopback() || own.is_unspecified() {
        return;
    }
    let (v, warning) = view_args(&dir, &["--bind", &own.to_string()], &[]);
    assert!(warning.contains("warning") && warning.contains("anyone") && warning.contains("no login"), "{warning:?}");
    assert_eq!(v.request("GET", "/api/ground", &format!("{own}:{}", v.port)).status, 200);
    for host in [format!("127.0.0.1:{}", v.port), format!("localhost:{}", v.port)] {
        assert_eq!(v.request("GET", "/api/ground", &host).status, 403, "{host}");
    }
    let loopback = TcpStream::connect_timeout(
        &("127.0.0.1".parse::<std::net::IpAddr>().unwrap(), v.port).into(),
        Duration::from_secs(2),
    );
    assert!(loopback.is_err(), "bound to {own}, it listens on 127.0.0.1 too");
}

/// `--bind` is repeatable (decision 5863740ca157): at most one loopback and one other address, every
/// refusal applying to each, and with one address it's as before.
#[test]
fn bind_twice_refuses_two_of_a_kind_and_any_bad_address() {
    let dir = ground();
    for pair in [["127.0.0.1", "::1"], ["127.0.0.1", "::ffff:127.0.0.1"], ["100.74.240.73", "100.74.240.74"]] {
        let err = view_refuses(&dir, &["--bind", pair[0], "--bind", pair[1]]);
        assert!(err.contains("at most one"), "{pair:?}: {err}");
    }
    for bad in ["0.0.0.0", "::ffff:0.0.0.0", "224.0.0.1", "255.255.255.255"] {
        let err = view_refuses(&dir, &["--bind", "127.0.0.1", "--bind", bad]);
        assert!(err.contains("--bind"), "{bad}: {err}");
    }
}

/// One process on two listeners, loopback and this machine's own address, each with its own Host rules:
/// localhost and 127.0.0.1 only on loopback; the address and the --host name only on their own listener.
/// It listens on a real interface while it runs, so it's opt-in: `cargo test -- --ignored`.
#[test]
#[ignore = "listens on this machine's network address; run with --ignored"]
fn two_listeners_keep_their_own_host_rules() {
    let dir = ground();
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let own = udp.connect("192.0.2.1:9").and_then(|()| udp.local_addr()).map(|a| a.ip()).unwrap();
    assert!(!own.is_loopback() && !own.is_unspecified(), "no network address here");
    let mut child = Command::new(BIN)
        .args(["view", "--bind", &own.to_string(), "--bind", "127.0.0.1", "--host", "named.example"])
        .env("BRACKEN_DIR", &dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let (mut first, mut second) = (String::new(), String::new());
    out.read_line(&mut first).unwrap();
    out.read_line(&mut second).unwrap();
    let mut warning = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut warning).unwrap();
    let port: u16 = first.trim().trim_end_matches('/').rsplit(':').next().unwrap().parse().unwrap();
    let loopback = View { child, port, at: format!("127.0.0.1:{port}") };
    assert_eq!(first.trim(), format!("view at http://127.0.0.1:{port}/"), "loopback first");
    assert_eq!(second.trim(), format!("view at http://{own}:{port}/"));
    assert!(warning.contains(&format!("{own}:{port}")) && warning.contains("no login"), "{warning:?}");
    let request = |at: String, host: &str| {
        let mut s = TcpStream::connect(at.as_str()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "GET /api/ground HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        parse(&raw).status
    };
    let (lo, net) = (format!("127.0.0.1:{port}"), format!("{own}:{port}"));
    for (at, host, status) in [
        (&lo, format!("127.0.0.1:{port}"), 200),
        (&lo, format!("localhost:{port}"), 200),
        (&lo, format!("named.example:{port}"), 403),
        (&lo, format!("{own}:{port}"), 403),
        (&net, format!("{own}:{port}"), 200),
        (&net, format!("named.example:{port}"), 200),
        (&net, format!("localhost:{port}"), 403),
        (&net, format!("127.0.0.1:{port}"), 403),
    ] {
        assert_eq!(request(at.clone(), &host), status, "on {at}, Host {host}");
    }
    drop(loopback);
}
