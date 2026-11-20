//! Synthetic Grounds: real signed contributions, generated deterministically from a seed, in about the mix
//! of kinds, tags and relation kinds this repository's Ground has (note `f2cf04a0a0b7`, section 1), with
//! concurrent branches, retraction chains, supersessions by a subject's author and by others, triage and
//! merge-gate passes, so every rule of the prelude has cases to decide.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use manyroot::{
    AgentId, Body, Contribution, ContributionId, Graph, GroundId, GroundInfo, Identity, RelationKind, Verdict,
};

/// A small, fast, deterministic generator (xorshift64*); quality is beside the point.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn word(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.word() % n.max(1) as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.word() % 100 < percent
    }

    /// An index into the last `window` of `len` items, so most references are to recent work.
    pub fn recent(&mut self, len: usize, window: usize) -> usize {
        len - 1 - self.below(len.min(window))
    }
}

pub struct Generated {
    pub graph: Graph,
    pub names: Vec<(String, AgentId)>,
    pub identities: Vec<Identity>,
}

/// About `n` contributions by `agents` agents, the first named `manager`.
pub fn ground(seed: u64, n: usize, agents: usize) -> Generated {
    let ground = GroundId([seed as u8; 32]);
    let identities: Vec<Identity> = (0..agents.max(2))
        .map(|i| Identity::generate(&if i == 0 { "manager".to_owned() } else { format!("agent-{i}") }).expect("a key"))
        .collect();
    let names = identities.iter().map(|i| (i.name().to_owned(), i.agent_id())).collect();
    let mut graph = Graph::new(ground);
    grow(&mut graph, &identities, seed, n);
    Generated { graph, names, identities }
}

/// Adds about `n` contributions by `identities` (the first is the triager) on top of what `graph` holds,
/// building on its heads. Returns the ids added, in order.
pub fn grow(graph: &mut Graph, identities: &[Identity], seed: u64, n: usize) -> Vec<ContributionId> {
    let mut rng = Rng::new(seed);
    let ground = graph.ground();
    // Every contribution with its author's index, and the subjects with theirs, in order.
    let mut all: Vec<(ContributionId, usize)> = Vec::new();
    let mut subjects: Vec<(ContributionId, usize)> = Vec::new();
    let mut claims_by: Vec<Vec<ContributionId>> = vec![Vec::new(); identities.len()];
    let mut written_by: Vec<Vec<ContributionId>> = vec![Vec::new(); identities.len()];
    let mut prs: Vec<ContributionId> = Vec::new();
    let mut is_pr: HashSet<ContributionId> = HashSet::new();
    let mut tasks: Vec<ContributionId> = Vec::new();
    let mut findings: Vec<(ContributionId, usize)> = Vec::new();
    let mut ts = 1_790_000_000_000u64;
    let tags = ["note", "decision", "report", "gotcha", "question", "plan"];

    while all.len() < n {
        let who = if rng.chance(25) { 0 } else { 1 + rng.below(identities.len() - 1) };
        let roll = rng.below(1000);
        let body = if subjects.len() < 4 || roll < 280 {
            // A claim: a pr claim, a finding against one, a task, or something else.
            let (statement, tags): (String, Vec<String>) = match rng.below(100) {
                0..19 => (format!("PR #{} branch @ {:040x}: a change", prs.len() + 1, rng.word()), vec!["pr".into()]),
                19..42 => (format!("Finding {}: a problem", rng.word() % 100_000), vec!["finding".into()]),
                42..52 => {
                    let mut t = vec!["task".to_owned()];
                    if rng.chance(30) {
                        t.push("deferred".into());
                    }
                    (format!("Task {}", rng.word() % 100_000), t)
                }
                _ => (
                    format!("A {} about {}", tags[rng.below(tags.len())], rng.word() % 1000),
                    vec![tags[rng.below(tags.len())].to_owned()],
                ),
            };
            Body::Claim { statement, tags }
        } else if roll < 710 {
            // Evidence on a recent subject: the manager triaging a finding, a merge gate, or a test run.
            let (target, _) = subjects[rng.recent(subjects.len(), 200)];
            let verdict = if rng.chance(80) { Verdict::Pass } else { Verdict::Fail };
            let method = if who == 0 && is_pr.contains(&target) && rng.chance(40) {
                format!("merge gate @ {:040x}", rng.word())
            } else {
                "cargo test".to_owned()
            };
            Body::Evidence { target, verdict, method, details: String::new() }
        } else if roll < 713 {
            Body::Artifact {
                name: format!("file-{}.txt", rng.word() % 1000),
                media_type: "text/plain".into(),
                content: rng.word().to_le_bytes().to_vec(),
            }
        } else if roll < 985 {
            let (kind, from, to) = match rng.below(100) {
                // A finding contradicting a recent pr claim.
                0..23 if !findings.is_empty() && !prs.is_empty() => {
                    let (f, _) = findings[rng.recent(findings.len(), 50)];
                    (RelationKind::Contradicts, f, prs[rng.recent(prs.len(), 30)])
                }
                // A supersession: mostly of the author's own claim, sometimes of someone else's.
                23..40 if claims_by[who].len() >= 2 || rng.chance(20) => {
                    if claims_by[who].len() >= 2 && rng.chance(85) {
                        let mine = &claims_by[who];
                        let old = mine[rng.recent(mine.len() - 1, 20)];
                        (RelationKind::Supersedes, *mine.last().expect("two or more"), old)
                    } else {
                        let a = subjects[rng.recent(subjects.len(), 100)].0;
                        let b = subjects[rng.recent(subjects.len(), 100)].0;
                        (RelationKind::Supersedes, a, b)
                    }
                }
                // The manager deferring a finding with a task.
                40..46 if !tasks.is_empty() && !findings.is_empty() => (
                    RelationKind::DependsOn,
                    tasks[rng.recent(tasks.len(), 20)],
                    findings[rng.recent(findings.len(), 50)].0,
                ),
                k => {
                    let kind = match k % 4 {
                        0 | 1 => RelationKind::Implements,
                        2 => RelationKind::Refines,
                        _ => RelationKind::Supports,
                    };
                    let a = subjects[rng.recent(subjects.len(), 100)].0;
                    (kind, a, subjects[rng.recent(subjects.len(), 300)].0)
                }
            };
            if from == to {
                continue;
            }
            Body::Relation { from, kind, to, note: String::new() }
        } else {
            // A retraction of something of the author's: a subject, a link, or another retraction.
            let Some(target) = written_by[who].get(rng.recent(written_by[who].len().max(1), 40)).copied() else {
                continue;
            };
            Body::Retract { target, reason: "withdrawn".into() }
        };
        // Usually build on every head; sometimes on a recent contribution only, which makes a branch.
        let parents: Vec<ContributionId> = if all.len() > 10 && rng.chance(30) {
            vec![all[rng.recent(all.len(), 30)].0]
        } else {
            graph.heads().collect()
        };
        // Some evidence is the manager's (triage, merge gates), and every depends_on is: deferrals are theirs.
        // A retraction stays with `who`, whose contribution it withdraws.
        let me = match &body {
            Body::Relation { kind: RelationKind::DependsOn, .. } => 0,
            Body::Evidence { .. } if rng.chance(40) => 0,
            _ => who,
        };
        ts += 1 + rng.below(60_000) as u64;
        let c = Contribution::create(ground, &identities[me], parents, ts, body);
        let id = c.id();
        let is_subject = c.body().is_subject();
        let tags_of = match c.body() {
            Body::Claim { tags, .. } => tags.clone(),
            _ => vec![],
        };
        if graph.insert(c).is_err() {
            continue;
        }
        all.push((id, me));
        written_by[me].push(id);
        if is_subject {
            subjects.push((id, me));
            claims_by[me].push(id);
            if tags_of.iter().any(|t| t == "pr") {
                prs.push(id);
                is_pr.insert(id);
            }
            if tags_of.iter().any(|t| t == "finding") {
                findings.push((id, me));
            }
            if tags_of.iter().any(|t| t == "task") && me == 0 {
                tasks.push(id);
            }
        }
    }
    all.into_iter().map(|(id, _)| id).collect()
}

/// The file `write` puts beside `ground.json`, holding the Ground's id: what marks a replica as one this
/// tool generated.
const MARKER: &str = "query-lab-synthetic";

/// Refuses any replica `write` didn't create: one without the marker naming its own Ground, one holding
/// `identities/` (no generated replica has keys on disk), or a `.bracken`. Only those may be extended.
pub fn ensure_synthetic(dir: &Path) -> Result<()> {
    let info: GroundInfo = serde_json::from_str(
        &fs::read_to_string(dir.join("ground.json")).with_context(|| format!("{} isn't a replica", dir.display()))?,
    )?;
    let marker = fs::read_to_string(dir.join(MARKER)).unwrap_or_default();
    anyhow::ensure!(
        marker.trim() == info.ground.to_hex()
            && !dir.join("identities").exists()
            && dir.canonicalize()?.file_name() != Some(".bracken".as_ref()),
        "{} isn't a synthetic Ground this tool generated; only those can be extended",
        dir.display()
    );
    Ok(())
}

/// Writes a generated Ground as a replica directory: `ground.json`, `objects/` and the public name map
/// `agents/`. No private key is written: the identities stay in memory.
pub fn write(
    dir: &Path,
    graph: &Graph,
    names: &[(String, AgentId)],
    ids: impl IntoIterator<Item = ContributionId>,
) -> Result<()> {
    let info = GroundInfo { ground: graph.ground(), name: "synthetic".into() };
    let manifest = dir.join("ground.json");
    if !manifest.exists() {
        fs::create_dir_all(dir)?;
        fs::write(&manifest, serde_json::to_string_pretty(&info)? + "\n")?;
        fs::write(dir.join(MARKER), graph.ground().to_hex() + "\n")?;
    }
    fs::create_dir_all(dir.join("agents"))?;
    for (name, agent) in names {
        let entry = serde_json::json!({ "name": name, "agent": agent });
        fs::write(dir.join("agents").join(format!("{name}.json")), serde_json::to_string_pretty(&entry)? + "\n")?;
    }
    let mut shards = HashSet::new();
    for id in ids {
        let hex = id.to_hex();
        let shard = dir.join("objects").join(&hex[..2]);
        if shards.insert(hex[..2].to_owned()) {
            fs::create_dir_all(&shard)?;
        }
        let c = graph.get(&id).context("a written id is in the graph")?;
        fs::write(shard.join(&hex[2..]), c.encode())?;
    }
    Ok(())
}
