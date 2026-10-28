# Docs

| Doc | What it's for |
|---|---|
| [vision.md](vision.md) | Why Manyroot and Bracken exist, principles, roadmap |
| [protocol.md](protocol.md) | The Manyroot protocol, version 1 draft |
| [demo.md](demo.md) | The agent demo harness: how to run it and what its report measures |
| [development.md](development.md) | This repo's values for the team process: profile, models, risky classes, tool mapping, Forge commands. The process itself is [process.md](../.claude/skills/agent-team-kickoff/process.md) |
| [builder.md](builder.md) | One-page role card for builders |
| [design/network-sync.md](design/network-sync.md) | Network sync design, decided in part (2026-10-27): syncing Grounds over a network, and what it retires from the process |
| [design/phase1-experiment.md](design/phase1-experiment.md) | Proposal (2026-10-27): the first Phase 1 experiment, 10 agents under Git, CRDT and Bracken modes: task, setup, driver, metrics, cost, build steps |
| [design/phase1-results.md](design/phase1-results.md) | Results (2026-10-28): the Phase 1 calibration, the 10-agent pilot and its two replicates, the six measures per mode with their spread, use, what three replicates support, and the options for scale |
| [releases.md](releases.md) | Release stages, the owner's-go list, pinned-binary upgrades, migrations, rollback |

Stale docs move to `archive/`. Knowledge that isn't documentation (decisions, gotchas, tasks, the plan, weekly metrics) lives in the Ground: `scripts/bk log --tag <tag> --current`.
