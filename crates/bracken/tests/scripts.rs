//! The team scripts in `scripts/` (and their copies in the agent-team-kickoff skill), run
//! against a throwaway Ground in a throwaway git repository.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("BRACKEN_BIN", dir.join("../bracken"))
        .env("REAL_BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap()
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
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    let wrapper = tmp.path().join("bracken");
    std::fs::write(&wrapper, "#!/usr/bin/env bash\nset -o pipefail\n\"$REAL_BRACKEN\" \"$@\" | cat\n").unwrap();
    std::fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::copy(repo_root().join("scripts/bk"), dir.join("scripts/bk")).unwrap();
    std::fs::copy(bk_evidence, dir.join("scripts/bk-evidence")).unwrap();
    std::fs::write(dir.join(".gitignore"), ".bracken/\n").unwrap();
    let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "scripts"]);
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    bk(&dir, &["init", "--name", "scripts"]);
    bk(&dir, &["id", "new", "val"]);
    (tmp, dir, sha)
}

/// --any-head must refuse a pr claim even when `log --tag pr --json` is over the 64 KiB pipe
/// buffer: a search that stops at the first match must not turn the writer's SIGPIPE into a pass.
#[test]
fn bk_evidence_any_head_refuses_a_pr_claim_in_a_large_pr_log() {
    for script in ["scripts/bk-evidence", ".claude/skills/agent-team-kickoff/templates/bk-evidence"] {
        let (_tmp, dir, sha) = workspace(&repo_root().join(script));
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
