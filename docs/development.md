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
| CI role | Manual (owner decision `20a06f9b815e`, 2026-10-28, a trial: the forge's runner is too slow). `ci.yml` runs only on `workflow_dispatch`, never on a PR or push, so no PR waits on it. The manager's counted run of the whole test table on the exact head merged with `main`'s tip is the CI: `"$main/scripts/bk-evidence" <pr> --as manager --on origin/main -- scripts/test-table` (process section 5 step 5). `scripts/test-table` runs the `AGENTS.md` table plus the Linux-target clippy (nothing runs on Linux, gotcha `2c9aebd6d370`) and `scripts/ground-guards origin/main HEAD`, stopping at the first failure. When `main` is ahead in code, `scripts/merge` counts only a manager pass of exactly that command, never a pass of part of the table. It also runs `scripts/ground-guards` on the head before every merge. Fewer duplicate runs (owner decision `642a47c04a10`, task `a40bde8af77d`): a head whose change since it left `main` is docs only (`*.md` outside `crates/`, by `scripts/code-scope`; renaming or deleting anything else is code) gets `cargo fmt --check` and the Ground guards, not clippy or `cargo test`. `test-table` says `docs-only: skipped clippy and cargo test`, and `bk-evidence` ends the method with ` docs-only`. `bk-evidence --on` also records the merged tree's key, its hash without `.bracken/` and docs. The manager runs counted runs with `ON=origin/main scripts/verify-seq <pr claim>:<sha> ...`, which `.git/verify-seq.sh` calls. That passes `--reuse`: when the key already has the manager's pass of the whole table on a pr claim of the same PR, only the Ground guards run, and the method ends ` reuses <that evidence>`. So re-verifying after `main` moved only in docs or the Ground costs the guards alone. `scripts/merge` checks both with main's copy of `code-scope`: a docs-only pass counts only on a docs-only head, and a reused one only if it cites the manager's whole-table pass on the same PR with the same key. The counted run is now split (owner decision `01a2d668b67e`): `ON=origin/main scripts/verify-seq --split [-j 2] <pr claim>:<sha> ...` runs `scripts/test-table --split`, the full table in a boat box plus the macOS-only tests on this Mac, as one evidence per PR. `merge` and `--reuse` count it as the whole table (see Remote test runs below). A manual run is an observer, as before (decision `8b2454e4cf81`) |
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

## Remote test runs (boat.dev)

Owner decision `25c1e3543e76` (task `489bfeb6b5af`, reference `ea41264b3c38`): the test table can run in a boat.dev box, a fresh Linux x86_64 machine, instead of on the owner's machine. It's a trial.

**When to use it.** Run `scripts/test-table --remote` instead of a local `cargo test` (or a local `scripts/test-table`) whenever the machine is loaded: several builders or reviewers testing at once, or a long build already running. It's also the only place the Linux tests actually run (gotcha `2c9aebd6d370`). On an idle machine the local table is faster. Record a remote run like any test run: `"$main/scripts/bk-evidence" <pr> --as <you> -- scripts/test-table --remote`. But `--remote` alone is never the counted merge-gate run (manager decision `52c4eb7d2425`): on Linux it skips the macOS-only tests. The counted run is `--split` (below), which runs those here.

**The trial's limits.** At most 2 boxes at a time across the whole team, 75 box starts a day and 25 hours of compute; `boat limits` shows what's left. So keep **at most one box of your own running**, and delete it the moment you're done with it (the owner's preference): never leave one idle. `boat run` does that for you. A full table on a default box (4 vCPU, 8 GB) takes 4½ to 5½ minutes from a cold build and costs about a third of a cent: 4m 28s and $0.0027 on PR #68 at `4e1c0a6` (evidence `8492a0dcfa0d`), 5m 19s and $0.0032 on its earlier head `8e67dc7`.

### `scripts/test-table --remote`

It tests HEAD, so commit first: it refuses a dirty tree, and needs `origin/main` (fetch it). It puts HEAD and `origin/main` in a git bundle, the only thing pushed, and runs one `boat run --no-env --push <bundle dir> --type default --timeout 2400`. In the box, it installs what the box lacks (below), clones the bundle with HEAD checked out and `origin/main` beside it, and runs the same, unchanged `scripts/test-table`, Ground guards included: the bundle is 5 MB and takes under a second, so the guards check the same commits as locally. The guards use the debug `bracken` the table just built (`BRACKEN_BIN`). It prints the box's output, then `test-table: box: exit N · time · type · cost`, and exits with the box's exit code (130 if interrupted).

The box is deleted on every way out. `boat run` deletes it when the command ends, fails or times out, and when it's interrupted (Ctrl-C). Once boat has printed the box's id (`bx_… ready`), `--remote` also checks `boat list --all` as it exits, for any reason, and deletes the box by id if it's listed in any state, or if boat can't list boxes at all; plain `boat list` shows only running boxes, and one left stopped, pending, stopping or in error keeps its data (gotcha `93bae3ba4156`). If the box is still listed after that, it says so with the command to delete it. A Ctrl-C before boat prints the id leaves the deletion to `boat run` alone: check `boat list --all`. `crates/bracken/tests/scripts.rs` tests this with a stub `boat` that lists like the real one: a failed run that leaves its box stopped and an interrupted one each leave no box.

### `scripts/test-table --split`: the manager's counted run

Owner decision `01a2d668b67e`: the full table in one box, exactly as `--remote` runs it (the same bundle of HEAD and `origin/main`, `--no-env`, a default box, deleted on every way out), and at the same time, on this Mac, only the macOS-only tests: `cargo test --workspace -- macos_only::`. It passes only if both pass, and it ends with the last lines of both parts, so the evidence's details hold both. With `bk-evidence --on`, HEAD is the merge, so both parts test the merged tree, and the method is `scripts/test-table --split @ <head> on <main> tree <key>`. `scripts/merge` counts that as the whole table, as `bk-evidence --reuse` does; `--remote` alone counts for neither. A docs-only head runs the local docs-only table, with no box.

The manager runs it through `scripts/verify-seq`: `ON=origin/main scripts/verify-seq --split [-j 2] <pr claim>:<sha> ...`. `-j 2` runs two PRs at once, a box each (the trial's 2 boxes), with one evidence per PR. Without `--split`, `-j` is refused, because the local runs would compete.

**Which tests are macOS-only comes from the code.** Every test that needs macOS goes in a `mod macos_only`: today the sandbox tests in `crates/experiment/tests/agent.rs` (ignored on Linux) and the ACL tests in `crates/demo` (compiled only on macOS). The box also lists the tests Linux has and those it ignores, and the Mac lists its own. A test that runs on the Mac but is missing or ignored on Linux, and isn't in a `macos_only` module, fails the run by name, so no new test can be skipped on both sides.

Stopping it (a Ctrl-C, or a failure on the Mac side) stops both parts. Each runs as a job with job control, in its own process group, with SIGINT not ignored as a plain background job's is (finding `87cfdf4680c7`). Each part gets SIGINT, which `boat run` takes as an interrupt and deletes the box; after 15 s, SIGTERM; then SIGKILL. Then `test-table` deletes the box by id if `boat list --all` still shows it. The tests on the Mac run under `timeout`, in a process group of its own that it signals whole at the limit, so a test whose child hangs ends with everything it started (finding `c7d17b82e315`); their part passes a SIGINT or SIGTERM on to `timeout` as SIGTERM (a background `timeout` ignores SIGINT), which signals that group, and if the tests were stopped or hit their limit, `test-table` sends the group SIGKILL as it exits, in case a test that ignores SIGTERM outlived them; never after they ended by themselves. This Mac's test lists run the same way, each bounded at 300 s. Under `verify-seq`, every run is such a job too: a Ctrl-C to `verify-seq` is passed to each running run as SIGINT and a SIGTERM as SIGTERM, so a `verify-seq` started with SIGINT ignored still stops on SIGTERM (note `c1468539ea94`). Both scripts find their jobs with `jobs -p`, which holds a job from its fork, so a signal before its pid is saved still reaches it. `bk-evidence` records nothing for a run it interrupts, and `verify-seq` exits 130. The test-list commands in the box run under `pipefail` and end with a marker line; a failed or cut-short list fails the run, so it can't hide a test that is ignored only on Linux.

### Using boat by hand

| Task | Command |
|---|---|
| Run a command in a fresh box, then delete it | `boat run --no-env --push <dir> [--type small\|default] [--timeout <s>] -- <cmd>`. `--push` copies the git-tracked and unignored files under `<dir>` to `~/<dir name>` in the box, and the command runs there. It prints `bx_<id> ready`, the command's output, and `exit N · time · type · cost · bx_<id> deleted` |
| See your boxes | `boat list --all` (JSON). Plain `boat list` shows only running ones. It should be empty whenever you aren't running something |
| Delete a box | `boat delete --yes <bx_id>`. Without a terminal, `--yes` is required |
| What's left of the trial | `boat limits` |

The rules for every box:

- **Always `--no-env`**, and never `-e`/`--env`: no secret, token or credential goes into a box, forge credentials included. A box gets no `~/.bracken-team`, no `.bracken/identities/` and nothing gitignored. Push a bundle or a clean directory, not a checkout with untracked files in it.
- **Run it in the foreground**, and delete the box yourself if anything stops `boat run` from doing so (check `boat list --all`).
- **A plain `--push .` has no `.git`.** Anything that needs git history (the Ground guards, git mode, tests that expect to be in a checkout) needs the bundle route that `--remote` uses: `git bundle create <dir>/tree.bundle HEAD refs/remotes/origin/main`, then in the box `git init -q tree && cd tree && git fetch -q ../tree.bundle +HEAD:refs/heads/tested +refs/remotes/origin/main:refs/remotes/origin/main && git checkout -q tested`.

### What a box has, and what tests need installed

A box is Ubuntu 24.04 (x86_64) with Rust 1.97 (cargo, clippy, rustfmt), cc, git 2.43, `timeout`, python3, `sudo` and `apt-get`. The test table also needs:

| Needed by | What | Install in the box |
|---|---|---|
| The Ground guards, tests run in a checkout | `git` and a clone with history | The history comes from the bundle (above) |
| The experiment's git-mode tests | a git newer than Ubuntu's 2.43, which neither traces the programs it starts (`trace: start_command:`) nor fsck-checks a fetched bundle, so the hub there accepted a malformed commit | `sudo add-apt-repository -y ppa:git-core/ppa && sudo apt-get install -y -qq git` (2.56 in October 2026) |
| `scripts/team-server token` and its test | `b3sum` | `apt-get install -y -qq b3sum` (or with `sudo`) |
| The Ground guards | a `bracken` binary | `BRACKEN_BIN="$PWD/target/debug/bracken"`, built by the table's `cargo test` |
| The table's Linux-target clippy | the `x86_64-unknown-linux-gnu` target | Native in the box |

`--remote` installs the PPA's git and `b3sum` in every box, quietly unless that fails: then it prints the failing command, its exit and its output, and stops; anything a future test needs goes in its inner script too, and in this table. On Linux, the experiment's sandboxed tests are skipped with a printed reason (`needs macOS sandbox-exec`): `sandbox-exec` exists only on macOS, and they still run there. The workspace builds dependencies at `opt-level = 2` even in debug (`Cargo.toml`), which made the demo's adversary tests three times faster; without it they alone took 13 minutes in a box.

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
