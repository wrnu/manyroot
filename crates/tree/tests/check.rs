//! The check runner (docs/design/weave.md, section 3, *The check runner*; build step 7, first part).
//! Every forbidden access is tried from inside a check, against files that exist, and each is
//! refused. macOS only: elsewhere the runner refuses to run at all, which one test checks.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::time::Instant;

#[cfg(target_os = "macos")]
use bracken_tree::check::Exit;
use bracken_tree::check::{CheckError, RecordedTail, Runner, recorded_tail};
use bracken_tree::secrets::KnownKeys;
use bracken_tree::{CheckDefinition, ContentHash, Entry, Manifest, Mode};

/// A tree: its manifest and its files by content hash.
fn tree(files: &[(&str, Mode, &[u8])]) -> (Manifest, BTreeMap<ContentHash, Vec<u8>>) {
    let entries = files
        .iter()
        .map(|(path, mode, bytes)| Entry {
            path: (*path).to_owned(),
            mode: *mode,
            hash: ContentHash::of(bytes),
            size: bytes.len() as u64,
        })
        .collect();
    let blobs = files.iter().map(|(_, _, bytes)| (ContentHash::of(bytes), bytes.to_vec())).collect();
    (Manifest::new(entries), blobs)
}

/// A throwaway base directory (standing in for `$TMPDIR`), a vendor directory inside it, and a
/// runner making its fresh directories there.
struct Setup {
    base: tempfile::TempDir,
    runner: Runner,
}

impl Setup {
    fn new() -> Setup {
        let base = tempfile::tempdir().expect("a base directory");
        let vendor = base.path().join("vendor");
        std::fs::create_dir(&vendor).expect("mkdir vendor");
        let mut runner = Runner::new(&vendor);
        runner.base = base.path().to_path_buf();
        runner.timeout = Duration::from_secs(20);
        Setup { base, runner }
    }

    fn path(&self) -> PathBuf {
        self.base.path().canonicalize().expect("canonical")
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn run(&self, argv: &[&str], files: &[(&str, Mode, &[u8])]) -> Result<bracken_tree::check::Outcome, CheckError> {
        let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        let (manifest, blobs) = tree(files);
        self.runner.run(&CheckDefinition { name: "test", argv: &argv }, &manifest, |h| blobs.get(h).cloned())
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    /// Runs a shell script as the check, and returns its output.
    fn sh(&self, script: &str) -> String {
        let out = self.run(&["bash", "-c", script], &[("README", Mode::File, b"r\n")]).expect("the check runs");
        String::from_utf8_lossy(&out.tail).into_owned()
    }

    /// `runner` refuses, with NoSandbox, a check that would leave a marker, and leaves nothing behind.
    fn assert_refused(&self, runner: &Runner) {
        let marker = self.path().join("ran");
        let script = format!("touch {}", marker.display());
        let argv: Vec<String> = ["sh", "-c", &script].iter().map(|a| (*a).to_owned()).collect();
        let (manifest, blobs) = tree(&[("README", Mode::File, b"r\n")]);
        let result = runner.run(&CheckDefinition { name: "test", argv: &argv }, &manifest, |h| blobs.get(h).cloned());
        assert!(matches!(result, Err(CheckError::NoSandbox(_))), "{result:?}");
        assert!(!marker.exists(), "the check ran");
        self.assert_cleaned_up();
    }

    /// The base holds no fresh directory: each run removed its own.
    fn assert_cleaned_up(&self) {
        let left: Vec<_> = std::fs::read_dir(self.base.path())
            .expect("read base")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("weave-check-"))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }
}

#[cfg(target_os = "macos")]
mod macos_only {
    use std::path::Path;

    use super::*;

    /// The Command Line Tools' python3: /usr/bin/python3 is a shim the sandbox can't run.
    const PYTHON: &str = "/Library/Developer/CommandLineTools/usr/bin/python3";

    /// The longest a run with an orphaned escapee may take: far past a normal run on a loaded machine, so
    /// only a run that waited on the escapee (a reader that didn't stop) reaches it (finding 166c97812d6e).
    const ESCAPEE_RUN_LIMIT: Duration = Duration::from_secs(120);

    #[test]
    fn a_passing_and_a_failing_check() {
        let s = Setup::new();
        let files: &[(&str, Mode, &[u8])] = &[("src/a.txt", Mode::File, b"a\n")];
        let pass = s.run(&["test", "-f", "src/a.txt"], files).expect("runs");
        assert_eq!(pass.exit, Exit::Code(0));
        assert!(pass.exit.passed());
        assert_eq!(pass.method, CheckDefinition { name: "test", argv: &pass.argv }.method());
        let fail = s.run(&["sh", "-c", "echo failing; exit 3"], files).expect("runs");
        assert_eq!(fail.exit, Exit::Code(3));
        assert_eq!(fail.tail, b"failing\n");
        s.assert_cleaned_up();
    }

    // argv[0] is resolved against the fixed PATH, never the tree, even with an executable of that
    // name at the tree's root; a name with a '/' is taken relative to the tree.
    #[test]
    fn argv0_is_resolved_against_the_fixed_path() {
        let s = Setup::new();
        let files: &[(&str, Mode, &[u8])] = &[
            ("true", Mode::Executable, b"#!/bin/sh\necho the tree's own\nexit 7\n"),
            ("run.sh", Mode::Executable, b"#!/bin/sh\necho ran\n"),
        ];
        let out = s.run(&["true"], files).expect("runs");
        assert_eq!(out.program, Path::new("/usr/bin/true"));
        assert_eq!(out.exit, Exit::Code(0), "{}", String::from_utf8_lossy(&out.tail));
        let out = s.run(&["./run.sh"], files).expect("runs");
        assert_eq!((out.exit, out.tail.as_slice()), (Exit::Code(0), &b"ran\n"[..]));
        assert!(matches!(s.run(&["no-such-tool-anywhere"], files), Err(CheckError::NotFound(_))));
        // Finding 01f2bed19039: a path outside the tree is refused, absolute or through '..'.
        for outside in ["/usr/bin/true", "../run.sh", "./a/../../run.sh", "/bin/sh"] {
            assert!(matches!(s.run(&[outside], files), Err(CheckError::Outside(_))), "{outside}");
        }
        assert!(matches!(s.run(&[], files), Err(CheckError::EmptyArgv(_))));
        s.assert_cleaned_up();
    }

    // The environment holds exactly PATH, LANG, HOME, TMPDIR and CARGO_HOME, whatever the caller's
    // holds (BRACKEN_* included), and cargo is pointed offline at the vendor directory.
    #[test]
    fn the_environment_is_minimal() {
        let s = Setup::new();
        let out = s.sh("/usr/bin/env; echo ---; cat \"$CARGO_HOME/config.toml\"; pwd");
        let (env, rest) = out.split_once("---\n").expect("both parts");
        let vars: BTreeMap<&str, &str> = env.lines().filter_map(|l| l.split_once('=')).collect();
        let keys: Vec<&str> = vars.keys().copied().collect();
        // bash itself adds PWD, SHLVL and _.
        assert_eq!(keys, ["CARGO_HOME", "HOME", "LANG", "PATH", "PWD", "SHLVL", "TMPDIR", "_"], "{env}");
        assert_eq!(vars["PATH"], "/usr/bin:/bin:/usr/sbin:/sbin");
        let base = s.path();
        // HOME is <base>/<fresh>/w/home: the sandbox writes only under w/.
        let fresh = Path::new(vars["HOME"]).parent().expect("a parent");
        assert_eq!(fresh.file_name().and_then(|n| n.to_str()), Some("w"));
        assert_eq!(fresh.parent().and_then(Path::parent), Some(base.as_path()), "the fresh directory is in the base");
        for (key, dir) in [("HOME", "home"), ("TMPDIR", "tmp"), ("CARGO_HOME", "cargo")] {
            assert_eq!(Path::new(vars[key]), fresh.join(dir), "{key}");
        }
        let vendor = s.runner.vendor.canonicalize().expect("canonical");
        assert!(rest.contains(&format!("directory = \"{}\"", vendor.display())), "{rest}");
        assert!(rest.contains("offline = true") && rest.contains("replace-with = \"vendored-sources\""), "{rest}");
        assert!(rest.trim_end().ends_with(&format!("{}", fresh.join("tree").display())), "runs in the tree: {rest}");
    }

    // Each forbidden access, tried from inside a check against something that exists, is refused.
    #[test]
    fn the_sandbox_refuses_every_forbidden_access() {
        let s = Setup::new();
        let base = s.path();
        // A sibling throwaway Ground in the base ($TMPDIR), with a key built at runtime, and a
        // checkout's .bracken/ beside it.
        let key = format!("{{\"secret_key\": \"{}\"}}\n", "ab".repeat(32));
        std::fs::create_dir_all(base.join("g/identities")).expect("mkdir");
        std::fs::write(base.join("g/identities/alice.json"), &key).expect("write");
        std::fs::create_dir_all(base.join("repo/.bracken")).expect("mkdir");
        std::fs::write(base.join("repo/.bracken/ground.json"), "{}\n").expect("write");
        std::fs::create_dir_all(base.join("bin")).expect("mkdir");
        // A binary that runs outside the sandbox (a copy of Apple's own wouldn't run anywhere): this test.
        // Finding 5431d893f020: in the vendor directory, which the profile lets a check read but not
        // run, so the exec rule refuses it, not the read rule.
        let outside_bin = s.runner.vendor.join("t");
        std::fs::copy(std::env::current_exe().expect("this test"), &outside_bin).expect("copy");
        let listed =
            std::process::Command::new(&outside_bin).args(["--list", "--exact", "nothing"]).output().expect("runs");
        assert!(listed.status.success(), "the copy doesn't run even outside the sandbox");
        let home = std::env::var("HOME").expect("HOME");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let socket = base.join("s.sock");
        let unix = std::os::unix::net::UnixListener::bind(&socket).expect("bind unix");
        unix.set_nonblocking(true).expect("nonblocking");

        let tries = [
            ("read a sibling Ground's key", format!("cat {}", base.join("g/identities/alice.json").display())),
            ("list identities/", format!("ls {}", base.join("g/identities").display())),
            ("read a checkout's .bracken/", format!("cat {}", base.join("repo/.bracken/ground.json").display())),
            ("list the base ($TMPDIR)", format!("ls {}", base.display())),
            ("list the owner's home", format!("ls {home}")),
            ("read ~/.ssh", format!("ls {home}/.ssh")),
            ("read ~/.cargo", format!("ls {home}/.cargo")),
            ("write outside", format!("echo x > {}", base.join("outside.txt").display())),
            ("exec outside", format!("{} --list --exact nothing", outside_bin.display())),
            ("loopback TCP", format!("echo hi > /dev/tcp/127.0.0.1/{port}")),
            ("a unix socket", format!("echo hi | /usr/bin/nc -w 1 -U {}", socket.display())),
            // Both work outside the sandbox: each needs a mach service.
            ("IPC to launchd", "/bin/launchctl list".to_owned()),
            ("IPC to the pasteboard", "/usr/bin/pbpaste".to_owned()),
            // Finding af30f9599508: the process table can't be listed (what remains is in weave.md).
            ("list the process table", "/bin/ps -A".to_owned()),
        ];
        for (what, command) in &tries {
            let out = s.sh(&format!("if ( {command} ) >/dev/null 2>&1; then echo ALLOWED; else echo REFUSED; fi"));
            assert_eq!(out.trim(), "REFUSED", "{what}: {command}");
        }
        assert!(!base.join("outside.txt").exists(), "a write outside landed");
        assert!(listener.accept().is_err(), "the loopback listener was reached");
        assert!(unix.accept().is_err(), "the unix socket was reached");
        // The control: run outside the sandbox, each try works, so none above was refused for another
        // reason. (~/.ssh may not exist.)
        for (what, command) in &tries {
            if *what == "read ~/.ssh" && !Path::new(&home).join(".ssh").exists() {
                continue;
            }
            let script = format!("if ( {command} ) >/dev/null 2>&1; then echo ALLOWED; else echo REFUSED; fi");
            let out = std::process::Command::new("/bin/bash").args(["-c", &script]).current_dir(&base).output();
            let out = String::from_utf8_lossy(&out.expect("bash runs").stdout).into_owned();
            assert_eq!(out.trim(), "ALLOWED", "{what} fails even without the sandbox: {command}");
        }
        // Within the fresh directory, reads, writes and exec work.
        let out = s.sh("echo ok > here && cat here && printf '#!/bin/sh\\necho exec-ok\\n' > e && chmod +x e && ./e");
        assert_eq!(out, "ok\nexec-ok\n");
        s.assert_cleaned_up();
    }

    /// Whether a process holds `pid`: `kill -0` sends no signal, only checks.
    fn alive(pid: &str) -> bool {
        std::process::Command::new("/bin/kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("kill")
            .success()
    }

    // The timeout kills the check's whole process group, and so does a normal end: nothing it
    // started outlives it.
    #[test]
    fn the_process_group_is_killed_at_the_timeout_and_at_the_end() {
        let mut s = Setup::new();
        s.runner.timeout = Duration::from_secs(1);
        let started = Instant::now();
        let out = s.run(&["sh", "-c", "sleep 30 & echo $!; sleep 30"], &[]).expect("runs");
        assert_eq!(out.exit, Exit::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
        let pid = String::from_utf8_lossy(&out.tail).trim().to_owned();
        assert!(!pid.is_empty() && !alive(&pid), "the background child {pid} outlived the timeout");

        s.runner.timeout = Duration::from_secs(20);
        let out = s.run(&["sh", "-c", "sleep 30 & echo $!"], &[]).expect("runs");
        assert_eq!(out.exit, Exit::Code(0));
        let pid = String::from_utf8_lossy(&out.tail).trim().to_owned();
        assert!(!pid.is_empty() && !alive(&pid), "the background child {pid} outlived the check");
        s.assert_cleaned_up();
    }

    // Finding 002a41a816f8: setsid and setpgid (job control's way) are denied, so a child can't leave
    // the group and outlive the kill.
    #[test]
    fn a_check_cant_leave_its_process_group() {
        let s = Setup::new();
        let escapes = [
            ("setsid", "perl -MPOSIX -e 'my $p = fork; if (!$p) { POSIX::setsid(); sleep 30; exit } print \"$p\\n\"'"),
            ("set -m", "set -m; sleep 30 & echo $!"),
            (
                "setpgid",
                "perl -MPOSIX -e 'my $p = fork; if (!$p) { POSIX::setpgid(0, 0); sleep 30; exit } print \"$p\\n\"'",
            ),
        ];
        for (what, script) in escapes {
            let out = s.run(&["bash", "-c", script], &[]).expect("runs");
            let tail = String::from_utf8_lossy(&out.tail).into_owned();
            let pid =
                tail.lines().rfind(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit())).unwrap_or_default();
            assert!(!pid.is_empty(), "{what}: no pid in {tail:?}");
            assert!(!alive(pid), "{what}: the child {pid} left the group and outlived the check");
        }
        s.assert_cleaned_up();
    }

    // Answer 606e4c61281c: posix_spawn with POSIX_SPAWN_SETSID leaves the group past the profile,
    // so the sweep finds the child by parent pid, kills it, and fails the run. Both when the spawner
    // is still running at the timeout, and when the check's leader has already ended normally but
    // left the spawner running in the group.
    #[test]
    fn a_posix_spawn_setsid_child_is_found_killed_and_fails_the_run() {
        let mut s = Setup::new();
        s.runner.timeout = Duration::from_secs(3);
        let python = PYTHON;
        assert!(Path::new(python).exists(), "this test needs the Command Line Tools' python3 at {python}");
        let spawn = format!(
            "{python} -c 'import os, sys, time; p = os.posix_spawn(\"/bin/sleep\", [\"sleep\", \"60\"], {{}}, setsid=True); \
             print(p, os.getpgid(p) != os.getpgrp(), flush=True); time.sleep(60)'"
        );
        for (what, script) in
            [("at the timeout", spawn.clone()), ("after a normal end", format!("{spawn} & sleep 1; exit 0"))]
        {
            let out = s.run(&["bash", "-c", &script], &[]).expect("runs");
            let tail = String::from_utf8_lossy(&out.tail).into_owned();
            let (pid, outside) = tail.trim().split_once(' ').unwrap_or_else(|| panic!("{what}: {tail:?}"));
            let pid: i32 = pid.parse().expect("a pid");
            assert_eq!(outside, "True", "{what}: the child didn't leave the group");
            assert_eq!(out.exit, Exit::LeftGroup(vec![pid]), "{what}: {tail}");
            assert!(!out.exit.passed());
            assert!(out.exit.to_string().contains("left the process group"), "{}", out.exit);
            assert!(!alive(&pid.to_string()), "{what}: the child {pid} outlived the check");
        }
        s.assert_cleaned_up();
    }

    // Finding 04592044646f: clean-up never follows a symlink out of the fresh directory, and still
    // removes a directory the check made unreadable.
    #[test]
    fn clean_up_follows_no_symlink_and_removes_locked_directories() {
        use std::os::unix::fs::PermissionsExt;
        let s = Setup::new();
        let locked = s.path().join("locked");
        std::fs::create_dir(&locked).expect("mkdir");
        std::fs::create_dir(locked.join("inner")).expect("mkdir");
        std::fs::set_permissions(locked.join("inner"), std::fs::Permissions::from_mode(0o500)).expect("chmod");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        let script = format!("ln -s {} link && mkdir -p d/e && chmod 000 d/e d", locked.display());
        let out = s.run(&["bash", "-c", &script], &[]).expect("runs");
        assert_eq!(out.exit, Exit::Code(0), "{}", String::from_utf8_lossy(&out.tail));
        let mode = |p: &Path| std::fs::metadata(p).expect("stat").permissions().mode() & 0o777;
        assert_eq!((mode(&locked), mode(&locked.join("inner"))), (0o500, 0o500), "clean-up followed the symlink");
        s.assert_cleaned_up();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        std::fs::set_permissions(locked.join("inner"), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    }

    #[test]
    fn the_tail_keeps_the_end_of_the_output() {
        let mut s = Setup::new();
        s.runner.tail_bytes = 10;
        let out = s.run(&["sh", "-c", "printf 0123456789abcdef; printf XY >&2"], &[]).expect("runs");
        assert_eq!(out.tail, b"89abcdefXY");
        assert_eq!(out.output_bytes, 18);
    }

    /// The check's timeout for an escapee's run: room for a slow setup on a loaded machine (finding
    /// be74fee20f83), and still well inside [`ESCAPEE_RUN_LIMIT`].
    const ESCAPEE_CHECK_TIMEOUT: Duration = Duration::from_secs(90);

    /// Runs `escapee` (bash, with bash's default PATH) as a process that leaves the check's group with
    /// posix_spawn's setsid, so the sweep can't find it once its spawner is gone, after the commands in
    /// `before`. The escapee first writes the pids to kill (its own, and any it starts) to `escapee.pids`
    /// in the tree, which this reads while the run goes on. The run is on a thread of its own: past
    /// [`ESCAPEE_RUN_LIMIT`] (a reader that didn't stop), those processes are killed, which frees it, and
    /// the test fails rather than hangs (finding be74fee20f83). Returns the setup, the outcome, how long
    /// the run took, and a guard that kills the escapee's processes.
    fn orphaned(
        mut s: Setup,
        before: &str,
        escapee: &str,
    ) -> (Setup, bracken_tree::check::Outcome, Duration, KillOnDrop) {
        let python = PYTHON;
        assert!(Path::new(python).exists(), "this test needs the Command Line Tools' python3 at {python}");
        s.runner.timeout = ESCAPEE_CHECK_TIMEOUT;
        let script = format!(
            "{before}{python} -c 'import os, sys; os.posix_spawn(\"/bin/bash\", [\"bash\", \"-c\", sys.argv[1]], {{}}, \
             setsid=True)' '{escapee}'"
        );
        let base = s.path();
        let (done, finished) = std::sync::mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            let out = s.run(&["bash", "-c", &script], &[]).expect("runs");
            let _ = done.send((s, out));
        });
        let mut kill = KillOnDrop(Vec::new());
        loop {
            if kill.0.is_empty() {
                kill = KillOnDrop::of(&escapee_pids(&base));
            }
            match finished.recv_timeout(Duration::from_millis(50)) {
                Ok((s, out)) => {
                    assert!(!kill.0.is_empty(), "the escapee wrote no pids: {}", String::from_utf8_lossy(&out.tail));
                    return (s, out, started.elapsed(), kill);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if started.elapsed() < ESCAPEE_RUN_LIMIT => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    drop(kill);
                    let _ = finished.recv_timeout(Duration::from_secs(30));
                    panic!("the run was still reading the escapee's output after {ESCAPEE_RUN_LIMIT:?}");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the run's thread panicked"),
            }
        }
    }

    /// The pids an escapee wrote to `escapee.pids` in a run's tree, in `base`; none until it has. Only
    /// those above 1: `kill` takes 0 and -1 as the caller's group and every process it may signal
    /// (finding 91a14a75ae16).
    fn escapee_pids(base: &Path) -> Vec<i32> {
        let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("weave-check-") || name.ends_with(".removing") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path().join("w/tree/escapee.pids")) {
                return text.split_whitespace().filter_map(|p| p.parse().ok()).filter(|&p| p > 1).collect();
            }
        }
        Vec::new()
    }

    // Finding 234b9aad594d and the reviewer's asks: the documented gap, an escapee orphaned before
    // the sweep, still can't keep the output reader alive (the run returns and joins it), and holding the
    // output fails the run (task 6b8a1b5a6108). It can't create the fresh directory again, during its
    // removal or after: clean-up first renames it, and the sandbox may write only under the old path's w/.
    // Finding 32a23fca15ab: the removal is made long (twenty thousand files), and the escapee, without a
    // pause, keeps making directories it locks under the old absolute path, which would leave the
    // directory behind if one landed during the removal; and writes relative to its own directory, which
    // is inside the renamed tree.
    #[test]
    fn an_orphaned_escapee_cant_hold_the_reader_or_recreate_the_directory() {
        let bulk =
            format!("mkdir bulk && {PYTHON} -c 'for i in range(20000): open(\"bulk/%d\" % i, \"w\").close()' && ");
        let escapee = r#"echo $$ > p.tmp && mv p.tmp escapee.pids; F=$(cd ../.. && pwd); i=0; while :; do i=$((i+1)); mkdir -p "$F/w/l$i/x" 2>/dev/null && chmod 000 "$F/w/l$i" 2>/dev/null; mkdir "r$i" 2>/dev/null; echo x > "../f$i" 2>/dev/null; done"#;
        let (s, out, took, _kill) = orphaned(Setup::new(), &bulk, escapee);
        assert_eq!(out.exit, Exit::HeldOutput, "{}", out.exit);
        assert!(!out.exit.passed());
        assert!(took < ESCAPEE_RUN_LIMIT, "the run waited on the escapee: {took:?}");
        std::thread::sleep(Duration::from_millis(500));
        s.assert_cleaned_up();
    }

    // Task 6b8a1b5a6108: orphaned escapees that write without pause, eight at once, still can't keep the
    // reader going: it stops when told, and the run fails as held. (Whether the pipe ever runs dry here is
    // up to the scheduler; check.rs's own test gives the reader a pipe that never does.)
    #[test]
    fn orphaned_escapees_writing_without_pause_cant_hold_the_reader() {
        let writers = "for i in 1 2 3 4 5 6 7 8; do /bin/cat /dev/zero & echo $! >> p.tmp; done; echo $$ >> p.tmp; mv p.tmp escapee.pids; wait";
        let (s, out, took, _kill) = orphaned(Setup::new(), "", writers);
        assert_eq!(out.exit, Exit::HeldOutput, "{}", out.exit);
        assert!(took < ESCAPEE_RUN_LIMIT, "the run waited on the escapees: {took:?}");
        s.assert_cleaned_up();
    }

    // Finding 91a14a75ae16: the guard kills a process it saw running, and not one holding a pid it
    // recorded with another start time, as a reused pid would be. Each sleep is killed and reaped when
    // the test ends, even if an assert fails, and the last assert builds no guard, as one for an exited
    // pid could kill whatever took it (finding 39cd8d65bd09).
    #[test]
    fn the_kill_guard_kills_only_the_processes_it_saw() {
        let spawn = || Reaped(std::process::Command::new("/bin/sleep").arg("60").spawn().expect("a sleep"));
        let (mut seen, mut reused) = (spawn(), spawn());
        let guard = KillOnDrop::of(&[seen.0.id() as i32]);
        assert_eq!(guard.0.len(), 1, "the running sleep is recorded");
        drop(guard);
        let status = seen.0.wait().expect("the sleep's status");
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(9), "{status}");
        drop(KillOnDrop(vec![(reused.0.id() as i32, "Thu Jan  1 00:00:00 1970".to_owned())]));
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            reused.0.try_wait().expect("the other sleep's status").is_none(),
            "a pid that isn't the one seen was killed"
        );
        assert!(started(&[seen.0.id() as i32]).is_empty(), "an exited process isn't recorded");
    }

    /// A child that is killed and reaped when dropped. Once reaped, `Child::kill` sends nothing, so a
    /// pid taken since isn't signalled.
    struct Reaped(std::process::Child);

    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // A panic inside the run (here, the file source) still removes the fresh directory.
    #[test]
    fn a_panic_during_the_run_removes_the_fresh_directory() {
        let s = Setup::new();
        let argv = vec!["true".to_owned()];
        let (manifest, _) = tree(&[("README", Mode::File, b"r\n")]);
        let runner = s.runner.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner.run(&CheckDefinition { name: "test", argv: &argv }, &manifest, |_| panic!("the file source failed"))
        }));
        assert!(result.is_err(), "the panic didn't reach the caller");
        s.assert_cleaned_up();
    }

    // With a toolchain: RUSTUP_HOME and DEVELOPER_DIR are set, PATH starts with the toolchain's
    // bin, and its rustc runs under the profile.
    #[test]
    fn a_toolchain_is_on_path_and_runs_under_the_profile() {
        let mut s = Setup::new();
        let sysroot = std::process::Command::new("rustc").args(["--print", "sysroot"]).output().expect("rustc runs");
        let sysroot = PathBuf::from(String::from_utf8(sysroot.stdout).expect("UTF-8").trim());
        let toolchains = sysroot.parent().expect("toolchains");
        assert_eq!(
            toolchains.file_name().and_then(|n| n.to_str()),
            Some("toolchains"),
            "{} isn't a rustup toolchain",
            sysroot.display()
        );
        let rustup_home = toolchains.parent().expect("rustup home").to_path_buf();
        s.runner.toolchain =
            Some(bracken_tree::check::Toolchain { bin: sysroot.join("bin"), rustup_home: rustup_home.clone() });
        let out = s.sh("echo \"$RUSTUP_HOME|$DEVELOPER_DIR|${PATH%%:*}\"; rustc --version");
        let mut lines = out.lines();
        assert_eq!(
            lines.next(),
            Some(
                format!(
                    "{}|{}|{}",
                    rustup_home.display(),
                    bracken_tree::check::DEVELOPER_DIR,
                    sysroot.join("bin").display()
                )
                .as_str()
            )
        );
        assert!(lines.next().is_some_and(|l| l.starts_with("rustc ")), "rustc didn't run: {out}");
        s.assert_cleaned_up();
    }

    // No sandbox, no run: without a usable sandbox-exec the runner refuses before materializing
    // anything, so the check never starts. Here, so the Mac half of a split run exercises the probe
    // (finding 4891b9db1618): elsewhere the runner refuses before any probe.
    #[test]
    fn without_a_sandbox_nothing_runs() {
        let s = Setup::new();
        // Finding 63ce139411e0: a stand-in that runs the program without the profile fails the probe too.
        let open = s.path().join("open-sandbox-exec");
        std::fs::write(&open, "#!/bin/sh\nwhile [ \"$1\" = -D ] || [ \"$1\" = -p ]; do shift 2; done\nexec \"$@\"\n")
            .expect("write");
        std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
        for sandbox_exec in [Path::new("/nonexistent/sandbox-exec"), Path::new("/usr/bin/false"), open.as_path()] {
            s.assert_refused(&s.runner.clone().with_sandbox_exec(sandbox_exec));
        }
    }
}

// On another OS there is no sandbox, so even the real sandbox-exec path is refused and nothing runs.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_nothing_runs() {
    let s = Setup::new();
    s.assert_refused(&s.runner);
}

// The tail as recorded: control characters stripped, and withheld as its size and hash when it holds
// key material (built at runtime, gotcha 5a3dda0b1f93).
#[test]
fn a_tail_holding_a_key_is_withheld() {
    let known = KnownKeys::new();
    assert_eq!(
        recorded_tail(b"ok\x1b[31m red\x07\n\tdone\n", &known),
        RecordedTail::Text("ok[31m red\n\tdone\n".to_owned())
    );
    let pem = format!("build log\n-----BEGIN {}PRIVATE KEY-----\nMIIE\n", "RSA ");
    match recorded_tail(pem.as_bytes(), &known) {
        RecordedTail::Withheld { size, hash, .. } => {
            assert_eq!(size, pem.len() as u64);
            assert_eq!(hash, ContentHash::of(pem.as_bytes()));
        }
        other => panic!("not withheld: {other:?}"),
    }
}

/// Kills processes by pid when dropped, so a test leaves no escapee behind even when it fails. Each pid
/// is kept with its process's start time, and is killed only if the process holding it then started at
/// that same time: the escapees may have exited by then, and a reused pid isn't theirs (finding
/// 91a14a75ae16). The start time, not the command, as a background child may not have run its exec yet.
#[cfg(target_os = "macos")]
struct KillOnDrop(Vec<(i32, String)>);

#[cfg(target_os = "macos")]
impl KillOnDrop {
    /// A guard for those of `pids` that are running now.
    fn of(pids: &[i32]) -> KillOnDrop {
        KillOnDrop(started(pids))
    }
}

/// Each of `pids` still running, with its start time, as `ps` gives it.
#[cfg(target_os = "macos")]
fn started(pids: &[i32]) -> Vec<(i32, String)> {
    if pids.is_empty() {
        return Vec::new();
    }
    let list = pids.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    // `ps` exits 1 when a pid isn't running, and still lists the others.
    let Ok(out) = std::process::Command::new("/bin/ps").args(["-o", "pid=,lstart=", "-p", &list]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, start) = line.trim().split_once(' ')?;
            Some((pid.parse().ok()?, start.trim().to_owned()))
        })
        .filter(|(pid, _)| pids.contains(pid))
        .collect()
}

#[cfg(target_os = "macos")]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let pids: Vec<i32> = self.0.iter().map(|(pid, _)| *pid).collect();
        for still in started(&pids).iter().filter(|p| self.0.contains(p)) {
            let _ = std::process::Command::new("/bin/kill").args(["-KILL", &still.0.to_string()]).status();
        }
    }
}
