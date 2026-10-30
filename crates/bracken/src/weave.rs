//! `bracken weave`: Bracken's source trees on Weave: manifests, lines and
//! decisions, heads and forks (docs/design/weave.md, section 3; build steps 3
//! and 5). Policies, proposals and merges are in [`merge`] (step 6).
//!
//! Every write checks that the Ground holds a counted Weave marker, and prints
//! one labelled line per record (`manifest blake3:<hex>`, `file blake3:<hex>
//! <path>`, `decision <id>`), or named fields with `--json`. Every read has
//! `--json`. A content hash is always written `blake3:<hex>`; an option that
//! takes a manifest refuses a bare hex value, and names the kind of
//! contribution it names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bracken_tree::format::{Decision, Guardrails, Line, Manifest, Marker, Record, VERSION};
use bracken_tree::ground::{Index, Problem, kind_of};
use bracken_tree::import::{self, Exclusion, Plan, Source};
use bracken_tree::lines::{DecisionRecord, Lines, Unqualified};
use bracken_tree::materialize::materialize;
use bracken_tree::secrets::KnownKeys;
use bracken_tree::{ContentHash, Entry, diff};
use clap::Subcommand;
use manyroot::contribution::MAX_CONTRIBUTION_BYTES;
use manyroot::{Body, Contribution, ContributionId, Identity, Store};
use serde_json::{Value, json};

use super::{Output, contribute, decide, print_json, write_stdout};

mod merge;

#[derive(Subcommand)]
pub(crate) enum WeaveCommand {
    /// Create a Weave Ground at --dir: the Weave marker and a `main` line, signed with --as (created
    /// there). --dir must not exist yet and its parent must; refuses if the parent is inside a git checkout.
    Init {
        /// What this Weave Ground is for.
        #[arg(long, default_value = "dogfood")]
        purpose: String,
        #[command(flatten)]
        out: Output,
    },
    /// Record a git tree as file versions (only those new to the Ground) and one manifest. Takes git's
    /// view only: a commit's tree, or the index. Refuses, writing nothing, if anything looks like a secret.
    Import {
        /// The commit whose tree to import.
        #[arg(long, value_name = "REV", conflicts_with = "index")]
        from_git: Option<String>,
        /// Import the index: tracked and staged files, with their staged contents.
        #[arg(long)]
        index: bool,
        /// List what would be written, excluded and refused, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// The git repository to import from.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// Write a manifest's tree into an empty directory. Never follows a symlink; on a failure part way,
    /// removes what it wrote.
    Checkout {
        /// The manifest, by content hash: blake3:<hex>.
        #[arg(long, value_name = "HASH")]
        manifest: String,
        /// An empty directory to write into. (Not `dir`, which would clash with the global --dir.)
        #[arg(value_name = "DIR")]
        target: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// The per-file difference between two manifests or decisions, with a unified diff of each changed
    /// text file.
    Diff {
        /// The first manifest (blake3:<hex>) or decision (an id).
        a: String,
        /// The second manifest (blake3:<hex>) or decision (an id).
        b: String,
        #[command(flatten)]
        out: Output,
    },
    /// Compare a manifest with a git commit's tree: the same paths, modes and bytes, with the excluded
    /// root entries left out. Exits non-zero if they differ. Compares the line's head unless --at or
    /// --manifest names another.
    Verify {
        /// The commit to compare with.
        #[arg(long, value_name = "REV")]
        git: String,
        /// A decision, whose manifest to compare.
        #[arg(long, value_name = "DECISION", conflicts_with = "manifest")]
        at: Option<String>,
        /// The manifest: blake3:<hex>.
        #[arg(long, value_name = "HASH")]
        manifest: Option<String>,
        /// The line whose head to compare: a line's id or name. Defaults to the default line.
        #[arg(long, value_name = "LINE")]
        line: Option<String>,
        /// The git repository the commit is in.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// List Weave payloads that are invalid or break the ancestor rule, manifests not yet complete,
    /// markers past the one that counts, and policy versions ignored for citing no owner decision. Exits
    /// non-zero when it lists any.
    Check {
        #[command(flatten)]
        out: Output,
    },
    /// The line's head with its manifest, or every head of a forked line, in ascending order.
    Head {
        /// A line's id or name. Defaults to the default line.
        #[arg(long, value_name = "LINE")]
        line: Option<String>,
        #[command(flatten)]
        out: Output,
    },
    /// The line's decisions, newest first in causal order: manifest, parents, breakage and note, never
    /// a payload.
    Log {
        /// A line's id or name. Defaults to the default line.
        #[arg(long, value_name = "LINE")]
        line: Option<String>,
        /// How many decisions to print at most.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only decisions that aren't this decision or in its causal past.
        #[arg(long, value_name = "DECISION")]
        since: Option<String>,
        #[command(flatten)]
        out: Output,
    },
    /// git's HEAD, or its index, against the line's head: each file added, modified, deleted or with
    /// its mode changed. Writes nothing.
    Status {
        /// Compare the index instead of HEAD.
        #[arg(long)]
        index: bool,
        /// A line's id or name. Defaults to the default line.
        #[arg(long, value_name = "LINE")]
        line: Option<String>,
        /// Compare with this decision instead of the line's head (needed while it is forked).
        #[arg(long, value_name = "DECISION")]
        at: Option<String>,
        /// The git repository.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// Integrator: record a given tree as the line's next decision, listing every head of the line as a
    /// parent, so on a forked line it settles the fork. Imports the tree first (git's view, with
    /// import's refusals); refuses, writing no decision, if the decision wouldn't qualify.
    Integrate {
        /// A line's id or name. Defaults to the default line.
        #[arg(long, value_name = "LINE")]
        line: Option<String>,
        /// The commit whose tree to record (the default is HEAD); the decision names it as its source.
        #[arg(long, value_name = "REV", conflicts_with_all = ["index", "manifest"])]
        from_git: Option<String>,
        /// Record the index: tracked and staged files, with their staged contents.
        #[arg(long, conflicts_with = "manifest")]
        index: bool,
        /// Record a manifest this Ground already holds: blake3:<hex>.
        #[arg(long, value_name = "HASH")]
        manifest: Option<String>,
        /// Merge these proposals onto the line's head, in order, file by file against each one's base,
        /// with the resolutions this Ground holds. A conflict without one stops it, listed, and nothing is
        /// written. The line defaults to the proposals'.
        #[arg(long, value_name = "PROPOSAL", num_args = 1.., conflicts_with_all = ["from_git", "index", "manifest"])]
        proposals: Vec<String>,
        /// What the decision does, in words.
        #[arg(long, default_value = "")]
        note: String,
        /// The git repository to import from.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// Line policies, and merges between lines.
    Line {
        #[command(subcommand)]
        command: merge::LineCommand,
    },
    /// Record a tree (git's view, with import's refusals) and a proposal to integrate it onto a line,
    /// against a base decision: by default the line's head.
    Propose(merge::ProposeArgs),
    /// Write a resolution for the conflict at one path of a merge, named as `line merge` (--into, --from)
    /// or `integrate` (--proposals) names it.
    Resolve(merge::ResolveArgs),
}

/// `weave init`, which creates its Ground and so runs before any is opened.
pub(crate) fn init(dir: Option<&Path>, agent: Option<&str>, purpose: String, out: Output) -> Result<ExitCode> {
    let Some(dir) = dir else { bail!("weave init needs --dir (or BRACKEN_DIR): where to create the Weave Ground") };
    let Some(agent) = agent else { bail!("weave init needs --as: the identity that signs the marker and the line") };
    // Decision 7529dd229d6f: --dir is new and its parent exists. The parent's real
    // path is searched for .git, and a single mkdir makes the Ground directory, so
    // nothing already there (a symlink included) is written through.
    let absolute = std::path::absolute(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        bail!("{} doesn't end in a directory name; give the new Ground's path", dir.display());
    };
    let parent = match std::fs::canonicalize(parent) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("{} doesn't exist; weave init needs the parent of --dir to exist", parent.display())
        }
        Err(e) => return Err(e).with_context(|| format!("resolving {}", parent.display())),
    };
    if let Some(checkout) = parent.ancestors().find(|a| a.join(".git").exists()) {
        bail!(
            "{} is inside the git checkout {}; a Weave Ground lives outside every repository",
            dir.display(),
            checkout.display()
        );
    }
    let ground = parent.join(name);
    match std::fs::create_dir(&ground) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("{} already exists; weave init makes a new directory for its Ground", dir.display())
        }
        Err(e) => return Err(e).with_context(|| format!("creating {}", ground.display())),
    }
    let mut s = Store::init(&ground, "weave")?;
    let who = s.create_identity(agent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let identities = s.identities_dir();
        std::fs::set_permissions(&identities, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("making {} private", identities.display()))?;
    }
    let marker = Marker { v: VERSION, purpose, in_place: None };
    let line = Line {
        v: VERSION,
        name: "main".to_owned(),
        integrators: vec![who.agent_id()],
        validators: vec![],
        goal_setters: vec![who.agent_id()],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: None,
    };
    let marker_id = write_record(&mut s, &who, &marker)?;
    let line_id = match write_record(&mut s, &who, &line) {
        Ok(id) => id,
        Err(e) => {
            decide(false);
            if out.json {
                print_json(&json!({"ground": s.info().ground, "marker": marker_id}));
            } else {
                println!("marker {marker_id}");
            }
            return Err(e.context("the marker was written, but not the main line"));
        }
    };
    if out.json {
        print_json(&json!({"ground": s.info().ground, "dir": dir, "marker": marker_id, "line": line_id}));
    } else {
        println!("marker {marker_id}");
        println!("line {line_id} (main)");
    }
    Ok(ExitCode::SUCCESS)
}

fn write_record<R: Record>(s: &mut Store, who: &Identity, record: &R) -> Result<ContributionId> {
    let body = Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    };
    contribute(s, who, body)
}

pub(crate) fn run(s: &mut Store, agent: Option<&str>, command: WeaveCommand) -> Result<ExitCode> {
    match command {
        WeaveCommand::Init { .. } => unreachable!("handled before opening a store"),
        WeaveCommand::Import { from_git, index, dry_run, repo, out } => {
            let source = if index { Source::Index } else { Source::Rev(from_git.unwrap_or_else(|| "HEAD".to_owned())) };
            import_tree(s, agent, &repo, &source, dry_run, out)
        }
        WeaveCommand::Checkout { manifest, target: dir, out } => {
            let ix = Index::build(s.graph());
            let hash = manifest_hash(s, &ix, &manifest, "--manifest")?;
            let manifest = complete_manifest(&ix, &hash)?;
            let g = s.graph();
            materialize(manifest, |h| ix.file(g, h).map(<[u8]>::to_vec), &dir)
                .with_context(|| format!("checking out {hash} into {}", dir.display()))?;
            if out.json {
                print_json(&json!({"manifest": hash.to_string(), "dir": dir, "files": manifest.entries.len()}));
            } else {
                println!("checkout {hash} {}", dir.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        WeaveCommand::Diff { a, b, out } => {
            let ix = Index::build(s.graph());
            let lines = Lines::build(s.graph(), &ix);
            let (ha, hb) =
                (tree_at(s, &ix, &lines, &a, "the first tree")?, tree_at(s, &ix, &lines, &b, "the second tree")?);
            let (ma, mb) = (held_manifest(&ix, &ha)?, held_manifest(&ix, &hb)?);
            let diffs = diff::manifests(ma, mb);
            // A changed file whose version this replica doesn't hold has no text diff: say which.
            let unheld = |d: &diff::FileDiff| -> Vec<String> {
                [d.before, d.after]
                    .into_iter()
                    .flatten()
                    .filter(|v| !ix.holds_file(&v.hash))
                    .map(|v| v.hash.to_string())
                    .collect()
            };
            if out.json {
                let files: Vec<Value> = diffs
                    .iter()
                    .map(|d| {
                        json!({"path": d.path, "change": d.kind.as_str(), "before": d.before.map(file_value_json),
                               "after": d.after.map(file_value_json), "not_held": unheld(d)})
                    })
                    .collect();
                print_json(&json!({"a": ha.to_string(), "b": hb.to_string(), "files": files}));
                return Ok(ExitCode::SUCCESS);
            }
            let g = s.graph();
            for d in &diffs {
                let letter = match d.kind {
                    diff::Kind::Added => "A",
                    diff::Kind::Deleted => "D",
                    diff::Kind::Modified => "M",
                    diff::Kind::ModeChanged => "T",
                };
                let modes = match (d.before, d.after) {
                    (Some(x), Some(y)) if x.mode != y.mode => format!(" ({:o} -> {:o})", x.mode.bits(), y.mode.bits()),
                    _ => String::new(),
                };
                println!("{letter} {}{modes}", crate::terminal::printable(&d.path));
                let missing = unheld(d);
                if d.kind == diff::Kind::Modified && !missing.is_empty() {
                    println!("  (no text: this Ground doesn't hold {})", missing.join(" or "));
                    continue;
                }
                if let (diff::Kind::Modified, Some(x), Some(y)) = (d.kind, d.before, d.after) {
                    let (old, new) = (ix.file(g, &x.hash), ix.file(g, &y.hash));
                    if let (Some(old), Some(new)) = (old, new)
                        && diff::is_text(x.mode, old)
                        && diff::is_text(y.mode, new)
                    {
                        let (old, new) = (std::str::from_utf8(old)?, std::str::from_utf8(new)?);
                        match diff::unified(&d.path, old, new) {
                            Some(text) => {
                                for line in text.lines() {
                                    println!("{}", crate::terminal::printable(line));
                                }
                            }
                            None => println!("  (too large to diff)"),
                        }
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        WeaveCommand::Verify { git, at, manifest, line, repo, out } => {
            let ix = Index::build(s.graph());
            let lines = Lines::build(s.graph(), &ix);
            let hash = match (at, manifest) {
                (Some(at), _) => decision_manifest(s, &lines, &at, "--at")?,
                (None, Some(manifest)) => manifest_hash(s, &ix, &manifest, "--manifest")?,
                (None, None) => {
                    let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
                    lines.decision(&single_head(&lines, &line)?).expect("a head is a decision").decision.manifest
                }
            };
            let manifest = held_manifest(&ix, &hash)?;
            let plan = import::plan(&repo, &Source::Rev(git.clone()), &KnownKeys::new())?;
            let differences = compare(manifest, &plan);
            let equal = differences.is_empty();
            if out.json {
                print_json(
                    &json!({"manifest": hash.to_string(), "git": git, "equal": equal, "differences": differences}),
                );
            } else if equal {
                println!("equal: {hash} is git's {git}");
            } else {
                for d in &differences {
                    println!("{}", crate::terminal::printable(d["text"].as_str().unwrap_or_default()));
                }
                println!("differ: {hash} is not git's {git} ({} differences)", differences.len());
            }
            Ok(decide(equal))
        }
        WeaveCommand::Check { out } => {
            let ix = Index::build(s.graph());
            let mut rows: Vec<Value> = ix.problems().iter().map(problem_json).collect();
            for id in Lines::build(s.graph(), &ix).ignored_versions() {
                rows.push(json!({
                    "problem": "no_owner_decision", "id": id,
                    "text": format!("policy version {id} cites no owner decision, so it counts for nothing; only the main line weave init writes may"),
                }));
            }
            if out.json {
                print_json(&json!({"marker": ix.counted_marker(), "problems": rows}));
            } else if rows.is_empty() {
                println!("no problems");
            } else {
                for row in &rows {
                    println!("{}", crate::terminal::printable(row["text"].as_str().unwrap_or_default()));
                }
            }
            Ok(decide(rows.is_empty()))
        }
        WeaveCommand::Head { line, out } => {
            let ix = Index::build(s.graph());
            let lines = Lines::build(s.graph(), &ix);
            let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
            let name = &lines.line(&line).expect("resolved").policy.name;
            let heads: Vec<(ContributionId, ContentHash)> = lines
                .heads(&line)
                .into_iter()
                .map(|id| (id, lines.decision(&id).expect("a head is a decision").decision.manifest))
                .collect();
            if out.json {
                let rows: Vec<Value> =
                    heads.iter().map(|(id, m)| json!({"decision": id, "manifest": m.to_string()})).collect();
                print_json(&json!({"line": line, "name": name, "forked": heads.len() > 1, "heads": rows}));
                return Ok(ExitCode::SUCCESS);
            }
            let name = crate::terminal::printable(name);
            match heads.len() {
                0 => println!("line {line} ({name}): no decision yet"),
                1 => println!("line {line} ({name})"),
                n => println!("line {line} ({name}): forked, {n} heads; a decision listing them all settles it"),
            }
            for (id, manifest) in &heads {
                println!("head {id} manifest {manifest}");
            }
            Ok(ExitCode::SUCCESS)
        }
        WeaveCommand::Log { line, limit, since, out } => {
            let ix = Index::build(s.graph());
            let lines = Lines::build(s.graph(), &ix);
            let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
            let since = since.map(|d| decision_id(s, &lines, &d, "--since")).transpose()?;
            let all = lines.log(s.graph(), &line, since.as_ref());
            let shown = &all[..all.len().min(limit)];
            let names = super::Names::load(s)?;
            if out.json {
                let rows: Vec<Value> = shown.iter().map(|d| decision_json(d, &names)).collect();
                print_json(&json!({"line": line, "decisions": rows, "more": all.len() > shown.len()}));
                return Ok(ExitCode::SUCCESS);
            }
            for d in shown {
                let r = &d.decision;
                let mut text = format!("decision {} manifest {} by {}", d.id, r.manifest, names.display(&d.author));
                if !r.parents.is_empty() {
                    let parents: Vec<String> = r.parents.iter().map(ContributionId::short).collect();
                    text.push_str(&format!(" after {}", parents.join(" ")));
                }
                if let Some(source) = &r.source {
                    text.push_str(&format!(" git {}", source.git));
                }
                println!("{}", crate::terminal::printable(&text));
                for b in &r.breakage {
                    let line = format!("  breakage {}: {} ({})", b.check, b.detail, b.paths.join(", "));
                    println!("{}", crate::terminal::printable(&line));
                }
                if !r.note.trim().is_empty() {
                    println!("  {}", crate::terminal::printable(r.note.lines().next().unwrap_or_default()));
                }
            }
            if all.len() > shown.len() {
                println!("({} more; raise --limit, or read on with --since)", all.len() - shown.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        WeaveCommand::Status { index, line, at, repo, out } => {
            let ix = Index::build(s.graph());
            let lines = Lines::build(s.graph(), &ix);
            let at = match at {
                Some(at) => Some(decision_id(s, &lines, &at, "--at")?),
                None => {
                    let line = resolve_line(s, &lines, line.as_deref(), "--line")?;
                    match lines.heads(&line).as_slice() {
                        [] => None,
                        [one] => Some(*one),
                        _ => single_head(&lines, &line).map(Some)?,
                    }
                }
            };
            let empty = Manifest::new(Vec::new());
            let base = match &at {
                Some(d) => held_manifest(&ix, &lines.decision(d).expect("resolved").decision.manifest)?,
                None => &empty,
            };
            let source = if index { Source::Index } else { Source::Rev("HEAD".to_owned()) };
            let plan = import::plan(&repo, &source, &KnownKeys::new())?;
            let ours = Manifest::new(plan.entries.clone());
            let diffs = diff::manifests(base, &ours);
            let refused: Vec<String> = plan.refused.iter().map(ToString::to_string).collect();
            let what = if index { "the index" } else { "HEAD" };
            if out.json {
                let files: Vec<Value> =
                    diffs.iter().map(|d| json!({"path": d.path, "change": d.kind.as_str()})).collect();
                print_json(&json!({"at": at, "manifest": at.map(|_| base.content_hash().to_string()),
                                   "git": if index { "index" } else { "HEAD" }, "files": files, "refused": refused}));
                return Ok(ExitCode::SUCCESS);
            }
            match at {
                Some(d) => println!("{what} against decision {d}"),
                None => println!("{what} against an empty tree: the line has no decision yet"),
            }
            for d in &diffs {
                println!("{} {}", d.kind.as_str(), crate::terminal::printable(&d.path));
            }
            for r in &refused {
                println!("refused {r}");
            }
            if diffs.is_empty() && refused.is_empty() {
                println!("no differences");
            }
            Ok(ExitCode::SUCCESS)
        }
        WeaveCommand::Integrate { line, proposals, note, out, .. } if !proposals.is_empty() => {
            merge::integrate_proposals(s, agent, line.as_deref(), &proposals, note, out)
        }
        WeaveCommand::Line { command } => merge::line(s, agent, command),
        WeaveCommand::Propose(args) => merge::propose(s, agent, args),
        WeaveCommand::Resolve(args) => merge::resolve(s, agent, args),
        WeaveCommand::Integrate { line, from_git, index, manifest, note, repo, out, .. } => {
            let source = match (manifest, index) {
                (Some(m), _) => Given::Manifest(m),
                (None, true) => Given::Git(Source::Index),
                (None, false) => Given::Git(Source::Rev(from_git.unwrap_or_else(|| "HEAD".to_owned()))),
            };
            integrate(s, agent, line.as_deref(), source, note, &repo, out)
        }
    }
}

/// The tree `integrate` records.
enum Given {
    Git(Source),
    Manifest(String),
}

fn integrate(
    s: &mut Store,
    agent: Option<&str>,
    line: Option<&str>,
    given: Given,
    note: String,
    repo: &Path,
    out: Output,
) -> Result<ExitCode> {
    let ix = Index::build(s.graph());
    if ix.counted_marker().is_none() {
        bail!(
            "this Ground holds no Weave marker, so weave writes nothing here; create a Weave Ground with `bracken weave init`"
        );
    }
    let lines = Lines::build(s.graph(), &ix);
    let line = resolve_line(s, &lines, line, "--line")?;
    let who = super::signer(s, agent)?;
    let parents = lines.heads(&line);
    let draft = |manifest: ContentHash, source: Option<String>| Decision {
        v: VERSION,
        line,
        manifest,
        parents: parents.clone(),
        proposals: vec![],
        breakage: vec![],
        answers: vec![],
        resolutions: vec![],
        goals: vec![],
        source: source.map(|git| bracken_tree::Source { git }),
        note: note.clone(),
    };
    // Rules 2 and 3 don't depend on the tree, so they are checked before anything is written. The
    // manifest's own rule is checked again once the candidate is written.
    match lines.judge_new(s.graph(), &ix, who.agent_id(), &draft(ContentHash::of(b""), None)) {
        Ok(_) | Err(Unqualified::ManifestNotHeld(_) | Unqualified::ManifestIncomplete { .. }) => {}
        Err(why) => bail!("the decision wouldn't qualify, so nothing was written: {why}"),
    }

    let (hash, commit, files, manifest_id) = match given {
        Given::Manifest(m) => {
            let hash = manifest_hash(s, &ix, &m, "--manifest")?;
            complete_manifest(&ix, &hash)?;
            (hash, None, Vec::new(), None)
        }
        Given::Git(source) => {
            let plan = import::plan(repo, &source, &known_keys(s, repo)?)?;
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
            let hash = manifest.content_hash();
            let held = ix.manifest(&hash).is_some();
            let new: Vec<&import::FileVersion> = plan.new_files(|h| ix.holds_file(h)).collect();
            let (written, failure) = write_tree(s, &who, &new, &manifest, held)?;
            let files: Vec<(ContentHash, String)> = written
                .iter()
                .filter_map(|(p, _)| p.map(|p| (new.iter().find(|f| f.name == p).expect("written").hash, p.to_owned())))
                .collect();
            if let Some(e) = failure {
                decide(false);
                print_tree_written(None, &files, out);
                return Err(
                    e.context(format!("{} file versions were written, and no manifest or decision", files.len()))
                );
            }
            let manifest_id = written.iter().find(|(p, _)| p.is_none()).map(|(_, id)| *id);
            (hash, plan.commit.clone(), files, manifest_id)
        }
    };

    let ix = Index::build(s.graph());
    let lines = Lines::build(s.graph(), &ix);
    let decision = draft(hash, commit);
    if let Err(why) = lines.judge_new(s.graph(), &ix, who.agent_id(), &decision) {
        decide(false);
        print_tree_written(Some(hash), &files, out);
        bail!("the decision wouldn't qualify, so none was written (the candidate stays): {why}");
    }
    let id = match write_record(s, &who, &decision) {
        Ok(id) => id,
        Err(e) => {
            decide(false);
            print_tree_written(Some(hash), &files, out);
            return Err(e.context("the candidate was written, but not the decision"));
        }
    };
    if out.json {
        let rows: Vec<Value> = files.iter().map(|(h, p)| json!({"hash": h.to_string(), "path": p})).collect();
        print_json(&json!({"manifest": hash.to_string(), "manifest_id": manifest_id, "files": rows, "decision": id,
                           "line": line, "parents": parents}));
    } else {
        println!("manifest {hash}");
        for (h, p) in &files {
            println!("file {h} {}", crate::terminal::printable(p));
        }
        println!("decision {id}");
    }
    Ok(ExitCode::SUCCESS)
}

/// What `integrate` wrote before it stopped: the manifest, if it is held, and the file versions.
fn print_tree_written(manifest: Option<ContentHash>, files: &[(ContentHash, String)], out: Output) {
    if out.json {
        let rows: Vec<Value> = files.iter().map(|(h, p)| json!({"hash": h.to_string(), "path": p})).collect();
        print_json(&json!({"manifest": manifest.map(|m| m.to_string()), "files": rows, "decision": Value::Null}));
        return;
    }
    if let Some(m) = manifest {
        println!("manifest {m}");
    }
    for (h, p) in files {
        println!("file {h} {}", crate::terminal::printable(p));
    }
}

fn decision_json(d: &DecisionRecord, names: &super::Names) -> Value {
    let r = &d.decision;
    let breakage: Vec<Value> =
        r.breakage.iter().map(|b| json!({"check": b.check, "detail": b.detail, "paths": b.paths})).collect();
    json!({"decision": d.id, "author": d.author.to_string(), "author_name": names.display(&d.author),
           "manifest": r.manifest.to_string(), "parents": r.parents, "breakage": breakage,
           "proposals": r.proposals, "resolutions": r.resolutions,
           "source": r.source.as_ref().map(|s| json!({"git": s.git})), "note": r.note})
}

/// A line named on the command line, by its id (or a prefix) or its name; the
/// default line if none is given.
fn resolve_line(s: &Store, lines: &Lines, given: Option<&str>, what: &str) -> Result<ContributionId> {
    let Some(given) = given else {
        return lines
            .default_line()
            .context("this Ground has no default line: no line named main by the marker's author");
    };
    if let Ok(id) = s.graph().resolve_prefix(given) {
        if lines.line(&id).is_some() {
            return Ok(id);
        }
        if lines.version(&id).is_some() {
            bail!("{what}: {} is a later version of a line's policy; name the line by its first artifact", id.short());
        }
        let c = s.graph().get(&id).expect("resolved ids exist");
        bail!("{what}: {} is a {}, not a line", id.short(), kind_of(c));
    }
    let named: Vec<ContributionId> = lines.lines().filter(|v| v.policy.name == given).map(|v| v.id).collect();
    match named.as_slice() {
        [one] => Ok(*one),
        [] => bail!("{what}: no line is named {given:?}"),
        many => {
            let ids: Vec<String> = many.iter().map(ContributionId::short).collect();
            bail!("{what}: {} lines are named {given:?} ({}); name one by its id", many.len(), ids.join(", "))
        }
    }
}

/// A line's one head; refuses while it is forked, or before its first decision.
fn single_head(lines: &Lines, line: &ContributionId) -> Result<ContributionId> {
    match lines.heads(line).as_slice() {
        [one] => Ok(*one),
        [] => bail!("line {} has no decision yet", line.short()),
        heads => {
            let ids: Vec<String> = heads.iter().map(ContributionId::short).collect();
            bail!("line {} is forked, with heads {}; name one with --at", line.short(), ids.join(", "))
        }
    }
}

/// A decision named on the command line by its id; anything else is refused,
/// naming what it is.
fn decision_id(s: &Store, lines: &Lines, given: &str, what: &str) -> Result<ContributionId> {
    if given.parse::<ContentHash>().is_ok() {
        bail!("{what}: {given} is a content hash; a decision is named by its id");
    }
    let id = s.graph().resolve_prefix(given).with_context(|| format!("{what}: {given:?}"))?;
    if lines.decision(&id).is_none() {
        let c = s.graph().get(&id).expect("resolved ids exist");
        bail!("{what}: {} is a {}, not a decision", id.short(), kind_of(c));
    }
    Ok(id)
}

fn decision_manifest(s: &Store, lines: &Lines, given: &str, what: &str) -> Result<ContentHash> {
    let id = decision_id(s, lines, given, what)?;
    Ok(lines.decision(&id).expect("resolved").decision.manifest)
}

/// A tree named on the command line: a manifest by content hash, or a decision by id.
fn tree_at(s: &Store, ix: &Index, lines: &Lines, given: &str, what: &str) -> Result<ContentHash> {
    if given.parse::<ContentHash>().is_err()
        && let Ok(id) = s.graph().resolve_prefix(given)
        && lines.decision(&id).is_some()
    {
        return decision_manifest(s, lines, given, what);
    }
    manifest_hash(s, ix, given, what)
}

fn file_value_json(v: bracken_tree::FileValue) -> Value {
    json!({"hash": v.hash.to_string(), "mode": format!("{:o}", v.mode.bits())})
}

/// A manifest named on the command line, by content hash. A bare hex value, or
/// the content hash of something else, is refused with what it names instead.
fn manifest_hash(s: &Store, ix: &Index, given: &str, what: &str) -> Result<ContentHash> {
    match given.parse::<ContentHash>() {
        Ok(hash) => match ix.hash_kind(&hash) {
            Some("manifest") => Ok(hash),
            Some(kind) => bail!("{what}: {hash} is a {kind}, not a manifest"),
            None => bail!("{what}: this Ground holds no manifest {hash}"),
        },
        Err(_) => {
            let named = s.graph().resolve_prefix(given).ok().and_then(|id| s.graph().get(&id));
            match named {
                Some(c) => bail!(
                    "{what}: {} is a contribution id, of a {}, not a manifest's content hash; give blake3:<hex>",
                    c.id().short(),
                    kind_of(c)
                ),
                None => bail!("{what}: {given:?} isn't a content hash; a manifest is named blake3:<64 hex digits>"),
            }
        }
    }
}

fn held_manifest<'a>(ix: &'a Index, hash: &ContentHash) -> Result<&'a Manifest> {
    ix.manifest(hash).map(|(_, m)| m).with_context(|| format!("this Ground holds no valid manifest {hash}"))
}

fn complete_manifest<'a>(ix: &'a Index, hash: &ContentHash) -> Result<&'a Manifest> {
    let manifest = held_manifest(ix, hash)?;
    let missing = ix.missing(manifest);
    if !missing.is_empty() {
        bail!(
            "{hash} isn't complete here: this replica lacks the files, at the sizes it names, of {} paths, such as {:?}",
            missing.len(),
            missing[0]
        );
    }
    Ok(manifest)
}

/// What differs between a manifest and git's tree, as JSON rows with a `text` line each.
fn compare(manifest: &Manifest, plan: &Plan) -> Vec<Value> {
    let mut rows = Vec::new();
    for refusal in &plan.refused {
        rows.push(json!({"difference": "refused", "detail": refusal.to_string(), "text": format!("refused in git: {refusal}")}));
    }
    let ours: BTreeMap<&str, &Entry> = manifest.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let theirs: BTreeMap<&str, &Entry> = plan.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    for (path, e) in &ours {
        match theirs.get(path) {
            None => rows.push(json!({"difference": "only_in_manifest", "path": path, "text": format!("only in the manifest: {path}")})),
            Some(g) if g.hash != e.hash || g.mode != e.mode || g.size != e.size => rows.push(json!({
                "difference": "differs", "path": path,
                "manifest": {"hash": e.hash.to_string(), "mode": format!("{:o}", e.mode.bits())},
                "git": {"hash": g.hash.to_string(), "mode": format!("{:o}", g.mode.bits())},
                "text": format!("differs: {path}"),
            })),
            Some(_) => {}
        }
    }
    for path in theirs.keys().filter(|p| !ours.contains_key(*p)) {
        rows.push(json!({"difference": "only_in_git", "path": path, "text": format!("only in git: {path}")}));
    }
    rows
}

fn problem_json(p: &Problem) -> Value {
    match p {
        Problem::InvalidPayload { id, media_type, error } => json!({
            "problem": "invalid_payload", "id": id, "media_type": media_type, "error": error.to_string(),
            "text": format!("invalid payload {id} ({media_type}): {error}"),
        }),
        Problem::NotAnAncestor { id, reference } => json!({
            "problem": "not_an_ancestor", "id": id, "reference": reference,
            "text": format!("ancestor rule: {id} refers to {reference}, which isn't its causal ancestor"),
        }),
        Problem::Incomplete { id, hash, missing } => json!({
            "problem": "incomplete", "id": id, "manifest": hash.to_string(), "missing": missing,
            "text": format!("incomplete: manifest {hash} lacks {} files here, at the sizes it names", missing.len()),
        }),
        Problem::ExtraMarker { id, counted } => json!({
            "problem": "extra_marker", "id": id, "counted": counted,
            "text": format!("extra marker {id}: the one that counts is {counted}"),
        }),
    }
}

/// The keys import must never take: this Ground's identities, and the
/// coordination Ground's at the repository's main checkout, if it has one.
fn known_keys(s: &Store, repo: &Path) -> Result<KnownKeys> {
    let mut known = KnownKeys::new();
    known.add_identities_dir(&s.identities_dir()).context("reading this Ground's identities")?;
    let coordination = import::main_checkout(repo)?.join(".bracken").join("identities");
    known.add_identities_dir(&coordination).with_context(|| format!("reading {}", coordination.display()))?;
    Ok(known)
}

fn exclusion(e: &Exclusion) -> &'static str {
    match e {
        Exclusion::BrackenRoot => "a .bracken root entry",
        Exclusion::Submodule => "a submodule",
    }
}

fn import_tree(
    s: &mut Store,
    agent: Option<&str>,
    repo: &Path,
    source: &Source,
    dry_run: bool,
    out: Output,
) -> Result<ExitCode> {
    let ix = Index::build(s.graph());
    if !dry_run && ix.counted_marker().is_none() {
        bail!(
            "this Ground holds no Weave marker, so weave writes nothing here; create a Weave Ground with `bracken weave init`"
        );
    }
    let plan = import::plan(repo, source, &known_keys(s, repo)?)?;
    let excluded: Vec<Value> =
        plan.excluded.iter().map(|(p, why)| json!({"path": p, "reason": exclusion(why)})).collect();
    let refused: Vec<String> = plan.refused.iter().map(ToString::to_string).collect();
    let Some(manifest) = plan.manifest() else {
        if out.json {
            print_json(&json!({"refused": refused, "excluded": excluded}));
        } else {
            for r in &refused {
                println!("refused {r}");
            }
        }
        decide(false);
        bail!("the import is refused ({} refusals), and nothing was written", refused.len());
    };
    let hash = manifest.content_hash();
    let manifest_held = ix.manifest(&hash).is_some();
    let new: Vec<&import::FileVersion> = plan.new_files(|h| ix.holds_file(h)).collect();

    if dry_run {
        if out.json {
            let files: Vec<Value> = manifest
                .entries
                .iter()
                .map(|e| {
                    json!({"path": e.path, "mode": format!("{:o}", e.mode.bits()), "size": e.size,
                                "hash": e.hash.to_string(), "new": !ix.holds_file(&e.hash)})
                })
                .collect();
            print_json(&json!({"manifest": hash.to_string(), "manifest_new": !manifest_held, "files": files,
                               "new_files": new.len(), "excluded": excluded, "refused": refused}));
        } else {
            for e in &manifest.entries {
                let state = if ix.holds_file(&e.hash) { "held" } else { "new" };
                println!(
                    "file {} {:o} {} {} ({state})",
                    e.hash,
                    e.mode.bits(),
                    e.size,
                    crate::terminal::printable(&e.path)
                );
            }
            for (p, why) in &plan.excluded {
                println!("excluded {} ({})", crate::terminal::printable(p), exclusion(why));
            }
            let state = if manifest_held { "held" } else { "new" };
            println!("manifest {hash} ({state}; {} new files; dry run, nothing written)", new.len());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let who = super::signer(s, agent)?;
    let (written, failure) = write_tree(s, &who, &new, &manifest, manifest_held)?;
    let file_hash: BTreeMap<&str, ContentHash> = new.iter().map(|f| (f.name.as_str(), f.hash)).collect();
    if failure.is_some() {
        decide(false);
    }
    let manifest_id = written.iter().find(|(p, _)| p.is_none()).map(|(_, id)| *id);
    if out.json {
        let files: Vec<Value> = written
            .iter()
            .filter_map(|(p, id)| p.map(|p| json!({"hash": file_hash[p].to_string(), "path": p, "id": id})))
            .collect();
        let manifest_field = if failure.is_none() { json!(hash.to_string()) } else { Value::Null };
        print_json(&json!({"manifest": manifest_field, "manifest_id": manifest_id, "manifest_new": !manifest_held,
                           "files": files, "excluded": excluded}));
    } else {
        if failure.is_none() {
            if manifest_held {
                println!("held {hash}: this manifest was already held, so none was written");
            } else {
                println!("manifest {hash}");
            }
        }
        for (p, _) in &written {
            if let Some(p) = p {
                println!("file {} {}", file_hash[p], crate::terminal::printable(p));
            }
        }
    }
    match failure {
        None => Ok(ExitCode::SUCCESS),
        Some(e) => Err(e.context(format!("{} file versions were written, and no manifest", written.len()))),
    }
}

/// What [`write_tree`] wrote, by path (`None` for the manifest), and the error that stopped it.
type Written<'a> = (Vec<(Option<&'a str>, ContributionId)>, Option<anyhow::Error>);

/// Writes a tree's new file versions, then its manifest unless it is held. Every
/// write is checked against the contribution limit before the first, with the
/// parents the first write would have, so a tree isn't written part way for size
/// (note fefde0fa66f2 item 3). Returns what was written, by path (`None` for the
/// manifest), and the error that stopped it, if one did.
fn write_tree<'a>(
    s: &mut Store,
    who: &Identity,
    new: &[&'a import::FileVersion],
    manifest: &Manifest,
    manifest_held: bool,
) -> Result<Written<'a>> {
    let heads: Vec<ContributionId> = s.graph().heads().collect();
    let mut bodies: Vec<(Option<&str>, Body)> = new
        .iter()
        .map(|f| {
            let body = Body::Artifact {
                name: f.name.clone(),
                media_type: import::FileVersion::MEDIA_TYPE.to_owned(),
                content: f.bytes.clone(),
            };
            (Some(f.name.as_str()), body)
        })
        .collect();
    if !manifest_held {
        let body = Body::Artifact {
            name: manifest.artifact_name().to_owned(),
            media_type: Manifest::MEDIA_TYPE.to_owned(),
            content: manifest.encode(),
        };
        bodies.push((None, body));
    }
    for (path, body) in &bodies {
        let size =
            Contribution::create(s.info().ground, who, heads.iter().copied(), u64::MAX, body.clone()).encode().len();
        if size > MAX_CONTRIBUTION_BYTES {
            let what = path.map_or_else(|| "the manifest".to_owned(), |p| format!("the file version of {p:?}"));
            bail!(
                "{what} would be {size} bytes as a contribution, over the {MAX_CONTRIBUTION_BYTES}-byte limit; nothing was written"
            );
        }
    }

    let mut written: Vec<(Option<&str>, ContributionId)> = Vec::new();
    let mut failure = None;
    for (path, body) in bodies {
        match contribute(s, who, body) {
            Ok(id) => written.push((path, id)),
            Err(e) => {
                let what = path.map_or_else(|| "the manifest".to_owned(), |p| format!("the file version of {p:?}"));
                failure = Some(e.context(format!("writing {what}")));
                break;
            }
        }
    }
    Ok((written, failure))
}
