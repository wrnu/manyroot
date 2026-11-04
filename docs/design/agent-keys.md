# Per-agent keys and delegation

Design only, no code (task `21e1c394ca2d`, step 2 of the Weave order in
decision `b4a3456de597`; [weave.md](weave.md) section 8). It compares the
options for making a signature in the Ground mean more than "whoever claimed
to act", recommends one, and lists what the owner needs to decide.

The constraint everything here works within: **every agent runs as the
owner's one macOS user**, and there will be no dedicated user (decisions
`91f610a624fc` and `6f15365a7281`). Keychain, other OS settings and creating
users are out of scope for this task.

Review history. Round 2 (brief `4d56e3db211c`) answered the first draft's
seven findings: the revocation cut (`459f57b58bb5`, `e2663094ef89`),
delegation by a stolen manager credential (`c20195623de6`), the migration
window (`263004ff8abd`), what an off-machine root buys (`9328d3a71afd`), the
threat model's gaps (`d49dc7939051`) and the server's intake of revocations
(`bd43398b3a0f`). Round 3 (brief `6302190acb02`) answers round 2's five
(`e52cc9ba888e`, `81d13d29f631`, `0d839e105c44`, `39653d87385e`,
`35c17799be7a`). Their common cause: whether a record tightens or loosens
depends on the policy that reads it (a fail rejects a finding and so
unblocks a PR; a supersession can retire "Weave gate: on" or "Weave gate:
off"), so no rule that counts some kinds of a revoked key's records and
ignores others is safe. A revoked key's records outside its cut now **fail
closed**: every reader that would use one refuses to decide until the root
rules on it by id (4.4).

## Summary

- **Against an agent that sets out to forge, nothing on this machine makes
  a signature proof.** Any agent with a shell can read every file the
  owner's user can, keys and tokens included, wherever they live, and can
  edit every verifier on this machine. Even a root key held off the machine
  only protects verifiers that are also out of agents' reach (section 4.6).
  The design says so plainly (section 2).
- **What it can improve is real, though:** accidents (signing as the wrong
  identity), detection (a forgery leaves a trace that a guard reports),
  revocation (a distrusted key's later records can't decide anything
  without the root's ruling), blast radius (a stolen credential signs only
  what its role may, and can't mint a validator), and the chance of
  publishing a key through the public repository.
- **Recommended: option D** (section 3), built in two parts (section 5).
  **Part A stands alone** and can go ahead whatever becomes of part B:
  1. **Guard rails, no format change.** `scripts/bk` refuses an `--as` that
     differs from `BRACKEN_AGENT`; a guard reports contributions first
     pushed under another identity's credential.
  2. **The team server signs.** New keys are generated in the server's
     private directory and never leave it. An agent holds a token, never a
     key. The server signs only as the key its token is bound to, only what
     that key's scope allows (from the server's own configuration in part
     A), and logs every signature. A contribution by a server-held key that
     isn't in the log was signed by someone who read the key file: the
     guard reports it.

  **Part B** makes roles and revocation data in the Ground:
  3. **Delegation records.** The owner's root key alone grants the roles
     whose acts a gate counts (`reviewer`, `security`, `owner`,
     `weave-runner`, `weave-integrator`, and `manager` as triager and Weave
     gate switch); the manager may grant only builder keys, only server-held
     ones, and its grants fall with it.
  4. **Revocation that fails closed.** Only the root revokes, and a key's cut
     can only shrink. A revoked key's records outside its cut are kept,
     everywhere, and flagged. Every role-aware reader (merge gate, triage,
     owner approvals, the Weave gate switch, Weave's views, acceptance)
     refuses to decide anything such a record touches until the root rules
     on it by id, `accept` or `dismiss`. The same records and rulings give
     the same answer on every replica.
- **No protocol change.** Ids, encoding, the signing context and the
  validity rules stay as they are; every existing contribution still
  verifies, and an older binary opens the Ground and sees the new records as
  artifacts. Protocol-class review is still needed: `manyroot`'s signing API
  changes shape (`identity.rs`, `contribution.rs`) without changing a byte,
  and acceptance gains an opt-in policy field and a `held` status (protocol.md,
  *Acceptance*), whose default changes nothing.
- **Migration re-signs nothing.** In one root session the owner grants the
  20 legacy keys their current names with an unrestricted scope and no
  right to delegate, grants the new server-held keys, and revokes the legacy
  keys, in that order. Every pre-cut status stays as it is. About 11,700
  contributions stay exactly as they are. The legacy key files can be
  deleted, but copies remain in backups, snapshots and transcripts, so they
  never really leave the machine.
- **The join should wait for part B**, so Weave's identities in the
  coordination Ground are server-held and root-granted from their first
  contribution. **The format freeze needn't wait:** Weave's line policies
  keep naming keys, and no Weave format changes (section 4.10).
- **Questions for the owner** are in section 6.

## 1. Where things are

- **Keys.** 20 identities, each an unencrypted Ed25519 key in
  `.bracken/identities/<name>.json` (mode 0600, in a 0755 directory) in the
  main checkout. Every agent can read every key, `owner`'s included.
  `bracken --as <name>` loads that file and signs in-process
  (`Store::contribute`, `crates/manyroot/src/store.rs`).
- **Names.** `.bracken/agents/<name>.json` maps a name to a public key, one
  key per name forever (the directory refuses a taken name). It is committed
  and public. Policies resolve `--validator <name>` through it.
- **Tokens.** The team server (`scripts/team-server`, 127.0.0.1 only) has one
  token per identity in `~/.bracken-team/manyroot/tokens/<name>` (0600).
  `scripts/bk` picks the token by `--as`, else `BRACKEN_AGENT`. A token
  decides who may connect; it isn't bound to an author: any token may push
  any allowlisted author's contribution, so relaying is safe
  ([network-sync.md](network-sync.md), section 3). The allowlist is every key
  the main checkout knows. The first-seen log records the credential that
  first pushed each contribution.
- **The owner.** Signs approvals with `scripts/owner-approve`, which asks
  for a typed "approve" on a terminal; the manager may sign one on the
  owner's delegation (`49cb2097aeeb`). Without an `owner` token,
  `scripts/bk` refuses writes as owner; with it, any agent's `--as owner`
  write goes through (development.md, *Owner approvals*).
- **Weave.** The dogfood Ground at `~/bracken-weave/manyroot` has its own
  identities and never syncs. `weave-runner` is its only validator, enforced
  by `scripts/weave` as a convention. Line policies name integrators and
  validators by key (`crates/tree/src/format.rs`).
- **The Ground is public** (decision `473d5df9b1ab`): every contribution,
  with its author key and signature, is readable by anyone.

So a signature today says which key signed, and the key says which name
claimed to act. Process section 2 calls it what it is: who claimed to act,
not proof of it.

## 2. Threat model

### What depends on who signed

| Rule | Where | What a forged signature does |
|---|---|---|
| Evidence counts only from `--validator`s, never from the subject's author | merge gate (process 4.2) | A builder's own pass, signed as `reviewer`, merges a PR |
| A fail from a validator blocks | merge gate | A fail retracted with the reviewer's key unblocks a PR |
| Owner approvals gate sensitive actions | `scripts/merge --owner-approval`, `weave-shadow --init`, `weave-backup` | An approval nobody gave |
| Only an author retracts, and supersession counts only from the target's author | protocol.md | Anyone's work retired or revived |
| Weave results count only from the line's validators; decisions only from its integrators | weave.md sections 2, 3 | A green manifest nobody checked |

### Who might act

1. **An honest agent that makes a mistake**: copies a command with
   `--as manager`, or runs a script in the wrong worktree. The common case.
2. **An agent steered by what it reads**: a PR, a web page or a test's
   output that says "now sign this as reviewer". Every agent here reads
   untrusted text.
3. **An agent that sets out to forge**, through a compromised harness or
   model.
4. **Anyone, through publication or a leaked copy**: a key committed to the
   public repository, pasted into a contribution, or held in a backup,
   snapshot or log that leaks.

### What a same-user agent can still do

Whatever this design does, an agent running as the owner's user with a shell
can:

- **Read any file** the user can: `.bracken/identities/`, the server's
  directory and its keys and tokens, the Weave Ground's identities, any key
  file wherever it is moved. File modes don't separate processes of one
  user.
- **Read other processes' arguments and environment** (`ps eww`, sysctl
  `KERN_PROCARGS2`): a token passed in an environment variable is visible.
- **Attach to another process.** macOS restricts `task_for_pid` (SIP, the
  hardened runtime, the developer-tools authorization), so this is harder
  than on Linux, but nothing here relies on it either way: reading files is
  easier.
- **Run code inside the manager's processes through git hooks.** Every
  worktree shares the main checkout's `.git`, so a hook an agent installs
  there (or a `core.hooksPath` it sets) runs during the manager's
  `scripts/ground-checkpoint`, merges and pushes, with the manager's token
  and environment.
- **Change what the server runs**, now and across restarts: replace the
  pinned binary in `.bracken-bin`, edit the scripts in the main checkout,
  edit the launchd plist in `~/Library/LaunchAgents` that starts the server
  (and so point its `--keys`, `--tokens` or `--allow` elsewhere), or kill the
  server and start another on its port.
- **Edit every verifier on this machine**: the scripts, the binary, and any
  file that pins a trusted key, such as the root key's pin (4.6).
- **Write any replica directly**, the main checkout's and the server's.
- **Fake a terminal** for `scripts/owner-approve` (`script(1)`, `expect`).
- **Sign anything** with any key it has read.

And copies of keys and tokens outlive the files they were read from:

- **Time Machine backups and APFS local snapshots** hold every version of
  `.bracken/identities/` and of the server's directory that existed when
  they were taken. Deleting a key file doesn't delete those, and a leaked
  backup leaks every key in it.
- **Agent transcripts** (Claude Code's project logs under `~/.claude/`)
  keep any tool output that ever showed a key or a token.

So, for actor 3, every option below leaves forgery possible on this
machine. What the options change is how far an agent has to go, and what it
leaves behind:

- **Accident** (actor 1): signing as another identity takes a deliberate,
  unusual act, not a typo.
- **Audit** (actors 2 and 3): a forgery leaves evidence that a routine guard
  reports, in a log the forger has to find and alter as well.
- **Revocation** (all): once a key is distrusted, nothing it signs outside
  its cut decides anything until the root rules on it, and the cut can't be
  widened.
- **Blast radius** (2 and 3, after discovery): a stolen token signs only what
  its role may, so a runner's credential can't approve a merge and a
  manager's can't mint a validator.
- **Publication** (4): no private key under the repository's working tree.
  Backups, snapshots and transcripts are still copies (question 3).

Steering (actor 2) is the case these help most: an injected instruction can
easily make an agent type `--as reviewer`, much less easily make it hunt for
a key file, sign by hand outside `scripts/bk`, and doctor a log.

## 3. Options

| | Option | Protocol | Accident | Audit | Revocation | Blast radius | Publication | Forging agent |
|---|---|---|---|---|---|---|---|---|
| A | Today: shared key folder | none | no | first-seen credential only | by decision text; old name still counts | none | keys under the working tree | free |
| B | Guard rails: `--as` must match `BRACKEN_AGENT`; credential checked against author | none | yes | mismatches reported | as A | none | as A | free |
| C | B, and the server holds the keys and signs | none (signing API reshaped) | yes | every signature logged; signing outside the server detected | token removed; old key still counts | per-role scope at signing | no key under the tree; backups still hold them | must read the server's key files and evade the log |
| D | C, and root-signed delegation and revocation records; verifiers resolve roles | none (opt-in acceptance field) | yes | as C, and roles public | root-set cut that only shrinks; out-of-cut records stop every reader they touch until the root rules on them | per-role scope; only the root grants gate roles; grants fall with their granter | as C | as C |
| E | D, with the root key held off this machine | none | yes | as D, and records an agent can't sign | as D | as D | as D | can't sign a root record, but can still fool every verifier on this machine (4.6) |
| F | Session authentication by agent key (sign a server challenge) | new signing context | as B | as B | as B | none | as A | free |
| G | Per-agent harness rules denying reads of key paths (Claude Code's sandbox or deny rules) | none | yes | as A | as A | yes, if they hold | as A | blocked only as far as the harness enforces it |

Notes on each:

- **B** costs a few lines and catches the typo case today. Binding a
  credential to an author has a catch: the main checkout's replica relays
  other agents' unpushed contributions under whichever token runs the next
  `scripts/bk`. Refusing those would strand an ended builder's pending
  writes, so B logs and reports mismatches rather than refusing them.
- **C** moves the keys, not the trust boundary: the server's directory is
  as readable as `.bracken/identities/`. Its gains are that the normal path
  never touches a key, that it can refuse to sign outside a role's scope, and
  that its signing log makes a forgery with a server-held key detectable.
  Writes need the server up; they already do, since an unconfirmed write
  isn't done (process 4.3).
- **D** makes roles and revocations data in the Ground instead of local
  configuration and decision prose, so every replica and every outside
  reader of the public Ground computes the same answer. It also closes a
  hole that exists today: a leaked key can retract its own past
  contributions, a reviewer's fail among them, and process 9.3's recovery
  can't stop that: after a revocation, such a retraction is pending, and the gate stops on it (4.4).
- **E** is D with the root key on a device an agent can't reach. It makes
  root records unforgeable, but only verifiers outside this machine's user
  benefit from that: the owner's device, an outside reader pinning the root
  from reviewed history. Section 4.6 says what it takes and what it buys.
  It needs a device and tooling, so it is the owner's call (question 1).
- **F** was deferred in network-sync.md for this reason: proving possession
  of a key every agent can read proves nothing. It is a protocol change for
  no gain here. Not proposed.
- **G** is the only option that could make reads of a key file fail for an
  agent's process. Whether Claude Code's deny rules and sandbox hold for
  every tool in the permission mode the team runs in isn't verified, and it
  is a harness setting, which this task leaves out. Listed as question 6.

**Recommended: D**, built in steps so each one pays on its own, with E as
the owner's choice for the root key.

## 4. The design

### 4.1 The server signs

`bracken serve` gains `--keys <dir>`: a directory of identity files, inside
the team directory (`~/.bracken-team/manyroot/keys/`, 0700), and a binding
from each credential to exactly one key. Keys are generated there
(`team-server key <name>`) and never copied out. A new request on the same
listener:

`POST /v1/<ground>/sign`, with the bearer token, and the contribution's
canonical `content` as the body. The server:

1. decodes `content` and refuses it unless it re-encodes to the same bytes
   (the same check as protocol.md, *Encoding*), its `ground` is the server's,
   its `protocol` is 1, and its `author` is the key bound to the token;
2. refuses it unless the body is within the key's scope (from `scopes.toml`
   in part A, 4.2, and from its grant in part B, 4.3), and, in part B, for a
   delegation granting a `held: server` key, unless that key is in the
   server's key directory;
3. computes the id, signs `"manyroot/v1/contribution:" || id` (the existing
   context, so nothing new can be signed), appends `{id, credential, time}`
   to `signed.jsonl`, and returns the signature.

The client assembles the contribution, adds it to its replica and pushes it
as now, so `scripts/bk`'s write-then-confirm flow is unchanged. The server
never signs a bare id: it signs only content it has read, which is what
lets it apply a scope.

**Detection.** `scripts/ground-guards`, at every merge and checkpoint,
reports:
- a contribution by a server-held key that isn't in `signed.jsonl`: someone
  signed with the key file;
- a counted grant (4.3) of a key that is neither in the server's key
  directory though its grant says `held: server` (part B): a key minted
  outside the server;
- a contribution first pushed under a credential bound to another key
  (step 1).

A forger can append to the log too, being the same user; to make rewriting
old entries detectable, the manager's checkpoint records the log's length
and BLAKE3 hash in a claim, so a log that no longer extends the recorded one
is reported. The guard itself runs on this machine, so it is audit, not a
barrier (section 2).

**In `manyroot`**, contribution creation takes a signer instead of an
`Identity`: a trait with `agent_id()` and `sign(&ContributionId)`, which
`Identity` implements and a remote signer implements over the request
above. No bytes change, and the protocol guard's golden vectors show it.

**Availability.** With the server down nothing can be signed. Writes
already fail to confirm then, and the merge gate already refuses, so what's
lost is only the unconfirmed local write.


### 4.2 Tokens, `--as` and scopes before grants

- `scripts/bk` refuses an `--as` that names a different identity from
  `BRACKEN_AGENT` when both are set. The agent sets its own `BRACKEN_AGENT`,
  so this guards against mistakes only.
- A token is bound to one key. In part A the binding is the server's
  configuration; in part B a role's current key is the one its current
  grant names (4.3), so agents keep writing `--as manager` and `scripts/bk`
  resolves the role to the key and its token.
- **Scopes in part A** come from a file in the team directory,
  `scopes.toml`, mapping each server-held key's name to a scope in the form
  of 4.3's table. The manager edits it; it is local configuration, as
  readable and writable by agents as the rest of the team directory, so it
  caps mistakes and stolen tokens, not a forging agent. In part B the
  server reads scopes from counted grants instead, and `scopes.toml` goes.
- **Retiring a key** is server-side: `team-server retire <name>` stops
  signing for it, drops its token, and the manager notes it in a claim. No
  record in the Ground is needed, so no builder scope has to allow one. A
  retired key's past contributions keep counting. Anything new by it isn't
  in `signed.jsonl`, so the guard reports it. In part B the root revokes
  retired keys in its next session (4.4), which turns that into a cut every
  reader computes.
- The `owner` token goes away if the root key leaves this machine (4.6).

Part A needs nothing else from this section or the next two: no delegation
record, no revocation, no change to how any reader counts. Its gains are
the accident guard, the signing log and its guard, scoped signing, and no
new key under the repository's working tree.

### 4.3 Delegation records (part B)

A delegation is an `artifact` with media type
`application/vnd.manyroot.delegation+cbor`, name `delegation`, and a
canonical CBOR payload pinned by golden vectors, like Weave's formats:

| Field | Type | Meaning |
|---|---|---|
| `v` | uint | `1` |
| `grants` | array of maps | One per key |
| `grants[].role` | text | A role name, `[a-z0-9_-]{1,64}` |
| `grants[].key` | AgentId | The key that acts as it |
| `grants[].held` | text | `server` (in the server's key directory), `owner` (on the owner's own device, 4.6) or `legacy` (a key from before the migration, 4.9) |
| `grants[].scope` | map | What it may sign: `kinds: [text]` (body kinds), `relations: [text]` (relation kinds), and for claims optionally `tags_only: [text]` or `tags_forbidden: [text]`; or `all` |
| `grants[].may_delegate` | array of text | Role patterns it may grant; empty for every role but the manager's |
| `note` | text | Why |

One record holds many grants, so the owner signs the whole team's setup
once, not twenty times.

**Gate roles come only from the root.** A gate role is one whose acts some
reader counts: `owner` (approvals), `manager` (it triages findings for the
merge gate and writes the Weave gate switch), `reviewer`, `security`,
`weave-runner` and `weave-integrator`. Only the root grants them;
`may_delegate` can never match one, and a grant that does is ignored. The
manager's `may_delegate` is `builder-*`, whose contributions no reader
counts as a role's act (a builder's `pr` claim is a subject, and its
evidence never counts toward its own claim). A stolen manager credential can
therefore mint builders, which can open PRs and write notes, but not a
reviewer whose pass merges one (finding `c20195623de6`). `triage` stays
unused until a reader counts it, and is then a gate role.

A grant **counts** when:
- its record is by the root key, or by a key whose own grant counts, is not
  revoked (4.4), and has a `may_delegate` matching the role;
- the granting key's own grant record is in the delegating record's causal
  past;
- it is `held: server`, or the root wrote it (only the root grants
  `held: owner` or `held: legacy`).

**Grants fall with their granter.** Revoking a key voids every grant it
made, inside its cut or not, unless the root re-grants the same key. So a
compromised manager's builders lose their role with it, and the new manager
re-grants the ones still working. Builder roles count for nothing in any
reader, so voiding them changes no decision.

**Server-held grants are checked against the server.** The server refuses
to sign a delegation granting a `held: server` key that isn't in its key
directory (4.1), and the guard reports any counted `held: server` grant of
a key that isn't there.

Chains are at most two deep: root, then manager, then builders. A key's
grant covers its whole history: keys are generated when they are granted,
so there is nothing before it, except the legacy keys of 4.9, whose grant
says so by its scope `all`.

**The root key's pin** is local configuration, like `--validator` names
today: a tracked file outside `.bracken/` (its path decided in the build),
which the merge gate, the server and `scripts/bk` read. A delegation,
revocation or ruling by any other key counts for nothing. Any agent can edit
that file, as it can edit the scripts that read it (section 2); 4.6 says
what can be done about that and what can't.

**Scope limits what the server signs and which acts count for a role. It
never changes author rules.** A key's retracts and supersessions of its own
contributions count by protocol.md's rules whatever its scope says, so a
scope can't revive a superseded finding or a retracted claim. Scopes for new
keys, checked against what each role has written in this Ground (relations
by kind, as of this round: reviewer 414 `contradicts`, 19 `refines`, 7
`supersedes`, 1 `supports`; security 346, 28, 4 and 0; builders 862
`implements`, 396 `refines`, 366 `supersedes`, 1 `contradicts`, 1
`supports`; the manager every kind; owner 16 `supports`):

| Role | Granted by | Kinds | Relations | Claim tags |
|---|---|---|---|---|
| `owner` (approvals) | root | claim, relation | `supports` | only `approval`, `delegated` |
| `manager` | root | all | all | any; may delegate `builder-*` |
| `builder-*` | root or manager | claim, relation, evidence, retract | `implements`, `refines`, `supersedes`, `supports`, `contradicts`, `depends_on`, `tests` | none of `approval`, `decision`, `task`, `owner-review`, `weave-gate` |
| `reviewer`, `security` | root | claim, relation, evidence, retract | `contradicts`, `supports`, `refines`, `supersedes`, `depends_on` | only `finding`, `note`, `process`, `question`, `report`, `gotcha`, `reference` |
| `weave-runner` | root | evidence, artifact (`text/plain` only) | none | none |
| `weave-integrator` | root | artifact (Weave media types), relation | `supersedes`, `supports`, `refines` | none |

Tags for owner, reviewer and security fit their claims so far (owner's 16
are all `approval` and `delegated`; reviewers' and security's are `finding`,
`note` or `process`). One builder claim is tagged `task`, which the builder
scope would refuse in future. **Legacy keys get scope `all`** (4.9), so
nothing already in the Ground falls outside a scope and every pre-cut status
is unchanged by construction.

### 4.4 Revocation (part B)

**Revocations and rulings.** Both are root-only artifacts:

- a revocation, media type `application/vnd.manyroot.revocation+cbor`:
  `{v: 1, keys: [AgentId], reason: text}`;
- a ruling, media type `application/vnd.manyroot.ruling+cbor`:
  `{v: 1, accept: [ContributionId], dismiss: [ContributionId], reason:
  text}`.

A revocation or ruling by any other key, the revoked key itself or its
granter included, counts for nothing (findings `459f57b58bb5`,
`e2663094ef89`). The manager stops a key server-side (4.2) and asks the root
for the revocation.

**A key's cut** is the intersection of the causal pasts of every counted
revocation of it: a contribution by the key is *inside* the cut when it is
an ancestor of every one of them. A second revocation can only shrink the
cut, never widen it. The root's tool takes its parents from a replica that
has just pulled from the server, all its heads, and refuses otherwise; but
parents are the writer's choice in the protocol, and the content is
assembled on this machine, so verifiers don't rely on the cut being
complete. Nothing below depends on it.

**Pending records.** A contribution by a revoked key that is outside its cut
is *pending* until a counted ruling names it. A ruling that accepts it makes
it count exactly as if it were inside the cut; one that dismisses it makes
it count as if it weren't in the Ground. If counted rulings both accept and
dismiss the same id, it stays pending. A ruling can also dismiss a record
inside the cut: the case of a forgery made before the revocation with the
key already leaked, a fail on a `pr` claim, say. (Without the root, such a
fail is escaped by a new head and a new `pr` claim, which it doesn't
target.)

**Fail closed.** No reader counts or ignores a pending record. Every
role-aware reader:
1. finds the pending records that *touch* the thing it decides (4.5 says
   what touching is for each);
2. if there are any, refuses to decide it, says so, and lists their ids,
   the record kind and the key;
3. decides normally once each has a ruling.

So it doesn't matter whether a record would have tightened or loosened the
answer, which is what both round 2 counterexamples turned on: a triage
`fail` that rejects a finding and so unblocks a PR (`e52cc9ba888e`), a
reviewer's `fail` on a superseding claim that would revive the claim it
replaced, and a manager's supersession "Weave gate: off" whose loss turns
the gate back on (`81d13d29f631`). Each is pending, and its reader stops.
The answer is a function of the graph and the pin alone, so replicas that
hold the same contributions agree.

**Pending records are kept, and flagged, everywhere** (findings
`0d839e105c44`, `39653d87385e`). The server's allowlist keeps every revoked
key, and its intake admits their contributions, inside the cut or not, as it
would any allowlisted key's, marking each one pending in its first-seen log.
The guard lists every pending record at each merge and checkpoint. So a
reviewer's real fail written while the root session revoking its key ran
reaches the server, blocks its PR's gate (the gate stops on it), and the
root accepts it by id; it isn't dropped, and no replica sees a different
answer from the server's for want of it. The intake's quotas bound how many
a leaked key can add: a revoked key's quota drops to a small number a day
(question 2), past which its pushes are refused and the guard says so.

**The price.** A leaked revoked key can stop any decision it can touch, by
writing records the root must rule on. That is a denial of service, loud and
attributable, not a silent merge. The root's rulings list ids it has read
(4.6), and the quota bounds the work.

**Rotation** is a new key granted for the role, then the old one revoked,
in the same root session. **Revocations ignore retraction**: retracting one
would undo its cut. A mistaken revocation is answered with a new grant of a
new key, and rulings that accept the old key's pending records.

### 4.5 Readers, and what touching means (part B)

A pending record *touches* a thing a reader decides when the reader would
read it in deciding that thing: it is the thing, targets it, is a relation
from or to it, or targets or relates to a record the reader reads for it.
For each reader:

| Reader | Decides | A pending record touches it when it is | Test (step 4) |
|---|---|---|---|
| `manyroot` acceptance, with `revoked` set | a subject's status | the subject itself; evidence on it; a relation from or to it; a retract of any of those or of it; or it touches a subject whose status this one depends on (a supersessor, or the other side of a `contradicts`) | a pending fail on a supersessor holds both the supersessor and the claim it replaced (`e52cc9ba888e` b); default policy unchanged on the fixture Ground |
| merge gate (`scripts/merge`) | a `pr` claim | touching the `pr` claim or its supersession chain; evidence on it; a finding that contradicts it, that finding's triage evidence, its links and their retracts; an approval of it or its retract; the Weave gate switch, if the run is Weave-gated | a pending triage fail on a finding stops the gate rather than rejecting the finding (`e52cc9ba888e` a); a pending reviewer fail stops it and, accepted, blocks; dismissed, doesn't |
| findings and triage (`bracken findings`, `log --open`) | a finding's triage state | the finding; the triager's evidence on it; its `contradicts` links and their retracts; a deferral task relation | a finding with a pending triage record shows `pending`, not `rejected` |
| owner approvals (`--owner-approval`, `weave-shadow --init`, `weave-backup`) | whether an approval stands | the approval; its retract; its `supports` relation | a pending retract of an approval stops the check |
| Weave gate switch (`scripts/weave-gate`) | on or off | any claim tagged `weave-gate`, a relation from or to one, a retract of either | a pending "Weave gate: off" supersession stops the gate, and git's full run is still available (`81d13d29f631`) |
| Weave views (`weave green`, `head`, decisions) | a manifest's green, a line's head | results on the manifest; decisions, line versions and objections on the line; their relations and retracts | a pending result stops `green` for that manifest; a pending decision stops `head` for that line |
| `scripts/bk log`, `show`, `--current` | nothing (reads) | n/a: they show pending records, marked, and decide nothing | a pending record is marked |

**In `manyroot`**, this is one opt-in policy field, `revoked` (each revoked
key and its counted revocation ids, with the counted rulings' accepted and
dismissed ids), and one new status, `held`, for a subject touched by a
pending record. A held subject is neither accepted nor rejected, and a
subject whose status depends on a held one is held too. It stays in the
graph and validity doesn't change. With the field empty, which is the
default, there are no pending records and every status is as today. It names
no format and no role; Bracken fills it from the counted revocations and
rulings.

**In Bracken:**
- `--validator <role>` and `--triager <role>` resolve to the keys with a
  counted grant for that role, with their revocations and rulings as
  `revoked`. A role whose key rotated keeps its name, so the gate's commands
  and the role briefs don't change at each rotation, unlike process 9.3's
  rename. `scripts/merge` passes `--triager manager` as it does today.
- `scripts/merge --owner-approval` requires an approval by a key granted
  `owner`, within its scope, and not held.
- `bracken` names: `.bracken/agents/<name>.json` stays one key per name. New
  keys get names `<role>-k<n>` (`manager-k2`); roles are what people type.

**Author rules across a rotation.** Retraction and supersession stay tied
to the key, so a new key can't retract or supersede what the old one wrote.
Process 9.4's handover already deals with that for the manager (the old
identity writes the supersession while its key exists). A pending
supersession or retraction by the old key is ruled on by the root like any
other pending record. A role-level version of the author rules would change
protocol.md's derived state for everyone and isn't proposed.

### 4.6 The owner's root key

The root key signs grants and revocations, a few times a month, in
sessions. Where it lives decides what its signature means, but **no
placement makes this machine's verifiers trustworthy**: the root's pin, the
scripts that read it and the binary are files any agent can edit. An agent
that wants a delegation of its own needn't forge the root's signature; it
can swap the pin for its own key, and every verifier on this machine then
accepts its records (finding `9328d3a71afd`).

**On this machine** (option D alone): a file only the owner's tool reads,
outside the server's key directory, so the normal path never touches it.
Root records then mean what owner-signed approvals mean today: the owner's
intent, with an audit trail.

**Off it** (option E): on a device the owner holds. For that to be worth
having, three things are needed:
1. **The device shows what it signs.** The content is assembled on this
   machine, where an agent could swap a granted key, add a role, slip an id
   into a ruling or narrow the parents. So the device decodes the payload and
   shows each grant (role, key prefix, scope), each revoked key, each
   accepted and dismissed id with the record it names (kind, key, target), and the number of parents and the heads they name, against
   the server's current heads, and the owner confirms that. A device that
   signs an opaque 32-byte id shows nothing and protects nothing.
2. **What comes back is checked.** The returned bundle must hold exactly
   one contribution, with the expected id, authored by the root, and
   verifying; `scripts/bk` refuses anything else.
3. **The pin is checked where agents can't edit it.** The device keeps its
   own copy of the root key and of the team's server-held keys as last
   confirmed, and checks a bundle of the Ground (or of the guard's report)
   against them: grants it didn't sign, a pin that changed, keys on the
   allowlist it doesn't know. Outside readers pin the root from reviewed
   history in the public repository; a change to the pin is a protocol-class
   PR with the owner's go, visible to anyone. GitHub Actions can re-run
   that check on every push, as an observer with no secrets, though the
   manager can still merge what it reports.

What E then buys: root records no agent can sign; a check of the Ground
that runs out of agents' reach; outside readers who can trust the public
Ground's roles from reviewed history. What it doesn't: the merge gate, the
server and `scripts/bk` on this machine can still be pointed at another root
by an agent that edits them. That shows up on the device's next check, not
before.

Which device can do all three isn't settled. Hardware keys that sign Ed25519
generally show nothing about the message, so the likely form is a small
signing tool on a second computer or phone the owner already has, with the
bundle and signature carried across as files or QR codes. It needs checking
before it's chosen (question 1).

Either way, approvals, which are frequent, can keep an `owner` key on this
machine, granted by the root with the approvals-only scope. A stolen
approvals key can then sign approvals, as today, but can't grant a key,
revoke one or rule on a pending record, and the root can revoke it.

### 4.7 The runner key

`weave-runner` gets its own server-held key, granted by the root with the
`weave-runner` scope: evidence and its log artifacts, nothing else. Its
token is used only by `scripts/weave-gate`'s validate step. A stolen runner
token then signs check results and nothing more, and the signing log shows
each one. It can't approve, decide or review. That is the "runner key only
weave-runner's process uses" of weave.md section 8, as far as one user
allows: a convention of which process asks, enforced by scope and visible
in the log, not a process boundary.

This applies once Weave writes to a Ground the team server serves, which is
the join. The dogfood Ground isn't served, and serving it is new
infrastructure (weave.md section 5), so until the join its runner stays a
shared key, as documented (question 5).


### 4.8 Protocol impact

| Part | Class | Ground change |
|---|---|---|
| A: signing through a trait instead of `Identity` (`identity.rs`, `contribution.rs`) | protocol (review): the files are protocol class, though no byte changes | none; golden vectors unchanged |
| A: `POST /sign`, `--keys`, credential binding, `scopes.toml`, retirement | cli, merge-gate tooling | none |
| B: delegation, revocation and ruling payload formats | pinned like Weave's, reviewed as protocol class | new artifacts only; an older binary sees artifacts |
| B: `revoked` and `held` in acceptance (protocol.md, *Acceptance*) | protocol (review): opt-in derived state, like `follow_changes` | none: default unchanged |
| B: role resolution and fail-closed readers in `scripts/bk`, `scripts/merge`, `scripts/weave-gate`, the allowlist | merge-gate tooling | none |

No encoding, id, signing context or validity rule changes, so the protocol
guard's vectors and the fixture Ground stay as they are, and every existing
Ground opens. A release note in docs/releases.md is still owed for the
acceptance field and the new formats.

### 4.9 Migrating this Ground

Nothing is re-signed: re-signing would change every id (`ground` and
`author` are in the signed content) and break every reference.

**After part A**, nothing about the Ground changes: new identities are
created in the server's key directory instead of `.bracken/identities/`,
and the legacy keys keep signing as they do, from their files. Moving a live
legacy identity onto the server would mean copying its key, which this
design doesn't do; it rotates instead, in part B.

**Part B**, in order:

1. **Root key.** The owner creates it where question 1 decides, and the
   repository's pin names its public key, in a reviewed PR.
2. **New keys, server-held.** The server generates one key per live role in
   its key directory (`manager-k2`, `reviewer-k2`, …). Nothing grants them
   yet, so no reader counts them. Creating them is the owner's go (decision
   `adcb78b88c6c`), asked with question 2.
3. **Handovers.** At a quiet point, with no PR between review and merge,
   the legacy keys do their last handovers (process 9.4: the manager's
   claims, open tasks the new keys will carry on).
4. **One root session**, its records written in this order, each a child of
   the last, and pushed together:
   1. the legacy grant: each of the 20 legacy keys its current name as its
      role, `held: legacy`, with scope `all` and **no
      `may_delegate`**, so no legacy key can grant anything (finding
      `263004ff8abd`), and nothing already in the Ground falls outside a
      scope;
   2. the new grant: the new server-held keys, by the root, with 4.3's
      scopes;
   3. the revocation of all 20 legacy keys. Its parents are the session's
      replica's heads, freshly pulled.

   Every pre-cut status is unchanged: legacy records inside the cut count as
   before, under scope `all`. The only time legacy grants count without a cut
   is between the first record and the third, inside one session, and a
   legacy key can grant nothing during it. Anything a legacy key wrote
   concurrently with the session is pending, and the readers it touches stop
   until the root rules on it in a follow-up session. Role-aware reads
   (build step 4) go live with the session, not before.
5. **The old files.** With the owner's go, the legacy key files can be
   deleted from `.bracken/identities/`, so nothing on the normal path reads
   them. That doesn't make them leave the machine: Time Machine backups,
   APFS snapshots and any transcript that showed one keep copies. After the
   cut, what a legacy key can still do with such a copy is add pending
   records, which stop the readers they touch until the root rules on them,
   within the revoked keys' quota.

Open `pr` claims written before the cut keep their evidence. Builders started
after the cut get server-held keys from their first contribution.

### 4.10 Weave, the freeze and the join

- **The freeze doesn't depend on this.** Line policies keep naming keys.
  Rotating the runner or the integrator is a new policy version under the
  owner's decision, as any policy change is. No Weave format changes, so
  step 3 of decision `b4a3456de597` can go ahead in parallel.
- **Weave's views fail closed** on pending records, as 4.5's table says:
  `green` refuses for a manifest a pending result or objection touches, and
  `head` for a line a pending decision or line version touches. That is
  derived state in `crates/tree` (full table, protocol-class review), with
  no format change, and it should land before the join.
- **The join depends on it.** At the join, `weave-integrator` and
  `weave-runner` begin writing to the coordination Ground. Created there
  after part B, they are server-held, root-granted and scoped from their
  first contribution, so Weave's author rules (integrator, validator, line
  author) become as strong as the rest of the Ground's, and the summary's
  caveat in weave.md shrinks to section 2's same-user limit. Joined before,
  they would start as shared keys and need rotating at once. The join
  doesn't need part B to be finished in every detail, only steps 3 and 4.
- **`owner_decision`** in a line policy names a coordination claim; after
  the join, `scripts/weave` checks it as an approval by a key granted
  `owner`.

## 5. Build plan

Two parts. **Part A (steps 1 and 2) stands alone**: it needs no delegation,
revocation or ruling record and changes no reader's counting, so it can go
ahead even if part B needs more design. **Part B (steps 3 to 5)** builds on
A. A step that touches `crates/manyroot` or `crates/tree/src/` takes the
full table (`scripts/code-scope full`).

| # | Part | PR | Class | Tests |
|---|---|---|---|---|
| 1 | A | `scripts/bk` refuses `--as` ≠ `BRACKEN_AGENT`; `scripts/ground-guards` reports contributions first pushed under another identity's credential (the first-seen log already records both) | merge-gate tooling | mismatch refused; a relayed contribution reported, not refused |
| 2 | A | Signer trait in `manyroot`; `bracken serve --keys`, credential binding, `POST /sign` with canonical-content, author and scope checks, scopes from `scopes.toml` (4.2); `signed.jsonl`; `team-server key` and `retire`; `scripts/bk` signing through it; new identities created on the server; the guard for contributions by server-held keys missing from the log | protocol (review), cli; **full table** (touches `crates/manyroot`) | golden vectors unchanged; a token can't sign as another key; non-canonical content, a foreign Ground, another author, a body outside `scopes.toml`'s scope refused; a retired key refused; a contribution signed with the key file directly reported |
| 3 | B | Delegation, revocation and ruling formats with golden vectors; grant counting (root-only gate roles, chains, `held`, scopes, grants voided with their granter); root-only revocations and rulings; the intersection cut; the server reading scopes from grants instead of `scopes.toml`; the delegation check (keys in its directory); the allowlist keeping revoked keys, the intake marking pending records, the revoked keys' quota | protocol (review); full table if the formats live in `crates/manyroot` or `crates/tree/src/` | each counting case; a manager's grant of `reviewer` ignored; a manager's grant of a key outside the key directory refused at signing and reported; a revoked manager's builder grants void; a revocation or ruling by the revoked key or its granter ignored; a second revocation narrowing, never widening, the cut; a revoked key's out-of-cut fail admitted by the server and marked pending (`0d839e105c44`, `39653d87385e`); conflicting rulings leave a record pending |
| 4 | B | `revoked` and `held` in acceptance; role resolution for `--validator`, `--triager`, `--owner-approval`; every reader of 4.5's table failing closed; Weave's views failing closed | protocol (review); **full table** | each row of 4.5's table; and the round 1 and round 2 counterexamples: a self-revocation leaving out a reviewer's fail ignored (`459f57b58bb5`), a second revocation by the revoked key ignored (`e2663094ef89`), a pending triage fail stops the gate (`e52cc9ba888e` a), a pending fail on a supersessor holds the replaced claim too (`e52cc9ba888e` b), a pending "Weave gate: off" stops the gate (`81d13d29f631`), a real concurrent reviewer fail, accepted by ruling, blocks (`39653d87385e`); an in-cut forged fail dismissed by ruling stops counting |
| 5 | B | The migration (4.9), run by the manager and the owner, with the owner's go | process | every pre-cut status unchanged (scope `all`, the refines and supersedes of every role included, `35c17799be7a`); the gate's commands by role; no legacy key can grant |

Docs that change with them: process.md sections 2 and 9.3, development.md
*Owner approvals* and *Team server*, the README's *Not done yet*,
network-sync.md section 3, weave.md's summary caveat, and AGENTS.md's
`.bracken/identities/` line.

## 6. Questions for the owner

Part A needs only question 2's first half; part B needs the rest.

1. **Where does the root key live?** (a) On this machine: root records
   (grants, revocations, rulings) record your intent with an audit trail,
   as approvals do today, and an agent could forge them. (b) Off it, on a
   second computer or phone you already have, running a small signing tool
   that shows each grant, revocation and ruling before you confirm, and
   keeps its own check of the Ground. Root records then can't be forged, and
   a check runs out of agents' reach, but this machine's merge gate can
   still be pointed at another root by an agent that edits it; the device
   catches that on its next check, not before. It costs a session by hand a
   few times a month, plus one whenever a revoked key's records need a
   ruling. Recommended: (b), if you have a device for it; the design works
   either way. Should approvals stay on this machine under a root-granted
   approvals-only key (recommended), or also go off it?
2. **New keys, and the cut's terms.** (Part A) May the team server generate
   and hold new agents' keys from now on, in its own directory (your go on
   credentials, under `adcb78b88c6c`)? (Part B) May the 20 legacy key files
   be deleted from `.bracken/identities/` after the cut, and do you accept
   the cut's terms: after it, anything a legacy key signs outside the cut
   stops every decision it touches until you rule on it by id (accept or
   dismiss); a leaked legacy key can make you do that work, up to a quota
   of, say, 20 records a day per revoked key?
3. **Copies of keys.** Time Machine, APFS snapshots and agent transcripts
   keep copies of every key file, deleted or not. Exclude the server's key
   directory from Time Machine (a setting on this machine), accepting that
   losing the machine means re-creating and re-granting those keys?
4. **The threat model.** Do you accept section 2's statement: on one user,
   against an agent that sets out to forge, signatures remain intent plus
   audit, revocation that fails closed, and a capped blast radius, not
   proof; an off-machine root adds unforgeable root records and a check out
   of agents' reach, not a trustworthy gate on this machine?
5. **The dogfood Ground.** Keep its shared keys until the join
   (recommended), or serve it so its runner signs through a server now
   (new infrastructure)?
6. **Harness read rules (G).** Would you like a separate task to test
   whether Claude Code's deny rules or sandbox can keep agents' tools from
   reading the server's key directory, in the permission mode the team uses?
   It's a harness setting, outside this task, and unverified.

## 7. Not proposed

- A dedicated macOS user (decisions `91f610a624fc`, `6f15365a7281`).
- Keychain or other OS settings (out of this task's scope; question 3 only
  asks about a backup exclusion).
- Session authentication by agent key (option F).
- Revocations or rulings by anyone but the root, and cuts that a later
  revocation could widen.
- Counting or ignoring a revoked key's out-of-cut records by kind or
  verdict: round 2 showed no such rule is safe for every reader.
- Role-level authorship for retraction and supersession (4.5).
- Membership and permissions as protocol rules (Stewards): delegation here
  is Bracken's local policy over plain artifacts, which may move into
  `manyroot` once it has been used, as weave.md does with its parts.
