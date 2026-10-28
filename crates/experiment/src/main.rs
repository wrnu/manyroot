//! `bracken-experiment`: the Phase 1 experiment's processes.
//!
//! - `worker` holds one store and answers the driver (started by the driver).
//! - `shim` relays MCP between Claude Code and the driver (started by Claude Code).
//! - `guard` leads an agent turn's process group and kills it when the driver goes (started by the driver).
//! - `mock-run` runs a whole mode with the mock backend, for a look at the run directory.
//! - `run` runs a whole mode with Claude Code playing every agent.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use bracken_experiment::claude::{Caps, ClaudeBackend, ClaudeConfig, Expect, Pin, Routing};
use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, core_tools, git_binary, run};
use bracken_experiment::mock::MockBackend;
use bracken_experiment::mode::{StoreConfig, mode_named};

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
        #[arg(long)]
        git: PathBuf,
        /// Record every command git starts in the store's git-trace.log (tests).
        #[arg(long)]
        trace: bool,
    },
    /// Relay MCP between standard input and output and the driver's socket.
    Shim {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Wait for standard input to close, then kill this process's group.
    Guard,
    /// Run Bracken mode with Claude Code into a new directory, every worker sandboxed.
    Run {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 10)]
        agents: usize,
        #[arg(long, default_value_t = 8)]
        turns: u32,
        /// The versioned Claude Code binary to pin (not a `latest` link).
        #[arg(long)]
        claude: PathBuf,
        /// The `claude_code_version` it must report.
        #[arg(long)]
        claude_version: String,
        /// The Claude Code settings file holding the routing entries; read for them alone.
        #[arg(long)]
        settings: PathBuf,
        #[arg(long)]
        prompt_file: PathBuf,
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        /// The bracken binary; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
    },
    /// Run one mode with the mock backend into a new directory.
    MockRun {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "bracken", value_parser = ["bracken", "git"])]
        mode: String,
        #[arg(long, default_value_t = 3)]
        agents: usize,
        #[arg(long, default_value_t = 5)]
        turns: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// The bracken binary; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
        /// Run the workers under their sandbox profiles.
        #[arg(long)]
        sandbox: bool,
    },
}

fn main() {
    let result = match Cli::parse().command {
        Cmd::Worker { mode, identity, store, bracken, git, trace } => {
            bracken_experiment::worker::serve(&mode, StoreConfig { dir: store, bracken, git, identity, trace })
        }
        Cmd::Shim { socket } => bracken_experiment::shim::run(&socket),
        Cmd::Guard => bracken_experiment::claude::guard(),
        Cmd::Run { dir, agents, turns, claude, claude_version, settings, prompt_file, model, bracken } => {
            claude_run(ClaudeRun { dir, agents, turns, claude, claude_version, settings, prompt_file, model, bracken })
        }
        Cmd::MockRun { dir, mode, agents, turns, seed, bracken, sandbox } => {
            mock_run(dir, &mode, agents, turns, seed, bracken, sandbox)
        }
    };
    if let Err(e) = result {
        eprintln!("bracken-experiment: {e}");
        std::process::exit(1);
    }
}

struct ClaudeRun {
    dir: PathBuf,
    agents: usize,
    turns: u32,
    claude: PathBuf,
    claude_version: String,
    settings: PathBuf,
    prompt_file: PathBuf,
    model: String,
    bracken: Option<PathBuf>,
}

fn claude_run(a: ClaudeRun) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bracken = std::fs::canonicalize(a.bracken.unwrap_or_else(|| me.with_file_name("bracken")))
        .map_err(|e| format!("no bracken: {e}"))?;
    bracken_demo::safety::refuse_git_checkout(&a.dir, false, &bracken_demo::safety::DEMO).map_err(|e| e.to_string())?;
    let mode = mode_named("bracken").ok_or("no Bracken mode")?;
    let tools = mode.tools().into_iter().chain(core_tools()).map(|t| t.name.to_string()).collect();
    let prompt = std::fs::read_to_string(&a.prompt_file).map_err(|e| format!("reading the prompt: {e}"))?;
    let backend = ClaudeBackend::new(ClaudeConfig {
        pin: Pin::of(&a.claude, &a.claude_version)?,
        expect: Expect::pinned_2_1_311(&a.model),
        caps: Caps::pilot(),
        routing: Routing::from_settings(&a.settings)?,
        dir: a.dir.join("claude"),
        shim: me.clone(),
        prompt,
        tools,
        upstream: None,
    })?;
    let root = a.dir.join("run");
    let cfg = RunConfig {
        root,
        mode: "bracken",
        agents: a.agents,
        turns: a.turns,
        worker_bin: me,
        bracken,
        git: git_binary()?,
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox: true,
    };
    let outcome = run(&cfg, &backend)?;
    println!("events: {}", outcome.events.display());
    Ok(())
}

fn mock_run(
    dir: PathBuf,
    mode: &str,
    agents: usize,
    turns: u32,
    seed: u64,
    bracken: Option<PathBuf>,
    sandbox: bool,
) -> Result<(), String> {
    let mode = bracken_experiment::mode::mode_named(mode).ok_or("no such mode")?.name();
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bracken = match bracken {
        Some(b) => b,
        None => me.with_file_name("bracken"),
    };
    let bracken = std::fs::canonicalize(&bracken).map_err(|e| format!("no bracken at {}: {e}", bracken.display()))?;
    bracken_demo::safety::refuse_git_checkout(&dir, false, &bracken_demo::safety::DEMO).map_err(|e| e.to_string())?;
    let cfg = RunConfig {
        root: dir,
        mode,
        agents,
        turns,
        worker_bin: me,
        bracken,
        git: git_binary()?,
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        sandbox,
    };
    let outcome = run(&cfg, &MockBackend { seed, wrong_rate: 0.15, shim: None })?;
    for (module, text) in &outcome.modules {
        println!("# module {module}\n{text}");
    }
    println!("events: {}", outcome.events.display());
    Ok(())
}
