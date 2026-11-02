//! Checks that run outside the sandboxed runner: a check with an [`Executor`](crate::format::Executor)
//! (plan cc496dcee0e3, section 2's second result path). The manifest's tree still runs only as the
//! policy pins it:
//! - the runner script is the policy's, a file inside the validator's runner directory (the main
//!   checkout) whose content hash must equal the one the policy pins, or nothing runs. Those bytes,
//!   read once, are what runs: a copy in the fresh directory. So an executor script must be
//!   self-contained: it calls no script beside it, which the pin wouldn't cover (finding fa1611bbec29);
//! - the manifest is written, by [`materialize`](crate::materialize), into a fresh directory and made a
//!   git commit there, with git's own configuration, hooks and attribute drivers kept out. That commit's
//!   import must be exactly the manifest, nothing excluded or refused, or nothing runs. So the commit,
//!   and its tree id, stand for the manifest;
//! - a `box` check gets a bundle of that commit, and its sha256. The script pushes it to a remote box,
//!   which must print the bundle's sha256, the commit and the checked-out tree id before any of the
//!   tree's code runs; the script writes what the box printed, the box's id, its cost line and, once
//!   the box printed its end line after the table passed, `end`, into the facts file, a local file the
//!   box never sees. They must match, or the run is refused and nothing is recorded
//!   ([`CheckError::Proof`]); without `end` the run fails;
//! - a `mac` check runs in the commit's checkout here, with the sandboxed runner's clean environment
//!   but no sandbox, since its tests apply `sandbox-exec` themselves. The tree's code there can reach
//!   anything its user can, the facts file included, so a mac check's facts prove nothing and none
//!   are required (finding 7b00a1f28300).
//!
//! What this can't do: once the tree's code runs, in the box or here, it can do whatever its user can.
//! The proofs say which tree ran and that a run happened, not that the tree's own tests are honest.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{CheckError, Exit, Fresh, LANG, Runner, canonical, io, toml_string};
use crate::format::{CheckDefinition, ExecutorKind, Manifest, Record};
use crate::hash::ContentHash;
use crate::import::{self, Source};
use crate::secrets::KnownKeys;

/// git, as a check's setup runs it.
const GIT: &str = "/usr/bin/git";

/// shasum, for the bundle's sha256: the box checks it with sha256sum.
const SHASUM: &str = "/usr/bin/shasum";

/// The commit's author, committer and date: fixed, so a manifest always makes the same commit.
const AUTHOR: (&str, &str, &str) = ("weave-runner", "weave-runner@localhost", "2000-01-01T00:00:00+0000");

/// The most output an outside check's log keeps: a mebibyte under a contribution's limit, which the
/// artifact holding it must fit (finding 2e9d57852ec7). A longer output keeps its last bytes only, and
/// its log is withheld.
pub const LOG_CAP: usize = manyroot::contribution::MAX_CONTRIBUTION_BYTES - 1024 * 1024;

/// The most a facts file may hold.
const FACTS_CAP: u64 = 64 * 1024;

/// The content hash of `script`, a relative path inside `runner_dir`, which it must not leave through a
/// symlink.
pub fn script_hash(runner_dir: &Path, script: &str) -> Result<ContentHash, CheckError> {
    script_bytes(runner_dir, script).map(|(_, hash)| hash)
}

/// `script`'s bytes, read once, and their content hash.
fn script_bytes(runner_dir: &Path, script: &str) -> Result<(Vec<u8>, ContentHash), CheckError> {
    crate::path::validate_path(script).map_err(|e| {
        CheckError::Script(format!("{script:?} isn't a relative path inside the runner directory: {e}"))
    })?;
    let dir = canonical(runner_dir)?;
    let path = canonical(&dir.join(script))?;
    if !path.starts_with(&dir) || !path.is_file() {
        return Err(CheckError::Script(format!("{script:?} isn't a file inside {}", dir.display())));
    }
    let bytes = std::fs::read(&path).map_err(io(format!("reading {}", path.display())))?;
    let hash = ContentHash::of(&bytes);
    Ok((bytes, hash))
}

/// Opens `path` for reading, never through a symlink, and without blocking on a FIFO.
fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    Ok(std::fs::File::from(rustix::fs::open(path, flags, Mode::empty())?))
}

/// What an outside check ran on, and how it went.
#[derive(Debug, Clone)]
pub struct OutsideOutcome {
    /// `<name> blake3:<definition hash>`.
    pub method: String,
    pub argv: Vec<String>,
    pub exit: Exit,
    /// The manifest's commit, and its tree id.
    pub commit: String,
    pub tree: String,
    /// For a box: the bundle's sha256, the box's id and boat's cost line, and whether it printed its
    /// end line after the table passed.
    pub bundle_sha256: Option<String>,
    pub box_id: Option<String>,
    pub cost: Option<String>,
    pub ended: bool,
    /// The whole output, up to [`LOG_CAP`] from the end, how many bytes there were, and the hash of
    /// them all, kept as they were read (finding 0fb11da198a5).
    pub log: Vec<u8>,
    pub output_bytes: u64,
    pub output_hash: ContentHash,
}

impl OutsideOutcome {
    /// Whether the check passed: exit 0 and, for a box, its end line.
    pub fn passed(&self) -> bool {
        self.exit.passed() && (self.bundle_sha256.is_none() || self.ended)
    }

    /// Whether the log is the whole output: under [`LOG_CAP`].
    pub fn whole(&self) -> bool {
        self.log.len() as u64 == self.output_bytes
    }

    /// The lines a result's details give about the run, after its exit
    /// ([`details_with_run`](crate::results::details_with_run)).
    pub fn run_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("commit {}", self.commit), format!("tree {}", self.tree)];
        if let Some(b) = &self.bundle_sha256 {
            lines.push(format!("bundle sha256:{b}"));
        }
        if let Some(b) = &self.box_id {
            lines.push(format!("box {b}"));
        }
        if let Some(c) = &self.cost {
            lines.push(format!("cost {c}"));
        }
        if self.bundle_sha256.is_some() {
            lines.push(if self.ended { "end seen".to_owned() } else { "end missing".to_owned() });
        }
        lines.push(format!("output {} bytes", self.output_bytes));
        lines
    }
}

/// Files an outside check is given besides its tree, by name: written into the directory
/// `WEAVE_INPUTS` names.
pub type Inputs = Vec<(String, Vec<u8>)>;

/// What a box check's script gets from the validator's own environment: boat's `PATH` and `HOME`.
pub type BoxEnv = Vec<(OsString, OsString)>;

impl Runner {
    /// Runs `check`, which has an executor, on `manifest` (files from `files` by content hash), with the
    /// runner script from `runner_dir`. `known` holds the keys the commit's import must refuse.
    /// `inputs` are given to the script (the mac check's box logs). A box check's script gets `box_env`
    /// besides its own variables; a mac check gets the sandboxed runner's clean environment, with the
    /// directories in `tools` on its `PATH` before the system's.
    #[allow(clippy::too_many_arguments)]
    pub fn run_outside(
        &self,
        check: &CheckDefinition,
        runner_dir: &Path,
        manifest: &Manifest,
        files: impl Fn(&ContentHash) -> Option<Vec<u8>>,
        known: &KnownKeys,
        inputs: &Inputs,
        box_env: &BoxEnv,
        tools: &[PathBuf],
    ) -> Result<OutsideOutcome, CheckError> {
        let Some(executor) = check.executor else {
            return Err(CheckError::Script(format!("check {} has no executor", check.name)));
        };
        if check.argv.is_empty() {
            return Err(CheckError::EmptyArgv(check.name.to_owned()));
        }
        let (bytes, pinned) = script_bytes(runner_dir, &executor.script)?;
        if pinned != executor.script_hash {
            return Err(CheckError::Script(format!(
                "{} in {} is {pinned}, not the {} the policy pins: update the policy, or run the pinned script",
                executor.script,
                runner_dir.display(),
                executor.script_hash
            )));
        }
        let fresh = Fresh::make(&self.base)?;
        let root = fresh.0.join("w");
        let (tree, home, tmp, cargo, inputs_dir, runner) = (
            root.join("tree"),
            root.join("home"),
            root.join("tmp"),
            root.join("cargo"),
            root.join("inputs"),
            root.join("runner"),
        );
        for dir in [&root, &tree, &home, &tmp, &cargo, &inputs_dir, &runner] {
            std::fs::create_dir(dir).map_err(io(format!("creating {}", dir.display())))?;
        }
        // The bytes hashed are the bytes that run.
        let name = Path::new(&executor.script).file_name().unwrap_or_else(|| std::ffi::OsStr::new("runner"));
        let program = runner.join(name);
        std::fs::write(&program, &bytes).map_err(io("copying the runner script"))?;
        #[cfg(unix)]
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(io("making the runner script executable"))?;
        crate::materialize::materialize(manifest, files, &tree)?;
        let hash = manifest.content_hash();
        let (commit, tree_id) = commit_of(&tree, &home, &hash)?;
        // The commit stands for the manifest only if its import is exactly the manifest.
        let plan = import::plan(&tree, &Source::Rev(commit.clone()), known)
            .map_err(|e| CheckError::Proof(format!("importing the manifest's commit: {e}")))?;
        let imported = plan.manifest().map(|m| m.content_hash());
        if !plan.excluded.is_empty() || !plan.refused.is_empty() || imported != Some(hash) {
            return Err(CheckError::Proof(format!(
                "the commit {commit} made from {hash} doesn't import as it ({} excluded, {} refused), so it can't \
                 stand for it",
                plan.excluded.len(),
                plan.refused.len()
            )));
        }
        for (name, bytes) in inputs {
            if name.is_empty() || name.contains('/') || name.starts_with('.') {
                return Err(CheckError::Script(format!("input {name:?} isn't a plain file name")));
            }
            std::fs::write(inputs_dir.join(name), bytes).map_err(io(format!("writing input {name}")))?;
        }
        let facts = root.join("facts");
        std::fs::write(&facts, b"").map_err(io("creating the facts file"))?;
        let mut env: Vec<(OsString, OsString)> = vec![
            ("WEAVE_MANIFEST".into(), hash.to_string().into()),
            ("WEAVE_COMMIT".into(), commit.clone().into()),
            ("WEAVE_TREE".into(), tree_id.clone().into()),
            ("WEAVE_FACTS".into(), facts.clone().into_os_string()),
            ("WEAVE_INPUTS".into(), inputs_dir.clone().into_os_string()),
            ("LANG".into(), LANG.into()),
            ("TMPDIR".into(), tmp.clone().into_os_string()),
        ];
        let mut bundle_sha256 = None;
        match executor.kind {
            ExecutorKind::Box => {
                let bundle = root.join("tree.bundle");
                git(&tree, &home, &["bundle", "create", "-q", &path_str(&bundle)?, "weave"])?;
                let sha = sha256(&bundle)?;
                env.push(("WEAVE_BUNDLE".into(), bundle.into_os_string()));
                env.push(("WEAVE_BUNDLE_SHA256".into(), sha.clone().into()));
                env.extend(box_env.iter().cloned());
                bundle_sha256 = Some(sha);
            }
            ExecutorKind::Mac => {
                let vendor = canonical(&self.vendor)?;
                let config = format!(
                    "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = {}\n\n\
                     [net]\noffline = true\n",
                    toml_string(&vendor)?
                );
                std::fs::write(cargo.join("config.toml"), config).map_err(io("writing the cargo config"))?;
                let mut path: Vec<PathBuf> = self.toolchain.iter().map(|t| t.bin.clone()).collect();
                path.extend(tools.iter().cloned());
                path.extend(super::SYSTEM_PATH.iter().map(PathBuf::from));
                let path = std::env::join_paths(path).map_err(|e| io("joining PATH")(std::io::Error::other(e)))?;
                env.push(("PATH".into(), path));
                env.push(("HOME".into(), home.clone().into_os_string()));
                env.push(("CARGO_HOME".into(), cargo.clone().into_os_string()));
                if let Some(t) = &self.toolchain {
                    env.push(("RUSTUP_HOME".into(), t.rustup_home.clone().into_os_string()));
                    env.push(("DEVELOPER_DIR".into(), super::DEVELOPER_DIR.into()));
                }
            }
        }
        let mut command = Command::new(&program);
        command.args(check.argv).current_dir(&tree).env_clear().envs(env);
        let (exit, log, output_bytes, output_hash) =
            super::run::run(command, self.outside_timeout, Some(self.outside_grace), LOG_CAP)?;
        let (mut box_id, mut cost, mut ended) = (None, None, false);
        if let Some(sha) = &bundle_sha256 {
            let facts = Facts::read(&facts)?;
            facts.require("head", &commit)?;
            facts.require("tree", &tree_id)?;
            facts.require("bundle", sha)?;
            ended = facts.get("end").is_some();
            let id = facts.get("box").ok_or_else(|| CheckError::Proof("the run named no box".to_owned()))?;
            let plain = id.strip_prefix("bx_").is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            });
            if !plain {
                return Err(CheckError::Proof(format!("the run's box {id:?} isn't a box id")));
            }
            box_id = Some(id.to_owned());
            cost = facts.get("cost").map(str::to_owned);
        }
        Ok(OutsideOutcome {
            method: check.method(),
            argv: check.argv.to_vec(),
            exit,
            commit,
            tree: tree_id,
            bundle_sha256,
            box_id,
            cost,
            ended,
            log,
            output_bytes,
            output_hash,
        })
    }
}

/// The facts file: one `<key> <value>` per line, the first of each key counting.
struct Facts(Vec<(String, String)>);

impl Facts {
    /// Reads the facts file: a regular file, never through a symlink, and at most [`FACTS_CAP`] bytes.
    fn read(path: &Path) -> Result<Facts, CheckError> {
        use std::io::Read;
        let file = open_plain(path).map_err(io("opening the facts file"))?;
        let meta = file.metadata().map_err(io("reading the facts file"))?;
        if !meta.is_file() || meta.len() > FACTS_CAP {
            return Err(CheckError::Proof(format!("the facts file isn't a file of at most {FACTS_CAP} bytes")));
        }
        let mut bytes = Vec::new();
        file.take(FACTS_CAP).read_to_end(&mut bytes).map_err(io("reading the facts file"))?;
        let text = String::from_utf8_lossy(&bytes);
        let pairs = text.lines().filter_map(|l| l.split_once(' ')).map(|(k, v)| (k.to_owned(), v.trim().to_owned()));
        Ok(Facts(pairs.collect()))
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn require(&self, key: &str, want: &str) -> Result<(), CheckError> {
        match self.get(key) {
            Some(v) if v == want => Ok(()),
            Some(v) => Err(CheckError::Proof(format!("the run's {key} was {v:?}, not {want}"))),
            None => Err(CheckError::Proof(format!("the run gave no {key}, so it can't be tied to the manifest"))),
        }
    }
}

fn path_str(path: &Path) -> Result<String, CheckError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| io(format!("{} isn't UTF-8", path.display()))(std::io::Error::other("not UTF-8")))
}

/// git in `dir`, with none of the caller's or the system's configuration, no hooks, and `home` as HOME.
fn git(dir: &Path, home: &Path, args: &[&str]) -> Result<String, CheckError> {
    let out = Command::new(GIT)
        .args(["-c", "core.hooksPath=/dev/null", "-c", "core.autocrlf=false", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", super::SYSTEM_PATH.join(":"))
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", AUTHOR.0)
        .env("GIT_AUTHOR_EMAIL", AUTHOR.1)
        .env("GIT_AUTHOR_DATE", AUTHOR.2)
        .env("GIT_COMMITTER_NAME", AUTHOR.0)
        .env("GIT_COMMITTER_EMAIL", AUTHOR.1)
        .env("GIT_COMMITTER_DATE", AUTHOR.2)
        .env("DEVELOPER_DIR", super::DEVELOPER_DIR)
        .stdin(Stdio::null())
        .output()
        .map_err(io(format!("running git {}", args.join(" "))))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(CheckError::Proof(format!("git {} failed: {}", args.join(" "), why.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Makes the materialized `tree` a git repository holding one commit of all of it, on branch `weave`.
/// Returns the commit and its tree id.
fn commit_of(tree: &Path, home: &Path, manifest: &ContentHash) -> Result<(String, String), CheckError> {
    git(tree, home, &["init", "-q", "-b", "weave"])?;
    // -f: a path the tree's own .gitignore names is still the manifest's.
    git(tree, home, &["add", "-A", "-f"])?;
    git(tree, home, &["commit", "-q", "--no-verify", "--allow-empty", "-m", &format!("weave manifest {manifest}")])?;
    let commit = git(tree, home, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let tree_id = git(tree, home, &["rev-parse", "--verify", "HEAD^{tree}"])?;
    Ok((commit, tree_id))
}

/// The sha256 of `file`, by shasum, in lowercase hex.
fn sha256(file: &Path) -> Result<String, CheckError> {
    let out = Command::new(SHASUM)
        .args(["-a", "256", "--"])
        .arg(file)
        .env_clear()
        .env("PATH", super::SYSTEM_PATH.join(":"))
        .stdin(Stdio::null())
        .output()
        .map_err(io("running shasum"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let hex = text.split_whitespace().next().unwrap_or_default();
    if !out.status.success() || hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io("hashing the bundle")(std::io::Error::other(format!("shasum gave {:?}", text.trim()))));
    }
    Ok(hex.to_ascii_lowercase())
}
