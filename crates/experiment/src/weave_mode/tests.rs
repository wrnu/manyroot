use super::*;

#[test]
fn a_path_an_agent_names_is_plain() {
    for ok in ["tiny/stats.py", "tests/test_x.py", "a/b/c.txt", "README.md", "a/x.git"] {
        assert!(plain_path(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "/etc/passwd",
        "../x",
        "a/../../x",
        "a/./b",
        "a//b",
        "a/",
        ".git/config",
        ".GIT/config",
        "a/.git/hooks/pre-commit",
        ".bracken/identities/x",
        ".bracken-x",
        "a\\b",
        "a\nb",
        "a\0b",
    ] {
        assert!(plain_path(bad).is_err(), "{bad:?}");
    }
    assert!(plain_path(&"a".repeat(256)).is_err(), "a part over 255 bytes");
    assert!(plain_path(&["a"; 600].join("/")).is_err(), "a path over 1024 bytes");
}

#[test]
fn a_project_releases_at_the_start_and_once_more_at_most() {
    assert_eq!(change_share(&[0.0, 0.0]), Ok(None));
    assert_eq!(change_share(&[0.0, 0.5, 0.5]), Ok(Some(0.5)));
    assert!(change_share(&[0.0, 0.3, 0.6]).is_err(), "two release points after the start");
    // 2 agents x 3 turns: half the run is 3 agent-turns; a share between rounds up, never early.
    assert_eq!(release_after(0.5, 3, 2), 3);
    assert_eq!(release_after(0.4, 3, 2), 3);
    assert_eq!(release_after(0.34, 3, 2), 3);
    assert_eq!(release_after(0.33, 3, 2), 2);
}

#[test]
fn a_reference_is_a_decision_id_or_a_manifest() {
    let hex = "0123456789abcdef".repeat(4);
    assert!(reference(&hex).is_ok());
    assert!(reference(&hex[..8]).is_ok());
    assert!(reference(&format!("blake3:{hex}")).is_ok());
    for bad in ["--note=x", "blake3:00", "HEAD", "0123456", &format!("blake3:{}", hex.to_uppercase())] {
        assert!(reference(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_tools_never_mention_how_runs_are_judged() {
    let tools = serde_json::to_string(&WeaveMode.tools(Task::Phase1)).expect("JSON").to_lowercase();
    for word in ["experiment", "metric", "measure", "token", "score", "grader", "hidden", "crdt", "bracken"] {
        assert!(!tools.contains(word), "a tool mentions {word:?}");
    }
}

/// The change goes out after exactly ceil(share x turns x agents) agent-turns, for every two-digit share
/// (finding d8167e497c9d): 0.1 x 3 x 10 is 3, not 4.
#[test]
fn the_change_goes_out_neither_early_nor_late() {
    assert_eq!(release_after(0.1, 3, 10), 3);
    assert_eq!(release_after(0.05, 6, 10), 3);
    assert_eq!(release_after(0.07, 10, 10), 7);
    for hundredths in 1..100u32 {
        let share: f64 = format!("0.{hundredths:02}").parse().expect("a share");
        for turns in 1..=20 {
            for agents in 1..=10usize {
                let exact = (hundredths * turns * agents as u32).div_ceil(100);
                assert_eq!(release_after(share, turns, agents), exact, "{share} x {turns} x {agents}");
            }
        }
    }
}

/// A decision comes after all its parents, through forks and merges, each once.
#[test]
fn decisions_are_ordered_after_their_parents() {
    let d = |pairs: &[(&str, &[&str])]| -> std::collections::BTreeMap<String, (String, Vec<String>)> {
        pairs
            .iter()
            .map(|(id, ps)| (id.to_string(), (format!("m-{id}"), ps.iter().map(|p| p.to_string()).collect())))
            .collect()
    };
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // A fork settled twice, criss-cross: m and n both merge x and y.
    let log = d(&[
        ("seed", &[]),
        ("x", &["seed"]),
        ("y", &["seed"]),
        ("m", &["x", "y"]),
        ("n", &["y", "x"]),
        ("z", &["seed"]),
    ]);
    let order = ancestry_order(&log, &ids(&["m", "n"])).expect("an order");
    assert_eq!(order.len(), 5, "each once, and nothing outside the heads' past: {order:?}");
    let at = |id: &str| order.iter().position(|o| o == id).expect("placed");
    for (id, parents) in [("x", &["seed"][..]), ("y", &["seed"]), ("m", &["x", "y"]), ("n", &["x", "y"])] {
        assert!(parents.iter().all(|p| at(p) < at(id)), "{id} after {parents:?}: {order:?}");
    }
    assert!(ancestry_order(&log, &ids(&["nope"])).is_err());
}

/// The sweep of earlier checkouts' old trees runs only while the tree is a working checkout, its
/// repository's files exactly (task af9ef395181b): with no repository, or with files the repository doesn't
/// hold, an old tree may be the only copy of the agent's work, and stays. Every tool that checks out needs a
/// clean tree first, so this reaches the sweep directly.
#[test]
fn old_trees_are_swept_only_from_a_working_checkout() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let config = StoreConfig {
        dir: tmp.path().to_path_buf(),
        bracken: PathBuf::from("/nonexistent/bracken"),
        git: crate::driver::git_binary().expect("git"),
        identity: "agent-0".into(),
        trace: false,
        task: Task::Phase1,
        accept_rule: Default::default(),
    };
    let mut store = WeaveStore { config, runs: 0 };
    let (tree, old) = (store.tree(), tmp.path().join("tree.old.0"));
    std::fs::create_dir_all(old.join("tiny")).expect("an old tree");
    std::fs::write(old.join("tiny/zero.py"), "NAME = 'zero'\n").expect("its file");
    std::fs::create_dir(&tree).expect("the tree");
    std::fs::write(tree.join("seed.py"), "SEED = 1\n").expect("a file");

    store.sweep_old_trees();
    assert!(old.exists(), "a tree with no repository: the old tree stays");
    store.git_start("seed").expect("the tree's repository");
    std::fs::write(tree.join("stray.py"), "STRAY = 1\n").expect("a file git doesn't hold");
    store.sweep_old_trees();
    assert!(old.exists(), "a tree with files its repository doesn't hold: the old tree stays");
    std::fs::remove_file(tree.join("stray.py")).expect("the stray file");
    store.sweep_old_trees();
    assert!(!old.exists(), "a working checkout: the old tree goes");
}

/// The ledger project.
fn ledger() -> Project {
    Project::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("projects/ledger")).expect("the ledger project")
}

/// Task `b58cf9180030` (report `b7b35a307aea`): with every ledger item released, each result the task tool can
/// give, the index and each item in full, is under Pi's MCP output cap as the driver sends it, so an agent
/// reads the whole spec and every item; and the index names every item, the change's revisions included.
#[test]
fn every_ledger_task_result_is_under_pis_cap_with_all_items_released() {
    use crate::driver::{MAX_RESULT_BYTES, PI_MCP_OUTPUT_MAX_BYTES};
    let project = ledger();
    let outputs = task_outputs(&project).expect("the outputs");
    assert_eq!(outputs.len(), project.items.len() + 1, "the index, and each item");
    for (what, text) in &outputs {
        assert!(text.len() <= MAX_RESULT_BYTES, "{what}: {} bytes", text.len());
        assert!(text.len() < PI_MCP_OUTPUT_MAX_BYTES);
        assert_eq!(crate::driver::bounded(text.clone(), MAX_RESULT_BYTES).1, None, "{what}: the guard leaves it whole");
    }
    check_task_outputs(&project).expect("the preflight passes");
    let index: Value = serde_json::from_str(&outputs[0].1).expect("JSON");
    let spec = std::fs::read_to_string(&project.spec).expect("the spec");
    assert_eq!(index["spec"], spec.as_str(), "the spec, whole");
    let ids: Vec<&str> = index["items"].as_array().expect("items").iter().filter_map(|i| i["id"].as_str()).collect();
    let want: Vec<&str> = project.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, want, "every item, in order");
    for (entry, item) in index["items"].as_array().expect("items").iter().zip(&project.items) {
        assert_eq!(entry["kind"], item.kind.name(), "{}", item.id);
        assert_eq!(entry["title"], item.title.as_str(), "{}", item.id);
        let changes: Vec<&str> = entry["changes"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        assert_eq!(changes, item.revises, "{}", item.id);
    }
    for (item, (_, text)) in project.items.iter().zip(&outputs[1..]) {
        let shown: Value = serde_json::from_str(text).expect("JSON");
        let issue = std::fs::read_to_string(&item.text).expect("the item's text");
        assert!(shown["text"].as_str().is_some_and(|t| t.contains(issue.trim())), "{} in full", item.id);
    }
}

#[test]
fn task_shows_one_item_by_id_and_refuses_one_not_released() {
    let item = |id: &str, revises: &[&str]| {
        item_statement(
            &json!({"id": id, "kind": "bug", "title": "A (title): with marks", "text": "Body.", "revises": revises}),
        )
    };
    let items = vec![item("a-1", &[]), item("b-2", &["a-1", "c"])];
    let index = task_view(Some("the spec"), &items, None).expect("the index");
    assert_eq!(
        index["items"],
        json!([
            {"id": "a-1", "kind": "bug", "title": "A (title): with marks"},
            {"id": "b-2", "kind": "bug", "title": "A (title): with marks", "changes": ["a-1", "c"]},
        ])
    );
    assert!(index["item_text"].as_str().is_some_and(|t| t.contains("item set to its id")));
    let one = task_view(Some("the spec"), &items, Some("b-2")).expect("one item");
    assert_eq!(one, json!({"item": "b-2", "text": items[1]}));
    assert!(task_view(None, &items, Some("a")).expect_err("a prefix isn't an id").contains("no released item"));
    // A statement of another shape is shown whole in the index.
    let odd = vec!["not an item line".to_string()];
    assert_eq!(task_view(None, &odd, None).expect("the index")["items"], json!([{"text": "not an item line"}]));
}

/// The preflight refuses a project whose spec would put the task tool's result over the cap.
#[test]
fn the_preflight_refuses_a_spec_over_the_cap() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let copy = dir.path().join("ledger");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("projects/ledger");
    let status = std::process::Command::new("cp").arg("-R").arg(&src).arg(&copy).status().expect("cp");
    assert!(status.success());
    let spec = copy.join("spec.md");
    let mut text = std::fs::read_to_string(&spec).expect("the spec");
    text.push_str(&"More words for the spec. ".repeat(crate::driver::PI_MCP_OUTPUT_MAX_BYTES / 20));
    std::fs::write(&spec, text).expect("written");
    let err = check_task_outputs(&Project::load(&copy).expect("still a project")).expect_err("over the cap");
    assert!(err.contains("the task tool's result for the index would be"), "{err}");
}

/// A diff of `n` files, each `lines` lines added.
fn big_diff(n: usize, lines: usize) -> (Vec<String>, String) {
    let mut files = Vec::new();
    let mut diff = String::new();
    for i in 0..n {
        let path = format!("pkg/m{i:03}.py");
        files.push(format!("A\t{path}"));
        diff.push_str(&format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{lines} @@\n"
        ));
        for l in 0..lines {
            diff.push_str(&format!("+x_{l} = \"{}\"\n", "y".repeat(40)));
        }
    }
    (files, diff)
}

/// Task `b58cf9180030`: a diff too large for one result keeps whole files' diffs, names the rest, and stays
/// under the bound; a small one is shown whole.
#[test]
fn changes_keeps_whole_file_diffs_under_the_bound_and_names_the_rest() {
    let (files, diff) = big_diff(3, 5);
    let small = changes_view("b".into(), files.iter().map(String::as_str).collect(), &diff);
    assert_eq!(small["diff"], diff.as_str());
    assert!(small.get("diff_not_shown").is_none() && small.get("note").is_none(), "{small}");

    let (files, diff) = big_diff(40, 20);
    assert!(diff.len() > 2 * crate::driver::PI_MCP_OUTPUT_MAX_BYTES);
    let v = changes_view("b".into(), files.iter().map(String::as_str).collect(), &diff);
    assert!(v.to_string().len() <= crate::driver::MAX_RESULT_BYTES, "{} bytes", v.to_string().len());
    assert_eq!(v["files"].as_array().expect("files").len(), 40, "every file listed");
    let shown = v["diff"].as_str().expect("a diff");
    let kept = shown.matches("diff --git ").count();
    assert!(kept > 0 && diff.starts_with(shown), "whole files' diffs, from the first");
    let left: Vec<&str> = v["diff_not_shown"].as_array().expect("named").iter().filter_map(Value::as_str).collect();
    assert_eq!(left.len(), 40 - kept);
    assert_eq!(left[0], format!("pkg/m{kept:03}.py"), "the first left out");
    assert!(v["note"].as_str().is_some_and(|n| n.contains("Call changes with path")), "{v}");
}

#[test]
fn diff_lists_files_briefly_when_they_dont_fit() {
    let file = |i: usize| {
        let h = format!("blake3:{}", "0".repeat(64));
        json!({"path": format!("pkg/m{i:04}.py"), "change": "modified", "before": {"hash": h, "mode": "100644"},
               "after": {"hash": h, "mode": "100644"}, "not_held": []})
    };
    let few = json!({"a": "x", "b": "y", "files": (0..3).map(file).collect::<Vec<_>>()});
    assert_eq!(diff_view(few.clone()), few, "whole when it fits");
    let many = json!({"a": "x", "b": "y", "files": (0..2000).map(file).collect::<Vec<_>>()});
    let v = diff_view(many);
    assert!(v.to_string().len() <= crate::driver::MAX_RESULT_BYTES, "{} bytes", v.to_string().len());
    let shown = v["files"].as_array().expect("files");
    assert_eq!(shown[0], json!({"path": "pkg/m0000.py", "change": "modified"}));
    assert_eq!(shown.len() as u64 + v["files_not_shown"].as_u64().expect("a count"), 2000);
}

#[test]
fn an_integrate_that_stops_lists_its_conflicts_within_the_bound() {
    let conflict = |i: usize| {
        json!({"path": format!("pkg/m{i:04}.py"), "base": {"hash": "h"}, "into": {"hash": "i"}, "from": Value::Null,
               "offered": [{"resolution": "ab12", "author": "k", "author_name": "agent-1", "result": Value::Null}],
               "text_conflicts": [[3, 4]]})
    };
    assert_eq!(conflicts_error(&json!({"conflicts": []})), None);
    let one = conflicts_error(&json!({"step": "proposal 1a2b", "conflicts": [conflict(0)]})).expect("an error");
    assert!(one.starts_with("merging proposal 1a2b stopped at 1 conflicts"), "{one}");
    assert!(
        one.contains(
            r#"{"from":"deleted","into":"present","offered":["ab12"],"path":"pkg/m0000.py","regions":[[3,4]]}"#
        ),
        "{one}"
    );
    let many: Vec<Value> = (0..2000).map(conflict).collect();
    let err = conflicts_error(&json!({"conflicts": many})).expect("an error");
    assert!(err.len() <= crate::driver::MAX_RESULT_BYTES, "{} bytes", err.len());
    assert!(err.contains("stopped at 2000 conflicts") && err.contains("more not shown"), "{}", &err[..200]);
}
