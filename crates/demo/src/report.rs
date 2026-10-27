//! The measured report. Everything is computed from the replicas, through
//! bracken's JSON, so it doesn't depend on which kind of agent ran.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::bracken::Entry;
use crate::harness::{Config, Run, Topology};
use crate::task;

#[derive(Debug, Serialize)]
pub struct Report {
    pub config: ConfigReport,
    pub contributions: Contributions,
    pub work: Work,
    pub duplication: Duplication,
    pub availability: Availability,
    pub conflicts: Conflicts,
    pub ground_truth: GroundTruth,
    pub policies: Vec<PolicyReport>,
    pub replicas: Replicas,
    /// The only part that varies between runs with the same seed.
    pub timing: Timing,
}

#[derive(Debug, Serialize)]
pub struct ConfigReport {
    pub agents: usize,
    pub rounds: u32,
    pub seed: u64,
    pub topology: Topology,
    pub sample: usize,
    pub specs: usize,
}

#[derive(Debug, Serialize)]
pub struct Contributions {
    /// Made by agents; the task setter's are excluded.
    pub total: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub by_agent: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct Work {
    /// Artifacts that implement a spec.
    pub implementations: usize,
    /// Implementations that supersede another one.
    pub fixes: usize,
    pub checks: usize,
    pub passes: usize,
    pub fails: usize,
    /// Claims tagged `finding`.
    pub findings: usize,
}

#[derive(Debug, Serialize)]
pub struct Duplication {
    /// New implementations (not fixes) of a spec that another agent had already implemented.
    pub duplicates: usize,
    /// Of those, made when the earlier work hadn't reached the agent's replica yet.
    pub blind: usize,
    /// Of those, made when the agent's replica already held the earlier work.
    pub visible: usize,
    pub new_implementations: usize,
}

#[derive(Debug, Serialize)]
pub struct Availability {
    /// (contribution, other agent) pairs measured.
    pub pairs: usize,
    /// Sync rounds from a contribution's creation until another agent's replica holds it.
    pub mean_rounds: f64,
    pub median_rounds: u32,
    pub max_rounds: u32,
    /// Sync rounds to the number of pairs that took that many.
    pub histogram: BTreeMap<u32, usize>,
    /// Pairs that only arrived in the final all-to-all sync.
    pub final_sync_only: usize,
    /// Pairs that never arrived.
    pub missing: usize,
}

#[derive(Debug, Serialize)]
pub struct Conflicts {
    /// Open at the end, among everything not retracted.
    pub divergences: usize,
    pub contradictions: usize,
    pub supersedes_relations: usize,
    pub contradicts_relations: usize,
}

#[derive(Debug, Serialize)]
pub struct GroundTruth {
    /// Implementations right on every input, judged by the harness.
    pub correct: usize,
    pub wrong: usize,
    /// Wrong implementations that got at least one failing check.
    pub wrong_caught: usize,
}

#[derive(Debug, Serialize)]
pub struct PolicyReport {
    pub min_passes: usize,
    /// Subjects (claims and artifacts) that meet the policy, superseded or not.
    pub accepted: usize,
    /// Of those, replaced by another accepted subject.
    pub superseded: usize,
    pub implementations: usize,
    pub correct: usize,
    pub wrong: usize,
    /// Specs with an accepted, not superseded, correct implementation.
    pub specs_solved: usize,
    /// Conflicts in the accepted layer.
    pub conflicts: usize,
}

#[derive(Debug, Serialize)]
pub struct Replicas {
    pub converged: bool,
    pub contributions: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct Timing {
    pub wall_ms: u128,
    pub bracken_calls: u64,
    pub bracken_ms: u128,
}

/// An implementation in the final Ground.
struct Impl<'a> {
    entry: &'a Entry,
    spec: &'a str,
    correct: bool,
    fix: bool,
}

pub fn build(cfg: &Config, run: &Run, wall: Duration, calls: u64, bracken: Duration) -> Report {
    let log = &run.log;
    let by_id: HashMap<&str, &Entry> = log.iter().map(|e| (e.id.as_str(), e)).collect();
    let mine: Vec<&Entry> = log.iter().filter(|e| run.origin.contains_key(&e.id)).collect();
    let relations = |kind: &'static str| {
        mine.iter().filter(move |e| e.kind == "relation" && !e.retracted && e.text("kind") == kind)
    };

    let mut by_kind = BTreeMap::new();
    let mut by_agent: BTreeMap<String, usize> = run.names.iter().map(|n| (n.clone(), 0)).collect();
    for e in &mine {
        *by_kind.entry(e.kind.clone()).or_default() += 1;
        *by_agent.entry(run.names[run.origin[&e.id].agent].clone()).or_default() += 1;
    }

    // Implementations: artifacts their own author relates to a spec claim.
    let spec_of_claim: HashMap<&str, &str> = log
        .iter()
        .filter_map(|e| match e.tags()[..] {
            ["spec", s] if e.kind == "claim" => Some((e.id.as_str(), s)),
            _ => None,
        })
        .collect();
    let fixes: BTreeSet<&str> = relations("supersedes")
        .filter(|r| by_id.get(r.text("from")).is_some_and(|f| f.author == r.author))
        .map(|r| r.text("from"))
        .collect();
    let mut impls: BTreeMap<&str, Impl> = BTreeMap::new();
    for r in relations("implements") {
        let (Some(a), Some(spec)) = (by_id.get(r.text("from")), spec_of_claim.get(r.text("to"))) else { continue };
        let Some(s) = task::spec(spec).filter(|_| a.kind == "artifact" && a.author == r.author) else { continue };
        impls.entry(a.id.as_str()).or_insert(Impl {
            entry: a,
            spec,
            correct: s.is_correct(a.text("text")),
            fix: fixes.contains(a.id.as_str()),
        });
    }

    let evidence: Vec<&&Entry> = mine.iter().filter(|e| e.kind == "evidence").collect();
    let failed: BTreeSet<&str> = evidence
        .iter()
        .filter(|e| e.text("verdict") == "fail" && by_id.get(e.text("target")).is_some_and(|t| t.author != e.author))
        .map(|e| e.text("target"))
        .collect();

    let mut duplication = Duplication { duplicates: 0, blind: 0, visible: 0, new_implementations: 0 };
    for i in impls.values().filter(|i| !i.fix) {
        duplication.new_implementations += 1;
        let o = run.origin[&i.entry.id];
        let earlier: Vec<&str> = impls
            .values()
            .filter(|j| j.spec == i.spec && j.entry.author != i.entry.author && run.origin[&j.entry.id].turn < o.turn)
            .map(|j| j.entry.id.as_str())
            .collect();
        if earlier.is_empty() {
            continue;
        }
        duplication.duplicates += 1;
        let held = &run.held_before[&i.entry.id];
        if earlier.iter().any(|id| held.contains(*id)) {
            duplication.visible += 1;
        } else {
            duplication.blind += 1;
        }
    }

    let final_phase = cfg.rounds + 1;
    let mut latencies = Vec::new();
    let mut missing = 0;
    let mut final_sync_only = 0;
    for e in &mine {
        let o = run.origin[&e.id];
        for b in (0..run.names.len()).filter(|&b| b != o.agent) {
            match run.arrival.get(&(e.id.clone(), b)) {
                Some(&phase) => {
                    latencies.push(phase - o.round + 1);
                    final_sync_only += usize::from(phase == final_phase);
                }
                None => missing += 1,
            }
        }
    }
    latencies.sort_unstable();
    let mut histogram = BTreeMap::new();
    for &l in &latencies {
        *histogram.entry(l).or_default() += 1;
    }
    let availability = Availability {
        pairs: latencies.len() + missing,
        mean_rounds: if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().map(|&l| f64::from(l)).sum::<f64>() / latencies.len() as f64
        },
        median_rounds: latencies.get(latencies.len() / 2).copied().unwrap_or(0),
        max_rounds: latencies.last().copied().unwrap_or(0),
        histogram,
        final_sync_only,
        missing,
    };

    let count_type = |v: &Value, t: &str| v.as_array().map_or(0, |a| a.iter().filter(|c| c["type"] == t).count());
    let conflicts = Conflicts {
        divergences: count_type(&run.conflicts, "divergence"),
        contradictions: count_type(&run.conflicts, "contradiction"),
        supersedes_relations: relations("supersedes").count(),
        contradicts_relations: relations("contradicts").count(),
    };

    let policies = run
        .accepted
        .iter()
        .map(|(k, v)| {
            let accepted = v["accepted"].as_array().map(Vec::as_slice).unwrap_or_default();
            let status = |a: &Value| a["assessment"]["status"].as_str().unwrap_or_default().to_owned();
            let accepted_impls: Vec<(&Impl, String)> = accepted
                .iter()
                .filter_map(|a| impls.get(a["id"].as_str().unwrap_or_default()).map(|i| (i, status(a))))
                .collect();
            let solved: BTreeSet<&str> =
                accepted_impls.iter().filter(|(i, s)| i.correct && s == "accepted").map(|(i, _)| i.spec).collect();
            PolicyReport {
                min_passes: *k,
                accepted: accepted.len(),
                superseded: accepted.iter().filter(|a| status(a) == "superseded").count(),
                implementations: accepted_impls.len(),
                correct: accepted_impls.iter().filter(|(i, _)| i.correct).count(),
                wrong: accepted_impls.iter().filter(|(i, _)| !i.correct).count(),
                specs_solved: solved.len(),
                conflicts: v["conflicts"].as_array().map_or(0, Vec::len),
            }
        })
        .collect();

    Report {
        config: ConfigReport {
            agents: cfg.agents,
            rounds: cfg.rounds,
            seed: cfg.seed,
            topology: cfg.topology,
            sample: cfg.sample,
            specs: task::SPECS.len(),
        },
        contributions: Contributions { total: mine.len(), by_kind, by_agent },
        work: Work {
            implementations: impls.len(),
            fixes: impls.values().filter(|i| i.fix).count(),
            checks: evidence.len(),
            passes: evidence.iter().filter(|e| e.text("verdict") == "pass").count(),
            fails: evidence.iter().filter(|e| e.text("verdict") == "fail").count(),
            findings: mine.iter().filter(|e| e.kind == "claim" && e.tags().first() == Some(&"finding")).count(),
        },
        duplication,
        availability,
        conflicts,
        ground_truth: GroundTruth {
            correct: impls.values().filter(|i| i.correct).count(),
            wrong: impls.values().filter(|i| !i.correct).count(),
            wrong_caught: impls.values().filter(|i| !i.correct && failed.contains(i.entry.id.as_str())).count(),
        },
        policies,
        replicas: Replicas { converged: run.converged, contributions: run.replica_sizes.clone() },
        timing: Timing { wall_ms: wall.as_millis(), bracken_calls: calls, bracken_ms: bracken.as_millis() },
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let c = &self.config;
        let topology = serde_json::to_value(c.topology).expect("topologies serialize");
        writeln!(
            f,
            "Bracken demo: {} agents, {} rounds, seed {}, {} sync, checks on {} sampled inputs, {} specs\n",
            c.agents,
            c.rounds,
            c.seed,
            topology.as_str().unwrap_or("?"),
            c.sample,
            c.specs
        )?;

        let kinds: Vec<String> = self.contributions.by_kind.iter().map(|(k, n)| format!("{k} {n}")).collect();
        let agents: Vec<String> = self.contributions.by_agent.iter().map(|(a, n)| format!("{a} {n}")).collect();
        writeln!(f, "Contributions    {} by agents: {}", self.contributions.total, kinds.join(", "))?;
        writeln!(f, "                 {}", agents.join(", "))?;

        let w = &self.work;
        writeln!(
            f,
            "Work             {} implementations ({} fixes), {} checks ({} pass, {} fail), {} findings",
            w.implementations, w.fixes, w.checks, w.passes, w.fails, w.findings
        )?;
        let d = &self.duplication;
        writeln!(
            f,
            "Duplicated work  {} of {} new implementations repeat another agent's spec ({} blind, {} visible)",
            d.duplicates, d.new_implementations, d.blind, d.visible
        )?;
        let a = &self.availability;
        let hist: Vec<String> = a.histogram.iter().map(|(r, n)| format!("{r}: {n}")).collect();
        writeln!(
            f,
            "Availability     {:.2} sync rounds on average until another agent holds a contribution \
             (median {}, max {}; {} of {} only in the final sync{})",
            a.mean_rounds,
            a.median_rounds,
            a.max_rounds,
            a.final_sync_only,
            a.pairs,
            if a.missing > 0 { format!(", {} never", a.missing) } else { String::new() }
        )?;
        writeln!(f, "                 by rounds: {}", hist.join(", "))?;
        let k = &self.conflicts;
        writeln!(
            f,
            "Conflicts        {} divergences, {} contradictions open at the end \
             (from {} supersedes and {} contradicts relations)",
            k.divergences, k.contradictions, k.supersedes_relations, k.contradicts_relations
        )?;
        let g = &self.ground_truth;
        writeln!(
            f,
            "Ground truth     {} implementations correct, {} wrong; {} of the wrong ones got a failing check\n",
            g.correct, g.wrong, g.wrong_caught
        )?;

        writeln!(f, "Policy         accepted  superseded  impls  correct  wrong  specs solved  conflicts")?;
        for p in &self.policies {
            writeln!(
                f,
                "min passes {}  {:>8}  {:>10}  {:>5}  {:>7}  {:>5}  {:>8} of {}  {:>9}",
                p.min_passes,
                p.accepted,
                p.superseded,
                p.implementations,
                p.correct,
                p.wrong,
                p.specs_solved,
                c.specs,
                p.conflicts
            )?;
        }

        let r = &self.replicas;
        let sizes: BTreeSet<usize> = r.contributions.iter().copied().collect();
        writeln!(f)?;
        if r.converged {
            let n = sizes.first().copied().unwrap_or(0);
            writeln!(f, "Replicas         converged: all {} hold the same {n} contributions", r.contributions.len())?;
        } else {
            writeln!(f, "Replicas         NOT converged: sizes {:?}", r.contributions)?;
        }
        let t = &self.timing;
        write!(
            f,
            "Timing           {:.1} s wall, {} bracken calls ({:.1} s)",
            t.wall_ms as f64 / 1000.0,
            t.bracken_calls,
            t.bracken_ms as f64 / 1000.0
        )
    }
}
