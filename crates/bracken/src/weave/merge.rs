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
//!
//! `resolve --markers <file>` writes nothing to the Ground: for a text
//! conflict, it writes the merge with each conflicting region between
//! diff3-style markers to a new file, to edit and then record with `resolve
//! --file` (task 7cbd0a0ce7d0, design 68b57f9aed6d section 3).

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
#[command(group(ArgGroup::new("result").required(true).args(["take", "delete", "file", "markers"])))]
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
    /// Write nothing to the Ground: write this new file with the conflict's lines merged and each
    /// conflicting region between markers (`<<<<<<< into`, `||||||| base`, `=======`, `>>>>>>> from`), to
    /// edit and then give to --file. Only for a conflict whose base and sides are all text.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["mode", "dry_run"])]
    markers: Option<PathBuf>,
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
            let lines = super::lines_for(s, &ix, &[Some(&line)]);
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
            let lines = super::lines_for(s, &ix, &[Some(&into), Some(&from), base.as_deref()]);
            let target = lines_target(s, &lines, &ix, &into, &from, base.as_deref())?;
            write_merge(s, agent, &ix, &lines, target, note, dry_run, None, None, &Default::default(), out)
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
    let lines = super::lines_for(s, &ix, &[line.as_deref(), base.as_deref()]);
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

/// The git commit a merge mirrors (`integrate --mirrors`), and its tree as import would record it.
struct Mirror {
    commit: String,
    manifest: Manifest,
}

/// `weave integrate --proposals`, and with `mirrors` (a revision and the repository it's in), the git
/// commit the merge must equal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn integrate_proposals(
    s: &mut Store,
    agent: Option<&str>,
    line: Option<&str>,
    proposals: &[String],
    mirrors: Option<(&str, &Path)>,
    note: String,
    run: Option<&checks::Run>,
    decide_args: &super::review::DecideArgs,
    out: Output,
) -> Result<ExitCode> {
    let ix = Index::build(s.graph());
    require_marker(&ix)?;
    let named: Vec<Option<&str>> = proposals.iter().map(|p| Some(p.as_str())).chain([line]).collect();
    let lines = super::lines_for(s, &ix, &named);
    let target = proposals_target(s, &lines, &ix, line, proposals)?;
    let mirror = match mirrors {
        None => None,
        Some((rev, repo)) => {
            let plan = import::plan(repo, &Source::Rev(rev.to_owned()), &known_keys(s, repo)?)?;
            let Some(manifest) = plan.manifest() else {
                bail!(
                    "--mirrors {rev}: import refuses {} of its files, so it can't be mirrored; nothing was written",
                    plan.refused.len()
                );
            };
            let commit = plan.commit.clone().context("import names the commit it read")?;
            Some(Mirror { commit, manifest })
        }
    };
    write_merge(s, agent, &ix, &lines, target, note, false, run, mirror, decide_args, out)
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

/// Resolves one step's merge: resolutions first, then the line merge (task 7cbd0a0ce7d0), reading a
/// file's bytes from the replica or from what an earlier step's line merge made.
fn resolve_step(
    g: &manyroot::Graph,
    ix: &Index,
    merged: &files::Merged,
    applied: &Resolutions,
    made: &Made,
) -> Result<Resolved, MergeError> {
    // A resolution may take a version an earlier step merged by lines, held nowhere yet (finding
    // fa1c271c4db7): it is written with the tree.
    let size = |h: &ContentHash| ix.file_size(h).or_else(|| made.get(h).map(|f| f.bytes.len() as u64));
    files::resolve_lines(merged, applied, size, |h| file_bytes(g, ix, made, h))
}

/// The file versions a merge's line merges made, by hash: the tree so far may hold them before any is
/// written.
type Made = BTreeMap<ContentHash, import::FileVersion>;

/// The bytes of the file version `h`, from the replica or from what the line merge made.
fn file_bytes<'a>(g: &'a manyroot::Graph, ix: &'a Index, made: &'a Made, h: &ContentHash) -> Option<&'a [u8]> {
    ix.file(g, h).or_else(|| made.get(h).map(|f| f.bytes.as_slice()))
}

/// Builds the merge, step by step; where it stops, the step, why, and what the line merge made before
/// it. Its `merged_lines` are the paths a step merged by lines whose version the final tree still holds,
/// in path order, named by path, never by content (finding f34ae4b848c0). A resolution may keep such a
/// version, as `--take into` after a line merge does.
fn build(g: &manyroot::Graph, ix: &Index, target: &Target) -> Result<Resolved, (usize, MergeError, Made)> {
    let mut tree = target.start.clone();
    let mut used = Vec::new();
    let mut made = BTreeMap::new();
    let mut by_path: BTreeMap<String, import::FileVersion> = BTreeMap::new();
    for (i, step) in target.steps.iter().enumerate() {
        let merged = files::three_way(&step.base, &tree, &step.from);
        let resolved = match resolve_step(g, ix, &merged, &target.applied, &made) {
            Ok(r) => r,
            Err(e) => return Err((i, e, made)),
        };
        tree = resolved.manifest;
        used.extend(resolved.used);
        for f in resolved.merged_lines {
            made.insert(f.hash, f.clone());
            by_path.insert(f.name.clone(), f);
        }
    }
    used.sort();
    used.dedup();
    let merged_lines =
        tree.entries.iter().filter_map(|e| by_path.get(&e.path).filter(|f| f.hash == e.hash).cloned()).collect();
    Ok(Resolved { manifest: tree, used, merged_lines })
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
    mirror: Option<Mirror>,
    decide_args: &super::review::DecideArgs,
    out: Output,
) -> Result<ExitCode> {
    let resolved = match build(s.graph(), ix, &target) {
        Ok(r) => r,
        Err((step, MergeError::Unresolved(conflicts), made)) => {
            let label = &target.steps[step].label;
            let names = Names::load(s)?;
            // Where a text conflict's lines conflict, for the versions this replica holds or an earlier
            // step's line merge made.
            let text_regions = |c: &Conflict| files::text_conflicts(c, |h| file_bytes(s.graph(), ix, &made, h));
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
                               "from": value_json(c.theirs), "offered": offered,
                               "text_conflicts": text_regions(c).map(|r| r.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>())})
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
                    match text_regions(c) {
                        Some(r) if r.is_empty() => println!("  its lines merge; its modes don't"),
                        Some(r) => {
                            let shown: Vec<String> = r
                                .iter()
                                .map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") })
                                .collect();
                            println!("  {} conflicting regions, at base lines {}", r.len(), shown.join(", "));
                        }
                        None => {}
                    }
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
                     --file <f>; --markers <f> writes a text conflict out to edit)",
                    target.selector
                );
            }
            decide(false);
            bail!("{} conflicts have no resolution, so nothing was written", conflicts.len());
        }
        Err((step, e, _)) => bail!("merging {}: {e}; nothing was written", target.steps[step].label),
    };
    let hash = resolved.manifest.content_hash();
    // Mirroring a git commit, the merge must come out as git's tree, or nothing is written: never forced.
    if let Some(m) = &mirror
        && m.manifest.content_hash() != hash
    {
        let differs = diff::manifests(&m.manifest, &resolved.manifest);
        if out.json {
            let rows: Vec<Value> = differs.iter().map(|d| json!({"path": d.path, "change": d.kind.as_str()})).collect();
            print_json(&json!({"merge": target.about, "mirrors": m.commit, "differs": rows,
                               "manifest": hash.to_string(), "decision": Value::Null}));
        } else {
            println!("the merge differs from {}:", m.commit);
            for d in &differs {
                println!("{} {}", d.kind.as_str(), crate::terminal::printable(&d.path));
            }
        }
        decide(false);
        bail!("the merged tree differs from {}'s at {} paths, so nothing was written", m.commit, differs.len());
    }
    let held = ix.manifest(&hash).is_some();
    // The line merge's file versions are written with the tree, so only the others must be held.
    let made: std::collections::BTreeSet<&str> = resolved.merged_lines.iter().map(|f| f.name.as_str()).collect();
    let missing: Vec<String> =
        ix.missing(&resolved.manifest).into_iter().filter(|p| !made.contains(p.as_str())).collect();
    if !missing.is_empty() {
        bail!(
            "the merged tree isn't complete here: this replica lacks the files of {} paths, such as {:?}; nothing was written",
            missing.len(),
            missing[0]
        );
    }
    let mut decision = Decision {
        v: VERSION,
        line: target.line,
        manifest: hash,
        parents: target.parents.clone(),
        proposals: target.proposals.clone(),
        breakage: vec![],
        answers: vec![],
        resolutions: resolved.used.clone(),
        goals: vec![],
        source: mirror.map(|m| bracken_tree::format::Source { git: m.commit }),
        note,
    };
    decide_args.apply(s, ix, lines, &mut decision)?;
    let names = Names::load(s)?;
    let used = used_resolutions(s, &resolved.used, &names);
    let line_merged: Vec<&str> = resolved.merged_lines.iter().map(|f| f.name.as_str()).collect();
    if dry_run {
        if out.json {
            print_json(&json!({"merge": target.about, "conflicts": [], "manifest": hash.to_string(),
                               "manifest_new": !held, "resolutions": resolved.used, "used": used,
                               "merged_lines": line_merged, "parents": decision.parents, "decision": Value::Null}));
        } else {
            print_used(&used);
            print_merged_lines(&line_merged);
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
    // The line merge's file versions this replica lacks, once each, as import writes them.
    let mut seen = std::collections::BTreeSet::new();
    let new: Vec<&import::FileVersion> =
        resolved.merged_lines.iter().filter(|f| !ix.holds_file(&f.hash) && seen.insert(f.hash)).collect();
    let (_, failure) = write_tree(s, &who, &new, &resolved.manifest, held)?;
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
    // Its own line and its parents' lines, as rule 3 judges its parents.
    let lines = super::judged_for(s, &ix, |l| {
        let parents = decision.parents.iter().filter_map(|p| l.decision(p)).map(|p| p.decision.line);
        std::iter::once(decision.line).chain(parents).collect()
    });
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
                           "resolutions": decision.resolutions, "used": used, "merged_lines": line_merged,
                           "results": results}));
    } else {
        println!("manifest {hash}");
        print_used(&used);
        print_merged_lines(&line_merged);
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

/// The paths a merge merged line by line, which no one wrote as a whole (task 7cbd0a0ce7d0).
fn print_merged_lines(paths: &[&str]) {
    for path in paths {
        println!("merged lines: {}", crate::terminal::printable(path));
    }
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

/// The first conflict at `path` that neither an integrator's resolution nor the line merge settles, and
/// the step it is in, with what the line merge made in the steps before. A step whose conflict there is
/// settled is applied and passed (finding 90de879d935c). With none unsettled, the last settled one, as a
/// conflict may get another resolution (decision 91c12b4da2ad), and one may overrule a line merge.
fn conflict_at(g: &manyroot::Graph, ix: &Index, target: &Target, path: &str) -> Result<(String, Conflict, Made)> {
    let mut tree = target.start.clone();
    let mut settled = None;
    let mut made = BTreeMap::new();
    for step in &target.steps {
        let merged = files::three_way(&step.base, &tree, &step.from);
        let here = merged.conflicts.iter().find(|c| c.key.path == path);
        match resolve_step(g, ix, &merged, &target.applied, &made) {
            Ok(r) => {
                if let Some(c) = here {
                    settled = Some((step.label.clone(), c.clone(), made.clone()));
                }
                tree = r.manifest;
                made.extend(r.merged_lines.into_iter().map(|f| (f.hash, f)));
            }
            Err(MergeError::Unresolved(cs)) => {
                if let Some(c) = cs.iter().find(|c| c.key.path == path) {
                    return Ok((step.label.clone(), c.clone(), made));
                }
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
            Err(e) => match here {
                Some(c) if target.applied.for_key(&c.key).is_empty() => {
                    return Ok((step.label.clone(), c.clone(), made));
                }
                _ => bail!("merging {}: {e}", step.label),
            },
        }
    }
    settled.context(format!("this merge has no conflict at {path:?}"))
}

pub(crate) fn resolve(s: &mut Store, agent: Option<&str>, args: ResolveArgs) -> Result<ExitCode> {
    let ResolveArgs { path, into, from, base, proposals, line, take, delete, file, markers, mode, dry_run, repo, out } =
        args;
    let ix = Index::build(s.graph());
    if !dry_run && markers.is_none() {
        require_marker(&ix)?;
    }
    let named: Vec<Option<&str>> = [into.as_deref(), from.as_deref(), base.as_deref(), line.as_deref()]
        .into_iter()
        .chain(proposals.iter().map(|p| Some(p.as_str())))
        .collect();
    let lines = super::lines_for(s, &ix, &named);
    let target = match (&into, &from) {
        (Some(into), Some(from)) => lines_target(s, &lines, &ix, into, from, base.as_deref())?,
        _ => proposals_target(s, &lines, &ix, line.as_deref(), &proposals)?,
    };
    let (label, conflict, made) = conflict_at(s.graph(), &ix, &target, &path)?;
    if let Some(to) = markers {
        return write_markers(s, &ix, &made, &label, &conflict, &repo, &to, out);
    }
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
        (None, false, None) => unreachable!("clap requires one of --take, --delete, --file and --markers"),
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

/// `resolve --markers`: writes the conflict's marked merge to the new file `to`, outside the Ground.
#[allow(clippy::too_many_arguments)]
fn write_markers(
    s: &Store,
    ix: &Index,
    made: &Made,
    label: &str,
    conflict: &Conflict,
    repo: &Path,
    to: &Path,
    out: Output,
) -> Result<ExitCode> {
    let path = &conflict.key.path;
    let shown = to.display();
    // Checked before anything is written, so --json can always name it (finding 4c4e1914f2bf).
    let named = to.to_str().with_context(|| format!("--markers {shown}: its path isn't UTF-8"))?;
    let content = |h: &ContentHash| file_bytes(s.graph(), ix, made, h);
    let labels = bracken_tree::text_merge::Labels { ours: "into", base: "base", theirs: "from" };
    let (Some(regions), Some(bytes)) =
        (files::text_conflicts(conflict, content), files::conflict_markers(conflict, content, &labels))
    else {
        bail!(
            "--markers: the conflict at {path:?} isn't one of text: its base and both sides must be regular \
             files this replica holds, and text (at most 8 MiB, UTF-8, no NUL), with each side's diff within \
             the line merge's {}-line cap; settle it with --take, --delete or --file",
            bracken_tree::text_merge::MAX_DIFF_LINES
        );
    };
    // Only where --file will take it back (finding d49f362be57f), and never into the Ground. The file
    // doesn't exist yet, so its directory is resolved and its name joined.
    let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else {
        bail!("--markers {shown}: name a file");
    };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let real = std::fs::canonicalize(parent).with_context(|| format!("--markers {shown}"))?.join(name);
    let ground = std::fs::canonicalize(s.dir()).context("finding the Ground's directory")?;
    ensure!(!real.starts_with(&ground), "--markers {shown}: it is inside the Ground {}", ground.display());
    checkout_path(repo, &real, "--markers", &shown.to_string())?;
    // Executable where the sides' modes merge to executable, so --file records that mode back (finding
    // 445429f73583). Where they don't merge, --mode chooses.
    let (b, x, y) = (conflict.key.base, conflict.ours, conflict.theirs);
    let mode = match (b, x, y) {
        (Some(b), Some(x), Some(y)) => files::merge_mode(b.mode, x.mode, y.mode),
        _ => None,
    };
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, if mode == Some(Mode::Executable) { 0o755 } else { 0o644 });
    let mut f = match open.open(to) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("--markers {shown}: it already exists, and markers never overwrite a file; name a new one")
        }
        Err(e) => return Err(e).with_context(|| format!("--markers {shown}")),
    };
    std::io::Write::write_all(&mut f, &bytes).with_context(|| format!("--markers {shown}"))?;
    if out.json {
        let mode = mode.map(|m| format!("{:o}", m.bits()));
        print_json(&json!({"path": path, "merging": label, "markers": named, "mode": mode,
                           "text_conflicts": regions.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()}));
    } else {
        let what = if regions.is_empty() {
            "no conflicting regions, as its lines merge".to_owned()
        } else {
            let at: Vec<String> = regions.iter().map(|(a, b)| format!("{a}-{b}")).collect();
            format!("{} conflicting regions, at base lines {}", regions.len(), at.join(", "))
        };
        println!("markers {shown} for {} merging {label}: {what}", crate::terminal::printable(path));
        match mode {
            Some(m) => println!(
                "edit it, then settle the conflict with the same selector and --file {shown}; it is mode {:o}, \
                 as the sides' modes merge",
                m.bits()
            ),
            None => println!(
                "edit it, then settle the conflict with the same selector, --file {shown} and --mode, as the \
                 sides' modes conflict"
            ),
        }
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
    let (top, relative) = checkout_path(repo, &real, "--file", &shown.to_string())?;
    let relative = relative.as_str();
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

/// The checkout's top and `real`'s path beneath it, for `flag` (`--file`, or `--markers`, which writes
/// a file only where `--file` will read it): `real`, a resolved path, must be inside the work tree of
/// the git checkout at `repo`, outside `.git` and every `.bracken*` root entry, and pass the deny-list.
fn checkout_path(repo: &Path, real: &Path, flag: &str, shown: &str) -> Result<(PathBuf, String)> {
    let top = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git")?;
    ensure!(top.status.success(), "{flag} needs a git checkout: {}", String::from_utf8_lossy(&top.stderr).trim());
    let top = String::from_utf8(top.stdout).context("the checkout's path isn't UTF-8")?;
    let top = std::fs::canonicalize(top.trim_end_matches('\n')).context("resolving the checkout's path")?;
    let Ok(relative) = real.strip_prefix(&top) else {
        bail!("{flag} {shown}: it is outside the checkout {}; give a file inside its work tree", top.display());
    };
    let relative = relative.to_str().with_context(|| format!("{flag} {shown}: its path isn't UTF-8"))?;
    let components: Vec<String> = relative.split('/').map(path::fold).collect();
    if components.iter().any(|c| c == ".git") {
        bail!("{flag} {shown}: it is inside a .git directory");
    }
    if components.first().is_some_and(|c| c.starts_with(".bracken")) {
        bail!("{flag} {shown}: it is inside a .bracken root entry");
    }
    if let Some(rule) = secrets::deny_listed(relative) {
        bail!("{flag} {shown}: {relative:?} looks like a secret: {rule}; nothing was written");
    }
    Ok((top, relative.to_owned()))
}
