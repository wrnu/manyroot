# Per-agent keys and delegation

Design only, no code (task `21e1c394ca2d`, step 2 of the Weave order in
decision `b4a3456de597`; [weave.md](weave.md) section 8). It compares the
options for making a signature in the Ground mean more than "whoever claimed
to act", recommends one, and lists what the owner needs to decide.

The constraint everything here works within: **every agent runs as the
owner's one macOS user**, and there will be no dedicated user (decisions
`91f610a624fc` and `6f15365a7281`). Keychain, other OS settings and creating
users are out of scope for this task.

Round 2 (brief `4d56e3db211c`) answers the seven findings on the first
draft: the revocation cut (`459f57b58bb5`, `e2663094ef89`), delegation by a
stolen manager credential (`c20195623de6`), the migration window
(`263004ff8abd`), what an off-machine root buys (`9328d3a71afd`), the
threat model's gaps (`d49dc7939051`) and the server's intake of revocations
(`bd43398b3a0f`).

## Summary

- **Against an agent that sets out to forge, nothing on this machine makes
  a signature proof.** Any agent with a shell can read every file the
  owner's user can, keys and tokens included, wherever they live, and can
  edit every verifier on this machine. Even a root key held off the machine
  only protects verifiers that are also out of agents' reach (section 4.6).
  The design says so plainly (section 2).
- **What it can improve is real, though:** accidents (signing as the wrong
  identity), detection (a forgery leaves a trace that a guard reports),
  revocation (a distrusted key stops counting, and no one can use a
  revocation to unblock anything), blast radius (a stolen credential signs
  only what its role may, and can't mint validators), and the chance of
  publishing a key through the public repository.
- **Recommended: option D** (section 3), built in steps (section 5):
  1. **Guard rails, no format change.** `scripts/bk` refuses an `--as` that
     differs from `BRACKEN_AGENT`; a guard reports contributions first
     pushed under another identity's credential.
  2. **The team server signs.** New keys are generated in the server's
     private directory and never leave it. An agent holds a token, never a
     key. The server signs only as the key its token is bound to, only what
     that key's role may sign, and logs every signature. A contribution by a
     server-held key that isn't in the log was signed by someone who read
     the key file: the guard reports it.
  3. **Delegation and revocation records.** The owner's root key alone
     grants the roles whose acts gates count (reviewer, security, owner,
     weave-runner); the manager may grant only builder and triage keys,
     only to server-held keys, and its grants fall with it. The root alone
     revokes. A key's cut can only shrink, and **a revocation never
     unblocks**: outside its cut a revoked key's passes, approvals,
     retractions and supersessions count for nothing, while its fails and
     contradictions still block until the root sets them aside by id.
  4. **Verifiers resolve roles**: the merge gate's `--validator`, the owner-
     approval check, the server's allowlist and Weave's views.
- **No protocol change.** Ids, encoding, the signing context and the
  validity rules stay as they are; every existing contribution still
  verifies, and an older binary opens the Ground and sees the new records as
  artifacts. Protocol-class review is still needed: `manyroot`'s signing API
  changes shape (`identity.rs`, `contribution.rs`) without changing a byte,
  and acceptance gains an opt-in policy field for revoked keys (protocol.md,
  *Acceptance*), whose default changes nothing.
- **Migration re-signs nothing.** In one root session the owner grants the
  20 legacy keys their roles with no right to delegate, grants the new
  server-held keys, and revokes the legacy keys, in that order, so the
  window in which legacy grants count without a cut is the length of one
  signing session. About 11,700 contributions stay exactly as they are. The
  legacy key files can be deleted, but copies remain in backups, snapshots
  and transcripts, so they never really leave the machine.
- **The join should wait for steps 2 to 4**, so Weave's identities in the
  coordination Ground are server-held and delegated from their first
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
  its cut can unblock anything, and the cut can't be widened.
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
| D | C, and root-signed delegation and revocation records; verifiers resolve roles | none (opt-in acceptance field) | yes | as C, and roles public | root-set cut that only shrinks; a revocation never unblocks | per-role scope; only the root grants gate roles; grants fall with their granter | as C | as C |
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
  can't stop that (4.5).
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
  is a harness setting, which this task leaves out. Listed as question 5.

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
2. refuses it unless the body is within the role's scope (4.3), and, for a
   delegation, unless every key it grants is in the server's key directory;
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
  directory nor the root or a key the root granted as off-machine: a key
  minted outside the server;
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

### 4.2 Tokens and `--as`

- `scripts/bk` refuses an `--as` that names a different identity from
  `BRACKEN_AGENT` when both are set. The agent sets its own `BRACKEN_AGENT`,
  so this guards against mistakes only.
- A token is bound to one key. A role's current key is the one its current
  grant names (4.3), so agents keep writing `--as manager`; `scripts/bk`
  resolves the role to the key and its token.
- **Retiring a key** is server-side: `team-server retire <name>` stops
  signing for it, drops its token and takes it off the allowlist, and the
  manager notes it in a claim. No record in the Ground is needed, so no
  builder scope has to allow one. A retired key's past contributions keep
  counting; anything new by it is refused at the door, and, if it turns up
  in a replica anyway, isn't in `signed.jsonl`, so the guard reports it. The
  root revokes retired keys in its next session (4.4) to make that a cut
  every reader computes.
- The `owner` token goes away if the root key leaves this machine (4.6).

### 4.3 Delegation records

A delegation is an `artifact` with media type
`application/vnd.manyroot.delegation+cbor`, name `delegation`, and a
canonical CBOR payload pinned by golden vectors, like Weave's formats:

| Field | Type | Meaning |
|---|---|---|
| `v` | uint | `1` |
| `grants` | array of maps | One per key |
| `grants[].role` | text | A role name, `[a-z0-9_-]{1,64}` |
| `grants[].key` | AgentId | The key that acts as it |
| `grants[].scope` | map | What it may sign: `kinds: [text]` (body kinds), and for claims optionally `tags_only: [text]` or `tags_forbidden: [text]` |
| `grants[].may_delegate` | array of text | Role patterns it may grant; empty for every role but the manager's |
| `note` | text | Why |

One record holds many grants, so the owner signs the whole team's setup
once, not twenty times.

**Gate roles come only from the root.** The roles whose acts a gate counts,
`reviewer`, `security`, `owner`, `weave-runner` and `weave-integrator`, are
granted only by the root key. `may_delegate` can never match one of them;
a grant that does is ignored. The manager's `may_delegate` is `builder-*`
and `triage`, whose contributions no gate counts. A stolen manager
credential can therefore mint builders, which can open PRs and write notes,
but not a reviewer whose pass merges one (finding `c20195623de6`).

A grant **counts** when:
- its record is by the root key, or by a key whose own grant counts, is
  not revoked (4.4), and has a `may_delegate` matching the role;
- the granting key's own grant record is in the delegating record's causal
  past.

**Grants fall with their granter.** Revoking a key voids every grant it
made, inside its cut or not, unless the root re-grants the same key. So a
compromised manager's builders lose their role with it, and the new
manager re-grants the ones still working. Builder roles count for nothing
in a gate, so voiding them changes no gate's past answers.

**Only server-held keys are granted by the manager.** The server refuses to
sign a delegation for a key outside its key directory (4.1), and the guard
reports any counted grant of such a key. Keys outside the server are
granted only by the root: the owner's off-machine keys, if any (4.6).

Chains are at most two deep: root, then manager, then builders. A key's
grant covers its whole history: keys are generated when they are granted,
so there is nothing before it.

**The root key's pin** is local configuration, like `--validator` names
today: a tracked file outside `.bracken/` (its path decided in the build),
which the merge gate, the server and `scripts/bk` read. A delegation or
revocation by any other key counts for nothing. Any agent can edit that
file, as it can edit the scripts that read it (section 2); 4.6 says what
can be done about that and what can't.

**Scopes**, as first set:

| Role | Granted by | Scope |
|---|---|---|
| `owner` (approvals) | root | claims tagged only `approval`, `delegated`; `supports` relations |
| `manager` | root | everything; may delegate `builder-*`, `triage` |
| `builder-*`, `triage` | root or manager | claims with no `approval`, `decision`, `task` or `owner-review` tag; relations; evidence; retracts |
| `reviewer`, `security` | root | claims tagged `finding`, `note`, `process`, `question`, `report`; `contradicts` and `supports`; evidence; retracts |
| `weave-runner` | root | evidence; `text/plain` artifacts (its logs) |
| `weave-integrator` | root | Weave artifacts; relations |

The server refuses to sign outside a scope; verifiers don't count a
contribution outside its signer's scope as an act of the role. Scopes are
coarse on purpose: they cap a stolen credential, they don't encode the
process. They fit what the Ground holds: every owner claim so far is tagged
`approval` (16, all also `delegated`); reviewers' and security's claims are
tagged `finding`, `note` or `process`; builders' claims carry no `decision`,
`approval` or `owner-review` tag, and one carries `task`, which the scope
would refuse in future. No scope but the manager's allows delegation or
revocation artifacts, and none needs to: retirement is server-side (4.2).

### 4.4 Revocation

A revocation is an `artifact` with media type
`application/vnd.manyroot.revocation+cbor`:

| Field | Type | Meaning |
|---|---|---|
| `v` | uint | `1` |
| `keys` | array of AgentId | The keys revoked |
| `dismiss` | array of ContributionId | Fails and contradictions by these keys, outside their cut, that the root sets aside (below); usually empty |
| `reason` | text | Why |

**Only the root revokes.** A revocation by any other key, the revoked key
itself or its granter included, counts for nothing. The manager stops a key
server-side (4.2) and asks the root for the revocation. This takes away the
lever in findings `459f57b58bb5` and `e2663094ef89`: a holder of a stolen
reviewer or manager key can't write a revocation that counts.

**A key's cut** is the intersection of the causal pasts of every counted
revocation of it: a contribution by the key is *inside* the cut when it is
an ancestor of every one of them. A second revocation can only shrink the
cut, never widen it, so there is nothing to gain by writing one later.

**The root's tool must take its parents from a replica that has just pulled
from the server**, all its heads, and refuses otherwise. Nothing in the
protocol enforces that (parents are the writer's choice), and the content is
assembled on this machine, where an agent could narrow them before the
owner signs. So verifiers don't trust the cut to be complete. They make a
narrow cut harmless instead:

**A revocation never unblocks.** Outside its cut, a revoked key's
contributions that would let something through count for nothing: its
`pass` evidence, its retracts, its `supersedes`, `supports`, `implements`,
`refines`, `depends_on` and `tests` relations, its grants and its claims and
artifacts as subjects that could be accepted. Its contributions that can
only hold something back keep counting: its `fail` evidence and its
`contradicts` relations. So:
- a reviewer's fail stays a fail whatever cut the revocation draws: inside
  the cut it counts as before; outside, it still blocks;
- a forged retraction of that fail, made after the revocation with
  backdated parents, is outside the cut and retracts nothing;
- a narrowed cut can only take away the key's own passes and retractions,
  which makes the gate stricter, never looser.

The price: a leaked revoked key can add fails that block. The server refuses
them (below), and the guard lists any that turn up in a replica. The root
lifts one only by naming its id in a later revocation's `dismiss`, which the
owner reads before signing (4.6). That turns the leaked key's worst case
from a silent merge into a loud, visible block.

What it gives, and doesn't:
- Every replica that holds the same revocations computes the same cut, with
  no timestamp involved, as protocol.md's goal 2 asks.
- A forgery the revoker had already seen stays inside the cut. As today
  (process 9.3), everything a leaked key signed is suspect; the server's
  first-seen log and signing log say which contributions the server signed
  and when they arrived.
- A legitimate contribution the revoked key wrote concurrently, outside the
  cut, is re-recorded under the new key if it was a pass; a fail keeps
  counting anyway.
- Revocations ignore retraction: retracting one would revive what it cut
  off. A mistaken revocation is answered with a new grant of a new key.

**Rotation** is a new key granted for the role, then the old one revoked,
in the same root session.

**The server's intake** (finding `bd43398b3a0f`): the allowlist admits a
revoked key's contributions that are inside its cut, so a revocation whose
parents include one the server lacks still arrives with it, and refuses
only contributions outside the cut. Contributions by a key that has been
stopped (4.2) but not yet revoked are refused unless they are already in
`signed.jsonl`. The root key and the owner's granted keys are always on
the allowlist.

### 4.5 Verifiers

**In `manyroot`**, acceptance gains one opt-in policy field, `revoked`: a
map from a key to a set of revocation ids, and a set of dismissed ids. A
contribution by a revoked key is inside its cut when it is an ancestor of
every id in the key's set. For that policy, outside the cut:
- `pass` evidence doesn't count; `fail` evidence counts unless dismissed;
- a retract retracts nothing;
- a `contradicts` relation counts unless dismissed; every other relation
  kind counts for nothing;
- a claim or artifact is not a subject that can be accepted.

It stays in the graph, and validity doesn't change. With the field empty,
which is the default, every status is as today. It is generic (it names no
format and no role), and Bracken fills it from the counted revocations. It
closes the retraction hole for every policy that uses it, by construction:
no choice of cut makes a revoked key's fail stop counting or its forged
retraction start.

**In Bracken:**
- `--validator <role>` resolves to the keys with a counted grant for that
  role, with their revocations as `revoked`. A role whose key rotated keeps
  its name, so the gate's commands and the role briefs don't change at each
  rotation, unlike process 9.3's rename.
- `scripts/merge --owner-approval` requires an approval by a key granted
  `owner`, within scope and not outside a cut.
- `scripts/bk log`, `show` and `--current` apply the revocations by default
  and mark contributions they ignore.
- `bracken` names: `.bracken/agents/<name>.json` stays one key per name.
  New keys get names `<role>-k<n>` (`manager-k2`); roles are what people
  type.

**Author rules across a rotation.** Retraction and supersession stay tied
to the key, so a new key can't retract or supersede what the old one wrote.
Process 9.4's handover already deals with that for the manager (the old
identity writes the supersession while its key exists). A role-level
version would change protocol.md's derived state for everyone and isn't
proposed.

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
   machine, where an agent could swap a granted key, add a role, add a
   dismissal or narrow the parents. So the device decodes the payload and
   shows each grant (role, key prefix, scope), each revoked key and
   dismissed id, and the number of parents and the heads they name, against
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
approvals key can then sign approvals, as today, but can't grant a key or
dismiss a fail, and the root can revoke it.

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
shared key, as documented (question 4).

### 4.8 Protocol impact

| Part | Class | Ground change |
|---|---|---|
| Signing through a trait instead of `Identity` (`identity.rs`, `contribution.rs`) | protocol (review): the files are protocol class, though no byte changes | none; golden vectors unchanged |
| `revoked` in acceptance (protocol.md, *Acceptance*) | protocol (review): opt-in derived state, like `follow_changes` | none: default unchanged |
| Delegation and revocation payload formats | pinned like Weave's, reviewed as protocol class | new artifacts only; an older binary sees artifacts |
| `POST /sign`, `--keys`, credential binding, retirement | cli, merge-gate tooling | none |
| Role resolution in `scripts/bk`, `scripts/merge`, the allowlist | merge-gate tooling | none |

No encoding, id, signing context or validity rule changes, so the protocol
guard's vectors and the fixture Ground stay as they are, and every existing
Ground opens. A release note in docs/releases.md is still owed for the
acceptance field and the new formats.

### 4.9 Migrating this Ground

Nothing is re-signed: re-signing would change every id (`ground` and
`author` are in the signed content) and break every reference. In order:

1. **Root key.** The owner creates it where question 1 decides, and the
   repository's pin names its public key, in a reviewed PR.
2. **New keys, server-held.** The server generates one key per live role in
   its key directory (`manager-k2`, `reviewer-k2`, …). Nothing grants them
   yet, so nothing counts them. Creating them is the owner's go (decision
   `adcb78b88c6c`), asked with question 2.
3. **Handovers.** At a quiet point, with no PR between review and merge,
   the legacy keys do their last handovers (process 9.4: the manager's
   claims, open tasks the new keys will carry on).
4. **One root session**, its records written in this order, each a child of
   the last, and pushed together:
   1. the legacy grant: each of the 20 legacy keys its current name as its
      role, with its scope and **no `may_delegate`**, the manager's
      included, so no legacy key can grant anything (finding
      `263004ff8abd`). Their grants cover their whole history, so
      everything already in the Ground counts as before;
   2. the new grant: the new server-held keys, by the root;
   3. the revocation of all 20 legacy keys, with no dismissals. Its parents
      are the session's replica's heads, freshly pulled.

   The only time legacy grants count without a cut is between the first
   record and the third, inside one session, and a legacy key can grant
   nothing during it. Role-aware reads (step 4 of the build) go live with
   the session, not before.
5. **The old files.** With the owner's go, the legacy key files can be
   deleted from `.bracken/identities/`, so nothing on the normal path reads
   them. That doesn't make them leave the machine: Time Machine backups,
   APFS snapshots and any transcript that showed one keep copies. After the
   cut, what a legacy key can still do with such a copy is add fails that
   block (4.4), which the server refuses and the guard reports.

Open `pr` claims written before the cut keep their evidence. Builders started
after the cut get server-held keys from their first contribution.

### 4.10 Weave, the freeze and the join

- **The freeze doesn't depend on this.** Line policies keep naming keys.
  Rotating the runner or the integrator is a new policy version under the
  owner's decision, as any policy change is. No Weave format changes, so
  step 3 of decision `b4a3456de597` can go ahead in parallel.
- **Weave's views apply revocations** with the same rule as 4.5: outside a
  cut, a revoked key's passing results, decisions and line versions count
  for nothing, while its failing results and objections still hold back.
  That is derived state in `crates/tree` (full table, protocol-class
  review), with no format change, and it should land before the join.
- **The join depends on it.** At the join, `weave-integrator` and
  `weave-runner` begin writing to the coordination Ground. Created there
  after steps 2 to 4, they are server-held, root-granted and scoped from
  their first contribution, so Weave's author rules (integrator, validator,
  line author) become as strong as the rest of the Ground's, and the
  summary's caveat in weave.md shrinks to section 2's same-user limit.
  Joined before, they would start as shared keys and need rotating at once.
- **`owner_decision`** in a line policy names a coordination claim; after
  the join, `scripts/weave` checks it as an approval by a key granted
  `owner`.

## 5. Build plan

Each step is one PR and pays without the next. A step that touches
`crates/manyroot` or `crates/tree/src/` takes the full table
(`scripts/code-scope full`).

| # | PR | Class | Tests |
|---|---|---|---|
| 1 | `scripts/bk` refuses `--as` ≠ `BRACKEN_AGENT`; `scripts/ground-guards` reports contributions first pushed under another identity's credential (the first-seen log already records both) | merge-gate tooling | mismatch refused; a relayed contribution reported, not refused |
| 2 | Signer trait in `manyroot`; `bracken serve --keys`, credential binding, `POST /sign` with canonical-content, author and scope checks; `signed.jsonl`; `team-server key` and `retire`; `scripts/bk` signing through it; the guard for contributions by server-held keys missing from the log | protocol (review), cli; **full table** (touches `crates/manyroot`) | golden vectors unchanged; a token can't sign as another key; non-canonical content, a foreign Ground, another author, an out-of-scope body refused; a retired key refused; a contribution signed with the key file directly reported |
| 3 | Delegation and revocation formats with golden vectors; grant counting (root-only gate roles, chains, scopes, grants voided with their granter); root-only revocations and the intersection cut; the server's delegation check (keys in its directory) and allowlist from grants and cuts | protocol (review); full table if the formats live in `crates/manyroot` or `crates/tree/src/` | each counting case; a manager's grant of `reviewer` ignored; a manager's grant of a key outside the key directory refused at signing and reported; a revoked manager's builder grants void; a revocation by the revoked key or its granter ignored; a second revocation narrowing, never widening, the cut; the server admitting a revoked key's in-cut contribution and its revocation, refusing an out-of-cut one |
| 4 | `revoked` in acceptance; role resolution for `--validator`, `--owner-approval` and reads; Weave's views applying revocations | protocol (review); **full table** | a reviewer's fail outside a narrowed cut still blocks; a forged retraction of a fail, after the revocation with backdated parents, retracts nothing; a self-revocation by the reviewer key that leaves out its fail ignored (`459f57b58bb5`'s case); a second revocation by the revoked key with its forgery among the parents ignored (`e2663094ef89`'s case); an out-of-cut pass ignored, an in-cut pass counted; a dismissed fail ignored; default policy unchanged on the fixture Ground |
| 5 | The migration (4.9), run by the manager and the owner, with the owner's go | process | every pre-cut status unchanged; the gate's commands by role; no legacy key can grant |

Docs that change with them: process.md sections 2 and 9.3, development.md
*Owner approvals* and *Team server*, the README's *Not done yet*,
network-sync.md section 3, weave.md's summary caveat, and AGENTS.md's
`.bracken/identities/` line.

## 6. Questions for the owner

1. **Where does the root key live?** (a) On this machine: root records
   (grants, revocations, dismissals) record your intent with an audit trail,
   as approvals do today, and an agent could forge them. (b) Off it, on a
   second computer or phone you already have, running a small signing tool
   that shows each grant, revocation and dismissal before you confirm, and
   keeps its own check of the Ground. Root records then can't be forged, and
   a check runs out of agents' reach, but this machine's merge gate can
   still be pointed at another root by an agent that edits it; the device
   catches that on its next check, not before. It costs a session by hand a
   few times a month. Recommended: (b), if you have a device for it; the
   design works either way. Should approvals stay on this machine under a
   root-granted approvals-only key (recommended), or also go off it?
2. **New keys and the cut.** May the server's directory generate and hold
   the agents' new keys, and may the 20 legacy key files be deleted from
   `.bracken/identities/` after the cut (both are your go on credentials,
   under `adcb78b88c6c`)? And do you accept the cut's terms: after it, a
   legacy key's passes, approvals and retractions count for nothing, while a
   fail it signs still blocks until you dismiss it by id?
3. **Copies of keys.** Time Machine, APFS snapshots and agent transcripts
   keep copies of every key file, deleted or not. Should the server's key
   directory be excluded from Time Machine (a setting on this machine, so
   your call), accepting that losing the machine then loses those keys and
   they are re-created and re-granted?
4. **The threat model.** Do you accept section 2's statement: on one user,
   against an agent that sets out to forge, signatures remain intent plus
   audit, a revocation that can't unblock, and a capped blast radius, not
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
- Revocations by anyone but the root, and cuts that a later revocation
  could widen.
- Role-level authorship for retraction and supersession (4.5).
- Membership and permissions as protocol rules (Stewards): delegation here
  is Bracken's local policy over plain artifacts, which may move into
  `manyroot` once it has been used, as weave.md does with its parts.
