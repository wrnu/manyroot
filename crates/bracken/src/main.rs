//! `bracken`: agent-native collaborative development on Manyroot.
//!
//! Every command that reads the graph accepts `--json` so agents can consume
//! it directly. Writes print the new contribution's full id on stdout.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use manyroot::store::{self, GroundInfo};
use manyroot::validation::{Assessor, Trusted, open_conflicts, open_supersessors, relation_counts};
use manyroot::*;
use serde_json::{Value, json};

mod listing;
mod remote;
mod runs;
mod serve;
mod terminal;
mod verify;
mod view;
mod wire;

/// `println!`, through [`write_stdout`]. Shadows the standard macro in this file.
macro_rules! println {
    () => {
        write_stdout(b"\n")
    };
    ($($arg:tt)*) => {
        write_stdout(format!("{}\n", format_args!($($arg)*)).as_bytes())
    };
}

// Declared after `println!`, so the modules use it too.
mod watch;
mod weave;

/// Directory holding a Bracken workspace's Ground replica.
const DIR_NAME: &str = ".bracken";

#[derive(Parser)]
#[command(name = "bracken", version, about = "Software that grows together.")]
struct Cli {
    /// Ground replica to use. Defaults to the nearest .bracken directory.
    #[arg(long, global = true, env = "BRACKEN_DIR")]
    dir: Option<PathBuf>,

    /// Local identity to sign with. Commands that only read ignore it.
    #[arg(long = "as", global = true, env = "BRACKEN_AGENT", value_name = "NAME")]
    agent: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Output {
    /// Print machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct PolicyArgs {
    /// Independent validators that must report a pass.
    #[arg(long, default_value_t = 1)]
    min_passes: usize,
    /// Only count evidence from these agents (local name or public key). Repeatable.
    #[arg(long = "validator", value_name = "AGENT")]
    validators: Vec<String>,
    /// Acceptance follows changes: an accepted contribution whose dependencies (through depends_on,
    /// implements and refines relations that count) were superseded or retracted after its passes shows
    /// as needs_recheck, and is out of the accepted layer until a pass that has seen them.
    #[arg(long)]
    follow_changes: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new Ground in ./.bracken.
    Init {
        /// Local label for the Ground.
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a local replica of the Ground in a bundle, and import it.
    Join { bundle: PathBuf },
    /// Manage local agent identities.
    #[command(subcommand)]
    Id(IdCommand),
    /// Contribute a claim: an idea, hypothesis, requirement, observation or decision.
    Claim {
        statement: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// One of your own claims that this one replaces; records the supersedes relation too. Repeatable.
        #[arg(long, value_name = "ID")]
        supersedes: Vec<String>,
        #[command(flatten)]
        out: Output,
    },
    /// Contribute an artifact from a file.
    Artifact {
        file: PathBuf,
        /// Name to record. Defaults to the file path as given.
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
        /// The claim (or other contribution) this artifact implements: records the implements relation
        /// too, signed by the same identity.
        #[arg(long, value_name = "ID")]
        implements: Option<String>,
        #[command(flatten)]
        out: Output,
    },
    /// Relate two contributions, e.g. `bracken relate <a> supersedes <b>`.
    Relate {
        from: String,
        kind: RelationKind,
        to: String,
        #[arg(long, default_value = "")]
        note: String,
        #[command(flatten)]
        out: Output,
    },
    /// Record the outcome of checking a contribution: a verdict you give, or with --run, the result of
    /// running a command.
    ///
    /// Usage: `bracken evidence <TARGET> <VERDICT> --method <METHOD>`, where the target and verdict may
    /// also come after `--`; or `bracken evidence <TARGET> --run -- <COMMAND> [ARGS]`.
    Evidence {
        /// The contribution checked (required).
        target: Option<String>,
        /// pass or fail (required, except with --run, where the command's exit decides).
        #[arg(conflicts_with = "run")]
        verdict: Option<Verdict>,
        /// How it was checked, e.g. "cargo test" (required, except with --run, where it's the command line).
        #[arg(long, conflicts_with = "run")]
        method: Option<String>,
        #[arg(long, default_value = "", conflicts_with = "run")]
        details: String,
        /// Read the details from a regular file, or "-" for stdin, so they never appear on a command
        /// line. Process substitution (`<(...)`) is a pipe, not a regular file: pipe into "-" instead.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["details", "run"])]
        details_file: Option<PathBuf>,
        /// With a fail: what it needs that isn't accepted yet, recorded as a `depends_on` relation from
        /// the evidence to each. `status` shows the target blocked until all of them are accepted (or
        /// something that implements them is, by a relation its own author or theirs wrote), then
        /// ready. Only each validator's latest fail counts for this. Each must be a claim or an
        /// artifact. Ready work isn't accepted: the fail still counts until its author retracts it.
        /// Repeatable.
        #[arg(long = "depends-on", value_name = "ID", conflicts_with = "run")]
        depends_on: Vec<String>,
        /// Run the command given after `--` (no shell, in this directory, with no input), its output
        /// shown on stderr, and record pass if it exits 0, else fail. The method is the command line, and
        /// the details are its exit and the last 40 lines (at most 8000 bytes) of its output, which
        /// become permanent in the Ground: don't run a command whose output can hold secrets. Nothing is
        /// recorded if it can't start. Exits with the command's exit code.
        #[arg(long, requires = "command")]
        run: bool,
        /// After `--`: the command for --run, or without --run the target and verdict.
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
        #[command(flatten)]
        out: Output,
    },
    /// Withdraw one of your own contributions.
    Retract {
        target: String,
        #[arg(long, default_value = "")]
        reason: String,
        #[command(flatten)]
        out: Output,
    },
    /// List contributions in causal order.
    Log {
        #[arg(long)]
        kind: Option<BodyKind>,
        #[arg(long)]
        tag: Option<String>,
        /// Local name or public key.
        #[arg(long)]
        author: Option<String>,
        /// Only what is current: omit retracted contributions, claims and artifacts their author superseded
        /// (with a replacement anyone may have written), and relations, evidence and retractions about anything
        /// omitted. Another author's supersession doesn't count, and is shown as a marker instead.
        #[arg(long)]
        current: bool,
        /// Only what arrived since a cursor: contributions that are neither in it nor ancestors of any id in
        /// it. A cursor is comma-separated ids, the causal heads `log --since` printed last ("" for
        /// everything). Prints the next cursor after the list, or adds it to the JSON as "cursor". A cursor
        /// means the same only under the same filters: new work the filters hid is past it all the same.
        /// Or a date, `2026-10-29` (from its start, UTC) or `2026-10-29T18:27:00Z`: contributions stamped
        /// then or later. A timestamp is the author's clock, so this is a rough cut; a cursor is exact.
        #[arg(long, value_name = "CURSOR|DATE")]
        since: Option<String>,
        /// Only the most recent N, after every other filter. Says how many earlier ones it left out (in the
        /// JSON as "omitted"); the cursor is still the heads, so the next --since starts after all of them.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// One short line each, at most 200 characters (counted as chars, not display columns): the
        /// shortest unique id prefix (at least 8 digits), author, kind and tags, and the statement or name
        /// cut to one line. Control characters print as escapes such as \u{1b}, as in every log line.
        /// The default with --since, --tag or --open; `show <id>` prints an entry in full.
        #[arg(long, conflicts_with_all = ["json", "full"])]
        compact: bool,
        /// With --since, --tag or --open: full lines, as without them.
        #[arg(long, conflicts_with = "json")]
        full: bool,
        /// Only open findings: those `scripts/merge` would block a current head on (process 4.2). Any
        /// contribution, whatever its tags (finding `8c90edd63f5c`), not retracted, with a contradicts
        /// relation to something current and not yet merged (no live pass of the triager's on it whose
        /// method starts with `merge gate`, which `scripts/merge` takes back if the forge refuses)
        /// that still counts (live, or retracted by someone other than the finding's author and the triager), that
        /// the triager hasn't triaged (no live evidence of theirs on it) or has confirmed (a live pass of
        /// theirs, whatever else they or anyone gave) and not deferred (a depends_on of theirs from a
        /// current task of theirs tagged `deferred`). Anyone else's evidence doesn't triage.
        #[arg(long)]
        open: bool,
        /// With --open: the identity whose evidence triages findings.
        #[arg(long, value_name = "NAME", default_value = "manager", requires = "open")]
        triager: String,
        #[command(flatten)]
        out: Output,
    },
    /// Print the new contributions since a cursor, one compact line (or JSON object) each with the cursor
    /// past it, then that cursor; with --follow, keep printing them as they arrive.
    Watch(watch::WatchArgs),
    /// Show one contribution with its relations, evidence and status.
    Show {
        id: String,
        #[command(flatten)]
        policy: PolicyArgs,
        #[command(flatten)]
        out: Output,
    },
    /// Write an artifact's content to stdout.
    Cat { id: String },
    /// List causal heads.
    Heads {
        /// One short line each, as `log --compact`.
        #[arg(long, conflicts_with = "json")]
        compact: bool,
        #[command(flatten)]
        out: Output,
    },
    /// The merge gate's findings check for one claim: each finding with a contradicts link to it that still
    /// counts, and how the triager has dealt with it (untriaged, confirmed, deferred to a task, rejected).
    /// Untriaged and confirmed ones block the claim. The rules are `log --open`'s, for the claim itself, merged
    /// or not; scripts/merge reads this (task `dea48fd16754`).
    Findings {
        claim: String,
        /// The identity whose evidence triages findings.
        #[arg(long, value_name = "NAME", default_value = "manager")]
        triager: String,
        #[command(flatten)]
        out: Output,
    },
    /// List open conflicts among everything not retracted.
    Conflicts {
        #[command(flatten)]
        out: Output,
    },
    /// The accepted layer under a validation policy, and its conflicts.
    Accepted {
        #[command(flatten)]
        policy: PolicyArgs,
        #[command(flatten)]
        out: Output,
    },
    /// Say you're working on a contribution, so others don't start the same work: an intent claim
    /// (tagged `intent`, with its expiry) that refines it. It ends when it expires, when you retract
    /// it, or when you relate something that implements the contribution.
    Intend {
        id: String,
        /// How long the intent holds, e.g. 30m or 2h. Advisory, like every timestamp.
        #[arg(long = "for", value_name = "DURATION", default_value = "1h", value_parser = humantime::parse_duration)]
        duration: std::time::Duration,
        #[command(flatten)]
        out: Output,
    },
    /// The review queue: one line for each piece of work that implements a current claim, what
    /// needs an independent pass first (oldest first), then what is blocked on unaccepted work, and
    /// accepted work last. With --as (or BRACKEN_AGENT), personal: your own work, contradictions of
    /// your claims and the work waiting for your pass come first.
    Status {
        /// Also list the current claims that nothing implements, at the end.
        #[arg(long)]
        all: bool,
        /// Each claim with all its implementations, pass counts and live intents, unbounded.
        #[arg(long, conflicts_with = "json")]
        full: bool,
        /// The most lines the queue prints; it says how many more there are.
        #[arg(long, value_name = "N", default_value_t = 40)]
        limit: usize,
        #[command(flatten)]
        policy: PolicyArgs,
        #[command(flatten)]
        out: Output,
    },
    /// Exchange contributions with another replica of the same Ground: a local directory, a server
    /// at an http:// URL on this machine, or a server reached through a program (--exec).
    Sync {
        /// A directory containing .bracken, a replica directory itself, or http://127.0.0.1:<port>.
        #[arg(required_unless_present = "exec", conflicts_with = "exec")]
        other: Option<String>,
        /// Run this program (no shell) and sync over its stdin and stdout, e.g. `--exec ssh host
        /// bracken --dir <path> serve --stdio`. Takes the rest of the command line.
        #[arg(long, num_args = 1.., allow_hyphen_values = true, value_name = "PROGRAM")]
        exec: Option<Vec<String>>,
        /// For an http:// server: read the bearer token from this file. Defaults to $BRACKEN_TOKEN.
        #[arg(long, value_name = "PATH")]
        token_file: Option<PathBuf>,
        /// With a server: after the sync, open a second session and fail unless the server's heads
        /// then cover everything this replica held when the first began.
        #[arg(long)]
        confirm: bool,
        #[command(flatten)]
        bounds: BoundsArgs,
    },
    /// Serve this replica to sync sessions: over HTTP on 127.0.0.1, or one session over stdio.
    Serve {
        /// Serve one session on stdin and stdout (for SSH), instead of HTTP.
        #[arg(long)]
        stdio: bool,
        /// HTTP port on 127.0.0.1; 0 picks a free one. The URL is printed on stdout.
        #[arg(long, default_value_t = 0, conflicts_with = "stdio")]
        port: u16,
        /// Author keys whose contributions this server stores, one hex key per line. Without it, the
        /// server stores nothing (pull only).
        #[arg(long, value_name = "PATH", requires = "first_seen")]
        allow: Option<PathBuf>,
        /// HTTP credentials: lines of `<name> <blake3 hex of the token>`. Required for HTTP.
        #[arg(long, value_name = "PATH", conflicts_with = "stdio")]
        tokens: Option<PathBuf>,
        /// Append-only log of each contribution stored for the first time, as JSON lines.
        #[arg(long, value_name = "PATH")]
        first_seen: Option<PathBuf>,
        /// Contributions each author key, and each credential, may add in 24 hours.
        #[arg(long, default_value_t = 10_000)]
        quota_contributions: u64,
        /// Bytes each author key, and each credential, may add in 24 hours.
        #[arg(long, default_value_t = 256 << 20)]
        quota_bytes: u64,
        /// The most bytes of contributions the replica may hold.
        #[arg(long, default_value_t = 4 << 30)]
        store_cap: u64,
        /// Over HTTP, the longest wait between a session's requests, in seconds.
        #[arg(long, default_value_t = 300)]
        idle: u64,
        /// Sessions open at once, over HTTP.
        #[arg(long, default_value_t = 16)]
        max_sessions: usize,
        #[command(flatten)]
        bounds: BoundsArgs,
    },
    /// Serve a read-only web view of this replica on 127.0.0.1, with its JSON API, until stopped.
    View {
        /// Port; 0 picks a free one. The URL is printed on stdout.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// An address to listen on, instead of 127.0.0.1, e.g. this machine's NetBird IP. Repeatable: at most
        /// one loopback and one other address, all at --port, in one process. Anyone who can reach a
        /// non-loopback address can read the whole Ground: there's no login. Never every interface,
        /// multicast or broadcast.
        #[arg(long, value_name = "IP", default_value = "127.0.0.1")]
        bind: Vec<std::net::IpAddr>,
        /// A name this view also answers to, as the Host a browser sends (e.g. warren-mbp.jx.rs), on the
        /// non-loopback listener (or the only one). Repeatable. Every other Host is refused.
        #[arg(long = "host", value_name = "NAME")]
        hosts: Vec<String>,
        /// An experiment run to show on the Runs page, read once and never written: one mode's run
        /// directory, or a directory of mode runs to compare. Each needs the report.json that
        /// `bracken-experiment report --json` writes. Served on loopback only.
        #[arg(long, value_name = "DIR")]
        runs: Option<PathBuf>,
        /// The policy every status in the view is under.
        #[command(flatten)]
        policy: PolicyArgs,
    },
    /// Bracken's source trees on Weave: import from git, manifests, lines and their heads, integrate, checkout,
    /// diff and verify.
    #[command(subcommand)]
    Weave(weave::WeaveCommand),
    /// Write every contribution to a bundle file ("-" for stdout).
    Export { file: PathBuf },
    /// Import contributions from a bundle file ("-" for stdin).
    Import { file: PathBuf },
}

/// What one sync session may take from its peer.
#[derive(Args)]
struct BoundsArgs {
    /// Seconds from the session's start until it ends with an error.
    #[arg(long, default_value_t = 600)]
    deadline: u64,
    /// Bytes the peer may send in one session, framing and id lists included.
    #[arg(long, default_value_t = 1 << 30)]
    budget: u64,
    /// Bytes of those that may be id lists (heads, offer, need).
    #[arg(long, default_value_t = 64 << 20)]
    id_budget: u64,
    /// The largest batch this side accepts, and sends. Under 16 MiB, the largest contributions
    /// can't be exchanged.
    #[arg(long, default_value_t = 64 << 20, value_parser = clap::value_parser!(u64).range(44..))]
    batch_bytes: u64,
}

impl BoundsArgs {
    fn bounds(&self) -> wire::Bounds {
        let deadline = std::time::Duration::from_secs(self.deadline);
        wire::Bounds { deadline, budget: self.budget, id_budget: self.id_budget, batch_bytes: self.batch_bytes }
    }
}

#[derive(Subcommand)]
enum IdCommand {
    /// Create a new identity.
    New { name: String },
    /// List known agents, marking those this replica can sign as.
    List {
        #[command(flatten)]
        out: Output,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bracken: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Init { name } => {
            let dir = cli.dir.unwrap_or_else(|| PathBuf::from(DIR_NAME));
            let name = name.unwrap_or_else(|| default_name(&dir));
            let s = Store::init(&dir, &name)?;
            println!("created ground {} ({name}) in {}", s.info().ground, dir.display());
        }
        Command::Join { bundle: path } => {
            let dir = cli.dir.unwrap_or_else(|| PathBuf::from(DIR_NAME));
            let (ground, contributions) = read_bundle(&path)?;
            let mut s = Store::create(&dir, GroundInfo { ground, name: default_name(&dir) })?;
            let report = s.import(contributions)?;
            let code = decide(report.is_clean());
            print_report(&report);
            println!("joined ground {ground} in {}", dir.display());
            return Ok(code);
        }
        Command::Weave(weave::WeaveCommand::Init { purpose, out }) => {
            return weave::init(cli.dir.as_deref(), cli.agent.as_deref(), purpose, out);
        }
        command => return run_in_store(open_store(cli.dir.as_deref())?, cli.agent.as_deref(), command),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_in_store(mut s: Store, agent: Option<&str>, command: Command) -> Result<ExitCode> {
    match command {
        Command::Init { .. } | Command::Join { .. } => unreachable!("handled before opening a store"),
        Command::Weave(command) => return weave::run(&mut s, agent, command),
        Command::Watch(args) => return watch::run(s, agent, args),
        Command::Id(IdCommand::New { name }) => {
            let id = s.create_identity(&name)?;
            println!("{}", id.agent_id());
        }
        Command::Id(IdCommand::List { out }) => {
            let local: BTreeSet<AgentId> = s.identities()?.iter().map(|i| i.agent_id()).collect();
            let agents = s.agents()?;
            if out.json {
                let rows: Vec<Value> = agents
                    .iter()
                    .map(|a| json!({"name": a.name, "agent": a.agent, "can_sign": local.contains(&a.agent)}))
                    .collect();
                print_json(&rows);
            } else {
                for a in agents {
                    let key = if local.contains(&a.agent) { "" } else { "  (no local key)" };
                    println!("{}  {}{key}", a.agent, terminal::printable(&a.name));
                }
            }
        }
        Command::Claim { statement, tags, supersedes, out } => {
            let who = signer(&s, agent)?;
            let names = Names::load(&s)?;
            let old = supersedes.iter().map(|id| own_claim(&s, &names, &who, id)).collect::<Result<BTreeSet<_>>>()?;
            let mut w = Writes::new(out.json);
            let new = w.add(&mut s, &who, Body::Claim { statement, tags })?;
            for to in old {
                let note = String::new();
                let relation = Body::Relation { from: new, kind: RelationKind::Supersedes, to, note };
                if let Err(e) = w.add(&mut s, &who, relation) {
                    w.finish_failed();
                    return Err(e.context(format!("claim {new} was recorded, but not all its supersedes relations")));
                }
            }
            w.finish();
        }
        Command::Artifact { file, name, media_type, implements, out } => {
            let who = signer(&s, agent)?;
            // Resolved first, so an unknown id writes nothing.
            let target = implements.as_deref().map(|id| resolve(&s, id)).transpose()?;
            let content = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let name = name.unwrap_or_else(|| file.display().to_string());
            let mut w = Writes::new(out.json);
            let artifact = w.add(&mut s, &who, Body::Artifact { name, media_type, content })?;
            if let Some(to) = target {
                let relation =
                    Body::Relation { from: artifact, kind: RelationKind::Implements, to, note: String::new() };
                match w.add(&mut s, &who, relation) {
                    Ok(_) => {}
                    // The artifact stays: the Ground is append-only. Its id is printed.
                    Err(e) => {
                        w.finish_failed();
                        eprintln!(
                            "bracken: artifact {artifact} was recorded, but its implements relation to {to} couldn't be: \
                             {e:#}; relate it with `bracken relate {} implements {}`",
                            artifact.short(),
                            to.short()
                        );
                        return Ok(decide(false));
                    }
                }
            }
            w.finish();
        }
        Command::Relate { from, kind, to, note, out } => {
            let who = signer(&s, agent)?;
            let (from, to) = (resolve(&s, &from)?, resolve(&s, &to)?);
            let mut w = Writes::new(out.json);
            w.add(&mut s, &who, Body::Relation { from, kind, to, note })?;
            w.finish();
        }
        Command::Evidence { target, verdict, method, details, details_file, run, command, depends_on, out } => {
            // Everything after `--` is one list to clap. With --run it's the command. Without, it's the
            // target and verdict, so values that look like options can't be read as options there.
            let (target, verdict, command) = if run {
                ensure!(!command.is_empty(), "--run needs a command after --, e.g. --run -- cargo test");
                (target.context("which contribution? give its id before --run")?, None, command)
            } else {
                let mut words = command.into_iter();
                let target = target.or_else(|| words.next()).context("which contribution? give its id")?;
                let verdict = match verdict {
                    Some(v) => v,
                    None => words
                        .next()
                        .context("pass or fail? give a verdict, or --run with a command")?
                        .parse::<Verdict>()
                        .map_err(|e| anyhow::anyhow!("the verdict is pass or fail: {e}"))?,
                };
                ensure!(words.next().is_none(), "a command after -- runs only with --run; nothing recorded");
                (target, Some(verdict), Vec::new())
            };
            if !depends_on.is_empty() && verdict != Some(Verdict::Fail) {
                bail!("--depends-on goes with a fail: it says what the fail needs that isn't accepted yet");
            }
            let who = signer(&s, agent)?;
            let target = resolve(&s, &target)?;
            if run {
                // The signer and target are settled first, so a run is never wasted on a refusal after.
                let outcome = verify::run(&command)?;
                let body = Body::Evidence {
                    target,
                    verdict: outcome.verdict,
                    method: outcome.method,
                    details: outcome.details,
                };
                let mut w = Writes::new(out.json);
                w.add(&mut s, &who, body)?;
                BROKEN_PIPE_STATUS.store(i32::from(outcome.status), Ordering::Relaxed);
                w.finish();
                return Ok(ExitCode::from(outcome.status));
            }
            let details = match details_file {
                Some(path) => read_details(&path)?,
                None => details,
            };
            let verdict = verdict.expect("set above without --run");
            let method = method.context("how was it checked? give --method, or use --run")?;
            // Every id resolves before anything is written, so a bad one writes nothing.
            let mut needs: Vec<ContributionId> = depends_on.iter().map(|d| resolve(&s, d)).collect::<Result<_>>()?;
            needs.sort();
            needs.dedup();
            for n in &needs {
                let subject = s.graph().get(n).is_some_and(|c| c.body().is_subject());
                ensure!(
                    subject,
                    "--depends-on {}: only a claim or an artifact can be accepted; nothing recorded",
                    n.short()
                );
            }
            let mut w = Writes::new(out.json);
            let evidence = w.add(&mut s, &who, Body::Evidence { target, verdict, method, details })?;
            for (i, &to) in needs.iter().enumerate() {
                let body = Body::Relation { from: evidence, kind: RelationKind::DependsOn, to, note: String::new() };
                match w.add(&mut s, &who, body) {
                    Ok(_) => {}
                    Err(e) => {
                        w.finish_failed();
                        let left: Vec<String> = needs[i..].iter().map(ToString::to_string).collect();
                        bail!(
                            "evidence {evidence} recorded, but depends_on to {} not written ({e:#}): relate them by \
                             hand, `relate {evidence} depends_on <id>`",
                            left.join(", ")
                        );
                    }
                }
            }
            w.finish();
        }
        Command::Retract { target, reason, out } => {
            let who = signer(&s, agent)?;
            let target = resolve(&s, &target)?;
            let mut w = Writes::new(out.json);
            w.add(&mut s, &who, Body::Retract { target, reason })?;
            w.finish();
        }
        Command::Log { kind, tag, author, current, since, limit, compact, full, open, triager, out } => {
            ensure!(
                !full || since.is_some() || tag.is_some() || open,
                "--full only undoes the compact form of --since, --tag or --open"
            );
            let compact = compact || ((since.is_some() || tag.is_some() || open) && !full);
            let names = Names::load(&s)?;
            let author = author.map(|a| names.resolve(&a)).transpose()?;
            let g = s.graph();
            let since_filter = since.as_deref().map(|c| listing::Since::parse(g, c)).transpose()?;
            let order = g.topo_order();
            let view = (current || open).then(|| CurrentView::new(g, &order));
            let triager = if open {
                Some(names.resolve(&triager).with_context(|| format!("--open: who triages? no identity {triager}"))?)
            } else {
                None
            };
            let rows: Vec<&Contribution> = order
                .iter()
                // --open alone keeps a finding its author superseded, as scripts/merge does: it hides only
                // the retracted.
                .filter(|id| !current || view.as_ref().is_none_or(|v| !v.hidden.contains(id)))
                .map(|id| g.get(id).expect("ordered ids exist"))
                .filter(|c| since_filter.as_ref().is_none_or(|s| s.admits(c)))
                .filter(|c| {
                    triager.is_none_or(|t| open_finding(g, view.as_ref().expect("--open builds the view"), t, c))
                })
                .filter(|c| kind.is_none_or(|k| c.body().kind() == k))
                .filter(|c| author.is_none_or(|a| c.author() == a))
                .filter(|c| {
                    tag.as_ref().is_none_or(|t| matches!(c.body(), Body::Claim { tags, .. } if tags.contains(t)))
                })
                .collect();
            let omitted = limit.map_or(0, |n| rows.len().saturating_sub(n));
            let rows = &rows[omitted..];
            // What's new since a cursor, personal to the caller: what concerns them first.
            let (me, note) = if since.is_some() { caller(&names, agent) } else { (None, None) };
            let for_you_ids: Vec<ContributionId> =
                me.map(|me| rows.iter().filter(|c| concerns(g, me, c)).map(|c| c.id()).collect()).unwrap_or_default();
            let for_you: HashSet<ContributionId> = for_you_ids.iter().copied().collect();
            if out.json {
                let rows: Vec<Value> = rows
                    .iter()
                    .map(|c| {
                        let mut v = contribution_json(g, &names, c);
                        if let Some(view) = &view {
                            v["superseded_by_others"] = view
                                .by_others(&c.id())
                                .iter()
                                .map(|(s, a)| json!({"id": s, "author": a, "author_name": names.0.get(a)}))
                                .collect();
                        }
                        v
                    })
                    .collect();
                if since.is_some() || limit.is_some() {
                    let mut v = json!({"contributions": rows, "cursor": listing::cursor(g), "omitted": omitted});
                    if me.is_some() {
                        v["for_you"] = json!(for_you_ids);
                    }
                    print_json(&v);
                } else {
                    print_json(&rows);
                }
            } else {
                let prefixes =
                    compact.then(|| listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS));
                if let Some(note) = note {
                    println!("{note}");
                }
                let (first, then): (Vec<&Contribution>, Vec<&Contribution>) =
                    rows.iter().partition(|c| for_you.contains(&c.id()));
                let headed = !first.is_empty();
                if headed {
                    println!("for you:");
                }
                for (n, &c) in first.iter().chain(&then).enumerate() {
                    if headed && n == first.len() && !then.is_empty() {
                        println!("everything else:");
                    }
                    let mut text = match &prefixes {
                        Some(p) => compact_line(g, &names, p, c),
                        None => line(g, &names, c),
                    };
                    let by_others = view.as_ref().map_or(&[][..], |v| v.by_others(&c.id()));
                    if compact && !by_others.is_empty() {
                        // The text gives way to the marks, so they're never what's cut.
                        let marks = compact_marks(&names, by_others);
                        text = listing::cap_to(text, listing::COMPACT_LINE_CHARS - marks.chars().count());
                        text.push_str(&marks);
                    } else {
                        for (by, author) in by_others {
                            text.push_str(&superseded_by_suffix(&names, by, author));
                        }
                    }
                    println!("{text}");
                }
                if omitted > 0 {
                    println!("omitted: {omitted} earlier (--limit)");
                }
                if since.is_some() {
                    println!("cursor: {}", listing::cursor(g));
                }
            }
        }
        Command::Findings { claim, triager, out } => {
            let names = Names::load(&s)?;
            let triager_key = names.resolve(&triager).with_context(|| format!("who triages? no identity {triager}"))?;
            let g = s.graph();
            let claim = resolve(&s, &claim)?;
            let view = CurrentView::new(g, &g.topo_order());
            let found = findings_against(g, &view, triager_key, &claim);
            let blocking = found.iter().filter(|(_, t)| t.blocks()).count();
            if out.json {
                let findings: Vec<Value> = found
                    .iter()
                    .map(|(f, t)| {
                        let task = if let Triage::Deferred(task) = t { Some(*task) } else { None };
                        json!({"id": f, "triage": t.name(), "task": task, "blocks": t.blocks()})
                    })
                    .collect();
                print_json(&json!({
                    "claim": claim,
                    "triager": triager_key,
                    "triager_name": names.0.get(&triager_key),
                    "findings": findings,
                    "blocking": blocking,
                }));
            } else {
                let prefixes = listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS);
                for (f, t) in &found {
                    let task =
                        if let Triage::Deferred(task) = t { format!(" -> {}", prefixes[task]) } else { String::new() };
                    let mark = if t.blocks() { "blocks" } else { "ok" };
                    println!("{} {mark} {}{task}", prefixes[f], t.name());
                }
                println!("{} against {}, {blocking} blocking", found.len(), prefixes[&claim]);
            }
        }
        Command::Show { id, policy, out } => {
            let names = Names::load(&s)?;
            let policy = policy.build(&names)?;
            let g = s.graph();
            let c = g.get(&resolve(&s, &id)?).expect("resolved ids exist");
            if out.json {
                let view = CurrentView::new(g, &g.topo_order());
                print_json(&show_json(g, &names, &policy, &view, c, now_ms()));
            } else {
                let assessment = policy.assess(g, &c.id());
                let referrers: Vec<&Contribution> = g.referrers(&c.id()).collect();
                let intents = live_intents(g, &CurrentView::new(g, &g.topo_order()), &c.id(), now_ms());
                print_show(g, &names, c, assessment.as_ref(), &referrers);
                if !intents.is_empty() {
                    println!();
                    println!("live intents:");
                    for i in &intents {
                        println!("{}", terminal::printable(&intent_line(&names, i, now_ms())));
                    }
                }
            }
        }
        Command::Cat { id } => {
            let id = resolve(&s, &id)?;
            let Body::Artifact { content, .. } = s.graph().get(&id).expect("resolved ids exist").body() else {
                bail!("{id} is not an artifact");
            };
            write_stdout(content);
        }
        Command::Heads { compact, out } => {
            let names = Names::load(&s)?;
            let g = s.graph();
            let heads: Vec<&Contribution> = g.heads().map(|id| g.get(&id).expect("heads exist")).collect();
            if out.json {
                print_json(&heads.iter().map(|c| contribution_json(g, &names, c)).collect::<Vec<_>>());
            } else {
                let prefixes =
                    compact.then(|| listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS));
                for c in heads {
                    match &prefixes {
                        Some(p) => println!("{}", compact_line(g, &names, p, c)),
                        None => println!("{}", line(g, &names, c)),
                    }
                }
            }
        }
        Command::Conflicts { out } => {
            let names = Names::load(&s)?;
            print_conflicts(s.graph(), &names, &open_conflicts(s.graph()), out.json);
        }
        Command::Accepted { policy, out } => {
            let names = Names::load(&s)?;
            let policy = policy.build(&names)?;
            let g = s.graph();
            if out.json {
                print_json(&accepted_json(g, &names, &policy));
            } else {
                let e = policy.evaluate(g);
                if e.accepted.is_empty() {
                    println!("nothing is accepted under this policy");
                }
                for a in &e.accepted {
                    println!("{}", line(g, &names, g.get(&a.id).expect("assessed ids exist")));
                }
                if !e.conflicts.is_empty() {
                    println!();
                    print_conflicts(g, &names, &e.conflicts, false);
                }
            }
        }
        Command::Status { all, full, limit, policy, out } => {
            print_status(&s, &policy, &StatusArgs { all, full, limit, json: out.json, agent })?
        }
        Command::View { port, bind, hosts, runs, policy } => {
            // Validators by name from the public agent directory: the view never reads identities.
            let policy = policy.build(&view::public_names(&s)?)?;
            view::serve(s.dir(), policy, &view::Bind { ips: bind, port, names: hosts }, runs.as_deref())?;
        }
        Command::Intend { id, duration, out } => {
            let who = signer(&s, agent)?;
            let target = resolve(&s, &id)?;
            // RFC 3339 years have four digits, so the last expiry it can write is 9999-12-31T23:59:59Z.
            let last = std::time::UNIX_EPOCH + std::time::Duration::from_secs(253_402_300_799);
            let until =
                std::time::SystemTime::now().checked_add(duration).filter(|t| *t <= last).with_context(|| {
                    format!("--for {} would end after the year 9999", humantime::format_duration(duration))
                })?;
            let until = humantime::format_rfc3339_seconds(until).to_string();
            let statement = format!("intent: working on {} until {until}", target.short());
            let tags = vec![INTENT_TAG.to_owned(), format!("{UNTIL_TAG}{until}")];
            let mut w = Writes::new(out.json);
            let intent = w.add(&mut s, &who, Body::Claim { statement, tags })?;
            let relation =
                Body::Relation { from: intent, kind: RelationKind::Refines, to: target, note: String::new() };
            match w.add(&mut s, &who, relation) {
                Ok(_) => w.finish(),
                // The claim stays: the Ground is append-only. Its id is printed.
                Err(e) => {
                    w.finish_failed();
                    eprintln!(
                        "bracken: intent {intent} was recorded, but its refines relation to {target} couldn't be: \
                         {e:#}; relate it with `bracken relate {} refines {}`",
                        intent.short(),
                        target.short()
                    );
                    return Ok(decide(false));
                }
            }
        }
        Command::Sync { exec: Some(program), confirm, bounds, .. } => {
            return sync_remote(&mut s, confirm, |s| remote::sync_exec(s, &program, bounds.bounds()));
        }
        Command::Sync { other: Some(url), token_file, confirm, bounds, .. } if url.contains("://") => {
            let token = match token_file {
                Some(path) => std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
                None => std::env::var("BRACKEN_TOKEN").context("set BRACKEN_TOKEN or pass --token-file")?,
            };
            return sync_remote(&mut s, confirm, |s| remote::sync_http(s, &url, token.trim(), bounds.bounds()));
        }
        Command::Sync { confirm: true, .. } => bail!("--confirm is for a server: an http:// URL or --exec"),
        Command::Sync { other, .. } => {
            let other = PathBuf::from(other.expect("clap requires a target without --exec"));
            let other_dir = if other.join(DIR_NAME).is_dir() { other.join(DIR_NAME) } else { other };
            let mut o = Store::open(&other_dir)?;
            let (here, there) = store::sync(&mut s, &mut o)?;
            let code = decide(here.is_clean() && there.is_clean());
            print_report(&here);
            print_report(&there);
            println!("received {}, sent {}", here.added.len(), there.added.len());
            return Ok(code);
        }
        Command::Serve {
            stdio,
            port,
            allow,
            tokens,
            first_seen,
            quota_contributions,
            quota_bytes,
            store_cap,
            idle,
            max_sessions,
            bounds,
        } => {
            let tokens = match (&tokens, stdio) {
                (Some(path), false) => Some(serve::Tokens::read(path)?),
                (None, false) => bail!("serving over HTTP needs --tokens"),
                (_, true) => None,
            };
            let policy = serve::Policy {
                allow: allow.as_deref().map(serve::read_allowlist).transpose()?,
                quota_contributions,
                quota_bytes,
                store_cap,
                bounds: bounds.bounds(),
                idle: std::time::Duration::from_secs(idle),
                max_sessions,
            };
            let intake = serve::Intake::new(s, policy, first_seen.as_deref())?;
            match tokens {
                Some(tokens) => serve::serve_http(intake, tokens, port)?,
                None => serve::serve_stdio(intake)?,
            }
        }
        Command::Export { file } => {
            let all = s.graph().missing_from(&Graph::new(s.info().ground));
            let n = if file.as_os_str() == "-" {
                bundle::write(BufWriter::new(io::stdout().lock()), s.info().ground, all)?
            } else {
                let f = File::create(&file).with_context(|| format!("creating {}", file.display()))?;
                bundle::write(BufWriter::new(f), s.info().ground, all)?
            };
            eprintln!("exported {n} contributions");
        }
        Command::Import { file } => {
            let (ground, contributions) = read_bundle(&file)?;
            if ground != s.info().ground {
                bail!("bundle is for ground {ground}, this replica is {}", s.info().ground);
            }
            let report = s.import(contributions)?;
            let code = decide(report.is_clean());
            print_report(&report);
            println!("imported {}, already had {}", report.added.len(), report.already_present);
            return Ok(code);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn default_name(dir: &Path) -> String {
    let abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
    let project = if abs.file_name().is_some_and(|n| n == DIR_NAME) { abs.parent() } else { Some(abs.as_path()) };
    project.and_then(|p| p.file_name()).map_or_else(|| "ground".into(), |n| n.to_string_lossy().into_owned())
}

fn open_store(dir: Option<&Path>) -> Result<Store> {
    if let Some(dir) = dir {
        return Ok(Store::open(dir)?);
    }
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(DIR_NAME);
        if candidate.join("ground.json").is_file() {
            return Ok(Store::open(&candidate)?);
        }
    }
    bail!("not in a Bracken workspace (no {DIR_NAME} here or in any parent); run `bracken init`")
}

fn read_bundle(path: &Path) -> Result<(GroundId, Vec<Contribution>)> {
    Ok(if path.as_os_str() == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        bundle::read(buf.as_slice())?
    } else {
        let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        bundle::read(BufReader::new(f))?
    })
}

/// Evidence details from a regular file, or stdin for "-". The type is checked on the opened
/// file, and reading stops past the size limit.
fn read_details(path: &Path) -> Result<String> {
    let limit = contribution::MAX_CONTRIBUTION_BYTES as u64;
    let mut bytes = Vec::new();
    if path.as_os_str() == "-" {
        io::stdin().take(limit + 1).read_to_end(&mut bytes).context("reading stdin")?;
    } else {
        let not_regular =
            || anyhow::anyhow!("{} isn't a regular file (for a pipe, use --details-file -)", path.display());
        // Checked before opening too, because opening a FIFO blocks until something writes to it.
        if !std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.is_file() {
            return Err(not_regular());
        }
        let f = File::open(path).with_context(|| format!("reading {}", path.display()))?;
        if !f.metadata().with_context(|| format!("reading {}", path.display()))?.is_file() {
            return Err(not_regular());
        }
        f.take(limit + 1).read_to_end(&mut bytes).with_context(|| format!("reading {}", path.display()))?;
    }
    if bytes.len() as u64 > limit {
        bail!("{} is over the {limit}-byte contribution limit", path.display());
    }
    String::from_utf8(bytes).with_context(|| format!("{} isn't UTF-8 text", path.display()))
}

/// What one write command recorded, the primary record first (task `d4e958fd6719`): one labelled line
/// each, `claim <id>` or `relation <id> (supersedes)`, or with --json `{"id": <primary>, "written": [{"id",
/// "kind", "relation"?}]}`. Printed only once every write is done, as a reader that stops early must not
/// stop one half-way, and on a failure part way, for what was written before it.
struct Writes {
    json: bool,
    written: Vec<Value>,
}

impl Writes {
    fn new(json: bool) -> Writes {
        Writes { json, written: Vec::new() }
    }

    fn add(&mut self, s: &mut Store, who: &Identity, body: Body) -> Result<ContributionId> {
        let kind = body.kind().as_str();
        let relation = match &body {
            Body::Relation { kind, .. } => Some(kind.as_str()),
            _ => None,
        };
        let id = contribute(s, who, body)?;
        let mut row = json!({"id": id, "kind": kind});
        if let Some(r) = relation {
            row["relation"] = json!(r);
        }
        self.written.push(row);
        Ok(id)
    }

    /// After a failure part way: decides the failing exit first, as `decide` does, so a reader that
    /// stops early can't turn it into success (finding `58def77d1d97`), then prints what was written.
    fn finish_failed(&self) {
        decide(false);
        self.finish();
    }

    /// Prints whatever was written.
    fn finish(&self) {
        if self.json {
            if !self.written.is_empty() {
                print_json(&json!({"id": self.written[0]["id"], "written": self.written}));
            }
            return;
        }
        for row in &self.written {
            let (kind, id) = (row["kind"].as_str().unwrap_or_default(), row["id"].as_str().unwrap_or_default());
            match row["relation"].as_str() {
                Some(r) => println!("{kind} {id} ({r})"),
                None => println!("{kind} {id}"),
            }
        }
    }
}

/// Signs and adds a contribution, unless it holds the private key of an identity in this replica.
/// The Ground is permanent and goes into git, so this catches the likely accidents: a key file
/// attached as details or as an artifact, or a key pasted into text. A key re-encoded (base64)
/// gets through.
fn contribute(s: &mut Store, who: &Identity, body: Body) -> Result<ContributionId> {
    let fields: Vec<&[u8]> = match &body {
        Body::Claim { statement, tags } => {
            std::iter::once(statement.as_bytes()).chain(tags.iter().map(|t| t.as_bytes())).collect()
        }
        Body::Artifact { name, media_type, content } => vec![name.as_bytes(), media_type.as_bytes(), content],
        Body::Relation { note, .. } => vec![note.as_bytes()],
        Body::Evidence { method, details, .. } => vec![method.as_bytes(), details.as_bytes()],
        Body::Retract { reason, .. } => vec![reason.as_bytes()],
    };
    for (name, hex, raw) in local_secret_keys(s)? {
        let found = fields.iter().any(|f| contains(&f.to_ascii_lowercase(), hex.as_bytes()) || contains(f, &raw));
        if found {
            bail!("this contribution contains the private key of identity {name:?}; it can't go into the Ground");
        }
    }
    Ok(s.contribute(who, body)?)
}

/// Each local identity's name and secret key, as lowercase hex and as bytes. Read from the
/// identity files themselves, like scripts/ground-checkpoint does.
fn local_secret_keys(s: &Store) -> Result<Vec<(String, String, Vec<u8>)>> {
    let dir = s.identities_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut keys = Vec::new();
    for entry in entries {
        let path = entry.with_context(|| format!("reading {}", dir.display()))?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let file: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let Some(hex) = file["secret_key"].as_str() else { continue };
        let hex = hex.to_ascii_lowercase();
        let raw = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        let name = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        keys.push((name, hex, raw));
    }
    Ok(keys)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn resolve(s: &Store, prefix: &str) -> Result<ContributionId> {
    Ok(s.graph().resolve_prefix(prefix)?)
}

fn signer(s: &Store, agent: Option<&str>) -> Result<Identity> {
    let name = agent.context("this command signs a contribution: pass --as <NAME> or set BRACKEN_AGENT")?;
    Ok(s.identity(name)?)
}

/// Resolves a `--supersedes` target, which must be a claim `who` wrote. Superseding someone
/// else's claim is refused: they may not see it as replaced, and the old claim's evidence and
/// relations don't carry over.
fn own_claim(s: &Store, names: &Names, who: &Identity, prefix: &str) -> Result<ContributionId> {
    let id = resolve(s, prefix)?;
    let c = s.graph().get(&id).expect("resolved ids exist");
    if !matches!(c.body(), Body::Claim { .. }) {
        bail!("--supersedes {}: it is a {}, not a claim", id.short(), c.body().kind());
    }
    if c.author() != who.agent_id() {
        bail!(
            "--supersedes {}: it is {}'s claim; you can only supersede your own (contradict or refine it instead)",
            id.short(),
            names.display(&c.author())
        );
    }
    Ok(id)
}

/// Where one piece of work stands in the review queue: the policy's status, with a rejection
/// whose fails all name what they need (`depends_on` relations from the fail evidence, by its
/// author) told apart as blocked, or ready once all of that is accepted. Under `--follow-changes`,
/// accepted work whose dependencies changed after its passes needs a recheck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Waiting,
    Recheck,
    Ready,
    Blocked,
    Rejected,
    Superseded,
    Retracted,
    Accepted,
}

impl Standing {
    fn label(self) -> &'static str {
        match self {
            Standing::Waiting => "waiting",
            Standing::Recheck => "recheck",
            Standing::Ready => "ready",
            Standing::Blocked => "blocked",
            Standing::Rejected => "rejected",
            Standing::Superseded => "superseded",
            Standing::Retracted => "retracted",
            Standing::Accepted => "accepted",
        }
    }

    /// Queue order: what needs an independent pass first, accepted work last.
    fn rank(self) -> u8 {
        match self {
            Standing::Waiting | Standing::Recheck | Standing::Ready => 0,
            Standing::Blocked => 1,
            Standing::Rejected => 2,
            Standing::Superseded => 3,
            Standing::Retracted => 4,
            Standing::Accepted => 5,
        }
    }

    fn needs_a_pass(self) -> bool {
        self.rank() == 0
    }
}

/// What a fail evidence says it needs: the targets of current `depends_on` relations from it that
/// its own author wrote.
fn fail_needs(g: &Graph, view: &CurrentView, evidence: &ContributionId) -> Vec<ContributionId> {
    let Some(e) = g.get(evidence) else { return Vec::new() };
    let mut out: Vec<ContributionId> = g
        .referrers(evidence)
        .filter(|r| r.author() == e.author() && !view.hidden.contains(&r.id()))
        .filter_map(|r| match r.body() {
            Body::Relation { from, kind: RelationKind::DependsOn, to, .. } if from == evidence => Some(*to),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Standings for one `status` call. Each contribution's acceptance and each need's are worked out once
/// and kept, so the whole call stays linear in the replica, whatever relations anyone writes.
struct Judge<'a> {
    g: &'a Graph,
    view: &'a CurrentView,
    assessor: Assessor<'a>,
    /// Each contribution's place in causal order.
    position: HashMap<ContributionId, usize>,
    accepted: HashMap<ContributionId, bool>,
    met: HashMap<ContributionId, bool>,
    /// Assessments made: each contribution's at most once.
    assessed: usize,
}

impl<'a> Judge<'a> {
    fn new(g: &'a Graph, view: &'a CurrentView, policy: &'a Policy, order: &[ContributionId]) -> Self {
        let position = order.iter().enumerate().map(|(n, id)| (*id, n)).collect();
        let assessor = policy.assessor(g);
        Judge { g, view, assessor, position, accepted: HashMap::new(), met: HashMap::new(), assessed: 0 }
    }

    fn accepted(&mut self, id: &ContributionId) -> bool {
        if let Some(&known) = self.accepted.get(id) {
            return known;
        }
        self.assessed += 1;
        let yes = self.assessor.assess(id).is_some_and(|a| a.status == Status::Accepted);
        self.accepted.insert(*id, yes);
        yes
    }

    /// Whether what a fail needs is there: the policy accepts it, or accepts something that currently
    /// implements it by a relation its source's author or the need's author wrote, as the accepted
    /// count takes implements (a stranger's relation doesn't make someone else's work ready). That is
    /// the shared authorship rule with this feature's set, [`Trusted::SOURCE_OR_TARGET`].
    fn met(&mut self, id: &ContributionId) -> bool {
        if let Some(&known) = self.met.get(id) {
            return known;
        }
        let (g, view) = (self.g, self.view);
        let mut sources: Vec<ContributionId> = g
            .referrers(id)
            .filter(|r| !view.hidden.contains(&r.id()) && relation_counts(g, r, &Trusted::SOURCE_OR_TARGET))
            .filter_map(|r| match r.body() {
                Body::Relation { from, kind: RelationKind::Implements, to, .. } if to == id => Some(*from),
                _ => None,
            })
            .collect();
        sources.sort();
        sources.dedup();
        let yes = self.accepted(id) || sources.iter().any(|f| self.accepted(f));
        self.met.insert(*id, yes);
        yes
    }

    /// Each author's latest of `fails` in causal order (bracken's deterministic order among concurrent
    /// ones). An earlier fail by the same validator says what it needed then.
    fn latest_fails(&self, fails: &[ContributionId]) -> Vec<ContributionId> {
        let mut latest: HashMap<AgentId, (usize, ContributionId)> = HashMap::new();
        for f in fails.iter().filter_map(|f| self.g.get(f)) {
            let at = self.position.get(&f.id()).copied().unwrap_or(0);
            let best = latest.entry(f.author()).or_insert((at, f.id()));
            if at > best.0 {
                *best = (at, f.id());
            }
        }
        latest.into_values().map(|(_, id)| id).collect()
    }

    /// `a`'s standing, and what keeps it blocked. Its counted fails are the policy's; whether each
    /// validator's latest fail names only what is now accepted decides blocked or ready. A ready work's
    /// fails still count, so it stays ready, not accepted, until their authors retract them.
    fn standing(&mut self, a: &Assessment) -> (Standing, Vec<ContributionId>) {
        let plain = match a.status {
            Status::Pending => Standing::Waiting,
            Status::NeedsRecheck => Standing::Recheck,
            Status::Accepted => Standing::Accepted,
            Status::Superseded => Standing::Superseded,
            Status::Retracted => Standing::Retracted,
            Status::Rejected => {
                let needs: Vec<Vec<ContributionId>> =
                    self.latest_fails(&a.fails).iter().map(|f| fail_needs(self.g, self.view, f)).collect();
                if needs.iter().any(Vec::is_empty) {
                    Standing::Rejected
                } else {
                    let mut unmet: Vec<ContributionId> = needs.into_iter().flatten().collect();
                    unmet.sort();
                    unmet.dedup();
                    unmet.retain(|n| !self.met(n));
                    return if unmet.is_empty() { (Standing::Ready, unmet) } else { (Standing::Blocked, unmet) };
                }
            }
        };
        (plain, Vec::new())
    }
}

/// One implementation of one claim, as `status` lists it.
struct Work<'a> {
    claim: &'a Contribution,
    work: &'a Contribution,
    assessment: Assessment,
    standing: Standing,
    blocked_on: Vec<ContributionId>,
}

/// Each claim `status` lists, with what implements it and its live intents.
type StatusRow<'a> = (&'a Contribution, Vec<Work<'a>>, Vec<Intent<'a>>);

/// What `status` lists: each current claim that something implements (through a current `implements`
/// relation from it, whoever wrote the relation, whatever kind of contribution it is) or has a live
/// intent on, with its implementations in queue order; claims with something needing a pass first,
/// otherwise in causal order; and (with `all`) the current claims with neither.
fn status_rows<'a>(g: &'a Graph, policy: &Policy, all: bool, now: u64) -> (Vec<StatusRow<'a>>, Vec<&'a Contribution>) {
    let order = g.topo_order();
    let view = CurrentView::new(g, &order);
    let mut judge = Judge::new(g, &view, policy, &order);
    let mut implemented: Vec<StatusRow> = Vec::new();
    let mut unimplemented: Vec<&Contribution> = Vec::new();
    for id in &order {
        let c = g.get(id).expect("ordered ids exist");
        // Intents are about other claims, not subjects of their own.
        let is_claim = matches!(c.body(), Body::Claim { .. }) && !is_intent(g, &view, c);
        if view.hidden.contains(id) || !is_claim {
            continue;
        }
        let mut by: Vec<ContributionId> = g
            .referrers(id)
            .filter(|r| !view.hidden.contains(&r.id()))
            .filter_map(|r| match r.body() {
                Body::Relation { from, kind: RelationKind::Implements, to, .. } if to == id => Some(*from),
                _ => None,
            })
            .filter(|from| !view.hidden.contains(from))
            .collect();
        by.sort();
        by.dedup();
        let mut rows: Vec<Work> = by
            .iter()
            .filter_map(|i| {
                let assessment = judge.assessor.assess(i)?;
                let (standing, blocked_on) = judge.standing(&assessment);
                Some(Work { claim: c, work: g.get(i).expect("related ids exist"), assessment, standing, blocked_on })
            })
            .collect();
        let intents = live_intents(g, &view, id, now);
        if rows.is_empty() && intents.is_empty() {
            if all {
                unimplemented.push(c);
            }
            continue;
        }
        rows.sort_by_key(|w| (w.standing.rank(), w.work.content().timestamp_ms));
        implemented.push((c, rows, intents));
    }
    // Claims with something needing a pass first; otherwise in causal order (the sort is stable).
    implemented.sort_by_key(|(_, rows, _)| !rows.iter().any(|w| w.standing.needs_a_pass()));
    (implemented, unimplemented)
}

/// Whether work with this status needs a pass: its first, or one after a change it depends on.
fn needs_pass(status: Status) -> bool {
    matches!(status, Status::Pending | Status::NeedsRecheck)
}

/// Distinct validators with a counted pass, as the policy counts them: one validator passing twice is one.
fn independent_passes(g: &Graph, a: &Assessment) -> usize {
    a.passes.iter().filter_map(|e| g.get(e)).map(|e| e.author()).collect::<HashSet<_>>().len()
}

/// The review queue, personal to `me` if given: `me`'s own work, contradictions of `me`'s claims, the
/// others' work waiting for `me`'s pass, then everything else; each part in queue order (what needs
/// an independent pass first, oldest first, accepted work last).
struct Queue<'a> {
    yours: Vec<&'a Work<'a>>,
    contradictions: Vec<&'a Contribution>,
    for_your_pass: Vec<&'a Work<'a>>,
    rest: Vec<&'a Work<'a>>,
}

fn queue<'a>(g: &'a Graph, rows: &'a [StatusRow<'a>], me: Option<AgentId>) -> Queue<'a> {
    let mut all: Vec<&Work> = rows.iter().flat_map(|(_, w, _)| w).collect();
    all.sort_by_key(|w| (w.standing.rank(), w.work.content().timestamp_ms, w.work.id()));
    let Some(me) = me else {
        return Queue { yours: Vec::new(), contradictions: Vec::new(), for_your_pass: Vec::new(), rest: all };
    };
    // A pass from before a change doesn't speak to it, so recheck work waits for anyone's pass, its earlier
    // validators' included.
    let passed_by_me = |w: &Work| {
        w.standing != Standing::Recheck
            && w.assessment.passes.iter().any(|e| g.get(e).is_some_and(|e| e.author() == me))
    };
    let (yours, others): (Vec<&Work>, Vec<&Work>) = all.into_iter().partition(|w| w.work.author() == me);
    let (for_your_pass, rest) = others.into_iter().partition(|w| w.standing.needs_a_pass() && !passed_by_me(w));
    let order = g.topo_order();
    let view = CurrentView::new(g, &order);
    let contradictions = order
        .iter()
        .filter(|id| !view.hidden.contains(id))
        .map(|id| g.get(id).expect("ordered ids exist"))
        .filter(|r| {
            r.author() != me
                && matches!(r.body(), Body::Relation { kind: RelationKind::Contradicts, to, .. }
                    if g.get(to).is_some_and(|t| t.author() == me && matches!(t.body(), Body::Claim { .. })))
        })
        .collect();
    Queue { yours, contradictions, for_your_pass, rest }
}

/// Whether `c` concerns `me`, for `log --since`: a verdict on `me`'s work, a contradiction of `me`'s
/// claim, or someone else's work toward a claim (through an `implements` relation), which waits for an
/// independent pass such as `me`'s.
fn concerns(g: &Graph, me: AgentId, c: &Contribution) -> bool {
    let mine = |id: &ContributionId| g.get(id).is_some_and(|t| t.author() == me);
    if c.author() == me {
        return false;
    }
    match c.body() {
        Body::Evidence { target, .. } => mine(target),
        Body::Relation { kind: RelationKind::Contradicts, to, .. } => mine(to),
        Body::Relation { from, kind: RelationKind::Implements, .. } => !mine(from),
        Body::Claim { .. } | Body::Artifact { .. } => g.referrers(&c.id()).any(
            |r| matches!(r.body(), Body::Relation { from, kind: RelationKind::Implements, .. } if *from == c.id()),
        ),
        _ => false,
    }
}

/// The caller `status` and `log --since` are personal to: the `--as` identity, by local name or
/// public key. A name this replica doesn't know is said, and the view isn't personal.
fn caller(names: &Names, agent: Option<&str>) -> (Option<AgentId>, Option<String>) {
    match agent.map(|a| (a, names.resolve(a))) {
        None => (None, None),
        Some((_, Ok(id))) => (Some(id), None),
        Some((a, Err(_))) => (None, Some(format!("not personal: {} is unknown here", terminal::printable(a)))),
    }
}

fn work_json(g: &Graph, names: &Names, w: &Work, now: u64) -> Value {
    let mut v = contribution_json(g, names, w.work);
    v["assessment"] = json!(w.assessment);
    v["waiting"] = json!(needs_pass(w.assessment.status));
    v["standing"] = json!(w.standing.label());
    v["blocked_on"] = json!(w.blocked_on);
    v["independent_passes"] = json!(independent_passes(g, &w.assessment));
    v["age_ms"] = json!(now.saturating_sub(w.work.content().timestamp_ms));
    v
}

/// `status --json`: every claim with its implementations, and the queue as id pairs, personal to `me`.
fn status_json(g: &Graph, names: &Names, policy: &Policy, all: bool, me: Option<AgentId>, now: u64) -> Value {
    let (implemented, unimplemented) = status_rows(g, policy, all, now);
    let claims: Vec<Value> = implemented
        .iter()
        .map(|(c, rows, intents)| {
            let by: Vec<Value> = rows.iter().map(|w| work_json(g, names, w, now)).collect();
            let intents: Vec<Value> = intents.iter().map(|i| intent_json(g, names, i, now)).collect();
            json!({"claim": contribution_json(g, names, c), "implemented_by": by, "intents": intents})
        })
        .collect();
    let q = queue(g, &implemented, me);
    let pairs = |ws: &[&Work]| -> Vec<Value> {
        ws.iter().map(|w| json!({"claim": w.claim.id(), "work": w.work.id(), "standing": w.standing.label()})).collect()
    };
    let unimplemented: Vec<Value> = unimplemented.iter().map(|c| contribution_json(g, names, c)).collect();
    let mut v = json!({
        "min_passes": policy.min_passes,
        "claims": claims,
        "queue": pairs(&q.rest),
        "unimplemented": unimplemented,
    });
    if let Some(me) = me {
        v["caller"] = json!(me);
        v["yours"] = json!(pairs(&q.yours));
        v["contradictions_of_yours"] = q.contradictions.iter().map(|r| contribution_json(g, names, r)).collect();
        v["for_your_pass"] = json!(pairs(&q.for_your_pass));
    }
    v
}

/// What `status` prints, and how much of it.
struct StatusArgs<'a> {
    all: bool,
    full: bool,
    limit: usize,
    json: bool,
    agent: Option<&'a str>,
}

fn print_status(s: &Store, policy: &PolicyArgs, a: &StatusArgs) -> Result<()> {
    let names = Names::load(s)?;
    let policy = policy.build(&names)?;
    let g = s.graph();
    let now = now_ms();
    let (me, note) = caller(&names, a.agent);
    if a.json {
        print_json(&status_json(g, &names, &policy, a.all, me, now));
        return Ok(());
    }
    if let Some(note) = note {
        println!("{note}");
    }
    if a.full {
        return print_full_status(g, &names, &policy, a.all, now);
    }
    let (implemented, unimplemented) = status_rows(g, &policy, a.all, now);
    if implemented.is_empty() {
        println!("no current claim is implemented by anything, or has a live intent");
    }
    let prefixes = listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS);
    let q = queue(g, &implemented, me);
    let mut left = a.limit;
    let mut more = 0;
    let mut part = |title: Option<&str>, lines: Vec<String>| {
        if lines.is_empty() {
            return;
        }
        if let Some(t) = title.filter(|_| left > 0) {
            println!("{t}");
        }
        for l in lines {
            if left == 0 {
                more += 1;
                continue;
            }
            left -= 1;
            println!("{l}");
        }
    };
    let rows = |ws: &[&Work]| ws.iter().map(|w| queue_line(g, &names, &prefixes, w, now)).collect::<Vec<_>>();
    if me.is_some() {
        part(Some("yours:"), rows(&q.yours));
        let lines = q.contradictions.iter().map(|r| compact_line(g, &names, &prefixes, r)).collect();
        part(Some("contradicting your claims:"), lines);
        part(Some("waiting for your pass:"), rows(&q.for_your_pass));
        part(Some("everything else:"), rows(&q.rest));
    } else {
        part(None, rows(&q.rest));
    }
    let intents: Vec<String> = implemented
        .iter()
        .flat_map(|(c, _, intents)| intents.iter().map(move |i| (c, i)))
        .map(|(c, i)| {
            let line = format!(
                "intent {} {} until {} ({} left) for {} {}",
                prefixes[&i.claim.id()],
                names.display(&i.claim.author()),
                i.until,
                age(i.until_ms.saturating_sub(now)),
                prefixes[&c.id()],
                listing::compact_text(c.body(), summary(g, c)),
            );
            listing::cap_line(terminal::printable(&line))
        })
        .collect();
    part(Some("live intents:"), intents);
    if a.all {
        let idle = unimplemented.iter().map(|c| compact_line(g, &names, &prefixes, c)).collect();
        part(Some("nothing implements, or intends to:"), idle);
    }
    if more > 0 {
        println!("{more} more: status --limit <N>, or status --full");
    }
    Ok(())
}

/// One piece of work on one short line: standing, id, author, age, what it is, the claim it implements,
/// and what keeps it blocked.
fn queue_line(g: &Graph, names: &Names, prefixes: &HashMap<ContributionId, String>, w: &Work, now: u64) -> String {
    let blocked = if w.blocked_on.is_empty() {
        String::new()
    } else {
        let ids: Vec<&str> = w.blocked_on.iter().map(|i| prefixes.get(i).map_or("?", String::as_str)).collect();
        format!(" (needs {})", ids.join(","))
    };
    let line = format!(
        "{} {} {} {} ago {}{blocked} for {} {}",
        w.standing.label(),
        prefixes[&w.work.id()],
        names.display(&w.work.author()),
        age(now.saturating_sub(w.work.content().timestamp_ms)),
        listing::compact_text(w.work.body(), summary(g, w.work)),
        prefixes[&w.claim.id()],
        listing::compact_text(w.claim.body(), summary(g, w.claim)),
    );
    listing::cap_line(terminal::printable(&line))
}

/// `status --full`: each claim with every implementation and intent, one row each.
fn print_full_status(g: &Graph, names: &Names, policy: &Policy, all: bool, now: u64) -> Result<()> {
    let (implemented, unimplemented) = status_rows(g, policy, all, now);
    let age_ms = |c: &Contribution| now.saturating_sub(c.content().timestamp_ms);
    if implemented.is_empty() {
        println!("no current claim is implemented by anything, or has a live intent");
    }
    for (n, (c, rows, intents)) in implemented.iter().enumerate() {
        if n > 0 {
            println!();
        }
        // Other agents' text is printed escaped, so it can't forge lines (finding 252f052a283d).
        println!("{}", terminal::printable(&line(g, names, c)));
        for i in intents {
            println!("{}", terminal::printable(&intent_line(names, i, now)));
        }
        for w in rows {
            let blocked = if w.blocked_on.is_empty() {
                String::new()
            } else {
                let ids: Vec<String> = w.blocked_on.iter().map(ContributionId::short).collect();
                format!("  needs {}", ids.join(", "))
            };
            let row = format!(
                "  {:<10}  {}  {:<8}  {:<12}  {:>4} ago  {}/{} passes  {}{blocked}",
                w.standing.label(),
                w.work.id().short(),
                w.work.body().kind(),
                names.display(&w.work.author()),
                age(age_ms(w.work)),
                independent_passes(g, &w.assessment),
                policy.min_passes,
                summary(g, w.work)
            );
            println!("{}", terminal::printable(&row));
        }
    }
    if !unimplemented.is_empty() {
        println!();
        println!("nothing implements, or intends to:");
        for c in &unimplemented {
            println!("{}", terminal::printable(&line(g, names, c)));
        }
    }
    Ok(())
}

/// Milliseconds since the Unix epoch, now.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The tag that makes a claim an intent, and the prefix of the tag holding its expiry.
const INTENT_TAG: &str = "intent";
const UNTIL_TAG: &str = "until:";

/// A live intent: a current claim tagged `intent`, with a tag `until:<RFC 3339 time>` still in the
/// future, related to its target by a current `refines` relation its own author wrote, and not
/// ended by delivery: no `implements` relation to the target by the same author that isn't causally
/// before the intent and that its author hasn't retracted. A convention on ordinary contributions;
/// the time is advisory.
struct Intent<'a> {
    claim: &'a Contribution,
    until: String,
    until_ms: u64,
}

/// The live intents on `target`, oldest first.
fn live_intents<'a>(g: &'a Graph, view: &CurrentView, target: &ContributionId, now: u64) -> Vec<Intent<'a>> {
    let relations = |kind: RelationKind| {
        g.referrers(target).filter_map(move |r| match r.body() {
            Body::Relation { from, kind: k, to, .. } if *k == kind && to == target => Some((r, *from)),
            _ => None,
        })
    };
    let mut intents: Vec<Intent> = relations(RelationKind::Refines)
        .filter(|(r, _)| !view.hidden.contains(&r.id()))
        .filter_map(|(r, from)| {
            // Current already: a relation that refers to a hidden (retracted, superseded) claim is hidden too.
            let claim = g.get(&from)?;
            if r.author() != claim.author() {
                return None;
            }
            let (until, until_ms) = intent_until(claim)?;
            // Only the delivery relation's own retraction undoes it, not what happens to the
            // contribution it relates, which someone else may have written.
            let delivered = relations(RelationKind::Implements).any(|(d, _)| {
                d.author() == claim.author() && !g.is_retracted(&d.id()) && !g.is_ancestor(&d.id(), &claim.id())
            });
            (until_ms > now && !delivered).then(|| Intent { claim, until: until.to_owned(), until_ms })
        })
        .collect();
    intents.sort_by_key(|i| (i.claim.content().timestamp_ms, i.claim.id()));
    intents.dedup_by_key(|i| i.claim.id());
    intents
}

/// An intent claim's expiry, as written and in milliseconds since the Unix epoch: `None` unless `c`
/// is a claim tagged `intent` with a parseable `until:` tag.
fn intent_until(c: &Contribution) -> Option<(&str, u64)> {
    let Body::Claim { tags, .. } = c.body() else { return None };
    if !tags.iter().any(|t| t == INTENT_TAG) {
        return None;
    }
    let until = tags.iter().find_map(|t| t.strip_prefix(UNTIL_TAG))?;
    let at = humantime::parse_rfc3339(until).ok()?;
    Some((until, at.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64))
}

/// Whether `c` is an intent, live or not: it has an expiry, and its own author relates it to
/// something with a current `refines` relation. Any other claim tagged `intent` is an ordinary claim.
fn is_intent(g: &Graph, view: &CurrentView, c: &Contribution) -> bool {
    intent_until(c).is_some()
        && g.referrers(&c.id()).any(|r| {
            r.author() == c.author()
                && !view.hidden.contains(&r.id())
                && matches!(r.body(), Body::Relation { from, kind: RelationKind::Refines, .. } if *from == c.id())
        })
}

fn intent_json(g: &Graph, names: &Names, i: &Intent, now: u64) -> Value {
    let mut v = contribution_json(g, names, i.claim);
    v["until"] = json!(i.until);
    v["until_ms"] = json!(i.until_ms);
    v["age_ms"] = json!(now.saturating_sub(i.claim.content().timestamp_ms));
    v
}

/// A live intent's row: its id, author, age and expiry.
fn intent_line(names: &Names, i: &Intent, now: u64) -> String {
    format!(
        "  {:<10}  {}  {:<8}  {:<12}  {:>4} ago  until {} ({} left)",
        "intent",
        i.claim.id().short(),
        "claim",
        names.display(&i.claim.author()),
        age(now.saturating_sub(i.claim.content().timestamp_ms)),
        i.until,
        age(i.until_ms.saturating_sub(now))
    )
}

/// A duration in milliseconds, in its largest whole unit: 45s, 12m, 3h, 2d.
fn age(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// Whether `id` was merged: the triager's live pass on it with a method starting `merge gate`, which
/// `scripts/merge` records on the pr claim it merges, and retracts if the forge refuses the merge
/// (finding `d494f36dee9d`). A merged PR's last pr claim stays current, so a finding against it is
/// closed for `log --open` (task `3217747ec88b`).
fn merged(g: &Graph, triager: AgentId, id: &ContributionId) -> bool {
    g.referrers(id).any(|r| {
        r.author() == triager
            && !g.is_retracted(&r.id())
            && matches!(r.body(), Body::Evidence { target, verdict: Verdict::Pass, method, .. }
                if target == id && method.starts_with("merge gate"))
    })
}

/// How the triager has dealt with a finding (process 4.2). Only the triager's live evidence triages: none
/// leaves it untriaged; a pass confirms it, whatever else they or anyone gave; anything else rejects it. A
/// confirmed finding is deferred by a live depends_on of the triager's to it, from a task of theirs that
/// `view` shows, tagged `deferred` (the first such, in referrer order). Anyone else's evidence or deferral
/// counts for nothing. This and [`link_counts`] are the one definition of the merge gate's findings check:
/// `log --open` and `findings` both read them, and scripts/merge reads `findings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Triage {
    Untriaged,
    Confirmed,
    Deferred(ContributionId),
    Rejected,
}

impl Triage {
    /// Whether it blocks a head: untriaged, or confirmed and not deferred.
    fn blocks(self) -> bool {
        matches!(self, Triage::Untriaged | Triage::Confirmed)
    }

    fn name(self) -> &'static str {
        match self {
            Triage::Untriaged => "untriaged",
            Triage::Confirmed => "confirmed",
            Triage::Deferred(_) => "deferred",
            Triage::Rejected => "rejected",
        }
    }
}

fn triage(g: &Graph, view: &CurrentView, triager: AgentId, finding: &ContributionId) -> Triage {
    let (mut triaged, mut confirmed, mut deferred) = (false, false, None);
    for r in g.referrers(finding) {
        if g.is_retracted(&r.id()) || r.author() != triager {
            continue;
        }
        match r.body() {
            Body::Evidence { target, verdict, .. } if target == finding => {
                triaged = true;
                confirmed |= *verdict == Verdict::Pass;
            }
            Body::Relation { from, kind: RelationKind::DependsOn, to, .. } if to == finding && deferred.is_none() => {
                let defers = g.get(from).is_some_and(|t| {
                    t.author() == triager
                        && !view.hidden.contains(from)
                        && matches!(t.body(), Body::Claim { tags, .. } if tags.iter().any(|x| x == "task")
                            && tags.iter().any(|x| x == "deferred"))
                });
                if defers {
                    deferred = Some(*from);
                }
            }
            _ => {}
        }
    }
    match (triaged, confirmed, deferred) {
        (false, ..) => Triage::Untriaged,
        (true, true, Some(task)) => Triage::Deferred(task),
        (true, true, None) => Triage::Confirmed,
        (true, false, _) => Triage::Rejected,
    }
}

/// Whether a contradicts relation from `finding` still counts: live, or retracted by someone other than the
/// finding's author and `triager` (a retract is always its relation's author's), so no one else can drop a
/// finding by retracting their own link to it (finding `7c38339206c5`).
fn link_counts(g: &Graph, triager: AgentId, finding: &Contribution, link: &Contribution) -> bool {
    !g.is_retracted(&link.id()) || (link.author() != finding.author() && link.author() != triager)
}

/// A finding `log --open` lists, whatever its kind or tags (finding `8c90edd63f5c`): one the merge gate would
/// block a current head on. Not retracted; with a contradicts relation from it that [`link_counts`], to a
/// contribution `view` shows and the triager hasn't merged; and blocking by its [`triage`].
fn open_finding(g: &Graph, view: &CurrentView, triager: AgentId, c: &Contribution) -> bool {
    if g.is_retracted(&c.id()) {
        return false;
    }
    let against_current = g.referrers(&c.id()).any(|r| {
        matches!(r.body(), Body::Relation { from, kind: RelationKind::Contradicts, to, .. }
            if *from == c.id() && g.get(to).is_some() && !view.hidden.contains(to) && !merged(g, triager, to))
            && link_counts(g, triager, c, r)
    });
    against_current && triage(g, view, triager, &c.id()).blocks()
}

/// The merge gate's findings check for one claim (scripts/merge, section 3): every contribution with a
/// contradicts relation to it, in id order, that isn't retracted and has a link that [`link_counts`], with its
/// [`triage`]. Unlike `log --open`, it looks at the claim itself, current or not, merged or not.
fn findings_against(
    g: &Graph,
    view: &CurrentView,
    triager: AgentId,
    claim: &ContributionId,
) -> Vec<(ContributionId, Triage)> {
    let links: Vec<&Contribution> = g
        .referrers(claim)
        .filter(|r| matches!(r.body(), Body::Relation { kind: RelationKind::Contradicts, to, .. } if to == claim))
        .collect();
    let mut from: Vec<ContributionId> = links
        .iter()
        .filter_map(|r| match r.body() {
            Body::Relation { from, .. } => Some(*from),
            _ => None,
        })
        .collect();
    from.sort();
    from.dedup();
    from.into_iter()
        .filter(|f| !g.is_retracted(f))
        .filter(|f| {
            let finding = g.get(f).expect("a relation's ends exist");
            links.iter().any(|r| {
                matches!(r.body(), Body::Relation { from, .. } if from == f) && link_counts(g, triager, finding, r)
            })
        })
        .map(|f| (f, triage(g, view, triager, &f)))
        .collect()
}

/// What `log --current` shows.
struct CurrentView {
    /// Omitted: retracted contributions; claims and artifacts with a supersession that counts under
    /// docs/protocol.md (written by their own author, with a live newer side, whoever wrote it); and
    /// anything that refers to an omitted contribution.
    hidden: HashSet<ContributionId>,
    /// Shown but marked: live subjects that other authors claim to supersede. Those supersessions
    /// don't count, so someone else can't hide your claim, or the relations and evidence about it.
    /// Each entry is the claimed successor and the author of the supersedes relation.
    others: HashMap<ContributionId, Vec<(ContributionId, AgentId)>>,
}

impl CurrentView {
    /// `order` is causal, so references are decided before their referrers.
    fn new(g: &Graph, order: &[ContributionId]) -> CurrentView {
        let mut view = CurrentView { hidden: HashSet::new(), others: HashMap::new() };
        for id in order {
            let c = g.get(id).expect("ordered ids exist");
            let superseded = c.body().is_subject() && !open_supersessors(g, id).is_empty();
            if g.is_retracted(id) || superseded || c.body().references().iter().any(|r| view.hidden.contains(r)) {
                view.hidden.insert(*id);
                continue;
            }
            let others: Vec<(ContributionId, AgentId)> = g
                .referrers(id)
                .filter(|r| r.author() != c.author() && !g.is_retracted(&r.id()))
                .filter_map(|r| match r.body() {
                    Body::Relation { from, kind: RelationKind::Supersedes, to, .. }
                        if to == id && g.get(from).is_some_and(|f| f.body().is_subject()) && !g.is_retracted(from) =>
                    {
                        Some((*from, r.author()))
                    }
                    _ => None,
                })
                .collect();
            if !others.is_empty() {
                view.others.insert(*id, others);
            }
        }
        view
    }

    fn by_others(&self, id: &ContributionId) -> &[(ContributionId, AgentId)] {
        self.others.get(id).map_or(&[], Vec::as_slice)
    }
}

/// Syncs with a server and reports the session: what each side refused, and what each received.
/// Fails if either side refused anything or was left with contributions missing ancestors.
///
/// With `confirm`, a clean session is followed by a second one, and the command fails unless the
/// heads the server sends at the start of that one cover every head this replica held when the
/// first began: the server's own word that it holds everything this replica had.
fn sync_remote(
    s: &mut Store,
    confirm: bool,
    mut session: impl FnMut(&mut Store) -> Result<wire::Side>,
) -> Result<ExitCode> {
    let held: Vec<ContributionId> = s.graph().heads().collect();
    let side = session(s)?;
    let clean = report_session(&side);
    println!("received {}, sent {}", side.report.added.len(), side.peer.added);
    if !confirm || !clean {
        return Ok(decide(clean));
    }
    let check = session(s)?;
    if !report_session(&check) {
        return Ok(decide(false));
    }
    let g = s.graph();
    let covered = |h: &ContributionId| check.peer_heads.iter().any(|p| p == h || g.is_ancestor(h, p));
    let missing = held.iter().filter(|h| !covered(h)).count();
    if missing > 0 {
        eprintln!("not confirmed: the server's heads don't cover {missing} of this replica's {} heads", held.len());
        return Ok(decide(false));
    }
    println!("confirmed: the server holds all {} heads", held.len());
    Ok(ExitCode::SUCCESS)
}

/// Prints what a session refused or left unresolved, on either side; true if nothing was.
fn report_session(side: &wire::Side) -> bool {
    let peer = &side.peer;
    print_report(&side.report);
    for r in &peer.rejected {
        eprintln!("the server refused {}: {}", r.id.short(), r.reason);
    }
    if peer.unresolved > 0 {
        eprintln!("the server is missing ancestors of {} contributions", peer.unresolved);
    }
    side.report.is_clean() && peer.rejected.is_empty() && peer.unresolved == 0
}

fn print_report(report: &ImportReport) {
    for (id, why) in &report.rejected {
        eprintln!("rejected {}: {why}", id.short());
    }
    if !report.unresolved.is_empty() {
        eprintln!("{} contributions are missing ancestors", report.unresolved.len());
    }
}

/// The exit status [`write_stdout`] uses if stdout's reader goes away: 0 unless the command has
/// already decided to fail.
static BROKEN_PIPE_STATUS: AtomicI32 = AtomicI32::new(0);

/// Decides a command's exit status before it prints, so a reader that stops early can't turn a
/// failure into success.
fn decide(clean: bool) -> ExitCode {
    if clean {
        return ExitCode::SUCCESS;
    }
    BROKEN_PIPE_STATUS.store(1, Ordering::Relaxed);
    ExitCode::FAILURE
}

impl PolicyArgs {
    fn build(&self, names: &Names) -> Result<Policy> {
        let validators = if self.validators.is_empty() {
            None
        } else {
            Some(self.validators.iter().map(|v| names.resolve(v)).collect::<Result<BTreeSet<_>>>()?)
        };
        Ok(Policy { min_passes: self.min_passes, validators, follow_changes: self.follow_changes })
    }
}

/// `log --current`'s mark on a line that another author claims to supersede. The rest of the line is
/// escaped already; the name is escaped here, since an agent entry's name isn't checked when it's read
/// (security evidence cb1a56001700).
fn superseded_by_suffix(names: &Names, by: &ContributionId, author: &AgentId) -> String {
    format!("  (superseded by {} {})", terminal::printable(&names.display(author)), by.short())
}

/// The compact form's marks, within `listing::COMPACT_MARKS_CHARS`: each `(superseded by <author> <id>)`
/// with the author's name cut to `listing::COMPACT_MARK_NAME_CHARS`, so a long name can't push out its
/// id or the marks after it (finding `69725885c7fb`), and `(+N more)` for those that don't fit. The
/// first always does.
fn compact_marks(names: &Names, by_others: &[(ContributionId, AgentId)]) -> String {
    let mut out = String::new();
    for (i, (by, author)) in by_others.iter().enumerate() {
        let name = listing::cap_to(terminal::printable(&names.display(author)), listing::COMPACT_MARK_NAME_CHARS);
        let mark = format!("  (superseded by {name} {})", by.short());
        let rest = by_others.len() - i;
        let room = if rest > 1 { format!("  (+{} more)", rest - 1).chars().count() } else { 0 };
        if i > 0 && out.chars().count() + mark.chars().count() + room > listing::COMPACT_MARKS_CHARS {
            out.push_str(&format!("  (+{rest} more)"));
            break;
        }
        out.push_str(&mark);
    }
    out
}

/// Local names for agents. Agents without a local identity show by key.
struct Names(HashMap<AgentId, String>);

impl Names {
    fn load(s: &Store) -> Result<Names> {
        Ok(Names(s.agents()?.into_iter().map(|a| (a.agent, a.name)).collect()))
    }

    fn display(&self, agent: &AgentId) -> String {
        self.0.get(agent).cloned().unwrap_or_else(|| format!("~{}", &agent.short()[..8]))
    }

    fn resolve(&self, name_or_key: &str) -> Result<AgentId> {
        if let Some((id, _)) = self.0.iter().find(|(_, n)| n.as_str() == name_or_key) {
            return Ok(*id);
        }
        name_or_key.parse().with_context(|| format!("{name_or_key:?} is neither a local identity nor a public key"))
    }
}

fn summary(g: &Graph, c: &Contribution) -> String {
    let short = |id: &ContributionId| id.short();
    match c.body() {
        Body::Claim { statement, tags } if tags.is_empty() => statement.clone(),
        Body::Claim { statement, tags } => format!("{statement}  [{}]", tags.join(", ")),
        Body::Artifact { name, content, .. } => format!("{name} ({} bytes)", content.len()),
        Body::Relation { from, kind, to, note } if note.is_empty() => format!("{} {kind} {}", short(from), short(to)),
        Body::Relation { from, kind, to, note } => format!("{} {kind} {}: {note}", short(from), short(to)),
        Body::Evidence { target, verdict, method, .. } => format!("{verdict} {} via {method}", short(target)),
        Body::Retract { target, reason } => {
            let what =
                g.get(target).map_or_else(|| short(target), |t| format!("{} ({})", short(target), t.body().kind()));
            if reason.is_empty() { what } else { format!("{what}: {reason}") }
        }
    }
}

fn line(g: &Graph, names: &Names, c: &Contribution) -> String {
    let retracted = if g.is_retracted(&c.id()) { "  (retracted)" } else { "" };
    let line = format!(
        "{}  {:<8}  {:<12}  {}{retracted}",
        c.id().short(),
        c.body().kind(),
        names.display(&c.author()),
        summary(g, c)
    );
    terminal::printable(&line)
}

/// A contribution on one short line: its shortest unique prefix, author, kind and tags, and its text.
fn compact_line(g: &Graph, names: &Names, prefixes: &HashMap<ContributionId, String>, c: &Contribution) -> String {
    let tags = match c.body() {
        Body::Claim { tags, .. } if !tags.is_empty() => format!(" [{}]", tags.join(",")),
        _ => String::new(),
    };
    let retracted = if g.is_retracted(&c.id()) { " (retracted)" } else { "" };
    let text = listing::compact_text(c.body(), summary(g, c));
    let line =
        format!("{} {} {}{tags}{retracted} {text}", prefixes[&c.id()], names.display(&c.author()), c.body().kind());
    listing::cap_line(terminal::printable(&line))
}

fn body_json(body: &Body) -> Value {
    match body {
        Body::Claim { statement, tags } => json!({"statement": statement, "tags": tags}),
        Body::Artifact { name, media_type, content } => json!({
            "name": name,
            "media_type": media_type,
            "size": content.len(),
            "text": std::str::from_utf8(content).ok(),
        }),
        Body::Relation { from, kind, to, note } => json!({"from": from, "kind": kind, "to": to, "note": note}),
        Body::Evidence { target, verdict, method, details } => {
            json!({"target": target, "verdict": verdict, "method": method, "details": details})
        }
        Body::Retract { target, reason } => json!({"target": target, "reason": reason}),
    }
}

fn contribution_json(g: &Graph, names: &Names, c: &Contribution) -> Value {
    json!({
        "id": c.id(),
        "kind": c.body().kind(),
        "author": c.author(),
        "author_name": names.0.get(&c.author()),
        "parents": c.parents(),
        "timestamp_ms": c.content().timestamp_ms,
        "retracted": g.is_retracted(&c.id()),
        "body": body_json(c.body()),
    })
}

/// `show --json`: the contribution, its assessment, everything referring to it, and its live intents.
/// `view` is the replica's current view, which a caller answering many requests works out once.
fn show_json(g: &Graph, names: &Names, policy: &Policy, view: &CurrentView, c: &Contribution, now: u64) -> Value {
    let mut v = contribution_json(g, names, c);
    v["assessment"] = json!(policy.assess(g, &c.id()));
    v["referrers"] = g.referrers(&c.id()).map(|r| contribution_json(g, names, r)).collect();
    let intents = live_intents(g, view, &c.id(), now);
    v["intents"] = intents.iter().map(|i| intent_json(g, names, i, now)).collect();
    v
}

/// `accepted --json`: the accepted layer under `policy`, and its conflicts.
fn accepted_json(g: &Graph, names: &Names, policy: &Policy) -> Value {
    let e = policy.evaluate(g);
    let accepted: Vec<Value> = e
        .accepted
        .iter()
        .map(|a| {
            let mut v = contribution_json(g, names, g.get(&a.id).expect("assessed ids exist"));
            v["assessment"] = json!(a);
            v
        })
        .collect();
    let conflicts: Vec<Value> = e.conflicts.iter().map(|c| json!(c)).collect();
    json!({"accepted": accepted, "conflicts": conflicts})
}

fn print_show(
    g: &Graph,
    names: &Names,
    c: &Contribution,
    assessment: Option<&Assessment>,
    referrers: &[&Contribution],
) {
    println!("id        {}", c.id());
    println!("kind      {}", c.body().kind());
    println!("author    {} ({})", terminal::printable(&names.display(&c.author())), c.author());
    let parents: Vec<String> = c.parents().iter().map(|p| p.short()).collect();
    println!("parents   {}", if parents.is_empty() { "(root)".into() } else { parents.join(" ") });
    if g.is_retracted(&c.id()) {
        println!("retracted yes");
    }
    if let Some(a) = assessment {
        let status = serde_json::to_value(a.status).expect("status serializes");
        println!(
            "status    {} ({} passing, {} failing)",
            status.as_str().unwrap_or("?"),
            a.passes.len(),
            a.fails.len()
        );
        if !a.superseded_by.is_empty() {
            let by: Vec<String> = a.superseded_by.iter().map(|i| i.short()).collect();
            println!("replaced  by {}", by.join(" "));
        }
        if !a.changes.is_empty() {
            let by: Vec<String> = a.changes.iter().map(|i| i.short()).collect();
            println!("changed   by {} (a pass that has seen them clears it)", by.join(" "));
        }
    }
    println!();
    // Text other agents wrote: each of its lines starts "| ", so none can pass for a line of this listing,
    // and anything else that could move the cursor or reorder the line prints as an escape.
    let quoted = |text: &str| {
        for l in text.split('\n') {
            println!("| {}", terminal::printable(l));
        }
    };
    match c.body() {
        Body::Claim { statement, tags } => {
            quoted(statement);
            if !tags.is_empty() {
                println!("\ntags: {}", terminal::printable(&tags.join(", ")));
            }
        }
        Body::Artifact { name, media_type, content } => {
            let (name, media_type) = (terminal::printable(name), terminal::printable(media_type));
            println!("{name}  {media_type}  {} bytes", content.len());
            println!("(use `bracken cat {}` for the content)", c.id().short());
        }
        Body::Evidence { details, .. } if !details.is_empty() => {
            println!("{}\n", terminal::printable(&summary(g, c)));
            quoted(details);
        }
        _ => println!("{}", terminal::printable(&summary(g, c))),
    }
    if !referrers.is_empty() {
        println!("\nreferenced by:");
        for r in referrers {
            println!("  {}", line(g, names, r));
        }
    }
}

fn print_conflicts(g: &Graph, names: &Names, conflicts: &[Conflict], as_json: bool) {
    if as_json {
        print_json(&conflicts);
        return;
    }
    let describe = |id: &ContributionId| {
        g.get(id).map_or_else(
            || id.short(),
            |c| terminal::printable(&format!("{}  {}  {}", id.short(), names.display(&c.author()), summary(g, c))),
        )
    };
    for conflict in conflicts {
        match conflict {
            Conflict::Divergence { target, successors } => {
                println!("divergence: {} competing replacements for", successors.len());
                println!("    {}", describe(target));
                for s in successors {
                    println!("  > {}", describe(s));
                }
            }
            Conflict::Contradiction { relation, a, b } => {
                println!("contradiction ({}):", relation.short());
                println!("    {}", describe(a));
                println!("  x {}", describe(b));
            }
        }
    }
}

/// Writes to stdout. A reader that stops early (`| head -1`) ends the program quietly, where
/// `println!` would panic, with the status the command already decided ([`decide`]), or 0.
/// Commands finish writing to the Ground before they print, so stopping here never leaves a
/// write half-done.
fn write_stdout(bytes: &[u8]) {
    if let Err(e) = io::stdout().write_all(bytes) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(BROKEN_PIPE_STATUS.load(Ordering::Relaxed));
        }
        eprintln!("bracken: writing to stdout: {e}");
        std::process::exit(1);
    }
}

fn print_json(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).expect("values serialize"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding `7ae8094d7f55`: W works blocked on one need, and R relations saying one contribution
    /// implements it. Each standing used to assess that contribution once per relation, every assessment
    /// reading all R relations: W x R^2. Now each contribution is assessed once per status call.
    #[test]
    fn status_stays_linear_however_many_relations_anyone_writes() {
        let ground: GroundId = "0".repeat(64).parse().expect("a test ground");
        let (s, p) = (Identity::generate("s").expect("a key"), Identity::generate("p").expect("a key"));
        let mut g = Graph::new(ground);
        let mut ts = 0;
        let mut add = |g: &mut Graph, who: &Identity, body: Body| {
            ts += 1;
            let c = Contribution::create(ground, who, [], ts, body);
            let id = c.id();
            g.insert(c).expect("a valid contribution");
            id
        };
        let claim = |text: &str| Body::Claim { statement: text.into(), tags: vec![] };
        let relate = |from, kind, to, note: String| Body::Relation { from, kind, to, note };
        let goal = add(&mut g, &s, claim("goal"));
        let need = add(&mut g, &s, claim("need"));
        let x = add(&mut g, &s, claim("x"));
        let (w, r) = (1000, 1000);
        for i in 0..r {
            add(&mut g, &s, relate(x, RelationKind::Implements, need, format!("{i}")));
        }
        for i in 0..w {
            let work = add(&mut g, &s, claim(&format!("work {i}")));
            add(&mut g, &s, relate(work, RelationKind::Implements, goal, String::new()));
            let fail =
                Body::Evidence { target: work, verdict: Verdict::Fail, method: "m".into(), details: String::new() };
            let fail = add(&mut g, &p, fail);
            add(&mut g, &p, relate(fail, RelationKind::DependsOn, need, String::new()));
        }
        let policy = Policy::default();
        let start = std::time::Instant::now();
        let (rows, _) = status_rows(&g, &policy, false, ts);
        let works: Vec<&Work> = rows.iter().filter(|(c, _, _)| c.id() == goal).flat_map(|(_, ws, _)| ws).collect();
        assert_eq!(works.len(), w);
        assert!(works.iter().all(|w| w.standing == Standing::Blocked && w.blocked_on == [need]));
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());

        let order = g.topo_order();
        let view = CurrentView::new(&g, &order);
        let mut judge = Judge::new(&g, &view, &policy, &order);
        for work in &works {
            judge.standing(&work.assessment);
        }
        assert_eq!(judge.assessed, 2, "the need and x, once each");
    }

    #[test]
    fn the_superseded_by_mark_escapes_the_agent_name() {
        let author: AgentId = "a".repeat(64).parse().expect("a test key");
        let by: ContributionId = "b".repeat(64).parse().expect("a test id");
        let names = Names(HashMap::from([(author, "ev\u{1b}[2Kil\r\nfake".to_owned())]));
        assert_eq!(
            superseded_by_suffix(&names, &by, &author),
            format!("  (superseded by ev\\u{{1b}}[2Kil\\u{{d}}\\u{{a}}fake {})", by.short())
        );
    }
}
