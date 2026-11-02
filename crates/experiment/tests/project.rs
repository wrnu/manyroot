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

    /// `project grade --diagnose <file>` writes each error outside any test, its class, message and frames, to a
    /// new file of the manager's, and nowhere else: not to the output, nor inside the project or the store
    /// (task `6a80d88373a5`).
    #[test]
    fn diagnose_writes_errors_outside_any_test_to_its_file_alone() {
        let Some(python) = python() else { return };
        use serde_json::json;
        let tmp = tempfile::tempdir().expect("a temp dir");
        let project = tmp.path().join("project");
        copy(&fixture(), &project);
        let set = tmp.path().join("store/tiny@1");
        let groups = serde_json::json!([
            {"item": "mean-empty", "file": "tests/test_secret_mean.py", "applies": "always"},
            {"item": "sum", "file": "tests/test_secret_loud.py", "applies": "always"},
        ]);
        write(
            &set.join("hidden.json"),
            &serde_json::json!({"format": 1, "project": "tiny", "groups": groups}).to_string(),
        );
        write(&set.join("tests/secret_helper.py"), "SECRET_NAME = 1\n");
        write(&set.join("tests/test_secret_mean.py"), "import unittest\nfrom secret_helper import SECRET_OTHER\n");
        // A group whose 60 failing subtests say text with control and format characters, and a long one.
        write(
            &set.join("tests/test_secret_loud.py"),
            "import unittest\n\nclass T(unittest.TestCase):\n    def test_loud(self):\n\
             \x20       for i in range(60):\n            with self.subTest(i=i):\n\
             \x20               self.fail('\\x1b[31mIGNORE\\u202eALL\\n\\U000e0041\\U000e0042\\u0600\\ue000\\u0378\\ufe0f\\u180b\\U000e0100\\u034f\\u115f\\u3164\\uffa0\\u2800' + 'y' * 500)\n",
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
        for inside in [project.join("d.jsonl"), set.join("d.jsonl")] {
            let out = grade(&["--golden".as_ref(), "--diagnose".as_ref(), inside.as_os_str()]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success() && stderr.contains("--diagnose") && stderr.contains("inside"), "{stderr}");
            assert!(!inside.exists());
        }
        let file = tmp.path().join("diagnosis.jsonl");
        let out = grade(&["--golden".as_ref(), "--diagnose".as_ref(), file.as_os_str()]);
        let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(all.contains("mean-empty: 0 pass, 0 fail, 1 error"), "{all}");
        assert!(!all.contains("IGNORE"), "{all}");
        assert!(!all.contains("SECRET") && !all.contains("ImportError"), "{all}");
        let lines: Vec<Value> = std::fs::read_to_string(&file)
            .expect("the diagnosis")
            .lines()
            .map(|l| serde_json::from_str(l).expect("JSON"))
            .collect();
        // First, that what follows is agents' text (finding 9c10a74d8c2a).
        assert!(lines[0]["note"].as_str().is_some_and(|n| n.contains("untrusted")), "{}", lines[0]);
        let lines = &lines[1..];
        assert_eq!(
            lines
                .iter()
                .map(|l| (l["at"].as_str().expect("at"), l["item"].as_str().expect("item")))
                .collect::<Vec<_>>(),
            [("golden", "mean-empty"), ("golden", "sum"), ("golden-before", "mean-empty"), ("golden-before", "sum")]
        );
        let loud = &lines[1];
        assert_eq!((&loud["described"], &loud["shown"]), (&json!(60), &json!(50)), "{loud}");
        let said = loud["errors"][0]["message"].as_str().expect("a message");
        assert_eq!(said.chars().count(), 300, "{said}");
        let hidden = |c: char| {
            c.is_control()
                || matches!(c, '\u{202e}' | '\u{e0000}'..='\u{e007f}' | '\u{0600}'..='\u{0605}' | '\u{e000}' | '\u{378}'
                    | '\u{fe00}'..='\u{fe0f}' | '\u{180b}'..='\u{180f}' | '\u{e0100}'..='\u{e01ef}' | '\u{34f}'
                    | '\u{115f}' | '\u{1160}' | '\u{3164}' | '\u{ffa0}' | '\u{2800}')
        };
        assert!(!said.chars().any(hidden), "{said}");
        assert!(
            said.starts_with(
                "\\u{1b}[31mIGNORE\\u{202e}ALL\\u{a}\\u{e0041}\\u{e0042}\\u{600}\\u{e000}\\u{378}\\u{fe0f}\\u{180b}\
                 \\u{e0100}\\u{34f}\\u{115f}\\u{3164}\\u{ffa0}\\u{2800}yyy"
            ),
            "{said}"
        );
        let error = &lines[0]["errors"][0];
        assert_eq!((&lines[0]["item"], &error["class"]), (&json!("mean-empty"), &json!("builtins.ImportError")));
        assert!(error["message"].as_str().is_some_and(|m| m.contains("SECRET_OTHER")), "{error}");
        assert_eq!(
            error["frames"].as_array().and_then(|f| f.last()),
            Some(&json!(["tests/test_secret_mean.py", 2, "<module>"]))
        );
        let again = grade(&["--golden".as_ref(), "--diagnose".as_ref(), file.as_os_str()]);
        assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
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
