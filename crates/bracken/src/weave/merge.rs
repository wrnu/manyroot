//! `bracken weave` for policies, proposals and merges: `line create`, `line
//! policy`, `line merge`, `propose`, `integrate --proposals` and `resolve`
//! (docs/design/weave.md, sections 1 to 3; build step 6).
//!
//! A merge is built in steps, each merging one tree into the tree so far,
//! file by file against a base: another line's head (`line merge`), or each
//! proposal in turn (`integrate --proposals`). A conflict that no resolution
//! settles stops it, and the command lists the conflicts and writes nothing.
//! `resolve` takes the same selector, rebuilds the same merge, and writes a
//! resolution for the conflict at one path; `--take into` is the side merged
//! into (the line), `--take from` the side merged in.
//!
//! A merge applies on its own only resolutions written by an integrator of the
//! target line, the smallest id first. Anyone else's are listed as offered, and
//! an integrator adopts one by writing their own: `resolve --take <id>`
//! (decision 91c12b4da2ad).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail, ensure};
use bracken_tree::format::Record;
use bracken_tree::format::{
    Change, Decision, FileValue, Guardrails, Line, Manifest, Mode, Proposal, Resolution, VERSION,
};
use bracken_tree::ground::{Index, kind_of};
use bracken_tree::import::{self, Source};
use bracken_tree::lines::{Lines, Unqualified};
use bracken_tree::merge::{self as files, Conflict, MergeError, Resolutions, Resolved};
use bracken_tree::{ContentHash, diff, path, secrets};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use manyroot::{AgentId, Body, ContributionId, RelationKind, Store};
use serde_json::{Value, json};

use super::{
    checks, decision_id, file_value_json, held_manifest, known_keys, print_tree_written, refusal, resolve_line,
    write_record, write_tree,
};
use crate::{Names, Output, contribute, decide, print_json, signer, write_stdout};

#[derive(Subcommand)]
pub(crate) enum LineCommand {
    /// Create a line: its first policy version, signed by --as, who alone can change it later. With no
    /// --integrator or --goal-setter, the author is the only one.
    Create {
        /// The line's name: only a label; a line is named by the id of its first artifact.
        name: String,
        #[command(flatten)]
        policy: PolicyArgs,
        /// Show the policy, and write nothing.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        out: Output,
    },
    /// Write a new version of a line's policy, superseding every current version (so it also settles a
    /// diverged chain). Only the line's author can. An option not given keeps the value of the current
    /// version it starts from: the only one, or the one --from names.
    Policy {
        /// The line: its id or name.
        line: String,
        /// A new name for the line.
        #[arg(long)]
        name: Option<String>,
        /// The current version to start from: needed while the chain has diverged into several. The new
        /// version supersedes every current one all the same.
        #[arg(long, value_name = "VERSION")]
        from: Option<String>,
        /// Empty a list before adding to it: integrators, validators, goal-setters, checks or protected.
        #[arg(long, value_enum, value_name = "FIELD")]
        clear: Vec<Field>,
        #[command(flatten)]
        policy: PolicyArgs,
        /// Show the new version and what it would supersede, and write nothing.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        out: Output,
    },
    /// Integrator: merge one line's head into another's, file by file against their merge base. With every
    /// conflict resolved, writes the merged manifest and a decision on --into whose parents are both heads;
    /// otherwise, or with --dry-run, lists the conflicts and writes nothing. A line with no decision yet
    /// starts from the other's head.
    Merge {
        /// The line merged in: its id or name.
        from: String,
        /// The line merged into: its id or name.
        #[arg(long, value_name = "LINE")]
        into: String,
        /// The merge base, a decision: chooses one when the heads have several (a criss-cross).
        #[arg(long, value_name = "DECISION")]
        base: Option<String>,
        /// List what the merge gives, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// What the decision does, in words.
        #[arg(long, default_value = "")]
        note: String,
        #[command(flatten)]
        out: Output,
    },
}

/// A policy's fields, as options.
#[derive(Args)]
pub(crate) struct PolicyArgs {
    /// An integrator, by local identity name or public key. Repeat for several.
    #[arg(long = "integrator", value_name = "IDENTITY")]
    integrators: Vec<String>,
    /// A validator, by local identity name or public key. Repeat for several.
    #[arg(long = "validator", value_name = "IDENTITY")]
    validators: Vec<String>,
    /// A goal setter, by local identity name or public key. Repeat for several.
    #[arg(long = "goal-setter", value_name = "IDENTITY")]
    goal_setters: Vec<String>,
    /// A check, as NAME=ARGV with the argv split at spaces, e.g. 'test=cargo test --workspace'. Repeat for
    /// several; a name given again replaces its argv.
    #[arg(long = "check", value_name = "NAME=ARGV")]
    checks: Vec<String>,
    /// Guardrail: how many known breakages a decision may carry.
    #[arg(long, value_name = "N")]
    max_known: Option<u64>,
    /// Guardrail: for how many decisions a breakage may stay.
    #[arg(long, value_name = "N")]
    max_age: Option<u64>,
    /// Guardrail: a path prefix no breakage may name. Repeat for several.
    #[arg(long = "protected", value_name = "PREFIX")]
    protected: Vec<String>,
    /// The id of the owner's decision, in the coordination Ground, that allows this version: 12 to 64 hex
    /// digits. A version without one counts for nothing.
    #[arg(long, value_name = "ID", required = true)]
    owner_decision: String,
}

/// A list field of a policy.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Field {
    Integrators,
    Validators,
    GoalSetters,
    Checks,
    Protected,
}

/// `weave propose`.
#[derive(Args)]
pub(crate) struct ProposeArgs {
    /// The line: its id or name. Defaults to the default line.
    #[arg(long, value_name = "LINE")]
    line: Option<String>,
    /// The decision the tree was made from. Defaults to the line's head.
    #[arg(long, value_name = "DECISION")]
    base: Option<String>,
    /// The commit whose tree to propose (the default is HEAD).
    #[arg(long, value_name = "REV", conflicts_with = "index")]
    from_git: Option<String>,
    /// Propose the index: tracked and staged files, with their staged contents.
    #[arg(long)]
    index: bool,
    /// What the proposal does, in words.
    #[arg(long, default_value = "")]
    summary: String,
    /// List the change and what would be written, and write nothing.
    #[arg(long)]
    dry_run: bool,
    /// The git repository to import from.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[command(flatten)]
    out: Output,
}

/// A mode a resolution may give its result.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ModeArg {
    File,
    Executable,
}

/// `weave resolve`.
#[derive(Args)]
#[command(group(ArgGroup::new("merge").required(true).args(["into", "proposals"])))]
#[command(group(ArgGroup::new("result").required(true).args(["take", "delete", "file"])))]
pub(crate) struct ResolveArgs {
    /// The conflicted path.
    path: String,
    /// As for line merge: the line merged into.
    #[arg(long, value_name = "LINE", requires = "from")]
    into: Option<String>,
    /// As for line merge: the line merged in.
    #[arg(long, value_name = "LINE", requires = "into")]
    from: Option<String>,
    /// As for line merge: the merge base.
    #[arg(long, value_name = "DECISION", requires = "into")]
    base: Option<String>,
    /// As for integrate: the proposals, in order.
    #[arg(long, value_name = "PROPOSAL", num_args = 1.., conflicts_with = "into")]
    proposals: Vec<String>,
    /// As for integrate: the line. Defaults to the proposals' line.
    #[arg(long, value_name = "LINE", requires = "proposals")]
    line: Option<String>,
    /// Settle it with `into` (the line's side), `from` (the side merged in), or a resolution of this
    /// conflict by its id: adopting one somebody else offered, as a resolution of your own.
    #[arg(long, value_name = "into|from|RESOLUTION")]
    take: Option<String>,
    /// Settle it by deleting the file.
    #[arg(long)]
    delete: bool,
    /// Settle it with this file's bytes. It must be inside this git checkout's work tree, outside .git and
    /// the .bracken root entries, and pass import's secret refusals.
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
    /// The result's mode. Defaults to the side's mode with --take, and to the file's executable bit with
    /// --file.
    #[arg(long, value_enum, conflicts_with = "delete")]
    mode: Option<ModeArg>,
    /// Show the resolution, and write nothing.
    #[arg(long)]
    dry_run: bool,
    /// The git checkout --file must be in.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[command(flatten)]
    out: Output,
}

pub(super) fn require_marker(ix: &Index) -> Result<()> {
    if ix.counted_marker().is_none() {
        bail!(
            "this Ground holds no Weave marker, so weave writes nothing here; create a Weave Ground with `bracken weave init`"
        );
    }
    Ok(())
}

pub(crate) fn line(s: &mut Store, agent: Option<&str>, command: LineCommand) -> Result<ExitCode> {
    match command {
        LineCommand::Create { name, policy, dry_run, out } => {
            let ix = Index::build(s.graph());
            if !dry_run {
                require_marker(&ix)?;
            }
            let who = signer(s, agent)?;
            let names = Names::load(s)?;
            let mut line = Line {
                v: VERSION,
                name,
                integrators: vec![],
                validators: vec![],
                goal_setters: vec![],
                checks: BTreeMap::new(),
                guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
                owner_decision: None,
            };
            apply(&mut line, &policy, &names)?;
            for list in [&mut line.integrators, &mut line.goal_setters] {
                if list.is_empty() {
                    list.push(who.agent_id());
                }
            }
            line.validate().context("the policy isn't valid, and nothing was written")?;
            if dry_run {
                print_policy_dry_run(&line, &[], &names, out);
                return Ok(ExitCode::SUCCESS);
            }
            let id = write_record(s, &who, &line)?;
            if out.json {
                print_json(&json!({"line": id, "name": line.name}));
            } else {
                println!("line {id} ({})", crate::terminal::printable(&line.name));
            }
            Ok(ExitCode::SUCCESS)
        }
        LineCommand::Policy { line, name, from, clear, policy, dry_run, out } => {
            let ix = Index::build(s.graph());
            if !dry_run {
                require_marker(&ix)?;
            }
            let lines = Lines::build(s.graph(), &ix);
            let first = resolve_line(s, &lines, Some(&line), "the line")?;
            let who = signer(s, agent)?;
            let author = lines.line(&first).expect("resolved").author;
            // Core supersession counts only when the old version's author signs it.
            if who.agent_id() != author {
                let names = Names::load(s)?;
                bail!("only the line's author, {}, can change its policy; nothing was written", names.display(&author));
            }
            let current = lines.current_versions(&first);
            // On a diverged chain, which version's fields carry on is the author's choice, never the
            // largest id's (finding 4eead6290997).
            let start = match (from, current.as_slice()) {
                (None, [one]) => *one,
                (None, many) => {
                    let names = Names::load(s)?;
                    let listed: Vec<String> = many
                        .iter()
                        .map(|v| {
                            let p = &lines.version(v).expect("a current version").policy;
                            let who: Vec<String> = p.integrators.iter().map(|a| names.display(a)).collect();
                            format!("{} (integrators {})", v.short(), who.join(", "))
                        })
                        .collect();
                    bail!(
                        "line {}'s policy has diverged into {}: {}; name the one to start from with --from, and the \
                         new version supersedes them all",
                        first.short(),
                        many.len(),
                        listed.join("; ")
                    );
                }
                (Some(given), _) => {
                    let id = s.graph().resolve_prefix(&given).with_context(|| format!("--from: {given:?}"))?;
                    if !current.contains(&id) {
                        let ids: Vec<String> = current.iter().map(ContributionId::short).collect();
                        bail!(
                            "--from: {} isn't a current version of the line; those are {}",
                            id.short(),
                            ids.join(", ")
                        );
                    }
                    id
                }
            };
            let mut next = lines.version(&start).expect("a current version").policy.clone();
            if let Some(name) = name {
                next.name = name;
            }
            for field in &clear {
                match field {
                    Field::Integrators => next.integrators.clear(),
                    Field::Validators => next.validators.clear(),
                    Field::GoalSetters => next.goal_setters.clear(),
                    Field::Checks => next.checks.clear(),
                    Field::Protected => next.guardrails.protected.clear(),
                }
            }
            let names = Names::load(s)?;
            apply(&mut next, &policy, &names)?;
            next.validate().context("the policy isn't valid, and nothing was written")?;
            if dry_run {
                print_policy_dry_run(&next, &current, &names, out);
                return Ok(ExitCode::SUCCESS);
            }
            let id = write_record(s, &who, &next)?;
            let mut relations = Vec::new();
            for old in &current {
                let body = Body::Relation { from: id, kind: RelationKind::Supersedes, to: *old, note: String::new() };
                match contribute(s, &who, body) {
                    Ok(r) => relations.push((r, *old)),
                    Err(e) => {
                        decide(false);
                        print_policy(first, id, &relations, out);
                        return Err(e.context(format!(
                            "line {id} was written, but not its supersedes relation to {old}; until it is, it \
                             doesn't change the policy"
                        )));
                    }
                }
            }
            print_policy(first, id, &relations, out);
            Ok(ExitCode::SUCCESS)
        }
        LineCommand::Merge { from, into, base, dry_run, note, out } => {
            let ix = Index::build(s.graph());
            if !dry_run {
                require_marker(&ix)?;
            }
            let lines = Lines::build(s.graph(), &ix);
            let target = lines_target(s, &lines, &ix, &into, &from, base.as_deref())?;
            write_merge(s, agent, &ix, &lines, target, note, dry_run, None, out)
        }
    }
}

fn print_policy(
    line: ContributionId,
    version: ContributionId,
    relations: &[(ContributionId, ContributionId)],
    out: Output,
) {
    if out.json {
        let rows: Vec<Value> = relations.iter().map(|(id, old)| json!({"relation": id, "supersedes": old})).collect();
        print_json(&json!({"line": line, "version": version, "relations": rows}));
        return;
    }
    println!("line {version}");
    for (id, _) in relations {
        println!("relation {id} (supersedes)");
    }
}

fn print_policy_dry_run(line: &Line, supersedes: &[ContributionId], names: &Names, out: Output) {
    let agents = |list: &[AgentId]| -> Vec<String> { list.iter().map(|a| names.display(a)).collect() };
    if out.json {
        let ids = |list: &[AgentId]| -> Vec<String> { list.iter().map(ToString::to_string).collect() };
        print_json(&json!({"line": Value::Null, "name": line.name, "integrators": ids(&line.integrators),
                           "validators": ids(&line.validators), "goal_setters": ids(&line.goal_setters),
                           "checks": line.checks, "guardrails": {"max_known": line.guardrails.max_known,
                           "max_age": line.guardrails.max_age, "protected": line.guardrails.protected},
                           "owner_decision": line.owner_decision, "supersedes": supersedes}));
        return;
    }
    let printable = crate::terminal::printable;
    println!("line {} (dry run, nothing written)", printable(&line.name));
    println!("  integrators: {}", printable(&agents(&line.integrators).join(", ")));
    println!("  validators: {}", printable(&agents(&line.validators).join(", ")));
    println!("  goal setters: {}", printable(&agents(&line.goal_setters).join(", ")));
    for (name, argv) in &line.checks {
        println!("  check {}: {}", printable(name), printable(&argv.join(" ")));
    }
    let g = &line.guardrails;
    println!(
        "  guardrails: max known {}, max age {}, protected {}",
        g.max_known,
        g.max_age,
        printable(&g.protected.join(", "))
    );
    println!("  owner decision {}", printable(line.owner_decision.as_deref().unwrap_or_default()));
    for old in supersedes {
        println!("  supersedes {old}");
    }
}

/// Adds the options given to a policy.
fn apply(line: &mut Line, given: &PolicyArgs, names: &Names) -> Result<()> {
    let agents = |list: &[String], what: &str| -> Result<Vec<AgentId>> {
        list.iter().map(|n| names.resolve(n).with_context(|| format!("--{what}"))).collect()
    };
    for (list, given, what) in [
        (&mut line.integrators, &given.integrators, "integrator"),
        (&mut line.validators, &given.validators, "validator"),
        (&mut line.goal_setters, &given.goal_setters, "goal-setter"),
    ] {
        for agent in agents(given, what)? {
            if !list.contains(&agent) {
                list.push(agent);
            }
        }
    }
    for check in &given.checks {
        let (name, argv) = check.split_once('=').with_context(|| format!("--check {check:?}: give NAME=ARGV"))?;
        let argv: Vec<String> = argv.split_whitespace().map(str::to_owned).collect();
        line.checks.insert(name.trim().to_owned(), argv);
    }
    if let Some(n) = given.max_known {
        line.guardrails.max_known = n;
    }
    if let Some(n) = given.max_age {
        line.guardrails.max_age = n;
    }
    for prefix in &given.protected {
        if !line.guardrails.protected.contains(prefix) {
            line.guardrails.protected.push(prefix.clone());
        }
    }
    let cited = given.owner_decision.trim();
    ensure!(
        (12..=64).contains(&cited.len()) && cited.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "--owner-decision {cited:?}: give the id of the owner's decision claim, 12 to 64 lowercase hex digits"
    );
    line.owner_decision = Some(given.owner_decision.clone());
    Ok(())
}

pub(crate) fn propose(s: &mut Store, agent: Option<&str>, args: ProposeArgs) -> Result<ExitCode> {
    let ProposeArgs { line, base, from_git, index, summary, dry_run, repo, out } = args;
    let ix = Index::build(s.graph());
    if !dry_run {
        require_marker(&ix)?;
    }
    let lines = Lines::build(s.graph(), &ix);
    let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
    let base = match base {
        Some(b) => {
            let id = decision_id(s, &lines, &b, "--base")?;
            match lines.verdict(&id) {
                Some(Ok(q)) if q.line == line => id,
                Some(Ok(_)) => bail!("--base: decision {} is on another line", id.short()),
                _ => bail!("--base: decision {} doesn't qualify", id.short()),
            }
        }
        None => match lines.heads(&line).as_slice() {
            [one] => *one,
            [] => bail!("line {} has no decision yet, so there is nothing to propose against", line.short()),
            heads => {
                let ids: Vec<String> = heads.iter().map(ContributionId::short).collect();
                bail!("line {} is forked, with heads {}; name the base with --base", line.short(), ids.join(", "))
            }
        },
    };
    let base_manifest = held_manifest(&ix, &lines.decision(&base).expect("a decision").decision.manifest)?;
    let source = if index { Source::Index } else { Source::Rev(from_git.unwrap_or_else(|| "HEAD".to_owned())) };
    let plan = import::plan(&repo, &source, &known_keys(s, &repo)?)?;
    let Some(manifest) = plan.manifest() else {
        let refused: Vec<String> = plan.refused.iter().map(ToString::to_string).collect();
        if out.json {
            print_json(&json!({"refused": refused}));
        } else {
            for r in &refused {
                println!("refused {r}");
            }
        }
        decide(false);
        bail!("the tree is refused ({} refusals), and nothing was written", refused.len());
    };
    let change = Change::between(base_manifest, &manifest);
    if change.entries().is_empty() {
        bail!("the tree is the base's own, {}; there is nothing to propose", base.short());
    }
    let hash = manifest.content_hash();
    let held = ix.manifest(&hash).is_some();
    let new: Vec<&import::FileVersion> = plan.new_files(|h| ix.holds_file(h)).collect();
    let diffs = diff::manifests(base_manifest, &manifest);
    if dry_run {
        if out.json {
            let files: Vec<Value> = diffs.iter().map(|d| json!({"path": d.path, "change": d.kind.as_str()})).collect();
            print_json(&json!({"line": line, "base": base, "manifest": hash.to_string(), "manifest_new": !held,
                               "change": change.hash().to_string(), "files": files, "new_files": new.len()}));
        } else {
            for d in &diffs {
                println!("{} {}", d.kind.as_str(), crate::terminal::printable(&d.path));
            }
            let state = if held { "held" } else { "new" };
            println!(
                "proposal against {base}: manifest {hash} ({state}; {} new files; dry run, nothing written)",
                new.len()
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    let who = signer(s, agent)?;
    let (written, failure) = write_tree(s, &who, &new, &manifest, held)?;
    let files: Vec<(ContentHash, String)> = written
        .iter()
        .filter_map(|(p, _)| p.map(|p| (new.iter().find(|f| f.name == p).expect("written").hash, p.to_owned())))
        .collect();
    if let Some(e) = failure {
        decide(false);
        print_tree_written(None, &files, &[], out);
        return Err(e.context(format!("{} file versions were written, and no manifest or proposal", files.len())));
    }
    let proposal = Proposal { v: VERSION, line, base, manifest: hash, change: change.hash(), summary };
    let id = match write_record(s, &who, &proposal) {
        Ok(id) => id,
        Err(e) => {
            decide(false);
            print_tree_written(Some(hash), &files, &[], out);
            return Err(e.context("the tree was written, but not the proposal"));
        }
    };
    if out.json {
        let rows: Vec<Value> = files.iter().map(|(h, p)| json!({"hash": h.to_string(), "path": p})).collect();
        print_json(&json!({"manifest": hash.to_string(), "files": rows, "proposal": id, "line": line, "base": base,
                           "change": change.hash().to_string()}));
    } else {
        println!("manifest {hash}");
        for (h, p) in &files {
            println!("file {h} {}", crate::terminal::printable(p));
        }
        println!("proposal {id}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `weave integrate --proposals`.
pub(crate) fn integrate_proposals(
    s: &mut Store,
    agent: Option<&str>,
    line: Option<&str>,
    proposals: &[String],
    note: String,
    run: Option<&checks::Run>,
    out: Output,
) -> Result<ExitCode> {
    let ix = Index::build(s.graph());
    require_marker(&ix)?;
    let lines = Lines::build(s.graph(), &ix);
    let target = proposals_target(s, &lines, &ix, line, proposals)?;
    write_merge(s, agent, &ix, &lines, target, note, false, run, out)
}

/// One step of a merge: `from` merged into the tree so far, against `base`.
struct Step {
    /// What is merged in, for messages.
    label: String,
    base: Manifest,
    from: Manifest,
}

/// A merge named on the command line.
struct Target {
    line: ContributionId,
    parents: Vec<ContributionId>,
    proposals: Vec<ContributionId>,
    start: Manifest,
    steps: Vec<Step>,
    /// How `resolve` names this merge.
    selector: String,
    /// What is merged, for the listing's JSON.
    about: Value,
    /// The target line's integrators, under its current policy.
    integrators: Vec<AgentId>,
    /// The resolutions this merge applies on its own: those its integrators wrote.
    applied: Resolutions,
}

/// The target line's integrators under its current policy, and the resolutions they wrote.
fn integrators_of(s: &Store, ix: &Index, lines: &Lines, line: &ContributionId) -> Result<(Vec<AgentId>, Resolutions)> {
    let integrators = match lines.current_versions(line).as_slice() {
        [one] => lines.version(one).expect("a current version").policy.integrators.clone(),
        many => {
            let ids: Vec<String> = many.iter().map(ContributionId::short).collect();
            bail!("line {}'s policy has diverged into {}; its author settles it first", line.short(), ids.join(", "))
        }
    };
    let g = s.graph();
    let applied = ix.resolutions().only(|id| g.get(id).is_some_and(|c| integrators.contains(&c.author())));
    Ok((integrators, applied))
}

/// `line merge <from> --into <into>`, and resolve's --into and --from.
fn lines_target(s: &Store, lines: &Lines, ix: &Index, into: &str, from: &str, base: Option<&str>) -> Result<Target> {
    let into_line = resolve_line(s, lines, Some(into), "--into")?;
    let from_line = resolve_line(s, lines, Some(from), "the line merged in")?;
    ensure!(into_line != from_line, "a line can't be merged into itself");
    let head = |line: &ContributionId| -> Result<ContributionId> {
        match lines.heads(line).as_slice() {
            [one] => Ok(*one),
            [] => bail!("line {} has no decision yet", line.short()),
            heads => {
                let ids: Vec<String> = heads.iter().map(ContributionId::short).collect();
                bail!("line {} is forked, with heads {}; settle the fork first", line.short(), ids.join(", "))
            }
        }
    };
    let manifest =
        |d: &ContributionId| held_manifest(ix, &lines.decision(d).expect("a decision").decision.manifest).cloned();
    let (integrators, applied) = integrators_of(s, ix, lines, &into_line)?;
    let from_head = head(&from_line)?;
    if lines.heads(&into_line).is_empty() {
        // A line with no decision yet starts from the other's head, as it is.
        ensure!(base.is_none(), "--base: line {} has no decision yet, so there is nothing to merge against", into);
        return Ok(Target {
            line: into_line,
            parents: vec![from_head],
            proposals: vec![],
            start: manifest(&from_head)?,
            steps: vec![],
            selector: format!("--into {into} --from {from}"),
            about: json!({"into": into_line, "into_head": Value::Null, "from": from_line, "from_head": from_head,
                          "base": Value::Null}),
            integrators,
            applied,
        });
    }
    let into_head = head(&into_line)?;
    if lines.decision_ancestry(&into_head).contains(&from_head) {
        bail!("nothing to merge: {}'s head {} is already in {}'s history", from, from_head.short(), into);
    }
    // --base only chooses among the merge bases; it never names another tree (finding 506c2ad3283b).
    let bases = lines.merge_bases(&into_head, &from_head);
    let listed = |ids: &[ContributionId]| ids.iter().map(ContributionId::short).collect::<Vec<_>>().join(", ");
    let base = match (base, bases.as_slice()) {
        (_, []) => bail!(
            "the heads {} and {} share no decision, so there is no merge base and they can't be merged; record the \
             tree you want with integrate",
            into_head.short(),
            from_head.short()
        ),
        (None, [one]) => *one,
        (None, many) => bail!(
            "the heads {} and {} have {} merge bases (a criss-cross): {}; choose one with --base",
            into_head.short(),
            from_head.short(),
            many.len(),
            listed(many)
        ),
        (Some(b), many) => {
            let id = decision_id(s, lines, b, "--base")?;
            if !many.contains(&id) {
                bail!("--base: {} isn't a merge base of the heads; the merge bases are {}", id.short(), listed(many));
            }
            id
        }
    };
    let mut parents = vec![into_head, from_head];
    parents.sort();
    Ok(Target {
        line: into_line,
        parents,
        proposals: vec![],
        start: manifest(&into_head)?,
        steps: vec![Step {
            label: format!("line {from} at {}", from_head.short()),
            base: manifest(&base)?,
            from: manifest(&from_head)?,
        }],
        selector: if bases.len() > 1 {
            format!("--into {into} --from {from} --base {base}")
        } else {
            format!("--into {into} --from {from}")
        },
        about: json!({"into": into_line, "into_head": into_head, "from": from_line, "from_head": from_head, "base": base}),
        integrators,
        applied,
    })
}

/// `integrate --proposals`, and resolve's --proposals.
fn proposals_target(s: &Store, lines: &Lines, ix: &Index, line: Option<&str>, given: &[String]) -> Result<Target> {
    let mut ids = Vec::new();
    for p in given {
        let id = s.graph().resolve_prefix(p).with_context(|| format!("--proposals: {p:?}"))?;
        let Some(record) = lines.proposal(&id) else {
            let c = s.graph().get(&id).expect("resolved ids exist");
            bail!("--proposals: {} is a {}, not a proposal", id.short(), kind_of(c));
        };
        if let Err(why) = &record.verdict {
            bail!("--proposals: proposal {} is invalid: {why}", id.short());
        }
        ensure!(!ids.contains(&id), "--proposals: {} is given twice", id.short());
        ids.push(id);
    }
    let first = ids.first().context("--proposals needs at least one proposal")?;
    let line = match line {
        Some(l) => resolve_line(s, lines, Some(l), "--line")?,
        None => lines.proposal(first).expect("checked").proposal.line,
    };
    for id in &ids {
        let on = lines.proposal(id).expect("checked").proposal.line;
        ensure!(on == line, "--proposals: proposal {} is for line {}, not {}", id.short(), on.short(), line.short());
    }
    let head = match lines.heads(&line).as_slice() {
        [one] => *one,
        heads => {
            let ids: Vec<String> = heads.iter().map(ContributionId::short).collect();
            bail!("line {} is forked, with heads {}; settle the fork first", line.short(), ids.join(", "))
        }
    };
    let decision_manifest =
        |d: &ContributionId| held_manifest(ix, &lines.decision(d).expect("a decision").decision.manifest).cloned();
    let mut steps = Vec::new();
    for id in &ids {
        let p = &lines.proposal(id).expect("checked").proposal;
        steps.push(Step {
            label: format!("proposal {}", id.short()),
            base: decision_manifest(&p.base)?,
            from: held_manifest(ix, &p.manifest)?.clone(),
        });
    }
    let listed: Vec<String> = ids.iter().map(ContributionId::to_string).collect();
    let (integrators, applied) = integrators_of(s, ix, lines, &line)?;
    Ok(Target {
        line,
        parents: vec![head],
        proposals: ids.clone(),
        start: decision_manifest(&head)?,
        steps,
        selector: format!("--line {line} --proposals {}", listed.join(" ")),
        about: json!({"line": line, "head": head, "proposals": ids}),
        integrators,
        applied,
    })
}

/// Builds the merge, step by step; where it stops, the step and why.
fn build(ix: &Index, target: &Target) -> Result<Resolved, (usize, MergeError)> {
    let mut tree = target.start.clone();
    let mut used = Vec::new();
    for (i, step) in target.steps.iter().enumerate() {
        let merged = files::three_way(&step.base, &tree, &step.from);
        let resolved = files::resolve(&merged, &target.applied, |h| ix.file_size(h)).map_err(|e| (i, e))?;
        tree = resolved.manifest;
        used.extend(resolved.used);
    }
    used.sort();
    used.dedup();
    Ok(Resolved { manifest: tree, used })
}

fn value_json(v: Option<FileValue>) -> Value {
    v.map_or(Value::Null, file_value_json)
}

fn value_text(v: Option<FileValue>) -> String {
    v.map_or_else(|| "absent".to_owned(), |v| format!("{} {:o}", v.hash, v.mode.bits()))
}

/// Writes the merge's manifest and decision, or lists what stops it.
#[allow(clippy::too_many_arguments)]
fn write_merge(
    s: &mut Store,
    agent: Option<&str>,
    ix: &Index,
    lines: &Lines,
    target: Target,
    note: String,
    dry_run: bool,
    run: Option<&checks::Run>,
    out: Output,
) -> Result<ExitCode> {
    let resolved = match build(ix, &target) {
        Ok(r) => r,
        Err((step, MergeError::Unresolved(conflicts))) => {
            let label = &target.steps[step].label;
            let names = Names::load(s)?;
            let offered = |c: &Conflict| -> Vec<(ContributionId, AgentId, Option<FileValue>)> {
                ix.resolutions()
                    .for_key(&c.key)
                    .iter()
                    .filter_map(|(id, r)| {
                        let author = s.graph().get(id)?.author();
                        (!target.integrators.contains(&author)).then_some((*id, author, r.result))
                    })
                    .collect()
            };
            if out.json {
                let rows: Vec<Value> = conflicts
                    .iter()
                    .map(|c| {
                        let offered: Vec<Value> = offered(c)
                            .iter()
                            .map(|(id, author, result)| {
                                json!({"resolution": id, "author": author.to_string(),
                                       "author_name": names.display(author), "result": value_json(*result)})
                            })
                            .collect();
                        json!({"path": c.key.path, "base": value_json(c.key.base), "into": value_json(c.ours),
                               "from": value_json(c.theirs), "offered": offered})
                    })
                    .collect();
                print_json(&json!({"merge": target.about, "step": label, "conflicts": rows, "manifest": Value::Null,
                                   "decision": Value::Null}));
            } else {
                println!("merging {label}:");
                for c in &conflicts {
                    println!(
                        "conflict {}: base {} into {} from {}",
                        crate::terminal::printable(&c.key.path),
                        value_text(c.key.base),
                        value_text(c.ours),
                        value_text(c.theirs)
                    );
                    for (id, author, result) in offered(c) {
                        println!(
                            "  offered: resolution {id} by {}, result {}; adopt it with --take {id}",
                            crate::terminal::printable(&names.display(&author)),
                            value_text(result)
                        );
                    }
                }
                println!(
                    "settle each with `bracken weave resolve <path> {} --take into|from|<resolution>` (or --delete, or \
                     --file <f>)",
                    target.selector
                );
            }
            decide(false);
            bail!("{} conflicts have no resolution, so nothing was written", conflicts.len());
        }
        Err((step, e)) => bail!("merging {}: {e}; nothing was written", target.steps[step].label),
    };
    let hash = resolved.manifest.content_hash();
    let held = ix.manifest(&hash).is_some();
    let missing = ix.missing(&resolved.manifest);
    if !missing.is_empty() {
        bail!(
            "the merged tree isn't complete here: this replica lacks the files of {} paths, such as {:?}; nothing was written",
            missing.len(),
            missing[0]
        );
    }
    let decision = Decision {
        v: VERSION,
        line: target.line,
        manifest: hash,
        parents: target.parents.clone(),
        proposals: target.proposals.clone(),
        breakage: vec![],
        answers: vec![],
        resolutions: resolved.used.clone(),
        goals: vec![],
        source: None,
        note,
    };
    let names = Names::load(s)?;
    let used = used_resolutions(s, &resolved.used, &names);
    if dry_run {
        if out.json {
            print_json(&json!({"merge": target.about, "conflicts": [], "manifest": hash.to_string(),
                               "manifest_new": !held, "resolutions": resolved.used, "used": used,
                               "parents": decision.parents, "decision": Value::Null}));
        } else {
            print_used(&used);
            let state = if held { "held" } else { "new" };
            println!("manifest {hash} ({state}; no conflicts left; dry run, nothing written)");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let who = signer(s, agent)?;
    match lines.judge_new(s.graph(), ix, who.agent_id(), &decision) {
        Ok(_) => {}
        Err(Unqualified::ManifestNotHeld(_)) if !held => {}
        Err(why) => bail!("the decision wouldn't qualify, so nothing was written: {}", refusal(&why)),
    }
    let (_, failure) = write_tree(s, &who, &[], &resolved.manifest, held)?;
    if let Some(e) = failure {
        decide(false);
        return Err(e.context("nothing was written"));
    }
    let mut results = Vec::new();
    if let Some(run) = run {
        let recorded = checks::run_for_integrate(s, &who, run, &target.line, &hash, out.json)?;
        results = recorded.results;
        if let Some(e) = recorded.failure {
            decide(false);
            print_tree_written(Some(hash), &[], &results, out);
            return Err(e.context("the candidate was written, and no decision"));
        }
    }
    let ix = Index::build(s.graph());
    let lines = Lines::build(s.graph(), &ix);
    if let Err(why) = lines.judge_new(s.graph(), &ix, who.agent_id(), &decision) {
        decide(false);
        print_tree_written(Some(hash), &[], &results, out);
        bail!("the decision wouldn't qualify, so none was written (the candidate stays): {}", refusal(&why));
    }
    let id = match write_record(s, &who, &decision) {
        Ok(id) => id,
        Err(e) => {
            decide(false);
            print_tree_written(Some(hash), &[], &results, out);
            return Err(e.context("the candidate was written, but not the decision"));
        }
    };
    if out.json {
        print_json(&json!({"manifest": hash.to_string(), "manifest_new": !held, "decision": id, "line": target.line,
                           "parents": decision.parents, "proposals": decision.proposals,
                           "resolutions": decision.resolutions, "used": used, "results": results}));
    } else {
        println!("manifest {hash}");
        print_used(&used);
        println!("decision {id}");
    }
    Ok(ExitCode::SUCCESS)
}

/// The resolutions a merge used, each with its author, path and result, as JSON rows.
fn used_resolutions(s: &Store, used: &[ContributionId], names: &Names) -> Vec<Value> {
    used.iter()
        .filter_map(|id| {
            let c = s.graph().get(id)?;
            let Body::Artifact { content, .. } = c.body() else { return None };
            let r = Resolution::decode(content).ok()?;
            Some(json!({"resolution": id, "author": c.author().to_string(), "author_name": names.display(&c.author()),
                        "path": r.path, "result": value_json(r.result)}))
        })
        .collect()
}

fn print_used(used: &[Value]) {
    for u in used {
        let result = match &u["result"] {
            Value::Null => "deleted".to_owned(),
            v => format!("{} {}", v["hash"].as_str().unwrap_or_default(), v["mode"].as_str().unwrap_or_default()),
        };
        let line = format!(
            "uses resolution {} by {}: {} {result}",
            u["resolution"].as_str().unwrap_or_default(),
            u["author_name"].as_str().unwrap_or_default(),
            u["path"].as_str().unwrap_or_default()
        );
        println!("{}", crate::terminal::printable(&line));
    }
}

/// The first conflict at `path` that no integrator's resolution settles, and the step it is in. A step
/// whose conflict there is settled is applied and passed (finding 90de879d935c). With none unsettled, the
/// last settled one, as a conflict may get another resolution (decision 91c12b4da2ad).
fn conflict_at(ix: &Index, target: &Target, path: &str) -> Result<(String, Conflict)> {
    let mut tree = target.start.clone();
    let mut settled = None;
    for step in &target.steps {
        let merged = files::three_way(&step.base, &tree, &step.from);
        if let Some(c) = merged.conflicts.iter().find(|c| c.key.path == path) {
            if target.applied.for_key(&c.key).is_empty() {
                return Ok((step.label.clone(), c.clone()));
            }
            settled = Some((step.label.clone(), c.clone()));
        }
        match files::resolve(&merged, &target.applied, |h| ix.file_size(h)) {
            Ok(r) => tree = r.manifest,
            Err(MergeError::Unresolved(cs)) => {
                if let Some(found) = settled {
                    return Ok(found);
                }
                let paths: Vec<&str> = cs.iter().map(|c| c.key.path.as_str()).collect();
                bail!(
                    "merging {} stops at conflicts at {}, and none at {path:?}; resolve those first",
                    step.label,
                    paths.join(", ")
                );
            }
            Err(e) => bail!("merging {}: {e}", step.label),
        }
    }
    settled.context(format!("this merge has no conflict at {path:?}"))
}

pub(crate) fn resolve(s: &mut Store, agent: Option<&str>, args: ResolveArgs) -> Result<ExitCode> {
    let ResolveArgs { path, into, from, base, proposals, line, take, delete, file, mode, dry_run, repo, out } = args;
    let ix = Index::build(s.graph());
    if !dry_run {
        require_marker(&ix)?;
    }
    let lines = Lines::build(s.graph(), &ix);
    let target = match (&into, &from) {
        (Some(into), Some(from)) => lines_target(s, &lines, &ix, into, from, base.as_deref())?,
        _ => proposals_target(s, &lines, &ix, line.as_deref(), &proposals)?,
    };
    let (label, conflict) = conflict_at(&ix, &target, &path)?;
    let mode = mode.map(|m| match m {
        ModeArg::File => Mode::File,
        ModeArg::Executable => Mode::Executable,
    });
    let mut new_file = None;
    let result = match (take, delete, &file) {
        (Some(take), _, _) => {
            let taken = match take.as_str() {
                "into" => conflict.ours,
                "from" => conflict.theirs,
                given => adopted(s, &ix, &conflict, given)?,
            };
            match (taken, mode) {
                (None, Some(_)) => bail!("--mode: that side deleted the file, so it has no mode"),
                (Some(v), Some(_)) if v.mode == Mode::Symlink => bail!("--mode: that side is a symlink"),
                (Some(v), Some(m)) => Some(FileValue { hash: v.hash, mode: m }),
                (v, None) => v,
            }
        }
        (None, true, _) => None,
        (None, false, Some(f)) => {
            let (bytes, executable) = resolution_file(s, &repo, f)?;
            let hash = ContentHash::of(&bytes);
            if !ix.holds_file(&hash) {
                new_file = Some(bytes);
            }
            let default = if executable { Mode::Executable } else { Mode::File };
            Some(FileValue { hash, mode: mode.unwrap_or(default) })
        }
        (None, false, None) => unreachable!("clap requires one of --take, --delete and --file"),
    };
    let resolution = Resolution::new(path.clone(), conflict.key.base, conflict.key.sides, result);
    resolution.validate().context("the resolution isn't valid, and nothing was written")?;
    if dry_run {
        if out.json {
            print_json(&json!({"path": path, "merging": label, "base": value_json(resolution.base),
                               "sides": [value_json(resolution.sides[0]), value_json(resolution.sides[1])],
                               "result": value_json(result), "file_new": new_file.is_some(),
                               "resolution": Value::Null}));
        } else {
            println!(
                "resolution of {}: base {}, sides {} and {}, result {} (dry run, nothing written)",
                crate::terminal::printable(&path),
                value_text(resolution.base),
                value_text(resolution.sides[0]),
                value_text(resolution.sides[1]),
                value_text(result)
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let who = signer(s, agent)?;
    let mut file_id = None;
    if let (Some(bytes), Some(value)) = (new_file, result) {
        let body = Body::Artifact {
            name: path.clone(),
            media_type: import::FileVersion::MEDIA_TYPE.to_owned(),
            content: bytes,
        };
        file_id =
            Some((contribute(s, &who, body).context("writing the file version; nothing was written")?, value.hash));
    }
    let id = match write_record(s, &who, &resolution) {
        Ok(id) => id,
        Err(e) => {
            decide(false);
            if let Some((_, hash)) = file_id {
                print_tree_written(None, &[(hash, path.clone())], &[], out);
            }
            return Err(e.context("the resolution wasn't written"));
        }
    };
    if out.json {
        let file = file_id.map(|(id, hash)| json!({"id": id, "hash": hash.to_string(), "path": path}));
        print_json(&json!({"file": file, "resolution": id, "path": path, "result": value_json(result)}));
    } else {
        if let Some((_, hash)) = file_id {
            println!("file {hash} {}", crate::terminal::printable(&path));
        }
        println!("resolution {id}");
    }
    Ok(ExitCode::SUCCESS)
}

/// The result of the resolution `given` names, which must settle exactly this conflict.
fn adopted(s: &Store, ix: &Index, conflict: &Conflict, given: &str) -> Result<Option<FileValue>> {
    let id =
        s.graph().resolve_prefix(given).with_context(|| format!("--take: {given:?} is into, from, or a resolution"))?;
    if let Some((_, r)) = ix.resolutions().for_key(&conflict.key).iter().find(|(r, _)| *r == id) {
        return Ok(r.result);
    }
    let c = s.graph().get(&id).expect("resolved ids exist");
    match kind_of(c).as_str() {
        "resolution" => {
            bail!("--take: resolution {} settles another conflict, not this one at {:?}", id.short(), conflict.key.path)
        }
        kind => bail!("--take: {} is a {kind}, not a resolution", id.short()),
    }
}

/// The bytes of `resolve --file`, and whether it is executable. Its real path must
/// be inside the work tree of the git checkout at `repo`, outside `.git` and every
/// `.bracken*` root entry; that path must pass the deny-list, and its bytes the
/// content scan (docs/design/weave.md, section 3).
fn resolution_file(s: &Store, repo: &Path, file: &Path) -> Result<(Vec<u8>, bool)> {
    let shown = file.display();
    let real = std::fs::canonicalize(file).with_context(|| format!("--file {shown}"))?;
    let top = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git")?;
    ensure!(top.status.success(), "--file needs a git checkout: {}", String::from_utf8_lossy(&top.stderr).trim());
    let top = String::from_utf8(top.stdout).context("the checkout's path isn't UTF-8")?;
    let top = std::fs::canonicalize(top.trim_end_matches('\n')).context("resolving the checkout's path")?;
    let Ok(relative) = real.strip_prefix(&top) else {
        bail!("--file {shown}: it is outside the checkout {}; give a file inside its work tree", top.display());
    };
    let relative = relative.to_str().with_context(|| format!("--file {shown}: its path isn't UTF-8"))?;
    let components: Vec<String> = relative.split('/').map(path::fold).collect();
    if components.iter().any(|c| c == ".git") {
        bail!("--file {shown}: it is inside a .git directory");
    }
    if components.first().is_some_and(|c| c.starts_with(".bracken")) {
        bail!("--file {shown}: it is inside a .bracken root entry");
    }
    if let Some(rule) = secrets::deny_listed(relative) {
        bail!("--file {shown}: {relative:?} looks like a secret: {rule}; nothing was written");
    }
    // One open, beneath the checkout and following no symlink, so the checks above and the bytes read
    // are about the same file (finding 0bbc434b2588).
    #[cfg(unix)]
    let read = import::read_beneath(&top, relative).with_context(|| format!("--file {shown}"))?;
    #[cfg(not(unix))]
    let read: import::ReadFile = bail!("--file {shown}: reading a file without following symlinks needs a unix system");
    if let Some(rule) = secrets::scan(&read.bytes, &known_keys(s, repo)?) {
        bail!("--file {shown}: its content looks like a secret: {rule}; nothing was written");
    }
    Ok((read.bytes, read.executable))
}
