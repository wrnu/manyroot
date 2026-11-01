//! `bracken-experiment project check` on the example project in `tests/fixtures/project-tiny`
//! (docs/design/eval-suite.md, Project format).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny")
}

fn check(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(bin()).args(["project", "check"]).args(args).output().expect("project check runs")
}

/// A copy of every file under `from` in `to`.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from).expect("a directory") {
        let entry = entry.expect("an entry");
        if entry.file_type().expect("a type").is_dir() {
            copy(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).expect("a copy");
        }
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    std::fs::write(path, text).expect("a file");
}

#[test]
fn the_example_project_checks_and_says_what_it_holds() {
    let out = check(&[fixture().as_os_str()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("tiny: python 3.12, 3 items (1 feature, 1 fix, 1 change), 1 released later"), "{stdout}");
    assert!(stdout.contains("seed: 3 files, 1 public test files"), "{stdout}");
    assert!(stdout.contains("hidden tiny@1: not checked"), "{stdout}");

    let out = check(&[fixture().as_os_str(), "--json".as_ref()]);
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(v["items"], 3);
    assert_eq!(v["kinds"]["change"], 1);
    assert_eq!(v["limits"]["cpu_seconds"], 5);
    assert_eq!(v["limits"]["memory_mb"], 512, "a limit not set takes its default");
    assert_eq!(v["hidden"]["checked"], Value::Null);
}

/// With --hidden, the store is checked against the project's digest, and only counts are printed: no
/// test file's name or content.
#[test]
fn a_hidden_set_is_checked_by_digest_and_reported_in_counts() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let project = tmp.path().join("project");
    copy(&fixture(), &project);
    let set = tmp.path().join("store/tiny@1");
    let groups = serde_json::json!([
        {"item": "sum", "file": "tests/test_secret_sum_before.py", "applies": "before-change"},
        {"item": "sum", "file": "tests/test_secret_sum_after.py", "applies": "after-change"},
        {"item": "mean-empty", "file": "tests/test_secret_mean.py", "applies": "always"},
    ]);
    write(&set.join("hidden.json"), &serde_json::json!({"format": 1, "project": "tiny", "groups": groups}).to_string());
    for file in ["test_secret_sum_before.py", "test_secret_sum_after.py", "test_secret_mean.py"] {
        write(&set.join("tests").join(file), "import unittest  # SECRET-CONTENT\n");
    }
    write(&set.join("golden/tiny/__init__.py"), "");
    write(&set.join("golden-before/tiny/__init__.py"), "");
    write(&set.join("golden/tiny/stats.py"), "# SECRET-CONTENT\n");
    let store = tmp.path().join("store");

    let out = check(&[project.as_os_str(), "--hidden".as_ref(), store.as_os_str()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("not the project's"), "{stderr}");
    let digest = bracken_experiment::project::digest(&set).expect("a digest");
    let manifest = std::fs::read_to_string(project.join("project.json")).expect("the manifest");
    std::fs::write(project.join("project.json"), manifest.replace(&"0".repeat(64), &digest)).expect("written");

    let out = check(&[project.as_os_str(), "--hidden".as_ref(), store.as_os_str()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        stdout.contains(
            "hidden tiny@1: digest matches; 3 groups in 3 test files; golden solution 2 files, 1 before the change"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("items no hidden test judges: decimals"), "{stdout}");
    let json = check(&[project.as_os_str(), "--hidden".as_ref(), store.as_os_str(), "--json".as_ref()]);
    for out in [&out, &json] {
        let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!all.contains("secret") && !all.contains("SECRET"), "{all}");
    }
    let v: Value = serde_json::from_slice(&json.stdout).expect("JSON");
    assert_eq!(v["hidden"]["checked"]["groups"], 3);
}

/// These need Docker: where it isn't running, each says so and passes without checking anything. In `macos_only`
/// though they don't need macOS, so the counted run's Mac side runs them (docs/development.md).
mod macos_only {
    use super::*;

    /// The image these tests pin, as `python::tests` does; None, having said why, where they can't run.
    fn python() -> Option<String> {
        bracken_experiment::python::for_tests().map(|p| p.image)
    }

    /// `project grade` grades a tree and runs the approval check, saying only counts per item: no test's
    /// name or content.
    #[test]
    fn grade_says_counts_per_item_and_runs_the_approval_check() {
        let Some(python) = python() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let project = tmp.path().join("project");
        copy(&fixture(), &project);
        let set = tmp.path().join("store/tiny@1");
        let groups =
            serde_json::json!([{"item": "mean-empty", "file": "tests/test_secret_mean.py", "applies": "always"}]);
        write(
            &set.join("hidden.json"),
            &serde_json::json!({"format": 1, "project": "tiny", "groups": groups}).to_string(),
        );
        write(
            &set.join("tests/test_secret_mean.py"),
            "import unittest  # SECRET-CONTENT\nfrom tiny.stats import mean\n\nclass T(unittest.TestCase):\n\
             \x20   def test_secret(self):\n        self.assertEqual(mean([1, 2, 3]), 2)\n",
        );
        for tree in ["golden", "golden-before"] {
            copy(&project.join("seed/tiny"), &set.join(tree).join("tiny"));
        }
        let digest = bracken_experiment::project::digest(&set).expect("a digest");
        let manifest = std::fs::read_to_string(project.join("project.json")).expect("the manifest");
        std::fs::write(project.join("project.json"), manifest.replace(&"0".repeat(64), &digest)).expect("written");
        let grade = |extra: &[&std::ffi::OsStr]| {
            Command::new(bin())
                .args(["project", "grade"])
                .arg(&project)
                .arg("--hidden")
                .arg(tmp.path().join("store"))
                .arg("--python")
                .arg(&python)
                .args(extra)
                .output()
                .expect("project grade runs")
        };
        let golden = grade(&["--golden".as_ref()]);
        let seed = grade(&["--tree".as_ref(), project.join("seed").as_os_str(), "--json".as_ref()]);
        let stdout = String::from_utf8_lossy(&golden.stdout);
        assert!(golden.status.success(), "{stdout}{}", String::from_utf8_lossy(&golden.stderr));
        assert!(stdout.contains("  mean-empty: 1 pass, 0 fail, 0 error, 0 skip"), "{stdout}");
        assert!(stdout.contains("approval check: passed"), "{stdout}");
        let v: Value = serde_json::from_slice(&seed.stdout).expect("JSON");
        assert_eq!(v["grade"]["items"][0]["counts"]["pass"], 1, "{v}");
        for out in [&golden, &seed] {
            let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(!all.contains("secret") && !all.contains("SECRET"), "{all}");
        }
    }

    /// `run-tests` runs the example's public test under the Python profile, with the image it pins.
    #[test]
    fn run_tests_runs_the_example_projects_public_tests() {
        let Some(python) = python() else { return };
        let out = Command::new(bin())
            .args(["run-tests", "--project"])
            .arg(fixture())
            .arg("--python")
            .arg(&python)
            .output()
            .expect("run-tests runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.starts_with("1 tests: 1 pass, 0 fail, 0 error, 0 skip (Code(0)); python 3.12."), "{stdout}");
    }
}
