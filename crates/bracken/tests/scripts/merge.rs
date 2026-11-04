//! scripts/merge, a PR at a time. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// merge reaches GitHub only through gh's own credential store: it never handles a token, and gh runs
/// with GH_TOKEN, GITHUB_TOKEN, the enterprise token variables and GH_DEBUG unset, so a token the caller
/// exports is never used or logged. The `gh` stub records its arguments and every such variable it sees;
/// it has no login, so merge stops there and says how to log in.
#[test]
fn merge_runs_gh_on_its_own_credential_store_and_never_a_token_from_the_environment() {
    for scripts in SCRIPTS {
        let merge = repo_root().join(scripts).join("merge");
        let (tmp, dir) = repo(&[("scripts/merge", &merge)], ".bracken/\n", |_| {});
        let url = ["config", "remote.origin.url", "https://github.example/o/r.git"];
        assert!(run(&dir, "git", &url).status.success());
        let probe = tmp.path().join("probe.log");
        write_executable(&tmp.path().join("bracken"), "#!/usr/bin/env bash\n[ \"$1\" != heads ] || exit 0\nexit 1\n");
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        write_executable(
            &tmp.path().join("stubs/gh"),
            "#!/usr/bin/env bash\necho \"gh $*\" >>\"$PROBE_LOG\"\n\
             for v in GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_DEBUG; do\n\
               [ -z \"${!v+x}\" ] || echo \"set: $v\" >>\"$PROBE_LOG\"\ndone\n\
             for v in $(compgen -e); do [[ ${!v} != *DUMMY-TOKEN* ]] || echo \"exported: $v\"; done >>\"$PROBE_LOG\"\n\
             exit 1\n",
        );
        let path = std::env::var("PATH").unwrap();
        let out = command(
            &dir,
            "scripts/merge",
            &["1", "0123456789ab", "--validator", "v", "--min-passes", "1", "--dry-run"],
        )
        .env("PATH", format!("{}:{path}", tmp.path().join("stubs").display()))
        .env("PROBE_LOG", &probe)
        .env("GH_TOKEN", "DUMMY-TOKEN-0123")
        .env("GITHUB_TOKEN", "DUMMY-TOKEN-0123")
        .env("GH_DEBUG", "api")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {said}");
        assert!(said.contains("gh has no login for github.example"), "{scripts}: {said}");
        assert!(!said.contains("DUMMY-TOKEN"), "{scripts}: {said}");
        let log = std::fs::read_to_string(&probe).unwrap_or_else(|_| panic!("{scripts}: gh never ran: {said}"));
        assert_eq!(log, "gh auth status --hostname github.example\n", "{scripts}: gh saw a token: {log}");
        let first = "team server: none set up (no .bracken-team); reading the local replica\n";
        assert!(String::from_utf8_lossy(&out.stdout).starts_with(first), "{scripts}: {:?}", out.stdout);
    }
}

/// Runs `scripts/merge --dry-run` for PR #1 against a local stand-in for GitHub: a bare repository
/// standing in for origin (through `insteadOf`) and a `gh` stub that answers every call with the PR. main holds a committed Ground;
/// `on_branch` changes the PR's head and `on_main` what main gains after the branch point, each committed
/// (`git add -A`, so a caller forces anything ignored in itself). The pr claim is by `builder` and passed
/// by `val`, so the gate accepts it.
fn merge_dry_run(scripts: &str, on_branch: impl FnOnce(&Path), on_main: impl FnOnce(&Path)) -> Output {
    merge_dry_run_on(scripts, |_| {}, on_branch, on_main, None)
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
    git(&["config", "remote.origin.url", "https://github.example/o/r.git"]);
    git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
    git(&["config", &format!("url.{}.insteadOf", forge.display()), "https://github.example/o/r.git"]);
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
    write_executable(&tmp.path().join("stubs/gh"), &format!("#!/bin/sh\necho '{pr}'\n"));
    let path = std::env::var("PATH").unwrap();
    let mut merge =
        command(&dir, "scripts/merge", &["1", &claim[..12], "--validator", "val", "--min-passes", "1", "--dry-run"]);
    merge.env("PATH", format!("{}:{path}", tmp.path().join("stubs").display())).env("GIT_CONFIG_NOSYSTEM", "1");
    merge.env("CODE_SCOPE_TOKENS", affected_stub(&tmp.path().join("stubs/bracken-scope")));
    (tmp, dir, merge)
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

    // Task 962bd6b61626: a docs-only or reusing pass recorded without --on, its main and key empty, is
    // still checked: on a code head it fails the scope check.
    for (what, suffix, why) in [
        ("a docs-only pass without --on", " docs-only".to_owned(), "a docs-only pass".to_owned()),
        ("a reusing pass without --on", format!(" reuses {}", "9".repeat(64)), format!("reusing {}", "9".repeat(64))),
    ] {
        let (_tmp, dir, mut merge) = merge_dry_run_command(scripts, |_| {}, code, |_| {}, None);
        let head = String::from_utf8(run(&dir, "git", &["rev-parse", "pr"]).stdout).unwrap().trim().to_owned();
        let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
        let claim = prs[0]["id"].as_str().unwrap().to_owned();
        let method = format!("scripts/test-table @ {head}{suffix}");
        bk(&dir, &["evidence", &claim, "pass", "--method", &method, "--as", "manager"]);
        let out = merge.output().unwrap();
        let line = check_line(&out, "scope");
        assert_eq!(out.status.code(), Some(1), "{}: {line}", case(what));
        assert!(line.starts_with("FAIL") && line.contains(&why), "{}: {line}", case(what));
    }

    // Task c55bf6355adf: a docs-only pass recorded with --on but no tree key, its key empty, on a code head
    // with main ahead in code. It fails the scope check, and its main doesn't count as tested in check 5.
    let what = case("a docs-only pass with --on and no tree");
    let out = merge_dry_run_on(
        scripts,
        |_| {},
        code,
        lib,
        Some(Tested { suffix: " docs-only", ..tested("manager", "main") }),
    );
    let line = check_line(&out, "scope");
    assert_eq!(out.status.code(), Some(1), "{what}: {line}");
    assert!(
        line.starts_with("FAIL") && line.contains("a docs-only pass on main") && line.contains("code.txt"),
        "{what}: {line}"
    );
    let line = check_line(&out, "main");
    assert!(line.starts_with("FAIL") && !line.contains("tested") && line.contains("lib.rs"), "{what}: {line}");

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

/// A light pass (`scripts/test-table --split -p … @ <sha> on <main> tree <key>`, decision `464d3ccca45e`,
/// answer `10cf8a5053a3`) counts as the merged-tree form does when main is ahead in code, but fails the
/// scope check for a head that scripts/code-scope full names, each kind of path in its list; for one
/// whose packages, from code-scope packages, it leaves out, or that has none (findings 0095e19fcdb7,
/// 4825c59b07e9); and unless the pr claim's author has the five passes on the exact sha (finding
/// 013ac6db02d9). A failing code-scope fails it too. Without the merged tree, with main since moved in
/// code, without packages, in the old --light form, or written by anyone but the manager, it doesn't
/// count.
fn merge_counts_a_light_pass_only_where_the_whole_table_isnt_needed(scripts: &str) {
    let lib = |dir: &Path| put(dir, "lib.rs", "main's code\n");
    let case = |what: &str| format!("{scripts}: {what}");
    let key = "1".repeat(40);
    let scope = "-p bracken -p bracken-tree";
    let five = |scope: &str| {
        [
            "cargo fmt --check".to_owned(),
            "cargo clippy --all-targets -- -D warnings".to_owned(),
            "cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings".to_owned(),
            format!("cargo nextest run -P fast {scope}"),
            format!("scripts/test-table --remote {scope}"),
        ]
    };
    // How a case differs from the light pass that counts: what the branch changes, the manager's method
    // ({head} and {tip} filled in), and the builder's passes, with the sha they name.
    struct Light<'a> {
        path: &'a str,
        method: &'a str,
        who: &'a str,
        passes: Vec<String>,
        passes_by: &'a str,
        passes_on_head: bool,
        failing_full: bool,
    }
    let ok_method = "scripts/test-table --split -p bracken -p bracken-tree @ {head} on {tip} tree {key}";
    let base = || Light {
        path: "code.txt",
        method: ok_method,
        who: "manager",
        passes: five(scope).to_vec(),
        passes_by: "builder",
        passes_on_head: true,
        failing_full: false,
    };
    let merge = |l: Light| {
        let (_tmp, dir, mut merge) =
            merge_dry_run_command(scripts, |_| {}, |dir| put(dir, l.path, "a change\n"), lib, None);
        let rev = |r: &str| String::from_utf8(run(&dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let (head, tip, before) = (rev("pr"), rev("main"), rev("main~1"));
        let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
        let claim = prs[0]["id"].as_str().unwrap().to_owned();
        let on = if l.passes_on_head { head.clone() } else { tip.clone() };
        for p in &l.passes {
            bk(&dir, &["evidence", &claim, "pass", "--method", &format!("{p} @ {on}"), "--as", l.passes_by]);
        }
        let method = l
            .method
            .replace("{head}", &head)
            .replace("{tip}", &tip)
            .replace("{before}", &before)
            .replace("{key}", &key);
        bk(&dir, &["evidence", &claim, "pass", "--method", &method, "--as", l.who]);
        if l.failing_full {
            let real = dir.join("scripts/code-scope");
            std::fs::rename(&real, dir.join("scripts/code-scope.real")).unwrap();
            write_executable(
                &real,
                "#!/bin/sh\nif [ \"$1\" = full ]; then echo 'code-scope: no such commit' >&2; exit 2; fi\n\
                 exec \"$(dirname \"$0\")/code-scope.real\" \"$@\"\n",
            );
        }
        merge.output().unwrap()
    };
    let refused = |what: &str, check: &str, l: Light, says: &[&str]| {
        let out = merge(l);
        let line = check_line(&out, check);
        assert_eq!(out.status.code(), Some(1), "{}: {line}", case(what));
        assert!(line.starts_with("FAIL"), "{}: {line}", case(what));
        for s in says {
            assert!(line.contains(s), "{}: expected {s:?}: {line}", case(what));
        }
    };

    let out = merge(base());
    assert!(out.status.success(), "{}: {}", case("a light pass"), String::from_utf8_lossy(&out.stdout));
    assert!(check_line(&out, "scope").starts_with("ok "), "{}", case("a light pass"));
    assert!(check_line(&out, "main").contains("tested"), "{}", case("a light pass"));
    // A wider scope than the head needs counts too.
    let wider = "scripts/test-table --split -p bracken -p bracken-demo -p bracken-tree @ {head} on {tip} tree {key}";
    let out = merge(Light { method: wider, ..base() });
    assert!(out.status.success(), "{}: {}", case("a wider light pass"), String::from_utf8_lossy(&out.stdout));

    for path in [
        "crates/manyroot/src/graph.rs",
        "Cargo.toml",
        "Cargo.lock",
        "docs/protocol.md",
        "crates/tree/src/lib.rs",
        "crates/tree/src/check.rs",
        "crates/tree/Cargo.toml",
        "crates/tree/tests/fixtures/weave-v1/ground.json",
        ".cargo/config.toml",
        "rust-toolchain.toml",
    ] {
        refused(path, "scope", Light { path, ..base() }, &["needs the whole table", path]);
    }

    // The scope comes from the changed files: crates/demo's row needs three packages.
    refused(
        "a scope narrower than the head's",
        "scope",
        Light { path: "crates/demo/src/main.rs", ..base() },
        &["leaves out bracken-demo bracken-experiment"],
    );
    refused(
        "a head with no package to test",
        "scope",
        Light { path: "rustfmt.toml", ..base() },
        &["no package to test"],
    );

    // The builder's five passes, on the exact sha, by the pr claim's author.
    refused("no builder passes", "scope", Light { passes: vec![], ..base() }, &["pr claim's author has no pass on"]);
    refused(
        "the builder's passes on another sha",
        "scope",
        Light { passes_on_head: false, ..base() },
        &["cargo fmt --check", "cargo nextest run -P fast -p bracken -p bracken-tree"],
    );
    refused("val's passes, not the author's", "scope", Light { passes_by: "val", ..base() }, &["no pass on"]);
    let mut no_clippy = five(scope).to_vec();
    no_clippy.remove(1);
    refused(
        "the builder's passes without clippy",
        "scope",
        Light { passes: no_clippy, ..base() },
        &["of: cargo clippy --all-targets -- -D warnings"],
    );
    let mut workspace = five(scope).to_vec();
    workspace[3] = "cargo nextest run -P fast --workspace".to_owned();
    refused(
        "a --workspace fast run",
        "scope",
        Light { passes: workspace, ..base() },
        &["cargo nextest run -P fast -p bracken -p bracken-tree"],
    );
    refused("code-scope full failing", "scope", Light { failing_full: true, ..base() }, &["code-scope full failed"]);

    // Main moved in code since the light pass's main: it doesn't count in 5.
    let before = "scripts/test-table --split -p bracken -p bracken-tree @ {head} on {before} tree {key}";
    refused("main moved in code since the light pass", "main", Light { method: before, ..base() }, &[]);
    for (what, l) in [
        (
            "a light pass of the bare head",
            Light { method: "scripts/test-table --split -p bracken -p bracken-tree @ {head}", ..base() },
        ),
        ("val's light pass", Light { who: "val", ..base() }),
        (
            "a split pass without the merged tree's key",
            Light { method: "scripts/test-table --split -p bracken @ {head} on {tip}", ..base() },
        ),
        (
            "the old --light form",
            Light {
                method: "scripts/test-table --light -p bracken -p bracken-tree @ {head} on {tip} tree {key}",
                ..base()
            },
        ),
    ] {
        refused(what, "main", l, &[]);
    }
}

#[test]
fn merge_counts_a_light_pass_only_where_the_whole_table_isnt_needed_in_scripts() {
    merge_counts_a_light_pass_only_where_the_whole_table_isnt_needed(SCRIPTS[0]);
}

#[test]
fn merge_counts_a_light_pass_only_where_the_whole_table_isnt_needed_in_the_kickoff_templates() {
    merge_counts_a_light_pass_only_where_the_whole_table_isnt_needed(SCRIPTS[1]);
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

/// Finding d494f36dee9d: merge records its "merge gate" pass before it asks GitHub to merge, and a
/// live one is what tells `log --open` a PR merged. When the merge call fails, merge asks GitHub
/// (findings f1fc25380904, 3623855aa0e8). Merged at the head keeps the pass. Open, at any head, retracts
/// it: at once after a 4xx, and after a 5xx or no reply only if it stays open while merge asks again, so a
/// slow merge behind a proxy's timeout keeps it. Anything else stops, naming the evidence, and keeps it.
/// When the pass is retracted, the finding against the claim stays open.
#[test]
fn a_failed_merge_call_retracts_the_gate_evidence_only_when_github_shows_the_pr_unmerged() {
    // The merge call's HTTP status (000: gh got no reply), GitHub's answers to each GET after it (the
    // last repeated), and what becomes of the pass.
    let cases = [
        ("refused, still open", "409", &["open"][..], "retracted"),
        ("refused, the head moved", "409", &["moved"][..], "retracted"),
        ("not mergeable", "405", &["open"][..], "retracted"),
        ("refused, merged at another head", "409", &["merged elsewhere"][..], "unclear"),
        ("reply lost, merged", "502", &["merged"][..], "stands"),
        ("slow merge behind a proxy", "504", &["open", "merged"][..], "stands"),
        ("proxy timeout, never merged", "504", &["open"][..], "retracted"),
        ("dropped, no answer", "000", &["error"][..], "unclear"),
    ];
    for scripts in SCRIPTS {
        for (case, status, answers, expect) in cases {
            let change = |dir: &Path| std::fs::write(dir.join("code.txt"), "a change\n").unwrap();
            let (tmp, dir, mut dry) = merge_dry_run_command(scripts, |_| {}, change, |_| {}, None);
            let out = dry.output().unwrap();
            assert!(out.status.success(), "{scripts}: the dry run: {}", String::from_utf8_lossy(&out.stderr));
            let prs: serde_json::Value = serde_json::from_str(&bk(&dir, &["log", "--tag", "pr", "--json"])).unwrap();
            let claim = prs[0]["id"].as_str().unwrap().to_owned();
            let finding = bk(&dir, &["claim", "it breaks", "--tag", "finding", "--as", "val"]);
            bk(&dir, &["relate", &finding, "contradicts", &claim, "--as", "val"]);
            bk(&dir, &["evidence", &finding, "fail", "--method", "triage: not a bug", "--as", "manager"]);
            // GitHub answers every GET as before until the merge call, which fails; then as the case says.
            let stub = tmp.path().join("stubs/gh");
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
                        _ => "exit 1".to_owned(),
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
                    "#!/bin/sh\ncase \"$*\" in *'-X PUT'*) : >'{posted}'; [ {status} = 000 ] || echo 'HTTP/2.0 {status} X'; exit 1 ;; esac\n\
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

/// Task d5dc6c27c7f2 (finding d9682243cef7): merge's findings check decides through the real binary's
/// `findings`. An untriaged finding refuses the merge, a confirmed one too; one the manager deferred to a
/// current task of theirs doesn't. And a `findings --json` in any other shape refuses it, whatever it
/// says, even with nothing blocking.
#[test]
fn merge_decides_findings_through_the_binary_and_refuses_another_shape() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        let claim = b.claims[0].clone();
        let dry = |b: &Batch| b.merge(&["1", &claim[..12], "--dry-run"]);
        assert!(dry(&b).status.success(), "{scripts}: {}", text(&dry(&b)));
        let finding = bk(&b.dir, &["claim", "member 1 breaks x", "--tag", "finding", "--as", "val"]);
        bk(&b.dir, &["relate", &finding, "contradicts", &claim, "--as", "val"]);
        let f = &finding[..12];

        let out = dry(&b);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: untriaged: {said}");
        assert!(said.contains(&format!("untriaged (no manager evidence): {f};")), "{scripts}: {said}");

        bk(&b.dir, &["evidence", &finding, "pass", "--method", "triage", "--as", "manager"]);
        let out = dry(&b);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: confirmed: {said}");
        assert!(said.contains("untriaged (no manager evidence): none;"), "{scripts}: {said}");
        assert!(said.contains(&format!("depends_on it): {f}")), "{scripts}: {said}");

        let task = bk(&b.dir, &["claim", "fix x later", "--tag", "task", "--tag", "deferred", "--as", "manager"]);
        bk(&b.dir, &["relate", &task, "depends_on", &finding, "--as", "manager"]);
        let out = dry(&b);
        let said = text(&out);
        assert!(out.status.success(), "{scripts}: deferred: {said}");
        assert!(said.contains(&format!("confirmed and deferred: {f}->{}", &task[..12])), "{scripts}: {said}");

        // The same Ground, but `findings --json` answers in another shape: refused every time.
        let wrapper = b.tmp.path().join("stubs/shaped-bracken");
        write_executable(
            &wrapper,
            "#!/usr/bin/env bash\nfor a in \"$@\"; do [ \"$a\" != findings ] || { cat \"$T/shape\"; exit 0; }; done\n\
             exec \"$REAL_BRACKEN\" \"$@\"\n",
        );
        let id = "a".repeat(64);
        for shape in [
            r#"{"findings":5,"blocking":0}"#.to_owned(),
            format!(r#"{{"findings":[{{"id":"{id}","triage":"weird"}}],"blocking":0}}"#),
            format!(r#"{{"findings":[{{"id":"{id}","triage":"deferred","task":null}}],"blocking":0}}"#),
            format!(r#"{{"findings":[{{"id":"{id}","triage":"untriaged"}}],"blocking":0}}"#),
            r#"{"findings":[],"blocking":"0"}"#.to_owned(),
            r#"{"findings":[{"id":"short","triage":"rejected"}],"blocking":0}"#.to_owned(),
            // Finding 6fd12dd898ee: two documents, the last of them clean, and an id that splits away.
            format!(
                "{{\"findings\":[{{\"id\":\"{id}\",\"triage\":\"untriaged\"}}],\"blocking\":1}}\n{{\"findings\":[],\"blocking\":0}}"
            ),
            format!(r#"{{"findings":[{{"id":"{}","triage":"untriaged"}}],"blocking":1}}"#, " ".repeat(64)),
            // Finding b0cec6d917b6: an id, and a task, of 64 hex digits and a final newline.
            format!(r#"{{"findings":[{{"id":"{id}\n","triage":"rejected"}}],"blocking":0}}"#),
            format!(r#"{{"findings":[{{"id":"{id}","triage":"deferred","task":"{id}\n"}}],"blocking":0}}"#),
            format!(
                r#"{{"findings":[{{"id":"{}","triage":"deferred","task":"{}"}}],"blocking":0}}"#,
                id,
                "A".repeat(64)
            ),
        ] {
            std::fs::write(b.tmp.path().join("shape"), &shape).unwrap();
            let mut all = vec!["1", &claim[..12], "--dry-run"];
            all.extend(["--validator", "val", "--min-passes", "1", "--as", "manager"]);
            let out = b.command("scripts/merge", &all).env("BRACKEN_BIN", &wrapper).output().unwrap();
            let said = text(&out);
            assert_eq!(out.status.code(), Some(1), "{scripts}: {shape}: {said}");
            assert!(said.contains("isn't in the shape this check reads"), "{scripts}: {shape}: {said}");
        }
        assert!(b.merged().is_empty(), "{scripts}: a dry run merged");
    }
}

/// GitHub merges only the exact head merge checked: merge sends that sha with the squash call, so a push
/// that lands between the checks and the call is refused (409), nothing merges, and the gate's pass is
/// retracted.
#[test]
fn merge_sends_the_checked_sha_so_github_refuses_a_head_that_moved_before_the_call() {
    for scripts in SCRIPTS {
        let b = Batch::new(scripts, 1, &[], false);
        assert!(b.verify(&[1]).status.success(), "{scripts}");
        let before = b.forge_main();
        std::fs::write(b.tmp.path().join("move-1"), "").unwrap();
        let out = b.merge(&["1", &b.claims[0][..12]]);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {said}");
        assert!(said.contains("HTTP 409") && said.contains("is retracted"), "{scripts}: {said}");
        assert!(b.merged().is_empty() && b.forge_main() == before, "{scripts}: merged a moved head: {said}");
        let gate = b.evidence(1).into_iter().filter(|(_, m)| m.starts_with("merge gate")).count();
        assert_eq!(gate, 1, "{scripts}");
        let open: serde_json::Value = serde_json::from_str(&bk(&b.dir, &["show", &b.claims[0], "--json"])).unwrap();
        let live = open["referrers"].as_array().unwrap().iter().any(|r| {
            r["body"]["method"].as_str().is_some_and(|m| m.starts_with("merge gate")) && r["retracted"] == false
        });
        assert!(!live, "{scripts}: the moved head's gate pass stands");
    }
}

/// GitHub's merge call can't delete the head branch, so merge does after the merge: a branch of origin's
/// repository still at the merged sha. A fork's branch, or one pushed to since, is left alone, and the
/// merge stands either way.
#[test]
fn merge_deletes_the_merged_head_branch_only_in_origins_repository_at_the_merged_sha() {
    for scripts in SCRIPTS {
        for case in ["ours", "fork", "moved"] {
            let b = Batch::new(scripts, 1, &[], false);
            assert!(b.verify(&[1]).status.success(), "{scripts}");
            let forge = b.forge.to_str().unwrap().to_owned();
            assert!(run(&b.dir, "git", &["push", "-q", &forge, "pr1:refs/heads/pr1"]).status.success());
            match case {
                "fork" => std::fs::write(b.tmp.path().join("fork-1"), "").unwrap(),
                "moved" => {
                    let git = ["--git-dir", &forge, "update-ref", "refs/heads/pr1", "main"];
                    assert!(run(&b.dir, "git", &git).status.success());
                }
                _ => {}
            }
            let out = b.merge(&["1", &b.claims[0][..12]]);
            let said = text(&out);
            assert!(out.status.success(), "{scripts}, {case}: {said}");
            assert_eq!(b.merged(), [1], "{scripts}, {case}");
            let deleted = std::fs::read_to_string(b.tmp.path().join("deleted")).unwrap_or_default();
            let left = run(&b.dir, "git", &["--git-dir", &forge, "rev-parse", "--verify", "-q", "refs/heads/pr1"]);
            if case == "ours" {
                assert!(deleted == "pr1\n" && said.contains("deleted branch pr1"), "{scripts}: {said}");
                assert!(!left.status.success(), "{scripts}: pr1 is still on origin");
            } else {
                assert!(deleted.is_empty() && !said.contains("deleted branch"), "{scripts}, {case}: {said}");
                assert!(left.status.success(), "{scripts}, {case}: pr1 was deleted");
            }
        }
    }
}
