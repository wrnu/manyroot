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
    let backend = MockBackend { task, seed: 5, wrong_rate: 0.25, ..MockBackend::default() };
    mock_run_with(dir, mode, agents, turns, backend)
}

/// A run of `backend` (its task, its moves), which records usage under the run's directory.
fn mock_run_with(dir: &Path, mode: &'static str, agents: usize, turns: u32, backend: MockBackend) -> PathBuf {
    let task = backend.task;
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
    run(&cfg, &MockBackend { usage: Some(root.join("usage.jsonl")), ..backend }).expect("the run");
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

/// Task `07cb4ada01ea`: the scripted moves' checkpoint measures, checked by hand per mode. Two agents, four
/// turns, played one agent-turn at a time; everyone settles and nothing breaks. Agent 0 implements abs, dec,
/// diff, lt (every fifth spec from the first) and agent 1 sign, sq, add3, le, one a turn. Wave 1 (sign among
/// its targets) goes out after turn 1 and wave 2 (abs) after turn 2.
/// - CRDT: each write lands in the shared document, and each agent rewrites its own function the turn after
///   a wave changes it, before the checkpoint: 2, 4, 6, 8 correct.
/// - Bracken: the integrated library holds only implementations with an independent pass, and each agent
///   checks the other's newest first. Turn 1: abs (agent 1 passed it); turn 2: abs, dec (the old sign
///   failed, its rewrite unchecked); turn 3: those, sign, sq, the rewritten abs and diff; turn 4: add3, lt.
/// - Git: a PR merges once the other agent approves it, and an agent reads the task on its own branch, so
///   it learns of a wave only once it merges main in. Turn 1: abs; turn 2: dec; turn 3: diff, while the old
///   abs fails under wave 2 (a breakage after it); turn 4: sign and sq, mended and merged, and lt, while abs's
///   rewrite can't merge.
#[test]
fn scripted_phase_two_moves_give_hand_checked_checkpoints_in_every_mode() {
    let dir = tempfile::tempdir().expect("a temp dir");
    struct Expect {
        rows: [(u32, u64, u64); 4],
        from: &'static [(&'static str, u32)],
        median: f64,
        recovery: [(u64, Option<u64>); 2],
        breakage: (u64, u64),
    }
    let expect = [
        (
            "crdt",
            Expect {
                rows: [(1, 0, 2), (2, 1, 4), (3, 2, 6), (4, 2, 8)],
                from: &[("abs", 1), ("add3", 3), ("dec", 2), ("diff", 3), ("le", 4), ("lt", 4), ("sign", 1), ("sq", 2)],
                median: 2.5,
                // Wave 1 changes sign, correct before it; wave 2, abs. Each is rewritten by the next checkpoint.
                recovery: [(1, Some(1)), (1, Some(1))],
                breakage: (0, 0),
            },
        ),
        (
            "bracken",
            Expect {
                rows: [(1, 0, 1), (2, 1, 2), (3, 2, 5), (4, 2, 7)],
                from: &[("abs", 1), ("add3", 4), ("dec", 2), ("diff", 3), ("lt", 4), ("sign", 3), ("sq", 3)],
                median: 3.0,
                // sign wasn't in the library before wave 1, so it has nothing to recover: n/a.
                recovery: [(0, None), (1, Some(1))],
                breakage: (0, 0),
            },
        ),
        (
            "git",
            Expect {
                rows: [(1, 0, 1), (2, 1, 2), (3, 2, 2), (4, 2, 5)],
                from: &[("dec", 2), ("diff", 3), ("lt", 4), ("sign", 4), ("sq", 4)],
                median: 4.0,
                recovery: [(0, None), (1, None)],
                breakage: (1, 0),
            },
        ),
    ];
    for (mode, e) in expect {
        let backend = MockBackend {
            task: Task::Phase2,
            moves: bracken_experiment::mock::Moves::PLAIN,
            lockstep: true,
            ..MockBackend::default()
        };
        let root = mock_run_with(dir.path(), mode, 2, 4, backend);
        let events = std::fs::read_to_string(root.join("events.jsonl")).expect("events");
        assert!(!events.contains("\"turn_error\""), "{mode}");
        let c = report(&root, None).expect("the report").measures.checkpoints.expect("checkpoints");
        let rows: Vec<(u32, u64, u64)> = c.turns.iter().map(|k| (k.turn, k.waves, k.correct)).collect();
        assert_eq!(rows, e.rows, "{mode}");
        assert!(c.turns.iter().all(|k| k.standalone == k.correct), "{mode}: base functions call nothing");
        let from: Vec<(&str, u32)> = c.correct_from.iter().map(|(f, t)| (f.as_str(), *t)).collect();
        assert_eq!(from, e.from, "{mode}");
        assert_eq!(c.median_turn_correct, Some(e.median), "{mode}");
        let recovery: Vec<(u64, Option<u64>)> = c.recovery.iter().map(|r| (r.functions, r.checkpoints)).collect();
        assert_eq!(recovery, e.recovery, "{mode}");
        assert_eq!((c.breakage.after_wave, c.breakage.other), e.breakage, "{mode}");
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
