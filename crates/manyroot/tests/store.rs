//! Store writes stay inside the Ground (task 26a5c735230d).

#![cfg(unix)]

use std::os::unix::fs::symlink;

use manyroot::identity::IdentityError;
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

// Task c0eb61747345: the agent directory, identity files, writes and the open paths fail closed.

fn write_entry(dir: &std::path::Path, file: &str, name: &str, agent: AgentId) {
    let entry = serde_json::json!({ "name": name, "agent": agent });
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::write(dir.join("agents").join(file), entry.to_string()).unwrap();
}

fn corrupt(result: Result<Vec<AgentEntry>, StoreError>, want: &str) {
    match result {
        Err(StoreError::Corrupt { reason, .. }) => assert!(reason.contains(want), "{reason:?} doesn't say {want:?}"),
        other => panic!("expected Corrupt ({want}), got {other:?}"),
    }
}

/// A replica holding one contribution and `manager`'s directory entry, but not its key: a clone.
fn clone_without_manager_key(tmp: &std::path::Path) -> (Store, AgentId) {
    let origin = Store::init(&tmp.join("origin"), "g").unwrap();
    let manager = origin.create_identity("manager").unwrap().agent_id();
    let dir = tmp.join("clone");
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::copy(origin.dir().join("ground.json"), dir.join("ground.json")).unwrap();
    std::fs::copy(origin.agents_dir().join("manager.json"), dir.join("agents/manager.json")).unwrap();
    (Store::open(&dir).unwrap(), manager)
}

#[test]
fn an_entry_naming_another_agent_cannot_take_its_name() {
    // Neon's first counterexample: an entry named manager with another key, on a replica without
    // manager's identity. A second manager.json can't exist, so it sits under another file name.
    let tmp = tempfile::tempdir().unwrap();
    let (s, manager) = clone_without_manager_key(tmp.path());
    let other = Identity::generate("other").unwrap().agent_id();
    assert_eq!(s.agents().unwrap(), vec![AgentEntry { name: "manager".into(), agent: manager }]);
    write_entry(s.dir(), "zzz.json", "manager", other);
    corrupt(s.agents(), "names \"manager\", not \"zzz\"");

    // Without the real entry the impostor still can't name itself manager.
    std::fs::remove_file(s.agents_dir().join("manager.json")).unwrap();
    corrupt(s.agents(), "not \"zzz\"");
    assert!(Store::open(s.dir()).is_ok(), "opening doesn't read agents/");
}

#[test]
fn an_entry_cannot_rename_a_listed_key() {
    // Neon's second counterexample: {name: zzz, agent: <manager's key>}.
    let tmp = tempfile::tempdir().unwrap();
    let (s, manager) = clone_without_manager_key(tmp.path());
    write_entry(s.dir(), "zzz.json", "zzz", manager);
    corrupt(s.agents(), "under another name");
}

#[test]
fn a_local_key_cannot_be_listed_under_another_name() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let alice = s.create_identity("alice").unwrap().agent_id();
    std::fs::remove_file(s.agents_dir().join("alice.json")).unwrap();
    s.agents().expect("a local identity with no entry is fine");
    write_entry(s.dir(), "zzz.json", "zzz", alice);
    corrupt(s.agents(), "local identity \"alice\"'s key under another name");
}

#[test]
fn agents_refuses_entries_not_named_for_a_valid_name() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let key = Identity::generate("x").unwrap().agent_id();

    write_entry(s.dir(), "Bad.json", "Bad", key);
    corrupt(s.agents(), "for a valid name");
    std::fs::remove_file(s.agents_dir().join("Bad.json")).unwrap();

    std::fs::write(s.agents_dir().join("README"), "stray").unwrap();
    corrupt(s.agents(), "for a valid name");
    std::fs::remove_file(s.agents_dir().join("README")).unwrap();

    // A dot name is another process's write in flight.
    std::fs::write(s.agents_dir().join(".tmp-1-new-00"), "partial").unwrap();
    assert_eq!(s.agents().unwrap(), vec![]);
}

#[test]
fn agents_refuses_entries_that_are_not_regular_files() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let key = Identity::generate("bob").unwrap().agent_id();
    let elsewhere = tmp.path().join("bob.json");
    std::fs::write(&elsewhere, serde_json::json!({ "name": "bob", "agent": key }).to_string()).unwrap();
    std::fs::create_dir_all(s.agents_dir()).unwrap();
    symlink(&elsewhere, s.agents_dir().join("bob.json")).unwrap();
    corrupt(s.agents(), "not a regular file");
    std::fs::remove_file(s.agents_dir().join("bob.json")).unwrap();
    std::fs::create_dir(s.agents_dir().join("bob.json")).unwrap();
    corrupt(s.agents(), "not a regular file");
}

#[test]
fn an_identity_file_must_be_a_regular_file_named_for_its_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    s.create_identity("alice").unwrap();
    let ids = s.identities_dir();

    // alice's key under bob's file name: neither bob nor the listing may load it.
    std::fs::copy(ids.join("alice.json"), ids.join("bob.json")).unwrap();
    assert!(matches!(s.identity("bob"), Err(StoreError::Identity(IdentityError::Malformed { .. }))));
    assert!(s.identities().is_err());
    assert!(s.agents().is_err());
    std::fs::remove_file(ids.join("bob.json")).unwrap();

    // A symlink to a real identity file elsewhere.
    let elsewhere = tmp.path().join("carol");
    std::fs::create_dir(&elsewhere).unwrap();
    Identity::generate("carol").unwrap().save(&elsewhere.join("carol.json")).unwrap();
    symlink(elsewhere.join("carol.json"), ids.join("carol.json")).unwrap();
    assert!(Identity::load(&ids.join("carol.json")).is_err());
    assert!(s.identity("carol").is_err());
    assert!(s.identities().is_err());
    std::fs::remove_file(ids.join("carol.json")).unwrap();

    // A name the rules refuse, in a file named for it.
    let file = std::fs::read_to_string(ids.join("alice.json")).unwrap().replace("\"alice\"", "\"Al ice\"");
    std::fs::write(ids.join("Al ice.json"), file).unwrap();
    assert!(matches!(Identity::load(&ids.join("Al ice.json")), Err(IdentityError::InvalidName(_))));
    assert!(s.identities().is_err());
    std::fs::remove_file(ids.join("Al ice.json")).unwrap();
    assert_eq!(s.identities().unwrap().len(), 1);
}

#[test]
fn a_file_at_the_old_temporary_name_does_not_block_a_write() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    let shard = s.dir().join("objects").join(shard);
    std::fs::create_dir(&shard).unwrap();
    // The name write_object used before: predictable from the pid and the id alone.
    std::fs::write(shard.join(format!(".tmp-{}-{}", std::process::id(), c.id().short())), b"planted").unwrap();
    assert_eq!(s.import([c.clone()]).unwrap().added, vec![c.id()]);
}

#[test]
fn a_failed_object_write_leaves_no_temporary_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    let shard = s.dir().join("objects").join(shard);
    // A non-empty directory at the object's path: the rename fails.
    let hex = c.id().to_hex();
    std::fs::create_dir_all(shard.join(&hex[2..]).join("x")).unwrap();
    assert!(s.import([c]).is_err());
    let names: Vec<_> = std::fs::read_dir(&shard).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, vec![std::ffi::OsString::from(&hex[2..])], "only what was there before");
}

#[test]
fn create_does_not_write_ground_json_through_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("g");
    std::fs::create_dir(&dir).unwrap();
    let target = tmp.path().join("target");
    symlink(&target, dir.join("ground.json")).unwrap();
    assert!(matches!(Store::init(&dir, "g"), Err(StoreError::AlreadyExists(_))));
    assert!(!target.exists(), "nothing written through the dangling link");
    let temporary = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name() != "ground.json");
    assert!(temporary.map(|e| e.unwrap().file_name()).all(|n| n == "objects" || n == "identities"));
}

#[test]
fn create_identity_does_not_write_its_entry_through_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir_all(s.agents_dir()).unwrap();
    symlink(&target, s.agents_dir().join("bob.json")).unwrap();
    assert!(matches!(s.create_identity("bob"), Err(StoreError::NameTaken(_))));
    assert!(!target.exists(), "nothing written through the dangling link");
    assert!(!s.identities_dir().join("bob.json").exists(), "the new key isn't left behind");
    let left: Vec<_> = std::fs::read_dir(s.agents_dir()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, vec![std::ffi::OsString::from("bob.json")], "no temporary file left");
}

/// A Ground with one contribution, its object moved out to `elsewhere/<shard>/` and linked back by `link`.
fn moved_out(tmp: &std::path::Path, link: impl Fn(&std::path::Path, &std::path::Path, &str)) -> (Store, Contribution) {
    let mut s = Store::init(&tmp.join("g"), "g").unwrap();
    let (c, shard) = one(s.info().ground);
    s.import([c.clone()]).unwrap();
    let elsewhere = tmp.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::rename(s.dir().join("objects").join(&shard), elsewhere.join(&shard)).unwrap();
    link(&s.dir().join("objects"), &elsewhere, &shard);
    (s, c)
}

fn open_corrupt(result: Result<Store, StoreError>, want: &str) {
    match result {
        Err(StoreError::Corrupt { reason, .. }) => assert!(reason.contains(want), "{reason:?} doesn't say {want:?}"),
        Err(other) => panic!("expected Corrupt ({want}), got {other:?}"),
        Ok(_) => panic!("expected Corrupt ({want}), got a store"),
    }
}

#[test]
fn open_refuses_a_symlinked_shard() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _) = moved_out(tmp.path(), |objects, elsewhere, shard| {
        symlink(elsewhere.join(shard), objects.join(shard)).unwrap();
    });
    open_corrupt(Store::open(s.dir()), "not a shard directory");
}

#[test]
fn open_refuses_a_symlinked_object_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, c) = moved_out(tmp.path(), |objects, elsewhere, shard| {
        std::fs::create_dir(objects.join(shard)).unwrap();
        for entry in std::fs::read_dir(elsewhere.join(shard)).unwrap() {
            let entry = entry.unwrap();
            symlink(entry.path(), objects.join(shard).join(entry.file_name())).unwrap();
        }
    });
    open_corrupt(Store::open(s.dir()), "not a regular file");

    // The uncached tail of a trusting open reads an untrusted object the same way.
    let mut s2 = Store::init(&tmp.path().join("g2"), "g").unwrap();
    let who = Identity::generate("bob").unwrap();
    let claim = |statement: &str| Body::Claim { statement: statement.into(), tags: vec![] };
    let trusted = Contribution::create(s2.info().ground, &who, [], 2, claim("trusted"));
    s2.import([trusted.clone()]).unwrap();
    let linked = Contribution::create(s2.info().ground, &who, [], 3, claim("linked"));
    let outside = tmp.path().join("outside-object");
    std::fs::write(&outside, linked.encode()).unwrap();
    let path = object(s2.dir(), &linked.id());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&outside, &path).unwrap();
    open_corrupt(Store::open_trusting(s2.dir(), vec![trusted]), "not a regular file");
    open_corrupt(Store::open(s2.dir()), "not a regular file");
    let _ = c;
}

#[test]
fn open_refuses_a_stray_file_in_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let (c, _) = one(s.info().ground);
    s.import([c.clone()]).unwrap();
    std::fs::write(s.dir().join("objects").join(".DS_Store"), b"dot names are skipped").unwrap();
    Store::open(s.dir()).unwrap();
    std::fs::write(s.dir().join("objects").join("stray"), b"x").unwrap();
    open_corrupt(Store::open(s.dir()), "not a shard directory");
    open_corrupt(Store::open_trusting(s.dir(), vec![c]), "not a shard directory");
}

#[test]
fn the_encoded_length_is_the_encoding_s_without_encoding_again() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let who = s.create_identity("alice").unwrap();
    let body = Body::Artifact { name: "a".into(), media_type: "text/plain".into(), content: vec![7; 70_000] };
    let c = Contribution::create(s.info().ground, &who, [], 1, body);
    assert_eq!(c.encoded_len(), c.encode().len());
    let decoded = Contribution::decode(&c.encode()).unwrap();
    assert_eq!(decoded.encoded_len(), c.encode().len());
    assert_eq!(decoded, c);
    assert_eq!(decoded.encode(), c.encode());
    s.import([c.clone()]).unwrap();
    let reopened = Store::open(s.dir()).unwrap();
    let read = reopened.graph().get(&c.id()).unwrap();
    assert_eq!(read.encoded_len(), std::fs::read(object(s.dir(), &c.id())).unwrap().len());
}

fn object(dir: &std::path::Path, id: &ContributionId) -> std::path::PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

#[test]
fn the_directory_alone_never_reads_identities_but_checks_agents_as_agents_does() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::init(&tmp.path().join("g"), "g").unwrap();
    let alice = s.create_identity("alice").unwrap().agent_id();
    // A broken identity file: agents() reads identities/ and fails; directory() doesn't read it.
    std::fs::write(s.identities_dir().join("bob.json"), "not an identity").unwrap();
    assert!(s.agents().is_err());
    assert_eq!(s.directory().unwrap(), vec![AgentEntry { name: "alice".into(), agent: alice }]);
    write_entry(s.dir(), "zzz.json", "zzz", alice);
    corrupt(s.directory(), "under another name");
}
