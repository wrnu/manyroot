//! scripts/metrics (task 9b65c13e9d66): the week's process metrics and the experiments' state, read-only.

use crate::support::*;

/// A date `days` from now, UTC, as YYYY-MM-DD.
fn date_from_now(days: i64) -> String {
    let out = Command::new("jq").args(["-nr", &format!("now + {days} * 86400 | todate[0:10]")]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A throwaway Ground and main line with one of everything metrics counts, all from this week: two merged
/// PRs (one a fix) and a revert on main; a real, a rejected and an untriaged finding; PR #1 reviewed at two
/// heads and PR #2 at one; a misfire claim and a retracted merge-gate pass; an approval and an owner ask;
/// and five experiments. With --today a week ahead, the week counted is this one. It counts each, lists the
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
    let first = claim("PR #1 a @ 0000: a thing", "pr", "builder");
    let second = claim("PR #1 a @ 1111: a thing", "pr", "builder");
    let fix = claim("PR #2 c @ 2222: a fix", "pr", "builder");
    claim("PR #3 d @ 3333: not merged", "pr", "builder");
    evidence(&first, "fail", "independent review (m) @ 0000", "reviewer");
    evidence(&second, "pass", "independent review (m) @ 1111", "reviewer");
    evidence(&fix, "pass", "security review (m) @ 2222", "security");
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
    let state = |id: &str| {
        let all = got["experiments"].as_array().unwrap();
        all.iter().find(|e| e["id"] == &id[..12]).unwrap_or_else(|| panic!("{id}: {got}")).clone()
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
