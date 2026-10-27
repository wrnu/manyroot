//! `bracken-experiment`: the Phase 1 experiment's processes.
//!
//! - `worker` holds one store and answers the driver (started by the driver).
//! - `shim` relays MCP between Claude Code and the driver (started by Claude Code).
//! - `mock-run` runs a whole mode with the mock backend, for a look at the run directory.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use bracken_experiment::driver::{RunConfig, run};
use bracken_experiment::mock::MockBackend;
use bracken_experiment::mode::StoreConfig;

#[derive(Parser)]
#[command(name = "bracken-experiment", about = "The Phase 1 experiment's processes")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Hold one store and answer the driver on standard input and output.
    Worker {
        #[arg(long)]
        mode: String,
        #[arg(long)]
        identity: String,
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        bracken: PathBuf,
    },
    /// Relay MCP between standard input and output and the driver's socket.
    Shim {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Run Bracken mode with the mock backend into a new directory.
    MockRun {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 3)]
        agents: usize,
        #[arg(long, default_value_t = 5)]
        turns: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// The bracken binary; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
    },
}

fn main() {
    let result = match Cli::parse().command {
        Cmd::Worker { mode, identity, store, bracken } => {
            bracken_experiment::worker::serve(&mode, StoreConfig { dir: store, bracken, identity })
        }
        Cmd::Shim { socket } => bracken_experiment::shim::run(&socket),
        Cmd::MockRun { dir, agents, turns, seed, bracken } => mock_run(dir, agents, turns, seed, bracken),
    };
    if let Err(e) = result {
        eprintln!("bracken-experiment: {e}");
        std::process::exit(1);
    }
}

fn mock_run(dir: PathBuf, agents: usize, turns: u32, seed: u64, bracken: Option<PathBuf>) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bracken = match bracken {
        Some(b) => b,
        None => me.with_file_name("bracken"),
    };
    let bracken = std::fs::canonicalize(&bracken).map_err(|e| format!("no bracken at {}: {e}", bracken.display()))?;
    bracken_demo::safety::refuse_git_checkout(&dir, false, &bracken_demo::safety::DEMO).map_err(|e| e.to_string())?;
    let cfg = RunConfig { root: dir, mode: "bracken", agents, turns, worker_bin: me, bracken };
    let outcome = run(&cfg, &MockBackend { seed, wrong_rate: 0.15, shim: None })?;
    for (module, text) in &outcome.modules {
        println!("# module {module}\n{text}");
    }
    println!("events: {}", outcome.events.display());
    Ok(())
}
