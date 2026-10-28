# The next experiment's task

> **Proposal, 2026-10-28.** Task `465f182cbd1f`, the second half of the
> owner's decision `885847a45a8f`: design a harder or real task for a later
> experiment, because Bracken and CRDT reached 30 of 30 in every Phase 1
> replicate and the toy task may not separate the modes
> ([phase1-results.md](phase1-results.md)). No run is proposed here: each run
> needs the owner's own go on its stated use.

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

## What this can't settle

The token estimates scale Phase 1's measurements and are not measured. The
first calibration run of whichever task is chosen replaces them. Whether the
Bracken fixes bring its use near the other modes' is also unmeasured until a
run with them. The candidates still use one model and one prompt, so they say
nothing about diverse agents (design section 4).
