//! Build step 5 end to end: a mock run of each mode (Bracken, Git, CRDT) writes its plan first and
//! records enough for the report, and the report refuses a plan that isn't
//! the one the run logged.

use std::path::{Path, PathBuf};

use serde_json::Value;

use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, git_binary, run};
use bracken_experiment::metrics::table;
use bracken_experiment::mock::MockBackend;
use bracken_experiment::plan::{Plan, PlanInputs, report};
use bracken_experiment::task::{Task, Version, change_tag};

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
/// so, and the report grades Phase 2's functions. Its two waves go out once each, to all agents at once,
/// at the same point in every mode: with 4 turns, after turns 1 and 2, so after 2 and 4 agent-turns.
#[test]
fn a_phase_two_mock_run_of_each_mode_is_graded_as_phase_two_and_releases_both_waves() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let t = Task::Phase2;
    for mode in ["bracken", "git", "crdt"] {
        let root = mock_run_of(dir.path(), mode, t, 2, 4);
        let plan: Plan = serde_json::from_slice(&std::fs::read(root.join("plan.json")).expect("plan")).expect("json");
        assert_eq!(plan.inputs.task, t);
        assert_eq!(plan.waves, [1, 2]);
        let hash = |t: String| blake3::hash(t.as_bytes()).to_hex().to_string();
        assert_eq!(plan.prompts["task-original"], hash(t.task_text(Version::ORIGINAL)));
        assert_eq!(plan.prompts["task-after-wave-1"], hash(t.task_text(t.after_wave(1))));
        assert_eq!(plan.prompts["task-changed"], hash(t.task_text(t.after_wave(2))));
        let events: Vec<Value> = std::fs::read_to_string(root.join("events.jsonl"))
            .expect("events")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        let releases: Vec<&Value> = events.iter().filter(|e| e["event"] == "changes_released").collect();
        assert_eq!(releases.len(), 2, "{mode}");
        for (k, r) in releases.iter().enumerate() {
            let wave = k + 1;
            assert_eq!(r["data"]["wave"], wave, "{mode}");
            assert_eq!(r["data"]["turns_done"], [2, 4][k], "{mode}");
            let tags: Vec<&str> = t.wave_changes(wave).map(|c| change_tag(c.change)).collect();
            assert_eq!(r["data"]["result"]["released"], serde_json::json!(tags), "{mode}");
            // The agent-turns it counts have ended (another may end while the driver releases).
            let before = events.iter().filter(|e| e["event"] == "turn_end" && e["seq"].as_u64() < r["seq"].as_u64());
            assert!(before.count() >= [2, 4][k], "{mode}: wave {wave}");
        }
        if mode == "bracken" {
            // Each sync records the agent's contract members.
            assert!(events.iter().filter(|e| e["event"] == "sync").all(|e| e["data"]["record"]["members"].is_array()));
        }
        // A checkpoint once both agents end each turn, and none before a turn's last agent ended it.
        let checkpoints: Vec<&Value> = events.iter().filter(|e| e["event"] == "checkpoint").collect();
        assert_eq!(checkpoints.iter().map(|e| e["data"]["turn"].as_u64()).collect::<Vec<_>>(), [1, 2, 3, 4].map(Some));
        for c in &checkpoints {
            let turn = &c["data"]["turn"];
            let ends = events.iter().filter(|e| e["event"] == "turn_end" && e["data"]["turn"] == *turn);
            assert!(ends.clone().count() == 2 && ends.clone().all(|e| e["seq"].as_u64() < c["seq"].as_u64()), "{mode}");
        }
        let m = report(&root, None).expect("the report").measures;
        assert_eq!(m.quality.total, 90, "{mode}");
        assert!(m.improvements.gained > 0, "{mode}: the mock delivered Phase 2's functions");
        let c = m.checkpoints.clone().expect("Phase 2 measures its checkpoints");
        assert_eq!(c.turns.iter().map(|k| k.turn).collect::<Vec<_>>(), [1, 2, 3, 4], "{mode}");
        // With no barrier between turns, a wave can go out before the last agent ends its turn.
        let waves: Vec<u64> = c.turns.iter().map(|k| k.waves).collect();
        assert!(waves.windows(2).all(|w| w[0] <= w[1]) && waves.last() == Some(&2), "{mode}: {waves:?}");
        assert_eq!(c.recovery.len(), 2);
        assert!(table(std::slice::from_ref(&m)).contains("Ranking: checkpoints"));
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
        assert!(m.checkpoints.is_none(), "{mode}: Phase 1 logs no checkpoints");
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
