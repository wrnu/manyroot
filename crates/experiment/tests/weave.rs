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
            limit: Default::default(),
        };
        let sockets = dir.path().join("s");
        std::fs::create_dir(&sockets).expect("sockets");
        let project = Project::load(&tiny()).expect("the fixture");
        let mut hub = Worker::spawn(&bin(), "weave", &config("hub", "setter")).expect("hub");
        hub.ok(&request("project", hub_data(&project, ANY_AGENT, false).expect("the data"))).expect("the project");
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
    assert!(items.iter().all(|i| i["id"] != "decimals"), "{task}");
    // The whole task in one page here: each item in full, on the page the index says (task fb08637d1101).
    assert_eq!((task["page"].as_u64(), task["pages"].as_u64()), (Some(1), Some(1)), "{task}");
    let texts = task["texts"].as_array().expect("texts");
    assert_eq!(texts.len(), 2, "{task}");
    assert!(items.iter().zip(texts).all(|(i, t)| i["page"] == 1
        && t.as_str().is_some_and(|t| t.starts_with(&format!("Item {} (", i["id"].as_str().unwrap_or_default())))));
    // The task shows an index of the items; each one's full text comes by its id (task b58cf9180030).
    let first = items[0]["id"].as_str().expect("an id").to_string();
    let one = s.ok(0, "task", json!({"item": first}));
    assert!(one["text"].as_str().is_some_and(|t| t.starts_with(&format!("Item {first} ("))), "{one}");
    let err = s.tool(0, "task", json!({"item": "decimals"})).expect_err("not released yet");
    assert!(err.contains("no released item"), "{err}");

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
    // A top-level file named base is a path, never the tag changes diffs against (finding 0fe23b26f83e).
    s.ok(0, "write_file", json!({"path": "base", "text": "a file\n"}));
    let changes = s.ok(0, "changes", json!({}));
    assert!(changes["files"].as_array().expect("files").iter().any(|f| f["path"] == "base" && f["change"] == "A"));
    // With a path, that file's changes alone.
    let one = s.ok(0, "changes", json!({"path": "base"}));
    assert_eq!(one["files"], json!([{"path": "base", "change": "A", "page": 1}]), "{one}");
    assert!(s.tool(0, "changes", json!({"page": 2})).expect_err("one page").contains("1 pages"));
    assert!(one["diff"].as_str().is_some_and(|d| d.contains("+a file") && !d.contains("total")), "{one}");
    assert!(s.tool(0, "changes", json!({"path": "../x"})).is_err());
    s.ok(0, "delete_file", json!({"path": "base"}));
    let head = s.ok(0, "head", json!({}));
    assert_eq!(head["unproposed_edits"], true, "{head}");
    assert_eq!(head["heads"][0]["decision"], head["base"], "{head}");
    assert!(s.tool(0, "update", json!({})).is_err_and(|e| e.contains("not yet proposed")));

    let proposed = s.ok(0, "propose", json!({"summary": "a total"}));
    let proposal = proposed["proposal"].as_str().expect("a proposal").to_string();
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false);
    let open = s.ok(0, "proposals", json!({}));
    assert_eq!(open["proposals"][0]["proposal"], proposal.as_str(), "{open}");
    let seed = head["base"].as_str().expect("the seed decision").to_string();
    let integrated = s.ok(0, "integrate", json!({"proposals": [&proposal[..12]], "note": "a total"}));
    let decision = integrated["decision"].as_str().expect("a decision").to_string();
    // diff shows each changed file's text (decision b423e8767735), and leaves the agent's tree alone.
    let diff = s.ok(0, "diff", json!({"a": &seed[..12], "b": &decision[..12]}));
    assert_eq!(diff["files"], json!([{"path": "tiny/total.py", "change": "added", "page": 1}]), "{diff}");
    let text = diff["diff"].as_str().expect("text");
    assert!(
        text.starts_with("diff --git a/tiny/total.py b/tiny/total.py\n") && text.contains("+def total(xs):"),
        "{diff}"
    );
    let back = s.ok(0, "diff", json!({"a": &decision[..12], "b": &seed[..12]}));
    assert!(back["diff"].as_str().is_some_and(|t| t.contains("deleted file mode") && t.contains("-def total(xs):")));
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false, "the tree untouched");
    let leftovers = ["diff.tree", "diff.index"].map(|f| s.trees[0].parent().expect("the store").join(f).exists());
    assert_eq!(leftovers, [false, false], "the diff's scratch is cleared");
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
    let change = s.ok(0, "task", json!({"item": "decimals"}));
    assert!(change["text"].as_str().is_some_and(|t| t.starts_with("Item decimals (")), "{change}");

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
    let err = second.expect_err("the second edits the same line");
    // The agent is told each conflict, not bracken's first words (task b58cf9180030).
    assert!(err.contains("stopped at 1 conflicts") && err.contains(r#""path":"tiny/stats.py""#), "{err}");
    assert!(err.contains("Settle each with resolve"), "{err}");
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
    // A conflict is open until resolve settles it, so one with no markers to find (a binary file's) never
    // passes unsettled (finding e26a1c74e007); a file still marked can't be settled as it stands.
    let err = s.tool(0, "integrate", json!({"note": "still marked"})).expect_err("unsettled");
    assert!(err.contains("isn't settled"), "{err}");
    let err = s.tool(0, "resolve", json!({"path": "tiny/stats.py", "take": "tree"})).expect_err("markers left");
    assert!(err.contains("conflict markers"), "{err}");
    for bad in [json!({"path": "tiny/zero.py", "take": "tree"}), json!({"path": "tiny/stats.py", "take": "from"})] {
        assert!(s.tool(0, "resolve", bad.clone()).is_err(), "{bad}");
    }
    s.ok(
        0,
        "write_file",
        json!({"path": "tiny/stats.py", "text": stats.replace(&line, &format!("{line}  # zero and one"))}),
    );
    let err = s.tool(0, "integrate", json!({"note": "settled, not said"})).expect_err("not resolved");
    assert!(err.contains("isn't settled"), "{err}");
    let resolved = s.ok(0, "resolve", json!({"path": "tiny/stats.py", "take": "tree"}));
    assert_eq!(resolved["conflicts"], json!([]), "{resolved}");
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

/// Two agents' integrations, met at a sync: a forked line, each head with one file of its own.
fn forked() -> Stores {
    let mut s = Stores::new(&["agent-0", "agent-1"]);
    for (i, name) in [(0, "zero"), (1, "one")] {
        s.ok(i, "write_file", json!({"path": format!("tiny/{name}.py"), "text": format!("NAME = '{name}'\n")}));
        s.ok(i, "integrate", json!({"note": format!("agent {i}'s tree")}));
    }
    for i in [0, 1, 0, 1] {
        s.sync(i);
    }
    assert_eq!(s.ok(0, "head", json!({}))["forked"], true);
    s
}

/// What a copy's directory holds beside its tree: tree.new and any tree.old.<n>.
fn aside(tree: &Path) -> Vec<String> {
    let dir = tree.parent().expect("the copy's directory");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("the copy's directory")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n == "tree.new" || n.starts_with("tree.old."))
        .collect();
    names.sort();
    names
}

/// A failed checkout that couldn't put everything back leaves git's repository in tree.new, or also the
/// agent's files in tree.old.<n> with the tree missing (task 43ca0a00d4b9). The next tool puts each back
/// before anything is cleared, so neither copy is lost; where it can't tell which is the tree, or both trees
/// hold a repository, it refuses and removes nothing. Each state is made by hand, as such a failure leaves it.
#[test]
fn what_a_failed_checkout_left_aside_is_put_back_or_kept() {
    let mut s = Stores::new(&["agent-0"]);
    let tree = s.trees[0].clone();
    let dir = tree.parent().expect("the copy's directory").to_path_buf();
    s.ok(0, "write_file", json!({"path": "tiny/zero.py", "text": "NAME = 'zero'\n"}));
    s.ok(0, "integrate", json!({"note": "zero"}));
    let whole = |s: &mut Stores| {
        assert!(tree.join(".git").is_dir() && tree.join("tiny/zero.py").exists(), "the tree and its repository");
        assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false, "the tree is as it was");
    };

    // Git's repository alone in tree.new: it goes back, and the checkout after clears tree.new.
    std::fs::create_dir(dir.join("tree.new")).expect("tree.new");
    std::fs::rename(tree.join(".git"), dir.join("tree.new/.git")).expect("the repository aside");
    s.ok(0, "update", json!({}));
    whole(&mut s);
    assert_eq!(aside(&tree), Vec::<String>::new());

    // The tree in tree.old.0 and its repository in tree.new: both go back.
    std::fs::create_dir(dir.join("tree.new")).expect("tree.new");
    std::fs::rename(tree.join(".git"), dir.join("tree.new/.git")).expect("the repository aside");
    std::fs::rename(&tree, dir.join("tree.old.0")).expect("the tree aside");
    s.ok(0, "update", json!({}));
    whole(&mut s);
    assert_eq!(aside(&tree), Vec::<String>::new());

    // Two old trees and no tree: which was the tree is unclear, so both stay, and nothing runs.
    std::fs::rename(&tree, dir.join("tree.old.0")).expect("the tree aside");
    std::fs::create_dir(dir.join("tree.old.1")).expect("another old tree");
    let err = s.tool(0, "update", json!({})).expect_err("unclear");
    assert!(err.contains("the tree is missing, and 2 old trees are left"), "{err}");
    assert!(dir.join("tree.old.0/.git").is_dir() && dir.join("tree.old.1").is_dir(), "both kept");
    std::fs::remove_dir(dir.join("tree.old.1")).expect("the empty one");
    s.ok(0, "update", json!({}));
    whole(&mut s);

    // A repository in both trees: neither is removed.
    std::fs::create_dir_all(dir.join("tree.new/.git")).expect("a second repository");
    let err = s.tool(0, "update", json!({})).expect_err("two repositories");
    assert!(err.contains("hold a git repository") && err.contains("neither is removed"), "{err}");
    assert!(dir.join("tree.new/.git").is_dir() && tree.join(".git").is_dir(), "both kept");
    std::fs::remove_dir_all(dir.join("tree.new")).expect("the second");
    s.ok(0, "update", json!({}));
    whole(&mut s);
}

/// A checkout whose new tree can't be placed puts the old tree back, with its repository and files
/// (task 43ca0a00d4b9): the merge says what failed, the tree is as it was, and the next merge goes through,
/// leaving nothing aside. The placing is made to fail by the debug build's `fail-placing-the-tree` file,
/// since nothing else makes that one rename fail, a root user's included.
#[test]
fn a_tree_that_cant_be_placed_is_put_back_and_the_next_merge_goes_through() {
    let mut s = forked();
    let tree = s.trees[0].clone();
    let marker = tree.parent().expect("the copy's directory").join("fail-placing-the-tree");
    std::fs::write(&marker, "").expect("the marker");
    let err = s.tool(0, "merge", json!({})).expect_err("placing fails");
    std::fs::remove_file(&marker).expect("the marker");
    assert!(err.contains("placing the tree: failed for a test"), "{err}");
    assert!(!err.contains("putting the tree back") && !err.contains("not a git repository"), "{err}");
    assert!(tree.join(".git").is_dir() && tree.join("tiny/zero.py").exists(), "the old tree, with its repository");
    assert!(!tree.join("tiny/one.py").exists(), "not the other head's");
    // The old tree itself came back, not just its files by the merge's reset: none is left aside, only the
    // checkout that couldn't be placed.
    assert_eq!(aside(&tree), ["tree.new"]);
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false, "the tree is as it was");

    let merged = s.ok(0, "merge", json!({}));
    assert_eq!(merged["conflicts"], json!([]), "{merged}");
    for name in ["zero", "one"] {
        assert!(tree.join(format!("tiny/{name}.py")).exists(), "{name}'s file is in the merge");
    }
    assert_eq!(aside(&tree), Vec::<String>::new());
}

/// Something that makes the tree again between the set-aside and the placing (the unknown writer behind
/// task 61c34b3c2704's flake) fails the placing, and the old tree, holding the agent's files, isn't put back
/// over it (task af9ef395181b). That old tree stays until the tree is a working checkout again, and goes only
/// then. The debug build's `recreate-the-tree` file stands in for the writer. Separately, a tree with no
/// repository while an old tree holds one is refused, and neither is moved or removed.
#[test]
fn an_old_tree_stays_while_the_tree_isnt_a_working_checkout() {
    let mut s = Stores::new(&["agent-0"]);
    let tree = s.trees[0].clone();
    let dir = tree.parent().expect("the copy's directory").to_path_buf();
    s.ok(0, "write_file", json!({"path": "tiny/zero.py", "text": "NAME = 'zero'\n"}));
    s.ok(0, "integrate", json!({"note": "zero"}));

    let marker = dir.join("recreate-the-tree");
    std::fs::write(&marker, "").expect("the marker");
    let err = s.tool(0, "update", json!({})).expect_err("the placing fails");
    std::fs::remove_file(&marker).expect("the marker");
    assert!(err.contains("placing the tree"), "{err}");
    assert!(dir.join("tree.old.0/tiny/zero.py").exists(), "the agent's files are kept aside");
    assert!(tree.join(".git").is_dir() && tree.join("recreated").exists(), "the writer's tree, given the repository");
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], true, "the tree isn't its repository's files");
    let err = s.tool(0, "update", json!({})).expect_err("not a clean tree");
    assert!(err.contains("not yet proposed"), "{err}");
    assert!(dir.join("tree.old.0/tiny/zero.py").exists(), "still kept");

    // Once the tree is its repository's files again, the next checkout sweeps the old tree.
    std::fs::remove_file(tree.join("recreated")).expect("the writer's file");
    let git = bracken_experiment::driver::git_binary().expect("git");
    let out = std::process::Command::new(git).arg("-C").arg(&tree).args(["reset", "-q", "--hard"]).output();
    assert!(out.expect("git runs").status.success(), "the files back from the repository");
    s.ok(0, "update", json!({}));
    assert!(tree.join("tiny/zero.py").exists());
    assert_eq!(aside(&tree), Vec::<String>::new());

    // A tree with no repository while an old tree holds one: neither is moved or removed.
    std::fs::create_dir(dir.join("tree.old.0")).expect("an old tree");
    std::fs::rename(tree.join(".git"), dir.join("tree.old.0/.git")).expect("its repository");
    let err = s.tool(0, "head", json!({})).expect_err("unclear");
    assert!(err.contains("has no git repository, while") && err.contains("neither is moved or removed"), "{err}");
    assert!(dir.join("tree.old.0/.git").is_dir() && tree.join("tiny/zero.py").exists(), "both kept");
    std::fs::rename(dir.join("tree.old.0/.git"), tree.join(".git")).expect("the repository back");
    std::fs::remove_dir(dir.join("tree.old.0")).expect("the empty old tree");
    assert_eq!(s.ok(0, "head", json!({}))["unproposed_edits"], false);
}

/// When the heads' merge fails and so does putting the tree back, the merge's own failure is told first
/// (task 43ca0a00d4b9, after 898fff65aaef). A lock git finds already held fails both, root or not: the
/// mirror's commit and the reset each need the index lock, while the status before them doesn't.
#[test]
fn a_merge_whose_restore_fails_too_tells_the_merges_failure_first() {
    let mut s = forked();
    let lock = s.trees[0].join(".git/index.lock");
    std::fs::write(&lock, "").expect("a held lock");
    let err = s.tool(0, "merge", json!({})).expect_err("both fail");
    std::fs::remove_file(&lock).expect("the lock");
    let (merge, restore) = err.split_once("; and restoring the tree: ").unwrap_or_else(|| panic!("both: {err}"));
    // Git's messages are cut short, before the lock's name: each step says it couldn't create it.
    assert!(merge.starts_with("git add: ") && merge.contains("Unable to create"), "the merge's first: {err}");
    assert!(restore.starts_with("git reset: ") && restore.contains("Unable to create"), "then the restore's: {err}");
}

/// A fork whose heads both change `tiny/stats.py`'s return line, met at a sync: its merge conflicts there.
fn forked_with_a_conflict() -> Stores {
    let mut s = Stores::new(&["agent-0", "agent-1"]);
    let stats = std::fs::read_to_string(s.trees[0].join("tiny/stats.py")).expect("the seed's stats");
    let line = stats.lines().find(|l| l.contains("return")).expect("a return line").to_string();
    for (i, name) in [(0, "zero"), (1, "one")] {
        s.ok(i, "edit_file", json!({"path": "tiny/stats.py", "old": line, "new": format!("{line}  # {name}")}));
        s.ok(i, "integrate", json!({"note": format!("agent {i}'s tree")}));
    }
    for i in [0, 1, 0, 1] {
        s.sync(i);
    }
    s
}

/// A conflict on a path git would quote (a non-ASCII byte, a double quote) is reported as the path is, so
/// resolve can settle it and the merge can be integrated (finding f475a036f200).
#[test]
fn a_conflict_on_a_path_git_would_quote_can_be_settled() {
    let mut s = Stores::new(&["agent-0", "agent-1"]);
    let paths = ["tiny/café.py", "tiny/qu\"ote.py"];
    // Each head adds both files, with its own text.
    for (i, name) in [(0, "zero"), (1, "one")] {
        for path in paths {
            s.ok(i, "write_file", json!({"path": path, "text": format!("X = '{name}'\n")}));
        }
        s.ok(i, "integrate", json!({"note": format!("agent {i}'s tree")}));
    }
    for i in [0, 1, 0, 1] {
        s.sync(i);
    }
    let merged = s.ok(0, "merge", json!({}));
    assert_eq!(merged["conflicts"], json!(paths), "{merged}");
    for path in paths {
        s.ok(0, "write_file", json!({"path": path, "text": "X = 'both'\n"}));
        s.ok(0, "resolve", json!({"path": path, "take": "tree"}));
    }
    let settled = s.ok(0, "integrate", json!({"note": "both heads, settled"}));
    assert_eq!(settled["parents"].as_array().expect("parents").len(), 2, "{settled}");
}

/// A merge's conflicts are open until resolve settles them, even when the list of them is lost: a missing
/// conflicts.txt is never read as none, and one that couldn't be written leaves no merge.txt behind to vouch
/// for the tree (finding 1f85d3eb08d1).
#[test]
fn a_merges_conflicts_stay_open_when_their_list_is_lost() {
    let mut s = forked_with_a_conflict();
    let dir = s.trees[0].parent().expect("the copy's directory").to_path_buf();
    let merged = s.ok(0, "merge", json!({}));
    assert_eq!(merged["conflicts"], json!(["tiny/stats.py"]), "{merged}");
    std::fs::remove_file(dir.join("conflicts.txt")).expect("the conflicts' list");
    let err = s.tool(0, "integrate", json!({"note": "list lost"})).expect_err("the conflict is open");
    assert!(err.contains("can't be read"), "{err}");

    let mut s = forked_with_a_conflict();
    let dir = s.trees[0].parent().expect("the copy's directory").to_path_buf();
    // The conflicts' list can't be written: its temporary copy's name is taken by a directory.
    std::fs::create_dir_all(dir.join("conflicts.txt.new/held")).expect("a directory in the way");
    let err = s.tool(0, "merge", json!({})).expect_err("the list can't be written");
    assert!(err.contains("conflicts.txt"), "{err}");
    assert!(!dir.join("merge.txt").exists(), "no merge.txt vouches for the tree");
    let err = s.tool(0, "integrate", json!({"note": "list unwritten"})).expect_err("the conflict is open");
    assert!(err.contains("merge them into your tree first"), "{err}");
}

/// Bracken refuses to integrate a proposal on a forked line with "is forked", the words the mock's weave
/// turn takes as the proposal left open for later (mock.rs, task 61c34b3c2704): this pins them, so a change
/// to bracken's message fails here, not as a mock run's lost turn.
#[test]
fn integrating_a_proposal_on_a_forked_line_is_refused_as_forked() {
    let mut s = forked();
    s.ok(0, "write_file", json!({"path": "tiny/late.py", "text": "LATE = True\n"}));
    let p = s.ok(0, "propose", json!({"summary": "late"}));
    let err = s.tool(0, "integrate", json!({"proposals": [p["proposal"]], "note": "late"})).expect_err("forked");
    assert!(err.contains("is forked"), "{err}");
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

    /// A checkout sets the old tree aside whole and removes it after, so one that can't be cleared, as when
    /// something writes into it while it is (task 61c34b3c2704), neither fails it nor costs the tree its repository
    /// (task 898fff65aaef). A read-only directory stands for that here, which a root user could clear, so it's
    /// here. The next checkout removes what's left once it can.
    #[test]
    fn a_checkout_sets_the_old_tree_aside_and_keeps_its_repository() {
        use std::os::unix::fs::PermissionsExt;
        let mut s = Stores::new(&["agent-0", "agent-1"]);
        for (i, name) in [(0, "zero"), (1, "one")] {
            s.ok(i, "write_file", json!({"path": format!("tiny/{name}.py"), "text": format!("NAME = '{name}'\n")}));
            s.ok(i, "integrate", json!({"note": format!("agent {i}'s tree")}));
        }
        for i in [0, 1, 0, 1] {
            s.sync(i);
        }
        assert_eq!(s.ok(0, "head", json!({}))["forked"], true);
        let mode = |dir: &Path, m| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(m)).expect("a mode");
        let dir = s.trees[0].parent().expect("the copy's directory").to_path_buf();
        let aside = || -> Vec<PathBuf> {
            let entries = std::fs::read_dir(&dir).expect("the copy's directory").flatten();
            entries.map(|e| e.path()).filter(|p| p.to_string_lossy().contains("/tree.old.")).collect()
        };

        // The merge's checkouts set the old tree aside, though it can't be cleared, and the merge goes through.
        mode(&s.trees[0].join("tiny"), 0o555);
        let merged = s.ok(0, "merge", json!({}));
        assert_eq!(merged["conflicts"], json!([]), "{merged}");
        assert!(s.trees[0].join(".git").is_dir(), "the tree keeps its repository");
        for name in ["zero", "one"] {
            assert!(s.trees[0].join(format!("tiny/{name}.py")).exists(), "{name}'s file is in the merge");
        }
        let left = aside();
        assert_eq!(left.len(), 1, "the old tree that couldn't be cleared: {left:?}");

        // Once it can be removed, the next checkout does.
        mode(&left[0].join("tiny"), 0o755);
        s.ok(0, "integrate", json!({"note": "the merge"}));
        s.ok(0, "update", json!({}));
        assert_eq!(aside(), Vec::<PathBuf>::new());
    }
    use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, run};
    use bracken_experiment::mock::MockBackend;
    use bracken_experiment::plan::{PlanInputs, ProjectPlan};

    fn events(path: &Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).expect("the event log");
        text.lines().map(|l| serde_json::from_str(l).expect("a JSON line")).collect()
    }

    /// The image these tests pin, as `python::tests` does; None, having said why, where they can't run.
    fn pin() -> Option<bracken_experiment::python::PythonPin> {
        bracken_experiment::python::for_tests().map(|p| p.plan_pin())
    }

    /// Agents playing at once, not in lockstep: every agent integrates its first turn before it syncs, so the
    /// line forks, and later turns settle forks with merges. Every decision with more than one parent, in the
    /// hub's own log, keeps every file of each of its parents (finding 9f4e48c4c547): no settled fork loses a
    /// head's work. The mock writes only new files, so a parent's file missing from a merge is lost work.
    #[test]
    fn concurrent_agents_fork_the_line_and_every_merge_keeps_both_heads() {
        let Some(pin) = pin() else { return };
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
            trace: true,
            calls_per_turn: CALLS_PER_TURN,
            turn_limit: TURN_LIMIT,
            plan: PlanInputs {
                project: Some(project),
                python: Some(pin),
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
            "no turn erred: {:?}\n{}",
            log.iter().filter(|e| e["event"] == "turn_error").collect::<Vec<_>>(),
            std::fs::read_to_string(cfg.root.join("turn-errors.txt")).unwrap_or_default()
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
        let Some(pin) = pin() else { return };
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
        assert!(plan["inputs"]["python"]["image"].as_str().is_some_and(|i| i.contains("@sha256:")), "{plan}");
        assert_eq!(plan["inputs"]["integrator"], ANY_AGENT);
        assert_eq!(plan["change_after"], 3);
        let text = serde_json::to_string(&plan).expect("JSON").to_lowercase();
        assert!(!text.contains("wave"), "no wave in a project run's plan");
        for e in log.iter().filter(|e| e["event"] == "changes_released" || e["event"] == "checkpoint") {
            assert!(!e.to_string().contains("wave"), "no wave in its events: {e}");
        }
    }
}
