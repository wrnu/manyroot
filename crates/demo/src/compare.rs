//! Running the same seeds under several behaviours, side by side.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::bracken::Bracken;
use crate::harness::{self, Behaviour, Config, Schedule, StaleRule, Topology};
use crate::report::{PolicyReport, Report};
use crate::safety::{COMPARE, refuse_git_checkout};

/// What to vary, and what stays fixed.
pub struct Plan {
    pub base: Config,
    pub seeds: Vec<u64>,
    pub topologies: Vec<Topology>,
    pub work_rounds: Vec<u32>,
    pub schedules: Vec<Schedule>,
    /// Staleness rules for the wip rows.
    pub stale_rules: Vec<StaleRule>,
    /// Overrides each rule's default stale window.
    pub stale_after: Option<u32>,
    /// Runs at once.
    pub jobs: usize,
}

/// Means over the seeds for one behaviour.
#[derive(Debug, Serialize)]
pub struct Row {
    pub topology: Topology,
    pub behaviour: Behaviour,
    pub runs: usize,
    pub concurrent_starts: f64,
    /// Standard error of that mean over the seeds, to tell an effect from seed noise.
    pub concurrent_starts_se: f64,
    pub concurrent_same_round: f64,
    pub duplicates: f64,
    pub duplicates_blind: f64,
    pub duplicates_visible: f64,
    pub availability_rounds: f64,
    /// Under min passes 2.
    pub accepted_implementations: f64,
    pub accepted_correct: f64,
    pub specs_solved: f64,
    pub contributions: f64,
    pub bracken_calls: f64,
    pub wip_claims: f64,
    pub wip_taken_over: f64,
    /// Takeovers of work that was alive: already delivered, or in progress.
    pub wrongful_takeovers: f64,
    pub wrongful_takeovers_se: f64,
    pub takeovers_already_delivered: f64,
    pub takeovers_in_progress_delivered: f64,
    pub takeovers_in_progress_not_delivered: f64,
    pub takeovers_already_dropped: f64,
    pub silent_drops: f64,
    pub silent_drops_taken_over: f64,
    /// Mean over the runs that had a taken-over silent drop.
    pub silent_drop_delay_rounds: f64,
    pub silent_drops_missed: f64,
    pub heartbeats: f64,
    /// Summed, not averaged: it should be 0.
    pub wip_ignored_fresh: usize,
    /// Every run converged.
    pub converged: bool,
}

/// Every combination, with and without wip, for every seed.
pub fn run(bin: &Path, plan: &Plan) -> Result<Vec<Row>> {
    let mut behaviours = Vec::new();
    for &topology in &plan.topologies {
        for &w in &plan.work_rounds {
            for &schedule in &plan.schedules {
                behaviours.push((topology, Behaviour::new(w, false, schedule)));
                for &rule in &plan.stale_rules {
                    let b = Behaviour::new(w, true, schedule).with_rule(rule);
                    behaviours
                        .push((topology, Behaviour { stale_after: plan.stale_after.unwrap_or(b.stale_after), ..b }));
                }
            }
        }
    }
    let jobs: Vec<(usize, u64)> = (0..behaviours.len()).flat_map(|b| plan.seeds.iter().map(move |&s| (b, s))).collect();
    let results: Mutex<Vec<Option<Result<Report>>>> = Mutex::new((0..jobs.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..plan.jobs.clamp(1, jobs.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(b, seed)) = jobs.get(i) else { break };
                    let (topology, behaviour) = behaviours[b];
                    let cfg = Config { seed, topology, behaviour, ..plan.base.clone() };
                    let report = one(bin, &cfg);
                    results.lock().expect("no worker panics while holding the lock")[i] = Some(report);
                }
            });
        }
    });
    let results = results.into_inner().expect("no worker panicked");

    let mut rows = Vec::new();
    for (b, &(topology, behaviour)) in behaviours.iter().enumerate() {
        let reports: Vec<&Report> = jobs
            .iter()
            .zip(&results)
            .filter(|((jb, _), _)| *jb == b)
            .map(|((_, seed), r)| match r {
                Some(Ok(r)) => Ok(r),
                Some(Err(e)) => Err(anyhow!("seed {seed}, {topology:?}, {behaviour:?}: {e:#}")),
                None => Err(anyhow!("seed {seed} didn't run")),
            })
            .collect::<Result<_>>()?;
        let mean = |f: &dyn Fn(&Report) -> f64| reports.iter().map(|r| f(r)).sum::<f64>() / reports.len() as f64;
        let se = |f: &dyn Fn(&Report) -> f64| {
            let (n, m) = (reports.len() as f64, mean(f));
            if n < 2.0 {
                return 0.0;
            }
            (reports.iter().map(|r| (f(r) - m).powi(2)).sum::<f64>() / (n - 1.0) / n).sqrt()
        };
        rows.push(Row {
            topology,
            behaviour,
            runs: reports.len(),
            concurrent_starts: mean(&|r| r.concurrent_starts.total as f64),
            concurrent_starts_se: se(&|r| r.concurrent_starts.total as f64),
            concurrent_same_round: mean(&|r| r.concurrent_starts.same_round as f64),
            duplicates: mean(&|r| r.duplication.duplicates as f64),
            duplicates_blind: mean(&|r| r.duplication.blind as f64),
            duplicates_visible: mean(&|r| r.duplication.visible as f64),
            availability_rounds: mean(&|r| r.availability.mean_rounds),
            accepted_implementations: mean(&|r| policy2(r).implementations as f64),
            accepted_correct: mean(&|r| policy2(r).correct as f64),
            specs_solved: mean(&|r| policy2(r).specs_solved as f64),
            contributions: mean(&|r| r.contributions.total as f64),
            bracken_calls: mean(&|r| r.timing.bracken_calls as f64),
            wip_claims: mean(&|r| r.wip.claims as f64),
            wip_taken_over: mean(&|r| r.wip.taken_over as f64),
            wrongful_takeovers: mean(&|r| r.wip.takeovers.wrongful as f64),
            wrongful_takeovers_se: se(&|r| r.wip.takeovers.wrongful as f64),
            takeovers_already_delivered: mean(&|r| r.wip.takeovers.already_delivered as f64),
            takeovers_in_progress_delivered: mean(&|r| r.wip.takeovers.in_progress_delivered as f64),
            takeovers_in_progress_not_delivered: mean(&|r| r.wip.takeovers.in_progress_not_delivered as f64),
            takeovers_already_dropped: mean(&|r| r.wip.takeovers.already_dropped as f64),
            silent_drops: mean(&|r| r.wip.silent_drops.total as f64),
            silent_drops_taken_over: mean(&|r| r.wip.silent_drops.taken_over as f64),
            silent_drop_delay_rounds: {
                let with: Vec<f64> = reports
                    .iter()
                    .filter(|r| r.wip.silent_drops.taken_over > 0)
                    .map(|r| r.wip.silent_drops.mean_delay_rounds)
                    .collect();
                if with.is_empty() { 0.0 } else { with.iter().sum::<f64>() / with.len() as f64 }
            },
            silent_drops_missed: mean(&|r| r.wip.silent_drops.missed as f64),
            heartbeats: mean(&|r| r.wip.heartbeats as f64),
            wip_ignored_fresh: reports.iter().map(|r| r.wip.ignored_fresh).sum(),
            converged: reports.iter().all(|r| r.replicas.converged),
        });
    }
    Ok(rows)
}

fn policy2(r: &Report) -> &PolicyReport {
    r.policies.iter().find(|p| p.min_passes == 2).expect("min passes 2 is reported")
}

/// One run in its own temporary directory.
fn one(bin: &Path, cfg: &Config) -> Result<Report> {
    let bracken = Bracken::new(bin)?;
    let temp = tempfile::tempdir().context("creating a temporary directory")?;
    let root = std::path::absolute(temp.path()).context("resolving the temporary directory")?;
    refuse_git_checkout(&root, false, &COMPARE)?;
    std::fs::write(root.join(".gitignore"), "*\n").context("writing .gitignore")?;
    harness::run_scripted(&bracken, &root, cfg)
}

/// The rows as a table, one line per behaviour: without wip, then each staleness rule.
pub fn table(rows: &[Row]) -> String {
    let name = |v: serde_json::Value| v.as_str().unwrap_or("?").to_owned();
    let mut out = String::from(
        "topology  work  schedule  wip          K | concurrent ±se (same)  duplicates (blind/vis)  avail | \
         accepted ok  solved | contribs  calls | claims  takeovers: wrongful ±se (dlv/prog-dlv/prog-not) dropped | \
         silent drops: taken (delay) missed | beats  ignored\n",
    );
    for r in rows {
        let b = &r.behaviour;
        let wip = if b.wip { name(serde_json::json!(b.stale_rule)) } else { "off".into() };
        let k = if b.wip { b.stale_after.to_string() } else { "-".into() };
        out += &format!(
            "{:<8}  {:>4}  {:<8}  {:<11} {:>2} | {:>6.1} ±{:.1} ({:>5.1})  {:>10.1} ({:.1}/{:.1})  {:>5.2} | \
             {:>8.1} {:>4.1}  {:>4.1} | {:>8.1}  {:>5.0} | {:>6.1}  {:>9.1} ±{:.1} ({:.1}/{:.1}/{:.1})  {:>4.1} | \
             {:>4.1} of {:>3.1} ({:.2}) {:>3.1} | {:>5.1}  {:>7}{}\n",
            name(serde_json::json!(r.topology)),
            b.work_rounds,
            name(serde_json::json!(b.schedule)),
            wip,
            k,
            r.concurrent_starts,
            r.concurrent_starts_se,
            r.concurrent_same_round,
            r.duplicates,
            r.duplicates_blind,
            r.duplicates_visible,
            r.availability_rounds,
            r.accepted_implementations,
            r.accepted_correct,
            r.specs_solved,
            r.contributions,
            r.bracken_calls,
            r.wip_claims,
            r.wrongful_takeovers,
            r.wrongful_takeovers_se,
            r.takeovers_already_delivered,
            r.takeovers_in_progress_delivered,
            r.takeovers_in_progress_not_delivered,
            r.takeovers_already_dropped,
            r.silent_drops_taken_over,
            r.silent_drops,
            r.silent_drop_delay_rounds,
            r.silent_drops_missed,
            r.heartbeats,
            r.wip_ignored_fresh,
            if r.converged { "" } else { "  NOT CONVERGED" },
        );
    }
    out
}
