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

mod remote;
mod serve;
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
                    println!("{}  {}{key}", a.agent, a.name);
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
        Command::Artifact { file, name, media_type } => {
            let who = signer(&s, agent)?;
            let content = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let name = name.unwrap_or_else(|| file.display().to_string());
            println!("{}", contribute(&mut s, &who, Body::Artifact { name, media_type, content })?);
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
        Command::Log { kind, tag, author, current, out } => {
            let names = Names::load(&s)?;
            let author = author.map(|a| names.resolve(&a)).transpose()?;
            let g = s.graph();
            let order = g.topo_order();
            let view = current.then(|| CurrentView::new(g, &order));
            let rows: Vec<&Contribution> = order
                .iter()
                .filter(|id| view.as_ref().is_none_or(|v| !v.hidden.contains(id)))
                .map(|id| g.get(id).expect("ordered ids exist"))
                .filter(|c| kind.is_none_or(|k| c.body().kind() == k))
                .filter(|c| author.is_none_or(|a| c.author() == a))
                .filter(|c| {
                    tag.as_ref().is_none_or(|t| matches!(c.body(), Body::Claim { tags, .. } if tags.contains(t)))
                })
                .collect();
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
                print_json(&rows);
            } else {
                for c in rows {
                    let mut text = line(g, &names, c);
                    for (by, author) in view.as_ref().map_or(&[][..], |v| v.by_others(&c.id())) {
                        let author = names.display(author);
                        text.push_str(&format!("  (superseded by {author} {})", by.short()));
                    }
                    println!("{text}");
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
            if out.json {
                let mut v = contribution_json(g, &names, c);
                v["assessment"] = json!(assessment);
                v["referrers"] = referrers.iter().map(|r| contribution_json(g, &names, r)).collect();
                print_json(&v);
            } else {
                print_show(g, &names, c, assessment.as_ref(), &referrers);
            }
        }
        Command::Cat { id } => {
            let id = resolve(&s, &id)?;
            let Body::Artifact { content, .. } = s.graph().get(&id).expect("resolved ids exist").body() else {
                bail!("{id} is not an artifact");
            };
            write_stdout(content);
        }
        Command::Heads { out } => {
            let names = Names::load(&s)?;
            let g = s.graph();
            let heads: Vec<&Contribution> = g.heads().map(|id| g.get(&id).expect("heads exist")).collect();
            if out.json {
                print_json(&heads.iter().map(|c| contribution_json(g, &names, c)).collect::<Vec<_>>());
            } else {
                for c in heads {
                    println!("{}", line(g, &names, c));
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
        Command::Sync { exec: Some(program), bounds, .. } => {
            return sync_remote(remote::sync_exec(&mut s, &program, bounds.bounds())?);
        }
        Command::Sync { other: Some(url), token_file, bounds, .. } if url.contains("://") => {
            let token = match token_file {
                Some(path) => std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
                None => std::env::var("BRACKEN_TOKEN").context("set BRACKEN_TOKEN or pass --token-file")?,
            };
            return sync_remote(remote::sync_http(&mut s, &url, token.trim(), bounds.bounds())?);
        }
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

/// Reports a session with a server: what each side refused, and what each received. Fails if
/// either side refused anything or was left with contributions missing ancestors.
fn sync_remote(side: wire::Side) -> Result<ExitCode> {
    let peer = &side.peer;
    let code = decide(side.report.is_clean() && peer.rejected.is_empty() && peer.unresolved == 0);
    print_report(&side.report);
    for r in &peer.rejected {
        eprintln!("the server refused {}: {}", r.id.short(), r.reason);
    }
    if peer.unresolved > 0 {
        eprintln!("the server is missing ancestors of {} contributions", peer.unresolved);
    }
    println!("received {}, sent {}", side.report.added.len(), peer.added);
    Ok(code)
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
    format!(
        "{}  {:<8}  {:<12}  {}{retracted}",
        c.id().short(),
        c.body().kind(),
        names.display(&c.author()),
        summary(g, c)
    )
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
    println!("author    {} ({})", names.display(&c.author()), c.author());
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
    match c.body() {
        Body::Claim { statement, tags } => {
            println!("{statement}");
            if !tags.is_empty() {
                println!("\ntags: {}", tags.join(", "));
            }
        }
        Body::Artifact { name, media_type, content } => {
            println!("{name}  {media_type}  {} bytes", content.len());
            println!("(use `bracken cat {}` for the content)", c.id().short());
        }
        Body::Evidence { details, .. } if !details.is_empty() => println!("{}\n\n{details}", summary(g, c)),
        _ => println!("{}", summary(g, c)),
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
            |c| format!("{}  {}  {}", id.short(), names.display(&c.author()), summary(g, c)),
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
