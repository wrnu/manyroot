//! The snapshot against `Store::open`, the oracle: on generated replicas written to disk, every way of
//! opening (building the snapshot, reusing it, extending it with new objects, rebuilding a damaged one)
//! gives the same tables as a full verifying open, and an object that breaks a validity rule is refused.

use std::fs;
use std::path::Path;

use bracken_query_lab::generate;
use bracken_query_lab::ground::Ground;
use bracken_query_lab::snapshot;
use manyroot::{Body, Contribution, Identity};

fn assert_same(a: &Ground, b: &Ground) {
    assert_eq!(a.ids, b.ids, "rows");
    assert_eq!(a.fact, b.fact, "facts");
    assert_eq!(a.parents, b.parents, "parents");
    assert_eq!(a.referrers, b.referrers, "referrers");
    assert_eq!(a.timestamp_ms, b.timestamp_ms, "timestamps");
    assert_eq!(a.heads, b.heads, "heads");
    assert_eq!(a.tags, b.tags, "tags");
    assert_eq!(a.by_tag, b.by_tag, "the tag index");
    let authors = |g: &Ground| g.author.iter().map(|x| g.agents[*x as usize]).collect::<Vec<_>>();
    assert_eq!(authors(a), authors(b), "authors");
    let named = |g: &Ground| (0..g.len() as u32).map(|r| g.author_name(r)).collect::<Vec<_>>();
    assert_eq!(named(a), named(b), "author names");
}

fn replica(dir: &Path, n: usize) -> generate::Generated {
    let g = generate::ground(5, n, 5);
    generate::write(dir, &g.graph, &g.names, g.graph.topo_order()).expect("written");
    g
}

#[test]
fn every_way_of_opening_gives_store_opens_tables() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    let mut g = replica(&dir, 400);

    let (built, report) = snapshot::open(&dir, &snap).expect("opens");
    assert!(report.rebuilt.is_some(), "no snapshot yet");
    assert_same(&built, &bracken_query_lab::open(&dir).expect("opens"));

    let (reused, report) = snapshot::open(&dir, &snap).expect("opens");
    assert_eq!((report.cached, report.new, report.rebuilt), (built.len(), 0, None));
    assert_same(&reused, &built);

    // New work arrives, some concurrent with what the snapshot holds: rows must still be the protocol's
    // causal order, which can put a new contribution before old ones.
    let late: Vec<Identity> = (0..2).map(|i| Identity::generate(&format!("late-{i}")).expect("a key")).collect();
    let added = generate::grow(&mut g.graph, &late, 9, 60);
    let names: Vec<_> = late.iter().map(|i| (i.name().to_owned(), i.agent_id())).collect();
    generate::write(&dir, &g.graph, &names, added.iter().copied()).expect("written");
    let (extended, report) = snapshot::open(&dir, &snap).expect("opens");
    assert_eq!((report.cached, report.new, report.rebuilt), (built.len(), added.len(), None));
    assert_same(&extended, &bracken_query_lab::open(&dir).expect("opens"));
    let (again, report) = snapshot::open(&dir, &snap).expect("opens");
    assert_eq!(report.new, 0, "the extended snapshot was written back");
    assert_same(&again, &extended);
}

#[test]
fn a_damaged_or_foreign_snapshot_is_rebuilt() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    replica(&dir, 200);
    let (good, _) = snapshot::open(&dir, &snap).expect("opens");
    let mut bytes = fs::read(&snap).expect("written");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 1;
    fs::write(&snap, &bytes).expect("written");
    let (g, report) = snapshot::open(&dir, &snap).expect("opens");
    assert!(report.rebuilt.as_deref().is_some_and(|w| w.contains("checksum")), "{report:?}");
    assert_same(&g, &good);

    // Another replica's snapshot.
    let other = tmp.path().join("other");
    let o = generate::ground(6, 50, 3);
    generate::write(&other, &o.graph, &o.names, o.graph.topo_order()).expect("written");
    let (_, report) = snapshot::open(&other, &snap).expect("opens");
    assert!(report.rebuilt.is_some(), "a snapshot of another Ground isn't used");
    let (g, report) = snapshot::open(&dir, &snap).expect("opens");
    assert!(report.rebuilt.is_some());
    assert_same(&g, &good);
}

#[test]
fn a_snapshot_listing_what_the_replica_lacks_is_rebuilt() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    let g = replica(&dir, 200);
    snapshot::open(&dir, &snap).expect("opens");
    // A copy of the replica holding less, opened with the fuller snapshot.
    let fewer = tmp.path().join("fewer");
    let order = g.graph.topo_order();
    generate::write(&fewer, &g.graph, &g.names, order[..100].iter().copied()).expect("written");
    let (opened, report) = snapshot::open(&fewer, &snap).expect("opens");
    assert!(report.rebuilt.as_deref().is_some_and(|w| w.contains("lacks")), "{report:?}");
    assert_eq!(opened.len(), 100);
}

fn object(dir: &Path, c: &Contribution) -> std::path::PathBuf {
    let hex = c.id().to_hex();
    let shard = dir.join("objects").join(&hex[..2]);
    fs::create_dir_all(&shard).expect("a shard");
    shard.join(&hex[2..])
}

#[test]
fn new_objects_that_break_a_rule_are_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    let g = replica(&dir, 200);
    snapshot::open(&dir, &snap).expect("opens");
    let heads: Vec<_> = g.graph.heads().collect();

    // A bad signature: the last byte of the encoding is the signature's.
    let c = Contribution::create(
        g.graph.ground(),
        &g.identities[1],
        heads.clone(),
        1,
        Body::Claim { statement: "forged".into(), tags: vec![] },
    );
    let mut bytes = c.encode();
    *bytes.last_mut().expect("not empty") ^= 1;
    let path = object(&dir, &c);
    fs::write(&path, &bytes).expect("written");
    let err = snapshot::open(&dir, &snap).err().expect("refused").to_string();
    assert!(err.contains("signature"), "{err}");
    assert!(manyroot::Store::open(&dir).is_err(), "Store::open refuses it too");
    fs::remove_file(&path).expect("removed");

    // A retraction of another author's contribution, correctly signed.
    let theirs = g.graph.iter().find(|c| c.author() != g.identities[1].agent_id()).expect("someone else's").id();
    let r = Contribution::create(
        g.graph.ground(),
        &g.identities[1],
        heads.clone(),
        2,
        Body::Retract { target: theirs, reason: "not mine".into() },
    );
    let path = object(&dir, &r);
    fs::write(&path, r.encode()).expect("written");
    let err = snapshot::open(&dir, &snap).err().expect("refused").to_string();
    assert!(err.contains("another author"), "{err}");
    assert!(manyroot::Store::open(&dir).is_err(), "Store::open refuses it too");
    fs::remove_file(&path).expect("removed");

    // Rule 1: a claim of protocol 2, re-signed by its author, so only the version is wrong. The version is
    // the encoding's `"protocol"` key and a one-byte integer; the signature is its last 64 bytes.
    let c = Contribution::create(
        g.graph.ground(),
        &g.identities[1],
        heads.clone(),
        3,
        Body::Claim { statement: "from the future".into(), tags: vec![] },
    );
    let mut bytes = c.encode();
    let key = b"\x68protocol\x01";
    let at = bytes.windows(key.len()).position(|w| w == key).expect("the version is encoded once");
    bytes[at + key.len() - 1] = 2;
    let unsigned = Contribution::decode(&bytes).expect("still canonical");
    assert_eq!(unsigned.content().protocol, 2);
    let n = bytes.len();
    bytes[n - 64..].copy_from_slice(g.identities[1].sign(&unsigned.id()).as_bytes());
    let v2 = Contribution::decode(&bytes).expect("decodes");
    assert!(v2.signature_is_valid(), "re-signed");
    let path = object(&dir, &v2);
    fs::write(&path, &bytes).expect("written");
    let err = snapshot::open(&dir, &snap).err().expect("refused").to_string();
    assert!(err.contains("protocol 2"), "{err}");
    assert!(manyroot::Store::open(&dir).is_err(), "Store::open refuses it too");
    fs::remove_file(&path).expect("removed");

    // Rule 8: a claim whose statement is only Unicode whitespace (an em space, an ideographic space, a
    // no-break space and a line break), none of it ASCII but the last.
    let w = Contribution::create(
        g.graph.ground(),
        &g.identities[1],
        heads,
        4,
        Body::Claim { statement: "\u{2003}\u{3000}\u{a0}\n".into(), tags: vec![] },
    );
    let path = object(&dir, &w);
    fs::write(&path, w.encode()).expect("written");
    let err = snapshot::open(&dir, &snap).err().expect("refused").to_string();
    assert!(err.contains("blank"), "{err}");
    assert!(manyroot::Store::open(&dir).is_err(), "Store::open refuses it too");
    fs::remove_file(&path).expect("removed");

    let (_, report) = snapshot::open(&dir, &snap).expect("the replica is valid again");
    assert_eq!((report.new, report.rebuilt), (0, None));
}

#[test]
fn a_snapshot_inside_a_ground_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    replica(&dir, 50);
    for inside in [dir.join("snap"), dir.join("objects").join("snap")] {
        let err = snapshot::open(&dir, &inside).err().expect("refused").to_string();
        assert!(err.contains("inside the Ground"), "{err}");
        assert!(!inside.exists());
    }
    let bracken = tmp.path().join(".bracken");
    fs::create_dir(&bracken).expect("made");
    let err = snapshot::open(&dir, &bracken.join("snap")).err().expect("refused").to_string();
    assert!(err.contains("inside the Ground"), "{err}");
}

#[cfg(unix)]
#[test]
fn writing_a_snapshot_never_follows_a_link() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    replica(&dir, 50);
    // A link where the temporary file goes, to a file the snapshot must not overwrite.
    let victim = tmp.path().join("victim");
    fs::write(&victim, "untouched").expect("written");
    std::os::unix::fs::symlink(&victim, snap.with_extension("tmp")).expect("linked");
    snapshot::open(&dir, &snap).expect("opens");
    assert_eq!(fs::read_to_string(&victim).expect("read"), "untouched");
    assert!(fs::symlink_metadata(&snap).expect("written").is_file());
    assert!(!snap.with_extension("tmp").exists(), "renamed into place");
}

#[test]
fn only_a_generated_ground_can_be_extended() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    replica(&dir, 50);
    generate::ensure_synthetic(&dir).expect("one this tool wrote");
    fs::create_dir(dir.join("identities")).expect("made");
    assert!(generate::ensure_synthetic(&dir).is_err(), "keys on disk: a real Ground");
    fs::remove_dir(dir.join("identities")).expect("removed");
    fs::write(dir.join("query-lab-synthetic"), "00\n").expect("written");
    assert!(generate::ensure_synthetic(&dir).is_err(), "the marker names another Ground");
    fs::remove_file(dir.join("query-lab-synthetic")).expect("removed");
    assert!(generate::ensure_synthetic(&dir).is_err(), "no marker");
}

#[test]
fn scale_removes_nothing_inside_a_ground() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path().join("g");
    replica(&dir, 50);
    // `scale` removes its snapshot before each timed build: inside a Ground, that would be the Ground's.
    let inside = dir.join("ground.json");
    let before = fs::read(&inside).expect("written");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_query-lab"))
        .args([
            "scale".as_ref(),
            dir.as_os_str(),
            "--snapshot".as_ref(),
            inside.as_os_str(),
            "--runs".as_ref(),
            "1".as_ref(),
        ])
        .output()
        .expect("runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the Ground"), "{out:?}");
    assert_eq!(fs::read(&inside).expect("still there"), before);
}

#[test]
fn an_object_named_in_uppercase_is_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let (dir, snap) = (tmp.path().join("g"), tmp.path().join("snap"));
    let g = replica(&dir, 50);
    snapshot::open(&dir, &snap).expect("opens");
    let c = Contribution::create(
        g.graph.ground(),
        &g.identities[1],
        g.graph.heads().collect::<Vec<_>>(),
        1,
        Body::Claim { statement: "shouting".into(), tags: vec![] },
    );
    let lower = object(&dir, &c);
    let upper = lower.with_file_name(lower.file_name().expect("named").to_string_lossy().to_uppercase());
    assert_ne!(lower, upper, "the id has a letter in its name");
    fs::write(&upper, c.encode()).expect("written");
    let err = snapshot::open(&dir, &snap).err().expect("refused").to_string();
    assert!(err.contains("lowercase"), "{err}");
    assert!(manyroot::Store::open(&dir).is_err(), "Store::open refuses it too");
}
