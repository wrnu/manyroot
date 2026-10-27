//! Keeping a run's directory, and the private keys in it, away from repositories.

use std::path::{Component, Path};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Fails if `..` follows a directory that doesn't exist yet. The checks below
/// read such a path lexically, but creating it follows `..` physically, so the
/// directory could end up somewhere they never looked.
pub fn refuse_parent_in_new_part(path: &Path) -> Result<()> {
    let existing = path.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let new = path.strip_prefix(existing).expect("an ancestor is a prefix");
    if new.components().any(|c| c == Component::ParentDir) {
        bail!("{} has '..' after a directory that doesn't exist yet; give the path without it", path.display());
    }
    Ok(())
}

/// Fails if `path`, which need not exist yet, is inside a git checkout, or if the
/// environment points git at one, unless `allowed`.
pub fn refuse_git_checkout(path: &Path, allowed: bool) -> Result<()> {
    if allowed {
        return Ok(());
    }
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        if std::env::var_os(var).is_some() {
            bail!(
                "{var} is set, so git may treat {} as part of a checkout; unset it, or pass --allow-in-repo",
                path.display()
            );
        }
    }
    let abs = std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;
    let existing = abs.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let real = std::fs::canonicalize(existing).with_context(|| format!("resolving {}", existing.display()))?;
    let refuse = |checkout: &Path| {
        bail!(
            "{} is inside the git checkout {}; run the demo outside any repository, or pass --allow-in-repo",
            path.display(),
            checkout.display()
        )
    };
    // Ask git, which also knows work trees whose git directory is elsewhere.
    let git = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree", "--show-toplevel"])
        .current_dir(&real)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = git
        && out.status.success()
        && out.stdout.starts_with(b"true")
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        return refuse(Path::new(stdout.lines().nth(1).unwrap_or("?")));
    }
    // And look for a .git ourselves, in case git isn't installed.
    if let Some(checkout) = real.ancestors().find(|p| p.join(".git").exists()) {
        return refuse(checkout);
    }
    Ok(())
}
