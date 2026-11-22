# Protocol design and extensibility: what to freeze, what to configure, and how rules amend themselves

> Date: 2026-11-21. Part of the November 2026 research behind [docs/design/layers.md](../../design/layers.md).
> Question: What belongs in a general, frozen protocol, what in per-project rules, and how those rules can be configurable and amend themselves through their own process without losing efficiency or trust?

> **Correction (2026-11-22):** section 4.5 (and finding F2's *Ours* paragraph) says a v1 replica refuses a v2 or unknown-variant contribution visibly, in sync's `rejected`. It doesn't: such a contribution fails to decode, so the whole sync batch or bundle import fails (`crates/manyroot/src/sync.rs` `batch` via `bundle::read`, `crates/manyroot/src/bundle.rs:72`). See question `e82eaca382d0`, answer `7b337526dc68`, and [layers.md](../../design/layers.md) section 1 (V20). The report below is left as written.

*Research report A, 2026-11-21 (expanded edition).*

**Scope.** What belongs in a general, frozen protocol for signed, content-addressed contribution graphs, what belongs in per-project rules, and how to make those rules configurable, and amendable through their own process, without losing efficiency or trust.

**The owner's three addenda.** Every recommendation is tested against them:
- Manyroot is for meaningful information structures of any kind: software, finance, organisations, processes.
- The founding model is the scientific method and peer review.
- A project's rules should be able to improve themselves through the same machinery.

**Sources.**
- Our own figures come from a read-only export of this repo's Ground (13,981 contributions, `scripts/bk log --json`, 2026-11-21), from retrospective `16cbeff051ea`, and from the code at `main` d3b5c297.
- External sources are linked next to each claim.
- "Abstract only" marks a source I saw only as an abstract or search snippet. "General knowledge" marks a claim I didn't re-check for this report.

---

## Summary

1. **Every lasting open system splits a frozen, schema-agnostic envelope from named, versioned application types.** Examples: AT Protocol Lexicons, Nostr kinds, in-toto predicate types, RO-Crate profiles, nanopublication templates, Datomic's schema-as-data. We have the envelope but no type name. Claims carry only free-text tags, and the body, relation and verdict sets are closed enums.
2. **Free-text typing has failed measurably in our own Ground.**
   - 40 distinct tags, including the malformed `task,deferred`.
   - A tag collision that hid a goal.
   - 5,522 distinct evidence `method` strings across 6,352 evidence records.
   - 2,563 of 3,680 claims quote ids in prose, invisible to the core.
   - The merge gate parses PR numbers and shas out of statements with a regex.

   XBRL (21–24% company-specific tags, falling comparability) and Crossref (34 spellings of update types, including "retration") show the same rot at industrial scale.
3. **Closed unions are where frozen protocols ossify** (Lexicon's own rule, RFC 9170). Today a v1 replica can't even *store* a contribution with a new relation kind, so a new kind forks replication.
4. **Keep per-project rules out of validity.** Holochain (per-app validation) and Matrix (12 room versions) show that rules which decide *validity* force network-wide agreement and migrations. Our split, where validity depends only on a contribution and its parents and acceptance is per-reader policy, lets rules differ and change without forking a Ground. Write this down as a protocol invariant.
5. **Restricted rule languages buy speed, termination and analysability.**
   - Cedar is 29–35× faster than OpenFGA and 43–81× faster than Rego, and has a sound and complete SMT encoding.
   - Zanzibar's small rewrite language serves over 2 trillion tuples at p95 under 10 ms.
   - CEL rules carry static worst-case cost bounds.
6. **Our acceptance rules are Datalog-shaped, not Cedar-shaped.** `needs_recheck` follows chains of relations, status precedence needs layered negation, and "only validators count" is trust scoped by key. Recommendation: stratified Datalog with counting aggregates, key- and role-scoped trust in the style of Biscuit, and built-in causal-past predicates backed by our existing bitset sweep.
7. **Weave is the proof that the core lacks typed payloads and typed references.** It built both on artifacts with media types, then had to reimplement the ancestor rule and scan and decode every payload at each open.
8. **Protocol v2 proposal (section 4), small and additive.**
   - Keep the five body kinds and the seven core relation kinds, which carry all core semantics.
   - Add an open, namespaced `type` on every body.
   - Add a generic relation kind `relates`, with no core semantics.
   - Add typed `refs` and `data`.
   - Add an optional evidence `method_ref` and a third verdict, `inconclusive`.
   - Switch to CBOR Common Deterministic Encoding (CDE).
   - Add greasing test vectors.

   Unknown types are always valid and replicable.
9. **Per-project layer: a "rulebook" contribution (section 5).**
   - Contents: types (fields, indexes, required references), conformance checks (CEL-like, cost-bounded, flagging rather than rejecting), acceptance policies (Datalog), and views.
   - Named by namespace and content id, and importable by content id.
   - Versioned by supersession and signed by stewards rooted in the Ground's founding contribution.
   - Found with `rules --current`.
   - Every evaluation cites the rulebook it ran under.
10. **Three worked rulebooks (section 6) show the mechanism is domain-neutral:** Bracken's software process, a double-entry ledger, and a peer-review community.
11. **The founding scientific model maps onto the frozen core.** Claim, evidence, review, replication, contradiction, author retraction and correction belong there. Consensus, editorial notices, evidence grading and reviewer eligibility are per-community rules. Causal parents give pre-registration for free.
12. **Self-amendment (section 7).** The protocol should provide:
    - rules as contributions;
    - judgement by the rules in force in the subject's causal past;
    - an amendment rule always evaluated under the *previous* rulebook;
    - a protected core that no rulebook can reach;
    - well-formedness checks before acceptance.

    Quorums, periods, expiry and "what counts as improvement" stay per-project. Comparisons covered: Nomic, Tezos, constitutional entrenchment, and Holochain/Matrix upgrades.
13. **Most open questions can be answered from precedent (section 9).**
    - Proposition identity: declared key fields, as Wikidata does.
    - The first rulebook's authority: the founding root, as Matrix's create event and our Weave marker do.
    - Nonconformance: severities that never change data, as SHACL does.
    - Graded evidence: a three-valued core verdict with grading in data, as SEPIO and audit opinions do.
    - Rule time versus reading time: both modes, labelled, as IAS 8 distinguishes retrospective from prospective changes.
    - Migration: per-Ground opt-in, as Git's `objectFormat` does.

---

## 1. Findings, with evidence

### F1. Lasting open systems freeze an envelope and name their application types

| System | Frozen core | Application extension point | Evolution rule |
|---|---|---|---|
| AT Protocol | Signed repo (MST of records), DIDs, CBOR data model, firehose | Lexicon schemas named by NSIDs (reverse-DNS) | "All old data must still be valid under the updated Lexicon, and new data must be valid under the old Lexicon"; breaking changes need a new NSID ([Lexicon](https://atproto.com/specs/lexicon)) |
| Nostr | One event type: `id, pubkey, created_at, kind, tags, content, sig` | Integer `kind`; NIPs define meanings; kind *ranges* encode storage semantics (regular, replaceable, ephemeral, addressable) | Kinds are never redefined; new behaviour gets new kinds ([NIP-01](https://raw.githubusercontent.com/nostr-protocol/nips/master/01.md)) |
| in-toto | DSSE envelope and Statement (`subject` digests and `predicateType` URI) | Predicate, "arbitrary metadata… with a type-specific schema" | Predicate types carry versions in their URI; the Statement's `_type` carries the spec's major version ([spec](https://github.com/in-toto/attestation/blob/main/spec/README.md)) |
| RO-Crate | JSON-LD with schema.org | Profiles declared with `conformsTo`, "mainly duck-typing" | "Guidance over strictness" ([Soiland-Reyes et al. 2022](https://doi.org/10.3233/DS-210053)) |
| Nanopublications | Assertion, provenance and pubinfo graphs; trusty URIs (content hashes) | Templates, themselves published as nanopublications | Content-addressed, so a template version is immutable ([Kuhn et al. 2021](https://www.researchgate.net/publication/349898311), abstract only) |
| Datomic | Datoms (entity, attribute, value, transaction, op) | Attributes are entities "added to the database using `d/transact`"; "schema is composed of ordinary Datomic data" | `:db/valueType` "cannot be updated"; predicates are enforced "starting on the transaction after they are asserted" ([schema reference](https://docs.datomic.com/schema/schema-reference.html)) |
| Git | Four object types, unchanged since 2005 | Commit headers, notes, refs, trailers | Repository format extensions; old Git refuses to open what it can't read ([hash transition](https://git-scm.com/docs/hash-function-transition)) |

**Ours.** The envelope is excellent and frozen: protocol, ground, author, parents, timestamp, signature, BLAKE3 id. But the body is a closed enum of five kinds (`crates/manyroot/src/contribution.rs:26-43`). A claim's only type information is `tags: [text]`. Relation kinds are a closed enum of seven (`:83-98`) and the verdict is a closed pair (`:102-105`). There is no type name, no payload schema and no versioning below the protocol version.

### F2. Closed unions ossify, and ours make new kinds unreplicable

Lexicon: "Closed unions can not change (types can not be added or removed)", so its unions are open by default, and "implementations should be permissive when validating, in case they do not have the most recent version of the Lexicon" ([Lexicon](https://atproto.com/specs/lexicon)).

[RFC 9170](https://www.rfc-editor.org/rfc/rfc9170.html), "Long-Term Viability of Protocol Extension Mechanisms":
- "Mechanisms that aren't used are the ones that fail most often."
- "Only by using the extension capabilities of a protocol is the availability of that capability assured."
- Its case studies:
  - new DNS record types became so hard to deploy that SPF was carried in TXT records;
  - TLS implementations broke on new `signature_algorithms` values;
  - TLS 1.3 abandoned version negotiation in the version field for a new mechanism;
  - IPv4's 240/4 was reserved for future use and never reclaimed.
- Its §3.3 describes GREASE (sending meaningless reserved values to keep implementations tolerant) and its limits.

**Ours.** Decoding uses serde's derived enums. An unknown body or relation variant is a decode error, so the contribution fails Validity rule 2 (protocol.md) and the replica refuses it in sync's `rejected`. A community that wanted `replicates` or `answers` as a relation kind would need protocol v2, and v1 replicas couldn't even carry the new contributions for v2 readers. Validity, the most conservative layer, would be gatekeeping vocabulary.

### F3. Typing by free text has failed in our Ground, and elsewhere at scale

Our Ground (export of 2026-11-21):

| Convention | Measurement | Consequence |
|---|---|---|
| Tags as types | 40 distinct tags across 3,680 claims; `task,deferred` appears as one tag; 141 `task`+`deferred` pairs; 22 `approval`+`delegated` | Gotcha `11230613d276`: a decision tagged `goal` hid the goal. Retractions `157ccddb4284`, `0955b8dd330e` and `8aa4d3881398` fixed wrong or missing tags. `claim --supersedes` didn't copy tags (`5fdf88a2c108`). Weave had to namespace its tag as `weave-goal` "because tags are queries" (weave.md) |
| Evidence `method` as free text | 6,352 evidence records, **5,522 distinct methods**; 5,707 embed `@ <sha>` | One check under 8+ spellings (`16cbeff051ea`). Exact matching "would silently never count" (weave.md). One method captured a whole `PATH` (`8aaacf7d0f2d`) |
| Ids in prose | 2,563 of 3,680 claims contain a 12-hex id | Not checked against `parents`, not indexed, ambiguous with git shas and content hashes (72 such cases in the retrospective) |
| PR claims | 728 `pr` claims, all matching `^PR #n branch @ sha40:` | Held only because `scripts/merge:306` enforces it with a regex; the structure lives in bash, not the protocol |
| Triage | 796 `manager triage` evidence records (785 pass, 11 fail) | A `pass` on a finding means "confirmed" and a `pass` on a PR means "works"; the gate's semantics (blocking, deferral) live in CLI code (`bracken findings`) and AGENTS.md |
| Own evidence | 3,427 of 6,352 evidence records (54%) are by the target's author | They never count under the protocol's acceptance rule. Typed evidence (for example "self-report" versus "check result") would make their role explicit instead of silently ignored |

**Worked example: a deferred finding.** This is real; the ids are from our Ground.
1. The security reviewer writes claim `b7f6724d61bb`, tagged `finding`, with the statement "…process.md:299 low: main's protection is enable_push=true with no push whitelist…". Its file, line and severity are all in prose.
2. It writes relation `9f4e0c6824ff`: finding `contradicts` the PR claim.
3. The manager writes evidence `0b34b7706e86` on the finding: method `"manager triage"` (free text), verdict `pass`, details "Confirmed as a future risk… Deferred to the bot-access decision…".
4. The manager writes claim `284f93c1fd6b`, tagged `task`+`deferred`, with "Task (deferred): if the owner grants bot accounts access…".
5. Relation `f10656def0cd`: the task `depends_on` the finding.

The merge gate decides "deferred, therefore not blocking" from a conjunction:
- a tag string;
- a method string;
- the triager's identity;
- a `depends_on` relation from a claim with a certain tag.

None of this is typed or declared anywhere a generic reader could find it; it is AGENTS.md prose plus Rust in `bracken`. Section 6.1 shows the same rule as six lines of rulebook.

**Elsewhere:**
- **XBRL.** Company-specific extension tags are about 21–24% of tags in SEC filings, and comparability declined more for firms using more extensions ([JAPP 2015](https://www.sciencedirect.com/science/article/abs/pii/S1467089515000378), abstract only). Evidence suggests managers use extensions strategically to raise complexity and obscure ([Huang, No & Vasarhelyi 2019](https://www.researchgate.net/publication/325771935), abstract only). Europe's ESEF responded by requiring every extension to be **anchored** to "the ESEF taxonomy concept having the closest wider accounting meaning" ([XBRL International](https://www.xbrl.org/guidance/esef-rules-anchoring-extensions/)). That is our "super-kind" idea (P1 below), arrived at by regulators.
- **Crossref.** It defines 12 Crossmark update types (`correction`, `retraction`, `expression_of_concern`…; [Crossref](https://www.crossref.org/documentation/crossmark/participating-in-crossmark/)), but a facet query over the deposited data reportedly returned 34 distinct values, including "retration" and "Retraction" ([Hyperresearch guide](https://hyperresearch.ai/guides/retraction-aware-research), secondary, unverified). A controlled vocabulary that isn't validated at write time rots.
- **ORKG.** Crowdsourced structured paper descriptions became heterogeneous enough that comparisons had "many empty values". The fix was expert-written templates and mining recurring predicate groups as template candidates ([D'Souza et al.](https://www.jlis.it/index.php/jlis/article/download/547/529); [arXiv 2210.02034](https://arxiv.org/pdf/2210.02034)).

**Lesson.** Types are needed, they must be validated at write time, and reusing a shared type must be cheaper than inventing one. Extensions must declare how they relate to the shared vocabulary.

### F4. Per-application *validity* rules force network agreement; per-reader *interpretation* rules don't

- **Holochain.** Each app's validation must "always yield the same true/false outcome for a given operation regardless of who is validating them and when". Non-deterministic host functions are blocked, dependencies must be fetched by hash, and "invalid data is treated as an intentionally malicious act" ([Holochain validation](https://developer.holochain.org/build/validation/)). Because peers judge each other by these rules, changing them means a different network (general knowledge: the integrity code is part of the DNA hash).
- **Matrix.** Its room versions bundle event format, auth rules, state resolution, redaction and signing-key rules. There have been 12 so far, for example "v3 introduces events whose IDs are the event's hash" and "v6 alters several authorisation rules", with a warning that new auth rules can leave older servers "in a split-brain situation" ([room versions](https://spec.matrix.org/latest/rooms/)). Upgrading means a new room: the old one gets an `m.room.tombstone`, the new one's create event names its predecessor, and every member's client must notice and move ([MSC1501](https://github.com/matrix-org/matrix-spec-proposals/blob/main/proposals/1501-room-version-upgrades.md)).
- **AT Protocol.** It splits this explicitly: "relays (specifically) should not validate records against lexicons". Relays check signatures, structure and size, and consuming services judge content ([sync spec](https://atproto.com/specs/sync)).

**Ours.** Validity depends only on a contribution and its parents (`crates/manyroot/src/graph.rs:1-7`; protocol.md, Validity), and "what counts as accepted is decided by each reader" (Goal 4). We are already on AT Protocol's side of the line. Per-project rules can live entirely in reading, so two readers with different rulebooks share one Ground, and a rule change needs no migration.

### F5. Restricted rule languages: measured speed, termination and analysis

| Language | Model | Termination and cost | Analysis | Measured |
|---|---|---|---|---|
| **Cedar** | Permit/forbid policies over entities and hierarchies; schema-typed | No loops or recursion; most operators constant time ([Cutler et al.](https://arxiv.org/html/2403.04651v2)) | Sound and complete SMT encoding; equivalence queries average 75.1 ms; validator soundness proved in Lean | 28.7–35.2× faster than OpenFGA, 42.8–80.8× faster than Rego; slicing a further 10–18× |
| **Zanzibar** | Relation tuples plus namespace "userset rewrite" rules (union, intersection, exclusion, following relations) | Bounded by the graph; consistency via "zookies" | Limited | >2 trillion tuples, millions of checks/s, p95 <10 ms, >1,500 namespaces ([Pang et al. 2019](https://www.usenix.org/system/files/atc19-pang.pdf)) |
| **CEL** | Side-effect-free expressions | Non-Turing-complete, with static worst-case cost estimation; Kubernetes rejects rules whose estimate is too high ([KEP-2876](https://github.com/kubernetes/enhancements/blob/master/keps/sig-api-machinery/2876-crd-validation-expression-language/README.md)) | Type-checked | GA in Kubernetes 1.29 |
| **Biscuit Datalog** | Facts, rules, checks, allow/deny; facts scoped by the signing key of their block | Run limits (default 1,000 facts, 100 iterations, 1 ms: [source](https://docs.rs/biscuit-auth/latest/src/biscuit_auth/datalog/mod.rs.html)) | Monotone apart from policy order | Sized for tokens, not graphs |
| **Rego (OPA)** | Datalog-inspired, JSON documents | Built-ins include network calls (`http.send`) and time (general knowledge) | Hard | Slowest in Cedar's comparison |
| **Soufflé / stratified Datalog** | Recursive rules, stratified negation, aggregates | Polynomial data complexity without value invention (general knowledge) | Well studied | Compiled; used for program analysis at scale |
| **SHACL** | Shapes graphs validating data graphs | Recursive shapes "not defined" ([W3C](https://www.w3.org/TR/shacl/)) | Results never change data | Mature tooling |

### F6. Our acceptance rules already need Datalog's features

The protocol's acceptance and `follow_changes` rules (protocol.md, Acceptance) need:
- **Recursion.** The dependency closure follows `depends_on`, `implements` and `refines` transitively, with conditions on authors at each step.
- **Stratified negation.** Retracted outranks rejected, which outranks accepted, which outranks pending. "Current" means nothing live supersedes it.
- **Counting.** `min_passes` is a count of distinct authors.
- **Trust scoping.** `validators` restricts whose evidence counts. Biscuit's `trusting ed25519/…` generalises this.
- **Causal-past tests.** "Had every change among their pass's ancestors" (`needs_recheck`).

Cedar has only hierarchy membership and no recursion over arbitrary relations, so the closure is out of reach. CEL can't join. Zanzibar's rewrites handle role membership well but not status precedence. Datalog with stratified negation and aggregates covers all of these. Causal-past tests should be a built-in predicate backed by the bitset sweep `validation.rs` already does (`Seen::sweep`), because materialising `ancestor(a, b)` is quadratic.

### F7. Weave already built the missing layer, and paid for it

Weave defines:
- seven media types under `application/vnd.bracken.weave` (`crates/tree/src/format.rs:26-35`);
- a `"v": 1` in every payload, with a frozen-format guard (weave-freeze.md);
- definition-hashed check methods (`<name> blake3:<hash of [name, argv]>`; weave.md, Checks);
- a line policy that is a signed record citing an owner decision.

That is a per-application type system with schemas, versions, typed method identity and policies as records. It is the right design, but the core couldn't help:
- **Typed references** live inside payloads, so Weave re-implements an ancestor rule (`crates/tree/src/ground.rs:175-201`) to check that every referenced id is a causal ancestor. That is core Validity rule 5, rebuilt.
- **Index build** (`ground.rs:204+`) visits every contribution in id order and decodes every Weave payload, at every open. The retrospective estimated about 45 MB of Weave data in four days, "all decoded at every open".
- **Goals** still had to be claims tagged `weave-goal`, because claims have no type.

### F8. Fewer, shared extension points last longer

- **Rekor.** v1 offered 11 pluggable entry types (rpm, jar, helm, tuf, alpine, intoto…). Rekor v2 (GA October 2025, tile-backed on Trillian-Tessera) keeps only `hashedrekord` and `dsse`. The rest were removed "due to a lack of usage and to simplify the API" ([Sigstore blog](https://blog.sigstore.dev/rekor-v2-ga/)).
- **RFC 9170 §4.1:** "Use of a shared extension point for any purpose can protect rarer or more specialized uses."
- **Git:** four object types for 20 years.

**Implication.** Don't add many body kinds. Add *one* generic mechanism, a type name plus typed data and typed references, that every application uses, so it is exercised constantly and can't rust.

### F9. Scholarly encodings got the atoms right; the social layer is where they struggled

| Encoding | What it encodes | What worked | What didn't |
|---|---|---|---|
| **Nanopublications** | Assertion, provenance and publication-info graphs; trusty-URI content hashes; optional signatures; decentralised servers; templates as nanopubs | Immutable, verifiable atoms; a decentralised network; templates for authoring | Uptake mostly in a few biomedical datasets (general knowledge). Signing was "experimental" in early tooling ([nanopub-java](https://arxiv.org/pdf/1508.04977)) |
| **Micropublications** | Claims argued by data, methods and materials; challenge and support links ([Clark et al. 2014](https://jbiomedsem.biomedcentral.com/articles/10.1186/2041-1480-5-28)) | Claims need their evidence chain, transitively | Heavy to author; little adoption beyond pilots (general knowledge) |
| **SEPIO** | Proposition versus assertion, evidence lines with direction and strength, methods and agents ([Brush et al. 2016](https://ceur-ws.org/Vol-1747/IT605_ICBO2016.pdf)) | Used for clinical variant interpretation (ClinGen) | Its repo says the OWL file is outdated; development moved to a LinkML model and "profiles" ([GitHub](https://github.com/monarch-initiative/SEPIO-ontology)). The general core stayed small, and communities profiled it |
| **CiTO** | About 90 typed citation properties (supports, disputes, extends, usesMethodIn…), all sub-properties of `cites` (general knowledge) | Generic tools can treat everything as `cites` | Low author uptake for fine-grained typing (general knowledge) |
| **Wikidata** | Statements with qualifiers, references and ranks; best rank for queries ([Help:Ranking](https://www.wikidata.org/wiki/Help:Ranking)) | Provenance kept separate from preference; deprecated values kept, not deleted | One global, editable rank. "Ranks are not a way of asserting your view for a disputed value", so disputes move to talk pages |
| **ORKG** | Papers' contributions as structured comparisons | Templates, once added | Heterogeneity without templates (F3) |
| **Crossmark** | 12 types of post-publication update by publishers | Machine-readable retraction status | Vocabulary drift (F3) |

**Mapping to us.**

| Scientific concept | Our structure today | Layer |
|---|---|---|
| Claim or proposition | `claim` (the author's assertion; no separate proposition, see Q1) | Core |
| Data, materials, manuscript | `artifact` | Core |
| Typed citation | `relation`; CiTO's "everything is a sub-property of cites" equals our proposed super-kind rule | Core kinds plus typed subtypes |
| Review | `evidence` by a non-author | Core |
| Replication | Counted evidence from distinct authors (`min_passes`); "same method" needs P3's `method_ref` | Core plus policy |
| Contradiction or failure to replicate | `contradicts` | Core |
| Author retraction or correction | `retract`; `supersedes` | Core |
| Editor's retraction or expression of concern | Someone else's statement, which a policy may weigh | Rulebook |
| Consensus or "best rank" | Derived per reader by policy, never stored | Rulebook |
| Pre-registration | A method contribution in the result's causal past (F11) | Core mechanism, rulebook requirement |

### F10. What the core can see decides what can be indexed

- **Nostr:** "all single-letter… key tags are expected to be indexed by relays", and "only the first value in any given tag is indexed".
- **AT Protocol:** records live under collection NSIDs, so `listReposByCollection` exists.
- **Datomic:** an index is generated only for attributes with `:db/index`.

**Ours.** The graph indexes ids, children, referrers and heads (`graph.rs:86-97`). Everything else is a scan. The retrospective measured about 0.75 s per `bk` read, "filtered or not", before the snapshot cache. A type name, typed references and rulebook-declared indexes would make "current decisions", "open findings for PR n" and "check results under method m" index lookups.

### F11. Causal ancestry gives pre-registration and fixed rules for free

Registered reports work because methods are fixed before results are known. In a causal graph, "M is in R's causal past" is unforgeable once R is signed: it doesn't depend on timestamps, which are advisory here. So:
- a result that names a pre-registered analysis plan as its `method_ref` proves the plan came first;
- a judgement under the rulebook in force in the subject's causal past can't have been rigged after the fact (section 7).

Weave's definition-hashed check method is the software special case. In a ledger it is "the valuation model existed before the valuation". In governance, "the procedure was adopted before the vote".

### F12. Self-amending systems separate tiers and vote on exact content

Covered in section 7: Nomic's immutable and mutable rules, Tezos's amendable amendment procedure, voted on by hash, and constitutional entrenchment.

---

## 2. What our design gets right

1. **Validity is local and monotone; acceptance is policy.** This is AT Protocol's relay/AppView split and Holochain's determinism without per-app code, and it avoids Matrix's room-version churn. "Truth isn't a CRDT" (vision.md) matches SEPIO's split between assertion and evidence, and it improves on Wikidata's single global rank, because each community gets its own best rank.
2. **A small, frozen, well-guarded envelope.** `protocol_guard.rs` pins golden vectors, negative vectors, causal order, bundles, sync and a fixture Ground, and mutation sweeps check the guard. RFC 9170 §4.2 recommends declaring invariants, and Git's and Rekor's histories favour small cores.
3. **Five domain-neutral speech acts** (claim, artifact, relation, evidence, retract). Every domain in the addenda needs exactly these. Section 6 shows a ledger and a science community expressed in them without new kinds.
4. **Author-only retraction and supersession.** These are the only acts that change how *another* reader sees someone's contribution, so tying them to the author is the minimal trust assumption. Third-party retractions (Crossmark's publisher notices) are rightly statements weighed by policy.
5. **Content addressing with the Ground bound into the signature.** Ids are global, like trusty URIs, and replay across Grounds is impossible.
6. **Weave's patterns:** namespaced types, a version per payload, definition-hashed methods, policy as a signed record, and frozen tiers. The core should adopt them.
7. **`needs_recheck`.** Acceptance follows changes in what a subject depends on. This is a domain-neutral form of "the evidence base changed, re-review", which journals handle only by manual notices.

---

## 3. What is wrong or missing

| # | Problem | Where | Consequence |
|---|---|---|---|
| W1 | Closed enums; an unknown variant is a decode error and makes the contribution invalid | `contribution.rs:26-43, 83-105`; protocol.md Validity 2 | New vocabulary forks replication (F2) |
| W2 | No type name; tags are the type system | `Body::Claim { tags }` | Collisions, drift, no index (F3) |
| W3 | No structured payload on claims or evidence | protocol.md, Body kinds | Structure parsed from prose (`scripts/merge:306`) |
| W4 | References in prose or payloads are invisible to the core | 2,563 claims; `tree/src/ground.rs:175` | No ancestry check or index; each app rebuilds them (F7) |
| W5 | Policy isn't a record: CLI flags and bash | `validation.rs:19-30`; `bracken/src/main.rs:70-80, 2112-2117`; `scripts/merge` classes | Results can't say which rules produced them; agents can't discover the rules |
| W6 | Policy model too narrow: no types, methods, roles or class-dependent requirements | `Policy { min_passes, validators, follow_changes }` | Gate semantics in AGENTS.md prose and CLI code (the worked example in F3) |
| W7 | Binary verdict | `Verdict { Pass, Fail }` | "Inconclusive" and graded evidence go into `details` text |
| W8 | Canonical encoding is serde's field order | protocol.md Encoding, *Open* | A second implementation must copy our serializer |
| W9 | Algorithm agility undefined | protocol.md Identifiers | Undecided, not necessarily wrong (Q8) |
| W10 | No test that unknown types are tolerated | `protocol_guard.rs` | Extension points rust (RFC 9170) |
| W11 | No notion of which rules were in force when work was judged | — | Rules can be changed after seeing results; no reproducible acceptance history |

---

## 4. Protocol v2, in enough detail to evaluate

### 4.1 Design choices

1. **Keep the five body kinds and seven relation kinds closed.** They carry every core semantic (retraction, supersession, conflicts, acceptance, closure), and closing them keeps those rules decidable from the contribution alone.
2. **Put all openness in one shared mechanism:**
   - a `type` string on every body;
   - typed `refs`;
   - opaque but canonical `data`;
   - one new relation kind, `relates`, which has no core semantics. Extension relations that are neither support, contradiction, dependency, refinement, supersession, implementation nor test go here (for example `answers`, `debits`, `cites-as-background`).
3. **An extension relation picks its core kind in the contribution itself.** For example `kind: supports, type: "sci.replicates"`. A reader without the rulebook still knows exactly what the relation means to the core. This is CiTO's "sub-property of cites" and ESEF's anchoring, made mandatory and machine-checkable without any registry.
4. **Unknown types are always valid and replicable.** Validity checks only the type's *syntax*.
5. **No new mutable or ephemeral semantics** (Nostr's replaceable kinds). "Latest version" stays supersession, judged per reader.

### 4.2 Encoding

Use CBOR **Common Deterministic Encoding** ([draft-ietf-cbor-cde](https://datatracker.ietf.org/doc/draft-ietf-cbor-cde/)): preferred serialisation, definite lengths only, and map keys sorted bytewise. A "CDE-checking decoder" rejects anything else. The v1 rule, re-encode and compare, still applies on top as a belt-and-braces check. This settles protocol.md's *Open* note: a second implementation needs only CDE, not our serializer.

Map keys become small unsigned integers. That is compact, sorts trivially and needs no field-order convention, and the JSON form keeps readable names. In CDDL:

```cddl
contribution = { 0: content, 1: signature }
content = {
  0: 2,                       ; protocol
  1: ground-id,               ; bstr .size 32
  2: agent-id,                ; bstr .size 32 (Ed25519)
  3: [* contribution-id],     ; parents: strictly ascending, no duplicates
  4: uint,                    ; timestamp_ms, advisory
  5: body,
}
body = claim / artifact / relation / evidence / retract

claim    = { 0: 0, ? 1: type, 2: tstr,  ? 3: [* tag], ? 4: refs, ? 5: data }
                                  ; statement
artifact = { 0: 1, ? 1: type, 2: tstr,  6: tstr, 7: bstr, ? 4: refs, ? 5: data }
                                  ; name, media_type, content
relation = { 0: 2, ? 1: type, 8: rel-kind, 9: contribution-id, 10: contribution-id, 2: tstr, ? 5: data }
                                  ; kind, from, to, note
evidence = { 0: 3, ? 1: type, 9: contribution-id, 11: verdict, 12: tstr, 2: tstr,
             ? 13: contribution-id, ? 4: refs, ? 5: data }
                                  ; target, verdict, method, details, method_ref
retract  = { 0: 4, 9: contribution-id, 2: tstr }   ; target, reason

rel-kind = 0..7    ; supports, contradicts, depends_on, refines, supersedes, implements, tests, relates
verdict  = 0..2    ; pass, fail, inconclusive
type     = tstr .regexp "[a-z][a-z0-9-]*(\\.[a-z0-9-]+)+(@[0-9]+)?"
                   ; e.g. "bracken.pr", "sci.replicates", "ledger.entry@2"
tag      = tstr .regexp "[a-z0-9][a-z0-9._-]*"     ; no commas or spaces
refs     = [* [tstr, contribution-id]]             ; [role, id], strictly ascending by (role, id)
data     = any                                     ; CDE CBOR, counted in the size limit
```

Notes:
- **`type` is optional.** An absent type is the bare core kind, so v1 semantics map one-to-one.
- **Namespaces.** A type's namespace (the part before the first dot) is advisory. Authority over a namespace is a rulebook matter (section 5.4), not validity. The `@n` suffix is a *major* version: a breaking change gets a new major version (Lexicon's rule), and readers treat `x@2` as distinct from `x`.
- **Tags** stay as free labels for people, now syntax-checked so that `task,deferred` can't happen.
- **`data`** is CBOR, not bytes, so generic tools can show it and rulebooks can select fields from it. Large binary content stays in artifacts.

### 4.3 Validity rules (v2), changes from v1

1. `protocol` is 1 or 2. A v2 replica validates v1 contributions by v1 rules, unchanged.
2. The encoding is CDE and within 16 MiB.
3. The signature is strict Ed25519 over `"manyroot/v2/contribution:" || id`. The context changes so that v1 and v2 signatures can never be confused.
4. **(generalised)** Every id in `refs`, every relation end, every evidence or retract target, and `method_ref` is listed in `parents`.
5. `refs` is strictly ascending and has no duplicate pair. Roles are non-blank.
6. `type` and tags match their syntax.
7. A relation's `from` and `to` differ, and `method_ref` differs from `target` (both v1-style rules).
8. The v1 retract-author rule and blank-text rules carry over.

Nothing about the *meaning* of `type` or `data` affects validity.

### 4.4 Derived state (v2)

- Every v1 rule is unchanged for core kinds. `type` changes no core semantics.
- `relates` takes part in nothing: no conflicts, no closure, no acceptance.
- `inconclusive` evidence never counts as a pass or a fail under the core policy. Rulebooks may weigh it.
- **New core indexes** (storage-local, not replicated):
  - by `type`;
  - by `(role, id)` for refs;
  - by `method_ref`;
  - by `(type, field)` for fields a rulebook marks indexed (section 5).

### 4.5 Compatibility and rollout

- **v1 contributions stay v1 forever.** Re-encoding would change ids (releases.md), and v2 parents may be v1 ids. One Ground holds both.
- **Rollout per Ground, following Git's `objectFormat` extension.** Old Git refuses to open what it can't read, rather than misreading it ([Git](https://git-scm.com/docs/hash-function-transition)).
  1. Replicas advertise `v2` in sync `hello.features`. The field already exists, and unknown features are ignored.
  2. A steward writes a v1 claim of type-tag `manyroot-v2-enabled` (tags are all v1 has).
  3. Writers emit v2 only after that claim and only to peers advertising `v2`.

  A v1-only replica refuses v2 contributions (protocol ≠ 1), which is correct and visible in `rejected`.
- **Guard.** New golden vectors for every v2 body and relation kind and every verdict. Negative vectors for:
  - non-CDE encodings;
  - unsorted refs;
  - a ref not in parents;
  - bad type or tag syntax;
  - a v1 signature context on a v2 contribution.

  A `ground-v2` fixture sits beside `ground-v1`.
- **Grease vectors (RFC 9170).** Contributions with unseen types (`zz-grease.x7f3`), unseen ref roles and arbitrary `data` must validate, replicate and show as their core kind. Releases rotate the grease names so no implementation hard-codes them.

### 4.6 Cost estimate

| Part | Work |
|---|---|
| `manyroot` | A new body encoding (hand-written CDE codec or ciborium with a CDE check); widened rules 4–6; new indexes; v1 path untouched |
| Guard | v2 vectors and fixture; a differential test that v1 statuses are unchanged |
| `bracken` | `--type`, `--ref role=id`, `--data` (JSON to CBOR), `--method-ref`; `log --type`; JSON output of the new fields |
| Weave | Nothing at first. Later, a Weave format v2 that puts references in `refs` and its record kind in `type`; v1 stays readable under the freeze |
| Risk class | Protocol: independent review, owner approval and a migration note per AGENTS.md §1 |

Size impact is negligible. Integer keys save about 50 bytes per contribution against v1's text keys, which roughly offsets typical refs.

### 4.7 Things deliberately *not* in v2

- **Cross-Ground references.** Parents must be present, and Canopy is separate. Section 9, Q5 gives a content-hash approach that needs none.
- **Mutable or replaceable records.** Supersession covers them.
- **Hash agility fields.** Q8 explains why recording an invariant suffices.
- **Rulebook semantics in validity** (P5).

---

## 5. The rulebook: per-project configuration as signed contributions

### 5.1 What a rulebook is

A rulebook is an `artifact` with `type: "manyroot.rulebook"`, `media_type: "application/vnd.manyroot.rulebook+cbor"` and CDE content, compiled from a text source that is also stored, as a second artifact referenced with role `source`. It has seven sections:

1. **Header:** name (`<namespace>/<name>`), imports (by content hash), and the previous version (via `supersedes`).
2. **Roles:** how agents get roles. Either a fixed list of keys, or a rule: role claims of a given type signed by a given role.
3. **Types:** for each `type`, its core kind, `data` schema (CDDL subset), required and allowed ref roles with their target types, indexed fields, and an optional proposition key (Q1).
4. **Conformance checks** per type: CEL-like expressions over the contribution and its direct refs, each with a severity (`violation`, `warning`, `info`).
5. **Policies:** named Datalog programs, each defining `status(X, S)` for the subjects it covers.
6. **Views:** named Datalog queries, for example `open_findings(P, F)`.
7. **Amendment:** the policy a successor rulebook must satisfy (section 7).

### 5.2 Conformance language

A CEL subset:
- field access, comparisons, boolean logic, string `matches` and `startsWith`, `size()`;
- bounded comprehensions (`all`, `exists`, `map`, `sum`) over lists inside `data`;
- `ref(role)` returning the referenced contribution's core fields and type.

There are no joins beyond direct refs. Costs are estimated statically, as Kubernetes does, and a rulebook whose checks exceed a fixed budget is itself nonconforming.

**Semantics.** A failed check produces a *conformance result* labelled with its severity. It never changes validity or the stored data, following SHACL: "The data graph and the shapes graph MUST remain immutable", and "sh:severity [has] no impact on the validation" ([W3C SHACL](https://www.w3.org/TR/shacl/)). Policies see conformance as facts (`nonconforming(X, Severity)`) and decide. The default is that a `violation` keeps a subject out of the accepted layer. Clients run the same checks at write time and refuse by default, so most drift never enters.

### 5.3 Policy language

Datalog with:
- **Stratified negation and stratified aggregates** (`count`, `sum`, `min`, `max`), with no value invention: rules can't create new ids or numbers except through aggregates.
- **Extensional facts the engine extracts from the graph:**
  - `contribution(X, Kind, Type, Author)`, `parent(X, P)`, `ref(X, Role, Y)`;
  - `relation(R, RelKind, From, To)`;
  - `evidence(E, Target, Verdict, MethodRef)`;
  - `retracted(X)` and `superseded_by(X, Y)`, per the core rules;
  - `field(X, Path, Value)`, for indexed fields only;
  - `nonconforming(X, Severity)`;
  - `role(Agent, Role)`.
- **Built-ins** backed by native code:
  - `in_past(A, B)` ("A is a causal ancestor of B"), via the existing bitset sweep;
  - `core_status(X, S)` (the v1 policy, as a fast path);
  - `rulebook_in_force(X, R)` (section 7).
- **Trust scoping.** Every fact carries its author. A policy declares `trusting role(...)` or `trusting key(...)` per rule, as in Biscuit, so a rule can't accidentally count an untrusted author's statement.
- **Limits.** Facts, iterations and time are bounded per run, as Biscuit's `RunLimits` are. The defaults must suit graphs of about 10⁶ facts, not tokens.

**Performance.** Our Ground yields about 30k base facts (14k contributions, 3.7k relations, 6.4k evidence, 0.2k retracts, plus types and roles). Semi-naive evaluation over that is milliseconds (general knowledge). Incremental maintenance on each new contribution is a solved problem:
- DBSP defines incremental versions of recursive queries and was formalised in Lean ([Budiu et al., VLDB 2023](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf), best paper);
- differential dataflow is a production alternative.

The causal-past built-in avoids the quadratic `ancestor` relation. The current Rust `Policy` stays as the fast path. **A differential test requires the Datalog form of the core policy to produce identical statuses on `ground-v1`, `ground-v2` and this repo's Ground.** That is how the engine earns trust before any project relies on it.

**Analysis.** Datalog without value invention allows bounded checks such as "is policy B at least as strict as A on every graph with up to n contributions?", by enumeration or SAT. That is weaker than Cedar's complete SMT encoding, but enough to catch an amendment that silently loosens a gate (section 7).

### 5.4 Naming, versioning, signing, discovery

- **Name:** `<namespace>/<name>`, for example `bracken/process` or `sci-review/core`.
  - A namespace owned by a Ground is that Ground's id: `g:3fa9…/process`.
  - A namespace owned by a domain is verified as Lexicon does: a DNS TXT record at `_manyroot.<domain>` names the steward key.
  - Names are labels. The **content hash** of the compiled rulebook is its identity.
- **Versions:** each version supersedes its predecessor. A change keeps a type's name only if old data stays conformant and new data conforms to the old version (Lexicon's rule, checked mechanically by running both rulebooks' checks over the Ground). Otherwise the type gets a new major version, `x@2`.
- **Imports:** by content hash, for example `imports sci-review/core @ blake3:…`. The imported rulebook's bytes are copied into the Ground as an artifact with the same content (Q5), so evaluation never needs another Ground. Shared vocabularies (an accounting chart of types, a review rubric) are reused, not forked. This addresses the XBRL problem.
- **Signing and authority:**
  - The Ground's founding contribution, or a v1 steward claim for existing Grounds, names a **root key**.
  - The first rulebook counts if the root signed it.
  - Each later rulebook counts if accepted under its predecessor's amendment policy (section 7).
  - This follows Weave's root-signed marker and line policies citing owner decisions.
- **Discovery:**
  - `bracken rules --current` resolves the current rulebook by following supersession from the root's first one, counting only accepted versions.
  - `--json` gives types, policies, views and the content hash.
  - Agents read it at task start, replacing hand-run tag queries.
  - `bracken accepted --policy merge-gate` evaluates a named policy, and every output carries `{rulebook: <hash>, policy: "merge-gate"}`.

---

## 6. Three rulebook sketches

The syntax is illustrative: CDDL-like types, CEL checks and Soufflé-like Datalog. `:-` reads "if", and `!` is negation.

### 6.1 Bracken's software process

```text
rulebook bracken/process
imports manyroot/core @ blake3:…          # the core policy as Datalog, plus standard roles

roles
  role(A, R) :- contribution(C, claim, "bracken.role", Root), root(Root),
                field(C, "agent", A), field(C, "role", R), !retracted(C).

types
  claim bracken.task      { data { title: tstr, class: "docs"/"code"/"protocol"/"storage"/"crypto"/"gate" } }
  claim bracken.pr        { data { number: uint, branch: tstr, head: bstr .size 20,
                                   weave_proposal: ? bstr .size 32 }
                            refs { implements: bracken.task (1) }
                            index number, head
                            key [number, head] }                 # proposition key: one PR head
  claim bracken.check-def { data { name: tstr, argv: [+ tstr] } key [name, argv] }
  evidence bracken.check  { method_ref: bracken.check-def
                            data { head: bstr .size 20, exit: int, merged_with: ? bstr .size 20 } }
  claim bracken.finding   { data { file: tstr, line: ? uint,
                                   severity: "low"/"medium"/"high"/"critical",
                                   label: "confirmed"/"possible" }
                            refs { against: bracken.pr (1) } }
  evidence bracken.triage { data { disposition: "confirmed"/"rejected"/"deferred" } }
  relation depends_on/bracken.defers                          # task -> finding

checks
  bracken.pr:      violation  data.branch.matches("^[a-z0-9][a-z0-9/._-]*$")
  bracken.check:   violation  data.head == ref("target").data.head
  bracken.finding: warning    data.label == "confirmed" || data.severity != "critical"

policy merge-gate                                              # status of bracken.pr subjects
  trusting role(_, _)
  risky(P)    :- ref(P, "implements", T), field(T, "class", C), C != "docs", C != "code".
  need(P, "manager")  :- contribution(P, claim, "bracken.pr", _).
  need(P, "security") :- contribution(P, claim, "bracken.pr", _).
  need(P, "reviewer") :- risky(P).
  passed(P, R) :- evidence(E, P, pass, _), contribution(E, _, _, A), role(A, R),
                  contribution(P, _, _, B), A != B, !retracted(E).
  required_check(N) :- N = "fmt" ; N = "clippy" ; N = "clippy-linux" ; N = "test-table".
  green(P, N) :- evidence(E, P, pass, M), contribution(E, _, "bracken.check", A), role(A, "manager"),
                 field(M, "name", N), !retracted(E).
  triaged(F, D) :- evidence(E, F, _, _), contribution(E, _, "bracken.triage", A), role(A, "manager"),
                   field(E, "disposition", D), !retracted(E).
  deferred(F) :- triaged(F, "deferred"), relation(R, depends_on, T, F),
                 contribution(R, _, "bracken.defers", A), role(A, "manager"), !retracted(T).
  blocking(P, F) :- ref(F, "against", P), contribution(F, claim, "bracken.finding", _), !retracted(F),
                    !triaged(F, "rejected"), !deferred(F).
  missing(P) :- need(P, R), !passed(P, R).
  missing(P) :- contribution(P, claim, "bracken.pr", _), required_check(N), !green(P, N).
  status(P, rejected) :- core_status(P, rejected).
  status(P, blocked)  :- !core_status(P, rejected), blocking(P, _).
  status(P, ready)    :- !core_status(P, rejected), !blocking(P, _), !missing(P).
  status(P, pending)  :- !core_status(P, rejected), !blocking(P, _), missing(P).

view open_findings(P, F) :- blocking(P, F).

amend
  status(R, accepted) :- contribution(R, artifact, "manyroot.rulebook", _),
                         passed(R, "manager"), owner_approved(R).
  owner_approved(R) :- evidence(E, R, pass, _), contribution(E, _, _, A), role(A, "owner").
```

**What it replaces:**
- the regex at `scripts/merge:306` (typed `bracken.pr` data plus a conformance check);
- "pass means confirmed" (`bracken.triage` with an explicit disposition);
- deferral by tag (`bracken.defers`);
- the per-class `--validator`/`--min-passes` choice made by hand at each merge (`risky` and `need`);
- 5,522 method spellings (`method_ref` to `bracken.check-def`, keyed by name and argv, which is Weave's definition hash, generalised).

The F3 worked example becomes three typed contributions and one relation, judged by six readable rules.

### 6.2 A double-entry ledger

```text
rulebook acme-finance/ledger
imports std-accounts/core @ blake3:…      # a shared chart of account types (the "taxonomy")

roles  (fixed keys for controller, approver, auditor; role claims signed by controller)

types
  claim ledger.account   { data { code: tstr, class: "asset"/"liability"/"equity"/"income"/"expense",
                                  anchor: tstr } key [code] }
                          # anchor: the std-accounts concept it narrows, as ESEF requires of extensions
  claim ledger.entry     { data { date: tstr, memo: tstr,
                                  lines: [+ { account: uint, amount: int, currency: tstr }] }
                           refs { account: ledger.account (1..*) } index date }
                          # lines[i].account indexes into the account refs, in ref order
  claim ledger.close     { data { period: tstr } }              # parents = the books as closed
  claim ledger.procedure { data { name: tstr, text: tstr } }    # an audit or valuation procedure
  evidence ledger.approval {}
  evidence ledger.audit  { method_ref: ledger.procedure
                           data { opinion: "unqualified"/"qualified"/"adverse"/"disclaimer" } }
  relation supersedes/ledger.reversal                          # corrections reverse, never edit

checks
  ledger.entry:  violation  data.lines.map(l, l.currency).all(c,
                              data.lines.filter(l, l.currency == c).map(l, l.amount).sum() == 0)
                            # debits equal credits, per currency
  ledger.entry:  violation  data.lines.all(l, l.amount != 0)
  ledger.account: violation std_accounts.has(data.anchor)     # every extension is anchored

policy posted                                                  # status of ledger.entry subjects
  trusting role(_, _)
  total(X, T)    :- contribution(X, claim, "ledger.entry", _), T = sum A : field(X, "lines.amount.abs", A).
  approvals(X, N):- N = count A : evidence(E, X, pass, _), contribution(E, _, "ledger.approval", A),
                    role(A, "approver"), contribution(X, _, _, B), A != B, !retracted(E).
  needed(X, 2)   :- total(X, T), T >= 1000000.             # four-eyes above a threshold
  needed(X, 1)   :- total(X, T), T <  1000000.
  late(X)        :- contribution(C, claim, "ledger.close", _), field(C, "period", P),
                    field(X, "date", D), in_period(D, P), !in_past(X, C).
                    # dated in a closed period but not in the books as closed
  status(X, accepted) :- needed(X, N), approvals(X, M), M >= N, !late(X), !nonconforming(X, violation).
  status(X, late)     :- late(X).
  status(X, pending)  :- needed(X, N), approvals(X, M), M < N, !late(X).

view balance(Account, Currency, Sum) :- … sum over accepted, non-superseded entries …
view audited(Close) :- evidence(E, Close, pass, M), contribution(E, _, "ledger.audit", A),
                       role(A, "auditor"), field(E, "opinion", "unqualified"), in_past(M, Close).
                       # the procedure was fixed before the close it audits
```

**Domain points the core supplies unchanged:**
- **Immutability with corrections by reversal.** `supersedes` by the author, with the history kept. This is event sourcing's "never update, append a compensating event".
- **Period close as a causal cut.** The close's parents are the books as closed, so "posted before close" is `in_past`, not a timestamp comparison that a backdated entry could game.
- **Segregation of duties.** "Own evidence never counts" is already core, and the four-eyes threshold is policy.
- **Audit opinions.** Four levels map to `pass` (unqualified), `pass` with `data` (qualified), `fail` (adverse) and `inconclusive` (disclaimer). This is why `inconclusive` belongs in the core (Q4).
- **Shared taxonomy plus anchored extensions**, which is the ESEF fix for XBRL's extension sprawl.

### 6.3 A peer-review and science community

```text
rulebook open-review/core

roles
  role(A, "editor")   :- signed role claims by the community root
  role(A, "reviewer") :- editor-signed role claims
  affiliation(A, Org) :- self-declared claims of type sci.affiliation (data { org })

types
  claim    sci.hypothesis   { data { statement_formal: ? tstr, variables: ? [* tstr] } }
  claim    sci.prereg       { data { analysis_plan: tstr, primary_outcome: tstr, n_planned: uint }
                              refs { tests: sci.hypothesis (1) } }
  artifact sci.dataset      {}
  evidence sci.result       { method_ref: sci.prereg
                              data { effect: float, ci_low: float, ci_high: float, n: uint }
                              refs { data: sci.dataset (0..*) } }
  evidence sci.review       { data { recommendation: "sound"/"minor"/"major"/"unsound",
                                     rubric: tstr } }
  relation supports/sci.replicates
  relation contradicts/sci.fails-to-replicate
  claim    sci.notice       { data { kind: "expression_of_concern"/"retraction"/"correction"/"reinstatement" }
                              refs { about: any (1) } }        # Crossmark's vocabulary, typed

checks
  sci.result: violation  data.ci_low <= data.effect && data.effect <= data.ci_high
  sci.result: warning    data.n >= ref_method().data.n_planned  # under-powered relative to the plan

policy established                                              # status of sci.hypothesis subjects
  trusting role(_, _), key(_)                                   # anyone may report results; reviewers gate them
  independent(A, B) :- A != B, !(affiliation(A, O), affiliation(B, O)).
  preregistered(E)  :- evidence(E, _, _, M), contribution(M, claim, "sci.prereg", PA),
                       contribution(E, _, _, EA), in_past(M, E).
                       # the plan was in the result's causal past
  reviewed(E) :- count R : evidence(R, E, pass, _), contribution(R, _, "sci.review", RA),
                 role(RA, "reviewer"), contribution(E, _, _, EA), independent(RA, EA) >= 2.
  support(H, E, A)  :- evidence(E, H, pass, _), contribution(E, _, "sci.result", A),
                       preregistered(E), reviewed(E), !retracted(E), !noticed(E, "retraction").
  refute(H, E, A)   :- evidence(E, H, fail, _), contribution(E, _, "sci.result", A),
                       preregistered(E), reviewed(E), !retracted(E).
  noticed(X, K)     :- contribution(N, claim, "sci.notice", Ed), role(Ed, "editor"),
                       ref(N, "about", X), field(N, "kind", K), !retracted(N).
  replicated(H)     :- support(H, _, A), support(H, _, B), independent(A, B).
  status(H, retracted)  :- core_status(H, retracted).
  status(H, contested)  :- replicated(H), refute(H, _, _).
  status(H, established):- replicated(H), !refute(H, _, _), !noticed(H, "expression_of_concern").
  status(H, preliminary):- support(H, _, _), !replicated(H), !refute(H, _, _).
  status(H, open)       :- contribution(H, claim, "sci.hypothesis", _), !support(H, _, _), !refute(H, _, _).

view weight_of_evidence(H, Pooled) :- … left to application code (meta-analysis isn't Datalog's job) …
```

**What this shows:**
- **Replication** is "two independent, pre-registered, reviewed supporting results". Independence (distinct authors with no shared affiliation) and pre-registration (`in_past`) are rules, not core fields.
- **Editorial retraction** is a typed notice by an editor role, weighed by policy, while author retraction stays core. This is the Crossmark distinction, typed so that "retration" can't happen.
- **Consensus is a view, never a stored fact.** Another community can import the same types and define `established` with one result and no affiliation rule, over the same Ground.
- **Statistics stay in application code.** Pooling effect sizes, Bayesian updating and Lakatosian programme-level judgements are computations a policy language shouldn't attempt. Datalog decides *which* evidence counts; applications weigh it.

---

## 7. Self-amendment: protocol support for rules that improve themselves

The goal: a project's rules (its rulebook, including policies, checks, process and the amendment procedure itself) can be changed through its own claim, evidence and review process. Today's manual version is `experiment` claims with rule, metric, baseline, target and expiry, lapsing unless a metric-citing decision supports them; weekly `metrics` claims; and one rule in, one out (AGENTS.md §6).

### 7.1 Comparisons

- **Nomic** (Suber, 1982). Rules are immutable or mutable. Immutable rules take precedence and must be "transmuted" to mutable before amendment. Suber's [*Paradox of Self-Amendment*](https://legacy.earlham.edu/~peters/writing/psa/sec01.htm) argues that a rule amending itself leaves only two options: an unamendable supreme clause, or contradiction. His resolution is "direct acceptance": entrenchment ultimately rests on what participants accept.
- **Tezos** ([Octez docs](https://octez.tezos.com/docs/active/voting.html)):
  - "the proposal, voting and activation processes are part of the economic protocol itself… the amendment rules themselves are subject to amendments";
  - five periods of 14 cycles (about 14 days) each: proposal, exploration, cooldown, promotion, adoption;
  - quorum from a moving average of participation, bounded between 20% and 70%;
  - supermajority above 80% of Yea plus Nay stake;
  - voters vote on "the hash of the files that constitute the source code";
  - if the hash names an invalid protocol, "the activation step fails" and the chain stalls.
- **Constitutions** entrench a core behind supermajorities (US Article V), and some forbid particular amendments outright (Germany's Basic Law Art. 79(3), general knowledge).
- **Holochain and Matrix.** Rule changes mean a new DNA or a new room version, with every member migrating (MSC1501: tombstone, predecessor, clients must follow). This is costly because their rules decide validity.

**Lessons:**
1. Separate tiers, so the amendment procedure can't be bypassed by an ordinary change.
2. Vote on exact content (a hash).
3. Leave time between acceptance and effect.
4. Check that a proposal is well-formed before it can win.
5. Keep rule changes off the validity path, so nobody must migrate.

### 7.2 What the protocol, plus the engine in `manyroot`, should provide

1. **Rules as versioned contributions.** Section 5's rulebook. A proposed rulebook is an ordinary subject that collects evidence: metric results with `method_ref` to a metric definition, and reviews. History is the supersession chain, and the content hash is the exact text voted on (Tezos's lesson).
2. **Rules recorded before the work they judge.** The built-in `rulebook_in_force(X, R)` gives the *accepted* rulebook current in X's causal past. A view in *as judged* mode evaluates each subject under its own `R`. *Restated* mode evaluates everything under the reader's current rulebook, and its output carries both hashes (Q7). Nobody can change a rule after seeing the result and have the change silently apply to that result.
3. **Amendment evaluated under the previous rulebook.** A rulebook R′ becomes current only if it is `accepted` under the `amend` policy of `rulebook_in_force(R′)`, the rulebook in R′'s own causal past, never under R′'s own `amend`. A proposal that names its author sole validator and approves itself fails, because the old amendment policy judges it. Changing `amend` itself goes the same way, which gives Nomic's two-step transmutation without special cases.
4. **A protected core outside every rulebook:**
   - validity, signatures, causal order and the core derived state (retraction, author-only supersession, conflicts);
   - the root key named at founding, changeable only by the root's own signature (Weave's root-signed marker is the precedent);
   - rule 3 itself.

   Changing these is a protocol version: owner-approved, deliberately expensive and out of band. Suber's "direct acceptance" remains as the ultimate escape: any reader can evaluate under any rulebook, and the shared evidence is unaffected. Exit is always available, so entrenchment never traps anyone.
5. **Well-formedness before acceptance.** A candidate rulebook must compile, pass its own cost bounds, and satisfy the compatibility check for every type whose name it keeps (5.4). Otherwise it is nonconforming and can't be accepted. This avoids Tezos's invalid-hash stall.
6. **Bounded impact analysis attached as evidence.** An automated validator runs the candidate's policies against the current Ground and records, as `inconclusive` or `pass` evidence with `data`, the list of subjects whose status would change. An `amend` policy can then require "no accepted subject becomes unaccepted without a reviewer's pass", which is the self-improvement analogue of a migration diff.
7. **Typed metrics.** A metric definition is a claim (`type: "*.metric"`, with data giving the query or command). A measurement is evidence with `method_ref` to it. "An experiment lapses unless a decision cites metrics meeting its target" becomes Datalog over typed facts instead of prose.

### 7.3 Better left to each project

- quorums, supermajorities, voting weights, periods and cooldowns;
- experiment expiry and lapse (timestamps are advisory, so lapse is judged by the reader's clock or by an explicit `lapse` contribution);
- one-rule-in-one-out and size budgets;
- which metrics define improvement;
- stake- or reputation-weighted voting (Sybil-sensitive and domain-specific);
- automatic activation.

The protocol should make these expressible and auditable, not choose them. Tezos shows a working amendment procedure can live entirely inside the amendable layer, provided the meta-rule of point 3 and the protected core sit outside it.

---

## 8. Recommendations, with layer, benefit and cost

Layer key: **P** = protocol (frozen, every application); **R** = rulebook engine and format (domain-neutral mechanism, per-project contents); **A** = application.

| # | Recommendation | Layer | Benefit | Cost | Order |
|---|---|---|---|---|---|
| P5 | State the invariant: project rules never affect validity | P (doc) | Rule changes never fork a Ground | None | Now |
| P4a | Pin encoding to CDE for v2; grease vectors | P | Second implementations; tolerant readers | Low | With v2 |
| P1 | `type` on every body; `relates` kind; extension relations carry their core kind | P v2 | New vocabulary without protocol versions; generic readers keep semantics | Medium | v2 |
| P2 | Typed `refs` and `data` | P v2 | Core-checked, indexed references; ends prose parsing; Weave's ancestor rule becomes core | Medium | v2 |
| P3 | Evidence `method_ref`; `inconclusive` verdict | P v2 | Pre-registration; replication under the same method; audit opinions and inconclusive results | Low | v2 |
| R4 | Indexes by type, ref and declared fields | storage | Reads become lookups | Medium | With v2 |
| R1 | Rulebook format, naming, imports, discovery, root authority | R | Policy as a record; types registered; agents discover rules | Medium | First (works on v1 with tags as types) |
| R2 | CEL-subset conformance with severities | R | Drift caught at write, flagged at read | Low–medium | With R1 |
| R3 | Stratified Datalog policies with trust scoping and causal-past built-ins; differential test against the core policy | R | Project-defined acceptance without code; bounded analysis | High | After R1 and R2 |
| R5 | Self-amendment support (section 7: rules in force, old-rule amendment, protected core, impact evidence) | R (meta-rule protected by P) | Rules improve through review, safely | Medium | With R3 |
| A1 | Bracken's process as a rulebook (6.1) | A | Retires regexes and prose gates | Medium | After R3 |
| A2 | Weave formats declared in a rulebook; Weave format v2 on P2 | A | Generic tools read Weave; index without decoding | Medium | After v2 |

Every P and R item passes the domain-neutrality test: none mentions code, and sections 6.2 and 6.3 use them unchanged. A1 and A2 are deliberately Bracken's.

---

## 9. Open questions, with what research settles

**Q1. Proposition identity.** When two authors assert the same thing, do we get one proposition or two?
- *Precedent:*
  - Wikidata addresses statements by item and property, so duplicates are visible by structure.
  - SEPIO separates proposition from assertion.
  - ORKG's heterogeneity came from having no keys.
- *Answer:* the core keeps assertions as they are, since every claim is someone's. A rulebook type may declare `key` fields (as in 6.1), and readers index a proposition key `hash(type, canonical key fields)`. Normalisation is domain-specific (R); the index is generic (storage). This also gives duplicate detection, a hard problem the vision names.
- *Still open:* key collisions across rulebooks that use the same type name differently. Mitigation: key on the type's defining rulebook hash.

**Q2. Who authorises the first rulebook, and how do stewards change?**
- *Precedent:*
  - Matrix's create event, where v12 gives creators unlimited power;
  - Weave's root-signed marker;
  - Tezos's amendment rules inside the amendable protocol.
- *Answer:* the founding contribution names a root key. The root signs the first rulebook; later ones follow 7.2(3); the root changes only by its own signature. Multi-party roots (k-of-n keys) are a natural later extension, through the same `amend` mechanism over a "root" rulebook.

**Q3. Should nonconforming contributions be hidden or flagged?**
- *Precedent:*
  - SHACL results never change the data, and severities are labels.
  - Lexicon validates optimistically ("fail-open") but treats data that fails validation as "entirely invalid".
  - Crossref's 34 update-type spellings show what happens without validation at write time.
- *Answer:* strict at write time (clients refuse a `violation`), tolerant at read time (stored, visible, flagged). Policies decide whether nonconformance excludes from acceptance, and the default `violation` does.

**Q4. Graded evidence: is pass/fail/inconclusive enough?**
- *Precedent:*
  - SEPIO models evidence direction and strength separately.
  - Audit opinions have four levels that map onto pass/fail/inconclusive plus data.
  - Medical evidence grading (GRADE) varies by field (general knowledge).
- *Answer:* a three-valued core verdict, since decision procedures need a discrete, domain-neutral direction. Strength and scales go in typed `data` (R). Portability across communities comes from importing shared rulebooks, not from a core strength field.

**Q5. How are rules shared across Grounds before references across Grounds exist?**
- *Precedent:*
  - Lexicon publishes schemas as records, resolved by DNS;
  - nanopub templates are themselves nanopubs;
  - Weave identifies file content by content hash, independent of the contribution carrying it.
- *Answer:* import by content hash, and copy the bytes into the importing Ground as an artifact (new contribution id, same content hash). Any Ground can verify the import offline; Canopy isn't needed. A namespace's DNS record says which steward key's rulebooks are authoritative for that namespace, so readers can warn about lookalike copies.

**Q6. Is a Datalog engine in `manyroot` justified before a second domain exists?**
- *Evidence for waiting:*
  - R1 and R2 deliver most of the typing benefit alone.
  - Biscuit's engine is tuned for about 1,000 facts, so we'd need a different engine design.
- *Evidence for building:*
  - Our own gate already exceeds `Policy` (F3's worked example).
  - DBSP and differential dataflow make incremental evaluation tractable.
  - Section 6 shows identical engine needs in three domains.
- *Answer:* build R1 and R2 first. Prototype R3 against this Ground with the differential test. Then encode one non-software Ground, a small ledger or decision register as in 6.2, to confirm the engine's facts and built-ins are complete before freezing the rulebook format.

**Q7. Rules in force at the time of the work, or the reader's current rules?**
- *Precedent:*
  - Legal systems presume against retroactivity (general knowledge).
  - Accounting (IAS 8) applies changes in accounting *policy* retrospectively, restating comparatives with disclosure, but changes in *estimates* prospectively (general knowledge).
  - Datomic enforces new attribute predicates "starting on the transaction after they are asserted", and a new uniqueness constraint "does not change history".
- *Answer:* support both modes, labelled. *As judged* is the default for gates and history; *restated* is for analysis and cleanup. `needs_recheck` can extend naturally: an accepted subject whose rulebook in force has since been superseded *and* whose restated status differs is flagged for re-review. That gives continuous re-validation without silent retroactivity.

**Q8. Hash and signature agility.**
- *Precedent:*
  - Git's SHA-1 to SHA-256 plan took from 2017 into the 2020s. It needed translation tables and dual signatures, and it relies on hash *length* to tell signed payloads apart: "a future same-length hash would need a new payload format" ([Git](https://git-scm.com/docs/hash-function-transition)).
  - Multihash-style tags cost bytes on every reference.
- *Answer:* keep 32-byte BLAKE3 ids and Ed25519 in v2, and record two invariants:
  - a future id algorithm comes with a new protocol version *and* a different id length;
  - signing contexts always name the protocol version (v2's context does).

  That is Git's working mechanism, chosen deliberately rather than inherited.

**Q9. Migration of existing Grounds.**
- *Precedent:*
  - Matrix rooms keep their version until explicitly upgraded;
  - Git's `objectFormat` makes old clients refuse rather than misread.
- *Answer:* v1 contributions stay v1 forever. A Ground opts into v2 by a steward claim; replicas advertise `v2` in `hello.features`; writers switch only when peers support it (4.5). Rulebooks (R1) can start on v1 immediately, using tags as type names and statements' structured prefixes, so the rule layer doesn't wait for v2.

**Q10. What stops type sprawl?**
- *Precedent:*
  - XBRL's 21–24% extension rate;
  - ESEF anchoring;
  - Rekor pruning unused types;
  - ORKG mining recurring predicate groups.
- *Answer:* the mandatory core kind (P1), imports of shared rulebooks, compatibility checks for any reused name, and a metric: the share of contributions per type and per rulebook origin, reported by `scripts/metrics`. A type nobody uses should be retired, as Rekor did.
