# Weave: the product spec

The spec builders implement. It turns the design sketch
[weave-mode.md](weave-mode.md) into part of Bracken itself, not a mode of the
experiment harness (owner decision `53ee3443eb9c`, task `04b85815f4a1`). It
takes as given the sketch's decisions and the owner's answers:
- the CRDT is at the record layer only;
- content is immutable unit versions and manifests;
- an agent integrator works toward goals;
- main and green are causal queries over lines;
- merges work per unit;
- configurations are options;
- the unit is a file for now;
- the first milestone is dogfooding on this repository;
- the dogfood runs in a separate Ground, and the team's coordination Ground
  stays untouched (decision `16ff5718c166`).

It follows the sketch's review findings, which are named where they apply:
`6b3daf366e21` and `ec772d4abda9` (causal heads and windows),
`d497a769334f` (selection, not supersession) and `61934bda795c` (content
hash and ContributionId). Finding `5ae750d9b987` concerned the sketch's own
reading of an experiment result, and the merged sketch already fixed it.
It also takes what the retrospective `16cbeff051ea` found in how the team
used the coordination Ground: what it recommends before the build is folded
in where it applies (named checks, re-runs, one counted run, reviews that
survive a rebase, labelled ids, bounded reads, a namespaced goal tag), and
its later items are listed in section 8.

Nothing here is built yet. This is design only: one docs PR, no code.

## Summary

- **No protocol change in v0.** Every Weave record is an existing
  contribution kind:
  - a file version, a manifest, a line, a decision, a proposal or a
    resolution is an `artifact` with a Weave media type and a canonical
    CBOR payload;
  - a goal is a `claim` tagged `weave-goal`;
  - a check result is `evidence` on a manifest, written by Weave's check
    runner (by convention under shared keys);
  - agreement and objection are `supports` and `contradicts` relations.

  A binary from before Weave opens and syncs a Weave Ground; it just sees
  artifacts and claims. Every Weave writer produces only contributions that
  pass manyroot's validity rules as they are.
- **This is Bracken's code layer, on Manyroot's Weave.** It is a new
  Bracken crate, `crates/tree` (package `bracken-tree`), depending on
  `manyroot`. manyroot stays as it is and assumes nothing about code or
  files; the tree crate knows about files and paths. The commands keep the
  name `bracken weave`: Bracken using Weave (owner decision
  `46b91e01edc2`; *Where this sits*).
- **Every Weave view is derived from the Ground deterministically.** That
  covers head, fork, merge base, green, open objections and guardrails. They
  use causal order with ties broken by the smallest id, never timestamps.
- **Weave's payload formats are pinned like the protocol.** Golden vectors
  live in the tree crate. Changing a format is reviewed as protocol class.
- **Weave imports only git's view of a tree**, never a raw directory. Every
  write that creates a file version refuses secrets by name and by content,
  and Weave never writes outside the tree it materializes.
- **Checks that count run in Weave's sandbox**, by convention under shared
  keys, as the line's policy defines them, and once per manifest. A result
  is bound to the check's exact definition, and nobody types a method
  string.
- **The dogfood Ground lives outside every repository**, at
  `~/bracken-weave/manyroot`. `scripts/bk` refuses to reach it and
  `scripts/weave` refuses to reach the coordination Ground. The checks are
  on the Ground's id, not only its path.
- **Keys are shared in v0** (weave-mode.md, *Trust, in v0*). Every rule
  below that names an author (the line's author, an integrator, a
  validator) records who claimed to act. Under shared keys these rules are
  conventions that honest but fallible agents follow, not guarantees
  against a dishonest one. They become guarantees with per-agent keys.
- **The owner never signs.** The owner holds no key in the Weave Ground,
  and no Weave rule asks for the owner's signature, so no signature there
  stands for the owner. Agent identities sign every record. The owner's
  go reaches Weave only as text: a policy version's `owner_decision`
  names a claim in the coordination Ground, which `scripts/weave` checks
  before a line write (section 1).

## Where this sits

In Manyroot's vision (docs/vision.md), *Weave* is the convergence
subsystem: replication, plus materialized views of merged state. The graph
itself is the CRDT. This spec is mostly Bracken's code layer on top of it.
Its first materialized view is a source tree (owner decision
`46b91e01edc2`). So it is built as Bracken's crate `crates/tree` (package
`bracken-tree`, like `crates/demo`'s `bracken-demo`), not inside manyroot.
The user-facing names stay `bracken weave` and `scripts/weave`.

| Part | Where it lives now | Generic candidate for manyroot after dogfooding |
|---|---|---|
| Files, paths, modes, folding, symlinks, import from git, materializing a tree, the deny-list | Bracken (`crates/tree`) | No: these are about source code |
| Manifests: a set of named values, each a content hash | Bracken | **Weave**: a materialized view as a named set of content hashes, for any kind of content |
| Lines, decisions, heads, forks and merge bases, under the ancestor rule | Bracken | **Weave** and **Rings**: lines of views, and checkpoints of them |
| Green, check results on a configuration, objections with a basis and disposition, guardrails | Bracken | **Proof**: acceptance of a whole configuration under a policy, beside today's per-contribution acceptance |
| Per-file merges and reusable resolutions | Bracken | **Weave**, if other content has units that merge the same way |
| The separate Ground, the scripts and the checkpoint tripwire | Bracken's team process | No |

Nothing moves into manyroot until dogfooding shows a part is generic. Then
the move is its own change, with manyroot's rule kept: nothing in manyroot
may assume a contribution is code. Moving a part changes no format: the
records are already plain contributions.

## 1. Data model

### Unit: a file, by path

A unit is a file of a git tree, named by its path relative to the tree's
root. A file has bytes and a mode.

**Paths.** A path is valid only if all of these hold:
- it is UTF-8, uses `/` as the separator, and isn't empty;
- it is at most 4096 bytes, and each component at most 255 bytes. These
  limits are checked before anything is folded, so checking a path costs
  time and memory linear in its length (finding `d65749f2b45d`);
- it has no leading or trailing `/`, no `.` or `..` component, and no NUL;
- no component holds a `:` or a `\`. Windows reads a name before `:` as a
  drive, whatever the name (`C:x`, `1:x`; git's CVE-2019-1351), and a name
  after it as a stream (`.git:x`), and NTFS can't hold one in a name
  (findings `6f00787d66b9` and `044cd093a6bf`);
- no component is blank: empty, or made only of the White_Space characters
  of protocol.md's rule 8. So a path is never blank, and can be an
  artifact's `name`;
- no component is `.git`, or starts with `.bracken`, after folding;
- it passes git's `core.protectHFS` and `core.protectNTFS` component checks
  (ignorable code points, trailing dots and spaces, 8.3 short names).
  Refusing `:` covers its stream names.

**Folding.** To fold a path, drop default-ignorable code points, as git's
`protectHFS` does, then take Unicode's canonical caseless match form
(definition D145): `NFD(casefold(NFD(s)))`, with full case folding. Case
folding can leave combining marks out of canonical order, or split a
composed letter into a base and a mark, so the second normalization is what
makes canonically equivalent names fold alike (finding `cc6570efb144`). It
is how macOS's APFS and HFS+ and Windows' NTFS compare names. Folding
applies to the component rules above, the `.bracken` prefix test included,
and to the collision rules in *Manifests*. The Unicode tables it uses are
pinned in the format guard, so a dependency that changes them is reviewed
as protocol class.

**Modes.** Three, as git stores them:
- `0o100644`: a file;
- `0o100755`: an executable file;
- `0o120000`: a symlink, whose bytes are its target.

**Symlink targets.** A target is valid only if all of these hold:
- it is relative, UTF-8, has no NUL, and isn't empty;
- its components pass the component rules above, so it has no `.`, no
  `..`, nothing blank, and nothing that folds to `.git` or starts with
  `.bracken`.

A target therefore only ever points down from the symlink's own directory,
and a chain of such symlinks never leaves the tree. A target that would need
`..` stays git's job (section 7).

Directories exist only as path prefixes, as in git. Empty directories and
submodules aren't units (section 7).

### Content hash and ContributionId

These are two different things (finding `61934bda795c`):
- A **content hash** is `BLAKE3(payload bytes)`: of a file's bytes, or of a
  Weave artifact's CBOR payload. It depends on nothing else.
- A **ContributionId** is the hash of the whole signed contribution: author,
  parents, timestamp and body (protocol.md, *Identifiers*).

So two agents writing byte-identical files make two artifacts with two
ContributionIds and one content hash. Manifests name files by content hash.
A replica maps each content hash to the artifacts carrying it. Nothing about
ContributionIds changes. Content hashes don't depend on the Ground either,
which the later join relies on (section 5).

Both are 32 bytes, and both print as 64 hex digits, so in text a content
hash is always written `blake3:<64 hex>`, in output and in input. A bare 64
hex digits is a ContributionId. Inside payloads the two are separate fields,
so the prefix isn't stored.

### The records

All payloads are canonical CBOR. A payload is valid for Weave only if
decoding then re-encoding it gives the same bytes, the same rule
contributions follow (protocol.md, *Encoding*): maps with text keys in the
order listed, integers in their shortest form, byte strings as major
type 2. Every payload starts with `"v": 1`.

An invalid payload doesn't make the contribution invalid. Validity is
manyroot's, and it is unchanged. Weave ignores an invalid payload in every
view, and `bracken weave check` lists it.

A **file value** is `[hash, mode]`: a content hash and a mode. Merges,
resolutions and manifests all compare file values, never bare hashes.

| Record | Kind | Media type | `name` | Payload |
|---|---|---|---|---|
| File version | artifact | `application/vnd.bracken.weave.file` | the path it was first imported at (advisory, never blank) | the file's bytes, raw |
| Manifest | artifact | `application/vnd.bracken.weave.manifest+cbor` | `manifest` | `{v, entries: [[path, mode, hash, size]]}` |
| Weave marker | artifact | `application/vnd.bracken.weave.ground+cbor` | `weave` | `{v, purpose: text, in_place: ContributionId \| null}` |
| Line | artifact | `application/vnd.bracken.weave.line+cbor` | the line's name (never blank) | the line's policy, below |
| Decision | artifact | `application/vnd.bracken.weave.decision+cbor` | `decision` | below |
| Proposal | artifact | `application/vnd.bracken.weave.proposal+cbor` | `proposal` | `{v, line, base, manifest, change, summary}` |
| Resolution | artifact | `application/vnd.bracken.weave.resolution+cbor` | `resolution` | `{v, path, base, sides, result}` |
| Goal | claim, tag `weave-goal` | | | the goal in words, related `refines` to the line |
| Check result | evidence | | | target: a manifest artifact; `method`: a check's name; `details`: below |
| Review: agreement, objection | relation `supports`, `contradicts` | | | to a decision, a proposal or a manifest; an objection's `from` sets its basis (below) |

Every record that can be one is a **subject** (claim or artifact), so the
existing tools (`show`, `log`, `relate`, `evidence`, `retract`) work on
Weave records unchanged.

**Why existing kinds, not new body kinds.** manyroot decodes `Body` and
`RelationKind` as closed sets. A binary that meets a body kind it doesn't
know fails in two ways (checked in the code, security note `31bfa91416e8`):
- **Opening a replica.** If that replica's `objects/` holds such an object,
  `Store::open` stops with `StoreError::Corrupt`, and the whole Ground
  doesn't open.
- **Syncing.** Sync rejects the contribution as malformed. Every later
  contribution that lists it as a parent then stays unresolved (`Graph`'s
  import report), so that replica stops following the Ground for good.

A new kind is therefore a hard fork for every older binary. Artifacts with a
media type carry the same information with none of that: the payload is
opaque bytes to manyroot, and structured data to Weave. The cost is that
references inside a payload (a decision's parents, say) aren't checked by
validity rule 5. Weave checks them itself instead: a reference counts only
if it is a causal ancestor of the record holding it (the *ancestor rule*).
Writers list all current heads as parents, as every writer already does, so
the rule holds for honest writers without extra relations.

### Manifests

A manifest is a tree, like a git tree:
- `entries` are sorted by path bytes, and every path is valid;
- **no two paths collide**: no two paths are equal after folding, and no
  folded path is a prefix directory of another folded path. So `a` and
  `a/b` can't both appear, nor `h` and `H/x`, nor two spellings of one name
  in NFC and NFD;
- `hash` is a file's content hash, and `size` its byte count.

A manifest that breaks a rule is invalid: it is never materialized, and
no view uses it (finding `d5112c023747`).

A manifest records no history: identical trees have one content hash,
whoever made them. History lives in decisions.

A manifest is **complete** on a replica when the replica holds a file
version for every hash it names, with the right size. Views use only
complete manifests. Completeness can differ between replicas until they
sync, then converges.

With about 70 bytes per entry, one manifest holds about 200,000 paths within
the 16 MiB limit. This repository has 292 tracked files outside `.bracken/`,
about 5.9 MB in all.

### Lines

A line is a sequence of decisions. Its artifact payload holds the line's
policy:

```
{v, name: text,
 integrators: [AgentId], validators: [AgentId], goal_setters: [AgentId],
 checks: {name: [text]},                # a name, never blank, to an argv
 guardrails: {max_known: uint, max_age: uint, protected: [path prefix]},
 owner_decision: text | null}           # the owner decision this version follows
```

- **Checks are named, and bound to their definition.** `checks` maps a
  name (`test`, `clippy`, `fmt`) to the argv that runs it, for example
  `["cargo", "test", "--workspace"]`. A check's **definition hash** is the
  content hash of the canonical CBOR of `[name, argv]`. A check result's
  `method` is exactly `<name> blake3:<definition hash>`, for example
  `test blake3:9f2c…`. Weave's runner writes it, so nobody types it: the
  coordination Ground has one check under eight spellings, and exact
  matching on typed strings would silently never count (retrospective
  `16cbeff051ea`). Because the method carries the definition, a result for
  another line's `test` with a different argv never counts for this one
  (finding `4189811f3cdd`), and changing a check's argv changes its method.
- **A policy version cites the owner's decision.** `owner_decision` is the
  id, as text, of the claim tagged `decision` in the coordination Ground
  that allows this version (text, because the two Grounds can't reference
  each other until P3). The policy argv is code this machine runs, so
  `scripts/weave` refuses a line write (`weave line create` or `weave line
  policy`) unless `--owner-decision` names a claim tagged `decision`, not
  retracted, that it finds in the coordination Ground through the main
  checkout's `scripts/bk` (finding `3cd6cd19f9a0`), and `weave check` lists
  versions that lack one. Only the `main` line that `weave init` writes may
  carry null: a first version named `main` by the counted marker's author.
  Views ignore every other version with a null `owner_decision`, as if it
  weren't there, so it neither starts a line nor changes one, and its
  supersedes relations count for nothing (finding `12643881ac59`). The
  binary's `weave line create` and `weave line policy` require
  `--owner-decision` too, as 12 to 64 lowercase hex digits, since a version
  without one would count for nothing. The binary can't reach the
  coordination Ground, so it checks only the shape.
- A line is identified by the ContributionId of its first artifact. The name
  is only a label.
- A policy change is a new line artifact by the same author, which
  `supersedes` the previous one. Core supersession counts only when the old
  artifact's author signs it, so only the line's author can change its
  policy. Under shared keys, that means only someone signing as the line's
  author.
- The **policy in force for a record** is the newest version in that chain
  that is a causal ancestor of the record. It judges whether a decision
  qualifies (section 2), including the basis of every objection the
  decision answers, and that judgement never changes later.
- The **current policy** is the version in the chain with no current
  successor, as `log --current` finds it. It judges green (section 2). If
  the chain diverges (two current successors), the line has no current
  policy: green is undefined, and `weave green` lists the versions until
  the line's author settles them.
- An integrator who isn't the line's author can't change the line's
  guardrails, because only the author can change the policy (again a
  convention under shared keys).
- **The marker that counts** is the valid, non-retracted Weave marker with
  no other such marker among its causal ancestors. If several are
  concurrent, the smallest id among them counts, and `weave check` lists the
  others. So a marker written after the first has been seen, by anyone, can
  never take over the default line (finding `d8168e1a9ebb`, decision
  `049d02449a94`). A deliberately concurrent marker, written with forged
  parents, is outside the threat model of decision `7529dd229d6f`: mistakes,
  not adversaries, under shared keys.
- The **default line** is the line named `main` by that marker's author.
  Only a line's first artifact that supersedes no version in the replica is
  a candidate, so a version that joins another line's chain stops being
  one, though decisions made on it keep their judgement (finding
  `fdd17cbdc816`). If there are several, it is the one with the smallest
  id.

### Decisions: integration as a record

A decision moves a line. It is the commit:

```
{v, line: ContributionId, manifest: content hash,
 parents: [ContributionId],              # decisions, ascending, no duplicates
 proposals: [ContributionId],            # the proposals it integrates
 breakage: [{check: text, detail: text, paths: [path]}],   # check: a check's name
 answers: [{objection: ContributionId,
            disposition: "fixed" | "fell_back" | "accepted_breakage" | "overridden",
            reason: text}],              # reason never blank
 resolutions: [ContributionId],          # the resolutions its merge used
 goals: [ContributionId],                # goals it serves
 source: {git: text} | null,             # the git commit it mirrors, while dogfooding
 note: text}
```

- `parents` are decisions, on this line or another (a merge). They must obey
  the ancestor rule.
- `breakage` is the *known breakage* the decision accepts. Its age isn't
  stored: it is counted in line moves (section 2).
- `proposals` and `resolutions` name the proposals the decision integrates
  and the resolutions its merge used. Both obey the ancestor rule.
- A decision refers to at most 1,024 contributions in all: its line,
  parents, proposals, answered objections, resolutions and goals. Every
  reference is checked on each open, so a longer list makes the payload
  invalid (finding `1873f4834edd`).
- An answer is a record of how the integrator dealt with an objection, not
  proof that it was settled. It doesn't need the objector's agreement.
  Section 2 says which dispositions an objection allows.

Selecting among alternatives retires nothing in the Ground (finding
`d497a769334f`). A manifest names the chosen version of each file, and the
unchosen version stays a current contribution, only not on the line. Weave
never writes a `supersedes` relation for another author's artifact.

### Proposals and resolutions

- **A proposal** asks for a tree to be integrated onto a line. `base` is the
  decision it was made from; `manifest` is the content hash of the proposed
  tree. Its change is the per-file difference between the base's manifest
  and the proposed one: the paths that differ, sorted by path bytes, each
  as `[path, before, after]` with full file values (null where absent).
  `change` is the content hash of that list's canonical CBOR. Views
  recompute it, and a proposal whose `change` doesn't match is invalid.
- **A rebase keeps a proposal's reviews.** A rebased proposal is a new
  artifact with a new base. If its change is the same, so is `change`, and
  every review of the old proposal also applies to it (section 2). If the
  rebase changed what the proposal does to any file, including a file the
  new base changed under it, the key differs and the reviews don't carry
  over. In the coordination Ground a review is tied to "pr claim @ sha", so
  any new head, even a clean merge of main, discards it (retrospective
  `16cbeff051ea`: 3.6 reviews per PR, 65% of evidence on superseded
  heads).
- **A resolution** settles one file conflict at `path`:
  - `base` is the common ancestor's file value, or null if the file was
    added on both sides;
  - `sides` holds the two sides' file values in ascending order, with null
    for a deleted side;
  - `result` is a file value, or null to delete the file.

  The key `(path, base, sides)` compares full file values, mode included
  (finding `80c76d054e5d`). So a resolution is reused only for exactly the
  same conflict, and never silently changes a mode.
- Anyone may write proposals and resolutions. Only a decision puts one on a
  line.

### Goals, checks and objections

- **A goal** is a claim tagged `weave-goal`, related `refines` to the line's
  first artifact. The tag is namespaced because tags are queries: the
  coordination Ground already holds claims tagged `goal`, which would meet
  Weave's at the join (retrospective `16cbeff051ea`, gotcha
  `11230613d276`). A goal binds the line under a policy if its author, and
  the author of its `refines` link, are in that policy's `goal_setters`. So
  no one else can bind a setter's goal by linking it, including after the
  setter withdrew it by retracting their link (task `8ff95e0b756f`).
  Whether it binds a decision is judged within the decision's past.
- **A check result** is evidence whose target is a manifest artifact and
  whose `method` names one of the line's `checks` with its definition hash.
  By convention under shared keys, only Weave's runner writes them (`weave
  validate` and `weave integrate --run`, section 3). Their `details` start
  with the manifest's content hash, then the attempt number, the argv that
  ran, the exit code and the output tail (section 3). Results attach to
  the configuration (the manifest), not to a decision, nor to one artifact
  of it: retracting the artifact a result targets doesn't remove the result
  (finding `348e506fce4c`). So a candidate that never becomes a decision
  keeps its failures on record (finding `91b3c74acc47`), and a decision
  whose manifest was already checked, a fast-forward say, needs no new
  run.

  Nothing proves the check ran on that manifest: under shared keys, a
  result is a report, not a proof.
- **A review** is a `supports` (agreement) or `contradicts` (objection)
  relation, with the reason in its `note`. Its target is one of:
  - a decision;
  - a manifest artifact, which reviews every decision and proposal with
    that manifest hash;
  - a proposal, which reviews every proposal on the same line with the
    same `change`, so it survives a rebase that leaves the change as it was.

  A target resolves to what it names whether or not it is retracted: a
  manifest artifact to its manifest hash, a proposal to its line and
  `change`. So retracting the artifact a review targets clears nothing
  while the manifest or the change stands through another (finding
  `06bbc6a34636`).

  Agreement is shown (`weave review`) but not required in v0. Objections
  bind integrators (section 2, rule 5).
- **An objection's basis** comes from its `from` end, so it is read from the
  Ground's structure and never parsed out of text. Call the manifest it
  objects to the *objected manifest*: the decision's, the manifest
  artifact's or the proposal's. Under a policy `P`, the basis is:
  - *evidence-backed*: `from` is a check result counted under `P`
    (section 2) that fails, on the objected manifest;
  - *goal-backed*: `from` is a goal that binds the line under `P`;
  - *discussion*: anything else.

  Which `P` applies is fixed in section 2: the policy in force for the
  decision being judged, never today's.

## 2. Derived state

These are computed by `crates/tree` from one replica's Ground. All of them
use causal order and break ties by the smallest id; none use `timestamp_ms`
(protocol.md: advisory only). The same Ground gives the same answer on every
replica. The one exception is completeness, which converges once replicas
sync.

**Who pays for a line.** Whether a decision qualifies reads only what is
in its past: its causal ancestors that are decisions, on any line, with
their verdicts; the objections, answers, check results and goal links
there; and its own line's policy chain (rules 1 to 6 below). So a reader
judges only the lines it
asks about, plus the default line: their decisions, and the decisions
those have as ancestors, on whatever line. It also judges the proposals on
those lines. Every other decision is left unjudged, and every other
proposal is kept by its payload alone, so that naming it still finds its
line. Each command asks about the lines it names, by id or name, or by the
id of one of their decisions or proposals. A name names every line of that
name, and anyone can write a line called `main`, which never becomes the
default line. So `--line main` judges such a line too, and is then refused
as ambiguous; only giving no `--line` (the default line), or the line's id,
can't be made to. `scripts/weave-shadow` does that (finding
`b5b5871cc017`). A line nobody names, and no
named line's decision descends from, costs a reader nothing beyond holding
its records (task `a4d1a06a1072`). Asking about a line still judges it
whole, with the same verdict a judgement of every line gives. This changes
what is computed, never what qualifies.

**Counted check results.** Counting is always done under one policy
version `P`, over one set of contributions `S` (the whole replica, or one
decision's causal ancestors). A check result on manifest hash `H` is
*eligible* for a decision `D` under `P`, in `S`, if all of these hold:
- its method is exactly `<name> blake3:<definition hash>` for one of `P`'s
  `checks`: the name, and the hash of that name with `P`'s argv for it
  (finding `4189811f3cdd`). This is tested first, before the epoch and the
  latest-result rule, so a result for a different definition never takes
  part in either. In particular it can't replace a real failure;
- it is in `S`, and it isn't retracted in `S`: no retraction of it in
  `S` stands there, by the protocol's rule, so a retracted retraction
  restores it (finding `1c584afefc50`);
- its author is in `P`'s `validators`;
- its author isn't `D`'s author. So the integrator's own results never
  count, including the ones `weave integrate --run` records;
- it targets a manifest artifact with hash `H`;
- it is a causal descendant of `P`'s artifact. This is the **policy
  epoch**: a result recorded before `P` existed isn't eligible under `P`,
  pass or fail, even for the same check definition.

Of the eligible results, **each validator's latest result per manifest
hash and check counts**, and its earlier ones don't. Latest is in causal
order: a result that has no eligible descendant from the same validator
for the same `H` and method. If a validator has several latest results that
are concurrent, they count together as one fail. So one flaky fail doesn't
block a manifest for good: the validator runs the check again (`weave
validate --again`), and its new result replaces the old one. A fail is
never retracted to clear it; retraction means the result was wrong
(retrospective `16cbeff051ea`: seven flaky fails were retracted in the
coordination Ground). Every attempt stays on record with its number, and
`weave green` shows, per check, the fails a later attempt replaced, so
re-running until a check passes is visible.

**Qualifying decision.** Decisions are taken in causal order, so each rule
depends only on ancestors. Every rule judging `E` reads only `E`'s causal
ancestors, under `E`'s policy `P_E`: the policy in force for `E`. That
includes counting check results and classifying each objection's basis.
So a later policy change, goal, check result or retraction of a result
never changes whether `E` qualified (finding `b2098df8e4be`). A decision
`E` on line `L` qualifies if all of these hold:
1. Its payload is valid, and it isn't retracted.
2. Its author is in `P_E`'s `integrators`.
3. Every parent is a qualifying decision and a causal ancestor of `E`. A
   decision with no parents is a root of `L`.
4. Its manifest is complete, and valid. Manifests and file versions are
   content, named by hash, so they resolve whether or not the artifact
   carrying them is retracted: a later retraction of a manifest artifact or
   a file version never unqualifies `E`, and v0 has no content takedown
   (answer `2c28892e9701`). Completeness alone is read over the replica, as
   it converges once replicas sync.
5. **Every objection in its window is answered, and allowed.** Take any
   non-retracted `contradicts` relation `O` that is a causal ancestor of `E`
   and targets a qualifying decision `D`, where `D` is a strict ancestor of
   `E` in the decision graph. Some decision `A` must answer `O`, where `A`
   is `E` itself, or is a descendant of `D` and an ancestor of `E`. So an
   objection made before the line moved counts against that move, and one
   made later binds the next move. This is the causal window, counted in
   line moves (findings `6b3daf366e21` and `ec772d4abda9`).
   "Non-retracted" is judged within `E`'s past, by the protocol's rule (a
   retracted retraction restores it), and a target resolves whether or not
   it is retracted (gotcha `b4d45c7352d4`).

   **Objections to what `E` uses unchanged.** Take any non-retracted
   `contradicts` relation `O` that is a causal ancestor of `E` and targets
   either a manifest artifact with `E`'s manifest hash, or a proposal `X`
   that `E` uses. `E` uses `X` if it lists, in `proposals`, `X` or any
   proposal on `X`'s line with the same `change`, or if every entry of `X`'s change
   appears in `E`'s own change against one of its parents on `L`. The
   second test catches an integrate of a given tree that lists no
   proposals, as stage 1 does (security note `17906a29698f`). Then `E`, or
   an ancestor of `E` on `L`, must answer `O`. Because `E` uses the objected change as
   it is, only `accepted_breakage` and `overridden` can apply. To fix it
   instead, the integrator integrates a different change, which the
   objection doesn't reach.

   The answer's disposition must be allowed for `O`'s basis (finding
   `e99c224ed5b6`). The basis is classified under `P_E`, over `E`'s
   ancestors, and a counted fail is one eligible for the objected decision
   (or, for an objection to a manifest or a proposal, for `E`), whether or
   not a later result replaced it; its check is the failing check:

   | Disposition | Means, and is checked as | Allowed for |
   |---|---|---|
   | `fixed` | `A`'s manifest hash differs from `D`'s, and, for an evidence-backed objection, the failing check isn't in `A`'s breakage | any basis |
   | `fell_back` | `A`'s manifest hash equals the manifest of a qualifying ancestor of `D` | any basis |
   | `accepted_breakage` | `A`'s breakage lists the failing check, so the guardrails govern it | evidence-backed |
   | `overridden` | nothing beyond the reason | discussion only |

   So an objection backed by failing evidence or by a binding goal can't
   simply be overridden. Whether a `fixed` really fixed anything shows in
   green, once the checks have run on `A`'s manifest.

   **One answer can cover many objections** (decisions `8cf20fe7669a` and
   `4afee4855944`, findings `4d9669a353b3` and `05e9995ccde0`). An
   answer's `objection` names one of:
   - an objection, which it answers;
   - a target: a decision, a manifest artifact or a proposal. It answers
     every objection to that target (to a manifest hash, or to a
     proposal's line and change) that rule 5 judges at the answering
     decision;
   - the answering decision's own line (its `line`). It answers every
     objection in the decision's window, whatever its target. It doesn't
     answer objections to what the decision uses, which need their own
     answer or one naming their target.

   An answer answers each objection it covers under its disposition only
   where that disposition is allowed for that objection's basis, judged
   under `P_E` as for any answer; the others stay unanswered. So one
   reference answers any number of objections, spread over any number of
   targets, and no flood of objections, from anyone, can stop a line, while
   a goal-backed or evidence-backed objection can't be overridden in bulk.

   **An answered objection is settled.** `O` is settled at `A` only if
   rule 5 judged `O` at `A` and found an allowed answer there:
   - `O` is in `A`'s window, or targets a manifest or proposal `A` uses;
   - `O` is in `A`'s past and not retracted there;
   - an answer of `A` names `O`, its target, or `A`'s line (for an
     objection in the window);
   - and `A` qualifies, so that answer is allowed for `O`'s basis under
     `A`'s policy.

   Then `O` is settled for every descendant of `A` on `A`'s own line `L`,
   both in the window and in what a decision uses, and no later decision on
   `L` judges it again, even if `O` is later retracted and restored. A
   decision on another line, including a merge on another line that brings
   `A` in, judges `O` again under its own `P_E`, since a basis is
   classified per line: a goal binds only the lines it refines, and a
   result counts only under a line's validators and checks (finding
   `620ab4bd76ba`, decision `4afee4855944`). An answer at a decision that
   doesn't use the objected manifest or proposal, or to an objection
   retracted in its past, settles nothing, so a restored objection binds
   the next move again (finding `0bf37b345567`). So each objection costs a
   judgement per move on a line only until it is answered there.

   For example: under `P0`, `G` isn't a goal setter, so an objection from
   `G`'s goal is discussion, and `E` may answer it `overridden`. If `P1`
   later adds `G`, `E` still qualifies, because `P1` isn't `P_E`, and the
   objection stays settled for `E`'s descendants. Only decisions whose
   policy in force is `P1` treat `G`'s goals as binding, so a new objection
   from `G`'s goal binds the moves under `P1`.
6. **Guardrails** (finding `ca41b1e0a0fa`):
   - **Protected paths: always.** No breakage entry names a path under
     `protected`. No exception.
   - **Count and age.** `E`'s breakage has at most `max_known` entries, and
     no entry is more than `max_age` line moves old. An entry's age is the
     length of the longest unbroken chain of ancestors on `L`, ending at
     `E`, that list the same `check`, counted in decisions with `E` itself:
     a new entry is 1 move old, so `max_age` 0, like `max_known` 0, allows
     no breakage.
   - A path is under a protected prefix if it is the prefix, or below it.
     "Strict subset" compares the entries' checks.
   - **Reducing is allowed.** Past the count or age limit, `E` still
     qualifies if it has at least one parent on `L`, and its breakage is a
     strict subset of every `L` parent's breakage. A root, or a decision
     whose parents are all on other lines, gets no exception: it must meet
     every limit.

**Line moves** are counted in decisions, not in turns or time.

**Head and fork.** The *heads* of `L` are its qualifying decisions with no
qualifying descendant on `L`.
- With one head, that is `L`'s head, and **main** is the default line's
  head.
- With several, `L` is **forked**. Every replica shows the same heads in
  ascending order. A decision with all the heads as parents (a merge)
  settles it.

**Merge base.** For two decisions, it is their common ancestors in the
decision graph that aren't ancestors of another common ancestor. In v0 there
must be exactly one. With several (a criss-cross merge), `line merge`
refuses, lists them, and needs `--base`, which must name one of them. With
one, `--base` may only repeat it, and with none (no shared history) the
heads can't be merged. So `--base` never brings in another tree (finding
`506c2ad3283b`).

**Per-file merge.** A file's value is its file value, or absent. Comparing
base `B` with sides `X` and `Y`:
- `X` equal to `Y`: take it.
- One side equal to `B`: take the other.
- Otherwise it is a **conflict**, including added on both sides with
  different content or mode, and deleted on one side while changed on the
  other.
  - A resolution with the key `(path, B, {X, Y})` settles it first: see
    below. Without one, a conflict may still merge line by line.
  - **Line merge** (task `7cbd0a0ce7d0`, design `68b57f9aed6d`, answers
    `c8a511445626` and `55e0a67c68a2`). A conflict merges line by line only
    when all of these hold:
    - `B`, `X` and `Y` are all present, so an add on both sides and a
      delete against a change stay conflicts;
    - none is a symlink;
    - their modes merge as values do: equal sides are taken, and a side
      equal to `B` gives way to the other;
    - each is text: at most 8 MiB, valid UTF-8 and no NUL byte;
    - each side's diff is within the cap: once the lines `B` and the side
      share at their start and their end are set aside, at most 10,000
      lines are left of each (finding `933f535c9476`, answer
      `47a1c1088fd6`). The histogram diff is quadratic in lines that occur
      once, so the cap bounds an integrator's wait to seconds. It counts
      lines, never time, so it is deterministic, and the format guard pins
      it just under and just over. This repository's largest text file is
      about 6,500 lines, so every file of ours merges in full; a bigger
      rewrite stays a conflict, which falls back safely;
    - the merged bytes fit one file version (16,711,680 bytes; finding
      `d189e282cdee`), which two 8 MiB sides can exceed.
  - **How it merges.** A line ends after each `\n`, so a last line without
    one is a line of its own. Nothing is normalised. Each side is diffed
    against `B` with imara-diff's histogram algorithm, pinned exactly, its
    hunks slid as far down as they go. Hunks of the two sides whose `B`
    ranges overlap or touch, one ending where the other starts, form a
    region. A region with hunks of one side takes that side, and one with
    both sides is clean only if both give the same lines. Any region left
    unclean leaves the whole file a conflict, keyed as above, so the
    resolution record doesn't change. The merged bytes are a new file
    version, written with the tree, and the merge names each such path
    (`merged lines: <path>`, and `merged_lines` in `--json`): the paths a
    step merged by lines whose version the final tree still holds, by
    path, never by content (finding `f34ae4b848c0`). A resolution may take
    such a version, as `--take into` after a line merge does, and it is
    written with the tree too (finding `fa1c271c4db7`).
  - **It is Weave's own rule, not git's.** It is deterministic, a function
    of the three files' bytes only, and the format guard pins its results
    with golden vectors. It agrees with `git merge-file
    --diff-algorithm=histogram` in nearly all cases, but not byte for
    byte. Where repeated lines can be aligned more than one way, imara-diff
    and git's xdiff may choose differently: on 20,000 generated triples
    built to repeat lines, 20 of them, about 0.1%. The cases the format
    guard's oracle finds on its seed are pinned there, so a change to the
    crate or to git that moves them is reviewed. Under `--mirrors` a
    difference is a fallback, never a wrong tree.
  - A resolution comes before the line merge. So every merge recorded
    before line merging computes the same tree again, and an integrator
    can overrule a clean textual merge that is wrong in meaning.
  - The conflict listing names, for a conflict whose versions this replica
    holds, or an earlier step's line merge made, and that are text, the
    base lines where they conflict (`text_conflicts` in `--json`).
  - `resolve <path> <selector> --markers <f>` helps a person settle such a
    conflict. It writes nothing to the Ground: it writes the new file `<f>`,
    never over an existing file, and only where `--file` will take it back:
    inside the checkout's work tree, outside `.git`, every `.bracken*` root
    entry and the Ground, and past the deny-list (finding `d49f362be57f`).
    It is executable where the sides' modes merge to executable, so
    `--file` records that mode (finding `445429f73583`); where they don't
    merge, `--mode` chooses. It holds the line
    merge with each conflicting region between diff3-style markers
    (`<<<<<<< into`, the line's lines, `||||||| base`, the base's,
    `=======`, the side merged in, `>>>>>>> from`). A section without a
    final line end gets one before the next marker. The person edits it and
    records it with `resolve --file <f>`. The markers are an editing aid
    only, and never enter a tree or a key. A conflict that isn't of text
    (absent on a side, a symlink, not held, not text, or over the diff
    cap) is refused.
  - A resolution with the key `(path, B, {X, Y})` settles it, if an
    integrator of the target line, under its current policy, wrote it. If
    there are several, a merge or integrate applies the one with the
    smallest id, and the decision lists it in `resolutions`.
  - Resolutions by anyone else are only offered: the listing names each,
    with its author and result. An integrator adopts one by writing their
    own resolution with the same result, `resolve --take <resolution id>`.
    Whoever wrote first, or ground a small id, can't put content on
    neither side into a decision the integrator signs (decision
    `91c12b4da2ad`, finding `0a45dfffcd48`).
  - Any conflict may get another resolution, even one that already has
    one. Every merge lists the resolutions it used, with author and
    result, in its text and its `--json`.
  - A rename is a delete plus an add.

A merged tree is a candidate like any other. It moves a line only through
a decision, and goes green only by passing its checks.

**Green** is judged against the line's *current* policy, not the policy in
force for each old decision (finding `058bab0efab8`). It is the only view
that uses the current policy; qualification never does. Let `P` be `L`'s
current policy version, and count over the whole replica. A qualifying
decision `D` on `L` is *green* if both hold:
- its `breakage` is empty;
- for every check in `P`'s `checks`, the results counted for `D` under `P`
  include at least one pass and no fail.

The policy epoch is a filter on which results count, not a condition on
`D` (finding `98dd56b8170a`). A pass or a fail recorded before `P` is
simply not counted. So changing the checks, or any policy change, makes
green start over: under `P1` a validator checks the manifest again, and
the manifest goes green on the new results alone, whatever was recorded
under `P0`. If `L` has no current policy (a diverged chain), green is
undefined.

`green(L)` is computed from the green decisions that are ancestors of, or
equal to, every head of `L`. Among those, it is the ones with no green
descendant in the set:
- one: that is green;
- none: there is no green yet;
- several: `green` reports them all as ambiguous, smallest id first.

So while `L` is forked, green is the last green before the fork, which is
what weave-mode.md asks for.

**Open objections.** These are the `contradicts` relations on `L`'s
decisions that rule 5 hasn't yet seen answered: what the next move must
answer. That is the window of a decision with every head as a parent, less
the objections an answer there allows, judged under `L`'s current policy
over the whole replica, each with the dispositions its basis allows. A
decision `integrate` would write is judged the same way: its past is the
whole replica. `weave review` also lists the reviews of each open proposal on
`L`, carried over by `change`, and what a decision using it unchanged
would have to answer.

**Policy, not acceptance.** These views are Weave's. They aren't
`manyroot::Policy` acceptance, and `bracken accepted` says nothing about
heads. A manifest made and tested by its own integrator has no independent
pass under manyroot. Weave's green requires one, from `validators`, under
the shared-keys caveat in the summary.

## 3. The CLI

The commands are `bracken weave <command>`. Every command that reads has
`--json`, as AGENTS.md requires. `--line` defaults to the default line.

**Output names what each value is** (retrospective `16cbeff051ea`: the
two-ids trap after `claim --supersedes` produced four gotchas and three
retracted relations, and content hashes look like ids):
- Every write prints one labelled line per record it wrote, in a fixed
  order: `manifest blake3:<hex>`, `file blake3:<hex> <path>`,
  `decision <id>`, `evidence <id> <check> pass|fail`.
- `--json` uses named fields (`{"manifest": "blake3:…", "decision": "…"}`),
  never a bare list of ids.
- Every option that takes a record names its kind (`--at`, `--base`,
  `--proposals`, `--manifest`). Given an id of another kind, a command
  refuses and names the kind it got: "`<id>` is a proposal, not a
  decision". Given a bare 64-hex value where a content hash is expected, it
  refuses and asks for `blake3:`.

**Every `bracken weave` write checks the marker.** It writes only to a
Ground holding a counted Weave marker. Under `scripts/weave` it also checks
the Ground's id (section 5).

### What import takes

Import takes **git's view of a tree, never a directory listing** (finding
`c2ae8da5a06b`). The same refusals apply to **every write that creates a
file version**: `import`, `propose`, `resolve --file`, and the merged
candidate in `integrate`'s step 2 (finding `587bda937d44`).
- `--from-git <rev>` (the default is `HEAD`) takes a commit's tree;
- `--index` takes the index: tracked and staged files, with their staged
  contents.

Untracked and ignored files are never seen, so `.gitignore` applies through
git. Changes in the working tree that aren't staged aren't imported; stage
or commit them first. On top of git's view:

- **Excluded.** Every root entry starting with `.bracken`: the
  coordination Ground, `.bracken-bin`, `.bracken-team`. They are skipped,
  and listed. A `.bracken*` component deeper in a tracked path makes the
  path invalid (section 1), so it is refused, not skipped.
- **Refused.** The import stops, writes nothing and names the path, if a
  path or file looks like a secret:
  - a component under `identities/`;
  - a basename that is `.env`, or starts with `.env.`;
  - one ending in `.pem`, `.key`, `.p12` or `.pfx`;
  - `id_rsa`, `id_dsa`, `id_ecdsa` or `id_ed25519`, with or without a
    suffix;
  - content holding a PEM private-key header
    (`-----BEGIN ... PRIVATE KEY-----`), whose label is made of capital
    letters, digits and spaces as in real PEM, so prose that quotes the
    header with an ellipsis, as this line does, isn't refused;
  - content holding an armored PGP private key block
    (`-----BEGIN PGP ... BLOCK-----`), an RFC 4716 SSH2 private-key header
    (`---- BEGIN SSH2 ... ----`), or a PuTTY key file's header (a line
    starting `PuTTY-User-Key-File-<n>:`), with labels read the same way
    (finding `ecfbb7d52203`);
  - content holding a Bracken identity's key: a `secret_key` field with a
    64-hex-digit value, in JSON (`"secret_key": "<hex>"`) or as a YAML or
    TOML line (`secret_key: <hex>`, `'secret_key': <hex>`,
    `secret_key = "<hex>"`). A key embedded
    as an escaped JSON string isn't matched; the exact-key rule below still
    catches either Ground's own;
  - content holding the exact key of any identity in either Ground's
    `identities/` directory, in hex, as `contribute()` already refuses for
    its own Ground. That catches a raw hex key without the JSON field. The
    keys are read in memory for the comparison and never printed.

  The content scan matches key material, not the word: this repository
  mentions `secret_key` in code and has no key, and imports cleanly. There
  is no override in v0. A refused file stays in git (section 7).
- **`--dry-run`** lists what would be written: paths, modes, sizes, content
  hashes, and what is new to the Ground. It also lists what is excluded and
  what is refused, and writes nothing. `propose` and `resolve` take it too.
- **`resolve` finds the conflict to settle** by rebuilding the merge step
  by step. It takes the first conflict at the path that neither an
  integrator's resolution nor the line merge settles, applying and passing
  a step whose conflict there is settled. So with several proposals that
  conflict at one path in turn, each gets its resolution (finding
  `90de879d935c`), and the one it settles is the one the listing showed.
  With none unsettled, it takes the last settled one, so a conflict may
  get another resolution, or one that overrules a line merge.
- **`resolve --file <f>`** takes a file from disk, so it has extra
  refusals. `<f>`, after `realpath`, must be inside the current git
  checkout's working tree and outside its `.git/` and every `.bracken*`
  root entry, and its path there must pass the deny-list above. So
  `--file .env`, `--file ~/.ssh/id_ed25519` and a merged copy in `/tmp`
  are all refused. The file is then opened once, directory by directory
  from the checkout's top with `openat` and `O_NOFOLLOW`, and its type,
  size and bytes all come from that one open file. So a symlink swapped in
  after the path was checked is refused (finding `0bbc434b2588`). The
  content scan then runs on its bytes.

If a secret reaches the Weave Ground anyway, treat it as leaked: rotate it
first, since rotation is the only real fix, and tell the owner. The Ground
can't be edited, and nothing in it can be removed. With the owner's go, the
whole dogfood Ground is moved aside (never edited by hand), and a fresh one
is created and re-imported from git (section 5). Moved aside means moved to
`~/bracken-weave/quarantine/<UTC date>/`, at mode 0700 and outside every
repository. It still holds the secret, so deleting it also needs the
owner's go. Until the join, no Weave content is ever in the coordination
Ground or in git, so the leak stays on this machine.

### Writing a tree out

`checkout` and every check run materialize a manifest the same way. They
never write outside the target directory, and never follow a symlink
(finding `d5112c023747`):
- Only a valid manifest is materialized: no colliding paths, valid
  targets.
- Each directory component is opened with `openat` and
  `O_DIRECTORY | O_NOFOLLOW`. Each file is created with
  `O_CREAT | O_EXCL | O_NOFOLLOW`, and its mode is set exactly to `0644` or
  `0755` with `fchmod`, whatever the umask.
- Symlinks are created last, after every file and directory.
- An update `lstat`s every parent first, and refuses if anything on disk
  differs from the manifest it came from.

### Commands

| Command | Does |
|---|---|
| `weave init [--purpose text]` | Creates a new Ground at `--dir` and writes the Weave marker and a `main` line into it, signed by `--as` (an identity created there), with its `identities/` directory at mode 0700. The line's author is its only integrator and goal setter, with no checks yet. `--dir` must not exist yet and its parent must; the parent's real path is searched for `.git`, and the Ground directory is made with a single mkdir. Refuses inside a git checkout. It guards against mistakes, not against concurrent writers on the path, and doesn't see a work tree declared only through `core.worktree` or `GIT_DIR`. |
| `weave init --in-place --decision <id>` | The join only (section 5): writes the marker into an existing Ground, naming in `in_place` a claim tagged `decision` in that Ground, which must exist. |
| `weave import [--from-git <rev> \| --index] [--dry-run]` | Records the tree as file versions (only those whose content hash is new) and one manifest. Prints the manifest's hash. |
| `weave status [--index] [--at D]` | The index or `HEAD` against main's manifest (or `D`'s, which a forked line needs): added, changed, deleted, mode changed. |
| `weave diff <a> <b>` | Per-file difference between two manifests or decisions. Text files show a unified diff. |
| `weave propose [--line L] [--base D] [--from-git <rev> \| --index] [--summary text] [--dry-run]` | Imports the tree and writes a proposal against `base` (default: the line's head), which must be a qualifying decision on the line. Refuses a tree equal to the base's. |
| `weave integrate [--line L] [--proposals ... [--mirrors REV]] [--run] [--answer O=DISPOSITION:REASON]... [--breakage CHECK[:PATHS]]... [--goal G]...` | Integrator: builds the candidate and writes the decision, in the order below. Runs no checks unless `--run` is given. Refuses if the result wouldn't qualify. Without proposals it records a given tree, `--from-git <rev>` (the default is `HEAD`, named in the decision's `source`), `--index` or `--manifest H`, with every head of the line as a parent, so on a forked line it settles the fork. With `--proposals P...` it merges each valid proposal in turn onto the line's single head, against the proposal's own base, applies the resolutions the Ground holds, and writes the decision listing the proposals and the resolutions it used. The line defaults to the proposals'. A conflict without a resolution stops it: it lists the conflicts and writes nothing. With `--mirrors REV` (stage 2a), the decision names that git commit as its `source`, and nothing is written unless the merged tree equals the commit's tree as `import` would record it; otherwise it lists the paths that differ. Nothing is imported from the commit. `--answer` answers an objection with a disposition and a reason, or, naming its target (a decision, a manifest artifact or a proposal), every objection to it the disposition is allowed for, or, naming the line, every objection in the decision's window the disposition is allowed for; `--breakage` records known breakage of a check, with the paths it affects, under the line's guardrails; `--goal` names a goal the decision serves. It refuses, naming rule 5, while an objection the decision must answer has no allowed answer (`weave review` lists them), and rule 6 when the breakage breaks a guardrail. |
| `weave validate --manifest H [--line L] [--check <name>]... [--again] [--timeout SECONDS] [--runner DIR] [--dry-run]` | Validator: runs the named checks of line `L`'s policy (default: the default line, and all of its checks) on manifest `H` through the check runner, and records each result. Refuses a check it already has a latest result for on `H` unless `--again`. `--timeout` bounds each check (default 1,800 seconds), after which its process group is killed. A box or mac check (*Checks outside the sandbox*, below) needs `--runner`, the directory its pinned runner script is in; mac checks run after the others. `--dry-run` shows the line, its validators and what would run, and runs and writes nothing. |
| `weave review [--line L] [--proposal P]` | The reviews of the line's heads, the open objections the next move must answer, each with its basis and the dispositions it allows, and the reviews of each open proposal on the line (one whose change no decision on it lists), carried over by change, with what a decision using it unchanged would have to answer. With `--proposal`, that proposal alone. |
| `weave head [--line L]` | The head, or the fork's heads, with the manifest hash. |
| `weave green [--line L] [--manifest H]` | Green under the line's current policy: the green decision, none, the ambiguous set, or a diverged chain's versions; and per head, each check's counted passes and fails, with the fails a later attempt replaced, each with its validator and attempt. With `--manifest`, the same counted results on any manifest the replica holds instead of the heads, leaving out the line's integrators' own results, as for a decision they would write, and whether every check is green; `scripts/merge` reads a candidate's results through it (section 7). |
| `weave log [--line L] [--limit N] [--since D]` | Decisions on a line, newest first in causal order, with their checks and breakage. At most 20 unless `--limit` says otherwise; never prints payloads. |
| `weave slice [--at D] <path or prefix>...` | Those paths' contents at a decision or manifest, for an agent that needs a part, not the whole. |
| `weave checkout [--at D] <dir>` | Materializes a manifest into an empty directory, or updates a directory that is clean against it. If writing fails part way, it removes what it wrote, or names what it couldn't remove. |
| `weave line create <name> [--dry-run] [--integrator ...] [--validator ...] [--goal-setter ...] [--check NAME=ARGV ...] [--executor NAME=KIND:SCRIPT ... --runner DIR] [--max-known N] [--max-age N] [--protected P ...] --owner-decision <id>`, `weave line policy <line> [--dry-run] [--from <version>] [--name n] [--clear <field>]... ... --owner-decision <id>` | Creates a line (with no integrator or goal setter given, its author is the only one), or writes a new policy version, starting from the current one, that supersedes every current version, so it also settles a diverged chain. On a diverged chain it refuses, listing the current versions, unless `--from` names the one to start from (finding `4eead6290997`). Only the line's author can change a policy. `--owner-decision` is required, as 12 to 64 hex digits. `--executor` runs a check outside the sandbox, `KIND` being `box` or `mac`, through `SCRIPT`, a path inside `--runner` whose content hash the policy pins; `--clear executors` drops them, and `--clear checks` drops them with the checks. Prints `line <id>`, then `relation <id> (supersedes)` for each relation. `--dry-run` shows the policy, and what it would supersede, and writes nothing. |
| `weave line merge <from> --into <line> [--base D] [--dry-run] [--note text]` | Integrator: merges `<from>`'s head into `<line>`'s, file by file against the merge base (`--base` only chooses among several). With every conflict resolved by an integrator's resolution, it writes the merged manifest and a decision on `<line>` whose parents are both heads, judged by rules 1-4, so its author must be an integrator of `<line>`. With a conflict unresolved, or with `--dry-run`, it lists the conflicts, with the resolutions offered for each, and writes nothing. It lists the resolutions it used. A line with no decision yet starts from `<from>`'s head, its first decision having that head as its only parent (answer `12000ab70ae5`). |
| `weave resolve <path> (--into L --from L2 [--base D] \| --proposals P... [--line L]) (--take into\|from\|<resolution> \| --delete \| --file <f> \| --markers <f>) [--mode m] [--dry-run]` | Writes a resolution for the conflict at `<path>`. It takes the selector of the command that listed the conflict (`line merge`'s `--into`, `--from` and `--base`, or `integrate`'s `--proposals`), rebuilds that merge, and recomputes the key, so nobody types file values. `into` is the line's side, `from` the side merged in, and a resolution id adopts that resolution's result. A conflict may get another resolution even if it has one. `--file` has the refusals above (answer `12000ab70ae5`). `--markers <f>` writes no resolution: it writes a text conflict's marked merge to a new file to edit, for `--file`. |
| `weave verify --git <rev> [--at D \| --manifest H]` | Compares a manifest with a git commit's tree: the same paths, modes and bytes, with the excluded root entries left out. Used throughout the dogfood. |
| `weave check` | Lists Weave payloads that are invalid or break the ancestor rule, manifests not yet complete, markers past the one that counts, and policy versions ignored for citing no owner decision. It exits non-zero when it lists any. |

Plain commands work on Weave records too: `relate ... supports` or
`contradicts` reviews a decision, proposal or manifest, and `claim --tag
weave-goal` plus `relate ... refines` sets a goal. Check results are the
exception: `scripts/weave` refuses the plain `evidence` command, so a result
on a manifest only ever comes from the check runner below.

### The check runner

`weave validate` and `weave integrate --run` run checks the same way, and
nothing else runs a check on a Weave manifest (security note
`1c63577b405d`, finding `11e4a679401d`). For each named check:
- The manifest is written by the safe materialization above into a fresh
  temporary directory, and the check runs there. It never runs in the
  caller's directory.
- Before running, it prints the policy version's id and the argv. It runs
  that argv, never through a shell. `argv[0]` is resolved against the fixed
  `PATH` below before the change of directory, so a relative entry can't
  let the tree pick the binary. A bare name is never looked up in the tree;
  a name with a `/` (`./scripts/test.sh`) is a path inside the tree, and one
  that is absolute or has a `..` component is refused.
- Its environment is minimal, and every `BRACKEN_*` variable and every
  token path is dropped, so a test that runs bracken can't reach the live
  Weave Ground:
  - `PATH` is fixed: the system's directories and the toolchain's `bin`;
  - `LANG`, and a fresh `HOME` and `TMPDIR` inside the temporary directory;
  - `RUSTUP_HOME` is the owner's rustup directory, read-only;
  - `DEVELOPER_DIR` is the Command Line Tools, so the linker finds its SDK
    there and not in an Xcode the profile doesn't allow;
  - `CARGO_HOME` is a fresh directory inside the temporary directory, whose
    `config.toml` points cargo at a read-only vendor directory,
    `~/bracken-weave/vendor`, and sets `offline = true`.
- **Dependencies come from the vendor directory.** It is filled only
  outside the sandbox, by the manager running `cargo vendor` on a tree git's
  `main` holds, never on a candidate. A candidate that needs a crate that
  isn't vendored fails its check, and the output says which crate. So
  `~/.cargo` and `~/.rustup` are never writable from a check, and unreviewed
  code can't plant something that a later, unsandboxed build would run.
- **The sandbox profile is deny-by-default**, like the experiment's
  (`crates/experiment/src/sandbox.rs`), and on macOS it is applied with
  `sandbox-exec`:
  - reads are allowed only from the temporary directory, the system's own
    directories, the vendor directory and the toolchain, read-only. The
    temporary directory is the fresh one alone, not its parent `$TMPDIR`,
    where throwaway Grounds keep keys. So a check can't read any
    `identities/` directory, `~/bracken-weave` outside the vendor
    directory, the main checkout's `.bracken/` or `$main/.bracken-team`,
    `~/.ssh`, `gh`'s credentials, git's credential stores, or
    cargo's `credentials*` and `config*` files;
    Metadata (`stat`) stays readable everywhere, as in the experiment's
    profile, since tools resolve their paths through directories they can't
    read; listing or reading those directories is still refused;
  - writes are allowed only inside the temporary directory;
  - exec is allowed only from the temporary directory, the toolchain and
    the system's directories;
  - all network is denied, loopback and unix sockets included, and so are
    mach-lookup and other IPC, so `open` or `osascript` can't hand work to
    a process outside the sandbox;
  - listing the process table (`sysctl kern.proc`, `proc_listallpids`) is
    denied, and so is reading any process's arguments and environment
    (`KERN_PROCARGS2`, `KERN_PROCARGS`), path or `proc_pidinfo`, by
    `(deny process-info*)` with no `(target others)`, which leaves out the
    check's own process group (note `1301a2adcc85`). So are `getpgid` and
    `getsid`, which would tell a pid's group and session, for every pid
    since the rule can't tell a process's own apart (task `72fd057d4b33`),
    and `setsid` and `setpgid`. Which pids are live can still be learnt, one
    pid at a time: `getpriority` answers for a live pid or group, and a
    denied call fails differently for a live pid and a dead one (finding
    `f55c629069a9`). None of that gives a process's arguments, environment,
    name or path. `posix_spawn` with a new session or group can
    still leave the process group, so on every exit path the runner stops
    the group, finds the check's descendants by parent pid (again until no
    new ones appear), stops and kills each, and fails the run if any had
    left the group. The gap that remains: a process that left the group and
    was orphaned (its parent exited, so launchd adopted it) before the walk
    can't be found. It stays a documented limit: checks run as the owner's
    own user (decision `6f15365a7281`). A child that joins
    another group of the session, the runner's included, through
    `POSIX_SPAWN_SETPGROUP`, is found and killed the same way, but only while
    its parent chain to the check is alive. Orphaned there, it escapes as any
    orphan does, and if it closed its output nothing fails the run (finding
    `7a937874d039`). The runner doesn't kill the rest of its own group, the
    processes in it that it didn't start: that group is its caller's, which
    holds the shell or script that started it, its pipelines (`tee`, `cat`),
    their other jobs, and helpers git or cargo left behind, and nothing tells
    those from a check's orphan;
  - each check has a timeout, and its whole process group is killed when it
    ends or times out.
- **No sandbox, no run.** If the profile can't be applied, on another OS or
  because `sandbox-exec` fails, the runner refuses to run the check and
  records nothing.
- **Its result.** The `method` is `<name> blake3:<definition hash>`. The
  `details` start with the manifest hash, then the attempt number, the
  argv and the exit code, then the output tail. The tail is permanent and
  could carry what a check read, so before it is recorded, control
  characters are stripped and the content refusals of import run on it. If
  any refusal matches, the tail is replaced by its size and content hash,
  and the result says it was withheld.

Stage 2 of the dogfood doesn't start until this runner and its sandbox
exist, on the validators' path as well as the integrator's.

### Checks outside the sandbox

Some of the test table can't run under the sandbox: the Linux half, which
runs in a remote box, and tests that use loopback, read the repository's
git, or apply `sandbox-exec` themselves (plan `cc496dcee0e3`, owner
decision `9a218021bbf0`). A policy runs such a check through an
*executor*: the line's optional `executors` field maps a check's name to
its kind, `box` or `mac`, and a runner script, a relative path inside the
validator's runner directory (the main checkout), pinned by its content
hash. The field is left out of the payload when empty, so a line without
it encodes as before. A line version that has one doesn't decode with a
build from before the field, since it wouldn't re-encode the same, and such
a build skips it without a word: it reads the line under the version
before, which has no executors (finding `5d50422b9e11`). So every binary
that reads a Weave Ground, the pinned one included, must be at least the
build that added executors before a policy with them is written, and is
never rolled back below it (docs/releases.md). From that build on, a line
version that doesn't decode is reported: `weave head`, `green`, `review`
and the other reads warn, `weave check` lists it, and `validate`,
`integrate`, `line policy` and `line merge` refuse, doing nothing, until
the binary is upgraded. Only an artifact that would change a line counts:
one that supersedes a version of its own author's, in a relation that
author signed, as a policy change does. A stray artifact, or one whose
relation someone else signed, changes no line here, and `weave check` alone
lists it (finding `e89cf76e9ea2`). The definition hash of a check with
an executor is that of `[name, argv, executor]`, so a result names the
kind, the script and its hash, and changing any of them is a new policy
version, under the owner's decision.

Only `weave validate` runs one, as one of the line's validators, through
the same command that records every other result, so `scripts/weave` still
refuses a plain `evidence`. Before it runs:
- the runner script's content hash must equal the pinned one;
- the manifest is written by the safe materialization into a fresh
  directory and made a git commit there, with no configuration, hooks or
  attribute drivers of the caller's or the system's, and every file added,
  ignored or not. The commit's import must be exactly the manifest, with
  nothing excluded or refused, or nothing runs: an attribute that makes git
  store a file otherwise is refused. That commit, and its tree id, stand
  for the manifest.

Then a copy of the script, read once and checked against its hash, runs
from a directory of its own beside the checkout, with the commit, the tree
id and a facts file in its environment. The script must be
self-contained: anything else it runs from the runner directory is not
pinned. The facts file is out of the tree's reach only for a box check,
whose tree runs in the box; a mac check's tree runs here as the same user
and could write it, so a mac check's facts are a consistency check, not a
proof: a passing mac check must give the commit and tree, a failing one
may give neither, and any it gives must be these, or the run is refused
and nothing is recorded (finding `79114ae0b4ab`). The test table's mac mode
also requires its checkout's tree to be the tree id and its working tree
to be clean before any of the tree's code runs.
- **box**: it also gets a bundle of the commit and the bundle's sha256, and
  the caller's `PATH`, `HOME`, `USER` and `LOGNAME`, for boat. It starts
  in the runner script's own directory, not the checkout, and the `PATH`
  keeps only its absolute entries, so no file of the tree can stand in for
  a program the script, or `env` on its first line, looks up (finding
  `367a51649450`). In the box,
  before any of the tree's code runs, the runner's inner script prints the
  bundle's sha256, the commit and the checked-out tree id; the script
  writes those, the box's id as boat announced it, boat's cost line and,
  only if the box printed its end line after the table passed, `end`, into
  the facts file;
- **mac**: it runs in the commit's checkout on this Mac, with the check
  runner's environment (no `BRACKEN_*` variable or token path, a fresh
  `HOME`, `TMPDIR` and `CARGO_HOME` reading the vendor directory offline),
  the directories of `cargo-nextest`, `timeout` and `jq` on its `PATH`, but
  no sandbox profile. Its `HOME` holds `.cargo`, which is its `CARGO_HOME`,
  and `.rustup`, a link to the toolchain's rustup home, where the tests that
  run `validate` look for it. A box or mac check's fresh directory is made
  in `/private/tmp`, not the per-user temporary directory, so that a Unix
  socket a few directories under its `TMPDIR` stays within the 104 bytes
  macOS allows (note `20e075f475e0`). It's given the output of the validator's latest
  passing result on the manifest of each box check, which the test table
  compares its own test list with.

A box check's facts must give the commit, tree, bundle's sha256 and a box
id (`bx_` and letters, digits, `_` or `-`), or the run is refused and
nothing is recorded; without `end` it fails. Its result's details give,
after the exit, the commit, tree, bundle, box, cost, `end seen` or `end
missing` and output size, then `log <artifact> <hash>`: the whole output,
recorded as a `text/plain` artifact after the content refusals, or
`log withheld:` with its size, the whole output's hash (taken as it was
read, so it covers an output too big to keep) and the reason: it matched a
refusal, or it is over what an artifact may hold, 15 MiB, a MiB under the
contribution limit. A mac check runs only given a box check's recorded log: with none
on the manifest, `validate` refuses it and records nothing for it.

A box or mac check's timeout is an hour by default (`--timeout` sets it),
past boat's own 2400 s. At its timeout, or when `validate` gets SIGINT,
SIGTERM or SIGHUP (a Ctrl-C, a kill, a closed terminal), its process group
gets a SIGTERM and 60 seconds to end, so the box script's trap deletes its
box, before it is killed. The group has no terminal of its own, so only
`validate` can pass a Ctrl-C on. An interrupted check records nothing, and
`validate` fails (finding `02ab829d2caf`), also when the signal comes after
the check's process ended but before its run was over (finding
`11e32fff3656`). Nothing catches a SIGKILL of
`validate`: then the script's output pipe breaks, and the script must
clean up on that too, as test-table's Weave modes do.

Once the tree's code runs it can do whatever its user can, in
the box or here: the proofs establish which tree ran and that a run
happened, not that the tree's own tests are honest. `integrate --run`
runs no box or mac check.

**One counted run per manifest.** A check's result attaches to the manifest
hash, so each check runs once per manifest for green: a validator runs
`weave validate`, and every decision with that manifest uses the result.
`integrate` runs nothing by default. `--run` is opt-in, for the
integrator's own information, and its results never count for the
integrator's own decision. They count for another integrator's decision
on that manifest if the integrator is one of the policy's validators, as
section 2's eligibility rule says (decision `195c6410262f`). In stage 2 the
line's only validator is a named runner identity, `weave-runner`, which
runs the test table's checks once per candidate (retrospective
`16cbeff051ea`: 57% of the results on pr claims never counted, and the
owner objected to repeated runs, `642a47c04a10` and `b7a4d8a66357`).
`scripts/weave` accepts `--as weave-runner` only for `weave validate`, and
runs `validate` only for an identity in the line's `validators`. Under
shared keys, both are conventions too: a plain binary can still sign as
anyone.

### `integrate`, step by step

Evidence can only target a contribution that already exists (protocol
rules 4 and 5), so the order is fixed:

1. **Build the candidate.** Start from the line's head, merge the
   proposals per file, and resolve conflicts with resolutions.
2. **Write the candidate.** Write its new file versions, after the content
   refusals above, and its manifest artifact.
3. **Only with `--run`: run the checks** through the check runner, and
   record each result on the manifest artifact.
4. **Write the decision**, which names the manifest's content hash, with
   its answers, breakage and goals, if it would qualify, by every rule
   including 5 and 6. Otherwise write nothing more: the candidate, and any
   failing results, stay on record, and `integrate` reports which rule
   stopped it.

A validator then runs `weave validate --manifest <hash>`, and green follows
from its results.

## 4. Storage and size

- **One file version per content hash.** `import` writes a file version
  only when the replica has no artifact with that content hash yet, so
  unchanged files cost nothing. In v0 the hash index is built in memory when
  the Ground opens. A persistent index would change the storage format,
  which is the storage class, and is left for later.
- **Every version is stored in full.** There are no deltas. For this
  repository that is small: 5.9 MB for the whole tree. Its largest file is
  `crates/bracken/tests/scripts.rs` at 145 KB, so each new version of it
  stores another 145 KB.
- **Files over the limit.** A file whose artifact encoding would pass the
  16 MiB contribution limit is refused by `import`, which lists it. It stays
  in git (section 7).
  - If one is ever needed, chunking is a payload change, not a protocol
    change: a file version whose payload lists chunk hashes, each chunk its
    own artifact.
  - A manifest past the limit splits by directory the same way: an entry
    with a `tree` mode points at a sub-manifest. That is format version 2.
- **Opening reads everything.** A Ground's whole history is decoded when it
  opens. That is one more reason the Weave Ground stays apart from the
  coordination Ground, whose every `scripts/bk` call would otherwise pay for
  Weave's growth.
- **Expect about 45 MB in four days** on this repository (retrospective
  `16cbeff051ea`): main's history alone has 801 file versions (19 MB), and
  each candidate adds a manifest of about 20 KB. That is eight times the
  coordination Ground's bytes, which already take 0.75 s to open for every
  read. So build step 5 measures open time on a 50 MB fixture Ground before
  stage 2 starts, and reads stay bounded: `weave log` prints at most 20
  decisions by default and never prints payloads. Loading payloads lazily
  is left for later (section 8).
- **What opening costs.** Opening checks each distinct path once, however
  many manifests list it, and answers every ancestor question without a
  walk per question (design note `e93d47ad8a20`, answer `b2dbc4f5e810`).
  Questions are answered 64 contributions at a time: one pass over the
  history in causal order per 64, with two words per contribution reused
  by every pass.
  - A record's references are each kept as a 4-byte index and a bit, and
    the 1,024-reference cap bounds them by the record's size (finding
    `1873f4834edd`). All markers together are one more target, so each
    marker keeps one bit: is another marker a strict ancestor?
  - Lines asks its own chain questions, after it has dropped the versions
    that count for nothing. A decision or proposal naming a version asks
    about every supersedes relation in that version's component: the
    versions the relations Lines counts join, whichever way. Lines only
    follows those relations from the line, so it asks nothing else, and no
    rule about which versions are first artifacts is guessed in advance.
    Each relation is in one component, so stored once. Each record keeps
    its verdict, not its answers.
  - **Shared answers.** Records whose pasts provably hold the same
    relations share one answer vector. A contribution's class is fixed by
    its parents' classes, leaving out the class of contributions with no
    relation below, and by which parents are relations asked about: with
    one such class and no such parent it keeps that class, and otherwise
    it gets the class of that combination, made when first seen. So the
    decisions written after a line's last policy change cost one vector
    between them, however many there are, and so do honest merges of the
    same history (finding `6c8ea309c1e2`). Records whose pasts are equal
    but reached through different combinations still get different
    vectors: a writer can make one per record with a side branch that
    holds a relation of its own.
  - **The batch budget.** Answer vectors are held at most one 64-bit word
    per contribution at a time, then turned into verdicts and freed. A
    component with more vectors than fit takes more rounds, not more
    memory, so memory is linear in the history in every shape. A round's
    pass over 64 relations sweeps only from the first of them to the
    round's last record, and is skipped if they all come after it.
  - **Time.** With `K` relations in a component and `U` answer vectors
    among the records naming it (at most one per record), opening takes
    at most `ceil(U * ceil(K / 64) / n) * ceil(K / 64)` passes over at
    most `n` contributions, summed over components. That is cubic in what
    one writer adds when the writer makes both `U` and `K` grow with `n`.
    For example, a policy change then a decision, repeated, gives
    `U = K = n / 2`: the bound is `n^2 / 32,768` passes, and the sweeps'
    limits halve that, to 15,750 passes and 1.3 seconds at `n = 32,000`
    (`ancestry::rounds` on a chain of `n`, release build; finding
    `98ce07a5e421`). That is still never slower than walking the graph per
    question, as opening did before. Decisions written after the chain,
    the finding's other shape, share one vector: 250 passes and 26 ms at
    the same size, where opening without shared answers took 31,250 passes
    and 3.1 s.
  - **A diverged policy's versions.** A decision whose line's policy has
    diverged keeps the count and at most the first 8 versions
    (`DIVERGED_SHOWN`), as one list shared by every decision that found
    the same ones; `Lines::current_versions` lists them all. Otherwise a
    line's author who adds a diverging version then a decision, repeated,
    would make each decision keep its own longer list: about `K^2 / 2`
    versions (task `a90865cb60db`).
  - **Check results** (build step 7). Only results by a validator under
    some policy version are asked about, so nobody else's evidence costs
    anything. The latest result of each validator per manifest and check
    takes a pass per 64 results: each result reads its own word and marks
    the results of its group below it, keeping a bit per result. The
    policy epoch is one more reference per result, to the current policy
    version of each line whose checks it names. Counting over a decision's
    own past, under the policy in force for it, asks which of the results
    and their retractions are in that past, and whether the policy version
    is in each result's: references, a bit each. Which of those results
    lie below one another is a list per validator, check and manifest,
    answered once per list however many decisions are counted: each
    eligible result reads its word once, and ORs it into a mark word per
    decision it is eligible for (finding `eb8b992ae58f`). So a list of `N`
    results costs `N/64` passes, `N^2/64` words read and, per decision,
    `N/64` words of marks. Measured in a release build on an M-series Mac,
    one validator's 8,000 failed results on one manifest take about 0.25 s
    for one decision and 0.3 s for ten (4.3 s for one before). A later
    re-run or retraction never changes what the decision saw.
  - **Objections** (build step 7, rules 5 and 6). Only a decision that may
    qualify is worked out. One that is retracted, on no line, by no
    integrator under the policy in force for it, or with a parent that may
    not, fails rule 1, 2 or 3 whatever it must answer, and so does every
    descendant, so it costs no more than its own count (finding
    `e4f57cd207b6`). For the rest, each decision's window (the objections to
    its ancestors in the decision graph), the answers below it and the
    objections to what it uses come from the decision graph, with no Ground
    walk. Every question they ask of a decision's past is asked at once:
    whether each objection, the retractions below it, its basis's result and
    goal links and their retractions are in the past, and whether the policy
    in force is in the result's, all references through one ancestor answer.
    A decision asks one question per objection in its window that no answer
    below it settled, so an objection is asked about at each move only until
    it is answered: 2,000 objections settled by one answer cost later moves
    nothing.
    - A decision with one parent on its line takes the parent's window,
      less what the parent settled, plus the objections to the parent, and
      shares it if there are none of either. Each decision also keeps its
      history (the objections to it and its ancestors, settled or not), what
      answers there and below settled, by line, and the manifests of the
      qualifying decisions there and below. These are persistent sets of
      words, a bit per objection or manifest, that share what they have in
      common, so a decision adding nothing shares its parent's, and adding
      one copies a path of seven nodes. What was settled is a persistent map
      of such sets, from a number per line that anything was settled on,
      with eight levels of 16 ways. A settle copies the path to its line, and
      a merge copies only the paths where its parents' maps differ (finding
      `0a2094d4cf7b`). So a decision costs the lines it settles on or brings
      in, not every line settled on below it.
    - Settles reach only decisions on the settling decision's line, so a
      parent on another line gives no window to share. A merge's parents may
      also each have settled what the other's window holds. Either way, the
      window is the parents' histories less what was settled on the
      decision's line, read a word per 64 objections, and only where the two
      differ.
    - A `fell_back` answer's bases, the manifests of the objected decision's
      qualifying strict ancestors, are its parents' sets, whether the answer
      names an objection, the decision itself (blanket) or the line (findings
      `fa578bd1da48` and `67aa4fdb882b`).
    - Settling asks, once per answer, whether each objection it names, and
      each retraction below it, is in the answering decision's past. An
      answer naming the line names the window, which settling itself
      decides, so with such answers settling takes two passes: the first
      leaves them out, and asks about the window it finds, which holds the
      window the second finds.
    - Measured in a release build on an M-series Mac, a line of 4,000 moves,
      each objected to and answered by the next, asks 7,998 questions. It
      builds in 0.06 s answered `fixed` and 0.08 s answered `fell_back`
      (0.54 s and 3.0 s before). 4,000 decisions by someone else on a
      made-up line, with its head as their parent, add no questions and
      about 0.01 s (62 s before). Left unanswered, the line asks 8 million
      questions, one per objection per later move, in 1.1 s. What grows
      faster than the line is the ancestor index's passes, one per 64
      askers: at 8,000 moves, answered `fixed`, it builds in 0.16 s (2.2 s
      before). 8,000 lines, each settling the one objection at its root,
      merged into main one by one, build in 0.9 s, allocating 152 MB in all,
      twice what 4,000 lines allocate (5.1 s and 2.98 GB peak before: each
      merge copied a map of every line below it). `measure_long_lines` in
      `crates/tree/tests/objections.rs` gives these figures.
    - Rules 1 to 3 stop junk only from someone who can't qualify. A line's
      own integrators can make it cost the stated bound. On a line someone
      makes for themselves, `n` moves each objected to and left unanswered
      cost `n^2/2` rule-5 questions, as on any line: each move must answer
      every objection to the moves below it. Only a reader that asks about
      that line, or about a line whose decisions descend from its moves,
      pays this cost (section 2, *Who pays for a line*). With 300 such
      moves beside main, judging main asks exactly the questions it asks
      without them, and no stray move is judged, while a full build asks
      44,850 questions. In the CLI, `weave head`, `weave check` and
      `weave line merge main --into dev` judge the same decisions with 40
      such moves as with none, and `weave log --line stray` judges all 40
      and asks 780 questions (`a_line_nobody_asks_about_costs_a_reader_of_main_nothing`
      in `crates/tree/tests/objections.rs`, and
      `a_line_nobody_names_costs_main_nothing_and_naming_it_judges_it` in
      `crates/bracken/tests/weave_lines.rs`).
  - A question nothing asked at build would be a bug. It is answered by
    walking and counted, and fails debug builds; the tests assert the count
    is zero.
- **Compaction is left open.** Signed history can't be shortened without
  losing what it signs. A later checkpoint scheme (Rings, protocol.md *Not
  yet specified*) is where it belongs.

## 5. Migration, and protecting the coordination Ground

### Protocol changes

**None in v0.** No encoding, id, signing context, validity rule or body kind
changes. Existing Grounds open unchanged. A binary from before Weave opens
and syncs a Weave Ground, and shows its records as artifacts and claims.

Weave's own payload formats are pinned instead:
- by golden vectors in `crates/tree/tests/format_guard.rs`: one record of
  every Weave media type, with its bytes and content hash;
- and by a fixture Weave Ground.

Changing a pinned format is reviewed as protocol class: an independent
review, the manager's go logged as `owner-review`, and an entry in
`docs/releases.md`.

**Deferred protocol changes.** These aren't needed for v0, and each needs
its own go. The `Body` and `RelationKind` sets are closed, and an older
binary meets a new kind as section 1 describes: a Ground holding it doesn't
open, and sync stops at it.

1. **P1: tolerate unknown kinds.**
   - Encoding: a body kind, or relation kind, outside the known set decodes
     as an opaque `unknown { kind: text, raw: bytes }`, keeping its exact
     bytes.
   - Canonical: an unknown body must still be one CBOR map with one text
     key, canonical by the same decode-and-re-encode check over generic
     CBOR.
   - Id and signing: unchanged, over the same canonical bytes.
   - Validity: rules 1 to 4 apply as now; rules 5 to 8 apply only to known
     kinds. An unknown kind's references can't be checked, so views ignore
     it.
   - What a binary from before P1 does with an unknown kind: fails as today.
     So P1 must reach every replica before any new kind is written.
   - **The Ground can still split** (note `31bfa91416e8`). A P1 binary
     accepts any instance of a new kind under rules 1 to 4. A binary that
     knows the kind may reject an instance, for example one whose references
     aren't in its parents. Then every descendant written by a P1 binary
     stays unresolved on the newer one. So a kind added after P1 may add no
     validity rule beyond 1 to 4, and checks the rest only in its views, as
     Weave does with its payloads.
2. **P2 isn't a protocol change**, and is dropped from this list.
   Letting an integrator retire another author's file version would be a
   status rule about lines and file versions. That is Weave's business,
   which would belong in `crates/tree`, never in manyroot ("Nothing in
   `manyroot` may assume a contribution is code"). Under shared keys, it
   would let any agent retire anyone's work. v0 doesn't need it, because
   manifests select without retiring anything.
3. **P3: cross-Ground references**, so a contribution in one Ground can cite
   one in another (Canopy).
   - **It is a breaking encoding change, even after P1** (finding
     `da2bd32ace14`). A foreign endpoint inside an existing `relation` (a
     map `{ground, id}` where a 32-byte id is expected) is a new schema for
     a known kind. P1 tolerates unknown kinds, not unknown shapes of known
     ones, so a P1 binary still fails to decode it. To be safe for P1
     binaries, P3 must be a new body kind, for example
     `foreign_relation { from, kind, to_ground, to, note }`, which a P1
     binary keeps opaque. It must also add no validity rule beyond 1 to 4,
     as P1 requires.
   - Id and signing: unchanged.
   - Foreign references can't be verified locally. Replay into another
     Ground isn't a risk, because the signed content includes `ground`.
   - Weave would use it for the join below. v0 cites ids in text instead.
4. **Per-agent keys** aren't a protocol change: they are identities and
   governance. They turn this spec's author-based conventions into
   guarantees (the summary's caveat).

### Where the Weave Ground lives

The dogfood Ground is a replica directory at
**`~/bracken-weave/manyroot`** on the owner's machine, the same machine and
user as the coordination Ground. Every worktree reaches it through
`scripts/weave`.

- **Outside every git checkout.** So no `git add`, checkpoint or worktree
  can stage it, and bracken's nearest-`.bracken` lookup from inside the
  repository never finds it. `weave init` refuses to create a Ground inside
  a git checkout.
- **Its keys are never committed.** Its identities live in
  `~/bracken-weave/manyroot/identities/`, created at mode 0700, outside
  every repository. They are never copied into a repository or into the
  coordination Ground, and never printed. A leaked key follows
  releases.md's *Rollback*.
- **It is never edited or deleted by hand.** Write only through
  `scripts/weave`, the same rule as for `.bracken/`. AGENTS.md gains that
  line in the PR that adds `scripts/weave`.
- **Its identities are its own.** It doesn't share them with the
  coordination Ground: the integrator is `weave-integrator`, and builders
  use their usual names.
- **It never syncs in v0.** `scripts/weave` never reads `.bracken-team` or
  its tokens, since every agent runs on this machine. Hosting it on the
  team server later is new infrastructure, which needs the owner's go.
- **It is backed up only by the owner's choice.** Stage 1 can be rebuilt
  from git; stage 2's proposals and evidence can't, so a backup comes
  before stage 2 (section 8). `scripts/weave-backup --owner-decision <id>
  <dest>` copies it, identities included, to `<dest>/<UTC time>/`, and
  only to the destination the owner approved: `<id>` is an approval the
  owner signed, not superseded, whose text names `<dest>`'s real path.
  It writes the copy
  beside that, and moves it into place only once the pinned binary opens
  it and it holds at least what the Ground held when the copy began. It
  copies again while the counts differ, up to 5 passes, and once they
  agree the heads and identities must too. A Ground written to throughout
  gives a copy that opens and holds at least what it held at the start,
  and the script says so. It keeps the newest 20 copies, the new one
  among them, and deletes only older copies of this Ground. `<dest>` must
  be a directory of yours at mode 0700 with no ACL, outside every git
  checkout and `~/bracken-weave`, and not a symlink. Each directory above
  it must be yours or root's, with no ACL that allows anything, and
  writable by others only if it is sticky, since a copy holds the private
  keys. The script prints counts only. The manager runs it by
  hand. A schedule is the owner's go too (task `5b81f744a0ea`).

### Keeping the two scripts apart

Finding `56de5febc425` and note `b141f90ebf5e`: an environment variable and
a fixed path aren't enough on their own, because the CLI's global `--dir`
overrides `BRACKEN_DIR`, and anyone can write a Weave marker with a plain
`artifact` command. So the scripts check what they will actually open:

- **Both scripts refuse `--dir` and `--dir=`**, anywhere in the arguments.
  They set `BRACKEN_DIR` themselves and ignore an inherited one.
- **Both find the real command** by parsing the arguments past global
  options (`--as`, `--as=`), as `scripts/bk` already does for writes, never
  by the first argument alone.
- **`scripts/bk` refuses Weave.** It refuses the command `weave`, and an
  `artifact` whose media type starts with `application/vnd.bracken.weave`,
  given as `--media-type <t>` or `--media-type=<t>`, compared without
  regard to case. It uses the same matcher as the ground-guards tripwire
  below.
- **`scripts/weave` refuses `evidence`**, so check results come only from
  the check runner (section 3).
- **`scripts/weave` refuses Weave records made by hand**: an `artifact`
  with a Weave media type, matched as `scripts/bk` matches it, and `relate`
  with `supersedes` among any of its arguments, those after `--` included,
  since bracken still reads the kind there (finding `56dadb600ba2`). Only `bracken weave` writes Weave records, and only
  `weave line policy` writes a line's supersedes relations, so the
  owner-decision rule can't be bypassed (finding `12643881ac59`). This
  applies only under `scripts/weave`, against the Weave Ground.
- **`scripts/weave` checks the Ground it opens.** It uses the same pinned
  binary. Before running anything, it resolves `~/bracken-weave/manyroot`
  with `realpath`, exports that resolved path as `BRACKEN_DIR`, so the path
  it checked is the path bracken opens, and refuses if any of these holds:
  - the path is inside a git checkout;
  - it is the main checkout's `.bracken/`;
  - its `ground.json` id equals that of the main checkout's
    `.bracken/ground.json`.

  So a symlinked `~/bracken-weave` can't lead it to the coordination
  Ground.
- **A checkpoint tripwire.** Until the join, `scripts/ground-guards` (run
  by every `ground-checkpoint`) refuses a `.bracken/` holding any
  contribution with a Weave media type. It names the contribution and says
  to tell the owner. A marker written into the coordination Ground by hand
  is then caught before it reaches git.
- **The pinned binary gains `weave` commands through a normal CLI upgrade.**
  Weave adds no kinds, so the upgraded binary reads the coordination Ground
  exactly as before. A fixture test checks that: it opens the
  coordination-format fixture Ground with Weave linked in, and compares
  every status.

### How the two are joined later

Joining needs an owner decision once Weave is stable: its formats are
frozen, and it has been dogfooded through many merges. Two facts make the
join simple:
- Weave adds no kinds, so the coordination Ground can hold Weave records
  without any reader breaking.
- Content hashes don't depend on the Ground, so a tree imported into either
  Ground has the same manifest hash.

The join is a **cutover, not a copy**, in one PR of protocol class (an
independent review and `owner-review`):
1. The owner decides the cutover point: a green decision in the dogfood
   Ground, verified equal to a git commit. The manager records that as a
   claim tagged `decision` in the coordination Ground.
2. The PR lifts the ground-guards tripwire and lets `scripts/weave` open the
   coordination Ground.
3. `weave init --in-place --decision <that claim>` writes the marker into
   the coordination Ground. It refuses unless the claim exists there, and is
   tagged `decision`.
4. `weave import --from-git <that commit>` records the tree. Its manifest
   hash equals the dogfood decision's, which the cutover record states.
5. The dogfood Ground stays as a read-only archive, and its history is cited
   by id.

Copying history across instead would mean re-signing every contribution,
because `ground` is inside the signed content. That needs every author's
key, and changes every id and every evidence link, so it isn't proposed.
Linking the histories properly is P3's job.

Until the join, a coordination claim that refers to Weave work cites it in
text as `weave:<decision id>`, for example in a pr claim.

## 6. Build plan

Each step is one small PR with its own tests, in this order. Each step
depends only on the steps before it (finding `b4024802b2d2`). None needs a
model run.

| # | PR | Class | Tests |
|---|---|---|---|
| 1 | `crates/tree` (package `bracken-tree`): payload formats for every record, canonical CBOR, content hashes, file values, a proposal's `change`, check definition hashes, the policy's `owner_decision`; path, folding, collision and symlink-target rules | library | Golden vectors (format guard); every path and target rule, positive and negative, including NFC/NFD and case collisions and blank components; decoding then re-encoding round-trips; a check's definition hash changes with its argv |
| 2 | Import from git (`--from-git`, `--index`) with exclusions, refusals and `--dry-run`; safe materialization of a manifest | library | Untracked and ignored files never read; each deny-list entry and every key pattern refused, including a raw hex key from an `identities/` directory, and this repository imports cleanly; a nested `.bracken*` component refused; both counterexamples of `d5112c023747` refused; exact modes whatever the umask; deduplication; the over-limit file |
| 3 | CLI, manifests only: `weave init` (with the marker check on writes), `import`, `checkout --manifest`, `diff` of two manifests, `verify --git --manifest`, `check`, with `--json`; labelled output | cli | A throwaway Ground per test; refusal without a marker; `verify` against this repository's own git tree; every write's labelled lines and `--json` fields; an id of the wrong kind refused, naming its kind; a bare hex value refused where `blake3:` is expected |
| 4 | `scripts/weave`; `scripts/bk`'s refusals; the ground-guards tripwire; AGENTS.md lines on the Weave Ground | merge-gate tooling | `--dir` and `--dir=` refused by both; the command found past `--as`; Weave media types refused by `bk` as `--media-type <t>` and `--media-type=<t>`, in any case; `evidence` refused by `weave`; a symlinked path and a matching Ground id refused by `weave`, which exports the resolved path; the tripwire on a Weave artifact in `.bracken/`; `--as weave-runner` refused for anything but `validate`; a line write without a non-empty `--owner-decision` refused |
| 5 | Lines and decisions under qualification rules 1 to 4; heads and forks; the ancestor rule; `head`, bounded `log`, `status`, `diff` and `verify --at`; `integrate` with no proposals and no checks (it records a given tree) | library + cli | Forks by two integrators; merge settles them; a reference that isn't an ancestor is ignored; shuffled timestamps give the same heads; `log`'s default limit; open time measured on a 50 MB fixture Ground |
| 6 | Proposals, per-file merge with file values, conflicts, resolutions and their reuse; `propose`, `line create`, `line merge`, `resolve` (with `--dry-run`); merge base and the criss-cross refusal | library + cli | Each merge case, mode conflicts included; reuse only for the same key; delete against change; `resolve --file` refusing a PEM header, a `secret_key` file, a deny-listed path and a file outside the checkout |
| 7 | Named checks; the check runner with the clean environment and the sandbox; `validate` and `integrate --run`, with `scripts/weave` running `validate` only for an identity in the line's `validators`; counted results with the epoch and the latest-result rule; qualification rules 5 and 6; dispositions; reviews of decisions, proposals and manifests; goals; current policy and green; `green`, `review` | library + cli | Each disposition against each basis; a basis classified under the policy in force, unchanged by a later policy (`b2098df8e4be`'s counterexample); objections to a manifest and a proposal binding a decision that uses them; a review carried over by a rebase with the same change, and not by one without; root and cross-line decisions held to every guardrail; green on fresh results after a policy change, old passes and fails ignored (`98dd56b8170a`'s counterexample); a validator's re-run replacing its fail, and concurrent latest results counting as a fail; the integrator's results not counting; a result for another line's check of the same name and a different argv neither counts nor replaces a fail (`4189811f3cdd`'s counterexample); every `--again` attempt shown by `green`; a proposal's objection binding an `integrate` of a given tree with the same change; on both `validate` and `integrate --run`, a check sees no `BRACKEN_*` and a fixed `PATH`, can't read an `identities/` directory, `~/.ssh` or `$TMPDIR`'s other entries, can't write `~/.cargo` or `~/.rustup`, has no network, loopback or IPC, and is killed with its process group at the timeout; a missing vendored crate fails the check; no run at all when the sandbox can't be applied; an output tail holding a key withheld as its size and hash |
| 8 | Format fixture, and the coordination-Ground fixture test with Weave linked in | protocol (review) | The fixture opens; every coordination status is unchanged |
| 9 | Dogfood, stage 1 (shadow): after each merge to `main`, the manager runs `weave integrate --from-git` on the merged commit and records `weave verify` | process | Every decision verifies equal to its git commit |
| 10a | Dogfood, stage 2a (owner approval `535f098d7cfa`, design `c4fe14c88fe1`): after each push, builders propose the pushed commit with `scripts/weave-propose` (`weave propose --from-git`, based on the decision mirroring the branch's merge-base). After git merges the PR, `weave-integrator` merges its proposal with `weave integrate --proposals P --mirrors <commit>` (a batch merge: one proposal per PR, in squash order, into one decision), written only if it equals git's tree; a per-file conflict, a missing proposal, or (after a stop) a mismatch falls back to importing the commit, noted. No checks run in Weave | process | Weave's head stays equal to git's `main`; the shadow counts how often Weave's own merge was used |
| 10b | Dogfood, stage 2b (its own go): `weave-runner` runs the test table's checks once per candidate with `weave validate`, replacing a run | process | Weave's head stays equal to git's `main` |

Stage 1 starts with the owner's go to create the Weave Ground (it is new
data on the machine, though not a new service). Later stages, and the join,
each need their own decision.

## 7. What stays git's job until Weave proves itself

- **The repository of record.** `main` on GitHub, PRs, reviews and the
  merge gate, through `scripts/merge` and the counted test-table run.
- **History before the dogfood starts**, and blame. Weave starts from an
  imported tree.
- **The coordination Ground's own storage.** `.bracken/` stays committed to
  git by `scripts/ground-checkpoint`, and Weave never imports it.
- **What Weave doesn't model in v0.**
  - Empty directories and submodules.
  - Files over the size limit, and files the deny-list refuses.
  - Symlinks whose target needs `..`, and paths that collide when folded.
  - Git attributes and hooks.
- **Releases and tags**, which are already the owner's go.
- **CI and the test table.** In stage 2 they run as Weave checks too. From
  stage 2b step 1b (plan `cc496dcee0e3`, owner approval `e8fd45c83ba6`,
  decision `bbbf898cbcfe`), line main's results can gate a merge in place
  of git's counted run, but only while the Weave gate is on. The gate is on
  only while exactly one current manager claim tagged `weave-gate` has
  `Weave gate: on` as its whole first line; with none, any other, or
  several, it's off, and git's
  run gates as before.
  - `scripts/verify-seq --weave` takes git's merge M of main with the head,
    as `scripts/merge` computes it, and imports it as manifest H. It
    checks that `weave verify --git M --manifest H` says equal. weave-runner
    validates the checks H has no result for. If `weave green --manifest H`
    shows fmt, clippy, table and mac all green, it records one manager
    evidence on the pr claim, citing H and the result ids.
  - `scripts/merge` counts that as a run of the whole table on main merged
    with the head only if `scripts/weave-gate check` agrees, reading
    everything afresh: the switch, git's merge and its key, `weave verify`,
    and `weave green --manifest`, whose result ids must be the ones cited.
  - Git's run is still needed for:
    - a head that doesn't merge cleanly;
    - a pr claim that names no Weave proposal (`weave-shadow --proposal`
      checks Weave's own merge against git's after the merge);
    - a head that changes `scripts/test-table`, its template copy, or
      `Cargo.lock` (the policy pins main's test-table, and the vendor
      directory follows main's lock);
    - a check that isn't green;
    - batches.
  - Results are read only through `weave green --manifest`: each check's
    counted results on any manifest the replica holds, under the line's
    current policy, leaving out its integrators' own.

## 8. Later

Not needed before the build; each gets its own decision when it comes up
(retrospective `16cbeff051ea`, and section 4):
- **A backup of the Weave Ground, before stage 2.** Stage 1 can be rebuilt
  from git, stage 2's proposals and evidence can't. `scripts/weave-backup`
  is ready to run by hand (section 5). Its destination and a schedule wait
  for the owner's go.
- **Retract means "wrong", never "done".** Objections close only through
  answers, and questions through a lifecycle state rather than retraction.
- **Lazy payload loading and a persistent hash index**, if step 5's
  measurement shows opening is too slow. Both change the storage format.
- **Delegation and per-agent keys**, which turn the author conventions into
  guarantees, including a runner key only `weave-runner` holds.
