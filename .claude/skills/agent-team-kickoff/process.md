# The process

How an owner and a team of agents work on a repository that coordinates through a Bracken Ground. It's set up by [SKILL.md](SKILL.md). Each repo's `docs/development.md` gives its value for every `<placeholder>` here, plus its tool mapping. Section references ("section 9.4") are to this file.

## 1. Goal

> The vision in `<vision doc>`: `<vision>`. Along the way: nothing reaches users untested, the owner decides only what matters, and agents work in parallel without losing track or stepping on each other.

The goal says what the work is **for**, not only how it's done. `<vision doc>` is the project's vision document, and `<vision>` sums it up in a sentence or two, in the owner's terms. The rest is how the team works toward it.

The goal is in the Ground as well as the repo:
- The vision document is an **artifact** in the Ground, named by its repo path. Every change to the document adds a new artifact that supersedes the previous one, **in the same PR**. `scripts/ground-guards` checks that the latest one matches the file, at every merge (section 10).
- The goal is the current claim tagged `goal` (`scripts/bk log --tag goal --current`), and it `depends_on` the vision artifact it was written against. A new goal supersedes the old one (`claim … --supersedes <old>`).

Every decision, task and process change is judged against the goal, the vision first. A task that ships safely but doesn't move the vision forward needs a reason. The manager may propose new wording; only the owner changes it.

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

**Delegation.** The owner may delegate their go to the manager for everything but a list of sensitive or destructive actions (the project's list is in `docs/development.md`). The manager then decides the "ask first" items itself and records each such decision as a claim tagged `owner-review` (what, why, how to undo), and reports them to the owner at the next check-in to agree or reverse. A check-in happens when the owner returns or asks: the manager lists `scripts/bk log --tag owner-review --current` and records each answer (section 4.1).

**Builders and reviewers ask the manager** for any approval, never the owner: a `question` claim, then a message. The manager answers within the delegation, and raises an `owner-ask` only for the owner's list.

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
| Owner review | claim, tag `owner-review`: a decision the manager took under the owner's delegation (section 3): what, why, how to undo. Once the owner has reviewed it, record their answer as a claim that `refines` it (a reversal also undoes it), then retract the `owner-review` claim | manager |
| Task | claim, tag `task`: the full builder brief (section 6) | manager |
| Design note, hypothesis, discovery | claim, tag `note`; relate it to the task or the code it concerns | anyone |
| PR head | claim, tag `pr`: "PR #<n> <branch> @ <full sha>: <title>"; `relate <pr> implements <task>` | builder |
| New commit on a PR | new `pr` claim with `--supersedes <old>`. Evidence on the old head no longer counts, so every head is verified on its own | builder |
| Test run | evidence on the `pr` claim, recorded with `"$main/scripts/bk-evidence" <pr> --as <you> -- <command>` (the main checkout's copy, step 5): `--method "<command> @ <sha>"`, `--details` with the exit code and output tail | builder (recorded, never counted), manager (counted) |
| Review verdict | evidence on the `pr` claim: `pass` or `fail`, `--method "independent review (<model>) @ <sha>"` | reviewer, security |
| Review finding | claim, tag `finding`, with file:line, severity, the requirement broken and a counterexample; `relate <finding> contradicts <pr>` | reviewer, security |
| Finding triage | evidence on the `finding`: `pass` = confirmed (say how), `fail` = rejected (say why) | manager |
| Gotcha, preference, reference | claim, tag `gotcha` / `preference` / `reference` (section 9.1) | anyone; preferences quote the owner |
| Manager, builder threads, plan | claims, tags `manager` / `builder` / `manager-state` (section 9.4) | manager |
| Process proposal | claim, tag `process` | anyone; the manager decides and records a `decision` |
| Withdrawal | `retract <own id> --reason "..."` | the author only |

Rules:

- **Contribute as you learn, not at the end.** A builder that finds out something another agent would need (a gotcha, an interface decision, a dead end) contributes a `note` right away and relates it. That's what lets parallel work build on itself.
- **Never act on a claim alone.** Use `scripts/bk show <id>` to check its status, evidence and referrers. A `note` is a lead, not a fact.
- **Ids in messages, content in the Ground.** Quote the short id (12 hex characters) everywhere: messages, PR descriptions, commits.
- **No secrets or personal data in the Ground.** Contributions are immutable: a retracted contribution still exists, and it's in git history for good.
- **Tags are queries.** Readers take the current claim with a tag such as `goal`, `manager-state`, `pr` or `manager` (`scripts/bk log --tag <tag> --current`), so tag a claim with one of those only if it is one. A decision about the goal is tagged `decision`, not `goal`. Put topics in the statement.
- **Disagreement is data.** If you think a decision, a finding or a test is wrong, contribute a claim that `contradicts` it with your reasoning. Don't argue in messages, and don't quietly work around it.

### 4.2 Merge gate

Merge readiness is the acceptance status of the **current `pr` claim** under the class's policy. An author's evidence about their own contribution never counts: the library ignores it. So the gate depends on who wrote the `pr` claim. It also requires the remote PR's head to be the `pr` claim's full sha, checked and recorded right before the squash-merge (step 8).

| Who wrote the `pr` claim | Class | Gate: `scripts/bk show <pr> …` reports `accepted` |
|---|---|---|
| A builder | Low risk | `--validator manager --validator security --min-passes 2` |
| A builder | `<risky classes>` | `--validator manager --validator reviewer --validator security --min-passes 3` |
| The manager | Any | `--validator reviewer --validator security --min-passes 2` |

**When the manager authors a change** (process tweaks, setup), its own verification can't count, so the independent review is required whatever the class. The reviewer also re-runs the done-when checks in a fresh worktree at the head, and its pass says so in `--details`.

Any counted `fail` makes it `rejected`. The fix is a new commit and a new `pr` claim that supersedes the old one, where the gate starts again; or that validator retracts its `fail` and passes.

CI is not part of the gate (section 10). Where it runs on every PR, its result is a second opinion: when it disagrees with the Ground's evidence, the manager records why before merging. Where it runs only by hand (the repo's CI role, `docs/development.md`), no PR waits on it: the manager's counted run of the test table on the exact head (step 5) is the CI.

**Findings.** The complete list of findings against a head is the referrers of `scripts/bk show <pr>`: every `finding` that `contradicts` it, triaged or not. Retracting a `contradicts` link drops the finding only when the finding's author or the manager wrote the link; `scripts/merge` keeps counting it otherwise. `scripts/bk conflicts` lists them too, except one its own author has retracted or superseded; someone else's supersession can't silence a finding. Rejected findings stay listed until the head changes; that's expected, not a blocker. Before merging, every finding against the head must have manager evidence:

- A **confirmed** finding (pass) blocks the merge until it's fixed in a new head, or **deferred**: a `task` claim (tags `task`, `deferred`) that `depends_on` the finding and says what will fix it and when. Only the manager records a deferral, writing both the task and the relation; `scripts/merge` counts no other. The manager defers only findings that don't make this PR unsafe to merge, such as a risk that only becomes real after a later change, and says so in its triage evidence. A deferral can't override a validator's `fail` verdict, since any counted `fail` rejects the head: a confirmed finding behind a `fail` is fixed in a new head, or that validator retracts its `fail` and passes with conditions (section 5.2).
- A **rejected** finding (fail) stays on record and doesn't block.

A new head supersedes the old one, and that clears *all* of the old head's contradictions, fixed or not. So the next review round's brief lists every confirmed finding, and the reviewer must check each one against the new head. Anything still unfixed gets re-filed against the new head.

### 4.3 The team server

Optionally, a sync server on the owner's machine holds the team's Ground, so replicas elsewhere (other machines, later) stay current within seconds. The manager runs it with `scripts/team-server`. The main checkout's `.bracken/` stays a full replica, and checkpoints (9.3) carry on as before. Once it's set up:

- **Every agent sets `BRACKEN_AGENT=<its identity>`** in its environment, so that `scripts/bk` can find its token. The manager issues one token per identity with `scripts/team-server token <identity>`. The token stays in a private file outside the repo; agents never see or pass it. With `BRACKEN_AGENT` set, `scripts/bk` refuses an `--as` that names another identity, which stops signing as someone else by mistake. A script that acts as another identity on purpose sets `BRACKEN_AGENT` for its own `scripts/bk` calls, as `scripts/owner-approve` does. Relays aren't refused: the replica pushes other identities' unpushed contributions under whichever token runs next, and `scripts/ground-guards` reports them from the server's first-seen log, per credential and author, without failing.
- **A write confirms.** After each write, `scripts/bk` pushes and checks that the server's heads cover it. If they don't, it fails, and the work isn't done: the contribution is only local until a later sync pushes it (any `scripts/bk` read does). Never report a write that failed.
- **Reads pull first** and fall back to the local replica if the server is down, saying so on stderr.
- **The merge gate fails closed.** `scripts/merge` requires every read to pull. With the server down, it refuses to decide.

The manager starts the server after each restart of the machine. Issuing a token restarts a running server, since it reads its tokens and allowlist only when it starts. The repo's values (where the server's files live, the port) are in `docs/development.md`. Reaching the server from another machine, and any off-host backup, need the owner's go.

## 5. A change, step by step

1. **Task:** the manager contributes a `task` claim holding the brief, then launches the builder with that id. One task, one worktree, one branch, one PR.
2. **Branch:** short kebab-case, no prefix.
3. **Build and test locally:** every command in the `AGENTS.md` test table for the touched packages, plus the brief's definition-of-done checks. **Never delete, skip or weaken a test to get a green run**; if a test seems wrong, contribute a claim that `contradicts` it and let the manager decide.
4. **PR head:** commit, then contribute a `pr` claim (or a superseding one) and evidence for each test run, recorded with the main checkout's `"$main/scripts/bk-evidence" <pr> --as builder-<branch> -- <command>` (step 5) so the exit code and output are the real ones. For a pipeline, use `bash -o pipefail -c '…'`, or only the last stage's exit code is recorded. The PR description has the change **class**, what wasn't tested, the release steps, and the task id. It can't hold the `pr` claim id, which needs the PR number first: the current `pr` claim is found with `scripts/bk log --tag pr --current`.
5. **Verify.** The manager re-runs the done-when checks in a **fresh worktree at the exact head commit**, never in a checkout that might hold uncommitted files: `git worktree add --detach "$TMPDIR/verify-<sha>" <sha>`, run the checks there, then remove the worktree (section 6, Worktrees). It records the result as counted `manager` evidence with the sha in `--method`, running the main checkout's copy from inside the verify worktree: `"$main/scripts/bk-evidence" <pr> --as manager --on origin/main -- <command>`, where `main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")`. With `--on origin/main` it tests the tree the merge will produce: `main`'s tip merged with the head, in a scratch worktree it removes, recorded as `<command> @ <head> on <main sha>`. Run the whole test table as that one command, `scripts/test-table`: when `main` is ahead of the head in code, `scripts/merge` counts only a manager pass of `scripts/test-table`, because a pass of part of the table (a re-run of just the tests after `main` moves) proves too little. It runs the merged tree's own copy, so read any change a PR makes to `scripts/test-table`. The `pr` claim, reviews and other validators' passes stay on the head. A head that conflicts with `main` is refused, and only then does the builder merge `main` into the branch. Verifying runs the PR's code on the host, where it can read the Ground's keys and the remote's credential (`gh`'s token), so first read the PR's changes to build scripts and dependencies (`build.rs`, `Cargo.lock`, `package.json` and the like), and call the **main checkout's** `scripts/bk` and `scripts/bk-evidence` by absolute path, never the PR worktree's copies: a PR can change them, and its copy would then sign counted evidence on its own PR. Where CI runs on every PR (section 10), it runs too; if CI and the manager's evidence disagree, the manager contributes a claim tagged `ci-disagreement` saying which was right and why.
6. **Reviews** (independent for risky classes; security for all) record findings and verdicts in the Ground. **Finish every review round before manual testing**, so the tested build is the one that merges.
7. **Manual test**, if the change can be seen or felt: test the real thing, and record the result as manager evidence with a short report in `--details`.
8. **Merge:** once the gate passes (section 4.2), and right before the squash-merge:
   - **`main`:** its tip must be an ancestor of the verified head, or every commit on `main` that the branch lacks touches only `.bracken/`, or files the branch doesn't touch that are `docs/` files other than `docs/vision.md`, or `.md` files under `.claude/skills/` outside any `templates/` directory. None of those can change what builds, tests or runs. Otherwise `main` may be ahead in any path if a counted manager pass tested the merge (step 5, `--on`): its method names this head and a `main` sha that `main`'s tip has moved past since only in those same paths. If `main` has moved in code since, the manager verifies again on the new tip; the head, its reviews and the other passes stand. Merging `main` into the branch is needed only when it conflicts: that's a new head, and the gate applies to it.
   - **Ground guards:** `scripts/ground-guards <main's tip> <head>` must pass on the head: no keys or stray files under `.bracken/`, no Ground file deleted or changed, the committed Ground opens, and the vision document matches it (section 10).
   - **PR head:** the remote PR's head commit must be the current `pr` claim's full sha: `gh pr view <n> --json headRefOid`, or the API's `.head.sha`.
   - **Record** them as manager evidence on the `pr` claim: `--method "merge gate @ <sha>"`, `--details` with the PR head, which `main` case applied, and the guards' result.

   Then squash-merge at once. `scripts/merge <n> <pr claim> --validator … --min-passes N --as manager` does all of this: it runs the checks (`--dry-run` stops there), records the evidence, and squash-merges through the GitHub API (`gh api`, `PUT pulls/<n>/merge` with `sha` set to the verified sha), so GitHub refuses the merge (409) if the branch moved after the check. Merging by hand, a push between the check and the merge isn't caught. If it deploys, wait for the owner's go. If the PR touched the code of the pinned binary (for Bracken: `crates/bracken` or `crates/manyroot`), upgrade the pinned binary right after the merge (`docs/releases.md`), before sending any agent work that relies on the new docs. After the merge, bring it into the main checkout only through `scripts/ground-checkpoint` and `git merge --no-edit <the sha it names>` (9.3), never `git pull` or a `--ff-only` merge of `origin/main`: those take whatever `origin`'s `main` holds by then, unchecked.
9. **Each release stage** toward users needs the owner's go, recorded as an `approval`.
10. **Watch it land.** Check triage and error reports for anything new from the release, and close out the task with a `note` saying what shipped. Then remove the builder's worktree (section 6, Worktrees).

### 5.1 Independent review (risky classes)

Risky classes for this project: `<risky classes>`. Low-risk changes by a builder merge after the manager's verification and security review.

- **delegate** a child on the review model; if that model wrote the PR, use another. It runs as `reviewer` and writes only `finding` claims and evidence. Run it for risky classes, and for **every PR the manager authored** (section 4.2); then the brief also asks it to re-run the done-when checks in a fresh worktree.
- Brief it with the PR number, the `pr` claim id, the class, what must not break and the diff. **Don't give it your conclusions**, and don't point it at your own notes about the PR.
- **Every review brief opens with context, in three layers:**
  - **PROJECT:** the project's goal, one paragraph from the vision (this repo's is in its development doc);
  - **ARC:** the current epic, its goal, and what this PR must give it;
  - **TASK:** the PR's purpose, what must hold, and what's out of scope.
- **Then calibration.** A finding names the project, arc or task property it threatens and how. Anything else goes in the verdict's details as a note, and the verdict says how many notes it kept out of findings. Prefer findings shown by a run, a test or a mutation. A clean pass is a good outcome, and excluded scope isn't re-litigated. A note that's a real gap becomes a follow-up task at the manager's call, not a blocker.
- Each round is a new child. Its brief carries the original brief, the earlier finding ids and how each was handled (the manager's evidence on them), and open disagreements.
- Each finding is its own `finding` claim with file:line, a severity, **the requirement it breaks** (from the brief, the PR description or the project's rules) and **a concrete counterexample**, ideally a failing test. It's labelled **confirmed** (shown to fail) or **possible risk**. The round ends with a verdict as evidence on the `pr` claim.
- AI reviewers often reject correct code and invent requirements, and asking for fixes makes this worse. So the manager **triages findings before passing them to the builder**. It rejects (fail evidence, with the reason) findings outside the task's scope (never a security finding, section 5.2) or with no requirement behind them. It asks for a counterexample when one is missing. It checks confirmed findings by running the test, then records a pass.
- **At most three rounds.** After that the manager decides on what's still open and records a `decision` saying why.
- The builder answers every confirmed finding: fixed in a new head, a deferral proposed to the manager (a `question` claim saying what would fix it and when), or disputed (a claim that `contradicts` it). If the manager agrees, it records the deferral itself.

### 5.2 Security review (every proposal, every PR)

A child on a strong model, running as `security`, its brief opening with the same context and calibration as section 5.1's, checks the list below. The calibration never narrows this checklist: it applies in full whatever the TASK excludes. A real risk under it is always a finding, never only a note, even one a run can't show (labelled possible risk), and the manager's triage never rejects it as outside the task's scope. The finding blocks unless the manager defers it as sections 4.2 and 5.1 allow, recording why.

- **Credentials:** nothing that grants access ships in a client bundle; secrets live server-side in a secret manager. Every proposal says where its credentials live and what a leak would expose.
- **Data flows:** what leaves the device or backend, where it goes, whether users would expect it.
- **Access control and permissions:** data rules, endpoint auth, admin paths, CI service accounts, workflow triggers. PR code must never reach a credential.
- **Input handling, supply chain** (new crates, packages and actions), **logging and telemetry** (no secrets, personal data or user content).
- **Protocol integrity** (Bracken repo): changes to encoding, ids, signatures or validity rules must keep the guarantees in `docs/protocol.md`. Above all: validity depends only on a contribution and its parents, there's exactly one encoding per contribution, and signatures are verified strictly.
- **Agents and the Ground:** secrets never appear in agent output, PRs, comments, memory or **any contribution**, which is immutable and committed to git history. Contributions refer to people outside the team by role, not by name. No production user data or secrets go into any brief or task claim, since briefs may reach other model providers.

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
- **The manager verifies rather than trusts:** it re-runs the done-when checks on the head commit (or, where CI runs on every PR, confirms CI ran them) and records its own evidence. It also checks the original objective was met, not only that the code compiles.
- The manager uses **watch-pr** instead of polling where it can.

### Shared resources (manager only; builders ask)

| Resource | Rule |
|---|---|
| Merges to `main` | One at a time, each verified on `main`'s current tip (step 5, `--on`); merge `main` into the branch only for a real conflict. Deploying merges need the owner's go. |
| The pinned `bracken` binary | Only the manager upgrades it, and only from `main`, between merges. Before upgrading, commit a Ground checkpoint (9.3) and check that the new binary opens it (`scripts/bk log >/dev/null`). Protocol or storage changes need the owner's go (or the manager's, where delegated: section 3) and a migration note in `docs/releases.md`. |
| The Ground itself | Everyone contributes through `scripts/bk`; no one edits or deletes files under `.bracken/`. Only the manager runs `init`, `import`, `join` or `sync`, and only the manager commits `.bracken/` (9.3). Builders never stage it: their worktree's copy is a stale snapshot. |
| Long-lived release branches | Only through PRs merged by the manager. Never make one a PR's head (merging deletes the head). |
| Publishing, deploys, billed builds | The manager, with the owner's go. |

### Avoiding conflicts

- Split work so builders touch different files; otherwise run tasks in sequence or stack them. Note overlaps in the task claims with `depends_on` relations.
- Never rewrite another agent's branch. Merge `main` into your own when it conflicts (not to keep up: step 8); force-push only your own, with `--force-with-lease`.
- **`git stash` is shared across worktrees.** Use a WIP commit instead.
- Concurrent contributions never conflict in the Ground. Two builders contributing at the same moment just produce two causal heads. Only a claim's author can supersede it (another agent's `supersedes` relation stays visible but doesn't count), so if the author accepts **two different replacements** for one claim, `scripts/bk conflicts` shows a divergence. That's a real disagreement for the manager to settle, not an error to clear.

### Worktrees

Whoever makes a worktree removes it as soon as its work is done. Each one holds a full build tree, and stale ones fill the disk and make `git worktree list` unreadable; this team once had 120.

| Worktree | Made by | Removed by, and when |
|---|---|---|
| Builder's | **launch-builder** | The manager, once the PR is merged or closed, the builder's report is in (step 10) and the builder has been told it's done and that turn has ended. Its thread runs `scripts/bk` from that worktree, so ask nothing more of it afterwards |
| Delegated child's (review, search) | **delegate** | The manager, once the child has finished and its findings and evidence are in the Ground |
| Scratch: verifying a head (step 5), a reviewer's clean or mutation copy | The agent that runs `git worktree add` | That agent, before it reports. A reviewer's report says it did |

How:

- **Know whose it is.** Remove only a worktree you can match to a finished task or child through the harness (`docs/development.md` says how), never one whose agent may still be running. The owner's own threads can have worktrees that look just like the team's; leave any you can't match, and list them for the owner.
- **Check it holds no work.** First run `git fetch --prune origin`, and if it fails, stop: remove no worktree and delete no branch until it succeeds. Without the prune, the local copies of remote branches stay after the remote deletes them, and both this check and the branch checks below trust those copies. Then both must print nothing: `git -C <path> status --porcelain` (uncommitted files) and `git -C <path> log --oneline HEAD --not --branches --remotes` (commits on no branch or remote, as on a detached HEAD, whose only record goes with the worktree). If either prints something, leave the worktree and ask its author (or the manager) to keep the work. Don't commit or push files someone else left without reading them: they can hold anything, a copied key included.
- **Remove it** with `git worktree remove <path>`, then `git worktree prune`. Without `--force`, git refuses a worktree with modified or untracked files, and ignored build output doesn't stop it. If it refuses, look at what's there, and add `--force` only once you've judged it disposable. Don't delete the directory by hand: git keeps its entry, and its branch stays checked out, so no other worktree can use that branch.
- **Branches.** Removing a worktree leaves its branch. To delete the local branch (after the prune above), use `git branch -d`, which refuses commits that aren't on its upstream (or, with none, on the current branch). Squash-merged branches are never ancestors of `main`, so `-d` refuses many that are done with. Use `-D` only when, after the prune, `git branch -r --contains <branch>` lists a remote branch, or a PR head the remote still holds contains the tip: `git merge-base --is-ancestor <branch> <sha>` succeeds for a sha from `git ls-remote origin 'refs/pull/*/head'` (it fails, which is safe, if that sha isn't local). Otherwise leave it. Never delete a remote branch by hand, or a branch someone else uses, without the owner's go (section 3). The one exception is `scripts/merge` (step 8), which deletes a PR's head branch once it has merged it.
- **Sweep at each Ground checkpoint (9.3):** the manager runs `git worktree list` and removes, as above, every worktree whose task is done or whose child has finished.

An agent working unattended must not run commands the harness asks a human to approve, or it waits until the owner is back. Cleanup is the usual one: some harnesses ask before any file deletion they can't prove safe, even in full-access mode. So clean up with `git worktree remove` and `git rm`, and leave scratch directories made with `mktemp -d` for the system to clear. `docs/development.md` says what this repo's harness asks about and how it's handled.

## 7. Monitoring and triage

- **Error monitor:** a scheduled job reads production logs and files issues (label `error-monitor`) for new errors.
- **Crash/error reporting:** tag every report with its release; add breadcrumbs for the steps before a problem; report silent failures as non-fatals; **never include user content**.
- **Daily triage** (**schedule**, fixed time, fresh thread, identity `triage`; read-only for code, writes only to the Ground) covers:
  - new error-monitor issues, and failed CI on `main` where CI runs on every push;
  - what shipped since yesterday;
  - `scripts/bk conflicts` (divergences, and findings against current heads; the referrers of each open `pr` are the complete list);
  - `owner-ask` claims still open, and `owner-review` claims not yet reviewed;
  - `pr` claims pending for more than a day;
  - tasks with no recent contributions.

  It contributes a claim tagged `triage` and sends the manager its id. On a quiet day: one line, no claim.

## 8. Improving the process

When something costs time, gets lost, needs the owner when it shouldn't, or nearly reaches users untested, the manager asks what change would have prevented it.

- Everyone proposes via `process` claims. The manager decides, and records a `decision` that `supports` or `contradicts` the proposal.
- **Small changes** (wording, a missing step, a checklist item, a better brief, a lint rule, a gotcha or preference claim, a local script): the manager makes them, merges them (they deploy nothing) and mentions them in its next update.
- **Bigger changes:** a short proposal to the owner: problem, change, cost, and which numbers (below) it should move.

### Keeping the skill current

The skill is the team's process, and the owner wants it to match where development actually is at all times. Anyone kicking off a new team from it should get today's setup, not last month's. So:

- `process.md` (this file) is the only copy of the process. A repo's `docs/development.md` holds only that repo's values and links here; it never copies the process.
- **Every change to how the team works updates the skill in the same PR:** `process.md` for the process, `SKILL.md` for kickoff steps, `templates/` for installed files. The PR description says which.
- Lessons that would help any project (a gotcha in the kickoff steps, a better default) go into the skill too, not only into one repo's Ground.
- Before merging any `process`-class PR, the manager checks that the skill and the repo agree: no doc, template or AGENTS.md rule contradicts `process.md`.

### Measuring

Judge the process by results, not by whether steps were followed. Once a week, the manager (or a scheduled run reporting to it) contributes a claim tagged `metrics` with the week's row:

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

Long threads get compacted, agents end, and anything not written down is lost. Here **everything worth keeping is a contribution**: there's no separate memory folder and no state file. What makes that work is the rule in 4.1, contribute as you learn. Anything that exists only in a context window is already at risk.

### 9.1 Shared knowledge

Besides decisions and tasks, three tags carry what a future agent would otherwise get wrong or have to ask:

| Tag | What | Example |
|---|---|---|
| `gotcha` | A trap that cost time, with the fix | "`gh repo create` makes a personal repo unless you name the org: `gh repo create <org>/<repo>`" |
| `preference` | How the owner wants agents to work, quoted and dated | "Owner, 2026-10-27: no AI attribution in commits" |
| `reference` | Where something lives outside the repo | "GitHub CLI manual: `gh help` and https://cli.github.com/manual/" |

- **At the start of a task**, read `scripts/bk log --tag decision --current`, `--tag gotcha --current` and `--tag preference --current`, and `--tag reference --current` when it's relevant. Each prints one short line per entry, and `scripts/bk show <id>` prints one in full. Read them whole: `--since <date or cursor>` and `--limit N` are for follow-up reads only, such as what's new since the cursor you last read at. For findings, read `scripts/bk log --open` (those the merge gate would block a current head on: untriaged by the manager, or confirmed and not deferred), not every finding. `--current` leaves out retracted entries, entries their author superseded (the replacement can be anyone's), and anything about those. An entry that someone else superseded stays, marked `(superseded by <author> <id>)`: nobody can hide another author's entry. Before relying on one, run `scripts/bk show <id>` to see whether something contradicts it.
- **Verify before acting:** if an entry names a file, function or flag, confirm it still exists.
- **Don't contribute:** what the code, `git log` or docs already say (update those instead); secrets or personal data (contributions are permanent); anything only the current conversation needs.
- **Keep it current, never delete.** When an entry changes, its author contributes the new version with `claim … --supersedes <old>`, which also records the `supersedes` relation. `claim --supersedes` refuses another author's claim: contradict or refine it instead. When it's wrong, anyone contributes a correction that `contradicts` it, and its author can retract it.
- **Ids are the links.** Refer to entries by short id in docs, PRs and other claims.

### 9.2 No compaction hook

The original process ran a headless model before each compaction to draft memory from the transcript. This one doesn't. Drafts written into the Ground would be permanent even when rejected, and the Ground is already the safety net when agents contribute as they go. If a compaction loses something, contribute a `process` claim saying what wasn't recorded and why.

### 9.3 Ground checkpoints

The live Ground is the main checkout's `.bracken/`, and git is its durable record. Agents add new files there continuously, and the manager commits them at least daily, and always before upgrading the pinned binary:

```sh
scripts/ground-checkpoint                 # if it names a checked commit of origin's main:
git merge --no-edit <that sha>            # pull exactly that commit, not a fresh git pull
git push origin main
```

The Ground is append-only, and its committed files have exactly three shapes: `ground.json`, `agents/<name>.json` and `objects/<2 hex>/<62 hex>`. `scripts/ground-checkpoint` commits only new files of those shapes. It stops without committing if a tracked Ground file was changed or deleted, if anything under `.bracken/` has another shape (a key, a backup copy of the keys, a stray file), if an agent entry holds a private key, if a new file contains the value of any key in `.bracken/identities/` written as hex in any case (an identity file added as an artifact, a key pasted into a claim; a re-encoded key, such as base64, isn't detected), if the main checkout isn't on `main`, if anything else is staged, or if the Ground doesn't open. It also fetches `origin`'s `main` and, if that has commits the local `main` doesn't, runs `scripts/ground-guards` on them from the local `main` (section 10) and stops if they fail: that's every pull into the live Ground, so a commit that reached `main` without `scripts/merge` is checked before it lands. Pull only the commit it names as checked. That's the only way to update the main checkout, after a merge too: a `git pull`, or a fetch and `--ff-only` merge of `origin/main`, takes commits no guard checked, and the next checkpoint then sees nothing to check. If `origin` can't be reached, the checkpoint still commits the local Ground's new files, says it checked nothing, and names no commit to pull; rerun it once `origin` is reachable, then push. Never commit Ground files any other way, and no PR may delete or change one (`scripts/ground-guards` checks at every merge, section 10). The one exception to running on `main` is the kickoff's setup branch, which uses `--setup` (SKILL.md, Phase 2).

**A change to the root `.gitattributes`.** Checkout applies attributes to the Ground's files (an `eol=crlf` rule would break every contribution it converts), so `scripts/ground-guards` refuses any change to the root `.gitattributes`, `scripts/merge` won't merge one, and every checkpoint after one reaches `main` stops on it. Nested `.gitattributes` files (test fixtures, say) apply only to their own directory and pass. To land a root one:
1. Read the change, and check that `.bracken/` keeps exactly `text: unset`: in a scratch clone at the PR's head, `git check-attr -a -- .bracken/ground.json .bracken/objects/ab/x` prints `text: unset` and nothing else for each.
2. Merge the PR on the remote by hand.
3. In the main checkout, run `scripts/ground-checkpoint --attributes-checked <origin's main, full sha>`. It runs every other guard on that commit, records a `note` as manager saying the change was checked, commits, and names the sha to merge; then merge it and push as usual. If `main` moved on after the merge, check what else it brought in before passing its sha.

A checkpoint commit only adds `.bracken/` files and deploys nothing, so it merges like any other no-deploy change. Use a PR where branch protection requires one. Commit before pulling or merging into the main checkout: git refuses a pull that would overwrite untracked files. After each checkpoint, sweep the worktrees (section 6, Worktrees).

**Never commit `.bracken/identities/`.** If `git status` ever shows a file there as staged or tracked, stop and tell the owner: that key must be treated as leaked and replaced.

**A key inside a contribution.** If the checkpoint stops because a new file contains a key, that contribution can't be removed, and retracting it doesn't help: it still holds the key, so every later checkpoint stops too. Treat the key as exposed and tell the owner. Recovery, with the owner's go: create a replacement identity under a new name, and record a `decision` saying which key is compromised. Then the owner moves the old identity file out of `.bracken/identities/` (to somewhere outside the repo, or deletes it). With that key no longer local, the checkpoint commits again.

The old key is distrusted only by that decision, not by the protocol or the CLI. Its name still resolves (`.bracken/agents/<name>.json` stays), so `--validator <old name>` would still count evidence that anyone holding the leaked key signs. So:

- Replace the old name with the new one in section 4.2's gate commands, in the role briefs (sections 5.1 and 5.2) and in `docs/development.md`, and never pass the old name to `--validator` again.
- Treat everything the old key signed as suspect, not only what came after the leak: `timestamp_ms` is advisory, so a forgery can be backdated. Re-check any open `pr` whose gate counted that key, and have the new identity re-record any evidence or decision still needed.

To restore, clone the repo. Contributions, causal history and agent names all come back; private keys don't. Create new identities under new names (`manager-2`); the directory refuses names that are already taken. Recorded signatures stay verifiable, because each contribution carries its author's public key.

### 9.4 The manager's state

The manager's state is in the Ground too:

| What | How |
|---|---|
| Manager thread | Claim `--tag manager`: thread id and worktree. A new manager contributes its own claim, which supersedes the last (`claim … --supersedes <old>` under the same identity; under a new one, `relate <new> supersedes <old>` signed by the new one). |
| Builder threads | Claim `--tag builder`: "Thread <id> builds task <task id> as builder-<branch>", then `relate <claim> refines <task>`. Retract it when the builder is done. |
| Plan | Claim `--tag manager-state`: the next steps in order, and what's waiting on whom. Each update is a new claim with `--supersedes <previous>`, so the current claim is the plan and the chain is its history. After a handover to a new manager identity, the old manager's last `manager` and `manager-state` claims stay in `--current`, marked as superseded by the new one, until the old identity retracts them, or for good if its key is gone. |

Update the plan whenever it changes, not at the end of a session.

**After a compaction, before anything else**, rebuild the picture:

```sh
scripts/bk log --tag manager-state --current   # the current plan; scripts/bk show <id> for all of it
scripts/bk log --tag builder --current
scripts/bk log --tag decision --current
scripts/bk log --tag task --current
scripts/bk log --tag owner-ask --current
scripts/bk log --tag owner-review --current   # delegated decisions the owner hasn't reviewed yet
scripts/bk conflicts
gh pr list                                     # or the remote's equivalent
```

Then read the builder threads. The harness's own memory (in Claude Code, auto-memory) holds only a pointer, indexed as "read first after compaction": "this thread manages <repo>'s agent team; after a compaction, follow section 9.4 of `.claude/skills/agent-team-kickoff/process.md`."

## 10. CI and branch protection

**CI is an observer, not a gate.** The Ground's evidence decides merges. CI runs the test table on every PR and on `main` in a clean environment, which covers two gaps the Ground can't yet: a check that only passed because of local state, and evidence that's only as trustworthy as the shared identity keys. Where the runner's compute is too slow for that, or a public repo's Actions can't be trusted with secrets, CI runs only by hand (`workflow_dispatch`), and the manager's counted run of the test table on the exact head is the CI: the repo's CI role (`docs/development.md`) says which. Branch protection doesn't require any check. Each `ci-disagreement` claim is data. After about 10 PRs or two weeks, the manager proposes keeping CI, gating on it, or dropping it, with the disagreements as evidence. When Bracken can sync over the network, CI should become a validator that contributes evidence to the Ground (section 11).

**Branch protection blocks force-push and deletion on `main`, applying to admins.** The Ground's history lives in git, so rewriting `main` would erase contributions. Direct pushes stay allowed, so the manager can commit Ground checkpoints (9.3). Changing protection needs the owner's go. Test a new rule on a scratch branch with the same settings, not on `main`.

The first four guards below are one script, `scripts/ground-guards <base> <head>`, which reads only what `<head>` committed and compares it with its merge base with `<base>`. `scripts/merge` runs it on the PR's head against `main`'s tip before every merge, `scripts/ground-checkpoint` runs it on what `origin`'s `main` gained before the manager pulls it into the main checkout (9.3), and the CI workflow calls it, so they hold whether or not CI runs on every PR. It reports the guard that failed, and a guard that can't check fails.

- **Only Ground files under `.bracken/`:** fails if any tracked file there isn't one of the three Ground shapes (9.3) as a plain file (mode 100644: no symlink, submodule or executable), if any path is `.bracken/` in another case (`.BRACKEN/` is the same directory on a case-insensitive filesystem), if any path in the head has a non-ASCII byte (a case-insensitive filesystem can fold one, such as a Kelvin sign for `k`, into `.bracken`), if the root `.gitattributes` was added, changed or removed since the merge base (landed by hand, 9.3), or if an agent entry holds a private key. It's the backstop for keys and backup copies if `.gitignore` ever loses its line.
- **Ground is append-only:** fails if a tracked Ground file was deleted or edited since the merge base. If there's nothing to compare with (no merge base, none within a CI fetch's depth, or a head that is the base itself), it fails rather than passing unchecked. Deleting a finding or a failing piece of evidence would change what the gate concludes.
- **Ground opens:** opens the committed Ground (with the pinned `bracken` at a merge, or the PR's own build in CI), which verifies every contribution's encoding, hash, signature and parents. It opens a copy of the `.bracken` tree taken without git attributes (a root `.gitattributes` with `export-ignore` could otherwise leave a file out), and fails unless the copy holds every file the head committed there.
- **Vision document matches the Ground:** the latest artifact named `<vision doc>` has exactly the file's contents (section 1).
- **No repo or org Actions secrets** while CI runs PR code: anything a workflow can read, a PR can read.
- **Token only in the checkout step:** fetch everything a later step needs (including `main` for the append-only check) in the step that has the token, so steps running the PR's code never see it.
- **Tests, typecheck, lint** for every package, on every PR and on `main` (for Rust: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`). A container without Node can't run JavaScript actions such as checkout: fetch with plain `git`, passing the token through `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_0`/`GIT_CONFIG_VALUE_0` environment variables, never argv or the remote URL. Pin images by digest.
- **Lint warning cap:** zero warnings with `-D warnings`. If a project starts above zero, cap at today's count and lower it as warnings are fixed; never raise it.
- **Protocol guard** (Bracken repo): a test that opens a checked-in fixture Ground written by an earlier release. A PR that breaks it is a protocol change and needs the migration note and the owner's go (or the manager's, where delegated: section 3).
- **Publish workflows:** a separate `concurrency` group per stage; tag every publish in git (e.g. `v<version>-<stage>`); only a tested pre-release whose commit is on `main` may be published.
- **Deploy workflows** run on merge to `main`, filtered by path. Those filters decide which merges need the owner's go.

## 11. Retiring the scaffolding

This process is a bridge. As Bracken gains capabilities, the manager proposes retiring the parts they replace, as a bigger process change with before-and-after numbers:

| When Bracken has… | Retire… |
|---|---|
| Network sync, so CI can contribute to the Ground | CI as a separate system: it becomes a `ci` validator whose evidence policies can count (section 10) |
| Network sync between replicas | `scripts/bk`'s shared-directory trick; agents on other machines join the Ground |
| Notifications or subscriptions (Signal) | Most **send** wake-ups and the triage queries |
| Ground membership and per-key permissions (Stewards) | The shared identities folder and the "never use another role's identity" honour rule |
| Code as artifacts with a materialized tree (Weave) | Branches and PRs for code, the merge to `main` step, and finally Git as the team's source of truth. Git stays only for publishing releases (Rings) |
