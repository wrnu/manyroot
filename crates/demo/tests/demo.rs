//! Runs small demos through the real binaries and checks the report's invariants.
//!
//! The demo drives the `bracken` binary that cargo builds next to it, so these
//! tests need the bracken package built (`cargo test --workspace` does that).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn demo(args: &[&str]) -> Output {
    demo_command(args).output().unwrap()
}

/// The demo with a clean environment for bracken and git.
fn demo_command(args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_bracken-demo"));
    c.args(args);
    for var in ["BRACKEN_DIR", "BRACKEN_AGENT", "GIT_DIR", "GIT_WORK_TREE"] {
        c.env_remove(var);
    }
    c
}

/// Every file under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { out.extend(files(&path)) } else { out.push(path) }
    }
    out
}

fn report(args: &[&str]) -> Value {
    let out = demo(&[args, &["--json"]].concat());
    assert!(out.status.success(), "bracken-demo {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn bracken_bin() -> PathBuf {
    let demo = PathBuf::from(env!("CARGO_BIN_EXE_bracken-demo"));
    demo.with_file_name(format!("bracken{}", std::env::consts::EXE_SUFFIX))
}

fn bracken_json(replica: &Path, args: &[&str]) -> Value {
    let out = Command::new(bracken_bin())
        .arg("--dir")
        .arg(replica)
        .args(args)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap();
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn without_timing(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("timing");
    v
}

#[test]
fn a_small_demo_converges_and_its_report_adds_up() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = tmp.path().join("run");
    let args = ["--agents", "3", "--rounds", "3", "--seed", "5", "--topology", "ring"];
    let r = report(&[&args[..], &["--keep", kept.to_str().unwrap()]].concat());

    // The kept directory holds private keys, so git ignores all of it.
    assert_eq!(std::fs::read_to_string(kept.join(".gitignore")).unwrap(), "*\n");

    // Every replica ends with the same contributions, in the same causal order.
    assert_eq!(r["replicas"]["converged"], true);
    let logs: Vec<Value> =
        (0..3).map(|i| bracken_json(&kept.join(format!("agent-{i}/.bracken")), &["log", "--json"])).collect();
    let ids = |log: &Value| -> Vec<String> {
        log.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect()
    };
    assert!(logs.windows(2).all(|w| ids(&w[0]) == ids(&w[1])));
    for n in r["replicas"]["contributions"].as_array().unwrap() {
        assert_eq!(n.as_u64().unwrap() as usize, ids(&logs[0]).len());
    }

    // Every accepted subject has passes from enough agents other than its author, and no counted fail.
    let log = logs[0].as_array().unwrap();
    let author: HashMap<&str, &str> =
        log.iter().map(|c| (c["id"].as_str().unwrap(), c["author"].as_str().unwrap())).collect();
    let replica = kept.join("agent-0/.bracken");
    for policy in r["policies"].as_array().unwrap() {
        let k = policy["min_passes"].as_u64().unwrap() as usize;
        let accepted = bracken_json(&replica, &["accepted", "--min-passes", &k.to_string(), "--json"]);
        let accepted = accepted["accepted"].as_array().unwrap();
        assert_eq!(policy["accepted"].as_u64().unwrap() as usize, accepted.len());
        for a in accepted {
            let id = a["id"].as_str().unwrap();
            let counted = |verdict: &str| -> BTreeSet<&str> {
                log.iter()
                    .filter(|c| c["kind"] == "evidence" && c["retracted"] == false)
                    .filter(|c| c["body"]["target"] == id && c["body"]["verdict"] == verdict)
                    .map(|c| c["author"].as_str().unwrap())
                    .filter(|who| *who != author[id])
                    .collect()
            };
            assert!(counted("pass").len() >= k, "{id} accepted at min passes {k} with {:?}", counted("pass"));
            assert!(counted("fail").is_empty(), "{id} accepted despite a counted fail");
        }
        let p = |f: &str| policy[f].as_u64().unwrap();
        assert_eq!(p("correct") + p("wrong"), p("implementations"));
        assert!(p("specs_solved") <= r["config"]["specs"].as_u64().unwrap());
    }

    // The counts are consistent with each other.
    let n = |path: &[&str]| path.iter().fold(&r, |v, k| &v[*k]).as_u64().unwrap();
    // The setter's: the task claim, and a claim and a refines relation per spec.
    let setter = 1 + 2 * n(&["config", "specs"]);
    assert_eq!(n(&["contributions", "total"]) + setter, ids(&logs[0]).len() as u64);
    assert_eq!(n(&["duplication", "blind"]) + n(&["duplication", "visible"]), n(&["duplication", "duplicates"]));
    assert_eq!(n(&["work", "passes"]) + n(&["work", "fails"]), n(&["work", "checks"]));
    assert_eq!(n(&["ground_truth", "correct"]) + n(&["ground_truth", "wrong"]), n(&["work", "implementations"]));
    assert_eq!(n(&["availability", "missing"]), 0);
    assert_eq!(n(&["availability", "pairs"]), n(&["contributions", "total"]) * 2, "each reaches the other 2 agents");
    let histogram: u64 =
        r["availability"]["histogram"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert_eq!(histogram, n(&["availability", "pairs"]));
    // In a ring of 3, a contribution needs at most 2 syncs to reach everyone.
    assert!(n(&["availability", "max_rounds"]) <= 2);
}

#[test]
fn the_same_seed_gives_the_same_report() {
    let args = ["--agents", "3", "--rounds", "2", "--seed", "11", "--topology", "gossip"];
    let first = without_timing(report(&args));
    assert_eq!(first, without_timing(report(&args)));
    let other = without_timing(report(&["--agents", "3", "--rounds", "2", "--seed", "12", "--topology", "gossip"]));
    assert_ne!(first, other, "a different seed should change the run");
}

#[test]
fn check_runs_a_program_against_a_spec() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("max2.rpn");
    std::fs::write(&file, "a b > a b ?").unwrap();
    let out = demo(&["check", "max2", file.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "pass 169/169 cases passed");

    std::fs::write(&file, "a b < a b ?").unwrap();
    let out = demo(&["check", "max2", file.to_str().unwrap(), "--json"]);
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["pass"], false);
    assert_eq!(v["total"], 169);
}

#[test]
fn keep_refuses_an_existing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let out = demo(&["--keep", tmp.path().to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already exists"));
}

#[test]
fn runs_refuse_a_directory_inside_a_git_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let kept = tmp.path().join("nested/run");
    let out = demo(&["--keep", kept.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the git checkout"));
    assert!(!kept.exists(), "nothing is written when refused");

    let args = ["--agents", "2", "--rounds", "1", "--allow-in-repo", "--keep", kept.to_str().unwrap()];
    assert_eq!(report(&args)["replicas"]["converged"], true);
    assert_eq!(std::fs::read_to_string(kept.join(".gitignore")).unwrap(), "*\n");
}

#[test]
fn a_bare_bracken_name_is_not_looked_up_on_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, cwd) = (tmp.path().join("bin"), tmp.path().join("cwd"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // A working bracken on PATH, which must not be picked up.
    let on_path = bin.join(format!("bracken{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(bracken_bin(), &on_path).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bracken-demo"))
        .args(["--agents", "2", "--rounds", "1", "--bracken", "bracken"])
        .current_dir(&cwd)
        .env("PATH", &bin)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap();
    assert!(!out.status.success(), "a bare name must not resolve through PATH");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no bracken binary at bracken"));

    // With a `bracken` in the current directory, that one runs, not the one on PATH.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake = cwd.join("bracken");
        std::fs::write(&fake, "#!/bin/sh\necho fake bracken >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_bracken-demo"))
            .args(["--agents", "2", "--rounds", "1", "--bracken", "bracken"])
            .current_dir(&cwd)
            .env("PATH", &bin)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("fake bracken"));
    }
}

#[test]
fn a_relative_keep_writes_only_inside_the_kept_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("a/b/c");
    std::fs::create_dir_all(&cwd).unwrap();
    let out =
        demo_command(&["--agents", "2", "--rounds", "1", "--keep", "../../kept"]).current_dir(&cwd).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let kept = tmp.path().join("a/kept");
    let all = files(tmp.path());
    assert!(all.iter().all(|f| f.starts_with(&kept)), "files outside {}: {all:?}", kept.display());
    assert!(kept.join(".gitignore").is_file());
    assert!(kept.join("setter/.bracken/identities").is_dir(), "the setter's replica is in the kept directory");
    assert!(kept.join("agent-1/.bracken/ground.json").is_file());
}

#[test]
fn runs_refuse_when_git_is_pointed_elsewhere() {
    let tmp = tempfile::tempdir().unwrap();
    let kept = tmp.path().join("run");
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        let out = demo_command(&["--keep", kept.to_str().unwrap()]).env(var, tmp.path()).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("{var} is set")));
        assert!(!kept.exists());
    }
}

#[test]
fn runs_refuse_a_directory_inside_a_real_git_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let init = Command::new("git").args(["init", "-q"]).current_dir(tmp.path()).env_remove("GIT_DIR").status();
    if !init.is_ok_and(|s| s.success()) {
        eprintln!("git isn't available; skipping");
        return;
    }
    let kept = tmp.path().join("deep/run");
    let out = demo(&["--keep", kept.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the git checkout"));
}
