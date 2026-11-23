//! The Weave pages' API in `bracken view` (docs/design/weave-view.md), end to end on 127.0.0.1 against
//! throwaway Weave Grounds this test makes with its own binary: never this repository's `.bracken/`, and
//! never the real Weave Ground. Read-only: a session of requests changes no file under the Ground.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use bracken_tree::format::{Guardrails, LINE_MEDIA_TYPE, Line, Record, VERSION};
use serde_json::Value;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_bracken");
const AS: &str = "weave-integrator";
/// An owner decision's id, as a line's policy cites one.
const OD: &str = "0123456789abcdef";
const HOSTILE: &str = "</script><img src=x onerror=alert(1)> & \u{1b}[2K\u{202e}txt.exe \u{2028}\"quoted\"";

/// A throwaway Weave Ground at `<tmp>/g`, fed from a git repository at `<tmp>/repo`.
struct Weave {
    tmp: TempDir,
    ground: PathBuf,
    repo: PathBuf,
}

impl Weave {
    fn new() -> Weave {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let ground = tmp.path().join("g");
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir repo");
        let w = Weave { tmp, ground, repo };
        w.git(&["init", "-q", "-b", "main"]);
        w.ok(&["weave", "init", "--as", AS]);
        w
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(BIN)
            .args(args)
            .current_dir(&self.repo)
            .env("BRACKEN_DIR", &self.ground)
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("bracken runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).expect("JSON")
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// Commits `files` and records the commit as the default line's next decision, with `note`.
    fn decide(&self, files: &[(&str, &str)], note: &str) -> String {
        for (path, text) in files {
            let file = self.repo.join(path);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
            std::fs::write(file, text).expect("write");
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", "c"]);
        let out = self.json(&["weave", "integrate", "--as", AS, "--note", note, "--json"]);
        out["decision"].as_str().expect("a decision id").to_owned()
    }

    /// A Weave Ground with two decisions on main, the second noted with hostile text.
    fn with_two_decisions() -> (Weave, String, String) {
        let w = Weave::new();
        let first = w.decide(&[("README.md", "hello\n"), ("src/lib.rs", "pub fn a() {}\n")], "first");
        let second = w.decide(&[("odd/x\u{202e}y.txt", "tricky\n")], HOSTILE);
        (w, first, second)
    }

    /// The id a `--json` command prints under `key`.
    fn id_of(&self, args: &[&str], key: &str) -> String {
        self.json(args)[key].as_str().unwrap_or_else(|| panic!("{args:?} prints {key}")).to_owned()
    }

    fn main_line(&self) -> String {
        let lines = self.json(&["weave", "line", "list", "--json"]);
        lines["lines"][0]["line"].as_str().expect("a line").to_owned()
    }
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

/// `bracken view` on `dir`, once it has printed where it listens: within 60 seconds, or the test fails
/// (a view that blocked reading the replica would never print).
fn view(dir: &Path) -> View {
    view_with(dir, &[])
}

/// [`view`] with environment variables set: the view's test hooks.
fn view_with(dir: &Path, env: &[(&str, &str)]) -> View {
    let mut child = Command::new(BIN)
        .arg("view")
        .envs(env.iter().copied())
        .env("BRACKEN_DIR", dir)
        .env_remove("BRACKEN_AGENT")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("bracken view starts");
    let stdout = child.stdout.take().expect("its stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_default();
    let port = line
        .trim()
        .strip_prefix("view at http://127.0.0.1:")
        .and_then(|p| p.strip_suffix('/'))
        .and_then(|p| p.parse().ok());
    let Some(port) = port else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("bracken view didn't start: {line:?}");
    };
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

/// What a Weave page answers while its state is being read (answer a300d75086d5).
const READING: &str = "reading the Weave records";

impl View {
    /// One request; a Weave page still reading its state is asked again, for up to 60 seconds.
    fn request(&self, path: &str) -> Reply {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let r = self.request_once(path);
            let reading = r.status == 503 && String::from_utf8_lossy(&r.body).contains(READING);
            if !reading || Instant::now() > deadline {
                return r;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn request_once(&self, path: &str) -> Reply {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("connects");
        s.set_read_timeout(Some(Duration::from_secs(30))).expect("a timeout");
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", self.port).expect("writes");
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).expect("reads");
        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a head");
        let head = String::from_utf8(raw[..end].to_vec()).expect("an ASCII head");
        let mut lines = head.lines();
        let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).expect("a status");
        let headers =
            lines.filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned())).collect();
        Reply { status, headers, body: raw[end + 4..].to_vec() }
    }

    fn api(&self, path: &str) -> Value {
        let r = self.request(path);
        assert_eq!(r.status, 200, "{path}: {}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.headers["content-type"], "application/json", "{path}");
        assert!(r.headers.contains_key("content-security-policy"), "{path}");
        assert_eq!(r.headers["x-content-type-options"], "nosniff", "{path}");
        r.json()
    }

    fn refused(&self, path: &str, status: u16) -> String {
        let r = self.request(path);
        assert_eq!(r.status, status, "{path}: {}", String::from_utf8_lossy(&r.body));
        r.json()["error"].as_str().unwrap_or_default().to_owned()
    }
}

/// Every file under `dir`, with its bytes (a link or a special file, by its type).
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("lists") {
            let p = e.expect("an entry").path();
            let kind = std::fs::symlink_metadata(&p).expect("its type").file_type();
            if kind.is_dir() {
                stack.push(p);
            } else if kind.is_file() {
                out.insert(p.clone(), std::fs::read(&p).expect("reads"));
            } else {
                out.insert(p.clone(), format!("{kind:?}").into_bytes());
            }
        }
    }
    out
}

#[test]
fn the_weave_pages_show_lines_decisions_and_records() {
    let (w, first, second) = Weave::with_two_decisions();
    let line = w.main_line();
    let v = view(&w.ground);

    let home = v.api("/api/weave");
    assert_eq!(home["weave"], true);
    assert_eq!(home["default_line"], line.as_str());
    assert_eq!(home["lines"]["total"], 1);
    let row = &home["lines"]["items"][0];
    assert_eq!(
        (row["name"].as_str(), row["default"].as_bool(), row["decisions"].as_u64()),
        (Some("main"), Some(true), Some(2))
    );
    let heads: Vec<&str> =
        home["default"]["heads"].as_array().unwrap().iter().map(|h| h["decision"].as_str().unwrap()).collect();
    assert_eq!(heads, [second.as_str()]);
    // The same green as `weave green --json`: a line whose policy names no checks is green at its head.
    assert_eq!(home["default"]["green"], w.json(&["weave", "green", "--json"]));
    assert_eq!(home["default"]["green"]["state"], "one");
    assert_eq!(home["default"]["forked"], false);

    let l = v.api(&format!("/api/weave/line/{line}"));
    assert_eq!(
        (l["name"].as_str(), l["decisions"].as_u64(), l["diverged"].as_bool()),
        (Some("main"), Some(2), Some(false))
    );
    assert_eq!(l["current"]["items"][0]["version"], line.as_str());
    assert_eq!(l["versions"]["total"], 1);
    assert_eq!(l["goals"]["total"], 0);
    assert_eq!(l["open_objections"], 0);
    // The same green as `weave green --json`.
    assert_eq!(l["green"], w.json(&["weave", "green", "--json"]));

    // The log, a page at a time, newest first, in `weave log --json`'s rows.
    let page = v.api(&format!("/api/weave/log/{line}?limit=1"));
    assert_eq!((page["total"].as_u64(), page["more"].as_bool()), (Some(2), Some(true)));
    assert_eq!(page["items"][0]["decision"], second.as_str());
    let next = v.api(&format!("/api/weave/log/{line}?limit=1&after={second}"));
    assert_eq!((next["items"][0]["decision"].as_str(), next["more"].as_bool()), (Some(first.as_str()), Some(false)));
    let cli = w.json(&["weave", "log", "--json"]);
    assert_eq!(cli["decisions"][1]["note"], next["items"][0]["note"]);
    assert_eq!(page["items"][0]["note"], HOSTILE, "Ground text comes back as it is, as a JSON string");
    assert_eq!(page["items"][0]["parents_total"], 1);

    let d = v.api(&format!("/api/weave/decision/{}", &second[..12]));
    assert_eq!(d["decision"], second.as_str());
    assert_eq!(d["verdict"], "qualifies");
    assert_eq!(d["line"], line.as_str());
    assert_eq!(d["line_name"], "main");
    assert_eq!(d["policy"], line.as_str());
    assert_eq!(d["note"], HOSTILE);
    assert_eq!(d["reviews"]["total"], 0);
    assert_eq!(d["parents"][0], first.as_str());

    let r = v.api(&format!("/api/weave/record/{}", &first[..10]));
    assert_eq!((r["kind"].as_str(), r["page"].as_str()), (Some("decision"), Some("decision")));
    let r = v.api(&format!("/api/weave/record/{}", &line[..10]));
    assert_eq!((r["kind"].as_str(), r["page"].as_str()), (Some("line"), Some("line")));
    let manifest = d["manifest"].as_str().unwrap();
    let r = v.api(&format!("/api/weave/record/{manifest}"));
    assert_eq!(r["kind"], "manifest");

    // Markup never reaches the bytes: <, > and & go out as \u escapes.
    for path in [format!("/api/weave/log/{line}"), format!("/api/weave/decision/{second}")] {
        let body = v.request(&path).body;
        for bad in *b"<>&" {
            assert!(!body.contains(&bad), "{path} carries a raw {}", bad as char);
        }
    }
}

#[test]
fn every_weave_list_pages_and_bad_parameters_are_refused() {
    let (w, first, _) = Weave::with_two_decisions();
    let line = w.main_line();
    let v = view(&w.ground);
    for (path, status, why) in [
        ("/api/weave?limit=0".to_owned(), 400, "limit is a whole number"),
        ("/api/weave?list=lines&limit=2001".to_owned(), 400, "limit is a whole number"),
        (format!("/api/weave/log/{line}?limit=201"), 400, "at most 200"),
        ("/api/weave?list=problems&limit=201".to_owned(), 400, "at most 200"),
        (format!("/api/weave/log/{line}?after=00ff"), 400, "after names no item"),
        ("/api/weave?list=everything".to_owned(), 400, "list is one of"),
        ("/api/weave?after=x".to_owned(), 400, "name it with list"),
        (format!("/api/weave/log/{line}?list=decisions"), 400, "takes no parameter"),
        ("/api/weave/record/abc".to_owned(), 404, "at least 8 hex"),
        (format!("/api/weave/line/{first}"), 404, "not a line"),
        (format!("/api/weave/decision/{line}"), 404, "not a decision"),
        ("/api/weave/nowhere".to_owned(), 404, "no such Weave page"),
    ] {
        let said = v.refused(&path, status);
        assert!(said.contains(why), "{path}: {said}");
    }
    // A list's own page: the lines after the only one are none.
    let rest = v.api(&format!("/api/weave?list=lines&after={line}"));
    assert_eq!(
        (rest["list"].as_str(), rest["items"].as_array().map(Vec::len), rest["more"].as_bool()),
        (Some("lines"), Some(0), Some(false))
    );
}

/// Each line's log is the CLI's, though the view sorts the graph once for every line together.
#[test]
fn each_lines_log_is_the_clis() {
    let (w, ..) = Weave::with_two_decisions();
    for name in ["side", "other"] {
        w.ok(&["weave", "line", "create", name, "--as", AS, "--owner-decision", "0123456789ab"]);
        w.ok(&["weave", "integrate", "--line", name, "--as", AS, "--note", name]);
    }
    w.decide(&[("more.txt", "more\n")], "on main again");
    let v = view(&w.ground);
    let home = v.api("/api/weave");
    assert_eq!(home["lines"]["total"], 3);
    for row in home["lines"]["items"].as_array().unwrap() {
        let id = row["line"].as_str().unwrap();
        let cli = w.json(&["weave", "log", "--line", id, "--limit", "100", "--json"]);
        let ours = v.api(&format!("/api/weave/log/{id}"));
        let ids = |list: &Value| -> Vec<String> {
            list.as_array().unwrap().iter().map(|d| d["decision"].as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(ids(&ours["items"]), ids(&cli["decisions"]), "line {}", row["name"]);
        assert_eq!(row["decisions"].as_u64(), Some(ids(&cli["decisions"]).len() as u64));
    }
}

#[test]
fn a_ground_without_weave_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("g");
    let out = Command::new(BIN).args(["init", "--name", "plain"]).env("BRACKEN_DIR", &dir).output().unwrap();
    assert!(out.status.success());
    let v = view(&dir);
    let home = v.api("/api/weave");
    assert_eq!(home["weave"], false);
    assert!(home["why"].as_str().unwrap().contains("no Weave root"), "{home}");
    v.refused("/api/weave/line/0123456789abcdef", 404);
}

/// Read-only: a session of requests over every Weave endpoint, and the Loom's, leaves every file under the
/// Ground as it was: same names, same bytes, `cache/` included.
#[test]
fn a_session_of_requests_changes_no_file_under_the_ground() {
    let (w, first, second) = Weave::with_two_decisions();
    let line = w.main_line();
    w.ok(&["log", "--limit", "1"]); // so the snapshot is up to date before the view starts
    let before = files(&w.ground);
    assert!(before.keys().any(|p| p.ends_with("cache/snapshot")), "a snapshot to keep: {:?}", before.keys());
    {
        let v = view(&w.ground);
        for path in [
            "/api/weave".to_owned(),
            "/api/weave?list=lines".to_owned(),
            "/api/weave?list=problems".to_owned(),
            format!("/api/weave/line/{line}"),
            format!("/api/weave/line/{line}?list=versions"),
            format!("/api/weave/log/{line}"),
            format!("/api/weave/decision/{first}"),
            format!("/api/weave/decision/{second}?list=reviews"),
            format!("/api/weave/record/{line}"),
            "/api/ground".to_owned(),
            "/api/graph".to_owned(),
            "/weave.html".to_owned(),
        ] {
            assert_eq!(v.request(&path).status, 200, "{path}");
        }
        std::thread::sleep(Duration::from_millis(1_500)); // a watcher poll or so
    }
    assert_eq!(files(&w.ground), before, "the view changed a file under the Ground");
}

/// The view never follows a link at weave-root and never blocks on a FIFO there: it starts, serves the
/// Loom, and says why it has no Weave pages (finding 53fca000203f).
#[test]
fn a_weave_root_link_or_fifo_gives_no_weave_and_doesnt_block() {
    let (w, ..) = Weave::with_two_decisions();
    let root = w.ground.join("weave-root");
    let real = w.tmp.path().join("real-root");
    std::fs::rename(&root, &real).unwrap();
    std::os::unix::fs::symlink(&real, &root).unwrap();
    {
        let v = view(&w.ground);
        let home = v.api("/api/weave");
        assert_eq!(home["weave"], false);
        assert!(home["why"].as_str().unwrap().contains("symbolic link"), "{home}");
        v.api("/api/ground");
    }
    std::fs::remove_file(&root).unwrap();
    assert!(Command::new("mkfifo").arg(&root).status().unwrap().success());
    let v = view(&w.ground);
    let home = v.api("/api/weave");
    assert_eq!(home["weave"], false);
    assert!(home["why"].as_str().unwrap().contains("isn't a regular file"), "{home}");
}

/// `weave root set` renames a new weave-root into place, which adds no object: the view's signature
/// counts weave-root too, so the Weave pages follow it.
/// The Weave state is read on the watcher thread, the old one dropped first: while a re-read runs, the
/// Weave pages answer 503 with a Retry-After and the other pages keep answering; then the Weave pages
/// answer again, from the new snapshot.
#[test]
fn while_the_weave_state_is_read_its_pages_say_so_and_the_others_answer() {
    let (w, ..) = Weave::with_two_decisions();
    let v = view_with(&w.ground, &[("BRACKEN_VIEW_REREAD_DELAY_MS", "3000")]);
    assert_eq!(v.api("/api/weave")["weave"], true);
    // A change the signature sees: weave-root's times.
    let root = w.ground.join("weave-root");
    let text = std::fs::read(&root).expect("reads weave-root");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&root, text).expect("rewrites weave-root");
    let deadline = Instant::now() + Duration::from_secs(30);
    let reading = loop {
        let r = v.request_once("/api/weave");
        if r.status == 503 {
            break r;
        }
        assert!(Instant::now() < deadline, "the view never re-read the replica");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(String::from_utf8_lossy(&reading.body).contains(READING));
    assert_eq!(reading.headers["retry-after"], "2");
    assert_eq!(v.request_once("/api/ground").status, 200, "the other pages answer meanwhile");
    assert_eq!(v.request_once("/weave.html").status, 200);
    assert_eq!(v.api("/api/weave")["weave"], true, "then the Weave pages answer again");
}

#[test]
fn the_view_follows_weave_root() {
    let (w, ..) = Weave::with_two_decisions();
    let root = w.ground.join("weave-root");
    let aside = w.tmp.path().join("aside");
    std::fs::rename(&root, &aside).unwrap();
    let v = view(&w.ground);
    assert_eq!(v.api("/api/weave")["weave"], false);
    std::fs::rename(&aside, &root).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while v.api("/api/weave")["weave"] != true {
        assert!(Instant::now() < deadline, "the view didn't see weave-root come back");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Finding 9f37a1b9146c: a re-read that fails leaves the Weave pages unavailable, saying why, and never
/// rebuilds the Weave state from the old graph against what is on disk now; the other pages keep answering.
#[test]
fn a_failed_re_read_leaves_the_weave_pages_unavailable_and_says_why() {
    let (w, ..) = Weave::with_two_decisions();
    w.ok(&["id", "new", "val"]);
    w.ok(&["claim", "a second author", "--as", "val"]);
    let v = view(&w.ground);
    assert_eq!(v.api("/api/weave")["weave"], true);
    // A directory entry naming another's key (as tests/view.rs's a_name_two_keys_claim_is_refused): the
    // replica no longer reads.
    let val: Value =
        serde_json::from_str(&std::fs::read_to_string(w.ground.join("agents/val.json")).expect("val's entry"))
            .expect("JSON");
    let forged = serde_json::json!({"name": AS, "agent": val["agent"]});
    std::fs::write(w.ground.join("agents/mallory.json"), forged.to_string()).expect("writes the entry");
    let deadline = Instant::now() + Duration::from_secs(30);
    let failed = loop {
        let r = v.request_once("/api/weave");
        if r.status == 503 && !String::from_utf8_lossy(&r.body).contains(READING) {
            break r;
        }
        assert!(Instant::now() < deadline, "the Weave pages never said the re-read failed: {}", r.status);
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        failed.json()["error"].as_str().unwrap_or_default().contains("couldn't be read again"),
        "{}",
        failed.json()
    );
    // Over several polls, the Weave pages stay unavailable: no stale state comes back as 200.
    let until = Instant::now() + Duration::from_secs(4);
    while Instant::now() < until {
        assert_eq!(v.request_once("/api/weave").status, 503, "a stale Weave state was served");
        assert_eq!(v.request_once("/api/ground").status, 200, "the other pages keep answering");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Finding 80050786bf97: a policy too big for a row keeps its ids whole. Its lists are bounded by count in
/// a row (5 and the total), and the line pages the current policy's checks in full, by check name.
#[test]
fn a_wide_policy_keeps_its_ids_whole_and_pages_its_checks() {
    let (w, ..) = Weave::with_two_decisions();
    let long = "x".repeat(300);
    let checks: Vec<String> = (0..30).map(|i| format!("check{i:02}=run --flag={long} --more={long}")).collect();
    let mut args = vec!["weave", "line", "create", "wide", "--owner-decision", OD, "--as", AS, "--json"];
    for c in &checks {
        args.extend(["--check", c.as_str()]);
    }
    let wide = w.id_of(&args, "line");
    let author = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    let v = view(&w.ground);
    let page = v.api(&format!("/api/weave/line/{wide}"));
    for row in [&page["current"]["items"][0], &page["versions"]["items"][0]] {
        assert_eq!(row["version"], wide.as_str(), "the version's id is whole");
        assert_eq!(row["author"], author.as_str(), "the author's key is whole");
        assert_eq!(row["checks_total"], 30);
        assert_eq!(row["checks"].as_array().expect("checks").len(), 5);
        assert!(serde_json::to_vec(row).expect("JSON").len() <= 24 * 1024, "a nested row is at most 24 KiB");
    }
    let listed = v.api(&format!("/api/weave/line/{wide}?list=versions"));
    assert_eq!(listed["items"][0]["version"], wide.as_str());
    // The checks, page by page, after each page's last check name.
    assert_eq!(page["policy"]["checks"]["total"], 30);
    let mut names = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let q = match &after {
            Some(a) => format!("?list=checks&limit=7&after={a}"),
            None => "?list=checks&limit=7".to_owned(),
        };
        let got = v.api(&format!("/api/weave/line/{wide}{q}"));
        for c in got["items"].as_array().expect("items") {
            assert_eq!(c["argv"][1].as_str().map(str::len), Some(7 + 300), "argv kept whole within its row");
            names.push(c["check"].as_str().expect("a name").to_owned());
        }
        if got["more"] != true {
            break;
        }
        after = names.last().cloned();
    }
    let want: Vec<String> = (0..30).map(|i| format!("check{i:02}")).collect();
    assert_eq!(names, want);
}

/// Finding e7e9850e4d63: another author's version isn't a line's successor, though the line's author
/// relates it, as `Lines` (crates/tree/src/lines.rs) chains only the line's author's versions.
#[test]
fn another_authors_version_is_not_a_lines_successor() {
    let (w, ..) = Weave::with_two_decisions();
    w.ok(&["id", "new", "bob"]);
    let one = w.id_of(&["weave", "line", "create", "one", "--owner-decision", OD, "--as", AS, "--json"], "line");
    let two = w.id_of(&["weave", "line", "create", "two", "--owner-decision", OD, "--as", "bob", "--json"], "line");
    w.ok(&["relate", &two, "supersedes", &one, "--as", AS]);
    let v = view(&w.ground);
    let page = v.api(&format!("/api/weave/line/{one}"));
    assert_eq!(page["versions"]["total"], 1, "{}", page["versions"]);
    assert_eq!(page["versions"]["items"][0]["version"], one.as_str());
}

/// Finding 08c43e704dff: a line whose policy has diverged has no single current policy, so its open
/// objections and a decision's counted checks are null, as the CLI gives, never 0 or [].
#[test]
fn a_diverged_policy_gives_null_not_none() {
    let (w, ..) = Weave::with_two_decisions();
    let dev = w.id_of(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS, "--json"], "line");
    let decision =
        w.id_of(&["weave", "integrate", "--line", "dev", "--as", AS, "--note", "on dev", "--json"], "decision");
    w.ok(&["weave", "line", "policy", "dev", "--max-known", "1", "--owner-decision", OD, "--as", AS]);
    // A second successor of the first version, written by hand, diverges the chain.
    let me = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    let other = Line {
        v: VERSION,
        name: "dev".into(),
        integrators: vec![me.parse().expect("a key")],
        validators: vec![me.parse().expect("a key")],
        goal_setters: vec![me.parse().expect("a key")],
        checks: BTreeMap::new(),
        executors: BTreeMap::new(),
        guardrails: Guardrails { max_known: 2, max_age: 0, protected: vec![] },
        owner_decision: Some(OD.into()),
    };
    let file = w.tmp.path().join("other.cbor");
    std::fs::write(&file, other.encode()).expect("writes the policy");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", LINE_MEDIA_TYPE, "--as", AS]);
    let other = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    w.ok(&["relate", &other, "supersedes", &dev, "--as", AS]);
    let cli = w.json(&["weave", "green", "--line", &dev, "--json"]);
    assert!(cli["heads"][0]["checks"].is_null(), "the CLI's answer: {cli}");

    let v = view(&w.ground);
    let line = v.api(&format!("/api/weave/line/{dev}"));
    assert_eq!(line["diverged"], true);
    assert_eq!(line["current"]["total"], 2);
    assert!(line["open_objections"].is_null(), "{}", line["open_objections"]);
    assert!(line["policy"]["checks"].is_null(), "no single current policy to page");
    let page = v.api(&format!("/api/weave/decision/{decision}"));
    assert!(page["counted"].is_null(), "{}", page["counted"]);
    v.refused(&format!("/api/weave/decision/{decision}?list=counted"), 404);
}
