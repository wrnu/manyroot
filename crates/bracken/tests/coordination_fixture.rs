//! The coordination-Ground fixture through the `bracken` binary, which links Weave in (docs/design/weave.md,
//! section 5 *Keeping the two scripts apart*; build step 8). The pinned binary gains its `weave` commands
//! through a normal upgrade, so this checks that the binary still reads the coordination format's fixture
//! Ground, `crates/manyroot/tests/fixtures/ground-v1`, as recorded in `ground-v1-acceptance.json`: every
//! contribution's status under one pass and under two, the accepted layer and its conflicts. It also checks
//! that `weave check` finds nothing there, and that reading writes nothing. If this fails, that is protocol
//! class: don't update the recorded statuses to make it pass.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../manyroot/tests/fixtures")
}

/// Every file under `dir` with its bytes, by path relative to it.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).expect("read the Ground") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("read");
                found.insert(path.strip_prefix(dir).expect("inside").to_path_buf(), bytes);
            }
        }
    }
    found
}

fn copy(from: &Path, to: &Path) {
    for (path, bytes) in files(from) {
        let path = to.join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }
}

#[test]
fn the_binary_reads_the_coordination_fixture_unchanged() {
    let tmp = tempfile::tempdir().expect("a temporary directory");
    let ground = tmp.path().join("ground-v1");
    copy(&fixtures().join("ground-v1"), &ground);
    let before = files(&ground);
    let bracken = |args: &[&str]| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(tmp.path())
            .env("BRACKEN_DIR", &ground)
            .env_remove("BRACKEN_AGENT")
            .output()
            .expect("bracken runs");
        assert!(out.status.success(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("UTF-8")
    };
    let json = |args: &[&str]| -> Value { serde_json::from_str(&bracken(args)).expect("JSON") };
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("ground-v1-acceptance.json")).expect("the recorded statuses"),
    )
    .expect("JSON");

    for (name, passes) in [("default", "1"), ("two_passes", "2")] {
        let expected = &recorded[name];
        // Each contribution with a status shows it, as recorded.
        let assessments = expected["assessments"].as_array().expect("assessments");
        assert!(!assessments.is_empty());
        for a in assessments {
            let id = a["id"].as_str().expect("an id");
            let shown = json(&["show", id, "--min-passes", passes, "--json"]);
            assert_eq!(&shown["assessment"], a, "{name}: {id}");
        }
        // The accepted layer, in order, and its conflicts.
        let accepted = json(&["accepted", "--min-passes", passes, "--json"]);
        let layer: Vec<&Value> =
            accepted["accepted"].as_array().expect("accepted").iter().map(|c| &c["assessment"]).collect();
        let want: Vec<&Value> = expected["evaluation"]["accepted"].as_array().expect("accepted").iter().collect();
        assert_eq!(layer, want, "{name}");
        assert_eq!(accepted["conflicts"], expected["evaluation"]["conflicts"], "{name}");
    }
    assert_eq!(bracken(&["weave", "check"]).trim(), "no problems");
    assert_eq!(files(&ground), before, "reading wrote to the Ground");
}
