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
four tiers:

- **Frozen.** A record of this format, as written today, reads the same
  way forever. Its bytes, its content hash and its validity never change in
  place. Changing it means a new format version beside it (section 3), and
  every later binary still reads version 1.
- **Pinned, not frozen.** The rules that compute views from valid
  records, and the line merge. Existing tests pin them, and a change to
  them is protocol class, as today. It can change what a Ground's views
  show, never which records are valid. Every rule that decides validity,
  the ancestor rule included, is frozen instead (1.3).
- **Refusals at write and use, changeable as security-sensitive.** What
  import refuses to write, what `materialize` refuses to put on disk, and
  what the runner refuses to publish (1.8). These may tighten when a new
  hazard is found. They never change which records are valid or what a view
  shows. A valid record that a refusal stops stays valid, and the refusal
  is reported.
- **Not frozen.** Everything else: CLI output, scripts and their text
  conventions, how derived state is stored or computed.

## 1. What the freeze covers

### 1.1 Encoding, versions and media types

| Item | Rule | Where |
|---|---|---|
| Canonical CBOR | A payload is valid only if decoding then re-encoding it gives the same bytes. That means maps with text keys in the listed field order, integers in their shortest form, byte strings as major type 2, no trailing bytes, no unknown field and no missing nullable field. | `crates/tree/src/format.rs` `to_cbor`, `from_cbor` (73-87), `Record::decode` (114-118); the encoder is ciborium, as pinned in `Cargo.lock` |
| Format version | Every CBOR payload starts with `"v": 1` (`VERSION`). A reader refuses any other value, as section 3 describes. | `format.rs` 24, `Record::validate` (102-107), `Manifest::decode_with` (241-248) |
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
284-296; weave.md section 1, *Lines*). It's the one frozen rule that
depends on who signed, and on which marker counts, so both are frozen
with it:
- **the marker that counts**: the valid, non-retracted marker with no other
  such marker among its causal ancestors, the smallest id among concurrent
  ones (`ground.rs` 330; weave.md section 1, *Lines*, decision
  `049d02449a94`);
- **its author**: the AgentId that signed the marker. A change to whose
  signature stands for that author, such as a delegate's under part B, is
  a change to this rule (section 4).

**The ancestor rule is frozen too**, since it's a validity rule: Weave's
replacement for protocol rule 5 for references inside payloads (weave.md
section 1, *Why existing kinds*). What's frozen is which payload fields are
references and that each must be a causal ancestor of the record that holds
it. The references are a marker's `in_place`; a decision's `line`,
`parents`, `proposals`, its answers' `objection`s, `resolutions` and
`goals`; and a proposal's `line` and `base`. A record that breaks the rule
is invalid: `Problem::NotAnAncestor`, ignored by every view and listed by
`weave check` (`crates/tree/src/ground.rs` `references` 165-192, 309-317).
How the question is answered (`ancestry.rs`'s index and passes) isn't
frozen. Only the answer is.

### 1.4 Paths, folding and collisions

These decide whether a manifest, a resolution or a policy's path is valid.
Refusing to write a valid path to some file system is a use-site rule
(1.8), not a change to these.

| Item | Rule | Where |
|---|---|---|
| Path | UTF-8 with `/` separators; not empty; at most 4,096 bytes and 255 per component, checked before folding; no leading or trailing `/`; no `.`, `..` or NUL; no `:` or `\`; no blank component; nothing that folds to `.git` or starts with `.bracken`; git's `protectHFS` and `protectNTFS` component checks | `crates/tree/src/path.rs` `MAX_PATH_BYTES` (15), `MAX_COMPONENT_BYTES` (18), `validate_path` (53), `validate_component` (84) |
| Folding | Drop default-ignorable code points, then `NFD(casefold(NFD(s)))` with full case folding, from Unicode 18.0's tables | `path.rs` `fold` (128), `is_default_ignorable` (135); the `unicode-normalization` and `icu_casemap` versions in `Cargo.lock` |
| Collisions | No two paths are equal once folded, and no folded path is a directory prefix of another | `path.rs` `find_collision` (165), `PathCache` |

### 1.5 A proposal's change, check definitions and results

| Item | Rule | Where |
|---|---|---|
| Change | The paths whose file values differ between the base's manifest and the proposed one, ascending by path bytes, each as `[path, before \| null, after \| null]`. `change` is the content hash of that list's canonical CBOR. | `format.rs` `ChangeEntry`, `Change` (572-621) |
| Definition hash | Content hash of the canonical CBOR of `[name, argv]`, or of `[name, argv, executor]` for a check that has an executor | `format.rs` `CheckDefinition::hash` (664-669) |
| Method | A result's `method` is exactly `<name> blake3:<definition hash>`. The name may contain spaces; the method is split at its last space. | `format.rs` `method` (673-675), `parse_method` (679-685) |
| Result `details`, the counted part | The first line is the manifest's content hash in text form, and the second is `attempt <n>`. Counting reads both. | `crates/tree/src/results.rs` `details` (59-67), `details_with_run` (72-88), `parse_details` (92-97); `crates/tree/src/ground.rs` 305 |
| A result's evidence | Its target is a manifest artifact, its verdict is `pass` or `fail`, and its method is as above | `ground.rs` 295-307 |

### 1.6 Pinned, not frozen

These decide what a Ground's views show from its valid records. Tests pin
them, and changing them is protocol class, but they don't decide which
records are valid, so they can change without a new format version. A
change still needs every reader upgraded before it's relied on, since two
binaries on different rules show different views (section 3):

- **The derivation rules** in weave.md section 2: the default line, the policy in
  force and the current policy, qualification rules 1 to 6, heads, forks
  and merge bases, counted results and the latest-result rule, green,
  reviews and goals, and open objections. That's `crates/tree/src/`
  `lines.rs`, `results.rs`, `reviews.rs` and `objections.rs`, apart from
  the rules 1.3 freezes (the null `owner_decision` rule, the marker that
  counts and the ancestor rule), which live in the same files. `crates/tree/tests/weave_fixture.rs`'s
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
  informational as a format. The one reader past line 2 is `box_logs`
  (`crates/bracken/src/weave/checks.rs`), which looks for the `log` line;
  if a change hides it, the mac check fails closed (`scripts/test-table`
  867: "no output of box check"). Whether output is withheld is a
  different matter, under 1.8.
- **The check runner and its sandbox** (`crates/tree/src/check*`), except
  what it writes into a result's method and first two details lines, and
  the output rules in 1.8.

### 1.8 Refusals at write and use

These are what keeps a secret out of a Ground and a hostile path off a
disk. They may tighten whenever a new hazard is found, and must, without
waiting for a format version. They never change which records are valid
or what a view shows. A record they stop stays valid, and the refusal names
it.

| Refusal | Where | When it runs |
|---|---|---|
| Import's exclusions, deny-list and key scan, and its symlink-target check (relative, UTF-8, not empty, no NUL, every component passing 1.4's component rules) | `crates/tree/src/import.rs` (338 for targets), `secrets.rs` | before a file version is written |
| `materialize`'s checks: the manifest's validity, then each symlink's target, as at import | `crates/tree/src/materialize.rs` 66-83 | before anything is written to disk, by `weave checkout`, the sandboxed runner (`check.rs` 319) and the outside runner (`check/outside.rs` 197) |
| The key set the scans match: every known identity's key, in hex and raw, which no PEM rule catches (`SecretRule::KnownIdentityKey`) | `crates/bracken/src/weave.rs` `known_keys` (993), for import and `resolve --file`; the runner builds its own set from the same identity directories (`crates/bracken/src/weave/checks.rs` 143-148) | before import's and `resolve --file`'s scans, and before the runner's output rules |
| `weave resolve --file`'s own refusals: the file must be inside the checkout, its path not deny-listed, its bytes clean by the scan | `crates/bracken/src/weave/merge.rs` `resolution_file` and `checkout_path` (1403-1446) | before a resolution's file version is written |
| The runner's output rules: `recorded_tail` and the key rules it matches, `LOG_CAP`, the `log withheld: …` and `output withheld: …` branches, and whether the log artifact is written | `crates/tree/src/check.rs` 973-985; `crates/bracken/src/weave/checks.rs` 541-560; `crates/tree/src/results.rs` 59-88 | before a result or log artifact is written |
| The symlink-target rule both import and `materialize` call | `crates/tree/src/path.rs` `validate_symlink_target` (146) | as above |

**Their class is security-sensitive.** A change needs an independent review
as well as the security review every PR gets, and the full table. Loosening
one also needs a releases.md entry. Once the join puts these outputs into
the coordination Ground, they're public and permanent. Today the files in
`crates/tree/src/` already take the full table. `crates/bracken/src/weave.rs`
and `crates/bracken/src/weave/` don't, and nothing asks for the independent
review: that's G6's job, keyed on those directories.

**A tightened refusal protects only where it runs.** It needs no
readers-first rollout, since it changes no view. But import's scan and
the runner's output rules run in the writer, so until every writer and
runner uses the release (the pinned `.bracken-bin`, `~/bracken-weave`'s
binary, the box and mac runners), an old one keeps writing what the new
one would refuse, permanently. For an output rule or a key scan, that
upgrade should be prompt, and the releases.md entry says so.

**The runner's log artifact** is written under this rule. Its media type is
`text/plain`, not a Weave type, and it's named `<check> output on <hash>`,
holding raw output (`checks.rs` 92, 555). Its bytes mean nothing to Weave's
views, and the result's `log` line pins them by hash. Because its media
type is outside the prefix, `scripts/bk`'s refusal and `ground-guards`'
tripwire don't match it (section 4).

## 2. How a change would be caught

### 2.1 The guards that exist

| Guard | What it catches | How it fails |
|---|---|---|
| `crates/tree/tests/format_guard.rs` `golden_vectors` | One record of each media type, by bytes and content hash: a file, a manifest with all three modes, a marker with `in_place` null, a line without executors, a decision, a proposal and a resolution | `a Weave payload format changed: this is protocol class`, with the new vectors printed |
| `format_guard.rs`, the rule tests | Round trips; key order, an unknown field, a missing field or a long integer refused as not canonical; only `v` 1 read; unknown modes refused; manifest, line, decision and resolution rules; `MAX_REFERENCES`; change rows; a definition hash changing with argv and executor; `parse_method` | An assertion naming the rule |
| `format_guard.rs` `unicode_tables_are_pinned` | Unicode's version, and a digest of `fold` over every scalar value, which covers icu_casemap's data and the ignorables | Version mismatch, or the digest |
| `format_guard.rs`, the line-merge tests | The line merge (1.6): vectors, the cap, and the cases where git differs | `vector <i>`, `<n> lines`, or the `KNOWN_DIFFERENT` list |
| `crates/tree/tests/paths.rs` | Every path, folding, collision and symlink-target rule, both ways; the length limits relative to their constants | An assertion naming the rule |
| `refuse_unreadable` (`crates/bracken/src/weave.rs` 820-829), over `crates/tree/src/lines.rs` 246-251 and 309-322 (finding `5d50422b9e11`) | A line-media-type artifact that doesn't decode with this build, and supersedes a counted version of its own author's in a relation that author signed. `integrate`, `validate`, `line policy` and `line merge` refuse while one exists anywhere in the Ground, on any line (`weave.rs` 612; `weave/merge.rs` 303, 397, 641; `weave/checks.rs` 262): the list isn't scoped to the line a command acts on (`restrict`, `lines.rs` 524, never filters it), and anyone can open a line (`weave/merge.rs` 260-296), so any signer can stop every line this way. Every other read warns (`judged_for`, `weave.rs` 802-809). `scripts/weave-gate` refuses too, but only by accident: `green_of` (`scripts/weave-gate` 120-128) captures `weave green`'s stderr with its output, so the warning line breaks the JSON and the gate says the output isn't in the shape it reads. A decision that doesn't decode is dropped (`lines.rs` 264-268). A record under a media type this build doesn't know is never collected, so it's neither refused nor warned about | The refusal names the version and says to upgrade |
| `crates/tree/tests/lines.rs` | The ancestor rule: a reference that isn't an ancestor makes the record `NotAnAncestor` and views ignore it | An assertion |
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
| The ancestor rule | `lines.rs`'s tests; the fixture, every record of which passes it | none |
| A policy version this build can't read | `refuse_unreadable` (2.1), for the same media type only, and Ground-wide | **G8**: the refusal isn't scoped per line, so any signer can stop every line; `weave green` only warns, and `scripts/weave-gate` refuses only by accident; an unreadable decision is dropped silently. **Not covered by G8**: a record under a new media type isn't seen at all, and would need a further reader change |
| The marker that counts, and its author | the fixture's derived statuses (its default line and the null-`owner_decision` version); the coordination fixture test's `counted_marker() == None` | none |
| Decision | golden vector; `decision_rules`; `a_decision_refers_to_at_most_max_references`; the fixture, which holds decisions with `source` null and set | none |
| Proposal and its change | golden proposal (its `change` field is an arbitrary hash); change rows by value; the fixture's proposals, whose ids pin their recomputed `change` | **G2**: no golden vector pins a change list's hash directly |
| Resolution | golden vector; `resolution_rules`; the fixture | none |
| Path rules, folding, collisions | `paths.rs`; the Unicode digest; `manifest_rules`; the deep-path test | **G3**: `MAX_PATH_BYTES`, `MAX_COMPONENT_BYTES` and `MAX_REFERENCES` are tested only through their constants (`paths.rs` 213-235, `format_guard.rs` 391-405), so changing a constant's value passes every test unless it's off by a lot. `VERSION` isn't in this gap: the golden manifest and resolution are written with it, and `only_version_1_is_read` fails if it changes |
| Definition hash, method | relative assertions in `format_guard.rs`; the fixture pins argv-only methods through its evidence ids | **G2**: no golden value for a definition hash with an executor, of either kind |
| Details, the counted part | the fixture (`details()` with `Exit::Code` and a text tail) | **G4**: nothing pins `details_with_run`'s first two lines, or `details` with a withheld tail or another `Exit` variant |
| Any of the above, as a review requirement | AGENTS.md and releases.md, read by people | **G6**: nothing mechanical requires protocol-class review for a Weave format change. `scripts/merge` never asks for `owner-review`; `code-scope full` doesn't list `format_guard.rs` or `paths.rs`; AGENTS.md's protocol bullet names only manyroot's files and `docs/protocol.md` |
| The refusals in 1.8, as a review requirement | the security review every PR gets | **G6**: `crates/bracken/src/weave.rs` and `crates/bracken/src/weave/` take a light run, and nothing asks for an independent review for any 1.8 file |

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
- **G3. Pin the constants by value**: `MAX_REFERENCES` 1,024,
  `MAX_PATH_BYTES` 4,096 and `MAX_COMPONENT_BYTES` 255, asserted as literals
  in `format_guard.rs`. `VERSION`, the prefix and the goal tag are already
  pinned through the golden vectors and `artifact_names_and_media_types`.
- **G4. Golden details**: `details` with each `Exit` variant and a withheld
  tail, and `details_with_run`, pinned as text; and `parse_details` on each,
  reading the same manifest and attempt.
- **G5. Content-hash text form**: in `format_guard.rs`, a golden parse of
  `blake3:<hex>`, with refusals of the bare, uppercase and short forms. It
  repeats `hash.rs`'s unit tests in the file the freeze names.
- **G6. Make the review requirement mechanical** (merge-gate tooling). The
  set is keyed on directories, as `code-scope full` is, never on a list of
  modules: frozen rules live in `format.rs`, `hash.rs`, `path.rs`,
  `lines.rs`, `ground.rs`, `results.rs` and `import.rs`, and a module split
  or a new file would slip past any list.
  - `scripts/code-scope frozen <base> <head>` prints every changed path
    under `crates/tree/src/`, `crates/tree/Cargo.toml`,
    `crates/tree/tests/{format_guard,paths,lines,weave_fixture}.rs` and
    `crates/tree/tests/fixtures/`, and a `Cargo.lock` change to
    `unicode-normalization`, `icu_casemap`, `ciborium`, `blake3` or
    `imara-diff`, each with its reason. It also prints a `security:` line
    for every changed path in 1.8's directories: all of `crates/tree/src/`
    (import, `materialize`, `check.rs`, `check/`, `path.rs`, `results.rs` and
    `secrets.rs` are all there, and a new file there lands in the set by
    itself), `crates/bracken/src/weave.rs` and `crates/bracken/src/weave/`.
    So every file 1.8 names gets one, including files that don't exist yet.
  - `scripts/merge`, through main's copy of `code-scope` as it does for
    `full`, refuses a head for which `frozen` prints anything unless the
    coordination Ground holds a current claim tagged `owner-review`, by
    `manager`, not retracted, whose first line names the pr claim's full id
    and the head's full sha. A head that changes a golden vector, a pinned
    fixture value or `tests/fixtures/` also needs, under question 3, an
    approval signed by `owner` (`scripts/owner-approve`) that names the same
    pr claim and sha. For a `security:` line, it needs an independent
    review's pass on the pr claim at that sha. Each refusal says which path
    and what's missing.
  - `crates/bracken/src/weave.rs`, `crates/bracken/src/weave/`,
    `format_guard.rs`, `paths.rs` and `lines.rs` join `code-scope full`'s
    list.
  - AGENTS.md's protocol bullet names `crates/tree/src/` and this page.

  This makes every `crates/tree/src/` change log an `owner-review` and get
  an independent review, and every change to Weave's CLI an independent
  review and the full table. That's more than the frozen items and the
  refusals strictly need. A narrower list would be cheaper, but it would
  rot. An `owner-review` is one claim, and the owner's own go is asked
  only where a pin or the fixture changes.
- **G7. Extend the fixture** by appending to the end of the
  `write_weave_v1_fixture` scenario, so no existing id changes (releases.md,
  2026-10-31): a policy version with executors, and a result written
  through `details_with_run`. This needs the same review as any fixture
  change.
- **G8. Scope `refuse_unreadable` per line, and extend it.** Today it
  refuses Ground-wide (2.1), which lets any signer stop every line, and
  that has to close before the join. G8:
  - counts an unreadable policy version only against the line of the
    counted version it supersedes, under today's rule (it supersedes a
    counted version of its own author's, in a relation that author signed,
    `lines.rs` 309-322). `integrate`, `validate`, `line policy`, `line
    merge`, `weave green` and `scripts/weave-gate` refuse only when a line
    they act on holds one. A line someone opens for themselves can then
    stop only that line;
  - decodes a payload's `v` first, as generic CBOR, so the refusal names
    the version whatever v2's fields are (section 3);
  - makes `weave green` refuse instead of warning, and gives
    `scripts/weave-gate` its own explicit refusal in place of today's
    accidental one;
  - treats a decision that doesn't decode the same way only where it isn't
    retracted, names a line in a top-level `line` field (read as generic
    CBOR), and is signed by an integrator under the last readable policy
    in force on that line. Today such a decision is dropped.

  Any other unreadable record is ignored and listed by `weave check`, as
  today. A record under a media type this build doesn't know isn't
  covered: G8 doesn't collect it. This is a reader change, shipped in the
  readers-first release before any v2 is written. It changes no format.

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
  it. For a policy version, `refuse_unreadable` (2.1) then stops
  `integrate`, `validate`, `line policy` and `line merge` on every line,
  until G8 scopes it to the version's own line. Any other kind is ignored by views and listed by `weave check`.
  The Ground still opens and syncs, since manyroot sees an artifact. The
  refusal's
  wording depends on v2's fields. `Record::decode` (`format.rs` 114-117)
  runs `from_cbor` first, which re-encodes and compares (81-86), and only
  then checks `v`. Serde drops an unknown field and fills a missing
  optional one, so for a v2 whose fields differ from v1's the re-encoding
  differs, and every binary written so far says `payload is not in
  canonical encoding`. Only a v2 with exactly v1's fields gets `payload has
  format version 2; this build reads version 1`. So today's refusal doesn't
  say to upgrade. G8 fixes that for binaries from its release on.
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
- **A cutover, where v2 closes a hole in decisions.** Reading v1 forever
  doesn't mean v1 decisions qualify forever. Otherwise a writer, a
  compromised integrator key say, keeps writing v1 decisions without what
  v2 added. The cutover uses the existing policy machinery, and no new
  trigger:
  - v2's line policy can declare a minimum version. A v1 decision whose
    policy in force (`lines.rs` 729-768) declares v2 the minimum doesn't
    qualify. That's a qualification rule, defined with v2 and frozen with
    it;
  - only the line's author can write that policy version (the supersedes
    rule, `lines.rs` 273-283), under the owner's decision (1.3). No other
    record triggers the cutover, so nobody without authority on the line
    can invalidate honest records;
  - no existing record changes, since no policy declares a minimum before
    v2;
  - step 1's release reads it and refuses to write a v1 decision on a line
    whose current policy declares v2 the minimum.

  **What it doesn't cover, stated plainly.** A decision is judged under
  the policy in force in its own causal past. A writer who leaves the
  minimum-declaring policy version out of a decision's causal parents gets
  that decision judged under the old policy, and it qualifies. Until a
  decision under the new policy exists, it can be the line's only head, and
  nothing forks. After that, the dodging decision can't name one as a
  parent without pulling the new policy into its past (the ancestor rule),
  so it sits beside it as a second head: the line forks, and `weave head`
  shows it. The next honest `integrate` lists every head as a parent
  (`weave.rs` 613-615), so it adopts the dodge without saying so. That's the same exposure every
  policy change has today, such as removing a validator. Proposals,
  manifests, resolutions and file versions carry no policy, so the cutover
  doesn't reach them at all. Question 7 asks whether that's acceptable.
- **An added optional field** is still a new version. The executors field
  went in as an optional field under v1, encoded only when present. A
  binary from before it rejects a line that has one as not canonical,
  which is the right effect. After the freeze, such a field bumps `v`, so
  a reader that knows v2 can tell the two shapes apart. A refusal names the
  version only on binaries with G8, but `refuse_unreadable` already refuses
  while such a policy version exists, Ground-wide until G8.
- **Pinned-not-frozen changes** (1.6) stay protocol class with a
  releases.md entry, as today, but need no new version: they change views,
  not records. They take the same readers-first rollout when two binaries
  on different rules would show different heads.
- **Refusals at write and use** (1.8) may tighten in any release, under
  their own review. They need no readers-first rollout: an old binary that
  still writes what a new one refuses produces valid records, which the new
  binary reads and, where it must, refuses to materialize. But they protect
  only once every writer and runner uses the release (1.8), so that upgrade
  is part of shipping the fix.

## 4. The join, and agent-keys.md 4.10

- **The freeze comes before the join.** weave.md section 5 makes frozen
  formats a condition of the join, because the join puts Weave records into
  the coordination Ground. From then on, a Weave format change affects
  every reader of that Ground: `scripts/bk`, `scripts/merge`, the pinned
  binary and the coordination fixture tests.
- **The merge gate relies on failing closed.** After the join, a stale
  binary reading the coordination Ground must not compute green under a
  policy that's no longer in force. The readers-first rollout is a rule
  that nothing checks. Today `refuse_unreadable` stops a stale binary from
  acting under an old policy, but on every line at once, so any signer can
  stop every line with one record of their own. `weave green` only warns,
  and `scripts/weave-gate` refuses only by accident (2.1). G8 scopes the
  refusal to the lines a command acts on, and makes green and the gate
  refuse deliberately. It must be in every gate reader before the join and
  before any v2 is written. That holds only if a format change keeps its
  media type, since G8 doesn't see a new one (question 2).
- **The join's own record is frozen too.** Step 3 of the join writes a
  marker with `in_place` set. Its format is frozen (1.3), but no golden
  vector pins it yet, so G1 should land before the join.
- **The join relies on two frozen things.**
  - Content hashes don't depend on the Ground, and manifests encode the
    same in either Ground, so step 4's manifest hash can equal the dogfood
    decision's.
  - The prefix is frozen, so `ground-guards`' tripwire and `scripts/bk`'s
    refusal keep matching every record with a Weave media type until the
    join PR lifts them on purpose. The runner's log artifact isn't one of
    them: it's `text/plain`, so neither matches it (1.8). The join's design
    has to name it: whether it lands in the coordination Ground at all, and
    which guard checks it there. After the join, 1.8's output rules are
    what keep a key out of a public, permanent Ground.

  Import's exclusions aren't frozen (1.8). If they change between the
  dogfood decision and the cutover, step 4's hashes differ and the cutover
  stops visibly, instead of recording the wrong tree.
- **agent-keys.md 4.10.** The freeze depends on neither part A nor part B.
  A line policy names keys as 32-byte AgentIds, and that stays frozen.
  Rotating the runner or the integrator is a new policy version under an
  owner decision, in the same format. Part A's server-held keys sign the
  same records. Part B's delegation, revocation and ruling records would be
  new artifacts with formats of their own, pinned like Weave's (agent-keys.md
  4.8). They don't change the frozen formats. Part B would change who counts
  as a validator or integrator, which is a derivation rule. That's why those
  rules are pinned, not frozen (1.6): part B can change them with a
  protocol-class review and no new format version. Validity mostly doesn't
  depend on who signed. The exception is the null `owner_decision` rule,
  which depends on the counted marker and its author (1.3), so they're
  frozen with it. A part B change to which marker counts, or to whose
  signature stands for its author, is a frozen-format change (section 3),
  not a derivation change. `owner_decision` stays
  text naming a coordination claim, which `scripts/weave` checks (4.10's
  last point).

## 5. Questions for the owner

1. **What the freeze covers.** Freeze 1.1 to 1.5 as version 1, the
   ancestor rule included; keep the derivation rules and the line merge
   pinned but not frozen (1.6); let 1.8's refusals tighten as
   security-sensitive changes; and leave 1.7 out? *Recommended: yes.* Part
   B, and later fixes to Weave's views, need the derivation rules to stay
   changeable under review. A security fix needs the refusals to tighten
   without a format version. The records are what can never be
   rewritten.
2. **How a format changes.** A new `v` under the same media type, or a new
   media type? *Recommended: a new `v` under the same media type.* It's
   the option that fails closed with today's binaries. A policy version
   with a new `v` doesn't decode, so `refuse_unreadable` stops `integrate`,
   `validate`, `line policy` and `line merge` (2.1). A new media type is
   never collected, so today's binaries act under the superseded policy
   with no warning, the fail-open that finding `5d50422b9e11` closed. The
   cost, until G8: today's refusal is Ground-wide, so a v2 policy version
   on any line stops `integrate`, `validate`, `line policy` and `line merge`
   on every line for each binary that hasn't been upgraded, and the
   message (`not in canonical encoding` when v2's fields differ) doesn't
   say to upgrade. Readers-first keeps that from happening to honest
   binaries. A new media type would need a reader change beyond G8, which
   doesn't collect unknown media types. A v2 that closes a hole in
   decisions also takes section 3's cutover.
3. **Whose go a frozen-format change needs.** Protocol-class merges are
   delegated to the manager today, logged as `owner-review` (decision
   `adcb78b88c6c`). Keep that for frozen formats, or require your own go?
   *Recommended: your own go until the join, then review the rule again.*
   A frozen-format change is rare, can't be undone once records exist, and
   at the join it reaches the coordination Ground. Changes to the
   pinned-not-frozen rules stay delegated.
4. **Close the guard gaps before the freeze?** G1 to G6 are small build
   tasks; G7 needs a fixture change; G8 is a reader change.
   *Recommended: land G1 to G6 first, then your go to freeze; G7 can
   follow, and G8 must land before the join and before any v2.* The freeze
   would otherwise rest on pins that don't fully exist yet, notably the
   join's marker (G1) and the review rule (G6). G8 closes today's
   Ground-wide stop (2.1) before the join opens the Ground to more
   writers.
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
7. **Is a policy-version cutover enough for a v2 that closes a hole?**
   Section 3's cutover is a v2 line policy declaring a minimum version.
   It's written only by the line's author, under your decision. It stops
   v1 decisions qualifying wherever that policy is in force. It doesn't
   stop a writer who leaves the policy out of a decision's causal past,
   and it doesn't reach proposals, manifests, resolutions or file
   versions. *Recommended: yes, use it only for a v2 that closes a hole in
   decisions, and accept the gap.* The gap is the exposure every policy
   change already has, and closing it is about who may write to a line
   (agent-keys.md, part B), not about formats. A v2 that closes a hole in
   another kind needs its own design, under question 3's go.
