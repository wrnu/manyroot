//! The Ground benchmark (task `7a3d17f979c3`, Wave 2 E's measure step): times the `bracken` binary this
//! package builds on a synthetic Ground and, with `--copy`, on a copy of a real one.
//!
//! ```text
//! cargo bench -p bracken --bench ground -- [--synthetic N] [--mib M] [--copy <replica>] [--reads R]
//!                                          [--writes W] [--verifies V] [--work <dir>] [--keep] [--json <file>]
//! ```
//!
//! With no arguments it builds a small synthetic Ground (2,000 contributions, 4 MiB), quickly. Wave 2 E's
//! bar is `--synthetic 50000 --mib 200`. `--copy <replica>` (a `.bracken` directory) copies its
//! `ground.json`, `agents/` and `objects/` only, never `identities/` or `cache/`, and measures the copy;
//! `--synthetic 0` skips the synthetic Ground. Each Ground lives in a fresh directory under `--work`
//! (default `$TMPDIR/bracken-scratch/ground-bench`), removed at the end unless `--keep`.
//!
//! Per Ground, in this order:
//! - `verify`: the first `bracken verify` (every object read and checked, the snapshot written), then
//!   `--verifies` more, p50 and p95;
//! - reads, each run once to warm the cache and then `--reads` times, round-robin so load spikes spread
//!   over all of them, p50 and p95: `log --limit 50`, `log --tag decision --current`, `show <id>` (a claim
//!   with the median number of referrers among claims that have any), `accepted` and `conflicts`;
//! - writes: `--writes` claims, each timed, with the bytes of its object and the bytes it wrote to
//!   `cache/` (the snapshot), counted as a new file or a file renamed into place counting whole and a file
//!   that grew in place counting its growth.
//!
//! Every time is a whole process's wall time: what an agent waits for. The load average goes in the
//! output, since this machine is shared. Results print as a table; `--json` also writes them to a file.

mod measure;
mod synth;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use manyroot::{Body, ContributionId, Store};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "ground-bench")]
struct Args {
    /// Contributions in the synthetic Ground; 0 for none.
    #[arg(long, default_value_t = 2_000)]
    synthetic: usize,
    /// About how many MiB of objects the synthetic Ground holds.
    #[arg(long, default_value_t = 4)]
    mib: u64,
    /// A replica to copy and measure too (its keys and cache are left behind).
    #[arg(long, value_name = "REPLICA")]
    copy: Option<PathBuf>,
    /// Timed runs of each read.
    #[arg(long, default_value_t = 5)]
    reads: usize,
    /// Timed claim writes.
    #[arg(long, default_value_t = 3)]
    writes: usize,
    /// Timed `verify` runs after the first.
    #[arg(long, default_value_t = 1)]
    verifies: usize,
    #[arg(long, default_value_t = 7)]
    seed: u64,
    /// Where the Grounds are built.
    #[arg(long, value_name = "DIR")]
    work: Option<PathBuf>,
    /// Keep the Grounds after the run.
    #[arg(long)]
    keep: bool,
    /// Also write the results as JSON here.
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,
    /// Passed by `cargo bench`; ignored.
    #[arg(long, hide = true)]
    bench: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(args.reads > 0 && args.verifies > 0, "--reads and --verifies must be at least 1");
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_bracken"));
    let work = match &args.work {
        Some(w) => w.clone(),
        None => std::env::temp_dir().join("bracken-scratch").join("ground-bench"),
    };
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let scratch = tempfile::Builder::new().prefix("run-").tempdir_in(&work)?;
    println!("bracken {}; work in {}", bin.display(), scratch.path().display());

    let mut grounds = Vec::new();
    if args.synthetic > 0 {
        let dir = scratch.path().join("synthetic");
        let spec = synth::Spec { contributions: args.synthetic, bytes: args.mib << 20, agents: 8, seed: args.seed };
        println!("building a synthetic Ground: {} contributions, about {} MiB", spec.contributions, args.mib);
        let built = synth::build(&dir, &spec)?;
        println!("built {} contributions, {} bytes of objects", built.contributions, built.object_bytes);
        grounds.push(bench(&bin, "synthetic", &dir, &built.agents[0], &args)?);
    }
    if let Some(from) = &args.copy {
        let dir = scratch.path().join("copy");
        println!("copying {} (ground.json, agents/, objects/)", from.display());
        measure::copy_replica(from, &dir)?;
        measure::run(&bin, &dir, &["id", "new", "bench"], &[])?;
        grounds.push(bench(&bin, "copy", &dir, "bench", &args)?);
    }

    let report = json!({
        "bracken": bin,
        "load_average": load_average(),
        "reads": args.reads,
        "writes": args.writes,
        "verifies": args.verifies,
        "grounds": grounds,
    });
    if let Some(path) = &args.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)? + "\n")
            .with_context(|| format!("writing {}", path.display()))?;
        println!("results in {}", path.display());
    }
    if args.keep {
        println!("kept {}", scratch.keep().display());
    }
    Ok(())
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn stats(samples: &[Duration]) -> Value {
    json!({"p50_ms": ms(measure::percentile(samples, 50)), "p95_ms": ms(measure::percentile(samples, 95)), "runs": samples.len()})
}

/// A claim with the median number of referrers among claims that have any (the first claim if none has).
fn show_subject(dir: &Path) -> Result<ContributionId> {
    let store = Store::open(dir)?;
    let graph = store.graph();
    let mut claims: Vec<(usize, ContributionId)> = graph
        .iter()
        .filter(|c| matches!(c.body(), Body::Claim { .. }))
        .map(|c| (graph.referrers(&c.id()).count(), c.id()))
        .collect();
    claims.sort();
    let referred: Vec<_> = claims.iter().filter(|(n, _)| *n > 0).collect();
    let pick = referred.get(referred.len() / 2).copied().or(claims.first()).context("the Ground holds no claim")?;
    Ok(pick.1)
}

fn bench(bin: &Path, label: &str, dir: &Path, writer: &str, args: &Args) -> Result<Value> {
    let objects = measure::tree_bytes(&dir.join("objects"))?;
    println!("\n== {label}: {} MiB of objects in {}", objects >> 20, dir.display());

    let (first, _) = measure::run(bin, dir, &["verify"], &[])?;
    let snapshot = std::fs::metadata(dir.join("cache").join("snapshot")).map(|m| m.len()).unwrap_or(0);
    let mut verifies = Vec::new();
    for _ in 0..args.verifies {
        verifies.push(measure::run(bin, dir, &["verify"], &[])?.0);
    }
    let contributions = Store::open(dir)?.graph().len();
    let show = show_subject(dir)?.to_hex();
    println!("{contributions} contributions; snapshot {snapshot} bytes; show subject {show}");
    println!("verify: first {:.1} ms; {}", ms(first), line(&verifies));

    let reads: Vec<(&str, Vec<&str>)> = vec![
        ("log --limit 50", vec!["log", "--limit", "50"]),
        ("log --tag decision --current", vec!["log", "--tag", "decision", "--current"]),
        ("show <id>", vec!["show", &show]),
        ("accepted", vec!["accepted"]),
        ("conflicts", vec!["conflicts"]),
    ];
    let mut times: Vec<Vec<Duration>> = vec![Vec::new(); reads.len()];
    for (_, a) in &reads {
        measure::run(bin, dir, a, &[])?;
    }
    for _ in 0..args.reads {
        for (i, (_, a)) in reads.iter().enumerate() {
            times[i].push(measure::run(bin, dir, a, &[])?.0);
        }
    }
    let mut read_rows = serde_json::Map::new();
    for ((name, _), t) in reads.iter().zip(&times) {
        println!("{name:<30} {}", line(t));
        read_rows.insert((*name).to_owned(), stats(t));
    }

    let cache = dir.join("cache");
    let (mut write_times, mut object_bytes, mut cache_bytes) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..args.writes {
        let before = measure::files(&cache)?;
        let statement = format!("benchmark write {i}");
        let (took, out) = measure::run(bin, dir, &["claim", &statement, "--json"], &[("BRACKEN_AGENT", writer)])?;
        let after = measure::files(&cache)?;
        let written: Value = serde_json::from_str(&out).context("claim --json output")?;
        let id: ContributionId = written["id"].as_str().context("claim --json printed no id")?.parse()?;
        let hex = id.to_hex();
        object_bytes.push(std::fs::metadata(dir.join("objects").join(&hex[..2]).join(&hex[2..]))?.len());
        cache_bytes.push(measure::written(&before, &after));
        write_times.push(took);
    }
    let average = |v: &[u64]| if v.is_empty() { 0 } else { v.iter().sum::<u64>() / v.len() as u64 };
    if args.writes > 0 {
        println!(
            "claim write: {}; object {} bytes, cache (snapshot) {} bytes written per write (mean)",
            line(&write_times),
            average(&object_bytes),
            average(&cache_bytes)
        );
    }
    Ok(json!({
        "ground": label,
        "contributions": contributions,
        "object_bytes": objects,
        "snapshot_bytes": snapshot,
        "show_subject": show,
        "verify_first_ms": ms(first),
        "verify": stats(&verifies),
        "reads": read_rows,
        "write": if args.writes > 0 { stats(&write_times) } else { Value::Null },
        "write_object_bytes": object_bytes,
        "write_cache_bytes": cache_bytes,
    }))
}

fn line(samples: &[Duration]) -> String {
    format!(
        "p50 {:.1} ms, p95 {:.1} ms ({} runs)",
        ms(measure::percentile(samples, 50)),
        ms(measure::percentile(samples, 95)),
        samples.len()
    )
}

/// The 1-, 5- and 15-minute load averages, as `uptime` prints them, or nothing.
fn load_average() -> Value {
    std::process::Command::new("uptime")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split("load average").nth(1).map(|t| t.trim_start_matches(['s', ':', ' ']).trim().to_owned()))
        .map_or(Value::Null, Value::String)
}
