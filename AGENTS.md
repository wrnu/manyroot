# Agent guide

## Things that break production

- **Protocol changes can make every existing Ground unreadable**, this repo's own `.bracken/` included. That covers encoding, ids, the signing context and validity rules (`crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`). They're the `protocol` class: independent review, the manager's go (logged as `owner-review`), and a migration note in `docs/releases.md`.
- **Never commit `.bracken/identities/`.** It holds unencrypted private keys. If git ever shows a file there as staged or tracked, stop and tell the manager.
- **Never edit or delete files under `.bracken/` by hand.** Write only through `scripts/bk`. Only the manager commits `.bracken/`, and only with `scripts/ground-checkpoint`.
- **Coordinate only through `scripts/bk`.** It runs the pinned binary against the main checkout's Ground. When you test your own branch's binary, give it a throwaway Ground: `BRACKEN_DIR=$(mktemp -d)/g target/debug/bracken init`. Without `BRACKEN_DIR`, a binary run in a worktree finds that worktree's stale copy of `.bracken/`. `scripts/bk` always ignores `BRACKEN_DIR`.

## Conventions

- Rust 2024. Formatting follows `rustfmt.toml` (width 120). Use `thiserror` errors in `manyroot` and `anyhow` in the CLI. Outside tests, use `expect("why it can't fail")`, never a bare `unwrap()`.
- Public APIs use plain names (`contribution`, `causal heads`, `evidence`). The subsystem names (Seed, Roots, Proof…) appear only in docs and module comments.
- Nothing in `manyroot` may assume a contribution is code.
- Every read command in the CLI keeps a `--json` form; agents depend on it.
- Commit messages and PRs carry no AI attribution.
- Forge, not GitHub: `fj --host forge.jx.rs …` for PRs, `fj-ex` for Actions. Commands are in `docs/development.md`, under Forge commands.

## Tests

Run all three before every `pr` claim. This table is the CI: the forge's workflow runs only by hand, and the manager's counted run of the table on the exact head is what a merge waits for (decision `20a06f9b815e`, [docs/development.md](docs/development.md)).

| Package | Test | Lint | Format |
|---|---|---|---|
| workspace | `cargo test --workspace` | `cargo clippy --all-targets -- -D warnings` | `cargo fmt --check` |

## Releases

Nothing is published, and no workflow deploys on merge. Publishing crates, making the repo public, licence changes and access or protection changes need the owner's go. See [docs/releases.md](docs/releases.md).

## The Ground

- Always run `scripts/bk … --as <your identity>`, with `BRACKEN_AGENT=<your identity>` set, so reads pull from the team server. A write that `scripts/bk` fails to confirm isn't done (process section 4.3).
- At the start of every task, read `scripts/bk log --tag decision --current` and `scripts/bk show <task id>`. `--current` leaves out retracted entries and those their own author superseded; one someone else superseded stays, marked.
- Check a claim with `scripts/bk show <id>` before acting on it. A `note` is a lead, not a fact.
- Contribute what others would need **as you learn it**, not at the end.
- Messages carry ids; the content goes in the Ground. No secrets or personal data: contributions are permanent.

| What | How |
|---|---|
| Decision | claim `--tag decision`, quoting the owner with the date (choices that aren't gated) |
| Approval of a gated action | the owner signs it with `scripts/owner-approve <id> "<what>"`, or the manager with `--delegated "<the owner's yes>"` after asking for that one approval; never another agent ([docs/development.md](docs/development.md), Owner approvals) |
| Task | claim `--tag task` holding the brief (manager) |
| Note, discovery, dead end | claim `--tag note`, related to the task |
| Question to the manager | claim `--tag question`, related to the task; the answer `refines` it |
| PR head | claim `--tag pr`: `PR #<n> <branch> @ <full sha>: <title>`, then `relate <pr> implements <task>` |
| New commit | a new `pr` claim with `--supersedes <old>` |
| Test run | `"$main/scripts/bk-evidence" <pr> --as <you> -- <command>`, the main checkout's copy (`main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")`): records the real exit code and output tail, `@ <HEAD sha>` |
| Review finding | claim `--tag finding` (file:line, severity, requirement broken, counterexample), then `relate <finding> contradicts <pr>` |
| Final report | claim `--tag report`, then `relate <report> implements <task>` |
| Gotcha, preference, reference | claim `--tag gotcha` / `preference` / `reference` |
| Process idea | claim `--tag process` |
| Disagreement | a claim that `contradicts` the thing you disagree with, with your reasoning |

## Shared knowledge

There's no memory folder: what a future agent needs is in the Ground (process section 9.1).

- At task start, read `scripts/bk log --tag gotcha --current` and `--tag preference --current` (plus `--tag reference --current` when relevant). `scripts/bk show <id>` tells you whether one is contradicted.
- Contribute a `gotcha` (the trap and the fix), a `preference` (the owner, quoted and dated) or a `reference` (where something outside the repo lives) the moment you learn it.
- Don't contribute what the code, git log or docs already say, or secrets. To change your own entry, contribute the new one with `claim … --supersedes <old>`.

## Working with other agents

Roles, the merge gate and everything else about how the team works are in the process, [process.md](.claude/skills/agent-team-kickoff/process.md). This repo's values for it (models, risky classes, Forge commands) are in [docs/development.md](docs/development.md). Builders start with [docs/builder.md](docs/builder.md). "Process section N" means a section of `process.md`.
