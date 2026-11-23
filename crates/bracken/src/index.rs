//! The index store (task `7d140f132b1a`, decision `f294933b4244`, [docs/design/index-store.md]): a local,
//! derived, rebuildable SQLite database, `cache/index.sqlite` in WAL mode, holding what reads need:
//! ids, kinds, authors, parents, heads, tags, references and the `--current` hidden set.
//!
//! **A cache, never the truth.** Everything in it is derived from `objects/`, and deleting it loses
//! nothing: the next open rebuilds it. It has the snapshot's trust model (decision `96e282819607`, the
//! header of [`crate::snapshot`]): it's this replica's own record of contributions its readers already
//! verified, kept under `cache/`, which git ignores. Here every read still opens the store through the
//! snapshot; the index is kept current beside it, and `BRACKEN_FULL_CHECK` compares its answers with
//! the full recompute (see [`compare`]), the oracle, until reads are served from it.
//!
//! **Freshness** (design section 5). Objects can arrive outside `bracken`: a git pull, a checkpoint,
//! another tool, an older binary. Before the store lists `objects/`, [`Shards::take`] stats each shard
//! directory (mtime, ctime, inode). A shard whose stamp differs from the one the index recorded is
//! dirty: only dirty shards are diffed, the index's ids with that first byte against the store's.
//! What the store holds and the index lacks is added; an indexed id whose object is gone means a
//! rebuild. A stamp is recorded only from a stat taken before the listing it was reconciled with, and
//! only once it is more than [`RACY_NS`] older than the clock (git's racy-index rule), so a file added
//! in the same timestamp tick as the stat is never missed: the shard stays dirty and is diffed again.
//!
//! **Writes.** An object is written (fsync and rename) before the index transaction commits. A crash in
//! between leaves an object the index lacks in a shard whose stamp changed, so the next open adds it.
//! The index never names an object that wasn't written.
//!
//! **Rebuilds** happen in place, in one transaction, so concurrent readers never see a half-built
//! index: on a schema version or ground mismatch, a missing parent, or an indexed object that's gone.
//! Only a file SQLite can't open as a database is deleted and recreated. The database is opened with
//! `SQLITE_OPEN_NOFOLLOW`, inside a `cache/` that [`crate::snapshot`]'s cache module refuses if it's a
//! link.
//!
//! [docs/design/index-store.md]: ../../../docs/design/index-store.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use manyroot::{Body, BodyKind, Contribution, ContributionId, RelationKind, Store, Verdict};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};

/// Bumped whenever the schema or what a row means changes; a mismatch means rebuild.
pub const SCHEMA_VERSION: i64 = 1;
const FILE: &str = "index.sqlite";
/// How much older than the clock a shard's stamp must be before it's recorded (git's racy-index rule):
/// covers coarse filesystem timestamps (1-2 s on some, a kernel tick on Linux).
pub const RACY_NS: i128 = 2_000_000_000;

const SCHEMA: &str = "
CREATE TABLE obj    (seq INTEGER PRIMARY KEY, id BLOB NOT NULL UNIQUE, kind INTEGER NOT NULL,
                     author INTEGER NOT NULL, ts_ms INTEGER NOT NULL);
CREATE TABLE agent  (n INTEGER PRIMARY KEY, key BLOB NOT NULL UNIQUE);
CREATE TABLE parent (child INTEGER NOT NULL, parent INTEGER NOT NULL, PRIMARY KEY (child, parent)) WITHOUT ROWID;
CREATE TABLE child  (parent INTEGER NOT NULL, child INTEGER NOT NULL, PRIMARY KEY (parent, child)) WITHOUT ROWID;
CREATE TABLE head   (seq INTEGER PRIMARY KEY);
CREATE TABLE tag    (tag TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (tag, seq)) WITHOUT ROWID;
CREATE INDEX obj_kind   ON obj(kind, seq);
CREATE INDEX obj_author ON obj(author, seq);
CREATE TABLE ref    (target INTEGER NOT NULL, rkind INTEGER NOT NULL, src INTEGER NOT NULL,
                     PRIMARY KEY (target, rkind, src)) WITHOUT ROWID;
CREATE TABLE fwd    (src_end INTEGER NOT NULL, rkind INTEGER NOT NULL, rel INTEGER NOT NULL,
                     to_end INTEGER NOT NULL, PRIMARY KEY (src_end, rkind, rel)) WITHOUT ROWID;
CREATE TABLE hidden (seq INTEGER PRIMARY KEY);
CREATE TABLE shard  (n INTEGER PRIMARY KEY, mtime_ns INTEGER NOT NULL, ctime_ns INTEGER NOT NULL,
                     ino INTEGER NOT NULL);
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);
";
const TABLES: [&str; 12] =
    ["obj", "agent", "parent", "child", "head", "tag", "ref", "fwd", "hidden", "shard", "meta", "sqlite_stat1"];

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

/// A shard directory's stamp: what changes when a file in it is added, renamed or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    mtime_ns: i64,
    ctime_ns: i64,
    ino: i64,
}

/// `objects/`'s shard directories and their stamps, taken before the store lists them. A shard with no
/// stamp (a non-Unix platform, or a stat that failed) is always dirty.
#[derive(Clone, Debug, Default)]
pub struct Shards(BTreeMap<u8, Option<Stamp>>);

impl Shards {
    pub fn take(dir: &Path) -> Shards {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(dir.join("objects")) else { return Shards(out) };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(n) = name.to_str().filter(|s| s.len() == 2).and_then(|s| u8::from_str_radix(s, 16).ok()) else {
                continue;
            };
            out.insert(n, stamp(&entry.path()));
        }
        Shards(out)
    }
}

#[cfg(unix)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path).ok().filter(|m| m.is_dir())?;
    let ns = |s: i64, n: i64| s.checked_mul(1_000_000_000)?.checked_add(n);
    Some(Stamp { mtime_ns: ns(m.mtime(), m.mtime_nsec())?, ctime_ns: ns(m.ctime(), m.ctime_nsec())?, ino: m.ino() as i64 })
}

#[cfg(not(unix))]
fn stamp(_: &Path) -> Option<Stamp> {
    None
}

/// Whether to record the stamps taken before the store's listing. A refresh after a write doesn't:
/// stamps taken then may cover objects other processes wrote that this store never listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Record {
    Yes,
    No,
}

/// What a refresh did, for tests and the benchmark.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub rebuilt: bool,
    pub dirty_shards: usize,
    pub added: usize,
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("cache").join(FILE)
}

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

/// Whether an error says the file isn't a usable database, so it should be replaced.
fn is_damage(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase | ErrorCode::CannotOpen))
}

/// Opens the index under `dir/cache`, making `cache/` (refused if it's a link) and the file if missing.
/// A file SQLite can't use as a database is deleted with its WAL and shared-memory files and made anew.
fn connect(dir: &Path) -> Result<Connection> {
    crate::snapshot::cache::Cache::open(dir, true).context("opening cache/")?.context("no cache/ was made")?;
    let path = index_path(dir);
    let probe = |path: &Path| -> rusqlite::Result<Connection> {
        let conn = open_db(path)?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
        Ok(conn)
    };
    match probe(&path) {
        Ok(conn) => Ok(conn),
        Err(e) if is_damage(&e) => {
            for suffix in ["", "-wal", "-shm"] {
                let p = PathBuf::from(format!("{}{suffix}", path.display()));
                match std::fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e).with_context(|| format!("removing {}", p.display())),
                }
            }
            probe(&path).with_context(|| format!("recreating {}", path.display()))
        }
        Err(e) => Err(e).with_context(|| format!("opening {}", path.display())),
    }
}

/// Keeps the index current with `store`, which was opened after `before` was taken: built on first use,
/// rebuilt on a mismatch or damage, otherwise only dirty shards reconciled. Callers treat a failure as a
/// cache that's only slower next time.
pub fn refresh(dir: &Path, store: &Store, before: &Shards, record: Record) -> Result<Outcome> {
    refresh_with(dir, store, before, record, RACY_NS)
}

pub(crate) fn refresh_with(dir: &Path, store: &Store, before: &Shards, record: Record, racy_ns: i128) -> Result<Outcome> {
    let mut conn = connect(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = match reconcile(&tx, store, before, record, racy_ns) {
        Ok(outcome) => outcome,
        Err(Needs::Rebuild) => {
            rebuild_in(&tx, store, before, racy_ns)?;
            Outcome { rebuilt: true, dirty_shards: before.0.len(), added: store.graph().len() }
        }
        Err(Needs::Error(e)) if e.downcast_ref::<rusqlite::Error>().is_some_and(is_damage) => {
            drop(tx);
            drop(conn);
            return rebuild(dir, store, before).map(|()| Outcome {
                rebuilt: true,
                dirty_shards: before.0.len(),
                added: store.graph().len(),
            });
        }
        Err(Needs::Error(e)) => return Err(e),
    };
    tx.commit()?;
    Ok(outcome)
}

/// `bracken verify`: the index rebuilt whole from `store`, a full open made after `before` was taken.
pub fn rebuild(dir: &Path, store: &Store, before: &Shards) -> Result<()> {
    let mut conn = connect(dir)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    rebuild_in(&tx, store, before, RACY_NS)?;
    tx.commit()?;
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        bail!("the index failed SQLite's quick_check after a rebuild: {integrity}");
    }
    Ok(())
}

enum Needs {
    Rebuild,
    Error(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Needs {
    fn from(e: E) -> Needs {
        Needs::Error(e.into())
    }
}

fn meta(tx: &Connection, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    tx.query_row("SELECT v FROM meta WHERE k = ?1", [key], |r| r.get(0)).optional()
}

fn has_schema(tx: &Connection) -> rusqlite::Result<bool> {
    tx.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'", [], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
}

/// Brings an index of the right version and ground up to date, or says it needs a rebuild.
fn reconcile(tx: &Transaction, store: &Store, before: &Shards, record: Record, racy_ns: i128) -> Result<Outcome, Needs> {
    if !has_schema(tx)? {
        return Err(Needs::Rebuild);
    }
    let version = meta(tx, "schema_version")?;
    let ground = meta(tx, "ground")?;
    if version.as_deref() != Some(&SCHEMA_VERSION.to_le_bytes()[..])
        || ground.as_deref() != Some(&store.info().ground.as_bytes()[..])
    {
        return Err(Needs::Rebuild);
    }
    let recorded = recorded_stamps(tx)?;
    let dirty: BTreeSet<u8> = before
        .0
        .iter()
        .filter(|(n, s)| s.is_none() || recorded.get(n) != Some(*s))
        .map(|(n, _)| *n)
        .chain(recorded.keys().filter(|n| !before.0.contains_key(n)).copied())
        .collect();
    let g = store.graph();
    let mut added: Vec<ContributionId> = Vec::new();
    if !dirty.is_empty() {
        let mut in_graph: HashMap<u8, HashSet<ContributionId>> = dirty.iter().map(|n| (*n, HashSet::new())).collect();
        for c in g.iter() {
            if let Some(set) = in_graph.get_mut(&c.id().as_bytes()[0]) {
                set.insert(c.id());
            }
        }
        for n in &dirty {
            let indexed = ids_in_shard(tx, *n)?;
            for id in &indexed {
                // Indexed but not in this store: newer than this store's listing if its object is there,
                // gone (a checkout that went back) if not.
                if !in_graph[n].contains(id) && !object_exists(store.dir(), id) {
                    return Err(Needs::Rebuild);
                }
            }
            added.extend(in_graph[n].iter().filter(|id| !indexed.contains(id)));
        }
    }
    let added_count = added.len();
    if !added.is_empty() {
        let wanted: HashSet<ContributionId> = added.into_iter().collect();
        let order: Vec<ContributionId> = g.topo_order().into_iter().filter(|id| wanted.contains(id)).collect();
        let mut ins = Inserter::new(tx)?;
        for id in &order {
            if !ins.insert(g.get(id).expect("ordered ids exist"))? {
                return Err(Needs::Rebuild);
            }
        }
        tx.execute("INSERT OR REPLACE INTO meta VALUES ('hidden_stale', 1)", [])?;
    }
    // The hidden set is recomputed whole from this store, and only when the index holds exactly what the
    // store holds; otherwise it's left marked stale for a later refresh.
    let stale = meta(tx, "hidden_stale")?.is_some_and(|v| v != [0]);
    if stale && count(tx)? == g.len() as i64 {
        write_hidden(tx, store)?;
    }
    if record == Record::Yes {
        record_stamps(tx, before, &dirty, racy_ns)?;
    }
    Ok(Outcome { rebuilt: false, dirty_shards: dirty.len(), added: added_count })
}

fn count(tx: &Connection) -> rusqlite::Result<i64> {
    tx.query_row("SELECT coalesce(max(seq) + 1, 0) FROM obj", [], |r| r.get(0))
}

fn object_exists(dir: &Path, id: &ContributionId) -> bool {
    let hex = id.to_hex();
    std::fs::symlink_metadata(dir.join("objects").join(&hex[..2]).join(&hex[2..])).is_ok_and(|m| m.is_file())
}

fn recorded_stamps(tx: &Connection) -> rusqlite::Result<BTreeMap<u8, Stamp>> {
    let mut st = tx.prepare("SELECT n, mtime_ns, ctime_ns, ino FROM shard")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, u8>(0)?, Stamp { mtime_ns: r.get(1)?, ctime_ns: r.get(2)?, ino: r.get(3)? }))
    })?;
    rows.collect()
}

fn now_ns() -> i128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0)
}

/// Records each reconciled shard's stamp from `before`, unless it's too recent to trust (it stays dirty).
fn record_stamps(tx: &Connection, before: &Shards, reconciled: &BTreeSet<u8>, racy_ns: i128) -> rusqlite::Result<()> {
    let cutoff = now_ns() - racy_ns;
    for n in reconciled {
        tx.execute("DELETE FROM shard WHERE n = ?1", [n])?;
        if let Some(Some(s)) = before.0.get(n)
            && (s.mtime_ns as i128) < cutoff
            && (s.ctime_ns as i128) < cutoff
        {
            tx.execute("INSERT INTO shard VALUES (?1, ?2, ?3, ?4)", params![n, s.mtime_ns, s.ctime_ns, s.ino])?;
        }
    }
    Ok(())
}

fn ids_in_shard(tx: &Connection, n: u8) -> rusqlite::Result<HashSet<ContributionId>> {
    let lo = [n];
    let mut st = if n == u8::MAX {
        tx.prepare_cached("SELECT id FROM obj WHERE id >= ?1")?
    } else {
        tx.prepare_cached("SELECT id FROM obj WHERE id >= ?1 AND id < ?2")?
    };
    let hi = [n.wrapping_add(1)];
    let map = |r: &rusqlite::Row| r.get::<_, Vec<u8>>(0);
    let rows: Vec<Vec<u8>> = if n == u8::MAX {
        st.query_map([&lo[..]], map)?.collect::<rusqlite::Result<_>>()?
    } else {
        st.query_map([&lo[..], &hi[..]], map)?.collect::<rusqlite::Result<_>>()?
    };
    Ok(rows.into_iter().filter_map(|b| <[u8; 32]>::try_from(b).ok().map(ContributionId)).collect())
}

/// Empties the index and fills it from `store`, inside `tx`: concurrent readers see the old index or
/// the new one, never a half-built one.
fn rebuild_in(tx: &Transaction, store: &Store, before: &Shards, racy_ns: i128) -> Result<()> {
    for table in TABLES {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))?;
    }
    tx.execute_batch("DROP INDEX IF EXISTS obj_kind; DROP INDEX IF EXISTS obj_author;")?;
    tx.execute_batch(SCHEMA)?;
    let g = store.graph();
    {
        let mut ins = Inserter::new(tx)?;
        for id in g.topo_order() {
            let inserted = ins.insert(g.get(&id).expect("ordered ids exist"))?;
            assert!(inserted, "topological order puts parents first");
        }
    }
    tx.execute("INSERT INTO meta VALUES ('schema_version', ?1)", [&SCHEMA_VERSION.to_le_bytes()[..]])?;
    tx.execute("INSERT INTO meta VALUES ('ground', ?1)", [&store.info().ground.as_bytes()[..]])?;
    write_hidden(tx, store)?;
    let all: BTreeSet<u8> = before.0.keys().copied().collect();
    record_stamps(tx, before, &all, racy_ns)?;
    Ok(())
}

/// Replaces the hidden set with [`crate::CurrentView`]'s over `store`, and clears the stale mark.
fn write_hidden(tx: &Connection, store: &Store) -> rusqlite::Result<()> {
    let g = store.graph();
    let view = crate::CurrentView::new(g, &g.topo_order());
    let mut want: BTreeSet<i64> = BTreeSet::new();
    let mut seq = tx.prepare_cached("SELECT seq FROM obj WHERE id = ?1")?;
    for id in &view.hidden {
        want.insert(seq.query_row([&id.as_bytes()[..]], |r| r.get(0))?);
    }
    let have: BTreeSet<i64> =
        tx.prepare("SELECT seq FROM hidden")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for s in have.difference(&want) {
        tx.execute("DELETE FROM hidden WHERE seq = ?1", [s])?;
    }
    for s in want.difference(&have) {
        tx.execute("INSERT INTO hidden VALUES (?1)", [s])?;
    }
    tx.execute("INSERT OR REPLACE INTO meta VALUES ('hidden_stale', ?1)", [&[0u8][..]])?;
    Ok(())
}

/// Inserts contributions, parents first, assigning each the next seq.
struct Inserter<'t> {
    tx: &'t Connection,
    next: i64,
}

impl<'t> Inserter<'t> {
    fn new(tx: &'t Connection) -> rusqlite::Result<Inserter<'t>> {
        Ok(Inserter { tx, next: count(tx)? })
    }

    fn seq_of(&self, id: &ContributionId) -> rusqlite::Result<Option<i64>> {
        self.tx.prepare_cached("SELECT seq FROM obj WHERE id = ?1")?.query_row([&id.as_bytes()[..]], |r| r.get(0)).optional()
    }

    /// Adds `c`; `false` (and nothing written) if a parent or a referenced contribution isn't indexed.
    fn insert(&mut self, c: &Contribution) -> rusqlite::Result<bool> {
        let tx = self.tx;
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
        let key = &c.author().as_bytes()[..];
        tx.prepare_cached("INSERT OR IGNORE INTO agent (key) VALUES (?1)")?.execute([key])?;
        let author: i64 = tx.prepare_cached("SELECT n FROM agent WHERE key = ?1")?.query_row([key], |r| r.get(0))?;
        let seq = self.next;
        tx.prepare_cached("INSERT INTO obj VALUES (?1, ?2, ?3, ?4, ?5)")?.execute(params![
            seq,
            &c.id().as_bytes()[..],
            kind_code(c.body().kind()),
            author,
            c.content().timestamp_ms as i64
        ])?;
        for p in parents {
            tx.prepare_cached("INSERT OR IGNORE INTO parent VALUES (?1, ?2)")?.execute([seq, p])?;
            tx.prepare_cached("INSERT OR IGNORE INTO child VALUES (?1, ?2)")?.execute([p, seq])?;
            tx.prepare_cached("DELETE FROM head WHERE seq = ?1")?.execute([p])?;
        }
        tx.prepare_cached("INSERT INTO head VALUES (?1)")?.execute([seq])?;
        if let Body::Claim { tags, .. } = c.body() {
            for t in tags {
                tx.prepare_cached("INSERT OR IGNORE INTO tag VALUES (?1, ?2)")?.execute(params![t, seq])?;
            }
        }
        for (target, rkind) in &refs {
            tx.prepare_cached("INSERT OR IGNORE INTO ref VALUES (?1, ?2, ?3)")?.execute([*target, *rkind, seq])?;
        }
        if let Body::Relation { kind, .. } = c.body() {
            let (to, from) = (refs[0].0, refs[1].0);
            tx.prepare_cached("INSERT OR IGNORE INTO fwd VALUES (?1, ?2, ?3, ?4)")?.execute([
                from,
                relation_code(*kind),
                seq,
                to,
            ])?;
        }
        self.next += 1;
        Ok(true)
    }
}

/// Everything the index answers, as id-keyed rows, so two derivations can be compared.
#[derive(Debug, Default, PartialEq, Eq)]
struct Answers {
    objects: BTreeSet<([u8; 32], i64, [u8; 32], i64)>,
    parents: BTreeSet<([u8; 32], [u8; 32])>,
    children: BTreeSet<([u8; 32], [u8; 32])>,
    heads: BTreeSet<[u8; 32]>,
    tags: BTreeSet<(String, [u8; 32])>,
    refs: BTreeSet<([u8; 32], i64, [u8; 32])>,
    forward: BTreeSet<([u8; 32], i64, [u8; 32], [u8; 32])>,
    hidden: BTreeSet<[u8; 32]>,
}

/// The oracle: the answers recomputed from the store, through the graph's own maps (referrers, heads,
/// parents) rather than the bodies the index was filled from, and the hidden set from `CurrentView`.
fn expected(store: &Store) -> Answers {
    let g = store.graph();
    let mut a = Answers::default();
    for c in g.iter() {
        let id = *c.id().as_bytes();
        a.objects.insert((id, kind_code(c.body().kind()), *c.author().as_bytes(), c.content().timestamp_ms as i64));
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
    a.hidden = crate::CurrentView::new(g, &g.topo_order()).hidden.iter().map(|h| *h.as_bytes()).collect();
    a
}

fn id32(b: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    <[u8; 32]>::try_from(b).map_err(|_| rusqlite::Error::InvalidColumnType(0, "id".into(), rusqlite::types::Type::Blob))
}

/// What an index says, read back as id-keyed rows; also checks that every edge runs from a lower seq
/// (parents first) and that seqs are dense.
fn actual(conn: &Connection) -> Result<Answers> {
    let mut a = Answers::default();
    let mut st = conn.prepare("SELECT o.id, o.kind, a.key, o.ts_ms FROM obj o JOIN agent a ON a.n = o.author")?;
    for row in st.query_map([], |r| Ok((id32(r.get(0)?)?, r.get(1)?, id32(r.get(2)?)?, r.get(3)?)))? {
        a.objects.insert(row?);
    }
    let pairs = |sql: &str| -> Result<BTreeSet<([u8; 32], [u8; 32])>> {
        let mut st = conn.prepare(sql)?;
        let rows = st.query_map([], |r| Ok((id32(r.get(0)?)?, id32(r.get(1)?)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    a.parents = pairs("SELECT c.id, p.id FROM parent x JOIN obj c ON c.seq = x.child JOIN obj p ON p.seq = x.parent")?;
    a.children = pairs("SELECT p.id, c.id FROM child x JOIN obj p ON p.seq = x.parent JOIN obj c ON c.seq = x.child")?;
    let singles = |sql: &str| -> Result<BTreeSet<[u8; 32]>> {
        let mut st = conn.prepare(sql)?;
        let rows = st.query_map([], |r| id32(r.get(0)?))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    a.heads = singles("SELECT o.id FROM head h JOIN obj o ON o.seq = h.seq")?;
    a.hidden = singles("SELECT o.id FROM hidden h JOIN obj o ON o.seq = h.seq")?;
    let mut st = conn.prepare("SELECT t.tag, o.id FROM tag t JOIN obj o ON o.seq = t.seq")?;
    for row in st.query_map([], |r| Ok((r.get(0)?, id32(r.get(1)?)?)))? {
        a.tags.insert(row?);
    }
    let mut st = conn.prepare("SELECT t.id, r.rkind, s.id FROM ref r JOIN obj t ON t.seq = r.target JOIN obj s ON s.seq = r.src")?;
    for row in st.query_map([], |r| Ok((id32(r.get(0)?)?, r.get(1)?, id32(r.get(2)?)?)))? {
        a.refs.insert(row?);
    }
    let mut st = conn.prepare(
        "SELECT f.id, x.rkind, r.id, t.id FROM fwd x JOIN obj f ON f.seq = x.src_end JOIN obj r ON r.seq = x.rel
         JOIN obj t ON t.seq = x.to_end",
    )?;
    for row in st.query_map([], |r| Ok((id32(r.get(0)?)?, r.get(1)?, id32(r.get(2)?)?, id32(r.get(3)?)?)))? {
        a.forward.insert(row?);
    }
    let backwards: i64 = conn.query_row("SELECT count(*) FROM parent WHERE parent >= child", [], |r| r.get(0))?;
    let (n, max): (i64, i64) = conn.query_row("SELECT count(*), coalesce(max(seq), -1) FROM obj", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if backwards != 0 || n != max + 1 {
        bail!("the index's order is broken: {backwards} parent edges run forwards, {n} rows for seqs 0..={max}");
    }
    Ok(a)
}

/// Fails naming the first part where `got` (what the index says) differs from `want` (the oracle).
fn differs(what: &str, want: &Answers, got: &Answers) -> Result<()> {
    fn part<T: Ord + std::fmt::Debug>(what: &str, name: &str, want: &BTreeSet<T>, got: &BTreeSet<T>) -> Result<()> {
        if want != got {
            let missing = want.difference(got).next();
            let extra = got.difference(want).next();
            bail!(
                "{what} disagrees with the full recompute on {name}: {} missing (first {missing:?}), {} extra (first {extra:?})",
                want.difference(got).count(),
                got.difference(want).count()
            );
        }
        Ok(())
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

/// `BRACKEN_FULL_CHECK`: compares the index's answers with the full recompute from `store` (a full
/// open, made after `before` was taken), writing nothing. Two indexes are checked: one built fresh in
/// memory, which checks the derivation, and the replica's own, brought up to date inside a transaction
/// that is rolled back, which checks what freshness and the writes kept. A replica's index that would
/// need a rebuild is reported as such, not compared. Any disagreement is an error.
pub fn compare(dir: &Path, store: &Store, before: &Shards) -> Result<()> {
    let want = expected(store);
    let mut fresh = Connection::open_in_memory()?;
    {
        let tx = fresh.transaction()?;
        rebuild_in(&tx, store, before, RACY_NS)?;
        tx.commit()?;
    }
    differs("a freshly built index", &want, &actual(&fresh)?)?;

    let path = index_path(dir);
    if std::fs::symlink_metadata(&path).is_err() {
        return Ok(());
    }
    let mut conn = match open_db(&path) {
        Ok(conn) => conn,
        Err(e) if is_damage(&e) => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match reconcile(&tx, store, before, Record::No, RACY_NS) {
        Ok(_) => differs("this replica's index", &want, &actual(&tx)?)?,
        Err(Needs::Rebuild) => {}
        Err(Needs::Error(e)) if e.downcast_ref::<rusqlite::Error>().is_some_and(is_damage) => {}
        Err(Needs::Error(e)) => return Err(e),
    }
    tx.rollback()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manyroot::Identity;

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

    /// Adds about `n` contributions through `Store` alone, as an older binary or a git pull would: tagged
    /// claims, evidence, relations (supersessions among them) and retractions of the author's own work.
    fn grow(dir: &Path, seed: u64, n: usize) {
        let mut store = Store::open(dir).expect("valid");
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

    /// Opens as a read does (shards stamped first), refreshes with racy window `racy`, and checks the
    /// replica's index against the full recompute.
    fn read(dir: &Path, racy: i128) -> Outcome {
        let before = Shards::take(dir);
        let store = Store::open(dir).expect("valid");
        let outcome = refresh_with(dir, &store, &before, Record::Yes, racy).expect("refreshed");
        let conn = open_db(&index_path(dir)).expect("opens");
        differs("the index", &expected(&store), &actual(&conn).expect("reads")).expect("agrees");
        outcome
    }

    fn rows(dir: &Path, sql: &str) -> i64 {
        open_db(&index_path(dir)).expect("opens").query_row(sql, [], |r| r.get(0)).expect("a count")
    }

    #[test]
    fn built_on_first_open_then_only_dirty_shards_are_diffed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 1, 120);
        let first = read(&dir, 0);
        assert!(first.rebuilt, "no index yet, so it's built: {first:?}");
        let again = read(&dir, 0);
        assert_eq!(again, Outcome::default(), "nothing changed, so no shard is dirty");
    }

    #[test]
    fn an_object_written_without_the_index_is_found_by_freshness() {
        // What an older binary, another tool or a git pull does: objects appear with no index update.
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 2, 80);
        read(&dir, 0);
        grow(&dir, 3, 1);
        let after = read(&dir, 0);
        assert!(!after.rebuilt && after.added >= 1 && after.dirty_shards >= 1, "{after:?}");
        grow(&dir, 4, 40);
        let more = read(&dir, 0);
        assert!(!more.rebuilt && more.added >= 30, "{more:?}");
    }

    #[test]
    fn a_shard_whose_mtime_was_put_back_is_still_dirty() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 5, 40);
        read(&dir, 0);
        let before = Shards::take(&dir);
        grow(&dir, 6, 1);
        let after = Shards::take(&dir);
        let (n, old) = before
            .0
            .iter()
            .find(|(n, s)| after.0.get(n) != Some(s))
            .map(|(n, s)| (*n, s.expect("stamped")))
            .or_else(|| after.0.keys().find(|n| !before.0.contains_key(n)).map(|n| (*n, Stamp { mtime_ns: 0, ctime_ns: 0, ino: 0 })))
            .expect("the write changed a shard");
        if old.ino != 0 {
            let shard = dir.join("objects").join(format!("{n:02x}"));
            let t = rustix::fs::Timespec { tv_sec: old.mtime_ns / 1_000_000_000, tv_nsec: (old.mtime_ns % 1_000_000_000) as _ };
            let times = rustix::fs::Timestamps { last_access: t, last_modification: t };
            rustix::fs::utimensat(rustix::fs::CWD, &shard, &times, rustix::fs::AtFlags::empty()).expect("set mtime back");
        }
        let outcome = read(&dir, 0);
        assert!(outcome.added >= 1, "the ctime still differs, so the shard is diffed: {outcome:?}");
    }

    #[test]
    fn a_recent_stamp_is_not_recorded_so_its_shard_stays_dirty() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 7, 30);
        read(&dir, RACY_NS);
        assert_eq!(rows(&dir, "SELECT count(*) FROM shard"), 0, "every shard was written in the last 2 s");
        let again = read(&dir, RACY_NS);
        assert!(again.dirty_shards > 0 && again.added == 0 && !again.rebuilt, "{again:?}");
        read(&dir, 0);
        assert!(rows(&dir, "SELECT count(*) FROM shard") > 0);
    }

    #[test]
    fn a_crash_between_the_object_write_and_the_index_commit_is_repaired_on_the_next_open() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 8, 40);
        read(&dir, 0);
        // A write command: the store opened, an object written, then the process dies before the
        // index transaction (Refreshing's drop, which refreshes the index, never runs).
        let written = {
            let mut s = crate::snapshot::Refreshing::open(&dir, crate::snapshot::Refresh::Yes).expect("opens");
            let who = s.identity("builder").expect("an identity");
            let id = s.contribute(&who, Body::Claim { statement: "lost".into(), tags: vec!["note".into()] }).expect("ok");
            std::mem::forget(s);
            id
        };
        let after = read(&dir, 0);
        assert!(after.added >= 1 && !after.rebuilt, "{after:?}");
        let n = rows(&dir, &format!("SELECT count(*) FROM obj WHERE hex(id) = '{}'", written.to_hex().to_uppercase()));
        assert_eq!(n, 1);
    }

    #[test]
    fn a_write_through_refreshing_is_indexed_when_it_ends() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 9, 30);
        read(&dir, 0);
        let written = {
            let mut s = crate::snapshot::Refreshing::open(&dir, crate::snapshot::Refresh::Yes).expect("opens");
            let who = s.identity("builder").expect("an identity");
            s.contribute(&who, Body::Claim { statement: "kept".into(), tags: vec!["decision".into()] }).expect("ok")
        };
        let n = rows(&dir, &format!("SELECT count(*) FROM tag t JOIN obj o ON o.seq = t.seq WHERE t.tag = 'decision' AND hex(o.id) = '{}'", written.to_hex().to_uppercase()));
        assert_eq!(n, 1, "indexed by the write itself, before any read");
        read(&dir, 0);
    }

    #[test]
    fn a_version_or_ground_mismatch_and_damage_mean_rebuild() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 10, 40);
        read(&dir, 0);
        let conn = open_db(&index_path(&dir)).expect("opens");
        conn.execute("UPDATE meta SET v = ?1 WHERE k = 'schema_version'", [&0i64.to_le_bytes()[..]]).expect("set");
        drop(conn);
        assert!(read(&dir, 0).rebuilt, "a schema version mismatch rebuilds");
        let conn = open_db(&index_path(&dir)).expect("opens");
        conn.execute("UPDATE meta SET v = ?1 WHERE k = 'ground'", [&[9u8; 32][..]]).expect("set");
        drop(conn);
        assert!(read(&dir, 0).rebuilt, "a ground mismatch rebuilds");
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", index_path(&dir).display()));
        }
        std::fs::write(index_path(&dir), b"not a database, just bytes long enough to look like a header..").expect("w");
        assert!(read(&dir, 0).rebuilt, "a damaged file is replaced");
    }

    #[test]
    fn an_indexed_object_that_is_gone_means_rebuild() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 11, 40);
        read(&dir, 0);
        let store = Store::open(&dir).expect("valid");
        let head = store.graph().heads().next().expect("a head");
        let hex = head.to_hex();
        std::fs::remove_file(dir.join("objects").join(&hex[..2]).join(&hex[2..])).expect("removed");
        assert!(read(&dir, 0).rebuilt, "a checkout that went back");
    }

    #[test]
    fn a_missing_index_row_with_a_clean_stamp_fails_the_full_check() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 12, 60);
        read(&dir, 0);
        let before = Shards::take(&dir);
        let store = Store::open(&dir).expect("valid");
        compare(&dir, &store, &before).expect("an index kept current agrees");
        let conn = open_db(&index_path(&dir)).expect("opens");
        conn.execute("DELETE FROM tag WHERE rowid IN (SELECT rowid FROM tag LIMIT 1)", []).ok();
        conn.execute("DELETE FROM tag WHERE seq = (SELECT min(seq) FROM tag)", []).expect("deleted");
        drop(conn);
        let err = compare(&dir, &store, &before).expect_err("a lost row is caught");
        assert!(format!("{err:#}").contains("tags"), "{err:#}");
        let conn = open_db(&index_path(&dir)).expect("opens");
        conn.execute("DELETE FROM hidden WHERE seq = (SELECT min(seq) FROM hidden)", []).expect("deleted");
        drop(conn);
        assert!(compare(&dir, &store, &before).is_err());
    }

    #[test]
    fn the_full_check_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 13, 30);
        read(&dir, 0);
        grow(&dir, 14, 10);
        let before = Shards::take(&dir);
        let store = Store::open(&dir).expect("valid");
        let count = rows(&dir, "SELECT count(*) FROM obj");
        compare(&dir, &store, &before).expect("agrees after an in-transaction catch-up");
        assert_eq!(rows(&dir, "SELECT count(*) FROM obj"), count, "rolled back");
    }

    #[test]
    fn concurrent_refreshes_agree() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 15, 60);
        read(&dir, 0);
        grow(&dir, 16, 60);
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let before = Shards::take(&dir);
                    let store = Store::open(&dir).expect("valid");
                    refresh_with(&dir, &store, &before, Record::Yes, 0).expect("refreshed")
                })
            })
            .collect();
        let added: usize = handles.into_iter().map(|h| h.join().expect("joined").added).sum();
        assert!(added >= 50, "each object added once, by whichever got there first: {added}");
        assert_eq!(read(&dir, 0), Outcome::default());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_index_file_is_never_written_through() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = ground(tmp.path());
        grow(&dir, 17, 20);
        std::fs::create_dir_all(dir.join("cache")).expect("cache/");
        let outside = tmp.path().join("outside");
        std::fs::write(&outside, b"keep me").expect("w");
        std::os::unix::fs::symlink(&outside, index_path(&dir)).expect("link");
        let before = Shards::take(&dir);
        let store = Store::open(&dir).expect("valid");
        let _ = refresh(&dir, &store, &before, Record::Yes);
        assert_eq!(std::fs::read(&outside).expect("r"), b"keep me");
    }
}
