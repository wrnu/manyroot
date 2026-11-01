//! These need Docker, with the pinned image pulled (`docker pull` [`IMAGE`]): where Docker isn't running, each
//! says so and passes without checking anything. The counted run on the Mac runs them.

/// A plan's pin of a host interpreter, from before agent code ran in containers, is refused saying so, not as an
/// unknown field (finding 264a0f0c8160); a pin of an image is read.
#[test]
fn an_old_plans_interpreter_pin_is_refused_saying_why() {
    let old = serde_json::json!({"path": "/opt/homebrew/bin/python3.12", "version": "3.12.16", "digest": "ab"});
    let err = serde_json::from_value::<super::PythonPin>(old).expect_err("refused").to_string();
    assert!(err.contains("before agent code ran in containers"), "{err}");
    let new = serde_json::json!({"image": super::IMAGE, "version": "3.12.16"});
    assert_eq!(serde_json::from_value::<super::PythonPin>(new).expect("read").image, super::IMAGE);
    let extra = serde_json::json!({"image": super::IMAGE, "version": "3.12.16", "more": 1});
    assert!(serde_json::from_value::<super::PythonPin>(extra).is_err(), "no other field");
}

/// In `macos_only` though they don't need macOS, so the counted run's Mac side runs them: Docker is on this Mac
/// and not in the test boxes, where they skip (decision ac22cedd8fee, docs/development.md).
pub(crate) mod macos_only {
    use std::io::BufRead;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::super::*;

    /// The image these tests pin ([`for_tests`]); None, having said why, where they can't run.
    pub(crate) fn python() -> Option<Python> {
        for_tests()
    }

    fn limits() -> Limits {
        Limits { cpu_seconds: 5, memory_mb: 256, output_bytes: 1 << 20, wall_seconds: 20, scratch_mb: 64 }
    }

    /// `code` run under the profile, with `limits`.
    fn contained(python: &Python, code: &str, limits: &Limits) -> Outcome {
        let scratch = Scratch::new().expect("a scratch directory");
        python.run(&scratch, &["-c".into(), code.into()], limits).expect("ran")
    }

    fn stdout(o: &Outcome) -> String {
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    fn stderr(o: &Outcome) -> String {
        String::from_utf8_lossy(&o.stderr).into_owned()
    }

    /// The image is pinned by its digest, and a plan's pin takes only that image with that version.
    #[test]
    fn the_image_is_pinned_by_digest() {
        let Some(p) = python() else { return };
        assert!(p.version.starts_with("3.12."), "{}", p.version);
        assert!(p.image.ends_with(&format!("@{}", p.digest)), "{p:?}");
        let err = Python::pin("python:3.12", "3.12").expect_err("no digest");
        assert!(err.contains("by digest"), "{err}");
        let none = format!("python@sha256:{}", "0".repeat(64));
        assert!(Python::pin(&none, "3.12").expect_err("another digest").contains("isn't here"));
        let err = Python::pin(&p.image, "3.11").expect_err("another version");
        assert!(err.contains("written for 3.11"), "{err}");
        let pin = p.plan_pin();
        assert_eq!(Python::from_plan(&pin, "3.12").expect("the plan's").digest, p.digest);
        let other = PythonPin { version: "3.12.0".into(), ..pin.clone() };
        assert!(Python::from_plan(&other, "3.12").expect_err("another version").contains("not the plan's"));
        let other = PythonPin { image: none, ..pin };
        assert!(Python::from_plan(&other, "3.12").expect_err("another digest").contains("isn't here"));
        // The standard library works in it, the modules that load libraries of their own too.
        let o = contained(&p, "import decimal, sqlite3, ssl, json, datetime; print('OK')", &limits());
        assert_eq!((o.exit.clone(), stdout(&o).trim()), (Exit::Code(0), "OK"), "{}", stderr(&o));
    }

    /// The code reaches nothing of the host's and nothing outside its scratch directory: no network, no path of the
    /// host's, no write but to the scratch directory, no capability, and no way to its init or the supervisor, which
    /// run as other users. Only whether each worked is printed, never what it read.
    #[test]
    fn the_code_reaches_nothing_but_its_scratch_directory() {
        let Some(p) = python() else { return };
        let outside = tempfile::tempdir().expect("a temp dir");
        std::fs::write(outside.path().join("secret.txt"), "secret").expect("a file");
        let home = std::env::var("HOME").expect("a HOME");
        let code = format!(
            "import os, signal, socket\n\
             def tried(what, f):\n    try:\n        f(); print(what, 'worked')\n    except OSError:\n        print(what, 'refused')\n\
             tried('connect', lambda: socket.create_connection(('1.1.1.1', 53), 2))\n\
             tried('resolve', lambda: socket.getaddrinfo('example.com', 80))\n\
             tried('read outside', lambda: open('{}/secret.txt').read())\n\
             tried('list the home', lambda: os.listdir('{home}'))\n\
             for path in ['/etc/x', '/tmp/x', '/usr/local/x', '/in/x']:\n    tried('write ' + path, lambda: open(path, 'w'))\n\
             tried('write scratch', lambda: open('/scratch/x', 'w').write('x'))\n\
             others = [int(d) for d in os.listdir('/proc') if d.isdigit() and int(d) != os.getpid()]\n\
             for pid in others:\n    tried('signal another', lambda: os.kill(pid, signal.SIGKILL))\n\
             \x20   tried('read another', lambda: open(f'/proc/{{pid}}/mem', 'rb'))\n\
             status = open('/proc/self/status').read().splitlines()\n\
             print(os.getuid(), [l.split()[1] for l in status if l.startswith(('CapEff', 'CapBnd', 'NoNewPrivs'))])\n",
            outside.path().display()
        );
        let o = contained(&p, &code, &limits());
        let out = stdout(&o);
        assert_eq!(o.exit, Exit::Code(0), "{out}{}", stderr(&o));
        let worked: Vec<&str> = out.lines().filter(|l| l.ends_with(" worked")).collect();
        assert_eq!(worked, ["write scratch worked"], "{out}");
        assert_eq!(out.matches("signal another refused").count(), 2, "init and the supervisor: {out}");
        assert!(out.ends_with("65534 ['0000000000000000', '0000000000000000', '1']\n"), "{out}");
    }

    #[test]
    fn the_environment_is_only_its_own() {
        let Some(p) = python() else { return };
        let o = contained(&p, "import os, json; print(json.dumps(sorted(os.environ)))", &limits());
        let names: Vec<String> = serde_json::from_slice(&o.stdout).expect("names");
        assert_eq!(names, ["HOME", "LANG", "PATH", "TMPDIR"]);
        let o = contained(&p, "import os; print(os.environ['HOME'] == os.getcwd() == '/scratch')", &limits());
        assert_eq!(stdout(&o).trim(), "True", "HOME is the scratch directory");
    }

    /// The code writes to its standard output what looks like the supervisor's last frame, saying it passed and that
    /// its scratch directory was full: it comes back as its output, and its exit is its own.
    #[test]
    fn the_code_cant_forge_the_supervisors_word() {
        let Some(p) = python() else { return };
        let code = "import os, struct, sys\nend = b'{\"status\": 0, \"scratch\": true}'\n\
                    sys.stdout.buffer.write(b'x' + struct.pack('>I', len(end)) + end)\nsys.stdout.flush()\nos._exit(3)";
        let o = contained(&p, code, &limits());
        assert_eq!(o.exit, Exit::Code(3), "{}", stderr(&o));
        assert!(o.stdout.ends_with(br#"{"status": 0, "scratch": true}"#), "{:?}", stdout(&o));
    }

    #[test]
    fn each_limit_ends_the_run() {
        let Some(p) = python() else { return };
        let quick = Limits { cpu_seconds: 1, memory_mb: 128, output_bytes: 64 << 10, wall_seconds: 3, scratch_mb: 8 };
        // A second of CPU can take several of wall time on a loaded machine, so the wall time is generous here.
        let o = contained(&p, "while True: pass", &Limits { wall_seconds: 60, ..quick });
        assert_eq!(o.exit, Exit::Signal(24), "SIGXCPU");

        // The memory limit is the run's own: the scratch directory's share of the container's isn't the code's.
        let o = contained(&p, "b = bytearray(1 << 30)\nfor i in range(0, len(b), 4096): b[i] = 1", &quick);
        assert_eq!(o.exit, Exit::Memory);
        let o =
            contained(&p, "b = bytearray(100 << 20)\nfor i in range(0, len(b), 4096): b[i] = 1\nprint('ok')", &quick);
        assert_eq!((o.exit.clone(), stdout(&o).trim()), (Exit::Code(0), "ok"), "{}", stderr(&o));

        // Exit::Output is reported whenever the output filled its buffer, whichever limit then ended the
        // run (finding 2e8cdbfcea20), so the wall time is 60 s here: the code's own time ending short of it shows
        // the output limit ended it. Its own, not the test's: a loaded machine's slow container start made the
        // test's clock read 38.8 s (task 1346b7694f40).
        let o = contained(
            &p,
            "import sys\nwhile True: sys.stdout.write('x' * 4096)",
            &Limits { wall_seconds: 60, ..quick },
        );
        assert_eq!(o.exit, Exit::Output);
        assert_eq!(o.stdout.len(), 64 << 10, "kept up to the limit");
        assert!(o.ran < Duration::from_secs(60), "{:?}", o.ran);

        // It would sleep 60 s: ending well before that, with Exit::WallTime, is the 3 s wall time's kill.
        let o = contained(&p, "import time; time.sleep(60)", &quick);
        assert_eq!(o.exit, Exit::WallTime);
        assert!(o.ran < Duration::from_secs(30), "{:?}", o.ran);

        // Python ignores SIGXFSZ, so a write past the file size limit fails instead of ending it.
        let roomy = Limits { wall_seconds: 60, ..quick };
        let o = contained(&p, "open('big', 'wb').write(b'x' * (32 << 20))", &Limits { scratch_mb: 64, ..roomy });
        assert!(stderr(&o).contains("File too large"), "{o:?}");

        // The scratch directory's size and its count of files are each limited, and a run that fills either ends
        // as past the scratch limit, whatever it does next.
        let o = contained(&p, "for i in range(16): open(f'f{i}', 'wb').write(b'x' * (1 << 20))", &roomy);
        assert_eq!(o.exit, Exit::Scratch, "{}", stderr(&o));
        let fill = "import errno\ntry:\n    for i in range(16): open(f'f{i}', 'wb').write(b'x' * (1 << 20))\n\
                    except OSError as e: print(errno.errorcode[e.errno], i)\nimport time; time.sleep(1)";
        let o = contained(&p, fill, &roomy);
        assert_eq!((stdout(&o).trim(), o.exit.clone()), ("ENOSPC 8", Exit::Scratch), "{}", stderr(&o));
        let o = contained(&p, "import os\nfor i in range(12000): os.mkdir(f'd{i}')", &roomy);
        assert_eq!(o.exit, Exit::Scratch, "{}", stderr(&o));
        // So do inputs too big for the scratch directory: the code never starts.
        let scratch = Scratch::new().expect("a scratch directory");
        std::fs::write(scratch.dir.join("big"), vec![b'x'; 9 << 20]).expect("an input");
        let o = p.run(&scratch, &["-c".into(), "print('started')".into()], &roomy).expect("ran");
        assert_eq!((stdout(&o).as_str(), o.exit), ("", Exit::Scratch));

        // And processes, however the code starts them.
        let forks = "import os, time\nn = 0\ntry:\n    while n < 500:\n        if os.fork() == 0:\n            time.sleep(30)\n\
                     \x20           os._exit(0)\n        n += 1\nexcept OSError as e:\n    print(n < 64, e.errno)";
        let o = contained(&p, forks, &Limits { wall_seconds: 20, ..quick });
        assert_eq!((stdout(&o).trim(), o.exit.clone()), ("True 11", Exit::Code(0)), "{}", stderr(&o));
    }

    /// Forked children share one CPU limit (finding ee32bfe90b5c): four of them each using 0.9 s, under the
    /// 1 s each process's rlimit allows, end the run as the CPU limit once the container's total passes it.
    #[test]
    fn forked_children_share_one_cpu_limit() {
        let Some(p) = python() else { return };
        let limits = Limits { cpu_seconds: 1, memory_mb: 128, output_bytes: 64 << 10, wall_seconds: 60, scratch_mb: 8 };
        let code = "import os, time\nfor _ in range(4):\n    if os.fork() == 0:\n        end = time.process_time() + 0.9\n\
                    \x20       while time.process_time() < end: pass\n        os._exit(0)\n\
                    for _ in range(4): os.wait()\nprint('done')";
        let o = contained(&p, code, &limits);
        assert_eq!((stdout(&o).as_str(), o.exit.clone()), ("", Exit::Signal(24)), "{}", stderr(&o));
    }

    /// An image isn't pulled at run time (finding 5e407a7db683): one that isn't here fails the run.
    #[test]
    fn an_image_that_isnt_here_isnt_pulled() {
        let Some(p) = python() else { return };
        let digest = format!("sha256:{}", "0".repeat(64));
        let gone = Python { image: format!("python@{digest}"), digest, version: p.version };
        let scratch = Scratch::new().expect("a scratch directory");
        let err = gone.run(&scratch, &["-c".into(), "pass".into()], &limits()).expect_err("refused");
        assert!(err.contains("No such image"), "{err}");
    }

    /// What no file's size shows is bounded too, as the scratch directory is a filesystem of its own of a fixed size
    /// (findings 153f50387cca, 371bba702fb6, PR #185): files unlinked but held open, attributes on them, attributes on
    /// many files and many empty files each end the run as past the scratch limit, with the code's write refused;
    /// files mapped and closed too, held at the limit. A mapped page past it is SIGBUS, which ends the code at once,
    /// and its pages are freed as it ends, so no check may see the directory full: that run ends as the signal.
    /// Attributes on the scratch directory itself are refused outright, as it isn't the code's. The same bytes under
    /// the limit are no trouble.
    #[test]
    fn what_no_file_size_shows_is_bounded() {
        let Some(p) = python() else { return };
        let roomy = Limits { cpu_seconds: 20, memory_mb: 128, output_bytes: 64 << 10, wall_seconds: 60, scratch_mb: 8 };
        let setup = "import errno, mmap, os, time\nheld = []\nvalue = b'x' * (64 << 10)\n";
        let cases = [
            (
                "unlinked files held open",
                "f = open(f'u{i}', 'wb'); os.unlink(f'u{i}'); held.append(f)\nf.write(b'x' * (1 << 20)); f.flush()",
            ),
            (
                "attributes on unlinked files held open",
                "f = open(f'v{i}', 'wb'); os.unlink(f'v{i}'); held.append(f)\nos.setxattr(f.fileno(), 'user.big', value)",
            ),
            ("attributes on many files", "open(f'a{i}', 'wb').close()\nos.setxattr(f'a{i}', 'user.big', value)"),
            ("many empty files", "open(f'e{i}', 'wb').close()"),
        ];
        for (what, piece) in cases {
            let body: String = piece.lines().map(|l| format!("        {l}\n")).collect();
            let code = format!(
                "{setup}try:\n    for i in range(20000):\n{body}except OSError as e:\n    print(errno.errorcode[e.errno], flush=True)\n\
                 else:\n    print('never refused', flush=True)\ntime.sleep(1)"
            );
            let o = contained(&p, &code, &roomy);
            assert_eq!((stdout(&o).trim(), o.exit.clone()), ("ENOSPC", Exit::Scratch), "{what}: {}", stderr(&o));
        }
        // Unlinked files mapped and closed, 32 KiB short of the limit in all, held.
        let map = |size: &str| {
            format!(
                "{setup}for i, n in enumerate([1 << 20] * 7 + [{size}]):\n\
                 \x20   fd = os.open(f'm{{i}}', os.O_RDWR | os.O_CREAT); os.unlink(f'm{{i}}'); os.ftruncate(fd, n)\n\
                 \x20   m = mmap.mmap(fd, n); os.close(fd); m.write(b'x' * n); held.append(m)\n\
                 print('held', flush=True)\ntime.sleep(1)"
            )
        };
        let o = contained(&p, &map("(1 << 20) - (32 << 10)"), &roomy);
        assert_eq!((stdout(&o).trim(), o.exit.clone()), ("held", Exit::Scratch), "{}", stderr(&o));
        let o = contained(&p, &map("2 << 20"), &roomy);
        assert!(matches!(o.exit, Exit::Signal(7) | Exit::Scratch) && stdout(&o).is_empty(), "{o:?}");
        let o = contained(&p, "import os\nos.setxattr('.', 'user.b', b'x')", &roomy);
        assert!(stderr(&o).contains("PermissionError"), "{o:?}");
        let small = "import os\nopen('f', 'wb').write(b'x' * (3 << 20))\nos.setxattr('f', 'user.big', b'x' * (64 << 10))\n\
                     f = open('g', 'wb'); os.unlink('g'); f.write(b'x' * (2 << 20)); f.flush(); print('ok')";
        let o = contained(&p, small, &roomy);
        assert_eq!((stdout(&o).trim(), o.exit.clone()), ("ok", Exit::Code(0)), "{}", stderr(&o));
    }

    /// Containers end, and are removed, however their runs end; the inputs' directory is removed when dropped. The
    /// runs are in a process of their own, the ignored test below, so no other test's containers are seen.
    #[test]
    fn nothing_is_left_behind() {
        if python().is_none() {
            return;
        }
        let exe = std::env::current_exe().expect("the test binary");
        let inner = "python::tests::macos_only::runs_that_end_each_way";
        let child = Command::new(exe)
            .args(["--exact", "--ignored", "--nocapture", "--test-threads", "1", inner])
            .output()
            .expect("ran");
        let text = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success() && text.contains("1 passed"),
            "{text}{}",
            String::from_utf8_lossy(&child.stderr)
        );
        // With one thread, libtest starts the test's own output on its `test … ...` line.
        let pid = text.lines().find_map(|l| l.split("runner ").nth(1)).expect("the runner said its pid");
        let left = docker(&["ps", "-aq", "--filter", &format!("label={LABEL}={pid}")]).expect("docker ps");
        assert_eq!(String::from_utf8_lossy(&left.stdout).trim(), "", "containers left behind");
    }

    #[test]
    #[ignore = "run by nothing_is_left_behind, in a process of its own"]
    fn runs_that_end_each_way() {
        let p = python().expect("Docker");
        println!("runner {}", std::process::id());
        let quick = Limits { cpu_seconds: 1, memory_mb: 64, output_bytes: 4 << 10, wall_seconds: 2, scratch_mb: 8 };
        let scratch = Scratch::new().expect("a scratch directory");
        let dir = scratch.dir.clone();
        for code in ["pass", "import time; time.sleep(60)", "while True: print('x' * 100)", "raise SystemExit(4)"] {
            p.run(&scratch, &["-c".into(), code.into()], &quick).expect("ran");
        }
        let mut running =
            p.start(&scratch, &["-c".into(), "import time; time.sleep(60)".into()], &quick).expect("started");
        running.send("a line").expect("sent");
        assert_eq!(running.finish(), Exit::Signal(9), "ended by finish");
        drop(scratch);
        assert!(!dir.exists(), "{}", dir.display());
    }

    /// A runner killed while its container runs leaves it behind, and the next runner's sweep removes it. The runner
    /// is this test binary, running the ignored test below, killed with SIGKILL. Another test run's sweep at the same
    /// time may remove it first, which is the same sweep: so it's checked as there only while its runner lives.
    #[test]
    fn a_killed_runners_container_is_swept_by_the_next() {
        if python().is_none() {
            return;
        }
        let exe = std::env::current_exe().expect("the test binary");
        let inner = "python::tests::macos_only::holds_a_container_until_killed";
        let mut runner = Command::new(exe)
            .args(["--exact", "--ignored", "--nocapture", "--test-threads", "1", inner])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("started");
        let pid = runner.id();
        let mut lines = std::io::BufReader::new(runner.stdout.take().expect("piped")).lines();
        assert!(
            lines.by_ref().map_while(Result::ok).any(|l| l.ends_with("running")),
            "the runner started its container"
        );
        let theirs = || {
            let out = docker(&["ps", "-aq", "--filter", &format!("label={LABEL}={pid}")]).expect("docker ps");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // Seen while its runner is alive, which no sweep removes.
        assert_ne!(theirs(), "", "there while its runner runs");
        let _ = runner.kill();
        let _ = runner.wait();
        // Once its runner is dead, any process's first container sweeps it: another test run's, at the same time,
        // may already have, or be part way through removing it, so this one's own removal fails and it is still
        // listed for a moment (task b7b88eed51c5). So it sweeps until it's gone, for a while on a loaded machine.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            sweep_now();
            if theirs().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "still there a minute after its runner was killed: {}", theirs());
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    #[ignore = "run by a_killed_runners_container_is_swept_by_the_next, which kills it"]
    fn holds_a_container_until_killed() {
        let p = python().expect("Docker");
        let scratch = Scratch::new().expect("a scratch directory");
        let code = "import time\nprint('running', flush=True)\ntime.sleep(120)";
        let running = p.start(&scratch, &["-c".into(), code.into()], &limits()).expect("started");
        let line = running.next(Instant::now() + Duration::from_secs(60));
        assert_eq!(line.as_deref(), Some("running"));
        println!("running");
        std::thread::sleep(Duration::from_secs(120));
    }

    /// A started child's CPU time is the supervisor's word, a tenth of a second behind at most.
    #[test]
    fn a_started_childs_cpu_time_is_reported() {
        let Some(p) = python() else { return };
        let scratch = Scratch::new().expect("a scratch directory");
        let code = "import sys, time\nfor line in sys.stdin:\n    end = time.process_time() + 0.5\n\
                    \x20   while time.process_time() < end: pass\n    print('spun', flush=True)";
        let mut running = p.start(&scratch, &["-c".into(), code.into()], &limits()).expect("started");
        running.send("spin").expect("sent");
        assert_eq!(running.next(Instant::now() + Duration::from_secs(30)).as_deref(), Some("spun"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while running.cpu().unwrap_or_default() < Duration::from_millis(500) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        assert!(running.cpu().unwrap_or_default() >= Duration::from_millis(500), "{:?}", running.cpu());
        // Its loop ends with its input, which finishing closes.
        assert_eq!(running.finish(), Exit::Code(0));
    }

    /// What a run costs on top of its code's own time, printed: a measurement, run by hand with
    /// `cargo test -p bracken-experiment --lib -- --ignored --nocapture overhead`.
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn overhead() {
        let p = python().expect("Docker");
        let scratch = Scratch::new().expect("a scratch directory");
        let mut times: Vec<Duration> = (0..11)
            .map(|_| {
                let start = Instant::now();
                let o = p.run(&scratch, &["-c".into(), "pass".into()], &limits()).expect("ran");
                assert_eq!(o.exit, Exit::Code(0));
                start.elapsed()
            })
            .collect();
        let first = times.remove(0);
        times.sort();
        println!("first {first:?}, then min {:?}, median {:?}, max {:?}", times[0], times[5], times[9]);
    }

    /// The fixture's public test passes; a tree with a failing, an erroring and an unimportable test says so.
    #[test]
    fn run_tests_reports_each_test() {
        let Some(python) = python() else { return };
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny");
        let project = Project::load(&fixture).expect("the fixture");
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
