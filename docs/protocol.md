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
   not blank.

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
  order, an indefinite-length map, trailing bytes), protocol version 2, a
  foreign Ground, a small-order key with a signature that plain RFC 8032
  verification accepts but strict verification rejects, parents out of order
  or duplicated, and a body reference missing from `parents`. The size limit
  is pinned at its boundary: exactly 16 MiB passes, one byte more fails both
  decoding and the validity check.
- **Bundle vector.** A two-contribution bundle's bytes: the magic, the Ground
  id and the u32 big-endian length framing. It must read back.
- **Fixture Ground.** `crates/manyroot/tests/fixtures/ground-v1/` is a replica
  written by version 0.1.0 of `manyroot` (`ground.json`, `objects/`,
  `agents/`; no private keys). The test opens it, verifies every
  contribution, and checks pinned ids, agent names, the open contradiction,
  and every validation status, including that retraction outranks a failure
  and a failure outranks a pass. Cases appended for the supersession rule
  pin that another author's supersession neither supersedes an accepted
  claim nor silences a contradiction, and that an author's acceptance of
  someone else's replacement does. The fixture holds a contribution of every
  body kind, relation kind and verdict, and a test that fails to compile when
  a variant is added keeps it that way.

The guard has no vectors for the remaining validity rules (missing parents,
self-relations, retraction by another author, blank fields) or for
supersession and conflicts beyond the fixture's cases. Those are tested in
`crates/manyroot/tests/protocol.rs`, except blank artifact names and evidence
methods, which nothing tests yet.

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
- A network sync protocol beyond bundles.
- Identities with readable names that other replicas can see.
