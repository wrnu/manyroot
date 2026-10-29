# The first Phase 1 experiment: 10 agents, three modes

> **Proposal, 2026-10-27.** Task `4d74467ebac0`, following the owner's
> decisions to start with 10 agents (`6b2bfb7ecd2d`) and to run the agents as
> headless Claude Code on this machine's existing setup, routed through the
> owner's LLM proxy (`060a6669d466`, `d8cce1147b05`, `e8be1670f346`). The
> owner accepted the first draft's open questions as recommended
> (`b29d5db02da8`); section 8 records the answers. Running real model-driven
> agents spends the owner's quota, so the manager tells the owner before each
> real run, with its expected use.
>
> **Results:** the calibration (step 8), the 10-agent pilot (step 9) and two
> more replicates ran on 2026-10-28; their numbers are in
> [phase1-results.md](phase1-results.md). Three replicates per mode show
> consistent directions, with one model, one prompt and a toy task, not
> proven effects.

[vision.md](../vision.md) asks for the same tasks, model budget and test
environment under three modes (Git branches and pull requests, CRDT shared
source editing, Bracken semantic contributions), measured six ways. This
proposes how to run that at 10 agents so that a run can be built, budgeted
and judged: what the agents work on, how each mode is set up, how the agents
are driven and kept apart, how each number is computed and why the agents
can't move it except by doing the work, what to build in which order, and
how much of the subscription one run uses.

## Summary

- **Task:** a larger version of the demo's stack language, `stacklib`: about
  30 functions in four modules that call each other, plus 6 change requests
  that arrive mid-run and alter the contracts of shared functions. Programs
  run only in a pure Rust interpreter with step and depth limits, so no agent
  code ever runs on the host, and a hidden grader can score any version of
  the library exactly.
- **Modes:** each mode gets the same task text, the same checker, the same
  private notes, the same hub-and-spoke sync, and the coordination its
  real-world counterpart has, nothing more: PRs and reviews in Git mode, a
  shared notes document in CRDT mode, claims and relations in Bracken mode.
- **Agents:** each agent turn is one headless Claude Code process
  (`claude -p`) on this machine's existing setup, through the owner's LLM
  proxy, with every built-in tool switched off: its only tools are the
  mode's, relayed to a separately sandboxed tool worker that validates every
  argument, and logged by the driver. Each process has its own throwaway
  home and config directory (none of the owner's hooks, skills, plugins,
  memory or settings), a fixed environment, and a macOS sandbox that keeps it
  out of the owner's files, repositories and credentials and lets it reach
  only the LLM proxy. 10 agents per mode, each 8 turns, each turn a fresh
  conversation.
- **Measurement:** computed by the harness from three sources the agents
  can't write to: the token usage Claude Code reports for every turn, the
  tool event log, and the hidden grader run on every version of the
  integrated library. Nothing an agent says about its own work counts.
- **Quota:** 27 to 47 million tokens (7.2 million of them output counted at
  the per-turn ceiling, an upper bound until real output is measured; most
  of the rest cache reads) and 30 to 45 minutes for one 10-agent pilot of
  all three modes on Claude Opus 5.5. The owner's go caps the whole pilot
  at 75 million tokens, 5 million output and 3 hours, which the driver
  enforces as a third per mode (with a raised share, the run order keeps the
  total: section 5, the caps); the output bound at the ceiling is above
  5 million, so a mode would stop at its cap first. The calibration run measured it (1.36 million tokens counted, about
  5 minutes; section 5). One
  pilot calibrates; claims need three replicates. No money is spent beyond
  the subscription.
- **Build:** 10 steps, each one PR, none touching `crates/manyroot` or the
  protocol. Steps 1 to 7 spend no model quota; the mock model backend runs
  every mode end to end in tests.

## 1. The task

### Why not the demo's six functions

The demo's task (six independent one-line functions) is too small for 10
agents: they would finish it in a turn or two, and nothing in it depends on
anything else, so the experiment couldn't see integration overhead or
semantic conflicts, which are two of its six measures. Its grid (−6 to 6) is
also small enough for an agent to check exhaustively with the public
checker, so passing the checker would be the same as being right.

### Why not a real codebase yet

A real Rust or Python project is the eventual goal, but in a first run it
costs three things the toy doesn't: agent-written code has to execute
(tests), which needs a sandbox on the host; grading needs a held-out test
suite that is expensive to write well; and every turn reads far more
context, so a run costs 5 to 20 times more. The first run should find out
whether the harness and metrics work, at a price that makes a mistake cheap.
A real-code task is the second tier (section 9).

### `stacklib`

The demo's language, extended just enough to make a library:

- A module is a text file of definitions, one per line:
  `def clamp3 3 : a b max2@ c min2@`. `#` starts a comment. Four modules
  (`base`, `order`, `arith`, `shape`), 6 to 8 functions each. A module
  with any line that doesn't parse contributes nothing, as a source file
  with a syntax error doesn't compile, and a name defined more than once
  counts as undefined. A module is at most 16 KiB, and errors quote at
  most 40 characters of a bad word.
- Functions take up to 3 inputs (`a`, `b`, `c`). `name@` calls another
  function in the library: it pops that function's arity in arguments and
  pushes its result. Calls are resolved by name across all modules, so a
  module depends on others the way source files do.
- The interpreter stops a program after 10,000 steps or a call depth of 64
  and reports a failure, so no program can hang the grader. It runs inside
  the harness, in Rust, on integers with wrapping arithmetic, like the
  demo's `task::eval`. Nothing an agent writes is ever executed any other
  way.
- 30 specs in layers, one layer per module: layer 0 (`base`) needs only the
  language (`abs`, `sign`, `max2`), layer 1 (`order`) calls layer 0
  (`clamp3`, `dist`, `median3`), and so on up to layer 3 (`shape`), so later
  functions are only as right as what they call. The list is `SPECS` in
  `crates/experiment/src/task/phase1.rs`. Runs choose their task with
  `--task`, and Phase 1's is the default, so this one stays runnable as it
  ran ([phase2-task.md](phase2-task.md), Step 1 as built).
- **6 change requests**, released together when the agents have used 40% of
  all their turns in that mode (the same point in every mode). Each changes
  a shared contract: `clamp3` takes `(lo, hi, x)` instead of `(x, lo, hi)`,
  so its callers must change their code; `sign(0)` becomes 1; `nearer`
  breaks ties the other way; `between` excludes its upper bound; `abs`
  saturates instead of wrapping; `spread3` counts the integers spanned, so
  `flat3`, whose own contract doesn't change, must change its code. A spec
  defined in terms of a changed function follows it (`dist` follows `abs`),
  so its code needn't change but its callee's must. A change made by one
  agent and not followed by the callers' owners is the experiment's source
  of semantic conflicts.

Each spec has a reference implementation in Rust, at least two correct and
two subtly wrong candidate programs (for the scripted agents and the tests,
as in `task.rs` today), and two input sets:

- **Public cases**: 60 per function, drawn from −6 to 6 as now (a function
  of one input gets all 13). They are the same in every run. The agents'
  `check` tool runs these and nothing else.
- **Hidden cases**: the public ones plus 2,000 seeded draws from −1,000 to
  1,000 and the edge values (0, ±1, `i64::MIN`, `i64::MAX`). Only the grader
  runs them, and they never reach a model. A program that special-cases the
  public inputs passes `check` and fails the grader.

The grader scores a function two ways, which is what makes semantic
conflicts measurable:

- **In library**: calls resolve to the library's own versions, as a user of
  the library would see it.
- **Standalone**: calls to other specs resolve to the reference
  implementations, so only the function's own code is judged. That holds
  only for specs of the function's own layer or a lower one whose contract
  isn't defined in terms of it; the function itself, an agent's own helper
  functions, higher layers and specs defined by it resolve in the library,
  so a program can't score by handing its work back to an oracle of
  itself.

A function that passes standalone but fails in library is broken by what it
calls, not by its own code. The hidden cases' seed comes from the run's
plan. In the code, everything an agent may see is in `task` (the language,
specs, change requests, public cases and `check`), and the hidden cases and
scores are in `grader`, which `task` never uses; a test checks both.

## 2. The three modes

### What is the same in every mode

| | Same in every mode |
|---|---|
| Task text | The same brief and specs, in each mode's native form (a `TASKS.md` and issues in Git, a `tasks` document in CRDT, a task claim and spec claims in Bracken), generated from one source |
| Change requests | Released at the same point (40% of turns used), in the same native form |
| The final artifact | The same four module files, graded by the same grader |
| Checker | `check <function>` runs the public cases on the agent's own working copy, with every call resolved in that copy |
| Private memory | A notes file of up to 4,000 characters per agent that the harness keeps between turns. Nobody else can read it |
| Sync | One hub per mode, held by the harness. Every agent syncs with the hub at the start and end of each turn, and can sync whenever it likes with one tool call |
| Model, prompt, budget | Same model and effort, same base prompt, same limits per call, turn, agent and run (section 5) |
| Identity | The harness sets each agent's identity (git author, CRDT actor, Bracken key); agents can't choose or change it |

No mode has a chat channel. Messages that bypass the mode would let agents
coordinate in a way the mode doesn't provide, and the comparison would be of
the chat channel.

### Mode 1: Git branches and pull requests

- **Hub:** a bare git repository in the run directory. Each agent has its
  own clone. git runs with a cleared environment, no system or global
  config, hooks disabled (`core.hooksPath` to an empty directory), and no
  remote but the hub.
- **Tools:** read a file (a module or `TASKS.md`), edit a module file
  (replace a line range or a unique string), `status`, `diff`, `log`, create
  or switch a branch (from the hub's `main` or an open PR's head), commit,
  merge `main` into the branch (conflicts are left to edit and commit) or
  rebase onto it, `check`, and a small local PR system the hub serves: open,
  list, view (diff, CI, reviews, comments), comment, review (approve or
  request changes), merge, close. There are no separate push and fetch
  tools: every sync pushes all the agent's branches to the hub's
  per-agent namespace and fetches `main` and every open PR's head, and the
  tool descriptions say so (decision `6b13e672e7d4` point 5). PR tools'
  values are checked in the agent's own tool worker, and the hub runs the
  checked form as that agent; free text in PRs is data in the hub's PR
  record, never an argument.
- **Task and change requests:** `TASKS.md` on `main`, with a
  machine-readable line of released change tags. Only the setter commits it,
  at setup and at the change release; agents can't edit it (the file-name
  rule), and CI refuses any PR that changes it (decision `6b13e672e7d4`
  point 6, like Bracken mode's setter-only spec claims). A copy's `check`
  uses the version its last-fetched `main` states.
- **Merge rule** (decision 6, section 8, made exact by decision
  `6b13e672e7d4`, all part of the pre-registered plan): a PR merges into
  `main` when
  1. **CI** passes on the merge result: every module parses with no name
     defined twice, only module files change, every function that passes
     its public cases on `main` still does, and every function the PR
     changes does, against the cases of the hub's released version. Missing
     functions are skipped, so after the change release a PR fixing one
     function isn't blocked by others that now fail on `main`. The hub
     computes CI at merge time (and shows it in `list` and `view`);
  2. someone other than the author **approved the PR's current head**: a new
     push needs a new approval, so approve-then-swap can't merge unreviewed
     code;
  3. no reviewer's latest review **requests changes**. Each reviewer counts
     once, by their latest review, and the author never counts. A request
     for changes blocks until that reviewer approves, parallel to Bracken's
     counted fail (decision `5e8df6996a2e`), and is measured the same way: a
     block on a PR the hidden grader scores correct is a semantic-conflict
     event (section 4);
  4. **any agent** may merge an open PR once 1 to 3 hold. The hub makes a
     `--no-ff` merge commit of the PR's head into `main`; a merge that
     conflicts is refused and the PR stays open, for its author to merge
     `main` into the branch, resolve, commit and sync.
- **Integrated state:** `main` on the hub. Unlike a superseded Bracken spec,
  a function merged before the change release stays on `main` until a PR
  changes it.

The PR system is built locally rather than using the Forge: the Forge is an
outside service with credentials, and its API latency and rate limits would
be part of what's measured.

### Mode 2: CRDT shared source editing

- **Hub:** one Automerge document holding the four module files as text and
  a shared `notes` text for coordination (like comments in a shared editor).
  Each agent holds its own copy; sync is Automerge's sync protocol between
  the agent's copy and the hub's, its messages carried by the driver.
- **Tools:** read a file, edit a file (the same two edit forms as Git mode,
  applied as text-CRDT operations), read and edit `notes`, sync.
- **No gate.** Every edit is the integrated state once it reaches the hub.
  That's the mode: continuous integration of text, with nothing deciding
  what's right.
- **Integrated state:** the hub's document.

Using the `automerge` crate is a new dependency, in the experiment crate
only, and needs the supply-chain part of security review. Writing a text
CRDT ourselves would be cheaper to vet and much easier to get subtly wrong.
CRDT mode is in the first run (decision 3, section 8).

**As built** (task `aaa30dca350b`, decisions `89568ebb896f`, `285ce6a0cad9`
and `7f454b0343ff`; points 1 to 4 are part of the pre-registered plan):

1. **Check and integrated state.** The document holds `base.stk`,
   `order.stk`, `arith.stk`, `shape.stk`, `TASKS.md` and `NOTES.md`, named as
   in Git mode. `check <function>` runs the public cases with every call
   resolved in the agent's own document, at the version its copy's
   `TASKS.md` states. The integrated state is the hub's four module texts as
   they are. There's no resolution rule beyond what the text holds, so a
   function defined twice fails to parse, as a module would in Git mode.
   `TASKS.md` is the setter's only: the edit tools refuse it, and every
   store refuses a received change to it by anyone else. `edit` and
   `replace` take Git mode's arguments, and each becomes the smallest text
   splice from the old text to the new.
2. **Identity.** The harness gives each agent an Automerge actor of 32 bytes
   derived from its name. The hub takes a change it lacks only from the
   actor of the agent it is syncing with, so every change the hub holds is
   by the setter or by the agent that brought it. A copy, which gets
   changes only from the hub, takes the setter's edits to `TASKS.md` and any
   other actor's to the module files and `NOTES.md`.
3. **Bounds on what a peer sends.** Agents are honest Claude Code
   sessions that reach the hub only through the harness's own code
   (decision `7f454b0343ff`), so these bounds keep a normal run from
   hanging or running out of memory, not crafted messages out. Automerge's
   own receive path inflates compressed changes and V2 messages' documents
   without a bound, sizes a bloom filter's probe list from the wire, and
   divides by a wire-chosen length (note `7f9a154018f5`). So each message
   is parsed first, and accepted only as a V1 message with at most 1 024
   hashes per list and 4 `have` entries, a bloom filter with Automerge's own
   constants and exact length, and at most 10 000 changes, each an
   uncompressed change chunk of at most 2 MiB. A message holds at most
   250 000 ops and 17 MiB. Automerge sends a peer everything it lacks in
   one message, so this is also about how far a copy can fall behind and
   still catch up. A document holds at most 20 000 changes and 16 MiB. Each
   new change must be by an allowed actor and hold only single-character
   inserts and deletes on that actor's text objects. A message that fails
   any of this is refused whole, before any of it is applied, and the
   session ends with an error. A session gets 60 s, 32 MiB and 200
   messages. Both sides clear the peer's advertised capabilities after
   every message, so neither ever sends V2.

   Every Automerge call on the document runs under `catch_unwind`. After a
   caught panic or any error, the in-memory document is thrown away, never
   saved, and reloaded from its last save. A document is saved only if it
   loads back with the same heads, since Automerge can apply a change and
   then save a document that doesn't load (finding `0357e515ecaa`). Ops
   aren't checked against the elements they name (findings `f28fcc5b25ac`,
   `70cba757d87f`, `0357e515ecaa` and `a9c642a058cf`, accepted risk under
   `7f454b0343ff`); a session that trips Automerge just fails, and the hub
   keeps its last save.
4. **Changes whose deps are missing** (a departure from `89568ebb896f`,
   which refused them on arrival; accepted by decision `285ce6a0cad9`).
   They are refused at the end of the session, not the moment they arrive.
   Automerge's bloom filter has false positives (about 1%), so an honest
   sender sometimes leaves out a change the receiver lacks, sends its
   children, and sends the change itself only when asked. With a fresh
   sync state each session the same false positive recurs every time, so
   refusing at once would leave that copy unable to sync. So within a
   session Automerge may hold such a change and ask for its deps, under
   four conditions:
   1. A held change is never saved, sent to another peer, used in the
      integrated state, or used by `check` or the grader. Saves drop held
      changes, and a session's document is used only by that session.
   2. A session is kept only if it ends normally with every dep held, and
      its document saves and loads back. Any other end (an error, the
      deadline, the budget, the frame cap, the peer gone, a caught panic)
      drops everything the session brought, held changes included, by
      reloading the document's last save.
   3. Held changes count against the session's 32 MiB and 200 messages,
      and the document's caps, like any others. Nothing held carries over
      to the next session.
   4. A change whose own deps are held but themselves dropped is dropped
      with them: the drop is a reload of the last save, which never held
      either.

**Sync on the wire.** Over the driver's two sockets, each side sends in
turn one frame, a u32 big-endian length and then a V1 sync message (length
0 for nothing to send), until both have sent 0 in a row. Each session
starts from a fresh sync state.

### Mode 3: Bracken semantic contributions

- **Hub:** a Bracken replica held by the harness. Each agent has its own
  replica and identity, as in the demo. The hub's first export seeds each
  replica; after that, each sync is one Bracken sync session between the
  agent's replica and the hub, relayed through the driver, so only what each
  side is missing moves (section 3). The harness serializes syncs.
- **Tools:** the read commands (`log`, `show`, `heads`, `conflicts`,
  `accepted`, `cat`), `claim`, `artifact`, `relate`, `evidence`, `retract`,
  each run through the real `bracken` binary with `--json`, plus `check` on
  an artifact. Under the one-pass rule two more show Bracken's own views
  (task `541b6f3db130`): `status`, Bracken's status view as `bracken status
  --as <agent>` prints it (task `f1c8478d1012`: one line per implementation,
  the agent's own work, contradictions of its claims and work waiting for its
  pass first; within each part, work waiting for an independent pass or ready
  first, oldest first, then blocked, rejected, superseded, retracted and
  accepted; live intents; at most 40 lines), and `news`, an incremental log:
  `bracken log --since <cursor> --compact --as <agent>`, one line per
  contribution that arrived since the agent's last `news` call, what concerns
  the agent first, by a cursor the harness keeps in each agent's own store.
  The `evidence` tool's optional `depends_on` ids, each the agent's own
  choice, become `--depends-on`: with a fail, status shows the target
  blocked until they are accepted, then ready (answer `ae21de46a008`). News
  that needs more than a page comes oldest first instead, without the
  personal order. A `news` page stops below the result bound (36,000
  characters, measured JSON-escaped as the driver sends it, so the bound
  never cuts it) and says how many more arrived; its cursor is the old
  cursor's ids and the page's, so the next call starts after everything
  shown and nothing is skipped or repeated (findings `f8b022bbf5fd`,
  `36c77161816c`). A position file that can't be read is reported, not
  taken as a fresh start. Id arguments take the first 8 or more
  digits of an id when no other id in the agent's replica starts with
  them, as Bracken's own prefix rule does (manyroot's `resolve_prefix`);
  the harness resolves them to full ids before running anything, and
  refuses an ambiguous or unknown one with a message saying so. A run under
  Phase 1's rule gets Phase 1's tools exactly: no `status` or `news`, ids
  of 12 digits or more, and Phase 1's descriptions; the plan's tool hash
  records which. An implementation is an artifact named for its function,
  which its own author relates as `implements` the function's current spec
  claim. Its text must parse on its own, define the function once, and fit
  2000 bytes, so a module with every function at that size still fits the
  language's 16 KiB module limit and no artifact can break the others in its
  module (finding `bc4a1eb3bdcd`). It may define helpers, each named
  starting with the function's name (`median3lo`), and no other task
  function. No spec name starts another, so a helper never stands in for a
  task function and never collides with another function's helpers (finding
  `624fb21cfacd`). The `artifact` tool refuses text that breaks these rules,
  and the materializer ignores it.
- **Only the task setter states the task.** The task claim, the spec claims
  and the change-request claims count only when the setter's key signed
  them. Each replica learns that key from the hub's first export, which
  holds only the setter's work. A claim tagged `spec` or `change` by an agent
  is just a claim (finding `29c4dbee556d`).
- **`check` on an artifact** resolves calls in the agent's working library
  (answer `88ae128c5fcd`, part of the pre-registered plan): the artifact
  under check for its own function; for every other function, the fixed
  rule's pick below if there is one, else the agent's own latest current
  implementation, else the latest current one by anyone. So, as in the other
  modes, an agent checks against what its copy holds, accepted or not.
- **Change requests** are new spec claims by the task setter that supersede
  the old ones (the setter wrote them, so the supersession counts), each with
  a claim tagged `change` and the request's own tag stating it. An
  implementation of a superseded spec no longer counts; a copy learns of a
  change when it syncs, and checks against the changed contracts from then
  on.
- **Integrated state:** the accepted layer at the hub, materialized into the
  four module files by a fixed rule: for each current spec, the current
  (not retracted or superseded) implementations with at least one
  independent pass and no counted fail; if there are several, the earliest
  in causal order. Under the one-pass rule a fail counts as Bracken's
  default policy counts it (decision `d036bab456a9`): any fail from someone
  other than the author that isn't retracted keeps the implementation out,
  whatever that validator recorded later, and a validator lifts its fail by
  retracting it. So the `status` view and the `accepted` tool agree on
  fails. One independent pass accepts, as Bracken's default
  policy does, with no ranking by pass count (owner decision
  `a4d8622c75d5` point 2). If none, the function is missing (decision 7,
  section 8). Phase 1 ran with a different tie rule, the most passes first
  and then the earliest; a run can still choose it (`--accept-rule
  phase1-most-passes`) to reproduce Phase 1, and the plan states which rule
  a run used, in its hashed rules. The `accepted` tool lists, per function,
  the implementation that counts as accepted and the function's other
  implementations as waiting, without pass counts. It is what counts, and
  it differs from Bracken's `status` view only by the task's own rules: an
  implementation counts only if its own author related it, to the setter's
  current spec claim, by an artifact that keeps the artifact rules above,
  and only the earliest accepted one counts, where `status` may show
  several. Under Phase 1's rule verdicts count as decision `5e8df6996a2e`
  says: each validator counts at most once per implementation, by its
  latest current verdict. Under either rule the implementation's author
  never counts. Passes are counted by validator, not by evidence
  contribution, so one validator recording the same pass again gains
  nothing (finding `40f448eb0951`). So one agent's fail keeps an
  implementation out, as in Bracken. That is Bracken's real semantics, and
  it is what the experiment tests. How often it blocks correct code is
  measured: a counted fail on an implementation the hidden grader scores
  correct is a **semantic-conflict event** (section 4).

Bracken mode doesn't use the demo's ring or gossip topologies. Git and CRDT
editors both sync through a central server in practice, and giving Bracken a
slower or different topology would measure the topology. Peer-to-peer runs
can come later as a separate comparison of Bracken against itself.

### Why each mode uses its own unit of work

Git and CRDT edit module files; Bracken contributes one definition per
artifact. That is a difference between the modes, not a bias: each works in
its natural unit, and all three are judged on the same materialized files.
Forcing Bracken to edit whole files, or Git to commit one file per function,
would test a mode nobody uses.

## 3. Driving the agents

### Processes

Each agent turn is one headless Claude Code process on this machine's
existing Claude Code setup, which routes every model request through the
owner's LLM proxy (decision `e8be1670f346`). Five kinds of process take
part, and each gets only what its job needs:

| Process | Runs as | Can read | Can write | Can reach | Holds the proxy's auth value |
|---|---|---|---|---|---|
| **Driver** (`experiment run`) | The owner, unsandboxed | Its plan; the routing entries of the owner's `~/.claude/settings.json`; the bytes the workers send it (sync data, the integrated files to grade) | The run directory, the event log, transcripts, the report | Only the LLM proxy, through its own local forwarding proxy | Yes, in memory only |
| **Claude Code**, one per agent turn | The owner, under the **agent** profile | The Claude Code installation and system libraries; its own config directory; its read-only turn configuration | Its own config directory (`HOME`, `CLAUDE_CONFIG_DIR`, `TMPDIR`) | The driver's local proxy port, and the shim's socket | Yes: it needs it to make requests |
| **MCP shim**, started by Claude Code | The owner, under the agent profile (inherited), with an empty environment | Its own binary | Nothing | The driver's socket for this agent | No |
| **Tool worker**, one per agent | The owner, under the **tools** profile, started by the driver | Its agent's working copy; the `git` and `bracken` binaries | Its agent's working copy | Only its pipes to the driver | No |
| **Hub worker**, one per mode | The owner, under the **hub** profile, started by the driver | The mode's hub; the `git` and `bracken` binaries | The mode's hub | Only its pipes to the driver | No |

How a tool call travels: the model asks Claude Code for a tool; Claude Code
sends it to the MCP shim over stdio; the shim, which does nothing else,
forwards the bytes to the driver over a Unix socket (one per agent, mode
0600, open only during that agent's turn); the driver logs the request,
passes it to that agent's tool worker over the worker's standard input (one
JSON line each way, bytes hex-encoded), logs the reply and returns it. Sync goes through the driver too, one hub at a time (Moving data,
below). So:

- **Only the driver writes the event log**, and it records what actually
  passed, not what a worker says it did. To grade, the driver asks the hub
  worker for the integrated files and grades the bytes it gets back; the
  grader is the Rust interpreter, inside the driver, and runs nothing.
- **Agent-chosen values are parsed only in the tool worker**, whose sandbox
  holds one agent's working copy and nothing else. A tool worker broken by a
  hostile argument can spoil its own agent's copy, which the hub worker and
  the grader then see as that agent's work, and no more.
- **The hub worker never gets free agent input as arguments.** It runs fixed
  commands the driver builds from validated values (Arguments, below), such
  as fetching `refs/heads/<branch>` from a received bundle into a per-agent
  namespace.
- **The auth value stays in the driver and Claude Code.** The driver reads
  the routing entries from the owner's settings and passes them only in the
  environment of the Claude Code process. Claude Code passes its whole
  environment on to MCP servers (note `5b11e56c79a6`), so `mcp.json` starts
  the shim through `/usr/bin/env -i <shim>`, and the shim exits at once if
  any variable is set. The tool and hub workers are the driver's children,
  started with a cleared environment that never held it. The value is never
  written to the run directory, the event log, a transcript, the Ground or
  git, and nothing the driver prints contains it.

### Moving data between copies and hubs

No worker ever opens another's store. Data moves as bytes through the
driver: one side writes a bundle or a message, the driver logs its size and
hash and hands it on, and the other side reads it from its standard input,
a socket, or a file it writes in its own store, and applies it. So each worker writes only its own store, and none needs a
shell: git's local transport, which starts `git-upload-pack` through
`/bin/sh` and then `/bin/bash` on macOS (finding `6e3f4df3e7ce`), is never
used.

| Mode | Copy to hub (push, a sync's first half) | Hub to copy (fetch, a sync's second half) |
|---|---|---|
| Git | The tool worker bundles every branch of its copy without what the hub has (`--not --remotes=hub` and the tips the hub last said it holds of this agent's branches) and sends the tips with it; the hub worker checks the bundle (below) and fetches its `refs/heads/<branch>` into that agent's namespace (`refs/agents/<agent>/<branch>`), then moves any branch whose tip it already had. A PR merge is then a merge inside the hub. Each agent's pushes are capped at the run's per-agent byte quota | The hub worker bundles `main` and every open PR's head (`refs/pr/<n>`) without the commits the copy said it has, and sends their tips and what it holds of the agent's branches; the tool worker checks the bundle and fetches it into `refs/remotes/hub/main` and `refs/remotes/hub/pr-<n>`, moves any of those whose tip it already had, and drops PRs no longer open. A sync with nothing new moves no bundle at all |
| CRDT | The tool worker produces an Automerge sync message from its document's sync state; the hub worker applies it | The reverse, repeated until neither side has anything to send |
| Bracken | One session, both halves: the tool worker runs `bracken sync --exec <bracken-experiment> shim --socket <driver socket>` in its replica, and the hub worker runs `bracken serve --stdio --allow <the agents' keys>` in the hub, relaying its standard input and output to its own driver socket. The driver relays between the two sockets, counting and hashing each direction. Heads and offers go first, so only what each side is missing moves. Both sides cap the session's time and bytes (60 s, 32 MiB). Over stdio every session has the same credential, so the hub gives each agent its own first-seen log, and with it its own quota, set from the run's caps (turns × 25 calls × 9, the most contributions one call adds, and their bytes): no agent within the caps reaches it, and one past them is refused alone (finding `57acf0d9ce57`) | (same session) |

Every import validates what it receives as it does any bundle (Bracken
checks signatures and parents; git checks objects), and a bundle that fails
is refused and logged, never retried as a different command. A git worker
fetches only bytes it has written to a file of its own that start with a
bundle signature (`# v2 git bundle` or `# v3 git bundle`) and pass `git
bundle verify`, and only the refs `git bundle list-heads` shows that map to
its fixed namespaces; every other ref, or a gitfile, a directory or any
other payload, is refused before git reads it as a repository, so the hub
never starts `git-upload-pack` (task `d0f9eefc612c`). With
`transfer.fsckObjects` and `fetch.fsckObjects` on, a malformed object is
refused too. Git before 2.46 doesn't apply them to a fetch from a bundle
(Ubuntu's 2.43 doesn't, note `107842fc89e0`), so the hub refuses such a git
at setup. A bundle is fetched, with no ref written, into a quarantine
object directory (git's `GIT_OBJECT_DIRECTORY`, with the store's own
objects as an alternate). A push may bring at most 500 commits the hub
doesn't have, counted before any tree is read. Its objects join the store
only if every new commit's tree holds nothing but regular (`100644`) files
at the four module paths and `TASKS.md`, each at most 64 KiB, checked in
three `git cat-file` processes however many commits there are; then only
the objects its refs reach are packed into the store. Otherwise the
quarantine is dropped, so a refused push leaves nothing behind. A store
remembers the heads it has checked and never checks one again, so a long
PR costs other agents' syncs and views nothing (finding `4bcfe41f2818`).
An agent may have at most 4 PRs open, each on a branch with commits `main`
doesn't have. A sync reads and writes every PR ref in one git process each
way, and CI is computed once per (`main`, head) pair, so another agent's
sync costs a fixed number of git processes however many PRs are open
(finding `f582d00f535f`). PR text is capped too: a title of 200 bytes, a
body, comment or review of 4000; at most 8 comments and reviews by one
agent on one PR, 32 PRs by one agent over a run, and 128 KiB of PR text by
one agent as the hub's record stores it. `pr_view` shows a PR's latest 20
comments and reviews, and `pr_list` only the number, author and state of a
closed or merged PR. A sync reads only the list of open PRs and the push
counter, each in a small file of its own, never the PR record (finding
`7d119d53922f`). Every child process that is both fed and read
(`git cat-file --batch`, `bracken import -`) gets its input from a thread
while its output is read, so neither pipe can fill and stall it (finding
`0521992b7de5`). A symlink, gitlink,
executable, directory or other path is refused, in the hub and in every
copy, before anything is checked out (finding `f1dcf89fd948`). Every ref
the hub or a copy moves to a commit a peer named checks that commit's
history new to the store the same way, so a later push can't name a
commit that never passed (finding `44769e26999c`). Refused pushes count
toward the agent's quota. The hub leaves out of every copy's sync any open
PR head whose history fails the check, and says so in the sync's event, so
one agent's PR never fails another's sync; `pr_view` shows such a PR's
problem and reads none of its content. CI checks the merge result's tree
the same way, and reads a module only after its size (finding
`32a67d979d81`). The demo's
`bracken sync <dir>`, which writes both replicas, isn't used. A sync that
fails, like any failure of one agent's (a dropped connection, an MCP
message over 256 KiB, its backend's error), is logged as that agent's
`turn_error`, with its size and digest only, and ends only that turn. The
run goes on (finding `2b37d97ab9c6`). Nothing waits without a deadline:
the driver serves a turn's connection until the turn's time (10 minutes)
is spent, every read of a sync relay has a deadline past bracken's own
60 s session deadline, and the hub's relay into `bracken serve` stops once
bracken has finished rather than waiting for the socket to close, since on
macOS a shutdown doesn't wake a read blocked on the socket's clone
(finding `05bee0cd4fc4`). Both halves of that relay run on their own
threads, and the hub worker stops `bracken serve` once the session's
deadline passes, whatever the relay is waiting on; a reply to an agent
that has stopped reading waits at most until the turn's deadline (finding
`a8cd5b213d6f`). A worker whose reply can't be read whole (over the line
limit, say) is stopped and fails every later call, so no call is ever
answered with another call's reply.

### Starting Claude Code

```sh
<pinned claude binary> -p "<turn message>" \
  --model claude-opus-5-5 --effort medium \
  --system-prompt-file <turn config>/prompt.md \
  --tools "" \
  --strict-mcp-config --mcp-config <turn config>/mcp.json \
  --setting-sources "" --settings <turn config>/settings.json \
  --permission-mode dontAsk \
  --no-session-persistence \
  --output-format stream-json --verbose \
  --max-budget-usd <per-turn stop> < /dev/null
```

Its environment is exactly: `HOME`, `CLAUDE_CONFIG_DIR`, `TMPDIR` and
`CLAUDE_CODE_TMPDIR` pointing into the agent's own directory, a minimal `PATH`, `HTTPS_PROXY`
pointing at the driver's local proxy, the setting that turns off
non-essential traffic, and the owner's routing entries
(`ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`,
`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY`). If the owner's settings ever
hold an `env` entry outside that list, the driver refuses to start, so a
routing change is noticed instead of copied blindly. That is the machine's
existing setup used as it is: the same proxy and the same value, with
nothing bypassed and no second login.

- `--tools ""` switches off every built-in tool (shell, files, web, agents),
  and `--strict-mcp-config` loads no MCP server but the shim, so the model's
  only tools are the mode's.
- Three traps found by probing the pinned binary (note `24aa40160488`):
  `sandbox-exec` matches resolved paths, so every path a profile names is
  resolved first (`/tmp/x` is `/private/tmp/x` to the kernel); Claude Code
  writes `/tmp/claude-<uid>` whatever `TMPDIR` says, unless
  `CLAUDE_CODE_TMPDIR` points elsewhere; and with a standard input that
  isn't closed it waits for input first, so it gets `/dev/null`.
- `--setting-sources ""` and the agent's own `CLAUDE_CONFIG_DIR` and `HOME`
  load none of the owner's Claude Code settings: no hooks (the owner's
  settings set four kinds), no model or attribution settings, no user
  `CLAUDE.md`, no user skills, plugins or MCP servers, and Claude Code's
  memory lands in the agent's own directory. Routing still works, because it
  travels in the environment, not in the settings. The skills and plugins
  that remain listed are Claude Code's bundled ones, which do nothing
  without a Skill tool (note `5b11e56c79a6`).
- The driver's `settings.json` allows only the shim's tools
  (`mcp__experiment__*`); `dontAsk` denies anything else instead of waiting
  for a person. `--system-prompt-file` replaces Claude Code's own prompt.
- The **turn configuration** (prompt, `mcp.json`, `settings.json`) is
  written by the driver to a directory the agent profile can read but not
  write, outside the agent's own directory, and the driver checks the files'
  hashes against the plan before every turn.
- The **binary is pinned**: the plan records the real path of the versioned
  Claude Code binary (not the package manager's `latest` link) and its
  blake3 hash (the workspace's hash; no new crate for SHA-256), and the driver checks both before every turn, so an upgrade
  during a run stops the run rather than changing it.

**The gate.** Before the model's first move, the driver reads Claude Code's
`init` event and ends the turn, and the run, unless it shows exactly: the
pinned `claude_code_version`; the planned model; permission mode `dontAsk`;
no built-in tools and exactly the mode's tools; the shim as the only MCP
server; skills and plugins equal to the bundled list recorded in the plan; a
memory path inside the agent's directory; and the expected credential source
(the auth token from the environment, which Claude Code reports as
`apiKeySource: none`). Claude Code's per-content-block `assistant` events
come after it. The shim separately refuses to start
with any environment variable set.

### Arguments

Every value an agent chooses is checked in the tool worker before anything
runs, against a fixed rule for its kind, and refused with an error result if
it doesn't match:

| Kind | Rule |
|---|---|
| File names | One of the four modules or the notes file, from a fixed list. No paths, and nothing else (`.gitattributes`, `.git`) can be named |
| Branch names | `^[a-z][a-z0-9-]{0,39}$`. The only refs the tools accept are those branches and `main`; no revision expressions (`HEAD~1`, `@{u}`, ranges) |
| Commit ids | 40 hex digits, and must already exist |
| PR numbers, line numbers, counts | Decimal integers in a fixed range |
| Bracken ids | 8 to 64 hex digits (12 to 64 under Phase 1's rule), resolving to exactly one contribution in the agent's replica, which the harness then passes in full |
| Relation kinds, choices | From a fixed list |
| Tags | `^[a-z][a-z0-9-]{0,31}$`, at most 8 |
| Agent names | From the roster |
| Free text (file contents, statements, notes, comments, commit messages, PR titles and bodies, evidence details, retraction reasons) | At most 16 KiB, valid UTF-8, no NUL. Never in a place where it could be read as an option: on standard input or in a file the worker writes in its own copy (`git commit -F -`, `bracken evidence --details-file -`, `bracken artifact <the worker's file>`); where a command takes it only as an argument (a Bracken claim's statement), as the one argument after `--`, and where only as an option's value (a relation's note, evidence's method, a retraction's reason), inside one `--option=value` argument, which can't be split or read as another option |

And for every command the workers run:

- argv is built as a list and started directly, never through a shell;
- every positional comes after `--end-of-options` (git) or `--` (bracken),
  so a value starting with `-` can never be an option, even if a rule above
  is wrong;
- options are only ever the worker's own, from a fixed set per tool: no
  agent value selects an option, and no value an agent supplies reaches
  `--output`, `--details-file`, `-C`, `--git-dir`, `--receive-pack`,
  `--upload-pack`, `--exec` or the like;
- git runs from the real binary in git's exec path (never a launcher such
  as macOS's `/usr/bin/git`, which starts `xcrun`), with a cleared
  environment, no system or global config, `core.hooksPath` pointing at an
  empty directory, `core.fsmonitor` off, `core.symlinks` off (a link checks
  out as a plain file), no attributes or excludes file,
  only the `file` protocol allowed, `transfer.fsckObjects` and
  `fetch.fsckObjects` on, and automatic gc and maintenance off, so no file
  in a working copy can make git run a program (and the file-name rule means
  no agent can write `.gitattributes` or `.git/config` anyway). A tool
  worker reads and writes only regular files, never following a link.
  Step 3's
  tests read git's own trace of every program it starts and find only git
  itself: no shell, no `git-upload-pack`, no hook.

Tests (steps 2, 3 and 6): for every tool, a table of hostile arguments (a
leading `-`, `--receive-pack=<command>`, `--upload-pack=`, `--output=`,
`--details-file=`, `--exec=`, `-C`, `..`, `/`, an absolute path, NUL,
newline, an overlong value, look-alike Unicode) must be refused before any
process starts, shown by the driver's log recording no command. A second
set turns the rule checks off in a test build and shows that the
`--end-of-options` and `--` placement alone makes git and bracken refuse
the same values; the security review's `--receive-pack` reproduction
(finding `455e12da3ecb`) is one of them.

### Turns

1. A fresh conversation: the base system prompt, the mode's tools, and a
   first message saying which turn this is out of how many, the agent's
   name, and its notes file. The prefix is identical for every turn of every
   agent in a mode, so it is cached.
2. The model calls tools until it stops. The driver reads the event stream
   as it arrives: the `init` event, every assistant message with its token
   usage, and the final `result` event with the turn's usage per model and
   its duration.
3. The turn ends when the model stops, or when the driver ends the process
   at a cap (section 5): 150,000 tokens of context, or 10 minutes. The
   25-tool-call cap ends a turn softly (decision `147a36d5d623`): the
   driver's MCP server refuses the 26th call and every later one, except
   one `notes_write` (decision `74d00180c642`), telling the agent its tool
   calls are used up and to save its notes and stop, and the turn may still
   end itself and write its `result` event. Only if it hasn't within 60
   seconds or 3 more model calls does the driver end the process. The
   harness then syncs the agent with the hub and saves its notes.

Agents run their turns back to back, concurrently with each other, not in
lockstep rounds: real agents take minutes per turn, and lockstep would make
every agent wait for the slowest. Each mode stops when every agent has had 8
turns or spent its budget.

Fresh context per turn bounds each turn's use and makes it predictable. It
also means each agent has to rediscover the state from the mode itself,
which is what the modes differ in; the notes file gives every mode the same
private memory so that this doesn't punish modes with poor in-band memory
alone.

**Prompts.** One base prompt for all modes (the goal: a correct library, the
specs and change requests matter, check your work, coordinate through the
tools you have), plus each mode's tool descriptions. The prompts never
mention the experiment, the other modes or the metrics, so agents work on
the task rather than on the numbers. They are reviewed for neutrality before
the pilot (an independent reviewer reads the three side by side), and their
hashes go into the run's plan.

The files are in `crates/experiment/prompts/`: `base.md`, shared, and one
"Where things are" section per mode (decision `4cf00e1d701d`). The system
prompt is the base, then the mode's section. They are built into the binary.
The run's `plan.json` holds the blake3 of the system prompt it actually gives
agents (`inputs.system_prompt`, from `--prompt-file` in tests), of the tools
as agents see them (`prompts.tools`) and of the turn message template
(`prompts.turn-message`).
Each turn's first message is the same in every mode: "This is your turn
<t> of <n>. You are <name>. Start by reading your notes."

The mode sections have one shape: the same four sentences in the same order,
of 84 to 99 words (Bracken's is 99). A test holds that shape, and holds that no prompt or tool
description mentions the experiment, a measure or another mode.

| Sentence | Git | CRDT | Bracken | Why it differs |
|---|---|---|---|---|
| 1. Where the task is | TASKS.md on the hub's main, written only by the setter | TASKS.md in the shared document, written only by the setter | The setter's task claim and one spec claim per function | Each mode states the task in its own native form (section 2). An agent can't start without knowing where to look |
| 2. Where changes appear | A new version of TASKS.md on main; its last line lists the released changes | The same, in the shared document | A new spec claim superseding the old one, with a claim tagged change | The same release, in each mode's native form |
| 3. What counts as integrated | Main on the hub: a PR gets there only when someone merges it with pr_merge, which needs passing checks, a non-author's approval of its head, and no one else's latest review asking for changes | The hub's copy of the document: every edit to a module file counts on reaching the hub, with no other step | At the hub: implementations their authors related to each function's current spec claim, with a non-author's pass and no unretracted fail from anyone else; accepted lists them, and status shows what waits for a pass | This is the difference under test. Each sentence states the mode's real rule (decisions `6b13e672e7d4`, `5e8df6996a2e`), names the step an agent must take where one exists (Git's pr_merge), and says where the agent can see the integrated state (main, the hub's document, the accepted tool, which shows exactly the harness's count). Bracken's also names its status view, where work waiting for a pass is found, as Git agents find PRs waiting for review with pr_list (task `541b6f3db130`). CRDT's is shortest because it has no gate |
| 4. How work moves | Only when you sync | Only when you sync | Only when you sync | Identical: every mode syncs with one hub (section 2) |

Every tool result, in every mode, is bounded at 40,000 characters
(`MAX_RESULT_CHARS`, decision `74d00180c642`), below Claude Code's own
tool-result limit of about 50,000, so Claude Code never cuts one itself. A
longer result keeps its first 40,000 characters and ends with a note giving
its full length and saying how to narrow the call (one file, function or
item, a line range or a smaller count, where the tool takes one); the event
log records the full length as `cut_from`. No result is cut silently.

The base prompt tells every agent alike to sync before choosing what to
work on (task `541b6f3db130`): a pass usually arrived a turn later in
Phase 1. When the harness syncs doesn't change.

What the three share word for word: the base prompt, the turn message, the
core tools (`sync`, `notes_read`, `notes_write`), and the fourth sentence. The
mode tools' descriptions differ because the tools do (section 2). Each says
what its tool does, in the same register, with no advice on strategy.
Bracken's use plain words for the terms other modes don't need (replica,
current, the relation's direction, a verdict's method), so they are the
longest per tool: counted at PR #33's head, Git's mode tools average 14.3
words (258 over 18 tools), CRDT's 19.2 (77 over 4) and Bracken's 22.8 (273
over 12). The extra words define Bracken's terms and parameters and give its
check rule; models already know Git's vocabulary. The neutrality review
judged the detail comparable, with no strategy beyond the integration step
Git's prompt names too (evidence `e7e5868b02e8`). With `status`, `news`,
short ids and the fail rule (task `541b6f3db130`), Bracken's are 25.4 words
per tool (356 over 14).

In Phase 2, Bracken mode's `check` takes an optional `record` (owner decision `f3405c284180`, change B,
task `d508aae1ea72`). With it, the check's result also goes in as the caller's evidence on the artifact
in the same call: pass or fail, method `check`, and details what the check found. It goes through the
evidence tool's own command and argument rules, under the same sandbox. A recorded fail is logged as
blocking the artifact, as the evidence tool's fail is. It adds 26 words to check's description, and no
advice on when to use it. Phase 1's `check` is unchanged, so its runs' tool hash still holds. The other
modes keep their tools. Git's nearest step, approving a PR after a check, is already its own tool
(`pr_review`), so this asks the neutrality review whether one call where Git takes two (check, then
review) is comparable. That is the product change being measured, as `status` was.

Each mode names its shared channel once, in one tool description, in the same
sentence: "<it> is where agents talk to each other: everyone can read it
with <tool>." The channel is pr_comment's comments in Git, NOTES.md in CRDT
(in read's description), and claims in Bracken (in claim's). The base prompt
names none, so no mode gets a hint the others lack.

**The mock backend.** The driver has a second backend that makes the same
tool calls from scripted moves instead of starting Claude Code, built on the
demo's scripted agent. It runs every mode end to end with no model, so the
tests, the metrics and the whole run can be checked before any quota is
used.

### Isolation

The layers, each of which holds if another fails:

1. **No tools but the run's.** Built-in tools are off, the only MCP server
   is the shim, the permission settings allow only its tools, and the gate
   checks all of it every turn. The model can't run a command, read or write
   a file, or reach the network itself.
2. **Validated arguments** (above), so a tool call can't become a different
   command.
3. **Separate processes, separately sandboxed.** Each process runs under a
   `sandbox-exec` profile that allows only its row of the table above and
   denies everything else:
   - **agent profile** (Claude Code and the shim): no reads of the owner's
     home outside the agent's directory, of any repository, `~/.ssh` or the
     team's Ground; no mach service at all, so no keychain; no process started but the shim
     (through `/usr/bin/env`); outbound connections only to the driver's
     local proxy port and the shim's socket;
   - **tools profile**: reads and writes only its agent's copy; runs only
     the absolute paths of `git` (with the helpers in its own exec path,
     such as `git-pack-objects`) and `bracken`, never `/bin/sh`,
     `/bin/bash` or any other shell; no network but its socket; no keychain;
   - **hub profile**: reads and writes only its mode's hub, never a copy;
     runs only `git` (as above) and `bracken`, never a shell; no network but
     its socket; no keychain.

   `sandbox-exec` is deprecated but still works on current macOS. Step 6
   proves each profile with tests that try each forbidden access from inside
   it, including a hostile tool call that tries the owner's home, another
   agent's copy, the hub and the event log. If the profiles can't be made to
   work, the fallback is a separate macOS user account for the runs, which
   needs the owner's go.
4. **Network only to the model endpoint.** `sandbox-exec` filters by port,
   not by host (finding `d71aa9bb8134`), so the driver runs a small local
   forwarding proxy: Claude Code gets `HTTPS_PROXY` pointing at it, the
   agent profile allows outbound connections only to its port, and it
   accepts `CONNECT` only to the host and port of the owner's
   `ANTHROPIC_BASE_URL` (the LLM proxy, `llmproxy.jx.rs:443`). It only
   relays bytes; TLS is still between Claude Code and the LLM proxy, so it
   never sees the auth value. The workers have no network at all: git,
   Automerge and Bracken sync with hubs on local disk. Step 6 tests that a
   request to another HTTPS host, direct or through the forwarding proxy,
   fails.
5. **The gate** stops any turn whose Claude Code isn't the pinned binary,
   configured as planned.

The rest:

- **No agent code runs** except in the `stacklib` interpreter, in Rust, with
  step and depth limits.
- **The run directory** is a private (mode 0700) directory outside any git
  checkout, made through directory handles once task `efc537ca8e65` lands.
  Agents' Bracken keys are throwaway and stay in it. It is removed after the
  report is written, except the logs and transcripts the report needs
  (section 6).
- **Agents read each other's work**, so one agent's text can try to steer
  another (prompt injection). Its reach is limited to the mode's own tools
  on the reader's own copy: at worst it misleads an agent into bad work,
  which the grader sees. Tool results that carry another agent's text are
  marked as such.

### What the shared login exposes

The agents use the owner's own Claude Code setup, so every Claude Code
process holds the auth value for the owner's LLM proxy. If something
escaped the agent profile, or the model found a way to make Claude Code
reveal its environment, the exposure is that value: whoever holds it can
send model requests through the owner's proxy, against the owner's quota and
under the owner's name, for as long as the proxy accepts it, plus whatever
else the proxy lets that value do. That is the owner's whole model access,
not one run's. By itself it exposes no code, forge access, Ground keys or
other files; those stay behind the sandbox layers. If a leak is suspected,
the remedy is rotating the value at the proxy, which the owner controls. The
owner accepted that agents run with the owner's own login (decision
`d8cce1147b05`, refined by `e8be1670f346`).

## 4. Measuring

### Sources

The harness computes every number after the run from three records that no
agent can write to:

1. **Token accounting.** For every model call the driver records the usage
   Claude Code reports in its event stream (input, cache reads, cache
   writes, output, thinking), the model,
   the agent, the turn and which tools the call asked for.
2. **The event log.** Every tool call: agent, time, tool, arguments' hash and
   size, result, and for writes, the function definitions it created or
   changed. In Git and CRDT modes a write is a commit or an edit; in Bracken
   mode, an artifact.
3. **Graded snapshots.** Every time the integrated state at the hub changes
   (a merge to `main`, an edit reaching the CRDT hub, a sync that changes
   the Bracken hub's accepted layer), the harness materializes the four
   module files and runs the hidden grader on every function, in library and
   standalone. That gives the library's true quality as a curve over time
   and tokens.

### The six measures

| Vision measure | Definition | Computed from |
|---|---|---|
| Validated improvements per million tokens | An **improvement** is a change to the integrated state after which a function passes the hidden grader in library that didn't before (net: a change that makes one fail counts −1). Divided by the mode's total tokens (input, cache reads, cache writes and output) in millions. Also reported per million output tokens | Snapshots, token accounting |
| Duplicated work | A **delivery** is a write of a function's definition that differs from the then-integrated one. A delivery is **duplicated** if, when the agent made it, another agent's correct (by the grader) version of that function had already reached the hub (**visible**), or another agent delivered one after this agent last read that function's state (**blind**), as in the demo. Reported as counts and as the share of output tokens spent in the turns that made them | Event log, snapshots |
| Integration and validation overhead | The share of tokens and of wall time spent on calls whose tools integrate (sync, fetch, merge, rebase, conflict editing, PR operations, Bracken sync sessions) or validate (`check`, reviews, evidence), and the count of failed integrations (merge conflicts, PRs closed unmerged, failed CI) | Token accounting, event log |
| Time until a useful discovery is available to others | A **useful discovery** is a delivery that passes the grader standalone, or a bug report naming an input that really fails (section below). **Available**: seconds and other agents' turns from its making until it reached the hub where others could read it (pushed branch, synced edit, synced contribution). **Integrated**: until it was in the integrated state. **Taken up**: until another agent first read it | Event log |
| Semantically incompatible contributions | Changes to the integrated state after which a function the change didn't touch still passes standalone but no longer passes in library: each change was fine alone, together they're wrong. Also counted: changes after which a module no longer parses (mostly interleaved CRDT edits), and the semantic-conflict events: in Bracken mode a counted fail on an implementation the hidden grader scores correct (decision `5e8df6996a2e`), in Git mode a request for changes blocking a PR the grader scores correct (decision `6b13e672e7d4`) | Snapshots, the Ground |
| Quality of the final artifact | The final integrated library: functions correct in library under the latest specs (all change requests applied), out of about 30, and the number that are present but wrong | Final snapshot |

Bug reports are found in what agents write (PR comments, CRDT notes,
`finding` claims) by one pattern, a function, its inputs and the wrong
output, and count only if the harness re-runs the input and it really fails.
A false report never counts, so writing many costs the writer tokens and
gains nothing.

### Why the agents can't move the numbers except by doing the work

- **Correctness comes from the hidden grader**, on cases no model sees. An
  agent's evidence, approvals or claims of success don't enter any measure.
  Special-casing the public inputs fails the grader.
- **Tokens come from Claude Code's event stream**, read by the driver outside
  the model's reach. Agents
  can't see or change the counter.
- **Times come from the harness's clock** in the event log, never from text
  an agent wrote, such as a round number in a claim (the problem task
  `3e31e716104f` fixes in the demo).
- **Identity is set by the harness.** An agent can't commit as another
  author, edit as another CRDT actor, or sign as another key, so it can't
  approve its own PR or count its own evidence twice.
- **Volume doesn't score.** Contributions, comments and PRs aren't measures;
  only integrated, correct functions are.
- **The analysis is fixed before the run.** Each run starts by writing a
  plan: the harness commit, the metric code's hash, the prompts' hashes, the
  model and effort, the caps, the materialization rule with how Bracken
  mode counts verdicts (decision `5e8df6996a2e`: one per validator, its
  latest; never the author; at least one pass and no counted fail; fails on
  implementations the grader scores correct counted as semantic-conflict
  events), Git mode's merge rule (decision `6b13e672e7d4` points 1 to 4:
  CI on the merge result with no regressions and every changed function
  passing; an approval of the current head by someone else; no reviewer's
  latest review requesting changes, each reviewer once and never the author,
  with blocks on PRs the grader scores correct counted as semantic-conflict
  events; any agent merges, `--no-ff`, and a conflicting merge is refused),
  Bracken mode's working-library rule for `check` (answer `88ae128c5fcd`)
  and the seeds. It is
  contributed to the team's Ground as an artifact before the first model
  call, and the report quotes its id. Changing the analysis afterwards means
  a new plan and saying why.
- **Agents don't know they're measured.** The prompts don't mention metrics
  or other modes.

### How step 5 records and reports

Built in step 5 (`crates/experiment`, `plan.rs` and `metrics.rs`), with the
definitions the manager settled (question `6c4aa5dc0a01`, decision
`bbb261d50871`):

- **The plan** is written to `plan.json` before any worker starts, and the
  setup event logs its blake3. It holds the run's configuration, the model,
  effort, caps and seeds, `git --version`, the blake3 of the measuring code
  (`metrics.rs`, `grader.rs`, and the task's code and `lang.rs` it judges by)
  and of the task texts, tool list and turn message, the
  integration rules, and every measure's definition as `metrics.rs` computes
  it. The report refuses a plan whose hash isn't the logged one, or whose
  measuring code isn't this build's. It also refuses records that would make
  a number silently wrong: a log with a gap or cut short (no closing
  `integrated` event), a stop the plan doesn't expect, a failed snapshot or
  record, or a turn without usage that didn't fail before playing. A run
  stopped by one of the backend's own caps (design section 5), which the
  real run's plan expects, is measured from what exists, and the table says
  which cap stopped it.
- **Snapshots:** the driver logs the hub's integrated library (a `snapshot`
  event) after setup, after the release, and after any sync or hub tool call
  that changes it. The report grades them afterwards.
- **Writes:** after each successful tool call the store's `record` says, for
  the log only, which function definitions the call wrote (a commit's,
  compared with every parent; an artifact's), which ones a verdict blocks,
  and whether a merge stopped on a conflict. In CRDT mode, deliveries come
  from the snapshots instead.
- **Bug reports** are the form `check` prints, `f(1, 2) gave 3`, found in any
  tool's arguments except `notes_write` and logged with the tool call.
- **Tokens** come from `usage.jsonl` in the rows step 6's backend writes
  (its tool names, `mcp__experiment__<tool>`, are read without the prefix),
  at the path the plan names; the mock writes the same rows with made-up
  numbers. The real run's plan also states the model, effort, caps and the
  system prompt's hash.
- `bracken-experiment report <run>...` prints the comparison table, one
  column per run (`--json` for the full measures).

### What this can't show

All agents are one model with one prompt, so the run says nothing about
diverse agents, and correlated behaviour (vision, Hard problems) is built in.
One replicate per mode can't separate the modes from luck: model runs vary,
so the pilot calibrates quota use and checks the harness, and only three or more
replicates per mode support a claim about the modes. And `stacklib` is a toy:
a win here is evidence about the coordination mechanisms, not about real
software.

## 5. Subscription use and how it is capped

The runs use the owner's existing Claude subscription, through the same LLM
proxy as the team's own sessions, so they cost no money beyond it, but they
use its quota, which the team's own sessions use too.
Use is stated in tokens and wall-clock time, as Claude Code reports them.

### Estimate for one 10-agent pilot (all three modes)

Measured in the calibration run (step 8, 2026-10-28; reports `eb60e66777b3`
and `c59e23bd4be6`): Claude Opus 5.5 at medium effort, 2 agents, 2 turns, all
three modes, 12 turns in all. Every turn ended at the 25-tool-call cap, after
5 to 14 model calls (8.7 on average): the model asks for 2 to 5 tools per
call. So no turn had a `result` event, and each turn's output is counted at
the fail-closed ceiling of 30,000 tokens. The real output, thinking included,
is unmeasured: the stream shows only a stub, and the visible part (text and
tool inputs) was about 1,200 to 2,300 tokens per mode of four turns. It will
be measured once the soft stop at the cap (decision `147a36d5d623`) lets a
turn end with its `result` event. Until then, the ceiling is the estimate's
safe upper bound for output. Through the gateway, almost all input arrives
as cache reads and writes (17 new input tokens a turn).

The table scales the measured average turn (git, Bracken and CRDT together)
to the pilot:

| | Per call | Per turn (8.7 calls) | Per agent (8 turns) | Per mode (10 agents) | Pilot (3 modes) |
|---|---|---|---|---|---|
| Model calls | 1 | 8.7 | 69 | 690 | 2,100 |
| Cache reads | 7,600 | 66,000 | 530,000 | 5.3 M | 16 M |
| Cache writes | 2,000 | 17,000 | 140,000 | 1.4 M | 4.1 M |
| New input | 2 | 17 | 140 | 1,400 | 4,200 |
| Output (at the ceiling) | | 30,000 | 240,000 | 2.4 M | 7.2 M |
| **All tokens** | | 113,000 | 0.9 M | 9.1 M | **27 M** |

By mode, a turn used 111,000 tokens in git, 138,000 in Bracken and 90,000 in
CRDT, each with its 30,000 of counted output. These are first and second
turns of 2 agents. Later turns, with 10 agents, read a bigger state: the largest
context of one call was 11,000 tokens in git and CRDT and 38,000 in Bracken.
So the input side is allowed to double by the eighth turn, which puts the
pilot at **27 to 47 million tokens**, 7.2 million of them output counted at
the ceiling. Real output will lower the total once it is measured. Until the
soft stop lands, an agent whose every turn is counted at the ceiling reaches
the per-agent output cap of 180,000 after 6 of its 8 turns.

The owner's go for the pilot (approval `3418fe14b8ed`) caps the whole pilot
at 75 million tokens, 5 million output tokens and 3 hours. One `bracken-experiment
run` is one mode, and the modes run one after another, so each run is capped
at a third: 25 million tokens, 1,666,666 output tokens and 1 hour, equal in
every mode (decision `ea053c2c4d48`; caps item 4 below):

| | Re-estimate, pilot | Re-estimate, per mode | Pilot cap | Per-mode cap | Within it? |
|---|---|---|---|---|---|
| Tokens | 27 to 47 M | 9 to 16 M | 75 M | 25 M | Yes |
| Output | 7.2 M at the ceiling; 5.4 M at most under the per-agent caps | 2.4 M at the ceiling; 1.8 M at most under the per-agent caps | 5 M | 1,666,666 | No, at the ceiling |
| Wall-clock | 30 to 45 minutes | about 10 minutes | 3 hours | 1 hour | Yes |

So the output bound is over the cap: 7.2 million at the ceiling, or 5.4
million under the per-agent caps, against 5 million. It can't all happen. A
mode whose counted output reaches 1,666,666 stops at its run cap, and its
report is written from what exists. With the soft stop, real output is
expected well below the ceiling: the visible part was under 600 tokens a
turn. If it isn't, the manager goes back to the owner rather than raising a
cap.

A run can take one mode's token cap in place of the 25 million, as
`run --mode-token-cap <mode>=<tokens>` (the manager's note `2018c9beceff`).
The value applies only to a run of the named mode, and it can lower the cap
as well as raise it. The run's plan records the caps in effect, so the
plan's hash changes with it. Without it, every run's caps are as above. The
flag refuses a value above 75 million, the whole replicate's tokens.

The owner sets a mode's share with the go for a replicate. For Phase 2's
replicate 1 (approval `1316798b18f9`), Bracken's share is 30 million, and
git's and crdt's are 25 million each, so the shares add up to more than the
replicate's 75 million. No part of the harness adds up tokens across a
replicate's runs. So the run order keeps the total (decision
`c90a34c1ca38`):

- The first two modes in the drawn order run at their shares.
- The last mode runs with `--mode-token-cap <mode>=<cap>`, where the cap is
  the smaller of its share and 75 million minus the tokens the first two
  runs counted (their reports' totals).
- The last run's plan records that cap, so a replicate can't pass 75 million.

The expected use is about 40 million, so the last cap shouldn't bind in
practice.

Output tokens are what weigh most against usage limits; cache reads are most
of the total but the cheapest part.

- **Calibration run (done):** Opus 5.5, 104 model calls and 1.36 million
  tokens counted (git 446,000, Bracken 552,000, CRDT 359,000; 360,000 of
  them output at the ceiling), about 5 minutes for the three modes. The
  Haiku 5.5 shakedown before it (decision 2, section 8): 65 model calls,
  0.92 million tokens counted, also every turn at the cap. Both were clean:
  every turn passed the gate, with no sandbox denial, unreadable usage,
  crash or rate-limit wait.
- **Three replicates** for a claim about the modes: about 80 to 140 million
  tokens (22 million output at the ceiling), spread over separate sessions.
- **Larger runs:** use grows at least with the number of agent turns, and
  faster once the state is bigger than a turn can read cheaply: 20 agents
  about 55 to 95 million tokens a pilot, 100 agents well over 270 million.
  Those may be beyond what a subscription's limits allow in one sitting, and
  each needs its own go.

### Wall-clock time

Measured in the calibration run, a turn takes about 40 to 50 seconds (it
ends at the tool-call cap after about 9 calls), and a mode of 2 agents and 2
turns took 84 to 102 seconds. At that rate an agent's 8 turns take about 6
to 7 minutes. Agents in a mode run at once, so a mode should take about 10
minutes, and the pilot 30 to 45 minutes with the modes one after another,
mode order randomized and recorded. The soft stop's grace at the cap, and
rate-limit waits with 10 concurrent sessions, can lengthen that. Running the three modes at once (30
concurrent sessions) is more likely to hit the subscription's limits and
would hit the modes unequally, so it isn't proposed.

Ten concurrent Opus sessions may still meet the subscription's rate limits,
and they compete with the team's own sessions. So the manager tells the
owner before each run, runs it when the team is otherwise idle, and the
driver pauses an agent that gets a rate-limit error and resumes it after the
wait the error gives. Waiting time is recorded and left out of the time
measures, so throttling can't favour one mode.

### Caps

Each layer stops a run's use on its own:

1. **Per call:** Claude Code's own output limit per response.
2. **Per turn:** 25 tool calls, 150,000 tokens of context, 10 minutes.
   The driver ends the process at the context and time caps. The tool-call
   cap is a soft stop (decision `147a36d5d623`): the driver's MCP server
   refuses the 26th call and every later one, so none reaches a tool, with
   the message "This turn's tool calls are used up: no more will run this
   turn, except one notes_write. Finish now: save your notes if you haven't,
   then stop and end your turn." The one exception is the turn's first
   `notes_write` past the cap, in every mode (decision `74d00180c642`): in
   the pilot data, Bracken agents saved notes in 7 to 19 of 80 turns against
   Git's 64 to 71, so turns at the cap lost their notes. A second
   `notes_write` is refused like any other call. The count is also taken from
   the event stream (the `tool_use` blocks): from the 26th, the turn has a
   grace of 60 seconds and 3 more model calls to end with its own `result`
   event, which is then counted as any other turn's. Past the grace the
   driver ends the process, and the turn counts at the output ceiling below,
   as any turn without a result does. In the calibration run every turn
   ended at the cap by kill, so no turn had a result and real output was
   never measured; the grace lets the pilot measure below the ceiling. Claude Code's `--max-budget-usd` does apply through a gateway base
   URL (step 6 checked it against a fake endpoint: it stopped the turn after
   the first call that crossed the limit), so it is set on every turn at
   1 USD. At the price Claude Code applied to claude-opus-5-5 in that probe
   (about 4 USD per million input and 20 per million output tokens; cache
   reads taken at a tenth of input, and cache writes here at 1.25 times
   input), the measured turn above (about 66,000 cache reads and 17,000
   cache writes) is about 0.11 USD before output. Its output is unmeasured.
   At the 30,000 ceiling the turn would be about 0.71 USD, and with only the
   visible output about 0.12. The calibration couldn't re-check the stop:
   no turn had a `result` event, so Claude Code's cost estimate was never
   seen. That re-check waits for the soft stop (decision `147a36d5d623`). It counts Claude Code's
   API-equivalent estimate, not money spent, and it is the in-turn stop on
   output, which the driver only learns at the turn's end (below).
3. **Per agent:** 3 million tokens and 180,000 output tokens, equal in every
   mode (about 1.5 times the estimate). An agent that reaches either stops.
4. **Per run:** the announced use is for the whole pilot (75 million
   tokens, 5 million output, 3 hours; approval `3418fe14b8ed`). One run is
   one mode, so each run gets a third: 25 million tokens, 1,666,666 output
   tokens and 1 hour of wall-clock time (decision `ea053c2c4d48`; the
   driver's change is task `037bf6218a32`). The driver checks the run's
   running total against them before every turn and while each turn
   streams; at the limit it stops every agent and writes the report from
   what exists.

**The caps fail closed** (finding `8d72c61ddf30`):

- **Missing usage is a breach.** An assistant message or `result` event
  without a usage record the driver can parse (a field missing, renamed or
  not a number, a `result` without usage per model, a line that isn't JSON)
  stops the run, as a cap breach would. Nothing is ever counted as 0
  because it couldn't be read.
- **Output comes from the `result` event.** Claude Code's `assistant`
  events repeat the usage of the response's start, whose output is a stub
  (1), and even its re-emitted `message_delta` shows that stub; only the
  `result` event has the true output (note `24aa40160488`). So the stream's
  output is a lower bound while the turn runs, and the result's is counted.
  A turn that ends without a readable `result` event (ended at a cap,
  crashed, cut off) is counted at the larger of its output cap, 30,000
  tokens (twice a turn's expected output of 15,000), and what the stream
  already counted, never less (decisions `e753d794dd1d`, `312e385f673c`).
- **Totals count the larger figure.** The `result` event's usage per model
  (which includes any model calls Claude Code makes besides the agent's own)
  is added up and compared with the sum of the per-message usage; the turn
  counts whichever is larger, field by field. A gap of more than 10% on the
  input side (input, cache reads and cache writes, where both figures are
  exact) stops the run.
- **Agents die with the driver.** Each Claude Code process starts in its own
  process group, which the driver kills on every exit path (normal end,
  error, panic, signal). The group's leader is a small guard process,
  `bracken-experiment guard`, holding a pipe from the driver: if the driver
  dies, even by `SIGKILL`, the pipe closes and the guard kills the group.
  Two more things still stop the agents: the shim and the tool workers refuse every call
  once the driver's socket closes, and the forwarding proxy, which is part
  of the driver, is gone, so Claude Code can't reach the model at all.
  Step 6 tests this by killing the driver mid-turn and checking that no
  further model request leaves and every agent process exits.

These caps are "the same model budget" in the vision's sense: the same for
every agent in every mode. A mode that needs fewer tokens finishes with less,
and the per-token measures show it.

**Measured, per run:** tokens of each kind per call, turn, agent, mode and
model, from Claude Code's event stream; wall-clock time per call and turn,
active and waiting; and rate-limit waits. The report states them next to the
estimate, and the Ground gets them as part of the run's report.

## 6. Credentials and data

**No new credential.** The agents use this machine's existing Claude Code
setup, routed through the owner's LLM proxy (decisions `d8cce1147b05` and
`e8be1670f346`). The proxy's auth value already lives in the owner's
`~/.claude/settings.json`; the experiment creates, stores and moves no
credential. How it is kept out of everything but the driver and Claude Code,
and what a leak would expose, are in section 3 (Processes, and What the
shared login exposes). In short: a leak exposes the owner's model access
through the proxy until the owner rotates the value there, and nothing else
by itself.

**Data flows.** What leaves the machine: the prompts, the task text and the
agents' tool results, sent through the owner's LLM proxy to the model
provider, as the team's own sessions already are. All of it is toy-task
content written for the experiment: no repository code, no Ground contents,
no personal data, no secrets. Claude Code's non-essential traffic is turned
off, and the forwarding proxy allows no other destination. Transcripts and
logs stay on the owner's machine, outside any repository; the driver writes
them from the event stream, which doesn't carry the auth value, and checks
each file for it before keeping it, deleting the file and stopping the run
if it's there. The Ground gets the plan, the report and the summary numbers;
transcripts are not contributed (they are large, and contributions are
permanent).

## 7. What the demo gives, and what must be built

### What `bracken-demo` already gives

- The **`Agent` trait and the measure-from-the-record design**: agents act
  only through tools, and the harness measures from the replicas, never from
  what agents say.
- **Bracken mode almost entirely**: per-agent replicas and identities, the
  task setter, bracken through `--json` (`bracken.rs`), sync and arrival
  tracking, and the report's definitions of duplicated, blind and visible
  work, availability, conflicts and policies.
- **The task's shape**: the stack language, `eval`, `check`, seeded samples
  and the correct and buggy candidates.
- **The scripted agent** as the base of the mock backend.
- **Safety**: refusing git checkouts and `GIT_DIR`, absolute paths, and the
  `--keep` path checks (with `efc537ca8e65` making them race-free).
- **Comparison** across seeds (`compare.rs`), and the rules a real-agent
  driver must follow ([demo.md](../demo.md#other-agents)).

### What must be built

- `stacklib`: modules, calls, limits, about 30 specs with references and
  candidates, change requests, public and hidden cases, the two-way grader.
- A mode-neutral core: a `Mode` trait (set up the hub and copies, the tool
  set, sync, materialize the integrated state, report writes), the event
  log, the tool and hub workers with the argument rules of section 3, and
  concurrent agents (the demo is single-threaded and runs agents in turn).
- Git mode with its local PR system; CRDT mode on Automerge; Bracken mode's
  tools and the materializer.
- The metrics over the three records, the plan, and the report.
- The agent driver: starting Claude Code as section 3 says, the sandbox
  profiles, the forwarding proxy, the shim, the `init` gate, token
  accounting from the event stream, the caps of section 5, transcripts, the
  mock backend.

That is a new crate of moderate size, not a large new system: the hardest
parts (the Ground, sync, the demo's measures) exist. Nothing needs a new
service: the agents use this machine's existing Claude Code setup.

### Build steps

Each step is one PR in a new crate, `crates/experiment` (`bracken-experiment`),
reusing `bracken-demo` as a library. None changes `crates/manyroot`, the
protocol or the `bracken` CLI. Class `cli` unless stated. The last column
lists every crate a step plans to add, so its supply-chain review knows what
to expect; "none" means only crates the workspace already has (`anyhow`,
`clap`, `serde`, `serde_json`, `tempfile`, `blake3`, `hex`) and the standard
library.

| Step | What | Depends on | Spends model quota | New crates |
|---|---|---|---|---|
| 0 | Prerequisites, already tasked: harden the demo agents against an adversarial agent (`3e31e716104f`), and handle-based run directories (`efc537ca8e65`) | | No | As those tasks say (`rustix`) |
| 1 | `stacklib`: language extension, about 30 specs in four layers, change requests, public and hidden cases, the in-library and standalone grader, candidates, tests | | No | None |
| 2 | The core and Bracken mode: `Mode` trait, run directory and hubs, event log, the tool and hub workers with section 3's argument rules, the driver's relay over Unix sockets, concurrent agents (threads, no async runtime), Bracken tools and materializer, the mock backend driving it end to end. Tests: the hostile-argument tables for every Bracken tool, with and without the rule checks | 1 | No | None (MCP's stdio transport is newline-delimited JSON-RPC, written with `serde_json`; no MCP crate) |
| 3 | Git mode: bare hub, git with section 3's fixed options and configuration, tools, the local PR system and merge rule, CI as the public checker. Tests: the hostile-argument tables for every Git tool, including `--receive-pack` | 2 | No | None (the system `git`, its path and version pinned in the plan; no git library) |
| 4 | CRDT mode on Automerge | 2 | No | `automerge` and its dependency tree, listed in the PR (class `cli`, with the supply-chain part of security review) |
| 5 | Metrics and report: graded snapshots, the six measures, the plan and its hashes, the comparison table; tested on mock runs of every mode | 2 (3 and 4 for their rows) | No | None (`blake3` for hashes) |
| 6 | The agent driver: Claude Code started as section 3 says; the agent, tools and hub `sandbox-exec` profiles; the forwarding proxy; the shim; the `init` gate; token accounting; section 5's caps, failing closed; process groups; transcripts and the auth-value check on them. Tests with a fake `claude` executable: each profile refuses each forbidden access (the owner's home, a repository, `~/.ssh`, the keychain, another agent's copy, the hub, the event log, another HTTPS host direct and through the forwarding proxy, starting any process but its own); the tools and hub profiles refuse `/bin/sh` and `/bin/bash`, the hub profile can't write any copy and the tools profile can't write the hub, and a sync both ways in every mode still works under them; the shim refuses to start with any variable set; the gate refuses a stream with another version, model, permission mode, credential source, tool, skill, plugin or MCP server, and a changed turn-configuration file or binary; caps end a fake Claude Code that streams past them or drops its usage; killing the driver mid-turn leaves no agent process and sends no further request. Then the pinned real Claude Code binary, under the final agent profile, with a throwaway home and config directory and a fake auth value, pointed at a local fake endpoint through the forwarding proxy, run to its `init` event and its first request, with the forbidden-access checks repeated against it; no model is called (finding `b2a6109bbf92`). Any profile change found after this step, in calibration or later, is a new step-6 change with its own review, not a fix made during a run. Also finds out, against a fake endpoint, whether `--max-budget-usd` stops a turn when Claude Code runs through a proxy | 2, 0 | No | None (the proxy on `std::net`; process groups through the standard library and `rustix`; `sandbox-exec` is part of macOS) |
| 7 | Prompts: base prompt and tool descriptions; neutrality review; a full mock run of all three modes | 3, 4, 5, 6 | No | None |
| 8 | Calibration run: 2 agents, 2 turns, three modes; replace section 5's assumptions with measurements and re-estimate the pilot | 7; the manager tells the owner first | Done: 1.36 M tokens counted on Opus 5.5 and 0.92 M on the Haiku 5.5 shakedown, about 5 minutes each | None |
| 9 | The 10-agent pilot, plan contributed first | 8, and the soft stop at the tool-call cap (decision `147a36d5d623`); the manager tells the owner the re-estimated use first | 27 to 47 M tokens (7.2 M output at the ceiling, an upper bound), 30 to 45 minutes; capped at 75 M tokens, 5 M output and 3 hours in total, a third per mode | None |
| 10 | Results in `docs/`, and the decision on replicates and scale | 9 | No | None |

Steps 3, 4 and 6 can be built in parallel once step 2 lands.

## 8. Decisions

The owner accepted all eight recommendations of the first draft (decision
`b29d5db02da8`), then changed the first (`d8cce1147b05`, refined by
`e8be1670f346`):

1. **Model access:** the agents use this machine's existing Claude Code
   setup as it is, routed through the owner's LLM proxy. No separate token,
   no new login, no bypass (sections 3 and 6).
2. **Model:** Claude Opus 5.5 at medium effort for the pilot, with an
   optional Haiku 5.5 shakedown of the full pipeline first, a much lighter
   use of the quota that catches harness bugs before the real run.
3. **CRDT mode is in the first run**, on Automerge. If its supply-chain
   review turns up a problem, Git runs against Bracken first and CRDT comes
   next; the pilot would then use about 39 million tokens and an hour.
4. **Task:** `stacklib` first, a real-code task second (section 9).
5. **Sync:** hub-and-spoke in every mode, with sync at turn boundaries and
   on demand. Peer-to-peer Bracken comes later as its own comparison.
6. **Git mode's merge rule:** CI passes and one approval by someone other
   than the author.
7. **Bracken's materialization rule:** current implementations with at
   least one independent pass, most passes first, then causal order, fixed
   in the plan before the run. Changed after Phase 1 by owner decision
   `a4d8622c75d5` point 2: one independent pass accepts, and rivals resolve
   by causal order alone (section 2, Mode 3); Phase 1's rule stays
   selectable to reproduce Phase 1.
8. **Quota and replicates:** the calibration run (done: 1.36 million tokens
   counted, about 5 minutes) first, then the pilot on its re-estimated use
   (27 to 47 million tokens, 7.2 million output at the ceiling; capped at 75
   million tokens, 5 million output and 3 hours for the whole pilot, a third
   of each per mode), run
   when the team is otherwise idle; replicates (about 80 to 140 million
   tokens for three) are decided after reading the pilot.

## 9. After the pilot

The pilot's and the replicates' results, and the remaining options for scale
with their expected use and caps, are in [phase1-results.md](phase1-results.md).

- **Replicates**: three or more per mode, with the mode order randomized,
  before claiming that one mode beats another.
- **A real-code task**: a small Rust or Python library with a held-out test
  suite, agent code running in a sandbox with no network and no host access
  (its design is a proposal of its own; the candidates for the next task are
  in [phase2-task.md](phase2-task.md)).
- **Scale**: 20, 50 and 100 agents, as the vision asks, each with its own
  expected use and the owner's go.
- **Bracken variants**: peer-to-peer sync, wip claims, liveness, stricter
  policies, each against Bracken's own baseline.
