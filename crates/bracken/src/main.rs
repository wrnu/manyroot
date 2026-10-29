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

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use manyroot::store::{self, GroundInfo};
use manyroot::validation::{open_conflicts, open_supersessors};
use manyroot::*;
use serde_json::{Value, json};

mod listing;
mod remote;
mod serve;
mod terminal;
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
    },
    /// Relate two contributions, e.g. `bracken relate <a> supersedes <b>`.
    Relate {
        from: String,
        kind: RelationKind,
        to: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Record the outcome of checking a contribution.
    Evidence {
        target: String,
        verdict: Verdict,
        /// How it was checked, e.g. "cargo test".
        #[arg(long)]
        method: String,
        #[arg(long, default_value = "")]
        details: String,
        /// Read the details from a regular file, or "-" for stdin, so they never appear on a command
        /// line. Process substitution (`<(...)`) is a pipe, not a regular file: pipe into "-" instead.
        #[arg(long, value_name = "PATH", conflicts_with = "details")]
        details_file: Option<PathBuf>,
    },
    /// Withdraw one of your own contributions.
    Retract {
        target: String,
        #[arg(long, default_value = "")]
        reason: String,
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
        #[arg(long, value_name = "CURSOR")]
        since: Option<String>,
        /// Only the most recent N, after every other filter. Says how many earlier ones it left out (in the
        /// JSON as "omitted"); the cursor is still the heads, so the next --since starts after all of them.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// One short line each, at most 200 characters (counted as chars, not display columns): the
        /// shortest unique id prefix (at least 8 digits), author, kind and tags, and the statement or name
        /// cut to one line. Control characters print as escapes such as \u{1b}, as in every log line.
        #[arg(long, conflicts_with = "json")]
        compact: bool,
        #[command(flatten)]
        out: Output,
    },
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
    },
    /// For each current claim that something implements or intends to, what does: which one the
    /// policy accepts, which are still waiting for an independent pass (listed first), and the live
    /// intents.
    Status {
        /// Also list the current claims that nothing implements, at the end.
        #[arg(long)]
        all: bool,
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
        command => return run_in_store(open_store(cli.dir.as_deref())?, cli.agent.as_deref(), command),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_in_store(mut s: Store, agent: Option<&str>, command: Command) -> Result<ExitCode> {
    match command {
        Command::Init { .. } | Command::Join { .. } => unreachable!("handled before opening a store"),
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
        Command::Claim { statement, tags, supersedes } => {
            let who = signer(&s, agent)?;
            let names = Names::load(&s)?;
            let old = supersedes.iter().map(|id| own_claim(&s, &names, &who, id)).collect::<Result<BTreeSet<_>>>()?;
            let new = contribute(&mut s, &who, Body::Claim { statement, tags })?;
            let mut written = vec![new];
            for to in old {
                let note = String::new();
                written.push(contribute(
                    &mut s,
                    &who,
                    Body::Relation { from: new, kind: RelationKind::Supersedes, to, note },
                )?);
            }
            for id in written {
                println!("{id}");
            }
        }
        Command::Artifact { file, name, media_type, implements } => {
            let who = signer(&s, agent)?;
            // Resolved first, so an unknown id writes nothing.
            let target = implements.as_deref().map(|id| resolve(&s, id)).transpose()?;
            let content = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let name = name.unwrap_or_else(|| file.display().to_string());
            let artifact = contribute(&mut s, &who, Body::Artifact { name, media_type, content })?;
            println!("{artifact}");
            if let Some(to) = target {
                let relation =
                    Body::Relation { from: artifact, kind: RelationKind::Implements, to, note: String::new() };
                match contribute(&mut s, &who, relation) {
                    Ok(id) => println!("{id}"),
                    // The artifact stays: the Ground is append-only. Its id is printed above.
                    Err(e) => {
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
        }
        Command::Relate { from, kind, to, note } => {
            let who = signer(&s, agent)?;
            let (from, to) = (resolve(&s, &from)?, resolve(&s, &to)?);
            println!("{}", contribute(&mut s, &who, Body::Relation { from, kind, to, note })?);
        }
        Command::Evidence { target, verdict, method, details, details_file } => {
            let who = signer(&s, agent)?;
            let details = match details_file {
                Some(path) => read_details(&path)?,
                None => details,
            };
            let target = resolve(&s, &target)?;
            println!("{}", contribute(&mut s, &who, Body::Evidence { target, verdict, method, details })?);
        }
        Command::Retract { target, reason } => {
            let who = signer(&s, agent)?;
            let target = resolve(&s, &target)?;
            println!("{}", contribute(&mut s, &who, Body::Retract { target, reason })?);
        }
        Command::Log { kind, tag, author, current, since, limit, compact, out } => {
            let names = Names::load(&s)?;
            let author = author.map(|a| names.resolve(&a)).transpose()?;
            let g = s.graph();
            let seen = since.as_deref().map(|c| listing::seen(g, c)).transpose()?;
            let order = g.topo_order();
            let view = current.then(|| CurrentView::new(g, &order));
            let rows: Vec<&Contribution> = order
                .iter()
                .filter(|id| seen.as_ref().is_none_or(|s| !s.contains(id)))
                .filter(|id| view.as_ref().is_none_or(|v| !v.hidden.contains(id)))
                .map(|id| g.get(id).expect("ordered ids exist"))
                .filter(|c| kind.is_none_or(|k| c.body().kind() == k))
                .filter(|c| author.is_none_or(|a| c.author() == a))
                .filter(|c| {
                    tag.as_ref().is_none_or(|t| matches!(c.body(), Body::Claim { tags, .. } if tags.contains(t)))
                })
                .collect();
            let omitted = limit.map_or(0, |n| rows.len().saturating_sub(n));
            let rows = &rows[omitted..];
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
                    print_json(&json!({"contributions": rows, "cursor": listing::cursor(g), "omitted": omitted}));
                } else {
                    print_json(&rows);
                }
            } else {
                let prefixes =
                    compact.then(|| listing::unique_prefixes(g.iter().map(|c| c.id()), listing::COMPACT_ID_DIGITS));
                for &c in rows {
                    let mut text = match &prefixes {
                        Some(p) => compact_line(g, &names, p, c),
                        None => line(g, &names, c),
                    };
                    for (by, author) in view.as_ref().map_or(&[][..], |v| v.by_others(&c.id())) {
                        text.push_str(&superseded_by_suffix(&names, by, author));
                    }
                    if compact {
                        text = listing::cap_line(text);
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
        Command::Show { id, policy, out } => {
            let names = Names::load(&s)?;
            let policy = policy.build(&names)?;
            let g = s.graph();
            let c = g.get(&resolve(&s, &id)?).expect("resolved ids exist");
            let assessment = policy.assess(g, &c.id());
            let referrers: Vec<&Contribution> = g.referrers(&c.id()).collect();
            let intents = live_intents(g, &CurrentView::new(g, &g.topo_order()), &c.id(), now_ms());
            if out.json {
                let mut v = contribution_json(g, &names, c);
                v["assessment"] = json!(assessment);
                v["referrers"] = referrers.iter().map(|r| contribution_json(g, &names, r)).collect();
                v["intents"] = intents.iter().map(|i| intent_json(g, &names, i, now_ms())).collect();
                print_json(&v);
            } else {
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
            let e = policy.evaluate(g);
            if out.json {
                let accepted: Vec<Value> = e
                    .accepted
                    .iter()
                    .map(|a| {
                        let mut v = contribution_json(g, &names, g.get(&a.id).expect("assessed ids exist"));
                        v["assessment"] = json!(a);
                        v
                    })
                    .collect();
                let conflicts: Vec<Value> = e.conflicts.iter().map(|c| json!(c)).collect();
                print_json(&json!({"accepted": accepted, "conflicts": conflicts}));
            } else {
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
        Command::Status { all, policy, out } => print_status(&s, all, &policy, out.json)?,
        Command::Intend { id, duration } => {
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
            let intent = contribute(&mut s, &who, Body::Claim { statement, tags })?;
            println!("{intent}");
            let relation =
                Body::Relation { from: intent, kind: RelationKind::Refines, to: target, note: String::new() };
            match contribute(&mut s, &who, relation) {
                Ok(id) => println!("{id}"),
                // The claim stays: the Ground is append-only. Its id is printed above.
                Err(e) => {
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

/// `status`: each current claim that something implements or has a live intent on, with what
/// implements it and how the policy assesses each, waiting ones first, and its live intents; with
/// `all`, then the current claims with neither. A contribution implements a claim through a current
/// `implements` relation from it to the claim, whoever wrote the relation, and whatever kind of
/// contribution it is.
fn print_status(s: &Store, all: bool, policy: &PolicyArgs, json_out: bool) -> Result<()> {
    let names = Names::load(s)?;
    let policy = policy.build(&names)?;
    let g = s.graph();
    let order = g.topo_order();
    let view = CurrentView::new(g, &order);
    let now = now_ms();
    let rank = |status: Status| match status {
        Status::Pending => 0,
        Status::Accepted => 1,
        Status::Superseded => 2,
        Status::Rejected => 3,
        Status::Retracted => 4,
    };
    // Each claim, with what implements it and its live intents.
    type Row<'a> = (&'a Contribution, Vec<(&'a Contribution, Assessment)>, Vec<Intent<'a>>);
    let mut implemented: Vec<Row> = Vec::new();
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
        let mut rows: Vec<(&Contribution, Assessment)> =
            by.iter().filter_map(|i| Some((g.get(i).expect("related ids exist"), policy.assess(g, i)?))).collect();
        let intents = live_intents(g, &view, id, now);
        if rows.is_empty() && intents.is_empty() {
            if all {
                unimplemented.push(c);
            }
            continue;
        }
        rows.sort_by_key(|(i, a)| (rank(a.status), i.content().timestamp_ms));
        implemented.push((c, rows, intents));
    }
    // Claims with something waiting first; otherwise in causal order (the sort is stable).
    implemented.sort_by_key(|(_, rows, _)| rows.iter().all(|(_, a)| a.status != Status::Pending));

    let age_ms = |c: &Contribution| now.saturating_sub(c.content().timestamp_ms);
    // Distinct validators with a counted pass, as the policy counts them: one validator passing twice is one.
    let independent =
        |a: &Assessment| a.passes.iter().filter_map(|e| g.get(e)).map(|e| e.author()).collect::<HashSet<_>>().len();
    if json_out {
        let claims: Vec<Value> = implemented
            .iter()
            .map(|(c, rows, intents)| {
                let by: Vec<Value> = rows
                    .iter()
                    .map(|(i, a)| {
                        let mut v = contribution_json(g, &names, i);
                        v["assessment"] = json!(a);
                        v["waiting"] = json!(a.status == Status::Pending);
                        v["independent_passes"] = json!(independent(a));
                        v["age_ms"] = json!(age_ms(i));
                        v
                    })
                    .collect();
                let intents: Vec<Value> = intents.iter().map(|i| intent_json(g, &names, i, now)).collect();
                json!({"claim": contribution_json(g, &names, c), "implemented_by": by, "intents": intents})
            })
            .collect();
        let unimplemented: Vec<Value> = unimplemented.iter().map(|c| contribution_json(g, &names, c)).collect();
        print_json(&json!({"min_passes": policy.min_passes, "claims": claims, "unimplemented": unimplemented}));
        return Ok(());
    }
    if implemented.is_empty() {
        println!("no current claim is implemented by anything, or has a live intent");
    }
    for (n, (c, rows, intents)) in implemented.iter().enumerate() {
        if n > 0 {
            println!();
        }
        // Other agents' text is printed escaped, so it can't forge lines (finding 252f052a283d).
        println!("{}", terminal::printable(&line(g, &names, c)));
        for i in intents {
            println!("{}", terminal::printable(&intent_line(&names, i, now)));
        }
        for (i, a) in rows {
            let label = match a.status {
                Status::Pending => "waiting",
                Status::Accepted => "accepted",
                Status::Superseded => "superseded",
                Status::Rejected => "rejected",
                Status::Retracted => "retracted",
            };
            let row = format!(
                "  {label:<10}  {}  {:<8}  {:<12}  {:>4} ago  {}/{} passes  {}",
                i.id().short(),
                i.body().kind(),
                names.display(&i.author()),
                age(age_ms(i)),
                independent(a),
                policy.min_passes,
                summary(g, i)
            );
            println!("{}", terminal::printable(&row));
        }
    }
    if !unimplemented.is_empty() {
        println!();
        println!("nothing implements, or intends to:");
        for c in &unimplemented {
            println!("{}", terminal::printable(&line(g, &names, c)));
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
        Ok(Policy { min_passes: self.min_passes, validators })
    }
}

/// `log --current`'s mark on a line that another author claims to supersede. The rest of the line is
/// escaped already; the name is escaped here, since an agent entry's name isn't checked when it's read
/// (security evidence cb1a56001700).
fn superseded_by_suffix(names: &Names, by: &ContributionId, author: &AgentId) -> String {
    format!("  (superseded by {} {})", terminal::printable(&names.display(author)), by.short())
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
