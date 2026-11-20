# Releases

## Stages

Nothing is released yet. Every merge to `main` runs checks only and deploys nothing.

| Stage | What it is | Status |
|---|---|---|
| `main` | Merged once the Ground's merge gate passed (GitHub Actions CI observes every push and PR, and doesn't gate) | In use |
| Tagged pre-release | A `v<version>-pre.<n>` tag on a `main` commit | `TODO(owner)` when there's something to hand out |
| Published crates | `manyroot` and `bracken` on crates.io, under `MIT OR Apache-2.0` | `TODO(owner)` |

Each step past `main` needs the owner's go, recorded in the Ground as a claim `--tag approval`.

## Release path by change class

The classes and the risky ones are defined in [development.md](development.md). Any PR the manager authored also needs the independent review (process section 4.2).

| Class | Review | Extra steps |
|---|---|---|
| docs, process | security | none |
| cli, library | security | upgrade the pinned binary afterwards (see below) |
| merge-gate tooling: all of `scripts/`, `.github/` | security, independent | the change runs its own checks on the PR before it merges |
| crypto | security, independent | none |
| protocol, storage | security, independent | the manager's go, logged as `owner-review`; a migration note below; upgrade the pinned binary afterwards |

**After merging task bfc942cd7ced** (research crates move to `research/`; `scripts/code-scope`, `bracken-scope` and the light run are gone; owner decision `c8bbb2886be3`, superseding `464d3ccca45e` and `73f2ba4c08e5`): no binary upgrade, and no protocol or format change. The manager re-pins main's Weave policy right after the merge, since `scripts/test-table`'s hash, which the policy pins, changed: a new version of line main's policy through `scripts/weave line policy main`, citing the owner's decision (`scripts/weave-pin` is the Ground's pin, which doesn't change): until then the Weave gate can't count a head, and every head takes git's counted run. `scripts/verify-seq --light` and `scripts/test-table --split -p`, `--light` and `--remote -p` are refused, and `scripts/merge` counts no light pass, so a head whose only counted pass is light needs the full run again. `scripts/tree-scope key` now leaves out everything under `docs/` but keeps `docs/protocol.md` and `docs/design/weave-freeze.md`, so a tree key recorded before the merge can differ from the same tree's key after it: a `--reuse` or batch pass across the merge fails closed and needs a fresh run. Old worktrees' `target/` hold stale `bracken-scope`, `bracken-demo` and `bracken-experiment` builds that `scripts/cleanup` clears as usual.

**After merging task 3b96803d923c** (`scripts/weave-propose` reads gh's JSON apart from its stderr, and `scripts/weave-gate` reads `weave green`'s the same way): no binary upgrade. Builders run the main checkout's copy, so the fix takes effect when the main checkout is on the merge. Until then, with gh 2.105.0 or later, `weave-propose` refuses every PR as a fork (gotcha `d532678d900c`): say so in the `pr` claim, and the merge falls back to the import. `weave-gate` still refuses when `weave green` warns on stderr that a record doesn't decode.

## Needs the owner's go

The owner delegated everything else to the manager (decision `adcb78b88c6c`), including protocol-class merges and pinned-binary upgrades, which the manager logs as `owner-review` claims. Still the owner's go:

- Publishing crates; changing the repository's visibility; licence changes.
- Granting access to `wrnu/manyroot`; changing branch protection.
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

**Upgrade after merging task f5231b1cde7f** (`log --tag` compact by default, `--since <date>`, `log --open`). AGENTS.md and the process point to these as soon as it merges, but `scripts/bk` runs the pinned binary, so upgrade it right after the merge. Until then, add `--compact` to `log --tag` reads and use `--limit N`; `--open` and a date for `--since` fail on the old binary. Anything that read `log --tag`'s full text lines needs `--full` now (`--json` is unchanged).

**Upgrade after merging task d4e958fd6719** (labelled ids from every write, `--json` on writes). `scripts/bk-evidence` and `scripts/merge` read either form, so they work before and after the upgrade. Until it, writes print bare ids, the claim's first and then its relation's, one per line.

**Upgrade after merging task e75a246bf541** (`evidence --details-file`). From that merge, `scripts/bk-evidence` passes the output tail through a file, and it refuses to run, naming this step, until the pinned binary has `--details-file`. Upgrade right after the merge. Until then, wait for the upgrade (it takes minutes) rather than record evidence by hand, which would put the output back on a command line and lose the real exit code.

**Upgrade after merging task dea48fd16754** (`findings <claim>`, the merge gate's findings check as one CLI read). `scripts/merge` kept its own copy of the check until the pin had `findings`; a second PR, #205, then switched it to read `findings` (answer `945b6e49fa35`). From that switch on, `scripts/merge` refuses to decide its findings check on a pin without `findings` (its read fails), so a rollback of the pin past that merge needs the switch reverted too.

**Upgrade after merging task ac538d60e8f9** (the verified snapshot, owner decision `96e282819607`). Every read then goes through `.bracken/cache/snapshot` (docs/cli.md, The verified snapshot), about nine times less CPU at this size (1.0 s to 0.11 s per read on 9,708 contributions). Nothing in the Ground changes and no migration is needed: an older binary never looks in `cache/`, which ignores itself in git and is in `.gitignore`, and the first read after the upgrade builds the snapshot with a full check. The trade: a cached object overwritten or made unreadable on disk later isn't noticed by a cached read (a deleted one, or one replaced by a link or a directory, is). `scripts/merge`, `scripts/verify-seq` and `scripts/ground-guards` set `BRACKEN_FULL_CHECK=1` on their own Ground reads from this merge on, never for the tests they run, and `scripts/test-table` clears it before running cargo; an older pin ignores it and checks everything anyway, so they keep full checks before and after the upgrade.

## Protocol and storage migrations

Each protocol or storage change adds an entry here: what changed, whether existing Grounds still open, and if not, how to carry them forward. Re-encoding a contribution changes its id, so it has to be signed again, and that needs its author's private key. Without the keys, old contributions stay readable only by a binary that still understands the old format. So a protocol change either keeps reading the old format, or migrates while every key still exists.

The protocol guard (`crates/manyroot/tests/protocol_guard.rs`, see "Protocol guard" in [protocol.md](protocol.md)) fails on changes to what it pins (listed there). Updating its pinned values or rebuilding its fixture Ground is part of the protocol change, never a way to make the tests pass: it needs the same review and go (the manager's, logged as `owner-review`), and the entry here names the fixture that keeps the old format.

### 2026-11-19: the store fails closed on a bad agent directory, identity file or object listing (task c0eb61747345)

Neon's recommendation 2 (decision `c8bbb2886be3`, lead `0fe1e087ae37`). Storage hardening only: no change to
the encoding, ids, the signing context, the validity rules, bundles, sync, or any golden vector or fixture.
`Contribution::encode` gives the same bytes; it borrows the content instead of copying it, and each
contribution keeps its encoded length (`Contribution::encoded_len`), which `Graph::check`, sync's batching and
`bracken serve`'s quotas read instead of encoding again.
- **`Store::agents` fails closed** (`StoreError::Corrupt`) on an `agents/` entry that isn't a regular file
  named `<name>.json` for a valid name, whose `name` isn't its file's, a second entry for a name or a key, or a
  local identity whose key the directory lists under another name. Names starting with a dot are skipped.
  Every command that shows names reads it, and `bracken verify` now does too. `Store::open` still doesn't.
  `bracken view` reads the directory alone through `Store::directory`, with the same rules and without
  `identities/`, so it refuses such a directory as the CLI does where it used to pick or drop a name.
- **Identity files** (`Identity::load`, `Store::identities`) must be regular files named `<name>.json` for the
  valid name they hold.
- **Writes**: an object's temporary file has a random name, is synced before the rename and is removed if the
  write fails. `ground.json` and a new `agents/<name>.json` are written to a synced temporary file and linked
  into place, so neither follows a planted symlink, replaces a file or is left half-written.
- **Opening** refuses, as `Corrupt`, a shard of `objects/` that is a symlink or not a directory, any other
  entry there not named with a dot, and an object file that isn't a regular file, in `Store::open` and in the
  untrusted objects of `Store::open_trusting`. Before, a symlinked shard or object was followed and stray files
  were skipped.
- **No symlink is followed inside a replica** (round 2, findings `58b9903c1820`, `99f862b78057`,
  `23253fd20e5b`). `agents/`, `identities/`, `objects/`, every shard, `ground.json` and a `--key-dir` that is a
  symlink are refused on every read and write (`Corrupt`); only the replica's own directory is followed. Every
  directory under it is opened with `O_DIRECTORY | O_NOFOLLOW` and every file with `O_NOFOLLOW | O_NONBLOCK`,
  relative to its directory's descriptor, and a file's type is checked on the open descriptor, so a link or a
  FIFO swapped in after a listing is refused rather than followed or blocked on (`crates/manyroot/src/fsx.rs`,
  through `rustix`, which `crates/tree` already used). `Identity::save` writes a 0600 dot-named temporary file
  (no `.json`), synced, then hard-links it in place, so a key is never seen half-written. None of this
  repository's setups (the main Ground, `~/bracken-weave/manyroot`, the team server's `ground/` and `keys/`,
  worktree copies) uses a symlinked directory or `ground.json`.
- **Existing Grounds open unchanged** if they hold none of these. This repository's own `agents/`,
  `identities/` names and `objects/` were checked against a copy before the merge and pass. A replica that
  holds one now fails with an error naming the path: fix or remove that entry.
- **No migration.** Upgrade the pinned binary after the merge.

### 2026-11-19: Weave readers bound an unreadable record's cost, and a line name never reads as an id first (task fe2c9d96a787)

Two fixes from the hostile-writer review (task `ee121849975b`), accepted by the owner (decision
`a2e0b9c241c7`). Reader changes only: no format, encoding, id, golden vector or fixture changes, and the same
records are valid, count and are unreadable as before. Every existing Ground opens and reads as before.
- **A record of a version this build doesn't read costs about its own size** (findings `3832a3c04d2d`,
  `8b99e0d6d326`). Every command decoded each Weave record as its type, building every field at many times
  its size, and for a line or decision artifact that doesn't decode, G8 then read its top-level `v` and
  `line` by decoding the whole payload into a generic CBOR value, up to three times. So one 16 MB artifact
  by any writer, padded under an unknown key or inside a field of its type, cost every Weave command about
  1 GB and seconds. Now each record's `v` and `line` are read first, once, walking past the other values
  without building them (`PayloadHead` in `crates/tree/src/format.rs`), and only a record whose `v` is 1 is
  decoded as its type (`decode_bounded`, in `Index::build` and `Lines::build`). Only version 1 is ever
  valid, so this changes no record's validity, and the head reads what the generic value held, within the
  same bounds (256 levels of nesting): the refusals and warnings name the same records, lines and versions.
  **Still paid:** a record whose `v` is 1 is decoded as its type, as every valid record is, so a malformed
  version 1 padded inside its fields still costs a reader about what a valid record of that size would
  (the frozen format bounds neither). `weave check` now names the version of a record whose `v` isn't 1
  (`payload has format version 2; ...`), or says it has none, in place of the decoder's error.
- **A line name never reads as an id first** (finding `2bb70bcae1d2`, task `3b9ab8a4a918`). `--line`,
  `--into`, `line policy <line>` and `line merge <line>` tried a text as an id prefix before looking for a
  line of that name, so a writer who wrote a contribution whose id starts with a hex-like name of one of
  the root's lines could take the name over or stop it. Now a name reaches the counted marker's author's
  lines first, and a text is read as an id prefix only when none of them has that name; an id that a
  name of theirs shadows takes a longer prefix. `weave line create` and `weave line policy --name` refuse
  a name of 4 or more hex digits, of either case, writing nothing. `weave check` lists a line of the
  marker's author's with such a name as `name_reads_as_id`, so it exits 1 while one exists.
- **Old and new binaries.** They agree wherever no line of the root's has a hex-like name, which is every
  Ground we hold today (the finding found none), and an older binary pays the old cost for an unreadable
  record of another version, but counts the same ones. **Migration:** none for the records; upgrade the pinned `.bracken-bin`
  and `~/bracken-weave`'s binary after the merge, as for any CLI change. After the upgrade, the manager
  checks that `scripts/weave weave check --json` lists no `name_reads_as_id`; if one appears, reach that
  line by its id, since an older binary reads its name as an id first.

### 2026-11-19: the gate's scripts hold against another writer (task ab5acf6b2ae5)

Hostile-writer fixes, PR 1, scripts only (decision `a2e0b9c241c7`): no record's bytes, validity rule, format
or fixture changes, every Ground opens and reads as before, and the pinned binary needn't change. What
changes is what the merge gate's scripts accept (docs/design/weave-freeze.md 2.5, the last four rows).
- **Supersedes.** `scripts/merge` (the pr claim, a batch claim, an owner-review, the owner's approvals) and
  `scripts/weave-gate`'s switch count another identity's supersede only when manager or owner wrote it;
  anyone else's is ignored and named (finding `20a7646f3579`). A takeover's newer pr claim needs the
  manager's supersede of the old one to stop it counting.
- **What Weave leaves out.** `scripts/weave-gate` takes git's counted run when git's merge holds a submodule,
  or a root entry other than `.bracken/` that could fold to start with `.bracken` (finding `81648e74e519`).
- **Timeouts.** Every Weave read the gate makes runs under `timeout` or `gtimeout`, `WEAVE_GATE_TIMEOUT`
  seconds (120 by default), and fails closed when it runs out; with neither tool on `PATH` the gate doesn't
  run (report `0b5600268856`).
- **Objections.** `scripts/weave-shadow` answers, once, `overridden`, discussion objections from identities
  outside line main's policy (finding `598258717737`). It reads the policy with a new read through
  `scripts/weave`: exactly `weave line policy <line> --dry-run` with no `--owner-decision`, given a fixed
  all-zero placeholder that never reaches a record (answer `f47f2f255c75`).
- **Migration:** none. A supersede of a gate claim that a non-manager wrote no longer stops a merge: the
  manager supersedes a taken-over pr claim itself.

### 2026-11-17: an unreadable Weave record stops only the lines that judge it (task ca6338ee7701)

G8 of docs/design/weave-freeze.md, a reader change shipped before any Weave v2 is written (decisions
`bae92c497714` Q4, `0290487abd90`). No record's bytes, validity rule, golden vector or fixture changes, and
every existing Ground opens and reads as before; what changes is which commands refuse, and what they say.
- **Scoped per line.** A policy version that doesn't decode counts only against the line of the counted
  version it supersedes (in a relation its own author signed, as before). `integrate`, `validate`, `line
  policy`, `line merge` and now `weave green` refuse only when a line they judge holds one: the lines they
  name, the default line, and the lines a decision on those brings in when its author integrates its line
  under the policy in force for it: its parents' lines, decoded or not, and so on through parents whose
  authors integrate theirs, such as another line's decision a line merge took in (keeping finding
  `5d50422b9e11`'s fail-open closed). A parent that doesn't decode brings its line in whoever wrote it,
  since a build that reads it may find its author integrates that line under a policy version this one
  can't read (finding `0783a2caf192`). A decision by anyone else brings no line in, since it can't qualify
  whatever its parents are (finding `c9da29e1ac4b`). So a writer who integrates no line a command judges
  can't stop it: another writer's line no longer stops main, unless main's integrator merges it in. Every
  command judges the default line, so one on main still stops them all.
- **Decisions too.** A decision that doesn't decode counts the same way only when its payload's `v`, read
  as generic CBOR, is a version this build doesn't read (above 1), it isn't retracted, its top-level `line`
  names a counted version in its causal past, and its author is an integrator under the policy in force
  there. Any other, a malformed version 1 included, is ignored and listed by `weave check`, as before
  (finding `9c0ae448ee12`). `line policy` refuses only for a policy version, never for a decision, which
  changes no policy: so the line's author can still remove an integrator who wrote one. That doesn't lift
  the stop: the decision is judged under the policy in its own past, which still names its author, so the
  line's other commands refuse until bracken reads its version or its author retracts it. Any integrator
  of a line a command judges, main's included, can stop it this way, and removing them doesn't end it.
- **The refusal names the version.** A payload's `v` is read first as generic CBOR, so a version 2 with
  other fields says `payload has format version 2; this build reads version 1` instead of `not in canonical
  encoding`. `weave check` still prints the decoder's own error.
- **`weave green` refuses** where it warned, and `scripts/weave-gate` refuses explicitly (`line main's
  judgement holds a record this binary can't read`) when `weave head --line main` warns of one, in place of
  the refusal it got by accident from that warning breaking green's JSON (gotcha `098c2193c7ab`).
- **Old and new binaries.** Nothing on our Grounds triggers either rule today, so they agree. An older
  binary refuses more, Ground-wide, and warns on green instead of refusing; the gate's check matches its
  warning too, so the gate fails closed across the upgrade. **Migration:** upgrade both binaries that read
  the Weave Ground (the pinned `.bracken-bin` and `~/bracken-weave`'s) after the merge, as for any CLI
  change, and in any case before a v2 record is written (weave-freeze.md section 3, step 2); never roll
  back below this release once a v2 exists.

### 2026-11-17: a Weave line name reaches only the counted marker's author's lines (task b4a9456b4e88)

Gap G9 of docs/design/weave-freeze.md (section 2.4). A reader change only: no format, encoding, id, golden
vector or fixture changes, and the same records are valid and count as before. What changes is which line
a **name** names. `--line <name>`, `--into`, `line policy <name>` and `line merge <name>` now resolve a name
only among the current lines whose first policy version is by the counted marker's author (the root, since
option A), the author the default-line rule already uses. Before, a name named every line of that name, so
any writer could stop `--line main` readers by opening a second line called `main` (2.5, the duplicate
line name).
- **What readers see.** A line anyone else opens is reached by its id only, and isn't judged when a
  command names its name. Two lines of one name by the marker's author still make the name refused as
  shared. A name only others' lines carry is refused, listing their ids. With no counted marker, no name
  reaches a line. New: `weave line list [--json]` lists every current line with its author, marking the
  default line and each line a name doesn't reach (`reach`: `name`, `name_shared` or `id_only`).
  `weave check` lists a line by anyone else that shares a name with one of the marker's author's lines as
  `name_by_id_only`, so it exits 1 while one exists.
- **`scripts/weave-gate`.** Its `taken_over` check is unchanged: it still refuses when `weave head --line
  main` doesn't name the default line. A second `main` opened by someone else no longer makes that happen,
  so it's no longer a stop at the gate; a second `main` by the root's key holder still is.
- **Old and new binaries.** They agree on every Ground where each name is carried by lines of one author,
  the marker's. A line opened by anyone else is reached by its id only on a newer binary, and by its name
  too on an older one, which also refuses a name such a line shares with one of the marker's author's. Nothing to migrate:
  upgrade the pinned binary and `~/bracken-weave`'s. After the upgrade, the manager checks with
  `scripts/weave weave line list` that every line a script names by name (today only `main`) is marked
  `name`, and that `scripts/weave weave head --line main` names the same line as `scripts/weave weave head`.

### 2026-11-11: only the Weave Ground's root's marker counts (task 8ffd4b252d9a)

Option A of docs/design/weave-marker.md (owner decision `d01c83b654d1`), closing the forged-marker
takeover (finding `26d9361e0a9f`). A Weave reader is given the Ground's **root**, one AgentId, and only a
marker that root signed can count; among the root's markers, the old rule picks (causally first, smallest
id among concurrent ones). `Index::build` takes the root, and the ancestry used for counting covers the
root's markers only, so a marker someone else placed earlier can't keep the root's from counting. No
record's bytes change: the formats, ids and the weave-v1 fixture's pinned values are as they were.
- **Where the root comes from.** A replica's `weave-root` file beside `ground.json`. `weave init` writes it
  for its signer; `weave root set <AgentId> [--replace <current>]` sets it. A replica without one has no
  counted marker: no default line, and every `bracken weave` write refuses. In this repository,
  `scripts/weave`, `weave-gate` and `weave-shadow` also check the Weave Ground's id and the replica's root
  against the committed pin `.config/weave-root`, read from `refs/heads/main` (`scripts/weave-pin`), and
  refuse on any mismatch; `scripts/weave` lets through only `weave root set <the pinned root>`.
- **Old and new binaries.** On every Ground we hold, the rules agree: `~/bracken-weave/manyroot` and its
  backups have one marker, by `weave-integrator`, the pinned root. They differ only on a Ground holding a
  marker by someone else, where an older binary still counts it the old way. `weave check` now prints the
  root and lists a marker not by it as `not_by_root`; `--json` adds `ground` and `root`.
- **Migration: rollout step 2, the manager's, right after the merge.** Upgrade both binaries that read the
  Weave Ground (the pinned `.bracken-bin` and `~/bracken-weave`'s). Until the next step every script that
  reads the Weave Ground refuses, so Weave-gated merges fall back to git's counted run, and no merge is
  shadowed or backed up. Then run `scripts/weave weave root set
  6bc92b401e36be06cf432a65cb2cbd7cc765c56b6ad40a3c0c5ffb9c24c6298b`, never by hand; `scripts/weave` checks
  it wrote the pin. Check that `scripts/weave weave check` names the same marker (`488c40418f98`) and no
  problem, and `scripts/weave weave head --line main` the same line; then shadow this merge. A backup made
  before this step lacks `weave-root`; once put back at `~/bracken-weave/manyroot` (by the manager, with the
  owner's go: no script restores one), the same command sets it.

### 2026-11-04: signing through a trait, and a server that signs (task 9fdcbbc35806)

Part A step 2 of docs/design/agent-keys.md. In `manyroot`, a contribution can be signed through the
`Signer` trait (`Contribution::create_with`, `Store::contribute_with`), which `Identity` implements, and
`ContributionContent` gains `encode` and a canonical-only `decode`. No byte changes: the encoding, ids, the
signing context and the validity rules are as they were, the protocol guard's vectors and fixture Ground
are unchanged, and every existing Ground opens. New in the CLI: `bracken serve --keys/--scopes/--signed`
and its `POST /v1/<ground>/sign` request, `bracken id new --key-dir`, and `BRACKEN_SIGN_URL` and
`BRACKEN_SIGN_TOKEN_FILE` for writes (docs/cli.md, Keys a server holds). An older binary can't sign for a
server-held identity, but reads and verifies its contributions like any other. Nothing to migrate:
existing identities keep their key files and sign from them. Upgrade the pinned binary before
`scripts/team-server key` is used, since `scripts/bk` then needs it to sign through the server.

### 2026-10-31: Weave line policies gain executors

Task `7fbf11b1a6a5` (plan `cc496dcee0e3`, part one). A line policy has an
optional `executors` field, naming the checks that run in a box or on the
Mac through a pinned runner script.

- **Existing Grounds open unchanged.** The field is left out when empty, so
  every existing line version, and the `weave-v1` fixture, encodes and
  hashes as before.
- **Upgrade every reader first, and never roll back.** A line version with
  executors doesn't decode with a build from before this change, and such a
  build skips it silently: it reads the line under the version before, with
  no executors, and could count results or move the line under a policy no
  longer in force (finding `5d50422b9e11`). So before anyone writes a policy
  with executors, every binary that reads a Weave Ground, the pinned
  `.bracken-bin` one and `~/bracken-weave`'s included, must be at least this
  version, and none is ever reinstalled below it, the rollback above
  included. From this version on, a line version that doesn't decode is
  reported, and `validate`, `integrate`, `line policy` and `line merge`
  refuse until the binary is upgraded.

### 2026-10-31: Weave merges text files line by line

Task `7cbd0a0ce7d0` (design `68b57f9aed6d`, answers `c8a511445626` and `55e0a67c68a2`). Not a format change: no record, field or validity rule changes, and views never recompute a merge, so every existing Weave Ground opens and means what it did.

- **What changed:** `weave line merge` and `weave integrate` merge a conflicting file line by line when its three versions are text (docs/design/weave.md, *Per-file merge*).
- **Merges already recorded:** none needs migrating. An integrator's resolution of a conflict's key still comes first, so each recomputes the same tree. A new merge of the same heads may now succeed where one fell back or stopped before.
- **Bounded:** a file merges line by line only if each side's diff leaves at most 10,000 lines of the base and of the side once their common first and last lines are set aside, and the merge fits one file version. Over either, it stays a per-file conflict, as before (findings `933f535c9476` and `d189e282cdee`, answer `47a1c1088fd6`).
- **New dependency:** imara-diff 0.2.0, licensed Apache-2.0 only, not MIT OR Apache-2.0 like this repository. It can be distributed with this repository under either of its licences, but anything that ships imara-diff, such as a built `bracken` binary, must carry a copy of the Apache-2.0 licence for it (its section 4; the crate has no NOTICE file). Nothing is published yet, so there is nothing to do now; publishing needs the owner's go anyway. Its dependencies, hashbrown, foldhash and memchr, were already in `Cargo.lock`.
- **Pinned:** imara-diff, exactly. The format guard's line-merge vectors and its oracle's pinned list of cases where git differs fail on any change to the crate or the algorithm. Updating them is a protocol-class change, never a way to make the tests pass.

### 2026-10-31: Weave's payload formats are pinned

Task `3be1b325a43e` (Weave build step 8, docs/design/weave.md section 5).
No format changes: this pins the formats Weave already writes, so that a
later change to them is caught.

- **Existing Grounds open unchanged.** No encoding, id, signing context,
  validity rule or body kind changed, in the coordination Ground or in
  Weave's records.
- **Pinned:** Weave's seven media types (file, manifest, marker, line,
  decision, proposal, resolution) are pinned twice:
  - by the golden vectors in `crates/tree/tests/format_guard.rs`, one record
    of each type with its bytes and content hash;
  - by the fixture Weave Ground `crates/tree/tests/fixtures/weave-v1`, which
    `crates/tree/tests/weave_fixture.rs` opens. It checks every id, every
    record's bytes, and every derived status: qualification, heads, green,
    counted results, reviews, goals and open objections.
- **The coordination Ground with Weave linked in:**
  - `crates/tree/tests/coordination_fixture.rs` opens the coordination
    fixture `ground-v1` from the library;
  - `crates/bracken/tests/coordination_fixture.rs` opens it through the
    binary.

  Both compare every status, the accepted layer and its conflicts with
  `ground-v1-acceptance.json`, under one pass and two. The binary's test also
  checks that `weave check` finds nothing and that reading writes nothing.
- **A later change to a Weave format, or to how the fixture reads, is
  protocol class:** an independent review, the manager's go logged as
  `owner-review`, and an entry here naming what changed. Rebuilding
  `weave-v1` (its ignored `write_weave_v1_fixture` test) or editing a pinned
  value is part of that change, never a way to make a test pass. Add new
  cases to the scenario's end, so that the existing ids stay as they are.

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
