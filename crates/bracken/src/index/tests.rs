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
    /// Makes [`durable`] fail on this thread, after recording what it was asked to sync.
    pub(super) static FAIL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether [`durable`] makes its `fsync` calls on this thread. Off in these unit tests, which check
    /// what is synced and when through `SYNCED` and `FAIL_SYNC`; everything else `durable` does (each
    /// path opened without following a link, its type checked) still runs. The integration tests run the
    /// binary, which always syncs, and `durable_syncs_files_and_directories` turns it on here (note
    /// b33068a95bbf: on the box's ext4 these syscalls were half the unit tests' time).
    pub(super) static REAL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
    settle(dir);
    let mut before = Before::take(dir);
    if before.clock.is_none()
        && let Some(cache) = crate::snapshot::cache::Cache::open(dir, true).expect("cache/")
    {
        before.clock = cache.clock().ok().map(|(t, _)| t);
    }
    before
}

/// Waits until the filesystem's clock has passed every shard's mtime and ctime. The tests read with no
/// racy margin, but a stamp is still recorded only if it is strictly older than T, and Linux stamps files
/// from a coarse clock: a read in the same tick as the writes before it would leave their shards dirty
/// once more. In use, the margin is 2 s; this only lets the time pass that any real read follows.
fn settle(dir: &Path) {
    use std::os::unix::fs::MetadataExt;
    let ns = |s: i64, n: i64| i128::from(s) * 1_000_000_000 + i128::from(n);
    let newest = std::fs::read_dir(dir.join("objects"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| ns(m.mtime(), m.mtime_nsec()).max(ns(m.ctime(), m.ctime_nsec())))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let probe = dir.with_extension("tick");
    for _ in 0..2_000 {
        std::fs::write(&probe, b"t").expect("a probe beside the Ground");
        let m = std::fs::metadata(&probe).expect("its stat");
        if ns(m.mtime(), m.mtime_nsec()) > newest {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the filesystem's clock never passed the newest shard");
}

fn opts(racy: i128) -> Options {
    Options { racy_ns: racy, busy: BUSY, sync: true }
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

/// A copy of the directory tree `from` at `to` (files and directories only, as a Ground holds).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for e in std::fs::read_dir(from).expect("listed") {
        let e = e.expect("an entry");
        let kind = e.file_type().expect("its type");
        if kind.is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).expect("copied");
        }
    }
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
    // Until a new contribution lands in a shard that was already there (finding 3da85ae371bf: most land
    // in a new one, where putting the mtime back tests nothing).
    for k in 0..200u64 {
        let before = take(&dir);
        grow(&dir, 17 + k, 1);
        let head = Store::open(&dir).expect("valid").graph().heads().next().expect("a head");
        let n = shard_of(&head);
        let Seen::Present(Some(old)) = before.shards[usize::from(n)] else {
            read(&dir, 0);
            continue;
        };
        let shard = dir.join("objects").join(format!("{n:02x}"));
        let t = rustix::fs::Timespec { tv_sec: old.mtime_s, tv_nsec: old.mtime_ns as _ };
        let times = rustix::fs::Timestamps { last_access: t, last_modification: t };
        rustix::fs::utimensat(rustix::fs::CWD, &shard, &times, rustix::fs::AtFlags::empty()).expect("mtime put back");
        let now = Before::stats(&dir).shards[usize::from(n)];
        let Seen::Present(Some(now)) = now else { panic!("the shard is there") };
        assert_eq!((now.mtime_s, now.mtime_ns), (old.mtime_s, old.mtime_ns), "the mtime is the recorded one");
        assert_eq!(read(&dir, 0).added, 1, "the ctime still differs, so the shard is diffed");
        return;
    }
    panic!("no new contribution landed in an existing shard");
}

#[test]
fn a_write_through_refreshing_is_indexed_and_leaves_its_shard_without_a_row() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 18, 30);
    read(&dir, 0);
    let indexed_shards: HashSet<u8> =
        Store::open(&dir).expect("valid").graph().iter().map(|c| shard_of(&c.id())).collect();
    SYNCED.with(|s| s.borrow_mut().clear());
    let written = {
        let mut s = crate::snapshot::Refreshing::open(&dir, crate::snapshot::Refresh::Yes).expect("opens");
        let who = s.identity("builder").expect("an identity");
        s.contribute(&who, Body::Claim { statement: "kept".into(), tags: vec!["decision".into()] }).expect("ok")
    };
    let hex = written.to_hex().to_uppercase();
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM obj WHERE hex(id) = '{hex}'")), 1, "indexed by the write");
    let synced = SYNCED.with(|s| s.borrow().clone());
    // The store synced the write's own object before its rename; the index doesn't again.
    assert!(!synced.contains(&object_path(&dir, &written)), "the store synced the write's own: {synced:?}");
    let shard = dir.join("objects").join(format!("{:02x}", shard_of(&written)));
    assert!(synced.contains(&shard), "its shard directory, before the commit (the store doesn't sync it): {synced:?}");
    let new = !indexed_shards.contains(&shard_of(&written));
    assert_eq!(synced.contains(&dir.join("objects")), new, "objects/ for a shard new to the index only: {synced:?}");
    assert_eq!(rows(&dir, &format!("SELECT count(*) FROM shard WHERE n = {}", shard_of(&written))), 0);
    assert_eq!(read(&dir, 0).added, 0);
}

#[test]
fn a_writes_refresh_syncs_every_object_it_adds_but_its_own() {
    // Security review note 1: a write's refresh may add another process's object, renamed in without a
    // sync; only what this command's store wrote is known to be synced.
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 20, 30);
    read(&dir, 0);
    let mut store = Store::open(&dir).expect("valid");
    let before_ids: HashSet<ContributionId> = store.graph().iter().map(|c| c.id()).collect();
    let indexed_shards: HashSet<u8> = before_ids.iter().map(shard_of).collect();
    grow_store(&mut store, 21, 6);
    let added: Vec<ContributionId> =
        store.graph().iter().map(|c| c.id()).filter(|id| !before_ids.contains(id)).collect();
    let (own, others) = added.split_at(added.len() / 2);
    let written: HashSet<ContributionId> = own.iter().copied().collect();
    SYNCED.with(|s| s.borrow_mut().clear());
    let outcome = refresh_after_write(&dir, &store, &Before::stats(&dir), &written).expect("refreshed");
    assert_eq!(outcome.added, added.len());
    let synced = SYNCED.with(|s| s.borrow().clone());
    for id in own {
        assert!(!synced.contains(&object_path(&dir, id)), "its own, synced by the store: {synced:?}");
    }
    for id in others {
        assert!(synced.contains(&object_path(&dir, id)), "another's, before the commit: {synced:?}");
    }
    for id in &added {
        let shard = dir.join("objects").join(format!("{:02x}", shard_of(id)));
        assert!(synced.contains(&shard), "every shard directory: {synced:?}");
    }
    let new = added.iter().any(|id| !indexed_shards.contains(&shard_of(id)));
    assert_eq!(synced.contains(&dir.join("objects")), new, "objects/ for a shard new to the index only");
    agrees(&dir, &store);

    // One write at a time, until one lands in a shard new to the index and one in a shard it holds.
    let (mut seen_new, mut seen_held) = (false, false);
    for k in 0..400u64 {
        let indexed: HashSet<u8> = store.graph().iter().map(|c| shard_of(&c.id())).collect();
        let had: HashSet<ContributionId> = store.graph().iter().map(|c| c.id()).collect();
        grow_store(&mut store, 100 + k, 1);
        let Some(id) = store.graph().iter().map(|c| c.id()).find(|id| !had.contains(id)) else { continue };
        SYNCED.with(|s| s.borrow_mut().clear());
        refresh_after_write(&dir, &store, &Before::stats(&dir), &HashSet::from([id])).expect("refreshed");
        let synced = SYNCED.with(|s| s.borrow().clone());
        let new = !indexed.contains(&shard_of(&id));
        assert_eq!(synced.contains(&dir.join("objects")), new, "objects/ for a shard new to the index only");
        seen_new |= new;
        seen_held |= !new;
        if seen_new && seen_held {
            return;
        }
    }
    panic!("no write landed in both a new shard and a held one");
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
    // A stale index, none, and a damaged one (finding 3da85ae371bf): four processes at once add each
    // object exactly once between them, whichever gets there first.
    for (what, seed) in [("stale", 22u64), ("missing", 24), ("damaged", 26)] {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, seed, 60);
        if what != "missing" {
            read(&dir, 0);
        }
        grow(&dir, seed + 1, 60);
        let total = Store::open(&dir).expect("valid").graph().len();
        let expected = match what {
            "stale" => total - rows(&dir, "SELECT count(*) FROM obj") as usize,
            _ => total,
        };
        if what == "damaged" {
            // Not a database: its -wal and -shm go too, or SQLite would read every page from the WAL,
            // which stays between commands.
            let file = path(&dir).expect("path");
            for suffix in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", file.display()));
            }
            std::fs::write(&file, vec![7u8; 8192]).expect("not a database");
        }
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
        let outcomes: Vec<Outcome> = handles.into_iter().map(|h| h.join().expect("joined")).collect();
        let added: usize = outcomes.iter().map(|o| o.added).sum();
        assert_eq!(added, expected, "{what}: each object added once: {outcomes:?}");
        assert_eq!(outcomes.iter().filter(|o| o.rebuilt).count(), usize::from(what != "stale"), "{what}: {outcomes:?}");
        assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), total as i64, "{what}");
        assert_eq!(read(&dir, 0), Outcome::default(), "{what}");
    }
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
    let quick = Options { racy_ns: 0, busy: Duration::from_millis(50), sync: true };
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
        ("meta ids", "UPDATE meta SET v = zeroblob(32) WHERE k = 'ids'"),
        ("meta key", "UPDATE meta SET v = zeroblob(32) WHERE k = 'key'"),
    ];
    // One Ground, copied for each case: its 80 writes, each synced by the store, once.
    let template = tempfile::tempdir().expect("tmp");
    let made = ground(template.path());
    grow(&made, 29, 80);
    for (what, sql) in cases {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("g");
        copy_tree(&made, &dir);
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

/// A refresh recomputes only what its additions can change (section 4): positions after the smallest
/// largest-parent position among them, and the hidden status of the additions alone unless one is a
/// retraction or a supersession. Refreshed after every contribution, and after small batches of each
/// kind, the index equals the full recompute every time, order and hidden set included.
#[test]
fn refreshes_after_each_addition_recompute_only_what_it_changes_and_agree() {
    for seed in [41u64, 42, 43] {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, seed, 4);
        read(&dir, 0);
        for i in 0..40 {
            grow(&dir, seed * 1000 + i, 1 + (i as usize % 3));
            let outcome = read(&dir, 0);
            assert!(!outcome.rebuilt, "seed {seed}, step {i}: an addition never rebuilds: {outcome:?}");
        }
    }
}

#[test]
fn can_hide_is_only_retractions_and_supersessions() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 7, 200);
    let store = Store::open(&dir).expect("valid");
    let mut seen = HashSet::new();
    for c in store.graph().iter() {
        let want = match c.body() {
            Body::Retract { .. } => true,
            Body::Relation { kind, .. } => *kind == RelationKind::Supersedes,
            _ => false,
        };
        assert_eq!(can_hide(c), want, "{:?}", c.body().kind());
        seen.insert((c.body().kind(), want));
    }
    assert!(seen.len() >= 5, "every kind, and both sorts of relation: {seen:?}");
}

/// Writes `body` as `who` through `Store` alone (no index update), as another process would.
fn contribute(dir: &Path, who: &str, body: Body) -> ContributionId {
    let mut store = Store::open(dir).expect("valid");
    let me = store.identity(who).expect("an identity");
    store.contribute(&me, body).expect("valid")
}

fn claim(statement: &str) -> Body {
    Body::Claim { statement: statement.into(), tags: vec!["note".into()] }
}

/// Finding cf0181613a78: the index and the store can hold sets of one size that differ. Another process
/// indexes Z, which this store never listed, while this store holds X, in a shard its stamps say is
/// clean, which the index lacks. A count says they agree; recomputing `hidden` from this store then drops
/// Z, which is hidden (it is evidence about a retracted claim), and nothing later puts it back. With the
/// ids' digest the derived tables wait until the sets agree.
#[test]
fn equal_counts_of_different_sets_never_bring_the_derived_tables_up_to_date() {
    for attempt in 0..20u32 {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        let c0 = contribute(&dir, "manager", claim(&format!("c0 {attempt}")));
        contribute(&dir, "manager", Body::Retract { target: c0, reason: "withdrawn".into() });
        let c1 = contribute(&dir, "builder", claim(&format!("c1 {attempt}")));
        read(&dir, 0);
        let pre = take(&dir);
        // X, Y and Z are siblings, each written by its own process from the same heads.
        let mut stores: Vec<Store> = (0..3).map(|_| Store::open(&dir).expect("valid")).collect();
        let mut write = |k: usize, who: &str, body: Body| -> ContributionId {
            let me = stores[k].identity(who).expect("an identity");
            stores[k].contribute(&me, body).expect("valid")
        };
        // Y, a retraction (so this refresh recomputes `hidden` whole), in a shard left dirty.
        let y = write(0, "builder", Body::Retract { target: c1, reason: "withdrawn".into() });
        let stale = take(&dir);
        // X, which the index never gets from this refresh: its shard is clean in `stale`.
        let x = write(1, "reviewer", claim(&format!("x {attempt}")));
        let this = Store::open(&dir).expect("valid");
        // Z, hidden, written and indexed by another process that never saw X's or Y's shard change.
        let z = write(
            2,
            "manager",
            Body::Evidence { target: c0, verdict: Verdict::Pass, method: "check".into(), details: String::new() },
        );
        let shards = [shard_of(&y), shard_of(&x), shard_of(&z)];
        if shards[0] == shards[1] || shards[1] == shards[2] || shards[0] == shards[2] {
            continue; // the scenario needs three shards; ids are random
        }
        let mut other = Before::stats(&dir);
        for n in [shards[0], shards[1]] {
            other.shards[usize::from(n)] = pre.shards[usize::from(n)];
        }
        let theirs = Store::open(&dir).expect("valid");
        let theirs_outcome = refresh_with(&dir, &theirs, &other, Record::No, opts(0)).expect("they index Z");
        assert_eq!(theirs_outcome.added, 1, "only Z: {theirs_outcome:?}");
        assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), this.graph().len() as i64 - 1);
        // This refresh: Y added, so the index has as many contributions as this store, but not the same.
        refresh_with(&dir, &this, &stale, Record::Yes, opts(0)).expect("refreshed");
        assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), this.graph().len() as i64);
        // Every later open, with nothing left dirty, must still agree with the full computation.
        read(&dir, 0);
        read(&dir, 0);
        return;
    }
    panic!("no attempt put Y, X and Z in three shards");
}

/// Finding e138686585be: an open with no trusted clock treats every shard as dirty and drops every old
/// stamp, even where an index already holds them (section 5 step 1).
#[test]
fn an_open_without_a_clock_finds_every_shard_dirty_and_drops_the_old_stamps() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 50, 40);
    read(&dir, 0);
    assert_eq!(rows(&dir, "SELECT count(*) FROM shard"), 256);
    let clock = dir.join("cache").join("clock");
    std::fs::remove_file(&clock).expect("clock removed");
    std::fs::create_dir(&clock).expect("a directory where cache/clock goes");
    let before = Before::take(&dir);
    assert!(before.clock.is_none());
    let store = Store::open(&dir).expect("valid");
    let outcome = refresh_with(&dir, &store, &before, Record::Yes, opts(0)).expect("refreshed");
    assert_eq!(outcome.dirty_shards, 256, "{outcome:?}");
    assert_eq!(rows(&dir, "SELECT count(*) FROM shard"), 0, "every old stamp goes");
    agrees(&dir, &store);
}

/// Finding 32fc7df1acf4: a `cache/` that is a link is never used, by an ordinary open or the full check,
/// and nothing behind the link changes.
#[cfg(unix)]
#[test]
fn a_linked_cache_is_never_used_even_by_the_full_check() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 51, 30);
    read(&dir, 0);
    let victim = tmp.path().join("victim");
    std::fs::rename(dir.join("cache"), &victim).expect("moved");
    std::os::unix::fs::symlink(&victim, dir.join("cache")).expect("linked");
    let state = |p: &Path| -> Vec<(String, u64, std::time::SystemTime)> {
        let mut v: Vec<_> = std::fs::read_dir(p)
            .expect("listed")
            .flatten()
            .map(|e| {
                let m = e.metadata().expect("m");
                (e.file_name().to_string_lossy().into_owned(), m.len(), m.modified().expect("mtime"))
            })
            .collect();
        v.sort();
        v
    };
    let was = state(&victim);
    assert!(!usable(&dir), "a linked cache/ isn't usable");
    let store = Store::open(&dir).expect("valid");
    compare(&dir, &store, &Before::stats(&dir)).expect("compares the in-memory index only");
    assert_eq!(state(&victim), was, "nothing behind the link changed");
}

/// Security review note 3: a file named with the last generation doesn't overflow.
#[test]
fn the_last_generation_damaged_doesnt_overflow() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 52, 10);
    std::fs::create_dir_all(dir.join("cache")).expect("cache/");
    std::fs::write(dir.join("cache").join(file_name(u64::MAX)), vec![7u8; 8192]).expect("not a database");
    let before = take(&dir);
    let store = Store::open(&dir).expect("valid");
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| refresh(&dir, &store, &before, Record::Yes)));
    let got = got.expect("no overflow panic");
    assert!(got.is_err(), "no generation after the last, so the index isn't used: {got:?}");
}

/// Security review note 4: syncing never waits on a FIFO put where an object or a shard was.
#[cfg(unix)]
#[test]
fn durable_syncs_files_and_directories() {
    REAL_SYNC.with(|r| r.set(true));
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 54, 20);
    let outcome = read(&dir, 0);
    assert!(outcome.synced > 0, "the first build syncs what it names: {outcome:?}");
    let file = Store::open(&dir).expect("valid").graph().iter().map(|c| object_path(&dir, &c.id())).next();
    let paths = [file.expect("an object"), dir.join("objects")];
    assert_eq!(durable(&paths).expect("synced"), 2);
    REAL_SYNC.with(|r| r.set(false));
}

#[test]
fn a_fifo_in_the_sync_list_is_refused_without_waiting() {
    let tmp = tempfile::tempdir().expect("tmp");
    let fifo = tmp.path().join("fifo");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status().expect("mkfifo runs");
    assert!(made.success(), "a FIFO");
    let (send, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(durable(&[fifo]).is_err());
    });
    let refused = got.recv_timeout(Duration::from_secs(20)).expect("durable returned instead of waiting on the FIFO");
    assert!(refused, "a FIFO is neither a file nor a directory");
}

/// Security review note 5: the index's connections distrust the file's schema and run in defensive mode.
#[test]
fn connections_distrust_the_files_schema() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 53, 5);
    read(&dir, 0);
    let c = conn(&dir);
    let trusted: i64 = c.query_row("PRAGMA trusted_schema", [], |r| r.get(0)).expect("pragma");
    assert_eq!(trusted, 0);
    assert!(c.db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE).expect("db_config"));
}

/// Security review note 2: a directory at the index's path is damage (the next generation is used), and a
/// view or a trigger put in the file doesn't stop a rebuild.
#[test]
fn a_directory_at_the_index_path_or_a_view_in_the_file_doesnt_stop_the_index() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 54, 20);
    std::fs::create_dir_all(dir.join("cache").join(file_name(1))).expect("a directory where the index goes");
    let outcome = read(&dir, 0);
    assert!(outcome.new_generation, "{outcome:?}");

    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 55, 20);
    read(&dir, 0);
    let c = conn(&dir);
    c.execute_batch(
        "DROP TABLE tag; CREATE VIEW tag AS SELECT 1 AS tag, 2 AS seq, x'00' AS ck; \
         CREATE TRIGGER noisy AFTER INSERT ON obj BEGIN SELECT 1; END;",
    )
    .expect("a view and a trigger");
    drop(c);
    let store = Store::open(&dir).expect("valid");
    rebuild(&dir, &store, &take(&dir)).expect("a rebuild clears whatever the file held");
    agrees(&dir, &store);
    let left: i64 = conn(&dir)
        .query_row("SELECT count(*) FROM sqlite_master WHERE type IN ('view', 'trigger')", [], |r| r.get(0))
        .expect("count");
    assert_eq!(left, 0);
}

/// Security review note 1: a write's own refresh syncs each object it adds that another writer put there.
#[test]
fn a_writes_refresh_syncs_the_objects_it_adds() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 56, 20);
    read(&dir, 0);
    let theirs = contribute(&dir, "reviewer", claim("renamed in by another writer"));
    let store = Store::open(&dir).expect("valid");
    SYNCED.with(|s| s.borrow_mut().clear());
    refresh_with(&dir, &store, &Before::stats(&dir), Record::No, opts(0)).expect("refreshed");
    let synced = SYNCED.with(|s| s.borrow().clone());
    assert!(synced.contains(&object_path(&dir, &theirs)), "{synced:?}");
}

/// Section 8, Value corruption: a row deleted from a shard whose stamp is clean, which no refresh will
/// look at again, fails the full check, in every table that holds rows per contribution.
#[test]
fn a_row_deleted_under_a_clean_stamp_fails_the_full_check() {
    for sql in [
        "DELETE FROM obj WHERE seq = (SELECT max(seq) FROM obj)",
        "DELETE FROM tag WHERE seq = (SELECT min(seq) FROM tag)",
        "DELETE FROM ref WHERE src = (SELECT max(src) FROM ref)",
        "DELETE FROM parent WHERE child = (SELECT max(child) FROM parent)",
        "DELETE FROM head",
    ] {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 57, 60);
        read(&dir, 0);
        assert_eq!(read(&dir, 0), Outcome::default(), "every stamp clean");
        conn(&dir).execute_batch(sql).expect("deleted");
        let store = Store::open(&dir).expect("valid");
        assert!(compare(&dir, &store, &Before::stats(&dir)).is_err(), "{sql}");
    }
}

/// Section 8, Read-only: with an index already there, the view and a replica whose cache/ can't be
/// written leave it as it was, and make no `-wal` or `-shm`.
#[cfg(unix)]
#[test]
fn an_existing_index_is_left_unchanged_by_the_view_and_a_read_only_cache() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 58, 30);
    read(&dir, 0);
    grow(&dir, 59, 5); // something for an open that wrote to find
    // Every file in cache/, with its bytes and mtime: the index, its -wal and -shm (which stay between
    // commands, note b33068a95bbf), the clock and the snapshot.
    let state = || cache_files(&dir);
    let was = state();
    let file = path(&dir).expect("path");
    let name = |s: &str| format!("{}{s}", file.file_name().expect("a name").to_string_lossy());
    for n in [name(""), name("-wal"), name("-shm")] {
        assert!(was.contains_key(&n), "{n} is there: {:?}", was.keys());
    }
    crate::snapshot::open(&dir, crate::snapshot::Refresh::No).expect("the view opens");
    assert!(state() == was, "the view left the index unchanged");
    if rustix::process::geteuid().is_root() {
        return; // permissions don't bind root
    }
    std::fs::set_permissions(dir.join("cache"), std::fs::Permissions::from_mode(0o555)).expect("read-only");
    let opened = crate::snapshot::open(&dir, crate::snapshot::Refresh::Yes);
    let after = state();
    std::fs::set_permissions(dir.join("cache"), std::fs::Permissions::from_mode(0o755)).expect("restored");
    opened.expect("opens through the full path");
    assert!(after == was, "a read-only cache/ left the index unchanged");
}

/// Finding a713038d77ae: a schema SQLite can't drop (a virtual table of a module this build lacks, written
/// in by another SQLite client with defensive mode off) is damage, so verify and a rebuild use the next
/// generation instead of failing every time.
#[test]
fn a_schema_that_cant_be_dropped_moves_to_the_next_generation() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 60, 20);
    read(&dir, 0);
    let first = path(&dir).expect("path");
    {
        let c = rusqlite::Connection::open(&first).expect("another client");
        c.execute_batch(
            "PRAGMA writable_schema = ON; \
             INSERT INTO sqlite_master VALUES ('table', 'vt', 'vt', 0, 'CREATE VIRTUAL TABLE vt USING nosuchmod()'); \
             PRAGMA writable_schema = OFF;",
        )
        .expect("a virtual table of an unknown module");
    }
    let store = Store::open(&dir).expect("valid");
    rebuild(&dir, &store, &take(&dir)).expect("verify's rebuild moves on to the next generation");
    assert_ne!(path(&dir).expect("path"), first, "the next generation");
    agrees(&dir, &store);
    // Later opens use it (verify's own racy margin left its recent shards dirty, so they're diffed again).
    let next = read(&dir, 0);
    assert!(!next.rebuilt && !next.new_generation && next.added == 0, "{next:?}");
}

/// The ids digest is keyed per index (security review round 2, note 1): two indexes, and two builds of one,
/// never share a key, so chosen ids can't be solved for a collision without reading this file.
#[test]
fn each_index_has_its_own_digest_key() {
    let key = |dir: &Path| -> Vec<u8> {
        conn(dir).query_row("SELECT v FROM meta WHERE k = 'key'", [], |r| r.get(0)).expect("a key")
    };
    let tmp = tempfile::tempdir().expect("tmp");
    let (a, b) = (ground(&tmp.path().join("a")), ground(&tmp.path().join("b")));
    for dir in [&a, &b] {
        grow(dir, 61, 10);
        read(dir, 0);
    }
    let (ka, kb) = (key(&a), key(&b));
    assert_eq!(ka.len(), 32);
    assert_ne!(ka, kb, "two indexes");
    let store = Store::open(&a).expect("valid");
    rebuild(&a, &store, &take(&a)).expect("rebuilt");
    assert_ne!(key(&a), ka, "a rebuild makes a new one");
    agrees(&a, &store);
}

/// The full check while another process writes (the box's speculative queue, evidence d625fbdec0f3's run):
/// that process may index what this process's store, opened earlier, never listed. The index then holds
/// more than this store, which isn't a disagreement in itself: the check lists the store again under the
/// index's lock and compares the index with that, whole. Anything a store holds that the index lacks
/// still fails it.
#[test]
fn the_full_check_allows_what_another_process_indexed_after_this_store_listed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 62, 30);
    read(&dir, 0);
    let before = Before::stats(&dir);
    let this = Store::open(&dir).expect("valid");
    grow(&dir, 63, 3);
    read(&dir, 0); // another process indexes the three
    compare(&dir, &this, &before).expect("an index ahead of this store is no disagreement");

    // Ahead, and wrong about what it's ahead with: a tag row of one of the three deleted. The full check
    // compares a store listed under its lock, so it still sees it.
    conn(&dir).execute_batch("DELETE FROM tag WHERE seq = (SELECT max(seq) FROM tag)").expect("deleted");
    assert!(compare(&dir, &this, &before).is_err(), "an index ahead of the store is still compared");
    rebuild(&dir, &Store::open(&dir).expect("valid"), &take(&dir)).expect("rebuilt");

    // Behind it instead: a row of a contribution already indexed, deleted, still fails (no refresh puts a
    // row back for a contribution the index already holds, whether its shard's stamp is recorded or not).
    conn(&dir).execute_batch("DELETE FROM tag WHERE seq = (SELECT min(seq) FROM tag)").expect("deleted");
    let now = Store::open(&dir).expect("valid");
    assert!(compare(&dir, &now, &Before::stats(&dir)).is_err());
}

/// Finding 48732994a715: the full check's two passes over an index ahead of its store don't share
/// writes. This store holds X, unindexed, in a dirty shard; another process, whose store never listed X,
/// indexed C, so the index is ahead. Bringing the index up to date with this store adds X and marks the
/// derived tables stale; if that stayed for the pass against the reopened store, that pass would recompute
/// `hidden` whole and hide damage to it. With the first pass rolled back to a savepoint, a damaged `hidden`
/// still fails the check, and the same scenario without the damage passes.
#[test]
fn an_index_ahead_with_damaged_derived_rows_still_fails_the_full_check() {
    let run = |damage: bool| -> Result<()> {
        // A fresh Ground for each attempt: one whose X and C shared a shard leaves both unindexed.
        for attempt in 0..20 {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = ground(tmp.path());
            grow(&dir, 64, 60);
            read(&dir, 0);
            assert!(rows(&dir, "SELECT count(*) FROM hidden") > 0, "the scenario needs something hidden");
            if damage {
                conn(&dir).execute_batch("DELETE FROM hidden").expect("damaged");
            }
            let mut theirs = Store::open(&dir).expect("valid"); // never lists X
            let x = contribute(&dir, "reviewer", claim(&format!("x {attempt}")));
            let before = Before::stats(&dir);
            let this = Store::open(&dir).expect("valid");
            let me = theirs.identity("builder").expect("an identity");
            let c = theirs.contribute(&me, claim(&format!("c {attempt}"))).expect("valid");
            if shard_of(&x) == shard_of(&c) {
                continue; // X's shard must stay dirty, apart from C's
            }
            let outcome = refresh_with(&dir, &theirs, &Before::stats(&dir), Record::No, opts(0)).expect("C indexed");
            assert_eq!(outcome.added, 1, "{outcome:?}");
            return compare(&dir, &this, &before);
        }
        panic!("no attempt put X and C in two shards");
    };
    run(false).expect("without the damage the index agrees");
    let err = run(true).expect_err("a damaged hidden set fails the check, the index being ahead or not");
    assert!(format!("{err:#}").contains("hidden"), "{err:#}");
}

/// Every file in `dir`'s cache/, by name, with its bytes and mtime.
fn cache_files(dir: &Path) -> std::collections::BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
    std::fs::read_dir(dir.join("cache"))
        .expect("cache/")
        .flatten()
        .map(|e| {
            let m = e.metadata().expect("m");
            (
                e.file_name().to_string_lossy().into_owned(),
                (std::fs::read(e.path()).expect("r"), m.modified().expect("t")),
            )
        })
        .collect()
}

/// Answer ddaf02041077: the index's commit is where what it names becomes durable, so no path commits a
/// row before syncing every object it names, its shard directory and objects/. With every sync failing, a
/// first build, a rebuild, a freshness refresh and a write's own refresh each commit nothing; with syncs
/// working, each syncs those paths.
#[test]
fn no_path_commits_before_it_syncs_what_it_names() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 70, 30);
    let failing = |dir: &Path, record: Record| {
        let store = Store::open(dir).expect("valid");
        FAIL_SYNC.with(|f| f.set(true));
        let got = refresh_with(dir, &store, &take(dir), record, opts(0));
        FAIL_SYNC.with(|f| f.set(false));
        assert!(got.is_err(), "{record:?}: a failed sync fails the refresh: {got:?}");
    };
    let ids = |dir: &Path| -> Vec<ContributionId> {
        Store::open(dir).expect("valid").graph().iter().map(|c| c.id()).collect()
    };
    let all_named = |dir: &Path, synced: &[PathBuf], ids: &[ContributionId]| {
        for id in ids {
            assert!(synced.contains(&object_path(dir, id)), "{id:?}'s object");
            let shard = dir.join("objects").join(format!("{:02x}", shard_of(id)));
            assert!(synced.contains(&shard), "{id:?}'s shard");
        }
    };

    // The first build: nothing committed, so no schema; then built, everything synced.
    failing(&dir, Record::Yes);
    let schema: i64 = conn(&dir).query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).expect("count");
    assert_eq!(schema, 0, "the failed build committed nothing");
    SYNCED.with(|s| s.borrow_mut().clear());
    assert!(read(&dir, 0).rebuilt);
    let synced = SYNCED.with(|s| s.borrow().clone());
    all_named(&dir, &synced, &ids(&dir));
    assert!(synced.contains(&dir.join("objects")), "and objects/");

    // Freshness and a write's own refresh: nothing of the new objects committed, then committed after.
    for record in [Record::Yes, Record::No] {
        let before: Vec<ContributionId> = ids(&dir);
        grow(&dir, 71 + record as u64, 5);
        let new: Vec<ContributionId> = ids(&dir).into_iter().filter(|id| !before.contains(id)).collect();
        let indexed = rows(&dir, "SELECT count(*) FROM obj");
        failing(&dir, record);
        assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), indexed, "{record:?}: nothing committed");
        SYNCED.with(|s| s.borrow_mut().clear());
        let store = Store::open(&dir).expect("valid");
        refresh_with(&dir, &store, &take(&dir), record, opts(0)).expect("refreshed");
        all_named(&dir, &SYNCED.with(|s| s.borrow().clone()), &new);
        agrees(&dir, &store);
    }

    // A rebuild in place (a ground mismatch): the old index stays whole, then everything is synced.
    put_meta(&conn(&dir), "ground", &[9u8; 32]).expect("a ground mismatch");
    let indexed = rows(&dir, "SELECT count(*) FROM obj");
    failing(&dir, Record::Yes);
    assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), indexed, "the failed rebuild committed nothing");
    SYNCED.with(|s| s.borrow_mut().clear());
    assert!(read(&dir, 0).rebuilt);
    all_named(&dir, &SYNCED.with(|s| s.borrow().clone()), &ids(&dir));
}

/// Note b33068a95bbf: a refresh that records nothing and adds nothing writes nothing, so a read of a Ground
/// whose index is current makes no SQLite commit, and no checkpoint has anything to sync.
#[test]
fn a_refresh_with_nothing_to_record_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = ground(tmp.path());
    grow(&dir, 75, 30);
    read(&dir, 0);
    assert_eq!(read(&dir, 0), Outcome::default(), "current");
    // The database file and its WAL, which a commit writes. Not -shm: SQLite's shared memory, where every
    // reader marks its place, which is never synced and holds no commit.
    let index = |dir: &Path| {
        let mut files = cache_files(dir);
        files.retain(|n, _| n.starts_with("index-") && !n.ends_with("-shm"));
        files
    };
    let was = index(&dir);
    assert!(was.keys().any(|n| n.ends_with("-wal")), "{:?}", was.keys());
    for _ in 0..3 {
        assert_eq!(read(&dir, 0), Outcome::default());
    }
    let now = index(&dir);
    let changed: Vec<&String> = now.keys().filter(|k| now.get(*k) != was.get(*k)).collect();
    assert!(changed.is_empty(), "changed: {changed:?}");

    // A shard written within the margin stays dirty and is listed again, and still nothing is written.
    grow(&dir, 76, 1);
    read(&dir, i128::MAX / 4); // adds it; the margin records no stamp
    let was = index(&dir);
    let again = read(&dir, i128::MAX / 4);
    assert!(again.dirty_shards > 0 && again.added == 0 && again.recorded == 0, "{again:?}");
    assert!(index(&dir) == was, "a dirty shard with nothing new writes nothing");
}
