//! A synthetic Ground for the benchmark: a chosen number of contributions and about a chosen number of
//! bytes of objects, shaped like this repository's Ground (research report B, section 1.3): about 30%
//! claims, 44% evidence, 23% relations, 2% retractions and 1% artifacts, which carry the bytes beyond what
//! the small kinds take. Every contribution is signed and valid; parents are the current heads (up to four,
//! so a few concurrent branches stay open), plus whatever the body references. Objects are written
//! straight to `objects/` without the store's fsync, which would cost minutes at 50,000 objects and
//! measures nothing the benchmark asks about; nothing else is written there, so the replica is exactly
//! what `bracken` reads.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use manyroot::{Body, Contribution, ContributionId, Identity, RelationKind, Store, Verdict};

/// What to build.
#[derive(Clone, Debug)]
pub struct Spec {
    pub contributions: usize,
    /// Total bytes of objects to aim for. The artifacts take what the small kinds leave; if they leave
    /// nothing, each artifact is small and the total is what the small kinds come to.
    pub bytes: u64,
    pub agents: usize,
    pub seed: u64,
}

/// What was built.
#[derive(Clone, Debug)]
pub struct Built {
    pub contributions: usize,
    pub object_bytes: u64,
    /// The identity names, each with its key in the replica's `identities/`.
    pub agents: Vec<String>,
}

/// splitmix64: deterministic, no dependency. Ids still differ between runs, since keys are random.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0), near enough for a benchmark.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const WORDS: &[&str] = &[
    "ground",
    "claim",
    "evidence",
    "relation",
    "snapshot",
    "review",
    "merge",
    "gate",
    "builder",
    "manager",
    "test",
    "store",
    "graph",
    "policy",
    "accepted",
    "current",
    "head",
    "parent",
    "object",
    "payload",
    "verify",
    "signature",
    "finding",
    "task",
    "decision",
    "the",
    "a",
    "of",
    "and",
    "to",
    "in",
    "is",
    "for",
    "with",
    "on",
    "that",
    "this",
];

const TAGS: &[(&str, u64)] = &[
    ("note", 30),
    ("pr", 15),
    ("finding", 15),
    ("task", 10),
    ("report", 10),
    ("decision", 8),
    ("gotcha", 7),
    ("question", 5),
];

fn text(rng: &mut Rng, min: usize, max: usize) -> String {
    let len = min + rng.below(max - min + 1);
    let mut out = String::with_capacity(len + 12);
    while out.len() < len {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(WORDS[rng.below(WORDS.len())]);
    }
    out
}

fn weighted<T: Copy>(rng: &mut Rng, table: &[(T, u64)]) -> T {
    let total: u64 = table.iter().map(|(_, w)| w).sum();
    let mut at = rng.next() % total;
    for (item, w) in table {
        if at < *w {
            return *item;
        }
        at -= w;
    }
    table[0].0
}

/// An index into `0..len`, mostly among the most recent 500, sometimes anywhere.
fn recent(rng: &mut Rng, len: usize) -> usize {
    if rng.chance(80) { len - 1 - rng.below(len.min(500)) } else { rng.below(len) }
}

#[derive(Clone, Copy)]
enum Kind {
    Claim,
    Evidence,
    Relation,
    Retract,
    Artifact,
}

/// Builds a new replica in `dir`, which must not hold one yet.
pub fn build(dir: &Path, spec: &Spec) -> Result<Built> {
    ensure!(spec.contributions > 0 && spec.agents > 0, "at least one contribution and one agent");
    let store = Store::init(dir, "bench").with_context(|| format!("creating a Ground in {}", dir.display()))?;
    let ground = store.info().ground;
    let mut agents: Vec<Identity> = Vec::new();
    for i in 0..spec.agents {
        agents.push(store.create_identity(&format!("agent-{i}"))?);
    }
    let mut rng = Rng::new(spec.seed);
    let n = spec.contributions;
    let artifacts_total = (n / 100).max(1);
    let mut artifacts_left = artifacts_total;
    let mut small_bytes = 0u64;
    let mut small_count = 0u64;
    let mut written = 0u64;

    let mut heads: Vec<ContributionId> = Vec::new();
    let mut subjects: Vec<ContributionId> = Vec::new();
    let mut own: Vec<Vec<ContributionId>> = vec![Vec::new(); agents.len()];
    let mut retracted: HashSet<ContributionId> = HashSet::new();
    let start_ms = 1_767_225_600_000u64; // 2026-01-01T00:00:00Z

    for i in 0..n {
        let left = n - i;
        let kind = if subjects.is_empty() {
            Kind::Claim
        } else if artifacts_left > 0 && (left <= artifacts_left || rng.below(left) < artifacts_left) {
            Kind::Artifact
        } else {
            weighted(&mut rng, &[(Kind::Claim, 30), (Kind::Evidence, 44), (Kind::Relation, 23), (Kind::Retract, 2)])
        };
        let mut author = rng.below(agents.len());
        let body = match kind {
            Kind::Claim => {
                let tags = vec![weighted(&mut rng, TAGS).to_owned()];
                Body::Claim { statement: text(&mut rng, 120, 900), tags }
            }
            Kind::Artifact => {
                artifacts_left -= 1;
                let small_left = (n - i - 1 - artifacts_left) as u64;
                let average_small = small_bytes.checked_div(small_count).unwrap_or(800);
                let budget = spec.bytes.saturating_sub(written + small_left * average_small);
                let size = (budget / (artifacts_left as u64 + 1)).clamp(256, 15 * 1024 * 1024) as usize;
                let mut content = vec![0u8; size];
                for chunk in content.chunks_mut(8) {
                    let word = rng.next().to_le_bytes();
                    chunk.copy_from_slice(&word[..chunk.len()]);
                }
                Body::Artifact {
                    name: format!("artifact-{i}.bin"),
                    media_type: "application/octet-stream".into(),
                    content,
                }
            }
            Kind::Evidence => {
                let target = subjects[recent(&mut rng, subjects.len())];
                let verdict = if rng.chance(80) { Verdict::Pass } else { Verdict::Fail };
                Body::Evidence {
                    target,
                    verdict,
                    method: format!("cargo test --workspace @ {}", rng.next() % 100_000),
                    details: text(&mut rng, 40, 1200),
                }
            }
            Kind::Relation => {
                let from = subjects[recent(&mut rng, subjects.len())];
                let to = subjects[recent(&mut rng, subjects.len())];
                if from == to {
                    Body::Claim { statement: text(&mut rng, 120, 900), tags: vec!["note".into()] }
                } else {
                    let kind = weighted(
                        &mut rng,
                        &[
                            (RelationKind::Implements, 30),
                            (RelationKind::Contradicts, 23),
                            (RelationKind::Refines, 22),
                            (RelationKind::Supersedes, 16),
                            (RelationKind::DependsOn, 6),
                            (RelationKind::Supports, 3),
                        ],
                    );
                    Body::Relation { from, kind, to, note: String::new() }
                }
            }
            Kind::Retract => {
                let candidates = &own[author];
                match (0..8)
                    .map(|_| candidates.get(rng.below(candidates.len().max(1))).copied())
                    .find(|c| c.is_some_and(|c| !retracted.contains(&c)))
                {
                    Some(Some(target)) => {
                        retracted.insert(target);
                        Body::Retract { target, reason: text(&mut rng, 10, 80) }
                    }
                    _ => {
                        author = rng.below(agents.len());
                        Body::Claim { statement: text(&mut rng, 120, 900), tags: vec!["note".into()] }
                    }
                }
            }
        };
        let is_subject = body.is_subject();
        let is_artifact = matches!(body, Body::Artifact { .. });
        // Up to four heads stay open; otherwise build on one or two of them, as concurrent writers do.
        let parents: Vec<ContributionId> = if heads.len() >= 4 || rng.chance(50) {
            heads.clone()
        } else {
            let mut p = vec![heads.get(rng.below(heads.len().max(1))).copied()];
            if rng.chance(30) {
                p.push(heads.get(rng.below(heads.len().max(1))).copied());
            }
            p.into_iter().flatten().collect()
        };
        let c = Contribution::create(ground, &agents[author], parents, start_ms + i as u64 * 60_000, body);
        let id = c.id();
        heads.retain(|h| !c.parents().contains(h));
        heads.push(id);
        if is_subject {
            subjects.push(id);
            own[author].push(id);
        }
        let bytes = c.encode();
        if !is_artifact {
            small_bytes += bytes.len() as u64;
            small_count += 1;
        }
        written += bytes.len() as u64;
        let hex = id.to_hex();
        let shard = dir.join("objects").join(&hex[..2]);
        fs::create_dir_all(&shard).with_context(|| format!("creating {}", shard.display()))?;
        fs::write(shard.join(&hex[2..]), &bytes).with_context(|| format!("writing object {hex}"))?;
    }

    Ok(Built { contributions: n, object_bytes: written, agents: agents.iter().map(|a| a.name().to_owned()).collect() })
}
