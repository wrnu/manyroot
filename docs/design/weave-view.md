# The Weave view: design

Design (2026-11-23), task `51acc5b46820`, from the owner's request in
question `7e01a0dbf714`: "There could be a specific UI that's specific for
Weave ... We should even essentially be able to view the full source code
and all of that." The Weave view is a set of pages in `bracken view` for a
Ground that holds Weave records ([weave.md](weave.md)). It is separate from
the Loom and the other Ground pages ([visualizer.md](visualizer.md)). Its
open questions went to the manager as question `90dc7287b194`, and answer
`e490eaaaebb3` accepts every recommendation (section 9). Round 1's findings
and how this revision handles them are in section 10.

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
  versions, its heads, its green decision, its goals, its open objections
  and its open proposals. Its decisions are listed newest first in causal
  order, a page at a time.
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
  marked as a symlink. A file is named with its mode, size and content hash,
  and its bytes aren't shown, when it is:
  - binary;
  - over the text cap;
  - a symlink whose target isn't text;
  - not held by this replica at its entry's size.
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
  proposal on a line with one head, it shows a **conflict preview** against
  that head, as integrate would meet it (section 4, *Conflict preview*):
  - each conflicting path;
  - whether its lines would merge cleanly, or the base line ranges where
    they conflict;
  - the resolutions offered for that key, with their authors and results,
    marking those an integrator of the line wrote.
- **See checks and their evidence.** For any manifest, every check result on
  it, by anyone (`Index::check_results`), with:
  - its author, marked if it is a validator under the line's current policy;
  - its check, pass or fail, and attempt;
  - whether it is retracted or replaced;
  - its details as recorded: argv, exit and output tail;
  - a box check's log, linked and readable as text.

  The page also shows the counted results under a line's current policy (as
  `weave green --manifest`), the reviews of the manifest, and the objections
  to integrating it unchanged (`Lines::manifest_objections`).
- **Goals and objections.** Goals show with whether they bind under the
  line's current policy. **Open** objections (`Lines::open_objections`,
  `proposal_objections`, `manifest_objections`) show with their basis
  (evidence-backed, goal-backed or discussion, and `replaced`) and the
  dispositions they allow. For every other objection, the view shows only
  what the payloads say: the objection itself (`Obligations::list`), that
  it is not open on that line, and the decisions whose `answers` name it,
  its target or their own line, each with its disposition and reason. It
  doesn't say which decision *settled* an objection, or how an answered
  objection's basis was classified, because `crates/tree` keeps those
  private (`Obligations`' settled map and its basis classification). Showing
  them needs a public read in `crates/tree`. That is a later item, outside
  this task, and as a change to `crates/tree` it needs the review and
  approvals AGENTS.md section 1 sets.
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
  join (section 7 says what that join must then approve), and cost the team
  Ground's view nothing before it.
- **The visual language is the visualizer's** (soil and specimen,
  visualizer.md section 2; owner preference `c60c03bca23d`). Lines read as
  stems. Decisions are knots on them, with the decision knot's shape. Green
  is the accepted colour. Code is set plainly on the paper ground, with no
  syntax colouring.
- **Navigation without links.** `common.js` refuses `a` elements and the
  `href` attribute by design (visualizer.md, the allowlist; the test in
  `view.rs` that no script but `common.js` sets attributes). `weave.js`
  keeps that rule. Every way to move between Weave pages is a `button`
  whose click handler sets `location.hash` to a fragment. `weave.js`
  builds the fragment from a route name it holds and a record id it has
  checked as hex (or `blake3:` and 64 hex digits). A path goes in only
  after `encodeURIComponent`. The page reads the route back on
  `hashchange` and checks it the same way before using it, as
  `root.js` does. `common.js` doesn't change.
- **Code layout.** A new module, `crates/bracken/src/view_weave.rs`, holds
  the Weave state and every `/api/weave` handler. The JSON reuses the CLI's
  `--json` shapes and field names (`weave log`, `head`, `green`, `review`,
  `line list`). The helpers that build them in `weave.rs` and
  `weave/review.rs` become `pub(crate)`. They aren't copied.
- **Shared files.** These are the files the plan touches that other pages
  or other builders use too:
  - `crates/bracken/src/view.rs`: the Weave state in `Snapshot`, one
    routing arm, three `ASSETS` rows, the new endpoints' parameters in the
    `takes` table, a non-blocking `Slots::try_take` (section 4, Caps),
    `weave-root` added to `signature()` (section 3), and the
    scripts-checking test's count, `assert_eq!(scripts, 11, ...)`, raised
    to 12;
  - `crates/bracken/src/main.rs`: `Command::View` hands its `Store` to
    `view::serve`, instead of keeping it open beside the view's own
    (section 5);
  - `crates/bracken/assets/view/index.html`: one nav line,
    `<a href="/weave.html">Weave</a>`, static markup beside the Loom,
    Trust, Tension and Runs links. That is the Loom's builder's file
    (task `b140c091be3d`); the manager passes the change to them;
  - `crates/bracken/tests/view.rs`: `/weave.html` added to the list of
    pages whose `src` and `href` must be served.

  `common.js`, `visualizer.css` and the fonts are read, not changed.

## 3. The Weave state

When the watcher re-reads the replica (`Viewer::watch`), it builds the Weave
state with the snapshot, once per change, off the request path. Every request
reads from that state:

| Field | Built with | Notes |
|---|---|---|
| `root` | the replica's `weave-root`, read as below | No root, no Weave state: nothing else is built |
| `index` | `Index::build(g, root)` | |
| `lines` | `Lines::build(g, &index)` | Every line is judged: the home page lists them all |
| `logs` | `Lines::log(g, line, None)` once per current line, kept as id lists | `Lines::log` runs `newest_first`, a sort of the whole graph, on every call. So no request calls it |
| `checks` | `Mutex<Checks>`, from `Checks::build(g, &index, &lines)` | `Checks` holds a `Cell` (its epoch-miss count), so it is `Send` but not `Sync`. A mutex shares it, so green and counted results are computed one request at a time. PR (b) measures how long it is held, now and at 10x |
| `reviews` | `Reviews::build(&index, &lines)` | |
| `records` | one pass over `g` | The ids of every proposal, resolution and `text/plain` artifact, and every `supersedes` relation between line versions (a version's chain, which `Lines` keeps private), by media type and kind. `Lines` lists only valid proposals per line, and `Resolutions` has no iterator |
| `previews` | an empty map, filled on request | Conflict previews, by proposal and head (section 4) |

**Reading `weave-root`.** `weave::root` reads the file with an unbounded
`read_to_string`. `scripts/weave` refuses a `weave-root` that isn't a
regular file only once, when it starts. So the view doesn't call
`weave::root`. On each rebuild it opens the file itself, with these rules:
- open with `O_NOFOLLOW | O_NONBLOCK`;
- check on the open descriptor that it is a regular file;
- read at most 256 bytes;
- parse it as `weave::root` does.

A link, a FIFO or anything else gives no root. The view logs why and serves
no Weave pages until the file is fixed. `signature()` counts `weave-root`'s
size and modification time, so a `weave root set` is seen at the next poll.
This read is the only file access in `view_weave.rs`, in one named function.
The test in section 8 allows that function and refuses any other.

If the state fails to build, the snapshot keeps the Loom's part and the Weave
endpoints answer 503 with the reason. The view's other pages are never lost
to a Weave problem.

## 4. The API

Every endpoint is a GET and answers JSON (`application/json`), built by the
existing `json_bytes` (`<`, `>`, `&`, U+2028 and U+2029 escaped). As
everywhere in the view, a parameter an endpoint doesn't take is refused with
a 400, and so is a bad value.

- **Records.** A record in a path is a ContributionId of 8 or more hex
  digits, resolved as `/api/contribution` resolves it, or `blake3:<64 hex>`
  for a manifest.
- **Lines.** A `<line>` is always an id. Names are resolved by the page
  from `/api/weave`'s list, which marks the lines a name reaches (gap G9),
  never by the server from text.
- **Paths.** A file path is the `path` query parameter, percent-decoded by
  the existing `percent_decode`.
- **Lists.** They take `limit` (default 200, at most 2,000) and `after`,
  the last key of the previous page. That is an id, or a path for
  path-ordered lists. In a tree it's a child's name, not its full path, so
  a request stays far under the 16 KiB head limit (`serve.rs`, `MAX_HEAD`).
  Each list answers `more: bool`, so a page stays the same as new records
  arrive.

| Endpoint | Gives | Read with |
|---|---|---|
| `/api/weave` | `weave`, marker, root, the lines (as `weave line list --json`, each with `heads`, `forked`, green `state`, and counts of decisions, open proposals and open objections), `problems` (first 200, each `Incomplete`'s `missing` cut to 20, and `total`), `unreadable`, `ignored_versions` | `Index::counted_marker`, `root`, `problems`; `Lines::lines`, `heads`, `unreadable`, `ignored_versions`; `Checks::green` |
| `/api/weave/line/<line>` | current policy versions (several if diverged) with every policy field, every version of the chain (`records`' supersedes relations between `Lines::version`s, by the line's author), heads, `green` (as `weave green --json`), goals with `binds`, open objections (as `weave review --json`'s `open`) | `Lines::line`, `current_versions`, `version`, `heads`, `open_objections`; `Checks::green`, `counted`; `reviews::goals`, `Goal::binds` |
| `/api/weave/log/<line>?after&limit` | decisions newest first, as `weave log --json` rows | `logs`; `Lines::decision` |
| `/api/weave/decision/<id>` | the `weave log` row, plus `verdict` (`qualifies`, or the rule's message), `policy` in force, `counted` per check, reviews | `Lines::decision`, `verdict`, `policy_in_force_for`; `Checks::counted`; `Reviews::of_decision` |
| `/api/weave/tree/<record>?dir&after&limit` | the record's manifest hash, and the direct children of `dir` (default the root): directories with the number of files below them, then files with mode, size, hash and `held` | `Index::manifest`, `file_size` (held means held at the entry's size); the manifest's sorted entries, by a range search on the prefix |
| `/api/weave/file/<record>?path` | `path`, `mode`, `size`, `hash`, `shown`: `text`, `symlink`, `binary`, `oversized` or `missing`; `text` only when shown, with `lines` | `Manifest::get`, `Index::file`, `file_size`, `diff::is_text` |
| `/api/weave/history/<line>?path&after&limit` | the line's decisions, newest first, where the path's file value differs from the decision's first parent on the line, each with `kind`, `before` and `after`. A decision with no parent on the line (the line's root, or a first decision a line merge started from another line's head) compares with its first parent on any line, or, with no parent, with an absent file | `logs`; `Lines::decision`; `Index::manifest`; `Manifest::get` |
| `/api/weave/diff?a&b&after&limit` | the per-file list from `a`'s tree to `b`'s: `path`, `kind`, `before`, `after` | `diff::manifests` |
| `/api/weave/diff?a&b&path` | that one file: `unified` text, or `shown` saying why not (`binary`, `oversized`, `too_large_to_diff`, `missing`) | `diff::unified`, `diff::is_text` |
| `/api/weave/proposals/<line>?status&after&limit` | proposals on the line, ascending by id, each with `status`, author, base, summary. `status` filters (`open`, `integrated`, `invalid`, `retracted`, `all`). No change is computed for a list: the change's size is on the proposal's page | `records`; `Lines::proposal`, `proposal_named`; `logs` and `Lines::decision` for what each decision lists, compared by the payloads' `change` hashes |
| `/api/weave/proposal/<id>?after&limit` | the proposal, its status and integrating decision with its note, its change entries (`Change::between`, one per request, first 2,000 and `total`), reviews, objections to answer, and for an open proposal the conflict preview, a page at a time | `Lines::proposal`, `proposal_objections`; `Reviews::of_proposal`; `format::Change::between`; *Conflict preview* below |
| `/api/weave/manifest/<hash>?line` | every check result on it (`Index::check_results` filtered by manifest), each with details (cut to 64 KiB), a box log link and validator mark; the counted results under `line`'s current policy (default the default line; none when the policy has diverged); its reviews and the objections to it; the decisions and proposals with this manifest | `Index::check_results`; `Checks::counted_on_manifest`; `Reviews::of_manifest`; `Lines::manifest_objections`; each evidence's `details` from `g` |
| `/api/weave/log-artifact/<id>` | a `text/plain` box log's text, cut to 64 KiB, shown only if it is UTF-8 without NUL; otherwise named with its size | `records`; `g`; `diff::is_text` |
| `/api/weave/record/<id or hash>` | `kind` and the page that shows it | `ground::kind_of`, `Index::hash_kind` |

- **What a tree shows.** A tree's record can be a decision (its manifest), a
  proposal (its manifest), a manifest artifact, or a content hash. A tree is
  shown only for a valid manifest. Its completeness is reported
  (`Index::missing`, as a count and the first 200 paths). A file this
  replica doesn't hold at its entry's size shows as `missing`.
- **Box logs.** A result's box log link is read as `checks.rs`'s `box_logs`
  reads it: from the details' run lines only, before the first blank line,
  the line `log <id> <hash>`. It is linked only if that artifact's content
  hashes as named.

### Conflict preview

The preview shows what `weave integrate --proposals <P>` would meet, without
running integrate's merge. It never calls `merge::resolve_lines`, which
line-merges every unresolved conflict and keeps every merged file. It
computes only this:

1. **When.** Only for an open, valid proposal on a line with exactly one
   head and one current policy. A forked line, or a diverged policy, gets
   `preview: null` with the reason. Integrate refuses there too.
2. **The merge.** `merge::three_way(base, head, proposal)`. `base` is the
   manifest of the proposal's own base decision, as integrate merges a
   proposal against its own base. `head` is the line head's manifest, and
   `proposal` is the proposal's. So no merge base is searched for. This is
   linear in the three manifests' entries, at most about 200,000 each. Its
   conflicts are paged by path, by `after` and `limit`, and the answer
   gives their `total`.
3. **Each conflict on the page.** The resolutions offered for its key
   (`Resolutions::for_key` on `Index::resolutions`), each with its author,
   result and whether that author integrates the line under its current
   policy. Then, within the budget, `merge::text_conflicts`: an empty list
   means its lines would merge cleanly, a non-empty one gives the base line
   ranges where they conflict, and `None` means it can't merge line by line
   (not text, absent on a side, a symlink, not held, or over
   `text_merge::MAX_DIFF_LINES`).
4. **The budget, per request.** At most 20 conflicts get `text_conflicts`.
   Their three sides add up to at most 16 MiB, and each side is at most the
   view's 1 MiB text cap, not `text_merge`'s 8 MiB. A conflict past the
   budget is listed with `lines: "not computed"`, and the next page computes
   the next 20. Nothing merged is kept: `text_conflicts` returns ranges, not
   bytes. So duplicate content hashes across many paths (one 8 MiB blob
   named at 1,000 paths) cost at most 20 diffs of 1 MiB sides a request,
   never 1,000 merges.
5. **Cached.** The answer for each (proposal, head, page) is kept in the
   state's `previews`, at most 64 entries, dropped with the snapshot, so a
   reload costs nothing.
6. **Slotted.** At most 2 previews build at once (section 4, Caps).

### Caps

| What | Cap | Why |
|---|---|---|
| A file's text shown | 1 MiB, and rendered 5,000 lines at a time | About 20,000 lines of code fit in 1 MiB, but a megabyte of newlines is a million lines. So the page builds the rows for 5,000 lines and adds the next 5,000 when asked, from the text it already holds. The answer gives `lines`, the count |
| Each side of a unified diff | 1 MiB, and `diff::MAX_DIFF_CELLS` | `diff::unified` builds a table of up to 16 million `u32`s (64 MB). The page renders its output 5,000 lines at a time, as for files |
| Unified diffs and conflict previews building at once | 2 each | Taken with a new non-blocking `Slots::try_take`. Today's `Slots::take` waits, which would hold a connection. A request past the slots gets a 503 at once, to retry |
| A conflict preview | 20 `text_conflicts` a request, sides at most 1 MiB each and 16 MiB in all | *Conflict preview* above |
| List pages | 200 by default, 2,000 at most | |
| Check details, box logs | 64 KiB, cut on a character boundary (`cut_bytes`) | The view's existing `TEXT_BYTES` |
| Problems, missing paths, change entries | 200 (each `Incomplete` with 20 paths), 200, 2,000, each with a `total` | |

The text cap is 1 MiB, for files and each side of a diff. The 64 KiB cap
applies only to check details and box logs.

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
- a history: one manifest lookup for each of the line's 157 decisions, from
  the cached log;
- a diff list: one pass over two manifests.

Only unified diffs and conflict previews do real work, within the caps
above.

**Memory.** Today `bracken view` holds two copies of the replica, and three
during a re-read:
- `main.rs` opens a `Store` and keeps it open while `view::serve` runs;
- `Viewer::new` opens its own;
- `Viewer::watch` opens a third while it re-reads.

PR (b) changes `Command::View` to hand its `Store` to `view::serve`, which
uses it as the first snapshot. That makes one copy, and two during a
re-read. The Weave state adds the decoded manifests, about 70 bytes a path
per distinct manifest, and indexes much smaller than the payload.

**At 10x** (about 33,000 contributions and 1.3 GB of payload; 1,570
decisions; an 8,300-file tree):
- **Memory:** about 1.3 GB, briefly about 2.6 GB during a re-read.
- **Rebuild:** `Index`, `Lines`, `Checks` and the per-line logs are
  near-linear in the records they index. The estimate is about 5 to 10 s,
  off the request path.
- **Requests:** a history is 1,570 lookups; a tree directory is a range
  search over 8,300 entries.

PR (b) measures the rebuild, the memory, and how long requests hold
`Mutex<Checks>`, on a generated 10x Weave Ground, and records the numbers.
If a rebuild passes 10 s or memory passes 4 GB, the builder stops and
reports rather than tune `crates/tree`. Past that size the answer is the
index store ([index-store.md](index-store.md)), not this view.

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
  The view never calls `materialize`, the check runner, `checkout`,
  `resolve --markers` or `merge::resolve_lines`. Its only file access is
  the guarded `weave-root` read (section 3).
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
  that view's service. Either for the Weave Ground would be the owner's own
  approval.
- **A gap to close.** Until PR (a), `scripts/weave` refuses only `sync` and
  `serve`, so `scripts/weave view --bind <any address>` and `--runs` pass
  through. PR (a) (PR #76) makes `scripts/weave` let `view` through with
  `--port`, the policy options, `--as` and help only, and refuse `--bind`,
  `--host`, `--runs` and anything else, with tests that fail on `main`.
  Its header says why. That PR is merge-gate tooling, a risky class, and
  gets an independent review and a security review.
- The pages reach the Weave Ground only once the manager upgrades the
  pinned binary past their merge (releases.md).

## 7. Security model

Every path, file name, file's content, note, summary, reason, goal, check
name, argv, output tail and log on a Weave Ground is author-controlled.

- **Nothing is rendered as HTML.** All of it leaves the server only as JSON
  string values, escaped by `json_bytes`, under the view's CSP (no inline
  script or style, no eval, `default-src 'none'`). The page inserts it only
  through `common.js`'s `h()` and `text()`, as text nodes. The existing test
  that the view's scripts never parse text as markup or code, and set no
  attribute outside `common.js`, covers `weave.js` as soon as it is in
  `ASSETS`. Line numbers, the tree and diff rows are built as elements,
  never as strings.
- **Author text reaches no attribute.** Navigation is by buttons that set
  `location.hash` (section 2). The only author-controlled text in a
  fragment is a path, encoded by `encodeURIComponent`, and it is decoded
  and looked up in a manifest, never inserted into anything but text. A
  `title` attribute set through `h()` (for a long path's full text) goes
  through `common.js`'s `plain()`.
- **Invisible characters are marked.** This covers every author-controlled
  string the page shows: paths, names, file text, notes, summaries,
  reasons, goals, argv, output tails and logs. It marks bidirectional
  controls (U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F, U+061C)
  and other invisible format characters (zero-width, U+FEFF, U+00AD, the
  tag block). So a file can't read differently from its bytes (Trojan
  Source, CVE-2021-42574).
  - `weave.js` turns each into a visible marker (`⟨U+202E⟩`) before it
    passes the string to `h()`. `common.js`'s `plain()` strips bidi
    controls, so marking afterwards would find none.
  - A path with control characters shows them as `\u{..}` escapes, as the
    CLI's `terminal::printable` does.
- **Paths never touch the file system.** A path is a key into a manifest
  held in memory (`Manifest::get`, or a range search on sorted entries),
  compared as bytes. It is never joined to a directory, opened, normalised,
  or used to choose a response type. A path that isn't in the manifest is a
  404. A record id is hex, resolved in the graph.
- **Served bytes have safe content types.** The view serves only its own
  embedded assets, with the types in `ASSETS`, and `application/json`.
  There is no endpoint that serves a file's or a log's raw bytes, so no
  author byte is ever a response body except inside a JSON string. A box
  log or a symlink target that isn't UTF-8 without NUL is named, not shown.
  Binary files are named, not shown or offered for download. Every response
  keeps:
  - `X-Content-Type-Options: nosniff`;
  - the CSP;
  - `Cross-Origin-Resource-Policy: same-origin`;
  - `X-Frame-Options: DENY`.
- **Bounded work.** Every list pages, every text is capped, and the
  expensive computations (unified diffs, conflict previews) have their
  inputs capped and their slots refused at once when full (section 4). The
  listener's connection budget, Host checking and timeouts are the view's
  own and unchanged. A hostile Weave Ground costs the view the same as its
  size, as it costs every reader.
- **Cross-site requests are refused on `/api/weave`.** CORS and the Host
  check already keep another site from *reading* a response. But a page in
  the reader's browser could still send blind `no-cors` GETs to the
  loopback port, with a valid Host, and hold the diff and preview slots.
  So a `/api/weave` request whose `Sec-Fetch-Site` is `cross-site` or
  `same-site` gets a 403. Browsers send that header on every request.
  Tools without a browser (curl, tests) send none, and are served. The
  Loom's endpoints keep today's behaviour.
- **Not secrets, but not published either.** Import refuses secrets
  (weave.md section 3), but a check's output tail or a box log could still
  hold something private about this machine. The view never serves the
  Weave Ground beyond loopback (section 6).
- **At the join.** After the join (weave.md section 5), the team Ground
  holds Weave records, so these pages appear on the team Ground's view,
  which owner decision `917816c72975` serves on NetBird. That would show
  every NetBird peer the source trees, check output tails and box logs.
  The join's own approval must name that, or the Weave pages must be
  served on loopback only by then. This design doesn't decide it.
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
| (a), #76 | `scripts/weave view`: `--port`, the policy options, `--as` and help only; `--bind`, `--host`, `--runs` and anything else refused; tests that fail on `main`; header and docs/cli.md | merge-gate tooling (risky) |
| (b) | The Weave state in the snapshot, with the guarded `weave-root` read and `signature()` watching it; `main.rs` handing its `Store` to the view; `/api/weave`, `line`, `log`, `decision`, `record`; the `Sec-Fetch-Site` refusal; the home, line and decision pages, with button navigation; the `index.html` nav line and the shared files in section 2. Its tests: <br>• the 10x measurement; <br>• a hostile Weave Ground's strings come back escaped; <br>• `view_weave.rs` names no file-system call but the `weave-root` read; <br>• a `weave-root` symlink or FIFO gives no root and doesn't block; <br>• a session of requests leaves the Ground byte for byte the same | cli |
| (c) | `tree` and `file`; the tree browser and file page, with line numbers rendered 5,000 at a time and invisible characters marked; tests for binary, oversized, symlink (text and not), missing and short-held files, and a megabyte of newlines in the Node check | cli |
| (d) | `history` and `diff`; the history and diff pages; tests at the caps, for a line's root and a first decision made by a line merge, and for the diff slots refusing at once | cli |
| (e) | `proposals` and `proposal`, with status and the conflict preview; the proposal pages. Its tests cover: <br>• each status; <br>• a forked line and a diverged policy getting no preview; <br>• a conflict with an offered resolution and one by an integrator; <br>• the preview budget, with 1,000 paths naming one blob: at most 20 computed, the rest `not computed`, paged; <br>• the cache | cli |
| (f) | `manifest` and `log-artifact`; check results by anyone, with details and box logs, goals and objections (open ones with their basis, others with the answers naming them); tests for counted, replaced, retracted and non-validator results, each objection basis, a diverged policy, and a non-UTF-8 log | cli |

Done when (a) to (f) are merged.

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
5. A proposal's conflicts are computed on request, in memory, capped and
   slotted, writing nothing (section 4, *Conflict preview*).
6. Reuse the CLI's JSON helpers as `pub(crate)`.
7. A one-line link to `/weave.html` in the Loom's nav (`index.html`), in PR
   (b), passed to the Loom's builder by the manager.
8. Invisible characters marked.
9. The 10x measurement and its stop thresholds.
10. The build plan above.

## 10. Round 1, and how it is handled

| Finding | Handling |
|---|---|
| `4d2c3775c999`, `cc5d437b1852`: `resolve_lines` uncapped; conflicts unpaged; `Change::between` per listed proposal | The preview never calls `resolve_lines`. It runs `three_way` once, then pages the conflicts, and runs `text_conflicts` (ranges, not merged bytes) on at most 20 a request, with sides at most 1 MiB and 16 MiB in all. It caches by proposal, head and page, and its slots refuse at once (section 4). The proposals list computes no change; a proposal's page computes one. PR (e) tests the 1,000-paths-one-blob case |
| `53fca000203f`: `weave-root` read unbounded on every rebuild | Read with `O_NOFOLLOW`, a regular-file check and 256 bytes, in the one file access the module has; `signature()` watches it; tested in PR (b) (section 3) |
| `f3ec9bbabd28`: a byte cap allows a million lines | Files and diffs render 5,000 lines at a time; PR (c)'s Node check uses a megabyte of newlines (section 4, Caps) |
| `eed6a83b1862`: answered objections' settling decision and basis aren't public; `Checks::on_manifest` holds validators' results only | Narrowed. The view shows open objections with their basis, and for the rest only what the payloads say, and names the missing `crates/tree` read as a later item outside this task. Check results come from `Index::check_results`, marked validator or not (section 1) |
| `d7bc1fd686e8`: `common.js` refuses `a` and `href`; shared files not named | Buttons that set `location.hash`, as `root.js` reads it; `common.js` unchanged; every shared file named, the `scripts == 11` assertion and `tests/view.rs` included (section 2) |
| `e7710acc020d`: three `Store`s during a re-read; `Lines::log` sorts the graph per request | `main.rs` hands its `Store` to the view, so one copy and two during a re-read; per-line logs cached in the state; figures corrected (sections 3 and 5) |

Notes taken: the `Sec-Fetch-Site` refusal, and what the join must approve
(security notes 1 and 2), are in section 7. Also from security:
- note 3: attribute wording;
- note 4: the preview merges against the proposal's own base, as integrate
  does;
- note 5: measuring `Mutex<Checks>`;
- note 6: invisible characters marked in all author text;
- note 8: non-UTF-8 logs.

From the reviewer:
- note 1: the "done when" line;
- note 2: the `retracted` filter, and no `Change::hash` in the list;
- note 3: version chains, and lines by id;
- note 4: held at the entry's size;
- note 5: non-UTF-8 targets and logs, and which text cap is meant;
- note 6: missing paths cut, no counted results on a diverged policy, log
  links from the run lines, and `manifest_objections`;
- note 7: forked and diverged lines, and history's first decisions;
- note 8: `try_take`, `signature()`, no index without a root, marking
  before `plain()`, and child names as `after`.

Security's note 7 (diff compare cost is cells times line length) is bounded
by the same cap and slots, and needs no change.
