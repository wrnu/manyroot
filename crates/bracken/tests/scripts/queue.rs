//! Batches and speculative queues: verify-seq --batch and --speculative, and merge --batch. Each test runs the script and its kickoff-template copy.

use crate::support::*;

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
        // bracken, failing the `findings` read alone, as a pull that drops would.
        let wrapper = b.tmp.path().join("stubs/failing-bracken");
        write_executable(
            &wrapper,
            "#!/usr/bin/env bash\nset -o pipefail\nfor a in \"$@\"; do [ \"$a\" != findings ] || { echo 'pull dropped' >&2; exit 1; }; done\n\
             \"$REAL_BRACKEN\" \"$@\" | cat\n",
        );
        for (form, args) in
            [("batch", vec!["--batch", &b.batch_claim()[..12]]), ("plain", vec!["2", &b.claims[1][..12]])]
        {
            let mut all = args.clone();
            all.extend(["--validator", "val", "--min-passes", "1", "--as", "manager"]);
            let out = b.command("scripts/merge", &all).env("BRACKEN_BIN", &wrapper).output().unwrap();
            let said = text(&out);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {form}: {said}");
            assert!(said.contains("couldn't read the findings against"), "{scripts}: {form}: {said}");
            assert!(b.merged().is_empty(), "{scripts}: {form}: merged past an unread finding");
        }
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
