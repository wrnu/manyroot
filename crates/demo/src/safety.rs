//! Keeping a run's directory, and the private keys in it, away from repositories.

use std::path::{Component, Path, PathBuf};
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

/// What to tell the operator when a run's directory is refused: each caller has
/// its own way out.
pub struct Advice {
    /// For a directory inside a checkout.
    pub checkout: &'static str,
    /// For GIT_DIR or GIT_WORK_TREE in the environment.
    pub env: &'static str,
}

/// The plain demo, which has --allow-in-repo.
pub const DEMO: Advice = Advice {
    checkout: "run the demo outside any repository, or pass --allow-in-repo",
    env: "unset it, or pass --allow-in-repo",
};

/// compare, whose run directories come from the system's temporary directory.
pub const COMPARE: Advice = Advice { checkout: "set TMPDIR to a directory outside any repository", env: "unset it" };

/// Fails if `path`, which need not exist yet, is inside a git checkout, or if the
/// environment points git at one, unless `allowed`.
pub fn refuse_git_checkout(path: &Path, allowed: bool, advice: &Advice) -> Result<()> {
    if allowed {
        return Ok(());
    }
    for var in ["GIT_DIR", "GIT_WORK_TREE"] {
        if std::env::var_os(var).is_some() {
            bail!("{var} is set, so git may treat {} as part of a checkout; {}", path.display(), advice.env);
        }
    }
    let abs = std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;
    let existing = abs.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let real = std::fs::canonicalize(existing).with_context(|| format!("resolving {}", existing.display()))?;
    let refuse = |checkout: &Path| {
        bail!("{} is inside the git checkout {}; {}", path.display(), checkout.display(), advice.checkout)
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

/// Creates `dir`, which must not exist yet, and returns where it really is.
///
/// The deepest existing ancestor is resolved once, and `check` runs on the
/// target beneath that real path. Each new directory is then made beneath it,
/// never by resolving the typed path again, and recorded with its device and
/// inode. If a check fails, or anything recorded changed while the directories
/// were being made, the directories are removed innermost first, but only those
/// that still are the ones this made: a path that now leads somewhere else is
/// left alone.
pub fn create_checked(dir: &Path, check: impl Fn(&Path) -> Result<()>) -> Result<PathBuf> {
    create_checked_with(dir, check, |_| {})
}

/// [`create_checked`], calling `after_create` after each directory is made, so tests
/// can change the filesystem at the worst moment.
fn create_checked_with(
    dir: &Path,
    check: impl Fn(&Path) -> Result<()>,
    mut after_create: impl FnMut(&Path),
) -> Result<PathBuf> {
    let existing = dir.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let new = dir.strip_prefix(existing).expect("an ancestor is a prefix");
    let parts: Vec<Component> = new.components().filter(|c| *c != Component::CurDir).collect();
    if parts.is_empty() {
        bail!("{} already exists", dir.display());
    }
    if parts.iter().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("{} has a component that isn't a plain name after {}", dir.display(), existing.display());
    }
    let base = std::fs::canonicalize(existing).with_context(|| format!("resolving {}", existing.display()))?;
    let target: PathBuf = parts.iter().fold(base, |p, c| p.join(c));
    check(&target)?;

    let mut created: Vec<(PathBuf, Identity)> = Vec::new();
    let result = (|| {
        let mut path = target.clone();
        for _ in 1..parts.len() {
            path.pop();
        }
        for (i, c) in parts.iter().enumerate() {
            if i > 0 {
                path.push(c);
            }
            std::fs::create_dir(&path).with_context(|| format!("creating {}", path.display()))?;
            created.push((path.clone(), Identity::of(&path)?));
            after_create(&path);
        }
        // Everything made must still be what was made: nothing swapped underneath.
        for (p, id) in &created {
            if Identity::of(p).ok().as_ref() != Some(id) {
                bail!("{} changed while the run's directory was being created", p.display());
            }
        }
        check(&target)?;
        Ok(target.clone())
    })();
    if result.is_err() {
        // Innermost first; remove_dir only removes empty directories.
        for (p, id) in created.iter().rev() {
            if Identity::of(p).ok().as_ref() == Some(id) {
                let _ = std::fs::remove_dir(p);
            }
        }
    }
    result
}

/// Which directory a path names, without following a final symlink.
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl Identity {
    fn of(path: &Path) -> Result<Identity> {
        let meta = std::fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
        if !meta.is_dir() {
            bail!("{} isn't a directory", path.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Identity { dev: meta.dev(), ino: meta.ino() })
        }
        #[cfg(not(unix))]
        Ok(Identity {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_directory_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b/c");
        let refused = create_checked(&dir, |_| bail!("refused"));
        assert_eq!(refused.unwrap_err().to_string(), "refused");
        assert!(!tmp.path().join("a").exists(), "every directory it created is gone, parents included");

        let made = create_checked(&dir, |_| Ok(())).unwrap();
        assert_eq!(made, std::fs::canonicalize(&dir).unwrap());
        assert!(create_checked(&dir, |_| Ok(())).is_err(), "an existing directory is refused");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_follows_the_real_path_not_the_typed_one() {
        // The typed path goes through a symlink; the directories really live elsewhere.
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, tmp.path().join("link")).unwrap();
        let refused = create_checked(&tmp.path().join("link/x/y"), |_| bail!("refused"));
        assert!(refused.is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing left where the directories really were");
    }

    /// Replaces the directory at `path` with a symlink to `to`, keeping the original as `<path>.moved`.
    #[cfg(unix)]
    fn swap_for_symlink(path: &Path, to: &Path) {
        std::fs::rename(path, path.with_extension("moved")).unwrap();
        std::os::unix::fs::symlink(to, path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_swapped_for_a_symlink_mid_creation_is_never_removed_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = std::fs::canonicalize(dir.path()).unwrap();
        let victims = tmp.join("victims");
        std::fs::create_dir_all(victims.join("b")).unwrap(); // pre-existing, empty, not ours
        std::fs::create_dir(tmp.join("safe")).unwrap();

        // After 'a' is made, it's swapped for a link to victims, where 'b' already exists.
        let a = tmp.join("safe/a");
        let mut first = true;
        let made = create_checked_with(
            &a.join("b"),
            |_| Ok(()),
            |p| {
                if std::mem::take(&mut first) {
                    assert_eq!(p, a);
                    swap_for_symlink(p, &victims);
                }
            },
        );
        assert!(made.is_err());
        assert!(victims.join("b").is_dir(), "the pre-existing directory survives");

        // The same swap, where 'b' doesn't exist yet: what it made through the link is the
        // only thing removed, and the run is refused because 'a' changed.
        let victims2 = tmp.join("victims2");
        std::fs::create_dir(&victims2).unwrap();
        std::fs::create_dir(victims2.join("keep")).unwrap();
        let a2 = tmp.join("safe/a2");
        let mut first = true;
        let made = create_checked_with(
            &a2.join("b"),
            |_| Ok(()),
            |p| {
                if std::mem::take(&mut first) {
                    swap_for_symlink(p, &victims2);
                }
            },
        );
        assert!(made.unwrap_err().to_string().contains("changed while"));
        let left: Vec<_> = std::fs::read_dir(&victims2).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["keep"], "only the directory it made is gone");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_flipped_during_creation_never_costs_a_pre_existing_directory() {
        // The reviewer's race (finding 62bc7a9e9a35), in-process: a thread flips safe/sw between
        // safe/real and victims while runs create sw/runN, and a check refuses victims, as if it
        // were a checkout. victims/runN all exist beforehand.
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let tmp = std::fs::canonicalize(dir.path()).unwrap();
        let (safe, victims) = (tmp.join("safe"), tmp.join("victims"));
        let real = safe.join("real");
        std::fs::create_dir_all(&real).unwrap();
        const N: usize = 300;
        for i in 0..N {
            std::fs::create_dir_all(victims.join(format!("run{i}"))).unwrap();
        }
        let sw = safe.join("sw");
        std::os::unix::fs::symlink(&real, &sw).unwrap();
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                let mut k = 0;
                while !stop.load(Ordering::Relaxed) {
                    let tmp_link = safe.join("tmpln");
                    std::os::unix::fs::symlink(if k % 2 == 1 { &victims } else { &real }, &tmp_link).unwrap();
                    std::fs::rename(&tmp_link, &sw).unwrap();
                    k += 1;
                }
            });
            for i in 0..N {
                let refuse_victims = |p: &Path| {
                    if p.starts_with(&victims) { bail!("inside the checkout") } else { Ok(()) }
                };
                let _ = create_checked(&sw.join(format!("run{i}")), refuse_victims);
            }
            stop.store(true, Ordering::Relaxed);
        });
        let gone: Vec<usize> = (0..N).filter(|i| !victims.join(format!("run{i}")).exists()).collect();
        assert!(gone.is_empty(), "pre-existing directories removed: {gone:?}");
        let extra = std::fs::read_dir(&victims).unwrap().count() - N;
        assert_eq!(extra, 0, "nothing was left inside the refused directory");
    }
}
