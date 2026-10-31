//! # Query lab
//!
//! A research prototype (task `b6e2e0ca24fa`, design note `f2cf04a0a0b7`): can a declarative query
//! language answer the questions agents ask of a Ground faster and more simply than the CLI's hand-written
//! commands? It reads a Ground through manyroot's public API only, and nothing in the workspace depends on
//! it.

pub mod bench;
pub mod datalog;
pub mod generate;
pub mod ground;
pub mod output;
pub mod pipeline;
pub mod profile;

use std::path::Path;

use anyhow::Result;
use manyroot::Store;

/// Opens a replica directory read-only, verifying it as the CLI does, and builds the query tables.
pub fn open(dir: &Path) -> Result<ground::Ground> {
    let store = Store::open(dir)?;
    let names = profile::names(&store)?;
    Ok(ground::Ground::from_graph(store.graph(), &names))
}
