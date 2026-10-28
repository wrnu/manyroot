# Network sync between Ground replicas

> **Decided in part, 2026-10-27.** The owner accepted the recommendations
> (decision `4c55e65bc290`), with decision 2 deferred and decision 6
> amended; both are recorded below. The supersession rule (decision 4) is
> in [protocol.md](../protocol.md). The sync protocol itself enters
> protocol.md only with its own `protocol`-class PR. Task `9fa2c890ae5a`.
> Steps 1 and 2 are built; [As built in step 2](#as-built-in-step-2) says
> what the transports do (task `57072a4ea13f`).

Today replicas exchange whole bundles by file, or `bracken sync` merges two
local directories, and the team shares one Ground in the main checkout,
committed to git. This proposes sync over a network.

## Decisions

The owner's answers, 2026-10-27 (decision `4c55e65bc290`: "Accept your
recommendations").

1. **Accepted: the exchange and transports** (sections 1 and 2): heads
   exchange, then an exact offer/need step, with bundles as batches, carried
   over HTTP first and SSH second. Peer-to-peer later.
2. **Deferred: where the team's sync server runs**, who can reach it, and
   its off-host backup (sections 4 and 5), until there's something to
   deploy. The backup must be one the server can't rewrite, and holds the
   objects, the first-seen log and the server's configuration.
3. **Accepted: v1 authentication** (section 3): credentials stay in the
   transport, and the server keeps an allowlist of author keys. Each machine
   generates its own identities, and only public keys leave it. No
   authentication in the protocol until membership (Stewards). Until each
   agent holds only its own key, author-based rules are conventions inside
   the team.
4. **Accepted: the supersession rule** (section 6, answering
   `3d381dfdb8a9`): a `supersedes` relation counts only when its author wrote
   the contribution it replaces. Now in protocol.md, with a migration note
   in [releases.md](../releases.md); it changes nothing in this repo's
   Ground.
5. **Accepted: CI stays an observer in v1** (section 5), as decision
   `8b2454e4cf81` has it. CI as a validator is under
   [Later](#later-ci-as-a-validator), because a CI pass on a PR's tree can't
   be independent of that tree. While CI runs PR code, no repository or
   organization Actions secrets exist.
6. **Amended: the manual Ground checkpoints stay** (`scripts/ground-checkpoint`
   from the main checkout) until the off-host backup in decision 2 exists.
   There's no automated export, because one would have to hold a credential
   that can push to the code repository (finding `99c8fcc0702f`). Decision
   `34dd510c2457` stands.

## 1. What's exchanged

### The key property

A replica only adds a contribution once its parents are present (validity
rule 4), so every replica's set is **closed under ancestry**: its causal
heads describe it exactly.

### Recommendation: heads, then offer and need

One session between replicas A and B:

1. **Hello.** Both send `{protocol: 1, ground, limits, features}`. If the
   Grounds differ, they stop.
2. **Heads.** Both send their causal heads.
3. **Candidates.** Each side computes what it may need to send: its own set
   minus the peer's heads that it holds and their ancestors.
   - If it holds *all* the peer's heads, the peer's set is a subset of its
     own, and the candidates are exactly what the peer lacks. Skip to step 5.
   - Otherwise the peer has concurrent work this side hasn't seen, and some
     candidates may already be there. Go to step 4.
4. **Offer and need.** Send `Offer {ids}` (32 bytes each). The peer answers
   `Need {ids}` with the ones it lacks.
5. **Batch.** Send the needed contributions, parents first, as a bundle in
   today's format (`MRBUNDL1`, ground id, length-prefixed contributions). One
   session may send several batches, each under the receiver's byte limit.
6. **Result.** The receiver imports and replies
   `{added, already_present, rejected: [{id, reason}], unresolved}`. That's
   `ImportReport`, which exists already.

Pull and push are the two halves; the server's permissions decide which a
client may run. Messages are CBOR maps with a `type` field. The exchange is
exact: it never sends a contribution the receiver holds, at a cost of at most
one extra round trip and 32 bytes per candidate id.

**Wire format, as built in step 1** (answer `22cdcf597acf`; normative text in
[protocol.md](../protocol.md), "Sync"):

- Each message is one canonical CBOR map, `type` first, re-encoded on receipt
  and rejected if the bytes differ. Framing between messages is the
  transport's.
- `hello {protocol: 1, ground, limits: {batch_bytes}, features: [text]}`;
  unknown features are ignored. A `batch_bytes` under 44, too small for one
  contribution, ends the session (finding `4922d8de4c62`).
- `heads {heads}` carries the sender's causal heads, sorted by id;
  `offer {ids}` the candidates, in causal order; `need {ids}` the offered ids
  the receiver lacks, in offer order. Ids are 32-byte strings (finding
  `f29c3b7292ea`). Each is one list split across messages of
  at most 65 536 ids (a protocol constant, not tied to `batch_bytes`); a short
  message, possibly empty, ends the list. The message size limit is the larger
  of a batch and a full id list, so a replica with a small `batch_bytes` still
  takes a large offer (finding `0372553b36ac`).
- `batch {bundle: bytes, last: bool}`: the bundle is today's `MRBUNDL1` bytes.
  `last` tells the receiver the sender is done, which it can't otherwise know
  under the shortcut. Every side's sending ends with exactly one `last` batch,
  possibly empty.
- Batches go one at a time; the next waits for the peer's `result`.
- `result {added, already_present, rejected: [{id, reason}], unresolved}`;
  `reason` is advisory text. `rejected` holds at most 1 024 entries with
  reasons of at most 256 bytes, so a `result` fits the peer's message limit
  under asymmetric `batch_bytes` (finding `f29c3b7292ea`).
- After a `need`, a `last` batch before every needed id has arrived ends the
  session (finding `309a0d9671f2`). Arriving isn't being added: a peer that
  sends a child before its parent leaves the child unresolved, and the
  session still completes. The `result`'s `unresolved` count reports it, and
  `bracken sync` then exits non-zero (finding `5c4022219fea`).
- A receiver refuses a batch over its own `batch_bytes` before decoding it. A
  sender stops if one contribution can't fit the peer's limit.
- Over-sending ends the session: a contribution the receiver neither asked
  for with `need` nor could have been sent under the subset shortcut (one it
  held when it sent its `heads`), or one sent before in this session. So the
  shortcut never ends its own session. An `offer` or `need` listing an id
  twice ends it too, as does an empty batch that isn't `last` (findings
  `a38cae31c621`, `ab5965f73636`). A session deadline and a byte budget are the
  transport's, and so is a bound on id lists, which have no total limit
  (finding `b8b70206f0f8`); step 2 built all three (task `d04580486e94`).
- There's no error message in the protocol: a violation ends the session
  locally, and the transport reports it (an HTTP status and a one-line
  reason, or stderr and a closed stream).

**Alternatives:** git-style have/ack negotiation pays off only for huge,
mostly shared histories (a later optimization). Range-based set
reconciliation (Negentropy) doesn't need ancestry-closed sets, so it belongs
with selective replication (Canopy). Bloom filters (Automerge) are compact,
but false positives cost rounds.

### Availability

The demo found most duplicated work is blind (`bd7882c5dcae`): agents can't
see what hasn't reached them. So clients push **right after every write**, and
readers long-poll heads (`heads?wait=30`), which returns as soon as they
change. That narrows the window without closing it; agents starting together
still need work-in-progress claims (`bd5c57651365`).

## 2. Transports

The protocol is the message sequence above. A transport only has to carry it.

- **Streams** (stdio, SSH, TCP, later QUIC). Each message is
  `u32 big-endian length || CBOR message`, the bundle's framing.
  `bracken serve --stdio` makes SSH work like git's: `ssh host bracken serve
  --stdio <path>`, with SSH doing authentication.
- **HTTP** carries the same session, in rounds. `POST /v1/<ground>/sessions`
  opens one: the body is the client's messages, framed as on a stream, and
  the response holds the server's, with the session id in an
  `X-Bracken-Session` header. Each later round is `POST
  /v1/<ground>/sessions/<id>`, until both sides are done.

  This replaces the stateless mapping first proposed here (`heads`, `offer`,
  `fetch`, `need` and `push` as separate requests; manager's answer
  `7db500f36577`): that would have been a second protocol beside the one in
  protocol.md, with its own rules to specify, test and keep in step. With
  rounds, HTTP is only framing, and every rule the session enforces holds
  over HTTP unchanged. Long-polling heads (`heads?wait`) waits for step 3.

**Recommendation: HTTP first**: other machines reach HTTPS without SSH host
keys (gotcha `463398c097a2`). Stdio comes almost free and tests both ends in
one process. **Peer-to-peer later**: ids are hashes and contributions are
signed, so any peer can serve any contribution and the receiver checks it.
Integrity never depends on who serves the data; availability does. The
protocol stays transport-independent because the library exposes a session
state machine (messages in, messages out) with no I/O.

### As built in step 2

`bracken serve` serves one replica; `bracken sync` is the client. Task
`57072a4ea13f`, with the manager's answers in `7db500f36577`.

| Command | What it does |
|---|---|
| `bracken serve --tokens <file> [--port <n>]` | HTTP on 127.0.0.1 only (no TLS, and no flag to bind elsewhere); prints `listening on http://127.0.0.1:<port>` |
| `bracken serve --stdio` | One session on stdin and stdout, for `ssh host bracken --dir <path> serve --stdio`; the credential is SSH's, logged as `stdio` |
| `bracken sync http://127.0.0.1:<port>` | The token from `$BRACKEN_TOKEN` or `--token-file`, never the command line; loopback URLs only |
| `bracken sync --exec <program> [args...]` | Runs the program without a shell, and without `BRACKEN_TOKEN` or `BRACKEN_DIR` in its environment, and syncs over its stdin and stdout. The program stays in bracken's process group, so it can prompt on the terminal (ssh's password and host-key questions). The client's reads and writes end at the deadline whatever holds the pipes, so neither the program nor a descendant (forked, in another group or session) can keep it waiting. The program's stderr is relayed through a pipe of bracken's, so a descendant left alive doesn't hold the caller's stderr either. The program is killed at the deadline, or 2 seconds after the session ends if it hasn't exited; its descendants aren't killed and may outlive the client |

**Server files**, all outside the Ground directory, and refused if a
symlink, not a regular file, or writable by anyone:

- `--allow`: author keys whose contributions the server stores, one hex key
  per line, optionally followed by a name, `#` for comments. Without it the
  server stores nothing: clients can pull but not push.
- `--tokens`: lines of `<credential name> <blake3 hex of the token>`, so the
  file holds no secret. Required for HTTP; there is no unauthenticated mode.
  Hashes are compared in constant time. An HTTP session is bound to the
  credential that opened it: a request with another credential gets `404`,
  as an unknown id does. Session ids are 128 bits from the OS's random
  source, and appear in no log or error message.
- `--first-seen`: required with `--allow`. An append-only log, one JSON line
  per contribution the server stores for the first time: `{id,
  first_seen_ms, credential, author, bytes}`, synced to disk after each
  batch. An id is logged once, across sessions and restarts. If the log
  can't be written, or a write to the replica fails partway through a batch
  (a full disk), what was stored before the failure is still logged and
  counted, and the server stores nothing more until it restarts.

**Refused at intake**, listed in the `result`'s `rejected`, so the client
exits non-zero and nothing refused is stored: an author not on the
allowlist; over a quota, per author key and per credential, of 10 000
contributions or 256 MiB in a rolling 24 hours (replayed from the first-seen
log at startup, so a restart doesn't reset them); and over the 4 GiB storage
cap. A contribution the server already holds is never refused. A refusal is
local policy, not a protocol violation, so the session goes on; descendants
of a refused contribution come back `unresolved`.

**Each session's bounds**, on server and client alike: a deadline 10 minutes
from its start, and a budget of 1 GiB received from the peer, of which id
lists (`heads`, `offer`, `need`) may take 64 MiB, about 1.9 million ids. A
session's memory grows to roughly 8x the id lists it receives, so the id
budget holds it near 512 MiB. The budget is counted per frame, from the
length prefix, before the frame is read, and an HTTP body over what's left
is refused from its `Content-Length`. Past either bound the session ends with
an error, and the replica stays valid, because each contribution is written
whole by atomic rename. An import under way finishes, and the session ends
right after it. Over HTTP a session also ends after 5 minutes without a
request; at most 16 are open, counting opens under way (a slot is reserved
before the first body is read), and 64 connections are served at once.

**What a side sends** is bounded too: a batch never exceeds the smaller of
the peer's `batch_bytes` and the sender's own (64 MiB, `--batch-bytes`), so a
peer can't ask for one batch of everything, and an HTTP response carries at
most one batch. A client following the protocol answers each batch with one
`result` and receives at most one batch per response, so it never sends two
in a round; a request whose results would release a second batch is refused.
A response therefore holds at most a batch and an id list, and with 16
sessions the server's session memory stays within about 16 times that plus
the id budget. All of these bounds are flags.

**Memory, for choosing step 3's limits** (task `c2d2bc516624`). A busy
session holds about three copies of the batch it is sending (the
contributions, the bundle, the response body), so 3 x `--batch-bytes`, plus
the id lists it has received, up to about 8 x `--id-budget`. The defaults
(64 MiB batches, a 64 MiB id budget, 16 sessions) put the worst case near 16 x
(192 MiB + 512 MiB), about 11 GiB, and a typical busy session near 200 MiB.
The team's Ground on 2026-10-27 is 2 328 contributions in 9.2 MB, and the
owner's machine has 64 GiB. Recommended for the team server in step 3:
`--batch-bytes 17825792` (17 MiB, which still carries a 16 MiB contribution),
`--id-budget 8388608` (8 MiB, about 240 000 ids, a hundred times today's
Ground) and `--max-sessions 16`, for a worst case near 16 x (51 MiB + 64 MiB),
under 2 GiB. Revisit the id budget once the Ground nears 100 000
contributions: a session between replicas that each hold work the other
lacks offers every contribution outside what they share, so a replica that
was offline for long can offer many.

**HTTP framing.** One request per connection (`Connection: close`; a
pipelined second request is read and dropped). Heads of at most 16 KiB and
32 headers; bodies by `Content-Length` only (`Transfer-Encoding` gets `501`).
Every wait has an absolute end, however slowly a peer trickles bytes: a
request head must arrive within 10 seconds in all (before authentication, so
this bounds what any local process can hold), a body before the session's
deadline (and at most 30 seconds between its bytes), a response must be
written within 30 seconds, and the client's whole round ends at its
deadline. The client connects to 127.0.0.1 or ::1 itself, never to what
`localhost` resolves to. Parsing is `httparse`, the only HTTP dependency.

**Store writes stay inside the Ground** (task `26a5c735230d`): `objects/` and
each shard must be real directories, not symlinks; the temporary file is
created new; and the rename replaces whatever is at the object's path, a
symlink included, rather than writing through it.

**Limits of step 2:** the server reads its replica once, at startup, so it
must be the replica's only writer while it runs. (Step 3 gives the server
its own replica, outside the repo; `scripts/bk` writes the main checkout's
and syncs. `bracken sync --confirm` runs a second session and checks that
the heads the server sends at its start cover every head this replica held
when the first began.) A stdio peer that stops
reading holds the session's process until the deadline. Sixteen sessions at
the full id budget could hold several GiB, which is acceptable only behind
credentials on one machine. A local process can still keep all 64
connection slots busy by reconnecting every 10 seconds; that's denial of
service from inside the machine, which step 2 accepts. One credential can
hold all 16 sessions (no per-credential limit yet). Tokens must be long and
random: the file holds unsalted blake3 hashes. Withholding (section 4) is
unchanged.

## 3. Authentication and who may push

Signatures already say who wrote each contribution. What a server still
decides is **who may read**, and **whose contributions it will store**.

**Recommended for v1:**

- **Reading:** the transport authenticates (an HTTPS bearer token or SSH
  key). Anyone authenticated reads the whole Ground. Partial reads are
  Canopy's job.
- **Pushing:** the server stores only contributions whose `author` is on its
  allowlist of agent keys. That doesn't depend on who pushes, so relaying
  others' work is safe. The allowlist is server configuration (a local
  policy), not protocol.
- **Session authentication by agent key** (signing a server challenge) needs
  a new signing context, a protocol change, and would prove little while the
  team's keys share one folder. Later.

**Trade-off.** An allowlist can split replicas: if a key is removed, a
member's later contribution that builds on that key's work can't be stored
(its parent is missing), so the server reports it `unresolved` and the
member's replica holds more than the server. v1 keeps the allowlist add-only
in practice, and treats removal as a key-compromise event (process 9.3).
Membership as contributions (Stewards) would make "who may contribute"
converge with the graph. That's the proper fix, and it's out of v1.

**Within the team, keys are conventions.** Today every key sits in the
shared `.bracken/identities/` folder, so any agent can sign as any other
(`--as`). Until each agent holds only its own key, the allowlist, retraction,
evidence independence and the supersession rule in section 6 all hold inside
the team only by the honour rule (process 2). A compromised machine yields
every key on it.

**Keys on other machines.** Each machine generates its own identities
locally, and only their public keys go to the server's allowlist. No one
copies the identities folder between machines. A machine then holds only its
own agents' keys, which is the first real step toward one key per agent.

**Names in policies.** `--validator <name>` resolves names through local
configuration only (the `.bracken/agents/` directory, kept in step with the
allowlist, which names each key), never through identity claims in the graph
(section 5), and refuses a name that maps to more than one key. Otherwise any allowlisted key could
claim to be `ci` or `manager`.

**v1 leaves out:** membership in the graph, per-key permissions, read
permissions finer than the whole Ground, key revocation in the protocol, and
encryption at rest.

## 4. Abuse

Validity rules already reject wrong Grounds, bad signatures, non-canonical
encodings, missing parents and over-16-MiB contributions. A server accepting
pushes needs more:

- **Size:** cap the request before decoding (e.g. 64 MiB per batch), and
  check each entry's length prefix before allocating, as `bundle::read` does.
  A server may set a lower limit for artifacts than the protocol's 16 MiB.
- **Flooding:** every contribution is permanent, and deleting one would break
  set equality with every other replica. So refuse at the door: per-key and
  per-credential rate and byte quotas, and a total storage cap. An allowlisted
  key that floods gets its credential and key removed, which is the
  compromise path in section 3.
- **Unknown parents:** a push must be closed under ancestry against what the
  server holds. Whatever stays `unresolved` at the end of a request is
  dropped, not buffered across requests, so orphans can't fill memory.
- **Invalid signatures:** check length, decoding and ground before the
  signature.
- **Foreign Grounds:** `Hello` names the Ground and the path carries it; a
  mismatch ends the session before any contribution is read.
- **Pathological graphs:** storing a contribution never derives state, so a
  server stores a long chain of self-retractions without trouble. Every
  *reader* that derives state (`log`, `status`, `conflicts`, the merge gate)
  overflowed its stack in `Graph::is_retracted` on such a chain (note
  `38642d5cfb86`). A chain can't be removed once it's replicated, so the fix
  (task `ae0ffeace5b8`, PR #8) must be in every reader's pinned binary
  before any replica pulls from a server. A depth cap at intake would be a
  local rejection that splits replicas, as the allowlist can.
- **Withholding:** a server can't forge contributions, but it can hide some,
  such as a failing finding against a PR, or show different replicas
  different sets. Comparing heads among replicas that sync only through that
  server can't detect this. **It's a stated limit of v1, with one server.**
  Detection needs a copy that doesn't come through the server, such as the
  Ground checkpoints in git (decision 6) or a direct sync between two
  replicas.
- **Backdating:** `timestamp_ms` is advisory, so a leaked key can backdate
  forgeries (process 9.3). The server should keep a first-seen log
  (contribution id, time, credential), unreplicated but backed up off-host
  (decision 2). It bounds when a contribution really appeared, at no
  protocol cost.

## 5. The team's own use

**Shape.** One sync server holds the team's Ground. The main checkout's
`.bracken/` stays a full replica, and agents on other machines get their own.

- **Writes confirm.** `scripts/bk` pushes after every write, then checks
  that the server's heads cover the new contribution. If they don't, it
  fails, and the writer must not report the work done.
- **Ordinary reads** pull first, with a short timeout, and fall back to the
  local replica if the server is down.
- **The merge gate fails closed.** Its reads must pull successfully, with no
  offline fallback. A replica that can't confirm it's current with the
  server refuses to decide.

**CI stays an observer.** Sync doesn't change what CI can prove: whatever
runs the tests, the PR's own tree decides what they do. The team's
independence comes from reviewers reading build and test changes, which the
process already requires (process step 5). Two standing rules go with this:

- **No Actions secrets.** A same-repo PR's run uses the PR's own workflow
  files, so it can read any repository or organization secret (note
  `404c66f318ce`). While CI runs PR code, none may exist. The job token is
  the remaining exposure, finding `b7f6724d61bb`.
- **The merged commit is the verified commit.** The gate certifies the sha
  in the current `pr` claim, but the Forge PR's head can move, and a `pr`
  claim is free text. Before merging, the manager checks that the Forge PR's
  head is exactly the current `pr` claim's full sha, and merges by that sha:
  the merge API's expected-head field (`head_commit_id` in Forgejo's merge
  options; to be verified before use, since `fj pr merge` has no such
  option), so a head that moved fails the merge. That binds the PR's side
  only: a squash merge lands the verified head combined with main's tip at
  merge time (finding `1b203aeab09b`). So the manager also checks that
  main's current tip is an ancestor of the verified sha (`git merge-base
  --is-ancestor origin/main <sha>`); if it isn't, main is merged into the
  branch again and the gate applies to the new head. This belongs in
  process step 8 and section 4.2, and applies with or without network
  sync.

**Agent names.** `.bracken/agents/` isn't a contribution, so network sync
won't carry it (note `8d64aea0f186`). *Assumed:* each identity contributes a
self-signed claim tagged `identity` with its name, and the CLI shows names
from those, for display only. Two keys claiming one name show with key
prefixes. Policies resolve names from local configuration (section 3).

**What it retires** (process section 11 and nearby):

| Retired | Replaced by |
|---|---|
| `scripts/bk`'s main-checkout-only Ground and `BRACKEN_DIR` trick | Each worktree or machine uses a replica synced with the server |
| Manual checkpoints (9.3), `scripts/ground-checkpoint`, gotcha `a407d726a0ee` | The off-host backup (decision 2), once it exists; until then, checkpoints continue (decision 6) |
| The main checkout as the only live Ground | Server and full replicas; the checkpoints keep the independent timeline |
| Restore by clone (9.3) | Pull from the server or any replica |

CI keeps checking `docs/vision.md` against the Ground committed in git. It
doesn't pull from the server, which would need a credential (the no-secrets
rule above).

**Not retired:** CI as an observer (process section 10, and the first row
of process section 11, stay as they are in v1); CI's Ground-shape and
append-only checks, which guard the checkpoints; one key per agent (needs
Stewards); and branches and PRs for code (need Weave).

**Transition.** Checkpoints continue alongside the server, and the
manager compares the server's heads with the committed Ground at each
checkpoint. Retiring them waits for the off-host backup, and needs your go
(decision 6).

## 6. Cross-author supersession

Until 2026-10-27 a `supersedes` relation counted when its author wrote the
*new* contribution, whoever wrote the old one. So anyone can mark anyone's claim
superseded by writing a claim and a relation. Finding `8d90a9a5d109` showed
this hides others' entries from reading views, and task `780f658d4b84` that
it silences contradictions in `conflicts`. With replicas run by different
people, it lets one organization quietly retire another's claims in every
reader's open layer.

**Decided** (decision 4): a `supersedes` relation counts only when **its
author wrote the target** (`to`). The new contribution may be anyone's: an author
can declare their own claim replaced by someone else's ("I accept yours").
Anything else stays a plain relation, visible: `log --current` keeps the
target and marks it `(superseded by <relation's author> <id>)`.

**Later, policy only: trusting more.** A policy could name
`supersession_authorities` (empty by default, so the default policy trusts
no one extra): in that policy's accepted layer, their cross-author
supersessions would count too. That keeps a reader's right to decide (vision
principle 4) without letting any writer decide for everyone. It changes no
replicated data, so it can come whenever a reader needs it; it isn't part of
the protocol change.

**Effects:**

- `780f658d4b84` is fixed by the rule: `open_conflicts` uses the open layer,
  where cross-author supersession no longer counts.
- The manager handover (process 9.4) under a new identity needs the old
  identity to write the relation while its key exists. Otherwise the new
  manager's claim stands alongside, marked, which is what happens today.
- No validity rule changes, so every existing Ground still opens. Derived
  state changes only where cross-author supersessions exist, and this repo's
  Ground has none (note `4f72b184f324`; every status was compared before and
  after). It's still a `protocol`-class change, with a migration note in
  releases.md.

**Caveat:** inside the team, the rule binds only as far as keys do. Any
agent can still sign the relation with the target author's key, as with
everything in section 3.

**Alternative:** a proposal the old author accepts with a second relation.
Same outcome; the recommendation needs one relation.

## 7. Staged plan

| Step | What | Class | Needs your go |
|---|---|---|---|
| 0 | Supersession rule and its tests (independent of sync; task `e8a78fc1b51a`) | protocol | Yes (decision 4) |
| 1 | Sync messages and session state machine in `manyroot`, with no I/O; `bracken sync` between two local directories uses it. (The iterative `is_retracted` is already PR #8.) | protocol (wire format in protocol.md), library | Yes (wire format) |
| 2 | `bracken serve` (HTTP and `--stdio`), allowlist, quotas, first-seen log; `bracken sync <url>`. Tested on localhost against throwaway Grounds. **Built**: [As built in step 2](#as-built-in-step-2) | cli, storage | No |
| 3 | Every reader's pinned binary includes PR #8 (done); then run the team server and its off-host backup; `scripts/bk` confirms pushes and the gate fails closed; per-machine keys; checkpoints continue. **Built, on the owner's machine, without the off-host backup** (task `c03e893d04eb`): `scripts/team-server`, `bracken sync --confirm`, and [Team server](../development.md#team-server) | merge-gate tooling, process | Yes (decision 2, still open; 3) |
| 4 | Once the off-host backup exists, retire manual checkpoints; update the skill | process, merge-gate tooling | Yes (decision 6) |
| 5 | Agent-key session auth; membership as contributions (Stewards) | protocol, crypto | Yes |
| 6 | Peer-to-peer discovery; range reconciliation with selective replication (Canopy) | protocol | Yes |

The merge checks (section 5) and the no-secrets rule need no sync, so
they can land now as a `process` change.

**The smallest useful step is 1**: it makes the exchange exact and testable
before any network code, and replaces whole-set copying in today's local
`sync`. Step 3 is the first one the team feels: agents on other machines,
and contributions visible in seconds.

## Later: CI as a validator

Not in v1. A `ci` key whose evidence policies count (`--validator ci`) would
let CI join the Ground, but its pass can only ever mean "the PR's own tests
pass in a clean environment", never an independent check. The PR's tree
decides what `cargo test` does: `.cargo/config.toml` (a test runner of
`true`, a `rustc-wrapper`), `rust-toolchain.toml`, test harness settings in
`Cargo.toml`, deleted tests, `build.rs` and proc-macros (finding
`06241d722a91`). So a reviewer's evidence stays required alongside it. A
signer would need at least:

- **Isolation** (finding `4612a2e0930f`): builds run on a separate,
  disposable VM, not on the host holding the key or the sync server, with no
  network, no host mounts or container socket, unprivileged, with resource
  limits. Only the outcome goes back to the signer.
- **Dependencies** fetched in a separate step, from the PR's `Cargo.lock` but
  with cargo configuration the signer supplies, into a read-only vendor
  directory. The fetch needs network, so it also runs on the disposable
  side, never on the key's host. Cargo reads `.cargo/config.toml` from every
  directory above where it runs, and the repository's can name a registry
  credential provider (an executable), source replacement or path
  overrides. So the fetch runs outside the checkout tree, or with the
  repository's `.cargo/` removed, with a signer-set `CARGO_HOME` (finding
  `53325c072be5`). The build ignores the repository's `.cargo/` and
  toolchain file and uses a fixed toolchain image.
- **A policy for build-configuration changes:** a PR that touches `.cargo/`,
  `rust-toolchain*`, `build.rs`, harness settings, or dependencies
  (`Cargo.toml` or `Cargo.lock` adding crates brings their `build.rs` and
  proc-macros), or deletes tests, gets no `ci` pass, only a flag for
  reviewers. A test can also end its process early (`std::process::exit(0)`
  makes the whole test binary exit 0), which is one more reason a ci pass is
  never independent.
- **Binding** (finding `f68a026f861f`): it runs only for a full 40-hex sha
  that is the Forge PR's current head, records the PR number, and the merge
  uses the expected-head check in section 5.
- **The key:** readable only by the signer's service user. A leaked key lets
  its holder sign passing evidence on any claim, permanently and backdated,
  until rotated as in process 9.3: a new key under a new name, a `decision`
  naming the compromised one, the old key removed from the allowlist and
  every `--validator`, and everything it signed re-checked.
- **Prerequisite:** finding `b7f6724d61bb` closed, so no PR run can push to
  main.

## Comparison with existing systems

**Git's smart protocol.** The server advertises refs; the client sends `want`
and `have` lines until both agree on common commits; the server sends a
packfile. Authentication is the transport's, and v2 runs statelessly over
HTTP. We borrow transport authentication,
the stateless HTTP mapping and `serve --stdio` over SSH. We need less: git's
refs are mutable, so pushes can conflict and need locking; a Ground's heads are
derived, and every push is a set union that can't conflict.

**Automerge sync.** Peers exchange their heads and a Bloom filter of the
changes since the heads they last had in common, then send what the filter
says is missing, repeating until both sides' heads match. It has no
authentication: that's the application's job. We borrow the heads-first
exchange, and replace the Bloom filter
with offer/need, which is exact, because our ids are cheap to list.

**Hypercore.** Each feed is a single-writer append-only log, with a Merkle
tree signed by its writer; peers replicate ranges, tracked with bitfields,
and find each other through a DHT (Hyperswarm). Multi-writer needs a layer on
top (Autobase). Our graph is multi-writer, with no per-author
sequence numbers, so per-author range sync would need a protocol change. What
carries over is the peer-to-peer part: DHT discovery, and fetching verifiable
data from untrusted peers, as IPFS's Bitswap does with want-lists of content
ids, like `Need`.
