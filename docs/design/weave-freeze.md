# Freezing Weave's formats

Design only: it changes no format, code, test or fixture (task
`d8cc249f0c92`, decision `285f7e43f540` (a), step 3 of the Weave order in
decision `b4a3456de597`). Nothing is frozen until the owner's go. It pins:
1. what the freeze covers, by file, and what it leaves out;
2. how a change would be caught today, and where nothing would catch it;
3. what a frozen-format change takes once the freeze holds;
4. how the freeze relates to the join and to agent-keys.md 4.10;
5. the owner's questions.

It builds on [weave.md](weave.md): section 1 for the records, section 5 for
how formats are pinned and how the join works. Where this page and
weave.md differ, the code at `main` 4604d400 is what was checked, and the
difference is listed in 2.3. File references are to that commit.

## 0. What "frozen" means

The records in an existing Weave Ground can never be rewritten. Re-encoding
a contribution changes its id, which needs its author's key, and it breaks
every reference to it ([releases.md](../releases.md), *Protocol and storage
migrations*). So a format, once records in it exist, has to stay readable
by every later binary. The freeze makes that a rule, not a habit, with
three tiers:

- **Frozen.** A record of this format, as written today, reads the same
  way forever. Its bytes, its content hash and its validity never change in
  place. Changing it means a new format version beside it (section 3), and
  every later binary still reads version 1.
- **Pinned, not frozen.** The rules that compute views from records, and
  the line merge. Existing tests pin them, and a change to them is protocol
  class, as today. It can change what a Ground's views show, never which
  records are valid.
- **Not frozen.** Everything else: CLI output, scripts and their text
  conventions, how derived state is stored or computed, import's
  exclusions.

## 1. What the freeze covers

### 1.1 Encoding, versions and media types

| Item | Rule | Where |
|---|---|---|
| Canonical CBOR | A payload is valid only if decoding then re-encoding it gives the same bytes. That means maps with text keys in the listed field order, integers in their shortest form, byte strings as major type 2, no trailing bytes, no unknown field and no missing nullable field. | `crates/tree/src/format.rs` `to_cbor`, `from_cbor` (73-87), `Record::decode` (114-118); the encoder is ciborium, as pinned in `Cargo.lock` |
| Format version | Every CBOR payload starts with `"v": 1` (`VERSION`). A reader refuses any other value. | `format.rs` 24, `Record::validate` (102-107), `Manifest::decode_with` (241-248) |
| Media types | The seven media types: `application/vnd.bracken.weave.file`, `.manifest+cbor`, `.ground+cbor` (the marker), `.line+cbor`, `.decision+cbor`, `.proposal+cbor` and `.resolution+cbor` | `format.rs` 26-32 |
| Media-type prefix | `application/vnd.bracken.weave`. Every Weave record's media type starts with it, and `scripts/bk`, `scripts/weave` and `scripts/ground-guards` match on it, ignoring case. | `format.rs` 35; `scripts/bk` 61, `scripts/weave` 74, `scripts/ground-guards` 142 |
| Artifact names | `manifest`, `weave`, `decision`, `proposal`, `resolution`; a line artifact carries its policy's `name`, and a file version the path it was first imported at (advisory, never blank) | `format.rs` `artifact_name` of each record; `crates/tree/src/import.rs` 86-95 |
| Goal tag | `weave-goal` | `format.rs` 38 |
| Record kinds | Every Weave record is an existing contribution kind (weave.md section 1, *The records*): artifacts, a goal claim, evidence, and `refines`, `supports`, `contradicts` and `supersedes` relations. manyroot's own freeze covers those kinds. | `docs/protocol.md`; `crates/manyroot` |

### 1.2 Content hashes and file values

| Item | Rule | Where |
|---|---|---|
| Content hash | `BLAKE3(payload bytes)`, 32 bytes, independent of the Ground | `crates/tree/src/hash.rs` `ContentHash::of` |
| Hash in payloads | A 32-byte CBOR byte string, with no label | `hash.rs` `Serialize` and `Deserialize` (non-human-readable branch) |
| Hash in text | `blake3:` followed by 64 lowercase hex digits. A bare 64 hex digits is refused, since it's a ContributionId. | `hash.rs` `PREFIX`, `FromStr`, `Display` |
| Modes | `0o100644`, `0o100755` and `0o120000`, encoded as a CBOR unsigned integer. Any other value is refused. | `format.rs` `Mode` (139-178) |
| File value | `[hash, mode]`. Values sort by hash bytes, then mode. | `format.rs` `FileValue` (184-200) |
| File version payload | The file's raw bytes. It has no CBOR and no `v`. | weave.md section 1; `format.rs` `FILE_MEDIA_TYPE` |

### 1.3 Each record's payload

Field names, their order and their types are all frozen, along with each
record's own validity rule.

| Record | Payload, in field order | Its rules | Where |
|---|---|---|---|
| Manifest | `{v, entries: [[path, mode, hash, size]]}` | Entries strictly ascending by path bytes; every path valid (1.4); no collision after folding | `format.rs` `Entry` (204-228), `Manifest` (233-290) |
| Marker | `{v, purpose: text, in_place: ContributionId \| null}` | none beyond encoding | `format.rs` `Marker` (295-315) |
| Line (a policy version) | `{v, name, integrators, validators, goal_setters, checks: {name: [text]}, executors?, guardrails: {max_known, max_age, protected}, owner_decision: text \| null}`. `executors` is omitted when it's empty. | Name not blank; check names not blank; argv not empty; each executor names one of the checks, and its `script` is a valid path; each `protected` prefix is a valid path; `owner_decision` not blank if present | `format.rs` `Guardrails`, `Line` (318-399) |
| Executor | `{kind: "box" \| "mac", script: path, script_hash: hash}` | as for Line | `format.rs` `ExecutorKind`, `Executor` (626-651) |
| Decision | `{v, line, manifest, parents, proposals, breakage: [{check, detail, paths}], answers: [{objection, disposition, reason}], resolutions, goals, source: {git} \| null, note}` | Parents strictly ascending; at most `MAX_REFERENCES` (1,024) references in all; a breakage `check` not blank and its `paths` valid; an answer's `reason` not blank. A disposition is one of `fixed`, `fell_back`, `accepted_breakage` or `overridden`. | `format.rs` 44, `Breakage` to `Decision` (402-495) |
| Proposal | `{v, line, base, manifest, change, summary}` | `change` must equal the recomputed change (1.5); views check this | `format.rs` `Proposal` (501-524); weave.md section 1, *Proposals and resolutions* |
| Resolution | `{v, path, base: value \| null, sides: [value \| null, value \| null], result: value \| null}` | Path valid; sides strictly ascending, null first | `format.rs` `Resolution` (532-568) |

The policy rule about a null `owner_decision` is frozen too: only the first
`main` version written by the counted marker's author may carry null, and
views ignore every other version that does (`crates/tree/src/lines.rs`
293; weave.md section 1, *Lines*).

### 1.4 Paths, folding, collisions and symlink targets

| Item | Rule | Where |
|---|---|---|
| Path | UTF-8 with `/` separators; not empty; at most 4,096 bytes and 255 per component, checked before folding; no leading or trailing `/`; no `.`, `..` or NUL; no `:` or `\`; no blank component; nothing that folds to `.git` or starts with `.bracken`; git's `protectHFS` and `protectNTFS` component checks | `crates/tree/src/path.rs` `MAX_PATH_BYTES` (15), `MAX_COMPONENT_BYTES` (18), `validate_path` (53), `validate_component` (84) |
| Folding | Drop default-ignorable code points, then `NFD(casefold(NFD(s)))` with full case folding, from Unicode 18.0's tables | `path.rs` `fold` (128), `is_default_ignorable` (135); the `unicode-normalization` and `icu_casemap` versions in `Cargo.lock` |
| Collisions | No two paths are equal once folded, and no folded path is a directory prefix of another | `path.rs` `find_collision` (165), `PathCache` |
| Symlink target | Relative, UTF-8, not empty, no NUL; every component passes the component rules, so it never contains `.` or `..` | `path.rs` `validate_symlink_target` (146) |

### 1.5 A proposal's change, check definitions and results

| Item | Rule | Where |
|---|---|---|
| Change | The paths whose file values differ between the base's manifest and the proposed one, ascending by path bytes, each as `[path, before \| null, after \| null]`. `change` is the content hash of that list's canonical CBOR. | `format.rs` `ChangeEntry`, `Change` (572-621) |
| Definition hash | Content hash of the canonical CBOR of `[name, argv]`, or of `[name, argv, executor]` for a check that has an executor | `format.rs` `CheckDefinition::hash` (664-669) |
| Method | A result's `method` is exactly `<name> blake3:<definition hash>`. The name may contain spaces; the method is split at its last space. | `format.rs` `method` (673-675), `parse_method` (679-685) |
| Result `details`, the counted part | The first line is the manifest's content hash in text form, and the second is `attempt <n>`. Counting reads both. | `crates/tree/src/results.rs` `details` (59-67), `details_with_run` (72-88), `parse_details` (92-97); `crates/tree/src/ground.rs` 305 |
| A result's evidence | Its target is a manifest artifact, its verdict is `pass` or `fail`, and its method is as above | `ground.rs` 295-307 |

### 1.6 Pinned, not frozen

These decide what a Ground's views show. Tests pin them, and changing them
is protocol class, but they don't decide which records are valid, so they
can change without a new format version:

- **The derivation rules** in weave.md section 2: the ancestor rule as
  views apply it, the marker that counts, the default line, the policy in
  force and the current policy, qualification rules 1 to 6, heads, forks
  and merge bases, counted results and the latest-result rule, green,
  reviews and goals, and open objections. That's `crates/tree/src/`
  `ancestry.rs`, `lines.rs`, `results.rs`, `reviews.rs` and
  `objections.rs`. `crates/tree/tests/weave_fixture.rs`'s
  `every_derived_status_is_as_pinned` pins their results on the fixture.
  Per-agent keys (part B) would change some of them (agent-keys.md 4.10),
  which is one reason they stay outside the freeze.
- **The line merge**: imara-diff's histogram diff and the 10,000-line cap,
  in `crates/tree/src/text_merge.rs`. `format_guard.rs`'s line-merge vectors,
  its cap vectors and `KNOWN_DIFFERENT` pin it. Views never recompute a
  merge (releases.md, 2026-10-31), so changing it changes future merges,
  never existing records.

### 1.7 Not frozen

- **CLI output**, text and `--json`: `crates/bracken/src/weave*`. It's
  covered by the CLI's own tests, and AGENTS.md keeps a `--json` form for
  every read command.
- **How derived state is stored and computed**: indexes, answer vectors,
  caches, laziness (weave.md section 4). A persistent index would be the
  storage class, not this freeze.
- **Scripts and their text conventions**: `scripts/weave*`. That includes
  a proposal's `summary` (`PR #<n> <branch> @ <sha>`, which
  `scripts/weave-shadow` parses), a decision's `note` (`shadow of <sha>: …`,
  which the shadow's count reads) and pr claims' `Weave proposal <id>`. The
  freeze keeps `summary` and `note` as free text; what goes in them is the
  scripts' business, tested in `crates/bracken/tests/scripts/weave.rs`.
- **Everything after a result's second details line**: the argv, the exit,
  an outside run's lines (`commit`, `tree`, `bundle`, `box`, `cost`, `end`,
  `output`, `log <id> <hash>` and `log withheld: …`) and the tail. These are
  informational. The one reader past line 2 is `box_logs`
  (`crates/bracken/src/weave/checks.rs`), which looks for the `log` line;
  if a change hides it, the mac check fails closed (`scripts/test-table`
  866: "no output of box check").
- **The runner's log artifact**: media type `text/plain`, named
  `<check> output on <hash>`, holding raw output
  (`crates/bracken/src/weave/checks.rs` 92, 555). It means nothing to Weave
  beyond its bytes, and the result's `log` line pins those by hash.
- **What import takes**: the exclusions, the deny-list and the key scan
  (`crates/tree/src/import.rs`, `secrets.rs`). They decide what gets
  written, never how a written record reads. Loosening them is a security
  review.
- **The check runner and its sandbox** (`crates/tree/src/check*`), except
  what it writes into a result's method and first two details lines.

## 2. How a change would be caught

### 2.1 The guards that exist

| Guard | What it catches | How it fails |
|---|---|---|
| `crates/tree/tests/format_guard.rs` `golden_vectors` | One record of each media type, by bytes and content hash: a file, a manifest with all three modes, a marker with `in_place` null, a line without executors, a decision, a proposal and a resolution | `a Weave payload format changed: this is protocol class`, with the new vectors printed |
| `format_guard.rs`, the rule tests | Round trips; key order, an unknown field, a missing field or a long integer refused as not canonical; only `v` 1 read; unknown modes refused; manifest, line, decision and resolution rules; `MAX_REFERENCES`; change rows; a definition hash changing with argv and executor; `parse_method` | An assertion naming the rule |
| `format_guard.rs` `unicode_tables_are_pinned` | Unicode's version, and a digest of `fold` over every scalar value, which covers icu_casemap's data and the ignorables | Version mismatch, or the digest |
| `format_guard.rs`, the line-merge tests | The line merge (1.6): vectors, the cap, and the cases where git differs | `vector <i>`, `<n> lines`, or the `KNOWN_DIFFERENT` list |
| `crates/tree/tests/paths.rs` | Every path, folding, collision and symlink-target rule, both ways; the length limits relative to their constants | An assertion naming the rule |
| `crates/tree/tests/weave_fixture.rs` with `tests/fixtures/weave-v1` | A Ground written by `bracken-tree` 0.1.0: 80 contributions with pinned ids, covering every media type, results, reviews, a goal and a policy change. It checks that it opens, that every record reads back to its bytes, every derived status, and that the scenario rebuilds the fixture byte for byte. Since the ids are pinned, so is every byte the scenario writes: `details` from `details()`, argv-only methods, change hashes, `Exit::Code`'s text. | An id, byte or status mismatch, or the rebuild differing |
| `crates/tree/tests/coordination_fixture.rs` and `crates/bracken/tests/coordination_fixture.rs` | The coordination fixture `ground-v1` reads as recorded with Weave linked in, and Weave finds nothing of its own there | A status or acceptance mismatch |
| `scripts/code-scope full` | A head touching `crates/tree/src/*`, `crates/tree/Cargo.toml`, `weave_fixture.rs`, `tests/fixtures/*`, `Cargo.toml` or `Cargo.lock` gets the full table, so every guard above runs | `scripts/verify-seq --light` and `scripts/merge` refuse a light run |
| AGENTS.md's protocol class; releases.md's release path | A protocol-class change needs an independent review, the manager's go logged as `owner-review`, and a migration note | Review, not a script: nothing refuses the merge |

### 2.2 Each frozen item, and its guard

| Frozen item (section 1) | Caught by | Gap |
|---|---|---|
| Canonical CBOR, `v`, unknown fields | golden vectors; `non_canonical_payloads_are_refused`; `only_version_1_is_read`; the fixture | none |
| The seven media types and the prefix | golden vectors (media type strings), `artifact_names_and_media_types`; the scripts' tests for the prefix | none |
| Artifact names, goal tag | `artifact_names_and_media_types`; the fixture's ids | none |
| Content hash, the `blake3:` text form | golden vectors (hashes); `hash.rs` unit tests | G5: the unit tests live in `src/`, so they're covered by `full`, but no golden vector pins a text-form parse |
| Modes, file values | golden manifest (all three modes) and resolution; `unknown_modes_are_refused` | none |
| Manifest | golden vector; `manifest_rules`; the fixture | none |
| Marker | golden vector with `in_place` null; the fixture | **G1**: nothing pins a marker with `in_place` set by its bytes, and that's the record the join writes |
| Line without executors | golden vector; `line_rules`; the fixture | none |
| Line with executors, Executor | `executors_are_left_out_when_there_are_none`, round trips, `line_rules` | **G1**: no golden bytes for a line with an executor, of either kind; the fixture predates executors |
| `owner_decision` and its null rule | golden line; `line_rules`; the fixture's derived statuses | none |
| Decision | golden vector; `decision_rules`; `a_decision_refers_to_at_most_max_references`; the fixture, which holds decisions with `source` null and set | none |
| Proposal and its change | golden proposal (its `change` field is an arbitrary hash); change rows by value; the fixture's proposals, whose ids pin their recomputed `change` | **G2**: no golden vector pins a change list's hash directly |
| Resolution | golden vector; `resolution_rules`; the fixture | none |
| Path rules, folding, collisions, targets | `paths.rs`; the Unicode digest; `manifest_rules`; the deep-path test | **G3**: `MAX_PATH_BYTES`, `MAX_COMPONENT_BYTES`, `MAX_REFERENCES` and `VERSION` are tested only through their constants, so changing a constant's value passes every test unless it's off by a lot |
| Definition hash, method | relative assertions in `format_guard.rs`; the fixture pins argv-only methods through its evidence ids | **G2**: no golden value for a definition hash with an executor, of either kind |
| Details, the counted part | the fixture (`details()` with `Exit::Code` and a text tail) | **G4**: nothing pins `details_with_run`'s first two lines, or `details` with a withheld tail or another `Exit` variant |
| Any of the above, as a review requirement | AGENTS.md and releases.md, read by people | **G6**: nothing mechanical requires protocol-class review for a Weave format change. `scripts/merge` never asks for `owner-review`; `code-scope full` doesn't list `format_guard.rs` or `paths.rs`; AGENTS.md's protocol bullet names only manyroot's files and `docs/protocol.md` |

### 2.3 Docs that disagree with the code

- weave.md section 1, *Lines*: the policy block lacks `executors`, which the
  code has (`format.rs` 339-340) and releases.md describes (2026-10-31).
  This page's 1.3 follows the code.
- weave.md section 5 says changing a pinned format takes an independent
  review and the manager's go. Section 3 here adds the rollout and the
  owner's question 3.

### 2.4 Proposed guards: later build tasks, not built here

Each would be its own small PR. G1 to G4 change only
`crates/tree/tests/format_guard.rs`, which is library class, needs the
full table, and adds pins without changing any format.

- **G1. Golden vectors for the rest of the line and marker shapes**: a
  line with a `box` executor and one with a `mac` executor, and a marker
  with `in_place` set, each pinned by bytes and content hash.
- **G2. Golden values for derived hashes**: a definition hash for an
  argv-only check, a `box` check and a `mac` check, with their method
  strings; and a change list covering an added, a deleted, a changed and a
  mode-only row, pinned by hash.
- **G3. Pin the constants by value**: `VERSION` 1, `MAX_REFERENCES` 1,024,
  `MAX_PATH_BYTES` 4,096, `MAX_COMPONENT_BYTES` 255, the prefix and the
  goal tag, asserted as literals in `format_guard.rs`.
- **G4. Golden details**: `details` with each `Exit` variant and a withheld
  tail, and `details_with_run`, pinned as text; and `parse_details` on each,
  reading the same manifest and attempt.
- **G5. Content-hash text form**: in `format_guard.rs`, a golden parse of
  `blake3:<hex>`, with refusals of the bare, uppercase and short forms. It
  repeats `hash.rs`'s unit tests in the file the freeze names.
- **G6. Make the review requirement mechanical** (merge-gate tooling):
  - a `scripts/code-scope frozen <base> <head>` mode listing the frozen
    files: `crates/tree/src/{format,hash,path}.rs`, `results.rs`'s `details`
    functions (the whole file, since a script can't see functions),
    `crates/tree/tests/{format_guard,paths,weave_fixture}.rs`,
    `crates/tree/tests/fixtures/`, and the `unicode-normalization`,
    `icu_casemap`, `ciborium` and `blake3` lines of `Cargo.lock`;
  - `scripts/merge` refusing a head that touches them unless a current
    `owner-review` claim (or, under question 3, an owner approval) names
    its pr claim;
  - `format_guard.rs` and `paths.rs` added to `code-scope full`'s list;
  - AGENTS.md's protocol bullet naming Weave's frozen files and this page.
- **G7. Extend the fixture** by appending to the end of the
  `write_weave_v1_fixture` scenario, so no existing id changes (releases.md,
  2026-10-31): a policy version with executors, and a result written
  through `details_with_run`. This needs the same review as any fixture
  change.

## 3. Changing a frozen format after the freeze

- **Class**: protocol. That means an independent review and a security
  review, the full table, and a migration entry in
  [releases.md](../releases.md). Whose go it takes is question 3.
- **Never in place.** A v1 record keeps its bytes and meaning. A change
  that would make an existing record read differently, or stop it reading,
  isn't allowed. That rules out changing a field's type or order,
  tightening a rule, or loosening one: under a looser rule an old binary
  would reject records a new one accepts, and the two would see different
  heads.
- **A new version.** A changed record carries `"v": 2` under the same
  media type and prefix (question 2). A binary that reads only v1 refuses
  it as `payload has format version 2; this build reads version 1`. Its
  views then ignore the record, and `weave check` lists it. The Ground
  still opens and syncs, since manyroot sees an artifact.
- **Readers first, then writers**, as releases.md already requires for
  executors (2026-10-31). An old binary that ignores a new decision or
  policy version computes different heads, or reads a line under a policy
  that's no longer in force. So:
  1. a release that reads both v1 and v2 and writes only v1;
  2. every binary that reads a Weave Ground upgraded to it: the pinned
     `.bracken-bin`, `~/bracken-weave`'s, and after the join, the
     coordination Ground's readers;
  3. only then, a release that writes v2;
  4. never a rollback below step 1's release.
- **Grounds already written** aren't migrated. Every later binary keeps
  reading v1: `weave-v1` stays checked in with its test, and a
  `weave-v2` fixture is added beside it. A writer may stop writing v1;
  no reader may stop reading it.
- **An added optional field** is still a new version. The executors field
  went in as an optional field under v1, encoded only when present. A
  binary from before it rejects a line that has one as not canonical,
  which is the right effect but a confusing message. After the freeze,
  such a field bumps `v`, so the refusal names the version.
- **Pinned-not-frozen changes** (1.6) stay protocol class with a
  releases.md entry, as today, but need no new version: they change views,
  not records.

## 4. The join, and agent-keys.md 4.10

- **The freeze comes before the join.** weave.md section 5 makes frozen
  formats a condition of the join, because the join puts Weave records into
  the coordination Ground. From then on, a Weave format change affects
  every reader of that Ground: `scripts/bk`, `scripts/merge`, the pinned
  binary and the coordination fixture tests.
- **The join's own record is frozen too.** Step 3 of the join writes a
  marker with `in_place` set. Its format is frozen (1.3), but no golden
  vector pins it yet, so G1 should land before the join.
- **The join relies on two frozen things.**
  - Content hashes don't depend on the Ground, and manifests encode the
    same in either Ground, so step 4's manifest hash can equal the dogfood
    decision's.
  - The prefix is frozen, so `ground-guards`' tripwire and `scripts/bk`'s
    refusal keep matching every Weave record until the join PR lifts them
    on purpose.

  Import's exclusions aren't frozen (1.7). If they change between the
  dogfood decision and the cutover, step 4's hashes differ and the cutover
  stops visibly, instead of recording the wrong tree.
- **agent-keys.md 4.10.** The freeze depends on neither part A nor part B.
  A line policy names keys as 32-byte AgentIds, and that stays frozen.
  Rotating the runner or the integrator is a new policy version under an
  owner decision, in the same format. Part A's server-held keys sign the
  same records. Part B's delegation, revocation and ruling records would be
  new artifacts with formats of their own, pinned like Weave's (agent-keys.md
  4.8). They don't change the frozen formats. Part B would change who counts
  as a validator or integrator, which is a derivation rule. That's why the
  rules are pinned, not frozen (1.6): part B can change them with a
  protocol-class review and no new format version. `owner_decision` stays
  text naming a coordination claim, which `scripts/weave` checks (4.10's
  last point).

## 5. Questions for the owner

1. **What the freeze covers.** Freeze section 1 as version 1, keep the
   derivation rules and the line merge pinned but not frozen (1.6), and
   leave 1.7 out? *Recommended: yes.* Part B, and later fixes to Weave's
   views, need the rules to stay changeable under review. The records are
   what can never be rewritten.
2. **How a format changes.** A new `v` under the same media type, rather
   than a new media type? *Recommended: a new `v`.* Every decoder already
   refuses an unknown `v` with a message that says to upgrade. Keeping the
   media types keeps the prefix checks and the scripts unchanged, and a
   media type that encodes a version would duplicate `v`.
3. **Whose go a frozen-format change needs.** Protocol-class merges are
   delegated to the manager today, logged as `owner-review` (decision
   `adcb78b88c6c`). Keep that for frozen formats, or require your own go?
   *Recommended: your own go until the join, then review the rule again.*
   A frozen-format change is rare, can't be undone once records exist, and
   at the join it reaches the coordination Ground. Changes to the
   pinned-not-frozen rules stay delegated.
4. **Close the guard gaps before the freeze?** G1 to G6 are small build
   tasks; G7 needs a fixture change. *Recommended: land G1 to G6 first, then
   your go to freeze; G7 can follow.* The freeze would rest on pins that
   don't fully exist yet otherwise, notably the join's marker (G1) and the
   review rule (G6).
5. **Pin the Unicode and encoding crates exactly** (`=` versions in
   `Cargo.toml` for `unicode-normalization`, `icu_casemap`, `ciborium` and
   `blake3`)? *Recommended: yes, as part of G6.* The digest and the golden
   vectors already fail on a change. An exact pin stops `cargo update`
   pulling one in, so a routine dependency bump doesn't turn into a
   protocol-class PR.
6. **Timing against stage 2b.** Freeze independently of the 20 Weave-gated
   merges (decision `b4a3456de597` step 1)? *Recommended: yes.* The freeze
   changes no format and blocks no merge. The count is about whether Weave's
   merges are trustworthy, not about its formats. The join still waits for
   both, and for its own go.
