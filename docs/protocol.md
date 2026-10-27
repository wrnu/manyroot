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
`relation { from: S, kind: supersedes, to: T }` whose author is also `S`'s
author. Agents can replace their own contributions or anyone else's, but
only by contributing something new themselves.

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

## Not yet specified

- Ground membership, permissions and governance (Stewards).
- References across Grounds and selective replication (Canopy).
- Checkpoints and releases (Rings).
- A network sync protocol beyond bundles.
- Identities with readable names that other replicas can see.
