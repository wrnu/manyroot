//! A mode's token cap for a run (`run --mode-token-cap <mode>=<tokens>`, the manager's note `2018c9beceff`, decision `c90a34c1ca38`): without
//! it every run's caps are the pilot's, exactly; with it only the named mode's token cap changes; and the
//! plan records the caps in effect, so its hash changes with them.

use std::path::{Path, PathBuf};
use std::process::Command;

use bracken_experiment::claude::{Caps, ModeTokenCap, REPLICATE_TOKENS};
use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, git_binary};
use bracken_experiment::mode::mode_named;
use bracken_experiment::plan::{Plan, PlanInputs};

const MODES: [&str; 3] = ["bracken", "git", "crdt"];

/// The pilot's caps as the plans of the pilot and calibrations recorded them, before this option existed
/// (main 1f288796).
const PILOT: &str = "Caps { calls_per_turn: 25, cap_grace: 60s, cap_grace_calls: 3, context: 150000, \
                     turn_time: 600s, turn_output: 30000, turn_budget_usd: Some(1.0), agent_tokens: 3000000, \
                     agent_output: 180000, run_tokens: 25000000, run_output: 1666666, run_time: 3600s }";

fn cap(s: &str) -> ModeTokenCap {
    s.parse().expect("a mode token cap")
}

#[test]
fn without_the_option_every_modes_caps_are_the_pilots_exactly() {
    assert_eq!(Caps::pilot().plan_value(), PILOT);
    for mode in MODES {
        let caps = Caps::for_mode(mode, &[]).expect("caps");
        assert_eq!(caps.plan_value(), PILOT, "{mode}");
    }
}

#[test]
fn the_option_changes_only_the_named_modes_token_cap() {
    let given = [cap("bracken=30000000")];
    let bracken = Caps::for_mode("bracken", &given).expect("caps");
    assert_eq!(bracken.run_tokens, 30_000_000);
    assert_eq!(Caps { run_tokens: Caps::pilot().run_tokens, ..bracken }.plan_value(), PILOT, "nothing else moves");
    for mode in ["git", "crdt"] {
        assert_eq!(Caps::for_mode(mode, &given).expect("caps").plan_value(), PILOT, "{mode}");
    }
    // One entry per mode, each for its own runs.
    let both = [cap("bracken=30000000"), cap("git=20000000")];
    assert_eq!(Caps::for_mode("git", &both).expect("caps").run_tokens, 20_000_000);
    assert_eq!(Caps::for_mode("crdt", &both).expect("caps").plan_value(), PILOT);
    assert!(Caps::for_mode("bracken", &[cap("bracken=1"), cap("bracken=2")]).is_err(), "a mode named twice");
    for bad in ["bracken", "nope=5", "bracken=0", "bracken=-5", "bracken=+5", "bracken=3e7", "bracken=", "=5"] {
        assert!(bad.parse::<ModeTokenCap>().is_err(), "{bad}");
    }
}

/// No mode's cap may pass the whole replicate's tokens (decision `c90a34c1ca38`): a value above 75M is a
/// mistake, refused like 0, while 75M itself and a cap lowered below the share are taken.
#[test]
fn a_cap_above_the_replicates_total_is_refused_and_a_lower_one_taken() {
    assert_eq!(REPLICATE_TOKENS, 75_000_000);
    for bad in ["bracken=75000001", "bracken=300000000", "git=18446744073709551615", "crdt=99999999999999999999"] {
        let err = bad.parse::<ModeTokenCap>().expect_err(bad);
        assert!(err.contains("75000000"), "{bad}: {err}");
    }
    assert_eq!(cap("bracken=75000000").tokens, 75_000_000);
    // The last mode of a replicate may run below its share, at what is left of the total.
    let lowered = Caps::for_mode("crdt", &[cap("crdt=20100000")]).expect("caps");
    assert_eq!(lowered.run_tokens, 20_100_000);
}

fn plan_hash(mode: &'static str, caps: &Caps) -> String {
    let cfg = RunConfig {
        root: PathBuf::from("/nonexistent/run"),
        mode,
        agents: 10,
        turns: 8,
        worker_bin: PathBuf::from("/nonexistent/worker"),
        bracken: PathBuf::from("/nonexistent/bracken"),
        sandbox: true,
        git: git_binary().expect("git"),
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan: PlanInputs { caps: caps.plan_value(), ..PlanInputs::default() },
    };
    let plan = Plan::new(&cfg, mode_named(mode).expect("a mode"));
    blake3::hash(&plan.bytes()).to_hex().to_string()
}

#[test]
fn the_plan_records_the_caps_in_effect_so_its_hash_changes_with_them() {
    let given = [cap("bracken=30000000")];
    let pilot = Caps::pilot();
    let raised = Caps::for_mode("bracken", &given).expect("caps");
    assert_ne!(plan_hash("bracken", &pilot), plan_hash("bracken", &raised));
    assert_eq!(plan_hash("bracken", &pilot), plan_hash("bracken", &Caps::for_mode("bracken", &[]).expect("caps")));
    // Another mode's run with the same option plans exactly as without it.
    assert_eq!(plan_hash("git", &pilot), plan_hash("git", &Caps::for_mode("git", &given).expect("caps")));
}

/// The command line refuses a malformed value, or one above the replicate's total, before anything runs.
#[test]
fn run_refuses_a_malformed_or_too_large_mode_token_cap() {
    let bin = Path::new(env!("CARGO_BIN_EXE_bracken-experiment"));
    for (value, says) in [("nope=5", "not a mode"), ("bracken=75000001", "whole replicate")] {
        let dir = tempfile::tempdir().expect("a temp dir");
        let out = Command::new(bin)
            .args(["run", "--dir"])
            .arg(dir.path().join("run"))
            .args([
                "--mode",
                "bracken",
                "--claude",
                "/nonexistent",
                "--claude-version",
                "0",
                "--settings",
                "/nonexistent",
            ])
            .args(["--mode-token-cap", value])
            .output()
            .expect("the binary runs");
        assert_eq!(out.status.code(), Some(2), "{value}: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains(says), "{value}: {out:?}");
        assert!(!dir.path().join("run").exists());
    }
}
