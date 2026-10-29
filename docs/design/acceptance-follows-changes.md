# Acceptance follows changes

> **Decided, 2026-10-29.** The owner chose to build this as proposed
> (decision `2b91a9541aba`), with the answers recorded in
> [Decisions](#decisions). Task `cd514bf16db9` wrote the note, under decision
> `f3405c284180` (item D), from cause 2 of the Phase 2 pilot analysis
> (`e6194943a1bd`). The build is task `cdfe62a33a9d`.

## The problem

Today an accepted contribution stays accepted however the things it rests on
change. In the Phase 2 pilot, wave 2 superseded 6 spec claims. Functions
defined in terms of those specs, and the members of their open contracts,
kept their old specs, so their implementations kept their passes and stayed
in the accepted layer. 14 functions ended wrong. In 9 of them, the cause was
that an implementation's passes had been judged before the change it now had
to follow.

Nothing in the Ground said those passes were stale. A reader saw `accepted`,
and so did every validator looking for work.

## The rule

Under a policy that opts in, an accepted subject `X` is reported as
**`needs_recheck`** when something it depends on changed after the evidence
that accepts it was recorded.

- **What `X` depends on.** Its *dependency closure*: everything reachable
  from `X` by following, from `from` to `to`, non-retracted relations that
  count, of these kinds:
  - `depends_on`: `X` needs `Y` to make sense or to work.
  - `implements`: an artifact realizes a claim.
  - `refines`: a narrower claim rests on the broader one.

  A relation counts according to the node `N` it leaves, at every step of
  the walk, not only at `X` (security note `9769ab37127d`):
  - an `implements` relation counts only if `N`'s author wrote it;
  - a `depends_on` or `refines` relation counts only if its author is `N`'s
    author, or the author of a claim that `N` implements through such an
    `implements`.

  So a stranger can't make `X` implement their own claim, and a third
  party's edge out of someone else's node drops out of the chain, while
  their edges out of their own claims count. A stranger's relation can't
  force a re-check. In the experiment, an artifact's `implements` is its
  author's, and the setter writes the specs' `depends_on` as the author of
  the spec claims. Content is never parsed: a dependency exists only if a
  relation says so.
- **What changed means.** A contribution `Y` in the closure is *changed* if
  either of these holds:
  - `Y` has a current supersessor under the protocol's supersession rule (a
    `supersedes` relation by `Y`'s author, whose replacement is live).
  - `Y` is retracted.
- **After the evidence.** For a pass `E` that counts for `X`, a change is
  after `E` when the contribution that made it is not an ancestor of `E`: the
  `supersedes` relation, or the `retract`. That means `E`'s author hadn't seen
  it when judging. Only causal order is used, never timestamps.
- **The status.** `X` is `needs_recheck` if it would be `accepted` today and
  fewer than `min_passes` distinct counted passes come after every change in
  its closure. The assessment lists the changes, so a reader sees what to
  re-check against.

Supersession comes first (review `433005735918`, security `5c1e7b364f8a`). A
`needs_recheck` replacement still supersedes the version it retired, which
stays `superseded` and out of the accepted layer. Superseded work is
`superseded`, not work to recheck.

Today's statuses keep their meaning: `retracted`, `rejected`, `accepted`,
`pending`, `superseded`. `needs_recheck` only ever replaces `accepted`, and
only under the opt-in. It is not part of the policy's accepted layer, so
nothing builds on it until it is re-checked (decision 1).

**Transitivity and cost.** The closure is transitive: an implementation of
`within`, which calls `dist`, which calls `abs`, follows `abs`'s spec. Cycles
(such as contract partners depending on each other) are walked once.

The cost of one evaluation, for `A` accepted subjects, closures of at most
`k`, `C` changes and `N` contributions:
- **The closures:** one walk each, `O(A·k)`. They can be shared, since most
  subjects reach the same few claims.
- **"After":** one descendant walk per change, `O(C·N)`, then a constant-time
  lookup per pair of pass and change. As built, this is a single sweep in
  causal order carrying 64 changes at a time, `O(N·C/64)`, shared by every
  subject (see [As built](#as-built)).
- **At the pilot's size,** about 1,000 contributions, 90 specs with 139
  dependency edges, and 12 changes: a few thousand steps, small next to the
  topological order every evaluation already computes.

**How a re-check clears it.** A new counted pass on `X`, from an author who
has seen every change in its closure, so that every change is its ancestor.
`min_passes` such passes make `X` `accepted` again. Other routes are:
- A counted fail makes `X` `rejected`, as today.
- `X`'s author supersedes it with a new contribution, which is judged afresh.
- The change is undone (a retracted supersession, say). `X` is then
  `accepted` on its old evidence.

## What doesn't shift

- **Off by default.** `Policy` gains an option, `follow_changes`, false by
  default. The CLI gains `--follow-changes` on `show`, `accepted` and
  `status`. Without it, every status and every accepted layer is computed
  exactly as today, on any Ground, old or new.
- **This team's merge gate.** `scripts/merge` asks `bk show <claim>
  --validator … --min-passes N` for `accepted`, and passes no new flag. So a
  pr claim's gate doesn't change. Opting the gate in would be a separate
  process decision. It would make a pr claim `needs_recheck` when, say, the
  task it implements is superseded after the counted test run.
- **Stored data.** Nothing new is written. The rule reads relations,
  retractions and parents that already exist.

## Class

This touches `manyroot::validation` and the CLI only:
- A policy option.
- A `NeedsRecheck` status, serialized as `needs_recheck`, and only produced
  when opted in.
- The list of changes in an assessment.
- The flags and the status view.

It changes no encoding, id, signing context or validity rule. Validity
converges as before, because acceptance is computed by each reader and
never replicated (protocol.md, *Derived state*). It uses the existing
relation kinds and `Graph::is_ancestor`.

`docs/protocol.md`'s *Acceptance* section would gain the opt-in rule. That
file is listed with the `protocol` class in AGENTS.md, so the manager should
class the change. It was classed as library and CLI, with an independent
review (decision `2b91a9541aba`). It is a reader's policy, not a validity
rule, and needs no migration note: every existing Ground reads the same.

## The experiment's Bracken mode

In the pilot, agents wrote 240 `implements` relations and no `depends_on`.
The rule needs dependencies that the Ground states. Two ways in:

1. **The setter states them** (decided, decision 3). When it publishes the task, the
   setter relates each spec claim with `depends_on` to the spec claims of
   the functions it is defined in terms of, and to its open contract's other
   members. For Phase 2 that's 99 call edges and 40 partner edges, taken from
   the task's own structure, which the task text already states to every
   mode. So no mode learns more than the others. An implementation then
   follows its spec's dependencies through `implements`.
2. **Agents state them.** Each artifact relates `depends_on` to the
   implementations it calls. This is more generic and finer grained (it
   follows a rewritten callee, not only a changed spec). But it costs a call
   per edge in a turn that is already at its cap, and needs a prompt change.

Either is a Bracken-mode change and goes through the neutrality review. The
mode's own accepted layer (the harness's pick in `bracken_mode.rs`) would
use the same rule, and A's review queue would list `needs_recheck` work with
the pending work.

## How it would have changed the pilot

This is replayed on the pilot's end state with the setter's relations of way
1 added. Timestamps stand in for causal order, which the replay can't
rebuild exactly. Of the 73 functions with an accepted implementation at the
end:

| | Correct at the end | Wrong at the end |
|---|---|---|
| Marked `needs_recheck` | 5 (deadzone, shrink, mulsign, band, gap) | 9 (dist, flip, isless, issame, energy, path3, span, within, snap) |
| Left `accepted` | 54 | 5 (nearer, mix, press, grade, index) |

- **The 9 caught:** passed before wave 2 and changed by it. dist, flip,
  isless and issame are wrong in their own code. The other five are right
  on their own but call those. They would have shown as needing a
  re-check instead of as accepted.
- **The 5 correct ones flagged:** each costs one re-check that passes.
- **The 5 missed:** each was passed after the waves. They are wrong through
  what they call (nearer and mix call flagged functions), or a validator
  passed wrong code (grade). This rule can't catch either.
- **The 17 never-accepted functions** are unaffected: they had no
  acceptance to follow.

Flagging isn't fixing. Each flagged function still needs a validator, which
cause 1 of the analysis says were scarce. So the rule's effect on the
ranking depends on A's queue putting this work in front of reviewers. What
it changes for certain: a reader of the Ground, or of the report, no longer
sees 9 wrong functions as established.

## Decisions

The owner's answers (decision `2b91a9541aba`, 2026-10-29) to the questions
this note left open:

1. **A `needs_recheck` subject is out of the policy's accepted layer.**
   Nothing builds on an acceptance whose basis changed.
2. **The closure follows `depends_on`, `implements` and `refines`.** It
   doesn't follow `tests` or `supports`.
3. **In the experiment, the setter states the dependencies** from the task's
   structure (way 1).
4. **A dependency relation counts only if its author is the subject's author
   or the author of the claim the subject implements.** The manager's answer
   for the build (`5d2fc3a65d90`, from security note `9769ab37127d`) applies
   this at every node of the walk, and counts only `implements` relations by
   the node's own author. [The rule](#the-rule) states it so.

It is off by default (`Policy::follow_changes`, CLI `--follow-changes`), and
this team's merge gate is unchanged. The class is library and CLI
(`manyroot::validation`), not protocol. It has an independent review because
`docs/protocol.md`'s *Acceptance* section gains the opt-in rule. Its use in
Bracken mode goes through the neutrality review.

## As built

Task `cdfe62a33a9d`:
- **Library.** `Policy::follow_changes`, `Status::NeedsRecheck` and
  `Assessment::changes` are in `manyroot::validation`, with
  `validation::dependencies` for the closure. `Policy::assessor` shares the
  work between subjects, and `evaluate` and `status` use one. Once per
  assessor:
  - each node's direct dependencies and changes;
  - which changes each pass had seen (one causal sweep);
  - which changes each node reaches (one walk over strongly connected
    components).

  The first version asked `is_ancestor` per pair of pass and change, and a
  stranger could make that cubic (finding `73252cef945e`: 11.7 s at 1,200
  contributions). Now a subject costs its passes times the number of changes
  divided by 64. Tests bound both shapes: one subject with many passes and
  changes, and many subjects on a long chain. Tests are in
  `crates/manyroot/tests/follow_changes.rs`. They include both of security's
  cases (a stranger's `implements` onto their own claim, and a third author's
  `depends_on` mid-chain). With the option off, the fixture Ground's
  assessments and accepted layer are compared with
  `tests/fixtures/ground-v1-acceptance.json`, recorded with the code before
  this change (`44da9245`).
- **CLI.** `--follow-changes` on `show`, `accepted` and `status`. `show`
  prints the changes. In `status`'s review queue (task `f1c8478d1012`, PR
  #77), `needs_recheck` work stands as `recheck`. It ranks with the waiting
  and ready work, before blocked, rejected and accepted work. It is also
  listed for every validator's pass except its author's, including the
  validators who passed it before the change, since their earlier passes
  don't speak to the change.
- **Bracken mode.** Only Phase 2 tasks (the ones with waves) use it, so
  Phase 1 runs read as before. The setter states each spec's `depends_on`
  when it publishes the task, and states them again for the new spec claims
  at each release. The harness's pick and the `accepted`, `status` and `show`
  tools read with `--follow-changes`.
- **Not added.** The pilot replay as a regression case: it needs the pilot's
  Ground, which isn't in the repo. A mode test covers the same path instead.
  It passes an implementation of `toward`, then wave 1 changes `sign`. The
  implementation then needs a recheck, and a new pass clears it.
