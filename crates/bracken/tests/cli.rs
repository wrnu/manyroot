mod common;

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn bracken(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(args)
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap();
    assert!(out.status.success(), "bracken {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

fn bracken_fails(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(args)
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .output()
        .unwrap();
    assert!(!out.status.success(), "bracken {args:?} unexpectedly succeeded");
    String::from_utf8(out.stderr).unwrap()
}

fn json(dir: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&bracken(dir, args)).unwrap()
}

#[test]
fn two_replicas_preserve_and_resolve_a_disagreement() {
    let tmp = tempfile::tempdir().unwrap();
    let (laptop, server) = (tmp.path().join("laptop"), tmp.path().join("server"));
    std::fs::create_dir_all(&laptop).unwrap();
    std::fs::create_dir_all(&server).unwrap();

    bracken(&laptop, &["init", "--name", "auth"]);
    bracken(&laptop, &["id", "new", "alice"]);
    bracken(&laptop, &["id", "new", "val"]);
    let req = bracken(&laptop, &["claim", "Sessions need bounded token lifetimes", "--tag", "auth", "--as", "alice"]);
    bracken(&laptop, &["evidence", &req[..8], "pass", "--method", "review", "--as", "val"]);

    let bundle = tmp.path().join("seed.mrb");
    bracken(&laptop, &["export", bundle.to_str().unwrap()]);
    bracken(&server, &["join", bundle.to_str().unwrap()]);
    bracken(&server, &["id", "new", "bob"]);

    // Concurrent, conflicting replacements on each replica.
    let short = bracken(&laptop, &["claim", "Tokens expire after 15 minutes", "--as", "alice"]);
    bracken(&laptop, &["relate", &short, "supersedes", &req, "--as", "alice"]);
    let long = bracken(&server, &["claim", "Tokens expire after 60 minutes", "--as", "bob"]);
    bracken(&server, &["relate", &long, "supersedes", &req, "--as", "bob"]);
    std::fs::write(server.join("ttl.rs"), "pub const TTL: u64 = 3600;\n").unwrap();
    let artifact = bracken(&server, &["artifact", "ttl.rs", "--name", "src/ttl.rs", "--as", "bob"]);

    assert_eq!(bracken(&server, &["sync", "../laptop"]).lines().next(), Some("received 2, sent 3"));
    assert_eq!(bracken(&server, &["sync", "../laptop"]), "received 0, sent 0", "a session sends only what's missing");
    let ids = |dir: &Path| -> Vec<Value> {
        json(dir, &["log", "--json"]).as_array().unwrap().iter().map(|c| c["id"].clone()).collect()
    };
    assert_eq!(ids(&laptop).len(), 7);
    assert_eq!(ids(&laptop), ids(&server), "same graph, same causal order");

    // Bob's supersession of Alice's requirement doesn't count: only its author can retire it.
    assert_eq!(json(&laptop, &["conflicts", "--json"]), Value::Array(vec![]));
    // Alice accepts Bob's replacement too, so the requirement has two current successors.
    bracken(&laptop, &["relate", &long, "supersedes", &req, "--as", "alice"]);
    let conflicts = json(&laptop, &["conflicts", "--json"]);
    assert_eq!(conflicts[0]["type"], "divergence");
    assert_eq!(conflicts[0]["target"], req.as_str());

    // Only the original requirement has evidence, so it is still what's accepted.
    let accepted = json(&laptop, &["accepted", "--json"]);
    assert_eq!(accepted["accepted"].as_array().unwrap().len(), 1);
    assert_eq!(accepted["accepted"][0]["id"], req.as_str());

    // Validating both replacements surfaces the divergence in the accepted layer.
    bracken(&laptop, &["evidence", &short, "pass", "--method", "threat model", "--as", "val"]);
    bracken(&laptop, &["evidence", &long, "pass", "--method", "load test", "--as", "val"]);
    let accepted = json(&laptop, &["accepted", "--json"]);
    assert_eq!(accepted["accepted"].as_array().unwrap().len(), 2);
    assert_eq!(accepted["conflicts"][0]["type"], "divergence");
    assert_eq!(json(&laptop, &["show", &req, "--json"])["assessment"]["status"], "superseded");

    // A stricter policy on the same evidence accepts nothing.
    let strict = json(&laptop, &["accepted", "--min-passes", "2", "--json"]);
    assert!(strict["accepted"].as_array().unwrap().is_empty());

    bracken(&server, &["sync", "../laptop"]);
    assert_eq!(bracken(&laptop, &["cat", &artifact[..10]]), "pub const TTL: u64 = 3600;");
    assert!(bracken_fails(&laptop, &["retract", &short, "--as", "val"]).contains("only the author"));
}

#[test]
fn commands_outside_a_workspace_explain_how_to_start() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(bracken_fails(tmp.path(), &["log"]).contains("bracken init"));
}

fn ids(dir: &Path, args: &[&str]) -> Vec<String> {
    json(dir, args).as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn log_current_shows_only_what_is_current() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "bob"]);

    let first = bracken(dir, &["claim", "Plan: ship v1", "--tag", "plan", "--as", "alice"]);
    let note = bracken(dir, &["claim", "Unrelated note", "--tag", "note", "--as", "alice"]);
    let dropped = bracken(dir, &["claim", "Plan: a typo", "--tag", "plan", "--as", "alice"]);
    bracken(dir, &["retract", &dropped, "--as", "alice"]);
    assert_eq!(ids(dir, &["log", "--tag", "plan", "--current", "--json"]), std::slice::from_ref(&first));

    // Bob relates a claim he didn't write as superseding: that supersession doesn't count.
    let alices = bracken(dir, &["claim", "Plan: ship v2", "--tag", "plan", "--as", "alice"]);
    let by_bob = bracken(dir, &["relate", &alices, "supersedes", &first, "--as", "bob"]);
    let current = ids(dir, &["log", "--tag", "plan", "--current", "--json"]);
    assert_eq!(current, [first.clone(), alices.clone()]);
    assert!(ids(dir, &["log", "--current", "--json"]).contains(&by_bob));

    // Alice's own supersession counts: the old plan, and everything about it, drop out.
    let by_alice = bracken(dir, &["relate", &alices, "supersedes", &first, "--as", "alice"]);
    bracken(dir, &["evidence", &first, "pass", "--method", "review", "--as", "bob"]);
    assert_eq!(ids(dir, &["log", "--tag", "plan", "--current", "--json"]), std::slice::from_ref(&alices));
    let all = ids(dir, &["log", "--current", "--json"]);
    assert_eq!(all, [note.clone(), alices.clone()], "relations, evidence and retractions about hidden items go too");
    assert!(!bracken(dir, &["log", "--current"]).contains("(retracted)"));
    assert_eq!(ids(dir, &["log", "--json"]).len(), 8, "without --current, log shows everything");

    // Retracting the supersession brings the old plan back.
    bracken(dir, &["retract", &by_alice, "--as", "alice"]);
    assert_eq!(ids(dir, &["log", "--tag", "plan", "--current", "--json"]), [first, alices]);
}

#[test]
fn claim_supersedes_replaces_only_your_own_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "bob"]);
    let old = bracken(dir, &["claim", "Goal: v1", "--tag", "goal", "--as", "alice"]);

    let out = bracken(dir, &["claim", "Goal: v2", "--tag", "goal", "--supersedes", &old[..8], "--as", "alice"]);
    let printed: Vec<&str> = out.lines().collect();
    assert_eq!(printed.len(), 2, "prints the claim, then the relation: {out}");
    let (new, relation) = (printed[0], printed[1]);
    let r = json(dir, &["show", relation, "--json"]);
    assert_eq!(r["body"]["from"], new);
    assert_eq!(r["body"]["kind"], "supersedes");
    assert_eq!(r["body"]["to"], old.as_str());
    assert_eq!(ids(dir, &["log", "--tag", "goal", "--current", "--json"]), [new]);

    let before = ids(dir, &["log", "--json"]).len();
    let err = bracken_fails(dir, &["claim", "Goal: v3", "--supersedes", new, "--as", "bob"]);
    assert!(err.contains("only supersede your own"), "{err}");
    let err = bracken_fails(dir, &["claim", "Goal: v3", "--supersedes", relation, "--as", "alice"]);
    assert!(err.contains("not a claim"), "{err}");
    assert_eq!(ids(dir, &["log", "--json"]).len(), before, "a refused claim writes nothing");
}

/// Every write command prints one labelled line per record it creates, the primary first: `claim <id>`,
/// `relation <id> (supersedes)` and so on, so two ids are never told apart by their order alone (task
/// `d4e958fd6719`). With --json: `{"id": <primary>, "written": [{"id", "kind", "relation"?}]}`.
#[test]
fn write_commands_print_one_labelled_line_per_record() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "val"]);
    // The printed text itself, without the tests' bare_ids.
    let raw = |args: &[&str]| -> Vec<String> {
        let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .unwrap();
        assert!(out.status.success(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().lines().map(str::to_owned).collect()
    };
    let id_in = |line: &str, kind: &str| -> String {
        let id = line.strip_prefix(&format!("{kind} ")).unwrap_or_else(|| panic!("not a {kind} line: {line}"));
        let id = id.split(' ').next().unwrap();
        assert_eq!(id.len(), 64, "{line}");
        id.to_owned()
    };
    let shown = |id: &str| json(dir, &["show", id, "--json"]);

    let old = raw(&["claim", "v1", "--as", "alice"]);
    assert_eq!(old.len(), 1);
    let old = id_in(&old[0], "claim");
    let lines = raw(&["claim", "v2", "--supersedes", &old, "--as", "alice"]);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let new = id_in(&lines[0], "claim");
    assert!(lines[1].ends_with(" (supersedes)"), "{lines:?}");
    let relation = id_in(&lines[1], "relation");
    assert_eq!(
        (shown(&relation)["body"]["from"].as_str(), shown(&relation)["body"]["to"].as_str()),
        (Some(&*new), Some(&*old))
    );

    std::fs::write(dir.join("f.rs"), "fn f() {}\n").unwrap();
    let lines = raw(&["artifact", "f.rs", "--implements", &new, "--as", "alice"]);
    let artifact = id_in(&lines[0], "artifact");
    assert!(lines.len() == 2 && lines[1].ends_with(" (implements)"), "{lines:?}");
    let lines = raw(&["relate", &artifact, "supports", &new, "--as", "alice"]);
    assert!(lines.len() == 1 && lines[0].ends_with(" (supports)"), "{lines:?}");
    let lines = raw(&["evidence", &artifact, "fail", "--method", "review", "--depends-on", &old, "--as", "val"]);
    assert_eq!(lines.len(), 2, "{lines:?}");
    let evidence = id_in(&lines[0], "evidence");
    assert!(lines[1].ends_with(" (depends_on)"), "{lines:?}");
    let lines = raw(&["evidence", &artifact, "--run", "--as", "val", "--", "true"]);
    id_in(&lines[0], "evidence");
    let lines = raw(&["intend", &new, "--as", "val"]);
    assert_eq!(lines.len(), 2, "{lines:?}");
    id_in(&lines[0], "claim");
    assert!(lines[1].ends_with(" (refines)"), "{lines:?}");
    let lines = raw(&["retract", &evidence, "--as", "val"]);
    assert_eq!(lines.len(), 1);
    id_in(&lines[0], "retract");

    // --json: the primary's id, then everything written, in order, with each relation's kind.
    let v = json(dir, &["claim", "v3", "--supersedes", &new, "--as", "alice", "--json"]);
    let written = v["written"].as_array().unwrap();
    assert_eq!(written.len(), 2, "{v}");
    assert_eq!(v["id"], written[0]["id"]);
    assert_eq!((written[0]["kind"].as_str(), written[0].get("relation")), (Some("claim"), None));
    assert_eq!((written[1]["kind"].as_str(), written[1]["relation"].as_str()), (Some("relation"), Some("supersedes")));
    assert_eq!(shown(written[1]["id"].as_str().unwrap())["body"]["to"], new.as_str());
    let v = json(dir, &["retract", v["id"].as_str().unwrap(), "--as", "alice", "--json"]);
    assert_eq!(v["written"][0]["kind"], "retract");
}

/// A write that fails part way, its claim recorded and its supersedes relation not, fails even when
/// its reader has stopped (finding `58def77d1d97`): the status is decided before the claim's line is
/// printed. The relation is made to fail by a file-size limit between the claim's object (under 300
/// bytes) and the relation's (about 390), with SIGXFSZ ignored so the write returns an error.
#[test]
fn a_write_that_fails_part_way_fails_with_its_reader_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    let old = bracken(dir, &["claim", "v1", "--as", "alice"]);
    let limited = |closed: bool| {
        let limit = "import os, resource, signal, sys; signal.signal(signal.SIGXFSZ, signal.SIG_IGN); \
                     resource.setrlimit(resource.RLIMIT_FSIZE, (330, 330)); os.execv(sys.argv[1], sys.argv[1:])";
        let mut child = Command::new("python3")
            .args(["-c", limit, env!("CARGO_BIN_EXE_bracken"), "claim", "v2", "--supersedes", &old, "--as", "alice"])
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        if closed {
            drop(child.stdout.take());
        }
        child.wait_with_output().unwrap()
    };
    let claims = || ids(dir, &["log", "--kind", "claim", "--json"]).len();
    let relations = || ids(dir, &["log", "--kind", "relation", "--json"]).len();

    let out = limited(false);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("was recorded, but not all its supersedes relations"), "{stderr}");
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(printed.starts_with("claim ") && printed.lines().count() == 1, "the claim written is named: {printed}");
    assert_eq!((claims(), relations()), (2, 0));

    let out = limited(true);
    assert_eq!(out.status.code(), Some(1), "a closed reader made it succeed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!((claims(), relations()), (3, 0));
}

#[test]
fn read_commands_accept_and_ignore_as() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    let c = bracken(dir, &["claim", "Hello", "--as", "alice"]);

    assert_eq!(bracken(dir, &["log", "--as", "alice"]), bracken(dir, &["log"]));
    assert_eq!(bracken(dir, &["log", "--as", "nobody", "--json"]), bracken(dir, &["log", "--json"]));
    assert_eq!(bracken(dir, &["show", &c, "--as", "alice"]), bracken(dir, &["show", &c]));
    assert_eq!(bracken(dir, &["--as", "alice", "heads"]), bracken(dir, &["heads"]));

    // Writes still need a signer.
    assert!(bracken_fails(dir, &["claim", "Unsigned"]).contains("--as"));
}

#[test]
fn log_current_keeps_what_another_author_superseded() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "builder"]);
    bracken(dir, &["id", "new", "security"]);

    let pr = bracken(dir, &["claim", "PR #1 head", "--tag", "pr", "--as", "builder"]);
    let finding = bracken(dir, &["claim", "main.rs:1 leaks a key", "--tag", "finding", "--as", "security"]);
    let contradicts = bracken(dir, &["relate", &finding, "contradicts", &pr, "--as", "security"]);
    let fail = bracken(dir, &["evidence", &pr, "fail", "--method", "review", "--as", "security"]);

    // The builder supersedes the finding with a claim of their own. It's another author's supersession, so it
    // doesn't count, and the finding stays, marked.
    let addressed = bracken(dir, &["claim", "Addressed", "--as", "builder"]);
    let by_builder = bracken(dir, &["relate", &addressed, "supersedes", &finding, "--as", "builder"]);

    assert_eq!(ids(dir, &["log", "--tag", "finding", "--current", "--json"]), std::slice::from_ref(&finding));
    let current = ids(dir, &["log", "--current", "--json"]);
    for kept in [&pr, &finding, &contradicts, &fail, &addressed, &by_builder] {
        assert!(current.contains(kept), "{kept} should stay current");
    }
    let row = json(dir, &["log", "--tag", "finding", "--current", "--json"]);
    assert_eq!(row[0]["superseded_by_others"][0]["id"], addressed.as_str());
    assert_eq!(row[0]["superseded_by_others"][0]["author_name"], "builder");
    let text = bracken(dir, &["log", "--tag", "finding", "--current"]);
    assert!(text.contains(&format!("(superseded by builder {})", &addressed[..12])), "{text}");
    // However long the line, and however its escapes lengthen it, the compact form (the default with
    // --tag) keeps the whole mark within its 200 characters (finding c80965f348d5).
    for statement in ["x".repeat(130), "a\t".repeat(75)] {
        let long = bracken(dir, &["claim", &statement, "--tag", "long", "--as", "security"]);
        let by = bracken(dir, &["claim", "instead", "--as", "builder"]);
        bracken(dir, &["relate", &by, "supersedes", &long, "--as", "builder"]);
        let line = bracken(dir, &["log", "--tag", "long", "--current", "--limit", "1"]);
        let line = line.lines().next().unwrap();
        assert!(line.chars().count() <= 200, "{} characters: {line}", line.chars().count());
        assert!(line.starts_with(&long[..8]), "{line}");
        assert!(line.ends_with(&format!("  (superseded by builder {})", &by[..12])), "{line}");
    }
    // A name as long as names go (64) is cut inside its own mark, so its id and the next mark stay
    // (finding 69725885c7fb); marks past what fits are counted.
    let long_name = "z".repeat(64);
    bracken(dir, &["id", "new", &long_name]);
    let decision = bracken(dir, &["claim", "a decision", "--tag", "marked", "--as", "security"]);
    let mut by = Vec::new();
    for who in [long_name.as_str(), "builder"] {
        let b = bracken(dir, &["claim", "instead", "--as", who]);
        bracken(dir, &["relate", &b, "supersedes", &decision, "--as", who]);
        by.push(b);
    }
    let line = bracken(dir, &["log", "--tag", "marked", "--current"]);
    assert!(line.chars().count() <= 200, "{line}");
    let short = format!("{}…", "z".repeat(15));
    assert!(line.contains(&format!("  (superseded by {short} {})", &by[0][..12])), "{line}");
    assert!(line.ends_with(&format!("  (superseded by builder {})", &by[1][..12])), "{line}");
    let b = bracken(dir, &["claim", "instead", "--as", "builder"]);
    bracken(dir, &["relate", &b, "supersedes", &decision, "--as", "builder"]);
    let line = bracken(dir, &["log", "--tag", "marked", "--current"]);
    assert!(line.contains(&format!("  (superseded by {short} {})", &by[0][..12])), "{line}");
    assert!(line.contains(&format!("  (superseded by builder {})", &by[1][..12])), "{line}");
    assert!(line.ends_with("  (+1 more)") && line.chars().count() <= 200, "{line}");
    assert_eq!(bracken(dir, &["log", "--tag", "marked", "--current", "--full"]).matches("(superseded by").count(), 3);
    // Nor does it silence the finding in `conflicts` (task 780f658d4b84).
    let conflicts = json(dir, &["conflicts", "--json"]);
    assert_eq!(conflicts.as_array().unwrap().len(), 1, "{conflicts}");
    assert_eq!(conflicts[0]["type"], "contradiction");
    assert_eq!(conflicts[0]["relation"], contradicts.as_str());

    // The finding's own author superseding it still hides it, and everything about it.
    let revised = bracken(
        dir,
        &["claim", "main.rs:1 leaks a key (revised)", "--tag", "finding", "--supersedes", &finding, "--as", "security"],
    );
    let revised = revised.lines().next().unwrap().to_owned();
    assert_eq!(ids(dir, &["log", "--tag", "finding", "--current", "--json"]), [revised]);
    let current = ids(dir, &["log", "--current", "--json"]);
    assert!(!current.contains(&contradicts) && !current.contains(&by_builder));
    assert!(current.contains(&fail), "evidence about the pr is not about the finding");
}

#[test]
fn log_current_hides_what_its_author_replaced_with_someone_elses() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "builder"]);
    bracken(dir, &["id", "new", "security"]);

    let pr = bracken(dir, &["claim", "PR #1 head", "--tag", "pr", "--as", "builder"]);
    let finding = bracken(dir, &["claim", "main.rs:1 leaks a key", "--tag", "finding", "--as", "security"]);
    let contradicts = bracken(dir, &["relate", &finding, "contradicts", &pr, "--as", "security"]);
    let addressed = bracken(dir, &["claim", "Addressed: the key is read from a file", "--as", "builder"]);
    let by_builder = bracken(dir, &["relate", &addressed, "supersedes", &finding, "--as", "builder"]);
    assert_eq!(ids(dir, &["log", "--tag", "finding", "--current", "--json"]), std::slice::from_ref(&finding));

    // The finding's author accepts the builder's claim in its place: that supersession counts.
    let accepts = bracken(dir, &["relate", &addressed, "supersedes", &finding, "--as", "security"]);
    assert!(ids(dir, &["log", "--tag", "finding", "--current", "--json"]).is_empty());
    let current = ids(dir, &["log", "--current", "--json"]);
    for gone in [&finding, &contradicts, &by_builder, &accepts] {
        assert!(!current.contains(gone), "{gone} should leave log --current with the finding");
    }
    assert!(current.contains(&addressed) && current.contains(&pr));
    assert_eq!(json(dir, &["show", &finding, "--json"])["assessment"]["status"], "pending");
    assert!(json(dir, &["conflicts", "--json"]).as_array().unwrap().is_empty());
}

#[test]
fn a_reader_that_stops_early_is_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    let old = bracken(dir, &["claim", "Plan: v1", "--as", "alice"]);

    // Like `| head -1`, but the pipe is closed before bracken writes anything.
    let closed_early = |args: &[&str]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "bracken {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    closed_early(&["log"]);
    closed_early(&["log", "--json"]);
    closed_early(&["claim", "Plan: v2", "--supersedes", &old, "--as", "alice"]);

    // The write finished: the new claim and its relation are both there.
    let plans: Vec<Value> = json(dir, &["log", "--current", "--json"])
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["body"]["statement"].as_str().is_some_and(|s| s.starts_with("Plan")))
        .cloned()
        .collect();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0]["body"]["statement"], "Plan: v2");
}

/// Runs bracken with stdout already closed, as under `| head -0`. Returns its status and stderr.
fn with_closed_stdout(dir: &Path, args: &[&str]) -> (std::process::ExitStatus, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(args)
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    (out.status, String::from_utf8(out.stderr).unwrap())
}

#[test]
fn join_and_import_keep_a_failure_when_the_reader_stops_early() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, joined, replica) = (tmp.path().join("origin"), tmp.path().join("joined"), tmp.path().join("replica"));
    for d in [&origin, &joined, &replica] {
        std::fs::create_dir_all(d).unwrap();
    }
    bracken(&origin, &["init"]);
    bracken(&origin, &["id", "new", "alice"]);
    bracken(&origin, &["claim", "First", "--as", "alice"]);
    let good = tmp.path().join("good.mrb");
    bracken(&origin, &["export", good.to_str().unwrap()]);
    bracken(&origin, &["claim", "Second", "--as", "alice"]);
    // A bundle whose last contribution has a broken signature: it decodes, then is rejected.
    let bad = tmp.path().join("bad.mrb");
    bracken(&origin, &["export", bad.to_str().unwrap()]);
    let mut bytes = std::fs::read(&bad).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&bad, bytes).unwrap();

    let (status, stderr) = with_closed_stdout(&joined, &["join", bad.to_str().unwrap()]);
    assert_eq!(status.code(), Some(1), "join: {stderr}");
    assert!(stderr.contains("rejected"), "join: {stderr}");

    bracken(&replica, &["join", good.to_str().unwrap()]);
    let (status, stderr) = with_closed_stdout(&replica, &["import", bad.to_str().unwrap()]);
    assert_eq!(status.code(), Some(1), "import: {stderr}");
    assert!(stderr.contains("rejected"), "import: {stderr}");

    // A clean import with a closed stdout still succeeds.
    let (status, stderr) = with_closed_stdout(&replica, &["import", good.to_str().unwrap()]);
    assert!(status.success(), "clean import: {stderr}");
}

#[test]
fn evidence_details_come_from_a_file_or_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "val"]);
    let c = bracken(dir, &["claim", "Works", "--as", "alice"]);
    let details = |id: &str| json(dir, &["show", id, "--json"])["body"]["details"].as_str().unwrap().to_owned();

    std::fs::write(dir.join("out.txt"), "exit 0\nall 7 tests passed\n").unwrap();
    let from_file =
        bracken(dir, &["evidence", &c, "pass", "--method", "t", "--details-file", "out.txt", "--as", "val"]);
    assert_eq!(details(&from_file), "exit 0\nall 7 tests passed\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["evidence", &c, "fail", "--method", "t", "--details-file", "-", "--as", "val"])
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), b"exit 2\nfrom stdin\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let from_stdin = common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned());
    assert_eq!(details(&from_stdin), "exit 2\nfrom stdin\n");

    let before = ids(dir, &["log", "--json"]).len();
    let err = bracken_fails(
        dir,
        &["evidence", &c, "pass", "--method", "t", "--details", "x", "--details-file", "out.txt", "--as", "val"],
    );
    assert!(err.contains("cannot be used with"), "{err}");
    assert!(
        bracken_fails(dir, &["evidence", &c, "pass", "--method", "t", "--details-file", "missing.txt", "--as", "val"])
            .contains("missing.txt")
    );
    assert_eq!(ids(dir, &["log", "--json"]).len(), before, "a refused evidence writes nothing");
}

/// `evidence --run`: the command's exit is the verdict, its command line the method, its exit and output
/// tail the details; the command's exit code passes through. A command that can't start records
/// nothing, and a verdict, method or details given beside --run are refused.
#[test]
fn evidence_run_records_the_commands_result() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "val"]);
    let c = bracken(dir, &["claim", "Works", "--as", "alice"]);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(args)
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .unwrap()
    };
    let body = |out: &std::process::Output| {
        let id = common::bare_ids(String::from_utf8(out.stdout.clone()).unwrap().trim().to_owned());
        json(dir, &["show", &id, "--json"])["body"].clone()
    };

    let pass = run(&["evidence", &c, "--run", "--as", "val", "--", "sh", "-c", "echo 'all 7 passed'"]);
    assert_eq!(pass.status.code(), Some(0));
    let b = body(&pass);
    assert_eq!(
        (b["verdict"].as_str(), b["method"].as_str()),
        (Some("pass"), Some("sh -c 'echo '\\''all 7 passed'\\'''"))
    );
    assert_eq!(b["details"], "exit 0\nall 7 passed");
    assert!(String::from_utf8_lossy(&pass.stderr).contains("all 7 passed"), "the output shows as it runs");

    // A failure, with more output than the tail keeps and control characters in it. Both streams go to one
    // pipe here: from two, their order in the tail is a race (note `4a2c53c065a9`).
    let script = "exec 2>&1; for i in $(seq 1 100); do echo line $i; done; printf '\\033[2Kbad\\r\\n' >&2; exit 3";
    let fail = run(&["evidence", &c, "--run", "--as", "val", "--", "sh", "-c", script]);
    assert_eq!(fail.status.code(), Some(3), "the command's exit code passes through");
    let b = body(&fail);
    let details = b["details"].as_str().unwrap();
    assert_eq!(b["verdict"], "fail");
    assert!(details.starts_with("exit 3\n") && details.ends_with("line 100\n[2Kbad"), "{details}");
    assert_eq!(details.lines().count(), 41, "the exit and the last 40 lines: {details}");
    // What the command writes to stderr is kept too.
    let err_only = run(&["evidence", &c, "--run", "--as", "val", "--", "sh", "-c", "echo oops >&2; exit 1"]);
    assert_eq!(body(&err_only)["details"], "exit 1\noops");

    // Both streams go into the details in the order the command wrote them (note 4a2c53c065a9).
    let script = "for i in $(seq 1 20); do echo out $i; echo err $i >&2; done";
    let mixed = run(&["evidence", &c, "--run", "--as", "val", "--", "sh", "-c", script]);
    let want: String = (1..=20).map(|i| format!("\nout {i}\nerr {i}")).collect();
    assert_eq!(body(&mixed)["details"].as_str(), Some(format!("exit 0{want}").as_str()));

    // Without --run, the target and verdict may still come after `--`, out of reach of option parsing.
    let after = bracken(dir, &["evidence", "--method=-m", "--as", "val", "--", &c, "pass"]);
    assert_eq!(json(dir, &["show", &after, "--json"])["body"]["method"], "-m");

    let before = ids(dir, &["log", "--json"]).len();
    let err = bracken_fails(dir, &["evidence", &c, "--run", "--as", "val", "--", "./no-such-program"]);
    assert!(err.contains("couldn't start") && err.contains("nothing recorded"), "{err}");
    for refused in [
        vec!["evidence", &c, "pass", "--run", "--as", "val", "--", "true"],
        vec!["evidence", &c, "--method", "m", "--run", "--as", "val", "--", "true"],
        vec!["evidence", &c, "--details", "d", "--run", "--as", "val", "--", "true"],
        vec!["evidence", &c, "--run", "--as", "val"],
        vec!["evidence", &c, "pass", "--method", "m", "--as", "val", "--", "true"],
        vec!["evidence", "--method", "m", "--as", "val", "--", &c, "pass", "true"],
        vec!["evidence", "--method", "m", "--as", "val", "--", &c, "maybe"],
        vec!["evidence", &c, "pass", "--as", "val"],
    ] {
        bracken_fails(dir, &refused);
    }
    // Without a way to sign, nothing runs.
    let marker = dir.join("ran");
    bracken_fails(dir, &["evidence", &c, "--run", "--as", "nobody", "--", "touch", marker.to_str().unwrap()]);
    assert!(!marker.exists(), "a refusal comes before the command runs");
    assert_eq!(ids(dir, &["log", "--json"]).len(), before, "a refused evidence writes nothing");
}

#[test]
fn evidence_details_file_refuses_keys_devices_and_oversized_input() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    bracken(dir, &["id", "new", "val"]);
    let c = bracken(dir, &["claim", "Works", "--as", "val"]);
    let attach = |path: &str| {
        bracken_fails(dir, &["evidence", &c, "pass", "--method", "t", "--details-file", path, "--as", "val"])
    };
    let before = ids(dir, &["log", "--json"]).len();

    let key = dir.join(".bracken/identities/val.json");
    assert!(key.is_file(), "the identity lives where the test expects");
    assert!(attach(".bracken/identities/val.json").contains("private key"));
    let link = dir.join("innocent.log");
    std::os::unix::fs::symlink(&key, &link).unwrap();
    assert!(attach("innocent.log").contains("private key"), "a symlink to a key is a key");

    assert!(attach("/dev/zero").contains("isn't a regular file"));

    // A FIFO with no writer is refused at once; opening it would block forever.
    assert!(Command::new("mkfifo").arg(dir.join("fifo")).status().unwrap().success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["evidence", &c, "pass", "--method", "t", "--details-file", "fifo", "--as", "val"])
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > std::time::Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("--details-file on a FIFO blocked");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("isn't a regular file"));

    std::fs::write(dir.join("big.txt"), vec![b'y'; 16 * 1024 * 1024 + 1]).unwrap();
    assert!(attach("big.txt").contains("limit"));

    assert_eq!(ids(dir, &["log", "--json"]).len(), before, "nothing was recorded");
}

#[test]
fn a_local_private_key_never_reaches_the_ground() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init"]);
    let public = bracken(dir, &["id", "new", "val"]);
    let c = bracken(dir, &["claim", "Works", "--as", "val"]);
    let key_file = dir.join(".bracken/identities/val.json");
    let identity: Value = serde_json::from_str(&std::fs::read_to_string(&key_file).unwrap()).unwrap();
    let secret = identity["secret_key"].as_str().unwrap().to_owned();
    let before = ids(dir, &["log", "--json"]).len();
    let evidence = |extra: &[&str]| {
        let mut args = vec!["evidence", &c, "pass", "--method", "t", "--as", "val"];
        args.extend_from_slice(extra);
        args.into_iter().map(String::from).collect::<Vec<_>>()
    };
    let refused = |args: Vec<String>| {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let err = bracken_fails(dir, &args);
        assert!(err.contains("private key"), "{args:?}: {err}");
    };

    refused(evidence(&["--details", &format!("log line: {secret}")]));
    refused(evidence(&["--details", &secret.to_uppercase()]));
    std::fs::hard_link(&key_file, dir.join("build.log")).unwrap();
    refused(evidence(&["--details-file", "build.log"]));
    refused(vec!["claim".into(), format!("the key is {secret}"), "--as".into(), "val".into()]);
    refused(vec!["artifact".into(), ".bracken/identities/val.json".into(), "--as".into(), "val".into()]);
    // The key as raw bytes, not hex.
    let raw: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&secret[2 * i..2 * i + 2], 16).unwrap()).collect();
    std::fs::write(dir.join("key.bin"), raw).unwrap();
    refused(vec!["artifact".into(), "key.bin".into(), "--as".into(), "val".into()]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(evidence(&["--details-file", "-"]))
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &std::fs::read(&key_file).unwrap()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("private key"));
    assert_eq!(ids(dir, &["log", "--json"]).len(), before, "nothing was recorded");

    // A public key, or talk about secret_key, is fine.
    bracken(
        dir,
        &["evidence", &c, "pass", "--method", "t", "--details", &format!("signed by {public}"), "--as", "val"],
    );
    bracken(dir, &["claim", "Never commit a file with a secret_key field", "--as", "val"]);
}

/// `status` lists each current claim that something implements, with each implementer's assessment:
/// waiting ones first, within a claim and across claims; implementers of any kind; only current
/// relations and claims. `--all` adds the claims nothing implements; `--json` has the same content.
#[test]
fn status_shows_what_implements_each_claim_and_what_is_waiting() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "status"]);
    for who in ["alice", "bob", "val"] {
        bracken(dir, &["id", "new", who]);
    }
    let artifact = |name: &str, who: &str| {
        std::fs::write(dir.join("f"), name).unwrap();
        bracken(dir, &["artifact", "f", "--name", name, "--as", who])
    };
    let implements = |from: &str, to: &str, who: &str| bracken(dir, &["relate", from, "implements", to, "--as", who]);
    let pass = |id: &str| bracken(dir, &["evidence", id, "pass", "--method", "review", "--as", "val"]);

    // done: implemented and accepted, nothing waiting; written first.
    let done = bracken(dir, &["claim", "spec: done", "--as", "alice"]);
    let finished = artifact("finished.rs", "bob");
    implements(&finished, &done, "bob");
    pass(&finished);
    // A claim can implement a claim: nothing assumes code.
    let note = bracken(dir, &["claim", "a written answer", "--as", "bob"]);
    implements(&note, &done, "alice");
    pass(&note);
    // busy: one accepted, one waiting (its author's own pass doesn't count), one whose relation was retracted.
    let busy = bracken(dir, &["claim", "spec: busy", "--as", "alice"]);
    let accepted = artifact("accepted.rs", "bob");
    implements(&accepted, &busy, "bob");
    pass(&accepted);
    let waiting = artifact("waiting.rs", "alice");
    implements(&waiting, &busy, "alice");
    bracken(dir, &["evidence", &waiting, "pass", "--method", "self", "--as", "alice"]);
    let dropped = artifact("dropped.rs", "bob");
    let link = implements(&dropped, &busy, "bob");
    bracken(dir, &["retract", &link, "--as", "bob"]);
    // A superseded claim isn't current, so what implements it isn't listed.
    let old = bracken(dir, &["claim", "spec: old", "--as", "alice"]);
    implements(&artifact("old.rs", "bob"), &old, "bob");
    bracken(dir, &["claim", "spec: new", "--supersedes", &old, "--as", "alice"]);
    let idle = bracken(dir, &["claim", "spec: nobody yet", "--as", "alice"]);
    // many: three waiting and three accepted, made interleaved; listed waiting then accepted, each by
    // age. Rows start in id order, which is random, so this pins the sort (1 chance in 720 otherwise).
    let many = bracken(dir, &["claim", "spec: many", "--as", "alice"]);
    let (mut waits, mut oks) = (Vec::new(), Vec::new());
    for n in 0..3 {
        let w = artifact(&format!("w{n}.rs"), "alice");
        implements(&w, &many, "alice");
        waits.push(w);
        let a = artifact(&format!("a{n}.rs"), "bob");
        implements(&a, &many, "bob");
        pass(&a);
        oks.push(a);
    }

    let text = bracken(dir, &["status", "--full"]);
    let lines: Vec<&str> = text.lines().collect();
    let at = |id: &str| lines.iter().position(|l| l.contains(&id[..12])).unwrap_or_else(|| panic!("{id}: {text}"));
    assert!(at(&busy) < at(&done), "the claim with something waiting comes first:\n{text}");
    assert!(at(&waiting) < at(&accepted), "waiting before accepted:\n{text}");
    assert!(lines[at(&waiting)].trim_start().starts_with("waiting") && lines[at(&waiting)].contains("0/1 passes"));
    assert!(lines[at(&accepted)].trim_start().starts_with("accepted") && lines[at(&accepted)].contains("1/1 passes"));
    assert!(lines[at(&note)].contains("claim") && lines[at(&note)].contains("a written answer"), "{text}");
    for hidden in [&dropped, &old, &idle] {
        assert!(!text.contains(&hidden[..12]), "{hidden} listed:\n{text}");
    }
    let all = bracken(dir, &["status", "--all", "--full"]);
    let tail = all.split("nothing implements, or intends to:").nth(1).unwrap_or_else(|| panic!("{all}"));
    assert!(tail.contains(&idle[..12]) && tail.contains("spec: new") && !tail.contains(&busy[..12]), "{all}");
    // A stricter policy: nothing has two passes, so all waits. A second pass by the same validator is
    // still one independent pass.
    pass(&accepted);
    let strict = bracken(dir, &["status", "--min-passes", "2", "--full"]);
    assert!(!strict.contains("accepted ") && strict.contains("1/2 passes") && !strict.contains("2/2"), "{strict}");
    let accepted_row = strict.lines().find(|l| l.contains(&accepted[..12])).unwrap();
    assert!(accepted_row.trim_start().starts_with("waiting") && accepted_row.contains("1/2 passes"), "{strict}");
    let v = json(dir, &["status", "--min-passes", "2", "--json"]);
    let row = v["claims"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["implemented_by"].as_array().unwrap())
        .find(|i| i["id"] == accepted.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["independent_passes"], 1);
    assert_eq!(row["assessment"]["passes"].as_array().unwrap().len(), 2);

    let v = json(dir, &["status", "--json"]);
    assert_eq!(v["min_passes"], 1);
    assert_eq!(v["unimplemented"], serde_json::json!([]));
    let claims = v["claims"].as_array().unwrap();
    let ids: Vec<&str> = claims.iter().map(|c| c["claim"]["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [busy.as_str(), many.as_str(), done.as_str()]);
    let rows: Vec<&str> =
        claims[1]["implemented_by"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    let expected: Vec<&str> = waits.iter().chain(&oks).map(String::as_str).collect();
    assert_eq!(rows, expected, "waiting first, then accepted, each oldest first");
    let by = claims[0]["implemented_by"].as_array().unwrap();
    let by_ids: Vec<&str> = by.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(by_ids, [waiting.as_str(), accepted.as_str()]);
    assert_eq!(by[0]["waiting"], true);
    assert_eq!(by[0]["assessment"]["status"], "pending");
    assert_eq!(by[0]["author_name"], "alice");
    assert_eq!(by[0]["kind"], "artifact");
    assert_eq!(by[1]["assessment"]["status"], "accepted");
    assert_eq!(by[1]["independent_passes"], 1);
    assert!(by[1]["age_ms"].as_u64().is_some() && by[1]["timestamp_ms"].as_u64().is_some());
    let v = json(dir, &["status", "--all", "--json"]);
    let unimplemented: Vec<&str> =
        v["unimplemented"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(unimplemented.contains(&idle.as_str()) && !unimplemented.contains(&busy.as_str()), "{unimplemented:?}");
}

/// `artifact --implements` records the artifact and its implements relation, both signed by the same
/// identity, and prints both ids. An id that doesn't resolve fails before anything is written.
#[test]
fn artifact_implements_records_the_artifact_and_its_relation() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "deliver"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "bob"]);
    let spec = bracken(dir, &["claim", "spec: parse dates", "--as", "alice"]);
    std::fs::write(dir.join("dates.rs"), "fn parse() {}\n").unwrap();

    let out = bracken(dir, &["artifact", "dates.rs", "--implements", &spec[..12], "--as", "bob"]);
    let printed: Vec<&str> = out.lines().collect();
    assert_eq!(printed.len(), 2, "{out}");
    let artifact = json(dir, &["show", printed[0], "--json"]);
    assert_eq!(artifact["kind"], "artifact");
    assert_eq!(artifact["body"]["name"], "dates.rs");
    let relation = json(dir, &["show", printed[1], "--json"]);
    assert_eq!(relation["kind"], "relation");
    assert_eq!(relation["body"]["kind"], "implements");
    assert_eq!(relation["body"]["from"], printed[0]);
    assert_eq!(relation["body"]["to"], spec.as_str());
    assert_eq!(relation["author_name"], "bob");
    assert_eq!(artifact["author_name"], "bob");
    let status = json(dir, &["status", "--json"]);
    assert_eq!(status["claims"][0]["implemented_by"][0]["id"], printed[0]);

    let before = bracken(dir, &["log", "--json"]);
    let err = bracken_fails(dir, &["artifact", "dates.rs", "--implements", "ffffffffffff", "--as", "bob"]);
    assert!(err.contains("no matching contribution"), "{err}");
    let err = bracken_fails(dir, &["artifact", "missing.rs", "--implements", &spec, "--as", "bob"]);
    assert!(err.contains("missing.rs"), "{err}");
    assert_eq!(bracken(dir, &["log", "--json"]), before, "a failed artifact --implements wrote something");
}

/// `status` prints other agents' statements, tags and names with control characters and line or
/// paragraph separators escaped, so they can't forge lines on a terminal (finding 252f052a283d).
/// `--json` keeps the text as it is.
#[test]
fn status_escapes_control_characters_in_what_it_prints() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "escape"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["id", "new", "mallory"]);
    let forged = "spec\r\u{1b}[2Kaccepted  fake\u{b}\u{c}\u{85}\u{2028}\u{2029}\nsecond line";
    let spec = bracken(dir, &["claim", forged, "--tag", "t\u{1b}[31m", "--as", "alice"]);
    let idle = bracken(dir, &["claim", "idle\u{2028}spec", "--as", "mallory"]);
    std::fs::write(dir.join("f"), "x").unwrap();
    bracken(dir, &["artifact", "f", "--name", "impl\u{1b}]0;title\u{7}.rs", "--implements", &spec, "--as", "mallory"]);

    let text = bracken(dir, &["status", "--all", "--full"]) + "\n" + &bracken(dir, &["status", "--all"]);
    for (n, line) in text.lines().enumerate() {
        let bad: Vec<char> = line.chars().filter(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')).collect();
        assert!(bad.is_empty(), "line {n} holds {bad:?}:\n{text}");
    }
    assert!(
        text.contains("spec\\u{d}\\u{1b}[2Kaccepted  fake\\u{b}\\u{c}\\u{85}\\u{2028}\\u{2029}\\u{a}second line"),
        "{text}"
    );
    assert!(text.contains("t\\u{1b}[31m") && text.contains("impl\\u{1b}]0;title\\u{7}.rs"), "{text}");
    assert!(text.contains(&idle[..12]) && text.contains("idle\\u{2028}spec"), "{text}");
    // --full: one line for the claim, one for its implementer, then the "nothing implements" section;
    // compact: one line for the implementer, then that section.
    assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 4 + 3, "{text}");

    let v = json(dir, &["status", "--json"]);
    assert_eq!(v["claims"][0]["claim"]["body"]["statement"], forged);
}

/// `log --since` prints only what the cursor hasn't seen, then the cursor for next time; a cursor with two
/// heads (after concurrent work on two replicas) covers both, the filters and --json work with it, and an
/// unknown id in a cursor is refused.
#[test]
fn log_since_a_cursor_prints_only_what_is_new_and_the_next_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let (laptop, server) = (tmp.path().join("laptop"), tmp.path().join("server"));
    std::fs::create_dir_all(&laptop).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    bracken(&laptop, &["init", "--name", "since"]);
    bracken(&laptop, &["id", "new", "alice"]);
    let first = bracken(&laptop, &["claim", "first", "--as", "alice"]);
    // The text form ends with the cursor; everything before it is the log.
    let since = |dir: &Path, cursor: &str, also: &[&str]| {
        let out = bracken(dir, &[&["log", "--full", "--since", cursor][..], also].concat());
        let (rows, last) = out.rsplit_once('\n').map_or(("", out.as_str()), |(r, l)| (r, l));
        let cursor = last.strip_prefix("cursor: ").unwrap_or_else(|| panic!("no cursor after: {out}")).to_owned();
        let ids: Vec<String> = rows.lines().map(|l| l.split_whitespace().next().unwrap().to_owned()).collect();
        (ids, cursor)
    };
    let short = |id: &str| id[..12].to_owned();

    let (ids, start) = since(&laptop, "", &[]);
    assert_eq!((ids, start.clone()), (vec![short(&first)], first.clone()));

    let bundle = tmp.path().join("seed.mrb");
    bracken(&laptop, &["export", bundle.to_str().unwrap()]);
    bracken(&server, &["join", bundle.to_str().unwrap()]);
    bracken(&server, &["id", "new", "bob"]);
    let ours = bracken(&laptop, &["claim", "ours", "--tag", "x", "--as", "alice"]);
    let theirs = bracken(&server, &["claim", "theirs", "--as", "bob"]);
    bracken(&laptop, &["sync", "../server"]);
    let (mut ids, two) = since(&laptop, &start, &[]);
    ids.sort();
    let mut expected = vec![short(&ours), short(&theirs)];
    expected.sort();
    assert_eq!(ids, expected);
    let mut heads = [ours.clone(), theirs.clone()];
    heads.sort();
    assert_eq!(two, heads.join(","), "the cursor is both heads");
    assert_eq!(since(&laptop, &start, &["--tag", "x"]).0, vec![short(&ours)], "filters still apply");
    assert_eq!(since(&laptop, &two, &[]), (vec![], two.clone()), "nothing new since the latest cursor");

    let next = bracken(&laptop, &["claim", "after both", "--as", "alice"]);
    assert_eq!(since(&laptop, &two, &[]), (vec![short(&next)], next.clone()));
    // Either head alone hasn't seen the other.
    assert_eq!(since(&laptop, &ours[..12], &[]).0, vec![short(&theirs), short(&next)]);

    let v = json(&laptop, &["log", "--since", &two, "--json"]);
    let ids: Vec<&str> = v["contributions"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!((ids, v["cursor"].as_str()), (vec![next.as_str()], Some(next.as_str())));

    let unknown = "0".repeat(64);
    let err = bracken_fails(&laptop, &["log", "--since", &format!("{next},{unknown}")]);
    assert!(err.contains("unknown id") && err.contains(&unknown), "{err}");
}

/// `log --compact` and `heads --compact` print the shortest unique prefix, at least 8 digits, and it
/// resolves. When a new contribution shares those 8 digits, the old prefix becomes ambiguous and the
/// compact form grows to one that isn't. The pair that share 8 digits is found by making candidate claims
/// (about 80,000, on every core) and adding just those two.
#[test]
fn compact_ids_are_the_shortest_unique_prefix_and_grow_when_one_becomes_ambiguous() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "compact"]);
    bracken(dir, &["id", "new", "alice"]);
    bracken(dir, &["claim", "a line\nand another", "--tag", "t", "--as", "alice"]);

    let mut store = manyroot::Store::open(&dir.join(".bracken")).unwrap();
    let alice = store.identity("alice").unwrap();
    let (ground, heads) = (store.info().ground, store.graph().heads().collect::<Vec<_>>());
    let candidate = |n: u64| {
        let body = manyroot::Body::Claim { statement: format!("candidate {n}"), tags: vec![] };
        manyroot::Contribution::create(ground, &alice, heads.clone(), n, body)
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let mut by_prefix: std::collections::HashMap<[u8; 4], u64> = std::collections::HashMap::new();
    let (mut start, chunk) = (0u64, 4096u64);
    let (a, b) = 'search: loop {
        let found: Vec<Vec<([u8; 4], u64)>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|t| {
                    let from = start + t * chunk;
                    scope.spawn(move || {
                        (from..from + chunk)
                            .map(|n| (candidate(n).id().as_bytes()[..4].try_into().unwrap(), n))
                            .collect()
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        for (prefix, n) in found.into_iter().flatten() {
            if let Some(&m) = by_prefix.get(&prefix) {
                break 'search (candidate(m), candidate(n));
            }
            by_prefix.insert(prefix, n);
        }
        start += threads * chunk;
        assert!(start < 2_000_000, "no two of {start} candidates share 8 hex digits");
    };
    let (a_hex, b_hex) = (a.id().to_hex(), b.id().to_hex());
    assert_eq!(a_hex[..8], b_hex[..8]);

    store.add(a).unwrap();
    let compact = |args: &[&str]| -> Vec<String> { bracken(dir, args).lines().map(str::to_owned).collect() };
    let log = compact(&["log", "--compact"]);
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].ends_with(" alice claim [t] a line …"), "{}", log[0]);
    let line = log.iter().find(|l| a_hex.starts_with(l.split(' ').next().unwrap())).expect("a's line");
    let prefix = line.split(' ').next().unwrap();
    assert_eq!(prefix, &a_hex[..8], "{line}");
    assert!(bracken(dir, &["show", prefix]).contains(&a_hex));
    assert_eq!(compact(&["heads", "--compact"]), vec![line.clone()]);

    store.add(b).unwrap();
    let err = bracken_fails(dir, &["show", prefix]);
    assert!(err.contains("matches 2 contributions"), "{err}");
    let log = compact(&["log", "--compact"]);
    let grown: Vec<&str> = log.iter().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(grown.len(), 3);
    for (hex, want) in [(&a_hex, &a_hex), (&b_hex, &b_hex)] {
        let p = grown.iter().find(|p| hex.starts_with(**p)).expect("a line for each");
        assert!(p.len() > 8, "{p} is still ambiguous");
        assert!(bracken(dir, &["show", p]).contains(want.as_str()));
    }
}

/// However long a Ground's statements, tags and notes are, `log --compact` prints one line per contribution
/// of at most 200 characters, so a long Ground's log stays small. `--limit` keeps the most recent N after
/// the filters, says how many it left out, and still ends with the heads as the cursor.
#[test]
fn compact_lines_stay_short_and_limit_keeps_the_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "long"]);
    bracken(dir, &["id", "new", "alice"]);
    let mut store = manyroot::Store::open(&dir.join(".bracken")).unwrap();
    let alice = store.identity("alice").unwrap();
    let statement = format!("{}\nand a second line", "a long statement ".repeat(80));
    let tags: Vec<String> = (0..20).map(|i| format!("a-rather-long-tag-number-{i}")).collect();
    let (mut first, mut last) = (None, None);
    for i in 0..300 {
        let body = if i % 3 == 2 {
            let (from, to) = (last.unwrap(), first.unwrap());
            manyroot::Body::Relation { from, kind: manyroot::RelationKind::Supports, to, note: "n".repeat(5000) }
        } else {
            manyroot::Body::Claim { statement: statement.clone(), tags: tags.clone() }
        };
        last = Some(store.contribute(&alice, body).unwrap());
        first = first.or(last);
    }

    let log = bracken(dir, &["log", "--compact"]);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 300);
    for line in &lines {
        assert!(line.chars().count() <= 200, "{} characters: {line}", line.chars().count());
    }
    assert!(log.len() < 300 * 200 * 2, "{} bytes", log.len());

    let out = bracken(dir, &["log", "--since", "", "--limit", "5", "--compact", "--kind", "claim"]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 7, "{out}");
    assert!(lines[..5].iter().all(|l| l.contains(" claim ")), "{out}");
    assert_eq!(lines[5], "omitted: 195 earlier (--limit)");
    let cursor = lines[6].strip_prefix("cursor: ").unwrap().to_owned();
    assert_eq!(cursor, last.unwrap().to_hex(), "the cursor is the one head");
    let v = json(dir, &["log", "--limit", "2", "--json"]);
    assert_eq!((v["contributions"].as_array().unwrap().len(), v["omitted"].as_u64()), (2, Some(298)));
    let next = bracken(dir, &["claim", "the next one", "--as", "alice"]);
    let out = bracken(dir, &["log", "--since", &cursor, "--limit", "5", "--full"]);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(out.starts_with(&next[..12]) && out.ends_with(&format!("cursor: {next}")), "{out}");
}

/// Bounded task-start reads (task `f5231b1cde7f`): `log --tag` prints the compact form by default and
/// `--full` the full one; `--since` takes a date as well as a cursor; `--limit` keeps the newest N; and
/// `--open` lists only the findings still open.
#[test]
fn log_tag_is_compact_and_since_a_date_limit_and_open_bound_it() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "bounded"]);
    bracken(dir, &["id", "new", "alice"]);
    let long = format!("Decision: {}", "a long reason ".repeat(40));
    let d = bracken(dir, &["claim", &long, "--tag", "decision", "--as", "alice"]);
    let compact = bracken(dir, &["log", "--tag", "decision"]);
    assert!(compact.lines().count() == 1 && compact.chars().count() <= 200, "{compact}");
    assert!(compact.starts_with(&d[..8]) && compact.ends_with('…'), "{compact}");
    let full = bracken(dir, &["log", "--tag", "decision", "--full"]);
    assert!(full.contains(&long), "{full}");
    assert!(bracken(dir, &["log"]).contains(&long), "a plain log stays full");
    let err = bracken_fails(dir, &["log", "--full"]);
    assert!(err.contains("--full only undoes"), "{err}");

    // --since a date, by the contributions' stamps: two days apart, written straight to the store.
    let mut store = manyroot::Store::open(&dir.join(".bracken")).unwrap();
    let alice = store.identity("alice").unwrap();
    let ground = store.info().ground;
    let day = |n: u64| 1_793_145_600_000 + n * 86_400_000; // 2026-10-28T00:00:00Z, then days on
    let mut stamped = Vec::new();
    for (n, ms) in [(0, day(0) + 3_600_000), (1, day(1) + 66_000_000), (2, day(2) + 1)] {
        let heads: Vec<_> = store.graph().heads().collect();
        let body = manyroot::Body::Claim { statement: format!("dated {n}"), tags: vec!["dated".into()] };
        let c = manyroot::Contribution::create(ground, &alice, heads, ms, body);
        stamped.push(c.id().to_hex());
        store.add(c).unwrap();
    }
    drop(store);
    let since = |date: &str| -> Vec<String> {
        let v = json(dir, &["log", "--tag", "dated", "--since", date, "--json"]);
        v["contributions"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(since("2026-10-28"), stamped);
    assert_eq!(since("2026-10-29"), stamped[1..]);
    assert_eq!(since("2026-10-29T18:20:00Z"), stamped[1..], "stamped at the time itself counts");
    assert_eq!(since("2026-10-29T18:20:01Z"), stamped[2..], "a time, not only a day");
    assert_eq!(since("2026-10-31"), Vec::<String>::new());
    let text = bracken(dir, &["log", "--tag", "dated", "--since", "2026-10-29"]);
    assert_eq!(text.lines().count(), 3, "two compact lines and the cursor: {text}");
    let err = bracken_fails(dir, &["log", "--since", "2026-13-40"]);
    assert!(err.contains("neither a cursor nor a date"), "{err}");

    // --limit with --tag: the newest N.
    let out = bracken(dir, &["log", "--tag", "dated", "--limit", "1"]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(stamped[2].starts_with(lines[0].split(' ').next().unwrap()), "{out}");
    assert_eq!(lines[1], "omitted: 2 earlier (--limit)");

    // --open: the findings scripts/merge would block a current head on, by its rules (process 4.2): only
    // the triager's evidence triages, their pass stays confirmed, and a contradicts link retracted by
    // anyone but the finding's author or the triager still counts.
    for who in ["builder", "reviewer", "manager"] {
        bracken(dir, &["id", "new", who]);
    }
    let claim = |text: &str, tags: &[&str], who: &str| {
        let mut args = vec!["claim", text, "--as", who];
        for t in tags {
            args.extend(["--tag", t]);
        }
        bracken(dir, &args).lines().next().unwrap().to_owned()
    };
    let finding = |text: &str| claim(text, &["finding"], "reviewer");
    let contradicts = |f: &str, pr: &str, who: &str| bracken(dir, &["relate", f, "contradicts", pr, "--as", who]);
    let evidence = |f: &str, verdict: &str, who: &str| {
        bracken(dir, &["evidence", f, verdict, "--method", "triage", "--as", who]);
    };
    let old_pr = claim("PR #1 a @ 1", &["pr"], "builder");
    let new_pr = bracken(dir, &["claim", "PR #1 a @ 2", "--tag", "pr", "--supersedes", &old_pr, "--as", "builder"]);
    let pr = new_pr.lines().next().unwrap().to_owned();
    let on_pr = |text: &str| {
        let f = finding(text);
        contradicts(&f, &pr, "reviewer");
        f
    };
    let mut open = Vec::new();
    let mut closed = Vec::new();

    open.push(on_pr("untriaged"));
    let f = on_pr("confirmed by the manager");
    evidence(&f, "pass", "manager");
    open.push(f);
    let f = finding("confirmed, on the old head only");
    contradicts(&f, &old_pr, "reviewer");
    evidence(&f, "pass", "manager");
    closed.push(f);
    let f = on_pr("rejected by the manager");
    evidence(&f, "fail", "manager");
    closed.push(f);
    let f = on_pr("withdrawn");
    bracken(dir, &["retract", &f, "--as", "reviewer"]);
    closed.push(f);
    closed.push(finding("contradicting nothing"));
    // 090f4645800f: a builder's fail doesn't triage, and doesn't undo the manager's pass; nor does the
    // manager's own fail beside their pass.
    let f = on_pr("a builder's fail, no manager evidence");
    evidence(&f, "fail", "builder");
    open.push(f);
    let f = on_pr("the manager's pass, then a builder's fail");
    evidence(&f, "pass", "manager");
    evidence(&f, "fail", "builder");
    open.push(f);
    let f = on_pr("the manager's pass and fail");
    evidence(&f, "pass", "manager");
    evidence(&f, "fail", "manager");
    open.push(f);
    let f = on_pr("its author's own pass only");
    evidence(&f, "pass", "reviewer");
    open.push(f);
    // 7c38339206c5: the builder links it and then retracts the link: it still counts. The finding's
    // author retracting their own link drops it.
    let f = finding("linked by the builder, who retracts the link");
    let link = contradicts(&f, &pr, "builder");
    evidence(&f, "pass", "manager");
    bracken(dir, &["retract", &link, "--as", "builder"]);
    open.push(f);
    let f = finding("its author retracts their own link");
    let link = contradicts(&f, &pr, "reviewer");
    evidence(&f, "pass", "manager");
    bracken(dir, &["retract", &link, "--as", "reviewer"]);
    closed.push(f);
    // Deferred by the manager: a current task of theirs tagged deferred depends on it. Someone else's
    // deferral doesn't count.
    let f = on_pr("confirmed and deferred");
    evidence(&f, "pass", "manager");
    let task = claim("later", &["task", "deferred"], "manager");
    bracken(dir, &["relate", &task, "depends_on", &f, "--as", "manager"]);
    closed.push(f);
    // 8c90edd63f5c: a contradiction is open whatever its tags, as scripts/merge counts it.
    for tags in [&["note"][..], &[][..]] {
        let f = claim("a disagreement, not tagged finding", tags, "builder");
        contradicts(&f, &pr, "builder");
        open.push(f);
    }
    let f = on_pr("confirmed, deferred by the builder");
    evidence(&f, "pass", "manager");
    let task = claim("later", &["task", "deferred"], "builder");
    bracken(dir, &["relate", &task, "depends_on", &f, "--as", "builder"]);
    open.push(f);

    let listed = ids(dir, &["log", "--open", "--json"]);
    for f in &open {
        assert!(listed.contains(f), "{f} is open: {listed:?}");
    }
    for f in &closed {
        assert!(!listed.contains(f), "{f} is closed: {listed:?}");
    }
    assert_eq!(listed.len(), open.len(), "{listed:?}");
    let text = bracken(dir, &["log", "--open"]);
    assert_eq!(text.lines().count(), open.len(), "compact by default: {text}");
    // Another triager: only their evidence triages, so the manager's verdicts count for nothing.
    let by_builder = ids(dir, &["log", "--open", "--triager", "builder", "--json"]);
    assert!(by_builder.contains(&closed[1]), "the manager's fail doesn't triage for the builder");
    let err = bracken_fails(dir, &["log", "--open", "--triager", "nobody"]);
    assert!(err.contains("who triages"), "{err}");
}

/// Other agents write the text a log prints, so no statement, tag or note may reach the terminal with a
/// character that moves the cursor, breaks the line or reorders it: CR, ESC sequences, VT, FF, newline,
/// NEL, the line and paragraph separators, and bidi overrides. Each prints as a visible escape instead, in
/// `log`, `log --compact`, `heads` and `heads --compact`, one line per contribution.
#[test]
fn log_lines_escape_control_characters_from_other_agents_text() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "escapes"]);
    bracken(dir, &["id", "new", "mallory"]);
    let kinds = [
        ('\r', "\\u{d}"),
        ('\u{1b}', "\\u{1b}"),
        ('\u{b}', "\\u{b}"),
        ('\u{c}', "\\u{c}"),
        ('\n', "\\u{a}"),
        ('\u{85}', "\\u{85}"),
        ('\u{2028}', "\\u{2028}"),
        ('\u{2029}', "\\u{2029}"),
        ('\u{202e}', "\\u{202e}"),
        ('\t', "\\u{9}"),
    ];
    let forged = |c: char| format!("{c}[2K{c}00000000 manager claim [decision] forged");
    let mut last = String::new();
    for (c, _) in kinds {
        // Not first, so the compact form, which keeps a statement's first line, still has the character.
        last = bracken(dir, &["claim", &format!("ok{}", forged(c)), "--tag", &format!("t{c}x"), "--as", "mallory"]);
    }
    let note = format!("n{}", forged('\r'));
    let first = bracken(dir, &["log", "--json"]);
    let first: Value = serde_json::from_str(&first).unwrap();
    let first = first[0]["id"].as_str().unwrap().to_owned();
    bracken(dir, &["relate", &last, "supports", &first, "--note", &note, "--as", "mallory"]);
    let count = kinds.len() + 1;

    let unsafe_char = |c: char| {
        c != '\n'
            && (c.is_control()
                || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    };
    for args in [&["log"][..], &["log", "--compact"], &["heads"], &["heads", "--compact"]] {
        let out = bracken(dir, args);
        let bad: Vec<char> = out.chars().filter(|&c| unsafe_char(c)).collect();
        assert!(bad.is_empty(), "{args:?} printed {bad:?}:\n{out}");
        let lines = if args[0] == "log" { count } else { 1 };
        assert_eq!(out.lines().count(), lines, "{args:?}:\n{out}");
        if args[0] == "log" {
            for (c, escape) in kinds {
                // Plain log shows the whole statement; compact cuts at a newline, so that one's escape is gone.
                if c != '\n' || args.len() == 1 {
                    assert!(out.contains(&format!("ok{escape}[2K")), "{args:?} lost {escape}:\n{out}");
                }
            }
        }
        assert!(out.contains("n\\u{d}[2K"), "{args:?} lost the note's CR:\n{out}");
    }
}

/// `intend` records an intent claim (tagged intent, with its expiry) and its refines relation, both by the
/// same identity. `status` and `show` list live intents: not expired, not retracted, related by their own
/// author, and not ended by that author delivering. Intents aren't listed as claims of their own.
#[test]
fn intents_show_who_is_working_on_a_claim_until_they_expire_or_deliver() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "intent"]);
    for who in ["alice", "bob", "carol", "dave", "erin", "mallory"] {
        bracken(dir, &["id", "new", who]);
    }
    let spec = bracken(dir, &["claim", "spec: parse dates", "--as", "alice"]);
    let now_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;

    // alice: live for 30 minutes.
    let before = now_ms();
    let out = bracken(dir, &["intend", &spec[..12], "--for", "30m", "--as", "alice"]);
    let printed: Vec<&str> = out.lines().collect();
    assert_eq!(printed.len(), 2, "{out}");
    let alice = printed[0].to_owned();
    let intent = json(dir, &["show", &alice, "--json"]);
    let tags: Vec<&str> = intent["body"]["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags[0], "intent");
    assert!(tags[1].starts_with("until:") && tags[1].ends_with('Z'), "{tags:?}");
    let relation = json(dir, &["show", printed[1], "--json"]);
    assert_eq!(relation["body"]["kind"], "refines");
    assert_eq!(relation["body"]["from"], alice.as_str());
    assert_eq!(relation["body"]["to"], spec.as_str());
    assert_eq!(relation["author_name"], "alice");

    // erin: expired, and nothing else ends it. Written by hand, as the convention allows.
    let old = bracken(
        dir,
        &["claim", "intent: on it", "--tag", "intent", "--tag", "until:2020-01-01T00:00:00Z", "--as", "erin"],
    );
    bracken(dir, &["relate", &old, "refines", &spec, "--as", "erin"]);
    // carol: retracted, which hides its relation too.
    let carol = bracken(dir, &["intend", &spec, "--as", "carol"]).lines().next().unwrap().to_owned();
    bracken(dir, &["retract", &carol, "--as", "carol"]);
    // mallory's intent, related to the spec by someone else: not mallory's intent on it.
    let mallory = bracken(
        dir,
        &["claim", "intent: mine", "--tag", "intent", "--tag", "until:2999-01-01T00:00:00Z", "--as", "mallory"],
    );
    bracken(dir, &["relate", &mallory, "refines", &spec, "--as", "bob"]);
    // dave: ended by delivering; bob's delivery doesn't end alice's.
    let dave = bracken(dir, &["intend", &spec, "--as", "dave"]).lines().next().unwrap().to_owned();
    std::fs::write(dir.join("f"), "x").unwrap();
    bracken(dir, &["artifact", "f", "--name", "dave.rs", "--implements", &spec, "--as", "dave"]);
    bracken(dir, &["artifact", "f", "--name", "bob.rs", "--implements", &spec, "--as", "bob"]);

    let v = json(dir, &["status", "--json"]);
    let claims = v["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1, "intents aren't claims of their own: {v}");
    let intents = claims[0]["intents"].as_array().unwrap();
    let ids: Vec<&str> = intents.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [alice.as_str()], "only alice's intent is live");
    assert_eq!(intents[0]["author_name"], "alice");
    let until = intents[0]["until_ms"].as_u64().unwrap();
    assert!(until >= before + 29 * 60_000 && until <= now_ms() + 30 * 60_000, "{until}");
    assert!(intents[0]["until"].as_str().unwrap().ends_with('Z') && intents[0]["age_ms"].as_u64().is_some());

    let text = bracken(dir, &["status", "--full"]);
    let row = text.lines().find(|l| l.contains(&alice[..12])).unwrap_or_else(|| panic!("{text}"));
    assert!(row.trim_start().starts_with("intent") && row.contains("alice") && row.contains("until "), "{row}");
    for gone in [&old, &carol, &mallory, &dave] {
        assert!(!text.contains(&gone[..12]), "{gone} shown live:\n{text}");
    }
    let shown = bracken(dir, &["show", &spec]);
    let section = shown.split("live intents:").nth(1).unwrap_or_else(|| panic!("{shown}"));
    assert!(section.contains(&alice[..12]) && !section.contains(&dave[..12]), "{shown}");
    let shown = json(dir, &["show", &spec, "--json"]);
    assert_eq!(shown["intents"][0]["id"], alice.as_str());
    assert_eq!(shown["intents"].as_array().unwrap().len(), 1);

    // A claim with only a live intent is listed, with nothing implementing it.
    let other = bracken(dir, &["claim", "spec: format dates", "--as", "alice"]);
    bracken(dir, &["intend", &other, "--for", "2h", "--as", "bob"]);
    let v = json(dir, &["status", "--json"]);
    let listed = v["claims"].as_array().unwrap().iter().find(|c| c["claim"]["id"] == other.as_str()).unwrap().clone();
    assert_eq!(listed["implemented_by"], serde_json::json!([]));
    assert_eq!(listed["intents"][0]["author_name"], "bob");

    // Intents aren't claims nothing implements, either.
    let v = json(dir, &["status", "--all", "--json"]);
    let unimplemented: Vec<&str> =
        v["unimplemented"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    for i in [&alice, &old] {
        assert!(!unimplemented.contains(&i.as_str()), "intent {i} listed as a claim: {unimplemented:?}");
    }

    // Bad input writes nothing.
    let log = bracken(dir, &["log", "--json"]);
    assert!(bracken_fails(dir, &["intend", "ffffffffffff", "--as", "alice"]).contains("no matching contribution"));
    assert!(bracken_fails(dir, &["intend", &spec, "--for", "soon", "--as", "alice"]).contains("soon"));
    assert_eq!(bracken(dir, &["log", "--json"]), log, "a failed intend wrote something");
}

/// Delivery ends an intent through the author's own implements relation: retracting that relation makes
/// the intent live again, but someone else retracting the contribution it relates doesn't. A claim tagged
/// `intent` without an expiry, or without a refines relation by its author, is an ordinary claim. An
/// expiry past what RFC 3339 can write is an error that writes nothing.
#[test]
fn delivery_and_intent_shape_are_the_authors_own() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "intent"]);
    for who in ["alice", "bob", "carol", "dave", "erin", "frank"] {
        bracken(dir, &["id", "new", who]);
    }
    let spec = bracken(dir, &["claim", "spec: parse dates", "--as", "alice"]);
    let live = |dir: &Path| -> Vec<String> {
        let shown = json(dir, &["show", &spec, "--json"]);
        shown["intents"].as_array().unwrap().iter().map(|i| i["author_name"].as_str().unwrap().to_owned()).collect()
    };
    std::fs::write(dir.join("f"), "x").unwrap();

    // bob delivers bob's own artifact, then retracts the delivery: live again.
    bracken(dir, &["intend", &spec, "--as", "bob"]);
    let art = bracken(dir, &["artifact", "f", "--name", "bob.rs", "--as", "bob"]);
    let delivery = bracken(dir, &["relate", &art, "implements", &spec, "--as", "bob"]);
    assert_eq!(live(dir), Vec::<String>::new());
    bracken(dir, &["retract", &delivery, "--as", "bob"]);
    assert_eq!(live(dir), ["bob"]);

    // carol delivers dave's artifact; dave retracting it doesn't revive carol's intent.
    bracken(dir, &["intend", &spec, "--as", "carol"]);
    let daves = bracken(dir, &["artifact", "f", "--name", "dave.rs", "--as", "dave"]);
    bracken(dir, &["relate", &daves, "implements", &spec, "--as", "carol"]);
    bracken(dir, &["retract", &daves, "--as", "dave"]);
    assert_eq!(live(dir), ["bob"], "someone else's retraction revived carol's intent");
    let text = bracken(dir, &["status", "--full"]);
    assert!(text.lines().filter(|l| l.trim_start().starts_with("intent")).count() == 1, "{text}");
    let compact = bracken(dir, &["status"]);
    assert!(compact.lines().filter(|l| l.starts_with("intent ")).count() == 1, "{compact}");

    // Tagged intent but not an intent: no expiry; an expiry but no refines; refines by someone else.
    let no_until = bracken(dir, &["claim", "intent: someday", "--tag", "intent", "--as", "erin"]);
    bracken(dir, &["relate", &no_until, "refines", &spec, "--as", "erin"]);
    let until = "until:2999-01-01T00:00:00Z";
    let no_refines = bracken(dir, &["claim", "intent: alone", "--tag", "intent", "--tag", until, "--as", "erin"]);
    let others = bracken(dir, &["claim", "intent: theirs", "--tag", "intent", "--tag", until, "--as", "frank"]);
    bracken(dir, &["relate", &others, "refines", &spec, "--as", "erin"]);
    let v = json(dir, &["status", "--all", "--json"]);
    let unimplemented: Vec<&str> =
        v["unimplemented"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    for c in [&no_until, &no_refines, &others] {
        assert!(unimplemented.contains(&c.as_str()), "{c} hidden as an intent: {v}");
    }
    assert_eq!(live(dir), ["bob"]);

    // Past the year 9999: an error, not a panic, and nothing written.
    let log = bracken(dir, &["log", "--json"]);
    let err = bracken_fails(dir, &["intend", &spec, "--for", "9000y", "--as", "alice"]);
    assert!(err.contains("year 9999") && !err.contains("panicked"), "{err}");
    assert_eq!(bracken(dir, &["log", "--json"]), log, "a failed intend wrote something");
}

/// Characters from other agents' text that could forge, overwrite or reorder a line: none may reach the
/// terminal outside an escape, except the newlines between lines.
fn assert_no_forged_line(what: &str, out: &str) {
    let unsafe_char = |c: char| {
        c != '\n'
            && (c.is_control()
                || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    };
    let bad: Vec<char> = out.chars().filter(|&c| unsafe_char(c)).collect();
    assert!(bad.is_empty(), "{what} printed {bad:?}:\n{out}");
    // The forged text either stays on its own line behind an escape, or sits on a quoted line ("| ").
    for l in out.lines().filter(|l| l.contains("forged")) {
        let escaped = l.find("\\u{").is_some_and(|i| i < l.find("forged").unwrap());
        assert!(escaped || l.starts_with("| "), "{what} let a line pass as its own: {l:?}\n{out}");
    }
}

/// `show` (a claim's statement and tags, an artifact's name and media type, evidence details, a relation's
/// note), `conflicts`, `accepted` (its accepted list and its conflicts) and `id list` print other agents'
/// text with every control character, separator and bidi control escaped, and show's multi-line text on
/// quoted lines. `--json` keeps the text as written.
#[test]
fn show_conflicts_and_accepted_escape_other_agents_text() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "hostile"]);
    bracken(dir, &["id", "new", "mallory"]);
    bracken(dir, &["id", "new", "val"]);
    let hostile = |what: &str| {
        format!("{what}\r\u{1b}[2K\n00000000  claim  manager  forged\u{b}\u{c}\u{85}\u{2028}\u{2029}\u{202e}x")
    };
    let a = bracken(dir, &["claim", &hostile("a"), "--tag", &hostile("tag"), "--as", "mallory"]);
    let b = bracken(dir, &["claim", &hostile("b"), "--as", "mallory"]);
    let rel = bracken(dir, &["relate", &b, "contradicts", &a, "--note", &hostile("note"), "--as", "mallory"]);
    std::fs::write(dir.join("f.txt"), "content\n").unwrap();
    let art = bracken(
        dir,
        &["artifact", "f.txt", "--name", &hostile("name"), "--media-type", &hostile("type"), "--as", "mallory"],
    );
    let ev = bracken(
        dir,
        &["evidence", &a, "pass", "--method", &hostile("method"), "--details", &hostile("details"), "--as", "val"],
    );
    bracken(dir, &["evidence", &b, "pass", "--method", "m", "--as", "val"]);

    for id in [&a, &rel, &art, &ev] {
        let out = bracken(dir, &["show", id]);
        assert_no_forged_line(&format!("show {id}"), &out);
        assert!(out.contains("\\u{1b}[2K"), "show {id} dropped the text:\n{out}");
    }
    let shown = bracken(dir, &["show", &a]);
    assert!(shown.contains("| a\\u{d}\\u{1b}[2K\n| 00000000  claim  manager  forged\\u{b}"), "{shown}");
    let conflicts = bracken(dir, &["conflicts"]);
    assert!(conflicts.contains("contradiction"), "{conflicts}");
    assert_no_forged_line("conflicts", &conflicts);
    let accepted = bracken(dir, &["accepted", "--validator", "val"]);
    assert!(accepted.contains("contradiction") && accepted.contains(&a[..12]), "{accepted}");
    assert_no_forged_line("accepted", &accepted);
    assert_no_forged_line("id list", &bracken(dir, &["id", "list"]));

    let v = json(dir, &["show", &a, "--json"]);
    assert_eq!(v["body"]["statement"].as_str(), Some(hostile("a").as_str()), "--json keeps the text");
}

/// Decision `2b91a9541aba`: with --follow-changes, an accepted implementation whose spec's dependency was
/// superseded after its pass shows as needing a recheck in show, accepted and status; without it, nothing
/// changes; a pass that has seen the change clears it.
#[test]
fn follow_changes_asks_for_a_recheck_after_a_dependency_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "follow"]);
    for who in ["setter", "writer", "val"] {
        bracken(dir, &["id", "new", who]);
    }
    let spec = bracken(dir, &["claim", "spec: dist is the distance", "--as", "setter"]);
    let helper = bracken(dir, &["claim", "spec: abs is the magnitude", "--as", "setter"]);
    bracken(dir, &["relate", &spec, "depends_on", &helper, "--as", "setter"]);
    std::fs::write(dir.join("f"), "dist").unwrap();
    let work = bracken(dir, &["artifact", "f", "--name", "dist", "--as", "writer"]);
    bracken(dir, &["relate", &work, "implements", &spec, "--as", "writer"]);
    bracken(dir, &["evidence", &work, "pass", "--method", "review", "--as", "val"]);
    // Other work, which the change doesn't touch.
    let other = bracken(dir, &["claim", "spec: neg flips the sign", "--as", "setter"]);
    std::fs::write(dir.join("g"), "neg").unwrap();
    let neg = bracken(dir, &["artifact", "g", "--name", "neg", "--as", "writer"]);
    bracken(dir, &["relate", &neg, "implements", &other, "--as", "writer"]);
    bracken(dir, &["evidence", &neg, "pass", "--method", "review", "--as", "val"]);
    bracken(dir, &["claim", "spec: abs saturates", "--supersedes", &helper, "--as", "setter"]);

    let show = |flags: &[&str]| json(dir, &[&["show", &work, "--json"], flags].concat())["assessment"].clone();
    assert_eq!(show(&[])["status"], "accepted");
    assert!(show(&[]).get("changes").is_none(), "off by default, the output is as before");
    let a = show(&["--follow-changes"]);
    assert_eq!(a["status"], "needs_recheck");
    assert_eq!(a["changes"].as_array().map(Vec::len), Some(1));
    let text = bracken(dir, &["show", &work, "--follow-changes"]);
    assert!(text.contains("status    needs_recheck") && text.contains("changed   by"), "{text}");

    let accepted = |flags: &[&str]| -> Vec<String> {
        json(dir, &[&["accepted", "--json"], flags].concat())["accepted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(accepted(&[]).contains(&work));
    assert!(!accepted(&["--follow-changes"]).contains(&work), "out of the accepted layer");

    let status = bracken(dir, &["status", "--follow-changes"]);
    let row = status.lines().find(|l| l.contains(&work[..8])).unwrap_or_else(|| panic!("{status}"));
    assert!(row.trim_start().starts_with("recheck"), "{status}");
    let s = json(dir, &["status", "--follow-changes", "--json"]);
    let rows: Vec<&Value> =
        s["claims"].as_array().unwrap().iter().flat_map(|c| c["implemented_by"].as_array().unwrap()).collect();
    let row = rows.iter().find(|r| r["id"] == work.as_str()).unwrap();
    assert_eq!(
        (&row["waiting"], &row["standing"]),
        (&serde_json::json!(true), &serde_json::json!("recheck")),
        "it needs a pass, as waiting work does"
    );
    // In the review queue it comes with the work needing a pass, before accepted work, and waits for its
    // earlier validator's pass too.
    let queue: Vec<(String, String)> = s["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| (q["work"].as_str().unwrap().to_string(), q["standing"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(queue, [(work.clone(), "recheck".to_string()), (neg.clone(), "accepted".to_string())]);
    let mine = json(dir, &["status", "--follow-changes", "--json", "--as", "val"]);
    assert_eq!(mine["for_your_pass"][0]["work"], work.as_str(), "{mine}");
    assert_eq!(mine["for_your_pass"].as_array().map(Vec::len), Some(1));
    let plain = bracken(dir, &["status"]);
    assert!(plain.lines().any(|l| l.contains(&work[..8]) && l.trim_start().starts_with("accepted")), "{plain}");

    // A pass that has seen the change clears it.
    bracken(dir, &["evidence", &work, "pass", "--method", "review again", "--as", "val"]);
    assert_eq!(show(&["--follow-changes"])["status"], "accepted");
    assert!(accepted(&["--follow-changes"]).contains(&work));
}

/// `status` is a review queue: what needs an independent pass first, oldest first, then blocked work,
/// rejected work, and accepted work last, one short line each. A fail that names what it needs with
/// `evidence --depends-on` shows its work blocked while any of that is unaccepted, then ready. Nothing
/// here is code: every piece of work is a claim that implements another.
#[test]
fn status_is_a_review_queue_with_blocked_and_ready_work() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "queue"]);
    for who in ["alice", "bob", "carol"] {
        bracken(dir, &["id", "new", who]);
    }
    let goal = |text: &str| bracken(dir, &["claim", text, "--as", "alice"]);
    let answer = |text: &str, to: &str, who: &str| {
        let id = bracken(dir, &["claim", text, "--as", who]);
        bracken(dir, &["relate", &id, "implements", to, "--as", who]);
        id
    };
    let verdict = |id: &str, v: &str, who: &str, needs: &[&str]| {
        let mut args = vec!["evidence", id, v, "--method", "read it", "--as", who];
        for n in needs {
            args.extend(["--depends-on", n]);
        }
        bracken(dir, &args)
    };
    let (g1, g2, g3, g4) = (goal("goal one"), goal("goal two"), goal("goal three"), goal("goal four"));
    let accepted = answer("accepted answer", &g1, "bob");
    verdict(&accepted, "pass", "carol", &[]);
    let old = answer("older waiting answer", &g2, "bob");
    let rejected = answer("rejected answer", &g3, "bob");
    verdict(&rejected, "fail", "carol", &[]);
    let base = answer("the base it needs", &g4, "carol");
    let blocked = answer("blocked answer", &g3, "carol");
    let out = verdict(&blocked, "fail", "bob", &[&base]);
    assert_eq!(out.lines().count(), 2, "the evidence, then its depends_on relation: {out}");
    let new = answer("newer waiting answer", &g2, "carol");

    let order = |text: &str, ids: &[&str]| -> Vec<usize> {
        ids.iter()
            .map(|id| text.lines().position(|l| l.contains(&id[..8])).unwrap_or_else(|| panic!("{id}: {text}")))
            .collect()
    };
    let text = bracken(dir, &["status"]);
    // base and old/new wait; then blocked, rejected, accepted.
    let at = order(&text, &[&old, &base, &new, &blocked, &rejected, &accepted]);
    assert!(at.windows(2).all(|w| w[0] < w[1]), "queue order {at:?}:\n{text}");
    let row = |id: &str| text.lines().find(|l| l.contains(&id[..8])).unwrap().to_owned();
    assert!(row(&old).starts_with("waiting ") && row(&accepted).starts_with("accepted "), "{text}");
    assert!(row(&rejected).starts_with("rejected "), "{text}");
    assert!(
        row(&blocked).starts_with("blocked ") && row(&blocked).contains(&format!("needs {}", &base[..8])),
        "{text}"
    );
    assert!(text.lines().all(|l| l.chars().count() <= 200 && !l.contains("passes")), "short, no pass counts:\n{text}");

    // Once what it needs is accepted, the blocked work is ready, and waits for a pass again.
    verdict(&base, "pass", "alice", &[]);
    let text = bracken(dir, &["status"]);
    let ready = text.lines().find(|l| l.contains(&blocked[..8])).unwrap();
    assert!(ready.starts_with("ready "), "{text}");
    assert!(order(&text, &[&blocked, &rejected])[0] < order(&text, &[&rejected])[0], "{text}");
    let v = json(dir, &["status", "--json"]);
    let row = v["claims"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["implemented_by"].as_array().unwrap())
        .find(|i| i["id"] == blocked.as_str())
        .unwrap()
        .clone();
    assert_eq!((row["standing"].as_str(), row["blocked_on"].as_array().unwrap().len()), (Some("ready"), 0));
    assert_eq!(v["queue"][0]["work"], old.as_str(), "the queue starts with the oldest waiting work");

    // --limit bounds the lines and says how many more; --full is each claim with everything.
    let short = bracken(dir, &["status", "--limit", "2"]);
    assert_eq!(short.lines().count(), 3, "{short}");
    assert!(short.ends_with("more: status --limit <N>, or status --full"), "{short}");
    assert!(bracken(dir, &["status", "--full"]).contains("1/1 passes"));

    // --depends-on goes with a fail, and a bad id writes nothing.
    let before = bracken(dir, &["log", "--json"]);
    let err = bracken_fails(dir, &["evidence", &new, "pass", "--method", "m", "--depends-on", &base, "--as", "bob"]);
    assert!(err.contains("goes with a fail"), "{err}");
    // A run's verdict isn't known before it runs, so --depends-on doesn't go with --run.
    let err = bracken_fails(dir, &["evidence", &new, "--run", "--depends-on", &base, "--as", "bob", "--", "false"]);
    assert!(err.contains("cannot be used with"), "{err}");
    let err =
        bracken_fails(dir, &["evidence", &new, "fail", "--method", "m", "--depends-on", "ffffffffffff", "--as", "bob"]);
    assert!(err.contains("no matching contribution"), "{err}");
    assert_eq!(bracken(dir, &["log", "--json"]), before, "a refused evidence wrote something");
}

/// With --as, `status` and `log --since` are personal: your work, contradictions of your claims and
/// the work waiting for your pass come first. Work you already passed isn't waiting for you.
#[test]
fn status_and_log_since_put_what_concerns_the_caller_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "personal"]);
    for who in ["alice", "bob", "carol"] {
        bracken(dir, &["id", "new", who]);
    }
    let goal = bracken(dir, &["claim", "a goal", "--as", "alice"]);
    let answer = |text: &str, who: &str| {
        let id = bracken(dir, &["claim", text, "--as", who]);
        bracken(dir, &["relate", &id, "implements", &goal, "--as", who]);
        id
    };
    let bobs = answer("bob's answer", "bob");
    let carols = answer("carol's answer", "carol");
    let passed = answer("carol's other answer", "carol");
    bracken(dir, &["evidence", &passed, "pass", "--method", "m", "--as", "bob"]);
    let cursor = json(dir, &["log", "--since", "", "--json"])["cursor"].as_str().unwrap().to_owned();
    let objection = bracken(dir, &["claim", "that's wrong", "--as", "carol"]);
    bracken(dir, &["relate", &objection, "contradicts", &bobs, "--as", "carol"]);
    let verdict = bracken(dir, &["evidence", &bobs, "fail", "--method", "m", "--as", "carol"]);
    bracken(dir, &["claim", "unrelated chatter", "--as", "alice"]);

    let text = bracken(dir, &["status", "--as", "bob"]);
    let at = |title: &str| text.lines().position(|l| l == title).unwrap_or_else(|| panic!("{title}:\n{text}"));
    let line_of = |id: &str| text.lines().position(|l| l.contains(&id[..8])).unwrap_or_else(|| panic!("{id}:\n{text}"));
    assert!(at("yours:") < line_of(&bobs) && line_of(&bobs) < at("contradicting your claims:"), "{text}");
    assert!(at("contradicting your claims:") < at("waiting for your pass:"), "{text}");
    assert!(at("waiting for your pass:") < line_of(&carols) && line_of(&carols) < at("everything else:"), "{text}");
    assert!(at("everything else:") < line_of(&passed), "work bob passed isn't waiting for him:\n{text}");
    let v = json(dir, &["status", "--as", "bob", "--json"]);
    assert_eq!(v["for_your_pass"][0]["work"], carols.as_str());
    assert_eq!(v["contradictions_of_yours"].as_array().unwrap().len(), 1);
    // Without a caller, one queue; with an unknown one, it says so.
    assert!(!bracken(dir, &["status"]).contains("yours:"));
    assert!(bracken(dir, &["status", "--as", "nobody"]).starts_with("not personal: nobody is unknown here"));

    // log --since: what concerns bob (carol's verdict on his work, her contradiction) first.
    let news = bracken(dir, &["log", "--since", &cursor, "--as", "bob"]);
    let lines: Vec<&str> = news.lines().collect();
    assert_eq!(lines[0], "for you:", "{news}");
    let pos =
        |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{news}"));
    assert!(
        pos(&verdict[..8]) < pos("everything else:") && pos("everything else:") < pos("unrelated chatter"),
        "{news}"
    );
    assert!(lines.iter().all(|l| l.chars().count() <= 200), "compact by default:\n{news}");
    let v = json(dir, &["log", "--since", &cursor, "--as", "bob", "--json"]);
    let for_you: Vec<&str> = v["for_you"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
    assert!(for_you.contains(&verdict.as_str()) && for_you.len() == 2, "{v}");
    let ids: Vec<&str> = v["contributions"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 4, "the JSON keeps every row, in causal order: {v}");
}

/// Finding `10447f6d7e9f`: only each validator's latest fail decides blocked or ready. An earlier plain
/// fail doesn't keep the work rejected, and an earlier fail's need that was since replaced doesn't keep it
/// blocked. A need that can never be accepted is refused.
#[test]
fn only_each_validators_latest_fail_decides_blocked_or_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "latest"]);
    for who in ["alice", "bob", "carol"] {
        bracken(dir, &["id", "new", who]);
    }
    let goal = bracken(dir, &["claim", "a goal", "--as", "alice"]);
    let answer = |text: &str, who: &str| {
        let id = bracken(dir, &["claim", text, "--as", who]);
        bracken(dir, &["relate", &id, "implements", &goal, "--as", who]);
        id
    };
    let fail = |id: &str, needs: &[&str]| {
        let mut args = vec!["evidence", id, "fail", "--method", "read it", "--as", "bob"];
        for n in needs {
            args.extend(["--depends-on", n]);
        }
        bracken(dir, &args)
    };
    let standing = |id: &str| {
        let text = bracken(dir, &["status"]);
        text.lines().find(|l| l.contains(&id[..8])).unwrap_or_else(|| panic!("{id}:\n{text}")).to_owned()
    };

    // 1: a plain fail, then a fail naming what it needs; once that's accepted, ready.
    let w1 = answer("first work", "alice");
    let x = answer("what it needs", "carol");
    fail(&w1, &[]);
    fail(&w1, &[&x]);
    bracken(dir, &["evidence", &x, "pass", "--method", "m", "--as", "alice"]);
    assert!(standing(&w1).starts_with("ready "), "{}", standing(&w1));

    // 2: a fail naming x1; x1 is replaced by x2, never accepted; a fail naming x2; x2 accepted: ready.
    let w2 = answer("second work", "alice");
    let x1 = answer("a first need", "carol");
    fail(&w2, &[&x1]);
    let x2 = bracken(dir, &["claim", "its replacement", "--supersedes", &x1, "--as", "carol"]);
    let x2 = x2.lines().next().unwrap();
    bracken(dir, &["relate", x2, "implements", &goal, "--as", "carol"]);
    fail(&w2, &[x2]);
    assert!(standing(&w2).starts_with("blocked ") && standing(&w2).contains(&x2[..8]), "{}", standing(&w2));
    assert!(!standing(&w2).contains(&x1[..8]), "the replaced need: {}", standing(&w2));
    bracken(dir, &["evidence", x2, "pass", "--method", "m", "--as", "alice"]);
    assert!(standing(&w2).starts_with("ready "), "{}", standing(&w2));

    // A relation or an evidence can never be accepted: refused, and nothing written.
    let relation = bracken(dir, &["relate", &x, "supports", &goal, "--as", "carol"]);
    let before = bracken(dir, &["log", "--json"]);
    let err = bracken_fails(dir, &["evidence", &w1, "fail", "--method", "m", "--depends-on", &relation, "--as", "bob"]);
    assert!(err.contains("only a claim or an artifact"), "{err}");
    assert_eq!(bracken(dir, &["log", "--json"]), before);
}

/// Finding `d987014caf64`: what a fail needs is met by something accepted that implements it only by a
/// relation its own author or the need's author wrote. A stranger's relation leaves the work blocked.
#[test]
fn a_strangers_implements_doesnt_make_blocked_work_ready() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "stranger"]);
    for who in ["builder", "reviewer", "other", "stranger", "third"] {
        bracken(dir, &["id", "new", who]);
    }
    let task = bracken(dir, &["claim", "a task", "--as", "other"]);
    let work = bracken(dir, &["claim", "the work", "--as", "builder"]);
    bracken(dir, &["relate", &work, "implements", &task, "--as", "builder"]);
    let need = bracken(dir, &["claim", "what it needs", "--as", "other"]);
    // By a third author: neither the stranger nor the need's author wrote the relation's source.
    let x = bracken(dir, &["claim", "unrelated, accepted", "--as", "third"]);
    let policy = ["--validator", "reviewer"];
    bracken(dir, &["evidence", &x, "pass", "--method", "m", "--as", "reviewer"]);
    bracken(dir, &["evidence", &work, "fail", "--method", "m", "--depends-on", &need, "--as", "reviewer"]);
    let line = || {
        let mut args = vec!["status"];
        args.extend(policy);
        let text = bracken(dir, &args);
        text.lines().find(|l| l.contains(&work[..8])).unwrap_or_else(|| panic!("{text}")).to_owned()
    };
    assert!(line().starts_with("blocked "), "{}", line());
    bracken(dir, &["relate", &x, "implements", &need, "--as", "stranger"]);
    assert!(line().starts_with("blocked "), "a stranger's relation: {}", line());
    // The need's own author relating it does count, though they didn't write x: the queue trusts the
    // target's author too (Trusted::SOURCE_OR_TARGET).
    bracken(dir, &["relate", &x, "implements", &need, "--as", "other"]);
    assert!(line().starts_with("ready "), "{}", line());
}

/// `watch` without --follow: each new contribution the filters admit on one compact line, then the cursor;
/// that cursor round-trips, so the next watch prints only what came after it. Without --since it starts at
/// the heads. --tag, --open and --for-me filter as `log` and `status --as` do, and text is escaped.
#[test]
fn watch_filters_new_contributions_and_its_cursor_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "watch"]);
    for who in ["alice", "bob", "manager"] {
        bracken(dir, &["id", "new", who]);
    }
    // The text form ends with the cursor; the lines before it are the contributions' prefixes.
    let watch = |args: &[&str]| -> (Vec<String>, String) {
        let out = bracken(dir, &[&["watch"][..], args].concat());
        let (rows, last) = out.rsplit_once('\n').map_or(("", out.as_str()), |(r, l)| (r, l));
        let cursor = last.strip_prefix("cursor: ").unwrap_or_else(|| panic!("no cursor after: {out}")).to_owned();
        (rows.lines().map(|l| l.split_whitespace().next().unwrap().to_owned()).collect(), cursor)
    };
    let short = |id: &str| id[..8].to_owned();
    let goal = bracken(dir, &["claim", "alice's goal", "--tag", "task", "--as", "alice"]);
    let (rows, start) = watch(&[]);
    assert_eq!((rows, start.clone()), (vec![], goal.clone()), "without --since: nothing old, the heads");
    assert_eq!(watch(&["--since", ""]).0, vec![short(&goal)], "--since \"\" is everything");

    let chatter = bracken(dir, &["claim", "bob's chatter", "--tag", "note", "--as", "bob"]);
    let objection = bracken(dir, &["claim", "that goal is wrong", "--tag", "finding", "--as", "bob"]);
    let contradicts = bracken(dir, &["relate", &objection, "contradicts", &goal, "--as", "bob"]);
    let refinement = bracken(dir, &["claim", "a narrower goal", "--as", "bob"]);
    let refines = bracken(dir, &["relate", &refinement, "refines", &goal, "--as", "bob"]);
    let work = bracken(dir, &["claim", "bob's work", "--as", "bob"]);
    let implements = bracken(dir, &["relate", &work, "implements", &goal, "--as", "bob"]);
    let own = bracken(dir, &["claim", "alice talks to herself", "--as", "alice"]);
    bracken(dir, &["relate", &own, "refines", &goal, "--as", "alice"]);
    let tricky = bracken(dir, &["claim", "ev\u{1b}[2Kil\nfake line", "--as", "bob"]);

    let (rows, next) = watch(&["--since", &start]);
    assert_eq!(rows.len(), 10, "{rows:?}");
    assert_eq!(rows[0], short(&chatter));
    assert_eq!(next, tricky, "the cursor is the heads");
    assert_eq!(watch(&["--since", &next]), (vec![], next.clone()), "the cursor round-trips: nothing new");
    assert_eq!(watch(&["--since", &start, "--tag", "note"]), (vec![short(&chatter)], next.clone()));
    assert_eq!(watch(&["--since", &start, "--open"]).0, vec![short(&objection)], "an untriaged finding");
    let mut mine = watch(&["--since", &start, "--for-me", "--as", "alice"]).0;
    let mut expected = vec![short(&contradicts), short(&refines), short(&work), short(&implements)];
    mine.sort();
    expected.sort();
    assert_eq!(mine, expected, "others' contradicts and refines, and work waiting for alice's pass");
    let err = bracken_fails(dir, &["watch", "--for-me"]);
    assert!(err.contains("--for-me") && err.contains("--as"), "{err}");

    let text = bracken(dir, &["watch", "--since", &start]);
    let line = text.lines().find(|l| l.starts_with(&short(&tricky))).unwrap();
    assert!(line.contains("ev\\u{1b}[2Kil fake line") || line.contains("ev\\u{1b}[2Kil …"), "{line}");
    assert!(!text.contains('\u{1b}'), "{text}");

    let err = bracken_fails(dir, &["watch", "--since", &"0".repeat(64)]);
    assert!(err.contains("unknown id"), "{err}");
    let err = bracken_fails(dir, &["watch", "--interval", "3s"]);
    assert!(err.contains("--follow"), "{err}");
    let err = bracken_fails(dir, &["watch", "--follow", "--interval", "1s"]);
    assert!(err.contains("under the minimum"), "{err}");
}

/// `watch --json`: one JSON object per line, each new contribution as `log --json` has it with the cursor
/// just past it, then a line with only the cursor if the filters hid what came last.
#[test]
fn watch_json_is_one_object_per_line_each_with_a_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "watch-json"]);
    bracken(dir, &["id", "new", "alice"]);
    let start = bracken(dir, &["claim", "start", "--as", "alice"]);
    let a = bracken(dir, &["claim", "a", "--tag", "x", "--as", "alice"]);
    let b = bracken(dir, &["claim", "b", "--as", "alice"]);
    let out = bracken(dir, &["watch", "--since", &start, "--json"]);
    let lines: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "{out}");
    let c = &lines[0]["contribution"];
    let keys: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
    for key in ["id", "kind", "author", "author_name", "parents", "timestamp_ms", "retracted", "body"] {
        assert!(keys.contains(&key), "{key} missing from {c}");
    }
    assert_eq!((c["id"].as_str(), c["author_name"].as_str()), (Some(a.as_str()), Some("alice")));
    assert_eq!(c["body"]["tags"], serde_json::json!(["x"]));
    assert_eq!(lines[0]["cursor"], a.as_str(), "the cursor just past it");
    assert_eq!(lines[1]["contribution"]["id"], b.as_str());
    assert_eq!(lines[1]["cursor"], b.as_str());

    // Filtered: the last new contribution is hidden, so a cursor-only line ends it.
    let out = bracken(dir, &["watch", "--since", &start, "--json", "--tag", "x"]);
    let lines: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!((lines[0]["cursor"].as_str(), lines[1]["cursor"].as_str()), (Some(a.as_str()), Some(b.as_str())));
    assert!(lines[1].get("contribution").is_none(), "{out}");
    let out = bracken(dir, &["watch", "--since", &b, "--json"]);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&out).unwrap(), serde_json::json!({"cursor": b}));
}

/// The cursor doesn't skip what arrives out of order: a contribution synced in after a cursor was printed is
/// reported by the next watch, however old its timestamp, and only once.
#[test]
fn watch_reports_a_late_arrival_with_an_older_timestamp_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (laptop, server) = (tmp.path().join("laptop"), tmp.path().join("server"));
    std::fs::create_dir_all(&laptop).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    bracken(&laptop, &["init", "--name", "late"]);
    bracken(&laptop, &["id", "new", "alice"]);
    let first = bracken(&laptop, &["claim", "first", "--as", "alice"]);
    let bundle = tmp.path().join("seed.mrb");
    bracken(&laptop, &["export", bundle.to_str().unwrap()]);
    bracken(&server, &["join", bundle.to_str().unwrap()]);
    bracken(&server, &["id", "new", "bob"]);

    // Bob's claim, stamped in 1970 and on the first claim only, written while the laptop moves on.
    let mut store = manyroot::Store::open(&server.join(".bracken")).unwrap();
    let bob = store.identity("bob").unwrap();
    let body = manyroot::Body::Claim { statement: "late and old".into(), tags: vec![] };
    let old = manyroot::Contribution::create(store.info().ground, &bob, [first.parse().unwrap()], 1, body);
    let old_id = old.id().to_hex();
    store.add(old).unwrap();
    drop(store);
    let newer = bracken(&laptop, &["claim", "newer", "--as", "alice"]);
    let cursor = |out: &str| out.lines().last().unwrap().strip_prefix("cursor: ").unwrap().to_owned();
    let seen = bracken(&laptop, &["watch", "--since", &first]);
    assert!(seen.starts_with(&newer[..8]) && seen.lines().count() == 2, "{seen}");
    let at = cursor(&seen);
    assert_eq!(at, newer);

    bracken(&laptop, &["sync", "../server"]);
    let late = bracken(&laptop, &["watch", "--since", &at]);
    let lines: Vec<&str> = late.lines().collect();
    assert_eq!(lines.len(), 2, "{late}");
    assert!(lines[0].starts_with(&old_id[..8]) && lines[0].contains("late and old"), "{late}");
    let again = bracken(&laptop, &["watch", "--since", &cursor(&late)]);
    assert!(again.starts_with("cursor: "), "reported once: {again}");
    let mut heads = [newer, old_id];
    heads.sort();
    assert_eq!(cursor(&late), heads.join(","));
}

/// `watch --follow` prints what arrives while it runs, and exits 0 once stdout's reader goes away, even
/// though nothing more arrives to be written.
#[test]
fn watch_follow_streams_and_ends_on_a_closed_reader() {
    use std::io::BufRead;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "follow"]);
    bracken(dir, &["id", "new", "alice"]);
    let start = bracken(dir, &["claim", "start", "--as", "alice"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken"))
        .args(["watch", "--follow", "--interval", "2s", "--json"])
        .current_dir(dir)
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let line =
        |lines: &mut std::io::Lines<_>| -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };
    assert_eq!(line(&mut lines), serde_json::json!({"cursor": start}), "it starts at the heads");
    let next = bracken(dir, &["claim", "while following", "--as", "alice"]);
    let v = line(&mut lines);
    assert_eq!((v["contribution"]["id"].as_str(), v["cursor"].as_str()), (Some(next.as_str()), Some(next.as_str())));

    drop(lines);
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > std::time::Duration::from_secs(20) {
            child.kill().unwrap();
            panic!("watch --follow kept running after its reader went away");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(status.success(), "{status}");
}

/// Finding `baff8404bbf5`: `watch --open` reports a finding when it becomes open, which is usually when its
/// contradicts relation arrives, a round after the finding itself. It reports it once: not again when both
/// arrive in one batch, nor when the triager confirms it, nor when resuming from a cursor printed with it.
#[test]
fn watch_open_reports_a_finding_once_when_its_contradicts_arrives_later() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    bracken(dir, &["init", "--name", "watch-open"]);
    for who in ["alice", "bob", "manager"] {
        bracken(dir, &["id", "new", who]);
    }
    let watch = |since: &str| -> (Vec<String>, String) {
        let out = bracken(dir, &["watch", "--open", "--since", since]);
        let (rows, last) = out.rsplit_once('\n').map_or(("", out.as_str()), |(r, l)| (r, l));
        let cursor = last.strip_prefix("cursor: ").unwrap_or_else(|| panic!("no cursor after: {out}")).to_owned();
        (rows.lines().map(|l| l.split_whitespace().next().unwrap().to_owned()).collect(), cursor)
    };
    let pr = bracken(dir, &["claim", "PR #1", "--tag", "pr", "--as", "bob"]);
    let c0 = pr.clone();
    let finding = bracken(dir, &["claim", "it breaks", "--tag", "finding", "--as", "alice"]);
    let (rows, c1) = watch(&c0);
    assert!(rows.is_empty(), "not open before its contradicts: {rows:?}");
    bracken(dir, &["relate", &finding, "contradicts", &pr, "--as", "alice"]);
    let (rows, c2) = watch(&c1);
    assert_eq!(rows, vec![finding[..8].to_owned()], "open once its contradicts arrives");
    assert_eq!(watch(&c0).0, vec![finding[..8].to_owned()], "both in one batch: reported once");
    bracken(dir, &["evidence", &finding, "pass", "--method", "triage", "--as", "manager"]);
    let (rows, c3) = watch(&c2);
    assert!(rows.is_empty(), "a confirming triage doesn't report it again: {rows:?}");

    let lines = |since: &str| -> Vec<Value> {
        let out = bracken(dir, &["watch", "--open", "--since", since, "--json"]);
        out.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .filter(|v: &Value| v.get("contribution").is_some())
            .collect()
    };
    let ids = |lines: &[Value]| -> Vec<String> {
        lines.iter().map(|v| v["contribution"]["id"].as_str().unwrap().to_owned()).collect()
    };
    // Two findings in one batch: both at its end, with the cursor past all of it.
    let mut found = Vec::new();
    for text in ["second", "third"] {
        let f = bracken(dir, &["claim", text, "--tag", "finding", "--as", "alice"]);
        bracken(dir, &["relate", &f, "contradicts", &pr, "--as", "alice"]);
        found.push(f);
    }
    let both = lines(&c3);
    assert_eq!(ids(&both), found);
    assert_eq!(both[0]["cursor"], both[1]["cursor"]);
    assert!(lines(both[0]["cursor"].as_str().unwrap()).is_empty(), "resuming doesn't report them again");

    // Finding 408881286395: what makes the contradicted claim current again comes after the contradicts
    // relation in the batch. Resuming from the finding's line doesn't report it again.
    let p = bracken(dir, &["claim", "P", "--as", "bob"]);
    let ids_p2 = bracken(dir, &["claim", "P2", "--supersedes", &p, "--as", "bob"]);
    let supersedes = ids_p2.lines().nth(1).unwrap().to_owned();
    let f = bracken(dir, &["claim", "P is wrong", "--tag", "finding", "--as", "alice"]);
    let c = bracken(dir, &["watch", "--open", "--since", "", "--json"]);
    let c = serde_json::from_str::<Value>(c.lines().last().unwrap()).unwrap()["cursor"].as_str().unwrap().to_owned();
    bracken(dir, &["relate", &f, "contradicts", &p, "--as", "alice"]);
    bracken(dir, &["retract", &supersedes, "--as", "bob"]);
    let reported = lines(&c);
    assert_eq!(ids(&reported), vec![f.clone()], "open once P is current again");
    let again = lines(reported[0]["cursor"].as_str().unwrap());
    assert!(again.is_empty(), "resuming from its line reports it again: {again:?}");

    let err = bracken_fails(dir, &["watch", "--follow", "--interval", "1000years"]);
    assert!(err.contains("over the maximum"), "{err}");
}
