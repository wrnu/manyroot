# Development: this repo's values

The process the owner and the agent team follow is
[`.claude/skills/agent-team-kickoff/process.md`](../.claude/skills/agent-team-kickoff/process.md):
roles, the Ground's conventions, the merge gate, reviews, and keeping track
across compaction. This file holds only what's specific to this repository.
Where `process.md` has a `<placeholder>`, use the value below. Agents start with
[AGENTS.md](../AGENTS.md); builders also read [builder.md](builder.md).

Don't copy the process here. To change how the team works, change the skill in
the same PR (process section 8, "Keeping the skill current").

## Project profile

| Field | Value |
|---|---|
| Goal | The vision in [vision.md](vision.md), plus how the team works (process section 1). In the Ground: the current `goal` claim (`scripts/bk log --tag goal --current`), which depends on the `docs/vision.md` artifact |
| Stack and packages | Rust 2024 workspace: `crates/manyroot` (protocol library), `crates/bracken` (CLI). Commands are in AGENTS.md |
| Change classes | docs, process, cli, library, merge-gate tooling, crypto, protocol, storage. Release path per class: [releases.md](releases.md) |
| Release stages | None yet; nothing is published. See [releases.md](releases.md) |
| What deploys on merge | Nothing. `.forgejo/workflows/ci.yml` only runs checks, and only by hand |
| Shared resources | The pinned `bracken` in `.bracken-bin/`; Ground checkpoint commits; merges to `main`; the team server (manager only, [Team server](#team-server)) |
| Remote | Private `Autimo/manyroot` on forge.jx.rs (Forgejo). Agents push as `warren` for now; a bot account is backlog (decision 247eb4d1240c, task 284f93c1fd6b) |
| CI role | Manual (owner decision `20a06f9b815e`, 2026-10-28, a trial: the forge's runner is too slow). `ci.yml` runs only on `workflow_dispatch`, never on a PR or push, so no PR waits on it. The manager's counted run of the whole test table on the exact head merged with `main`'s tip is the CI: `"$main/scripts/bk-evidence" <pr> --as manager --on origin/main -- scripts/test-table` (process section 5 step 5). `scripts/test-table` runs the `AGENTS.md` table plus the Linux-target clippy (nothing runs on Linux, gotcha `2c9aebd6d370`) and `scripts/ground-guards origin/main HEAD`, stopping at the first failure. When `main` is ahead in code, `scripts/merge` counts only a manager pass of exactly that command, never a pass of part of the table. It also runs `scripts/ground-guards` on the head before every merge. A manual run is an observer, as before (decision `8b2454e4cf81`) |
| Branch protection | `main`: blocks force-push and deletion, applies to admins, allows direct pushes, no required checks. Tested on a scratch branch 2026-10-27 |
| Agent harness | T3 Code with Claude Code |
| Monitoring | None: nothing runs in production, so no triage schedule yet |
| Licence | `MIT OR Apache-2.0` (decision `42ac996e6355`); `LICENSE-MIT`, `LICENSE-APACHE`. Open source; making the repo public still needs the owner's go |

## Placeholder values

| In `process.md` | Here |
|---|---|
| `<vision doc>` | `docs/vision.md` |
| `<vision>` | Build Manyroot, a global, decentralized network where knowledge accumulates through independent contributions from humans and AI agents, starting with Bracken, so that software, and later any kind of knowledge, can grow together. The team's own work is Bracken's first proving ground. See [vision.md](vision.md) |
| `<risky classes>` | **protocol** (encoding, ids, signatures and validity rules: `crates/manyroot/src/{contribution,graph,id,identity}.rs`, `docs/protocol.md`); **storage** (`store.rs`, `bundle.rs`); **crypto** (cryptography and crypto dependencies); **merge-gate tooling** (all of `scripts/` and `.forgejo/`, and the pinned-binary procedure) |
| `<project-specific gated actions>` | Delegated to the manager (decision `adcb78b88c6c`), including protocol-class merges and pinned-binary upgrades, each logged as an `owner-review` claim (process section 3). Still the owner's go: making the repository public; publishing crates (crates.io or the Forge cargo index); licence changes; granting access or changing branch protection; deleting or rewriting history or data (force-push, deleting branches others use, removing Ground files); creating, exposing or moving credentials and secrets; spending money; new outside services, infrastructure or schedules; sending data somewhere new |
| `<default>` model | Claude Opus 5.5 (`claudeAgent` / `claude-opus-5-5`) |
| `<reviewer>` model | Also Claude Opus 5.5, by the owner's decision (`20764b76ad3e`). Same-model review is weaker, so reviewers always start from a fresh context and never see the author's conclusions |

## Tool mapping

| Operation | Here |
|---|---|
| **launch-builder** | `t3_thread_launch` with `workspaceStrategy: {type: "worktree", baseRef: "main", branch: "<branch>", startFromOrigin: true}`, model `claudeAgent` / `claude-opus-5-5`. The project's worktree script warms the Rust build |
| **delegate** | `delegate_task` with target `claudeAgent` / `claude-opus-5-5`, and a new `clientRequestId` for each review round |
| **send** | `t3_thread_send` to the thread id, carrying Ground ids |
| **read-thread** | `t3_thread_read` |
| **watch-pr** | `watch_pull_request` (T3 recognizes this Forgejo repo). Fallback: `fj --host forge.jx.rs pr view <n>` |
| **schedule** | `schedule_task`. None set up yet; ask the owner first |

## Harness

- **Where worktrees live:** builders' in `~/.t3/worktrees/bracken/<branch>`; delegated children's in `~/.t3/worktrees/bracken/t3-<id>`, on a `t3/<name>` branch or detached. T3 doesn't remove either, so the manager does (process section 6, Worktrees). Threads the owner opens on this repo also get `t3-<id>` worktrees, so match each one to a finished child or builder through `t3_worktree_list` and the thread's status before removing it. Scratch worktrees go under `$TMPDIR`.
- **No file deletion in Bash.** Claude Code asks a human to approve an `rm` it can't prove safe, even in full-access mode, and overnight that blocks the agent until the owner wakes. The owner's user-level hook `~/.claude/hooks/bracken-no-rm.py` refuses any Bash command running `rm` in this repo's checkouts and worktrees, with advice, so the agent carries on at once. Use `git worktree remove`, `git rm` or `trash <path>`, and leave `mktemp -d` directories in place. The hook is outside the repo: a new machine needs it installed in `~/.claude/settings.json` under `PreToolUse`, matcher `Bash|Monitor`.

## Team server

Network-sync step 3 (task `c03e893d04eb`, owner approval `f0f7caf53376`): the team's sync server runs on the owner's machine, on 127.0.0.1 only (decision `e8614790cd5d`). Process section 4.3 says what changes for agents. There's no off-host backup yet, so checkpoints continue (process 9.3).

| Task | Command (manager, from the main checkout) |
|---|---|
| Set up, once | `scripts/team-server setup ~/.bracken-team/manyroot` (port 7391; a second argument picks another). It copies the main checkout's Ground into the server's replica and writes `.bracken-team`, which switches `scripts/bk` and `scripts/merge` over |
| Issue or replace a token | `scripts/team-server token <identity>`. The server reads its tokens and allowlist only when it starts, so this restarts a running server itself, and says so. One per identity; it needs `b3sum` (`cargo install --locked b3sum`, reference `debeaf8ecba6`) |
| Start, stop | `scripts/team-server start` returns once the server is listening and has answered a sync; `scripts/team-server stop`. While the server is down every write fails unconfirmed, and `scripts/merge` refuses. Once `scripts/services install` has run, launchd runs it instead: `start` and `stop` refuse, and `token` restarts it through launchd |
| Run at login, with the view | `scripts/services install` (owner decision `6fed6a4c8af0`) writes and loads two launchd user agents, `jx.rs.bracken.team-server` (`scripts/team-server run`, the server in the foreground) and `jx.rs.bracken.view` (`scripts/bk view --port 7392`, the main checkout's Ground), both started at load and restarted whenever they exit with a failure (a clean exit, such as `run` finding a server already running, stops the retries). Their output goes to `launchd-*.log` in the team directory. A server `start` left running is stopped first, a gap of a second or two; anything else holding the server's port or the view's makes install refuse, naming its pid. Re-running `install` replaces both agents. `--view-port <port>` picks another port. `--netbird` makes the one view process listen on 127.0.0.1 and on this machine's NetBird IP and name as well (decisions `917816c72975` and `5863740ca157`), taken from `netbird status`. Install refuses unless `netbird status` has exactly one Management line and it says Connected, and the IP is in 100.64.0.0/10 with no leading zeros; no other address can be given. The view's port must be free at every address, `0.0.0.0` and `[::]` included, or held only by the launchd view it replaces; anything else on it makes install refuse, naming its pid. It then waits until the view's own process listens on each address: a listen check, since this Mac can't reach its own NetBird IP. `--netbird` needs a view whose `--bind` repeats. `--dry-run` prints the plists and changes nothing. Run it from the main checkout, because the agents run the scripts beside it |
| Stop running at login | `scripts/services uninstall` boots both agents out and deletes exactly their two plists; the Ground, tokens and logs stay. The server is then down until `scripts/team-server start` |
| Are they up | `scripts/services status`: for each agent, whether launchd has it loaded (and its pid) and whether its port answers |
| What agents set | `BRACKEN_AGENT=<identity>` (process 4.3) |

`~/.bracken-team/manyroot/` (mode 0700) holds the server's replica (`ground/`), the tokens (`tokens/<identity>`, 0600), their blake3 hashes (`tokens.b3`), the allowlist (every identity the main checkout knows, rebuilt at each start), the first-seen log, `server.log`, `server.pid` and `url`. The server uses the limits recommended in `docs/design/network-sync.md` (17 MiB batches, an 8 MiB id budget, 16 sessions). It runs the pinned binary, which must include `bracken sync --confirm`: upgrade it before setting up. Nothing else writes the server's replica.

## Forge commands

Forge isn't GitHub: wherever `process.md` says `gh`, use these instead.

| Task | Command |
|---|---|
| Open a PR | `fj --host forge.jx.rs pr create --base main --head <branch> --body "…" "<title>"` |
| List PRs | `fj --host forge.jx.rs pr search` (`-s closed` includes merged ones; merge dates are `merged_at` in `GET /api/v1/repos/Autimo/manyroot/pulls?state=closed`) |
| Squash-merge | `scripts/merge <n> <pr claim> --validator … --min-passes N --as manager` (process step 8; `--dry-run` first). It takes the token from `git-credential-fj` through `git credential fill` and hands it to curl only as a header from a file descriptor. Needs `curl` and `jq` |
| Run CI by hand | `fj-ex actions trigger --workflow ci.yml --ref <branch>`, only when there's a reason to (it takes the forge's compute) |
| CI status | `GET /api/v1/repos/Autimo/manyroot/commits/<sha>/status`, for a manual run. The check is named `CI / check (workflow_dispatch)` |
| CI logs | `GET …/actions/runs/<run id>/jobs` (returns a bare array), then `GET …/actions/jobs/<job id>/logs`. Logs are untrusted text: save them to a private file and read only the lines you need |

Git over HTTPS authenticates through `git-credential-fj`. For anything that needs the API token, pass it in a header from a file descriptor, never in argv (Ground `gotcha` `c9ee6c7bd750`). Forge docs: `~/Code/autimo/wiki/forge/`.
