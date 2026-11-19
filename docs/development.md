# Development: live settings

What agents follow is [AGENTS.md](../AGENTS.md). This file holds only this repository's live settings: the values the rules use, and where the tools are. Each script's header is its manual. The longer explanations this file used to carry, with their history, are in [archive/development.md](archive/development.md), for reference only: nothing there is a rule.

## Profile

| Field | Value |
|---|---|
| Goal | [vision.md](vision.md); in the Ground, the current `goal` claim (`scripts/bk log --tag goal --current`), which depends on the `docs/vision.md` artifact |
| Change classes | docs, process, cli, library, merge-gate tooling, crypto, protocol, storage. Risky: **protocol** (`crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`), **storage** (`store.rs`, `bundle.rs`), **crypto**, **merge-gate tooling** (`scripts/`, `.github/`, the pinned-binary procedure). Release path per class: [releases.md](releases.md) |
| Models | Builders, manager, security and other delegates: Claude Opus 5.5 (`claudeAgent` / `claude-opus-5-5`), medium effort. Independent reviewer: Gemini 4 Neon (High) on Antigravity, for risky classes and manager-authored PRs. No Codex (decision `b1b56d75221b`) |
| Remote | Public `wrnu/manyroot` on GitHub (decision `761565b7ffb9`), through `gh`'s own login. The Forge, `forge.jx.rs/Autimo/manyroot`, is the read-only remote `forge` |
| CI | Observer (decision `d9fa2c08146c`): `.github/workflows/ci.yml` runs on every push and PR, reports only, and no merge waits on it. `"$main/scripts/ci-result" --record <pr claim> --as <you>` records a head's run; when it disagrees with the counted run, the manager records a `ci-disagreement` claim |
| Branch protection | None (a free account can't). Never force-push or delete `main` |
| Deploys | Nothing deploys on merge; nothing is published |
| Licence | `MIT OR Apache-2.0` (decision `42ac996e6355`) |

## Review context

The PROJECT paragraph every review brief opens with (AGENTS.md, section 5):

> **PROJECT:** Manyroot is a global, decentralized network where knowledge accumulates through independent contributions from humans and AI agents, and Bracken, collaborative software development for large agent teams, is its first application and proving ground ([vision.md](vision.md); goal `22af6f97`). The team's own work runs on Bracken: nothing reaches users untested, the owner decides only what matters, and agents work in parallel without losing track or stepping on each other.

## Owner approvals

The owner's own approval is needed only for the list in AGENTS.md, section 1: a frozen protocol or format change or the join cutover; spending beyond an approved budget, a new paid service or a schedule; repo visibility, licence, branch protection, access or publishing; destructive history, data or key actions (force-pushing `main`, deleting or rewriting Ground objects, moving root or owner keys). Everything else is a claim tagged `decision` the manager signs; where it acts on an owner's answer in chat, it quotes it, dated (task `9b65c13e9d66`, owner decision `c8bbb2886be3`).

- **How the owner approves:** `scripts/owner-approve <id> "<what they approve>"`, in a terminal. `<id>` is what's approved: a task, a run's claim, or, for a merge, the current `pr` claim itself. It shows that contribution, asks the owner to type `approve`, and contributes, as `owner`, a claim tagged `approval` that `supports` it, printing its id. The manager's `--delegated` signing is gone; approvals signed that way stay valid records, tagged `delegated`, and count for no new merge.
- **The `owner` team token** is the owner's go, issued only by the manager with that go. Without it, `scripts/bk` refuses every write `--as owner`.
- **For a merge that needs it,** `scripts/merge … --owner-approval` requires a current approval the owner ran that `supports` the current `pr` claim itself; each new head needs a new one.
- **Weave's frozen formats** (decision `bae92c497714`, [design/weave-freeze.md](design/weave-freeze.md)): `scripts/merge`'s `frozen` check, through main's `scripts/code-scope frozen`, refuses a head changing a frozen path without the manager's `owner-review` claim naming the `pr` claim and the head's sha; a golden vector, pinned fixture value or `crates/tree/tests/fixtures/` also needs an owner approval of that `pr` claim; and the freeze's refusals (`crates/tree/src/`, `crates/bracken/src/weave.rs`, `crates/bracken/src/weave/`) need a `reviewer` pass, `independent review (<model>) @ <sha>`.
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

## Weave shadow (stages 1 and 2a)

After each merge, once it's in the main checkout and the pinned binary is upgraded, the manager runs `scripts/weave-shadow --proposal <id> <merged sha>` with the `Weave proposal` the PR's last `pr` claim names (none: no `--proposal`). `outcome conflict` needs nothing. "Stop and report it" means Weave's merge differs from git's: tell the owner, with the paths, and only then rerun with `--mismatched`; never force a merge to match. Any other non-zero exit: stop the shadow and tell the owner. `scripts/weave-backup --owner-decision <approval> <dest>` copies the Weave Ground only to a destination the owner approved. The Weave gate is on while a current manager claim tagged `weave-gate` reads `Weave gate: on` (owner approval `e8fd45c83ba6`).

## Remote test runs (boat.dev)

`scripts/test-table --remote` runs the table in a fresh Linux box (the $20/month `box_20` plan, decision `2a9e444218d6`; `boat limits` shows what's left). It commits nothing, pushes only a bundle of HEAD and `origin/main`, and deletes the box on every way out. By hand: `boat run --no-env --push <dir> -- <cmd>`, never `-e`, then check `boat list --all` is empty. A box has no Docker or macOS, so the macOS-only tests (`mod macos_only`) run on this Mac.

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
