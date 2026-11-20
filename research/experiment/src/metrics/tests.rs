//! Each measure on a small hand-built log whose answer is known.

use serde_json::{Value, json};

use super::*;
use crate::candidates::{candidates, correct_programs};
use crate::task::Task;

fn right(f: &str) -> String {
    def(f, correct_programs(Task::Phase1, Version::ORIGINAL)[f])
}

fn wrong(f: &str) -> String {
    def(f, candidates(Task::Phase1, f).wrong[0])
}

fn def(f: &str, program: &str) -> String {
    format!("def {f} {} : {program}", Task::Phase1.spec_named(f).expect("a spec").arity)
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
        measure(&records, &HiddenCases::new(Task::Phase1, 1)).expect("measures")
    }
}

#[test]
fn a_definition_holds_its_helpers_without_comments_or_spacing() {
    let modules = vec![
        ("base".to_string(), "def abs 1 :  a 0 < a absneg@ a ?   # the sign flips\ndef absneg 1 : a neg\n".to_string()),
        ("order".to_string(), "def max2 2 : a b > a b ?\nnot a definition\ndef dist 2 : a b - abs@\n".to_string()),
    ];
    let defs = definitions(Task::Phase1, &modules);
    assert_eq!(defs["abs"], "def abs 1 : a 0 < a absneg@ a ?\ndef absneg 1 : a neg");
    assert_eq!(defs["max2"], "def max2 2 : a b > a b ?");
    // A task function it calls is its own function, not part of this definition.
    assert_eq!(defs["dist"], "def dist 2 : a b - abs@");
    assert_eq!(defs.len(), 3, "helpers aren't task functions");
}

#[test]
fn bug_reports_are_found_in_the_form_check_prints() {
    let text = "check said abs(-3) gave -3, expected 3; max2(1, 2) gave 1 (5/60 cases passed)";
    assert_eq!(bug_reports(Task::Phase1, text), vec![("abs".into(), vec![-3]), ("max2".into(), vec![1, 2])]);
    for not in ["abs(-3) is wrong", "nosuch(1) gave 2", "max2(1) gave 1", "abs(x) gave 3", "abs(1) gave many"] {
        assert!(bug_reports(Task::Phase1, not).is_empty(), "{not}");
    }
}

#[test]
fn improvements_are_net_per_million_tokens_and_a_release_is_not_a_regression() {
    let all_sign = def("sign", correct_programs(Task::Phase1, Task::Phase1.all())["sign"]);
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
    incomplete(&log.records(), &[], Task::Phase1).expect("refused")
}

#[test]
fn a_whole_run_is_measured() {
    assert_eq!(incomplete(&whole().records(), &[], Task::Phase1), None);
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
    assert_eq!(incomplete(&log.records(), &[], Task::Phase1), None);
    // A turn that ended at the play step after usage: that turn's row is still required.
    let mut log = Log::new("git");
    log.start(1, "a", 1).ev(1, Some("a"), "turn_error", json!({"turn": 1, "step": "sync"}));
    log.ev(2, None, "integrated", json!({}));
    assert!(incomplete(&log.records(), &[], Task::Phase1).expect("refused").contains("no usage row"));
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
    assert_eq!(incomplete(&log.records(), &[], Task::Phase1), None);
    assert_eq!((log.measure().tokens, log.measure().output_tokens), (30000, 30000));
}

#[test]
fn a_run_stopped_by_its_own_cap_is_measured_only_if_the_plan_expects_it() {
    let cap = crate::backend::RUN_TOKEN_CAP;
    let mut log = Log::new("git");
    log.snap(0, None, &[right("max2")]).start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    log.turn_usage("a", 1, 10, 1).ev(3, None, "run_stopped", json!({"cap": cap}));
    let expected = [cap.to_string()];
    assert_eq!(incomplete(&log.records(), &expected, Task::Phase1), None, "measured from what exists");
    let m = log.measure();
    assert_eq!((m.stopped.as_deref(), m.quality.correct), (Some(cap), 1));
    assert!(incomplete(&log.records(), &[], Task::Phase1).expect("refused").contains("not by a stop the plan expects"));
    // Any other stop is never named, so never expected.
    let mut other = Log::new("git");
    other.start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    other.turn_usage("a", 1, 10, 1).ev(3, None, "run_stopped", json!({"cap": null}));
    assert!(incomplete(&other.records(), &expected, Task::Phase1).is_some());
}

/// Note `ba986b386c7a`: a wave none of whose functions was correct before it has nothing to recover. Its
/// recovery is n/a, never the best score, and the table prints how many functions each wave's counts.
#[test]
fn a_wave_with_nothing_correct_before_it_has_no_recovery() {
    let t = Task::Phase2;
    let hidden = HiddenCases::new(t, 1);
    let text = |programs: &crate::candidates::Programs| -> Vec<(String, String)> {
        crate::candidates::modules(t, programs).into_iter().map(|(m, s)| (m.to_string(), s)).collect()
    };
    // Turn 1 has only max2, which neither wave changes; both waves go out before turn 2, which has everything.
    let original = correct_programs(t, Version::ORIGINAL);
    let max2: crate::candidates::Programs =
        original.iter().filter(|(f, _)| **f == "max2").map(|(f, p)| (*f, *p)).collect();
    assert!(!changed_by(t, 1).contains("max2") && !changed_by(t, 2).contains("max2"));
    let mut log = Log::new("git");
    log.snap(1, None, &[]);
    log.ev(2, None, "checkpoint", json!({"turn": 1, "modules": text(&max2)}));
    log.ev(3, None, "changes_released", json!({"wave": 1}));
    log.ev(4, None, "changes_released", json!({"wave": 2}));
    log.ev(5, None, "checkpoint", json!({"turn": 2, "modules": text(&correct_programs(t, t.after_wave(2)))}));
    log.turn_usage("agent-0", 1, 1_000, 10);
    let m = measure(&log.records(), &hidden).expect("measures");
    let c = m.checkpoints.clone().expect("checkpoints");
    let none = |wave| Recovery { wave, functions: 0, checkpoints: None };
    assert_eq!(c.recovery, [none(1), none(2)], "all correct at turn 2, yet nothing was recovered");
    assert!(table(std::slice::from_ref(&m)).contains("| n/a (of 0), n/a (of 0) "));
    // With something to recover, the count is beside it.
    let c = measure(&checkpoint_log("git").records(), &hidden).expect("measures");
    assert!(table(std::slice::from_ref(&c)).contains("| never (of 34) "));
}

/// Task `81b0ce075a56` (decision `a8ba06399068` point 4): recovery's "before" is the hub when the wave went
/// out, its last snapshot before the release graded under the specs before it, not the last checkpoint.
/// Here the checkpoint before wave 1 holds only max2, which no wave changes, but a sync then lands the whole
/// library, so all 34 of wave 1's functions were correct when it went out. The snapshot logged after the
/// release, which here drops them as Bracken's accepted layer does for superseded specs, isn't the before.
#[test]
fn recovery_counts_from_the_hub_when_the_wave_went_out() {
    let t = Task::Phase2;
    let hidden = HiddenCases::new(t, 1);
    let text = |programs: &crate::candidates::Programs| -> Vec<(String, String)> {
        crate::candidates::modules(t, programs).into_iter().map(|(m, s)| (m.to_string(), s)).collect()
    };
    let original = correct_programs(t, Version::ORIGINAL);
    let max2: crate::candidates::Programs =
        original.iter().filter(|(f, _)| **f == "max2").map(|(f, p)| (*f, *p)).collect();
    let mut log = Log::new("git");
    log.snap(1, None, &[]);
    log.ev(2, None, "snapshot", json!({"after": "sync", "modules": text(&max2)}));
    log.ev(3, None, "checkpoint", json!({"turn": 1, "modules": text(&max2)}));
    log.ev(4, None, "snapshot", json!({"after": "sync", "modules": text(&original)}));
    log.ev(5, None, "changes_released", json!({"wave": 1}));
    log.ev(6, None, "snapshot", json!({"after": "release", "modules": text(&max2)}));
    let wave1 = text(&correct_programs(t, t.after_wave(1)));
    log.ev(7, None, "snapshot", json!({"after": "sync", "modules": wave1}));
    log.ev(8, None, "checkpoint", json!({"turn": 2, "modules": wave1}));
    log.turn_usage("agent-0", 1, 1_000, 10);
    let c = measure(&log.records(), &hidden).expect("measures").checkpoints.expect("checkpoints");
    assert_eq!(c.recovery, [Recovery { wave: 1, functions: 34, checkpoints: Some(1) }]);
}

/// Decision `a8ba06399068` point 3: beside the ranking, never in it, CRDT mode's checkpoints regraded with
/// identical copies collapsed. Here the document holds the whole original library plus a second, identical
/// copy of max2 and a different copy of mix. The ranking grades it as the grader does, with neither (nor
/// anything calling them); the diagnostic as the library with only the different copy of mix: max2 and its
/// callers count again, mix and its callers don't. Git's log of the same checkpoint has no diagnostic: n/a.
#[test]
fn crdt_has_a_diagnostic_with_identical_copies_collapsed_beside_the_ranking() {
    let t = Task::Phase2;
    let hidden = HiddenCases::new(t, 1);
    let mut modules: Vec<(String, String)> = crate::candidates::modules(t, &correct_programs(t, Version::ORIGINAL))
        .into_iter()
        .map(|(m, s)| (m.to_string(), s))
        .collect();
    let line = |f: &str| {
        modules.iter().flat_map(|(_, s)| s.lines()).find(|l| l.starts_with(&format!("def {f} "))).map(str::to_string)
    };
    let (max2, mix) = (line("max2").expect("max2"), line("mix").expect("mix"));
    let other_mix = format!("{} 0 +", mix);
    let base = modules.clone();
    modules.push(("copies".to_string(), format!("{max2}\n{other_mix}\n")));
    let correct = |extra: &str| {
        let mut m = base.clone();
        m.push(("copies".to_string(), extra.to_string()));
        let library = Library::parse(m.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        grade(&library, Version::ORIGINAL, &hidden).iter().filter(|g| g.in_library).count() as u64
    };
    let (ranked, collapsed) = (correct(&format!("{max2}\n{other_mix}\n")), correct(&format!("{other_mix}\n")));
    assert!(ranked < collapsed && collapsed < 90, "{ranked}, {collapsed}");
    let log_of = |mode: &str| {
        let mut log = Log::new(mode);
        log.snap(1, None, &[]);
        log.ev(2, None, "checkpoint", json!({"turn": 1, "modules": modules}));
        log.turn_usage("agent-0", 1, 1_000, 10);
        measure(&log.records(), &hidden).expect("measures")
    };
    let crdt = log_of("crdt");
    let c = crdt.checkpoints.clone().expect("checkpoints");
    assert_eq!((c.turns[0].correct, c.correct), (ranked, ranked), "the ranking keeps the grader's rule");
    assert_eq!(c.crdt_identical_collapsed, Some(vec![(1, collapsed)]));
    let git = log_of("git");
    assert_eq!(git.checkpoints.clone().expect("checkpoints").crdt_identical_collapsed, None);
    let t = table(&[crdt, git]);
    let row = t.lines().find(|l| l.starts_with("| Diagnostic, not ranked: CRDT")).expect("the row");
    let cells: Vec<&str> = row.split('|').map(str::trim).collect();
    assert_eq!(cells[2..4], [format!("1: {collapsed}").as_str(), "n/a"], "{t}");
    let rank = t.lines().position(|l| l.starts_with("| Ranking: checkpoints")).expect("ranking");
    let per_delivery = t.lines().position(|l| l.starts_with("| Per-delivery diagnostics")).expect("diagnostics");
    let here = t.lines().position(|l| l == row).expect("row");
    assert!(rank < here && here < per_delivery, "printed beside the ranking, after it: {t}");
}

/// Finding `bf3fde3cf637`: Phase 2 ranks by checkpoints, so a turn without one, or one that can't be read,
/// can't be measured.
#[test]
fn a_phase2_turn_without_a_readable_checkpoint_is_refused() {
    let p2 = Task::Phase2;
    assert_eq!(incomplete(&whole().records(), &[], Task::Phase1), None, "Phase 1 has none");
    assert_eq!(incomplete(&whole().records(), &[], p2).as_deref(), Some("turn 1 has no checkpoint"));
    let mut log = Log::new("git");
    log.start(1, "a", 1).ev(2, Some("a"), "turn_end", json!({"turn": 1}));
    log.ev(3, None, "checkpoint", json!({"turn": 1, "modules": []})).start(4, "a", 2);
    log.ev(5, Some("a"), "turn_end", json!({"turn": 2})).ev(6, None, "integrated", json!({}));
    log.turn_usage("a", 1, 10, 1).turn_usage("a", 2, 10, 1);
    assert_eq!(incomplete(&log.records(), &[], p2).as_deref(), Some("turn 2 has no checkpoint"));
    // An expected stop excuses only a turn it cut short: this one ended, so its checkpoint is missing.
    let cap = crate::backend::RUN_TOKEN_CAP;
    log.events.pop();
    log.ev(6, None, "run_stopped", json!({"cap": cap}));
    assert_eq!(incomplete(&log.records(), &[cap.to_string()], p2).as_deref(), Some("turn 2 has no checkpoint"));
    // A checkpoint that isn't a module list is an error, as a bad snapshot is.
    let mut bad = Log::new("git");
    bad.ev(1, None, "checkpoint", json!({"turn": 1, "modules": "not modules"}));
    let error = measure(&bad.records(), &HiddenCases::new(p2, 1)).expect_err("refused");
    assert!(error.contains("a bad checkpoint"), "{error}");
}

/// Reviews `45e1d3a1411d` and `243145a557ea`: an expected stop can catch the agents spread over several
/// turns. Each turn it cut short, one that some agent never ended, has no checkpoint and needs none.
#[test]
fn an_expected_stop_excuses_every_turn_it_cut_short() {
    let cap = crate::backend::RUN_TOKEN_CAP;
    let expected = [cap.to_string()];
    // Three agents: all end turn 1; turn 2 is ended by a and b only; turn 3, by a only. Then the cap stops c
    // in turn 2 and b in turn 3.
    let spread = |checkpoint_2: bool, c_ends_2: bool| {
        let mut log = Log::new("git");
        log.events[0] =
            json!({"seq": 0, "ms": 0, "agent": null, "event": "setup", "data": {"mode": "git", "agents": 3}})
                .to_string();
        let mut ms = 1;
        let mut step = |log: &mut Log, agent: Option<&str>, event: &str, data: Value| {
            log.ev(ms, agent, event, data);
            ms += 1;
        };
        for (turn, agents, ends) in [(1, "abc", "abc"), (2, "abc", if c_ends_2 { "abc" } else { "ab" }), (3, "ab", "a")]
        {
            for a in agents.chars().map(String::from) {
                step(&mut log, Some(&a), "turn_start", json!({"turn": turn}));
                log.turn_usage(&a, turn, 10, 1);
            }
            for a in ends.chars().map(String::from) {
                step(&mut log, Some(&a), "turn_end", json!({"turn": turn}));
            }
            if turn == 1 || (turn == 2 && checkpoint_2) {
                step(&mut log, None, "checkpoint", json!({"turn": turn, "modules": []}));
            }
        }
        step(&mut log, None, "run_stopped", json!({"cap": cap}));
        log
    };
    let p2 = Task::Phase2;
    assert_eq!(incomplete(&spread(false, false).records(), &expected, p2), None, "turns 2 and 3 were cut short");
    assert_eq!(incomplete(&spread(true, true).records(), &expected, p2), None);
    // Turn 2 ended for everyone, so its checkpoint should be there.
    let missing = spread(false, true);
    assert_eq!(incomplete(&missing.records(), &expected, p2).as_deref(), Some("turn 2 has no checkpoint"));
    // A stop the plan doesn't expect excuses nothing.
    assert!(incomplete(&spread(false, false).records(), &[], p2).expect("refused").contains("not by a stop"));
}

// Open contracts: a member is judged with the delivering agent's working copy (decision `979e5085a1e7`).

fn phase2(f: &str, program: &str) -> String {
    format!("def {f} {} : {program}", Task::Phase2.spec_named(f).expect("a spec").arity)
}

/// The rank contract's five members in one convention: the first correct programs', or the other one.
fn rank_contract(other: bool) -> Vec<String> {
    let canonical = correct_programs(Task::Phase2, Version::ORIGINAL);
    let other_choice: BTreeMap<_, _> = crate::candidates::phase2::OTHER_CHOICE.iter().copied().collect();
    ["rank", "flip", "isless", "issame", "ismore"]
        .iter()
        .map(|f| phase2(f, if other { other_choice[f] } else { canonical[f] }))
        .collect()
}

fn name_of(d: &str) -> String {
    d.split_whitespace().nth(1).expect("a def line").to_string()
}

/// One step of a scenario, played as each mode logs it. As in a run, where every turn starts with a sync, an
/// agent sees others' work only once it has synced.
#[derive(Clone)]
enum Step {
    /// An agent writes definitions: one Git commit, Bracken artifacts, CRDT edits to its document.
    Write(&'static str, Vec<String>),
    /// An agent syncs: in CRDT its document and the hub's merge, its edits winning.
    Sync(&'static str),
    /// Git and Bracken: the agent's work is integrated (merged, or accepted after a pass).
    Integrate(&'static str),
    /// Git: the agent merges main into its branch. Bracken and CRDT have nothing to do.
    Adopt(&'static str),
    /// The driver releases a wave. In Bracken it supersedes the spec claims of the wave's targets, so the
    /// hub's accepted implementations of them stop counting at once, and an agent's own at its next sync.
    Release(usize),
    /// An agent's verdict blocks definitions.
    Block(&'static str, Vec<String>),
}

/// The log `steps` leave in `mode`, with the records each mode's stores write.
fn play(mode: &str, steps: &[Step]) -> Log {
    use Step::*;
    let mut log = Log::new(mode);
    let mut ms = 10;
    let mut hub: BTreeMap<String, String> = BTreeMap::new();
    // Git: each agent's branch; Bracken: its own current implementations; CRDT: its document and unsynced edits.
    let mut copy: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    let mut edits: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    // Bracken: the targets of every change released so far, and how many of them each agent has synced.
    let mut superseded: Vec<&str> = Vec::new();
    let mut synced_upto: BTreeMap<&str, usize> = BTreeMap::new();
    let defs = |m: &BTreeMap<String, String>| m.values().cloned().collect::<Vec<_>>();
    log.snap(ms, None, &[]);
    for step in steps {
        ms += 10;
        match step {
            Write(agent, ds) => match mode {
                "git" => {
                    let branch = copy.entry(agent).or_default();
                    let mut writes = Vec::new();
                    for d in ds {
                        branch.retain(|(f, _)| *f != name_of(d));
                        branch.push((name_of(d), d.clone()));
                        writes.push(json!([name_of(d), d]));
                    }
                    let record = json!({"writes": writes, "members": branch});
                    log.ev(
                        ms,
                        Some(agent),
                        "tool_result",
                        json!({"turn": 1, "tool": "`commit`", "ok": true, "record": record}),
                    );
                }
                "bracken" => {
                    for (k, d) in ds.iter().enumerate() {
                        let own = copy.entry(agent).or_default();
                        own.push((name_of(d), d.clone()));
                        let record = json!({"writes": [[name_of(d), d]], "members": own});
                        let data = json!({"turn": 1, "tool": "`artifact`", "ok": true, "record": record});
                        log.ev(ms + k as u64, Some(agent), "tool_result", data);
                    }
                }
                _ => edits.entry(agent).or_default().extend(ds.iter().map(|d| (name_of(d), d.clone()))),
            },
            Sync(agent) => {
                if mode == "bracken" {
                    // The store records the agent's own current members after every sync.
                    let from = synced_upto.insert(agent, superseded.len()).unwrap_or(0);
                    let own = copy.entry(agent).or_default();
                    own.retain(|(f, _)| !superseded[from..].contains(&f.as_str()));
                    log.ev(ms, Some(agent), "sync", json!({"record": {"members": own}}));
                } else {
                    log.sync(ms, agent);
                }
                if mode == "crdt" {
                    hub.extend(edits.remove(agent).unwrap_or_default());
                    log.snap(ms + 1, Some(agent), &defs(&hub));
                }
            }
            Integrate(agent) => {
                // Bracken accepts each function's earliest implementation with a pass.
                for (f, d) in copy.get(agent).cloned().unwrap_or_default() {
                    if mode == "git" || !hub.contains_key(&f) {
                        hub.insert(f, d);
                    }
                }
                log.snap(ms, None, &defs(&hub));
            }
            Adopt(agent) => {
                if mode == "git" {
                    copy.insert(agent, hub.clone().into_iter().collect());
                }
            }
            Release(wave) => {
                log.ev(ms, None, "changes_released", json!({"wave": wave}));
                let targets: Vec<&str> = Task::Phase2.wave_changes(*wave).map(|c| c.target).collect();
                if mode == "bracken" {
                    hub.retain(|f, _| !targets.contains(&f.as_str()));
                    log.ev(ms + 1, None, "snapshot", json!({"after": "release", "modules": [["base", defs(&hub).iter().map(|d| format!("{d}\n")).collect::<String>()]]}));
                }
                superseded.extend(targets);
            }
            Block(agent, ds) => {
                let blocks: Vec<_> = ds.iter().map(|d| json!([name_of(d), d])).collect();
                log.result(ms, agent, "evidence", true, json!({"blocks": blocks}));
            }
        }
    }
    log
}

/// The decision's cases, each played in all three modes: the correct deliveries the measures count, Git's,
/// Bracken's and CRDT's, and Git's and Bracken's semantic conflicts.
#[test]
fn every_contract_case_is_judged_by_the_agents_working_copy_in_every_mode() {
    use Step::*;
    let (canonical, other) = (rank_contract(false), rank_contract(true));
    let mut mixed = other.clone();
    mixed[1..].clone_from_slice(&canonical[1..]);
    let adapted = phase2("isless", "a 0 < 0 +");
    let kept = phase2("isless", "a 5 = 0 +");
    let revised = phase2("isless", "0 a >");
    let after_wave_2 = correct_programs(Task::Phase2, Task::Phase2.after_wave(2));
    let wide: Vec<String> = ["rank", "ismore"].iter().map(|f| phase2(f, after_wave_2[f])).collect();
    let lost = |isless: &String| {
        vec![
            Write("a", other.clone()),
            Sync("a"),
            Write("b", canonical.clone()),
            Sync("b"),
            Integrate("b"),
            Sync("a"),
            Adopt("a"),
            Write("a", vec![isless.clone()]),
            Sync("a"),
        ]
    };
    let cases: Vec<(&str, Vec<Step>, [u64; 3], u64)> = vec![
        ("a whole contract at once", vec![Write("a", canonical.clone()), Sync("a")], [5, 5, 5], 0),
        ("adapting after losing", lost(&adapted), [11, 11, 11], 0),
        ("keeping the losing convention after losing", lost(&kept), [10, 10, 10], 0),
        (
            "a revision in a later turn",
            vec![Write("a", canonical.clone()), Sync("a"), Write("a", vec![revised.clone()]), Sync("a")],
            [6, 6, 6],
            0,
        ),
        (
            "a contract split over two turns: rank and flip have no readers when they reach others",
            vec![Write("a", canonical[..2].to_vec()), Sync("a"), Write("a", canonical[2..].to_vec()), Sync("a")],
            [3, 3, 3],
            0,
        ),
        (
            "a same-turn rewrite: the first version isn't judged against the second (CRDT never delivers it)",
            vec![Write("a", other.clone()), Write("a", canonical.clone()), Sync("a")],
            [10, 10, 5],
            0,
        ),
        (
            "wave 1 between delivery and verdict: it doesn't change rank, so the block of a correct member is a \
             semantic conflict",
            vec![Write("a", canonical.clone()), Sync("a"), Release(1), Block("b", vec![canonical[2].clone()])],
            [5, 5, 5],
            1,
        ),
        (
            "wave 2 between delivery and a verdict whose giver has synced it in (decision 5d049d7e4996): \
             rank-wide and ismore-far make a - b and its readers wrong, so blocking ismore is no conflict",
            vec![
                Write("a", canonical.clone()),
                Sync("a"),
                Release(1),
                Release(2),
                Sync("b"),
                Block("b", vec![canonical[4].clone()]),
            ],
            [5, 5, 5],
            0,
        ),
        (
            "wave 2 before a verdict whose giver hasn't synced since: it judges by the specs it can see, the \
             original, so blocking a correct ismore is a semantic conflict",
            vec![
                Write("a", canonical.clone()),
                Sync("a"),
                Release(1),
                Release(2),
                Block("b", vec![canonical[4].clone()]),
            ],
            [5, 5, 5],
            1,
        ),
        (
            "wave 1 between delivery and the sync that makes it available: it changes no rank member",
            vec![Write("a", canonical.clone()), Release(1), Sync("a")],
            [5, 5, 5],
            0,
        ),
        // Finding a3535d9c71b7's three probes. The agent wrote before its sync took in the waves, so every
        // mode judges under the specs it could see then, the original.
        (
            "probe 1: write, wave 1, wave 2, sync: the same in every mode",
            vec![Write("a", canonical.clone()), Release(1), Release(2), Sync("a")],
            [5, 5, 5],
            0,
        ),
        (
            "probe 2: another agent's contract accepted, this agent's in the other convention written, both waves, \
             sync: as with no wave (Bracken's copy holds the accepted one, as in the case without a wave)",
            vec![
                Write("b", canonical.clone()),
                Sync("b"),
                Integrate("b"),
                Sync("a"),
                Write("a", other.clone()),
                Release(1),
                Release(2),
                Sync("a"),
            ],
            [10, 5, 10],
            0,
        ),
        (
            "probe 3: a contract split around both waves, one sync: Git judges each commit's branch (rank and flip \
             lack readers at the first), Bracken and CRDT deliver the whole contract at the sync",
            vec![
                Write("a", canonical[..3].to_vec()),
                Release(1),
                Release(2),
                Write("a", canonical[3..].to_vec()),
                Sync("a"),
            ],
            [3, 5, 5],
            0,
        ),
        (
            "partners accepted from another agent, then both waves, then this agent's sync: Bracken takes the hub \
             as its last sync left it, before the waves dropped them, as Git takes main as of that sync",
            vec![
                Write("b", canonical.clone()),
                Sync("b"),
                Integrate("b"),
                Sync("a"),
                Write("a", vec![revised.clone()]),
                Release(1),
                Release(2),
                Sync("a"),
            ],
            [6, 6, 6],
            0,
        ),
        (
            "finding 0b115f0c0ec0: partners accepted and synced, then both waves mid-turn, then a write before \
             the next sync: judged under the specs and the hub the agent's last sync gave it, in every mode",
            vec![
                Write("b", canonical.clone()),
                Sync("b"),
                Integrate("b"),
                Sync("a"),
                Release(1),
                Release(2),
                Write("a", vec![revised.clone()]),
                Sync("a"),
            ],
            [6, 6, 6],
            0,
        ),
        (
            "a write after a sync took in wave 2: judged under wave 2 in every mode, where of a - b's contract only \
             issame is still right",
            vec![Release(1), Release(2), Sync("a"), Write("a", canonical.clone()), Sync("a")],
            [1, 1, 1],
            0,
        ),
        (
            "the contract redone once a sync brings wave 2: the new rank and ismore are right in every mode",
            vec![
                Write("a", canonical.clone()),
                Sync("a"),
                Release(1),
                Release(2),
                Sync("a"),
                Write("a", wide.clone()),
                Sync("a"),
            ],
            [7, 7, 7],
            0,
        ),
        ("a mixed-convention contract stays wrong", vec![Write("a", mixed.clone()), Sync("a")], [0, 0, 0], 0),
        (
            "another agent's contract accepted, this agent's in the other convention pending: wrong in Bracken, \
             where the accepted one is in the copy; right in Git's branch and CRDT's document, as each check says",
            vec![
                Write("b", canonical.clone()),
                Sync("b"),
                Integrate("b"),
                Sync("a"),
                Write("a", other.clone()),
                Sync("a"),
            ],
            [10, 5, 10],
            0,
        ),
    ];
    for (name, steps, want, conflicts) in cases {
        for (k, mode) in ["git", "bracken", "crdt"].into_iter().enumerate() {
            let records = play(mode, &steps).records();
            let hidden = HiddenCases::new(Task::Phase2, 1);
            assert_one_judging_point(&records, &hidden, &format!("{mode}: {name}"));
            let m = measure(&records, &hidden).expect("measures");
            assert_eq!(m.discovery.discoveries, want[k], "{mode}: {name}");
            let want_conflicts = (mode != "crdt").then_some(conflicts);
            assert_eq!(m.incompatible.semantic_conflicts, want_conflicts, "{mode}: {name}");
        }
    }
}

/// Finding `440196ce733f`: a verdict is judged under the version in force when it is given, as before open
/// contracts, even for a definition delivered before the change requests were released. Phase 1 keeps that;
/// Phase 2 refines it (decision `5d049d7e4996`): the version as of the verdict-giver's last sync before it.
#[test]
fn a_verdict_after_the_release_judges_under_the_released_specs() {
    let sign = phase2("sign", correct_programs(Task::Phase2, Version::ORIGINAL)["sign"]);
    let phase2_log = |giver_synced: bool| {
        let mut log = Log::new("bracken");
        log.snap(10, None, &[]);
        log.write(20, "a", 1, &sign);
        log.ev(30, None, "changes_released", json!({"wave": 1}));
        if giver_synced {
            log.sync(35, "b");
        }
        log.result(40, "b", "evidence", true, json!({"blocks": [["sign", sign.clone()]]}));
        let records = log.records();
        measure(&records, &HiddenCases::new(Task::Phase2, 1)).expect("measures").incompatible.semantic_conflicts
    };
    assert_eq!(phase2_log(true), Some(0), "b synced wave 1 in, where sign(0) = 0 is wrong");
    assert_eq!(phase2_log(false), Some(1), "b hasn't, so it blocks a sign right under the specs it sees");

    let mut log = Log::new("bracken");
    log.snap(10, None, &[]);
    log.write(20, "a", 1, &right("sign"));
    log.ev(30, None, "changes_released", json!({}));
    log.result(40, "b", "evidence", true, json!({"blocks": [["sign", right("sign")]]}));
    let m = log.measure();
    assert_eq!(m.incompatible.semantic_conflicts, Some(0), "sign(0) = 0 is wrong once sign-of-zero is released");
    // Before the release, the same block is a semantic conflict.
    let mut log = Log::new("bracken");
    log.snap(10, None, &[]);
    log.write(20, "a", 1, &right("sign"));
    log.result(40, "b", "evidence", true, json!({"blocks": [["sign", right("sign")]]}));
    assert_eq!(log.measure().incompatible.semantic_conflicts, Some(1));
}

/// Each delivery and verdict is judged under the specs in force at it, across both waves: wave 2 changes
/// clamp3's order again, so wave 1's clamp3 is right between the waves and wrong after.
#[test]
fn deliveries_and_verdicts_are_judged_under_each_waves_specs() {
    use Step::*;
    let t = Task::Phase2;
    let clamp3 = |wave: usize| phase2("clamp3", correct_programs(t, t.after_wave(wave))["clamp3"]);
    let (original, first, second) = (clamp3(0), clamp3(1), clamp3(2));
    assert!(original != first && first != second);
    let steps = vec![
        Write("a", vec![original.clone()]), // right
        Sync("a"),
        Release(1),
        Write("b", vec![first.clone()]), // right
        Sync("b"),
        // A fail on the original clamp3 is right once wave 1 is out: no semantic conflict.
        Block("b", vec![original.clone()]),
        Write("a", vec![phase2("clamp3", "a b max2@ dup c < swap c ?")]), // the original order, so wrong
        Sync("a"),
        Release(2),
        Write("b", vec![second.clone()]), // right
        Sync("b"),
        Write("a", vec![first.clone()]), // wave 1's order, so wrong
        Sync("a"),
        // A fail on wave 1's clamp3 after wave 2: no conflict either.
        Block("b", vec![first.clone()]),
    ];
    for mode in ["git", "bracken", "crdt"] {
        let m = measure(&play(mode, &steps).records(), &HiddenCases::new(t, 1)).expect("measures");
        assert_eq!(m.discovery.discoveries, 3, "{mode}");
        assert_eq!(m.incompatible.semantic_conflicts, (mode != "crdt").then_some(0), "{mode}");
    }
    // The same fail before wave 1 blocks a right definition: a semantic conflict.
    let early = vec![Write("a", vec![original.clone()]), Sync("a"), Block("b", vec![original])];
    let m = measure(&play("bracken", &early).records(), &HiddenCases::new(t, 1)).expect("measures");
    assert_eq!(m.incompatible.semantic_conflicts, Some(1));
}

/// Note 6b8e8ce21f94 point 1: past [`MAX_CHOICES`] combinations, the search keeps the newest ones. Here
/// isless's copy holds five implementations of each of four members, 625 combinations, and only the newest
/// of each is right; oldest first, the cap would stop at combination 256 and never reach it.
#[test]
fn the_combination_cap_keeps_the_newest_implementations() {
    use Step::*;
    let canonical = rank_contract(false);
    let mut steps = Vec::new();
    for k in 0..4 {
        let stale = ["rank", "flip", "issame", "ismore"].iter().map(|f| phase2(f, &format!("{}", 100 + k))).collect();
        steps.push(Write("a", stale));
    }
    let others: Vec<String> = canonical.iter().filter(|d| name_of(d) != "isless").cloned().collect();
    steps.push(Write("a", others));
    steps.push(Write("a", vec![canonical[2].clone()]));
    steps.push(Sync("a"));
    assert_eq!(name_of(&canonical[2]), "isless");
    let m = measure(&play("bracken", &steps).records(), &HiddenCases::new(Task::Phase2, 1)).expect("measures");
    assert_eq!(m.discovery.discoveries, 5, "the four newest members, and isless with them");
}

/// Finding `0b115f0c0ec0`: every delivery is judged at one point, the agent's last sync before it (decision
/// `5d049d7e4996`). The specs it is judged under and the hub its working copy falls back on both come from
/// that point, so no wave lies between them.
fn assert_one_judging_point(records: &Records, hidden: &HiddenCases, case: &str) {
    let (a, _, deliveries) = analyse(records, hidden).expect("an analysis");
    for d in &deliveries {
        assert_eq!(d.version, a.version_seen(d.seen, d.at.seq), "{case}: {} at {}", d.function, d.at.seq);
        if hidden.task().contract_of(&d.function).is_none() {
            continue;
        }
        let base = d.copy.base.expect("a hub snapshot");
        assert_eq!(Some(base), a.hub_seen(&d.agent, d.seen), "{case}: {}'s hub is from its judging point", d.function);
        // That snapshot is the hub's state at the judging point: the one its sync took, or the last before it
        // with none after it and before the point (a snapshot is logged only when the hub changes).
        let at = d.seen.unwrap_or(0);
        let taken = a.snaps[base].at.seq;
        let later = a.snaps.get(base + 1).map_or(u64::MAX, |s| s.at.seq);
        assert!(taken > at || later > at, "{case}: {}'s hub is older than its judging point", d.function);
        if hidden.task().waves().is_empty() {
            continue;
        }
        assert_eq!(d.version, d.seen.map_or(Version::ORIGINAL, |p| a.version_at(p)), "{case}: {}", d.function);
    }
}

/// The checkpoints a scenario logs, the same in every mode: Phase 2's ranking measures grade only them.
fn checkpoint_log(mode: &str) -> Log {
    let t = Task::Phase2;
    let text = |programs: &crate::candidates::Programs| -> Vec<(String, String)> {
        crate::candidates::modules(t, programs).into_iter().map(|(m, s)| (m.to_string(), s)).collect()
    };
    let original = correct_programs(t, Version::ORIGINAL);
    let mut without_mix = original.clone();
    without_mix.remove("mix");
    let mut wave1 = correct_programs(t, t.after_wave(1));
    wave1.insert("weigh", original["weigh"]); // weigh-subtracts left undone, so judge stays wrong too
    wave1.insert("total", candidates(t, "total").wrong[0]); // a breakage no wave explains
    let mut log = Log::new(mode);
    // The hub as the driver snapshots it, each time it changes: each checkpoint reads the same library.
    let hub = |log: &mut Log, ms, programs| {
        log.ev(ms, None, "snapshot", json!({"after": "sync", "modules": text(programs)}));
    };
    log.snap(1, None, &[]);
    hub(&mut log, 2, &without_mix);
    log.ev(2, None, "checkpoint", json!({"turn": 1, "modules": text(&without_mix)}));
    hub(&mut log, 3, &original);
    log.ev(3, None, "checkpoint", json!({"turn": 2, "modules": text(&original)}));
    log.ev(4, None, "changes_released", json!({"wave": 1}));
    log.ev(5, None, "checkpoint", json!({"turn": 3, "modules": text(&original)}));
    hub(&mut log, 6, &wave1);
    log.ev(6, None, "checkpoint", json!({"turn": 4, "modules": text(&wave1)}));
    log.turn_usage("agent-0", 1, 900_000, 1_000);
    log
}

/// Decision `288a16ce8cb2`'s measures on a hand-checked scenario, through every mode's log alike:
/// - turn 1: all of the original library but `mix` (89 of 90);
/// - turn 2: all of it (90);
/// - wave 1, then turn 3: the same library, now wrong for 21 functions under wave 1 (69);
/// - turn 4: wave 1's library, except that `weigh` is left undone, so `judge`, which calls it, is wrong in
///   the library though right standalone, and `total` breaks for no wave (87 in library, 88 standalone).
#[test]
fn checkpoints_rank_every_mode_alike_from_its_shared_library() {
    let t = Task::Phase2;
    let hidden = HiddenCases::new(t, 1);
    let original =
        Library::parse(crate::candidates::correct_library(t, Version::ORIGINAL).iter().map(|(m, s)| (*m, s.as_str())));
    let broken_by_wave1: BTreeSet<_> =
        grade(&original, t.after_wave(1), &hidden).into_iter().filter(|g| !g.in_library).map(|g| g.name).collect();
    assert_eq!(broken_by_wave1.len(), 21);
    assert!(
        broken_by_wave1.contains("clamp3") && broken_by_wave1.contains("weigh") && broken_by_wave1.contains("judge")
    );
    assert!(broken_by_wave1.is_subset(&changed_by(t, 1)), "a wave breaks only what it changes");
    assert_eq!(changed_by(t, 1).len(), 34);
    for mode in ["git", "bracken", "crdt"] {
        let m = measure(&checkpoint_log(mode).records(), &hidden).expect("measures");
        let c = m.checkpoints.expect("checkpoints");
        let rows: Vec<(u32, u64, u64, u64)> =
            c.turns.iter().map(|k| (k.turn, k.waves, k.correct, k.standalone)).collect();
        assert_eq!(rows, [(1, 0, 89, 89), (2, 0, 90, 90), (3, 1, 69, 69), (4, 1, 87, 88)], "{mode}");
        assert_eq!((c.correct, c.total), (87, 90), "{mode}: final quality is the last checkpoint");
        // Correct at every checkpoint: from turn 1; mix, from turn 2; fixed after wave 1 (clamp3), from turn 4.
        assert_eq!(c.correct_from["max2"], 1);
        assert_eq!(c.correct_from["mix"], 2);
        assert_eq!(c.correct_from["clamp3"], 4, "{mode}: a function fixed after the wave");
        assert!(!c.correct_from.contains_key("weigh") && !c.correct_from.contains_key("total"));
        assert_eq!(c.correct_from.values().filter(|t| **t == 4).count(), 19, "the 21, less weigh and judge");
        assert_eq!(c.median_turn_correct, Some(1.0));
        assert_eq!(c.recovery, [Recovery { wave: 1, functions: 34, checkpoints: None }], "{mode}: never recovered");
        assert_eq!(c.breakage, Breakage { after_wave: 21, other: 1 }, "{mode}");
        assert_eq!(c.tokens_per_correct, Some(901_000.0 / 87.0));
    }
    // Recovery counts checkpoints after the release: with weigh done at turn 4, wave 1 recovers at its second.
    let mut log = checkpoint_log("git");
    let mut fixed = correct_programs(t, t.after_wave(1));
    fixed.insert("total", candidates(t, "total").wrong[0]);
    let text: Vec<(String, String)> =
        crate::candidates::modules(t, &fixed).into_iter().map(|(m, s)| (m.to_string(), s)).collect();
    log.events.pop();
    log.ev(6, None, "checkpoint", json!({"turn": 4, "modules": text}));
    let c = measure(&log.records(), &hidden).expect("measures").checkpoints.expect("checkpoints");
    assert_eq!(c.recovery[0].checkpoints, Some(2));
    // Phase 1 logs no checkpoints, so its measures have none and read as recorded.
    assert!(Log::new("git").measure().checkpoints.is_none());
}
