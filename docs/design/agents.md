# Agents and Bracken

How agents relate to Bracken and Weave: what Bracken owns, what a harness
owns, and what an agent must be able to do with nothing but the CLI. The
owner accepted five recommendations and an order on 2026-10-30 (decision
`adf43778c85d`): Bracken doesn't launch agents and owns the work queue; the
integrator is an ordinary agent; `bracken watch`; CLI first, MCP later; and
the experiment crate repurposed. The details below (the queue's states,
`watch`'s filters and polling, the backend interface, one key per agent,
which experiment parts to keep) are the manager's proposals, settled when
each is built. It replaces the idea of adding a Pi integration first: Pi
becomes the test of whether Bracken is harness-neutral, not a feature of
its own.

This is design only. Nothing here is built yet, apart from what it names
as existing.

## The principle: agents drive Bracken

Bracken and Weave are the shared substrate: the record, the code, the
decisions and the checks. They don't launch, prompt or pay for agents. A
**harness** (T3 Code, Claude Code, Pi, a cron job, a person at a terminal)
does that, and talks to Bracken only through the CLI.

Two reasons:
- **Harnesses differ most at launch:** auth, sandboxes, models, costs and
  permissions. Owning launch would tie Bracken to every harness's quirks,
  which is the coupling `crates/experiment/src/claude.rs` already shows.
- **The CLI is already neutral.** Anything that can run a command and read
  JSON can take part today. Keeping it the only door keeps one surface to
  test, review and secure.

## What an agent is

An agent is an identity, the roles it holds, and the harness that runs it.

- **Identity.** A key in the Ground's `identities/`. In v0 every agent on a
  machine can read every key, so a signature records who claimed to act,
  not proof. The aim is for each agent to hold only its own key; the
  harness supplies its path and `BRACKEN_AGENT`.
- **Roles.** Weave's line policy already names them: `integrators`,
  `validators` and `goal_setters`, plus the `weave-runner` that runs checks.
  A builder is any agent that proposes. Roles are data in the Ground, so
  swapping who holds one is a policy change, not code.
- **Harness.** Not recorded by Bracken, beyond what an agent chooses to say
  in its own contributions.

### The integrator is an agent

The integrator is an ordinary agent in some harness, with its goals and
decisions in the Ground (weave.md, *Decisions* and *Goals, checks and
objections*). Bracken supplies the mechanics it relies on: heads, merges,
check results, guardrails and the qualification rules. The configurations
weave.md lists (dedicated, rotating, elected or several integrators) are
then a choice of which agents hold the role.

## The loop an agent must complete

With only the CLI and a short instructions file, an agent must be able to:

| Step | Command | Exists today |
|---|---|---|
| Find work | the work queue (below) | partly: `log --tag task`, `status` |
| Take it | `intend <task>` | yes |
| Get a workspace | `weave checkout` of a head | manifests only: `weave checkout --manifest`; heads come in step 5 |
| Contribute | `weave propose`; claims, evidence and relations | proposals come in step 6 |
| Respond | read findings and objections against its work | findings: `status --as` puts contradictions of its own claims first, `log --open` lists all open ones; objections come in step 7 |
| Be woken | `watch` (below) | **no** |
| Finish | a report that `implements` the task | yes |

The **neutrality test**: an agent in a harness we didn't build for (Pi
first) completes this loop on a real task. Anything it can't do is a gap
in Bracken, not a missing adapter.

## The work queue

Bracken owns the queue, so any launcher can read it. It is a derived view,
like `status`, over what the Ground already holds:

- **Open:** a current task claim with nothing current that `implements` it.
- **Taken:** an open task with a live intent (`intend`, which expires).
- **Blocked:** an open task that `depends_on` something not yet accepted.
- **Ready:** open, not taken and not blocked.

`status` already does most of this for review work. The queue extends it
to tasks, with `--json` and a filter by role, so a launcher can ask "what
is ready for a builder?" and start an agent on it.

An optional dispatcher may come later: a small program that reads the
queue and starts agents through harness adapters. It lives outside the
core and is replaceable; T3 Code plays that part today.

## `bracken watch`

The missing piece. Today waking an agent depends on T3 messages; any other
harness would have to poll `log`.

    bracken watch [filters] [--since <cursor>] [--follow] [--json]

- **Filters** are `log`'s: `--tag`, `--open`, plus "for me": contributions
  that contradict, refine or depend on my own, and work waiting for my
  pass (as `status --as` orders it).
- **Output** is one compact line (or JSON object) per new contribution,
  each with a cursor, like `log --since`. Without `--follow` it prints
  what's new since the cursor and exits, so a harness can call it in a
  loop or a hook. With `--follow` it keeps streaming.
- **Freshness.** Through `scripts/bk` a read pulls from the team server
  first; `watch --follow` pulls on an interval. No server push in v0.

It works on any replica, needs no new protocol, and is useful on its own
for people too.

## Tools: CLI first, MCP later

The CLI with `--json` is the interface, and every read command keeps its
JSON form (AGENTS.md). An MCP server may come once the Weave verbs settle, and
only as a thin layer: the same code paths, no power the CLI lacks, and the
same token checks as `scripts/bk`. Otherwise it is a second door to
secure.

## Evaluating configurations

`crates/experiment` is repurposed, not retired. It keeps what was hard to
build:
- the sandbox and fixed environment per agent;
- token accounting, caps and fail-closed handling;
- the forwarding proxy;
- the plan, gate and transcript checks.

It drops the bracken-versus-git-versus-crdt comparison. Instead it runs one
task under different Weave configurations (who integrates, guardrails,
windows, which head agents build on, test cadence) and compares them.

The backend is an interface: start a turn with a prompt and an
environment, stream events, report usage, stop. `crates/experiment/src/backend.rs`
holds it: a `Harness` says how to start a turn and read its events, and
the backend does the gate, the accounting, the caps, the process group and
the transcripts the same way for every harness. Claude Code (`claude.rs`)
is the first harness, Pi the second. Running the same configuration under
both is also the neutrality test above.

## Order

The owner's order (decision `adf43778c85d`):
1. Finish the Weave build steps already planned (weave.md section 6).
2. `bracken watch`.
3. This design note.
4. Repurpose `crates/experiment`, with Pi as the second backend.
5. MCP, if it still seems worth it.

Proposed, not yet placed in the order: the work queue view, and a generic
agent guide shipped with Bracken (the loop above, for any harness).

## Open questions

- **Keys per agent.** Who creates an agent's key, and how a harness gets
  only its own, before the per-agent key work lands.
- **Queue names.** Whether the queue is a mode of `status` or its own
  command.
- **Waking across machines.** `watch` covers one replica; a team on several
  machines relies on the team server, and a push channel may be needed
  later.
