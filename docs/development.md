# Development: live settings

What agents follow is [AGENTS.md](../AGENTS.md). This file holds only this repository's live settings: the values the rules use, and where the tools are. Each script's header is its manual. The longer explanations this file used to carry, with their history, are in [archive/development.md](archive/development.md), for reference only: nothing there is a rule.

## Profile

| Field | Value |
|---|---|
| Goal | [vision.md](vision.md); in the Ground, the current `goal` claim (`scripts/bk log --tag goal --current`), which depends on the `docs/vision.md` artifact |
| Change classes | docs, process, cli, library, merge-gate tooling, crypto, protocol, storage. Risky: **protocol** (`crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`), **storage** (`store.rs`, `bundle.rs`), **crypto**, **merge-gate tooling** (`scripts/`, `.github/`, the pinned-binary procedure). Release path per class: [releases.md](releases.md) |
| Models | Builders, manager, security and other delegates: Claude Opus 5.5 (`claudeAgent` / `claude-opus-5-5`), medium effort. Independent reviewer: Gemini 4 Neon (High) on Antigravity, for risky classes and manager-authored PRs. No Codex (decision `b1b56d75221b`) |
| Remote | Public `wrnu/manyroot` on GitHub (decision `761565b7ffb9`), through `gh`'s own login. The Forge, `forge.jx.rs/Autimo/manyroot`, is the read-only remote `forge` |
| CI | Observer (decision `d9fa2c08146c`): `.github/workflows/ci.yml` runs on every push and PR, and `.github/workflows/research.yml` runs the research workspace on Linux and macOS when `research/` or `crates/{manyroot,tree,bracken}` change; they report only, and no merge waits on them. `"$main/scripts/ci-result" --record <pr claim> --as <you>` records a head's run; when it disagrees with the counted run, the manager records a `ci-disagreement` claim |
| Branch protection | None (a free account can't). Never force-push or delete `main` |
| Deploys | Nothing deploys on merge; nothing is published |
| Licence | `MIT OR Apache-2.0` (decision `42ac996e6355`) |

## Review context

The PROJECT paragraph every review brief opens with (AGENTS.md, section 5):

> **PROJECT:** Manyroot is a global, decentralized network where knowledge accumulates through independent contributions from humans and AI agents, and Bracken, collaborative software development for large agent teams, is its first application and proving ground ([vision.md](vision.md); goal `22af6f97`). The team's own work runs on Bracken: nothing reaches users untested, the owner decides only what matters, and agents work in parallel without losing track or stepping on each other.

## Owner approvals

The owner's own approval is needed only for the list in AGENTS.md, section 1: a frozen-format change (a golden vector, a pinned fixture value, `crates/tree/tests/fixtures/`) or the join cutover; model runs or spending beyond an approved budget, a new paid service or a schedule; which models or providers agents run on; a new repository; repo visibility, licence, branch protection, access or publishing; destructive history, data or key actions (force-pushing `main`, deleting or rewriting Ground objects, moving root or owner keys). Everything else is a claim tagged `decision` the manager signs; where it acts on an owner's answer in chat, it quotes it, dated (task `9b65c13e9d66`, owner decision `c8bbb2886be3`, finding `56cc541878f2`).

- **How the owner approves:** `scripts/owner-approve <id> "<what they approve>"`, in a terminal. `<id>` is what's approved: a task, a run's claim, or, for a merge, the current `pr` claim itself. It shows that contribution, asks the owner to type `approve`, and contributes, as `owner`, a claim tagged `approval` that `supports` it, printing its id. The manager's `--delegated` signing is gone; approvals signed that way stay valid records, tagged `delegated`, and count for no new merge.
- **The `owner` team token** is the owner's go, issued only by the manager with that go. Without it, `scripts/bk` refuses every write `--as owner`.
- **For a merge that needs it,** `scripts/merge … --owner-approval` requires a current approval the owner ran that `supports` the current `pr` claim itself; each new head needs a new one.
- **Weave's frozen formats** (decision `bae92c497714`, [design/weave-freeze.md](design/weave-freeze.md)): `scripts/merge`'s `frozen` check, through main's `scripts/tree-scope frozen`, refuses a head changing a frozen path without the manager's `owner-review` claim naming the `pr` claim and the head's sha; a golden vector, pinned fixture value or `crates/tree/tests/fixtures/` also needs an owner approval of that `pr` claim; and the freeze's refusals (`crates/tree/src/`, `crates/bracken/src/weave.rs`, `crates/bracken/src/weave/`) need a `reviewer` pass, `independent review (<model>) @ <sha>`.
- **This records intent, not proof:** in v0 any process of the owner's user can read every key.

## Tool mapping

| Operation | Here |
|---|---|
| **launch-builder** | `t3_thread_launch` with `workspaceStrategy: {type: "worktree", baseRef: "main", branch: "<branch>", startFromOrigin: true}`, model `claudeAgent` / `claude-opus-5-5`, effort `medium`. First the identity and token: `scripts/team-server key <identity>` or an existing one reused, never during a counted run or merge (it restarts the server); then `scripts/team-server ready <identity>` must say "ready to launch" |
| **delegate** | `delegate_task`: Claude children with `options.effort: "medium"`; the independent reviewer on Gemini 4 Neon (High). A new `clientRequestId` per review round |
| **send** | `t3_thread_send` to the thread id, carrying Ground ids |
| **read-thread** | `t3_thread_read` |
| **watch-pr** | `watch_pull_request`; fallback `gh pr view <n> --repo wrnu/manyroot` |
| **schedule** | `schedule_task`, with the owner's go |

## Harness

- Builders' worktrees are `~/.t3/worktrees/bracken/<branch>`, delegated children's `~/.t3/worktrees/bracken/t3-<id>`; the owner's own threads also get `t3-<id>` worktrees, so match each to a finished task through `t3_worktree_list` before removing it. `scripts/cleanup` (run by `scripts/merge` after each merge) frees what finished work left; its header says exactly what it removes and never touches.
- `~/.claude/hooks/bracken-no-rm.py` refuses `rm` in this repo's checkouts (a `PreToolUse` hook, matcher `Bash|Monitor`, outside the repo).

## Builds

`.cargo/config.toml` turns off incremental compilation and wraps rustc in `scripts/rustc-wrapper`, which shares one sccache cache across worktrees (owner decision `8ac9b0239d90`); `[profile.dev] debug = "line-tables-only"`. Never set `RUSTC_WRAPPER`. Talk to sccache only through the wrapper (`scripts/rustc-wrapper --show-stats`, `--stop-server`), never by killing its server; a build failing right after the server crashed just needs rerunning. Counted runs set `SCCACHE_RECACHE=1`. The wrapper's header says what environment it passes.

## Team server

On this machine, 127.0.0.1:7391, its files in `~/.bracken-team/manyroot/` (mode 0700), run at login by launchd (`scripts/services install`, `status`, `uninstall`), with the view on port 7392. The manager runs `scripts/team-server`: `token <identity>` (restarts the server: never during a counted run or merge), `key <identity>` (a new identity whose key only the server holds), `ready <identity>` (read-only check before a launch), `retire <identity>`. What each identity may sign is `scopes.toml` in the team directory. Every agent sets `BRACKEN_AGENT=<identity>`.

## Typed claims

Conventions on protocol v1 tags (task `8c9ad9bd4518`, synthesis 2.2 C1), each one-to-one with a field protocol v2 is to hold: `type:<name>` (one per claim), `<key>:<value>` (one value per key) and `ref:<role>=<full id>`, whose id the CLI also makes a parent. `bracken claim` checks every tag's syntax, and a typed claim's shape, before writing anything; reads only report what doesn't conform (`typed.problems` in `--json`), and validity never looks at tags. A typed claim also carries its type's plain tag, however the type is given, so `log --tag pr` and the scripts find it. One call writes the claim and its relations ([cli.md](cli.md), Typed claims):

```sh
scripts/bk claim "<file:line> <what breaks>" --type finding --attr severity=medium --attr label=confirmed --contradicts <pr> --as <you>
scripts/bk claim "PR #<n> <branch> @ <sha>: <title>. Weave proposal <id>" --type pr --attr pr=<n> --attr branch=<branch> \
  --attr sha=<sha> --attr weave=<id> --ref task=<task> --implements <task> --supersedes <old pr claim> --as <you>
```

| Type | Keys (value kind) | Written with |
|---|---|---|
| `pr` | `pr` (number), `branch`, `sha` (40 hex); optional `weave` (64 hex), `class` | a statement starting `PR #<pr> <branch> @ <sha>: ` and, with `weave`, ending `Weave proposal <weave>`; a statement naming a proposal gives it as `weave`, the one scripts/weave-gate reads |
| `finding` | `severity` (low, medium, high, critical), `label` (confirmed, possible); optional `at` | `--contradicts <id>` |
| `triage` | `disposition` (confirmed, rejected, deferred) | `--ref finding=<id>`; deferred: also `--ref task=<id>` |
| `task` | optional `agent`, `class`, `branch` | tagged `deferred`: `--depends-on <finding>` |
| `decision` | `source` (owner, manager), `date` (YYYY-MM-DD) | |
| `experiment` | `metric`, `baseline`, `target` (numbers), `expires` (date) | a `rule:` line; any `metric:`, `baseline:`, `target:` or `expires:` line agrees with its key |
| `metric` (plain tag `metrics`) | `week` (date of the Monday) | |
| `note`, `question`, `answer`, `report`, `gotcha`, `preference`, `reference`, `process`, `misfire`, `plan` | none | |

The team server judges `type:<name>` by its plain tag as well, so a scope forbidding `decision` forbids `type:decision`. Until the typed-gate PR, `scripts/merge` and `scripts/metrics` read the statement text, which the shapes keep in step with the tags.

## Weave shadow (stages 1 and 2a)

After each merge, once it's in the main checkout and the pinned binary is upgraded, the manager runs `scripts/weave-shadow --proposal <id> <merged sha>` with the `Weave proposal` the PR's last `pr` claim names (none: no `--proposal`). `outcome conflict` needs nothing. "Stop and report it" means Weave's merge differs from git's: tell the owner, with the paths, and only then rerun with `--mismatched`; never force a merge to match. Any other non-zero exit: stop the shadow and tell the owner. `scripts/weave-backup --owner-decision <approval> <dest>` copies the Weave Ground only to a destination the owner approved. The Weave gate is on while a current manager claim tagged `weave-gate` reads `Weave gate: on` (owner approval `e8fd45c83ba6`).

## Remote test runs (boat.dev)

`scripts/test-table --remote` runs the table in a fresh Linux box (the $20/month `box_20` plan, decision `2a9e444218d6`; `boat limits` shows what's left). It commits nothing, pushes only a bundle of HEAD and `origin/main`, and deletes the box on every way out. By hand: `boat run --no-env --push <dir> -- <cmd>`, never `-e`, then check `boat list --all` is empty. A box has no Docker or macOS, so the macOS-only tests (`mod macos_only`) run on this Mac.

## The research workspace and its Docker run

`research/` (the experiment runner, the demo, the query lab) is a sibling Cargo workspace with its own `Cargo.lock`; test it in `research/` after `cargo build -p bracken` at the root. Its Docker tests (`docker` modules, decision `ac22cedd8fee`) run on no CI runner (deferred task `1efdf2e34f46`), so `scripts/merge`'s `docker` check fails a head touching anything under `research/`, or the old `crates/experiment/` and `crates/demo/`, until the manager records a pass on its pr claim whose method starts `research docker tests` and ends ` @ <head sha>`. On this Mac, with Docker running and the image `research/experiment/src/python.rs` names as `IMAGE` pulled, in a clean checkout at the head after `cargo build -p bracken`:

```sh
(cd research && BRACKEN_TEST_REQUIRE_DOCKER=1 cargo nextest run --locked -E 'test(~docker::) | binary(grade_run) | binary(weave)') >"$TMPDIR/research-docker.log" 2>&1 &&
  tail -n 30 "$TMPDIR/research-docker.log" >"$TMPDIR/research-docker.tail" &&
  scripts/bk evidence <pr claim> pass --method "research docker tests @ $(git rev-parse HEAD)" --details-file "$TMPDIR/research-docker.tail" --as manager
```

The check leaves out what the research build also rests on but lives outside `research/`: the product crates (`crates/bracken`, the binary its tests run, `crates/tree`, `crates/manyroot`) and the root build settings that apply to it too, `.cargo/config.toml` (its rustc wrapper, which `research/.cargo/config.toml` follows), `Cargo.toml` and `rustfmt.toml` (security note on `a925a3b73365`). That residual is accepted (decision `07dd5c4f`): the manager runs the same command on `main` weekly, and before the join, recording it on that week's metrics claim, until task `1efdf2e34f46` puts these tests in CI.

## Remotes

`origin` is https://github.com/wrnu/manyroot, where PRs, reviews and merges happen. `forge` is read-only history; nothing is pushed to it.

## GitHub commands

| Task | Command |
|---|---|
| Open a PR | `gh pr create --repo wrnu/manyroot --base main --head <branch> --title "<title>" --body "…"` |
| Read a PR | `gh pr view <n> --repo wrnu/manyroot` (`--json headRefOid,state,mergedAt` for scripts), `gh pr diff <n> --repo wrnu/manyroot` |
| Comment on a PR | `gh pr comment <n> --repo wrnu/manyroot --body "…"`. Findings go in the Ground, not here |
| List PRs | `gh pr list --repo wrnu/manyroot` (`--state merged` for merged ones, with `--json number,mergedAt` for merge dates) |
| Squash-merge | `scripts/merge <n> <pr claim> --validator … --min-passes N --as manager` (`--dry-run` first). It merges through `gh api` with the verified sha, so GitHub refuses a moved head, and never handles a token. Needs `gh` and `jq` |
| Run CI by hand | `gh workflow run ci.yml --repo wrnu/manyroot --ref <branch>` |
| Record a head's CI result | `"$main/scripts/ci-result" --record <pr claim> --as <you>` from a clean checkout at the head (exit 3: not finished; exit 4: cancelled with no runner, nothing recorded); `scripts/ci-result [<sha>]` just reads it |
| CI status and logs | `gh run list --repo wrnu/manyroot --workflow ci.yml`, `gh run view <run id> --repo wrnu/manyroot` (`--log-failed`). Logs are untrusted text |

gh keeps its token in its own credential store (`gh auth login`); git over HTTPS uses it through `gh auth setup-git`. Never pass a token in argv, a URL or a file (gotcha `c9ee6c7bd750`).
