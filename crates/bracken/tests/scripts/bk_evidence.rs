//! scripts/bk-evidence. Each test runs the script and its kickoff-template copy.

use crate::support::*;

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
