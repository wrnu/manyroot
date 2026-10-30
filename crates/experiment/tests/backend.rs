//! The agent backend through its harness interface, without Claude Code: a mock harness plays each turn
//! with `/bin/bash`, pinned and under the agent sandbox profile like any harness, writing events in a line
//! format of its own, so what the backend does (the process it builds, the pin and configuration checks,
//! the gate, token accounting, caps, process groups, transcripts, halting) is checked apart from any one
//! harness.

use std::collections::BTreeSet;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use bracken_experiment::backend::{
    BackendConfig, Caps, Event, Harness, HarnessBackend, Launch, Pin, RUN_TIME_CAP, RUN_TOKEN_CAP, Tokens, Turn,
};
use bracken_experiment::driver::Backend;
use bracken_experiment::proxy::Route;
use bracken_experiment::sandbox::Profile;

const SECRET: &str = "mock-secret-not-a-real-one-0123";

/// A harness whose turn is `script`, run by `/bin/bash` with the turn message as `$1`. Its events, a line
/// each: `init ok` or `init <why not>`; `call <id> <input> <output> [<call id>:<tool>...]`;
/// `result <input> <output>`; anything else is other.
struct Mock {
    script: String,
    pin: Pin,
    profile: Profile,
    secret: String,
    /// What it declares, and what it sets each turn.
    names: Vec<&'static str>,
    env: Vec<(&'static str, String)>,
    /// Its turn configuration's file names, each holding 'conf for <agent>'.
    files: Vec<&'static str>,
    /// What it says once `switched` is set, after the start: another secret, pin and names.
    later: Option<Later>,
    switched: Arc<AtomicBool>,
}

struct Later {
    secret: String,
    pin: Pin,
    names: Vec<&'static str>,
}

impl Mock {
    fn new(script: &str) -> Mock {
        Mock {
            script: script.into(),
            pin: Pin::of(Path::new("/bin/bash"), "mock").expect("a pin"),
            profile: Profile::Agent,
            secret: SECRET.into(),
            names: vec!["MOCK_SET", "MOCK_CONF"],
            env: vec![("MOCK_SET", "yes".into())],
            files: vec!["mock.conf"],
            later: None,
            switched: Arc::default(),
        }
    }

    /// What it says now: its later declarations once switched, else its first.
    fn now(&self) -> Option<&Later> {
        self.later.as_ref().filter(|_| self.switched.load(Ordering::SeqCst))
    }
}

fn number(word: Option<&str>) -> Result<u64, String> {
    word.and_then(|w| w.parse().ok()).ok_or_else(|| "an unreadable event".to_string())
}

impl Harness for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn route(&self) -> Route {
        Route { host: "llm.mock.invalid".into(), port: 443, connect: None }
    }

    fn secret(&self) -> &str {
        self.now().map_or(&self.secret, |l| &l.secret)
    }

    fn pin(&self) -> &Pin {
        self.now().map_or(&self.pin, |l| &l.pin)
    }

    fn profile(&self) -> Profile {
        self.profile
    }

    fn env_names(&self) -> &[&str] {
        self.now().map_or(&self.names, |l| &l.names)
    }

    fn config(&self, name: &str, _socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        Ok(self.files.iter().map(|f| (*f, format!("conf for {name}").into_bytes())).collect())
    }

    fn launch(&self, turn: &Turn) -> Result<Launch, String> {
        let mut l = Launch::default();
        l.args(["-c", &self.script, "bash", turn.message]);
        for (k, v) in &self.env {
            l.env(k, v);
        }
        l.env("MOCK_CONF", turn.config);
        Ok(l)
    }

    fn event(&self, line: &str, first: bool, _turn: &Turn) -> Result<Event, String> {
        let mut words = line.split(' ');
        let kind = words.next().unwrap_or_default();
        if first {
            let why: Vec<&str> = words.collect();
            return Ok(match (kind, why.as_slice()) {
                ("init", ["ok"]) => Event::Init(Ok(())),
                ("init", why) => Event::Init(Err(why.join(" "))),
                _ => Event::Other,
            });
        }
        match kind {
            "call" => {
                let id = words.next().ok_or("an unreadable event")?.to_string();
                let usage = Tokens { input: number(words.next())?, output: number(words.next())?, ..Tokens::default() };
                let tools = words
                    .map(|w| w.split_once(':').map(|(i, n)| (i.into(), n.into())).ok_or("an unreadable event"))
                    .collect::<Result<_, _>>()?;
                Ok(Event::Message { id, model: json!("mock"), usage, tools })
            }
            "result" => {
                let usage = Tokens { input: number(words.next())?, output: number(words.next())?, ..Tokens::default() };
                Ok(Event::Result { usage, ended: json!("success"), cost_usd: json!(0.5), duration_ms: json!(7) })
            }
            _ => Ok(Event::Other),
        }
    }
}

struct Setup {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    socket: PathBuf,
    backend: HarnessBackend<Mock>,
}

fn backend_config(root: &Path, caps: Caps) -> BackendConfig {
    BackendConfig {
        caps,
        dir: root.join("backend"),
        shim: PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment")),
        turns: 4,
    }
}

impl Setup {
    fn new(script: &str, caps: Caps) -> Setup {
        Setup::with(Mock::new(script), caps)
    }

    fn with(mock: Mock, caps: Caps) -> Setup {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let cfg = backend_config(&root, caps);
        let dir = cfg.dir.clone();
        let backend = HarnessBackend::start(mock, cfg).expect("the backend");
        Setup { _tmp: tmp, dir, socket: root.join("agent.sock"), backend }
    }

    /// Plays agent-0's turn `turn`. The mock never talks to the driver, so its socket needn't exist.
    fn turn(&self, turn: u32) -> Result<(), String> {
        self.backend.turn(0, "agent-0", turn, &self.socket)
    }

    fn usage(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("usage.jsonl")).expect("usage.jsonl");
        text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
    }

    fn transcript(&self, turn: u32) -> Option<String> {
        std::fs::read_to_string(self.dir.join(format!("transcripts/agent-0-{turn}.jsonl"))).ok()
    }

    /// The lines of a turn's transcript that start with `word `, without it.
    fn said(&self, turn: u32, word: &str) -> Vec<String> {
        let t = self.transcript(turn).expect("a transcript");
        t.lines().filter_map(|l| l.strip_prefix(&format!("{word} ")).map(String::from)).collect()
    }
}

fn caps() -> Caps {
    Caps { turn_time: Duration::from_secs(20), ..Caps::pilot() }
}

/// A loop that runs until its process group is killed, without starting a program (the sandbox lets the
/// mock start only its pinned bash).
const FOREVER: &str = "while :; do :; done";

/// Says what standard input is, `stdin null` or `stdin other`, from what fd 0 is rather than whether it
/// delivers data: an inherited stdin that sends nothing would look empty too (finding c758214ceca6).
const STDIN_CHECK: &str = "if [[ /dev/stdin -ef /dev/null ]]; then echo \"stdin null\"; else echo \"stdin other\"; fi";

#[test]
fn the_stdin_check_tells_null_from_an_inherited_stdin() {
    // The control: the same check, unsandboxed, says other for a pipe nothing is written to, as an
    // inherited stdin would be without .stdin(Stdio::null()), and null for null.
    for (stdin, want) in [(Stdio::piped(), "stdin other\n"), (Stdio::null(), "stdin null\n")] {
        let out = Command::new("/bin/bash").args(["-c", STDIN_CHECK]).stdin(stdin).output().expect("bash");
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
    }
}

#[test]
fn an_entry_the_harness_didnt_declare_stops_the_run_before_the_turn_starts() {
    let mock = Mock { env: vec![("MOCK_SET", "yes".into()), ("SNEAKY", "1".into())], ..Mock::new("echo init ok") };
    let s = Setup::with(mock, caps());
    assert_eq!(s.turn(1), Err("mock set `SNEAKY`, an environment entry it didn't declare".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
    assert!(s.backend.halted().is_some());
}

#[test]
fn a_harness_may_not_declare_the_backends_own_entries() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    for name in ["HOME", "PATH", "TMPDIR", "HTTPS_PROXY"] {
        let mock = Mock { names: vec![name], env: vec![], ..Mock::new("echo init ok") };
        let cfg = backend_config(tmp.path(), caps());
        let err = HarnessBackend::start(mock, cfg).err().expect("refused");
        assert_eq!(err, format!("mock declares {name}, which the backend sets"));
    }
    assert!(!tmp.path().join("backend").exists(), "refused before anything was made");
}

#[test]
fn a_harness_may_not_declare_a_proxy_setting_a_base_entry_in_another_case_or_a_name_that_isnt_one() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let refused = |name: &'static str| {
        let mock = Mock { names: vec!["MOCK_SET", name], ..Mock::new("echo init ok") };
        HarnessBackend::start(mock, backend_config(tmp.path(), caps())).err().expect("refused")
    };
    for name in ["HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy", "all_proxy", "npm_config_proxy"] {
        let want = format!(
            "mock declares {name}, a proxy setting: only the backend's HTTPS_PROXY says where the agent connects"
        );
        assert_eq!(refused(name), want);
    }
    for name in ["home", "Path", "tmpdir", "https_proxy", "Https_Proxy"] {
        assert_eq!(refused(name), format!("mock declares {name}, which the backend sets"));
    }
    for (name, shown) in
        [("BAD-NAME", "`BAD-NAME`"), ("", "``"), ("1ST", "`1ST`"), ("A=B", "`A=B`"), ("A\nB", "`A\\nB`")]
    {
        assert_eq!(refused(name), format!("mock declares {shown}, which isn't an environment variable name"));
    }
    assert!(!tmp.path().join("backend").exists(), "refused before anything was made");
    let mock = Mock { names: vec!["MOCK_SET", "MOCK_CONF", "_x1", "my_Proxy_host"], ..Mock::new("echo init ok") };
    let err = HarnessBackend::start(mock, backend_config(tmp.path(), caps())).err().expect("refused");
    assert!(err.contains("my_Proxy_host, a proxy setting"), "any name holding PROXY, in any case: {err}");
    let mock = Mock { names: vec!["MOCK_SET", "MOCK_CONF", "_x1", "Mixed_Case"], ..Mock::new("echo init ok") };
    HarnessBackend::start(mock, backend_config(tmp.path(), caps())).expect("plain names are fine");
}

#[test]
fn a_turn_configuration_file_name_that_isnt_one_plain_name_stops_the_run_before_anything_is_written() {
    let cases: [(Vec<&'static str>, &str); 12] = [
        (vec!["../x"], "the turn configuration's file name `../x` isn't one plain name"),
        (vec!["/abs"], "the turn configuration's file name `/abs` isn't one plain name"),
        (vec!["a/b"], "the turn configuration's file name `a/b` isn't one plain name"),
        (vec![""], "the turn configuration's file name `` isn't one plain name"),
        (vec!["."], "the turn configuration's file name `.` isn't one plain name"),
        (vec!["mock.conf", ".."], "the turn configuration's file name `..` isn't one plain name"),
        (vec!["mock.conf", "mock.conf"], "the turn configuration names mock.conf twice"),
        (vec!["mock.conf", "Mock.CONF"], "the turn configuration names Mock.CONF twice (as mock.conf)"),
        (vec!["a\0b"], "the turn configuration's file name `a\\0b` isn't one plain name"),
        (vec!["caf\u{e9}"], "the turn configuration's file name `caf\u{e9}` isn't one plain name"),
        (vec!["cafe\u{301}"], "the turn configuration's file name `cafe\u{301}` isn't one plain name"),
        (vec!["a b"], "the turn configuration's file name `a b` isn't one plain name"),
    ];
    for (files, want) in cases {
        let s = Setup::with(Mock { files, ..Mock::new("echo init ok; echo result 0 1") }, caps());
        assert_eq!(s.turn(1), Err(want.into()));
        assert!(s.backend.halted().is_some());
        assert!(s.transcript(1).is_none(), "nothing started");
        assert!(!s.dir.join("turn/agent-0").exists(), "nothing written");
        assert!(!s.dir.join("turn/x").exists() && !s.dir.join("x").exists(), "nothing written outside");
    }
}

/// A backend started with `script` and `set` as the mock's entries, whose mock then declares another
/// secret, a pin of `other`, and `LATER` as well as its names.
fn switched(script: &str, set: Vec<(&'static str, String)>, other: &Path) -> Setup {
    let later = Later {
        secret: "a different secret".into(),
        pin: Pin::of(other, "mock").expect("a pin"),
        names: vec!["MOCK_SET", "MOCK_CONF", "LATER"],
    };
    let mock = Mock { env: set, later: Some(later), ..Mock::new(script) };
    let flag = mock.switched.clone();
    let s = Setup::with(mock, caps());
    flag.store(true, Ordering::SeqCst);
    s
}

/// Another file, which the mock's later pin names.
fn other(tmp: &tempfile::TempDir) -> PathBuf {
    let other = tmp.path().join("other");
    std::fs::write(&other, "not a program").expect("another file");
    other
}

#[test]
fn an_entry_declared_only_after_the_start_is_still_one_the_harness_didnt_declare() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let set = vec![("MOCK_SET", "yes".to_string()), ("LATER", "1".into())];
    let s = switched("echo init ok; echo result 0 1", set, &other(&tmp));
    assert_eq!(s.turn(1), Err("mock set `LATER`, an environment entry it didn't declare".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
}

#[test]
fn a_secret_under_8_bytes_is_refused_at_the_start() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    for secret in ["", "a", "1234567"] {
        let mock = Mock { secret: secret.into(), ..Mock::new("echo init ok") };
        let err = HarnessBackend::start(mock, backend_config(tmp.path(), caps())).err().expect("refused");
        assert!(err.contains(&format!("{} bytes, under 8", secret.len())), "{err}");
    }
    let mock = Mock { secret: "12345678".into(), ..Mock::new("echo init ok") };
    HarnessBackend::start(mock, backend_config(tmp.path(), caps())).expect("8 bytes is enough");
}

#[test]
fn a_changed_pin_stops_the_run_before_the_turn_starts() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let program = tmp.path().join("program");
    std::fs::write(&program, "one").expect("a program");
    let mock = Mock { pin: Pin::of(&program, "mock").expect("a pin"), ..Mock::new("echo init ok") };
    let s = Setup::with(mock, caps());
    std::fs::write(&program, "two").expect("changed");
    assert_eq!(s.turn(1), Err("the pinned mock binary changed".into()));
    assert!(s.transcript(1).is_none(), "nothing started");
    assert!(s.backend.halted().is_some());
}

/// A tree to pin, in `tmp`: `root/bin/prog`, `root/lib/pkg/index.js`, `root/lib/pkg/data/a.json`,
/// `root/lib/current`, a link to `pkg`, an empty directory `root/lib/e` (mode 755) and a file
/// `root/lib/755`. Returns the root, resolved. Its program never runs: every test
/// here stops before the turn starts. A tree-pinned turn is in `tests/agent.rs`, with a program macOS will
/// run from anywhere, which no copy of a system binary such as bash is.
fn tree(tmp: &Path) -> PathBuf {
    let root = std::fs::canonicalize(tmp).expect("resolved").join("root");
    std::fs::create_dir_all(root.join("bin")).expect("bin");
    std::fs::create_dir_all(root.join("lib/pkg/data")).expect("lib");
    std::fs::write(root.join("bin/prog"), "the program\n").expect("a program");
    std::fs::write(root.join("lib/pkg/index.js"), "module.exports = 1;\n").expect("a file");
    std::fs::write(root.join("lib/pkg/data/a.json"), "{}\n").expect("a file");
    symlink("pkg", root.join("lib/current")).expect("a link");
    std::fs::create_dir(root.join("lib/e")).expect("an empty directory");
    std::fs::set_permissions(root.join("lib/e"), std::fs::Permissions::from_mode(0o755)).expect("its mode");
    std::fs::write(root.join("lib/755"), "").expect("a file");
    root
}

/// Makes something in a tree (its root, then a path outside it).
type Make = fn(&Path, &Path);

/// Changes a pinned tree, given its root.
type Change = fn(&Path);

#[test]
fn a_tree_pin_refuses_what_could_reach_outside_its_tree() {
    let cases: [(&str, Make); 5] = [
        ("lib/out links to ../../outside, which isn't inside the tree", |root, out| {
            std::fs::write(out, "outside").expect("a file outside");
            symlink("../../outside", root.join("lib/out")).expect("a link");
        }),
        ("lib/gone links to nowhere, which isn't inside the tree", |root, _| {
            symlink("nowhere", root.join("lib/gone")).expect("a link");
        }),
        ("lib/pkg/twin.js has another hard link", |root, out| {
            std::fs::write(root.join("lib/pkg/twin.js"), "x").expect("a file");
            std::fs::hard_link(root.join("lib/pkg/twin.js"), out).expect("a hard link");
        }),
        ("lib/fifo isn't a file, directory or symbolic link", |root, _| {
            let made = Command::new("/usr/bin/mkfifo").arg(root.join("lib/fifo")).status().expect("mkfifo");
            assert!(made.success());
        }),
        ("bin/../.. isn't a file inside", |_, _| {}),
    ];
    for (want, make) in cases {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = tree(tmp.path());
        make(&root, &tmp.path().join("outside"));
        let program = if want.starts_with("bin/../..") { tmp.path().join("outside") } else { root.join("bin/prog") };
        if !program.exists() {
            std::fs::write(&program, "outside").expect("a program outside");
        }
        let err = Pin::tree(&program, &root, "1").expect_err(want);
        let want =
            want.strip_prefix("bin/../.. ").map_or(want.to_string(), |rest| format!("{} {rest}", program.display()));
        assert!(err.starts_with(&want), "{want}: {err}");
    }
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = tree(tmp.path());
    Pin::tree(&root.join("bin/prog"), &root, "1").expect("a link inside the tree is fine");
}

/// Makes links in a tree (its resolved root, then the resolved directory around it) and says the error
/// the tree is refused with.
type Links = fn(&Path, &Path) -> String;

#[test]
fn a_tree_pin_refuses_a_link_that_leaves_the_tree_on_the_way_even_if_it_comes_back() {
    let cases: [(&str, Links); 5] = [
        ("out and back, absolute", |root, tmp| {
            symlink(root.join("lib/pkg"), tmp.join("hop")).expect("a link outside, back in");
            symlink(tmp.join("hop"), root.join("lib/x")).expect("a link out");
            format!("lib/x links to {}, which leads to an absolute path", tmp.join("hop").display())
        }),
        ("out and back, relative", |root, tmp| {
            symlink("root/lib/pkg", tmp.join("hop")).expect("a link outside, back in");
            symlink("../../hop", root.join("lib/x")).expect("a link out");
            "lib/x links to ../../hop, which isn't inside the tree".into()
        }),
        ("above the root after a link inside it", |root, tmp| {
            std::fs::write(tmp.join("outside"), "outside").expect("a file outside");
            // Read as text, without following `up`, the target is lib/outside, which is there too.
            std::fs::write(root.join("lib/outside"), "inside").expect("a file inside");
            symlink("..", root.join("lib/up")).expect("a link to the root");
            symlink("up/../outside", root.join("lib/x")).expect("a link up from it");
            "lib/x links to up/../outside, which isn't inside the tree".into()
        }),
        ("absolute, though to inside", |root, _| {
            symlink(root.join("lib/pkg"), root.join("lib/x")).expect("an absolute link");
            format!("lib/x links to {}, which leads to an absolute path", root.join("lib/pkg").display())
        }),
        ("a loop", |root, _| {
            symlink("y", root.join("lib/x")).expect("a link");
            symlink("x", root.join("lib/y")).expect("a link back");
            "lib/x links to y, which isn't inside the tree".into()
        }),
    ];
    for (what, make) in cases {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = tree(tmp.path());
        let want = make(&root, root.parent().expect("a parent"));
        assert_eq!(Pin::tree(&root.join("bin/prog"), &root, "1").err(), Some(want), "{what}");
    }
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = tree(tmp.path());
    symlink("../../lib/current/data/a.json", root.join("lib/pkg/back")).expect("a link up and back, inside");
    Pin::tree(&root.join("bin/prog"), &root, "1").expect("links that stay inside, through a link, are fine");
}

#[test]
fn a_tree_pins_hash_is_its_manifest_in_name_order() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("resolved").join("root");
    let mode = |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).expect("a mode");
    std::fs::create_dir_all(root.join("b")).expect("dirs");
    // Made in reverse, so only the walk's sort, not the file system, puts them in name order.
    let names = ["h", "g", "f", "e", "d", "c", "b", "a"];
    for n in names {
        std::fs::write(root.join("b").join(n), n).expect("a file");
        mode(&root.join("b").join(n), 0o644);
    }
    std::fs::write(root.join("a.txt"), "x").expect("a file");
    mode(&root.join("a.txt"), 0o600);
    symlink("b/a", root.join("l")).expect("a link");
    mode(&root.join("b"), 0o755);
    mode(&root, 0o755);

    // The manifest, by hand: each field ends in NUL; kind, path, then mode, contents' hash or target.
    let mut m: Vec<u8> = Vec::new();
    let mut entry = |fields: &[&str]| fields.iter().for_each(|f| m.extend([f.as_bytes(), b"\0"].concat()));
    let hash = |b: &str| blake3::hash(b.as_bytes()).to_hex().to_string();
    entry(&["d", "", "755"]);
    entry(&["f", "a.txt", "600", &hash("x")]);
    entry(&["d", "b", "755"]);
    for n in names.iter().rev() {
        entry(&["f", &format!("b/{n}"), "644", &hash(n)]);
    }
    entry(&["l", "l", "b/a"]);
    let pin = Pin::tree(&root.join("b/a"), &root, "1").expect("a pin");
    assert_eq!(pin.blake3(), blake3::hash(&m).to_hex().as_str());
}

#[test]
fn a_tree_pins_hash_is_its_content_not_the_order_it_was_made_in() {
    let (one, two) = (tempfile::tempdir().expect("a temp dir"), tempfile::tempdir().expect("a temp dir"));
    let a = tree(one.path());
    let root = std::fs::canonicalize(two.path()).expect("resolved").join("root");
    // The same tree, made in another order.
    std::fs::create_dir_all(root.join("lib/pkg/data")).expect("lib");
    symlink("pkg", root.join("lib/current")).expect("a link");
    std::fs::write(root.join("lib/pkg/data/a.json"), "{}\n").expect("a file");
    std::fs::write(root.join("lib/pkg/index.js"), "module.exports = 1;\n").expect("a file");
    std::fs::create_dir_all(root.join("bin")).expect("bin");
    std::fs::write(root.join("bin/prog"), "the program\n").expect("a program");
    std::fs::write(root.join("lib/755"), "").expect("a file");
    std::fs::create_dir(root.join("lib/e")).expect("an empty directory");
    std::fs::set_permissions(root.join("lib/e"), std::fs::Permissions::from_mode(0o755)).expect("its mode");
    let pin = |r: &Path| Pin::tree(&r.join("bin/prog"), r, "1").expect("a pin").blake3().to_string();
    assert_eq!(pin(&a), pin(&root));
}

#[test]
fn a_changed_pinned_tree_stops_the_run_before_the_turn_starts() {
    let changes: [(&str, Change); 11] = [
        ("a file deep in it", |r| std::fs::write(r.join("lib/pkg/data/a.json"), "{\"x\":1}\n").expect("changed")),
        ("a file added", |r| std::fs::write(r.join("lib/pkg/new.js"), "").expect("added")),
        ("a file removed", |r| std::fs::remove_file(r.join("lib/pkg/index.js")).expect("removed")),
        ("a file renamed", |r| {
            std::fs::rename(r.join("lib/pkg/index.js"), r.join("lib/pkg/main.js")).expect("renamed")
        }),
        ("a mode", |r| {
            std::fs::set_permissions(r.join("lib/pkg/index.js"), std::fs::Permissions::from_mode(0o755)).expect("chmod")
        }),
        ("a link's target, still inside", |r| {
            std::fs::remove_file(r.join("lib/current")).expect("unlinked");
            symlink("pkg/data", r.join("lib/current")).expect("relinked");
        }),
        ("the program", |r| std::fs::write(r.join("bin/prog"), "#!/bin/sh\n").expect("changed")),
        ("an empty directory added", |r| std::fs::create_dir(r.join("lib/pkg/empty")).expect("added")),
        ("a directory's mode", |r| {
            std::fs::set_permissions(r.join("lib/pkg/data"), std::fs::Permissions::from_mode(0o700)).expect("chmod")
        }),
        ("a file swapped for a link", |r| {
            std::fs::remove_file(r.join("lib/pkg/index.js")).expect("removed");
            symlink("data/a.json", r.join("lib/pkg/index.js")).expect("a link in its place");
        }),
        // Without the kind tag, ('d', lib/e, 755) and ('l', lib/e, 755) would be the same fields.
        ("a directory swapped for a link whose target is its mode", |r| {
            std::fs::remove_dir(r.join("lib/e")).expect("removed");
            symlink("755", r.join("lib/e")).expect("a link in its place");
        }),
    ];
    for (what, change) in changes {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = tree(tmp.path());
        let pin = Pin::tree(&root.join("bin/prog"), &root, "mock").expect("a pin");
        let s = Setup::with(Mock { pin, profile: Profile::Node, ..Mock::new("echo init ok; echo result 0 1") }, caps());
        change(&root);
        assert_eq!(s.turn(1), Err("the pinned mock install changed: its manifest's hash differs".into()), "{what}");
        assert!(s.transcript(1).is_none(), "{what}: nothing started");
        assert!(s.backend.halted().is_some(), "{what}");
    }
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = tree(tmp.path());
    let pin = Pin::tree(&root.join("bin/prog"), &root, "mock").expect("a pin");
    let outside = tmp.path().join("elsewhere");
    let s = Setup::with(Mock { pin, profile: Profile::Node, ..Mock::new("echo init ok") }, caps());
    std::fs::rename(&root, &outside).expect("moved");
    symlink(&outside, &root).expect("the root, now a link");
    let err = s.turn(1).expect_err("moved");
    assert!(err.starts_with("the pinned mock install changed: "), "{err}");
}

/// The tests that play a turn, which the backend runs under the agent profile: they need macOS, for
/// sandbox-exec. Linux ignores them, and `scripts/test-table --split` runs them on the Mac by this
/// module's name (owner decision `01a2d668b67e`).
mod macos_only {
    use super::*;

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn plain_turn_configuration_file_names_dotted_or_not_are_written() {
        let mock = Mock { files: vec!["mock.conf", ".hidden", "a..b"], ..Mock::new("echo init ok; echo result 0 1") };
        let s = Setup::with(mock, caps());
        s.turn(1).expect("plain names, dotted or not, are fine");
        assert!(s.dir.join("turn/agent-0/.hidden").exists());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_run_keeps_the_secret_and_pin_the_start_checked() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let other = other(&tmp);
        let set = || vec![("MOCK_SET", "yes".to_string())];

        // The pinned bash still plays the turn and is what's checked, though the harness now names a file
        // that can't run and no longer matches its own pin.
        let s = switched("echo init ok; echo result 0 1", set(), &other);
        std::fs::write(&other, "changed").expect("changed");
        s.turn(1).expect("the start's pin");

        // The start's secret is still refused in the stream, though the harness now gives another.
        let s = switched(&format!("echo init ok; echo {SECRET}; echo result 0 1"), set(), &other);
        assert_eq!(s.turn(1), Err("the event stream held the auth value".into()));
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_turn_is_started_gated_counted_and_kept() {
        let script = "echo init ok; echo \"note $1\"; echo call m1 1000 5 t1:notes_read; \
                      echo call m1 1000 9 t1:notes_read; echo call m2 1200 20; echo result 2200 40; echo warn >&2";
        let s = Setup::new(script, caps());
        s.turn(1).expect("the turn");
        let rows = s.usage();
        assert_eq!(rows[0], json!({"agent": "agent-0", "turn": 1, "gate": true}));
        let calls: Vec<_> = rows.iter().filter(|r| r["message"].is_string()).collect();
        assert_eq!(calls.len(), 3, "every call event is recorded: {rows:?}");
        assert_eq!(calls[0]["tools"], json!(["notes_read"]));
        let settled = rows.last().expect("a settled row");
        assert_eq!(
            (&settled["ended"], &settled["input"], &settled["output"]),
            (&json!("success"), &json!(2200), &json!(40))
        );
        assert_eq!(settled["cost_usd"], 0.5);
        let (mine, run) = s.backend.totals("agent-0");
        assert_eq!(
            (mine.input, mine.output),
            (2200, 40),
            "a call's repeated usage counts once; the result's output wins"
        );
        assert_eq!(mine, run);
        assert_eq!(s.said(1, "note"), ["This is your turn 1 of 4. You are agent-0. Start by reading your notes."]);
        let stderr = std::fs::read_to_string(s.dir.join("transcripts/agent-0-1.stderr")).expect("stderr");
        assert_eq!(stderr, "warn\n");
        assert!(s.backend.halted().is_none());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_backend_builds_the_environment_and_the_harness_adds_only_what_it_declared() {
        let script = format!(
            "echo init ok; for n in $(compgen -e); do echo \"name $n\"; done; \
             echo \"home $HOME\"; echo \"tmp $TMPDIR\"; echo \"proxy $HTTPS_PROXY\"; echo \"path $PATH\"; \
             echo \"set $MOCK_SET\"; {STDIN_CHECK}; echo result 0 1"
        );
        let s = Setup::new(&script, caps());
        s.turn(1).expect("the turn");
        // bash sets PWD, SHLVL and _ itself, and OLDPWD where it has one.
        let shell = ["PWD", "SHLVL", "_", "OLDPWD"];
        let names: BTreeSet<String> = s.said(1, "name").into_iter().filter(|n| !shell.contains(&n.as_str())).collect();
        let want: BTreeSet<String> =
            ["HOME", "TMPDIR", "PATH", "HTTPS_PROXY", "MOCK_SET", "MOCK_CONF"].iter().map(|n| n.to_string()).collect();
        assert_eq!(names, want, "nothing of the driver's environment, only the base and the declared entries");
        let own = s.dir.join("agents/agent-0");
        assert_eq!(s.said(1, "home"), [own.join("home").display().to_string()]);
        assert_eq!(s.said(1, "tmp"), [own.join("tmp").display().to_string()]);
        assert_eq!(s.said(1, "proxy"), [format!("http://127.0.0.1:{}", s.backend.proxy_port())]);
        assert_eq!(s.said(1, "path"), ["/usr/bin:/bin"]);
        assert_eq!(s.said(1, "set"), ["yes"]);
        assert_eq!(s.said(1, "stdin"), ["null"], "standard input is /dev/null, not the driver's");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_secret_of_8_bytes_plays_a_turn() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let mock = Mock { secret: "12345678".into(), ..Mock::new("echo init ok; echo result 0 1") };
        let backend = HarnessBackend::start(mock, backend_config(tmp.path(), caps())).expect("8 bytes is enough");
        backend.turn(0, "agent-0", 1, &tmp.path().join("agent.sock")).expect("the turn");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_turn_runs_in_the_agent_sandbox() {
        let script = "echo init ok; \
                      if /usr/bin/true 2>/dev/null; then echo exec allowed; else echo exec refused; fi; \
                      if (echo x > \"$MOCK_CONF/new\") 2>/dev/null; then echo write allowed; else echo write refused; fi; \
                      if (echo x > \"$HOME/new\") 2>/dev/null; then echo own allowed; else echo own refused; fi; \
                      read -r conf < \"$MOCK_CONF/mock.conf\"; echo \"conf $conf\"; echo result 0 1";
        // Without the sandbox, the same attempts work (gotcha fd7742fa71ce: show the refusal is the sandbox's).
        let tmp = tempfile::tempdir().expect("a temp dir");
        std::fs::write(tmp.path().join("mock.conf"), "conf").expect("a conf");
        let out = Command::new("/bin/bash")
            .args(["-c", script])
            .env("MOCK_CONF", tmp.path())
            .env("HOME", tmp.path())
            .stdin(Stdio::null())
            .output()
            .expect("bash");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("exec allowed") && text.contains("write allowed"), "{text}");

        let s = Setup::new(script, caps());
        s.turn(1).expect("the turn");
        assert_eq!(s.said(1, "exec"), ["refused"], "only the pinned program and the shim may start");
        assert_eq!(s.said(1, "write"), ["refused"], "the turn configuration is read-only");
        assert_eq!(s.said(1, "own"), ["allowed"], "the agent's own directory is writable");
        assert_eq!(s.said(1, "conf"), ["conf for agent-0"], "the configuration is readable");
        let conf = s.dir.join("turn/agent-0/mock.conf");
        let mode = std::fs::metadata(&conf).expect("the conf").permissions().mode();
        assert_eq!(mode & 0o777, 0o400);
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_changed_configuration_stops_the_run_before_the_turn_starts() {
        let s = Setup::new("echo init ok; echo result 0 1", caps());
        s.turn(1).expect("the first turn");
        let conf = s.dir.join("turn/agent-0/mock.conf");
        std::fs::set_permissions(&conf, std::fs::Permissions::from_mode(0o600)).expect("writable");
        std::fs::write(&conf, "changed").expect("changed");
        assert_eq!(s.turn(2), Err("the turn configuration's mock.conf changed".into()));
        assert!(s.transcript(2).is_none(), "nothing started");
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_refused_gate_stops_the_run() {
        let s = Setup::new("echo init the model is wrong; echo result 0 1", caps());
        let err = s.turn(1).expect_err("refused");
        assert_eq!(err, "the gate refused agent-0's turn 1: the model is wrong");
        assert_eq!(s.usage()[0]["gate"], false);
        assert_eq!(s.backend.halted(), Some(err.clone()));
        assert_eq!(s.turn(2), Err(err), "no further turn");
        assert!(s.transcript(2).is_none());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_first_event_that_isnt_init_stops_the_run() {
        let s = Setup::new("echo call m1 1 1; echo result 0 1", caps());
        let err = s.turn(1).expect_err("refused");
        assert!(err.contains("the first event isn't init"), "{err}");
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_turn_without_a_result_counts_at_its_output_cap() {
        let s = Setup::new("echo init ok; echo call m1 100 50", caps());
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended without a result".into()));
        assert_eq!(s.backend.totals("agent-0").0.output, caps().turn_output);
        assert!(s.usage().iter().any(|r| r["output_counted_at"] == caps().turn_output));
        assert!(s.backend.halted().is_none(), "only the turn ends");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn an_unreadable_event_stops_the_run() {
        let s = Setup::new("echo init ok; echo call m1 lots 1", caps());
        assert_eq!(s.turn(1), Err("an unreadable event".into()));
        assert!(s.backend.halted().is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_stream_and_result_that_disagree_by_over_10_percent_stop_the_run() {
        // The gap is measured against the larger figure: 112 of 1112 is over a tenth, 111 of 1111 isn't.
        let s = Setup::new("echo init ok; echo call m1 1000 1; echo result 1112 1", caps());
        let err = s.turn(1).expect_err("stopped");
        assert_eq!(err, "agent-0's turn 1: the stream and the result disagree on input by over 10%");
        assert_eq!(s.backend.totals("agent-0").0.input, 1112, "the larger is counted before the stop");
        assert!(s.backend.halted().is_some());

        let s = Setup::new("echo init ok; echo call m1 1000 1; echo result 1111 1", caps());
        s.turn(1).expect("just within");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_secret_is_never_kept() {
        let s = Setup::new(&format!("echo init ok; echo {SECRET}; echo result 0 1"), caps());
        let err = s.turn(1).expect_err("stopped");
        assert_eq!(err, "the event stream held the auth value");
        assert!(!s.transcript(1).expect("the lines before it").contains(SECRET));
        assert!(s.backend.halted().is_some());

        let s = Setup::new(&format!("echo init ok; echo {SECRET} >&2; echo result 0 1"), caps());
        let err = s.turn(1).expect_err("stopped");
        assert!(err.starts_with("a transcript held the auth value"), "{err}");
        assert!(s.transcript(1).is_none(), "removed");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_tool_call_cap_gives_grace_then_ends_the_turn() {
        let caps = Caps { calls_per_turn: 1, cap_grace_calls: 1, ..caps() };
        let script =
            format!("echo init ok; echo call m1 10 1 a:x b:y; echo call m2 10 1; echo call m3 10 1; {FOREVER}");
        let s = Setup::new(&script, caps);
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its tool calls cap".into()));
        assert!(s.usage().iter().any(|r| r["past_tool_call_cap"] == 2));
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn a_cap_kills_the_harness_and_everything_it_started() {
        let script = format!("({FOREVER}) & echo $! > \"$HOME/pid\"; echo init ok; echo call m1 200000 1; wait");
        let s = Setup::new(&script, caps());
        let start = Instant::now();
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its context cap".into()));
        assert!(start.elapsed() < Duration::from_secs(60), "{:?}", start.elapsed());
        let pid = std::fs::read_to_string(s.dir.join("agents/agent-0/home/pid")).expect("the pid");
        let alive = || {
            let kill = Command::new("/bin/kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status();
            kill.expect("kill").success()
        };
        let gone = Instant::now() + Duration::from_secs(30);
        while alive() && Instant::now() < gone {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(), "the loop the harness started was killed with its group");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn spent_tokens_end_an_agents_turns_and_the_runs_stop_the_run() {
        let script = format!("echo init ok; echo call m1 60 50; {FOREVER}");
        let s = Setup::new(&script, Caps { agent_tokens: 100, ..caps() });
        assert_eq!(s.turn(1), Err("agent-0's turn 1 ended at its agent tokens cap".into()));
        assert_eq!(s.turn(2), Err("agent-0 has spent its tokens".into()));
        assert!(s.backend.halted().is_none(), "the run goes on");

        let s = Setup::new(&script, Caps { run_tokens: 100, ..caps() });
        assert_eq!(s.turn(1), Err(RUN_TOKEN_CAP.into()));
        assert_eq!(s.backend.halted(), Some(RUN_TOKEN_CAP.into()));
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs macOS sandbox-exec")]
    fn the_run_time_cap_stops_a_turn_and_the_run() {
        let s = Setup::new(&format!("echo init ok; {FOREVER}"), Caps { run_time: Duration::from_secs(2), ..caps() });
        let start = Instant::now();
        assert_eq!(s.turn(1), Err(RUN_TIME_CAP.into()));
        assert!(start.elapsed() < Duration::from_secs(60), "{:?}", start.elapsed());
        assert_eq!(s.backend.halted(), Some(RUN_TIME_CAP.into()));
        assert_eq!(s.turn(2), Err(RUN_TIME_CAP.into()));
        assert!(s.transcript(2).is_none());
    }
}
