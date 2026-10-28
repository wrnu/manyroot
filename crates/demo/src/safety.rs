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

/// Makes the private directory a run happens in, under the system's temporary
/// directory once [`refuse_shared`] has passed it, and refuses it inside a git
/// checkout unless `allowed`. Returns the directory and its real, absolute path.
pub fn run_dir(allowed: bool, advice: &Advice) -> Result<(tempfile::TempDir, PathBuf)> {
    let base = std::env::temp_dir();
    let base = std::fs::canonicalize(&base).with_context(|| format!("resolving {}", base.display()))?;
    refuse_shared(&base)?;
    run_dir_in(&base, allowed, advice)
}

/// Makes the run's directory in `base`, which [`refuse_shared`] has passed, and checks
/// what it got: on macOS, a new directory inherits ACL entries from its parent, and so
/// does everything made in it, keys included, so it must have no allow entry at all.
fn run_dir_in(base: &Path, allowed: bool, advice: &Advice) -> Result<(tempfile::TempDir, PathBuf)> {
    let temp = tempfile::Builder::new().tempdir_in(base).context("creating a temporary directory")?;
    let root = temp.path().to_owned();
    #[cfg(target_os = "macos")]
    if acl::entries(&root).with_context(|| format!("reading the ACL of {}", root.display()))?.iter().any(|e| e.allow) {
        bail!(
            "the run's directory {} got an ACL entry from {} that lets others in; \
             set TMPDIR to a directory only you can use",
            root.display(),
            base.display()
        );
    }
    refuse_git_checkout(&root, allowed, advice)?;
    Ok((temp, root))
}

/// Fails unless nobody else can rename entries in `dir`, a real path, or in any
/// directory above it: each must be owned by this user or root, and either not
/// writable by group or others, or sticky. On macOS each must also have no ACL allow
/// entry, whoever it names, that grants adding, deleting or renaming entries, or
/// changing its permissions or owner, and none that new files and directories inherit,
/// whatever it grants: the run's directory and its keys would get it, whatever their
/// mode. (On Linux, ACLs show in the group bits.) bracken writes a run's files, keys
/// included, by path, so a rename there could send them into a checkout.
#[cfg(unix)]
pub fn refuse_shared(dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::getuid().as_raw();
    for d in dir.ancestors() {
        let m = std::fs::metadata(d).with_context(|| format!("reading {}", d.display()))?;
        let owned = m.uid() == me || m.uid() == 0;
        let closed = m.mode() & 0o022 == 0 || m.mode() & 0o1000 != 0;
        // On macOS: an allow entry granting a change, and whether any allow entry is inherited.
        #[cfg(target_os = "macos")]
        let (change, inherited) = {
            let acl = acl::entries(d).with_context(|| format!("reading the ACL of {}", d.display()))?;
            let allow: Vec<&acl::Entry> = acl.iter().filter(|e| e.allow).collect();
            (allow.iter().find_map(|e| e.changes), allow.iter().any(|e| e.inherits))
        };
        #[cfg(not(target_os = "macos"))]
        let (change, inherited): (Option<&str>, bool) = (None, false);
        let why = if !owned {
            "belongs to another user".to_owned()
        } else if !closed {
            "is writable by others and not sticky".to_owned()
        } else if let Some(perm) = change {
            format!("has an ACL entry allowing {perm}")
        } else if inherited {
            "has an ACL entry that new files inherit, whatever their mode".to_owned()
        } else {
            continue;
        };
        bail!(
            "the temporary directory {} isn't private: {} {why}, so others could reach the run's files, \
             keys included; set TMPDIR to a directory only you can write to",
            dir.display(),
            d.display()
        );
    }
    Ok(())
}

/// Reading a directory's ACL on macOS, which has no binding in rustix.
#[cfg(target_os = "macos")]
mod acl {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    // <sys/acl.h>
    unsafe extern "C" {
        fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
        fn acl_get_permset(entry: *mut c_void, permset: *mut *mut c_void) -> c_int;
        fn acl_get_perm_np(permset: *mut c_void, perm: c_int) -> c_int;
        fn acl_get_flagset_np(obj: *mut c_void, flagset: *mut *mut c_void) -> c_int;
        fn acl_get_flag_np(flagset: *mut c_void, flag: c_int) -> c_int;
        fn acl_free(obj: *mut c_void) -> c_int;
    }
    const ACL_TYPE_EXTENDED: c_int = 0x100;
    const ACL_FIRST_ENTRY: c_int = 0;
    const ACL_NEXT_ENTRY: c_int = -1;
    const ACL_EXTENDED_ALLOW: c_int = 1;
    /// Permissions that let someone add, remove or rename entries, or grant themselves that.
    const CHANGES: [(c_int, &str); 6] = [
        (1 << 2, "add_file"),
        (1 << 4, "delete"),
        (1 << 5, "add_subdirectory"),
        (1 << 6, "delete_child"),
        (1 << 12, "writesecurity"),
        (1 << 13, "chown"),
    ];
    /// Flags that pass an entry on to new files and directories.
    const INHERITS: [c_int; 2] = [1 << 5, 1 << 6];
    const ENOENT: i32 = 2;
    const ENOTSUP: i32 = 45;
    const EOPNOTSUPP: i32 = 102;

    /// One entry of an ACL.
    pub struct Entry {
        /// An allow entry, not a deny one.
        pub allow: bool,
        /// The first of [`CHANGES`] it grants, if any.
        pub changes: Option<&'static str>,
        /// Whether new files or directories inherit it.
        pub inherits: bool,
    }

    /// The entries of `path`'s ACL; none if it has no ACL.
    pub fn entries(path: &Path) -> std::io::Result<Vec<Entry>> {
        let c = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: `c` is a NUL-terminated path. A non-null ACL is freed below.
        let acl = unsafe { acl_get_link_np(c.as_ptr(), ACL_TYPE_EXTENDED) };
        if acl.is_null() {
            // No ACL, or a file system without them.
            let err = std::io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(ENOENT | ENOTSUP | EOPNOTSUPP) => Ok(Vec::new()),
                _ => Err(err),
            };
        }
        let mut entries = Vec::new();
        // SAFETY: `acl` is valid until `acl_free`, and every entry, permission set and flag
        // set read here points into it; each call's result is checked before its output
        // is used.
        unsafe {
            let (mut id, mut entry) = (ACL_FIRST_ENTRY, std::ptr::null_mut());
            while acl_get_entry(acl, id, &mut entry) == 0 {
                id = ACL_NEXT_ENTRY;
                let (mut tag, mut perms, mut flags) = (0, std::ptr::null_mut(), std::ptr::null_mut());
                if acl_get_tag_type(entry, &mut tag) != 0
                    || acl_get_permset(entry, &mut perms) != 0
                    || acl_get_flagset_np(entry, &mut flags) != 0
                {
                    acl_free(acl);
                    return Err(std::io::Error::last_os_error());
                }
                entries.push(Entry {
                    allow: tag == ACL_EXTENDED_ALLOW,
                    changes: CHANGES.iter().find(|&&(perm, _)| acl_get_perm_np(perms, perm) == 1).map(|&(_, n)| n),
                    inherits: INHERITS.iter().any(|&flag| acl_get_flag_np(flags, flag) == 1),
                });
            }
            acl_free(acl);
        }
        Ok(entries)
    }
}

/// Without unix there are no owners and modes to check.
#[cfg(not(unix))]
pub fn refuse_shared(_dir: &Path) -> Result<()> {
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
            use std::os::unix::ffi::OsStrExt;
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

    #[test]
    fn a_temporary_directory_others_can_rename_entries_in_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(&tmp);
        let dir = root.join("t");
        std::fs::create_dir(&dir).unwrap();
        let mode = |path: &Path, mode| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();

        refuse_shared(&dir).unwrap();
        for open in [0o777, 0o770, 0o722] {
            mode(&dir, open);
            let refused = refuse_shared(&dir).unwrap_err().to_string();
            assert!(refused.contains("writable by others and not sticky"), "{open:o}: {refused}");
        }
        mode(&dir, 0o1777);
        refuse_shared(&dir).unwrap();

        // A private directory under a shared one isn't private: its parent's entries can move.
        mode(&dir, 0o777);
        let below = dir.join("mine");
        std::fs::create_dir(&below).unwrap();
        mode(&below, 0o700);
        let refused = refuse_shared(&below).unwrap_err().to_string();
        assert!(refused.contains(&format!("{} is writable", dir.display())), "{refused}");
        mode(&dir, 0o700);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_temporary_directory_whose_acl_lets_others_rename_entries_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = real(&tmp).join("t");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let chmod =
            |args: &[&str]| assert!(Command::new("/bin/chmod").args(args).arg(&dir).status().unwrap().success());

        refuse_shared(&dir).unwrap();
        chmod(&["+a", "everyone deny delete_child"]);
        refuse_shared(&dir).unwrap();
        for perm in ["add_file", "add_subdirectory", "delete_child", "writesecurity"] {
            chmod(&["-N"]);
            chmod(&["+a", &format!("everyone allow list,{perm}")]);
            let refused = refuse_shared(&dir).unwrap_err().to_string();
            assert!(refused.contains(&format!("has an ACL entry allowing {perm}")), "{perm}: {refused}");
        }
        // Read only, but inherited: the run's directory and its keys would get it.
        chmod(&["-N"]);
        chmod(&[
            "+a",
            "everyone allow list,search,read,readattr,readextattr,readsecurity,file_inherit,directory_inherit",
        ]);
        let refused = refuse_shared(&dir).unwrap_err().to_string();
        assert!(refused.contains("has an ACL entry that new files inherit"), "{refused}");
        chmod(&["-N"]);
        refuse_shared(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_run_directory_that_inherits_an_acl_entry_is_refused_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let base = real(&tmp).join("base");
        std::fs::create_dir(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let chmod =
            |args: &[&str]| assert!(Command::new("/bin/chmod").args(args).arg(&base).status().unwrap().success());

        // A clean base: the run directory, and a key made in it, carry no ACL entry.
        let (temp, root) = run_dir_in(&base, false, &DEMO).unwrap();
        let key = root.join("agent-0/.bracken/identities/agent-0.json");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "secret").unwrap();
        assert!(acl::entries(&root).unwrap().is_empty());
        assert!(acl::entries(&key).unwrap().is_empty(), "the key has an ACL entry");
        drop(temp);

        // An inherited entry that refuse_shared would refuse, made here to test the check
        // after creation on its own.
        chmod(&["+a", "everyone allow list,search,read,file_inherit,directory_inherit"]);
        let refused = run_dir_in(&base, false, &DEMO).unwrap_err().to_string();
        assert!(refused.contains("got an ACL entry from"), "{refused}");
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0, "the refused directory is removed");
        chmod(&["-N"]);
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
