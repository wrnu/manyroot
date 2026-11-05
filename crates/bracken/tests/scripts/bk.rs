//! scripts/bk: its refusals, and its writes confirmed through the team server. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// Network-sync step 3 end to end, on throwaway Grounds: `team-server` sets up, issues tokens and
/// starts the server; alice writes in one replica and bob in another, through `bk`, and each sees
/// the other's write. With the server stopped, a read falls back to the local replica, a write
/// fails unconfirmed, and `merge` refuses to decide.
///
/// bracken runs through a wrapper that logs each `sync`'s arguments, fails one sync while the file
/// `fail-once` exists, and holds back `serve` for 2 seconds while `slow-serve` exists (a slow load).
#[test]
fn team_server_bk_confirms_writes_and_the_gate_fails_closed() {
    let real = env!("CARGO_BIN_EXE_bracken");
    for scripts in SCRIPTS {
        let copy = |name: &str| repo_root().join(scripts).join(name);
        let files = [
            ("scripts/bk", copy("bk")),
            ("scripts/bk-evidence", copy("bk-evidence")),
            ("scripts/team-server", copy("team-server")),
            ("scripts/merge", copy("merge")),
        ];
        let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
        let (tmp, a) = repo(&files, ".bracken/\n.bracken-team\n", |_| {});
        let team = tmp.path().join("team");
        // merge's tools; the gate's pull check comes before either is used.
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        for tool in ["curl", "jq"] {
            write_executable(&tmp.path().join("stubs").join(tool), "#!/bin/sh\nexit 1\n");
        }
        let path = format!("{}:{}", tmp.path().join("stubs").display(), std::env::var("PATH").unwrap());
        let wrapper = tmp.path().join("bracken-logged");
        write_executable(
            &wrapper,
            "#!/usr/bin/env bash\n\
             if [ \"$1\" = sync ]; then\n  echo \"$*\" >>\"$T/sync.log\"\n\
               if [ -e \"$T/fail-once\" ]; then rm -f \"$T/fail-once\"; echo refused >&2; exit 1; fi\nfi\n\
             [ \"$1\" != watch ] || echo \"$*\" >>\"$T/watch.log\"\n\
             [ \"${3:-}\" != serve ] || [ ! -e \"$T/slow-serve\" ] || sleep 2\n\
             exec \"$REAL_BRACKEN\" \"$@\"\n",
        );
        let syncs = || std::fs::read_to_string(tmp.path().join("sync.log")).unwrap_or_default();
        // The wrapper execs the real binary: team-server checks the pid it stops is the server's.
        let sh = |dir: &Path, program: &str, args: &[&str], agent: Option<&str>| {
            let mut c = command(dir, program, args);
            c.env("BRACKEN_BIN", &wrapper).env("T", tmp.path()).env("PATH", &path);
            if let Some(agent) = agent {
                c.env("BRACKEN_AGENT", agent);
            }
            c.output().unwrap()
        };
        let ok = |out: Output, what: &str| {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(out.status.success(), "{scripts}: {what} failed: {stderr}");
            (String::from_utf8(out.stdout).unwrap(), stderr)
        };
        struct Stop<'a>(&'a dyn Fn());
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                (self.0)()
            }
        }
        let stop = || {
            sh(&a, "scripts/team-server", &["stop"], None);
        };
        let _stop = Stop(&stop);

        ok(sh(&a, "scripts/bk", &["init", "--name", "team"], None), "init");
        for who in ["alice", "bob", "carol", "manager"] {
            ok(sh(&a, "scripts/bk", &["id", "new", who], None), "id new");
        }
        // Bob's replica: a clone of the repo with its own copy of the Ground and of bob's key.
        let b = tmp.path().join("b");
        assert!(run(tmp.path(), "git", &["clone", "-q", a.to_str().unwrap(), "b"]).status.success());
        let bundle = tmp.path().join("ground.bundle");
        ok(sh(&a, real, &["--dir", ".bracken", "export", bundle.to_str().unwrap()], None), "export");
        ok(sh(&b, real, &["--dir", ".bracken", "join", bundle.to_str().unwrap()], None), "join");
        std::fs::create_dir_all(b.join(".bracken/identities")).unwrap();
        std::fs::copy(a.join(".bracken/identities/bob.json"), b.join(".bracken/identities/bob.json")).unwrap();

        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
        ok(sh(&a, "scripts/team-server", &["setup", team.to_str().unwrap(), &port], None), "setup");
        assert!(!team.join("seed.bundle").exists(), "{scripts}: setup left its seed bundle");
        std::fs::copy(a.join(".bracken-team"), b.join(".bracken-team")).unwrap();
        for who in ["alice", "bob", "manager"] {
            ok(sh(&a, "scripts/team-server", &["token", who], None), "token");
            let mode = std::fs::metadata(team.join("tokens").join(who)).unwrap().permissions();
            assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600, "{scripts}: token mode");
        }
        let (out, stderr) = ok(sh(&a, "scripts/team-server", &["start"], None), "start");
        assert!(out.contains(&format!("listening on http://127.0.0.1:{port}")), "{scripts}: {out}");
        assert!(stderr.is_empty(), "{scripts}: the first start said {stderr:?}");

        // Writes push and confirm; stdout stays bracken's one labelled line for the new claim.
        let before = syncs().lines().count();
        let (alice, _) = ok(sh(&a, "scripts/bk", &["claim", "from alice", "--as", "alice"], None), "alice's claim");
        let new: Vec<String> = syncs().lines().skip(before).map(String::from).collect();
        assert!(new.len() == 1 && new[0].contains(" --confirm"), "{scripts}: alice's write synced as {new:?}");
        let (bob, _) = ok(sh(&b, "scripts/bk", &["claim", "from bob", "--as", "bob"], None), "bob's claim");
        for out in [&alice, &bob] {
            assert!(out.starts_with("claim ") && out.trim().lines().count() == 1, "{scripts}: {out:?}");
        }
        let (alice, bob) = (common::bare_ids(alice.trim().to_owned()), common::bare_ids(bob.trim().to_owned()));
        for id in [&alice, &bob] {
            assert!(id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()), "{scripts}: {id:?}");
        }
        // bk refuses sync with any other peer, but not with the team server (finding 6d623aa585d9).
        let url = std::fs::read_to_string(team.join("url")).unwrap();
        let token = team.join("tokens/alice");
        let args = ["sync", url.trim(), "--token-file", token.to_str().unwrap()];
        ok(sh(&a, "scripts/bk", &args, Some("alice")), "sync with the team server");
        // Each sees the other's: alice's replica learns bob's claim only by pulling.
        let (shown, stderr) = ok(sh(&a, "scripts/bk", &["show", bob.trim()], Some("alice")), "alice reads");
        assert!(shown.contains("from bob") && stderr.is_empty(), "{scripts}: {shown} {stderr}");
        let (shown, _) = ok(sh(&b, "scripts/bk", &["show", alice.trim()], Some("bob")), "bob reads");
        assert!(shown.contains("from alice"), "{scripts}: {shown}");
        // watch is a read: it pulls first. With --follow it syncs again on its interval, so a write from
        // the other replica shows up while it runs.
        let (out, _) = ok(sh(&a, "scripts/bk", &["watch"], Some("alice")), "alice watches");
        let at = out.trim().strip_prefix("cursor: ").unwrap_or_else(|| panic!("{scripts}: {out}")).to_owned();
        let (late, _) = ok(sh(&b, "scripts/bk", &["claim", "while alice watches", "--as", "bob"], None), "bob");
        let late = common::bare_ids(late.trim().to_owned());
        let (out, stderr) = ok(sh(&a, "scripts/bk", &["watch", "--since", &at], Some("alice")), "alice watches");
        assert!(out.starts_with(&late[..8]) && stderr.is_empty(), "{scripts}: {out} {stderr}");
        let mut follow = command(&a, "scripts/bk", &["watch", "--follow", "--interval", "2s", "--json"]);
        follow.env("BRACKEN_BIN", &wrapper).env("T", tmp.path()).env("PATH", &path).env("BRACKEN_AGENT", "alice");
        let mut follow = follow.stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut lines = std::io::BufRead::lines(std::io::BufReader::new(follow.stdout.take().unwrap()));
        let first = lines.next().unwrap().unwrap();
        assert!(first.starts_with("{\"cursor\""), "{scripts}: {first}");
        let (later, _) = ok(sh(&b, "scripts/bk", &["claim", "while alice follows", "--as", "bob"], None), "bob");
        let later = common::bare_ids(later.trim().to_owned());
        let next = lines.next().unwrap().unwrap();
        follow.kill().unwrap();
        follow.wait().unwrap();
        assert!(next.contains(&later) && next.contains("while alice follows"), "{scripts}: {next}");
        // It did so on the team server's change feed (task 09bf8248e69b): bk gives --feed to a bracken
        // that has it.
        let watches = std::fs::read_to_string(tmp.path().join("watch.log")).unwrap();
        let followed = watches.lines().rfind(|l| l.contains("--follow")).unwrap_or_else(|| panic!("{watches}"));
        assert!(followed.contains(" --sync ") && followed.ends_with(" --feed"), "{scripts}: {followed}");
        // Only bk gives watch its peer, the team server: another would bring in unconfirmed work.
        for peer in [&["--sync", "http://127.0.0.1:1"][..], &["--token-file=/dev/null"], &["--feed"]] {
            let args = [&["watch", "--follow"][..], peer].concat();
            for agent in [Some("alice"), Some("carol")] {
                let out = sh(&a, "scripts/bk", &args, agent);
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(!out.status.success() && stderr.contains("refused"), "{scripts}: {args:?}: {stderr}");
            }
        }
        // Without a team token, watch --follow says at once that it only reads the local replica.
        let mut local = command(&a, "scripts/bk", &["watch", "--follow", "--interval", "2s"]);
        local.env("BRACKEN_BIN", &wrapper).env("T", tmp.path()).env("PATH", &path).env("BRACKEN_AGENT", "carol");
        let mut local = local.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let said = std::io::BufRead::lines(std::io::BufReader::new(local.stderr.take().unwrap())).next();
        local.kill().unwrap();
        local.wait().unwrap();
        let said = said.unwrap().unwrap();
        assert!(said.contains("won't sync") && said.contains("only the local replica"), "{scripts}: {said}");
        // An intent is a write too (task 065b221bfdad): pushed and confirmed like a claim, so the other
        // replica sees it on its next read.
        let before = syncs().lines().count();
        let (intent, _) = ok(sh(&b, "scripts/bk", &["intend", &alice, "--as", "bob"], None), "bob's intent");
        let new: Vec<String> = syncs().lines().skip(before).map(String::from).collect();
        assert!(new.len() == 1 && new[0].contains(" --confirm"), "{scripts}: bob's intent synced as {new:?}");
        let intent = common::bare_ids(intent.trim().to_owned());
        let intent = intent.lines().next().unwrap();
        let (shown, _) = ok(sh(&a, "scripts/bk", &["show", intent], Some("alice")), "alice reads the intent");
        assert!(shown.contains("intent: working on"), "{scripts}: {shown}");
        // An --as after '--' is the command's, not bk's: the write pushes with the signer's token (note
        // 6b211180ec3a).
        let before = syncs().lines().count();
        let run = ["evidence", &alice[..12], "--run", "--as", "bob", "--", "/usr/bin/true", "--as", "alice"];
        ok(sh(&b, "scripts/bk", &run, None), "bob's evidence --run");
        let new: Vec<String> = syncs().lines().skip(before).map(String::from).collect();
        assert!(new.len() == 1 && new[0].contains("tokens/bob "), "{scripts}: pushed with {new:?}");
        // bk-evidence's reads and its write go through the server too.
        let evidence = [&alice[..12], "--as", "bob", "--any-head", "--", "/usr/bin/true"];
        let (_, stderr) = ok(sh(&b, "scripts/bk-evidence", &evidence, None), "bk-evidence");
        assert!(!stderr.contains("not pulled"), "{scripts}: bk-evidence's reads didn't pull: {stderr}");
        let (shown, _) = ok(sh(&a, "scripts/bk", &["show", alice.trim()], Some("alice")), "alice reads evidence");
        assert!(shown.contains("pass") && shown.contains("bob"), "{scripts}: {shown}");
        // An identity without a token can't write: nothing is written.
        let out = sh(&a, "scripts/bk", &["claim", "from carol", "--as", "carol"], None);
        assert_eq!(out.status.code(), Some(1), "{scripts}: carol wrote without a token");
        let (log, _) = ok(sh(&a, "scripts/bk", &["log"], Some("alice")), "log");
        assert!(!log.contains("from carol"), "{scripts}: {log}");
        // A relay isn't refused (docs/design/agent-keys.md, part A step 1): carol's contribution, written
        // in alice's replica past bk, goes to the server with alice's next write, under alice's
        // credential, and the first-seen log records that for scripts/ground-guards to report.
        let (relayed, _) = ok(sh(&a, real, &["--dir", ".bracken", "claim", "relayed", "--as", "carol"], None), "carol");
        let relayed = common::bare_ids(relayed.trim().to_owned());
        ok(sh(&a, "scripts/bk", &["claim", "relaying", "--as", "alice"], Some("alice")), "alice relays");
        let (ids, _) = ok(sh(&a, "scripts/bk", &["id", "list", "--json"], None), "id list");
        let ids: serde_json::Value = serde_json::from_str(&ids).unwrap();
        let carol = ids.as_array().unwrap().iter().find(|i| i["name"] == "carol").unwrap()["agent"].clone();
        let seen = std::fs::read_to_string(team.join("first-seen.jsonl")).unwrap();
        let entry: serde_json::Value = seen
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|e| e["id"] == relayed.trim())
            .unwrap_or_else(|| panic!("{scripts}: the relayed claim isn't in the first-seen log"));
        assert!(entry["credential"] == "alice" && entry["author"] == carol, "{scripts}: {entry}");

        // A confirm that fails once is retried, and the write confirms.
        std::fs::write(tmp.path().join("fail-once"), "").unwrap();
        let before = syncs().lines().count();
        ok(sh(&a, "scripts/bk", &["claim", "retried", "--as", "alice"], None), "retried claim");
        let new: Vec<String> = syncs().lines().skip(before).map(String::from).collect();
        assert!(new.len() == 2 && new.iter().all(|l| l.contains(" --confirm")), "{scripts}: {new:?}");

        // A token for a new identity restarts the running server, so the identity can write at once.
        ok(sh(&a, "scripts/bk", &["id", "new", "dave"], None), "id new dave");
        let (out, _) = ok(sh(&a, "scripts/team-server", &["token", "dave"], None), "token dave");
        assert!(out.contains("restarting the server"), "{scripts}: {out}");
        ok(sh(&a, "scripts/bk", &["claim", "from dave", "--as", "dave"], None), "dave's claim");
        ok(sh(&a, "scripts/bk", &["claim", "after dave", "--as", "alice"], None), "alice after dave");

        // start returns only once the server answers, even after a stop and with a slow load.
        ok(sh(&a, "scripts/team-server", &["stop"], None), "stop");
        std::fs::write(tmp.path().join("slow-serve"), "").unwrap();
        ok(sh(&a, "scripts/team-server", &["start"], None), "slow start");
        std::fs::remove_file(tmp.path().join("slow-serve")).unwrap();
        let mut heads = command(&a, "scripts/bk", &["heads"]);
        heads
            .env("BRACKEN_BIN", &wrapper)
            .env("T", tmp.path())
            .env("BRACKEN_AGENT", "alice")
            .env("BK_PULL", "required");
        ok(heads.output().unwrap(), "heads right after start");

        // With the server up, merge gets past its pull (and stops later, at the missing origin).
        let merge = ["1", &alice[..12], "--validator", "alice", "--min-passes", "1", "--dry-run"];
        let out = sh(&a, "scripts/merge", &merge, None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && !stderr.contains("team server"), "{scripts}: {stderr}");
        let said = format!("team server: every read pulls from http://127.0.0.1:{port}\n");
        assert!(String::from_utf8_lossy(&out.stdout).contains(&said), "{scripts}: {:?}", out.stdout);

        // run, launchd's way: the server in the foreground, as the process itself. check waits for it to
        // answer, start sees it running, and stop stops it by the pid run wrote.
        ok(sh(&a, "scripts/team-server", &["stop"], None), "stop before run");
        let mut foreground = command(&a, "scripts/team-server", &["run"]);
        foreground.env("BRACKEN_BIN", &wrapper).env("T", tmp.path()).env("PATH", &path);
        let mut foreground = foreground.spawn().unwrap();
        // Reaped as it exits, as launchd would, so stop doesn't wait on a zombie.
        let foreground = std::thread::spawn(move || foreground.wait().unwrap());
        let (out, _) = ok(sh(&a, "scripts/team-server", &["check"], None), "check");
        assert!(out.contains(&format!("listening on http://127.0.0.1:{port}")), "{scripts}: {out}");
        let out = sh(&a, "scripts/team-server", &["start"], None);
        assert!(!out.status.success(), "{scripts}: start beside run");
        // A second run beside it exits cleanly, so launchd stops retrying.
        let out = sh(&a, "scripts/team-server", &["run"], None);
        assert!(out.status.success(), "{scripts}: a second run failed");
        assert!(String::from_utf8_lossy(&out.stderr).contains("already running"), "{scripts}");
        ok(sh(&a, "scripts/team-server", &["stop"], None), "stop run");
        assert!(!foreground.join().unwrap().success(), "{scripts}: run's server ended by its signal");

        let (shown, stderr) = ok(sh(&a, "scripts/bk", &["show", bob.trim()], Some("alice")), "offline read");
        assert!(shown.contains("from bob"), "{scripts}: {shown}");
        assert!(stderr.contains("not pulled from the team server"), "{scripts}: {stderr}");
        let out = sh(&a, "scripts/bk", &["claim", "offline", "--as", "alice"], None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: an unconfirmed write passed");
        assert!(stderr.contains("didn't confirm"), "{scripts}: {stderr}");
        let out = sh(&a, "scripts/merge", &merge, None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("can't pull from the team server"), "{scripts}: {stderr}");
    }
}

/// docs/design/agent-keys.md, part A step 1: with BRACKEN_AGENT set, scripts/bk refuses an --as of its own
/// that names another identity, in either form and wherever it stands before '--', and runs nothing, for
/// a write and a read alike. The same identity, no BRACKEN_AGENT, an empty one, and an --as after '--'
/// (the command's own) all go through.
#[test]
fn bk_refuses_an_as_that_differs_from_bracken_agent() {
    for scripts in SCRIPTS {
        let bk_copy = repo_root().join(scripts).join("bk");
        let (_tmp, dir) = repo(&[("scripts/bk", &bk_copy)], ".bracken/\n", |_| {});
        bk(&dir, &["init", "--name", "team"]);
        bk(&dir, &["id", "new", "alice"]);
        bk(&dir, &["id", "new", "bob"]);
        let as_agent = |agent: Option<&str>, args: &[&str]| {
            let mut c = command(&dir, "scripts/bk", args);
            if let Some(agent) = agent {
                c.env("BRACKEN_AGENT", agent);
            }
            c.output().unwrap()
        };
        let target = common::bare_ids(bk(&dir, &["claim", "a target", "--as", "alice"]).trim().to_owned());
        let before = bk(&dir, &["log", "--json"]);
        for args in [
            &["claim", "as bob", "--as", "bob"][..],
            &["claim", "as bob", "--as=bob"],
            &["--as", "bob", "claim", "as bob"],
            &["claim", "as bob", "--as", "alice", "--as", "bob"],
            &["log", "--as", "bob"],
        ] {
            let out = as_agent(Some("alice"), args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}: {stderr}");
            assert!(stderr.contains("differs from BRACKEN_AGENT (alice)"), "{scripts}: {args:?}: {stderr}");
            assert!(out.stdout.is_empty(), "{scripts}: {args:?} ran");
        }
        assert_eq!(bk(&dir, &["log", "--json"]), before, "{scripts}: a refused --as wrote");
        for (agent, args) in [
            (Some("alice"), &["claim", "as alice", "--as", "alice"][..]),
            (Some("alice"), &["claim", "as alice", "--as=alice"]),
            (None, &["claim", "as bob", "--as", "bob"]),
            (Some(""), &["claim", "as bob", "--as", "bob"]),
            (Some("bob"), &["evidence", &target[..12], "--run", "--as", "bob", "--", "/usr/bin/true", "--as", "alice"]),
        ] {
            let out = as_agent(agent, args);
            assert!(out.status.success(), "{scripts}: {agent:?} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

/// Finding 410e2e5d0239: scripts/bk refuses import, which would add a bundle (other authors' work
/// included) to the team's Ground with no confirmation, and points at a throwaway Ground. Nothing is
/// written, with or without a team server set up.
#[test]
fn bk_refuses_import_and_writes_nothing() {
    for scripts in SCRIPTS {
        let bk_copy = repo_root().join(scripts).join("bk");
        let (tmp, dir) = repo(&[("scripts/bk", &bk_copy)], ".bracken/\n", |_| {});
        bk(&dir, &["init", "--name", "team"]);
        bk(&dir, &["id", "new", "alice"]);
        bk(&dir, &["claim", "ours", "--as", "alice"]);
        // A bundle from a copy of the Ground with a claim the team's doesn't have.
        let seed = tmp.path().join("seed.mrb");
        bk(&dir, &["export", seed.to_str().unwrap()]);
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let real = |args: &[&str]| {
            let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
                .args(args)
                .current_dir(&other)
                .env_remove("BRACKEN_DIR")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        real(&["join", seed.to_str().unwrap()]);
        real(&["id", "new", "bob"]);
        real(&["claim", "theirs", "--as", "bob"]);
        let bundle = tmp.path().join("other.mrb");
        real(&["export", bundle.to_str().unwrap()]);
        let before = bk(&dir, &["log", "--json"]);
        for args in [&["import", bundle.to_str().unwrap()][..], &["--as", "alice", "import", bundle.to_str().unwrap()]]
        {
            let out = run(&dir, "scripts/bk", args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}: {stderr}");
            assert!(stderr.contains("import is refused") && stderr.contains("mktemp -d"), "{scripts}: {stderr}");
        }
        assert_eq!(bk(&dir, &["log", "--json"]), before, "{scripts}: import wrote");
    }
}

/// Weave build step 4 (docs/design/weave.md, Keeping the two scripts apart) and task 5fd7de6b61b6:
/// scripts/bk refuses --dir in both forms, the weave command found past --as, an artifact with a Weave
/// media type in either form and any case, and sync with a replica directory or --exec, writing and
/// receiving nothing. Other artifacts still go through.
#[test]
fn bk_refuses_dir_weave_weave_media_types_and_sync_with_other_peers() {
    for scripts in SCRIPTS {
        let bk_copy = repo_root().join(scripts).join("bk");
        let (tmp, dir) = repo(&[("scripts/bk", &bk_copy)], ".bracken/\n", |_| {});
        bk(&dir, &["init", "--name", "team"]);
        bk(&dir, &["id", "new", "alice"]);
        std::fs::write(dir.join("doc.md"), "a doc\n").unwrap();
        // A replica directory holding a claim the team's Ground doesn't have.
        let seed = tmp.path().join("seed.mrb");
        bk(&dir, &["export", seed.to_str().unwrap()]);
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let real = |args: &[&str]| {
            let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
                .args(args)
                .current_dir(&other)
                .env_remove("BRACKEN_DIR")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        real(&["join", seed.to_str().unwrap()]);
        real(&["id", "new", "bob"]);
        real(&["claim", "theirs", "--as", "bob"]);
        let other_ground = other.join(".bracken");
        let prog = tmp.path().join("prog");
        write_executable(&prog, "#!/bin/sh\ntouch \"$0.ran\"\n");
        let peer_exec =
            format!("BRACKEN_DIR={} {} serve --stdio", other_ground.display(), env!("CARGO_BIN_EXE_bracken"));
        let before = bk(&dir, &["log", "--json"]);
        let cases: Vec<(Vec<&str>, &str)> = vec![
            (vec!["--dir", other_ground.to_str().unwrap(), "log"], "--dir is refused"),
            (vec!["log", "--dir=/tmp/elsewhere"], "--dir is refused"),
            (vec!["weave", "check"], "weave is refused"),
            (vec!["--as", "alice", "weave", "import"], "weave is refused"),
            (vec!["--as=alice", "weave", "init"], "weave is refused"),
            (
                vec![
                    "artifact",
                    "doc.md",
                    "--media-type",
                    "application/vnd.bracken.weave.manifest+cbor",
                    "--as",
                    "alice",
                ],
                "a Weave media type is refused",
            ),
            (
                vec!["--as", "alice", "artifact", "doc.md", "--media-type=APPLICATION/VND.BRACKEN.WEAVE.ground+cbor"],
                "a Weave media type is refused",
            ),
            (
                vec!["artifact", "doc.md", "--media-type", "Application/Vnd.Bracken.Weave.line+cbor", "--as", "alice"],
                "a Weave media type is refused",
            ),
            (vec!["sync", other.to_str().unwrap()], "sync is refused"),
            (vec!["sync", "--deadline", "5", other_ground.to_str().unwrap()], "sync is refused"),
            (
                vec!["--as", "alice", "sync", "--exec", env!("CARGO_BIN_EXE_bracken"), "serve", "--stdio"],
                "sync is refused",
            ),
            (vec!["sync", "--exec=/bin/true"], "sync is refused"),
            // Findings 26650dedc33e and 3e1a0454a03b: a trailing -h is --exec's program's argument.
            (vec!["sync", "--exec", prog.to_str().unwrap(), "-h"], "sync is refused"),
            (vec!["sync", "--exec", "sh", "-c", &peer_exec, "-h"], "sync is refused"),
            (vec!["--help", "sync", other.to_str().unwrap()], "sync is refused"),
            (vec!["weave", "-h"], "weave is refused"),
            (vec!["log", "--dir", "x", "--help"], "--dir is refused"),
            (vec!["artifact", "doc.md", "--media-type", "application/vnd.bracken.weave.x", "-h"], "a Weave media type"),
        ];
        for (args, expected) in cases {
            let out = run(&dir, "scripts/bk", &args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}: {stderr}");
            assert!(stderr.contains(expected), "{scripts}: {args:?}: expected {expected:?} in {stderr}");
        }
        assert_eq!(bk(&dir, &["log", "--json"]), before, "{scripts}: a refused command wrote or received");
        assert!(!tmp.path().join("prog.ran").exists(), "{scripts}: --exec's program ran");
        let out = bk(&dir, &["artifact", "doc.md", "--media-type", "text/markdown", "--as", "alice"]);
        assert!(out.len() == 64 && out.bytes().all(|b| b.is_ascii_hexdigit()), "{scripts}: {out}");
        // Help still works for a command that isn't refused.
        assert!(run(&dir, "scripts/bk", &["log", "--help"]).status.success(), "{scripts}: log --help");

        // With a team server configured (none listening): the one peer must be its URL exactly, wherever
        // --as is, and --as's value is never the peer (finding bf1a2d50d9d0).
        let url = "http://127.0.0.1:9";
        let team = tmp.path().join("team");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(team.join("url"), url).unwrap();
        std::fs::write(dir.join(".bracken-team"), team.to_str().unwrap()).unwrap();
        let before = bk(&dir, &["log", "--json"]);
        for args in [
            &["sync", "--as", url, other.to_str().unwrap()][..],
            &["--as", url, "sync", other.to_str().unwrap()],
            &["sync", url, other.to_str().unwrap()],
            &["sync", "--", other.to_str().unwrap()],
            &["sync", "--as=alice", "--token-file", url, other.to_str().unwrap()],
            &["sync", "http://127.0.0.1:9/"],
        ] {
            let out = run(&dir, "scripts/bk", args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {args:?}: {stderr}");
            assert!(stderr.contains("sync is refused"), "{scripts}: {args:?}: {stderr}");
        }
        assert_eq!(bk(&dir, &["log", "--json"]), before, "{scripts}: a refused sync received");
        for args in
            [&["sync", "--as", "alice", url, "--deadline", "1"][..], &["--as=alice", "sync", url, "--deadline=1"]]
        {
            let out = run(&dir, "scripts/bk", args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!stderr.contains("sync is refused"), "{scripts}: {args:?} was refused: {stderr}");
        }
    }
}

/// Part A step 2 end to end (docs/design/agent-keys.md, 4.1 and 4.2): `team-server key` makes a new identity
/// whose key only the team server holds, and bk's writes as it are signed by the server, within its scope,
/// and logged; an identity with a key file still signs from it. `retire` stops the signing.
#[test]
fn team_server_holds_new_identities_keys_and_signs_for_them() {
    let real = env!("CARGO_BIN_EXE_bracken");
    for scripts in SCRIPTS {
        let copy = |name: &str| repo_root().join(scripts).join(name);
        let files = [
            ("scripts/bk", copy("bk")),
            ("scripts/team-server", copy("team-server")),
            ("scripts/bk-evidence", copy("bk-evidence")),
            ("scripts/code-scope", copy("code-scope")),
        ];
        let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
        let (tmp, a) = repo(&files, ".bracken/\n.bracken-team\n", |_| {});
        let team = tmp.path().join("team");
        let sh = |program: &str, args: &[&str], agent: Option<&str>| {
            let mut c = command(&a, program, args);
            c.env("BRACKEN_BIN", real);
            if let Some(agent) = agent {
                c.env("BRACKEN_AGENT", agent);
            }
            c.output().unwrap()
        };
        let ok = |out: Output, what: &str| {
            assert!(out.status.success(), "{scripts}: {what} failed: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        };
        let failed = |out: Output, what: &str, why: &str| {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(!out.status.success() && stderr.contains(why), "{scripts}: {what}: {stderr}");
        };
        struct Stop<'a>(&'a dyn Fn());
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                (self.0)()
            }
        }
        let stop = || {
            sh("scripts/team-server", &["stop"], None);
        };
        let _stop = Stop(&stop);
        let lines = |file: &str| -> Vec<serde_json::Value> {
            let text = std::fs::read_to_string(team.join(file)).unwrap_or_default();
            text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        };

        ok(sh("scripts/bk", &["init", "--name", "team"], None), "init");
        ok(sh("scripts/bk", &["id", "new", "alice"], None), "id new");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
        ok(sh("scripts/team-server", &["setup", team.to_str().unwrap(), &port], None), "setup");
        ok(sh("scripts/team-server", &["token", "alice"], None), "token");
        ok(sh("scripts/team-server", &["start"], None), "start");

        // An older binary, without serve --keys or id new --key-dir: key refuses before changing anything,
        // so the server still restarts under it (finding 0d9dbdbc9f35).
        let old = tmp.path().join("bracken-old");
        write_executable(
            &old,
            "#!/usr/bin/env bash
             for a in \"$@\"; do case $a in --keys|--scopes|--signed|--key-dir) echo \"error: unexpected argument '$a'\" >&2; exit 2 ;; esac; done
             if [[ \" $* \" == *\" --help \"* ]]; then \"$REAL_BRACKEN\" \"$@\" | grep -v -e --keys -e --scopes -e --signed -e --key-dir; exit; fi
             exec \"$REAL_BRACKEN\" \"$@\"
",
        );
        let with_old =
            |args: &[&str]| command(&a, "scripts/team-server", args).env("BRACKEN_BIN", &old).output().unwrap();
        failed(with_old(&["key", "builder-x"]), "key with an old binary", "upgrade the pinned binary first");
        assert!(
            !team.join("keys").exists() && !team.join("scopes.toml").exists(),
            "{scripts}: key changed the team dir"
        );
        assert!(!a.join(".bracken/agents/builder-x.json").exists(), "{scripts}");
        let out = ok(with_old(&["token", "alice"]), "token with an old binary");
        assert!(out.contains("restarting the server"), "{scripts}: {out}");
        ok(sh("scripts/bk", &["claim", "after the old binary's restart"], Some("alice")), "alice");

        // key: the private key in the team directory, the public entry in the main checkout's Ground.
        let out = ok(sh("scripts/team-server", &["key", "builder-x"], None), "key");
        assert!(out.contains("made builder-x") && out.contains("restarting the server"), "{scripts}: {out}");
        assert!(team.join("keys/builder-x.json").exists(), "{scripts}");
        let mode = std::fs::metadata(team.join("keys")).unwrap().permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o700, "{scripts}: keys/ mode");
        assert!(!a.join(".bracken/identities/builder-x.json").exists(), "{scripts}: a key under the working tree");
        let entry = std::fs::read_to_string(a.join(".bracken/agents/builder-x.json")).unwrap();
        assert!(!entry.contains("secret_key"), "{scripts}: {entry}");
        let held = lines("held.jsonl");
        assert!(
            held.len() == 1 && held[0]["name"] == "builder-x" && entry.contains(held[0]["agent"].as_str().unwrap())
        );
        assert!(team.join("scopes.toml").exists() && team.join("tokens/builder-x").exists(), "{scripts}");

        // Its writes are the server's signatures, logged under its credential, and pushed.
        let out = ok(sh("scripts/bk", &["claim", "signed by the server", "--tag", "note"], Some("builder-x")), "claim");
        let id = common::bare_ids(out.trim().to_owned());
        let signed = lines("signed.jsonl");
        assert!(signed.len() == 1 && signed[0]["id"] == id.as_str() && signed[0]["credential"] == "builder-x");
        assert!(lines("first-seen.jsonl").iter().any(|e| e["id"] == id.as_str()), "{scripts}: not pushed");
        // Outside its scope: refused, nothing written.
        let out = sh("scripts/bk", &["claim", "a decision", "--tag", "decision"], Some("builder-x"));
        failed(out, "a decision tag", "not signed for builder-x");
        assert_eq!(lines("signed.jsonl").len(), 1, "{scripts}");
        // An existing identity signs from its key file, and the server logs nothing.
        ok(sh("scripts/bk", &["claim", "signed locally"], Some("alice")), "alice's claim");
        assert_eq!(lines("signed.jsonl").len(), 1, "{scripts}");
        failed(sh("scripts/team-server", &["key", "alice"], None), "key for alice", "already an identity");
        // bk-evidence records for it too: it has no key file here, but it has a team token (note c0346a9af0dc).
        let head = String::from_utf8(run(&a, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
        let out = ok(
            sh("scripts/bk", &["claim", &format!("PR #1 main @ {head}: t"), "--tag", "pr"], Some("builder-x")),
            "pr",
        );
        let pr = common::bare_ids(out.trim().to_owned());
        let ran = tmp.path().join("ran");
        let evidence = |agent: &str| {
            let touch = ["bash", "-c", "touch \"$0\"", ran.to_str().unwrap()];
            sh("scripts/bk-evidence", &[&[&pr[..12], "--as", agent, "--"][..], &touch].concat(), None)
        };
        ok(evidence("builder-x"), "bk-evidence as builder-x");
        assert!(ran.exists(), "{scripts}: the command didn't run");
        let signed = lines("signed.jsonl");
        assert!(signed.len() == 3 && signed[2]["credential"] == "builder-x", "{scripts}: {signed:?}");

        // retire: the server signs nothing more for it, and its token is gone.
        let out = ok(sh("scripts/team-server", &["retire", "builder-x"], None), "retire");
        assert!(out.contains("note it in a claim"), "{scripts}: {out}");
        assert!(team.join("keys/retired/builder-x.json").exists() && !team.join("tokens/builder-x").exists());
        failed(sh("scripts/bk", &["claim", "after retiring"], Some("builder-x")), "after retiring", "no team token");
        failed(sh("scripts/team-server", &["token", "builder-x"], None), "token after retiring", "is retired");
        // Without a token, bk-evidence refuses before it runs anything.
        std::fs::remove_file(&ran).unwrap();
        failed(evidence("builder-x"), "bk-evidence without a token", "no team token here");
        assert!(!ran.exists(), "{scripts}: the command ran");
        failed(sh("scripts/team-server", &["key", "builder-x"], None), "key after retiring", "already an identity");
        ok(sh("scripts/bk", &["claim", "alice still writes"], Some("alice")), "alice after the restart");
        let retired = std::fs::read_to_string(team.join("keys/retired.jsonl")).unwrap();
        assert!(retired.contains("\"name\":\"builder-x\"") && !retired.contains("secret"), "{scripts}: {retired}");
        // Put back by hand, it is still retired: token refuses it (finding ab071f5d3de6), and retire puts it
        // away again (finding 7dc098439105).
        std::fs::rename(team.join("keys/retired/builder-x.json"), team.join("keys/builder-x.json")).unwrap();
        failed(sh("scripts/team-server", &["token", "builder-x"], None), "token after a move back", "is retired");
        ok(sh("scripts/team-server", &["retire", "builder-x"], None), "retire after a move back");
        assert!(team.join("keys/retired/builder-x.json").exists() && !team.join("keys/builder-x.json").exists());
        failed(sh("scripts/team-server", &["retire", "builder-x"], None), "retire again", "already retired");
        // An interrupted retire, stopped after it listed the key: running it again finishes it.
        ok(sh("scripts/team-server", &["key", "builder-y"], None), "key builder-y");
        let held = std::fs::read_to_string(team.join("held.jsonl")).unwrap();
        let y = held.lines().find(|l| l.contains("\"builder-y\"")).unwrap();
        let mut list = std::fs::OpenOptions::new().append(true).open(team.join("keys/retired.jsonl")).unwrap();
        std::io::Write::write_all(&mut list, format!("{y}\n").as_bytes()).unwrap();
        drop(list);
        assert!(team.join("tokens/builder-y").exists() && team.join("keys/builder-y.json").exists());
        ok(sh("scripts/team-server", &["retire", "builder-y"], None), "retire after an interrupted one");
        assert!(team.join("keys/retired/builder-y.json").exists() && !team.join("keys/builder-y.json").exists());
        assert!(!team.join("tokens/builder-y").exists(), "{scripts}");
        let tokens = std::fs::read_to_string(team.join("tokens.b3")).unwrap();
        assert!(!tokens.contains("builder-y "), "{scripts}: {tokens}");
        let listed = std::fs::read_to_string(team.join("keys/retired.jsonl")).unwrap();
        assert_eq!(listed.matches("\"builder-y\"").count(), 1, "{scripts}: {listed}");
        failed(sh("scripts/team-server", &["retire", "builder-y"], None), "retire builder-y again", "already retired");
        // A retire stopped after it rewrote tokens.b3 and before the restart leaves nothing on disk, but the
        // running server still signs with the key and takes the token: running retire again restarts it
        // (finding f6e5238e58db).
        ok(sh("scripts/team-server", &["key", "builder-z"], None), "key builder-z");
        let held = std::fs::read_to_string(team.join("held.jsonl")).unwrap();
        let z = held.lines().find(|l| l.contains("\"builder-z\"")).unwrap();
        let mut list = std::fs::OpenOptions::new().append(true).open(team.join("keys/retired.jsonl")).unwrap();
        std::io::Write::write_all(&mut list, format!("{z}\n").as_bytes()).unwrap();
        drop(list);
        std::fs::rename(team.join("keys/builder-z.json"), team.join("keys/retired/builder-z.json")).unwrap();
        let token = std::fs::read(team.join("tokens/builder-z")).unwrap();
        std::fs::remove_file(team.join("tokens/builder-z")).unwrap();
        let tokens = std::fs::read_to_string(team.join("tokens.b3")).unwrap();
        let kept: String = tokens.lines().filter(|l| !l.starts_with("builder-z ")).map(|l| format!("{l}\n")).collect();
        std::fs::write(team.join("tokens.b3"), kept).unwrap();
        // The old token, offered again: the running server still signs, which is what the re-run must end.
        std::fs::write(team.join("tokens/builder-z"), &token).unwrap();
        let before = lines("signed.jsonl").len();
        ok(sh("scripts/bk", &["claim", "before the restart"], Some("builder-z")), "builder-z before the restart");
        assert_eq!(lines("signed.jsonl").len(), before + 1, "{scripts}");
        std::fs::remove_file(team.join("tokens/builder-z")).unwrap();
        let pid = std::fs::read_to_string(team.join("server.pid")).unwrap();
        let out = sh("scripts/team-server", &["retire", "builder-z"], None);
        assert!(String::from_utf8_lossy(&out.stdout).contains("restarting the server"), "{scripts}: {out:?}");
        failed(out, "retire after a stop before the restart", "already retired");
        assert_ne!(std::fs::read_to_string(team.join("server.pid")).unwrap(), pid, "{scripts}: not restarted");
        std::fs::write(team.join("tokens/builder-z"), &token).unwrap();
        assert!(!sh("scripts/bk", &["claim", "after the restart"], Some("builder-z")).status.success(), "{scripts}");
        assert_eq!(lines("signed.jsonl").len(), before + 1, "{scripts}: signed after the restart");
        std::fs::remove_file(team.join("tokens/builder-z")).unwrap();
        ok(sh("scripts/bk", &["claim", "alice after builder-z"], Some("alice")), "alice after builder-z");
        ok(sh("scripts/team-server", &["key", "builder-w"], None), "key builder-w");

        // A restart under a binary that can't serve the held keys refuses before stopping the server, which
        // goes on serving (finding d8703c51297c).
        failed(with_old(&["token", "alice"]), "token with an old binary and keys held", "left as it is");
        ok(sh("scripts/bk", &["claim", "the server is still up"], Some("alice")), "alice after the refusal");

        // With keys held, an older binary isn't started with flags it rejects: start says why.
        ok(sh("scripts/team-server", &["stop"], None), "stop");
        failed(with_old(&["start"]), "start with an old binary", "upgrade the pinned binary");

        // A refused retire changes nothing: a differing copy in keys/retired/ is found before the key is
        // listed, and a link in place of either key file is refused before it's compared or removed.
        let snapshot = || {
            let read = |f: &str| std::fs::read(team.join(f)).ok();
            [read("keys/retired.jsonl"), read("keys/builder-w.json"), read("tokens/builder-w"), read("tokens.b3")]
        };
        let before = snapshot();
        std::fs::write(team.join("keys/retired/builder-w.json"), "{}\n").unwrap();
        failed(sh("scripts/team-server", &["retire", "builder-w"], None), "retire with a differing copy", "differs");
        assert_eq!(snapshot(), before, "{scripts}: a refused retire changed the team dir");
        std::fs::remove_file(team.join("keys/retired/builder-w.json")).unwrap();
        std::os::unix::fs::symlink(team.join("keys/builder-w.json"), team.join("keys/retired/builder-w.json")).unwrap();
        failed(sh("scripts/team-server", &["retire", "builder-w"], None), "retire with a linked copy", "symbolic link");
        assert_eq!(snapshot(), before, "{scripts}: a refused retire changed the team dir");
        std::fs::remove_file(team.join("keys/retired/builder-w.json")).unwrap();
        std::fs::rename(team.join("keys/builder-w.json"), team.join("builder-w.json")).unwrap();
        std::os::unix::fs::symlink(team.join("builder-w.json"), team.join("keys/builder-w.json")).unwrap();
        failed(sh("scripts/team-server", &["retire", "builder-w"], None), "retire with a linked key", "symbolic link");
        assert_eq!(snapshot(), before, "{scripts}: a refused retire changed the team dir");
        assert!(team.join("builder-w.json").exists() && !team.join("keys/retired/builder-w.json").exists());
    }
}
