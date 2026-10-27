# Releases

## Stages

Nothing is released yet. Every merge to `main` runs checks only and deploys nothing.

| Stage | What it is | Status |
|---|---|---|
| `main` | Merged once the Ground's merge gate passed (CI observes, it doesn't gate) | In use |
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
| protocol, storage | security, independent | the owner's go; a migration note below; upgrade the pinned binary afterwards |

## Needs the owner's go

- Publishing crates; making the repository public; licence changes.
- Granting other accounts access to `Autimo/manyroot`; changing branch protection.
- Upgrading the pinned binary when the change touches the protocol or storage format.
- Anything that costs money, touches an outside system, or adds a schedule or service.

## Test reports

For a change that can be seen or felt, the manager records evidence on the `pr` claim with `--method "manual test @ <sha>"` and a short report in `--details`: what was run, what was expected, and what happened.

## Upgrading the pinned binary

The team coordinates through `.bracken-bin/bin/bracken`, built from `main`. After a merge that changes the CLI or library:

```sh
# main checkout, on an up-to-date main, between merges
scripts/ground-checkpoint && git push origin main     # process section 9.3
cargo install --locked --path crates/bracken --root .bracken-bin --force
scripts/bk log > /dev/null && echo "new binary opens the Ground"
```

If the new binary can't open the Ground, reinstall the previous commit's binary (`git worktree add /tmp/prev <old sha>`, then `cargo install --path /tmp/prev/crates/bracken --root .bracken-bin --force`), and treat the merge as a protocol break.

**Upgrade after merging task c3d2a3297a71** (`log --current`, `claim --supersedes`, `--as` accepted everywhere). The docs and skill use these as soon as it merges, but `scripts/bk` runs the pinned binary, so upgrade it right after the merge. Until then the old recipes still apply: to find the current entry, read `scripts/bk log --tag <tag>`, drop the lines marked `(retracted)` and take the last one; to supersede, contribute the claim, then `relate <new> supersedes <old>`; and leave `--as` off read commands.

**Upgrade after merging task e75a246bf541** (`evidence --details-file`). From that merge, `scripts/bk-evidence` passes the output tail through a file, and it refuses to run, naming this step, until the pinned binary has `--details-file`. Upgrade right after the merge. Until then, wait for the upgrade (it takes minutes) rather than record evidence by hand, which would put the output back on a command line and lose the real exit code.

## Protocol and storage migrations

Each protocol or storage change adds an entry here: what changed, whether existing Grounds still open, and if not, how to carry them forward. Re-encoding a contribution changes its id, so it has to be signed again, and that needs its author's private key. Without the keys, old contributions stay readable only by a binary that still understands the old format. So a protocol change either keeps reading the old format, or migrates while every key still exists.

The protocol guard (`crates/manyroot/tests/protocol_guard.rs`, see "Protocol guard" in [protocol.md](protocol.md)) fails on changes to what it pins (listed there). Updating its pinned values or rebuilding its fixture Ground is part of the protocol change, never a way to make the tests pass: it needs the same review and owner's go, and the entry here names the fixture that keeps the old format.

_No migrations yet._

## Rollback

- A bad merge: revert it on `main` through a PR. Ground contributions it caused stay: they're history. If they're wrong, contribute corrections or retractions.
- A bad pinned binary: reinstall the previous one (above).
- A leaked identity key: tell the owner. Stop using that identity, create a new one under a new name, and record a `decision` saying which key is compromised, from which date. If the key is inside a contribution, the checkpoint keeps stopping until the old identity file is moved out of `.bracken/identities/` (process section 9.3).
