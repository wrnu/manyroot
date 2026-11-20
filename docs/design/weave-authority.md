# Who may stop a line: a removed integrator, and a replaced fail

This is a design only. It changes no format, code, test or fixture (task
`40798b0c2c4f`, decision `a2e0b9c241c7`). The owner answered section 4 on
2026-11-19 (decision `51815720d632`): A5 yes, before the join; A4 yes, from
the join; A3 not now; B4 and B6 yes, before the join. Task `e2ab087e8d3e`
builds A5, B4, B6 and the `scripts/weave-shadow` change (docs/releases.md,
2026-11-19). It covers two items from the
hostile-writer review (task `ee121849975b`, weave-freeze.md question 8).
The owner chose to settle them by design, not in a fix PR:
- **A.** A removed integrator keeps the power to stop main, and so every
  line (finding `39cb855d11d0`, deferred task `87159875856a`).
- **B.** Any writer can reuse a validator's replaced fail as an
  evidence-backed objection, which blocks one green tree (finding
  `6cad0100be83`). This was accepted for now, with this note to decide
  whether the latest-result rule should apply to an objection's basis.

File references are to `main` at cb6eb1fc. The triage that sent both
items here is note `6ce8a8d82af5`, points (3) and (4).

## Summary

- **A: what's wrong.** The reader asks whether an unreadable (v2)
  decision's author is an integrator, and it asks this under the policy in
  force in *that decision's own* causal past. The writer chooses that past.
  So an integrator who was removed, writing from a stale replica, can still
  produce a decision that every reader counts. It stops `integrate`,
  `validate`, `green`, `line merge` and `scripts/weave-gate` on main, and so
  on every line. Retracting it is pointless, because they can write another.
  The same chosen past lets them do something smaller with ordinary v1
  records too: fork main. That's the stale-past exposure weave-freeze.md
  section 3 already states, and question 7 accepted it.
- **A: recommended, option A5.** Narrow G8's reach. An unreadable decision
  counts only if its author integrates the line under the policy in force
  in its own past (as today) **and** under the line's current policy. A
  decision brings other lines into a judgement only on the same terms.
  This is a reader change to a pinned rule, in `crates/tree/src/lines.rs`,
  and it changes no qualification verdict. It needs no v2, no cutover and
  no readers-first rollout. Once a reader holds the removal, removing an
  integrator ends any stop they started, and they can't start another.
  The exception is a line they author or still integrate that an honest
  integrator has merged into main. That's the route decision
  `a2e0b9c241c7` already accepted. This assumes every reader upgrades
  before any v2 decision is written. Also left is question 7's accepted
  fork. Option A3, a removal cut that
  also ends the fork, is set out but not recommended now. It's the
  line-local form of part B's revocation cut (agent-keys.md 4.4), and the
  two should be designed once, together (question 3).
- **B: what's wrong.** An objection's basis counts a validator's fail
  "whether or not a later result replaced it" (weave.md section 2, rule 5).
  An objection to a tree a decision uses can be answered only
  `accepted_breakage` or `overridden`, and `overridden` is for discussion
  only. So anyone who points a `contradicts` at a manifest from a fail the
  validator has since replaced makes that tree impossible to integrate
  unchanged under `max_known` 0. Under a higher `max_known` it can be
  integrated, but never green. The tree itself is the only casualty: any
  other tree gets past it (note `6ce8a8d82af5` (3)).
- **B: recommended, option B4.** A fail that a pass replaced still backs
  an evidence-backed objection, but that objection may also be answered
  `overridden`. A pass replaced it when the validator's latest results for
  that manifest and method, in the decision's past, are a single pass. A fail
  replaced by another fail, or by concurrent passes, stays binding. This widens rule 5's table and narrows nothing, so no
  decision that qualifies today stops qualifying. It's pinned, not frozen.
  It needs readers upgraded before any integrator relies on it, a matching
  change to `scripts/weave-shadow`, and `weave review` to show such
  objections (B6).
- **Neither recommendation changes a frozen format or rule** (weave-freeze.md
  section 0). Both are protocol class, as changes in `crates/tree/src/`, and
  need the manager's go logged as `owner-review`. Neither needs a v2.

## 1. A: a removed integrator keeps a stop

### 1.1 The problem

G8 (weave-freeze.md 2.1 and 2.4) makes a reader refuse to act on a line
while that line holds a decision it can't decode. This is how a stale binary
fails closed, rather than computing heads without a decision a newer build
would count. To stop anyone at all from stopping a line this way, G8 counts
an unreadable decision only in these cases (`lines.rs` 308-323, 393-405):
- its top-level `v` is above 1;
- it isn't retracted;
- its top-level `line` is a counted version in its causal past;
- under the newest version of that line's chain in **its own causal past**
  (`newest(&line, &before)`, with `before` testing ancestry of the
  decision), its author is an integrator.

Rule 2 judges decoded decisions the same way: `P_E` is the policy in force
for `E` (`lines.rs` 739, `policy_in_force` 898-936; weave.md section 1,
*Lines*). Weave chose this on purpose: "that judgement never changes later"
(finding `b2098df8e4be`). The protocol lets a writer list any present
contributions as parents (`crates/manyroot/src/graph.rs` 159-166). So the
writer decides which policy is in force for their record, among the
versions they hold.

**Counterexample** (finding `39cb855d11d0`, reproduced by the security
reviewer on a throwaway Ground):
1. The owner adds mallory as an integrator of main. Mallory keeps a copy
   of her replica.
2. The owner removes her (`line policy main --clear integrators
   --integrator weave-integrator`). Main is unaffected.
3. On her stale copy, which doesn't hold the removal, mallory writes a
   `{v: 2, line: <main>}` decision artifact, and it is imported.
4. Every reader that holds both the removal and her decision judges her
   decision under the version before the removal, which names her. G8
   counts it. `integrate` and `green` on main refuse ("1 record(s) on the
   lines this judges don't decode ... upgrade bracken"), and so does
   `scripts/weave-gate`. Every command judges the default line, so every
   line stops.
5. Only she can retract it, and from the same stale copy she can write
   another.

So **anyone who was ever an integrator of main has a lasting stop over every
line until every reader decodes v2**, whether or not they were removed
first. docs/releases.md (G8 entry) and weave-freeze.md 2.5 say removal
doesn't lift an existing stop. The finding adds that removal doesn't take
away the power to start one either. The evidence agrees with the finding:
nothing in the code above reads any version outside the decision's own
past.

### 1.2 What the same past gives a removed integrator with v1 records

This is outside the finding, but every option below has to be weighed
against it.

A v1 decision by mallory from the same stale past qualifies, by rule 2. Its
parents are old decisions on main, so it becomes a second head: main forks,
and `weave head` shows the fork. Green drops back to the last green before
the fork (weave.md section 2, *Green*). The next honest `integrate` lists
every head of the line as a parent (`lines.heads(&line)`,
`crates/bracken/src/weave.rs` 729), which
adopts her tree without saying so. The tree still has to pass the line's
checks to go green, and her decision still has to answer every objection
in its window, but she chooses the tree.

This is the exposure weave-freeze.md section 3 states for the cutover: "the
same exposure every policy change has today, such as removing a validator".
The owner accepted it under question 7 (decision `bae92c497714`). G8 turns
it from a fork that anyone can see and an integrator can settle into a stop
that only a new build ends.

Two more of mallory's decoded decisions matter for the options. A
stale-past v1 decision of hers counts as "integrating" (`lines.rs`
438-467). So it brings the lines of its parents into the judgement, decoded
or not (`read_lines`, 661-683; finding `0783a2caf192`). A v2 decision on a
line she opened herself, named as that v1 decision's parent, therefore
stops main by the same route. Any fix to G8's reach has to close that route
too.

### 1.3 Options

Each option says which tier it changes (weave-freeze.md section 0), whether
it needs a v2 or section 3's cutover, what it costs, what it breaks, and
what's left.

"Existing Grounds" means `~/bracken-weave/manyroot` and its backups, the
`weave-v1` fixture, and the coordination Ground and its fixture. The fixture
never removes an integrator: alice is the only one throughout
(`crates/tree/tests/weave_fixture.rs` 41, 111). Whether the Weave Ground
has ever removed one has to be checked by any build task that changes rule
2, before it lands (A1, A3).

#### A1. Judge rule 2 against the line's current policy

**Rule.** A decision qualifies only if its author integrates under the
line's current policy (the version with no current successor), rather than
under the policy in force in its past. A variant reads "the policy at the
reader's view", the newest version this replica holds. On a converged
replica that's the same thing. The two differ only on a diverged chain,
where there is no current policy.

**Changes.** Rule 2 (pinned, not frozen; weave-freeze.md 1.6), and with it
the G8 test, which follows rule 2. No v2, no cutover. Heads change, so
every reader would have to be upgraded first (section 3 of
weave-freeze.md, *Pinned-not-frozen changes*).

**Breaks.** History. Rule 3 requires every parent to qualify. So removing
or rotating an integrator would unqualify every decision they ever made,
and every descendant of those, and main would fall back to before their
first decision. agent-keys.md 4.10 makes rotating the integrator an
ordinary policy change, and under A1 that change would wipe out main. It
also breaks the pinned promise that a later policy change never changes
whether `E` qualified (weave.md section 2), in every case rather than in
one.

**Residual.** None of the stop or the fork. **Not viable**: it trades a
stop by a removed integrator for a stop by the line's own author every time
they rotate a key.

#### A2. Bound authority by time, or by the integrator's own later records

**Rule.** For example, a decision counts only if its timestamp is before
the removal. Or an integrator's authority ends at a record of their own,
such as a resignation.

**Breaks and residual.** Neither binds a hostile writer. `timestamp_ms` is
advisory and set by the writer (docs/protocol.md), and every view in Weave
ignores it (weave.md section 2). A hostile integrator writes no record
against themselves. **Not viable.**

#### A3. A removal cut: bound by the line author's later records

**Rule.** Rule 2 holds as today, and one more condition is added. Take any
version `V` of the line's chain, in the reader's replica, that doesn't name
`E`'s author as an integrator, and that isn't in `E`'s causal past. Then `E`
must be in `V`'s causal past. In words: a decision concurrent with a policy
version that drops its author doesn't qualify. G8's test, and the set of
decisions that bring lines in, take the same condition. Every test is about
causal ancestry, which manyroot answers without decoding anything, so it
works for an unreadable decision too.

This is part B's revocation cut (agent-keys.md 4.4, "a key's cut"), limited
to one line and one role. The line author's removal acts as the revocation,
and it needs no ruling, because what falls outside the cut doesn't count.

**Changes.** Rule 2 (pinned), and G8's reach (pinned). No v2, no cutover.
Heads change on a Ground where an integrator was removed while they had a
concurrent decision, so this needs readers-first.

**Costs.** The index has to answer "is `E` an ancestor of `V`" for each
decision by an author some version drops: more questions for the
`Ancestry` indexes `Lines::build` already builds (`ask_chains`, `lines.rs`
783, and the rule-5 pasts, 514). That's few in practice. Rule 2 stops being a function of `E`'s past
alone, in this one case. A removal that arrives on a replica unqualifies
the removed integrator's concurrent decisions, and every descendant of
them (rule 3). That includes an honest integrator's merge that adopted
one, so main's head can move back on a replica that learns of the removal
late. The next `integrate` recovers the tree, since the honest
integrator's tree is still held.

**Breaks.** An honest integrator who integrated while offline and was
removed at the same time loses that work. That's arguably what removal
means. Stale replicas compute the old answer until they sync, as with any
record they don't hold. Existing Grounds change only where an integrator
was removed (see above). It doesn't interact with per-agent keys, but part
B would replace it with its own cut, and two cut rules, one per line and
one per key, would have to agree.

**Residual.** A v2 decision mallory wrote *before* her removal, and that
the removal's author already held, is inside the cut: it's in the
removal's past. It still stops main until a build reads v2, and only she
can retract it. (One she withheld until after the removal falls outside the
cut, so A3 ignores it.) A3 ends the power to start a stop, but not a stop
already started and seen. Pairing A3 with
A5's test for the unreadable decision itself ends both.

#### A4. Revoke the key at the team server (part A)

**Rule.** When the manager removes an integrator, the team server also stops
signing for that identity's token (agent-keys.md 4.2).

**Changes.** Nothing in Weave: neither tier. It's an operational step. It
applies only to server-held identities. Before the join, Weave's keys are
files (weave-marker.md, *Summary*), so it does nothing.

**Residual.** A record signed before the revocation and withheld still
works. Since parents are the writer's choice, a withheld record looks the
same as a fresh one. And any agent on this machine can read the server's
key directory (agent-keys.md section 2). It narrows A, and fixes nothing.
It's worth doing at the join anyway.

#### A5. Narrow G8's reach to the current policy (recommended)

**Rule.** Two changes to G8's counting, and none to qualification:
1. An unreadable decision counts against its line only if its author
   integrates under the policy in force in its own past (as today) **and**
   under the line's current policy (`Lines::current_versions`, `lines.rs`
   1048). If the chain has diverged, any current version that names the
   author is enough, which fails closed. Only the line's author can make a
   chain diverge.
2. A decoded decision brings other lines into a scoped judgement
   (`integrating`, `lines.rs` 438-467, read by `read_lines`) only on the
   same two terms.

**What it refuses.** Once a reader holds the removal, nothing a removed
integrator writes, before or after the removal, stops a line through G8. Not
a v2 decision on the line, and not a v1 decision whose parent is a v2
decision on a line of their own (1.2). The one exception is a line of
theirs that an honest integrator has already merged into main (see
*Residual*). Re-adding them restores their power, as it should. A current integrator can still stop the line. They're trusted,
and removing them now ends the stop, which is what docs/releases.md's
reader would expect.

**Changes.** G8's reach rule in `lines.rs`. That's pinned, not frozen
(weave-freeze.md 1.6: the rules in `lines.rs` apart from 1.3's). No v2, no
cutover. Verdicts don't change, so heads don't either. The only effect is
that some refusals become warnings that aren't shown. So it needs **no
readers-first rollout**. An old binary refuses where a new one proceeds,
which fails closed. `scripts/weave-gate` follows `weave head --line main`'s
warning and needs no change of its own. docs/releases.md's G8 entry and
weave-freeze.md 2.5 change with it. It relies on a v2 decision keeping
top-level `v` and `line`, which note `5f2ca4c476e2` already requires of any
v2.

**Costs.** Small: one more test in the undecodable-decision branch (393-405)
and in the `integrating` loop (456-460), against a current policy that is
computed once per line. Tests: the G8 tests named in weave-freeze.md 2.2,
plus the counterexample above as a test that fails before and passes after,
and the 1.2 route (a stale v1 decision whose parent is a v2 decision on a
line of hers). One existing test is inverted rather than kept:
`only_a_newer_decision_counts_and_line_policy_still_runs`
(`crates/bracken/tests/weave_lines.rs` 1647) asserts that main stays
stopped after the decision's author is removed. Under A5 it must assert
that the removal ends the stop. That's a deliberate change of pinned
behaviour, for the reviews to see. Protocol class: full table, `owner-review`, independent and security
review. Its tests go beside G8's (`crates/bracken/tests/weave_lines.rs`,
`crates/tree/tests/`), and no golden
vector or fixture changes, so it doesn't need the owner's own approval
(development.md, *Owner approvals*).

**Breaks.** Nothing honest, **provided every reader upgrades before any v2
decision is written**, as weave-freeze.md section 3's readers-first rule
requires. G8 exists so that a stale binary doesn't act without a decision a
newer build would count. A5 lets a v1 reader act without a v2 decision by
someone the line's author has removed. While readers-first holds, no v2
decision exists while v1 readers do. So a v2 decision that a v1 reader sees
is already outside the rules, and ignoring it once its author has been
removed is the owner's removal taking effect.

**If a v1 reader misses the upgrade**, here is what it does differently.
Take a legitimate v2 decision, written by an integrator who was later
removed, whose past names them. A v2 reader counts it by rule 2. A v1 reader
with A5 ignores it instead of refusing, and computes main's heads without
it. Today's v1 reader refuses. So the straggler's view diverges from the v2
readers' where today it fails closed: it can report a different head or
green, and integrate on it. That's the same kind of divergence question 7
accepts, a decision judged under a past the reader doesn't share, and it
lasts only until the straggler upgrades. The gate's readers are the pinned
binary and `~/bracken-weave`'s binary, both upgraded by the release step.
Stale replicas, offline writers and existing Grounds are unaffected: no
build has ever written a record with `v` above 1. Per-agent keys don't
interact with it.

**Residual: the merge route.** A5 narrows only decisions by the removed
author. Suppose an honest, current integrator of main has merged one of
mallory's lines into main. That merge decision is still integrating, so
`read_lines` still brings her line into main's judgement. An unreadable
policy version on that line still counts, and only she can write one,
since she's the line's author and nobody can remove her from her own line.
The same holds for an unreadable decision on a line she still integrates.
So a stop by her continues after her removal from main, A5 or not.
Removal from main ends her power to stop it only if no line she authors,
or still integrates, has been merged into main.
`an_unreadable_policy_version_stops_only_the_lines_that_judge_it`
(`crates/bracken/tests/weave_lines.rs` 1435-1468) shows this for another
line, and main follows the same code (finding `aa1d71de6af9`). The owner
has already accepted this route: "main's integrator merging a hostile line
that can then stop main" (decision `a2e0b9c241c7`). A5 doesn't change it,
and neither would A3.

**Residual: the fork.** Question 7's fork (1.2) stays: mallory's stale-past v1
decision still qualifies. One corner comes with it. Suppose her stale v1
decision names a parent on a line of hers that has an unreadable *policy*
version. A5 no longer brings that line in. So a v1 reader judges the parent
under the line's last readable policy, where a v2 reader might reject it. A
v1 reader can then integrate over a fork that a later v2 reader finds
unqualified, and main's head moves back on upgrade until the next
`integrate`. This needs the stale-past dodge, a v2 policy version written
before the readers-first release, and an honest integrator adopting the
fork. A3 closes it, together with the fork.

#### A6. Accept and document

**Rule.** No change. The docs already say removal doesn't lift a stop (since
PR #38). They would add that removal doesn't take away the power to start
one, and that the remedy is a build that reads v2.

**Residual.** Every identity that was ever an integrator of main holds a
stop over every line until v2 ships. That stays true after the join, when
integrators may be agents with their own keys. Removing a suspect or
compromised integrator, which is the main reason to remove one, wouldn't
protect main.

### 1.4 Comparison

| Option | Ends a removed integrator's stop | Ends the stale-past fork | Tier | v2 or cutover | Readers-first | Main cost or residual |
|---|---|---|---|---|---|---|
| A1, current policy for rule 2 | yes | yes | pinned | no | yes | rotating an integrator unqualifies main's history |
| A2, time or own records | no | no | pinned | no | yes | binds no hostile writer |
| A3, removal cut | new stops only | yes | pinned | no | yes | one exception to "never changes later"; a stop already started stays |
| A3 with A5's test | yes | yes | pinned | no | yes | as A3; two cut rules until part B |
| A4, server revocation | no | no | neither | no | no | withheld records; no effect before the join |
| **A5, G8 against the current policy** | **yes** | no | pinned | no | **no** | question 7's fork, and its corner |
| A6, accept | no | no | neither | no | no | every past integrator can stop every line |

### 1.5 Recommendation

**A5 before the join, and A4 at the join.** A5 closes what the finding
says, a stop that lasts past removal, using the smallest rule change that
does: verdicts don't change, rollout order doesn't matter, and no frozen
rule is touched. It brings G8 into line with what an owner removing a
suspect integrator would expect.

**A3 is not recommended now.** It goes further than the finding, since it
closes the fork the owner accepted under question 7. It's also the first
exception to "a later policy change never changes whether `E` qualified".
And it's a line-local copy of part B's key cut, which agent-keys.md 4.10
leaves open: whether the join waits for part B is the owner's call. Adopting
it now would mean designing two cut rules that must agree. So the
recommendation is to design it with part B's revocation, or before the join
if the owner wants removal to be revocation now (question 3).

## 2. B: a replaced fail, reused as an objection

### 2.1 The problem

An objection's basis comes from its `from` end. It's *evidence-backed*
when `from` is a check result counted under `P_E` that fails, on the
objected manifest (weave.md section 1, *Goals, checks and objections*).
For rule 5, "a counted fail is one eligible for the objected decision ...
whether or not a later result replaced it" (weave.md section 2, rule 5). The
code checks that the result is eligible and in `E`'s past, but not that
it's the validator's latest (`crates/tree/src/objections.rs` 614-626).

The basis decides the answers allowed (`Basis::allows`, 57-65). An
objection to a manifest or proposal that `E` uses unchanged allows only
`accepted_breakage` and `overridden`, and `overridden` is for discussion
only. So an evidence-backed objection to a tree used unchanged has exactly
one answer, `accepted_breakage`, which puts the check in `E`'s breakage.
Rule 6 then decides. Under `max_known` 0, the default for main
(`crates/bracken/src/weave.rs` 354, `weave/merge.rs` 330), `E` doesn't
qualify. Under a higher limit it qualifies, but carries breakage, so it is
never green (`results.rs` 522).

Anyone can write the `contradicts`. Only its author can retract it.

**Counterexample** (finding `6cad0100be83`, reproduced by the reviewer on a
throwaway Ground):
1. Line main has validator `val` and one check `t`.
2. Commit 1 is imported as manifest artifact `MID`.
3. `val` records `MID` fail, then `MID` pass: a flake, which `weave validate
   --again` replaced. `green --manifest` says green.
4. Mallory relates that fail `contradicts MID`.
5. `weave integrate` of commit 1 refuses: unanswered evidence-backed
   objection. With `--answer MID=overridden`, it says "answered only
   overridden, which its basis doesn't allow". With `--answer
   MID=accepted_breakage --breakage t`, it says "1 entries, more than the 0
   its policy allows".
6. `green --manifest` still says green, and `weave review` lists nothing
   open, so the integrator gets no warning (2.5's residual row:
   `weave review` doesn't list objections to a manifest artifact).

What the writer needs is their own key, and a fail on that manifest that a
later attempt replaced. Weave keeps every attempt on record on purpose
(weave.md section 2, *Counted results*), so any flake will do.

**What it breaks.** That a validator's own later pass decides the
manifest. The latest-result rule exists so "one flaky fail doesn't block a
manifest for good", and a non-validator turns the replaced fail back into a
binding one.

### 2.2 What gets main past it today

- **Any other tree.** The objection is bound to the manifest hash. A
  decision with a different tree doesn't use it, and an objection in a
  decision's window allows `fixed` and `fell_back` for any basis. So a
  one-byte change, or the next commit, integrates. This is why note
  `6ce8a8d82af5` (3) calls it a nuisance, not a lasting stop.
- **A new policy version.** A result counts only if it descends from `P`'s
  artifact (the policy epoch, `objections.rs` 625). After the line's author
  writes a new version, the old fail isn't eligible under it, so for
  decisions under the new policy the objection is discussion, and
  `overridden` is allowed. This needs an owner decision for the version
  (`scripts/weave`), and green starts over on every manifest (weave.md
  section 2, *Green*).
- **Under `scripts/weave-shadow`**, neither happens by itself. It answers
  only discussion objections by non-members (2.5, finding `598258717737`),
  so it stops on that commit, and main's mirror waits for the next one.

### 2.3 Options

#### B1. The latest-result rule for an objection's basis

**Rule.** A fail stops backing an evidence-backed objection only when it
has been *replaced by a pass*. That means: in `E`'s past, the validator's
latest eligible results for that manifest and method are a single pass.
This is green's latest-result rule, judged within `E`'s past, so several
concurrent latest results count as one fail, even if all passed (finding
`91b445207f20`). A fail replaced by another fail (the check
really fails again) stays binding. So does a fail beside a concurrent pass.
Only an objection from a fail a pass replaced becomes discussion.

**Changes.** The basis rule (pinned; weave.md 575-582, `objections.rs`
basis). No v2, no cutover. Readers-first, since heads can change.

**Breaks.** Discussion doesn't allow `accepted_breakage`. So a decision that
answered a replaced-fail objection `accepted_breakage` would stop
qualifying on upgrade, along with its descendants. Whether any existing
Ground holds one would have to be checked. B4 avoids this. It also adds an
ancestry question per objection's result (is there a later result by the
same validator, in `E`'s past?) to the asks the index builds.

**Residual.** None from replay. A fail that wasn't replaced by a pass marks a
tree that really fails for that validator. That tree isn't green anyway,
so an objection from it adds no new stop.

#### B2. Count it as evidence only when a policy member objects

**Rule.** A `contradicts` is evidence-backed only when its author is the
result's validator, or an integrator, validator or goal setter under `P_E`.
This is the finding's first fix option.

**Changes.** The basis rule (pinned). Readers-first.

**Breaks.** A reviewer outside the policy can no longer bind an
integrator by pointing at a validator's *current* fail. That's an honest use
the design allows today, and any writer may review (weave.md section 1). It
makes who wrote an objection matter for its basis, which no rule does
today. The same `accepted_breakage` problem as B1 applies to existing
answers.

**Residual.** A policy member can still replay a replaced fail. They're
trusted, but that's the same gap.

#### B3. B1, unless the fail's own validator objects

**Rule.** As B1, but a replaced fail still backs an evidence-backed
objection when the `contradicts` is by that result's validator. So the
validator can say "my pass was the fluke".

**Breaks and residual.** As B1, with one more rule to pin. A validator who
thinks its pass was wrong can already record a new fail, which becomes its
latest. That's the channel the latest-result rule provides, so B3 adds
little.

#### B4. A replaced fail also allows `overridden` (recommended)

**Rule.** The basis stays evidence-backed. The objection allows
`overridden` **as well** only when, in `E`'s past, a pass replaced the
failing result. That is: the validator has later eligible results for that
manifest and method, and its latest is a single pass. This matches green's
rule that several concurrent latest results count as a fail, even if all
passed (findings `32833dfc442b`, `91b445207f20`). A fail replaced by another fail, or standing beside a
concurrent latest fail, stays binding exactly as today: `accepted_breakage`
is its only answer for a tree used unchanged. When B4 applies, the
objection's allowed set becomes `accepted_breakage` and `overridden` for a
tree used unchanged, and `fixed`, `fell_back`, `accepted_breakage` and
`overridden` for one in a decision's window, as `Basis::allows` gives today
plus `overridden`.
The answer's `reason` is still required, and stays on record.

**Changes.** Rule 5's table (pinned; weave.md 575-593, `Basis::allows` and
`allowed`, `objections.rs` 57-65, 650-680). It needs to know whether the
result was replaced by a pass, the same question as B1. No v2, no cutover.

**Costs.** As B1's ancestry question. `scripts/weave-shadow`'s
`overridable` must also accept such an objection by a non-member, so the
shadow's mirror moves on as it does for discussion. `weave review`'s and
`weave integrate`'s output should say "replaced" next to the basis (B6).

**Breaks.** Nothing that qualifies today. B4 only adds an allowed answer,
so every decision that qualifies under today's rule qualifies under B4 (it
widens and never narrows). Nor does it widen what an integrator can wave
through past a tree that fails: where the validator's latest results in
`E`'s past still hold a fail, nothing changes. The counterexample in
finding `32833dfc442b` (a fail, `--again`, a second fail, an honest
reviewer objecting from the first) stays binding. An old reader judges a decision that answers
`overridden` this way as unqualified, so heads differ between binaries.
Readers must be upgraded before an integrator uses it, which here means the
pinned `.bracken-bin` and `~/bracken-weave`'s binary. Stale replicas,
offline writers and per-agent keys are unaffected.

**Residual.** The integrator has to answer, once per target: one answer
naming the manifest artifact covers every objection to it (weave.md section
2, *One answer can cover many objections*). That's the cost every
discussion objection already has. A replaced fail can't block a tree.

#### B5. Accept and document (status quo)

**Rule.** No change. Decision `a2e0b9c241c7` accepted it for now.

**Residual.** Any writer can block any tree a validator ever flaked on,
until the tree changes or the policy is bumped (2.2). It's worst under
`weave-shadow`, which mirrors commits as git made them and can't change the
tree. After the join, every writer of the coordination Ground can do it.

#### B6. Show it (tooling, with any of the above)

**Rule.** `weave review` and `weave green --manifest` list objections to a
manifest artifact, with their basis, and say when an evidence-backed one
rests on a replaced fail. That's the residual 2.5 records for finding
`598258717737`.

**Changes.** CLI output only, which isn't frozen (weave-freeze.md 1.7). It's
the security-sensitive class for `crates/bracken/src/weave*` (G6). It
fixes nothing alone, but today the integrator gets no warning.

### 2.4 Comparison

"Replaced" here means replaced by a pass: the validator's latest results
for that manifest and method, in `E`'s past, are a single pass. A fail
replaced by another fail, or by concurrent passes, stays binding under every
option.

| Option | A replaced fail blocks a tree | Honest objections kept | Tier | Existing decisions | Main cost or residual |
|---|---|---|---|---|---|
| B1, latest result | no | yes | pinned | an `accepted_breakage` answer to one may unqualify | readers-first; one ancestry question per result |
| B2, members only | by members | no: a reviewer's evidence becomes discussion | pinned | as B1 | a new author rule |
| B3, B1 unless its validator objects | by its validator | yes | pinned | as B1 | one more rule, little gained |
| **B4, also allow `overridden`, after a pass** | **no** | **yes: a fail still latest stays binding** | **pinned** | **unchanged (it only widens)** | readers-first; `weave-shadow` follows |
| B5, accept | yes | yes | neither | unchanged | any writer, any flaked tree |
| B6, show it | yes, but visible | yes | neither | unchanged | complements the others |

### 2.5 Recommendation

**B4 with B6, before the join.** B4 restores what the latest-result rule
is for, that a validator's later pass decides the manifest, without moving
any existing verdict and without making the author of an objection matter.
It keeps the objection, its basis and its answer on record, so the
integrator's `overridden` is visible and has a reason, rather than the
basis silently changing as under B1. It can share a PR with A5: both are
pinned `crates/tree/src/` rule changes with tests beside the existing
ones. B4 needs readers-first, and A5 doesn't.

## 3. Tiers and rollout

- **No recommended option changes a frozen format or rule.** A5 changes G8's
  reach, and B4 changes rule 5's table. Both are derivation rules in
  `lines.rs` and `objections.rs` that weave-freeze.md 1.6 pins and doesn't
  freeze. Neither changes which records are valid, the ancestor rule, the
  null-`owner_decision` rule or the marker that counts (1.3). So neither
  needs the owner's own go under question 3, or a v2 under section 3, or the
  cutover.
- **Class:** protocol, since both touch `crates/tree/src/`. That means the
  full table, an `owner-review` by the manager naming the pr claim and head,
  an independent review and a security review, and a releases.md entry.
  `scripts/code-scope frozen` lists the paths, and `scripts/merge` checks
  them (G6).
- **Rollout.** A5 can ship in any order, because it changes no verdict and
  an old binary fails closed. B4 is readers first: the release that reads it
  goes to every Weave reader before any integrator answers a replaced-fail
  objection `overridden`. Until then `weave integrate` should refuse to
  write such an answer. Today that's the pinned binary and
  `~/bracken-weave`'s binary, and after the join, the coordination Ground's
  readers.
- **A3, if chosen,** is pinned too, with readers-first, and changes
  qualification. Before it lands, its build task checks whether any Weave
  Ground we hold removed an integrator who had a concurrent decision.

## 4. Questions for the owner

1. **A: narrow G8 to the current policy (A5)?** An unreadable decision, and
   a decision that brings other lines into a judgement, count only if their
   author integrates the line under its current policy as well as in their
   own past. *Recommended: yes, before the join.* Removing an integrator
   then ends any stop they started and takes away the power to start one,
   for every reader that holds the removal. It changes no verdict and no
   frozen rule, and needs no v2 or special rollout. **One exception:** if an
   honest integrator has merged into main a line the removed integrator
   authors, or still integrates, an unreadable version on that line still
   stops main. Removal from main ends their power to stop it only if no such
   line has been merged in. That's the route you already accepted, "main's
   integrator merging a hostile line that can then stop main" (decision
   `a2e0b9c241c7`). Also left: question 7's accepted fork, and the corner in
   1.3, A5. And it assumes every reader upgrades before any v2 decision is
   written. A v1 reader that missed the upgrade would ignore a v2 decision
   by a since-removed integrator rather than refuse.
2. **A: revoke a removed integrator's token at the team server (A4)?**
   *Recommended: yes, from the join, as a step in removing a Weave
   integrator.* It narrows what a removed integrator can still sign to what
   they signed before and withheld. It does nothing before the join, since
   Weave's keys are files.
3. **A: a removal cut for rule 2 (A3)?** A decision concurrent with a policy
   version that drops its author wouldn't qualify, which ends question 7's
   stale-past fork for removals. *Recommended: not now. Design it with part
   B's revocation cut (agent-keys.md 4.4), or before the join if you want
   removal to mean revocation from the start.* It reopens what question 7
   accepted. It's the first case where a later policy change unqualifies a
   decision, and a second cut rule beside part B's would have to agree with
   it.
4. **B: let an objection be answered `overridden` when its fail was
   replaced by a pass (B4), and show it (B6)?** That means the validator's
   latest results for that manifest and method, in the decision's past,
   are a single pass. A fail replaced by another fail, or by concurrent
   passes, stays binding.
   *Recommended: yes, before the join, with `scripts/weave-shadow`
   following.* It ends the replay without moving any existing verdict, and
   it keeps honest evidence objections binding while the tree still fails. A
   validator who thinks its pass was wrong records a new fail, which is its
   latest.
5. **One PR or two?** A5 and B4 are both pinned rule changes in
   `crates/tree/src/`, with the same review path. *Recommended: one
   protocol-class PR for both, with B6 and the `weave-shadow` change in it
   or straight after.* B4's readers-first step is a release note that
   says to upgrade both Weave binaries before answering such an objection.
   Today one upgrade does that.
