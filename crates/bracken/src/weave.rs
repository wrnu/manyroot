//! `bracken weave`: Bracken's source trees on Weave, manifests only so far
//! (docs/design/weave.md, section 3; build step 3).
//!
//! Every write checks that the Ground holds a counted Weave marker, and prints
//! one labelled line per record (`manifest blake3:<hex>`, `file blake3:<hex>
//! <path>`), or named fields with `--json`. Every read has `--json`. A content
//! hash is always written `blake3:<hex>`; an option that takes a manifest
//! refuses a bare hex value, and names the kind of contribution it names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bracken_tree::format::{Guardrails, Line, Manifest, Marker, Record, VERSION};
use bracken_tree::ground::{Index, Problem, kind_of};
use bracken_tree::import::{self, Exclusion, Plan, Source};
use bracken_tree::materialize::materialize;
use bracken_tree::secrets::KnownKeys;
use bracken_tree::{ContentHash, Entry, diff};
use clap::Subcommand;
use manyroot::contribution::MAX_CONTRIBUTION_BYTES;
use manyroot::{Body, Contribution, ContributionId, Identity, Store};
use serde_json::{Value, json};

use super::{Output, contribute, decide, print_json, write_stdout};

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
    /// The per-file difference between two manifests, with a unified diff of each changed text file.
    Diff {
        /// The first manifest: blake3:<hex>.
        a: String,
        /// The second manifest: blake3:<hex>.
        b: String,
        #[command(flatten)]
        out: Output,
    },
    /// Compare a manifest with a git commit's tree: the same paths, modes and bytes, with the excluded
    /// root entries left out. Exits non-zero if they differ.
    Verify {
        /// The commit to compare with.
        #[arg(long, value_name = "REV")]
        git: String,
        /// The manifest: blake3:<hex>.
        #[arg(long, value_name = "HASH")]
        manifest: String,
        /// The git repository the commit is in.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[command(flatten)]
        out: Output,
    },
    /// List Weave payloads that are invalid or break the ancestor rule, manifests not yet complete,
    /// and markers past the one that counts. Exits non-zero when it lists any.
    Check {
        #[command(flatten)]
        out: Output,
    },
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
            let (ha, hb) =
                (manifest_hash(s, &ix, &a, "the first manifest")?, manifest_hash(s, &ix, &b, "the second manifest")?);
            let (ma, mb) = (held_manifest(&ix, &ha)?, held_manifest(&ix, &hb)?);
            let diffs = diff::manifests(ma, mb);
            if out.json {
                let files: Vec<Value> = diffs
                    .iter()
                    .map(|d| {
                        json!({"path": d.path, "change": d.kind.as_str(), "before": d.before.map(file_value_json),
                               "after": d.after.map(file_value_json)})
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
        WeaveCommand::Verify { git, manifest, repo, out } => {
            let ix = Index::build(s.graph());
            let hash = manifest_hash(s, &ix, &manifest, "--manifest")?;
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
            let rows: Vec<Value> = ix.problems().iter().map(problem_json).collect();
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
    }
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
            "{hash} isn't complete here: this replica lacks the files of {} paths, such as {:?}",
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
            "text": format!("incomplete: manifest {hash} lacks {} files here", missing.len()),
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
    // Every write is checked against the contribution limit before the first, with the parents the
    // first write would have, so an import doesn't stop part way for size (note fefde0fa66f2 item 3).
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
            Contribution::create(s.info().ground, &who, heads.iter().copied(), u64::MAX, body.clone()).encode().len();
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
        match contribute(s, &who, body) {
            Ok(id) => written.push((path, id)),
            Err(e) => {
                let what = path.map_or_else(|| "the manifest".to_owned(), |p| format!("the file version of {p:?}"));
                failure = Some(e.context(format!("writing {what}")));
                break;
            }
        }
    }
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
