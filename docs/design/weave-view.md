# The Weave view: design

Design (2026-11-23), task `51acc5b46820`, from the owner's request in
question `7e01a0dbf714`: "There could be a specific UI that's specific for
Weave ... We should even essentially be able to view the full source code
and all of that." The Weave view is a set of pages in `bracken view` for a
Ground that holds Weave records ([weave.md](weave.md)). It is separate from
the Loom and the other Ground pages ([visualizer.md](visualizer.md)). Its
open questions went to the manager as question `90dc7287b194`, and answer
`e490eaaaebb3` accepts every recommendation (section 9).

It changes nothing in `crates/tree`, in manyroot's protocol files or in any
frozen format ([weave-freeze.md](weave-freeze.md)). It adds no dependency. It
never writes to any Ground, and it never writes a tree to disk.

## 1. What a reader can do

- **See the Weave Ground at a glance.** The counted marker and the root,
  every current line with its name, author and reach (`weave line list`),
  its head or the heads of a fork, its green state, and how many decisions,
  open proposals and open objections it has. Also what `weave check` would
  list: invalid payloads, incomplete manifests, extra markers and ignored
  policy versions, plus records this build can't read (`Lines::unreadable`).
- **Browse a line.** Its current policy: integrators, validators, goal
  setters, each check with its argv and definition hash, executors,
  guardrails, and the owner decision the version cites. Also the policy's
  version chain, its heads, its green decision, its goals, its open
  objections and its open proposals. Its decisions are listed newest first
  in causal order, a page at a time.
- **Open a decision.** Every field of its payload: parents, proposals,
  resolutions, answers with their dispositions and reasons, breakage, goals,
  the git commit it mirrors, and the integrator's note. Also its verdict
  (qualifies, or the first rule it breaks, in the words of `Unqualified`'s
  message), the policy in force for it, its counted check results under the
  line's current policy (passes, fails, and fails a later attempt replaced),
  and its reviews.
- **Read any tree.** A line head, any decision, any proposal or any
  manifest opens as a source tree: directories as path prefixes, then files.
  A file shows as text with line numbers. A symlink shows its target,
  marked as a symlink. A binary file, a file over the text cap, or a file
  this replica doesn't hold is named with its mode, size and content hash,
  and its bytes aren't shown.
- **Follow a file's history.** On a line, the decisions where a path's file
  value changed: added, modified, mode changed or deleted. Each links to the
  file at that decision and to its diff against the decision before.
- **Diff two records.** Any two decisions, proposals or manifests. First the
  per-file list (`diff::manifests`), then, for one file, the unified diff of
  its text (`diff::unified`). A file that is too large or not text is named
  only.
- **Follow proposals.** On a line, each proposal with its status:
  - *open*: valid, and no decision on the line lists it or a proposal with
    the same change;
  - *integrated*: a decision lists it or one with the same `change`. It
    links to that decision and the integrator's note;
  - *invalid*, with the reason (`InvalidProposal`);
  - *retracted*.

  A proposal's page shows its base, summary, author and change (the paths
  it changes, before and after). It also shows its reviews, carried over by
  change (`Reviews::of_proposal`), and the objections a decision using it
  unchanged would have to answer (`Lines::proposal_objections`). For an open
  proposal it shows its **conflicts** with the line's current head, as
  integrate would meet them: each conflicting path, the base line ranges
  where the text conflicts, and the resolutions offered for that key, with
  their authors and results, marking those an integrator of the line wrote.
- **See checks and their evidence.** For any manifest: every check result
  on it, with validator, check, pass or fail, attempt, and whether it's
  retracted or replaced. Each shows the details as recorded: argv, exit and
  output tail. A box check's log artifact is linked and readable as text.
  The page also shows the counted results under a line's current policy (as
  `weave green --manifest`) and the reviews of the manifest. Goals show
  with whether they bind under the current policy. Objections show with
  their basis (evidence-backed, goal-backed or discussion, and `replaced`),
  the dispositions they allow, and the decision that answered them, if any.
- **Jump to any record by id.** A prefix of 8 or more hex digits, or
  `blake3:<64 hex>` for a content hash, opens the right page for that
  record, or says what kind of record it is (`ground::kind_of`).

Every page has a URL fragment (`/weave.html#decision/<id>`,
`#tree/<record>/<path>`), so any view can be linked and reloaded.

## 2. Where it sits

- **In `bracken view`, not a new server.** The Weave pages are
  `/weave.html` with `weave.js` and `weave.css`. They reuse `common.js`,
  `visualizer.css` and the embedded fonts. Code uses the system monospace
  font (`ui-monospace, monospace`), so nothing new is downloaded. The API is
  under `/api/weave/`. A replica with no counted Weave marker answers
  `/api/weave` with `{"weave": false}` and every other Weave endpoint with a
  404 that says why. So the same pages work on the team Ground after the
  join, and cost the team Ground's view nothing before it.
- **The visual language is the visualizer's** (soil and specimen,
  visualizer.md section 2; owner preference `c60c03bca23d`). Lines read as
  stems. Decisions are knots on them, with the decision knot's shape. Green
  is the accepted colour. Code is set plainly on the paper ground, with no
  syntax colouring.
- **Code layout.** A new module, `crates/bracken/src/view_weave.rs`, holds
  the Weave state and every `/api/weave` handler. `view.rs` changes in only
  four places: an optional Weave state in `Snapshot`, one routing arm, the
  three asset rows, and each new endpoint's parameters in the
  `takes` table. The JSON reuses the CLI's `--json` shapes and field names
  (`weave log`, `head`, `green`, `review`, `line list`). The helpers that
  build them in `weave.rs` and `weave/review.rs` become `pub(crate)`. They
  aren't copied. The Loom's builder (task `b140c091be3d`, PR #73) edits the
  Loom's own files. Of the files this work touches, only `view.rs` is
  shared with them, and only in the places above.

## 3. The Weave state

When the watcher re-reads the replica (`Viewer::watch`), it builds the Weave
state with the snapshot, once per change, off the request path. Every request
reads from that state:

| Field | Built with | Notes |
|---|---|---|
| `index` | `Index::build(g, weave::root(dir))` | The root comes from the replica's `weave-root` file, read-only, as every `bracken weave` read does |
| `lines` | `Lines::build(g, &index)` | Every line is judged: the home page lists them all |
| `checks` | `Mutex<Checks>`, from `Checks::build(g, &index, &lines)` | `Checks` holds a `Cell` (its epoch-miss count), so it is `Send` but not `Sync`. A mutex shares it, so green and counted results are computed one request at a time. They take milliseconds |
| `reviews` | `Reviews::build(&index, &lines)` | |
| `records` | one pass over `g` | The ids of every proposal, resolution and `text/plain` artifact, by media type. `Lines` lists only valid proposals per line, and `Resolutions` has no iterator |

Nothing is built when the index finds no counted marker. If the state fails
to build, the snapshot keeps the Loom's part and the Weave endpoints answer
503 with the reason. The view's other pages are never lost to a Weave
problem.

## 4. The API

Every endpoint is a GET and answers JSON (`application/json`), built by the
existing `json_bytes` (`<`, `>`, `&`, U+2028 and U+2029 escaped). As
everywhere in the view, a parameter an endpoint doesn't take is refused with
a 400, and so is a bad value. A record in a path is a ContributionId of 8 or
more hex digits, resolved as `/api/contribution` resolves it, or
`blake3:<64 hex>` for a manifest. A file path is the `path` query parameter,
percent-decoded by the existing `percent_decode`. Lists take `limit` (default
200, at most 2,000) and `after`, the last key of the previous page: an id, or
a path for path-ordered lists. Each list answers `more: bool`, so a page
stays the same as new records arrive.

| Endpoint | Gives | Read with |
|---|---|---|
| `/api/weave` | `weave`, marker, root, the lines (as `weave line list --json`, each with `heads`, `forked`, green `state`, and counts of decisions, open proposals and open objections), `problems` (first 200 and `total`), `unreadable`, `ignored_versions` | `Index::counted_marker`, `root`, `problems`; `Lines::lines`, `heads`, `unreadable`, `ignored_versions`; `Checks::green` |
| `/api/weave/line/<line>` | current policy versions (several if diverged) with every policy field, the version chain, heads, `green` (as `weave green --json`), goals with `binds`, open objections (as `weave review --json`'s `open`) | `Lines::line`, `current_versions`, `version`, `heads`, `open_objections`; `Checks::green`, `counted`; `reviews::goals`, `Goal::binds` |
| `/api/weave/log/<line>?after&limit` | decisions newest first, as `weave log --json` rows | `Lines::log` |
| `/api/weave/decision/<id>` | the `weave log` row, plus `verdict` (`qualifies`, or the rule's message), `policy` in force, `counted` per check, reviews | `Lines::decision`, `verdict`, `policy_in_force_for`; `Checks::counted`; `Reviews::of_decision` |
| `/api/weave/tree/<record>?dir&after&limit` | the record's manifest hash, and the direct children of `dir` (default the root): directories with the number of files below them, then files with mode, size, hash and `held` | `Index::manifest`, `holds_file`; the manifest's sorted entries, by a range search on the prefix |
| `/api/weave/file/<record>?path` | `path`, `mode`, `size`, `hash`, `shown`: `text`, `symlink`, `binary`, `oversized` or `missing`; `text` only when shown | `Manifest::get`, `Index::file`, `diff::is_text` |
| `/api/weave/history/<line>?path&after&limit` | the line's decisions, newest first, where the path's file value differs from the decision's first parent on the line (or the path appears in a root decision), each with `kind`, `before` and `after` | `Lines::log`, `decision`; `Index::manifest`; `Manifest::get` |
| `/api/weave/diff?a&b&after&limit` | the per-file list from `a`'s tree to `b`'s: `path`, `kind`, `before`, `after` | `diff::manifests` |
| `/api/weave/diff?a&b&path` | that one file: `unified` text, or `shown` saying why not (`binary`, `oversized`, `too_large_to_diff`, `missing`) | `diff::unified`, `diff::is_text` |
| `/api/weave/proposals/<line>?status&after&limit` | proposals on the line, ascending by id, each with `status`, author, base, summary, change size; `status` filters (`open`, `integrated`, `invalid`, `all`) | `records`; `Lines::proposal`, `proposal_named`, `log`; `format::Change::hash` |
| `/api/weave/proposal/<id>` | the proposal, its status and integrating decision with its note, change entries (first 2,000 and `total`), reviews, objections to answer, and, if it's open, `conflicts` | `Lines::proposal`, `proposal_objections`; `Reviews::of_proposal`; `merge::three_way`, `resolve_lines`, `text_conflicts`; `Resolutions::for_key`, `Index::resolutions` |
| `/api/weave/manifest/<hash>?line` | every check result on it, with details (cut to 64 KiB, as the view's `TEXT_BYTES`), a box log link where the details name one (`log <id> <hash>`) and it hashes as named, the counted results under `line`'s current policy (default the default line), its reviews, the decisions and proposals with this manifest | `Checks::on_manifest`, `counted_on_manifest`; `Reviews::of_manifest`; the evidence's `details` from `g` |
| `/api/weave/log-artifact/<id>` | a `text/plain` box log's text, cut to the text cap | `records`; `g` |
| `/api/weave/record/<id or hash>` | `kind` and the page that shows it | `ground::kind_of`, `Index::hash_kind` |

A tree's record can be a decision (its manifest), a proposal (its manifest),
a manifest artifact, or a content hash. A tree is shown only for a valid
manifest. Its completeness is reported (`Index::missing`, as a count and the
first 200 paths), and a file this replica doesn't hold shows as `missing`.

### Caps

| What | Cap | Why |
|---|---|---|
| A file's text shown | 1 MiB | About 20,000 lines of code, rendered in the page at once. The repository's largest text file is about 6,500 lines |
| Each side of a unified diff | 1 MiB, and `diff::MAX_DIFF_CELLS` | `diff::unified` builds a table of up to 16 million `u32`s, 64 MB. So at most **2 diffs** build at once (the `Slots` that bound `as_of` builds today); a third request gets a 503 to retry |
| A proposal's conflicts | 50 text-conflict computations a request, 2 requests at once | Each is a histogram diff of up to 10,000 lines per side (`text_merge::MAX_DIFF_LINES`). The rest of the conflicting paths are listed without line ranges |
| List pages | 200 by default, 2,000 at most | |
| Check details, box logs | 64 KiB, cut on a character boundary (`cut_bytes`) | The view's existing text cap |
| Problems, missing paths, change entries | 200, 200, 2,000, each with a `total` | |

## 5. Performance

**Now.** The Weave Ground holds 3,314 contributions and 126 MB of payload,
counted through `scripts/weave log --json` on 2026-11-23:
- 2,033 file versions;
- 429 manifests, the largest 58 KB, about 830 paths;
- 314 proposals and 157 decisions;
- 6 policy versions on one line;
- 250 check results, 118 box logs and 5 reviews.

A whole `scripts/weave weave head|log|green|review|check --json` run takes
0.6 to 0.75 s. That covers process start, opening the replica through its
snapshot, `Index`, `Lines` and the command. So a re-read with the Weave state
should cost well under a second, once per change.

Requests then cost:
- a tree directory: a range search over at most about 830 entries;
- a file: one map lookup and the JSON escape of at most 1 MiB;
- a history: one manifest lookup for each of the line's 157 decisions;
- a diff list: one pass over two manifests.

Only unified diffs and conflicts do real work, within the caps above.

**At 10x** (about 33,000 contributions and 1.3 GB of payload; 1,570
decisions; an 8,300-file tree): the replica is held in memory, as every
`bracken` reader holds it. During a re-read the old snapshot keeps serving
while the new one builds, so memory is briefly about twice the replica.
`Index`, `Lines` and `Checks` are near-linear in the records they index. The
estimate is a rebuild of about 5 to 10 s, off the request path. A history is
1,570 lookups; a tree directory is a range search over 8,300 entries. PR (b)
measures this on a generated 10x Weave Ground and records the numbers. If a
rebuild passes 10 s or memory passes 4 GB, the builder stops and reports
rather than tune `crates/tree`. Past that size the answer is the index store
([index-store.md](index-store.md)), not this view.

The page fetches only what it shows. A tree is fetched one directory at a
time, a file only when it is opened, and a diff one file at a time.

## 6. Serving the Weave Ground

The Weave Ground, `~/bracken-weave/manyroot`, is reached only through
`scripts/weave` (AGENTS.md section 1). The view is reached the same way:

```sh
scripts/weave view [--port N]     # the pinned binary, BRACKEN_DIR set to the checked Weave Ground
```

- `scripts/weave` already resolves the Ground's real path and exports it
  as `BRACKEN_DIR`. Before running anything it refuses a path inside a git
  checkout, the coordination Ground, and a Ground whose id or root doesn't
  match the committed pin. Those checks hold for `view` unchanged.
- **Read-only.** `bracken view` opens the replica through the snapshot with
  `Refresh::No` (`main.rs`), so it never writes `cache/snapshot`. It reads
  agents' names from `agents/` only (`public_names`), never `identities/`,
  so no private key of the Weave Ground is ever in the process. Every
  endpoint is a GET. The Weave state is in-memory reads of `crates/tree`.
  The view never calls `materialize`, the check runner, `checkout` or
  `resolve --markers`, and nothing in `view_weave.rs` touches the file
  system: a test checks that the module names no `std::fs`, `File` or
  `materialize` (section 7).
- **What it writes to `cache/`: nothing.** A `scripts/weave` read today
  opens with `Refresh::Yes`. It may append a segment to `cache/snapshot`,
  or rewrite it, and create `cache/` and its `.gitignore`. `bracken view`
  writes none of these: with `Refresh::No` it reads the snapshot and never
  writes it (snapshot.rs, and its test that the view leaves the file's
  bytes unchanged). The index store (index-store.md section 5, *Opens that
  can't write*; PR #74) never opens its SQLite files for `bracken view`,
  because even a read-only SQLite open writes `-wal` and `-shm`. The view
  reads through the full path instead. So the view writes less to the
  Weave Ground's `cache/` than any `scripts/weave` read does, and nothing
  anywhere else. PR (b)'s test holds it to that: it runs a session of
  requests and checks that every file under the Ground, `cache/` included,
  is unchanged in name, size and bytes. Once PR #74 lands, the same test
  shows no `index-*.sqlite`, `-wal`, `-shm` or `clock` file was created or
  changed.
- **Loopback only, in the foreground.** The Weave view isn't bound to
  NetBird and isn't a launchd service. Owner decision `917816c72975` opens
  only the team Ground's view on NetBird, and `6fed6a4c8af0` approves only
  that view's service. Either for the Weave Ground would be the owner's go.
- **A gap to close.** Today `scripts/weave` refuses only `sync` and
  `serve`, so `scripts/weave view --bind <any address>` and `--runs` already
  pass through. PR (a) makes `scripts/weave` let `view` through with
  `--port` and the policy options only, and refuse `--bind`, `--host` and
  `--runs`, with a test for each that fails on `main`. Its header says why.
  That PR is merge-gate tooling, a risky class, and gets an independent
  review and a security review.
- The pages reach the Weave Ground only once the manager upgrades the
  pinned binary past their merge (releases.md).

## 7. Security model

Every path, file name, file's content, note, summary, reason, goal, check
name, argv, output tail and log on a Weave Ground is author-controlled.

- **Nothing is rendered as HTML.** All of it leaves the server only as JSON
  string values, escaped by `json_bytes`, under the view's CSP (no inline
  script or style, no eval, `default-src 'none'`). The page inserts it only
  with `textContent` or `createTextNode`. The existing test that the view's
  scripts never parse text as markup or code covers `weave.js` as soon as
  it is in `ASSETS`. Line numbers, the tree and diff markup are built as
  elements, never as strings. Text never goes into an attribute, except a
  `href` the page builds itself from a fragment and a record id it has
  checked as hex. A path in a link is percent-encoded by
  `encodeURIComponent`.
- **Invisible characters are marked.** Bidirectional controls (U+202A to
  U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C) and other invisible
  format characters (zero-width, U+FEFF, U+00AD, the tag block) are
  shown as visible markers in paths, names and file text. So a file can't
  read differently from its bytes (Trojan Source, CVE-2021-42574). A path
  with control characters shows them as `\u{..}` escapes, as the CLI's
  `terminal::printable` does.
- **Paths never touch the file system.** A path is a key into a manifest
  held in memory (`Manifest::get`, or a range search on sorted entries),
  compared as bytes. It is never joined to a directory, opened, normalised,
  or used to choose a response type. A path that isn't in the manifest is a
  404. A record id is hex, resolved in the graph.
- **Served bytes have safe content types.** The view serves only its own
  embedded assets, with the types in `ASSETS`, and `application/json`.
  There is no endpoint that serves a file's or a log's raw bytes, so no
  author byte is ever a response body except inside a JSON string. Binary
  files are named, not shown or offered for download. Every response keeps
  `X-Content-Type-Options: nosniff`, the CSP, `Cross-Origin-Resource-Policy:
  same-origin` and `X-Frame-Options: DENY`.
- **Bounded work.** Every list pages, every text is capped, and the
  expensive computations (unified diffs, conflicts) are slotted, with their
  inputs capped (section 4). The listener's connection budget, Host
  checking and timeouts are the view's own and unchanged. A hostile Weave
  Ground costs the view the same as its size, as it costs every reader.
- **Not secrets, but not published either.** Import refuses secrets
  (weave.md section 3), but a check's output tail or a box log could still
  hold something private about this machine. The view never serves the
  Weave Ground beyond loopback (section 6).
- **No new trust.** The view shows Weave's own derived state (qualifying,
  green, bases), computed by `crates/tree` as the CLI computes it. Under
  shared keys an author is who claimed to act (weave.md, Summary), and the
  page says so where it names one.

## 8. Build plan

One PR at a time through the gate. Each PR includes:
- Rust tests of its API on throwaway Weave Grounds that the test builds
  with its own binary (`bracken weave init`, `import`, `propose`,
  `integrate`) in a temporary directory;
- a deterministic check of its page in Node's `vm` with a mock DOM, as PR
  #73's `loom_artifacts_check.js` does;
- a phone-width screenshot.

No PR touches `~/bracken-weave/manyroot` except through `scripts/weave`,
read-only.

| PR | Gives | Class |
|---|---|---|
| (a) | `scripts/weave view`: `--port` and the policy options only; `--bind`, `--host`, `--runs` refused; tests; header and docs/cli.md | merge-gate tooling (risky) |
| (b) | The Weave state in the snapshot; `/api/weave`, `line`, `log`, `decision`, `record`; the home, line and decision pages; the 10x measurement; a test that a hostile Weave Ground's strings come back escaped and that `view_weave.rs` names no file-system call; a test that a session of requests leaves the Ground byte for byte the same | cli |
| (c) | `tree` and `file`; the tree browser and file page, with line numbers and invisible characters marked; tests for binary, oversized, symlink, missing and colliding-looking paths | cli |
| (d) | `history` and `diff`; the history and diff pages; tests at the caps and for the diff slots | cli |
| (e) | `proposals` and `proposal`, with status and conflicts; the proposal pages; tests for each status and a conflict with an offered and an integrator's resolution | cli |
| (f) | `manifest` and `log-artifact`; check results with details and box logs, goals and objections; tests for counted, replaced and retracted results, and each objection basis | cli |

Done when (b) to (f) are merged, and (a) is merged or the manager has
decided against it.

## 9. Questions, answered

Question `90dc7287b194` asked these. The manager's answer `e490eaaaebb3`
accepts every recommendation. It adds that PR (a) needs tests that fail on
`main`, gets an independent review on Gemini and a security review, and that
the design says what the view writes to `cache/` (section 6). Any other
exposure needs the owner's own approval.
1. Loopback only, in the foreground, no NetBird and no service (section 6).
2. PR (a) closes the `scripts/weave view` gap, and goes first.
3. No raw-bytes endpoint and no download in this task.
4. The caps in section 4.
5. A proposal's conflicts are computed on request, in memory, as integrate
   would meet them.
6. Reuse the CLI's JSON helpers as `pub(crate)`.
7. A one-line link to `/weave.html` in the Loom's nav (`index.html`), in PR
   (b), coordinated with the Loom's builder.
8. Invisible characters marked.
9. The 10x measurement and its stop thresholds.
10. The build plan above.
