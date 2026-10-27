# Manyroot protocol, version 1 (draft)

This document specifies what replicas must agree on to interoperate. It
describes the reference implementation in `crates/manyroot`, but it doesn't
depend on it. Anything not specified here, such as storage layout, local
names or validation policy, is a local choice.

## Goals

1. Any number of participants can contribute concurrently without
   coordination.
2. Replicas that hold the same contributions agree on all of their state.
3. Disagreement is preserved, never silently resolved.
4. What counts as accepted is decided by each reader, from shared evidence.

## Identifiers

| Name             | Size     | Definition                                         |
| ---------------- | -------- | -------------------------------------------------- |
| `GroundId`       | 32 bytes | Random, chosen when the Ground is created          |
| `AgentId`        | 32 bytes | Ed25519 public key                                 |
| `ContributionId` | 32 bytes | BLAKE3 hash of the canonical encoding of `content` |
| `Signature`      | 64 bytes | Ed25519 signature                                  |

Identifiers are byte strings in CBOR and lowercase hex in JSON.

## Contributions

A contribution is a CBOR map `{content, signature}`. `content` is:

| Field          | Type                    | Rule                                                 |
| -------------- | ----------------------- | ---------------------------------------------------- |
| `protocol`     | uint                    | `1`                                                  |
| `ground`       | `GroundId`              | The Ground it belongs to                             |
| `author`       | `AgentId`               | The signer                                           |
| `parents`      | array of ContributionId | Strictly ascending by bytes, no duplicates           |
| `timestamp_ms` | uint                    | Advisory only; never used for ordering or validity   |
| `body`         | map with one key        | One of the body kinds below                          |

The id is `BLAKE3(encode(content))`. The signature is Ed25519 over
`"manyroot/v1/contribution:" || id`, verified strictly (RFC 8032, with
small-order and non-canonical points rejected).

### Encoding

Fields are encoded as CBOR maps with text keys, in the order listed in this
document. Integers use the shortest form. Byte strings use major type 2. A
contribution has exactly one valid encoding: a replica that decodes a
contribution re-encodes it and rejects it if the bytes differ, which includes
any trailing data. The encoded contribution must not exceed 16 MiB.

> **Open:** v1 inherits field order from the reference implementation's
> serializer. A second implementation will need this pinned to RFC 8949 §4.2
> deterministic encoding, or to an explicit byte-level schema.

### Body kinds

| Kind       | Fields                                           | References         |
| ---------- | ------------------------------------------------ | ------------------ |
| `claim`    | `statement: text`, `tags: [text]`                | none               |
| `artifact` | `name: text`, `media_type: text`, `content: bytes` | none             |
| `relation` | `from`, `kind`, `to`, `note: text`               | `from`, `to`       |
| `evidence` | `target`, `verdict: "pass"/"fail"`, `method: text`, `details: text` | `target` |
| `retract`  | `target`, `reason: text`                         | `target`           |

Relation kinds: `supports`, `contradicts`, `depends_on`, `refines`,
`supersedes`, `implements`, `tests`.

Claims and artifacts are **subjects**. The other kinds are statements about
other contributions.

## Validity

A replica adds a contribution only if all of these hold:

1. `protocol` is 1, and `ground` is this replica's Ground.
2. The encoding is canonical and within the size limit.
3. The signature verifies for `author`.
4. `parents` are strictly ascending, and every parent is already present.
5. Every contribution the body references is listed in `parents`.
6. A relation's `from` and `to` differ.
7. A retract's target has the same `author`.
8. A claim's `statement`, an artifact's `name` and an evidence's `method` are
   not blank. Blank means empty, or made only of the 25 characters with the
   `White_Space` property in Unicode 17.0: U+0009 to U+000D, U+0020, U+0085,
   U+00A0, U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F and
   U+3000. Nothing else is blank, including characters that other languages'
   trim functions remove: U+FEFF, U+200B, U+180E (White_Space before Unicode
   6.3) and U+001C to U+001F.

Every rule depends only on the contribution and its parents. Any two replicas
that hold the parents therefore reach the same verdict, so validity converges
along with the graph.

## Replication

A Ground's state is the set of valid contributions a replica holds. Merging
is set union. Replicas send each other the contributions the other lacks, and
the receiver adds each one once its parents are present. Arrival order does
not matter.

New contributions should list all current causal heads (contributions with
no children) as parents, which merges concurrent lines of work.

Bundles carry contributions over any transport:

```
"MRBUNDL1"  GroundId  ( u32 big-endian length  ||  encoded contribution )*
```

## Sync

A sync session exchanges exactly the contributions each replica lacks.
Because a replica only holds a contribution once it holds its parents, its
set is closed under ancestry, and its causal heads describe it.

**Messages.** Each message is one CBOR map, encoded like a contribution:
canonical, text keys in the order listed, `type` first. A receiver decodes a
message, re-encodes it and rejects it if the bytes differ. Ids are 32-byte
strings. How messages are framed on a connection is the transport's concern.

| `type`   | Fields, in order                                                                  |
| -------- | --------------------------------------------------------------------------------- |
| `hello`  | `protocol: 1`, `ground: GroundId`, `limits: {batch_bytes: uint}`, `features: [text]` |
| `heads`  | `heads: [ContributionId]`, the sender's causal heads, sorted by id                |
| `offer`  | `ids: [ContributionId]`, what the sender holds and the receiver may lack, in causal order |
| `need`   | `ids: [ContributionId]`, the offered ids the sender lacks                         |
| `batch`  | `bundle: bytes`, a bundle (above) for this Ground; `last: bool`                   |
| `result` | `added: uint`, `already_present: uint`, `rejected: [{id, reason: text}]`, `unresolved: uint` |

**Id lists.** `heads`, `offer` and `need` each send one list, split across
messages of that type: every message holds at most 65 536 ids, every one but
the last holds exactly 65 536, and the last holds fewer, possibly none, which
ends the list. So a list of up to 65 535 ids is one message, and a list of
exactly 65 536 is a full message followed by an empty one. A replica decodes
any message up to its `batch_bytes` or 65 536 × 34 bytes (a full id list),
whichever is larger, plus 64 bytes. A full id list therefore always fits,
however small the peer's `batch_bytes` is.

**Session.** Both sides run the same steps:

1. Send `hello`, then the `heads` list. On the peer's `hello`, end the
   session if its `protocol` isn't 1, its `ground` isn't this replica's, or
   its `batch_bytes` is under 44 (an empty bundle plus one length prefix),
   before reading anything else. Unknown `features` are ignored.
2. On the peer's `heads`, let *known* be the peer's heads this side holds,
   and their ancestors. The candidates are this side's contributions outside
   *known*, in causal order.
   - If this side holds every one of the peer's heads, the candidates are
     exactly what the peer lacks: send them as batches.
   - Otherwise send the `offer` list of candidates. Once the whole list has
     arrived, the peer answers with the `need` list of the ones it lacks, and
     this side sends those as batches, in the order needed.
3. Batches go one at a time: send the next only after the peer's `result` for
   the last. Each bundle fits the peer's `batch_bytes`; a contribution too
   large for it ends the session. A sender may also keep its batches within
   its own `batch_bytes`, as `manyroot` does (the smaller of the two limits),
   so a peer can't make it build a batch larger than it would accept; a
   contribution too large for that ends the session too. The receiver checks
   only its own limit either way. The final batch has `last: true` and may be
   empty, so every side sends at least one batch. Only the final batch may be
   empty.
4. On a batch, import its contributions and answer `result` with the import
   report. `reason` is advisory text. `rejected` lists at most the first 1 024
   rejected contributions, each `reason` cut to at most 256 bytes (at a
   character boundary), so a `result` always fits the peer's message limit,
   whatever either side's `batch_bytes`.
5. The session ends when both sides have sent their last batch and received
   its `result`.

A side ends the session at once if the peer breaks these rules: a message out
of order, a message over the size limit above or a `heads`, `offer` or `need`
with more than 65 536 ids, a bundle over its own `batch_bytes` (checked before
decoding) or for another Ground, an empty batch without `last`, an `offer` or
`need` list that holds an id twice, a `need` for an id it didn't `offer`, a
`last` batch while ids this side listed in its `need` haven't all arrived, or
over-sending. A contribution is over-sent if the receiver neither asked for it
with `need` nor could have been sent it under the subset shortcut, or if it
was sent before: after a `need`, anything not listed or already received;
after the shortcut, anything the receiver held when it sent its `heads`, or
already received this session. (It may hold more by the time the batch
arrives, from elsewhere; those count as `already_present`.) The exchange
never sends a contribution twice, or one the receiver holds; it costs at most
one extra round trip and 34 bytes per candidate.

A peer can still keep a session open by sending new contributions under the
shortcut, by sending a `heads` or `offer` list that never ends (full messages
forever), or by not answering. Id lists have no total bound, and the receiver
keeps every id it's sent, so memory grows with them too. Bounding a session's
time, bytes and memory is the transport's concern, not the protocol's. The
`bracken` transports (network-sync.md, "As built in step 2") end a session at
a wall-clock deadline, and once the peer has sent more than a byte budget
(counted per frame before the frame is read), of which id lists may take only
a smaller share. Which contributions a replica stores (an allowlist, quotas)
is local policy too: a refused contribution is listed in `rejected`, like one
that breaks a validity rule.

## Derived state

These are computed from the graph and are never replicated.

**Causal order.** A topological order that breaks ties by ascending id.
Every replica holding the same set produces the same order.

**Retraction.** A contribution is retracted if it is the target of a retract
that is not itself retracted. Retracted contributions stay in the graph.

**Supersession.** `S` supersedes `T` if there is a non-retracted
`relation { from: S, kind: supersedes, to: T }` whose author is also `T`'s
author. Only a contribution's own author can retire it, but the replacement
`S` may be anyone's: an author can accept someone else's contribution in
place of their own. A `supersedes` relation by anyone else stays in the
graph as a visible statement, and counts for nothing. A supersession only
holds while its replacement is live: if `S` is retracted, or in a policy's
accepted layer isn't accepted (a counted failure, say), `T` is current again,
with everything about it. So accepting someone else's replacement lets its
author, or a validator, revive `T` without any action by `T`'s author. (Before 2026-10-27 the
rule required the relation's author to have written `S` instead; see
[releases.md](releases.md).)

## Acceptance

A policy has `min_passes` (default 1) and an optional set of `validators`.
For a subject `X`, evidence *counts* when it targets `X`, is not retracted,
was not written by `X`'s author, and its author is in `validators` (if
`validators` is set). Then `X` is:

1. `retracted` if `X` is retracted; otherwise
2. `rejected` if any counted evidence is a `fail`; otherwise
3. `accepted` if counted `pass` evidence comes from at least `min_passes`
   distinct authors; otherwise
4. `pending`.

An accepted subject is reported as `superseded` if an accepted subject
supersedes it.

## Conflicts

Conflicts are computed over a set of *live* subjects. For open conflicts,
live means not retracted. For a policy's accepted layer, live means accepted.
A live subject is *current* if no live subject supersedes it.

- **Divergence:** a contribution has two or more current supersessors.
- **Contradiction:** a non-retracted `contradicts` relation between two
  current subjects.

Conflicts are resolved by contributing, never by deleting. For example, a new
subject can supersede all the competing ones, or the authors can retract
their positions.

## Protocol guard

`crates/manyroot/tests/protocol_guard.rs` pins this version to bytes, so an
accidental change fails a test instead of making existing Grounds unreadable.
It pins:

- **Golden vectors.** One contribution of every body kind, built from fixed
  keys, timestamps and bodies, with its encoding, id and signature hard-coded
  as hex. This pins the CBOR field names and order, BLAKE3 ids, Ed25519 key
  derivation and the signing context. The pinned bytes must still decode,
  verify and pass the validity rules.
- **Negative vectors.** Inputs a replica must reject, each with its specific
  error: non-canonical encodings (a non-shortest integer, map keys out of
  order, an indefinite-length map, trailing bytes), protocol versions 0 and 2,
  a foreign Ground, a small-order key with a signature that plain RFC 8032
  verification accepts but strict verification rejects, parents out of order
  or duplicated, a missing parent among several, a body reference missing from
  `parents` (an evidence's or a retract's target, including a target the
  replica doesn't hold, and either end of a relation), another author's
  retraction of each kind of contribution, and a blank claim statement,
  artifact name or evidence method, made of each White_Space character in
  turn. On the other side, a field made of U+FEFF, U+200B, U+180E or U+001C to
  U+001F is accepted, and a test checks that the toolchain's White_Space set
  is exactly the 25 that rule 8 lists, so a Unicode upgrade that changes it
  fails. The size limit is pinned at its boundary: exactly 16 MiB passes, one
  byte more fails both decoding and the validity check.
- **Causal order vector.** The pinned order of six contributions, some of
  them concurrent, which pins the tie-break: the smallest id whose parents
  are all listed comes next.
- **Bundle vectors.** A two-contribution bundle's bytes: the magic, the Ground
  id and the u32 big-endian length framing. It must read back, and a reader
  must refuse another magic. The entry size limit is pinned at its boundary:
  an entry of exactly 16 MiB reads, and a length prefix one byte over is
  refused. A bundle cut inside a length prefix or inside an entry is refused,
  not read as shorter.
- **Sync vectors.** One message of every sync type, as pinned bytes that must
  decode back to the same message, then the id-list size (65 536), an empty
  `offer`'s bytes, a full id list fitting the smallest replica's message
  limit, and the `result` bounds (1 024 rejected, 256-byte reasons) with the
  largest `result` fitting that limit too, and the 44-byte minimum
  `batch_bytes` in `hello`. New vectors are appended; pinned ones never change.
- **Fixture Ground.** `crates/manyroot/tests/fixtures/ground-v1/` is a replica
  written by version 0.1.0 of `manyroot` (`ground.json`, `objects/`,
  `agents/`; no private keys). The test opens it, verifies every contribution,
  and checks pinned ids, agent names, the exact list of open conflicts, and
  every validation status, including that retraction outranks a failure and a
  failure outranks a pass. Its statuses also pin which evidence and
  retractions count: an author's pass on their own claim doesn't, a retracted
  pass doesn't, retracting a retraction restores the original, and a retracted
  `supersedes` or `contradicts` relation stops counting. Only an accepted
  subject is reported superseded, and only by an accepted one: a rejected or
  pending claim with an accepted successor keeps its status, and an accepted
  claim with a pending successor stays accepted. Open conflicts are only
  between current subjects that aren't retracted: the fixture's contradictions
  between two pieces of evidence, against a superseded claim and from a
  retracted claim are not reported. Cases appended for the supersession rule
  pin that another author's supersession neither supersedes an accepted claim
  nor silences a contradiction, and that an author's acceptance of someone
  else's replacement does. The fixture holds a contribution of every body
  kind, relation kind and verdict, and a test that fails to compile when a
  variant is added keeps it that way.

The guard does not pin these rules; `crates/manyroot/tests/protocol.rs`
tests them:

- **Validity:** a relation's `from` and `to` differ (rule 6).
- **Acceptance:** `min_passes` above 1, distinct authors, and the
  `validators` set. The fixture uses only the default policy.
- **Conflicts:** divergence, and the accepted layer's conflicts. The fixture
  has one open contradiction and no divergence.
- **Replication:** merging two replicas by set union. Opening the fixture
  does pin that a contribution is added once its parents are present.

These lists say what each test was written to pin, not that a rule can't be
loosened in a way no test notices. Gaps are found by mutation: changing one
rule at a time in `crates/manyroot/src` and checking that `cargo test
--workspace` fails. That sweep is a standing task (`7c6e5b8b4326` in this
repository's Ground), and every surviving mutant it finds is either pinned
by a new test or listed here. One check has no mutant that changes
behaviour: evidence counts only for its own target, but the reader only
looks at a subject's referrers, and evidence refers to nothing but its
target.

Changing a pinned value or the fixture is a protocol change: it needs
independent review, the manager's go (logged as `owner-review`) and a migration note in
[releases.md](releases.md). A new protocol version should get a new fixture
next to the old one, which stays to show what still opens. To rebuild a
fixture deliberately, delete it and run its ignored generator:

```sh
rm -r crates/manyroot/tests/fixtures/ground-v1
cargo test -p manyroot --test protocol_guard -- --ignored write_ground_v1_fixture
```

The generator is deterministic: on an unchanged protocol it rewrites the
fixture byte for byte.

## Not yet specified

- Ground membership, permissions and governance (Stewards).
- References across Grounds and selective replication (Canopy).
- Checkpoints and releases (Rings).
- Transports for sync sessions, and who may push (Stewards).
- Identities with readable names that other replicas can see.
