//! Where a cold read's time goes: each phase of `Store::open` timed on its own, through manyroot's public
//! API, then building the query tables.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use manyroot::{Contribution, Graph, GroundInfo, Store};

use crate::ground::Ground;

#[derive(Debug, Default)]
pub struct Phases {
    pub contributions: usize,
    pub bytes: usize,
    pub read: Duration,
    pub decode: Duration,
    pub verify: Duration,
    /// `Graph::import`, which checks every rule again, signatures included.
    pub import: Duration,
    pub store_open: Duration,
    pub tables: Duration,
}

impl Phases {
    /// Each phase's minimum of the two.
    pub fn min(&self, other: &Phases) -> Phases {
        Phases {
            contributions: self.contributions,
            bytes: self.bytes,
            read: self.read.min(other.read),
            decode: self.decode.min(other.decode),
            verify: self.verify.min(other.verify),
            import: self.import.min(other.import),
            store_open: self.store_open.min(other.store_open),
            tables: self.tables.min(other.tables),
        }
    }
}

pub fn phases(dir: &Path) -> Result<Phases> {
    let mut p = Phases::default();

    let t = Instant::now();
    let mut files = Vec::new();
    for shard in fs::read_dir(dir.join("objects")).context("reading objects/")? {
        let shard = shard?.path();
        if !shard.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&shard)? {
            let path = entry?.path();
            if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.')) {
                files.push(fs::read(&path)?);
            }
        }
    }
    p.read = t.elapsed();
    p.contributions = files.len();
    p.bytes = files.iter().map(Vec::len).sum();

    let t = Instant::now();
    let decoded = files.iter().map(|b| Contribution::decode(b)).collect::<Result<Vec<_>, _>>()?;
    p.decode = t.elapsed();

    let t = Instant::now();
    anyhow::ensure!(decoded.iter().all(Contribution::signature_is_valid), "a signature doesn't verify");
    p.verify = t.elapsed();

    let info: GroundInfo = serde_json::from_str(&fs::read_to_string(dir.join("ground.json"))?)?;
    let t = Instant::now();
    let mut graph = Graph::new(info.ground);
    let report = graph.import(decoded);
    p.import = t.elapsed();
    anyhow::ensure!(report.is_clean(), "the import wasn't clean");

    let t = Instant::now();
    let store = Store::open(dir)?;
    p.store_open = t.elapsed();

    let names = names(&store)?;
    let t = Instant::now();
    let ground = Ground::from_graph(store.graph(), &names);
    p.tables = t.elapsed();
    anyhow::ensure!(ground.len() == p.contributions, "the tables lost rows");
    Ok(p)
}

pub fn names(store: &Store) -> Result<Vec<(String, manyroot::AgentId)>> {
    Ok(store.agents()?.into_iter().map(|e| (e.name, e.agent)).collect())
}
