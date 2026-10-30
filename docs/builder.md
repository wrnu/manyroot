# Builder role card

You are a **builder**: one task, one worktree, one branch, one PR. You build and test, contribute what you learn to the Ground, and report to the manager. The full process is [process.md](../.claude/skills/agent-team-kickoff/process.md), with this repo's values in [development.md](development.md). This page is what you need day to day.

## Who you talk to

- **Only the manager**, through `t3_thread_send` to the manager's thread id (it's in your brief and in `scripts/bk log --tag manager`). Messages carry Ground ids, not content.
- Never message the owner. In your own thread, write only "Reported to manager."
- **Any approval comes from the manager**, never the owner: a `question` claim related to your task, then send its id. The manager answers, or takes it to the owner itself.
- **Ask instead of guessing** when the spec is ambiguous, two reasonable designs exist, or an assumption would be hard to undo. Contribute a claim `--tag question` related to your task, then send the manager its id.

## Your brief

`scripts/bk show <task id>` holds it:

| Field | What it means for you |
|---|---|
| Identity | Sign everything with `--as builder-<branch>` |
| Objective | The outcome to reach, not the steps |
| Read first | Files, docs and Ground ids to read before starting |
| Class and branch | The change class decides your review path; the branch is already created for you |
| Out of scope | Don't touch it, even if it looks wrong. Contribute a `note` instead |
| Done when | The checks you must show evidence for |
| Stop and report if | Limits. When you hit one, stop and report; don't push past it |
| Report | When to report, and anything extra to include |

## Contributing to the Ground

Always use `scripts/bk`, never a `bracken` binary built from your branch. The table of what to contribute is in [AGENTS.md](../AGENTS.md#the-ground). As a builder:

- **As you learn:** a `note` for anything another agent would want (an interface choice, a gotcha, a dead end), related to your task.
- **Each pushed head:** a `pr` claim (`PR #<n> <branch> @ <full sha>: <title>`), `relate <pr> implements <task>`, and for a later head add `--supersedes <old>` to the new `pr` claim.
- **Each test run:** `"$main/scripts/bk-evidence" <pr> --as builder-<branch> -- <command>` from your committed worktree, where `main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")`: the main checkout's copy, never your branch's (it may differ, and it signs with the team's Ground). It runs the command and records `pass` or `fail` on the `pr` claim with the command and HEAD sha in `--method` and the real exit code and output tail in `--details`, then exits with the command's code. For a pipeline, use `bash -o pipefail -c '…'`, or only the last stage's exit code counts. Its output becomes a permanent contribution, so don't use it for commands whose output can hold secrets, and don't hand-build `--details`. It refuses, recording nothing, when:
  - the claim isn't a hex id of a claim starting `PR #<n> <branch> @ <HEAD's full sha>:` (pass `--any-head` only for a claim that isn't a `pr` claim; it refuses `pr` claims);
  - the identity is missing or this replica can't sign as it;
  - you're outside a git checkout, the checkout isn't clean before and after the run, or the command moved HEAD;
  - the command isn't an executable file: a shell builtin such as `true` or `echo` counts, so pass its path (`/usr/bin/true`) or wrap it in `bash -c '…'`;
  - the pinned `bracken` is too old to read details from a file: ask the manager to upgrade it.

  Your own evidence is recorded but never counts toward the merge gate; the manager re-runs the checks. So run only the packages [AGENTS.md](../AGENTS.md#tests) lists for what your branch changes, never the whole workspace's tests.
- **A test you think is wrong:** never delete, skip or weaken it. Contribute a claim that `contradicts` it, with your reasoning.
- **Review findings:** answer each confirmed finding by fixing it in a new head, proposing a deferral (a `question` claim saying what would fix it and when; if the manager agrees, it records the deferred task itself), or disputing it (a claim that `contradicts` it).

## Git

- Merge `main` into your branch only when it conflicts with `main` (check with `git fetch origin && git merge-tree --write-tree HEAD origin/main`). The manager tests your head merged with `main`'s tip (process steps 5 and 8), so keeping up isn't needed. Never rebase or force-push someone else's branch; force-push only your own, with `--force-with-lease`.
- `git stash` is shared across worktrees; use a WIP commit.
- Your own worktree is the manager's to remove, after your PR merges. Any extra worktree you add (a clean copy, a scratch check) is yours. Put it, and any other scratch, under the team's scratch root, `$TMPDIR/bracken-scratch/<task id>` (`mkdir -p` it first), and remove it on every exit, whether the work passed, failed or stopped, with the checks in process section 6, Worktrees (`git worktree remove <path>`, without `--force` unless you've looked and the files are disposable). `scripts/cleanup` clears what's left there after 2 hours unused, but don't count on it: a dirty checkout stays until someone looks. The `rm` command is refused in Bash here (development.md, Harness), only to avoid approval prompts; it is no guard against deletion. Use `git rm` or `trash <path>`.
- Never stage `.bracken/`. Your worktree's copy is a stale snapshot.
- Open the PR with `fj --host forge.jx.rs pr create --base main --head <branch> --body "…" "<title>"`. The body has the change class, what wasn't tested, the release steps (usually "none"), and the task id. It doesn't carry the `pr` claim id (the claim needs the PR number first); reviewers find it with `scripts/bk log --tag pr --current`.

## Final report

Contribute a claim `--tag report` and `relate <report> implements <task>`, then send the manager its id. It contains:

1. PR URL, head commit, and the current `pr` claim id.
2. What changed and why.
3. **Evidence ids for each done-when check.** Ids of evidence with the command and output, not "tests pass".
4. What wasn't tested.
5. Any test you think is wrong, with the id of your contradicting claim.
6. What you need from the manager.
7. Process suggestions, each as its own claim `--tag process`, listed by id.
