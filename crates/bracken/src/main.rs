//! `bracken`: agent-native collaborative development on Manyroot.
//!
//! Every command that reads the graph accepts `--json` so agents can consume
//! it directly. Writes print the new contribution's full id on stdout.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use manyroot::store::{self, GroundInfo};
use manyroot::validation::open_conflicts;
use manyroot::*;
use serde_json::{Value, json};

/// Directory holding a Bracken workspace's Ground replica.
const DIR_NAME: &str = ".bracken";

#[derive(Parser)]
#[command(name = "bracken", version, about = "Software that grows together.")]
struct Cli {
    /// Ground replica to use. Defaults to the nearest .bracken directory.
    #[arg(long, global = true, env = "BRACKEN_DIR")]
    dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Author {
    /// Local identity to sign with.
    #[arg(long = "as", env = "BRACKEN_AGENT", value_name = "NAME")]
    agent: String,
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
        #[command(flatten)]
        author: Author,
    },
    /// Contribute an artifact from a file.
    Artifact {
        file: PathBuf,
        /// Name to record. Defaults to the file path as given.
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
        #[command(flatten)]
        author: Author,
    },
    /// Relate two contributions, e.g. `bracken relate <a> supersedes <b>`.
    Relate {
        from: String,
        kind: RelationKind,
        to: String,
        #[arg(long, default_value = "")]
        note: String,
        #[command(flatten)]
        author: Author,
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
        #[command(flatten)]
        author: Author,
    },
    /// Withdraw one of your own contributions.
    Retract {
        target: String,
        #[arg(long, default_value = "")]
        reason: String,
        #[command(flatten)]
        author: Author,
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
    /// Exchange contributions with another replica of the same Ground.
    Sync {
        /// A directory containing .bracken, or a replica directory itself.
        other: PathBuf,
    },
    /// Write every contribution to a bundle file ("-" for stdout).
    Export { file: PathBuf },
    /// Import contributions from a bundle file ("-" for stdin).
    Import { file: PathBuf },
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
            print_report(&report);
            println!("joined ground {ground} in {}", dir.display());
            return Ok(exit_for(&report));
        }
        command => return run_in_store(open_store(cli.dir.as_deref())?, command),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_in_store(mut s: Store, command: Command) -> Result<ExitCode> {
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
        Command::Claim { statement, tags, author } => {
            let who = s.identity(&author.agent)?;
            println!("{}", s.contribute(&who, Body::Claim { statement, tags })?);
        }
        Command::Artifact { file, name, media_type, author } => {
            let who = s.identity(&author.agent)?;
            let content = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let name = name.unwrap_or_else(|| file.display().to_string());
            println!("{}", s.contribute(&who, Body::Artifact { name, media_type, content })?);
        }
        Command::Relate { from, kind, to, note, author } => {
            let who = s.identity(&author.agent)?;
            let (from, to) = (resolve(&s, &from)?, resolve(&s, &to)?);
            println!("{}", s.contribute(&who, Body::Relation { from, kind, to, note })?);
        }
        Command::Evidence { target, verdict, method, details, author } => {
            let who = s.identity(&author.agent)?;
            let target = resolve(&s, &target)?;
            println!("{}", s.contribute(&who, Body::Evidence { target, verdict, method, details })?);
        }
        Command::Retract { target, reason, author } => {
            let who = s.identity(&author.agent)?;
            let target = resolve(&s, &target)?;
            println!("{}", s.contribute(&who, Body::Retract { target, reason })?);
        }
        Command::Log { kind, tag, author, out } => {
            let names = Names::load(&s)?;
            let author = author.map(|a| names.resolve(&a)).transpose()?;
            let g = s.graph();
            let rows: Vec<&Contribution> = g
                .topo_order()
                .iter()
                .map(|id| g.get(id).expect("ordered ids exist"))
                .filter(|c| kind.is_none_or(|k| c.body().kind() == k))
                .filter(|c| author.is_none_or(|a| c.author() == a))
                .filter(|c| {
                    tag.as_ref().is_none_or(|t| matches!(c.body(), Body::Claim { tags, .. } if tags.contains(t)))
                })
                .collect();
            if out.json {
                print_json(&rows.iter().map(|c| contribution_json(g, &names, c)).collect::<Vec<_>>());
            } else {
                for c in rows {
                    println!("{}", line(g, &names, c));
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
            io::stdout().write_all(content)?;
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
        Command::Sync { other } => {
            let other_dir = if other.join(DIR_NAME).is_dir() { other.join(DIR_NAME) } else { other };
            let mut o = Store::open(&other_dir)?;
            let (here, there) = store::sync(&mut s, &mut o)?;
            println!("received {}, sent {}", here.added.len(), there.added.len());
            print_report(&here);
            print_report(&there);
            return Ok(if here.is_clean() && there.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
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
            println!("imported {}, already had {}", report.added.len(), report.already_present);
            print_report(&report);
            return Ok(exit_for(&report));
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

fn resolve(s: &Store, prefix: &str) -> Result<ContributionId> {
    Ok(s.graph().resolve_prefix(prefix)?)
}

fn print_report(report: &ImportReport) {
    for (id, why) in &report.rejected {
        eprintln!("rejected {}: {why}", id.short());
    }
    if !report.unresolved.is_empty() {
        eprintln!("{} contributions are missing ancestors", report.unresolved.len());
    }
}

fn exit_for(report: &ImportReport) -> ExitCode {
    if report.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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

fn print_json(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).expect("values serialize"));
}
