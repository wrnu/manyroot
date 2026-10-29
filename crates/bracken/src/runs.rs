//! `bracken view --runs <dir>`: one experiment run, read once at start and never written (the Runs
//! page, docs/design/visualizer.md section 1.10).
//!
//! The directory is either one mode's run (it holds `plan.json`, or `run/plan.json` as `bracken-experiment
//! run --dir` writes it) or a directory whose immediate children are mode runs, one per mode, for the
//! side-by-side comparison. Nothing deeper is looked at. Each run must hold the `report.json` that
//! `bracken-experiment report --json` wrote for it, whose plan hash must be `plan.json`'s and the one
//! the run logged at setup. The page is given only that report's measures and the plan's waves. No
//! file is ever served, no path leaves this module, and a symlink anywhere it reads is refused.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

/// The most `plan.json` or `report.json` may hold.
const MAX_FILE: u64 = 16 << 20;
/// How far into `events.jsonl` the setup event may be: it's the first line the driver writes.
const SETUP_WITHIN: u64 = 4 << 20;

/// Refuses `--runs` unless every address the view listens on is loopback, so run data never reaches
/// a network view by accident.
pub fn check_binds(addresses: &[IpAddr]) -> Result<()> {
    if let Some(ip) = addresses.iter().find(|ip| !ip.is_loopback()) {
        bail!("--runs is served on loopback only, and this view would also listen on {ip}: drop --bind or --runs");
    }
    Ok(())
}

/// What `/api/runs` serves for the run at `dir` (see the module docs), or why it can't be read.
pub fn load(dir: &Path) -> Result<Value> {
    let root = dir.canonicalize().with_context(|| format!("--runs {}", dir.display()))?;
    ensure!(root.is_dir(), "--runs {} isn't a directory", dir.display());
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok()) {
        ensure!(
            !home.starts_with(&root),
            "--runs {} is a home directory or holds one: give the run's own directory",
            dir.display()
        );
    }
    let name = |p: &Path| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    if let Some(run) = run_in(&root)? {
        found.push((name(&root), run));
    } else {
        let mut children: Vec<PathBuf> = fs::read_dir(&root)
            .with_context(|| format!("reading {}", root.display()))?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        children.sort();
        for child in children {
            if name(&child).starts_with('.') || !real(&child)?.is_some_and(|m| m.is_dir()) {
                continue;
            }
            if let Some(run) = run_in(&child)? {
                found.push((name(&child), run));
            }
        }
    }
    ensure!(
        !found.is_empty(),
        "{} holds no experiment run: neither plan.json, run/plan.json, nor a directory of mode runs",
        dir.display()
    );
    let mut runs = Vec::new();
    let mut alike: Option<Value> = None;
    for (name, run) in &found {
        let (shown, plan) = one(name, run)?;
        let key = json!([plan["inputs"]["task"], plan["agents"], plan["turns"], plan["waves"]]);
        match &alike {
            None => alike = Some(key),
            Some(first) => ensure!(
                *first == key,
                "{name}'s plan differs from {}'s in task, agents, turns or waves, so they don't compare",
                found[0].0
            ),
        }
        let mode = &shown["measures"]["mode"];
        ensure!(
            !runs.iter().any(|r: &Value| r["measures"]["mode"] == *mode),
            "two runs of mode {mode}: a comparison is one run per mode"
        );
        runs.push(shown);
    }
    Ok(json!({ "runs": runs }))
}

/// The run in `dir`: `dir` itself if it holds plan.json, else its `run` directory if that does.
fn run_in(dir: &Path) -> Result<Option<PathBuf>> {
    if real(&dir.join("plan.json"))?.is_some() {
        return Ok(Some(dir.to_owned()));
    }
    let run = dir.join("run");
    match real(&run)? {
        Some(m) if m.is_dir() && real(&run.join("plan.json"))?.is_some() => Ok(Some(run)),
        _ => Ok(None),
    }
}

/// The metadata of `path` if it exists, refusing a symlink.
fn real(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            bail!("{} is a symlink: --runs reads only what's really inside the directory it's given", path.display())
        }
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A file's bytes, at most [`MAX_FILE`], refusing a symlink.
fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let Some(m) = real(path)? else { return Ok(None) };
    ensure!(m.is_file(), "{} isn't a file", path.display());
    ensure!(m.len() <= MAX_FILE, "{} is over {} MiB", path.display(), MAX_FILE >> 20);
    let mut bytes = Vec::new();
    File::open(path)?.take(MAX_FILE).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// One run: what the page is given, and its plan (for the comparison's check).
fn one(name: &str, dir: &Path) -> Result<(Value, Value)> {
    let plan_bytes = read(&dir.join("plan.json"))?.with_context(|| format!("{} has no plan.json", dir.display()))?;
    let plan: Value = serde_json::from_slice(&plan_bytes).with_context(|| format!("{}/plan.json", dir.display()))?;
    let hash = blake3::hash(&plan_bytes).to_hex().to_string();
    let Some(report) = read(&dir.join("report.json"))? else {
        bail!(
            "{0} has no report.json. Make one with: bracken-experiment report --json {0} > {0}/report.json",
            dir.display()
        );
    };
    let report: Value = serde_json::from_slice(&report).with_context(|| format!("{}/report.json", dir.display()))?;
    // `report --json` prints a list, one report per directory it was given.
    let report = match report {
        Value::Array(mut list) if list.len() == 1 => list.remove(0),
        Value::Object(_) => report,
        _ => bail!("{}/report.json isn't one run's report", dir.display()),
    };
    ensure!(
        report["plan"] == hash.as_str(),
        "{}/report.json was made for another plan than this plan.json",
        dir.display()
    );
    ensure!(report["measures"].is_object(), "{}/report.json has no measures", dir.display());
    ensure!(
        setup_plan(&dir.join("events.jsonl"))?.as_deref() == Some(hash.as_str()),
        "{}/plan.json isn't the plan the run logged at setup",
        dir.display()
    );
    let shown = json!({ "name": name, "plan": hash, "waves": plan["waves"], "measures": report["measures"] });
    Ok((shown, plan))
}

/// The plan hash the run's setup event logged, reading no further than it.
fn setup_plan(path: &Path) -> Result<Option<String>> {
    real(path)?.with_context(|| format!("{} is missing", path.display()))?;
    let mut lines = BufReader::new(File::open(path)?.take(SETUP_WITHIN));
    let mut line = String::new();
    while lines.read_line(&mut line)? > 0 {
        if let Ok(event) = serde_json::from_str::<Value>(&line)
            && event["event"] == "setup"
        {
            return Ok(event["data"]["plan"].as_str().map(str::to_owned));
        }
        line.clear();
    }
    Ok(None)
}
