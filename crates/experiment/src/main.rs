//! `bracken-experiment`: the Phase 1 experiment's processes.
//!
//! - `worker` holds one store and answers the driver (started by the driver).
//! - `shim` relays MCP between Claude Code and the driver (started by Claude Code).
//! - `guard` leads an agent turn's process group and kills it when the driver goes (started by the driver).
//! - `mock-run` runs a whole mode with the mock backend, for a look at the run directory.
//! - `report` measures finished runs and prints the comparison table.
//! - `run` runs a whole mode with Claude Code playing every agent.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use bracken_experiment::bracken_mode::AcceptRule;
use bracken_experiment::claude::{
    Caps, ClaudeBackend, ClaudeConfig, Expect, Pin, RUN_TIME_CAP, RUN_TOKEN_CAP, Routing,
};
use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, core_tools, git_binary, run};
use bracken_experiment::mock::MockBackend;
use bracken_experiment::mode::{StoreConfig, mode_under};
use bracken_experiment::plan::PlanInputs;
use bracken_experiment::task::Task;

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
        #[arg(long, value_parser = TASKS)]
        task: String,
        /// Record every command git starts in the store's git-trace.log (tests).
        #[arg(long)]
        trace: bool,
        /// Bracken mode: which implementation counts for a function.
        #[arg(long, default_value = "one-pass", value_parser = AcceptRule::NAMES)]
        accept_rule: String,
    },
    /// Relay MCP between standard input and output and the driver's socket.
    Shim {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Wait for standard input to close, then kill this process's group.
    Guard,
    /// Run one mode with Claude Code into a new directory, every worker sandboxed.
    Run {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, value_parser = ["bracken", "git", "crdt"])]
        mode: String,
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
        /// Tests only: a system prompt file in place of the reviewed prompts built into the binary.
        #[arg(long)]
        prompt_file: Option<PathBuf>,
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        /// The task agents are given: Phase 1's or Phase 2's.
        #[arg(long, default_value = "phase1", value_parser = TASKS)]
        task: String,
        /// The bracken binary; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
        /// Bracken mode: which implementation counts for a function (phase1-most-passes reproduces Phase 1).
        #[arg(long, default_value = "one-pass", value_parser = AcceptRule::NAMES)]
        accept_rule: String,
    },
    /// Run one mode with the mock backend into a new directory.
    MockRun {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "bracken", value_parser = ["bracken", "git", "crdt"])]
        mode: String,
        #[arg(long, default_value_t = 3)]
        agents: usize,
        #[arg(long, default_value_t = 5)]
        turns: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// The task agents are given: Phase 1's or Phase 2's.
        #[arg(long, default_value = "phase1", value_parser = TASKS)]
        task: String,
        /// The bracken binary; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
        /// Run the workers under their sandbox profiles.
        #[arg(long)]
        sandbox: bool,
        /// Bracken mode: which implementation counts for a function (phase1-most-passes reproduces Phase 1).
        #[arg(long, default_value = "one-pass", value_parser = AcceptRule::NAMES)]
        accept_rule: String,
    },
    /// Measure finished runs, one column each.
    Report {
        #[arg(required = true)]
        dirs: Vec<PathBuf>,
        /// The usage rows, if not in the run directory (one run only).
        #[arg(long)]
        usage: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let result = match Cli::parse().command {
        Cmd::Worker { mode, identity, store, bracken, git, task, trace, accept_rule } => {
            let accept_rule = AcceptRule::named(&accept_rule).expect("clap checked the name");
            let task = task_named(&task);
            bracken_experiment::worker::serve(
                &mode,
                StoreConfig { dir: store, bracken, git, identity, trace, accept_rule, task },
            )
        }
        Cmd::Shim { socket } => bracken_experiment::shim::run(&socket),
        Cmd::Guard => bracken_experiment::claude::guard(),
        Cmd::Run {
            dir,
            mode,
            agents,
            turns,
            claude,
            claude_version,
            settings,
            prompt_file,
            model,
            task,
            bracken,
            accept_rule,
        } => claude_run(ClaudeRun {
            accept_rule: AcceptRule::named(&accept_rule).expect("clap checked the name"),
            task: task_named(&task),
            dir,
            mode,
            agents,
            turns,
            claude,
            claude_version,
            settings,
            prompt_file,
            model,
            bracken,
        }),
        Cmd::MockRun { dir, mode, agents, turns, seed, task, bracken, sandbox, accept_rule } => {
            let accept_rule = AcceptRule::named(&accept_rule).expect("clap checked the name");
            let plan = PlanInputs {
                task: task_named(&task),
                backend_seed: seed,
                harness_commit: harness_commit(),
                accept_rule,
                ..PlanInputs::default()
            };
            mock_run(dir, &mode, agents, turns, bracken, sandbox, plan)
        }
        Cmd::Report { dirs, usage, json } => report(&dirs, usage.as_deref(), json),
    };
    if let Err(e) = result {
        eprintln!("bracken-experiment: {e}");
        std::process::exit(1);
    }
}

/// The tasks `--task` takes.
const TASKS: [&str; 2] = ["phase1", "phase2"];

/// The task `name`, which the argument parser has checked is one of [`TASKS`].
fn task_named(name: &str) -> Task {
    Task::named(name).expect("the parser accepts only the tasks' names")
}

struct ClaudeRun {
    task: Task,
    dir: PathBuf,
    mode: String,
    agents: usize,
    turns: u32,
    claude: PathBuf,
    claude_version: String,
    settings: PathBuf,
    prompt_file: Option<PathBuf>,
    model: String,
    bracken: Option<PathBuf>,
    accept_rule: AcceptRule,
}

fn claude_run(a: ClaudeRun) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let bracken = std::fs::canonicalize(a.bracken.unwrap_or_else(|| me.with_file_name("bracken")))
        .map_err(|e| format!("no bracken: {e}"))?;
    bracken_demo::safety::refuse_git_checkout(&a.dir, false, &bracken_demo::safety::DEMO).map_err(|e| e.to_string())?;
    let mode = mode_under(&a.mode, a.accept_rule).ok_or("no such mode")?;
    let tools = mode.tools(a.task).into_iter().chain(core_tools()).map(|t| t.name.to_string()).collect();
    let prompt = match &a.prompt_file {
        Some(file) => std::fs::read_to_string(file).map_err(|e| format!("reading the prompt: {e}"))?,
        None => bracken_experiment::prompts::system_prompt(mode.name())?,
    };
    if a.prompt_file.is_some() {
        println!("the system prompt is from --prompt-file, not the reviewed prompts; the plan hashes it");
    }
    let (expect, caps) = (Expect::pinned_2_1_311(&a.model), Caps::pilot());
    let plan = PlanInputs {
        task: a.task,
        model: expect.model.clone(),
        effort: expect.effort.clone(),
        harness_commit: harness_commit(),
        caps: serde_json::Value::String(format!("{caps:?}")),
        system_prompt: blake3::hash(prompt.as_bytes()).to_hex().to_string(),
        expected_stops: vec![RUN_TOKEN_CAP.into(), RUN_TIME_CAP.into()],
        accept_rule: a.accept_rule,
        usage: std::path::absolute(a.dir.join("claude").join("usage.jsonl"))
            .map_err(|e| e.to_string())?
            .display()
            .to_string(),
        ..PlanInputs::default()
    };
    let backend = ClaudeBackend::new(ClaudeConfig {
        pin: Pin::of(&a.claude, &a.claude_version)?,
        expect,
        caps,
        routing: Routing::from_settings(&a.settings)?,
        dir: a.dir.join("claude"),
        shim: me.clone(),
        prompt,
        turns: a.turns,
        tools,
        upstream: None,
    })?;
    let root = a.dir.join("run");
    let cfg = RunConfig {
        root,
        mode: mode.name(),
        agents: a.agents,
        turns: a.turns,
        worker_bin: me,
        bracken,
        git: git_binary()?,
        trace: false,
        calls_per_turn: CALLS_PER_TURN,
        turn_limit: TURN_LIMIT,
        plan,
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
    bracken: Option<PathBuf>,
    sandbox: bool,
    plan: PlanInputs,
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
        plan,
        sandbox,
    };
    let (seed, task) = (cfg.plan.backend_seed, cfg.plan.task);
    let usage = Some(cfg.root.join("usage.jsonl"));
    // Through the shim, as Claude Code reaches the driver.
    let mock = MockBackend { task, seed, wrong_rate: 0.15, shim: Some(cfg.worker_bin.clone()), usage };
    let outcome = run(&cfg, &mock)?;
    for (module, text) in &outcome.modules {
        println!("# module {module}\n{text}");
    }
    println!("events: {}", outcome.events.display());
    // A mock turn that errs is a harness or mock fault, never a model's choice: fail the run (finding `373915eadd42`).
    let log = std::fs::read_to_string(&outcome.events).map_err(|e| format!("reading the event log: {e}"))?;
    let errors = log.lines().filter(|l| l.contains("\"event\":\"turn_error\"")).count();
    if errors > 0 {
        return Err(format!("{errors} turns ended in an error; see {}", outcome.events.display()));
    }
    Ok(())
}

/// The commit this binary was run from, if it runs in a checkout, marked `+dirty` with uncommitted changes.
fn harness_commit() -> String {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success());
        out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match (git(&["rev-parse", "HEAD"]), git(&["status", "--porcelain"])) {
        (Some(head), Some(status)) if !status.is_empty() => format!("{head}+dirty"),
        (Some(head), _) => head,
        _ => "unknown".into(),
    }
}

fn report(dirs: &[PathBuf], usage: Option<&std::path::Path>, json: bool) -> Result<(), String> {
    if usage.is_some() && dirs.len() > 1 {
        return Err("--usage is for one run".into());
    }
    let reports = dirs.iter().map(|d| bracken_experiment::plan::report(d, usage)).collect::<Result<Vec<_>, _>>()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reports).map_err(|e| e.to_string())?);
        return Ok(());
    }
    for r in &reports {
        println!("{} ({}): plan {}", r.measures.mode, r.dir, r.plan);
    }
    let measures: Vec<_> = reports.into_iter().map(|r| r.measures).collect();
    print!("\n{}", bracken_experiment::metrics::table(&measures));
    Ok(())
}
