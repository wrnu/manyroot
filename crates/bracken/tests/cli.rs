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
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
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
    let ids = |dir: &Path| -> Vec<Value> {
        json(dir, &["log", "--json"]).as_array().unwrap().iter().map(|c| c["id"].clone()).collect()
    };
    assert_eq!(ids(&laptop).len(), 7);
    assert_eq!(ids(&laptop), ids(&server), "same graph, same causal order");

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

    // The builder supersedes the finding with a claim of their own: a counted supersession, by another author.
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
