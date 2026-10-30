//! `bracken-experiment`: the Phase 1 experiment's processes.
//!
//! - `worker` holds one store and answers the driver (started by the driver).
//! - `shim` relays MCP between an agent harness and the driver (started by the harness).
//! - `guard` leads an agent turn's process group and kills it when the driver goes (started by the driver).
//! - `mock-run` runs a whole mode with the mock backend, for a look at the run directory.
//! - `report` measures finished runs and prints the comparison table.
//! - `run` runs a whole mode with one agent harness, Claude Code or Pi, playing every agent.

use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};

use bracken_experiment::backend::{Caps, ModeTokenCap, RUN_TIME_CAP, RUN_TOKEN_CAP};
use bracken_experiment::bracken_mode::AcceptRule;
use bracken_experiment::claude::{ClaudeBackend, ClaudeConfig, Expect, Pin, Routing};
use bracken_experiment::driver::{CALLS_PER_TURN, RunConfig, TURN_LIMIT, core_tools, git_binary, run};
use bracken_experiment::mock::MockBackend;
use bracken_experiment::mode::{StoreConfig, mode_under};
use bracken_experiment::pi::{PiBackend, PiConfig, PiExpect};
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
    /// Run one mode with an agent harness into a new directory, every worker sandboxed.
    Run {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, value_parser = ["bracken", "git", "crdt"])]
        mode: String,
        #[arg(long, default_value_t = 10)]
        agents: usize,
        #[arg(long, default_value_t = 8)]
        turns: u32,
        // No clap default: with one, an unset `--claude` would pass `required_if_eq`.
        /// Which agent harness plays every turn: Claude Code unless given.
        #[arg(long, value_parser = ["claude", "pi"])]
        harness: Option<String>,
        /// Claude Code: the versioned binary to pin (not a `latest` link).
        #[arg(long, required_unless_present = "harness", required_if_eq("harness", "claude"))]
        claude: Option<PathBuf>,
        /// Claude Code: the `claude_code_version` it must report.
        #[arg(long, required_unless_present = "harness", required_if_eq("harness", "claude"))]
        claude_version: Option<String>,
        /// Pi: a prefix `scripts/pi-prefix` built, pinned whole.
        #[arg(long, required_if_eq("harness", "pi"))]
        pi_prefix: Option<PathBuf>,
        /// Pi: the version the prefix must hold [default: 1.4.0].
        #[arg(long)]
        pi_version: Option<String>,
        /// A Claude Code settings file holding the routing entries, read for them alone: either harness
        /// sends its model calls there, with its token.
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
        /// A mode's token cap in place of the pilot's 25M, as MODE=TOKENS (e.g. bracken=30000000). It applies
        /// only when MODE is this run's mode; give it once per mode. The plan records the caps in effect.
        #[arg(long, value_name = "MODE=TOKENS")]
        mode_token_cap: Vec<ModeTokenCap>,
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
        Cmd::Guard => bracken_experiment::backend::guard(),
        Cmd::Run {
            dir,
            mode,
            agents,
            turns,
            harness,
            claude,
            claude_version,
            pi_prefix,
            pi_version,
            settings,
            prompt_file,
            model,
            task,
            bracken,
            accept_rule,
            mode_token_cap,
        } => agent_run(AgentRun {
            mode_token_cap,
            accept_rule: AcceptRule::named(&accept_rule).expect("clap checked the name"),
            task: task_named(&task),
            dir,
            mode,
            agents,
            turns,
            harness: HarnessArgs::of(harness.as_deref(), claude, claude_version, pi_prefix, pi_version)
                .unwrap_or_else(|e| refuse_run(&e)),
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

/// The harness `run` plays every turn with, and what it pins.
enum HarnessArgs {
    Claude { binary: PathBuf, version: String },
    Pi { prefix: PathBuf, version: String },
}

/// Ends the process as clap ends it on a bad `run` command line: the message, `run`'s usage, exit 2.
fn refuse_run(message: &str) -> ! {
    let mut cli = Cli::command();
    cli.build();
    let run = cli.find_subcommand_mut("run").expect("run is a subcommand");
    run.error(ErrorKind::ArgumentConflict, message).exit()
}

/// The Pi version a prefix must hold when `--pi-version` isn't given.
const PI_VERSION: &str = "1.4.0";

impl HarnessArgs {
    /// The harness `--harness` names, Claude Code unless given, from its own options. An option for the
    /// other harness is refused, not ignored (task 4cd030ab8dd1); clap has already asked for the
    /// harness's own.
    fn of(
        harness: Option<&str>,
        claude: Option<PathBuf>,
        claude_version: Option<String>,
        pi_prefix: Option<PathBuf>,
        pi_version: Option<String>,
    ) -> Result<HarnessArgs, String> {
        let pi = harness == Some("pi");
        let others: Vec<&str> = if pi {
            [("--claude", claude.is_some()), ("--claude-version", claude_version.is_some())].to_vec()
        } else {
            [("--pi-prefix", pi_prefix.is_some()), ("--pi-version", pi_version.is_some())].to_vec()
        }
        .into_iter()
        .filter_map(|(name, given)| given.then_some(name))
        .collect();
        if !others.is_empty() {
            let (theirs, this, other) =
                if pi { ("Claude Code's", "Pi", "claude") } else { ("Pi's", "Claude Code", "pi") };
            return Err(format!(
                "{} {} {theirs}, but the harness is {this}: drop {}, or give --harness {other}",
                others.join(" and "),
                if others.len() == 1 { "is" } else { "are" },
                if others.len() == 1 { "it" } else { "them" },
            ));
        }
        Ok(if pi {
            HarnessArgs::Pi {
                prefix: pi_prefix.expect("clap requires it"),
                version: pi_version.unwrap_or_else(|| PI_VERSION.into()),
            }
        } else {
            HarnessArgs::Claude {
                binary: claude.expect("clap requires it"),
                version: claude_version.expect("clap requires it"),
            }
        })
    }
}

struct AgentRun {
    task: Task,
    dir: PathBuf,
    mode: String,
    agents: usize,
    turns: u32,
    harness: HarnessArgs,
    settings: PathBuf,
    prompt_file: Option<PathBuf>,
    model: String,
    bracken: Option<PathBuf>,
    accept_rule: AcceptRule,
    mode_token_cap: Vec<ModeTokenCap>,
}

fn agent_run(a: AgentRun) -> Result<(), String> {
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
    let (expect, caps) = (Expect::pinned_2_1_311(&a.model), Caps::for_mode(&a.mode, &a.mode_token_cap)?);
    if caps.run_tokens != Caps::pilot().run_tokens {
        println!("this {} run's token cap is {} (--mode-token-cap); the plan records it", a.mode, caps.run_tokens);
    }
    // The backend's own directory, named for its harness.
    let backend_dir = a.dir.join(match a.harness {
        HarnessArgs::Claude { .. } => "claude",
        HarnessArgs::Pi { .. } => "pi",
    });
    let plan = PlanInputs {
        task: a.task,
        model: expect.model.clone(),
        effort: expect.effort.clone(),
        harness_commit: harness_commit(),
        caps: caps.plan_value(),
        system_prompt: blake3::hash(prompt.as_bytes()).to_hex().to_string(),
        expected_stops: vec![RUN_TOKEN_CAP.into(), RUN_TIME_CAP.into()],
        accept_rule: a.accept_rule,
        usage: std::path::absolute(backend_dir.join("usage.jsonl")).map_err(|e| e.to_string())?.display().to_string(),
        ..PlanInputs::default()
    };
    let cfg = |plan: PlanInputs| -> Result<RunConfig, String> {
        Ok(RunConfig {
            root: a.dir.join("run"),
            mode: mode.name(),
            agents: a.agents,
            turns: a.turns,
            worker_bin: me.clone(),
            bracken: bracken.clone(),
            git: git_binary()?,
            trace: false,
            calls_per_turn: CALLS_PER_TURN,
            turn_limit: TURN_LIMIT,
            plan,
            sandbox: true,
        })
    };
    let routing = Routing::from_settings(&a.settings)?;
    let outcome = match &a.harness {
        HarnessArgs::Claude { binary, version } => {
            let backend = ClaudeBackend::new(ClaudeConfig {
                pin: Pin::of(binary, version)?,
                expect,
                caps,
                routing,
                dir: backend_dir,
                shim: me.clone(),
                prompt,
                turns: a.turns,
                tools,
                upstream: None,
            })?;
            run(&cfg(PlanInputs { harness: backend.harness_record().clone(), ..plan })?, &backend)?
        }
        HarnessArgs::Pi { prefix, version } => {
            let backend = PiBackend::new(PiConfig {
                prefix: prefix.clone(),
                version: version.clone(),
                expect: PiExpect { provider: "anthropic".into(), model: expect.model, thinking: expect.effort },
                caps,
                routing,
                dir: backend_dir,
                shim: me.clone(),
                prompt,
                turns: a.turns,
                tools,
                upstream: None,
            })?;
            run(&cfg(PlanInputs { harness: backend.harness_record().clone(), ..plan })?, &backend)?
        }
    };
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
    let mock = MockBackend {
        task,
        seed,
        wrong_rate: 0.15,
        shim: Some(cfg.worker_bin.clone()),
        usage,
        ..MockBackend::default()
    };
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
