# Performance and scaling: the contribution graph and the code store

> Date: 2026-11-21. Part of the November 2026 research behind [docs/design/layers.md](../../design/layers.md).
> Question: How do the contribution graph and the code store perform today, where do they stop scaling (towards 1M and 100M contributions), and what fixes it, with or without a format change?

Researcher B, 2026-11-21 (expanded edition). Sources are linked next to claims. "Snippet" marks a source I saw only in a search summary; "(own knowledge)" marks what I didn't re-check.

**How I measured.** Everything was read-only:
- **Team Ground:** read commands through `scripts/bk` with the pinned binary on the local replica, with no pull, on an M1 Max.
- **Throwaway Grounds** under `$TMPDIR/bracken-scratch/research-b`:
  - a 2,000-contribution Ground to trace sync bytes through `sync --exec` with a `tee` wrapper;
  - a 20,000-contribution on-disk Ground built with the `manyroot` crate and read with the pinned binary;
  - an in-memory benchmark at 10k–200k contributions, built against `crates/manyroot` with the team's mix of kinds: 30% claims, 45% evidence, 23% relations, 2% retracts.

Nothing in the repository was modified.

## Summary

1. **Every command rebuilds the world.** A read decodes all contributions from a 16 MB snapshot, rebuilds the graph and recomputes views. A write appends one file, then rewrites the whole snapshot. Reads take 0.26–0.64 s at 14k contributions and use about 70 MB of RAM. Every operation is O(N) or worse; nothing is persistent except the objects and that snapshot.
2. **Acceptance under `follow_changes` grows about quadratically.** Measured: 0.2 s at 10k, 0.8 s at 50k, 4.7 s at 100k, 13.7 s at 150k, 17.9 s at 200k (exponent ≈ 2.2). That extrapolates to roughly 8–15 minutes at 1M and is infeasible at 100M. This is exactly the needs_recheck and retraction-cascade rule at the heart of the scientific-method model.
3. **Sync's O(N) inference is confirmed by trace.** If a side doesn't hold every one of the peer's heads, it offers every id it holds outside the ancestry of the heads it does hold. After one local write on a 2,001-contribution Ground, the server sent a 68 KB offer of all 2,001 ids; with both sides diverged, both sent about 68 KB. On the team Ground that is about 0.5 MB per write or per pull with news; 34 MB at 1M; 3.4 GB at 100M.
4. **The fix is the one every large immutable store uses:** immutable objects plus persistent derived indexes, updated incrementally. That is git's commit-graph, midx and bitmaps, Datomic's EAVT/AEVT/AVET/VAET, Mononoke's and Scion's "derived data", and TigerBeetle's LSM forest. Our rule that validity depends only on a contribution and its parents is precisely what makes this possible.
5. **Ranked changes:**
   - (R1) a persistent index store, sketched below in SQLite;
   - (R2) incremental acceptance, current view and needs_recheck, with a chain-cover reachability index. A new change makes the affected subjects needs_recheck with **no reachability query at all**, because no existing pass can have seen a new change;
   - (R3) a sync "bookmark" fix with no protocol change, then range-based set reconciliation;
   - (R4) packed objects and parallel verification;
   - (R5) cheap hot-spot fixes now.
6. **Frozen-format decisions to take before data accumulates:**
   - out-of-line payloads (an artifact holds a content hash);
   - an optional validated `height`;
   - a fixed signature-verification equation (strict now, so no batch verification);
   - header fields before the body;
   - possibly a **batch** contribution, one signature over many records, for ledger- and knowledge-graph-scale ingest.

   Indexes, packs, caches and Weave's layouts stay implementation detail.
7. **Non-code workloads raise the bar.**
   - **Ledgers** need aggregate indexes (balances) and ingest at 10⁴–10⁶ records a second. Per-record ed25519 verification caps ingest at about 10–25k a second per core.
   - **Knowledge graphs** reach 10¹⁰ facts. Wikidata, at more than 16 billion triples, had to split its query graph in 2025.
   - **Citation and retraction cascades** are reverse-reachability over very large fan-in. About 25% of citations to retracted papers come after the retraction.

   These argue for a structured-fields body, per-Ground declared indexes and propagation rules (the configurable layer), and lazy, push-marked invalidation.
8. **Staged plan.** Days: hot spots and the sync bookmark. Weeks: index store as a rebuildable cache, verified against the current full recompute. Then incremental acceptance, packs, and finally a protocol v2 with the format items above, behind owner approval.

## 1. Our system today

### 1.1 Layout, encoding, open path

- **Objects.** One file per contribution: `objects/ab/<62 hex>` (`crates/manyroot/src/store.rs:1-20`, `object_path` store.rs:619). Each is written by a temp file, fsync and rename (`write_object` store.rs:630).
- **Encoding.** Canonical CBOR of `ContributionContent` (`protocol`, `ground`, `author`, `parents`, `timestamp_ms`, `body`) plus the signature. The id is the blake3 hash of the content. Decode re-encodes to check canonical form (`contribution.rs:208-217`). Signatures are checked by `verify_strict` over `"manyroot/v1/contribution:" || id` (`identity.rs:163-168`, protocol.md:41).
- **In-memory graph.** `Graph` holds `contributions`, `children`, `referrers` and `heads` HashMaps and a lazily computed retracted set (`graph.rs:81-90`). It is rebuilt on every process start.
- **Open.** `Store::open` reads, decodes, verifies and inserts every object. `Store::open_trusting` (store.rs:145-250) takes the verified snapshot (`crates/bracken/src/snapshot.rs`): one file with every contribution's encoding and a blake3 checksum. It lists all of `objects/` (`object_files` store.rs:561), decodes the snapshot (re-encoding and hashing each), and replays Kahn's algorithm over everything. Only objects missing from the snapshot are read and verified.
- **Snapshot rewrite.** After any command that added contributions, `Refreshing::drop` rewrites the whole snapshot (snapshot.rs, `impl Drop for Refreshing`).
- **Queries:**
  - `log --tag`: a filter over all N; tags live inside claim bodies.
  - `--current`: `CurrentView::new` over the full topological order (`crates/bracken/src/main.rs:2008`).
  - `show <id>`: builds `topo_order()` and `CurrentView` twice (main.rs:847, 852), then `policy.assess`.
  - `--since <cursor>`: a full ancestry walk of the cursor (`listing.rs:28-45`).
  - Acceptance: `Assessor` (validation.rs:112-235). With `follow_changes`, `Seen::sweep` (validation.rs:248-290) makes a pass over the whole order for every 64 changes, holds a bitset per pass, and runs Tarjan's algorithm over the dependency relations.
  - `evaluate` also computes `conflicts` over the full order (validation.rs:536).
- **Long-lived processes.** `bracken view` (view.rs) and `watch` keep one store in memory and reopen when the object count changes, so they amortise the open but still recompute views.

### 1.2 Sync, confirmed

`Session::plan` (sync.rs:424-436):

```rust
let held: Vec<ContributionId> = peer_heads.iter().filter(|h| graph.contains(h)).copied().collect();
let known = ancestry(graph, &held);
let candidates: Vec<ContributionId> = graph.topo_order().into_iter().filter(|id| !known.contains(id)).collect();
if held.len() == peer_heads.len() { /* send exactly the difference */ }
/* else */ Ok(Step { send: split(&candidates, MAX_IDS, |ids| Message::Offer { ids }), .. })
```

`known` is built only from peer heads this side already holds. After a local write, the writer's only head is its new contribution, which the server lacks, so `held` is empty, `known` is empty, and the server offers **all** its ids. Symmetrically, after the server gains anything the puller lacks, the puller offers all of its ids.

**Trace.** Two throwaway replicas of 2,000 claims, synced with `bracken sync --exec wrap.sh` (a `tee` around `serve --stdio`):

| Case | Client → server | Server → client |
|---|---|---|
| Fast-forward (client wrote 1) | 596 B (it held the server's heads, so it sent the difference) | **68,399 B** (an offer of all 2,001 ids at 34 B each) |
| Both diverged (each wrote 1) | **68,687 B** | **68,770 B** |

Both sides also run `topo_order()` and an `ancestry` walk per session, which is O(N log N) CPU. `bk` syncs before every read that has a token and after every write, plus a `--confirm` session. The O(N) offer is paid on every write and on every pull with news. A no-op pull is cheap. **Inference confirmed.**

### 1.3 The team Ground's shape

- 13,983 contributions over 25 days, about 560 a day.
- By kind: 6,352 evidence, 3,719 relations, 3,682 claims, 227 retracts, 3 artifacts.
- Relations: implements 1,134, contradicts 860, refines 831, supersedes 598, depends_on 230, supports 66.
- 1.83 parents on average, at most 5.
- Median body 417 B as JSON. Apparent size is 19 MB; disk use is 57 MB because 4 KB blocks hold ~1.4 KB objects.
- About 6% of contributions are "changes" (supersessions plus retractions).

### 1.4 Measurements

Team Ground (N = 14k), pinned binary through `bk`, wall clock. `bk --version` alone takes 0.08 s.

| Command | Wall | CPU | Note |
|---|---|---|---|
| `log --tag decision --current` | 0.26–0.52 s | 0.15 s | 70 MB max RSS |
| `log --limit 50` | 0.30–0.44 s | 0.13 s | O(N) for 50 lines |
| `heads` | 0.34 s | | |
| `show <id>` | 0.61–0.64 s | | two views |
| `log --open` | 0.43 s | | |
| a read with `BRACKEN_FULL_CHECK=1` | 4.9–5.7 s | 0.9 + 1.0 sys | the merge gate's path |
| baseline: `cat` of 14k objects (warm) | 2.05 s | | about 145 µs per file on this Mac |

Throwaway Ground, N = 20k on disk, pinned binary:
- `log --tag … --current` takes 0.19 s.
- `accepted --follow-changes` takes 0.33 s warm, or 1.3 s first time.
- **The first read after objects arrived** (full open and snapshot write) took 8.9 s.
- **A single `claim` write** took 0.36–0.43 s: open, sign, fsync and an 11 MB snapshot rewrite. Generating 20k contributions through the `Store` API took 123 s, about 6 ms each, dominated by fsync.

In-memory benchmark, `manyroot` release build, single thread:

| N | topo order | decode all | verify all | checked import | `evaluate` | `evaluate` + follow_changes |
|---|---|---|---|---|---|---|
| 10k | 3 ms | 23 ms | 1.3 s | 1.0 s | 0.19 s | 0.21 s |
| 25k | 24 ms | 130 ms | 2.8 s | 2.6 s | 0.16 s | 0.42 s |
| 50k | 17 ms | 234 ms | 4.3 s | 3.7 s | 0.14 s | 0.82 s |
| 100k | 50 ms | 257 ms | 4.4 s | 6.3 s | 0.63 s | **4.7 s** |
| 150k | 104 ms | 656 ms | 11.8 s | 14.6 s | 1.3 s | **13.7 s** |
| 200k | 112 ms | 751 ms | 11.9 s | 15.3 s | 0.75 s | **17.9 s** |

Verification ran at about 45–80 µs per signature (the variation is noise from other load). `follow_changes` grows at about N^2.2 between 50k and 200k (individual steps are noisy), matching the O(N·C/64) sweep with C ∝ N, plus hash-map cache misses. Its bitsets cost (passes × C/64) words: about 3.4 GB at 1M.

## 2. Cost model per operation

Constants come from section 1.4. The scaling exponent is from the code: N contributions, C ≈ 0.06N changes, P ≈ 0.4N passes, d = the sync difference. "After R1–R4" is the design in section 5.

| Operation | Today, 14k (measured) | 1M (extrapolated) | 100M (extrapolated) | After R1–R4 |
|---|---|---|---|---|
| **Open for any read** (snapshot path): list `objects/` (~6 µs/entry), decode (~5 µs), checked insert of new objects only, Kahn replay (~10 µs) | 0.2–0.5 s, 70 MB | 20–40 s, 4–5 GB RSS, 1.2 GB snapshot | ~1 h, 400–500 GB RSS (infeasible) | 0: open an mmap'd index, O(1) |
| **Write, local** (sign, fsync one object, rewrite snapshot) | 0.4 s, 16 MB written | 20–40 s open + 1.2 GB rewrite | infeasible | O(log N) index update + fsync, ~1–5 ms |
| **Write through `bk`** (write + sync + confirm) | ~1 s incl. 0.5 MB offer | + 34 MB offer + O(N log N) topo on server | + 3.4 GB offer | O(d) bytes; O(d log N) with RBSR |
| **Pull with news** | 0.5 MB offer upstream | 34 MB | 3.4 GB | O(d) |
| `log --tag T --current` | 0.26 s | 30–60 s (open + O(N) view) | infeasible | O(log N + k) |
| `show <id>` | 0.6 s | ~1–2 min (two full views + assess) | infeasible | O(referrers + closure) |
| `log --since <cursor>` | O(ancestry) walk | seconds | minutes, plus open | O(k) via height/sequence range |
| "As of cursor H" | O(N) walk | same | same | O(1) membership per id with a chain index |
| `accepted` (plain policy) | ~0.1 s | ~10 s (N log N) | ~30 min | O(k) read of a maintained status table |
| `accepted --follow-changes` / needs_recheck | 0.2 s | **~8–15 min, ~3 GB bitsets** | **months; infeasible** | O(affected) per insert; O(k) per read |
| `conflicts` | O(N) | seconds | minutes | maintained set, O(k) |
| Full check (merge gate): read every file, strict verify, insert | 5 s | 6–8 min | 10–14 h single-threaded | packed reads + parallel verify on 10 cores: ~10 s at 1M, ~20 min at 100M; incremental: only new objects |
| Disk | 57 MB for 19 MB of data | ~4 GB, 1M inodes | ~400 GB, 100M inodes | ~1.1× data in packs |
| Weave open (decodes the whole Weave history, bitset ancestry) | n/a (separate Ground) | worst case n²/32,768 passes (weave.md §4) | infeasible | derived data persisted per contribution |

**When we reach those sizes.** At today's 560 a day, 1M arrives in about five years. At 100 agents, or when one Ground holds a ledger, a sensor feed or a bulk citation import, it arrives in weeks. A single double-entry book with 10⁴ postings a day passes 1M in three months.

## 3. Findings from the literature

1. **Production systems over immutable content-addressed objects separate objects from derived, persistent, incrementally maintained indexes.**
   - git: the commit-graph with generation numbers ([git docs](https://git-scm.com/docs/commit-graph)), the multi-pack-index, and reachability bitmaps. With bitmaps, walking all objects in Linux took 1.57 s instead of 33 s, and GitHub saved 5.67 CPU-days per hour after rolling out multi-pack bitmaps ([GitHub, Scaling monorepo maintenance](https://github.blog/open-source/git/scaling-monorepo-maintenance/)). Incremental MIDX chains with per-layer bitmaps arrived in [Git 2.50](https://github.blog/open-source/git/highlights-from-git-2-50/).
   - Scion stores a changeset as a write-ahead-log entry: files changed against the first parent, plus content-addressed blobs. Everything else is "derived data", where "each changeset's value is computed from its parents' values": blame, path history, commit-graph skip lists, copy detection and path search. Each kind is eager or deferred and written back. Merges went from 2,500 to 30,000 a day; infrastructure is 200× smaller than the git mirror fleet; checkouts are about 100× faster; every cache tier hits over 99%; there are no packfiles or locks; a ref move is one compare-and-swap on a Spanner row ([Anthropic](https://www.anthropic.com/engineering/redesigning-source-control); [Pragmatic Engineer](https://blog.pragmaticengineer.com/the-pulse-anthropic-builds-its-own-source-control-system/)).
   - The Scion design credits Mononoke's Bonsai model and derived data.
2. **Datomic is the closest existing analogue of a Ground.** It is an immutable fact log with four covering indexes ([Datomic index model](https://docs.datomic.com/indexes/index-model.html)): EAVT for everything about an entity; AEVT for column access; AVET for value lookup (costliest, and opt-in on Pro); and VAET, a reverse-reference index over ref attributes only. Index segments are immutable wide trees that any process can cache. Our `referrers` is VAET, held only in memory. Datomic advises against storing both directions of a relationship, because VAET navigates the reverse; that matches our relation-as-contribution design.
3. **Ledgers maintain balances as derived state over immutable transfers.** TigerBeetle's base state is an immutable, hash-chained log of prepares, each a batch of up to 8,190 transfers. Its derived state is current balances in an LSM forest, one tree per index, converging byte-for-byte across replicas ([ARCHITECTURE.md](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/ARCHITECTURE.md); [Jepsen 0.16.11](https://jepsen.io/analyses/tigerbeetle-0.16.11)). An independent benchmark measured about 48k transfers a second sustained on a Mac mini, with each insert fanning out to 14 LSM trees (snippet, [backend.how](https://backend.how/posts/1b-payments-per-day/)). Two lessons: batch the unit of signing and replication, and treat aggregates as maintained derived state.
4. **Incremental view maintenance is solved in general.** DBSP defines the incremental form of any query with relational operators, aggregation and recursion as Q^Δ = D∘↑Q∘I ([Budiu et al., PVLDB 16(7) 2023](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf); [VLDBJ 2025](https://link.springer.com/article/10.1007/s00778-025-00922-y)). Our status rules are a small stratified query over a grow-only input, the ideal case: inserts only, with negation through retraction.
5. **Reachability ("has pass P seen change C?") has cheap indexes.**
   - Generation numbers are a negative cut: if gen(A) ≤ gen(B), A can't reach B. Git chose corrected commit dates (v2) for locality and speed ([GitHub blog](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)).
   - Synapse's **chain cover index** splits the DAG into chains with sequence numbers and stores links between chains, so reachability is a lookup ([Synapse docs](https://matrix-org.github.io/synapse/develop/auth_chain_difference_algorithm.html)).
   - Static labellings (GRAIL, FERRARI, pruned landmark labelling) trade build time for O(1) queries ([SIGMOD 2023 tutorial](https://chaozhang-cs.github.io/files/sigmod23-tutorial-short.pdf); [O'Reach](https://arxiv.org/pdf/2008.10932)). Our DAG is narrow, about one chain per concurrent writer, which suits chain cover.
6. **Set reconciliation beats id lists.**
   - Range-based set reconciliation (RBSR) costs O(d log n) and is stateless per peer ([Meyer, arXiv 2212.13567](https://arxiv.org/pdf/2212.13567)). Negentropy (NIP-77) estimates about 3 round trips at 1M items; the Rust implementation reports 25 ms and 39 ms at 1M (snippet, [repo](https://github.com/nostrdevkit/negentropy), benchmarks marked unstable). Range-summarisable stores speed it further (snippet, [arXiv 2603.19820](https://arxiv.org/html/2603.19820)).
   - Merkle search trees ([Auvolat & Taïani, SRDS 2019](https://ftaiani.ouvaton.org/PUBLI/PEER_REV_CONF/2019_auvolat_hal-02303490.html)) and prolly trees ([Dolt](https://www.dolthub.com/blog/2022-06-27-prolly-chunker/)) give history-independent roots, at the cost of per-peer tree state.
   - AT Protocol's relay verifies signatures and inverts the MST for every event, handling thousands of events a second on about 2 vCPUs and 12 GB ([Relay ops](https://atproto.com/blog/relay-ops)).
7. **Signature costs and semantics.** ed25519-dalek takes about 40 µs per single verify (u64 backend) or 26 µs (AVX2), and about 13–19 µs each in batches of 96 ([crates.io README](https://crates.io/crates/ed25519-dalek)); on the M1 I measured 45–80 µs. Batch verification is cofactored, and cofactored and strict verification disagree on edge cases. A protocol must pin one equation; ZIP-215 does this to make batching consensus-safe ([ZIP 215](https://zips.z.cash/zip-0215); own knowledge). We pinned strict, which forbids batch verification.
8. **Knowledge graphs at scale break monolithic indexes.** Wikidata's query service passed 16 billion triples, growing about 1 billion a year. Blazegraph reloads took weeks to a month and corruption grew with size. On 9 May 2025 the graph was split into main and scholarly graphs, with scholarly data more than 50% of triples ([WDQS graph split](https://www.wikidata.org/wiki/Wikidata:SPARQL_query_service/WDQS_graph_split); [CEUR-WS paper](https://ceur-ws.org/Vol-4064/PD-paper3.pdf)). Lessons: partition by scope, make indexes rebuildable incrementally, and never require a full reload.
9. **Retraction cascades are large and mostly invisible in practice.**
   - About 25% of citations to 50,500 retracted papers came after retraction (snippet, [JDIS 2026](https://doi.org/10.1515/jdis-2026-0064)).
   - Only 5.4% of post-retraction citation contexts mention the retraction ([Hsiao & Schneider, QSS 2021](https://direct.mit.edu/qss/article/2/4/1144/107356/Continued-use-of-retracted-papers-Temporal-trends)).
   - Harm grows with citation distance (Huang et al. 2025, snippet).

   A system that pushes invalidation to dependents, like our needs_recheck, fixes this, but only if the push costs O(dependents).
10. **Signed micro-publications use rebuildable query layers.** Nanopublication services held more than 10M signed, content-addressed records across 11 servers. They answer queries through template (grlc) and Linked Data Fragments services over triple stores, deliberately more restricted than SPARQL so they scale ([Kuhn et al., PeerJ CS 2021](https://peerj.com/articles/cs-387/), snippet). The newer network indexes signed records from registries into query services (snippet, [Knowledge Pixels](https://knowledgepixels.com/nanopub-ecosystem-paper/)). That is the same split as objects versus indexes.
11. **Code stores avoid whole snapshots** (own knowledge, except Scion):
   - Bonsai and Scion store first-parent changes.
   - git trees and prolly trees share unchanged subtrees, so a one-file change costs O(depth).
   - FastCDC (Xia et al., USENIX ATC 2016) deduplicates edited large files.
   - Pijul's performance comes from a persisted "pristine" graph in an on-disk B-tree (sanakirja), not from replaying patches.
   - Unison hash-addresses definitions, so renames are free.

## 4. What our design gets right

- **Validity depends only on a contribution and its parents** (protocol.md "Validity", graph.rs:1-7). This is the property Scion's derived data and DBSP's incrementalisation need: everything derived can be computed at insert time from parents. It must stay frozen.
- **Immutable, content-addressed, canonical, signed.** Any index or cache tier can be built and shared without coordination, and nothing is ever invalidated except by new inserts.
- **Acceptance is reader-side and never replicated** (validation.rs:1-6). Policy views are caches, never format, so they can be rebuilt, versioned and changed freely.
- **A grow-only set CRDT.** Sync, packing and indexing are monotone, with no compaction of deletions, so RBSR and Merkle approaches apply directly.
- **Heads-first sync** is optimal for the fast-forward direction (trace: 596 B).
- **The verified snapshot** caches "verified" by id, which is the right idea. It is monolithic and rewritten on every write, which is the wrong shape.
- **Weave lives in its own Ground**, so code-store growth doesn't tax coordination reads (weave.md §4).

## 5. Design sketches

### 5.1 R1: the index store

**Goal.** Open in O(1), reads in O(log N + results), writes in O(log N), and everything rebuildable from `objects/`.

**Engine.** SQLite in WAL mode, or LMDB through `heed`. Our pattern is one writer process at a time per replica (the CLI or the server) and many readers, and SQLite WAL serves that with crash safety and ad-hoc queries for debugging. LMDB is faster for point reads; RocksDB suits write-heavy ingest such as a ledger. Choose by measurement. The schema below is engine-neutral: each "table" is an ordered key space.

The file lives in `cache/index.sqlite`, is never committed, and is versioned by `schema_version`. A mismatch or a damaged file means rebuild, which replaces today's snapshot fallback.

```sql
-- One row per contribution; seq is the local insertion order, a causal order (parents first).
CREATE TABLE obj (
  seq        INTEGER PRIMARY KEY,           -- dense local number; every other table uses it
  id         BLOB NOT NULL UNIQUE,          -- 32 bytes
  kind       INTEGER NOT NULL,              -- claim/artifact/relation/evidence/retract
  author     INTEGER NOT NULL,              -- -> agent.n
  ts_ms      INTEGER NOT NULL,              -- advisory
  height     INTEGER NOT NULL,              -- 1 + max(parent height); derived now, in format later (R6b)
  chain      INTEGER NOT NULL,              -- chain-cover: which chain
  chain_pos  INTEGER NOT NULL,              -- position on it
  loc        BLOB NOT NULL,                 -- loose path or (pack, offset, len)
  verified   INTEGER NOT NULL               -- 1 once signature and rules are checked
);
CREATE TABLE agent  (n INTEGER PRIMARY KEY, key BLOB UNIQUE);
CREATE TABLE parent (child INTEGER, parent INTEGER, PRIMARY KEY (child, parent)) WITHOUT ROWID;
CREATE TABLE child  (parent INTEGER, child INTEGER, PRIMARY KEY (parent, child)) WITHOUT ROWID;
CREATE TABLE head   (seq INTEGER PRIMARY KEY);
-- AEVT / AVET: by tag, kind and author, newest first by seq.
CREATE TABLE tag    (tag TEXT, seq INTEGER, PRIMARY KEY (tag, seq)) WITHOUT ROWID;
CREATE INDEX obj_kind   ON obj(kind, seq);
CREATE INDEX obj_author ON obj(author, seq);
-- VAET: every reference, typed. rkind = relation kind, or 'evidence:pass', 'evidence:fail', 'retract'.
CREATE TABLE ref    (target INTEGER, rkind INTEGER, src INTEGER, src_author INTEGER,
                     other INTEGER,          -- the relation's other end (from), if any
                     PRIMARY KEY (target, rkind, src)) WITHOUT ROWID;
CREATE TABLE fwd    (src_end INTEGER, rkind INTEGER, rel INTEGER, to_end INTEGER,
                     PRIMARY KEY (src_end, rkind, rel)) WITHOUT ROWID;   -- relations by `from`
-- Exploration-layer derived state (policy-independent).
CREATE TABLE retracted (seq INTEGER PRIMARY KEY, by_seq INTEGER);   -- live retraction
CREATE TABLE hidden    (seq INTEGER PRIMARY KEY, reasons INTEGER);  -- CurrentView.hidden, with a reason count
CREATE TABLE chain_link (from_chain INTEGER, from_pos INTEGER, to_chain INTEGER, to_pos INTEGER,
                     PRIMARY KEY (from_chain, from_pos, to_chain)) WITHOUT ROWID;  -- Synapse-style
CREATE TABLE meta   (k TEXT PRIMARY KEY, v BLOB);  -- schema_version, ground, last_seq, sync bookmarks
```

**Insert path** for a contribution c with parents p₁…pₖ, in one transaction after the existing validity check:
1. Verify, unless trusted; write the object; insert into `obj`, with height and chain assignment (below).
2. Insert into `parent` and `child`. Delete pᵢ from `head` and insert c.
3. Index by kind: a claim adds `tag` rows; a relation, evidence or retract adds `ref` rows, and a relation adds a `fwd` row.
4. Run the derived updaters of 5.2, which touch only the rows c affects.

**Chain assignment.** If c has a parent at the end of its chain, a chain no other child has extended yet, c continues that chain with `chain_pos + 1`, preferring the author's own. Otherwise c starts a new chain. For each other parent, record a `chain_link (c.chain, c.pos) → (parent.chain, parent.pos)`.

Reachability "does x reach y?" (y is an ancestor of x or x itself) means: is there a path in the chain-link graph from (x.chain, ≤ x.pos) to (y.chain, ≥ y.pos)? Synapse computes this with a closure over links, cached per (chain, pos) range. With height as a negative cut (height(y) ≥ height(x) and y ≠ x means no), most queries end immediately. For a narrow DAG with W concurrent writers, the link closure is O(W) per query.

**Reads:**
- `log --tag T --current`: `SELECT seq FROM tag WHERE tag=? AND seq NOT IN hidden ORDER BY seq DESC LIMIT k`, then fetch objects.
- `show X`: `obj` by id or prefix (the `id` B-tree supports prefix range scans), plus `ref WHERE target=X`, plus the status row.
- `log --since H`: `seq > bookmark` when H was this replica's own earlier head set. Otherwise, rows whose chain position isn't reached from H.
- Heads come from `head`; the cursor is unchanged.

**Trust and integrity.**
- The index is a cache with the snapshot's trust model (snapshot.rs header), and `bracken verify` rebuilds it.
- The merge gate's `BRACKEN_FULL_CHECK` builds a fresh index in a temp dir and compares the derived state with a from-scratch recompute (section 7). The existing full computation stays as the oracle.

**Size.** About 150–250 B per contribution of index (ids, edges, tags), around 20% of the object bytes.

### 5.2 R2: incremental acceptance, current view and needs_recheck

The rules (protocol.md "Derived state", "Acceptance") split into a policy-independent part and a per-policy part.

**Policy-independent derived state**, updated on every insert:

| State | Definition | Changes when | Update |
|---|---|---|---|
| `retracted(x)` | a live retract targets x | retract r of x inserted; retract of r inserted | set or clear; then fan out (below) |
| `supersessor(t)` | live `supersedes s→t` by t's author, s live | that relation or a retraction of s or of the relation | recompute from `ref WHERE target=t AND rkind=supersedes`, O(refs of t) |
| `hidden(x)` (current view) | retracted, or has a live supersessor, or references a hidden x | either of the above, or a reference's hidden flag flips | reason counting: `hidden.reasons` = own reasons + hidden references; a flip pushes ±1 to `ref WHERE target=x` sources, transitively. Cost O(affected referrers). |
| `dep_edges(n)` | counting `depends_on`/`implements`/`refines` out of n (trust rule, validation.rs:401-460) | such a relation is inserted or retracted; an `implements` by n's author is added (it widens who may add counted `depends_on` from n) | recompute n's out-edges from `fwd WHERE src_end=n`, O(out-relations of n) |
| `changes(d)` | live supersedes by d's author with a live replacement, or a retraction of d | as for supersessor and retracted | recompute from `ref WHERE target=d` |
| `rdeps(d)` | reverse of `dep_edges`, stored as an indexed table | with `dep_edges` | O(1) per edge |
| conflicts | divergences and contradictions among current subjects | a supersessor set, a contradiction, or currency changes | recheck only the touched target and its `contradicts` refs |

**Per-policy state**, for each maintained policy, keyed by `policy_hash` = the hash of (min_passes, sorted validators, follow_changes):

```sql
CREATE TABLE pol_pass  (pol BLOB, subject INTEGER, author INTEGER, ev INTEGER, PRIMARY KEY (pol, subject, author, ev)) WITHOUT ROWID;
CREATE TABLE pol_fail  (pol BLOB, subject INTEGER, ev INTEGER, PRIMARY KEY (pol, subject, ev)) WITHOUT ROWID;
CREATE TABLE pol_status(pol BLOB, subject INTEGER, base INTEGER, status INTEGER, PRIMARY KEY (pol, subject)) WITHOUT ROWID;
-- follow_changes only: which changes in the subject's closure each pass author has seen, kept only while unresolved.
CREATE TABLE pol_unseen(pol BLOB, subject INTEGER, change INTEGER, seen_by INTEGER, PRIMARY KEY (pol, subject, change)) WITHOUT ROWID;
```

Update paths:
- **Evidence e on X**: if it counts (not by X's author; in validators), insert into `pol_pass` or `pol_fail` and recompute X's base status from its own rows: O(evidence on X). If X's standing (accepted or needs_recheck versus not) flips, re-evaluate `supersessor` for every t that X supersedes (`fwd WHERE src_end=X AND rkind=supersedes`), since an accepted replacement makes t superseded in the accepted layer.
- **Retraction of evidence**: delete its row and recompute as above.
- **needs_recheck, the key observation.** X is needs_recheck when fewer than `min_passes` pass authors have every change in closure(X) among their pass's ancestors. A newly inserted change C cannot be an ancestor of any existing contribution. So when C is inserted:
  1. Find the subjects whose closure contains C's target d: `D = reverse-closure(d)` by BFS over `rdeps`, cycles allowed. Cost O(|D|), the true semantic size of the cascade.
  2. For each X in D that is accepted under a follow_changes policy, add a `pol_unseen(X, C, seen_by = 0)` row. X now has fewer than `min_passes` authors who saw everything, so it becomes needs_recheck at once. **No reachability query.**
  3. A retraction of C (C stops being a change) deletes those rows and recomputes X from its remaining unseen changes.
- **A new pass P on X** (follow_changes): for each `pol_unseen(X, C)` row, test reach(P, C) on the chain index, O(W). Increment `seen_by` per distinct author, and delete the row when `seen_by ≥ min_passes`. X returns to accepted when no rows are left and its authors who saw everything number at least `min_passes`. This is where the existing rule "a single author must have seen *every* change" is kept: store per (X, author) a count of seen unseen-rows rather than a per-change count. Cost O(|unseen(X)|).
- **A new counting dependency edge n→m** (new `depends_on`, `refines` or `implements`): every X in reverse-closure(n) gains closure(m) and its changes. Those changes are *old*, so existing passes may have seen them. For each X in reverse-closure(n), for each change C in closure(m) not yet tracked, test reach(P, C) for X's passes: O(|D(n)| × |changes(m)| × passes). This is the expensive case, but it is rare, and it is linear in what the edge actually affects instead of in N.
- **Supersession first.** A replacement that is needs_recheck still supersedes (protocol.md). So `supersessor` in the accepted layer reads "stands = accepted or needs_recheck", which these updates maintain.

**Why this matches the current semantics.** Every rule reads only a subject's referrers, its closure and its passes' ancestry. The sketch keeps each of those as a table and updates it when an insert touches it. The existing `Assessor` stays as the test oracle. A property test over random graphs, plus the merge gate on the real Ground, asserts equality (section 7).

**Lazy versus eager.** In domains with huge fan-in, such as a retracted foundational paper with 10⁵ transitive dependents, step 1 can be deferred: insert a `dirty(d)` row and process it in the background, or when someone reads a subject whose closure includes d. This is Scion's eager-or-deferred choice. Correctness on read stays: a read of X first drains any dirty change reachable in its closure. Finding that cheaply is itself a reachability query on the forward dependency index, bounded by |closure(X)|.

**Policies to maintain.** Maintain the Ground's declared default policies, the per-Ground configuration in R7, eagerly. Compute ad-hoc policies on demand from `ref`, which costs O(evidence on the subject) for one subject. `evaluate` over a whole ad-hoc policy stays O(subjects) but no longer O(N·C).

### 5.3 R3: sync

**Stage A, no protocol change** (client side only). Store a bookmark, the peer's heads at the end of the last successful sync with it, in `meta`. A Ground only grows, so the peer still holds them. In `plan`, compute `known = ancestry(held ∪ bookmark)`. The client's offer after a pull with news shrinks from N ids to the client's own new ids.

**Stage B, small protocol change.** The server has the mirror-image problem: it offers all its ids after a client write. Let a side add ids the peer is known to hold to its `heads` list, or add a `have` list behind a `features` flag (protocol.md step 1 ignores unknown features). The receiver adds those to `held`. `plan` doesn't check that heads are heads, but protocol.md defines them as such, so this needs a spec change and review in the protocol class.

**Stage C, RBSR fallback.** For peers with no bookmark, or many peers, reconcile over ids ordered by (height, id), with additive range fingerprints maintained in the index (an XOR or sum of ids per B-tree node). The cost is O(d log N) bytes; about 3 round trips at 1M.

**Server CPU.** Replace `topo_order()` with `seq` order from the index, and `ancestry()` with a reach test or a seq/height range scan.

### 5.4 R4: storage

- **Loose objects stay the git-carried form.** Locally, a geometric repack moves them into append-only packs named by content hash, with a sorted `id → (pack, offset)` index, as git's midx does. Disk use falls from 3× to about 1.1× and the cost of opening many small files disappears. Choose between "git carries loose files" and "git carries packs" (open question 1).
- **Full check:** read packs sequentially and verify on all cores with rayon. That is about 10 s at 1M, and at 100M dominated by signatures (100M × 50 µs ÷ 10 cores ≈ 8 min).
- **Fsync batching:** a write of k contributions, such as an import or a sync batch, does one fsync per batch, not one per object. Generating 20k objects cost 6 ms each, mostly fsync.

### 5.5 R5: hot spots, days

- `show`: one view, or just referrers and assess (main.rs:847-853).
- `log --limit k`: stop after k in the index.
- Snapshot: append new contributions as a segment with its own checksum instead of rewriting (a bridge to R1).
- Parallel verification in `Store::open`.
- The sync bookmark (Stage A).

### 5.6 R6: frozen-format items (protocol class, owner approval)

| Item | Why | Domain-general? | Cost if late |
|---|---|---|---|
| **a. Out-of-line payloads**: an artifact carries `content_hash` and `size`, with bytes as separate blobs; small inline payloads may stay | lazy loading, partial and sparse replicas, chunking, cheap indexes (bodies stay small) | yes: datasets, documents, models, evidence attachments | every existing artifact keeps the old form; two forms forever |
| **b. Optional validated `height`** (= 1 + max parent height, a local rule) | a negative cut on partial replicas, RBSR order, "as of" bounds; Matrix carries `depth` (unvalidated, and abused); git's generation numbers | yes | can always be derived locally on a full replica, so only urgent if partial replicas come |
| **c. The verification equation**: keep strict (no batch) or move to ZIP-215 in v2 | batch verification is about 2× faster but a different rule; mixing them is a consensus bug | yes | pinned by golden vectors already |
| **d. Header before body** in canonical field order: kind, author, parents, height, references, tags, then payload | indexers and sync can stop decoding early | yes | cheap now, impossible after v2 is frozen |
| **e. Batch contribution**: one signature over a Merkle root of up to K records, each record addressable by (batch id, index) or its own hash | ledgers and knowledge graphs ingest 10⁴–10⁶ records a second; per-record ed25519 caps at about 10–25k a second per core and adds 64 B per record; TigerBeetle batches 8,190, and AT Protocol signs one commit over an MST root | yes; this is the biggest question for non-code domains | records signed one by one can't later share a signature |
| **f. Structured fields** in claims (a canonical map of typed values beside `statement` and `tags`) | aggregate and value indexes (AVET) need values, not prose: amounts, dates, quantities, units | yes (XBRL facts, Wikidata qualifiers, SHACL-typed properties) | otherwise parsed from strings per application |

**Implementation detail, never format:** the index store, packs, snapshots, chain-cover labels, policy caches, bookmarks and Weave's derived data.

### 5.7 R7: the configurable layer

Each Ground declares, in contributions under a configuration tag that the index reads:
- **Indexed attributes**: which tags or structured fields get value indexes (AVET), and which aggregates are maintained (sum by account, count of distinct reviewers).
- **Propagation rules**: which relation kinds carry a change into needs_recheck. Science might choose depends_on and uses-data-from but not cites. A ledger might choose posting → account but not annotation. An organisation might choose reports-to for re-approval cascades. Today the closure kinds are hard-coded (validation.rs:401-460); making them Ground configuration keeps the protocol generic, but the trust rule about who may add an edge must stay protocol-defined.
- **Default policies** to maintain eagerly (5.2).
- **Scopes and partitions**: a key, such as a project, account, paper or department, that indexes, sync and sparse replicas can partition on. This is the lesson of Wikidata's split.

### 5.8 R8: Weave

- Store changes against the first parent and derive manifests (Bonsai), or make manifests a tree of trees or a prolly tree: O(depth) per one-file change instead of a 20 KB to 14 MB whole manifest per candidate.
- Chunk large files with FastCDC.
- Persist the content-hash index, the ancestry answers and line state as derived data in the same index store, keyed by seq, "computed from parents" like Scion's. This replaces open-time bitset sweeps, whose worst case is n²/32,768 passes (weave.md §4).
- Keep this out of the general store: Weave tables get their own namespace and updaters, so a coordination-only Ground pays nothing.

## 6. Non-code workloads

| Workload | Shape | Volume | What it needs from us | Where it lives |
|---|---|---|---|---|
| **Double-entry ledger** | immutable postings (debit and credit legs) referencing accounts; corrections are compensating entries, never edits (TigerBeetle, accounting practice) | 10³–10⁶ postings a second at bank scale; 10⁴ a day for a small business | maintained balances per account and as-of (aggregate derived state); value indexes on amount and date; batch signing (R6e); structured fields (R6f); audit trail by account = `ref` by target | P: e, f. C: aggregates. A: ledger app |
| **Knowledge graph** (Wikidata-like statements with qualifiers, references and ranks) | statement = claim; references = evidence or `supports`; rank (preferred, normal, deprecated) ≈ acceptance status under a policy; deprecation = supersedes | Wikidata: more than 16B triples, +1B a year; the query service split in 2025 | per-statement records at ~1.2 KB each would be 18 TB for Wikidata, so batching (R6e) and compact bodies are essential; partitions by scope (R7); value indexes | P: e, f. C: scopes, indexes |
| **Citation and contradiction graphs** | cites (non-propagating), contradicts, supports, replicates (evidence) | OpenAlex-scale is billions of citation links (own knowledge, not re-checked) | VAET by target; per-subject aggregates (replications, contradictions); conflict detection limited to current subjects | C: which kinds index and aggregate |
| **Retraction cascade** | a retraction or supersession of a foundational claim propagates needs_recheck along depends_on and uses-data-from | a famous retracted paper: 10³–10⁴ direct citers; transitive dependents can reach 10⁵ | push-marking O(dependents) (5.2), deferrable; a propagation depth or kind set per Ground (R7); notification of affected authors. Literature: ~25% of citations come after retraction and 5.4% mention it, so push matters | P: semantics fixed (needs_recheck). C: kinds and depth |
| **Consensus over many reviews** | k reviews per subject; distinct-author counting; validator sets | 10–10³ per subject | `pol_pass` keyed (subject, author): O(1) per review; any policy over one subject is O(reviews) | A or C |
| **Event-sourced business process or organisation** | events per entity (order, employee, unit); projections = current state; "as of" queries; process mining over traces | 10⁶–10⁹ events | per-entity streams = scope partitions; projections = maintained views; as-of = chain or height ranges | C: scopes, projections |

**Structures that generalise across all of these:**
- the immutable signed record;
- the typed reference with a reverse index (VAET);
- maintained per-target aggregates;
- causal position (height and chain) for "as of";
- declared propagation rules for invalidation;
- scope partitions;
- batching of records under one signature.

Everything else is application.

## 7. Staged migration plan

Each stage ships behind the existing oracle. The current full computation keeps running in the merge gate (`BRACKEN_FULL_CHECK`) and in property tests until a stage's metrics show agreement.

| Stage | Contents | Format change? | Exit metric (add to `scripts/metrics`) | Effort |
|---|---|---|---|---|
| **0. Instrument** | record open time, bytes synced per session, ids offered, snapshot bytes written, `follow_changes` time | no | baseline captured for a week | 1–2 days |
| **1. Hot spots** | R5: `show`, `--limit`, append-segment snapshot, parallel verify; sync bookmark (5.3 A) | no | read p50 down 30%+; offered ids per pull with news ≈ new ids; bytes written per write ≈ object size | about 1 week |
| **2. Index store as cache** | R1 schema; built from objects on first open; reads answered from it; the snapshot retired; `verify` rebuilds; the gate compares index-derived `log`, `heads`, `show` and `--current` with a fresh recompute | no (local cache, storage class review) | equality on every gate run for 2 weeks; reads < 50 ms at 14k and on a synthetic 1M Ground | 2–3 weeks |
| **3. Incremental acceptance** | 5.2: retracted, hidden, supersessor, dependency edges, per-policy status, pol_unseen; chain index; `Assessor` as oracle | no | equality with `Assessor` on random-graph property tests and the real Ground; `accepted --follow-changes` < 100 ms at 1M synthetic | 2–3 weeks |
| **4. Storage** | local packs + midx, batched fsync, parallel full check | no (if git keeps loose files) | full check < 15 s at 1M synthetic | 1–2 weeks |
| **5. Sync B/C** | `have` list (feature flag), then RBSR | sync protocol (additive) | sync bytes O(d) on a diverged 1M pair | 2 weeks |
| **6. Protocol v2** | R6 a–f as decided; migration note in releases.md; both versions readable | **yes**: owner approval, golden vectors | n/a | design first; 3–6 weeks |
| **7. Configurable layer + Weave derived data** | R7 declarations; Weave tables in their own namespace | no (contributions in an existing kind) | a non-code pilot (ledger or citation graph) at 10M on one machine | ongoing |

Stages 0–4 need no frozen-format change and deliver almost all of the speed-up for the current team. Stage 6 is what makes the protocol fit ledger and knowledge-graph scale. It should be designed now, while the Ground is 14k contributions, even if it ships later.

## 8. What our design gets wrong or is missing (checklist with references)

1. No persistent indexes; every command is O(N) (store.rs:145-250, snapshot.rs).
2. O(N) bytes written per write (snapshot.rs `Drop`).
3. `show` does two whole-graph views (main.rs:847, 852); `log --limit` is O(N).
4. `follow_changes` is O(N·C/64), measured at about N^2.2 (validation.rs:248-290).
5. Sync offers O(N) ids after any local write or remote news (sync.rs:424-436; traced).
6. One file per object: 3× disk, 145 µs per open; fsync per object (store.rs:630).
7. Inline artifact bytes up to 16 MiB inside signed content (contribution.rs:20).
8. No causal position (height or chain) per contribution, so "as of" and "since" are walks.
9. No structured fields and no batch records, so non-code domains can't index values or ingest at scale.
10. Propagating relation kinds are hard-coded (validation.rs:401-460), not configurable per Ground.
11. Weave: whole versions and manifests; open decodes everything; cubic worst-case ancestry (weave.md §4).

## 9. Open questions

1. Must `.bracken/` stay committable as loose files? That decides whether packs are local-only (R4).
2. Will replicas ever be partial (metadata without payloads, or a scope)? If yes, R6a and R6b move up, and the index needs "known but absent" rows.
3. Which policies are maintained eagerly? A server can't maintain every reader's policy; declared defaults plus on-demand per-subject evaluation is my proposal.
4. Strict or ZIP-215? Batching matters only at ledger-scale ingest; for the team, strict and parallel is enough.
5. Is a batch contribution (R6e) compatible with per-record retraction and supersession? It is if every record has its own id (its hash) and the batch signature covers the Merkle root. A retraction then targets a record, not the batch.
6. Propagation depth: should needs_recheck cascade transitively without limit (the current rule) in domains with 10⁵-wide fan-in, or should Grounds be able to bound it and mark the rest as "stale upstream"?
7. Is SQLite fast enough at 100M, or does that scale need an LSM forest (TigerBeetle) or a partitioned server index (Scion's Spanner plus caches)? I expect one replica holding 100M to be a server role, not a laptop role.
