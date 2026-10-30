//! `bracken serve` and `bracken sync <url>` / `--exec`, end to end on 127.0.0.1 and over stdio,
//! against throwaway Grounds.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use manyroot::sync::{Limits, MAX_IDS, Message, SYNC_PROTOCOL};
use manyroot::{ContributionId, GroundId};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_bracken");
const TOKEN: &str = "test-token-alpha";
const OTHER_TOKEN: &str = "test-token-beta";

fn cmd(dir: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.env("BRACKEN_DIR", dir).env_remove("BRACKEN_AGENT").env_remove("BRACKEN_TOKEN");
    c
}

fn run(dir: &Path, args: &[&str]) -> Output {
    cmd(dir).args(args).output().unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

/// Two replicas of one Ground, `a` and `b`, sharing identities alice and bob (in `a`), and the
/// scratch directory holding them and the server's files.
struct Grounds {
    tmp: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

fn grounds() -> Grounds {
    let tmp = tempfile::tempdir().unwrap().keep();
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    ok(&a, &["init", "--name", "g"]);
    ok(&a, &["id", "new", "alice"]);
    ok(&a, &["id", "new", "mallory"]);
    let bundle = tmp.join("seed.mrb");
    ok(&a, &["export", bundle.to_str().unwrap()]);
    ok(&b, &["join", bundle.to_str().unwrap()]);
    // b signs as bob, whose key lives only in b.
    ok(&b, &["id", "new", "bob"]);
    Grounds { tmp, a, b }
}

fn key(dir: &Path, name: &str) -> String {
    let list: Value = serde_json::from_str(&ok(dir, &["id", "list", "--json"])).unwrap();
    list.as_array().unwrap().iter().find(|e| e["name"] == name).unwrap()["agent"].as_str().unwrap().to_owned()
}

fn ground_id(dir: &Path) -> GroundId {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("ground.json")).unwrap()).unwrap();
    info["ground"].as_str().unwrap().parse().unwrap()
}

fn ids(dir: &Path) -> Vec<String> {
    let log: Value = serde_json::from_str(&ok(dir, &["log", "--json"])).unwrap();
    log.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect()
}

fn claim(dir: &Path, who: &str, text: &str) -> String {
    ok(dir, &["claim", text, "--as", who])
}

impl Grounds {
    /// Server files: an allowlist of alice and bob, a tokens file with two credentials, and the
    /// first-seen log's path.
    fn config(&self) -> (PathBuf, PathBuf, PathBuf) {
        let allow = self.tmp.join("allow");
        let line = |name| format!("{} {name}\n", key(if name == "bob" { &self.b } else { &self.a }, name));
        std::fs::write(&allow, format!("# the team\n{}{}", line("alice"), line("bob"))).unwrap();
        let tokens = self.tmp.join("tokens");
        let hash = |t: &str| blake3::hash(t.as_bytes()).to_hex().to_string();
        std::fs::write(&tokens, format!("alpha {}\nbeta {}\n", hash(TOKEN), hash(OTHER_TOKEN))).unwrap();
        (allow, tokens, self.tmp.join("first-seen.jsonl"))
    }

    fn serve_args(&self, extra: &[&str]) -> Vec<String> {
        let (allow, _, seen) = self.config();
        let mut args: Vec<String> =
            ["serve", "--allow", allow.to_str().unwrap(), "--first-seen", seen.to_str().unwrap()]
                .map(String::from)
                .to_vec();
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    }

    /// `bracken serve` over HTTP for `b`, on an ephemeral port.
    fn http(&self, extra: &[&str]) -> Server {
        self.http_for(&self.b, extra)
    }

    fn http_for(&self, dir: &Path, extra: &[&str]) -> Server {
        let (_, tokens, _) = self.config();
        let mut args = self.serve_args(extra);
        args.extend(["--tokens".into(), tokens.to_str().unwrap().into()]);
        Server::start(dir, &args)
    }

    /// `bracken sync --exec bracken serve --stdio` from `a` to `b`.
    fn exec(&self, extra: &[&str]) -> Output {
        let mut args: Vec<String> = vec!["sync".into(), "--exec".into(), BIN.into(), "--dir".into()];
        args.push(self.b.to_str().unwrap().into());
        args.extend(self.serve_args(extra));
        args.push("--stdio".into());
        cmd(&self.a).args(&args).output().unwrap()
    }

    fn first_seen(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.tmp.join("first-seen.jsonl")).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

struct Server {
    child: Child,
    url: String,
    port: u16,
}

impl Server {
    fn start(dir: &Path, args: &[String]) -> Server {
        let mut child = cmd(dir).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let url = line.trim().strip_prefix("listening on ").unwrap_or_else(|| panic!("no URL in {line:?}")).to_owned();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        let port = url.rsplit(':').next().unwrap().parse().unwrap();
        Server { child, url, port }
    }

    fn alive(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sync_http(dir: &Path, url: &str, token: &str, extra: &[&str]) -> Output {
    cmd(dir).args(["sync", url]).args(extra).env("BRACKEN_TOKEN", token).output().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn two_replicas_converge_exactly_over_http() {
    let g = grounds();
    for i in 0..5 {
        claim(&g.a, "alice", &format!("a{i}"));
        claim(&g.b, "bob", &format!("b{i}"));
    }
    let server = g.http(&[]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "received 5, sent 5");
    assert_eq!(ids(&g.a), ids(&g.b), "same graph, same causal order");
    let again = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert_eq!(stdout(&again), "received 0, sent 0", "{}", stderr(&again));
    drop(server);
    // The server's replica, reopened from disk, holds the same set.
    assert_eq!(ids(&g.a), ids(&g.b));
}

#[test]
fn two_replicas_converge_exactly_over_stdio() {
    let g = grounds();
    claim(&g.a, "alice", "from a");
    claim(&g.b, "bob", "from b");
    let out = g.exec(&[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "received 1, sent 1");
    assert_eq!(ids(&g.a), ids(&g.b));
    assert_eq!(stdout(&g.exec(&[])), "received 0, sent 0");
}

#[test]
fn an_author_not_on_the_allowlist_is_refused_and_nothing_of_theirs_is_stored() {
    let g = grounds();
    let good = claim(&g.a, "alice", "allowed");
    let bad = claim(&g.a, "mallory", "not allowed");
    let server = g.http(&[]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success(), "a refusal fails the sync");
    assert!(stderr(&out).contains("allowlist"), "{}", stderr(&out));
    drop(server);
    let held = ids(&g.b);
    assert!(held.contains(&good));
    assert!(!held.contains(&bad));
    let logged: Vec<String> = g.first_seen().iter().map(|s| s["id"].as_str().unwrap().to_owned()).collect();
    assert_eq!(logged, vec![good], "only what was stored is logged");
    let mallory = key(&g.a, "mallory");
    assert!(ok(&g.b, &["log", "--json", "--author", &mallory]).trim() == "[]");

    // Over stdio too.
    let out = g.exec(&[]);
    assert!(!out.status.success());
    assert!(!ids(&g.b).contains(&bad));
}

#[test]
fn a_server_without_an_allowlist_stores_nothing_but_serves_reads() {
    let g = grounds();
    claim(&g.a, "alice", "push me");
    claim(&g.b, "bob", "pull me");
    let (_, tokens, _) = g.config();
    let server = Server::start(&g.b, &["serve".into(), "--tokens".into(), tokens.to_str().unwrap().into()]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("accepts no contributions"), "{}", stderr(&out));
    assert_eq!(ids(&g.a).len(), 2, "a still received b's claim");
    drop(server);
    assert_eq!(ids(&g.b).len(), 1);
}

#[test]
fn the_first_seen_log_records_each_new_contribution_once() {
    let g = grounds();
    let mut mine = vec![claim(&g.a, "alice", "one"), claim(&g.a, "alice", "two")];
    let server = g.http(&[]);
    assert!(sync_http(&g.a, &server.url, TOKEN, &[]).status.success());
    assert!(sync_http(&g.a, &server.url, OTHER_TOKEN, &[]).status.success());
    mine.push(claim(&g.a, "alice", "three"));
    assert!(sync_http(&g.a, &server.url, OTHER_TOKEN, &[]).status.success());
    drop(server);
    // A restarted server, and a session over stdio, log nothing twice.
    let server = g.http(&[]);
    assert!(sync_http(&g.a, &server.url, TOKEN, &[]).status.success());
    drop(server);
    assert!(g.exec(&[]).status.success());
    let log = g.first_seen();
    let logged: Vec<&str> = log.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(logged, mine.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(log[0]["credential"], "alpha");
    assert_eq!(log[2]["credential"], "beta");
    assert_eq!(log[0]["author"], key(&g.a, "alice").as_str());
    assert!(log[0]["bytes"].as_u64().unwrap() > 0 && log[0]["first_seen_ms"].as_u64().unwrap() > 0);
}

#[test]
fn quota_exhaustion_is_refused_and_survives_a_restart() {
    let g = grounds();
    for i in 0..3 {
        claim(&g.a, "alice", &format!("c{i}"));
    }
    let server = g.http(&["--quota-contributions", "2"]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("quota"), "{}", stderr(&out));
    drop(server);
    assert_eq!(ids(&g.b).len(), 2, "two stored, the third refused");

    // The quota is replayed from the first-seen log, so a restart doesn't reset it.
    let server = g.http(&["--quota-contributions", "2"]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(stderr(&out).contains("quota"), "{}", stderr(&out));
    drop(server);
    assert_eq!(ids(&g.b).len(), 2);

    // The byte quota and the storage cap refuse the same way.
    for flag in ["--quota-bytes", "--store-cap"] {
        let server = g.http(&[flag, "10"]);
        let out = sync_http(&g.a, &server.url, TOKEN, &[]);
        assert!(!out.status.success(), "{flag}");
        drop(server);
        assert_eq!(ids(&g.b).len(), 2, "{flag}");
    }
}

#[test]
fn a_session_over_its_byte_budget_ends_with_an_error_and_leaves_the_store_valid() {
    let g = grounds();
    for i in 0..20 {
        claim(&g.a, "alice", &format!("{i}: {}", "x".repeat(500)));
    }
    let server = g.http(&["--budget", "4000"]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("budget"), "{}", stderr(&out));
    drop(server);
    assert!(ids(&g.b).len() < 20, "cut off before everything arrived");

    // Over stdio, the server's budget ends the session as well.
    let out = g.exec(&["--budget", "4000"]);
    assert!(!out.status.success());
    ok(&g.b, &["log"]); // the replica still opens and verifies

    // The client holds the server to its own budget, too: a fresh replica pulling all of a.
    let c = g.tmp.join("c");
    ok(&c, &["join", g.tmp.join("seed.mrb").to_str().unwrap()]);
    let server = g.http_for(&g.a, &[]);
    let out = sync_http(&c, &server.url, TOKEN, &["--budget", "1500"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("budget"), "{}", stderr(&out));
    ok(&c, &["log"]);
}

/// A peer on a raw stream to `bracken serve --stdio`.
struct Raw {
    child: Child,
}

impl Raw {
    fn start(g: &Grounds, extra: &[&str]) -> Raw {
        let mut args = g.serve_args(extra);
        args.push("--stdio".into());
        let child =
            cmd(&g.b).args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        Raw { child }
    }

    fn send(&mut self, bytes: &[u8]) -> bool {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(bytes)
            .and_then(|()| self.child.stdin.as_mut().unwrap().flush())
            .is_ok()
    }

    /// Waits for the server to exit, at most `limit`. Returns its status code and stderr.
    fn finish(mut self, limit: Duration) -> (Option<i32>, String) {
        drop(self.child.stdin.take());
        let start = Instant::now();
        while self.child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < limit, "the server didn't end the session in time");
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = self.child.wait().unwrap();
        let mut err = String::new();
        self.child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
        (status.code(), err)
    }
}

fn frame(m: &Message) -> Vec<u8> {
    let bytes = m.encode();
    let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
    out.extend(bytes);
    out
}

fn hello(ground: GroundId) -> Message {
    Message::Hello { protocol: SYNC_PROTOCOL, ground, limits: Limits::default(), features: vec![] }
}

#[test]
fn a_silent_peer_is_cut_off_at_the_deadline() {
    let g = grounds();
    let mut raw = Raw::start(&g, &["--deadline", "1"]);
    let start = Instant::now();
    // Stdin stays open and silent: the watchdog ends the session.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(raw.child.try_wait().unwrap().is_some(), "still running after its deadline");
    let (code, err) = raw.finish(Duration::from_secs(5));
    assert_eq!(code, Some(1));
    assert!(err.contains("deadline"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(5));
    ok(&g.b, &["log"]);

    // Over HTTP, a session's next request after its deadline is refused.
    let server = g.http(&["--deadline", "1"]);
    let ground = ground_id(&g.b);
    let mut body = frame(&hello(ground));
    body.extend(frame(&Message::Heads { heads: vec![] }));
    let (status, head, _) = post(server.port, &format!("/v1/{ground}/sessions"), TOKEN, &body);
    assert_eq!(status, 200);
    let id = header(&head, "x-bracken-session").unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let (status, _, text) = post(server.port, &format!("/v1/{ground}/sessions/{id}"), TOKEN, &[]);
    assert_eq!(status, 408, "{}", String::from_utf8_lossy(&text));
}

#[test]
fn endless_full_id_lists_are_cut_off_at_the_id_budget() {
    let g = grounds();
    let ground = ground_id(&g.b);
    let full: Vec<ContributionId> = (0..MAX_IDS as u32)
        .map(|i| {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&i.to_be_bytes());
            ContributionId(id)
        })
        .collect();
    let heads = frame(&Message::Heads { heads: full });
    let mut raw = Raw::start(&g, &["--id-budget", "5000000"]);
    raw.send(&frame(&hello(ground)));
    let mut sent = 0;
    while sent < 20 && raw.send(&heads) {
        sent += 1;
    }
    assert!(sent < 20, "the server kept reading heads past its id budget");
    let (code, err) = raw.finish(Duration::from_secs(10));
    assert_eq!(code, Some(1));
    assert!(err.contains("id lists"), "{err}");
}

#[test]
fn a_hostile_stream_peer_cant_crash_the_server_or_corrupt_the_store() {
    let g = grounds();
    claim(&g.b, "bob", "before");
    let ground = ground_id(&g.b);
    let before = ids(&g.b);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("garbage", b"\x00\x00\x00\x05hello".to_vec()),
        ("a frame over the message limit", u32::MAX.to_be_bytes().to_vec()),
        ("a truncated frame", [100u32.to_be_bytes().as_slice(), b"short"].concat()),
        ("a batch before hello", frame(&Message::Batch { bundle: vec![], last: true })),
        ("a foreign ground", frame(&hello(GroundId([7; 32])))),
        ("heads before hello", frame(&Message::Heads { heads: vec![] })),
        (
            "a bad bundle",
            [
                frame(&hello(ground)),
                frame(&Message::Heads { heads: vec![] }),
                frame(&Message::Batch { bundle: b"MRBUNDL1xx".to_vec(), last: true }),
            ]
            .concat(),
        ),
        ("an empty stream", vec![]),
    ];
    for (what, bytes) in cases {
        let mut raw = Raw::start(&g, &[]);
        raw.send(&bytes);
        let (code, err) = raw.finish(Duration::from_secs(10));
        assert_eq!(code, Some(1), "{what}: {err}");
        assert!(!err.contains("panicked"), "{what}: {err}");
    }
    assert_eq!(ids(&g.b), before, "the store is unchanged and still verifies");
}

/// Sends one HTTP request; returns the status, the head and the body.
fn post(port: u16, path: &str, token: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    raw_http(port, &[head.as_bytes(), body].concat(), true)
}

/// Writes `bytes`, optionally half-closes, and reads the whole response.
fn raw_http(port: u16, bytes: &[u8], close: bool) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
    let _ = s.write_all(bytes);
    if close {
        let _ = s.shutdown(std::net::Shutdown::Write);
    }
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").map_or(out.len(), |i| i + 4);
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, head, out[split..].to_vec())
}

fn header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
    })
}

#[test]
fn a_hostile_http_peer_cant_crash_the_server_or_corrupt_the_store() {
    let g = grounds();
    claim(&g.a, "alice", "after the attack");
    claim(&g.b, "bob", "before the attack");
    let mut server = g.http(&["--budget", "100000"]);
    let ground = ground_id(&g.b);
    let open = format!("/v1/{ground}/sessions");
    let opening = [frame(&hello(ground)), frame(&Message::Heads { heads: vec![] })].concat();
    let req = |extra: &str, body: &[u8]| {
        [format!("POST {open} HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n{extra}\r\n").as_bytes(), body].concat()
    };
    let len = |n: usize| format!("Content-Length: {n}\r\n");
    let cases: Vec<(&str, Vec<u8>, bool, u16)> = vec![
        ("oversized headers", req(&format!("X-Pad: {}\r\n{}", "a".repeat(20_000), len(0)), b""), true, 431),
        ("too many headers", req(&format!("{}{}", "X-A: b\r\n".repeat(40), len(0)), b""), true, 431),
        ("no Content-Length", req("", b""), true, 411),
        ("chunked", req("Transfer-Encoding: chunked\r\n", b"0\r\n\r\n"), true, 501),
        ("two Content-Lengths", req(&format!("{}{}", len(0), len(1)), b""), true, 400),
        ("a Content-Length over the budget", req(&len(1 << 40), b""), true, 413),
        ("a body shorter than its Content-Length", req(&len(opening.len() + 50), &opening), true, 400),
        ("garbage frames", req(&len(9), b"\x00\x00\x00\x05hello"), true, 400),
        ("a frame over the message limit", req(&len(4), &u32::MAX.to_be_bytes()), true, 413),
        ("not HTTP", b"\x16\x03\x01garbage\r\n\r\n".to_vec(), true, 400),
        ("GET", format!("GET {open} HTTP/1.1\r\n\r\n").into_bytes(), true, 405),
        ("a head that never ends", b"POST / HTTP/1.1\r\nX: y".to_vec(), true, 400),
    ];
    for (what, bytes, close, want) in cases {
        let (status, _, body) = raw_http(server.port, &bytes, close);
        assert_eq!(status, want, "{what}: {}", String::from_utf8_lossy(&body));
        assert!(server.alive(), "{what} crashed the server");
    }
    let batch_first = frame(&Message::Batch { bundle: vec![], last: true });
    let (status, _, _) = post(server.port, &open, TOKEN, &batch_first);
    assert_eq!(status, 400, "a batch before hello is out of order");

    // Credentials: none, a wrong one, and another credential's session.
    assert_eq!(post(server.port, &open, "wrong", &opening).0, 401);
    assert_eq!(
        raw_http(server.port, format!("POST {open} HTTP/1.1\r\nContent-Length: 0\r\n\r\n").as_bytes(), true).0,
        401
    );
    let (status, head, _) = post(server.port, &open, TOKEN, &opening);
    assert_eq!(status, 200);
    let id = header(&head, "x-bracken-session").unwrap();
    let (status, _, text) = post(server.port, &format!("{open}/{id}"), OTHER_TOKEN, &[]);
    assert_eq!(status, 404, "another credential can't use the session");
    assert!(!String::from_utf8_lossy(&text).contains(&id), "the session id isn't echoed");
    assert_eq!(post(server.port, &format!("{open}/{}", "0".repeat(32)), TOKEN, &[]).0, 404);
    assert_eq!(post(server.port, &format!("/v1/{}/sessions", GroundId([1; 32])), TOKEN, &opening).0, 404);

    // Pipelined requests: only the first is answered, and the server carries on.
    let one = [
        format!("POST {open} HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n{}\r\n", len(opening.len())).as_bytes(),
        &opening,
    ]
    .concat();
    let (status, _, body) = raw_http(server.port, &[one.clone(), one].concat(), true);
    assert_eq!(status, 200);
    assert!(!String::from_utf8_lossy(&body).contains("HTTP/1.1"), "one response per connection");

    // A slow-loris that sends nothing holds no session open, and the server still serves.
    let _idle = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    assert!(server.alive());
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    drop(server);
    assert_eq!(ids(&g.a), ids(&g.b), "the store is valid and converged after the attack");
}

#[test]
fn a_symlinked_objects_shard_is_refused() {
    let g = grounds();
    let id = claim(&g.a, "alice", "aimed outside");
    let outside = g.tmp.join("outside");
    std::fs::create_dir(&outside).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, g.b.join("objects").join(&id[..2])).unwrap();
    let server = g.http(&[]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success());
    drop(server);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing written outside the Ground");
    assert!(g.first_seen().is_empty(), "nothing logged as stored");
}

#[test]
fn server_files_and_urls_are_checked() {
    let g = grounds();
    let (allow, tokens, seen) = g.config();
    let serve = |args: &[&str]| run(&g.b, args);
    // HTTP needs tokens; a symlinked or world-writable config file is refused; --allow needs a log.
    assert!(stderr(&serve(&["serve"])).contains("--tokens"));
    let link = g.tmp.join("tokens-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&tokens, &link).unwrap();
    assert!(stderr(&serve(&["serve", "--tokens", link.to_str().unwrap()])).contains("symlink"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&allow, std::fs::Permissions::from_mode(0o666)).unwrap();
        let args = [
            "serve",
            "--tokens",
            tokens.to_str().unwrap(),
            "--allow",
            allow.to_str().unwrap(),
            "--first-seen",
            seen.to_str().unwrap(),
        ];
        assert!(stderr(&serve(&args)).contains("writable by anyone"));
    }
    assert!(!serve(&["serve", "--stdio", "--allow", allow.to_str().unwrap()]).status.success());

    // The client only speaks to loopback, and never takes a token on its command line.
    let out = sync_http(&g.a, "http://example.com:80", TOKEN, &[]);
    assert!(stderr(&out).contains("loopback"), "{}", stderr(&out));
    let out = cmd(&g.a).args(["sync", "http://127.0.0.1:1"]).output().unwrap();
    assert!(stderr(&out).contains("BRACKEN_TOKEN"), "{}", stderr(&out));
}

/// Writes `bytes` one at a time, `every` apart, until the peer closes or `limit` passes.
fn trickle(mut s: TcpStream, bytes: Vec<u8>, every: Duration, limit: Duration) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let start = Instant::now();
        for b in bytes {
            if start.elapsed() > limit || s.write_all(&[b]).is_err() {
                return;
            }
            std::thread::sleep(every);
        }
    })
}

/// Reads a response until the server closes, and returns its status.
fn read_status(s: &mut TcpStream) -> u16 {
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[test]
fn a_trickling_body_is_cut_off_at_the_session_deadline_and_frees_its_slot() {
    let g = grounds();
    let server = g.http(&["--deadline", "2", "--max-sessions", "1"]);
    let ground = ground_id(&g.b);
    let open = format!("/v1/{ground}/sessions");
    let opening = [frame(&hello(ground)), frame(&Message::Heads { heads: vec![] })].concat();
    let (status, head, _) = post(server.port, &open, TOKEN, &opening);
    assert_eq!(status, 200);
    let start = Instant::now();
    let id = header(&head, "x-bracken-session").unwrap();

    // A round whose one frame arrives a byte at a time, each well inside the per-read timeout.
    let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let head = format!("POST {open}/{id} HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 904\r\n\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(&900u32.to_be_bytes()).unwrap();
    let writer = trickle(s.try_clone().unwrap(), vec![0xa0; 900], Duration::from_millis(200), Duration::from_secs(15));
    assert_eq!(read_status(&mut s), 408);
    assert!(start.elapsed() < Duration::from_secs(5), "cut off at the deadline, not later: {:?}", start.elapsed());
    drop(s);
    writer.join().unwrap();
    // The session's slot is free again.
    assert_eq!(post(server.port, &open, TOKEN, &opening).0, 200);
}

#[test]
fn a_trickling_request_head_is_cut_off_before_authentication() {
    let g = grounds();
    let server = g.http(&[]);
    let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let start = Instant::now();
    let writer = trickle(s.try_clone().unwrap(), vec![b'X'; 1000], Duration::from_millis(500), Duration::from_secs(30));
    assert_eq!(read_status(&mut s), 408);
    let took = start.elapsed();
    assert!(took >= Duration::from_secs(8) && took < Duration::from_secs(15), "the head gets 10 s in all: {took:?}");
    drop(s);
    writer.join().unwrap();
}

#[test]
fn the_client_cuts_off_a_trickling_server_at_its_deadline() {
    let g = grounds();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let fake = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf);
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".repeat(10);
        trickle(s, response, Duration::from_millis(200), Duration::from_secs(15)).join().unwrap();
    });
    let start = Instant::now();
    let out = sync_http(&g.a, &format!("http://127.0.0.1:{port}"), TOKEN, &["--deadline", "2"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("deadline"), "{}", stderr(&out));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    fake.join().unwrap();
}

#[test]
fn a_store_write_failing_mid_batch_still_logs_and_counts_what_was_stored() {
    let g = grounds();
    let one = claim(&g.a, "alice", "one");
    let mut two = claim(&g.a, "alice", "two");
    let mut n = 0;
    while two[..2] == one[..2] {
        n += 1;
        two = claim(&g.a, "alice", &format!("two {n}"));
    }
    let outside = g.tmp.join("outside");
    std::fs::create_dir(&outside).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, g.b.join("objects").join(&two[..2])).unwrap();
    let server = g.http(&[]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(!out.status.success());
    // Whatever reached the replica is in the first-seen log, and the server stores nothing more.
    let again = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(stderr(&again).contains("can't store anything"), "{}", stderr(&again));
    drop(server);
    let held = ids(&g.b);
    let logged: Vec<String> = g.first_seen().iter().map(|s| s["id"].as_str().unwrap().to_owned()).collect();
    assert!(held.contains(&one));
    assert_eq!(logged, held, "every stored contribution is logged, once");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

    // The quota counted it too: with room for one contribution, a restarted server refuses more.
    let server = g.http(&["--quota-contributions", "1"]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(stderr(&out).contains("quota"), "{}", stderr(&out));
}

/// The messages in a response body.
fn messages(body: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    let mut rest = body;
    while rest.len() >= 4 {
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        out.push(Message::decode(&rest[4..4 + len], usize::MAX).unwrap());
        rest = &rest[4 + len..];
    }
    out
}

fn hello_with(ground: GroundId, batch_bytes: u64) -> Message {
    Message::Hello { protocol: SYNC_PROTOCOL, ground, limits: Limits { batch_bytes }, features: vec![] }
}

#[test]
fn a_response_holds_one_batch_within_the_servers_own_limit() {
    let g = grounds();
    for i in 0..20 {
        claim(&g.b, "bob", &format!("{i}: {}", "y".repeat(500)));
    }
    let server = g.http(&["--batch-bytes", "5000"]);
    let ground = ground_id(&g.b);
    let open = format!("/v1/{ground}/sessions");
    // A peer asking for batches of any size gets them no larger than the server's own limit.
    let body = [frame(&hello_with(ground, u64::MAX)), frame(&Message::Heads { heads: vec![] })].concat();
    let (status, head, reply) = post(server.port, &open, TOKEN, &body);
    assert_eq!(status, 200);
    let batches: Vec<usize> = messages(&reply)
        .iter()
        .filter_map(|m| if let Message::Batch { bundle, .. } = m { Some(bundle.len()) } else { None })
        .collect();
    assert_eq!(batches.len(), 1);
    assert!(batches[0] <= 5000, "{batches:?}");

    // Results pipelined ahead of their batches would release them all into one response.
    let id = header(&head, "x-bracken-session").unwrap();
    let result = frame(&Message::Result { added: 0, already_present: 0, rejected: vec![], unresolved: 0 });
    let (status, _, text) = post(server.port, &format!("{open}/{id}"), TOKEN, &result.repeat(15));
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&text));
    assert!(String::from_utf8_lossy(&text).contains("one batch"));
}

#[test]
fn replicas_converge_over_many_small_batches() {
    let g = grounds();
    for i in 0..10 {
        claim(&g.a, "alice", &format!("a{i}: {}", "x".repeat(600)));
        claim(&g.b, "bob", &format!("b{i}: {}", "y".repeat(600)));
    }
    let server = g.http(&["--batch-bytes", "3000"]);
    let out = sync_http(&g.a, &server.url, TOKEN, &["--batch-bytes", "2000"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "received 10, sent 10");
    drop(server);
    assert_eq!(ids(&g.a), ids(&g.b));
    claim(&g.a, "alice", &"z".repeat(600));
    claim(&g.b, "bob", &"w".repeat(600));
    let out = g.exec(&["--batch-bytes", "2000"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(ids(&g.a), ids(&g.b));
}

#[test]
fn concurrent_opens_cant_pass_max_sessions() {
    let g = grounds();
    let server = g.http(&["--max-sessions", "1"]);
    let ground = ground_id(&g.b);
    let body = [frame(&hello(ground)), frame(&Message::Heads { heads: vec![] })].concat();
    let request = [
        format!(
            "POST /v1/{ground}/sessions HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes(),
        body,
    ]
    .concat();
    let (all_but_last, last) = request.split_at(request.len() - 1);
    let mut conns: Vec<TcpStream> = (0..5)
        .map(|_| {
            let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            s.write_all(all_but_last).unwrap();
            s
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    for s in &mut conns {
        let _ = s.write_all(last);
    }
    let statuses: Vec<u16> = conns.iter_mut().map(read_status).collect();
    assert_eq!(statuses.iter().filter(|&&s| s == 200).count(), 1, "{statuses:?}");
    assert_eq!(statuses.iter().filter(|&&s| s == 429).count(), 4, "{statuses:?}");
}

#[test]
fn sync_exec_kills_a_program_that_outlives_the_session_and_hides_the_token() {
    let g = grounds();
    // A program that breaks the session and stays is killed soon after, long before the deadline.
    let start = Instant::now();
    let out = cmd(&g.a)
        .args(["sync", "--deadline", "60", "--exec", "sh", "-c", "printf '\\000\\000\\000\\001x'; exec sleep 25"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(start.elapsed() < Duration::from_secs(6), "waited {:?} for the program", start.elapsed());

    // A program that never answers is killed at the deadline.
    let start = Instant::now();
    let out = cmd(&g.a).args(["sync", "--deadline", "2", "--exec", "sleep", "25"]).output().unwrap();
    assert!(stderr(&out).contains("deadline"), "{}", stderr(&out));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());

    // The program doesn't see this side's token or replica.
    let seen = g.tmp.join("seen-env");
    let script = format!("printf '%s|%s' \"${{BRACKEN_TOKEN:-none}}\" \"${{BRACKEN_DIR:-none}}\" > {}", seen.display());
    let out = cmd(&g.a).env("BRACKEN_TOKEN", TOKEN).args(["sync", "--exec", "sh", "-c", &script]).output().unwrap();
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "none|none");
}

#[test]
fn sync_exec_ends_at_the_deadline_whatever_the_programs_descendants_do() {
    let g = grounds();
    let has = |program: &str| Command::new(program).arg("-v").output().is_ok_and(|o| o.status.success());
    // Each leaves a descendant holding the program's stdout after the program itself is killed:
    // a forked child, a background job in its own process group, and one in a new session.
    let mut scripts = vec!["sleep 20; true", "sleep 20 & exec sleep 25", "set -m; sleep 20 & wait"];
    if has("perl") {
        scripts.push("perl -MPOSIX -e 'POSIX::setsid() or die; sleep 20'; true");
    } else {
        eprintln!("perl isn't installed: skipping the setsid case");
    }
    for script in scripts {
        let start = Instant::now();
        // Through pipes, as a caller capturing bracken's output reads it: a descendant left alive
        // must hold neither of them.
        let out = cmd(&g.a).args(["sync", "--deadline", "3", "--exec", "sh", "-c", script]).output().unwrap();
        assert!(stderr(&out).contains("deadline"), "{script}: {}", stderr(&out));
        assert!(start.elapsed() < Duration::from_secs(6), "{script}: took {:?}", start.elapsed());
    }
}

#[test]
fn sync_exec_relays_the_programs_stderr() {
    let g = grounds();
    let out = cmd(&g.a).args(["sync", "--exec", "sh", "-c", "echo from-the-program >&2"]).output().unwrap();
    assert!(stderr(&out).contains("from-the-program"), "{}", stderr(&out));
}

#[cfg(unix)]
#[test]
fn sync_exec_keeps_the_program_in_brackens_process_group_so_it_can_use_the_terminal() {
    // A program outside the terminal's foreground group is stopped when it reads /dev/tty (ssh's
    // password and host-key prompts). This test has no terminal, so it checks the condition
    // instead: the program shares bracken's process group, which is the foreground one whenever
    // bracken runs at a terminal.
    let g = grounds();
    let seen = g.tmp.join("pgids");
    let script = format!("ps -o pgid= -p $$ > {0}; ps -o pgid= -p $PPID >> {0}", seen.display());
    let out = cmd(&g.a).args(["sync", "--exec", "sh", "-c", &script]).output().unwrap();
    assert!(!out.status.success(), "the program speaks no protocol");
    let text = std::fs::read_to_string(&seen).unwrap();
    let pgids: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(pgids.len(), 2, "{text:?}");
    assert_eq!(pgids[0], pgids[1], "the program is in bracken's process group");
}

/// `sync --confirm` confirms only on the server's own heads, sent at the start of a second session:
/// a server that holds everything confirms; one whose second session reaches a replica lacking this
/// replica's heads doesn't, even though both sessions were clean.
#[test]
fn sync_confirm_needs_the_servers_heads_to_cover_this_replica() {
    let g = grounds();
    claim(&g.a, "alice", "to confirm");
    let mut args: Vec<String> = vec!["sync".into(), "--confirm".into(), "--exec".into(), BIN.into(), "--dir".into()];
    args.push(g.b.to_str().unwrap().into());
    args.extend(g.serve_args(&[]));
    args.push("--stdio".into());
    let out = cmd(&g.a).args(&args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).ends_with("confirmed: the server holds all 1 heads"), "{}", stdout(&out));

    // A "server" whose second session reaches c, a replica that never got the new claim.
    let c = g.tmp.join("c");
    ok(&c, &["join", g.tmp.join("seed.mrb").to_str().unwrap()]);
    claim(&g.a, "alice", "not on c");
    let (allow, _, seen) = g.config();
    let script = g.tmp.join("two-servers");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ -e {n} ]; then dir={c}; else : >{n}; dir={b}; fi\n\
             exec {bin} --dir \"$dir\" serve --allow {allow} --first-seen {seen} --stdio\n",
            n = g.tmp.join("served-once").display(),
            c = c.display(),
            b = g.b.display(),
            bin = BIN,
            allow = allow.display(),
            seen = seen.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let out = cmd(&g.a).args(["sync", "--confirm", "--exec", script.to_str().unwrap()]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("not confirmed: the server's heads don't cover 1 of this replica's 1 heads"), "{stderr}");

    // --confirm needs a server.
    let out = run(&g.a, &["sync", "--confirm", g.b.to_str().unwrap()]);
    assert!(!out.status.success());
}

// The change feed (task 09bf8248e69b): `POST /v1/<ground>/changes`, and `watch --feed` reading it.

fn feed_body(since: &str, wait_ms: u64) -> Vec<u8> {
    serde_json::json!({"since": since, "wait_ms": wait_ms}).to_string().into_bytes()
}

/// A change feed request: the status, the head, and the reply's contributions.
fn changes(port: u16, ground: GroundId, since: &str, wait_ms: u64) -> (u16, String, Vec<manyroot::Contribution>) {
    let (status, head, body) = post(port, &format!("/v1/{ground}/changes"), TOKEN, &feed_body(since, wait_ms));
    if status != 200 {
        return (status, head, Vec::new());
    }
    let (from, contributions) = manyroot::bundle::read(body.as_slice()).expect("a bundle");
    assert_eq!(from, ground);
    (status, head, contributions)
}

/// The heads of what `cs` holds, as a cursor.
fn cursor_of(cs: &[manyroot::Contribution]) -> String {
    let parents: std::collections::HashSet<ContributionId> = cs.iter().flat_map(|c| c.parents().to_vec()).collect();
    let mut heads: Vec<String> =
        cs.iter().map(|c| c.id()).filter(|id| !parents.contains(id)).map(|id| id.to_hex()).collect();
    heads.sort();
    heads.join(",")
}

/// Every contribution's parents come before it, or were seen already.
fn in_causal_order(cs: &[manyroot::Contribution], seen: &[manyroot::Contribution]) -> bool {
    let mut held: std::collections::HashSet<ContributionId> = seen.iter().map(|c| c.id()).collect();
    cs.iter().all(|c| {
        let ok = c.parents().iter().all(|p| held.contains(p));
        held.insert(c.id());
        ok
    })
}

#[test]
fn the_change_feed_answers_what_a_cursor_hasnt_seen() {
    let g = grounds();
    for i in 0..3 {
        claim(&g.b, "bob", &format!("b{i}"));
    }
    let server = g.http(&[]);
    let ground = ground_id(&g.b);
    let path = format!("/v1/{ground}/changes");
    // A token is needed, and the server's ground.
    assert_eq!(post(server.port, &path, "not-a-token", &feed_body("", 0)).0, 401);
    let body = feed_body("", 0);
    let unsigned = format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n", body.len());
    assert_eq!(raw_http(server.port, &[unsigned.as_bytes(), &body].concat(), true).0, 401, "no token at all");
    assert_eq!(post(server.port, "/v1/nowhere/changes", TOKEN, &feed_body("", 0)).0, 404);
    // From "": everything, in causal order, in one reply.
    let (status, head, all) = changes(server.port, ground, "", 0);
    assert_eq!(status, 200, "{head}");
    let mut got: Vec<String> = all.iter().map(|c| c.id().to_hex()).collect();
    let mut want = ids(&g.b);
    got.sort();
    want.sort();
    assert_eq!(got, want);
    assert!(in_causal_order(&all, &[]), "parents first");
    assert_eq!(header(&head, "x-bracken-more"), None);
    // Past the heads, nothing, once the wait is over.
    let at = cursor_of(&all);
    let started = Instant::now();
    let (status, head, none) = changes(server.port, ground, &at, 400);
    assert!(status == 200 && none.is_empty(), "{head}");
    assert!(started.elapsed() >= Duration::from_millis(400), "answered before its wait was over");
    // Past the first claim only, the rest. A head the server lacks is named, and left out of what's seen.
    let first =
        all.iter().position(|c| matches!(c.body(), manyroot::Body::Claim { statement, .. } if statement == "b0"));
    let first = &all[..=first.expect("b0")];
    let lacked = "0".repeat(64);
    let (status, head, rest) = changes(server.port, ground, &format!("{},{lacked}", cursor_of(first)), 0);
    assert_eq!(status, 200, "{head}");
    assert_eq!(rest.len(), all.len() - first.len());
    assert!(in_causal_order(&rest, first));
    assert_eq!(header(&head, "x-bracken-unknown-heads").as_deref(), Some(lacked.as_str()));
    // Bad requests.
    for (body, status) in [
        (feed_body("", 25_001), 400),
        (b"{\"since\": \"\"}".to_vec(), 400),
        (feed_body("abc", 0), 400),
        (feed_body(&at[..12], 0), 400),
        (vec![b' '; 64 * 1024 + 1], 413),
    ] {
        assert_eq!(post(server.port, &path, TOKEN, &body).0, status, "{}", String::from_utf8_lossy(&body[..20]));
    }
}

#[test]
fn a_change_feed_long_poll_is_woken_by_a_push() {
    let g = grounds();
    let server = g.http(&[]);
    let ground = ground_id(&g.b);
    let (_, _, all) = changes(server.port, ground, "", 0);
    let at = cursor_of(&all);
    let port = server.port;
    let started = Instant::now();
    let waiting = std::thread::spawn(move || changes(port, ground, &at, 20_000));
    std::thread::sleep(Duration::from_millis(500));
    let pushed = claim(&g.a, "alice", "while the feed waits");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let (status, head, new) = waiting.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "the long poll wasn't woken: {:?}", started.elapsed());
    assert_eq!(status, 200, "{head}");
    assert_eq!(new.iter().map(|c| c.id().to_hex()).collect::<Vec<_>>(), vec![pushed]);
}

#[test]
fn the_change_feed_pages_by_size_and_bounds_its_waiters() {
    let g = grounds();
    // Two 5 MiB artifacts: the 8 MiB reply holds the first, and says there is more.
    for name in ["one", "two"] {
        let file = g.tmp.join(name);
        std::fs::write(&file, vec![name.as_bytes()[0]; 5 << 20]).unwrap();
        ok(&g.a, &["artifact", file.to_str().unwrap(), "--as", "alice"]);
    }
    let server = g.http_for(&g.a, &[]);
    let ground = ground_id(&g.a);
    let (status, head, page) = changes(server.port, ground, "", 0);
    assert_eq!(status, 200, "{head}");
    assert_eq!(header(&head, "x-bracken-more").as_deref(), Some("1"));
    let (_, head, rest) = changes(server.port, ground, &cursor_of(&page), 0);
    assert_eq!(header(&head, "x-bracken-more"), None);
    assert_eq!(page.len() + rest.len(), ids(&g.a).len());
    assert!(rest.len() == 1 && in_causal_order(&rest, &page));
    // Sixteen requests may wait at once; the seventeenth is refused until one ends.
    let at = cursor_of(&[page, rest].concat());
    let port = server.port;
    let waiters: Vec<_> = (0..16)
        .map(|_| {
            let at = at.clone();
            std::thread::spawn(move || changes(port, ground, &at, 3_000).0)
        })
        .collect();
    std::thread::sleep(Duration::from_millis(1_000));
    assert_eq!(changes(port, ground, &at, 0).0, 429);
    assert!(waiters.into_iter().all(|w| w.join().unwrap() == 200));
    assert_eq!(changes(port, ground, &at, 0).0, 200);
}

/// `watch --follow --feed` against `url`, reading `dir`, with its stdout lines arriving on a channel.
fn watch_feed(dir: &Path, url: &str, token_file: &Path, since: &str) -> (Child, std::sync::mpsc::Receiver<String>) {
    let mut child = cmd(dir)
        .args(["watch", "--follow", "--interval", "2s", "--json", "--since", since, "--sync", url, "--feed"])
        .arg("--token-file")
        .arg(token_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, lines) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    (child, lines)
}

/// The next line that prints a contribution, within `limit`.
fn next_contribution(lines: &std::sync::mpsc::Receiver<String>, limit: Duration) -> Option<Value> {
    let end = Instant::now() + limit;
    loop {
        let line = lines.recv_timeout(end.saturating_duration_since(Instant::now())).ok()?;
        let v: Value = serde_json::from_str(&line).unwrap();
        if v.get("contribution").is_some() {
            return Some(v);
        }
    }
}

#[test]
fn watch_feed_prints_what_the_feed_brings_and_writes_nothing() {
    let g = grounds();
    let bobs = claim(&g.b, "bob", "only on the server");
    let server = g.http(&[]);
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let at = cursor_of(&manyroot::bundle::read(std::fs::read(g.tmp.join("seed.mrb")).unwrap().as_slice()).unwrap().1);
    let (mut watch, lines) = watch_feed(&g.a, &server.url, &token, &at);
    let first = next_contribution(&lines, Duration::from_secs(20)).expect("bob's claim, from the feed");
    assert_eq!(first["contribution"]["id"], bobs.as_str());
    // Read-only: a's replica doesn't hold it.
    assert!(!ids(&g.a).contains(&bobs));
    // A push wakes it.
    let alices = claim(&g.a, "alice", "pushed while watched");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let next = next_contribution(&lines, Duration::from_secs(20)).expect("alice's claim, from the feed");
    assert_eq!(next["contribution"]["id"], alices.as_str());
    watch.kill().unwrap();
    watch.wait().unwrap();
}

/// A server on 127.0.0.1 that answers every request with `status` and `body`.
fn fake_server(status: u16, body: Vec<u8>) -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // The head, then the body its Content-Length says.
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
                let length: usize = header(&head, "content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                if got.len() >= end + 4 + length {
                    break;
                }
            }
            let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&[head.as_bytes(), &body].concat());
        }
    });
    url
}

#[test]
fn watch_feed_drops_a_reply_that_fails_verification_and_falls_back_without_a_feed() {
    let g = grounds();
    claim(&g.b, "bob", "b0");
    claim(&g.b, "bob", "b1");
    let exported = g.tmp.join("b.mrb");
    ok(&g.b, &["export", exported.to_str().unwrap()]);
    let (ground, all) = manyroot::bundle::read(std::fs::read(&exported).unwrap().as_slice()).unwrap();
    let bob = |text: &str| {
        all.iter()
            .find(|c| matches!(c.body(), manyroot::Body::Claim { statement, .. } if statement == text))
            .unwrap()
            .clone()
    };
    let bundle = |ground, cs: Vec<manyroot::Contribution>| {
        let mut out = Vec::new();
        manyroot::bundle::write(&mut out, ground, cs).unwrap();
        out
    };
    // A signature byte changed: the last of the encoding, which ends in the signature.
    let mut bad_signature = bundle(ground, vec![bob("b0")]);
    *bad_signature.last_mut().unwrap() ^= 1;
    // Another ground's bundle.
    let other = g.tmp.join("other");
    ok(&other, &["init", "--name", "other"]);
    let another_ground = bundle(ground_id(&other), vec![bob("b0")]);
    // b1 without b0, its parent.
    let missing_parent = bundle(ground, vec![bob("b1")]);
    // b0 with a byte after its encoding, counted in its length: not canonical.
    let mut not_canonical = bundle(ground, vec![bob("b0")]);
    let len = bob("b0").encode().len();
    let at = not_canonical.len() - len - 4;
    not_canonical[at..at + 4].copy_from_slice(&u32::try_from(len + 1).unwrap().to_be_bytes());
    not_canonical.push(0);
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let at = cursor_of(&manyroot::bundle::read(std::fs::read(g.tmp.join("seed.mrb")).unwrap().as_slice()).unwrap().1);
    let cases = [
        ("bad signature", bad_signature, "signature"),
        ("another ground", another_ground, "another ground"),
        ("missing parent", missing_parent, "lack a parent"),
        ("not canonical", not_canonical, "decoding the reply"),
    ];
    let runs: Vec<_> = cases
        .into_iter()
        .map(|(case, body, says)| {
            let (dir, token, at) = (g.a.clone(), token.clone(), at.clone());
            std::thread::spawn(move || {
                let url = fake_server(200, body);
                let (mut watch, lines) = watch_feed(&dir, &url, &token, &at);
                let printed = next_contribution(&lines, Duration::from_secs(30));
                // Three bad replies, two seconds apart, end it: well within the limit.
                let end = Instant::now() + Duration::from_secs(30);
                while watch.try_wait().unwrap().is_none() && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = watch.kill();
                let status = watch.wait().unwrap();
                let mut stderr = String::new();
                watch.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
                assert!(printed.is_none(), "{case}: printed {printed:?}");
                assert_eq!(status.code(), Some(1), "{case}: {stderr}");
                assert!(stderr.contains("failed verification") && stderr.contains(says), "{case}: {stderr}");
            })
        })
        .collect();
    for run in runs {
        run.join().unwrap();
    }
    // A server without the feed is synced with instead: here it refuses that too.
    let url = fake_server(404, b"no such ground here\n".to_vec());
    let (mut watch, _lines) = watch_feed(&g.a, &url, &token, &at);
    let mut said = BufReader::new(watch.stderr.take().unwrap()).lines();
    let first = said.next().unwrap().unwrap();
    let second = said.next().unwrap().unwrap();
    assert!(watch.try_wait().unwrap().is_none(), "it stopped");
    watch.kill().unwrap();
    watch.wait().unwrap();
    assert!(first.contains("has no change feed (404); syncing with it instead"), "{first}");
    assert!(second.contains("not synced with"), "{second}");
}

/// A proxy on 127.0.0.1 in front of `port` that counts the requests it passes on.
fn counting_proxy(port: u16) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&count);
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { continue };
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::spawn(move || {
                let Ok(upstream) = TcpStream::connect(("127.0.0.1", port)) else { return };
                let (mut c, mut u) = (client.try_clone().unwrap(), upstream.try_clone().unwrap());
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut c, &mut u);
                    let _ = u.shutdown(std::net::Shutdown::Write);
                });
                let (mut u, mut c) = (upstream, client);
                let _ = std::io::copy(&mut u, &mut c);
                let _ = c.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    (url, count)
}

/// Finding 6552f9b16c65: with a head of the watching replica that the server lacks, the first reply brings
/// back what is here already, and from what it brought the next cursor holds the server's heads too, so
/// the request waits; and once the head reaches the server, it counts as seen and the request still
/// waits, rather than coming back at once, again and again.
#[test]
fn watch_feed_waits_while_and_after_a_local_head_reaches_the_server() {
    let g = grounds();
    let server = g.http(&[]);
    // A claim both hold, and one a holds alone.
    claim(&g.a, "alice", "on both");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    claim(&g.a, "alice", "not pushed yet");
    let (url, requests) = counting_proxy(server.port);
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let (mut watch, _lines) = watch_feed(&g.a, &url, &token, "");
    let count = || requests.load(std::sync::atomic::Ordering::SeqCst);
    // The first reply names the head and brings back what a holds (it came at once with nothing new, so
    // the next waits --interval, 2 s); the second, without it in what's seen, waits.
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(count(), 2, "requests while the head is only a's");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    std::thread::sleep(Duration::from_secs(4));
    let after = count();
    assert!(watch.try_wait().unwrap().is_none(), "watch stopped");
    watch.kill().unwrap();
    watch.wait().unwrap();
    assert!(after <= 3, "{after} requests once the head reached the server");
}

/// Finding d4f10edc63fb: with a chain of a's own work the server lacks, watch learns what the server
/// holds from the one reply that re-sends it, and from then on waits. It doesn't forget, so the server
/// isn't asked from the start again: two requests in eight seconds, the second still waiting.
#[test]
fn watch_feed_with_an_unpushed_chain_resends_once_then_waits() {
    let g = grounds();
    let server = g.http(&[]);
    for i in 0..5 {
        claim(&g.a, "alice", &format!("on both {i}"));
    }
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    // Each claim's parents are the replica's heads, so these two make a chain.
    claim(&g.a, "alice", "not pushed yet");
    claim(&g.a, "alice", "nor this, after it");
    let (url, requests) = counting_proxy(server.port);
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let (mut watch, _lines) = watch_feed(&g.a, &url, &token, "");
    std::thread::sleep(Duration::from_secs(8));
    let count = requests.load(std::sync::atomic::Ordering::SeqCst);
    assert!(watch.try_wait().unwrap().is_none(), "watch stopped");
    watch.kill().unwrap();
    watch.wait().unwrap();
    assert_eq!(count, 2, "requests with an unpushed chain of two");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Finding 2e421e838287: a server restarted with less (from a copy taken before the last push) lacks the
/// head watch knew it held. Its one reply of what watch holds already replaces what watch knew, so the
/// request after it waits again, rather than bringing the same back every --interval; and what reaches
/// the server next is printed.
#[test]
fn watch_feed_recovers_when_the_server_restarts_with_less() {
    let g = grounds();
    for i in 0..3 {
        claim(&g.a, "alice", &format!("shared {i}"));
    }
    let server = g.http(&[]);
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    // A copy of the server's replica, and the watching replica, both before the last push.
    let (before, w) = (g.tmp.join("b-before"), g.tmp.join("w"));
    copy_dir(&g.b, &before);
    copy_dir(&g.b, &w);
    claim(&g.a, "alice", "the last push");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let (url, requests) = counting_proxy(server.port);
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let (mut watch, lines) = watch_feed(&w, &url, &token, "");
    // First what w holds, then the last push, from the feed.
    while next_contribution(&lines, Duration::from_secs(20)).expect("the last push, from the feed")["contribution"]["body"]
        ["statement"]
        != "the last push"
    {}
    // The server comes back on the same port from the copy: it lacks the last push.
    let port = server.port.to_string();
    drop(server);
    let server = g.http_for(&before, &["--port", &port]);
    let restarted = requests.load(std::sync::atomic::Ordering::SeqCst);
    // The waiting request fails, the next brings back what watch holds, and the one after waits:
    // each of the first two is followed by --interval, 2 s.
    std::thread::sleep(Duration::from_secs(10));
    let count = requests.load(std::sync::atomic::Ordering::SeqCst) - restarted;
    assert!(count <= 3, "{count} requests in 10 s against the server with less");
    // New work reaching it is printed.
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let new = claim(&g.a, "alice", "after the restart");
    let out = sync_http(&g.a, &server.url, TOKEN, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = next_contribution(&lines, Duration::from_secs(20)).expect("new work, after the restart");
    assert_eq!(printed["contribution"]["id"], new.as_str());
    watch.kill().unwrap();
    watch.wait().unwrap();
}

/// Finding ab41112b47f2: a catch-up of several pages, each of only what watch holds already, adds each
/// page to what watch knows the server holds; replacing it with each page dropped an earlier one, and
/// paging cycled. Three concurrent 5 MiB artifacts (one per 8 MiB page) and a claim merging them, which
/// the watching replica holds along with work of its own the server lacks: once it has paged through
/// them, new work reaching the server is printed.
#[test]
fn watch_feed_pages_through_a_catch_up_of_concurrent_large_contributions() {
    let g = grounds();
    let server = g.http(&[]);
    let sync = |dir: &Path| {
        let out = sync_http(dir, &server.url, TOKEN, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
    };
    claim(&g.a, "alice", "the root");
    sync(&g.a);
    // Each artifact is made on its own copy of a, so none is the others' parent.
    for (i, byte) in b"xyz".iter().copied().enumerate() {
        let copy = g.tmp.join(format!("a{i}"));
        copy_dir(&g.a, &copy);
        let file = g.tmp.join(format!("big{i}"));
        std::fs::write(&file, vec![byte; 5 << 20]).unwrap();
        ok(&copy, &["artifact", file.to_str().unwrap(), "--as", "alice"]);
        sync(&copy);
    }
    sync(&g.a);
    claim(&g.a, "alice", "merging the three");
    sync(&g.a);
    let w = g.tmp.join("w");
    copy_dir(&g.a, &w);
    claim(&w, "alice", "not pushed yet");
    let token = g.tmp.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    let (mut watch, lines) = watch_feed(&w, &server.url, &token, "");
    // The pages take a few rounds of --interval (2 s); then the request waits, and new work wakes it.
    std::thread::sleep(Duration::from_secs(12));
    let new = claim(&g.a, "alice", "after the catch-up");
    sync(&g.a);
    let end = Instant::now() + Duration::from_secs(40);
    let printed = loop {
        let left = end.saturating_duration_since(Instant::now());
        match next_contribution(&lines, left) {
            Some(v) if v["contribution"]["id"] == new.as_str() => break true,
            Some(_) => continue,
            None => break false,
        }
    };
    watch.kill().unwrap();
    watch.wait().unwrap();
    assert!(printed, "new work never arrived: the catch-up didn't finish");
}
