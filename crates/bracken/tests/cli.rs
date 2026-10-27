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
