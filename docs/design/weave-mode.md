# Weave: content, integration and main as a query

A design sketch, not a plan. The product spec built from it is
[weave.md](weave.md) (task `04b85815f4a1`). Owner decision `af67403d89cd` asked for it and
task `c053814f2efc` holds the brief; the owner's answers on 2026-10-29 shaped
this version. Nothing here is built, and nothing here changes the protocol.
A model run of the experiment mode at the end needs the owner's go.

## Where we start

Phase 2 so far (pilot `67d2243b2e13`, replicate 1 `15c2fffb7fdc`) shows each
mode's strength and its weakness:

- **CRDT** never blocks and converges at once, but on whatever was typed.
  Replicate 1's CRDT ended with 35 of 90 functions wrong, and its shared
  document broke 156 times along the way (a count of breakages across
  deliveries, not the final state).
  Nothing in the mode can say no.
- **Git** got all 90 right in both runs. One canonical `main`, which a change
  either merges into or doesn't, keeps a bad state from spreading.
- **Bracken** has the richest way to say what is good (evidence, policies,
  rechecks when a dependency changes) but no content of its own, no
  integration step, and a high cost of reading (calibration `952057369465`:
  its rise is all context).

Bracken's users are **many agents** (owner, 2026-10-29), not humans typing
together. Agents work in whole turns, reading is their main cost, and two
attempts at the same thing are normal.

## What we take from earlier work

- **The Ground is already the right CRDT.** Kleppmann's Byzantine
  fault-tolerant CRDTs and Merkle-CRDTs use a graph of signed updates named
  by their hashes; replicas compare heads and take the union, and it holds
  with any number of misbehaving nodes. That is manyroot's design. A CRDT
  belongs at the record layer, and it is there already.
- **Merges needn't fail.** Pijul (a CRDT over graphs of lines) and Jujutsu
  record a conflict as a state, resolved later by a change that then holds
  everywhere. Concurrent versions of one thing are *alternatives*, not an
  error and not an interleaving.
- **A code unit's identity includes its dependencies.** Unison names each
  definition by a hash of its syntax tree with its dependencies' hashes in
  it; names are pointers that can move.
- **Main stays green only if every change is tested as it will land.** The
  "not rocket science rule" (Ben Elliston; Graydon Hoare's bors for Rust,
  and today's merge queues) tests each change merged onto the tip of main
  and moves main only when that passes. It exists because two changes that
  each pass alone can break together (merge skew). Batching, with a failing
  batch bisected, keeps it fast.
- **Content CRDTs are for people typing together.** Automerge and Ink &
  Switch's Patchwork merge keystrokes; agents don't produce keystrokes.

## Two decisions this sketch rests on

1. **CRDT at the record layer, not the content layer** (owner: yes). Content
   is immutable unit versions in the Ground; nothing merges text character
   by character. A content CRDT may come back later as an optional drafting
   tool on top, outside the protocol.
2. **An integrator role** (owner: yes). Any replica may take it; what it
   does is recorded in the Ground like any other contribution. It is a
   role, not a server.

## The layers

| Layer | Holds | Today |
|---|---|---|
| Record | Signed, hash-named contributions; sync by union | manyroot, as is |
| Content | Unit versions as artifacts; *manifests*: an artifact listing unit hashes, like a git tree | New, generic |
| Claims | implements, depends_on, evidence, policies, rechecks | As is |
| Integration | The integrator tests whole configurations and records evidence on manifests | New, in Bracken's own terms |
| Views | Main and other lines as queries; an agent's slice; per-language lenses | New; lenses outside manyroot |

**Content.** A unit is whatever a lens says it is: a function, a file, a
document section. manyroot sees artifacts and manifests only and never
assumes code. A manifest names a configuration exactly: these units, at
these *content hashes*. A content hash is the hash of a unit version's
payload alone. It is not the artifact's `ContributionId`, which also covers
the author, parents and timestamp (`docs/protocol.md`): two agents writing
byte-identical versions make two artifacts with one content hash. The
manifest maps each content hash to the signed artifacts that carry it, so
nothing about contribution ids changes.

**Acceptance attaches to configurations.** A unit's own evidence (its cases
pass) is useful but never enough: a unit accepted beside one version of its
dependency may break beside the next. So *what counts* is a manifest that
has passed as a whole. The integrator builds the next candidate (the line's
current manifest plus accepted unit changes), tests it, records pass or
fail on it, and the line moves only on a pass.

**Writes never block.** Anyone adds unit versions and proposes changes at
any time. Two versions of one unit since the last manifest are
alternatives. They are resolved by *selection*, not retirement: the next
manifest names the version chosen, and that holds on every replica. Core
`supersedes` counts only when the retired contribution's author signs it
(`docs/protocol.md`), so an integrator can't retire someone else's
alternative in the Ground, and doesn't need to: the unchosen version stays
current there, just not in main. Giving anyone else authority to retire it
would be protocol work and is not proposed here.

**Main is a query.** Main is the head of the project's line: the manifest
accepted under its policy that descends from the previous main and that no
other qualifying manifest builds on (defined causally, as below). Agents ask for their
slice of it ("the accepted definitions of what I call") instead of reading
the log.

## Lines: main and the others

The owner asked whether there can be several divergent materializations,
like `main` and other branches, that are later brought back together, as in
Git. Yes, and there are two different things that word covers.

**Lines of history.** A line is a sequence of manifests, each naming its
parent manifest or, for a merge, two parents. `main`, `release-2`, or an
experiment line are lines. Each has a policy (who must pass a manifest for
the line to move) and an integrator. Lines diverge freely: nothing is
copied, since unchanged units are the same hashes on both.

- **A line's head is a query, not a mutable pointer**: the manifest
  accepted under the line's policy that descends from its previous head and
  that no other qualifying manifest builds on. If
  two manifests both qualify and neither descends from the other (two
  integrators moved the line at once), the line has *forked*. That is
  Git's non-fast-forward, made visible, and it is settled the same way as
  any divergence: a merge.
- **Bringing lines back together** is a merge manifest with both heads as
  parents, worked out per unit against their common ancestor manifest:
  - a unit changed on one side only takes that side;
  - a unit changed identically on both is one hash, so nothing to do;
  - a unit changed differently on both is a pair of alternatives, a
    first-class conflict, resolved by a contribution;
  - a unit added on one side and removed on the other is also a conflict.
  The merged manifest is then a candidate like any other: the integrator
  tests it whole before the target line moves, because units that merge
  cleanly can still break together.
- **Resolutions are reusable.** A resolution names the alternatives it
  settles, so the same pair met again (merging the same lines twice,
  replaying a merge onto another line) resolves the same way, as in Pijul.

**Views under different policies.** The same history can have more than
one accepted head if teams accept differently: a strict release policy
moves its main less often than the team's. These are not divergent
histories but different readings of one; they never need merging, only
choosing. A build names the line and policy it materializes.

**What makes merges hard, still.** Renames and splits (one unit becoming
two) look like delete-plus-add unless a lens records them as a relation; a
long divergence piles up alternatives; and a unit unchanged on both sides
can still need a recheck if what it calls changed on one. The last is why
every merge goes through the integrator.

## The hard parts

- **Throughput.** One integrator per line serializes progress. Batches of
  accepted changes, with a failing batch bisected, are the known answer.
- **Units in real languages.** Macros, impl blocks, imports and tests make
  units fuzzy in Rust or Python. That is the lens's problem, per language,
  outside manyroot.
- **The test oracle.** The integrator's evidence is only as good as the
  tests it runs; flaky and slow tests matter more here than in Git, since
  every line move waits on them.
- **Trust.** Keys are shared today, so a signature records who claimed to
  act. An integrator whose pass moves main needs its own key.
- **Size.** Manifests and unit versions grow; compaction must keep
  signatures valid.
- **Determinism.** Every replica must compute the same head for a line and
  policy from the same Ground, so policies and the head query must be
  deterministic.

## Testing it in the experiment

The fourth mode tests the integration idea, not a content CRDT: **Bracken
plus an integrator.**

- Agents work as in Bracken mode: unit artifacts, `check`, evidence, rechecks.
- After every agent-turn the harness acts as the integrator, deterministically
  and with no model calls: it builds a candidate from main plus the units
  accepted since, runs the whole library's public cases, and moves main only
  on a pass. On a fail it records the fail on that candidate manifest, not
  on any unit, and tries smaller groups of the batch. Bisection only
  sometimes finds a single culprit: a change and the one it needs fail
  apart and pass together, and two changes that each pass can fail only
  together. So failures stay attached to exact configurations, and the
  integrator records which groups passed and which didn't.
- Agents read main through an `accepted` tool (their slice of main's
  manifest) instead of the log.
- The grader grades main at each checkpoint, so what counts is what the
  integrator accepted, as in Git.

It answers two questions: can Bracken's evidence plus a Git-style gate reach
Git's 90 while keeping writes that never block; and does reading main
instead of the log bring Bracken's tokens per correct function (306k in
replicate 1) toward Git's (150k)?

**Not a fair race.** Weave mode is exploration, not a ranking entry (owner,
2026-10-29: "I don't really care about staying fair to other modes"). It may
have extra agents, its own tools and prompts, and an integrator on top of
the working agents. The other three modes stay untouched, so their recorded
runs stay valid as reference points, and none of this changes the Phase 2
replicates.

**Cost.** One builder PR in `crates/experiment` (an integrator over
Bracken mode's store, the `accepted` tool, tests and mock runs, no model
calls), a day or two of builder time and the usual reviews. Then a 2×2
calibration (about 0.6 M tokens) and a replicate. Four modes make a
replicate about 56 M tokens, so either the per-replicate total rises to
about 100 M or the new mode runs against fresh runs of the others; that is
the owner's call, with the runs, each needing a go.

## The owner's answers, and what they mean

On 2026-10-29 the owner answered the open questions:

1. **Weave mode alone, for a while.** It is developed on its own, not in a
   four-mode replicate. Runs compare it with the Phase 2 replicates already
   recorded; those carry their own machine load and harness version, so
   they guide development and don't support claims. A fair head-to-head
   comes later, with fresh runs of the others, each with a go.
2. **The integrator is an agent with goals, and integration is a
   collaborative decision.** It decides when to integrate and what moves
   main from what the team is trying to achieve now. It may accept some
   breakage to make progress, then fix and reintegrate.

## The integrator as an agent

This replaces the deterministic integrator above. The mechanics (build a
candidate, run the tests, bisect a failing batch) stay, as the
integrator's tools; the decisions become judgment, recorded.

**Goals are contributions.** The team's current goals are claims tagged
`goal`: "wave 2's signature change lands everywhere by turn 6", "nothing in
`base` regresses". Anyone may propose one; the policy says whose goals
bind. The integrator reads them and integrates toward them.

**Integration decisions are contributions.** Each move of main is a claim
by the integrator: this manifest, these tests' results, the failures it
accepts (*known breakage*), the goal it serves, and by when the breakage is
to be fixed. Agents `support` or `contradict` it with reasons, as with any
claim. That is the collaboration, in the Ground and on the record, not in
messages.

**Agreement comes from the record and the descent rule.** The Ground has
no total order: timestamps are advisory (`docs/protocol.md`), so there is
no "latest" decision to read. A line's head is therefore defined causally:
an integration decision by an authorized integrator moves the line only if
its manifest descends from the head the decision names as its parent, and
the head is the decision that no other qualifying decision builds on. Two
qualifying decisions that build on the same head, with neither on the
other, are a fork, shown as such on every replica, and settled by a merge
decision that names both. Every replica holds the same decisions, so every
replica computes the same head or the same fork. This keeps the earlier
sketch's descent rule rather than replacing it.

**Two heads, so breakage is never hidden.**

- *Main* is where the team works: the line's head as defined above, known
  breakage and all.
- *Green* is the nearest manifest at or behind main, following main's
  ancestry, with no known breakage against the *current* specs and tests.
  While main is forked there is no single main, so green is undefined until
  the fork is merged; readers then use the last green before the fork. A pass recorded before a spec changed doesn't
  carry over: after a change request, green is the last manifest that has
  passed the new tests, or none yet. Anything that must not break (a
  release, a build that ships, an agent that wants a stable base) reads
  green.

Both are queries over the same line; nothing is copied. "Fix and
reintegrate" is main getting back to green.

**Guardrails, so breakage stays a choice.** The policy, not the
integrator, sets limits: at most *k* known failures, none older than *t*
line moves, never in units the policy marks protected. Past a limit, main's next
move must reduce breakage, or main falls back to green. The integrator can
argue to change a limit; the change is a policy decision like any other.

**Collaboration, bounded.** Deliberation costs tokens, and the record has
no clock to count a window in. So the window is causal: a contradiction
counts against a decision only if it names that decision and the next
decision on the line builds on it (it was seen before the line moved on).
One made after the line moved on stays open: the line's next decision must
build on it and answer it, by fixing, overriding with a recorded reason, or
falling back to green. A decision that leaves an open contradiction
unanswered doesn't qualify to move the line. A contradiction wins by default only if it cites
recorded failing evidence on the decision's manifest or names a goal the
decision breaks; anything else the integrator may override, with its
reason recorded.

**This is application policy, not manyroot's.** Heads, green, goal binding
and override authority are views a Weave layer computes over the Ground.
They are not `manyroot::Policy` acceptance: under it, a manifest made and
tested by the same integrator has no independent pass, and a manifest with
known failing evidence stays rejected. That is fine as long as the two are
never confused: a Weave head says "the line moved here", not "manyroot
accepts this". Breakage age and the contradiction window are counted in
line moves (decisions), not turns or time, so every replica counts them
the same way.

**Trust, in v0.** Keys are shared today, so any agent can sign as an
integrator, a goal-setter, a policy author or a contradictor, and a
signature records who claimed to act. Until keys are per agent:

- only the policy names integrators, goal-setters and who may change the
  policy, and an integrator may not change its own guardrails;
- green needs a pass on the manifest from someone other than the
  integrator, as a merge needs today;
- goals and reasons are free text that other agents read, so they can carry
  prompt injection; the integrator's tools act only on manifests and
  evidence, never on instructions found in text;
- elections (a later configuration) need real identities first.


## What I'd push back on

- **It is harder to measure.** With a deciding agent, a result shows
  whether the team met its goals, not which mechanism helped. So every run
  records the goals, every decision and its reasons, and the grader adds
  two measures: turns spent below green, and known breakage that was never
  fixed.
- **It costs tokens.** The integrator is a model agent too. It runs as an
  extra agent on top of the working agents, and its tokens are reported on
  their own so its cost is visible.
- **Breakage spreads.** Others build on a broken main. Green is the
  defence: agents choose which head to build on, and the integrator's
  reasons say which to prefer.
- **Who integrates is a trust question.** One dedicated agent, a rotating
  role, or one the team elects: each changes the dynamics. The first runs
  use one dedicated agent (owner: agreed). The others are later
  experiments, and the mode keeps the integrator's selection a run option
  from the start so they need no redesign: `dedicated`, `rotating` (each
  agent integrates in turn), `elected` (agents name one by claims), and
  perhaps `several` (more than one integrator on a line, forks settled by
  merge).

## Configurations

Owner, 2026-10-29: goals, the integrator, guardrails and the rest "should
be different configurations that we could try out and experiment with". So
the mode is built with each as a run option, recorded in the run's plan,
and no answer is fixed in code. The first development runs use the simplest
value of each (marked *first*); the rest are later experiments.

| Option | Values |
|---|---|
| Goals from | `setter` (*first*: the task's priorities), `team`, `both` |
| Integrator | `dedicated` (*first*), `rotating`, `elected`, `several` |
| Guardrails | `none`, `fixed` (*k* known failures, *t* line moves, protected units), `learned` (set from earlier runs); *first*: `fixed`, generous values recorded in the plan |
| Deliberation window | line moves a decision stays open to contradiction (causal, as above); *first*: 1. Separately, harness pacing: how many agent-turns the integrator waits before deciding, which never changes what counts |
| Heads agents build on | `main` (*first*), `green`, `agent's choice` |
| Integrator's tests | per decision (*first*), or batched with bisect |

**Runs still need the owner's go.** Configurations don't change that: each
model run, or a series of them the owner approves together, is asked for
with its configurations and token estimate first.
