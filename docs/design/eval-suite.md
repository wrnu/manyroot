# An evaluation suite: Bracken and Weave on real projects

> **Proposal, 2026-10-30.** The owner's idea: once Bracken and Weave form a
> full development system, test it repeatedly on real software projects, each
> with a spec, a hidden test suite and a golden solution. The owner set two
> things: projects in **Python** (cheap to run, no compile step), and an
> **exploratory** start: run it, watch the behaviour, and decide what to
> optimise from what we see, rather than fixing metrics in advance. The budget
> is open until the cost estimate below is discussed. Nothing here runs a
> model; each run needs the owner's go, as every model run does.

It builds on [agents.md](agents.md) (agents drive Bracken; the experiment
crate becomes a harness-neutral evaluation runner, decision `adf43778c85d`)
and on [phase2-task.md](phase2-task.md)'s Candidate B, which already worked
out grading and sandboxing for a Python library with a held-out test suite.

## The idea in one paragraph

A **project** is a spec, a starting repository, hidden tests and a golden
solution. A **configuration** is how the team works: how many agents, who
integrates, the guardrails, the test cadence, the harness and model. A
**run** puts one configuration on one project, records everything that
happens, and grades the result at checkpoints. Running the same pair several
times shows the spread; running different configurations on the same project
shows what they change. The first runs are for looking, not for scoring; after that, each
improvement is judged against our own earlier runs.

## A project

| Part | Who sees it | What it is |
|---|---|---|
| Spec | agents | a README-style description of what to build, any required interface, and a few public example tests |
| Seed repo | agents | the starting state: empty, a skeleton, or an existing codebase to extend |
| Hidden tests | the grader only | pytest cases and property tests, written from the spec, that judge behaviour, not structure |
| Golden solution | the grader only | proof that the spec can be met and that the hidden tests are right; never shown to agents, never graded against directly |

**Tests come from the spec, not the solution.** A test written from the
golden solution's internals punishes valid alternatives. The hidden suite is
written against the spec, then run against the golden solution, which must
pass all of it. Where the spec leaves a choice open, the tests accept every
reasonable answer, or the spec is tightened.

**Python only** (the owner's choice), and, as a proposal, **standard
library only at first**. No build step, pytest as
the grader, and a sandbox that needs only a pinned interpreter. Third-party
packages come later, if a project needs them, with a pinned, offline wheel
set.

## A ladder of projects

Start small, so a failure points at one cause.

| Rung | Example | What it exercises |
|---|---|---|
| 1. Small, existing conformance suite | a TOML or CommonMark subset, graded by a slice of `toml-test` or the CommonMark spec tests | the whole loop end to end, cheaply enough for many runs |
| 2. Medium, written by us | a small domain library or CLI with about 100 hidden tests (phase2-task.md's interval sets is one) | real parallel work and integration, with no chance a model has seen it |
| 3. A feature on an existing codebase | add a feature to a seed repo of a few thousand lines | reading before writing; not breaking what works |
| 4. A spec that changes mid-run | rung 2, with a requirement revised halfway | whether the integrator adapts, as the owner asked an integrator to |

Rung 1 suites are public, so models may have seen them; they test the
machinery, not the models. The projects that tell us most are rungs 2 to 4,
written for this suite and never published.

**Where projects live.** Specs and seed repos can live in this repository.
Hidden tests and golden solutions can't: this repository's tree is readable
by every agent, goes into the dogfood Weave Ground (decision
`16ff5718c166`), and would be published if the repository ever is. They live
in a separate store that only the grader and a project's authors can read
and that no Ground imports.

**Who writes them** (a proposal): agents can draft a project, with
independent review and the owner approving each one before use. Their
review records only ids and verdicts in the Ground, never test content, and
agents that drafted or reviewed a project never run on it.

## Configurations

What a run varies, taken from weave.md's configurations and agents.md:
- **Team:** one agent, or several; and how many.
- **Integrator:** dedicated, rotating, elected, several; its goals.
- **Guardrails and windows:** how much breakage the integrator may accept,
  and for how long.
- **Which head agents build on**, and **test cadence**.
- **Harness and model:** Claude Code first, Pi second (agents.md).

**No external baselines.** The goal is to improve our own system steadily
from what the runs show, not to compare it with other tools (owner, decision
`6f17c6a66d8b`). Each change to Bracken, Weave or a configuration is
compared with our own earlier runs on the same project. A single agent is
still a configuration we can run when a question needs it.

## Record everything, decide metrics later

Because the start is exploratory, a run keeps a full trace, so any metric we
think of later can be computed over every past run, not just future ones:
- every contribution in the run's Grounds, which are already a signed,
  causally ordered log;
- Weave's heads and decisions over time;
- each agent's transcript, tool calls and token use (the runner already
  records these);
- the hidden tests' results at regular checkpoints and at the end, per test;
- wall time, and every cap or failure that stopped a turn.

**Metrics we expect to look at first** (a starting list, not a commitment):
- hidden tests passing over time, not only at the end;
- how long the main head stayed broken, and how it recovered;
- tokens and time per newly passing test;
- duplicated or discarded work;
- review findings, and how many were real;
- how often the integrator chose to accept breakage, and what followed;
- interventions a human had to make.

After the first runs we look at the traces together, decide what we want to
optimise, and write that down before comparing configurations in earnest.
Only then do replicates and spread matter.

## Isolation

The grader runs agent code, and agents must never see hidden tests or the
golden solution. Candidate B's design applies in full:
- a pinned interpreter (path and hash) run as `-I -S -B`, with a cleared
  environment, reads only of the copy under test and the standard library,
  a fresh scratch directory for each graded version, no network, no process
  but the interpreter, and limits on CPU, memory and output;
- hidden tests stay in the grader's own process: it sends one case's inputs
  to the child and compares the result itself;
- hidden tests and the golden solution never enter any Ground, Weave's
  included, or any directory an agent can read.

Requirements for building the runner (from security's review, evidence
`bfa24c63c84f`):
- **pytest runs in the grader**, calling agent code through a proxy to the
  sandboxed child. Hidden tests never import agent code into the grader's
  process, where it could read expected values, patch asserts or exit early.
- **The limits are enforced by the operating system**, not in Python:
  `ctypes`, `os` and `subprocess` are all in the standard library.
- **Hidden-test results stay away from agents for the whole run:** out of
  every run Ground, out of the integrator's guardrails and out of every
  prompt. Otherwise the hidden suite becomes an oracle.
- **No access to earlier runs** of the same project: their Grounds, traces
  or results.
- **Isolation is proven before rung 2.** Rung 1's suites are public, so they
  can't show it; the forbidden-access tests must.

That execution profile is a new risky surface, and gets its own tests and
security review, as Candidate B planned.

## Cost estimate

From Phase 1 and 2's measured use, and Candidate B's estimates for Python
code (whose files are several times larger than Phase 1's): an agent-turn
costs roughly **150,000 to 300,000 tokens** for git- or CRDT-like reading,
more for Bracken before phase1-bracken-lessons.md's fixes. Weave's own figure
is unknown until measured; the first runs measure it.

| Run | Agents × turns | Tokens per run (rough) |
|---|---|---|
| Pilot, rung 1 | 3 × 6 | 3 to 6 million |
| Small team, rung 2 | 5 × 8 | 6 to 12 million |
| Full team, rung 2 | 10 × 8 | 12 to 25 million, more if Weave reads heavily |

Phase 2's measured runs used 37.8, 40.0 and 34.3 million tokens for three
modes together, so one mode at a time is roughly a third of that. Turning tokens into money needs
the current price of the chosen model; that, and a monthly figure, is the
budget discussion still to have.

## Order

After Weave's build steps and `bracken watch` (decision `adf43778c85d`):
1. The evaluation runner: the experiment crate repurposed with Claude Code
   and Pi backends, the Python execution profile, and the grader, with their
   tests and security review.
2. One rung 1 project, with its spec, seed, hidden tests and golden solution.
3. A pilot run of one configuration, with the owner's go, then a look at the
   trace together.
4. One rung 2 project, written for the suite.
5. Decide the first metrics to optimise from what the pilot showed; then
   compare configurations, with replicates.

## Open questions

- **Budget:** per run and per month; decides how many replicates and
  projects.
- **The unit of work in Weave for Python:** file-level units are what Weave
  has today; whether that's fine-grained enough is something the pilot shows.
- **Human time:** how much of the owner's attention each look at a trace
  takes, and whether a summary view should come with the runner.
