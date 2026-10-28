# Phase 1 experiment: calibration, pilot and replicates

Results of steps 8 and 9 of [the first Phase 1 experiment](phase1-experiment.md),
and of the two replicates after the pilot (option A below), all run on
2026-10-28. The design's measures, caps and modes are defined there; this page
gives the numbers.

**Three replicates per mode, with one model, one prompt and a toy task.** The
replicates show which differences between the modes recur in every run and
which don't ([Three replicates](#three-replicates)). They are consistent
directions, not proven effects: with three runs per mode, no significance test
is strong (the smallest two-sided permutation p-value for three runs against
three is 0.1). And `stacklib` is a toy, so a difference here is evidence about
the coordination mechanisms, not about real software (design section 4, What
this can't show).

## Summary

- **The harness held.** All 720 turns of the nine measured runs passed the
  gate and ended with a `result` event. There were no sandbox denials,
  unreadable usage, crashes, turn errors, rate-limit waits or cap events, and
  no mode stopped early. One run in replicate 2 was stopped by a false alarm in
  the separate watcher, not the harness, and was run again (below).
- **Use per replicate:** 23.5, 24.0 and 23.4 million tokens; 0.62, 0.65 and
  0.67 million output; 17, 19 and 20 minutes. That is steady from run to run.
- **Consistent across the three replicates:**
  - Bracken used about three times CRDT's tokens and four times Git's, and
    had the fewest improvements per token.
  - CRDT alone broke the library, 32 to 47 times per run, and always repaired
    it to 30 of 30.
  - Git alone had failed integrations, 5 to 11 per run.
  - Bracken and CRDT spent 52 to 66% of their output on duplicated work,
    against Git's 27 to 29%.
- **Not consistent:** Git's final quality, which was 16, 16 and 30 correct.
- **The pilot (replicate 1)**, which the sections up to
  [Three replicates](#three-replicates) describe:
  - **Use:** 23.5 million tokens, 0.62 million of them output, and 17 minutes
    for the 10-agent pilot of all three modes. That is below the re-estimate
    of 27 to 47 million tokens and 30 to 45 minutes, and well inside the
    owner's caps of 75 million tokens, 5 million output and 3 hours.
  - **The soft stop works.** Every pilot turn ended with its `result` event,
    so all output is real: 2,600 tokens a turn on average, against the 30,000
    ceiling that step 8 had to count.
  - **Final quality:** Bracken and CRDT ended with all 30 functions correct;
    Git with 16 correct and 14 wrong.

## Setup

| | Calibration (step 8) | Pilot (step 9, replicate 1) |
|---|---|---|
| Model | Claude Opus 5.5, medium effort, after a Claude Haiku 5.5 shakedown | Claude Opus 5.5, medium effort |
| Agents x turns per mode | 2 x 2 | 10 x 8 |
| Harness | Main `d83ce6b`, Claude Code 2.1.311 pinned | Main `cfbdb7a` (with the soft stop at the tool-call cap and per-mode run caps), Claude Code 2.1.311 pinned |
| Mode order, drawn at random | Opus: Git, Bracken, CRDT | Bracken, CRDT, Git |
| Plan | Ground `8ce4b1f7b4d9` | Ground `3dba55e644d4` |
| Reports | Ground `eb60e66777b3` (Haiku), `c59e23bd4be6` (Opus) | Ground `c0487eedf593`; per mode `d38c31096b44`, `5c54a75c8346`, `1078cc6bb654` |

Each run also had an automatic watcher outside the harness. It totalled usage
as the driver counts it and would have stopped the driver at the pilot-wide
caps, or on unreadable usage. It never fired. Run directories and transcripts
stay on the machine that ran them and aren't committed.

## The six measures (pilot, replicate 1)

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
  output cap, and an agent whose every turn reached the call cap would have
  stopped after 6 of its 8 turns (120 of the 240 turns reached it).

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

## Patterns the pilot suggested

These came from the pilot alone, as questions for the replicates.
[Three replicates](#three-replicates) answers them: all held except Git's
final quality and CRDT's lead in net improvements per token, which Git took in
replicate 3. Whether the task is too easy stays open.

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

## Three replicates

The owner chose option A below (approval `fca648d64ce6`): two more replicates
of the pilot, each with its own plan and drawn mode order, the same harness
(`cfbdb7a`), binaries, prompts, commands and caps.

| | Replicate 1 (the pilot) | Replicate 2 | Replicate 3 |
|---|---|---|---|
| Mode order | Bracken, CRDT, Git | Git, Bracken, CRDT | Bracken, Git, CRDT |
| Plan | Ground `3dba55e644d4` | Ground `658f0ab4eec6` | Ground `c1c6960425ef` |
| Report | Ground `c0487eedf593` | Ground `9e6d47ff7c1c` | Ground `bd61ba5fd6cf` |
| Tokens, output | 23.46 M, 0.62 M | 24.00 M, 0.65 M | 23.44 M, 0.67 M |
| Wall-clock | 17.2 minutes | 18.6 minutes | 19.9 minutes |
| Turns with a `result` event | 240 of 240 | 240 of 240 | 240 of 240 |

The combined report is Ground `283a1af6619a`. In replicate 2, the watcher
outside the harness stopped the first Bracken run during turn 7 (57 of its 80
turns had ended), on a false
alarm: it read a usage row the driver was still writing (Ground
`c1c112cf9315`). The usage file was intact and the harness had no fault. That
run is left out of the measures. Bracken was run again in a new directory
with the watcher fixed, and the stopped run's 11.9 million tokens and 0.27
million output counted toward replicate 2's caps (35.9 million and 0.92
million in all, of 75 and 5 million).

### The six measures across the replicates

Mean over the three replicates, with the lowest and highest run in brackets.

| Measure | Bracken | CRDT | Git |
|---|---|---|---|
| Tokens | 14.7 M [14.6 to 14.9] | 5.1 M [5.0 to 5.3] | 3.8 M [3.6 to 4.0] |
| Output | 350,000 [334,000 to 375,000] | 183,000 [177,000 to 195,000] | 113,000 [102,000 to 122,000] |
| **1. Improvements**: net | 39.7 [36 to 42], no losses | 44 [44 to 44]: +137, -93 | 34.7 [30 to 44], no losses |
| per million tokens | 2.69 [2.44 to 2.88] | 8.64 [8.30 to 8.85] | 9.11 [7.55 to 11.47] |
| per million output tokens | 113 [108 to 120] | 240 [225 to 248] | 308 [246 to 385] |
| **2. Duplicated work**: deliveries | 293 [274 to 323] | 268 [217 to 307] | 111 [95 to 140] |
| duplicated: visible, blind | 218 [193 to 247], 37 [32 to 40] | 236 [181 to 277], 0 | 78 [57 to 110], 2.7 [0 to 8] |
| share of output tokens | 55% [52 to 61] | 65% [65 to 66] | 27% [27 to 29] |
| **3. Overhead**: tokens, integrate | 3% [2 to 3.5] | 16% [15 to 16] | 26% [23 to 27] |
| tokens, validate | 25% [24 to 26] | 8% [8 to 9] | 10% [9 to 11] |
| time, integrate | 0.6% [0.4 to 0.8] | 0.7% [0.4 to 1.0] | 7% [2.6 to 14] |
| failed integrations | 0 | 0 | 9 [5 to 11] |
| **4. Useful discoveries** | 291 [273 to 319] | 93 [79 to 103] | 111 [95 to 140] |
| integrated: count, median time | 156 [140 to 183], 88 s [75 to 97] | 93 [79 to 103], at once | 68 [57 to 75], 29 s [15 to 46] |
| taken up, median time | 29 s [23 to 35] | 0.7 s [0.4 to 1.1] | 5.9 s [2.3 to 12.5] |
| **5. Incompatible contributions**: broken in library | 0 | 38 [32 to 47], across 6 to 7 changes | 0 |
| modules broken | 0 | 3.7 [2 to 5] | 0 |
| semantic-conflict events | 0 | n/a | 0 |
| **6. Final quality**: correct of 30 | 30, 30, 30 | 30, 30, 30 | 16, 16, 30 (14, 14 and 0 wrong) |

One bug report, in Bracken in replicate 2, was the only one in nine runs. The
other useful discoveries are deliveries that passed the grader standalone.

### What three replicates support, and what they don't

They support these as consistent directions: over the three runs, each
mode's range on the measure doesn't overlap the others'.

- **Bracken costs more.** It used 2.8 to 3.0 times CRDT's tokens and 3.7 to
  4.1 times Git's, and 1.7 to 2.1 times CRDT's output and 2.7 to 3.4 times
  Git's. It had the fewest improvements per token. The extra goes into model
  calls, a context that grows with the Ground (up to 70,000 tokens in one call
  in the pilot), and validation (a quarter of its tokens).
- **CRDT breaks and repairs.** Only CRDT broke the library, 32 to 47 times per
  run, with up to 5 changes after which a module didn't parse. It ended at 30
  of 30 every time. Its net improvements were the same, 44, in all three runs.
- **Git pays at the gate.** Only Git had failed integrations (5 to 11). It
  spent the largest share of tokens integrating (23 to 27%), and it had the
  fewest duplicated deliveries (27 to 29% of output).
- **Bracken and CRDT duplicate work.** They spent 52 to 66% of their output
  on deliveries another agent had already made correctly.
- **Bracken integrates slowest.** Its median discovery took 75 to 97 seconds
  and 10 to 15 turns to be integrated, because it needs a non-author's pass.
  CRDT's are integrated as they're written, and Git's took 15 to 46 seconds.

They don't support:

- **Git's final quality.** It ended with 14 of 30 wrong in replicates 1 and 2
  and with all 30 correct in replicate 3. The pilot's pattern didn't hold.
- **Git's improvements per token against CRDT's.** Git's range (7.55 to
  11.47) contains CRDT's (8.30 to 8.85).
- **Anything beyond this setup.** All agents are one model with one prompt,
  the task is a toy, and the final quality saturates: Bracken and CRDT reached
  30 of 30 in every run. A harder task, fewer turns or more agents would test
  whether the costs above buy anything that this task can't show.

Why Bracken mode cost what it did, and what to change in Bracken, is analysed
in [phase1-bracken-lessons.md](phase1-bracken-lessons.md).

## Options for the next runs

Each option needs the owner's own go (approval `3418fe14b8ed` covered the pilot
only). Estimates scale from the pilot's measured use. The owner chose option A
(approval `fca648d64ce6`), and it ran as [Three replicates](#three-replicates)
describes, at 23.4 to 24.0 million tokens per replicate. B and C remain
options.

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
