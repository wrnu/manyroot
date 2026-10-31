//! Weave mode (task `e4970250b3e4`): agents build an evaluation project's tree through Weave. The stores
//! through their workers, as a run drives them; a project run's refusals; and, on macOS, a mock run of the
//! tiny fixture project end to end, every worker sandboxed, with the public tests run through the driver.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use bracken_experiment::driver::git_binary;
use bracken_experiment::mode::{Session, StoreConfig};
use bracken_experiment::project::Project;
use bracken_experiment::task::Task;
use bracken_experiment::weave_mode::{ANY_AGENT, hub_data};
use bracken_experiment::worker::{Request, Worker};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn bracken() -> PathBuf {
    let b = bin().with_file_name("bracken");
    assert!(b.exists(), "build bracken first: cargo build -p bracken");
    b
}

fn tiny() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny")
}

fn request(op: &str, args: Value) -> Request {
    Request { op: op.into(), args, ..Request::default() }
}

/// A hub and `names` copies of the tiny project, each set up as a run sets them up, with the policy naming
/// every copy an integrator.
struct Stores {
    _dir: tempfile::TempDir,
    sockets: PathBuf,
    hub: Worker,
    copies: Vec<Worker>,
    peers: Vec<String>,
    /// Each copy's working tree.
    trees: Vec<PathBuf>,
}

impl Stores {
    fn new(names: &[&str]) -> Stores {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = |sub: &str, identity: &str| StoreConfig {
            dir: dir.path().join(sub),
            bracken: bracken(),
            git: git_binary().expect("git"),
            identity: identity.into(),
            trace: false,
            task: Task::Phase1,
            accept_rule: Default::default(),
        };
        let sockets = dir.path().join("s");
        std::fs::create_dir(&sockets).expect("sockets");
        let project = Project::load(&tiny()).expect("the fixture");
        let mut hub = Worker::spawn(&bin(), "weave", &config("hub", "setter")).expect("hub");
        hub.ok(&request("project", hub_data(&project, ANY_AGENT).expect("the data"))).expect("the project");
        let setup = hub.ok(&request("setup", Value::Null)).expect("hub setup");
        assert_eq!(setup.result["released"], json!(["sum", "mean-empty"]), "the items released at the start");
        let seed = Worker::bytes(&hub.ok(&request("export", Value::Null)).expect("export")).expect("bytes");
        let (mut copies, mut peers, mut trees) = (Vec::new(), Vec::new(), Vec::new());
        for name in names {
            let mut w = Worker::spawn(&bin(), "weave", &config(name, name)).expect("copy");
            let reply = w
                .ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })
                .expect("copy setup");
            peers.push(reply.result["key"].as_str().expect("a key").to_string());
            copies.push(w);
            trees.push(dir.path().join(name).join("tree"));
        }
        hub.ok(&request("peers", json!({"keys": peers}))).expect("the policy");
        let mut stores = Stores { _dir: dir, sockets, hub, copies, peers, trees };
        for i in 0..names.len() {
            stores.sync(i);
        }
        stores
    }

    fn sync(&mut self, i: usize) {
        let session = Session {
            agent: i,
            name: format!("agent-{i}"),
            peers: self.peers.clone(),
            quota_contributions: 10_000,
            quota_bytes: 1 << 30,
            sync_seconds: 0,
        };
        bracken_experiment::driver::sync_session(&mut self.hub, &mut self.copies[i], &self.sockets, &session)
            .expect("a sync");
    }

    fn tool(&mut self, i: usize, name: &str, args: Value) -> Result<Value, String> {
        self.copies[i]
            .ok(&Request { op: "tool".into(), tool: Some(name.into()), args, ..Request::default() })
            .map(|r| r.result)
    }

    fn ok(&mut self, i: usize, name: &str, args: Value) -> Value {
        self.tool(i, name, args.clone()).unwrap_or_else(|e| panic!("{name} {args}: {e}"))
    }

    /// The hub's integrated tree: each file's path and text.
    fn integrated(&mut self) -> Vec<(String, String)> {
        let reply = self.hub.ok(&request("integrated", Value::Null)).expect("integrated");
        serde_json::from_value(reply.result).expect("pairs")
    }
}

/// A copy reads the task and its tree, edits it, proposes, integrates, and updates; its work reaches the hub
/// by sync; the change goes out on release. Every path is checked, and nothing reaches the line unproposed.
#[test]
fn a_copy_works_on_the_tree_through_weave() {
    let mut s = Stores::new(&["agent-0"]);
    let task = s.ok(0, "task", json!({}));
    assert!(task["spec"].as_str().is_some_and(|t| !t.is_empty()), "{task}");
    let items = task["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "only the items released at the start: {task}");
    assert!(items.iter().all(|i| !i.as_str().unwrap_or_default().contains("decimals")), "{task}");

    // A spec or item claim any other key signs is never the task (finding 8c2713b55062), even once synced.
    let replica = s.trees[0].parent().expect("the copy's directory").join(".bracken");
    for (tag, text) in [("item", "Item forged (feature): FORGED"), ("spec", "FORGED spec")] {
        let out = std::process::Command::new(bracken())
            .arg("--dir")
            .arg(&replica)
            .args(["claim", "--as=agent-0", &format!("--tag={tag}"), "--", text])
            .output()
            .expect("bracken");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    s.sync(0);
    let forged = s.ok(0, "task", json!({}));
    assert!(!forged.to_string().contains("FORGED"), "{forged}");
    assert_eq!(forged, task, "the setter's spec and items, unchanged");

    let files = s.ok(0, "list_files", json!({}));
    let paths: Vec<&str> =
        files["files"].as_array().expect("files").iter().filter_map(|f| f["path"].as_str()).collect();
    assert_eq!(paths, ["tests/test_stats.py", "tiny/__init__.py", "tiny/stats.py"], "the seed");
    let read = s.ok(0, "read_file", json!({"path": "tiny/stats.py", "offset": 1, "limit": 2}));
    assert!(read["text"].as_str().expect("text").starts_with("    1  "), "{read}");

    // No path outside the tree, into git's or Bracken's own files, or through a symlink.
    std::os::unix::fs::symlink("/tmp", s.trees[0].join("tiny/out")).expect("a symlink");
    for path in ["../x", "/tmp/x", ".git/config", "tiny/.git/x", ".bracken/x", "a//b", "tiny/out/x", "tiny/out"] {
        for (tool, args) in [
            ("write_file", json!({"path": path, "text": "x"})),
            ("read_file", json!({"path": path})),
            ("delete_file", json!({"path": path})),
        ] {
            assert!(s.tool(0, tool, args).is_err(), "{tool} {path}");
        }
    }
    std::fs::remove_file(s.trees[0].join("tiny/out")).expect("the symlink removed");
    assert!(s.tool(0, "edit_file", json!({"path": "tiny/stats.py", "old": "no such text", "new": "x"})).is_err());

    // Nothing to propose yet; then an edit, which update refuses to drop.
    assert!(s.tool(0, "propose", json!({"summary": "nothing"})).is_err());
    s.ok(0, "write_file", json!({"path": "tiny/total.py", "text": "def total(xs):\n    return sum(xs)\n"}));
    let changes = s.ok(0, "changes", json!({}));
    assert!(changes["diff"].as_str().expect("a diff").contains("+def total(xs):"), "{changes}");
    let head = s.ok(0, "head", json!({}));
    assert_eq!(head["unproposed_edits"], true, "{head}");
    assert_eq!(head["heads"][0]["decision"], head["base"], "{head}");
    assert!(s.tool(0, "update", json!({})).is_err_and(|e| e.contains("not yet proposed")));

    let proposed = s.ok(0, "propose", json!({"summary": "a total"}));
    let proposal = proposed["proposal"].as_str().expect("a proposal").to_string();
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false);
    let open = s.ok(0, "proposals", json!({}));
    assert_eq!(open["proposals"][0]["proposal"], proposal.as_str(), "{open}");
    let integrated = s.ok(0, "integrate", json!({"proposals": [&proposal[..12]], "note": "a total"}));
    let decision = integrated["decision"].as_str().expect("a decision").to_string();
    let updated = s.ok(0, "update", json!({}));
    assert_eq!(updated["base"], decision.as_str());

    // The hub has it once the copy syncs.
    assert!(!s.integrated().iter().any(|(p, _)| p == "tiny/total.py"), "not before the sync");
    s.sync(0);
    let tree = s.integrated();
    assert!(tree.iter().any(|(p, t)| p == "tiny/total.py" && t.contains("return sum(xs)")), "{tree:?}");

    // The change goes out on its release, and the task shows it after a sync.
    let released = s.hub.ok(&request("release", json!({"wave": 1}))).expect("the release");
    assert_eq!(released.result["released"], json!(["decimals"]));
    assert!(s.hub.ok(&request("release", json!({"wave": 2}))).is_err(), "one release after the start");
    s.sync(0);
    let items = s.ok(0, "task", json!({}))["items"].as_array().expect("items").len();
    assert_eq!(items, 3);

    // What the driver runs the public tests on: the working tree, without git's files.
    let tree = s.copies[0].ok(&request("tree", Value::Null)).expect("the tree");
    let names: Vec<&str> = tree.result.as_array().expect("files").iter().filter_map(|f| f[0].as_str()).collect();
    assert_eq!(names, ["tests/test_stats.py", "tiny/__init__.py", "tiny/stats.py", "tiny/total.py"]);
}

/// Two agents' proposals that edit the same line from the same decision: the second's merge conflicts and
/// writes nothing, until a resolution settles it.
#[test]
fn a_conflicting_proposal_waits_for_a_resolution() {
    let mut s = Stores::new(&["agent-0", "agent-1"]);
    let stats = std::fs::read_to_string(s.trees[0].join("tiny/stats.py")).expect("the seed's stats");
    let line = stats.lines().find(|l| l.contains("return")).expect("a return line").to_string();
    let mut proposals = Vec::new();
    for i in 0..2 {
        let new = format!("{line}  # agent {i}");
        s.ok(i, "edit_file", json!({"path": "tiny/stats.py", "old": line, "new": new}));
        let p = s.ok(i, "propose", json!({"summary": format!("agent {i}'s edit")}));
        proposals.push(p["proposal"].as_str().expect("a proposal").to_string());
        // What it proposed still shows against the decision it started from (finding 24bae8fdb48a).
        let changes = s.ok(i, "changes", json!({}));
        assert!(changes["diff"].as_str().expect("a diff").contains(&format!("# agent {i}")), "{changes}");
        s.sync(i);
    }
    s.sync(0);
    let first = s.ok(0, "integrate", json!({"proposals": [proposals[0]], "note": "the first"}));
    assert!(first["decision"].is_string());
    let second = s.tool(0, "integrate", json!({"proposals": [proposals[1]], "note": "the second"}));
    assert!(second.is_err(), "the second edits the same line: {second:?}");
    assert_eq!(
        s.ok(0, "head", json!({}))["heads"].as_array().expect("heads").len(),
        1,
        "the refused merge wrote nothing"
    );

    for bad in [
        json!({"proposals": [], "path": "tiny/stats.py", "take": "from"}),
        json!({"proposals": [proposals[1]], "path": "../x", "take": "from"}),
        json!({"proposals": [proposals[1]], "path": "tiny/stats.py", "take": "mine"}),
    ] {
        assert!(s.tool(0, "resolve", bad.clone()).is_err(), "{bad}");
    }
    s.ok(0, "resolve", json!({"proposals": [proposals[1]], "path": "tiny/stats.py", "take": "from"}));
    let second = s.ok(0, "integrate", json!({"proposals": [proposals[1]], "note": "the second, resolved"}));
    assert!(second["decision"].is_string(), "{second}");
    s.sync(0);
    let tree = s.integrated();
    let stats = &tree.iter().find(|(p, _)| p == "tiny/stats.py").expect("stats").1;
    assert!(stats.contains("# agent 1") && !stats.contains("# agent 0"), "the proposal's side: {stats}");
}

/// Two integrations that meet fork the line. Recording a tree is refused on a forked line until it is the
/// merge of every head, and for a tree that didn't start from the line's head: either would drop work
/// (finding 9f4e48c4c547). The merge keeps both heads' files, leaves the file both changed marked until it is
/// settled, and the settled tree keeps everything integrated.
#[test]
fn a_fork_is_settled_by_a_merge_that_keeps_both_heads() {
    let mut s = Stores::new(&["agent-0", "agent-1"]);
    let stats = std::fs::read_to_string(s.trees[0].join("tiny/stats.py")).expect("the seed's stats");
    let line = stats.lines().find(|l| l.contains("return")).expect("a return line").to_string();
    for (i, name) in [(0, "zero"), (1, "one")] {
        s.ok(i, "write_file", json!({"path": format!("tiny/{name}.py"), "text": format!("NAME = '{name}'\n")}));
        s.ok(i, "edit_file", json!({"path": "tiny/stats.py", "old": line, "new": format!("{line}  # {name}")}));
        let own = s.ok(i, "integrate", json!({"note": format!("agent {i}'s tree")}));
        assert_eq!(own["parents"].as_array().expect("parents").len(), 1, "{own}");
    }
    for i in [0, 1, 0, 1] {
        s.sync(i);
    }
    let heads = s.ok(0, "head", json!({}));
    assert_eq!(heads["forked"], true, "{heads}");

    // Neither head's tree may settle the fork by itself.
    s.ok(0, "write_file", json!({"path": "tiny/extra.py", "text": "X = 1\n"}));
    let err = s.tool(0, "integrate", json!({"note": "one side"})).expect_err("forked, no merge");
    assert!(err.contains("merge"), "{err}");
    assert!(s.tool(0, "merge", json!({})).is_err_and(|e| e.contains("not yet proposed")));
    s.ok(0, "delete_file", json!({"path": "tiny/extra.py"}));

    let merged = s.ok(0, "merge", json!({}));
    assert_eq!(merged["conflicts"], json!(["tiny/stats.py"]), "{merged}");
    for name in ["zero", "one"] {
        assert!(s.trees[0].join(format!("tiny/{name}.py")).exists(), "{name}'s file is in the merge");
    }
    let marked = std::fs::read_to_string(s.trees[0].join("tiny/stats.py")).expect("stats");
    assert!(marked.contains("<<<<<<<") && marked.contains("# zero") && marked.contains("# one"), "{marked}");
    let err = s.tool(0, "integrate", json!({"note": "still marked"})).expect_err("markers left");
    assert!(err.contains("conflict markers"), "{err}");
    s.ok(
        0,
        "write_file",
        json!({"path": "tiny/stats.py", "text": stats.replace(&line, &format!("{line}  # zero and one"))}),
    );
    let settled = s.ok(0, "integrate", json!({"note": "both heads, settled"}));
    assert_eq!(settled["parents"].as_array().expect("parents").len(), 2, "{settled}");
    s.sync(0);
    let tree = s.integrated();
    let paths: Vec<&str> = tree.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"tiny/zero.py") && paths.contains(&"tiny/one.py"), "both heads' work: {paths:?}");
    assert!(tree.iter().any(|(p, t)| p == "tiny/stats.py" && t.contains("# zero and one")), "{tree:?}");
    let line_head = s.hub.ok(&request("line", Value::Null)).expect("the line").result;
    assert_eq!(line_head["forked"], false, "{line_head}");
    assert_eq!(line_head["heads"][0]["decision"], settled["decision"], "{line_head}");
    assert!(line_head["heads"][0]["manifest"].as_str().is_some_and(|m| m.starts_with("blake3:")), "{line_head}");

    // Agent 1's tree started from its own head, no longer the line's: recording it is refused, and its work
    // reaches the line by a proposal, merged, with nothing lost.
    s.sync(1);
    s.ok(1, "write_file", json!({"path": "tiny/late.py", "text": "LATE = True\n"}));
    let err = s.tool(1, "integrate", json!({"note": "stale"})).expect_err("a stale base");
    assert!(err.contains("not the line's head"), "{err}");
    let p = s.ok(1, "propose", json!({"summary": "late"}));
    s.ok(1, "integrate", json!({"proposals": [p["proposal"]], "note": "late, merged"}));
    s.sync(1);
    let paths: Vec<String> = s.integrated().into_iter().map(|(p, _)| p).collect();
    for f in ["tiny/zero.py", "tiny/one.py", "tiny/late.py"] {
        assert!(paths.iter().any(|p| p == f), "{f}: {paths:?}");
    }
}

/// A project run is Weave mode's, and needs the Python pin; Weave mode needs a project. Each is refused before
/// the run directory is made.
#[test]
fn a_project_run_needs_weave_mode_and_the_python_pin() {
    use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, run};
    use bracken_experiment::mock::MockBackend;
    use bracken_experiment::plan::{PlanInputs, ProjectPlan};
    let dir = tempfile::tempdir().expect("a temp dir");
    let project = ProjectPlan::of(&Project::load(&tiny()).expect("the fixture")).expect("its plan");
    let cfg = |mode: &'static str, plan: PlanInputs| RunConfig {
        root: dir.path().join("run"),
        mode,
        agents: 2,
        turns: 2,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan,
        sandbox: false,
    };
    let mock = MockBackend { lockstep: true, ..MockBackend::default() };
    let with_project = PlanInputs { project: Some(project.clone()), ..PlanInputs::default() };
    for (mode, plan, why) in [
        ("weave", PlanInputs::default(), "needs a project"),
        ("weave", with_project.clone(), "needs the Python pin"),
        ("bracken", with_project, "takes no project"),
    ] {
        let err = run(&cfg(mode, plan), &mock).err().expect("refused");
        assert!(err.contains(why), "{mode}: {err}");
        assert!(!dir.path().join("run").exists(), "{mode}: no run directory");
    }
}

#[cfg(target_os = "macos")]
mod macos_only {
    use super::*;
    use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, run};
    use bracken_experiment::mock::MockBackend;
    use bracken_experiment::plan::{PlanInputs, ProjectPlan};
    use bracken_experiment::python::Python;

    fn events(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).expect("the event log");
        text.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect()
    }

    fn pin() -> bracken_experiment::python::PythonPin {
        let path = std::env::var_os("BRACKEN_TEST_PYTHON").unwrap_or_else(|| "/opt/homebrew/bin/python3.12".into());
        assert!(Path::new(&path).exists(), "this test needs Python 3.12 at {path:?} (or set BRACKEN_TEST_PYTHON)");
        Python::pin(Path::new(&path), "3.12", None).expect("pinned").plan_pin()
    }

    /// Agents playing at once, not in lockstep: every agent integrates its first turn before it syncs, so the
    /// line forks, and later turns settle forks with merges. Every decision with more than one parent, in the
    /// hub's own log, keeps every file of each of its parents (finding 9f4e48c4c547): no settled fork loses a
    /// head's work. The mock writes only new files, so a parent's file missing from a merge is lost work.
    #[test]
    fn concurrent_agents_fork_the_line_and_every_merge_keeps_both_heads() {
        let project = ProjectPlan::of(&Project::load(&tiny()).expect("the fixture")).expect("its plan");
        let dir = tempfile::tempdir().expect("a temp dir");
        let cfg = RunConfig {
            root: dir.path().join("run"),
            mode: "weave",
            agents: 3,
            turns: 3,
            worker_bin: bin(),
            bracken: bracken(),
            git: git_binary().expect("git"),
            trace: false,
            calls_per_turn: CALLS_PER_TURN,
            turn_limit: TURN_LIMIT,
            plan: PlanInputs {
                project: Some(project),
                python: Some(pin()),
                integrator: Some(ANY_AGENT.into()),
                ..PlanInputs::default()
            },
            sandbox: false,
        };
        let mock = MockBackend { seed: 1, shim: Some(bin()), lockstep: false, ..MockBackend::default() };
        run(&cfg, &mock).expect("the run");
        let log = events(&cfg.root.join("events.jsonl"));
        assert!(
            log.iter().all(|e| e["event"] != "turn_error"),
            "no turn erred: {:?}",
            log.iter().filter(|e| e["event"] == "turn_error").collect::<Vec<_>>()
        );
        let failed: Vec<&Value> = log
            .iter()
            .filter(|e| e["event"] == "tool_result" && e["data"]["ok"] != true && e["data"]["tool"] != "`integrate`")
            .collect();
        assert!(failed.is_empty(), "only an integration may be refused: {failed:?}");
        let merges = log.iter().filter(|e| e["event"] == "tool_result" && e["data"]["tool"] == "`merge`").count();
        assert!(merges > 0, "the agents' integrations met, and were merged");

        // The hub's log, every decision's files.
        let hub = cfg.root.join("hub/.bracken");
        let weave = |args: &[&str]| {
            let out = std::process::Command::new(bracken())
                .arg("--dir")
                .arg(&hub)
                .arg("weave")
                .args(args)
                .output()
                .expect("bracken");
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            out.stdout
        };
        let log: Value = serde_json::from_slice(&weave(&["log", "--json", "--limit=1000"])).expect("the log");
        let views = tempfile::tempdir().expect("a temp dir");
        let mut files = std::collections::BTreeMap::new();
        for (k, d) in log["decisions"].as_array().expect("decisions").iter().enumerate() {
            let view = views.path().join(k.to_string());
            std::fs::create_dir(&view).expect("a view");
            weave(&[
                "checkout",
                &format!("--manifest={}", d["manifest"].as_str().expect("a manifest")),
                view.to_str().expect("UTF-8"),
            ]);
            let (mut paths, mut todo) = (std::collections::BTreeSet::new(), vec![view.clone()]);
            while let Some(dir) = todo.pop() {
                for e in std::fs::read_dir(&dir).expect("a directory") {
                    let e = e.expect("an entry");
                    if e.file_type().expect("a type").is_dir() {
                        todo.push(e.path());
                    } else {
                        paths.insert(e.path().strip_prefix(&view).expect("in the view").display().to_string());
                    }
                }
            }
            files.insert(d["decision"].as_str().expect("an id").to_string(), (d["parents"].clone(), paths));
        }
        let mut settled = 0;
        for (id, (parents, paths)) in &files {
            let parents: Vec<&str> = parents.as_array().expect("parents").iter().filter_map(Value::as_str).collect();
            if parents.len() < 2 {
                continue;
            }
            settled += 1;
            for p in parents {
                let lost: Vec<&String> = files[p].1.iter().filter(|f| !paths.contains(*f)).collect();
                assert!(lost.is_empty(), "merge {id} lost {lost:?} of its parent {p}");
            }
        }
        assert!(settled > 0, "a fork was settled");
        let all: std::collections::BTreeSet<&String> = files.values().flat_map(|(_, p)| p).collect();
        for agent in 0..3 {
            assert!(all.iter().any(|p| p.contains(&format!("by_agent_{agent}_"))), "agent {agent}'s work: {all:?}");
        }
    }

    /// The tiny project, 2 agents x 3 turns, every worker sandboxed as in a real run: the seed is the first
    /// tree, the change goes out once half the agent-turns are used, a checkpoint follows every turn, the
    /// public tests run on each agent's tree through the driver, and every agent's module reaches the line.
    #[test]
    fn a_weave_mock_run_goes_end_to_end_sandboxed() {
        let path = std::env::var_os("BRACKEN_TEST_PYTHON").unwrap_or_else(|| "/opt/homebrew/bin/python3.12".into());
        assert!(Path::new(&path).exists(), "this test needs Python 3.12 at {path:?} (or set BRACKEN_TEST_PYTHON)");
        let pin = Python::pin(Path::new(&path), "3.12", None).expect("pinned").plan_pin();
        let project = ProjectPlan::of(&Project::load(&tiny()).expect("the fixture")).expect("its plan");
        let dir = tempfile::tempdir().expect("a temp dir");
        let cfg = RunConfig {
            root: dir.path().join("run"),
            mode: "weave",
            agents: 2,
            turns: 3,
            worker_bin: bin(),
            bracken: bracken(),
            git: git_binary().expect("git"),
            trace: false,
            calls_per_turn: CALLS_PER_TURN,
            turn_limit: TURN_LIMIT,
            plan: PlanInputs {
                project: Some(project),
                python: Some(pin),
                integrator: Some(ANY_AGENT.into()),
                ..PlanInputs::default()
            },
            sandbox: true,
        };
        let mock = MockBackend { seed: 1, shim: Some(bin()), lockstep: true, ..MockBackend::default() };
        let outcome = run(&cfg, &mock).expect("the run");
        let log = events(&outcome.events);
        let failed: Vec<&Value> =
            log.iter().filter(|e| e["event"] == "tool_result" && e["data"]["ok"] != true).collect();
        assert!(failed.is_empty(), "every call succeeded: {failed:?}");
        assert!(log.iter().all(|e| e["event"] != "turn_error"), "no turn erred");
        let tests = log.iter().filter(|e| e["event"] == "tool_result" && e["data"]["tool"] == "`run_tests`").count();
        assert_eq!(tests, 6, "run_tests once each agent-turn");

        // The change, by itself, once 3 of the 6 agent-turns are used: no wave in the record.
        let released: Vec<&Value> = log.iter().filter(|e| e["event"] == "changes_released").collect();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0]["data"], json!({"turns_done": 3, "share": 0.5, "released": ["decimals"]}));
        let checkpoints: Vec<&Value> = log.iter().filter(|e| e["event"] == "checkpoint").collect();
        assert_eq!(checkpoints.len(), 3, "one after every turn of all agents");
        // Each tree logged names the line's head, by decision and manifest (finding 01e0ce2648ad).
        for e in
            log.iter().filter(|e| ["checkpoint", "snapshot", "integrated"].contains(&e["event"].as_str().unwrap_or("")))
        {
            let line = &e["data"]["line"];
            assert!(line["forked"].is_boolean(), "{e}");
            assert!(line["heads"][0]["decision"].is_string(), "{e}");
            assert!(line["heads"][0]["manifest"].as_str().is_some_and(|m| m.starts_with("blake3:")), "{e}");
        }

        // The seed and both agents' modules of every turn are on the line.
        let paths: Vec<&str> = outcome.modules.iter().map(|(p, _)| p.as_str()).collect();
        for agent in 0..2 {
            for turn in 1..=3 {
                let module = format!("tiny/by_agent_{agent}_turn_{turn}.py");
                assert!(paths.contains(&module.as_str()), "{module}: {paths:?}");
            }
        }
        assert!(paths.contains(&"tiny/stats.py"), "the seed: {paths:?}");

        // The plan records the project, the pin and the integrator setting, and when the change goes out.
        let plan: Value =
            serde_json::from_slice(&std::fs::read(cfg.root.join("plan.json")).expect("the plan")).expect("JSON");
        assert_eq!(plan["inputs"]["project"]["name"], "tiny");
        assert!(plan["inputs"]["python"]["digest"].is_string(), "{plan}");
        assert_eq!(plan["inputs"]["integrator"], ANY_AGENT);
        assert_eq!(plan["change_after"], 3);
        let text = serde_json::to_string(&plan).expect("JSON").to_lowercase();
        assert!(!text.contains("wave"), "no wave in a project run's plan");
        for e in log.iter().filter(|e| e["event"] == "changes_released" || e["event"] == "checkpoint") {
            assert!(!e.to_string().contains("wave"), "no wave in its events: {e}");
        }
    }
}
