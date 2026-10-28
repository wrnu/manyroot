# What Phase 1 showed about Bracken

> **Design analysis, 2026-10-28.** Task `d2ee25d369a8`, under the owner's
> decision `885847a45a8f` to work on Bracken itself next. It asks why Bracken
> mode duplicated work, integrated discoveries slowly and read so much in
> [the Phase 1 runs](phase1-results.md), separates what is Bracken from what is
> the experiment's prompt and tools, and ranks changes to Bracken.

## Sources and method

- **The three Bracken runs** of the pilot and its replicates (Ground reports
  `c0487eedf593`, `9e6d47ff7c1c`, `bd61ba5fd6cf`; combined report
  `283a1af6619a`). Each run had 10 agents for 8 turns: 240 turns and 6,863 tool
  calls in all.
- **The local transcripts** of those turns: every tool call, its arguments and
  the size and head of its result, and the private notes agents saved. They
  are paraphrased here, never quoted, and stay on the machine that ran them.
- **The code as run:** `crates/experiment/src/bracken_mode.rs` (the tools),
  `crates/experiment/prompts/` (the prompts) and the `bracken` CLI at main
  `cfbdb7a`.

Counts below are per run unless they say otherwise, given as the range over
the three runs. "Bracken" means the protocol, the `manyroot` library and the
`bracken` CLI. "The experiment" means the harness, its tools and its prompts.

## Summary

These are the main causes in this setup: one model, one prompt and a toy
task, over three runs ([What this can't show](#what-this-cant-show)).

| Question | Main cause in these runs | Whose |
|---|---|---|
| (a) Duplicated deliveries | Work waiting for validation is invisible: the only "what counts" view lists accepted work, and three quarters of implementations never got a pass. Agents also start the same functions at once, with no way to say what they're on | Bracken (no view of pending work, no intent claims), made worse by the experiment's call cap |
| (b) Slow integration | Few agents validate pending work, because finding it takes a crawl through the log. Those who do validate often add passes to work already accepted, which the experiment's acceptance rule (most passes wins) rewards. A pass arrives a turn later, when the validator next syncs | Bracken (no view of what needs a pass), and the experiment (its acceptance rule and pass counts, turn-boundary sync) |
| (c) Validation tokens and large contexts | Reading the log: 95% of what tools returned was reads, two thirds of it `log`, and 23% of `log` calls returned nothing usable because the result was too large for Claude Code to return. The full spec list was fetched 134 times, mostly to recover full ids. Notes, which could have saved re-reading, were seldom written because turns ran out at the call cap | Bracken (an unbounded, non-incremental log), and the experiment (call cap, id rule) |

The ranked proposals are in [Proposed changes](#proposed-changes).

## (a) Why agents duplicated deliveries

The report counts a delivery as duplicated if another agent's correct version
of that function had already reached the hub (visible) or arrived after this
agent last read it (blind). Bracken mode had 193 to 247 visible and 32 to 40
blind duplicates per run, 52 to 61% of its output.

Implementations here are counted from the transcripts: every successful
`artifact` call, 302 to 327 per run. The report's deliveries (274 to 323) count
only those that differed from the then-integrated version.

**Agents didn't ignore accepted work.** From turn 2 on, every delivery (213 to
237 per run) came after the agent had called `accepted` earlier in the same
turn; the 83 to 95 deliveries of turn 1 came before anything was accepted. The
function it then wrote was already listed there in only 0, 29 and 4 cases. So
agents wrote functions that `accepted` showed as having no accepted
implementation.

**But most of those functions already had an implementation waiting.** For
the same 213 to 237 deliveries per run (about 70%: every delivery after turn
1), another agent had already delivered
an implementation of the same function in an earlier turn, and in 183 to 202
cases that one was related to its spec. It just hadn't been accepted, because
nobody else had passed it yet. Only 22 to 24% of all implementations ever got a
pass from someone other than their author (69 of 308, 67 of 302, 77 of 327).
Work in that waiting state appears in no view meant to answer "what still needs
doing". The experiment's `accepted` tool lists only what counts. To find a
waiting implementation an agent has to read the log, or `show` each spec claim
and follow its `implements` relations. Agents' notes say as much: several
wrote that the artifact log was too big to read and that `show` on each spec
was the way to find candidates, at a cost they also noted.

**Agents also started the same work at once.** In turn 1, each function was
written by 2.7 to 3.1 agents on average. Across the runs, 60 to 68 times two or
more agents wrote the same function in the same turn. Nothing in Bracken lets
an agent say "I'm working on this" so that others can steer clear. The notes
show agents splitting the work by module after the fact, once they saw what
others had done.

**Many implementations could never count.** Only 109 to 164 of 302 to 327
implementations per run (36 to 54%) were related to their spec with
`implements`, which is required for them to count. Bracken's flow for one
delivery is three calls: artifact, check, relate. Agents batch them across
several functions at once. In 70 to 78 of 80 turns the 25-call cap cut the
turn short, and the batch's last calls were refused: 987 refused calls over the
three runs, the largest share of them `relate` (258), then `check` (236),
`evidence` (195) and `artifact` (158). An implementation left unrelated is invisible
to the count, so someone writes the function again.

**Whose:** Bracken has no view of pending work and no intent claims. The
experiment's `accepted` tool, its call cap and its prompt (which says what
counts, but not how to find work awaiting a pass) amplify it.

## (b) Why a discovery took 88 seconds and 12 turns to integrate

The measure is the median time from a delivery reaching the hub to it being in
the integrated state, and the number of other agents' turns in between. In
Bracken mode that needs the author's `implements` relation and a pass from
someone else.

**The non-author rule itself isn't the obstacle.** Agents followed it: of 555
verdicts on implementations whose author is known, 547 were by someone other
than the author.

**Few validate pending work, and passes go to the wrong place.** Each run had
173 to 186 independent passes, but they landed on only 67 to 77
implementations. 104 to 110 passes per run (about 60%) went to implementations
that already had one, up to 7 on one implementation. Meanwhile three quarters of
implementations never got any. The notes show why: in later turns, agents
looked for accepted implementations with only one pass and added theirs. The
experiment's acceptance rule (among rival implementations, most passes wins)
and the pass counts its `accepted` tool shows make an extra pass look useful.
Bracken's own default policy has neither: one independent pass accepts, with
no ranking by passes. And nothing, in Bracken or the experiment, shows which
related implementations have no pass at all.

**A pass comes a turn later.** For the implementations that did get a pass,
the validator's turn number was the author's plus one in 55 to 71% of cases,
and the same turn number in only 3 to 9 cases. A validator sees new work only
after a sync. The harness syncs at each turn's start and end, and agents called
`sync` themselves only 98 to 110 times in 80 turns. With 10 agents running
turns of about a minute, one turn of delay is about 10 other agents' turns and
over a minute, which matches the medians (75 to 97 s, 10 to 15 turns).

**Whose:** the lack of a view of what needs a pass is Bracken's. The
incentive to pile passes is the experiment's: its acceptance rule and the pass
counts in its `accepted` tool, which differ from Bracken's default. The sync
timing is the experiment's too: it syncs at turn boundaries, and its prompt
doesn't suggest syncing before choosing work.

## (c) Where the validation tokens and the large contexts went

**Reads are nearly everything.** Over the three runs, tools returned 8.7
million characters. 95% of that came from `log`, `show`, `accepted` and
`heads`, and 67% from `log` alone: 635 calls. Every
result stays in the conversation for the rest of the turn, so each later model
call reads it again from the cache. In the pilot, a Bracken model call read
15,100 cached tokens on average, against 6,100 in CRDT and 5,700 in Git. Its
largest context was 70,000 tokens.

**A quarter of the log reads returned nothing.** Claude Code doesn't return a
tool result over about 50,000 characters: it replaces it with a short error
saying the output was saved to a file, which the agents had no tool to read.
146 of the 635 `log` calls (23%) ended that way, from turn 2 on, with results
that would have been 50,000 to 391,000 characters. Git and CRDT never hit it.
Agents' notes record learning to avoid the unfiltered log for that reason.

**The same things were read again and again:**

| `log` call (3 runs) | Calls | Too large to return | Average returned |
|---|---|---|---|
| Everything | 77 | 63 | 33,100 characters (only early turns) |
| Artifacts | 129 | 73 | 23,300 |
| Relations | 13 | 10 | 45,300 |
| Claims | 74 | 0 | 20,700 |
| The spec claims | 134 | 0 | 13,100 |
| The change claims | 107 | 0 | 2,000 |

The averages are of the results that came back. The spec list was fetched 134 times. It never changed except at the one
release of changes. Agents re-read it mostly to recover the full 64-digit ids
the tools need. Their notes kept 8-digit prefixes, which the experiment's
argument rule (12 to 64 hex digits) refuses, and short ids caused 25 refused
calls.

**Notes seldom carried state between turns.** Bracken agents saved notes in
only 7 to 19 of 80 turns, against 44 to 46 in CRDT and 64 to 71 in Git. The
reason is the call cap. Bracken turns reached it in 70 to 78 of 80 turns (CRDT
37 to 40, Git 10 to 21), and once a turn is at the cap, the soft stop refuses
every further call, including saving notes. So most Bracken turns started from
nothing and rebuilt their picture of the Ground by reading it.

**Validation's 25% of tokens** is mostly the cost of `check` and `evidence`
calls carrying this large context. `check` was the most-called tool (1,351
calls), but its results are tiny (72 characters on average). A smaller context
makes every call cheaper, validation included.

**A cheaper view would do.** What agents needed each turn fits in a small
table: for each current spec, its id, whether something is accepted, and the
candidates waiting for a pass, with their authors and pass counts. Plus what
changed since the agent last looked. Agents assembled that from the full log,
the spec list and many `show` calls, at 27,000 to 52,000 characters of reads
a turn on average, depending on the turn number, and a quarter of their log
calls came back empty.

**Whose:** the log's shape (everything, every time, full ids and keys, no
"since", no limit and no summary) is Bracken's. The 50,000-character limit is
Claude Code's, and any agent harness will have one. The call cap's effect on notes, the
12-digit id rule and the tool set are the experiment's.

## Proposed changes

Ranked by expected effect on the three costs. Each would be measured by
re-running the 10-agent setup with the change, against the three replicates'
ranges; each such run spends model quota and needs the owner's own go
(decision `885847a45a8f` covers this analysis only).

| Rank | Change | Class | Addresses | Expected effect | How a later run measures it |
|---|---|---|---|---|---|
| 1 | **A status view per spec:** `bracken status` (or `accepted --pending`) listing, for each current spec claim, first the related implementations still waiting for an independent pass (needs a pass), with authors and ages, then the accepted implementation if any, then specs with nothing related | cli (on the library's existing accepted layer, under its default policy) | (a), (b), (c) | Waiting work becomes visible and comes first, so agents validate it instead of re-writing it or adding passes to accepted work. Validators find it in one call instead of a crawl | Visible duplicates and their output share fall from 52 to 61%; implementations with an independent pass rise from 22 to 24%; passes beyond the first (about 60%) fall; integration median falls from 75 to 97 s; `show` and `log` calls per turn fall |
| 2 | **An incremental, bounded, compact log:** `bracken log --since <cursor>` (what arrived since the agent's last read or sync, with the cursor to pass next time), a `--limit`, and a compact form with short unique ids, author names and a statement summary | cli | (c) | Each turn reads what's new, not everything, and no read is too large to return. Reads drop from about 35,000 characters a turn to a few thousand, and the context shrinks for every call | Too-large results (146 in three runs) fall to none; cached tokens per call (15,100 in the pilot) toward CRDT's and Git's; largest context; `log` result characters per turn; validation token share |
| 3 | **Intent claims:** a lightweight "I'm on this" claim tied to a spec, with an expiry, shown in the status view; Bracken's demo already has wip claims and liveness to build on | library and cli (convention first, protocol only if expiry needs it) | (a) | Fewer simultaneous starts on the same function | Same-turn collisions (60 to 68 a run now) and turn-1 implementations per function (2.7 to 3.1) fall; blind duplicates (32 to 40) fall |
| 4 | **Short ids everywhere:** print the shortest unique prefix (at least 8 digits) and accept it in every command | cli | (c) | No more re-reading the spec list for full ids; fewer refused calls | Spec-list reads (134 in three runs) and id refusals fall |
| 5 | **A one-call delivery:** `bracken artifact --implements <spec>` creating the artifact and its relation together | cli | (a) | No more implementations stranded without their relation when a turn ends | Share of implementations related to their spec (36 to 54% now) rises toward 100% |

**Classes:** none needs a protocol change. Rank 3 is a convention on claims
and tags unless expiry has to be enforced. The others are CLI views or
conveniences over what the library already computes, under its existing
default policy (one independent pass, no ranking by passes).

### Changes for the experiment, not Bracken

These would make a later run fairer to every mode and are the harness's to
make, each as a reviewed step-6 or step-7 change:

- **Match Bracken's acceptance rule.** The harness's own rule (among rival
  implementations, most passes wins, decision `b29d5db02da8` point 7) and the
  pass counts in its `accepted` tool reward piling passes on accepted work.
  Bracken's default accepts on one independent pass with no ranking. Using
  that rule, and leaving pass counts out of the tool's output, would steer
  passes to waiting work: passes beyond the first (about 60% now) should fall.

- **Let `notes_write` through after the cap.** The soft stop refuses every
  call past 25, so turns that reach the cap lose their notes. Exempting one
  `notes_write` would give every mode the memory the design intends. Bracken,
  which reaches the cap most, would gain most.
- **Use Bracken's own views in its tools.** The experiment's `accepted` is the
  harness's count. A later run should give agents Bracken's status view (rank
  1) once it exists, so the run measures Bracken.
- **Accept the ids Bracken accepts.** The 12-digit minimum is stricter than
  the CLI's prefix rule would need to be after rank 4.
- **Say where the waiting work is.** The Bracken prompt says what counts but
  not how to find work that needs a pass. Once the status view exists, the
  mode's "where things are" section should name it, as Git's names `pr_merge`.

## What this can't show

The runs used one model, one prompt and a toy task, so these causes are what
this setup showed, not general laws. Some effects interact: the call cap
strands relations and drops notes, and both raise reads and duplication, so
fixing the experiment's cap alone would lower Bracken's costs somewhat without
any change to Bracken. A run that makes one change at a time, with the
harness fixes first, would separate them.
