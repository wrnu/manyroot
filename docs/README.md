# Docs

| Doc | What it's for |
|---|---|
| [vision.md](vision.md) | Why Manyroot and Bracken exist, principles, roadmap |
| [protocol.md](protocol.md) | The Manyroot protocol, version 1 draft |
| [cli.md](cli.md) | Every `bracken` command: recording work, reading state, the web view, sync, intents, Weave (moved from the README) |
| [demo.md](demo.md) | The agent demo harness: how to run it and what its report measures |
| [development.md](development.md) | This repo's values for the team process: profile, models, risky classes, tool mapping, remotes and GitHub commands. The process itself is [process.md](../.claude/skills/agent-team-kickoff/process.md) |
| [builder.md](builder.md) | One-page role card for builders |
| [design/eval-suite.md](design/eval-suite.md) | Proposal (2026-10-30): an evaluation suite of Python projects (spec, seed, hidden tests, golden solution) to test Bracken and Weave as a development system; full traces, metrics decided after exploratory pilots, isolation, a cost estimate |
| [design/agents.md](design/agents.md) | Decided (2026-10-30): how agents relate to Bracken and Weave: Bracken owns the record and the work queue, harnesses launch agents, `bracken watch`, CLI first, and the experiment crate repurposed to evaluate Weave configurations with Claude Code and Pi |
| [design/weave-mode.md](design/weave-mode.md) | Design sketch (2026-10-30): Weave's model, content as immutable file versions and manifests, an agent integrator, and main and green as causal queries; the spec below is built from it |
| [design/weave.md](design/weave.md) | Product spec (2026-10-30), built through stage 2a and stage 2b step 1b: Weave's data model, derived state, the `bracken weave` CLI, storage, migration, the build plan and what stays git's job |
| [design/weave-freeze.md](design/weave-freeze.md) | Design (2026-11-08), not yet approved: freezing Weave's formats, covering what is frozen by file, which guard catches a change and the gaps, what a frozen-format change takes, the join, and the owner's questions |
| [design/visualizer.md](design/visualizer.md) | Design (2026-10-29): the JSON `bracken view` serves, the visual language, and the mockups in [design/visualizer/](design/visualizer/) |
| [design/agent-keys.md](design/agent-keys.md) | Design (2026-11-04): per-agent keys and delegation, the options for making a signature mean more than whoever claimed to act, and part A (keys a server holds) accepted and built, part B open ([cli.md](cli.md), Keys a server holds) |
| [design/acceptance-follows-changes.md](design/acceptance-follows-changes.md) | Decided (2026-10-29): an accepted contribution whose dependencies changed after its evidence shows as needs re-check, under an opt-in policy; class, cost, and its effect on the Phase 2 pilot |
| [design/network-sync.md](design/network-sync.md) | Network sync design, decided in part (2026-10-27): syncing Grounds over a network, and what it retires from the process |
| [design/phase1-experiment.md](design/phase1-experiment.md) | Proposal (2026-10-27): the first Phase 1 experiment, 10 agents under Git, CRDT and Bracken modes: task, setup, driver, metrics, cost, build steps |
| [design/phase1-results.md](design/phase1-results.md) | Results (2026-10-28): the Phase 1 calibration, the 10-agent pilot and its two replicates, the six measures per mode with their spread, use, what three replicates support, and the options for scale |
| [design/phase1-bracken-lessons.md](design/phase1-bracken-lessons.md) | Design analysis (2026-10-28): why Bracken mode duplicated work, integrated slowly and read so much in Phase 1, what is Bracken's and what the experiment's, and ranked changes to Bracken |
| [design/phase2-task.md](design/phase2-task.md) | Proposal (2026-10-28): the next experiment's task, three harder candidates with grading, sandboxing, expected use and build steps, the experiment-side fixes a next run needs, and which of them need the owner's go |
| [releases.md](releases.md) | Release stages, the owner's-go list, pinned-binary upgrades, migrations, rollback |

Stale docs move to `archive/`. Knowledge that isn't documentation (decisions, gotchas, tasks, the plan, weekly metrics) lives in the Ground: `scripts/bk log --tag <tag> --current`.
