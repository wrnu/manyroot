//! The team scripts in `scripts/` (and their copies in the agent-team-kickoff skill), run
//! against a throwaway Ground in a throwaway git repository.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use manyroot::Body;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const SCRIPTS: [&str; 2] = ["scripts", ".claude/skills/agent-team-kickoff/templates"];

fn command(dir: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env("BRACKEN_BIN", dir.join("../bracken"))
        .env("REAL_BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT");
    command
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    command(dir, program, args).output().unwrap()
}

fn write_executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn bk(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, "scripts/bk", args);
    assert!(out.status.success(), "bk {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A git repository holding `scripts/bk` and the given `bk-evidence`, with a Ground and identity `val`.
///
/// `scripts/bk` runs bracken through a pipe that fails when its reader stops early, as an
/// older bracken or a `printf | grep -q` did: the scripts must not depend on how the writer
/// takes a closed pipe.
fn workspace(bk_evidence: &Path) -> (tempfile::TempDir, PathBuf, String) {
    let (tmp, dir) = repo(&[("scripts/bk-evidence", bk_evidence)], ".bracken/\n", |_| {});
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
    std::fs::copy(repo_root().join("scripts/bk"), dir.join("scripts/bk")).unwrap();
    for (to, from) in files {
        std::fs::copy(from, dir.join(to)).unwrap();
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
/// (dummy) token, by name; it runs only once the token is set.
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
            "#!/usr/bin/env bash\necho ran >>\"$PROBE_LOG\"\n\
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
    }
}
