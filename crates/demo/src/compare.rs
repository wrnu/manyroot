//! Running the same seeds under several behaviours, side by side.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::bracken::Bracken;
use crate::harness::{self, Behaviour, Config, Schedule, Topology};
use crate::report::{PolicyReport, Report};
use crate::safety::refuse_git_checkout;

/// What to vary, and what stays fixed.
pub struct Plan {
    pub base: Config,
    pub seeds: Vec<u64>,
    pub topologies: Vec<Topology>,
    pub work_rounds: Vec<u32>,
    pub schedules: Vec<Schedule>,
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
                for wip in [false, true] {
                    behaviours.push((topology, Behaviour::new(w, wip, schedule)));
                }
            }
        }
    }
    let jobs: Vec<(usize, u64)> = (0..behaviours.len()).flat_map(|b| plan.seeds.iter().map(move |&s| (b, s))).collect();
    let results: Mutex<Vec<Option<Result<Report>>>> = Mutex::new((0..jobs.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..plan.jobs.max(1) {
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
        rows.push(Row {
            topology,
            behaviour,
            runs: reports.len(),
            concurrent_starts: mean(&|r| r.concurrent_starts.total as f64),
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
    refuse_git_checkout(&root, false)?;
    std::fs::write(root.join(".gitignore"), "*\n").context("writing .gitignore")?;
    harness::run_scripted(&bracken, &root, cfg)
}

/// The rows as a table, two lines (without and with wip) per behaviour.
pub fn table(rows: &[Row]) -> String {
    let name = |v: serde_json::Value| v.as_str().unwrap_or("?").to_owned();
    let mut out = String::from(
        "topology  work  schedule  wip  | concurrent (same rnd)  duplicates (blind/vis)  avail | \
         accepted ok  solved | contribs  calls | wip claims  taken  ignored\n",
    );
    for r in rows {
        let b = &r.behaviour;
        out += &format!(
            "{:<8}  {:>4}  {:<8}  {:<3}  | {:>10.1} ({:>8.1})  {:>10.1} ({:.1}/{:.1})  {:>5.2} | \
             {:>8.1} {:>2.1}  {:>6.1} | {:>8.1}  {:>5.0} | {:>10.1}  {:>5.1}  {:>7}{}\n",
            name(serde_json::json!(r.topology)),
            b.work_rounds,
            name(serde_json::json!(b.schedule)),
            if b.wip { "on" } else { "off" },
            r.concurrent_starts,
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
            r.wip_taken_over,
            r.wip_ignored_fresh,
            if r.converged { "" } else { "  NOT CONVERGED" },
        );
    }
    out
}
