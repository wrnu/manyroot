//! scripts/verify-seq, a head at a time. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// verify-seq runs each head in a scratch worktree of the main checkout, through the bk-evidence beside it,
/// as manager; with ON set it tests the merge with ON and asks to reuse a pass of the same tree. The
/// worktree is gone afterwards. A stub bk-evidence logs where it ran and its arguments.
#[test]
fn verify_seq_runs_each_head_in_its_own_worktree_and_reuses_only_with_on() {
    for scripts in SCRIPTS {
        let seq = repo_root().join(scripts).join("verify-seq");
        let (tmp, dir) = repo(&[("scripts/verify-seq", &seq)], ".bracken/\n", |dir| {
            write_executable(
                &dir.join("scripts/bk-evidence"),
                "#!/bin/sh\necho \"$PWD|$*\" >>\"$LOG\"\necho bk-evidence: recorded pass aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa '(exit 0)' >&2\n\
                 [ -z \"${BRACKEN_FULL_CHECK+x}\" ] || echo leaked >\"$LOG.full\"\n",
            );
        });
        let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
        let log = tmp.path().join("log");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        for (on, args) in [
            (Some("origin/main"), "abc123 --as manager --on origin/main --reuse -- scripts/test-table"),
            (None, "abc123 --as manager -- scripts/test-table"),
        ] {
            let _ = std::fs::remove_file(&log);
            let mut c = command(&dir.join("scripts"), "./verify-seq", &[&format!("abc123:{sha}")]);
            // Whatever this test's own environment holds (finding 6524b6f14aa3): the stub below says
            // whether verify-seq handed the full check on, not whether the caller had it.
            c.env("LOG", &log).env("TMPDIR", &scratch).env_remove("ON").env_remove("BRACKEN_FULL_CHECK");
            if let Some(on) = on {
                c.env("ON", on);
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
            // Its own reads are full checks; the run it records isn't one (finding 607273d1d193).
            assert!(!log.with_extension("full").exists(), "{scripts}: BRACKEN_FULL_CHECK reached the tests");
            let line = std::fs::read_to_string(&log).unwrap();
            let (cwd, got) = line.trim_end().split_once('|').unwrap();
            assert!(cwd.ends_with(&format!("verify-{sha}")), "{scripts}: ran in {cwd}");
            assert_eq!(got, args, "{scripts}");
            assert!(String::from_utf8_lossy(&out.stdout).contains("removed"), "{scripts}");
            let worktrees = String::from_utf8(run(&dir, "git", &["worktree", "list"]).stdout).unwrap();
            assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");
        }
    }
}

/// verify-seq --light (decision `464d3ccca45e`, answer `10cf8a5053a3`), with a stub bk-evidence that logs its
/// arguments and the run's mode. Its scope is what scripts/code-scope packages computes from the head's
/// changed files (findings 0095e19fcdb7, 4825c59b07e9), whatever the builder ran. For each head it
/// refuses, recording nothing, until the pr claim's author (not anyone else) has passes on that exact head
/// (not another sha) of fmt, both clippies (neither alone is enough), a fast run and a --remote run each
/// naming every package in the scope (a --workspace fast run doesn't), and it refuses a head
/// scripts/code-scope full names, or with no package to test. Then it runs scripts/test-table --split -p
/// <the scope> on the merge with origin/main, without --reuse (finding 94e07ed7dd6b), the mode in
/// TEST_TABLE_MODE. Without --light, the full run's mode names what needs the whole table, and --reason.
/// Batches, --split, --speculative and -j refuse --light.
#[test]
fn verify_seq_light_counts_only_on_the_builders_passes_and_where_the_full_table_isnt_needed() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (seq, scope) = (file("verify-seq"), file("code-scope"));
        let files = [("scripts/verify-seq", seq.as_path()), ("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            write_executable(
                &dir.join("scripts/bk-evidence"),
                "#!/bin/sh\necho \"$*|$TEST_TABLE_MODE\" >>\"$LOG\"\necho bk-evidence: recorded pass aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa '(exit 0)' >&2\n",
            );
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?}");
        let rev = |r: &str| String::from_utf8(run(&dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        bk(&dir, &["init", "--name", "seq"]);
        for id in ["builder", "val", "manager"] {
            bk(&dir, &["id", "new", id]);
        }
        // Heads off main, each changing one path; each claim's text is its own.
        let claims = std::cell::Cell::new(0);
        let claim_on = |branch: &str, sha: &str| {
            claims.set(claims.get() + 1);
            let text = format!("PR #{} {branch} @ {sha}: x", claims.get());
            bk(&dir, &["claim", &text, "--tag", "pr", "--as", "builder"])
        };
        let head = |branch: &str, path: &str| {
            git(&["switch", "-qc", branch, "main"]);
            put(&dir, path, "a change\n");
            commit_all(&dir, branch);
            let sha = rev("HEAD");
            git(&["switch", "-q", "main"]);
            (claim_on(branch, &sha), sha)
        };
        let (claim, sha) = head("code", "code.txt");
        let (lock_claim, lock_sha) = head("lock", "Cargo.lock");
        let (fmt_claim, fmt_sha) = head("fmt", "rustfmt.toml");
        let scope = "-p bracken -p bracken-tree";
        let five = |scope: &str| {
            vec![
                "cargo fmt --check".to_owned(),
                "cargo clippy --all-targets -- -D warnings".to_owned(),
                "cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings".to_owned(),
                format!("cargo nextest run -P fast {scope}"),
                format!("scripts/test-table --remote {scope}"),
            ]
        };
        let record = |claim: &str, sha: &str, who: &str, methods: &[String]| {
            for method in methods {
                bk(&dir, &["evidence", claim, "pass", "--method", &format!("{method} @ {sha}"), "--as", who]);
            }
        };
        let log = tmp.path().join("log");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let seq = |args: &[&str]| {
            let _ = std::fs::remove_file(&log);
            let mut c = command(&dir.join("scripts"), "./verify-seq", args);
            let out = c
                .env("LOG", &log)
                .env("TMPDIR", &scratch)
                .env("BRACKEN_BIN", tmp.path().join("bracken"))
                .env("CODE_SCOPE_TOKENS", affected_stub(&tmp.path().join("bracken-scope")))
                .env_remove("ON")
                .output()
                .unwrap();
            (out, std::fs::read_to_string(&log).unwrap_or_default())
        };
        let pair = format!("{claim}:{sha}");
        let refused = |args: &[&str], expected: &str| {
            let (out, logged) = seq(args);
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}: {stdout}");
            assert!(
                stdout.contains("light refused") && stdout.contains(expected),
                "{scripts}: expected {expected:?}: {stdout}"
            );
            assert!(logged.is_empty(), "{scripts}: ran bk-evidence: {logged}");
        };
        // A fresh claim on the code head with these passes by the builder, refused for `expected`.
        let refused_with = |methods: &[String], on: &str, expected: &str| {
            let c = claim_on("code", &sha);
            record(&c, on, "builder", methods);
            refused(&["--light", &format!("{c}:{sha}")], expected);
        };

        // No builder pass, then val's passes only, then the builder's with a --remote run of too little.
        refused(&["--light", &pair], "cargo fmt --check");
        record(&claim, &sha, "val", &five(scope));
        refused(&["--light", &pair], "builder has no pass");
        let mut narrow = five(scope);
        narrow[4] = "scripts/test-table --remote -p bracken".to_owned();
        record(&claim, &sha, "builder", &narrow);
        refused(&["--light", &pair], "scripts/test-table --remote -p bracken -p bracken-tree");

        // Passes on another sha; one clippy missing; a --workspace fast run; a fast run narrower than the
        // scope the changed files need, which the builder chose.
        refused_with(&five(scope), &lock_sha, "cargo fmt --check; cargo clippy --all-targets -- -D warnings");
        let mut no_clippy = five(scope);
        no_clippy.remove(2);
        refused_with(
            &no_clippy,
            &sha,
            "of: cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings (on",
        );
        let mut workspace = five(scope);
        workspace[3] = "cargo nextest run -P fast --workspace".to_owned();
        refused_with(&workspace, &sha, "of: cargo nextest run -P fast -p bracken -p bracken-tree (on");
        refused_with(&five("-p bracken"), &sha, "cargo nextest run -P fast -p bracken -p bracken-tree");

        // All five: the scope's split table on the merge, and the mode.
        record(&claim, &sha, "builder", &five("-p bracken-tree -p bracken"));
        let (out, logged) = seq(&["--light", "--reason", "a scripts-only change", &pair]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stdout));
        let (args, mode) = logged.trim_end().split_once('|').unwrap();
        assert_eq!(
            args,
            format!("{claim} --as manager --on origin/main -- scripts/test-table --split -p bracken -p bracken-tree"),
            "{scripts}"
        );
        assert!(
            mode.starts_with("light: a scripts-only change; the scope -p bracken -p bracken-tree, from the changed files; builder's passes on ")
                && mode.contains("--remote"),
            "{scripts}: {mode}"
        );

        // A head touching Cargo.lock, with all five passes: refused, naming it. One with no package to test.
        record(
            &lock_claim,
            &lock_sha,
            "builder",
            &five("-p bracken -p bracken-demo -p bracken-experiment -p bracken-scope -p bracken-tree -p manyroot"),
        );
        refused(&["--light", &format!("{lock_claim}:{lock_sha}")], "Cargo.lock");
        record(&fmt_claim, &fmt_sha, "builder", &five(scope));
        refused(&["--light", &format!("{fmt_claim}:{fmt_sha}")], "no package to test");

        // The full run says why: what needs the whole table, and --reason, or the default.
        let (out, logged) = seq(&["--reason", "the manager's call", &format!("{lock_claim}:{lock_sha}")]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(
            logged.trim_end().ends_with("|full: required: Cargo.lock (the workspace's Cargo.lock); the manager's call"),
            "{scripts}: {logged}"
        );
        let (_, logged) = seq(&[&pair]);
        assert!(logged.trim_end().ends_with("-- scripts/test-table|full: the default"), "{scripts}: {logged}");

        for args in [
            &["--light", "--split", &pair][..],
            &["--light", "--batch", &pair],
            &["--light", "--split", "--speculative", &pair],
            &["--light", "--speculative", &pair],
            &["--light", "-j", "2", &pair],
            &["--light", "-j", "1", &pair],
            &["--reason", "", &pair],
        ] {
            let (out, logged) = seq(args);
            assert_eq!(out.status.code(), Some(2), "{scripts}: {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            assert!(logged.is_empty(), "{scripts}: {args:?} ran");
        }
        let worktrees = String::from_utf8(run(&dir, "git", &["worktree", "list"]).stdout).unwrap();
        assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");
    }
}

/// verify-seq --split runs the split counted run, and with -j 2 two PRs at once, each its own evidence; -j
/// takes 1 to 8 boxes, 4 by default with --split (answer c3e60ba8b879), and only with --split. A stub
/// bk-evidence logs when each starts and ends.
#[test]
fn verify_seq_runs_two_split_runs_at_once() {
    for scripts in SCRIPTS {
        let seq = repo_root().join(scripts).join("verify-seq");
        let (tmp, dir) = repo(&[("scripts/verify-seq", &seq)], ".bracken/\n", |dir| {
            write_executable(
                &dir.join("scripts/bk-evidence"),
                "#!/bin/sh\necho \"start $1 $*\" >>\"$LOG\"\nsleep 2\necho \"end $1\" >>\"$LOG\"\n[ -e \"$LOG.silent-$1\" ] || echo bk-evidence: recorded pass aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa '(exit 0)' >&2\n",
            );
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?}");
        let sha = |r: &str| String::from_utf8(run(&dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let first = sha("HEAD");
        put(&dir, "b.txt", "b\n");
        commit_all(&dir, "b");
        let second = sha("HEAD");
        git(&["switch", "-q", "--detach", "HEAD"]);
        let log = tmp.path().join("log");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let seq_run = |args: &[&str]| {
            let _ = std::fs::remove_file(&log);
            let mut c = command(&dir.join("scripts"), "./verify-seq", args);
            c.env("LOG", &log).env("TMPDIR", &scratch).env("ON", "origin/main");
            c.output().unwrap()
        };
        let (a, b) = (format!("aaa111:{first}"), format!("bbb222:{second}"));
        let out = seq_run(&["--split", "-j", "2", &a, &b]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let lines = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = lines.lines().collect();
        assert_eq!(lines.len(), 4, "{scripts}: {lines:?}");
        assert!(lines[..2].iter().all(|l| l.starts_with("start ")), "{scripts}: one after the other: {lines:?}");
        for claim in ["aaa111", "bbb222"] {
            let want =
                format!("start {claim} {claim} --as manager --on origin/main --reuse -- scripts/test-table --split");
            assert!(lines.contains(&want.as_str()), "{scripts}: {lines:?}");
        }
        let worktrees = String::from_utf8(run(&dir, "git", &["worktree", "list"]).stdout).unwrap();
        assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");
        // --split alone: up to 4 at once, so both start before either ends.
        let out = seq_run(&["--split", &a, &b]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let lines = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = lines.lines().collect();
        assert!(lines.len() == 4 && lines[..2].iter().all(|l| l.starts_with("start ")), "{scripts}: {lines:?}");
        for args in [&["-j", "2", &a][..], &["--split", "-j", "9", &a], &["--split", "-j", "0", &a]] {
            let out = seq_run(args);
            assert_eq!(out.status.code(), Some(2), "{scripts}: {args:?}");
            assert!(!log.exists(), "{scripts}: ran with {args:?}");
        }
        // A worktree that can't be made (a file in its way) is said, the other PR still runs, and
        // verify-seq exits 1: never a PR skipped with exit 0 (evidence b588cf3bd3a6).
        let blocker = scratch.join(format!("verify-{first}"));
        std::fs::write(&blocker, "in the way\n").unwrap();
        let out = seq_run(&["--split", &a, &b]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(
            stderr.contains("couldn't make a worktree for aaa111") && stderr.contains("not run: aaa111"),
            "{scripts}: {stderr}"
        );
        let lines = std::fs::read_to_string(&log).unwrap();
        assert!(lines.contains("start bbb222") && !lines.contains("start aaa111"), "{scripts}: {lines}");
        std::fs::remove_file(&blocker).unwrap();
        // A run that records nothing (bk-evidence refused, say) is a PR not run: exit 1, naming it
        // (finding f764c35b1147).
        let silent = tmp.path().join("log.silent-aaa111");
        std::fs::write(&silent, "").unwrap();
        let out = seq_run(&["--split", &a, &b]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("not run: aaa111") && !stderr.contains("bbb222"), "{scripts}: {stderr}");
        std::fs::remove_file(&silent).unwrap();
    }
}

/// Finding `87cfdf4680c7`: a Ctrl-C during `verify-seq --split -j 2` stops both runs. A stub `boat` and
/// `cargo` stand in for the box and the tests here, each running 10 minutes: once both boxes and both
/// local runs have started, SIGINT goes to verify-seq's process group, as a terminal's Ctrl-C would. Both
/// boxes are deleted, no stub process is left, no evidence is recorded, no worktree is left, and it exits 130.
#[test]
fn verify_seq_ctrl_c_under_two_jobs_deletes_both_boxes_and_records_nothing() {
    verify_seq_interrupted(false);
}

/// Note `c1468539ea94`: a `verify-seq` started with SIGINT ignored, as a plain background job is, still
/// stops on a SIGTERM sent to it alone. It passes SIGTERM on as SIGTERM, and every part ends with nothing
/// left, as on a Ctrl-C.
#[test]
fn verify_seq_sigterm_with_sigint_ignored_still_stops_both_runs() {
    verify_seq_interrupted(true);
}

/// Reads `from` to its end on a thread of its own.
fn drain(mut from: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut all = Vec::new();
        from.read_to_end(&mut all).unwrap();
        all
    })
}

/// verify-seq under test, stopped on every exit path: dropped, as a failed assertion unwinds, it kills what
/// is left of it, so nothing outlives the test and later finds the real boat on PATH once the stubs'
/// directory is gone (task 1a85a68533d7). verify-seq's jobs run in process groups of their own (`set -m`),
/// so it kills, with SIGKILL: verify-seq's group while it is unreaped or still has members, every process
/// descended from it while it is unreaped, and each stub process recorded in `pids` that is still a
/// `sleep 600`, with its group. Never this test's own group.
struct Stopped {
    child: std::process::Child,
    pids: PathBuf,
    reaped: bool,
}

impl Stopped {
    /// Waits up to `limit` for verify-seq to exit, reaping it.
    fn wait(&mut self, limit: std::time::Duration) -> Option<std::process::ExitStatus> {
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.reaped = true;
                return Some(status);
            }
            if started.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn stop(&mut self) {
        let ps = Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,pgid=,args="]).output();
        let ps = ps.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let rows: Vec<(u32, u32, u32, String)> = ps
            .lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let mut n = || f.next()?.parse::<u32>().ok();
                let (pid, ppid, pgid) = (n()?, n()?, n()?);
                Some((pid, ppid, pgid, l.split_whitespace().skip(3).collect::<Vec<_>>().join(" ")))
            })
            .collect();
        let own = rows.iter().find(|r| r.0 == std::process::id()).map(|r| r.2);
        let id = self.child.id();
        let mut targets: Vec<String> = Vec::new();
        if !self.reaped || rows.iter().any(|r| r.2 == id) {
            targets.push(format!("-{id}"));
        }
        let mut tree = if self.reaped { Vec::new() } else { vec![id] };
        let mut i = 0;
        while i < tree.len() {
            let children: Vec<u32> =
                rows.iter().filter(|r| r.1 == tree[i] && !tree.contains(&r.0)).map(|r| r.0).collect();
            tree.extend(children);
            i += 1;
        }
        let recorded = std::fs::read_to_string(&self.pids).unwrap_or_default();
        for r in &rows {
            let stub = recorded.lines().any(|p| p.trim() == r.0.to_string()) && r.3 == "sleep 600";
            if stub || tree.contains(&r.0) {
                targets.push(r.0.to_string());
                if Some(r.2) != own {
                    targets.push(format!("-{}", r.2));
                }
            }
        }
        targets.sort();
        targets.dedup();
        for target in targets {
            let _ =
                Command::new("/bin/kill").args(["-KILL", "--", &target]).stderr(std::process::Stdio::null()).status();
        }
        if !self.reaped {
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Two split runs under `verify-seq -j 2`, then a Ctrl-C to its group or, with `term`, SIGINT ignored
/// and a SIGTERM to `verify-seq` only.
fn verify_seq_interrupted(term: bool) {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let names = ["verify-seq", "bk-evidence", "test-table", "code-scope"];
        let copies: Vec<(String, PathBuf)> = names.iter().map(|n| (format!("scripts/{n}"), file(n))).collect();
        let files: Vec<(&str, &Path)> = copies.iter().map(|(to, from)| (to.as_str(), from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            write_executable(&dir.join("scripts/ground-guards"), "#!/bin/sh\nexit 0\n");
            put(dir, "code.txt", "code\n");
        });
        let git = |args: &[&str]| {
            let out = run(&dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        bk(&dir, &["init", "--name", "ctrl-c"]);
        bk(&dir, &["id", "new", "manager"]);
        let mut pairs = Vec::new();
        for (n, branch) in ["one", "two"].iter().enumerate() {
            git(&["switch", "-q", "-c", branch, "main"]);
            put(&dir, "code.txt", &format!("change {branch}\n"));
            commit_all(&dir, branch);
            let sha = git(&["rev-parse", "HEAD"]);
            let claim = bk(
                &dir,
                &["claim", &format!("PR #{} {branch} @ {sha}: a change", n + 1), "--tag", "pr", "--as", "manager"],
            );
            pairs.push(format!("{}:{sha}", &claim[..12]));
        }
        git(&["switch", "-q", "main"]);
        let t = tmp.path().join("t");
        std::fs::create_dir_all(&t).unwrap();
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        // boat run: a box per run, live while $T/live-<id> exists, which an interrupt deletes as boat does;
        // its sleep's pid goes to $T/pids. list and delete as boat's, over the live files.
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) id=bx_s$$; echo \"$id ready in 1.0s\"; touch \"$T/live-$id\" \"$T/box-$id\"\n\
               sleep 600 & s=$!; echo $s >>\"$T/pids\"\n\
               trap 'kill $s; echo \"interrupted: deleting $id\"; rm -f \"$T/live-$id\"; exit 130' INT TERM\n\
               wait $s; exit 0 ;;\n\
             list) printf '{\"sandboxes\":['; for f in \"$T\"/live-*; do [ -e \"$f\" ] && printf '{\"id\": \"%s\"}' \"${f##*/live-}\"; done; echo ']}' ;;\n\
             delete) echo \"$*\" >>\"$T/deleted\"; rm -f \"$T/live-$3\" ;;\n\
             esac\n",
        );
        // cargo: says nextest is there; otherwise the tests here run 10 minutes, their pid in $T/pids.
        write_executable(
            &stubs.join("cargo"),
            "#!/bin/bash\n[ \"$*\" != 'nextest --version' ] || exit 0\necho $$ >>\"$T/pids\"; touch \"$T/here-$$\"\nexec sleep 600\n",
        );
        let count = |prefix: &str| {
            std::fs::read_dir(&t)
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(prefix))
                .count()
        };
        let mut args = vec!["--split", "-j", "2"];
        args.extend(pairs.iter().map(String::as_str));
        let mut c = if term {
            let mut wrapped = vec!["-c", "trap '' INT; exec scripts/verify-seq \"$@\"", "verify-seq"];
            wrapped.extend(&args);
            command(&dir, "bash", &wrapped)
        } else {
            command(&dir, "scripts/verify-seq", &args)
        };
        c.env("PATH", format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap()))
            .env("T", &t)
            .env("TMPDIR", tmp.path())
            .env_remove("ON");
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let child = c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let mut child = Stopped { child, pids: t.join("pids"), reaped: false };
        // Read as it comes, so a full pipe can't hold verify-seq up before the runs start (note a3d37148ea05).
        let (stdout, stderr) = (drain(child.child.stdout.take().unwrap()), drain(child.child.stderr.take().unwrap()));
        let text = |stdout: std::thread::JoinHandle<Vec<u8>>, stderr: std::thread::JoinHandle<Vec<u8>>| {
            let (o, e) = (stdout.join().unwrap(), stderr.join().unwrap());
            format!("{}{}", String::from_utf8_lossy(&o), String::from_utf8_lossy(&e))
        };
        // A loaded machine can take minutes to start both; the limit stays below the stubs' 10 minutes, and
        // verify-seq ending first fails at once with what it said.
        let started = std::time::Instant::now();
        while count("box-") < 2 || count("here-") < 2 {
            if let Some(status) = child.wait(std::time::Duration::ZERO) {
                // Stopped first: what's left would hold the output open.
                child.stop();
                panic!("{scripts}: verify-seq ended ({status}) before both runs started: {}", text(stdout, stderr));
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(480), "{scripts}: the runs never started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let id = child.child.id();
        let target = if term { id.to_string() } else { format!("-{id}") };
        let signal = if term { "-TERM" } else { "-INT" };
        assert!(Command::new("/bin/kill").args([signal, "--", &target]).status().unwrap().success());
        let interrupted = std::time::Instant::now();
        let status = child.wait(std::time::Duration::from_secs(120));
        let took = interrupted.elapsed();
        // What's still running is noted before anything left is stopped, which would hold the output open.
        let pids = std::fs::read_to_string(t.join("pids")).unwrap();
        let mut left = Vec::new();
        for pid in pids.lines() {
            let alive =
                Command::new("/bin/kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().unwrap();
            let what = Command::new("/bin/ps").args(["-o", "pid,ppid,pgid,args", "-p", pid]).output().unwrap();
            if alive.success() {
                left.push(String::from_utf8_lossy(&what.stdout).into_owned());
            }
        }
        child.stop();
        let text = text(stdout, stderr);
        let status = status.unwrap_or_else(|| panic!("{scripts}: took over {took:?} to stop: {text}"));
        assert_eq!(status.code(), Some(130), "{scripts}: {text}");
        assert_eq!(count("live-"), 0, "{scripts}: a box was left: {text}");
        assert!(left.is_empty(), "{scripts}: left running: {left:?}: {text}");
        let evidence = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        assert_eq!(evidence, "[]", "{scripts}: recorded evidence for an interrupted run: {text}");
        let worktrees = git(&["worktree", "list"]);
        assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");
    }
}

/// verify-seq checks the free space before any run: below VERIFY_SEQ_MIN_GB it refuses, and below
/// VERIFY_SEQ_WARN_GB it warns and carries on (here to its usage, given no heads).
#[test]
fn verify_seq_refuses_below_the_free_space_floor_and_warns_below_the_warning() {
    for scripts in SCRIPTS {
        let seq = repo_root().join(scripts).join("verify-seq");
        let (_tmp, dir) = repo(&[("scripts/verify-seq", &seq)], ".bracken/\n", |_| {});
        let huge = "999999999";
        let out = command(&dir, "scripts/verify-seq", &["abc123:0"]).env("VERIFY_SEQ_MIN_GB", huge).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("GB free, below 999999999 GB") && stderr.contains("cleanup"), "{scripts}: {stderr}");
        let out = command(&dir, "scripts/verify-seq", &[])
            .env("VERIFY_SEQ_MIN_GB", "0")
            .env("VERIFY_SEQ_WARN_GB", huge)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{scripts}: {stderr}");
        assert!(stderr.contains("warning: only") && stderr.contains("usage"), "{scripts}: {stderr}");
    }
}
