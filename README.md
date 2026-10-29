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
# The artifact and its implements relation in one call; it prints both ids, the artifact's first.
IMPL=$(bracken artifact src/session.rs --media-type text/x-rust --implements $REQ | head -1)
bracken evidence $IMPL pass --method "cargo test" --as val

bracken log                          # everything, in causal order
bracken log --since ""               # the same, then a cursor: pass it next time for only what is new
bracken log --compact                # one line each (at most 200 characters), shortest unique ids (8+ digits)
bracken log --since "$C" --limit 50  # only the most recent 50 of what is new; the cursor still moves past all
```

With `--since` or `--limit`, `log --json` prints an object, `{"contributions": [...], "cursor": "...",
"omitted": N}`, instead of the bare array. A cursor means the same only under the same filters: new work
that `--tag`, `--kind`, `--author` or `--current` hid is past it all the same, so keep one cursor per set of
filters. Every read command prints control characters from contributions (CR, ESC, newlines and the
like) as escapes such as `\u{1b}`, so no contribution can forge or hide a line; `show` keeps a statement's
or evidence's line breaks, with each of its lines starting `| `. `--json` escapes them as JSON does.

```sh
bracken accepted                     # what the default policy accepts
bracken accepted --min-passes 2 --validator val
bracken status                       # per claim: what implements it, accepted or still waiting
bracken status --all                 # plus the claims nothing implements yet
bracken intend $REQ --for 30m        # "I'm on this": shown in status and show until it ends
bracken conflicts                    # divergences and contradictions
bracken show $REQ --json             # every read command supports --json
```

`bracken view [--port N]` serves a read-only web view of the replica on 127.0.0.1 only, with its JSON API
under `/api/` (the shapes are in [docs/design/visualizer.md](docs/design/visualizer.md)) and new work
announced on `/api/events`. It never writes, reads no identity, and sends Ground text only as JSON, under
a Content-Security-Policy that allows no inline script.

Moving contributions between replicas:

```sh
bracken sync ../other-checkout       # exchange with another replica on disk
bracken serve --tokens T --allow A --first-seen L   # serve this replica on 127.0.0.1
BRACKEN_TOKEN=... bracken sync http://127.0.0.1:<port>
bracken sync --exec ssh host bracken --dir <path> serve --stdio
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

**Intents** are a convention on ordinary contributions, not part of the protocol. An intent is a
claim tagged `intent` and `until:<RFC 3339 time>`, related to the contribution it's about by a
`refines` relation from the same author; `bracken intend <id> [--for 1h]` writes both. `status`
and `show` list an intent as live until it expires, its author retracts it, or its author relates
something that `implements` the contribution. Retracting that relation makes the intent live again.
A claim tagged `intent` without an expiry, or without a `refines` relation from its author, is an
ordinary claim. Like every timestamp, the expiry is advisory: it's the author's clock, so readers
treat it as a hint, not a lock.

The protocol is specified in [docs/protocol.md](docs/protocol.md). The long-term vision, principles
and roadmap are in [docs/vision.md](docs/vision.md).

## Layout

| Crate                               | What it is                                                               |
| ----------------------------------- | ------------------------------------------------------------------------ |
| [`manyroot`](crates/manyroot)       | The protocol library: contributions, causal graph, validation, storage.  |
| [`bracken`](crates/bracken)         | The `bracken` CLI, for agents and humans.                                |
| [`bracken-demo`](crates/demo)       | Agents on a toy task through `bracken`, with a measured report ([docs/demo.md](docs/demo.md)). |
| [`bracken-experiment`](crates/experiment) | The Phase 1 experiment; so far its task (`stacklib`), the hidden grader, the driver with its workers and shim, Bracken mode, Git mode and a mock backend ([design](docs/design/phase1-experiment.md)). |

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

- **Network sync.** `bracken serve` and `bracken sync <url>` work on one
  machine (HTTP on 127.0.0.1, no TLS) and over a program's stdio (SSH). No
  team server runs yet (docs/design/network-sync.md, step 3).
- **Key protection.** Identity keys are stored unencrypted under
  `.bracken/identities/`. Keep that folder out of git. The rest of
  `.bracken/` (contributions, `ground.json`, the public `agents/` directory)
  is safe to commit, and git merges it without conflicts.
- **Ground governance.** Any key can contribute to any Ground it can reach.
- **Scale.** Each replica loads its whole graph into memory on open.

Next up is the Phase 1 experiment: comparing Git branches and PRs, shared
CRDT text editing, and Bracken's semantic contributions with 10 to 100 agents
on the same tasks and budgets.

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Contributions are accepted under the same terms.
