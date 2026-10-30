//! The Pi harness with a real Pi (task 68675575fd58; design note e19ff6fc8285): the node of a prefix
//! `scripts/pi-prefix` built, under the Node profile, and one whole Pi turn through the forwarding
//! proxy to a stub provider (`tests/pi-stub.mjs`). No model is called. Both need the prefix and macOS,
//! so both are ignored unless asked for: `scripts/pi-prefix <dir>`, then
//! `EXPERIMENT_PI_PREFIX=<dir> cargo test -p bracken-experiment --test pi -- --ignored`.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use bracken_experiment::claude::{Caps, Routing};
use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, core_tools, git_binary, run};
use bracken_experiment::mode::mode_named;
use bracken_experiment::pi::{CLI, PiBackend, PiConfig, PiExpect};
use bracken_experiment::plan::PlanInputs;
use bracken_experiment::sandbox::{AgentPaths, Profile, Sandbox, resolved};
use bracken_experiment::task::Task;

const FAKE_AUTH: &str = "fake-auth-value-not-a-secret-0123456789";
const HOST: &str = "llm.stub.invalid";
const MODEL: &str = "claude-opus-5-5";
const PI_VERSION: &str = "1.4.0";

fn prefix() -> PathBuf {
    let p = PathBuf::from(std::env::var("EXPERIMENT_PI_PREFIX").expect("EXPERIMENT_PI_PREFIX: a pi-prefix dir"));
    resolved(&p).expect("the prefix")
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn bracken() -> PathBuf {
    let b = bin().with_file_name("bracken");
    assert!(b.exists(), "build bracken first: cargo build -p bracken");
    b
}

fn tools() -> Vec<String> {
    let mode = mode_named("bracken").expect("a mode");
    mode.tools(Task::Phase1).into_iter().chain(core_tools()).map(|t| t.name.to_string()).collect()
}

/// What node, under a profile, tries: each `read`, `write` (a new file), `exec` and `tcp` target, as
/// `{"<kind>:<target>": "allowed" | "denied"}`.
const PROBE: &str = r#"
const fs = require("node:fs"), net = require("node:net"), cp = require("node:child_process");
const spec = JSON.parse(process.argv[1]), out = {};
for (const p of spec.read || []) { try { fs.readFileSync(p); out["read:" + p] = "allowed"; } catch { out["read:" + p] = "denied"; } }
for (const p of spec.write || []) { try { fs.writeFileSync(p, "x", { flag: "wx" }); fs.rmSync(p); out["write:" + p] = "allowed"; } catch { out["write:" + p] = "denied"; } }
for (const p of spec.exec || []) { const r = cp.spawnSync(p, ["-c", "exit 0"]); out["exec:" + p] = r.error ? "denied" : "allowed"; }
(async () => {
  for (const t of spec.tcp || []) {
    const [host, port] = t.split(":");
    out["tcp:" + t] = await new Promise((done) => {
      const s = net.connect({ host, port: Number(port) }, () => { s.destroy(); done("allowed"); });
      s.on("error", () => done("denied"));
    });
  }
  console.log(JSON.stringify(out));
})();
"#;

fn probe(cmd: &mut Command, spec: &Value) -> Value {
    let out = cmd.arg("-e").arg(PROBE).arg(spec.to_string()).stdin(Stdio::null()).output().expect("node");
    assert!(out.status.success(), "probe: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("JSON")
}

/// Pi's node under the Node profile: reads its tree, writes its own directory, reaches the proxy's port;
/// reads nothing of the owner's, writes no turn configuration, starts no shell and reaches no other port.
#[test]
#[ignore = "runs the Pi prefix's node; set EXPERIMENT_PI_PREFIX"]
fn pis_node_does_only_what_the_node_profile_allows() {
    let prefix = prefix();
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = resolved(tmp.path()).expect("resolved");
    let (own, turn) = (root.join("own"), root.join("turn"));
    std::fs::create_dir_all(&own).expect("own");
    std::fs::create_dir_all(&turn).expect("turn");
    std::fs::write(turn.join("prompt.md"), "x").expect("a turn file");
    let proxy = TcpListener::bind("127.0.0.1:0").expect("bind");
    let other = TcpListener::bind("127.0.0.1:0").expect("bind");
    let (proxy_port, other_port) = (proxy.local_addr().expect("addr").port(), other.local_addr().expect("addr").port());
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let pkg = prefix.join("pi/package.json");
    let spec = json!({
        "read": [pkg, turn.join("prompt.md"), home.join(".zshrc")],
        "write": [own.join("probe"), turn.join("probe")],
        "exec": ["/bin/sh"],
        "tcp": [format!("127.0.0.1:{proxy_port}"), format!("127.0.0.1:{other_port}")],
    });
    let k = |kind: &str, t: &dyn std::fmt::Display| format!("{kind}:{t}");
    let refused = [
        k("read", &home.join(".zshrc").display()),
        k("write", &turn.join("probe").display()),
        k("exec", &"/bin/sh"),
        k("tcp", &format!("127.0.0.1:{other_port}")),
    ];

    // Without the sandbox, each refused attempt works (gotcha fd7742fa71ce).
    let open = probe(Command::new(prefix.join("bin/node")).env_clear().env("HOME", &own), &spec);
    for key in &refused {
        if key.starts_with("read:") && !home.join(".zshrc").exists() {
            continue;
        }
        assert_eq!(open[key], "allowed", "{key} unsandboxed: {open:#}");
    }

    let sandbox = Sandbox::agent(
        Profile::Node,
        &AgentPaths {
            program: &prefix.join("bin/node"),
            install: &prefix,
            own: &own,
            turn: &turn,
            shim: &bin(),
            socket: &root.join("agent.sock"),
            proxy_port,
        },
    )
    .expect("the profile");
    let mut cmd = sandbox.command(&prefix.join("bin/node"));
    cmd.env_clear().env("HOME", &own);
    let r = probe(&mut cmd, &spec);
    for key in [
        k("read", &pkg.display()),
        k("read", &turn.join("prompt.md").display()),
        k("write", &own.join("probe").display()),
        k("tcp", &format!("127.0.0.1:{proxy_port}")),
    ] {
        assert_eq!(r[&key], "allowed", "{key}: {r:#}");
    }
    for key in &refused {
        assert_eq!(r[key], "denied", "{key}: {r:#}");
    }
}

/// The stub provider, run by the prefix's node; killed when dropped.
struct Stub {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Stub {
    fn start(prefix: &Path, dir: &Path) -> Stub {
        let (cert, key, log) = (dir.join("server.pem"), dir.join("server.key"), dir.join("stub.jsonl"));
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pi-stub.mjs");
        let mut child = Command::new(prefix.join("bin/node"))
            .args([&script, &cert, &key, &log])
            .arg(FAKE_AUTH)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the stub");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout")).read_line(&mut line).expect("its port");
        Stub { child, port: line.trim().parse().expect("a port"), log }
    }

    fn requests(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        text.lines().map(|l| serde_json::from_str(l).expect("JSON")).collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A test CA and a certificate for [`HOST`] it signs, made with the system's openssl in `dir`: the CA's
/// certificate (PEM) is returned; `server.pem` and `server.key` are left for the stub. Nothing is kept.
fn certificates(dir: &Path) -> Vec<u8> {
    let openssl = |args: &[&str]| {
        let out = Command::new("/usr/bin/openssl").args(args).current_dir(dir).output().expect("openssl");
        assert!(out.status.success(), "openssl {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    std::fs::write(dir.join("ca.ext"), "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign\n")
        .expect("ca.ext");
    std::fs::write(
        dir.join("server.ext"),
        format!("subjectAltName=DNS:{HOST}\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n"),
    )
    .expect("server.ext");
    openssl(&["genrsa", "-out", "ca.key", "2048"]);
    openssl(&["req", "-new", "-key", "ca.key", "-subj", "/CN=bracken pi test CA", "-out", "ca.csr"]);
    openssl(&[
        "x509", "-req", "-in", "ca.csr", "-signkey", "ca.key", "-days", "1", "-extfile", "ca.ext", "-out", "ca.pem",
    ]);
    openssl(&["genrsa", "-out", "server.key", "2048"]);
    openssl(&["req", "-new", "-key", "server.key", "-subj", &format!("/CN={HOST}"), "-out", "server.csr"]);
    openssl(&[
        "x509",
        "-req",
        "-in",
        "server.csr",
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-days",
        "1",
        "-extfile",
        "server.ext",
        "-out",
        "server.pem",
    ]);
    std::fs::read(dir.join("ca.pem")).expect("ca.pem")
}

/// One whole Pi turn, sandboxed, through the forwarding proxy to the stub: the gate passes on the
/// session header, the stub sees the bearer token and exactly the shim's tools, the usage is the stub's
/// and recorded as the stream's sum, and the transcript holds no token.
#[test]
#[ignore = "runs the Pi prefix; set EXPERIMENT_PI_PREFIX"]
fn a_pi_turn_runs_through_the_proxy_to_a_stub_provider() {
    let prefix = prefix();
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = resolved(tmp.path()).expect("resolved");
    let certs = root.join("certs");
    std::fs::create_dir(&certs).expect("certs");
    let ca = certificates(&certs);
    let stub = Stub::start(&prefix, &certs);
    let routing = Routing::new(vec![
        ("ANTHROPIC_BASE_URL".into(), format!("https://{HOST}")),
        ("ANTHROPIC_AUTH_TOKEN".into(), FAKE_AUTH.into()),
    ])
    .expect("routing");
    let backend = PiBackend::with_test_ca(
        PiConfig {
            prefix: prefix.clone(),
            version: PI_VERSION.into(),
            expect: PiExpect { provider: "anthropic".into(), model: MODEL.into(), thinking: "medium".into() },
            caps: Caps { turn_time: Duration::from_secs(120), turn_budget_usd: None, ..Caps::pilot() },
            routing,
            dir: root.join("pi"),
            shim: bin(),
            prompt: "You are a test. Reply with one word.".into(),
            turns: 1,
            tools: tools(),
            upstream: Some(([127, 0, 0, 1], stub.port).into()),
        },
        ca,
    )
    .expect("the backend");
    let cfg = RunConfig {
        root: root.join("run"),
        mode: "bracken",
        agents: 1,
        turns: 1,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan: PlanInputs::default(),
        sandbox: true,
    };
    let ran = run(&cfg, &backend);
    let transcript = std::fs::read_to_string(root.join("pi/transcripts/agent-0-1.jsonl")).unwrap_or_default();
    let stderr = std::fs::read_to_string(root.join("pi/transcripts/agent-0-1.stderr")).unwrap_or_default();
    ran.unwrap_or_else(|e| panic!("the run: {e}\ntranscript:\n{transcript}\nstderr:\n{stderr}"));

    let usage: Vec<Value> = std::fs::read_to_string(root.join("pi/usage.jsonl"))
        .expect("usage.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();
    assert!(usage.iter().any(|r| r["gate"] == true), "the gate passed: {usage:#?}");
    let settled = usage.iter().find(|r| r["result"].is_string()).unwrap_or_else(|| {
        panic!(
            "no settled row\nusage: {usage:#?}\ntranscript:\n{transcript}\nstderr:\n{stderr}\nstub: {:#?}",
            stub.requests()
        )
    });
    assert_eq!(settled["result"], "the stream's sum");
    assert_eq!(
        (&settled["input"], &settled["cache_read"], &settled["cache_write"], &settled["output"]),
        (&json!(1200), &json!(4000), &json!(300), &json!(7)),
        "{settled}\ntranscript:\n{transcript}\nstderr:\n{stderr}\nstub: {:#?}",
        stub.requests()
    );
    assert!(!transcript.contains(FAKE_AUTH), "no token in the transcript");

    let requests: Vec<Value> = stub.requests().into_iter().filter(|r| r["path"].as_str().is_some()).collect();
    let messages: Vec<&Value> = requests
        .iter()
        .filter(|r| r["path"].as_str().is_some_and(|p| p.split('?').next() == Some("/v1/messages")))
        .collect();
    assert_eq!(messages.len(), 1, "one model call: {requests:#?}");
    assert_eq!(messages[0]["auth"], "the token", "the bearer token arrived through the proxy");
    assert_eq!(messages[0]["model"], MODEL);
    // Beside them, Pi sends one placeholder for Anthropic's prompt cache, with defer_loading: in its
    // source, 'never activated and the model cannot see it'. A call to it would stop the run.
    let declared: std::collections::BTreeSet<String> = messages[0]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t.as_str().expect("a name").to_string())
        .filter(|t| t != "__pi_deferred_placeholder__")
        .collect();
    let want: std::collections::BTreeSet<String> = tools().iter().map(|t| format!("mcp__experiment__{t}")).collect();
    assert_eq!(declared, want, "exactly the shim's tools, declared");
    assert!(prefix.join(CLI).exists());
}
