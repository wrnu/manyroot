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

/// Copies the finished run in `from`, a private directory, to `dest`, which must
/// not exist yet, and returns where `dest` really is.
///
/// On unix, nothing here can be redirected by renaming or swapping directories on
/// the path while it runs. The deepest existing ancestor of `dest` is opened once,
/// as a directory handle, and `check` runs on the real path of that handle's target
/// beneath it. Every new directory and file is then made relative to its parent's
/// handle, without following symlinks, and recorded with its device and inode.
/// If anything fails, or the final check on where the copy really is refuses, what
/// was made is removed through each parent's handle, entry by entry, and only
/// entries that still have the device and inode recorded for them. The first new
/// directory is the only one made where someone else might rename it (its parent
/// existed before; every directory below it is ours, mode 0700), so its handle
/// stays open and it's emptied through that handle wherever it has been moved, then
/// removed only if it's still where it was made. Anything below it that isn't the
/// entry made there stays, and so does anything in it that wasn't made here. Apart
/// from the existing directory and that one, only the handles on the current path
/// are open, so the number of open files doesn't grow with the size of the run.
///
/// One gap stays: a directory swapped in for a new one between creating and
/// opening it is caught if it's a symlink or isn't empty, not if it's another
/// empty directory. Nothing is ever removed or written through it either way.
#[cfg(unix)]
pub fn keep_run(from: &Path, dest: &Path, check: impl Fn(&Path) -> Result<()>) -> Result<PathBuf> {
    keep_run_with(from, dest, check, |_, _| {})
}

/// [`keep_run`], calling `after_mkdir` between making each new directory and
/// opening it, so tests can swap it at the worst moment.
#[cfg(unix)]
fn keep_run_with(
    from: &Path,
    dest: &Path,
    check: impl Fn(&Path) -> Result<()>,
    mut after_mkdir: impl FnMut(&Path, &std::ffi::OsStr),
) -> Result<PathBuf> {
    use handles::*;

    let existing = dest.ancestors().find(|p| p.exists()).context("no part of the path exists")?;
    let new = dest.strip_prefix(existing).expect("an ancestor is a prefix");
    let parts: Vec<&std::ffi::OsStr> = new
        .components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| match c {
            Component::Normal(name) => Ok(name),
            _ => bail!("{} has a component that isn't a plain name after {}", dest.display(), existing.display()),
        })
        .collect::<Result<_>>()?;
    if parts.is_empty() {
        bail!("{} already exists", dest.display());
    }
    let base = open_dir(existing).with_context(|| format!("opening {}", existing.display()))?;
    check(&parts.iter().fold(path_of(&base)?, |p, c| p.join(c)))?;

    let (mut made, mut top) = (Vec::new(), None);
    let result = (|| {
        let (first, mut into) = dir(&base, &mut made, parts[0], &mut after_mkdir)?;
        let top = &*top.insert(first);
        let mut last = None;
        for name in &parts[1..] {
            let (fd, inside) = dir(last.as_ref().unwrap_or(top), into, name, &mut after_mkdir)?;
            (last, into) = (Some(fd), inside);
        }
        let parent = last.as_ref().unwrap_or(top);
        copy_tree(from, parent, into, &mut after_mkdir)?;
        let real = path_of(parent)?;
        check(&real)?;
        Ok(real)
    })();
    if result.is_err() {
        undo_all(&base, top.as_ref(), &made);
    }
    result
}

/// [`keep_run`] without directory handles: creates `dest` and copies by path, so a
/// concurrent swap on the path can redirect it. Only unix has the handle version.
#[cfg(not(unix))]
pub fn keep_run(from: &Path, dest: &Path, check: impl Fn(&Path) -> Result<()>) -> Result<PathBuf> {
    fn copy(from: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir(to).with_context(|| format!("creating {}", to.display()))?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                copy(&e.path(), &to.join(e.file_name()))?;
            } else {
                std::fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    check(dest)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    copy(from, dest)?;
    Ok(dest.to_owned())
}

/// Directory-handle operations for [`keep_run`].
#[cfg(unix)]
mod handles {
    use std::ffi::{OsStr, OsString};
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};
    use rustix::fs::{AtFlags, Mode, OFlags};

    /// A directory or file this made, as its name and its identity, with what it made
    /// inside a directory. No handles: they're reopened from the parent to undo.
    pub struct Entry {
        name: OsString,
        dev: u64,
        ino: u64,
        inside: Option<Vec<Entry>>,
    }

    pub fn open_dir(path: &Path) -> Result<OwnedFd> {
        Ok(rustix::fs::open(path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?)
    }

    /// Where a handle's directory is now.
    pub fn path_of(fd: &OwnedFd) -> Result<PathBuf> {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let path = rustix::fs::getpath(fd).context("asking where a directory is")?;
            Ok(PathBuf::from(OsStr::from_bytes(path.as_bytes())))
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        {
            use std::os::fd::AsRawFd;
            std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).context("asking where a directory is")
        }
    }

    fn identity(fd: &OwnedFd) -> Result<(u64, u64)> {
        let st = rustix::fs::fstat(fd)?;
        #[allow(clippy::unnecessary_cast)]
        Ok((st.st_dev as u64, st.st_ino as u64))
    }

    fn is_empty(fd: &OwnedFd) -> Result<bool> {
        let mut dir = rustix::fs::Dir::read_from(fd)?;
        while let Some(entry) = dir.read() {
            let name = entry?.file_name().to_bytes().to_vec();
            if name != b"." && name != b".." {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Makes directory `name` in `parent` and opens it, refusing a symlink or a
    /// directory that isn't empty in its place. Records it in `into`, and returns its
    /// handle and where to record what's made inside it.
    pub fn dir<'a>(
        parent: &OwnedFd,
        into: &'a mut Vec<Entry>,
        name: &OsStr,
        after_mkdir: &mut impl FnMut(&Path, &OsStr),
    ) -> Result<(OwnedFd, &'a mut Vec<Entry>)> {
        let shown = || Path::new(name).display().to_string();
        rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(0o700))
            .with_context(|| format!("creating {}", shown()))?;
        after_mkdir(&path_of(parent).unwrap_or_default(), name);
        let fd = open_beneath(parent, name)
            .with_context(|| format!("{} changed while the kept run was being created", shown()))?;
        if !is_empty(&fd)? {
            bail!("{} changed while the kept run was being created", shown());
        }
        let (dev, ino) = identity(&fd)?;
        into.push(Entry { name: name.to_owned(), dev, ino, inside: Some(Vec::new()) });
        let inside = into.last_mut().and_then(|e| e.inside.as_mut()).expect("just pushed a directory");
        Ok((fd, inside))
    }

    fn open_beneath(parent: &OwnedFd, name: &OsStr) -> rustix::io::Result<OwnedFd> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        rustix::fs::openat(parent, name, flags, Mode::empty())
    }

    /// Makes file `name` in `parent`, which must not exist, with `content` and `mode`.
    fn file(parent: &OwnedFd, into: &mut Vec<Entry>, name: &OsStr, content: &[u8], mode: u32) -> Result<()> {
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(parent, name, flags, Mode::from_raw_mode((mode & 0o777) as rustix::fs::RawMode))
            .with_context(|| format!("creating {}", Path::new(name).display()))?;
        let (dev, ino) = identity(&fd)?;
        into.push(Entry { name: name.to_owned(), dev, ino, inside: None });
        std::fs::File::from(fd).write_all(content)?;
        Ok(())
    }

    /// Copies the directory tree at `from` into the directory `to`, recording it in `into`.
    pub fn copy_tree(
        from: &Path,
        to: &OwnedFd,
        into: &mut Vec<Entry>,
        after_mkdir: &mut impl FnMut(&Path, &OsStr),
    ) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(from)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let kind = e.file_type()?;
            if kind.is_dir() {
                let (sub, inside) = dir(to, into, &e.file_name(), after_mkdir)?;
                copy_tree(&e.path(), &sub, inside, after_mkdir)?;
            } else if kind.is_file() {
                let mode = e.metadata()?.permissions().mode();
                file(to, into, &e.file_name(), &std::fs::read(e.path())?, mode)?;
            } else {
                bail!("{} is neither a file nor a directory", e.path().display());
            }
        }
        Ok(())
    }

    /// Removes everything in `made`, all made beneath `base`. The first new directory
    /// is emptied through its own handle `top` first, so a rename can't keep its
    /// contents; see [`keep_run`](super::keep_run).
    pub fn undo_all(base: &OwnedFd, top: Option<&OwnedFd>, made: &[Entry]) {
        if let (Some(top), Some(Entry { inside: Some(inside), .. })) = (top, made.first()) {
            undo(top, inside);
        }
        undo(base, made);
    }

    /// Removes everything in `made`, newest first, through `parent`'s handle, skipping
    /// any entry that is no longer the one made. A directory is entered by reopening it
    /// from its parent without following symlinks, and only if it's still the one made.
    fn undo(parent: &OwnedFd, made: &[Entry]) {
        let is_made = |e: &Entry, st: rustix::fs::Stat| {
            #[allow(clippy::unnecessary_cast)]
            let id = (st.st_dev as u64, st.st_ino as u64);
            id == (e.dev, e.ino)
        };
        for e in made.iter().rev() {
            if let Some(inside) = &e.inside
                && let Ok(fd) = open_beneath(parent, &e.name)
                && rustix::fs::fstat(&fd).is_ok_and(|st| is_made(e, st))
            {
                undo(&fd, inside);
            }
            let Ok(st) = rustix::fs::statat(parent, e.name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW) else {
                continue;
            };
            if is_made(e, st) {
                let flags = if e.inside.is_some() { AtFlags::REMOVEDIR } else { AtFlags::empty() };
                let _ = rustix::fs::unlinkat(parent, e.name.as_os_str(), flags);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A finished run to keep: a file, a private key and a nested directory.
    fn run_dir(root: &Path) -> PathBuf {
        let run = root.join("run");
        std::fs::create_dir_all(run.join("agent-0/.bracken/identities")).unwrap();
        std::fs::write(run.join(".gitignore"), "*\n").unwrap();
        let key = run.join("agent-0/.bracken/identities/agent-0.json");
        std::fs::write(&key, "secret").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        run
    }

    fn real(dir: &tempfile::TempDir) -> PathBuf {
        std::fs::canonicalize(dir.path()).unwrap()
    }

    #[test]
    fn keeps_a_copy_and_leaves_nothing_when_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let run = run_dir(&real(&tmp));
        let dest = real(&tmp).join("kept/a/b");

        let refused = keep_run(&run, &dest, |_| bail!("refused"));
        assert_eq!(refused.unwrap_err().to_string(), "refused");
        assert!(!real(&tmp).join("kept").exists(), "a refusal before copying makes nothing");

        let checks = std::cell::Cell::new(0);
        let late = keep_run(&run, &dest, |_| {
            checks.set(checks.get() + 1);
            if checks.get() == 2 { bail!("refused late") } else { Ok(()) }
        });
        assert!(late.is_err());
        assert!(!real(&tmp).join("kept").exists(), "a refusal after copying removes everything it made");

        let kept = keep_run(&run, &dest, |_| Ok(())).unwrap();
        assert_eq!(kept, dest);
        assert_eq!(std::fs::read_to_string(kept.join(".gitignore")).unwrap(), "*\n");
        let key = kept.join("agent-0/.bracken/identities/agent-0.json");
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "secret");
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600, "keys stay private");
        assert!(keep_run(&run, &dest, |_| Ok(())).is_err(), "an existing directory is refused");
    }

    #[test]
    fn keeps_and_undoes_more_directories_than_the_open_file_limit() {
        // Runs itself again in a child with a low open-file limit, where the work happens.
        const LIMIT: usize = 64;
        if std::env::var_os("KEEP_FD_LIMIT_CHILD").is_none() {
            let name = format!("{}::keeps_and_undoes_more_directories_than_the_open_file_limit", module_path!());
            let name = name.split_once("::").expect("a crate path").1;
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &format!("ulimit -n {LIMIT} && exec \"$0\" \"$@\"")])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", name, "--test-threads", "1"])
                .env("KEEP_FD_LIMIT_CHILD", "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
            assert!(stdout.contains("1 passed"), "the child ran the test: {stdout}");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let run = run_dir(&root);
        for i in 0..4 * LIMIT {
            std::fs::create_dir_all(run.join(format!("agent-0/.bracken/objects/{i:03}"))).unwrap();
            std::fs::write(run.join(format!("agent-0/.bracken/objects/{i:03}/o")), "x").unwrap();
        }
        let dest = root.join("kept/a");

        let checks = std::cell::Cell::new(0);
        let late = keep_run(&run, &dest, |_| {
            checks.set(checks.get() + 1);
            if checks.get() == 2 { bail!("refused late") } else { Ok(()) }
        });
        assert_eq!(late.unwrap_err().to_string(), "refused late");
        assert!(!root.join("kept").exists(), "undo removes everything it made");

        let kept = keep_run(&run, &dest, |_| Ok(())).unwrap();
        let objects = std::fs::read_dir(kept.join("agent-0/.bracken/objects")).unwrap().count();
        assert_eq!(objects, 4 * LIMIT);
        assert_eq!(std::fs::read_to_string(kept.join("agent-0/.bracken/objects/255/o")).unwrap(), "x");
    }

    #[test]
    fn a_new_directory_renamed_away_before_undo_is_emptied_where_it_went() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let run = run_dir(&root);
        std::fs::create_dir(root.join("safe")).unwrap();
        let away = root.join("away");

        let checks = std::cell::Cell::new(0);
        let late = keep_run(&run, &root.join("safe/kept/a"), |_| {
            checks.set(checks.get() + 1);
            if checks.get() == 1 {
                return Ok(());
            }
            std::fs::rename(root.join("safe/kept"), &away).unwrap();
            std::fs::write(away.join("theirs"), "not ours").unwrap();
            bail!("refused late")
        });
        assert_eq!(late.unwrap_err().to_string(), "refused late");
        let left: Vec<_> = std::fs::read_dir(&away).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["theirs"], "everything made is gone from where it went, and nothing else");
        assert!(!root.join("safe/kept").exists());
    }

    /// Replaces directory `name` in `parent` with a symlink to `to`, keeping it as `<name>.moved`.
    fn swap_for_symlink(parent: &Path, name: &std::ffi::OsStr, to: &Path) {
        let path = parent.join(name);
        std::fs::rename(&path, path.with_extension("moved")).unwrap();
        std::os::unix::fs::symlink(to, &path).unwrap();
    }

    #[test]
    fn a_new_directory_swapped_for_a_symlink_is_never_used_or_removed_through_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let run = run_dir(&root);
        let victims = root.join("victims");
        std::fs::create_dir_all(victims.join("x")).unwrap(); // pre-existing, empty, not ours
        std::fs::create_dir(root.join("safe")).unwrap();

        let mut first = true;
        let kept = keep_run_with(
            &run,
            &root.join("safe/a/x"),
            |_| Ok(()),
            |parent, name| {
                if std::mem::take(&mut first) {
                    swap_for_symlink(parent, name, &victims);
                }
            },
        );
        assert!(kept.unwrap_err().to_string().contains("changed while"));
        assert!(victims.join("x").is_dir(), "the pre-existing directory survives");
        assert_eq!(std::fs::read_dir(&victims).unwrap().count(), 1, "nothing was written through the link");
    }

    #[test]
    fn a_new_directory_swapped_for_a_full_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let run = run_dir(&root);
        let full = root.join("full");
        std::fs::create_dir_all(full.join("data")).unwrap();
        std::fs::create_dir(root.join("safe")).unwrap();

        let mut first = true;
        let kept = keep_run_with(
            &run,
            &root.join("safe/a"),
            |_| Ok(()),
            |parent, name| {
                if std::mem::take(&mut first) {
                    let path = parent.join(name);
                    std::fs::rename(&path, path.with_extension("moved")).unwrap();
                    std::fs::rename(&full, &path).unwrap();
                }
            },
        );
        assert!(kept.unwrap_err().to_string().contains("changed while"));
        assert!(root.join("safe/a/data").is_dir(), "the other directory and its contents survive");
    }

    #[test]
    fn swaps_on_the_path_during_a_keep_never_cost_or_receive_anything() {
        // The reviewers' races, in-process: a thread swaps safe/real for a link into
        // victims (race3) while keeps go to real/runN; a check refuses victims, as if
        // it were a checkout. victims/runN all exist beforehand.
        use std::sync::atomic::{AtomicBool, Ordering};
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let run = run_dir(&root);
        let (safe, victims) = (root.join("safe"), root.join("victims"));
        let realdir = safe.join("real");
        std::fs::create_dir_all(&realdir).unwrap();
        const N: usize = 200;
        for i in 0..N {
            std::fs::create_dir_all(victims.join(format!("run{i}"))).unwrap();
        }
        let link = safe.join("link");
        std::os::unix::fs::symlink(&victims, &link).unwrap();
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                let parked = safe.join("parked");
                while !stop.load(Ordering::Relaxed) {
                    // real -> parked, link -> real, then back.
                    let _ = std::fs::rename(&realdir, &parked);
                    let _ = std::fs::rename(&link, &realdir);
                    let _ = std::fs::rename(&realdir, &link);
                    let _ = std::fs::rename(&parked, &realdir);
                }
            });
            for i in 0..N {
                let refuse_victims =
                    |p: &Path| if p.starts_with(&victims) { bail!("inside the checkout") } else { Ok(()) };
                let _ = keep_run(&run, &realdir.join(format!("run{i}")), refuse_victims);
            }
            stop.store(true, Ordering::Relaxed);
        });
        let gone: Vec<usize> = (0..N).filter(|i| !victims.join(format!("run{i}")).is_dir()).collect();
        assert!(gone.is_empty(), "pre-existing directories removed: {gone:?}");
        let mut written = Vec::new();
        for i in 0..N {
            written.extend(std::fs::read_dir(victims.join(format!("run{i}"))).unwrap().map(|e| e.unwrap().path()));
        }
        assert_eq!(std::fs::read_dir(&victims).unwrap().count(), N, "nothing new in victims");
        assert!(written.is_empty(), "nothing written into victims: {written:?}");
    }
}
