//! Import from git (docs/design/weave.md, *What import takes*).
//!
//! Import takes git's view of a tree, never a directory listing: a commit's tree
//! (`--from-git <rev>`) or the index (`--index`), with their blobs read from git's
//! object store. So untracked and ignored files are never seen, and neither are
//! changes in the working tree that aren't staged.
//!
//! On top of git's view, every root entry starting with `.bracken` is excluded,
//! and anything that looks like a secret, an invalid path or symlink target, a
//! collision or a file over the size limit is refused. One refusal refuses the
//! whole import: [`Plan::manifest`] gives a manifest only when nothing was
//! refused. A plan writes nothing, so it is also what `--dry-run` shows.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use manyroot::contribution::MAX_CONTRIBUTION_BYTES;

use crate::format::{Entry, FILE_MEDIA_TYPE, Manifest, Mode};
use crate::hash::ContentHash;
use crate::path::{self, Collision, PathError};
use crate::secrets::{self, KnownKeys, SecretRule};

/// The largest file import takes. A file version is one artifact contribution,
/// which must stay under manyroot's 16 MiB limit with its name, media type and
/// envelope. 64 KiB is left for the envelope: author, signature, and up to
/// about 1,900 parents.
pub const MAX_FILE_BYTES: usize = MAX_CONTRIBUTION_BYTES - 64 * 1024;

/// Which of git's views to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A commit's tree. The revision must name a commit (directly or through a
    /// tag): a tree id, or a tag to a tree, is refused, since its paths would be
    /// relative to a subtree and slip past the `.bracken` and `identities/` rules.
    Rev(String),
    /// The index: tracked and staged files, with their staged contents.
    Index,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("running git: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("git {command} failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error("{0:?} doesn't name a commit")]
    BadRev(String),
    #[error("the index has unmerged entries, such as {0:?}; resolve them first")]
    Unmerged(String),
    #[error("unexpected output from git {command}: {detail}")]
    Output { command: String, detail: String },
}

/// Why an entry wasn't imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exclusion {
    /// A root entry starting with `.bracken`: the coordination Ground and its tools.
    BrackenRoot,
    /// A submodule, which Weave doesn't model.
    Submodule,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("{path:?} looks like a secret: {rule}")]
    Secret { path: String, rule: SecretRule },
    #[error("{path:?} is not a valid path: {error}")]
    Path { path: String, error: PathError },
    #[error("a path is not UTF-8: {lossy:?}")]
    NotUtf8 { lossy: String },
    #[error("{path:?} is a symlink with an invalid target: {error}")]
    Target { path: String, error: PathError },
    #[error("{path:?} is {size} bytes, over the {MAX_FILE_BYTES} byte limit; it stays in git")]
    TooLarge { path: String, size: u64 },
    #[error("{path:?} has mode {mode:o}, which Weave doesn't model")]
    Mode { path: String, mode: u32 },
    #[error("paths collide: {0}")]
    Collision(Collision),
}

/// A file import would write, once per content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    pub hash: ContentHash,
    /// The path it was first seen at: the artifact's advisory name.
    pub name: String,
    pub bytes: Vec<u8>,
}

impl FileVersion {
    pub const MEDIA_TYPE: &'static str = FILE_MEDIA_TYPE;
}

/// What an import would do. Nothing is written.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The commit whose tree this is, in full, for a [`Source::Rev`].
    pub commit: Option<String>,
    /// The manifest's entries, in path order.
    pub entries: Vec<Entry>,
    /// One file version per content hash in the tree, in order of first path.
    pub files: Vec<FileVersion>,
    pub excluded: Vec<(String, Exclusion)>,
    pub refused: Vec<Refusal>,
}

impl Plan {
    /// The manifest, if nothing was refused.
    pub fn manifest(&self) -> Option<Manifest> {
        self.refused.is_empty().then(|| Manifest::new(self.entries.clone()))
    }

    /// The file versions a Ground that already holds the hashes `held` lacks:
    /// what the import would write.
    pub fn new_files<'a>(&'a self, held: impl Fn(&ContentHash) -> bool + 'a) -> impl Iterator<Item = &'a FileVersion> {
        self.files.iter().filter(move |f| !held(&f.hash))
    }
}

/// One entry of git's view.
struct GitEntry {
    mode: u32,
    oid: String,
    path: Vec<u8>,
}

/// git, on the repository at `repo` and nothing else, whatever the caller's
/// environment says: every `GIT_*` variable is dropped (`GIT_DIR`,
/// `GIT_COMMON_DIR`, `GIT_CONFIG_COUNT` and the rest), the system config isn't
/// read, and no configured command runs (no fsmonitor, no hooks).
fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    for (var, _) in std::env::vars_os() {
        if var.to_string_lossy().starts_with("GIT_") {
            command.env_remove(var);
        }
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"]);
    command.arg("-C").arg(repo);
    command
}

fn run(repo: &Path, args: &[&str]) -> Result<Vec<u8>, ImportError> {
    let output = git(repo).args(args).stdin(Stdio::null()).output().map_err(ImportError::Spawn)?;
    if !output.status.success() {
        return Err(ImportError::Git {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output.stdout)
}

fn output_error(command: &str, detail: impl Into<String>) -> ImportError {
    ImportError::Output { command: command.to_owned(), detail: detail.into() }
}

/// Lists git's view: `ls-tree` of a tree, or `ls-files --stage` of the index,
/// both run at the top of the working tree so paths are from its root.
fn list(top: &Path, source: &Source) -> Result<(Option<String>, Vec<GitEntry>), ImportError> {
    let mut resolved = None;
    let (command, raw) = match source {
        Source::Rev(rev) => {
            if rev.starts_with('-') || rev.is_empty() {
                return Err(ImportError::BadRev(rev.clone()));
            }
            let spec = format!("{rev}^{{commit}}");
            let commit = run(top, &["rev-parse", "--verify", "--quiet", "--end-of-options", &spec])
                .map_err(|_| ImportError::BadRev(rev.clone()))?;
            let commit = String::from_utf8_lossy(&commit).trim().to_owned();
            let raw = run(top, &["ls-tree", "-r", "-z", "--full-tree", &commit])?;
            resolved = Some(commit);
            ("ls-tree", raw)
        }
        Source::Index => ("ls-files", run(top, &["ls-files", "--stage", "-z"])?),
    };
    let mut entries = Vec::new();
    for record in raw.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let tab = record.iter().position(|&b| b == b'\t').ok_or_else(|| output_error(command, "no tab"))?;
        let (meta, path) = (&record[..tab], &record[tab + 1..]);
        let meta = std::str::from_utf8(meta).map_err(|_| output_error(command, "metadata isn't UTF-8"))?;
        let fields: Vec<&str> = meta.split(' ').collect();
        // ls-tree: <mode> <type> <oid>; ls-files --stage: <mode> <oid> <stage>
        let (mode, oid) = match (source, fields.as_slice()) {
            (Source::Rev(_), [mode, _kind, oid]) => (*mode, *oid),
            (Source::Index, [mode, oid, stage]) => {
                if *stage != "0" {
                    return Err(ImportError::Unmerged(String::from_utf8_lossy(path).into_owned()));
                }
                (*mode, *oid)
            }
            _ => return Err(output_error(command, format!("unexpected fields {meta:?}"))),
        };
        let mode = u32::from_str_radix(mode, 8).map_err(|_| output_error(command, format!("mode {mode:?}")))?;
        entries.push(GitEntry { mode, oid: oid.to_owned(), path: path.to_vec() });
    }
    Ok((resolved, entries))
}

/// A blob from git's object store, or the size of one over [`MAX_FILE_BYTES`],
/// which is skipped rather than read into memory.
enum Blob {
    Bytes(Vec<u8>),
    TooLarge(u64),
}

/// Reads blobs from git's object store with one `cat-file --batch`, in order.
fn read_blobs(repo: &Path, oids: &[&str]) -> Result<Vec<Blob>, ImportError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ImportError::Spawn)?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let request: Vec<u8> = oids.iter().flat_map(|oid| format!("{oid}\n").into_bytes()).collect();
    // Written from another thread, so a full pipe either way can't deadlock.
    let writer = std::thread::spawn(move || stdin.write_all(&request));
    let mut out = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut blobs = Vec::with_capacity(oids.len());
    for oid in oids {
        let mut header = String::new();
        out.read_line(&mut header).map_err(ImportError::Spawn)?;
        let fields: Vec<&str> = header.trim_end().split(' ').collect();
        let size = match fields.as_slice() {
            [got, "blob", size] if got == oid => {
                size.parse::<usize>().map_err(|_| output_error("cat-file", format!("size {size:?}")))?
            }
            _ => return Err(output_error("cat-file", format!("for {oid}: {:?}", header.trim_end()))),
        };
        if size > MAX_FILE_BYTES {
            let skipped =
                std::io::copy(&mut (&mut out).take(size as u64), &mut std::io::sink()).map_err(ImportError::Spawn)?;
            if skipped != size as u64 {
                return Err(output_error("cat-file", format!("{oid} ended early")));
            }
            blobs.push(Blob::TooLarge(size as u64));
        } else {
            let mut blob = vec![0u8; size];
            out.read_exact(&mut blob).map_err(ImportError::Spawn)?;
            blobs.push(Blob::Bytes(blob));
        }
        let mut newline = [0u8; 1];
        out.read_exact(&mut newline).map_err(ImportError::Spawn)?;
        if newline != *b"\n" {
            return Err(output_error("cat-file", format!("no newline after {oid}")));
        }
    }
    drop(out);
    writer.join().expect("the writer thread doesn't panic").map_err(ImportError::Spawn)?;
    let status = child.wait().map_err(ImportError::Spawn)?;
    if !status.success() {
        return Err(output_error("cat-file", format!("exited with {status}")));
    }
    Ok(blobs)
}

/// The main checkout of the repository at `repo`: the directory holding its
/// common `.git`, so the coordination Ground's `.bracken/` is found there from
/// any worktree.
pub fn main_checkout(repo: &Path) -> Result<PathBuf, ImportError> {
    let common = run(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = String::from_utf8(common).map_err(|_| output_error("rev-parse", "the git directory isn't UTF-8"))?;
    let common = PathBuf::from(common.trim_end_matches('\n'));
    common.parent().map(Path::to_path_buf).ok_or_else(|| output_error("rev-parse", "the git directory has no parent"))
}

/// Plans an import of git's view of the repository at `repo`. `known` holds the
/// keys of every identity whose key must never be imported: both Grounds'.
pub fn plan(repo: &Path, source: &Source, known: &KnownKeys) -> Result<Plan, ImportError> {
    let top = run(repo, &["rev-parse", "--show-toplevel"])?;
    let top = String::from_utf8(top).map_err(|_| output_error("rev-parse", "the top level isn't UTF-8"))?;
    let top = PathBuf::from(top.trim_end_matches('\n'));
    let (commit, listed) = list(&top, source)?;
    let mut plan = Plan { commit, ..Plan::default() };
    let mut wanted: Vec<(String, Mode, &str)> = Vec::new();
    for entry in &listed {
        let Ok(path) = std::str::from_utf8(&entry.path) else {
            plan.refused.push(Refusal::NotUtf8 { lossy: String::from_utf8_lossy(&entry.path).into_owned() });
            continue;
        };
        let root = path.split('/').next().expect("split always yields one part");
        if path::fold(root).starts_with(".bracken") {
            plan.excluded.push((path.to_owned(), Exclusion::BrackenRoot));
            continue;
        }
        let mode = match entry.mode {
            0o160000 => {
                plan.excluded.push((path.to_owned(), Exclusion::Submodule));
                continue;
            }
            // git reads the old group-writable mode as a plain file.
            0o100664 => Mode::File,
            bits => match Mode::from_bits(bits) {
                Some(mode) => mode,
                None => {
                    plan.refused.push(Refusal::Mode { path: path.to_owned(), mode: bits });
                    continue;
                }
            },
        };
        if let Err(error) = path::validate_path(path) {
            plan.refused.push(Refusal::Path { path: path.to_owned(), error });
            continue;
        }
        if let Some(rule) = secrets::deny_listed(path) {
            plan.refused.push(Refusal::Secret { path: path.to_owned(), rule });
            continue;
        }
        wanted.push((path.to_owned(), mode, &entry.oid));
    }

    let oids: Vec<&str> = wanted.iter().map(|(_, _, oid)| *oid).collect();
    let blobs = read_blobs(&top, &oids)?;

    let mut seen: BTreeMap<ContentHash, ()> = BTreeMap::new();
    for ((path, mode, _), blob) in wanted.into_iter().zip(blobs) {
        let bytes = match blob {
            Blob::Bytes(bytes) => bytes,
            Blob::TooLarge(size) => {
                plan.refused.push(Refusal::TooLarge { path, size });
                continue;
            }
        };
        if let Some(rule) = secrets::scan(&bytes, known) {
            plan.refused.push(Refusal::Secret { path, rule });
            continue;
        }
        if mode == Mode::Symlink
            && let Err(error) = path::validate_symlink_target(&bytes)
        {
            plan.refused.push(Refusal::Target { path, error });
            continue;
        }
        let hash = ContentHash::of(&bytes);
        plan.entries.push(Entry { path: path.clone(), mode, hash, size: bytes.len() as u64 });
        if seen.insert(hash, ()).is_none() {
            plan.files.push(FileVersion { hash, name: path, bytes });
        }
    }
    plan.entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    if let Some(collision) = path::find_collision(plan.entries.iter().map(|e| e.path.as_str())) {
        plan.refused.push(Refusal::Collision(collision));
    }
    Ok(plan)
}
