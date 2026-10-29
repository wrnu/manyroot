//! Build step 5 end to end: a mock run of each mode (Bracken, Git, CRDT) writes its plan first and
//! records enough for the report, and the report refuses a plan that isn't
//! the one the run logged.

use std::path::{Path, PathBuf};

use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, git_binary, run};
use bracken_experiment::metrics::table;
use bracken_experiment::mock::MockBackend;
use bracken_experiment::plan::{Plan, PlanInputs, report};
use bracken_experiment::task::{Task, Version};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bracken-experiment"))
}

fn bracken() -> PathBuf {
    let b = bin().with_file_name("bracken");
    assert!(b.exists(), "build bracken first: cargo build -p bracken");
    b
}

fn mock_run(dir: &Path, mode: &'static str) -> PathBuf {
    mock_run_of(dir, mode, Task::Phase1, 3, 4)
}

fn mock_run_of(dir: &Path, mode: &'static str, task: Task, agents: usize, turns: u32) -> PathBuf {
    let root = dir.join(format!("{mode}-{}", task.name()));
    let cfg = RunConfig {
        root: root.clone(),
        mode,
        agents,
        turns,
        worker_bin: bin(),
        bracken: bracken(),
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan: PlanInputs { task, grader_seed: 7, ..PlanInputs::default() },
        sandbox: false,
    };
    let usage = Some(root.join("usage.jsonl"));
    run(&cfg, &MockBackend { task, seed: 5, wrong_rate: 0.25, shim: None, usage }).expect("the run");
    root
}

/// The task selector end to end: a Phase 2 run gives its agents Phase 2's task in every mode, its plan says
/// so, and the report grades Phase 2's functions.
#[test]
fn a_phase_two_mock_run_of_each_mode_is_graded_as_phase_two() {
    let dir = tempfile::tempdir().expect("a temp dir");
    for mode in ["bracken", "git", "crdt"] {
        let root = mock_run_of(dir.path(), mode, Task::Phase2, 2, 2);
        let plan: Plan = serde_json::from_slice(&std::fs::read(root.join("plan.json")).expect("plan")).expect("json");
        assert_eq!(plan.inputs.task, Task::Phase2);
        let hash = |t: String| blake3::hash(t.as_bytes()).to_hex().to_string();
        assert_eq!(plan.prompts["task-original"], hash(Task::Phase2.task_text(Version::ORIGINAL)));
        let m = report(&root, None).expect("the report").measures;
        assert_eq!(m.quality.total, 90, "{mode}");
        assert!(m.improvements.gained > 0, "{mode}: the mock delivered Phase 2's functions");
    }
}

#[test]
fn a_mock_run_of_each_mode_produces_the_report() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut measures = Vec::new();
    for mode in ["bracken", "git", "crdt"] {
        let root = mock_run(dir.path(), mode);
        let r = report(&root, None).expect("the report");
        let m = &r.measures;
        assert_eq!(m.mode, mode);
        assert!(m.snapshots >= 2, "{mode}: the setup state and at least one change");
        assert!(m.tokens > m.output_tokens && m.output_tokens > 0, "{mode}: the mock's usage rows");
        assert!(m.duplication.deliveries >= 3, "{mode}: each agent delivers about once a turn");
        assert!(m.improvements.gained > 0 && m.improvements.per_million_tokens.is_some(), "{mode}");
        assert!(m.overhead.integrate_tokens.is_some_and(|s| s > 0.0), "{mode}: every turn syncs");
        assert!(m.discovery.discoveries > 0, "{mode}");
        // CRDT mode has no review step (decision `bbb261d50871`).
        assert_eq!(m.incompatible.semantic_conflicts.is_some(), mode != "crdt", "{mode}");
        assert_eq!(m.quality.total, 30);
        assert_eq!(m.quality.correct + m.quality.wrong + m.quality.missing, 30);

        // The plan the report quotes is the one written before any turn.
        let plan: Plan = serde_json::from_slice(&std::fs::read(root.join("plan.json")).expect("plan")).expect("json");
        assert_eq!((plan.mode.as_str(), plan.agents, plan.inputs.grader_seed), (mode, 3, 7));
        assert!(plan.prompts.contains_key("turn-message"), "the turn message is a prompt too");
        assert_eq!(r.plan, blake3::hash(&std::fs::read(root.join("plan.json")).expect("plan")).to_hex().to_string());
        let first = std::fs::read_to_string(root.join("events.jsonl")).expect("events");
        assert!(first.lines().next().expect("a line").contains(&r.plan), "the first event quotes the plan");
        measures.push(r.measures);
    }
    let t = table(&measures);
    let header = t.lines().next().expect("a header");
    assert!(["| bracken", "| git", "| crdt"].iter().all(|m| header.contains(m)), "{t}");
    let semantic = t.lines().find(|l| l.starts_with("| Semantic-conflict events")).expect("the row");
    assert_eq!(semantic.split('|').nth(4).map(str::trim), Some("n/a"), "CRDT's cell: {t}");
    assert_eq!(t.lines().count(), 22, "a header, a rule and a row per line of the measures");
}

#[test]
fn the_report_refuses_a_plan_changed_after_the_run() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = mock_run(dir.path(), "bracken");
    let path = root.join("plan.json");
    let planned: Plan = serde_json::from_slice(&std::fs::read(&path).expect("plan")).expect("json");

    // A different seed for the hidden cases: not the plan the run logged.
    let reseeded = Plan { inputs: PlanInputs { grader_seed: 8, ..planned.inputs.clone() }, ..planned.clone() };
    std::fs::write(&path, reseeded.bytes()).expect("write");
    let err = report(&root, None).expect_err("refused");
    assert!(err.contains("isn't the plan the run logged"), "{err}");

    // Other metric code: refused before anything is measured.
    let other = Plan { metrics_code: "0".repeat(64), ..planned.clone() };
    std::fs::write(&path, other.bytes()).expect("write");
    let err = report(&root, None).expect_err("refused");
    assert!(err.contains("fixed metric code"), "{err}");

    std::fs::write(&path, planned.bytes()).expect("write");
    report(&root, None).expect("the plan as written");
}

/// Phase 1 is measured as before the task selector and open contracts: Phase 1 mock-run logs (3 agents,
/// 3 turns, seed 2, one per mode), made and measured by main's build at e7cf42c (its report's measures,
/// compacted), measure the same with this one, to the byte.
#[test]
fn phase_one_logs_measure_as_main_measured_them() {
    use bracken_experiment::grader::HiddenCases;
    use bracken_experiment::metrics::{Records, measure};
    let fixtures = [
        (
            "bracken",
            include_str!("fixtures/phase1-bracken-mock-log/events.jsonl"),
            include_str!("fixtures/phase1-bracken-mock-log/usage.jsonl"),
            include_str!("fixtures/phase1-bracken-mock-log/measures.json"),
        ),
        (
            "git",
            include_str!("fixtures/phase1-git-mock-log/events.jsonl"),
            include_str!("fixtures/phase1-git-mock-log/usage.jsonl"),
            include_str!("fixtures/phase1-git-mock-log/measures.json"),
        ),
        (
            "crdt",
            include_str!("fixtures/phase1-crdt-mock-log/events.jsonl"),
            include_str!("fixtures/phase1-crdt-mock-log/usage.jsonl"),
            include_str!("fixtures/phase1-crdt-mock-log/measures.json"),
        ),
    ];
    for (mode, events, usage, want) in fixtures {
        // Phase 1's records hold no working copies: those are for a task with open contracts.
        assert!(!events.contains("\"members\""), "{mode}");
        let records = Records::parse(events, usage).expect("records");
        let got = measure(&records, &HiddenCases::new(Task::Phase1, 1)).expect("measures");
        // As text: parsing main's floats back can be off by one unit in the last place.
        assert_eq!(serde_json::to_string(&got).expect("JSON"), want.trim(), "{mode}");
    }
}
