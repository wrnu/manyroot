# Agent guide

## Things that break production

- **Protocol changes can make every existing Ground unreadable**, this repo's own `.bracken/` included. That covers encoding, ids, the signing context and validity rules (`crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`). They're the `protocol` class: independent review, the manager's go (logged as `owner-review`), and a migration note in `docs/releases.md`.
- **Never commit `.bracken/identities/`.** It holds unencrypted private keys. If git ever shows a file there as staged or tracked, stop and tell the manager.
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

This table is the CI: GitHub Actions runs it on every push and PR as an observer (`.github/workflows/ci.yml`; `scripts/ci-result` records its result), and the manager's counted run on the merged tree, main merged with the exact head, is what a merge waits for ([docs/development.md](docs/development.md)). With the Weave gate on (a current manager claim tagged `weave-gate`, `Weave gate: on`; owner approval `e8fd45c83ba6`, decision `bbbf898cbcfe`), line main's Weave results on that merged tree can stand in for the full run: `scripts/verify-seq --weave` records a pointer to them, and `scripts/merge` checks them afresh. A head that changes `scripts/test-table` or `Cargo.lock`, names no Weave proposal, or doesn't merge cleanly still takes git's run. The manager chooses how much of the table it covers, by risk (owner decision `464d3ccca45e`, superseding `20a06f9b815e` for counted runs):
- **Light:** the tests and doctests of the packages your changed files affect (`scripts/code-scope packages` decides them; the table below documents it), fmt, both clippies and the Ground guards, on the merged tree, in a box plus their macOS-only tests on the Mac (`scripts/test-table --split -p`). It needs your recorded passes on the exact head of `cargo fmt --check`, both clippies, `cargo nextest run -P fast -p <packages>` and `scripts/test-table --remote -p <packages>`, each naming at least those packages. The fast pass may also carry `-j <n>`, `--no-fail-fast` and `--test-threads <n>` among its `-p` arguments, and nothing else (task `b1156a629af9`). They're its precondition only: nothing counted rests on them.
- **Full:** the whole table, which is also the default. It's always required for a head that touches `crates/manyroot`, the root `Cargo.toml` or `Cargo.lock`, `docs/protocol.md`, anything in `crates/tree/src/`, `crates/tree/Cargo.toml`, the weave-v1 fixtures, `.cargo/`, `rust-toolchain*` or any `build.rs`. `scripts/code-scope full` holds that list, and `scripts/verify-seq --light` and `scripts/merge` both refuse a light run for such a head.

| Package | Test | Lint | Format |
|---|---|---|---|
| workspace | `cargo test --workspace` | `cargo clippy --all-targets -- -D warnings` | `cargo fmt --check` |

Builders don't run the whole `cargo test --workspace` (decision `ef5b807d3196`): the manager's counted run does. Before every `pr` claim, run the lint and format checks (`cargo clippy` for this machine and with `--target x86_64-unknown-linux-gnu`), then `cargo nextest run -P fast` with the packages `scripts/code-scope packages origin/main HEAD` prints, one `-p` each, and `cargo test --doc` with those of them that have a library. That's every package but `bracken`, which is a binary only: `cargo test --doc -p bracken` fails with "no library targets", so when `-p bracken` is your whole scope there's no doctest step. You need cargo-nextest: `cargo install --locked cargo-nextest` (reference `4f8a96525a1e`). The `fast` profile in `.config/nextest.toml` leaves out the slowest tests, which the counted run still runs (decision `4bf44934cb67`). Then run `scripts/test-table --remote -p <the same packages>` once. Record each of these on the exact head with `bk-evidence`: a light counted run needs all five, with `-p` packages, not `--workspace`.

`scripts/code-scope packages` decides the packages, not this table (owner decision `73f2ba4c08e5`, task `de42fa5c776d`). It prints every package when `scripts/code-scope full` names a path. Otherwise it runs `crates/scope` (`bracken-scope`), guppy's determinator: a path in a crate selects that crate and every package that depends on it in the real cargo graph, and `.config/determinator.toml` maps the paths outside the crates. Any change to `bracken`'s build, a path in it or in a crate it depends on, also selects `bracken-demo` and `bracken-experiment`, whose tests run its binary though cargo's graph doesn't show it (`RUN_BINARY` in `crates/scope/src/packages.rs`). This table documents what it prints on this repository; when the rules or the graph change, the table follows:

| You changed | Packages |
|---|---|
| a path in `scripts/code-scope full`'s list: root `Cargo.toml` or `Cargo.lock`, `crates/manyroot`, `crates/tree/src/`, `crates/tree/Cargo.toml`, the weave-v1 fixtures, `docs/protocol.md`, `.cargo/`, `rust-toolchain*`, any `build.rs` | every package (the full table) |
| the rest of `crates/tree` | `-p bracken -p bracken-demo -p bracken-experiment -p bracken-tree` |
| `crates/bracken` | `-p bracken -p bracken-demo -p bracken-experiment`: demo's and experiment's tests run the `bracken` binary |
| `crates/demo` | `-p bracken -p bracken-demo -p bracken-experiment`: `-p bracken` builds the binary their tests run |
| `crates/experiment` | `-p bracken -p bracken-experiment` |
| `crates/scope` | `-p bracken-scope` |
| `crates/query-lab` | `-p bracken-query-lab`: a research prototype nothing depends on |
| `code-scope`, in `scripts/` or `.claude/skills/` | `-p bracken -p bracken-scope`: bracken-scope's tests run it too |
| `scripts/`, `.claude/skills/` | `-p bracken` |
| `.config/determinator.toml` | `-p bracken -p bracken-scope -p bracken-tree` |
| docs, `.bracken/`, the rest of `.claude/` and `.config/`, `.github/`, and the root files the rules name | `-p bracken -p bracken-tree`: their tests import every tracked file |
| `rustfmt.toml` | none: the format check alone |
| anything else, such as a new root file or directory | every package, until a rule in `.config/determinator.toml` names it |

A Rust file changed only in comments takes a doc's row, `-p bracken -p bracken-tree`, and never needs the full table (task `0837da3df7a4`): `code-scope` asks for README.md's packages in its place. That's the same tokens on both sides, by `bracken-scope comment-only`, where a doc comment is code, since rustdoc and doctests read it. It isn't docs-only, since clippy reads plain comments too (finding `c40ed7b8d3f8`), so its head gets a light run.

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
