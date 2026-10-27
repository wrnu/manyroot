//! `bracken-demo`: run the demo, or check a program against a spec.

use std::path::{Path, PathBuf};
use std::process::{self, ExitCode, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use bracken_demo::agent::{Agent, ScriptedAgent};
use bracken_demo::bracken::Bracken;
use bracken_demo::harness::{self, Config, Topology};
use bracken_demo::rng::Rng;
use bracken_demo::{report, task};

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
    };

    // Every path from here on is absolute: bracken runs in each agent's work
    // directory, where a relative path would point somewhere else.
    let _temp;
    let root = match args.keep {
        Some(dir) => {
            let dir = std::path::absolute(&dir).with_context(|| format!("resolving {}", dir.display()))?;
            if dir.exists() {
                bail!("{} already exists; --keep needs a new directory", dir.display());
            }
            refuse_git_checkout(&dir, args.allow_in_repo)?;
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            dir
        }
        None => {
            _temp = tempfile::tempdir().context("creating a temporary directory")?;
            let dir = std::path::absolute(_temp.path()).context("resolving the temporary directory")?;
            refuse_git_checkout(&dir, args.allow_in_repo)?;
            dir
        }
    };
    // The replicas hold unencrypted private keys: keep them out of any repository.
    let ignore = root.join(".gitignore");
    std::fs::write(&ignore, "*\n").with_context(|| format!("writing {}", ignore.display()))?;

    let start = Instant::now();
    let run = harness::run(&bracken, &root, &cfg, |i, name| -> Box<dyn Agent> {
        Box::new(ScriptedAgent::new(name, cfg.seed, i as u64, cfg.sample))
    })?;
    let report = report::build(&cfg, &run, start.elapsed(), bracken.calls(), bracken.elapsed());
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).expect("reports serialize"));
    } else {
        println!("{report}");
    }
    Ok(if report.replicas.converged { ExitCode::SUCCESS } else { ExitCode::FAILURE })
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

/// Fails if `path`, which need not exist yet, is inside a git checkout, or if the
/// environment points git at one, unless `allowed`.
fn refuse_git_checkout(path: &Path, allowed: bool) -> Result<()> {
    if allowed {
        return Ok(());
    }
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        if std::env::var_os(var).is_some() {
            bail!(
                "{var} is set, so git may treat {} as part of a checkout; unset it, or pass --allow-in-repo",
                path.display()
            );
        }
    }
    let abs = std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;
    let existing = abs.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let real = std::fs::canonicalize(existing).with_context(|| format!("resolving {}", existing.display()))?;
    let refuse = |checkout: &Path| {
        bail!(
            "{} is inside the git checkout {}; run the demo outside any repository, or pass --allow-in-repo",
            path.display(),
            checkout.display()
        )
    };
    // Ask git, which also knows work trees whose git directory is elsewhere.
    let git = process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree", "--show-toplevel"])
        .current_dir(&real)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = git
        && out.status.success()
        && out.stdout.starts_with(b"true")
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        return refuse(Path::new(stdout.lines().nth(1).unwrap_or("?")));
    }
    // And look for a .git ourselves, in case git isn't installed.
    if let Some(checkout) = real.ancestors().find(|p| p.join(".git").exists()) {
        return refuse(checkout);
    }
    Ok(())
}
