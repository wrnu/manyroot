//! Each measure on a small hand-built log whose answer is known.

use serde_json::{Value, json};

use super::*;
use crate::candidates::{candidates, correct_programs};
use crate::task::spec_named;

fn right(f: &str) -> String {
    def(f, correct_programs(Version::ORIGINAL)[f])
}

fn wrong(f: &str) -> String {
    def(f, candidates(f).wrong[0])
}

fn def(f: &str, program: &str) -> String {
    format!("def {f} {} : {program}", spec_named(f).expect("a spec").arity)
}

/// An event log and usage rows, written as the driver and a backend write them.
struct Log {
    events: Vec<String>,
    usage: Vec<String>,
}

impl Log {
    fn new(mode: &str) -> Log {
        let mut log = Log { events: Vec::new(), usage: Vec::new() };
        log.ev(0, None, "setup", json!({"mode": mode}));
        log
    }

    fn ev(&mut self, ms: u64, agent: Option<&str>, event: &str, data: Value) -> &mut Log {
        let seq = self.events.len();
        self.events.push(json!({"seq": seq, "ms": ms, "agent": agent, "event": event, "data": data}).to_string());
        self
    }

    fn snap(&mut self, ms: u64, agent: Option<&str>, defs: &[String]) -> &mut Log {
        let text: String = defs.iter().map(|d| format!("{d}\n")).collect();
        self.ev(ms, agent, "snapshot", json!({"after": "sync", "modules": [["base", text]]}))
    }

    fn sync(&mut self, ms: u64, agent: &str) -> &mut Log {
        self.ev(ms, Some(agent), "sync", json!({}))
    }

    fn start(&mut self, ms: u64, agent: &str, turn: u64) -> &mut Log {
        self.ev(ms, Some(agent), "turn_start", json!({"turn": turn}))
    }

    fn write(&mut self, ms: u64, agent: &str, turn: u64, d: &str) -> &mut Log {
        let f = d.split_whitespace().nth(1).expect("a def line");
        let data = json!({"turn": turn, "tool": "`commit`", "ok": true, "record": {"writes": [[f, d]]}});
        self.ev(ms, Some(agent), "tool_result", data)
    }

    fn call(&mut self, ms: u64, agent: &str, tool: &str, data: Value) -> &mut Log {
        let mut data = data;
        data["tool"] = json!(format!("`{tool}`"));
        self.ev(ms, Some(agent), "tool_call", data)
    }

    fn result(&mut self, ms: u64, agent: &str, tool: &str, ok: bool, record: Value) -> &mut Log {
        let data = json!({"tool": format!("`{tool}`"), "ok": ok, "record": record});
        self.ev(ms, Some(agent), "tool_result", data)
    }

    fn turn_usage(&mut self, agent: &str, turn: u64, input: u64, output: u64) -> &mut Log {
        let row = json!({"agent": agent, "turn": turn, "ended": "success", "input": input, "cache_read": 0,
            "cache_write": 0, "output": output});
        self.usage.push(row.to_string());
        self
    }

    /// A message of its own, in one row.
    fn message(&mut self, agent: &str, input: u64, tools: &[&str]) -> &mut Log {
        let id = format!("m{}", self.usage.len());
        let row = json!({"agent": agent, "turn": 1, "message": id, "input": input, "cache_read": 0,
            "cache_write": 0, "output_at_least": 0, "tools": tools});
        self.usage.push(row.to_string());
        self
    }

    fn records(&self) -> Records {
        Records::parse(&self.events.join("\n"), &self.usage.join("\n")).expect("records")
    }

    fn measure(&self) -> Measures {
        let records = Records::parse(&self.events.join("\n"), &self.usage.join("\n")).expect("records");
        measure(&records, &HiddenCases::new(1)).expect("measures")
    }
}

#[test]
fn a_definition_holds_its_helpers_without_comments_or_spacing() {
    let modules = vec![
        ("base".to_string(), "def abs 1 :  a 0 < a absneg@ a ?   # the sign flips\ndef absneg 1 : a neg\n".to_string()),
        ("order".to_string(), "def max2 2 : a b > a b ?\nnot a definition\ndef dist 2 : a b - abs@\n".to_string()),
    ];
    let defs = definitions(&modules);
    assert_eq!(defs["abs"], "def abs 1 : a 0 < a absneg@ a ?\ndef absneg 1 : a neg");
    assert_eq!(defs["max2"], "def max2 2 : a b > a b ?");
    // A task function it calls is its own function, not part of this definition.
    assert_eq!(defs["dist"], "def dist 2 : a b - abs@");
    assert_eq!(defs.len(), 3, "helpers aren't task functions");
}

#[test]
fn bug_reports_are_found_in_the_form_check_prints() {
    let text = "check said abs(-3) gave -3, expected 3; max2(1, 2) gave 1 (5/60 cases passed)";
    assert_eq!(bug_reports(text), vec![("abs".into(), vec![-3]), ("max2".into(), vec![1, 2])]);
    for not in ["abs(-3) is wrong", "nosuch(1) gave 2", "max2(1) gave 1", "abs(x) gave 3", "abs(1) gave many"] {
        assert!(bug_reports(not).is_empty(), "{not}");
    }
}

#[test]
fn improvements_are_net_per_million_tokens_and_a_release_is_not_a_regression() {
    let all_sign = def("sign", correct_programs(Version::all())["sign"]);
    let mut log = Log::new("bracken");
    log.snap(1, None, &[])
        .snap(2, Some("a"), &[right("abs")]) // +1
        .snap(3, Some("a"), &[wrong("abs"), right("max2"), right("sign")]) // -1 +1 +1
        .ev(4, None, "changes_released", json!({}))
        // The release makes sign and abs wrong; graded under the new version on both sides, nothing changes.
        .snap(5, None, &[wrong("abs"), right("max2"), right("sign"), right("min2")]) // +1
        .snap(6, Some("b"), &[wrong("abs"), right("max2"), all_sign.clone(), right("min2")]) // +1
        .turn_usage("a", 1, 1_500_000, 500_000);
    let m = log.measure();
    assert_eq!(m.snapshots, 5);
    assert_eq!((m.improvements.gained, m.improvements.lost, m.improvements.net), (5, 1, 4));
    assert_eq!(m.tokens, 2_000_000);
    assert_eq!(m.improvements.per_million_tokens, Some(2.0));
    assert_eq!(m.improvements.per_million_output, Some(8.0));
}

#[test]
fn duplicates_are_visible_or_blind_and_cost_their_turns_output() {
    let mut log = Log::new("git");
    log.snap(0, None, &[])
        .start(1, "a", 1)
        .start(1, "b", 1)
        .start(1, "c", 1)
        .sync(2, "c") // c's last look at the hub
        .write(3, "a", 1, &right("abs"))
        .sync(4, "a") // a's abs is available
        .write(5, "b", 1, &right("abs")) // visible: a's correct abs was available
        .write(6, "a", 1, &wrong("max2")) // not yet available
        .write(7, "c", 1, &right("max2")) // blind: a delivered max2 after c's last sync
        .write(8, "c", 1, &right("sign")) // nobody else's
        .snap(9, Some("a"), &[right("abs")])
        .write(10, "b", 1, &right("abs")) // the integrated definition already: not a delivery
        // Neither: b's min2 was never available, and it came before c's last sync.
        .write(11, "b", 1, &right("min2"))
        .sync(12, "c")
        .write(13, "c", 1, &right("min2"))
        // Neither: a's dec was available but wrong, and came before b's last sync.
        .write(14, "a", 1, &wrong("dec"))
        .sync(15, "a")
        .sync(16, "b")
        .write(17, "b", 1, &right("dec"))
        .turn_usage("a", 1, 0, 100)
        .turn_usage("b", 1, 0, 300)
        .turn_usage("c", 1, 0, 600);
    let d = log.measure().duplication;
    assert_eq!((d.deliveries, d.visible, d.blind), (9, 1, 1));
    assert_eq!(d.output_share, Some(0.9), "b's and c's turns");
}

#[test]
fn overhead_splits_tokens_by_tool_and_times_calls() {
    let mut log = Log::new("git");
    log.start(0, "a", 1)
        .call(100, "a", "sync", json!({}))
        .result(300, "a", "sync", false, Value::Null) // failed
        .call(400, "a", "check", json!({}))
        .result(500, "a", "check", true, Value::Null)
        .call(500, "a", "merge", json!({}))
        .result(600, "a", "merge", true, json!({"conflict": true})) // stopped on a conflict
        .call(600, "a", "pr_close", json!({}))
        .result(650, "a", "pr_close", true, Value::Null) // closed unmerged
        .ev(1000, Some("a"), "turn_end", json!({"turn": 1}))
        .message("a", 1000, &["sync"])
        .message("a", 1000, &["check"])
        .message("a", 2000, &["mcp__experiment__read", "mcp__experiment__sync"]) // as Claude Code names them
        .message("a", 1000, &[]);
    let o = log.measure().overhead;
    assert_eq!((o.integrate_tokens, o.validate_tokens), (Some(0.4), Some(0.2)));
    assert_eq!((o.integrate_time, o.validate_time), (Some(0.35), Some(0.1)));
    assert_eq!(o.failed_integrations, 3);
}

#[test]
fn discoveries_are_timed_to_available_integrated_and_taken_up() {
    let mut log = Log::new("bracken");
    log.snap(0, None, &[wrong("max2")])
        .start(0, "a", 1)
        .write(1000, "a", 1, &right("abs"))
        .start(2000, "b", 1) // one other agent's turn before it's available
        .sync(3000, "a")
        .snap(4000, Some("a"), &[right("abs"), wrong("max2")])
        .sync(5000, "b")
        // A true report, the same one again, and one about a function that's right.
        .call(6000, "b", "evidence", json!({"reports": [["max2", [1, 2]]]}))
        .call(6100, "b", "evidence", json!({"reports": [["max2", [1, 2]]]}))
        .call(6200, "b", "evidence", json!({"reports": [["abs", [-3]]]}))
        .sync(7000, "b")
        .sync(9000, "a");
    let d = log.measure().discovery;
    assert_eq!((d.discoveries, d.bug_reports), (2, 1));
    assert_eq!((d.available.reached, d.available.median_seconds), (2, Some(1.5)), "2 s and 1 s");
    assert_eq!(d.available.median_turns, Some(0.5), "1 turn and 0");
    assert_eq!((d.integrated.reached, d.integrated.median_seconds), (1, Some(3.0)));
    assert_eq!(d.taken_up.median_seconds, Some(3.5), "b at 5 s, a at 9 s");
}

#[test]
fn incompatible_changes_break_an_untouched_caller_and_verdicts_on_correct_code_count() {
    let mut log = Log::new("bracken");
    log.snap(0, None, &[right("abs"), right("dist")])
        .snap(1, Some("a"), &[wrong("abs"), right("dist")]) // dist untouched, right alone, wrong in library
        .snap(1, Some("a"), &[right("abs"), right("dist")])
        // dist breaks in library again, but this change rewrote it: not an incompatibility.
        .snap(1, Some("a"), &[wrong("abs"), def("dist", "b a - abs@")])
        .ev(
            2,
            Some("a"),
            "snapshot",
            json!({"after": "sync", "modules": [["base", "def abs 1 : a\n"], ["order", "oops\n"]]}),
        )
        .result(3, "b", "evidence", true, json!({"blocks": [["abs", right("abs")]]}))
        .result(4, "b", "evidence", true, json!({"blocks": [["abs", wrong("abs")]]}))
        .result(5, "b", "evidence", true, json!({"blocks": []}));
    let i = log.measure().incompatible;
    assert_eq!((i.broken_in_library, i.changes, i.unparsable), (1, 1, 1));
    assert_eq!(i.semantic_conflicts, Some(1));
}

#[test]
fn crdt_deliveries_are_hub_syncs_that_change_a_definition() {
    let mut log = Log::new("crdt");
    log.snap(0, None, &[])
        .start(0, "a", 1)
        .start(0, "b", 1)
        .sync(1, "a")
        .snap(1, Some("a"), &[right("abs")])
        .sync(2, "b")
        .snap(2, Some("b"), &[wrong("abs"), right("max2")])
        // Only a sync credits its agent: not a hub tool's change, nor the setter's release.
        .ev(
            3,
            Some("a"),
            "snapshot",
            json!({"after": "hub_tool", "modules": [["base", format!("{}\n", right("min2"))]]}),
        )
        .ev(3, None, "snapshot", json!({"after": "release", "modules": [["base", format!("{}\n", right("sign"))]]}));
    let m = log.measure();
    assert_eq!((m.duplication.deliveries, m.duplication.visible), (3, 1), "b's abs came after a's was available");
    assert_eq!(m.incompatible.semantic_conflicts, None, "no review step: n/a");
    assert!(table(&[m]).contains("| n/a"));
}

#[test]
fn quality_grades_the_last_snapshot_under_every_change() {
    let mut log = Log::new("git");
    // abs's original program wraps, which the change requests no longer allow.
    log.snap(0, None, &[right("max2"), wrong("min2"), right("abs")]);
    let q = log.measure().quality;
    assert_eq!((q.correct, q.wrong, q.missing, q.total), (1, 2, 27, 30));
}

#[test]
fn a_turn_without_a_settled_row_counts_its_output_at_the_cap() {
    let mut log = Log::new("git");
    log.turn_usage("a", 1, 1000, 200);
    for (message, input, output) in [("m1", 300, 10), ("m2", 400, 20)] {
        let row = json!({"agent": "a", "turn": 2, "message": message, "input": input, "cache_read": 0,
            "cache_write": 0, "output_at_least": output, "tools": ["read"]});
        log.usage.push(row.to_string());
    }
    log.usage.push(json!({"agent": "a", "turn": 2, "ended": "Cap", "output_counted_at": 30000}).to_string());
    // A turn that ended before its first message is charged its cap all the same (finding `3d74eb494361`).
    log.usage.push(json!({"agent": "a", "turn": 3, "ended": "NoResult", "output_counted_at": 30000}).to_string());
    let m = log.measure();
    assert_eq!((m.tokens, m.output_tokens), (1200 + 700 + 30000 + 30000, 200 + 30000 + 30000));
}

#[test]
fn a_message_in_several_rows_counts_once_with_all_its_tools() {
    // The backend's rows for one message: a text block, a tool call and more text, each repeating its usage
    // (finding `9543c1272ea0`).
    let mut log = Log::new("git");
    for tools in [json!([]), json!(["mcp__experiment__sync"]), json!([])] {
        let row = json!({"agent": "a", "turn": 1, "message": "m1", "input": 1000, "cache_read": 0,
            "cache_write": 0, "output_at_least": 0, "tools": tools});
        log.usage.push(row.to_string());
    }
    log.usage.push(json!({"agent": "a", "turn": 1, "ended": "Cap", "output_counted_at": 30000}).to_string());
    let m = log.measure();
    assert_eq!(m.tokens, 31000);
    assert_eq!(m.overhead.integrate_tokens, Some(1.0), "the message asked only for sync");
}

/// A whole run's records: setup, one turn with usage, the closing integrated event.
fn whole() -> Log {
    let mut log = Log::new("git");
    log.start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1})).ev(3, None, "integrated", json!({}));
    log.turn_usage("a", 1, 10, 1);
    log
}

fn refused(log: &Log) -> String {
    incomplete(&log.records(), &[]).expect("refused")
}

#[test]
fn a_whole_run_is_measured() {
    assert_eq!(incomplete(&whole().records(), &[]), None);
}

#[test]
fn a_log_with_a_gap_is_refused() {
    let mut log = whole();
    log.events.remove(1);
    assert!(refused(&log).contains("skips from seq 1 to 2"));
}

#[test]
fn a_log_cut_short_is_refused() {
    let mut log = whole();
    log.events.pop();
    assert!(refused(&log).contains("cut short"));
}

#[test]
fn a_stopped_run_is_refused() {
    let mut log = whole();
    log.ev(4, None, "run_stopped", json!({})).ev(5, None, "integrated", json!({}));
    assert!(refused(&log).contains("stopped"));
}

#[test]
fn a_failed_snapshot_is_refused() {
    let mut log = whole();
    log.ev(4, Some("a"), "snapshot_failed", json!({})).ev(5, None, "integrated", json!({}));
    assert!(refused(&log).contains("snapshot"));
}

#[test]
fn a_failed_record_is_refused() {
    let mut log = whole();
    log.result(4, "a", "commit", true, json!({"error": "a tree over the limit"})).ev(5, None, "integrated", json!({}));
    assert!(refused(&log).contains("record"));
}

#[test]
fn a_turn_without_usage_is_refused() {
    let mut log = whole();
    log.usage.clear();
    // A gate row carries no tokens.
    log.usage.push(json!({"agent": "a", "turn": 1, "gate": true}).to_string());
    assert!(refused(&log).contains("a's turn 1 has no usage row"));
}

/// A turn by `a` that failed at play (`turn_error` step play) and wrote no usage.
fn unplayed(log: &mut Log, ms: u64, turn: u64) {
    log.start(ms, "a", turn).ev(ms, Some("a"), "turn_error", json!({"turn": turn, "step": "play"})).ev(
        ms,
        Some("a"),
        "turn_end",
        json!({"turn": turn}),
    );
}

#[test]
fn a_turn_that_failed_before_playing_needs_no_usage() {
    // A failed sync before the turn: the backend never ran (finding `244614b1c7fe`).
    let mut log = Log::new("git");
    unplayed(&mut log, 1, 1);
    log.ev(2, None, "integrated", json!({}));
    assert_eq!(incomplete(&log.records(), &[]), None);
    // A turn that ended at the play step after usage: that turn's row is still required.
    let mut log = Log::new("git");
    log.start(1, "a", 1).ev(1, Some("a"), "turn_error", json!({"turn": 1, "step": "sync"}));
    log.ev(2, None, "integrated", json!({}));
    assert!(incomplete(&log.records(), &[]).expect("refused").contains("no usage row"));
}

#[test]
fn a_turn_refused_for_a_spent_budget_counts_nothing() {
    let mut log = Log::new("git");
    log.start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    log.usage.push(
        json!({"agent": "a", "turn": 1, "ended": "Cap(\"agent tokens\")", "output_counted_at": 30000}).to_string(),
    );
    unplayed(&mut log, 3, 2);
    log.ev(4, None, "integrated", json!({}));
    assert_eq!(incomplete(&log.records(), &[]), None);
    assert_eq!((log.measure().tokens, log.measure().output_tokens), (30000, 30000));
}

#[test]
fn a_run_stopped_by_its_own_cap_is_measured_only_if_the_plan_expects_it() {
    let cap = crate::claude::RUN_TOKEN_CAP;
    let mut log = Log::new("git");
    log.snap(0, None, &[right("max2")]).start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    log.turn_usage("a", 1, 10, 1).ev(3, None, "run_stopped", json!({"cap": cap}));
    let expected = [cap.to_string()];
    assert_eq!(incomplete(&log.records(), &expected), None, "measured from what exists");
    let m = log.measure();
    assert_eq!((m.stopped.as_deref(), m.quality.correct), (Some(cap), 1));
    assert!(incomplete(&log.records(), &[]).expect("refused").contains("not by a stop the plan expects"));
    // Any other stop is never named, so never expected.
    let mut other = Log::new("git");
    other.start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    other.turn_usage("a", 1, 10, 1).ev(3, None, "run_stopped", json!({"cap": null}));
    assert!(incomplete(&other.records(), &expected).is_some());
}
