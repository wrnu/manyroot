//! scripts/ci-result: a head's GitHub Actions result, read through gh, as a check bk-evidence records.

use crate::support::*;

/// A repository with ci-result, bk-evidence and code-scope, a Ground with identity `val`, origin on a
/// GitHub URL, and a `gh` stub that answers the runs query from `$T/runs.json` and the jobs query from
/// `$T/jobs.json`, logging each call and any token variable it sees to `$T/gh.log`.
fn ci_repo() -> (tempfile::TempDir, PathBuf, String) {
    let (tmp, dir, _) = workspace(&repo_root().join("scripts/bk-evidence"));
    copy_executable(&repo_root().join("scripts/ci-result"), &dir.join("scripts/ci-result"));
    commit_all(&dir, "ci-result");
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    let url = ["config", "remote.origin.url", "https://github.example/o/r.git"];
    assert!(run(&dir, "git", &url).status.success());
    std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
    write_executable(
        &tmp.path().join("stubs/gh"),
        "#!/usr/bin/env bash\necho \"gh $*\" >>\"$T/gh.log\"\n\
         for v in GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_DEBUG; do\n\
           [ -z \"${!v+x}\" ] || echo \"set: $v\" >>\"$T/gh.log\"\ndone\n\
         case \"${!#}\" in\n\
           */actions/workflows/ci.yml/runs\\?head_sha=*) cat \"$T/runs.json\" ;;\n\
           */actions/runs/*/jobs*) cat \"$T/jobs.json\" ;;\n\
           *) exit 1 ;;\nesac\n",
    );
    (tmp, dir, sha)
}

fn ci(tmp: &Path, dir: &Path, args: &[&str]) -> Output {
    let path = format!("{}:{}", tmp.join("stubs").display(), std::env::var("PATH").unwrap());
    command(dir, "scripts/ci-result", args)
        .env("PATH", path)
        .env("T", tmp)
        .env("GH_TOKEN", "DUMMY-TOKEN-0123")
        .env("GITHUB_TOKEN", "DUMMY-TOKEN-0123")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn run_json(id: u64, sha: &str, event: &str, status: &str, conclusion: Option<&str>) -> String {
    let conclusion = conclusion.map_or("null".to_owned(), |c| format!("\"{c}\""));
    let base = "b".repeat(40);
    format!(
        r#"{{"id":{id},"head_sha":"{sha}","event":"{event}","status":"{status}","conclusion":{conclusion},"run_attempt":1,"html_url":"https://github.example/o/r/actions/runs/{id}","pull_requests":[{{"base":{{"ref":"main","sha":"{base}"}}}}]}}"#
    )
}

fn runs(tmp: &Path, runs: &[String]) {
    std::fs::write(tmp.join("runs.json"), format!(r#"{{"workflow_runs":[{}]}}"#, runs.join(","))).unwrap();
}

fn jobs(tmp: &Path, jobs: &[(&str, &str)]) {
    let jobs: Vec<String> =
        jobs.iter().map(|(n, c)| format!(r#"{{"name":"{n}","status":"completed","conclusion":"{c}"}}"#)).collect();
    std::fs::write(tmp.join("jobs.json"), format!(r#"{{"jobs":[{}]}}"#, jobs.join(","))).unwrap();
}

/// It takes the head's latest pull_request run (the head merged with main), or with --event the latest of
/// that event, and passes only on a completed run whose conclusion and every job's are success. A run of
/// another sha is never read; none, or one still running, says so with its own exit code. gh never sees
/// a token from the environment, and only GET requests are made.
#[test]
fn ci_result_reads_the_heads_run_and_passes_only_on_success() {
    let (tmp, dir, sha) = ci_repo();
    let t = tmp.path();
    let other = "c".repeat(40);
    let both = [("Test table (Linux)", "success"), ("macOS-only tests and clippy (macOS)", "success")];

    runs(t, &[run_json(9, &other, "pull_request", "completed", Some("success"))]);
    jobs(t, &both);
    let out = ci(t, &dir, &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("no ci.yml run on o/r for"), "{}", text(&out));

    runs(
        t,
        &[
            run_json(5, &sha, "pull_request", "completed", Some("failure")),
            run_json(7, &sha, "pull_request", "completed", Some("success")),
            run_json(8, &sha, "push", "completed", Some("failure")),
        ],
    );
    let out = ci(t, &dir, &[]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("https://github.example/o/r/actions/runs/7"), "{said}");
    assert!(said.contains("the head merged with main at bbbb"), "{said}");
    assert!(said.contains(&format!("passed at {sha}")), "{said}");

    let out = ci(t, &dir, &["--event", "push", &sha]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("runs/8"), "{}", text(&out));

    jobs(t, &[("Test table (Linux)", "success"), ("macOS-only tests and clippy (macOS)", "failure")]);
    let out = ci(t, &dir, &[]);
    assert_eq!(out.status.code(), Some(1), "a failed job passed: {}", text(&out));
    jobs(t, &[]);
    let out = ci(t, &dir, &[]);
    assert_eq!(out.status.code(), Some(1), "a run without jobs passed: {}", text(&out));

    runs(t, &[run_json(10, &sha, "pull_request", "in_progress", None)]);
    let out = ci(t, &dir, &[]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains("hasn't completed"), "{}", text(&out));

    let log = std::fs::read_to_string(t.join("gh.log")).unwrap();
    assert!(!log.contains("set: "), "gh saw a token variable: {log}");
    assert!(log.lines().all(|l| l.contains(" -X GET ") && l.contains("--hostname github.example")), "{log}");
}

/// --record runs bk-evidence on the pr claim with ci-result as the command: a pass naming
/// `scripts/ci-result @ <head>`, and a fail when the run failed.
#[test]
fn ci_result_record_puts_the_result_on_the_pr_claim_through_bk_evidence() {
    let (tmp, dir, sha) = ci_repo();
    let t = tmp.path();
    let claim = bk(&dir, &["claim", &format!("PR #1 b @ {sha}: a change"), "--tag", "pr", "--as", "val"]);
    runs(t, &[run_json(7, &sha, "pull_request", "completed", Some("success"))]);
    jobs(t, &[("Test table (Linux)", "success")]);
    let out = ci(t, &dir, &["--record", &claim[..12], "--as", "val"]);
    assert!(out.status.success(), "{}", text(&out));
    runs(t, &[run_json(8, &sha, "pull_request", "completed", Some("failure"))]);
    let out = ci(t, &dir, &["--record", &claim[..12], "--as", "val"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));

    let shown: serde_json::Value = serde_json::from_str(&bk(&dir, &["show", &claim, "--json"])).unwrap();
    let evidence: Vec<(String, String)> = shown["referrers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "evidence")
        .map(|r| (r["body"]["verdict"].as_str().unwrap().to_owned(), r["body"]["method"].as_str().unwrap().to_owned()))
        .collect();
    let method = format!("scripts/ci-result @ {sha}");
    assert_eq!(evidence.len(), 2, "{evidence:?}");
    assert!(evidence.contains(&("pass".to_owned(), method.clone())), "{evidence:?}");
    assert!(evidence.contains(&("fail".to_owned(), method)), "{evidence:?}");
}
