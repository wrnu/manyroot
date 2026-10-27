//! Store writes stay inside the Ground (task 26a5c735230d).

#![cfg(unix)]

use std::os::unix::fs::symlink;

use manyroot::*;

/// A contribution signed for `ground`, and the shard directory its object goes in.
fn one(ground: GroundId) -> (Contribution, String) {
    let who = Identity::generate("alice").unwrap();
    let c = Contribution::create(ground, &who, [], 1, Body::Claim { statement: "hello".into(), tags: vec![] });
    let shard = c.id().to_hex()[..2].to_owned();
    (c, shard)
}

fn files_in(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

#[test]
fn a_symlinked_shard_is_refused_and_nothing_is_written_outside() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    symlink(&outside, s.dir().join("objects").join(&shard)).unwrap();

    let err = s.import([c.clone()]).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
    assert_eq!(files_in(&outside), 0, "nothing written through the symlink");
    assert!(!s.graph().contains(&c.id()));
}

#[test]
fn a_symlinked_objects_directory_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let dir = tmp.path().join("g");
    let mut s = Store::init(&dir, "g").unwrap();
    std::fs::remove_dir(dir.join("objects")).unwrap();
    symlink(&outside, dir.join("objects")).unwrap();

    let (c, _) = one(s.info().ground);
    assert!(s.import([c]).is_err());
    assert_eq!(files_in(&outside), 0);
}

#[test]
fn a_shard_that_is_a_file_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    std::fs::write(s.dir().join("objects").join(&shard), b"x").unwrap();
    assert!(s.import([c]).is_err());
}

#[test]
fn a_symlink_at_the_object_path_is_replaced_not_written_through() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    std::fs::write(&target, b"untouched").unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    let hex = c.id().to_hex();
    std::fs::create_dir(s.dir().join("objects").join(&shard)).unwrap();
    symlink(&target, s.dir().join("objects").join(&shard).join(&hex[2..])).unwrap();

    assert_eq!(s.import([c.clone()]).unwrap().added, vec![c.id()]);
    assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
    let reopened = Store::open(s.dir()).unwrap();
    assert!(reopened.graph().contains(&c.id()));
}

#[test]
fn a_missing_objects_directory_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("g");
    let info = Store::init(&dir, "g").unwrap().info().clone();
    std::fs::remove_dir(dir.join("objects")).unwrap();
    let mut s = Store::open(&dir).unwrap();
    let (c, _) = one(info.ground);
    s.import([c.clone()]).unwrap();
    assert!(Store::open(&dir).unwrap().graph().contains(&c.id()));
}
