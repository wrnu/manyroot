//! scripts/weave, weave-shadow and weave-backup. Each test runs the script and its kickoff-template copy.

use crate::support::*;

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
    // The plan was asked for without the caller's --json, and the run itself gets the arguments as given,
    // and --runner <the main checkout>, where a box or mac check's pinned runner script is (plan
    // cc496dcee0e3).
    let main = dir.canonicalize().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("RAN --as weave-runner weave validate --manifest blake3:00 --json --runner {}", main.display())
    );
    let out = planned(r#"{"signer": "aa", "validators": ["aa"]}"#, &["weave", "validate", "--", "x"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("takes no '--'"), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("RAN"));
    // The caller's own --runner is refused, on validate and on a line write.
    for args in [
        &[&validate[..], &["--runner", "/tmp"]].concat()[..],
        &["weave", "line", "policy", "main", "--runner=/tmp", "--owner-decision", "0123456789abcdef"],
    ] {
        let out = planned(r#"{"signer": "aa", "validators": ["aa"]}"#, args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("--runner is refused"), "{args:?}: {stderr}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("RAN"), "{args:?}: it ran");
    }

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
/// recorded decision that no longer matches git's tree (through git replace) fails. A second line named
/// main, by someone else, changes none of it: the script uses the default line, never the name (finding
/// b5b5871cc017). Never the real ~/bracken-weave: HOME is under $TMPDIR.
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
    let replaced = bk(&dir, &["claim", "Approval (owner): an old go", "--tag", "approval", "--as", "owner"]);
    bk(
        &dir,
        &["claim", "Approval (owner): a newer go", "--tag", "approval", "--supersedes", &replaced, "--as", "owner"],
    );
    refused(&["--init", "--owner-decision", &replaced], "not retracted and not superseded");
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
    // With the default line there, a policy dry run that fails stops --init with its error, and nothing is
    // written: never a second line main (finding b33d60c94e6d).
    let count = || {
        let out =
            command(&dir, "scripts/weave", &["log", "--json", "--limit", "0"]).env("HOME", &home).output().unwrap();
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["omitted"].as_u64().unwrap()
    };
    let before = count();
    let no_dry_run = tmp.path().join("no-dry-run");
    write_executable(
        &no_dry_run,
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" line policy \"*\" --dry-run \"*) echo no-dry-run: refused >&2; exit 1 ;; esac\n\
             exec {:?} \"$@\"\n",
            tmp.path().join("bracken")
        ),
    );
    let out = run_in(Some(&no_dry_run), &["--init", "--owner-decision", &approval]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no-dry-run: refused") && stderr.contains("can't read line main's policy"), "{stderr}");
    assert_eq!(count(), before, "a failed dry run wrote something");
    // Nor does a weave head that fails for another reason than having no default line (finding b9e95bfb20fc).
    let no_head = tmp.path().join("no-head");
    write_executable(
        &no_head,
        &format!(
            "#!/bin/sh\ncase \" $* \" in *\" weave head \"*) echo no-head: broken >&2; exit 1 ;; esac\nexec {:?} \"$@\"\n",
            tmp.path().join("bracken")
        ),
    );
    let out = run_in(Some(&no_head), &["--init", "--owner-decision", &approval]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no-head: broken") && stderr.contains("weave head failed"), "{stderr}");
    assert_eq!(count(), before, "a failed weave head wrote something");
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

    // Someone else's line named main: it never becomes the default line, and from here on naming main is
    // ambiguous, so the script mustn't name it.
    let weave_ok = |args: &[&str]| {
        let out = command(&dir, "scripts/weave", args).env("HOME", &home).output().unwrap();
        assert!(out.status.success(), "weave {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    weave_ok(&["id", "new", "mallory"]);
    weave_ok(&["weave", "line", "create", "main", "--owner-decision", &decision, "--as", "mallory"]);
    let out = command(&dir, "scripts/weave", &["weave", "head", "--line", "main"]).env("HOME", &home).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 lines are named"), "{out:?}");
    refused(&["--init", "--owner-decision", &approval], "already set up");

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
    // The commit not on origin/main, `unmerged`, is a child of commits[2] whose tree is commits[2]'s with
    // one file more, outside .bracken/, which Weave's recorded tree leaves out: made through a scratch
    // index, so that it differs from commits[2]'s whatever the repository's last commits changed (task
    // bb59a076515a: two of them may differ only in .bracken/).
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

    // A mismatch fails: a merged commit (here `unmerged`, once origin/main points at it), recorded, then
    // shown by git with another tree (git replace, by commits[2], whose tree lacks the extra file), no longer
    // verifies.
    git_out(&clone, &["update-ref", "refs/remotes/origin/main", &unmerged]);
    let out = in_clone(&unmerged);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git_out(&clone, &["replace", &unmerged, &commits[2]]);
    let out = in_clone(&unmerged);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(&format!("doesn't verify equal to git's {unmerged}")), "{stderr}");
}

/// Weave dogfood stage 2a (task 87a1bcba025d, design c4fe14c88fe1): a builder proposes each pushed head with
/// scripts/weave-propose, against the decision that mirrors its merge-base or its nearest recorded
/// first-parent ancestor; after git's merge, weave-shadow --proposal has Weave merge it itself, written only
/// if it equals git's tree. A merge commit whose first parent is a checkpoint can bring in a batch of
/// squashes, one --proposal each, merged in squash order into one decision; one PR without a proposal makes
/// the whole merge an import, noted with the PR. Each proposal must be of a PR the merge brings in, and of
/// the head its current pr claim names. A per-file conflict falls back to the import, noted with its paths;
/// a merge that differs from git's stops with nothing written, and --mismatched then imports, noted. Merges
/// are shadowed in order, and each run counts the outcomes. Only weave-integrator moves a line. Findings
/// 155debbbee94, 1f66448c728d and da532127b55f. Never the real ~/bracken-weave: HOME is under $TMPDIR.
#[test]
fn weave_shadow_merges_builders_proposals_and_counts_the_fallbacks() {
    let shadow = repo_root().join("scripts/weave-shadow");
    let propose = repo_root().join("scripts/weave-propose");
    let (tmp, dir, home, _) =
        weave_repo_with(&[("scripts/weave-shadow", &shadow), ("scripts/weave-propose", &propose)]);
    let approval = owner_approval(&dir);
    bk(&dir, &["id", "new", "alice"]);
    bk(&dir, &["id", "new", "bob"]);
    bk(&dir, &["id", "new", "manager"]);
    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    let script = |name: &str, args: &[&str]| -> Output {
        let mut c = command(&dir, name, args);
        c.env("HOME", &home).output().unwrap()
    };
    let ok = |name: &str, args: &[&str]| -> String {
        let out = script(name, args);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{name} {args:?}: {}{stdout}", String::from_utf8_lossy(&out.stderr));
        stdout
    };
    let refused = |name: &str, args: &[&str], expected: &str| {
        let out = script(name, args);
        let (stderr, stdout) = (String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
        assert_eq!(out.status.code(), Some(1), "{name} {args:?}: {stderr}{stdout}");
        assert!(stderr.contains(expected), "{name} {args:?}: expected {expected:?} in {stderr}{stdout}");
    };
    ok("scripts/weave-shadow", &["--init", "--owner-decision", &approval]);
    let git = |args: &[&str]| git_out(&dir, args);
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
    let commit = |msg: &str| {
        git(&["add", "-A"]);
        git(&["commit", "-qm", msg]);
        git(&["rev-parse", "HEAD"])
    };
    let push = |name: &str| git(&["update-ref", &format!("refs/remotes/origin/{name}"), "HEAD"]);
    // A builder's branch from main, committed and pushed (its remote-tracking ref set).
    let branch = |name: &str, change: &dyn Fn()| {
        git(&["switch", "-q", "-c", name, "main"]);
        change();
        commit(name);
        push(name);
    };
    // The builder's pr claim for the pushed head, and with `proposed`, its proposal first.
    let claimed = std::cell::RefCell::new(std::collections::HashMap::<String, (String, String)>::new());
    let claim = |pr: &str| {
        let (name, sha) = (git(&["symbolic-ref", "--short", "HEAD"]), git(&["rev-parse", "HEAD"]));
        let id = bk(&dir, &["claim", &format!("PR #{pr} {name} @ {sha}: {name}"), "--tag", "pr", "--as", "alice"]);
        claimed.borrow_mut().insert(pr.to_owned(), (id, sha));
    };
    // scripts/merge's pass on the PR's last pr claim: what the shadow checks a proposal against.
    let gate = |pr: &str| {
        let (id, sha) = claimed.borrow()[pr].clone();
        bk(&dir, &["evidence", &id, "pass", "--method", &format!("merge gate @ {sha}"), "--as", "manager"]);
    };
    let proposal = |pr: &str| {
        let out = ok("scripts/weave-propose", &["--pr", pr, "--as", "alice"]);
        claim(pr);
        out.lines().find_map(|l| l.strip_prefix("proposal ")).expect("a proposal").to_owned()
    };
    let origin_main = |sha: &str| git(&["update-ref", "refs/remotes/origin/main", sha]);
    // git's squash merge of a branch straight onto main, now origin/main.
    let squash = |name: &str, pr: &str| {
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--squash", name]);
        let sha = commit(&format!("{name} (#{pr})"));
        origin_main(&sha);
        sha
    };
    // As main has it: the forge squashes each branch in turn onto main's tip, a checkpoint commit lands on
    // main meanwhile (only .bracken/ changes), and the local merge of the two is the new main.
    let index = tmp.path().join("scratch-index");
    let batch = |prs: &[(&str, &str)]| {
        git(&["switch", "-q", "main"]);
        let tip = git(&["rev-parse", "HEAD"]);
        let mut forge = tip.clone();
        for (name, pr) in prs {
            let tree = git(&["merge-tree", "--write-tree", &forge, name]);
            forge = git(&["commit-tree", &tree, "-p", &forge, "-m", &format!("{name} (#{pr})")]);
        }
        let scratch = |args: &[&str]| {
            let out = command(&dir, "git", args).env("GIT_INDEX_FILE", &index).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        scratch(&["read-tree", &tip]);
        let blob = git(&["hash-object", "-w", "--stdin", "--path", "x"]);
        scratch(&["update-index", "--add", "--cacheinfo", &format!("100644,{blob},.bracken/checkpoint")]);
        let checkpoint = git(&["commit-tree", &scratch(&["write-tree"]), "-p", &tip, "-m", "ground: checkpoint"]);
        let tree = git(&["merge-tree", "--write-tree", &checkpoint, &forge]);
        let merge = git(&[
            "commit-tree",
            &tree,
            "-p",
            &checkpoint,
            "-p",
            &forge,
            "-m",
            "Merge remote-tracking branch 'origin/main'",
        ]);
        git(&["reset", "-q", "--hard", &merge]);
        origin_main(&merge);
        merge
    };
    let shadowed = |args: &[&str], outcome: &str| -> String {
        let out = ok("scripts/weave-shadow", args);
        assert!(out.contains(&format!("\noutcome {outcome}\n")), "{args:?}: {out}");
        assert!(out.contains("verified "), "{out}");
        out
    };
    let count = || {
        let out = script("scripts/weave", &["log", "--json", "--limit", "0"]);
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["omitted"].as_u64().unwrap()
    };

    // The first commit, as stage 1 records it.
    write("x.txt", "one\ntwo\nthree\nfour\nfive\n");
    // Not UTF-8 (a Latin-1 é), so Weave merges it per file, where git merges its lines.
    let latin1 = |first: &[u8], last: &[u8]| {
        std::fs::write(dir.join("y.txt"), [first, b"\ntwo\ncaf\xe9\nfour\n", last, b"\n"].concat()).unwrap()
    };
    latin1(b"one", b"five");
    let c0 = commit("x");
    origin_main(&c0);
    shadowed(&[&c0], "first");

    // Only what was pushed is proposed, and only weave-integrator moves a line.
    git(&["switch", "-q", "-c", "unpushed"]);
    write("u.txt", "u\n");
    commit("u");
    refused("scripts/weave-propose", &["--pr", "9", "--as", "alice"], "push first");
    refused("scripts/weave", &["weave", "integrate", "--as", "alice"], "only --as weave-integrator");

    // PR #1: proposed, merged by git, then merged by Weave equal to it.
    branch("f1", &|| write("f1.txt", "f1\n"));
    let p1 = proposal("1");
    let m1 = squash("f1", "1");
    gate("1");
    let out = shadowed(&["--proposal", &p1, &m1], "merged");
    assert!(
        out.contains(
            "stage 2a so far: 1 merges, 1 by Weave's own merge; fallbacks: 0 no proposal, 0 conflict, 0 mismatch, 0 ancestor base"
        ),
        "{out}"
    );

    // PRs #2 and #3 both change x.txt and y.txt, in different lines: git merges both. Weave merges #3, and
    // on #2 merges x.txt line by line (task 7cbd0a0ce7d0) but not y.txt, which isn't UTF-8 text, so it
    // conflicts and falls back to the import, noted with the path.
    branch("f2", &|| {
        write("x.txt", "ONE\ntwo\nthree\nfour\nfive\n");
        latin1(b"ONE", b"five");
    });
    let p2 = proposal("2");
    branch("f3", &|| {
        write("x.txt", "one\ntwo\nthree\nfour\nFIVE\n");
        latin1(b"one", b"FIVE");
    });
    let p3 = proposal("3");
    let m3 = squash("f3", "3");
    // A proposal must be of the PR merged: #2's isn't #3's.
    gate("3");
    gate("2");
    refused("scripts/weave-shadow", &["--proposal", &p2, &m3], "which");
    shadowed(&["--proposal", &p3, &m3], "merged");
    let m2 = squash("f2", "2");
    shadowed(&["--proposal", &p2, &m2], "conflict");

    // PR #4's merge in git carries more than its proposal: it stops with nothing written, then --mismatched
    // imports it, noted.
    branch("f4", &|| write("f4.txt", "f4\n"));
    let p4 = proposal("4");
    let m4 = {
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--squash", "f4"]);
        write("extra.txt", "the manager's\n");
        let sha = commit("f4 (#4)");
        origin_main(&sha);
        sha
    };
    let before = count();
    gate("4");
    refused("scripts/weave-shadow", &["--proposal", &p4, &m4], "Stop and report it");
    assert_eq!(count(), before, "a mismatch wrote something");
    shadowed(&["--proposal", &p4, "--mismatched", &m4], "mismatch");

    // A merge without a proposal is imported, noted.
    write("direct.txt", "d\n");
    let m5 = commit("direct");
    origin_main(&m5);
    let out = shadowed(&[&m5], "none");
    assert!(
        out.contains(
            "stage 2a so far: 5 merges, 2 by Weave's own merge; fallbacks: 1 no proposal, 1 conflict, 1 mismatch, 0 ancestor base"
        ),
        "{out}"
    );

    // A branch from a commit the shadow hasn't recorded is proposed against its nearest recorded first-parent
    // ancestor. Its merge waits for the commit before it: one merge at a time, in order.
    write("m6.txt", "6\n");
    let m6 = commit("m6");
    origin_main(&m6);
    branch("f7", &|| write("f7.txt", "7\n"));
    let p7 = proposal("7");
    let m7 = squash("f7", "7");
    gate("7");
    refused("scripts/weave-shadow", &["--proposal", &p7, &m7], "shadow the merges in order");
    shadowed(&[&m6], "none");
    shadowed(&["--proposal", &p7, &m7], "merged");

    // A batch: the local merge, after a checkpoint, of two squashes. Their proposals, given in any order, are
    // merged in squash order into one decision.
    branch("f8", &|| write("f8.txt", "8\n"));
    let p8 = proposal("8");
    branch("f9", &|| write("f9.txt", "9\n"));
    let p9 = proposal("9");
    let b1 = batch(&[("f8", "8"), ("f9", "9")]);
    gate("8");
    gate("9");
    shadowed(&["--proposal", &p9, "--proposal", &p8, &b1], "merged");

    // Another batch, where #10 has no proposal and #11's is of an earlier push than its pr claim names. That
    // proposal is refused; with none for #10, the whole merge is imported, noted with which PR.
    branch("f10", &|| write("f10.txt", "10\n"));
    claim("10");
    branch("f11", &|| write("f11.txt", "11\n"));
    let stale = proposal("11");
    write("f11.txt", "11, again\n");
    commit("f11 again");
    push("f11");
    claim("11");
    let b2 = batch(&[("f10", "10"), ("f11", "11")]);
    gate("11");
    refused("scripts/weave-shadow", &["--proposal", &stale, &b2], "propose the merged head");
    refused("scripts/weave-shadow", &["--proposal", &p8, &b2], "which");
    git(&["switch", "-q", "f11"]);
    let p11 = proposal("11");
    let out = shadowed(&["--proposal", &p11, &b2], "none");
    assert!(
        out.contains(
            "stage 2a so far: 9 merges, 4 by Weave's own merge; fallbacks: 3 no proposal, 1 conflict, 1 mismatch, 0 ancestor base"
        ),
        "{out}"
    );

    // PR #13 forks from #12's squash before the shadow records it, so its base is an ancestor's and it
    // carries #12's g.txt. #14 deletes g.txt before #13 merges: Weave's merge brings it back, git's doesn't.
    // That isn't Weave's merge going wrong: it's imported, noted as from an ancestor base, with no stop.
    branch("f12", &|| write("g.txt", "g\n"));
    claim("12");
    let s12 = squash("f12", "12");
    branch("f13", &|| write("h.txt", "h\n"));
    let out = ok("scripts/weave-propose", &["--pr", "13", "--as", "alice"]);
    assert!(out.contains(&format!(", an ancestor of the merge-base {s12},")), "{out}");
    let p13 = out.lines().find_map(|l| l.strip_prefix("proposal ")).expect("a proposal").to_owned();
    claim("13");
    gate("13");
    // Only the pr claim that merged counts: bob's newer claim for #13, of another head, doesn't.
    bk(&dir, &["claim", &format!("PR #13 f13 @ {s12}: not bob's"), "--tag", "pr", "--as", "bob"]);
    // Nor does a proposal of the merged head by anyone but that claim's author.
    let by_bob = ok("scripts/weave-propose", &["--pr", "13", "--as", "bob"]);
    let by_bob = by_bob.lines().find_map(|l| l.strip_prefix("proposal ")).expect("a proposal").to_owned();
    git(&["switch", "-q", "main"]);
    std::fs::remove_file(dir.join("g.txt")).unwrap();
    let s14 = commit("undo g (#14)");
    origin_main(&s14);
    let s13 = squash("f13", "13");
    shadowed(&[&s12], "none");
    shadowed(&[&s14], "none");
    refused("scripts/weave-shadow", &["--proposal", &by_bob, &s13], "is by bob");
    let out = shadowed(&["--proposal", &p13, &s13], "ancestor");
    assert!(
        out.contains(
            "stage 2a so far: 12 merges, 4 by Weave's own merge; fallbacks: 5 no proposal, 1 conflict, 1 mismatch, 1 ancestor base"
        ),
        "{out}"
    );

    // An ancestor base excuses only exactly that: #15's squash adds k.txt, #17 forks from it unrecorded, and
    // git's merge of #17 changes k.txt again where Weave's doesn't. k.txt is a path the unrecorded commit
    // changed, but Weave's merge isn't bringing anything back there, so it stops like any wrong merge.
    branch("f15", &|| write("k.txt", "k\n"));
    claim("15");
    let s15 = squash("f15", "15");
    branch("f17", &|| write("l.txt", "l\n"));
    let p17 = proposal("17");
    let s17 = {
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--squash", "f17"]);
        write("k.txt", "k, again\n");
        let sha = commit("f17 (#17)");
        origin_main(&sha);
        sha
    };
    shadowed(&[&s15], "none");
    gate("17");
    let before = count();
    refused("scripts/weave-shadow", &["--proposal", &p17, &s17], "Stop and report it");
    assert_eq!(count(), before, "a mismatch wrote something");
    let out = shadowed(&["--proposal", &p17, "--mismatched", &s17], "mismatch");
    assert!(
        out.contains(
            "stage 2a so far: 14 merges, 4 by Weave's own merge; fallbacks: 6 no proposal, 1 conflict, 2 mismatch, 1 ancestor base"
        ),
        "{out}"
    );

    // Batches with an ancestor base (task 0a58850fa794, finding 4dd7841ae9c7). #20's squash changes p.txt,
    // #22 and #23 fork from it unrecorded, so both carry its change, and #21 undoes it before they merge.
    // #28, from after the undo, leaves p.txt alone. In one batch Weave's merge brings p.txt back, git's
    // doesn't, and no other member changed p.txt but to bring the same back: an ancestor base, no stop.
    write("p.txt", "v0\n");
    let s18 = commit("p");
    origin_main(&s18);
    shadowed(&[&s18], "none");
    branch("f20", &|| write("p.txt", "v1\n"));
    claim("20");
    let s20 = squash("f20", "20");
    branch("f22", &|| write("a22.txt", "22\n"));
    let p22 = proposal("22");
    branch("f23", &|| write("a23.txt", "23\n"));
    let p23 = proposal("23");
    git(&["switch", "-q", "main"]);
    write("p.txt", "v0\n");
    let s21 = commit("undo p (#21)");
    origin_main(&s21);
    shadowed(&[&s20], "none");
    shadowed(&[&s21], "none");
    branch("f28", &|| write("a28.txt", "28\n"));
    let p28 = proposal("28");
    let b3 = batch(&[("f22", "22"), ("f23", "23"), ("f28", "28")]);
    for pr in ["22", "23", "28"] {
        gate(pr);
    }
    shadowed(&["--proposal", &p22, "--proposal", &p23, "--proposal", &p28, &b3], "ancestor");

    // The same again, but #27, from after the undo, edits p.txt, and git's merge takes #27's edit. A Weave
    // that dropped #27's edit would give the tree that bringing #25's p.txt back gives, so the shadow
    // mustn't call that an ancestor base: it stops. A stand-in scripts/weave plays that Weave for one run;
    // the real one conflicts at p.txt.
    branch("f24", &|| write("p.txt", "v1\n"));
    claim("24");
    let s24 = squash("f24", "24");
    branch("f25", &|| write("a25.txt", "25\n"));
    let p25 = proposal("25");
    git(&["switch", "-q", "main"]);
    write("p.txt", "v0\n");
    let s26 = commit("undo p again (#26)");
    origin_main(&s26);
    shadowed(&[&s24], "none");
    shadowed(&[&s26], "none");
    branch("f27", &|| write("p.txt", "X\n"));
    let p27 = proposal("27");
    let b4 = batch(&[("f25", "25"), ("f27", "27")]);
    gate("25");
    // Only a pass is the merge gate's: a failed 'merge gate' doesn't count.
    let (id27, sha27) = claimed.borrow()["27"].clone();
    bk(&dir, &["evidence", &id27, "fail", "--method", &format!("merge gate @ {sha27}"), "--as", "manager"]);
    refused(
        "scripts/weave-shadow",
        &["--proposal", &p25, "--proposal", &p27, &b4],
        "carries the manager's merge gate pass",
    );
    gate("27");
    // A stand-in scripts/weave whose `integrate --proposals` says the merge differs from git's `sha` only at
    // `path`, with the manifest of `sha`'s tree with `path` as commit `from` has it: a Weave that took
    // `from`'s side there. The shadow runs with it once, where it must stop and write nothing.
    let stand_in = |sha: &str, path: &str, from: &str, args: &[&str]| {
        let index = tmp.path().join("stand-in-index");
        let scratch = |args: &[&str]| {
            let out = command(&dir, "git", args).env("GIT_INDEX_FILE", &index).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        scratch(&["read-tree", sha]);
        let blob = git(&["rev-parse", &format!("{from}:{path}")]);
        scratch(&["update-index", "--cacheinfo", &format!("100644,{blob},{path}")]);
        let c = git(&["commit-tree", &scratch(&["write-tree"]), "-p", sha, "-m", "a stand-in's merge"]);
        let import = ok(
            "scripts/weave",
            &["weave", "import", "--from-git", &c, "--repo", dir.to_str().unwrap(), "--dry-run", "--json"],
        );
        let manifest = serde_json::from_str::<serde_json::Value>(&import).unwrap()["manifest"].clone();
        let real = dir.join("scripts/weave.real");
        std::fs::rename(dir.join("scripts/weave"), &real).unwrap();
        std::fs::write(
            dir.join("scripts/weave.out"),
            serde_json::json!({"differs": [{"path": path, "change": "modified"}], "manifest": manifest}).to_string(),
        )
        .unwrap();
        write_executable(
            &dir.join("scripts/weave"),
            "#!/usr/bin/env bash\nhere=$(dirname \"$0\")\n\
             case \" $* \" in *' weave integrate --proposals '*) cat \"$here/weave.out\"; exit 1 ;; esac\n\
             exec \"$here/weave.real\" \"$@\"\n",
        );
        let before = count();
        refused("scripts/weave-shadow", args, "Stop and report it");
        assert_eq!(count(), before, "a mismatch wrote something");
        std::fs::rename(&real, dir.join("scripts/weave")).unwrap();
        std::fs::remove_file(dir.join("scripts/weave.out")).unwrap();
    };
    stand_in(&b4, "p.txt", &s24, &["--proposal", &p25, "--proposal", &p27, &b4]);
    let out = shadowed(&["--proposal", &p25, "--proposal", &p27, &b4], "conflict");
    assert!(
        out.contains(
            "stage 2a so far: 21 merges, 4 by Weave's own merge; fallbacks: 11 no proposal, 2 conflict, 2 mismatch, 2 ancestor base"
        ),
        "{out}"
    );

    // Another member brings the path back only if it brings the same entry (task 35ba3809bede, finding
    // d84291cfaacf). #50's squash changes r.txt's first line; #51 and #52 fork from it unrecorded, and #52
    // also changes its last line; #53 undoes #50. A Weave that kept #51's r.txt, dropping #52's edit, gives
    // the tree #51's bringing back gives, so it must stop.
    write("r.txt", "1\n2\n3\n4\n5\n");
    let s49 = commit("r");
    origin_main(&s49);
    shadowed(&[&s49], "none");
    branch("f50", &|| write("r.txt", "one\n2\n3\n4\n5\n"));
    claim("50");
    let s50 = squash("f50", "50");
    branch("f51", &|| write("a51.txt", "51\n"));
    let p51 = proposal("51");
    branch("f52", &|| write("r.txt", "one\n2\n3\n4\nfive\n"));
    let p52 = proposal("52");
    git(&["switch", "-q", "main"]);
    write("r.txt", "1\n2\n3\n4\n5\n");
    let s53 = commit("undo r (#53)");
    origin_main(&s53);
    shadowed(&[&s50], "none");
    shadowed(&[&s53], "none");
    let b6 = batch(&[("f51", "51"), ("f52", "52")]);
    gate("51");
    gate("52");
    let args = ["--proposal", &p51, "--proposal", &p52, &b6];
    stand_in(&b6, "r.txt", &s50, &args);
    // The real merge keeps both edits, where git's keeps #52's only: a mismatch.
    refused("scripts/weave-shadow", &args, "Stop and report it");
    shadowed(&[&args[..], &["--mismatched"]].concat(), "mismatch");

    // And only where line main's head still has the path as that member's base had it. #41 forks unrecorded
    // from #40's squash, which changes q.txt, so its base has q0; main then sets q.txt to u (#42), #43 sets
    // it to #40's q1 again, and #44 forks from that unrecorded, so its base has u; #45 undoes #43. Both
    // bring q1 back, but main's head has u, not #41's base's q0, so #41 doesn't merely bring it back. A Weave
    // that took q1 there must stop; the real one conflicts, main's u against #41's q1.
    write("q.txt", "q0\n");
    let s39 = commit("q");
    origin_main(&s39);
    shadowed(&[&s39], "none");
    branch("f40", &|| write("q.txt", "q1\n"));
    claim("40");
    let s40 = squash("f40", "40");
    branch("f41", &|| write("a41.txt", "41\n"));
    let p41 = proposal("41");
    git(&["switch", "-q", "main"]);
    write("q.txt", "u\n");
    let s42 = commit("q u (#42)");
    origin_main(&s42);
    shadowed(&[&s40], "none");
    shadowed(&[&s42], "none");
    branch("f43", &|| write("q.txt", "q1\n"));
    claim("43");
    let s43 = squash("f43", "43");
    branch("f44", &|| write("a44.txt", "44\n"));
    let p44 = proposal("44");
    git(&["switch", "-q", "main"]);
    write("q.txt", "u\n");
    let s45 = commit("undo q (#45)");
    origin_main(&s45);
    shadowed(&[&s43], "none");
    shadowed(&[&s45], "none");
    let b5 = batch(&[("f41", "41"), ("f44", "44")]);
    gate("41");
    gate("44");
    let args = ["--proposal", &p41, "--proposal", &p44, &b5];
    stand_in(&b5, "q.txt", &s43, &args);
    let out = shadowed(&args, "conflict");
    assert!(
        out.contains(
            "stage 2a so far: 31 merges, 4 by Weave's own merge; fallbacks: 19 no proposal, 3 conflict, 3 mismatch, 2 ancestor base"
        ),
        "{out}"
    );

    let log =
        serde_json::from_str::<serde_json::Value>(&ok("scripts/weave", &["weave", "log", "--limit", "100", "--json"]))
            .unwrap();
    let notes: Vec<&str> = log["decisions"].as_array().unwrap().iter().map(|d| d["note"].as_str().unwrap()).collect();
    assert_eq!(
        notes,
        [
            format!("shadow of {b5}: proposals {p41} {p44} conflicted at q.txt"),
            format!("shadow of {s45}: no proposal for PR #45"),
            format!("shadow of {s43}: no proposal for PR #43"),
            format!("shadow of {s42}: no proposal for PR #42"),
            format!("shadow of {s40}: no proposal for PR #40"),
            format!("shadow of {s39}: no proposal"),
            format!("shadow of {b6}: proposals {p51} {p52} mismatched at r.txt"),
            format!("shadow of {s53}: no proposal for PR #53"),
            format!("shadow of {s50}: no proposal for PR #50"),
            format!("shadow of {s49}: no proposal"),
            format!("shadow of {b4}: proposals {p25} {p27} conflicted at p.txt"),
            format!("shadow of {s26}: no proposal for PR #26"),
            format!("shadow of {s24}: no proposal for PR #24"),
            format!("shadow of {b3}: proposals {p22} {p23} {p28} differed at p.txt, from an ancestor base"),
            format!("shadow of {s21}: no proposal for PR #21"),
            format!("shadow of {s20}: no proposal for PR #20"),
            format!("shadow of {s18}: no proposal"),
            format!("shadow of {s17}: proposal {p17} mismatched at k.txt"),
            format!("shadow of {s15}: no proposal for PR #15"),
            format!("shadow of {s13}: proposal {p13} differed at g.txt, from an ancestor base"),
            format!("shadow of {s14}: no proposal for PR #14"),
            format!("shadow of {s12}: no proposal for PR #12"),
            format!("shadow of {b2}: no proposal for PR #10"),
            format!("shadow of {b1}: merged proposals {p8} {p9}"),
            format!("shadow of {m7}: merged proposal {p7}"),
            format!("shadow of {m6}: no proposal"),
            format!("shadow of {m5}: no proposal"),
            format!("shadow of {m4}: proposal {p4} mismatched at extra.txt"),
            format!("shadow of {m2}: proposal {p2} conflicted at y.txt"),
            format!("shadow of {m3}: merged proposal {p3}"),
            format!("shadow of {m1}: merged proposal {p1}"),
            format!("shadow of {c0}"),
        ]
    );
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

/// Task 5b81f744a0ea: scripts/weave-backup copies the Weave Ground, identities included, to
/// <dest>/<UTC time>/, at mode 0700. It does so only with the owner's approval naming <dest>'s real path,
/// and only once the pinned binary opens the copy with the Ground's contributions, heads and identities.
/// Writes in flight (.tmp-*) are left out, and a pass that misses an ancestor while the Ground is written
/// to meanwhile is copied again. It keeps the newest --keep copies, the new one among them. It deletes only
/// older directories holding a copy of this Ground, and stops at a name dated after now. It refuses a
/// destination inside a git checkout (through a symlinked parent too) or ~/bracken-weave, one holding the
/// Ground, a symlink, a file, one not at 0700 or with an ACL, and one under a directory others can write
/// to. It prints counts only. --verify opens a copy and refuses an incomplete one and one of another
/// Ground. Never the real ~/bracken-weave: HOME is under $TMPDIR. Findings a9ad923091bf, 85c08b7f8aea,
/// 9c4a64c905c3 and 661f68c123b7.
#[test]
fn weave_backup_copies_the_weave_ground_whole_and_keeps_the_newest() {
    use std::os::unix::fs::PermissionsExt;
    let backup = repo_root().join("scripts/weave-backup");
    let (tmp, dir, home, _) = weave_repo_with(&[("scripts/weave-backup", &backup)]);
    let real_path = std::env::var("PATH").unwrap();
    let shim = tmp.path().join("shim");
    let run_with = |path: &str, args: &[&str]| -> Output {
        let mut c = command(&dir, "scripts/weave-backup", args);
        c.env("HOME", &home).env("PATH", path).env("REAL_PATH", &real_path).env("T", tmp.path());
        c.output().unwrap()
    };
    let run_backup = |args: &[&str]| run_with(&real_path, args);
    let refused = |args: &[&str], expected: &str| {
        let out = run_backup(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(expected), "{args:?}: expected {expected:?} in {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}");
    };
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
    let private_dir = |p: &Path| {
        std::fs::create_dir(p).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    // A path as the script names it: its parent's real path, then its name ($TMPDIR may be a symlink).
    let real = |p: &Path| std::fs::canonicalize(p.parent().unwrap()).unwrap().join(p.file_name().unwrap());
    let s = |p: &Path| p.to_str().unwrap().to_owned();

    // `a` is under a sticky directory others can write to, which is allowed (finding 99b7ce082502), and on
    // macOS one with an ACL that only denies.
    let sticky = tmp.path().join("sticky");
    std::fs::create_dir(&sticky).unwrap();
    std::fs::set_permissions(&sticky, std::fs::Permissions::from_mode(0o1777)).unwrap();
    let acl_parent = tmp.path().join("acl-parent");
    std::fs::create_dir(&acl_parent).unwrap();
    let a = sticky.join("a");
    let [b, open, file, acl, fut] = ["b", "open", "file", "acl", "fut"].map(|n| tmp.path().join(n));
    let approval = |text: &str| bk(&dir, &["claim", text, "--tag", "approval", "--as", "owner"]);
    bk(&dir, &["id", "new", "owner"]);
    let go = approval(&format!(
        "Approval (owner): back up the Weave Ground to `{}`, {}, {}, {}/ {} {} and {}.",
        s(&real(&a)),
        s(&real(&acl_parent.join("bk"))),
        s(&real(&b)),
        s(&real(&open)),
        s(&real(&file)),
        s(&real(&acl)),
        s(&real(&fut))
    ));
    let a_old = approval(&format!("Approval (owner): back up to {}-old", s(&real(&a))));
    let replaced = approval(&format!("Approval (owner): back up to {}", s(&real(&a))));
    bk(
        &dir,
        &[
            "claim",
            "Approval (owner): not there after all",
            "--tag",
            "approval",
            "--supersedes",
            &replaced,
            "--as",
            "owner",
        ],
    );
    bk(&dir, &["id", "new", "manager"]);
    let not_owner =
        bk(&dir, &["claim", &format!("Approval: {}", s(&real(&a))), "--tag", "approval", "--as", "manager"]);
    let with = |dest: &Path| [s(dest)];
    let backup_to = |dest: &Path, more: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = vec!["--owner-decision".into(), go.clone()];
        args.extend(more.iter().map(|m| m.to_string()));
        args.extend(with(dest));
        args
    };
    let refused_to = |dest: &Path, expected: &str| {
        let args = backup_to(dest, &[]);
        refused(&args.iter().map(String::as_str).collect::<Vec<_>>(), expected);
    };
    refused_to(&a, "no Weave Ground at");
    assert!(!a.exists());

    std::fs::create_dir(home.join("bracken-weave")).unwrap();
    let ok = |args: &[&str]| -> String {
        let out = weave(&dir, "scripts/weave", &home, None, args);
        assert!(out.status.success(), "weave {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let init = ok(&["weave", "init", "--as", "weave-integrator"]);
    let marker = init.lines().find_map(|l| l.strip_prefix("marker ")).expect("a marker").to_owned();
    ok(&["claim", "hello", "--as", "weave-integrator"]);
    let ground = home.join("bracken-weave/manyroot");
    // A write in flight, which the copy leaves out.
    let shard = ground.join("objects").join(&marker[..2]);
    std::fs::write(shard.join(".tmp-1-inflight"), b"half").unwrap();

    // Refused before anything is created.
    let symlink = tmp.path().join("link");
    std::os::unix::fs::symlink(tmp.path(), &symlink).unwrap();
    let into_checkout = tmp.path().join("into-checkout");
    std::os::unix::fs::symlink(&dir, &into_checkout).unwrap();
    let shared = tmp.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&file, b"").unwrap();
    refused_to(&dir.join("backups"), "inside the git checkout");
    refused_to(&into_checkout.join("backups"), "inside the git checkout");
    refused_to(&home.join("bracken-weave/backups"), "back up the Weave Ground somewhere else");
    refused_to(&ground.join("objects"), "back up the Weave Ground somewhere else");
    refused_to(&home, "holds the Weave Ground");
    refused_to(&symlink, "is a symlink");
    refused_to(&shared.join("bk"), "is writable by group or others and not sticky");
    refused(&[&s(&a)], "a backup needs --owner-decision");
    refused(&["--owner-decision", "TODO", &s(&a)], "12 to 64 lowercase hex");
    refused(&["--owner-decision", "0123456789ab", &s(&a)], "has no such contribution");
    refused(&["--owner-decision", &not_owner, &s(&a)], "isn't a claim tagged approval, signed by owner");
    refused(&["--owner-decision", &a_old, &s(&a)], "the approval doesn't name");
    refused(&["--owner-decision", &replaced, &s(&a)], "not retracted and not superseded");
    refused(&["--owner-decision", &go, &s(&tmp.path().join("elsewhere"))], "the approval doesn't name");
    refused_to(&file, "isn't a directory");
    refused_to(&open, "must be 0700");
    refused(&["--owner-decision", &go, "--keep", "0", &s(&a)], "give a whole number");
    if cfg!(target_os = "macos") {
        let chmod_a = |entry: &str, p: &Path| {
            assert!(Command::new("chmod").args(["+a", entry]).arg(p).status().unwrap().success());
        };
        chmod_a("everyone deny delete", &sticky);
        chmod_a("everyone allow add_file", &acl_parent);
        refused_to(&acl_parent.join("bk"), "has an ACL that allows something");
        assert!(!acl_parent.join("bk").exists());
        private_dir(&acl);
        let chmod = Command::new("chmod").args(["+a", "everyone allow list"]).arg(&acl).status().unwrap();
        assert!(chmod.success());
        refused_to(&acl, "has an ACL");
        assert_eq!(std::fs::read_dir(&acl).unwrap().count(), 0);
    }
    private_dir(&fut);
    std::fs::write(fut.join("99990101T000000Z"), b"").unwrap();
    refused_to(&fut, "is named for a time after now");
    assert_eq!(std::fs::read_dir(&fut).unwrap().count(), 1, "a refusal wrote into {fut:?}");
    assert!(!a.exists() && !dir.join("backups").exists() && !shared.join("bk").exists(), "a refusal created something");
    assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0, "a refusal wrote into {open:?}");

    // A first copy, into a directory it creates at 0700: identities included, byte for byte, the write in
    // flight left out, and counts only printed.
    let args = backup_to(&a, &[]);
    let out = run_backup(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(mode(&a), 0o700);
    let copies: Vec<PathBuf> = std::fs::read_dir(&a).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    let copy = &copies[0];
    assert_eq!(stdout, format!("backup {}: 3 contributions, 1 heads, 1 identities\n", s(&real(copy))));
    assert_eq!(mode(copy), 0o700);
    let key = |g: &Path| std::fs::read(g.join("identities/weave-integrator.json")).unwrap();
    assert_eq!(key(copy), key(&ground));
    assert_eq!(mode(&copy.join("identities/weave-integrator.json")), 0o600);
    assert!(copy.join("objects").join(&marker[..2]).join(&marker[2..]).exists());
    assert!(!copy.join("objects").join(&marker[..2]).join(".tmp-1-inflight").exists(), "a write in flight was copied");
    let out = run_backup(&["--verify", &s(copy)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("verified {}: 3 contributions, 1 heads, 1 identities\n", s(copy))
    );

    // --verify refuses a copy missing a contribution others build on, and a copy of another Ground.
    let broken = tmp.path().join("broken");
    assert!(Command::new("cp").arg("-Rp").arg(copy).arg(&broken).status().unwrap().success());
    std::fs::remove_file(broken.join("objects").join(&marker[..2]).join(&marker[2..])).unwrap();
    refused(&["--verify", &s(&broken)], "isn't a complete copy");
    let other = tmp.path().join("other");
    let out = Command::new(env!("CARGO_BIN_EXE_bracken")).arg("init").env("BRACKEN_DIR", &other).output().unwrap();
    assert!(out.status.success());
    refused(&["--verify", &s(&other)], "a copy of another Ground");

    // The first pass misses the marker, as if its directory was copied just before it was written, while
    // someone writes to the Ground: the copy is made again, and holds the new contribution too. Then only the
    // two oldest copies of this Ground go: never a directory that isn't one, a file or a symlink.
    std::fs::create_dir(&shim).unwrap();
    write_executable(
        &shim.join("tar"),
        &format!(
            "#!/bin/sh\nPATH=$REAL_PATH tar \"$@\" || exit\ncase \" $* \" in *\" -xpf \"*) ;; *) exit 0 ;; esac\n\
             [ ! -e \"$T/tar.once\" ] || exit 0\ntouch \"$T/tar.once\"\n\
             rm \"$2/objects/{}/{}\"\nHOME={home:?} PATH=$REAL_PATH {weave:?} claim racing --as weave-integrator >/dev/null\n",
            &marker[..2],
            &marker[2..],
            home = home,
            weave = dir.join("scripts/weave"),
        ),
    );
    private_dir(&b);
    for name in ["20200101T000000Z", "20200102T000000Z", "20200103T000000Z"] {
        assert!(Command::new("cp").arg("-Rp").arg(copy).arg(b.join(name)).status().unwrap().success());
    }
    std::fs::create_dir(b.join("20200101T120000Z")).unwrap();
    std::fs::create_dir(b.join("notes")).unwrap();
    std::fs::write(b.join("20200104T000000Z"), b"").unwrap();
    std::os::unix::fs::symlink(copy, b.join("20200105T000000Z")).unwrap();
    let args = backup_to(&b, &["--keep", "2"]);
    let out =
        run_with(&format!("{}:{real_path}", shim.display()), &args.iter().map(String::as_str).collect::<Vec<_>>());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(tmp.path().join("tar.once").exists(), "the shim ran");
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(": 4 contributions, 1 heads, 1 identities\n"), "{stdout}");
    assert!(stdout.ends_with("deleted 20200101T000000Z\ndeleted 20200102T000000Z\n"), "{stdout}");
    let mut left: Vec<String> =
        std::fs::read_dir(&b).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    let kept = ["20200101T120000Z", "20200103T000000Z", "20200104T000000Z", "20200105T000000Z", "notes"];
    let new: Vec<&String> = left.iter().filter(|n| !kept.contains(&n.as_str())).collect();
    assert_eq!((left.len(), new.len()), (6, 1), "{left:?}");
    let new = new[0].clone();
    assert!(copy.exists(), "the symlink's target is kept");
    let out = run_backup(&["--verify", &s(&b.join(&new))]);
    assert!(String::from_utf8_lossy(&out.stdout).contains(": 4 contributions, 1 heads, 1 identities"));
}
