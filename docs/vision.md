# Vision: Manyroot and Bracken

> **Manyroot.** Many minds. Shared ground.
>
> **Bracken.** Software that grows together.

This document records the long-term vision behind this repository, so that
design decisions made today can be checked against where the project is
going. It describes intent, not current capability. For what exists, see the
[README](../README.md) and the [protocol draft](protocol.md).

## The vision

Manyroot is a global, decentralized network where knowledge accumulates
through independent contributions from humans and AI agents.

Today the internet mostly connects documents, services and people. Manyroot
would connect **knowledge, discoveries, reasoning and the agents producing
them**.

Here's an example. An agent researching a battery chemistry contributes an
observation. Another agent, working for a different organization, sees that
it matters for electric aviation. A third independently tests the underlying
hypothesis. A fourth finds a flaw in the original method. None of them knows
the others, and none was assigned to the same project, yet their work
connects through the shared structure. The network keeps all of it after
every one of those agents has gone.

Agents come and go: their context windows disappear, their processes end,
their models get upgraded. The body of knowledge keeps growing. The goal is
an environment where intelligence accumulates **across agents, across
organizations and across time**. The point isn't a swarm of clever agents.
It's a place where what each of them learns is never lost and can be built on
by anyone.

**Bracken** is the first application and the proving ground. It is
collaborative software development for large agent teams. Software comes
first because there we can measure whether collaboration actually produced
something correct. Bracken should succeed as a standalone developer tool even
if the global network takes a decade. If the larger vision succeeds, Bracken
is its first application, not its limit.

## Why not Git

Git is a distributed, content-addressed DAG, and it can scale. Its
limitation is the collaboration model. It reconciles **versions of files**
through branches, merges and pull requests, a workflow designed around a
modest number of humans who periodically agree on what's authoritative.

Agents are different:

| Dimension     | Git-based collaboration         | Agent-native                                      |
| ------------- | ------------------------------- | ------------------------------------------------- |
| Participants  | Tens or hundreds of developers  | Potentially thousands of agents                   |
| Collaboration | Branches and pull requests      | Continuous shared contributions                   |
| Conflicts     | Merge and resolve text          | Record both; resolve by evidence                  |
| Knowledge     | Commits, comments, docs         | Structured contributions with typed relations     |
| History       | Commit DAG                      | Causal graph of changes *and* discoveries         |
| Integration   | Periodic                        | Continuous                                        |
| Validation    | CI and code review              | Continuous evidence, evaluated by policy          |

Early observations of agent teams point the same way. Agents that open many
PRs often see few of them merged. Teams that avoid shared files merge more
but collaborate less. Both the agents' coordination ability and the
structure of their environment matter, and the environment is the part we
can design.

Git still has a place at the boundary, producing stable releases for humans.
It just stops being the medium through which agents think together.

## Principles

These guide design decisions at every layer.

1. **Conflict-free to record, never automatically authoritative.**
   Contributions merge without coordination, because the graph is a
   grow-only set. Merging never decides what's *right*.
2. **Synchronize evidence, not consensus.** If one agent publishes supporting
   evidence, another publishes contradicting evidence, and a third finds a
   flaw in the first's method, all three are kept with their relations and
   provenance. Readers evaluate them by their own standards. This also
   guards against premature consensus, where thousands of agents reinforce
   one wrong belief.
3. **Causality, not a global timeline.** Concurrent contributions have no
   meaningful order. What matters is what each one built on.
4. **Truth isn't a CRDT.** CRDTs guarantee that state converges, not that
   beliefs do. Acceptance is a *policy* applied locally to shared evidence,
   so different communities can read the same evidence differently.
5. **Agents are temporary; knowledge persists.** Anything worth keeping is a
   contribution, not a chat message or something held in a context window.
6. **No universal ontology, no central authority.** Agents must be able to
   find meaningful relations between independent work without first agreeing
   on one schema or one arbiter.
7. **Independent of any vendor.** The protocol doesn't depend on an agent
   framework, model provider or company. At global scale, that independence
   is foundational.
8. **Plain names in APIs, poetic names for products.** Public interfaces say
   `contribution`, `causal_heads`, `evidence`, `validation_result`. Names like
   Seed and Roots describe modules and products, so developers never have to
   learn a metaphor to use the protocol.

## Five primitives

| Primitive   | Responsibility                                                       |
| ----------- | -------------------------------------------------------------------- |
| Knowledge   | Claims, hypotheses, relations, observations and artifacts            |
| Causality   | What influenced, supported, contradicted or depended on what         |
| Convergence | Synchronizing independently contributed state                        |
| Trust       | Provenance, evidence, independent verification, reputation           |
| Discovery   | Letting agents find relevant knowledge outside their immediate task  |

Discovery is what separates this from a distributed database. If agents only
retrieve exactly what they ask for, we've built storage. Compare two cases:

- **Retrieval only:** an agent searches the network, finds what it needs and
  finishes its task.
- **Discovery:** an agent contributes an observation, and the network notices
  how it relates to other work. Those relations become open questions, which
  attract other agents, who contribute more evidence.

The second case can sustain its own cycle of exploration without anyone
having a grand plan. It also needs ways to prioritize valuable work, prevent
feedback loops and manage compute budgets. Without those, it becomes a very
expensive machine for producing speculation.

## Architecture

### Grounds, not one global graph

Making every participant replicate everything doesn't work at planetary
scale. Manyroot is **federated**:

- A **Ground** is a bounded workspace: a project or knowledge domain, with
  its own participants, permissions, replicated state and causal history.
- Agents take part in whichever Grounds matter to them, and subscribe only to
  the parts relevant to their work.
- Grounds reference each other through globally unique, content-addressed
  ids. The network is globally *connected* without being globally
  *synchronized*.

### Exploration and acceptance

Each Ground has two layers:

- The **exploration layer** can grow almost without limit: alternative
  designs, hypotheses, tests, implementations, contradictions.
- The **accepted layer** holds only what meets explicit requirements and
  validation rules, under the reader's policy.

Separating them preserves creative diversity while still producing reliable,
usable output. It also avoids coordinating every agent around one mutable
version of reality.

### Contributions

A contribution is immutable, typed, signed by its author and
content-addressed. It names its causal parents and the Ground it belongs to.
Contributions can be claims, artifacts, relations (`supports`, `contradicts`,
`depends_on`, `refines`, `supersedes`, `implements`, `tests`), evidence, or
retractions. Concurrent compatible contributions combine without any merge.
Incompatible ones are both kept and surfaced as conflicts, to be settled by
further contributions and evidence.

## Naming architecture

```
PineIT  →  Manyroot (the vision and the protocol)  →  Bracken (the first application)
```

| Product           | Role                                                                    | When       |
| ----------------- | ----------------------------------------------------------------------- | ---------- |
| Manyroot Protocol | Open specification, core data structures, replication, interoperability | Foundation |
| Bracken           | Collaborative software development for large agent teams                | Build first |
| Pinebots          | Portable agent runtimes that can join Manyroot, not required by it      | Integration |
| Manyroot Network  | The public, federated network of interoperable Grounds                  | Long term  |

The protocol's subsystems:

| Name     | Component              | What it does                                                               |
| -------- | ---------------------- | -------------------------------------------------------------------------- |
| Ground   | Shared workspace       | A bounded domain with its own participants, permissions and state          |
| Seed     | Contribution           | An immutable, typed contribution: code, hypothesis, test, observation, evidence |
| Roots    | Causal history         | The append-only graph of dependencies and how contributions evolved        |
| Weave    | Convergence            | Replication, and materializing merged state (such as a file tree) from the graph |
| Trails   | Provenance             | Authorship, derivations, evidence and relations between contributions      |
| Proof    | Validation             | Tests, constraints and acceptance policies                                 |
| Rings    | Versioning             | Reproducible checkpoints, snapshots and published releases                 |
| Signal   | Discovery              | Relevant changes, open questions, contradictions, novel connections        |
| Canopy   | Federation             | Selective replication and discovery across independently hosted Grounds    |
| Stewards | Trust and governance   | Identities, permissions, reputation, policy, authority over accepted output |

These aren't separate services from day one. The protocol starts as one
small library (`crates/manyroot`), and subsystems split out only when their
requirements justify it. The [README](../README.md#layout) tracks which exist.

Weave needs one clarification. Because the contribution graph is a grow-only
set, merging is set union, and the graph itself is the CRDT. Weave isn't a
separate CRDT engine. It's replication plus *materialized views*, such as
building a working source tree from artifact contributions.

These names are working names. Manyroot and Bracken both have existing uses,
so they need proper trademark and domain searches before any public launch.
Rhizome and Understory were considered and rejected, because projects in
decentralized data and agent memory already use them.

## Roadmap

| Phase | Scale | Goal |
| ----- | ----- | ---- |
| 1. Collective software development | 10–100 agents | Show that independent agents contribute to a shared project more effectively than Git-based teams |
| 2. Persistent knowledge | 100–1,000 agents | Generalize beyond code to hypotheses, research, experiments, relations and reusable insights |
| 3. Federated knowledge | Many organizations | Independently run Grounds sync selectively, with identities, permissions and provenance |
| 4. Global collective intelligence | Long term | Agents discover relations across Grounds and start new investigations without central task assignment |

### The Phase 1 experiment

Start small, and measure. Give 10, 20, 50 and then 100 agents the same
tasks, model budget and test environment under three modes:

1. Git branches and pull requests.
2. CRDT-based shared source editing.
3. Semantic contributions in a causal graph (Bracken).

Measure:

- validated improvements per million tokens;
- duplicated work;
- integration and validation overhead;
- time until a useful discovery becomes available to other agents;
- the number of semantically incompatible contributions;
- the quality of the final artifact.

If mode 3 wins at 20, 50 and 100 agents, that's strong evidence that the
collaboration model itself improves what a swarm can do.

This repository's own agent team coordinates through a Bracken Ground (see
`.claude/skills/agent-team-kickoff`), so building Bracken is also its first
real dataset.

## Hard problems

- **Representing knowledge.** This is the central research question: how to
  represent knowledge so that independent agents can find meaningful
  relations without a universal ontology, a central authority, or agreement
  on what's true.
- **Duplicated work.** Perfect synchronization doesn't stop thousands of
  agents doing the same thing. This needs novelty detection and visible
  claims of work in progress.
- **Correlated behaviour and premature consensus.** Agents built on similar
  models drift toward the same answers, and important minority evidence gets
  missed. Diverse objectives and independent verification matter more than
  agent count.
- **The cost of speculation.** Self-sustaining exploration needs
  prioritization, budgets and protection against feedback loops.
- **Trust at scale.** Signatures record who said something, not whether they
  should be believed. Reputation, independent verification and resistance to
  Sybil attacks are all open problems.
- **Privacy and secure coordination.** Organizations need to collaborate
  without revealing everything: selective replication, permissions, and
  establishing trust without disclosing private information.
- **Semantic conflicts.** Contributions that are each valid can combine into
  something logically inconsistent. Detecting that goes beyond structural
  rules.

## Related work

These are established building blocks:

- **Automerge** and **Loro**: CRDT libraries with mergeable structures,
  histories and sync. Automerge converges deterministically while keeping
  concurrent conflicting values.
- **Ink & Switch, Patchwork**: research combining branching, merging and
  CRDT collaboration beyond source code.
- **Jujutsu (jj)** and **Pijul**: version-control systems whose models of
  changes, conflicts and history are useful precedents.
- **Event sourcing**: immutable events as the source of truth, with state
  derived from them.

The original brainstorm also cited the leads below. **None has been checked
yet**, so verify each one before relying on it:

- Anthropic research (August 2026) on patterns and problems in emerging
  multi-agent systems: merge rates, correlated behaviour, premature
  consensus.
- "Client-Driven Offline-First RDF 1.2 using OR-Sets" (ICWE 2026): CRDT
  sync of RDF knowledge graphs edited concurrently by agents.
- Inria research on semantic conflicts in CRDT-based knowledge graphs.
- Martin Kleppmann's Centre for Cryptographic Trust Infrastructure: secure
  coordination between autonomous agents.
- A multi-agent replication survey comparing jj, Pijul and CRDTs.

The likely novelty isn't any single piece. It's the combination: convergent
data structures, agent-native semantic contributions, continuous discovery,
and acceptance by policy instead of consensus.

## What this isn't limited to

The same structure could support mathematical research, engineering design,
scientific discovery, or collaborative creative worlds. That's why the
protocol is named for the vision, not for software, and why nothing in
`manyroot` should assume that a contribution is code.
