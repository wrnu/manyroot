//! The check runner's two commands, `weave validate` and `weave integrate --run`
//! (docs/design/weave.md, section 3, *The check runner*; build step 7).
//!
//! Both run a line's named checks through [`Runner`] on a manifest this
//! replica holds whole, and record each result as evidence on the manifest's
//! artifact, found by its content hash: results attach to the manifest, so
//! any artifact of it will do (gotcha `b4d45c7352d4`). The method names the
//! check and its definition hash; the details give the manifest hash, the
//! attempt, the argv, how it ended and the output tail, withheld as its size
//! and hash if a content refusal matches it.
//!
//! `validate` runs only for one of the line's validators, and refuses a check
//! the validator already has a latest result for on the manifest unless
//! `--again`. `integrate --run` is for the integrator's own information: its
//! results are recorded but never counted, as their author is the decision's.
//!
//! No sandbox, no run: everything that could refuse is settled before
//! anything is written, the sandbox's own platform first, and a check that
//! can't run stops the command and records nothing for it.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use bracken_tree::ContentHash;
use bracken_tree::check::{CheckError, Outcome, RecordedTail, Runner, Toolchain, recorded_tail};
use bracken_tree::format::{CheckDefinition, Line, Manifest};
use bracken_tree::ground::Index;
use bracken_tree::import;
use bracken_tree::lines::Lines;
use bracken_tree::results::{self, Checks};
use bracken_tree::secrets::KnownKeys;
use clap::Args;
use manyroot::{Body, ContributionId, Identity, Store, Verdict};
use serde_json::{Value, json};

use super::{complete_manifest, manifest_hash, resolve_line};
use crate::{Output, contribute, decide, print_json, write_stdout};

/// How long a check may run when no `--timeout` is given: 30 minutes.
const DEFAULT_TIMEOUT: u64 = 30 * 60;

/// The longest `--timeout`: 7 days, in seconds (finding 0f0118a08480).
pub(super) const MAX_TIMEOUT: u64 = 7 * 24 * 60 * 60;

/// `weave validate`.
#[derive(Args)]
pub(crate) struct ValidateArgs {
    /// The manifest to check, blake3:<hex>: one this replica holds whole.
    #[arg(long, value_name = "HASH")]
    manifest: String,
    /// The line whose policy names the checks: its id or name. Defaults to the default line.
    #[arg(long, value_name = "LINE")]
    line: Option<String>,
    /// A check to run, by name. Repeat for several; the default is every check of the line's policy.
    #[arg(long = "check", value_name = "NAME")]
    checks: Vec<String>,
    /// Run a check you already have a latest result for on this manifest; the new result replaces it.
    #[arg(long)]
    again: bool,
    /// How long each check may run, in seconds, before its process group is killed: 1 to 604800 (7
    /// days), default 1800.
    #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..=MAX_TIMEOUT))]
    timeout: Option<u64>,
    /// Show the line, its validators and what would run, and run and write nothing.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    out: Output,
}

/// What runs the checks: the runner, and the keys a recorded tail must never hold.
pub(super) struct Run {
    runner: Runner,
    known: KnownKeys,
}

impl Run {
    /// A runner for this Ground's checks: the vendor directory beside the Ground (`~/bracken-weave/vendor`
    /// for `~/bracken-weave/manyroot`), the active rustup toolchain if there is one, and the timeout.
    /// Refuses where the sandbox can't exist, so that nothing is written before a run that can't happen.
    pub(super) fn new(s: &Store, timeout: Option<u64>) -> Result<Run> {
        if !cfg!(target_os = "macos") {
            bail!("no sandbox, so no check runs: checks run only on macOS, under sandbox-exec; nothing was written");
        }
        let ground = s.dir().canonicalize().with_context(|| format!("resolving {}", s.dir().display()))?;
        let vendor = ground.parent().context("the Ground has no parent directory")?.join("vendor");
        ensure!(
            vendor.is_dir(),
            "no vendor directory at {}: checks read their dependencies only from there, and the manager fills it with \
             cargo vendor (weave.md section 3); nothing was written",
            vendor.display()
        );
        let mut runner = Runner::new(vendor);
        runner.toolchain = toolchain()?;
        runner.timeout = Duration::from_secs(timeout.unwrap_or(DEFAULT_TIMEOUT));
        // --timeout is bounded when parsed; this keeps the runner's deadline from overflowing all the same.
        ensure!(
            std::time::Instant::now().checked_add(runner.timeout).is_some(),
            "--timeout {}: too long to count down; nothing was written",
            runner.timeout.as_secs()
        );
        let mut known = KnownKeys::new();
        known.add_identities_dir(&s.identities_dir()).context("reading this Ground's identities")?;
        // The coordination Ground's keys too, when run from one of its checkouts, as scripts/weave is.
        if let Ok(main) = import::main_checkout(std::path::Path::new(".")) {
            let coordination = main.join(".bracken").join("identities");
            known.add_identities_dir(&coordination).with_context(|| format!("reading {}", coordination.display()))?;
        }
        Ok(Run { runner, known })
    }
}

/// The owner's default rustup toolchain, found without running anything (finding 899bfab10dec): the
/// settings file of the rustup home, `$HOME/.rustup`, names it, and its directory must be
/// `toolchains/<name>` directly inside that home. So neither the caller's directory, where a candidate's
/// rust-toolchain.toml could name another toolchain or a program, nor RUSTUP_HOME, RUSTUP_TOOLCHAIN or
/// PATH in the caller's environment can choose it. Every check may read the rustup home, so one that is
/// $HOME, or holds it, is refused. None without a rustup home or a default toolchain: checks then have
/// the system's directories alone on their PATH.
fn toolchain() -> Result<Option<Toolchain>> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set, so the rustup home can't be found")?);
    let home = home.canonicalize().with_context(|| format!("resolving HOME, {}", home.display()))?;
    let Ok(rustup_home) = home.join(".rustup").canonicalize() else { return Ok(None) };
    ensure!(
        !home.starts_with(&rustup_home),
        "the rustup home {} is {} or holds it, and every check may read the rustup home; nothing was written",
        rustup_home.display(),
        home.display()
    );
    let Ok(settings) = fs::read_to_string(rustup_home.join("settings.toml")) else { return Ok(None) };
    let Some(name) = settings.lines().find_map(|l| {
        let value = l.trim().strip_prefix("default_toolchain")?.trim_start().strip_prefix('=')?.trim();
        value.strip_prefix('"')?.strip_suffix('"')
    }) else {
        return Ok(None);
    };
    ensure!(
        !name.is_empty()
            && !name.starts_with('.')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')),
        "rustup's default toolchain {name:?} isn't a toolchain name; nothing was written"
    );
    let Ok(toolchains) = rustup_home.join("toolchains").canonicalize() else { return Ok(None) };
    let Ok(sysroot) = toolchains.join(name).canonicalize() else { return Ok(None) };
    ensure!(
        sysroot.parent() == Some(toolchains.as_path()) && toolchains.starts_with(&rustup_home),
        "the toolchain {name} resolves to {}, outside the rustup home's toolchains, {}; nothing was written",
        sysroot.display(),
        toolchains.display()
    );
    let bin = sysroot.join("bin");
    if !bin.join("rustc").is_file() {
        return Ok(None);
    }
    Ok(Some(Toolchain { bin, rustup_home }))
}

/// One check to run.
struct Planned {
    name: String,
    argv: Vec<String>,
    method: String,
    attempt: u64,
}

/// What a run recorded, and the error that stopped it, if one did.
pub(super) struct Recorded {
    pub(super) results: Vec<Value>,
    pub(super) passed: bool,
    pub(super) failure: Option<anyhow::Error>,
}

/// The single current policy version of `line`, refused while its chain has diverged.
fn current_policy<'l>(lines: &'l Lines, line: &ContributionId) -> Result<(ContributionId, &'l Line)> {
    match lines.current_versions(line).as_slice() {
        [one] => Ok((*one, &lines.version(one).expect("a current version").policy)),
        many => {
            let ids: Vec<String> = many.iter().map(ContributionId::short).collect();
            bail!("line {}'s policy has diverged into {}; its author settles it first", line.short(), ids.join(", "))
        }
    }
}

pub(crate) fn validate(s: &mut Store, agent: Option<&str>, args: ValidateArgs) -> Result<ExitCode> {
    let ValidateArgs { manifest, line, checks: names, again, timeout, dry_run, out } = args;
    let ix = Index::build(s.graph());
    if !dry_run {
        super::merge::require_marker(&ix)?;
    }
    let lines = Lines::build(s.graph(), &ix);
    let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
    let (version, policy) = current_policy(&lines, &line)?;
    let who = crate::signer(s, agent)?;
    if !policy.validators.contains(&who.agent_id()) {
        bail!(
            "{} isn't one of line {}'s validators under its policy {}, so it runs no checks for it; nothing ran",
            who.agent_id(),
            line.short(),
            version.short()
        );
    }
    let hash = manifest_hash(s, &ix, &manifest, "--manifest")?;
    let held = complete_manifest(&ix, &hash)?;
    let (target, _) = ix.manifest(&hash).expect("a held manifest");
    ensure!(
        !policy.checks.is_empty(),
        "line {}'s policy {} names no checks; nothing ran",
        line.short(),
        version.short()
    );
    let mut wanted: Vec<String> = Vec::new();
    for name in &names {
        if !policy.checks.contains_key(name) {
            let known: Vec<&str> = policy.checks.keys().map(String::as_str).collect();
            bail!("--check {name:?}: line {}'s policy has no such check; it has {}", line.short(), known.join(", "));
        }
        if !wanted.contains(name) {
            wanted.push(name.clone());
        }
    }
    if wanted.is_empty() {
        wanted = policy.checks.keys().cloned().collect();
    }
    let results = Checks::build(s.graph(), &ix, &lines);
    let mut planned = Vec::new();
    let mut refused = Vec::new();
    for name in wanted {
        let method = policy.check_method(&name).expect("one of its checks");
        let latest = results.latest(&who.agent_id(), &hash, &method);
        if !latest.is_empty() && !again {
            let ids: Vec<String> = latest.iter().map(ContributionId::short).collect();
            refused.push(format!("{name} ({})", ids.join(", ")));
        }
        let attempt = results.next_attempt(&who.agent_id(), &hash, &method);
        planned.push(Planned { argv: policy.checks[&name].clone(), name, method, attempt });
    }
    if !refused.is_empty() {
        bail!(
            "you already have a latest result on {hash} for {}; give --again to run them again; nothing ran",
            refused.join(", ")
        );
    }
    if dry_run {
        let rows: Vec<Value> = planned
            .iter()
            .map(|p| json!({"check": p.name, "method": p.method, "argv": p.argv, "attempt": p.attempt}))
            .collect();
        if out.json {
            let validators: Vec<String> = policy.validators.iter().map(ToString::to_string).collect();
            print_json(&json!({"line": line, "policy": version, "signer": who.agent_id().to_string(),
                               "validators": validators, "manifest": hash.to_string(), "manifest_id": target,
                               "checks": rows}));
        } else {
            println!("policy {version}");
            for p in &planned {
                println!("would run {} attempt {}: {:?} (dry run, nothing run or written)", p.name, p.attempt, p.argv);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let run = Run::new(s, timeout)?;
    let recorded = run_checks(s, &who, &run, &ix, version, &hash, target, held, &planned, out.json);
    if out.json {
        print_json(&json!({"line": line, "policy": version, "manifest": hash.to_string(), "manifest_id": target,
                           "results": recorded.results}));
    }
    match recorded.failure {
        Some(e) => {
            decide(false);
            Err(e.context(format!("{} results were recorded before it stopped", recorded.results.len())))
        }
        None => Ok(decide(recorded.passed)),
    }
}

/// `integrate --run`, step 3: runs every check of the line's current policy on the candidate, whose
/// manifest this replica now holds, and records each result signed by the integrator. Their attempts
/// count on from the integrator's own earlier results, which no validator's index holds.
pub(super) fn run_for_integrate(
    s: &mut Store,
    who: &Identity,
    run: &Run,
    line: &ContributionId,
    hash: &ContentHash,
    json: bool,
) -> Result<Recorded> {
    let ix = Index::build(s.graph());
    let lines = Lines::build(s.graph(), &ix);
    let (version, policy) = current_policy(&lines, line)?;
    let manifest = complete_manifest(&ix, hash)?;
    let (target, _) = ix.manifest(hash).expect("a held manifest");
    let planned: Vec<Planned> = policy
        .checks
        .iter()
        .map(|(name, argv)| {
            let method = policy.check_method(name).expect("one of its checks");
            let highest = ix
                .check_results()
                .iter()
                .filter(|r| r.author == who.agent_id() && r.manifest == *hash && r.method == method)
                .filter_map(|r| r.attempt)
                .max();
            Planned { name: name.clone(), argv: argv.clone(), method, attempt: highest.unwrap_or(0).saturating_add(1) }
        })
        .collect();
    Ok(run_checks(s, who, run, &ix, version, hash, target, manifest, &planned, json))
}

/// Runs each planned check on `manifest` and records its result on `target`, printing the policy
/// version and each argv before it runs. Stops at the first check that can't run, recording nothing
/// for it.
#[allow(clippy::too_many_arguments)]
fn run_checks(
    s: &mut Store,
    who: &Identity,
    run: &Run,
    ix: &Index,
    version: ContributionId,
    hash: &ContentHash,
    target: ContributionId,
    manifest: &Manifest,
    planned: &[Planned],
    json: bool,
) -> Recorded {
    // With --json, stdout holds only the JSON, so what is printed first goes to stderr.
    let say = |line: String| if json { eprintln!("{line}") } else { println!("{line}") };
    let mut recorded = Recorded { results: vec![], passed: true, failure: None };
    if planned.is_empty() {
        return recorded;
    }
    say(format!("policy {version}"));
    for p in planned {
        say(format!("run {} attempt {}: {}", p.name, p.attempt, crate::terminal::printable(&format!("{:?}", p.argv))));
        let check = CheckDefinition { name: &p.name, argv: &p.argv };
        let outcome: Outcome = match run.runner.run(&check, manifest, |h| ix.file(s.graph(), h).map(<[u8]>::to_vec)) {
            Ok(o) => o,
            Err(e) => {
                let why = match e {
                    CheckError::NoSandbox(_) => "it didn't run, and nothing was recorded for it",
                    _ => "it couldn't run, and nothing was recorded for it",
                };
                recorded.failure = Some(anyhow::Error::new(e).context(format!("check {}: {why}", p.name)));
                return recorded;
            }
        };
        let tail = recorded_tail(&outcome.tail, &run.known);
        let passed = outcome.exit.passed();
        let body = Body::Evidence {
            target,
            verdict: if passed { Verdict::Pass } else { Verdict::Fail },
            method: outcome.method.clone(),
            details: results::details(hash, p.attempt, &outcome.argv, &outcome.exit, &tail),
        };
        let id = match contribute(s, who, body) {
            Ok(id) => id,
            Err(e) => {
                recorded.failure =
                    Some(e.context(format!("check {} ran ({}), but its result wasn't recorded", p.name, outcome.exit)));
                return recorded;
            }
        };
        recorded.passed &= passed;
        let withheld = match &tail {
            RecordedTail::Text(_) => Value::Null,
            RecordedTail::Withheld { size, hash, rule } => {
                json!({"size": size, "hash": hash.to_string(), "rule": rule.to_string()})
            }
        };
        if !json {
            let verdict = if passed { "pass" } else { "fail" };
            let note = if withheld.is_null() { String::new() } else { ", output withheld".to_owned() };
            println!("evidence {id} ({verdict}: {}, {}{note})", p.name, outcome.exit);
        }
        recorded.results.push(json!({"evidence": id, "check": p.name, "method": outcome.method,
                                     "attempt": p.attempt, "argv": outcome.argv, "passed": passed,
                                     "exit": outcome.exit.to_string(), "withheld": withheld}));
    }
    recorded
}
