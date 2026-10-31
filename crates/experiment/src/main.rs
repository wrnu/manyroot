//! `bracken-experiment`: the Phase 1 experiment's processes.
//!
//! - `worker` holds one store and answers the driver (started by the driver).
//! - `shim` relays MCP between an agent harness and the driver (started by the harness).
//! - `guard` leads an agent turn's process group and kills it when the driver goes (started by the driver).
//! - `mock-run` runs a whole mode with the mock backend, for a look at the run directory.
//! - `report` measures finished runs and prints the comparison table.
//! - `run` runs a whole mode with one agent harness, Claude Code or Pi, playing every agent.
//! - `project check` checks an evaluation project, and its hidden set in a private store if given.
//! - `run-tests` runs a project's public tests on a tree, under the Python profile.
//! - `grade-run` grades a finished project run's trees with the project's hidden set, into a grades file.

use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};

use bracken_experiment::backend::{Caps, ModeTokenCap, RUN_TIME_CAP, RUN_TOKEN_CAP};
use bracken_experiment::bracken_mode::AcceptRule;
use bracken_experiment::claude::{ClaudeBackend, ClaudeConfig, Expect, Pin, Routing};
use bracken_experiment::driver::{
    CALLS_PER_TURN, RunConfig, TURN_LIMIT, check_socket_room, core_tools, git_binary, run,
};
use bracken_experiment::mock::MockBackend;
use bracken_experiment::mode::{StoreConfig, mode_under};
use bracken_experiment::pi::{PiBackend, PiConfig, PiCredential, PiExpect};
use bracken_experiment::plan::{PlanInputs, ProjectPlan};
use bracken_experiment::task::Task;

#[derive(Parser)]
#[command(name = "bracken-experiment", about = "The Phase 1 experiment's processes")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

// Parsed once, at start: the size of `Run`'s options costs nothing.
#[allow(clippy::large_enum_variant)]
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
        #[arg(long, value_parser = ["bracken", "git", "crdt", "weave"])]
        mode: String,
        #[arg(long, default_value_t = 10)]
        agents: usize,
        /// Weave mode: the evaluation project agents build, in place of the task (docs/design/eval-suite.md).
        #[arg(long)]
        project: Option<PathBuf>,
        /// A project run: the Python interpreter to pin, whose version must be the project's. The plan records
        /// it, and every run of agent code pins it from there.
        #[arg(long, requires = "project")]
        python: Option<PathBuf>,
        /// A project run: the digest the interpreter must have.
        #[arg(long, requires = "python")]
        python_digest: Option<String>,
        /// A project run: hold the project's change back, so only the items released at the start go out.
        #[arg(long, requires = "project")]
        hold_change: bool,
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
        /// Pi: the provider its model calls go to [default: anthropic]. anthropic is routed by
        /// --settings; openrouter goes to Pi's own OpenRouter URL with the key in --pi-auth.
        #[arg(long, value_parser = ["anthropic", "openrouter"])]
        pi_provider: Option<String>,
        /// Pi with openrouter: a Pi auth.json (Pi's /login writes one) holding an OpenRouter API key or
        /// an OpenRouter login. Only that entry's type and key, or its login's access and expiry, are read,
        /// into memory. A login expiring before the run's time cap and a 30-minute margin is refused; it
        /// is never refreshed, and nothing writes the file.
        #[arg(long)]
        pi_auth: Option<PathBuf>,
        /// Pi: the thinking level asked for [default: the plan's effort, medium]. Pick one the model
        /// supports: DeepSeek V4.1 Flash has off, low, high and max.
        #[arg(long, value_parser = ["off", "minimal", "low", "medium", "high", "xhigh", "max"])]
        pi_thinking: Option<String>,
        /// A Claude Code settings file holding the routing entries, read for them alone: Claude Code,
        /// and Pi with anthropic, send their model calls there, with its token.
        #[arg(long)]
        settings: Option<PathBuf>,
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
        #[arg(long, default_value = "bracken", value_parser = ["bracken", "git", "crdt", "weave"])]
        mode: String,
        #[arg(long, default_value_t = 3)]
        agents: usize,
        /// Weave mode: the evaluation project agents build, in place of the task (docs/design/eval-suite.md).
        #[arg(long)]
        project: Option<PathBuf>,
        /// A project run: the Python interpreter to pin, whose version must be the project's. The plan records
        /// it, and every run of agent code pins it from there.
        #[arg(long, requires = "project")]
        python: Option<PathBuf>,
        /// A project run: the digest the interpreter must have.
        #[arg(long, requires = "python")]
        python_digest: Option<String>,
        /// A project run: hold the project's change back, so only the items released at the start go out.
        #[arg(long, requires = "project")]
        hold_change: bool,
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
    /// Evaluation projects (docs/design/eval-suite.md, Project format).
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Run a project's public tests on a tree (the seed by default) with the pinned interpreter, under the
    /// Python profile (docs/design/eval-suite.md, Isolation).
    RunTests {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        tree: Option<PathBuf>,
        /// The Python interpreter to pin: its version must be the project's.
        #[arg(long)]
        python: PathBuf,
        /// The digest the interpreter must have, as a run's plan records it.
        #[arg(long)]
        python_digest: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Grade a finished project run: each checkpoint's tree and the end's, with the project's hidden set
    /// (docs/design/eval-suite.md, Running a project). One line a tree goes to --grades, a new file outside the
    /// run, the project, the store, any Ground and any git checkout.
    GradeRun {
        /// The run directory, holding plan.json and events.jsonl.
        #[arg(long)]
        dir: PathBuf,
        /// The private store holding the project's hidden set. Named nowhere but here.
        #[arg(long)]
        hidden: PathBuf,
        /// The grades file to write.
        #[arg(long)]
        grades: PathBuf,
        /// The bracken binary that checks trees out of the run's hub; defaults to the one next to this executable.
        #[arg(long)]
        bracken: Option<PathBuf>,
        #[arg(long)]
        json: bool,
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
            project,
            python,
            python_digest,
            hold_change,
            turns,
            harness,
            claude,
            claude_version,
            pi_prefix,
            pi_version,
            pi_provider,
            pi_auth,
            pi_thinking,
            settings,
            prompt_file,
            model,
            task,
            bracken,
            accept_rule,
            mode_token_cap,
        } => agent_run(AgentRun {
            project: project_inputs(project.as_deref(), python.as_deref(), python_digest.as_deref(), hold_change)
                .unwrap_or_else(|e| refuse_run(&e)),
            mode_token_cap,
            accept_rule: AcceptRule::named(&accept_rule).expect("clap checked the name"),
            task: task_named(&task),
            dir,
            mode,
            agents,
            turns,
            harness: HarnessOptions {
                harness,
                claude,
                claude_version,
                pi_prefix,
                pi_version,
                pi_provider,
                pi_auth,
                pi_thinking,
                settings,
            }
            .args()
            .unwrap_or_else(|e| refuse_run(&e)),
            prompt_file,
            model,
            bracken,
        }),
        Cmd::MockRun {
            dir,
            mode,
            agents,
            project,
            python,
            python_digest,
            hold_change,
            turns,
            seed,
            task,
            bracken,
            sandbox,
            accept_rule,
        } => {
            let accept_rule = AcceptRule::named(&accept_rule).expect("clap checked the name");
            let inputs =
                match project_inputs(project.as_deref(), python.as_deref(), python_digest.as_deref(), hold_change) {
                    Ok(inputs) => inputs,
                    Err(e) => {
                        eprintln!("bracken-experiment: {e}");
                        std::process::exit(1);
                    }
                };
            let plan = PlanInputs {
                task: task_named(&task),
                backend_seed: seed,
                harness_commit: harness_commit(),
                accept_rule,
                ..inputs.into_plan(PlanInputs::default())
            };
            mock_run(dir, &mode, agents, turns, bracken, sandbox, plan)
        }
        Cmd::Project(ProjectCmd::Check { dir, hidden, json }) => project_check(&dir, hidden.as_deref(), json),
        Cmd::Project(cmd @ ProjectCmd::Grade { .. }) => project_grade(cmd),
        Cmd::RunTests { project, tree, python, python_digest, json } => {
            run_tests(&project, tree.as_deref(), &python, python_digest.as_deref(), json)
        }
        Cmd::GradeRun { dir, hidden, grades, bracken, json } => std::env::current_exe()
            .map_err(|e| e.to_string())
            .map(|me| bracken.unwrap_or_else(|| me.with_file_name("bracken")))
            .and_then(|bracken| bracken_experiment::grade_run::grade_run(&dir, &hidden, &grades, &bracken))
            .map(|s| {
                if json {
                    println!("{}", serde_json::to_string(&s).expect("a summary is plain JSON"));
                } else {
                    println!(
                        "{} ({}): {} trees graded, {} passing all that applies, {} not graded",
                        s.project, s.hidden, s.points, s.all_pass, s.errors
                    );
                }
            }),
        Cmd::Report { dirs, usage, json } => report(&dirs, usage.as_deref(), json),
    };
    if let Err(e) = result {
        eprintln!("bracken-experiment: {e}");
        std::process::exit(1);
    }
}

/// A project run's plan inputs, from `--project`, `--python`, `--python-digest` and `--hold-change`: the project
/// as the plan records it, the interpreter pinned, the configuration's integrator setting and whether the change
/// is held back. None of them without a project; a project without `--python` is refused (answer
/// `5e40cacdbfb4`), and so is holding back a change a project hasn't.
fn project_inputs(
    project: Option<&std::path::Path>,
    python: Option<&std::path::Path>,
    digest: Option<&str>,
    hold_change: bool,
) -> Result<ProjectInputs, String> {
    let Some(dir) = project else { return Ok(ProjectInputs::default()) };
    let project = bracken_experiment::project::Project::load(dir)?;
    let python = python.ok_or("a project run needs --python, the interpreter to pin")?;
    if hold_change && project.first_change().is_none() {
        return Err(format!("--hold-change: project {} has no change to hold back", project.name));
    }
    let pin = bracken_experiment::python::Python::pin(python, &project.python, digest)?.plan_pin();
    Ok(ProjectInputs {
        project: Some(ProjectPlan::of(&project)?),
        python: Some(pin),
        integrator: Some(bracken_experiment::weave_mode::ANY_AGENT.to_string()),
        hold_change,
    })
}

/// The tasks `--task` takes.
const TASKS: [&str; 2] = ["phase1", "phase2"];

/// The task `name`, which the argument parser has checked is one of [`TASKS`].
fn task_named(name: &str) -> Task {
    Task::named(name).expect("the parser accepts only the tasks' names")
}

/// The harness `run` plays every turn with, what it pins, and where its model calls go.
enum HarnessArgs {
    Claude { binary: PathBuf, version: String, settings: PathBuf },
    Pi { prefix: PathBuf, version: String, thinking: Option<String>, provider: PiProvider },
}

/// Pi's provider and where its key comes from.
enum PiProvider {
    Anthropic { settings: PathBuf },
    OpenRouter { auth: PathBuf },
}

/// `run`'s harness options as given.
struct HarnessOptions {
    harness: Option<String>,
    claude: Option<PathBuf>,
    claude_version: Option<String>,
    pi_prefix: Option<PathBuf>,
    pi_version: Option<String>,
    pi_provider: Option<String>,
    pi_auth: Option<PathBuf>,
    pi_thinking: Option<String>,
    settings: Option<PathBuf>,
}

/// The refusal of options `given` for something else: `--x is <theirs>, but <but>: drop it, or give <fix>`.
fn not_for(given: &[(&str, bool)], theirs: &str, but: &str, fix: &str) -> Result<(), String> {
    let named: Vec<&str> = given.iter().filter_map(|(name, set)| set.then_some(*name)).collect();
    if named.is_empty() {
        return Ok(());
    }
    let (verb, them) = if named.len() == 1 { ("is", "it") } else { ("are", "them") };
    Err(format!("{} {verb} {theirs}, but {but}: drop {them}, or give {fix}", named.join(" and ")))
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

impl HarnessOptions {
    /// The harness `--harness` names, Claude Code unless given, from its own options, and for Pi the
    /// provider `--pi-provider` names, anthropic unless given, from its own. An option for another
    /// harness or provider is refused, not ignored (task 4cd030ab8dd1); clap has already asked for the
    /// harness's binary or prefix.
    fn args(self) -> Result<HarnessArgs, String> {
        let o = self;
        if o.harness.as_deref() != Some("pi") {
            let pi = [
                ("--pi-prefix", o.pi_prefix.is_some()),
                ("--pi-version", o.pi_version.is_some()),
                ("--pi-provider", o.pi_provider.is_some()),
                ("--pi-auth", o.pi_auth.is_some()),
                ("--pi-thinking", o.pi_thinking.is_some()),
            ];
            not_for(&pi, "Pi's", "the harness is Claude Code", "--harness pi")?;
            return Ok(HarnessArgs::Claude {
                binary: o.claude.expect("clap requires it"),
                version: o.claude_version.expect("clap requires it"),
                settings: o.settings.ok_or("Claude Code needs --settings, the file holding its routing entries")?,
            });
        }
        let claude = [("--claude", o.claude.is_some()), ("--claude-version", o.claude_version.is_some())];
        not_for(&claude, "Claude Code's", "the harness is Pi", "--harness claude")?;
        let provider = if o.pi_provider.as_deref() == Some("openrouter") {
            let but = "Pi's provider is openrouter";
            not_for(&[("--settings", o.settings.is_some())], "anthropic's", but, "--pi-provider anthropic")?;
            PiProvider::OpenRouter {
                auth: o.pi_auth.ok_or("Pi's openrouter provider needs --pi-auth, a Pi auth.json holding its key")?,
            }
        } else {
            let but = "Pi's provider is anthropic";
            not_for(&[("--pi-auth", o.pi_auth.is_some())], "openrouter's", but, "--pi-provider openrouter")?;
            PiProvider::Anthropic {
                settings: o.settings.ok_or("Pi's anthropic provider needs --settings, the file holding its routing")?,
            }
        };
        Ok(HarnessArgs::Pi {
            prefix: o.pi_prefix.expect("clap requires it"),
            version: o.pi_version.unwrap_or_else(|| PI_VERSION.into()),
            thinking: o.pi_thinking,
            provider,
        })
    }
}

/// A project run's plan inputs: see [`project_inputs`].
#[derive(Default)]
struct ProjectInputs {
    project: Option<ProjectPlan>,
    python: Option<bracken_experiment::python::PythonPin>,
    integrator: Option<String>,
    hold_change: bool,
}

impl ProjectInputs {
    /// `plan` with these inputs.
    fn into_plan(self, plan: PlanInputs) -> PlanInputs {
        let ProjectInputs { project, python, integrator, hold_change } = self;
        PlanInputs { project, python, integrator, hold_change, ..plan }
    }
}

struct AgentRun {
    project: ProjectInputs,
    task: Task,
    dir: PathBuf,
    mode: String,
    agents: usize,
    turns: u32,
    harness: HarnessArgs,
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
    // Before the backend makes its own directory: `run` checks again before making the run's.
    check_socket_room(&a.dir.join("run"), a.agents)?;
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
    // Pi's thinking level is the plan's effort, Claude Code's own unless --pi-thinking names one.
    let effort = match &a.harness {
        HarnessArgs::Pi { thinking: Some(t), .. } => t.clone(),
        _ => expect.effort.clone(),
    };
    let plan = PlanInputs {
        task: a.task,
        model: expect.model.clone(),
        effort: effort.clone(),
        harness_commit: harness_commit(),
        caps: caps.plan_value(),
        system_prompt: blake3::hash(prompt.as_bytes()).to_hex().to_string(),
        expected_stops: vec![RUN_TOKEN_CAP.into(), RUN_TIME_CAP.into()],
        accept_rule: a.accept_rule,
        usage: std::path::absolute(backend_dir.join("usage.jsonl")).map_err(|e| e.to_string())?.display().to_string(),
        ..a.project.into_plan(PlanInputs::default())
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
    let outcome = match &a.harness {
        HarnessArgs::Claude { binary, version, settings } => {
            let backend = ClaudeBackend::new(ClaudeConfig {
                pin: Pin::of(binary, version)?,
                expect,
                caps,
                routing: Routing::from_settings(settings)?,
                dir: backend_dir,
                shim: me.clone(),
                prompt,
                turns: a.turns,
                tools,
                upstream: None,
            })?;
            run(&cfg(PlanInputs { harness: backend.harness_record().clone(), ..plan })?, &backend)?
        }
        HarnessArgs::Pi { prefix, version, provider, .. } => {
            let credential = match provider {
                PiProvider::Anthropic { settings } => PiCredential::anthropic(&Routing::from_settings(settings)?)?,
                // A login must last the whole run and a margin (decision 467e1c8dbc09).
                PiProvider::OpenRouter { auth } => {
                    let until = std::time::SystemTime::now() + caps.run_time + bracken_experiment::pi::LOGIN_MARGIN;
                    PiCredential::openrouter(auth, until)?
                }
            };
            let backend = PiBackend::new(PiConfig {
                prefix: prefix.clone(),
                version: version.clone(),
                expect: PiExpect { provider: credential.provider().into(), model: expect.model, thinking: effort },
                caps,
                credential,
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

#[derive(Subcommand)]
enum ProjectCmd {
    /// Check a project; with --hidden, also its hidden set in the private store. Says only counts of what
    /// the store holds.
    Check {
        dir: PathBuf,
        /// The private store holding the project's hidden set.
        #[arg(long)]
        hidden: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Grade a tree with a project's hidden tests, in counts per item (docs/design/eval-suite.md, Grading);
    /// with --golden, the approval check of the hidden set's golden trees. Says nothing of the tests.
    Grade {
        dir: PathBuf,
        /// The private store holding the project's hidden set.
        #[arg(long)]
        hidden: PathBuf,
        /// The Python interpreter to pin: its version must be the project's.
        #[arg(long)]
        python: PathBuf,
        /// The digest the interpreter must have, as a run's plan records it.
        #[arg(long)]
        python_digest: Option<String>,
        /// The tree to grade, laid out as the seed is.
        #[arg(long, required_unless_present = "golden", conflicts_with = "golden")]
        tree: Option<PathBuf>,
        #[arg(long)]
        golden: bool,
        /// The share of the run's agent-turns used: the groups that apply then. The end by default.
        #[arg(long, default_value_t = 1.0)]
        share: f64,
        #[arg(long)]
        json: bool,
    },
}

fn project_grade(cmd: ProjectCmd) -> Result<(), String> {
    use bracken_experiment::project::{HiddenSet, Project};
    use bracken_experiment::project_grader::{Grade, Tree, golden_check, grade};
    use bracken_experiment::python::Python;
    let ProjectCmd::Grade { dir, hidden, python, python_digest, tree, golden, share, json } = cmd else {
        unreachable!("only Grade is passed here")
    };
    if !(0.0..=1.0).contains(&share) {
        return Err(format!("--share {share} isn't from 0 to 1"));
    }
    let project = Project::load(&dir)?;
    let set = HiddenSet::open(&project, &hidden)?;
    let python = Python::pin(&python, &project.python, python_digest.as_deref())?;
    let lines = |name: &str, g: &Grade| {
        println!("{name} at share {}: {}", g.share, if g.all_pass() { "all pass" } else { "not all pass" });
        for i in &g.items {
            let c = i.counts;
            let unfinished =
                if i.unfinished > 0 { format!(", {} groups unfinished", i.unfinished) } else { String::new() };
            println!("  {}: {} pass, {} fail, {} error, {} skip{unfinished}", i.id, c.pass, c.fail, c.error, c.skip);
        }
    };
    if golden {
        let check = golden_check(&python, &project, &set)?;
        if json {
            println!("{}", serde_json::json!({"passed": check.passed(), "check": check, "python": python.plan_pin()}));
        } else {
            lines("golden", &check.after);
            if let Some(before) = &check.before {
                lines("golden-before", before);
            }
            println!("approval check: {}", if check.passed() { "passed" } else { "failed" });
        }
        return if check.passed() { Ok(()) } else { Err("the golden trees don't pass the approval check".into()) };
    }
    let tree = tree.expect("the parser requires --tree without --golden");
    let g = grade(&python, &project, &set, Tree::Dir(&tree), share)?;
    if json {
        println!("{}", serde_json::json!({"grade": g, "python": python.plan_pin()}));
    } else {
        lines("tree", &g);
    }
    Ok(())
}

fn project_check(dir: &std::path::Path, hidden: Option<&std::path::Path>, json: bool) -> Result<(), String> {
    use bracken_experiment::project::{HiddenSet, Kind, Project};
    let project = Project::load(dir)?;
    let store = hidden.map(|store| HiddenSet::open(&project, store).map(|s| s.summary(&project))).transpose()?;
    let kinds = [Kind::Feature, Kind::Refactor, Kind::Fix, Kind::Change]
        .map(|k| (k.name(), project.items.iter().filter(|i| i.kind == k).count()));
    let later = project.items.len() - project.released(0.0).count();
    let (seed, tests) = (project.seed_files()?.len(), project.public_test_files()?.len());
    if json {
        let value = serde_json::json!({
            "name": project.name,
            "python": project.python,
            "items": project.items.len(),
            "kinds": kinds.iter().map(|(k, n)| (k.to_string(), serde_json::json!(n))).collect::<serde_json::Map<_, _>>(),
            "released_later": later,
            "seed_files": seed,
            "public_test_files": tests,
            "limits": project.limits,
            "hidden": {"id": project.hidden.id, "checked": store},
        });
        println!("{value}");
        return Ok(());
    }
    let kinds: Vec<String> = kinds.iter().filter(|(_, n)| *n > 0).map(|(k, n)| format!("{n} {k}")).collect();
    println!(
        "{}: python {}, {} items ({}), {later} released later",
        project.name,
        project.python,
        project.items.len(),
        kinds.join(", ")
    );
    println!("seed: {seed} files, {tests} public test files");
    match store {
        Some(s) => {
            println!(
                "hidden {}: digest matches; {} groups in {} test files; golden solution {} files, {} before the change",
                project.hidden.id, s.groups, s.test_files, s.golden_files, s.golden_before_files
            );
            if !s.items_without_tests.is_empty() {
                println!("items no hidden test judges: {}", s.items_without_tests.join(", "));
            }
        }
        None => println!("hidden {}: not checked (no --hidden)", project.hidden.id),
    }
    Ok(())
}

fn run_tests(
    project: &std::path::Path,
    tree: Option<&std::path::Path>,
    python: &std::path::Path,
    python_digest: Option<&str>,
    json: bool,
) -> Result<(), String> {
    use bracken_experiment::python::{Python, run_tests};
    let project = bracken_experiment::project::Project::load(project)?;
    let python = Python::pin(python, &project.python, python_digest)?;
    let run = run_tests(&python, &project, tree.unwrap_or(&project.seed))?;
    if json {
        println!("{}", serde_json::json!({"python": python, "run": run}));
        return Ok(());
    }
    eprint!("{}", run.output);
    if !run.reported {
        return Err(format!("the test runner reported nothing ({:?}): see its output above", run.exit));
    }
    let counts = ["pass", "fail", "error", "skip"].map(|o| format!("{} {o}", run.count(o)));
    println!(
        "{} tests: {} ({:?}); python {} {}",
        run.tests.len(),
        counts.join(", "),
        run.exit,
        python.version,
        python.digest
    );
    for (id, outcome) in run.tests.iter().filter(|(_, o)| o == "fail" || o == "error") {
        println!("{outcome}: {id}");
    }
    Ok(())
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
