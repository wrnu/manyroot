# The index store

> **Decided, 2026-11-23.** SQLite in WAL mode, through `rusqlite` 0.43 (features
> `bundled` and `fallible_uint`), in `crates/bracken` next to `snapshot.rs`
> (decision `f294933b4244`, answer `76aa001b623d`). This note is PR 1 of task
> `7d140f132b1a`, which builds Phase 3 of [layers.md](layers.md) (owner decision
> `817afe6653a8`, item 2). The measurements are in note `799ce5d903e9`. PR 2 adds the
> index as a cache behind the existing read API. PR 3 serves reads from it.

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

The index lives in `cache/index.sqlite`, with its `-wal` and `-shm` files. `cache/`
already holds a `.gitignore` of `*`.

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
CREATE TABLE hidden (seq INTEGER PRIMARY KEY, reasons INTEGER NOT NULL);   -- what --current hides
CREATE TABLE shard  (n INTEGER PRIMARY KEY,          -- 0..255
                     mtime_ns INTEGER, ctime_ns INTEGER, ino INTEGER);
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB);    -- schema_version, ground id, objects/ stat
```

The index is rebuilt from `objects/` in any of these cases:

- `schema_version` or the ground id differs from what this binary expects;
- the open fails;
- `PRAGMA quick_check` fails.

`hidden` must equal `CurrentView`'s hidden set. PR 2's equality tests compare the
two. This note's benchmark used a simpler approximation.

## 5. Freshness after a git pull (G5)

Objects can arrive outside Bracken: a git pull, a checkpoint, another tool, or an
older `bracken` binary. Each open finds them as follows.

1. Stat `objects/` and its 256 shard directories (0.45 ms), and compare each
   shard's (mtime, ctime, inode) with the `shard` table.
   - Adding, renaming or removing a file changes its directory's times on every
     POSIX filesystem.
   - A new shard changes `objects/`.
   - ctime can't be set from user space, so a `touch -m` that resets mtime is
     still caught.
2. List only the changed shards, and diff each one against the index's id range
   for that prefix.
3. A file the index lacks is read and fully checked, then indexed in one
   transaction. The check covers its signature and every protocol rule, as
   `Store::open_trusting_with` checks objects missing from the snapshot today.
   An indexed id whose file is gone (a checkout that went backwards) means
   rebuild, as a missing object sets the snapshot aside today.
4. Record a shard's stat only when both of these hold:
   - the stat was taken before the listing the index was reconciled with;
   - its times are more than 2 s older than the clock. This is git's racy-index
     rule, and it covers coarse timestamps: Linux's coarse clock, and 1–2 s
     filesystems.

   Otherwise the shard stays dirty, and the next open lists it again (5 ms at 1M).

There is no git hook. Clones, checkpoints, other tools and older binaries are all
caught the same way.

## 6. Writes, crashes and concurrency

**Order.** A write fsyncs and renames its object first, as today. Then it commits
the index transaction (with `synchronous=NORMAL`).

**Crash.** If the process dies between those two steps, an object exists that the
index lacks. Its shard's stat has changed, so the next open finds the object,
checks it and indexes it. The index never names an object that wasn't written.

A crash inside the index transaction is SQLite's to recover: the WAL rolls back
an uncommitted transaction. With `NORMAL`, a power loss can drop the last
committed transactions. Those objects are then found by freshness in the same
way.

**Concurrency.** WAL readers each read a consistent snapshot while one writer
commits, and `busy_timeout` serialises writers.

A reader that finds dirty shards takes the write lock (`BEGIN IMMEDIATE`) and
repeats the check inside it, so two readers don't index the same objects twice.

**Long-lived processes.** `bracken view`, `watch` and `serve` keep one connection
and re-run the freshness check on each refresh.

## 7. Trust and the oracle

The index has the snapshot's trust model (decision `96e282819607`,
[snapshot.rs](../../crates/bracken/src/snapshot.rs) header):

- it is this replica's own cache of its own verification;
- anyone who can write `cache/` can already write the replica;
- an object's bytes overwritten in place go unnoticed, as with the snapshot.

**Opening the file.** `cache/` is checked to be a real directory, not followed
through a link, as today. The database is opened with `SQLITE_OPEN_NOFOLLOW`.

**Oracle.**

- `bracken verify` rebuilds the index.
- With `BRACKEN_FULL_CHECK`, every read does a full `Store::open`, which stays
  the oracle, and compares the index's answers with it.
- PR 3 adds a gate check that makes the same comparison on the main Ground.
- The snapshot is retired only if that equality holds on every test.

## 8. Compatibility

There is no migration step.

- An older binary, the pinned one or the team server's, ignores
  `cache/index.sqlite`, and freshness (section 5) catches the objects it writes.
- The first open of an existing Ground with the new binary builds the index. At
  14k that takes about 0.1 s on top of today's full open.

PR 2 tests both cases:

- an object added with no index update is found by freshness;
- a crash between the object write and the index commit is repaired on the next
  open.

## 9. Exit metric

From [layers.md](layers.md) Phase 3:

- equality with the full recompute on every gate run for two weeks;
- reads under 50 ms at 14k and on a synthetic 1M Ground.

Each PR reports those numbers.
