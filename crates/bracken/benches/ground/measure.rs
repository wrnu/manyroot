//! What the benchmark measures and how: wall time of a `bracken` process, nearest-rank percentiles, the
//! bytes a write leaves in `cache/` (the snapshot), and a copy of a replica that leaves its keys behind.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

/// Runs `bracken --dir <dir> <args>`, failing unless it exits 0, and returns its wall time and standard
/// output. Clears the variables that would change what it reads: no identity, no full check, no other
/// replica.
pub fn run(bin: &Path, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<(Duration, String)> {
    let mut cmd = Command::new(bin);
    cmd.arg("--dir").arg(dir).args(args);
    for var in ["BRACKEN_AGENT", "BRACKEN_FULL_CHECK", "BRACKEN_DIR", "BRACKEN_TOKEN"] {
        cmd.env_remove(var);
    }
    cmd.envs(env.iter().copied()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let start = Instant::now();
    let out = cmd.output().with_context(|| format!("running {}", bin.display()))?;
    let took = start.elapsed();
    if !out.status.success() {
        bail!("bracken {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok((took, String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// The nearest-rank percentile `p` (0..=100) of `samples`, which must not be empty.
pub fn percentile(samples: &[Duration], p: u32) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort();
    let rank = (u64::from(p) * sorted.len() as u64).div_ceil(100).max(1) as usize;
    sorted[rank - 1]
}

/// Every regular file directly in a directory: its name, and its identity (inode on Unix) and length.
pub type Files = BTreeMap<String, (u64, u64)>;

pub fn files(dir: &Path) -> Result<Files> {
    let mut out = Files::new();
    let entries = match fs::read_dir(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        other => other.with_context(|| format!("listing {}", dir.display()))?,
    };
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            out.insert(entry.file_name().to_string_lossy().into_owned(), (inode(&meta), meta.len()));
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn inode(meta: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(meta)
}

#[cfg(not(unix))]
fn inode(_: &fs::Metadata) -> u64 {
    0
}

/// The bytes written between two listings of one directory: a file that is new, or the same name on a new
/// inode (written and renamed into place), counts whole; a file on the same inode counts what it grew by.
/// Rewriting a file in place on the same inode isn't seen, so this is a lower bound for that kind of write.
pub fn written(before: &Files, after: &Files) -> u64 {
    after
        .iter()
        .map(|(name, (ino, len))| match before.get(name) {
            Some((old_ino, old_len)) if old_ino == ino => len.saturating_sub(*old_len),
            _ => *len,
        })
        .sum()
}

/// Total bytes of the regular files under `dir`, recursively (objects, say).
pub fn tree_bytes(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            total += tree_bytes(&entry.path())?;
        } else if kind.is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

/// Copies a replica's `ground.json`, `agents/` and `objects/` into `to`, never `identities/` or `cache/`:
/// the benchmark of a real Ground holds no keys. Regular files only, skipping names starting with a dot
/// (another process's write in flight); a symlink anywhere refuses the copy.
pub fn copy_replica(from: &Path, to: &Path) -> Result<()> {
    ensure!(from.join("ground.json").is_file(), "{} holds no ground.json", from.display());
    fs::create_dir_all(to)?;
    fs::copy(from.join("ground.json"), to.join("ground.json"))?;
    for sub in ["agents", "objects"] {
        if from.join(sub).exists() {
            copy_tree(&from.join(sub), &to.join(sub))?;
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from).with_context(|| format!("listing {}", from.display()))? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).with_context(|| format!("copying {}", entry.path().display()))?;
        } else {
            bail!("{} is neither a file nor a directory", entry.path().display());
        }
    }
    Ok(())
}
