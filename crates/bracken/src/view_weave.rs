//! The Weave pages' API in `bracken view` (docs/design/weave-view.md): `/api/weave` and its endpoints, for a
//! replica that holds a counted Weave marker.
//!
//! The Weave state is built once per snapshot, by the watcher, off the request path (section 3). Nothing in
//! it is built per line: the decision logs come from one `newest_first` sort for every line together. The
//! reads that sort the whole graph or walk a line's history (`heads`, `green`, the objection reads) run only
//! for the one line a request names, at most [`DERIVED_AT_ONCE`] at a time, and their answers are cached in
//! the state, so a reload costs nothing.
//!
//! Read-only, like the rest of the view: every answer is computed in memory from the snapshot. The one file
//! this module reads is the replica's `weave-root`, through [`read_root`], which never follows a link, never
//! blocks on a FIFO and reads at most [`ROOT_BYTES`]. A test in this file checks that nothing else here names
//! a file-system call.
//!
//! Every list in every answer is paged (section 4): an answer carries at most [`IN_ANSWER`] items of each
//! list, with its `total` and `more`, and the same endpoint with `list=<name>`, `after` and `limit` gives
//! the next ones. Every string is cut to a cap: [`LIST_TEXT`] in a list item, [`PAGE_TEXT`] in a record's
//! own answer.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use bracken_tree::ContentHash;
use bracken_tree::FileValue;
use bracken_tree::diff::Kind;
use bracken_tree::format::{Change, Decision, Entry, Manifest, Mode, PROPOSAL_MEDIA_TYPE};
use bracken_tree::ground::{Index, kind_of};
use bracken_tree::lines::{Lines, newest_first};
use bracken_tree::merge::{Conflict, Key, merge_mode, text_conflicts, three_way};
use bracken_tree::results::Checks;
use bracken_tree::reviews::{Reviews, Target, goals};
use manyroot::{AgentId, Body, ContributionId, Graph, RelationKind};
use serde_json::{Map, Value, json};

use crate::Names;
use crate::view::Slots;
use crate::weave::{
    ListedLine, counted_json, decision_json, file_value_json, green_json, line_row_json, listed_lines, problem_json,
    review_json,
};

/// The most bytes of `weave-root` read: an AgentId in hex is 64, and a newline.
pub(crate) const ROOT_BYTES: usize = 256;
/// The most items of each list an answer carries; the rest are paged with `list`.
const IN_ANSWER: usize = 50;
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 2_000;
/// A string in a list item, and in a record's own answer, is cut to these.
const LIST_TEXT: usize = 4 * 1024;
const PAGE_TEXT: usize = 64 * 1024;
/// What the derived answers kept per snapshot may hold, by their serialized size, and the most one may
/// hold to be kept at all; a larger one is computed again for every request (section 3).
const DERIVED_BYTES: usize = 32 << 20;
const DERIVED_ENTRY_BYTES: usize = 4 << 20;
/// A list inside a list item: its most entries, and the most characters of each entry's text
/// (`SHORT_CHARS` in view.rs). An endpoint whose rows hold such lists takes at most [`MAX_NESTED_LIMIT`].
const NESTED: usize = 5;
const NESTED_CHARS: usize = 200;
/// The most bytes one nested entry may take as JSON: 200 characters can be 800 bytes of UTF-8.
const NESTED_BYTES: usize = 800;
/// The most bytes a list row may take as JSON: a row holding nested lists, and any other (section 4).
const NESTED_ROW_BYTES: usize = 24 * 1024;
const ROW_BYTES: usize = 4 * 1024;
const MAX_NESTED_LIMIT: usize = 200;
/// The most per-line reads (heads, green, objections) that run at once; more get a 503 at once.
pub(crate) const DERIVED_AT_ONCE: usize = 2;
/// The most unified diffs built at once; more get a 503 at once.
const DIFFS_AT_ONCE: usize = 2;
/// The most conflict previews built at once; more get a 503 at once.
const PREVIEWS_AT_ONCE: usize = 2;
/// A conflict preview's page: at most so many conflicts, so many resolutions each in the answer, and so
/// many bytes of the sides it reads in all (section 4, *Conflict preview*).
const PREVIEW_CONFLICTS: usize = 20;
const PREVIEW_RESOLUTIONS: usize = 10;
const PREVIEW_BYTES: u64 = 16 << 20;
/// The most base line ranges a conflict's row lists, with their total.
const PREVIEW_RANGES: usize = 50;
/// The fewest hex digits an id in a path may have, as for `/api/contribution`.
const ID_DIGITS: usize = 8;
/// The most bytes of a file's text the file page shows; a larger file is named, not shown (section 4).
const FILE_TEXT: usize = 1 << 20;

/// A refusal: the status and its reason.
pub(crate) type Fail = (u16, String);

fn fail<T>(status: u16, why: impl Into<String>) -> Result<T, Fail> {
    Err((status, why.into()))
}

/// What a snapshot holds of Weave.
pub(crate) enum Weave {
    /// No Weave pages here, and why: no root, or no counted marker.
    Absent(String),
    /// The state, built.
    Built(Box<State>),
}

/// Everything the Weave endpoints read, built once per snapshot (section 3).
pub(crate) struct State {
    root: AgentId,
    marker: ContributionId,
    marker_author: Option<AgentId>,
    index: Index,
    lines: Lines,
    /// Each line's qualifying decisions, newest first in causal order: what `Lines::log(g, line, None)`
    /// gives, from one sort of the graph.
    logs: HashMap<ContributionId, Vec<ContributionId>>,
    /// `Checks` holds a `Cell`, so it is `Send` but not `Sync`: one request at a time counts.
    checks: Mutex<Checks>,
    reviews: Reviews,
    /// Each policy version, by the versions that supersede it in a relation its own author signed.
    successors: HashMap<ContributionId, Vec<ContributionId>>,
    /// Every proposal artifact whose payload decodes, retracted or invalid too, by its payload's line,
    /// ascending: `Lines` lists only valid ones per line (section 3, `records`).
    proposals: HashMap<ContributionId, Vec<ContributionId>>,
    /// Answers of the reads that sort the graph or walk a line, by key, oldest first, each with its
    /// serialized size; at most [`DERIVED_BYTES`] in all.
    derived: Mutex<Derived>,
}

/// The derived answers kept, oldest first, bounded by their serialized bytes.
#[derive(Default)]
struct Derived {
    kept: VecDeque<(String, Value, usize)>,
    bytes: usize,
}

impl Derived {
    fn get(&self, key: &str) -> Option<Value> {
        self.kept.iter().find(|(k, ..)| k == key).map(|(_, v, _)| v.clone())
    }

    /// Keeps `v` if it is at most [`DERIVED_ENTRY_BYTES`], dropping the oldest until all fit.
    fn keep(&mut self, key: String, v: &Value) {
        let size = serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());
        if size > DERIVED_ENTRY_BYTES || self.kept.iter().any(|(k, ..)| *k == key) {
            return;
        }
        while self.bytes + size > DERIVED_BYTES {
            let Some((_, _, gone)) = self.kept.pop_front() else { break };
            self.bytes -= gone;
        }
        self.bytes += size;
        self.kept.push_back((key, v.clone(), size));
    }
}

/// A snapshot's Weave state: being read until the watcher thread has built it, and emptied again before
/// the next snapshot's is read, so at most one is held at a time (answer a300d75086d5). A request takes a
/// counted reference, so a state emptied here is dropped when the last request reading it ends. If it
/// can't be had (the replica's re-read failed, or the build panicked) it says why, and nothing rebuilds it
/// from a graph it wasn't read with (finding 9f37a1b9146c).
#[derive(Default)]
pub(crate) struct Cell(Mutex<Held>);

#[derive(Default)]
enum Held {
    #[default]
    Reading,
    Ready(Arc<Weave>),
    Failed(String),
}

/// What a Weave page answers while its state is being read; `weave.js` waits only for this.
pub(crate) const READING: &str = "reading the Weave records: try again in a moment";

impl Cell {
    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The state, or why there is none: [`READING`], or why it failed.
    pub(crate) fn get(&self) -> Result<Arc<Weave>, String> {
        match &*self.lock() {
            Held::Ready(w) => Ok(Arc::clone(w)),
            Held::Reading => Err(READING.to_owned()),
            Held::Failed(why) => Err(why.clone()),
        }
    }

    /// Whether the watcher has yet to build it.
    pub(crate) fn reading(&self) -> bool {
        matches!(*self.lock(), Held::Reading)
    }

    pub(crate) fn set(&self, weave: Weave) {
        self.put(Held::Ready(Arc::new(weave)));
    }

    pub(crate) fn fail(&self, why: String) {
        self.put(Held::Failed(why));
    }

    pub(crate) fn clear(&self) {
        self.put(Held::Reading);
    }

    /// Replaces what is held; the old state is dropped after the lock is let go (a large state takes a
    /// while to free, and no request should wait on that).
    fn put(&self, next: Held) {
        let old = std::mem::replace(&mut *self.lock(), next);
        drop(old);
    }
}

impl Weave {
    /// The Weave state of the replica in `dir`, read as `g`.
    pub(crate) fn build(g: &Graph, dir: &Path) -> Weave {
        let root = match read_root(dir) {
            Ok(Some(root)) => root,
            Ok(None) => return Weave::Absent("this replica has no Weave root (weave-root)".to_owned()),
            Err(why) => {
                eprintln!("bracken view: no Weave pages: {why}");
                return Weave::Absent(why);
            }
        };
        let index = Index::build(g, Some(root));
        let Some(marker) = index.counted_marker() else {
            return Weave::Absent("this replica holds no Weave marker by its root".to_owned());
        };
        let marker_author = g.get(&marker).map(|c| c.author());
        let lines = Lines::build(g, &index);
        let mut logs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for id in newest_first(g) {
            if let Some(Ok(q)) = lines.verdict(&id) {
                logs.entry(q.line).or_default().push(id);
            }
        }
        let mut successors: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for c in g.iter() {
            if let Body::Relation { from, kind: RelationKind::Supersedes, to, .. } = c.body()
                && let (Some(new), Some(old)) = (lines.version(from), lines.version(to))
                // `Lines::build`'s rule (crates/tree/src/lines.rs), which keeps its chain private: the
                // relation by the old version's author, and the new version by that author too.
                && c.author() == old.author
                && new.author == old.author
                && !g.is_retracted(&c.id())
            {
                successors.entry(old.id).or_default().push(new.id);
            }
        }
        for list in successors.values_mut() {
            list.sort();
            list.dedup();
        }
        let mut proposals: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for c in g.iter() {
            if let Body::Artifact { media_type, .. } = c.body()
                && media_type == PROPOSAL_MEDIA_TYPE
                && let Some(p) = lines.proposal_named(&c.id())
            {
                proposals.entry(p.line).or_default().push(c.id());
            }
        }
        for list in proposals.values_mut() {
            list.sort();
        }
        let checks = Mutex::new(Checks::build(g, &index, &lines));
        let reviews = Reviews::build(&index, &lines);
        Weave::Built(Box::new(State {
            root,
            marker,
            marker_author,
            index,
            lines,
            logs,
            checks,
            reviews,
            successors,
            proposals,
            derived: Mutex::new(Derived::default()),
        }))
    }
}

/// The replica's root, from its `weave-root`: opened without following a link and without blocking (a FIFO
/// opens at once, and is then refused), checked to be a regular file on the open descriptor, and read to at
/// most [`ROOT_BYTES`]. A missing file is no root; a link, anything but a regular file, or a longer file is
/// refused with why. A file that doesn't parse is no root, as `weave::root` reads it (finding 53fca000203f).
pub(crate) fn read_root(dir: &Path) -> Result<Option<AgentId>, String> {
    use rustix::fs::{FileType, Mode, OFlags};
    let path = dir.join(crate::weave::ROOT_FILE);
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
    let fd = match rustix::fs::open(&path, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => return Err(format!("{} is a symbolic link", path.display())),
        Err(e) => return Err(format!("opening {}: {e}", path.display())),
    };
    let stat = rustix::fs::fstat(&fd).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(format!("{} isn't a regular file", path.display()));
    }
    let mut bytes = vec![0u8; ROOT_BYTES + 1];
    let mut got = 0;
    while got < bytes.len() {
        match rustix::io::read(&fd, &mut bytes[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(format!("reading {}: {e}", path.display())),
        }
    }
    if got > ROOT_BYTES {
        return Err(format!("{} is longer than {ROOT_BYTES} bytes", path.display()));
    }
    Ok(std::str::from_utf8(&bytes[..got]).ok().and_then(|t| t.trim().parse().ok()))
}

/// What `signature()` counts of `weave-root`, never following a link: its device, inode, size and
/// modification and change times, so a `weave root set`, which renames a new file into place, is seen.
pub(crate) fn root_signature(dir: &Path) -> Option<(u64, u64, u64, i64, i64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(dir.join(crate::weave::ROOT_FILE)).ok()?;
    Some((m.dev(), m.ino(), m.size(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()))
}

/// The parameters each Weave endpoint takes; any other is refused before routing, as for every endpoint.
pub(crate) fn takes(path: &str) -> &'static [&'static str] {
    let rest = path.strip_prefix("/api/weave").unwrap_or_default();
    if rest.is_empty() || rest.starts_with("/line/") || rest.starts_with("/decision/") {
        &["list", "after", "limit"]
    } else if rest.starts_with("/tree/") {
        &["dir", "list", "after", "limit"]
    } else if rest.starts_with("/file/") {
        &["path"]
    } else if rest.starts_with("/history/") {
        &["path", "after", "limit"]
    } else if rest == "/diff" {
        &["a", "b", "path", "list", "after", "limit"]
    } else if rest.starts_with("/log/") {
        &["after", "limit"]
    } else if rest.starts_with("/proposals/") {
        &["status", "after", "limit"]
    } else if rest.starts_with("/proposal/") {
        &["path", "list", "after", "limit"]
    } else {
        &[]
    }
}

/// Whether `path` is one of the Weave endpoints.
pub(crate) fn is_weave(path: &str) -> bool {
    path == "/api/weave" || path.starts_with("/api/weave/")
}

/// A request's paging: the list it pages, if any, from after which key, how many.
struct Paging<'q> {
    list: Option<&'q str>,
    after: Option<&'q str>,
    limit: usize,
}

impl<'q> Paging<'q> {
    /// The most this page's rows may number: [`MAX_NESTED_LIMIT`] when they hold nested lists.
    fn nested(&self) -> Result<(), Fail> {
        if self.limit > MAX_NESTED_LIMIT {
            return fail(
                400,
                format!("this list's rows hold lists of their own, so limit is at most {MAX_NESTED_LIMIT}"),
            );
        }
        Ok(())
    }

    fn of(query: &'q HashMap<String, String>) -> Result<Paging<'q>, Fail> {
        let limit = match query.get("limit") {
            None => DEFAULT_LIMIT,
            Some(l) => match l.parse::<usize>() {
                Ok(n) if (1..=MAX_LIMIT).contains(&n) => n,
                _ => return fail(400, format!("limit is a whole number from 1 to {MAX_LIMIT}")),
            },
        };
        Ok(Paging { list: query.get("list").map(String::as_str), after: query.get("after").map(String::as_str), limit })
    }

    /// For an answer with several lists: `after` and `limit` page one of them, named by `list`, which must be
    /// one of `names`.
    fn check(&self, names: &[&str], query: &HashMap<String, String>) -> Result<(), Fail> {
        match self.list {
            Some(l) if !names.contains(&l) => fail(400, format!("list is one of {}", names.join(", "))),
            None if query.contains_key("after") || query.contains_key("limit") => {
                fail(400, format!("after and limit page one list: name it with list ({})", names.join(", ")))
            }
            _ => Ok(()),
        }
    }
}

/// One page of `items`, in their order: those after the one whose key is `after` (which must be one of
/// them), at most `limit`. Then whether more follow.
fn page<'a, T>(
    items: &'a [T],
    key: impl Fn(&T) -> String,
    after: Option<&str>,
    limit: usize,
) -> Result<(&'a [T], bool), Fail> {
    let start = match after {
        None => 0,
        Some(a) => match items.iter().position(|i| key(i) == a) {
            Some(at) => at + 1,
            None => return fail(400, format!("after names no item of this list: {a:?}")),
        },
    };
    let end = items.len().min(start.saturating_add(limit));
    Ok((&items[start..end], end < items.len()))
}

/// A list as an answer carries it: its first [`IN_ANSWER`] items, each built by `item` and cut to
/// [`LIST_TEXT`] and `row_bytes`, and its total.
fn embedded<T>(items: &[T], row_bytes: usize, item: impl Fn(&T) -> Value) -> Value {
    let shown = &items[..items.len().min(IN_ANSWER)];
    json!({"items": shown.iter().map(|i| row(item(i), row_bytes)).collect::<Vec<_>>(),
           "total": items.len(), "more": items.len() > shown.len()})
}

/// A list's row within its caps. A row that is a bare string (a protected path, an id) is its own key for
/// `after`, so it is never cut (finding 886fba8945e1); its policy bounds it (a path is at most 4 KiB).
fn row(v: Value, row_bytes: usize) -> Value {
    if v.is_string() || row_bytes == WHOLE { v } else { fit(capped(v, LIST_TEXT), row_bytes) }
}

/// The `row_bytes` of a list whose rows are never cut: a tree child, whose name and path are a link and a
/// cursor (finding 9c8b398bffe2). Its strings are a name of at most 255 bytes, a path of at most
/// `MAX_PATH_BYTES` (4 KiB), a hash and a mode, so a row is bounded without cutting.
const WHOLE: usize = usize::MAX;

/// One page of a list, alone: what `list=<name>` answers.
fn listed<T>(
    name: &str,
    items: &[T],
    key: impl Fn(&T) -> String,
    item: impl Fn(&T) -> Value,
    row_bytes: usize,
    p: &Paging,
) -> Result<Value, Fail> {
    let (shown, more) = page(items, key, p.after, p.limit)?;
    let rows: Vec<Value> = shown.iter().map(|i| row(item(i), row_bytes)).collect();
    Ok(json!({"list": name, "items": rows, "total": items.len(), "more": more}))
}

/// The most bytes `v` takes as JSON.
fn json_size(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// A string the view never cuts (finding 80050786bf97): a record id or a content hash, which a page links
/// to or pages after.
fn kept_whole(s: &str) -> bool {
    let hex64 = |h: &str| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit());
    hex64(s) || s.strip_prefix("blake3:").is_some_and(hex64)
}

/// Keys whose string values are never cut: a check's name is the key `list=counted` and `list=checks` page
/// after.
const WHOLE_KEYS: &[&str] = &["check"];

/// How a string is cut: to at most so many bytes, or so many characters.
#[derive(Clone, Copy)]
enum Cap {
    Bytes(usize),
    Chars(usize),
}

/// Cuts every string in `v` to `cap`, on a character boundary, except ids and hashes and the values of
/// `whole_keys`; whether any was cut.
fn cut_strings(v: &mut Value, cap: Cap, whole_keys: &[&str]) -> bool {
    fn walk(v: &mut Value, cap: Cap, whole: bool, keys: &[&str]) -> bool {
        match v {
            Value::String(s) if !whole && !kept_whole(s) => {
                let end = match cap {
                    Cap::Bytes(max) if s.len() > max => {
                        let mut end = max;
                        while !s.is_char_boundary(end) {
                            end -= 1;
                        }
                        end
                    }
                    Cap::Chars(max) => match s.char_indices().nth(max) {
                        Some((at, _)) => at,
                        None => return false,
                    },
                    Cap::Bytes(_) => return false,
                };
                s.truncate(end);
                true
            }
            Value::Array(a) => a.iter_mut().fold(false, |any, x| walk(x, cap, whole, keys) | any),
            Value::Object(o) => {
                o.iter_mut().fold(false, |any, (k, x)| walk(x, cap, keys.contains(&k.as_str()), keys) | any)
            }
            _ => false,
        }
    }
    walk(v, cap, false, whole_keys)
}

/// Marks `v`, an object, as cut, so the page can say so (security note 4 on PR #79).
fn mark_cut(v: &mut Value, cut: bool) {
    if cut && let Value::Object(o) = v {
        o.insert("cut".into(), json!(true));
    }
}

/// `v` with every string cut to at most `max` bytes, except those kept whole, and `cut` set if any was.
fn capped(mut v: Value, max: usize) -> Value {
    let cut = cut_strings(&mut v, Cap::Bytes(max), WHOLE_KEYS);
    mark_cut(&mut v, cut);
    v
}

/// `v` within `max` bytes as JSON, by cutting its longest strings (never those kept whole) to the most each
/// may keep for the whole to fit, so shorter strings are kept whole; then whether any was cut. Arrays are
/// bounded by count before this ([`nest`]), so what can't be cut is ids, hashes, check names and structure.
fn fitted(v: Value, max: usize, whole_keys: &[&str]) -> (Value, bool) {
    fn longest(v: &Value) -> usize {
        match v {
            Value::String(s) => s.len(),
            Value::Array(a) => a.iter().map(longest).max().unwrap_or(0),
            Value::Object(o) => o.values().map(longest).max().unwrap_or(0),
            _ => 0,
        }
    }
    if json_size(&v) <= max {
        return (v, false);
    }
    let shrunk = |cap: usize| {
        let mut w = v.clone();
        cut_strings(&mut w, Cap::Bytes(cap), whole_keys);
        w
    };
    // The largest per-string cap that fits; a cap of 0 is tried last, and kept whether or not it fits.
    let (mut low, mut high) = (0, longest(&v));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if json_size(&shrunk(mid)) <= max {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut v = v;
    let cut = cut_strings(&mut v, Cap::Bytes(low), whole_keys);
    (v, cut)
}

/// [`fitted`], marked `cut` if it was.
fn fit(v: Value, max: usize) -> Value {
    // Room for the mark, so a cut row is within `max` with it.
    let room = if json_size(&v) > max { max.saturating_sub(r#","cut":true"#.len()) } else { max };
    let (mut v, cut) = fitted(v, room, WHOLE_KEYS);
    mark_cut(&mut v, cut);
    v
}

/// `list` cut to its first `n` entries, with `<key>_total` set in `o`.
fn bound(o: &mut Map<String, Value>, key: &str, n: usize) {
    if let Some(Value::Array(list)) = o.get_mut(key) {
        let total = list.len();
        list.truncate(n);
        o.insert(format!("{key}_total"), json!(total));
    }
}

/// `row` with each of its lists named in `keys` cut to [`NESTED`] entries, with `<key>_total` beside it
/// (section 4). Each entry's own lists (a breakage entry's `paths`, a check's `argv`) are cut the same way,
/// with their totals; then each entry is cut to [`NESTED_CHARS`] characters a string and [`NESTED_BYTES`]
/// in all, never cutting ids, hashes or check names. The row is marked `cut` if any string was.
fn nest(mut row: Value, keys: &[&str]) -> Value {
    let mut cut = false;
    if let Value::Object(o) = &mut row {
        for key in keys {
            bound(o, key, NESTED);
            if let Some(Value::Array(list)) = o.get_mut(*key) {
                for entry in list.iter_mut() {
                    if let Value::Object(e) = entry {
                        let inner: Vec<String> =
                            e.iter().filter(|(_, x)| x.is_array()).map(|(k, _)| k.clone()).collect();
                        for k in inner {
                            bound(e, &k, NESTED);
                        }
                    }
                    // A nested entry is never paged after, so its check names are cut like any text.
                    cut |= cut_strings(entry, Cap::Chars(NESTED_CHARS), &[]);
                    let (fitted, more) = fitted(std::mem::take(entry), NESTED_BYTES, &[]);
                    *entry = fitted;
                    cut |= more;
                }
            }
        }
    }
    mark_cut(&mut row, cut);
    row
}

/// A decision as a row of a list: `weave log --json`'s row, its lists nested.
fn decision_row(d: &bracken_tree::lines::DecisionRecord, names: &Names) -> Value {
    nest(decision_json(d, names), &["parents", "proposals", "resolutions", "goals", "answers", "breakage"])
}

/// The id a path names: at least [`ID_DIGITS`] hex digits, a unique prefix.
fn resolve(g: &Graph, prefix: &str) -> Result<ContributionId, Fail> {
    if prefix.len() < ID_DIGITS || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return fail(404, format!("use at least {ID_DIGITS} hex characters"));
    }
    g.resolve_prefix(prefix).map_err(|e| (404, e.kind.to_string()))
}

/// Answers one Weave request.
pub(crate) fn route(
    weave: &Weave,
    g: &Graph,
    names: &Names,
    path: &str,
    query: &HashMap<String, String>,
    slots: &WeaveSlots,
) -> Result<Value, Fail> {
    let rest = path.strip_prefix("/api/weave").unwrap_or_default();
    let state = match weave {
        Weave::Built(state) => state,
        Weave::Absent(why) if rest.is_empty() => return Ok(json!({"weave": false, "why": why})),
        Weave::Absent(why) => return fail(404, why.clone()),
    };
    let at = Ctx { s: state, g, names, slots };
    let p = Paging::of(query)?;
    let answer = if rest.is_empty() {
        at.summary(&p, query)?
    } else if let Some(id) = rest.strip_prefix("/line/") {
        at.line(id, &p, query)?
    } else if let Some(id) = rest.strip_prefix("/log/") {
        at.log(id, &p)?
    } else if let Some(id) = rest.strip_prefix("/decision/") {
        at.decision(id, &p, query)?
    } else if let Some(id) = rest.strip_prefix("/record/") {
        at.record(id)?
    } else if let Some(id) = rest.strip_prefix("/tree/") {
        at.tree(id, &p, query)?
    } else if let Some(line) = rest.strip_prefix("/proposals/") {
        at.proposals(line, &p, query)?
    } else if let Some(id) = rest.strip_prefix("/proposal/") {
        at.proposal(id, &p, query)?
    } else if let Some(line) = rest.strip_prefix("/history/") {
        at.history(line, &p, query)?
    } else if rest == "/diff" {
        // One file's unified diff is capped at FILE_TEXT a side, not PAGE_TEXT, as a file's text is.
        return at.diff(&p, query);
    } else if let Some(id) = rest.strip_prefix("/file/") {
        // A file's text is capped at FILE_TEXT, not PAGE_TEXT: its other strings are its path (at most
        // 4 KiB, as any path), mode and hash.
        return at.file(id, query);
    } else {
        return fail(404, "no such Weave page");
    };
    Ok(capped(answer, PAGE_TEXT))
}

/// One request's view of the state.
struct Ctx<'a> {
    s: &'a State,
    g: &'a Graph,
    names: &'a Names,
    slots: &'a WeaveSlots,
}

/// The Weave endpoints' slots, each taken without waiting (section 4, Caps): the per-line reads that sort
/// the graph or walk a line, and unified diffs.
pub(crate) struct WeaveSlots {
    reads: Slots,
    diffs: Slots,
    previews: Slots,
}

impl WeaveSlots {
    pub(crate) fn new() -> WeaveSlots {
        WeaveSlots {
            reads: Slots::new(DERIVED_AT_ONCE),
            diffs: Slots::new(DIFFS_AT_ONCE),
            previews: Slots::new(PREVIEWS_AT_ONCE),
        }
    }
}

impl Ctx<'_> {
    /// A derived answer: kept, or computed now within the slots and kept.
    fn derived(&self, key: String, compute: impl FnOnce() -> Value) -> Result<Value, Fail> {
        self.derived_in(&self.slots.reads, key, || {
            pause(&READ_DELAY, "BRACKEN_VIEW_READ_DELAY_MS");
            compute()
        })
    }

    /// A derived answer, computed within one of `slots` if it isn't kept.
    fn derived_in(&self, slots: &Slots, key: String, compute: impl FnOnce() -> Value) -> Result<Value, Fail> {
        let lock = || self.s.derived.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = lock().get(&key) {
            return Ok(v);
        }
        let Some(_slot) = slots.try_take() else {
            return fail(503, "the view is busy with other Weave reads: try again in a moment");
        };
        let v = compute();
        lock().keep(key, &v);
        Ok(v)
    }

    fn agent(&self, a: &AgentId) -> (String, String) {
        (a.to_string(), self.names.display(a))
    }

    fn line_of(&self, given: &str) -> Result<ContributionId, Fail> {
        let id = resolve(self.g, given)?;
        if self.s.lines.line(&id).is_some() {
            return Ok(id);
        }
        if self.s.lines.version(&id).is_some() {
            return fail(404, format!("{} is a later version of a line's policy, not a line", id.short()));
        }
        let kind = self.g.get(&id).map(kind_of).unwrap_or_default();
        fail(404, format!("{} is a {kind}, not a line", id.short()))
    }

    /// A line's heads, green under its current policy, and how many objections its next move must answer:
    /// the reads that walk the line or sort the graph, kept per line.
    fn line_state(&self, line: &ContributionId) -> Result<Value, Fail> {
        self.derived(format!("line {line}"), || {
            let lines = &self.s.lines;
            let heads = lines.heads(line);
            let rows: Vec<Value> = heads
                .iter()
                .map(|h| json!({"decision": h, "manifest": lines.decision(h).expect("a head").decision.manifest.to_string()}))
                .collect();
            let checks = self.s.checks.lock().unwrap_or_else(|p| p.into_inner());
            let green = checks.green(self.g, lines, line);
            let per_head: Vec<_> = heads.iter().map(|h| (*h, checks.counted(self.g, lines, h))).collect();
            let green = green_json(line, &green, &per_head, lines, &checks, self.names);
            drop(checks);
            // With no objection anywhere, the count is 0, but only for a line with one current policy:
            // `open_objections` has no answer (null) for a diverged one.
            let open = if lines.objections().list().is_empty() && lines.current_versions(line).len() == 1 {
                Some(0)
            } else {
                lines.open_objections(self.g, &self.s.index, line).map(|o| o.len())
            };
            json!({"heads": rows, "forked": heads.len() > 1, "green": green, "open_objections": open})
        })
    }

    fn summary(&self, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        p.check(&["lines", "problems", "unreadable", "ignored_versions"], query)?;
        let lines = &self.s.lines;
        let all_lines = listed_lines(lines, self.s.marker_author.as_ref());
        let line_row = |l: &ListedLine| {
            let mut row = line_row_json(l, self.names);
            row["decisions"] = json!(self.s.logs.get(&l.0.id).map_or(0, Vec::len));
            row
        };
        let line_key = |l: &ListedLine| l.0.id.to_hex();
        let problems = self.s.index.problems();
        // By their place in the list, which `at` gives for `after`.
        let at: Vec<usize> = (0..problems.len()).collect();
        let problem = |i: &usize| {
            let mut v = nest(problem_json(&problems[*i]), &["missing"]);
            v["at"] = json!(i);
            v
        };
        let unreadable = lines.unreadable();
        let unreadable_row = |u: &bracken_tree::lines::Unreadable| json!({"id": u.id, "kind": u.kind.as_str(), "line": u.line, "why": u.why.to_string()});
        let ignored = lines.ignored_versions();
        let numbered = |i: &usize| i.to_string();
        match p.list {
            Some("lines") => return listed("lines", &all_lines, line_key, line_row, ROW_BYTES, p),
            Some("problems") => {
                p.nested()?;
                return listed("problems", &at, numbered, problem, NESTED_ROW_BYTES, p);
            }
            Some("unreadable") => {
                return listed("unreadable", unreadable, |u| u.id.to_hex(), unreadable_row, ROW_BYTES, p);
            }
            Some("ignored_versions") => {
                return listed("ignored_versions", ignored, |i| i.to_hex(), |i| json!(i), ROW_BYTES, p);
            }
            _ => {}
        }
        let default = match lines.default_line() {
            Some(line) => {
                let mut v = self.line_state(&line)?;
                v["line"] = json!(line);
                v["name"] = json!(lines.line(&line).map(|l| l.policy.name.clone()));
                v
            }
            None => Value::Null,
        };
        let (root, root_name) = self.agent(&self.s.root);
        Ok(json!({
            "weave": true,
            "marker": self.s.marker,
            "root": root,
            "root_name": root_name,
            "marker_author": self.s.marker_author.map(|a| a.to_string()),
            "default_line": lines.default_line(),
            "default": default,
            "lines": embedded(&all_lines, ROW_BYTES, line_row),
            "problems": embedded(&at, NESTED_ROW_BYTES, problem),
            "unreadable": embedded(unreadable, ROW_BYTES, unreadable_row),
            "ignored_versions": embedded(ignored, ROW_BYTES, |i| json!(i)),
        }))
    }

    /// Every version of a line's policy chain, from its first artifact through the versions superseding each.
    fn chain(&self, line: &ContributionId) -> Vec<ContributionId> {
        let mut seen = vec![*line];
        let mut at = 0;
        while let Some(v) = seen.get(at).copied() {
            for next in self.s.successors.get(&v).into_iter().flatten() {
                if !seen.contains(next) {
                    seen.push(*next);
                }
            }
            at += 1;
        }
        seen.sort();
        seen
    }

    fn policy_json(&self, version: &ContributionId) -> Value {
        let Some(v) = self.s.lines.version(version) else { return Value::Null };
        let line = &v.policy;
        let ids = |list: &[AgentId]| -> Vec<Value> {
            list.iter().map(|a| json!({"agent": a.to_string(), "name": self.names.display(a)})).collect()
        };
        let definitions: Vec<Value> = line
            .definitions()
            .map(|d| {
                json!({"check": d.name, "argv": d.argv, "method": d.method(),
                            "executor": d.executor.map(|e| json!({"kind": e.kind.as_str(), "script": e.script,
                                                                  "script_hash": e.script_hash.to_string()}))})
            })
            .collect();
        let (author, author_name) = self.agent(&v.author);
        json!({"version": v.id, "author": author, "author_name": author_name, "name": line.name,
               "integrators": ids(&line.integrators), "validators": ids(&line.validators),
               "goal_setters": ids(&line.goal_setters), "checks": definitions,
               "guardrails": {"max_known": line.guardrails.max_known, "max_age": line.guardrails.max_age,
                              "protected": line.guardrails.protected},
               "owner_decision": line.owner_decision})
    }

    fn line(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        const POLICY_LISTS: [&str; 5] = ["checks", "integrators", "validators", "goal_setters", "protected"];
        p.check(
            &["current", "versions", "goals", "checks", "integrators", "validators", "goal_setters", "protected"],
            query,
        )?;
        let line = self.line_of(given)?;
        let lines = &self.s.lines;
        let current = lines.current_versions(&line);
        let versions = self.chain(&line);
        let all_goals = goals(&self.s.index, &line);
        let policy = match current.as_slice() {
            [one] => lines.version(one).map(|v| &v.policy),
            _ => None,
        };
        let goal_row = |goal: &bracken_tree::reviews::Goal| {
            let statement = match self.g.get(&goal.id).map(|c| c.body()) {
                Some(Body::Claim { statement, .. }) => statement.clone(),
                _ => String::new(),
            };
            let (author, author_name) = self.agent(&goal.author);
            json!({"goal": goal.id, "statement": statement, "author": author, "author_name": author_name,
                   "link": goal.link, "linker": goal.linker.to_string(), "linker_name": self.names.display(&goal.linker),
                   "binds": policy.map(|pol| goal.binds(pol))})
        };
        // A version as a list row: its own lists nested, 5 and their totals; the current policy's are paged
        // in full below (finding 80050786bf97).
        let version_row = |v: &ContributionId| {
            let mut row = nest(self.policy_json(v), &["integrators", "validators", "goal_setters", "checks"]);
            if let Some(g) = row.get_mut("guardrails") {
                *g = nest(std::mem::take(g), &["protected"]);
                if g.get("cut").is_some() {
                    row["cut"] = json!(true);
                }
            }
            row
        };
        // The one current policy's lists, each paged in full; none when the policy has diverged, where each
        // current version's lists are nested in `current`.
        let one = match current.as_slice() {
            [one] => Some(self.policy_json(one)),
            _ => None,
        };
        let policy_list = |name: &str| -> Vec<Value> {
            let Some(v) = &one else { return Vec::new() };
            let list = if name == "protected" { &v["guardrails"]["protected"] } else { &v[name] };
            // Each entry once, in order: a policy may name an agent or a path twice (Line::check allows
            // it), and a repeat would be a key `after` matches twice (round-3 note 1 on PR #79).
            let mut once = std::collections::HashSet::new();
            list.as_array().into_iter().flatten().filter(|x| once.insert(x.to_string())).cloned().collect()
        };
        let policy_key = |name: &'static str| {
            move |x: &Value| match name {
                "checks" => x["check"].as_str().unwrap_or_default().to_owned(),
                "protected" => x.as_str().unwrap_or_default().to_owned(),
                _ => x["agent"].as_str().unwrap_or_default().to_owned(),
            }
        };
        // A check's argv is bounded by count, as a nested list is, so the row's byte cap cuts only text.
        let policy_row = |name: &'static str| {
            move |x: &Value| {
                let mut x = x.clone();
                if name == "checks"
                    && let Value::Object(o) = &mut x
                {
                    bound(o, "argv", IN_ANSWER);
                }
                x
            }
        };
        let policy_bytes = |name: &str| if name == "checks" { NESTED_ROW_BYTES } else { ROW_BYTES };
        match p.list {
            Some(name @ ("current" | "versions")) => {
                p.nested()?;
                let list = if name == "current" { &current } else { &versions };
                return listed(name, list, |v| v.to_hex(), version_row, NESTED_ROW_BYTES, p);
            }
            Some("goals") => return listed("goals", &all_goals, |g| g.link.to_hex(), goal_row, ROW_BYTES, p),
            Some(name) => {
                let name = POLICY_LISTS.into_iter().find(|n| *n == name).expect("checked above");
                if one.is_none() {
                    return fail(404, "this line's policy has diverged: each current version's lists are in current");
                }
                if name == "checks" {
                    p.nested()?;
                }
                let items = policy_list(name);
                return listed(name, &items, policy_key(name), policy_row(name), policy_bytes(name), p);
            }
            None => {}
        }
        let first = lines.line(&line).expect("resolved to a line");
        let (author, author_name) = self.agent(&first.author);
        let mut v = self.line_state(&line)?;
        let fields = json!({
            "line": line, "name": first.policy.name, "author": author, "author_name": author_name,
            "default": lines.default_line() == Some(line),
            "decisions": self.s.logs.get(&line).map_or(0, Vec::len),
            "diverged": current.len() > 1,
            "current": embedded(&current, NESTED_ROW_BYTES, version_row),
            "versions": embedded(&versions, NESTED_ROW_BYTES, version_row),
            "goals": embedded(&all_goals, ROW_BYTES, goal_row),
        });
        if let (Value::Object(into), Value::Object(from)) = (&mut v, fields) {
            into.extend(from);
        }
        let mut policy = Map::new();
        for name in POLICY_LISTS {
            let items = policy_list(name);
            let shown =
                if one.is_some() { embedded(&items, policy_bytes(name), policy_row(name)) } else { Value::Null };
            policy.insert(name.into(), shown);
        }
        v["policy"] = Value::Object(policy);
        Ok(v)
    }

    fn log(&self, given: &str, p: &Paging) -> Result<Value, Fail> {
        let line = self.line_of(given)?;
        let empty = Vec::new();
        let log = self.s.logs.get(&line).unwrap_or(&empty);
        p.nested()?;
        let row =
            |id: &ContributionId| decision_row(self.s.lines.decision(id).expect("logged decisions exist"), self.names);
        let mut v = listed("decisions", log, |id| id.to_hex(), row, NESTED_ROW_BYTES, p)?;
        v["line"] = json!(line);
        Ok(v)
    }

    fn decision(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        p.check(&["counted", "reviews"], query)?;
        let id = resolve(self.g, given)?;
        let lines = &self.s.lines;
        let Some(record) = lines.decision(&id) else {
            let kind = self.g.get(&id).map(kind_of).unwrap_or_default();
            return fail(404, format!("{} is a {kind}, not a decision", id.short()));
        };
        let reviews: Vec<(&bracken_tree::ground::Review, Target)> = self.s.reviews.of_decision(lines, &id);
        let review_row = |(r, t): &(&bracken_tree::ground::Review, Target)| review_json(r, t, self.names);
        if p.list == Some("reviews") {
            return listed("reviews", &reviews, |(r, _)| r.id.to_hex(), review_row, ROW_BYTES, p);
        }
        // Counted under the line's current policy: none (null) when it has diverged, as `weave green --json`
        // gives (finding 08c43e704dff). `Checks` is held only to build the rows; each row's results nested.
        let counted: Option<Vec<(String, Value)>> = {
            let checks = self.s.checks.lock().unwrap_or_else(|p| p.into_inner());
            checks.counted(self.g, lines, &id).map(|list| {
                list.iter()
                    .map(|c| {
                        (c.check.clone(), nest(counted_json(c, &checks, self.names), &["passes", "fails", "replaced"]))
                    })
                    .collect()
            })
        };
        let counted_row = |(_, row): &(String, Value)| row.clone();
        if p.list == Some("counted") {
            p.nested()?;
            let Some(counted) = &counted else {
                return fail(404, "this decision's line has no single current policy, so nothing is counted");
            };
            return listed("counted", counted, |(check, _)| check.clone(), counted_row, NESTED_ROW_BYTES, p);
        }
        let mut v = decision_json(record, self.names);
        let verdict = match lines.verdict(&id) {
            Some(Ok(_)) => json!("qualifies"),
            Some(Err(why)) => json!({"unqualified": why.to_string()}),
            None => json!("not judged"),
        };
        let line = record.decision.line;
        let mut more = Map::new();
        more.insert("verdict".into(), verdict);
        more.insert("line".into(), json!(line));
        more.insert("compared_with".into(), json!(self.compared_parent(&record.decision)));
        more.insert("line_name".into(), json!(lines.line(&line).map(|l| l.policy.name.clone())));
        more.insert("policy".into(), json!(lines.policy_in_force_for(&id).map(|v| v.id)));
        more.insert(
            "counted".into(),
            counted.as_ref().map_or(Value::Null, |c| embedded(c, NESTED_ROW_BYTES, counted_row)),
        );
        more.insert("reviews".into(), embedded(&reviews, ROW_BYTES, review_row));
        if let Value::Object(o) = &mut v {
            o.extend(more);
        }
        Ok(v)
    }

    fn record(&self, given: &str) -> Result<Value, Fail> {
        if let Some(hex) = given.strip_prefix("blake3:") {
            let hash: ContentHash = given.parse().map_err(|_| (404, format!("blake3:{hex} isn't a content hash")))?;
            return match self.s.index.hash_kind(&hash) {
                Some(kind) => Ok(json!({"hash": hash.to_string(), "kind": kind,
                                        "page": (kind == "manifest").then_some("tree")})),
                None => fail(404, format!("this replica holds nothing with content hash {hash}")),
            };
        }
        let id = resolve(self.g, given)?;
        let kind = self.g.get(&id).map(kind_of).unwrap_or_default();
        let lines = &self.s.lines;
        let page = if lines.decision(&id).is_some() {
            Some("decision")
        } else if lines.line(&id).is_some() {
            Some("line")
        } else if lines.proposal_named(&id).is_some() {
            Some("proposal")
        } else {
            None
        };
        let page = page.or_else(|| self.manifest_of(&id).map(|_| "tree"));
        Ok(json!({"id": id, "kind": kind, "page": page}))
    }

    /// The manifest a record names, and its hash: a decision's or a proposal's, a manifest artifact's, or
    /// the one with a content hash. Only a valid manifest has a tree.
    fn manifest_of(&self, id: &ContributionId) -> Option<(ContentHash, &Manifest)> {
        let lines = &self.s.lines;
        let hash = if let Some(d) = lines.decision(id) {
            d.decision.manifest
        } else if let Some(p) = lines.proposal(id) {
            p.proposal.manifest
        } else {
            self.s.index.manifest_named(id)?
        };
        self.s.index.manifest(&hash).map(|(_, m)| (hash, m))
    }

    /// The tree a path names: `<id prefix>` of a decision, proposal or manifest artifact, or
    /// `blake3:<hash>` of a manifest.
    fn tree_of(&self, given: &str) -> Result<(ContentHash, &Manifest), Fail> {
        if given.starts_with("blake3:") {
            let hash: ContentHash = given.parse().map_err(|_| (404, format!("{given} isn't a content hash")))?;
            return match self.s.index.manifest(&hash) {
                Some((_, m)) => Ok((hash, m)),
                None => fail(404, format!("this replica holds no valid manifest with content hash {hash}")),
            };
        }
        let id = resolve(self.g, given)?;
        match self.manifest_of(&id) {
            Some(found) => Ok(found),
            None => {
                let kind = self.g.get(&id).map(kind_of).unwrap_or_default();
                fail(404, format!("{} is a {kind} with no valid manifest, so it has no tree", id.short()))
            }
        }
    }

    /// Whether this replica holds an entry's file at the size the entry names.
    fn held(&self, e: &Entry) -> bool {
        self.s.index.file_size(&e.hash) == Some(e.size)
    }

    /// `/api/weave/tree/<record>?dir`: the direct children of `dir` (the root if none), directories first
    /// with the number of files below them, then files; and the paths this replica doesn't hold.
    fn tree(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        p.check(&["children", "missing"], query)?;
        let (hash, manifest) = self.tree_of(given)?;
        // Exactly as given, never normalised (section 7): the root is no `dir` or an empty one.
        let dir = query.get("dir").map(String::as_str).unwrap_or_default();
        let children = children(manifest, dir);
        if !dir.is_empty() && children.is_empty() {
            return fail(404, "no directory at that path in this tree");
        }
        let child_row = |c: &Child| match c {
            Child::Dir { name, files } => json!({"name": name, "dir": true, "files": files}),
            Child::File(e) => json!({"name": e.path.rsplit('/').next().unwrap_or(&e.path), "dir": false,
                                     "path": e.path, "mode": e.mode.bits(), "symlink": e.mode == Mode::Symlink,
                                     "size": e.size, "hash": e.hash.to_string(), "held": self.held(e)}),
        };
        if p.list == Some("children") {
            return listed("children", &children, Child::key, child_row, WHOLE, p);
        }
        // Computed only for the answers that show it (round-1 notes on PR #81).
        let missing = self.s.index.missing(manifest);
        if p.list == Some("missing") {
            return listed("missing", &missing, String::clone, |m| json!(m), ROW_BYTES, p);
        }
        Ok(json!({"manifest": hash.to_string(), "dir": dir, "files": manifest.entries.len(),
                  "complete": missing.is_empty(),
                  "children": embedded(&children, WHOLE, child_row),
                  "missing": embedded(&missing, ROW_BYTES, |m| json!(m))}))
    }

    /// For a tree a decision names, its line and the decision its history compares it with (as
    /// [`Self::history`] does): what the file page links its history and its diff by. Null for any other.
    fn decision_context(&self, given: &str) -> Value {
        if given.starts_with("blake3:") {
            return Value::Null;
        }
        let lines = &self.s.lines;
        let Some(r) = resolve(self.g, given).ok().and_then(|id| lines.decision(&id)) else { return Value::Null };
        json!({"decision": r.id, "line": r.decision.line, "parent": self.compared_parent(&r.decision)})
    }

    /// The decision a decision's file and tree are compared with: its first parent on its own line; with
    /// none there (the line's root, or a first decision a line merge started from another line's head), its
    /// first parent; with no parent, none.
    fn compared_parent(&self, d: &Decision) -> Option<ContributionId> {
        let lines = &self.s.lines;
        let on_line = d.parents.iter().find(|p| lines.decision(p).is_some_and(|x| x.decision.line == d.line));
        on_line.or(d.parents.first()).copied()
    }

    /// `/api/weave/history/<line>?path`: the line's decisions, newest first, where the file at `path` differs
    /// from the decision it is compared with: its first parent on the line; with none there (the line's root,
    /// or a first decision a line merge started from another line's head), its first parent on any line; with
    /// no parent, an absent file (section 4).
    fn history(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        let line = self.line_of(given)?;
        let Some(path) = query.get("path") else { return fail(400, "name the file with path") };
        let lines = &self.s.lines;
        let empty = Vec::new();
        let mut rows: Vec<(ContributionId, Value)> = Vec::new();
        for id in self.s.logs.get(&line).unwrap_or(&empty) {
            let d = &lines.decision(id).expect("logged decisions exist").decision;
            let parent = self.compared_parent(d);
            let before = parent
                .and_then(|p| lines.decision(&p))
                .and_then(|r| value_at(&self.s.index, &r.decision.manifest, path));
            let after = value_at(&self.s.index, &d.manifest, path);
            let Some(kind) = change(before, after) else { continue };
            let author = lines.decision(id).expect("logged").author;
            rows.push((
                *id,
                json!({"decision": id, "author": author.to_string(), "author_name": self.names.display(&author),
                                   "parent": parent, "parent_on_line": parent.and_then(|p| lines.decision(&p)).is_some_and(|r| r.decision.line == line), "kind": kind.as_str(),
                                   "before": before.map(file_value_json), "after": after.map(file_value_json)}),
            ));
        }
        let mut v = listed("history", &rows, |(id, _)| id.to_hex(), |(_, row)| row.clone(), WHOLE, p)?;
        v["line"] = json!(line);
        v["path"] = json!(path);
        Ok(v)
    }

    /// `/api/weave/diff?a&b`: the files that differ from `a`'s tree to `b`'s, paged by path; with `path`, that
    /// one file's unified diff, built within a diff slot and kept (section 4).
    fn diff(&self, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        let (Some(a), Some(b)) = (query.get("a"), query.get("b")) else {
            return fail(400, "name the two trees with a and b");
        };
        let ((ha, ma), (hb, mb)) = (self.tree_of(a)?, self.tree_of(b)?);
        if let Some(path) = query.get("path") {
            if p.list.is_some() || query.contains_key("after") || query.contains_key("limit") {
                return fail(400, "path names one file: it takes no list, after or limit");
            }
            return self.file_diff(&ha, ma, &hb, mb, path);
        }
        p.check(&["files"], query)?;
        let diffs = bracken_tree::diff::manifests(ma, mb);
        // Held at the entry's size, as the tree and one file's diff judge it (finding 81ffe9a422c4).
        let unheld = |m: &Manifest, path: &str| m.get(path).is_some_and(|e| !self.held(e));
        let row = |d: &bracken_tree::diff::FileDiff| {
            json!({"path": d.path, "kind": d.kind.as_str(), "before": d.before.map(file_value_json),
                   "after": d.after.map(file_value_json), "not_held": unheld(ma, &d.path) || unheld(mb, &d.path)})
        };
        if p.list.is_some() {
            return listed("files", &diffs, |d| d.path.clone(), row, WHOLE, p);
        }
        Ok(json!({"a": ha.to_string(), "b": hb.to_string(), "files": embedded(&diffs, WHOLE, row)}))
    }

    /// One file's diff from `ma` to `mb`: its unified text, or why not. A side that isn't held at its entry's
    /// size is `missing`; over [`FILE_TEXT`], `oversized`; not text (or a symlink), `binary`; a diff past
    /// `diff::MAX_DIFF_CELLS`, `too_large_to_diff`; only the mode changed, `mode_only`.
    fn file_diff(
        &self,
        ha: &ContentHash,
        ma: &Manifest,
        hb: &ContentHash,
        mb: &Manifest,
        path: &str,
    ) -> Result<Value, Fail> {
        let (ea, eb) = (ma.get(path), mb.get(path));
        if ea.is_none() && eb.is_none() {
            return fail(404, "neither tree has a file at that path");
        }
        let (before, after) = (ea.map(Entry::value), eb.map(Entry::value));
        let mut v = json!({"a": ha.to_string(), "b": hb.to_string(), "path": path,
                           "kind": change(before, after).map(Kind::as_str),
                           "before": before.map(file_value_json), "after": after.map(file_value_json)});
        // A side's text: Ok(text), or Err(why it has none). An absent side is empty text.
        let side = |e: Option<&Entry>| -> Result<&str, &'static str> {
            let Some(e) = e else { return Ok("") };
            if !self.held(e) {
                return Err("missing");
            }
            let bytes = self.s.index.file(self.g, &e.hash).ok_or("missing")?;
            if bytes.len() > FILE_TEXT {
                return Err("oversized");
            }
            if e.mode == Mode::Symlink {
                return Err("binary");
            }
            text(bytes).ok_or("binary")
        };
        // Neither side is read when the contents are the same.
        let sides = || side(ea).and_then(|old| side(eb).map(|new| (old, new)));
        let shown = match change(before, after) {
            None => "same",
            Some(Kind::ModeChanged) => "mode_only",
            Some(_) => match sides() {
                Err(why) => why,
                Ok((old, new)) => {
                    let key = format!("diff {ha} {hb} {path}");
                    let unified = self.derived_in(&self.slots.diffs, key, || {
                        pause(&DIFF_DELAY, "BRACKEN_VIEW_DIFF_DELAY_MS");
                        json!(bracken_tree::diff::unified(path, old, new))
                    })?;
                    match unified.as_str() {
                        Some(t) => {
                            v["lines"] = json!(line_count(t));
                            v["unified"] = json!(t);
                            "text"
                        }
                        None => "too_large_to_diff",
                    }
                }
            },
        };
        v["shown"] = json!(shown);
        Ok(v)
    }

    /// `/api/weave/file/<record>?path`: one file of a tree, named by its mode, size and hash, with its text
    /// only if it is a held regular file of UTF-8 text without NUL, at most [`FILE_TEXT`]; a symlink with
    /// its target if that is such text. The path is looked up in the manifest held in memory, never
    /// joined to a directory (section 7).
    fn file(&self, given: &str, query: &HashMap<String, String>) -> Result<Value, Fail> {
        let (hash, manifest) = self.tree_of(given)?;
        let Some(path) = query.get("path") else { return fail(400, "name the file with path") };
        let Some(e) = manifest.get(path) else { return fail(404, "no file at that path in this tree") };
        let mut v = json!({"manifest": hash.to_string(), "path": e.path, "mode": e.mode.bits(),
                           "symlink": e.mode == Mode::Symlink, "size": e.size, "hash": e.hash.to_string(),
                           "decision": self.decision_context(given)});
        let bytes = if self.held(e) { self.s.index.file(self.g, &e.hash) } else { None };
        let (shown, text) = match bytes {
            None => ("missing", None),
            // Over the cap is oversized, before the bytes are scanned, a symlink's as any file's (finding
            // 88362a2d04a6).
            Some(b) if b.len() > FILE_TEXT => ("oversized", None),
            Some(b) if e.mode == Mode::Symlink => match text(b) {
                Some(t) => ("symlink", Some(t)),
                None => ("binary", None),
            },
            Some(b) => match text(b) {
                Some(t) => ("text", Some(t)),
                None => ("binary", None),
            },
        };
        v["shown"] = json!(shown);
        if let Some(t) = text {
            v["text"] = json!(t);
            v["lines"] = json!(line_count(t));
        }
        Ok(v)
    }
}

/// What a manifest holds at `path`, if anything.
fn value_at(ix: &Index, manifest: &ContentHash, path: &str) -> Option<FileValue> {
    ix.manifest(manifest).and_then(|(_, m)| m.get(path)).map(Entry::value)
}

/// How a file's value changed, in `diff::Kind`'s words; none if it didn't.
fn change(before: Option<FileValue>, after: Option<FileValue>) -> Option<Kind> {
    match (before, after) {
        (None, None) => None,
        (None, Some(_)) => Some(Kind::Added),
        (Some(_), None) => Some(Kind::Deleted),
        (Some(x), Some(y)) if x == y => None,
        (Some(x), Some(y)) if x.hash == y.hash => Some(Kind::ModeChanged),
        _ => Some(Kind::Modified),
    }
}

/// A proposal's status (section 1, *Follow proposals*), and the decision that integrated it, if one did.
struct Status {
    status: &'static str,
    why: Option<String>,
    decision: Option<ContributionId>,
}

impl Ctx<'_> {
    /// For each change a decision on `line` integrated, the oldest such decision: what a proposal with that
    /// change is integrated by, whichever proposal the decision named (section 1).
    fn integrations(&self, line: &ContributionId) -> HashMap<ContentHash, ContributionId> {
        let lines = &self.s.lines;
        let mut by_change = HashMap::new();
        let empty = Vec::new();
        for id in self.s.logs.get(line).unwrap_or(&empty).iter().rev() {
            for p in &lines.decision(id).expect("logged decisions exist").decision.proposals {
                if let Some(named) = lines.proposal_named(p) {
                    by_change.entry(named.change).or_insert(*id);
                }
            }
        }
        by_change
    }

    /// A proposal's status: `retracted` and `invalid` first (views ignore those), then `integrated` if a
    /// decision on its line lists it or one with the same change, else `open`. `decision` names the decision
    /// that integrated its change whatever its status.
    fn status(&self, id: &ContributionId, integrations: &HashMap<ContentHash, ContributionId>) -> Status {
        let lines = &self.s.lines;
        let Some(named) = lines.proposal_named(id) else {
            return Status { status: "invalid", why: Some("its payload doesn't decode".into()), decision: None };
        };
        let decision = integrations.get(&named.change).copied();
        let (status, why) = if self.g.is_retracted(id) {
            ("retracted", None)
        } else {
            match lines.proposal(id).map(|r| &r.verdict) {
                Some(Err(e)) => ("invalid", Some(e.to_string())),
                None => ("invalid", Some("this replica doesn't judge it".into())),
                Some(Ok(())) if decision.is_some() => ("integrated", None),
                Some(Ok(())) => ("open", None),
            }
        };
        Status { status, why, decision }
    }

    fn proposal_row(&self, id: &ContributionId, st: &Status) -> Value {
        let named = self.s.lines.proposal_named(id);
        let author = self.g.get(id).map(|c| c.author());
        json!({"proposal": id, "status": st.status, "why": st.why, "decision": st.decision,
               "author": author.map(|a| a.to_string()), "author_name": author.map(|a| self.names.display(&a)),
               "line": named.map(|p| p.line), "base": named.map(|p| p.base),
               "manifest": named.map(|p| p.manifest.to_string()), "change": named.map(|p| p.change.to_string()),
               "summary": named.map(|p| p.summary.clone())})
    }

    /// `/api/weave/proposals/<line>?status&after&limit`: the line's proposals, ascending by id, each with its
    /// status; `status` filters (default `all`). No change is computed for a list (section 4).
    fn proposals(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        let line = self.line_of(given)?;
        let want = query.get("status").map_or("all", String::as_str);
        if !["open", "integrated", "invalid", "retracted", "all"].contains(&want) {
            return fail(400, "status is one of open, integrated, invalid, retracted, all");
        }
        let integrations = self.integrations(&line);
        let empty = Vec::new();
        let rows: Vec<(ContributionId, Status)> = self
            .s
            .proposals
            .get(&line)
            .unwrap_or(&empty)
            .iter()
            .map(|id| (*id, self.status(id, &integrations)))
            .filter(|(_, st)| want == "all" || st.status == want)
            .collect();
        let mut v =
            listed("proposals", &rows, |(id, _)| id.to_hex(), |(id, st)| self.proposal_row(id, st), ROW_BYTES, p)?;
        v["line"] = json!(line);
        v["status"] = json!(want);
        Ok(v)
    }

    /// `/api/weave/proposal/<id>?list&after&limit&path`: the proposal, its status and integrating decision,
    /// its change, reviews, the objections a decision using it unchanged would answer, and for an open one
    /// on a line with one head and one current policy the conflict preview (section 4).
    fn proposal(&self, given: &str, p: &Paging, query: &HashMap<String, String>) -> Result<Value, Fail> {
        p.check(&["change", "reviews", "objections", "conflicts", "resolutions"], query)?;
        if query.contains_key("path") && p.list != Some("resolutions") {
            return fail(400, "path names a conflict's resolutions: it goes with list=resolutions");
        }
        let id = resolve(self.g, given)?;
        let lines = &self.s.lines;
        let Some(named) = lines.proposal_named(&id) else {
            let kind = self.g.get(&id).map(kind_of).unwrap_or_default();
            return fail(404, format!("{} is a {kind}, not a proposal", id.short()));
        };
        let st = self.status(&id, &self.integrations(&named.line));
        // Each list is worked out only for an answer that shows it (independent note 2 on PR #86).
        let change = || {
            let base_tree = lines.decision(&named.base).and_then(|d| self.s.index.manifest(&d.decision.manifest));
            match (base_tree, self.s.index.manifest(&named.manifest)) {
                (Some((_, b)), Some((_, m))) => Some(Change::between(b, m)),
                _ => None,
            }
        };
        let change_row = |e: &bracken_tree::format::ChangeEntry| json!({"path": e.path, "before": e.before.map(file_value_json), "after": e.after.map(file_value_json)});
        let reviews = || self.s.reviews.of_proposal(lines, &id);
        let review_row = |(r, t): &(&bracken_tree::ground::Review, Target)| review_json(r, t, self.names);
        // What the preview merges, or why there is none; a busy read is a 503, never "no preview"
        // (finding 7deca14f22ed).
        let preview = || -> Result<Result<PreviewTarget<'_>, String>, Fail> {
            if st.status == "open" { self.preview_target(&id) } else { Ok(Err("it isn't open".into())) }
        };
        let no_preview = |why: String| (404, format!("no conflict preview: {why}"));
        match p.list {
            Some("change") => {
                let change = change();
                let entries = change.as_ref().map_or(&[][..], Change::entries);
                return listed("change", entries, |e| e.path.clone(), change_row, WHOLE, p);
            }
            Some("reviews") => {
                return listed("reviews", &reviews(), |(r, _)| r.id.to_hex(), review_row, ROW_BYTES, p);
            }
            Some("objections") => {
                let objections = self.proposal_objections(&id)?;
                let Some(items) = objections.as_array() else {
                    return fail(404, "its line has no single current policy, so no objections are listed");
                };
                let key = |o: &Value| o["objection"].as_str().unwrap_or_default().to_owned();
                return listed("objections", items, key, Value::clone, ROW_BYTES, p);
            }
            Some("conflicts") => {
                let target = preview()?.map_err(no_preview)?;
                // A page is at most 20 conflicts, and 20 when no limit is given.
                let limit = if query.contains_key("limit") { p.limit } else { PREVIEW_CONFLICTS };
                if limit > PREVIEW_CONFLICTS {
                    return fail(400, format!("a conflict preview's page is at most {PREVIEW_CONFLICTS} conflicts"));
                }
                let mut v = self.preview(&id, &target, p.after, limit)?;
                v["list"] = json!("conflicts");
                return Ok(v);
            }
            Some("resolutions") => {
                let target = preview()?.map_err(no_preview)?;
                let Some(path) = query.get("path") else {
                    return fail(400, "name the conflict with path");
                };
                let key = target.key(path).ok_or_else(|| (404, "no conflict at that path".to_owned()))?;
                let offered = self.s.index.resolutions().for_key(&key);
                let row = |(rid, r): &(ContributionId, bracken_tree::format::Resolution)| {
                    self.resolution_json(rid, r, &target.integrators)
                };
                let mut v = listed("resolutions", offered, |(rid, _)| rid.to_hex(), row, ROW_BYTES, p)?;
                v["path"] = json!(path);
                return Ok(v);
            }
            _ => {}
        }
        let objections = self.proposal_objections(&id)?;
        let objection_items = objections.as_array().cloned().unwrap_or_default();
        let (preview, preview_why) = match preview()? {
            Ok(target) => (self.preview(&id, &target, None, PREVIEW_CONFLICTS)?, Value::Null),
            Err(why) => (Value::Null, json!(why)),
        };
        let change = change();
        let entries = change.as_ref().map_or(&[][..], Change::entries);
        let integrating =
            st.decision.and_then(|d| lines.decision(&d)).map(|d| json!({"decision": d.id, "note": d.decision.note}));
        let mut v = self.proposal_row(&id, &st);
        let fields = json!({
            "integrated_by": integrating,
            "change_held": change.is_some(),
            "change": embedded(entries, WHOLE, change_row),
            "reviews": embedded(&reviews(), ROW_BYTES, review_row),
            "objections": if objections.is_null() { Value::Null } else { embedded(&objection_items, ROW_BYTES, Value::clone) },
            "preview": preview,
            "preview_why": preview_why,
        });
        if let (Value::Object(into), Value::Object(from)) = (&mut v, fields) {
            into.extend(from);
        }
        Ok(v)
    }

    /// The objections a decision using the proposal unchanged would have to answer, as `open_json` rows;
    /// null when its line has no current policy. Kept per proposal: it walks the line's ancestry.
    fn proposal_objections(&self, id: &ContributionId) -> Result<Value, Fail> {
        self.derived(format!("proposal objections {id}"), || {
            match self.s.lines.proposal_objections(self.g, &self.s.index, id) {
                Some(list) => Value::Array(list.iter().map(|o| crate::weave::open_json(o, self.names)).collect()),
                None => Value::Null,
            }
        })
    }

    /// What a preview merges, or why there is none: an open proposal on a line with one head and one current
    /// policy, its base held (section 4, *Conflict preview*, 1). Reading the line's heads can be refused while
    /// the reads are busy: that is the outer error, a 503, never a reason for no preview.
    fn preview_target(&self, id: &ContributionId) -> Result<Result<PreviewTarget<'_>, String>, Fail> {
        let lines = &self.s.lines;
        let Some(record) = lines.proposal(id) else { return Ok(Err("it isn't a valid proposal".into())) };
        let line = record.proposal.line;
        let current = lines.current_versions(&line);
        let [policy] = current.as_slice() else {
            return Ok(Err("the line's policy has diverged, so integrate would refuse".into()));
        };
        let integrators = lines.version(policy).map(|v| v.policy.integrators.clone()).unwrap_or_default();
        let state = self.line_state(&line)?;
        let heads = state["heads"].as_array().cloned().unwrap_or_default();
        let [head] = heads.as_slice() else {
            return Ok(Err(format!("the line has {} heads, so integrate would refuse", heads.len())));
        };
        let Some(head) = head["decision"].as_str().and_then(|h| h.parse::<ContributionId>().ok()) else {
            return Ok(Err("the line's head isn't readable".into()));
        };
        let tree = |d: &ContributionId| {
            lines.decision(d).and_then(|r| self.s.index.manifest(&r.decision.manifest)).map(|(_, m)| m)
        };
        let Some(base) = tree(&record.proposal.base) else { return Ok(Err("its base's tree isn't held".into())) };
        let Some(into) = tree(&head) else { return Ok(Err("the line head's tree isn't held".into())) };
        let Some((_, from)) = self.s.index.manifest(&record.proposal.manifest) else {
            return Ok(Err("its tree isn't held".into()));
        };
        Ok(Ok(PreviewTarget { head, integrators, base, into, from }))
    }

    fn resolution_json(
        &self,
        id: &ContributionId,
        r: &bracken_tree::format::Resolution,
        integrators: &[AgentId],
    ) -> Value {
        let author = self.g.get(id).map(|c| c.author());
        json!({"resolution": id, "author": author.map(|a| a.to_string()),
               "author_name": author.map(|a| self.names.display(&a)),
               "integrator": author.is_some_and(|a| integrators.contains(&a)),
               "result": r.result.map(file_value_json)})
    }

    /// One page of the conflict preview: the three-way merge's conflicts after `after`, at most `limit`, and
    /// fewer if their sides would pass [`PREVIEW_BYTES`]: the page stops before the conflict that would, and
    /// the next page starts from it. Kept per proposal, head and page. The merge runs only in one of the
    /// preview slots, so with both taken a page that isn't kept is refused at once (finding dda45ad01045).
    fn preview(
        &self,
        id: &ContributionId,
        t: &PreviewTarget,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Value, Fail> {
        // The first page and a page after a cursor never share a key, even after an empty cursor
        // (independent note 1 on PR #86).
        let from = after.map_or_else(|| "start".to_owned(), |a| format!("after {}", encode_key(a)));
        let key = format!("preview {id} {} {from} {limit}", t.head);
        let lock = || self.s.derived.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = lock().get(&key) {
            return Ok(v);
        }
        let Some(_slot) = self.slots.previews.try_take() else {
            return fail(503, "the view is busy with other Weave reads: try again in a moment");
        };
        pause(&PREVIEW_DELAY, "BRACKEN_VIEW_PREVIEW_DELAY_MS");
        let merged = three_way(t.base, t.into, t.from);
        let start = match after {
            None => 0,
            Some(a) => match merged.conflicts.iter().position(|c| c.key.path == a) {
                Some(at) => at + 1,
                None => return fail(400, format!("after names no conflict of this preview: {a:?}")),
            },
        };
        let mut rows = Vec::new();
        let mut spent: u64 = 0;
        let mut next = start;
        while next < merged.conflicts.len() && rows.len() < limit {
            let c = &merged.conflicts[next];
            // What its sides cost is known from their sizes, before any is read.
            let cost = self.read_cost(c);
            if !rows.is_empty() && spent + cost > PREVIEW_BYTES {
                break;
            }
            spent += cost;
            rows.push(self.conflict_json(c, &t.integrators));
            next += 1;
        }
        let v = json!({"head": t.head, "items": rows, "total": merged.conflicts.len(),
                       "more": next < merged.conflicts.len(), "settled": merged.settled.len()});
        lock().keep(key, &v);
        Ok(v)
    }

    /// How a conflict's lines would merge, from its sides' modes and held sizes alone, when that settles it:
    /// `not_text` where `text_conflicts` (as integrate) would give none without reading a byte (a side absent,
    /// a symlink, not held, or over `text_merge::MAX_TEXT_BYTES`); then `over_view_cap` where a side is over
    /// the view's cap. `None` when the sides are to be read (finding 9e061e918bcd).
    fn lines_unread(&self, c: &Conflict) -> Option<&'static str> {
        let sizes: Option<Vec<u64>> = [c.key.base, c.ours, c.theirs]
            .iter()
            .map(|v| v.filter(|v| v.mode != Mode::Symlink).and_then(|v| self.s.index.file_size(&v.hash)))
            .collect();
        let Some(sizes) = sizes else { return Some("not_text") };
        if sizes.iter().any(|s| *s > bracken_tree::text_merge::MAX_TEXT_BYTES as u64) {
            Some("not_text")
        } else if sizes.iter().any(|s| *s > FILE_TEXT as u64) {
            Some("over_view_cap")
        } else {
            None
        }
    }

    /// The bytes [`Ctx::conflict_json`] reads of a conflict's sides: all three, held and within the view's
    /// cap, or none.
    fn read_cost(&self, c: &Conflict) -> u64 {
        if self.lines_unread(c).is_some() {
            return 0;
        }
        [c.key.base, c.ours, c.theirs].iter().flatten().filter_map(|v| self.s.index.file_size(&v.hash)).sum()
    }

    /// A conflict's row. A side over the view's cap is named, not read.
    fn conflict_json(&self, c: &Conflict, integrators: &[AgentId]) -> Value {
        let ix = &self.s.index;
        let (lines, ranges) = match self.lines_unread(c) {
            Some(lines) => (lines, Vec::new()),
            None => match text_conflicts(c, |h| ix.file(self.g, h)) {
                None => ("not_text", Vec::new()),
                Some(r) if r.is_empty() => {
                    let modes = match (c.key.base, c.ours, c.theirs) {
                        (Some(b), Some(x), Some(y)) => merge_mode(b.mode, x.mode, y.mode).is_some(),
                        _ => true,
                    };
                    (if modes { "clean" } else { "mode" }, Vec::new())
                }
                Some(r) => ("conflicts", r),
            },
        };
        let offered = ix.resolutions().for_key(&c.key);
        let rows: Vec<Value> = offered
            .iter()
            .take(PREVIEW_RESOLUTIONS)
            .map(|(rid, r)| self.resolution_json(rid, r, integrators))
            .collect();
        // What integrate would try on its own: an integrator's resolution, the smallest id first. Integrate
        // still refuses if its result isn't held here (`settled_by_held`), or if the merged paths collide,
        // which only the whole merged tree shows (security note 2 on PR #86).
        let settled =
            offered.iter().find(|(rid, _)| self.g.get(rid).is_some_and(|x| integrators.contains(&x.author())));
        let settled_by_held = settled.map(|(_, r)| r.result.is_none_or(|v| ix.file_size(&v.hash).is_some()));
        let shown: Vec<Value> = ranges.iter().take(PREVIEW_RANGES).map(|(a, b)| json!([a, b])).collect();
        json!({"path": c.key.path, "base": c.key.base.map(file_value_json),
               "into": c.ours.map(file_value_json), "from": c.theirs.map(file_value_json),
               "lines": lines, "ranges": shown, "ranges_total": ranges.len(),
               "resolutions": rows, "resolutions_total": offered.len(),
               "settled_by": settled.map(|(rid, _)| *rid), "settled_by_held": settled_by_held})
    }
}

/// The three trees a preview merges, the line head it was computed against, and the line's integrators.
struct PreviewTarget<'a> {
    head: ContributionId,
    integrators: Vec<AgentId>,
    base: &'a Manifest,
    into: &'a Manifest,
    from: &'a Manifest,
}

impl PreviewTarget<'_> {
    /// The key of the conflict at `path`, if the merge has one there: three lookups, by `three_way`'s rule
    /// (a path is settled where the sides agree, or one side is the base's), never the whole merge
    /// (finding dda45ad01045).
    fn key(&self, path: &str) -> Option<Key> {
        let value = |m: &Manifest| m.get(path).map(Entry::value);
        let (b, x, y) = (value(self.base), value(self.into), value(self.from));
        (x != y && x != b && y != b).then(|| Key::new(path.to_owned(), b, [x, y]))
    }
}

/// A cursor as part of a cache key: its bytes in hex, so no path can make two keys collide.
fn encode_key(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Test hooks: how long a per-line read, a unified diff, and a conflict preview's page wait inside their
/// slots, so a test can hold the slots: `BRACKEN_VIEW_READ_DELAY_MS`, `BRACKEN_VIEW_DIFF_DELAY_MS` and
/// `BRACKEN_VIEW_PREVIEW_DELAY_MS`, each at most 2 s. Unset, nothing sleeps.
static READ_DELAY: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
static DIFF_DELAY: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
static PREVIEW_DELAY: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();

fn pause(cell: &'static std::sync::OnceLock<std::time::Duration>, var: &str) {
    let delay = *cell.get_or_init(|| {
        let ms = std::env::var(var).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        std::time::Duration::from_millis(ms.min(2_000))
    });
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}

/// A child of a tree's directory: a directory, with the number of files below it, or a file.
enum Child<'m> {
    Dir { name: &'m str, files: usize },
    File(&'m Entry),
}

impl Child<'_> {
    /// Its key for `after`: its name, a directory's with a trailing `/`, so a file and a directory of the same
    /// name are told apart.
    fn key(&self) -> String {
        match self {
            Child::Dir { name, .. } => format!("{name}/"),
            Child::File(e) => e.path.rsplit('/').next().unwrap_or(&e.path).to_owned(),
        }
    }
}

/// The direct children of `dir` in `m`: directories, then files, each in path-byte order. The entries under
/// `dir` are a contiguous range of the sorted manifest, found by binary search.
fn children<'m>(m: &'m Manifest, dir: &str) -> Vec<Child<'m>> {
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let start = m.entries.partition_point(|e| e.path.as_bytes() < prefix.as_bytes());
    let end = start + m.entries[start..].partition_point(|e| e.path.as_bytes().starts_with(prefix.as_bytes()));
    let (mut dirs, mut files): (Vec<Child>, Vec<Child>) = (Vec::new(), Vec::new());
    for e in &m.entries[start..end] {
        let rest = &e.path[prefix.len()..];
        match rest.split_once('/') {
            Some((name, _)) => match dirs.last_mut() {
                Some(Child::Dir { name: last, files }) if *last == name => *files += 1,
                _ => dirs.push(Child::Dir { name, files: 1 }),
            },
            None => files.push(Child::File(e)),
        }
    }
    dirs.extend(files);
    dirs
}

/// `b` as text: UTF-8 without NUL, by `diff::is_text`'s rule, so the file page and diffs agree on it (a
/// symlink's mode is checked by the caller, which shows a text target as a target).
fn text(b: &[u8]) -> Option<&str> {
    if bracken_tree::diff::is_text(Mode::File, b) { std::str::from_utf8(b).ok() } else { None }
}

/// How many lines `text` has: its newlines, and one more if it doesn't end with one.
fn line_count(text: &str) -> usize {
    let newlines = text.bytes().filter(|b| *b == b'\n').count();
    newlines + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Section 3: this module's only file access is the guarded `weave-root` read (and its metadata, for
    /// the signature). Anything else that could open, write or list a file is refused here.
    #[test]
    fn nothing_here_touches_the_file_system_but_weave_root() {
        let text = include_str!("view_weave.rs");
        let code = text.split("#[cfg(test)]").next().expect("the module before its tests");
        let allowed = [
            "use rustix::fs::{FileType, Mode, OFlags};",
            "rustix::fs::open(&path",
            "rustix::fs::fstat(&fd)",
            "std::fs::symlink_metadata(dir.join(",
        ];
        let mut rest = code.to_owned();
        for a in allowed {
            assert_eq!(rest.matches(a).count(), 1, "{a} once");
            rest = rest.replace(a, "");
        }
        for bad in
            ["std::fs", "rustix::fs::", "File::", "OpenOptions", "materialize", "create_dir", "remove_", "write("]
        {
            assert!(!rest.contains(bad), "view_weave.rs names {bad}");
        }
    }

    /// Section 3: a Ground of many lines costs one log sort. `newest_first` is called once, in
    /// `Weave::build`, and nothing calls `Lines::log`, which sorts the whole graph per line.
    #[test]
    fn the_logs_come_from_one_sort() {
        let text = include_str!("view_weave.rs");
        let code = text.split("#[cfg(test)]").next().expect("the module before its tests");
        assert_eq!(code.matches("newest_first(").count(), 1, "one newest_first call");
        let build = code.split("fn build(").nth(1).and_then(|b| b.split("\n    }\n").next()).expect("Weave::build");
        assert!(build.contains("newest_first(g)"), "the one call is in Weave::build");
        assert!(!code.contains(".log(g"), "nothing calls Lines::log");
    }

    #[test]
    fn paging_follows_after_and_refuses_an_unknown_key() {
        let items: Vec<u32> = (0..10).collect();
        let key = |i: &u32| i.to_string();
        assert_eq!(page(&items, key, None, 3).unwrap(), (&items[0..3], true));
        assert_eq!(page(&items, key, Some("2"), 3).unwrap(), (&items[3..6], true));
        assert_eq!(page(&items, key, Some("6"), 5).unwrap(), (&items[7..10], false));
        assert_eq!(page(&items, key, Some("9"), 5).unwrap(), (&items[10..10], false));
        assert_eq!(page(&items, key, Some("x"), 5).unwrap_err().0, 400);
    }

    #[test]
    fn rows_fit_their_byte_caps_and_nested_entries_theirs() {
        // A breakage entry: its paths cut to 5 with their total, the entry within 800 bytes though each
        // string keeps 200 characters of four-byte UTF-8 at most.
        let wide = "\u{1F600}".repeat(300);
        let paths: Vec<String> = (0..9).map(|i| format!("{i}{wide}")).collect();
        let breakage: Vec<Value> = (0..7).map(|_| json!({"check": wide, "detail": wide, "paths": paths})).collect();
        let row = nest(json!({"id": "x", "note": "n".repeat(30_000), "breakage": breakage}), &["breakage"]);
        assert_eq!(row["breakage_total"], 7);
        let entries = row["breakage"].as_array().expect("a list");
        assert_eq!(entries.len(), NESTED);
        for e in entries {
            assert_eq!(e["paths_total"], 9);
            assert!(e["paths"].as_array().expect("paths").len() <= NESTED);
            assert!(json_size(e) <= NESTED_BYTES, "{}", json_size(e));
        }
        let fitted = fit(capped(row, LIST_TEXT), NESTED_ROW_BYTES);
        assert!(json_size(&fitted) <= NESTED_ROW_BYTES);
        assert_eq!(fitted["id"], "x", "short strings are kept whole");
        // A flat row: strings that need escaping still fit, and a row already within its cap is untouched.
        let flat = fit(json!({"a": "\"".repeat(5_000), "b": "kept"}), ROW_BYTES);
        assert!(json_size(&flat) <= ROW_BYTES);
        assert_eq!(flat["b"], "kept");
        let small = json!({"a": "short"});
        assert_eq!(fit(small.clone(), ROW_BYTES), small);
    }

    /// Finding 80050786bf97: ids, content hashes and check names are never cut, whatever the cap; arrays in
    /// a nested entry are bounded by count; and a cut row says so.
    #[test]
    fn ids_hashes_and_check_names_are_never_cut() {
        let id = "ab".repeat(32);
        let hash = format!("blake3:{}", "cd".repeat(32));
        let check = "c".repeat(5_000);
        let mut row = json!({"version": id, "manifest": hash, "check": check, "note": "n".repeat(10_000),
                             "parents": vec![id.clone(); 9]});
        row = capped(row, 16);
        assert_eq!(row["version"], id.as_str());
        assert_eq!(row["manifest"], hash.as_str());
        assert_eq!(row["check"], check.as_str());
        assert_eq!(row["note"].as_str().map(str::len), Some(16));
        assert_eq!(row["cut"], true);
        let fitted = fit(json!({"version": id, "note": "n".repeat(10_000)}), 200);
        assert_eq!(fitted["version"], id.as_str());
        assert_eq!(fitted["cut"], true);
        let untouched = fit(json!({"version": id}), ROW_BYTES);
        assert!(untouched.get("cut").is_none(), "a row within its cap isn't marked");
        // A check in a nested list: its argv bounded by count, with its total.
        let nested = nest(json!({"checks": [{"check": "t", "argv": vec!["a"; 40]}]}), &["checks"]);
        assert_eq!(nested["checks"][0]["argv"].as_array().map(Vec::len), Some(NESTED));
        assert_eq!(nested["checks"][0]["argv_total"], 40);
    }

    /// Independent note 10: the derived cache evicts its oldest entries to stay within its bytes, and
    /// keeps nothing over an entry's cap.
    #[test]
    fn derived_evicts_the_oldest_to_stay_within_its_bytes() {
        let mut d = Derived::default();
        let entry = |n: usize| json!("x".repeat(n));
        // Entries of 3 MiB: 10 fit in the 32 MiB.
        let third = 3 << 20;
        for k in 0..10 {
            d.keep(format!("k{k}"), &entry(third));
        }
        assert!(d.get("k0").is_some() && d.get("k9").is_some());
        d.keep("k10".into(), &entry(third));
        assert!(d.get("k0").is_none(), "the oldest went first");
        assert!(d.get("k1").is_some() && d.get("k10").is_some());
        assert!(d.bytes <= DERIVED_BYTES);
        d.keep("big".into(), &entry(DERIVED_ENTRY_BYTES + 1));
        assert!(d.get("big").is_none(), "an entry over its cap isn't kept");
    }

    #[test]
    fn strings_are_cut_on_a_character_boundary() {
        let v = capped(json!({"a": ["ééé", {"b": "abcdef"}], "n": 7}), 3);
        assert_eq!(v, json!({"a": ["é", {"b": "abc"}], "n": 7, "cut": true}));
    }

    #[test]
    fn weave_root_is_read_only_if_a_short_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let root_file = dir.path().join(crate::weave::ROOT_FILE);
        assert_eq!(read_root(dir.path()), Ok(None), "missing");
        let agent = "6bc92b401e36be06cf432a65cb2cbd7cc765c56b6ad40a3c0c5ffb9c24c6298b";
        std::fs::write(&root_file, format!("{agent}\n")).unwrap();
        assert_eq!(read_root(dir.path()), Ok(Some(agent.parse().unwrap())));
        std::fs::write(&root_file, "not an agent\n").unwrap();
        assert_eq!(read_root(dir.path()), Ok(None), "malformed is no root, as weave::root reads it");
        std::fs::write(&root_file, vec![b'a'; ROOT_BYTES + 1]).unwrap();
        assert!(read_root(dir.path()).unwrap_err().contains("longer than"));
        std::fs::remove_file(&root_file).unwrap();
        std::fs::write(dir.path().join("real"), format!("{agent}\n")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), &root_file).unwrap();
        assert!(read_root(dir.path()).unwrap_err().contains("symbolic link"));
        std::fs::remove_file(&root_file).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(&root_file).status().unwrap().success());
        // A FIFO with no writer: a blocking open would wait forever.
        assert!(read_root(dir.path()).unwrap_err().contains("isn't a regular file"));
    }
}
