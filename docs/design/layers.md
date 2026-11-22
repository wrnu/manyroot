# Manyroot's layers: protocol, configurable rules, application, performance and self-improvement

*Design, 2026-11-21 (the manager's synthesis of the November research), decided 2026-11-22: the owner took all eight decisions in section 6 as recommended (decision `ea6f5419a31e`), with the owner-gated items listed there still gated. Sources: research reports [A (protocol)](../research/2026-11/research-a-protocol.md), [B (performance)](../research/2026-11/research-b-performance.md), [C (agent access)](../research/2026-11/research-c-agent-access.md), [D (self-improvement)](../research/2026-11/research-d-self-improvement.md), the earlier [software-factory survey](../research/2026-11/software-factory-research.md), and the author's read-only check of the code at `main` d3b5c297 and of the team Ground (13,984 contributions) through `scripts/bk`. "I" below is the synthesis's author. Repo citations were rechecked against `main` (code unchanged since d3b5c297) on 2026-11-22 and corrected where they had drifted.*

---

## Executive summary

**The four reports agree on the shape. My check of the code supports them, with one claim refuted and several corrections.**

1. **Validity stays as it is: local, monotone and frozen.** Validity depends only on a contribution and its parents; acceptance is each reader's policy. Every report says this split is the project's best decision, and it is what lets project rules change without forking a Ground. Write it down as a protocol invariant: *project rules never affect validity*.
2. **What is missing is one level of structure above the five body kinds.**
   - Claims have no type: 40 free-text tags, including `task,deferred` as one tag.
   - Evidence has no method identity: 5,522 distinct method strings across 6,352 evidence records.
   - References live in prose. I counted 9,026 citations of existing contribution ids in claim text, and **only 293 (3%) of them are also parents**, so the core can't see or order almost any of them.
   - The merge gate reads PR structure out of prose with a regex (`scripts/merge:306`).
3. **Performance is fine today and won't scale.**
   - Every command rebuilds the graph from a monolithic snapshot (0.37–0.52 s per read at 14k, which I measured).
   - Every write rewrites that whole snapshot.
   - `follow_changes` does a full pass per 64 changes, so it is quadratic. Even a single `show --follow-changes` pays for the whole sweep.
   - After any write, sync offers every id.

   All of this can be fixed without a format change, with persistent derived indexes and incremental acceptance. That work should come first.
4. **Policies are CLI flags.** Rules are AGENTS.md prose and bash. Experiments lapse with a printed note, not a failure. Weave has already built the right model: line policies as signed records, judged "in force" by causal position, with "diverged, so no policy in force" handled. Generalise that model rather than invent a new one.

**Recommendation in one line.** Build the configurable layer now, on protocol v1, using typed tag conventions, policy records and rule records. Make reads and acceptance incremental. Design protocol v2 now, as one package, but freeze it only after the conventions have run for four weeks and one non-code pilot has exercised them.

**Main reconciliation calls:**
- **Typed attributes:** `type:`/`key:value`/`ref:` tags first (C), with the CLI putting every referenced id into `parents`. A's protocol `type`/`refs`/`data` comes later in v2.
- **Policy language:** a fixed, declarative policy schema evaluated natively, not Datalog. Our own `research/query-lab` prototype, which no report cited, found generic Datalog growing to 6.8–18.5 s at 100× our size, and it had to build in retraction. Datalog stays for views and analysis.
- **Batch contributions:** no. Use parallel strict verification instead.
- **Signature verification:** keep strict. Don't move to ZIP-215.
- **`inconclusive`:** add it as a third core verdict in v2. Evidence class, strength and independence stay per project.

**The self-improvement loop.** Rules, metrics and experiments become typed, signed records. A runner identity, not the proposer, measures. Amendments are judged under the *previous* rulebook. Lapse becomes a gate failure. The protocol needs no new mechanism for this: only the "rules never affect validity" invariant now, and `method_ref` in v2.

**Next week, without the owner's approval:**
- instrumentation;
- the hot-spot fixes, which are already approved as Wave 2 E of plan `5568bbde91e0`;
- typed conventions and the typed gate (Wave 2 F);
- typed experiment and metric records, lapse as a gate failure, and an `escapes_14d` guardrail metric;
- the v2 design document, on paper.

**Needs the owner** (decided 2026-11-22 as recommended, decision `ea6f5419a31e`; section 6 lists what stays gated):
- the invariant text;
- the v2 freeze and its contents;
- payload erasure;
- model spend for the brief-optimiser experiment;
- the non-code pilot's domain;
- owner-signed decisions for the top rule tiers.

**What the research does not show:**
- that typed structure improves agent outcomes (no replay has been run);
- that any of this works outside software (every non-code example is a sketch);
- the 1M and 100M numbers, which are extrapolations.

Several literature claims that drive the recommendations rest on abstracts or snippets (section 7).

---

## 1. Verification of the claims the recommendations depend on

I checked each claim by reading the code at d3b5c297; line numbers were rechecked against `main` on 2026-11-22 (V20 was added then). Timings come from `scripts/bk` reads with no identity, so they read the local replica with no pull, on a loaded shared Mac. Counts come from one `bk log --json` export, which I have since removed.

| # | Claim (source) | Verdict | Where | Notes |
|---|---|---|---|---|
| V1 | Closed enums: an unknown body, relation or verdict kind fails to decode, so the contribution is invalid and can't replicate (A F2, W1) | **Confirmed, and stronger than stated** | `crates/manyroot/src/contribution.rs:24-43, 81-106` (serde-derived enums); decode at `:209-222` and `:361-381` re-encodes and compares; protocol.md Validity rule 2 | An unknown *field* also fails: serde drops it, the re-encoding differs, and the contribution is refused. A v1 replica can't carry any extension at all, which is why v2 must be a new `protocol` value. |
| V2 | No persistent index; every read rebuilds from a full snapshot (B §1.1, C §0.1) | **Confirmed** | `crates/bracken/src/snapshot.rs:46-68`; `crates/manyroot/src/store.rs:145-250` (lists all of `objects/`, decodes every snapshot entry, replays Kahn's algorithm); `graph.rs:78-90` (in-memory HashMaps only); no database crate in any Cargo.toml | Measured: `log --tag decision --current` 0.37–0.41 s, `log --limit 5` 0.32 s, `status` 0.31 s, `accepted` 0.52 s. That matches B's range. |
| V3 | A write rewrites the whole snapshot (B §1.1) | **Confirmed** | `snapshot.rs:104-115` (`Refreshing::drop`), `:133-146` (`encode` writes every contribution), `:174-205` (`write`) | I didn't measure the snapshot's size; B and C say 16 MB. |
| V4 | `follow_changes` costs O(N·C/64) and grows about quadratically (B §1.4) | **Complexity confirmed; B's timings not re-run** | `validation.rs:248-293` (`Seen::sweep`): one pass over the whole causal order per 64 changes, plus a bitset per pass | **New:** `compute_base` (`validation.rs:137`; the sweep at `:165-166`) triggers the full sweep for *one* accepted subject, so `show --follow-changes` on an accepted claim pays O(N·C/64). The 3.4 GB bitset figure at 1M follows from the formula (0.4N passes × 0.06N/64 words × 8 B). |
| V5 | Sync offers O(N) ids after any local write (B §1.2; sync.rs:424-436) | **Confirmed by code; trace not re-run** | `crates/manyroot/src/sync.rs:424-437` (`plan`). protocol.md Sync step 2 *requires* this definition of *known* | Caveat for B's "Stage A, no protocol change": narrowing *known* with a bookmark departs from step 2's normative text, so it needs a protocol.md edit and protocol-class review, though no wire change. It also assumes the peer never loses data; a server restored from backup would break it (section 3, G6). |
| V6 | `show` builds the current view twice (B §1.1, R5; main.rs:847, 852) | **Refuted** | `crates/bracken/src/main.rs:841-860`: line 847 is the `--json` branch and line 852 the text branch, so they never both run | Each path builds one `CurrentView` over the whole order, so `show` is still O(N) for one id (measured 0.37 s, the same as `log`). The fix B proposes (no whole-graph view for one id) still stands. |
| V7 | The merge gate parses `pr` claims with a regex (A F3; scripts/merge:306) | **Confirmed** | `scripts/merge:306` | All 728 `pr` claims match the pattern (I recounted: 0 misses). The structure holds only because the regex enforces it. |
| V8 | Policies live only in CLI flags (A W5, C §3.5, D P1) | **Confirmed for manyroot and Bracken; Weave differs** | `validation.rs:18-31` (`Policy`); `main.rs:70-83, 2110-2118` (`PolicyArgs`); `scripts/merge:207-215` (the manager passes `--validator`/`--min-passes` by hand per merge) and `:324, :833` (recorded only as prose in evidence details); the server's admission policy `serve.rs:38-52` is also configuration | Weave **already** keeps policies as signed records, decided in causal order, with an explicit "diverged, so no policy in force" (`crates/tree/src/lines.rs:1-13, 80-88`). This is the precedent to generalise. |
| V9 | Lapse has no teeth (D §0, §3.8) | **Confirmed** | `scripts/merge:81-84` ("never blocks the merge"), `:234-259`; `scripts/metrics:117-120, 140` | "Kept" means *any* current manager decision `supports` the experiment. Nothing checks that the decision cites metrics. **New:** expiry is computed from the advisory `timestamp_ms`, which the writer controls (`scripts/merge:240-245`). |
| V10 | (New) The merge gate never uses `follow_changes` | **Confirmed** | `scripts/merge` passes no `--follow-changes` | The protocol's central "retraction propagates" rule is unused in the one place the team relies on acceptance. |
| V11 | Signatures are strict Ed25519, so no batch verification (B §3.7) | **Confirmed** | `crates/manyroot/src/identity.rs:162-169` (`verify_strict`); protocol.md:40-42 | |
| V12 | Free-text typing statistics (A F3) | **Confirmed** (exact for tags, methods and self-evidence) | Ground export | 40 distinct tags; `task,deferred` present. 5,522 distinct methods across 6,352 evidence records. 3,427 evidence records by the target's author. Ids in prose: A says 2,563 claims; my regex (12-hex prefixes of existing ids, as whole words) gives 2,494, the same order. **New:** 9,026 such citations, of which 293 (3%) are parents. |
| V13 | Weave re-implements the ancestor rule and indexes every contribution at each open (A F7, B §5.8) | **Confirmed** | `crates/tree/src/ground.rs:171-201` (`references`), `:203-218` (`Index::build` iterates all ids) | Head-first decoding bounds the cost of one bad record, but not the O(N) open. |
| V14 | AGENTS.md is at its 12 KB cap (D §0) | **Confirmed** | `AGENTS.md` is 12,223 bytes against a limit of 12,288 (`crates/bracken/tests/agents_md.rs:6-17`) | 65 bytes of headroom. |
| V15 | Start-of-task reads are about 12k tokens compact (C §0.1) | **Confirmed** | measured 26.0 + 18.8 + 1.8 + 1.7 KB = 48.3 KB | |
| V16 | Decisions show as `pending (0 passing)` (C §0.2) | **Confirmed** | `bk show ea293edbdbb6` | The type system can't say that authority, not evidence, decides a decision. |
| V17 | "Two formal experiments in four weeks" (D §0) | **Partly confirmed** | Ground | Two claims are tagged `experiment`: `4f7404d019cc` (malformed) and `08fc19239548`. D's other example, `d64c3d545e81`, is tagged `process`; it predates the section 6 form. D's substance stands. |
| V18 | The `follow_changes` closure follows only `depends_on`, `implements` and `refines`; a retracted method or artifact cited by evidence is not a change (C finding 9) | **Confirmed** | `validation.rs:418-438` | |
| V19 | `log --since` walks the cursor's whole ancestry (B §1.1) | **Confirmed** | `crates/bracken/src/listing.rs:26-46` (`seen`) | |
| V20 | (Added 2026-11-22) A v1 replica refuses a v2 contribution visibly, in `rejected` (A §4.5; section 3.1's rollout as first written) | **Refuted** | `crates/manyroot/src/sync.rs:481` (`batch` reads the bundle with `bundle::read`); `crates/manyroot/src/bundle.rs:72` (`Contribution::decode` on each entry); `graph.rs:146-147` | A v2 contribution (integer-keyed CDE) fails to *decode*, so the whole sync batch is refused (`bad batch`) and the session ends; a bundle import holding one fails whole. Only a v1-encoded contribution saying `protocol: 2` reaches `rejected`. The refusal is visible and nothing is misread, but it stops the batch. Section 3.1's rollout is corrected; question `e82eaca382d0`, answer `7b337526dc68`. |

**Not verified by me:**
- B's sync byte trace, and its in-memory timings at 10k–200k;
- every 1M and 100M extrapolation;
- C's token savings for `brief` (they are estimates);
- D's metric time series;
- all literature figures.

---

## 2. Reconciling the reports

### 2.1 Where they agree (merged)

| Point | A | B | C | D |
|---|---|---|---|---|
| Validity stays local; project rules never decide validity | F4, P5 | §4 | §4.8 | P1 |
| Policies and rules as signed Ground records, causally prior to what they judge | R1, §7.2 | R7 | R4 | P1, P2 |
| Claims need types; evidence needs method identity | P1–P3 | R6f | R1–R3 | P3 |
| Persistent, rebuildable derived indexes; incremental acceptance | R4 | R1, R2 | R10 | — |
| Amendments judged under the previous rules; a protected core; tiers | §7.2 | — | — | §4.1–4.2 |
| Keep optimisers, views and caches in applications | §8 | §5.6 | R8, R12 | §4.3 |

### 2.2 Conflicts and decisions

**C1. Typed attributes: a protocol `type`/`refs`/`data` (A, and B R6f) or `key:value` tags first (C)?**

*Decision:* tags first, now. Protocol fields come in v2, after measurement.
- **Now:** `type:<ns.name>`, `key:value` and `ref:<role>=<id>` tags, with tag syntax checked at write so that `task,deferred` can't happen.
- **The CLI puts every `ref:` id into `parents`.** v1 already allows any present contribution as a parent, so causal ordering ("the cited thing existed first") holds today without a format change. The 3% figure in V12 is the gap this closes.
- **Shapes** (required keys, enums, allowed relations) are checked at write by Bracken, and reported, not enforced, on read.

*Why:*
- C is right that we don't yet know which keys earn their place.
- A is right that the core must eventually *see* references. The parents trick captures their safety benefit now, and the local index captures their speed benefit.
- Freezing `data` schemas before a non-code domain exists risks freezing software-shaped fields.

*Cost:* tag parsing on read. With an index it is paid once per insert.

**C2. Policy language: stratified Datalog (A), named views with Datalog behind them (C), or policies as records (D)?**

*Decision:* two steps.

Step 1 is a **policy record with a fixed declarative schema**, evaluated by native Rust. It generalises today's `Policy`:
- requirements per subject type, as `{by: role | class | key, count, distinct_by: independence key}`;
- requirements conditional on an attribute value (for example `class in [protocol, storage, crypto, gate]` needs a reviewer);
- required named checks;
- admissible attackers: which findings block, and how triage dispositions and deferral resolve them;
- `follow_changes` relation kinds and recheck classes;
- an `amend` requirement.

Step 2 revives `research/query-lab`'s Datalog for **views and analysis**. Promote it to a policy language only if a second domain needs something the schema can't express, and only with incremental evaluation that meets the 100× bar.

*Why:*
- **Our own prototype** (report `9e306c8654a0`, which none of the four reports cites) measured semi-naive Datalog at 6–31 ms at 1× (8.8k contributions), 135–418 ms at 10×, and **6.8–18.5 s at 100×**, "growing faster than the data". A hand-written pipeline stayed at 30–406 ms.
- It also found that `retracted` "must be built in" (local stratification by causal order), that `follow_changes` was "expressible but expensive; left out", and that magic sets made agents care about rule shape.
- A's "milliseconds over 30k facts" holds at 1× only.
- Cedar can't express the dependency closure (A F6), so it is out. A small schema is analysable by enumeration, and it covers C's worked example and the Ground-visible half of the merge gate.

**C3. How much to freeze now?**

The reports differ:
- A: a whole v2.
- B: six items, R6 a–f.
- C: almost nothing.
- D: P3 and P4, bundled into v2.

*Decision:* freeze nothing in the format now. Freeze only two text invariants:
- *project rules never affect validity*, and acceptance in protocol.md is the reference policy;
- *a future id algorithm brings a new protocol version and a new id length*, and *signing contexts name the protocol version*.

**Design v2 now as one package and freeze it once**, after the tag conventions have run for four or more weeks and a non-code pilot has used them. The package (section 3.1) takes A's `type`, `refs`, `data`, `relates`, `method_ref`, `inconclusive`, CBOR CDE and grease vectors, plus B's out-of-line payloads.
- It excludes B's validated `height`: a full replica can derive it, and only partial replicas would need it in the format, and none are planned.
- It excludes batches and ZIP-215 (C4 and C5).
- B's "header before body" comes free with A's integer-keyed CDE map: CDE sorts keys, so numbering the header fields 0–4 and the body 5 puts the header first.

**C4. Batch contributions (B R6e)?**

*Decision:* no, not in v2.
- Parallel strict verification gives roughly 10 cores × 12–20k verifications a second, about 120–200k a second, which is enough for every workload except bank-scale ledgers.
- A batch complicates per-record retraction and supersession, which are the core of the scientific model.
- Records signed one by one can't later share a signature, but that only affects records already written.
- Revisit when a pilot measures ingest above what parallel verification can do.

**C5. Signature verification (B §3.7, R6c).**

*Decision:* keep strict verification, run it in parallel, verify once per replica and cache the result (the snapshot already does; the index store will). ZIP-215 would change validity, and today it would only buy ingest speed that C4 shows we don't need.

**C6. Graded evidence: `inconclusive` in the core (A Q4) or class and strength as tags (C §4.3)?**

*Decision:* both, at different layers.
- `inconclusive` joins the core verdict in v2. Decision procedures in every domain need a discrete "neither", for example an audit disclaimer or a null result.
- Class (executed, measurement, replication, confirmation, review, attestation, inquiry), ordinal strength and independence key are per-project vocabulary, held in tags now and in `data` later.

**C7. Independence: trust by role (A) or counting by an independence key (C)?**

*Decision:* the policy schema supports both. A requirement can be `by: role` and `distinct_by: [model_family, harness]`. Profiles that state model, harness and operator are claims attested by the root.

Do not *rely* on independence keys until C's calibration metric (R11), run on our roughly 6,000 evidence records, shows how many effective validators we really have. The "nine judges ≈ two votes" result comes from a snippet.

**C8. Argumentation over `contradicts` (C R6).**

*Decision:* an opt-in policy term. First run an experiment replaying grounded semantics against `log --open` and the gate's findings check on historical PRs. Not core.

**C9. What the protocol provides for self-improvement (A §7.2 versus D P1–P4).**

*Decision:* no validity change, now or in v2. The pieces go here:
- **The library (the configurable layer's engine in manyroot, domain-neutral):**
  - "rules in force at a causal position";
  - "amend under the previous rulebook";
  - "divergence means none is in force" (Weave's rule);
  - well-formedness checks.
- **v2:** `method_ref`, which is D's P3.
- **The founding root key:** D's P4 constitution. Weave's root-signed marker is the precedent.

**C10. Rule tiers: an ordered ladder or a graph (D Q4)?** Ordered tiers first. They match AGENTS.md §1's owner list, and a graph can come later if it's needed.

**C11. Should `follow_changes` be on by default (C R7)?** Not until it is incremental (Phase 4). Then turn it on in the merge gate as an experiment, measuring the false-recheck rate.

### 2.3 Gaps none of the reports covered

- **G1. Prior in-house evidence was missed.**
  - `research/query-lab` and its reports (`715b5a28ebe6` → `9e306c8654a0`) are the team's own Datalog-over-Ground study, and they drive decision C2.
  - Plan `5568bbde91e0` already holds, under the owner's acceptance of Neon's recommendations (`c8bbb2886be3`):
    - Wave 2 E: lazy payloads, an append-only snapshot and a single-decode tree, against a 50k-object / 200 MiB bar;
    - Wave 2 F: a typed Bracken gate replacing the regexes in `scripts/merge` and weave-gate.

  The roadmap below puts B's stage 1 and A's typed gate *into* E and F rather than running parallel tracks.
- **G2. Concurrent amendments.** Two stewards can supersede one rulebook concurrently. Neither A nor D says what is then in force. Weave's answer, "diverged, so no policy in force" (`lines.rs:80-88`), fails closed and is the right default; a later contribution that supersedes both resolves it.
- **G3. Erasure.** The Ground is public and permanent (decision `473d5df9b1ab`). Finance, HR and organisational domains carry personal data subject to deletion rights. Out-of-line payloads, where the signed record keeps a content hash and the bytes are a separate, deletable blob, are the only general answer that keeps signatures valid. That moves B's R6a from performance into governance, and is why it belongs in v2.
- **G4. Admission is a third policy layer.** Server allow-lists and quotas (`serve.rs:39-52`) decide what a replica *stores*. A self-amending rulebook must never drive admission by default, or an amendment could silence its critics. Keep admission local and outside rulebooks, and keep exit (any reader can evaluate under any rulebook) as the final guarantee (A §7.2(4)).
- **G5. Index freshness after `git pull`.** B's O(1) open needs to notice objects that arrive outside Bracken, such as checkpoints and pulls. Listing `objects/` is O(N), about 6 µs per entry. Options:
  - compare the 256 shard directories' mtimes with the values stored in the index;
  - a git post-merge hook;
  - local packs.

  This must be designed, not assumed.
- **G6. Sync bookmarks and replaced peers.** A server restored from backup, or a new server at the same URL, invalidates "the peer still holds what it held". Key the bookmark on a replica-instance id advertised in `hello.features` (unknown features are ignored, so this needs no format change), and drop the bookmark when it changes.
- **G7. Key custody underlies independence and sealed evaluation.** `scripts/merge` says "in v0 keys are shared, so this checks recorded intent, not proof". Independence classes (C), a runner identity and a sealed holdout (D) are only as strong as who can sign as whom. Make server-held, per-identity keys a stated precondition for anything that relies on separate identities.
- **G8. Typing the existing 14k contributions.** Nobody says how. Proposal: a rulebook may declare *legacy aliases* (tag `finding` means type `bracken.finding`) used at read time, and current items that matter (open tasks, current decisions) get a typed successor by `supersedes`. Never rewrite history.
- **G9. Half the merge gate isn't in the Ground.** These checks have no Ground facts to read:
  - the remote head equalling the claimed sha;
  - the counted run on the merged tree;
  - freeze checks;
  - the research Docker run;
  - Weave results standing in for the counted run.

  A's "six lines of rulebook" covers only the Ground-visible half. Each external check must first become typed evidence from a runner identity, which is also D's runner.
- **G10. v2 moves the ossification point; it doesn't remove it.** A keeps `rel-kind 0..7` and `verdict 0..2` closed in v2, so grease protects only `type`. That is a sound choice, because closed kinds keep core semantics decidable, but it should be stated: a new *core* relation kind will always mean a new protocol version.
- **G11. Weave's frozen formats.** A's "Weave format v2 on typed refs" (A2) is a frozen-format change. It needs the owner, and it interacts with the join. Don't plan it before the join lands.
- **G12. Experiment expiry by advisory clock** (V9). Judge lapse by an explicit `lapse` contribution from the runner, or by the reader's clock, and say which.

---

## 3. The layered design

### 3.1 Protocol: frozen, domain-neutral, owner approval

**Now (text only; no change to bytes or vectors):**
1. *Project rules never affect validity.* protocol.md's Acceptance section is the **reference policy**, one peer among policy records.
2. Agility invariants: a new id algorithm brings a new protocol version and a different id length; signing contexts name the protocol version.
3. Sync step 2 *may* narrow *known* with ids this side knows the peer holds (bookmarks), keyed by the peer's replica-instance feature. This is a protocol-class review under AGENTS.md §1, but changes no golden vector.

**v2 package: design now, freeze after Phases 2 and 8.** A's CDDL (§4.2) is the starting point.

| Item | Source | In? | Reason |
|---|---|---|---|
| Optional namespaced `type` on every body; tag syntax | A P1 | yes | the one shared extension point (RFC 9170, Rekor) |
| `relates` relation with no core semantics; an extension relation states its core kind | A P1 | yes | new vocabulary without new protocol versions |
| Typed `refs` [role, id], each in `parents` | A P2 | yes | core-checked, indexable references; ends prose citation |
| `data` (canonical CBOR) on claims and evidence | A P2, B R6f, C R2 | yes | values for value indexes and aggregates in every domain |
| Evidence `method_ref` | A P3, D P3 | yes | pre-registration, replication under the same method, metric versions |
| `inconclusive` verdict | A Q4 | yes | C6 |
| CBOR Common Deterministic Encoding, integer keys, header fields first | A P4a, B R6d | yes | a second implementation needs only CDE |
| Grease vectors | A §4.5 | yes | keeps `type` tolerant |
| Out-of-line payloads (content hash and size; bytes optional) | B R6a | yes | lazy loading, partial replicas, **erasure** (G3) |
| Validated `height` | B R6b | no | derivable on full replicas |
| Batch contribution | B R6e | no | C4 |
| ZIP-215 | B R6c | no | C5 |
| New signing context `manyroot/v2/contribution:` | A §4.3 | yes | no confusion between v1 and v2 signatures |

**Rollout:**
- v1 contributions stay v1 forever.
- A Ground opts in with a root-signed claim, and replicas advertise `v2` in `hello.features`.
- A v1 replica can't decode a v2 contribution, so a sync batch or bundle import holding one fails whole (V20); it can't list it as `rejected`, as A §4.5 says. So a v2 replica sends a peer that doesn't advertise `v2` in `hello.features` only contributions whose whole ancestry is v1, and a bundle written for v1 readers holds only those. If a sender ignores that, the session or import error is the visible signal. The detail is in `docs/design/protocol-v2.md` (task `fdbc081f07db`, in progress when this was written).

### 3.2 Per-project configurable layer: signed contributions in the Ground

The **rulebook** is a root-signed artifact (`type:rulebook` tag in v1; `type: manyroot.rulebook` in v2), versioned by `supersedes`, and identified by its content hash. Every derived output names the rulebook hash and policy it ran under. It holds:

| Section | Contents | First user |
|---|---|---|
| Types and shapes | required keys and value kinds, enums, allowed relations in and out, decision mode (`evidence` / `authority` / `record`, from C R1), legacy tag aliases (G8), proposition key fields (A Q1) | `bracken.pr`, `finding`, `triage`, `task`, `decision`, `experiment`, `metric` |
| Evidence vocabulary | classes, strength scales with a sentence per step, independence keys, which classes can confirm which types (C §4.3) | review versus executed checks |
| Policies | the C2 schema: requirements, conditions, required checks, admissible attackers, triage and deferral, `follow_changes` kinds and recheck classes | the Ground-visible half of the merge gate |
| Views | named, parameterised views for agents (`brief`, `queue`, `open_findings`) | task start |
| Rules | rule records with tier, scope and text; AGENTS.md becomes a *rendering* of the current rule records, and the 12 KB test applies to the rendering | process rules |
| Metrics | versioned definitions (command or query, with a content hash), kind `proxy` / `guardrail` / `gold` | `escapes_14d`, `builder_pass_minutes` |
| Experiments | rule under test, metric version, baseline window, sample or stopping rule, pre-registered decision rule, guardrails, expiry, the rule it retires | every process change |
| Amendment | per tier, the requirement a successor rulebook or rule must meet (tier 0 owner and root; tier 1 owner and a cross-family review; tier 2 a manager decision citing the experiment's result and an independent reviewer; tier 3 an automated proposal plus a held-out pass and manager sign-off) | D §4.2 |
| Maintenance hints | which policies to maintain eagerly, which fields to index, scope keys for partitioning (B R7) | the index store |

**Engine semantics**, in manyroot as a domain-neutral library, never in validity:
- **Rules in force.** The rulebook in force for a subject is the accepted one current in its causal past. Two views:
  - *as judged* (each subject under its own rulebook), the default for gates and history;
  - *restated* (everything under the reader's current rulebook), with both hashes shown.
- **Amendments** are accepted under the *previous* rulebook's `amend` requirement.
- **Divergence** means no rulebook is in force (G2).
- **Conformance results** never change data. Clients refuse a violation at write time.
- **The protected core:** validity, signatures, causal order, author-only retraction and supersession, the founding root key, and the "amend under previous" rule.

### 3.3 Application: Bracken, Weave, tools

**Bracken:**
- the index store and incremental acceptance (B R1–R3);
- one-process batch reads for scripts (query-lab recommendation A);
- `rules --current`;
- the typed gate;
- typed one-call writes (`claim --type finding --contradicts <pr> --attr severity=medium`);
- views with `because`, three nested detail levels, a token budget, deterministic order and absolute times (C R5, R8);
- calibration metrics (C R11).

**Weave** (kept and developed, as the owner wants):
- persist its index, ancestry answers and line state as derived data in the index store, in its own namespace, so a coordination-only Ground pays nothing;
- manifests as deltas against the first parent, or as trees of trees, so a one-file change costs O(depth) instead of a whole manifest (B R8);
- FastCDC for large files later;
- a descriptive rulebook for its seven media types, so generic tools can read them;
- a Weave format v2 that uses core `refs` only after v2 is frozen and the join has landed, with the owner's approval (G11).

**Tools:**
- an experiment runner under its own identity;
- a GEPA-style brief optimiser;
- control-chart dashboards and a "rules in force at X" view;
- an MCP layer mirroring the views later.

### 3.4 The self-improvement loop, made explicit

```
 observe ─▶ propose ─▶ pre-register ─▶ evaluate ─▶ decide ─▶ enact ─▶ monitor ─┐
   ▲                                                                          │
   └──────────────────────── archive (the Ground itself) ◀───────────────────┘
```

| Step | Record (configurable layer) | Who | Safeguard |
|---|---|---|---|
| Observe | metric definitions (versioned); measurements as evidence with `method_ref` to the definition version | runner identity | proxy, guardrail and gold kinds; `escapes_14d` as the first gold metric |
| Propose | a rule record that `refines` the current one, with its predicted effect (decision `9be9ab6b1b1f`) | anyone; tier 3 also an optimiser | one rule in, one out |
| Pre-register | an experiment record with its decision rule, sample, guardrails and expiry, causally before any result | proposer | causal parents prove ordering; the runner refuses a malformed record |
| Evaluate | evidence from the runner; for tier 3, a sealed holdout committed by hash, with its content outside the public Ground | runner and evaluator identities, never the proposer | evaluator code in a risky class; held-out data the improver can't read; cross-family check for gaming |
| Decide | an amendment accepted under the *previous* rulebook's tier requirement | per tier | proposer is never the sole judge; tier ≤ 1 signed by the owner, not quoted |
| Enact | `supersedes` on the rule record; AGENTS.md re-rendered | manager | divergence means no rule in force; rollback is a supersede |
| Monitor | a lapse at expiry without a decision citing the result is a **gate failure** if the lapsed rule is still rendered; restated-mode views flag subjects whose status would change | runner, gate | guardrail breach reverts by supersede |

**Protocol support needed:** the invariant now, and `method_ref` in v2. Everything else is the configurable layer, which keeps the loop domain-neutral. A ledger community amends its four-eyes threshold through exactly the same records.

---

## 4. Roadmap

Ordered by value against risk, with the no-format-change performance work first. Sizes are builder-weeks at the team's current pace and are rough. Every stage keeps the current full computation as its oracle in the gate (`BRACKEN_FULL_CHECK`) until its metrics show agreement.

| Phase | Contents | Layer | Depends on | Exit metric (`scripts/metrics` key) | Size | Owner approval? |
|---|---|---|---|---|---|---|
| **0. Instrument** | read wall time p50/p95, bytes synced and ids offered per session, snapshot bytes written per write, `follow_changes` time, regex parse misses, `escapes_14d` | A | — | a week of baselines | 2 days | no |
| **1. Hot spots** (= Wave 2 E) | append-only snapshot segments; lazy payloads; single-decode Weave tree; `show` without a whole-graph view; `--limit` stopping early; parallel verification; one-process batch reads for `scripts/merge`; client sync bookmark with the replica-instance feature (G6) | A (bookmark: protocol-class review) | 0 | read p50 down 30% or more; bytes per write ≈ object size; ids offered per pull with news ≈ new ids; Wave 2 E's 50k / 200 MiB bar | 1–2 wk | no (E approved in `c8bbb2886be3`) |
| **2. Typed conventions and typed gate** (= Wave 2 F) | tag syntax check; `type:`, `key:value` and `ref:` conventions with refs put into parents; write-time shapes for pr, finding, triage, task, decision, experiment and metric; typed one-call writes; the gate reads typed fields; experiment and metric records typed; lapse as a gate failure; runner identity posts metrics | C, A | 0 | zero regex parses of pr or experiment statements in `scripts/`; ≥95% of new claims of those types conform; zero malformed experiments | 2–3 wk | no (F approved; process rules as experiments) |
| **3. Index store as a cache** | B R1 schema (SQLite WAL or LMDB, chosen by measurement); freshness after git pull (G5); `verify` rebuilds; the gate compares index answers with a full recompute | A (storage class) | 1 | equality on every gate run for two weeks; reads under 50 ms at 14k and on a synthetic 1M Ground | 2–3 wk | no |
| **4. Incremental acceptance** | B R2: maintained retracted, hidden, supersessor and dependency edges; per-policy status; `pol_unseen`; chain-cover reachability; `Assessor` as the oracle; then `follow_changes` in the gate as an experiment (C11) | A, then C | 3 | property-test equality; `accepted --follow-changes` under 100 ms at 1M synthetic; false-recheck rate measured | 2–3 wk | no |
| **5. Policy records and rulebook v0** | C2 schema; root-signed rulebook artifact; `rules --current`; outputs carry the rulebook hash; amend under the previous rulebook; tiers; divergence rule; AGENTS.md rendered from rule records; external gate checks as runner evidence (G9) | C (engine in manyroot), A | 2; 4 for maintained policies | replaying the last 100 merges gives the same gate verdicts as `scripts/merge`; one real rule change goes through the amendment path | 3–4 wk | no, but the merge gate is a risky class |
| **6. Agent views** | `brief`, `queue`, `status --type` with `because`; detail levels; budgets; deterministic output; `--since` everywhere | A | 2, 3 | median task-start read ≤4k tokens (from ~12k); no reports or decisions in the review queue | 2 wk | no |
| **7. Self-improvement step B** | D §6 step B: an optimiser on the reviewer brief, sealed holdout of 20 PRs, cross-family gaming check | A, C | 2, 5, G7 | sealed-set recall up 15% with guardrails within 10% | 3 wk | **yes** (model spend) |
| **8. Non-code pilot** | a decision register or small ledger on v1 plus conventions, with synthetic data at 1M–10M, using a rulebook from section 3.2 | C, A | 3–5 | runs end to end; lists the fields and types v2 must hold; measured ingest rate | 3–4 wk | yes if it spends on models; **the domain is the owner's choice** |
| **9. Protocol v2 freeze** | section 3.1 package: golden and negative vectors, `ground-v2` fixture, grease, a differential test that v1 statuses are unchanged, a migration note | P | design from week 2; freeze after 2 and 8 | n/a | 3–6 wk | **yes** |
| **10. Sync B and C** | a `have` list behind a feature flag, then range-based set reconciliation | P (additive) | 3 | sync bytes O(d) on a diverged 1M pair | 2 wk | review only, unless it touches golden vectors |
| **Weave track** | derived data in the index store; manifests as deltas or trees | A | 3, after the join | open time flat in history length | 2–4 wk | Weave format v2 only: yes |
| **Datalog** | revive query-lab for views and analysis; incremental evaluation | A | 3 | the 100× bar (under 1 s) before any policy use | research | no |

**Sequencing notes:**
- Phases 1 and 2 run in parallel; they touch different files, apart from `scripts/merge`, which Phase 2 changes after Phase 1's batch reads.
- Phases 3–5 must not block the owner-gated Weave join (Wave 3). Plan them around it.
- The v2 design document should be written while the Ground is small, but frozen only once Phases 2 and 8 have shown which fields matter.

**What the team can start next week without the owner's approval:**
- Phase 0;
- Phase 1, as Wave 2 E;
- Phase 2, as Wave 2 F, with its process parts as `experiment` claims;
- D §6 step A (typed experiments, `escapes_14d`, runner identity, lapse as a failure);
- the v2 design document, on paper, with prototypes only in throwaway Grounds.

All of these need the manager's decisions and the reviews their classes require. The sync bookmark is protocol-class: it needs an independent review and the manager's `owner-review` claim, but no owner approval, because no frozen vector changes.

---

## 5. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Two type systems: tag conventions now, v2 fields later | high | make each convention map 1:1 onto a v2 field; a read-time adapter, never a rewrite |
| The index cache diverges from the objects, giving silent wrong answers | medium | full recompute as the oracle in the gate; rebuild on any doubt; G5 freshness design; same trust model as the snapshot (decision `96e282819607`) |
| More ceremony lowers throughput (the survey's weakest point for us) | medium | typed one-call writes and generated templates must *remove* steps; track `builder_pass_minutes` and PRs merged as guardrails on Phase 2 |
| The policy schema turns out too weak, and pressure for Datalog arrives before it scales | medium | the 100× bar first; keep the schema extensible by new requirement kinds, which are versioned by rulebook hash |
| Rulebook capture, Goodhart drift, self-judging, adopting noise | medium | D §5: tiers, previous-rulebook amendment, proposer never sole judge, gold and guardrail metrics, pre-registered decision rules, owner-signed tier ≤ 1 |
| Key custody makes "distinct identity" meaningless | high in v0 | G7: no reliance on runner or independence separation until keys are per-identity and server-held |
| A sealed holdout leaks, or ends up in the public Ground | medium | only hash commitments in the Ground (decision `473d5df9b1ab`); content held by the evaluator's identity off-Ground |
| Domain generality is assumed, not shown | high | Phase 8 is a precondition of the v2 freeze |
| Team bandwidth: the join, eval stages and hostile-writer fixes are in flight | high | Phases 1 and 2 are already-approved Wave 2 items; everything else waits for capacity |
| Over-specifying v2 before the data exists | medium | the freeze gate in Phase 9; out-of-line payloads and `data` are the only items with a strong reason to freeze early |

---

## 6. Decisions taken

On 2026-11-22 the owner answered "yes to all" to the eight decisions this section put to them (decision `ea6f5419a31e`, point c). Each was taken as recommended:

1. **The invariant "project rules never affect validity"** is adopted, and protocol.md's Acceptance section is reframed as the reference policy. It is text only, and it is the foundation of the configurable layer. (The protocol.md edit is its own task.)
2. **Sequence:** tag conventions and policy records on v1 now; protocol v2 designed now and frozen after a non-code pilot (about 8–10 weeks), rather than freezing v2 first.
3. **v2's contents when frozen:** `type`, `relates`, `refs`, `data`, `method_ref`, `inconclusive`, CDE, grease and out-of-line payloads; *not* batch, `height` or ZIP-215.
4. **Erasure:** in v2, a Ground may delete a payload's bytes while keeping its signed record (a content-hash commitment). Without it, non-code domains holding personal data can't use a public, permanent Ground.
5. **The non-code pilot domain:** a decision register or a small double-entry ledger with synthetic data, with no model spend at first.
6. **Self-improvement step B:** a brief optimiser at tier 3 only (prompts and briefs), scored on a sealed holdout, with a small fixed budget, after step A has run for two weeks.
7. **Rule tiers:** tier 0 (constitution and root) and tier 1 (safety, gates, risky classes) need the owner's own signature, never a manager-signed quote; tiers 2 and 3 are manager decisions under experiments. This removes the "bundle approval" pattern D found.
8. **Packs versus loose objects in git** (B's question 1, alongside the join and decision `473d5df9b1ab`): git keeps loose objects for now; packs are local-only, in Phase 3 or later.

The same answer started the work section 4 lists as startable without the owner (point b): Phase 0, Phase 1 (Wave 2 E), Phase 2 (Wave 2 F), self-improvement step A, and the v2 design on paper, with prototypes only in throwaway Grounds.

**Still owner-gated.** A chat answer can't approve what AGENTS.md section 1 reserves for the owner's own approval (`scripts/owner-approve`), so these remain open:
- the v2 freeze (a frozen-format change);
- the spend for self-improvement step B;
- any model spend in the non-code pilot;
- the model and provider move in `53985dbbf098`.

---

## 7. Uncertainty, and what the research does not show

- **No outcome evidence for the core bet.** Nothing measured shows that typed structure, views or policy records improve what agents deliver. C's token savings are estimates. The test is C's open question 1: replay a Phase 1 replicate with `brief` and `queue` only. It needs model spend.
- **Domain generality is argued, not demonstrated.** The ledger, science and pricing rulebooks (A §6, C §4.4) are sketches. The "ledger pilot" in the eval suite is a *software* project and doesn't count.
- **Extrapolations.** Every figure at 1M and 100M is extrapolated. B's N^2.2 exponent comes from noisy single-machine runs. I confirmed the complexity from the code, not the constants. My own timings are at 14k, on a loaded shared Mac.
- **Snippet-level sources behind recommendations:**
  - independence classes rest on "nine judges ≈ two votes" (arXiv 2605.29800) and judge correlation (2610.07109), both snippets;
  - bounded reads rest partly on context-rot and NoLiMa, both snippets;
  - the optimiser choice rests on GEPA (snippet);
  - "improvers game visible evaluators" rests on METR o3 (a secondary source) and DGM (read at source, appendix via summaries);
  - the proxy-overoptimisation curve (Gao et al.) is a snippet;
  - Crossref's "34 spellings" is secondary and unverified.

  Read at source: Cedar, Zanzibar, AT Protocol's Lexicon and sync specs, Matrix room versions, the DGM main results, Zep's table, Kaplan & Irvin, Anthropic's tool guidance and Synapse's chain cover.
- **General knowledge, not checked:** Holochain's DNA-hash claim, Datalog complexity bounds, and the evidence for argumentation and truth maintenance (C cites these from memory).
- **Our process data is small:** two `experiment` claims, four `metrics` claims, and metric definitions that changed after the baselines. Nothing about the self-improvement loop has been measured yet.
- **What I didn't verify:** B's sync byte trace and benchmarks, the snapshot's size, and any literature number.

---

## Appendix: recommendations mapped to their sources

| Recommendation (this document) | Source sections | Verification |
|---|---|---|
| Invariant: rules never affect validity | A F4, P5; B §4; C R13; D P1 | V1, V8 |
| Agility invariants | A Q8 | — |
| Sync bookmark (with replica-instance feature) | B §5.3 Stage A; G6 (new) | V5 |
| v2: `type`, `relates`, extension relations state their core kind | A F1–F2, §4.1–4.2, P1 | V1 |
| v2: `refs` in parents; v1 refs into parents now | A P2, W4; G (new: parents trick) | V12 |
| v2: `data`; v1 `key:value` tags | A P2; B R6f; C R2, §4.2 | V12, V16 |
| v2: `method_ref` | A P3; D P3 | V12 |
| v2: `inconclusive` | A Q4; C §4.3 (class and strength stay per project) | — |
| v2: CDE, integer keys, header first, grease | A §4.2, §4.5; B R6d | protocol.md:52-54 |
| v2: out-of-line payloads; erasure | B R6a; G3 (new) | — |
| Not batch, not `height`, not ZIP-215 | B R6b, R6c, R6e (rejected, with reasons) | V11 |
| Rulebook as a root-signed, superseding, content-hashed artifact | A §5.1, §5.4; C R4; D §4.2 | V8 (Weave precedent) |
| Types, shapes, decision modes, legacy aliases | A §5.1; C R1, §4.2; G8 (new) | V12, V16 |
| Evidence vocabulary and independence keys | C §4.3, R3, R11; A §5.3 (trust scoping) | — |
| Policy schema first, Datalog later | A F5–F6, §5.3, Q6; C §4.6, R12; query-lab `9e306c8654a0` (G1) | V4, V8 |
| Rules in force, amend under the previous rulebook, protected core, divergence rule | A §7.2; D P2, P4; G2 (new) | V8 (`lines.rs`) |
| Rule records, tiers, AGENTS.md as a rendering | D §4.2; A §7.3 | V14 |
| Typed metrics and experiments; runner identity; lapse as a failure; `escapes_14d` | D §3, §4.2, §6 step A; A §7.2(7) | V9, V17 |
| Sealed holdouts off-Ground | D §4.2, §6 step B; decision `473d5df9b1ab` | — |
| Index store | B R1, §5.1; C R10; A R4; G5 (new) | V2, V19 |
| Incremental acceptance and chain index | B R2, §5.2 | V4, V10 |
| Hot spots and batch reads | B R5; query-lab recommendation A; plan `5568bbde91e0` Wave 2 E | V3, V6 (refuted; fix still valid) |
| Typed gate | A §6.1; plan `5568bbde91e0` Wave 2 F; decision `c8bbb2886be3`; G9 (new) | V7 |
| `follow_changes` in the gate (experiment) | C R7; B §6; V10 (new) | V10, V18 |
| Grounded argumentation (opt-in) | C R6, §4.4 | — |
| Views: `brief`, `queue`, `because`, levels, budgets | C §4.1, R5, R8 | V15, V16 |
| Weave derived data, manifest deltas | B R8; A F7 | V13 |
| Sync `have` list and RBSR | B §5.3 Stages B and C | V5 |
| Non-code pilot before freezing | A Q6; B §6; C §4.5 | — |
| Self-improvement step B | D §6 step B; D findings 1–5 | — |
| Admission stays local | G4 (new); protocol.md Sync ("local policy") | V8 (`serve.rs`) |
| Key custody as a precondition | G7 (new); D Q2 | `scripts/merge` header |
