//! The six measures (design section 4), computed after a run from records no
//! agent can write: the driver's event log (with its graded snapshots of the
//! integrated state) and the token usage the backend records.
//!
//! [`DEFINITIONS`] states each measure as this code computes it; the plan
//! quotes it, so a run's plan fixes the analysis (decisions `bbb261d50871`
//! and the answers to question `6c4aa5dc0a01`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::grader::{FunctionGrade, HiddenCases, grade, grade_function};
use crate::lang::{Calls, Library, Target, Token, parse_module, run};
use crate::task::{Task, Version};

/// Each measure exactly as computed here.
pub const DEFINITIONS: &[(&str, &str)] = &[
    (
        "definition",
        "A function's definition is its def line plus the lines of the non-task helpers it calls, transitively, \
         each with comments stripped and whitespace normalized.",
    ),
    (
        "complete",
        "A run is measured only if its log has no gap, ends with its integrated event (or with the stop of one \
         of the backend's own caps the plan expects, measured from what exists), and holds no failed snapshot \
         or record, and every turn has a usage row or failed at play (a failed sync, a spent budget), using none.",
    ),
    (
        "snapshot",
        "The driver logs the hub's integrated library after setup, after the change requests are released, and \
         after every sync and hub tool call that changes it. Each snapshot is graded under the spec version in \
         force when it was taken: the original until the first release, then the change requests released so \
         far (Phase 1's all at once, Phase 2's wave by wave).",
    ),
    (
        "tokens",
        "Usage rows of one message (one per content block, each repeating the message's usage) count once, at \
         their largest figures, asking for the tools of all of them. A turn's tokens are its settled usage row \
         (input, cache reads, cache writes, output); a turn without one counts its messages' input side and the \
         output it was counted at, with or without messages.",
    ),
    (
        "improvements",
        "Each snapshot is compared with the one before, both graded in library under the newer snapshot's \
         version: +1 for each function that now passes and didn't, -1 for each that passed and now doesn't. \
         Net improvements per million tokens and per million output tokens.",
    ),
    (
        "delivery",
        "Git and Bracken: a write (a commit's functions whose definition differs from every parent's, an artifact's \
         function) whose definition differs from the integrated one at that moment. CRDT: a hub sync after which a \
         function's definition at the hub differs from before, credited to the syncing agent. A delivery is \
         correct if it passes the grader standalone under the version in force (Phase 2: the specs released as \
         of the delivering agent's last sync before it); an open contract's member, with the delivering agent's \
         working copy of the other members (CRDT: its own edits that sync delivered; Git: the branch at the \
         commit; Bracken: each member accepted at the hub, else any of the agent's own current \
         implementations before the sync that makes the delivery available), and for members the copy \
         doesn't hold the integrated library as the agent's last sync before the delivery left it, the point \
         its specs come from (CRDT: the sync before the delivering one). A verdict's blocked definition is \
         judged under the version at the verdict (Phase 2: as of the verdict-giver's last sync before it), a \
         member with its latest delivery's working copy.",
    ),
    (
        "duplicated",
        "Visible: another agent's correct delivery of the same function was available before it. Blind: otherwise, \
         another agent delivered the same function after this agent's last sync before it. Also the share of \
         output tokens spent in the turns that made a duplicated delivery.",
    ),
    (
        "overhead",
        "Each message's tokens are split equally among the tools it asked for (mcp__<server>__ \
         prefixes dropped). Integrate tools: sync, \
         merge, rebase, conflicts, pr_open, pr_list, pr_view, pr_comment, pr_merge, pr_close. Validate tools: \
         check, evidence, pr_review. Time: each call's time from call to result, over the turns' total time. \
         Failed integrations: integrate calls that return an error, merges that stop on a conflict, and PRs \
         closed unmerged.",
    ),
    (
        "discovery",
        "A useful discovery is a correct delivery, or the first bug report of a function and its inputs that \
         really fails: text 'f(1, 2) gave 3' in any tool's arguments but notes_write, where f is no open \
         contract's member (which has no single right answer) and, at that moment, \
         gives a wrong answer or fails on those inputs in the integrated library or in the latest delivered \
         definition of f. Available: the maker's next sync (a CRDT delivery, and a PR tool's text, at once). \
         Integrated: the first snapshot holding the delivered definition. Taken up: another agent's first sync \
         after it was available. Each in seconds and in other agents' turns started, as medians.",
    ),
    (
        "incompatible",
        "A function whose definition a snapshot didn't change, that passes standalone after it and passed in \
         library before it (both under the newer version) but not after. Also snapshots after which a module \
         that parsed no longer does. Semantic-conflict events: a verdict blocking a definition the grader scores \
         correct standalone (Bracken: a non-author's fail evidence; Git: a request_changes review on a PR whose \
         changed functions all are); n/a in CRDT mode.",
    ),
    (
        "quality",
        "The last snapshot graded under all change requests: functions correct in library, present but wrong, \
         and missing.",
    ),
    (
        "checkpoints",
        "Phase 2 ranks the modes by these alone (decision 288a16ce8cb2); its per-delivery measures above are \
         per-mode diagnostics, not comparable across modes. A checkpoint is the end of a turn index, once \
         every agent has finished that turn: the driver logs the hub's integrated library then (Git's main, \
         Bracken's accepted layer, CRDT's document). Each is graded with the grader, the same in every mode, \
         under the change requests released before it; a function is correct when it is right in the library. \
         Final: correct at the last checkpoint. Time to correct: per function correct at the last, the turn of \
         the first checkpoint from which it is correct at every one. Recovery per wave: of the checkpoints \
         after the wave's release, how many until those of its functions that were correct at the checkpoint \
         before it are all correct again (never, if not by the last; n/a if none was), beside how many those \
         are; its functions are its targets, every function defined in terms of them, directly or not, and \
         their open contracts' members. \
         Breakage: a function correct at one checkpoint and wrong at the next, after a wave if one was \
         released between them. Tokens per correct function: the run's tokens over the final count.",
    ),
];

/// Tools that integrate work, and tools that validate it, in any mode.
pub const INTEGRATE: &[&str] =
    &["sync", "merge", "rebase", "conflicts", "pr_open", "pr_list", "pr_view", "pr_comment", "pr_merge", "pr_close"];
pub const VALIDATE: &[&str] = &["check", "evidence", "pr_review"];

/// Every task function's definition in `modules`, as [`DEFINITIONS`] says. A line that doesn't parse is skipped.
pub fn definitions(task: Task, modules: &[(String, String)]) -> BTreeMap<String, String> {
    let mut lines: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for (_, text) in modules {
        for raw in text.lines() {
            for d in parse_module("line", raw).unwrap_or_default() {
                let line = raw.split('#').next().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
                let calls = d.body.iter().filter_map(|t| if let Token::Call(n) = t { Some(n.clone()) } else { None });
                lines.entry(d.name.clone()).or_default().push((line, calls.collect()));
            }
        }
    }
    let mut out = BTreeMap::new();
    for s in task.specs() {
        let Some(own) = lines.get(s.name) else { continue };
        let mut text: Vec<String> = own.iter().map(|(l, _)| l.clone()).collect();
        let (mut seen, mut helpers) = (BTreeSet::new(), Vec::new());
        let mut todo: Vec<String> = own.iter().flat_map(|(_, c)| c.clone()).collect();
        while let Some(n) = todo.pop() {
            if task.spec_named(&n).is_some() || !seen.insert(n.clone()) {
                continue;
            }
            for (l, c) in lines.get(&n).into_iter().flatten() {
                helpers.push(l.clone());
                todo.extend(c.iter().cloned());
            }
        }
        helpers.sort();
        text.extend(helpers);
        out.insert(s.name.to_string(), text.join("\n"));
    }
    out
}

/// Bug reports in `text`: each `f(1, 2) gave 3` where `f` is a task function with that many inputs and a
/// reference implementation (an open contract's members have none, so no single answer to report against).
pub fn bug_reports(task: Task, text: &str) -> Vec<(String, Vec<i64>)> {
    let mut out = Vec::new();
    for (open, _) in text.match_indices('(') {
        let name_start = text[..open].rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let Some(spec) = task.spec_named(&text[name_start..open]).filter(|s| task.contract_of(s.name).is_none()) else {
            continue;
        };
        let Some(close) = text[open..].find(')').map(|i| open + i) else { continue };
        let args: Option<Vec<i64>> = text[open + 1..close].split(',').map(|a| a.trim().parse().ok()).collect();
        let rest = &text[close + 1..];
        if let Some(args) = args.filter(|a| a.len() == usize::from(spec.arity))
            && let Some(after) = rest.strip_prefix(" gave ")
            && after.split(|c: char| !(c == '-' || c.is_ascii_digit())).next().is_some_and(|n| n.parse::<i64>().is_ok())
        {
            out.push((spec.name.to_string(), args));
        }
    }
    out
}

/// The cap that stopped the run, if the backend stopped it by one of its own (design section 5).
fn stop_cap(records: &Records) -> Option<&str> {
    records.events.iter().find(|e| e.event == "run_stopped").and_then(|e| e.data["cap"].as_str())
}

/// Why `records` can't be measured, if they can't (findings `9fe5861f0b22`, `244614b1c7fe`): a log cut
/// short or with a gap, a stop the plan doesn't `expect`, a snapshot or record that failed, a turn
/// without usage that didn't fail before the model ran, or, for a `task` with waves, a turn without its
/// checkpoint (finding `bf3fde3cf637`). Any of these would make a number wrong without saying so.
pub fn incomplete(records: &Records, expect: &[String], task: Task) -> Option<String> {
    let events = &records.events;
    if let Some((i, e)) = events.iter().enumerate().find(|(i, e)| e.seq != *i as u64) {
        return Some(format!("the log skips from seq {i} to {}", e.seq));
    }
    // A run its own cap stopped ends at the stop, as the plan expects; any other ends with its integrated state.
    let expected = stop_cap(records).filter(|c| expect.iter().any(|x| x == c));
    for e in events {
        let what = match e.event.as_str() {
            "run_stopped" if expected.is_none() => "the run stopped, and not by a stop the plan expects",
            "snapshot_failed" => "a snapshot of the hub failed",
            "checkpoint_failed" => "a checkpoint of the hub failed",
            "tool_result" if e.data["record"].get("error").is_some() => "a store failed to record a tool call",
            "sync" if e.data["record"].get("error").is_some() => "a store failed to record a sync",
            _ => continue,
        };
        return Some(format!("{what} (seq {})", e.seq));
    }
    let last = if expected.is_some() { "run_stopped" } else { "integrated" };
    if events.last().is_none_or(|e| e.event != last) {
        return Some(format!("the log doesn't end with the run's {last} event: it was cut short"));
    }
    let counted: BTreeSet<(&str, u64)> = records
        .usage
        .iter()
        .filter(|r| ["message", "output", "output_counted_at"].iter().any(|k| r.get(*k).is_some()))
        .map(|r| (r["agent"].as_str().unwrap_or_default(), u(&r["turn"])))
        .collect();
    // A turn that failed before or instead of playing (a failed sync, a spent budget) used no tokens:
    // the backend writes a row whenever Claude Code ran.
    if !task.waves().is_empty() {
        let checked: BTreeSet<u64> =
            events.iter().filter(|e| e.event == "checkpoint").map(|e| u(&e.data["turn"])).collect();
        let started: BTreeSet<u64> =
            events.iter().filter(|e| e.event == "turn_start").map(|e| u(&e.data["turn"])).collect();
        let agents = events.iter().find(|e| e.event == "setup").map_or(0, |e| u(&e.data["agents"]));
        let ended = |turn: u64| events.iter().filter(|e| e.event == "turn_end" && u(&e.data["turn"]) == turn).count();
        // A checkpoint follows a turn's last end, so a turn that a stop cut short, which some agent never
        // ended, has none: any such turn, under a stop the plan expects (reviews `45e1d3a1411d`, `243145a557ea`).
        let cut_short = |turn: u64| expected.is_some() && (ended(turn) as u64) < agents;
        if let Some(turn) = started.difference(&checked).find(|t| !cut_short(**t)) {
            return Some(format!("turn {turn} has no checkpoint"));
        }
    }
    let unplayed: BTreeSet<(&str, u64)> = events
        .iter()
        .filter(|e| e.event == "turn_error" && e.data["step"] == "play")
        .map(|e| (e.agent.as_deref().unwrap_or_default(), u(&e.data["turn"])))
        .collect();
    events
        .iter()
        .filter(|e| e.event == "turn_start")
        .map(|e| (e.agent.as_deref().unwrap_or_default(), u(&e.data["turn"])))
        .find(|t| !counted.contains(t) && !unplayed.contains(t))
        .map(|(agent, turn)| format!("{agent}'s turn {turn} has no usage row"))
}

/// One line of the event log.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ms: u64,
    #[serde(default)]
    pub agent: Option<String>,
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

/// A run's records: the event log and the usage rows.
pub struct Records {
    pub events: Vec<Event>,
    pub usage: Vec<Value>,
}

impl Records {
    pub fn parse(events: &str, usage: &str) -> Result<Records, String> {
        let lines = |text: &str| text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect::<Vec<_>>();
        let events = lines(events)
            .iter()
            .map(|l| serde_json::from_str(l).map_err(|e| format!("a bad event: {e}")))
            .collect::<Result<Vec<Event>, _>>()?;
        let usage = lines(usage)
            .iter()
            .map(|l| serde_json::from_str(l).map_err(|e| format!("a bad usage row: {e}")))
            .collect::<Result<Vec<Value>, _>>()?;
        Ok(Records { events, usage })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Improvements {
    pub gained: u64,
    pub lost: u64,
    pub net: i64,
    pub per_million_tokens: Option<f64>,
    pub per_million_output: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Duplication {
    pub deliveries: u64,
    pub visible: u64,
    pub blind: u64,
    pub output_share: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Overhead {
    pub integrate_tokens: Option<f64>,
    pub validate_tokens: Option<f64>,
    pub integrate_time: Option<f64>,
    pub validate_time: Option<f64>,
    pub failed_integrations: u64,
}

/// How long discoveries took to reach one stage.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stage {
    pub reached: u64,
    pub median_seconds: Option<f64>,
    pub median_turns: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Discovery {
    pub discoveries: u64,
    pub bug_reports: u64,
    pub available: Stage,
    pub integrated: Stage,
    pub taken_up: Stage,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Incompatible {
    pub broken_in_library: u64,
    pub changes: u64,
    pub unparsable: u64,
    /// None: not this mode (CRDT).
    pub semantic_conflicts: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Quality {
    pub correct: u64,
    pub wrong: u64,
    pub missing: u64,
    pub total: u64,
}

/// Phase 2's ranking measures, from the checkpoints ([`DEFINITIONS`], `checkpoints`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Checkpoints {
    /// Per checkpoint, in turn order.
    pub turns: Vec<Checkpoint>,
    /// Correct at the last checkpoint, of all functions.
    pub correct: u64,
    pub total: u64,
    /// Per function correct at the last checkpoint, the turn from which it is correct at every one.
    pub correct_from: BTreeMap<String, u32>,
    pub median_turn_correct: Option<f64>,
    pub recovery: Vec<Recovery>,
    pub breakage: Breakage,
    pub tokens_per_correct: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Checkpoint {
    pub turn: u32,
    /// Waves released before it.
    pub waves: u64,
    /// Right in the library, and right standalone.
    pub correct: u64,
    pub standalone: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Recovery {
    pub wave: u64,
    /// The functions that must be correct again.
    pub functions: u64,
    /// Checkpoints after the release until they all are, counting that one; none if never, or if there
    /// are none (`functions` is 0: nothing to recover).
    pub checkpoints: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Breakage {
    pub after_wave: u64,
    pub other: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Measures {
    pub mode: String,
    /// The backend's own cap that stopped the run early, if one did.
    pub stopped: Option<String>,
    pub tokens: u64,
    pub output_tokens: u64,
    pub snapshots: u64,
    pub improvements: Improvements,
    pub duplication: Duplication,
    pub overhead: Overhead,
    pub discovery: Discovery,
    pub incompatible: Incompatible,
    pub quality: Quality,
    /// Phase 2's ranking measures; Phase 1's runs logged no checkpoints, so theirs have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<Checkpoints>,
}

/// When something happened: its place in the log and the harness's clock.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    seq: u64,
    ms: u64,
}

struct Snap {
    at: Point,
    defs: BTreeMap<String, String>,
    library: Library,
    /// Graded under the version in force at it.
    grades: Vec<FunctionGrade>,
    /// The snapshot before, graded under this one's version.
    before: Vec<FunctionGrade>,
}

/// One message's usage (input, cache reads, cache writes, output) and the tools it asked for.
type Message<'a> = ([u64; 4], BTreeSet<&'a str>);

/// A useful discovery: its maker, when it was made, available, and integrated.
type Discovered = (String, Point, Option<Point>, Option<Point>);

struct Delivery {
    at: Point,
    agent: String,
    turn: u64,
    function: String,
    def: String,
    correct: bool,
    available: Option<Point>,
    /// For an open contract's member, the working copy it is judged with ([`working_copy`]).
    copy: Copy,
    /// The judging point ([`Analysis::seen_at`]), and the specs it is judged under; tests check that the copy's
    /// hub is from the same point.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the tests' invariant"))]
    seen: Option<u64>,
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the tests' invariant"))]
    version: Version,
}

/// What an open contract's member is judged with (decision `979e5085a1e7`): the delivering agent's working
/// copy of the contracts when the delivery becomes available to others, and the integrated library for the
/// members the copy doesn't hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Copy {
    /// The snapshot whose library holds what the working copy doesn't.
    base: Option<usize>,
    /// Each other member the copy holds, with the definitions it may be: one in Git and CRDT, any of the
    /// agent's current implementations in Bracken.
    choices: Vec<(String, Vec<String>)>,
}

/// The most combinations of a Bracken agent's current implementations a member is judged with, newest
/// first.
const MAX_CHOICES: usize = 256;

/// The working copy delivery `i` is judged with, if it is an open contract's member, per mode:
/// - CRDT: the agent's own edits that the sync delivered;
/// - Git: the branch's members at the commit (its record);
/// - Bracken: for each member accepted in the hub, that one, else any of the agent's own current
///   implementations (its records' latest list before the sync that makes the delivery available), as the
///   agent's check resolves;
///
/// and in every mode, for the rest, the hub as `seen`, the delivery's judging point, left it.
fn working_copy(a: &Analysis, mode: &str, records: &CopyRecords, found: &[Found], i: usize, seen: Option<u64>) -> Copy {
    let task = a.hidden.task();
    let (at, agent, _, function, _, _) = &found[i];
    let Some(contract) = task.contract_of(function) else { return Copy::default() };
    let others = |f: &str| f != function && contract.members.contains(&f);
    // In every mode the hub part of the copy is the hub as the agent saw it at the judging point.
    let base = a.hub_seen(agent, seen);
    match mode {
        // The agent's document as it synced: the hub at its sync before, with its own edits, which are what
        // this sync delivered.
        "crdt" => {
            let choices = found
                .iter()
                .filter(|(p, by, _, f, _, _)| p.seq == at.seq && by == agent && others(f))
                .map(|(_, _, _, f, d, _)| (f.clone(), vec![d.clone()]))
                .collect();
            Copy { base, choices }
        }
        "git" => {
            let held = records.members_at.get(&at.seq).cloned().unwrap_or_default();
            let choices = held.into_iter().filter(|(f, _)| others(f)).map(|(f, d)| (f, vec![d])).collect();
            Copy { base, choices }
        }
        _ => {
            // The agent's own members as it published them: its records before the sync that makes the
            // delivery available, so a batch delivered at one sync stays together.
            let synced = a.next_sync(agent, at.seq).map_or(u64::MAX, |p| p.seq);
            let integrated = |f: &str| base.is_some_and(|k| a.snaps[k].library.get(f).is_some());
            let own = records.own.get(agent).and_then(|r| r.iter().rev().find(|(seq, _)| *seq < synced));
            let mut choices: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (f, d) in own.map(|(_, held)| held.as_slice()).unwrap_or_default() {
                if others(f) && !integrated(f) {
                    choices.entry(f.clone()).or_default().push(d.clone());
                }
            }
            Copy { base, choices: choices.into_iter().collect() }
        }
    }
}

/// The working copies the records state: Git's per commit, by event, and each Bracken agent's own current
/// implementations of members, in the order its calls and syncs recorded them, each list oldest first.
#[derive(Default)]
struct CopyRecords {
    members_at: BTreeMap<u64, Held>,
    own: BTreeMap<String, Vec<(u64, Held)>>,
}

/// Definitions a working copy holds, by function.
type Held = Vec<(String, String)>;

/// A delivery as found in the log: when, by whom, in which turn, what, and when it is available to others.
type Found = (Point, String, u64, String, String, Option<Point>);

/// The tool an event names: the log quotes it as `` `name` ``.
fn tool_of(e: &Event) -> &str {
    e.data["tool"].as_str().unwrap_or_default().trim_matches('`')
}

/// A tool's own name: Claude Code's usage rows call it `mcp__<server>__<name>`.
fn bare(tool: &str) -> &str {
    tool.strip_prefix("mcp__").and_then(|t| t.split_once("__")).map_or(tool, |(_, name)| name)
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or_default()
}

fn share(part: f64, whole: f64) -> Option<f64> {
    (whole > 0.0).then(|| part / whole)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    (n > 0).then(|| if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// A definition's calls resolved in it first, then in a library.
struct Over<'a> {
    top: &'a Library,
    base: &'a Library,
}

impl Calls for Over<'_> {
    fn resolve(&self, name: &str) -> Option<Target<'_>> {
        self.top.get(name).or_else(|| self.base.get(name)).map(Target::Code)
    }
}

/// Whether `function`'s definition in `top` (else `base`) gives a wrong answer or fails on `args`.
fn fails(task: Task, top: &Library, base: &Library, function: &str, args: &[i64], version: Version) -> bool {
    let calls = Over { top, base };
    let Some(Target::Code(def)) = calls.resolve(function) else { return false };
    run(def, args, &calls) != Ok(task.reference(function, args, version))
}

struct Analysis<'a> {
    hidden: &'a HiddenCases,
    /// Each release's event, and the specs in force after it.
    releases: Vec<(u64, Version)>,
    snaps: Vec<Snap>,
    /// For each snapshot a sync took, the syncing agent.
    snap_by: Vec<Option<String>>,
    syncs: Vec<(Point, String)>,
    starts: Vec<(Point, String, u64)>,
    /// By function, definition, version and, for an open contract's member, the working copy it is judged with.
    correct: BTreeMap<(String, String, Version, Copy), bool>,
}

impl Analysis<'_> {
    fn version_at(&self, seq: u64) -> Version {
        self.releases.iter().rev().find(|(r, _)| *r < seq).map_or(Version::ORIGINAL, |(_, v)| *v)
    }

    /// The point `agent`'s work at `seq` is judged at (decision `5d049d7e4996`): its last sync before `seq`,
    /// or `None` before its first. Both the specs it is judged under ([`Analysis::version_seen`]) and the
    /// hub its working copy falls back on ([`Analysis::hub_seen`]) come from this one point, so they can't
    /// drift apart (finding `0b115f0c0ec0`).
    fn seen_at(&self, agent: &str, seq: u64) -> Option<u64> {
        self.syncs.iter().rev().find(|(p, a)| a == agent && p.seq < seq).map(|(p, _)| p.seq)
    }

    /// The specs released as of `seen`, a point from [`Analysis::seen_at`]: what the agent could see, as
    /// its own check sees them. Phase 1, which has no waves, keeps the specs in force at `seq`, the work's
    /// own point, as its runs were measured.
    fn version_seen(&self, seen: Option<u64>, seq: u64) -> Version {
        if self.hidden.task().waves().is_empty() {
            return self.version_at(seq);
        }
        seen.map_or(Version::ORIGINAL, |p| self.version_at(p))
    }

    /// The hub as `agent` saw it at `seen`, a point from [`Analysis::seen_at`]: the snapshot that sync took,
    /// else the last one before it (the sync changed nothing); before any sync, the setup snapshot. A wave
    /// can't fall between a sync and its snapshot, since the driver holds the hub across both.
    fn hub_seen(&self, agent: &str, seen: Option<u64>) -> Option<usize> {
        let Some(p) = seen else { return (!self.snaps.is_empty()).then_some(0) };
        let next = self.next_sync(agent, p).map_or(u64::MAX, |n| n.seq);
        let own = self
            .snaps
            .iter()
            .enumerate()
            .position(|(k, s)| s.at.seq > p && s.at.seq < next && self.snap_by[k].as_deref() == Some(agent));
        own.or_else(|| self.snaps.iter().rposition(|s| s.at.seq < p))
    }

    fn snap_before(&self, seq: u64) -> Option<&Snap> {
        self.snaps.iter().rev().find(|s| s.at.seq < seq)
    }

    /// Whether `def` is a correct definition of `function` under `version`: standalone, and for an open
    /// contract's member with some choice of the working copy `copy` holds, over its base library.
    fn correct(&mut self, function: &str, def: &str, version: Version, copy: &Copy) -> bool {
        let task = self.hidden.task();
        let copy = if task.contract_of(function).is_some() { copy.clone() } else { Copy::default() };
        let key = (function.to_string(), def.to_string(), version, copy.clone());
        if let Some(&c) = self.correct.get(&key) {
            return c;
        }
        if task.spec_named(function).is_none() {
            return false;
        }
        let base = copy.base.map_or_else(Library::default, |k| self.snaps[k].library.clone());
        let delivery = Library::parse([("delivery", def)]);
        let combinations = copy.choices.iter().map(|(_, d)| d.len()).product::<usize>().min(MAX_CHOICES);
        let mut c = false;
        // Combination 0 takes every member's newest implementation, and the cap cuts off the oldest.
        for mut n in 0..combinations.max(1) {
            let mut library = base.clone();
            for (_, defs) in &copy.choices {
                let newest_first = defs.len() - 1 - n % defs.len();
                library = library.with(&Library::parse([("delivery", defs[newest_first].as_str())]));
                n /= defs.len();
            }
            if grade_function(&library.with(&delivery), function, version, self.hidden).standalone {
                c = true;
                break;
            }
        }
        self.correct.insert(key, c);
        c
    }

    fn next_sync(&self, agent: &str, after: u64) -> Option<Point> {
        self.syncs.iter().find(|(p, a)| a == agent && p.seq > after).map(|(p, _)| *p)
    }

    fn other_sync(&self, agent: &str, after: u64) -> Option<Point> {
        self.syncs.iter().find(|(p, a)| a != agent && p.seq > after).map(|(p, _)| *p)
    }

    fn last_sync_before(&self, agent: &str, before: u64) -> u64 {
        self.syncs.iter().rev().find(|(p, a)| a == agent && p.seq < before).map_or(0, |(p, _)| p.seq)
    }

    fn turn_at(&self, agent: &str, seq: u64) -> u64 {
        self.starts.iter().rev().find(|(p, a, _)| a == agent && p.seq < seq).map_or(0, |(_, _, t)| *t)
    }

    /// Other agents' turns started after `from` up to `to`.
    fn turns_between(&self, agent: &str, from: u64, to: u64) -> u64 {
        self.starts.iter().filter(|(p, a, _)| a != agent && p.seq > from && p.seq <= to).count() as u64
    }

    fn stage(&self, items: &[(String, Point, Option<Point>)]) -> Stage {
        let reached: Vec<_> = items.iter().filter_map(|(a, from, to)| to.map(|t| (a, *from, t))).collect();
        Stage {
            reached: reached.len() as u64,
            median_seconds: median(reached.iter().map(|(_, f, t)| (t.ms - f.ms) as f64 / 1000.0).collect()),
            median_turns: median(reached.iter().map(|(a, f, t)| self.turns_between(a, f.seq, t.seq) as f64).collect()),
        }
    }
}

/// The six measures of one run.
pub fn measure(records: &Records, hidden: &HiddenCases) -> Result<Measures, String> {
    let task = hidden.task();
    let events = &records.events;
    let point = |e: &Event| Point { seq: e.seq, ms: e.ms };
    let (mut a, mode, deliveries) = analyse(records, hidden)?;
    // Tokens, per turn. Step 6's backend writes a row per assistant event, and each content block of a
    // message is an event repeating the message's usage (finding `9543c1272ea0`): a message counts once,
    // at its largest figures, asking for the tools of all its rows.
    let mut turns: BTreeMap<(String, u64), (u64, u64)> = BTreeMap::new();
    let mut messages: BTreeMap<(String, u64, String), Message> = BTreeMap::new();
    let mut counted_at: BTreeMap<(String, u64), u64> = BTreeMap::new();
    for r in &records.usage {
        let key = (r["agent"].as_str().unwrap_or_default().to_string(), u(&r["turn"]));
        let input = u(&r["input"]) + u(&r["cache_read"]) + u(&r["cache_write"]);
        if let Some(id) = r.get("message") {
            let m = messages.entry((key.0, key.1, id.to_string())).or_default();
            let fields = ["input", "cache_read", "cache_write", "output_at_least"];
            for (most, field) in m.0.iter_mut().zip(fields) {
                *most = (*most).max(u(&r[field]));
            }
            m.1.extend(r["tools"].as_array().into_iter().flatten().filter_map(Value::as_str).map(bare));
        } else if r.get("output").is_some() {
            turns.insert(key, (input + u(&r["output"]), u(&r["output"])));
        } else if r.get("output_counted_at").is_some() {
            counted_at.insert(key, u(&r["output_counted_at"]));
        }
    }
    let mut streamed: BTreeMap<(String, u64), (u64, u64)> = BTreeMap::new();
    let mut by_tool = [0.0f64; 3];
    for ((agent, turn, _), ([input, read, write, output], tools)) in &messages {
        let s = streamed.entry((agent.clone(), *turn)).or_default();
        (s.0, s.1) = (s.0 + input + read + write, s.1 + output);
        let tokens = (input + read + write + output) as f64;
        if tools.is_empty() {
            by_tool[2] += tokens;
        }
        for t in tools {
            let class = if INTEGRATE.contains(t) {
                0
            } else if VALIDATE.contains(t) {
                1
            } else {
                2
            };
            by_tool[class] += tokens / tools.len() as f64;
        }
    }
    // A turn without a settled row: its messages' input side, and its output as counted, even with no
    // message at all (finding `3d74eb494361`).
    let unsettled: BTreeSet<&(String, u64)> = streamed.keys().chain(counted_at.keys()).collect();
    for key in unsettled {
        if !turns.contains_key(key) {
            let (input, output) = streamed.get(key).copied().unwrap_or_default();
            let out = counted_at.get(key).copied().unwrap_or(output);
            turns.insert(key.clone(), (input + out, out));
        }
    }
    let tokens: u64 = turns.values().map(|t| t.0).sum();
    let output_tokens: u64 = turns.values().map(|t| t.1).sum();
    let per_million = |n: i64, of: u64| (of > 0).then(|| n as f64 / (of as f64 / 1e6));

    // 1. Improvements.
    let mut improvements = Improvements::default();
    for s in a.snaps.iter().skip(1) {
        for (now, was) in s.grades.iter().zip(&s.before) {
            match (was.in_library, now.in_library) {
                (false, true) => improvements.gained += 1,
                (true, false) => improvements.lost += 1,
                _ => {}
            }
        }
    }
    improvements.net = improvements.gained as i64 - improvements.lost as i64;
    improvements.per_million_tokens = per_million(improvements.net, tokens);
    improvements.per_million_output = per_million(improvements.net, output_tokens);

    // 2. Duplicated work.
    let mut duplication = Duplication { deliveries: deliveries.len() as u64, ..Duplication::default() };
    let mut wasted: BTreeSet<(String, u64)> = BTreeSet::new();
    for d in &deliveries {
        let others = || deliveries.iter().filter(|o| o.agent != d.agent && o.function == d.function);
        let visible = others().any(|o| o.correct && o.available.is_some_and(|p| p.seq < d.at.seq));
        let since = a.last_sync_before(&d.agent, d.at.seq);
        let blind = !visible && others().any(|o| o.at.seq > since && o.at.seq < d.at.seq);
        if visible {
            duplication.visible += 1;
        } else if blind {
            duplication.blind += 1;
        }
        if visible || blind {
            wasted.insert((d.agent.clone(), d.turn));
        }
    }
    let wasted_output: u64 = wasted.iter().filter_map(|k| turns.get(k)).map(|t| t.1).sum();
    duplication.output_share = share(wasted_output as f64, output_tokens as f64);

    // 3. Integration and validation overhead.
    let all_tokens: f64 = by_tool.iter().sum();
    let mut time = [0u64; 3];
    let mut open: BTreeMap<&str, (u64, &str)> = BTreeMap::new();
    let mut failed_integrations = 0;
    let mut turn_time = 0;
    let mut started: BTreeMap<&str, u64> = BTreeMap::new();
    for e in events {
        let Some(agent) = e.agent.as_deref() else { continue };
        let tool = tool_of(e);
        match e.event.as_str() {
            "turn_start" => {
                started.insert(agent, e.ms);
            }
            "turn_end" => turn_time += started.remove(agent).map_or(0, |s| e.ms - s),
            "tool_call" => {
                open.insert(agent, (e.ms, tool));
            }
            "tool_result" => {
                if let Some((from, tool)) = open.remove(agent) {
                    let class = if INTEGRATE.contains(&tool) {
                        0
                    } else if VALIDATE.contains(&tool) {
                        1
                    } else {
                        2
                    };
                    time[class] += e.ms - from;
                }
                let ok = e.data["ok"].as_bool().unwrap_or(false);
                if (INTEGRATE.contains(&tool) && !ok)
                    || e.data["record"]["conflict"] == true
                    || (tool == "pr_close" && ok)
                {
                    failed_integrations += 1;
                }
            }
            _ => {}
        }
    }
    let overhead = Overhead {
        integrate_tokens: share(by_tool[0], all_tokens),
        validate_tokens: share(by_tool[1], all_tokens),
        integrate_time: share(time[0] as f64, turn_time as f64),
        validate_time: share(time[1] as f64, turn_time as f64),
        failed_integrations,
    };

    // 4. Time until a useful discovery is available to others.
    let mut items: Vec<Discovered> = Vec::new();
    for d in deliveries.iter().filter(|d| d.correct) {
        let integrated = a.snaps.iter().find(|s| s.at.seq >= d.at.seq && s.defs.get(&d.function) == Some(&d.def));
        items.push((d.agent.clone(), d.at, d.available, integrated.map(|s| s.at)));
    }
    let mut reported: BTreeSet<(String, Vec<i64>)> = BTreeSet::new();
    let mut bug_reports = 0;
    for e in events.iter().filter(|e| e.event == "tool_call") {
        let Some(agent) = &e.agent else { continue };
        for r in e.data["reports"].as_array().into_iter().flatten() {
            let (Some(f), Some(args)) = (r[0].as_str(), r[1].as_array()) else { continue };
            let args: Vec<i64> = args.iter().filter_map(Value::as_i64).collect();
            let version = a.version_at(e.seq);
            let empty = Library::default();
            let base = a.snap_before(e.seq).map_or(&empty, |s| &s.library);
            let latest = deliveries.iter().rev().find(|d| d.function == f && d.at.seq < e.seq);
            let latest = latest.map(|d| Library::parse([("delivery", d.def.as_str())]));
            let real = fails(task, &empty, base, f, &args, version)
                || latest.as_ref().is_some_and(|l| l.get(f).is_some() && fails(task, l, base, f, &args, version));
            if real && reported.insert((f.to_string(), args)) {
                bug_reports += 1;
                let tool = tool_of(e);
                let available = if tool.starts_with("pr_") { Some(point(e)) } else { a.next_sync(agent, e.seq) };
                items.push((agent.clone(), point(e), available, None));
            }
        }
    }
    let stage = |pick: &dyn Fn(&Discovered) -> Option<Point>| {
        a.stage(&items.iter().map(|i| (i.0.clone(), i.1, pick(i))).collect::<Vec<_>>())
    };
    let discovery = Discovery {
        discoveries: items.len() as u64,
        bug_reports,
        available: stage(&|i| i.2),
        integrated: stage(&|i| i.3),
        taken_up: stage(&|i| i.2.and_then(|p| a.other_sync(&i.0, p.seq))),
    };

    // 5. Semantically incompatible contributions.
    let mut incompatible = Incompatible::default();
    for k in 1..a.snaps.len() {
        let (prev, s) = (&a.snaps[k - 1], &a.snaps[k]);
        let broken: Vec<_> = task
            .specs()
            .iter()
            .zip(s.grades.iter().zip(&s.before))
            .filter(|(spec, (now, was))| {
                prev.defs.contains_key(spec.name)
                    && prev.defs.get(spec.name) == s.defs.get(spec.name)
                    && now.standalone
                    && was.in_library
                    && !now.in_library
            })
            .collect();
        incompatible.broken_in_library += broken.len() as u64;
        incompatible.changes += u64::from(!broken.is_empty());
        let parsed = |l: &Library, m: &str| !l.broken.iter().any(|(b, _)| b == m);
        let modules: BTreeSet<&str> = s.library.broken.iter().map(|(m, _)| m.as_str()).collect();
        incompatible.unparsable += u64::from(modules.iter().any(|m| parsed(&prev.library, m)));
    }
    if mode != "crdt" {
        let mut conflicts = 0;
        for e in events.iter().filter(|e| e.event == "tool_result") {
            let blocks: Vec<(String, String)> =
                serde_json::from_value(e.data["record"]["blocks"].clone()).unwrap_or_default();
            let giver = e.agent.as_deref().unwrap_or_default();
            let seen = a.seen_at(giver, e.seq);
            let version = a.version_seen(seen, e.seq);
            // A blocked definition is judged under the specs the verdict-giver could see, with the working copy its
            // latest delivery before the verdict had, else with the other blocked definitions over the library
            // integrated then. Only an open contract's member has a working copy, so anything else is judged by
            // its own code alone.
            let judged = |a: &mut Analysis, (f, d): &(String, String)| {
                let delivered = deliveries.iter().rev().find(|x| x.at.seq < e.seq && x.function == *f && x.def == *d);
                let copy = match delivered {
                    Some(x) => x.copy.clone(),
                    None => Copy {
                        base: a.hub_seen(giver, seen),
                        choices: blocks
                            .iter()
                            .filter(|(g, _)| g != f)
                            .map(|(g, d)| (g.clone(), vec![d.clone()]))
                            .collect(),
                    },
                };
                a.correct(f, d, version, &copy)
            };
            if !blocks.is_empty() && blocks.iter().all(|b| judged(&mut a, b)) {
                conflicts += 1;
            }
        }
        incompatible.semantic_conflicts = Some(conflicts);
    }

    // 6. Quality of the final artifact.
    let mut quality = Quality { total: task.specs().len() as u64, ..Quality::default() };
    let last = a.snaps.last().map(|s| grade(&s.library, task.all(), hidden)).unwrap_or_default();
    for g in &last {
        match (g.present, g.in_library) {
            (_, true) => quality.correct += 1,
            (true, false) => quality.wrong += 1,
            (false, false) => quality.missing += 1,
        }
    }
    quality.missing += quality.total - last.len() as u64;

    let checkpoints = checkpoints(&a, events, tokens)?;
    Ok(Measures {
        stopped: stop_cap(records).map(str::to_string),
        mode,
        tokens,
        output_tokens,
        snapshots: a.snaps.len() as u64,
        improvements,
        duplication,
        overhead,
        discovery,
        incompatible,
        quality,
        checkpoints,
    })
}

/// The functions wave `wave` changes, as recovery counts them: its targets, every spec defined in terms of
/// one of them, directly or not, and the members of their open contracts.
fn changed_by(task: Task, wave: usize) -> BTreeSet<&'static str> {
    let mut set: BTreeSet<&'static str> = BTreeSet::new();
    for c in task.wave_changes(wave) {
        set.insert(c.target);
        set.extend(task.contract_of(c.target).map(|k| k.members.iter().copied()).into_iter().flatten());
    }
    loop {
        let callers: Vec<&'static str> = task
            .specs()
            .iter()
            .filter(|s| !set.contains(s.name) && task.defined_by(s.name).iter().any(|d| set.contains(d)))
            .map(|s| s.name)
            .collect();
        if callers.is_empty() {
            return set;
        }
        set.extend(callers);
    }
}

/// Phase 2's ranking measures, if the log has checkpoints.
fn checkpoints(a: &Analysis, events: &[Event], tokens: u64) -> Result<Option<Checkpoints>, String> {
    let task = a.hidden.task();
    let mut graded: Vec<(u32, u64, BTreeSet<&'static str>)> = Vec::new();
    let mut turns = Vec::new();
    for e in events.iter().filter(|e| e.event == "checkpoint") {
        let modules: Vec<(String, String)> =
            serde_json::from_value(e.data["modules"].clone()).map_err(|e| format!("a bad checkpoint: {e}"))?;
        let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
        let grades = grade(&library, a.version_at(e.seq), a.hidden);
        let correct: BTreeSet<&'static str> = grades.iter().filter(|g| g.in_library).map(|g| g.name).collect();
        let turn = u(&e.data["turn"]) as u32;
        let waves = a.releases.iter().filter(|(r, _)| *r < e.seq).count() as u64;
        let standalone = grades.iter().filter(|g| g.standalone).count() as u64;
        turns.push(Checkpoint { turn, waves, correct: correct.len() as u64, standalone });
        graded.push((turn, e.seq, correct));
    }
    let Some((_, _, last)) = graded.last() else { return Ok(None) };
    let mut correct_from = BTreeMap::new();
    for f in last {
        let from = graded.iter().rposition(|(_, _, c)| !c.contains(f)).map_or(0, |k| k + 1);
        correct_from.insert(f.to_string(), graded[from].0);
    }
    let median_turn_correct = median(correct_from.values().map(|t| f64::from(*t)).collect());
    let recovery = a
        .releases
        .iter()
        .enumerate()
        .map(|(k, (r, _))| {
            // Correct again: those of the wave's functions that were correct at the checkpoint before it.
            let before = graded.iter().rev().find(|(_, seq, _)| seq < r).map(|(_, _, c)| c);
            let set: BTreeSet<&str> =
                changed_by(task, k + 1).into_iter().filter(|f| before.is_some_and(|c| c.contains(f))).collect();
            let after = graded.iter().filter(|(_, seq, _)| seq > r);
            let checkpoints = after
                .enumerate()
                .find(|(_, (_, _, c))| set.is_subset(c))
                .map(|(n, _)| n as u64 + 1)
                .filter(|_| !set.is_empty());
            Recovery { wave: k as u64 + 1, functions: set.len() as u64, checkpoints }
        })
        .collect();
    let mut breakage = Breakage::default();
    for pair in graded.windows(2) {
        let ((_, from, was), (_, to, now)) = (&pair[0], &pair[1]);
        let broken = was.difference(now).count() as u64;
        if a.releases.iter().any(|(r, _)| from < r && r < to) {
            breakage.after_wave += broken;
        } else {
            breakage.other += broken;
        }
    }
    let total = task.specs().len() as u64;
    let correct = last.len() as u64;
    let tokens_per_correct = (correct > 0).then(|| tokens as f64 / correct as f64);
    Ok(Some(Checkpoints {
        turns,
        correct,
        total,
        correct_from,
        median_turn_correct,
        recovery,
        breakage,
        tokens_per_correct,
    }))
}

/// The log read for the measures: the analysis, the mode, and every delivery, judged.
fn analyse<'h>(records: &Records, hidden: &'h HiddenCases) -> Result<(Analysis<'h>, String, Vec<Delivery>), String> {
    let task = hidden.task();
    let events = &records.events;
    let mode = events
        .iter()
        .find(|e| e.event == "setup")
        .and_then(|e| e.data["mode"].as_str())
        .ok_or("the log has no setup event")?
        .to_string();
    let point = |e: &Event| Point { seq: e.seq, ms: e.ms };
    let mut a = Analysis {
        hidden,
        // Phase 1's one release names no wave.
        releases: events
            .iter()
            .filter(|e| e.event == "changes_released")
            .map(|e| (e.seq, task.after_wave(e.data["wave"].as_u64().map_or(1, |w| w as usize))))
            .collect(),
        snaps: Vec::new(),
        snap_by: Vec::new(),
        syncs: events.iter().filter(|e| e.event == "sync").filter_map(|e| Some((point(e), e.agent.clone()?))).collect(),
        starts: events
            .iter()
            .filter(|e| e.event == "turn_start")
            .filter_map(|e| Some((point(e), e.agent.clone()?, u(&e.data["turn"]))))
            .collect(),
        correct: BTreeMap::new(),
    };

    // Graded snapshots.
    for e in events.iter().filter(|e| e.event == "snapshot") {
        let modules: Vec<(String, String)> =
            serde_json::from_value(e.data["modules"].clone()).map_err(|e| format!("a bad snapshot: {e}"))?;
        let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
        let version = a.version_at(e.seq);
        let grades = grade(&library, version, hidden);
        let before = match a.snaps.last() {
            None => grades.clone(),
            Some(prev) if a.version_at(prev.at.seq) == version => prev.grades.clone(),
            Some(prev) => grade(&prev.library, version, hidden),
        };
        let from_sync = e.data["after"] == "sync";
        a.snap_by.push(e.agent.clone().filter(|_| from_sync));
        a.snaps.push(Snap { at: point(e), defs: definitions(task, &modules), library, grades, before });
    }

    // Deliveries.
    let mut found: Vec<Found> = Vec::new();
    if mode == "crdt" {
        for (k, agent) in a.snap_by.iter().enumerate().skip(1) {
            let Some(agent) = agent else { continue };
            let (old, new) = (&a.snaps[k - 1], &a.snaps[k]);
            for (f, def) in &new.defs {
                if old.defs.get(f) != Some(def) {
                    let turn = a.turn_at(agent, new.at.seq);
                    found.push((new.at, agent.clone(), turn, f.clone(), def.clone(), Some(new.at)));
                }
            }
        }
    } else {
        for e in events.iter().filter(|e| e.event == "tool_result") {
            let Some(agent) = &e.agent else { continue };
            for w in e.data["record"]["writes"].as_array().into_iter().flatten() {
                let (Some(f), Some(def)) = (w[0].as_str(), w[1].as_str()) else { continue };
                if a.snap_before(e.seq).and_then(|s| s.defs.get(f)).map(String::as_str) != Some(def) {
                    let available = a.next_sync(agent, e.seq);
                    found.push((point(e), agent.clone(), u(&e.data["turn"]), f.into(), def.into(), available));
                }
            }
        }
    }
    let mut copies = CopyRecords::default();
    for e in events.iter().filter(|e| e.event == "tool_result" || e.event == "sync") {
        let Some(members) = e.data["record"].get("members") else { continue };
        let held: Held = serde_json::from_value(members.clone()).unwrap_or_default();
        match (mode.as_str(), &e.agent) {
            ("git", _) => drop(copies.members_at.insert(e.seq, held)),
            (_, Some(agent)) => copies.own.entry(agent.clone()).or_default().push((e.seq, held)),
            _ => {}
        }
    }
    let mut deliveries = Vec::new();
    for (i, (at, agent, turn, function, def, available)) in found.iter().cloned().enumerate() {
        // A CRDT delivery is the snapshot after the sync that delivered it: the work was made before that sync.
        let made = if mode == "crdt" { a.seen_at(&agent, at.seq).unwrap_or(at.seq) } else { at.seq };
        let seen = a.seen_at(&agent, made);
        let copy = working_copy(&a, &mode, &copies, &found, i, seen);
        let version = a.version_seen(seen, made);
        let correct = a.correct(&function, &def, version, &copy);
        deliveries.push(Delivery { at, agent, turn, function, def, correct, available, copy, seen, version });
    }
    Ok((a, mode, deliveries))
}

/// A table row: its name and how a run's cell reads.
type Row<'a> = (&'static str, Box<dyn Fn(&Measures) -> String + 'a>);

/// The comparison table: one column per run.
pub fn table(runs: &[Measures]) -> String {
    let f = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{v:.2}"));
    let pct = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{:.1}%", v * 100.0));
    let stage = |s: &Stage| format!("{} ({}s, {} turns)", s.reached, f(s.median_seconds), f(s.median_turns));
    let rows: Vec<Row> = vec![
        ("Stopped early by", Box::new(|m| m.stopped.clone().unwrap_or_else(|| "-".into()))),
        ("Tokens (output)", Box::new(|m| format!("{} ({})", m.tokens, m.output_tokens))),
        ("Graded snapshots", Box::new(|m| m.snapshots.to_string())),
        (
            "Improvements: net (+, -)",
            Box::new(|m| format!("{} (+{}, -{})", m.improvements.net, m.improvements.gained, m.improvements.lost)),
        ),
        ("  per million tokens", Box::new(move |m| f(m.improvements.per_million_tokens))),
        ("  per million output tokens", Box::new(move |m| f(m.improvements.per_million_output))),
        ("Deliveries", Box::new(|m| m.duplication.deliveries.to_string())),
        ("Duplicated: visible, blind", Box::new(|m| format!("{}, {}", m.duplication.visible, m.duplication.blind))),
        ("  share of output tokens", Box::new(move |m| pct(m.duplication.output_share))),
        (
            "Overhead tokens: integrate, validate",
            Box::new(move |m| format!("{}, {}", pct(m.overhead.integrate_tokens), pct(m.overhead.validate_tokens))),
        ),
        (
            "Overhead time: integrate, validate",
            Box::new(move |m| format!("{}, {}", pct(m.overhead.integrate_time), pct(m.overhead.validate_time))),
        ),
        ("Failed integrations", Box::new(|m| m.overhead.failed_integrations.to_string())),
        (
            "Useful discoveries (bug reports)",
            Box::new(|m| format!("{} ({})", m.discovery.discoveries, m.discovery.bug_reports)),
        ),
        ("  available", Box::new(move |m| stage(&m.discovery.available))),
        ("  integrated", Box::new(move |m| stage(&m.discovery.integrated))),
        ("  taken up", Box::new(move |m| stage(&m.discovery.taken_up))),
        (
            "Broken in library (changes)",
            Box::new(|m| format!("{} ({})", m.incompatible.broken_in_library, m.incompatible.changes)),
        ),
        ("Modules broken", Box::new(|m| m.incompatible.unparsable.to_string())),
        (
            "Semantic-conflict events",
            Box::new(|m| m.incompatible.semantic_conflicts.map_or("n/a".into(), |n| n.to_string())),
        ),
        (
            "Final: correct, wrong, missing",
            Box::new(|m| {
                let q = &m.quality;
                format!("{}/{}, {}, {}", q.correct, q.total, q.wrong, q.missing)
            }),
        ),
    ];
    // Phase 2: the checkpoint measures rank the modes; the rest are per-mode diagnostics (decision
    // `288a16ce8cb2`). Phase 1's table is as it was.
    let rows = if runs.iter().any(|m| m.checkpoints.is_some()) {
        let cp = |m: &Measures, f: &dyn Fn(&Checkpoints) -> String| m.checkpoints.as_ref().map_or("n/a".into(), f);
        let mut ranked: Vec<Row> = vec![
            (
                "Ranking: checkpoints (turn: correct)",
                Box::new(move |m| {
                    cp(m, &|c| {
                        c.turns.iter().map(|t| format!("{}: {}", t.turn, t.correct)).collect::<Vec<_>>().join(", ")
                    })
                }),
            ),
            ("  final correct", Box::new(move |m| cp(m, &|c| format!("{}/{}", c.correct, c.total)))),
            ("  median turn to correct", Box::new(move |m| cp(m, &|c| f(c.median_turn_correct)))),
            (
                "  recovery per wave (checkpoints)",
                Box::new(move |m| {
                    cp(m, &|c| {
                        let r: Vec<String> = c
                            .recovery
                            .iter()
                            .map(|r| match (r.functions, r.checkpoints) {
                                (0, _) => "n/a (of 0)".to_string(),
                                (k, None) => format!("never (of {k})"),
                                (k, Some(n)) => format!("{n} (of {k})"),
                            })
                            .collect();
                        r.join(", ")
                    })
                }),
            ),
            (
                "  breakage: after a wave, other",
                Box::new(move |m| cp(m, &|c| format!("{}, {}", c.breakage.after_wave, c.breakage.other))),
            ),
            ("  tokens per correct function", Box::new(move |m| cp(m, &|c| f(c.tokens_per_correct)))),
            ("Per-delivery diagnostics, per mode only, not comparable across modes:", Box::new(|_| String::new())),
        ];
        ranked.extend(rows);
        ranked
    } else {
        rows
    };
    let header: Vec<String> = runs.iter().map(|m| m.mode.clone()).collect();
    let cells: Vec<Vec<String>> = rows.iter().map(|(_, r)| runs.iter().map(r).collect()).collect();
    let first = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max("Measure".len());
    let widths: Vec<usize> =
        (0..runs.len()).map(|i| cells.iter().map(|c| c[i].len()).chain([header[i].len()]).max().unwrap_or(0)).collect();
    let line = |name: &str, cols: &[String]| {
        let mut l = format!("| {name:<first$} |");
        for (c, w) in cols.iter().zip(&widths) {
            l.push_str(&format!(" {c:<w$} |"));
        }
        l.push('\n');
        l
    };
    let mut out = line("Measure", &header);
    out.push_str(&line(&"-".repeat(first), &widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>()));
    for ((name, _), c) in rows.iter().zip(&cells) {
        out.push_str(&line(name, c));
    }
    out
}

#[cfg(test)]
mod tests;
