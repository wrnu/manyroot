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

**Weave** is how Bracken holds the code itself: a source tree stored as
contributions, with proposals, merges and checks recorded in the same Ground
as the discussion about them. This repository's own merges already go
through it, alongside git ([below](#weave-the-source-tree-as-contributions)).

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

## Weave: the source tree as contributions

Weave ([spec](docs/design/weave.md)) puts a source tree into a Ground without
changing the protocol: every record is an ordinary contribution with its own
media type, so any `bracken` binary can open and sync a Weave Ground.

- **Files and trees.** Each file version is an artifact, named by the BLAKE3
  hash of its bytes. A *manifest* lists a tree's paths, modes and hashes,
  like a git tree. Identical trees have one hash, whoever made them.
- **Lines and decisions.** A *line* is like a branch. It moves only by a
  *decision*, the equivalent of a commit: a signed record naming the tree,
  its parent decisions and what it integrates. The line's *policy* says who
  may integrate, who may validate, which checks count and what breakage is
  tolerated.
- **Proposals and merges.** Anyone can write a *proposal*: a tree to put on
  a line, based on one of its decisions. `bracken weave integrate` merges
  proposals into a new decision, file by file, and text files line by line.
  A conflict is recorded, and its *resolution* is a contribution too, reused
  whenever exactly the same conflict comes back. Reviews attach to a
  proposal's change, so they survive a rebase that leaves the change as it
  was.
- **Checks and green.** The line's validators run its named checks, in a
  sandbox with no network or, for checks that can't run there, on a remote
  Linux box or the Mac. Each result is evidence on the manifest it checked,
  bound to the check's exact command. A decision is *green* when it accepts
  no known breakage and every check in the line's current policy has a
  counted pass and no counted fail. Objections bind integrators until
  they're answered.
- **Everything is derived.** Heads, forks, merge bases and green are computed
  from the Ground in causal order, never from timestamps, so every replica
  that holds the same contributions gives the same answer.

Weave is being proven on this repository in stages
([weave.md](docs/design/weave.md), section 6). Git is still the repository
of record throughout:

1. **Shadow** (since 2026-10-31). After each merge to `main`, the merged tree
   is recorded as a decision on Weave's line `main` and verified identical
   to git's.
2. **Proposals** (stage 2a). Builders propose every pushed commit to Weave.
   After git merges a pull request, Weave merges its proposal itself, and
   that merge is kept only if it comes out exactly as git's tree. Otherwise
   the commit's tree is imported, with the reason noted.
3. **The Weave gate** (stage 2b, on since 2026-11-03). Weave runs the test
   table as checks on the tree git would merge. When those checks are green,
   they can stand in for the team's own test run as the merge gate.

The experiment harness also has a Weave mode, in which a run's agents build
an evaluation project through these same records (below).

## What we've measured

All runs compare ways for a team of agents to work on one task, measured the
same way, and are graded by tests the agents never see. The earlier runs compared three ways of working: Bracken, Git
branches and pull requests, and one shared text document edited as a CRDT.

**Phase 1: `stacklib`, 10 agents, 8 turns each, Claude Opus 5.5, three runs
per mode** ([results](docs/design/phase1-results.md)). `stacklib` is a toy
library of 30 small functions, with six change requests released together
partway through.

- Bracken ended with all 30 functions correct in every run, as CRDT did. Git
  ended with 16, 16 and 30.
- Bracken cost the most: 14.7 million tokens per run on average, against
  CRDT's 5.1 million and Git's 3.8 million. So it made the fewest
  improvements per million tokens: 2.69, against CRDT's 8.64 and Git's 9.11.
- Only CRDT broke the library during a run, 32 to 47 times per run, and it
  always repaired it. Only Git had failed integrations, 5 to 11 per run.
- Bracken and CRDT spent 52 to 66% of their output on duplicated work, and
  Git 27 to 29%.

**`stacklib`, two agents, DeepSeek V4.1 Flash through the Pi harness, three
runs per mode at each budget** (outcomes `6c9c86574d9d` and
`342162a87040`):

- With 4 turns each, Bracken ended 30 of 30 in every run. Git ended 16, 0 and
  16: in the 0, the two agents opened rival pull requests and nothing merged.
  CRDT ended 30, 30 and 15: in the 15, concurrent edits merged mid-token at
  the very end.
- With 6 turns each, all three modes ended 30 of 30 in every run. Bracken
  used the most tokens, a median of 3.56 million against Git's 1.50 million
  and CRDT's 1.32 million. Only CRDT had regressions, 30 to 47 per run.

**`ledger`, Weave mode, three agents, 6 turns each, DeepSeek V4.1 Flash
through Pi.** `stacklib` saturates, so the next task is a real project, the
first of an [evaluation suite](docs/design/eval-suite.md): `ledger`,
plain-text double-entry bookkeeping in Python, with 15 task items: features
such as budgets, CSV import and a register report, bug fixes, and a
requirement change released mid-run. Each run is graded at a checkpoint
after every round of turns. One run each:

- `w1t`, a shakedown on a small fixture project, showed that agents can
  work through Weave: 21 of the line's 29 decisions settled forks, where two
  agents had moved the line at once.
- `w2l` and `w3l` stopped at their 9 million token cap by the fourth round
  of turns, and the grader of the time couldn't grade `w2l`. `w3l` reached 87 of 101 tests, with 12 of its 15 groups
  passing whole, by its second checkpoint (note `466960485ef8`).
- `w4l`, with higher caps, ran all 18 agent-turns on 21.2 million tokens and
  ended exactly where `w3l` did, 87 of 101 and 12 of 15 groups whole: turns
  4 to 6 added nothing the tests could see (note `7d9a8aa5178d`).
- **That plateau is the harness's, not Weave's or the agents'.** No agent
  ever saw two of the three missing items, account names with spaces and
  postings with an implied amount, and the third, `check`, fails because of
  the second. Pi cuts the middle out of any tool output over 20 KiB, and the
  task list the agents read was 26.5 KB, so both items were cut from it
  (report `b7b35a307aea`). Every tool output now stays under that cap (task
  `b58cf9180030`, PR #254), but no `ledger` run has been made since, so the
  `w3l` and `w4l` scores on those items say nothing yet about Weave or the
  agents.

**What this doesn't show.** It's two tasks, one of them a toy, with at most
10 agents and three runs per setting, and single runs on `ledger`. Three runs
can't establish significance: the smallest possible two-sided p-value for
three runs against three is 0.1. These are consistent directions, not proven
effects. They don't show that Bracken beats Git on real software. They do
show that Bracken costs more tokens today, and that some differences, such
as which mode duplicates most, change with the budget.

## Built by its own agent team

This repository is developed by AI agents with one human owner: a manager,
builders, and independent reviewers, including a security reviewer. They
coordinate through a Bracken Ground kept in the repository, in `.bracken/`.
Tasks, decisions, review findings, test runs and reports are all
contributions there. Ids like `342162a87040` in this README and the docs
point into it: in a clone, `bracken show 342162a87040` prints one.

Every change gets an independent review, and nothing merges until a counted
test run passes on `main` merged with the exact commit under review, and no
review finding is left open. `scripts/merge` checks all of that before it merges.
Each merge also goes through Weave, as above, and with the Weave gate on,
Weave's own green checks on the merged tree can count as that test run.
GitHub Actions runs the test table on every push and pull request too, as
an observer that no merge waits on yet. The process is in [process.md](.claude/skills/agent-team-kickoff/process.md),
with this repository's values in [docs/development.md](docs/development.md).

## Where it's going

These are plans, not features:

- **Weave holds the tree.** Its checks gate merges, and git keeps only what
  Weave hasn't yet proven it can do, until Weave becomes the repository of
  record ([weave.md](docs/design/weave.md), sections 6 to 8).
- **The evaluation suite.** More real projects like `ledger`, each with a
  spec and a golden solution, to run Bracken and Weave as a whole
  development system again and again, and see what each way of working
  changes ([eval-suite.md](docs/design/eval-suite.md)).
- **The network.** Grounds that sync across machines, then across
  organizations with identities and permissions, and in the long run a
  public network of connected Grounds ([vision.md](docs/vision.md),
  Roadmap).

## Quick start

You need Rust (stable) and `jq`. Build `bracken` and record some work in a
throwaway Ground:

```sh
git clone https://github.com/wrnu/manyroot && cd manyroot
cargo build --release && export PATH="$PWD/target/release:$PATH"
export BRACKEN_DIR="$(mktemp -d)/g" && bracken init --name demo   # a throwaway Ground
bracken id new alice && bracken id new val      # two signing identities
cd "$(mktemp -d)" && printf 'pub fn ttl() -> u64 { 3600 }\n' > session.rs
REQ=$(bracken claim "Sessions need a bounded lifetime" --as alice --json | jq -r .id)
IMPL=$(bracken artifact session.rs --media-type text/x-rust --implements $REQ --as alice --json | jq -r .id)
bracken evidence $IMPL --run --as val -- grep -q ttl session.rs   # pass if it exits 0
bracken accepted                                # what the default policy accepts
bracken status                                  # the review queue
bracken log --compact                           # everything, one line each
```

Then Weave, in the same shell: two proposals made from the same tree, each
changing a different line of one file, merged onto line `main`:

```sh
W="$(mktemp -d)/weave"                          # a throwaway Weave Ground, outside any checkout
cd "$(mktemp -d)" && git init -q && printf 'a\nb\nc\n' > f.txt && git add f.txt && git commit -qm base
bracken weave init --dir "$W" --as integ        # line main, with integ as its integrator
bracken weave integrate --dir "$W" --as integ   # HEAD's tree: main's first decision
printf 'A\nb\nc\n' > f.txt && git commit -qam upper-a
P1=$(bracken weave propose --dir "$W" --as integ --json | jq -r .proposal)
git checkout -q HEAD~1 && printf 'a\nb\nC\n' > f.txt && git commit -qam upper-c
P2=$(bracken weave propose --dir "$W" --as integ --json | jq -r .proposal)
bracken weave integrate --dir "$W" --as integ --proposals $P1 $P2   # both, merged line by line
bracken weave log --dir "$W"                    # main's decisions, newest first
M=$(bracken weave head --dir "$W" --json | jq -r '.heads[0].manifest')
D=$(mktemp -d) && bracken weave checkout --dir "$W" --manifest $M "$D" && cat "$D/f.txt"   # A b C
```

Every `bracken` command, with its options and output, is in
[docs/cli.md](docs/cli.md), and `bracken weave`'s in
[weave.md](docs/design/weave.md), section 3. This repository's own Ground
comes with the clone: in a new shell, from the clone's root,
`bracken log --compact` reads it.

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
| Line         | Weave's branch: a sequence of decisions, each naming a tree, under the line's policy.     |

The protocol is specified in [docs/protocol.md](docs/protocol.md).

## Layout

| Crate                                     | What it is                                                               |
| ----------------------------------------- | ------------------------------------------------------------------------ |
| [`manyroot`](crates/manyroot)             | The protocol library: contributions, the causal graph, validation, storage and sync. It assumes nothing about code. |
| [`bracken`](crates/bracken)               | The `bracken` CLI, for agents and humans: recording and reading work, sync, the web view, `bracken watch` and `bracken weave`. |
| [`bracken-tree`](crates/tree)             | Bracken's code layer on Weave: payload formats, import from git, lines and decisions, proposals and merges, line-by-line text merge, checks run in a sandbox ([spec](docs/design/weave.md)). |
| [`bracken-experiment`](crates/experiment) | The experiment runner: one task under Bracken, Git, CRDT or Weave mode, measured and graded the same way, with Claude Code and Pi as harnesses ([design](docs/design/phase1-experiment.md)). Holds the `ledger` project. |
| [`bracken-demo`](crates/demo)             | Agents on a toy task through `bracken`, with a measured report ([docs/demo.md](docs/demo.md)). |
| [`bracken-scope`](crates/scope)           | Which packages a change affects, and which Rust files it touched only in comments, for the team's test scoping. |
| [`bracken-query-lab`](crates/query-lab)   | A research prototype: a Datalog query language over a Ground. Nothing depends on it. |

| Directory | What it holds |
| --- | --- |
| [`docs/`](docs/README.md) | The vision, the protocol, the CLI reference, the team's process values, and design notes and results under `docs/design/` |
| [`scripts/`](scripts) | The team's tools: `bk` (the pinned `bracken` on the team's Ground), `bk-evidence` (record a check's real result), `merge` and `verify-seq` (the merge gate and counted runs), `test-table`, `ci-result` (record a head's Actions result), `code-scope`, `weave`, `weave-propose`, `weave-shadow` and `weave-gate` (the Weave dogfood), `owner-approve`, and housekeeping |
| [`.bracken/`](.bracken) | This repository's own Ground: every task, decision, finding, test run and report |
| [`.claude/skills/agent-team-kickoff`](.claude/skills/agent-team-kickoff) | The agent-team process, as a skill that sets it up in a repository |

Manyroot's subsystems map onto modules as they come into being:

| Subsystem | Responsibility                          | Status                                            |
| --------- | --------------------------------------- | ------------------------------------------------- |
| Seed      | Contribution primitive                  | `manyroot::contribution`                          |
| Roots     | Causal history                          | `manyroot::graph`                                 |
| Proof     | Validation and acceptance               | `manyroot::validation`; green trees in `bracken-tree` |
| Ground    | Shared workspace                        | `manyroot::store` (local filesystem replica)      |
| Weave     | Replication, merged state and materialized views (first: Bracken's source trees) | `manyroot::sync`, `manyroot::bundle`; source trees in `bracken-tree` |
| Stewards  | Identity, permissions, governance       | Keys and signatures only (`manyroot::identity`)   |
| Trails    | Provenance queries                      | Relations and referrers in the graph; a query language in research (`bracken-query-lab`) |
| Rings     | Checkpoints and releases                | Not started                                       |
| Signal    | Discovery                               | Not started                                       |
| Canopy    | Federation across Grounds               | Not started                                       |

## Status

v0, and not published. What works today, tested:

- The protocol library, local replicas and the `bracken` CLI.
- Sync between replicas on disk, through bundles, over a program's stdio
  (such as SSH), and over HTTP on 127.0.0.1, with no TLS. This repository's
  team syncs through a server on the owner's machine
  ([network-sync.md](docs/design/network-sync.md)).
- Weave, into stage 2b of its build plan: the record formats, import from
  git, lines and decisions, proposals and merges with line-by-line text
  merge, the sandboxed check runner, and green. Every merge to `main` is
  recorded in a Weave Ground and verified identical to git's tree, and the
  Weave gate is on ([weave.md](docs/design/weave.md), section 6).
- The experiment runner, in four modes, with a grader that runs agents' code
  in a pinned container.

Not done yet:

- **Key protection.** Identity keys are stored unencrypted under
  `.bracken/identities/`. Keep that folder out of git. The rest of
  `.bracken/` (contributions, `ground.json`, the public `agents/` directory)
  is safe to commit, and git merges it without conflicts. A team server can
  hold new identities' keys instead and sign for them within a scope
  (`bracken serve --keys`), but they're still files any process of the same
  user can read.
- **Governance.** Any key can contribute to any Ground it can reach, and keys
  are shared among this repository's agents, so who signed what is a
  convention, not a guarantee. The same holds for Weave's integrators and
  validators.
- **Scale.** Each replica loads its whole graph into memory when it opens.
- **Weave as the repository of record.** Git still holds `main`, history,
  blame and releases ([weave.md](docs/design/weave.md), section 7).

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option. Contributions are accepted under the same terms.
