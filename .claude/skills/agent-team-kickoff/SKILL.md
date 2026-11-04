---
name: "agent-team-kickoff"
description: "Set up this repository for one human owner working with a team of AI coding agents (manager, builders, reviewers) that coordinate through a shared Bracken Ground instead of chat threads and state files, then start the manager. Use when starting or adopting an agent team here."
---

# Agent team kickoff (Bracken edition)

This skill sets up a project so that one human **owner** and a team of AI coding agents can build, test and ship software together. Agents work in parallel without stepping on each other, the owner decides only what matters, and nothing reaches users untested.

This edition changes **where the team's knowledge lives**. Agents don't keep it in thread messages, a manager's private state file and scattered PR comments. They contribute it to one shared **Ground**: a Bracken contribution graph that every agent in every worktree reads and writes. Tasks, decisions, findings, test runs and review verdicts become signed, immutable contributions. A merge gate becomes a **validation policy** over the evidence. A merge is ready when the required independent validators have passed the current head, and the author's own word never counts.

Git and GitHub stay the boundary for **code**, because Bracken can't yet hold a source tree (Weave doesn't exist yet). Each new Bracken capability should retire a piece of this process; see "Retiring the scaffolding".

Run it once per repository. It has three phases:

1. **Interview**: collect the project facts.
2. **Scaffold**: create the Ground and identities; write the docs and CI guards.
3. **Start the manager**: the agent running this skill becomes the manager, or hands off to one.

The process itself is in [process.md](process.md) and stays the same for every project. Only the **project profile** changes. Section numbers below ("section 4.1") refer to `process.md`.

---

## Phase 1: Interview

Read the repo before asking anything: README, `Cargo.toml` / package manifests, CI workflows, deploy configs, `docs/`, existing `AGENTS.md` / `CLAUDE.md`. Fill in what you can, then ask the owner only for what is still missing. Batch questions, at most four at a time, with your best guess offered as the default.

Build the **project profile**:

| Field | What to find out | Example |
|---|---|---|
| Vision | What the project is ultimately for: the vision document (`<vision doc>`, e.g. `docs/vision.md`; write one with the owner if there isn't one) and a one- or two-sentence summary in the owner's terms (`<vision>`). Together they're the first half of the goal (process section 1). Read the README and existing docs first, and offer a draft. | "Build a decentralized knowledge network, starting with an agent-native development tool" |
| Stack and packages | Each package, and its test / typecheck / lint commands | workspace: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` |
| Change classes | Categories that decide the release path | docs, cli, library, **protocol**, infra |
| Risky classes | Classes that need an independent review | protocol (encoding, ids, signatures, validity rules), storage format, crypto, release scripts |
| Release stages | From internal to users | local → tagged pre-release → crates.io |
| What deploys on merge | Workflows triggered by `push` to `main`, and their path filters | none yet |
| Gated actions | Actions that need the owner's go, beyond the defaults | publishing crates, creating the public repo, licence changes |
| Shared resources | Things only one agent may use at a time | **the pinned `bracken` binary**, Ground checkpoint commits |
| Things that break production | Footguns | "a change to encoding or validity rules can make every existing Ground unreadable" |
| Remote | Where PRs and CI live | GitHub `owner/repo` (the `scripts/merge` template speaks GitHub's API through `gh`), or `TODO(owner)`: no remote yet |
| CI role | Observer on every PR (default), manual, or gate | observer: runs on every PR, but merges are gated by the Ground (process section 10). Manual, when the runner's compute is too slow or a public repo's Actions can't be trusted with secrets: the workflow runs only on `workflow_dispatch`, and the manager's counted test-table run on the exact head is the CI |
| Agent harness | How threads, child tasks, messages and schedules work here (see Tool mapping) | T3 Code + Claude Code |
| Models | Default model, a different-model reviewer, a strong security model, and which quota is largest | default A; reviewer B; security A at high effort |
| Monitoring | Error monitor, crash reporting, where logs live | none yet |

If the owner has no answer for a field, write a sensible default, mark it `TODO(owner)` in the docs and list it in the hand-off.

**Without a remote**, there are no PRs or CI to gate on. Builders still work on branches in worktrees. The manager runs the test table itself, and merges locally with `git merge --squash` after the Ground's merge gate passes. Note this in `docs/development.md` and switch to PRs as soon as a remote exists.

### Tool mapping

The process uses six abstract operations. Fill this table for the project's harness and write it into `docs/development.md`. Discover the actual tool names available in the session; don't assume.

| Operation | Meaning | T3 Code | Claude Code alone (typical) |
|---|---|---|---|
| **launch-builder** | New long-lived thread in its own git worktree and branch | `t3_thread_launch` with `workspaceStrategy: {type: "worktree", baseRef, branch, startFromOrigin}` | a subagent or new session with worktree isolation |
| **delegate** | Short child task whose answer the manager needs (review, search) | `delegate_task` | a subagent call |
| **send** | Wake another thread | `t3_thread_send` | the harness's agent messaging tool, or PR comments |
| **read-thread** | Read another thread | `t3_thread_read` | read the agent's result / transcript |
| **watch-pr** | Be woken on CI, comments, conflicts | `watch_pull_request` | `gh pr checks --watch`, or a scheduled check |
| **schedule** | Recurring read-only run | `schedule_task` | the platform's scheduled tasks |

The Ground has no notifications, so **send** is still how one agent wakes another. But messages carry **contribution ids**, not content: "Report: `bk show 3fa9c1e2`", not three paragraphs. Whatever someone might need later belongs in the Ground, not in the message.

If an operation has no equivalent, note the fallback (usually: the manager polls with `bk log --json` and `gh`).

---

## Phase 2: Scaffold

Work on a branch, `agent-team-setup`, **in the main checkout**: the Ground must be created there, and its first files ship in this PR. Commit the Ground's files only with `scripts/ground-checkpoint --setup`, never with `git add`: it keeps the allowlist and key checks on the setup branch. Commit everything else with `.bracken` excluded (`git add -A -- . ':!.bracken'`). Open one PR, or squash locally if there's no remote. It deploys nothing. The manager authors it, so it takes the manager-authored gate: independent reviewer and security reviewer, with the reviewer re-running the checks (process section 4.2). Show the owner the summary and `AGENTS.md` before merging, since it sets the rules.

**Don't switch the main checkout back to `main` until the PR has merged.** The Ground's files are tracked on this branch and not yet on `main`, so switching would delete them from disk. After the merge: `git fetch && git checkout -B main origin/main`. That leaves the newer, untracked contributions in place. Then run the first `scripts/ground-checkpoint`.

### 2.1 The Ground

Do these steps from the **main checkout**, not a worktree: it holds the live Ground. From now on, never switch the main checkout to a commit that lacks the committed `.bracken/` files; git would delete them from disk. All later branch work happens in worktrees.

1. **Pin the binary into `.bracken-bin/`.** In the Bracken repo itself: `cargo install --locked --path crates/bracken --root .bracken-bin`. Elsewhere: install a released version the same way (`cargo install --locked bracken --version <v> --root .bracken-bin`, or from its source) and record the version. `scripts/bk` runs only this binary, never one on PATH. Agents never coordinate through a binary built from their own branch: a builder breaking the CLI must not break the team. Upgrading the pinned binary is a shared-resource action (section 6).
2. **Install the scripts.** Copy `templates/bk`, `templates/bk-evidence`, `templates/merge`, `templates/ground-guards`, `templates/ground-checkpoint`, `templates/team-server`, `templates/test-table`, `templates/code-scope`, `templates/verify-seq`, `templates/cleanup` and `templates/owner-approve` from this skill to `scripts/`, and `chmod +x` them. `bk` finds the main checkout from its own location, so it works from any worktree, and points `BRACKEN_DIR` at that checkout's `.bracken/`. `bk-evidence` runs a check and records its real exit code and output as evidence (process section 5). `merge` is the manager's merge step (process section 5, step 8), and runs `ground-guards`, the checks on a head's committed Ground (process section 10); it expects `origin` on GitHub over https, `gh` logged in for that host (it uses gh's own credential store, never a token from the environment) and `jq`. `ground-checkpoint` commits the Ground's files safely (process section 9.3). `team-server` runs an optional sync server for the team on this machine (process section 4.3); it needs `b3sum`. `test-table` runs the whole test table as one command, which is what `merge` counts when `main` has moved ahead in code (process section 5, step 5); replace its commands with the project's table. `code-scope` says which changes are docs only (`*.md` outside `crates/`; adjust to where the project's code reads `.md` files): `test-table` then runs only fmt and the guards, and `merge` checks that label. `verify-seq` runs the manager's counted runs one head at a time, and with `ON=origin/main` reuses an earlier pass of the same PR whose merged tree differs only in `.bracken/` and docs; with `--batch` it tests several heads merged together in one run, which `merge --batch` then merges all or nothing; with `--speculative` it tests every prefix of a merge queue at once, so each head merges with the plain `merge` as main reaches the prefix before it. It warns below 50 GB free and refuses below 20 GB. `cleanup` frees disk: it removes clean, unused T3 child worktrees and `$TMPDIR` checkouts, removes `target/` in builders' worktrees whose branch is merged, and clears `$TMPDIR/bracken-scratch`, all only after 2 hours unused; a dry run unless given `--apply`, which `merge` runs after each merge (process section 6, Worktrees). `owner-approve` is how the owner signs an `approval` as `owner`, in a terminal; with `--owner-approval`, `merge` requires one for the PR (process section 3). **Agents always run `scripts/bk`, never `bracken` directly.**
3. **Commit the Ground, never the keys.** `.gitignore` must exclude `.bracken/identities/` (private keys), `.bracken/objects/**/.tmp-*` (writes in progress), `.bracken-bin/` and `.bracken-team`. Everything else under `.bracken/` is committed: `ground.json`, `objects/` and the public agent directory `agents/`. Contribution files are immutable and named by their hash, so git merges them without conflicts, and git is the Ground's durable record (section 9.3).
4. **Create the Ground and identities:**

   ```sh
   scripts/bk init --name <project>        # run from the main checkout
   for who in owner manager reviewer security triage; do scripts/bk id new "$who"; done
   ```

   Builders get an identity when they're launched: `builder-<branch>`.
5. **Seed it.** Record the project profile's decisions as claims tagged `decision` (section 4.1). Contribute the vision document as an artifact named by its path (`scripts/bk artifact <vision doc> --name <vision doc> --media-type text/markdown --as manager`). Then record the goal as a claim tagged `goal`, with process section 1's wording filled in, and `relate <goal> depends_on <vision artifact>`.

### 2.2 Files

Create or update these files. If a file already exists, merge into it; don't overwrite the owner's content.

| File | Contents |
|---|---|
| `AGENTS.md` | Short; every agent reads it, and agents follow it closely, so every line costs tokens on every task. Include only what an agent can't learn from the code or README: no repository overview or directory tour. In this order: **Things that break production**; conventions; a **test command table** per package; a releases summary; a **The Ground** section (always `scripts/bk`; read `scripts/bk log --tag decision --current` at task start; the contribution conventions table from section 4.1); a **Shared knowledge** section (the rules from section 9.1); a **Working with other agents** section linking `process.md` and `docs/development.md`. If Claude Code is the harness, add a `CLAUDE.md` containing just `@AGENTS.md`: Claude Code reads `CLAUDE.md`, not `AGENTS.md`. Draft it, then have the owner cut and correct it before merging: human-written context files help, generated ones mostly add cost. |
| `docs/builder.md` | One-page role card for builders: the builder's role, who to talk to, the brief template's fields, what to contribute to the Ground and when, the evidence rules and the final report format (section 6). Builder briefs link this, not the whole process doc. |
| `docs/development.md` | The project profile, the tool mapping, this project's value for every `<placeholder>` in [process.md](process.md), and the PROJECT paragraph its review briefs open with (process sections 5.1 and 5.2). It links to `process.md` and never copies it, so there's one process to keep current. |
| `.claude/skills/agent-team-kickoff/` | A copy of this skill, if the repo doesn't have one, so the process is versioned with the code. Later process changes go into that copy (process section 8). |
| `docs/releases.md` | Release stages, the release path per change class, the owner's-go list, test report format, rollback steps. For protocol changes: how an existing Ground is migrated or why it doesn't need to be. |
| `docs/README.md` | Index of docs. Stale docs move to `docs/archive/`. |
| `scripts/bk`, `scripts/bk-evidence`, `scripts/merge`, `scripts/ground-guards`, `scripts/ground-checkpoint`, `scripts/team-server`, `scripts/test-table`, `scripts/code-scope`, `scripts/verify-seq`, `scripts/cleanup`, `scripts/owner-approve` | From `templates/` (`test-table` edited to the project's table). |
| `.gitattributes` | `.bracken/** -text`, so git never converts the Ground's line endings. At the repo root: the Ground's allowlist rejects `.bracken/.gitattributes`. |
| CI workflow | The guards in process section 10 that the project lacks, as an observer workflow; the Ground guards by calling `scripts/ground-guards`, not inline. Propose deploy-related changes; don't make them without the owner's go. |

Also, if the harness supports it, add a worktree setup script that warms the build (`cargo fetch && cargo build --workspace`), so builders can test straight away.

---

## Phase 3: Start the manager

1. Decide who the manager is: this thread, or a new long-lived thread. Only one manager per workstream.
2. The manager contributes its **manager claim** and its first **plan** (section 9.4), and adds the one-line pointer to its harness memory.
3. If monitoring exists, set up the daily triage schedule (ask first: it adds a schedule) pointing at the manager's thread ID.
4. Tell the owner, briefly: what was set up, the `TODO(owner)` items, the open gated decisions, and how to give the manager work.

---

## Kickoff checklist

Tick these off in the setup PR description:

- [ ] Project profile complete; `TODO(owner)` items listed
- [ ] Tool mapping filled for the harness
- [ ] Pinned `bracken` binary installed; `scripts/bk`, `scripts/bk-evidence`, `scripts/merge`, `scripts/ground-guards`, `scripts/ground-checkpoint`, `scripts/team-server` and `scripts/test-table` installed; `.gitattributes` has `.bracken/** -text`; `.bracken/identities/`, `.bracken-bin/` and `.bracken-team` ignored
- [ ] Ground created; identities `owner`, `manager`, `reviewer`, `security`, `triage` created
- [ ] Goal and profile decisions contributed as claims
- [ ] After the merge: main checkout back on `main` (`git checkout -B main origin/main`), first `scripts/ground-checkpoint` committed and pushed, and `git ls-files .bracken/identities` prints nothing
- [ ] `AGENTS.md` (breaks-production list, conventions, test table, releases summary, The Ground, Shared knowledge, Working with other agents)
- [ ] `docs/development.md`, `docs/releases.md`, `docs/README.md`, `docs/builder.md`
- [ ] `AGENTS.md` trimmed and approved by the owner
- [ ] Builder warm-build script on worktree creation
- [ ] Default, reviewer and security models chosen and authenticated
- [ ] CI observer workflow green on the setup PR (or, for a manual CI role, one manual run); `main` protected against force-push and deletion, tested on a scratch branch (owner's go)
- [ ] Monitoring and daily triage present or proposed
- [ ] Manager started: manager claim and first plan contributed, harness-memory pointer added, owner briefed
