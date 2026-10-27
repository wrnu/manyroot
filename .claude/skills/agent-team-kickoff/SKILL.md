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
2. **Scaffold**: create the Ground and identities; write the docs, memory, hook and CI guards.
3. **Start the manager**: the agent running this skill becomes the manager, or hands off to one.

The process itself (section "The process" below) stays the same for every project. Only the **project profile** changes.

---

## Phase 1: Interview

Read the repo before asking anything: README, `Cargo.toml` / package manifests, CI workflows, deploy configs, `docs/`, existing `AGENTS.md` / `CLAUDE.md`. Fill in what you can, then ask the owner only for what is still missing. Batch questions, at most four at a time, with your best guess offered as the default.

Build the **project profile**:

| Field | What to find out | Example |
|---|---|---|
| Goal | One sentence. Default below. | Ship improvements quickly and safely |
| Stack and packages | Each package, and its test / typecheck / lint commands | workspace: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` |
| Change classes | Categories that decide the release path | docs, cli, library, **protocol**, infra |
| Risky classes | Classes that need an independent review | protocol (encoding, ids, signatures, validity rules), storage format, crypto, release scripts |
| Release stages | From internal to users | local → tagged pre-release → crates.io |
| What deploys on merge | Workflows triggered by `push` to `main`, and their path filters | none yet |
| Gated actions | Actions that need the owner's go, beyond the defaults | publishing crates, creating the public repo, licence changes |
| Shared resources | Things only one agent may use at a time | **the pinned `bracken` binary**, Ground checkpoint commits |
| Things that break production | Footguns | "a change to encoding or validity rules can make every existing Ground unreadable" |
| Remote | Where PRs and CI live | GitHub `owner/repo`, or `TODO(owner)`: no remote yet |
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

Work on a branch (`agent-team-setup`) and open one PR, or squash locally if there's no remote. This change deploys nothing, so the manager may merge it once checks pass, but show the owner the summary first since it sets the rules.

### 2.1 The Ground

Do these steps from the **main checkout**, not a worktree:

1. **Pin the binary.** In the Bracken repo itself, run `cargo install --locked --path crates/bracken --root .bracken-bin`. Elsewhere, install a released `bracken` and record its version. Agents never coordinate through a binary built from their own branch: a builder breaking the CLI must not break the team. Upgrading the pinned binary is a shared-resource action (section 6).
2. **Install the wrapper.** Copy `templates/bk` from this skill to `scripts/bk` and `chmod +x` it. It resolves the main checkout from any worktree and points `BRACKEN_DIR` at its `.bracken/`. **Agents always run `scripts/bk`, never `bracken` directly.**
3. **Commit the Ground, never the keys.** `.gitignore` must exclude `.bracken/identities/` (private keys), `.bracken/objects/**/.tmp-*` (writes in progress) and `.bracken-bin/`. Everything else under `.bracken/` is committed: `ground.json`, `objects/` and the public agent directory `agents/`. Contribution files are immutable and named by their hash, so git merges them without conflicts, and git is the Ground's durable record (section 9.3).
4. **Create the Ground and identities:**

   ```sh
   scripts/bk init --name <project>        # run from the main checkout
   for who in owner manager reviewer security triage; do scripts/bk id new "$who"; done
   ```

   Builders get an identity when they're launched: `builder-<branch>`.
5. **Seed it.** Record the project profile's decisions as claims tagged `decision` (section 4.1). Record the goal as a claim tagged `goal`.

### 2.2 Files

Create or update these files. If a file already exists, merge into it; don't overwrite the owner's content.

| File | Contents |
|---|---|
| `AGENTS.md` | Short; every agent reads it, and agents follow it closely, so every line costs tokens on every task. Include only what an agent can't learn from the code or README: no repository overview or directory tour. In this order: **Things that break production**; conventions; a **test command table** per package; a releases summary; a **The Ground** section (always `scripts/bk`; read `scripts/bk log --tag decision` at task start; the contribution conventions table from section 4.1); a **Memory** section (copy "Shared repo memory" rules below); a **Working with other agents** section linking `docs/development.md`. If the project uses `CLAUDE.md`, make it a one-line pointer to `AGENTS.md`. Draft it, then have the owner cut and correct it before merging: human-written context files help, generated ones mostly add cost. |
| `docs/builder.md` | One-page role card for builders: the builder's role, who to talk to, the brief template's fields, what to contribute to the Ground and when, the evidence rules and the final report format (section 6). Builder briefs link this, not the whole process doc. |
| `docs/development.md` | "The process" below, adapted with the project profile and the tool mapping. |
| `docs/releases.md` | Release stages, the release path per change class, the owner's-go list, test report format, rollback steps. For protocol changes: how an existing Ground is migrated or why it doesn't need to be. |
| `docs/README.md` | Index of docs. Stale docs move to `docs/archive/`. |
| `memory/INDEX.md` | Headings: Preferences, Gotchas, Projects, References. (Decisions live in the Ground.) |
| `memory/archive/INDEX.md` | Empty index for archived entries. |
| `docs/process-metrics.md` | Weekly metrics log (see "Measuring"), starting with a header row and an empty baseline week. |
| `scripts/bk` | The wrapper, from `templates/bk`. |
| `.claude/settings.json` + `.claude/hooks/pre-compact-memory.sh` | The PreCompact hook (below), only if Claude Code is the harness. Needs `jq` and the `claude` CLI. |
| CI workflow changes | The CI guards below that the project lacks. Propose deploy-related changes; don't make them without the owner's go. |

Also, if the harness supports it, add a worktree setup script that warms the build (`cargo fetch && cargo build --workspace`), so builders can test straight away.

---

## Phase 3: Start the manager

1. Decide who the manager is: this thread, or a new long-lived thread. Only one manager per workstream.
2. The manager creates its **state file** (section 9.4) and records its thread id in the Ground: `scripts/bk claim "Manager thread: <id>, worktree: <path>" --tag manager --as manager`.
3. If monitoring exists, set up the daily triage schedule (ask first: it adds a schedule) pointing at the manager's thread ID.
4. Tell the owner, briefly: what was set up, the `TODO(owner)` items, the open gated decisions, and how to give the manager work.

---

# The process

Copy this into `docs/development.md`, replacing every `<placeholder>` from the project profile.

## 1. Goal

> Ship improvements to users quickly and safely: nothing reaches users untested, the owner decides only what matters, and agents work in parallel without losing track or stepping on each other.

Every process change is judged against the goal. The manager may propose new wording; only the owner changes it.

## 2. Roles

| Role | Identity | What it does |
|---|---|---|
| **Owner** | `owner` | Decides what gets built; approves gated steps. Talks **only to the manager**. |
| **Manager** | `manager` | One long-lived thread per workstream. Plans, splits work into task claims, launches builders, verifies, reviews and merges, owns shared resources, improves the process. **The only agent that talks to the owner.** |
| **Builder** | `builder-<branch>` | One thread per task, own worktree, own branch, one PR. Builds and tests, contributes what it learns, reports to the manager. Never messages the owner. |
| **Reviewer** | `reviewer` | Short-lived child on a **different model** from the PR's author. Reads code; writes only findings and evidence to the Ground. |
| **Security reviewer** | `security` | Short-lived child on a strong model. Reviews **every** proposal and **every** PR; writes only findings and evidence. |
| **Scheduled tasks** | `triage` | Recurring runs that read the code and write only to the Ground. Report to the manager, never the owner. |

Each identity signs only its own role's contributions. **v0 caveat:** all keys live in the shared `.bracken/identities/`, so the signature records who claimed to act, not proof of it. Never use another role's identity. The manager checks that evidence came from the agent that was briefed to produce it.

Small work needs no manager. As soon as a second thread works on the same repo at the same time, one of them is the manager.

## 3. What needs the owner's approval

Agents act alone only on what can't reach users, can't cost money and doesn't touch outside systems.

**Without asking:** branch, code, test, open PRs; contribute to the Ground; run local builds; read logs and CI; squash-merge a change whose merge gate passes **if the merge deploys nothing** (no changed path matches a deploy or publish workflow's `push` path filter); small process improvements.

**Ask first:** merging anything that publishes or deploys (batch these so the owner approves several at once); releases and publishing; billed builds, manual deploys, access or permission changes; `<project-specific gated actions>`; **upgrading the pinned `bracken` binary when the change touches the protocol or storage format**; major process changes, or ones that touch an outside system, send data somewhere new, cost money, add tools, services or schedules, or change how agents work with the owner.

**An approval covers the step it was given for, not later ones.** "Merge it" doesn't mean "and take it live". The manager records each approval as it's given (section 4.1), quoting the owner, and links it to exactly what it approves.

## 4. The Ground

### 4.1 Contribution conventions

Every agent follows these, so that `scripts/bk log --tag <tag>` answers the team's recurring questions.

| What | How | Who |
|---|---|---|
| Goal | claim, tag `goal` | owner or manager |
| Decision | claim, tag `decision`: "Owner, 2026-10-27: <decision>. Why: <reason>" | manager (quoting the owner) or owner |
| Owner approval | claim, tag `approval`: quote, date, scope; `relate <approval> supports <thing approved>` | manager or owner |
| Owner ask | claim, tag `owner-ask`; retract it once it's answered and the answer is recorded | manager |
| Task | claim, tag `task`: the full builder brief (section 6) | manager |
| Design note, hypothesis, discovery | claim, tag `note`; relate it to the task or the code it concerns | anyone |
| PR head | claim, tag `pr`: "PR #<n> <branch> @ <full sha>: <title>"; `relate <pr> implements <task>` | builder |
| New commit on a PR | new `pr` claim, then `relate <new> supersedes <old>`. Evidence on the old head no longer counts, so every head is verified on its own | builder |
| Test run | evidence on the `pr` claim: `--method "<command> @ <sha>"`, `--details "<relevant output>"` | builder (recorded, never counted), manager (counted) |
| Review verdict | evidence on the `pr` claim: `pass` or `fail`, `--method "independent review (<model>) @ <sha>"` | reviewer, security |
| Review finding | claim, tag `finding`, with file:line, severity, the requirement broken and a counterexample; `relate <finding> contradicts <pr>` | reviewer, security |
| Finding triage | evidence on the `finding`: `pass` = confirmed (say how), `fail` = rejected (say why) | manager |
| Process proposal | claim, tag `process` | anyone; the manager decides and records a `decision` |
| Withdrawal | `retract <own id> --reason "..."` | the author only |

Rules:

- **Contribute as you learn, not at the end.** A builder that finds out something another agent would need (a gotcha, an interface decision, a dead end) contributes a `note` right away and relates it. That's what lets parallel work build on itself.
- **Never act on a claim alone.** Use `scripts/bk show <id>` to check its status, evidence and referrers. A `note` is a lead, not a fact.
- **Ids in messages, content in the Ground.** Quote the short id (12 hex characters) everywhere: messages, PR descriptions, commits.
- **No secrets or personal data in the Ground.** Contributions are immutable: a retracted contribution still exists, and it's in git history for good.
- **Disagreement is data.** If you think a decision, a finding or a test is wrong, contribute a claim that `contradicts` it with your reasoning. Don't argue in messages, and don't quietly work around it.

### 4.2 Merge gate

Merge readiness is the acceptance status of the **current `pr` claim** under the class's policy. Builder self-evidence never counts.

| Class | Gate |
|---|---|
| Low risk | `scripts/bk show <pr> --validator manager --validator security --min-passes 2` reports `accepted` |
| `<risky classes>` | `scripts/bk show <pr> --validator manager --validator reviewer --validator security --min-passes 3` reports `accepted` |

Any counted `fail` makes it `rejected`. The fix is a new commit and a new `pr` claim that supersedes the old one, and the gate starts again on the new head.

**Findings.** `scripts/bk conflicts` lists every finding that contradicts a current head, whether triaged or not. Rejected ones stay listed until the head changes; that's expected, not a blocker. Before merging, every finding against the head must have manager evidence (see the referrers in `scripts/bk show <pr>`):

- A **confirmed** finding (pass) blocks the merge. The builder fixes it in a new head.
- A **rejected** finding (fail) stays on record and doesn't block.

A new head supersedes the old one, and that clears *all* of the old head's contradictions, fixed or not. So the next review round's brief lists every confirmed finding, and the reviewer must check each one against the new head. Anything still unfixed gets re-filed against the new head.

## 5. A change, step by step

1. **Task:** the manager contributes a `task` claim holding the brief, then launches the builder with that id. One task, one worktree, one branch, one PR.
2. **Branch:** short kebab-case, no prefix.
3. **Build and test locally:** every command in the `AGENTS.md` test table for the touched packages, plus the brief's definition-of-done checks. **Never delete, skip or weaken a test to get a green run**; if a test seems wrong, contribute a claim that `contradicts` it and let the manager decide.
4. **PR head:** commit, then contribute a `pr` claim (or a superseding one) and evidence for each test run. The PR description has the change **class**, what wasn't tested, the release steps, and the task and `pr` claim ids.
5. **CI** passes. The manager re-runs the done-when checks on the head (or confirms CI ran them) and records counted evidence as `manager`.
6. **Reviews** (independent for risky classes; security for all) record findings and verdicts in the Ground. **Finish every review round before manual testing**, so the tested build is the one that merges.
7. **Manual test**, if the change can be seen or felt: test the real thing, and record the result as manager evidence with a short report in `--details`.
8. **Merge:** once the gate passes (section 4.2), merge `main` into the branch, then squash-merge. If the merge changed the head, the gate applies to the new head. If it deploys, wait for the owner's go.
9. **Each release stage** toward users needs the owner's go, recorded as an `approval`.
10. **Watch it land.** Check triage and error reports for anything new from the release, and close out the task with a `note` saying what shipped.

### 5.1 Independent review (risky classes)

Risky classes for this project: `<risky classes>`. Low-risk changes merge after the manager's verification and security review.

- **delegate** a child on the review model; if that model wrote the PR, use another. It runs as `reviewer` and writes only `finding` claims and evidence.
- Brief it with the PR number, the `pr` claim id, the class, what must not break and the diff. **Don't give it your conclusions**, and don't point it at your own notes about the PR.
- Each round is a new child. Its brief carries the original brief, the earlier finding ids and how each was handled (the manager's evidence on them), and open disagreements.
- Each finding is its own `finding` claim with file:line, a severity, **the requirement it breaks** (from the brief, the PR description or the project's rules) and **a concrete counterexample**, ideally a failing test. It's labelled **confirmed** (shown to fail) or **possible risk**. The round ends with a verdict as evidence on the `pr` claim.
- AI reviewers often reject correct code and invent requirements, and asking for fixes makes this worse. So the manager **triages findings before passing them to the builder**. It rejects (fail evidence, with the reason) findings outside the task's scope or with no requirement behind them. It asks for a counterexample when one is missing. It checks confirmed findings by running the test, then records a pass.
- **At most three rounds.** After that the manager decides on what's still open and records a `decision` saying why.
- The builder answers every confirmed finding: fixed in a new head, deferred (a `task` claim that `depends_on` the finding), or disputed (a claim that `contradicts` it).

### 5.2 Security review (every proposal, every PR)

A child on a strong model, running as `security`, checks:

- **Credentials:** nothing that grants access ships in a client bundle; secrets live server-side in a secret manager. Every proposal says where its credentials live and what a leak would expose.
- **Data flows:** what leaves the device or backend, where it goes, whether users would expect it.
- **Access control and permissions:** data rules, endpoint auth, admin paths, CI service accounts, workflow triggers. PR code must never reach a credential.
- **Input handling, supply chain** (new crates, packages and actions), **logging and telemetry** (no secrets, personal data or user content).
- **Protocol integrity** (Bracken repo): changes to encoding, ids, signatures or validity rules must keep the guarantees in `docs/protocol.md`. Above all: validity depends only on a contribution and its parents, there's exactly one encoding per contribution, and signatures are verified strictly.
- **Agents and the Ground:** secrets never appear in agent output, PRs, comments, memory or **any contribution**, which is immutable and committed to git history. No production user data or secrets go into any brief or task claim, since briefs may reach other model providers.

Its verdict is evidence on the `pr` claim with the head commit in `--method`: `pass` (clear), or `fail` (blocked), with findings as `finding` claims. "Clear with conditions" is a `pass` whose `--details` lists the conditions; the manager checks them before merging. A new commit means a new head and a new review of what changed.

### 5.3 Model budget

Spend the quota you have most of. Default model for the manager, builders and security review: `<default>`. Independent reviewer: `<reviewer>`. Others rarely, with a stated reason. If a provider hits its limit, continue on the default rather than wait. Record the model in each review's `--method`, so the Ground shows which models found what.

## 6. Several agents at once

### Starting builders

The manager creates the builder's identity (`scripts/bk id new builder-<branch>`), contributes the `task` claim, and uses **launch-builder** (base `main`, from origin). For stacked work, base on the parent branch, not from origin. Use **delegate** instead for short child work whose answer the manager needs.

**Brief a builder like a peer who knows nothing yet.** Agents most often fail by not knowing when to stop, drifting from the task and guessing instead of asking. The brief goes in the `task` claim, using this template. The launch message is just: "Role: builder. Read docs/builder.md, then `scripts/bk show <task id>`."

```
Role: builder. Read docs/builder.md first.
Identity: builder-<branch>   Manager thread: <id>
Objective: <one or two sentences: the outcome, not the steps>
Read first: <files, docs, and Ground ids: decisions, notes, earlier tasks>
Class: <change class>   Branch: <name>
Out of scope: <what not to touch or change>
Done when: <checks that can be run: commands that must pass, behaviour to show>
Stop and report if: <limits, e.g. the fix needs files outside scope, more than ~N attempts at the same failure, a test seems wrong, a gated action is needed>
Ask the manager instead of guessing when: <the spec is ambiguous, two reasonable designs exist, an assumption would be hard to undo>
Report: <when, and anything beyond the standard final report>
```

### Communication

- **The Ground is the record.** GitHub holds code, CI and PR mechanics. The PR description links the Ground ids.
- **Builders talk only to the manager,** via **send** to the manager's thread ID, with contribution ids. Questions to the manager are claims tagged `question` that relate to the task; the answer is a claim that `refines` the question. In its own thread a builder writes only "Reported to manager." (the owner may see every thread).
- **Final report:** a claim tagged `report` with `relate <report> implements <task>`, holding: PR URL and head commit; the current `pr` claim id; what changed and why; **evidence ids for each done-when check**; what wasn't tested; any test it thinks is wrong (and the contradicting claim's id); what it needs from the manager; and process suggestions as separate `process` claims. Then **send** the manager the report's id.
- **The manager verifies rather than trusts:** it re-runs the done-when checks on the head commit (or confirms CI ran them) and records its own evidence. It also checks the original objective was met, not only that the code compiles.
- The manager uses **watch-pr** instead of polling where it can.

### Shared resources (manager only; builders ask)

| Resource | Rule |
|---|---|
| Merges to `main` | One at a time, after merging `main` into the branch. Deploying merges need the owner's go. |
| The pinned `bracken` binary | Only the manager upgrades it, and only from `main`, between merges. Before upgrading, commit a Ground checkpoint (9.3) and check that the new binary opens it (`scripts/bk log >/dev/null`). Protocol or storage changes need the owner's go and a migration note in `docs/releases.md`. |
| The Ground itself | Everyone contributes through `scripts/bk`; no one edits or deletes files under `.bracken/`. Only the manager runs `init`, `import`, `join` or `sync`, and only the manager commits `.bracken/` (9.3). Builders never stage it: their worktree's copy is a stale snapshot. |
| Long-lived release branches | Only through PRs merged by the manager. Never make one a PR's head (merging deletes the head). |
| Publishing, deploys, billed builds | The manager, with the owner's go. |

### Avoiding conflicts

- Split work so builders touch different files; otherwise run tasks in sequence or stack them. Note overlaps in the task claims with `depends_on` relations.
- Never rewrite another agent's branch. Merge `main` into your own; force-push only your own, with `--force-with-lease`.
- **`git stash` is shared across worktrees.** Use a WIP commit instead.
- Concurrent contributions never conflict in the Ground. Two builders contributing at the same moment just produce two causal heads. If two agents **supersede the same claim** differently, `scripts/bk conflicts` shows a divergence. That's a real disagreement for the manager to settle, not an error to clear.

## 7. Monitoring and triage

- **Error monitor:** a scheduled job reads production logs and files issues (label `error-monitor`) for new errors.
- **Crash/error reporting:** tag every report with its release; add breadcrumbs for the steps before a problem; report silent failures as non-fatals; **never include user content**.
- **Daily triage** (**schedule**, fixed time, fresh thread, identity `triage`; read-only for code, writes only to the Ground) covers:
  - new error-monitor issues and failed CI on `main`;
  - what shipped since yesterday;
  - `scripts/bk conflicts` (divergences and untriaged findings);
  - `owner-ask` claims still open;
  - `pr` claims pending for more than a day;
  - tasks with no recent contributions.

  It contributes a claim tagged `triage` and sends the manager its id. On a quiet day: one line, no claim.

## 8. Improving the process

When something costs time, gets lost, needs the owner when it shouldn't, or nearly reaches users untested, the manager asks what change would have prevented it.

- Everyone proposes via `process` claims. The manager decides, and records a `decision` that `supports` or `contradicts` the proposal.
- **Small changes** (wording, a missing step, a checklist item, a better brief, a lint rule, a memory entry, a local script): the manager makes them, merges them (they deploy nothing) and mentions them in its next update.
- **Bigger changes:** a short proposal to the owner: problem, change, cost, and which numbers (below) it should move.

### Measuring

Judge the process by results, not by whether steps were followed. Once a week, the manager (or a scheduled run reporting to it) adds one row to `docs/process-metrics.md`:

| Week | PRs merged | Reverts / follow-up fixes | Problems that reached users | Owner asks (gated / other) | Review findings (real / rejected) | Ground conflicts (opened / settled) | Notes |
|---|---|---|---|---|---|---|---|

Get the numbers from the Ground and GitHub, don't estimate:

- **Owner asks:** count `approval` claims (gated) and `owner-ask` claims (other). "Other" should trend down.
- **Review findings:** count `finding` claims whose manager evidence is a pass (real) versus a fail (rejected). A high rejected share means the review prompts need work.
- **Ground conflicts:** divergences and contradictions opened and settled. Persistent ones point at unclear ownership or vague tasks.
- **Merges and reverts:** `gh pr list --state merged --search "merged:>=<date>"`.

Use `scripts/bk log --tag <tag> --json` with `jq`. A bigger process change comes with the current numbers; about two weeks later the manager reports what moved, and proposes reverting it if nothing improved.

These numbers are also **Bracken's own Phase 1 data**: the Ground is the experiment's record of whether semantic contributions help a team.

## 9. Keeping track across compaction

Long threads get compacted and unwritten details are lost. Four layers protect against that. The Ground is the main one: anything contributed survives every compaction, every thread and every agent.

### 9.1 Shared repo memory: `memory/`

Committed and shared by every agent. It holds guidance for **operating** in the repo. **Decisions and project state live in the Ground**, which is easier to query, and records who decided and what each decision relates to. Read `memory/INDEX.md` at the start of a task and open relevant entries. **Verify an entry before acting on it**: if it names a file, function or flag, confirm it still exists.

**Store** only what a future agent would otherwise get wrong or have to ask:
- **preference:** how the owner wants agents to work;
- **gotcha:** a trap that cost time, with the fix;
- **project:** a pointer to ongoing work: the task claim ids and links, with absolute dates;
- **reference:** where something lives outside the repo.

**Don't store:** decisions (contribute them to the Ground); what code, `git log` or docs already say (update those instead); secrets or personal data; anything only the current conversation needs.

**Format:** one fact per file, `memory/<kebab-slug>.md`:

```markdown
---
name: <slug>
description: <one line, used to judge relevance>
type: preference | gotcha | project | reference
created: YYYY-MM-DD
updated: YYYY-MM-DD
source: <owner + date, PR/issue, Ground id, incident>
---

<The fact, with absolute dates. Preferences and gotchas add **Why:** and **How to apply:** lines. Link related entries with [[slug]] and Ground claims by short id.>
```

**Index:** one line per entry under its type heading in `memory/INDEX.md`: `- [slug](slug.md): <hook>`. Update an existing entry rather than adding a near-duplicate.

**Commit** memory through PRs, with related work or alone (`memory: …`).

**Archive, never delete:** `git mv` to `memory/archive/`, add `archived`, `archived-reason` and optional `superseded-by` to the frontmatter, and move the index line to `memory/archive/INDEX.md`. Don't edit archived bodies; agents don't read the archive by default.

**Keep it small:** an entry is a few lines (about 15 at most); the index stays under ~60 lines. **Summarize** when three or more entries cover one topic, a finished project leaves only lessons, or the index passes that limit: write one replacement entry, archive the originals.

### 9.2 PreCompact hook (Claude Code safety net)

Before compaction, a background headless Claude drafts `memory/` changes from the conversation, and lists decisions it found that aren't in the Ground. It never commits and never writes to the Ground. Its output is a proposal: generated context tends to add noise and cost. After a compaction, the manager reviews the changes (`git status memory/`, `git diff memory/`), keeps only entries that pass the "store" test above, and reverts the rest. It contributes any real missing decisions itself.

`.claude/settings.json`:

```json
{
  "hooks": {
    "PreCompact": [
      { "hooks": [ {
        "type": "command",
        "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pre-compact-memory.sh",
        "timeout": 30,
        "statusMessage": "Starting memory update"
      } ] }
    ]
  }
}
```

`.claude/hooks/pre-compact-memory.sh` (`chmod +x`; the log folder is named after the repo automatically):

```bash
#!/usr/bin/env bash
# PreCompact hook: a headless Claude reads the conversation and updates memory/
# by the rules in AGENTS.md (## Memory). Runs in the background; never commits.
set -u
[ -n "${MEMORY_HOOK_RUNNING:-}" ] && exit 0   # don't recurse

input=$(cat)
transcript=$(jq -r '.transcript_path // empty' <<<"$input")
session=$(jq -r '.session_id // "unknown"' <<<"$input")
root=$(git -C "$(jq -r '.cwd // "."' <<<"$input")" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -f "$root/memory/INDEX.md" ] && [ -f "$transcript" ] || exit 0

logdir="${TMPDIR:-/tmp}/$(basename "$root")-memory-hook"
mkdir -p "$logdir"
log="$logdir/$session.log"
excerpt="$logdir/$session.txt"

# User and assistant text only; long messages cut; newest ~150 KB kept
jq -r '
  select((.type == "user" or .type == "assistant") and (.isMeta | not)) |
  (.message.content |
    if type == "string" then .
    else map(select(.type == "text") | .text) | join("\n") end) as $text |
  select(($text | length) > 0 and ($text | startswith("<task-notification>") | not)) |
  "### \(.type)\n\(if ($text | length) > 6000 then $text[0:6000] + "\n[…cut]" else $text end)\n"
' "$transcript" 2>/dev/null | tail -c 150000 >"$excerpt"
[ -s "$excerpt" ] || exit 0

prompt="This conversation is about to be compacted. Save what a future agent should remember into the repo memory.
1. Read the '## Memory' and '## The Ground' sections of AGENTS.md and follow them exactly.
2. Read memory/INDEX.md and relevant entries. Run 'scripts/bk log --tag decision' to see recorded decisions.
3. Read the conversation excerpt at $excerpt (newest at the end).
4. Add or update memory entries for owner preferences, gotchas, project pointers and references. Archive entries shown to be wrong, finished or superseded; summarize where the rules say to. Keep both INDEX.md files in step.
5. Do not write to the Ground. If the conversation contains owner decisions that 'scripts/bk log --tag decision' lacks, list them at the end under 'Unrecorded decisions:' with the owner's words and date.
No secrets, personal data, or anything code, git history or docs already record. Only touch memory/, and never memory/manager-state.*. Don't commit. If nothing is worth saving, change nothing. End with one line per changed file and why."

(
  lock="$(git -C "$root" rev-parse --absolute-git-dir)/memory-hook.lock"
  mkdir "$lock" 2>/dev/null || { echo "$(date) another run active; skipped" >>"$log"; exit 0; }
  trap 'rmdir "$lock"' EXIT
  cd "$root" || exit 0
  echo "$(date) start ($transcript)" >>"$log"
  MEMORY_HOOK_RUNNING=1 claude -p "$prompt" \
    --model sonnet \
    --allowedTools "Read" "Glob" "Grep" "Edit(memory/**)" "Bash(git mv memory/*)" "Bash(scripts/bk log *)" \
    >>"$log" 2>&1
  echo "$(date) done (exit $?)" >>"$log"
) </dev/null >/dev/null 2>&1 &
disown

echo '{"systemMessage":"Updating memory/ from this conversation in the background (log: '"$log"')."}'
exit 0
```

### 9.3 Ground checkpoints

The live Ground is the main checkout's `.bracken/`, and git is its durable record. Agents add new files there continuously, and the manager commits them at least daily, and always before upgrading the pinned binary:

```sh
# from the main checkout, on main
git add .bracken
git diff --cached --name-only | grep -v '^\.bracken/' && echo "stop: other files staged"
git commit -m "ground: checkpoint $(date +%Y-%m-%d)"
```

A checkpoint commit only adds `.bracken/` files and deploys nothing, so it merges like any other no-deploy change. Use a PR where branch protection requires one. Commit before pulling or merging into the main checkout: git refuses a pull that would overwrite untracked files.

**Never commit `.bracken/identities/`.** If `git status` ever shows a file there as staged or tracked, stop and tell the owner: that key must be treated as leaked and replaced.

To restore, clone the repo. Contributions, causal history and agent names all come back; private keys don't. Create new identities under new names (`manager-2`); the directory refuses names that are already taken. Recorded signatures stay verifiable, because each contribution carries its author's public key.

### 9.4 The manager's state

Most of what the original process kept in a state file is now derivable from the Ground: open tasks, PR heads and their gate status, owner asks, unrecorded decisions. **Don't duplicate it.** The manager keeps only what the Ground can't hold in **`memory/manager-state.json`**, uncommitted (add `memory/manager-state.*` to `.git/info/exclude`):

```json
{
  "updated": "YYYY-MM-DDTHH:MM",
  "manager": { "thread": "<id>", "worktree": "<path>" },
  "builders": [ { "thread": "<id>", "identity": "builder-<branch>", "task": "<task claim id>" } ],
  "lastCheckpoint": "<commit sha>",
  "nextSteps": [ "<in order>" ]
}
```

Rules: update it **whenever state changes** and remove finished items. Change field values only; never reformat or rewrite the file wholesale. Check that the file still parses after each edit (`jq . memory/manager-state.json`).

**After a compaction, before anything else**, rebuild the picture:

```sh
jq . memory/manager-state.json
scripts/bk log --tag decision
scripts/bk log --tag task
scripts/bk log --tag owner-ask
scripts/bk heads
scripts/bk conflicts
gh pr list --author @me
```

Then read the builder threads. In Claude Code, add a `manager-state` entry to the manager's auto-memory, indexed as "read first after compaction", that points to this checklist.

## 10. CI guards

- **Tests, typecheck, lint** for every package, on every PR and on `main` (for Rust: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`).
- **Lint warning cap:** zero warnings with `-D warnings`. If a project starts above zero, cap at today's count and lower it as warnings are fixed; never raise it.
- **Protocol guard** (Bracken repo): a test that opens a checked-in fixture Ground written by an earlier release. A PR that breaks it is a protocol change and needs the migration note and the owner's go.
- **Publish workflows:** a separate `concurrency` group per stage; tag every publish in git (e.g. `v<version>-<stage>`); only a tested pre-release whose commit is on `main` may be published.
- **Deploy workflows** run on merge to `main`, filtered by path. Those filters decide which merges need the owner's go.

## 11. Retiring the scaffolding

This process is a bridge. As Bracken gains capabilities, the manager proposes retiring the parts they replace, as a bigger process change with before-and-after numbers:

| When Bracken has… | Retire… |
|---|---|
| Network sync between replicas | `scripts/bk`'s shared-directory trick; agents on other machines join the Ground |
| Notifications or subscriptions (Signal) | Most **send** wake-ups and the triage queries |
| Ground membership and per-key permissions (Stewards) | The shared identities folder and the "never use another role's identity" honour rule |
| Code as artifacts with a materialized tree (Weave) | Branches and PRs for code, the merge to `main` step, and finally Git as the team's source of truth. Git stays only for publishing releases (Rings) |

---

## Kickoff checklist

Tick these off in the setup PR description:

- [ ] Project profile complete; `TODO(owner)` items listed
- [ ] Tool mapping filled for the harness
- [ ] Pinned `bracken` binary installed; `scripts/bk` installed; `.bracken/` and `.bracken-bin/` ignored
- [ ] Ground created; identities `owner`, `manager`, `reviewer`, `security`, `triage` created
- [ ] Goal and profile decisions contributed as claims
- [ ] First Ground checkpoint committed (and `git ls-files .bracken/identities` prints nothing)
- [ ] `AGENTS.md` (breaks-production list, conventions, test table, releases summary, The Ground, Memory, Working with other agents)
- [ ] `docs/development.md`, `docs/releases.md`, `docs/README.md`, `docs/builder.md`
- [ ] `AGENTS.md` trimmed and approved by the owner
- [ ] `memory/INDEX.md`, `memory/archive/INDEX.md`
- [ ] PreCompact hook + settings (Claude Code only; `jq` and `claude` installed)
- [ ] Builder warm-build script on worktree creation
- [ ] Default, reviewer and security models chosen and authenticated
- [ ] CI guards present or proposed (deploy changes wait for the owner)
- [ ] Monitoring and daily triage present or proposed
- [ ] `docs/process-metrics.md` created with a baseline week
- [ ] Manager started, manager claim contributed, state file written, owner briefed
