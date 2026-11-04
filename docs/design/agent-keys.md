# Per-agent keys and delegation

Design only, no code (task `21e1c394ca2d`, step 2 of the Weave order in
decision `b4a3456de597`; [weave.md](weave.md) section 8). It compares the
options for making a signature in the Ground mean more than "whoever claimed
to act", recommends one, and lists what the owner needs to decide.

The constraint everything here works within: **every agent runs as the
owner's one macOS user**, and there will be no dedicated user (decisions
`91f610a624fc` and `6f15365a7281`). Keychain, other OS settings and creating
users are out of scope for this task.

## Summary

- **Against an agent that sets out to forge, nothing on this machine makes
  a signature proof**, short of a key held off it. Any agent with a shell can
  read every file the owner's user can, keys and tokens included, wherever
  they live. The design says so plainly (section 2) and doesn't pretend
  otherwise.
- **What it can improve is real, though:** accidents (signing as the wrong
  identity), detection (a forgery leaves a trace that a guard finds),
  revocation (a leaked key stops counting from a point every replica
  agrees on), blast radius (a stolen credential signs only what its role
  may), and the chance of publishing a key through the public repository.
- **Recommended: option D** (section 3), built in four steps (section 5):
  1. **Audit and guard rails, no format change.** `scripts/bk` refuses an
     `--as` that differs from `BRACKEN_AGENT`; the team server logs which
     credential pushed each contribution against its author, and a guard
     reports mismatches.
  2. **The team server signs.** Agents' private keys move out of
     `.bracken/identities/` into the server's private directory. An agent
     holds a token, never a key. The server signs only as the identity its
     token is bound to, only what that identity's role may sign, and logs
     every signature. A contribution by a server-held key that isn't in that
     log was signed by someone who read the key file: the guard finds it.
  3. **Delegation and revocation records**, signed by an owner root key: which
     key acts as which role, and from which point a key stops counting. A
     revocation's cut is its own causal past, so every replica agrees on it,
     and timestamps play no part.
  4. **Verifiers resolve roles**: the merge gate's `--validator`, the owner-
     approval check, the server's allowlist and Weave's views count a
     contribution only while its key's delegation stands.
- **No protocol change.** Ids, encoding, the signing context and the
  validity rules stay as they are; every existing contribution still
  verifies, and an older binary opens the Ground and sees the new records as
  artifacts. Two things are protocol class to review all the same:
  `manyroot`'s signing API changes shape (`identity.rs`, `contribution.rs`)
  without changing a byte, and acceptance gains an opt-in policy field for
  revoked keys (protocol.md, *Acceptance*), whose default changes nothing.
- **Migration re-signs nothing.** The 20 existing keys get one owner-signed
  delegation record and, once their replacements are live, one revocation
  record whose cut is the migration point. About 11,700 contributions stay
  exactly as they are.
- **The join should wait for steps 2 to 4**, so Weave's identities in the
  coordination Ground are server-held and delegated from their first
  contribution. **The format freeze needn't wait:** Weave's line policies
  keep naming keys, and no Weave format changes (section 4.10).
- **Questions for the owner** are in section 6. The main one is where the
  root key lives: on this machine it records intent, as today; off it, it
  is the one signature here that an agent can't forge.

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
4. **Anyone, through publication**: a key committed to the public
   repository, pasted into a contribution, or copied into a backup that
   leaks.

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
- **Change what the server runs**: replace the pinned binary in
  `.bracken-bin`, edit the scripts in the main checkout, kill the server and
  start another on its port.
- **Write any replica directly**, the main checkout's and the server's.
- **Fake a terminal** for `scripts/owner-approve` (`script(1)`, `expect`).
- **Sign anything** with any key it has read.

So, for actor 3, every option below leaves forgery possible, with one
exception: a key held off this machine, which only the owner can use. What
the options change is how far an agent has to go, and what it leaves
behind:

- **Accident** (actor 1): signing as another identity takes a deliberate,
  unusual act, not a typo.
- **Audit** (actors 2 and 3): a forgery leaves evidence that a routine guard
  reports, in a log the forger has to find and alter as well.
- **Revocation** (all): once a key is distrusted, every replica stops
  counting what it signs after a point they all agree on.
- **Blast radius** (2 and 3, after discovery): a stolen token signs only what
  its role may, so a runner's credential can't approve a merge.
- **Publication** (4): no private key under the repository's working tree.

Steering (actor 2) is the case these help most: an injected instruction can
easily make an agent type `--as reviewer`, much less easily make it hunt for
a key file, sign by hand outside `scripts/bk`, and doctor a log.

## 3. Options

| | Option | Protocol | Accident | Audit | Revocation | Blast radius | Publication | Forging agent |
|---|---|---|---|---|---|---|---|---|
| A | Today: shared key folder | none | no | first-seen credential only | by decision text; old name still counts | none | keys under the working tree | free |
| B | Guard rails: `--as` must match `BRACKEN_AGENT`; credential checked against author | none | yes | mismatches reported | as A | none | as A | free |
| C | B, and the server holds the keys and signs | none (signing API reshaped) | yes | every signature logged; signing outside the server detected | token removed; old key still counts | per-role scope at signing | no key under the tree | must read the server's key files and evade the log |
| D | C, and owner-signed delegation and revocation records; verifiers resolve roles | none (opt-in acceptance field) | yes | as C, and roles public | cut every replica agrees on; retractions and supersessions after it ignored | per-role scope, signed and checked by verifiers | as C | as C; can't forge delegations if the root is off-machine |
| E | D, with the owner's root key off this machine | none | yes | as D | as D | as D | as D | can't forge a delegation, a revocation or (if kept there too) an approval |
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
  reader of the public Ground computes the same answer. It fixes a hole
  that exists today: a leaked key can retract its own past contributions, a
  reviewer's fail among them, and process 9.3's recovery can't stop that.
- **E** is D with the root key somewhere an agent can't reach. It is the
  only option that makes some signature proof against a forging agent. It
  needs a device and tooling, so it is the owner's call (question 1).
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
from each credential to exactly one key. A new request on the same listener:

`POST /v1/<ground>/sign`, with the bearer token, and the contribution's
canonical `content` as the body. The server:

1. decodes `content` and refuses it unless it re-encodes to the same bytes
   (the same check as protocol.md, *Encoding*), its `ground` is the server's,
   its `protocol` is 1, and its `author` is the key bound to the token;
2. refuses it unless the body is within the role's scope (4.3);
3. computes the id, signs `"manyroot/v1/contribution:" || id` (the existing
   context, so nothing new can be signed), appends `{id, credential, time}`
   to `signed.jsonl`, and returns the signature.

The client assembles the contribution, adds it to its replica and pushes it
as now, so `scripts/bk`'s write-then-confirm flow is unchanged. The server
never signs a bare id: it signs only content it has read, which is what
lets it apply a scope.

**Detection.** Every legitimate contribution by a server-held key is in
`signed.jsonl`. A contribution by such a key that isn't was signed by
someone holding the key file. `scripts/ground-guards`, at every merge and
checkpoint, lists any. A forger can append to the log too, being the same
user; to make rewriting old entries detectable, the manager's checkpoint
records the log's length and BLAKE3 hash in a claim, so a log that no longer
extends the recorded one is reported.

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
  delegation names (4.3), so agents keep writing `--as manager`; `scripts/bk`
  resolves the role to the key and its token.
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
| `grants[].may_delegate` | array of text | Role patterns it may delegate, `builder-*` say; empty for most |
| `note` | text | Why |

One record holds many grants, so the owner signs the whole team's setup
once, not twenty times.

A grant **counts** when its record's author is the root key, or is a key
whose own grant counts and whose `may_delegate` matches the role, and that
grant's record is in the delegating record's causal past. Chains are at most
two deep: root, then manager, then builders and reviewers. A key's grant
covers its whole history: keys are generated when they are delegated, so
there is nothing before it.

**The root key** is local configuration, like `--validator` names today: a
tracked file outside `.bracken/` (its path is decided in the build), which
the merge gate, the server and `scripts/bk` read. A delegation artifact by
any other key counts for nothing, so anyone can write one without effect.
An outside reader of the public Ground pins the same root from the
repository.

**Scopes**, as first set:

| Role | Scope |
|---|---|
| `owner` (approvals) | claims tagged only `approval`, `delegated`; `supports` relations |
| `manager` | everything; may delegate `builder-*`, `reviewer`, `security`, `triage` |
| `builder-*` | claims with no `approval`, `decision`, `task` or `owner-review` tag; relations; evidence; retracts |
| `reviewer`, `security` | claims tagged `finding`, `note`, `process`, `question`, `report`; `contradicts` and `supports`; evidence; retracts |
| `weave-runner` | evidence; `text/plain` artifacts (its logs) |
| `weave-integrator` | Weave artifacts; relations |

The server refuses to sign outside a scope; verifiers don't count a
contribution outside its signer's scope as an act of the role. Scopes are
coarse on purpose: they cap a stolen credential, they don't encode the
process. They fit what the Ground holds: every owner claim so far is tagged
`approval` (16, all also `delegated`); reviewers' and security's claims are
tagged `finding`, `note` or `process`; builders' claims carry no `decision`,
`approval` or `owner-review` tag, and one carries `task`, which the scope
would refuse in future.

### 4.4 Revocation and rotation

A revocation is an `artifact` with media type
`application/vnd.manyroot.revocation+cbor`: `{v: 1, keys: [AgentId], reason:
text}`. It counts when its author is the root key, the key that delegated
the revoked one, or the revoked key itself (a retiring agent, or one that
thinks it leaked).

**The cut is the revocation's causal past.** A contribution by a revoked key
counts only if it is an ancestor of the revocation record. Because writers
list every head as parents, that is everything the revoker had seen when it
wrote. The rule needs no timestamp and no field to trust: it is derived from
the graph, so every replica holding the revocation agrees, and a replica
that doesn't hold it yet agrees once it does, as protocol.md's goal 2 asks.

What it gives, and doesn't:
- A forgery made after the revocation, even with backdated parents, isn't
  an ancestor of it, so it counts for nothing, whatever its timestamp.
- A forgery the revoker had already seen stays inside the cut. As today
  (process 9.3), everything a leaked key signed is suspect; the server's
  first-seen log and signing log say which contributions the server
  signed and when they arrived.
- A legitimate contribution the revoked key wrote concurrently, which the
  revoker hadn't seen, falls outside. Its work is re-recorded under the new
  key.
- Revocations ignore retraction: retracting one would revive every forgery
  it cut off. A mistaken revocation is undone with a new delegation of a
  new key.

Delegations and revocations are contributions like any other, so the rule
applies to them too: a revoked manager key's delegation or revocation
written after its cut counts for nothing.

**Rotation** is a new key delegated for the role, then the old one revoked,
written together. **Retiring** a builder at the end of its task is a
self-revocation, so its key stops counting even if it leaks later.

The server's allowlist becomes the keys with a counted grant and no counted
revocation. A revoked key's later pushes are refused at the door, with the
split noted in network-sync.md section 3: what builds on them stays
unresolved.

### 4.5 Verifiers

**In `manyroot`**, acceptance gains one opt-in policy field, `revoked`: a
map from a key to its cut, a set of contribution ids. For that policy, a
contribution by a revoked key that isn't an ancestor-or-self of one of its
cut's ids is ignored by every derived rule: its retracts retract nothing,
its relations count for nothing, its evidence doesn't count, and its claims
and artifacts aren't subjects. It stays in the graph, and validity doesn't
change. With the field empty, which is the default, every status is as
today. It is generic, it names no format, and Bracken fills it from the
revocation records. This closes the retraction hole in section 3 for every
policy that uses it.

**In Bracken:**
- `--validator <role>` resolves to the keys with a counted grant for that
  role, with their revocations as `revoked`. A role whose key rotated keeps
  its name, so the gate's commands and the role briefs don't change at each
  rotation, unlike process 9.3's rename.
- `scripts/merge --owner-approval` requires an approval by a key granted
  `owner`, within scope and not revoked.
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

The root key signs delegations and revocations, a few times a month. If it
sits on this machine, any agent can forge a delegation, and D's records
mean what owner-signed approvals mean today: the owner's intent, with an
audit trail. If it sits off this machine, D's records are the only
signatures in the Ground an agent can't forge.

Two ways, the owner's choice (question 1):
- **On this machine** (recommended only if E is declined): a file only the
  owner's tool reads, not in the server's key directory, so the normal path
  never touches it.
- **Off it (E):** on another device the owner holds, signing the record
  there and bringing back the signed contribution as a bundle. Which device
  can sign a raw Ed25519 message over this context isn't settled here; it
  needs checking before it is chosen.

Either way approvals, which are frequent, can keep an `owner` key on this
machine, delegated by the root with the approvals-only scope. A stolen
approvals key can then sign approvals, as today, but can't delegate a key
or undo a revocation, and the root can revoke it.

### 4.7 The runner key

`weave-runner` gets its own server-held key with the `weave-runner` scope:
evidence and its log artifacts, nothing else. Its token is used only by
`scripts/weave-gate`'s validate step. A stolen runner token then signs check
results and nothing more, and the signing log shows each one. It can't
approve, decide or review. That is the "runner key only weave-runner's
process uses" of weave.md section 8, as far as one user allows: a
convention of which process asks, enforced by scope and visible in the log,
not a process boundary.

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
| `POST /sign`, `--keys`, credential binding | cli, merge-gate tooling | none |
| Role resolution in `scripts/bk`, `scripts/merge`, the allowlist | merge-gate tooling | none |

No encoding, id, signing context or validity rule changes, so the protocol
guard's vectors and the fixture Ground stay as they are, and every existing
Ground opens. A release note in docs/releases.md is still owed for the
acceptance field and the new formats.

### 4.9 Migrating this Ground

Nothing is re-signed: re-signing would change every id (`ground` and
`author` are in the signed content) and break every reference. In order:

1. **Root key.** The owner creates it where question 1 decides, and the
   repository's root file names its public key.
2. **Legacy grants.** One delegation record, signed by the root, grants
   each of the 20 existing keys its current name as its role, with the scope
   above. Its grants cover the keys' whole history, so everything already in
   the Ground counts as before.
3. **New keys, server-held.** The server generates one key per live role in
   its key directory (`manager-k2`, `reviewer-k2`, …), and a second
   delegation record grants them. Moving or creating keys is the owner's go
   (decision `adcb78b88c6c`), asked with question 2.
4. **The cut.** At a quiet point, with no PR between review and merge, the
   old keys do their last handovers (process 9.4), then one revocation by
   the root revokes all 20 legacy keys. Its causal past is the migration
   point: everything written before it counts as before; anything signed
   with a legacy key afterwards counts for nothing, under any role-aware
   read.
5. **Remove the files.** With the owner's go, the legacy key files leave
   `.bracken/identities/`, which then stays empty; the checkpoint's key scan
   stays as a guard.

Open `pr` claims written before the cut keep their evidence. Builders started
after the cut get server-held keys from their first contribution.

### 4.10 Weave, the freeze and the join

- **The freeze doesn't depend on this.** Line policies keep naming keys.
  Rotating the runner or the integrator is a new policy version under the
  owner's decision, as any policy change is. No Weave format changes, so
  step 3 of decision `b4a3456de597` can go ahead in parallel.
- **Weave's views apply revocations.** Green, decisions and line versions
  ignore a contribution by a revoked key outside its cut, the same rule as
  4.5. That is derived state in `crates/tree` (full table, protocol-class
  review), with no format change, and it should land before the join.
- **The join depends on it.** At the join, `weave-integrator` and
  `weave-runner` begin writing to the coordination Ground. Created there
  after steps 2 to 4, they are server-held, scoped and delegated from their
  first contribution, so Weave's author rules (integrator, validator, line
  author) become as strong as the rest of the Ground's, and the summary's
  caveat in weave.md shrinks to section 2's same-user limit. Joined before,
  they would start as shared keys and need rotating at once.
- **`owner_decision`** in a line policy names a coordination claim; after
  the join, `scripts/weave` checks it as an approval by a key granted
  `owner`.

## 5. Build plan

Each step is one PR and pays without the next.

| # | PR | Class | Tests |
|---|---|---|---|
| 1 | `scripts/bk` refuses `--as` ≠ `BRACKEN_AGENT`; the server records credential and author in the first-seen log (it already does) and `scripts/ground-guards` reports contributions first pushed under another identity's credential | merge-gate tooling | mismatch refused; a relayed contribution reported, not refused |
| 2 | Signer trait in `manyroot`; `bracken serve --keys`, credential binding, `POST /sign` with canonical-content and author checks; `signed.jsonl`; `scripts/bk` signing through it; the guard for contributions by server-held keys missing from the log | protocol (review), cli | golden vectors unchanged; a token can't sign as another key; non-canonical content, a foreign Ground, another author refused; a contribution signed with the key file directly reported |
| 3 | Delegation and revocation formats with golden vectors; grant counting, chains, scopes; the server's scope check and allowlist from grants | protocol (review) | each counting case; a delegation by a key outside the chain ignored; a scope refused at signing |
| 4 | `revoked` in acceptance; role resolution for `--validator`, `--owner-approval` and reads; Weave's views applying revocations | protocol (review), full table | a post-cut forgery with old parents ignored; a post-cut retraction of a fail ignored; an ancestor of the cut counted; default policy unchanged on the fixture Ground |
| 5 | The migration (4.9), run by the manager with the owner's go | process | every pre-cut status unchanged; the gate's commands by role |

Docs that change with them: process.md sections 2 and 9.3, development.md
*Owner approvals* and *Team server*, the README's *Not done yet*,
network-sync.md section 3, weave.md's summary caveat, and AGENTS.md's
`.bracken/identities/` line.

## 6. Questions for the owner

1. **Where does the root key live?** (a) On this machine, recording intent
   as approvals do today, with delegations and revocations auditable but
   forgeable by an agent; or (b) off it, on a device you hold, which makes
   those records the one signature here an agent can't forge, at the cost of
   bringing records across by hand a few times a month. Recommended: (b), if
   you have a device for it; the design works either way. Should approvals
   stay on this machine under a delegated approvals-only key (recommended),
   or also go off it?
2. **Moving the keys.** May the server's directory hold the agents' keys,
   new ones generated there, and may the 20 legacy key files leave
   `.bracken/identities/` after the cut (both are moving credentials, your
   go under `adcb78b88c6c`)? And do you accept the cut: after it, anything
   signed with a legacy key counts for nothing in role-aware reads?
3. **The threat model.** Do you accept the statement in section 2: on one
   user, against an agent that sets out to forge, signatures remain intent
   plus audit, revocation and a capped blast radius, not proof, except for
   a root key held off the machine?
4. **The dogfood Ground.** Keep its shared keys until the join
   (recommended), or serve it so its runner signs through a server now
   (new infrastructure)?
5. **Harness read rules (G).** Would you like a separate task to test
   whether Claude Code's deny rules or sandbox can keep agents' tools from
   reading the server's key directory, in the permission mode the team uses?
   It's a harness setting, outside this task, and unverified.

## 7. Not proposed

- A dedicated macOS user (decisions `91f610a624fc`, `6f15365a7281`).
- Keychain or other OS settings (out of this task's scope).
- Session authentication by agent key (option F).
- Role-level authorship for retraction and supersession (4.5).
- Membership and permissions as protocol rules (Stewards): delegation here
  is Bracken's local policy over plain artifacts, which may move into
  `manyroot` once it has been used, as weave.md does with its parts.
