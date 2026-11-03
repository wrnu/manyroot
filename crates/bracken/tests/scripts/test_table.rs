//! scripts/test-table. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// test-table runs exactly the table, in order, from the checkout's top, and stops at the first
/// failure with its exit status non-zero. The tests run under nextest, then the doctests under cargo test
/// (decision `4bf44934cb67` (4)), after a check that nextest is there. A `cargo` stub logs each call, and a `ground-guards` stub its
/// arguments.
#[test]
fn test_table_runs_the_whole_table_in_order_and_stops_at_a_failure() {
    for scripts in SCRIPTS {
        let (table, scope) =
            (repo_root().join(scripts).join("test-table"), repo_root().join(scripts).join("code-scope"));
        let files = [("scripts/test-table", table.as_path()), ("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            write_executable(&dir.join("scripts/ground-guards"), "#!/bin/sh\necho \"ground-guards $*\" >>\"$LOG\"\n");
            std::fs::create_dir_all(dir.join("sub")).unwrap();
            std::fs::write(dir.join("sub/.keep"), "").unwrap();
        });
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(
            &stubs.join("cargo"),
            "#!/bin/sh\necho \"cargo $*\" >>\"$LOG\"\n[ -z \"${BRACKEN_FULL_CHECK+x}\" ] || echo 'BRACKEN_FULL_CHECK reached cargo' >>\"$LOG\"\n\
             case \"$*\" in *\"$FAIL_ON\"*) [ -n \"$FAIL_ON\" ] && exit 3 ;; esac\nexit 0\n",
        );
        let log = tmp.path().join("log");
        // A caller's BRACKEN_FULL_CHECK never reaches the tests (finding 607273d1d193): the calls below
        // would hold a line saying so.
        let table_run = |fail_on: &str| {
            let _ = std::fs::remove_file(&log);
            let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
            let out = command(&dir.join("sub"), "../scripts/test-table", &[])
                .env("PATH", path)
                .env("LOG", &log)
                .env("FAIL_ON", fail_on)
                .env("BRACKEN_FULL_CHECK", "1")
                .output()
                .unwrap();
            (out, std::fs::read_to_string(&log).unwrap_or_default())
        };

        let (out, calls) = table_run("");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            calls,
            "cargo nextest --version\n\
             cargo fmt --check\n\
             cargo clippy --locked --all-targets -- -D warnings\n\
             cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings\n\
             cargo nextest run --locked --workspace --no-fail-fast\n\
             cargo test --locked --doc --workspace\n\
             ground-guards origin/main HEAD\n",
            "{scripts}"
        );

        let (out, calls) = table_run("--target x86_64");
        assert!(!out.status.success(), "{scripts}: a failing step passed");
        assert_eq!(calls.lines().count(), 4, "{scripts}: went on after a failure: {calls}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("failed: cargo clippy"), "{scripts}");

        // Without cargo-nextest the table stops before its first step, saying what it needs.
        let (out, calls) = table_run("nextest --version");
        assert!(!out.status.success(), "{scripts}: ran without nextest");
        assert_eq!(calls, "cargo nextest --version\n", "{scripts}: went on without nextest");
        assert!(String::from_utf8_lossy(&out.stderr).contains("needs cargo-nextest"), "{scripts}");

        // HEAD at origin/main (verify-seq's main alone): the whole table, but the Ground guards, which
        // would have nothing to compare, are skipped, saying so (task 1e89b53c6a4a). A head past it runs them.
        assert!(run(&dir, "git", &["update-ref", "refs/remotes/origin/main", "HEAD"]).status.success());
        let (out, calls) = table_run("");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!calls.contains("ground-guards") && calls.contains("cargo test --locked --doc"), "{scripts}: {calls}");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("HEAD is origin/main: skipped the Ground guards"), "{scripts}: {said}");
        put(&dir, "code.txt", "a change\n");
        commit_all(&dir, "code");
        let (out, calls) = table_run("");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(calls.ends_with("ground-guards origin/main HEAD\n"), "{scripts}: {calls}");
    }
}

/// `test-table --light -p <package>...` (decision `464d3ccca45e`), with a `cargo` stub that logs each call and
/// knows two packages, one with a library: fmt, both clippies, nextest of exactly those packages, the
/// doctests of those with a library only, then the Ground guards. With TEST_TABLE_MODE set it ends with
/// the counted run's mode, on a pass and on a failing step. A package the workspace lacks is refused
/// before any check runs.
#[test]
fn test_table_light_runs_the_named_packages_and_the_guards() {
    for scripts in SCRIPTS {
        let (table, scope) =
            (repo_root().join(scripts).join("test-table"), repo_root().join(scripts).join("code-scope"));
        let files = [("scripts/test-table", table.as_path()), ("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            write_executable(&dir.join("scripts/ground-guards"), "#!/bin/sh\necho \"ground-guards $*\" >>\"$LOG\"\n");
        });
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        let meta = r#"{"packages":[{"name":"lib-a","targets":[{"kind":["lib"]},{"kind":["test"]}]},{"name":"bin-b","targets":[{"kind":["bin"]}]}]}"#;
        write_executable(
            &stubs.join("cargo"),
            &format!(
                "#!/bin/sh\necho \"cargo $*\" >>\"$LOG\"\ncase \"$*\" in metadata*) echo '{meta}' ;; esac\n\
                 case \"$*\" in *\"$FAIL_ON\"*) [ -n \"$FAIL_ON\" ] && exit 3 ;; esac\nexit 0\n"
            ),
        );
        let log = tmp.path().join("log");
        let light = |args: &[&str], fail_on: &str| {
            let _ = std::fs::remove_file(&log);
            let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
            let mut all = vec!["--light"];
            all.extend_from_slice(args);
            let out = command(&dir, "scripts/test-table", &all)
                .env("PATH", path)
                .env("LOG", &log)
                .env("FAIL_ON", fail_on)
                .env("TEST_TABLE_MODE", "light: the builder's passes")
                .output()
                .unwrap();
            (out, std::fs::read_to_string(&log).unwrap_or_default())
        };
        let start = "cargo nextest --version\ncargo metadata --locked --no-deps --format-version 1\n";
        let checks = "cargo fmt --check\n\
                      cargo clippy --locked --all-targets -- -D warnings\n\
                      cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings\n";

        let (out, calls) = light(&["-p", "lib-a", "-p", "bin-b"], "");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            calls,
            format!(
                "{start}{checks}cargo nextest run --locked --no-fail-fast -p lib-a -p bin-b\n\
                 cargo test --locked --doc -p lib-a\nground-guards origin/main HEAD\n"
            ),
            "{scripts}"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("light run passed at") && stdout.contains("not the whole table"),
            "{scripts}: {stdout}"
        );
        assert!(
            stdout.trim_end().ends_with("test-table: counted run mode: light: the builder's passes"),
            "{scripts}: {stdout}"
        );

        // A binary-only scope has no doctests to run, and says so.
        let (out, calls) = light(&["-p", "bin-b"], "");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!calls.contains("--doc") && String::from_utf8_lossy(&out.stdout).contains("no doctests"), "{scripts}");

        // A failing step stops the run, and the mode still ends the output.
        let (out, calls) = light(&["-p", "lib-a"], "nextest run");
        assert!(!out.status.success(), "{scripts}: a failing step passed");
        assert!(!calls.contains("--doc") && !calls.contains("ground-guards"), "{scripts}: went on: {calls}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("failed: ") && stderr.contains("counted run mode: light"), "{scripts}: {stderr}");

        // A package the workspace lacks, or no package, is refused before any check.
        let (out, calls) = light(&["-p", "lib-a", "-p", "nope"], "");
        assert!(!out.status.success(), "{scripts}: an unknown package ran");
        assert!(String::from_utf8_lossy(&out.stderr).contains("no package nope"), "{scripts}");
        assert_eq!(calls, start, "{scripts}: ran a check for an unknown package");
        for args in [&[][..], &["lib-a"], &["-p", "--workspace"]] {
            let (out, calls) = light(args, "");
            assert!(!out.status.success() && calls.is_empty(), "{scripts}: {args:?} ran: {calls}");
        }
    }
}

/// `test-table --remote` (task `489bfeb6b5af`), with a stub `boat`: it asks for one default box with
/// `--no-env`, no `-e`, a 2400 s limit and nothing pushed but a bundle of HEAD and origin/main; it
/// prints the box's cost line and exits with the box's exit. A box boat leaves listed is deleted on
/// every way out: a failed run and an interrupted one. A dirty tree is refused before any box.
#[test]
fn test_table_remote_runs_one_box_and_leaves_none_behind() {
    for scripts in SCRIPTS {
        let table = repo_root().join(scripts).join("test-table");
        let (tmp, dir) = repo(&[("scripts/test-table", &table)], ".bracken/\n", |_| {});
        assert!(run(&dir, "git", &["update-ref", "refs/remotes/origin/main", "HEAD"]).status.success());
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // run: records its arguments and what was pushed, keeps the bundle, says which box (and touches
        // $T/ready once it has), then sleeps
        // $SLEEP seconds and exits $EXIT. list: logs its arguments and, like boat, shows the box only
        // while it's running unless given --all; $T/left holds its state while it exists. delete: logs it.
        // Its output also names another box, as the table's own tests do: that one is never deleted.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s\\n' \"$@\" >\"$T/args\"\n\
               while [ $# -gt 0 ]; do [ \"$1\" = --push ] && push=$2; shift; done\n\
               ls -A \"$push\" >\"$T/pushed\"; cp \"$push/tree.bundle\" \"$T/\"\n\
               echo 'bx_stub01 ready in 1.0s'; echo 'test-table: deleting box bx_other99'; touch \"$T/ready\"\n\
               sleep \"${SLEEP:-0}\"\n\
               echo \"exit $EXIT · 1m 00s · default · \\$0.0010 · bx_stub01 deleted\"; exit \"$EXIT\" ;;\n\
             list) echo \"$*\" >>\"$T/listed\"\n\
               if [ -e \"$T/left\" ] && { [ \"$(cat \"$T/left\")\" = running ] || [[ \" $* \" == *' --all '* ]]; }; then\n\
                 echo '{\"sandboxes\":[{\"id\": \"bx_stub01\"}]}'; else echo '{\"sandboxes\":[]}'; fi ;;\n\
             delete) echo \"$*\" >>\"$T/deleted\"; rm -f \"$T/left\" ;;\n\
             esac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let remote = |exit: &str, sleep: &str| {
            let mut c = command(&dir, "scripts/test-table", &["--remote"]);
            c.env("PATH", &path).env("T", tmp.path()).env("EXIT", exit).env("SLEEP", sleep);
            c
        };
        let read = |name: &str| std::fs::read_to_string(tmp.path().join(name)).unwrap_or_default();

        let out = remote("0", "0").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains("test-table: box: exit 0 · 1m 00s · default · $0.0010"), "{scripts}: {stdout}");
        let args: Vec<String> = read("args").lines().map(String::from).collect();
        for want in ["--no-env", "--type", "default", "--timeout", "2400", "--push"] {
            assert!(args.iter().any(|a| a == want), "{scripts}: no {want} in {args:?}");
        }
        assert!(!args.iter().any(|a| a == "-e" || a.starts_with("--env")), "{scripts}: {args:?}");
        assert_eq!(read("pushed"), "tree.bundle\n", "{scripts}: pushed more than the bundle");
        let bundle = tmp.path().join("tree.bundle");
        let heads = run(&dir, "git", &["bundle", "list-heads", bundle.to_str().unwrap()]);
        let heads = String::from_utf8_lossy(&heads.stdout);
        assert!(heads.contains(" HEAD\n") && heads.contains(" refs/remotes/origin/main\n"), "{scripts}: {heads}");
        assert!(read("deleted").is_empty(), "{scripts}: boat had deleted its box");

        // A failed run that leaves its box stopped, which plain boat list doesn't show: test-table exits
        // with the box's exit, and deletes it.
        std::fs::write(tmp.path().join("left"), "stopped").unwrap();
        let out = remote("3", "0").output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("exited 3"), "{scripts}");
        assert!(read("listed").lines().all(|l| l.contains("--all")), "{scripts}: {}", read("listed"));
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left");
        assert!(!tmp.path().join("left").exists());

        // Interrupted mid-run (Ctrl-C reaches the whole process group): the box is deleted too, without
        // waiting for the box's 10 minutes. The bounds count from the interrupt and leave headroom for a
        // loaded machine (note 300ac9b6a423: a load average near 300 broke a 25 s bound on the whole run).
        std::fs::remove_file(tmp.path().join("deleted")).unwrap();
        std::fs::remove_file(tmp.path().join("ready")).unwrap();
        std::fs::write(tmp.path().join("left"), "running").unwrap();
        let mut child = remote("0", "600");
        std::os::unix::process::CommandExt::process_group(&mut child, 0);
        let mut child = child.spawn().unwrap();
        let started = std::time::Instant::now();
        while !tmp.path().join("ready").exists() {
            assert!(started.elapsed() < std::time::Duration::from_secs(180), "{scripts}: boat never ran");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let group = format!("-{}", child.id());
        assert!(Command::new("/bin/kill").args(["-INT", "--", &group]).status().unwrap().success());
        let interrupted = std::time::Instant::now();
        let status = child.wait().unwrap();
        assert!(!status.success(), "{scripts}: an interrupted run passed");
        let after = interrupted.elapsed();
        assert!(after < std::time::Duration::from_secs(120), "{scripts}: waited {after:?} for the box");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the interrupted box was left");

        // A dirty tree: refused, no box.
        std::fs::remove_file(tmp.path().join("args")).unwrap();
        std::fs::write(dir.join("dirty"), "x").unwrap();
        let out = remote("0", "0").output().unwrap();
        assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("commit"), "{scripts}");
        assert!(read("args").is_empty(), "{scripts}: a box for a dirty tree");
    }
}

/// `test-table --remote -p <package>...` (task `57c9181daa96`, decision `2a9e444218d6`), with a stub `boat`:
/// the box gets the same setup but runs only `cargo test --locked -p ...` for the packages named, never the
/// table, and says it wasn't the whole table. A name that isn't a package name, a `-p` without one, or
/// anything else after `--remote` is refused before any box.
#[test]
fn test_table_remote_p_runs_only_the_named_packages_tests() {
    for scripts in SCRIPTS {
        let table = repo_root().join(scripts).join("test-table");
        let (tmp, dir) = repo(&[("scripts/test-table", &table)], ".bracken/\n", |_| {});
        assert!(run(&dir, "git", &["update-ref", "refs/remotes/origin/main", "HEAD"]).status.success());
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // run: records its arguments, one a line except the box's script, the last, kept whole in $T/inner.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s\\n' \"${@:1:$#-1}\" >\"$T/args\"; printf '%s' \"${!#}\" >\"$T/inner\"\n\
               echo 'bx_stub01 ready in 1.0s'; echo 'exit 0 · 1m 00s · default · $0.0010 · bx_stub01 deleted' ;;\n\
             list) echo '{\"sandboxes\":[]}' ;;\n\
             esac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let remote = |args: &[&str]| {
            let _ = std::fs::remove_file(tmp.path().join("inner"));
            let mut c = command(&dir, "scripts/test-table", args);
            c.env("PATH", &path).env("T", tmp.path());
            c.output().unwrap()
        };
        let inner = || std::fs::read_to_string(tmp.path().join("inner")).unwrap_or_default();

        let out = remote(&["--remote", "-p", "bracken", "-p", "bracken-tree"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let script = inner();
        assert!(script.ends_with("\ncargo test --locked -p bracken -p bracken-tree"), "{scripts}: {script}");
        assert!(script.contains("git checkout -q tested"), "{scripts}: the same setup: {script}");
        // Every crate the host's graph needs, before any test (task 53efd11c6e4a).
        let fetch = "setup cargo fetch --locked --target \"$(rustc -vV | sed -n \"s/^host: //p\")\"";
        assert!(script.lines().any(|l| l == fetch), "{scripts}: the box doesn't fetch first: {script}");
        assert!(!script.contains("scripts/test-table"), "{scripts}: the box ran the table: {script}");
        assert!(stdout.contains("cargo test --locked -p bracken -p bracken-tree passed at "), "{scripts}: {stdout}");
        assert!(stdout.contains("in a box; not the whole table"), "{scripts}: {stdout}");

        // Without -p, the box runs the table as before.
        let out = remote(&["--remote"]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(inner().ends_with("\nBRACKEN_BIN=\"$PWD/target/debug/bracken\" scripts/test-table"), "{scripts}");

        for bad in [
            &["--remote", "-p", "bracken; touch $T/ran"][..],
            &["--remote", "-p", "-p"],
            &["--remote", "-p"],
            &["--remote", "bracken"],
        ] {
            let out = remote(bad);
            assert!(!out.status.success(), "{scripts}: {bad:?} passed");
            assert!(inner().is_empty(), "{scripts}: {bad:?} started a box");
            assert!(!tmp.path().join("ran").exists(), "{scripts}: {bad:?} ran in the shell");
        }
    }
}

/// The box's nextest (decision `4bf44934cb67` (4), answer `92eacabfd720`): the script test-table sends a box
/// downloads the pinned release and checks it against the sha256 in test-table before unpacking it, and a
/// tarball that doesn't match stops the box before it unpacks anything or checks out the tree. The script
/// runs here with stubs for the box's system tools: `curl` writes the payload asked for, `sha256sum` checks
/// with this Mac's shasum, and `tar` and `git` log that they ran.
#[test]
fn test_table_box_refuses_a_nextest_tarball_that_doesnt_match_its_pin() {
    for scripts in SCRIPTS {
        let table = repo_root().join(scripts).join("test-table");
        let (tmp, dir) = repo(&[("scripts/test-table", &table)], ".bracken/\n", |_| {});
        assert!(run(&dir, "git", &["update-ref", "refs/remotes/origin/main", "HEAD"]).status.success());
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // boat run: keeps the box's script, its last argument, whole in $T/inner.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s' \"${!#}\" >\"$T/inner\"; echo 'exit 0 · 1m 00s · default · $0.0010 · bx_stub01 deleted' ;;\n\
             list) echo '{\"sandboxes\":[]}' ;;\n\
             esac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let out = command(&dir, "scripts/test-table", &["--remote"]).env("PATH", &path).env("T", tmp.path()).output();
        assert!(out.unwrap().status.success(), "{scripts}");
        let inner = std::fs::read_to_string(tmp.path().join("inner")).unwrap();
        let pinned = "b0959bc64232974224cf9fb22c915845398c0e8668fa7831eb5b43fb1eef781f";
        assert!(inner.starts_with(&format!("nextest_version=0.9.150 nextest_sha256={pinned}\n")), "{scripts}: {inner}");

        // The box's tools, for running its script here.
        let tools = tmp.path().join("box-tools");
        std::fs::create_dir_all(&tools).unwrap();
        for (name, body) in [
            ("sudo", "exec \"$@\""),
            ("add-apt-repository", "exit 0"),
            ("apt-get", "exit 0"),
            ("curl", "while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\nprintf %s \"$PAYLOAD\" >\"$out\""),
            (
                "sha256sum",
                "[ -z \"$SHA_OK\" ] || exit 0\nread -r want file\n[ \"$(shasum -a 256 \"$file\" | cut -d' ' -f1)\" = \"$want\" ]",
            ),
            ("tar", "echo tar >>\"$T/ran\""),
            ("git", "[ \"$1\" = --version ] && { echo 'git version 2.56.0'; exit 0; }\necho git >>\"$T/ran\""),
            ("timeout", "shift; exec \"$@\""),
        ] {
            write_executable(&tools.join(name), &format!("#!/bin/bash\n{body}\n"));
        }
        let boxed = tmp.path().join("box");
        std::fs::create_dir_all(&boxed).unwrap();
        let boxed_run = |sha_ok: &str| {
            let _ = std::fs::remove_file(tmp.path().join("ran"));
            let out = Command::new("/bin/bash")
                .args(["-c", &inner])
                .current_dir(&boxed)
                .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
                .env("T", tmp.path())
                .env("PAYLOAD", "not the pinned release")
                .env("SHA_OK", sha_ok)
                .output()
                .unwrap();
            (out, std::fs::read_to_string(tmp.path().join("ran")).unwrap_or_default())
        };
        // The control: a check that passes goes on to unpack, so the stubs would show a run that went on.
        let (_, ran) = boxed_run("1");
        assert!(ran.starts_with("tar\n"), "{scripts}: the control didn't unpack: {ran}");

        let (out, ran) = boxed_run("");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("does not match its pinned sha256; not running it"), "{scripts}: {stderr}");
        assert!(ran.is_empty(), "{scripts}: went on after a mismatch: {ran}");
    }
}

/// Task 95302a01f49a: the box's git-core PPA, which Launchpad sometimes times out on. test-table's box
/// script runs here with stubs for the box's tools. `add-apt-repository` fails the first `$PPA_FAILS`
/// times; `git --version` says `$IMAGE_GIT` until the PPA's git is installed, then 2.56. Adding the PPA is
/// tried four times and setup goes on once it works; if it never does, the image's git is used only when
/// it's 2.46 or later, and otherwise setup fails, saying why. A git still too old after install fails too.
#[test]
fn test_table_box_retries_the_git_ppa_and_falls_back_only_to_a_new_enough_git() {
    for scripts in SCRIPTS {
        let table = repo_root().join(scripts).join("test-table");
        let (tmp, dir) = repo(&[("scripts/test-table", &table)], ".bracken/\n", |_| {});
        assert!(run(&dir, "git", &["update-ref", "refs/remotes/origin/main", "HEAD"]).status.success());
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\ncase \"$1\" in\nrun) printf '%s' \"${!#}\" >\"$T/inner\"; echo 'exit 0 · 1m 00s · default · $0.0010 · bx_stub01 deleted' ;;\nlist) echo '{\"sandboxes\":[]}' ;;\nesac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let out = command(&dir, "scripts/test-table", &["--remote"]).env("PATH", &path).env("T", tmp.path()).output();
        assert!(out.unwrap().status.success(), "{scripts}");
        let inner = std::fs::read_to_string(tmp.path().join("inner")).unwrap();

        let tools = tmp.path().join("box-tools");
        std::fs::create_dir_all(&tools).unwrap();
        for (name, body) in [
            ("sudo", "exec \"$@\""),
            ("timeout", "[ \"$1\" = 120 ] || exit 9\nshift; UNDER_TIMEOUT=120 exec \"$@\""),
            (
                "add-apt-repository",
                "[ \"$UNDER_TIMEOUT\" = 120 ] || { echo 'not run under timeout 120'; exit 7; }\n\
                 n=$(cat \"$T/tries\" 2>/dev/null || echo 0); echo $((n + 1)) >\"$T/tries\"\n\
                 [ \"$n\" -ge \"$PPA_FAILS\" ] || { echo 'TimeoutError: [Errno 110] Connection timed out'; exit 1; }",
            ),
            ("apt-get", "echo \"apt-get $*\" >>\"$T/ran\"\ncase \"$*\" in *' git '*) touch \"$T/ppa-git\" ;; esac"),
            (
                "git",
                "[ \"$1\" = --version ] || exit 0\nif [ -e \"$T/ppa-git\" ]; then echo \"git version ${PPA_GIT:-2.56.0}\"; else echo \"git version $IMAGE_GIT\"; fi",
            ),
            ("curl", "while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n: >\"$out\""),
            ("sha256sum", "exit 0"),
            ("tar", "echo tar >>\"$T/ran\""),
            ("sleep", "echo \"sleep $1\" >>\"$T/ran\""),
        ] {
            write_executable(&tools.join(name), &format!("#!/bin/bash\n{body}\n"));
        }
        let boxed = tmp.path().join("box");
        std::fs::create_dir_all(&boxed).unwrap();
        let boxed_run = |fails: &str, image: &str, ppa_git: &str| {
            for f in ["ran", "tries", "ppa-git"] {
                let _ = std::fs::remove_file(tmp.path().join(f));
            }
            let out = Command::new("/bin/bash")
                .args(["-c", &inner])
                .current_dir(&boxed)
                .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
                .env("T", tmp.path())
                .env("PPA_FAILS", fails)
                .env("IMAGE_GIT", image)
                .env("PPA_GIT", ppa_git)
                .output()
                .unwrap();
            let ran = std::fs::read_to_string(tmp.path().join("ran")).unwrap_or_default();
            let tries = std::fs::read_to_string(tmp.path().join("tries")).unwrap_or_default().trim().to_owned();
            (out, ran, tries)
        };

        // Two failures, then the PPA: it waits 10 s and 30 s, installs the PPA's git, and goes on.
        let (out, ran, tries) = boxed_run("2", "2.43.0", "");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(tries, "3", "{scripts}: {stderr}");
        assert!(ran.starts_with("sleep 10\nsleep 30\napt-get install"), "{scripts}: {ran}");
        assert!(ran.contains(" git b3sum ") && ran.contains("\ntar\n"), "{scripts}: went on: {ran}");
        assert!(stderr.contains("TimeoutError") && stderr.contains("trying again in 10s"), "{scripts}: {stderr}");
        assert!(!stderr.contains("box setup failed"), "{scripts}: {stderr}");

        // Launchpad down for all four tries, with today's image git (2.43): setup fails, saying why, and
        // installs nothing.
        let (out, ran, tries) = boxed_run("9", "2.43.0", "");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!((out.status.code(), tries.as_str()), (Some(1), "4"), "{scripts}: {stderr}");
        assert!(
            stderr.contains("box setup failed: couldn't add ppa:git-core/ppa after 4 tries")
                && stderr.contains("git version 2.43.0 is older than the 2.46"),
            "{scripts}: {stderr}"
        );
        assert_eq!(ran, "sleep 10\nsleep 30\nsleep 90\n", "{scripts}: went on: {ran}");

        // Down for all four, with an image git of 2.46 or later: it says so, uses it, and goes on.
        for image in ["2.46.0", "2.50.1", "3.0"] {
            let (out, ran, _) = boxed_run("9", image, "");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!stderr.contains("box setup failed"), "{scripts}: {image}: {stderr}");
            assert!(stderr.contains(&format!("using the image's git version {image}")), "{scripts}: {stderr}");
            assert!(ran.contains("apt-get install") && !ran.contains(" git b3sum"), "{scripts}: {ran}");
            assert!(ran.contains("\ntar\n"), "{scripts}: {image}: went on: {ran}");
        }

        // The PPA works but installs a git too old: setup fails, saying so.
        let (out, ran, _) = boxed_run("0", "2.43.0", "2.45.2");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("box setup failed: git version 2.45.2 is older than the 2.46"), "{scripts}: {stderr}");
        assert!(!ran.contains("tar"), "{scripts}: went on: {ran}");
    }
}

/// Finding b8b6193329db: the git version test-table's box needs is the experiment's MIN_GIT, the one
/// number in crates/experiment/src/git_mode.rs, and the box script's comparison reads only `need_git`.
#[test]
fn test_table_box_needs_the_experiments_min_git() {
    let source = std::fs::read_to_string(repo_root().join("crates/experiment/src/git_mode.rs")).unwrap();
    let line = source.lines().find(|l| l.starts_with("pub const MIN_GIT: (u32, u32) = (")).expect("MIN_GIT");
    let pair = line.trim_start_matches("pub const MIN_GIT: (u32, u32) = (").trim_end_matches(");");
    let (major, minor) = pair.split_once(", ").expect("two numbers");
    let (major, minor): (u32, u32) = (major.parse().unwrap(), minor.parse().unwrap());
    for scripts in SCRIPTS {
        let table = std::fs::read_to_string(repo_root().join(scripts).join("test-table")).unwrap();
        let set: Vec<&str> = table.lines().filter(|l| l.starts_with("need_git=")).collect();
        assert_eq!(set, [format!("need_git={major}.{minor}")], "{scripts}: test-table's need_git is MIN_GIT");
        assert!(
            table.contains(r#"[ "$minor" -ge "${need_git#*.}" ]"#) && !table.contains(&format!("-ge {minor} ]")),
            "{scripts}: the comparison reads need_git, not a number of its own"
        );
    }
}

/// test-table on a docs-only change (owner decision 642a47c04a10): when everything the head changed since
/// it left origin/main is *.md outside crates/, only fmt and the Ground guards run, and it says so. A change
/// that also touches code, renames or deletes a file that isn't docs, touches a .md under crates/ (the
/// code compiles some in), or changes nothing runs the whole table.
#[test]
fn test_table_skips_clippy_and_cargo_test_only_for_a_docs_only_change() {
    let full = "cargo nextest --version\n\
                cargo fmt --check\n\
                cargo clippy --locked --all-targets -- -D warnings\n\
                cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings\n\
                cargo nextest run --locked --workspace --no-fail-fast\n\
                cargo test --locked --doc --workspace\n\
                ground-guards origin/main HEAD\n";
    let docs = "cargo fmt --check\nground-guards origin/main HEAD\n";
    let git = |dir: &Path, args: &[&str]| assert!(run(dir, "git", args).status.success(), "git {args:?}");
    type Change = fn(&Path);
    let cases: [(&str, Change, &str); 7] = [
        ("a doc", |d| put(d, "docs/guide.md", "more\n"), docs),
        ("a new top-level doc", |d| put(d, "NOTES.md", "new\n"), docs),
        (
            "a doc and code",
            |d| {
                put(d, "docs/guide.md", "more\n");
                put(d, "code.txt", "more\n")
            },
            full,
        ),
        ("code renamed to a doc", |d| assert!(run(d, "git", &["mv", "code.txt", "code.md"]).status.success()), full),
        ("code deleted", |d| assert!(run(d, "git", &["rm", "-q", "code.txt"]).status.success()), full),
        ("a .md under crates/", |d| put(d, "crates/p/prompt.md", "changed\n"), full),
        ("no change", |_| {}, full),
    ];
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (table, scope) = (file("test-table"), file("code-scope"));
        for (case, change, want) in cases {
            let files = [("scripts/test-table", table.as_path()), ("scripts/code-scope", scope.as_path())];
            let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
                write_executable(
                    &dir.join("scripts/ground-guards"),
                    "#!/bin/sh\necho \"ground-guards $*\" >>\"$LOG\"\n",
                );
                put(dir, "code.txt", "code\n");
                put(dir, "docs/guide.md", "a guide\n");
                put(dir, "crates/p/prompt.md", "a prompt\n");
            });
            git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
            git(&dir, &["switch", "-qc", "pr"]);
            change(&dir);
            git(&dir, &["add", "-A"]);
            git(&dir, &["commit", "-q", "--allow-empty", "-m", case]);
            let stubs = tmp.path().join("stubs");
            std::fs::create_dir_all(&stubs).unwrap();
            write_executable(&stubs.join("cargo"), "#!/bin/sh\necho \"cargo $*\" >>\"$LOG\"\n");
            let log = tmp.path().join("log");
            let out = command(&dir, "scripts/test-table", &[])
                .env("PATH", format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap()))
                .env("LOG", &log)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{scripts}: {case}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(std::fs::read_to_string(&log).unwrap(), want, "{scripts}: {case}: {stdout}");
            let said = stdout.lines().any(|l| l == "test-table: docs-only: skipped clippy and cargo test");
            assert_eq!(said, want == docs, "{scripts}: {case}: {stdout}");
        }
    }
}

/// `test-table --split` (owner decision `01a2d668b67e`), with a stub `boat` and `cargo`: the whole table in
/// one box, as --remote runs it, and here only the macos_only tests, under nextest; it passes only if both
/// do, and ends with both parts' last lines. Which tests need macOS comes from the code: a test that runs
/// here but is missing or ignored on Linux, by the two sides' lists (nextest's, and cargo test --doc's for
/// doctests), must be in a macos_only module. The box's lists come from the real test-table --lists, with a
/// cargo that writes its own lines on standard error in the middle of its list's lines, as cargo nextest
/// list's build output can land (task 06d7b4ef2c75): they arrive whole, and a list damaged on the way
/// fails as damaged.
/// A failure here stops the box and deletes it; a docs-only head uses no box. With -p <package>... (the
/// light counted run, finding 94e07ed7dd6b) the box runs test-table --light and --lists for those packages,
/// and here only their macOS-only tests and lists, the doctests' for those with a library.
#[test]
fn test_table_split_runs_the_table_in_a_box_and_the_macos_tests_here() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (table, scope) = (file("test-table"), file("code-scope"));
        let files = [("scripts/test-table", table.as_path()), ("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            write_executable(&dir.join("scripts/ground-guards"), "#!/bin/sh\necho \"ground-guards $*\" >>\"$LOG\"\n");
            put(dir, "code.txt", "code\n");
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?}");
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["switch", "-qc", "pr"]);
        put(&dir, "code.txt", "a change\n");
        commit_all(&dir, "code");
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // boat run: as in the --remote test, then the box's lists ($T/linux), then $SLEEP seconds and $EXIT.
        // With $BOX_PATH, the lists come instead from the box script's last line, the --lists command the
        // run asked for, run in $REPO with the box's cargo: the real test-table --lists (fail 40550d3c989e).
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s\\n' \"$@\" >\"$T/args\"\n\
               echo 'bx_stub01 ready in 1.0s'; touch \"$T/ready\"; echo 'test-table: all passed at x, in a box'\n\
               if [ -n \"$BOX_PATH\" ]; then\n\
                 last=$(printf '%s\\n' \"${!#}\" | tail -n 1); printf '%s\\n' \"$last\" >\"$T/ran-lists\"\n\
                 (cd \"$REPO\" && PATH=\"$BOX_PATH\" bash -c \"$last\") || { echo \"exit 9 · 5m 00s · default · \\$0.0030 · bx_stub01 deleted\"; exit 9; }\n\
               elif [ -e \"$T/linux\" ]; then cat \"$T/linux\"; fi\n\
               sleep \"${SLEEP:-0}\"\n\
               echo \"exit $EXIT · 5m 00s · default · \\$0.0030 · bx_stub01 deleted\"; exit \"$EXIT\" ;;\n\
             list) if [ -e \"$T/left\" ]; then echo '{\"sandboxes\":[{\"id\": \"bx_stub01\"}]}'; else echo '{\"sandboxes\":[]}'; fi ;;\n\
             delete) echo \"$*\" >>\"$T/deleted\"; rm -f \"$T/left\" ;;\n\
             esac\n",
        );
        // cargo: says nextest is there, unless $NO_NEXTEST; logs its other arguments; prints this Mac's lists for nextest list
        // ($T/mac-{all,ignored}.nextest) and for cargo test --doc's --list ($T/mac-{all,ignored}.doc), or with
        // $HANG_LIST hangs in them; the filtered run waits for the box to start, then passes, or fails with
        // $FAIL_HERE. With $HANG its child hangs; with $SURVIVE it leaves a process in timeout's group; with
        // $IGNORE_TERM its child ignores SIGTERM.
        write_executable(
            &stubs.join("cargo"),
            "#!/bin/bash\n[ \"$*\" != 'nextest --version' ] || exit \"${NO_NEXTEST:-0}\"\necho \"cargo $*\" >>\"$LOG\"\n\
             if [ -n \"$HANG_LIST\" ] && [[ $* == *list* ]]; then touch \"$T/listing\"; sleep 600 & echo $! >>\"$T/pids\"; wait; fi\n\
             case \"$*\" in\n\
             metadata*) echo '{\"packages\":[{\"name\":\"demo\",\"targets\":[{\"kind\":[\"lib\"]}]},{\"name\":\"agent\",\"targets\":[{\"kind\":[\"bin\"]}]}]}' ;;\n\
             *'nextest list'*'--run-ignored only'*) cat \"$T/mac-ignored.nextest\" ;;\n\
             *'nextest list'*) cat \"$T/mac-all.nextest\" ;;\n\
             *'--list --ignored'*) cat \"$T/mac-ignored.doc\" ;;\n\
             *--list*) cat \"$T/mac-all.doc\" ;;\n\
             *macos_only::*) while [ ! -e \"$T/ready\" ]; do sleep 0.1; done\n\
               if [ -n \"$HANG\" ]; then sleep 600 & echo $! >>\"$T/pids\"; wait; fi\n\
               if [ -n \"$IGNORE_TERM\" ]; then (trap '' TERM; exec sleep 600) & echo $! >>\"$T/pids\"; touch \"$T/ignoring\"; wait; fi\n\
               if [ -n \"$SURVIVE\" ]; then sleep 60 >/dev/null 2>&1 & echo $! >\"$T/survivor\"; fi\n\
               echo '     Summary [   1.000s] 1 test run: 1 passed, 0 skipped'; [ -z \"$FAIL_HERE\" ] || exit \"$FAIL_HERE\" ;;\n\
             esac\n",
        );
        // The box's cargo, for test-table --lists: prints the box's lists ($T/linux-{all,ignored}.{nextest,doc}).
        // nextest's comes in pieces with a line of cargo's on standard error between them, mid-line; the
        // doctests' with cargo's whole lines between its own, as cargo and the doctests' harness, each
        // writing its own lines, interleave in the one stream that list is read from.
        let box_stubs = tmp.path().join("box-stubs");
        std::fs::create_dir_all(&box_stubs).unwrap();
        write_executable(
            &box_stubs.join("cargo"),
            "#!/bin/bash
             case \"$*\" in
             metadata*) echo '{\"packages\":[{\"name\":\"agent\",\"targets\":[{\"kind\":[\"bin\"]}]}]}'; exit ;;
             *'nextest list'*'--run-ignored only'*) f=ignored.nextest ;;
             *'nextest list'*) f=all.nextest ;;
             *'--list --ignored'*) f=ignored.doc ;;
             *--list*) f=all.doc ;;
             *) exit 1 ;;
             esac
             while IFS= read -r line; do
               if [[ $f == *nextest ]]; then printf '%s' \"${line:0:3}\"; echo '    Finished `test` profile' >&2; printf '%s\\n' \"${line:3}\"
               else echo '     Running unittests' >&2; printf '%s\\n' \"$line\"; fi
             done <\"$T/linux-$f\"
",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let box_path = format!("{}:{}", box_stubs.display(), std::env::var("PATH").unwrap());
        let log = tmp.path().join("log");
        let t = |name: &str| tmp.path().join(name);
        let read = |name: &str| std::fs::read_to_string(t(name)).unwrap_or_default();
        // The lists as each side prints them: the tests of the binary `agent` as cargo nextest list
        // --message-format oneline does, then the doctests (names starting doc:) of the crate `demo` as cargo
        // test --doc -- --list does: its Doc-tests line, then each test.
        let nextest = |prefix: &str, tests: &[&str]| {
            tests.iter().filter(|n| !n.starts_with("doc:")).map(|n| format!("{prefix}agent {n}\n")).collect::<String>()
        };
        let doc = |prefix: &str, tests: &[&str]| {
            let mut s = format!("{prefix}   Doc-tests demo\n");
            for name in tests.iter().filter_map(|n| n.strip_prefix("doc:")) {
                s.push_str(&format!("{prefix}src/lib.rs - {name} (line 1): test\n"));
            }
            s
        };
        let both = ["plain", "macos_only::sandboxed", "doc:f"];
        let setup = |mac_all: &[&str], mac_ign: &[&str], linux_all: &[&str], linux_ign: &[&str]| {
            for name in ["args", "deleted", "ready", "left", "linux"] {
                let _ = std::fs::remove_file(t(name));
            }
            let _ = std::fs::remove_file(&log);
            std::fs::write(t("mac-all.nextest"), nextest("", mac_all)).unwrap();
            std::fs::write(t("mac-ignored.nextest"), nextest("", mac_ign)).unwrap();
            std::fs::write(t("mac-all.doc"), doc("", mac_all)).unwrap();
            std::fs::write(t("mac-ignored.doc"), doc("", mac_ign)).unwrap();
            std::fs::write(t("linux-all.nextest"), nextest("", linux_all)).unwrap();
            std::fs::write(t("linux-ignored.nextest"), nextest("", linux_ign)).unwrap();
            std::fs::write(t("linux-all.doc"), doc("", linux_all)).unwrap();
            std::fs::write(t("linux-ignored.doc"), doc("", linux_ign)).unwrap();
            let mut c = command(&dir, "scripts/test-table", &["--lists"]);
            let lists = c.env("PATH", &box_path).env("T", tmp.path()).output().unwrap();
            let linux = String::from_utf8(lists.stdout).unwrap();
            assert!(lists.status.success(), "{scripts}: --lists: {}", String::from_utf8_lossy(&lists.stderr));
            assert!(!linux.contains("Finished"), "{scripts}: nextest's standard error in the lists: {linux}");
            std::fs::write(t("linux"), &linux).unwrap();
        };
        // Fails, rather than hangs, if the run outlasts 120 s: it then stops its process group. With
        // `stop_on`, a Ctrl-C goes to the run's group once that file exists.
        let split_with = |exit: &str, sleep: &str, fail_here: &str, env: &[(&str, &str)], stop_on: Option<&str>| {
            let mut c = command(&dir, "scripts/test-table", &["--split"]);
            c.env("PATH", &path).env("T", tmp.path()).env("LOG", &log).env("EXIT", exit).env("SLEEP", sleep);
            c.env("FAIL_HERE", fail_here).env("TEST_TABLE_HERE_LIMIT", "3");
            for name in ["HANG", "HANG_LIST", "SURVIVE", "IGNORE_TERM", "NO_NEXTEST", "BOX_PATH", "REPO"] {
                c.env(name, env.iter().find(|(n, _)| *n == name).map_or("", |(_, v)| *v));
            }
            // Files, not pipes: a process the run left behind would hold a pipe open, and reading it would hang.
            let (out_file, err_file) = (t("run.out"), t("run.err"));
            c.stdout(std::fs::File::create(&out_file).unwrap()).stderr(std::fs::File::create(&err_file).unwrap());
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            let mut child = c.spawn().unwrap();
            let started = std::time::Instant::now();
            let mut stopped = false;
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if !stopped && stop_on.is_some_and(|name| t(name).exists()) {
                    let group = format!("-{}", child.id());
                    assert!(Command::new("/bin/kill").args(["-INT", "--", &group]).status().unwrap().success());
                    stopped = true;
                }
                if started.elapsed() > std::time::Duration::from_secs(120) {
                    let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
                    panic!("{scripts}: the run didn't end within 120 s: {}", read("run.err"));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            Output { status, stdout: std::fs::read(&out_file).unwrap(), stderr: std::fs::read(&err_file).unwrap() }
        };
        let split = |exit: &str, sleep: &str, fail_here: &str| split_with(exit, sleep, fail_here, &[], None);
        let none_left = |what: &str| {
            for pid in read("pids").lines() {
                let alive =
                    Command::new("/bin/kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().unwrap();
                assert!(!alive.success(), "{scripts}: {what}: process {pid} was left running");
            }
        };
        let text =
            |out: &Output| format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

        // Both pass, and Linux ignores exactly the macos_only test. A process left in timeout's group by
        // tests that ended by themselves is not killed: the group's id could be another's by then.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let out = split_with("0", "0", "", &[("SURVIVE", "1")], None);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        let survivor = read("survivor");
        let alive = Command::new("/bin/kill").args(["-0", survivor.trim()]).status().unwrap();
        let _ = Command::new("/bin/kill").args([survivor.trim()]).status();
        assert!(alive.success(), "{scripts}: a passing run killed {survivor} in timeout's old group");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("== the box, last lines") && stdout.contains("== here, last lines"), "{scripts}");
        assert!(stdout.contains("exit 0 · 5m 00s") && stdout.contains("1 test run: 1 passed"), "{scripts}: {stdout}");
        assert!(stdout.contains("the whole table in a box, and the macOS-only tests here"), "{scripts}");
        assert!(!stdout.contains("LIST "), "{scripts}: printed the box's lists");
        let calls = std::fs::read_to_string(&log).unwrap();
        let here = "cargo nextest run --locked --workspace --no-fail-fast -E test(~macos_only::)";
        assert!(calls.lines().any(|l| l == here), "{scripts}: {calls}");
        assert!(
            !calls.contains("clippy") && !calls.contains("ground-guards"),
            "{scripts}: the box runs those: {calls}"
        );
        let args = read("args");
        assert!(args.lines().any(|a| a == "--no-env") && !args.contains("--env"), "{scripts}: {args}");

        // The full run's box script, through the real --lists: the whole workspace's lists, with no -p
        // (fail 40550d3c989e: it asked for '--lists -p ', which --lists refuses).
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let out = split_with("0", "0", "", &[("BOX_PATH", &box_path), ("REPO", dir.to_str().unwrap())], None);
        assert!(out.status.success(), "{scripts}: the full split through the real --lists: {}", text(&out));
        assert_eq!(read("ran-lists"), "scripts/test-table --lists\n", "{scripts}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("the whole table in a box, and the macOS-only tests here")
        );

        // --split -p: the box runs the light table and the lists for the packages; here, their macOS-only
        // tests (none is no failure) and lists, and the doctests' list only for the one with a library.
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(t("args"));
        let mut c = command(&dir, "scripts/test-table", &["--split", "-p", "demo", "-p", "agent"]);
        c.env("PATH", &path).env("T", tmp.path()).env("LOG", &log).env("EXIT", "0").env("SLEEP", "0");
        c.env("FAIL_HERE", "").env("TEST_TABLE_HERE_LIMIT", "3").env("TEST_TABLE_MODE", "light: the scope");
        for name in ["HANG", "HANG_LIST", "SURVIVE", "IGNORE_TERM", "NO_NEXTEST", "BOX_PATH", "REPO"] {
            c.env(name, "");
        }
        let out = c.output().unwrap();
        assert!(out.status.success(), "{scripts}: --split -p: {}", text(&out));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("light run passed at")
                && stdout.contains("-p demo -p agent, in a box, and their macOS-only tests here; not the whole table"),
            "{scripts}: {stdout}"
        );
        assert!(stdout.trim_end().ends_with("test-table: counted run mode: light: the scope"), "{scripts}: {stdout}");
        let inner = read("args");
        for line in [
            "  setup $sudo apt-get install -y -qq -o Acquire::Retries=3 git b3sum curl jq lsof",
            "setup cargo fetch --locked --target \"$(rustc -vV | sed -n \"s/^host: //p\")\"",
            "cargo build --locked -q -p bracken --bin bracken",
            "BRACKEN_BIN=\"$PWD/target/debug/bracken\" scripts/test-table --light -p demo -p agent",
            "scripts/test-table --lists -p demo -p agent",
        ] {
            assert!(inner.lines().any(|l| l == line), "{scripts}: the box's script lacks {line:?}: {inner}");
        }
        assert!(
            !inner.lines().any(|l| l == "BRACKEN_BIN=\"$PWD/target/debug/bracken\" scripts/test-table"),
            "{scripts}"
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        for call in [
            "cargo nextest run --locked -p demo -p agent --no-fail-fast --no-tests=pass -E test(~macos_only::)",
            "cargo nextest list --locked -p demo -p agent --run-ignored all --message-format oneline",
            "cargo test --locked --doc -p demo -- --list",
            "cargo test --locked --doc -p demo -- --list --ignored",
        ] {
            assert!(calls.lines().any(|l| l == call), "{scripts}: no {call:?} in {calls}");
        }
        assert!(!calls.contains("--workspace"), "{scripts}: {calls}");

        // --lists -p in the box: a binary-only package has no doctests' list.
        let mut c = command(&dir, "scripts/test-table", &["--lists", "-p", "agent"]);
        let lists = c.env("PATH", &box_path).env("T", tmp.path()).output().unwrap();
        let linux = String::from_utf8(lists.stdout).unwrap();
        assert!(lists.status.success(), "{scripts}: --lists -p: {}", String::from_utf8_lossy(&lists.stderr));
        assert!(linux.contains("LIST all nextest ") && !linux.contains("LIST all doc "), "{scripts}: {linux}");
        for args in [&["--split", "-p"][..], &["--split", "-p", "-x"], &["--lists", "agent"]] {
            let out = command(&dir, "scripts/test-table", args).env("PATH", &path).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}");
        }

        // Without cargo-nextest it refuses to start, saying what it needs: no box, and no tests here.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let out = split_with("0", "0", "", &[("NO_NEXTEST", "101")], None);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("needs cargo-nextest"), "{scripts}: {}", text(&out));
        assert!(read("args").is_empty(), "{scripts}: started a box without nextest");
        assert!(read("log").is_empty(), "{scripts}: ran cargo without nextest: {}", read("log"));

        // A test that runs here but not on Linux, outside macos_only, is named, whichever way Linux skips it,
        // and a doctest as well as a test in a test binary.
        for (what, mac_all, linux_all, linux_ign, escaped) in [
            (
                "compiled out on Linux",
                &["plain", "acl", "macos_only::sandboxed", "doc:f"][..],
                &both[..],
                &["macos_only::sandboxed"][..],
                "agent|acl",
            ),
            ("ignored only on Linux", &both[..], &both[..], &["plain", "macos_only::sandboxed"][..], "agent|plain"),
            (
                "a doctest compiled out on Linux",
                &["plain", "macos_only::sandboxed", "doc:f", "doc:g"][..],
                &both[..],
                &["macos_only::sandboxed"][..],
                "doc:demo|src/lib.rs - g (line 1)",
            ),
            (
                "a doctest ignored only on Linux",
                &both[..],
                &both[..],
                &["macos_only::sandboxed", "doc:f"][..],
                "doc:demo|src/lib.rs - f (line 1)",
            ),
        ] {
            setup(mac_all, &[], linux_all, linux_ign);
            let out = split("0", "0", "");
            assert_eq!(out.status.code(), Some(1), "{scripts}: {what}: {}", text(&out));
            assert!(text(&out).contains("neither part runs them") && text(&out).contains(escaped), "{scripts}: {what}");
        }

        // The box fails: so does the run, with its exit.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let out = split("3", "0", "");
        assert_eq!(out.status.code(), Some(3), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("the box's table exited 3"), "{scripts}");

        // The box lists nothing, or a list stops short of its end line (a list command that failed): which
        // tests need macOS is unknown, so it fails, even though a test ignored only on Linux could hide there.
        setup(&both, &[], &both, &[]);
        std::fs::write(t("linux"), "").unwrap();
        let out = split("0", "0", "");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("lists didn't finish"), "{scripts}: {}", text(&out));
        setup(&both, &[], &both, &[]);
        let whole = read("linux");
        let cut = &whole[..whole.find("LIST end ignored").expect("the end of the ignored list")];
        std::fs::write(t("linux"), cut).unwrap();
        let out = split("0", "0", "");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("lists didn't finish"), "{scripts}: {}", text(&out));

        // A line damaged on the way, here by a line of standard error inside it, fails as damaged: it can't
        // drop a test from the list and pass it off as one that needs macOS.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let whole = read("linux");
        let line = "LIST all nextest agent plain\n";
        assert!(whole.contains(line), "{scripts}: {whole}");
        std::fs::write(t("linux"), whole.replace(line, "LIST all nextest ag    Finished `test` profile\nent plain\n"))
            .unwrap();
        let out = split("0", "0", "");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("list of all tests came back damaged"), "{scripts}: {}", text(&out));
        assert!(!text(&out).contains("neither part runs them"), "{scripts}: {}", text(&out));

        // The tests here fail while the box runs (its 10 minutes): the box is stopped and deleted at once.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        std::fs::write(t("left"), "running").unwrap();
        let started = std::time::Instant::now();
        let out = split("0", "600", "4");
        let took = started.elapsed();
        assert_eq!(out.status.code(), Some(4), "{scripts}: {}", text(&out));
        assert!(took < std::time::Duration::from_secs(180), "{scripts}: waited {took:?} for the box");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left: {}", text(&out));

        // A test program here that hangs (cargo's child, which outlives cargo) ends at the bound, here 3 s,
        // with nothing left running, and the box is stopped and deleted (finding c7d17b82e315).
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        std::fs::write(t("left"), "running").unwrap();
        let _ = std::fs::remove_file(t("pids"));
        let started = std::time::Instant::now();
        let out = split_with("0", "600", "", &[("HANG", "1")], None);
        let took = started.elapsed();
        assert_eq!(out.status.code(), Some(124), "{scripts}: {}", text(&out));
        assert!(took < std::time::Duration::from_secs(120), "{scripts}: took {took:?}: {}", text(&out));
        none_left("the hung test");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left: {}", text(&out));

        // A Ctrl-C while this Mac lists its tests stops the list too: nothing outlives the run.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        std::fs::write(t("left"), "running").unwrap();
        for name in ["pids", "listing"] {
            let _ = std::fs::remove_file(t(name));
        }
        let started = std::time::Instant::now();
        let out = split_with("0", "600", "", &[("HANG_LIST", "1")], Some("listing"));
        let took = started.elapsed();
        assert_eq!(out.status.code(), Some(130), "{scripts}: {}", text(&out));
        assert!(took < std::time::Duration::from_secs(120), "{scripts}: took {took:?}: {}", text(&out));
        none_left("the hung list");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left: {}", text(&out));

        // A Ctrl-C while a test here ignores SIGTERM: test-table still kills it, by timeout's group, as it
        // exits (finding a9acfeadbe06).
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        std::fs::write(t("left"), "running").unwrap();
        for name in ["pids", "ignoring"] {
            let _ = std::fs::remove_file(t(name));
        }
        let started = std::time::Instant::now();
        let out = split_with("0", "600", "", &[("IGNORE_TERM", "1")], Some("ignoring"));
        let took = started.elapsed();
        assert_eq!(out.status.code(), Some(130), "{scripts}: {}", text(&out));
        assert!(took < std::time::Duration::from_secs(120), "{scripts}: took {took:?}: {}", text(&out));
        none_left("the test ignoring SIGTERM");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left: {}", text(&out));

        // The tests here hit their limit, here 3 s, with no Ctrl-C, while a test ignores SIGTERM: timeout
        // exits 124 as cargo does, and test-table still kills the test, by timeout's group (note 263eb49eb068).
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        std::fs::write(t("left"), "running").unwrap();
        let _ = std::fs::remove_file(t("pids"));
        let started = std::time::Instant::now();
        let out = split_with("0", "600", "", &[("IGNORE_TERM", "1")], None);
        let took = started.elapsed();
        assert_eq!(out.status.code(), Some(124), "{scripts}: {}", text(&out));
        assert!(took < std::time::Duration::from_secs(120), "{scripts}: took {took:?}: {}", text(&out));
        none_left("the test ignoring SIGTERM at the limit");
        assert_eq!(read("deleted"), "delete --yes bx_stub01\n", "{scripts}: the box was left: {}", text(&out));

        // Tests here that fail fast, timeout reaped before test-table waits on it (a delay put in on
        // purpose): the run still fails with their exit, not a pass (note fa805ecf517e).
        let script = std::fs::read_to_string(dir.join("scripts/test-table")).unwrap();
        let fork = "  t=$!\n  echo \"$t\" >\"$split_tmp/timeout.pid\"\n";
        assert_eq!(script.matches(fork).count(), 1, "{scripts}: the delay has nowhere to go");
        std::fs::write(
            dir.join("scripts/test-table"),
            script.replace(fork, &format!("  t=$!\n  sleep 1\n{}", &fork[9..])),
        )
        .unwrap();
        commit_all(&dir, "a delay after the fork");
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        let out = split("0", "0", "4");
        assert_eq!(out.status.code(), Some(4), "{scripts}: a failure here passed: {}", text(&out));

        // A docs-only head: the local docs-only table, no box.
        setup(&both, &[], &both, &["macos_only::sandboxed"]);
        git(&["switch", "-qc", "docs", "refs/remotes/origin/main"]);
        put(&dir, "docs/guide.md", "a guide\n");
        commit_all(&dir, "docs only");
        let out = split("0", "0", "");
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert!(read("args").is_empty(), "{scripts}: a box for a docs-only head");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "cargo fmt --check\nground-guards origin/main HEAD\n",
            "{scripts}"
        );
    }
}

/// A manifest's checkout as weave validate makes it (plan cc496dcee0e3): a repository whose one commit is
/// on branch weave, a bundle of it, and the WEAVE_* variables validate gives, with an empty facts file.
/// The test-table copy runs from `scripts`, outside it, as the main checkout's pinned copy does.
struct WeaveCheckout {
    tmp: tempfile::TempDir,
    tree: PathBuf,
    commit: String,
    tree_id: String,
}

impl WeaveCheckout {
    fn new(scripts: &str) -> WeaveCheckout {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(tmp.path().join("runner")).unwrap();
        copy_executable(&repo_root().join(scripts).join("test-table"), &tmp.path().join("runner/test-table"));
        std::fs::create_dir_all(&tree).unwrap();
        put(&tree, "a.txt", "one\n");
        // An attribute a checkout would apply: id.txt's $Id$ expands unless attributes are off.
        put(&tree, ".gitattributes", "id.txt ident\n");
        put(&tree, "id.txt", "$Id$\n");
        for args in [
            &["init", "-q", "-b", "weave"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@example.invalid"],
            &["add", "-A"],
            &["commit", "-qm", "weave manifest"],
        ] {
            assert!(run(&tree, "git", args).status.success(), "git {args:?} failed");
        }
        let rev = |r: &str| String::from_utf8(run(&tree, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let (commit, tree_id) = (rev("HEAD"), rev("HEAD^{tree}"));
        let bundle = tmp.path().join("tree.bundle");
        assert!(run(&tree, "git", &["bundle", "create", "-q", bundle.to_str().unwrap(), "weave"]).status.success());
        std::fs::write(tmp.path().join("facts"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("inputs")).unwrap();
        WeaveCheckout { tmp, tree, commit, tree_id }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    fn sha256(&self) -> String {
        let out = Command::new("shasum").args(["-a", "256"]).arg(self.path("tree.bundle")).output().unwrap();
        String::from_utf8(out.stdout).unwrap().split_whitespace().next().unwrap().to_owned()
    }

    /// The runner copy of test-table with `args`, in the checkout, with validate's variables.
    fn table(&self, args: &[&str]) -> Command {
        let mut c = Command::new(self.path("runner/test-table"));
        c.args(args)
            .current_dir(&self.tree)
            .env("WEAVE_BUNDLE", self.path("tree.bundle"))
            .env("WEAVE_BUNDLE_SHA256", self.sha256())
            .env("WEAVE_COMMIT", &self.commit)
            .env("WEAVE_TREE", &self.tree_id)
            .env("WEAVE_FACTS", self.path("facts"))
            .env("WEAVE_INPUTS", self.path("inputs"))
            .env("T", self.tmp.path());
        c
    }

    fn facts(&self) -> String {
        std::fs::read_to_string(self.path("facts")).unwrap()
    }
}

/// `test-table --weave-box`, weave validate's box check (plan cc496dcee0e3; weave.md section 3, Checks
/// outside the sandbox), with a stub boat that plays the box: the box gets the bundle and this copy of the
/// script and nothing else, prints the proofs before any of the tree's code, and the facts get the first
/// of each, the box boat announced and its cost line, never what the tree prints after 'WEAVE start'. The
/// box's own commands, from the bundle to 'WEAVE start', print the bundle's sha256, the commit and tree.
#[test]
fn test_table_weave_box_pushes_the_bundle_and_this_script_and_records_the_boxs_proofs() {
    for scripts in SCRIPTS {
        let w = WeaveCheckout::new(scripts);
        let stubs = w.path("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // boat run: records its arguments and what was pushed, keeps the push, and plays the box from $T/box.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s\\n' \"$@\" >\"$T/args\"\n\
               while [ $# -gt 0 ]; do [ \"$1\" = --push ] && push=$2; shift; done\n\
               (cd \"$push\" && find . -type f | sort) >\"$T/pushed\"; cp -R \"$push\" \"$T/push-copy\"\n\
               echo 'bx_stub01 ready in 1.0s'; cat \"$T/box\"\n\
               echo \"exit $EXIT · 1m 00s · default · \\$0.0010 · bx_stub01 deleted\"; exit \"$EXIT\" ;;\n\
             list) echo '{\"sandboxes\":[]}' ;;\n\
             delete) echo \"$*\" >>\"$T/deleted\" ;;\n\
             esac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let sha = w.sha256();
        std::fs::write(
            w.path("box"),
            format!(
                "WEAVE bundle {sha}\nWEAVE head {}\nWEAVE tree {}\nWEAVE start\nWEAVE tree 0000 printed by the tree\n\
                 exit 0 · printed by the tree\ntest-table: all passed\nWEAVE end\n",
                w.commit, w.tree_id
            ),
        )
        .unwrap();
        let out = w.table(&["--weave-box"]).env("PATH", &path).env("EXIT", "0").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}{stdout}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            w.facts(),
            format!(
                "bundle {sha}\nhead {}\ntree {}\nbox bx_stub01\ncost exit 0 · 1m 00s · default · $0.0010 · bx_stub01 deleted\n\
                 end seen\n",
                w.commit, w.tree_id
            ),
            "{scripts}"
        );
        let args = std::fs::read_to_string(w.path("args")).unwrap();
        assert!(args.lines().any(|a| a == "--no-env"), "{scripts}: {args}");
        assert_eq!(
            std::fs::read_to_string(w.path("pushed")).unwrap(),
            "./runner/test-table\n./tree.bundle\n",
            "{scripts}"
        );
        assert_eq!(
            std::fs::read(w.path("push-copy/runner/test-table")).unwrap(),
            std::fs::read(w.path("runner/test-table")).unwrap(),
            "{scripts}: the box runs this copy"
        );
        // The box's script: the proofs come before the tree's code, which runs this copy, never the tree's.
        // boat run ... -- bash -c <inner>: the box script is the last argument, after -c.
        let inner = args.split("\n-c\n").nth(1).expect("bash -c <script>").to_owned();
        // One stream from the first line, so the proofs keep their place (finding 694f565ee5cd).
        assert!(inner.starts_with("exec 2>&1\n"), "{scripts}: {inner}");
        let start = inner.find("echo \"WEAVE start\"").expect("the start line");
        let table = inner.find("bash ../runner/test-table --in-tree").expect("this copy, in the tree");
        assert!(start < table, "{scripts}: {inner}");
        let lists = inner.find("bash ../runner/test-table --in-tree --lists").expect("the lists");
        assert!(inner[lists..].trim_end().ends_with("echo 'WEAVE end'"), "{scripts}: {inner}");
        assert!(!inner.contains("scripts/test-table"), "{scripts}: the tree's own copy runs: {inner}");
        // Those commands themselves, from the checkout to the start line, on the pushed bundle (sha256sum
        // as the box has it).
        let from = inner.find("git init -q tree").expect("the checkout");
        let prove = &inner[from..start + "echo \"WEAVE start\"".len()];
        let box_dir = w.path("box-run");
        std::fs::create_dir_all(&box_dir).unwrap();
        std::fs::copy(w.path("tree.bundle"), box_dir.join("tree.bundle")).unwrap();
        write_executable(&stubs.join("sha256sum"), "#!/bin/sh\nexec shasum -a 256 \"$@\"\n");
        let ran = Command::new("bash")
            .args(["-c", &format!("set -eu\n{prove}")])
            .current_dir(&box_dir)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(ran.status.success(), "{scripts}: {}", String::from_utf8_lossy(&ran.stderr));
        // The checkout is the commit's bytes: its own .gitattributes changed none (finding 42ac5a6e8aee).
        assert_eq!(std::fs::read_to_string(box_dir.join("tree/id.txt")).unwrap(), "$Id$\n", "{scripts}");
        assert_eq!(
            String::from_utf8_lossy(&ran.stdout),
            format!("WEAVE bundle {sha}\nWEAVE head {}\nWEAVE tree {}\nWEAVE start\n", w.commit, w.tree_id),
            "{scripts}"
        );

        // Nothing of the tree's runs on this Mac (findings 42ac5a6e8aee, 367a51649450): a boat or a dirname
        // in the tree, found first through a relative PATH entry, never runs. (A bash there would, through
        // env on the script's first line: weave validate's PATH and start directory keep that out.)
        for name in ["boat", "dirname"] {
            write_executable(&w.tree.join(name), &format!("#!/bin/sh\ntouch \"$T/tree-{name}-ran\"\nexit 7\n"));
        }
        std::fs::write(w.path("facts"), "").unwrap();
        let relative = format!(".:{path}");
        let out = w.table(&["--weave-box"]).env("PATH", &relative).env("EXIT", "0").output().unwrap();
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        for name in ["boat", "dirname"] {
            assert!(!w.path(&format!("tree-{name}-ran")).exists(), "{scripts}: the tree's {name} ran");
            std::fs::remove_file(w.tree.join(name)).unwrap();
        }

        // -p: the light table and the lists of those packages.
        std::fs::write(w.path("facts"), "").unwrap();
        let out = w.table(&["--weave-box", "-p", "demo"]).env("PATH", &path).env("EXIT", "0").output().unwrap();
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let args = std::fs::read_to_string(w.path("args")).unwrap();
        assert!(args.contains("bash ../runner/test-table --in-tree --light -p demo\n"), "{scripts}: {args}");
        assert!(args.contains("bash ../runner/test-table --in-tree --lists -p demo"), "{scripts}: {args}");

        // The box failed: so does the check, with the proofs still written.
        std::fs::write(w.path("facts"), "").unwrap();
        let out = w.table(&["--weave-box"]).env("PATH", &path).env("EXIT", "4").output().unwrap();
        assert_eq!(out.status.code(), Some(4), "{scripts}");
        assert!(w.facts().contains("box bx_stub01\n"), "{scripts}");

        // No start line: no proofs. No end line after the table's success: the check fails, with no end.
        std::fs::write(w.path("facts"), "").unwrap();
        std::fs::write(w.path("box"), format!("WEAVE head {}\ntest-table: all passed\nWEAVE end\n", w.commit)).unwrap();
        let out = w.table(&["--weave-box"]).env("PATH", &path).env("EXIT", "0").output().unwrap();
        assert!(!out.status.success(), "{scripts}");
        let facts = w.facts();
        assert!(!facts.contains("head ") && !facts.contains("end seen"), "{scripts}: {facts}");
        std::fs::write(w.path("facts"), "").unwrap();
        std::fs::write(w.path("box"), "WEAVE start\nWEAVE end\ntest-table: all passed\n").unwrap();
        let out = w.table(&["--weave-box"]).env("PATH", &path).env("EXIT", "0").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("no 'WEAVE end'"), "{scripts}");
        assert!(!w.facts().contains("end seen"), "{scripts}");

        // weave validate gone, even by SIGKILL: the output pipe loses its reader, and the box is still
        // deleted, by the EXIT trap (finding 34f8b1a9991f); boat was also told the box's lifetime.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) echo 'bx_stub02 ready in 1.0s'; for i in $(seq 100); do echo \"line $i\"; sleep 0.1; done ;;\n\
             list) if [ -e \"$T/deleted\" ]; then echo '{\"sandboxes\":[]}'; \
                   else echo '{\"sandboxes\":[{\"id\": \"bx_stub02\"}]}'; fi ;;\n\
             delete) echo \"$*\" >>\"$T/deleted\" ;;\n\
             extend) echo \"$*\" >>\"$T/extended\" ;;\n\
             esac\n",
        );
        let _ = std::fs::remove_file(w.path("deleted"));
        let (mut reader, writer) = std::io::pipe().unwrap();
        let mut child = w
            .table(&["--weave-box"])
            .env("PATH", &path)
            .stdout(writer.try_clone().unwrap())
            .stderr(writer)
            .spawn()
            .unwrap();
        let mut first = [0u8; 64];
        std::io::Read::read_exact(&mut reader, &mut first).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        drop(reader);
        let status = child.wait().unwrap();
        assert!(!status.success(), "{scripts}");
        let deleted = std::fs::read_to_string(w.path("deleted")).unwrap_or_default();
        assert!(deleted.contains("delete --yes bx_stub02"), "{scripts}: no delete: {deleted:?}");
        let extended = std::fs::read_to_string(w.path("extended")).unwrap_or_default();
        assert_eq!(extended, "extend bx_stub02 --ttl 2700\n", "{scripts}");

        // Not as validate's check: refused, and no box.
        std::fs::remove_file(w.path("args")).unwrap();
        let out = w.table(&["--weave-box"]).env("PATH", &path).env_remove("WEAVE_BUNDLE").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("which gives WEAVE_BUNDLE"), "{scripts}");
        assert!(!w.path("args").exists(), "{scripts}: a box ran");
    }
}

/// `test-table --weave-mac <box check>`, weave validate's mac check (plan cc496dcee0e3): in the manifest's
/// checkout it writes the commit and tree first, runs the macOS-only tests, and compares this Mac's lists
/// with the box's, read from the box check's output. A test that runs here and on neither side's list
/// fails it, as in --split; so do a missing box output and a checkout that isn't the manifest's commit.
#[test]
fn test_table_weave_mac_runs_the_macos_tests_and_compares_with_the_boxs_lists() {
    for scripts in SCRIPTS {
        let w = WeaveCheckout::new(scripts);
        let stubs = w.path("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // cargo: nextest is there; this Mac's lists from $T/mac-all (the ignored ones and doctests empty);
        // the macOS-only run passes, and logs that it ran.
        write_executable(
            &stubs.join("cargo"),
            "#!/bin/bash\n[ \"$*\" != 'nextest --version' ] || exit 0\n\
             case \"$*\" in\n\
             *'nextest list'*'--run-ignored only'*) ;;\n\
             *'nextest list'*) cat \"$T/mac-all\" ;;\n\
             *--list*) ;;\n\
             *macos_only::*) echo ran >\"$T/ran-here\"; echo '     Summary [   1.000s] 1 test run: 1 passed, 0 skipped' ;;\n\
             esac\n",
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let cksum = |text: &str| {
            let mut c = Command::new("cksum")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::io::Write::write_all(c.stdin.as_mut().unwrap(), text.as_bytes()).unwrap();
            String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap().trim().to_owned()
        };
        // The box's output: its lists, as test-table --lists prints them.
        let box_log = |tests: &str| {
            let lines: String = tests.lines().map(|t| format!("LIST all nextest {t}\n")).collect();
            format!("the box's table\n{lines}LIST end all {}\nLIST end ignored {}\n", cksum(tests), cksum(""))
        };
        std::fs::write(w.path("inputs/table.log"), box_log("demo::t a\n")).unwrap();
        std::fs::write(w.path("mac-all"), "demo::t a\ndemo::t macos_only::b\n").unwrap();
        let mac = || w.table(&["--weave-mac", "table"]).env("PATH", &path).output().unwrap();

        let out = mac();
        assert!(
            out.status.success(),
            "{scripts}: {}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(w.facts(), format!("head {}\ntree {}\n", w.commit, w.tree_id), "{scripts}");
        assert!(w.path("ran-here").exists(), "{scripts}: the macOS-only tests didn't run");

        // A test here that the box doesn't list, and not macOS-only: neither side runs it.
        std::fs::write(w.path("mac-all"), "demo::t a\ndemo::t c\n").unwrap();
        let out = mac();
        assert!(!out.status.success(), "{scripts}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("demo::t|c"),
            "{scripts}: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // The box's list came back damaged.
        std::fs::write(w.path("mac-all"), "demo::t a\n").unwrap();
        std::fs::write(w.path("inputs/table.log"), box_log("demo::t a\n").replace("t a", "t z")).unwrap();
        let out = mac();
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("damaged"),
            "{scripts}: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // No output of the box check: refused.
        std::fs::remove_file(w.path("inputs/table.log")).unwrap();
        let out = mac();
        assert!(String::from_utf8_lossy(&out.stderr).contains("no output of box check table"), "{scripts}");

        // Not the manifest's commit.
        std::fs::write(w.path("facts"), "").unwrap();
        let out =
            w.table(&["--weave-mac", "table"]).env("PATH", &path).env("WEAVE_COMMIT", "0".repeat(40)).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("isn't the manifest's commit"), "{scripts}");

        // Not the manifest's tree, or not clean (finding 79114ae0b4ab): refused before any test runs or
        // any fact is written.
        std::fs::write(w.path("inputs/table.log"), box_log("demo::t a\n")).unwrap();
        std::fs::remove_file(w.path("ran-here")).unwrap();
        let out =
            w.table(&["--weave-mac", "table"]).env("PATH", &path).env("WEAVE_TREE", "0".repeat(40)).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("isn't the manifest's tree"), "{scripts}");
        for (name, change) in [("a.txt", "changed\n"), ("new.txt", "new\n")] {
            let before = std::fs::read(w.tree.join(name)).ok();
            std::fs::write(w.tree.join(name), change).unwrap();
            let out = mac();
            assert!(!out.status.success(), "{scripts}: {name}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("isn't clean"),
                "{scripts}: {name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            match before {
                Some(bytes) => std::fs::write(w.tree.join(name), bytes).unwrap(),
                None => std::fs::remove_file(w.tree.join(name)).unwrap(),
            }
        }
        // An ignored file, and a change git is told not to look at (finding 35815ad2763b).
        std::fs::write(w.tree.join(".git/info/exclude"), "ign\n").unwrap();
        std::fs::write(w.tree.join("ign"), "ignored\n").unwrap();
        let out = mac();
        assert!(String::from_utf8_lossy(&out.stderr).contains("isn't clean"), "{scripts}: ignored");
        std::fs::remove_file(w.tree.join("ign")).unwrap();
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let mark = |on: bool| {
                let flag = if on { flag.to_owned() } else { flag.replacen("--", "--no-", 1) };
                assert!(run(&w.tree, "git", &["update-index", flag.as_str(), "a.txt"]).status.success(), "{flag}");
            };
            mark(true);
            let out = mac();
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("skip the comparison"),
                "{scripts}: {flag}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            mark(false);
        }
        assert_eq!(w.facts(), "", "{scripts}");
        assert!(!w.path("ran-here").exists(), "{scripts}: the macOS-only tests ran");
        assert!(mac().status.success(), "{scripts}: clean again");
    }
}
