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

use bracken_tree::ContentHash;
use bracken_tree::format::{
    DECISION_MEDIA_TYPE, Decision, Entry, FILE_MEDIA_TYPE, Guardrails, LINE_MEDIA_TYPE, Line, MANIFEST_MEDIA_TYPE,
    Manifest, Mode, PROPOSAL_MEDIA_TYPE, Proposal, Record, VERSION,
};
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
        self.git_out(args);
    }

    /// `git` in the repository, with its output.
    fn git_out(&self, args: &[&str]) -> String {
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
        String::from_utf8(out.stdout).expect("UTF-8")
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
            // The tree and file endpoints, with each of their parameters (finding df1009d45d54).
            format!("/api/weave/tree/{second}"),
            format!("/api/weave/tree/{second}?dir=src"),
            format!("/api/weave/tree/{second}?list=children&after=src/&limit=5"),
            format!("/api/weave/tree/{second}?dir=src&list=missing"),
            format!("/api/weave/file/{second}?path=README.md"),
            format!("/api/weave/file/{first}?path=src/lib.rs"),
            // History and diffs, with each of their parameters.
            format!("/api/weave/history/{line}?path=README.md"),
            format!("/api/weave/history/{line}?path=README.md&after={first}&limit=5"),
            format!("/api/weave/diff?a={first}&b={second}"),
            format!("/api/weave/diff?a={first}&b={second}&list=files&after={}&limit=5", encode("odd/x\u{202e}y.txt")),
            format!("/api/weave/diff?a={first}&b={second}&path=README.md"),
            format!("/api/weave/diff?a={first}&b={second}&path={}", encode("odd/x\u{202e}y.txt")),
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

/// Finding c50605c13cd4: once the change that failed to read goes away, the replica is what the view last
/// read, and the Weave pages come back.
#[test]
fn the_weave_pages_come_back_when_a_failed_change_is_undone() {
    let (w, ..) = Weave::with_two_decisions();
    w.ok(&["id", "new", "val"]);
    w.ok(&["claim", "a second author", "--as", "val"]);
    let v = view(&w.ground);
    assert_eq!(v.api("/api/weave")["weave"], true);
    let val: Value =
        serde_json::from_str(&std::fs::read_to_string(w.ground.join("agents/val.json")).expect("val's entry"))
            .expect("JSON");
    let forged = w.ground.join("agents/mallory.json");
    std::fs::write(&forged, serde_json::json!({"name": AS, "agent": val["agent"]}).to_string())
        .expect("writes the entry");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let r = v.request_once("/api/weave");
        if r.status == 503 && !String::from_utf8_lossy(&r.body).contains(READING) {
            break;
        }
        assert!(Instant::now() < deadline, "the re-read never failed");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_file(&forged).expect("undoes the change");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let r = v.request_once("/api/weave");
        if r.status == 200 {
            assert_eq!(r.json()["weave"], true);
            break;
        }
        assert!(Instant::now() < deadline, "the Weave pages stayed unavailable: {}", String::from_utf8_lossy(&r.body));
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Finding 886fba8945e1: a protected path is its own key for `after`, so it is never cut, even at the
/// longest a path may be; paging past it reaches every path.
#[test]
fn protected_paths_page_whole_at_the_longest_a_path_may_be() {
    let (w, ..) = Weave::with_two_decisions();
    let longest = vec!["a".repeat(255); 16].join("/");
    assert_eq!(longest.len(), 4095);
    let paths: Vec<String> = (0..51).map(|i| if i == 49 { longest.clone() } else { format!("p{i:02}") }).collect();
    let mut args = vec!["weave", "line", "create", "guarded", "--owner-decision", OD, "--as", AS, "--json"];
    for p in &paths {
        args.extend(["--protected", p.as_str()]);
    }
    let line = w.id_of(&args, "line");
    let v = view(&w.ground);
    let first = v.api(&format!("/api/weave/line/{line}"));
    let protected = &first["policy"]["protected"];
    assert_eq!(protected["total"], 51);
    let shown: Vec<String> =
        protected["items"].as_array().expect("items").iter().map(|p| p.as_str().expect("a path").to_owned()).collect();
    assert!(shown.contains(&longest), "the longest path comes back whole");
    let last = shown.last().expect("a page").clone();
    let rest = v.api(&format!("/api/weave/line/{line}?list=protected&after={}&limit=50", encode(&last)));
    let mut all = shown;
    all.extend(rest["items"].as_array().expect("items").iter().map(|p| p.as_str().expect("a path").to_owned()));
    let mut want = paths.clone();
    want.sort();
    let mut got = all.clone();
    got.sort();
    assert_eq!(got, want, "every protected path is reached");
}

/// `s` as a query value: everything but unreserved characters percent-encoded.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// ---- Trees and files (PR (c)) ----

/// A Weave Ground whose head decision's tree holds a text file, an HTML file, a binary file, an oversized
/// file and a symlink to text, nested two directories deep. Import refuses a symlink whose target isn't
/// UTF-8, so that case is a hand-made manifest's (`hand_made_manifests_name_what_they_cant_show`).
fn with_a_tree() -> (Weave, String) {
    let w = Weave::new();
    let files: &[(&str, &[u8])] = &[
        ("README.md", b"hello\nworld"),
        ("page.html", b"<script>alert(1)</script>\n<img src=x onerror=alert(2)>\n"),
        ("src/lib.rs", b"pub fn a() {}\n"),
        ("src/deep/mod.rs", b"\xe2\x80\xaecode\n"),
        ("blob.bin", b"\x00\x01\x02 not text"),
    ];
    for (path, bytes) in files {
        let file = w.repo.join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
        std::fs::write(file, bytes).expect("write");
    }
    std::fs::write(w.repo.join("big.txt"), "x\n".repeat((1 << 19) + 1)).expect("write");
    w.git(&["add", "-A"]);
    // A symlink staged straight into the index, so the test needn't make one on disk.
    let blob = w.tmp.path().join("link.target");
    std::fs::write(&blob, "src/lib.rs").expect("write");
    let sha = w.git_out(&["hash-object", "-w", blob.to_str().expect("UTF-8")]);
    w.git(&["update-index", "--add", "--cacheinfo", &format!("120000,{},link", sha.trim())]);
    w.git(&["commit", "-q", "-m", "tree"]);
    let out = w.json(&["weave", "integrate", "--as", AS, "--note", "a tree", "--json"]);
    (w, out["decision"].as_str().expect("a decision").to_owned())
}

#[test]
fn a_tree_lists_directories_then_files_and_pages_by_name() {
    let (w, d) = with_a_tree();
    let v = view(&w.ground);
    let root = v.api(&format!("/api/weave/tree/{d}"));
    assert_eq!(root["files"], 7);
    assert_eq!(root["complete"], true);
    let names: Vec<(String, bool)> = root["children"]["items"]
        .as_array()
        .expect("children")
        .iter()
        .map(|c| (c["name"].as_str().expect("a name").to_owned(), c["dir"] == true))
        .collect();
    let want = [
        ("src", true),
        ("README.md", false),
        ("big.txt", false),
        ("blob.bin", false),
        ("link", false),
        ("page.html", false),
    ];
    assert_eq!(names, want.map(|(n, d)| (n.to_owned(), d)));
    assert_eq!(root["children"]["items"][0]["files"], 2, "src holds 2 files below it");
    let link = &root["children"]["items"][4];
    assert_eq!(
        (link["symlink"].clone(), link["mode"].clone(), link["held"].clone()),
        (true.into(), 0o120000.into(), true.into())
    );
    // A directory, and a page of it after a child's key; a directory's key ends with '/'.
    let src = v.api(&format!("/api/weave/tree/{d}?dir=src"));
    assert_eq!(src["children"]["items"][0]["name"], "deep");
    assert_eq!(src["children"]["items"][1]["path"], "src/lib.rs");
    let after = v.api(&format!("/api/weave/tree/{d}?dir=src&list=children&after=deep/"));
    assert_eq!(after["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(after["items"][0]["name"], "lib.rs");
    v.refused(&format!("/api/weave/tree/{d}?dir=nowhere"), 404);
    // The same tree by the manifest's content hash.
    let hash = root["manifest"].as_str().expect("a hash");
    assert_eq!(v.api(&format!("/api/weave/tree/{hash}"))["files"], 7);
    assert_eq!(v.api(&format!("/api/weave/record/{hash}"))["page"], "tree");
}

#[test]
fn a_file_shows_only_held_text_within_the_cap_and_names_the_rest() {
    let (w, d) = with_a_tree();
    let v = view(&w.ground);
    let file = |path: &str| v.api(&format!("/api/weave/file/{d}?path={path}"));
    let readme = file("README.md");
    assert_eq!(
        (readme["shown"].as_str(), readme["text"].as_str(), readme["lines"].as_u64()),
        (Some("text"), Some("hello\nworld"), Some(2))
    );
    let deep = file("src/deep/mod.rs");
    assert_eq!(deep["text"], "\u{202e}code\n", "the bidi control comes back as it is, for the page to mark");
    let blob = file("blob.bin");
    assert_eq!(blob["shown"], "binary");
    assert!(blob.get("text").is_none());
    let big = file("big.txt");
    assert_eq!((big["shown"].as_str(), big["size"].as_u64()), (Some("oversized"), Some(((1u64 << 19) + 1) * 2)));
    assert!(big.get("text").is_none());
    let link = file("link");
    assert_eq!((link["shown"].as_str(), link["text"].as_str()), (Some("symlink"), Some("src/lib.rs")));
    v.refused(&format!("/api/weave/file/{d}"), 400);
}

/// Security note 5 on PR #79: a file's bytes leave the view only as a JSON string, under the view's
/// headers; a path is a key in the manifest, never a file-system path.
#[test]
fn a_files_bytes_are_json_text_and_a_path_never_reaches_the_file_system() {
    let (w, d) = with_a_tree();
    let v = view(&w.ground);
    let r = v.request(&format!("/api/weave/file/{d}?path=page.html"));
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "application/json");
    assert_eq!(r.headers["x-content-type-options"], "nosniff");
    assert!(r.headers["content-security-policy"].contains("default-src 'none'"));
    let raw = String::from_utf8(r.body.clone()).expect("UTF-8");
    assert!(!raw.contains('<') && !raw.contains('>'), "markup is escaped in the bytes: {raw}");
    assert_eq!(r.json()["text"], "<script>alert(1)</script>\n<img src=x onerror=alert(2)>\n");
    // Paths that would climb out of a directory, or name the replica's own files, aren't in the manifest.
    let ground = w.ground.to_str().expect("UTF-8").to_owned();
    for path in [
        "../README.md",
        "/README.md",
        "src/../README.md",
        "%2e%2e%2fREADME.md",
        "./README.md",
        "src//lib.rs",
        "README.md/",
        "ground.json",
        "weave-root",
        &format!("{ground}/ground.json"),
        "/etc/passwd",
    ] {
        v.refused(&format!("/api/weave/file/{d}?path={path}"), 404);
    }
    for dir in ["..", "../src", "/", "/src", "src/", "src/..", "./src", "%2e%2e"] {
        v.refused(&format!("/api/weave/tree/{d}?dir={dir}"), 404);
    }
}

/// A hand-made manifest is browsable. A file this replica doesn't hold, or holds at another size, is named
/// as missing; a symlink whose target isn't text is named as binary; neither is shown.
#[test]
fn hand_made_manifests_name_what_they_cant_show() {
    let (w, d) = with_a_tree();
    let readme = ContentHash::of(b"hello\nworld");
    let blob: &[u8] = b"\x00\x01\x02 not text";
    let never = ContentHash::of(b"bytes this replica never saw");
    let m = Manifest::new(vec![
        Entry { path: "held.md".into(), mode: Mode::File, hash: readme, size: 11 },
        Entry { path: "short.md".into(), mode: Mode::File, hash: readme, size: 99 },
        Entry { path: "gone.md".into(), mode: Mode::File, hash: never, size: 5 },
        Entry { path: "badlink".into(), mode: Mode::Symlink, hash: ContentHash::of(blob), size: blob.len() as u64 },
    ]);
    let file = w.tmp.path().join("m.cbor");
    std::fs::write(&file, m.encode()).expect("writes the manifest");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", MANIFEST_MEDIA_TYPE, "--as", AS]);
    let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let v = view(&w.ground);
    assert_eq!(v.api(&format!("/api/weave/record/{id}"))["page"], "tree");
    let t = v.api(&format!("/api/weave/tree/{id}"));
    assert_eq!(t["complete"], false);
    let missing: Vec<&str> =
        t["missing"]["items"].as_array().expect("missing").iter().filter_map(Value::as_str).collect();
    assert_eq!(missing, ["gone.md", "short.md"]);
    let held: Vec<bool> =
        t["children"]["items"].as_array().expect("children").iter().map(|c| c["held"] == true).collect();
    assert_eq!(held, [true, false, true, false], "badlink, gone, held, short");
    let bad = v.api(&format!("/api/weave/file/{id}?path=badlink"));
    assert_eq!((bad["shown"].as_str(), bad["symlink"].as_bool()), (Some("binary"), Some(true)));
    assert!(bad.get("text").is_none());
    assert_eq!(v.api(&format!("/api/weave/file/{id}?path=held.md"))["text"], "hello\nworld");
    for path in ["short.md", "gone.md"] {
        let f = v.api(&format!("/api/weave/file/{id}?path={path}"));
        assert_eq!(f["shown"], "missing", "{path}");
        assert!(f.get("text").is_none(), "{path}");
    }
    // A decision's own tree is still whole.
    assert_eq!(v.api(&format!("/api/weave/tree/{d}"))["complete"], true);
}

/// Round-3 note 1 on PR #79: a hand-made policy may name a path twice (the CLI never writes one). Each
/// entry is listed once, so paging after a repeated key can't loop and every path is reached.
#[test]
fn a_policy_list_with_repeats_pages_each_entry_once() {
    let (w, ..) = Weave::with_two_decisions();
    let me = w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    // 101 entries, the 50th and the 100th the same path, as the reviewer's case.
    let mut protected: Vec<String> = (0..101).map(|i| format!("p{i:03}")).collect();
    protected[99] = protected[49].clone();
    let line = Line {
        v: VERSION,
        name: "repeats".into(),
        integrators: vec![me.parse().expect("a key"), me.parse().expect("a key")],
        validators: vec![],
        goal_setters: vec![me.parse().expect("a key")],
        checks: BTreeMap::new(),
        executors: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: protected.clone() },
        owner_decision: Some(OD.into()),
    };
    let file = w.tmp.path().join("repeats.cbor");
    std::fs::write(&file, line.encode()).expect("writes the policy");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", LINE_MEDIA_TYPE, "--as", AS]);
    let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let v = view(&w.ground);
    let page = v.api(&format!("/api/weave/line/{id}"));
    assert_eq!(page["policy"]["integrators"]["total"], 1, "the same integrator twice is listed once");
    assert_eq!(page["policy"]["protected"]["total"], 100);
    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..10 {
        let q = after.as_ref().map_or(String::new(), |a| format!("&after={a}"));
        let got = v.api(&format!("/api/weave/line/{id}?list=protected&limit=50{q}"));
        seen.extend(got["items"].as_array().expect("items").iter().map(|p| p.as_str().expect("a path").to_owned()));
        if got["more"] != true {
            break;
        }
        after = seen.last().cloned();
    }
    let mut want = protected;
    want.remove(99); // the repeat of the 50th
    assert_eq!(seen, want, "each path once, in order, and paging ends");
}

/// Finding 9c8b398bffe2: a tree child's name and path are a link and a cursor, so neither is cut, even for
/// a path of nearly the longest a path may be whose characters JSON must escape.
#[test]
fn a_tree_childs_path_and_name_are_never_cut() {
    let (w, ..) = Weave::with_two_decisions();
    let readme = ContentHash::of(b"hello\n");
    // 16 components of 255 bytes: each a quote, which JSON writes as two bytes, and the last a file.
    let part = "\"".repeat(255);
    let dir = vec![part.as_str(); 15].join("/");
    let long = format!("{dir}/{part}");
    assert_eq!(long.len(), 4095);
    let m = Manifest::new(vec![
        Entry { path: long.clone(), mode: Mode::File, hash: readme, size: 6 },
        Entry { path: format!("{dir}/z"), mode: Mode::File, hash: readme, size: 6 },
    ]);
    let file = w.tmp.path().join("m.cbor");
    std::fs::write(&file, m.encode()).expect("writes the manifest");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", MANIFEST_MEDIA_TYPE, "--as", AS]);
    let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let v = view(&w.ground);
    let t = v.api(&format!("/api/weave/tree/{id}?dir={}", encode(&dir)));
    let first = &t["children"]["items"][0];
    assert_eq!(first["path"], long.as_str(), "the path is whole");
    assert_eq!(first["name"], part.as_str(), "the name is whole");
    assert!(first.get("cut").is_none());
    // Paging after it, by its whole name, reaches the next child.
    let next = v.api(&format!("/api/weave/tree/{id}?dir={}&list=children&after={}", encode(&dir), encode(&part)));
    assert_eq!(next["items"][0]["name"], "z");
    assert_eq!(v.api(&format!("/api/weave/file/{id}?path={}", encode(&long)))["text"], "hello\n");
}

/// Finding 88362a2d04a6: a symlink whose target is over the 1 MiB cap is oversized, as any file is, not
/// binary. Round-1 note 4: a symlink's target is shown as text and never followed, and a NUL in a path is
/// a key like any other, so it names no file.
#[test]
fn a_symlink_over_the_cap_is_oversized_and_a_target_is_never_followed() {
    let (w, _) = with_a_tree();
    let big = "x\n".repeat((1 << 19) + 1);
    let escape: &[u8] = b"../../../etc/passwd";
    let file = w.tmp.path().join("escape.txt");
    std::fs::write(&file, escape).expect("writes the target");
    w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", FILE_MEDIA_TYPE, "--as", AS]);
    let held = |b: &[u8]| (ContentHash::of(b), b.len() as u64);
    let (big_hash, big_size) = held(big.as_bytes());
    let (escape_hash, escape_size) = held(escape);
    let m = Manifest::new(vec![
        Entry { path: "big-link".into(), mode: Mode::Symlink, hash: big_hash, size: big_size },
        Entry { path: "escape".into(), mode: Mode::Symlink, hash: escape_hash, size: escape_size },
    ]);
    let file = w.tmp.path().join("m.cbor");
    std::fs::write(&file, m.encode()).expect("writes the manifest");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", MANIFEST_MEDIA_TYPE, "--as", AS]);
    let id = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let v = view(&w.ground);
    let big = v.api(&format!("/api/weave/file/{id}?path=big-link"));
    assert_eq!((big["shown"].as_str(), big["symlink"].as_bool()), (Some("oversized"), Some(true)));
    assert!(big.get("text").is_none());
    let escape = v.api(&format!("/api/weave/file/{id}?path=escape"));
    assert_eq!((escape["shown"].as_str(), escape["text"].as_str()), (Some("symlink"), Some("../../../etc/passwd")));
    for path in ["escape%00", "%00", "escape/", "escape/passwd"] {
        v.refused(&format!("/api/weave/file/{id}?path={path}"), 404);
    }
}

// ---- History and diffs (PR (d)) ----

/// A Weave Ground where `f.txt` on main is added, modified, has its mode changed and is left alone. Then
/// a line `dev` starts by merging main into it, and changes the file once. Then main deletes it.
/// Returns the Ground, main's decisions in order, and dev's two.
fn with_a_history() -> (Weave, Vec<String>, Vec<String>) {
    let w = Weave::new();
    let mut main = vec![w.decide(&[("f.txt", "one\n")], "add"), w.decide(&[("f.txt", "one\ntwo\n")], "modify")];
    let f = w.repo.join("f.txt");
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    main.push(w.decide(&[], "mode"));
    main.push(w.decide(&[("other.txt", "x\n")], "unrelated"));
    w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS]);
    let merged = w.json(&["weave", "line", "merge", "main", "--into", "dev", "--as", AS, "--json"]);
    let mut dev = vec![merged["decision"].as_str().expect("a decision").to_owned()];
    std::fs::write(&f, "one\ntwo\nthree\n").expect("write");
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "dev"]);
    let out = w.json(&["weave", "integrate", "--line", "dev", "--as", AS, "--note", "on dev", "--json"]);
    dev.push(out["decision"].as_str().expect("a decision").to_owned());
    std::fs::remove_file(&f).expect("delete");
    main.push(w.decide(&[], "delete"));
    (w, main, dev)
}

fn line_named(w: &Weave, name: &str) -> String {
    let lines = w.json(&["weave", "line", "list", "--json"]);
    let row = lines["lines"].as_array().expect("lines").iter().find(|l| l["name"] == name).expect("the line");
    row["line"].as_str().expect("an id").to_owned()
}

#[test]
fn a_files_history_lists_each_change_on_the_line_newest_first() {
    let (w, main, dev) = with_a_history();
    let (main_line, dev_line) = (line_named(&w, "main"), line_named(&w, "dev"));
    let v = view(&w.ground);
    let h = v.api(&format!("/api/weave/history/{main_line}?path=f.txt"));
    let rows: Vec<(String, String)> = h["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| (r["decision"].as_str().expect("id").to_owned(), r["kind"].as_str().expect("kind").to_owned()))
        .collect();
    let want = [(&main[4], "deleted"), (&main[2], "mode changed"), (&main[1], "modified"), (&main[0], "added")];
    assert_eq!(rows, want.map(|(d, k)| (d.clone(), k.to_owned())), "the unrelated decision isn't listed");
    // The line's root compares with an absent file, and has no parent.
    assert!(h["items"][3]["parent"].is_null() && h["items"][3]["before"].is_null());
    assert_eq!(h["items"][1]["before"]["mode"], "100644");
    assert_eq!(h["items"][1]["after"]["mode"], "100755");
    // Paging after a decision.
    let next = v.api(&format!("/api/weave/history/{main_line}?path=f.txt&after={}&limit=1", main[2]));
    assert_eq!(next["items"][0]["decision"], main[1].as_str());
    assert_eq!(next["more"], true);
    // dev's first decision, made by a line merge, has no parent on dev: it compares with main's head,
    // where the file is the same, so it isn't listed; dev's own change is.
    let d = v.api(&format!("/api/weave/history/{dev_line}?path=f.txt"));
    assert_eq!(d["total"], 1, "{d}");
    assert_eq!(d["items"][0]["decision"], dev[1].as_str());
    assert_eq!(
        (d["items"][0]["kind"].as_str(), d["items"][0]["parent_on_line"].as_bool()),
        (Some("modified"), Some(true))
    );
    let merge = v.api(&format!("/api/weave/file/{}?path=f.txt", dev[0]));
    assert_eq!(merge["decision"]["parent"], main[3].as_str(), "the merge's parent is main's head, on another line");
    v.refused(&format!("/api/weave/history/{main_line}"), 400);
}

#[test]
fn a_diff_lists_the_files_that_changed_and_diffs_one() {
    let (w, main, _) = with_a_history();
    let v = view(&w.ground);
    let (a, b) = (&main[0], &main[3]);
    let list = v.api(&format!("/api/weave/diff?a={a}&b={b}"));
    let files: Vec<(String, String)> = list["files"]["items"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["path"].as_str().expect("a path").to_owned(), f["kind"].as_str().expect("a kind").to_owned()))
        .collect();
    assert_eq!(files, [("f.txt".to_owned(), "modified".to_owned()), ("other.txt".to_owned(), "added".to_owned())]);
    let cli = w.json(&["weave", "diff", a, b, "--json"]);
    assert_eq!(cli["files"].as_array().map(Vec::len), Some(2), "the CLI lists the same two");
    // The same two trees named by their manifests' content hashes.
    let (ha, hb) = (list["a"].as_str().expect("a hash"), list["b"].as_str().expect("a hash"));
    assert!(ha.starts_with("blake3:"));
    assert_eq!(v.api(&format!("/api/weave/diff?a={ha}&b={hb}"))["files"], list["files"]);
    let page = v.api(&format!("/api/weave/diff?a={a}&b={b}&list=files&after=f.txt"));
    assert_eq!(page["items"][0]["path"], "other.txt");
    let one = v.api(&format!("/api/weave/diff?a={a}&b={b}&path=f.txt"));
    assert_eq!(one["shown"], "text");
    let unified = one["unified"].as_str().expect("unified text");
    assert!(unified.contains("+two\n") && unified.contains(" one\n"), "{unified}");
    assert_eq!(one["lines"].as_u64(), Some(unified.lines().count() as u64));
    let added = v.api(&format!("/api/weave/diff?a={a}&b={b}&path=other.txt"));
    assert_eq!(added["shown"], "text");
    assert!(added["unified"].as_str().expect("text").contains("+x\n"));
    // Mode only, the same file, a path in neither, and a missing parameter.
    assert_eq!(v.api(&format!("/api/weave/diff?a={}&b={}&path=f.txt", main[1], main[2]))["shown"], "mode_only");
    assert_eq!(v.api(&format!("/api/weave/diff?a={a}&b={a}&path=f.txt"))["shown"], "same");
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&path=nowhere"), 404);
    v.refused(&format!("/api/weave/diff?a={a}"), 400);
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&path=f.txt&limit=5"), 400);
    // after and limit page the list only when it is named, never ignored (finding fb21c13db9b1).
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&after=f.txt&limit=1"), 400);
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&limit=1"), 400);
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&list=files&after=nowhere"), 400);
    v.refused(&format!("/api/weave/diff?a={a}&b={b}&list=tree"), 400);
}

/// The caps: a side over 1 MiB, a side not text, a side not held, and two sides too long to diff line by
/// line are each named, with no text.
#[test]
fn a_diff_at_the_caps_names_why_it_has_no_text() {
    let w = Weave::new();
    let long_a: String = (0..5_000).map(|i| format!("a{i}\n")).collect();
    let long_b: String = (0..4_000).map(|i| format!("b{i}\n")).collect();
    let a = w.decide(&[("big.txt", "small\n"), ("bin.dat", "text\n"), ("long.txt", &long_a)], "a");
    let big = "x\n".repeat((1 << 19) + 1);
    std::fs::write(w.repo.join("bin.dat"), b"\x00binary").expect("write");
    std::os::unix::fs::symlink("big.txt", w.repo.join("link")).expect("a symlink");
    let b = w.decide(&[("big.txt", &big), ("long.txt", &long_b)], "b");
    // A side not held: a hand-made manifest naming a file this replica never saw.
    // And one held, but at another size than its entry names (finding 81ffe9a422c4).
    let m = Manifest::new(vec![
        Entry { path: "big.txt".into(), mode: Mode::File, hash: ContentHash::of(b"never seen"), size: 10 },
        Entry { path: "short.txt".into(), mode: Mode::File, hash: ContentHash::of(b"small\n"), size: 99 },
    ]);
    let file = w.tmp.path().join("m.cbor");
    std::fs::write(&file, m.encode()).expect("writes the manifest");
    let out = w.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", MANIFEST_MEDIA_TYPE, "--as", AS]);
    let hand = out.trim().strip_prefix("artifact ").expect("labelled").to_owned();
    let v = view(&w.ground);
    let shown = |path: &str| v.api(&format!("/api/weave/diff?a={a}&b={b}&path={path}"))["shown"].clone();
    assert_eq!(shown("big.txt"), "oversized");
    assert_eq!(shown("bin.dat"), "binary");
    assert_eq!(shown("link"), "binary", "a symlink's target isn't diffed as a file's text");
    assert_eq!(shown("long.txt"), "too_large_to_diff", "5,000 x 4,000 lines pass diff::MAX_DIFF_CELLS");
    let missing = v.api(&format!("/api/weave/diff?a={a}&b={hand}&path=big.txt"));
    assert_eq!(missing["shown"], "missing");
    assert!(missing.get("unified").is_none());
    let list = v.api(&format!("/api/weave/diff?a={a}&b={hand}"));
    let row =
        list["files"]["items"].as_array().expect("files").iter().find(|f| f["path"] == "big.txt").expect("big.txt");
    assert_eq!(row["not_held"], true);
    let short =
        list["files"]["items"].as_array().expect("files").iter().find(|f| f["path"] == "short.txt").expect("short.txt");
    assert_eq!(short["not_held"], true, "held, but not at the entry's size");
    assert_eq!(v.api(&format!("/api/weave/diff?a={a}&b={hand}&path=short.txt"))["shown"], "missing");
}

/// The unified diffs build at most 2 at a time: a third, while 2 are building, is refused at once with a
/// 503, never left waiting.
#[test]
fn the_diff_slots_refuse_a_third_at_once() {
    let w = Weave::new();
    let a = w.decide(&[("1.txt", "a\n"), ("2.txt", "a\n"), ("3.txt", "a\n")], "a");
    let b = w.decide(&[("1.txt", "b\n"), ("2.txt", "b\n"), ("3.txt", "b\n")], "b");
    let v = view_with(&w.ground, &[("BRACKEN_VIEW_DIFF_DELAY_MS", "1500")]);
    assert_eq!(v.api("/api/weave")["weave"], true);
    let started = Instant::now();
    let replies: Vec<(u16, String)> = std::thread::scope(|s| {
        let handles: Vec<_> = ["1.txt", "2.txt", "3.txt"]
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let (a, b, v) = (a.clone(), b.clone(), &v);
                s.spawn(move || {
                    std::thread::sleep(Duration::from_millis(200 * i as u64));
                    let r = v.request_once(&format!("/api/weave/diff?a={a}&b={b}&path={path}"));
                    (r.status, String::from_utf8_lossy(&r.body).into_owned())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a request")).collect()
    });
    let refused: Vec<&(u16, String)> = replies.iter().filter(|(s, _)| *s == 503).collect();
    assert_eq!(refused.len(), 1, "{replies:?}");
    assert!(refused[0].1.contains("busy"), "{}", refused[0].1);
    assert_eq!(replies.iter().filter(|(s, _)| *s == 200).count(), 2);
    assert!(started.elapsed() < Duration::from_millis(1_500 * 3), "the third wasn't left waiting");
}

/// A merge's "what changed" compares with its parent on its own line, as its history and file pages do,
/// whichever parent sorts first (security note N1, independent note 3).
#[test]
fn a_merge_is_compared_with_its_parent_on_its_own_line() {
    let w = Weave::new();
    let first = w.decide(&[("a.txt", "a\n")], "a");
    w.ok(&["weave", "line", "create", "dev", "--owner-decision", OD, "--as", AS]);
    w.ok(&["weave", "line", "merge", "main", "--into", "dev", "--as", AS, "--json"]);
    std::fs::write(w.repo.join("b.txt"), "b\n").expect("write");
    w.git(&["add", "-A"]);
    w.git(&["commit", "-q", "-m", "dev"]);
    let dev_head =
        w.id_of(&["weave", "integrate", "--line", "dev", "--as", AS, "--note", "on dev", "--json"], "decision");
    let main_head = w.decide(&[("c.txt", "c\n")], "c");
    let merge = w.id_of(&["weave", "line", "merge", "main", "--into", "dev", "--as", AS, "--json"], "decision");
    let v = view(&w.ground);
    let d = v.api(&format!("/api/weave/decision/{merge}"));
    let mut parents: Vec<&str> = d["parents"].as_array().expect("parents").iter().filter_map(Value::as_str).collect();
    parents.sort_unstable();
    let mut both = [dev_head.as_str(), main_head.as_str()];
    both.sort_unstable();
    assert_eq!(parents, both);
    assert_eq!(d["compared_with"], dev_head.as_str());
    assert_eq!(v.api(&format!("/api/weave/file/{merge}?path=a.txt"))["decision"]["parent"], dev_head.as_str());
    // The line's root has nothing to compare with.
    let root = v.api(&format!("/api/weave/decision/{}", first));
    assert!(root["compared_with"].is_null(), "{root}");
}

/// `BRACKEN_VIEW_DIFF_DELAY_MS` is capped at 2 s: a huge value still answers a diff within a few seconds.
#[test]
fn the_diff_delay_is_capped() {
    let w = Weave::new();
    let a = w.decide(&[("f.txt", "a\n")], "a");
    let b = w.decide(&[("f.txt", "b\n")], "b");
    let v = view_with(&w.ground, &[("BRACKEN_VIEW_DIFF_DELAY_MS", "600000")]);
    assert_eq!(v.api("/api/weave")["weave"], true);
    let started = Instant::now();
    assert_eq!(v.api(&format!("/api/weave/diff?a={a}&b={b}&path=f.txt"))["shown"], "text");
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1_900), "the delay applies: {took:?}");
    assert!(took < Duration::from_secs(10), "the delay isn't capped: {took:?}");
}

// ---- Proposals and the conflict preview (PR (e)) ----

impl Weave {
    /// Commits the work tree as it is, on the current branch.
    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Writes `files` into the work tree.
    fn write(&self, files: &[(&str, &[u8])]) {
        for (path, bytes) in files {
            std::fs::write(self.repo.join(path), bytes).expect("write");
        }
    }

    /// A proposal of git branch `branch`'s tree against `base`, signed `AS`.
    fn propose(&self, branch: &str, base: &str, summary: &str) -> String {
        self.id_of(
            &["weave", "propose", "--from-git", branch, "--base", base, "--summary", summary, "--as", AS, "--json"],
            "proposal",
        )
    }

    /// An artifact of `bytes` with `media_type`, signed `AS`: its id.
    fn artifact(&self, name: &str, bytes: &[u8], media_type: &str) -> String {
        let file = self.tmp.path().join(name);
        std::fs::write(&file, bytes).expect("writes the artifact");
        let out = self.ok(&["artifact", file.to_str().expect("UTF-8"), "--media-type", media_type, "--as", AS]);
        out.trim().strip_prefix("artifact ").expect("labelled").to_owned()
    }
}

fn exec(w: &Weave, path: &str) {
    std::fs::set_permissions(w.repo.join(path), std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
}

/// A Weave Ground with, on main: an open proposal `open` from the first decision, `base`, whose preview
/// against the head has one conflict of each kind; an integrated one; a retracted one; and an invalid one.
struct Proposals {
    w: Weave,
    line: String,
    base: String,
    head: String,
    open: String,
    integrated: String,
    retracted: String,
    invalid: String,
}

fn with_proposals() -> Proposals {
    let w = Weave::new();
    let base = w.decide(
        &[("f.txt", "1\n2\n3\n"), ("h.txt", "1\n2\n3\n"), ("m.sh", "echo\n"), ("bin.dat", "a\n"), ("big.txt", "b\n")],
        "base",
    );
    let line = w.main_line();
    // The proposal: f.txt and h.txt each change one line, m.sh only its mode, bin.dat becomes binary and
    // big.txt over the view's 1 MiB cap.
    w.git(&["checkout", "-q", "-b", "p"]);
    let big = vec![b'p'; (1 << 20) + 1];
    w.write(&[("f.txt", b"x\n2\n3\n"), ("h.txt", b"1\n2\nz\n"), ("bin.dat", b"\x00p"), ("big.txt", &big)]);
    exec(&w, "m.sh");
    w.commit("p");
    let open = w.propose("p", &base, "the open one");
    // The head: each of those files changes another way.
    w.git(&["checkout", "-q", "main"]);
    w.write(&[
        ("f.txt", b"y\n2\n3\n"),
        ("h.txt", b"q\n2\n3\n"),
        ("m.sh", b"echo hi\n"),
        ("bin.dat", b"\x00h"),
        ("big.txt", b"h\n"),
    ]);
    let after_base = w.decide(&[], "head");
    // Integrated: a proposal from the head, integrated by name.
    w.git(&["checkout", "-q", "-b", "q"]);
    w.write(&[("q.txt", b"q\n")]);
    w.commit("q");
    let integrated = w.propose("q", &after_base, "integrated");
    w.git(&["checkout", "-q", "main"]);
    let head = w.id_of(
        &["weave", "integrate", "--proposals", &integrated, "--as", AS, "--note", "takes q", "--json"],
        "decision",
    );
    // Retracted.
    w.git(&["checkout", "-q", "-b", "r"]);
    w.write(&[("r.txt", b"r\n")]);
    w.commit("r");
    let retracted = w.propose("r", &head, "retracted");
    w.ok(&["retract", &retracted, "--as", AS]);
    w.git(&["checkout", "-q", "main"]);
    // Invalid: a change that isn't its base's to its tree.
    let named = w.json(&["weave", "head", "--json"]);
    let manifest: ContentHash = named["heads"][0]["manifest"].as_str().expect("a manifest").parse().expect("a hash");
    let bad = Proposal {
        v: VERSION,
        line: line.parse().expect("an id"),
        base: head.parse().expect("an id"),
        manifest,
        change: ContentHash::of(b"not this change"),
        summary: "invalid".into(),
    };
    let invalid = w.artifact("bad.cbor", &bad.encode(), PROPOSAL_MEDIA_TYPE);
    Proposals { w, line, base, head, open, integrated, retracted, invalid }
}

#[test]
fn proposals_list_each_status_and_filter_by_it() {
    let p = with_proposals();
    let v = view(&p.w.ground);
    let all = v.api(&format!("/api/weave/proposals/{}", p.line));
    let status: BTreeMap<String, String> = all["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| (r["proposal"].as_str().expect("id").to_owned(), r["status"].as_str().expect("status").to_owned()))
        .collect();
    let want: BTreeMap<String, String> =
        [(&p.open, "open"), (&p.integrated, "integrated"), (&p.retracted, "retracted"), (&p.invalid, "invalid")]
            .into_iter()
            .map(|(id, s)| (id.clone(), s.to_owned()))
            .collect();
    assert_eq!(status, want);
    let ids: Vec<&str> =
        all["items"].as_array().expect("items").iter().filter_map(|r| r["proposal"].as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ascending by id");
    let row = |id: &str| {
        all["items"].as_array().expect("items").iter().find(|r| r["proposal"] == id).expect("listed").clone()
    };
    assert_eq!(row(&p.integrated)["decision"], p.head.as_str());
    assert!(row(&p.invalid)["why"].as_str().expect("why").contains("isn't the change"));
    assert_eq!(row(&p.open)["summary"], "the open one");
    for (status, id) in
        [("open", &p.open), ("integrated", &p.integrated), ("retracted", &p.retracted), ("invalid", &p.invalid)]
    {
        let one = v.api(&format!("/api/weave/proposals/{}?status={status}", p.line));
        assert_eq!(one["total"], 1, "{status}: {one}");
        assert_eq!(one["items"][0]["proposal"], id.as_str());
    }
    let page = v.api(&format!("/api/weave/proposals/{}?after={}&limit=1", p.line, ids[0]));
    assert_eq!((page["items"][0]["proposal"].as_str(), page["more"].as_bool()), (Some(ids[1]), Some(true)));
    v.refused(&format!("/api/weave/proposals/{}?status=closed", p.line), 400);
    v.refused(&format!("/api/weave/proposals/{}", p.open), 404);
    // An integrated one's page names the decision and its note; only an open one has a preview.
    let i = v.api(&format!("/api/weave/proposal/{}", p.integrated));
    assert_eq!(i["integrated_by"]["note"], "takes q");
    assert!(i["preview"].is_null() && i["preview_why"].as_str().expect("why").contains("isn't open"));
    assert_eq!(i["change"]["items"][0]["path"], "q.txt");
    assert!(i["change"]["items"][0]["before"].is_null());
    v.refused(&format!("/api/weave/proposal/{}", p.base), 404);
    for id in [&p.open, &p.retracted, &p.invalid] {
        assert_eq!(v.api(&format!("/api/weave/record/{id}"))["page"], "proposal", "{id}");
    }
}

#[test]
fn an_open_proposals_preview_names_each_conflict_as_integrate_would_meet_it() {
    let p = with_proposals();
    // A review of each kind: an objection, which a decision using it would answer, and an agreement.
    let claim = p.w.id_of(&["claim", "f.txt is wrong", "--as", AS, "--json"], "id");
    let objection = p.w.id_of(&["relate", &claim, "contradicts", &p.open, "--note", "no", "--as", AS, "--json"], "id");
    let agreement = p.w.id_of(&["relate", &claim, "supports", &p.open, "--note", "yes", "--as", AS, "--json"], "id");
    p.w.ok(&["log", "--limit", "1"]); // so the snapshot is up to date before the view starts
    let before = files(&p.w.ground);
    let v = view(&p.w.ground);
    let o = v.api(&format!("/api/weave/proposal/{}", p.open));
    let reviews: Vec<(&str, &str)> = o["reviews"]["items"]
        .as_array()
        .expect("reviews")
        .iter()
        .map(|r| (r["review"].as_str().expect("id"), r["kind"].as_str().expect("kind")))
        .collect();
    let mut want = vec![(objection.as_str(), "objection"), (agreement.as_str(), "agreement")];
    want.sort_unstable();
    assert_eq!(reviews, want);
    assert_eq!(o["objections"]["items"][0]["objection"], objection.as_str(), "{}", o["objections"]);
    assert_eq!(o["objections"]["items"][0]["basis"]["kind"], "discussion");
    for list in ["change", "reviews", "objections", "conflicts"] {
        let page = v.api(&format!("/api/weave/proposal/{}?list={list}&limit=2", p.open));
        assert_eq!(page["list"], list);
        assert_eq!(
            page["items"].as_array().map(Vec::len),
            Some(if list == "objections" { 1 } else { 2 }),
            "{list}: {page}"
        );
    }
    assert_eq!(o["status"], "open");
    assert_eq!(o["change"]["total"], 5, "{}", o["change"]);
    let preview = &o["preview"];
    assert_eq!(preview["head"], p.head.as_str());
    let lines: Vec<(&str, &str)> = preview["items"]
        .as_array()
        .expect("conflicts")
        .iter()
        .map(|c| (c["path"].as_str().expect("path"), c["lines"].as_str().expect("lines")))
        .collect();
    assert_eq!(
        lines,
        [
            ("big.txt", "over_view_cap"),
            ("bin.dat", "not_text"),
            ("f.txt", "conflicts"),
            ("h.txt", "clean"),
            ("m.sh", "clean")
        ],
        "a mode-only change against a change of text merges line by line"
    );
    let f = &preview["items"][2];
    assert_eq!(f["ranges"], serde_json::json!([[1, 1]]));
    assert_eq!(f["into"]["mode"], "100644");
    let m = &preview["items"][4];
    assert_eq!((m["base"]["mode"].as_str(), m["from"]["mode"].as_str()), (Some("100644"), Some("100755")));
    assert_eq!(preview["total"], 5);
    assert_eq!(preview["more"], false);
    v.request(&format!("/api/weave/proposals/{}", p.line));
    v.request(&format!("/api/weave/proposal/{}?list=resolutions&path=f.txt", p.open));
    std::thread::sleep(Duration::from_millis(1_500)); // a watcher poll or so
    drop(v);
    assert_eq!(files(&p.w.ground), before, "the view changed a file under the Ground");
}

/// The resolutions offered for a conflict's key: the first 10 in the preview, the rest paged by path, each
/// marked if an integrator of the line wrote it; the first an integrator wrote is what integrate applies.
#[test]
fn a_conflicts_resolutions_are_marked_and_paged() {
    let p = with_proposals();
    p.w.ok(&["id", "new", "alice"]);
    // `weave resolve` settles a merge's conflicts in path order: big.txt and bin.dat come first.
    for path in ["big.txt", "bin.dat"] {
        p.w.ok(&["weave", "resolve", "--proposals", &p.open, "--take", "into", path, "--as", AS]);
    }
    let mut by_integrator = Vec::new();
    for _ in 0..10 {
        by_integrator.push(p.w.id_of(
            &["weave", "resolve", "--proposals", &p.open, "--take", "into", "f.txt", "--as", AS, "--json"],
            "resolution",
        ));
    }
    let by_alice = p.w.id_of(
        &["weave", "resolve", "--proposals", &p.open, "--take", "from", "f.txt", "--as", "alice", "--json"],
        "resolution",
    );
    let mut all: Vec<&str> = by_integrator.iter().map(String::as_str).chain([by_alice.as_str()]).collect();
    all.sort_unstable();
    let v = view(&p.w.ground);
    let o = v.api(&format!("/api/weave/proposal/{}", p.open));
    let f = o["preview"]["items"].as_array().expect("conflicts").iter().find(|c| c["path"] == "f.txt").expect("f.txt");
    assert_eq!(f["resolutions_total"], 11);
    let shown: Vec<&str> =
        f["resolutions"].as_array().expect("resolutions").iter().filter_map(|r| r["resolution"].as_str()).collect();
    assert_eq!(shown, all[..10], "the first 10, by id");
    let mut integrators = by_integrator.clone();
    integrators.sort_unstable();
    assert_eq!(f["settled_by"], integrators[0].as_str(), "integrate applies an integrator's, the smallest id first");
    let rest = v.api(&format!("/api/weave/proposal/{}?list=resolutions&path=f.txt&after={}", p.open, all[9]));
    assert_eq!(rest["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(rest["items"][0]["resolution"], all[10]);
    let page = v.api(&format!("/api/weave/proposal/{}?list=resolutions&path=f.txt&limit=20", p.open));
    let marks: Vec<(String, bool)> = page["items"]
        .as_array()
        .expect("resolutions")
        .iter()
        .map(|r| (r["resolution"].as_str().expect("id").to_owned(), r["integrator"].as_bool().expect("a mark")))
        .collect();
    assert!(marks.iter().all(|(id, integrator)| *integrator == (*id != by_alice)), "{marks:?}");
    assert_eq!(
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|r| r["resolution"] == by_alice.as_str())
            .expect("alice's")["author_name"],
        "alice"
    );
    v.refused(&format!("/api/weave/proposal/{}?list=resolutions", p.open), 400);
    v.refused(&format!("/api/weave/proposal/{}?list=resolutions&path=nowhere", p.open), 404);
    v.refused(&format!("/api/weave/proposal/{}?path=f.txt", p.open), 400);
}

/// Integrate refuses on a forked line and under a diverged policy, so the preview does too, saying why.
#[test]
fn a_forked_line_and_a_diverged_policy_get_no_preview() {
    let p = with_proposals();
    // A second decision from the base, written by hand, forks main.
    let base_manifest = {
        let log = p.w.json(&["weave", "log", "--json"]);
        let rows = log["decisions"].as_array().or_else(|| log.as_array()).expect("decisions").clone();
        let row = rows.iter().find(|d| d["decision"] == p.base.as_str()).expect("the base").clone();
        row["manifest"].as_str().expect("a manifest").parse::<ContentHash>().expect("a hash")
    };
    let fork = Decision {
        v: VERSION,
        line: p.line.parse().expect("an id"),
        manifest: base_manifest,
        parents: vec![p.base.parse().expect("an id")],
        proposals: vec![],
        breakage: vec![],
        answers: vec![],
        resolutions: vec![],
        goals: vec![],
        source: None,
        note: "a fork".into(),
    };
    p.w.artifact("fork.cbor", &fork.encode(), DECISION_MEDIA_TYPE);
    {
        let v = view(&p.w.ground);
        let o = v.api(&format!("/api/weave/proposal/{}", p.open));
        assert!(o["preview"].is_null(), "{o}");
        assert!(o["preview_why"].as_str().expect("why").contains("2 heads"), "{}", o["preview_why"]);
        v.refused(&format!("/api/weave/proposal/{}?list=conflicts", p.open), 404);
    }
    // A second successor of main's first policy version diverges its chain.
    let me = p.w.json(&["id", "list", "--json"])[0]["agent"].as_str().expect("an agent").to_owned();
    for max_known in [1, 2] {
        let next = Line {
            v: VERSION,
            name: "main".into(),
            integrators: vec![me.parse().expect("a key")],
            validators: vec![me.parse().expect("a key")],
            goal_setters: vec![],
            checks: BTreeMap::new(),
            executors: BTreeMap::new(),
            guardrails: Guardrails { max_known, max_age: 0, protected: vec![] },
            owner_decision: Some(OD.into()),
        };
        let id = p.w.artifact(&format!("v{max_known}.cbor"), &next.encode(), LINE_MEDIA_TYPE);
        p.w.ok(&["relate", &id, "supersedes", &p.line, "--as", AS]);
    }
    let v = view(&p.w.ground);
    let o = v.api(&format!("/api/weave/proposal/{}", p.open));
    assert!(o["preview"].is_null());
    assert!(o["preview_why"].as_str().expect("why").contains("diverged"), "{}", o["preview_why"]);
}

/// An open proposal whose preview has 1,000 conflicts naming one small blob (`d/<n>.txt`), then 17 whose
/// proposal side is a 1 MiB line (`w/<n>.txt`): the line's main, the proposal.
fn with_a_large_preview() -> (Weave, String) {
    let w = Weave::new();
    let write_all = |d: &str, w_side: &[u8]| {
        std::fs::create_dir_all(w.repo.join("d")).expect("mkdir");
        std::fs::create_dir_all(w.repo.join("w")).expect("mkdir");
        for i in 0..1_000 {
            std::fs::write(w.repo.join(format!("d/{i}.txt")), d).expect("write");
        }
        for i in 0..17 {
            std::fs::write(w.repo.join(format!("w/{i}.txt")), w_side).expect("write");
        }
    };
    write_all("a\n", b"a\n");
    w.commit("base");
    let base = w.id_of(&["weave", "integrate", "--as", AS, "--note", "base", "--json"], "decision");
    w.git(&["checkout", "-q", "-b", "p"]);
    let mut line = vec![b'p'; (1 << 20) - 1];
    line.push(b'\n');
    write_all("p\n", &line);
    w.commit("p");
    let proposal = w.propose("p", &base, "many");
    w.git(&["checkout", "-q", "main"]);
    write_all("h\n", b"h\n");
    w.commit("head");
    w.ok(&["weave", "integrate", "--as", AS, "--note", "head", "--json"]);
    (w, proposal)
}

/// The preview's budget (section 4, *Conflict preview*): at most 20 conflicts a page, whatever one blob
/// many paths name; and a page stops before the conflict whose sides would pass 16 MiB read, which the
/// next page starts from, so every conflict is computed on some page.
#[test]
fn a_preview_pages_20_conflicts_at_most_and_16_mib_of_sides() {
    let (w, proposal) = with_a_large_preview();
    let v = view(&w.ground);
    let first = v.api(&format!("/api/weave/proposal/{proposal}"));
    let preview = &first["preview"];
    assert_eq!(preview["total"], 1_017);
    assert_eq!(preview["items"].as_array().map(Vec::len), Some(20));
    assert_eq!(preview["more"], true);
    let mut seen: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let url = match &after {
            None => format!("/api/weave/proposal/{proposal}?list=conflicts"),
            Some(a) => format!("/api/weave/proposal/{proposal}?list=conflicts&after={}", encode(a)),
        };
        let page = v.api(&url);
        let items = page["items"].as_array().expect("conflicts");
        pages.push(items.len());
        seen.extend(items.iter().map(|c| c["path"].as_str().expect("path").to_owned()));
        if page["more"] == false {
            break;
        }
        after = seen.last().cloned();
    }
    assert_eq!(seen.len(), 1_017, "each conflict once");
    assert!(pages[..50].iter().all(|n| *n == 20), "{pages:?}");
    assert_eq!(
        pages[50..],
        [15, 2],
        "15 sides of 1 MiB and their 4-byte others fit 16 MiB; the 16th starts the next page"
    );
    v.refused(&format!("/api/weave/proposal/{proposal}?list=conflicts&limit=21"), 400);
    v.refused(&format!("/api/weave/proposal/{proposal}?list=conflicts&after=nowhere"), 400);
}

/// Previews build at most 2 at once, and a third is refused at once; a page already built is kept, and
/// answers while both slots are taken.
#[test]
fn the_preview_slots_refuse_a_third_and_a_kept_page_still_answers() {
    let (w, proposal) = with_a_large_preview();
    let v = view_with(&w.ground, &[("BRACKEN_VIEW_PREVIEW_DELAY_MS", "1500")]);
    let warm = Instant::now();
    v.api(&format!("/api/weave/proposal/{proposal}"));
    assert!(warm.elapsed() >= Duration::from_millis(1_400), "the first page is built, in a slot");
    let replies: Vec<(u16, Duration, String)> = std::thread::scope(|s| {
        let urls = [
            (0, format!("/api/weave/proposal/{proposal}?list=conflicts&after={}", encode("d/1.txt"))),
            (0, format!("/api/weave/proposal/{proposal}?list=conflicts&after={}", encode("d/2.txt"))),
            (300, format!("/api/weave/proposal/{proposal}")),
            (400, format!("/api/weave/proposal/{proposal}?list=conflicts&after={}", encode("d/3.txt"))),
        ];
        let handles: Vec<_> = urls
            .into_iter()
            .map(|(wait, url)| {
                let v = &v;
                s.spawn(move || {
                    std::thread::sleep(Duration::from_millis(wait));
                    let started = Instant::now();
                    let r = v.request_once(&url);
                    (r.status, started.elapsed(), String::from_utf8_lossy(&r.body).into_owned())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a request")).collect()
    });
    assert_eq!((replies[0].0, replies[1].0), (200, 200), "{replies:?}");
    assert_eq!(replies[2].0, 200, "the kept page: {replies:?}");
    assert!(replies[2].1 < Duration::from_millis(1_000), "the kept page waited: {:?}", replies[2].1);
    assert_eq!(replies[3].0, 503, "{replies:?}");
    assert!(replies[3].2.contains("busy") && replies[3].1 < Duration::from_millis(1_000), "{:?}", replies[3]);
}
