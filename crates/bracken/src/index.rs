//! The index store (task `7d140f132b1a`, decision `f294933b4244`): a local, derived, rebuildable
//! SQLite database under `cache/`, in WAL mode, holding what reads need: ids, kinds, authors, the
//! causal order, parents, children, heads, tags, references and the `--current` hidden set. The design,
//! and the review findings it answers, is [docs/design/index-store.md]; section numbers below are its.
//!
//! **A cache, never the truth.** Everything here is derived from `objects/`, and deleting it loses
//! nothing. It has the snapshot's trust model (decision `96e282819607`, [`crate::snapshot`]'s header).
//! In this step every read still opens the store through the snapshot; the index is kept current beside
//! it, and `BRACKEN_FULL_CHECK` compares its answers with the full recompute ([`compare`]).
//!
//! **Freshness** (section 5). [`Before::take`] reads the filesystem's clock (`cache/clock`'s mtime, *T*)
//! and stats all 256 shards before anything lists `objects/`, the store's own open included. A shard is
//! clean only if its recorded stamp still matches; dirty shards are diffed against the store's ids. A
//! stamp is recorded only if it's more than [`RACY_NS`] older than *T*, only on allow-listed local
//! filesystems, and only after its directory (and any objects found in it) were synced.
//!
//! **Writes** (section 6). The store syncs an object and its shard directory before the index commits
//! it, and the write's transaction deletes that shard's row, so the next open lists it again.
//!
//! **Rebuilds** happen in place, in one transaction. A file SQLite calls corrupt, or one whose schema
//! version doesn't match its name, is never touched again: the next generation's file is made beside it.
//! Each schema version has its own files, so binaries of different versions never rebuild each other's.
//!
//! **Row checksums** (decision `c489f7f9dea9`): every row of every table carries `ck`, 8 bytes of blake3
//! over its other fields. Every row this module reads is checked; a mismatch rebuilds the index.
//!
//! [docs/design/index-store.md]: ../../../docs/design/index-store.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use manyroot::{Body, BodyKind, Contribution, ContributionId, RelationKind, Store, Verdict};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};

/// Bumped whenever the schema or what a row means changes. Each version has its own files.
pub const SCHEMA_VERSION: i64 = 1;
/// How much older than *T* a shard's stamp must be before it's recorded (git's racy-index rule).
pub const RACY_NS: i128 = 2_000_000_000;
const BUSY: Duration = Duration::from_secs(10);

const SCHEMA: &str = "
CREATE TABLE obj    (seq INTEGER PRIMARY KEY, pos INTEGER NOT NULL UNIQUE, id BLOB NOT NULL UNIQUE,
                     kind INTEGER NOT NULL, author INTEGER NOT NULL, ts_key INTEGER NOT NULL, ck BLOB NOT NULL);
CREATE TABLE agent  (n INTEGER PRIMARY KEY, key BLOB NOT NULL UNIQUE, ck BLOB NOT NULL);
CREATE TABLE parent (child INTEGER NOT NULL, parent INTEGER NOT NULL, ck BLOB NOT NULL,
                     PRIMARY KEY (child, parent)) WITHOUT ROWID;
CREATE TABLE child  (parent INTEGER NOT NULL, child INTEGER NOT NULL, ck BLOB NOT NULL,
                     PRIMARY KEY (parent, child)) WITHOUT ROWID;
CREATE TABLE head   (seq INTEGER PRIMARY KEY, ck BLOB NOT NULL);
CREATE TABLE tag    (tag TEXT NOT NULL, seq INTEGER NOT NULL, ck BLOB NOT NULL, PRIMARY KEY (tag, seq)) WITHOUT ROWID;
CREATE INDEX obj_kind   ON obj(kind, pos);
CREATE INDEX obj_author ON obj(author, pos);
CREATE TABLE ref    (target INTEGER NOT NULL, rkind INTEGER NOT NULL, src INTEGER NOT NULL, ck BLOB NOT NULL,
                     PRIMARY KEY (target, rkind, src)) WITHOUT ROWID;
CREATE TABLE fwd    (src_end INTEGER NOT NULL, rkind INTEGER NOT NULL, rel INTEGER NOT NULL,
                     to_end INTEGER NOT NULL, ck BLOB NOT NULL, PRIMARY KEY (src_end, rkind, rel)) WITHOUT ROWID;
CREATE TABLE hidden (seq INTEGER PRIMARY KEY, ck BLOB NOT NULL);
CREATE TABLE shard  (n INTEGER PRIMARY KEY, present INTEGER NOT NULL, mtime_s INTEGER, mtime_ns INTEGER,
                     ctime_s INTEGER, ctime_ns INTEGER, ino BLOB, dev BLOB, ck BLOB NOT NULL);
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL, ck BLOB NOT NULL);
";

// ---------------------------------------------------------------------------------------------------
// Codes and checksums

/// `obj.kind`.
fn kind_code(k: BodyKind) -> i64 {
    match k {
        BodyKind::Claim => 0,
        BodyKind::Artifact => 1,
        BodyKind::Relation => 2,
        BodyKind::Evidence => 3,
        BodyKind::Retract => 4,
    }
}

/// `ref.rkind` and `fwd.rkind`: a relation's kind, evidence by verdict, or a retraction.
fn relation_code(k: RelationKind) -> i64 {
    match k {
        RelationKind::Supports => 0,
        RelationKind::Contradicts => 1,
        RelationKind::DependsOn => 2,
        RelationKind::Refines => 3,
        RelationKind::Supersedes => 4,
        RelationKind::Implements => 5,
        RelationKind::Tests => 6,
    }
}

fn evidence_code(v: Verdict) -> i64 {
    match v {
        Verdict::Pass => 7,
        Verdict::Fail => 8,
    }
}

const RETRACT_CODE: i64 = 9;

/// The reference rows `c` contributes: (what it refers to, rkind).
fn refs_of(c: &Contribution) -> Vec<(ContributionId, i64)> {
    match c.body() {
        Body::Claim { .. } | Body::Artifact { .. } => vec![],
        Body::Relation { from, kind, to, .. } => vec![(*to, relation_code(*kind)), (*from, relation_code(*kind))],
        Body::Evidence { target, verdict, .. } => vec![(*target, evidence_code(*verdict))],
        Body::Retract { target, .. } => vec![(*target, RETRACT_CODE)],
    }
}

/// `timestamp_ms` as an i64 that orders as the u64 does (section 4, Integer widths).
pub fn ts_key(ms: u64) -> i64 {
    (ms ^ (1 << 63)) as i64
}

/// A field of a row, for its checksum.
#[derive(Clone, Copy)]
enum F<'a> {
    I(i64),
    B(&'a [u8]),
    N,
}

/// A row's checksum: the first 8 bytes of blake3 over its table's name and its fields, each tagged and
/// length-framed, so no two rows of different content share an encoding.
fn ck(table: &str, fields: &[F]) -> [u8; 8] {
    let mut h = blake3::Hasher::new();
    h.update(&(table.len() as u64).to_le_bytes());
    h.update(table.as_bytes());
    for f in fields {
        match f {
            F::I(i) => h.update(&[1]).update(&i.to_le_bytes()),
            F::B(b) => h.update(&[2]).update(&(b.len() as u64).to_le_bytes()).update(b),
            F::N => h.update(&[3]),
        };
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(&h.finalize().as_bytes()[..8]);
    out
}

/// A row whose checksum doesn't match: the index is damaged and is rebuilt.
#[derive(Debug)]
pub struct Damaged(&'static str);

impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "row checksum mismatch in the index's {} table", self.0)
    }
}

impl std::error::Error for Damaged {}

fn check(table: &'static str, fields: &[F], got: &[u8]) -> Result<()> {
    if ck(table, fields)[..] != *got {
        return Err(Damaged(table).into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// Before: the clock and the shards' stamps, taken before any listing

/// A shard directory's stamp: what changes when a file in it is added, renamed or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    mtime_s: i64,
    mtime_ns: i64,
    ctime_s: i64,
    ctime_ns: i64,
    ino: u64,
    dev: u64,
}

impl Stamp {
    fn mtime(&self) -> i128 {
        i128::from(self.mtime_s) * 1_000_000_000 + i128::from(self.mtime_ns)
    }
    fn ctime(&self) -> i128 {
        i128::from(self.ctime_s) * 1_000_000_000 + i128::from(self.ctime_ns)
    }
}

/// One shard as step 2 saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Absent,
    /// A directory, with its stamp if it could be taken.
    Present(Option<Stamp>),
}

/// Steps 1 and 2 (section 5): *T*, if this filesystem's stamps can be trusted, then every shard's state,
/// taken before anything lists `objects/`.
#[derive(Clone, Debug)]
pub struct Before {
    clock: Option<i128>,
    shards: Vec<Seen>,
}

impl Before {
    pub fn take(dir: &Path) -> Before {
        let clock = trusted_clock(dir);
        let shards = (0..=255u8).map(|n| seen(&dir.join("objects").join(format!("{n:02x}")))).collect();
        Before { clock, shards }
    }

    /// Stats only, no clock: for a refresh that records no stamps.
    pub fn stats(dir: &Path) -> Before {
        let shards = (0..=255u8).map(|n| seen(&dir.join("objects").join(format!("{n:02x}")))).collect();
        Before { clock: None, shards }
    }
}

/// *T*, the filesystem's clock, if stamps can be trusted here: Unix, `cache/clock` written, `cache/` and
/// `objects/` on one device, of an allow-listed local filesystem type.
fn trusted_clock(dir: &Path) -> Option<i128> {
    let cache = crate::snapshot::cache::Cache::open(dir, true).ok()??;
    let (t, dev) = cache.clock().ok()?;
    let objects = dir.join("objects");
    match stamp(&objects) {
        Some(s) if s.dev == dev && local_filesystem(&objects) => Some(t),
        // No objects/ yet: nothing to stamp but absences, which need no clock comparison.
        None if std::fs::symlink_metadata(&objects).is_err() && local_filesystem(dir) => Some(t),
        _ => None,
    }
}

#[cfg(unix)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path).ok().filter(|m| m.is_dir())?;
    Some(Stamp {
        mtime_s: m.mtime(),
        mtime_ns: m.mtime_nsec(),
        ctime_s: m.ctime(),
        ctime_ns: m.ctime_nsec(),
        ino: m.ino(),
        dev: m.dev(),
    })
}

#[cfg(not(unix))]
fn stamp(_: &Path) -> Option<Stamp> {
    None
}

fn seen(path: &Path) -> Seen {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Seen::Absent,
        _ => Seen::Present(stamp(path)),
    }
}

/// Whether `path` is on a filesystem type whose directory times this design trusts (section 5): APFS,
/// HFS+, ext4, xfs, btrfs, zfs, tmpfs.
pub fn local_filesystem(path: &Path) -> bool {
    filesystem_name(path).is_some_and(|name| allowed_filesystem(&name))
}

fn allowed_filesystem(name: &str) -> bool {
    matches!(name, "apfs" | "hfs" | "ext4" | "xfs" | "btrfs" | "zfs" | "tmpfs")
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn filesystem_name(path: &Path) -> Option<String> {
    let st = rustix::fs::statfs(path).ok()?;
    let bytes: Vec<u8> = st.f_fstypename.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8(bytes).ok()
}

#[cfg(target_os = "linux")]
fn filesystem_name(path: &Path) -> Option<String> {
    let st = rustix::fs::statfs(path).ok()?;
    #[allow(clippy::unnecessary_cast)]
    let name = match st.f_type as i64 {
        0xEF53 => "ext4",
        0x5846_5342 => "xfs",
        0x9123_683E => "btrfs",
        0x2FC1_2FC1 => "zfs",
        0x0102_1994 => "tmpfs",
        _ => return None,
    };
    Some(name.to_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux")))]
fn filesystem_name(_: &Path) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------------------------------
// Files and connections

/// Whether this process may use the index here (section 5, Opens that can't write): only where it can
/// write `cache/` (or create it), and only if `cache/` is a real directory, not a link to one (section 7,
/// Opening the file; finding 32fc7df1acf4).
pub fn usable(dir: &Path) -> bool {
    let cache = dir.join("cache");
    match std::fs::symlink_metadata(&cache) {
        Ok(m) => m.file_type().is_dir() && writable(&cache),
        Err(_) => writable(dir),
    }
}

#[cfg(unix)]
fn writable(path: &Path) -> bool {
    rustix::fs::access(path, rustix::fs::Access::WRITE_OK).is_ok()
}

#[cfg(not(unix))]
fn writable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}

fn file_name(generation: u64) -> String {
    format!("index-v{SCHEMA_VERSION}-{generation}.sqlite")
}

/// The highest generation of this schema version's files in `cache/`, if any.
fn current_generation(cache: &Path) -> Option<u64> {
    let prefix = format!("index-v{SCHEMA_VERSION}-");
    std::fs::read_dir(cache)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let g = name.strip_prefix(&prefix)?.strip_suffix(".sqlite")?;
            if g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()) || g.starts_with('0') {
                return None;
            }
            g.parse::<u64>().ok()
        })
        .max()
}

/// The canonical `cache/` (macOS's `/var` and `/tmp` are links, and `SQLITE_OPEN_NOFOLLOW` refuses a link
/// anywhere in the path), made first if missing, with its `.gitignore`, and refused if it's a link.
fn cache_dir(dir: &Path) -> Result<PathBuf> {
    crate::snapshot::cache::Cache::open(dir, true).context("opening cache/")?.context("no cache/ was made")?;
    let cache = std::fs::canonicalize(dir.join("cache")).context("resolving cache/")?;
    Ok(cache)
}

/// The index's current file, for tests.
#[cfg(test)]
pub fn path(dir: &Path) -> Result<PathBuf> {
    let cache = cache_dir(dir)?;
    Ok(cache.join(file_name(current_generation(&cache).unwrap_or(1))))
}

/// Opens the index file. Anything there that isn't a regular file (a directory, a link, a FIFO) is damage,
/// so the next generation is used (security review note 2). The connection distrusts the file's schema:
/// no SQL functions run from it, and defensive mode refuses writes that could corrupt it (note 5).
fn open_db(path: &Path, busy: Duration) -> Result<Connection> {
    if let Ok(m) = std::fs::symlink_metadata(path)
        && !m.file_type().is_file()
    {
        return Err(NotAFile.into());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let conn = Connection::open_with_flags(path, flags)?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    // No checkpoint when a command's connection closes, which would fsync the WAL and the file on every
    // command (note b33068a95bbf): SQLite's autocheckpoint, every 1,000 pages, does it. With
    // synchronous=NORMAL a power loss can only roll back recent commits whole, stamps with their rows,
    // so the index falls behind and freshness finds again what it lacks.
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    conn.busy_timeout(busy)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cell_size_check", "ON")?;
    Ok(conn)
}

/// SQLite saying the file isn't a sound database: a new generation (section 6, Rebuilds).
fn is_damage(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
            matches!(e.sqlite_error_code(), Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
        }) || c.downcast_ref::<WrongVersion>().is_some()
            || c.downcast_ref::<NotAFile>().is_some()
            || c.downcast_ref::<SchemaDamage>().is_some()
    })
}

/// A schema that couldn't be cleared or recreated: damage, so a new generation.
#[derive(Debug)]
struct SchemaDamage(String);

impl std::fmt::Display for SchemaDamage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the index file's schema can't be replaced: {}", self.0)
    }
}

impl std::error::Error for SchemaDamage {}

/// Something at the index's path that isn't a regular file: damage, so a new generation.
#[derive(Debug)]
struct NotAFile;

impl std::fmt::Display for NotAFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the index's path holds something that isn't a regular file")
    }
}

impl std::error::Error for NotAFile {}

/// A file whose `schema_version` disagrees with its name: damage, so a new generation.
#[derive(Debug)]
struct WrongVersion;

impl std::fmt::Display for WrongVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the index file's schema version doesn't match its name")
    }
}

impl std::error::Error for WrongVersion {}

// ---------------------------------------------------------------------------------------------------
// Refresh

/// Whether to record the stamps of `Before`. A write's own refresh doesn't: it runs after the write's
/// rename, and stamps taken then could cover other processes' objects that this store never listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Record {
    Yes,
    No,
}

/// What a refresh did, for tests and the benchmark.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub rebuilt: bool,
    pub new_generation: bool,
    pub dirty_shards: usize,
    pub added: usize,
    pub recorded: usize,
    pub synced: usize,
}

/// Knobs the tests turn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub racy_ns: i128,
    pub busy: Duration,
    /// Whether to make what a transaction names durable before it commits: false only for the full
    /// check's passes, which are rolled back or in memory.
    pub sync: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { racy_ns: RACY_NS, busy: BUSY, sync: true }
    }
}

/// The full check's options: nothing it builds is committed, so nothing is synced.
const CHECK: Options = Options { racy_ns: RACY_NS, busy: BUSY, sync: false };

/// Keeps the index current with `store`, which was opened after `before` was taken: built on first use,
/// rebuilt in place on a mismatch, moved to a new generation on damage, otherwise only dirty shards
/// reconciled. A failure (a lock not granted in time, say) leaves the index for a later command: the
/// caller reads through the full path either way.
pub fn refresh(dir: &Path, store: &Store, before: &Before, record: Record) -> Result<Outcome> {
    refresh_with(dir, store, before, record, Options::default())
}

/// A write's own refresh (no stamps recorded): `written` are the contributions this process's store
/// wrote, each synced by the store before its rename, so they aren't synced again (section 6, Order).
pub fn refresh_after_write(
    dir: &Path,
    store: &Store,
    before: &Before,
    written: &HashSet<ContributionId>,
) -> Result<Outcome> {
    refresh_inner(dir, store, before, Record::No, Options::default(), written)
}

pub(crate) fn refresh_with(
    dir: &Path,
    store: &Store,
    before: &Before,
    record: Record,
    opts: Options,
) -> Result<Outcome> {
    refresh_inner(dir, store, before, record, opts, &HashSet::new())
}

fn refresh_inner(
    dir: &Path,
    store: &Store,
    before: &Before,
    record: Record,
    opts: Options,
    written: &HashSet<ContributionId>,
) -> Result<Outcome> {
    let cache = cache_dir(dir)?;
    let generation = current_generation(&cache).unwrap_or(1);
    match refresh_file(dir, &cache.join(file_name(generation)), store, before, record, opts, written) {
        Err(e) if is_damage(&e) => {
            let next = cache.join(file_name(next_generation(generation)?));
            let mut outcome = refresh_file(dir, &next, store, before, record, opts, written)?;
            outcome.new_generation = true;
            Ok(outcome)
        }
        other => other,
    }
}

/// The generation after `g`; none after the last (security review note 3), and then the index isn't used.
fn next_generation(g: u64) -> Result<u64> {
    g.checked_add(1).context("no index generation is left after the last one")
}

fn refresh_file(
    dir: &Path,
    path: &Path,
    store: &Store,
    before: &Before,
    record: Record,
    opts: Options,
    written: &HashSet<ContributionId>,
) -> Result<Outcome> {
    let mut conn = open_db(path, opts.busy)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = match reconcile(&tx, dir, path, store, before, record, opts, written) {
        Ok(outcome) => outcome,
        Err(Needs::Rebuild | Needs::Damaged(_)) => rebuild_in(&tx, dir, path, store, before, record, opts)?,
        Err(Needs::Error(e)) => return Err(e),
    };
    tx.commit()?;
    truncate_wal(&conn, path);
    Ok(outcome)
}

/// The size past which a commit checkpoints the WAL into the file and truncates it (section 6,
/// Checkpoints).
const WAL_LIMIT: u64 = 256 * 1024;

/// After a commit that wrote, a WAL past [`WAL_LIMIT`] is checkpointed and truncated to nothing. Without a
/// checkpoint on close the WAL is never restarted, and every process's first connection rebuilds its index
/// from the whole file and counts none of it checkpointed, so it would grow without bound and each commit
/// past SQLite's autocheckpoint would copy all of it again, with an fsync of the WAL and of the file. Best
/// effort: the commit is already durable as `synchronous=NORMAL` makes it, and a checkpoint left undone
/// is done by a later commit.
fn truncate_wal(conn: &Connection, path: &Path) {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    if conn.total_changes() > 0 && std::fs::metadata(&wal).is_ok_and(|m| m.len() > WAL_LIMIT) {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
}

/// `bracken verify`: the index rebuilt whole from `store`, a full open made after `before` was taken,
/// then SQLite's `quick_check` (section 6, Integrity checks).
pub fn rebuild(dir: &Path, store: &Store, before: &Before) -> Result<()> {
    let cache = cache_dir(dir)?;
    let generation = current_generation(&cache).unwrap_or(1);
    let attempt = |path: &Path| -> Result<()> {
        let mut conn = open_db(path, BUSY)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        rebuild_in(&tx, dir, path, store, before, Record::Yes, Options::default())?;
        tx.commit()?;
        truncate_wal(&conn, path);
        let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            bail!("the index failed SQLite's quick_check after a rebuild: {integrity}");
        }
        Ok(())
    };
    match attempt(&cache.join(file_name(generation))) {
        Err(e) if is_damage(&e) => attempt(&cache.join(file_name(next_generation(generation)?))),
        other => other,
    }
}

enum Needs {
    /// A mismatch or a missing contribution: rebuild in place.
    Rebuild,
    /// A row whose checksum doesn't match: rebuild in place too, but the full check reports it.
    Damaged(anyhow::Error),
    Error(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Needs {
    fn from(e: E) -> Needs {
        let e = e.into();
        if e.downcast_ref::<Damaged>().is_some() { Needs::Damaged(e) } else { Needs::Error(e) }
    }
}

/// `meta` as read, every row's checksum checked.
fn read_meta(conn: &Connection) -> Result<HashMap<String, Vec<u8>>> {
    let mut st = conn.prepare("SELECT k, v, ck FROM meta")?;
    let mut out = HashMap::new();
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?)))? {
        let (k, v, c) = row?;
        check("meta", &[F::B(k.as_bytes()), F::B(&v)], &c)?;
        out.insert(k, v);
    }
    Ok(out)
}

fn put_meta(conn: &Connection, k: &str, v: &[u8]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES (?1, ?2, ?3)",
        params![k, v, &ck("meta", &[F::B(k.as_bytes()), F::B(v)])[..]],
    )?;
    Ok(())
}

fn has_schema(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n > 0)
}

/// The device and inode of the index file itself (section 7, Bound to its own file).
fn file_identity(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(m) = std::fs::symlink_metadata(path) {
            return [m.dev().to_be_bytes(), m.ino().to_be_bytes()].concat();
        }
    }
    let _ = path;
    Vec::new()
}

fn clock_bytes(t: i128) -> Vec<u8> {
    t.to_be_bytes().to_vec()
}

fn clock_of(v: &[u8]) -> Option<i128> {
    Some(i128::from_be_bytes(v.try_into().ok()?))
}

/// A shard row as recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Absent,
    Present(Stamp),
}

fn read_shards(conn: &Connection) -> Result<BTreeMap<u8, Row>> {
    let mut st = conn.prepare("SELECT n, present, mtime_s, mtime_ns, ctime_s, ctime_ns, ino, dev, ck FROM shard")?;
    let mut out = BTreeMap::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<Vec<u8>>>(6)?,
            r.get::<_, Option<Vec<u8>>>(7)?,
            r.get::<_, Vec<u8>>(8)?,
        ))
    })?;
    for row in rows {
        let (n, present, ms, mn, cs, cn, ino, dev, c) = row?;
        fn opt(x: Option<i64>) -> F<'static> {
            x.map_or(F::N, F::I)
        }
        fn optb(x: &Option<Vec<u8>>) -> F<'_> {
            x.as_deref().map_or(F::N, F::B)
        }
        check("shard", &[F::I(n), F::I(present), opt(ms), opt(mn), opt(cs), opt(cn), optb(&ino), optb(&dev)], &c)?;
        let n = u8::try_from(n).map_err(|_| Damaged("shard"))?;
        let row = match (present, ms, mn, cs, cn, ino, dev) {
            (0, ..) => Row::Absent,
            (1, Some(mtime_s), Some(mtime_ns), Some(ctime_s), Some(ctime_ns), Some(ino), Some(dev)) => {
                let u =
                    |b: Vec<u8>| -> Result<u64> { Ok(u64::from_be_bytes(b.try_into().map_err(|_| Damaged("shard"))?)) };
                Row::Present(Stamp { mtime_s, mtime_ns, ctime_s, ctime_ns, ino: u(ino)?, dev: u(dev)? })
            }
            _ => return Err(Damaged("shard").into()),
        };
        out.insert(n, row);
    }
    Ok(out)
}

fn put_shard(conn: &Connection, n: u8, row: Row) -> rusqlite::Result<()> {
    let n = i64::from(n);
    match row {
        Row::Absent => conn.execute(
            "INSERT OR REPLACE INTO shard VALUES (?1, 0, NULL, NULL, NULL, NULL, NULL, NULL, ?2)",
            params![n, &ck("shard", &[F::I(n), F::I(0), F::N, F::N, F::N, F::N, F::N, F::N])[..]],
        )?,
        Row::Present(s) => {
            let (ino, dev) = (s.ino.to_be_bytes(), s.dev.to_be_bytes());
            let c = ck(
                "shard",
                &[
                    F::I(n),
                    F::I(1),
                    F::I(s.mtime_s),
                    F::I(s.mtime_ns),
                    F::I(s.ctime_s),
                    F::I(s.ctime_ns),
                    F::B(&ino),
                    F::B(&dev),
                ],
            );
            conn.execute(
                "INSERT OR REPLACE INTO shard VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![n, s.mtime_s, s.mtime_ns, s.ctime_s, s.ctime_ns, &ino[..], &dev[..], &c[..]],
            )?
        }
    };
    Ok(())
}

/// An `obj` row, checked.
#[derive(Clone, Debug)]
struct ObjRow {
    seq: i64,
    pos: i64,
    id: [u8; 32],
    kind: i64,
    author: i64,
    ts_key: i64,
}

impl ObjRow {
    fn ck(&self) -> [u8; 8] {
        ck(
            "obj",
            &[F::I(self.seq), F::I(self.pos), F::B(&self.id), F::I(self.kind), F::I(self.author), F::I(self.ts_key)],
        )
    }
}

const OBJ_COLUMNS: &str = "seq, pos, id, kind, author, ts_key, ck";

fn obj_row(r: &rusqlite::Row) -> rusqlite::Result<(ObjRow, Vec<u8>)> {
    let id: Vec<u8> = r.get(2)?;
    let id = <[u8; 32]>::try_from(id)
        .map_err(|_| rusqlite::Error::InvalidColumnType(2, "id".into(), rusqlite::types::Type::Blob))?;
    Ok((
        ObjRow { seq: r.get(0)?, pos: r.get(1)?, id, kind: r.get(3)?, author: r.get(4)?, ts_key: r.get(5)? },
        r.get(6)?,
    ))
}

fn checked(row: (ObjRow, Vec<u8>)) -> Result<ObjRow> {
    if row.0.ck()[..] != *row.1 {
        return Err(Damaged("obj").into());
    }
    Ok(row.0)
}

/// The indexed ids whose first byte is `n`, each row checked, its id checked against the range asked for.
fn ids_in_shard(conn: &Connection, n: u8) -> Result<HashSet<ContributionId>> {
    let lo = [n];
    let rows: Vec<(ObjRow, Vec<u8>)> = if n == u8::MAX {
        let mut st = conn.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id >= ?1"))?;
        st.query_map([&lo[..]], obj_row)?.collect::<rusqlite::Result<_>>()?
    } else {
        let hi = [n + 1];
        let mut st = conn.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id >= ?1 AND id < ?2"))?;
        st.query_map([&lo[..], &hi[..]], obj_row)?.collect::<rusqlite::Result<_>>()?
    };
    let mut out = HashSet::new();
    for row in rows {
        let row = checked(row)?;
        if row.id[0] != n {
            return Err(Damaged("obj").into());
        }
        out.insert(ContributionId(row.id));
    }
    Ok(out)
}

fn object_path(dir: &Path, id: &ContributionId) -> PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

fn object_exists(dir: &Path, id: &ContributionId) -> bool {
    std::fs::symlink_metadata(object_path(dir, id)).is_ok_and(|m| m.is_file())
}

/// Brings an index of the right version, ground and file up to date (section 5 steps 3-6), or says it
/// needs a rebuild.
#[allow(clippy::too_many_arguments)]
fn reconcile(
    tx: &Connection,
    dir: &Path,
    path: &Path,
    store: &Store,
    before: &Before,
    record: Record,
    opts: Options,
    written: &HashSet<ContributionId>,
) -> Result<Outcome, Needs> {
    if !has_schema(tx)? {
        return Err(Needs::Rebuild);
    }
    let meta = read_meta(tx)?;
    if meta.get("schema_version").map(Vec::as_slice) != Some(&SCHEMA_VERSION.to_le_bytes()[..]) {
        return Err(Needs::Error(WrongVersion.into()));
    }
    if meta.get("ground").map(Vec::as_slice) != Some(&store.info().ground.as_bytes()[..])
        || meta.get("file").map(Vec::as_slice) != Some(&file_identity(path)[..])
    {
        return Err(Needs::Rebuild);
    }
    let recorded_clock = meta.get("fs_clock").and_then(|v| clock_of(v));
    let (Some(key), Some(mut indexed)) = (meta32(&meta, "key"), meta32(&meta, "ids")) else {
        return Err(Needs::Rebuild);
    };
    let mut rows = read_shards(tx)?;

    // Step 3: a clock that stepped back makes every shard dirty, before any stamp is compared. A later
    // clock recorded by a concurrent refresh isn't a step back: the clock is read again, under the lock.
    let mut stepped_back = false;
    if let (Some(t), Some(f)) = (before.clock, recorded_clock)
        && t < f
    {
        let now = trusted_clock(dir);
        if now.is_none_or(|now| now < f) {
            stepped_back = true;
            tx.execute("DELETE FROM shard", [])?;
            rows.clear();
        }
    }
    // No trusted clock (section 5 step 1): no stamp can be judged, so every shard is dirty and every old
    // stamp goes, as after a step back (finding e138686585be). A write's own refresh records nothing and
    // never has a clock, so it keeps them.
    if record == Record::Yes && before.clock.is_none() {
        stepped_back = true;
        tx.execute("DELETE FROM shard", [])?;
        rows.clear();
    }
    let dirty: Vec<u8> = (0..=255u8)
        .filter(|n| {
            let clean = match (rows.get(n), before.shards[usize::from(*n)]) {
                (Some(Row::Absent), Seen::Absent) => true,
                (Some(Row::Present(r)), Seen::Present(Some(s))) => *r == s,
                _ => false,
            };
            stepped_back || !clean
        })
        .collect();

    // Step 4: diff each dirty shard against the store's ids (the store listed and checked every object,
    // after `before`).
    let g = store.graph();
    let mut added: Vec<ContributionId> = Vec::new();
    let mut added_shards: BTreeSet<u8> = BTreeSet::new();
    // Shards the index holds nothing in: only their entries in objects/ may not be durable yet (every
    // commit that indexed a shard's first object synced objects/).
    let mut new_shards: BTreeSet<u8> = BTreeSet::new();
    if !dirty.is_empty() {
        let wanted: HashSet<u8> = dirty.iter().copied().collect();
        let mut in_graph: HashMap<u8, HashSet<ContributionId>> = HashMap::new();
        for c in g.iter() {
            let n = c.id().as_bytes()[0];
            if wanted.contains(&n) {
                in_graph.entry(n).or_default().insert(c.id());
            }
        }
        let empty = HashSet::new();
        for n in &dirty {
            let indexed = ids_in_shard(tx, *n)?;
            let listed = in_graph.get(n).unwrap_or(&empty);
            for id in &indexed {
                // Indexed but not in this store: newer than this store's listing if its object is there,
                // gone (a checkout that went back, a shard removed whole) if not.
                if !listed.contains(id) && !object_exists(dir, id) {
                    return Err(Needs::Rebuild);
                }
            }
            let new: Vec<ContributionId> = listed.iter().filter(|id| !indexed.contains(id)).copied().collect();
            if !new.is_empty() {
                added_shards.insert(*n);
                if indexed.is_empty() {
                    new_shards.insert(*n);
                }
            }
            added.extend(new);
        }
    }

    let mut outcome = Outcome { dirty_shards: dirty.len(), added: added.len(), ..Outcome::default() };

    // Step 5: what freshness found is made durable before it's committed, on every path, a write's own
    // refresh included: what it adds may be another writer's, renamed in without a sync (security review
    // note 1). Only the objects this process's store wrote, and so synced before their rename, aren't
    // synced again.
    let mut to_sync: Vec<PathBuf> =
        added.iter().filter(|id| !written.contains(id)).map(|id| object_path(dir, id)).collect();
    for n in &added_shards {
        to_sync.push(dir.join("objects").join(format!("{n:02x}")));
    }
    if !new_shards.is_empty() {
        to_sync.push(dir.join("objects"));
    }

    if !added.is_empty() {
        let wanted: HashSet<ContributionId> = added.iter().copied().collect();
        let order = g.topo_order();
        let mut ins = Inserter::new(tx, key)?;
        for id in order.iter().filter(|id| wanted.contains(id)) {
            if !ins.insert(g.get(id).expect("ordered ids exist"))? {
                return Err(Needs::Rebuild);
            }
        }
        indexed = xor(indexed, ins.digest);
        put_meta(tx, "ids", &indexed)?;
        put_meta(tx, "derived_stale", &[1])?;
    }
    // The derived tables (section 4): only what `added` can change, unless an earlier refresh left them
    // stale, which may cover contributions this one didn't add. Only when the index holds exactly what
    // the store holds: another process may have indexed what this store never listed, while this one
    // holds what that one didn't (finding cf0181613a78).
    let was_stale = meta.get("derived_stale").is_some_and(|v| v != &[0]);
    if (was_stale || !added.is_empty()) && count(tx)? == g.len() as i64 && indexed == store_digest(&key, store) {
        let order = g.topo_order();
        let after = if was_stale { -1 } else { suffix_start(tx, store, &added)? };
        write_order(tx, &order, after)?;
        if was_stale || added.iter().any(|id| can_hide(g.get(id).expect("added ids are in the graph"))) {
            write_hidden(tx, store, &order)?;
        } else {
            hide_added(tx, &order, store, &added)?;
        }
        put_meta(tx, "derived_stale", &[0])?;
    }

    // Step 6: stamps, recorded only from `before`, only if older than T by the margin, only once their
    // directories are synced. A write's refresh records nothing and leaves its shards without a row.
    match record {
        Record::Yes => {
            // Only rows that are there: a refresh that records and adds nothing writes nothing.
            for n in dirty.iter().filter(|n| rows.contains_key(n)) {
                tx.execute("DELETE FROM shard WHERE n = ?1", [i64::from(*n)])?;
            }
            if let Some(t) = before.clock {
                for n in &dirty {
                    match before.shards[usize::from(*n)] {
                        Seen::Absent => {
                            put_shard(tx, *n, Row::Absent)?;
                            outcome.recorded += 1;
                        }
                        Seen::Present(Some(s)) if s.mtime() < t - opts.racy_ns && s.ctime() < t - opts.racy_ns => {
                            put_shard(tx, *n, Row::Present(s))?;
                            to_sync.push(dir.join("objects").join(format!("{n:02x}")));
                            outcome.recorded += 1;
                        }
                        Seen::Present(_) => {}
                    }
                }
                // fs_clock is recorded with the stamps it judged (section 5 step 6), and after a step
                // back. A refresh that records none leaves the last one, which every stamp recorded is
                // more than the margin older than, so a later step back below it is still caught.
                if outcome.recorded > 0 || stepped_back {
                    let keep = recorded_clock.filter(|f| !stepped_back && *f > t).unwrap_or(t);
                    put_meta(tx, "fs_clock", &clock_bytes(keep))?;
                }
            }
        }
        Record::No => {
            for n in &added_shards {
                tx.execute("DELETE FROM shard WHERE n = ?1", [i64::from(*n)])?;
            }
        }
    }
    if opts.sync {
        outcome.synced = durable(&to_sync).map_err(|e| Needs::Error(e.context("syncing objects/")))?;
    }
    Ok(outcome)
}

/// Makes `paths` (object files and directories under `objects/`) durable: `fsync` on each, then on
/// macOS one `F_FULLFSYNC`, which flushes the drive's cache for everything before it. Returns how many
/// were synced.
fn durable(paths: &[PathBuf]) -> Result<usize> {
    let unique: BTreeSet<&PathBuf> = paths.iter().collect();
    #[cfg(test)]
    tests::SYNCED.with(|s| s.borrow_mut().extend(unique.iter().map(|p| (*p).clone())));
    #[cfg(test)]
    if tests::FAIL_SYNC.with(std::cell::Cell::get) {
        bail!("a sync that fails, for a test");
    }
    #[cfg(test)]
    let real = tests::REAL_SYNC.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let real = true;
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};
        let mut last = None;
        for p in &unique {
            // Nonblocking, so a FIFO put there can't hang the command, and then only a file or a directory
            // (security review note 4).
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = rustix::fs::open(*p, flags, Mode::empty())
                .with_context(|| format!("opening {} to sync it", p.display()))?;
            let kind = rustix::fs::FileType::from_raw_mode(rustix::fs::fstat(&fd)?.st_mode);
            if !matches!(kind, rustix::fs::FileType::RegularFile | rustix::fs::FileType::Directory) {
                bail!("{} is neither a file nor a directory, so it isn't synced or indexed", p.display());
            }
            if real {
                rustix::fs::fsync(&fd).with_context(|| format!("syncing {}", p.display()))?;
            }
            last = Some(fd);
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        if real && let Some(fd) = &last {
            rustix::fs::fcntl_fullfsync(fd).context("F_FULLFSYNC")?;
        }
        let _ = last;
    }
    #[cfg(not(unix))]
    let _ = real;
    Ok(unique.len())
}

/// The set of `ids` as one value, which a count alone doesn't give (finding cf0181613a78): the XOR of each
/// id's BLAKE3 hash keyed with this index's own random `key` (`meta`'s `key`, made by each rebuild). Ids
/// are unique, and ids arrive from others, who could choose some whose plain XOR collides (about 256 solve
/// it over GF(2)); keyed with a key they never see, two sets of one size agree only by chance.
fn digest<'a>(key: &[u8; 32], ids: impl IntoIterator<Item = &'a [u8; 32]>) -> [u8; 32] {
    let mut d = [0u8; 32];
    for id in ids {
        d = xor(d, *blake3::keyed_hash(key, id).as_bytes());
    }
    d
}

fn xor(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    let mut d = a;
    for (x, y) in d.iter_mut().zip(b) {
        *x ^= y;
    }
    d
}

/// The digest of what `store` holds, with `key`.
fn store_digest(key: &[u8; 32], store: &Store) -> [u8; 32] {
    let ids: Vec<[u8; 32]> = store.graph().iter().map(|c| *c.id().as_bytes()).collect();
    digest(key, &ids)
}

/// `meta`'s 32-byte value `k`, if it has one.
fn meta32(meta: &HashMap<String, Vec<u8>>, k: &str) -> Option<[u8; 32]> {
    meta.get(k).and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
}

fn count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT coalesce(max(seq) + 1, 0) FROM obj", [], |r| r.get(0))
}

/// Empties the index and fills it from `store` inside the caller's transaction (section 6, Rebuilds):
/// concurrent readers see the old index or the new one. Stamps are recorded from `before`, as step 6 says;
/// what a rebuild syncs is the shard directories whose stamps it records, not every object file.
fn rebuild_in(
    tx: &Connection,
    dir: &Path,
    path: &Path,
    store: &Store,
    before: &Before,
    record: Record,
    opts: Options,
) -> Result<Outcome> {
    // A schema that can't be cleared or made (a virtual table of a module this build lacks, say) is damage:
    // the next generation is used (finding a713038d77ae).
    clear_schema(tx).and_then(|()| Ok(tx.execute_batch(SCHEMA)?)).map_err(|e| SchemaDamage(format!("{e:#}")))?;
    let g = store.graph();
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("no random key for the index: {e}"))?;
    put_meta(tx, "key", &key)?;
    {
        let mut ins = Inserter::new(tx, key)?;
        for id in g.topo_order() {
            let inserted = ins.insert(g.get(&id).expect("ordered ids exist"))?;
            assert!(inserted, "topological order puts parents first");
        }
        put_meta(tx, "ids", &ins.digest)?;
    }
    write_hidden(tx, store, &g.topo_order())?;
    put_meta(tx, "schema_version", &SCHEMA_VERSION.to_le_bytes())?;
    put_meta(tx, "ground", store.info().ground.as_bytes())?;
    put_meta(tx, "file", &file_identity(path))?;
    put_meta(tx, "derived_stale", &[0])?;
    let mut outcome = Outcome { rebuilt: true, dirty_shards: 256, added: g.len(), ..Outcome::default() };
    // Every object the rebuild names, its shard directory and objects/, before the commit (answer
    // ddaf02041077): the store doesn't sync directories, so this commit is where they become durable.
    let mut to_sync: Vec<PathBuf> = g.iter().map(|c| object_path(dir, &c.id())).collect();
    let shards: BTreeSet<u8> = g.iter().map(|c| c.id().as_bytes()[0]).collect();
    to_sync.extend(shards.iter().map(|n| dir.join("objects").join(format!("{n:02x}"))));
    to_sync.push(dir.join("objects"));
    if record == Record::Yes
        && let Some(t) = before.clock
    {
        for n in 0..=255u8 {
            match before.shards[usize::from(n)] {
                Seen::Absent => {
                    put_shard(tx, n, Row::Absent)?;
                    outcome.recorded += 1;
                }
                Seen::Present(Some(s)) if s.mtime() < t - opts.racy_ns && s.ctime() < t - opts.racy_ns => {
                    put_shard(tx, n, Row::Present(s))?;
                    to_sync.push(dir.join("objects").join(format!("{n:02x}")));
                    outcome.recorded += 1;
                }
                Seen::Present(_) => {}
            }
        }
        put_meta(tx, "fs_clock", &clock_bytes(t))?;
    }
    if opts.sync && dir.join("objects").exists() {
        outcome.synced = durable(&to_sync)?;
    }
    Ok(outcome)
}

/// The last position that inserting `added` can't move (section 4, Order, Insert): Kahn's state after
/// emitting position *m* depends only on what was emitted, and none of `added` is ready before all its
/// parents are, so *m* is the smallest, over those of `added` whose parents are all already indexed, of
/// their parents' largest position (-1 for a root, which moves everything).
fn suffix_start(tx: &Connection, store: &Store, added: &[ContributionId]) -> Result<i64> {
    let g = store.graph();
    let new: HashSet<&ContributionId> = added.iter().collect();
    let mut st = tx.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id = ?1"))?;
    let mut m = i64::MAX;
    for id in added {
        let parents = g.get(id).expect("added ids are in the graph").parents();
        if parents.iter().any(|p| new.contains(p)) {
            continue;
        }
        let mut top = -1;
        for p in parents {
            top = top.max(checked(st.query_row([&p.as_bytes()[..]], obj_row)?)?.pos);
        }
        m = m.min(top);
    }
    Ok(if m == i64::MAX { -1 } else { m })
}

/// Drops every trigger, view, index and table the file holds, ours or not, so a rebuild starts from an
/// empty schema whatever was put there (security review note 2): `DROP TABLE` refuses a view, and a
/// trigger left behind would run on our writes.
fn clear_schema(tx: &Connection) -> Result<()> {
    let objects: Vec<(String, String)> = {
        let mut st = tx.prepare(
            "SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY CASE type WHEN 'trigger' THEN 0 WHEN 'view' THEN 1 WHEN 'index' THEN 2 ELSE 3 END",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (kind, name) in objects {
        let kind = match kind.as_str() {
            "trigger" => "TRIGGER",
            "view" => "VIEW",
            "index" => "INDEX",
            "table" => "TABLE",
            other => bail!("the index file holds a schema object of an unknown type: {other}"),
        };
        let quoted = name.replace('"', "\"\"");
        tx.execute_batch(&format!("DROP {kind} IF EXISTS \"{quoted}\""))?;
    }
    Ok(())
}

/// Sets the `pos` of every row after position `after` to its place in `order`, the graph's
/// `topo_order()` (section 4, Order); rows at or before it are already there. Only rows whose place
/// changed are written, in two steps, since `pos` is unique and SQLite checks it row by row.
fn write_order(tx: &Connection, order: &[ContributionId], after: i64) -> Result<()> {
    let first = usize::try_from(after + 1).expect("after is -1 or a position");
    let want: HashMap<[u8; 32], i64> =
        order.iter().enumerate().skip(first).map(|(i, id)| (*id.as_bytes(), i as i64)).collect();
    let rows: Vec<ObjRow> = {
        let mut st = tx.prepare(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE pos > ?1"))?;
        let raw: Vec<(ObjRow, Vec<u8>)> = st.query_map([after], obj_row)?.collect::<rusqlite::Result<_>>()?;
        raw.into_iter().map(checked).collect::<Result<_>>()?
    };
    let moved: Vec<(ObjRow, i64)> = rows
        .into_iter()
        .map(|r| {
            let to = *want.get(&r.id).context("the index holds a contribution the store doesn't")?;
            Ok((r.pos != to).then_some((r, to)))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()?;
    for (r, _) in &moved {
        tx.execute("UPDATE obj SET pos = ?1 WHERE seq = ?2", [-1 - r.seq, r.seq])?;
    }
    for (r, to) in &moved {
        let row = ObjRow { pos: *to, ..r.clone() };
        tx.execute("UPDATE obj SET pos = ?1, ck = ?2 WHERE seq = ?3", params![*to, &row.ck()[..], r.seq])?;
    }
    Ok(())
}

/// Whether adding `c` can change whether any other contribution is hidden (section 4): only a retraction
/// or a supersession can, since [`crate::CurrentView`] hides what is retracted, a subject with a
/// supersession that counts, and what refers to anything hidden.
fn can_hide(c: &Contribution) -> bool {
    matches!(c.body(), Body::Retract { .. } | Body::Relation { kind: RelationKind::Supersedes, .. })
}

/// Adds to the hidden set those of `added` that are hidden, when none of them [`can_hide`] anything
/// else: then nothing already indexed changes, and none of `added` is retracted or superseded (what
/// would do it is a retraction or a supersession, which would be new too), so each is hidden exactly
/// when something it refers to is. `order` is causal, so references are decided first.
fn hide_added(tx: &Connection, order: &[ContributionId], store: &Store, added: &[ContributionId]) -> Result<()> {
    let g = store.graph();
    let new: HashSet<&ContributionId> = added.iter().collect();
    let mut decided: HashMap<ContributionId, bool> = HashMap::new();
    let mut by_id = tx.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id = ?1"))?;
    let mut hidden = tx.prepare_cached("SELECT seq, ck FROM hidden WHERE seq = ?1")?;
    for id in order.iter().filter(|id| new.contains(id)) {
        let mut is_hidden = false;
        for r in g.get(id).expect("ordered ids exist").body().references() {
            is_hidden = match decided.get(&r) {
                Some(h) => *h,
                None => {
                    let seq = checked(by_id.query_row([&r.as_bytes()[..]], obj_row)?)?.seq;
                    match hidden
                        .query_row([seq], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
                        .optional()?
                    {
                        Some((s, c)) => {
                            check("hidden", &[F::I(s)], &c)?;
                            true
                        }
                        None => false,
                    }
                }
            };
            if is_hidden {
                break;
            }
        }
        if is_hidden {
            let seq = checked(by_id.query_row([&id.as_bytes()[..]], obj_row)?)?.seq;
            tx.execute("INSERT INTO hidden VALUES (?1, ?2)", params![seq, &ck("hidden", &[F::I(seq)])[..]])?;
        }
        decided.insert(*id, is_hidden);
    }
    Ok(())
}

/// Replaces the hidden set with [`crate::CurrentView`]'s over `store`, `order` being its `topo_order()`.
fn write_hidden(tx: &Connection, store: &Store, order: &[ContributionId]) -> Result<()> {
    let g = store.graph();
    let view = crate::CurrentView::new(g, order);
    let mut want: BTreeSet<i64> = BTreeSet::new();
    let mut seq = tx.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id = ?1"))?;
    for id in &view.hidden {
        let row = checked(seq.query_row([&id.as_bytes()[..]], obj_row)?)?;
        want.insert(row.seq);
    }
    let mut have: BTreeSet<i64> = BTreeSet::new();
    {
        let mut st = tx.prepare("SELECT seq, ck FROM hidden")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
            let (s, c) = row?;
            check("hidden", &[F::I(s)], &c)?;
            have.insert(s);
        }
    }
    for s in have.difference(&want) {
        tx.execute("DELETE FROM hidden WHERE seq = ?1", [s])?;
    }
    for s in want.difference(&have) {
        tx.execute("INSERT INTO hidden VALUES (?1, ?2)", params![s, &ck("hidden", &[F::I(*s)])[..]])?;
    }
    Ok(())
}

/// Inserts contributions, parents first. A row's `pos` is the next free one; [`write_order`] then puts
/// every row at its place in `topo_order()`.
struct Inserter<'t> {
    tx: &'t Connection,
    next_seq: i64,
    next_pos: i64,
    /// The index's digest key, and the [`digest`] of what this inserter added.
    key: [u8; 32],
    digest: [u8; 32],
}

impl<'t> Inserter<'t> {
    fn new(tx: &'t Connection, key: [u8; 32]) -> rusqlite::Result<Inserter<'t>> {
        let next_pos = tx.query_row("SELECT coalesce(max(pos) + 1, 0) FROM obj", [], |r| r.get(0))?;
        Ok(Inserter { tx, next_seq: count(tx)?, next_pos, key, digest: [0; 32] })
    }

    fn seq_of(&self, id: &ContributionId) -> Result<Option<i64>> {
        let mut st = self.tx.prepare_cached(&format!("SELECT {OBJ_COLUMNS} FROM obj WHERE id = ?1"))?;
        match st.query_row([&id.as_bytes()[..]], obj_row).optional()? {
            Some(row) => {
                let row = checked(row)?;
                if row.id != *id.as_bytes() {
                    return Err(Damaged("obj").into());
                }
                Ok(Some(row.seq))
            }
            None => Ok(None),
        }
    }

    fn agent(&self, key: &[u8]) -> Result<i64> {
        let tx = self.tx;
        let found: Option<(i64, Vec<u8>, Vec<u8>)> = tx
            .prepare_cached("SELECT n, key, ck FROM agent WHERE key = ?1")?
            .query_row([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((n, k, c)) = found {
            check("agent", &[F::I(n), F::B(&k)], &c)?;
            return Ok(n);
        }
        let n: i64 = tx.query_row("SELECT coalesce(max(n) + 1, 0) FROM agent", [], |r| r.get(0))?;
        tx.prepare_cached("INSERT INTO agent VALUES (?1, ?2, ?3)")?.execute(params![
            n,
            key,
            &ck("agent", &[F::I(n), F::B(key)])[..]
        ])?;
        Ok(n)
    }

    /// Adds `c`, unless it's already indexed (then a no-op, `true`); `false`, with nothing written, if a
    /// parent or a referenced contribution isn't indexed.
    fn insert(&mut self, c: &Contribution) -> Result<bool> {
        if self.seq_of(&c.id())?.is_some() {
            return Ok(true);
        }
        let mut parents = Vec::with_capacity(c.parents().len());
        for p in c.parents() {
            match self.seq_of(p)? {
                Some(s) => parents.push(s),
                None => return Ok(false),
            }
        }
        let mut refs = Vec::new();
        for (target, rkind) in refs_of(c) {
            match self.seq_of(&target)? {
                Some(s) => refs.push((s, rkind)),
                None => return Ok(false),
            }
        }
        let author = self.agent(c.author().as_bytes())?;
        let tx = self.tx;
        let row = ObjRow {
            seq: self.next_seq,
            pos: self.next_pos,
            id: *c.id().as_bytes(),
            kind: kind_code(c.body().kind()),
            author,
            ts_key: ts_key(c.content().timestamp_ms),
        };
        let seq = row.seq;
        self.digest = xor(self.digest, digest(&self.key, [&row.id]));
        tx.prepare_cached("INSERT INTO obj VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?.execute(params![
            row.seq,
            row.pos,
            &row.id[..],
            row.kind,
            row.author,
            row.ts_key,
            &row.ck()[..]
        ])?;
        for p in parents {
            tx.prepare_cached("INSERT OR IGNORE INTO parent VALUES (?1, ?2, ?3)")?.execute(params![
                seq,
                p,
                &ck("parent", &[F::I(seq), F::I(p)])[..]
            ])?;
            tx.prepare_cached("INSERT OR IGNORE INTO child VALUES (?1, ?2, ?3)")?.execute(params![
                p,
                seq,
                &ck("child", &[F::I(p), F::I(seq)])[..]
            ])?;
            tx.prepare_cached("DELETE FROM head WHERE seq = ?1")?.execute([p])?;
        }
        tx.prepare_cached("INSERT INTO head VALUES (?1, ?2)")?.execute(params![seq, &ck("head", &[F::I(seq)])[..]])?;
        if let Body::Claim { tags, .. } = c.body() {
            for t in tags {
                tx.prepare_cached("INSERT OR IGNORE INTO tag VALUES (?1, ?2, ?3)")?.execute(params![
                    t,
                    seq,
                    &ck("tag", &[F::B(t.as_bytes()), F::I(seq)])[..]
                ])?;
            }
        }
        for (target, rkind) in &refs {
            tx.prepare_cached("INSERT OR IGNORE INTO ref VALUES (?1, ?2, ?3, ?4)")?.execute(params![
                *target,
                *rkind,
                seq,
                &ck("ref", &[F::I(*target), F::I(*rkind), F::I(seq)])[..]
            ])?;
        }
        if let Body::Relation { kind, .. } = c.body() {
            let (to, from) = (refs[0].0, refs[1].0);
            let k = relation_code(*kind);
            tx.prepare_cached("INSERT OR IGNORE INTO fwd VALUES (?1, ?2, ?3, ?4, ?5)")?.execute(params![
                from,
                k,
                seq,
                to,
                &ck("fwd", &[F::I(from), F::I(k), F::I(seq), F::I(to)])[..]
            ])?;
        }
        self.next_seq += 1;
        self.next_pos += 1;
        Ok(true)
    }
}

// ---------------------------------------------------------------------------------------------------
// The oracle

type Id = [u8; 32];

/// Everything the index answers, as id-keyed rows, so two derivations can be compared.
#[derive(Debug, Default, PartialEq, Eq)]
struct Answers {
    order: Vec<[u8; 32]>,
    objects: BTreeSet<([u8; 32], i64, [u8; 32], i64)>,
    parents: BTreeSet<([u8; 32], [u8; 32])>,
    children: BTreeSet<([u8; 32], [u8; 32])>,
    heads: BTreeSet<[u8; 32]>,
    tags: BTreeSet<(String, [u8; 32])>,
    refs: BTreeSet<([u8; 32], i64, [u8; 32])>,
    forward: BTreeSet<(Id, i64, Id, Id)>,
    hidden: BTreeSet<[u8; 32]>,
}

/// The answers recomputed from the store, through the graph's own maps (referrers, children, heads)
/// rather than the bodies the index was filled from; the order from `topo_order()`, the hidden set from
/// `CurrentView`.
fn expected(store: &Store) -> Answers {
    let g = store.graph();
    let mut a = Answers::default();
    let order = g.topo_order();
    a.order = order.iter().map(|id| *id.as_bytes()).collect();
    for c in g.iter() {
        let id = *c.id().as_bytes();
        a.objects.insert((id, kind_code(c.body().kind()), *c.author().as_bytes(), ts_key(c.content().timestamp_ms)));
        for p in c.parents() {
            a.parents.insert((id, *p.as_bytes()));
        }
        for ch in g.children(&c.id()) {
            a.children.insert((id, *ch.as_bytes()));
        }
        if let Body::Claim { tags, .. } = c.body() {
            for t in tags {
                a.tags.insert((t.clone(), id));
            }
        }
        for r in g.referrers(&c.id()) {
            let rkind = match r.body() {
                Body::Relation { kind, .. } => relation_code(*kind),
                Body::Evidence { verdict, .. } => evidence_code(*verdict),
                Body::Retract { .. } => RETRACT_CODE,
                Body::Claim { .. } | Body::Artifact { .. } => continue,
            };
            a.refs.insert((id, rkind, *r.id().as_bytes()));
            if let Body::Relation { from, kind, to, .. } = r.body()
                && from == &c.id()
            {
                a.forward.insert((id, relation_code(*kind), *r.id().as_bytes(), *to.as_bytes()));
            }
        }
    }
    a.heads = g.heads().map(|h| *h.as_bytes()).collect();
    a.hidden = crate::CurrentView::new(g, &order).hidden.iter().map(|h| *h.as_bytes()).collect();
    a
}

/// What an index says, every row of every table read and its checksum checked; also checks that seqs
/// are dense and that every parent edge runs from a lower seq.
fn actual(conn: &Connection) -> Result<Answers> {
    let mut a = Answers::default();
    let mut by_seq: HashMap<i64, [u8; 32]> = HashMap::new();
    let mut agents: HashMap<i64, [u8; 32]> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT n, key, ck FROM agent")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?)))? {
            let (n, k, c) = row?;
            check("agent", &[F::I(n), F::B(&k)], &c)?;
            agents.insert(n, k.try_into().map_err(|_| Damaged("agent"))?);
        }
    }
    let mut objs: Vec<ObjRow> = {
        let mut st = conn.prepare(&format!("SELECT {OBJ_COLUMNS} FROM obj"))?;
        let raw: Vec<(ObjRow, Vec<u8>)> = st.query_map([], obj_row)?.collect::<rusqlite::Result<_>>()?;
        raw.into_iter().map(checked).collect::<Result<_>>()?
    };
    objs.sort_by_key(|r| r.pos);
    for (i, r) in objs.iter().enumerate() {
        if r.pos != i as i64 {
            bail!("the index's positions aren't 0..{}: {} at {i}", objs.len(), r.pos);
        }
        let author = agents.get(&r.author).context("an obj row names an agent that isn't there")?;
        a.objects.insert((r.id, r.kind, *author, r.ts_key));
        a.order.push(r.id);
        by_seq.insert(r.seq, r.id);
    }
    let mut seqs: Vec<i64> = by_seq.keys().copied().collect();
    seqs.sort_unstable();
    if seqs.iter().enumerate().any(|(i, s)| *s != i as i64) {
        bail!("the index's seqs aren't dense");
    }
    let meta = read_meta(conn)?;
    let key = meta32(&meta, "key").context("the index has no digest key")?;
    if meta32(&meta, "ids") != Some(digest(&key, by_seq.values())) {
        bail!("the index's ids digest doesn't match its obj rows");
    }
    let id = |s: i64| -> Result<[u8; 32]> { by_seq.get(&s).copied().context("a row names a seq that isn't there") };
    let pairs = |table: &'static str| -> Result<Vec<(i64, i64)>> {
        let mut st = conn.prepare(&format!("SELECT * FROM {table}"))?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (x, y, c) = row?;
            check(table, &[F::I(x), F::I(y)], &c)?;
            out.push((x, y));
        }
        Ok(out)
    };
    for (child, parent) in pairs("parent")? {
        if parent >= child {
            bail!("a parent edge runs forwards in the index");
        }
        a.parents.insert((id(child)?, id(parent)?));
    }
    for (parent, child) in pairs("child")? {
        a.children.insert((id(parent)?, id(child)?));
    }
    let singles = |table: &'static str| -> Result<Vec<i64>> {
        let mut st = conn.prepare(&format!("SELECT seq, ck FROM {table}"))?;
        let mut out = Vec::new();
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
            let (s, c) = row?;
            check(table, &[F::I(s)], &c)?;
            out.push(s);
        }
        Ok(out)
    };
    for s in singles("head")? {
        a.heads.insert(id(s)?);
    }
    for s in singles("hidden")? {
        a.hidden.insert(id(s)?);
    }
    {
        let mut st = conn.prepare("SELECT tag, seq, ck FROM tag")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?)))? {
            let (t, s, c) = row?;
            check("tag", &[F::B(t.as_bytes()), F::I(s)], &c)?;
            a.tags.insert((t, id(s)?));
        }
    }
    {
        let mut st = conn.prepare("SELECT target, rkind, src, ck FROM ref")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Vec<u8>>(3)?))
        })?;
        for row in rows {
            let (t, k, s, c) = row?;
            check("ref", &[F::I(t), F::I(k), F::I(s)], &c)?;
            a.refs.insert((id(t)?, k, id(s)?));
        }
    }
    {
        let mut st = conn.prepare("SELECT src_end, rkind, rel, to_end, ck FROM fwd")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        for row in rows {
            let (f, k, rel, t, c) = row?;
            check("fwd", &[F::I(f), F::I(k), F::I(rel), F::I(t)], &c)?;
            a.forward.insert((id(f)?, k, id(rel)?, id(t)?));
        }
    }
    read_shards(conn)?;
    read_meta(conn)?;
    Ok(a)
}

/// Fails naming the first part where `got` (what an index says) differs from `want` (the oracle).
fn differs(what: &str, want: &Answers, got: &Answers) -> Result<()> {
    fn part<T: Ord + std::fmt::Debug>(what: &str, name: &str, want: &BTreeSet<T>, got: &BTreeSet<T>) -> Result<()> {
        if want != got {
            bail!(
                "{what} disagrees with the full recompute on {name}: {} missing (first {:?}), {} extra (first {:?})",
                want.difference(got).count(),
                want.difference(got).next(),
                got.difference(want).count(),
                got.difference(want).next(),
            );
        }
        Ok(())
    }
    if want.order != got.order {
        let at = want
            .order
            .iter()
            .zip(&got.order)
            .position(|(a, b)| a != b)
            .unwrap_or(want.order.len().min(got.order.len()));
        bail!("{what} disagrees with the full recompute on the causal order, first at position {at}");
    }
    part(what, "objects", &want.objects, &got.objects)?;
    part(what, "parents", &want.parents, &got.parents)?;
    part(what, "children", &want.children, &got.children)?;
    part(what, "heads", &want.heads, &got.heads)?;
    part(what, "tags", &want.tags, &got.tags)?;
    part(what, "references", &want.refs, &got.refs)?;
    part(what, "relations by from", &want.forward, &got.forward)?;
    part(what, "the --current hidden set", &want.hidden, &got.hidden)?;
    Ok(())
}

/// `BRACKEN_FULL_CHECK` (section 7, Oracle): compares the index's answers with the full recompute from
/// `store` (a full open, made after `before` was taken), committing nothing. An index built fresh in
/// memory checks the derivation; the replica's own, if it has one, is brought up to date inside a
/// transaction that's rolled back, which checks what freshness and the writes kept. Every row's checksum
/// is checked. One that would need a rebuild or a new generation is reported as such, not compared.
pub fn compare(dir: &Path, store: &Store, before: &Before) -> Result<()> {
    let want = expected(store);
    let mut fresh = Connection::open_in_memory()?;
    {
        let tx = fresh.transaction()?;
        rebuild_in(&tx, dir, Path::new(""), store, &Before::stats(dir), Record::No, CHECK)?;
        tx.commit()?;
    }
    differs("a freshly built index", &want, &actual(&fresh)?)?;

    // cache/ only as a real directory, opened without following a link before it's resolved, as every
    // other open does (finding 32fc7df1acf4); none, or none this process can write, means no index here.
    if !usable(dir) || !matches!(crate::snapshot::cache::Cache::open(dir, false), Ok(Some(_))) {
        return Ok(());
    }
    let Ok(cache) = std::fs::canonicalize(dir.join("cache")) else { return Ok(()) };
    let Some(generation) = current_generation(&cache) else { return Ok(()) };
    let path = cache.join(file_name(generation));
    let mut conn = open_db(&path, BUSY).with_context(|| format!("opening {}", path.display()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stats = Before { clock: None, shards: before.shards.clone() };
    // The first pass in a savepoint, so a second starts from the index as it is (finding 48732994a715):
    // what the first wrote, its insertions and a stale mark, would make the second recompute the derived
    // tables whole and hide damage to them. The transaction, and its lock, stay.
    tx.execute_batch("SAVEPOINT full_check")?;
    match replica(&tx, dir, &path, store, &want, &stats)? {
        Replica::Agrees | Replica::Unused => {}
        // Another process indexed what `store`, opened earlier, never listed. Nothing can index more while
        // this transaction holds the lock, so a store listed now holds all the index does: the index is
        // brought up to date with that one instead, and compared with it whole.
        Replica::Ahead => {
            tx.execute_batch("ROLLBACK TO full_check; RELEASE full_check")?;
            let stats = Before::stats(dir);
            let now = Store::open(dir).context("BRACKEN_FULL_CHECK: reopening the store")?;
            let want = expected(&now);
            if let Replica::Ahead = replica(&tx, dir, &path, &now, &want, &stats)? {
                bail!("BRACKEN_FULL_CHECK: this replica's index holds more than a store listed under its lock");
            }
        }
    }
    tx.rollback()?;
    Ok(())
}

/// What the full check found in the replica's own index.
enum Replica {
    /// Brought up to date with the store, it gives the same answers.
    Agrees,
    /// It needs a rebuild or a new generation, so there's nothing of it to compare.
    Unused,
    /// It holds every contribution the store does and more, each with its object.
    Ahead,
}

/// The replica's index brought up to date with `store` inside `tx` (which the caller rolls back) and
/// compared with `want`, the full recompute from `store`. Anything the store holds that the index lacks
/// is a disagreement, and so is an indexed contribution with no object.
fn replica(tx: &Connection, dir: &Path, path: &Path, store: &Store, want: &Answers, stats: &Before) -> Result<Replica> {
    match reconcile(tx, dir, path, store, stats, Record::No, CHECK, &HashSet::new()) {
        Ok(_) => {}
        Err(Needs::Rebuild) => return Ok(Replica::Unused),
        Err(Needs::Damaged(e)) => return Err(e.context("BRACKEN_FULL_CHECK: this replica's index is damaged")),
        Err(Needs::Error(e)) if is_damage(&e) => return Ok(Replica::Unused),
        Err(Needs::Error(e)) => return Err(e),
    }
    let got = actual(tx)?;
    let held: BTreeSet<&Id> = want.order.iter().collect();
    let indexed: BTreeSet<&Id> = got.order.iter().collect();
    let extra: Vec<ContributionId> = indexed.difference(&held).map(|id| ContributionId(**id)).collect();
    if extra.is_empty() || !held.is_subset(&indexed) {
        differs("this replica's index", want, &got).context("BRACKEN_FULL_CHECK")?;
        return Ok(Replica::Agrees);
    }
    if let Some(id) = extra.iter().find(|id| !object_exists(dir, id)) {
        bail!("BRACKEN_FULL_CHECK: this replica's index holds {}, which has no object", id.to_hex());
    }
    Ok(Replica::Ahead)
}

#[cfg(test)]
mod tests;
