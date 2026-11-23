# The index store

> **Decided, 2026-11-23.** SQLite in WAL mode, through `rusqlite` 0.43 (features
> `bundled` and `fallible_uint`), in `crates/bracken` next to `snapshot.rs`
> (decision `f294933b4244`, answer `76aa001b623d`). This note is PR 1 of task
> `7d140f132b1a`, which builds Phase 3 of [layers.md](layers.md) (owner decision
> `817afe6653a8`, item 2). The measurements are in note `799ce5d903e9`. PR 2 adds the
> index as a cache behind the existing read API. PR 3 serves reads from it.
> Sections 4–8 were revised for the security review's findings `23579eecd2a7`
> (power loss), `de4b1cb363b6` (the clock) and `2538f1286416` (rebuilds and schema
> versions).

## 1. What it is

The index is a local, derived and rebuildable store of what reads need. It covers
ids, kinds, authors, parents, children, heads, tags, references and the
`--current` hidden set. With it, `log`, `heads`, `show` and `--current` stop
rebuilding the whole graph on every process start.

It holds nothing that `objects/` doesn't. Deleting it loses nothing: the next open
rebuilds it. It is never committed and never synced. It changes no encoding, id,
signature or validity rule, and nothing in `crates/tree` or any frozen format.

It is research B's R1 ([research-b-performance.md](../research/2026-11/research-b-performance.md)
section 5.1), without the chain-cover tables. Those wait for Phase 4 (incremental
acceptance), along with the other derived updaters.

## 2. Measurements

A scratch benchmark, kept outside the repository, built each engine's index in
three cases:

- this repository's Ground, read-only (14,566 contributions);
- a synthetic Ground of 50k contributions;
- a synthetic Ground of 1M contributions.

The synthetic Grounds have the shape of the Phase 1 benchmark (`benches/ground`).
The index holds no payload bytes, so the Phase 1 bar's 200 MiB of objects doesn't
change it.

The runs used a release build on this shared Mac at load 85–155, so the numbers
are noisy. All times are milliseconds unless marked.

### 2.1 One process per read

Each read spawns a process that opens the index, runs the query and exits. The
table gives p50 / p95 of 30 runs, after a warm-up. A process that does nothing
takes 3.3 ms.

| Read | 14k SQLite | 14k LMDB | 1M SQLite | 1M LMDB |
|---|---|---|---|---|
| open | 4.1 / 5.3 | 3.8 / 7.0 | 4.0 / 6.0 | 4.2 / 6.5 |
| `show` (row, parents, referrers) | 5.9 / 9.3 | 6.0 / 12.4 | 4.2 / 5.1 | 3.6 / 4.8 |
| 8-hex prefix | 7.0 / 13.4 | 5.0 / 8.4 | 4.0 / 5.6 | 4.8 / 8.8 |
| `log --tag decision --current --limit 50` and its count | 5.4 / 8.2 | 5.1 / 8.3 | 8.6 / 15.9 | 10.1 / 12.3 |
| `--author`, newest 50 | 5.3 / 11.5 | 6.7 / 18.9 | 3.9 / 4.9 | 3.6 / 4.5 |
| `heads` | 5.4 / 18.7 | 5.7 / 9.5 | 4.0 / 5.9 | 5.1 / 10.0 |

At 50k, every read was 4.0–4.5 ms p50 on both engines, and p95 stayed under
9.2 ms.

Today's reads at 14k take 150–230 ms (report `a2b7afcac5db`). Both engines are
3–12x under the 50 ms bar at 1M, and the index adds 1–7 ms to process start.

In the 1M synthetic Ground, the tag `decision` is on 8% of claims (24k rows), so
the full count dominates that read.

### 2.2 In process, warm, at 1M

Mean time per operation:

| Operation | SQLite | LMDB |
|---|---|---|
| point read by id | 5.5 µs | 1.7 µs |
| 8-hex prefix | 10.5 µs | 1.0 µs |
| `show` | 30 µs | 6 µs |
| tag, newest 50, `--current` | 30 µs | 24 µs |
| count of a 24k-row tag, `--current` | 3.8 ms | 5.6 ms |

LMDB is faster per lookup. Against process start, the difference doesn't matter.

### 2.3 Writes, rebuild and size

**Inserting one contribution per transaction** (p50 / p95):

- SQLite, `synchronous=NORMAL`: 0.06 / 0.09 ms;
- SQLite, `synchronous=FULL`: 0.10 / 0.15 ms;
- LMDB: 5.7 / 9.0 ms.

LMDB issues `F_FULLFSYNC` on macOS. SQLite's `FULL` is a plain `fsync` unless
`PRAGMA fullfsync` is set.

Inserting 1000 contributions in one transaction at 1M took 76 ms with SQLite and
44 ms with LMDB.

**Rebuild** (10k contributions per transaction):

| Ground | SQLite | LMDB |
|---|---|---|
| 14k | 0.10 s | 0.57 s |
| 50k | 0.54 s | 1.14 s |
| 1M | 38 s | 54 s |

**Size.**

- SQLite takes 194–203 B per contribution at every N: 2.7 MiB at 14k and 194 MiB
  at 1M. That matches R1's estimate.
- LMDB's live pages are twice that. Its file was 232 MiB at 14k and 924 MiB at 1M.
  Pages are 16 KiB on Apple Silicon, and freed pages grow the file. The file never
  shrinks without a compacting copy.

### 2.4 Freshness cost

On this Ground's `objects/`:

- statting `objects/` and all 256 shard directories took 0.45 ms (p50);
- listing one shard took 0.07 ms;
- listing every shard took 17.6 ms, about 1.2 µs per entry.

At 1M a shard holds about 3,900 entries, and listing one took 5.2 ms.

### 2.5 Not yet measured

PR 2 adds these numbers:

- a cold page cache (after a reboot) at 1M;
- Linux (the remote test table);
- many concurrent readers.

## 3. Choice: SQLite WAL

Both engines meet the bar by a wide margin, so the choice rests on everything
else:

- **Files.** SQLite's are 2–5x smaller, and they don't grow with churn.
- **Writes.** Durable single writes cost about 100x less on macOS.
- **Safety.** `SQLITE_OPEN_NOFOLLOW` covers the index file. With LMDB there's no
  map size to manage and no reader-slot table to clean after a crashed reader,
  but heed's open is `unsafe`, and a second open in one process is undefined.
  SQLite has none of that.
- **Debugging and views.** `PRAGMA quick_check` / `integrity_check` and the
  `sqlite3` CLI help debugging, and SQL serves Phase 4 and 6 views.
- **Supply chain.** SQLite adds 8 crates to `Cargo.lock`: `rusqlite`,
  `libsqlite3-sys`, `hashlink`, `fallible-iterator`, `fallible-streaming-iterator`,
  `smallvec`, `pkg-config` and `vcpkg` (Windows builds only).
  - `libsqlite3-sys` compiles the vendored amalgamation (public domain) with `cc`,
    which the workspace already locks.
  - Nothing is fetched at build time, and none of the 8 is a proc-macro.
  - heed would add about 15 crates, including `bincode` 1.x, the `phf_macros`
    proc-macro and a `doxygen-rs` build dependency.

## 4. Schema

**Files.** The index is `cache/index-v<schema_version>-<generation>.sqlite`, with its
`-wal` and `-shm` files. `cache/` already holds a `.gitignore` of `*`. The
generation starts at 1, and a binary uses the highest generation of its own
schema version (section 6, Rebuilds).

```sql
CREATE TABLE obj    (seq INTEGER PRIMARY KEY,        -- local insertion order, parents first
                     id BLOB NOT NULL UNIQUE,        -- 32 bytes; prefix lookups are range scans
                     kind INTEGER NOT NULL,          -- claim, artifact, relation, evidence, retract
                     author INTEGER NOT NULL,        -- -> agent.n
                     ts_ms INTEGER NOT NULL);        -- advisory
CREATE TABLE agent  (n INTEGER PRIMARY KEY, key BLOB NOT NULL UNIQUE);
CREATE TABLE parent (child INTEGER, parent INTEGER, PRIMARY KEY (child, parent)) WITHOUT ROWID;
CREATE TABLE child  (parent INTEGER, child INTEGER, PRIMARY KEY (parent, child)) WITHOUT ROWID;
CREATE TABLE head   (seq INTEGER PRIMARY KEY);
CREATE TABLE tag    (tag TEXT, seq INTEGER, PRIMARY KEY (tag, seq)) WITHOUT ROWID;
CREATE INDEX obj_kind   ON obj(kind, seq);
CREATE INDEX obj_author ON obj(author, seq);
-- rkind: a relation kind, evidence pass, evidence fail, or retract
CREATE TABLE ref    (target INTEGER, rkind INTEGER, src INTEGER,
                     PRIMARY KEY (target, rkind, src)) WITHOUT ROWID;
CREATE TABLE fwd    (src_end INTEGER, rkind INTEGER, rel INTEGER, to_end INTEGER,
                     PRIMARY KEY (src_end, rkind, rel)) WITHOUT ROWID;     -- relations by `from`
CREATE TABLE hidden (seq INTEGER PRIMARY KEY);       -- what --current hides
CREATE TABLE shard  (n INTEGER PRIMARY KEY,          -- 0..255; no row means dirty
                     mtime_ns INTEGER NOT NULL, ctime_ns INTEGER NOT NULL, ino INTEGER NOT NULL);
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);
                     -- schema_version, ground id, the file's own inode, hidden_stale
```

**What each insert maintains.** A contribution is inserted in one statement group
whether it comes from a write, from freshness or from a rebuild:

- `obj`, `agent`;
- `parent` and `child`;
- `head`: its parents leave and it joins;
- `tag` for a claim;
- `ref` for a relation (both ends), evidence or a retraction, and `fwd` for a
  relation.

A parent or referenced contribution missing from the index means rebuild.

`hidden` is the one table not maintained row by row. A supersession or retraction
can hide or unhide contributions anywhere above it, and maintaining that
incrementally is Phase 4 (research B R2). In PR 2, any change recomputes `hidden`
whole from `CurrentView`, and only when the index holds exactly what the store
holds; otherwise `hidden_stale` is set until a later refresh. PR 3 decides
hidden-ness per answered row instead, by `HiddenOne`'s rule over `ref`.

## 5. Freshness after a git pull (G5)

Objects can arrive outside Bracken: a git pull, a checkpoint, another tool, or an
older `bracken` binary. Each open finds them as follows.

1. Stat `objects/`'s shard directories (0.45 ms), and compare each shard's
   (mtime, ctime, inode) with the `shard` table.
   - Adding, renaming or removing a file changes its directory's times on every
     POSIX filesystem.
   - A shard without a row, or a row without a shard, is dirty.
   - ctime can't be set from user space, so a `touch -m` that resets mtime is
     still caught.
2. List only the dirty shards, and diff each one against the index's id range for
   that prefix. (In PR 2 the store has already listed and checked every object, so
   the diff runs against the store's ids.)
3. Read and fully check each file the index lacks, then index it in the same
   transaction. The check covers its signature and every protocol rule, as
   `Store::open_trusting_with` checks objects missing from the snapshot today.
   - A file that fails its check refuses the open, as today, and no stamp is
     recorded. git checkout writes files in place, so a concurrent open can see a
     truncated one.
   - An indexed id whose file is gone (a checkout that went backwards) means
     rebuild, as a missing object sets the snapshot aside today. An indexed id
     this store didn't list, whose file exists, was indexed by another process
     after this one listed. That's not an error.
4. Record a shard's stamp only when both of these hold:
   - the stat was taken before the listing the index was reconciled with;
   - its mtime and ctime are more than 2 s older than **the filesystem's own
     clock**: the mtime of `cache/clock`, a small file the refresh rewrites
     after its listing.

   This is git's racy-index rule, which compares an entry's mtime with the index
   file's own mtime. Both times come from the same filesystem clock, so skew
   between this machine and a network filesystem's server can't break it. The 2 s
   margin covers coarse timestamps: Linux's coarse clock, and 1–2 s filesystems.

   If `cache/clock` can't be written, or `cache/` and `objects/` are on different
   devices, no stamp is recorded. Every shard then stays dirty, which is slower
   but never wrong.

There is no git hook. Clones, checkpoints, other tools and older binaries are all
caught the same way.

## 6. Writes, crashes, rebuilds and concurrency

**Order.** A write follows three steps:

1. Write the object to a temporary file, fsync it and rename it into its shard,
   as today.
2. Fsync the shard directory, and `objects/` when the shard was new. Today's
   `write_object` doesn't; PR 2 adds it (`crates/manyroot/src/fsx.rs`).
3. Commit the index transaction (`synchronous=NORMAL`).

That transaction also deletes the shard's `shard` row, so the next open lists
that shard again. The rows a write adds therefore never rest on a stamp that a
power loss could put back.

**Process crash.** A crash between steps 1–2 and step 3 leaves an object the
index lacks, in a shard whose stat has changed. The next open finds it, checks it
and indexes it.

**Power loss.** After step 2, the object is durable before its row can be. The
`shard` row deletion holds even if step 2 were lost: if the index transaction
survived and the rename didn't, the shard has no row, so it is listed, the id is
found missing, and the index is rebuilt.

**The index never names an object that isn't on disk without that shard being
dirty.**

A crash inside the index transaction is SQLite's to recover: the WAL rolls back
an uncommitted transaction. With `NORMAL`, a power loss can drop the last
committed transactions, and freshness finds those objects again.

**Rebuilds.** A rebuild never unlinks or renames a database that may be open. That
is SQLite's documented corruption case.

- A rebuild runs in place, in one `BEGIN IMMEDIATE` transaction: drop the tables,
  recreate them, refill them from the store. Concurrent readers see the old index
  or the new one, never a half-built one.
- It happens when:
  - the ground id differs;
  - the file's own inode differs from the one in `meta` (see section 7);
  - a parent is missing;
  - an indexed object is gone.
- A file SQLite reports as not a database, or corrupt, isn't touched. The next
  generation's file (`-<g+1>`) is created beside it and used from then on.
  Processes that still hold the old one keep their handles, and nothing is
  unlinked under them. `bracken verify` may delete older generations only when it
  can take an exclusive lock on each; otherwise it leaves them.
- If opening or rebuilding fails, the command reads through the full path, as
  today. It never loops.

**Schema versions.** Each binary opens only its own schema version's files.

- An older binary never sees a newer schema's file. It never rebuilds over it and
  never writes to it.
- Two versions on one Ground keep two indexes, each current by freshness, without
  rebuilding each other's.
- A `schema_version` inside a file that disagrees with its name is damage, and
  gets a new generation.

**Integrity checks.** `PRAGMA quick_check` costs time in proportion to the file,
about 194 MiB at 1M, so it runs only in `bracken verify` and after a rebuild,
never on an ordinary open. Damage found on an ordinary open shows up as
SQLite's corruption error, which moves to a new generation as above.

**Concurrency.** WAL readers each read a consistent snapshot while one writer
commits, and `busy_timeout` serialises writers.

A reader that finds dirty shards takes the write lock (`BEGIN IMMEDIATE`) and
repeats the check inside it, so two readers don't index the same objects twice.
A lock not granted within the timeout skips the index for that command; it is
never taken as damage.

**Long-lived processes.** `bracken view`, `watch` and `serve` re-run the freshness
check each time they reopen the store.

## 7. Trust and the oracle

The index has the snapshot's trust model (decision `96e282819607`,
[snapshot.rs](../../crates/bracken/src/snapshot.rs) header):

- it is this replica's own cache of its own verification;
- anyone who can write `cache/` can already write the replica;
- an object's bytes overwritten in place go unnoticed, as with the snapshot.

**Bound to its own file.** `meta` records the inode of the file the index was
built in. A file that arrives another way gets a new inode: a tracked
`cache/index-*.sqlite` force-added to git and checked out, or a copied Ground.
Its recorded inode doesn't match, so it's rebuilt from the store before any
answer is used. The snapshot has the same gap today; it is blocked in this
repository by ground-guards guard 1, but not in other repositories.

**Opening the file.**

- `cache/` is checked to be a real directory, not followed through a link, as
  today.
- The database is opened with `SQLITE_OPEN_NOFOLLOW`. SQLite then refuses a link
  in any component of the path, and macOS's `/var` and `/tmp` are links, so the
  replica's path is canonicalised before opening.

**Oracle.**

- `bracken verify` rebuilds the index.
- With `BRACKEN_FULL_CHECK`, every read does a full `Store::open`, which stays
  the oracle. Then:
  - an index is built fresh in memory and compared with it, which checks the
    derivation;
  - the replica's own index is brought up to date inside a transaction that is
    rolled back, and compared too, which checks what freshness and the writes
    kept.

  A disagreement fails the command, and nothing is written.
- PR 3 adds a gate check that makes the same comparison on the main Ground.
- The snapshot is retired only if that equality holds on every test.

## 8. Compatibility

There is no migration step.

- An older binary, the pinned one or the team server's, ignores the index files,
  and freshness (section 5) catches the objects it writes.
- The first open of an existing Ground with the new binary builds the index. At
  14k that takes about 0.1 s on top of today's full open.

PR 2 tests these cases:

- an object added with no index update is found by freshness;
- a crash between the object write and the index commit is repaired on the next
  open;
- a write's transaction leaves its shard without a row;
- stamps are judged against the filesystem's clock, not this process's;
- two schema versions on one Ground don't rebuild each other's index;
- a rebuild, or a move to a new generation, while another connection is open
  leaves both connections answering what the full computation gives;
- a linked or foreign index file is never written through or trusted.

## 9. Exit metric

From [layers.md](layers.md) Phase 3:

- equality with the full recompute on every gate run for two weeks;
- reads under 50 ms at 14k and on a synthetic 1M Ground.

Each PR reports those numbers.
