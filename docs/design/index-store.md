# The index store

> **Decided, 2026-11-23.** SQLite in WAL mode, through `rusqlite` 0.43 (features
> `bundled` and `fallible_uint`), in `crates/bracken` next to `snapshot.rs`
> (decision `f294933b4244`, answer `76aa001b623d`). This note is PR 1 of task
> `7d140f132b1a`, which builds Phase 3 of [layers.md](layers.md) (owner decision
> `817afe6653a8`, item 2). The measurements are in note `799ce5d903e9`. PR 2 adds the
> index as a cache behind the existing read API. PR 3 serves reads from it.
> Rounds 2 and 3 revised sections 3–8 for the review findings (section 10).

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
- **Safety.** `SQLITE_OPEN_NOFOLLOW` covers the index file and its `-wal` and
  `-shm`. LMDB needs a map size chosen up front, a reader-slot table cleaned
  after a crashed reader, and an `unsafe` open (heed's), where a second open in
  one process is undefined. SQLite needs none of these.
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

**Files.** The index lives in `cache/index-v<schema_version>-<generation>.sqlite`,
with its `-wal` and `-shm` files.

- The generation starts at 1. A binary uses the highest generation of its own
  schema version (section 6, Rebuilds).
- The index creates `cache/` and its `.gitignore` of `*` if they're missing, as the
  snapshot does today, so retiring the snapshot leaves them in place.

```sql
CREATE TABLE obj    (seq INTEGER PRIMARY KEY,        -- stable row key; insertion order, parents first
                     pos INTEGER NOT NULL UNIQUE,    -- Graph::topo_order position (see below)
                     id BLOB NOT NULL UNIQUE,        -- 32 bytes; prefix lookups are range scans
                     kind INTEGER NOT NULL,          -- claim, artifact, relation, evidence, retract
                     author INTEGER NOT NULL,        -- -> agent.n
                     ts_key INTEGER NOT NULL,        -- timestamp_ms XOR 2^63, as i64 (see below)
                     ck BLOB NOT NULL);              -- row checksum (section 6)
CREATE TABLE agent  (n INTEGER PRIMARY KEY, key BLOB NOT NULL UNIQUE);
CREATE TABLE parent (child INTEGER, parent INTEGER, PRIMARY KEY (child, parent)) WITHOUT ROWID;
CREATE TABLE child  (parent INTEGER, child INTEGER, PRIMARY KEY (parent, child)) WITHOUT ROWID;
CREATE TABLE head   (seq INTEGER PRIMARY KEY);
CREATE TABLE tag    (tag TEXT, seq INTEGER, ck BLOB NOT NULL, PRIMARY KEY (tag, seq)) WITHOUT ROWID;
CREATE INDEX obj_kind   ON obj(kind, pos);
CREATE INDEX obj_author ON obj(author, pos);
-- rkind: a relation kind, evidence pass, evidence fail, or retract
CREATE TABLE ref    (target INTEGER, rkind INTEGER, src INTEGER, ck BLOB NOT NULL,
                     PRIMARY KEY (target, rkind, src)) WITHOUT ROWID;
CREATE TABLE fwd    (src_end INTEGER, rkind INTEGER, rel INTEGER, to_end INTEGER, ck BLOB NOT NULL,
                     PRIMARY KEY (src_end, rkind, rel)) WITHOUT ROWID;     -- relations by `from`
CREATE TABLE hidden (seq INTEGER PRIMARY KEY, ck BLOB NOT NULL);           -- what --current hides
CREATE TABLE shard  (n INTEGER PRIMARY KEY,          -- 0..255; no row means dirty
                     present INTEGER NOT NULL,       -- 0: reconciled as absent
                     mtime_s INTEGER, mtime_ns INTEGER, ctime_s INTEGER, ctime_ns INTEGER,
                     ino BLOB, dev BLOB);            -- u64s as 8 big-endian bytes
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);
                     -- schema_version, ground id, the file's own device and inode, hidden_stale, fs_clock
```

**Order.** Every answer that the full computation orders by `Graph::topo_order()`
(Kahn's algorithm, smallest id first among ready contributions) is ordered by
`pos`, which equals that position exactly. `seq` never orders output. `pos` is kept
exact like this:

- **Rebuild.** `pos` is the index in `topo_order()`.
- **Insert.** Kahn's state after emitting position *m* depends only on what was
  emitted, so inserting X changes no position up to *m* = max `pos` of X's
  parents. For several new contributions, *m* is the smallest such value over
  those whose parents are all already indexed. Positions after *m* are recomputed by running Kahn over the
  contributions after *m* plus the new ones, smallest id first, and only rows
  whose `pos` changed are rewritten. `pos` is `UNIQUE`, which SQLite checks row
  by row, so the rewrite takes two steps: the affected rows move to `-pos - 1`,
  then to their new values.
  - A contribution with no parents gives *m* = -1, so every position is
    recomputed. A new root is rare.
  - A local write's parents are every head of the store it was made in. When
    the index holds what that store holds, *m* is the last position, and X
    appends with no rewrite.
  - A pull of work concurrent with *S* local contributions rewrites at most *S*
    positions. A very old fork can rewrite up to N; that costs about what a
    rebuild costs, once.

**Integer widths.** SQLite integers are i64, and `fallible_uint` refuses a larger
u64.

- `timestamp_ms` is a u64 with no upper bound in the protocol, so it's stored as
  `ts_key = (timestamp_ms XOR 2^63) as i64`. That's lossless, and it orders as the
  u64 does, so `--since` comparisons hold for any value.
- Stat fields are stored whole: seconds and nanoseconds in separate columns, and
  inode and device as 8-byte big-endian blobs, compared only for equality.

**What each insert maintains.** A contribution is inserted the same way whether it
comes from a write, from freshness or from a rebuild. Inserting an id that is
already indexed is a no-op, so a write racing a freshness reconcile in another
process adds each row once.

- `obj` and `agent`;
- `parent` and `child`;
- `head`: its parents leave and it joins;
- `tag` for a claim: `INSERT OR IGNORE`, since a claim may repeat a tag;
- `ref` for a relation (both ends), evidence or a retraction, and `fwd` for a
  relation. A referrer's author comes from joining `obj` on `src`, and a
  relation's other end from `fwd`, which is what `CurrentView`'s `others` needs;
- `pos`, as above.

Contributions found together by freshness are inserted in causal order across
shards: Kahn over the new set, smallest id first. A parent or referenced
contribution missing from the index means rebuild.

`hidden` is the one table not maintained row by row. A supersession or retraction
can hide or unhide contributions anywhere above it, and maintaining that
incrementally is Phase 4 (research B R2).

- In PR 2, any change recomputes `hidden` whole from `CurrentView`, and only when
  the index holds exactly what the store holds. Otherwise `hidden_stale` is set
  until a later refresh.
- PR 3 decides hidden-ness per answered row, by `HiddenOne`'s rule over `ref`.

## 5. Freshness after a git pull (G5)

Objects can arrive outside Bracken: a git pull, a checkpoint, another tool, or an
older `bracken` binary. Each open finds them as follows.

**The rule for every open, freshness and rebuild alike:** *T* is read, and every
shard statted, before any listing of `objects/` or of a shard. That includes
`Store::open`'s own listing in PR 2. A stamp is only ever compared with, or
recorded from, a stat taken after *T* and before the listing it is reconciled
with.

1. **Read the filesystem's clock first.** Rewrite `cache/clock`, a small file,
   without fsync, and take its mtime as *T*.
   - This is git's racy-index rule, which compares an entry's mtime with a time
     from the same filesystem.
   - Taking *T* first, and from the filesystem rather than this process's wall
     clock, means neither a slow reconcile nor skew against a network
     filesystem's server can break it.
   - If `cache/clock` can't be written or read, this open records no stamps and
     treats every shard as dirty.
2. **Stat `objects/`'s directories** (0.45 ms). In PR 2 this happens before
   `Store::open` lists anything.
3. **Find the dirty shards.**
   - **Clock step back first.** If *T* is earlier than the `fs_clock` recorded in
     `meta`, the filesystem clock stepped back: every shard is dirty on this open,
     and every shard row is cleared, before any stamp is compared.
   - Otherwise, each of the 256 shards `00`–`ff` is decided. It is clean only if
     its row matches its current stamp (mtime, ctime, inode, device), or its row
     says absent and it is absent. Anything else is dirty: no row, a different
     stamp, a row for a shard that's gone, or a new shard.
   - ctime can't be set from user space, so a `touch -m` that resets mtime is
     still caught.
4. **List with `Store::open`'s own checks**, the code `crates/manyroot` exposes for
   this (unchanged; this work doesn't tighten `Store::open`). Those checks are:
   - **`objects/`**: opened without following a link. Its entries named with a dot
     are skipped. An entry that isn't UTF-8, isn't a directory, or is a link is
     `StoreError::Corrupt`.
   - **Any directory is accepted as a shard**, whatever its name. Its entries
     named with a dot (a write in flight) or not UTF-8 are skipped.
   - **Every other entry is read as an object.** A non-regular one (link,
     directory, FIFO) is refused, and so is a file whose path isn't its content's
     id.

   So freshness lists `objects/` itself on every open (257 entries, about 0.1 ms):
   - a stray file or symlinked shard refuses the open as `Store::open` does;
   - every dirty shard is listed;
   - every directory that isn't one of the 256 lowercase hex names is listed on
     every open. An empty one is accepted and any file in one refuses, exactly as
     `Store::open` does, because no such path can be an object's.

   Then each dirty shard's listing is diffed against the index's id range for that
   prefix. In PR 2 the store has already listed and checked every object, after
   step 2, so the diff runs against the store's ids.
5. **Index what the index lacks.** Each such file is read and fully checked, in
   causal order across shards, as `Store::open_trusting_with` checks objects
   missing from the snapshot today. The check covers its signature and every
   protocol rule.
   - **Make them durable first.** Before the transaction that adds them commits,
     fsync each added object file, each shard directory it was found in, and
     `objects/` if that shard is new. Another writer (git, an older binary,
     `serve`) may have renamed it in without a directory fsync, and the index must
     not commit a row, or a stamp, for a directory entry a power loss could take
     back.
   - A file that fails its check refuses the open, as today, and no stamp is
     recorded. git checkout writes files in place, so a concurrent open can see a
     truncated one.
   - An indexed id whose file is gone (a checkout that went backwards, or a shard
     removed whole) means rebuild, as a missing object sets the snapshot aside
     today.
   - An indexed id this store didn't list, whose file exists, was indexed by
     another process after this one listed. That's not an error, and inserting an
     id that's already indexed is a no-op, on every path.
6. **Record stamps and the clock.** In the same transaction, record `fs_clock` =
   *T*. A dirty shard's stamp, from step 2, is recorded only if its mtime and
   ctime are both more than 2 s older than *T*.
   - The margin covers coarse timestamps: Linux's coarse clock, and 1–2 s
     filesystems. Any write to that shard after step 2 lands at *T* or later, so it
     changes the stamp.
   - Otherwise the shard stays dirty, and the next open lists it again (5 ms at
     1M).
   - An absent shard is recorded as absent.
   - A rebuild follows the same steps. Its stamps come from step 2, taken before
     its own listing.

**Where stamps are recorded at all.** Only where the stamp is trustworthy:

- Unix;
- `cache/` and `objects/` on the same device;
- a local filesystem type from an allow-list: APFS, HFS+, ext4, xfs, btrfs, zfs and
  tmpfs, checked with `statfs`.

Network filesystems, FAT and exFAT, Windows and anything unknown record no stamps,
so every shard is always dirty. That's slower (a full listing, 18 ms at 14k) but
never wrong.

**Opens that can't write** never use the index at all. They read through the full
path, as today:

- `bracken view`, whose `Refresh::No` means it never writes;
- a replica whose `cache/` this process can't write.

Even `SQLITE_OPEN_READ_ONLY` writes on a writable `cache/`: SQLite recreates the
`-wal` and `-shm` files and writes read-marks to `-shm`. `immutable=1` would be
wrong for a file other processes change. So such an open never touches the index
files, and the clock-step check never needs a read-only form.

**What stays undetected:**

- an object's bytes overwritten in place (section 7);
- a write that the filesystem stamps with exactly a recorded (mtime, ctime) pair,
  after its clock went back and then forward again between two of this replica's
  opens. A step back that is still in effect is caught in step 3.

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
that shard again. The write path never records a stamp taken after its own
rename: that stamp could cover other processes' objects that this store never
listed.

**Process crash.** A crash between steps 1–2 and step 3 leaves an object the
index lacks, in a shard whose stat has changed. The next open finds it, checks it
and indexes it.

**Power loss.** After step 2, the object is durable before its row can be. The row
deletion holds even if step 2 were lost: if the index transaction survived and
the rename didn't, the shard has no row, so it is listed, the id is found
missing, and the index is rebuilt.

**The index never names an object that isn't on disk without that shard being
dirty.**

A crash inside the index transaction is SQLite's to recover: the WAL rolls back
an uncommitted transaction. With `NORMAL`, a power loss can drop the last
committed transactions, and freshness finds those objects again.

**Rebuilds.** A rebuild never unlinks or renames a database that may be open. That
is SQLite's documented corruption case.

- A rebuild, the first build included, runs in place in one `BEGIN IMMEDIATE`
  transaction: drop the tables, recreate them, refill them from the store, and
  write `meta` last but inside the same transaction.
  - Concurrent readers see the old index or the new one, never a half-built one.
  - A second process waiting on the lock rechecks `meta` once it holds the lock,
    and finds the index built.
  - A crash mid-rebuild rolls back, and the next open rebuilds.
- It happens when:
  - the file has no schema yet;
  - the ground id differs;
  - the file's own device or inode differs from the one in `meta` (section 7);
  - a parent or referenced contribution is missing;
  - an indexed object is gone.
- **SQLITE_CORRUPT or SQLITE_NOTADB**, from any statement, means the file isn't
  touched again. The next generation's file (`-<g+1>`) is created beside it and
  used from then on.
  - Processes still holding the old file keep their handles, and nothing is
    unlinked under them.
  - `bracken verify` deletes older generations only when it can take an exclusive
    lock on each; otherwise it leaves them.
- **SQLITE_BUSY** (a lock not granted within `busy_timeout`) is never taken as
  damage. The command skips the index and reads through the full path.
- **Any other failure** to open or rebuild also falls back to the full path. It
  never loops.

**Schema versions.** Each binary opens only its own schema version's files.

- An older binary never sees a newer schema's file. It never rebuilds over it and
  never writes to it.
- Two versions on one Ground keep two indexes, each current by freshness, without
  rebuilding each other's.
- A `schema_version` inside a file that disagrees with its name is damage, and
  gets a new generation.

**Integrity checks.**

- `PRAGMA quick_check` costs time in proportion to the file, about 194 MiB at 1M,
  so it runs only in `bracken verify` and after a rebuild, never on an ordinary
  open.
- `PRAGMA cell_size_check=ON` is set on every connection, which is cheap. Damage
  that SQLite sees surfaces as SQLITE_CORRUPT and moves to a new generation, as
  above.
- **Row checksums** (decision `c489f7f9dea9`). SQLite keeps no page checksums,
  so every `obj`, `tag`, `ref`, `fwd` and `hidden` row carries `ck`: the first
  8 bytes of a blake3 hash over its other fields, encoded in a fixed order.
  - A rewrite of `pos` rewrites that row's `ck`.
  - Every row a read returns is checked against its `ck`.
  - A mismatch rebuilds the index in place, since the file itself is sound
    (SQLITE_CORRUPT moves to a new generation instead, as above), and that read
    falls back to the full path.

  This keeps the snapshot's guarantee that a damaged byte is detected and
  rebuilt (`crates/bracken/tests/snapshot.rs`), at a cost of one hash per returned
  row. It isn't meant to resist forgery: anyone who can write `cache/` can
  recompute it (section 7).
- **Residual, accepted.** Structural damage that drops rows from a table or a
  secondary index, so a read never returns them, isn't caught by a row check.
  What catches it:
  - `BRACKEN_FULL_CHECK`, which compares every table and the order with the
    recompute;
  - the gate's check on the main Ground (PR 3);
  - `bracken verify`'s rebuild.

**Concurrency.** WAL readers each read a consistent snapshot while one writer
commits, and `busy_timeout` serialises writers. A reader that finds dirty shards
takes the write lock (`BEGIN IMMEDIATE`) and repeats the check inside it, so two
readers don't index the same objects twice.

**Read-only opens.** `bracken view` and replicas whose `cache/` this process can't
write never touch the index files. They read through the full path (section 5,
Opens that can't write).

**`bracken serve`** adds contributions through `Store::import`, outside
`Refreshing`. Those objects are found by freshness on the next open, by serve or
by anyone else. PR 3, which reads through the index, adds the write-path commit
after serve's imports, under the same order as above.

## 7. Trust and the oracle

The index has the snapshot's trust model (decision `96e282819607`,
[snapshot.rs](../../crates/bracken/src/snapshot.rs) header):

- it is this replica's own cache of its own verification;
- anyone who can write `cache/` can already write the replica;
- an object's bytes overwritten in place go unnoticed, as with the snapshot.

**Bound to its own file.** `meta` records the device and inode of the file the
index was built in. A file that arrives another way gets a new inode: a tracked
`cache/index-*.sqlite` force-added to git and checked out, or a copied Ground.
Its recorded device and inode don't match, so it's rebuilt from the store before any
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

  The comparison covers every table (objects, parents, children, heads, tags,
  references, relations by `from`, hidden) and the order (`pos` against
  `topo_order()`). A disagreement fails the command, and nothing is written.
- PR 3 adds a gate check that makes the same comparison on the main Ground.
- The snapshot is retired only if that equality holds on every test.

## 8. Compatibility and tests

There is no migration step.

- An older binary, the pinned one or the team server's, ignores the index files,
  and freshness (section 5) catches the objects it writes.
- The first open of an existing Ground with the new binary builds the index. At
  14k that takes about 0.1 s on top of today's full open.
- In PR 3, printing a row's statement or payload still reads its object file:
  about 145 µs per file warm, so about 7 ms for 50 rows. The section 2 timings
  leave that out.

Each property maps to a test, in PR 2 unless marked PR 3:

| Property | Test |
|---|---|
| Answers equal the full computation | the full-check comparison on generated Grounds after each operation below, every table and `pos` |
| Order across replicas | two replicas with concurrent work, pulled both ways: `pos` equals `topo_order()` on both; a pull that interleaves rewrites only the suffix; a new root recomputes all; an id already indexed is inserted once |
| Freshness: an object with no index update | an object written through `Store` alone (an older binary) is found |
| Freshness: removal | a head object deleted; a shard removed whole while unrecorded: both rebuild |
| Freshness: open's checks | matched to `Store::open` case by case: a stray file or symlinked shard in `objects/` refuses; an empty non-hex or uppercase directory is accepted and a file in one refuses; a link, directory, FIFO or misnamed file in a shard refuses (PR 3, when freshness lists) |
| T before any listing | an object written between the store's listing and the stamps would be recorded clean if stats came after the listing; with *T* and the stats first, the next open finds it |
| Freshness durability | objects found by freshness have their files and shard directories fsynced before the commit (counted through a test hook) |
| Racy window | a shard written within 2 s of *T* isn't recorded; a write after the listing in the same tick is found |
| Clock | stamps are judged against `cache/clock`'s mtime: a local clock offset changes nothing; with `fs_clock` ahead of *T*, every shard is dirty on that same open, before any stamp is compared; a `cache/clock` that can't be written records no stamps |
| Filesystems | an unlisted filesystem type records no stamps |
| mtime reset | a shard whose mtime is put back is still dirty (ctime) |
| Write order | a write's transaction leaves its shard without a row; the shard directory is fsynced |
| Process crash | a store dropped after the object write and before the index commit (and a real abort, through a debug-only hook) is repaired on the next open |
| Crash mid-rebuild | a rebuild rolled back leaves the old index or none, never half of one |
| Concurrency | several processes refreshing a stale, missing or damaged index at once agree, and each object is added once |
| Rebuild under an open connection | a rebuild, or a move to a new generation, while another connection reads: both answer what the full computation gives |
| Damage | a file that isn't a database moves to a new generation; a schema version or ground mismatch rebuilds; SQLITE_BUSY falls back without a rebuild |
| Value corruption | a byte changed in an `obj`, `tag`, `ref`, `fwd` or `hidden` row fails that row's checksum on read, rebuilds the index, and the read's answer equals the full computation's (as `tests/snapshot.rs` does for the snapshot); a row deleted with a clean stamp fails the full check |
| Schema versions | two schema versions alternating on one Ground don't rebuild each other's file |
| Widths | `timestamp_ms` = u64::MAX indexes, and orders after every smaller value |
| Read-only | `bracken view` and a replica with a read-only `cache/` leave every file under `cache/` unchanged, index files included, and answer as the full path does |
| Foreign file | a linked index file is never written through; a copied or checked-out index (another inode) is rebuilt |
| Found objects are checked | a forged or orphan object arriving by freshness refuses the open as `Store::open_trusting_with` does (PR 3) |
| serve | objects serve imports are found by the next open (PR 2); serve's write-path commit (PR 3) |

## 9. Exit metric

From [layers.md](layers.md) Phase 3:

- equality with the full recompute on every gate run for two weeks;
- reads under 50 ms at 14k and on a synthetic 1M Ground.

Each PR reports those numbers.

## 10. Review findings

| Finding | Handled in |
|---|---|
| `45cb95cc39a7` order: `seq` isn't `topo_order` | section 4, Order: `pos` equals `topo_order()` exactly, maintained by suffix recompute; `seq` never orders output |
| `baeb35cd5483`, `de4b1cb363b6` racy margin, clock skew, non-POSIX filesystems | section 5 steps 1 and 6: *T* from the filesystem, taken before the stats; a clock step back clears the rows; stamps only on allow-listed local filesystems |
| `41f111d3222e` stray entries, non-regular or misnamed files, absent shards | section 5 steps 2–4: `objects/` and dirty shards listed with the store's own checks on every open; all 256 shards decided, absent ones included |
| `155f7566cac1`, `2538f1286416` rebuild protocol, newer schema | section 6, Rebuilds and Schema versions: in place in one transaction, never unlinking an open file; per-version files; BUSY isn't damage |
| `1128ae98db65`, `23579eecd2a7` power loss | section 6, Order: shard directory fsync before the index commit, and the shard's row deleted in that transaction |
| `21fedaad26ab` integer widths | section 4, Integer widths: `ts_key` order-preserving, stat fields whole |
| `3f3b3d278d99` quick_check, value corruption | section 6, Integrity checks: when quick_check runs; SQLITE_CORRUPT means a new generation; per-row checksums checked on every returned row (decision `c489f7f9dea9`); dropped rows caught by the full check, the gate and verify |
| `5947f43b4b64` read-only opens, serve | section 6, Read-only opens and `bracken serve` |
| `6e6530271736` tests | section 8's table |

The reviews' notes are taken too. Section 3's LMDB and SQLite sentence is
corrected. Section 4 covers `cache/.gitignore`, `INSERT OR IGNORE` for tags,
referrer authors through the `obj` join, and causal order across shards. Section 6
says the write path records no stamp. Section 8 covers object reads for printed
rows. Section 7 canonicalises the path and binds the file's inode. Section 5
covers found objects that fail their check. Section 6 says when quick_check
runs. PR 2's body lists the Cargo.lock diff (answer `d3363867edca`).

Round 2, handled in round 3:

| Finding | Handled in |
|---|---|
| `1007a1bf46b4` freshness indexes an object whose directory entry isn't durable | section 5 step 5: added files, their shard directories and a new shard's `objects/` are fsynced before the commit |
| `1701912e08db` in PR 2 the store lists before *T* and the stamps | section 5, the rule for every open: *T* and the stats come before any listing, `Store::open`'s included; rebuilds too (step 6) |
| `64a69d0334dc` the clock step-back check runs too late | section 5 step 3: checked before any stamp is compared, so every shard is dirty on that same open |
| `3674e5ee100b` read-only opens still write | section 5, Opens that can't write: view and read-only replicas never touch the index files |
| `5e280b92e7c0` `Store::open` doesn't check shard names | section 5 step 4 states `Store::open`'s real checks; non-hex directories are listed on every open, so empty ones pass and files in them refuse, as `Store::open` does |

Round 2's notes are taken as well:

- section 6's accepted-risk line is replaced by the row checksums (decision `c489f7f9dea9`);
- the two-step `pos` rewrite and a new root's full recompute (section 4);
- inserting an indexed id is a no-op (sections 4 and 5);
- `meta` binds the device as well as the inode (section 7);
- a rebuild's stamps come before its own listing (section 5 step 6);
- no stamps if `cache/clock` fails (section 5 step 1).
