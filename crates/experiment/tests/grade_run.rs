//! Grading a project run after it ends (task `e4970250b3e4`, answer `07959d6d9bc0`): where a grades file may
//! go, and, on macOS, a mock run of the tiny fixture project graded with a throwaway hidden store, with nothing
//! of the store in the run; and a run that holds its change back.

use std::path::Path;

use serde_json::{Value, json};

use bracken_experiment::grade_run::check_grades_path;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    std::fs::write(path, text).expect("a file");
}

/// A grades file goes to a new path outside the run, the project, the store, any Ground and any git checkout;
/// a refusal names the store by what it is, never by its path.
#[test]
fn a_grades_file_goes_only_somewhere_new_and_apart() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("resolved");
    let (run, project, store) = (root.join("run"), root.join("project"), root.join("store-q7z"));
    for d in [&run, &project, &store] {
        std::fs::create_dir_all(d).expect("a directory");
    }
    let inside =
        [("the run", run.as_path()), ("the project", project.as_path()), ("the private store", store.as_path())];
    std::fs::create_dir_all(root.join("out")).expect("a directory");
    check_grades_path(&root.join("out/grades.jsonl"), &inside).expect("a new file apart from everything");

    write(&root.join("out/old.jsonl"), "");
    assert!(check_grades_path(&root.join("out/old.jsonl"), &inside).expect_err("exists").contains("already exists"));
    for (dir, what) in [(&run, "the run"), (&project, "the project"), (&store, "the private store")] {
        let err = check_grades_path(&dir.join("grades.jsonl"), &inside).expect_err("inside");
        assert!(err.contains(&format!("inside {what}")), "{err}");
        assert!(!err.contains("store-q7z"), "no path of the store: {err}");
    }

    // A Ground: a replica's directory, or a checkout's .bracken.
    write(&root.join("ground/ground.json"), "{}");
    std::fs::create_dir_all(root.join("ground/sub")).expect("a directory");
    assert!(
        check_grades_path(&root.join("ground/sub/grades.jsonl"), &inside).expect_err("a Ground").contains("Ground")
    );
    write(&root.join("work/.bracken/ground.json"), "{}");
    assert!(check_grades_path(&root.join("work/grades.jsonl"), &inside).expect_err("a Ground").contains("Ground"));

    // A git checkout.
    std::fs::create_dir_all(root.join("repo/.git")).expect("a directory");
    let err = check_grades_path(&root.join("repo/grades.jsonl"), &inside).expect_err("a checkout");
    assert!(err.contains("git checkout"), "{err}");
}

/// The hold-back flag: the hub is given only the items released at the start, and no change.
#[test]
fn a_held_change_never_reaches_the_hub() {
    use bracken_experiment::project::Project;
    use bracken_experiment::weave_mode::{ANY_AGENT, hub_data};
    let tiny = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny");
    let project = Project::load(&tiny).expect("the fixture");
    let ids = |d: &Value| d["items"].as_array().expect("items").iter().map(|i| i["id"].clone()).collect::<Vec<_>>();
    let all = hub_data(&project, ANY_AGENT, false).expect("the data");
    assert_eq!(
        (ids(&all), all["change"].clone()),
        (vec![json!("sum"), json!("mean-empty"), json!("decimals")], json!(true))
    );
    let held = hub_data(&project, ANY_AGENT, true).expect("the data");
    assert_eq!((ids(&held), held["change"].clone()), (vec![json!("sum"), json!("mean-empty")], json!(false)));
    assert!(!held.to_string().contains("decimal"), "nothing of the held change: {held}");
}

#[cfg(target_os = "macos")]
mod macos_only {
    use std::path::PathBuf;

    use super::*;
    use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, git_binary, run};
    use bracken_experiment::mock::MockBackend;
    use bracken_experiment::plan::{PlanInputs, ProjectPlan};
    use bracken_experiment::project::{Project, digest};
    use bracken_experiment::python::Python;
    use bracken_experiment::weave_mode::ANY_AGENT;

    /// What the hidden tests are marked with, in their files' names, their tests' names and their text.
    const MARKER: &str = "marker_q7z";

    fn bin() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
    }

    fn bracken() -> PathBuf {
        let b = bin().with_file_name("bracken");
        assert!(b.exists(), "build bracken first: cargo build -p bracken");
        b
    }

    fn pin() -> bracken_experiment::python::PythonPin {
        let path = std::env::var_os("BRACKEN_TEST_PYTHON").unwrap_or_else(|| "/opt/homebrew/bin/python3.12".into());
        assert!(Path::new(&path).exists(), "this test needs Python 3.12 at {path:?} (or set BRACKEN_TEST_PYTHON)");
        Python::pin(Path::new(&path), "3.12", None).expect("pinned").plan_pin()
    }

    /// The tiny fixture copied into `root/tiny`, its hidden set a throwaway store in `root/store`: one group for
    /// `sum` and one for `decimals`, two tests each: one needing a `sum_of` the seed hasn't, one passing.
    fn project_and_store(root: &Path) -> (PathBuf, PathBuf) {
        let set = root.join("store/tiny@1");
        let groups = json!([
            {"item": "sum", "file": format!("tests/test_sum_{MARKER}.py"), "applies": "always"},
            {"item": "decimals", "file": format!("tests/test_decimals_{MARKER}.py"), "applies": "always"},
        ]);
        write(&set.join("hidden.json"), &json!({"format": 1, "project": "tiny", "groups": groups}).to_string());
        for item in ["sum", "decimals"] {
            let test = format!(
                "# {MARKER}\nimport unittest\nimport tiny.stats\n\nclass T(unittest.TestCase):\n\
                 \x20   def test_{MARKER}_{item}_a(self):\n        self.assertTrue(hasattr(tiny.stats, 'sum_of'))\n\
                 \x20   def test_{MARKER}_{item}_b(self):\n        self.assertEqual(1, 1)\n"
            );
            write(&set.join(format!("tests/test_{item}_{MARKER}.py")), &test);
        }
        for tree in ["golden", "golden-before"] {
            write(&set.join(tree).join("tiny/__init__.py"), "");
            write(&set.join(tree).join("tiny/stats.py"), "def sum_of(xs):\n    return sum(xs)\n");
        }
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny");
        let p = root.join("tiny");
        for (path, bytes) in all_bytes(&fixture) {
            let to = p.join(path.strip_prefix(&fixture).expect("under the fixture"));
            std::fs::create_dir_all(to.parent().expect("a parent")).expect("a directory");
            std::fs::write(to, bytes).expect("a file");
        }
        let manifest = p.join("project.json");
        let mut m: Value = serde_json::from_slice(&std::fs::read(&manifest).expect("the manifest")).expect("JSON");
        m["hidden"]["digest"] = json!(digest(&set).expect("the set's digest"));
        std::fs::write(&manifest, m.to_string()).expect("the manifest");
        (p, root.join("store"))
    }

    fn config(root: PathBuf, agents: usize, turns: u32, project: &Path, hold_change: bool, sandbox: bool) -> RunConfig {
        RunConfig {
            root,
            mode: "weave",
            agents,
            turns,
            worker_bin: bin(),
            bracken: bracken(),
            git: git_binary().expect("git"),
            trace: false,
            calls_per_turn: CALLS_PER_TURN,
            turn_limit: TURN_LIMIT,
            plan: PlanInputs {
                project: Some(ProjectPlan::of(&Project::load(project).expect("the project")).expect("its plan")),
                python: Some(pin()),
                integrator: Some(ANY_AGENT.into()),
                hold_change,
                ..PlanInputs::default()
            },
            sandbox,
        }
    }

    /// Every file under `dir`, read whole.
    fn all_bytes(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for entry in std::fs::read_dir(&d).expect("a directory") {
                let entry = entry.expect("an entry");
                let meta = std::fs::symlink_metadata(entry.path()).expect("metadata");
                if meta.is_dir() {
                    todo.push(entry.path());
                } else if meta.is_file() {
                    out.push((entry.path(), std::fs::read(entry.path()).expect("a file")));
                }
            }
        }
        out
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// The tiny project, 2 agents x 3 turns, sandboxed, then graded: a line for each checkpoint and the end,
    /// the change's group applying from the first checkpoint after it went out, each group's outcomes by index.
    /// Nothing of the hidden set or the store's path is in the run, its Grounds or the grades.
    #[test]
    fn a_project_run_is_graded_after_it_ends() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let cfg = config(root.join("run"), 2, 3, &project, false, true);
        let mock = MockBackend { seed: 1, shim: Some(bin()), lockstep: true, ..MockBackend::default() };
        run(&cfg, &mock).expect("the run");

        std::fs::create_dir_all(root.join("grades")).expect("a directory");
        let grades = root.join("grades/run.jsonl");
        let out = std::process::Command::new(bin())
            .args(["grade-run", "--json", "--dir"])
            .arg(&cfg.root)
            .arg("--hidden")
            .arg(&store)
            .arg("--grades")
            .arg(&grades)
            .output()
            .expect("grade-run");
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{stdout}{stderr}");
        let summary: Value = serde_json::from_str(stdout.trim()).expect("a JSON summary");
        assert_eq!((summary["project"].clone(), summary["points"].clone()), (json!("tiny"), json!(4)), "{summary}");
        assert!(!stdout.contains(&*store.to_string_lossy()), "no path of the store: {stdout}");

        let text = std::fs::read_to_string(&grades).expect("the grades");
        let points: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect();
        let at: Vec<Value> = points.iter().map(|p| p["at"].clone()).collect();
        assert_eq!(at, [json!(1), json!(2), json!(3), json!("end")]);
        // The change goes out once 3 of the 6 agent-turns are used: during turn 2.
        let released: Vec<bool> = points.iter().map(|p| p["change_released"] == true).collect();
        assert_eq!(released, [false, true, true, true]);
        assert_eq!(points[0]["turn_share"], json!(1.0 / 3.0));
        assert_eq!(points[3]["turn_share"], json!(1.0));
        for p in &points {
            let indexes: Vec<u64> = p["grade"]["groups"]
                .as_array()
                .expect("groups")
                .iter()
                .map(|g| g["index"].as_u64().expect("an index"))
                .collect();
            let expect: &[u64] = if p["change_released"] == true { &[0, 1] } else { &[0] };
            assert_eq!(indexes, expect, "{p}");
            for g in p["grade"]["groups"].as_array().expect("groups") {
                // The seed has no `sum_of`, and no agent of the mock writes one: by index, the first fails.
                assert_eq!(g["outcomes"], json!(["fail", "pass"]), "{g}");
            }
        }
        // Each tree is checked out of the hub by the manifest its event names; in lockstep the line never forks.
        for p in &points {
            assert!(p["manifest"].as_str().is_some_and(|m| m.starts_with("blake3:")), "{p}");
            assert!(p["decision"].is_string() && p["forked"] == false, "{p}");
        }
        assert_eq!(points[2]["manifest"], points[3]["manifest"], "the end is the last checkpoint's tree");

        // Nothing of the store: no marker, test name or path, in the grades or anywhere in the run.
        let store_path = store.to_string_lossy().into_owned();
        assert!(!text.contains(MARKER) && !text.contains(&store_path), "{text}");
        for (path, bytes) in all_bytes(&cfg.root) {
            assert!(!contains(&bytes, MARKER.as_bytes()), "{} holds the marker", path.display());
            assert!(!contains(&bytes, store_path.as_bytes()), "{} holds the store's path", path.display());
        }

        // Graded once is graded: the file is never written over.
        let again = std::process::Command::new(bin())
            .args(["grade-run", "--dir"])
            .arg(&cfg.root)
            .arg("--hidden")
            .arg(&store)
            .arg("--grades")
            .arg(&grades)
            .output()
            .expect("grade-run");
        assert!(!again.status.success());
        assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
    }

    /// A run holding its change back never releases it, and its plan says so: no change point, no held item's
    /// text among what agents are given. Graded, no point has the change.
    #[test]
    fn a_held_change_is_never_released() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let cfg = config(root.join("run"), 1, 2, &project, true, false);
        let mock = MockBackend { seed: 1, shim: Some(bin()), lockstep: true, ..MockBackend::default() };
        run(&cfg, &mock).expect("the run");
        let log = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        assert!(!log.contains("\"changes_released\""), "no release");
        let plan: Value =
            serde_json::from_slice(&std::fs::read(cfg.root.join("plan.json")).expect("the plan")).expect("JSON");
        assert_eq!(plan["inputs"]["hold_change"], true);
        assert_eq!(plan["change_after"], Value::Null);
        assert!(plan["prompts"]["item-sum"].is_string(), "{plan}");
        assert!(plan["prompts"].get("item-decimals").is_none(), "{plan}");

        let grades = root.join("held.jsonl");
        let summary = bracken_experiment::grade_run::grade_run(&cfg.root, &store, &grades, &bracken()).expect("graded");
        assert_eq!(summary.points, 3);
        let text = std::fs::read_to_string(&grades).expect("the grades");
        assert!(text.lines().all(|l| l.contains("\"change_released\":false")), "{text}");
    }
}
