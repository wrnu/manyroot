# The bracken CLI

Every `bracken` command, with its options and output. The
[README](../README.md) has a ten-line quick start; `bracken <command> --help`
has each command's full options. Every read command keeps a `--json` form.

## Recording work

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"

bracken init --name auth
bracken id new alice && bracken id new val
export BRACKEN_AGENT=alice           # or pass --as on each command

# A write prints one labelled line per record, the primary first: `claim <id>`, `relation <id> (implements)`.
# --json gives {"id": <primary>, "written": [...]}, for scripts.
REQ=$(bracken claim "Sessions need bounded token lifetimes" --tag auth --json | jq -r .id)
# The artifact and its implements relation in one call.
IMPL=$(bracken artifact src/session.rs --media-type text/x-rust --implements $REQ --json | jq -r .id)
bracken evidence $IMPL pass --method "cargo test" --as val
bracken evidence $IMPL fail --method "read it" --depends-on $OTHER --as val  # blocked until $OTHER is accepted
# Or check and record in one step: pass if the command exits 0, else fail.
bracken evidence $IMPL --run --as val -- cargo test

bracken log                          # everything, in causal order
bracken log --since ""               # the same, then a cursor: pass it next time for only what is new
bracken log --compact                # one line each (at most 200 characters), shortest unique ids (8+ digits)
bracken log --since "$C" --limit 50  # only the most recent 50 of what is new; the cursor still moves past all
bracken log --since "$C" --as alice  # compact by default (--full for the long form); what concerns alice first
bracken watch --since "$C" --tag finding    # new contributions, one compact line each, then the next cursor
bracken watch --follow --for-me --as alice  # keep printing what concerns alice as it arrives
```

`evidence --run -- <command> [args]` runs the command without a shell, in the current directory, with no
input, and shows its output on stderr as it runs. It records `pass` if the command exits 0 and `fail`
otherwise. The method is the command line, and the details are its exit and the last 40 lines (at most
8000 bytes) of its output, with control characters removed. It records nothing if the command can't
start, and exits with the command's exit code. The output becomes permanent in the Ground, so don't use
it for a command whose output can hold secrets. Without `--run`, the target and verdict may come after
`--` too, as before.

With `--since` or `--limit`, `log --json` prints an object, `{"contributions": [...], "cursor": "...",
"omitted": N}`, instead of the bare array; with `--since` and a caller it adds `"for_you"`, the ids of
others' verdicts on and contradictions of the caller's work and of others' work that implements something. A cursor means the same only under the same filters: new work
that `--tag`, `--kind`, `--author` or `--current` hid is past it all the same, so keep one cursor per set of
filters. Every read command prints control characters from contributions (CR, ESC, newlines and the
like) as escapes such as `\u{1b}`, so no contribution can forge or hide a line; `show` keeps a statement's
or evidence's line breaks, with each of its lines starting `| `. `--json` escapes them as JSON does.

`watch` prints the new contributions since a cursor (`log --since`'s), filtered by `--tag`, `--open` (each
finding when what arrives makes it open as `log --open` decides it, usually its `contradicts` relation;
once, unless it closes and reopens) or `--for-me` (others' `contradicts`, `refines` and `depends_on` relations to the caller's
contributions, and work waiting for the caller's pass, as `status --as` orders it). Each is one compact
line, and the cursor to pass next time comes last; with `--json`, each is one line `{"contribution":
{...}, "cursor": "..."}`, the contribution as `log --json` has it with the cursor just past it, and a
`{"cursor": "..."}` line ends the output if what came last was filtered out. Without `--since` it starts
at the heads, so only what arrives from then on. A contribution that arrives late through sync is
reported once, however old its timestamp: the cursor counts what it has seen, not times. The cursor
names contributions by full id, so it means the same on every replica; treat it as opaque. With
`--follow` it re-reads the replica every `--interval` (5s, at least 2s) and keeps printing, and exits 0
once its reader goes away. Through `scripts/bk`, `watch` pulls from the team server first, and
`--follow` syncs before each look. Its output is other agents' text: check a contribution with `show`
before acting on it.

## Reading state

```sh
bracken accepted                     # what the default policy accepts
bracken accepted --min-passes 2 --validator val
bracken status                       # the review queue, one line each: waiting or ready first, oldest first,
                                     # then blocked, rejected, superseded, retracted, accepted; at most 40 lines
bracken status --as alice            # alice's work, contradictions of her claims, work waiting for her pass, the rest
bracken status --full                # per claim: what implements it, with passes and what blocked work needs
# A fail with --depends-on shows its work blocked, then ready once all it names is accepted (only each
# validator's latest fail counts for this). Ready isn't accepted: the fail counts until its author retracts it.
bracken status --all                 # plus the claims nothing implements yet
bracken intend $REQ --for 30m        # "I'm on this": shown in status and show until it ends
bracken conflicts                    # divergences and contradictions
bracken findings $PR --json          # the merge gate's findings check for one claim: each finding linked to it,
                                     # untriaged, confirmed, deferred (with the task) or rejected, and which block
bracken show $REQ --json             # every read command supports --json
```

`findings <claim> [--triager NAME]` (manager by default) lists every contribution with a `contradicts`
relation to the claim that isn't retracted and has a link that still counts: live, or retracted by
someone other than the finding's author and the triager. Each gets its triage, from the triager's live
evidence only: `untriaged` (none), `confirmed` (a pass, whatever else), `deferred` (confirmed, with a live
`depends_on` of the triager's from a current task of theirs tagged `deferred`), or `rejected` (no pass).
Untriaged and confirmed ones block the claim. These are `log --open`'s rules, which share the code, but
`findings` looks at the claim itself, current or not, merged or not, as the merge gate does. `--json` gives
`{"claim", "triager", "triager_name", "findings": [{"id", "triage", "task", "blocks"}], "blocking"}`.

## The verified snapshot

Every read and write opens the replica through `cache/snapshot` in it (`.bracken/cache/snapshot`): the
contributions this replica's readers have already verified, in one file (owner decision `96e282819607`).
A read lists `objects/`, adds the snapshot's contributions without reading their objects or checking
them again, and reads and checks only the objects the snapshot lacks, every validity rule as a full open
checks them (protocol.md, Validity). A command that writes rewrites the snapshot as it ends, so the
reads after it find nothing new and write nothing; a read rewrites it only when objects arrived another
way, such as a git pull. At this repository's size that skips the signature checks and per-file reads
that are most of a read's cost.

- **The trade.** An object the snapshot holds isn't read again, so if its bytes are overwritten on disk
  later, or made unreadable, a cached read doesn't notice, where a full check refuses the replica. A
  deleted object is noticed, since every read lists `objects/`, and so is one replaced by a link, a
  directory or anything but a regular file. Anyone who can write `cache/` could hide or alter
  contributions from this replica's readers, but they could already write the replica, keys included.
- **Falling back.** A snapshot that doesn't fit the replica (damaged, of another Ground, naming an object
  that's gone or out of causal order) is ignored: the read is a full check, which then rewrites it. A
  new object that breaks a rule fails the read exactly as a full check does.
- **Forcing a full check.** `BRACKEN_FULL_CHECK=1` makes every read a full check that neither reads nor
  writes the snapshot. `scripts/merge`, `scripts/verify-seq` and `scripts/ground-guards` set it.
  `bracken verify [--json]` re-reads and checks every object once, prints how many, and rewrites the
  snapshot; it fails, as a full check does, if any object is damaged or overwritten.
- **On disk.** `cache/` holds a `.gitignore` of `*`, so git never commits it, and nothing is written
  outside it. The snapshot goes to a temporary file of its own, created fresh (never through a link),
  then renamed into place. A `cache` that isn't a real directory is never written to. `bracken view`
  reads through the snapshot but never writes it. The snapshot is never synced: each replica builds its
  own.

## The web view

`bracken view [--port N]` serves a read-only web view of the replica on 127.0.0.1, with its JSON API
under `/api/` (the shapes are in [docs/design/visualizer.md](design/visualizer.md)) and new work
announced on `/api/events`. It never writes, reads no identity, and sends Ground text only as JSON, under
a Content-Security-Policy that allows no inline script.

`--bind <ip>` serves on the address you name instead, such as this machine's NetBird IP (owner decision
917816c72975). It's repeatable: one process can listen on one loopback and one other address, both at
`--port` (decision 5863740ca157).
- On a non-loopback address, anyone who can reach that network can read the whole Ground: there's no
  login, and it says so when it starts.
- It never binds every interface (`0.0.0.0`, `::`), a multicast or a broadcast address.
- Each listener has its own Host rules:
  - the loopback one answers to `127.0.0.1:<port>` and `localhost:<port>`;
  - the other answers to its own `<ip>:<port>` and any names given with `--host <name>`.

For example: `bracken view --bind 127.0.0.1 --bind <ip> --port 7392 --host <name>`.

`bracken view --runs <dir>` adds an experiment run to the view's Runs page: one mode's run directory, or a
directory of mode runs side by side. It reads each run's `report.json` once, at start, which
`bracken-experiment report --json <run> > <run>/report.json` writes, after checking it's for the run's
plan. It shows only that report and the plan's waves, and never serves a file. It looks no deeper than
the directory's children, refuses every symlink and any home directory, and is refused unless the view
listens on loopback only.

## Moving contributions between replicas

```sh
bracken sync ../other-checkout       # exchange with another replica on disk
bracken serve --tokens T --allow A --first-seen L   # serve this replica on 127.0.0.1
bracken serve ... --keys K --scopes S --signed L2   # and sign for the identities whose keys are in K
BRACKEN_TOKEN=... bracken sync http://127.0.0.1:<port>
bracken sync --exec ssh host bracken --dir <path> serve --stdio
bracken export ground.mrb            # or carry a bundle over any transport
bracken join ground.mrb              # create a new replica from a bundle
bracken import ground.mrb            # add a bundle to an existing replica
```

### Keys a server holds

A server can hold identities' keys and sign for them (docs/design/agent-keys.md, 4.1 and 4.2), so no key
of theirs sits in a replica's `identities/`. `bracken id new <name> --key-dir <dir>` makes an identity with
its private key in `<dir>` and only its public entry in the replica's `agents/`. `bracken serve --keys
<dir> --scopes <file> --signed <log>` binds each credential in `--tokens` to the key of the same name in
`<dir>` (which must be 0700), and answers `POST /v1/<ground>/sign`: the body is a contribution's content
in canonical encoding, the reply its 64-byte signature. It signs only if the content re-encodes to the same
bytes, is for its Ground and protocol 1, is authored by the token's own key, holds none of its keys (those in `<dir>/retired/` included, which it
reads for this check only and never signs with), and is
within that name's scope in `--scopes`, a TOML file it reads on every request:

```toml
[scope."builder-*"]          # a name, or a name's start and '*'; an exact name wins, then the longest
kinds = ["claim", "relation", "evidence", "retract"]   # absent: nothing
relations = ["implements", "refines"]                  # absent: none
forbidden_tags = ["decision", "approval"]              # and tags = [...]: the only tags allowed
```

Before it answers, it appends `{"id", "credential", "signed_ms"}` to `--signed` and syncs it, so every
signature it gave is in the log (a contribution it signed may still be refused by the client's replica
afterwards, and never stored). A key file moved out of `<dir>`, or into `<dir>/retired/`, is no longer signed with, nor is a key whose public
key is in `<dir>/retired.jsonl` (`{"name", "agent"}` lines) or `<dir>/retired/`, even if a copy is back in `<dir>`.

`bracken id scope <name> --scopes <file> [--json]` says what a server would sign for `<name>` under that
scopes file, parsed and matched by the server's own code: its kinds, relations and tags, or that it has no
scope. A file the server couldn't parse fails it, naming why; the server would then sign nothing. It reads
only the file, and needs no replica. `scripts/team-server ready` uses it.

A write command signs with the identity's file in `identities/` if there is one. Otherwise, with
`BRACKEN_SIGN_URL` (a loopback server's URL) and `BRACKEN_SIGN_TOKEN_FILE` set, it sends the content to
that server and checks the signature it gets back before adding the contribution; `scripts/bk` sets both
for its writes. The contribution is byte for byte what the key file would sign.

## Intents

**Intents** are a convention on ordinary contributions, not part of the protocol. An intent is a
claim tagged `intent` and `until:<RFC 3339 time>`, related to the contribution it's about by a
`refines` relation from the same author; `bracken intend <id> [--for 1h]` writes both. `status`
and `show` list an intent as live until it expires, its author retracts it, or its author relates
something that `implements` the contribution. Retracting that relation makes the intent live again.
A claim tagged `intent` without an expiry, or without a `refines` relation from its author, is an
ordinary claim. Like every timestamp, the expiry is advisory: it's the author's clock, so readers
treat it as a hint, not a lock.

## Weave

`bracken weave` keeps source trees in a Weave Ground: file versions and manifests imported from git,
lines and their decisions, proposals, merges, and the checks that make a line green. The spec is
[docs/design/weave.md](design/weave.md); this section follows `bracken weave <command> --help`. In this
repository the Weave Ground is reached only through `scripts/weave`, which picks the Ground and refuses
what an identity may not run (AGENTS.md).

```sh
bracken weave init --dir <new dir> --as alice     # the Weave marker and a `main` line
bracken weave import --from-git HEAD --as alice   # or --index; --dry-run lists what it would write
bracken weave propose --from-git HEAD --summary "..." --as alice   # against the line's head, or --base
bracken weave integrate --proposals <id> --as alice                # integrator only
bracken weave head                                # the line's head and its manifest
bracken weave verify --git HEAD                   # the head against a commit's tree; non-zero if they differ
```

Every command takes `--dir` and `--as`, and `--line <id or name>` where it reads or writes a line (the
default line otherwise). Every read has `--json`. A manifest is named by its content hash,
`blake3:<hex>`; a decision or proposal by its id.

| Command | What it does |
|---|---|
| `init [--purpose P]` | Creates a Weave Ground at `--dir`: the Weave marker and a `main` line, signed with `--as`. `--dir` must not exist yet and its parent must; refuses if the parent is inside a git checkout |
| `import (--from-git REV \| --index) [--repo R] [--dry-run]` | Records a git tree as file versions (only those new to the Ground) and one manifest. Takes git's view only: a commit's tree, or the index. Refuses, writing nothing, if anything looks like a secret |
| `checkout --manifest H <dir>` | Writes a manifest's tree into an empty directory. Never follows a symlink; on a failure part way, removes what it wrote |
| `diff A B` | The per-file difference between two manifests or decisions, with a unified diff of each changed text file |
| `verify --git REV [--at D \| --manifest H]` | Compares a manifest with a commit's tree: the same paths, modes and bytes, with the excluded root entries left out. Exits non-zero if they differ. Compares the line's head unless `--at` or `--manifest` names another |
| `check` | Lists Weave payloads that are invalid or break the ancestor rule, manifests not yet complete, markers past the one that counts, and policy versions ignored for citing no owner decision. Exits non-zero when it lists any |
| `head` | The line's head with its manifest, or every head of a forked line |
| `log [--limit N] [--since D]` | The line's decisions, newest first in causal order (20 by default): manifest, parents, breakage and note |
| `status [--index] [--at D]` | Git's `HEAD`, or its index, against the line's head (or `--at`, needed while the line is forked): each file added, modified, deleted or with its mode changed |
| `propose [--from-git REV \| --index] [--base D] [--summary S] [--dry-run]` | Records a tree (git's view, with import's refusals) and a proposal to integrate it onto the line, against a base decision, by default the line's head |
| `integrate` | Integrator: records the line's next decision, with every head of the line as a parent, so on a forked line it settles the fork. The tree is `--from-git REV` (default `HEAD`), `--index` or `--manifest H`, or `--proposals P...` merged in order onto the line's head, file by file against each one's base, with the resolutions the Ground holds; a conflict without one stops it, listed, and nothing is written. `--mirrors REV` (stage 2a) writes the merge only if it equals the commit's tree. `--run` runs the line's checks first, for your information only. `--answer`, `--breakage`, `--goal` and `--note` fill in the decision. Refuses if the decision wouldn't qualify |
| `green [--manifest H]` | The line's green under its current policy, and per head each check's counted results, with the fails a later attempt replaced. With `--manifest`, any manifest the replica holds, leaving out the integrators' own results |
| `review [--proposal P]` | Reviews of the line's heads and open proposals, and the objections the next move must answer, with the dispositions each allows |
| `validate --manifest H [--check NAME]... [--again] [--timeout S] [--runner DIR] [--dry-run]` | Validator: runs the line's named checks (default: all) on a manifest and records each result on it. A check runs through the sandboxed check runner unless it has an executor: a `box` check runs on a remote box, and a `mac` check on this Mac, unsandboxed ([weave.md](design/weave.md), *Checks outside the sandbox*). Such a check needs `--runner`, the directory its runner script is in. Only for one of the line's validators; refuses a check you already have a latest result for unless `--again` |
| `line create <name> --owner-decision ID` | Creates a line: its first policy version, signed by `--as`, who alone can change it. `--integrator`, `--validator`, `--goal-setter`, `--check NAME=ARGV`, `--executor NAME=KIND:SCRIPT` (a check that runs outside the sandbox: `box` on a remote box, `mac` on this Mac, unsandboxed) and the guardrails `--max-known`, `--max-age`, `--protected` set the policy |
| `line policy <line> --owner-decision ID` | Writes a new version of the line's policy, superseding every current version. Only the line's author can; an option not given keeps the current value, and `--clear` empties a list |
| `line merge <from> --into <line> [--base D] [--dry-run]` | Integrator: merges one line's head into another's, file by file against their merge base. With every conflict resolved, writes the merged manifest and a decision whose parents are both heads; otherwise lists the conflicts and writes nothing |
| `resolve <path>` | Writes a resolution for the conflict at one path of a merge, named as `line merge` (`--into`, `--from`) or `integrate` (`--proposals`) names it: `--take into\|from\|<resolution>`, `--delete`, or `--file F`. `--markers F` writes nothing to the Ground, only a file with the conflicting regions between markers, to edit and give to `--file` |

A policy version, from `line create` or `line policy`, counts only if it cites an owner decision with
`--owner-decision`. In a merge, a text file both sides changed merges line by line, three-way; it
conflicts only where both sides changed the same or adjacent lines differently. A file that isn't
text (over 8 MiB, not UTF-8, or holding a NUL), or whose diff on either side is longer than 10,000
lines once their common first and last lines are set aside (`MAX_DIFF_LINES`,
`crates/tree/src/text_merge.rs`), conflicts as a whole file. So does a mode conflict or a delete
against a change; each needs a resolution.
