# Agent guide

The one doc agents follow. Live settings: [docs/development.md](docs/development.md); each script's header is its manual; `docs/archive/` is history.

## 1. Safety and frozen boundaries

- **Protocol and Weave's formats.** A change to encoding, ids, the signing context or validity rules ([docs/protocol.md](docs/protocol.md), `crates/manyroot/src/{contribution,graph,id,identity}.rs`) or to anything in `crates/tree/src/` ([weave-freeze.md](docs/design/weave-freeze.md)) can make every Ground unreadable, this repo's included. It needs an independent review, the manager's `owner-review` claim naming the pr claim and head, and a migration note in `docs/releases.md`; a change to a frozen format (a golden vector, a pinned fixture value, `crates/tree/tests/fixtures/`) also needs the owner's own approval. `scripts/merge` checks this.
- **Keys.** Never commit `.bracken/identities/` (unencrypted private keys); if git shows a file there staged or tracked, stop and tell the manager. Server-held keys never leave the team server's key directory.
- **The Ground.** Never edit or delete files under `.bracken/` by hand: write only through `scripts/bk`. Only the manager commits `.bracken/`, only with `scripts/ground-checkpoint`.
- **The Weave Ground**, `~/bracken-weave/manyroot`, is reached only through `scripts/weave`. Never edit it or copy its `identities/`; only the manager runs `scripts/weave-backup`, to the destination the owner approved.
- **Your own binary** gets a throwaway Ground: `BRACKEN_DIR=$(mktemp -d)/g target/debug/bracken init`; `scripts/bk` uses the pinned one and the main Ground. Never stage `.bracken/` or sign as another identity.
- **No secrets or personal data** in the Ground, PRs or briefs: the Ground is public and permanent (decision `473d5df9b1ab`).
- **The owner's own approval** (`scripts/owner-approve <id> "<what>"`, run by the owner in a terminal) is needed only for: a frozen-format change as above, or the join cutover; model runs or spending beyond an approved budget, a new paid service or a schedule; which models or providers agents run on; a new repository; repo visibility, licence, branch protection, access or publishing; destructive history, data or key actions (force-pushing `main`, deleting or rewriting Ground objects, moving root or owner keys). Everything else is a manager `decision` claim. Earlier approvals, `--delegated` ones included, stay as records. Nothing deploys on merge.

## 2. Repo and code conventions

- Rust 2024; `rustfmt.toml` (width 120). `thiserror` in `manyroot`, `anyhow` in the CLI. Outside tests, `expect("why it can't fail")`, never a bare `unwrap()`.
- Public APIs use plain names (`contribution`, `causal heads`, `evidence`); subsystem names (Seed, Roots, Proof…) only in docs and module comments. Nothing in `manyroot` may assume a contribution is code. Every CLI read command keeps a `--json` form.
- No AI attribution in commits or PRs. GitHub is `wrnu/manyroot`, through `gh`; the old Forge is the read-only remote `forge`.
- zsh doesn't word-split `$var`: write commands out. Start with `PATH="$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:$PATH"`.
- No `rm` in Bash (a hook refuses it): use `git worktree remove`, `git rm` or `trash`. Never `pkill -f`.
- Scratch goes under `$TMPDIR/bracken-scratch/<task id>`; remove what you made on every exit. `touch <entry>/.keep` keeps an entry 24 hours from `scripts/cleanup`.
- `git stash` is shared across worktrees: use a WIP commit. Never rebase another agent's branch; force-push only your own, with `--force-with-lease`. Merge `main` into your branch only when it conflicts.
- boat.dev boxes: always `--no-env`, in the foreground, deleted when done (`boat list --all`).

## 3. Ground usage

- Run `scripts/bk … --as <you>` with `BRACKEN_AGENT=<you>`; read commands take no `--as`. A write `scripts/bk` doesn't confirm isn't done.
- **At task start**, read whole: `scripts/bk log --tag decision --current`, `--tag gotcha --current`, `--tag preference --current` (and `--tag reference --current` when relevant), then `scripts/bk show <task id>`. `--since`/`--limit` are for follow-ups. Open findings: `scripts/bk log --open`.
- Check a claim with `scripts/bk show <id>` before acting on it; a `note` is a lead. Contribute what others need **as you learn it**. Messages carry ids, the Ground the content.
- A write prints one labelled line per record, primary first (`claim <id>`, then `relation <id> (supersedes)`): pass on the id you mean.

| What | How |
|---|---|
| Decision | claim `--tag decision` (manager); an owner's decision quotes them, dated |
| Task | claim `--tag task` holding the brief (manager) |
| Note, discovery, dead end | claim `--tag note`, related to the task |
| Question to the manager | claim `--tag question`, related to the task; the answer `refines` it |
| PR head | claim `--tag pr`: `PR #<n> <branch> @ <full sha>: <title>`, ending `Weave proposal <64-hex id>`; `relate <pr> implements <task>`; a new head is a new claim `--supersedes <old>` |
| Test run | `"$main/scripts/bk-evidence" <pr> --as <you> -- <command>` (section 4) |
| Review finding | claim `--tag finding`, then `relate <finding> contradicts <pr>` (section 5) |
| Final report | claim `--tag report`, `relate <report> implements <task>` |
| Gotcha, preference, reference | claim with that tag; preferences quote the owner, dated |
| Process idea / experiment | claim `--tag process` / `--tag experiment` (section 6) |
| Gate misfire | claim `--tag misfire`: the gate refused a good head or passed a bad one |
| Disagreement | a claim that `contradicts` it, with your reasoning |

Don't repeat code, git log or docs. To change your entry, `claim … --supersedes <old>`.

## 4. Build, test and merge loop

**Roles.** The owner decides and talks only to the manager. The manager plans, writes tasks, verifies, triages, merges and owns shared resources (merges, the pinned binary, checkpoints, the team server). A builder has one task, worktree, branch and PR, and talks only to the manager (`t3_thread_send`, carrying ids; in its own thread only "Reported to manager."). Reviewers and security are short-lived children writing only findings and evidence. Ask the manager (a `question` claim, then its id) rather than guess: an ambiguous spec, two reasonable designs, a hard-to-undo assumption, any approval. Keep to the brief's scope and stop-and-report limits.

**Builder loop.**
1. Read your brief; if a read says your identity has no team token, stop and tell the manager. Your branch exists.
2. Build and test. Never delete, skip or weaken a test; if one seems wrong, contribute a claim that `contradicts` it.
3. Commit, push, `gh pr create --repo wrnu/manyroot --base main --head <branch> --title … --body …` (class, what wasn't tested, release steps, task id). Then `"$main/scripts/weave-propose" --pr <n>` (if it refuses, say why in the pr claim), then the `pr` claim and `implements`.
4. Record each check below on the exact head with `"$main/scripts/bk-evidence" <pr> --as <you> -- <command>`, the main checkout's copy (`main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")`), from a clean checkout, one call per command. Pipelines: `bash -o pipefail -c '…'`; builtins: their path. No `recorded pass|fail <id>` line means nothing was recorded. Your evidence never counts toward the gate.
5. Answer each confirmed finding: a fix in a new head, a deferral proposed as a `question`, or a `contradicts` claim.
6. Final report (claim `--tag report`, `implements` the task, id sent to the manager): PR URL, head and current `pr` claim; what changed and why; evidence ids per done-when check; what wasn't tested; tests you think wrong (with your claim's id); what you need; process ideas as separate `process` claims.

**Builder checks**, each recorded (experiment `08fc19239548`: no local nextest pass; the box runs the whole table, slow tests included):
```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
scripts/test-table --remote
```
A head changing `research/` (a sibling workspace `cargo test --workspace` never builds) also records `bash -c 'cd research && cargo test --locked'`, after `cargo build -p bracken` at the root.

**Counted run** (the manager's, on `main` merged with the exact head; GitHub Actions only observes): a head changing only `*.md` and `docs/**` gets fmt and the Ground guards, except `docs/protocol.md` and `docs/design/weave-freeze.md`, which take the full run: fmt, both clippies, `cargo test --workspace` on macOS and Linux, the Ground guards. With the Weave gate on, line main's Weave results on that merged tree can stand in (`scripts/verify-seq --weave`).

**Merge.** `scripts/merge` checks and records the gate: the current `pr` claim accepted under its class's policy (builder, low risk: `--validator manager --validator security --min-passes 2`; builder, risky class: add `--validator reviewer`, `--min-passes 3`; manager-authored: `--validator reviewer --validator security --min-passes 2`), every finding triaged and every confirmed one fixed or deferred, the remote head equal to the claimed sha, the counted run, the Ground guards, the freeze's checks, and for a head touching `research/` the manager's research Docker run (development.md). Before running a PR's code, the manager reads its build and dependency changes. After a merge the manager pulls only with `scripts/ground-checkpoint` and `git merge --no-edit <the sha it names>`, upgrades the pinned binary if `crates/bracken` or `crates/manyroot` changed ([releases.md](docs/releases.md)), and removes finished worktrees (clean, nothing unpushed).

## 5. Review standard

**Risky classes:** protocol, storage (`store.rs`, `bundle.rs`), crypto, merge-gate tooling (`scripts/`, `.github/`, the pinned-binary procedure). They, and every manager-authored PR, get an independent review on Gemini 4 Neon (High) as `reviewer`; every PR gets a security review on Claude as `security` (decision `b1b56d75221b`).

- **Briefs** open with three layers: PROJECT (the paragraph in development.md), ARC (the epic and what this PR gives it), TASK (purpose, what must hold, out of scope); then the PR, `pr` claim, class and diff, never the author's conclusions. Each round is a new child, given earlier findings and their handling. At most three rounds; then the manager decides and records why.
- **A finding** is its own claim: file:line, severity, the requirement it breaks, a concrete counterexample (a failing test where possible), labelled confirmed or possible risk; then the verdict, `pass` or `fail`, as evidence on the `pr` claim, `--method "independent review (<model>) @ <sha>"` or `"security review (<model>) @ <sha>"`. Anything without a requirement behind it goes in the verdict's details as a note.
- **Security** checks, always in full: credentials, data flows, access control and workflow triggers, input handling, supply chain, logging, protocol integrity (one encoding per contribution, validity from it and its parents, strict signatures), secrets in contributions. A real risk is always a finding.
- **Triage** (manager): `pass` evidence confirms, `fail` rejects with the reason. Only the manager defers a confirmed finding (a `task`, `deferred`, that `depends_on` it), and only if it doesn't make the PR unsafe.

## 6. Process changes and experiments

- A process change is a claim `--tag experiment` with lines `rule:`, `metric:` (a key `scripts/metrics --json` prints), `baseline:`, `target:` and `expires: <YYYY-MM-DD>` (default 14 days, at most 28). When it expires it lapses (`scripts/merge` names it) unless a manager `decision`, citing the metrics that show the target met, `supports` it.
- **One rule in, one rule out:** an experiment that adds a rule names the rule it retires, and this file stays under 12 KB (a test checks).
- Weekly, the manager records `scripts/metrics`' output: `scripts/bk claim "$(scripts/metrics)" --tag metrics --as <you>`.
- Anyone proposes with a `process` claim; the manager decides.
- Running: `08fc19239548` (builders record no local fast nextest pass; metric `builder_pass_minutes`, until 2026-12-03).
