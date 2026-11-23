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
use bracken_tree::ground::{Index, kind_of};
use bracken_tree::lines::{Lines, newest_first};
use bracken_tree::results::Checks;
use bracken_tree::reviews::{Reviews, Target, goals};
use manyroot::{AgentId, Body, ContributionId, Graph, RelationKind};
use serde_json::{Map, Value, json};

use crate::Names;
use crate::view::Slots;
use crate::weave::{
    ListedLine, counted_json, decision_json, green_json, line_row_json, listed_lines, problem_json, review_json,
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
/// The fewest hex digits an id in a path may have, as for `/api/contribution`.
const ID_DIGITS: usize = 8;

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

/// A snapshot's Weave state: empty until the watcher thread has read it, and emptied again before the
/// next snapshot's is read, so at most one is held at a time (answer a300d75086d5). A request takes a
/// counted reference, so a state emptied here is dropped when the last request reading it ends.
#[derive(Default)]
pub(crate) struct Cell(Mutex<Option<Arc<Weave>>>);

impl Cell {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<Weave>>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The state, if it has been read.
    pub(crate) fn get(&self) -> Option<Arc<Weave>> {
        self.lock().clone()
    }

    pub(crate) fn set(&self, weave: Weave) {
        *self.lock() = Some(Arc::new(weave));
    }

    pub(crate) fn clear(&self) {
        self.lock().take();
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
                && c.author() == old.author
                && !g.is_retracted(&c.id())
            {
                successors.entry(old.id).or_default().push(new.id);
            }
        }
        for list in successors.values_mut() {
            list.sort();
            list.dedup();
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
    } else if rest.starts_with("/log/") {
        &["after", "limit"]
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
    json!({"items": shown.iter().map(|i| fit(capped(item(i), LIST_TEXT), row_bytes)).collect::<Vec<_>>(),
           "total": items.len(), "more": items.len() > shown.len()})
}

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
    let rows: Vec<Value> = shown.iter().map(|i| fit(capped(item(i), LIST_TEXT), row_bytes)).collect();
    Ok(json!({"list": name, "items": rows, "total": items.len(), "more": more}))
}

/// The most bytes `v` takes as JSON.
fn json_size(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// `v` within `max` bytes as JSON, if cutting strings can bring it there: its longest strings are cut to
/// the most bytes each may keep for the whole to fit, so shorter strings are kept whole.
fn fit(v: Value, max: usize) -> Value {
    fn longest(v: &Value) -> usize {
        match v {
            Value::String(s) => s.len(),
            Value::Array(a) => a.iter().map(longest).max().unwrap_or(0),
            Value::Object(o) => o.values().map(longest).max().unwrap_or(0),
            _ => 0,
        }
    }
    if json_size(&v) <= max {
        return v;
    }
    // The largest per-string cap that fits; a cap of 0 is tried last, and kept whether or not it fits.
    let (mut low, mut high) = (0, longest(&v));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if json_size(&capped(v.clone(), mid)) <= max {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    capped(v, low)
}

/// `v` with every string cut to at most `max` bytes, on a character boundary.
fn capped(mut v: Value, max: usize) -> Value {
    fn cut(v: &mut Value, max: usize) {
        match v {
            Value::String(s) if s.len() > max => {
                let mut end = max;
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
            }
            Value::Array(a) => a.iter_mut().for_each(|x| cut(x, max)),
            Value::Object(o) => o.values_mut().for_each(|x| cut(x, max)),
            _ => {}
        }
    }
    cut(&mut v, max);
    v
}

/// `row` with each of its lists named in `keys` cut to [`NESTED`] entries, with `<key>_total` beside it,
/// and each of those entries cut to [`NESTED_CHARS`] characters a string and [`NESTED_BYTES`] in all. An
/// entry's own `paths` (a breakage entry's) is cut the same way, with `paths_total` (section 4).
fn nest(mut row: Value, keys: &[&str]) -> Value {
    fn short(v: &mut Value) {
        match v {
            Value::String(s) => {
                if let Some((at, _)) = s.char_indices().nth(NESTED_CHARS) {
                    s.truncate(at);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(short),
            Value::Object(o) => o.values_mut().for_each(short),
            _ => {}
        }
    }
    if let Value::Object(o) = &mut row {
        for key in keys {
            if let Some(Value::Array(list)) = o.get_mut(*key) {
                let total = list.len();
                list.truncate(NESTED);
                for entry in list.iter_mut() {
                    if let Value::Object(e) = entry
                        && let Some(Value::Array(paths)) = e.get_mut("paths")
                    {
                        let all = paths.len();
                        paths.truncate(NESTED);
                        e.insert("paths_total".into(), json!(all));
                    }
                    short(entry);
                    *entry = fit(std::mem::take(entry), NESTED_BYTES);
                }
                o.insert(format!("{key}_total"), json!(total));
            }
        }
    }
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
    slots: &Slots,
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
    slots: &'a Slots,
}

impl Ctx<'_> {
    /// A derived answer: kept, or computed now within the slots and kept.
    fn derived(&self, key: String, compute: impl FnOnce() -> Value) -> Result<Value, Fail> {
        let lock = || self.s.derived.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = lock().get(&key) {
            return Ok(v);
        }
        let Some(_slot) = self.slots.try_take() else {
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
            let open = if lines.objections().list().is_empty() {
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
        p.check(&["current", "versions", "goals"], query)?;
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
        let version_row = |v: &ContributionId| self.policy_json(v);
        match p.list {
            Some("current") => return listed("current", &current, |v| v.to_hex(), version_row, ROW_BYTES, p),
            Some("versions") => return listed("versions", &versions, |v| v.to_hex(), version_row, ROW_BYTES, p),
            Some("goals") => return listed("goals", &all_goals, |g| g.link.to_hex(), goal_row, ROW_BYTES, p),
            _ => {}
        }
        let first = lines.line(&line).expect("resolved to a line");
        let (author, author_name) = self.agent(&first.author);
        let mut v = self.line_state(&line)?;
        let fields = json!({
            "line": line, "name": first.policy.name, "author": author, "author_name": author_name,
            "default": lines.default_line() == Some(line),
            "decisions": self.s.logs.get(&line).map_or(0, Vec::len),
            "diverged": current.len() > 1,
            "current": embedded(&current, ROW_BYTES, version_row),
            "versions": embedded(&versions, ROW_BYTES, version_row),
            "goals": embedded(&all_goals, ROW_BYTES, goal_row),
        });
        if let (Value::Object(into), Value::Object(from)) = (&mut v, fields) {
            into.extend(from);
        }
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
        let checks = self.s.checks.lock().unwrap_or_else(|p| p.into_inner());
        let counted = checks.counted(self.g, lines, &id).unwrap_or_default();
        let counted_row = |c: &bracken_tree::results::Counted| counted_json(c, &checks, self.names);
        match p.list {
            Some("counted") => return listed("counted", &counted, |c| c.check.clone(), counted_row, ROW_BYTES, p),
            Some("reviews") => return listed("reviews", &reviews, |(r, _)| r.id.to_hex(), review_row, ROW_BYTES, p),
            _ => {}
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
        more.insert("line_name".into(), json!(lines.line(&line).map(|l| l.policy.name.clone())));
        more.insert("policy".into(), json!(lines.policy_in_force_for(&id).map(|v| v.id)));
        more.insert("counted".into(), embedded(&counted, ROW_BYTES, counted_row));
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
        } else {
            None
        };
        Ok(json!({"id": id, "kind": kind, "page": page}))
    }
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

    #[test]
    fn strings_are_cut_on_a_character_boundary() {
        let v = capped(json!({"a": ["ééé", {"b": "abcdef"}], "n": 7}), 3);
        assert_eq!(v, json!({"a": ["é", {"b": "abc"}], "n": 7}));
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
