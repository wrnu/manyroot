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
        limit: Default::default(),
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
    assert_eq!(outputs.len(), project.items.len() + 2, "two pages, and each item");
    for (what, text) in &outputs {
        assert!(text.len() <= MAX_RESULT_BYTES, "{what}: {} bytes", text.len());
        assert!(text.len() < PI_MCP_OUTPUT_MAX_BYTES);
        assert_eq!(
            crate::driver::bounded(text.clone(), crate::driver::ResultLimit::Pi).1,
            None,
            "{what}: the guard leaves it whole"
        );
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
    for (item, (_, text)) in project.items.iter().zip(&outputs[2..]) {
        let shown: Value = serde_json::from_str(text).expect("JSON");
        let issue = std::fs::read_to_string(&item.text).expect("the item's text");
        assert!(shown["text"].as_str().is_some_and(|t| t.contains(issue.trim())), "{} in full", item.id);
    }
}

/// Task `fb08637d1101` (note `7c1588b84d87`): with every ledger item released, the whole task, the spec and
/// all 15 items in full, is read in at most 2 calls, task and task with page 2, each under Pi's cap; each item
/// on the page the index says.
#[test]
fn the_whole_ledger_task_reads_in_two_calls_under_pis_cap() {
    let project = ledger();
    let data = hub_data(&project, ANY_AGENT, false).expect("the data");
    let items: Vec<String> = data["items"].as_array().expect("items").iter().map(item_statement).collect();
    assert_eq!(items.len(), 15);
    let spec = data["spec"].as_str();
    let first = task_view(spec, &items, None, 1).expect("page 1");
    let count = first["pages"].as_u64().expect("pages") as usize;
    assert!(count <= 2, "{count} pages");
    let pages: Vec<Value> = (1..=count).map(|p| task_view(spec, &items, None, p).expect("a page")).collect();
    let mut read = Vec::new();
    for (k, page) in pages.iter().enumerate() {
        let n = page.to_string().len();
        assert!(n <= crate::driver::MAX_RESULT_BYTES, "page {}: {n} bytes", k + 1);
        for text in page["texts"].as_array().expect("texts") {
            read.push((k + 1, text.as_str().expect("text").to_string()));
        }
    }
    assert_eq!(pages[0]["spec"], spec.expect("a spec"), "the spec, whole, on page 1");
    assert_eq!(read.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(), items, "every item in full, in order");
    for ((page, _), entry) in read.iter().zip(pages[0]["items"].as_array().expect("the index")) {
        assert_eq!(entry["page"], *page, "{entry}");
    }
    if count > 1 {
        assert!(pages[0]["note"].as_str().is_some_and(|n| n.contains("page set")), "{}", pages[0]);
        assert!(pages[1].get("spec").is_none(), "the spec once");
    }
    assert!(task_view(spec, &items, None, count + 1).expect_err("past the last").contains("pages, from 1"));
}

/// Items that fit go on one page with the spec; ones that don't, on the pages after, none lost.
#[test]
fn the_task_pages_hold_every_item_whatever_their_size() {
    let item = |i: usize, body: usize| {
        item_statement(&json!({"id": format!("i-{i}"), "kind": "feature", "title": "T", "text": "x".repeat(body)}))
    };
    let small: Vec<String> = (0..3).map(|i| item(i, 100)).collect();
    let one = task_view(Some("spec"), &small, None, 1).expect("one page");
    assert_eq!((one["pages"].as_u64(), one.get("note")), (Some(1), None));
    assert_eq!(one["texts"], json!(small));
    let big: Vec<String> = (0..9).map(|i| item(i, 6_000)).collect();
    let first = task_view(Some("spec"), &big, None, 1).expect("page 1");
    let count = first["pages"].as_u64().expect("pages") as usize;
    assert!(count >= 3, "{count}");
    let mut texts = Vec::new();
    for p in 1..=count {
        let page = task_view(Some("spec"), &big, None, p).expect("a page");
        assert!(page.to_string().len() <= crate::driver::MAX_RESULT_BYTES);
        texts.extend(page["texts"].as_array().expect("texts").iter().filter_map(|t| t.as_str().map(str::to_string)));
    }
    assert_eq!(texts, big);
}

#[test]
fn task_shows_one_item_by_id_and_refuses_one_not_released() {
    let item = |id: &str, revises: &[&str]| {
        item_statement(
            &json!({"id": id, "kind": "bug", "title": "A (title): with marks", "text": "Body.", "revises": revises}),
        )
    };
    let items = vec![item("a-1", &[]), item("b-2", &["a-1", "c"])];
    let index = task_view(Some("the spec"), &items, None, 1).expect("the index");
    assert_eq!(
        index["items"],
        json!([
            {"id": "a-1", "kind": "bug", "title": "A (title): with marks", "page": 1},
            {"id": "b-2", "kind": "bug", "title": "A (title): with marks", "changes": ["a-1", "c"], "page": 1},
        ])
    );
    let one = task_view(Some("the spec"), &items, Some("b-2"), 1).expect("one item");
    assert_eq!(one, json!({"item": "b-2", "text": items[1]}));
    assert!(task_view(None, &items, Some("a"), 1).expect_err("a prefix isn't an id").contains("no released item"));
    // A statement of another shape is shown whole in the index.
    let odd = vec!["not an item line".to_string()];
    assert_eq!(
        task_view(None, &odd, None, 1).expect("the index")["items"],
        json!([{"text": "not an item line", "page": 1}])
    );
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
    assert!(err.contains("the task tool's result for page 1 of the task would be"), "{err}");
}

/// Task `a5253414e178`: the index reads an item's revisions from its first line, which the hub writes, so an
/// issue text whose last line reads like a revision list changes nothing, and a real one is found however the
/// issue ends.
#[test]
fn an_items_revisions_dont_depend_on_its_texts_wording() {
    let statement = |text: &str, revises: &[&str]| {
        item_statement(&json!({"id": "x", "kind": "change", "title": "T", "text": text, "revises": revises}))
    };
    let index = |s: String| task_view(None, &[s], None, 1).expect("the index")["items"][0].clone();
    let lookalike = statement("Body.\n\nThis changes the requirement of: forged.", &[]);
    assert_eq!(index(lookalike), json!({"id": "x", "kind": "change", "title": "T", "page": 1}));
    let real = statement("Body ending in a list:\n- one\n- two", &["a-1", "b"]);
    assert!(real.starts_with("Item x (change; changes a-1, b): T\n"), "{real}");
    assert!(real.ends_with("This changes the requirement of: a-1, b."), "still said in words: {real}");
    assert_eq!(index(real), json!({"id": "x", "kind": "change", "title": "T", "changes": ["a-1", "b"], "page": 1}));
}

/// A git diff of `n` new files, each `lines` lines long: its name-status (`-z`) and its patch.
fn big_diff(n: usize, lines: usize) -> (String, String) {
    let (mut status, mut patch) = (String::new(), String::new());
    for i in 0..n {
        let path = format!("pkg/m{i:03}.py");
        status.push_str(&format!("A\0{path}\0"));
        patch.push_str(&format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{lines} @@\n"
        ));
        for l in 0..lines {
            patch.push_str(&format!("+x_{l} = \"{}\"\n", "y".repeat(40)));
        }
    }
    (status, patch)
}

/// Every page of `files`' diff, from 1, each checked under the bound.
fn all_pages(files: &[FileDiff]) -> Vec<Value> {
    let first = diff_page(files, 1, json!({})).expect("page 1");
    let count = first["pages"].as_u64().unwrap_or(1) as usize;
    let pages: Vec<Value> = (1..=count).map(|p| diff_page(files, p, json!({"base": "b"})).expect("a page")).collect();
    for (p, v) in pages.iter().enumerate() {
        let n = v.to_string().len();
        assert!(n <= crate::driver::MAX_RESULT_BYTES, "page {}: {n} bytes", p + 1);
    }
    assert!(diff_page(files, count + 1, json!({})).expect_err("past the last").contains(&format!("{count} pages")));
    pages
}

#[test]
fn a_diff_splits_per_file_with_its_changes() {
    let (status, patch) = big_diff(3, 2);
    let files = split_diff(&status, &patch);
    let paths: Vec<(&str, &str)> = files.iter().map(|f| (f.path.as_str(), f.change.as_str())).collect();
    assert_eq!(paths, [("pkg/m000.py", "A"), ("pkg/m001.py", "A"), ("pkg/m002.py", "A")]);
    assert_eq!(files.iter().map(|f| f.text.as_str()).collect::<String>(), patch, "nothing lost");
    assert!(split_diff("", "").is_empty());
}

/// Task `24a43694bbbf`: a path is name-status's, never parsed from git's header, so one holding ` b/` keeps its
/// name; and a type change's two sections are one file's, so the files after it keep their own changes.
#[test]
fn a_diffs_paths_and_changes_are_git_name_status() {
    let status = "M\0x b/y\0T\0link\0D\0gone\0";
    let patch = "diff --git a/x b/y b/x b/y\n--- a/x b/y\n+++ b/x b/y\n@@ -1 +1 @@\n-a\n+b\n\
                 diff --git a/link b/link\ndeleted file mode 100644\n--- a/link\n+++ /dev/null\n@@ -1 +0,0 @@\n-hi\n\
                 diff --git a/link b/link\nnew file mode 120000\n--- /dev/null\n+++ b/link\n@@ -0,0 +1 @@\n+y\n\
                 diff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n@@ -1 +0,0 @@\n-z\n";
    let files = split_diff(status, patch);
    let paths: Vec<(&str, &str)> = files.iter().map(|f| (f.path.as_str(), f.change.as_str())).collect();
    assert_eq!(paths, [("x b/y", "M"), ("link", "T"), ("gone", "D")]);
    assert!(files[1].text.contains("-hi\n") && files[1].text.contains("+y\n"), "both sections: {}", files[1].text);
    assert_eq!(files.iter().map(|f| f.text.as_str()).collect::<String>(), patch, "nothing lost");
}

/// Finding `f6bb7565bb90`: a diff is measured without encoding it, the same as serde_json counts it.
#[test]
fn a_texts_json_size_is_counted_as_serde_json_writes_it() {
    let all: String = (0u32..0x80).filter_map(char::from_u32).chain("é€😀\u{7f}\u{2028}".chars()).collect();
    for ch in all.chars() {
        let s = ch.to_string();
        assert_eq!(escaped_len(&s), json_len(&s) - 2, "{ch:?}");
    }
    assert_eq!(escaped_len(&all), json_len(&all) - 2);
}

/// Task `24a43694bbbf`, from review `f2869acaafee`: every file a page shows is named in its index, however many
/// files and however long their paths. The reviewer's adversarial case: a 3,000-line file of quotes, backslashes
/// and control characters at a 1,000-byte path, then 400 small files, every seventh with a 900-byte path.
#[test]
fn every_file_a_page_shows_is_in_its_index() {
    let long = format!("{}/big.txt", "d".repeat(990));
    let line = "+\"q\" \\ \u{1} tail\n";
    let mut files = vec![FileDiff {
        path: long.clone(),
        change: "A".into(),
        text: format!("diff --git a/{long} b/{long}\n{}", line.repeat(3000)),
    }];
    for i in 0..400 {
        let path = if i % 7 == 0 { format!("{}/f{i}.py", "e".repeat(890)) } else { format!("pkg/f{i}.py") };
        files.push(FileDiff { text: format!("diff --git a/{path} b/{path}\n+x = {i}\n"), path, change: "A".into() });
    }
    let pages = all_pages(&files);
    assert!(pages.len() > 2, "{} pages", pages.len());
    let mut shown = 0;
    for (k, p) in pages.iter().enumerate() {
        let listed: Vec<&str> =
            p["files"].as_array().expect("files").iter().filter_map(|f| f["path"].as_str()).collect();
        for f in &files {
            if p["diff"].as_str().expect("text").contains(&format!("diff --git a/{} ", f.path)) {
                assert!(listed.contains(&f.path.as_str()), "page {}: {} shown, not listed", k + 1, f.path);
                shown += 1;
            }
        }
    }
    assert_eq!(shown, files.len(), "each file's diff starts on one page");
}

/// Finding `e97ef2cd6d6e`: a diff over the bound comes in pages, in order, with nothing lost: a small one is
/// one page, whole; a file over a page alone goes across pages, each part naming it; and the files after it are
/// still on later pages, each file in the index with its pages.
#[test]
fn every_files_diff_is_on_some_page_however_large() {
    let (status, patch) = big_diff(3, 5);
    let files = split_diff(&status, &patch);
    let small = diff_page(&files, 1, json!({"base": "b"})).expect("one page");
    assert_eq!(small["diff"], patch.as_str());
    assert!(small.get("pages").is_none() && small.get("note").is_none(), "{small}");
    assert_eq!(small["files"][1], json!({"path": "pkg/m001.py", "change": "A", "page": 1}));

    // One 400-line file, about 22 KB, then two small ones: the case the finding shows.
    let (s1, p1) = big_diff(1, 400);
    let (s2, p2) = big_diff(3, 3);
    let mut files = split_diff(&s1, &p1);
    files.extend(split_diff(&s2, &p2).into_iter().skip(1).map(|mut f| {
        f.path = f.path.replace("m00", "n00");
        f.text = f.text.replace("m00", "n00");
        f
    }));
    let pages = all_pages(&files);
    assert!(pages.len() >= 2, "{} pages", pages.len());
    let text: String = pages.iter().map(|p| p["diff"].as_str().expect("text")).collect();
    for f in &files {
        for line in f.text.lines() {
            assert!(text.contains(line), "{}: {line} is on no page", f.path);
        }
    }
    assert!(pages[1]["diff"].as_str().expect("text").starts_with("[pkg/m000.py: part 2 of"), "{}", pages[1]["diff"]);
    let big = &pages[0]["files"][0];
    assert_eq!((&big["path"], &big["page"]), (&json!("pkg/m000.py"), &json!(1)));
    assert!(big["last_page"].as_u64().is_some_and(|l| l >= 2), "{big}");
    assert!(pages[0]["note"].as_str().is_some_and(|n| n.contains("page set")), "{}", pages[0]);
    // The file after it is named on the first page, with its page.
    let after = pages[0]["files"].as_array().expect("files").iter().find(|f| f["path"] == "pkg/n001.py").cloned();
    assert_eq!(after.and_then(|f| f["page"].as_u64()), Some(pages.len() as u64));

    // A single line longer than a page is cut inside it, never inside a character, and still all shown.
    let line = format!("+{}\n", "é".repeat(30_000));
    let one = vec![FileDiff { path: "wide.txt".into(), change: "M".into(), text: line.clone() }];
    let text: String = all_pages(&one).iter().map(|p| p["diff"].as_str().expect("text")).collect();
    assert_eq!(text.matches('é').count(), 30_000);
}

/// Task `a5253414e178`: past the bound, the index lists this page's files first and names as many others as
/// fit, with a count of the rest.
#[test]
fn the_index_of_a_large_diff_names_this_pages_files_first() {
    let (status, patch) = big_diff(600, 3);
    let files = split_diff(&status, &patch);
    let pages = all_pages(&files);
    let last = pages.last().expect("a page");
    let listed = last["files"].as_array().expect("files");
    let shown_here = last["diff"].as_str().expect("text").matches("diff --git ").count();
    let first_here = listed[0]["path"].as_str().expect("a path");
    assert!(last["diff"].as_str().expect("text").contains(&format!("b/{first_here}\n")), "this page's file first");
    assert!(listed.len() >= shown_here);
    assert_eq!(listed.len() as u64 + last["files_not_listed"].as_u64().unwrap_or(0), 600);
}

#[test]
fn an_integrate_that_stops_lists_its_conflicts_within_the_bound() {
    let conflict = |i: usize, regions: usize| {
        json!({"path": format!("pkg/m{i:04}.py"), "base": {"hash": "h"}, "into": {"hash": "i"}, "from": Value::Null,
               "offered": [{"resolution": "ab12", "author": "k", "author_name": "agent-1", "result": Value::Null}],
               "text_conflicts": (0..regions).map(|r| json!([r * 10, r * 10 + 1])).collect::<Vec<_>>()})
    };
    assert_eq!(conflicts_error(&json!({"conflicts": []})), None);
    let one = conflicts_error(&json!({"step": "proposal 1a2b", "conflicts": [conflict(0, 1)]})).expect("an error");
    assert!(one.starts_with("merging proposal 1a2b stopped at 1 conflicts"), "{one}");
    assert!(
        one.contains(
            r#"{"from":"deleted","into":"present","offered":["ab12"],"path":"pkg/m0000.py","regions":[[0,1]]}"#
        ),
        "{one}"
    );
    let many: Vec<Value> = (0..2000).map(|i| conflict(i, 1)).collect();
    let err = conflicts_error(&json!({"conflicts": many})).expect("an error");
    assert!(err.len() <= crate::driver::MAX_RESULT_BYTES, "{} bytes", err.len());
    assert!(err.contains("stopped at 2000 conflicts") && err.contains("more not shown"), "{}", &err[..200]);
    // A conflict too large to show whole keeps its path, and the next one is still shown (task a5253414e178).
    let huge = conflicts_error(&json!({"conflicts": [conflict(0, 5000), conflict(1, 1)]})).expect("an error");
    assert!(huge.len() <= crate::driver::MAX_RESULT_BYTES, "{} bytes", huge.len());
    assert!(huge.contains(r#""path":"pkg/m0000.py","regions":"too many to show""#), "{huge}");
    assert!(huge.contains(r#""path":"pkg/m0001.py","regions":[[0,1]]"#) && !huge.contains("more not shown"), "{huge}");
}
