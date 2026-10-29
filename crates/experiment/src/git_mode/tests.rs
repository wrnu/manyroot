//! Unit tests: Git mode's argument rules and placement, its bundle checks, CI
//! and the review rule, on stores in this process.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::candidates::correct_programs;

fn config(dir: &Path, name: &str) -> StoreConfig {
    StoreConfig {
        task: Task::Phase1,
        dir: dir.join(name),
        bracken: PathBuf::from("/nonexistent/bracken"),
        git: crate::driver::git_binary().expect("git"),
        identity: name.into(),
        trace: true,
        accept_rule: Default::default(),
    }
}

fn session(name: &str) -> Session {
    Session { agent: 0, name: name.into(), quota_contributions: 1000, quota_bytes: 1 << 26, ..Session::default() }
}

/// A hub and copies, synced as the driver syncs them.
struct Fixture {
    _dir: tempfile::TempDir,
    hub: GitStore,
    copies: Vec<GitStore>,
}

impl Fixture {
    fn new(agents: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut hub = GitStore::new(config(dir.path(), "setter"));
        hub.setup(None).expect("hub setup");
        let seed = hub.export().expect("export");
        let copies = agents
            .iter()
            .map(|a| {
                let mut c = GitStore::new(config(dir.path(), a));
                c.setup(Some(&seed)).expect("copy setup");
                c
            })
            .collect();
        Fixture { _dir: dir, hub, copies }
    }

    fn sync(&mut self, i: usize) {
        let s = session(&self.copies[i].config.identity);
        let push = self.copies[i].push().expect("push");
        let have = push.meta["have"].clone();
        self.hub.receive(&s, push).expect("receive");
        let answer = self.hub.send(&s, &have).expect("send");
        self.copies[i].fetch(answer).expect("fetch");
    }

    fn tool(&mut self, i: usize, name: &str, args: Value) -> Result<Value, String> {
        if PR_TOOLS.contains(&name) {
            let checked = self.copies[i].validate(name, &args)?;
            let s = session(&self.copies[i].config.identity);
            self.hub.hub_tool(&s, name, &checked)
        } else {
            self.copies[i].tool(name, &args)
        }
    }

    /// Agent `i` adds `def` to `file` on a new `branch`, commits, syncs and opens a PR; its number.
    fn propose(&mut self, i: usize, branch: &str, file: &str, def: &str) -> u64 {
        self.tool(i, "switch", json!({"branch": branch, "create": true})).expect("switch");
        let lines = self.tool(i, "read", json!({"file": file})).expect("read")["lines"].as_u64().expect("lines");
        self.tool(i, "edit", json!({"file": file, "start": lines + 1, "end": lines, "text": def})).expect("edit");
        self.tool(i, "commit", json!({"message": format!("add to {file}")})).expect("commit");
        self.sync(i);
        let pr = self.tool(i, "pr_open", json!({"branch": branch, "title": "t"})).expect("open");
        pr["number"].as_u64().expect("a number")
    }

    fn review(&mut self, i: usize, n: u64, verdict: &str) {
        self.tool(i, "pr_review", json!({"number": n, "verdict": verdict})).expect("review");
    }

    fn merge(&mut self, i: usize, n: u64) -> Result<Value, String> {
        self.tool(i, "pr_merge", json!({"number": n}))
    }
}

/// Every program git started, by its trace, is git itself: never a shell, `git-upload-pack` or a hook.
fn only_git_started(trace: &str) {
    let programs: Vec<&str> = trace
        .lines()
        .filter_map(|l| l.split_once("trace: start_command: "))
        .filter_map(|(_, cmd)| cmd.split_whitespace().next())
        .collect();
    assert!(!programs.is_empty(), "the trace records the programs git starts");
    for p in programs {
        assert!(p.ends_with("/git-core/git"), "git started {p}");
    }
}

fn def(name: &str) -> String {
    let spec = Task::Phase1.spec_named(name).expect("a spec");
    format!("def {name} {} : {}", spec.arity, correct_programs(Task::Phase1, Version::ORIGINAL)[name])
}

#[test]
fn a_pr_merges_with_ci_and_another_agents_approval() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    assert!(f.merge(0, n).is_err_and(|e| e.contains("approved")), "no approval yet");
    assert!(f.tool(0, "pr_review", json!({"number": n, "verdict": "approve"})).is_err(), "the author can't review");
    f.review(1, n, "approve");
    let merged = f.merge(1, n).expect("merged");
    assert_eq!(f.hub.integrated().expect("main")[0].1, format!("{}\n", def("abs")));
    // A merge of the PR's head, not a fast-forward: main has two parents.
    let main = merged["main"].as_str().expect("main").to_string();
    let parents = f.hub.git(&["rev-list", "--parents", "-n1", &main], None).expect("parents");
    assert_eq!(parents.split_whitespace().count(), 3);
    assert!(f.merge(1, n).is_err(), "merged once");
}

#[test]
fn an_approval_counts_only_for_the_head_it_was_given_on() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    f.review(1, n, "approve");
    // The author pushes another commit: the approval is of the old head.
    let both = format!("{}\n{}", def("abs"), def("sq"));
    f.tool(0, "replace", json!({"file": "base.stk", "old": def("abs"), "new": both})).expect("replace");
    f.tool(0, "commit", json!({"message": "and sq"})).expect("commit");
    f.sync(0);
    assert!(f.merge(1, n).is_err_and(|e| e.contains("approved")));
    f.review(1, n, "approve");
    f.merge(1, n).expect("approved again");
}

#[test]
fn a_request_for_changes_blocks_until_that_reviewer_approves() {
    let mut f = Fixture::new(&["agent-a", "agent-b", "agent-c"]);
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    f.review(1, n, "approve");
    f.review(2, n, "request_changes");
    assert!(f.merge(1, n).is_err_and(|e| e.contains("agent-c")), "agent-c's request blocks");
    // Each reviewer counts once, by their latest review: b approving again changes nothing.
    f.review(1, n, "approve");
    assert!(f.merge(1, n).is_err());
    f.review(2, n, "approve");
    f.merge(1, n).expect("unblocked");
}

#[test]
fn ci_refuses_regressions_failing_changes_and_conflicts() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    f.review(1, n, "approve");
    f.merge(1, n).expect("abs on main");
    f.sync(0);
    f.sync(1);
    // A changed function that fails its public cases.
    let wrong = f.propose(1, "wrongsq", "base.stk", "def sq 1 : a");
    f.review(0, wrong, "approve");
    assert!(f.merge(0, wrong).is_err_and(|e| e.contains("`sq` is changed and fails")));
    // A regression: breaking abs, which passes on main.
    f.tool(1, "switch", json!({"branch": "breaks", "create": true})).expect("switch");
    f.tool(1, "replace", json!({"file": "base.stk", "old": def("abs"), "new": "def abs 1 : a"})).expect("replace");
    f.tool(1, "commit", json!({"message": "m"})).expect("commit");
    f.sync(1);
    let n =
        f.tool(1, "pr_open", json!({"branch": "breaks", "title": "t"})).expect("open")["number"].as_u64().expect("n");
    f.review(0, n, "approve");
    assert!(f.merge(0, n).is_err_and(|e| e.contains("`abs` passes on main and fails")));
    // Two PRs adding at the same place: the second conflicts once the first is in.
    let first = f.propose(0, "sq", "base.stk", &def("sq"));
    let second = f.propose(1, "inc", "base.stk", &def("inc"));
    f.review(1, first, "approve");
    f.review(0, second, "approve");
    f.merge(1, first).expect("first");
    assert!(f.merge(0, second).is_err_and(|e| e.contains("conflicts")));
    // The author merges main in, resolves, commits and syncs; with a new approval it merges.
    f.sync(1);
    f.tool(1, "switch", json!({"branch": "inc"})).expect("switch");
    assert_eq!(f.tool(1, "merge", json!({})).expect("merge")["merged"], false);
    let lines = f.tool(1, "read", json!({"file": "base.stk"})).expect("read")["lines"].as_u64().expect("lines");
    let resolved = format!("{}\n{}\n{}\n", def("abs"), def("sq"), def("inc"));
    f.tool(1, "edit", json!({"file": "base.stk", "start": 1, "end": lines, "text": resolved})).expect("edit");
    f.tool(1, "commit", json!({"message": "merge main"})).expect("commit");
    f.sync(1);
    f.review(0, second, "approve");
    f.merge(0, second).expect("second, resolved");
}

#[test]
fn only_the_setter_writes_tasks_md() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    assert!(f.tool(0, "edit", json!({"file": "TASKS.md", "start": 1, "end": 1, "text": "x"})).is_err());
    assert!(f.tool(0, "read", json!({"file": "TASKS.md"})).is_ok());
    // A copy whose worker went wrong commits TASKS.md anyway: CI refuses it.
    f.tool(0, "switch", json!({"branch": "tasks", "create": true})).expect("switch");
    std::fs::write(f.copies[0].work().join(TASKS), Task::Phase1.tasks_md(Task::Phase1.all())).expect("write");
    f.copies[0].git(&["commit", "--quiet", "--all", "--file=-"], Some(b"m")).expect("commit");
    f.sync(0);
    let n =
        f.tool(0, "pr_open", json!({"branch": "tasks", "title": "t"})).expect("open")["number"].as_u64().expect("n");
    f.review(1, n, "approve");
    assert!(f.merge(1, n).is_err_and(|e| e.contains("TASKS.md")));
    assert_eq!(f.hub.hub_version().expect("version"), Version::ORIGINAL);
    // The setter's release changes it, and a copy's check uses it from its next sync.
    f.hub.release_changes().expect("release");
    assert_eq!(f.copies[1].version_at("refs/remotes/hub/main").expect("version"), Version::ORIGINAL);
    f.sync(1);
    assert_eq!(f.copies[1].version_at("refs/remotes/hub/main").expect("version"), Task::Phase1.all());
}

#[test]
fn sync_sends_only_what_is_missing() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    f.propose(0, "abs", "base.stk", &def("abs"));
    // Nothing new either way: no bundle at all.
    let push = f.copies[0].push().expect("push");
    assert!(push.bytes.is_empty(), "the hub has the branch");
    let s = session("agent-a");
    let have = push.meta["have"].clone();
    f.hub.receive(&s, push).expect("receive");
    assert!(f.hub.send(&s, &have).expect("send").bytes.is_empty(), "the copy has main and its own PR");
    // Another copy gets the PR's head, once.
    let push = f.copies[1].push().expect("push");
    let answer = f.hub.send(&session("agent-b"), &push.meta["have"]).expect("send");
    assert!(!answer.bytes.is_empty());
    f.copies[1].fetch(answer).expect("fetch");
    assert!(f.copies[1].rev("refs/remotes/hub/pr-1").expect("rev").is_some());
    let push = f.copies[1].push().expect("push");
    assert!(f.hub.send(&session("agent-b"), &push.meta["have"]).expect("send").bytes.is_empty());
}

#[test]
fn the_hub_refuses_anything_but_a_checked_bundle_and_starts_no_shell() {
    let mut f = Fixture::new(&["agent-a"]);
    let trace = f.hub.config.dir.join("git-trace.log");
    std::fs::write(&trace, "").expect("trace");
    let canary = f.copies[0].config.dir.join("canary");
    let hostile: Vec<Vec<u8>> = vec![
        format!("gitdir: {}\n", f.copies[0].work().join(".git").display()).into_bytes(),
        b"PACK\x00\x00\x00\x02".to_vec(),
        format!("#!/bin/sh\ntouch {}\n", canary.display()).into_bytes(),
        Vec::new(),
    ];
    let before = f.hub.runs.len();
    for bytes in hostile {
        let empty = bytes.is_empty();
        let push = Transfer { meta: json!({"branches": {}}), bytes };
        assert!(f.hub.receive(&session("agent-a"), push).is_ok() == empty, "only an empty push is no push");
    }
    assert_eq!(f.hub.runs.len(), before, "nothing that isn't a bundle reaches git");
    // Bundle-shaped bytes whose pack isn't one are refused, and leave nothing behind.
    let fake = b"# v2 git bundle\n0123456789012345678901234567890123456789 refs/heads/x\n\nPACK".to_vec();
    assert!(f.hub.receive(&session("agent-a"), Transfer { meta: json!({}), bytes: fake }).is_err());
    assert!(f.hub.rev("refs/agents/agent-a/x").expect("rev").is_none());
    assert!(!canary.exists());
    only_git_started(&std::fs::read_to_string(&trace).expect("traced"));
    // A bundle whose refs aren't branches is refused before it is fetched.
    f.copies[0].git(&["update-ref", "refs/tags/x", "refs/remotes/hub/main"], None).expect("ref");
    let bytes = f.copies[0].make_bundle(&["refs/tags/x".into()], &[]).expect("bundle");
    let err = f.hub.receive(&session("agent-a"), Transfer { meta: json!({}), bytes }).expect_err("refused");
    assert!(err.contains("refusing ref"), "{err}");
}

#[test]
fn objects_that_fail_fsck_are_refused() {
    let mut f = Fixture::new(&["agent-a"]);
    // A commit with a malformed author line, which git's fsck refuses.
    let c = &mut f.copies[0];
    let tree = c.git(&["rev-parse", "refs/remotes/hub/main^{tree}"], None).expect("tree");
    let body = format!("tree {}author nobody\ncommitter nobody\n\nbad\n", tree);
    let bad =
        c.git(&["hash-object", "-t", "commit", "-w", "--literally", "--stdin"], Some(body.as_bytes())).expect("bad");
    c.git(&["update-ref", "refs/heads/bad", bad.trim()], None).expect("ref");
    let push = c.push().expect("push");
    assert!(!push.bytes.is_empty());
    assert!(f.hub.receive(&session("agent-a"), push).is_err());
    assert!(f.hub.rev("refs/agents/agent-a/bad").expect("rev").is_none());
}

// The argument rules.

/// Values an agent might try to smuggle into a command line (finding `455e12da3ecb` among them).
fn hostile(canary: &str) -> Vec<String> {
    vec![
        "-x".into(),
        format!("--receive-pack=touch {canary}/pwned"),
        format!("--upload-pack=touch {canary}/pwned"),
        format!("--output={canary}/out"),
        "--exec=x".into(),
        "-C".into(),
        format!("--git-dir={canary}"),
        "HEAD~1".into(),
        "@{u}".into(),
        "main..x".into(),
        "main".into(),
        "..".into(),
        "/".into(),
        "/etc/passwd".into(),
        ".git/config".into(),
        ".gitattributes".into(),
        "a\0b".into(),
        "a\nb".into(),
        "x".repeat(20_000),
        "ｍａｉｎ".into(),
    ]
}

/// Each tool's agent-chosen string slots, with a valid value for the others.
fn slots() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        ("read", json!({"file": "base.stk"}), "file"),
        ("edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": "t"}), "file"),
        ("edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": "t"}), "text"),
        ("replace", json!({"file": "base.stk", "old": "o", "new": "n"}), "file"),
        ("replace", json!({"file": "base.stk", "old": "o", "new": "n"}), "old"),
        ("replace", json!({"file": "base.stk", "old": "o", "new": "n"}), "new"),
        ("diff", json!({"against": "main"}), "against"),
        ("switch", json!({"branch": "b"}), "branch"),
        ("switch", json!({"branch": "b", "create": true}), "branch"),
        ("commit", json!({"message": "m"}), "message"),
        ("check", json!({"function": "abs"}), "function"),
        ("pr_open", json!({"branch": "b", "title": "t"}), "branch"),
        ("pr_open", json!({"branch": "b", "title": "t"}), "title"),
        ("pr_open", json!({"branch": "b", "title": "t", "body": "b"}), "body"),
        ("pr_comment", json!({"number": 1, "text": "t"}), "text"),
        ("pr_review", json!({"number": 1, "verdict": "approve"}), "verdict"),
        ("pr_review", json!({"number": 1, "verdict": "approve", "text": "t"}), "text"),
        ("pr_view", json!({"number": 1}), "number"),
        ("log", json!({"count": 1}), "count"),
    ]
}

/// Free text may hold these values: it goes on stdin or into a file, or is data in the PR record.
fn is_free_text(slot: &str) -> bool {
    matches!(slot, "text" | "old" | "new" | "message" | "title" | "body")
}

#[test]
fn every_tool_slot_is_in_the_table() {
    for t in GitMode.tools(Task::Phase1) {
        let props = t.input_schema["properties"].as_object().expect("properties");
        for (key, schema) in props {
            if schema["type"] == "string" {
                assert!(slots().iter().any(|(n, _, s)| *n == t.name && s == key), "{}.{key}", t.name);
            }
        }
    }
}

#[test]
fn hostile_arguments_are_refused_before_any_process_starts() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let canary = dir.path().join("canary");
    let mut copy = GitStore::new(config(dir.path(), "copy"));
    for (tool, args, slot) in slots() {
        for v in hostile(&canary.to_string_lossy()) {
            let valid = slot == "against" && v == "main";
            if valid || is_free_text(slot) && v.len() <= crate::args::MAX_TEXT && !v.contains('\0') {
                continue;
            }
            let mut args = args.clone();
            args[slot] = json!(v);
            let result = if PR_TOOLS.contains(&tool) { copy.validate(tool, &args) } else { copy.tool(tool, &args) };
            assert!(result.is_err(), "{tool}.{slot} accepted {:?}", &v[..v.len().min(40)]);
        }
    }
    assert!(copy.runs.is_empty(), "a refused call starts no process: {:?}", copy.runs);
    assert!(!canary.exists());
}

/// Whether `v` can't be read as an option, or as an argument of its own, anywhere in `argv`:
/// an argument that is `v`, or starts with `-` and holds it, is after `--end-of-options`,
/// or is the one `--option=<v>`.
fn placed_safely(argv: &[String], v: &str) -> bool {
    let end = argv.iter().position(|a| a == "--end-of-options");
    argv.iter().enumerate().all(|(i, a)| {
        let risky = a == v || (a.starts_with('-') && a.contains(v));
        !risky || end.is_some_and(|e| i > e) || a.split_once('=').is_some_and(|(o, x)| o.starts_with("--") && x == v)
    })
}

#[test]
fn placement_alone_keeps_hostile_values_from_being_options() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let canary = f.copies[0].config.dir.join("canary");
    std::fs::create_dir(&canary).expect("canary dir");
    f.propose(1, "theirs", "base.stk", &def("abs"));
    f.sync(0);
    for v in hostile(&canary.to_string_lossy()).into_iter().filter(|v| !v.contains('\0') && v.len() < 1000) {
        // Copy tools with every rule off: only placement stands between the value and an option.
        let before = f.copies[0].runs.len();
        for args in [
            json!({"branch": v}),
            json!({"branch": v, "create": true}),
            json!({"branch": v, "create": true, "from_pr": 1}),
        ] {
            let _ = f.copies[0].copy_tool("switch", &Args::unchecked(&args).expect("an object"));
        }
        for (tool, args) in [("diff", json!({"against": v})), ("check", json!({"function": v}))] {
            let _ = f.copies[0].copy_tool(tool, &Args::unchecked(&args).expect("an object"));
        }
        for argv in &f.copies[0].runs[before..] {
            assert!(placed_safely(argv, &v), "{v:?} isn't placed safely in {argv:?}");
        }
        // Hub tools, with the arguments as if a copy's checks had passed them.
        let before = f.hub.runs.len();
        let s = session("agent-a");
        let _ = prs::run(&mut f.hub, &s, "pr_open", &json!({"branch": v, "title": v, "body": v}));
        let _ = prs::run(&mut f.hub, &s, "pr_review", &json!({"number": 1, "verdict": v, "text": v}));
        let _ = prs::run(&mut f.hub, &s, "pr_comment", &json!({"number": 1, "text": v}));
        for argv in &f.hub.runs[before..] {
            assert!(placed_safely(argv, &v), "{v:?} isn't placed safely in {argv:?}");
        }
    }
    // A commit message on stdin, whatever it holds.
    let _ = f.copies[0].copy_tool("switch", &Args::unchecked(&json!({"branch": "mine", "create": true})).expect("o"));
    let message = format!("--receive-pack=touch {}/pwned", canary.display());
    f.tool(0, "edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": def("sq")})).expect("edit");
    let before = f.copies[0].runs.len();
    f.tool(0, "commit", json!({"message": message})).expect("commit");
    assert!(f.copies[0].runs[before..].iter().all(|argv| !argv.iter().any(|a| a.contains("pwned"))));
    assert!(std::fs::read_dir(&canary).expect("canary").next().is_none(), "nothing was written through a value");
    let traced = std::fs::read_to_string(f.copies[0].config.dir.join("git-trace.log")).expect("traced");
    only_git_started(&traced);
    // A review whose verdict isn't one of the two counts as neither.
    let view = prs::run(&mut f.hub, &session("agent-a"), "pr_view", &json!({"number": 1})).expect("view");
    assert_eq!(view["approved"], false);
    assert!(view["blocked_by"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn the_authors_own_review_never_counts() {
    // The tool refuses an author's review; a record holding one anyway still doesn't count it.
    let review = |who: &str, verdict: &str| prs::Review {
        who: who.into(),
        verdict: verdict.into(),
        head: "h".into(),
        text: String::new(),
    };
    let mut pr = prs::Pr {
        number: 1,
        author: "agent-a".into(),
        branch: "b".into(),
        title: String::new(),
        body: String::new(),
        state: "open".into(),
        comments: Vec::new(),
        reviews: vec![review("agent-a", "approve")],
        merged_as: None,
    };
    assert!(!pr.approved("h"));
    pr.reviews.push(review("agent-b", "approve"));
    pr.reviews.push(review("agent-a", "request_changes"));
    assert!(pr.approved("h") && pr.blocked_by().is_empty());
}

/// A commit on the copy's `main` whose tree also holds `entry` (a `git mktree` line), on branch `branch`.
fn planted(c: &mut GitStore, branch: &str, entry: &str, replaces: &str) -> String {
    let listing = c.git(&["ls-tree", "refs/remotes/hub/main"], None).expect("ls-tree");
    let mut lines: Vec<String> =
        listing.lines().filter(|l| !l.ends_with(&format!("\t{replaces}"))).map(str::to_string).collect();
    lines.push(entry.to_string());
    let tree = c.git(&["mktree"], Some((lines.join("\n") + "\n").as_bytes())).expect("mktree");
    let commit = c.git(&["commit-tree", "-p", "refs/remotes/hub/main", tree.trim()], Some(b"m")).expect("commit");
    c.git(&["update-ref", &format!("refs/heads/{branch}"), commit.trim()], None).expect("ref");
    commit.trim().to_string()
}

fn blob(c: &mut GitStore, bytes: &[u8]) -> String {
    c.git(&["hash-object", "-w", "--stdin"], Some(bytes)).expect("blob").trim().to_string()
}

#[test]
fn only_regular_module_files_are_taken_in() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let outside = f._dir.path().join("outside-secret.txt");
    std::fs::write(&outside, "secret").expect("outside");
    let c = &mut f.copies[0];
    let link = blob(c, outside.to_string_lossy().as_bytes());
    let text = blob(c, b"def abs 1 : a\n");
    let main = c.rev("refs/remotes/hub/main").expect("rev").expect("main");
    let mktree_sub = c.git(&["mktree"], Some(format!("100644 blob {text}\tbase.stk\n").as_bytes())).expect("subtree");
    let cases = [
        ("link", format!("120000 blob {link}\tbase.stk"), "base.stk"),
        ("exec", format!("100755 blob {text}\tbase.stk"), "base.stk"),
        ("gitlink", format!("160000 commit {main}\tbase.stk"), "base.stk"),
        ("attrs", format!("100644 blob {text}\t.gitattributes"), ""),
        ("subdir", format!("040000 tree {}\tsrc", mktree_sub.trim()), ""),
        ("big", format!("100644 blob {}\tbase.stk", blob(c, &vec![b'#'; MAX_FILE + 1])), "base.stk"),
    ];
    for (branch, entry, replaces) in &cases {
        planted(&mut f.copies[0], branch, entry, replaces);
    }
    // Each branch on its own: every push holding one of them is refused, and no ref moves.
    for (branch, _, _) in &cases {
        let mut push = f.copies[0].push().expect("push");
        let refs: Vec<String> = vec![format!("refs/heads/{branch}")];
        push.bytes = f.copies[0].make_bundle(&refs, &["--remotes=hub".to_string()]).expect("bundle");
        assert!(push.bytes.len() < 64 * 1024, "a small bundle");
        push.meta = json!({"branches": {}});
        let err = f.hub.receive(&session("agent-a"), push).expect_err(branch);
        assert!(err.contains("refusing commit"), "{branch}: {err}");
        assert!(f.hub.rev(&format!("refs/agents/agent-a/{branch}")).expect("rev").is_none(), "{branch}");
        assert!(f.hub.git(&["for-each-ref", "refs/incoming"], None).expect("refs").is_empty());
    }
    // The same check guards a copy's fetch: another copy refuses a "main" holding a link.
    planted(&mut f.copies[0], "main", &cases[0].1, "base.stk");
    let bytes = f.copies[0].make_bundle(&["refs/heads/main".into()], &[]).expect("bundle");
    let before = f.copies[1].rev("refs/remotes/hub/main").expect("rev");
    let err = f.copies[1].fetch(Transfer { meta: json!({"refs": {}}), bytes }).expect_err("refused");
    assert!(err.contains("refusing commit"), "{err}");
    assert_eq!(f.copies[1].rev("refs/remotes/hub/main").expect("rev"), before);
}

#[test]
fn a_copy_never_follows_a_link_in_its_working_copy() {
    let mut f = Fixture::new(&["agent-a"]);
    let outside = f._dir.path().join("outside-secret.txt");
    std::fs::write(&outside, "secret").expect("outside");
    let base = f.copies[0].work().join("base.stk");
    std::fs::remove_file(&base).expect("the module file");
    std::os::unix::fs::symlink(&outside, &base).expect("a planted link");
    assert!(f.tool(0, "read", json!({"file": "base.stk"})).is_err());
    assert!(f.tool(0, "edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": "x"})).is_err());
    assert!(f.tool(0, "replace", json!({"file": "base.stk", "old": "secret", "new": "x"})).is_err());
    assert!(f.tool(0, "check", json!({"function": "abs"})).is_err());
    assert_eq!(std::fs::read_to_string(&outside).expect("outside"), "secret");
    // And git checks a link out as a plain file (core.symlinks=false).
    std::fs::remove_file(&base).expect("the link");
    let c = &mut f.copies[0];
    let link = blob(c, outside.to_string_lossy().as_bytes());
    let commit = planted(c, "local", &format!("120000 blob {link}\tbase.stk"), "base.stk");
    c.git(&["checkout", "--quiet", "--force", "--detach", "--end-of-options", &commit, "--"], None).expect("checkout");
    assert!(std::fs::symlink_metadata(&base).expect("checked out").file_type().is_file());
}

#[test]
fn ci_reads_no_module_over_the_limit() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    // Planted at the hub directly, as if a check had been missed: CI refuses it from its size alone.
    let big = blob(&mut f.hub, &vec![b'#'; MAX_FILE + 1]);
    let main = f.hub.rev("refs/heads/main").expect("rev").expect("main");
    let listing = f.hub.git(&["ls-tree", &main], None).expect("ls-tree");
    let mut lines: Vec<String> = listing.lines().filter(|l| !l.ends_with("\tbase.stk")).map(str::to_string).collect();
    lines.push(format!("100644 blob {big}\tbase.stk"));
    let tree = f.hub.git(&["mktree"], Some((lines.join("\n") + "\n").as_bytes())).expect("mktree");
    let commit = f.hub.git(&["commit-tree", "-p", &main, tree.trim()], Some(b"m")).expect("commit");
    f.hub.git(&["update-ref", "refs/agents/agent-a/big", commit.trim()], None).expect("ref");
    let n = f.tool(0, "pr_open", json!({"branch": "big", "title": "t"})).expect("open")["number"].as_u64().expect("n");
    let view = f.tool(1, "pr_view", json!({"number": n})).expect("view");
    assert_eq!(view["ci"]["pass"], false);
    assert!(view["ci"]["details"][0].as_str().is_some_and(|d| d.contains("over the limit")), "{view}");
    assert!(f.hub.modules_at(commit.trim()).is_err());
    f.review(1, n, "approve");
    assert!(f.merge(1, n).is_err());
}

/// A branch `branch` in copy `i` whose `base.stk` is a link out of every store; its commit.
fn link_branch(f: &mut Fixture, i: usize, branch: &str) -> String {
    let outside = f._dir.path().join("outside-secret.txt");
    std::fs::write(&outside, "secret").expect("outside");
    let link = blob(&mut f.copies[i], outside.to_string_lossy().as_bytes());
    planted(&mut f.copies[i], branch, &format!("120000 blob {link}\tbase.stk"), "base.stk")
}

#[test]
fn a_refused_push_leaves_nothing_a_later_push_can_name() {
    let mut f = Fixture::new(&["agent-a"]);
    let bad = link_branch(&mut f, 0, "evil");
    let push = f.copies[0].push().expect("push");
    assert!(f.hub.receive(&session("agent-a"), push).is_err());
    // The refused objects never joined the hub's store.
    assert!(!f.hub.has_commit(&bad).expect("checked"), "the quarantine was dropped");
    assert!(!f.hub.config.dir.join("quarantine").exists());
    // The two-push bypass: a later push names the refused commit with no bundle.
    let meta = json!({"branches": {"evil": bad}});
    assert!(f.hub.receive(&session("agent-a"), Transfer { meta, bytes: Vec::new() }).is_err());
    assert!(f.hub.rev("refs/agents/agent-a/evil").expect("rev").is_none());
    // A bad commit in the hub's store by any other route still gets no ref from a push.
    let hub_bad = {
        let h = &mut f.hub;
        let main = h.rev("refs/heads/main").expect("rev").expect("main");
        let listing = h.git(&["ls-tree", &main], None).expect("ls-tree");
        let link = blob(h, b"/etc/passwd");
        let mut lines: Vec<String> =
            listing.lines().filter(|l| !l.ends_with("\tbase.stk")).map(str::to_string).collect();
        lines.push(format!("120000 blob {link}\tbase.stk"));
        let tree = h.git(&["mktree"], Some((lines.join("\n") + "\n").as_bytes())).expect("mktree");
        h.git(&["commit-tree", "-p", &main, tree.trim()], Some(b"m")).expect("commit").trim().to_string()
    };
    let meta = json!({"branches": {"stored": hub_bad}});
    assert!(f.hub.receive(&session("agent-a"), Transfer { meta, bytes: Vec::new() }).is_err());
    assert!(f.hub.rev("refs/agents/agent-a/stored").expect("rev").is_none());
}

#[test]
fn refused_pushes_count_toward_the_quota() {
    let mut f = Fixture::new(&["agent-a"]);
    link_branch(&mut f, 0, "evil");
    let push = f.copies[0].push().expect("push");
    let size = push.bytes.len() as u64;
    let s = Session { quota_bytes: size * 2 + size / 2, ..session("agent-a") };
    for _ in 0..2 {
        let err = f.hub.receive(&s, push.clone()).expect_err("refused");
        assert!(err.contains("refusing commit"), "{err}");
    }
    let err = f.hub.receive(&s, push).expect_err("refused");
    assert!(err.contains("quota"), "{err}");
}

#[test]
fn a_bad_pr_head_at_the_hub_blocks_no_other_sync_and_is_never_read() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    // Planted at the hub as if every check had been missed.
    let bad = link_branch(&mut f, 0, "evil");
    let bytes = f.copies[0].make_bundle(&["refs/heads/evil".into()], &[]).expect("bundle");
    let path = f.hub.config.dir.join("planted.bundle");
    std::fs::write(&path, bytes).expect("bundle file");
    let file = path.to_string_lossy().into_owned();
    f.hub
        .git(&["fetch", "--quiet", "--end-of-options", &file, "refs/heads/evil:refs/agents/agent-a/evil"], None)
        .expect("plant");
    let n = f.tool(0, "pr_open", json!({"branch": "evil", "title": "t"})).expect("open")["number"].as_u64().expect("n");
    // Another agent's work still merges, and every copy's main still moves.
    let good = f.propose(1, "abs", "base.stk", &def("abs"));
    f.review(0, good, "approve");
    f.merge(0, good).expect("merged");
    let main = f.hub.rev("refs/heads/main").expect("rev").expect("main");
    let have = f.copies[1].push().expect("push").meta["have"].clone();
    let answer = f.hub.send(&session("agent-b"), &have).expect("send");
    assert_eq!(answer.meta["skipped"], json!([n]), "the bad head is left out, and said");
    f.copies[1].fetch(answer).expect("fetch");
    assert_eq!(f.copies[1].rev("refs/remotes/hub/main").expect("rev"), Some(main));
    assert!(f.copies[1].rev(&format!("refs/remotes/hub/pr-{n}")).expect("rev").is_none());
    // Viewing it reads none of its content: no diff, no blob.
    let before = f.hub.runs.len();
    let view = f.tool(1, "pr_view", json!({"number": n})).expect("view");
    assert_eq!(view["ci"]["pass"], false);
    assert_eq!(view["head"], bad);
    assert!(view.get("diff").is_none());
    assert!(
        f.hub.runs[before..].iter().all(|a| a[0] != "diff" && !(a[0] == "cat-file" && a[1] == "blob")),
        "{:?}",
        &f.hub.runs[before..]
    );
}

/// `n` commits on copy `i`'s branch `branch`, each with the hub's main tree, made by one git process; oldest first.
fn long_history(f: &mut Fixture, i: usize, branch: &str, n: usize) -> Vec<String> {
    let c = &mut f.copies[i];
    let main = c.rev("refs/remotes/hub/main").expect("rev").expect("main");
    let mut stream = format!("reset refs/heads/{branch}\nfrom {main}\n\n");
    for k in 0..n {
        let msg = format!("c{k}");
        stream.push_str(&format!(
            "commit refs/heads/{branch}\ncommitter a <a@x> 1700000000 +0000\ndata {}\n{msg}\n\n",
            msg.len()
        ));
    }
    c.git(&["fast-import", "--quiet"], Some(stream.as_bytes())).expect("fast-import");
    let list = c.git(&["rev-list", "--reverse", &format!("refs/heads/{branch}"), "--not", &main], None).expect("list");
    list.lines().map(str::to_string).collect()
}

#[test]
fn a_long_pr_keeps_every_other_sync_fast_and_a_push_is_capped() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let commits = long_history(&mut f, 0, "long", 2000);
    // Over the cap in one push: refused, before any tree is read.
    let err = {
        let push = f.copies[0].push().expect("push");
        f.hub.receive(&session("agent-a"), push).expect_err("over the cap")
    };
    assert!(err.contains("at most 500"), "{err}");
    // Within it, four pushes; then a PR.
    for k in [499, 999, 1499, 1999] {
        f.copies[0].git(&["update-ref", "refs/heads/long", &commits[k]], None).expect("ref");
        f.sync(0);
    }
    let n = f.tool(0, "pr_open", json!({"branch": "long", "title": "t"})).expect("open")["number"].as_u64().expect("n");
    // Another agent's first sync brings all 2000 commits, checked in a few git processes (counted,
    // not timed, so a loaded machine doesn't fail it).
    let before = f.copies[1].runs.len();
    f.sync(1);
    assert!(f.copies[1].runs.len() - before <= 30, "{} git processes", f.copies[1].runs.len() - before);
    assert!(f.copies[1].rev(&format!("refs/remotes/hub/pr-{n}")).expect("rev").is_some());
    // Each later sync, and a view, re-checks nothing: a fixed handful of git processes.
    for _ in 0..3 {
        let (hub_before, copy_before) = (f.hub.runs.len(), f.copies[1].runs.len());
        f.sync(1);
        assert!(f.hub.runs.len() - hub_before <= 20, "{:?}", &f.hub.runs[hub_before..]);
        assert!(f.copies[1].runs.len() - copy_before <= 20, "{:?}", &f.copies[1].runs[copy_before..]);
        assert!(!rechecked(&f.hub.runs[hub_before..]) && !rechecked(&f.copies[1].runs[copy_before..]));
    }
    let before = f.hub.runs.len();
    f.tool(1, "pr_view", json!({"number": n})).expect("view");
    assert!(f.hub.runs.len() - before <= 20, "{:?}", &f.hub.runs[before..]);
    assert!(!rechecked(&f.hub.runs[before..]), "a checked head isn't checked again");
}

/// Whether any of `runs` read objects to check trees.
fn rechecked(runs: &[Vec<String>]) -> bool {
    runs.iter().any(|a| a.len() == 2 && a[0] == "cat-file" && a[1] == "--batch")
}

#[test]
fn a_bundle_keeps_only_what_its_refs_reach() {
    let mut f = Fixture::new(&["agent-a"]);
    // A bundle whose pack also carries a blob none of its refs reach.
    let c = &mut f.copies[0];
    let extra = blob(c, b"not reachable from any ref\n");
    let commits = long_history(&mut f, 0, "short", 1);
    let c = &mut f.copies[0];
    let mut objects =
        c.git(&["rev-list", "--objects", "refs/heads/short", "--not", "--remotes=hub"], None).expect("objects");
    objects.push_str(&format!("{extra}\n"));
    let repo = c.work();
    let pack = c.git_in(&repo, &["pack-objects", "--stdout", "--quiet"], Some(objects.as_bytes())).expect("pack");
    let mut bytes = format!("# v2 git bundle\n{} refs/heads/short\n\n", commits[0]).into_bytes();
    bytes.extend(pack.raw);
    let meta = json!({"branches": {}});
    f.hub.receive(&session("agent-a"), Transfer { meta, bytes }).expect("a valid push");
    assert_eq!(f.hub.rev("refs/agents/agent-a/short").expect("rev"), Some(commits[0].clone()));
    let repo = f.hub.repo();
    assert!(!f.hub.git_in(&repo, &["cat-file", "-e", &extra], None).expect("ran").ok, "the extra blob was dropped");
}

/// Runs `f` on another thread; panics if it takes longer than `limit` (a deadlock, say).
fn within<T: Send + 'static>(limit: std::time::Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).expect("finished in time, not deadlocked")
}

#[test]
fn a_child_that_writes_as_it_reads_never_deadlocks() {
    // Megabytes each way: far more than any pipe holds.
    let input = vec![b'x'; 4 << 20];
    let out = within(std::time::Duration::from_secs(60), move || {
        let child = std::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("cat");
        crate::mode::communicate(child, Some(&input)).expect("ran")
    });
    assert_eq!(out.stdout.len(), 4 << 20);
    // And git's batch mode, as the tree checks use it: 100,000 ids in, a line out for each.
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut hub = GitStore::new(config(dir.path(), "setter"));
    hub.setup(None).expect("hub");
    let main = hub.rev("refs/heads/main").expect("rev").expect("main");
    let ids: Vec<String> = vec![main; 100_000];
    let (lines, _dir) = within(std::time::Duration::from_secs(120), move || {
        let here = hub.commits_here(&ids).expect("checked");
        let out = hub.git(&["cat-file", "--batch-check"], Some((ids.join("\n") + "\n").as_bytes())).expect("ran");
        assert_eq!(here.len(), 1);
        (out.lines().count(), dir)
    });
    assert_eq!(lines, 100_000);
}

#[test]
fn open_prs_are_capped_and_others_syncs_cost_the_same_at_the_cap() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    f.sync(1);
    let (hub0, copy0) = (f.hub.runs.len(), f.copies[1].runs.len());
    f.sync(1);
    let quiet = (f.hub.runs.len() - hub0, f.copies[1].runs.len() - copy0);
    // A branch with nothing main doesn't have can't be a PR.
    f.tool(0, "switch", json!({"branch": "empty", "create": true})).expect("switch");
    f.sync(0);
    let err = f.tool(0, "pr_open", json!({"branch": "empty", "title": "t"})).expect_err("refused");
    assert!(err.contains("no commits"), "{err}");
    for k in 0..prs::MAX_OPEN_PRS {
        f.propose(0, &format!("b{k}"), "base.stk", &def("abs"));
    }
    f.tool(0, "switch", json!({"branch": "over", "create": true})).expect("switch");
    f.tool(0, "edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": def("sq")})).expect("edit");
    f.tool(0, "commit", json!({"message": "m"})).expect("commit");
    f.sync(0);
    let err = f.tool(0, "pr_open", json!({"branch": "over", "title": "t"})).expect_err("over the cap");
    assert!(err.contains("PRs open"), "{err}");
    // Another agent's first sync checks the new heads once; later syncs cost a fixed number of processes.
    f.sync(1);
    for _ in 0..2 {
        let (hub0, copy0) = (f.hub.runs.len(), f.copies[1].runs.len());
        f.sync(1);
        let at_cap = (f.hub.runs.len() - hub0, f.copies[1].runs.len() - copy0);
        // One more hub process with PRs open (the one `update-ref --stdin` writing every PR ref), whatever their number.
        assert_eq!(at_cap, (quiet.0 + 1, quiet.1), "{:?}", &f.hub.runs[hub0..]);
    }
    // pr_list runs CI once per (main, head): the second call runs none.
    f.tool(1, "pr_list", json!({})).expect("list");
    let before = f.hub.runs.len();
    f.tool(1, "pr_list", json!({})).expect("list");
    assert!(f.hub.runs.len() - before <= 3, "{:?}", &f.hub.runs[before..]);
}

#[test]
fn pr_text_is_capped_and_replies_and_syncs_stay_bounded() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    f.sync(0);
    let (hub0, copy0) = (f.hub.runs.len(), f.copies[0].runs.len());
    f.sync(0);
    let quiet = (f.hub.runs.len() - hub0, f.copies[0].runs.len() - copy0);
    // One agent spends a whole run's calls on maximal comments, each 6 bytes a character in the record.
    let text = "\u{1}".repeat(prs::MAX_REMARK);
    let accepted = (0..200).filter(|_| f.tool(1, "pr_comment", json!({"number": n, "text": text})).is_ok()).count();
    // The text budget stops it first: 128 KiB of record holds 5 such comments.
    assert_eq!(accepted, prs::MAX_TEXT_PER_AGENT / (6 * prs::MAX_REMARK));
    assert!(f.tool(1, "pr_comment", json!({"number": n, "text": "x".repeat(prs::MAX_REMARK + 1)})).is_err());
    let record = std::fs::metadata(f.hub.config.dir.join("prs.json")).expect("record").len();
    assert!(record < prs::MAX_TEXT_PER_AGENT as u64 + 4096, "the record is {record} bytes");
    // Short comments meet the per-PR cap instead.
    let n2 = f.propose(0, "sq", "base.stk", &def("sq"));
    let short = (0..20).filter(|_| f.tool(1, "pr_comment", json!({"number": n2, "text": "ok"})).is_ok()).count();
    assert_eq!(short, prs::MAX_REMARKS);
    // Another agent's sync costs what it did, and reads no PR text.
    let (hub0, copy0) = (f.hub.runs.len(), f.copies[0].runs.len());
    f.sync(0);
    assert_eq!((f.hub.runs.len() - hub0, f.copies[0].runs.len() - copy0), quiet);
    // A sync reads no PR text at all: it works with the PR record unreadable.
    let record = f.hub.config.dir.join("prs.json");
    let saved = std::fs::read(&record).expect("record");
    std::fs::write(&record, b"not JSON").expect("spoil");
    f.sync(0);
    std::fs::write(&record, saved).expect("restore");
    // Replies stay small: pr_view shows the latest remarks, whatever the record holds.
    let mut all = prs::Prs::load(&f.hub.config.dir).expect("load");
    let many = (0..100).map(|_| prs::Comment { who: "agent-b".into(), text: "y".repeat(prs::MAX_REMARK) });
    all.prs[0].comments.extend(many);
    all.save(&f.hub.config.dir).expect("save");
    let view = f.tool(0, "pr_view", json!({"number": n})).expect("view");
    assert_eq!(view["comments_total"], 105);
    let size = view.to_string().len();
    assert!(size < 256 * 1024, "pr_view is {size} bytes");
    // pr_list summarises closed PRs.
    f.tool(0, "pr_close", json!({"number": n})).expect("close");
    let list = f.tool(1, "pr_list", json!({})).expect("list");
    assert_eq!(list[0].as_object().expect("a row").len(), 3, "{list}");
    // And a title is capped where it is checked.
    assert!(f.tool(0, "pr_open", json!({"branch": "abs", "title": "t".repeat(prs::MAX_TITLE + 1)})).is_err());
}

#[test]
fn records_say_what_a_commit_wrote_and_what_a_review_blocks() {
    let mut f = Fixture::new(&["agent-a", "agent-b"]);
    f.tool(1, "switch", json!({"branch": "sign", "create": true})).expect("switch");
    let n = f.propose(0, "abs", "base.stk", &def("abs"));
    let wrote = f.copies[0].record("commit", &json!({}), &json!({})).expect("record");
    assert_eq!(wrote["writes"], json!([["abs", def("abs")]]));
    let review = json!({"number": n, "verdict": "request_changes"});
    assert_eq!(f.hub.record("pr_review", &review, &json!({})).expect("record")["blocks"], json!([["abs", def("abs")]]));
    f.review(1, n, "approve");
    f.merge(1, n).expect("merged");

    // b adds sign where a added abs: merging main stops on a conflict.
    f.tool(1, "edit", json!({"file": "base.stk", "start": 1, "end": 0, "text": def("sign")})).expect("edit");
    f.tool(1, "commit", json!({"message": "sign"})).expect("commit");
    f.sync(1);
    let merged = f.tool(1, "merge", json!({})).expect("merge");
    assert_eq!(f.copies[1].record("merge", &json!({}), &merged).expect("record")["conflict"], true);

    // The resolution keeps both, so its commit wrote nothing: abs is main's, sign was b's already.
    let lines = f.tool(1, "read", json!({"file": "base.stk"})).expect("read")["lines"].as_i64().expect("lines");
    let both = format!("{}\n{}", def("abs"), def("sign"));
    f.tool(1, "edit", json!({"file": "base.stk", "start": 1, "end": lines, "text": both})).expect("resolve");
    f.tool(1, "commit", json!({"message": "merge main"})).expect("commit");
    assert_eq!(f.copies[1].record("commit", &json!({}), &json!({})).expect("record")["writes"], json!([]));
}
