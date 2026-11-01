//! The verified snapshot (task `ac538d60e8f9`) against a full check, the oracle: on generated Grounds,
//! every read gives the same output through the snapshot as with `BRACKEN_FULL_CHECK=1`, after new work
//! (retractions and supersessions among it), writes through the CLI, a deleted object and a damaged
//! snapshot. A tampered object the snapshot already holds is the documented exception: the full check
//! and `bracken verify` catch it, a cached read doesn't.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use manyroot::{Body, ContributionId, Identity, RelationKind, Store, Verdict};

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }
}

const PEOPLE: [&str; 3] = ["manager", "builder", "reviewer"];

/// Adds about `n` contributions to the replica at `dir`: tagged claims, implementations, findings and
/// their links, evidence, supersessions and retractions of the author's own work.
fn grow(dir: &Path, seed: u64, n: usize) {
    let mut store = Store::open(dir).expect("valid");
    let ids: Vec<Identity> = PEOPLE.iter().map(|p| store.identity(p).expect("an identity")).collect();
    let mut rng = Rng(seed);
    for i in 0..n {
        let who = rng.below(ids.len());
        let all: Vec<ContributionId> = store.graph().topo_order();
        let pick = |rng: &mut Rng| all[rng.below(all.len())];
        let tags = ["pr", "finding", "task", "deferred", "note"];
        let body = match (all.len(), rng.below(12)) {
            (0..=2, _) | (_, 0..=3) => {
                Body::Claim { statement: format!("claim {seed}-{i}"), tags: vec![tags[rng.below(tags.len())].into()] }
            }
            (_, 4..=5) => Body::Evidence {
                target: pick(&mut rng),
                verdict: if rng.below(3) == 0 { Verdict::Fail } else { Verdict::Pass },
                method: "check".into(),
                details: String::new(),
            },
            (_, 6..=9) => {
                let (from, to) = (pick(&mut rng), pick(&mut rng));
                if from == to {
                    continue;
                }
                let kinds = [
                    RelationKind::Supersedes,
                    RelationKind::Implements,
                    RelationKind::Contradicts,
                    RelationKind::DependsOn,
                ];
                Body::Relation { from, kind: kinds[rng.below(kinds.len())], to, note: String::new() }
            }
            _ => {
                let me = ids[who].agent_id();
                let own: Vec<_> =
                    all.iter().filter(|id| store.graph().get(id).expect("present").author() == me).collect();
                if own.is_empty() {
                    continue;
                }
                Body::Retract { target: *own[rng.below(own.len())], reason: "withdrawn".into() }
            }
        };
        store.contribute(&ids[who], body).expect("valid");
    }
}

fn ground(dir: &Path, seed: u64, n: usize) {
    let store = Store::init(dir, "snapshot").expect("a Ground");
    for p in PEOPLE {
        store.create_identity(p).expect("an identity");
    }
    grow(dir, seed, n);
}

fn bracken(dir: &Path, full: bool, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bracken"));
    cmd.args(args).env("BRACKEN_DIR", dir).env_remove("BRACKEN_AGENT").env_remove("BRACKEN_FULL_CHECK");
    if full {
        cmd.env("BRACKEN_FULL_CHECK", "1");
    }
    cmd.output().expect("runs")
}

/// A read's exit, stdout and stderr, without `age_ms`, which counts from the clock.
fn read(dir: &Path, full: bool, args: &[&str]) -> (bool, String, String) {
    let out = bracken(dir, full, args);
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    let stdout: Vec<&str> = stdout.lines().filter(|l| !l.contains("\"age_ms\"")).collect();
    (out.status.success(), stdout.join("\n"), String::from_utf8(out.stderr).expect("utf-8"))
}

/// Every read, with ids of the Ground's own to show and check findings against.
fn reads(dir: &Path) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = [
        &["log", "--json"][..],
        &["log", "--current", "--json"],
        &["log", "--open", "--json"],
        &["log", "--compact"],
        &["heads", "--json"],
        &["status", "--all", "--json"],
        &["accepted", "--json"],
        &["conflicts", "--json"],
    ]
    .iter()
    .map(|a| a.iter().map(|s| (*s).to_owned()).collect())
    .collect();
    let g = Store::open(dir).expect("valid").graph().clone();
    let order = g.topo_order();
    for id in [order[0], order[order.len() / 2], *order.last().expect("some")] {
        out.push(vec!["show".into(), id.to_hex(), "--json".into()]);
        out.push(vec!["findings".into(), id.to_hex(), "--json".into()]);
    }
    out
}

/// Every read through the snapshot gives what the full check gives. Returns how many reads it compared.
fn agree(dir: &Path, when: &str) -> usize {
    let all = reads(dir);
    for args in &all {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let full = read(dir, true, &args);
        assert!(full.0, "{when}: {args:?} fails a full check: {}", full.2);
        assert_eq!(read(dir, false, &args), full, "{when}: {args:?}");
    }
    all.len()
}

fn snapshot(dir: &Path) -> PathBuf {
    dir.join("cache").join("snapshot")
}

fn object(dir: &Path, id: &ContributionId) -> PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

#[test]
fn every_read_through_the_snapshot_gives_the_full_checks_output() {
    for seed in [1, 2, 3] {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let dir = tmp.path().join("g");
        ground(&dir, seed, 150);
        assert!(!snapshot(&dir).exists(), "nothing written yet");
        agree(&dir, "built");
        let first = fs::read(snapshot(&dir)).expect("a read writes the snapshot");
        assert_eq!(fs::read_to_string(dir.join("cache/.gitignore")).expect("written").lines().last(), Some("*"));
        agree(&dir, "reused");
        assert_eq!(fs::read(snapshot(&dir)).expect("kept"), first, "nothing new, nothing rewritten");

        // New work, with retractions and supersessions: checked, then written back.
        grow(&dir, seed + 100, 60);
        agree(&dir, "after new work");
        assert_ne!(fs::read(snapshot(&dir)).expect("kept"), first, "rewritten with the new work");

        // A write through the CLI.
        let out = bracken(&dir, false, &["claim", "a claim through the CLI", "--tag", "pr", "--as", "builder"]);
        assert!(out.status.success(), "{out:?}");
        // The write left the snapshot current, so the reads after it write nothing.
        let written = fs::read(snapshot(&dir)).expect("kept");
        agree(&dir, "after a CLI write");
        assert_eq!(fs::read(snapshot(&dir)).expect("kept"), written, "a read after a write rewrote the snapshot");
    }
}

#[test]
fn a_deleted_object_or_a_damaged_snapshot_falls_back_to_a_full_check() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    ground(&dir, 7, 120);
    agree(&dir, "built");

    // A head deleted: the snapshot names an object that's gone, so the read is a full one.
    let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
    fs::remove_file(object(&dir, &head)).expect("removed");
    agree(&dir, "a head deleted");

    // An object others depend on deleted: both refuse the replica, the same way.
    let g = Store::open(&dir).expect("valid").graph().clone();
    let inner = g.topo_order()[0];
    let bytes = fs::read(object(&dir, &inner)).expect("present");
    fs::remove_file(object(&dir, &inner)).expect("removed");
    let (full, cached) = (read(&dir, true, &["log", "--json"]), read(&dir, false, &["log", "--json"]));
    assert!(!full.0, "the replica lacks an ancestor");
    assert_eq!(cached, full, "the snapshot can't vouch for a replica missing an ancestor");
    fs::write(object(&dir, &inner), bytes).expect("restored");

    // A damaged snapshot: rebuilt.
    let mut damaged = fs::read(snapshot(&dir)).expect("written");
    let mid = damaged.len() / 2;
    damaged[mid] ^= 1;
    fs::write(snapshot(&dir), &damaged).expect("written");
    agree(&dir, "a damaged snapshot");
    assert_ne!(fs::read(snapshot(&dir)).expect("rewritten"), damaged);

    // Another Ground's snapshot: rebuilt.
    let other = tmp.path().join("other");
    ground(&other, 8, 20);
    read(&other, false, &["heads", "--json"]);
    fs::copy(snapshot(&other), snapshot(&dir)).expect("copied");
    agree(&dir, "another Ground's snapshot");
}

#[test]
fn a_new_object_that_breaks_a_rule_is_refused_as_the_full_check_refuses_it() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    ground(&dir, 9, 80);
    agree(&dir, "built");
    let store = Store::open(&dir).expect("valid");
    let me = store.identity("builder").expect("an identity");
    let heads: Vec<_> = store.graph().heads().collect();
    for (name, body) in [
        ("blank", Body::Claim { statement: "\u{2003}\n".into(), tags: vec![] }),
        ("forged", Body::Claim { statement: "forged".into(), tags: vec![] }),
    ] {
        let c = manyroot::Contribution::create(store.info().ground, &me, heads.clone(), 1, body);
        let mut bytes = c.encode();
        if name == "forged" {
            *bytes.last_mut().expect("not empty") ^= 1; // the signature's last byte
        }
        let path = object(&dir, &c.id());
        fs::create_dir_all(path.parent().expect("a shard")).expect("made");
        fs::write(&path, &bytes).expect("written");
        let (full, cached) = (read(&dir, true, &["log", "--json"]), read(&dir, false, &["log", "--json"]));
        assert!(!full.0, "{name}: the full check refuses it");
        assert_eq!(cached, full, "{name}: refused the same way");
        fs::remove_file(&path).expect("removed");
    }
    agree(&dir, "valid again");
}

/// The trade the owner accepted (decision 96e282819607): an object the snapshot holds isn't read again,
/// so overwriting it goes unnoticed by a cached read. The full check and `verify` catch it.
#[test]
fn an_overwritten_cached_object_is_caught_only_by_a_full_check() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    ground(&dir, 10, 60);
    let before = read(&dir, false, &["log", "--json"]);
    let order = Store::open(&dir).expect("valid").graph().topo_order();
    let other = fs::read(object(&dir, &order[1])).expect("present");
    fs::write(object(&dir, &order[0]), other).expect("overwritten");

    assert!(!read(&dir, true, &["log", "--json"]).0, "the full check refuses the replica");
    let verify = bracken(&dir, false, &["verify"]);
    assert!(!verify.status.success(), "verify re-reads everything");
    assert_eq!(read(&dir, false, &["log", "--json"]), before, "a cached read doesn't see it");
}

#[test]
fn verify_rewrites_the_snapshot_and_a_full_check_writes_none() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    ground(&dir, 11, 40);
    read(&dir, true, &["log", "--json"]);
    assert!(!dir.join("cache").exists(), "BRACKEN_FULL_CHECK neither reads nor writes the snapshot");
    let out = bracken(&dir, true, &["verify", "--json"]);
    assert!(out.status.success());
    assert!(!dir.join("cache").exists());

    let out = bracken(&dir, false, &["verify", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["snapshot_written"], true);
    assert_eq!(v["contributions"].as_u64(), Some(Store::open(&dir).expect("valid").graph().len() as u64));
    assert!(snapshot(&dir).is_file());
}

#[cfg(unix)]
#[test]
fn writing_the_snapshot_never_follows_a_link() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    ground(&dir, 12, 30);
    let victim = tmp.path().join("victim");
    fs::write(&victim, "untouched").expect("written");

    // A link where the snapshot goes: replaced, never written through.
    fs::create_dir(dir.join("cache")).expect("made");
    std::os::unix::fs::symlink(&victim, snapshot(&dir)).expect("linked");
    agree(&dir, "a link at the snapshot");
    assert_eq!(fs::read_to_string(&victim).expect("read"), "untouched");
    assert!(fs::symlink_metadata(snapshot(&dir)).expect("written").is_file());

    // `cache` itself a link, here into objects/: nothing is written there, and reads still work.
    fs::remove_dir_all(dir.join("cache")).expect("removed");
    std::os::unix::fs::symlink(dir.join("objects"), dir.join("cache")).expect("linked");
    let shards = fs::read_dir(dir.join("objects")).expect("listed").count();
    agree(&dir, "cache a link");
    assert_eq!(fs::read_dir(dir.join("objects")).expect("listed").count(), shards, "nothing landed in objects/");
    assert!(!dir.join("objects").join("snapshot").exists());
}
