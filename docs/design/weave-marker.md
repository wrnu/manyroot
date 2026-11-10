# Who may write the marker that counts

Design only: it changes no format, code, test or fixture (task
`730b5d1a92a9`, decision `03534914ec55` (1)). It answers finding
`26d9361e0a9f`, the forged-marker takeover in
[weave-freeze.md](weave-freeze.md) 2.5. Decision `bae92c497714` makes the
owner's go to freeze wait until this fix has landed, or is planned as a v2.
Nothing here is decided until the owner answers section 6.

File references are to `main` at 3668b09c, except `scripts/weave-gate`,
which is at 1f7d5f20, after PR #31.

## Summary

- **The hole.** Any writer can sign a marker whose parents leave out the
  counted one and, by varying `purpose` until its id is the smaller, make it
  count (`crates/tree/src/ground.rs` 328-330). Its author then owns the
  default line and the null-`owner_decision` exemption, and so the name
  `main` for every reader that resolves it (weave-freeze.md 2.5). Since
  PR #31, `scripts/weave-gate` refuses such a Ground instead of reading it
  (option H). And a marker placed before the honest one needs no race at
  all: at the join, it counts outright (section 1).
- **What a fix has to rest on.** Only its author can retract a marker, and
  an honest answer by id order can always be undercut in about two tries
  (notes `ba9e77679d4f`, `4064049282f2`). The Ground holds nothing that tells
  the honest marker from a forgery either: its id is random
  (docs/protocol.md), and the honest marker in `~/bracken-weave/manyroot`
  has no parents, just as a forgery needn't. So the fix needs **an anchor
  from outside the Ground: who may write a marker that counts**.
- **Recommended: option A, a pinned root identity.** Only a marker signed
  by the Ground's root, an AgentId every reader is given, can count. Among
  the root's own markers, today's rule still picks one. Every other marker
  is ignored and listed by `weave check`. A writer without the root's key
  can no longer take the default line, the exemption or, under G9, the name
  `main`.
- **It changes a rule the freeze will lock** (1.3's marker that counts),
  so it **lands before the freeze**. It can't be "a v2 under section 3":
  the forgery is a valid v1 marker, and section 3 changes nothing about how
  v1 records read (section 4). It changes no record's bytes, and no view of
  any Ground we hold: `~/bracken-weave/manyroot` and its backups have one
  marker, by the identity that would be pinned.
- **What it doesn't fix.** Whoever holds the root's key has all of this
  authority. **On this machine, A never stops an agent that sets out to
  forge, before the join or after it.** Today every Weave key is a file any
  agent can read. After the join the root would be server-held, but any
  agent running as the owner's user can read the server's key directory
  too (agent-keys.md section 2, *What a same-user agent can still do*), and
  the owner has refused a separate macOS user. So A stops mistakes, agents
  steered wrong that don't go looking for keys, and, after the join only,
  writers off this machine who hold a scoped token and no key. Before the
  join there are no tokens at all: `scripts/weave` checks `--as
  weave-integrator` by name only (96-99). The pin is local configuration,
  as editable as any verifier here (agent-keys.md 4.6). Weave-freeze.md
  2.5's other stops stay open, and until G9 a second line named `main`
  still stops the gate.

## 1. The rule today, and why id order can't fix it

- **The marker that counts** is the valid, non-retracted marker with no
  other such marker in its causal past, the smallest id among concurrent
  ones (`ground.rs` 328-330, decision `049d02449a94`). Every other marker is
  `Problem::ExtraMarker` (`ground.rs` 331-335).
- **Two rules read its author:** the default line is the line named `main`
  by that author (`crates/tree/src/lines.rs` 233-237), and only that
  author's first `main` version may carry a null `owner_decision`; views
  ignore every other null one (`lines.rs` 285-300). G9 would add a third:
  resolve a name only among that author's lines (weave-freeze.md 2.4).
- **The attack.** A manyroot writer chooses any parents it holds
  (`crates/manyroot/src/graph.rs` 159-166). A marker with empty parents is
  concurrent with the honest one, and wins half the time on id; varying
  `purpose` makes it win in about two tries. Then the attacker writes a
  `main` with a null `owner_decision`, and the honest main's first version
  is ignored with its links (weave-freeze.md 2.5 has the rest).
- **The pre-placed marker: no race needed.** A new contribution takes every
  current head as its parents (`Store::contribute_with`,
  `crates/manyroot/src/store.rs` 268). So a marker written into a Ground
  before the honest one ends up in the honest one's causal past, and under
  today's rule it counts outright and the honest one never can
  (`ground.rs` 330). It doesn't arise in `~/bracken-weave/manyroot`, whose
  marker `weave init` wrote first. It does at the join: `weave init
  --in-place` writes the root's marker into the busy coordination Ground
  (weave.md section 5, step 3), and any key there that may sign an
  artifact, the manager's or a legacy key with every kind in scope
  (agent-keys.md 4.9), can write a marker first. That's worse than the
  race: no tries, and nothing the join's writer can do afterwards.
- **Why no Ground-internal rule helps.** The honest marker in
  `~/bracken-weave/manyroot` is `488c40418f98`, by `weave-integrator`
  (`6bc92b40…`), with no parents: `weave init` writes it first. A forgery
  with no parents is indistinguishable by shape, causality or id.
  Timestamps are the author's own claim, and no view reads them. Retraction
  is the author's alone (`graph.rs` 177-178, protocol rule 7), so the
  honest side can't remove a forgery, only out-race it, and the race has
  no end. So whichever rule counts a marker must be told, from outside the
  records, whose marker may count, or which one does.

## 2. Options

Each option says what it refuses, what it would change and where, whether
it changes a rule 1.3 freezes, what our existing Weave Grounds do under it,
and what's left.

"Existing Grounds" means `~/bracken-weave/manyroot` and its backups made by
`scripts/weave-backup` (whole-directory copies of the same records), the
`weave-v1` fixture, and the coordination Ground and its fixture, which hold
no marker (`crates/tree/tests/coordination_fixture.rs` 31).

### A. A pinned root identity (recommended)

**Rule.** A reader is given the Ground's **root**, one AgentId. Only a
valid, non-retracted marker **signed by the root** can count; among those,
today's rule (none of the root's other markers in its causal past, the
smallest id among concurrent ones). Every other marker is ignored for
counting and listed by `weave check`. A reader with no root has no counted
marker: no default line, and `bracken weave` writes refuse, as they do
today for a Ground without a marker (`crates/bracken/src/weave.rs` 606).

**What it refuses.** A marker by anyone but the root, whatever its parents
or id. So nobody without the root's key can take the default line, the
null-`owner_decision` exemption, or, once G9 lands, the name `main`. A
forged marker becomes noise in `weave check`, not a takeover or a stop.

**What it changes, in a later build PR:**
- `crates/tree/src/ground.rs`: `Index::build` takes the root (an
  `Option<AgentId>`); the counted marker is found among the root's markers;
  a marker by anyone else is a problem of its own (say
  `Problem::NotByRoot`), so `weave check` tells a forgery from the root's
  own second marker. **The ancestry used for counting is built over the
  root's markers only.** Today `Ancestry::build` (`ground.rs` 309) gives
  each marker one bit, whether any other valid marker is below it. A
  build that keeps that and only filters the candidates would find the
  root's marker below a non-root one placed earlier (section 1, the
  pre-placed marker), so no marker would count: no default line, every
  `bracken weave` write refused, and only the forger able to end it, D's
  permanent stop.
- `crates/tree/src/lines.rs`: nothing. It reads `ix.counted_marker()`, whose
  author is now the root.
- `crates/bracken/src/weave.rs` and `weave/`: where the root comes from
  (question 3). Recommended: `weave init` writes it beside `ground.json`,
  as a file `weave-root` holding the AgentId in hex; every `bracken weave`
  command reads it; a missing or malformed file is no root. A one-time
  `weave root set <AgentId>` for a Ground that predates it, refusing when a
  root is already set and when that AgentId hasn't signed a valid marker.
  `weave check` prints the root.
- `scripts/weave`, `scripts/weave-gate`, `scripts/weave-shadow`: a
  committed pin, `.config/weave-root` (the Weave Ground's id and its root
  AgentId), checked against the replica's `weave-root` before anything
  runs; a mismatch or a missing file refuses. `scripts/code-scope frozen`
  lists `.config/weave-root`, so a change to it takes the owner's go like
  any frozen rule (development.md, Owner approvals).
- Tests: `crates/tree/tests/lines.rs`'s
  `a_marker_written_after_the_first_cant_take_the_default_line` (478-524)
  pins today's rule with alice's and eve's markers. Its first half expects
  eve's later markers as `ExtraMarker`, which becomes the new problem kind.
  Its second half builds two concurrent markers, alice's and eve's, and
  expects the smaller id to count, which may be eve's. That expectation
  flips: with alice as root, alice's counts whatever the ids. A new case
  covers the attack itself (eve's marker with empty parents and the smaller
  id, eve's null `main`, and the honest main still the default). Another
  covers the pre-placed marker: eve's marker in the causal past of alice's,
  expecting alice's to count. `crates/tree/tests/weave_fixture.rs` calls
  `Index::build` (704, 867) and asserts the counted marker, so it must pass
  alice as root and the file changes; the fixture's statuses don't move,
  since its one marker is alice's (339, 705), and its pinned ids and bytes
  don't change, since no record does. Every other test that calls
  `Index::build` changes the same way. `crates/bracken/tests/weave.rs` 462
  gains a forged marker by a second identity.
- Docs: weave.md section 1 (*Lines*, the marker that counts), weave-freeze.md
  1.3, and a releases.md note: readers need the root, and old binaries count
  the old way (section 5).

**Frozen rule?** Yes: it changes 1.3's marker that counts, and through it
which `main` version the null `owner_decision` rule exempts. So it lands
before the freeze (section 4). What's frozen afterwards is the rule
"only the root's markers can count", not how a reader learns the root
(question 6).

**Existing Grounds.** `~/bracken-weave/manyroot`: one marker,
`488c40418f98` by `weave-integrator`, `weave check` lists no problem, and
`main` (`0d137f65fe47`) is `weave-integrator`'s. With `weave-integrator`'s
AgentId as root, the counted marker, the default line and every view are
unchanged. Its backups hold the same records, so the same holds; a backup
made after the replica's `weave-root` exists carries it, since
`weave-backup` copies the whole directory, and one made before needs
`weave root set` with the committed pin. The `weave-v1` fixture: unchanged,
with alice pinned. The coordination Ground: no marker, nothing to count,
unchanged.

**Residual risk.**
- **The root's key is the whole authority.** Whoever holds it can write
  another marker, a second null-`main`, or a new policy version of `main`
  (main's author is the root). In `~/bracken-weave/manyroot` every key,
  `weave-integrator`'s included, is a file in `identities/` that any agent
  on this machine can read (agent-keys.md, section 2). After the join the
  root would be server-held, but the server's key directory is just as
  readable to an agent running as the owner's user (agent-keys.md section
  2), and the owner has refused a separate macOS user. So an agent that
  sets out to forge can sign as the root on this machine before and after
  the join: for example a second null-`main` with an id smaller than the
  honest one's, which `lines.rs` 234-236 then picks as the default line
  (`firsts` is ordered by id) and 291-296 exempt. A protects against
  mistakes, against agents that don't go looking for keys, and, after the
  join only, against writers off this machine with a scoped token and no
  key. Before the join no tokens exist for this Ground, and `scripts/weave`
  checks `--as weave-integrator` by name only (96-99). Question 8 of
  weave-freeze.md, the hostile-writer review, has to weigh this.
- **The pin is local configuration.** An agent that edits
  `.config/weave-root`, the replica's `weave-root` or the binary can point
  every reader on this machine at its own key (agent-keys.md 4.6). The
  committed pin is public and reviewed, so a change shows in git, and
  `code-scope frozen` makes it a frozen-rule change. That's detection after
  the fact, as for every verifier here.
- **The root can race itself.** Two concurrent markers by the root are
  still settled by id. Only the root can write them, so that's the root's
  mistake, and `weave check` lists it; it isn't a defence by id order
  (note `4064049282f2`), since nobody else can enter the race.
- **Readers must agree on the root.** Two readers with different roots
  compute different default lines. A reader without one fails closed. A
  binary from before the fix still counts any writer's marker, so the fix
  protects a reader only once it's upgraded (section 5).
- **Rotating, losing or having the root's key stolen isn't designed.** A
  new root has no marker and no `main`, so changing the pin would move the
  default line to a line that doesn't exist yet. If the root's key is lost,
  the existing marker still counts and decisions still qualify, but
  `main`'s policy can't change. If it's stolen, the thief can do anything
  the root can: retract the root's marker and `main`'s versions (a
  retraction is its author's, `graph.rs` 177-178), write a new marker and a
  `main` of their own, and change `main`'s policy. Nothing in A takes that
  authority back short of a new root, which is the rotation that isn't
  designed. That needs its own design, with part B or a v2 (question 7).
- **A non-root can still write a line named `main`.** It's not the default
  line, and its null `owner_decision` is ignored. Until G9, it still stops
  main-by-name readers as a duplicate name (weave-freeze.md 2.5, finding
  `b6b6ccfd1255`), the gate included: PR #31's check refuses when `weave
  head --line main` fails or names another line, and keeps doing so after
  question 5's narrowing. So A doesn't remove that stop. With G9 the line
  is reached by its id only, now soundly, since the counted marker is the
  root's.

### B. A pinned marker id

**Rule.** A reader is given the id of the marker that counts. Only that
marker counts, if it's valid and not retracted.

**What it refuses.** Every other marker, the root's own included.

**What it changes.** As A, with a marker id in place of an AgentId. `weave
init` can write the pin only after the marker exists; the join's marker
(weave.md section 5, step 3) can be pinned only after it's written.

**Frozen rule?** Yes, as A.

**Existing Grounds.** As A, with `488c40418f98` pinned; the fixture pins
its marker's id.

**Residual risk.** As A, since the marker's author still owns `main`: a
holder of its key can still write a second null-`main` and take the
default line (`lines.rs` 236 picks the smallest id among the author's
`main` lines). It removes only the root's self-race. It's worse for the
join: the pin can't be written until the marker is, and part B's roles
(agent-keys.md 4.3) grant keys, not records, so it doesn't extend to them.
A pinned AgentId does what B does, apart from the self-race.

### C. A marker must cite an owner decision

**Rule.** A marker counts only if it names an owner decision: in `purpose`
by convention, or in a new field (a v2 marker).

**What it refuses.** A marker written without one, which only catches a
mistake. A hostile writer cites a real decision's id: ids are public text,
and the binary can't reach the coordination Ground, so it can check only
the shape (the same gap weave-freeze.md 2.5 lists for `owner_decision`,
`crates/bracken/src/weave/merge.rs` 492-497). `scripts/weave`'s existence
check binds only writers who use it.

**What it changes.** `format.rs`'s `Marker` for a new field (a new `v`), or
a convention on `purpose`; the counting rule; `scripts/weave`.

**Frozen rule?** Yes, and with a new field also a format change.

**Existing Grounds.** `488c40418f98`'s `purpose` is `dogfood` and cites no
decision, so under a strict C it stops counting: the dogfood Ground loses
its default line unless the rule exempts v1 markers, which reopens the hole.

**Residual risk.** The whole takeover, against anyone who reads the ids in
the public Ground. A citation proves something only if the owner signs it,
and that's A with the owner as root.

### D. Refuse a Ground with more than one marker

**Rule.** If more than one valid, non-retracted marker exists, none counts.

**What it refuses.** The takeover: a forged marker leaves no counted
marker, so no default line, no exemption, and every `bracken weave` write
refuses.

**What it changes.** `ground.rs` 330; `lines.rs` and the CLI unchanged;
`lines.rs`'s marker test (478-524) flips: both its halves then expect no
counted marker.

**Frozen rule?** Yes.

**Existing Grounds.** One marker each, so unchanged.

**Residual risk.** **It turns a takeover into a permanent stop any writer
can cause.** Only the forger can retract their marker; nobody else can
ever restore the Ground. That's worse than today for a merely careless
writer, and the hostile-writer review (weave-freeze.md question 8) would
have to accept it in writing. It also rules out the root's own
replacement marker. It's a sound fail-closed fallback for a reader with no
root under A, which is how A uses it.

### E. Trust on first use

**Rule.** A replica records the counted marker the first time it sees
one, in a local file, and from then on counts only that one.

**What it refuses.** A forgery that reaches a replica after it recorded the
honest marker: on this machine's Weave Ground, every forgery from now on.

**What it changes.** As B, with the pin written by the binary on first
open instead of by a person.

**Frozen rule?** Yes: the view then depends on what a replica saw first.

**Existing Grounds.** `~/bracken-weave/manyroot` would record
`488c40418f98` on its first open after the upgrade. A backup opened fresh
records whatever counts in it, which is the same marker, unless a forgery
is already in it and wins on id.

**Residual risk.** A new replica, an outside reader, CI or a backup
restored after a forgery learns the forgery. After the join, every fresh
clone of the public Ground is such a reader. It's B with an unreviewed
source for the pin.

### F. Wait for agent-keys part B

**Rule.** The counted marker is one signed by a key holding a
root-granted role (say `weave-root`), resolved by part B's role function
(agent-keys.md 4.3, 4.6).

**What it refuses.** As A, with rotation and revocation part B would add.

**What it changes.** Part B is open and not to be built from its current
text: four unresolved findings, and a prerequisite refactor (agent-keys.md,
*Part B is open*; decision `98a0dc81afff`).

**Frozen rule?** Yes. And waiting for it means the freeze waits for part B.

**Existing Grounds.** Unknown until part B is designed.

**Residual risk.** The hole stays open for as long as part B does. A is the
first step of F, though: a pinned root is a role resolver with one fixed
answer, and if the freeze leaves how a reader learns the root unfrozen
(question 6), part B can later answer it without a frozen-rule change.

### G. Bind the Ground's id to a key

**Rule.** A Ground's id would commit to a root key (its hash, say), so
every contribution's signed `ground` field names the root.

**What it refuses.** As A, with the anchor inside every record instead of
in reader configuration.

**What it changes.** manyroot's protocol: `GroundId` is random today
(docs/protocol.md), and every contribution signs it. No existing Ground can
adopt it without re-signing every record, which changes every id. **Not
proposed.**

### H. Detection only: the interim check

**Rule.** No rule changes. `scripts/weave-gate` refuses a Weave Ground that
may have been taken over. This is the interim detection of decision
`03534914ec55` (2) (note `4064049282f2` (1)), built in PR #31 (task
`5c616750e3f9`, report `dfe0f5938ff8`). Its `taken_over` check runs in
both `record` and `check`, and fails closed when:
- `weave check --json` reports an `extra_marker` problem, or no counted
  marker;
- line main's first version, the default line, isn't signed with the key
  the Weave Ground holds for `weave-integrator` (compared by key, not by
  name);
- `weave head --line main`, the name `green` and `validate` read, names a
  line other than the default one;
- any of those reads fails.

`record` then exits 1 and says to run git's counted run; `check` exits 1,
so `scripts/merge` ignores the Weave pointer.

**What it refuses.** A gate pass under a forged marker, or with a second
line named `main` in play: the takeover becomes a stop at the gate, and
the merge falls back to git's counted run. It doesn't touch the CLI's
views.

**What it changes.** `scripts/weave-gate` and its kickoff-template copy.

**Frozen rule?** No.

**Existing Grounds.** Unchanged. On `~/bracken-weave/manyroot`, which holds
one marker and a `main` by `weave-integrator` with no problems (section 2),
it should pass; PR #31's report says it wasn't run there.

**Residual risk.** The takeover still holds everywhere but the gate:
`weave head`, `green` and the default line still follow the forged marker
for any other reader. At the gate it's a stop, and only the forger can
end it, by retracting their marker (the reason under D); until then every
Weave-gated merge falls back to git's run, which is a cost, not a wrong
answer. It also rests on a name: the expected author is whatever key the
local Ground holds for `weave-integrator`. **Once A lands, the check should
narrow.** Under A, a marker by anyone but the root is ignored, so refusing
on any `extra_marker` hands every writer a stop for nothing: anyone can
write a marker. It should then refuse only when there's no counted marker,
or the counted marker's author isn't the pinned root (which A makes
impossible unless the pin is missing or changed), and keep its check of
`weave head --line main`.

## 3. Comparison

| Option | Refuses a forged marker by a non-holder of the root key | Stop instead of takeover | Frozen rule | Lands | Existing Grounds' views | Main residual |
|---|---|---|---|---|---|---|
| A, pinned root identity | yes | no: ignored | yes | before the freeze | unchanged | the root's key; the pin is local config |
| B, pinned marker id | yes | no: ignored | yes | before the freeze | unchanged | as A; no pin before the marker exists |
| C, cite an owner decision | no | no | yes, maybe a format | before the freeze | dogfood loses its default line | public ids cite freely |
| D, at most one marker | yes | **yes, permanent** | yes | before the freeze | unchanged | any writer stops Weave for good |
| E, trust on first use | only for a replica that saw the honest one | no | yes | before the freeze | unchanged | fresh readers learn the forgery |
| F, part B roles | yes | no | yes | after part B | unknown | the hole stays open until then |
| G, Ground id bound to a key | yes | no | manyroot protocol | never for existing Grounds | every id changes | not proposed |
| H, detection only | no | yes, at the gate only | no | any time | unchanged | takeover everywhere but the gate |

## 4. Why it lands before the freeze, not as a v2

Decision `bae92c497714` lets the freeze go ahead once the fix has landed,
or once it's planned as a v2 under weave-freeze.md section 3. **This fix
doesn't fit a v2.**

- Section 3's v2 is a new version of a record's payload, read beside v1.
  A forged marker is a valid v1 marker. A rule that only v2 markers count
  would drop `488c40418f98`, which is v1; a rule that keeps v1 markers
  counting keeps the forgery counting.
- Section 3's cutover (question 7) is a line policy declaring a minimum
  version, and judges decisions only. Which marker counts is decided before
  any line exists, so no policy can reach it.
- Any fix therefore changes **how existing v1 markers are counted**, in
  place. Section 3 rules that out once the freeze holds, since two binaries
  would disagree about the default line.

So the fix lands before the freeze, as a change to a rule that isn't frozen
yet. It's still protocol class: an independent review, a security review,
the full table (it touches `crates/tree/src/`), and an `owner-review` for
each `frozen:` line `code-scope frozen` prints (`crates/tree/src/`,
`crates/tree/tests/lines.rs`, `crates/tree/tests/weave_fixture.rs`;
development.md, *Owner approvals*). **It also needs the owner's own go,
now, not just as a recommendation:** the build changes
`crates/tree/tests/weave_fixture.rs`, which calls `Index::build` (704, 867)
and must pass the root, and `code-scope`'s `owner_mode` marks that file as
needing the owner's own approval, which `scripts/merge` already enforces
(G6, PR #29), whether or not the freeze has started. It would be asked for
anyway: the change rewrites the rule 1.3 is about to freeze, and the
freeze waits on it.

The cost of changing it in place is small, and for a reason worth stating:
on every Ground with one marker, by the pinned root, the old and new rules
agree. They differ only on a Ground holding a marker by someone else, which
is where the old rule is wrong. That doesn't make an in-place change after
the freeze acceptable, since the freeze is a rule and this would be its
first exception; it's why landing it now is cheap.

## 5. Rollout

Readers first, as weave-freeze.md section 3 asks for any rule two binaries
could disagree on:
1. The build PR: option A in `crates/tree` and `crates/bracken`, `weave
   root set`, the committed `.config/weave-root`, the scripts' check, and
   PR #31's gate check narrowed as question 5 says.
2. Upgrade both binaries that read the Weave Ground, the pinned
   `.bracken-bin` and `~/bracken-weave`'s, then run `weave root set` with
   `weave-integrator`'s AgentId through `scripts/weave`, as the manager
   (never by hand, AGENTS.md). Check that `weave check` names the same
   marker and lists no problem, and `weave head --line main` the same line.
3. Only then does the freeze's go follow (decision `bae92c497714`).

A binary from before step 1 still counts the old way, so until step 2 the
gate's detection (H, PR #31) is what notices a forgery. Before the join nothing
outside this machine reads the Weave Ground, so the upgrade is the whole of
step 2. At the join, the coordination Ground's root is pinned in the join
PR, before the marker is written (question 2), and every reader of that
Ground must already have the fix.

## 6. Questions for the owner

1. **Which fix?** Option A, a pinned root identity: only a marker signed
   by the Ground's root can count, and today's rule picks among the root's
   own. *Recommended: yes.* It's the only option that refuses a forgery
   outright without handing any writer a stop (D) or waiting for part B
   (F), and it changes no view of any Ground we hold. Its limits, stated
   plainly: it protects only against writers without the root's key. On
   this machine that's mistakes and agents that don't go looking for keys,
   before the join and after it, since a same-user agent can read the team
   server's keys too (agent-keys.md section 2). After the join it adds
   writers off this machine with a scoped token. It never stops an agent
   here that sets out to forge. And it doesn't remove the stop a second
   line named `main` causes, at the gate or elsewhere, until G9 lands.
2. **Who is the root?** For `~/bracken-weave/manyroot`, `weave-integrator`,
   which wrote its marker and `main`. For the coordination Ground at the
   join, decide with the join; record now that it must be a server-held
   identity with a token of its own. *Recommended: yes to both.* Pinning
   `weave-integrator` moves nothing. An owner-held root would mean a new
   marker and a new `main`, which moves the default line. A server-held
   root means a stolen builder token can't sign as it, though an agent on
   this machine can still read its key (question 1). A scope narrower than
   that, only Weave's marker and line artifacts, **can't be expressed
   today**: part A's `Scope` (`crates/bracken/src/signing.rs` 144-152) holds
   body kinds, relation kinds and tags, with no media type, so a root that
   may sign any artifact may sign a decision, a manifest or a non-Weave
   artifact too. Enforcing it would need a media-type field in `Scope`,
   a part A change with a task of its own; the question only asks that the
   join's design decide whether to make it.
3. **Where does the pin live?** In the replica, as `weave-root` beside
   `ground.json`, written by `weave init` or once by `weave root set`, and in
   the repository as `.config/weave-root`, which `scripts/weave` and
   `scripts/weave-gate` check against the replica's before running.
   `code-scope frozen` lists `.config/weave-root`. *Recommended: yes.* The
   replica's copy lets `bracken weave` stand alone and travels with
   backups. The committed copy is public, reviewed and visible in git. A
   change to either is caught by the other, and a change to the committed
   one takes the owner's go.
4. **Pin an identity, not a marker id (A, not B)?** *Recommended: an
   identity.* B adds only protection from the root's own concurrent
   markers, while the root's key keeps every other power; an identity can
   be pinned before its marker exists, which the join needs; and part B
   grants keys, not records.
5. **The gate's detection (decision `03534914ec55` (2), PR #31).** Once A
   lands, narrow PR #31's check: refuse only when the counted marker's
   author isn't the pinned root, or there's no counted marker, or `weave
   head --line main` names another line; never because some other marker
   exists. *Recommended: yes.* Refusing on any extra marker gives every writer a
   permanent stop, since only a marker's author can retract it. The
   narrowed check still refuses while a second line named `main` exists
   (`weave head --line main` fails on a shared name), so that stop stays
   until G9; this question doesn't remove it.
6. **What the freeze locks.** Freeze "only a marker signed by the Ground's
   root can count, and among the root's markers, the causally first,
   smallest id among concurrent ones", and leave how a reader learns the
   root (the files in question 3) unfrozen, under the same review as the
   committed pin. *Recommended: yes.* Part B can then supply the root
   through its role function later without a frozen-format change, which
   weave-freeze.md section 4 otherwise says it would be.
7. **Rotating, losing or having the root's key stolen.** Leave it
   undesigned for now, as an accepted gap, to be designed with part B or
   before the join, whichever is first. *Recommended: yes.* Today a new
   root has no `main`, so a pin change moves the default line to a line
   that doesn't exist; designing that well needs part B's revocation. A
   lost key freezes `main`'s policy. A stolen key is worse: the thief can
   retract the root's marker and `main`, and write their own, and only a
   new root, which is this undesigned rotation, takes that back. Nothing
   needs a rotation before the join, but the gap is real from today, since
   any agent here can read the root's key (question 1).
