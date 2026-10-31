//! The team scripts in `scripts/` (and their copies in the agent-team-kickoff skill), run
//! against a throwaway Ground in a throwaway git repository.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use manyroot::Body;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const SCRIPTS: [&str; 2] = ["scripts", ".claude/skills/agent-team-kickoff/templates"];
// The tests split per copy (the _in_scripts and _in_the_kickoff_templates pairs) name each copy by
// index: a third copy needs a third of each.
const _: () = assert!(SCRIPTS.len() == 2);

fn command(dir: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env("BRACKEN_BIN", dir.join("../bracken"))
        .env("REAL_BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        // verify-seq's free-space floor would make these tests depend on the machine's disk.
        .env("VERIFY_SEQ_MIN_GB", "0")
        .env_remove("VERIFY_SEQ_WARN_GB");
    command
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    command(dir, program, args).output().unwrap()
}

/// Writes an executable through a child `sh`, so this test process never holds it open for writing. A
/// parallel test's fork would inherit such a descriptor until its exec, and running the file meanwhile
/// fails with ETXTBSY, "Text file busy" (task aa2ac61f8dd4).
fn write_executable(path: &Path, contents: &str) {
    let mut sh = Command::new("sh")
        .args(["-c", "cat >\"$0\"", path.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut sh.stdin.take().unwrap(), contents.as_bytes()).unwrap();
    assert!(sh.wait().unwrap().success(), "couldn't write {path:?}");
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

/// Copies a file with `cp`, for the same reason as [`write_executable`]: `std::fs::copy` would hold the
/// copy open for writing in this process.
fn copy_executable(from: &Path, to: &Path) {
    assert!(Command::new("cp").arg(from).arg(to).status().unwrap().success(), "cp {from:?} {to:?}");
}

fn bk(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, "scripts/bk", args);
    assert!(out.status.success(), "bk {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

/// A git repository holding `scripts/bk`, the given `bk-evidence` and the `code-scope` beside it, with a
/// Ground and identity `val`.
///
/// `scripts/bk` runs bracken through a pipe that fails when its reader stops early, as an
/// older bracken or a `printf | grep -q` did: the scripts must not depend on how the writer
/// takes a closed pipe.
fn workspace(bk_evidence: &Path) -> (tempfile::TempDir, PathBuf, String) {
    let scope = bk_evidence.with_file_name("code-scope");
    let (tmp, dir) =
        repo(&[("scripts/bk-evidence", bk_evidence), ("scripts/code-scope", &scope)], ".bracken/\n", |_| {});
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    bk(&dir, &["init", "--name", "scripts"]);
    bk(&dir, &["id", "new", "val"]);
    (tmp, dir, sha)
}

/// A git repository on main holding `scripts/bk`, the given files, and whatever `setup` adds, all committed.
fn repo(files: &[(&str, &Path)], gitignore: &str, setup: impl FnOnce(&Path)) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    write_executable(
        &tmp.path().join("bracken"),
        "#!/usr/bin/env bash\nset -o pipefail\n\"$REAL_BRACKEN\" \"$@\" | cat\n",
    );
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    copy_executable(&repo_root().join("scripts/bk"), &dir.join("scripts/bk"));
    for (to, from) in files {
        copy_executable(from, &dir.join(to));
    }
    std::fs::write(dir.join(".gitignore"), gitignore).unwrap();
    setup(&dir);
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@example.invalid"],
        &["add", "-A"],
        &["commit", "-qm", "scripts"],
    ] {
        assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    }
    (tmp, dir)
}

/// --any-head must refuse a pr claim even when `log --tag pr --json` is over the 64 KiB pipe
/// buffer: a search that stops at the first match must not turn the writer's SIGPIPE into a pass.
#[test]
fn bk_evidence_any_head_refuses_a_pr_claim_in_a_large_pr_log() {
    for scripts in SCRIPTS {
        let script = format!("{scripts}/bk-evidence");
        let (_tmp, dir, sha) = workspace(&repo_root().join(&script));
        let padding = "x".repeat(4000);
        let mut prs = Vec::new();
        for n in 1..=20 {
            prs.push(bk(
                &dir,
                &["claim", &format!("PR #{n} branch-{n} @ {sha}: {padding}"), "--tag", "pr", "--as", "val"],
            ));
        }
        let log = bk(&dir, &["log", "--tag", "pr", "--json"]);
        assert!(log.len() > 64 * 1024, "the pr log is only {} bytes", log.len());

        // The first and last pr claims, wherever they fall in the log, are refused.
        for pr in [&prs[0], &prs[19]] {
            let out =
                run(&dir, "scripts/bk-evidence", &[&pr[..12], "--as", "val", "--any-head", "--", "/usr/bin/true"]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{script} accepted pr claim {pr}: {stderr}");
            assert!(stderr.contains("is tagged pr"), "{script}: {stderr}");
        }
        assert_eq!(bk(&dir, &["log", "--kind", "evidence", "--json"]), "[]", "{script} recorded evidence");

        // A claim that isn't a pr claim still takes --any-head.
        let note = bk(&dir, &["claim", "a note", "--tag", "note", "--as", "val"]);
        let out = run(&dir, "scripts/bk-evidence", &[&note[..12], "--as", "val", "--any-head", "--", "/usr/bin/true"]);
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        // It names the evidence by its id alone, out of bracken's labelled line (task d4e958fd6719).
        let stderr = String::from_utf8_lossy(&out.stderr);
        let evidence = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        let id = serde_json::from_str::<serde_json::Value>(&evidence).unwrap()[0]["id"].as_str().unwrap().to_owned();
        assert!(stderr.contains(&format!("bk-evidence: recorded pass {id} (exit 0)")), "{script}: {stderr}");
    }
}

/// ground-checkpoint must refuse when what got staged isn't exactly the new Ground files, even when the
/// unexpected lines are over the 64 KiB pipe buffer: a search that stops at the first match must not turn
/// the writer's SIGPIPE into a pass. A `git` shim stages more along with the Ground's new files: new files
/// outside .bracken/, changes to tracked ones, or the removal of committed Ground files, whose paths are
/// allowed and are caught only because they aren't additions.
#[test]
fn ground_checkpoint_refuses_more_than_a_pipe_of_unexpected_staged_lines() {
    let real_git = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    let names: Vec<String> = (0..2000).map(|i| format!("extra/{i:04}-{}", "y".repeat(150))).collect();
    for scripts in SCRIPTS {
        for (case, also) in [
            ("new files", "add extra"),
            ("changed tracked files", "add extra"),
            ("unstaged Ground files", "rm -r --cached .bracken/objects"),
        ] {
            let tracked = case == "changed tracked files";
            let checkpoint = repo_root().join(scripts).join("ground-checkpoint");
            let (tmp, dir) = repo(&[("scripts/ground-checkpoint", &checkpoint)], ".bracken/identities/\n", |dir| {
                std::fs::create_dir_all(dir.join("extra")).unwrap();
                for name in names.iter().filter(|_| tracked) {
                    std::fs::write(dir.join(name), "old\n").unwrap();
                }
            });
            let case = format!("{scripts}, {case}");
            let checkpoint = |shim: bool| {
                let mut command = command(&dir, "scripts/ground-checkpoint", &[]);
                if shim {
                    let path = std::env::var("PATH").unwrap();
                    command.env("PATH", format!("{}:{path}", tmp.path().join("shim").display()));
                }
                command.env("REAL_GIT", &real_git).env("STAGE_ALSO", also).output().unwrap()
            };
            let head = || String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap();

            bk(&dir, &["init", "--name", "checkpoint"]);
            bk(&dir, &["id", "new", "val"]);
            bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
            if also.starts_with("rm") {
                // Enough committed objects that their removal is over 64 KiB of lines; written in
                // process, as a thousand bracken runs would be slow.
                let mut store = manyroot::Store::open(&dir.join(".bracken")).unwrap();
                let val = store.identity("val").unwrap();
                for i in 0..1000 {
                    store.contribute(&val, Body::Claim { statement: format!("filler {i}"), tags: vec![] }).unwrap();
                }
            }
            let out = checkpoint(false);
            assert!(out.status.success(), "{case}: {}", String::from_utf8_lossy(&out.stderr));

            bk(&dir, &["claim", "second", "--tag", "note", "--as", "val"]);
            for name in &names {
                std::fs::write(dir.join(name), "new\n").unwrap();
            }
            std::fs::create_dir_all(tmp.path().join("shim")).unwrap();
            write_executable(
                &tmp.path().join("shim/git"),
                "#!/usr/bin/env bash\n\"$REAL_GIT\" \"$@\" || exit\n\
                 [ \"$*\" != \"add --pathspec-from-file=-\" ] || \"$REAL_GIT\" $STAGE_ALSO\n",
            );
            let also: Vec<&str> = also.split(' ').collect();
            let dry_run = [&also[..1], &["--dry-run"], &also[1..]].concat();
            let staged = run(&dir, "git", &dry_run).stdout;
            assert!(staged.len() > 64 * 1024, "{case}: only {} bytes of extra staging", staged.len());
            let before = head();
            let out = checkpoint(true);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{case}: checkpoint committed {}", head());
            assert!(
                stderr.contains("staging didn't match what was checked"),
                "{case}: {}",
                &stderr[..500.min(stderr.len())]
            );
            assert_eq!(head(), before, "{case}: a commit was made");
            assert!(run(&dir, "git", &["diff", "--cached", "--quiet"]).status.success(), "{case}: left staged");
        }
    }
}

/// merge must not hand the forge token to its children when the caller exports a variable named `token`:
/// an assignment keeps an inherited export. The fake bracken reports every exported variable holding the
/// (dummy) token, by name, on every call after merge's first (`heads`, its pull check), which comes
/// before the token is set.
#[test]
fn merge_keeps_the_token_out_of_the_environment_when_the_caller_exports_token() {
    for scripts in SCRIPTS {
        let merge = repo_root().join(scripts).join("merge");
        let (tmp, dir) = repo(&[("scripts/merge", &merge)], ".bracken/\n", |_| {});
        for args in [
            &["config", "remote.origin.url", "https://forge.example/o/r.git"][..],
            &[
                "config",
                "credential.https://forge.example.helper",
                "!f() { echo username=t; echo password=DUMMY-TOKEN-0123; }; f",
            ],
        ] {
            assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
        }
        let probe = tmp.path().join("probe.log");
        write_executable(
            &tmp.path().join("bracken"),
            "#!/usr/bin/env bash\n[ \"$1\" != heads ] || exit 0\necho ran >>\"$PROBE_LOG\"\n\
             for v in $(compgen -e); do [[ ${!v} != *DUMMY-TOKEN* ]] || echo \"exported: $v\"; done >>\"$PROBE_LOG\"\n\
             exit 1\n",
        );
        // merge only checks that curl and jq exist before the token; nothing reaches them here.
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        for tool in ["curl", "jq"] {
            write_executable(&tmp.path().join("stubs").join(tool), "#!/bin/sh\nexit 1\n");
        }
        let path = std::env::var("PATH").unwrap();
        let out = command(
            &dir,
            "scripts/merge",
            &["1", "0123456789ab", "--validator", "v", "--min-passes", "1", "--dry-run"],
        )
        .env("PATH", format!("{}:{path}", tmp.path().join("stubs").display()))
        .env("PROBE_LOG", &probe)
        .env("token", "unrelated")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("DUMMY-TOKEN") && !String::from_utf8_lossy(&out.stdout).contains("DUMMY-TOKEN"));
        let log = std::fs::read_to_string(&probe).unwrap_or_else(|_| panic!("{scripts}: bracken never ran: {stderr}"));
        assert_eq!(log, "ran\n", "{scripts}: the token reached bracken's environment");
        let said = "team server: none set up (no .bracken-team); reading the local replica\n";
        assert!(String::from_utf8_lossy(&out.stdout).starts_with(said), "{scripts}: {:?}", out.stdout);
    }
}

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

/// Runs `scripts/merge --dry-run` for PR #1 against a local forge: a bare repository standing in for
/// origin (through `insteadOf`) and a `curl` stub that answers with the PR. main holds a committed Ground;
/// `on_branch` changes the PR's head and `on_main` what main gains after the branch point, each committed
/// (`git add -A`, so a caller forces anything ignored in itself). The pr claim is by `builder` and passed
/// by `val`, so the gate accepts it.
fn merge_dry_run(scripts: &str, on_branch: impl FnOnce(&Path), on_main: impl FnOnce(&Path)) -> Output {
    merge_dry_run_on(scripts, |_| {}, on_branch, on_main, None)
}

/// Evidence on the pr claim as bk-evidence --on records it: `<command> @ <head> on <main sha>`.
#[derive(Clone, Copy)]
struct Tested<'a> {
    who: &'a str,
    /// The command in the method: `scripts/test-table` unless a case says otherwise.
    command: &'a str,
    /// main's rev at the end, or a 40-hex sha taken as it is.
    rev: &'a str,
    /// The PR's head if None.
    head: Option<&'a str>,
    verdict: &'a str,
    /// Retracted by `who` afterwards.
    retracted: bool,
    /// After the main sha in the method: ' tree <key> docs-only', say.
    suffix: &'a str,
}

fn tested<'a>(who: &'a str, rev: &'a str) -> Tested<'a> {
    Tested { who, rev, command: "scripts/test-table", head: None, verdict: "pass", retracted: false, suffix: "" }
}

/// The same, with `at_base` committed in the branch point, and `tested` recorded on the claim.
fn merge_dry_run_on(
    scripts: &str,
    at_base: impl FnOnce(&Path),
    on_branch: impl FnOnce(&Path),
    on_main: impl FnOnce(&Path),
    tested: Option<Tested>,
) -> Output {
    let (_tmp, _dir, mut command) = merge_dry_run_command(scripts, at_base, on_branch, on_main, tested);
    command.output().unwrap()
}

/// The repository and the `scripts/merge --dry-run` command for [`merge_dry_run_on`], not yet run.
fn merge_dry_run_command(
    scripts: &str,
    at_base: impl FnOnce(&Path),
    on_branch: impl FnOnce(&Path),
    on_main: impl FnOnce(&Path),
    tested: Option<Tested>,
) -> (tempfile::TempDir, PathBuf, Command) {
    let file = |name: &str| repo_root().join(scripts).join(name);
    let (merge, guards, scope) = (file("merge"), file("ground-guards"), file("code-scope"));
    let files = [("scripts/merge", &merge), ("scripts/ground-guards", &guards), ("scripts/code-scope", &scope)];
    let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
    let (tmp, dir) = repo(&files, ".bracken/identities/\n", |_| {});
    let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    let commit = |message: &str| {
        git(&["add", "-A"]);
        git(&["commit", "-q", "--allow-empty", "-m", message]);
    };
    bk(&dir, &["init", "--name", "merge"]);
    bk(&dir, &["id", "new", "builder"]);
    bk(&dir, &["id", "new", "val"]);
    bk(&dir, &["id", "new", "manager"]);
    bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
    at_base(&dir);
    commit("ground");
    git(&["switch", "-qc", "pr"]);
    on_branch(&dir);
    commit("the change");
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    git(&["switch", "-q", "main"]);
    on_main(&dir);
    commit("main moves on");

    let forge = tmp.path().join("forge.git");
    git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
    git(&["push", "-q", forge.to_str().unwrap(), "main", "pr:refs/pull/1/head"]);
    git(&["config", "remote.origin.url", "https://forge.example/o/r.git"]);
    git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
    git(&["config", &format!("url.{}.insteadOf", forge.display()), "https://forge.example/o/r.git"]);
    git(&["config", "credential.https://forge.example.helper", "!f() { echo username=t; echo password=DUMMY; }; f"]);
    let claim = bk(&dir, &["claim", &format!("PR #1 pr @ {sha}: a change"), "--tag", "pr", "--as", "builder"]);
    bk(&dir, &["evidence", &claim, "pass", "--method", "checked", "--as", "val"]);
    if let Some(t) = tested {
        let on = if t.rev.len() == 40 {
            t.rev.to_owned()
        } else {
            String::from_utf8(run(&dir, "git", &["rev-parse", t.rev]).stdout).unwrap().trim().to_owned()
        };
        let method = format!("{} @ {} on {on}{}", t.command, t.head.unwrap_or(&sha), t.suffix);
        let id = bk(&dir, &["evidence", &claim, t.verdict, "--method", &method, "--as", t.who]);
        if t.retracted {
            bk(&dir, &["retract", &id, "--as", t.who]);
        }
    }

    std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
    let pr =
        format!(r#"{{"head":{{"sha":"{sha}"}},"state":"open","merged":false,"base":{{"ref":"main"}},"title":"t"}}"#);
    write_executable(&tmp.path().join("stubs/curl"), &format!("#!/bin/sh\necho '{pr}'\n"));
    let path = std::env::var("PATH").unwrap();
    let mut merge =
        command(&dir, "scripts/merge", &["1", &claim[..12], "--validator", "val", "--min-passes", "1", "--dry-run"]);
    merge.env("PATH", format!("{}:{path}", tmp.path().join("stubs").display())).env("GIT_CONFIG_NOSYSTEM", "1");
    (tmp, dir, merge)
}

/// The line `merge` printed for check `name`: `ok` or `FAIL`, then the check's name and detail.
fn check_line(out: &Output, name: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find(|l| l.split_whitespace().nth(1) == Some(name));
    line.unwrap_or_else(|| panic!("no {name} check in: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr)))
        .to_owned()
}

/// merge runs scripts/ground-guards on the PR's head: a clean head passes, and a head that commits a key
/// under .bracken/identities/ or changes a committed Ground object is refused, saying which guard.
#[test]
fn merge_refuses_a_head_that_fails_the_ground_guards() {
    for scripts in SCRIPTS {
        let clean = merge_dry_run(scripts, |dir| std::fs::write(dir.join("code.txt"), "a change\n").unwrap(), |_| {});
        let stderr = String::from_utf8_lossy(&clean.stderr);
        assert!(clean.status.success(), "{scripts}: {}{stderr}", String::from_utf8_lossy(&clean.stdout));
        assert!(check_line(&clean, "guards").starts_with("ok "), "{scripts}: {}", check_line(&clean, "guards"));

        let key = merge_dry_run(
            scripts,
            |dir| {
                std::fs::write(dir.join(".bracken/identities/leak.json"), "{\"secret_key\": \"00\"}\n").unwrap();
                assert!(run(dir, "git", &["add", "-f", ".bracken/identities/leak.json"]).status.success());
            },
            |_| {},
        );
        let line = check_line(&key, "guards");
        assert_eq!(key.status.code(), Some(1), "{scripts}: merged a head with a key");
        assert!(line.starts_with("FAIL") && line.contains("only Ground files under .bracken"), "{scripts}: {line}");

        let object = merge_dry_run(
            scripts,
            |dir| {
                let objects = dir.join(".bracken/objects");
                let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
                let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
                let mut bytes = std::fs::read(&file).unwrap();
                bytes.push(b'x');
                std::fs::write(&file, bytes).unwrap();
            },
            |_| {},
        );
        let line = check_line(&object, "guards");
        assert_eq!(object.status.code(), Some(1), "{scripts}: merged a head that edits the Ground");
        assert!(line.starts_with("FAIL") && line.contains("Ground is append-only"), "{scripts}: {line}");

        // Guard 3 passes and says so before guard 4 fails: merge names guard 4.
        let vision = merge_dry_run(
            scripts,
            |dir| {
                std::fs::create_dir_all(dir.join("docs")).unwrap();
                std::fs::write(dir.join("docs/vision.md"), "a vision with no artifact\n").unwrap();
            },
            |_| {},
        );
        let line = check_line(&vision, "guards");
        assert_eq!(vision.status.code(), Some(1), "{scripts}: merged a vision document the Ground doesn't hold");
        assert!(line.starts_with("FAIL") && line.contains("vision document"), "{scripts}: {line}");
    }
}

/// main may be ahead of the verified head in .md files under .claude/skills/ that the branch doesn't
/// touch, as in docs/: they can't change what builds or runs. Not in a templates/ directory (those are the
/// scripts and files a kickoff copies into a repo), not in other files, and not in a file the branch changed.
#[test]
fn merge_lets_main_be_ahead_only_in_skill_docs_the_branch_leaves_alone() {
    let write = |path: &'static str, text: &'static str| {
        move |dir: &Path| {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    };
    let code = write("code.txt", "a change\n");
    for scripts in SCRIPTS {
        for (main_file, branch_also, ok) in [
            (".claude/skills/kick/process.md", None, true),
            (".claude/skills/kick/deep/notes.md", None, true),
            ("docs/guide.md", None, true),
            (".claude/skills/kick/templates/AGENTS.md", None, false),
            (".claude/skills/kick/templates/merge", None, false),
            (".claude/skills/templates/x.md", None, false),
            (".claude/skills/kick/run.sh", None, false),
            (".claude/skills/kick/process.md", Some(".claude/skills/kick/process.md"), false),
        ] {
            let out = merge_dry_run(
                scripts,
                |dir| {
                    code(dir);
                    if let Some(f) = branch_also {
                        write(f, "the branch's version\n")(dir);
                    }
                },
                |dir| {
                    let file = dir.join(main_file);
                    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                    std::fs::write(file, "main's version\n").unwrap();
                },
            );
            let line = check_line(&out, "main");
            let case = format!("{scripts}: main ahead in {main_file}, branch also {branch_also:?}");
            assert_eq!(line.starts_with("ok "), ok, "{case}: {line}");
            assert_eq!(out.status.success(), ok, "{case}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

/// Stages `contents` at `path` with `mode`, as git's index takes it, without the working tree: for a
/// path in a case a case-insensitive filesystem would fold, or a symlink.
fn stage(dir: &Path, mode: &str, path: &str, contents: &str) {
    let blob = dir.join("../blob");
    std::fs::write(&blob, contents).unwrap();
    let oid = run(dir, "git", &["hash-object", "-w", blob.to_str().unwrap()]).stdout;
    let entry = format!("{mode},{},{path}", String::from_utf8(oid).unwrap().trim());
    let out = run(dir, "git", &["update-index", "--add", "--cacheinfo", &entry]);
    assert!(out.status.success(), "staging {path}: {}", String::from_utf8_lossy(&out.stderr));
}

/// The object path main's .gitattributes export-ignores, for a head to hide a file at.
const HIDDEN: &str = ".bracken/objects/ab/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// Runs `scripts/ground-guards <args>` from `cwd` inside the repository, in the C locale. main holds a
/// committed Ground with docs/vision.md as its artifact, and a .gitattributes that keeps the Ground's
/// files from conversion and export-ignores `HIDDEN`. Branch `pr` commits what `on_branch` stages.
fn ground_guards(scripts: &str, cwd: &str, args: [&str; 2], on_branch: impl FnOnce(&Path)) -> Output {
    let guards = repo_root().join(scripts).join("ground-guards");
    let (_tmp, dir) = repo(&[("scripts/ground-guards", &guards)], ".bracken/identities/\n", |dir| {
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/vision.md"), "the vision\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), format!(".bracken/** -text\n{HIDDEN} export-ignore\n")).unwrap();
    });
    let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    bk(&dir, &["init", "--name", "guards"]);
    bk(&dir, &["id", "new", "val"]);
    bk(&dir, &["artifact", "docs/vision.md", "--as", "val"]);
    git(&["config", "core.ignorecase", "false"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "ground"]);
    git(&["switch", "-qc", "pr"]);
    on_branch(&dir);
    git(&["commit", "-q", "--allow-empty", "-m", "the change"]);
    command(&dir, dir.join("scripts/ground-guards").to_str().unwrap(), &args)
        .current_dir(dir.join(cwd))
        .env("LC_ALL", "C")
        .env_remove("LANG")
        .output()
        .unwrap()
}

/// What a case stages on the branch.
type Stager<'a> = Box<dyn FnOnce(&Path) + 'a>;

/// ground-guards refuses each way a head can slip a key or a broken Ground past it, saying which guard: a
/// file hidden from the copy it opens by export-ignore, attributes that would convert the Ground on
/// checkout, a path with a non-ASCII byte (a Kelvin sign folds to k), a malformed object, a symlink or an
/// executable, a .bracken/ path in another case, an agent entry holding a key, a vision document the Ground
/// doesn't match, a head that is its own base, and an edited object when it's run from a subdirectory.
#[test]
fn ground_guards_refuse_each_way_a_head_can_hide_or_break_the_ground() {
    let object = |name: char| format!(".bracken/objects/ab/{}", name.to_string().repeat(62));
    let key = format!("{{\"secret_key\": \"{}\"}}\n", "0".repeat(64));
    for scripts in SCRIPTS {
        let cases: Vec<(&str, &str, [&str; 2], Stager, &str)> = vec![
            ("clean", ".", ["main", "pr"], Box::new(|dir| stage(dir, "100644", "code.txt", "a change\n")), ""),
            ("export-ignore", ".", ["main", "pr"], Box::new(|dir| stage(dir, "100644", HIDDEN, &key)), "Ground opens"),
            (
                "a nested .gitattributes",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", "crates/manyroot/tests/fixtures/.gitattributes", "* -text\n")),
                "",
            ),
            (
                "attributes converting the Ground",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    stage(dir, "100644", ".gitattributes", ".bracken/** -text\n.bracken/objects/** text eol=crlf\n")
                }),
                ".gitattributes changed",
            ),
            (
                "a Kelvin sign for k",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    stage(dir, "100644", &format!(".brac\u{212A}en/objects/ee/{}", "d".repeat(62)), "garbage\n")
                }),
                "a non-ASCII byte in the path",
            ),
            (
                "malformed object",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", &object('d'), "not a contribution\n")),
                "Ground opens: the committed Ground doesn't open",
            ),
            (
                "symlink",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "120000", &object('e'), "/etc/passwd")),
                "mode 120000",
            ),
            (
                "executable",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100755", &object('f'), "#!/bin/sh\n")),
                "mode 100755",
            ),
            (
                "other case",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", ".BRACKEN/identities/k.json", &key)),
                ".BRACKEN/identities/k.json (not a file under .bracken/",
            ),
            (
                "keyed agent entry",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", ".bracken/agents/x.json", &key)),
                "only Ground files under .bracken",
            ),
            (
                "vision mismatch",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", "docs/vision.md", "another vision\n")),
                "vision document: docs/vision.md differs",
            ),
            (
                "a Weave record",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    weave_artifact(dir, "Application/VND.Bracken.Weave.ground+cbor");
                    assert!(run(dir, "git", &["add", "-A"]).status.success());
                }),
                "no Weave records",
            ),
            (
                "head is the base",
                ".",
                ["pr", "pr"],
                Box::new(|dir| stage(dir, "100644", "code.txt", "a change\n")),
                "nothing to compare it with; can't check",
            ),
            (
                "from a subdirectory",
                "docs",
                ["main", "pr"],
                Box::new(|dir| {
                    let objects = dir.join(".bracken/objects");
                    let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
                    let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
                    let mut bytes = std::fs::read(&file).unwrap();
                    bytes.push(b'x');
                    std::fs::write(&file, bytes).unwrap();
                    assert!(run(dir, "git", &["add", "-A"]).status.success());
                }),
                "Ground is append-only",
            ),
        ];
        for (case, cwd, args, on_branch, expected) in cases {
            let out = ground_guards(scripts, cwd, args, on_branch);
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let case = format!("{scripts}, {case}");
            if expected.is_empty() {
                assert!(out.status.success(), "{case}: {stdout}{stderr}");
                assert!(stdout.contains("all guards pass"), "{case}: {stdout}");
                continue;
            }
            assert_eq!(out.status.code(), Some(1), "{case} passed: {stdout}{stderr}");
            let last = stderr.lines().rfind(|l| l.starts_with("ground-guards:")).unwrap_or_default();
            let failure = &stderr[stderr.find(last).unwrap_or_default()..];
            assert!(failure.contains(expected), "{case}: expected {expected:?} in: {stderr}");
        }
    }
}

/// ground-checkpoint runs ground-guards on what origin's main gained before anything is pulled: it names a
/// clean commit as checked, and stops on one that edits a committed Ground object. Offline, it still commits,
/// and says to rerun it before pushing. A root .gitattributes change lands only with --attributes-checked.
#[test]
fn ground_checkpoint_runs_the_ground_guards_on_what_it_would_pull() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (checkpoint, guards) = (file("ground-checkpoint"), file("ground-guards"));
        let files = [("scripts/ground-checkpoint", &checkpoint), ("scripts/ground-guards", &guards)];
        let (tmp, dir) = repo(&files.map(|(to, from)| (to, from.as_path())), ".bracken/identities/\n", |_| {});
        let git = |args: &[&str]| {
            let out = run(&dir, "git", args);
            assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        bk(&dir, &["init", "--name", "checkpoint"]);
        bk(&dir, &["id", "new", "val"]);
        bk(&dir, &["id", "new", "manager"]);
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "ground"]);
        let forge = tmp.path().join("forge.git");
        git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
        git(&["remote", "add", "origin", forge.to_str().unwrap()]);
        git(&["push", "-q", "origin", "main"]);
        // Origin's main gains a commit made elsewhere: `change` on a side branch, pushed as main.
        let push_ahead = |change: &dyn Fn(&Path)| {
            git(&["switch", "-qc", "side", "origin/main"]);
            change(&dir);
            git(&["add", "-A"]);
            git(&["commit", "-qm", "elsewhere"]);
            let sha = git(&["rev-parse", "HEAD"]);
            git(&["push", "-q", "origin", "side:main"]);
            git(&["switch", "-q", "main"]);
            git(&["branch", "-qD", "side"]);
            sha
        };
        let case = |what: &str| format!("{scripts}, {what}");

        let clean = push_ahead(&|dir| std::fs::write(dir.join("code.txt"), "a change\n").unwrap());
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}: {stdout}{}", case("clean"), String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains(&format!("git merge --no-edit {clean}")), "{}: {stdout}", case("clean"));
        git(&["merge", "-q", "--no-edit", &clean]);

        push_ahead(&|dir| {
            let objects = dir.join(".bracken/objects");
            let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
            let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
            let mut bytes = std::fs::read(&file).unwrap();
            bytes.push(b'x');
            std::fs::write(&file, bytes).unwrap();
        });
        let before = git(&["rev-parse", "HEAD"]);
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}: {stderr}", case("edited object"));
        assert!(
            stderr.contains("Ground is append-only") && stderr.contains("fails the Ground guards"),
            "{}: {stderr}",
            case("edited object")
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("git merge"), "{}", case("edited object"));
        assert_eq!(git(&["rev-parse", "HEAD"]), before, "{}: a commit was made", case("edited object"));

        // With origin out of reach, the local Ground's new files are still committed, and nothing is named to pull.
        git(&["remote", "set-url", "origin", tmp.path().join("gone.git").to_str().unwrap()]);
        bk(&dir, &["claim", "offline", "--tag", "note", "--as", "val"]);
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{}: {stdout}{stderr}", case("offline"));
        assert!(stderr.contains("checked nothing there"), "{}: {stderr}", case("offline"));
        assert!(stdout.contains("committed") && !stdout.contains("git merge"), "{}: {stdout}", case("offline"));
        assert!(
            stdout.contains("rerun ground-checkpoint") && !stdout.contains("push it"),
            "{}: {stdout}",
            case("offline")
        );
        assert_ne!(git(&["rev-parse", "HEAD"]), before, "{}: nothing was committed", case("offline"));

        // A change to the root .gitattributes stops every checkpoint until a manager lands it with
        // --attributes-checked and origin's main's full sha, which records a note.
        git(&["remote", "set-url", "origin", forge.to_str().unwrap()]);
        git(&["push", "-qf", "origin", &format!("{clean}:main")]);
        let attributes = push_ahead(&|dir| std::fs::write(dir.join(".gitattributes"), "*.png binary\n").unwrap());
        let checkpoint = |args: &[&str]| {
            let out = run(&dir, "scripts/ground-checkpoint", args);
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            (out.status.code(), text)
        };
        let (code, text) = checkpoint(&[]);
        assert_eq!(code, Some(1), "{}: {text}", case("attributes"));
        assert!(text.contains("root .gitattributes changed") && text.contains("--attributes-checked"), "{text}");
        let (code, text) = checkpoint(&["--attributes-checked", &clean]);
        assert_eq!(code, Some(1), "{}: took another sha: {text}", case("attributes"));
        assert!(text.contains("isn't origin's main"), "{}: {text}", case("attributes"));
        assert!(
            !bk(&dir, &["log", "--tag", "note", "--full"]).contains("--attributes-checked"),
            "{}",
            case("attributes")
        );
        let (code, text) = checkpoint(&["--attributes-checked", &attributes]);
        assert_eq!(code, Some(0), "{}: {text}", case("attributes"));
        assert!(text.contains(&format!("git merge --no-edit {attributes}")), "{}: {text}", case("attributes"));
        let notes = bk(&dir, &["log", "--tag", "note", "--full"]);
        assert!(notes.contains("manager") && notes.contains("--attributes-checked"), "{}: {notes}", case("attributes"));
        assert!(
            git(&["status", "--porcelain", "--", ".bracken"]).is_empty(),
            "{}: the note wasn't committed",
            case("attributes")
        );
    }
}

/// Writes `text` at `path` in `dir`, making its directories.
fn put(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// Commits everything in `dir` as `message`.
fn commit_all(dir: &Path, message: &str) {
    assert!(run(dir, "git", &["add", "-A"]).status.success());
    assert!(run(dir, "git", &["commit", "-qm", message]).status.success());
}

/// main may be ahead of the head in code once the manager has tested the merge: a pass of the whole
/// table naming the head and a main sha ('scripts/test-table @ <head> on <main>', as bk-evidence --on
/// records it) that main's tip has moved past only in .bracken/ and docs. A pass of part of the table
/// (cargo fmt --check alone, or cargo test) doesn't count (task 0e67e870e14e). Evidence on an older main that main has since moved past in code is refused,
/// naming that sha, as are no such evidence, evidence by anyone but the manager, and evidence on another head.
/// One test per copy (decision 4bf44934cb67 (5)), so nextest runs the two at once.
fn merge_accepts_main_ahead_in_code_only_where_the_manager_tested_the_merge(scripts: &str) {
    let code = |dir: &Path| put(dir, "code.txt", "a change\n");
    let case = |what: &str| format!("{scripts}: {what}");

    let lib = |dir: &Path| put(dir, "lib.rs", "main's code\n");

    // Tested on main's tip.
    let out = merge_dry_run_on(scripts, |_| {}, code, lib, Some(tested("manager", "main")));
    let line = check_line(&out, "main");
    assert!(out.status.success(), "{}: {line}", case("on the tip"));
    assert!(line.starts_with("ok ") && line.contains("tested") && !line.contains("moved past"), "{line}");

    // Tested on a main that main's tip has moved past only in the Ground and docs.
    let ground_after = |dir: &Path| {
        lib(dir);
        commit_all(dir, "code on main");
        bk(dir, &["claim", "later", "--tag", "note", "--as", "val"]);
        put(dir, "docs/guide.md", "a guide\n");
    };
    let out = merge_dry_run_on(scripts, |_| {}, code, ground_after, Some(tested("manager", "main~1")));
    let line = check_line(&out, "main");
    assert!(out.status.success(), "{}: {line}", case("ground and docs after"));
    assert!(line.contains("moved past only in .bracken/ and docs"), "{line}");

    // Tested on a main that main's tip has moved past in code.
    let code_after = |dir: &Path| {
        lib(dir);
        commit_all(dir, "code on main");
        put(dir, "more.rs", "more code\n");
    };
    let out = merge_dry_run_on(scripts, |_| {}, code, code_after, Some(tested("manager", "main~1")));
    let line = check_line(&out, "main");
    assert_eq!(out.status.code(), Some(1), "{}: {line}", case("code after"));
    assert!(line.starts_with("FAIL") && line.contains("has moved in code since") && line.contains("more.rs"), "{line}");

    // Tested on a main older than the branch point (finding e82a2f2c7ff5): main added x.rs at the branch
    // point and reverted it later, so the diff from the evidence's main to the tip is empty, yet the
    // tested tree held x.rs and the merge won't.
    let out = merge_dry_run_on(
        scripts,
        |dir| put(dir, "x.rs", "used by the branch\n"),
        |dir| put(dir, "code.txt", "uses x.rs\n"),
        |dir| std::fs::remove_file(dir.join("x.rs")).unwrap(),
        Some(tested("manager", ":/^scripts")),
    );
    let line = check_line(&out, "main");
    assert_eq!(out.status.code(), Some(1), "{}: {line}", case("evidence before the branch point"));
    assert!(line.starts_with("FAIL") && line.contains("x.rs"), "{line}");

    // Tested on a commit off main that holds the branch point and differs from the tip only in docs,
    // but isn't on main: it isn't the main the merge will use.
    let side = |dir: &Path| {
        lib(dir);
        commit_all(dir, "code on main");
        assert!(run(dir, "git", &["switch", "-qc", "side"]).status.success());
        put(dir, "docs/side.md", "off main\n");
        commit_all(dir, "side");
        assert!(run(dir, "git", &["switch", "-q", "main"]).status.success());
    };
    let out = merge_dry_run_on(scripts, |_| {}, code, side, Some(tested("manager", "side")));
    let line = check_line(&out, "main");
    assert_eq!(out.status.code(), Some(1), "{}: {line}", case("evidence off main"));
    assert!(line.starts_with("FAIL"), "{line}");

    // Evidence that doesn't count (finding eed0580eba69 for the last three).
    let (other_head, unknown) = ("f".repeat(40), "e".repeat(40));
    let manager = tested("manager", "main");
    for (what, evidence) in [
        ("no evidence", None),
        ("val's evidence", Some(tested("val", "main"))),
        ("evidence on another head", Some(Tested { head: Some(other_head.as_str()), ..manager })),
        ("a manager fail", Some(Tested { verdict: "fail", ..manager })),
        ("a retracted manager pass", Some(Tested { retracted: true, ..manager })),
        ("a main sha this checkout lacks", Some(Tested { rev: unknown.as_str(), ..manager })),
        ("a pass of cargo fmt --check alone", Some(Tested { command: "cargo fmt --check", ..manager })),
        ("a pass of cargo test alone", Some(Tested { command: "cargo test --workspace", ..manager })),
        ("test-table by another path", Some(Tested { command: "./scripts/test-table", ..manager })),
    ] {
        let out = merge_dry_run_on(scripts, |_| {}, code, lib, evidence);
        let line = check_line(&out, "main");
        assert_eq!(out.status.code(), Some(1), "{}: {line}", case(what));
        assert!(
            line.starts_with("FAIL") && line.contains("bk-evidence --on origin/main -- scripts/test-table"),
            "{}: {line}",
            case(what)
        );
    }
}

#[test]
fn merge_accepts_main_ahead_in_code_only_where_the_manager_tested_the_merge_in_scripts() {
    merge_accepts_main_ahead_in_code_only_where_the_manager_tested_the_merge(SCRIPTS[0]);
}

#[test]
fn merge_accepts_main_ahead_in_code_only_where_the_manager_tested_the_merge_in_the_kickoff_templates() {
    merge_accepts_main_ahead_in_code_only_where_the_manager_tested_the_merge(SCRIPTS[1]);
}

/// bk-evidence --on tests the tree the merge will produce, in a scratch worktree it removes, and records
/// '@ <head> on <main sha>'. A head that conflicts with main is refused with nothing run or recorded.
#[test]
fn bk_evidence_on_main_tests_the_merge_and_names_both_shas() {
    for scripts in SCRIPTS {
        let script = format!("{scripts}/bk-evidence");
        let (_tmp, dir, _) = workspace(&repo_root().join(&script));
        let git = |args: &[&str]| {
            let out = run(&dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        put(&dir, "old/f.txt", "main removes this later\n");
        commit_all(&dir, "old");
        git(&["switch", "-qc", "pr"]);
        put(&dir, "a.txt", "the branch\n");
        put(&dir, "sub/c.txt", "in a subdirectory\n");
        commit_all(&dir, "branch");
        let head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        put(&dir, "b.txt", "main\n");
        commit_all(&dir, "main");
        let tip = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "pr"]);
        let claim = bk(&dir, &["claim", &format!("PR #1 pr @ {head}: a change"), "--tag", "pr", "--as", "val"]);
        let both = ["--as", "val", "--on", "main", "--", "/bin/sh", "-c", "test -f a.txt && test -f b.txt"];
        // A caller that signs commits: the scratch merge doesn't (note c83c6d8423d5).
        git(&["config", "commit.gpgsign", "true"]);
        git(&["config", "gpg.program", "/usr/bin/false"]);

        let out = run(&dir, "scripts/bk-evidence", &[&[&claim[..12]][..], &both].concat());
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        let evidence = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        assert!(evidence.contains(&format!("@ {head} on {tip} tree ")), "{script}: {evidence}");
        assert_eq!(
            git(&["worktree", "list", "--porcelain"]).matches("worktree ").count(),
            1,
            "{script}: left a worktree"
        );
        assert_eq!(git(&["rev-parse", "HEAD"]), head, "{script}: moved HEAD");
        git(&["config", "--unset", "commit.gpgsign"]);

        // From a subdirectory, the command runs in that subdirectory of the merged tree (findings
        // 405c31a313ac, 607a3268141a).
        let sub = ["--as", "val", "--on", "main", "--", "/bin/sh", "-c", "test -f c.txt && test -f ../b.txt"];
        let out = command(&dir, "../scripts/bk-evidence", &[&[&claim[..12]][..], &sub].concat())
            .current_dir(dir.join("sub"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{script} from sub/: {}", String::from_utf8_lossy(&out.stderr));

        // A command that changes the merged tree records nothing (finding eed0580eba69).
        let before = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        let writes = ["--as", "val", "--on", "main", "--", "/bin/sh", "-c", "echo more >>a.txt"];
        let out = run(&dir, "scripts/bk-evidence", &[&[&claim[..12]][..], &writes].concat());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{script}: {stderr}");
        assert!(stderr.contains("changed the merged tree"), "{script}: {stderr}");
        assert_eq!(bk(&dir, &["log", "--kind", "evidence", "--json"]), before, "{script}: recorded evidence");

        // Without --on the same command sees only the head.
        let out =
            run(&dir, "scripts/bk-evidence", &[&claim[..12], "--as", "val", "--", "/bin/sh", "-c", "test -f b.txt"]);
        assert_eq!(out.status.code(), Some(1), "{script}: b.txt isn't on the head");

        // Run from a subdirectory the merge removes: refused, naming it (8d2751ea2383).
        git(&["switch", "-q", "main"]);
        git(&["rm", "-q", "-r", "old"]);
        commit_all(&dir, "main drops old/");
        git(&["switch", "-q", "pr"]);
        let before = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        let ls = ["--as", "val", "--on", "main", "--", "/bin/ls"];
        let out = command(&dir, "../scripts/bk-evidence", &[&[&claim[..12]][..], &ls].concat())
            .current_dir(dir.join("old"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{script}: {stderr}");
        assert!(stderr.contains("has no old/, the directory this was run from"), "{script}: {stderr}");
        assert_eq!(bk(&dir, &["log", "--kind", "evidence", "--json"]), before, "{script}: recorded evidence");

        // A head that conflicts with main: nothing run, nothing recorded, no worktree left.
        git(&["switch", "-q", "main"]);
        put(&dir, "a.txt", "main's own a\n");
        commit_all(&dir, "conflict");
        git(&["switch", "-q", "pr"]);
        let before = bk(&dir, &["log", "--kind", "evidence", "--json"]);
        let out = run(&dir, "scripts/bk-evidence", &[&[&claim[..12]][..], &both].concat());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{script}: {stderr}");
        assert!(stderr.contains("doesn't merge cleanly"), "{script}: {stderr}");
        assert_eq!(bk(&dir, &["log", "--kind", "evidence", "--json"]), before, "{script}: recorded evidence");
        assert_eq!(
            git(&["worktree", "list", "--porcelain"]).matches("worktree ").count(),
            1,
            "{script}: left a worktree"
        );
    }
}

/// merge runs ground-guards with GROUND_GUARDS_ATTRIBUTES_CHECKED cleared, so a value exported in the
/// manager's shell can't let a root .gitattributes change through (note 0e50e7fc03af). A stub
/// ground-guards fails if the variable reaches it.
#[test]
fn merge_clears_the_attributes_checked_variable_for_ground_guards() {
    let stub = "#!/bin/sh\n[ -z \"$GROUND_GUARDS_ATTRIBUTES_CHECKED\" ] || \
                { echo \"ground-guards: GROUND_GUARDS_ATTRIBUTES_CHECKED reached me\" >&2; exit 1; }\n\
                echo \"ground-guards: all guards pass\"\n";
    for scripts in SCRIPTS {
        let (_tmp, dir, mut merge) =
            merge_dry_run_command(scripts, |_| {}, |dir| put(dir, "code.txt", "a change\n"), |_| {}, None);
        write_executable(&dir.join("scripts/ground-guards"), stub);
        let out = merge.env("GROUND_GUARDS_ATTRIBUTES_CHECKED", "0".repeat(40)).output().unwrap();
        let line = check_line(&out, "guards");
        assert!(line.starts_with("ok "), "{scripts}: {line}");
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

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
            "#!/bin/sh\necho \"cargo $*\" >>\"$LOG\"\ncase \"$*\" in *\"$FAIL_ON\"*) [ -n \"$FAIL_ON\" ] && exit 3 ;; esac\nexit 0\n",
        );
        let log = tmp.path().join("log");
        let table_run = |fail_on: &str| {
            let _ = std::fs::remove_file(&log);
            let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
            let out = command(&dir.join("sub"), "../scripts/test-table", &[])
                .env("PATH", path)
                .env("LOG", &log)
                .env("FAIL_ON", fail_on)
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

/// `services` (owner decision `6fed6a4c8af0`), without touching this machine's launchd: `launchctl`
/// is a stub that logs its calls and has nothing loaded, and HOME is a throwaway directory. The dry
/// run prints two valid plists that keep alive what scripts/team-server run and scripts/bk view start,
/// log under the team directory, and hold no token; it writes nothing. With --netbird the one view
/// listens on 127.0.0.1 and the NetBird address (decision 5863740ca157). uninstall removes exactly the
/// two plists, and only them.
#[test]
fn services_dry_run_holds_no_secret_and_uninstall_removes_only_its_plists() {
    for scripts in SCRIPTS {
        let copy = |name: &str| repo_root().join(scripts).join(name);
        let files = [("scripts/services", copy("services")), ("scripts/team-server", copy("team-server"))];
        let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/\n.bracken-team\n", |_| {});
        // A team directory as team-server setup leaves it, with a token whose value must never show.
        let team = tmp.path().join("team & <co>");
        std::fs::create_dir_all(team.join("tokens")).unwrap();
        std::fs::write(team.join("url"), "http://127.0.0.1:7391\n").unwrap();
        let secret = "5ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7e75ec7";
        std::fs::write(team.join("tokens/alice"), secret).unwrap();
        std::fs::write(dir.join(".bracken-team"), format!("{}\n", team.display())).unwrap();
        let home = tmp.path().join("home");
        let stubs = tmp.path().join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("launchctl"), "#!/bin/sh\necho \"$*\" >>\"$T/launchctl.log\"\nexit 1\n");
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let services = |args: &[&str]| {
            let mut c = command(&dir, "scripts/services", args);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_TOKEN", secret);
            c.output().unwrap()
        };

        let out = services(&["install", "--dry-run", "--view-port", "7399"]);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!text.contains(secret) && !text.to_lowercase().contains("token"), "{scripts}: {text}");
        let plists: Vec<&str> = text.split("\n# ").collect();
        assert_eq!(plists.len(), 2, "{scripts}: {text}");
        assert!(
            !text.contains("view-local") && !text.contains("--bind"),
            "{scripts}: without --netbird, the view on 127.0.0.1 only: {text}"
        );
        for (plist, label, args) in [
            (plists[0], "jx.rs.bracken.team-server", vec!["team-server", "run"]),
            (plists[1], "jx.rs.bracken.view", vec!["bk", "view", "--port", "7399"]),
        ] {
            let body = &plist[plist.find("<?xml").expect("a plist")..];
            assert!(body.contains(&format!("<string>{label}</string>")), "{scripts}: {body}");
            for a in args {
                assert!(body.contains(a), "{scripts}: {label} lacks {a}");
            }
            let keep_alive = "<key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>";
            for key in ["<key>RunAtLoad</key>\n  <true/>", keep_alive] {
                assert!(body.contains(key), "{scripts}: {label} lacks {key}");
            }
            assert!(
                body.contains("team &amp; &lt;co&gt;/launchd-"),
                "{scripts}: logs under the team directory, escaped"
            );
            if std::path::Path::new("/usr/bin/plutil").exists() {
                let file = tmp.path().join(format!("{label}.plist"));
                std::fs::write(&file, body).unwrap();
                let lint = Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
                assert!(lint.status.success(), "{scripts}: {}", String::from_utf8_lossy(&lint.stdout));
            }
        }
        assert!(!home.join("Library").exists(), "{scripts}: the dry run wrote something");

        // No address can be named (finding bbd29eb576fc): --netbird alone moves the view, to the address
        // `netbird status` gives, and only a connected NetBird address in 100.64.0.0/10.
        for old in [&["--bind", "192.168.1.20", "--host", "x"][..], &["--bind", "100.74.240.73"]] {
            let args: Vec<&str> = ["install", "--dry-run"].iter().chain(old).copied().collect();
            assert_eq!(services(&args).status.code(), Some(2), "{scripts}: {old:?} taken");
        }
        let viewer = tmp.path().join("bracken-with-bind");
        write_executable(
            &viewer,
            "#!/bin/sh
[ \"$1 $2\" = 'view --help' ] && echo '      --bind <IP>  Repeatable: at most one loopback and one other'\nexit 0\n",
        );
        let netbird = |management: &str, ip: &str, fqdn: &str| {
            let status =
                format!("Management: {management}\nFQDN: {fqdn}\nNetBird IP: {ip}/16\nPeers count: 3/4 Connected\n");
            std::fs::write(tmp.path().join("netbird-status"), status).unwrap();
            let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &viewer);
            c.output().unwrap()
        };
        write_executable(&stubs.join("netbird"), "#!/bin/sh\n[ \"$1\" = status ] && cat \"$T/netbird-status\"\n");
        let out = netbird("Connected", "100.74.240.73", "warren-mbp.jx.rs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        // One view, one process, on both: 127.0.0.1 and the NetBird IP, with the name for the NetBird one.
        let plists: Vec<&str> = text.split("\n# ").collect();
        assert_eq!(plists.len(), 2, "{scripts}: one view agent: {text}");
        assert!(!text.contains("view-local"), "{scripts}: {text}");
        let both = "<string>view</string>\n    <string>--port</string>\n    <string>7392</string>\n    <string>--bind</string>\n    <string>127.0.0.1</string>\n    <string>--bind</string>\n    <string>100.74.240.73</string>\n    <string>--host</string>\n    <string>warren-mbp.jx.rs</string>\n  </array>";
        assert!(
            plists[1].contains("<string>jx.rs.bracken.view</string>") && plists[1].contains(both),
            "{scripts}: {text}"
        );
        if std::path::Path::new("/usr/bin/plutil").exists() {
            let file = tmp.path().join("view-netbird.plist");
            std::fs::write(&file, &plists[1][plists[1].find("<?xml").expect("a plist")..]).unwrap();
            let lint = Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
            assert!(lint.status.success(), "{scripts}: {}", String::from_utf8_lossy(&lint.stdout));
        }
        assert!(!home.join("Library").exists(), "{scripts}: the dry run wrote something");
        for (management, ip, fqdn, why) in [
            ("Disconnected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected", "192.168.1.20", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.128.0.1", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.63.255.1", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.74.240.300", "warren-mbp.jx.rs", "isn't a NetBird address"),
            // Leading zeros, which an inet_aton-style parser reads as octal (finding 81e8ef2f49f6).
            ("Connected", "100.064.000.001", "warren-mbp.jx.rs", "isn't a NetBird address"),
            ("Connected", "100.74.240.073", "warren-mbp.jx.rs", "isn't a NetBird address"),
            // Exactly one Management line, and it says Connected.
            ("Disconnected\nManagement: Connected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected\nManagement: Disconnected", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected (cached)", "100.74.240.73", "warren-mbp.jx.rs", "isn't connected"),
            ("Connected", "100.74.240.73", "evil host; rm", "isn't a host name"),
        ] {
            let out = netbird(management, ip, fqdn);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success() && stderr.contains(why), "{scripts}: {ip} {fqdn}: {stderr}");
            assert!(out.stdout.is_empty(), "{scripts}: printed a plist for {ip}");
        }
        // The real view (through the test's wrapper) can listen on two addresses, so --netbird goes ahead.
        let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
        c.env("HOME", &home).env("PATH", &path).env("T", tmp.path());
        netbird("Connected", "100.74.240.73", "warren-mbp.jx.rs");
        let out = c.output().unwrap();
        let bindless =
            std::process::Command::new(env!("CARGO_BIN_EXE_bracken")).args(["view", "--help"]).output().unwrap();
        // services tells a view that can listen on two addresses by this phrase in its help.
        assert!(
            String::from_utf8_lossy(&bindless.stdout).contains("at most one loopback"),
            "{scripts}: bracken view's --bind help lost the phrase services checks for"
        );
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        // A view from before decision 5863740ca157, whose --bind doesn't repeat: --netbird refuses.
        let single = tmp.path().join("bracken-single-bind");
        write_executable(&single, "#!/bin/sh\n[ \"$1 $2\" = 'view --help' ] && echo '      --bind <IP>'\nexit 0\n");
        let mut c = command(&dir, "scripts/services", &["install", "--dry-run", "--netbird"]);
        c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &single);
        let out = c.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && stderr.contains("two addresses"), "{scripts}: {stderr}");

        // A foreign server on the view's port at a wildcard address (finding a67053e1e3d2): lsof lists it
        // for the port, but not for 127.0.0.1:P or the NetBird IP, as the real lsof does. Install refuses,
        // naming it, and changes nothing, with --netbird or without.
        write_executable(
            &stubs.join("lsof"),
            "#!/bin/sh\ncase \" $* \" in *' -iTCP:7399 '*) echo 4242 ;; esac\nexit 0\n",
        );
        for netbird_flag in [&[][..], &["--netbird"]] {
            let args: Vec<&str> = ["install", "--view-port", "7399"].iter().chain(netbird_flag).copied().collect();
            let mut c = command(&dir, "scripts/services", &args);
            c.env("HOME", &home).env("PATH", &path).env("T", tmp.path()).env("BRACKEN_BIN", &viewer);
            let out = c.output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{scripts} {netbird_flag:?}: a wildcard holder wasn't refused: {stderr}");
            assert!(stderr.contains("port 7399 is taken (pid 4242)"), "{scripts} {netbird_flag:?}: {stderr}");
            assert!(!home.join("Library").exists(), "{scripts}: refused, but wrote something");
            let calls = std::fs::read_to_string(tmp.path().join("launchctl.log")).unwrap_or_default();
            assert!(!calls.contains("bootstrap") && !calls.contains("bootout"), "{scripts}: {calls}");
        }
        // status names a wildcard holder blocking the view's address, as real lsof's -F output gives it.
        write_executable(
            &stubs.join("lsof"),
            "#!/bin/sh\ncase \" $* \" in *' -iTCP:7392 '*-Fpn*) printf 'p4242\\nf3\\nn*:7392\\n' ;; esac\nexit 0\n",
        );
        let out = services(&["status"]);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(
            text.contains("jx.rs.bracken.view: not loaded, nothing on 127.0.0.1:7392, held by pid 4242 on *:7392"),
            "{scripts}: {text}"
        );
        std::fs::remove_file(stubs.join("lsof")).unwrap();

        let agents = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        for name in ["jx.rs.bracken.team-server.plist", "jx.rs.bracken.view.plist", "someone.else.plist"] {
            std::fs::write(agents.join(name), "x").unwrap();
        }
        let out = services(&["uninstall"]);
        assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
        let left: Vec<_> = std::fs::read_dir(&agents).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["someone.else.plist"], "{scripts}");
        assert!(team.join("tokens/alice").exists() && team.join("url").exists(), "{scripts}: uninstall kept the rest");
        let calls = std::fs::read_to_string(tmp.path().join("launchctl.log")).unwrap_or_default();
        assert!(!calls.contains("bootout"), "{scripts}: nothing was loaded, so nothing booted out: {calls}");
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
            ("git", "echo git >>\"$T/ran\""),
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

/// bk-evidence --on records the merged tree's key, and with --reuse a repeated tree runs only the Ground
/// guards (owner decision 642a47c04a10): after main moves only in docs and the Ground, the earlier pass of
/// the whole table on the same PR is cited, not rerun. After main moves in code it runs again. Neither
/// another PR's pass nor a docs-only one is reused, and a docs-only run is labelled in the method.
#[test]
fn bk_evidence_reuses_a_whole_table_pass_of_the_same_merged_tree_on_the_same_pr() {
    for scripts in SCRIPTS {
        let script = format!("{scripts}/bk-evidence");
        let (tmp, dir, _) = workspace(&repo_root().join(&script));
        let git = |args: &[&str]| {
            let out = run(&dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        write_executable(
            &dir.join("scripts/test-table"),
            "#!/bin/sh\necho table >>\"$LOG\"\n\
             [ -z \"$DOCS\" ] || echo 'test-table: docs-only: skipped clippy and cargo test'\n",
        );
        write_executable(&dir.join("scripts/ground-guards"), "#!/bin/sh\necho \"guards $*\" >>\"$LOG\"\n");
        put(&dir, "lib.txt", "code\n");
        commit_all(&dir, "stubs");
        git(&["switch", "-qc", "pr"]);
        put(&dir, "a.txt", "the branch\n");
        commit_all(&dir, "branch");
        let head = git(&["rev-parse", "HEAD"]);
        let claim = bk(&dir, &["claim", &format!("PR #1 pr @ {head}: a change"), "--tag", "pr", "--as", "val"]);
        let other = bk(&dir, &["claim", &format!("PR #2 pr @ {head}: another"), "--tag", "pr", "--as", "val"]);
        let log = tmp.path().join("log");
        let record = |claim: &str, docs: bool| {
            let _ = std::fs::remove_file(&log);
            let args = [&claim[..12], "--as", "val", "--on", "main", "--reuse", "--", "scripts/test-table"];
            let mut c = command(&dir, "scripts/bk-evidence", &args);
            c.env("LOG", &log);
            if docs {
                c.env("DOCS", "1");
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
            let ran = std::fs::read_to_string(&log).unwrap();
            let evidence: serde_json::Value =
                serde_json::from_str(&bk(&dir, &["log", "--kind", "evidence", "--json"])).unwrap();
            let last = evidence.as_array().unwrap().last().unwrap().clone();
            (ran, last["id"].as_str().unwrap().to_owned(), last["body"]["method"].as_str().unwrap().to_owned())
        };
        let switch_main = |change: &dyn Fn()| {
            git(&["switch", "-q", "main"]);
            change();
            git(&["add", "-A"]);
            git(&["commit", "-qm", "main moves"]);
            let tip = git(&["rev-parse", "HEAD"]);
            git(&["switch", "-q", "pr"]);
            tip
        };

        // The first run: the whole table, with the key in the method.
        let tip = git(&["rev-parse", "main"]);
        let (ran, first, method) = record(&claim, false);
        assert_eq!(ran, "table\n", "{script}");
        let key = method.strip_prefix(&format!("scripts/test-table @ {head} on {tip} tree ")).unwrap_or_else(|| {
            panic!("{script}: {method}");
        });
        assert!(key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()), "{script}: {method}");

        // main moves in docs and the Ground only (a file under .bracken/, as a checkpoint commits): the same
        // key, so only the guards run, citing the first pass.
        let tip = switch_main(&|| {
            put(&dir, "docs/guide.md", "a guide\n");
            put(&dir, ".bracken/checkpoint.txt", "a checkpoint\n");
            git(&["add", "-f", ".bracken/checkpoint.txt"]);
        });
        let (ran, _, method) = record(&claim, false);
        assert_eq!(ran, format!("guards {tip} HEAD\n"), "{script}");
        assert_eq!(method, format!("scripts/test-table @ {head} on {tip} tree {key} reuses {first}"), "{script}");

        // main moves in code: a new key, so the whole table runs.
        let tip = switch_main(&|| put(&dir, "lib.txt", "new code\n"));
        let (ran, _, method) = record(&claim, false);
        assert_eq!(ran, "table\n", "{script}");
        assert!(method.starts_with(&format!("scripts/test-table @ {head} on {tip} tree ")), "{script}: {method}");
        assert!(!method.contains(key) && !method.contains("reuses"), "{script}: {method}");

        // PR #1's pass isn't PR #2's; a docs-only run is labelled, and isn't reused either.
        let (ran, _, method) = record(&other, true);
        assert_eq!(ran, "table\n", "{script}: reused another PR's pass");
        assert!(method.ends_with(" docs-only"), "{script}: {method}");
        let (ran, _, method) = record(&other, false);
        assert_eq!(ran, "table\n", "{script}: reused a docs-only pass");
        assert!(!method.contains("docs-only") && !method.contains("reuses"), "{script}: {method}");

        // The split counted run is the whole table too: its pass is recorded under its own command, and a
        // later run of either form on the same tree reuses it (owner decision 01a2d668b67e).
        let tip = switch_main(&|| put(&dir, "lib.txt", "split code\n"));
        let _ = std::fs::remove_file(&log);
        let args = [&claim[..12], "--as", "val", "--on", "main", "--reuse", "--", "scripts/test-table", "--split"];
        let out = command(&dir, "scripts/bk-evidence", &args).env("LOG", &log).output().unwrap();
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "table\n", "{script}");
        let evidence: serde_json::Value =
            serde_json::from_str(&bk(&dir, &["log", "--kind", "evidence", "--json"])).unwrap();
        let last = evidence.as_array().unwrap().last().unwrap().clone();
        let split = last["id"].as_str().unwrap().to_owned();
        let method = last["body"]["method"].as_str().unwrap().to_owned();
        assert!(
            method.starts_with(&format!("scripts/test-table --split @ {head} on {tip} tree ")),
            "{script}: {method}"
        );
        let tip = switch_main(&|| put(&dir, "docs/split.md", "a doc\n"));
        let (ran, _, method) = record(&claim, false);
        assert_eq!(ran, format!("guards {tip} HEAD\n"), "{script}: didn't reuse the split pass");
        assert!(method.ends_with(&format!(" reuses {split}")), "{script}: {method}");

        // --reuse only with --on, and only for exactly scripts/test-table (or with --split).
        for args in [
            &[&claim[..12], "--as", "val", "--reuse", "--", "scripts/test-table"][..],
            &[&claim[..12], "--as", "val", "--on", "main", "--reuse", "--", "scripts/test-table", "--remote"],
            &[
                &claim[..12],
                "--as",
                "val",
                "--on",
                "main",
                "--reuse",
                "--",
                "scripts/test-table",
                "--remote",
                "-p",
                "bracken",
            ],
            &[&claim[..12], "--as", "val", "--on", "main", "--reuse", "--", "/usr/bin/true"],
        ] {
            let out = run(&dir, "scripts/bk-evidence", args);
            assert_eq!(out.status.code(), Some(1), "{script}: {args:?}");
            assert!(String::from_utf8_lossy(&out.stderr).contains("--reuse"), "{script}: {args:?}");
        }
    }
}

/// merge checks the two shortcuts with main's code-scope: a manager pass labelled docs-only counts only
/// on a docs-only head, and one that reuses an earlier pass only when that pass is the manager's, of the
/// whole table, on this PR, with the same key. Either fails the scope check otherwise.
/// One test per copy (decision 4bf44934cb67 (5)), so nextest runs the two at once.
fn merge_counts_a_docs_only_or_reused_pass_only_where_it_holds(scripts: &str) {
    let code = |dir: &Path| put(dir, "code.txt", "a change\n");
    let doc = |dir: &Path| put(dir, "docs/guide.md", "a guide\n");
    let lib = |dir: &Path| put(dir, "lib.rs", "main's code\n");
    fn docs_only(rev: &str) -> Tested<'_> {
        Tested { suffix: " tree 1111111111111111111111111111111111111111 docs-only", ..tested("manager", rev) }
    }
    let case = |what: &str| format!("{scripts}: {what}");

    // A docs-only pass on a docs-only head, main an ancestor and then ahead in code: both pass.
    for (what, on_main) in [("docs head", (|_| {}) as fn(&Path)), ("docs head, main ahead in code", lib)] {
        let out = merge_dry_run_on(scripts, |_| {}, doc, on_main, Some(docs_only("main")));
        assert!(out.status.success(), "{}: {}", case(what), String::from_utf8_lossy(&out.stdout));
        assert!(check_line(&out, "scope").contains("the head is docs-only"), "{}", case(what));
    }

    // A docs-only pass on a head that changes code fails, ahead or not, naming the path.
    for (what, on_main) in [("code head", (|_| {}) as fn(&Path)), ("code head, main ahead in code", lib)] {
        let out = merge_dry_run_on(scripts, |_| {}, code, on_main, Some(docs_only("main")));
        let line = check_line(&out, "scope");
        assert_eq!(out.status.code(), Some(1), "{}: {line}", case(what));
        assert!(line.starts_with("FAIL") && line.contains("code.txt"), "{}: {line}", case(what));
    }

    // A reused pass, main ahead in code: it counts when it cites the manager's whole-table pass on this
    // PR with the same key; not when the cited pass is someone else's, docs-only, another key's, or
    // on another PR.
    let key = "2".repeat(40);
    let earlier = |head: &str, main: &str, k: &str| format!("scripts/test-table @ {head} on {main} tree {k}");
    for (what, who, other_pr, cited_suffix, cited_key, ok) in [
        ("the manager's pass", "manager", false, "", key.as_str(), true),
        ("val's pass", "val", false, "", key.as_str(), false),
        ("a docs-only pass", "manager", false, " docs-only", key.as_str(), false),
        ("another key's pass", "manager", false, "", "3333333333333333333333333333333333333333", false),
        ("a pass on another PR", "manager", true, "", key.as_str(), false),
    ] {
        let (_tmp, dir, mut merge) = merge_dry_run_command(scripts, |_| {}, code, lib, None);
        let rev = |r: &str| String::from_utf8(run(&dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let (head, tip) = (rev("pr"), rev("main"));
        let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
        let claim = prs[0]["id"].as_str().unwrap().to_owned();
        let target = if other_pr {
            bk(&dir, &["claim", &format!("PR #2 other @ {head}: another"), "--tag", "pr", "--as", "builder"])
        } else {
            claim.clone()
        };
        let cited_method = format!("{}{cited_suffix}", earlier(&"a".repeat(40), &"b".repeat(40), cited_key));
        let cited = bk(&dir, &["evidence", &target, "pass", "--method", &cited_method, "--as", who]);
        let method = format!("{} reuses {cited}", earlier(&head, &tip, &key));
        bk(&dir, &["evidence", &claim, "pass", "--method", &method, "--as", "manager"]);
        let out = merge.output().unwrap();
        let line = check_line(&out, "scope");
        assert_eq!(out.status.success(), ok, "{}: {line}", case(what));
        assert_eq!(line.starts_with("ok "), ok, "{}: {line}", case(what));
        if ok {
            assert!(check_line(&out, "main").starts_with("ok "), "{}", case(what));
        } else {
            assert!(line.contains(&format!("reusing {cited}")), "{}: {line}", case(what));
        }
    }
}

#[test]
fn merge_counts_a_docs_only_or_reused_pass_only_where_it_holds_in_scripts() {
    merge_counts_a_docs_only_or_reused_pass_only_where_it_holds(SCRIPTS[0]);
}

#[test]
fn merge_counts_a_docs_only_or_reused_pass_only_where_it_holds_in_the_kickoff_templates() {
    merge_counts_a_docs_only_or_reused_pass_only_where_it_holds(SCRIPTS[1]);
}

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
                "#!/bin/sh\necho \"$PWD|$*\" >>\"$LOG\"\necho bk-evidence: recorded pass aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa '(exit 0)' >&2\n",
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
            c.env("LOG", &log).env("TMPDIR", &scratch).env_remove("ON");
            if let Some(on) = on {
                c.env("ON", on);
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "{scripts}: {}", String::from_utf8_lossy(&out.stderr));
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

/// `test-table --split` (owner decision `01a2d668b67e`), with a stub `boat` and `cargo`: the whole table in
/// one box, as --remote runs it, and here only the macos_only tests, under nextest; it passes only if both
/// do, and ends with both parts' last lines. Which tests need macOS comes from the code: a test that runs
/// here but is missing or ignored on Linux, by the two sides' lists (nextest's, and cargo test --doc's for
/// doctests), must be in a macos_only module. The box's lists come from the real test-table --lists, with a
/// cargo that writes its own lines on standard error in the middle of its list's lines, as cargo nextest
/// list's build output can land (task 06d7b4ef2c75): they arrive whole, and a list damaged on the way
/// fails as damaged.
/// A failure here stops the box and deletes it; a docs-only head uses no box.
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
        write_executable(
            &stubs.join("boat"),
            "#!/bin/bash\n\
             case \"$1\" in\n\
             run) printf '%s\\n' \"$@\" >\"$T/args\"\n\
               echo 'bx_stub01 ready in 1.0s'; touch \"$T/ready\"; echo 'test-table: all passed at x, in a box'\n\
               [ ! -e \"$T/linux\" ] || cat \"$T/linux\"\n\
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
            for name in ["HANG", "HANG_LIST", "SURVIVE", "IGNORE_TERM", "NO_NEXTEST"] {
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

/// The split counted run counts as the whole table: merge takes a manager pass of
/// `scripts/test-table --split` on the merge, main ahead in code, as it takes `scripts/test-table`'s;
/// a pass of `--remote` alone doesn't count, nor one of `--remote -p`, a builder's scoped tests in a box
/// (finding 4a25ddba5d49).
#[test]
fn merge_counts_a_split_run_as_the_whole_table_and_remote_alone_not() {
    let code = |dir: &Path| put(dir, "code.txt", "a change\n");
    let lib = |dir: &Path| put(dir, "lib.rs", "main's code\n");
    for scripts in SCRIPTS {
        let tree = " tree 4444444444444444444444444444444444444444";
        for (command, suffix, ok) in [
            ("scripts/test-table --split", tree, true),
            ("scripts/test-table --split", "", true),
            ("scripts/test-table --remote", tree, false),
            ("scripts/test-table --remote -p bracken", tree, false),
            ("scripts/test-table --remote -p bracken", "", false),
        ] {
            let pass = Tested { command, suffix, ..tested("manager", "main") };
            let out = merge_dry_run_on(scripts, |_| {}, code, lib, Some(pass));
            let line = check_line(&out, "main");
            assert_eq!(out.status.success(), ok, "{scripts}: {command}{suffix}: {line}");
        }
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

/// Sets the modification time of `path` and everything under it, not following symlinks.
fn set_mtimes(path: &Path, when: std::time::SystemTime) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            set_mtimes(&entry.unwrap().path(), when);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(when).unwrap();
}

/// cleanup (owner process 11f1ac552f26) on stubbed worktrees, with HOME and TMPDIR throwaway directories
/// and a stub cargo that logs any call. Of this repo's worktrees, it removes only the clean, unused ones
/// idle 2 hours under T3's root (t3-<id>) or TMPDIR, and removes target/ only in a builder's worktree
/// whose branch is squash-merged into origin/main; it clears only idle scratch entries. A dirty, in-use,
/// young, unmerged, key-holding or outside worktree stays, each with its reason. The dry run changes
/// nothing, and both runs end with a summary and exit 0.
#[test]
fn cleanup_removes_only_clean_unused_idle_checkouts_in_its_places_and_the_dry_run_changes_nothing() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);

        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let t3 = home.join(".t3/worktrees/repo");
        let scratch = tmpdir.join("bracken-scratch");
        std::fs::create_dir_all(home.join("bracken-runs/run1")).unwrap();
        let add = |path: &Path, branch: Option<&str>| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let path = path.to_str().unwrap();
            match branch {
                Some(b) => git(&dir, &["worktree", "add", "-q", "-b", b, path]),
                None => git(&dir, &["worktree", "add", "-q", "--detach", path]),
            }
        };
        let commit = |path: &Path, file: &str| {
            std::fs::write(path.join(file), file).unwrap();
            git(path, &["add", file]);
            git(path, &["commit", "-qm", file]);
        };
        for name in ["t3-clean", "t3-dirty", "t3-busy", "t3-key"] {
            add(&t3.join(name), None);
        }
        std::fs::write(t3.join("t3-dirty/wip"), "work").unwrap();
        std::fs::create_dir_all(t3.join("t3-key/.bracken/identities")).unwrap();
        std::fs::write(t3.join("t3-key/.bracken/identities/k"), "key").unwrap();
        for branch in ["merged", "unmerged", "linked"] {
            add(&t3.join(branch), Some(branch));
            commit(&t3.join(branch), branch);
        }
        for branch in ["merged", "unmerged"] {
            std::fs::create_dir_all(t3.join(branch).join("target/debug")).unwrap();
            std::fs::write(t3.join(branch).join("target/debug/big"), "build").unwrap();
        }
        // A shared build dir behind a symlinked target/: never followed (finding a3cb4a1ac00d).
        let shared = t.join("shared-target");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("other-build"), "build").unwrap();
        std::os::unix::fs::symlink(&shared, t3.join("linked/target")).unwrap();
        // The forge squash-merges "merged" and "linked": main gets their changes, not their commits.
        for branch in ["merged", "linked"] {
            git(&dir, &["merge", "-q", "--squash", branch]);
            git(&dir, &["commit", "-qm", &format!("{branch} (#1)")]);
        }
        git(&dir, &["push", "-q", "origin", "main"]);
        add(&scratch.join("task1/tree"), None);
        add(&t.join("elsewhere/tree"), None);
        // In use, with a backslash in its name, which awk -v would have unescaped (finding 786ebd4a2970).
        add(&tmpdir.join("b\\s"), None);
        // On origin only: the dry run mustn't fetch it (finding 41df1ea1240c).
        git(&origin, &["branch", "extra", "main"]);
        std::fs::create_dir_all(scratch.join("old/deep")).unwrap();
        std::fs::write(scratch.join("old/deep/file"), "x").unwrap();

        // Everything so far was last used 3 hours ago; what follows is new.
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600));
        add(&t3.join("t3-young"), None);
        std::fs::create_dir_all(scratch.join("new")).unwrap();
        std::fs::write(scratch.join("new/file"), "x").unwrap();

        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("cargo"), "#!/bin/sh\necho \"$PWD|$*\" >>\"$LOG\"\n");
        let log = t.join("cargo.log");
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let busy = t3.join("t3-busy");
        let mut sleeper = Command::new("sleep").arg("120").current_dir(&busy).spawn().unwrap();
        let mut slashed = Command::new("sleep").arg("120").current_dir(tmpdir.join("b\\s")).spawn().unwrap();
        let refs = || String::from_utf8(run(&dir, "git", &["for-each-ref"]).stdout).unwrap();
        let refs_before = refs();
        let cleanup = |args: &[&str]| {
            let out = command(&dir, "scripts/cleanup", args)
                .env("HOME", &home)
                .env("TMPDIR", &tmpdir)
                .env("PATH", &path)
                .env("LOG", &log)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            assert!(out.status.success(), "{scripts}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
            stdout
        };
        let line = |out: &str, path: &str| {
            out.lines()
                .find(|l| l.contains(&format!("{path}:")))
                .unwrap_or_else(|| panic!("{scripts}: no line for {path}: {out}"))
                .to_owned()
        };
        let worktrees = || String::from_utf8(run(&dir, "git", &["worktree", "list"]).stdout).unwrap();
        let before = worktrees();

        let dry = cleanup(&[]);
        let expect = [
            ("/t3-clean", "remove"),
            ("/task1/tree", "remove"),
            ("/merged", "clean"),
            ("/linked", "won't touch"),
            ("bracken-scratch/old", "clear"),
            ("/t3-dirty", "won't touch"),
            ("/t3-key", "won't touch"),
            ("elsewhere/tree", "won't touch"),
            ("/t3-busy", "keep"),
            ("b\\s", "keep"),
            ("/t3-young", "keep"),
            ("/unmerged", "keep"),
            ("bracken-scratch/new", "keep"),
        ];
        for (path, verb) in expect {
            assert!(line(&dry, path).starts_with(verb), "{scripts}: {path} should be {verb}: {dry}");
        }
        assert!(line(&dry, "/t3-dirty").contains("dirty"), "{scripts}: {dry}");
        assert!(line(&dry, "/t3-busy").contains("in use by pid"), "{scripts}: {dry}");
        assert!(line(&dry, "/unmerged").contains("isn't merged"), "{scripts}: {dry}");
        assert!(line(&dry, "elsewhere/tree").contains("outside"), "{scripts}: {dry}");
        assert!(dry.lines().last().unwrap().contains("4 to do with --apply"), "{scripts}: {dry}");
        assert_eq!(worktrees(), before, "{scripts}: the dry run changed the worktrees");
        assert!(!log.exists(), "{scripts}: the dry run ran cargo");
        assert_eq!(refs(), refs_before, "{scripts}: the dry run changed the refs");
        assert!(line(&dry, "b\\s").contains("in use by pid"), "{scripts}: {dry}");
        assert!(scratch.join("old/deep/file").exists() && t3.join("t3-clean").exists(), "{scripts}");

        let applied = cleanup(&["--apply"]);
        for s in [&mut sleeper, &mut slashed] {
            s.kill().unwrap();
            s.wait().unwrap();
        }
        assert!(refs().contains("refs/remotes/origin/extra"), "{scripts}: --apply didn't fetch");
        assert!(applied.lines().last().unwrap().contains("4 done"), "{scripts}: {applied}");
        let after = worktrees();
        for gone in ["t3-clean", "task1/tree"] {
            assert!(!after.contains(gone), "{scripts}: {gone} is still a worktree: {after}");
        }
        assert!(!t3.join("t3-clean").exists() && !scratch.join("old").exists(), "{scripts}: {applied}");
        for stays in ["t3-dirty", "t3-busy", "t3-key", "t3-young", "/merged", "unmerged", "/linked", "elsewhere/tree"] {
            assert!(after.contains(stays), "{scripts}: {stays} was removed: {applied}");
        }
        // git worktree list quotes a backslash, so look for the directory itself.
        assert!(line(&applied, "b\\s").starts_with("keep") && tmpdir.join("b\\s/Cargo.toml").exists(), "{scripts}");
        assert!(t3.join("t3-dirty/wip").exists() && t3.join("t3-key/.bracken/identities/k").exists());
        assert!(scratch.join("new/file").exists() && home.join("bracken-runs/run1").exists(), "{scripts}");
        // target/ is removed directly, never through cargo, and a symlinked one isn't followed.
        assert!(!log.exists(), "{scripts}: cargo ran: {applied}");
        assert!(!t3.join("merged/target").exists() && t3.join("unmerged/target/debug/big").exists(), "{scripts}");
        assert!(line(&applied, "/linked").contains("symlink") && shared.join("other-build").exists(), "{scripts}");
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

/// Finding 740976a60693: a copy of cleanup outside any git checkout, run from inside another repo, refuses
/// at once instead of taking the caller's directory for its main checkout.
#[test]
fn cleanup_outside_a_checkout_refuses_rather_than_cleaning_the_callers_repo() {
    for scripts in SCRIPTS {
        let (tmp, dir) = repo(&[], ".bracken/\n", |_| {});
        let loose = tmp.path().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        copy_executable(&repo_root().join(scripts).join("cleanup"), &loose.join("cleanup"));
        let out = command(&dir, loose.join("cleanup").to_str().unwrap(), &[])
            .env("GIT_CEILING_DIRECTORIES", tmp.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(out.stdout.is_empty() && stderr.contains("isn't in a git checkout"), "{scripts}: {stderr}");
    }
}

/// Runs a command on a pseudo-terminal of its own and types `answer` when it asks, or (`answer` None) in a
/// new session with no terminal at all. Prints what the terminal showed and exits with the command's code.
const ON_A_TERMINAL: &str = r#"import os, pty, signal, sys
answer, argv = sys.argv[1], sys.argv[2:]
signal.alarm(120)
if answer == "-":
    os.setsid()
    os.execvp(argv[0], argv)
pid, fd = pty.fork()
if pid == 0:
    os.execvp(argv[0], argv)
out, sent = b"", False
while True:
    try:
        data = os.read(fd, 1024)
    except OSError:
        break
    if not data:
        break
    out += data
    if not sent and b"Type approve" in out:
        os.write(fd, answer.encode() + b"\n")
        sent = True
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
sys.exit(os.waitstatus_to_exitcode(status))
"#;

/// Task ec880b9c2c86: owner-approve signs, as owner and only on a terminal where the owner types
/// `approve`, an approval claim naming the full id, related supports it, and prints its id and no key.
/// As anyone else, without a terminal, for an unknown id, or without `approve`, it writes nothing. With
/// --delegated it asks nothing and the approval says it was signed on the owner's delegation, quoting
/// the yes; the same yes can't sign a second approval.
#[test]
fn owner_approve_signs_as_owner_only_on_a_terminal_and_never_prints_a_key() {
    for scripts in SCRIPTS {
        let approve = repo_root().join(scripts).join("owner-approve");
        let (tmp, dir) = repo(&[("scripts/owner-approve", &approve)], ".bracken/\n", |_| {});
        bk(&dir, &["init", "--name", "owner-approve"]);
        bk(&dir, &["id", "new", "owner"]);
        bk(&dir, &["id", "new", "manager"]);
        let task = bk(&dir, &["claim", "Task: a model run", "--tag", "task", "--as", "manager"]);
        let helper = tmp.path().join("on-a-terminal.py");
        std::fs::write(&helper, ON_A_TERMINAL).unwrap();
        let on_a_terminal = |answer: &str, args: &[&str]| {
            let mut all = vec!["-I", helper.to_str().unwrap(), answer, "scripts/owner-approve"];
            all.extend(args);
            run(&dir, "python3", &all)
        };
        let approvals = || bk(&dir, &["log", "--tag", "approval", "--json"]);
        let refused = |out: &Output, says: &str| {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.status.code(), Some(1), "{scripts}: {text}");
            assert!(text.contains(says), "{scripts}: wanted {says:?}: {text}");
            assert_eq!(approvals(), "[]", "{scripts}: wrote an approval: {text}");
        };
        let words = "run replicate 2 on the pilot settings";

        let mut as_agent = command(&dir, "scripts/owner-approve", &[&task[..12], words]);
        refused(&as_agent.env("BRACKEN_AGENT", "manager").output().unwrap(), "only the owner approves");
        refused(&run(&dir, "scripts/owner-approve", &[&task[..12], words, "--as", "val"]), "only the owner approves");
        refused(&on_a_terminal("-", &[&task[..12], words]), "needs a terminal");
        refused(&on_a_terminal("approve", &["0123456789ab", words]), "no contribution");
        refused(&on_a_terminal("yes", &[&task[..12], words]), "not approved");

        let out = on_a_terminal("approve", &[&task[..12], words]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{scripts}: {text}{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains(&task) && text.contains("Task: a model run"), "{scripts}: didn't show it: {text}");
        let id = text.lines().map(str::trim).rfind(|l| l.len() == 64).unwrap_or_else(|| panic!("{scripts}: {text}"));
        let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", id, "--json"])).unwrap();
        assert_eq!(shown["author_name"], "owner", "{scripts}");
        assert_eq!(shown["body"]["tags"], serde_json::json!(["approval"]), "{scripts}");
        let statement = shown["body"]["statement"].as_str().unwrap();
        assert!(statement.starts_with("Approval (owner, ") && statement.ends_with(&format!(" of {task}: {words}")));
        assert!(statement.contains(", run by the owner) of "), "{scripts}: {statement}");
        let supports = shown["referrers"].as_array().unwrap().iter().any(|r| {
            r["author_name"] == "owner" && r["body"]["kind"] == "supports" && r["body"]["to"] == task.as_str()
        });
        assert!(supports, "{scripts}: no supports relation from owner: {shown}");
        // No hex run from the owner's identity file (its keys) reached the terminal.
        let key = std::fs::read_to_string(dir.join(".bracken/identities/owner.json")).unwrap();
        for run in key.split(|c: char| !c.is_ascii_hexdigit()).filter(|r| r.len() >= 32) {
            assert!(!text.contains(run), "{scripts}: printed part of the owner's identity file");
        }

        // Delegated: the manager, as owner, with no terminal; the approval quotes the owner's yes.
        let yes = "yes, run replicate 2";
        let delegated = |args: &[&str]| {
            let mut all = vec![&task[..12], words, "--delegated", yes];
            all.extend(args);
            let mut c = command(&dir, "scripts/owner-approve", &all);
            c.env("BRACKEN_AGENT", "manager").stdin(std::process::Stdio::null()).output().unwrap()
        };
        let count = || serde_json::from_str::<serde_json::Value>(&approvals()).unwrap().as_array().unwrap().len();
        let out = delegated(&[]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: signed as manager");
        assert_eq!(count(), 1, "{scripts}");
        let out = delegated(&["--as", "owner"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{scripts}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
        let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", stdout.trim(), "--json"])).unwrap();
        assert_eq!(shown["author_name"], "owner", "{scripts}");
        assert_eq!(shown["body"]["tags"], serde_json::json!(["approval", "delegated"]), "{scripts}");
        let statement = shown["body"]["statement"].as_str().unwrap();
        let says = format!(
            "signed by the manager on the owner's delegation for this approval only; the owner said: '{yes}') of {task}: {words}"
        );
        assert!(statement.contains(&says), "{scripts}: {statement}");
        let again = delegated(&["--as", "owner"]);
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert!(again.status.code() == Some(1) && stderr.contains("its own yes"), "{scripts}: {stderr}");
        assert_eq!(count(), 2, "{scripts}: the same yes signed twice");
        // Retracting the approval doesn't free its yes (finding 8d9cc19d0568).
        bk(&dir, &["retract", stdout.trim(), "--as", "owner"]);
        let again = delegated(&["--as", "owner"]);
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert!(again.status.code() == Some(1) && stderr.contains("its own yes"), "{scripts}: reused: {stderr}");
        assert_eq!(count(), 2, "{scripts}: a retracted approval's yes signed again");
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

/// Task 8b650f99aacd: scripts/bk exits 1 after a local write the team server didn't confirm. A stub bk
/// does that for one command ($UNCONFIRMED): owner-approve then says the approval (and, for relate, its
/// supports relation) is written locally but not confirmed, names it, and prints the commands to
/// finish, never that it wasn't written.
#[test]
fn owner_approve_names_a_write_the_team_server_didnt_confirm() {
    for scripts in SCRIPTS {
        let approve = repo_root().join(scripts).join("owner-approve");
        let real = repo_root().join("scripts/bk");
        let (tmp, dir) = repo(
            &[("scripts/owner-approve", &approve), ("scripts/bk.real", &real)],
            ".bracken/\n",
            |dir| {
                write_executable(
                    &dir.join("scripts/bk"),
                    "#!/usr/bin/env bash\n\"$(dirname \"$0\")/bk.real\" \"$@\" || exit\n\
                 if [ \"$1\" = \"${UNCONFIRMED:-}\" ]; then\n\
                 echo \"bk: written to the local replica, but the team server didn't confirm it (stub)\" >&2; exit 1\nfi\n",
                );
            },
        );
        let _ = &tmp;
        bk(&dir, &["init", "--name", "unconfirmed"]);
        bk(&dir, &["id", "new", "owner"]);
        bk(&dir, &["id", "new", "manager"]);
        let task = bk(&dir, &["claim", "Task: a model run", "--tag", "task", "--as", "manager"]);
        let approve = |unconfirmed: &str, yes: &str| {
            let args = [&task[..12], "run it", "--delegated", yes, "--as", "owner"];
            let mut c = command(&dir, "scripts/owner-approve", &args);
            let out = c.env("UNCONFIRMED", unconfirmed).stdin(std::process::Stdio::null()).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{scripts}: {}", String::from_utf8_lossy(&out.stdout));
            String::from_utf8_lossy(&out.stderr).into_owned()
        };
        let approvals = || -> Vec<serde_json::Value> {
            serde_json::from_str::<serde_json::Value>(&bk(&dir, &["log", "--tag", "approval", "--json"]))
                .unwrap()
                .as_array()
                .unwrap()
                .clone()
        };
        let supports = |id: &str| {
            let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", id, "--json"])).unwrap();
            shown["referrers"].as_array().unwrap().iter().filter(|r| r["body"]["kind"] == "supports").count()
        };

        let stderr = approve("claim", "yes once");
        let written = approvals();
        assert_eq!(written.len(), 1, "{scripts}: {stderr}");
        let id = written[0]["id"].as_str().unwrap();
        assert!(!stderr.contains("wasn't written"), "{scripts}: {stderr}");
        assert!(stderr.contains(&format!("approval {id} is written locally but not confirmed")), "{scripts}: {stderr}");
        assert!(stderr.contains(&format!("scripts/bk relate {id} supports {task} --as owner")), "{scripts}: {stderr}");
        assert!(stderr.contains("scripts/bk heads --as owner"), "{scripts}: {stderr}");
        assert_eq!(supports(id), 0, "{scripts}");

        let stderr = approve("relate", "yes twice");
        let written = approvals();
        assert_eq!(written.len(), 2, "{scripts}: {stderr}");
        let id = written[1]["id"].as_str().unwrap();
        assert_eq!(supports(id), 1, "{scripts}: the relation is written locally");
        assert!(
            stderr.contains(&format!("approval {id} and its supports relation ")) && stderr.contains("not confirmed"),
            "{scripts}: {stderr}"
        );
        assert!(stderr.contains("scripts/bk heads --as owner"), "{scripts}: {stderr}");
    }
}

/// Finding d494f36dee9d: merge records its "merge gate" pass before it asks the forge to merge, and a
/// live one is what tells `log --open` a PR merged. When the merge call fails, merge asks the forge
/// (findings f1fc25380904, 3623855aa0e8). Merged at the head keeps the pass. Open, at any head, retracts
/// it: at once after a 4xx, and after a 5xx or no reply only if it stays open while merge asks again, so a
/// slow merge behind a proxy's timeout keeps it. Anything else stops, naming the evidence, and keeps it.
/// When the pass is retracted, the finding against the claim stays open.
#[test]
fn a_failed_merge_call_retracts_the_gate_evidence_only_when_the_forge_shows_the_pr_unmerged() {
    // The merge call's status and curl's exit code, the forge's answers to each GET after it (the
    // last repeated), and what becomes of the pass.
    let cases = [
        ("refused, still open", "409", 22, &["open"][..], "retracted"),
        ("refused, the head moved", "409", 22, &["moved"][..], "retracted"),
        ("refused, merged at another head", "409", 22, &["merged elsewhere"][..], "unclear"),
        ("reply lost, merged", "502", 22, &["merged"][..], "stands"),
        ("slow merge behind a proxy", "504", 22, &["open", "merged"][..], "stands"),
        ("proxy timeout, never merged", "504", 22, &["open"][..], "retracted"),
        ("dropped, no answer", "000", 52, &["error"][..], "unclear"),
    ];
    for scripts in SCRIPTS {
        for (case, status, exit, answers, expect) in cases {
            let change = |dir: &Path| std::fs::write(dir.join("code.txt"), "a change\n").unwrap();
            let (tmp, dir, mut dry) = merge_dry_run_command(scripts, |_| {}, change, |_| {}, None);
            let out = dry.output().unwrap();
            assert!(out.status.success(), "{scripts}: the dry run: {}", String::from_utf8_lossy(&out.stderr));
            let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
            let claim = prs[0]["id"].as_str().unwrap().to_owned();
            let finding = bk(&dir, &["claim", "it breaks", "--tag", "finding", "--as", "val"]);
            bk(&dir, &["relate", &finding, "contradicts", &claim, "--as", "val"]);
            bk(&dir, &["evidence", &finding, "fail", "--method", "triage: not a bug", "--as", "manager"]);
            // The forge answers every GET as before until the merge call, which fails; then as the case says.
            let stub = tmp.path().join("stubs/curl");
            let get = std::fs::read_to_string(&stub).unwrap();
            let get = get.lines().nth(1).unwrap().to_owned();
            let merged = |g: &str| g.replace(r#""state":"open","merged":false"#, r#""state":"closed","merged":true"#);
            let moved = |g: &str| g.replace(r#""head":{"sha":""#, r#""head":{"sha":"0"#);
            let arms: String = answers
                .iter()
                .enumerate()
                .map(|(k, answer)| {
                    let reply = match *answer {
                        "open" => get.clone(),
                        "moved" => moved(&get),
                        "merged" => merged(&get),
                        "merged elsewhere" => merged(&moved(&get)),
                        _ => "exit 22".to_owned(),
                    };
                    assert!(*answer == "open" || reply != get, "{case}: the stub's {answer} reply didn't change");
                    let arm = if k + 1 == answers.len() { "*".to_owned() } else { (k + 1).to_string() };
                    format!("{arm}) {reply}; exit ;;\n")
                })
                .collect();
            let (posted, asked) = (tmp.path().join("posted"), tmp.path().join("asked"));
            let (posted, asked) = (posted.display(), asked.display());
            write_executable(
                &stub,
                &format!(
                    "#!/bin/sh\ncase \"$*\" in *'-X POST'*) : >'{posted}'; printf {status}; exit {exit} ;; esac\n\
                     if [ -e '{posted}' ]; then\n  n=$(($(cat '{asked}' 2>/dev/null || echo 0) + 1)); echo $n >'{asked}'\n  \
                     case $n in\n{arms}  esac\nfi\n{get}\n"
                ),
            );
            let path = format!("{}:{}", tmp.path().join("stubs").display(), std::env::var("PATH").unwrap());
            let args = ["1", &claim[..12], "--validator", "val", "--min-passes", "1", "--as", "manager"];
            let out = command(&dir, "scripts/merge", &args)
                .env("PATH", &path)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let evidence: serde_json::Value =
                serde_json::from_str(&bk(&dir, &["log", "--kind", "evidence", "--author", "manager", "--json"]))
                    .unwrap();
            let gate: Vec<&serde_json::Value> = evidence
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["body"]["method"].as_str().is_some_and(|m| m.starts_with("merge gate")))
                .collect();
            assert_eq!(gate.len(), 1, "{scripts}, {case}: {evidence}");
            let id = gate[0]["id"].as_str().unwrap();
            let retracted = gate[0]["retracted"] == true;
            let said = format!("{scripts}, {case}: {stderr}{stdout}");
            match expect {
                "retracted" => {
                    assert_eq!(out.status.code(), Some(1), "{said}");
                    assert!(stderr.contains(&format!("HTTP {status}")) && stderr.contains("is retracted"), "{said}");
                    assert!(retracted, "{said}: the pass stands");
                }
                "stands" => {
                    assert!(out.status.success(), "{said}");
                    assert!(stdout.contains(&format!("{id} stands")) && stdout.contains("merged PR #1"), "{said}");
                    assert!(!retracted, "{said}: the merged PR's pass is retracted");
                }
                _ => {
                    assert_eq!(out.status.code(), Some(1), "{said}");
                    assert!(stderr.contains("outcome is unclear") && stderr.contains(id), "{said}");
                    assert!(!retracted, "{said}: a pass retracted on an unclear outcome");
                }
            }
            // The claim counts as open exactly when the pass is retracted: then a finding the manager
            // confirms against it is listed.
            bk(&dir, &["evidence", &finding, "pass", "--method", "triage: on second thought", "--as", "manager"]);
            let open: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--open", "--json"])).unwrap();
            let listed = open.as_array().unwrap().iter().any(|f| f["id"] == finding.as_str());
            assert_eq!(listed, retracted, "{scripts}, {case}: {open}");
        }
    }
}

/// Task ec880b9c2c86: with --owner-approval, merge's owner check needs a current approval signed by owner
/// that supports the pr claim itself. None, a manager-signed quote, a retracted one, one of a task the
/// claim implements (finding 9e8e0c849724) or one borrowed from an earlier head through implements
/// (finding fb07d1777cf6) fails; without the flag there's no owner check.
#[test]
fn merge_with_owner_approval_needs_an_owner_signed_approval_of_the_pr_claim_itself() {
    for scripts in SCRIPTS {
        let change = |dir: &Path| std::fs::write(dir.join("code.txt"), "a change\n").unwrap();
        let (_tmp, dir, mut merge) = merge_dry_run_command(scripts, |_| {}, change, |_| {}, None);
        let plain = merge.output().unwrap();
        assert!(plain.status.success(), "{scripts}: {}", String::from_utf8_lossy(&plain.stderr));
        assert!(!String::from_utf8_lossy(&plain.stdout).contains(" owner "), "{scripts}: an owner check unasked");
        merge.arg("--owner-approval");
        let owner = |merge: &mut Command| {
            let out = merge.output().unwrap();
            (out.status.code(), check_line(&out, "owner"))
        };
        bk(&dir, &["id", "new", "owner"]);
        let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
        let claim = prs[0]["id"].as_str().unwrap().to_owned();
        let task = bk(&dir, &["claim", "Task: publish it", "--tag", "task", "--as", "manager"]);
        bk(&dir, &["relate", &claim, "implements", &task, "--as", "builder"]);

        let (code, line) = owner(&mut merge);
        assert!(code == Some(1) && line.starts_with("FAIL") && line.contains("owner-approve"), "{scripts}: {line}");
        let quote = bk(&dir, &["claim", "Owner, today: 'merge it'", "--tag", "approval", "--as", "manager"]);
        bk(&dir, &["relate", &quote, "supports", &claim, "--as", "manager"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(1) && line.starts_with("FAIL"), "{scripts}: a manager quote counted: {line}");

        let of_task = bk(&dir, &["claim", "Approval (owner): publish", "--tag", "approval", "--as", "owner"]);
        bk(&dir, &["relate", &of_task, "supports", &task, "--as", "owner"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(1) && line.starts_with("FAIL"), "{scripts}: a task's approval counted: {line}");

        // An approval of an earlier head, which the builder relates the current claim implements.
        let old = bk(&dir, &["claim", "PR #1 pr @ 0000: an earlier head", "--tag", "pr", "--as", "builder"]);
        let of_old = bk(&dir, &["claim", "Approval (owner): the earlier head", "--tag", "approval", "--as", "owner"]);
        bk(&dir, &["relate", &of_old, "supports", &old, "--as", "owner"]);
        bk(&dir, &["relate", &claim, "implements", &old, "--as", "builder"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(1) && line.starts_with("FAIL"), "{scripts}: an earlier head's approval counted: {line}");

        let of_pr = bk(&dir, &["claim", "Approval (owner): merge #1", "--tag", "approval", "--as", "owner"]);
        bk(&dir, &["relate", &of_pr, "supports", &claim, "--as", "owner"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(0) && line.contains("supports the pr claim"), "{scripts}: {line}");
        bk(&dir, &["retract", &of_pr, "--as", "owner"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(1) && line.starts_with("FAIL"), "{scripts}: a retracted approval counted: {line}");

        let tags = ["--tag", "approval", "--tag", "delegated", "--as", "owner"];
        let delegated = bk(&dir, &[&["claim", "Approval (owner, delegated): merge #1"][..], &tags].concat());
        bk(&dir, &["relate", &delegated, "supports", &claim, "--as", "owner"]);
        let (code, line) = owner(&mut merge);
        assert!(code == Some(0) && line.contains("on the owner's delegation"), "{scripts}: {line}");
    }
}

/// Writes an artifact with `media_type` into `dir`'s Ground with the real binary, past scripts/bk's
/// refusal, as an agent running bracken by hand could, signed by a new identity `hand`.
fn weave_artifact(dir: &Path, media_type: &str) {
    let real = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(["--dir", ".bracken"])
            .args(args)
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    std::fs::write(dir.join("../marker.cbor"), b"anything").unwrap();
    real(&["id", "new", "hand"]);
    real(&["artifact", "../marker.cbor", "--name", "weave", "--media-type", media_type, "--as", "hand"]);
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

/// The Weave tripwire in ground-checkpoint: a Weave record in the local Ground stops the commit, naming
/// it, whatever the media type's case.
#[test]
fn ground_checkpoint_refuses_a_ground_holding_a_weave_record() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (checkpoint, guards) = (file("ground-checkpoint"), file("ground-guards"));
        let files = [("scripts/ground-checkpoint", &checkpoint), ("scripts/ground-guards", &guards)];
        let (_tmp, dir) = repo(&files.map(|(to, from)| (to, from.as_path())), ".bracken/identities/\n", |_| {});
        bk(&dir, &["init", "--name", "checkpoint"]);
        bk(&dir, &["id", "new", "val"]);
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        weave_artifact(&dir, "application/VND.bracken.weave.manifest+cbor");
        let head = run(&dir, "git", &["rev-parse", "HEAD"]).stdout;
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("Weave media type") && stderr.contains("Tell the owner"), "{scripts}: {stderr}");
        assert_eq!(run(&dir, "git", &["rev-parse", "HEAD"]).stdout, head, "{scripts}: a commit was made");
        assert!(run(&dir, "git", &["diff", "--cached", "--quiet"]).status.success(), "{scripts}: left staged");
    }
}

/// A repository holding scripts/weave, with a throwaway HOME beside it, and a stub bracken that prints
/// the BRACKEN_DIR it was given and its arguments. The stub hands `show` to the real bracken, so
/// scripts/weave can still look an owner decision up in the coordination Ground through scripts/bk.
fn weave_repo() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    weave_repo_with(&[])
}

/// [`weave_repo`], with more scripts beside scripts/weave.
fn weave_repo_with(more: &[(&str, &Path)]) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let weave = repo_root().join("scripts/weave");
    let mut files: Vec<(&str, &Path)> = vec![("scripts/weave", &weave)];
    files.extend_from_slice(more);
    let (tmp, dir) = repo(&files, ".bracken/\n", |_| {});
    bk(&dir, &["init", "--name", "coordination"]);
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let stub = tmp.path().join("stub");
    write_executable(
        &stub,
        "#!/bin/sh\ncase \" $* \" in *\" show \"*) exec \"$REAL_BRACKEN\" \"$@\" ;; esac\necho \"BRACKEN_DIR=$BRACKEN_DIR $*\"\n",
    );
    (tmp, dir, home, stub)
}

/// A claim tagged decision in the coordination Ground of a [`weave_repo`], for --owner-decision.
fn owner_decision(dir: &Path) -> String {
    bk(dir, &["id", "new", "manager"]);
    bk(dir, &["claim", "Decision (owner): a weave line", "--tag", "decision", "--as", "manager"])
}

fn weave(dir: &Path, script: &str, home: &Path, bin: Option<&Path>, args: &[&str]) -> Output {
    let mut c = command(dir, script, args);
    c.env("HOME", home).env("BRACKEN_DIR", "/inherited/and/ignored");
    if let Some(bin) = bin {
        c.env("BRACKEN_BIN", bin);
    }
    c.output().unwrap()
}

/// Weave build step 4: scripts/weave creates and opens only ~/bracken-weave/manyroot, by its real path,
/// which it exports. It refuses --dir in both forms; evidence, sync and serve, found past --as;
/// --as weave-runner for anything but weave validate, and validate for a signer the binary's plan doesn't
/// list among the validators (this stub prints none); and a Ground that resolves
/// into a git checkout or has the coordination Ground's id.
#[test]
fn weave_opens_only_the_weave_ground_and_refuses_what_it_must() {
    let (tmp, dir, home, stub) = weave_repo();
    let script = "scripts/weave";
    let prog = tmp.path().join("prog");
    write_executable(&prog, "#!/bin/sh\ntouch \"$0.ran\"\n");
    let refused = |home: &Path, args: &[&str], expected: &str| {
        let out = weave(&dir, script, home, Some(&stub), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}{}", String::from_utf8_lossy(&out.stdout));
        assert!(stderr.contains(expected), "{args:?}: expected {expected:?} in {stderr}");
    };
    refused(&home, &["weave", "check"], "no Weave Ground at");
    refused(&home, &["weave", "init", "--as", "weave-integrator"], "bracken-weave doesn't exist");

    // weave init creates the Ground with the real binary; then it opens.
    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    let out = weave(&dir, script, &home, None, &["weave", "init", "--as", "weave-integrator"]);
    assert!(out.status.success(), "init: {}", String::from_utf8_lossy(&out.stderr));
    let ground = home.join("bracken-weave/manyroot");
    assert!(ground.join("ground.json").is_file());
    let out = weave(&dir, script, &home, None, &["weave", "check"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "no problems", "{}", String::from_utf8_lossy(&out.stderr));
    refused(&home, &["weave", "init", "--as", "weave-integrator"], "already exists");

    for (args, expected) in [
        (&["--dir", ".bracken", "log"][..], "--dir is refused"),
        (&["log", "--dir=.bracken"], "--dir is refused"),
        (&["evidence", "abc", "--verdict", "pass"], "evidence is refused"),
        (&["--as", "alice", "evidence", "abc"], "evidence is refused"),
        (&["--as=alice", "sync", "/tmp/x"], "sync is refused"),
        (&["serve", "--stdio"], "serve is refused"),
        (&["--as", "weave-runner", "weave", "check"], "only for weave validate"),
        (&["--as=weave-runner", "claim", "x"], "only for weave validate"),
        (&["--as", "weave-runner", "weave", "validate"], "isn't one of the line's validators"),
        // Findings 26650dedc33e and 3e1a0454a03b: -h and --help get no early exit.
        (&["sync", "--exec", prog.to_str().unwrap(), "--help"], "sync is refused"),
        (&["-h", "sync", "--exec", prog.to_str().unwrap()], "sync is refused"),
        (&["evidence", "-h"], "evidence is refused"),
        (&["--dir", ".bracken", "--help"], "--dir is refused"),
        (&["weave", "line", "create", "x", "--help"], "needs --owner-decision"),
    ] {
        refused(&home, args, expected);
    }
    assert!(!tmp.path().join("prog.ran").exists(), "--exec's program ran");
    // Help reaches bracken only for a command that isn't refused, with the Weave Ground as BRACKEN_DIR.
    let out = weave(&dir, script, &home, Some(&stub), &["weave", "check", "--help"]);
    let real_ground = ground.canonicalize().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("BRACKEN_DIR={} weave check --help", real_ground.display())
    );
    // BRACKEN_AGENT names the identity too.
    let out = command(&dir, script, &["weave", "check"])
        .env("HOME", &home)
        .env("BRACKEN_AGENT", "weave-runner")
        .env("BRACKEN_BIN", &stub)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("only for weave validate"));

    // A symlinked path is followed: it opens the real path and exports that.
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::rename(&ground, elsewhere.join("manyroot")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("manyroot"), &ground).unwrap();
    let out = weave(&dir, script, &home, Some(&stub), &["weave", "check"]);
    let real = elsewhere.join("manyroot").canonicalize().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("BRACKEN_DIR={} weave check", real.display()));

    // ... but never into a git checkout, the coordination Ground included.
    let into_repo = tmp.path().join("home-repo");
    std::fs::create_dir_all(into_repo.join("bracken-weave")).unwrap();
    std::os::unix::fs::symlink(dir.join(".bracken"), into_repo.join("bracken-weave/manyroot")).unwrap();
    refused(&into_repo, &["weave", "check"], "inside the git checkout");
    let linked_home = tmp.path().join("home-linked");
    std::fs::create_dir_all(&linked_home).unwrap();
    std::os::unix::fs::symlink(&dir, linked_home.join("bracken-weave")).unwrap();
    refused(&linked_home, &["weave", "init", "--as", "x"], "inside the git checkout");

    // ... and not a Ground with the coordination Ground's id, wherever it is.
    let copy = tmp.path().join("home-copy");
    std::fs::create_dir_all(copy.join("bracken-weave/manyroot")).unwrap();
    std::fs::copy(dir.join(".bracken/ground.json"), copy.join("bracken-weave/manyroot/ground.json")).unwrap();
    refused(&copy, &["weave", "check"], "has the coordination Ground's id");
}

/// scripts/weave runs weave validate only for one of the line's validators, as the pinned binary's own plan
/// (validate --dry-run --json) lists them, before anything runs; and never with a '--'.
#[test]
fn weave_runs_validate_only_for_a_validator() {
    let (tmp, dir, home, _) = weave_repo();
    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    let real = |args: &[&str]| weave(&dir, "scripts/weave", &home, None, args);
    let out = real(&["weave", "init", "--as", "weave-integrator"]);
    assert!(out.status.success(), "init: {}", String::from_utf8_lossy(&out.stderr));

    // A stub whose plan is $PLAN, and which otherwise says it ran.
    let stub = tmp.path().join("plan-stub");
    write_executable(
        &stub,
        "#!/bin/sh\ncase \" $* \" in *\" --dry-run \"*) printf '%s\\n' \"$PLAN\"; exit 0 ;; esac\necho \"RAN $*\"\n",
    );
    let planned = |plan: &str, args: &[&str]| {
        let mut c = command(&dir, "scripts/weave", args);
        c.env("HOME", &home).env("BRACKEN_BIN", &stub).env("PLAN", plan);
        c.output().unwrap()
    };
    let validate = ["--as", "weave-runner", "weave", "validate", "--manifest", "blake3:00", "--json"];
    for plan in [r#"{"signer": "aa", "validators": ["bb"]}"#, r#"{"signer": "aa", "validators": []}"#, "not JSON", ""] {
        let out = planned(plan, &validate);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{plan}: {stderr}");
        assert!(stderr.contains("isn't one of the line's validators"), "{plan}: {stderr}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("RAN"), "{plan}: it ran");
    }
    let out = planned(r#"{"signer": "aa", "validators": ["bb", "aa"]}"#, &validate);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // The plan was asked for without the caller's --json, and the run itself gets the arguments as given.
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "RAN --as weave-runner weave validate --manifest blake3:00 --json"
    );
    let out = planned(r#"{"signer": "aa", "validators": ["aa"]}"#, &["weave", "validate", "--", "x"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("takes no '--'"), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("RAN"));

    // With the real binary: a line whose only validator is val, and a manifest.
    let decision = owner_decision(&dir);
    for args in [
        &["id", "new", "val"][..],
        &["weave", "line", "policy", "main", "--validator", "val", "--check", "ok=true", "--owner-decision", &decision],
    ] {
        let mut args = args.to_vec();
        args.extend(["--as", "weave-integrator"]);
        let out = real(&args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let out = real(&["weave", "integrate", "--as", "weave-integrator", "--json"]);
    assert!(out.status.success(), "integrate: {}", String::from_utf8_lossy(&out.stderr));
    let integrated: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let hash = integrated["manifest"].as_str().unwrap();
    // The integrator isn't a validator: refused before anything runs.
    let out = real(&["--as", "weave-integrator", "weave", "validate", "--manifest", hash]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("isn't one of line") && stderr.contains("weave validate refused"), "{stderr}");
    // val is: its plan lists the check.
    let out = real(&["--as", "val", "weave", "validate", "--manifest", hash, "--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("would run ok attempt 1"), "{stdout}");
}

/// An approval of the Weave Ground's creation, as scripts/owner-approve writes it: a claim tagged
/// approval, signed by owner, in the coordination Ground of a [`weave_repo`], for weave-shadow --init.
fn owner_approval(dir: &Path) -> String {
    bk(dir, &["id", "new", "owner"]);
    bk(dir, &["claim", "Approval (owner): create the Weave Ground", "--tag", "approval", "--as", "owner"])
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, "git", args);
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Weave dogfood stage 1 (task 60fcf3d31dda): scripts/weave-shadow --init sets up a Weave Ground under a
/// throwaway HOME, given an owner-signed approval and nothing else, and finishes the job when run again
/// after stopping part way. Then three successive merged commits of this repository each become a decision
/// on line main, whose parent is the one before, and each verifies equal to its commit. Run again for the
/// recorded commit, it records nothing. Before writing anything it refuses a commit that isn't on
/// origin/main (even with a local branch of that name on it, finding 7315c1ccfaf4), one older than the
/// recorded commit, and an unrelated one (finding 93dea3dd7ec2). A
/// recorded decision that no longer matches git's tree (through git replace) fails. Never the real
/// ~/bracken-weave: HOME is under $TMPDIR.
#[test]
fn weave_shadow_records_each_merged_commit_and_verifies_it() {
    let shadow = repo_root().join("scripts/weave-shadow");
    let (tmp, dir, home, _) = weave_repo_with(&[("scripts/weave-shadow", &shadow)]);
    let approval = owner_approval(&dir);
    let decision = owner_decision(&dir);
    let manager_approval = bk(&dir, &["claim", "Approval: not the owner's", "--tag", "approval", "--as", "manager"]);
    let run_in = |bin: Option<&Path>, args: &[&str]| {
        let mut c = command(&dir, "scripts/weave-shadow", args);
        c.env("HOME", &home).env("BRACKEN_DIR", "/inherited/and/ignored");
        if let Some(bin) = bin {
            c.env("BRACKEN_BIN", bin);
        }
        c.output().unwrap()
    };
    let run_with = |args: &[&str]| run_in(None, args);
    let refused = |args: &[&str], expected: &str| {
        let out = run_with(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(expected), "{args:?}: expected {expected:?} in {stderr}");
    };
    refused(&["--init"], "--init needs --owner-decision");
    refused(&["--init", "--owner-decision", &approval], "bracken-weave doesn't exist");
    refused(&["HEAD"], "has no refs/remotes/origin/main");
    git_out(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    refused(&["HEAD"], "no Weave Ground at");

    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    // A wrong owner's go is refused before anything is created: only an approval signed by owner will do.
    refused(&["--init", "--owner-decision", "0123456789ab"], "has no such contribution");
    refused(&["--init", "--owner-decision", "TODO"], "12 to 64 lowercase hex");
    refused(&["--init", "--owner-decision", &decision], "isn't a claim tagged approval, signed by owner");
    refused(&["--init", "--owner-decision", &manager_approval], "isn't a claim tagged approval, signed by owner");
    let ground = home.join("bracken-weave/manyroot");
    assert!(!ground.exists(), "a refused --init left a Ground");

    // --init stopped part way (here its identity weave-runner fails once, after weave init) leaves a Ground
    // without line main's policy; run again, it finishes it; once more, it has nothing to do.
    let flaky = tmp.path().join("flaky");
    write_executable(
        &flaky,
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" id new weave-runner \"*) [ -e \"$0.failed\" ] || {{ touch \"$0.failed\"; \
             echo flaky: failed once >&2; exit 1; }} ;; esac\nexec {:?} \"$@\"\n",
            tmp.path().join("bracken")
        ),
    );
    let out = run_in(Some(&flaky), &["--init", "--owner-decision", &approval]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(ground.exists(), "the failure came after weave init");
    let out = run_in(Some(&flaky), &["--init", "--owner-decision", &approval]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "resumed --init: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("resuming --init in"), "{stdout}");
    refused(&["--init", "--owner-decision", &approval], "already set up");
    let weave = |args: &[&str]| -> serde_json::Value {
        let mut c = command(&dir, "scripts/weave", args);
        let out = c.env("HOME", &home).output().unwrap();
        assert!(out.status.success(), "weave {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let policy = weave(&[
        "weave",
        "line",
        "policy",
        "main",
        "--dry-run",
        "--owner-decision",
        &approval,
        "--as",
        "weave-integrator",
        "--json",
    ]);
    assert_eq!(policy["checks"].as_object().unwrap().len(), 2, "{policy}");
    assert_eq!(policy["checks"]["fmt"], serde_json::json!(["cargo", "fmt", "--check"]));
    assert_eq!(
        policy["checks"]["clippy"],
        serde_json::json!(["cargo", "clippy", "--all-targets", "--", "-D", "warnings"])
    );
    assert_eq!(policy["owner_decision"], approval);
    let ids = weave(&["id", "list", "--json"]);
    let runner = ids.as_array().unwrap().iter().find(|i| i["name"] == "weave-runner").expect("weave-runner");
    assert_eq!(policy["validators"], serde_json::json!([runner["agent"]]), "{policy}");
    assert_eq!(policy["integrators"].as_array().unwrap().len(), 1, "{policy}");

    // Three successive first-parent commits of this repository, oldest first, in a clone sharing its
    // objects, whose origin/main is this repository's HEAD: a branch under test isn't on the real origin/main.
    let clone = tmp.path().join("clone");
    git_out(
        tmp.path(),
        &["clone", "-q", "--shared", "--no-checkout", repo_root().to_str().unwrap(), clone.to_str().unwrap()],
    );
    let root_head = git_out(&repo_root(), &["rev-parse", "HEAD"]);
    git_out(&clone, &["update-ref", "refs/remotes/origin/main", &root_head]);
    // A box has no git identity, and commit-tree below needs one.
    git_out(&clone, &["config", "user.name", "t"]);
    git_out(&clone, &["config", "user.email", "t@example.invalid"]);
    let mut commits: Vec<String> =
        git_out(&clone, &["rev-list", "--first-parent", "-n", "3", "origin/main"]).lines().map(str::to_owned).collect();
    commits.reverse();
    assert_eq!(commits.len(), 3, "this repository has three first-parent commits");
    let in_clone = |sha: &str| run_with(&["--repo", clone.to_str().unwrap(), sha]);
    let mut decisions = Vec::new();
    for sha in &commits {
        let out = in_clone(sha);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{sha}: {}{stdout}", String::from_utf8_lossy(&out.stderr));
        let decision = stdout.lines().find_map(|l| l.strip_prefix("decision ")).expect("a decision").to_owned();
        assert!(stdout.contains(&format!("verified {decision} equal to {sha}")), "{stdout}");
        decisions.push(decision);
    }
    let log = weave(&["weave", "log", "--json"]);
    let rows = log["decisions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    // Newest first: each decision names its commit, and its parent is the one before.
    for (i, row) in rows.iter().rev().enumerate() {
        assert_eq!(row["decision"], decisions[i]);
        assert_eq!(row["source"]["git"], commits[i]);
        let parents: Vec<&str> = row["parents"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        let want: Vec<&str> = if i == 0 { vec![] } else { vec![decisions[i - 1].as_str()] };
        assert_eq!(parents, want);
    }
    let size = run(&ground, "du", &["-sk", "."]);
    let kib = String::from_utf8_lossy(&size.stdout).split_whitespace().next().unwrap_or("?").to_owned();
    eprintln!("the throwaway Weave Ground after three commits: {kib} KiB");

    // Again for the commit recorded: nothing new, and it verifies.
    let count = || weave(&["log", "--json", "--limit", "100000"])["contributions"].as_array().unwrap().len();
    let before = count();
    let out = in_clone(&commits[2]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(&format!("decision {} (already recorded for {})", decisions[2], commits[2])), "{stdout}");
    assert_eq!(count(), before);

    // Refused before anything is written: a commit not on origin/main, an older commit after a newer one,
    // and a commit unrelated to the recorded one (the test's own repository, whose origin/main is its HEAD).
    // Its tree is commits[2]'s with one file more, outside .bracken/, which Weave's recorded tree leaves
    // out: made through a scratch index, so that it differs from commits[2]'s whatever the repository's
    // last commits changed (task bb59a076515a: two of them may differ only in .bracken/).
    let extra = tmp.path().join("extra.txt");
    std::fs::write(&extra, "in no merged commit\n").unwrap();
    let blob = git_out(&clone, &["hash-object", "-w", extra.to_str().unwrap()]);
    let index = tmp.path().join("scratch-index");
    let scratch = |args: &[&str]| {
        let out = command(&clone, "git", args).env("GIT_INDEX_FILE", &index).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    scratch(&["read-tree", &commits[2]]);
    scratch(&["update-index", "--add", "--cacheinfo", &format!("100644,{blob},unmerged-only.txt")]);
    let tree = scratch(&["write-tree"]);
    let unmerged = git_out(&clone, &["commit-tree", &tree, "-p", &commits[2], "-m", "unmerged"]);
    let refused_in = |out: Output, expected: &str| {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(expected), "expected {expected:?} in {stderr}");
    };
    refused_in(in_clone(&unmerged), &format!("{unmerged} isn't on refs/remotes/origin/main"));
    // Nor does a local branch called origin/main make it merged: git would take it before the remote-tracking
    // ref (finding 7315c1ccfaf4).
    git_out(&clone, &["branch", "origin/main", &unmerged]);
    refused_in(in_clone(&unmerged), &format!("{unmerged} isn't on refs/remotes/origin/main"));
    refused_in(in_clone(&commits[0]), &format!("{} doesn't come after {}", commits[0], commits[2]));
    refused_in(run_with(&["HEAD"]), &format!("doesn't come after {}", commits[2]));
    assert_eq!(count(), before, "a refused commit wrote something");

    // A mismatch fails: a merged commit (here one whose tree is an earlier commit's), recorded, then shown by
    // git with another tree (git replace), no longer verifies.
    git_out(&clone, &["update-ref", "refs/remotes/origin/main", &unmerged]);
    let out = in_clone(&unmerged);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git_out(&clone, &["replace", &unmerged, &commits[2]]);
    let out = in_clone(&unmerged);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(&format!("doesn't verify equal to git's {unmerged}")), "{stderr}");
}

/// scripts/weave refuses a line write unless --owner-decision names a claim tagged decision, or an
/// approval signed by owner (what scripts/weave-shadow --init passes), not retracted, in the coordination
/// Ground (answer 2a620fd0b0dc, finding 3cd6cd19f9a0).
#[test]
fn weave_needs_an_owner_decision_for_line_writes() {
    let (_tmp, dir, home, stub) = weave_repo();
    std::fs::create_dir_all(home.join("bracken-weave/manyroot")).unwrap();
    let decision = owner_decision(&dir);
    let note = bk(&dir, &["claim", "a note", "--tag", "note", "--as", "manager"]);
    let retracted = bk(&dir, &["claim", "withdrawn", "--tag", "decision", "--as", "manager"]);
    bk(&dir, &["retract", &retracted, "--reason", "withdrawn", "--as", "manager"]);
    let approval = owner_approval(&dir);
    let not_owners = bk(&dir, &["claim", "Approval: not the owner's", "--tag", "approval", "--as", "manager"]);
    let run_with = |args: &[&str]| weave(&dir, "scripts/weave", &home, Some(&stub), args);
    for (args, expected) in [
        (&["weave", "line", "create", "main", "--as", "a"][..], "needs --owner-decision"),
        (&["--as", "a", "weave", "line", "policy", "x"], "needs --owner-decision"),
        (&["weave", "line", "create", "main", "--owner-decision", ""], "needs --owner-decision"),
        (&["weave", "line", "policy", "x", "--owner-decision="], "needs --owner-decision"),
        (&["weave", "line", "policy", "x", "--owner-decision", "TODO"], "12 to 64 lowercase hex"),
        (&["weave", "line", "create", "x", "--owner-decision", "0123456789ab"], "has no such contribution"),
        (&["weave", "line", "create", "x", "--owner-decision", &note], "isn't a claim tagged decision"),
        (&["weave", "line", "create", "x", "--owner-decision", &retracted], "isn't a claim tagged decision"),
        (&["weave", "line", "create", "x", "--owner-decision", &not_owners], "isn't a claim tagged decision"),
    ] {
        let out = run_with(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(expected), "{args:?}: expected {expected:?} in {stderr}");
    }
    let equals = format!("--owner-decision={}", &decision[..12]);
    for args in [
        &["weave", "line", "create", "main", "--owner-decision", &decision, "--as", "a"][..],
        &["weave", "line", "policy", "x", &equals],
        &["weave", "line", "policy", "x", "--owner-decision", &approval],
        &["weave", "check"],
    ] {
        let out = run_with(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// scripts/weave refuses what would make a line version or a supersedes relation by hand: artifact with a
/// Weave media type, in any case, and relate ... supersedes. line create and line policy still work through
/// it (answer 12000ab70ae5).
#[test]
fn weave_refuses_hand_made_weave_records_but_line_writes_work() {
    let (_tmp, dir, home, stub) = weave_repo();
    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    let out = weave(&dir, "scripts/weave", &home, None, &["weave", "init", "--as", "weave-integrator"]);
    assert!(out.status.success(), "init: {}", String::from_utf8_lossy(&out.stderr));
    for (args, expected) in [
        (
            &["artifact", "p.cbor", "--media-type", "application/vnd.bracken.weave.line+cbor", "--as", "a"][..],
            "Weave media type is refused",
        ),
        (
            &["artifact", "p.cbor", "--media-type=Application/VND.Bracken.Weave.decision+cbor"],
            "Weave media type is refused",
        ),
        (&["relate", "a", "supersedes", "b", "--as", "a"], "supersedes is refused"),
        (&["--as", "a", "relate", "--note", "n", "a", "Supersedes", "b"], "supersedes is refused"),
        // Finding 56dadb600ba2: bracken still reads the kind past '--'.
        (&["relate", "a", "--", "supersedes", "b"], "supersedes is refused"),
        (&["relate", "a", "supersedes", "--", "b"], "supersedes is refused"),
    ] {
        let out = weave(&dir, "scripts/weave", &home, Some(&stub), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(expected), "{args:?}: expected {expected:?} in {stderr}");
    }
    for args in [&["artifact", "p", "--media-type", "text/plain"][..], &["relate", "a", "supports", "b"]] {
        let out = weave(&dir, "scripts/weave", &home, Some(&stub), args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    let real = |args: &[&str]| {
        let out = weave(&dir, "scripts/weave", &home, None, args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let decision = owner_decision(&dir);
    let created = real(&["weave", "line", "create", "dev", "--owner-decision", &decision, "--as", "weave-integrator"]);
    assert!(created.starts_with("line ") && created.trim_end().ends_with(" (dev)"), "{created}");
    let changed = real(&[
        "weave",
        "line",
        "policy",
        "dev",
        "--validator",
        "weave-integrator",
        "--owner-decision",
        &decision,
        "--as",
        "weave-integrator",
    ]);
    let lines: Vec<&str> = changed.lines().collect();
    assert_eq!(lines.len(), 2, "{changed}");
    assert!(lines[0].starts_with("line ") && lines[1].starts_with("relation ") && lines[1].ends_with(" (supersedes)"));
    assert_eq!(real(&["weave", "check"]).trim(), "no problems");
}

/// A batch (decision db6f7705c0a4), end to end: a repository with the real merge, verify-seq, bk-evidence,
/// code-scope and ground-guards, a committed stub scripts/test-table that fails when the tree holds a
/// file named `broken`, and a local forge whose `curl` stub answers GET /pulls/<n> and does POST
/// /pulls/<n>/merge as a squash merge (git merge-tree onto main). PR #n adds `a<n>.rs`, and `broken` too
/// if it is in `broken`. With `$T/tamper` holding n, the forge's merge of #n leaves main's tree as it was.
/// With `$T/flaky`, the table's next run fails, once; with `$T/flaky-tree`, the next run whose a*.rs files
/// are those it lists (as `a1.rs a2.rs `) does. With `$T/slow`, each run waits 10 minutes before its
/// result; with `$T/slow-tree`, only the run of the tree it lists does. With `$T/die-tree`, that tree's run
/// kills its verify-seq job's leader (SIGKILL), so the job ends without its .done file, once, and goes on
/// in the job's process group, its pid in `$T/orphan`, for 10 minutes; a SIGINT stops it 3 s later, once
/// it has made `$T/orphan-stopped`. With
/// `$T/hook-<n>`, the forge runs it at the
/// `$T/hook-at-<n>`'th GET of /pulls/<n>. With `$T/post-502-<n>`, the POST of #n merges but answers 502,
/// as a proxy can while the forge merges anyway.
struct Batch {
    tmp: tempfile::TempDir,
    dir: PathBuf,
    forge: PathBuf,
    claims: Vec<String>,
    shas: Vec<String>,
}

impl Batch {
    fn new(scripts: &str, prs: usize, broken: &[usize], main_broken: bool) -> Batch {
        Batch::shaped(scripts, prs, broken, main_broken, &|_| {}, &|_| 0, &|_, _| {})
    }

    /// As `new`, with `on_main` adding to main's first commit, PR #n branching from PR #`base(n)`'s branch
    /// (0 for main), and `extra(dir, n)` adding to PR #n before its commit.
    fn shaped(
        scripts: &str,
        prs: usize,
        broken: &[usize],
        main_broken: bool,
        on_main: &dyn Fn(&Path),
        base: &dyn Fn(usize) -> usize,
        extra: &dyn Fn(&Path, usize),
    ) -> Batch {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let names = ["merge", "ground-guards", "code-scope", "verify-seq", "bk-evidence"];
        let paths: Vec<(String, PathBuf)> = names.iter().map(|n| (format!("scripts/{n}"), file(n))).collect();
        let files: Vec<(&str, &Path)> = paths.iter().map(|(to, from)| (to.as_str(), from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/identities/\n", |dir| {
            write_executable(
                &dir.join("scripts/test-table"),
                "#!/bin/sh\ntree=$(ls a*.rs 2>/dev/null | tr '\\n' ' ')\necho \"run $tree\" >>\"$LOG\"\n\
                 if [ -e \"$T/flaky\" ]; then rm \"$T/flaky\"; echo 'test result: FAILED (flaky)'; exit 1; fi\n\
                 if [ -e \"$T/flaky-tree\" ] && [ \"$(cat \"$T/flaky-tree\")\" = \"$tree\" ]; then rm \"$T/flaky-tree\"; echo 'test result: FAILED (flaky)'; exit 1; fi\n\
                 if [ -e \"$T/slow\" ]; then sleep 600; fi\n\
                 if [ -e \"$T/slow-tree\" ] && [ \"$(cat \"$T/slow-tree\")\" = \"$tree\" ]; then sleep 600; fi\n\
                 if [ -e \"$T/die-tree\" ] && [ \"$(cat \"$T/die-tree\")\" = \"$tree\" ]; then rm \"$T/die-tree\"\n\
                   echo $$ >\"$T/orphan\"; trap 'sleep 3; touch \"$T/orphan-stopped\"; exit 130' INT; kill -9 \"$(ps -o pgid= -p $$ | tr -d ' ')\"; sleep 600; fi\n\
                 if [ -e broken ]; then echo 'test result: FAILED'; exit 1; fi\necho 'test-table: all passed'\n",
            );
            if main_broken {
                put(dir, "broken", "on main\n");
            }
            on_main(dir);
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
        bk(&dir, &["init", "--name", "batch"]);
        for who in ["builder", "val", "manager"] {
            bk(&dir, &["id", "new", who]);
        }
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        commit_all(&dir, "ground");
        let mut shas = Vec::new();
        for n in 1..=prs {
            let from = match base(n) {
                0 => "main".to_owned(),
                b => format!("pr{b}"),
            };
            git(&["switch", "-qc", &format!("pr{n}"), &from]);
            put(&dir, &format!("a{n}.rs"), &format!("pr {n}\n"));
            if broken.contains(&n) {
                put(&dir, "broken", &format!("by pr {n}\n"));
            }
            extra(&dir, n);
            commit_all(&dir, &format!("pr {n}"));
            shas.push(String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned());
        }
        git(&["switch", "-q", "main"]);
        let forge = tmp.path().join("forge.git");
        git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
        let mut refs = vec!["main".to_owned()];
        refs.extend((1..=prs).map(|n| format!("pr{n}:refs/pull/{n}/head")));
        let mut push = vec!["push", "-q", forge.to_str().unwrap()];
        push.extend(refs.iter().map(String::as_str));
        git(&push);
        git(&["config", "remote.origin.url", "https://forge.example/o/r.git"]);
        git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
        git(&["config", &format!("url.{}.insteadOf", forge.display()), "https://forge.example/o/r.git"]);
        git(&[
            "config",
            "credential.https://forge.example.helper",
            "!f() { echo username=t; echo password=DUMMY; }; f",
        ]);
        git(&["fetch", "-q", "origin"]);
        let mut claims = Vec::new();
        for (n, sha) in shas.iter().enumerate() {
            let claim = bk(
                &dir,
                &[
                    "claim",
                    &format!("PR #{} pr{} @ {sha}: change {}", n + 1, n + 1, n + 1),
                    "--tag",
                    "pr",
                    "--as",
                    "builder",
                ],
            );
            bk(&dir, &["evidence", &claim, "pass", "--method", "checked", "--as", "val"]);
            claims.push(claim);
        }
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        write_executable(
            &tmp.path().join("stubs/curl"),
            "#!/bin/bash\nmethod=GET body=\nwhile [ $# -gt 0 ]; do case \"$1\" in -X) method=$2; shift 2 ;; \
             --data-binary) body=${2#@}; shift 2 ;; -H) shift 2 ;; *) url=$1; shift ;; esac; done\n\
             n=$(sed -E 's|.*/pulls/([0-9]+).*|\\1|' <<<\"$url\")\n\
             g() { git --git-dir=\"$FORGE\" \"$@\"; }\n\
             if [ \"$method\" = GET ]; then\n\
               echo x >>\"$T/gets-$n\"\n\
               if [ -x \"$T/hook-$n\" ] && [ \"$(wc -l <\"$T/gets-$n\")\" -eq \"$(cat \"$T/hook-at-$n\")\" ]; then \"$T/hook-$n\"; fi\n\
               head=$(g rev-parse \"refs/pull/$n/head\"); merged=false; [ ! -e \"$T/merged-$n\" ] || merged=true\n\
               echo \"{\\\"head\\\":{\\\"sha\\\":\\\"$head\\\"},\\\"state\\\":\\\"open\\\",\\\"merged\\\":$merged,\\\"base\\\":{\\\"ref\\\":\\\"main\\\"},\\\"title\\\":\\\"t$n\\\"}\"\n\
               exit 0\n\
             fi\n\
             sha=$(jq -r .head_commit_id \"$body\")\n\
             tree=$(g merge-tree --write-tree main \"$sha\") || exit 22\n\
             [ \"$(cat \"$T/tamper\" 2>/dev/null)\" != \"$n\" ] || tree=$(g rev-parse 'main^{tree}')\n\
             new=$(g -c user.name=f -c user.email=f@invalid commit-tree \"$tree\" -p main -m \"squash $n\")\n\
             g update-ref refs/heads/main \"$new\"; touch \"$T/merged-$n\"\n\
             if [ -e \"$T/post-502-$n\" ]; then echo 502; exit 22; fi\n\
             echo 200\n",
        );
        Batch { tmp, dir, forge, claims, shas }
    }

    fn command(&self, program: &str, args: &[&str]) -> Command {
        let path = std::env::var("PATH").unwrap();
        let mut c = command(&self.dir, program, args);
        c.env("PATH", format!("{}:{path}", self.tmp.path().join("stubs").display()))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LOG", self.tmp.path().join("log"))
            .env("T", self.tmp.path())
            .env("FORGE", &self.forge)
            .env("TMPDIR", self.tmp.path())
            .env_remove("ON")
            .env_remove("MERGE_BATCH")
            .env_remove("MERGE_BATCH_PRECHECK");
        c
    }

    /// `verify-seq --split --speculative -j <jobs>` over these pairs, in order.
    fn speculative(&self, pairs: &[String], jobs: &str) -> Output {
        let mut args = vec!["--split", "--speculative", "-j", jobs];
        args.extend(pairs.iter().map(String::as_str));
        self.command("scripts/verify-seq", &args).output().unwrap()
    }

    /// PR #n's pair, `<claim>:<sha>`.
    fn pair(&self, n: usize) -> String {
        format!("{}:{}", &self.claims[n - 1][..12], self.shas[n - 1])
    }

    /// The table's runs, sorted: parallel runs log in any order.
    fn sorted_runs(&self) -> Vec<String> {
        let mut runs = self.runs();
        runs.sort();
        runs
    }

    fn verify(&self, members: &[usize]) -> Output {
        let pairs: Vec<String> =
            members.iter().map(|n| format!("{}:{}", &self.claims[n - 1][..12], self.shas[n - 1])).collect();
        let mut args = vec!["--batch"];
        args.extend(pairs.iter().map(String::as_str));
        self.command("scripts/verify-seq", &args).output().unwrap()
    }

    fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.tmp.path().join("log")).unwrap_or_default().lines().map(String::from).collect()
    }

    /// The manager's evidence methods on PR #n's claim, with their verdicts.
    fn evidence(&self, n: usize) -> Vec<(String, String)> {
        let shown: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["show", &self.claims[n - 1], "--json"])).unwrap();
        shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence" && r["author_name"] == "manager")
            .map(|r| {
                (r["body"]["verdict"].as_str().unwrap().to_owned(), r["body"]["method"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    fn batch_claim(&self) -> String {
        let log: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["log", "--tag", "batch", "--json"])).unwrap();
        log[0]["id"].as_str().expect("a batch claim").to_owned()
    }

    fn merge(&self, args: &[&str]) -> Output {
        let mut all = args.to_vec();
        all.extend(["--validator", "val", "--min-passes", "1", "--as", "manager"]);
        self.command("scripts/merge", &all).output().unwrap()
    }

    fn forge_main(&self) -> String {
        let out = run(&self.dir, "git", &["--git-dir", self.forge.to_str().unwrap(), "rev-parse", "main"]);
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn merged(&self) -> Vec<usize> {
        (1..=self.claims.len()).filter(|n| self.tmp.path().join(format!("merged-{n}")).exists()).collect()
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn a_batch_passes_every_member_once_and_merge_batch_merges_them_onto_the_tested_tree() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[], false);
        let out = b.verify(&[1, 2, 3]);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert_eq!(b.runs(), ["run a1.rs a2.rs a3.rs "], "{scripts}: one run, of the whole batch");
        let batch = b.batch_claim();
        let key = String::from_utf8(
            run(&b.dir, "scripts/code-scope", &["chain", "origin/main", &b.shas[0], &b.shas[1], &b.shas[2]]).stdout,
        )
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .split(' ')
        .nth(2)
        .unwrap()
        .to_owned();
        for n in 1..=3 {
            let evidence = b.evidence(n);
            assert_eq!(evidence.len(), 1, "{scripts}: PR #{n}: {evidence:?}");
            let (verdict, method) = &evidence[0];
            assert_eq!(verdict, "pass");
            assert!(
                method.starts_with(&format!("scripts/test-table @ {} batch {batch} on ", b.shas[n - 1])),
                "{method}"
            );
            assert!(method.contains(&format!(" tree {key} run ")), "{scripts}: {method}");
        }

        // The plain form never accepts a prefix: PR #1 alone onto main isn't the tested tree.
        let out = b.merge(&["1", &b.claims[0][..12], "--dry-run"]);
        let line = check_line(&out, "main");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {line}");
        assert!(line.starts_with("FAIL") && line.contains("only for exactly the tested tree"), "{scripts}: {line}");

        // The batch form checks every member, then merges them in order onto the tested tree.
        let out = b.merge(&["--batch", &batch[..12], "--dry-run"]);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("would merge, in order: #1 #2 #3"), "{scripts}: {}", text(&out));
        assert!(b.merged().is_empty(), "{scripts}: a dry run merged");
        let out = b.merge(&["--batch", &batch[..12]]);
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert_eq!(b.merged(), [1, 2, 3], "{scripts}");
        let tip = b.forge_main();
        run(&b.dir, "git", &["fetch", "-q", "origin"]);
        let now = String::from_utf8(run(&b.dir, "scripts/code-scope", &["key", &tip]).stdout).unwrap();
        assert_eq!(now.trim(), key, "{scripts}: main ends on the tested tree");
        for n in 1..=3 {
            assert!(
                b.evidence(n).iter().any(|(v, m)| v == "pass" && m.starts_with("merge gate @ ")),
                "{scripts}: PR #{n}"
            );
        }
    }
}

#[test]
fn a_failing_batch_records_a_fail_only_on_the_member_bisect_pins() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[2], false);
        let out = b.verify(&[1, 2, 3]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        // The whole batch, then prefixes 1 (passes) and 2 (fails), and 2 once more before it is pinned.
        assert_eq!(
            b.runs(),
            ["run a1.rs a2.rs a3.rs ", "run a1.rs ", "run a1.rs a2.rs ", "run a1.rs a2.rs "],
            "{scripts}"
        );
        assert!(
            b.evidence(1).is_empty() && b.evidence(3).is_empty(),
            "{scripts}: {:?} {:?}",
            b.evidence(1),
            b.evidence(3)
        );
        let evidence = b.evidence(2);
        assert_eq!(evidence.len(), 1, "{scripts}: {evidence:?}");
        assert!(evidence[0].0 == "fail" && evidence[0].1.ends_with(" pinned"), "{scripts}: {evidence:?}");

        // A flaky fail of the whole batch: every prefix passes, and the pinned prefix's rerun too, so
        // nothing is recorded on any member (finding 663c221a4c20).
        let b = Batch::new(scripts, 3, &[], false);
        std::fs::write(b.tmp.path().join("flaky"), "").unwrap();
        let out = b.verify(&[1, 2, 3]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("a flaky failure, not pinned"), "{scripts}: {}", text(&out));
        assert_eq!(b.runs().len(), 4, "{scripts}: {:?}", b.runs());
        assert!((1..=3).all(|n| b.evidence(n).is_empty()), "{scripts}");

        // With main itself failing, nothing is pinned on a member.
        let b = Batch::new(scripts, 2, &[], true);
        let out = b.verify(&[1, 2]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("main alone"), "{scripts}: {}", text(&out));
        assert_eq!(b.runs(), ["run a1.rs a2.rs ", "run a1.rs ", "run "], "{scripts}");
        assert!(b.evidence(1).is_empty() && b.evidence(2).is_empty(), "{scripts}");
    }
}

#[test]
fn merge_batch_merges_none_when_a_member_fails_a_check_and_stops_when_main_isnt_the_prefix() {
    for scripts in SCRIPTS {
        // A member whose pr claim was retracted after the run: none of the batch merges.
        let b = Batch::new(scripts, 3, &[], false);
        assert!(b.verify(&[1, 2, 3]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        bk(&b.dir, &["retract", &b.claims[1], "--as", "builder"]);
        let before = b.forge_main();
        let out = b.merge(&["--batch", &batch[..12]]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("member 2 (PR #2) fails a check: merging none"), "{scripts}: {}", text(&out));
        assert!(b.merged().is_empty() && b.forge_main() == before, "{scripts}: something merged");

        // The forge's merge of #2 doesn't give the batch's prefix: it stops there, and #3 isn't merged.
        let b = Batch::new(scripts, 3, &[], false);
        assert!(b.verify(&[1, 2, 3]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        std::fs::write(b.tmp.path().join("tamper"), "2").unwrap();
        let out = b.merge(&["--batch", &batch[..12]]);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {said}");
        assert!(
            said.contains("isn't the batch's prefix 2") && said.contains("stopped at member 2"),
            "{scripts}: {said}"
        );
        // It says what merged by the Ground's live merge-gate passes, so #2, which did merge, is named.
        assert!(said.contains("merge-gate passes: #1 #2.") && said.contains("nothing tested"), "{scripts}: {said}");
        assert_eq!(b.merged(), [1, 2], "{scripts}");

        // main moved in code after the run: the batch is checked against its own main and refused whole.
        let b = Batch::new(scripts, 2, &[], false);
        assert!(b.verify(&[1, 2]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        put(&b.dir, "elsewhere.rs", "main moved\n");
        commit_all(&b.dir, "main moves in code");
        assert!(run(&b.dir, "git", &["push", "-q", b.forge.to_str().unwrap(), "main"]).status.success());
        let out = b.merge(&["--batch", &batch[..12]]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("has moved since the batch's main"), "{scripts}: {}", text(&out));
        assert!(b.merged().is_empty(), "{scripts}");
    }
}

#[test]
fn the_plain_merge_accepts_a_batch_pass_for_exactly_the_tested_tree() {
    for scripts in SCRIPTS {
        // A batch of one: the head merged with main is the tested tree, so the plain form accepts it,
        // even after main moved, in docs only.
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        put(&b.dir, "docs/guide.md", "a guide\n");
        commit_all(&b.dir, "docs on main");
        assert!(run(&b.dir, "git", &["push", "-q", b.forge.to_str().unwrap(), "main"]).status.success());
        let out = b.merge(&["1", &b.claims[0][..12], "--dry-run"]);
        let line = check_line(&out, "main");
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert!(line.contains("tested exactly this head merged with main"), "{scripts}: {line}");
    }
}

impl Batch {
    /// Commits `path` with `text` on the forge's main, as another merge would, through a temporary index.
    fn hook(&self, n: usize, at_get: usize, path: &str, text: &str) {
        write_executable(
            &self.tmp.path().join(format!("hook-{n}")),
            &format!(
                "#!/bin/bash\nset -e\ng() {{ git --git-dir=\"$FORGE\" \"$@\"; }}\nexport GIT_INDEX_FILE=\"$T/hook-index\"\n\
                 g read-tree main\nblob=$(printf '{text}' | g hash-object -w --stdin)\n\
                 g update-index --add --cacheinfo 100644,\"$blob\",'{path}'\n\
                 new=$(g -c user.name=h -c user.email=h@invalid commit-tree \"$(g write-tree)\" -p main -m intruder)\n\
                 g update-ref refs/heads/main \"$new\"\n"
            ),
        );
        std::fs::write(self.tmp.path().join(format!("hook-at-{n}")), at_get.to_string()).unwrap();
    }

    /// main gains `path` on the branch's checkout and the forge, as a merge of other work would.
    fn main_moves(&self, path: &str) {
        put(&self.dir, path, "main moved\n");
        commit_all(&self.dir, "main moves");
        assert!(run(&self.dir, "git", &["push", "-q", self.forge.to_str().unwrap(), "main"]).status.success());
    }
}

/// Finding aa857de39d64: each rule merge has for batch passes, with a case that breaks it.
#[test]
fn merge_counts_a_batch_pass_only_under_every_rule() {
    for scripts in SCRIPTS {
        // The environment carries no batch state (finding f692f52a1a43): the old variables are refused.
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        for var in ["MERGE_BATCH", "MERGE_BATCH_PRECHECK", "MERGE_NO_CLEANUP"] {
            let mut c =
                b.command("scripts/merge", &["1", &b.claims[0][..12], "--validator", "val", "--min-passes", "1"]);
            c.args(["--as", "manager"]).env(var, &batch);
            let out = c.output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{scripts}: {var}: {}", text(&out));
            assert!(text(&out).contains(&format!("{var} is set in the environment")), "{scripts}: {}", text(&out));
            assert!(b.merged().is_empty(), "{scripts}: {var}");
        }

        // The plain exact path refuses once main has moved in code, or in a root .md file, since the run.
        for moved in ["elsewhere.rs", "AGENTS.md"] {
            let b = Batch::new(scripts, 1, &[], false);
            assert!(b.verify(&[1]).status.success(), "{scripts}");
            b.main_moves(moved);
            let out = b.merge(&["1", &b.claims[0][..12], "--dry-run"]);
            let line = check_line(&out, "main");
            assert_eq!(out.status.code(), Some(1), "{scripts}: {moved}: {line}");
            assert!(line.starts_with("FAIL"), "{scripts}: {moved}: {line}");
            // And merge --batch refuses the whole batch.
            let out = b.merge(&["--batch", &b.batch_claim()[..12], "--dry-run"]);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {moved}: {}", text(&out));
            assert!(text(&out).contains("has moved since the batch's main"), "{scripts}: {}", text(&out));
        }

        // A batch claim by anyone but the manager, or superseded, counts for nothing.
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        let statement: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &batch, "--json"])).unwrap();
        let statement = statement["body"]["statement"].as_str().unwrap().to_owned();
        let forged = bk(&b.dir, &["claim", &statement, "--tag", "batch", "--as", "val"]);
        let out = b.merge(&["--batch", &forged[..12], "--dry-run"]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("isn't a current manager claim tagged batch"), "{scripts}: {}", text(&out));
        assert!(b.merge(&["1", &b.claims[0][..12], "--dry-run"]).status.success(), "{scripts}: before superseding");
        bk(&b.dir, &["claim", "a different batch", "--tag", "batch", "--supersedes", &batch, "--as", "manager"]);
        let out = b.merge(&["--batch", &batch[..12], "--dry-run"]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("is superseded"), "{scripts}: {}", text(&out));
        let out = b.merge(&["1", &b.claims[0][..12], "--dry-run"]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{scripts}: a superseded batch still counted: {}",
            check_line(&out, "main")
        );

        // A member pass whose run isn't the manager's pass on the batch claim, or is one of another tree.
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        let batch = b.batch_claim();
        let (_, method) = b.evidence(1).into_iter().next().unwrap();
        let shown: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &b.claims[0], "--json"])).unwrap();
        let ids: Vec<(String, String)> = shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence")
            .map(|r| (r["id"].as_str().unwrap().to_owned(), r["author_name"].as_str().unwrap().to_owned()))
            .collect();
        let real = ids.iter().find(|(_, who)| who == "manager").unwrap().0.clone();
        let vals = ids.iter().find(|(_, who)| who == "val").unwrap().0.clone();
        bk(&b.dir, &["retract", &real, "--as", "manager"]);
        let main_sha =
            String::from_utf8(run(&b.dir, "git", &["rev-parse", "origin/main"]).stdout).unwrap().trim().to_owned();
        let other_tree = bk(
            &b.dir,
            &["evidence", &batch, "pass", "--method", &format!("scripts/test-table @ {main_sha}"), "--as", "manager"],
        );
        let run_id = method.rsplit(' ').next().unwrap().to_owned();
        for wrong in [vals, other_tree] {
            let forged = method.replace(&run_id, &wrong);
            let id = bk(&b.dir, &["evidence", &b.claims[0], "pass", "--method", &forged, "--as", "manager"]);
            let out = b.merge(&["--batch", &batch[..12], "--dry-run"]);
            assert_eq!(out.status.code(), Some(1), "{scripts}: run {wrong}: {}", text(&out));
            assert!(text(&out).contains("no manager batch pass"), "{scripts}: {}", text(&out));
            bk(&b.dir, &["retract", &id, "--as", "manager"]);
        }

        // main moves in code between two members' merges: the second is refused before its POST, and
        // the batch stops with only the first merged.
        let b = Batch::new(scripts, 2, &[], false);
        assert!(b.verify(&[1, 2]).status.success(), "{scripts}");
        b.hook(2, 2, "intruder.rs", "elsewhere");
        let out = b.merge(&["--batch", &b.batch_claim()[..12]]);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {said}");
        assert!(
            said.contains("isn't the batch's prefix 1") && said.contains("stopped at member 2"),
            "{scripts}: {said}"
        );
        assert!(said.contains("by the live merge-gate passes: #1."), "{scripts}: {said}");
        assert_eq!(b.merged(), [1], "{scripts}: #2 was POSTed");
    }
}

/// Finding 3199c8e0a8ad: a member's POST that fails while the forge merges anyway goes through the
/// plain form's outcome handling (PR #107), which finds it merged; the batch's key check still runs, and
/// the batch goes on to the tested tree.
#[test]
fn merge_batch_carries_on_when_a_failed_post_merged_anyway_and_checks_the_prefix() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[], false);
        assert!(b.verify(&[1, 2, 3]).status.success(), "{scripts}");
        std::fs::write(b.tmp.path().join("post-502-2"), "").unwrap();
        let out = b.merge(&["--batch", &b.batch_claim()[..12]]);
        let said = text(&out);
        assert!(out.status.success(), "{scripts}: {said}");
        assert!(
            said.contains("the merge call failed (HTTP 502), but the forge shows PR #2 merged"),
            "{scripts}: {said}"
        );
        assert!(said.contains("main is the batch's prefix 2"), "{scripts}: the key check didn't run after it: {said}");
        assert_eq!(b.merged(), [1, 2, 3], "{scripts}");

        // The same, but what merged isn't the prefix: the key check after the outcome stops the batch.
        let b = Batch::new(scripts, 3, &[], false);
        assert!(b.verify(&[1, 2, 3]).status.success(), "{scripts}");
        std::fs::write(b.tmp.path().join("post-502-2"), "").unwrap();
        std::fs::write(b.tmp.path().join("tamper"), "2").unwrap();
        let out = b.merge(&["--batch", &b.batch_claim()[..12]]);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {said}");
        assert!(said.contains("merged") && said.contains("isn't the batch's prefix 2"), "{scripts}: {said}");
        assert_eq!(b.merged(), [1, 2], "{scripts}");
    }
}

/// Finding 54a83c918a26: merge --batch runs each member's checks with errexit on, so a read that fails
/// stops the gate rather than letting it decide without it. Member 2 has an untriaged finding, and every
/// read of that finding fails: the batch refuses, merging none, as the plain form does.
#[test]
fn merge_batch_refuses_when_a_findings_read_fails() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 2, &[], false);
        assert!(b.verify(&[1, 2]).status.success(), "{scripts}");
        let finding = bk(&b.dir, &["claim", "member 2 breaks x", "--tag", "finding", "--as", "val"]);
        bk(&b.dir, &["relate", &finding, "contradicts", &b.claims[1], "--as", "val"]);
        // bracken, failing `show` of the finding alone, as a pull that drops would.
        let wrapper = b.tmp.path().join("stubs/failing-bracken");
        write_executable(
            &wrapper,
            &format!(
                "#!/usr/bin/env bash\nset -o pipefail\nfor a in \"$@\"; do [ \"$a\" != {finding} ] || {{ echo 'pull dropped' >&2; exit 1; }}; done\n\
                 \"$REAL_BRACKEN\" \"$@\" | cat\n"
            ),
        );
        for (form, args) in
            [("batch", vec!["--batch", &b.batch_claim()[..12]]), ("plain", vec!["2", &b.claims[1][..12]])]
        {
            let mut all = args.clone();
            all.extend(["--validator", "val", "--min-passes", "1", "--as", "manager"]);
            let out = b.command("scripts/merge", &all).env("BRACKEN_BIN", &wrapper).output().unwrap();
            let said = text(&out);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {form}: {said}");
            assert!(said.contains(&format!("couldn't read finding {finding}")), "{scripts}: {form}: {said}");
            assert!(b.merged().is_empty(), "{scripts}: {form}: merged past an unread finding");
        }
    }
}

impl Batch {
    /// The manager's passes on PR #n's claim, by the batch claim each names.
    fn passes_by_batch(&self, n: usize) -> Vec<String> {
        self.evidence(n)
            .iter()
            .filter(|(v, _)| v == "pass")
            .filter_map(|(_, m)| m.split(" batch ").nth(1).map(|r| r[..64].to_owned()))
            .collect()
    }

    /// The speculative batch claims, by their member count.
    fn speculative_claims(&self) -> Vec<(usize, String)> {
        let log: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["log", "--tag", "speculative", "--json"])).unwrap();
        log.as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let statement = c["body"]["statement"].as_str().unwrap();
                let n = statement.strip_prefix("Batch of ").unwrap().split(' ').next().unwrap().parse().unwrap();
                (n, c["id"].as_str().unwrap().to_owned())
            })
            .collect()
    }
}

/// The speculative queue (task 74fb85858e88): every prefix main+A1..Ai is tested at once, each an ordinary
/// batch, and every member of a passing prefix gets its batch pass. The plain merge then takes A1, A2 and
/// A3 in order, each by the exact-key rule, as main reaches the prefix before it.
#[test]
fn a_speculative_queue_tests_every_prefix_at_once_and_the_plain_merge_takes_them_in_order() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[], false);
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert_eq!(
            b.sorted_runs(),
            ["run a1.rs ", "run a1.rs a2.rs ", "run a1.rs a2.rs a3.rs "],
            "{scripts}: each prefix once"
        );
        let mut claims = b.speculative_claims();
        claims.sort();
        assert_eq!(claims.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3], "{scripts}");
        // PR #i is a member of prefixes i..3, and has a pass for each.
        for (n, want) in [(1, 3), (2, 2), (3, 1)] {
            let passes = b.passes_by_batch(n);
            assert_eq!(passes.len(), want, "{scripts}: PR #{n}: {:?}", b.evidence(n));
            assert!(claims[n - 1..].iter().all(|(_, c)| passes.contains(c)), "{scripts}: PR #{n}");
        }
        assert!(text(&out).contains(&format!("scripts/merge --batch {}", &claims[2].1[..12])), "{scripts}");

        for n in 1..=3 {
            let out = b.merge(&[&n.to_string(), &b.claims[n - 1][..12]]);
            assert!(out.status.success(), "{scripts}: merging #{n}: {}", text(&out));
            assert!(check_line(&out, "main").contains("tested exactly this head merged with main"), "{scripts}");
        }
        assert_eq!(b.merged(), [1, 2, 3], "{scripts}");
    }
}

/// A member whose prefix fails twice is pinned with #115's fail. The prefixes before it keep their passes,
/// and the members after it are tested again without it, so the plain merge takes #1 and then #3.
#[test]
fn a_speculative_queue_pins_a_failing_member_and_retests_the_later_ones_without_it() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[2], false);
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("pinned: #2"), "{scripts}: {}", text(&out));
        let runs = b.sorted_runs();
        let count = |r: &str| runs.iter().filter(|x| x.as_str() == r).count();
        assert_eq!(count("run a1.rs a2.rs "), 2, "{scripts}: the failing prefix and its rerun: {runs:?}");
        assert_eq!(count("run a1.rs a3.rs "), 1, "{scripts}: #3 again without #2: {runs:?}");
        assert_eq!(count("run a1.rs "), 1, "{scripts}: prefix 1 isn't rerun: {runs:?}");

        let two = b.evidence(2);
        assert!(two.len() == 1 && two[0].0 == "fail" && two[0].1.ends_with(" pinned"), "{scripts}: {two:?}");
        assert_eq!(b.passes_by_batch(3).len(), 1, "{scripts}: #3's pass, from main+#1+#3: {:?}", b.evidence(3));
        assert!(b.evidence(3).iter().all(|(v, _)| v == "pass"), "{scripts}");
        // #1's passes are prefix 1's and main+#1+#3's, never one of a tree holding #2.
        assert_eq!(b.passes_by_batch(1).len(), 2, "{scripts}: {:?}", b.evidence(1));
        let worktrees = String::from_utf8(run(&b.dir, "git", &["worktree", "list"]).stdout).unwrap();
        assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");

        for n in [1, 3] {
            let out = b.merge(&[&n.to_string(), &b.claims[n - 1][..12]]);
            assert!(out.status.success(), "{scripts}: merging #{n}: {}", text(&out));
        }
        assert_eq!(b.merged(), [1, 3], "{scripts}");
    }
}

/// Answer c3e60ba8b879, Q2: a prefix whose first run fails and whose rerun passes counts as passed. The
/// rerun's pass is recorded, a note on the prefix's claim names the suspected flake, and the queue goes on.
#[test]
fn a_speculative_prefix_that_passes_its_rerun_counts_and_notes_the_flake() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[], false);
        std::fs::write(b.tmp.path().join("flaky-tree"), "a1.rs a2.rs ").unwrap();
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert!(out.status.success(), "{scripts}: {}", text(&out));
        assert_eq!(b.runs().iter().filter(|r| r.as_str() == "run a1.rs a2.rs ").count(), 2, "{scripts}");
        assert!((1..=3).all(|n| b.evidence(n).iter().all(|(v, _)| v == "pass")), "{scripts}: no fail on a member");
        assert_eq!(b.passes_by_batch(2).len(), 2, "{scripts}: {:?}", b.evidence(2));
        let mut claims = b.speculative_claims();
        claims.sort();
        let shown: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &claims[1].1, "--json"])).unwrap();
        let noted = shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "relation" && r["body"]["kind"] == "refines" && r["author_name"] == "manager");
        assert!(noted, "{scripts}: no note on prefix 2's claim: {shown}");
        let notes = bk(&b.dir, &["log", "--tag", "note", "--full"]);
        assert!(notes.contains("Suspected flake") && notes.contains("prefix 2"), "{scripts}: {notes}");
    }
}

/// Answer c3e60ba8b879, Q3: a member that doesn't merge onto the ones before it, or that changes only docs,
/// is refused up front by name, and nothing runs or is written. And the queue's options: --split is
/// needed, --batch isn't allowed with it, and -j takes 1 to 8.
#[test]
fn a_speculative_queue_refuses_conflicting_and_docs_only_members_up_front() {
    for scripts in SCRIPTS {
        // #1 and #2 both add `broken`, differently: #2 doesn't merge onto #1.
        let b = Batch::new(scripts, 3, &[1, 2], false);
        let git = |args: &[&str]| assert!(run(&b.dir, "git", args).status.success(), "git {args:?}");
        // Only the doc: the fixture's Ground is tracked, and committing it here would take it off main.
        git(&["switch", "-qc", "docs", "main"]);
        put(&b.dir, "notes.md", "docs only\n");
        git(&["add", "notes.md"]);
        git(&["commit", "-qm", "docs only"]);
        let docs = String::from_utf8(run(&b.dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
        git(&["switch", "-q", "main"]);
        let claim = bk(&b.dir, &["claim", &format!("PR #9 docs @ {docs}: notes"), "--tag", "pr", "--as", "builder"]);
        let docs_pair = format!("{}:{docs}", &claim[..12]);

        let before = bk(&b.dir, &["log", "--json"]);
        let out = b.speculative(&[b.pair(1), b.pair(2), docs_pair.clone(), b.pair(3)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        let said = text(&out);
        assert!(said.contains("#2 doesn't merge onto main with the members before it"), "{scripts}: {said}");
        assert!(said.contains("#9 changes only docs"), "{scripts}: {said}");
        assert!(!said.contains("#3 "), "{scripts}: #3 merges and changes code: {said}");
        assert!(b.runs().is_empty(), "{scripts}: ran {:?}", b.runs());
        assert_eq!(bk(&b.dir, &["log", "--json"]), before, "{scripts}: wrote to the Ground");

        for args in [
            &["--speculative", &b.pair(1)[..]][..],
            &["--split", "--batch", "--speculative", &b.pair(1)],
            &["--split", "--speculative", "-j", "9", &b.pair(1)],
        ] {
            let out = b.command("scripts/verify-seq", args).output().unwrap();
            assert_eq!(out.status.code(), Some(2), "{scripts}: {args:?}: {}", text(&out));
        }
        assert!(b.runs().is_empty(), "{scripts}");
    }
}

/// A Ctrl-C to the queue's process group, or a SIGTERM to verify-seq alone (finding 30d7d14cd54f (5)),
/// stops every prefix's run, records nothing on any member or prefix, leaves no worktree, and exits 130.
#[test]
fn a_speculative_queue_interrupted_stops_every_run_and_records_nothing() {
    for (scripts, signal) in SCRIPTS.iter().flat_map(|s| [(*s, "-INT"), (*s, "-TERM")]) {
        let b = Batch::new(scripts, 3, &[], false);
        std::fs::write(b.tmp.path().join("slow"), "").unwrap();
        let mut args = vec!["--split".to_owned(), "--speculative".to_owned(), "-j".to_owned(), "4".to_owned()];
        args.extend((1..=3).map(|n| b.pair(n)));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut c = b.command("scripts/verify-seq", &args);
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let mut child = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let started = std::time::Instant::now();
        while b.runs().len() < 3 {
            assert!(child.try_wait().unwrap().is_none(), "{scripts}: verify-seq ended before its runs started");
            assert!(started.elapsed() < std::time::Duration::from_secs(120), "{scripts}: the runs never started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let group = format!("-{}", child.id());
        let target = if signal == "-INT" { group.clone() } else { child.id().to_string() };
        assert!(Command::new("kill").args([signal, "--", &target]).status().unwrap().success());
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > std::time::Duration::from_secs(180) {
                let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
                panic!("{scripts}: verify-seq kept running after a Ctrl-C");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert_eq!(status.code(), Some(130), "{scripts}");
        assert!((1..=3).all(|n| b.evidence(n).is_empty()), "{scripts}: recorded on a member");
        for (_, claim) in b.speculative_claims() {
            let shown: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &claim, "--json"])).unwrap();
            let evidence = shown["referrers"].as_array().unwrap().iter().filter(|r| r["kind"] == "evidence").count();
            assert_eq!(evidence, 0, "{scripts}: recorded on prefix claim {claim}");
        }
        let worktrees = String::from_utf8(run(&b.dir, "git", &["worktree", "list"]).stdout).unwrap();
        assert_eq!(worktrees.lines().count(), 1, "{scripts}: left a worktree: {worktrees}");
    }
}

impl Batch {
    /// The manager's evidence on a batch claim, as (verdict, method) pairs.
    fn claim_evidence(&self, claim: &str) -> Vec<(String, String)> {
        let shown: serde_json::Value = serde_json::from_str(&bk(&self.dir, &["show", claim, "--json"])).unwrap();
        shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence")
            .map(|r| {
                (r["body"]["verdict"].as_str().unwrap().to_owned(), r["body"]["method"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    /// The bracken every script here runs refuses to write evidence on any claim listed in `$T/refuse`.
    fn refuse_evidence_on(&self, claims: &[&str]) {
        std::fs::write(self.tmp.path().join("refuse"), claims.join("\n") + "\n").unwrap();
        write_executable(
            &self.tmp.path().join("bracken"),
            "#!/usr/bin/env bash\nset -o pipefail\n\
             if [ \"$1\" = evidence ] && grep -qxF -- \"$2\" \"$T/refuse\"; then echo 'refused for the test' >&2; exit 1; fi\n\
             \"$REAL_BRACKEN\" \"$@\" | cat\n",
        );
    }

    fn worktrees(&self) -> usize {
        String::from_utf8(run(&self.dir, "git", &["worktree", "list"]).stdout).unwrap().lines().count()
    }
}

/// Finding 30d7d14cd54f (1) and (4). When prefix 1 fails twice, main alone runs: if main passes, A1 is pinned
/// and A2 is tested without it; if main fails too, nothing is pinned. With -j 1 the runs go one at a time, a
/// rerun before main alone, and those before the next prefix.
#[test]
fn a_speculative_queue_pins_the_first_member_only_when_main_alone_passes() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 2, &[1], false);
        let out = b.speculative(&[b.pair(1), b.pair(2)], "1");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert_eq!(
            b.runs(),
            ["run a1.rs ", "run a1.rs ", "run ", "run a2.rs "],
            "{scripts}: prefix 1, its rerun, main alone, then #2 without #1: {}",
            text(&out)
        );
        let one = b.evidence(1);
        assert!(one.len() == 1 && one[0].0 == "fail" && one[0].1.ends_with(" pinned"), "{scripts}: {one:?}");
        assert!(text(&out).contains("pinned: #1"), "{scripts}: {}", text(&out));
        assert_eq!(b.passes_by_batch(2).len(), 1, "{scripts}: {:?}", b.evidence(2));

        // Main itself fails: every prefix fails, and nothing is pinned on a member.
        let b = Batch::new(scripts, 2, &[], true);
        let out = b.speculative(&[b.pair(1), b.pair(2)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(
            text(&out).contains("main alone") && text(&out).contains("nothing pinned"),
            "{scripts}: {}",
            text(&out)
        );
        assert!(!text(&out).contains("pinned: #"), "{scripts}: {}", text(&out));
        assert!(b.evidence(1).is_empty() && b.evidence(2).is_empty(), "{scripts}");
        assert_eq!(b.worktrees(), 1, "{scripts}");
    }
}

/// Task 1e89b53c6a4a: the same, with the real test-table, code-scope and Ground guards, and a stub cargo
/// whose clippy fails on a tree holding `broken`. Main alone, where HEAD is origin/main, runs the whole
/// table but not the guards, which have nothing to compare, so it passes: --batch pins the first member,
/// and --speculative pins it too and then tests the second without it. --speculative's runs are --split
/// ones, in boxes; here its test-table drops --split and runs the real table's plain form.
#[test]
fn main_alone_passes_the_real_table_so_a_failing_first_member_is_pinned() {
    for scripts in SCRIPTS {
        let real = repo_root().join(scripts).join("test-table");
        let shaped = |plain: bool| {
            let on_main = |dir: &Path| {
                if plain {
                    std::fs::copy(&real, dir.join("scripts/test-table")).unwrap();
                } else {
                    std::fs::copy(&real, dir.join("scripts/real-table")).unwrap();
                    write_executable(&dir.join("scripts/test-table"), "#!/bin/sh\nexec scripts/real-table\n");
                }
            };
            let b = Batch::shaped(scripts, 2, &[1], false, &on_main, &|_| 0, &|_, _| {});
            write_executable(
                &b.tmp.path().join("stubs/cargo"),
                "#!/bin/sh\ncase \"$*\" in\n\
                 'fmt --check') echo \"run $(ls a*.rs 2>/dev/null | tr '\\n' ' ')\" >>\"$LOG\" ;;\n\
                 *clippy*) [ ! -e broken ] || { echo 'error: a lint'; exit 1; } ;;\n\
                 esac\n",
            );
            b
        };

        let b = shaped(true);
        let out = b.verify(&[1, 2]);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert_eq!(
            b.runs(),
            ["run a1.rs a2.rs ", "run a1.rs ", "run ", "run a1.rs "],
            "{scripts}: the batch, prefix 1, main alone, prefix 1 again: {}",
            text(&out)
        );
        let one = b.evidence(1);
        assert!(one.len() == 1 && one[0].0 == "fail" && one[0].1.ends_with(" pinned"), "{scripts}: {one:?}");

        let b = shaped(false);
        let out = b.speculative(&[b.pair(1), b.pair(2)], "1");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert_eq!(
            b.runs(),
            ["run a1.rs ", "run a1.rs ", "run ", "run a2.rs "],
            "{scripts}: prefix 1, its rerun, main alone, then #2 without #1: {}",
            text(&out)
        );
        let one = b.evidence(1);
        assert!(one.len() == 1 && one[0].0 == "fail" && one[0].1.ends_with(" pinned"), "{scripts}: {one:?}");
        assert!(text(&out).contains("pinned: #1"), "{scripts}: {}", text(&out));
        assert_eq!(b.passes_by_batch(2).len(), 1, "{scripts}: {:?}", b.evidence(2));
    }
}

/// Finding 30d7d14cd54f (2): at a pin, a later prefix still running is stopped. It records nothing, its
/// worktree goes, and the queue doesn't wait for it.
#[test]
fn a_speculative_pin_stops_a_later_prefix_that_is_still_running() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 3, &[2], false);
        std::fs::write(b.tmp.path().join("slow-tree"), "a1.rs a2.rs a3.rs ").unwrap();
        let started = std::time::Instant::now();
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert!(started.elapsed() < std::time::Duration::from_secs(120), "{scripts}: waited for the slow prefix");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(b.runs().iter().any(|r| r == "run a1.rs a2.rs a3.rs "), "{scripts}: prefix 3 never started");
        let mut claims = b.speculative_claims();
        claims.sort();
        let three = &claims.iter().find(|(n, _)| *n == 3).unwrap().1;
        assert!(
            b.claim_evidence(three).is_empty(),
            "{scripts}: the stopped run recorded {:?}",
            b.claim_evidence(three)
        );
        assert!(text(&out).contains("pinned: #2"), "{scripts}: {}", text(&out));
        assert_eq!(b.passes_by_batch(3).len(), 1, "{scripts}: #3 again without #2: {:?}", b.evidence(3));
        assert_eq!(b.worktrees(), 1, "{scripts}");
    }
}

/// Finding 30d7d14cd54f (3): a later member that merges only on top of the pinned one is named and not run,
/// and the exit is 1. #1 and #2 make the same change to `y`, so they merge; #3, built on #2, changes it
/// again, which merges onto main+#1+#2 but conflicts with main+#1 once #2 is out.
#[test]
fn a_speculative_queue_names_a_member_that_no_longer_merges_without_the_pinned_one() {
    for scripts in SCRIPTS {
        let b = Batch::shaped(
            scripts,
            3,
            &[2],
            false,
            &|dir| put(dir, "y", "1\n2\n3\n"),
            &|n| if n == 3 { 2 } else { 0 },
            &|dir, n| match n {
                1 | 2 => put(dir, "y", "X\n2\n3\n"),
                _ => put(dir, "y", "Y\n2\n3\n"),
            },
        );
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        let said = text(&out);
        assert!(said.contains("pinned: #2"), "{scripts}: {said}");
        assert!(said.contains("#3 doesn't merge onto main without #2: verify it again"), "{scripts}: {said}");
        assert!(b.evidence(3).is_empty(), "{scripts}: {:?}", b.evidence(3));
        assert_eq!(b.passes_by_batch(1).len(), 1, "{scripts}: prefix 1 only: {:?}", b.evidence(1));
        assert_eq!(b.worktrees(), 1, "{scripts}");
    }
}

/// Finding 8bfb7f457462: a pass or a pin that isn't written isn't reported as done, and the exit is 1.
#[test]
fn a_speculative_queue_that_cant_write_a_pass_or_a_pin_says_so_and_fails() {
    for scripts in SCRIPTS {
        // #2's pass is refused: #1 passed, #2 isn't listed as passed, and nothing after it is either.
        let b = Batch::new(scripts, 3, &[], false);
        let refused = b.claims[1].clone();
        b.refuse_evidence_on(&[&refused]);
        let out = b.speculative(&[b.pair(1), b.pair(2), b.pair(3)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        let said = text(&out);
        assert!(said.contains("couldn't record prefix 2's passes"), "{scripts}: {said}");
        assert!(
            said.contains("passed: #1 ") && !said.contains("passed: #2 ") && !said.contains("passed: #3 "),
            "{scripts}: {said}"
        );

        // The pinned fail on #2 is refused: it isn't listed as pinned.
        let b = Batch::new(scripts, 2, &[2], false);
        let refused = b.claims[1].clone();
        b.refuse_evidence_on(&[&refused]);
        let out = b.speculative(&[b.pair(1), b.pair(2)], "4");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        let said = text(&out);
        assert!(said.contains("couldn't record the pinned fail on #2; nothing pinned"), "{scripts}: {said}");
        assert!(!said.contains("pinned: #2"), "{scripts}: {said}");
    }
}

/// Finding 18c1a991466b: a run whose job dies without its .done file doesn't hang the queue. Its run
/// recorded nothing by then, so the queue stops there, exit 1. What the job left running in its process
/// group (its test-table, here) is stopped too (task 1d950948aa3f), and waited for, though it takes a
/// moment to stop, before its worktree is removed (finding fa85e1368475).
#[test]
fn a_speculative_run_that_dies_without_its_done_file_doesnt_hang() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 2, &[], false);
        std::fs::write(b.tmp.path().join("die-tree"), "a1.rs ").unwrap();
        let started = std::time::Instant::now();
        let out = b.speculative(&[b.pair(1), b.pair(2)], "4");
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "{scripts}: hung");
        assert_eq!(out.status.code(), Some(1), "{scripts}: {}", text(&out));
        assert!(text(&out).contains("prefix 1's run recorded nothing"), "{scripts}: {}", text(&out));
        assert!(b.tmp.path().join("orphan-stopped").exists(), "{scripts}: didn't wait for the group: {}", text(&out));
        assert_eq!(b.worktrees(), 1, "{scripts}: a worktree was left: {}", text(&out));
        let orphan = std::fs::read_to_string(b.tmp.path().join("orphan")).unwrap();
        let alive = || {
            Command::new("/bin/kill").args(["-0", orphan.trim()]).stderr(std::process::Stdio::null()).status().unwrap()
        };
        let since = std::time::Instant::now();
        while alive().success() && since.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if alive().success() {
            let _ = Command::new("/bin/kill").args(["-KILL", orphan.trim()]).status();
            panic!("{scripts}: the dead job's test-table {} was left running", orphan.trim());
        }
    }
}

/// pi-prefix (task 68675575fd58) checks node's tarball against its pin before it unpacks anything, and
/// builds only a new prefix. Stubs stand in for the platform and the download, so this runs anywhere:
/// `uname` says macOS on arm64, and `curl` writes bytes that aren't the pinned release.
#[test]
fn pi_prefix_refuses_a_node_tarball_that_doesnt_match_its_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let stubs = tmp.path().join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    write_executable(&stubs.join("uname"), "#!/bin/sh\n[ \"$1\" = -s ] && echo Darwin || echo arm64\n");
    write_executable(
        &stubs.join("curl"),
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\nprintf 'not node' >\"$out\"\n",
    );
    let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
    let script = repo_root().join("scripts/pi-prefix");
    let prefix = tmp.path().join("prefix");
    let out = Command::new(&script).arg(&prefix).env("PATH", &path).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("does not match its pinned sha256; not unpacking it"), "{stderr}");
    assert!(!prefix.join("bin").exists(), "nothing unpacked");

    // Only a new prefix: an existing directory is refused before anything is downloaded.
    std::fs::create_dir(tmp.path().join("there")).unwrap();
    let out = Command::new(&script).arg(tmp.path().join("there")).env("PATH", &path).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("exists: pi-prefix builds a new one"));
    // Elsewhere than macOS on arm64, nothing at all. (Here, the real uname would let it build.)
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let out = Command::new(&script).arg(&prefix).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).contains("builds for macOS on arm64 only"));
    }
}
