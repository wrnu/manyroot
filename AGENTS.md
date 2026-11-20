# Agent guide

## Things that break production

- **Protocol changes can make every existing Ground unreadable**, this repo's own `.bracken/` included. That covers encoding, ids, the signing context and validity rules (`crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`), and Weave's formats and their rules, all of `crates/tree/src/` ([docs/design/weave-freeze.md](docs/design/weave-freeze.md)). They're the `protocol` class: independent review, the manager's go (logged as `owner-review`), and a migration note in `docs/releases.md`. Weave's formats are frozen as version 1, and until the join a change to a frozen format needs the owner's own go too; rules pinned but not frozen keep the manager's go (decisions `bae92c497714`, `0290487abd90`). `scripts/merge` checks part of this through `scripts/tree-scope frozen`: an `owner-review` naming the pr claim and head for any frozen path, an independent review's pass at the head for `crates/tree/src/` and Weave's CLI, and the owner's own approval only for a golden vector, a pinned fixture value or `crates/tree/tests/fixtures/` ([docs/development.md](docs/development.md), Owner approvals).
- **Never commit `.bracken/identities/`.** It holds unencrypted private keys. If git ever shows a file there as staged or tracked, stop and tell the manager. New identities get no key there: the team server holds theirs (`scripts/team-server key`, [docs/development.md](docs/development.md), Team server), and nothing copies a key into or out of its key directory.
- **Never edit or delete files under `.bracken/` by hand.** Write only through `scripts/bk`. Only the manager commits `.bracken/`, and only with `scripts/ground-checkpoint`.
- **The Weave Ground, `~/bracken-weave/manyroot`, is reached only through `scripts/weave`.** Never edit or delete files there by hand, and never copy its `identities/` anywhere: only `scripts/weave-backup` copies the whole Ground, run by hand by the manager, and only to the destination the owner approved (its `--owner-decision`). Creating it waits for the owner's go. `scripts/bk` refuses `weave`, Weave media types, `--dir`, and `sync` with anything but the team server ([docs/design/weave.md](docs/design/weave.md), section 5).
- **Coordinate only through `scripts/bk`.** It runs the pinned binary against the main checkout's Ground. When you test your own branch's binary, give it a throwaway Ground: `BRACKEN_DIR=$(mktemp -d)/g target/debug/bracken init`. Without `BRACKEN_DIR`, a binary run in a worktree finds that worktree's stale copy of `.bracken/`. `scripts/bk` always ignores `BRACKEN_DIR`.

## Conventions

- Rust 2024. Formatting follows `rustfmt.toml` (width 120). Use `thiserror` errors in `manyroot` and `anyhow` in the CLI. Outside tests, use `expect("why it can't fail")`, never a bare `unwrap()`.
- Public APIs use plain names (`contribution`, `causal heads`, `evidence`). The subsystem names (Seed, Roots, Proof…) appear only in docs and module comments.
- Nothing in `manyroot` may assume a contribution is code.
- Every read command in the CLI keeps a `--json` form; agents depend on it.
- Commit messages and PRs carry no AI attribution.
- GitHub, `wrnu/manyroot`: `gh` for PRs and Actions. Commands are in `docs/development.md`, under GitHub commands; the old Forge is the read-only remote `forge` (Remotes there).

## Tests

This table is the CI: GitHub Actions runs it on every push and PR as an observer, less the Docker-backed macOS-only tests and `--split`'s list check (`.github/workflows/ci.yml`; `scripts/ci-result` records its result), and the manager's counted run on the merged tree, main merged with the exact head, is what a merge waits for ([docs/development.md](docs/development.md)). With the Weave gate on (a current manager claim tagged `weave-gate`, `Weave gate: on`; owner approval `e8fd45c83ba6`, decision `bbbf898cbcfe`), line main's Weave results on that merged tree can stand in for the full run: `scripts/verify-seq --weave` records a pointer to them, and `scripts/merge` checks them afresh. A head that changes `scripts/test-table` or `Cargo.lock`, names no Weave proposal, or doesn't merge cleanly still takes git's run. A counted run has two modes (owner decision `c8bbb2886be3`, superseding `464d3ccca45e` and `73f2ba4c08e5`):
- **Docs-only:** when every path the head changed is a `.md` file or under `docs/`, outside `crates/` and `research/`, and is neither `docs/protocol.md` nor `docs/design/weave-freeze.md` (`scripts/tree-scope docs-only`), `scripts/test-table` runs only `cargo fmt --check` and the Ground guards.
- **Full:** everything else: fmt, both clippies, `cargo test --workspace` (under nextest, then the doctests) and the Ground guards, in a Linux box, and the macOS-only tests on the Mac (`scripts/test-table --split`). There's no light or scoped run.

| Package | Test | Lint | Format |
|---|---|---|---|
| workspace | `cargo test --workspace` | `cargo clippy --all-targets -- -D warnings` | `cargo fmt --check` |

The workspace is the product: `crates/manyroot`, `crates/bracken` and `crates/tree`. The research crates (the experiment runner, the demo and the query lab) are a sibling workspace, `research/`, with its own `Cargo.lock`, which `cargo test --workspace` at the root never builds. Test them in `research/` with `cargo test` (or `cargo nextest run`) after `cargo build -p bracken` at the root: they build into the root's `target/` and run the `bracken` beside their binaries. `.github/workflows/research.yml` runs them on Linux and macOS when `research/` or `crates/{manyroot,tree,bracken}` change.

Before every `pr` claim, run and record these four on the exact head with `bk-evidence`: `cargo fmt --check`, both clippies (`cargo clippy --all-targets -- -D warnings`, and with `--target x86_64-unknown-linux-gnu`), `cargo nextest run -P fast --workspace`, and `scripts/test-table --remote` (the whole table in a box). You need cargo-nextest: `cargo install --locked cargo-nextest` (reference `4f8a96525a1e`). The `fast` profile in `.config/nextest.toml` leaves out the slowest tests, which `--remote` and the counted run still run (decision `4bf44934cb67`). Your passes are recorded but never count toward the merge gate: the manager's counted run does. A head that changes `research/` also records `cargo test` there.

## Releases

Nothing is published, and no workflow deploys on merge. Publishing crates, making the repo public, licence changes and access or protection changes need the owner's go. See [docs/releases.md](docs/releases.md).

## The Ground

- Always run `scripts/bk … --as <your identity>`, with `BRACKEN_AGENT=<your identity>` set, so reads pull from the team server. A write that `scripts/bk` fails to confirm isn't done (process section 4.3).
- At the start of every task, read `scripts/bk log --tag decision --current` and `scripts/bk show <task id>`. `--current` leaves out retracted entries and those their own author superseded; one someone else superseded stays, marked.
- `log --tag` prints one short line per entry; `scripts/bk show <id>` prints one in full. The task-start reads stay whole: no `--since` or `--limit` on them. Use those only for a follow-up read, such as what's new since the cursor or date you last read at. List findings with `scripts/bk log --open` (those `scripts/merge` would block a current head on: untriaged by the manager, or confirmed and not deferred), never every finding.
- Check a claim with `scripts/bk show <id>` before acting on it. A `note` is a lead, not a fact.
- Contribute what others would need **as you learn it**, not at the end.
- A write prints one labelled line per record, the primary first: `claim <id>`, then `relation <id> (supersedes)`. Pass on the id from the line you mean, never just the first word; scripts use `--json` and its `id`.
- Messages carry ids; the content goes in the Ground. No secrets or personal data: contributions are permanent.
- **The Ground is public.** `.bracken/` is committed to the public GitHub repository, so every contribution is a publication, readable by anyone and kept forever (decision `473d5df9b1ab`).

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

Roles, the merge gate and everything else about how the team works are in the process, [process.md](.claude/skills/agent-team-kickoff/process.md). This repo's values for it (models, risky classes, GitHub commands) are in [docs/development.md](docs/development.md). Builders start with [docs/builder.md](docs/builder.md). "Process section N" means a section of `process.md`.
