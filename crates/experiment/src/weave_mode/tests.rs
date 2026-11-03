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
