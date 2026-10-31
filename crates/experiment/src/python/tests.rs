#[cfg(not(target_os = "macos"))]
#[test]
fn elsewhere_agent_code_is_refused() {
    use super::*;
    let python = Python {
        executable: "/usr/bin/python3".into(),
        digest: String::new(),
        stamp: String::new(),
        version: "3.12.0".into(),
        prefix: "/usr".into(),
        libraries: Vec::new(),
        links: Vec::new(),
    };
    let scratch = Scratch::new().expect("a scratch directory");
    let err = python.run(&scratch, &["-c".into(), "pass".into()], &Limits::default()).expect_err("refused");
    assert!(err.contains("sandbox-exec"), "{err}");
}

#[cfg(target_os = "macos")]
mod macos_only {
    use std::net::TcpListener;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::super::*;

    /// The interpreter these tests pin: Homebrew's Python 3.12 (decision `c008b959c572`), or the one
    /// `BRACKEN_TEST_PYTHON` names.
    fn python() -> Python {
        let path = std::env::var_os("BRACKEN_TEST_PYTHON").unwrap_or_else(|| "/opt/homebrew/bin/python3.12".into());
        assert!(Path::new(&path).exists(), "these tests need Python 3.12 at {path:?} (or set BRACKEN_TEST_PYTHON)");
        Python::pin(Path::new(&path), "3.12", None).expect("pinned")
    }

    fn limits() -> Limits {
        Limits { cpu_seconds: 5, memory_mb: 256, output_bytes: 1 << 20, wall_seconds: 20, scratch_mb: 64 }
    }

    /// `code` run under the profile, with `limits`.
    fn sandboxed(code: &str, limits: &Limits) -> Outcome {
        let scratch = Scratch::new().expect("a scratch directory");
        python().run(&scratch, &["-c".into(), code.into()], limits).expect("ran")
    }

    /// `code` run with the same interpreter and flags, but no sandbox: to show an attempt works there.
    fn unsandboxed(code: &str) -> String {
        let out =
            Command::new(python().executable).args(["-I", "-S", "-B", "-c", code]).env_clear().output().expect("ran");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn stdout(o: &Outcome) -> String {
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    #[test]
    fn the_real_interpreter_is_pinned_with_the_libraries_it_loads() {
        let p = python();
        assert!(p.version.starts_with("3.12."), "{}", p.version);
        assert!(p.executable.starts_with(&p.prefix), "{} in {}", p.executable.display(), p.prefix.display());
        assert!(!p.libraries.is_empty(), "Homebrew's decimal, ssl and sqlite3 load libraries outside the prefix");
        assert!(p.libraries.iter().all(|l| !l.starts_with(&p.prefix) && l.is_absolute()), "{:?}", p.libraries);
        let err = Python::pin(&p.executable, "3.11", None).expect_err("another version");
        assert!(err.contains("written for 3.11"), "{err}");
        // A pin with the digest a plan recorded takes the same interpreter, and refuses one with another.
        let again = Python::pin(&p.executable, "3.12", Some(&p.digest)).expect("the same digest");
        assert_eq!(again.digest, p.digest);
        let err = Python::pin(&p.executable, "3.12", Some(&"0".repeat(64))).expect_err("another digest");
        assert!(err.contains("not the expected"), "{err}");
        p.check().expect("nothing has changed");
        // The standard library, with the modules that load those libraries, works under the profile.
        let o = sandboxed("import decimal, sqlite3, ssl, json, datetime; print('OK')", &limits());
        assert_eq!(
            (o.exit.clone(), stdout(&o).trim()),
            (Exit::Code(0), "OK"),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    /// The digest is of the installation's contents, `site-packages` aside; the stamp changes with any of its
    /// files, even rewritten with the same bytes.
    #[test]
    fn the_digest_and_the_stamp_follow_the_installation() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let prefix = std::fs::canonicalize(tmp.path()).expect("resolved");
        std::fs::create_dir_all(prefix.join("lib/site-packages")).expect("dirs");
        std::fs::create_dir_all(prefix.join("bin")).expect("dirs");
        std::fs::write(prefix.join("lib/os.py"), "x").expect("a file");
        std::fs::write(prefix.join("lib/site-packages/extra.py"), "x").expect("a file");
        std::fs::write(prefix.join("bin/python"), "x").expect("a file");
        let exe = prefix.join("bin/python");
        let both = || {
            let f = |what| fingerprint(&prefix, &exe, &[], what).expect("a fingerprint");
            (f(Fingerprint::Contents), f(Fingerprint::Metadata))
        };
        let (digest, stamp) = both();
        std::fs::write(prefix.join("lib/site-packages/extra.py"), "changed").expect("a file");
        assert_eq!(both(), (digest.clone(), stamp.clone()), "site-packages is not the interpreter's");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(prefix.join("lib/os.py"), "x").expect("a file");
        let (same, restamped) = both();
        assert_eq!(same, digest);
        assert_ne!(restamped, stamp, "rewritten");
        std::os::unix::fs::symlink("os.py", prefix.join("lib/link.py")).expect("a link");
        assert_ne!(both().0, digest, "a new file");
    }

    /// Each forbidden access is refused under the profile, and the same attempt works without it, so it is the
    /// profile that refuses it (gotcha fd7742fa).
    #[test]
    fn every_forbidden_access_is_refused_and_works_without_the_sandbox() {
        let outside = tempfile::tempdir().expect("a temp dir");
        let outside = std::fs::canonicalize(outside.path()).expect("resolved");
        std::fs::write(outside.join("secret.txt"), "secret").expect("a file");
        let listener = TcpListener::bind("127.0.0.1:0").expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        let _unix = std::os::unix::net::UnixListener::bind(outside.join("l.sock")).expect("a unix listener");
        let o = outside.display();
        let home = std::fs::canonicalize(std::env::var_os("HOME").expect("a HOME")).expect("resolved");
        let home = home.display();
        // Only whether each worked is printed, never what it read.
        let mut attempts = vec![
            ("read a file outside", format!("open('{o}/secret.txt').read(); print('OK')")),
            ("list a directory outside", format!("import os; os.listdir('{o}'); print('OK')")),
            ("write outside", format!("open('{o}/written.txt', 'w').write('x'); print('OK')")),
            ("fork", "import os\npid = os.fork()\nif pid == 0: os._exit(0)\nos.waitpid(pid, 0); print('OK')".into()),
            ("start a program", "import subprocess; subprocess.run(['/bin/echo']); print('OK')".into()),
            ("connect", format!("import socket; socket.create_connection(('127.0.0.1', {port})); print('OK')")),
            ("read the owner's settings", "import os; os.listdir('/Users'); print('OK')".into()),
            ("list the owner's home", format!("import os; os.listdir('{home}'); print('OK')")),
            ("stat a file outside", format!("import os; os.stat('{o}/secret.txt'); print('OK')")),
            ("stat the owner's home", format!("import os; os.stat('{home}'); print('OK')")),
            (
                "connect to a unix socket",
                format!("import socket\ns = socket.socket(socket.AF_UNIX)\ns.connect('{o}/l.sock'); print('OK')"),
            ),
            (
                "bind a unix socket",
                format!("import socket\ns = socket.socket(socket.AF_UNIX)\ns.bind('{o}/b.sock'); print('OK')"),
            ),
            (
                "look up a mach service",
                "import ctypes\nlibc = ctypes.CDLL(None)\nport = ctypes.c_uint(0)\n\
                bootstrap = ctypes.c_uint.in_dll(libc, 'bootstrap_port')\n\
                r = libc.bootstrap_look_up(bootstrap, b'com.apple.SecurityServer', ctypes.byref(port))\n\
                assert r == 0; print('OK')"
                    .into(),
            ),
            ("open a semaphore", "import multiprocessing; multiprocessing.Lock(); print('OK')".into()),
            (
                "open shared memory",
                "from multiprocessing import shared_memory\n\
                m = shared_memory.SharedMemory(create=True, size=16); m.close(); m.unlink(); print('OK')"
                    .into(),
            ),
        ];
        let ssh = std::path::PathBuf::from(home.to_string()).join(".ssh");
        if ssh.exists() {
            attempts.push(("list ~/.ssh", format!("import os; os.listdir('{}'); print('OK')", ssh.display())));
        }
        for (what, code) in attempts {
            assert_eq!(unsandboxed(&code).trim(), "OK", "{what} works without the sandbox");
            let _ = std::fs::remove_file(outside.join("written.txt"));
            let _ = std::fs::remove_file(outside.join("b.sock"));
            let o = sandboxed(&code, &limits());
            assert_ne!(stdout(&o).trim(), "OK", "{what} under the sandbox");
            assert_ne!(o.exit, Exit::Code(0), "{what}");
        }
        assert!(!outside.join("written.txt").exists());
        assert!(!outside.join("b.sock").exists());
    }

    /// The probe [`no_other_process_can_be_read`] runs: whether it can read its parent's arguments and
    /// environment (`KERN_PROCARGS2`, `KERN_PROCARGS`, by number and by name) or list or name other processes.
    /// It prints only a verdict for each, never what it read.
    const PROBE: &str = r#"import ctypes, os
libc = ctypes.CDLL(None, use_errno=True)
libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)

def sysctl(mib):
    # What's read can grow between asking its size and reading it, such as the process table: so the buffer
    # has room to spare, and a read that still finds it too small (ENOMEM) is tried again.
    mib = (ctypes.c_int * len(mib))(*mib)
    for _ in range(8):
        size = ctypes.c_size_t(0)
        if libc.sysctl(mib, len(mib), None, ctypes.byref(size), None, 0) != 0 or size.value == 0:
            return None
        size = ctypes.c_size_t(size.value * 2 + 65536)
        buf = ctypes.create_string_buffer(size.value)
        if libc.sysctl(mib, len(mib), buf, ctypes.byref(size), None, 0) == 0:
            return buf.raw[: size.value]
        if ctypes.get_errno() != 12:
            return None
    return None

def named(name):
    mib = (ctypes.c_int * 4)()
    n = ctypes.c_size_t(4)
    if libc.sysctlnametomib(name.encode(), mib, ctypes.byref(n)) != 0:
        return None
    return sysctl(list(mib)[: n.value] + [os.getppid()])

def verdict(data):
    if data is None:
        return "denied"
    return "leaked" if b"BRACKEN_PROBE_PLANTED_NAME" in data else "read"

path = ctypes.create_string_buffer(4096)
print("procargs2 by number:", verdict(sysctl([1, 49, os.getppid()])))
print("procargs by number:", verdict(sysctl([1, 38, os.getppid()])))
print("procargs2 by name:", verdict(named("kern.procargs2")))
print("procargs by name:", verdict(named("kern.procargs")))
print("all pids:", "denied" if libproc.proc_listallpids(None, 0) <= 0 else "read")
print("parent's path:", "denied" if libproc.proc_pidpath(os.getppid(), path, 4096) <= 0 else "read")
print("all processes:", verdict(sysctl([1, 14, 0])))
"#;

    /// The parent [`no_other_process_can_be_read`] starts, with the planted name in the environment it was
    /// started with: it runs each command its file names, the probe its child, and prints each one's output.
    const PARENT: &str = r#"import json, subprocess, sys
for run in json.load(open(sys.argv[1])):
    out = subprocess.run(run["argv"], cwd=run["cwd"], env=run["env"], capture_output=True, text=True, timeout=60)
    print(json.dumps({"stdout": out.stdout, "stderr": out.stderr[-2000:], "code": out.returncode}))
"#;

    /// Agent code can't read another process's arguments or environment, where a parent's secrets would be, or
    /// list or name other processes; the same probe without the profile reads its parent's (finding
    /// c44e0191291a, note 116ae2144d9d).
    #[test]
    fn no_other_process_can_be_read() {
        let python = python();
        let scratch = Scratch::new().expect("a scratch directory");
        std::fs::write(scratch.dir.join("probe.py"), PROBE).expect("the probe");
        let sandboxed = python.command(&scratch, &["probe.py".into()]).expect("a command");
        let argv: Vec<String> = std::iter::once(sandboxed.get_program())
            .chain(sandboxed.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let env: serde_json::Map<_, _> = sandboxed
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into())))
            .collect();
        let control = [python.executable.to_string_lossy(), "-I".into(), "-S".into(), "-B".into(), "probe.py".into()];
        let runs = serde_json::json!([
            {"argv": argv, "cwd": scratch.dir, "env": env},
            {"argv": control, "cwd": scratch.dir, "env": env},
        ]);
        let tmp = tempfile::tempdir().expect("a temp dir");
        std::fs::write(tmp.path().join("runs.json"), runs.to_string()).expect("the runs");
        std::fs::write(tmp.path().join("parent.py"), PARENT).expect("the parent");
        let out = Command::new(&python.executable)
            .args(["-I", "-S", "-B"])
            .arg(tmp.path().join("parent.py"))
            .arg(tmp.path().join("runs.json"))
            .env_clear()
            .env("BRACKEN_PROBE_PLANTED_NAME", "1")
            .output()
            .expect("ran");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let results: Vec<serde_json::Value> =
            String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).expect("a result")).collect();
        let verdicts = |r: &serde_json::Value| {
            assert_eq!(r["code"], 0, "{}", r["stderr"]);
            r["stdout"].as_str().expect("its output").to_string()
        };
        let others = |verdict| format!("all pids: {verdict}\nparent's path: {verdict}\nall processes: {verdict}\n");
        let all = |procargs, other| {
            format!(
                "procargs2 by number: {procargs}\nprocargs by number: {procargs}\nprocargs2 by name: {procargs}\n\
                 procargs by name: {procargs}\n{}",
                others(other)
            )
        };
        assert_eq!(verdicts(&results[1]), all("leaked", "read"), "without the profile, the probe reads its parent's");
        assert_eq!(verdicts(&results[0]), all("denied", "denied"), "under the profile");
    }

    #[test]
    fn the_environment_is_only_its_own() {
        let o = sandboxed("import os, json; print(json.dumps(sorted(os.environ)))", &limits());
        let names: Vec<String> = serde_json::from_slice(&o.stdout).expect("names");
        let allowed = ["HOME", "TMPDIR", "__CF_USER_TEXT_ENCODING"];
        assert!(names.iter().all(|n| allowed.contains(&n.as_str())), "{names:?}");
        let o = sandboxed("import os; print(os.environ['HOME'] == os.getcwd())", &limits());
        assert_eq!(stdout(&o).trim(), "True", "HOME is the scratch directory");
    }

    #[test]
    fn each_limit_ends_the_run() {
        let quick = Limits { cpu_seconds: 1, memory_mb: 128, output_bytes: 64 << 10, wall_seconds: 3, scratch_mb: 8 };
        // A second of CPU can take several of wall time on a loaded machine, so the wall time is generous here.
        let o = sandboxed("while True: pass", &Limits { wall_seconds: 60, ..quick });
        assert_eq!(o.exit, Exit::Signal(24), "SIGXCPU");

        let o = sandboxed("b = bytearray(1 << 30)\nfor i in range(0, len(b), 4096): b[i] = 1", &quick);
        assert_eq!(o.exit, Exit::Memory);

        let start = Instant::now();
        let o = sandboxed("import sys\nwhile True: sys.stdout.write('x' * 4096)", &quick);
        assert_eq!(o.exit, Exit::Output);
        assert_eq!(o.stdout.len(), 64 << 10, "kept up to the limit");
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());

        let start = Instant::now();
        let o = sandboxed("import time; time.sleep(60)", &quick);
        assert_eq!(o.exit, Exit::WallTime);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());

        // Python ignores SIGXFSZ, so a write past the file size limit fails instead of ending it.
        let o = sandboxed("open('big', 'wb').write(b'x' * (32 << 20))", &quick);
        assert!(String::from_utf8_lossy(&o.stderr).contains("File too large"), "{o:?}");

        // The scratch directory's size and its count of files are each limited, however many files hold it.
        let o = sandboxed(
            "for i in range(16): open(f'f{i}', 'wb').write(b'x' * (1 << 20))\nimport time; time.sleep(60)",
            &quick,
        );
        assert_eq!(o.exit, Exit::Scratch);
        // Nor can the code hide what it writes by first taking away the owner's read access to where it writes.
        let o = sandboxed(
            "import os, time\nos.mkdir('d')\nos.chmod('d', 0o300)\nos.chmod('.', 0o300)\n\
             for i in range(16): open(f'd/f{i}', 'wb').write(b'x' * (1 << 20))\ntime.sleep(60)",
            &quick,
        );
        assert_eq!(o.exit, Exit::Scratch);
        // Each directory made costs the sandbox's checks, so the CPU limit is generous here.
        let many = Limits { cpu_seconds: 20, wall_seconds: 60, ..quick };
        let o = sandboxed("import os, time\nfor i in range(12000): os.mkdir(f'd{i}')\ntime.sleep(60)", &many);
        assert_eq!(o.exit, Exit::Scratch);
    }

    /// The code can't make the parent act on a file outside by swapping a directory it took the owner's access
    /// to for a symlink to that file while the parent measures the scratch directory (finding b9a13da2a14c).
    #[test]
    fn the_parent_never_acts_on_a_path_the_code_can_swap() {
        use std::os::unix::fs::PermissionsExt;
        let outside = tempfile::tempdir().expect("a temp dir");
        let target = std::fs::canonicalize(outside.path()).expect("resolved").join("target");
        std::fs::write(&target, "x").expect("a file");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).expect("its mode");
        // The reproduction of b9a13da2a14c: many files to walk widen the window between looking at the directory
        // and acting on it, and a run gives the race one chance or a few, so it is run many times.
        let swap = format!(
            "import ctypes, os, time\nlibc = ctypes.CDLL(None)\nfor i in range(3000): open(f'f{{i}}', 'w').close()\n\
             os.mkdir('a')\nos.chmod('a', 0o077)\nos.symlink('{}', 'b')\nend = time.time() + 2\n\
             while time.time() < end: libc.renamex_np(b'a', b'b', 2)\n",
            target.display()
        );
        for _ in 0..20 {
            let o = sandboxed(&swap, &Limits { cpu_seconds: 20, wall_seconds: 12, ..limits() });
            let mode = std::fs::symlink_metadata(&target).expect("still there").permissions().mode() & 0o7777;
            assert_eq!(mode, 0o600, "the file outside keeps its mode");
            assert_eq!(o.exit, Exit::Scratch, "a directory it can't read is past the limit");
        }
    }

    #[test]
    fn the_scratch_directory_is_removed_whatever_the_code_did_to_it() {
        let scratch = Scratch::new().expect("a scratch directory");
        let dir = scratch.dir.clone();
        let code = "import os; os.makedirs('a/b'); open('a/b/f', 'w').write('x'); os.chmod('a/b', 0); os.chmod('a', 0)";
        let o = python().run(&scratch, &["-c".into(), code.into()], &limits()).expect("ran");
        assert_eq!(o.exit, Exit::Code(0), "{}", String::from_utf8_lossy(&o.stderr));
        drop(scratch);
        assert!(!dir.exists(), "{}", dir.display());
    }

    /// The fixture's public test passes; a tree with a failing, an erroring and an unimportable test says so.
    #[test]
    fn run_tests_reports_each_test() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny");
        let project = Project::load(&fixture).expect("the fixture");
        let python = python();
        let run = run_tests(&python, &project, &project.seed).expect("ran");
        assert!(run.reported, "{}", run.output);
        assert_eq!(run.tests, [("test_stats.MeanTest.test_mean".to_string(), "pass".to_string())]);

        let tmp = tempfile::tempdir().expect("a temp dir");
        let tree = tmp.path().join("tree");
        copy_tree(&project.seed, &tree).expect("a copy");
        std::fs::write(
            tree.join("tests/test_more.py"),
            "import unittest\nfrom tiny.stats import mean\n\nclass More(unittest.TestCase):\n    \
             def test_wrong(self):\n        self.assertEqual(mean([1]), 2)\n    \
             def test_raises(self):\n        mean([])\n",
        )
        .expect("a test");
        std::fs::write(tree.join("tests/test_broken.py"), "import tiny.missing\n").expect("a test");
        let run = run_tests(&python, &project, &tree).expect("ran");
        assert!(run.reported, "{}", run.output);
        assert_eq!((run.count("pass"), run.count("fail"), run.count("error")), (1, 1, 2), "{:?}", run.tests);
        assert!(run.output.contains("ZeroDivisionError"), "{}", run.output);
    }
}
