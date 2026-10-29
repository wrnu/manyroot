//! Writes a throwaway Ground of synthetic contributions, to look at `bracken view` at any size.
//!
//!     cargo run --example sample-ground -- <new dir under the temp directory> <contributions> [agents]
//!
//! It refuses any directory that isn't new and under the system's temp directory, so it can't touch a
//! real Ground. Then `BRACKEN_DIR=<dir> bracken view` shows it. Deterministic apart from keys and ids:
//! the same size gives the same shape. Every name and statement is synthetic. Agents work in bursts,
//! mostly build on the same few partners, and check each other's work, so the Loom has real paths
//! to show at 50,000. As on a team's Ground, some claims are prs and findings: a finding contradicts
//! another agent's pr, and a pr's author replaces it with a new one (now and then twice over, which is
//! a divergence), so the Tension map has contradictions open, cleared and forked.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use manyroot::{Body, Contribution, ContributionId, Identity, RelationKind, Store, Verdict};

const NAMES: [&str; 12] = [
    "owner",
    "manager",
    "builder-sync",
    "builder-demo",
    "builder-cli",
    "reviewer",
    "security",
    "builder-driver",
    "validator",
    "builder-docs",
    "builder-proto",
    "builder-ui",
];
const SUBJECTS: [&str; 12] = [
    "sync",
    "the merge gate",
    "intents",
    "the store",
    "bundles",
    "the log",
    "evidence",
    "the server",
    "cursors",
    "retractions",
    "the CLI",
    "the demo",
];
const VERBS: [&str; 8] =
    ["must refuse", "should batch", "keeps", "never drops", "verifies", "orders", "caches", "streams"];
const OBJECTS: [&str; 8] = [
    "unknown ids",
    "stale heads",
    "large bundles",
    "concurrent writes",
    "every signature",
    "expired intents",
    "a closed pipe",
    "the causal parents",
];
/// An invented day: 2025-10-27T08:00Z.
const START_MS: u64 = 1_761_552_000_000;

/// A small deterministic generator (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A float in [0, 1).
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[(self.next() % list.len() as u64) as usize]
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, n, rest @ ..] = args.as_slice() else {
        bail!("usage: sample-ground <new dir under the temp directory> <contributions> [agents]");
    };
    let n: usize = n.parse().context("contributions is a whole number")?;
    let agents: usize = rest.first().map_or(Ok(12), |a| a.parse()).context("agents is a whole number")?;
    ensure!((1..=NAMES.len()).contains(&agents), "agents is 1 to {}", NAMES.len());
    let dir = throwaway(Path::new(dir))?;
    generate(&dir, n, agents)?;
    println!("{} contributions by {agents} agents in {}", n, dir.display());
    Ok(())
}

/// `dir`, made absolute, if it doesn't exist yet and its parent is under the temp directory.
fn throwaway(dir: &Path) -> Result<PathBuf> {
    ensure!(!dir.exists(), "{} exists: give a new directory", dir.display());
    let parent = dir.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = parent.canonicalize().with_context(|| format!("{} must exist", parent.display()))?;
    let temp = std::env::temp_dir().canonicalize().context("finding the temp directory")?;
    ensure!(
        parent.starts_with(&temp) || parent.starts_with("/tmp") || parent.starts_with("/private/tmp"),
        "{} isn't under the temp directory {}: this writes throwaway Grounds only",
        parent.display(),
        temp.display()
    );
    Ok(parent.join(dir.file_name().context("the directory needs a name")?))
}

fn generate(dir: &Path, n: usize, agent_count: usize) -> Result<()> {
    let mut store = Store::init(dir, &format!("synthetic, {n}"))?;
    let ground = store.info().ground;
    let ids: Vec<Identity> =
        NAMES[..agent_count].iter().map(|name| store.create_identity(name)).collect::<Result<_, _>>()?;
    let mut r = Rng(n as u64 * 7919 + agent_count as u64);
    let days = if n > 5000 { 40 } else { 3 };
    let span = days as f64 * 86_400_000.0;
    // Each agent's share of the work, and whose work it builds on: everyone reads the manager, and each
    // builder has a reviewer and a neighbour or two.
    let weight: Vec<f64> = (0..agent_count)
        .map(|i| {
            if i == 1 {
                2.2
            } else if i == 0 {
                0.25
            } else {
                0.6 + r.f() * 1.4
            }
        })
        .collect();
    let total: f64 = weight.iter().sum();
    let partners: Vec<Vec<usize>> = (0..agent_count)
        .map(|i| {
            let list: Vec<usize> = if i == 1 {
                vec![2, 3, 4, 7, 9, 10, 11]
            } else {
                vec![1, (i + 1) % agent_count, if i % 2 == 1 { 5 } else { 8 }, if i % 3 == 0 { 5 } else { 6 }]
            };
            list.into_iter().filter(|&x| x != i && x < agent_count).collect()
        })
        .collect();

    let mut all: Vec<ContributionId> = Vec::with_capacity(n);
    let mut last: Vec<Option<usize>> = vec![None; agent_count];
    // Claims and artifacts, by position and author, for evidence, relations and retractions to name.
    let mut subjects: Vec<(usize, usize)> = Vec::new();
    // Prs by position and author; each agent's findings waiting for their contradicts relation; prs
    // already replaced.
    let mut prs: Vec<(usize, usize)> = Vec::new();
    let mut findings: Vec<Vec<(usize, usize, usize)>> = vec![Vec::new(); agent_count];
    // Each agent's prs that a finding contradicted, to fix; each replaced pr's replacement; and
    // replacements left standing beside another, so their divergence stays open.
    let mut fixes: Vec<Vec<usize>> = vec![Vec::new(); agent_count];
    let mut replaced: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut frozen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut batch: Vec<Contribution> = Vec::new();
    let mut t = START_MS as f64;
    for i in 0..n {
        t += (span / n as f64) * if r.f() < 0.08 { 6.0 + r.f() * 10.0 } else { 0.2 + r.f() * 0.6 };
        let mut pick = r.f() * total;
        let mut a = 0;
        while pick > weight[a] && a < agent_count - 1 {
            pick -= weight[a];
            a += 1;
        }
        // Parents: the agent's own last contribution, and what it last synced from one or two others.
        let mut parents: Vec<usize> = last[a].into_iter().collect();
        let merges = if r.f() < 0.55 {
            1
        } else if r.f() < 0.2 {
            2
        } else {
            0
        };
        for _ in 0..merges {
            let o = if r.f() < 0.9 && !partners[a].is_empty() {
                *r.pick(&partners[a])
            } else {
                (r.next() % agent_count as u64) as usize
            };
            parents.extend(last[o]);
        }
        if parents.is_empty() && i > 0 {
            parents.push(i - 1);
        }
        let roll = r.f();
        let others: Vec<&(usize, usize)> = subjects.iter().rev().take(30).filter(|(_, by)| *by != a).collect();
        let own: Vec<&(usize, usize)> = subjects.iter().rev().take(200).filter(|(_, by)| *by == a).collect();
        let statement = format!("{} {} {}", r.pick(&SUBJECTS), r.pick(&VERBS), r.pick(&OBJECTS));
        let their_prs: Vec<(usize, usize)> = prs.iter().rev().take(40).filter(|(_, by)| *by != a).copied().collect();
        let own_prs: Vec<usize> = prs.iter().rev().take(12).filter(|(_, by)| *by == a).map(|(p, _)| *p).collect();
        let body = if let Some((finding, pr, by)) = findings[a].pop() {
            fixes[by].push(pr);
            Body::Relation { from: all[finding], kind: RelationKind::Contradicts, to: all[pr], note: String::new() }
        } else if own_prs.len() > 1
            && ((0.3..0.36).contains(&roll) || ((0.36..0.5).contains(&roll) && !fixes[a].is_empty()))
        {
            // Its author replaces an earlier pr with its latest: first one a finding contradicted, as a fix
            // would. Now and then one already replaced, whose first replacement then stands beside the
            // second: a divergence.
            let latest = own_prs[0];
            let free = |p: &usize| *p != latest && !replaced.contains_key(p) && !frozen.contains(p);
            let fix = std::iter::from_fn(|| fixes[a].pop()).find(|p| free(p));
            let fork = (r.f() < 0.08)
                .then(|| {
                    own_prs[1..]
                        .iter()
                        .copied()
                        .find(|p| replaced.get(p).is_some_and(|s| *s != latest && !replaced.contains_key(s)))
                })
                .flatten();
            match fork.or(fix).or_else(|| own_prs[1..].iter().copied().find(|p| free(p))) {
                Some(old) => {
                    if let Some(first) = replaced.insert(old, latest) {
                        frozen.extend([first, latest]);
                    }
                    Body::Relation {
                        from: all[latest],
                        kind: RelationKind::Supersedes,
                        to: all[old],
                        note: String::new(),
                    }
                }
                None => Body::Claim { statement, tags: vec![] },
            }
        } else if i < 3 || roll < 0.3 || subjects.is_empty() {
            let tag = r.f();
            let (statement, tags) = if tag < 0.12 {
                (statement, vec!["task".to_owned()])
            } else if tag < 0.2 {
                (statement, vec!["decision".to_owned()])
            } else if tag < 0.3 {
                (format!("PR: {statement}"), vec!["pr".to_owned()])
            } else if tag < 0.36 && !their_prs.is_empty() {
                let (pr, by) = their_prs[(r.next() % their_prs.len() as u64) as usize];
                findings[a].push((i, pr, by));
                (format!("finding: {statement}"), vec!["finding".to_owned()])
            } else {
                (statement, vec![])
            };
            if tags.first().is_some_and(|t| t == "pr") {
                prs.push((i, a));
            }
            Body::Claim { statement, tags }
        } else if roll < 0.42 {
            let name = format!("src/{}.rs", r.pick(&SUBJECTS).trim_start_matches("the ").replace(' ', "_"));
            Body::Artifact { name, media_type: "text/plain".to_owned(), content: statement.into_bytes() }
        } else if roll < 0.7 || others.is_empty() {
            let (from, to) = if subjects.len() > 1 {
                (subjects[subjects.len() - 1].0, subjects[subjects.len() - 2].0)
            } else {
                (subjects[0].0, i - 1)
            };
            let kind = *r.pick(&[RelationKind::Implements, RelationKind::Supports, RelationKind::DependsOn]);
            Body::Relation { from: all[from], kind, to: all[to], note: String::new() }
        } else if roll < 0.97 || own.is_empty() {
            let target = others[(r.next() % others.len() as u64) as usize].0;
            let verdict = if r.f() < 0.82 { Verdict::Pass } else { Verdict::Fail };
            Body::Evidence { target: all[target], verdict, method: "cargo test".to_owned(), details: String::new() }
        } else {
            let target = own[(r.next() % own.len() as u64) as usize].0;
            Body::Retract { target: all[target], reason: "superseded by later work".to_owned() }
        };
        if body.is_subject() {
            subjects.push((i, a));
        }
        let c = Contribution::create(ground, &ids[a], parents.iter().map(|p| all[*p]), t as u64, body);
        all.push(c.id());
        batch.push(c);
        last[a] = Some(i);
        if batch.len() == 2000 {
            write(&mut store, &mut batch)?;
        }
    }
    write(&mut store, &mut batch)
}

fn write(store: &mut Store, batch: &mut Vec<Contribution>) -> Result<()> {
    let report = store.import(batch.drain(..))?;
    ensure!(report.rejected.is_empty() && report.unresolved.is_empty(), "the Ground refused some: {report:?}");
    Ok(())
}
