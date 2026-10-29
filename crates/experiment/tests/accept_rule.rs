//! Bracken mode's acceptance rule against runs recorded before it changed: Phase 1's rule,
//! applied to a Phase 1 mock run's hub log, gives exactly the library that run integrated.

use bracken_experiment::bracken_mode::{AcceptRule, Snapshot, modules_of};
use bracken_experiment::task::Task;
use serde_json::Value;

/// A Bracken mock run recorded with the harness as it was before the rule became selectable
/// (`mock-run --mode bracken --agents 5 --turns 6`): the hub's `log --json --current` at the end,
/// the setter's key, and the modules the run printed as integrated.
const PHASE1_RUNS: &[(&str, &str)] = &[
    ("seed 1", include_str!("fixtures/phase1-bracken-mock-seed1.json")),
    ("seed 2", include_str!("fixtures/phase1-bracken-mock-seed2.json")),
    ("seed 3", include_str!("fixtures/phase1-bracken-mock-seed3.json")),
];

fn library(run: &Value, rule: AcceptRule) -> Vec<(String, String)> {
    let setter = run["setter"].as_str().expect("the setter's key");
    let snapshot = Snapshot::new(&run["log"], setter, rule, Task::Phase1).expect("a snapshot");
    modules_of(Task::Phase1, &snapshot.integrated())
}

#[test]
fn phase1s_rule_reproduces_phase1s_mock_runs() {
    for (name, text) in PHASE1_RUNS {
        let run: Value = serde_json::from_str(text).expect("a fixture");
        let recorded: Vec<(String, String)> = serde_json::from_value(run["integrated"].clone()).expect("modules");
        assert_eq!(library(&run, AcceptRule::MostPasses), recorded, "{name}");
    }
}

#[test]
fn a_plan_written_before_the_rule_was_selectable_reads_as_phase1s() {
    use bracken_experiment::plan::PlanInputs;
    let mut old = serde_json::to_value(PlanInputs::default()).expect("plain JSON");
    assert_eq!(old["accept_rule"], "one-pass", "new runs use the one-pass rule");
    old.as_object_mut().expect("an object").remove("accept_rule");
    let read: PlanInputs = serde_json::from_value(old).expect("an older plan still reads");
    assert_eq!(read.accept_rule, AcceptRule::MostPasses);
}
