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
    use bracken_experiment::mock::{BREAK_LINE, EXIT_LINE, MockBackend, shared_line};
    use bracken_experiment::plan::{PlanInputs, ProjectPlan};
    use bracken_experiment::project::{Project, digest};
    use bracken_experiment::python::PythonPin;
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

    /// The image these tests pin, as `python::tests` does; None, having said why, where they can't run.
    fn pin() -> Option<bracken_experiment::python::PythonPin> {
        bracken_experiment::python::for_tests().map(|p| p.plan_pin())
    }

    /// The tiny fixture copied into `root/tiny`, its hidden set a throwaway store in `root/store`, four groups:
    /// `sum`'s before and after its change, `decimals`'s, each two tests (one needing a `sum_of` the seed hasn't,
    /// one passing), and `mean-empty`'s one test, which passes only once agent 0's turn-2 module is on the line,
    /// so the grades differ by tree and by side of the change (finding 1c68a820ee05).
    fn project_and_store(root: &Path) -> (PathBuf, PathBuf) {
        let set = root.join("store/tiny@1");
        let groups = json!([
            {"item": "sum", "file": format!("tests/test_sum_before_{MARKER}.py"), "applies": "before-change"},
            {"item": "sum", "file": format!("tests/test_sum_after_{MARKER}.py"), "applies": "after-change"},
            {"item": "decimals", "file": format!("tests/test_decimals_{MARKER}.py"), "applies": "always"},
            {"item": "mean-empty", "file": format!("tests/test_turn_{MARKER}.py"), "applies": "always"},
        ]);
        write(&set.join("hidden.json"), &json!({"format": 1, "project": "tiny", "groups": groups}).to_string());
        let turn = format!(
            "# {MARKER}\nimport unittest\n\nclass T(unittest.TestCase):\n\
             \x20   def test_{MARKER}_turn(self):\n        import tiny.by_agent_0_turn_2 as m\n\
             \x20       self.assertEqual(m.VALUE, 20)\n"
        );
        write(&set.join(format!("tests/test_turn_{MARKER}.py")), &turn);
        for item in ["sum_before", "sum_after", "decimals"] {
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

    fn config(
        python: PythonPin,
        root: PathBuf,
        agents: usize,
        turns: u32,
        project: &Path,
        hold_change: bool,
        sandbox: bool,
    ) -> RunConfig {
        config_of("weave", ANY_AGENT, python, root, agents, turns, project, hold_change, sandbox)
    }

    #[allow(clippy::too_many_arguments)]
    fn config_of(
        mode: &'static str,
        integrator: &str,
        python: PythonPin,
        root: PathBuf,
        agents: usize,
        turns: u32,
        project: &Path,
        hold_change: bool,
        sandbox: bool,
    ) -> RunConfig {
        RunConfig {
            root,
            mode,
            agents,
            turns,
            worker_bin: bin(),
            bracken: bracken(),
            git: git_binary().expect("git"),
            trace: false,
            calls_per_turn: CALLS_PER_TURN,
            // Every change to an integrator's main runs the public tests in a container, which on a loaded
            // machine can take minutes each: a turn's limit here is for a stuck turn, not a slow Docker.
            turn_limit: 3 * TURN_LIMIT,
            plan: PlanInputs {
                project: Some(ProjectPlan::of(&Project::load(project).expect("the project")).expect("its plan")),
                python: Some(python),
                integrator: Some(integrator.into()),
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
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let cfg = config(python, root.join("run"), 2, 3, &project, false, true);
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
        for (k, p) in points.iter().enumerate() {
            let groups = p["grade"]["groups"].as_array().expect("groups");
            let indexes: Vec<u64> = groups.iter().map(|g| g["index"].as_u64().expect("an index")).collect();
            // Before the change: sum's old tests and the turn test; after: sum's new tests, decimals' and it.
            let expect: &[u64] = if p["change_released"] == true { &[1, 2, 3] } else { &[0, 3] };
            assert_eq!(indexes, expect, "{p}");
            for g in groups {
                let outcomes = if g["index"] == 3 {
                    // Agent 0's turn-2 module is on the line from the second checkpoint.
                    if k == 0 { json!(["error"]) } else { json!(["pass"]) }
                } else {
                    // The seed has no `sum_of`, and no agent of the mock writes one: by index, the first fails.
                    json!(["fail", "pass"])
                };
                assert_eq!(g["outcomes"], outcomes, "point {k}: {g}");
            }
        }
        // Each tree is checked out of the hub by the manifest its event names; in lockstep the line never forks.
        for p in &points {
            assert!(p["manifest"].as_str().is_some_and(|m| m.starts_with("blake3:")), "{p}");
            assert!(p["decision"].is_string() && p["forked"] == false, "{p}");
        }
        assert_eq!(points[2]["manifest"], points[3]["manifest"], "the end is the last checkpoint's tree");
        assert!(
            points[0]["manifest"] != points[1]["manifest"] && points[1]["manifest"] != points[2]["manifest"],
            "each checkpoint's own tree, as the agents integrated"
        );

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

        // A tree that can't be checked out is that point's error, and every other tree is still graded
        // (finding aad6a4f0057b); nothing but the grades file is left beside it.
        let events = cfg.root.join("events.jsonl");
        let text = std::fs::read_to_string(&events).expect("the events");
        let mut edited = false;
        let lines: Vec<String> = text
            .lines()
            .map(|l| {
                let mut e: Value = serde_json::from_str(l).expect("an event");
                if !edited && e["event"] == "checkpoint" {
                    e["data"]["line"]["heads"][0]["manifest"] = json!(format!("blake3:{}", "0".repeat(64)));
                    edited = true;
                }
                e.to_string()
            })
            .collect();
        std::fs::write(&events, lines.join("\n") + "\n").expect("the events");
        let bad = root.join("grades/bad.jsonl");
        let inside = cfg.root.join("diagnosis.jsonl");
        let refused = bracken_experiment::grade_run::grade_run(&cfg.root, &store, &bad, &bracken(), Some(&inside));
        assert!(refused.is_err_and(|e| e.contains("--diagnose") && e.contains("inside the run")));
        assert!(!inside.exists() && !bad.exists());
        let diagnosis = root.join("diagnosis.jsonl");
        let summary = bracken_experiment::grade_run::grade_run(&cfg.root, &store, &bad, &bracken(), Some(&diagnosis))
            .expect("graded");
        // The mock's trees fail some tests: each is described with its index, by the point it was graded at,
        // and no error was outside any test. The grades file says nothing of it.
        let described: Vec<Value> = std::fs::read_to_string(&diagnosis)
            .expect("the diagnosis")
            .lines()
            .map(|l| serde_json::from_str(l).expect("JSON"))
            .collect();
        assert!(described[0]["note"].as_str().is_some_and(|n| n.contains("untrusted")), "{}", described[0]);
        for d in &described[1..] {
            assert!(d["at"]["manifest"].is_string() && d["group"].is_u64(), "{d}");
            let errors = d["errors"].as_array().expect("errors");
            assert!(!errors.is_empty() && errors.iter().all(|e| e["test"].is_u64() && e["class"].is_string()), "{d}");
        }
        let grades_text = std::fs::read_to_string(&bad).expect("the grades");
        assert!(!grades_text.contains("frames") && !grades_text.contains("AssertionError"), "{grades_text}");
        assert_eq!((summary.points, summary.errors), (4, 1), "{summary:?}");
        let lines: Vec<Value> = std::fs::read_to_string(&bad)
            .expect("the grades")
            .lines()
            .map(|l| serde_json::from_str(l).expect("JSON"))
            .collect();
        assert!(lines[0]["error"].is_string() && lines[0].get("grade").is_none(), "{}", lines[0]);
        assert!(lines[1..].iter().all(|l| l["grade"]["groups"].is_array() && l.get("error").is_none()));
        let names: Vec<String> = std::fs::read_dir(root.join("grades"))
            .expect("the directory")
            .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "no partial file left: {names:?}");
    }

    /// Git project mode (task `6bd13d994a03`): the tiny project, 4 agents x 2 turns, agent-3 the one integrator,
    /// sandboxed, then graded as a Weave run is: a line for each checkpoint and the end, each tree read out of the
    /// hub's repository by the tree id its event names, with main's commit. The builders' turn-2 work is on main
    /// by the second checkpoint, through agent-3's merges of their requests.
    #[test]
    fn a_git_project_run_is_graded_after_it_ends() {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let name = bracken_experiment::git_project::NAME;
        let cfg = config_of(name, "agent-3", python, root.join("run"), 4, 2, &project, false, true);
        let mock =
            MockBackend { seed: 1, shim: Some(bin()), lockstep: true, integrator: Some(3), ..MockBackend::default() };
        run(&cfg, &mock).expect("the run");
        let log = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        assert!(!log.contains("\"turn_error\""), "no turn erred");
        let plan: Value =
            serde_json::from_slice(&std::fs::read(cfg.root.join("plan.json")).expect("the plan")).expect("JSON");
        assert_eq!((plan["mode"].clone(), plan["rules"][0][0].clone()), (json!(name), json!("git-integrator")));
        let roles = (plan["prompts"]["role-integrator"].clone(), plan["prompts"]["role-builder"].clone());
        let hash = |t: &str| json!(blake3::hash(t.as_bytes()).to_hex().to_string());
        use bracken_experiment::prompts::{GIT_BUILDER_ROLE, GIT_INTEGRATOR_ROLE};
        assert_eq!(roles, (hash(GIT_INTEGRATOR_ROLE), hash(GIT_BUILDER_ROLE)), "git's role lines are hashed");

        let grades = root.join("git.jsonl");
        let summary =
            bracken_experiment::grade_run::grade_run(&cfg.root, &store, &grades, &bracken(), None).expect("graded");
        assert_eq!((summary.points, summary.errors), (3, 0), "{summary:?}");
        let points: Vec<Value> = std::fs::read_to_string(&grades)
            .expect("the grades")
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect();
        let at: Vec<Value> = points.iter().map(|p| p["at"].clone()).collect();
        assert_eq!(at, [json!(1), json!(2), json!("end")]);
        // The change goes out once 4 of the 8 agent-turns are used: after the first checkpoint.
        let released: Vec<bool> = points.iter().map(|p| p["change_released"] == true).collect();
        assert_eq!(released, [false, true, true]);
        for (k, p) in points.iter().enumerate() {
            let hex = |v: &Value| v.as_str().is_some_and(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(hex(&p["decision"]) && hex(&p["manifest"]) && p["forked"] == false, "main's commit and tree: {p}");
            let turn = p["grade"]["groups"].as_array().expect("groups").iter().find(|g| g["index"] == 3).expect("3");
            let outcomes = if k == 0 { json!(["error"]) } else { json!(["pass"]) };
            assert_eq!(turn["outcomes"], outcomes, "agent 0's turn-2 branch is on main from checkpoint 2: {p}");
        }
        assert_eq!(points[1]["manifest"], points[2]["manifest"], "the end is the last checkpoint's tree");
        assert_ne!(points[0]["manifest"], points[1]["manifest"]);
        let text = std::fs::read_to_string(&grades).expect("the grades");
        let store_path = store.to_string_lossy().into_owned();
        assert!(!text.contains(MARKER) && !text.contains(&store_path), "{text}");
        for (path, bytes) in all_bytes(&cfg.root) {
            assert!(!contains(&bytes, MARKER.as_bytes()), "{} holds the marker", path.display());
        }
    }

    /// A run holding its change back never releases it, and its plan says so: no change point, no held item's
    /// text among what agents are given. Graded, no point has the change.
    #[test]
    fn a_held_change_is_never_released() {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let cfg = config(python, root.join("run"), 1, 2, &project, true, false);
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
        let summary =
            bracken_experiment::grade_run::grade_run(&cfg.root, &store, &grades, &bracken(), None).expect("graded");
        assert_eq!(summary.points, 3);
        let text = std::fs::read_to_string(&grades).expect("the grades");
        for l in text.lines() {
            let p: Value = serde_json::from_str(l).expect("JSON");
            assert_eq!(p["change_released"], false, "{p}");
            let indexes: Vec<&Value> =
                p["grade"]["groups"].as_array().expect("groups").iter().map(|g| &g["index"]).collect();
            assert_eq!(indexes, [&json!(0), &json!(3)], "the before-change side, always: {p}");
        }
    }

    /// Stage 3 (task `cb6a40559593`), in each project mode: the tiny project, 4 agents x 2 turns, agent-0 the one
    /// integrator, agents playing at once, sandboxed, then graded. Builders take items, never one another's, and
    /// the integrator none; the change goes to the builder holding the item it revises. The integrator's last turn
    /// starts only once every builder's last turn has ended, so every builder's turn-2 work, finished at the run's
    /// end, is on main at the end.
    ///
    /// Stage 4 (task `61911c91247f`): each of the integrator's turns starts only once every builder has ended that
    /// turn, so the builders' turn-1 work is on main at checkpoint 1, not only at the end. And a call to a name the
    /// shim doesn't offer, refused by the harness, doesn't stop the run: it is counted, and the agent is told its
    /// tools on its next result (agent-1, turn 1) or, when its turn ended first, as its next turn opens (agent-2).
    fn a_run_divides_the_items_and_its_integrator_plays_last(mode: &'static str) {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, store) = project_and_store(&root);
        let cfg = config_of(mode, "agent-0", python, root.join("run"), 4, 2, &project, false, true);
        let offered = bracken_experiment::driver::harness_tools(
            bracken_experiment::mode::mode_named(mode).expect("a mode"),
            bracken_experiment::task::Task::Phase1,
            true,
        );
        let mock = MockBackend {
            seed: 1,
            shim: Some(bin()),
            lockstep: false,
            integrator: Some(0),
            unknown_at: vec![(1, 1, true), (2, 1, false)],
            unknown: bracken_experiment::backend::UnknownCalls::new(offered),
            ..MockBackend::default()
        };
        let outcome = run(&cfg, &mock).expect("the run goes on past the unknown calls");
        let text = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        let log: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("an event")).collect();
        assert!(!text.contains("\"turn_error\""), "no turn erred: {mode}");
        let of = |event: &'static str| log.iter().filter(move |e| e["event"] == event);

        // Disjoint items: each taken once, never by the integrator; the change by its revised item's taker.
        let takes: Vec<(String, String, String)> = of("item_taken")
            .map(|e| {
                let s = |v: &Value| v.as_str().expect("a string").to_string();
                (s(&e["data"]["item"]), s(&e["agent"]), s(&e["data"]["by"]))
            })
            .collect();
        let mut items: Vec<&str> = takes.iter().map(|(i, _, _)| i.as_str()).collect();
        items.sort();
        let before = items.len();
        items.dedup();
        assert_eq!(items.len(), before, "{mode}: no item taken twice: {takes:?}");
        assert!(takes.iter().all(|(_, a, _)| a != "agent-0"), "{mode}: the integrator takes none: {takes:?}");
        let sum = takes.iter().find(|(i, _, _)| i == "sum").expect("sum is taken in turn 1");
        assert_eq!(sum.2, "agent");
        let decimals = takes.iter().find(|(i, _, _)| i == "decimals").expect("the change is handed on");
        assert_eq!((&decimals.1, decimals.2.as_str()), (&sum.1, "release"), "{mode}: {takes:?}");
        assert_eq!(of("item_given_up").count(), 0);

        // The integrator's last turn after every builder's last.
        let seq = |e: &Value| e["seq"].as_u64().expect("a seq");
        let last = of("turn_start").find(|e| e["agent"] == "agent-0" && e["data"]["turn"] == 2).expect("its turn 2");
        for e in of("turn_end").filter(|e| e["agent"] != "agent-0") {
            assert!(seq(e) < seq(last), "{mode}: {e} ended after the integrator's last turn started");
        }
        assert_eq!(of("last_turn_held").count(), 1, "{mode}");

        // Each integrator turn after every builder's same turn, and main holds the builders' turn-1 work from
        // checkpoint 1 (task 61911c91247f).
        for turn in [1, 2] {
            let start = of("turn_start").find(|e| e["agent"] == "agent-0" && e["data"]["turn"] == turn).expect("one");
            for e in of("turn_end").filter(|e| e["agent"] != "agent-0" && e["data"]["turn"] == turn) {
                assert!(seq(e) < seq(start), "{mode}: {e} ended after the integrator's turn {turn} started");
            }
        }
        let held: Vec<&Value> = of("turn_held").map(|e| &e["data"]["turn"]).collect();
        assert_eq!(held, [&json!(1)], "{mode}");
        let first = of("checkpoint").find(|e| e["data"]["turn"] == 1).expect("checkpoint 1");
        let at_first: Vec<&str> =
            first["data"]["modules"].as_array().expect("modules").iter().filter_map(|m| m[0].as_str()).collect();
        for agent in 1..4 {
            let module = format!("tiny/by_agent_{agent}_turn_1.py");
            assert!(at_first.contains(&module.as_str()), "{mode}: {module} on main at checkpoint 1: {at_first:?}");
        }

        // The unknown calls: counted, no halt, each named back once with the offered tools.
        assert_eq!(outcome.unknown_calls, 2, "{mode}");
        let counted: Vec<&Value> = of("unknown_calls").map(|e| &e["data"]["count"]).collect();
        assert_eq!(counted, [&json!(2)], "{mode}");
        let on_results: Vec<&Value> = of("unknown_hint").map(|e| &e["agent"]).collect();
        assert_eq!(on_results, [&json!("agent-1")], "{mode}: one hint on a result");
        let hinted = mock.hinted.lock().expect("the hints").clone();
        let whom: Vec<(usize, u32)> = hinted.iter().map(|(a, t, _)| (*a, *t)).collect();
        assert_eq!(whom, [(1, 1), (2, 2)], "{mode}: agent-1's on its result, agent-2's opening its turn 2");
        for (_, _, hint) in &hinted {
            assert!(hint.starts_with("`mcp__experiment__give_up_item` isn't among your tools"), "{mode}: {hint}");
            assert!(hint.contains("take_item") && hint.contains("notes_write"), "{mode}: names the tools: {hint}");
        }
        let plan: Value =
            serde_json::from_slice(&std::fs::read(cfg.root.join("plan.json")).expect("the plan")).expect("JSON");
        let rules: Vec<&str> = plan["rules"].as_array().expect("rules").iter().filter_map(|r| r[0].as_str()).collect();
        assert!(rules.ends_with(&["take-items", "integrator-last", "integrator-each-turn"]), "{mode}: {rules:?}");

        // Every agent's turn-2 module is on main at the end.
        let paths: Vec<&str> = outcome.modules.iter().map(|(p, _)| p.as_str()).collect();
        for agent in 0..4 {
            let module = format!("tiny/by_agent_{agent}_turn_2.py");
            assert!(paths.contains(&module.as_str()), "{mode}: {module} at the end: {paths:?}");
        }

        let grades = root.join("grades.jsonl");
        let summary =
            bracken_experiment::grade_run::grade_run(&cfg.root, &store, &grades, &bracken(), None).expect("graded");
        assert_eq!((summary.points, summary.errors), (3, 0), "{mode}: {summary:?}");
        let points: Vec<Value> = std::fs::read_to_string(&grades)
            .expect("the grades")
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect();
        let end = points.last().expect("the end");
        assert_eq!(end["at"], "end");
        let turn = end["grade"]["groups"].as_array().expect("groups").iter().find(|g| g["index"] == 3).expect("3");
        assert_eq!(turn["outcomes"], json!(["pass"]), "{mode}: the integrator's own last work is on main: {end}");
    }

    /// After each change to an agent's own main the driver runs the public tests on it, the same in both modes
    /// (task `b0a9769a48da`, findings `9798bc24d2f3`, `dbec448397a2`): `breaker` breaks the package's import with
    /// `line` in turn 1, a builder's work the integrator brings in or the integrator's own; the change that brings it
    /// says main is broken (`untested` if the tests can't report); integrating others' work is then refused until
    /// the integrator repairs main in its own tree, which the tests then show sound; and main ends without the break.
    fn a_broken_main_is_repaired_before_anything_else_is_integrated(
        mode: &'static str,
        breaker: usize,
        line: &'static str,
    ) {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, _) = project_and_store(&root);
        // Traced, so a turn's error is kept whole in turn-errors.txt.
        let cfg = RunConfig {
            trace: true,
            ..config_of(mode, "agent-2", python, root.join("run"), 3, 2, &project, false, false)
        };
        let mock = MockBackend {
            seed: 1,
            shim: Some(bin()),
            lockstep: true,
            integrator: Some(2),
            breaks: Some((breaker, 1)),
            break_line: line,
            ..MockBackend::default()
        };
        let outcome = run(&cfg, &mock).expect("the run");
        let text = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        let log: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("an event")).collect();
        let errors: Vec<&Value> = log.iter().filter(|e| e["event"] == "turn_error").collect();
        let kept = std::fs::read_to_string(cfg.root.join("turn-errors.txt")).unwrap_or_default();
        assert!(errors.is_empty(), "{mode}: no turn erred: {errors:?}\n{kept}");
        let tested: Vec<&Value> = log.iter().filter(|e| e["event"] == "main_tests").collect();
        assert!(tested.iter().all(|e| e["agent"] == "agent-2"), "{mode}: only the integrator integrates: {tested:?}");
        // The break's report: regressed tests, or untested where the import ends the runner. A run this machine's
        // load stops can report untested too, before or after; the driver's retest then clears it.
        let real = |e: &&&Value| {
            let t = &e["data"]["tests"];
            t["broken"] == true && if line == EXIT_LINE { t["untested"].is_string() } else { t["regressed"].is_array() }
        };
        let broke = tested.iter().position(|e| real(&e)).expect("main broke");
        assert!(
            tested[..broke]
                .iter()
                .all(|e| e["data"]["tests"]["broken"] == false || e["data"]["tests"]["untested"].is_string())
        );
        let repaired = tested[broke + 1..].iter().find(|e| e["data"]["tests"]["broken"] == false).expect("a repair");
        let refusals: Vec<&Value> = log.iter().filter(|e| e["event"] == "main_broken_refusal").collect();
        assert!(!refusals.is_empty(), "{mode}: integrating onto the broken main was refused");
        let seq = |e: &Value| e["seq"].as_u64().expect("a seq");
        assert!(
            refusals.iter().any(|r| seq(tested[broke]) < seq(r) && seq(r) < seq(repaired)),
            "{mode}: in that order"
        );
        // Every refusal comes while main's last report was broken.
        for r in &refusals {
            let last = tested.iter().rfind(|e| seq(e) < seq(r)).expect("a report before it");
            assert_eq!(last["data"]["tests"]["broken"], true, "{mode}: refused only while broken: {r}");
        }
        assert!(outcome.modules.iter().all(|(_, t)| !t.contains("broken on purpose")), "{mode}: main ends sound");
        let work = format!("by_agent_{breaker}_turn_1");
        assert!(outcome.modules.iter().any(|(p, _)| p.contains(&work)), "{mode}: the breaker's work stays");
    }

    /// A run as a real one runs, every worker sandboxed (gotcha `73ebbb818ea1`): in turn 1 both builders also
    /// append their own line to the package's `__init__.py`, so their work conflicts there, and the integrator
    /// settles it keeping both. In Weave through the three-way view: each conflict shows its three sides, take
    /// markers writes them, and take tree settles it (defect `cc1a19b1508e`: in stage 5 every markers call failed
    /// under the sandbox, which unsandboxed tests never saw); in git project mode through git's own markers. The
    /// mock errs, and the run logs a turn error, if a conflict shows no three sides or a resolve fails.
    fn a_sandboxed_conflict_is_settled_keeping_both_sides(mode: &'static str) {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, _) = project_and_store(&root);
        let cfg = RunConfig {
            trace: true,
            ..config_of(mode, "agent-2", python, root.join("run"), 3, 2, &project, false, true)
        };
        let mock = MockBackend {
            seed: 1,
            shim: Some(bin()),
            lockstep: true,
            integrator: Some(2),
            conflicts: Some(1),
            ..MockBackend::default()
        };
        let outcome = run(&cfg, &mock).expect("the run");
        let text = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        let log: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("an event")).collect();
        let kept = std::fs::read_to_string(cfg.root.join("turn-errors.txt")).unwrap_or_default();
        let errors: Vec<&Value> = log.iter().filter(|e| e["event"] == "turn_error").collect();
        assert!(errors.is_empty(), "{mode}: no turn erred: {errors:?}\n{kept}");
        assert!(!kept.contains("needs a git checkout"), "{mode}: {kept}");
        let init = outcome.modules.iter().find(|(p, _)| p.ends_with("/__init__.py")).expect("the package");
        for agent in [0, 1] {
            assert!(init.1.contains(&shared_line(agent)), "{mode}: main keeps agent {agent}'s line: {}", init.1);
        }
        assert!(!init.1.contains("<<<<<<<") && !init.1.contains(">>>>>>>"), "{mode}: no marker left: {}", init.1);
    }

    #[test]
    fn a_sandboxed_weave_conflict_is_settled_through_the_three_way_view() {
        a_sandboxed_conflict_is_settled_keeping_both_sides("weave");
    }

    #[test]
    fn a_sandboxed_git_project_conflict_is_settled_through_gits_markers() {
        a_sandboxed_conflict_is_settled_keeping_both_sides(bracken_experiment::git_project::NAME);
    }

    /// Task `33ac510f7d4c` (owner decision `817afe6653a8`): in turn 1 both builders append their own line to the
    /// package's `__init__.py`, and the integrator leaves the one that conflicts open instead of settling it. Its
    /// builder is told so on its next turn's first tool result, with the paths that conflicted, and brings its work
    /// up to date with main: in Weave it redoes it on the line's head and proposes again; in git project mode it
    /// merges origin/main into its branch, settles, commits and asks again. Main then holds both builders' lines and
    /// turn-1 modules. Without the notice the builder never learns of it, and its line and module never land. With
    /// `silent`, both builders make no tool call in turn 2: the notice waits, and comes in turn 3 (finding
    /// `0220fc3d2c7e`).
    fn a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date(
        mode: &'static str,
        sandbox: bool,
        silent: bool,
    ) {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, _) = project_and_store(&root);
        let cfg = RunConfig {
            trace: true,
            ..config_of(
                mode,
                "agent-2",
                python,
                root.join("run"),
                3,
                if silent { 4 } else { 3 },
                &project,
                false,
                sandbox,
            )
        };
        let mock = MockBackend {
            seed: 1,
            shim: Some(bin()),
            lockstep: true,
            integrator: Some(2),
            conflicts: Some(1),
            leaves_conflicts: true,
            silent: if silent { vec![(0, 2), (1, 2)] } else { Vec::new() },
            ..MockBackend::default()
        };
        let outcome = run(&cfg, &mock).expect("the run");
        let text = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        let log: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("an event")).collect();
        let kept = std::fs::read_to_string(cfg.root.join("turn-errors.txt")).unwrap_or_default();
        let errors: Vec<&Value> = log.iter().filter(|e| e["event"] == "turn_error").collect();
        assert!(errors.is_empty(), "{mode}: no turn erred: {errors:?}\n{kept}");

        // The builder whose work conflicted is told at the start of its next turn, and only builders are.
        let told: Vec<&Value> = log.iter().filter(|e| e["event"] == "conflict_notice").collect();
        assert!(!told.is_empty(), "{mode}: a builder was told its work conflicts");
        assert!(told.iter().all(|e| e["agent"] != "agent-2"), "{mode}: only builders are told: {told:?}");
        assert!(told.iter().all(|e| e["data"]["turn"].as_u64() > Some(1)), "{mode}: told on a later turn: {told:?}");
        notices_follow_the_integrators_turn(mode, &log);
        if silent {
            assert!(
                told.iter().all(|e| e["data"]["turn"] != 2),
                "{mode}: nothing told in a turn with no call: {told:?}"
            );
            assert!(told.iter().any(|e| e["data"]["turn"] == 3), "{mode}: told on the next turn with a call: {told:?}");
        }
        let hinted = mock.hinted.lock().expect("the hints").clone();
        let (what, verb) = if mode == "weave" {
            ("Your proposal ", "propose it again")
        } else {
            ("Your request ", "ask for its integration again")
        };
        let notices: Vec<&(usize, u32, String)> = hinted.iter().filter(|(_, _, h)| h.starts_with(what)).collect();
        assert!(!notices.is_empty(), "{mode}: the notice reached the builder: {hinted:?}");
        for (agent, turn, notice) in &notices {
            assert!(*agent != 2 && *turn > 1, "{mode}: {notice}");
            assert!(notice.contains("stopped at conflicts in ") && notice.contains("__init__.py"), "{mode}: {notice}");
            assert!(notice.contains("Bring it up to date with main") && notice.contains(verb), "{mode}: {notice}");
        }
        if mode == "weave" {
            assert!(
                notices.iter().all(|(_, _, n)| n.contains("The integrator's note: `agent 2's turn ")),
                "{notices:?}"
            );
        }

        // And it brought it up to date: main holds both builders' lines and turn-1 work, and no marker.
        let init = outcome.modules.iter().find(|(p, _)| p.ends_with("/__init__.py")).expect("the package");
        for agent in [0, 1] {
            assert!(init.1.contains(&shared_line(agent)), "{mode}: main keeps agent {agent}'s line: {}", init.1);
            let work = format!("by_agent_{agent}_turn_1");
            assert!(outcome.modules.iter().any(|(p, _)| p.contains(&work)), "{mode}: main holds {work}");
        }
        assert!(!init.1.contains("<<<<<<<") && !init.1.contains(">>>>>>>"), "{mode}: no marker left: {}", init.1);
    }

    /// Finding `61993dafb91e`: the integrator's conflicts are noted only once its turn is over (after its last tool
    /// call of that turn), and a builder is told only of work noted so.
    fn notices_follow_the_integrators_turn(mode: &str, log: &[Value]) {
        let seq = |e: &Value| e["seq"].as_u64().expect("a seq");
        let noted: Vec<&Value> = log.iter().filter(|e| e["event"] == "conflict_noted").collect();
        for n in &noted {
            let last_call = log
                .iter()
                .filter(|e| {
                    e["event"] == "tool_call" && e["agent"] == "agent-2" && e["data"]["turn"] == n["data"]["turn"]
                })
                .map(seq)
                .max()
                .expect("the integrator's calls");
            assert!(seq(n) > last_call, "{mode}: noted only at the integrator's turn's end: {n}");
        }
        // Finding `a71c41f4c289`: a notice comes in a turn that started (and synced) after its work was noted.
        for t in log.iter().filter(|e| e["event"] == "conflict_notice") {
            let started = log
                .iter()
                .find(|e| {
                    e["event"] == "turn_start" && e["agent"] == t["agent"] && e["data"]["turn"] == t["data"]["turn"]
                })
                .expect("the turn's start");
            assert!(
                noted.iter().any(|n| n["data"]["for"] == t["agent"] && seq(n) < seq(started)),
                "{mode}: told only of work noted before its turn started: {t}"
            );
        }
    }

    /// Findings `61993dafb91e`, `a71c41f4c289` and `2d5c6f3e6a67`, in a concurrent run (no lockstep), where
    /// builders play their next turns while the integrator plays. With `leaves`, the integrator leaves a builder's
    /// conflicting work open: the builders are paced ([`MockBackend::paced_by`]) so a turn of theirs starts after
    /// the integrator's turn 1 ends, and the builder whose work conflicted is told then, not before, and brings it up
    /// to date. Without, the integrator settles every conflict in the turn it meets it, so no builder is ever told,
    /// though its first try at the work stopped at conflicts. Git project mode only: publishing notices is the same
    /// code in both modes, and only reading Weave's conflict error differs, which a unit test covers (security review
    /// `0ccccb53a79d`).
    fn a_concurrent_runs_notices_wait_for_the_integrators_turn(mode: &'static str, leaves: bool) {
        let Some(python) = pin() else { return };
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = std::fs::canonicalize(tmp.path()).expect("resolved");
        let (project, _) = project_and_store(&root);
        let cfg = RunConfig {
            trace: true,
            ..config_of(mode, "agent-2", python, root.join("run"), 3, 3, &project, false, false)
        };
        let mock = MockBackend {
            seed: 1,
            shim: Some(bin()),
            lockstep: false,
            integrator: Some(2),
            conflicts: Some(1),
            leaves_conflicts: leaves,
            paced_by: leaves.then(|| cfg.root.join("events.jsonl")),
            ..MockBackend::default()
        };
        let outcome = run(&cfg, &mock).expect("the run");
        let text = std::fs::read_to_string(cfg.root.join("events.jsonl")).expect("the events");
        let log: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("an event")).collect();
        let kept = std::fs::read_to_string(cfg.root.join("turn-errors.txt")).unwrap_or_default();
        let errors: Vec<&Value> = log.iter().filter(|e| e["event"] == "turn_error").collect();
        assert!(errors.is_empty(), "{mode}: no turn erred: {errors:?}\n{kept}");
        notices_follow_the_integrators_turn(mode, &log);
        let noted: Vec<&Value> = log.iter().filter(|e| e["event"] == "conflict_noted").collect();
        let told: Vec<&Value> = log.iter().filter(|e| e["event"] == "conflict_notice").collect();
        let init = outcome.modules.iter().find(|(p, _)| p.ends_with("/__init__.py")).expect("the package");
        if leaves {
            // The integrator's turn 1 holds for both builders' turn 1, so it meets the conflict and leaves it.
            assert!(noted.iter().any(|e| e["data"]["turn"] == 1), "{mode}: the conflict was noted: {noted:?}");
            assert!(!told.is_empty(), "{mode}: a builder was told");
            let work = |agent: usize| format!("by_agent_{agent}_turn_1");
            for agent in [0, 1] {
                assert!(init.1.contains(&shared_line(agent)), "{mode}: main keeps agent {agent}'s line: {}", init.1);
                assert!(
                    outcome.modules.iter().any(|(p, _)| p.contains(&work(agent))),
                    "{mode}: main holds {}",
                    work(agent)
                );
            }
        } else {
            assert!(
                noted.is_empty() && told.is_empty(),
                "{mode}: nothing told of settled conflicts: {noted:?} {told:?}"
            );
            for agent in [0, 1] {
                assert!(init.1.contains(&shared_line(agent)), "{mode}: main keeps agent {agent}'s line: {}", init.1);
            }
        }
    }

    #[test]
    fn a_concurrent_git_project_run_tells_nothing_of_conflicts_the_integrator_settles() {
        a_concurrent_runs_notices_wait_for_the_integrators_turn(bracken_experiment::git_project::NAME, false);
    }

    #[test]
    fn a_concurrent_git_project_runs_notices_wait_for_the_integrators_turn() {
        a_concurrent_runs_notices_wait_for_the_integrators_turn(bracken_experiment::git_project::NAME, true);
    }

    #[test]
    fn a_weave_builder_whose_proposal_conflicts_is_told_and_proposes_again() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date("weave", false, false);
    }

    #[test]
    fn a_git_project_builder_whose_request_conflicts_is_told_and_asks_again() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date(
            bracken_experiment::git_project::NAME,
            false,
            false,
        );
    }

    #[test]
    fn a_weave_notice_waits_out_a_turn_with_no_tool_call() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date("weave", false, true);
    }

    #[test]
    fn a_git_project_notice_waits_out_a_turn_with_no_tool_call() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date(
            bracken_experiment::git_project::NAME,
            false,
            true,
        );
    }

    #[test]
    fn a_sandboxed_weave_builder_whose_proposal_conflicts_is_told_and_proposes_again() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date("weave", true, false);
    }

    #[test]
    fn a_sandboxed_git_project_builder_whose_request_conflicts_is_told_and_asks_again() {
        a_builder_whose_work_conflicts_is_told_and_brings_it_up_to_date(
            bracken_experiment::git_project::NAME,
            true,
            false,
        );
    }

    #[test]
    fn a_weave_integrator_repairs_a_broken_main_first() {
        a_broken_main_is_repaired_before_anything_else_is_integrated("weave", 0, BREAK_LINE);
    }

    #[test]
    fn a_git_project_integrator_repairs_a_broken_main_first() {
        a_broken_main_is_repaired_before_anything_else_is_integrated(
            bracken_experiment::git_project::NAME,
            0,
            BREAK_LINE,
        );
    }

    /// An import that ends the test runner leaves main untested, which blocks as broken does (finding 9798bc24d2f3).
    #[test]
    fn a_weave_main_the_tests_cant_report_on_is_repaired_first() {
        a_broken_main_is_repaired_before_anything_else_is_integrated("weave", 0, EXIT_LINE);
    }

    #[test]
    fn a_git_project_main_the_tests_cant_report_on_is_repaired_first() {
        a_broken_main_is_repaired_before_anything_else_is_integrated(
            bracken_experiment::git_project::NAME,
            0,
            EXIT_LINE,
        );
    }

    /// The integrator's own change to main is tested as any other is, in both modes (finding dbec448397a2): its own
    /// proposal in Weave, its own commit on main in git.
    #[test]
    fn a_weave_integrators_own_break_is_caught() {
        a_broken_main_is_repaired_before_anything_else_is_integrated("weave", 2, BREAK_LINE);
    }

    #[test]
    fn a_git_project_integrators_own_break_is_caught() {
        a_broken_main_is_repaired_before_anything_else_is_integrated(
            bracken_experiment::git_project::NAME,
            2,
            BREAK_LINE,
        );
    }

    #[test]
    fn a_weave_run_divides_the_items_and_its_integrator_plays_last() {
        a_run_divides_the_items_and_its_integrator_plays_last("weave");
    }

    #[test]
    fn a_git_project_run_divides_the_items_and_its_integrator_plays_last() {
        a_run_divides_the_items_and_its_integrator_plays_last(bracken_experiment::git_project::NAME);
    }

    /// What a real run's harness allows (`harness_tools`, which `agent_run` gives Pi and Claude Code) is exactly
    /// what the shim advertises in a project run, in both modes, `take_item` included (finding `8fb431e50a6b`: at
    /// d7f8e0a3 the allow-list left it out, so a real run halted on the first `take_item`).
    #[test]
    fn a_project_runs_harness_allows_exactly_the_tools_the_shim_advertises() {
        use bracken_experiment::driver::{Backend, harness_tools};
        use bracken_experiment::mock::Client;
        use std::sync::Mutex;

        /// Asks `tools/list` through the shim, as a harness would, and keeps the names.
        struct Lister(Mutex<Vec<Vec<String>>>);
        impl Backend for Lister {
            fn turn(&self, _: usize, _: &str, _: u32, socket: &Path) -> Result<(), String> {
                let mut c = Client::connect(socket, Some(&bin()))?;
                let listed = c.request("tools/list", json!({}))?;
                let names = listed["tools"].as_array().ok_or("no tools")?.iter();
                let names = names.filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
                self.0.lock().expect("the list").push(names);
                Ok(())
            }
            fn lockstep(&self) -> bool {
                true
            }
        }

        let Some(python) = pin() else { return };
        for mode in ["weave", bracken_experiment::git_project::NAME] {
            let tmp = tempfile::tempdir().expect("a temp dir");
            let root = std::fs::canonicalize(tmp.path()).expect("resolved");
            let (project, _) = project_and_store(&root);
            let cfg = config_of(mode, "agent-1", python.clone(), root.join("run"), 2, 1, &project, false, false);
            let lister = Lister(Mutex::new(Vec::new()));
            run(&cfg, &lister).expect("the run");
            let m = bracken_experiment::mode::mode_named(mode).expect("the mode");
            let allowed = harness_tools(m, cfg.plan.task, true);
            assert!(allowed.iter().any(|t| t == "take_item"), "{mode}: {allowed:?}");
            let listed = lister.0.into_inner().expect("the list");
            assert_eq!(listed.len(), 2, "{mode}: one turn each");
            for names in listed {
                assert_eq!(names, allowed, "{mode}: the shim advertises what the harness allows");
            }
        }
    }
}
