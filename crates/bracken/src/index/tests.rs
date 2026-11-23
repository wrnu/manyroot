//! The index against the full recompute, one test per row of docs/design/index-store.md section 8's
//! table that this step can test (the rest need reads served from the index, PR 3).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use manyroot::store::GroundInfo;
use manyroot::{Body, Contribution, ContributionId, Identity, RelationKind, Store, Verdict};

use super::*;

thread_local! {
    /// What [`durable`] synced on this thread.
    pub(super) static SYNCED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

const PEOPLE: [&str; 3] = ["manager", "builder", "reviewer"];

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }
}

fn ground(tmp: &Path) -> PathBuf {
    let dir = tmp.join("g");
    let store = Store::init(&dir, "index").expect("a Ground");
    for p in PEOPLE {
        store.create_identity(p).expect("an identity");
    }
    dir
}

/// Adds about `n` contributions through `Store` alone, as an older binary or a git pull would: claims
/// with repeated tags, evidence, relations (supersessions among them) and retractions of one's own work.
fn grow(dir: &Path, seed: u64, n: usize) {
    let mut store = Store::open(dir).expect("valid");
    grow_store(&mut store, seed, n);
}

fn grow_store(store: &mut Store, seed: u64, n: usize) {
    let ids: Vec<Identity> = PEOPLE.iter().map(|p| store.identity(p).expect("an identity")).collect();
    let mut rng = Rng(seed);
    for i in 0..n {
        let who = rng.below(ids.len());
        let all: Vec<ContributionId> = store.graph().topo_order();
        let pick = |rng: &mut Rng| all[rng.below(all.len())];
        let tags = ["pr", "finding", "task", "decision", "note"];
        let body = match (all.len(), rng.below(12)) {
            (0..=2, _) | (_, 0..=3) => Body::Claim {
                statement: format!("claim {seed}-{i}"),
                tags: vec![tags[rng.below(tags.len())].into(), tags[rng.below(tags.len())].into()],
            },
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
                let kinds = [RelationKind::Supersedes, RelationKind::Implements, RelationKind::Contradicts];
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

/// [`Before::take`], trusting any local filesystem type: the test boxes' filesystems aren't all on the
/// allow-list, and these tests are about the logic, not the list ([`allow_list`] covers that).
fn take(dir: &Path) -> Before {
    let mut before = Before::take(dir);
    if before.clock.is_none()
        && let Some(cache) = crate::snapshot::cache::Cache::open(dir, true).expect("cache/")
    {
        before.clock = cache.clock().ok().map(|(t, _)| t);
    }
    before
}

fn opts(racy: i128) -> Options {
    Options { racy_ns: racy, busy: BUSY }
}

/// Opens as a read does (clock and stamps first), refreshes with racy window `racy`, and checks the
/// replica's index against the full recompute.
fn read(dir: &Path, racy: i128) -> Outcome {
    let before = take(dir);
    let store = Store::open(dir).expect("valid");
    let outcome = refresh_with(dir, &store, &before, Record::Yes, opts(racy)).expect("refreshed");
    agrees(dir, &store);
    outcome
}

fn agrees(dir: &Path, store: &Store) {
    let conn = conn(dir);
    differs("the index", &expected(store), &actual(&conn).expect("reads")).expect("agrees");
}

fn conn(dir: &Path) -> Connection {
    open_db(&path(dir).expect("a path"), BUSY).expect("opens")
}

fn rows(dir: &Path, sql: &str) -> i64 {
    conn(dir).query_row(sql, [], |r| r.get(0)).expect("a count")
}

fn shard_of(id: &ContributionId) -> u8 {
    id.as_bytes()[0]
}

#[test]
fn built_on_first_open_then_only_dirty_shards_are_diffed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 1, 120);
    let first = read(&dir, 0);
    assert!(first.rebuilt, "no index yet, so it's built: {first:?}");
    assert_eq!(first.recorded, 256, "every shard, present or absent, recorded");
    let again = read(&dir, 0);
    assert_eq!(again, Outcome::default(), "nothing changed, so no shard is dirty");
}

#[test]
fn an_object_written_without_the_index_is_found_by_freshness() {
    // An older binary, another tool, serve's imports or a git pull: objects appear with no index update.
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 2, 80);
    read(&dir, 0);
    grow(&dir, 3, 1);
    let after = read(&dir, 0);
    assert!(!after.rebuilt && after.added == 1 && after.dirty_shards >= 1, "{after:?}");
    grow(&dir, 4, 40);
    let more = read(&dir, 0);
    assert!(!more.rebuilt && more.added >= 30, "{more:?}");
}

#[test]
fn objects_found_by_freshness_are_synced_before_the_commit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 5, 20);
    read(&dir, 0);
    let before_ids: HashSet<ContributionId> =
        Store::open(&dir).expect("valid").graph().iter().map(|c| c.id()).collect();
    grow(&dir, 6, 3);
    let new: Vec<ContributionId> = Store::open(&dir)
        .expect("valid")
        .graph()
        .iter()
        .map(|c| c.id())
        .filter(|id| !before_ids.contains(id))
        .collect();
    SYNCED.with(|s| s.borrow_mut().clear());
    read(&dir, 0);
    let synced = SYNCED.with(|s| s.borrow().clone());
    for id in &new {
        assert!(synced.contains(&object_path(&dir, id)), "{id} synced");
        assert!(synced.contains(&dir.join("objects").join(format!("{:02x}", shard_of(id)))), "its shard synced");
    }
}

#[test]
fn an_indexed_object_that_is_gone_means_rebuild() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 7, 40);
    read(&dir, 0);
    let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
    std::fs::remove_file(object_path(&dir, &head)).expect("removed");
    assert!(read(&dir, 0).rebuilt, "a checkout that went back");
}

#[test]
fn a_shard_removed_whole_while_unrecorded_means_rebuild() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 8, 10);
    read(&dir, 0);
    // A head alone in its shard, written and indexed with its shard left unrecorded (inside the window).
    let mut found = None;
    for seed in 9..60 {
        grow(&dir, seed, 1);
        read(&dir, RACY_NS);
        let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
        let shard = dir.join("objects").join(format!("{:02x}", shard_of(&head)));
        if std::fs::read_dir(&shard).expect("listed").count() == 1 {
            found = Some((head, shard));
            break;
        }
    }
    let (head, shard) = found.expect("a head alone in its shard");
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM shard WHERE n = {}", shard_of(&head))), 0, "unrecorded");
    std::fs::remove_file(object_path(&dir, &head)).expect("removed");
    std::fs::remove_dir(&shard).expect("removed");
    assert!(read(&dir, 0).rebuilt, "an absent, unrecorded shard is still diffed");
}

#[test]
fn a_recent_stamp_is_not_recorded_so_its_shard_stays_dirty() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 10, 30);
    read(&dir, 0);
    // A shard written just now: its stamp is within the margin of T, so it isn't recorded.
    grow(&dir, 61, 1);
    let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
    let n = shard_of(&head);
    let before = take(&dir);
    let t = before.clock.expect("a clock");
    let Seen::Present(Some(s)) = before.shards[usize::from(n)] else { panic!("the head's shard is there") };
    assert!(s.mtime() > t - RACY_NS, "written within the margin");
    let store = Store::open(&dir).expect("valid");
    let outcome = refresh_with(&dir, &store, &before, Record::Yes, opts(RACY_NS)).expect("refreshed");
    assert!(outcome.added == 1, "{outcome:?}");
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM shard WHERE n = {n}")), 0, "not recorded");
    let again = read(&dir, RACY_NS);
    assert!(again.dirty_shards >= 1 && again.added == 0 && !again.rebuilt, "it stays dirty: {again:?}");
}

#[test]
fn stamps_taken_before_the_listing_catch_a_write_between_the_listing_and_the_stamps() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 11, 30);
    read(&dir, 0);
    // The design's order: T and the stats, then the store's listing; a write lands after both.
    let before = take(&dir);
    let store = Store::open(&dir).expect("valid");
    grow(&dir, 12, 1);
    refresh_with(&dir, &store, &before, Record::Yes, opts(0)).expect("refreshed");
    assert_eq!(read(&dir, 0).added, 1, "the pre-write stamp doesn't match, so the next open finds it");

    // The wrong order, stats after the listing, records the post-write stamp and misses the write.
    let store = Store::open(&dir).expect("valid");
    grow(&dir, 13, 1);
    let late = take(&dir);
    refresh_with(&dir, &store, &late, Record::Yes, opts(0)).expect("refreshed");
    let missed = read_without_check(&dir);
    assert_eq!(missed.added, 0, "stats after the listing miss it: {missed:?}");
}

fn read_without_check(dir: &Path) -> Outcome {
    let before = take(dir);
    let store = Store::open(dir).expect("valid");
    refresh_with(dir, &store, &before, Record::Yes, opts(0)).expect("refreshed")
}

#[test]
fn a_clock_that_stepped_back_makes_every_shard_dirty_on_that_open() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 14, 30);
    read(&dir, 0);
    put_meta(&conn(&dir), "fs_clock", &clock_bytes(i128::MAX / 2)).expect("set");
    let outcome = read(&dir, 0);
    assert_eq!(outcome.dirty_shards, 256, "{outcome:?}");
    assert_eq!(read(&dir, 0).dirty_shards, 0, "the stepped-back clock is recorded, and the next open is clean");
}

#[test]
fn a_clock_that_cant_be_written_records_no_stamps() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 15, 30);
    std::fs::create_dir_all(dir.join("cache").join("clock")).expect("a directory where cache/clock goes");
    let before = Before::take(&dir);
    assert!(before.clock.is_none());
    let store = Store::open(&dir).expect("valid");
    let outcome = refresh_with(&dir, &store, &before, Record::Yes, opts(0)).expect("refreshed");
    assert_eq!(outcome.recorded, 0);
    assert_eq!(rows(&dir, "SELECT count(*) FROM shard"), 0);
    agrees(&dir, &store);
}

#[test]
fn allow_list() {
    for ok in ["apfs", "hfs", "ext4", "xfs", "btrfs", "zfs", "tmpfs"] {
        assert!(allowed_filesystem(ok), "{ok}");
    }
    for no in ["nfs", "smbfs", "msdos", "exfat", "vfat", "fuse", "overlay", "9p", "virtiofs", ""] {
        assert!(!allowed_filesystem(no), "{no}");
    }
}

#[cfg(unix)]
#[test]
fn a_shard_whose_mtime_was_put_back_is_still_dirty() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 16, 40);
    read(&dir, 0);
    let before = take(&dir);
    grow(&dir, 17, 1);
    let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
    let n = shard_of(&head);
    if let Seen::Present(Some(old)) = before.shards[usize::from(n)] {
        let shard = dir.join("objects").join(format!("{n:02x}"));
        let t = rustix::fs::Timespec { tv_sec: old.mtime_s, tv_nsec: old.mtime_ns as _ };
        let times = rustix::fs::Timestamps { last_access: t, last_modification: t };
        rustix::fs::utimensat(rustix::fs::CWD, &shard, &times, rustix::fs::AtFlags::empty()).expect("mtime put back");
    }
    assert_eq!(read(&dir, 0).added, 1, "the ctime still differs, so the shard is diffed");
}

#[test]
fn a_write_through_refreshing_is_indexed_and_leaves_its_shard_without_a_row() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 18, 30);
    read(&dir, 0);
    let written = {
        let mut s = crate::snapshot::Refreshing::open(&dir, crate::snapshot::Refresh::Yes).expect("opens");
        let who = s.identity("builder").expect("an identity");
        s.contribute(&who, Body::Claim { statement: "kept".into(), tags: vec!["decision".into()] }).expect("ok")
    };
    let hex = written.to_hex().to_uppercase();
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM obj WHERE hex(id) = '{hex}'")), 1, "indexed by the write");
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM shard WHERE n = {}", shard_of(&written))), 0);
    assert_eq!(read(&dir, 0).added, 0);
}

#[test]
fn a_crash_between_the_object_write_and_the_index_commit_is_repaired_on_the_next_open() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 19, 40);
    read(&dir, 0);
    // A write command whose process dies after the object write: Refreshing's drop never runs.
    let written = {
        let mut s = crate::snapshot::Refreshing::open(&dir, crate::snapshot::Refresh::Yes).expect("opens");
        let who = s.identity("builder").expect("an identity");
        let id = s.contribute(&who, Body::Claim { statement: "lost".into(), tags: vec!["note".into()] }).expect("ok");
        std::mem::forget(s);
        id
    };
    let after = read(&dir, 0);
    assert!(after.added == 1 && !after.rebuilt, "{after:?}");
    let hex = written.to_hex().to_uppercase();
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM obj WHERE hex(id) = '{hex}'")), 1);
}

#[test]
fn a_rebuild_that_never_commits_leaves_the_old_index() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 20, 30);
    read(&dir, 0);
    let count = rows(&dir, "SELECT count(*) FROM obj");
    grow(&dir, 21, 10);
    {
        let mut c = conn(&dir);
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).expect("locked");
        let store = Store::open(&dir).expect("valid");
        rebuild_in(&tx, &dir, &path(&dir).expect("path"), &store, &take(&dir), Record::Yes, opts(0)).expect("built");
        // The process dies here: the transaction is never committed.
    }
    assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), count, "the old index, whole");
    read(&dir, 0);
}

#[test]
fn concurrent_refreshes_agree_and_add_each_object_once() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 22, 60);
    read(&dir, 0);
    grow(&dir, 23, 60);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let before = take(&dir);
                let store = Store::open(&dir).expect("valid");
                refresh_with(&dir, &store, &before, Record::Yes, opts(0)).expect("refreshed")
            })
        })
        .collect();
    let added: usize = handles.into_iter().map(|h| h.join().expect("joined").added).sum();
    let expected_new = Store::open(&dir).expect("valid").graph().len() as i64 - rows(&dir, "SELECT count(*) FROM obj");
    assert_eq!(expected_new, 0);
    assert!(added >= 40, "each object added once, by whichever got there first: {added}");
    assert_eq!(read(&dir, 0), Outcome::default());
}

#[test]
fn a_rebuild_under_an_open_reader_leaves_both_answering_right() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 24, 40);
    read(&dir, 0);
    let mut reader = conn(&dir);
    let rtx = reader.transaction().expect("a read transaction");
    let before_count: i64 = rtx.query_row("SELECT count(*) FROM obj", [], |r| r.get(0)).expect("count");
    grow(&dir, 25, 10);
    put_meta(&conn(&dir), "ground", &[9u8; 32]).expect("a ground mismatch");
    let outcome = read(&dir, 0);
    assert!(outcome.rebuilt, "{outcome:?}");
    let still: i64 = rtx.query_row("SELECT count(*) FROM obj", [], |r| r.get(0)).expect("count");
    assert_eq!(still, before_count, "the reader's snapshot is whole");
    drop(rtx);
    agrees(&dir, &Store::open(&dir).expect("valid"));
}

#[test]
fn a_wrong_version_inside_a_file_or_a_file_that_isnt_a_database_moves_to_a_new_generation() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 26, 30);
    read(&dir, 0);
    let first = path(&dir).expect("path");
    let old_reader = conn(&dir);
    put_meta(&conn(&dir), "schema_version", &0i64.to_le_bytes()).expect("set");
    let outcome = read(&dir, 0);
    assert!(outcome.new_generation, "{outcome:?}");
    let second = path(&dir).expect("path");
    assert_ne!(first, second);
    assert!(first.exists(), "the old generation is never unlinked");
    let _: i64 =
        old_reader.query_row("SELECT count(*) FROM obj", [], |r| r.get(0)).expect("the old handle still reads");

    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", second.display()));
    }
    std::fs::write(&second, vec![7u8; 8192]).expect("not a database");
    let outcome = read(&dir, 0);
    assert!(outcome.new_generation, "{outcome:?}");
    assert_eq!(std::fs::read(&second).expect("r"), vec![7u8; 8192], "a damaged file is never written");
}

#[test]
fn a_lock_not_granted_in_time_is_not_damage() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 27, 30);
    read(&dir, 0);
    let mut holder = conn(&dir);
    let _lock = holder.transaction_with_behavior(TransactionBehavior::Immediate).expect("locked");
    let before = take(&dir);
    let store = Store::open(&dir).expect("valid");
    let quick = Options { racy_ns: 0, busy: Duration::from_millis(50) };
    assert!(refresh_with(&dir, &store, &before, Record::Yes, quick).is_err());
    assert_eq!(current_generation(&std::fs::canonicalize(dir.join("cache")).expect("cache")), Some(1));
}

#[test]
fn a_ground_mismatch_rebuilds_in_place() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 28, 30);
    read(&dir, 0);
    let first = path(&dir).expect("path");
    put_meta(&conn(&dir), "ground", &[9u8; 32]).expect("set");
    let outcome = read(&dir, 0);
    assert!(outcome.rebuilt && !outcome.new_generation);
    assert_eq!(path(&dir).expect("path"), first);
}

#[test]
fn a_changed_value_in_any_table_fails_its_checksum() {
    let cases = [
        ("obj", "UPDATE obj SET kind = kind + 1 WHERE seq = (SELECT max(seq) FROM obj)"),
        ("agent", "UPDATE agent SET key = zeroblob(32) WHERE n = 0"),
        ("parent", "UPDATE parent SET ck = zeroblob(8) WHERE child = (SELECT max(child) FROM parent)"),
        ("child", "UPDATE child SET ck = zeroblob(8) WHERE parent = 0"),
        ("head", "UPDATE head SET seq = 0"),
        ("tag", "UPDATE tag SET tag = tag || 'x' WHERE seq = (SELECT min(seq) FROM tag)"),
        ("ref", "UPDATE ref SET rkind = 6 WHERE src = (SELECT max(src) FROM ref)"),
        ("fwd", "UPDATE fwd SET to_end = 0 WHERE rel = (SELECT max(rel) FROM fwd)"),
        ("hidden", "UPDATE hidden SET ck = zeroblob(8)"),
        ("shard", "UPDATE shard SET mtime_ns = 1 WHERE present = 1"),
        // meta's ck is checked before its values are: a changed schema_version is a checksum failure,
        // not another version's file.
        ("meta schema_version", "UPDATE meta SET v = zeroblob(8) WHERE k = 'schema_version'"),
        ("meta ground", "UPDATE meta SET v = zeroblob(32) WHERE k = 'ground'"),
        ("meta file (device and inode)", "UPDATE meta SET v = zeroblob(16) WHERE k = 'file'"),
        ("meta derived_stale", "UPDATE meta SET v = x'01' WHERE k = 'derived_stale'"),
        ("meta fs_clock", "UPDATE meta SET v = zeroblob(16) WHERE k = 'fs_clock'"),
    ];
    for (what, sql) in cases {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 29, 80);
        read(&dir, 0);
        let c = conn(&dir);
        c.execute_batch(sql).expect("changed");
        drop(c);
        let before = Before::stats(&dir);
        let store = Store::open(&dir).expect("valid");
        let err = compare(&dir, &store, &before).expect_err(what);
        assert!(format!("{err:#}").contains("checksum"), "{what}: {err:#}");
        // An ordinary refresh that reads the row rebuilds; verify always does. Either way the index
        // then agrees with the full computation.
        rebuild(&dir, &store, &take(&dir)).expect("verify rebuilds");
        compare(&dir, &store, &Before::stats(&dir)).expect(what);
    }
}

#[test]
fn a_changed_row_that_refresh_reads_makes_it_rebuild() {
    for sql in [
        "UPDATE meta SET v = x'01' WHERE k = 'derived_stale'",
        "UPDATE shard SET mtime_s = mtime_s + 1 WHERE present = 1",
        "UPDATE obj SET author = author + 0, ts_key = ts_key + 1",
    ] {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 30, 40);
        read(&dir, 0);
        conn(&dir).execute_batch(sql).expect("changed");
        if sql.contains("obj") {
            grow(&dir, 31, 1); // a dirty shard, so its obj rows are read
        }
        assert!(read(&dir, 0).rebuilt, "{sql}");
    }
}

#[test]
fn another_schema_versions_file_is_never_touched() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 32, 20);
    std::fs::create_dir_all(dir.join("cache")).expect("cache/");
    let other = dir.join("cache").join(format!("index-v{}-1.sqlite", SCHEMA_VERSION + 1));
    std::fs::write(&other, b"a newer binary's index").expect("w");
    let modified = std::fs::metadata(&other).expect("m").modified().expect("mtime");
    read(&dir, 0);
    grow(&dir, 33, 5);
    read(&dir, 0);
    assert_eq!(std::fs::read(&other).expect("r"), b"a newer binary's index");
    assert_eq!(std::fs::metadata(&other).expect("m").modified().expect("mtime"), modified);
}

#[test]
fn a_timestamp_beyond_i64_is_indexed_and_orders_last() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 34, 10);
    let mut store = Store::open(&dir).expect("valid");
    let who = store.identity("builder").expect("an identity");
    let heads: Vec<ContributionId> = store.graph().heads().collect();
    let late = Contribution::create(
        store.info().ground,
        &who,
        heads,
        u64::MAX,
        Body::Claim { statement: "far future".into(), tags: vec!["note".into()] },
    );
    store.add(late).expect("valid");
    read(&dir, 0);
    let max: i64 = rows(&dir, "SELECT max(ts_key) FROM obj");
    assert_eq!(max, ts_key(u64::MAX));
    assert!(ts_key(u64::MAX) > ts_key(1 << 63) && ts_key(1 << 63) > ts_key(0));
}

#[test]
fn the_view_and_a_full_check_without_an_index_write_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 35, 20);
    crate::snapshot::open(&dir, crate::snapshot::Refresh::No).expect("opens");
    let listed = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir.join("cache"))
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    };
    assert!(listed(&dir).iter().all(|n| !n.starts_with("index-") && n != "clock"), "{:?}", listed(&dir));
    compare(&dir, &Store::open(&dir).expect("valid"), &Before::stats(&dir)).expect("nothing to compare");
    assert!(listed(&dir).iter().all(|n| !n.starts_with("index-") && n != "clock"), "{:?}", listed(&dir));
}

#[cfg(unix)]
#[test]
fn a_replica_whose_cache_cant_be_written_never_uses_the_index() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return; // permissions don't bind root
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 36, 20);
    std::fs::create_dir_all(dir.join("cache")).expect("cache/");
    std::fs::set_permissions(dir.join("cache"), std::fs::Permissions::from_mode(0o555)).expect("read-only");
    assert!(!usable(&dir));
    crate::snapshot::open(&dir, crate::snapshot::Refresh::Yes).expect("opens through the full path");
    let names: Vec<String> = std::fs::read_dir(dir.join("cache"))
        .expect("listed")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    std::fs::set_permissions(dir.join("cache"), std::fs::Permissions::from_mode(0o755)).expect("restored");
    assert!(names.iter().all(|n| !n.starts_with("index-")), "{names:?}");
}

#[cfg(unix)]
#[test]
fn a_linked_index_file_is_never_written_through() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 37, 20);
    std::fs::create_dir_all(dir.join("cache")).expect("cache/");
    let outside = tmp.path().join("outside");
    std::fs::write(&outside, b"keep me").expect("w");
    std::os::unix::fs::symlink(&outside, dir.join("cache").join(file_name(1))).expect("link");
    let before = take(&dir);
    let store = Store::open(&dir).expect("valid");
    let _ = refresh(&dir, &store, &before, Record::Yes);
    assert_eq!(std::fs::read(&outside).expect("r"), b"keep me");
}

#[cfg(unix)]
#[test]
fn a_copied_index_is_rebuilt_before_it_is_used() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 38, 30);
    read(&dir, 0);
    // A copy of the Ground, index included, as cp -R or a checked-out tracked file would make.
    let copy = tmp.path().join("copy");
    let status = std::process::Command::new("cp").arg("-R").arg(&dir).arg(&copy).status().expect("cp");
    assert!(status.success());
    let outcome = read(&copy, 0);
    assert!(outcome.rebuilt, "another file's inode: {outcome:?}");
}

#[test]
fn concurrent_work_pulled_both_ways_orders_as_topo_order_on_both_replicas() {
    let tmp = tempfile::tempdir().expect("tmp");
    let a_dir = ground(tmp.path());
    grow(&a_dir, 39, 30);
    let b_dir = tmp.path().join("b");
    let mut a = Store::open(&a_dir).expect("valid");
    let mut b = Store::create(&b_dir, GroundInfo { ground: a.info().ground, name: "b".into() }).expect("a replica");
    std::fs::create_dir_all(b.identities_dir()).expect("identities/");
    for p in PEOPLE {
        let id = a.identity(p).expect("an identity");
        id.save(&b.identities_dir().join(format!("{p}.json"))).expect("saved");
    }
    manyroot::store::sync(&mut a, &mut b).expect("synced");
    read(&a_dir, 0);
    read(&b_dir, 0);
    for round in 0..3 {
        let mut a = Store::open(&a_dir).expect("valid");
        let mut b = Store::open(&b_dir).expect("valid");
        grow_store(&mut a, 40 + round, 15);
        grow_store(&mut b, 50 + round, 15);
        read(&a_dir, 0);
        read(&b_dir, 0);
        manyroot::store::sync(&mut a, &mut b).expect("synced");
        let oa = read(&a_dir, 0);
        let ob = read(&b_dir, 0);
        assert!(oa.added > 0 && ob.added > 0 && !oa.rebuilt && !ob.rebuilt, "{oa:?} {ob:?}");
    }
    let a = Store::open(&a_dir).expect("valid");
    assert_eq!(expected(&a).order, actual(&conn(&b_dir)).expect("reads").order, "same graph, same order");
}

#[test]
fn a_new_root_is_ordered_too() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 60, 20);
    read(&dir, 0);
    let mut store = Store::open(&dir).expect("valid");
    let who = store.identity("reviewer").expect("an identity");
    let root = Contribution::create(
        store.info().ground,
        &who,
        [],
        0,
        Body::Claim { statement: "a new root".into(), tags: vec!["note".into()] },
    );
    store.add(root).expect("valid");
    assert_eq!(read(&dir, 0).added, 1);
}
