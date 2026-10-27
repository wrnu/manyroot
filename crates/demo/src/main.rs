//! `bracken-demo`: run the demo, or check a program against a spec.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use bracken_demo::bracken::Bracken;
use bracken_demo::harness::{self, Behaviour, Config, Schedule, StaleRule, Topology};
use bracken_demo::rng::Rng;
use bracken_demo::safety::{DEMO, keep_run, refuse_git_checkout, refuse_parent_in_new_part};
use bracken_demo::{compare, task};

#[derive(Parser)]
#[command(name = "bracken-demo", about = "Agents on a toy task through bracken, with a measured report.")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Args)]
struct RunArgs {
    /// Number of agents.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(2..=16))]
    agents: u16,
    /// Rounds of work. A sync follows each round, and a final all-to-all sync follows the last.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=100))]
    rounds: u32,
    /// Seed for the agents and the gossip topology. The same seed gives the same report, timings aside.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Who imports whose bundle after each round.
    #[arg(long, value_enum, default_value_t = Topology::Ring)]
    topology: Topology,
    /// Inputs per check. Smaller samples miss more edge-case bugs.
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u16).range(1..=169))]
    sample: u16,
    /// Rounds between starting an implementation and delivering it.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=10))]
    work_rounds: u32,
    /// Announce work with a wip claim before doing it, and skip specs others have announced.
    #[arg(long)]
    wip: bool,
    /// How agents decide another agent's wip claim is stale: by the round it states, or by
    /// heartbeats that stop reaching them.
    #[arg(long, value_enum, default_value_t = StaleRule::ClaimRound)]
    stale_rule: StaleRule,
    /// When agents start work relative to each other.
    #[arg(long, value_enum, default_value_t = Schedule::Lockstep)]
    schedule: Schedule,
    /// Rounds after its own round at which an open wip claim is stale. Defaults to one round
    /// after the latest the work could be due.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=20))]
    stale_after: Option<u32>,
    /// Put the replicas in this new directory and keep them, instead of a temporary one.
    /// It holds each agent's private key, unencrypted, so it gets a `.gitignore` that ignores everything.
    #[arg(long, value_name = "DIR")]
    keep: Option<PathBuf>,
    /// Allow the run's directory inside a git checkout. Refused by default, so agents never work
    /// where a repository's files, or its Ground, are within reach.
    #[arg(long)]
    allow_in_repo: bool,
    /// The bracken binary. Defaults to the one next to this executable, as cargo builds it.
    /// A relative path is relative to the current directory; PATH is never searched.
    #[arg(long, value_name = "PATH")]
    bracken: Option<PathBuf>,
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Check a program against a spec, on every input or on a seeded sample.
    Check {
        /// Spec name, e.g. max2.
        spec: String,
        /// File holding the program.
        file: PathBuf,
        /// Check this many inputs, chosen by --seed, instead of all of them.
        #[arg(long)]
        sample: Option<usize>,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        json: bool,
    },
    /// Print the language and the specs.
    Task,
    /// Run the same seeds with and without wip claims, for each topology, work length and
    /// schedule, and print the means side by side.
    Compare(CompareArgs),
}

#[derive(Args)]
struct CompareArgs {
    /// Seeds: a range like 1..10 (inclusive) or a list like 1,4,7.
    #[arg(long, default_value = "1..10", value_parser = parse_seeds)]
    seeds: Seeds,
    #[arg(long, value_enum, value_delimiter = ',', default_value = "ring,gossip")]
    topologies: Vec<Topology>,
    /// Rounds each piece of work takes, 0 to 10 each.
    #[arg(long, value_delimiter = ',', default_value = "1,2", value_parser = clap::value_parser!(u32).range(0..=10))]
    work_rounds: Vec<u32>,
    #[arg(long, value_enum, value_delimiter = ',', default_value = "lockstep,stagger,jitter")]
    schedules: Vec<Schedule>,
    /// Staleness rules for the wip rows.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "claim-round,liveness")]
    stale_rules: Vec<StaleRule>,
    /// Stale window for every wip row, instead of each rule's default.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=20))]
    stale_after: Option<u32>,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(2..=16))]
    agents: u16,
    /// Enough for jittered work to drift apart.
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u32).range(1..=100))]
    rounds: u32,
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u16).range(1..=169))]
    sample: u16,
    /// Runs at once, 1 to 64. Defaults to the number of CPUs.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=64))]
    jobs: Option<u8>,
    #[arg(long, value_name = "PATH")]
    bracken: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Clone)]
struct Seeds(Vec<u64>);

/// `a..b` (inclusive) or `a,b,c`.
fn parse_seeds(s: &str) -> Result<Seeds, String> {
    let bad = |_| format!("{s:?} is not a seed range like 1..10 or a list like 1,2,3");
    let seeds: Vec<u64> = match s.split_once("..") {
        Some((a, b)) => (a.parse().map_err(bad)?..=b.parse().map_err(bad)?).collect(),
        None => s.split(',').map(|n| n.trim().parse().map_err(bad)).collect::<Result<_, _>>()?,
    };
    if seeds.is_empty() || seeds.len() > 1000 {
        return Err(format!("{s:?} gives {} seeds; give 1 to 1000", seeds.len()));
    }
    Ok(Seeds(seeds))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bracken-demo: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Some(Command::Check { spec, file, sample, seed, json }) => {
            let s = task::spec(&spec).with_context(|| format!("no spec named {spec:?}"))?;
            let program = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let cases = match sample {
                Some(n) => s.sample(n, &mut Rng::new(seed)),
                None => s.grid(),
            };
            let check = s.check(&program, &cases);
            if json {
                let v = json!({"pass": check.ok(), "passed": check.passed, "total": check.total,
                               "first_failure": check.first_failure});
                println!("{}", serde_json::to_string_pretty(&v).expect("values serialize"));
            } else {
                println!("{} {}", if check.ok() { "pass" } else { "fail" }, check.details());
            }
            Ok(if check.ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        Some(Command::Task) => {
            println!("{}\n", task::LANGUAGE);
            for s in task::SPECS {
                println!("{:<7} {}", s.name, s.description);
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Compare(args)) => compare(args),
        None => demo(cli.run),
    }
}

fn demo(args: RunArgs) -> Result<ExitCode> {
    let bin = match args.bracken {
        Some(b) => b,
        None => default_bracken()?,
    };
    let bracken = Bracken::new(&bin)?;
    let cfg = Config {
        agents: usize::from(args.agents),
        rounds: args.rounds,
        seed: args.seed,
        topology: args.topology,
        sample: usize::from(args.sample),
        behaviour: Behaviour {
            stale_after: args.stale_after.unwrap_or_else(|| {
                Behaviour::new(args.work_rounds, args.wip, args.schedule).with_rule(args.stale_rule).stale_after
            }),
            ..Behaviour::new(args.work_rounds, args.wip, args.schedule).with_rule(args.stale_rule)
        },
    };

    // A bad --keep fails before the run, not after it.
    let keep = match &args.keep {
        Some(dir) => {
            let dir = std::path::absolute(dir).with_context(|| format!("resolving {}", dir.display()))?;
            if dir.exists() {
                bail!("{} already exists; --keep needs a new directory", dir.display());
            }
            refuse_parent_in_new_part(&dir)?;
            refuse_git_checkout(&dir, args.allow_in_repo, &DEMO)?;
            Some(dir)
        }
        None => None,
    };

    // The run always happens in a private temporary directory, where nobody else can
    // rename anything; --keep copies it out at the end, through directory handles.
    // Every path is absolute: bracken runs in each agent's work directory, where a
    // relative path would point somewhere else.
    let temp = tempfile::tempdir().context("creating a temporary directory")?;
    let root = std::path::absolute(temp.path()).context("resolving the temporary directory")?;
    refuse_git_checkout(&root, args.allow_in_repo, &DEMO)?;
    // The replicas hold unencrypted private keys: keep them out of any repository.
    let ignore = root.join(".gitignore");
    std::fs::write(&ignore, "*\n").with_context(|| format!("writing {}", ignore.display()))?;

    let report = harness::run_scripted(&bracken, &root, &cfg);
    if let Some(dest) = keep {
        let kept = keep_run(&root, &dest, |real| refuse_git_checkout(real, args.allow_in_repo, &DEMO))?;
        eprintln!("kept the run in {}", kept.display());
    }
    let report = report?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).expect("reports serialize"));
    } else {
        println!("{report}");
    }
    Ok(if report.replicas.converged { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn compare(args: CompareArgs) -> Result<ExitCode> {
    let bin = match args.bracken {
        Some(b) => b,
        None => default_bracken()?,
    };
    let bin = Bracken::new(&bin)?.bin().to_owned();
    let plan = compare::Plan {
        base: Config {
            agents: usize::from(args.agents),
            rounds: args.rounds,
            seed: 0,
            topology: Topology::Ring,
            sample: usize::from(args.sample),
            behaviour: Behaviour::default(),
        },
        seeds: args.seeds.0,
        topologies: args.topologies,
        work_rounds: args.work_rounds,
        schedules: args.schedules,
        stale_rules: args.stale_rules,
        stale_after: args.stale_after,
        jobs: args
            .jobs
            .map_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(64), usize::from),
    };
    let rows = compare::run(&bin, &plan)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows).expect("rows serialize"));
    } else {
        let seeds = &plan.seeds;
        println!(
            "Means over seeds {}: {} agents, {} rounds; accepted means under min passes 2\n",
            if seeds.len() > 2 { format!("{}..{}", seeds[0], seeds[seeds.len() - 1]) } else { format!("{seeds:?}") },
            plan.base.agents,
            plan.base.rounds
        );
        print!("{}", compare::table(&rows));
    }
    Ok(if rows.iter().all(|r| r.converged) { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// `bracken` next to this executable: cargo puts both in `target/<profile>/`
/// (test binaries are one level down, in `deps/`).
fn default_bracken() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding this executable")?;
    let name = format!("bracken{}", std::env::consts::EXE_SUFFIX);
    let dir = exe.parent().context("this executable has no directory")?;
    let candidates = [dir.join(&name), dir.parent().map(|p| p.join(&name)).unwrap_or_default()];
    Ok(candidates.iter().find(|c| c.is_file()).unwrap_or(&candidates[0]).clone())
}
