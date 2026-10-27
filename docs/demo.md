# The demo harness

`bracken-demo` runs a few agents on a toy task through the real `bracken`
binary. Each agent has its own identity and its own replica of one Ground,
replicas exchange bundles between rounds, and the run ends with a measured
report. It's the first step toward the Phase 1 experiment in
[vision.md](vision.md): the same measurements, on a run small enough to read.

## Running it

```sh
cargo build -p bracken            # the demo drives this binary
cargo run -p bracken-demo -- --agents 4 --rounds 5 --seed 1
```

The demo uses the `bracken` that cargo built next to it (`target/<profile>/`),
or the one you pass with `--bracken <path>`. That path is resolved once, to an
absolute path. A relative one is relative to the current directory, and `PATH`
is never searched. Rebuild bracken after changing it, or the demo runs the old
one.

| Option | Default | Meaning |
|---|---|---|
| `--agents N` | 4 | Agents, 2 to 16 |
| `--rounds N` | 5 | Rounds of work |
| `--seed N` | 1 | Seeds the agents and the gossip topology |
| `--topology full\|ring\|gossip` | ring | Who imports whose bundle after a round: everyone's; the previous agent's; one random agent's |
| `--sample N` | 6 | Inputs an agent checks per implementation |
| `--work-rounds N` | 0 | Rounds between starting an implementation and delivering it |
| `--schedule lockstep\|stagger\|jitter` | lockstep | When agents start work relative to each other (see [Work in progress](#work-in-progress)) |
| `--wip` | | Announce work with a `wip` claim before doing it, and skip specs others have announced |
| `--stale-after N` | see below | Rounds after its own round at which an open wip claim is stale |
| `--keep DIR` | | Keep the replicas in `DIR` (must not exist), instead of a temporary directory |
| `--allow-in-repo` | | Allow the run's directory inside a git checkout (refused by default) |
| `--json` | | Print the report as JSON |

The same seed and options give the same report, apart from `timing`. The demo
exits non-zero if the replicas didn't converge.

Every replica holds its agent's private key, unencrypted, under
`.bracken/identities/`. The keys are throwaway, but they're still keys, so the
demo writes a `.gitignore` that ignores everything at the top of the run's
directory. It also refuses to run in a directory inside a git checkout (asking
`git rev-parse`, and looking for a `.git` itself in case git isn't installed),
or with `GIT_DIR` or `GIT_WORK_TREE` set, unless you pass `--allow-in-repo`, so
agents never work with a repository's files, or its Ground, in reach. Every
path the demo hands to bracken is absolute, so a relative `--keep` stays where
you asked. A `--keep` path with `..` after a directory that doesn't exist yet
is refused, because creating it would follow the `..` to a place the checks
never looked at. The demo resolves the part of the path that exists once,
checks the directory beneath that real path, and makes each new directory
there without resolving the path as typed again. If a check refuses, or a
directory it made was swapped for something else meanwhile, it removes the
directories it made, but only those that are still the very ones it made.

These checks refuse paths in or through a checkout as they stand when the demo
looks, but they can't protect against someone who can swap directories in the
path while the demo runs: the run's files, keys included, are written by path,
so a well-timed swap can send them, or a cleanup, somewhere else. So `--keep`
must not point into a directory that other users or processes can write to.
A fix, creating and writing the run through directory handles instead of
paths, is tracked as task `efc537ca8e65`.
`compare` runs each seed in the system's temporary directory (`TMPDIR`), with
the same refusal; it takes `--work-rounds` 0 to 10 and `--jobs` 1 to 64.

To look around a run's Ground afterwards, keep it outside any repository and
point bracken at a replica:

```sh
cargo run -p bracken-demo -- --keep /tmp/demo
target/debug/bracken --dir /tmp/demo/agent-0/.bracken log
target/debug/bracken --dir /tmp/demo/agent-0/.bracken conflicts
```

## The toy task

Agents implement six small integer functions (`abs`, `max2`, `min2`, `sign`,
`clamp3`, `dist`) in a tiny stack language. `bracken-demo task` prints the
language and the specs. The task suits the demo because:

- **Answers are checkable.** A program either matches the reference on every
  input or it doesn't, and the checker is a few dozen lines, with no compiler
  or sandbox.
- **Checks can miss bugs.** Some wrong programs fail on only a few inputs, and
  an agent checks only a random sample, so one passing check doesn't prove
  much. That's what makes a validation policy matter.
- **A model could do it.** The language fits in a paragraph, so a later
  real-model agent can take the same task.

`bracken-demo check <spec> <file> [--sample N --seed S]` runs the same check
an agent runs. Without `--sample` it checks every input.

## How a run goes

1. A task setter creates the Ground and contributes a task claim (the language)
   and one `spec` claim per function, each refining the task. It exports a
   bundle and never acts again.
2. Each agent joins the Ground from that bundle into its own directory and
   creates its own identity.
3. In each round, the agents act one after another, each on its own replica
   only. Then every replica exports a bundle and imports from its sources in
   the topology.
4. After the last round, a final sync has every replica import every other
   one's bundle, and the report is computed.

The shipped agent is scripted and deterministic given the seed. Each round it:

- first accepts other agents' fixes of its own implementations, by relating
  each fix as superseding its implementation itself. A `supersedes` relation
  only counts when the replaced contribution's author writes it (protocol.md,
  "Supersession"), so until then another agent's fix is a proposal and the
  old implementation stays current;
- fixes its own failing implementation if it has one (a new artifact that
  `supersedes` it), sometimes fixes someone else's instead, or else implements
  a spec it sees no live, unfailed implementation of. Sometimes it implements
  a spec someone already covers;
- checks up to two implementations by others that it hasn't checked, on a
  seeded sample of inputs, and records `pass` or `fail` evidence;
- sometimes follows a failed check with a `finding` claim that `contradicts`
  the implementation.

Each agent draws its own skill and habits from the seed. It only knows what its
replica holds, and nothing tells agents what others are working on, so agents
working at the same time often pick the same spec.

### Work in progress

By default an agent delivers an implementation in the turn it decides to write
it. With `--work-rounds W`, it delivers W rounds after starting, and each round
it has a 10% chance of dropping the work instead. With `--wip`, it also:

- contributes a claim tagged `wip` and the spec's name when it starts
  (`Work in progress: agent-1 is implementing max2 (round 3)`), refining the
  spec;
- retracts that claim when it delivers (`delivered in round 4`). When it drops
  the work, half the time it retracts the claim (`abandoned in round 4`) and
  half the time it goes silent, so the claim goes stale;
- doesn't start a spec that another agent's open wip claim covers, unless that
  claim is stale: `--stale-after` rounds have passed since the round it
  states. Every agent reaches the same verdict. A start that takes over stale
  claims `refines` them, saying so: only a claim's own author can supersede
  it, so a stale claim stops blocking by age alone, and the takeover records
  who picked the work up.

The default stale window is one round after the latest the work could be due:
W + 1, or W + 2 under `jitter`. A claim is never stale while its work is on
time.

`--schedule` sets when agents start, in both modes alike:

- **lockstep**: everyone starts in round 1, and all work takes exactly W
  rounds, so all agents choose in the same rounds;
- **stagger**: agent i first starts in round 1 + (i mod (W + 1)), as if the
  agents had joined at different times;
- **jitter**: staggered first starts, and each piece of work takes W or W + 1
  rounds at random, so agents drift apart over time.

The plain demo is `--work-rounds 0 --schedule lockstep` without wip: the same
agents and the same numbers as before these options existed.

## What the numbers mean

The harness computes everything from the replicas, through bracken's JSON,
not from what agents say they did. Counts cover the agents' contributions; the
setter's are left out.

| Line | Meaning |
|---|---|
| Contributions | Agents' contributions, by kind and by agent |
| Work | Implementations (artifacts that `implement` a spec, by the artifact's author); fixes (implementations their author relates as superseding another, whether or not the replaced one's author has accepted it yet); checks (evidence) and their verdicts; findings (claims tagged `finding`) |
| Duplicated work | New implementations (not fixes) of a spec that another agent had already implemented anywhere in the Ground. **Blind**: the earlier one hadn't reached this agent's replica yet, so only better sync or visible work in progress would have avoided it. **Visible**: the agent's replica already held it. Competing fixes aren't counted here; they show up as divergences once the replaced implementation's author has accepted more than one |
| Concurrent work | Implementations started while another agent's implementation of the same spec was started and not yet delivered: the duplication that announcing work could prevent. The start round comes from the `implements` relation's note (`started in round N`). A pair started in the same round counts once, as **same round**: no sync could have shown either agent the other's start. **Later** means the other work started in an earlier round. Dropped work leaves no artifact, so it isn't counted |
| Wip claims | Shown with `--wip`: claims made, retracted on delivery, retracted on abandonment, taken over when stale (refined by another agent's wip claim), and **ignored while fresh**: claims made while the author's replica held another agent's open, fresh wip claim on the same spec. The scripted agent never does that, so it's always 0 |
| Availability | For each contribution and each other agent, the number of sync rounds until that agent's replica held it: 1 means the sync right after the round it was made. The final all-to-all sync counts as round `rounds + 1`, and the line says how many pairs only arrived then |
| Conflicts | What `bracken conflicts` reports at the end: divergences (one implementation superseded by two current ones) and contradictions (a current finding contradicting a current implementation). A fix resolves the contradictions against an implementation once it counts: when the implementation's author wrote the fix, or accepted it. Until then, someone else's fix leaves them open. The relation counts show how many of each kind agents made; the supersedes count includes acceptances |
| Ground truth | The harness checks every implementation on every input. "Caught" means a wrong one got at least one failing check from another agent |
| Policy table | `bracken accepted --min-passes N` for N = 1, 2, 3, with any agent as a validator. **accepted** counts subjects meeting the policy, including **superseded** ones; **impls**, **correct** and **wrong** are the accepted implementations by ground truth; **specs solved** counts specs with an accepted, not superseded, correct implementation; **conflicts** are those in the accepted layer |
| Replicas | Whether every replica ended with the same contributions |
| Timing | Wall time and bracken calls. The only part that changes between runs with the same seed |

Results computed before 2026-10-27, including PR #4's runs and the first
version of the compare table below (PR #10), used the old supersession rule,
under which any agent's fix retired an implementation at once and a takeover
superseded a stale wip claim. They count more divergences: seeds 1 to 3 over
10 rounds had 3, 4 and 3 then, and 0, 0 and 0 now. Those divergences were
races: another agent fixed a failing implementation while its author fixed
it too. Now the author's own fix counts and the other stays a proposal on a
retired implementation. Of the 14 fixes of someone else's work in those
three runs, the author's own fix won 11, one was accepted, and two arrived
after their target was already retired or the run had ended.

A sample, from `--agents 4 --rounds 5 --seed 1` (it has no fixes, so it's the
same under both rules):

```
Bracken demo: 4 agents, 5 rounds, seed 1, ring sync, checks on 6 sampled inputs, 6 specs

Contributions    75 by agents: artifact 16, claim 5, evidence 28, relation 26
                 agent-0 21, agent-1 18, agent-2 18, agent-3 18
Work             16 implementations (0 fixes), 28 checks (20 pass, 8 fail), 5 findings
Duplicated work  10 of 16 new implementations repeat another agent's spec (8 blind, 2 visible)
Concurrent work  6 implementations started while another agent's was under way (6 started the same round, 0 later)
Availability     1.90 sync rounds on average until another agent holds a contribution (median 2, max 3; 61 of 225 only in the final sync)
                 by rounds: 1: 75, 2: 97, 3: 53
Conflicts        0 divergences, 5 contradictions open at the end (from 0 supersedes and 5 contradicts relations)
Ground truth     10 implementations correct, 6 wrong; 6 of the wrong ones got a failing check

Policy         accepted  superseded  impls  correct  wrong  specs solved  conflicts
min passes 1         9           0      9        9      0         5 of 6          0
min passes 2         9           0      9        9      0         5 of 6          0
min passes 3         1           0      1        1      0         1 of 6          0

Replicas         converged: all 4 hold the same 88 contributions
Timing           5.5 s wall, 228 bracken calls (5.5 s)
```

## Experiments

### Does announcing work cut duplication?

The first result (Ground note `bd7882c5dcae`) was that most
duplicated work is blind: agents repeat a spec before the earlier work reaches
them. The vision's answer is visible claims of work in progress, so this
experiment adds them (`--wip`) and compares runs with and without:

```sh
cargo build --release -p bracken -p bracken-demo
target/release/bracken-demo compare            # seeds 1..10, ring and gossip, W = 1 and 2, all schedules
```

Every row is the mean over seeds 1 to 10, with 4 agents and 12 rounds: enough
rounds for jittered work to drift apart. "Accepted" is under min passes 2.
All 240 runs converged, and no agent ever ignored a fresh wip claim. The table
was re-run on 2026-10-27 under the current supersession rule; the first
version (PR #10) differed by at most 0.5 in any mean and 3 in contributions
or bracken calls, and the findings below hold under both.

| Topology | W | Schedule | Wip | Concurrent starts (same round) | Duplicates (blind/visible) | Accepted (correct) | Specs solved | Contributions | Bracken calls | Wip claims (taken over) |
|---|---|---|---|---|---|---|---|---|---|---|
| ring | 1 | lockstep | off | 6.0 (5.4) | 8.1 (4.5/3.6) | 12.3 (11.6) | 5.8 | 113 | 407 |  |
| ring | 1 | lockstep | on | 5.8 (5.4) | 8.1 (4.5/3.6) | 12.1 (11.5) | 5.8 | 182 | 476 | 22.4 (4.2) |
| ring | 1 | stagger | off | 7.1 (1.3) | 8.1 (1.6/6.5) | 11.8 (11.1) | 5.3 | 114 | 408 |  |
| ring | 1 | stagger | on | 3.8 (1.0) | 7.5 (1.8/5.7) | 10.5 (9.8) | 5.6 | 184 | 478 | 22.1 (7.9) |
| ring | 1 | jitter | off | 5.8 (1.8) | 5.7 (2.1/3.6) | 7.9 (7.7) | 4.7 | 82 | 376 |  |
| ring | 1 | jitter | on | 3.5 (1.7) | 5.2 (1.7/3.5) | 8.2 (7.4) | 5.2 | 131 | 424 | 17.6 (3.2) |
| ring | 2 | lockstep | off | 3.7 (2.4) | 4.5 (2.4/2.1) | 6.9 (6.7) | 4.1 | 69 | 362 |  |
| ring | 2 | lockstep | on | 3.1 (2.5) | 4.3 (2.3/2.0) | 6.7 (6.5) | 4.1 | 123 | 416 | 16.9 (4.6) |
| ring | 2 | stagger | off | 4.8 (0.6) | 5.0 (1.9/3.1) | 6.8 (6.4) | 3.8 | 65 | 358 |  |
| ring | 2 | stagger | on | 2.5 (0.8) | 3.8 (2.1/1.7) | 6.4 (5.9) | 4.7 | 107 | 400 | 15.3 (2.3) |
| ring | 2 | jitter | off | 3.5 (0.5) | 3.7 (1.4/2.3) | 4.6 (4.0) | 3.1 | 54 | 347 |  |
| ring | 2 | jitter | on | 1.7 (0.7) | 2.8 (0.9/1.9) | 3.9 (3.6) | 3.2 | 98 | 391 | 14.8 (2.1) |
| gossip | 1 | lockstep | off | 6.4 (5.9) | 6.7 (3.3/3.4) | 12.3 (11.4) | 5.9 | 113 | 406 |  |
| gossip | 1 | lockstep | on | 6.3 (6.0) | 6.7 (3.3/3.4) | 12.3 (11.4) | 5.9 | 182 | 475 | 21.8 (4.8) |
| gossip | 1 | stagger | off | 7.8 (1.4) | 7.5 (2.5/5.0) | 11.3 (11.0) | 5.7 | 116 | 409 |  |
| gossip | 1 | stagger | on | 5.6 (2.5) | 6.8 (2.4/4.4) | 10.9 (10.4) | 5.6 | 171 | 464 | 21.4 (4.2) |
| gossip | 1 | jitter | off | 5.4 (1.8) | 6.1 (2.3/3.8) | 8.1 (7.9) | 5.0 | 88 | 382 |  |
| gossip | 1 | jitter | on | 3.3 (1.7) | 4.4 (1.5/2.9) | 7.6 (7.0) | 5.4 | 133 | 426 | 17.3 (2.8) |
| gossip | 2 | lockstep | off | 3.9 (2.4) | 4.4 (2.0/2.4) | 6.6 (6.3) | 3.9 | 69 | 362 |  |
| gossip | 2 | lockstep | on | 3.5 (2.5) | 4.9 (2.2/2.7) | 6.9 (6.5) | 4.0 | 118 | 411 | 17.0 (4.1) |
| gossip | 2 | stagger | off | 4.6 (0.8) | 4.4 (1.9/2.5) | 6.8 (6.6) | 3.7 | 63 | 356 |  |
| gossip | 2 | stagger | on | 2.9 (1.2) | 4.5 (1.5/3.0) | 6.5 (6.1) | 4.6 | 114 | 407 | 15.9 (3.4) |
| gossip | 2 | jitter | off | 3.2 (0.6) | 3.8 (1.1/2.7) | 4.7 (4.6) | 3.1 | 54 | 347 |  |
| gossip | 2 | jitter | on | 2.3 (1.1) | 3.3 (0.7/2.6) | 4.6 (4.5) | 3.6 | 94 | 387 | 14.4 (2.5) |

**What changes between rows matters more than wip.**

- **The schedule decides whether wip can help at all.** In lockstep every agent
  chooses in the same rounds, so almost every concurrent start is a same-round
  start (5.4 of 6.0 on ring at W = 1). Wip can't help agents who all choose in
  the same round, and it doesn't: 6.0 becomes 5.8. Staggered and jittered
  schedules turn most concurrent starts into later-round ones (1.3 of 7.1 are
  same-round under stagger), and those are the ones a claim can prevent.
- **With the agents out of step, wip roughly halves concurrent starts:** on
  ring, from 7.1 to 3.8 (stagger) and 5.8 to 3.5 (jitter) at W = 1, and 4.8 to
  2.5 and 3.5 to 1.7 at W = 2. Gossip spreads claims more slowly and unevenly,
  so the gain is smaller (7.8 to 5.6 for stagger, W = 1).
- **Same-round starts stay blind in both modes.** Wip leaves them as they were,
  or adds a few (gossip, stagger, W = 1: 1.4 to 2.5), because steering agents
  away from claimed specs crowds them onto the few still free.
- **Longer work (W = 2) means less of everything** in 12 rounds: fewer
  implementations, fewer accepted, fewer specs solved. Comparing across W
  compares different amounts of work.

**PR #4's duplicate count barely moves** (ring, stagger, W = 1: 8.1 to 7.5),
because most of it is visible: re-implementing a spec whose earlier
implementation failed its checks, or deliberately repeating a covered one.
Wip doesn't target either. Concurrent starts is the number it targets.

**Overhead.** Wip adds 47 to 81% more contributions (claims, `refines`
relations, retractions) and 12 to 17% more bracken calls. Availability is
unchanged at about 2 sync rounds: claims don't make sync any faster.

**Quality is unchanged.** Accepted correct implementations and solved specs
stay within about 1.3 of the no-wip numbers, sometimes higher (specs solved
3.8 to 4.7 on ring, stagger, W = 2) and sometimes lower, since fewer duplicates
also mean fewer implementations to accept.

**Staleness can't tell slow sync from stalled work.** Takeovers (2 to 8 per
run) outnumber silently dropped claims, which at a 10% drop rate should be
about 1 or 2 per run. On ring and
gossip a delivery's retraction can take longer than the stale window to arrive,
so an agent takes over work that was already done. Those takeovers account for
some of the remaining later-round concurrent starts. A window based on sync
lag, or a liveness signal (a claim refreshed each round), would separate the
two. That's the next thing to try.

**What this doesn't show.** The agents are scripted, every run uses the same
six specs, and "work" is a delay, not effort. A real agent might ignore a
claim, pick a different spec for its own reasons, or announce more than it
does. The result is about the mechanism: announced work helps only when the
announcement reaches others before they choose, and nothing helps agents who
choose at the same moment.

## Against the Phase 1 list

| Phase 1 measures | Here |
|---|---|
| Validated improvements per million tokens | Accepted implementations per policy. There are no tokens: the agents are scripted |
| Duplicated work | Duplicated work, blind and visible |
| Integration and validation overhead | Checks and bracken calls, roughly. Not measured as such |
| Time until a discovery is available to others | Availability, in sync rounds |
| Semantically incompatible contributions | Divergences and contradictions |
| Quality of the final artifact | Ground truth, and wrong implementations accepted under each policy |

There's no comparison with Git or CRDT modes yet, and a toy run with scripted
agents says nothing about real agents. What it gives is a working loop and
definitions to argue about.

## Other agents

Agents implement one trait, `bracken_demo::agent::Agent`, with one method
called once per round. It gets the round, its replica (directory, work
directory, identity) and the roster of agent keys and names. Because the
harness measures from the replicas, any agent that writes through bracken can
be measured the same way.

A real-model driver would start a process in the agent's work directory with
a prompt holding the round and roster; the model acts with `bracken … --json`
and checks with `bracken-demo check`. That driver isn't built: running real
agents costs tokens and needs the owner's go. Whoever builds it must give the
model process:

- **A cleared environment**, holding only `BRACKEN_DIR` (the agent's replica),
  `BRACKEN_AGENT` (its identity) and what the model itself needs to run.
  Nothing inherited from the shell that started the demo.
- **The absolute path of the bracken binary** (`Replica::bracken_bin`), not
  a `PATH` lookup.
- **A work directory outside any git checkout**, so no `AGENTS.md`,
  `scripts/bk` or other repository files are within reach. The harness
  already refuses a run directory inside a checkout by default.
- **No access to a team's `.bracken/`**: only the agent's own replica.
- **No `--allow-in-repo`.** A real-agent driver must refuse it, so a model
  never works inside a repository. The harness also refuses to run when
  `GIT_DIR` or `GIT_WORK_TREE` is set, since either can make git treat the
  run's directory as part of a checkout.
