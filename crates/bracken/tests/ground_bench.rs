//! The Ground benchmark's own parts (benches/ground): the synthetic Ground is valid and the size asked
//! for, the bytes-written count sees a rename and an append for what they wrote, and a copy of a replica
//! leaves its keys and cache behind.

#[allow(dead_code)]
#[path = "../benches/ground/measure.rs"]
mod measure;
#[allow(dead_code)]
#[path = "../benches/ground/synth.rs"]
mod synth;

use std::collections::BTreeSet;
use std::fs;

use manyroot::{BodyKind, Store};

#[test]
fn the_synthetic_ground_is_valid_and_about_the_size_asked_for() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("g");
    let spec = synth::Spec { contributions: 600, bytes: 2 << 20, agents: 3, seed: 1 };
    let built = synth::build(&dir, &spec).expect("builds");
    // A full open checks every object against every validity rule.
    let store = Store::open(&dir).expect("every contribution is valid");
    assert_eq!(store.graph().len(), 600);
    assert_eq!(built.contributions, 600);
    assert_eq!(measure::tree_bytes(&dir.join("objects")).expect("objects"), built.object_bytes);
    let target = spec.bytes as f64;
    assert!((built.object_bytes as f64 - target).abs() < target * 0.1, "{} bytes", built.object_bytes);
    let kinds: BTreeSet<BodyKind> = store.graph().iter().map(|c| c.body().kind()).collect();
    assert_eq!(kinds.len(), BodyKind::ALL.len(), "every kind appears: {kinds:?}");
    assert!(store.graph().heads().count() <= 4);
    // Its identities sign for it.
    assert_eq!(store.identities().expect("identities").len(), 3);
}

#[test]
fn bytes_written_count_a_rename_whole_and_an_append_by_its_growth() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let d = tmp.path();
    fs::write(d.join("a"), [0u8; 100]).expect("write");
    fs::write(d.join("b"), [0u8; 50]).expect("write");
    let before = measure::files(d).expect("list");
    // a: replaced by a rename (a new inode); b: grown in place; c: new.
    fs::write(d.join(".a.tmp"), [0u8; 120]).expect("write");
    fs::rename(d.join(".a.tmp"), d.join("a")).expect("rename");
    let mut b = fs::OpenOptions::new().append(true).open(d.join("b")).expect("open");
    std::io::Write::write_all(&mut b, &[0u8; 30]).expect("append");
    fs::write(d.join("c"), [0u8; 7]).expect("write");
    let after = measure::files(d).expect("list");
    assert_eq!(measure::written(&before, &after), 120 + 30 + 7);
    assert_eq!(measure::written(&after, &after), 0);
}

#[test]
fn a_copy_of_a_replica_leaves_its_keys_and_cache_behind() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let from = tmp.path().join("from");
    synth::build(&from, &synth::Spec { contributions: 20, bytes: 0, agents: 2, seed: 2 }).expect("builds");
    fs::create_dir(from.join("cache")).expect("cache");
    fs::write(from.join("cache").join("snapshot"), b"x").expect("snapshot");
    let to = tmp.path().join("to");
    measure::copy_replica(&from, &to).expect("copies");
    assert!(!to.join("identities").exists());
    assert!(!to.join("cache").exists());
    assert_eq!(fs::read_dir(to.join("agents")).expect("agents").count(), 2);
    assert_eq!(Store::open(&to).expect("opens").graph().len(), 20);
}

#[test]
fn percentiles_are_nearest_rank() {
    let ms = |v: &[u64]| v.iter().map(|n| std::time::Duration::from_millis(*n)).collect::<Vec<_>>();
    let samples = ms(&[5, 1, 4, 2, 3, 10, 9, 8, 7, 6]);
    assert_eq!(measure::percentile(&samples, 50).as_millis(), 5);
    assert_eq!(measure::percentile(&samples, 95).as_millis(), 10);
    assert_eq!(measure::percentile(&ms(&[42]), 95).as_millis(), 42);
}
