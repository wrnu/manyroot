# The next experiment's task

> **Proposal, 2026-10-28.** Task `465f182cbd1f`, the second half of the
> owner's decision `885847a45a8f`: design a harder or real task for a later
> experiment, because Bracken and CRDT reached 30 of 30 in every Phase 1
> replicate and the toy task may not separate the modes
> ([phase1-results.md](phase1-results.md)). No run is proposed here: each run
> needs the owner's own go on its stated use.
>
> **Accepted, 2026-10-28** (decision `a4d8622c75d5`): Candidate A with
> Candidate C's open contracts and interacting changes, without C's per-agent
> release. Build task `b7c49e317b0e`; [Step 1 as built](#step-1-as-built)
> records what the first build step settled.

## What the next task has to fix

Phase 1's `stacklib` had 30 small functions in four layers. Three things limit
what it can show:

- **It saturates.** Bracken and CRDT ended at 30 of 30 in all three
  replicates, and Git in one of three. Final quality can't separate modes
  that all finish.
- **Its conflicts are few.** Six change requests, released together, gave
  CRDT 32 to 47 breaks in the library, and Git and Bracken none. No mode had a
  semantic-conflict event. The task never forced agents to agree on something
  the spec leaves open.
- **Its code is tiny.** A module is a few lines, so reading the state is cheap
  in Git and CRDT. Real code would make every mode's reading cost matter.

The candidates below each target some of these. Token estimates start from
Phase 1's measured use per agent-turn, the mean over three replicates:

| Mode | Tokens per agent-turn | Output per agent-turn |
|---|---|---|
| Bracken | 184,000 | 4,400 |
| CRDT | 64,000 | 2,300 |
| Git | 48,000 | 1,400 |

Bracken's figure includes the costs that
[phase1-bracken-lessons.md](phase1-bracken-lessons.md) traced to missing
views and to the harness. With those fixed it should fall, but how far is
unmeasured, so the estimates give both. **"With the fixes"** below assumes
Bracken's status view and incremental, bounded log in Bracken mode's tools,
and notes saved after the call cap. It assumes they remove about half of
Bracken's tokens per turn, the share that went on repeated full reads, too-large
results and re-reading after lost notes. That half is an estimate, not a
measurement. Git's and CRDT's figures don't change with the fixes.

## Candidate A: a larger `stacklib` with real dependencies

**What:** the same language and harness, with about 90 functions in 8 modules
and 5 layers instead of 30 in 4. Every function above layer 1 calls two or
three functions from at least two lower modules, so a change in one module
reaches several. Two waves of change requests, at 30% and 60% of the turns,
six in each. Some change the signature of a function that 10 to 15 others
call, so many callers must follow.

**What it measures that Phase 1 couldn't:**
- **No saturation in 8 turns.** Phase 1's Bracken and CRDT agents had all 30
  functions in place by about turn 7 of 8 (agents' notes in the pilot record
  all 30 accepted at turn 7). Three times the work shouldn't finish, so final
  quality and improvements per token separate modes that finish at different
  speeds.
- **Ripple:** an interface change with many callers tests how each mode
  carries a change through dependent work, which is where semantic
  incompatibility comes from.
- **Scale of state:** three times the contributions, module text and history,
  so reading costs grow as they would in a larger project.

**Grading, without leaking the answer:** exactly as Phase 1. Hidden cases
(2,000 seeded draws plus edge values) run only in the grader and never reach a
model. Public cases are the only thing `check` runs. Each function is scored
in library and standalone. Nothing new.

**Sandboxing:** none new. Agent code still runs only in the harness's
`stacklib` interpreter, in Rust, with step and depth limits.

**Expected use per agent-turn:** Git and CRDT read module text and history that
triple in size, so about 1.3 to 2 times Phase 1: Git 60,000 to 90,000 and CRDT
80,000 to 130,000. Bracken without the fixes, 250,000 to 400,000. Its log
already overflowed the model's tool-result limit in 23% of calls at Phase 1's
size, so a larger Ground makes that worse. With the status view, an
incremental log and notes after the cap, Bracken should fall to 120,000 to
200,000. Per 10-agent, 8-turn replicate:

| | Git | CRDT | Bracken | Replicate |
|---|---|---|---|---|
| Without the fixes | 4.8 to 7.2 M | 6.4 to 10.4 M | 20 to 32 M | 31 to 50 M |
| With the fixes | 4.8 to 7.2 M | 6.4 to 10.4 M | 9.6 to 16 M | 21 to 34 M |

Without the fixes, Bracken alone could pass its 25 million share of today's
75 million cap and stop early. With them, it fits every cap
([Against every cap](#against-every-cap)). Turns stay
at 8: the larger task supplies the extra work, not more turns.

**Build steps (no model calls):**
1. Specs, reference implementations in Rust, two correct and two subtly wrong
   candidates each, and public and hidden cases for about 90 functions, in
   `crates/experiment/src/task.rs` and `grader.rs`, with the same tests as
   Phase 1's step 1.
2. Two change waves, and the driver's release of the second wave.
3. Mock-backend scripts for the larger task, and a full mock run of all three
   modes.
4. The modes' "where things are" sections updated if anything they name
   changes, with the neutrality review again.
5. The experiment-side fixes below that the owner or the manager accepts.
6. A calibration run (2 agents, 2 turns, all modes), with the owner's go.

## Candidate B: a small real Python library with a held-out test suite

**What:** a pure-Python library with no dependencies beyond the standard
library, about 25 functions in 5 modules. An example is integer interval sets:
parsing, normalising, set algebra and formatting. Agents write real source
files and run the public tests. The design already names this as the second
tier (decision `b29d5db02da8` point 4, design section 9).

**What it measures that Phase 1 couldn't:**
- **Real code:** files of thousands of characters, so reading costs in Git and
  CRDT become real too, and Git's conflicts are real text conflicts.
- **The vision's case:** agents building software, not a toy language, with
  real tests as the check.
- **Bracken's unit of work for real code:** whether an artifact is a function
  or a file is a design question this run would have to settle first.

**Grading, without leaking the answer:** a hidden test suite (cases and
property tests) that only the grader holds, outside every agent's copy. Agents
see only the public tests. Standalone scoring substitutes reference modules
for a function's callees through import hooks, so a function is judged on its
own code. Writing a good hidden suite is the largest part of the work.

**Grading runs agent code too, so it is sandboxed like the tools.** Phase 1's
grader runs inside the driver, which can read the owner's routing settings.
That was safe only because `stacklib` programs never leave the harness's own
interpreter. For B:
- the grader never executes agent code itself. It starts a separate process
  for the code under test, under the execution profile below (or a stricter
  one), with a cleared environment as the workers have: no routing entries, no
  `HOME` but its own scratch directory;
- the held-out tests stay in the grader's own process. It sends the child one
  case's inputs at a time and gets back only the value or the error, then
  compares in its own process. Expected outputs, property tests and the
  reference implementation never enter the child, so agent code can't read or
  introspect them;
- the child's scratch directory is fresh for each graded version and discarded
  after, so nothing agent code writes survives into another run or reaches an
  agent.

**Sandboxing: new, and the main cost.** Agent-written code executes when
tests run, so the harness needs an execution profile. It would include:
- a pinned interpreter (path and hash, as Claude Code is pinned), started with
  `-I -S -B`;
- reads only of the copy under test and the standard library, and writes only
  to a scratch directory made fresh for each run;
- no network and no process started but the interpreter itself;
- limits on CPU time, memory and output.

That profile is a new risky surface, for the public-test runs the agents ask
for and for grading alike. It needs its own step-6-style tests (each forbidden
access tried from inside it, on both paths) and security review. Rust instead of
Python would add compile time to every check, and build scripts to sandbox, so
Python is the cheaper choice.

**Expected use per agent-turn:** Python modules are several times larger than
`stacklib`'s, so every read costs 3 to 5 times more: Git 145,000 to 240,000,
CRDT 190,000 to 320,000, and Bracken 550,000 to 920,000 without the fixes
(275,000 to 460,000 with them). Per 10-agent, 8-turn replicate that is 71 to
118 million tokens without the fixes and 49 to 82 million with them. It
doesn't fit today's caps (see [Against every cap](#against-every-cap)), so B
needs a change the owner decides: fewer agents or turns, or larger caps.

**Build steps (no model calls):**
1. The library's specification, reference implementation, public tests and
   hidden suite.
2. The execution profile and its runner, used both for the agents'
   public-test runs and by the grader, with tests of each forbidden access on
   both paths (including that the child can't read the hidden suite, the
   grader's memory or the routing settings, and gets a cleared environment),
   under security review.
3. File tools for each mode (read, write, run the public tests), with
   argument rules like Phase 1's, and Bracken's unit-of-work decision.
4. The grader with standalone substitution, and its tests.
5. Prompts and their neutrality review, and mock-backend scripts and a full
   mock run.
6. A calibration run, with the owner's go.

## Candidate C: Phase 1's task with deliberate semantic conflicts

**What:** the 30-function `stacklib`, with three additions aimed at conflicts:
- **Open contracts:** two or three shared conventions the specs leave to the
  agents, such as how a pair of values is packed into one integer. Producers
  and consumers in different modules must agree, or the library is wrong even
  though each function looks right alone.
- **Interacting changes:** pairs of change requests whose obvious fixes
  conflict unless the agents settle a shared contract.
- **Changes seen first by a few:** each change request reaches one or two
  agents first and the rest only through the mode, so it has to spread.

**What it measures that Phase 1 couldn't:**
- **Semantic-conflict events,** which Phase 1 never produced.
- **How each mode detects and settles a disagreement:** time to agree, and
  breaks in the library on the way.
- **How a change spreads** through each mode's channel.

**Grading, without leaking the answer:** hidden cases as now. Open contracts
are graded by hidden cross-module tests (round trips, and producer against
consumer) that hold for any consistent choice, so no single answer can leak.

**Sandboxing:** none new.

**Expected use per agent-turn:** close to Phase 1, plus some for the extra
coordination: about 1.1 to 1.3 times, so 26 to 31 million per replicate
without the fixes. That fits every cap ([Against every cap](#against-every-cap)).

**Build steps (no model calls):** open-contract specs and their hidden
cross-module tests in the grader; the interacting change pairs; a driver
change to release a change request to some agents first; mock scripts, a full
mock run and the neutrality review. Releasing changes to some agents first
departs from the design's rule that changes are released together at the same
point in every mode. That rule is the manager's (design section 1), but the
measures it affects were part of the design the owner accepted, so it goes to
the owner too.

## Against every cap

Today's caps, set for the pilot (design section 5, decision `ea053c2c4d48`):
per agent 3 million tokens and 180,000 output; per mode 25 million tokens and
1,666,666 output; per replicate 75 million tokens, 5 million output and 3
hours. Output is estimated at 1.3 to 2 times Phase 1's per turn for A, 2 to 4
times for B (more code written) and 1.1 to 1.3 times for C, with 10 agents
and 8 turns.

| Cap | A, without the fixes | A, with them | B, without the fixes | B, with them | C |
|---|---|---|---|---|---|
| Per agent, 3 M tokens (Bracken, the largest) | 2.0 to 3.2 M: may pass | 1.0 to 1.6 M | 4.4 to 7.4 M: passes | 2.2 to 3.7 M: may pass | 1.6 to 1.9 M |
| Per agent, 180,000 output (Bracken) | 46,000 to 70,000 | the same | 70,000 to 141,000 | the same | 39,000 to 46,000 |
| Per mode, 25 M tokens | Bracken 20 to 32 M: may pass | Bracken 10 to 16 M | Bracken 44 to 74 M and CRDT 15 to 26 M: pass | Bracken 22 to 37 M and CRDT 15 to 26 M: may pass | Bracken 16 to 19 M |
| Per mode, 1,666,666 output (Bracken) | 0.46 to 0.70 M | the same | 0.70 to 1.41 M | the same | 0.39 to 0.46 M |
| Per replicate, 75 M tokens | 31 to 50 M | 21 to 34 M | 71 to 118 M: may pass | 49 to 82 M: may pass | 26 to 31 M |
| Per replicate, 5 M output | 0.8 to 1.3 M | the same | 1.3 to 2.6 M | the same | 0.7 to 0.9 M |
| Per replicate, 3 hours | about 25 to 40 minutes | about 15 to 25 minutes | about 55 to 90 minutes | about 40 to 65 minutes | about 20 to 25 minutes |

What each would need:

- **A with the fixes** fits every cap as it stands. Without them, a Bracken
  agent may reach its 3 million and Bracken mode its 25 million, stopping
  early: a result the report would still measure, but not a fair one. So A
  should wait for the fixes, or the owner raises the per-agent cap to 4
  million and the per-mode share to 35 million.
- **B** needs the fixes and a change the owner decides. With the fixes and 6
  turns instead of 8, a Bracken agent stays under 3 million (1.7 to 2.8
  million). Per mode it then needs 6 agents (about 10 to 17 million for
  Bracken) or a larger share. Without the fixes, no setting short of a
  per-agent cap of about 8 million and a replicate cap of about 120 million
  fits. Fewer agents or turns changes the owner's 10-agent, 8-turn setup
  (decision `6b2bfb7ecd2d`); larger caps need the owner's go.
- **C** fits every cap as it stands, with or without the fixes.

The time row scales Phase 1's 17 to 20 minutes per replicate by each
candidate's tokens, and assumes no rate-limit waits.

## Recommendation

**Run A next, with C's open contracts and interacting changes folded in,**
after the Bracken changes and the experiment-side fixes below. Then B, as its
own design, once its execution sandbox has been built and reviewed.

- **A with C's additions** targets all three of Phase 1's limits but real code:
  no saturation, ripple from interface changes, and conflicts the agents must
  settle. It needs no new sandbox. With the fixes, it fits every one of today's
  caps.
- C's per-agent release is left out at first. It changes the design's release
  rule, and the open contracts already create conflicts.
- **B** is the vision's real case, but it costs 3 to 5 times more per turn and
  adds an execution sandbox that needs security review. It's better as a
  second step, once A has shown the modes apart on a task where the harness is
  already proven.

## Experiment-side fixes a next run needs

From [phase1-bracken-lessons.md](phase1-bracken-lessons.md), plus the tool
result limit it found. **Who decides** says whether the change touches a rule
the owner fixed (decisions `b29d5db02da8` as refined by `e8be1670f346`, and
`6b2bfb7ecd2d`),
which needs the owner's go (note `9c48f660b03c`), or is the manager's, as a
reviewed harness change.

| Fix | Why | Who decides |
|---|---|---|
| **Let `notes_write` through after the call cap** (one call, after the soft stop) | Bracken agents saved notes in 7 to 19 of 80 turns, Git in 64 to 71. A turn at the cap loses its notes, so the next turn re-reads everything | Manager: the cap is decision `147a36d5d623`. A reviewed step-6 change, equal in every mode |
| **Bound every tool result below the model's limit,** in every mode: page it, or refuse with a message saying how to filter | 146 of 635 Bracken `log` calls (23%) returned only Claude Code's too-large error | Manager: a reviewed step-6 change |
| **Give Bracken mode Bracken's status view and `log --since`,** once Bracken has them, in place of the harness's own `accepted` count | The run should measure Bracken's views, not the harness's | Manager for the tools, with the neutrality review (decision `4cf00e1d701d`). If it changes what counts, see the next row |
| **Match the acceptance rule to Bracken's default:** one independent pass, no "most passes" ranking, no pass counts in the tool | The harness's rule rewarded piling passes on accepted work (about 60% of passes) | **Owner:** it changes `b29d5db02da8` point 7 |
| **Accept shorter ids** (the shortest unique prefix, at least 8 digits) in Bracken mode's arguments | Agents re-read the spec list 134 times for full ids, and 25 calls were refused for short ids | Manager: an argument rule (design section 3), with security review |
| **Name the status view in Bracken's "where things are" section** | The prompt says what counts, not how to find work needing a pass | Manager, with the neutrality review |
| **Encourage a sync before choosing work** (prompt only) | A pass usually arrived a turn later. Syncing at turn boundaries and on demand stays as it is | Manager, with the neutrality review. Changing when the harness syncs would be **owner**: decision `b29d5db02da8` point 5 |
| **The task itself** (A, B or C) | This proposal | **Owner:** `b29d5db02da8` point 4 sets `stacklib` first and real code second; a new task and its run need the owner's go |
| **Agents, model and caps** | Unchanged for A and C. B may need fewer agents or a larger cap | **Owner:** decisions `6b2bfb7ecd2d` (10 agents), `b29d5db02da8` points 2 and 8, and each run's approval |

Git's merge rule (`b29d5db02da8` point 6) and the sync model (point 5) stay
as they are in every candidate.

## Step 1 as built

Build step 1 of the accepted task (task `b7c49e317b0e`) adds Phase 2's task
beside Phase 1's. The change waves are step 2's, in
[Step 2 as built](#step-2-as-built).

**A task selector.** `--task phase1` or `--task phase2` on `run` and
`mock-run`, with Phase 1's as the default. The plan records the task, and a
plan from before the selector reads as Phase 1's. Phase 1's task is its own
file (`crates/experiment/src/task/phase1.rs`), moved unchanged. A test pins
its task texts and each mode's tool list to the hashes in replicate 3's
plans, so Phase 1 gives agents byte for byte what its runs were given, except
for the wording of Bracken mode's `accepted` tool, which PR #55 changed along
with the acceptance rule. The
plan's measuring-code hash covers both tasks' files, so reporting a Phase 1
run needs the build that ran it, as before.

**The task** (`crates/experiment/src/task/phase2.rs`) has 90 functions in
eight modules, one layer per module:

| Layer | Modules | Functions |
|---|---|---|
| 0 | `base`, `logic` | 23 |
| 1 | `order`, `step` | 23 |
| 2 | `arith`, `range` | 24 |
| 3 | `shape` | 10 |
| 4 | `score` | 10 |

Layers are numbered from 0, as in Phase 1: layer 0 needs only the language.
So "above layer 1" in Candidate A means layers 2 to 4, and every function in
them is defined in terms of two or three functions from at least two modules
of lower layers. Layer 1's functions use layer 0's and, as in Phase 1, each
other (`nearer` uses `dist`). Twelve functions call
`clamp3`, so step 2 can change its signature for 10 to 15 callers. Function
names are new where Phase 1's would make one name start another (`ne`,
`negate`), because Bracken mode names an agent's helpers by prefix. As in
Phase 1, each function has a reference implementation in Rust, two correct
and two subtly wrong candidate programs, 60 public cases (13 for one input)
and the same hidden cases.

**Two open contracts,** each with five members in two modules:

- `rank`: `rank(a, b)` says how `a` compares with `b`, and `isless`,
  `issame`, `ismore` and `flip` read that value. The library chooses the
  encoding.
- `none`: `cut(a, b)` gives `a - b` or none, and `pos(a)` gives `a` or none.
  `isnone`, `either` and `plus` recognise none. The library chooses a value
  outside 0 to 4000.

A member has no reference implementation, because no single answer is right.
Thirteen properties grade the members instead, on inputs from −1000 to 1000
as the specs state. Each property relates two to four members, as a round
trip or as a producer against a consumer, for example
`isless(flip(rank(a, b)))` is 1 exactly when `a > b`, and
`either(cut(a, b), d)` is `a - b` when `a >= b` and `d` otherwise. One
property checks the spec's "one none, the same everywhere" rule: the none from
`cut` equals the none from `pos`, and lies outside 0 to 4000. A
property holds for every consistent choice, so passing doesn't reveal any
particular encoding. A test shows two different encodings both score in
full, and that producers of one with consumers of the other fail every
member. A member is graded by every property that calls it:

- The hidden cases are the property's public cases, 2,000 seeded draws and
  every combination of the range's edge values.
- Standalone, calls to other members resolve in the library, never to an
  oracle, since a member is right only together with its partners.
- An agent's `check` of a member runs the public cases of its properties.
  It reports the failing expression, such as
  `isless(rank(2, 5)) gave 0, expected 1`.
- A delivery of a member is judged by one rule (decision `979e5085a1e7`,
  after findings `e36915fb7841`, `1d63e2d4c3a0`, `672eb5c533d8`). The member
  is judged with the other members in the delivering agent's own working
  copy at the moment the delivery becomes available to others. Members the
  copy doesn't hold come from the integrated library. Each mode's working
  copy is reconstructed from its records:

  | Mode | When it becomes available | The working copy |
  |---|---|---|
  | Git | the commit | the branch's members at the commit, which a commit's record lists (open-contract tasks only) |
  | Bracken | the agent's next sync | each member accepted at the hub; for the rest, any of the agent's own current implementations, which its records list after each artifact, relate, claim or retract (open-contract tasks only) |
  | CRDT | the sync | the document after the sync: the hub's snapshot then |

  Bracken's order is the one the agent's own `check` resolves calls in
  (answer `88ae128c5fcd`): accepted first, then its own. An agent can hold
  several current implementations of one member, so the delivery counts as
  correct if it agrees with any one choice among them. What each mode's
  logs didn't hold before, and why Bracken's order is accepted-first, is in
  note `ddf2368aa636`. One table-driven metrics test runs every case in all
  three modes: a whole contract at once, adapting after losing and keeping
  the losing convention, a revision in a later turn, a contract split over
  two turns, a same-turn rewrite, a release between delivery and verdict, a
  mixed-convention contract, and a contract accepted from another agent
  while this agent's other-convention one is pending. The last is wrong in
  Bracken only, because in Git's branch and CRDT's document the agent's own
  work is its copy, and its own check would pass there too.
- A verdict that blocks a definition is judged under the specs in force at
  the verdict (finding `440196ce733f`), and a member with its latest
  delivery's working copy.
- Phase 1's measures are unchanged. All 16 recorded Phase 1 runs measure
  identically with main's metrics and this build's. A test measures Phase 1
  mock logs made by main's build and gets main's measures to the byte. Phase
  1's records gain nothing.
- A bug report names a function with a reference, so it never names a
  member.

The mock runs Phase 2 in every mode (a test does, with 2 agents and 2 turns).
The scripted moves and a full mock run of the larger task are step 3.

## Step 2 as built

Build step 2 (task `8d4ee9d1f345`) adds Phase 2's change requests and their
release, with no model calls. Phase 1 keeps its single release exactly as
it ran.

**Two waves of six,** at 30% and 60% of the turns. A wave goes out once
`share × turns` turns are used, rounded to the nearest turn with halves up,
the same way in every mode: with 8 turns, after turns 2 and 5. Wave 2
changes what wave 1 changed (`clamp3`'s inputs, twice).

| Wave | Tag | Spec (module, layer) | Kind | Under the change |
|---|---|---|---|---|
| 1 | `clamp-order` | `clamp3` (order, 1) | signature | `clamp3(lo, hi, x)` |
| 1 | `sign-of-zero` | `sign` (base, 0) | behaviour | `sign(0)` is 1 |
| 1 | `between-exclusive` | `between` (order, 1) | behaviour | `lo <= x < hi` |
| 1 | `cut-signed` | `cut` (arith, 2) | interacting, with `none-wide` | `a - b` whenever it is from −1000 to 1000, so none lies outside −1000 to 4000 |
| 1 | `none-wide` | `isnone` (range, 2) | interacting, with `cut-signed` | none lies outside −4000 to 4000 |
| 1 | `weigh-subtracts` | `weigh` (shape, 3) | behaviour | `gap − count3` |
| 2 | `clamp-hi-first` | `clamp3` (order, 1) | signature | `clamp3(hi, lo, x)` |
| 2 | `rank-wide` | `rank` (order, 1) | interacting, with `ismore-far` | inputs from −9×10¹⁸ to 9×10¹⁸ |
| 2 | `ismore-far` | `ismore` (step, 1) | interacting, with `rank-wide` | 1 if `a > b + 2` |
| 2 | `nearer-ties` | `nearer` (order, 1) | behaviour | ties go to `q` |
| 2 | `abs-saturates` | `abs` (base, 0) | behaviour | `abs(min)` is the largest integer |
| 2 | `mix-subtracts` | `mix` (score, 4) | behaviour | `tilt − clip` |

- **Both signature changes are `clamp3`'s.** Of the 90 functions, only
  `clamp3` has 10 to 15 callers (12; the next most called has 6), so both
  waves change its inputs' order and its twelve callers must follow each
  time. The other descriptions still write `clamp3(x, lo, hi)` for the
  value, as the change texts say, so a caller's contract never changes.
- **Two interacting pairs, one per wave.** Each change alone has an
  obvious fix that breaks the other:
  - `cut-signed` makes −1, the obvious none, a difference. Its obvious fix
    moves none just past cut's range (−1001, say), which `none-wide` rules
    out. `none-wide`'s obvious fix moves none far out but leaves `cut`
    unsigned. Only a library that agrees on both, such as none at −5000,
    is right.
  - `rank-wide` makes `a - b` overflow. Its obvious fix reads only the sign
    (`-1`, `0`, `1`), which can't say whether `a` is more than 2 above `b`.
    `ismore-far`'s obvious fix reads `a - b > 2`. The library must settle a
    value that keeps the distance up to 3 without overflowing, such as
    `±3` beyond 2 apart, `±1` within 2, and 0.

  A test plays each obvious fix and shows it right alone and wrong with its
  partner, while the settled convention is right under both.
- **The properties follow the specs in force.** The none properties take
  `cut`'s new validity and none's new range; the one-none property calls
  `cut(-1000, m)`, which is none under every version. Rank properties take
  three inputs. Once `rank-wide` applies, `a` and `b` span the wide range,
  and for half the cases `b` is within 3 of `a`, so `ismore`'s margin is
  tested. Public cases stay small, so `a - b`, which only overflows on wide
  inputs, passes the public check and fails hidden cases.
- **Each change has** its spec's new description, a version-aware
  reference, two correct and two subtly wrong candidates for its target
  (the last passes the public check), the updates the correct library
  needs, and the same public and hidden cases. Hidden cases stay
  grader-only.

**The release.** The driver releases each wave to all agents at once, at
the same point in every mode: when `turn × agents` agent-turns are used. A
wave due after turn 0 goes out before anyone plays. Phase 2's
`changes_released` event names its wave. Phase 1's event names none,
since it has one release, after 40% of the agent-turns as before. Each
mode's `release_changes` takes the wave:

- Bracken writes that wave's change claims, each with a spec claim that
  supersedes the current one.
- Git and CRDT write `TASKS.md` at the version after that wave, listing
  every change released so far.

The plan records the turns after which the waves go out (`waves`), and
the task text's hash after wave 1. A Phase 1 plan has neither.

**Phase 2 ranks the modes at checkpoints** (owner decision
`288a16ce8cb2`). A checkpoint is the end of a turn index, once every agent
has finished that turn. The driver then logs the hub's integrated library
as it is at that moment: Git's main, Bracken's accepted layer, CRDT's hub
document. Phase 1's logs gain nothing. Each checkpoint is graded with the
same grader in every mode, under the change requests released before it.
A function counts as correct when it is right in the library; the
standalone count is kept beside it. From the checkpoints, the same in
every mode:

- **Final quality:** the functions correct at the last checkpoint.
- **Time to correct:** per function correct at the last checkpoint, the
  turn of the first checkpoint from which it is correct at every one.
- **Recovery per wave:** of the checkpoints after the wave's release, how
  many pass until the wave's functions that were correct in the hub when
  the wave went out are all correct again, or never if not by the last.
  If none was correct then, there is nothing to recover and it reads n/a.
  "When the wave went out" is the hub's last snapshot before the release,
  graded under the specs in force before it: the same point in every mode,
  with no turn barrier (decision `a8ba06399068` point 4, task
  `81b0ce075a56`). Before, it was the last checkpoint before the release,
  which can be a turn stale when agents are on different turns. It isn't
  the snapshot logged after the release: in Bracken mode the release
  itself drops the implementations of the specs it supersedes from the
  accepted layer, and that loss is part of what the wave has to recover.
  The report prints the count beside each wave: `2 (of 5)`, `never (of 5)`,
  `n/a (of 0)`. A wave's functions are its targets, every function defined
  in terms of them (directly or not) and their open contracts' members.
  Step 3 narrowed this from all of the wave's functions, because the ones
  not yet written made every run read never (note `ba986b386c7a`).
- **Breakage:** a function correct at one checkpoint and wrong at the next.
  It counts as after a wave if a wave went out between the two, else as
  other: the semantic-conflict signal.
- **Tokens per correct function:** the run's tokens over the final count.
- **Beside the ranking, never in it: CRDT with identical copies
  collapsed** (decision `a8ba06399068` point 3, task `81b0ce075a56`). The
  grader counts a name defined more than once as missing, in every mode,
  and that rule stays. Only CRDT's shared document can hold several copies
  of a definition, so the report also prints CRDT's checkpoints regraded
  with copies identical to the first (same name, arity and code, in any
  module) counted as one; copies that differ still count as duplicates.
  It's labelled a diagnostic and reads n/a for Git and Bracken.

These are the report's ranking rows.

**Why checkpoints** (decision `288a16ce8cb2`). Judging individual
deliveries needs each mode's working copy and specs at the moment of each
delivery. Four review rounds of PR #62 (findings `06c729ef956b`,
`a3535d9c71b7`, `0b115f0c0ec0`) showed those points differ by mode in
ways that keep shifting credit between modes when a wave lands mid-turn,
which is routine with no barrier between turns. Grading the shared state
at one moment, the same for every mode, needs no such reconstruction.

**Per-delivery measures are per-mode diagnostics.** Discovery,
duplication and semantic conflicts are still reported below the ranking
rows, labelled as per mode only. They are not comparable across modes and
never rank them. They judge each delivery by the rule the four review
rounds settled (decisions `979e5085a1e7` and `5d049d7e4996`, findings
`06c729ef956b`, `a3535d9c71b7` and `0b115f0c0ec0`):

- **One judging point per delivery:** the delivering agent's last sync
  before it. For CRDT that is the sync before the one that delivered it.
  The specs come from that point, and so does the hub part of the working
  copy: the hub as that sync left it.
- **Each mode's copy of the other members:**
  - Git: the branch's members at the commit, over main as of that sync.
  - CRDT: the agent's own edits that the sync delivered, over the hub at
    its sync before.
  - Bracken: each member accepted at the hub, else any of the agent's own
    current implementations from its records before the sync that makes
    the delivery available. So a batch of artifacts delivered at one sync
    stays together (finding `e36915fb7841`).
- **Verdicts:** judged under the specs as of the verdict-giver's last sync.
- **Phase 1:** keeps the specs in force at each point, as its runs were
  measured.

A test checks, for every delivery in every case of the rule's table, that
the specs and the hub come from the same point. Bracken's store records an
agent's own members after every sync too (note `6b8e8ce21f94`), since a
release that supersedes a member's spec makes the agent's implementations
of it no longer current at its next sync and no tool call records that. A
store-level test plays artifacts, relates, syncs and both waves against a
real hub. Past 256 combinations of an agent's current implementations, the
search keeps the newest.

The rule's remaining differences between modes, without any wave:

- An unmerged Git branch is judged on its own content. Keeping the losing
  convention on it scores right and adapting one member to the winner's
  scores wrong, the reverse of Bracken and CRDT.
- Another agent's accepted member, against this agent's pending whole
  contract in the other convention, scores wrong in Bracken only.
- A contract split over two commits is judged per commit in Git, while
  Bracken and CRDT deliver it whole at one sync.

These are why the diagnostics don't rank the modes. Phase 1's measures are
as recorded: its headline stays per delivery, its logs are byte-identical
to main's, and it gets no checkpoint table.

## Step 3 as built

Task `07cb4ada01ea`. Phase 1's mock is unchanged: the same tool calls and
the same random draws, so its logs stay byte-identical to main's.

**Scripted moves** (`mock::Moves`). In Phase 2 each mock agent remembers
what it wrote, and each turn first rewrites up to three of its own
functions whose program isn't the one it now intends:

- **Following the waves:** after a wave, the agent's functions that it
  changes, clamp3's callers included, get the new spec's program.
- **Unsettled pairs:** every fifth agent writes each interacting pair's
  obvious fixes together (`candidates::phase2::UNSETTLED`). Each is right
  under its own change and wrong once both apply.
- **Breaks:** with nothing to rewrite, an agent sometimes (one turn in ten)
  replaces a right function with a wrong one, and mends it the turn after.
- **Each mode's way of rewriting:** Bracken writes a new artifact and
  retracts the old one. Git commits to the function's open branch, or to
  one new branch from main for the turn, with a PR. A merge conflict keeps
  this branch's side of a function both sides define. CRDT replaces the
  line. A CRDT agent also treats a function whose line the document no
  longer holds as unwritten, because concurrent edits of one line can
  corrupt it.
- **Spread:** agent k takes the lowest unwritten of every fifth spec from
  k, so two agents share each share.

A test plays two agents one agent-turn at a time (`Backend::lockstep`, for
tests only) and checks each mode's checkpoint rows and measures against a
hand-worked trace.

**Completeness** (finding `bf3fde3cf637`). A checkpoint that isn't a
module list is an error, as a bad snapshot is. A Phase 2 run with a turn
that has no checkpoint is incomplete and isn't measured. The exception is
a stop the plan expects. It can catch the agents spread over several turns,
and any turn that some agent never ended has no checkpoint and needs none.
A turn every agent ended still needs its checkpoint.

**A checkpoint and a release** (note `5b5757358523`). A checkpoint reads
the hub's library and logs it as one step under the hub's lock. A release
does the same with its change and its entry. So no wave goes out between a
checkpoint's read and its entry, and each checkpoint is graded under the
specs it was read under. A test pauses a checkpoint between the two while
another agent's turn makes a wave due, and the release waits.

**A wave at its boundary** (note `65049c2f8795`). Counting an agent-turn,
its checkpoint and the release it makes due are one step under one lock.
So no other agent-turn is counted until that wave has gone out, and each
wave goes out at exactly its boundary in agent-turns in every mode. That
holds even when the agent that reached the boundary waits on a busy hub.
Before this, another agent could end a turn first and release the wave one
agent-turn late. Meanwhile its own syncs were missing the wave, and how
late depended on each mode's hub load. No agent ever started a turn after
a boundary without the wave, because every agent passes the release check
between its turns. A test stalls the agent at the boundary while another
ends a turn.

**The full mock run:** 10 agents and 8 turns in each mode, sandboxed, with
no turn errors. The reports are recorded as evidence on the step's PR.

## Calibration as measured

Task `bb821dfc153c`, owner approval `a0e3b33bba1b`, report `95792fab29ce`,
2026-10-29. Opus 5.5 ran the Phase 2 task with 2 agents and 2 turns in each
mode, run as Phase 1's step 8 was: the pinned Claude Code, the LLM proxy,
the sandbox and the caps, with a fail-closed watcher beside each mode. The
drawn order was crdt, Bracken, git. It was clean: every turn passed the gate,
with no turn error, sandbox denial or rate-limit wait, and a checkpoint for
every turn.

Every turn reached the 25-call soft stop and then ended with its own
`result` event. So, unlike Phase 1's calibration, output is measured, not
counted at the 30,000 ceiling.

| Per agent-turn | crdt | Bracken | git |
|---|---|---|---|
| All tokens | 108,000 | 169,000 | 148,000 |
| Cache reads | 91,000 | 134,000 | 130,000 |
| Cache writes | 12,700 | 31,700 | 13,600 |
| Output | 4,650 | 3,900 | 4,750 |
| Model calls | 11.5 | 8.3 | 13.8 |
| Largest context of one call | 16,300 | 43,600 | 16,900 |

The three modes counted 1.70 million tokens in all, 53,000 of them output,
and took 98 to 142 seconds each. Tokens per agent-turn fall inside
candidate A's estimate above (about 125,000 to 200,000 with the fixes),
crdt below it. Output is below the estimate.

**The 10-agent, 8-turn pilot, re-estimated** against today's caps. The
input side runs from flat to doubled by turn 8. Phase 1's pilot grew
Bracken's input per turn about 1.7 times over its calibration.

| Cap | Re-estimate | Within it? |
|---|---|---|
| Per agent, 3 M tokens | Bracken 1.35 to 2.7 M | Yes |
| Per agent, 180,000 output | 31,000 to 38,000 | Yes |
| Per mode, 25 M tokens | Bracken 13.5 to 26.8 M, git 11.8 to 23.3 M, crdt 8.7 to 17.0 M | Bracken may reach it at the top |
| Per mode, 1,666,666 output | 0.31 to 0.38 M | Yes |
| Per replicate, 75 M tokens | 34 to 67 M | Yes |
| Per replicate, 5 M output | about 1.1 M | Yes |
| Per replicate, 3 hours | about 20 to 30 minutes | Yes |

So only Bracken's per-mode share is at risk, and only if its input per turn
nearly doubles. A Bracken run that reaches the cap stops, and its report is
measured from what exists.

## What this can't settle

The token estimates scale Phase 1's measurements and are not measured. The
first calibration run of whichever task is chosen replaces them. Whether the
Bracken fixes bring its use near the other modes' is also unmeasured until a
run with them. The candidates still use one model and one prompt, so they say
nothing about diverse agents (design section 4).
