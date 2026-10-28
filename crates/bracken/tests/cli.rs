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
    let from_stdin = String::from_utf8(out.stdout).unwrap();
    assert_eq!(details(from_stdin.trim()), "exit 2\nfrom stdin\n");

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

    let text = bracken(dir, &["status"]);
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
    let all = bracken(dir, &["status", "--all"]);
    let tail = all.split("nothing implements, or intends to:").nth(1).unwrap_or_else(|| panic!("{all}"));
    assert!(tail.contains(&idle[..12]) && tail.contains("spec: new") && !tail.contains(&busy[..12]), "{all}");
    // A stricter policy: nothing has two passes, so all waits. A second pass by the same validator is
    // still one independent pass.
    pass(&accepted);
    let strict = bracken(dir, &["status", "--min-passes", "2"]);
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

    let text = bracken(dir, &["status", "--all"]);
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
    // One line for the claim, one for its implementer, then the "nothing implements" section.
    assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 4, "{text}");

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
        let out = bracken(dir, &[&["log", "--since", cursor][..], also].concat());
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
    let out = bracken(dir, &["log", "--since", &cursor, "--limit", "5"]);
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(out.starts_with(&next[..12]) && out.ends_with(&format!("cursor: {next}")), "{out}");
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

    let text = bracken(dir, &["status"]);
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
    for i in [&alice, &old, &mallory] {
        assert!(!unimplemented.contains(&i.as_str()), "intent {i} listed as a claim: {unimplemented:?}");
    }

    // Bad input writes nothing.
    let log = bracken(dir, &["log", "--json"]);
    assert!(bracken_fails(dir, &["intend", "ffffffffffff", "--as", "alice"]).contains("no matching contribution"));
    assert!(bracken_fails(dir, &["intend", &spec, "--for", "soon", "--as", "alice"]).contains("soon"));
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
