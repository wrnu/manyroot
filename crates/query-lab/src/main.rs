use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bracken_query_lab::{bench, datalog, generate, output, pipeline};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "query-lab", about = "Research prototype: queries over a Ground, read-only")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Time each phase of loading a replica directory (the one holding ground.json and objects/).
    Profile {
        dir: PathBuf,
        /// Runs to take each phase's minimum over, which damps a busy machine's noise.
        #[arg(long, default_value_t = 5)]
        runs: usize,
    },
    /// Run a pipeline query, such as `tag pr | current`, and print its rows.
    Run {
        dir: PathBuf,
        query: String,
        /// ids, compact or json.
        #[arg(long, default_value = "compact")]
        format: String,
        /// Print the load and query times on stderr.
        #[arg(long)]
        time: bool,
    },
    /// Run a Datalog program (rules, then `?-` queries) on top of the prelude, and print each answer.
    Datalog {
        dir: PathBuf,
        program: String,
        /// Whose evidence triages findings ($triager).
        #[arg(long, default_value = "manager")]
        triager: String,
        /// Whose evidence counts toward acceptance; repeatable. Everyone's if absent.
        #[arg(long = "validator")]
        validators: Vec<String>,
        #[arg(long, default_value_t = 1)]
        min_passes: i64,
        /// A parameter naming a contribution, `name=<id or prefix>`, read as $name; repeatable.
        #[arg(long = "id", value_name = "NAME=ID")]
        ids: Vec<String>,
        /// Print the load and query times on stderr.
        #[arg(long)]
        time: bool,
        /// Don't apply the demand transformation (magic sets), to compare.
        #[arg(long)]
        no_magic: bool,
    },
    /// Run the representative queries both through a bracken binary and here, compare their answers id
    /// for id, and time both.
    Bench {
        dir: PathBuf,
        /// The bracken binary to compare against, e.g. the pinned one.
        #[arg(long)]
        bracken: PathBuf,
        /// Who triages, by local name or key.
        #[arg(long, default_value = "manager")]
        triager: String,
        /// How far back the cursor sits, in rows.
        #[arg(long, default_value_t = 300)]
        back: usize,
        /// The id whose referrers Q4 reads; the one with the most referrers if absent.
        #[arg(long)]
        about: Option<String>,
        #[arg(long, default_value_t = 5)]
        runs: usize,
        /// Only the cases whose names start with one of these, comma-separated (e.g. Q1,Q6,Q9).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Write a synthetic Ground of about N signed contributions to a new replica directory (no private
    /// keys are written).
    Generate {
        dir: PathBuf,
        #[arg(long)]
        n: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 14)]
        agents: usize,
    },
    /// Add about N synthetic contributions, by new agents, to a replica directory: what arrives between
    /// two reads. Only a replica `generate` wrote; any other Ground is refused.
    Extend {
        dir: PathBuf,
        #[arg(long)]
        n: usize,
        #[arg(long, default_value_t = 2)]
        seed: u64,
    },
    /// Open a replica with a snapshot (building, reusing or extending it) and say how it went.
    Open {
        dir: PathBuf,
        #[arg(long)]
        snapshot: PathBuf,
    },
    /// Time each way of loading a replica and the representative queries on it: Store::open, a snapshot
    /// built, reused and extended, and the queries warm.
    Scale {
        dir: PathBuf,
        /// Where the snapshot lives; outside the replica.
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long, default_value = "manager")]
        triager: String,
        #[arg(long, default_value_t = 3)]
        runs: usize,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Profile { dir, runs } => {
            let mut p = bracken_query_lab::profile::phases(&dir)?;
            for _ in 1..runs {
                p = p.min(&bracken_query_lab::profile::phases(&dir)?);
            }
            println!("minimum of {runs} runs per phase");
            println!("{} contributions, {} bytes", p.contributions, p.bytes);
            for (name, d) in [
                ("read files", p.read),
                ("decode", p.decode),
                ("verify signatures", p.verify),
                ("graph import (re-verifies)", p.import),
                ("Store::open total", p.store_open),
                ("build tables", p.tables),
            ] {
                println!("{name:<28} {:>8.1} ms", d.as_secs_f64() * 1000.0);
            }
        }
        Command::Run { dir, query, format, time } => {
            let stages = pipeline::parse(&query)?;
            let t = Instant::now();
            let g = bracken_query_lab::open(&dir)?;
            let load = t.elapsed();
            let t = Instant::now();
            let rows = pipeline::run(&g, &stages)?;
            let took = t.elapsed();
            match format.as_str() {
                "ids" => rows.iter().for_each(|r| println!("{}", g.ids[*r as usize])),
                "compact" => rows.iter().for_each(|r| println!("{}", output::compact(&g, *r))),
                "json" => {
                    let rows: Vec<_> = rows.iter().map(|r| output::json(&g, *r)).collect();
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                }
                f => anyhow::bail!("no format {f}: ids, compact or json"),
            }
            if time {
                eprintln!("{} rows; load {:.1} ms, query {:.3} ms", rows.len(), ms(load), ms(took));
            }
        }
        Command::Datalog { dir, program, triager, validators, min_passes, ids, time, no_magic } => {
            let ids: Vec<(String, String)> = ids
                .iter()
                .map(|p| p.split_once('=').map(|(n, v)| (n.to_owned(), v.to_owned())).context("--id takes name=id"))
                .collect::<Result<_>>()?;
            let t = Instant::now();
            let g = bracken_query_lab::open(&dir)?;
            let load = t.elapsed();
            let opts = datalog::Options {
                triager: Some(&triager),
                validators: &validators,
                min_passes,
                ids: &ids,
                magic: !no_magic,
            };
            let t = Instant::now();
            let (db, answers) = datalog::run(&g, &opts, &program)?;
            let took = t.elapsed();
            for a in &answers {
                println!("{}", a.vars.join("\t"));
                for row in &a.rows {
                    let cells: Vec<String> = row.iter().map(|v| datalog::show(&g, &db.syms, *v)).collect();
                    println!("{}", cells.join("\t"));
                }
            }
            if time {
                let rows: usize = answers.iter().map(|a| a.rows.len()).sum();
                let s = answers
                    .iter()
                    .fold((0, 0, 0), |acc, a| (acc.0 + a.stats.rules, acc.1 + a.stats.passes, acc.2 + a.stats.derived));
                eprintln!(
                    "{rows} rows; load {:.1} ms, evaluation {:.1} ms ({} rules, {} passes, {} facts derived)",
                    ms(load),
                    ms(took),
                    s.0,
                    s.1,
                    s.2
                );
            }
        }
        Command::Bench { dir, bracken, triager, back, about, runs, only } => {
            // The minimum of `runs` loads, as the CLI's times are the minimum of `runs` processes.
            let mut load = Duration::MAX;
            let mut g = None;
            for _ in 0..runs {
                let t = Instant::now();
                g = Some(bracken_query_lab::open(&dir)?);
                load = load.min(t.elapsed());
            }
            let g = g.context("--runs must be at least 1")?;
            anyhow::ensure!(!g.is_empty(), "the Ground has no contributions to query");
            // By default the busiest id: the most referrers.
            let about = about.unwrap_or_else(|| {
                let row = (0..g.len()).max_by_key(|r| g.referrers[*r].len()).expect("checked not empty above");
                g.ids[row].to_hex()
            });
            let mut cases = bench::cases(&g, &triager, back, &about)?;
            if !only.is_empty() {
                cases.retain(|c| only.iter().any(|o| c.name.starts_with(o.as_str())));
            }
            let outcomes = bench::run(&dir, &bracken, &g, &cases, runs)?;
            println!("{} contributions; load once (Store::open + tables) {:.1} ms", g.len(), ms(load));
            println!(
                "{:<34} {:>6} {:>6} {:>10} {:>10} {:>10}  same",
                "query", "rows", "calls", "cli ms", "warm ms", "cold ms"
            );
            let mut all_same = true;
            for o in &outcomes {
                all_same &= o.same;
                println!(
                    "{:<34} {:>6} {:>6} {:>10.1} {:>10.3} {:>10.1}  {}{}",
                    o.name,
                    o.rows,
                    o.calls,
                    ms(o.cli),
                    ms(o.warm),
                    ms(load + o.warm),
                    if o.same { "yes" } else { "NO " },
                    o.note
                );
            }
            let cli: Duration = outcomes.iter().map(|o| o.cli).sum();
            let warm: Duration = outcomes.iter().map(|o| o.warm).sum();
            let calls: usize = outcomes.iter().map(|o| o.calls).sum();
            println!(
                "all {} queries: cli {:.1} ms as {} processes; one process {:.1} ms (load once + {:.3} ms of queries)",
                outcomes.len(),
                ms(cli),
                calls,
                ms(load + warm),
                ms(warm)
            );
            if !all_same {
                anyhow::bail!("some answers differ from the CLI's");
            }
        }
        Command::Generate { dir, n, seed, agents } => {
            anyhow::ensure!(!dir.join("ground.json").exists(), "{} already holds a replica", dir.display());
            let t = Instant::now();
            let g = generate::ground(seed, n, agents);
            let made = t.elapsed();
            let ids = g.graph.topo_order();
            generate::write(&dir, &g.graph, &g.names, ids)?;
            println!(
                "{} contributions; generated in {:.1} s, written in {:.1} s",
                g.graph.len(),
                made.as_secs_f64(),
                (t.elapsed() - made).as_secs_f64()
            );
        }
        Command::Extend { dir, n, seed } => {
            generate::ensure_synthetic(&dir)?;
            let store = manyroot::Store::open(&dir)?;
            let mut graph = store.graph().clone();
            let identities: Vec<manyroot::Identity> =
                (0..3).map(|i| manyroot::Identity::generate(&format!("late-{seed}-{i}")).expect("a key")).collect();
            let added = generate::grow(&mut graph, &identities, seed, n);
            let names: Vec<_> = identities.iter().map(|i| (i.name().to_owned(), i.agent_id())).collect();
            generate::write(&dir, &graph, &names, added.iter().copied())?;
            println!("{} contributions added", added.len());
        }
        Command::Scale { dir, snapshot, triager, runs } => scale(&dir, &snapshot, &triager, runs)?,
        Command::Open { dir, snapshot } => {
            let t = Instant::now();
            let (g, report) = bracken_query_lab::snapshot::open(&dir, &snapshot)?;
            println!(
                "{} contributions in {:.1} ms: {} from the snapshot, {} new objects checked{}",
                g.len(),
                ms(t.elapsed()),
                report.cached,
                report.new,
                report.rebuilt.map(|w| format!(", rebuilt because {w}")).unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// The best of `runs` timings of `f`, and its last result.
fn best<T>(runs: usize, mut f: impl FnMut() -> Result<T>) -> Result<(Duration, T)> {
    let mut best = Duration::MAX;
    let mut out = None;
    for _ in 0..runs.max(1) {
        let t = Instant::now();
        out = Some(f()?);
        best = best.min(t.elapsed());
    }
    Ok((best, out.expect("at least one run")))
}

fn scale(dir: &std::path::Path, snap: &std::path::Path, triager: &str, runs: usize) -> Result<()> {
    use bracken_query_lab::snapshot;
    let (open, g) = best(runs, || bracken_query_lab::open(dir))?;
    println!("{} contributions", g.len());
    println!("{:<44} {:>10.1} ms", "Store::open + tables (verifies everything)", ms(open));
    let (build, _) = best(runs, || {
        let _ = std::fs::remove_file(snap);
        snapshot::open(dir, snap)
    })?;
    println!("{:<44} {:>10.1} ms", "snapshot built (Store::open, then written)", ms(build));
    let (reuse, (sg, report)) = best(runs, || snapshot::open(dir, snap))?;
    anyhow::ensure!(report.rebuilt.is_none() && report.new == 0, "the snapshot wasn't reused: {report:?}");
    anyhow::ensure!(sg.ids == g.ids && sg.fact == g.fact, "the snapshot's tables differ from Store::open's");
    println!("{:<44} {:>10.1} ms", "snapshot reused (nothing new)", ms(reuse));
    let size = std::fs::metadata(snap)?.len();
    println!("{:<44} {:>10.1} MB", "snapshot size", size as f64 / 1e6);
    let pr = g.tag_of.get("pr").and_then(|t| g.by_tag[*t as usize].last()).map(|r| g.ids[*r as usize].to_hex());
    let mid = g.ids[g.len() / 2].to_hex();
    let mut cases: Vec<(&str, bench::Ours)> = vec![
        ("Q1 pr claims, current (pipeline)", bench::Ours::Pipeline("tag pr | current".into())),
        ("Q2 everything current (pipeline)", bench::Ours::Pipeline("current".into())),
        ("Q3 findings since a cursor (pipeline)", bench::Ours::Pipeline(format!("since {mid} | tag finding"))),
    ];
    let dl = |program: &str, ids: Vec<(String, String)>| bench::Ours::Datalog {
        program: program.into(),
        triager: triager.into(),
        min_passes: 1,
        ids,
    };
    cases.push(("Q2 everything current (datalog)", dl("?- current(X).", vec![])));
    cases.push(("Q5 accepted (datalog)", dl("?- accepted(X).", vec![])));
    cases.push(("Q6 open findings (datalog)", dl("?- open_finding(F).", vec![])));
    cases.push(("Q8 ancestors of an id (datalog, magic)", dl("?- ancestor(A, $d).", vec![("d".into(), mid.clone())])));
    if let Some(pr) = pr {
        cases.push((
            "Q9 merge's 3 Ground checks, one program",
            dl("?- gate_current(C).\n?- gate_accepted(C).\n?- gate_blocking(F).", vec![("claim".into(), pr)]),
        ));
    }
    let (build, mut session) = best(1, || datalog::Session::new(&g))?;
    println!("{:<44} {:>10.1} ms", "datalog base relations, built once", ms(build));
    for (name, ours) in &cases {
        let (t, rows) = best(runs, || bench::answer_in(&mut session, &g, ours))?;
        println!("{name:<44} {:>10.1} ms  {} rows", ms(t), rows.len());
    }
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
