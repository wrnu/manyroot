# Releases

## Stages

Nothing is released yet. Every merge to `main` runs checks only and deploys nothing.

| Stage | What it is | Status |
|---|---|---|
| `main` | Merged once the Ground's merge gate passed (CI runs only by hand and doesn't gate) | In use |
| Tagged pre-release | A `v<version>-pre.<n>` tag on a `main` commit | `TODO(owner)` when there's something to hand out |
| Published crates | `manyroot` and `bracken` on crates.io or the Forge cargo index, under `MIT OR Apache-2.0` | `TODO(owner)` |

Each step past `main` needs the owner's go, recorded in the Ground as a claim `--tag approval`.

## Release path by change class

The classes and the risky ones are defined in [development.md](development.md). Any PR the manager authored also needs the independent review (process section 4.2).

| Class | Review | Extra steps |
|---|---|---|
| docs, process | security | none |
| cli, library | security | upgrade the pinned binary afterwards (see below) |
| merge-gate tooling: all of `scripts/`, `.forgejo/` | security, independent | the change runs its own checks on the PR before it merges |
| crypto | security, independent | none |
| protocol, storage | security, independent | the manager's go, logged as `owner-review`; a migration note below; upgrade the pinned binary afterwards |

## Needs the owner's go

The owner delegated everything else to the manager (decision `adcb78b88c6c`), including protocol-class merges and pinned-binary upgrades, which the manager logs as `owner-review` claims. Still the owner's go:

- Publishing crates; making the repository public; licence changes.
- Granting access to `Autimo/manyroot`; changing branch protection.
- Deleting or rewriting history or data: force-push, deleting branches others use, removing Ground files.
- Creating, exposing or moving credentials and secrets.
- Spending money; new outside services, infrastructure or schedules; sending data somewhere new.

## Test reports

For a change that can be seen or felt, the manager records evidence on the `pr` claim with `--method "manual test @ <sha>"` and a short report in `--details`: what was run, what was expected, and what happened.

## Upgrading the pinned binary

The team coordinates through `.bracken-bin/bin/bracken`, built from `main`. After a merge that changes the CLI or library:

```sh
# main checkout, on a main pulled through ground-checkpoint (process 9.3), between merges
scripts/ground-checkpoint && git push origin main     # process section 9.3
cargo install --locked --path crates/bracken --root .bracken-bin --force
scripts/bk log > /dev/null && echo "new binary opens the Ground"
```

If the new binary can't open the Ground, reinstall the previous commit's binary (`prev="$TMPDIR/prev-<old sha>"; git worktree add --detach "$prev" <old sha>`, then `cargo install --locked --path "$prev/crates/bracken" --root .bracken-bin --force` and `git worktree remove "$prev"`), and treat the merge as a protocol break.

**Upgrade after merging task c3d2a3297a71** (`log --current`, `claim --supersedes`, `--as` accepted everywhere). The docs and skill use these as soon as it merges, but `scripts/bk` runs the pinned binary, so upgrade it right after the merge. Until then the old recipes still apply: to find the current entry, read `scripts/bk log --tag <tag>`, drop the lines marked `(retracted)` and take the last one; to supersede, contribute the claim, then `relate <new> supersedes <old>`; and leave `--as` off read commands.

**Upgrade after merging task d4e958fd6719** (labelled ids from every write, `--json` on writes). `scripts/bk-evidence` and `scripts/merge` read either form, so they work before and after the upgrade. Until it, writes print bare ids, the claim's first and then its relation's, one per line.

**Upgrade after merging task e75a246bf541** (`evidence --details-file`). From that merge, `scripts/bk-evidence` passes the output tail through a file, and it refuses to run, naming this step, until the pinned binary has `--details-file`. Upgrade right after the merge. Until then, wait for the upgrade (it takes minutes) rather than record evidence by hand, which would put the output back on a command line and lose the real exit code.

## Protocol and storage migrations

Each protocol or storage change adds an entry here: what changed, whether existing Grounds still open, and if not, how to carry them forward. Re-encoding a contribution changes its id, so it has to be signed again, and that needs its author's private key. Without the keys, old contributions stay readable only by a binary that still understands the old format. So a protocol change either keeps reading the old format, or migrates while every key still exists.

The protocol guard (`crates/manyroot/tests/protocol_guard.rs`, see "Protocol guard" in [protocol.md](protocol.md)) fails on changes to what it pins (listed there). Updating its pinned values or rebuilding its fixture Ground is part of the protocol change, never a way to make the tests pass: it needs the same review and go (the manager's, logged as `owner-review`), and the entry here names the fixture that keeps the old format.

### 2026-10-27: supersession counts only when the relation's author wrote the target

Decision `4c55e65bc290` (point 4), answering `3d381dfdb8a9`. A
`supersedes` relation used to count when its author wrote the new
contribution (`from`), so anyone could retire anyone's claim by writing a
replacement. It now counts only when its author wrote the old one (`to`);
the replacement may be anyone's. As before, a supersession holds only while
the replacement is live: if the accepted replacement is retracted, or fails
validation under a policy, the replaced claim is current again. When the
replacement is someone else's, that's now in their hands (or a validator's),
not the replaced claim's author's.

- **Existing Grounds open unchanged.** No validity rule, encoding or id
  changed: only derived state (supersession, `superseded` status, divergence,
  contradictions between current subjects, `log --current`).
- **What changes:** derived state only where a `supersedes` relation's author
  didn't write its target, or wrote its target but not its replacement. On
  this repo's Ground (947 contributions, 60 supersedes relations, every one
  written by the author of both ends), every subject's status, `log
  --current`, `conflicts` and `accepted` are identical before and after.
- **Fixture:** `ground-v1` gained ten contributions appended after the
  original 23, whose ids and bytes are unchanged. A binary from before this
  change opens the extended fixture, but derives different statuses for the
  appended cases.
- **The demo** (`crates/demo`) follows the rule: an agent accepts other
  agents' fixes of its own implementations, and a wip takeover `refines` the
  stale claim instead of superseding it. Demo results from before this
  change, including PR #4's and PR #10's first compare table, count more
  divergences (docs/demo.md).
- **No re-signing or data migration.** Upgrade the pinned binary with the
  owner's go.

### 2026-10-27: sync wire format, version 1

Task `7a85f54c214d`. Adds the sync messages and session (protocol.md,
"Sync"). `bracken sync` between two local directories now runs a session and
exchanges only what each side lacks.

- **Existing Grounds open unchanged.** Contributions, bundles and storage are
  unchanged, so no migration is needed.
- **Pinned in the guard:** one message of every type, then (appended) the
  id-list size of 65 536 ids per `heads`, `offer` or `need` message, the
  `result` bounds (1 024 rejected, 256-byte reasons) and the 44-byte minimum
  `batch_bytes`.

### 2026-10-27: store writes refuse symlinked shards

Task `26a5c735230d`, in sync step 2 (task `57072a4ea13f`). A write refuses
an `objects/` directory or shard that is a symlink or not a directory,
creates its temporary file new, and replaces rather than follows a symlink at
the object's path.

- **Existing Grounds open unchanged.** Reading is unchanged; only a replica
  whose `objects/` or a shard is a symlink stops accepting writes, with an
  error naming the path. Replace the symlink with a real directory.
- **No migration.** Upgrade the pinned binary after the merge.

## Rollback

- A bad merge: revert it on `main` through a PR. Ground contributions it caused stay: they're history. If they're wrong, contribute corrections or retractions.
- A bad pinned binary: reinstall the previous one (above).
- A leaked identity key: tell the owner. Stop using that identity, create a new one under a new name, and record a `decision` saying which key is compromised, from which date. If the key is inside a contribution, the checkpoint keeps stopping until the old identity file is moved out of `.bracken/identities/` (process section 9.3).
