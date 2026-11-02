//! `bracken weave validate` and `weave integrate --run`: a line's named checks run through the
//! sandboxed check runner on a manifest, each result recorded as evidence on the manifest's artifact
//! (docs/design/weave.md, section 3, *The check runner*; build step 7). Every test uses throwaway
//! Grounds, made inside the temporary directory the checks' own fresh directories go in, so a
//! sibling Ground's keys are among what a check must not read. macOS only, but for the one test that
//! nothing runs elsewhere.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const AS: &str = "weave-integrator";
const VAL: &str = "val";

/// An owner decision's id: the binary checks only its shape.
const OD: &str = "0123456789abcdef";

struct Weave {
    tmp: TempDir,
    ground: PathBuf,
    repo: PathBuf,
}

impl Weave {
    /// A Weave Ground at `<tmp>/g` with a validator `val`, the vendor directory beside it, and a git
    /// repository at `<tmp>/repo` holding `files`. Checks make their fresh directories in `<tmp>`.
    fn new(files: &[(&str, &str)]) -> Weave {
        let tmp = tempfile::tempdir().expect("a temporary directory");
        let base = tmp.path().canonicalize().expect("canonical");
        let (ground, repo) = (base.join("g"), base.join("repo"));
        fs::create_dir(&repo).expect("mkdir repo");
        fs::create_dir(base.join("vendor")).expect("mkdir vendor");
        let w = Weave { tmp, ground, repo };
        w.git(&["init", "-q", "-b", "main"]);
        w.ok(&["weave", "init", "--as", AS]);
        w.ok(&["id", "new", VAL]);
        for (path, text) in files {
            let path = w.repo.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }
        w.git(&["add", "-A"]);
        w.git(&["commit", "-q", "--allow-empty", "-m", "c"]);
        w
    }

    fn base(&self) -> PathBuf {
        self.tmp.path().canonicalize().expect("canonical")
    }

    /// Sets the main line's checks, `NAME=ARGV` each, with `val` its only validator.
    fn checks(&self, checks: &[&str]) {
        let mut args = vec!["weave", "line", "policy", "main", "--clear", "checks", "--validator", VAL];
        for c in checks {
            args.extend(["--check", c]);
        }
        args.extend(["--owner-decision", OD, "--as", AS]);
        self.ok(&args);
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_bracken"));
        c.args(args)
            .current_dir(&self.repo)
            .env("BRACKEN_DIR", &self.ground)
            .env("BRACKEN_PROBE", "a BRACKEN_ variable a check must not see")
            .env("TMPDIR", self.base())
            .env_remove("BRACKEN_AGENT");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("bracken runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    }

    /// Runs a command that must fail, and returns its stderr.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
        String::from_utf8(out.stderr).expect("UTF-8")
    }

    /// The JSON on stdout, whatever the exit code: a failed check exits 1.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("bracken {args:?}: {e}: {}", String::from_utf8_lossy(&out.stderr)))
    }

    /// The manifest of the line's head, recorded by `integrate` of git's HEAD.
    fn integrate(&self) -> String {
        let out: Value = serde_json::from_str(&self.ok(&["weave", "integrate", "--as", AS, "--json"])).expect("JSON");
        out["manifest"].as_str().expect("a manifest").to_owned()
    }

    fn contributions(&self) -> usize {
        let log: Value = serde_json::from_str(&self.ok(&["log", "--json", "--limit", "100000"])).expect("JSON");
        log["contributions"].as_array().expect("a list of contributions").len()
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn show(&self, id: &str) -> Value {
        serde_json::from_str(&self.ok(&["show", id, "--json"])).expect("JSON")
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
}

// Finding 0f0118a08480: --timeout is bounded to 1 second to 7 days when parsed, on both commands, so
// one too long to count down is refused before anything runs or is written.
#[test]
fn a_timeout_out_of_bounds_is_refused_before_anything_is_written() {
    let w = Weave::new(&[("a.txt", "one\n")]);
    w.checks(&["ok=true"]);
    let hash = w.integrate();
    let before = w.contributions();
    for timeout in ["18446744073709551615", "604801", "0"] {
        for args in [
            &["weave", "validate", "--manifest", &hash, "--timeout", timeout, "--as", VAL][..],
            &["weave", "integrate", "--run", "--timeout", timeout, "--as", AS],
            &["weave", "integrate", "--run", "--manifest", &hash, "--timeout", timeout, "--as", AS],
        ] {
            let out = w.run(args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
            assert!(stderr.contains("--timeout"), "{args:?}: {stderr}");
            assert!(out.stdout.is_empty(), "{args:?} ran: {}", String::from_utf8_lossy(&out.stdout));
            assert_eq!(w.contributions(), before, "{args:?} wrote something");
        }
    }
}

// Elsewhere there is no sandbox, so neither command runs anything or writes anything.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_nothing_runs_and_nothing_is_written() {
    let w = Weave::new(&[("a.txt", "one\n")]);
    w.checks(&["ok=true"]);
    let hash = w.integrate();
    let before = w.contributions();
    for args in [
        &["weave", "validate", "--manifest", &hash, "--as", VAL][..],
        &["weave", "integrate", "--run", "--as", AS],
        &["weave", "integrate", "--run", "--manifest", &hash, "--as", AS],
    ] {
        let out = w.run(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} succeeded");
        assert!(stderr.contains("no sandbox"), "{args:?}: {stderr}");
        assert_eq!(w.contributions(), before, "{args:?} wrote something");
    }
}

#[cfg(target_os = "macos")]
mod macos_only {
    use std::path::Path;

    use super::*;

    /// The results a command's JSON lists.
    fn results(out: &Value) -> Vec<Value> {
        out["results"].as_array().expect("results").clone()
    }

    // validate runs every check (or those named), records each on the manifest's artifact with its
    // method and details, refuses a check with a latest result unless --again, and counts attempts on.
    #[test]
    fn validate_records_each_result_and_refuses_a_rerun_without_again() {
        let w = Weave::new(&[("a.txt", "one\n")]);
        w.checks(&["ok=true", "bad=false"]);
        let hash = w.integrate();
        let before = w.contributions();

        // Only a validator; and only checks the policy names.
        let stderr = w.fails(&["weave", "validate", "--manifest", &hash, "--as", AS]);
        assert!(stderr.contains("isn't one of line") && stderr.contains("validators"), "{stderr}");
        let stderr = w.fails(&["weave", "validate", "--manifest", &hash, "--check", "nope", "--as", VAL]);
        assert!(stderr.contains("no such check") && stderr.contains("bad, ok"), "{stderr}");
        assert_eq!(w.contributions(), before);

        // The policy version and each argv are printed before each runs.
        let out = w.run(&["weave", "validate", "--manifest", &hash, "--as", VAL]);
        assert_eq!(out.status.code(), Some(1), "a failed check exits 1");
        let text = String::from_utf8(out.stdout).expect("UTF-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[0].starts_with("policy "), "{text}");
        assert_eq!(lines[1], r#"run bad attempt 1: ["false"]"#);
        assert!(lines[2].starts_with("evidence ") && lines[2].ends_with("(fail: bad, exit 1)"), "{text}");
        assert_eq!(lines[3], r#"run ok attempt 1: ["true"]"#);
        assert!(lines[4].starts_with("evidence ") && lines[4].ends_with("(pass: ok, exit 0)"), "{text}");
        assert_eq!(w.contributions(), before + 2);

        // Each result is evidence on the manifest's artifact: the method names the check and its
        // definition, the details give the hash, the attempt, the argv and the exit, then the tail.
        let pass = w.show(lines[4].split(' ').nth(1).expect("an id"));
        assert_eq!(pass["author_name"], VAL);
        assert_eq!(pass["body"]["verdict"], "pass");
        assert!(pass["body"]["method"].as_str().expect("a method").starts_with("ok blake3:"));
        let details = pass["body"]["details"].as_str().expect("details");
        assert_eq!(details, format!("{hash}\nattempt 1\nargv [\"true\"]\nexit 0\n\n"));
        let manifest = w.show(pass["body"]["target"].as_str().expect("a target"));
        assert_eq!(manifest["body"]["media_type"], "application/vnd.bracken.weave.manifest+cbor", "{manifest}");

        // Again: refused, naming the checks and their latest results, unless --again.
        let before = w.contributions();
        let stderr = w.fails(&["weave", "validate", "--manifest", &hash, "--as", VAL]);
        assert!(stderr.contains("already have a latest result") && stderr.contains("--again"), "{stderr}");
        let stderr = w.fails(&["weave", "validate", "--manifest", &hash, "--check", "ok", "--as", VAL]);
        assert!(stderr.contains("for ok ("), "{stderr}");
        assert_eq!(w.contributions(), before, "a refused validate wrote");
        let out =
            w.json(&["weave", "validate", "--manifest", &hash, "--check", "ok", "--again", "--as", VAL, "--json"]);
        let rows = results(&out);
        assert_eq!(rows.len(), 1, "{out}");
        assert_eq!((rows[0]["check"].as_str(), rows[0]["attempt"].as_u64()), (Some("ok"), Some(2)));
        let again = w.show(rows[0]["evidence"].as_str().expect("an id"));
        assert!(again["body"]["details"].as_str().expect("details").starts_with(&format!("{hash}\nattempt 2\n")));
        let out = w.json(&["weave", "validate", "--manifest", &hash, "--again", "--as", VAL, "--json"]);
        let attempts: Vec<(String, u64)> = results(&out)
            .iter()
            .map(|r| (r["check"].as_str().expect("a name").to_owned(), r["attempt"].as_u64().expect("a number")))
            .collect();
        assert_eq!(attempts, [("bad".to_owned(), 2), ("ok".to_owned(), 3)]);

        // --dry-run shows the plan, its signer and validators, and runs and writes nothing.
        let before = w.contributions();
        let plan = w.json(&["weave", "validate", "--manifest", &hash, "--again", "--dry-run", "--as", VAL, "--json"]);
        assert_eq!(plan["checks"].as_array().map(Vec::len), Some(2), "{plan}");
        assert_eq!(plan["validators"].as_array().map(Vec::len), Some(1), "{plan}");
        assert_eq!(plan["validators"][0], plan["signer"]);
        assert_eq!(w.contributions(), before);
    }

    // integrate --run, on a given tree and on proposals, runs the line's checks on the candidate,
    // records the integrator's results on its manifest, then writes the decision.
    #[test]
    fn integrate_run_records_the_integrators_results_then_the_decision() {
        let w = Weave::new(&[("a.txt", "one\n")]);
        w.checks(&["ok=true", "bad=false"]);
        let out = w.json(&["weave", "integrate", "--run", "--as", AS, "--json"]);
        assert!(out["decision"].is_string(), "{out}");
        let rows = results(&out);
        assert_eq!(rows.len(), 2, "{out}");
        for r in &rows {
            let e = w.show(r["evidence"].as_str().expect("an id"));
            assert_eq!(e["author_name"], AS);
            assert_eq!(e["body"]["target"], out["manifest_id"]);
            assert_eq!(e["body"]["verdict"], if r["check"] == "ok" { "pass" } else { "fail" });
        }
        // A second run on the same manifest counts the integrator's attempts on.
        let hash = out["manifest"].as_str().expect("a hash").to_owned();
        let out = w.json(&["weave", "integrate", "--run", "--manifest", &hash, "--as", AS, "--json"]);
        assert!(results(&out).iter().all(|r| r["attempt"] == 2), "{out}");

        // Without --run, nothing runs; --timeout needs --run.
        let out = w.json(&["weave", "integrate", "--manifest", &hash, "--as", AS, "--json"]);
        assert_eq!(results(&out).len(), 0, "{out}");
        let stderr = w.fails(&["weave", "integrate", "--timeout", "5", "--as", AS]);
        assert!(stderr.contains("--run"), "{stderr}");

        // On proposals.
        fs::write(w.repo.join("b.txt"), "two\n").expect("write");
        w.git(&["add", "-A"]);
        w.git(&["commit", "-q", "-m", "b"]);
        let proposal: Value = serde_json::from_str(&w.ok(&["weave", "propose", "--as", VAL, "--json"])).expect("JSON");
        let id = proposal["proposal"].as_str().expect("a proposal").to_owned();
        let out = w.json(&["weave", "integrate", "--proposals", &id, "--run", "--as", AS, "--json"]);
        assert!(out["decision"].is_string(), "{out}");
        assert_eq!(results(&out).len(), 2, "{out}");
    }

    /// A probe for the check: each forbidden access, tried against something that exists, printing
    /// REFUSED or ALLOWED, and exiting 1 if any was allowed.
    fn probe(w: &Weave, port: u16, socket: &Path) -> String {
        let home = std::env::var("HOME").expect("HOME");
        let base = w.base();
        let unique = std::process::id();
        let tries = [
            ("read a Ground identities/ file", format!("cat {}/identities/*", w.ground.display())),
            ("list a Ground identities/", format!("ls {}/identities", w.ground.display())),
            ("read another TMPDIR entry", format!("cat {}/secret.txt", base.display())),
            ("list TMPDIR", format!("ls {}", base.display())),
            ("read ~/.ssh", format!("ls {home}/.ssh")),
            ("write ~/.cargo", format!("mkdir {home}/.cargo/bracken-probe-{unique}")),
            ("write ~/.rustup", format!("mkdir {home}/.rustup/bracken-probe-{unique}")),
            ("loopback TCP", format!("echo hi > /dev/tcp/127.0.0.1/{port}")),
            ("a unix socket", format!("echo hi | /usr/bin/nc -w 1 -U {}", socket.display())),
            ("IPC to launchd", "/bin/launchctl list".to_owned()),
            ("IPC to the pasteboard", "/usr/bin/pbpaste".to_owned()),
        ];
        let mut script = String::from("bad=0\n");
        script.push_str("if env | grep -q '^BRACKEN_'; then echo 'ALLOWED BRACKEN_*'; bad=1; fi\n");
        script.push_str("echo \"PATH=$PATH\"\n");
        for (what, command) in tries {
            script.push_str(&format!(
                "if ( {command} ) >/dev/null 2>&1; then echo 'ALLOWED {what}'; bad=1; else echo 'REFUSED {what}'; fi\n"
            ));
        }
        script.push_str("exit $bad\n");
        script
    }

    /// The rustup home, `~/.rustup`, resolved.
    fn rustup_home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").expect("HOME")).join(".rustup").canonicalize().expect("a rustup home")
    }

    /// The check's PATH: the bin of rustup's default toolchain, as its settings name it, then the system's.
    fn expected_path() -> String {
        let home = rustup_home();
        let settings = fs::read_to_string(home.join("settings.toml")).expect("rustup's settings");
        let name = settings
            .lines()
            .find_map(|l| l.strip_prefix("default_toolchain = \""))
            .and_then(|v| v.strip_suffix('"'))
            .expect("a default toolchain");
        format!("{}/bin:/usr/bin:/bin:/usr/sbin:/sbin", home.join("toolchains").join(name).display())
    }

    // Finding 899bfab10dec: the toolchain is rustup's default, found without running anything. A
    // rust-toolchain.toml with a path toolchain in the caller's directory (the candidate's checkout), and
    // PATH, RUSTUP_HOME and RUSTUP_TOOLCHAIN in the caller's environment, run nothing and change neither
    // the check's PATH nor its RUSTUP_HOME.
    #[test]
    fn the_callers_directory_and_environment_cant_choose_the_toolchain() {
        let w = Weave::new(&[("env.sh", "echo \"PATH=$PATH\"\necho \"RUSTUP_HOME=$RUSTUP_HOME\"\n")]);
        let evil = w.base().join("evil");
        let marker = w.base().join("evil-ran");
        fs::create_dir_all(evil.join("bin")).expect("mkdir");
        for program in ["rustc", "cargo", "rustup"] {
            let path = evil.join("bin").join(program);
            fs::write(&path, format!("#!/bin/sh\ntouch {}\necho {}\n", marker.display(), evil.display()))
                .expect("write");
            let mut perms = fs::metadata(&path).expect("stat").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
        fs::write(w.repo.join("rust-toolchain.toml"), format!("[toolchain]\npath = \"{}\"\n", evil.display()))
            .expect("write");
        w.git(&["add", "-A"]);
        w.git(&["commit", "-q", "-m", "toolchain"]);
        w.checks(&["env=bash env.sh"]);
        let path = format!("{}:{}", evil.join("bin").display(), std::env::var("PATH").expect("PATH"));
        let steered = |args: &[&str]| -> Value {
            let out = w
                .command(args)
                .env("PATH", &path)
                .env("RUSTUP_HOME", &evil)
                .env("RUSTUP_TOOLCHAIN", &evil)
                .output()
                .expect("bracken runs");
            serde_json::from_slice(&out.stdout)
                .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stderr)))
        };
        let integrated = steered(&["weave", "integrate", "--run", "--as", AS, "--json"]);
        let hash = integrated["manifest"].as_str().expect("a hash").to_owned();
        let validated = steered(&["weave", "validate", "--manifest", &hash, "--as", VAL, "--json"]);
        for (what, out) in [("integrate --run", &integrated), ("validate", &validated)] {
            let rows = results(out);
            assert_eq!(rows.len(), 1, "{what}: {out}");
            let e = w.show(rows[0]["evidence"].as_str().expect("an id"));
            let details = e["body"]["details"].as_str().expect("details");
            assert!(details.contains(&format!("PATH={}\n", expected_path())), "{what}: {details}");
            assert!(details.contains(&format!("RUSTUP_HOME={}\n", rustup_home().display())), "{what}: {details}");
        }
        assert!(!marker.exists(), "a program the caller's directory or environment named ran");
    }

    // On both commands, a check sees no BRACKEN_* and a fixed PATH; can't read an identities/
    // directory, ~/.ssh or $TMPDIR's other entries; can't write ~/.cargo or ~/.rustup; and has no
    // network, loopback or IPC.
    #[test]
    fn a_check_is_sandboxed_on_validate_and_on_integrate_run() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let sockets = tempfile::tempdir().expect("a directory");
        let socket = sockets.path().join("s.sock");
        let unix = std::os::unix::net::UnixListener::bind(&socket).expect("bind unix");
        unix.set_nonblocking(true).expect("nonblocking");

        let w = Weave::new(&[]);
        fs::write(w.base().join("secret.txt"), "a sibling's secret\n").expect("write");
        fs::write(w.repo.join("probe.sh"), probe(&w, port, &socket)).expect("write");
        w.git(&["add", "-A"]);
        w.git(&["commit", "-q", "-m", "probe"]);
        w.checks(&["probe=bash probe.sh"]);

        let integrated = w.json(&["weave", "integrate", "--run", "--as", AS, "--json"]);
        let hash = integrated["manifest"].as_str().expect("a hash").to_owned();
        let validated = w.json(&["weave", "validate", "--manifest", &hash, "--as", VAL, "--json"]);
        for (what, out) in [("integrate --run", &integrated), ("validate", &validated)] {
            let rows = results(out);
            assert_eq!(rows.len(), 1, "{what}: {out}");
            let e = w.show(rows[0]["evidence"].as_str().expect("an id"));
            let details = e["body"]["details"].as_str().expect("details");
            assert!(!details.contains("ALLOWED"), "{what}: {details}");
            assert_eq!(details.matches("REFUSED").count(), 11, "{what}: {details}");
            assert!(details.contains(&format!("PATH={}\n", expected_path())), "{what}: {details}");
            assert_eq!(e["body"]["verdict"], "pass", "{what}: {details}");
        }
        assert!(listener.accept().is_err(), "the loopback listener was reached");
        assert!(unix.accept().is_err(), "the unix socket was reached");
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        for dir in [".cargo", ".rustup"] {
            let probe = home.join(dir).join(format!("bracken-probe-{}", std::process::id()));
            assert!(!probe.exists(), "{} was written", probe.display());
        }
    }

    /// Whether a process holds `pid`: `kill -0` sends no signal, only checks.
    fn alive(pid: &str) -> bool {
        Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("kill")
            .success()
    }

    // On both commands, a check is killed with its process group at --timeout, and fails.
    #[test]
    fn a_check_is_killed_with_its_group_at_the_timeout() {
        let w = Weave::new(&[("slow.sh", "sleep 30 & echo $!\nsleep 30\n")]);
        w.checks(&["slow=bash slow.sh"]);
        let started = std::time::Instant::now();
        let integrated = w.json(&["weave", "integrate", "--run", "--timeout", "2", "--as", AS, "--json"]);
        let hash = integrated["manifest"].as_str().expect("a hash").to_owned();
        let validated = w.json(&["weave", "validate", "--manifest", &hash, "--timeout", "2", "--as", VAL, "--json"]);
        assert!(started.elapsed() < std::time::Duration::from_secs(25), "took {:?}", started.elapsed());
        for (what, out) in [("integrate --run", &integrated), ("validate", &validated)] {
            let rows = results(out);
            assert_eq!((rows.len(), rows[0]["exit"].as_str()), (1, Some("timed out")), "{what}: {out}");
            let e = w.show(rows[0]["evidence"].as_str().expect("an id"));
            assert_eq!(e["body"]["verdict"], "fail");
            let details = e["body"]["details"].as_str().expect("details");
            let pid = details.rsplit("\n\n").next().expect("a tail").trim();
            assert!(!pid.is_empty() && !alive(pid), "{what}: the background child {pid} outlived the timeout");
        }
    }

    // A tree that needs a crate the vendor directory doesn't hold fails its check, naming the crate.
    #[test]
    fn a_missing_vendored_crate_fails_the_check() {
        let w = Weave::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nunvendored-crate-xyz = \"1\"\n",
            ),
            ("src/lib.rs", "\n"),
        ]);
        w.checks(&["build=cargo build"]);
        let hash = w.integrate();
        let out = w.json(&["weave", "validate", "--manifest", &hash, "--as", VAL, "--json"]);
        let rows = results(&out);
        assert_eq!(rows[0]["passed"], false, "{out}");
        let e = w.show(rows[0]["evidence"].as_str().expect("an id"));
        let details = e["body"]["details"].as_str().expect("details");
        assert!(details.contains("unvendored-crate-xyz"), "{details}");
    }

    // An output tail holding a key is withheld: the result records only its size and hash.
    #[test]
    fn a_tail_holding_a_key_is_withheld() {
        // The key is put together when the check runs, so the tree itself holds none (gotcha 5a3dda0b1f93).
        let w = Weave::new(&[("key.sh", "printf '%s%s\\nMIIE\\n' '-----BEGIN RSA PRIV' 'ATE KEY-----'\n")]);
        w.checks(&["key=bash key.sh"]);
        let integrated = w.json(&["weave", "integrate", "--run", "--as", AS, "--json"]);
        let hash = integrated["manifest"].as_str().expect("a hash").to_owned();
        let validated = w.json(&["weave", "validate", "--manifest", &hash, "--as", VAL, "--json"]);
        let tail = format!("-----BEGIN RSA {}-----\nMIIE\n", "PRIVATE KEY");
        for (what, out) in [("integrate --run", &integrated), ("validate", &validated)] {
            let rows = results(out);
            let withheld = &rows[0]["withheld"];
            assert_eq!(withheld["size"], tail.len(), "{what}: {out}");
            assert_eq!(withheld["hash"], bracken_tree::ContentHash::of(tail.as_bytes()).to_string(), "{what}: {out}");
            let e = w.show(rows[0]["evidence"].as_str().expect("an id"));
            let details = e["body"]["details"].as_str().expect("details");
            assert!(!details.contains("BEGIN"), "{what}: {details}");
            let hash = withheld["hash"].as_str().expect("a hash");
            assert!(
                details.ends_with(&format!(
                    "output withheld: {} bytes, {hash}, as it matched {}",
                    tail.len(),
                    withheld["rule"].as_str().expect("a rule")
                )),
                "{what}: {details}"
            );
        }
    }

    /// Finding afb0e1a0267f: each of toolchain()'s three refusals, through a crafted HOME, stops validate
    /// before anything is written: a rustup home that is HOME or holds it, a default toolchain that isn't
    /// a plain name, and a toolchain resolving outside the home's toolchains. Deleting any refusal fails
    /// its case.
    #[test]
    fn a_crafted_rustup_home_is_refused_before_anything_is_written() {
        use std::os::unix::fs::symlink;
        let w = Weave::new(&[("ok.sh", "true\n")]);
        w.checks(&["ok=bash ok.sh"]);
        let hash = w.integrate();
        // A fresh HOME per case, beside the Ground, crafted by `craft`.
        let case = |label: &str, craft: &dyn Fn(&Path), refusal: &str| {
            let home = w.base().join(format!("home-{label}"));
            fs::create_dir_all(&home).expect("mkdir home");
            craft(&home);
            let before = w.contributions();
            let out = w
                .command(&["weave", "validate", "--manifest", &hash, "--as", VAL, "--json"])
                .env("HOME", &home)
                .output()
                .expect("bracken runs");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{label}: validate ran: {stderr}");
            assert!(stderr.contains(refusal), "{label}: {stderr}");
            assert!(stderr.contains("nothing was written"), "{label}: {stderr}");
            assert_eq!(w.contributions(), before, "{label}: something was written");
        };
        let settings = |rustup: &Path, name: &str| {
            fs::write(rustup.join("settings.toml"), format!("default_toolchain = \"{name}\"\n")).expect("write");
        };
        // The rustup home is HOME, or holds it.
        case("is-home", &|home| symlink(home, home.join(".rustup")).expect("symlink"), "or holds it");
        case(
            "holds-home",
            &|home| symlink(home.parent().expect("a parent"), home.join(".rustup")).expect("symlink"),
            "or holds it",
        );
        // The default toolchain isn't a plain name.
        for (label, name) in [("dotdot", ".."), ("dotdot-x", "../x"), ("slash", "a/b")] {
            case(
                label,
                &|home| {
                    let rustup = home.join(".rustup");
                    fs::create_dir_all(rustup.join("toolchains")).expect("mkdir");
                    settings(&rustup, name);
                },
                "isn't a toolchain name",
            );
        }
        // The toolchain resolves outside the home's toolchains: a toolchain linked to HOME, and the
        // toolchains directory itself linked elsewhere in HOME.
        case(
            "toolchain-is-home",
            &|home| {
                let rustup = home.join(".rustup");
                fs::create_dir_all(rustup.join("toolchains")).expect("mkdir");
                settings(&rustup, "x");
                symlink(home, rustup.join("toolchains").join("x")).expect("symlink");
            },
            "outside the rustup home's toolchains",
        );
        case(
            "toolchains-elsewhere",
            &|home| {
                let rustup = home.join(".rustup");
                fs::create_dir_all(home.join("other").join("x").join("bin")).expect("mkdir");
                fs::create_dir_all(&rustup).expect("mkdir");
                settings(&rustup, "x");
                symlink(home.join("other"), rustup.join("toolchains")).expect("symlink");
            },
            "outside the rustup home's toolchains",
        );
    }

    // Plan cc496dcee0e3, section 2: a box check runs the pinned runner script on a bundle of the manifest's
    // commit, and its result names the commit, tree, bundle, box and output; a mac check runs in the
    // commit's checkout with the clean environment, given the box's output. A run that names another commit
    // is refused with nothing recorded, and so is a runner script that isn't the one the policy pins.
    #[test]
    fn box_and_mac_checks_run_the_pinned_script_on_the_manifests_commit() {
        // b.txt is tracked though the tree's .gitignore names it: the manifest's commit holds it still.
        let w = Weave::new(&[("a.txt", "one\n"), (".gitignore", "b.txt\n")]);
        fs::write(w.repo.join("b.txt"), "two\n").expect("write b.txt");
        w.git(&["add", "-f", "b.txt"]);
        w.git(&["commit", "-q", "-m", "b"]);
        let runner = w.outside_checks();
        let hash = w.integrate();
        let dir = runner.to_str().expect("UTF-8");
        let validate = |checks: &[&str]| -> Value {
            let mut args = vec!["weave", "validate", "--manifest", &hash, "--runner", dir, "--as", VAL, "--json"];
            for c in checks {
                args.extend(["--check", c]);
            }
            let out = w.run(&args);
            let mut json: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
            json["stderr"] = String::from_utf8_lossy(&out.stderr).into_owned().into();
            json
        };

        // Without --runner, nothing runs.
        let before = w.contributions();
        let err = w.fails(&["weave", "validate", "--manifest", &hash, "--check", "table", "--as", VAL]);
        assert!(err.contains("needs --runner"), "{err}");
        assert_eq!(w.contributions(), before);

        // The mac check is given first but runs after the box's, with its output.
        let out = validate(&["mac", "table"]);
        let results = out["results"].as_array().expect("results");
        assert_eq!(results.len(), 2, "{out}");
        let (table, mac) = (&results[0], &results[1]);
        assert_eq!((table["check"].as_str(), mac["check"].as_str()), (Some("table"), Some("mac")), "{out}");
        let details_of = |r: &Value| w.show(r["evidence"].as_str().unwrap())["body"]["details"].clone();
        assert!(table["passed"].as_bool().unwrap(), "{}", details_of(table));
        assert!(mac["passed"].as_bool().unwrap(), "{}", details_of(mac));
        assert_eq!(table["box"], "bx_test1");
        assert_eq!(table["commit"], mac["commit"], "both ran on the manifest's one commit");
        let evidence = w.show(table["evidence"].as_str().unwrap());
        let details = evidence["body"]["details"].as_str().expect("details");
        let commit = table["commit"].as_str().unwrap();
        for line in [
            format!("commit {commit}"),
            format!("tree {}", table["tree"].as_str().unwrap()),
            format!("bundle sha256:{}", table["bundle_sha256"].as_str().unwrap()),
            "box bx_test1".to_owned(),
            "cost exit 0 · $0.01".to_owned(),
        ] {
            assert!(details.lines().any(|l| l == line), "{line:?} in {details}");
        }
        assert!(details.starts_with(&format!("{hash}\nattempt 1\n")), "{details}");
        // The output is recorded whole, and hashes as the details say.
        let log = w.show(table["log"].as_str().expect("a log artifact"));
        assert_eq!(log["body"]["media_type"], "text/plain");
        let log_line = details.lines().find(|l| l.starts_with("log ")).expect("a log line");
        assert!(log_line.ends_with(table["log_hash"].as_str().unwrap()), "{log_line}");

        // A run that names another commit can't stand for the manifest: nothing is recorded.
        let before = w.contributions();
        let err = w.fails(&["weave", "validate", "--manifest", &hash, "--check", "lie", "--runner", dir, "--as", VAL]);
        assert!(err.contains("can't be tied to the manifest"), "{err}");
        assert_eq!(w.contributions(), before);

        // A failing run is recorded as a fail, with its proof.
        let out = validate(&["fail"]);
        assert_eq!(out["results"][0]["passed"], false, "{out}");
        // A box run that exits 0 without its end line fails (task 58fb9682135e).
        let out = validate(&["noend"]);
        assert_eq!(out["results"][0]["passed"], false, "{out}");
        assert!(details_of(&out["results"][0]).as_str().unwrap().contains("\nend missing\n"), "{out}");
        // Output over what an artifact may hold: the result is recorded, its log withheld as its size
        // (finding 2e9d57852ec7).
        let out = validate(&["big"]);
        let big = &out["results"][0];
        assert_eq!(big["passed"], true, "{out}");
        assert!(big["log"].is_null(), "{out}");
        let details = details_of(big);
        let details = details.as_str().unwrap();
        // Its size and the whole output's hash, though only the last bytes were kept (finding 0fb11da198a5).
        let mut output = vec![b'y'; 17_000_000];
        output.extend_from_slice(b"\nthe big box's end\n");
        let whole = bracken_tree::ContentHash::of(&output);
        assert!(
            details.contains(&format!("log withheld: {} bytes, {whole}, over the", output.len())),
            "{details:.400}"
        );
        assert_eq!(big["log_hash"], whole.to_string());
        assert!(details.ends_with("the big box's end\n"), "the tail is still recorded");

        // A mac check on a manifest with no passing box result here doesn't run (finding ee89e79fbcc1).
        w.git(&["commit", "-q", "--allow-empty", "-m", "another"]);
        fs::write(w.repo.join("c.txt"), "three\n").expect("write");
        w.git(&["add", "c.txt"]);
        w.git(&["commit", "-q", "-m", "c"]);
        let other = w.integrate();
        let before = w.contributions();
        let err = w.fails(&["weave", "validate", "--manifest", &other, "--check", "mac", "--runner", dir, "--as", VAL]);
        assert!(err.contains("no box lists to compare with"), "{err}");
        assert_eq!(w.contributions(), before);

        // The script changed since the policy pinned it: nothing runs.
        fs::write(runner.join("run.sh"), format!("{RUNNER}\n# changed\n")).expect("write");
        let before = w.contributions();
        let err = w.fails(&[
            "weave",
            "validate",
            "--manifest",
            &hash,
            "--check",
            "table",
            "--again",
            "--runner",
            dir,
            "--as",
            VAL,
        ]);
        assert!(err.contains("the policy pins"), "{err}");
        assert_eq!(w.contributions(), before);

        // integrate --run leaves box and mac checks to validate.
        let out: Value =
            serde_json::from_str(&w.ok(&["weave", "integrate", "--run", "--as", AS, "--json"])).expect("JSON");
        assert_eq!(out["results"].as_array().map(Vec::len).unwrap_or(0), 0, "{out}");
    }
}

/// A runner script standing in for scripts/test-table's --weave-box and --weave-mac (plan cc496dcee0e3):
/// `box` checks the bundle as the box does and writes what a box would print, `mac` checks it got the
/// box's log and runs in the manifest's checkout, `lie` names another commit, and `fail` fails.
#[cfg(target_os = "macos")]
const RUNNER: &str = r#"#!/bin/bash
set -eu
case "$1" in
  box)
    [ "$(shasum -a 256 "$WEAVE_BUNDLE" | cut -d' ' -f1)" = "$WEAVE_BUNDLE_SHA256" ] || exit 9
    d=$(mktemp -d)
    git -C "$d" init -q
    git -C "$d" fetch -q "$WEAVE_BUNDLE" weave:tested
    printf 'bundle %s\nhead %s\ntree %s\nbox bx_test1\ncost exit 0 · $0.01\nend seen\n' "$WEAVE_BUNDLE_SHA256" \
      "$(git -C "$d" rev-parse tested)" "$(git -C "$d" rev-parse 'tested^{tree}')" >>"$WEAVE_FACTS"
    echo "the box listed its tests" ;;
  noend)
    printf 'bundle %s\nhead %s\ntree %s\nbox bx_test3\n' "$WEAVE_BUNDLE_SHA256" "$WEAVE_COMMIT" "$WEAVE_TREE" >>"$WEAVE_FACTS" ;;
  big)
    printf 'bundle %s\nhead %s\ntree %s\nbox bx_test4\nend seen\n' "$WEAVE_BUNDLE_SHA256" "$WEAVE_COMMIT" "$WEAVE_TREE" >>"$WEAVE_FACTS"
    head -c 17000000 /dev/zero | tr '\0' 'y'
    echo
    echo "the big box's end" ;;
  mac)
    printf 'head %s\ntree %s\n' "$(git rev-parse HEAD)" "$(git rev-parse 'HEAD^{tree}')" >>"$WEAVE_FACTS"
    grep -q 'the box listed its tests' "$WEAVE_INPUTS/table.log" || { echo "no box log"; exit 3; }
    [ -z "${BRACKEN_PROBE:-}" ] || { echo "saw BRACKEN_PROBE"; exit 5; }
    [ "$(cat a.txt b.txt)" = "one
two" ] || { echo "not the manifest's files"; exit 6; }
    echo "mac ok" ;;
  lie)
    printf 'head %s\ntree %s\n' 0123456789012345678901234567890123456789 "$WEAVE_TREE" >>"$WEAVE_FACTS" ;;
  fail)
    printf 'bundle %s\nhead %s\ntree %s\nbox bx_test2\n' "$WEAVE_BUNDLE_SHA256" "$WEAVE_COMMIT" "$WEAVE_TREE" >>"$WEAVE_FACTS"
    echo "a test failed"
    exit 1 ;;
esac
"#;

#[cfg(target_os = "macos")]
impl Weave {
    /// The runner directory, holding [`RUNNER`] as run.sh, and line main's checks: `table` in a box,
    /// `mac` on this Mac, `lie` and `fail` in a box, all through run.sh.
    fn outside_checks(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let runner = self.base().join("runner");
        fs::create_dir(&runner).expect("mkdir runner");
        fs::write(runner.join("run.sh"), RUNNER).expect("write run.sh");
        fs::set_permissions(runner.join("run.sh"), fs::Permissions::from_mode(0o755)).expect("chmod run.sh");
        let mut args: Vec<String> =
            ["weave", "line", "policy", "main", "--clear", "checks", "--validator", VAL].map(String::from).to_vec();
        for (name, argv, kind) in [
            ("table", "box", "box"),
            ("mac", "mac", "mac"),
            ("lie", "lie", "box"),
            ("fail", "fail", "box"),
            ("noend", "noend", "box"),
            ("big", "big", "box"),
        ] {
            args.extend([
                "--check".to_owned(),
                format!("{name}={argv}"),
                "--executor".to_owned(),
                format!("{name}={kind}:run.sh"),
            ]);
        }
        args.extend(
            ["--runner", runner.to_str().expect("UTF-8"), "--owner-decision", OD, "--as", AS].map(String::from),
        );
        self.ok(&args.iter().map(String::as_str).collect::<Vec<_>>());
        runner
    }
}
