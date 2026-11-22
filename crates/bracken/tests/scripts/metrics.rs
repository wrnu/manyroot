//! scripts/metrics (task 9b65c13e9d66): the week's process metrics and the experiments' state, read-only.

use crate::support::*;

/// A date `days` from now, UTC, as YYYY-MM-DD.
fn date_from_now(days: i64) -> String {
    let out = Command::new("jq").args(["-nr", &format!("now + {days} * 86400 | todate[0:10]")]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A throwaway Ground and main line with one of everything metrics counts, all from this week: two merged
/// PRs (one a fix) and a revert on main; a real, a rejected and an untriaged finding; PR #1 reviewed at two
/// heads, PR #2 at one and a Forge-era claim reusing #2's number on another branch at one more, which doesn't
/// count; a misfire claim and a retracted merge-gate pass; an approval and an owner ask;
/// five experiments; and two heads whose builder's last passes came 30 and 10 minutes after their commits,
/// a median of 20. With --today a week ahead, the week counted is this one. It counts each, lists the
/// experiments with their days left (one with no expires line ends 14 days after it was written, one asking
/// for more than 28 days gets 28), lapsed, or kept by the manager's decision, names what one lacks, and
/// writes nothing to the Ground.
#[test]
fn metrics_counts_the_week_and_the_experiments_and_writes_nothing() {
    let metrics = repo_root().join("scripts/metrics");
    let (_tmp, dir) = repo(&[("scripts/metrics", &metrics)], ".bracken/\n", |_| {});
    for (subject, file) in [("Add a thing (#1)", "a"), ("Revert \"Add a thing\"", "b"), ("Fix the thing (#2)", "c")] {
        put(&dir, file, subject);
        commit_all(&dir, subject);
    }
    bk(&dir, &["init", "--name", "metrics"]);
    for who in ["manager", "builder", "reviewer", "security", "owner"] {
        bk(&dir, &["id", "new", who]);
    }
    let claim = |text: &str, tag: &str, who: &str| bk(&dir, &["claim", text, "--tag", tag, "--as", who]);
    let evidence = |on: &str, verdict: &str, method: &str, who: &str| {
        bk(&dir, &["evidence", on, verdict, "--method", method, "--as", who])
    };
    // A Forge-era claim reusing PR #2's number, on another branch, reviewed twice: it isn't PR #2's head. PR #2's
    // own head is told apart by its branch, read from its claim with the merge-gate pass.
    let forge = claim("PR #2 old-forge-branch @ 9999: before the move", "pr", "builder");
    let first = claim("PR #1 a @ 0000: a thing", "pr", "builder");
    let second = claim("PR #1 a @ 1111: a thing", "pr", "builder");
    let fix = claim("PR #2 c @ 2222: a fix", "pr", "builder");
    claim("PR #3 d @ 3333: not merged", "pr", "builder");
    evidence(&first, "fail", "independent review (m) @ 0000", "reviewer");
    evidence(&second, "pass", "independent review (m) @ 1111", "reviewer");
    evidence(&fix, "pass", "security review (m) @ 2222", "security");
    evidence(&fix, "pass", "merge gate @ 2222", "manager");
    evidence(&forge, "fail", "independent review (m) @ 9999", "reviewer");
    evidence(&forge, "pass", "security review (m) @ 9999", "security");
    for (verdict, n) in [(Some("pass"), 1), (Some("fail"), 2), (None, 3)] {
        let finding = claim(&format!("Finding {n}"), "finding", "reviewer");
        if let Some(v) = verdict {
            evidence(&finding, v, "triage", "manager");
        }
    }
    claim("The gate refused a good head", "misfire", "manager");
    let gate = evidence(&fix, "pass", "merge gate @ 2222", "manager");
    bk(&dir, &["retract", &gate, "--as", "manager"]);
    claim("Approval (owner): it", "approval", "owner");
    claim("Owner ask: which?", "owner-ask", "manager");
    let experiment = |name: &str, metric: &str, expires: &str| {
        let mut text = format!("Experiment (manager): {name}\nrule: {name}\nmetric: {metric}\nbaseline: 1\ntarget: 2");
        if !expires.is_empty() {
            text += &format!("\nexpires: {expires}");
        }
        claim(&text, "experiment", "manager")
    };
    let default = experiment("default", "prs_merged", "");
    let capped = experiment("capped", "gate_misfires", "2099-01-01");
    let lapsed = experiment("lapsed", "reverts", "2020-01-01");
    let kept = experiment("kept", "reverts", "2020-01-02");
    let decision = claim("Decision (manager): keep it, the metrics met the target", "decision", "manager");
    bk(&dir, &["relate", &decision, "supports", &kept, "--as", "manager"]);
    let odd = claim("Experiment (manager): odd\nrule: odd\nmetric: vibes\nbaseline: 1", "experiment", "manager");

    // Two heads committed 30 and 10 minutes ago, each with its builder's pass now: a median of 20 minutes.
    // The builder's earlier pass and a reviewer's don't move it.
    let head = |name: &str, minutes_ago: u64| {
        put(&dir, name, name);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let date = format!("@{} +0000", now - minutes_ago * 60);
        assert!(run(&dir, "git", &["add", "-A"]).status.success());
        let out = command(&dir, "git", &["commit", "-qm", name]).env("GIT_COMMITTER_DATE", &date).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
        claim(&format!("PR #4 e @ {sha}: {name}"), "pr", "builder")
    };
    let thirty = head("thirty", 30);
    let ten = head("ten", 10);
    evidence(&thirty, "pass", "cargo fmt --check @ x", "builder");
    let gone = evidence(&ten, "pass", "late @ x", "builder");
    bk(&dir, &["retract", &gone, "--as", "builder"]);
    for pr in [&thirty, &ten] {
        evidence(pr, "pass", "scripts/test-table --remote @ x", "builder");
    }
    evidence(&ten, "pass", "independent review (m) @ x", "reviewer");

    let before = bk(&dir, &["log", "--json"]);
    let today = date_from_now(7);
    let out = run(&dir, "scripts/metrics", &["--today", &today, "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let want = [
        ("prs_merged", serde_json::json!(2)),
        ("reverts", serde_json::json!(1)),
        ("follow_up_fixes", serde_json::json!(1)),
        ("findings_real", serde_json::json!(1)),
        ("findings_rejected", serde_json::json!(1)),
        ("findings_untriaged", serde_json::json!(1)),
        ("findings_real_per_pr", serde_json::json!(0.5)),
        ("review_rounds_per_pr", serde_json::json!(1.5)),
        ("gate_misfires", serde_json::json!(2)),
        ("owner_asks_gated", serde_json::json!(1)),
        ("owner_asks_other", serde_json::json!(1)),
        ("experiments_active", serde_json::json!(3)),
    ];
    for (key, value) in want {
        assert_eq!(got[key], value, "{key}: {got}");
    }
    let minutes = got["builder_pass_minutes"].as_f64().unwrap_or_else(|| panic!("{got}"));
    assert!((19.5..21.5).contains(&minutes), "builder_pass_minutes {minutes}: {got}");
    let state = |id: &str| {
        let all = got["experiments"].as_array().unwrap();
        all.iter().find(|e| e["id"] == id[..12]).unwrap_or_else(|| panic!("{id}: {got}")).clone()
    };
    assert_eq!(state(&default)["days_left"], 7, "{got}");
    assert_eq!(state(&capped)["days_left"], 21, "{got}");
    assert_eq!(state(&lapsed)["state"], "lapsed", "{got}");
    assert_eq!(state(&kept)["state"], "kept", "{got}");
    assert_eq!(state(&odd)["missing"], serde_json::json!(["target", "a known metric"]), "{got}");

    let out = run(&dir, "scripts/metrics", &["--today", &today]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success() && text.starts_with("Metrics, week of ") && text.lines().count() == 1, "{text}");
    for part in [
        "PRs merged 2",
        "review rounds per PR 1.5",
        "gate misfires 2",
        &format!("{} reverts lapsed 2020-01-01", &lapsed[..12]),
    ] {
        assert!(text.contains(part), "lacks {part:?}: {text}");
    }
    assert_eq!(bk(&dir, &["log", "--json"]), before, "metrics wrote to the Ground");

    // A week given that isn't a Monday is refused; a week with nothing in it counts nothing.
    let out = run(&dir, "scripts/metrics", &["--week", "2026-11-18"]);
    assert_eq!(out.status.code(), Some(5), "{}", String::from_utf8_lossy(&out.stderr));
    let out = run(&dir, "scripts/metrics", &["--week", "2020-01-06", "--json"]);
    let empty: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(empty["prs_merged"] == 0 && empty["review_rounds_per_pr"].is_null(), "{empty}");
}

/// The two heuristic proxies and --post (task 8c9ad9bd4518 step 3, decision ff456e4e2fcc). escapes_14d: a
/// confirmed finding naming two paths that PR #5 changed three days before counts once; one whose own pr claim
/// is PR #5's doesn't, nor one never triaged; a revert today of a path #5 changed counts. perf_claims_without_ab:
/// a report giving a time counts, one citing an A/B doesn't. The first typed experiment (process idea
/// 5cae9af16145, as docs/development.md writes it) reads with its metric and lacks nothing. --post records the
/// text as a claim typed metric as runner, and without a runner identity fails and writes nothing.
#[test]
fn metrics_counts_escapes_and_perf_claims_and_posts_as_runner() {
    let metrics = repo_root().join("scripts/metrics");
    let (_tmp, dir) = repo(&[("scripts/metrics", &metrics)], ".bracken/\n", |_| {});
    let commit = |subject: &str, files: &[&str], days_ago: u64| {
        for f in files {
            put(&dir, f, subject);
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let date = format!("@{} +0000", now - days_ago * 86400);
        assert!(run(&dir, "git", &["add", "-A"]).status.success());
        let out = command(&dir, "git", &["commit", "-qm", subject]).env("GIT_COMMITTER_DATE", &date).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };
    std::fs::create_dir_all(dir.join("src")).unwrap();
    commit("Change a and b (#5)", &["src/a.rs", "src/b.rs"], 3);
    commit("Revert \"Change a and b\"", &["src/a.rs"], 0);
    bk(&dir, &["init", "--name", "metrics"]);
    for who in ["manager", "builder", "reviewer"] {
        bk(&dir, &["id", "new", who]);
    }
    let claim = |text: &str, tag: &str, who: &str| bk(&dir, &["claim", text, "--tag", tag, "--as", who]);
    let own = claim("PR #5 ab @ 5555: change a and b", "pr", "builder");
    let later = claim("PR #6 c @ 6666: something else", "pr", "builder");
    let finding = |text: &str, on: &str, triage: Option<&str>| {
        let id = claim(text, "finding", "reviewer");
        bk(&dir, &["relate", &id, "contradicts", on, "--as", "reviewer"]);
        if let Some(v) = triage {
            bk(&dir, &["evidence", &id, v, "--method", "triage", "--as", "manager"]);
        }
    };
    finding("src/a.rs:3 and src/b.rs:1 break it", &later, Some("pass"));
    finding("src/a.rs:4 breaks it", &own, Some("pass"));
    finding("src/b.rs:2 breaks it", &later, None);
    claim("Report: the open took 12 s", "report", "builder");
    claim("Report: 12 s, from a back-to-back A/B against main's binary", "report", "builder");
    let idea = claim("Process idea: performance claims cite an A/B", "process", "builder");
    let expires = date_from_now(14);
    let first = bk(
        &dir,
        &[
            "claim",
            "Experiment (manager, today), adopting process idea 5cae9af16145.\nrule: any performance claim in a pr or report claim cites a back-to-back A/B of main's binary and the head on the same Grounds",
            "--type",
            "experiment",
            "--attr",
            "metric=perf_claims_without_ab",
            "--attr",
            "baseline=14",
            "--attr",
            "target=0",
            "--attr",
            &format!("expires={expires}"),
            "--refines",
            &idea,
            "--as",
            "manager",
        ],
    );

    let today = date_from_now(7);
    let out = run(&dir, "scripts/metrics", &["--today", &today, "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["escapes_14d"], 2, "{got}");
    assert_eq!(got["perf_claims_without_ab"], 1, "{got}");
    let all = got["experiments"].as_array().unwrap();
    let state = all.iter().find(|e| e["id"] == first[..12]).unwrap_or_else(|| panic!("{got}"));
    assert_eq!(state["metric"], "perf_claims_without_ab", "{got}");
    assert_eq!(state["missing"], serde_json::json!([]), "{got}");
    assert_eq!(state["days_left"], 7, "{got}");

    let before = bk(&dir, &["log", "--json"]);
    let out = run(&dir, "scripts/metrics", &["--today", &today, "--post", "--json"]);
    assert!(!out.status.success(), "--post with --json");
    let out = run(&dir, "scripts/metrics", &["--today", &today, "--post"]);
    assert!(!out.status.success(), "posted without a runner: {}", text(&out));
    assert_eq!(bk(&dir, &["log", "--json"]), before, "a failed --post wrote to the Ground");
    bk(&dir, &["id", "new", "runner"]);
    let out = run(&dir, "scripts/metrics", &["--today", &today, "--post"]);
    assert!(out.status.success(), "{}", text(&out));
    let log: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--json"])).unwrap();
    let posted: Vec<_> = log.as_array().unwrap().iter().filter(|c| c["typed"]["type"] == "metric").collect();
    assert_eq!(posted.len(), 1, "{log}");
    let c = posted[0];
    assert_eq!(c["author_name"], "runner", "{c}");
    assert_eq!(c["typed"]["problems"], serde_json::json!([]), "{c}");
    assert_eq!(c["typed"]["data"]["escapes_14d"], "2", "{c}");
    assert!(c["typed"]["data"]["week"].as_str().is_some_and(|w| w.len() == 10), "{c}");
    assert!(c["body"]["statement"].as_str().unwrap().starts_with("Metrics, week of "), "{c}");
}
