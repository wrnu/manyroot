# The Weave view: design

Design (2026-11-23), task `51acc5b46820`, from the owner's request in
question `7e01a0dbf714`: "There could be a specific UI that's specific for
Weave ... We should even essentially be able to view the full source code
and all of that." The Weave view is a set of pages in `bracken view` for a
Ground that holds Weave records ([weave.md](weave.md)). It is separate from
the Loom and the other Ground pages ([visualizer.md](visualizer.md)). Its
open questions went to the manager as question `90dc7287b194`, and answer
`e490eaaaebb3` accepts every recommendation (section 9). Section 10 lists the
review findings and how this revision handles them.

It changes nothing in `crates/tree`, in manyroot's protocol files or in any
frozen format ([weave-freeze.md](weave-freeze.md)). It adds no dependency. It
never writes to any Ground, and it never writes a tree to disk.

## 1. What a reader can do

- **See the Weave Ground at a glance.** The counted marker and the root.
  Every current line, a page at a time, with its name, author, reach (`weave
  line list`) and number of decisions. The default line also shows its head
  or the heads of a fork, and its green state. Every other line shows those
  on its own page (section 4 says why). Also what `weave check` would list:
  - invalid payloads;
  - incomplete manifests;
  - extra markers;
  - ignored policy versions;
  - records this build can't read (`Lines::unreadable`).
- **Browse a line.** Its current policy: integrators, validators, goal
  setters, each check with its argv and definition hash, executors,
  guardrails, and the owner decision the version cites. Also the policy's
  versions, its heads, its green decision, its goals, its objections and its
  open proposals. Its decisions are listed newest first in causal order, a
  page at a time.
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
  it, by anyone (`Index::check_results`), a page at a time, with:
  - its author, marked if it is a validator under the line's current policy;
  - its check, pass or fail, and attempt;
  - whether it is retracted, and for a validator's result, whether a later
    attempt replaced it;
  - the first 4 KiB of its details (argv, exit and output tail). A result's
    own page has its details up to 64 KiB;
  - a box check's log, linked and readable as text.

  The page also shows the counted results under a line's current policy (as
  `weave green --manifest`), the reviews of the manifest, and the objections
  to integrating it unchanged (`Lines::manifest_objections`).
- **Goals and objections.** Goals show with whether they bind under the
  line's current policy. A line's objections page (`/api/weave/objections`,
  section 4) lists the line's objections, a page at a time. That includes
  objections to any decision in its heads' decision ancestry, whatever line
  that decision is on, as well as to its proposals and their manifests
  (section 4, *Which objections a line has*). *Open* objections (`Lines::open_objections`) show with their basis
  (evidence-backed, goal-backed or discussion, and `replaced`) and the
  dispositions they allow. Every other objection shows:
  - only what the Ground says: its author, note and target;
  - whether it is retracted now;
  - that it isn't open;
  - the decisions whose `answers` name it, or name its target, each with its
    disposition (each answer's reason is on that decision's page);
  - how many decisions on the line answer the whole line.

  The view doesn't say which decision *settled* an objection, or how an
  answered objection's basis was classified. `crates/tree` keeps both
  private (`Obligations`' settled map and its basis classification).
  Showing them needs a public read in `crates/tree`. That is a later item,
  outside this task, and as a change to `crates/tree` it needs the review
  and approvals AGENTS.md section 1 sets.
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
  after `encodeURIComponent`, in a fragment and in every API query, so a
  `&`, `=`, `+` or `%` in a path can't add a parameter or change its
  value. The page reads the route back on `hashchange` and checks it the
  same way before using it, as `root.js` does. `common.js` doesn't change.
- **Code layout.** A new module, `crates/bracken/src/view_weave.rs`, holds
  the Weave state and every `/api/weave` handler. The JSON reuses the CLI's
  `--json` shapes and field names (`weave log`, `head`, `green`, `review`,
  `line list`). The helpers that build them in `weave.rs` and
  `weave/review.rs` become `pub(crate)`. They aren't copied.
- **Shared files.** These are the files the plan touches that other pages
  or other builders use too:
  - `crates/bracken/src/view.rs`:
    - the Weave state in `Snapshot`;
    - one routing arm, and the new endpoints' parameters in the `takes`
      table;
    - `ASSETS` rows for `weave.html`, `weave.js` and `weave.css`;
    - `Sec-Fetch-Site` read in `read_request`, and its refusal on every
      `/api/` path, the Loom's included (section 7);
    - a non-blocking `Slots::try_take` (section 4, Caps);
    - `weave-root` added to `signature()` (section 3);
    - in the scripts-checking test, `assert_eq!(scripts, 11, ...)` raised
      to 13: that test counts pages and scripts, and `weave.html` and
      `weave.js` are one of each;
  - `crates/bracken/src/main.rs`: `Command::View` takes the replica's
    signature before it opens the `Store`, as the view's re-read does
    (view.rs, finding `31f5a44fc779`). It then hands both to `view::serve`,
    instead of keeping its `Store` open beside the view's own (section 5);
  - `crates/bracken/assets/view/index.html`: one nav line,
    `<a href="/weave.html">Weave</a>`, static markup beside the Loom,
    Trust, Tension and Runs links. That is the Loom's builder's file
    (task `b140c091be3d`); the manager passes the change to them;
  - `crates/bracken/tests/view.rs`: `/weave.html` added to the pages whose
    `src` and `href` must be served, and `/weave.css` to the stylesheets
    it reads; and a test for the `Sec-Fetch-Site` refusal on the Loom's
    endpoints.

  `common.js`, `visualizer.css` and the fonts are read, not changed.

## 3. The Weave state

The watcher thread (`Viewer::watch`) builds the Weave state, once per change,
never on the listener or on a request's thread (answer `a300d75086d5`):
- **At start,** the view listens and serves its other pages from the first
  snapshot at once. The watcher builds that snapshot's Weave state in the
  background, so the first visit usually finds it ready.
- **On a change,** the watcher drops the old snapshot's Weave state first,
  then reads the new snapshot, swaps it in, and builds its Weave state. So
  at most one Weave state is held at a time. A request that is still
  reading the old state holds a counted reference, and the state is freed
  when that request ends.
- **Until it is built,** every Weave endpoint answers 503 with
  `Retry-After: 2` and "reading the Weave records". `weave.js` waits that
  long and asks again, for up to ten minutes, while the reader stays on
  that page. Any other 503 it shows at once. The view's other pages keep
  answering throughout.
- **If the re-read fails** (a write half done elsewhere, say), the old
  snapshot keeps serving the other pages, and its Weave state stays
  dropped. It is never rebuilt from the old graph against what is on disk
  now. The Weave endpoints answer 503 with why. The same signature is
  tried again after 2 s, doubling up to 60 s, and a new signature at once.
  If the change is undone, so the signature is the old snapshot's again,
  its Weave state is built again from it.
- **If the build panics,** the watcher catches it. The Weave endpoints say
  so until the replica changes, and the rest of the view keeps
  refreshing.

Every request reads from that state. Nothing in it is built per line, so a
Ground with thousands of lines costs a rebuild what its records cost, not
that times the lines.

| Field | Built with | Notes |
|---|---|---|
| `root` | the replica's `weave-root`, read as below | No root, no Weave state: nothing else is built |
| `index` | `Index::build(g, root)` | |
| `lines` | `Lines::build(g, &index)` | Every line is judged, as `weave line list` does |
| `logs` | `lines::newest_first(g)` called **once**, then each qualifying decision put in its line's list by `Lines::verdict`'s `line` | The same order `Lines::log(g, line, None)` gives, without its whole-graph sort per line. No request calls `Lines::log` |
| `checks` | `Mutex<Checks>`, from `Checks::build(g, &index, &lines)` | `Checks` holds a `Cell` (its epoch-miss count), so it is `Send` but not `Sync`. A mutex shares it, so green and counted results are computed one request at a time. PR (b) measures how long it is held, now and at 10x |
| `reviews` | `Reviews::build(&index, &lines)` | |
| `records` | one pass over `g` | The ids of every proposal, resolution and `text/plain` artifact, and every `supersedes` relation between line versions (a version's chain, which `Lines` keeps private), by media type and kind. `Lines` lists only valid proposals per line, and `Resolutions` has no iterator |
| `answers` | one pass over every decision | Every decision answer, indexed by what it names, keyed as `crates/tree` resolves an answer (`Obligations`' keys): an objection id; a decision; a manifest artifact, by its manifest's hash; a proposal, by its line and change, so an answer to a rebased proposal with the same change is found too; or its own line. Each entry holds the decision and the disposition. Section 4's objections page reads it |
| `derived` | `Mutex<Cache>`, empty at build, filled on request | The answers of one line's or one record's reads that sort the graph (below), and conflict previews, dropped with the snapshot. Bounded by bytes, not entries: an entry is kept as its serialized JSON, and its size is that length. The cache holds at most 32 MiB in all, evicting the oldest first. An entry over 4 MiB isn't kept at all: that read is recomputed for every page, within its slot |

**Reads that sort the whole graph.** `Lines::open_objections`,
`proposal_objections` and `manifest_objections` each build an `Ancestry`,
which sorts the whole graph, on every call. `Checks::green` and
`Lines::heads` walk a line's decision history. So the view calls them only
for the one line or record a request names, never for every line.
- **What is kept.** `derived` keeps what the pages need of each read, never
  the read's whole result.
  - **For an objection read,** each objection in order with what its row
    needs that only that read gives: its id, and for an open one its basis
    (its kind; for `Basis::Evidence` the failing result, the check name cut
    to 800 bytes and `replaced`; for `Basis::Goal` the goal's id) and the
    dispositions it allows. That's at most about 1 KiB an objection; the
    row's other fields (author, note, retracted, answers) come from `Index`
    and the `answers` index for that page alone.
  - **For heads and green,** it is the line's answer.

  So a reload costs nothing, within the byte budget above. An objection
  read whose entry would pass 4 MiB isn't kept: each page of it is computed
  again from the `Lines` call, within its slot, and only that page's rows
  are built.
- **Slots.** At most 2 such reads run at once. A request past that gets a
  503 at once (section 4, Caps).

**Reading `weave-root`.** `weave::root` reads the file with an unbounded
`read_to_string`. `scripts/weave` refuses a `weave-root` that isn't a
regular file only once, when it starts. So the view doesn't call
`weave::root`. On each rebuild it opens the file itself, with these rules:
- open with `O_NOFOLLOW | O_NONBLOCK`;
- check on the open descriptor that it is a regular file;
- read at most 256 bytes;
- parse it as `weave::root` does.

A link, a FIFO or anything else gives no root. The view logs why and serves
no Weave pages until the file is fixed. `signature()` reads `weave-root`
with `symlink_metadata`, never following it. It counts its inode, size, and
modification and change times, so a `weave root set`, which renames a new
file into place, is seen at the next poll. This read is the only file access
in `view_weave.rs`, in one named function. The test in section 8 allows that
function and refuses any other.

If the state can't be had, the snapshot keeps the Loom's part and the Weave
endpoints answer 503 with the reason, as above. The view's other pages are
never lost to a Weave problem.

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

**Every list is paged.** This covers every list in every answer, whatever
its source: decisions, proposals, tree children, history, diff entries,
check results, reviews, goals, objections, answers, resolutions,
conflicts, problems, missing paths and change entries.
- **Inside an answer,** each list carries at most its first 50 items, with
  `total` and `more`.
- **Its next items** come from the same endpoint with `list=<name>`,
  `after` and `limit`. That answer holds only that list's page. `limit`
  defaults to 200 and is at most 2,000.
- **`after`** is the last key of the previous page: an id, or a path for
  path-ordered lists. In a tree it's a child's name, not its full path, so
  a request stays far under the 16 KiB head limit (`serve.rs`, `MAX_HEAD`).
  So a page stays the same as new records arrive.
- **A list inside a list item** carries at most 5 entries and its own
  `total`, with each entry's text cut to 200 characters (the view's
  `SHORT_CHARS`) and to 800 bytes in all, since 200 characters can take 800
  bytes of UTF-8. Its full list is on the item's own page or endpoint. This
  covers:
  - a decision row's `parents`, `proposals`, `resolutions`, `goals`,
    `answers` and `breakage` in a log;
  - a policy version's `integrators`, `validators`, `goal_setters`,
    `checks` and `guardrails.protected`, in `current` and `versions`;
  - a counted check's `passes`, `fails` and `replaced`;
  - an `Incomplete` problem's missing paths;
  - an objection's answers.

  An entry's own lists (a breakage entry's `paths`, a check's `argv`) are
  bounded the same way, 5 and their total, within the entry's 800 bytes.
  An endpoint whose items hold such lists (`/api/weave/log`, `list=problems`,
  `list=current`, `list=versions`, `list=counted`, `list=checks` and
  `/api/weave/objections`) takes `limit` up to 200, not 2,000.
- **What is never cut** (finding `80050786bf97`): a record id, a content
  hash, a check's name where it is the key a list pages after
  (`list=counted`, `list=checks`), and a row that is itself a key, such as
  a protected path in `list=protected` (at most 4 KiB, as a path is), or a
  tree child's name and path (finding `9c8b398bffe2`: a tree row is never
  cut, since its name is at most 255 bytes and its path at most 4 KiB).
  They are links and cursors. Only free text is cut, and
  arrays are bounded by count, never by cutting their strings.
- **A key is listed once.** A hand-made policy may name an agent or a path
  twice (`Line::check` allows it; the CLI never writes one). The view lists
  each entry of a policy's lists once, in order, so `after` names one item
  and paging can't loop (round-3 note 1 on PR #79).
  A row or answer with any string cut carries `"cut": true`, and the page
  says "(shortened)". The row's own page, or the CLI, has the rest.
- **So every answer is bounded by its limit times one row's cap, in
  bytes.** The view measures each row as serialized JSON. If it is over its
  cap, the view cuts the row's longest strings further until it fits.
  - A row holding nested lists is at most 24 KiB, so a page of 200 is at
    most about 4.7 MiB.
  - Any other row is at most 4 KiB, so a page of 2,000 is at most about
    8 MiB.
  - What is never cut isn't counted against these: an id is 64 bytes, and a
    check's name is as long as its policy makes it, since `Line::check`
    bounds neither. So a row is at most its cap plus its never-cut strings.
    A row whose never-cut strings alone pass its cap has its other strings
    cut to nothing, marked `cut`, and its own page has them (round-3 note
    2 on PR #79).

  These are bytes before `json_bytes`, which can write each one as up to 6
  bytes. So a page is at most about 28 MiB (200 nested rows) or 48 MiB
  (2,000 other rows) on the wire.

| Endpoint | Gives | Read with |
|---|---|---|
| `/api/weave?list&after&limit` | `weave`, marker, root; `lines` (as `weave line list --json`, each with its number of decisions from `logs`); the default line's `heads`, `forked` and green `state`; `problems` (each `Incomplete`'s `missing` as a nested list: 5 paths and its total); `unreadable`; `ignored_versions`. No other line's heads, green or objections: those sort or walk per line (section 3) | `Index::counted_marker`, `root`, `problems`; `Lines::lines`, `default_line`, `unreadable`, `ignored_versions`; `logs`; for the default line, `derived` |
| `/api/weave/line/<line>?list&after&limit` | the one current policy's `checks`, `integrators`, `validators`, `goal_setters` and `protected` in `policy`, each in full by `list=` (null when the policy has diverged: then each current version's lists are nested in `current`); current policy versions (several if diverged) with every policy field, every version of the chain (`records`' supersedes relations between `Lines::version`s, by the line's author), `heads`, `green` (as `weave green --json`), `goals` with `binds`, and the number of open objections: exactly the objections `/api/weave/objections/<line>?state=open` lists, which `Lines::open_objections` gives (*Which objections a line has*, below) | `Lines::line`, `current_versions`, `version`; `derived` for `heads`, `Checks::green` and `counted`, and `open_objections`; `reviews::goals`, `Goal::binds` |
| `/api/weave/objections/<line>?state&after&limit` | the line's objections (*Which objections a line has*, below), ascending by id, `limit` at most 200; `state` filters (`open`, `other`, `all`). Each has its author, its note (4 KiB) and whether it's retracted now (joined from `Index::reviews` by id). An open one also has its basis and allowed dispositions. Each has `answers`, a nested list of the decisions whose answers name the objection itself: 5 entries of decision and disposition, and the total. Answers that name its target or the line are the same for every objection to that target. So they come once per page, in `target_answers`, keyed by target (5 each, and the total), and `line_answers` is the line's count of line-wide answers. A decision's own page has each answer's full reason | `Lines::objections().list()` with each `Objection`'s target; `Lines::decision_ancestry`; `Index::reviews`; `answers`; `derived` for the line's objection ids and `open_objections` |
| `/api/weave/log/<line>?after&limit` | decisions newest first, as `weave log --json` rows | `logs`; `Lines::decision` |
| `/api/weave/decision/<id>?list&after&limit` | the `weave log` row, plus `verdict` (`qualifies`, or the rule's message), `policy` in force, `counted` per check, reviews, and `compared_with`: the parent its "what changed" diffs against, chosen as history chooses it (its first parent on its line, else its first parent; null for the line's root) | `Lines::decision`, `verdict`, `policy_in_force_for`; `Checks::counted`; `Reviews::of_decision` |
| `/api/weave/tree/<record>?dir&list&after&limit` | the record's manifest hash, its number of files, whether it's complete, and the direct children of `dir` (default the root; taken exactly as given, never normalised, so `src/` or `/src` is a 404): directories with the number of files below them, then files with mode, size, hash and `held`, paged by name (a directory's with a trailing `/`); and `missing`, the paths not held at their entry's size, paged | `Index::manifest`, `file_size` (held means held at the entry's size); the manifest's sorted entries, by a range search on the prefix |
| `/api/weave/file/<record>?path` | `path`, `mode`, `size`, `hash`, `shown`: `text`, `symlink`, `binary`, `oversized` or `missing`; `text` only when shown, with `lines`. A symlink's target is shown only if it is text; import refuses one that isn't, so only a hand-made manifest has one. The text is capped at 1 MiB, not the 64 KiB of a record's other text. For a decision, `decision` gives its `line` and the `parent` its file page diffs against (the first parent on the line, else the first parent; null for the line's root), so the page links to the history and to that diff | `Manifest::get`, `Index::file`, `file_size`, `diff::is_text` |
| `/api/weave/history/<line>?path&after&limit` | the line's decisions, newest first, where the path's file value differs from the decision's first parent on the line, each with `kind`, `before` and `after` (`hash` and octal `mode`, or null for absent), its author, the `parent` compared with and `parent_on_line`. A `path` is required. A decision with no parent on the line (the line's root, or a first decision a line merge started from another line's head) compares with its first parent on any line, or, with no parent, with an absent file | `logs`; `Lines::decision`; `Index::manifest`; `Manifest::get` |
| `/api/weave/diff?a&b&list&after&limit` | `a` and `b` as manifest hashes, and `files`: the per-file list from `a`'s tree to `b`'s, paged by path (`list=files`: `after` and `limit` without it are a 400, never ignored), each with `path`, `kind`, `before`, `after` and `not_held` (a side not held at its entry's size). `a` and `b` are any record with a tree, or `blake3:<hash>` of a manifest | `diff::manifests` |
| `/api/weave/diff?a&b&path` | that one file's `kind`, `before` and `after`, and `shown`: `text` with the `unified` text and its `lines`, or why not: `same`, `mode_only`, `missing`, `oversized`, `binary` (a side not text, or a symlink) or `too_large_to_diff`, checked in that order on `a`'s side then `b`'s. A path in neither tree is a 404; with `path`, `list`, `after` or `limit` is a 400. The unified text is kept in `derived` under the two hashes and the path, built in one of the 2 diff slots (section 4, Caps) | `diff::unified`, `diff::is_text` |
| `/api/weave/proposals/<line>?status&after&limit` | proposals on the line, ascending by id, each with `status`, author, base, summary. `status` filters (`open`, `integrated`, `invalid`, `retracted`, `all`). No change is computed for a list: the change's size is on the proposal's page. A status is `retracted` or `invalid` first, as views ignore those, then `integrated` or `open`; `decision` names the decision that integrated the proposal's change whatever its status, the oldest on the line | `records`; `Lines::proposal`, `proposal_named`; `logs` and `Lines::decision` for what each decision lists, compared by the payloads' `change` hashes |
| `/api/weave/proposal/<id>?list&after&limit` | the proposal, its status and integrating decision with its note (`integrated_by`), its change entries (`Change::between`, one per request; `change_held` false when its base's tree or its own isn't held), reviews, objections to answer (null with no single current policy), and for an open proposal the conflict preview, or null with `preview_why`. The preview's conflicts page by path with `list=conflicts` (20 at most, the default), each with `base`, `into` (the line head's side), `from` (the proposal's), `lines`, `ranges` (50 and `ranges_total`), the first 10 `resolutions` with `resolutions_total`, and `settled_by`, the integrator's resolution integrate would apply. A conflict's resolutions page with `list=resolutions&path=<path>`; `path` goes with nothing else | `Lines::proposal`; `derived` for `proposal_objections` and the preview; `Reviews::of_proposal`; `format::Change::between`; *Conflict preview* below |
| `/api/weave/manifest/<hash>?line&list&after&limit` | its check results by anyone (`Index::check_results` for that manifest, ascending by id), each with its first 4 KiB of details, a box log link and validator mark; the counted results under `line`'s current policy (default the default line; none when the policy has diverged); its reviews and the objections to it; the decisions and proposals with this manifest | `Index::check_results`; `Checks::counted_on_manifest`, and `Checks::latest` for `replaced` on a validator's result; `Reviews::of_manifest`; `derived` for `manifest_objections`; each evidence's `details` from `g` |
| `/api/weave/result/<id>` | one check result, with its details up to 64 KiB | `Index::check_results`; `g` |
| `/api/weave/log-artifact/<id>` | a `text/plain` box log's text, cut to 64 KiB, shown only if it is UTF-8 without NUL; otherwise named with its size | `records`; `g`; `diff::is_text` |
| `/api/weave/record/<id or hash>` | `kind` and the page that shows it (`decision`, `line`, `proposal` or `tree`) | `ground::kind_of`, `Index::hash_kind` |

- **What a tree shows.** A tree's record can be a decision (its manifest), a
  proposal (its manifest), a manifest artifact, or a content hash. A tree is
  shown only for a valid manifest. Its completeness is reported
  (`Index::missing`, as a count and the first paths, paged). A file this
  replica doesn't hold at its entry's size shows as `missing`.
- **Box logs.** A result's box log link is read as `checks.rs`'s `box_logs`
  reads it: from the details' run lines only, before the first blank line,
  the line `log <id> <hash>`. It is linked only if that artifact's content
  hashes as named.
- **Which objections a line has.** One scope, used by the line page's count
  and by the objections endpoint alike. It is the past rule 5 reads for the
  line's next move (weave.md section 2), widened to what a move could use.
  An objection belongs to a line when its target is one of these:
  - a decision in the decision ancestry of the line's heads, on any line
    (`Lines::decision_ancestry`, the heads included). A line that branched
    from or merged another line's decision therefore lists objections to
    that decision, as `Lines::open_objections` counts them (finding
    `a57dc4619c24`);
  - a proposal on that line, by its payload's `line`;
  - a manifest that one of those decisions, or a proposal on that line, has.

  `state=open` is exactly `Lines::open_objections` for the line, every one
  of which is in that scope. `state=other` is the rest of the scope. So the
  count on the line page and the open list always agree. An objection to a
  manifest two lines share is listed on both. Its target resolves whether
  or not the target is retracted, as weave.md section 1 says.
  `Obligations::list` includes retracted objections, so the view joins
  each one to `Index::reviews` by id and marks it. For a line, `derived`
  keeps each objection in the scope with what only the objection read
  gives: its id, and for an open one its basis and allowed dispositions
  (section 3, *What is kept*).

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
   conflicts are paged by path, at most **20 a page**, with their `total`.
3. **Each conflict on the page.**
   - **Resolutions offered** for its key (`Resolutions::for_key` on
     `Index::resolutions`): the first 10, with `total`. Each has its
     author, its result, and whether that author integrates the line under
     its current policy. The rest page with `list=resolutions&path=<path>`.
   - **Then its lines**, with `merge::merge_mode` and
     `merge::text_conflicts`:
     - `clean` when the modes merge and `text_conflicts` is empty;
     - `conflicts` with the base line ranges when `text_conflicts` gives
       some;
     - `mode` when only the modes don't merge;
     - `not_text` when `text_conflicts` is `None`: not text, absent on a
       side, a symlink, not held, or over `text_merge::MAX_DIFF_LINES`;
     - `over_view_cap` when a side is over the view's 1 MiB cap. Integrate
       itself line-merges sides up to 8 MiB, so this one says only that
       the view didn't compute it, never that it can't merge.
4. **The budget, per page.** The sides the page's conflicts read add up
   to at most 16 MiB. A page stops before the conflict that would pass that,
   with `more`, and the next page starts from it. A conflict alone reads at
   most 3 MiB, so the first on a page is always computed, and none is left
   `not_computed`. A side over the view's cap, and a conflict whose sides
   aren't all there, read nothing. Nothing merged is kept: `text_conflicts`
   returns ranges, not bytes. So duplicate content hashes across many paths
   (one 8 MiB blob named at 1,000 paths) cost at most 20 diffs of 1 MiB
   sides a request, never 1,000 merges.
5. **Cached.** Each page's answer is kept in `derived`, keyed by proposal,
   head and page, holding only what it returned: at most 20 conflicts, each
   with at most 10 resolutions.
6. **Slotted.** At most 2 previews build at once (section 4, Caps).

### Caps

| What | Cap | Why |
|---|---|---|
| A file's text shown | 1 MiB, and rendered 5,000 lines at a time | About 20,000 lines of code fit in 1 MiB, but a megabyte of newlines is a million lines. So the page builds the rows for 5,000 lines and adds the next 5,000 when asked, from the text it already holds. The answer gives `lines`, the count. A symlink's target is capped the same way, and over it is `oversized`, before its bytes are scanned |
| A tree child's row | Not bounded by the row caps, since its name and path are never cut, but by `MAX_PATH_BYTES` (4 KiB) and the 255-byte name: a path of control characters, each written `\u00XX`, makes a row of about 26 KiB | A page of 2,000 such rows by hand is about 52 MB; the page asks for 50 at a time, about 1.3 MB (security note 1 on PR #81 round 2) |
| Invisible characters marked as elements | 256 a string, and 2,000 a batch of 5,000 lines | Past that, each is written `[U+XXXX]` in the text after one span saying how many, so a megabyte of control characters is a few hundred elements, not a million (finding `3c69d509671e`). The same cap as `common.js`'s `B.shown` in PR #80 |
| Each side of a unified diff | 1 MiB, and `diff::MAX_DIFF_CELLS` | `diff::unified` builds a table of up to 16 million `u32`s (64 MB). The page renders its output 5,000 lines at a time, as for files |
| Work building at once | 2 unified diffs, 2 conflict previews, 2 graph-sorting reads (section 3) | Taken with a new non-blocking `Slots::try_take`. Today's `Slots::take` waits, which would hold a connection. A request past the slots gets a 503 at once, to retry |
| A conflict preview | 20 conflicts a page, 10 resolutions each, sides at most 1 MiB each and 16 MiB in all | *Conflict preview* above |
| Lists | 50 inside an answer; pages of 200 by default, 2,000 at most, or 200 at most for an endpoint whose rows hold nested lists | *Every list is paged* above |
| A list inside a list item | 5 entries and its total, each entry's text 200 characters and 800 bytes | The row's own page has the rest |
| A list row | 24 KiB with nested lists, 4 KiB otherwise, as serialized JSON; a tree child's row is the exception, below | *Every list is paged* above. A flat row's own text can be up to 4 KiB a string, so a row with a long note or a check result's full 4 KiB of details is trimmed a little further, its longest strings first, to fit. Its own page has the text in full, up to 64 KiB |
| A check result's details | 4 KiB in a list, 64 KiB on `/api/weave/result/<id>` | 64 KiB is the view's existing `TEXT_BYTES`. A list of 2,000 results is at most 8 MiB of details |
| A box log | 64 KiB | |
| Notes, summaries, reasons, goals and other short text in a list | 4 KiB each | A record's own page has its text up to 64 KiB |
| Answers per objection | 5 that name it, with the total; target-level answers once per page, 5 per target | Deduplicated, so one target's answers aren't repeated on every objection to it |
| `derived` | 32 MiB in all, 4 MiB an entry, by serialized size | Section 3 |

The text cap is 1 MiB, for files and each side of a diff. The 64 KiB and
4 KiB caps apply to the other text, as listed above. Text is cut on a
character boundary (`cut_bytes`).

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

**Rebuild.** It is one pass each for `Index`, `Lines`, `Checks`, `Reviews`,
`records` and `answers`, plus one `newest_first` sort for every line's log
together. It is never per line: a hostile Ground with 5,000 lines costs a
rebuild one sort, not 5,000.

**Requests** then cost:
- a tree directory: a range search over at most about 830 entries;
- a file: one map lookup and the JSON escape of at most 1 MiB;
- a history: one manifest lookup for each of the line's 157 decisions, from
  `logs`;
- a diff list: one pass over two manifests;
- a line's heads, green or objections: one walk or one graph sort for that
  line, slotted, then cached in `derived`.

Only unified diffs, conflict previews and those per-line reads do real work,
and they are capped, slotted and cached.

**Memory.** Today `bracken view` holds two copies of the replica, and three
during a re-read:
- `main.rs` opens a `Store` and keeps it open while `view::serve` runs;
- `Viewer::new` opens its own;
- `Viewer::watch` opens a third while it re-reads.

PR (b) changes `Command::View` to hand its `Store` to `view::serve`, which
uses it as the first snapshot. That makes one copy, and two during a
re-read. The Weave state adds the decoded manifests, about 70 bytes a path
per distinct manifest, and indexes much smaller than the payload.

**The Loom's `as_of` graphs.** The Loom's `?as_of=` reads
(`/api/tension`, `/api/root/<id>`) keep up to 4 past graphs and build up to
3 at once (`PAST_GRAPHS`, `PAST_BUILDS`). Each holds a copy of the replica
as of a cursor. So the view's worst case is 1 copy, plus 1 during a
re-read, plus 4 kept and 3 building as-of copies: 9 copies. The Weave pages
use no `as_of` read. Only a reader who opens those Loom views on the Weave
Ground pays for them. A blind cross-site request can't, since section 7
refuses those on every `/api/` path.

**At 10x** (about 33,000 contributions and 1.3 GB of payload; 1,570
decisions; an 8,300-file tree):
- **Memory, steady:** about 1.3 GB, and about 2.6 GB during a re-read. If a
  reader uses the Loom's `as_of` views too, each past graph adds up to
  1.3 GB.
- **Rebuild:** near-linear in the records indexed, one sort for all logs.
  The estimate is about 5 to 10 s, off the request path.
- **Requests:** a history is 1,570 lookups; a tree directory is a range
  search over 8,300 entries; a per-line read is one sort of 33,000
  contributions.

PR (b) measures these on a generated 10x Weave Ground and records the
numbers:
- the rebuild;
- memory steady and during a re-read;
- memory with the 4 `as_of` graphs kept;
- how long requests hold `Mutex<Checks>`;
- each per-line read.

The 4 GB stop applies to the steady state with a re-read. If that passes
4 GB, or a rebuild passes 10 s, the builder stops and reports rather than
tune `crates/tree`.

**Measured** (notes `8d2fa97daa7c` and `b81cf1bbcb74`). The generated
Ground reached 420 decisions on an 8,300-file tree (371 MB) before the
2-hour limit on the generator. Every figure was taken at a load average of
250 to 830 on a 10-core machine, so each overstates an idle one.
- **When the Weave state was built with the snapshot:** 6.4 s to listen,
  544 MiB steady and 1,042 MiB at the peak of a re-read. Linear from 20 to
  420 decisions, that is about 22 s and 3.9 GB at 1,570. That's past the
  10 s stop, which is why the state is now built lazily (above).
- **Built lazily** (the same copy, measured once at the end): 0.8 s to
  listen, with `/api/ground` answering at once; 4.1 s until the first Weave
  page answers; 543 MiB steady. On a change, the Weave pages answer again
  4.3 s later, and the peak is 588 MiB, since the old Weave state is
  dropped first. At 1,570 decisions that is about 3 s to listen, about
  15 s until the first Weave page, and about 2.2 GB at the peak.
- **What remains is `Index::build`** in the frozen `crates/tree`, which
  decodes and checks every decision's full manifest. On the same copy,
  `weave log --json` took 4.4 s and 548 MiB, and `weave green --json`
  8.8 s. Every weave CLI read pays the same, so the view's Weave state costs
  about one CLI read per change. Decoding manifests once, or lazily, is the
  held "Weave single-decode" item (owner decision `817afe6653a8`), with this
  measurement as its evidence (note `357e72db08ca`).
- **Requests,** once the state is built, answer in 1 to 5 ms: the summary,
  a line, a decision with `Mutex<Checks>` held, and a 200-row log page of
  129 KB. A per-line read (heads, green and the open count, computed once
  and kept) took about 4 ms. `Mutex<Checks>` was held under 1 ms for a
  decision's counted results.
- **The Loom's `as_of` graphs,** reported beside the stop: 4 kept took the
  view from 522 to 1,033 MiB, about 128 MiB each, and each took 0.1 to 0.2 s
  to build. At 1,570 decisions that is about 480 MiB each, so a reader who
  opens 4 of them adds about 1.9 GB. The `as_of` figure is reported beside it for the
manager. Past that size the answer is the index store
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
  fragment or a query is a path, encoded by `encodeURIComponent`. It is
  decoded and looked up in a manifest, and only ever inserted as text. A
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
  expensive computations (unified diffs, conflict previews, per-line graph
  reads) have their inputs capped, are cached, and have their slots refused
  at once when full (sections 3 and 4). Nothing is computed per line for
  all lines. The listener's connection budget, Host checking and timeouts
  are the view's own and unchanged. A hostile Weave Ground costs the view
  the same as its size, as it costs every reader.
- **Cross-site requests are refused on every `/api/` path.** CORS and the
  Host check already keep another site from *reading* a response. But a
  page in the reader's browser could still send blind `no-cors` GETs to the
  loopback port, with a valid Host:
  - to hold the Weave slots;
  - to make the Loom's `as_of` reads build past graphs, each a copy of the
    replica (section 5).

  So any `/api/` request whose `Sec-Fetch-Site` is `cross-site` or
  `same-site` gets a 403, the Loom's endpoints and the event stream
  included. The view's own pages send `same-origin`, or `none` when a URL
  is typed. Browsers send the header on every request. A tool without a
  browser (curl, the tests) sends none, and is served, as is an old browser
  that doesn't send it, which is today's behaviour for every reader. The
  embedded pages and assets themselves aren't refused, since they carry no
  Ground data and do no work. This touches the Loom's endpoints in
  `view.rs`, so it is listed among the shared changes (section 2), with
  its own test in `tests/view.rs`.
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
| (b) | Gives: <br>• the Weave state in the snapshot, with the guarded `weave-root` read and `signature()` watching it; <br>• `main.rs` taking the signature first and handing its `Store` to the view; <br>• `/api/weave`, `line`, `log`, `decision`, `record`, with list paging; <br>• `Slots::try_take` and `derived`; <br>• the `Sec-Fetch-Site` refusal on every `/api/` path; <br>• the home, line and decision pages, with button navigation; <br>• the `index.html` nav line and the shared files in section 2. <br>Its tests: <br>• the 10x measurement in section 5; <br>• a hostile Weave Ground's strings come back escaped; <br>• `view_weave.rs` names no file-system call but the `weave-root` read; <br>• a `weave-root` symlink or FIFO gives no root and doesn't block; <br>• a session of requests leaves the Ground byte for byte the same; <br>• a Ground of many lines costs one log sort; <br>• cross-site requests refused on the Weave and the Loom endpoints, and served without the header | cli |
| (c) | `tree` and `file`; the tree browser and file page, with line numbers rendered 5,000 at a time and invisible characters marked; tests for binary, oversized, symlink (text and not), missing and short-held files, and a megabyte of newlines in the Node check | cli |
| (d) | `history` and `diff`; the history and diff pages; tests at the caps, for a line's root and a first decision made by a line merge, and for the diff slots refusing at once | cli |
| (e) | `proposals` and `proposal`, with status and the conflict preview; the proposal pages. Its tests cover: <br>• each status; <br>• a forked line and a diverged policy getting no preview; <br>• a conflict with an offered resolution and one by an integrator, and 11 resolutions on one key paging; <br>• a mode-only conflict; <br>• the preview budget, with 1,000 paths naming one blob: at most 20 a page, the rest paged; <br>• a side over the view's cap; <br>• the cache | cli |
| (f) | `objections`, `manifest`, `result` and `log-artifact`; check results by anyone, with details and box logs, goals and objections; tests for: <br>• counted, replaced, retracted and non-validator results; <br>• each objection basis; <br>• a retracted objection; <br>• an objection answered by name, by target and line-wide; <br>• a diverged policy; <br>• 2,001 results on one manifest paging; <br>• objections to a decision another line branched from, counted and listed alike; <br>• many lines sharing one manifest with many objections to it, holding `derived` within its byte budget; <br>• a non-UTF-8 log | cli |

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

## 10. Review findings, and how they are handled

### Round 1

| Finding | Handling |
|---|---|
| `4d2c3775c999`, `cc5d437b1852`: `resolve_lines` uncapped; conflicts unpaged; `Change::between` per listed proposal | The preview never calls `resolve_lines`. It runs `three_way` once, then pages the conflicts, and runs `text_conflicts` (ranges, not merged bytes) on at most 20 a page, with sides at most 1 MiB and 16 MiB in all. It caches by proposal, head and page, and its slots refuse at once (section 4). The proposals list computes no change; a proposal's page computes one. PR (e) tests the 1,000-paths-one-blob case |
| `53fca000203f`: `weave-root` read unbounded on every rebuild | Read with `O_NOFOLLOW`, a regular-file check and 256 bytes, in the one file access the module has; `signature()` watches it; tested in PR (b) (section 3) |
| `f3ec9bbabd28`: a byte cap allows a million lines | Files and diffs render 5,000 lines at a time; PR (c)'s Node check uses a megabyte of newlines (section 4, Caps) |
| `eed6a83b1862`: answered objections' settling decision and basis aren't public; `Checks::on_manifest` holds validators' results only | Narrowed. The view shows open objections with their basis, and for the rest only what the payloads say, and names the missing `crates/tree` read as a later item outside this task. Check results come from `Index::check_results`, marked validator or not (section 1) |
| `d7bc1fd686e8`: `common.js` refuses `a` and `href`; shared files not named | Buttons that set `location.hash`, as `root.js` reads it; `common.js` unchanged; every shared file named, the scripts count and `tests/view.rs` included (section 2) |
| `e7710acc020d`: three `Store`s during a re-read; `Lines::log` sorts the graph per request | `main.rs` hands its `Store` to the view, so one copy and two during a re-read; logs built once (sections 3 and 5) |

Round 1's notes were taken as listed in the previous revision of this
section:
- security's `Sec-Fetch-Site` refusal, now on every `/api/` path, and what
  the join must approve;
- the merge base;
- measuring the checks mutex;
- marking invisible characters in all author text;
- non-UTF-8 logs;
- the reviewer's notes 1 to 8.

### Round 2

| Finding | Handling |
|---|---|
| `529a929f53e2`: results, reviews, goals, objections and preview resolutions unpaged | Every list in every answer is paged: 50 inside an answer, the rest by `list`, `after` and `limit`. Each item's text is capped (4 KiB in lists, 64 KiB on a record's own page, `/api/weave/result/<id>` for a result's details). A preview caches only its page: 20 conflicts, 10 resolutions each (section 4) |
| `817bba809548`: `as_of` copies left out of memory; the Loom's `as_of` open to blind cross-site requests | Section 5 counts them: 9 copies at worst, measured in PR (b) and reported beside the 4 GB stop. The `Sec-Fetch-Site` refusal covers every `/api/` path, the Loom's included, and is listed as a shared change with its own test (sections 2 and 7) |
| `7b26f824e133`: non-open objections promised but not served; `Obligations::list` includes retracted ones | A paged `/api/weave/objections/<line>` serves them. Section 4 says which objections a line has, joins each to `Index::reviews` for its note and whether it's retracted, and caps the answering decisions it lists from the `answers` index. Section 1 promises only that, and still nothing that needs `crates/tree` |
| `5151a25507b9`: `Lines::log` and the objection reads sort the whole graph per call | Logs come from one `newest_first` per rebuild, bucketed by verdict line. Heads, green and objections are computed only for the line or record a request names, slotted and cached, and never for every line; the home page shows them for the default line only (sections 3, 4 and 5) |

### Round 3

Decision `057c4e8dace6`: these three only, then a delta check.

| Finding | Handling |
|---|---|
| `a57dc4619c24`: the line page's count and the objections endpoint scope a line differently; proposal answers not keyed by line and change | One scope for both: objections to the decision ancestry of the line's heads on any line, the line's proposals, and their manifests. `state=open` is exactly `Lines::open_objections`. The `answers` index keys a proposal answer by line and change, as `crates/tree` does (sections 3 and 4) |
| `a011a2d353e1`: the objections endpoint's limit times nested answers breaks the bound | Nested lists carry 5 entries of 200 characters and a total. Endpoints with nested rows take `limit` up to 200. Target-level answers come once per page. A page is bounded by its limit times one row's cap, stated in numbers (section 4) |
| `63bb4c9c3cdd`: `derived` bounded by entries | 32 MiB in all and 4 MiB an entry, by serialized size, oldest evicted. Objection reads keep ids and a total, never the whole `Vec<Open>`; a larger read is recomputed per page within its slot (section 3) |

### Delta check after round 3

| Finding | Handling |
|---|---|
| `c7df2ff6b188`: an open objection's basis and allowed dispositions aren't in `Index`, so ids alone can't build its row | `derived` keeps each listed objection's basis and allowed dispositions with its id (about 1 KiB each). An entry over 4 MiB isn't kept, and each page is computed again from the `Lines` call within its slot (section 3) |

Notes taken:
- section 1 states the widened scope, and that reasons are on each
  decision's page;
- breakage `paths` follow the nested cap;
- row and page bounds are stated in bytes: 24 KiB and 4 KiB rows, 800
  bytes a nested entry.

### PR (b)

| What | Handling |
|---|---|
| Manager notes after the delta check: section 4 still said ids and a total; `Basis::Goal`'s goal id; the 4 KiB flat row trims | Section 4 points to section 3's *What is kept*; the goal id is named; the caps table says how a flat row is trimmed |
| The 10x rebuild passed 10 s (question `91abf8289969`) | Answer `a300d75086d5`, option 1: the state is built lazily on the watcher thread, the old one dropped first, the Weave pages answer 503 with `Retry-After` until it's ready, and it's built at start in the background. The numbers are in section 5 |
| PR #79 round 1, `9f37a1b9146c`: a failed re-read rebuilt the Weave state from the old graph every poll | It stays dropped and the Weave pages say why; retries back off; a panic in the build is caught (section 3) |
| PR #79 round 1, `80050786bf97`: row caps cut ids and cursors | Ids, hashes and paged check names are never cut; arrays are bounded by count; cut rows are marked; the line pages its current policy's lists (section 4) |
| PR #79 round 2, `c50605c13cd4`: the Weave pages stayed 503 after a failed change was undone | The old snapshot's Weave state is built again (section 3) |
| PR #79 round 2, `886fba8945e1`: a protected path near 4 KiB was cut, breaking its cursor | A row that is itself a key is never cut (section 4) |
| PR #79 round 3, notes on the builder's rationales (`2be6896ef4ff`), taken in PR (c) | Note 1: repeated policy entries are listed once, so paging can't loop. Note 2: section 4 states the row bound with never-cut check names. Note 3: the recovery after a failed re-read relies on `signature()`'s append-only counts, as every re-read does; a same-count replacement in `objects/` or `agents/` isn't seen until the next change. That's the Ground's append-only rule, and unchanged here |

### PR (e)

| What | Handling |
|---|---|
| The design's row | `tests/view_weave.rs`: each status, filtered; a fork (a second decision from the base, written by hand) and a diverged policy getting no preview; an integrator's resolution and another's, 11 on one key paging; a mode-only change against a change of text; 1,000 conflicts naming one blob, 20 a page; a side over the view's cap; the cache, answering while both slots are taken |
| A page past 16 MiB of sides | The page stops before the conflict, which starts the next page, rather than listing it as `not_computed` (section 4, *Conflict preview*, 4). 17 conflicts with a 1 MiB side give pages of 15 and 2 |
| `mode` | Kept as the design names it, though with only regular and executable files among text sides, two changed modes always merge, so it doesn't arise from import's trees |
| The preview slots' test | A hook, `BRACKEN_VIEW_PREVIEW_DELAY_MS` (at most 2 s), holds a preview page in its slot, as the diff hook does |
| `records` | The state keeps every proposal artifact that decodes by its line, ascending, retracted and invalid ones included; resolutions come from `Index::resolutions` |

### PR (d)

| What | Handling |
|---|---|
| The design's row: tests at the caps | `tests/view_weave.rs`: oversized, binary, a symlink, `too_large_to_diff` (5,000 against 4,000 differing lines), `missing` from a hand-made manifest; history from a line's root, and on a line whose first decision a line merge made |
| The diff slots refusing at once | Unified diffs take their own 2 slots, apart from the per-line reads, so a diff can't hold a line page waiting. A test hook, `BRACKEN_VIEW_DIFF_DELAY_MS` (at most 2 s), holds a diff in its slot; three at once give one 503 |
| A removed line that starts `--` reads as `---` | The page classes `---` and `+++` as the head only before the first hunk |
| PR #83 round 1, `fb21c13db9b1`: the diff list ignored `after` and `limit` without `list=files`, and the page relied on it | The list checks its paging as every list does; the page's first request takes the 50 in the answer, and its next pages name `list=files` |
| PR #83 round 1, `81ffe9a422c4`: `not_held` ignored the entry's size | It's judged as the tree and one file's diff judge it: held at the entry's size |
| PR #83 round 1, security N1 and independent note 3: a merge's "what changed" used its first parent by id | The decision answer's `compared_with` is the parent history and the file page compare with |
| PR #83 round 1, independent note 2: an empty file added or deleted has no hunk, so its head was classed as changes | With no hunk, every line is head |
| PR #83 round 1, independent note 1: the delay hook's cap was untested | A test sets 600,000 ms and gets the diff within 10 s; with no delay set, nothing sleeps |

Round 2's notes are taken too:
- the scripts count rises to 13;
- `encodeURIComponent` in queries as well as fragments;
- a side over the view's cap is `over_view_cap`, never "can't merge";
- browsers without `Sec-Fetch-Site` are served;
- `/weave.css` joins `tests/view.rs`' stylesheet list;
- the signature is taken before the `Store` opens;
- `derived` is behind a mutex, and preview pages are at most 20 conflicts,
  so none is skipped;
- `signature()` uses `symlink_metadata`, with the inode and change time;
- `replaced` applies to validators' results, and results are paged;
- `merge_mode` is checked beside `text_conflicts`.
