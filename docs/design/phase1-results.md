# Phase 1 experiment: calibration and pilot results

Results of steps 8 and 9 of [the first Phase 1 experiment](phase1-experiment.md),
run on 2026-10-28. The design's measures, caps and modes are defined there; this
page gives the numbers.

**This is one replicate per mode. It supports no claim that one mode beats
another** (design section 4, What this can't show). Model runs vary, so one run
per mode calibrates use and checks the harness. Only three or more replicates
per mode, with the mode order randomized, can separate the modes from luck. The
patterns below are worth testing, not conclusions.

## Summary

- **The harness held.** In the pilot, all 240 turns passed the gate. There
  were no sandbox denials, unreadable usage, crashes, turn errors, rate-limit
  waits or cap events, and no mode stopped early.
- **Use:** 23.5 million tokens, 0.62 million of them output, and 17 minutes
  for the 10-agent pilot of all three modes. That is below the re-estimate of
  27 to 47 million tokens and 30 to 45 minutes, and well inside the owner's
  caps of 75 million tokens, 5 million output and 3 hours.
- **The soft stop works.** Every pilot turn ended with its `result` event, so
  all output is real: 2,600 tokens a turn on average, against the 30,000
  ceiling that step 8 had to count.
- **Final quality:** Bracken and CRDT ended with all 30 functions correct; Git
  with 16 correct and 14 wrong.

## Setup

| | Calibration (step 8) | Pilot (step 9) |
|---|---|---|
| Model | Claude Opus 5.5, medium effort, after a Claude Haiku 5.5 shakedown | Claude Opus 5.5, medium effort |
| Agents x turns per mode | 2 x 2 | 10 x 8 |
| Harness | Main `d83ce6b`, Claude Code 2.1.311 pinned | Main `cfbdb7a` (with the soft stop at the tool-call cap and per-mode run caps), Claude Code 2.1.311 pinned |
| Mode order, drawn at random | Opus: git, Bracken, CRDT | Bracken, CRDT, git |
| Plan | Ground `8ce4b1f7b4d9` | Ground `3dba55e644d4` |
| Reports | Ground `eb60e66777b3` (Haiku), `c59e23bd4be6` (Opus) | Ground `c0487eedf593`; per mode `d38c31096b44`, `5c54a75c8346`, `1078cc6bb654` |

Each run also had an automatic watcher outside the harness. It totalled usage
as the driver counts it and would have stopped the driver at the pilot-wide
caps, or on unreadable usage. It never fired. Run directories and transcripts
stay on the machine that ran them and aren't committed.

## The six measures (pilot)

| Measure | Bracken | CRDT | Git |
|---|---|---|---|
| **1. Improvements**: net (+, -) | 41 (+41, -0) | 44 (+135, -91) | 30 (+30, -0) |
| per million tokens | 2.76 | 8.85 | 8.31 |
| per million output tokens | 120 | 248 | 294 |
| **2. Duplicated work**: deliveries | 274 | 217 | 95 |
| duplicated: visible, blind | 193, 38 | 181, 0 | 57, 8 |
| share of output tokens | 51.6% | 64.6% | 26.8% |
| **3. Overhead**: tokens, integrate / validate | 3.4% / 26.4% | 15.9% / 8.2% | 27.2% / 9.4% |
| time, integrate / validate | 0.4% / 0.6% | 0.4% / 0.0% | 4.1% / 0.3% |
| failed integrations | 0 | 0 | 11 |
| **4. Useful discoveries** (bug reports) | 273 (0) | 79 (0) | 95 (0) |
| available: count (median s, turns) | 273 (19.4 s, 2) | 79 (0 s, 0) | 95 (1.1 s, 0) |
| integrated | 144 (74.7 s, 10) | 79 (0 s, 0) | 57 (27.3 s, 6) |
| taken up | 273 (23.3 s, 2) | 79 (0.5 s, 0) | 95 (2.3 s, 0) |
| **5. Incompatible contributions**: broken in library (changes) | 0 (0) | 47 (6) | 0 (0) |
| modules broken | 0 | 5 | 0 |
| semantic-conflict events | 0 | n/a | 0 |
| **6. Final quality**: correct of 30, wrong, missing | 30, 0, 0 | 30, 0, 0 | 16, 14, 0 |
| Graded snapshots | 18 | 67 | 6 |

## Use

| | Bracken | CRDT | Git | Pilot |
|---|---|---|---|---|
| All tokens | 14,875,880 | 4,973,479 | 3,609,643 | **23,459,002** |
| Output (real, thinking included) | 341,589 | 177,764 | 102,052 | **621,405** |
| Cache reads | 12.44 M | 4.11 M | 3.17 M | 19.72 M |
| Cache writes | 2.09 M | 0.68 M | 0.34 M | 3.11 M |
| New input | 1,652 | 1,358 | 1,104 | 4,114 |
| Model calls (per turn) | 826 (10.3) | 679 (8.5) | 552 (6.9) | 2,057 (8.6) |
| Output per turn: average (range) | 4,270 (799 to 6,756) | 2,222 (737 to 4,327) | 1,276 (373 to 4,968) | 2,589 |
| Largest context of one call | 70,214 | 15,076 | 15,271 | |
| Wall-clock | 500 s | 281 s | 249 s | **17 min** |
| Turn duration, average | 57 s | 32 s | 23 s | |
| Rate-limit waits | 0 | 0 | 0 | 0 |

Against the caps the owner approved for the pilot (75 million tokens, 5
million output and 3 hours in total, a third of each per mode): the pilot used
31% of the tokens, 12% of the output and 9% of the time. The largest mode,
Bracken, used 60% of its 25 million share. The re-estimate from step 8 (27 to
47 million tokens) was close on tokens. Its output bound (7.2 million at the
ceiling) was 12 times the real output, as expected for a ceiling. Its time (30
to 45 minutes) was about twice the real time, because turns were shorter than
the calibration's.

Through the gateway, almost all input arrives as cache reads and writes.

## The soft stop

In step 8, every turn (24 of 24, Haiku and Opus) ended at the 25-tool-call
cap by a kill, with no `result` event. So each turn's output was counted at the
fail-closed ceiling of 30,000 tokens, and the real output was unknown. The
soft stop (decision `147a36d5d623`) lets a turn at the cap finish on its own
within a grace period, instead of being killed.

In the pilot:

- 240 of 240 turns ended with a `result` event. None was hard-stopped, and
  none was counted at the ceiling.
- 120 turns reached the 25-call cap and then ended within the grace: 70 in
  Bracken, 40 in CRDT and 10 in Git.
- The real output averaged 2,589 tokens a turn, against the 30,000 ceiling.
  Without the soft stop, the pilot would have counted up to 7.2 million output
  (5.4 million under the per-agent caps). That is over the owner's 5 million
  output cap, and each agent would have stopped after 6 of its 8 turns.

## The `--max-budget-usd` re-check

The per-turn stop is set at 1 USD of Claude Code's API-equivalent estimate,
which isn't money spent. Step 8 couldn't re-check it, because no turn produced
a `result` event, which carries the estimate. In the pilot, the estimate per
turn averaged 0.25 USD in Bracken, 0.10 in CRDT and 0.05 in Git, and was at most
0.54 (Bracken). No turn reached the stop, and the largest turn was about half
of it.

## Calibration (step 8)

| | Haiku 5.5 shakedown | Opus 5.5 calibration |
|---|---|---|
| Tokens counted | 0.92 M | 1.36 M (Git 446,000, Bracken 552,000, CRDT 359,000) |
| Of them output | 360,000, all at the ceiling | 360,000, all at the ceiling |
| Model calls | 65 | 104 |
| Turns at the call cap, hard-stopped | 12 of 12 | 12 of 12 |
| Wall-clock for the three modes | about 5 minutes | about 5 minutes |
| Harness faults | None | None |

The calibration showed that the cap decided each turn's end. That led to the
soft stop, and to the decision that the pilot's caps are pilot-wide, with each
mode's run capped at a third (decision `ea053c2c4d48`).

## Patterns worth testing

These come from one run per mode. Each is a question for the replicates, not
a finding.

- **CRDT's churn and breakage.** CRDT made the most graded snapshots (67) and
  the most gross improvements (+135). It also made the most regressions (-91),
  47 breaks in the library across 6 changes, and 5 changes after which a
  module didn't parse. It still ended at 30 of 30 and had the most net
  improvements per token. Does an ungated shared document reliably trade
  breakage for speed, and does the final state recover every time?
- **Git's final quality and failed integrations.** Git used the fewest tokens
  and had the highest improvements per output token. But it ended with 14 of
  30 functions wrong, had 11 failed integrations, and spent the largest share
  of tokens integrating (27%). Is the PR gate the bottleneck in 8 turns, and
  do wrong functions reach main through it, or stay there because nothing
  fixes them?
- **Bracken's token use.** Bracken used three to four times the tokens of the
  others: more model calls per turn, the largest context (70,000 tokens, as
  the Ground grows) and the largest validation share (26%). In return it had
  no breaks and 30 of 30. Its median time to an integrated discovery was the
  longest (75 s, 10 turns): it needs a non-author's pass. Does its use grow
  faster than the other modes' with more agents and turns?
- **Duplicated work.** CRDT and Bracken spent 65% and 52% of their output on
  deliveries another agent had already made correctly. Git spent 27%. With
  every function visible to every agent, do agents coordinate who does what,
  or redo it?
- **The task may be too easy to separate two modes.** Bracken and CRDT both
  reached 30 of 30 in 8 turns. Final quality may saturate, so the measures
  that don't (per token, time, breakage) carry the comparison. A harder task,
  or fewer turns, would test that.

## Options for the next runs

Each option needs the owner's own go (approval `3418fe14b8ed` covered the pilot
only). Estimates scale from the pilot's measured use.

| Option | What | Expected use | Caps it would need |
|---|---|---|---|
| **A. Two more replicates** (three in all, the minimum for a claim) | Two more 10-agent pilots of all three modes, as run, each with its own drawn mode order, in separate sessions | About 47 M tokens, 1.2 M output and 35 minutes in all (23.5 M, 0.62 M and 17 minutes each) | Unchanged for each replicate: 75 M tokens, 5 M output and 3 hours, a third per mode (25 M, 1,666,666, 1 hour). Bracken used 60% of its mode share, so a replicate that varies by more than about 1.7 times would stop at the cap; its report would still be written |
| **B. One 20-agent pilot** | 20 agents x 8 turns per mode, otherwise as run | 47 to 70 M tokens (twice the agent turns, plus up to half again for a larger state), 1.2 to 1.9 M output, 30 to 60 minutes | Tokens 150 M in all, 50 M per mode: Bracken alone would be 30 to 45 M, over today's 25 M share. Output can stay at 5 M (1,666,666 per mode). Time 3 hours, 1 hour per mode. The per-agent cap of 3 M tokens would need checking for Bracken (1.5 M per agent in the pilot) |
| **C. Three 20-agent replicates** | Option B, three times | 140 to 210 M tokens, 3.7 to 5.6 M output, 1.5 to 3 hours, over separate sessions | As B, for each replicate |

20 concurrent sessions are more likely to meet the subscription's rate limits
than 10. The driver pauses and resumes an agent that gets a rate-limit error,
and leaves the waits out of the time measures. The pilot had none.

Option A is what the design asks for before any claim (section 9). B and C
start on scale, which the vision asks for, and each gives a 20-agent baseline.
