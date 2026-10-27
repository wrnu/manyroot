# Bracken

**Software that grows together.**

Bracken is an agent-native collaborative development environment. It is the
first application built on **Manyroot**, a protocol for a decentralized network
where knowledge accumulates through independent contributions from humans and
AI agents.

> Manyroot — many minds, shared ground.

Why this exists and where it's going: [docs/vision.md](docs/vision.md).

## The idea

Git reconciles *versions of files*. Bracken records *contributions*: claims,
artifacts, the relations between them, and evidence about them. Each
contribution is immutable, signed by the agent that made it, and
content-addressed. Each one names the contributions it causally depends on.

- **Contributions are conflict-free to record.** A Ground's contributions
  form a grow-only causal graph. Merging two replicas is set union, so any
  number of agents can contribute concurrently with no branches, locks or
  central coordinator.
- **But nothing is automatically authoritative.** When two agents replace the
  same decision in incompatible ways, both replacements are kept and reported
  as a conflict. Nothing silently picks a winner.
- **Acceptance is a policy, not shared state.** Every replica holds the same
  evidence. Each reader applies its own validation policy, for example
  "two independent validators passed it", to decide what is accepted. Bracken
  synchronizes evidence. It does not enforce consensus.

## Quick start

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"

bracken init --name auth
bracken id new alice && bracken id new val
export BRACKEN_AGENT=alice           # or pass --as on each command

REQ=$(bracken claim "Sessions need bounded token lifetimes" --tag auth)
IMPL=$(bracken artifact src/session.rs --media-type text/x-rust)
bracken relate $IMPL implements $REQ
bracken evidence $IMPL pass --method "cargo test" --as val

bracken log                          # everything, in causal order
bracken accepted                     # what the default policy accepts
bracken accepted --min-passes 2 --validator val
bracken conflicts                    # divergences and contradictions
bracken show $REQ --json             # every read command supports --json
```

Moving contributions between replicas:

```sh
bracken sync ../other-checkout       # exchange with another replica on disk
bracken export ground.mrb            # or carry a bundle over any transport
bracken join ground.mrb              # create a new replica from a bundle
bracken import ground.mrb            # add a bundle to an existing replica
```

## Concepts

| Term         | Meaning                                                                                   |
| ------------ | ----------------------------------------------------------------------------------------- |
| Ground       | A bounded workspace with its own contribution graph. Identified by a random 32-byte id.   |
| Contribution | An immutable, signed, typed unit: `claim`, `artifact`, `relation`, `evidence`, `retract`. |
| Causal heads | Contributions nothing builds on yet. New contributions list all of them as parents.       |
| Relation     | `supports`, `contradicts`, `depends_on`, `refines`, `supersedes`, `implements`, `tests`.  |
| Evidence     | A `pass` or `fail` from checking a contribution, with the method used.                    |
| Policy       | Local rules for turning evidence into acceptance: minimum passes, trusted validators.     |
| Conflict     | A *divergence* (competing replacements) or a *contradiction* between current work.        |

The protocol is specified in [docs/protocol.md](docs/protocol.md). The long-term vision, principles
and roadmap are in [docs/vision.md](docs/vision.md).

## Layout

| Crate                               | What it is                                                               |
| ----------------------------------- | ------------------------------------------------------------------------ |
| [`manyroot`](crates/manyroot)       | The protocol library: contributions, causal graph, validation, storage.  |
| [`bracken`](crates/bracken)         | The `bracken` CLI, for agents and humans.                                |

Manyroot subsystems map onto modules as they come into being:

| Subsystem | Responsibility                          | Status                                            |
| --------- | --------------------------------------- | ------------------------------------------------- |
| Seed      | Contribution primitive                  | `manyroot::contribution`                          |
| Roots     | Causal history                          | `manyroot::graph`                                 |
| Proof     | Validation and acceptance               | `manyroot::validation`                            |
| Ground    | Shared workspace                        | `manyroot::store` (local filesystem replica)      |
| Weave     | Replication and merged state            | `manyroot::store::sync`, `manyroot::bundle`       |
| Stewards  | Identity, permissions, governance       | Keys and signatures only (`manyroot::identity`)   |
| Trails    | Provenance queries                      | Relations and referrers in the graph; no queries yet |
| Rings     | Checkpoints and releases                | Not started                                       |
| Signal    | Discovery                               | Not started                                       |
| Canopy    | Federation across Grounds               | Not started                                       |

## Status

v0. The core graph, the validation model, local replicas and the CLI work and
are tested. These are not done yet:

- **Network sync.** Replicas exchange contributions through shared
  filesystems or bundle files.
- **Key protection.** Identity keys are stored unencrypted under
  `.bracken/identities/`.
- **Ground governance.** Any key can contribute to any Ground it can reach.
- **Scale.** Each replica loads its whole graph into memory on open.

Next up is the Phase 1 experiment: comparing Git branches and PRs, shared
CRDT text editing, and Bracken's semantic contributions with 10 to 100 agents
on the same tasks and budgets.
