# Manyroot

**Many minds. Shared ground.**

Manyroot is a protocol for a shared record of work that humans and AI agents
build together. Each piece of work is a *contribution*: a claim, an artifact,
a relation between two contributions, or evidence about one. A contribution
is signed by whoever made it, can't be changed, and names the contributions
it builds on. Replicas merge by set union, so any number of agents can
contribute at once, with no locks and no central server. Nothing is
accepted just because it was written: each reader decides what counts from
the evidence, under its own policy.

**Bracken** is the first application: software development by teams of
agents. Today it is a command-line tool, `bracken`. It keeps a *Ground*, a
shared workspace of contributions, on disk. It syncs Grounds between
replicas, and answers questions like "what is accepted?", "what is waiting
for review?" and "what's new since I last looked?".

## Why not Git

Git works well for what it was designed for: a modest number of people who
now and then agree on which version of each file is authoritative. Its unit
is a version of a file, and its workflow is branches, merges and pull
requests.

Agent teams strain that model. There can be far more agents than human
developers, and they work continuously. Much of what they learn, such as a
hypothesis, a dead end or a test result, has no place in a commit. Manyroot
records each of these as a typed contribution. It keeps conflicting
contributions side by side instead of forcing a merge, and lets evidence
decide what counts. Git still has a place at the boundary, making releases
for people. The longer argument is in [docs/vision.md](docs/vision.md).

## What we've measured

So far, every run has used one toy task, `stacklib`: a library of 30 small
functions, with six change requests released together partway through,
graded by hidden tests.
Each run compares three ways of working: Bracken, Git branches and pull
requests, and one shared text document edited as a CRDT.

**Phase 1: 10 agents, 8 turns each, Claude Opus 5.5, three runs per mode**
([results](docs/design/phase1-results.md)):

- Bracken ended with all 30 functions correct in every run, as CRDT did. Git
  ended with 16, 16 and 30.
- Bracken cost the most: 14.7 million tokens per run on average, against
  CRDT's 5.1 million and Git's 3.8 million. So it made the fewest
  improvements per million tokens: 2.69, against CRDT's 8.64 and Git's 9.11.
- Only CRDT broke the library during a run, 32 to 47 times per run, and it
  always repaired it. Only Git had failed integrations, 5 to 11 per run.
- Bracken and CRDT spent 52 to 66% of their output on duplicated work, and
  Git 27 to 29%.

**Two agents, DeepSeek V4.1 Flash through the Pi harness, three runs per
mode at each budget** (outcomes `6c9c86574d9d` and `342162a87040`, below):

- With 4 turns each, Bracken ended 30 of 30 in every run. Git ended 16, 0 and
  16: in the 0, the two agents opened rival pull requests and nothing merged.
  CRDT ended 30, 30 and 15: in the 15, concurrent edits merged mid-token at
  the very end.
- With 6 turns each, all three modes ended 30 of 30 in every run. Bracken
  used the most tokens, a median of 3.56 million against Git's 1.50 million
  and CRDT's 1.32 million. Only CRDT had regressions, 30 to 47 per run.

**What this doesn't show.** It's one toy task and one prompt, with at most 10
agents and three runs per setting. Three runs can't establish significance:
the smallest possible two-sided p-value for three runs against three is 0.1.
These are consistent directions, not proven effects. They don't show that
Bracken beats Git on real software. They do show that Bracken costs more
tokens today, and that some differences, such as which mode duplicates most,
change with the budget.

## Built by its own agent team

This repository is developed by AI agents with one human owner: a manager,
builders, and independent reviewers. They coordinate through a Bracken
Ground kept in the repository, in `.bracken/`. Tasks, decisions, review
findings, test runs and reports are all contributions there. Ids like
`342162a87040` in this README and the docs point into it: in a clone,
`bracken show 342162a87040` prints one. Every change gets an independent
review, and nothing merges until the manager's counted test run passes on
the exact commit. The process is in
[process.md](.claude/skills/agent-team-kickoff/process.md).

The source tree is moving onto Manyroot as well. Weave
([spec](docs/design/weave.md)) stores a tree as content-addressed files and
manifests. It has lines, like branches, whose moves are signed decisions
that must meet the line's policy. In its first stage, the shadow, since
2026-10-31, each merge to `main` has also been recorded in a Weave Ground
and verified identical to git's tree. Git is still the repository of record.

## Where it's going

These are plans, not features:

- **Weave holds the tree.** In stage 2, builders propose changes in Weave,
  an integrator merges them, and a validator runs the checks once per
  candidate. Git keeps only what Weave hasn't yet proven it can do
  ([weave.md](docs/design/weave.md), sections 6 and 7).
- **An evaluation suite.** Real Python projects, each with a spec, hidden
  tests and a golden solution, to run Bracken and Weave as a whole
  development system again and again, and see what each way of working
  changes ([eval-suite.md](docs/design/eval-suite.md)).
- **The network.** Grounds that sync across machines, then across
  organizations with identities and permissions, and in the long run a
  public network of connected Grounds ([vision.md](docs/vision.md),
  Roadmap).

## Quick start

```sh
cargo build --release && export PATH="$PWD/target/release:$PATH"
cd "$(mktemp -d)" && bracken init --name demo   # a Ground in ./.bracken
bracken id new alice && bracken id new val      # two signing identities
printf 'pub fn ttl() -> u64 { 3600 }\n' > session.rs
REQ=$(bracken claim "Sessions need a bounded lifetime" --as alice --json | jq -r .id)
IMPL=$(bracken artifact session.rs --media-type text/x-rust --implements $REQ --as alice --json | jq -r .id)
bracken evidence $IMPL --run --as val -- grep -q ttl session.rs   # pass if it exits 0
bracken accepted                                # what the default policy accepts
bracken status                                  # the review queue
bracken log --compact                           # everything, one line each
```

Every command, with its options and output: [docs/cli.md](docs/cli.md).

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

The protocol is specified in [docs/protocol.md](docs/protocol.md).

## Layout

| Crate                                     | What it is                                                               |
| ----------------------------------------- | ------------------------------------------------------------------------ |
| [`manyroot`](crates/manyroot)             | The protocol library: contributions, the causal graph, validation, storage and sync. It assumes nothing about code. |
| [`bracken`](crates/bracken)               | The `bracken` CLI, for agents and humans, including `bracken weave`.     |
| [`bracken-tree`](crates/tree)             | Source trees on Weave: payload formats, import from git, lines and decisions, proposals and merges, checks run in a sandbox ([spec](docs/design/weave.md)). |
| [`bracken-demo`](crates/demo)             | Agents on a toy task through `bracken`, with a measured report ([docs/demo.md](docs/demo.md)). |
| [`bracken-experiment`](crates/experiment) | The experiment runner: one task under Bracken, Git and CRDT modes, measured the same way, with Claude Code and Pi as harnesses ([design](docs/design/phase1-experiment.md)). |

Manyroot's subsystems map onto modules as they come into being:

| Subsystem | Responsibility                          | Status                                            |
| --------- | --------------------------------------- | ------------------------------------------------- |
| Seed      | Contribution primitive                  | `manyroot::contribution`                          |
| Roots     | Causal history                          | `manyroot::graph`                                 |
| Proof     | Validation and acceptance               | `manyroot::validation`                            |
| Ground    | Shared workspace                        | `manyroot::store` (local filesystem replica)      |
| Weave     | Replication, merged state and materialized views (first: Bracken's source trees) | `manyroot::sync`, `manyroot::bundle`; source trees in `bracken-tree` |
| Stewards  | Identity, permissions, governance       | Keys and signatures only (`manyroot::identity`)   |
| Trails    | Provenance queries                      | Relations and referrers in the graph; no queries yet |
| Rings     | Checkpoints and releases                | Not started                                       |
| Signal    | Discovery                               | Not started                                       |
| Canopy    | Federation across Grounds               | Not started                                       |

## Status

v0, and not published. What works today, tested:

- The protocol library, local replicas and the `bracken` CLI.
- Sync between replicas on disk, through bundles, over a program's stdio
  (such as SSH), and over HTTP on 127.0.0.1, with no TLS. This repository's
  team syncs through a server on the owner's machine, with no off-host
  backup yet ([network-sync.md](docs/design/network-sync.md),
  step 3).
- Weave, through step 9 of its build plan: the record formats, import from
  git, lines and decisions, proposals and merges, and the sandboxed check
  runner. Since 2026-10-31, each merge to `main` has also been recorded in a
  Weave Ground and verified identical ([weave.md](docs/design/weave.md),
  section 6).

Not done yet:

- **Key protection.** Identity keys are stored unencrypted under
  `.bracken/identities/`. Keep that folder out of git. The rest of
  `.bracken/` (contributions, `ground.json`, the public `agents/` directory)
  is safe to commit, and git merges it without conflicts.
- **Governance.** Any key can contribute to any Ground it can reach, and keys
  are shared among this repository's agents, so who signed what is a
  convention, not a guarantee.
- **Scale.** Each replica loads its whole graph into memory when it opens.

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Contributions are accepted under the same terms.
