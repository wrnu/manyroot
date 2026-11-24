//! The index store (task `7d140f132b1a`) through the binary: a process that dies between its object
//! write and the index commit (a real abort, through a debug-only hook) leaves a replica the next command
//! repairs, and reads under `BRACKEN_FULL_CHECK`, which compare the index with the full recompute, pass
//! after writes, a crash and a pull.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bracken(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bracken"));
    cmd.args(args).env("BRACKEN_DIR", dir).env_remove("BRACKEN_AGENT").env_remove("BRACKEN_FULL_CHECK");
    cmd.env_remove("BRACKEN_TEST_ABORT_BEFORE_INDEX");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("runs")
}

/// The id a write prints: its first line, `claim <id>`.
fn written(out: &str) -> String {
    out.lines().next().and_then(|l| l.split_whitespace().nth(1)).expect("a written id").to_owned()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = bracken(dir, &[], args);
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8").trim().to_owned()
}

fn full_check(dir: &Path, args: &[&str]) {
    let out = bracken(dir, &[("BRACKEN_FULL_CHECK", "1")], args);
    assert!(out.status.success(), "{args:?} under BRACKEN_FULL_CHECK: {}", String::from_utf8_lossy(&out.stderr));
}

fn index_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir.join("cache"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("index-v") && n.ends_with(".sqlite"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the replica's index holds `id`, read with SQLite directly.
fn indexed(dir: &Path, id: &str) -> bool {
    let files = index_files(dir);
    let file = files.iter().max().expect("an index file");
    let conn = rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("opens");
    let n: i64 =
        conn.query_row("SELECT count(*) FROM obj WHERE lower(hex(id)) = ?1", [id], |r| r.get(0)).expect("a count");
    n == 1
}

#[test]
fn a_process_that_dies_before_the_index_commit_is_repaired_by_the_next_command() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("g");
    let dir_s = dir.to_str().expect("utf-8");
    ok(tmp.path(), &["init", "--dir", dir_s]);
    ok(&dir, &["id", "new", "alice"]);
    for i in 0..5 {
        ok(&dir, &["claim", &format!("claim {i}"), "--tag", "note", "--as", "alice"]);
    }
    assert!(!index_files(&dir).is_empty(), "the index is built by the first commands");
    full_check(&dir, &["log", "--json"]);

    let out = bracken(&dir, &[("BRACKEN_TEST_ABORT_BEFORE_INDEX", "1")], &["claim", "lost", "--as", "alice"]);
    assert!(!out.status.success(), "the hook aborts the process");
    let ids: Vec<String> = serde_json::from_str::<serde_json::Value>(&ok(&dir, &["log", "--json"]))
        .expect("json")
        .as_array()
        .expect("an array")
        .iter()
        .map(|c| c["id"].as_str().expect("an id").to_owned())
        .collect();
    let lost = ids.iter().find(|id| {
        let shown = ok(&dir, &["show", id, "--json"]);
        shown.contains("\"lost\"")
    });
    let lost = lost.expect("the object was written before the abort");
    assert!(indexed(&dir, lost), "the next command's open found it");
    full_check(&dir, &["log", "--json"]);
    full_check(&dir, &["log", "--current", "--json"]);
    full_check(&dir, &["heads", "--json"]);
}

#[test]
fn a_pulled_replica_and_verify_pass_the_full_check() {
    let tmp = tempfile::tempdir().expect("tmp");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    ok(tmp.path(), &["init", "--dir", a.to_str().expect("utf-8")]);
    ok(&a, &["id", "new", "alice"]);
    let first = written(&ok(&a, &["claim", "first", "--tag", "decision", "--as", "alice"]));
    let bundle = tmp.path().join("g.bundle");
    ok(&a, &["export", bundle.to_str().expect("utf-8")]);
    ok(tmp.path(), &["join", bundle.to_str().expect("utf-8"), "--dir", b.to_str().expect("utf-8")]);
    ok(&b, &["id", "new", "bob"]);
    let second = written(&ok(&a, &["claim", "second", "--tag", "decision", "--as", "alice"]));
    ok(&a, &["relate", &second, "supersedes", &first, "--as", "alice"]);
    ok(&b, &["claim", "concurrent", "--tag", "decision", "--as", "bob"]);
    ok(&b, &["log"]);
    ok(&a, &["sync", b.to_str().expect("utf-8")]);
    for dir in [&a, &b] {
        full_check(dir, &["log", "--json"]);
        full_check(dir, &["log", "--current", "--tag", "decision", "--json"]);
        ok(dir, &["verify"]);
        full_check(dir, &["heads", "--json"]);
    }
}
