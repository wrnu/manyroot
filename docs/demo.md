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
you asked.

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

## What the numbers mean

The harness computes everything from the replicas, through bracken's JSON,
not from what agents say they did. Counts cover the agents' contributions; the
setter's are left out.

| Line | Meaning |
|---|---|
| Contributions | Agents' contributions, by kind and by agent |
| Work | Implementations (artifacts that `implement` a spec, by the artifact's author); fixes (implementations that `supersede` another); checks (evidence) and their verdicts; findings (claims tagged `finding`) |
| Duplicated work | New implementations (not fixes) of a spec that another agent had already implemented anywhere in the Ground. **Blind**: the earlier one hadn't reached this agent's replica yet, so only better sync or visible work in progress would have avoided it. **Visible**: the agent's replica already held it. Competing fixes aren't counted here; they show up as divergences |
| Availability | For each contribution and each other agent, the number of sync rounds until that agent's replica held it: 1 means the sync right after the round it was made. The final all-to-all sync counts as round `rounds + 1`, and the line says how many pairs only arrived then |
| Conflicts | What `bracken conflicts` reports at the end: divergences (one implementation superseded by two current ones) and contradictions (a current finding contradicting a current implementation). Fixing an implementation resolves the contradictions against it. The relation counts show how many of each kind agents made |
| Ground truth | The harness checks every implementation on every input. "Caught" means a wrong one got at least one failing check from another agent |
| Policy table | `bracken accepted --min-passes N` for N = 1, 2, 3, with any agent as a validator. **accepted** counts subjects meeting the policy, including **superseded** ones; **impls**, **correct** and **wrong** are the accepted implementations by ground truth; **specs solved** counts specs with an accepted, not superseded, correct implementation; **conflicts** are those in the accepted layer |
| Replicas | Whether every replica ended with the same contributions |
| Timing | Wall time and bracken calls. The only part that changes between runs with the same seed |

A sample, from `--agents 4 --rounds 5 --seed 1`:

```
Bracken demo: 4 agents, 5 rounds, seed 1, ring sync, checks on 6 sampled inputs, 6 specs

Contributions    75 by agents: artifact 16, claim 5, evidence 28, relation 26
                 agent-0 21, agent-1 18, agent-2 18, agent-3 18
Work             16 implementations (0 fixes), 28 checks (20 pass, 8 fail), 5 findings
Duplicated work  10 of 16 new implementations repeat another agent's spec (8 blind, 2 visible)
Availability     1.90 sync rounds on average until another agent holds a contribution (median 2, max 3; 61 of 225 only in the final sync)
                 by rounds: 1: 75, 2: 97, 3: 53
Conflicts        0 divergences, 5 contradictions open at the end (from 0 supersedes and 5 contradicts relations)
Ground truth     10 implementations correct, 6 wrong; 6 of the wrong ones got a failing check

Policy         accepted  superseded  impls  correct  wrong  specs solved  conflicts
min passes 1         9           0      9        9      0         5 of 6          0
min passes 2         9           0      9        9      0         5 of 6          0
min passes 3         1           0      1        1      0         1 of 6          0

Replicas         converged: all 4 hold the same 88 contributions
Timing           5.0 s wall, 228 bracken calls (4.9 s)
```

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
