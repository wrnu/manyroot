//! Typed conventions (task `8c9ad9bd4518`): `type:`, `key:value` and `ref:` tags, checked at write by
//! `bracken claim`, with references made parents, and reported, never refused, on read.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use manyroot::{Body, Store};
use serde_json::Value;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(args)
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .expect("bracken runs")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).expect("utf-8").trim().to_owned())
}

fn refused(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
    assert!(out.stdout.is_empty(), "a refused write printed {:?}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8(out.stderr).expect("utf-8")
}

fn json(dir: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&ok(dir, args)).expect("json")
}

fn ground() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    ok(tmp.path(), &["init"]);
    ok(tmp.path(), &["id", "new", "alice"]);
    tmp
}

fn count(dir: &Path) -> usize {
    json(dir, &["log", "--json"]).as_array().expect("a list").len()
}

#[test]
fn a_typed_write_records_its_tags_relations_and_references_as_parents_in_one_call() {
    let tmp = ground();
    let dir = tmp.path();
    let task = ok(dir, &["claim", "Do the thing", "--type", "task", "--attr", "agent=builder-x", "--as", "alice"]);
    // Something after the task, so the task is no longer a causal head: only the reference makes it a parent.
    let later = ok(dir, &["claim", "Unrelated", "--tag", "note", "--as", "alice"]);

    let statement = format!("PR #12 typed-x @ {SHA}: Typed things");
    let written = ok(
        dir,
        &[
            "claim",
            &statement,
            "--type",
            "pr",
            "--attr",
            "pr=12",
            "--attr",
            "branch=typed-x",
            "--attr",
            &format!("sha={SHA}"),
            "--ref",
            &format!("task={}", &task[..10]),
            "--implements",
            &task[..10],
            "--as",
            "alice",
        ],
    );
    let ids: Vec<&str> = written.lines().collect();
    assert_eq!(ids.len(), 2, "the claim and its implements relation: {written}");
    let pr = json(dir, &["show", ids[0], "--json"]);
    let tags: Vec<&str> = pr["body"]["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    let task_ref = format!("ref:task={task}");
    let sha_tag = format!("sha:{SHA}");
    assert_eq!(tags, ["type:pr", "pr", "pr:12", "branch:typed-x", sha_tag.as_str(), task_ref.as_str()]);
    let parents: Vec<&str> = pr["parents"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert!(parents.contains(&task.as_str()) && parents.contains(&later.as_str()), "{parents:?}");
    assert_eq!(pr["typed"]["type"], "pr");
    assert_eq!(pr["typed"]["data"]["pr"], "12");
    assert_eq!(pr["typed"]["refs"][0]["role"], "task");
    assert_eq!(pr["typed"]["refs"][0]["id"], task.as_str());
    assert_eq!(pr["typed"]["problems"], serde_json::json!([]));
    let relation = json(dir, &["show", ids[1], "--json"]);
    assert_eq!(relation["body"]["kind"], "implements");
    assert_eq!(
        (relation["body"]["from"].as_str(), relation["body"]["to"].as_str()),
        (Some(ids[0]), Some(task.as_str()))
    );

    // Readers of plain tags find it, as they find untyped pr claims.
    let prs = json(dir, &["log", "--tag", "pr", "--json"]);
    assert_eq!(prs.as_array().unwrap().len(), 1);

    // A finding with its contradicts relation, in one call; the gate's findings check sees it.
    let finding = ok(
        dir,
        &[
            "claim",
            "src/x.rs:3 drops the error",
            "--type",
            "finding",
            "--attr",
            "severity=medium",
            "--attr",
            "label=confirmed",
            "--attr",
            "at=src/x.rs:3",
            "--contradicts",
            ids[0],
            "--as",
            "alice",
        ],
    );
    let finding = finding.lines().next().unwrap();
    let found = json(dir, &["findings", ids[0], "--triager", "alice", "--json"]);
    assert_eq!(found["findings"][0]["id"], finding);
}

#[test]
fn a_refused_write_writes_nothing_and_says_what_to_fix() {
    let tmp = ground();
    let dir = tmp.path();
    let target = ok(dir, &["claim", "Something", "--as", "alice"]);
    let before = count(dir);
    let cases: &[(&[&str], &str)] = &[
        (&["claim", "x", "--tag", "task,deferred"], "give each tag its own --tag"),
        (&["claim", "x", "--tag", "Finding"], "isn't a tag"),
        (&["claim", "x", "--tag", "two words"], "space"),
        (&["claim", "x", "--type", "fidning"], "there is no type fidning"),
        (&["claim", "x", "--type", "finding", "--attr", "severity=low", "--attr", "label=possible"], "--contradicts"),
        (
            &["claim", "x", "--type", "finding", "--attr", "severity=huge", "--contradicts", &target],
            "severity is one of",
        ),
        (&["claim", "x", "--type", "pr", "--attr", "pr=1"], "needs the key branch"),
        (
            &[
                "claim",
                "PR #2 b @ 0123456789abcdef0123456789abcdef01234567: t",
                "--type",
                "pr",
                "--attr",
                "pr=1",
                "--attr",
                "branch=b",
                "--attr",
                "sha=0123456789abcdef0123456789abcdef01234567",
            ],
            "PR #1 b @",
        ),
        (&["claim", "x", "--type", "decision", "--attr", "source=owner", "--attr", "date=2026-02-30"], "date"),
        (&["claim", "x", "--type", "task", "--tag", "deferred"], "--depends-on"),
        (&["claim", "x", "--attr", "severity=low", "--attr", "severity=high"], "given twice"),
        (&["claim", "x", "--type", "pr", "--tag", "type:finding"], "two types"),
        (&["claim", "x", "--attr", "noequals"], "KEY=VALUE"),
        (&["claim", "x", "--attr", "a:b=c"], "holds no ':'"),
        (&["claim", "x", "--ref", "task=ffff0000"], "--ref task=ffff0000"),
        (&["claim", "x", "--tag", "ref:task=abc"], "full 64-digit id"),
        (&["claim", "x", "--type", "note", "--contradicts", "ffff0000"], "--contradicts ffff0000"),
    ];
    for (args, why) in cases {
        let args: Vec<&str> = args.iter().copied().chain(["--as", "alice"]).collect();
        let err = refused(dir, &args);
        assert!(err.contains(why), "{args:?}: wanted {why:?} in {err}");
    }
    assert_eq!(count(dir), before, "a refused write wrote something");
}

#[test]
fn untyped_claims_keep_working() {
    let tmp = ground();
    let dir = tmp.path();
    let note = ok(dir, &["claim", "A note", "--tag", "note", "--tag", "finding", "--as", "alice"]);
    let c = json(dir, &["show", &note, "--json"]);
    assert_eq!(c["body"]["tags"], serde_json::json!(["note", "finding"]));
    assert!(c.get("typed").is_none(), "{c}");
    // A key:value tag alone is typed data, as an intent's until: is.
    let until = ok(dir, &["claim", "x", "--tag", "until:2999-01-01T00:00:00Z", "--as", "alice"]);
    assert_eq!(json(dir, &["show", &until, "--json"])["typed"]["data"]["until"], "2999-01-01T00:00:00Z");
    // --supersedes still works beside the new flags.
    let newer = ok(dir, &["claim", "A newer note", "--type", "note", "--supersedes", &note, "--as", "alice"]);
    assert_eq!(newer.lines().count(), 2, "{newer}");
}

/// Contributions written some other way, by an older client or another Ground, are as valid as ever: a read
/// reports what doesn't conform and never refuses it.
#[test]
fn a_read_reports_what_does_not_conform_and_never_refuses_it() {
    let tmp = ground();
    let dir = tmp.path();
    let target = ok(dir, &["claim", "Something", "--as", "alice"]);
    ok(dir, &["claim", "Later", "--as", "alice"]);
    let mut store = Store::open(&dir.join(".bracken")).expect("store");
    let alice = store.identity("alice").expect("alice");
    let tags = vec![
        "type:pr".to_owned(),
        "pr:x".to_owned(),
        format!("ref:task={target}"),
        "task,deferred".to_owned(),
        "t\u{1b}[31m:v".to_owned(),
    ];
    let id = store.contribute(&alice, Body::Claim { statement: "PR whatever".into(), tags }).expect("valid");
    drop(store);
    let c = json(dir, &["show", &id.to_hex(), "--json"]);
    let problems: Vec<&str> = c["typed"]["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    let all = problems.join("\n");
    for want in ["comma", "\"t\\u{1b}[31m:v\"", "pr:x", "needs the key branch", "isn't a parent"] {
        assert!(all.contains(want), "wanted {want:?} in {all}");
    }
    ok(dir, &["log"]);
    ok(dir, &["verify"]);
}
