# Manyroot protocol, version 2: design

*Design on paper (task `fdbc081f07db`), 2026-11-22. Nothing here is frozen.
The contents of the package are the owner's decision `ea6f5419a31e`
(points c2–c4); the freeze comes after the non-code pilot and needs the
owner's own approval (AGENTS.md, section 1). Version 1, in
[protocol.md](../protocol.md), is unchanged by this document.*

Sources: the research synthesis of 2026-11-21 (section 3.1 and the roadmap
in section 4), research report A (section 4: its CDDL and rollout), report
B (R6), and the code at `main` d3b5c297. Where the research and the code
disagreed, the code is the fact (section 6.3; question `e82eaca382d0`,
answer `7b337526dc68`).

## 1. Purpose and principles

Manyroot is a general protocol for meaningful information structures of
any kind: software, financial designs, organisations, business processes.
Its founding model is the scientific method and peer review: claims,
evidence, review, replication, contradiction, retraction and correction.
Version 2 has to serve every such domain without building in any one
project's rules. Five principles follow.

1. **Project rules never affect validity** (protocol.md, Validity). A v2
   rule may look only at the contribution and its parents. Nothing about
   the *meaning* of a type, a ref role or a `data` value decides validity.
2. **The core stays closed; openness has one door.** The five body kinds,
   the core relation kinds and the verdicts stay closed enums, because they
   carry every core semantic (retraction, supersession, conflicts,
   acceptance, the dependency closure) and closed sets keep those decidable.
   Everything new goes through one shared mechanism: a `type` name, typed
   `refs`, canonical `data`, and the meaning-free relation `relates`. A new
   *core* kind will always mean a new protocol version; v2 moves that
   point, it doesn't remove it.
3. **Unknown types are always valid and replicable.** Validity checks only a
   type's syntax. Grease vectors keep that true (section 5).
4. **v1 contributions stay v1 forever.** Re-encoding would change their ids.
   One Ground holds both versions, and v2 parents may be v1 ids.
5. **Agility, as protocol.md states:** v2 keeps 32-byte BLAKE3 ids and
   Ed25519, and its signing context names the version.

## 2. The package

| Item | In v2 | Why |
|---|---|---|
| `type` on every body except retract | yes | the shared extension point; v1 has only free-text tags |
| `relates` relation kind, no core semantics | yes | new vocabulary (`answers`, `debits`) without a protocol version |
| Typed `refs`, `[role, id]`, each in `parents` | yes | references the core checks and orders; today 3% of cited ids are parents |
| `data`, canonical CBOR, on claims, artifacts, evidence and relations | yes | values for indexes and aggregates in every domain |
| Evidence `method_ref` | yes | pre-registration, replication under the same method, versioned metrics |
| `inconclusive` verdict | yes | a discrete "neither": a null result, an audit disclaimer |
| CBOR Common Deterministic Encoding, integer keys, header first | yes | a second implementation needs only CDE, not our serializer |
| Grease vectors | yes | keeps `type`, roles, `data` and features tolerant |
| Out-of-line payloads, with erasure of payload bytes | yes | lazy loading, partial replicas, and data subject to deletion |
| Signing context `manyroot/v2/contribution:` | yes | v1 and v2 signatures can't be confused |
| Batch contributions | **no** | parallel strict verification is enough; a batch complicates per-record retraction and supersession (synthesis C4) |
| Validated `height` | **no** | a full replica derives it; only partial replicas would need it in the format (synthesis C3) |
| ZIP-215 verification | **no** | a different validity rule, bought only for ingest speed nobody needs yet (synthesis C5) |

Also not in v2 (report A 4.7): references across Grounds, mutable or
replaceable records (supersession covers them), hash-agility fields (the
invariant suffices), and rulebook semantics in validity.

## 3. Encoding

### 3.1 CDE

A v2 contribution is encoded in CBOR Common Deterministic Encoding
([draft-ietf-cbor-cde](https://datatracker.ietf.org/doc/draft-ietf-cbor-cde/)):
preferred serialization (shortest integers and lengths), definite lengths
only, map keys sorted bytewise by their encoding, no duplicate keys. A
replica decodes with a CDE-checking decoder, then still re-encodes and
compares, as in v1: two independent checks, so a decoder bug that accepts a
non-CDE form is caught by the comparison.

Map keys are small unsigned integers. Keys 0–23 encode as one byte, so
bytewise order is numeric order. The JSON form keeps readable names
(section 3.4). Numbering the content's header fields 0–4 and the body 5
puts the header first in the bytes, so indexers and sync can stop decoding
before the body (report B, R6d) at no cost.

**Closed shapes.** Every map the protocol defines (content, each body,
payload) is closed: an unknown key fails validity, as an unknown field
fails v1's re-encoding today. Extension happens only through `type`,
`refs` and `data`, never through new keys.

### 3.2 CDDL

Starting from report A's CDDL (section 4.2), with out-of-line payloads and
the corrections in section 3.3:

```cddl
contribution = { 0: content, 1: signature }
signature    = bstr .size 64                ; Ed25519

content = {
  0: 2,                                     ; protocol
  1: ground-id,                             ; bstr .size 32
  2: agent-id,                              ; bstr .size 32, Ed25519 public key
  3: [* contribution-id],                   ; parents: strictly ascending, no duplicates
  4: uint,                                  ; timestamp_ms, advisory only
  5: body,
}

body = claim / artifact / relation / evidence / retract

claim    = { 0: 0, ? 1: type, 2: text,                    ; statement
             ? 3: [* tag], ? 4: refs, ? 5: data }
artifact = { 0: 1, ? 1: type, 2: text,                    ; name
             6: tstr,                                     ; media_type
             (7: bstr // 14: payload),                    ; inline content, or a payload
             ? 4: refs, ? 5: data }
relation = { 0: 2, ? 1: type, 8: rel-kind,
             9: contribution-id, 10: contribution-id,     ; from, to
             2: tstr,                                     ; note
             ? 5: data }
evidence = { 0: 3, ? 1: type, 9: contribution-id,         ; target
             11: verdict, 12: text,                       ; method
             2: tstr,                                     ; details
             ? 13: contribution-id,                       ; method_ref
             ? 4: refs, ? 5: data }
retract  = { 0: 4, 9: contribution-id, 2: tstr }          ; target, reason

payload  = { 0: bstr .size 32,                            ; hash: BLAKE3 of the payload bytes
             1: uint }                                    ; size in bytes

rel-kind = 0..7  ; supports, contradicts, depends_on, refines, supersedes, implements, tests, relates
verdict  = 0..2  ; pass, fail, inconclusive
text     = tstr  ; not blank (protocol.md, Validity rule 8)
type     = tstr .regexp "[a-z][a-z0-9-]*(\\.[a-z0-9-]+)+(@[1-9][0-9]*)?"
                 ; "bracken.finding", "ledger.entry@2"; at most 128 bytes
tag      = tstr .regexp "[a-z0-9][a-z0-9._-]*"            ; at most 64 bytes; no commas, spaces or colons
refs     = [+ [role, contribution-id]]                    ; strictly ascending by (role, id)
role     = tstr .regexp "[a-z][a-z0-9_-]*"                ; at most 64 bytes
data     = data-value
data-value = int / bstr / tstr / bool / null / decimal
           / [* data-value] / { * data-key => data-value }
data-key = uint / tstr
decimal  = #6.4([int, int])                               ; exponent, mantissa (RFC 8949 3.4.4)
```

Retract carries no `type`, `refs` or `data`: it means exactly one thing.

### 3.3 Corrections to report A's CDDL

- **Payloads.** Artifact content is inline (`7`) or out of line (`14`),
  exactly one of them (section 4).
- **`refs` non-empty when present**, and roles have a syntax rather than
  only "non-blank", so a role can't be confused with a tag or a type.
- **`data` is restricted.** Report A had `data = any`. Floats are left out
  (NaN payloads and `-0.0` have several encodings, and CDE's float rules
  are where independent implementations most often differ); exact
  quantities use integers or the decimal tag 4, which a ledger needs
  anyway. No other tags, no `undefined`, no simple values but booleans and
  null. Map keys are integers or text. Nesting is at most 16 levels deep.
  Each of these is a proposal for the freeze (section 8).
- **Length bounds** on `type`, tags and roles, so an index key is bounded.
- **The type version** is `@1` and up, never `@0` or a leading zero, so one
  version has one spelling.

### 3.4 JSON form

Readable names, ids in lowercase hex, as in v1: `protocol`, `ground`,
`author`, `parents`, `timestamp_ms`, `body: {kind, type, statement, ...}`,
`payload: {hash, size}`. `data` maps to JSON with the decimal tag shown as
`{"decimal": [exp, mantissa]}` and byte strings as `{"bytes": "<hex>"}`.
The JSON form is for reading only; the id is always over the CBOR.

### 3.5 Id and signature

- The id is `BLAKE3(CDE(content))`, 32 bytes, as in v1.
- The signature is strict Ed25519 (RFC 8032, small-order and non-canonical
  points rejected) over `"manyroot/v2/contribution:" || id`.

## 4. Out-of-line payloads and erasure

### 4.1 Payloads

An artifact may carry `payload: {hash, size}` instead of inline `content`.
The payload bytes are a separate blob, identified by `hash` (BLAKE3 of the
bytes, 32 bytes, the id algorithm: the agility invariant covers it).

- **Validity never needs the bytes.** A payload artifact is valid when its
  record is; whether a replica holds the blob is storage, not validity.
  That is what lets validity converge while blobs come and go.
- **A blob is checked on arrival.** A replica stores or serves a blob only
  if its BLAKE3 equals `hash` and its length equals `size`; anything else
  is refused as a blob, which never affects any contribution.
- **Size.** The record keeps v1's 16 MiB limit. A payload in v2 is also at
  most 16 MiB, so one blob always fits a sync message; chunked payloads for
  larger content are an open question (section 8).
- **Inline or out of line** is the writer's choice. A Bracken default might
  put content over 64 KiB out of line. Content that may ever need erasure
  must be out of line from the start (4.3).

### 4.2 Erasure

Erasure deletes a payload's bytes and keeps its signed record. The record,
its id, its signature, its hash commitment and everything that cites it
stay valid and replicate as before.

- **The request** is a claim of type `manyroot.erase` with one ref
  `["erases", <artifact id>]` (so the artifact is a parent) and a statement
  of the reason. The `manyroot.` namespace is reserved for types this
  specification defines.
- **Who counts.** An erase request by the artifact's own author always
  counts. Who else may erase (a data-protection steward, say) is the
  Ground's policy, like acceptance: it never decides validity.
- **What a conforming replica does** with a request that counts: deletes
  the blob, stops serving it, and refuses to store it again while the
  request stands. Reads show the artifact with "payload erased" and its
  hash and size. If the request is retracted, replicas may accept the
  bytes again from anyone who still holds them; nothing obliges anyone to
  supply them.
- **Acceptance is unchanged.** Erasure changes no status: the artifact,
  its evidence and its relations read as before. A policy may of course
  decide that evidence over erased bytes can no longer be re-checked.

### 4.3 What erasure can't do

These limits belong in front of any domain that adopts it.

- **Inline fields are permanent.** A claim's statement, tags and `data`, an
  artifact's name and media type, inline content, evidence details,
  relation notes and retract reasons can't be erased. Personal data must
  go into payloads from the start; a rulebook can refuse at write time a
  type that puts it inline.
- **Copies can't be recalled.** A replica that doesn't follow the rule,
  one offline when the request was written, a backup, or anyone who
  already read the bytes keeps them. Erasure is a rule for conforming
  replicas, not a guarantee.
- **Git history keeps what was committed.** This repository commits
  `.bracken/` to git (decision `34dd510c2457`). A Ground that may need erasure
  must keep payload blobs out of anything git tracks; erasing a committed
  blob means rewriting history, which is an owner-gated destructive action.
- **Metadata stays.** That the artifact existed, its author, time, name,
  media type, size and hash, and every claim about it.
- **A hash commits to guessable content.** If the bytes are low-entropy (a
  salary, a yes or no), anyone can confirm a guess against the public
  hash. Writers of erasable data should put a random nonce of at least 16
  bytes inside the payload bytes (for example, a media type whose format
  has a nonce field), so the hash reveals nothing once the bytes are gone.
  Whether the protocol should frame every payload with a salt is an open
  question (section 8): it would stop dedup of public content by hash.
- **Not legal advice.** Whether this meets a given deletion right is for
  the domain's own counsel.

## 5. Validity, derived state and grease

### 5.1 Validity rules (v2)

A v2 replica adds a contribution only if all of these hold. A v1
contribution is validated by v1's rules, unchanged.

1. `protocol` is 1 or 2, and `ground` is this replica's Ground. A
   `protocol: 2` contribution is in the v2 encoding, and a `protocol: 1`
   one in v1's.
2. The encoding is CDE, every map is closed (3.1), and the contribution is
   within 16 MiB.
3. The signature verifies strictly for `author` under the v2 context.
4. `parents` are strictly ascending, and every parent is already present.
5. Every contribution the body references is in `parents`: every id in
   `refs`, both relation ends, an evidence or retract target, and
   `method_ref`.
6. A relation's `from` and `to` differ; an evidence's `method_ref`
   differs from its `target`.
7. A retract's target has the same `author`.
8. A claim's `statement`, an artifact's `name` and an evidence's `method`
   are not blank, as in v1 rule 8 (the same 25 White_Space characters).
9. `type`, tags and roles match their syntax and length bounds; `refs` are
   strictly ascending by `(role, id)`; `data` keeps to 3.3's kinds and
   depth; an artifact has exactly one of inline content and a payload.

Every rule still depends only on the contribution and its parents. Either
version may have parents of either version: a v2 contribution usually has
v1 parents, and a v1 contribution written by a v2 replica may name v2
parents. Such a v1 contribution has v2 ancestry, so section 6.2 never
sends it to a v1 peer; after the opt-in, writers write v2 anyway.

### 5.2 Derived state

- Every v1 rule (causal order, retraction, supersession, the reference
  policy, `follow_changes`, conflicts) applies unchanged to the core kinds,
  whatever their `type`. `type` changes no core semantics.
- `relates` takes part in nothing: no conflicts, no closure, no acceptance.
  An extension relation that *has* a core meaning uses that core kind with
  a type (`kind: supports, type: "sci.replicates"`), so a reader without
  the project's rulebook still knows what it means to the core.
- `inconclusive` evidence never counts as a pass or a fail under the
  reference policy, so it can't make a subject accepted or rejected.
  Project policies may weigh it.
- Causal order covers v1 and v2 contributions together: the tie-break is by
  id, and ids have one length.
- Indexes by type, by `(role, id)`, by `method_ref` and by fields a
  rulebook marks are storage-local, never replicated.

### 5.3 Grease

Following RFC 9170, the guard holds contributions with types, ref roles
and `data` no implementation knows (`zz-grease.x7f3`, role `zz-grease`, an
arbitrary nested `data`), and a `hello` with an unknown feature. They must
validate, replicate and read as their core kind. Each release adds new
grease names, so nothing can hard-code them.

## 6. Rollout

### 6.1 Per Ground, opt-in

Following Git's `objectFormat` extension: a replica that can't read
something refuses it rather than misreading it.

1. **Replicas advertise v2** with the feature `v2` in `hello.features`. The
   sync protocol stays version 1 (`hello.protocol` is 1), since a peer whose
   `hello.protocol` isn't 1 ends the session; unknown features are ignored,
   so a v1 replica is unaffected.
2. **A Ground opts in** with a v1 claim tagged `manyroot-v2-enabled`, signed
   by the Ground's root. The core has no root identity today; Weave pins one
   that readers are given as local configuration (weave-marker.md, option
   A). The opt-in uses that model: it counts only when signed by the root
   identity the reader is configured with. Making the root part of the
   Ground itself is an open question (section 8).
3. **Writers emit v2 only after an opt-in that counts.** This is a writer
   rule, not validity: a v2 replica accepts a valid v2 contribution whether
   or not the Ground opted in, so validity still depends only on the
   contribution and its parents.

### 6.2 Mixed sessions

- **Toward a v1 peer.** A v2 replica sends a peer that doesn't advertise
  `v2` only contributions whose whole ancestry is v1. Its candidates for
  that peer leave out every v2 contribution and everything after one; it
  may still send its heads. The v1 peer sees the v1 part of the Ground and
  nothing else.
- **Bundles.** A bundle for v1 readers holds only contributions with an all-v1
  ancestry and keeps the `MRBUNDL1` magic. A bundle that may hold v2
  contributions uses a new magic, `MRBUNDL2`, so a v1 reader refuses it
  at once with "not a bundle" rather than partway through. Between two v2
  replicas, sync batches use `MRBUNDL2`. In `MRBUNDL2` an entry is decoded
  by its first bytes: a v1 contribution is a map whose first key is the
  text `content`, a v2 one a map whose first key is 0.
- **Payloads** travel separately from records, between v2 replicas only,
  with two messages under the `v2` feature: a request for blobs by hash, and
  a reply carrying one blob. Their exact shape is for the freeze. Records
  sync without their blobs, and a replica may fetch blobs lazily.

### 6.3 What a v1 replica actually does with a v2 contribution

Report A (4.5) and the synthesis (3.1) say a v1 replica refuses a v2
contribution "visibly, in `rejected`". **The code does otherwise**, and
this design follows the code, not a change to v1 (answer `7b337526dc68`):

- A v2 contribution doesn't reach the protocol-version rule. A v1 decoder
  can't decode the integer-keyed CDE form, so it fails to decode
  (`Contribution::decode` in `contribution.rs`). Only a v1-encoded
  contribution saying `protocol: 2` reaches that rule and is rejected
  with "unsupported protocol version" (`graph.rs:146`).
- In a sync session, `bundle::read` fails on the first entry it can't
  decode (`bundle.rs:72`), so the whole batch is refused and the session
  ends with "bad batch: malformed contribution: ..." (`sync.rs:481`). The
  batch's other contributions aren't imported, and nothing appears in
  `rejected`. A bundle import fails whole in the same way.

So the rule in 6.2 is what keeps mixed sessions working, and **the visible
signal** to a v1 replica when a sender ignores that rule is the session
error (or the import error, or "not a bundle" for `MRBUNDL2`). Nothing is
ever misread. A tolerant v1 reader that listed unknown-version entries in
`rejected` and went on is an open question, not part of this design: it
would change v1's behaviour (section 8).

## 7. How the typed v1 conventions map onto v2

Task `8c9ad9bd4518` (builder-retire) adds typed conventions on v1 tags,
each mapping one-to-one onto a v2 field so that there is one type system
with two spellings, read by an adapter and never rewritten. The mapping
below follows that task's brief; when its convention document lands in
`docs/development.md`, this table is checked against it.

| v1 convention | v2 field | Notes |
|---|---|---|
| tag `type:<ns.name>` | `type` | at most one per claim. An unnamespaced legacy name (tag `finding`) maps through a rulebook's legacy alias (`bracken.finding`), never by rewriting |
| tag `key:value` | an entry `key => value` in `data` | the type's shape gives the value's kind (integer, decimal, enum, id, date as text), so the adapter converts the text; a key repeated in v1 becomes an array |
| tag `ref:<role>=<id>` | an entry `[role, id]` in `refs` | v1 already puts the id in `parents` (the CLI does), which is exactly v2's rule 5 |
| plain tags (`decision`, `task`) | `tags` | v2 checks their syntax; v1's write-time check matches it |
| a `claim --contradicts <pr>` one-call write | a claim plus a `contradicts` relation | unchanged: both versions use the core relation |
| evidence method in prose (5,522 distinct strings) | `method_ref` to a method claim, plus `method` text | v1 evidence has no tags, so it has no convention; a v1 writer can cite a method claim as a parent and in `details` |
| an extension relation (`answers`) | `relates` with a `type` | v1 has no such kind: a typed claim with two `ref:` tags stands in |
| inconclusive result | verdict `inconclusive` | v1 has none; v1 writers record `pass` or `fail` with the reason in `details`, or no evidence |
| out-of-line content, erasure | `payload`, `manyroot.erase` | none in v1: every v1 artifact is inline and permanent |

The adapter is read-time only: a reader sees a v1 claim with typed tags
and a v2 claim with fields as the same typed record, and both keep their
own ids.

## 8. Vectors a freeze will need

Pinned in `protocol_guard.rs` next to v1's, which never change; a
`ground-v2` fixture sits beside `ground-v1`.

- **Golden.** One v2 contribution of every body kind, every relation kind
  (`relates` included) and every verdict (`inconclusive` included); a
  typed claim with refs and `data` (every data kind: int, decimal, bytes,
  text, bool, null, array, map with uint and text keys); an artifact
  inline and one with a payload; evidence with `method_ref`. Encoding, id
  and signature in hex, as in v1.
- **Negative.** Non-CDE forms (a non-shortest integer or length, unsorted
  map keys, a duplicate key, indefinite length, trailing bytes); an unknown
  key in content, in each body and in a payload; a float, an unknown tag,
  `undefined` and depth 17 in `data`; refs unsorted, duplicated, empty, or
  not in parents; `method_ref` not in parents, and equal to the target;
  bad and over-long `type`, tag and role; an artifact with both inline
  content and a payload, and with neither; a payload hash of 31 bytes;
  a v1 signing context on a v2 contribution and the v2 context on a v1
  one; `protocol: 3`; a v2-encoded contribution saying `protocol: 1`; a
  retract with a `type`; and the size boundary at 16 MiB.
- **Grease.** 5.3's unknown types, roles, `data` and feature.
- **Mixed causal order.** v1 and v2 contributions interleaved, pinning one
  order across versions.
- **Differential.** A v2 replica opens `ground-v1` and gets exactly v1's
  statuses, conflicts and order.
- **Bundles and sync.** An `MRBUNDL2` bundle holding both versions; a v1
  reader refusing it by magic; a `hello` advertising `v2`; the payload
  request and reply messages; a mixed session in which the v1 side
  receives only the all-v1 part.
- **Payloads and erasure.** A payload artifact valid with its blob absent;
  a blob of the wrong hash or size refused without touching the record; an
  author's erase request removing the blob and refusing it again; another
  author's request not counting under the default; a retracted request
  letting the blob back.
- **Reference policy.** `inconclusive` never accepting or rejecting.

## 9. Open questions for the non-code pilot and the freeze

The pilot (decision `ea6f5419a31e` point c5: a decision register or a
small double-entry ledger, synthetic data, no model spend at first) should
answer the first group; the rest are for the freeze.

**From the pilot:**
1. Which `data` kinds a ledger or register actually needs: is decimal
   (tag 4) enough for amounts, do dates need a tag, are floats ever needed?
2. Is the depth limit of 16 right, and does `data` need a size cap below
   the record's?
3. Does ingest stay within parallel strict verification, or does the
   ledger make the case for batches after all (synthesis C4)?
4. Do claims and evidence need payloads too, for erasable statements, or
   is "personal data goes in a payload artifact, cited by ref" workable?
5. Which ref roles and types recur across domains, and does any want a
   core meaning that the seven kinds don't give?
6. Proposition keys (report A, Q1): do duplicate assertions need a key in
   the format, or is an index over `(type, key fields)` enough?

**For the freeze:**
7. Payloads over 16 MiB: chunking (and FastCDC) in v2, or later?
8. A protocol salt framing for payloads (4.3), against dedup by hash.
9. The Ground's root in the core (6.1), or kept as reader configuration.
10. Who besides the author may erase, and whether the default should be
    configurable per Ground.
11. A tolerant v1 reader that lists unknown-version entries in `rejected`
    and imports the rest (6.3). It would change v1's behaviour, so it needs
    its own protocol review and the owner; not decided here.
12. Whether the sync bookmark's replica-instance feature (synthesis G6)
    and the payload messages ship with v2 or separately.
13. Type namespaces: is authority over a namespace purely a rulebook
    matter, as report A proposes, or does `manyroot.` need anything more
    than this document's reservation?

## 10. Cost and class

- `manyroot`: a v2 codec (a hand-written CDE codec, or ciborium with a CDE
  check), the widened rules, payload storage and erasure, `MRBUNDL2`, the
  v1 path untouched.
- The guard: section 8's vectors and `ground-v2`.
- `bracken`: `--type`, `--ref role=id`, `--data`, `--method-ref`,
  `--inconclusive`, payload and erase commands, JSON of the new fields.
- Weave: nothing at first. A Weave format v2 on core `refs` is a
  frozen-format change for after the join, with the owner (synthesis G11).
- Class: protocol. The freeze needs an independent review, the manager's
  `owner-review`, the owner's own approval and a migration note in
  [releases.md](../releases.md).
