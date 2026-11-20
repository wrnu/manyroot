//! Git project mode (task `6bd13d994a03`): agents build an evaluation project's tree with git and branches. The
//! stores through their workers, as a run drives them: the tools, the hub's rule on main, integration requests,
//! the checks on every ref and tree; and, with Docker, a mock run of the tiny fixture end to end, graded.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use bracken_experiment::driver::git_binary;
use bracken_experiment::git_project::NAME;
use bracken_experiment::mode::{Session, StoreConfig};
use bracken_experiment::project::Project;
use bracken_experiment::task::Task;
use bracken_experiment::weave_mode::{ANY_AGENT, hub_data};
use bracken_experiment::worker::{Request, Worker};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn tiny() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project-tiny")
}

fn request(op: &str, args: Value) -> Request {
    Request { op: op.into(), args, ..Request::default() }
}

/// A hub and copies of the tiny project, set up as a run sets them up, under an integrator setting.
struct Stores {
    dir: tempfile::TempDir,
    hub: Worker,
    copies: Vec<Worker>,
    /// Each copy's working tree.
    trees: Vec<PathBuf>,
}

impl Stores {
    fn new(agents: usize, setting: &str) -> Stores {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = |sub: &str, identity: &str| StoreConfig {
            dir: dir.path().join(sub),
            bracken: bin(),
            git: git_binary().expect("git"),
            identity: identity.into(),
            trace: false,
            task: Task::Phase1,
            accept_rule: Default::default(),
            limit: Default::default(),
        };
        let project = Project::load(&tiny()).expect("the fixture");
        let mut hub = Worker::spawn(&bin(), NAME, &config("hub", "setter")).expect("hub");
        hub.ok(&request("project", hub_data(&project, setting, false).expect("the data"))).expect("the project");
        let setup = hub.ok(&request("setup", Value::Null)).expect("hub setup");
        assert_eq!(setup.result["released"], json!(["sum", "mean-empty"]), "the items released at the start");
        let seed = Worker::bytes(&hub.ok(&request("export", Value::Null)).expect("export")).expect("bytes");
        let (mut copies, mut trees) = (Vec::new(), Vec::new());
        for i in 0..agents {
            let name = format!("agent-{i}");
            let mut w = Worker::spawn(&bin(), NAME, &config(&name, &name)).expect("copy");
            w.ok(&Request { op: "setup".into(), bytes: Some(hex::encode(&seed)), ..Request::default() })
                .expect("copy setup");
            copies.push(w);
            trees.push(dir.path().join(&name).join("work"));
        }
        hub.ok(&request("peers", json!({"keys": []}))).expect("peers");
        let mut stores = Stores { dir, hub, copies, trees };
        for i in 0..agents {
            stores.sync(i);
        }
        stores
    }

    fn sync(&mut self, i: usize) -> Value {
        self.try_sync(i).expect("a sync")
    }

    fn try_sync(&mut self, i: usize) -> Result<Value, String> {
        let session = Session {
            agent: i,
            name: format!("agent-{i}"),
            peers: Vec::new(),
            quota_contributions: 10_000,
            quota_bytes: 1 << 30,
            sync_seconds: 0,
        };
        bracken_experiment::driver::sync_transfers(&mut self.hub, &mut self.copies[i], &session)
    }

    fn tool(&mut self, i: usize, name: &str, args: Value) -> Result<Value, String> {
        self.copies[i]
            .ok(&Request { op: "tool".into(), tool: Some(name.into()), args, ..Request::default() })
            .map(|r| r.result)
    }

    fn ok(&mut self, i: usize, name: &str, args: Value) -> Value {
        self.tool(i, name, args.clone()).unwrap_or_else(|e| panic!("{name} {args}: {e}"))
    }

    /// The hub's main: each file's path and text.
    fn integrated(&mut self) -> Vec<(String, String)> {
        let reply = self.hub.ok(&request("integrated", Value::Null)).expect("integrated");
        serde_json::from_value(reply.result).expect("pairs")
    }

    fn on_main(&mut self, path: &str) -> bool {
        self.integrated().iter().any(|(p, _)| p == path)
    }

    /// Writes `path` in copy `i` and commits it on its current branch.
    fn commit_file(&mut self, i: usize, path: &str, text: &str) {
        self.ok(i, "write_file", json!({"path": path, "text": text}));
        self.ok(i, "commit", json!({"message": path}));
    }
}

/// A copy reads the task after a sync, edits its tree, commits on a branch, and asks for its integration; the
/// integrator sees the request, merges the branch into its main, and its sync makes that the hub's main. The
/// change goes out on release and reaches the copies at their next sync.
#[test]
fn a_builders_branch_reaches_main_through_the_integrator() {
    let mut s = Stores::new(2, "agent-1");
    let task = s.ok(0, "task", json!({}));
    assert!(task["spec"].as_str().is_some_and(|t| !t.is_empty()), "{task}");
    let ids: Vec<&str> = task["items"].as_array().expect("items").iter().filter_map(|i| i["id"].as_str()).collect();
    assert_eq!(ids, ["sum", "mean-empty"]);
    let files = s.ok(0, "list_files", json!({}));
    assert!(files["files"].as_array().expect("files").iter().any(|f| f["path"] == "tiny/stats.py"), "{files}");
    let status = s.ok(0, "status", json!({}));
    assert_eq!((status["branch"].clone(), status["main_updated_by"].clone()), (json!("main"), json!("agent-1")));

    s.ok(0, "switch", json!({"branch": "feat/zero", "create": true}));
    s.ok(0, "write_file", json!({"path": "tiny/zero.py", "text": "NAME = 'zero'\n"}));
    let diff = s.ok(0, "diff", json!({}));
    assert_eq!(diff["files"][0]["path"], "tiny/zero.py", "an uncommitted new file is in the diff: {diff}");
    assert!(s.tool(0, "switch", json!({"branch": "main"})).is_err_and(|e| e.contains("uncommitted")));
    s.ok(0, "commit", json!({"message": "zero"}));
    let diff = s.ok(0, "diff", json!({"against": "origin/main"}));
    assert!(diff["diff"].as_str().is_some_and(|d| d.contains("+NAME = 'zero'")), "{diff}");
    let log = s.ok(0, "log", json!({}));
    assert!(log["log"].as_str().is_some_and(|l| l.contains("agent-0: zero")), "{log}");
    let asked = s.ok(0, "request_integration", json!({"branch": "feat/zero", "summary": "adds zero"}));
    assert!(asked["note"].as_str().is_some_and(|n| n.contains("next sync")), "{asked}");
    assert!(s.ok(1, "requests", json!({}))["requests"].as_array().is_some_and(Vec::is_empty), "not before a sync");
    s.sync(0);
    s.sync(1);
    let open = s.ok(1, "requests", json!({}));
    let r = &open["requests"][0];
    assert_eq!(
        (r["from"].clone(), r["branch"].clone(), r["summary"].clone()),
        (json!("agent-0"), json!("origin/agent-0/feat/zero"), json!("adds zero"))
    );
    assert!(!s.on_main("tiny/zero.py"), "a request alone changes nothing");
    let merged = s.ok(1, "merge", json!({"ref": "origin/agent-0/feat/zero"}));
    assert_eq!(merged["merged"], true, "{merged}");
    let synced = s.sync(1);
    assert_eq!(synced["main"], "updated", "{synced}");
    assert!(s.on_main("tiny/zero.py"), "the builder's work is on main");
    s.sync(0);
    assert!(s.ok(0, "requests", json!({}))["requests"].as_array().is_some_and(Vec::is_empty), "merged: closed");

    // The change goes out at the hub, and each copy sees it at its next sync.
    let released = s.hub.ok(&request("release", json!({"wave": 1}))).expect("the release");
    assert_eq!(released.result["released"], json!(["decimals"]));
    let ids = |v: &Value| {
        v["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|i| i["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&s.ok(0, "task", json!({}))).len(), 2, "not before a sync");
    s.sync(0);
    assert_eq!(ids(&s.ok(0, "task", json!({}))), ["sum", "mean-empty", "decimals"]);
    let line = s.hub.ok(&request("line", Value::Null)).expect("the line").result;
    let head = &line["heads"][0];
    assert!(line["forked"] == false && head["decision"].as_str().is_some_and(|c| c.len() == 40), "{line}");
    assert!(head["manifest"].as_str().is_some_and(|t| t.len() == 40 && t != head["decision"]), "{line}");
}

/// The integrator reviews a requested branch without switching to it (finding `a236e7206344`): a diff of two
/// refs shows what the branch brings to main, not inverted and not mixed with the integrator's own main, and
/// changes nothing. A request closed by its merge stays closed when the branch gets later commits.
#[test]
fn an_integrator_reviews_a_requested_branch_and_a_merged_request_stays_closed() {
    let mut s = Stores::new(2, "agent-1");
    s.ok(0, "switch", json!({"branch": "feat/zero", "create": true}));
    s.commit_file(0, "tiny/zero.py", "NAME = 'zero'\n");
    s.ok(0, "request_integration", json!({"branch": "feat/zero", "summary": "adds zero"}));
    s.sync(0);
    // The integrator has its own uncommitted and committed work on main.
    s.commit_file(1, "tiny/one.py", "NAME = 'one'\n");
    s.ok(1, "write_file", json!({"path": "tiny/draft.py", "text": "x = 1\n"}));
    s.sync(1);
    let review = s.ok(1, "diff", json!({"against": "origin/main", "to": "origin/agent-0/feat/zero"}));
    let paths: Vec<&Value> = review["files"].as_array().expect("files").iter().map(|f| &f["path"]).collect();
    assert_eq!(paths, [&json!("tiny/zero.py")], "only what the branch brings: {review}");
    assert_eq!(review["files"][0]["change"], "A", "{review}");
    assert!(review["diff"].as_str().is_some_and(|d| d.contains("+NAME = 'zero'")), "{review}");
    let one = s.ok(1, "diff", json!({"to": "origin/agent-0/feat/zero", "path": "tiny/zero.py"}));
    assert_eq!(one["files"].as_array().map(Vec::len), Some(1), "{one}");
    assert!(s.trees[1].join("tiny/draft.py").exists(), "the review changed nothing");
    assert_eq!(s.ok(1, "status", json!({}))["uncommitted"].as_array().map(Vec::len), Some(1));
    s.ok(1, "delete_file", json!({"path": "tiny/draft.py"}));
    s.ok(1, "merge", json!({"ref": "origin/agent-0/feat/zero"}));
    assert_eq!(s.sync(1)["main"], "updated");
    s.sync(0);
    assert_eq!(s.ok(0, "requests", json!({}))["requests"], json!([]));
    // A later commit on the merged branch doesn't reopen its request; a new request does.
    s.commit_file(0, "tiny/zero2.py", "NAME = 'zero2'\n");
    s.sync(0);
    s.sync(1);
    assert_eq!(s.ok(1, "requests", json!({}))["requests"], json!([]), "closed for good");
    s.ok(0, "request_integration", json!({"branch": "feat/zero", "summary": "adds zero2"}));
    s.sync(0);
    s.sync(1);
    assert_eq!(s.ok(1, "requests", json!({}))["requests"][0]["summary"], "adds zero2");
}

/// Only the agent the setting names updates main, and only by a fast-forward: the hub refuses the rest and says
/// why, whatever the agent's prompt told it; a refused main changes nothing at the hub.
#[test]
fn the_hub_lets_only_the_integrator_update_main_and_only_forward() {
    let mut s = Stores::new(3, "agent-2");
    s.commit_file(0, "tiny/zero.py", "NAME = 'zero'\n");
    let synced = s.sync(0);
    assert!(synced["main"].as_str().is_some_and(|m| m.starts_with("refused: only agent-2 updates main")), "{synced}");
    assert!(!s.on_main("tiny/zero.py"));

    // Under any-agent, two agents' mains: the first moves main, the second isn't a fast-forward until it merges.
    let mut s = Stores::new(2, ANY_AGENT);
    s.commit_file(0, "tiny/zero.py", "NAME = 'zero'\n");
    s.commit_file(1, "tiny/one.py", "NAME = 'one'\n");
    assert_eq!(s.sync(0)["main"], "updated");
    let refused = s.sync(1);
    assert!(refused["main"].as_str().is_some_and(|m| m.contains("not a fast-forward")), "{refused}");
    assert!(!s.on_main("tiny/one.py"));
    s.ok(1, "merge", json!({"ref": "origin/main"}));
    assert_eq!(s.sync(1)["main"], "updated");
    assert!(s.on_main("tiny/zero.py") && s.on_main("tiny/one.py"), "both, merged");
    assert_eq!(s.sync(0)["main"], "not pushed: your main is behind origin/main");
}

/// A merge that conflicts leaves markers in the files it lists; commit is refused while they stay, and
/// concludes the merge once they're settled. Abort undoes one.
#[test]
fn a_conflicting_merge_is_settled_in_the_files_then_committed() {
    let mut s = Stores::new(2, ANY_AGENT);
    s.commit_file(0, "tiny/stats.py", "def mean(numbers):\n    return 0\n");
    s.sync(0);
    s.commit_file(1, "tiny/stats.py", "def mean(numbers):\n    return 1\n");
    s.sync(1);
    let merged = s.ok(1, "merge", json!({"ref": "origin/main"}));
    assert_eq!((merged["merged"].clone(), merged["conflicts"].clone()), (json!(false), json!(["tiny/stats.py"])));
    let status = s.ok(1, "status", json!({}));
    assert_eq!((status["merging"].clone(), status["conflicts"].clone()), (json!(true), json!(["tiny/stats.py"])));
    assert!(s.tool(1, "commit", json!({"message": "m"})).is_err_and(|e| e.contains("conflict markers")));
    assert!(s.tool(1, "merge", json!({"ref": "origin/main"})).is_err_and(|e| e.contains("in progress")));
    s.ok(1, "merge", json!({"abort": true}));
    assert_eq!(s.ok(1, "status", json!({}))["merging"], false);
    s.ok(1, "merge", json!({"ref": "origin/main"}));
    s.ok(1, "write_file", json!({"path": "tiny/stats.py", "text": "def mean(numbers):\n    return 2\n"}));
    s.ok(1, "commit", json!({"message": "settled"}));
    assert_eq!(s.sync(1)["main"], "updated");
    let stats = s.integrated().into_iter().find(|(p, _)| p == "tiny/stats.py").expect("stats").1;
    assert!(stats.contains("return 2"), "{stats}");
}

/// Every path and ref an agent names is checked: no path into `.git` or out of the tree, no symlink on the way,
/// no ref that isn't a branch name git takes, no option where a ref goes; nothing a tool refuses runs git on it.
#[test]
fn every_path_and_ref_an_agent_names_is_checked() {
    let mut s = Stores::new(1, ANY_AGENT);
    for path in ["../x.py", "/etc/x", ".git/config", "tiny/.git/x", ".gitattributes", "a//b", "a\\b", ""] {
        assert!(s.tool(0, "write_file", json!({"path": path, "text": "x"})).is_err(), "{path:?}");
    }
    std::os::unix::fs::symlink("/tmp", s.trees[0].join("link")).expect("a symlink");
    assert!(s.tool(0, "write_file", json!({"path": "link/x.py", "text": "x"})).is_err_and(|e| e.contains("symlink")));
    std::fs::remove_file(s.trees[0].join("link")).expect("removed");
    for name in [
        "-b",
        "--force",
        "HEAD",
        "origin/main",
        "a..b",
        "a.lock",
        ".a",
        "a b",
        "A",
        "a/",
        "x".repeat(101).as_str(),
        "a\u{0}b",
        "@{-1}",
        "a~1",
        "a^",
    ] {
        let made = s.tool(0, "switch", json!({"branch": name, "create": true}));
        assert!(made.is_err(), "{name:?} made a branch");
    }
    for r in ["--all", "origin/../main", "origin/agent-x/b", "origin/agent-0", "HEAD~1", "main@{1}", "-n"] {
        assert!(s.tool(0, "merge", json!({"ref": r})).is_err(), "{r:?}");
        assert!(s.tool(0, "diff", json!({"against": r})).is_err(), "{r:?}");
        assert!(s.tool(0, "diff", json!({"to": r})).is_err(), "{r:?}");
        assert!(s.tool(0, "log", json!({"ref": r})).is_err(), "{r:?}");
    }
    assert!(s.tool(0, "switch", json!({"branch": "fix/a", "create": true, "start": "nope"})).is_err());
    s.ok(0, "switch", json!({"branch": "fix/a-1_b.c", "create": true}));
    assert!(s.tool(0, "request_integration", json!({"branch": "main", "summary": "s"})).is_err());
    assert!(s.tool(0, "request_integration", json!({"branch": "nope", "summary": "s"})).is_err());
    assert!(s.tool(0, "request_integration", json!({"branch": "fix/a-1_b.c", "summary": ""})).is_err());
    assert!(s.tool(0, "commit", json!({"message": "nothing"})).is_err_and(|e| e.contains("nothing to commit")));
    assert!(s.tool(0, "status", json!({"extra": 1})).is_err(), "no argument a tool doesn't take");
}

/// A push the hub can't take is refused whole, before any of it is kept: a tree with a symlink, a submodule,
/// a path into `.git`, a branch name git doesn't take, or a request for a branch it doesn't hold.
#[test]
fn the_hub_refuses_a_push_it_cant_take() {
    let mut s = Stores::new(1, ANY_AGENT);
    let git = git_binary().expect("git");
    let work = s.trees[0].clone();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&git)
            .current_dir(&work)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    // A commit holding a symlink, made behind the tools' back, on a branch of its own.
    run(&["switch", "-q", "-c", "bad"]);
    std::os::unix::fs::symlink("/etc/passwd", work.join("leak")).expect("a symlink");
    run(&["add", "leak"]);
    run(&["-c", "user.name=x", "-c", "user.email=x@x", "commit", "-q", "-m", "bad"]);
    let err = s.try_sync(0).expect_err("refused");
    assert!(err.contains("isn't a regular file"), "{err}");
    assert!(!s.on_main("leak"));
    run(&["switch", "-q", "main"]);
    run(&["branch", "-q", "-D", "bad"]);
    s.sync(0);
    // A branch name the tools would never make.
    run(&["branch", "UPPER"]);
    assert!(s.try_sync(0).expect_err("refused").contains("UPPER"));
    run(&["branch", "-q", "-D", "UPPER"]);
    // A request for a branch the push doesn't hold.
    std::fs::write(s.dir.path().join("agent-0/pending.json"), r#"[{"branch":"ghost","summary":"x"}]"#)
        .expect("written");
    assert!(s.try_sync(0).expect_err("refused").contains("doesn't hold"));
}

/// The project mode needs a project and the Python pin, and the git mode with a project is this one.
#[test]
fn git_with_a_project_is_git_project_mode() {
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
        bracken: bin(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan,
        sandbox: false,
    };
    let mock = MockBackend { lockstep: true, ..MockBackend::default() };
    let with_project = PlanInputs { project: Some(project.clone()), ..PlanInputs::default() };
    for (plan, why) in [(PlanInputs::default(), "needs a project"), (with_project, "needs the Python pin")] {
        let err = run(&cfg(NAME, plan), &mock).err().expect("refused");
        assert!(err.contains(why), "{err}");
        assert!(!dir.path().join("run").exists(), "no run directory");
    }
    let out = std::process::Command::new(bin())
        .args(["mock-run", "--mode", "git", "--project"])
        .arg(tiny())
        .args(["--python", "python:3.12@sha256:0000", "--dir"])
        .arg(dir.path().join("run"))
        .output()
        .expect("mock-run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && !err.contains("takes no project"), "git with a project is a project run: {err}");
}
