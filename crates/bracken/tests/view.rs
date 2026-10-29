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
    let mut child = Command::new(BIN)
        .args(["view"])
        .envs(env.iter().copied())
        .env("BRACKEN_DIR", dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port = line
        .trim()
        .strip_prefix("view at http://127.0.0.1:")
        .and_then(|p| p.strip_suffix('/'))
        .unwrap_or_else(|| panic!("{line:?}"))
        .parse()
        .unwrap();
    View { child, port }
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
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
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
        "/app.js".to_owned(),
        "/style.css".to_owned(),
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
    ok(&dir, &["evidence", art, "pass", "--method", "self", "--as", "bob"]);
    let live = ok(&dir, &["intend", &claim, "--as", "alice"]).lines().next().unwrap().to_owned();
    let done = ok(&dir, &["intend", &claim, "--as", "bob"]).lines().next().unwrap().to_owned();
    ok(&dir, &["relate", art, "implements", &claim, "--as", "bob"]);
    let gone = ok(&dir, &["intend", &claim, "--as", "val"]).lines().next().unwrap().to_owned();
    ok(&dir, &["retract", &gone, "--as", "val"]);
    let refinement = ok(&dir, &["claim", "spec: parse ISO dates", "--as", "val"]);
    ok(&dir, &["relate", &refinement, "refines", &claim, "--as", "val"]);
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
    assert_eq!(root["rivals"][0]["id"], rival.as_str());
    assert_eq!(root["rivals"][0]["how"], "contradicts");
    assert_eq!(root["rivals"][0]["relation"], contra.as_str());
    let claim_short = v.api(&format!("/api/root/{rival}"))["rivals"][0]["statement"].clone();
    assert_eq!(claim_short, "spec: parse dates", "short entries are the first line");

    let intents = v.api("/api/intents");
    assert_eq!(intents[0]["target"], claim.as_str());
    assert_eq!(intents[0]["intents"][0]["id"], live.as_str());
}

#[test]
fn events_announce_what_arrives() {
    let dir = ground();
    let claim = ok(&dir, &["claim", "spec", "--as", "alice"]);
    let art = ok(&dir, &["artifact", BIN, "--name", "bin", "--implements", &claim, "--as", "bob"]);
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
    let out = ok(&dir, &["artifact", BIN, "--name", "bin", "--implements", &spec, "--as", "bob"]);
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
