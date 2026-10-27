//! Running agents in rounds, each on its own replica, with bundle exchange
//! between rounds, and recording when each contribution reaches each replica.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::agent::{Agent, MEDIA_TYPE, Roster, Turn};
use crate::bracken::{Bracken, Entry, Replica};
use crate::rng::Rng;
use crate::task::{LANGUAGE, SPECS};

/// Who exchanges bundles with whom after each round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Topology {
    /// Every replica imports every other replica's bundle.
    Full,
    /// Each replica imports the bundle of the replica before it.
    Ring,
    /// Each replica imports one other replica's bundle, chosen at random.
    Gossip,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub agents: usize,
    pub rounds: u32,
    pub seed: u64,
    pub topology: Topology,
    /// Inputs per check.
    pub sample: usize,
}

/// Where and when a contribution was made.
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    pub agent: usize,
    pub round: u32,
    /// Position of the turn in the whole run, for "already existed" questions.
    pub turn: usize,
}

/// What the harness observed. The report is computed from this.
pub struct Run {
    pub names: Vec<String>,
    /// Contributions made by agents (not the task setter).
    pub origin: HashMap<String, Origin>,
    /// For each agent contribution, the ids its author's replica held when the turn began.
    pub held_before: HashMap<String, Rc<HashSet<String>>>,
    /// The sync phase after which an agent's replica first held a contribution.
    /// Phase `rounds + 1` is the final all-to-all sync.
    pub arrival: HashMap<(String, usize), u32>,
    /// Each replica's contribution count after the final sync.
    pub replica_sizes: Vec<usize>,
    /// Whether every replica ended with the same contributions.
    pub converged: bool,
    /// The first replica's view after the final sync.
    pub log: Vec<Entry>,
    pub conflicts: Value,
    /// `bracken accepted --min-passes <n> --json`, for each policy.
    pub accepted: Vec<(usize, Value)>,
}

/// Policies the report evaluates the final Ground under.
pub const POLICIES: [usize; 3] = [1, 2, 3];

fn ids(log: &[Entry]) -> HashSet<String> {
    log.iter().map(|e| e.id.clone()).collect()
}

pub fn run(
    bracken: &Bracken,
    root: &Path,
    cfg: &Config,
    mut make_agent: impl FnMut(usize, &str) -> Box<dyn Agent>,
) -> Result<Run> {
    // Bracken runs inside each agent's work directory, so bundle paths must be absolute.
    let root = &std::path::absolute(root).with_context(|| format!("resolving {}", root.display()))?;
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).with_context(|| format!("creating {}", bundles.display()))?;

    // The task setter creates the Ground and states the task; it never acts again.
    let setter = Replica::new(bracken, root.join("setter"), "setter")?;
    setter.init("demo")?;
    setter.create_identity()?;
    let task = setter.claim(
        &format!(
            "Task: implement each spec that refines this claim as a program in this language. {LANGUAGE} \
             An implementation is an artifact with media type {MEDIA_TYPE} that 'implements' its spec. \
             Check others' implementations with 'bracken-demo check' and record the result as evidence."
        ),
        &["task"],
    )?;
    for s in SPECS {
        let spec = setter.claim(&format!("Implement {}.", s.description), &["spec", s.name])?;
        setter.relate(&spec, "refines", &task, "")?;
    }
    let seed_bundle = bundles.join("seed.mrb");
    setter.export(&seed_bundle)?;

    let names: Vec<String> = (0..cfg.agents).map(|i| format!("agent-{i}")).collect();
    let mut replicas = Vec::new();
    let mut roster = Roster::default();
    for name in &names {
        let r = Replica::new(bracken, root.join(name), name)?;
        r.join(&seed_bundle)?;
        roster.add(r.create_identity()?, name.clone());
        replicas.push(r);
    }
    let mut agents: Vec<Box<dyn Agent>> = names.iter().enumerate().map(|(i, n)| make_agent(i, n)).collect();

    let mut run = Run {
        names: names.clone(),
        origin: HashMap::new(),
        held_before: HashMap::new(),
        arrival: HashMap::new(),
        replica_sizes: Vec::new(),
        converged: false,
        log: Vec::new(),
        conflicts: Value::Null,
        accepted: Vec::new(),
    };
    let mut held: Vec<HashSet<String>> = replicas.iter().map(|r| r.log().map(|l| ids(&l))).collect::<Result<_>>()?;
    let mut rng = Rng::derive(cfg.seed, 0);
    let mut turn = 0;

    for round in 1..=cfg.rounds {
        for (i, agent) in agents.iter_mut().enumerate() {
            let before = Rc::new(held[i].clone());
            agent
                .act(&Turn { round, replica: &replicas[i], roster: &roster })
                .with_context(|| format!("{} in round {round}", names[i]))?;
            let now = ids(&replicas[i].log()?);
            for id in now.difference(&held[i]) {
                run.origin.insert(id.clone(), Origin { agent: i, round, turn });
                run.held_before.insert(id.clone(), before.clone());
            }
            held[i] = now;
            turn += 1;
        }
        let n = cfg.agents;
        let sources: Vec<Vec<usize>> = (0..n)
            .map(|i| match cfg.topology {
                Topology::Full => (0..n).filter(|&j| j != i).collect(),
                Topology::Ring => vec![(i + n - 1) % n],
                Topology::Gossip => vec![(i + 1 + rng.below(n - 1)) % n],
            })
            .collect();
        sync(&replicas, &bundles, round, &sources, &mut held, &mut run.arrival)?;
    }

    let everyone: Vec<Vec<usize>> = (0..cfg.agents).map(|i| (0..cfg.agents).filter(|&j| j != i).collect()).collect();
    sync(&replicas, &bundles, cfg.rounds + 1, &everyone, &mut held, &mut run.arrival)?;

    run.replica_sizes = held.iter().map(HashSet::len).collect();
    run.converged = held.windows(2).all(|w| w[0] == w[1]);
    run.log = replicas[0].log()?;
    run.conflicts = replicas[0].conflicts()?;
    for k in POLICIES {
        run.accepted.push((k, replicas[0].accepted(k)?));
    }
    Ok(run)
}

/// One sync phase: every replica exports a bundle, then imports from its sources.
fn sync(
    replicas: &[Replica],
    bundles: &Path,
    phase: u32,
    sources: &[Vec<usize>],
    held: &mut [HashSet<String>],
    arrival: &mut HashMap<(String, usize), u32>,
) -> Result<()> {
    let paths: Vec<_> = replicas.iter().map(|r| bundles.join(format!("{phase}-{}.mrb", r.identity()))).collect();
    for (r, path) in replicas.iter().zip(&paths) {
        r.export(path)?;
    }
    for (i, r) in replicas.iter().enumerate() {
        for &j in &sources[i] {
            r.import(&paths[j])
                .with_context(|| format!("{} importing from {}", r.identity(), replicas[j].identity()))?;
        }
        let now = ids(&r.log()?);
        for id in now.difference(&held[i]) {
            arrival.insert((id.clone(), i), phase);
        }
        held[i] = now;
    }
    Ok(())
}
