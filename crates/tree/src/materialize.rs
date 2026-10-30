//! Writing a tree out (docs/design/weave.md, *Writing a tree out*).
//!
//! `checkout` and every check run materialize a manifest the same way, and never
//! write outside the target directory or follow a symlink (finding
//! `d5112c023747`):
//! - only a valid manifest whose files are all at hand, with the right hashes,
//!   sizes and symlink targets, is materialized, and that is checked before
//!   anything is written;
//! - the target is an existing, empty directory, opened without following a
//!   symlink;
//! - each directory component is opened with `openat` and
//!   `O_DIRECTORY | O_NOFOLLOW`, each file is created with
//!   `O_CREAT | O_EXCL | O_NOFOLLOW`, and its mode is set exactly to `0644` or
//!   `0755` with `fchmod`, whatever the umask;
//! - symlinks are created last, after every file and directory;
//! - if writing fails part way, everything this call made is removed again, in
//!   reverse order, so the target is left empty. If even that fails, the error
//!   is [`MaterializeError::PartlyWritten`], which lists what is left.
//!
//! This is Unix only. Elsewhere [`materialize`] refuses with
//! [`MaterializeError::Unsupported`] and writes nothing: a writer for Windows
//! must use the wide file APIs (gotcha `7595cf33d459`), and none exists yet.
//! Updating a directory that already holds a tree comes with `checkout`.

#[cfg(unix)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::format::{FormatError, Manifest, Mode, Record};
use crate::hash::ContentHash;
use crate::path::{self, PathError};

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("materializing a tree is supported only on Unix so far")]
    Unsupported,
    #[error("the manifest is invalid: {0}")]
    Invalid(#[from] FormatError),
    #[error("{path:?} needs the file {hash}, which isn't at hand")]
    Missing { path: String, hash: ContentHash },
    #[error("{path:?}: the file at hand doesn't match {hash} and its size")]
    Mismatch { path: String, hash: ContentHash },
    #[error("{path:?} is a symlink with an invalid target: {error}")]
    Target { path: String, error: PathError },
    #[error("{0} is not an empty directory")]
    NotEmpty(PathBuf),
    #[error("writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{error}; and these paths couldn't be removed again, so the target holds them: {left:?}")]
    PartlyWritten { error: Box<MaterializeError>, left: Vec<String> },
}

/// What [`materialize`] made, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Made {
    Dir,
    File,
    Symlink,
}

/// Writes `manifest` into `target`, an existing empty directory, taking each
/// file's bytes from `files` by content hash. Everything is checked before the
/// first write.
pub fn materialize(
    manifest: &Manifest,
    files: impl Fn(&ContentHash) -> Option<Vec<u8>>,
    target: &Path,
) -> Result<(), MaterializeError> {
    manifest.validate()?;
    let mut contents = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let Some(bytes) = files(&entry.hash) else {
            return Err(MaterializeError::Missing { path: entry.path.clone(), hash: entry.hash });
        };
        if ContentHash::of(&bytes) != entry.hash || bytes.len() as u64 != entry.size {
            return Err(MaterializeError::Mismatch { path: entry.path.clone(), hash: entry.hash });
        }
        if entry.mode == Mode::Symlink
            && let Err(error) = path::validate_symlink_target(&bytes)
        {
            return Err(MaterializeError::Target { path: entry.path.clone(), error });
        }
        contents.push(bytes);
    }
    write(manifest, &contents, target, None).map(|_| ())
}

#[cfg(not(unix))]
fn write(
    _manifest: &Manifest,
    _contents: &[Vec<u8>],
    _target: &Path,
    _fail_after: Option<usize>,
) -> Result<Vec<(String, Made)>, MaterializeError> {
    Err(MaterializeError::Unsupported)
}

/// Writes the checked contents and returns what it made, in order. On an error,
/// removes it all again. `fail_after` makes the write fail once that many things
/// are made, for tests.
#[cfg(unix)]
fn write(
    manifest: &Manifest,
    contents: &[Vec<u8>],
    target: &Path,
    fail_after: Option<usize>,
) -> Result<Vec<(String, Made)>, MaterializeError> {
    use std::os::fd::OwnedFd;

    use rustix::fs::{self as fs, CWD, Dir, OFlags};

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let shown = target.display().to_string();
    let root = fs::openat(CWD, target, dir_flags, fs::Mode::empty()).map_err(io(&shown))?;
    for entry in Dir::read_from(&root).map_err(io(&shown))? {
        let name = entry.map_err(io(&shown))?;
        if !matches!(name.file_name().to_bytes(), b"." | b"..") {
            return Err(MaterializeError::NotEmpty(target.to_owned()));
        }
    }

    // Directories opened so far, by path from the root ("" is the root).
    let mut dirs: BTreeMap<String, OwnedFd> = BTreeMap::new();
    dirs.insert(String::new(), root);
    let mut made = Vec::new();
    match write_all(manifest, contents, &mut dirs, &mut made, fail_after) {
        Ok(()) => Ok(made),
        Err(error) => {
            let left = undo(&dirs, &made);
            if left.is_empty() {
                Err(error)
            } else {
                Err(MaterializeError::PartlyWritten { error: Box::new(error), left })
            }
        }
    }
}

/// Removes what `made` lists, last first, through the directories already open.
/// Returns what couldn't be removed.
#[cfg(unix)]
fn undo(dirs: &BTreeMap<String, std::os::fd::OwnedFd>, made: &[(String, Made)]) -> Vec<String> {
    use rustix::fs::{self as fs, AtFlags};

    let mut left = Vec::new();
    for (path, kind) in made.iter().rev() {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let flags = if *kind == Made::Dir { AtFlags::REMOVEDIR } else { AtFlags::empty() };
        if fs::unlinkat(&dirs[parent], name, flags).is_err() {
            left.push(path.clone());
        }
    }
    left.reverse();
    left
}

#[cfg(unix)]
fn write_all(
    manifest: &Manifest,
    contents: &[Vec<u8>],
    dirs: &mut BTreeMap<String, std::os::fd::OwnedFd>,
    made: &mut Vec<(String, Made)>,
    fail_after: Option<usize>,
) -> Result<(), MaterializeError> {
    use std::io::Write;

    use rustix::fs::{self as fs, OFlags};

    let basename = |path: &str| path.rsplit('/').next().expect("split always yields one part").to_owned();
    let check = |made: &Vec<(String, Made)>| match fail_after {
        Some(n) if made.len() >= n => Err(MaterializeError::Io {
            path: made.last().map_or_else(String::new, |(p, _)| p.clone()),
            source: std::io::Error::other("injected failure"),
        }),
        _ => Ok(()),
    };

    let file_flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    for (entry, bytes) in manifest.entries.iter().zip(contents) {
        let mode = match entry.mode {
            Mode::File => 0o644,
            Mode::Executable => 0o755,
            Mode::Symlink => continue,
        };
        let parent = open_parent(dirs, made, &entry.path)?;
        check(made)?;
        let fd = fs::openat(&dirs[&parent], basename(&entry.path), file_flags, fs::Mode::from_raw_mode(0o600))
            .map_err(io(&entry.path))?;
        made.push((entry.path.clone(), Made::File));
        check(made)?;
        fs::fchmod(&fd, fs::Mode::from_raw_mode(mode)).map_err(io(&entry.path))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|source| MaterializeError::Io { path: entry.path.clone(), source })?;
    }
    // Symlinks last, so no file or directory is ever created through one.
    for (entry, target_bytes) in manifest.entries.iter().zip(contents) {
        if entry.mode != Mode::Symlink {
            continue;
        }
        let parent = open_parent(dirs, made, &entry.path)?;
        check(made)?;
        let link_target = std::str::from_utf8(target_bytes).expect("targets were checked to be UTF-8");
        fs::symlinkat(link_target, &dirs[&parent], basename(&entry.path)).map_err(io(&entry.path))?;
        made.push((entry.path.clone(), Made::Symlink));
        check(made)?;
    }
    Ok(())
}

#[cfg(unix)]
fn io(path: &str) -> impl FnOnce(rustix::io::Errno) -> MaterializeError {
    let path = path.to_owned();
    move |e| MaterializeError::Io { path, source: e.into() }
}

/// Opens (creating as needed, at exactly 0755) the directory holding `path`, one
/// component at a time from the root, never following a symlink, and returns its
/// key in `dirs`. Each directory it creates goes into `made`.
#[cfg(unix)]
fn open_parent(
    dirs: &mut BTreeMap<String, std::os::fd::OwnedFd>,
    made: &mut Vec<(String, Made)>,
    path: &str,
) -> Result<String, MaterializeError> {
    use rustix::fs::{self as fs, OFlags};

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut at = String::new();
    let components: Vec<&str> = path.split('/').collect();
    for name in &components[..components.len() - 1] {
        let next = if at.is_empty() { (*name).to_owned() } else { format!("{at}/{name}") };
        if !dirs.contains_key(&next) {
            let parent = &dirs[&at];
            let created = match fs::mkdirat(parent, *name, fs::Mode::from_raw_mode(0o755)) {
                Ok(()) => {
                    made.push((next.clone(), Made::Dir));
                    true
                }
                Err(rustix::io::Errno::EXIST) => false,
                Err(e) => return Err(io(&next)(e)),
            };
            let fd = fs::openat(parent, *name, dir_flags, fs::Mode::empty()).map_err(io(&next))?;
            if created {
                fs::fchmod(&fd, fs::Mode::from_raw_mode(0o755)).map_err(io(&next))?;
            }
            dirs.insert(next.clone(), fd);
        }
        at = next;
    }
    Ok(at)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::format::Entry;

    fn tree(files: &[(&str, Mode, &[u8])]) -> (Manifest, Vec<Vec<u8>>) {
        let entries = files
            .iter()
            .map(|(path, mode, bytes)| Entry {
                path: (*path).to_owned(),
                mode: *mode,
                hash: ContentHash::of(bytes),
                size: bytes.len() as u64,
            })
            .collect();
        let manifest = Manifest::new(entries);
        let contents = manifest
            .entries
            .iter()
            .map(|e| files.iter().find(|(p, _, _)| *p == e.path).expect("listed").2.to_vec())
            .collect();
        (manifest, contents)
    }

    fn sample() -> (Manifest, Vec<Vec<u8>>) {
        tree(&[
            ("a", Mode::Symlink, b"b/f"),
            ("b/f", Mode::File, b"f"),
            ("b/l", Mode::Symlink, b"f"),
            ("c/d/x", Mode::Executable, b"x"),
            ("z", Mode::File, b"z"),
        ])
    }

    // Note fefde0fa66f2 item 5: this fails if symlinks are written before files
    // or directories, whatever the paths' order.
    #[test]
    fn symlinks_are_made_after_every_file_and_directory() {
        let (manifest, contents) = sample();
        let dir = tempfile::tempdir().expect("a target");
        let made = write(&manifest, &contents, dir.path(), None).expect("write");
        let kinds: Vec<Made> = made.iter().map(|(_, k)| *k).collect();
        let first_link = kinds.iter().position(|k| *k == Made::Symlink).expect("symlinks are made");
        assert!(kinds[first_link..].iter().all(|k| *k == Made::Symlink), "{made:?}");
        assert_eq!(kinds.iter().filter(|k| **k == Made::Symlink).count(), 2);
        assert!(made.iter().any(|(p, k)| p == "c/d" && *k == Made::Dir));
    }

    // Note fefde0fa66f2 item 4: a failure at any point leaves the target empty.
    #[test]
    fn a_failure_part_way_leaves_nothing_behind() {
        let (manifest, contents) = sample();
        let total = {
            let dir = tempfile::tempdir().expect("a target");
            write(&manifest, &contents, dir.path(), None).expect("write").len()
        };
        for n in 1..=total {
            let dir = tempfile::tempdir().expect("a target");
            let result = write(&manifest, &contents, dir.path(), Some(n));
            assert!(matches!(result, Err(MaterializeError::Io { .. })), "failing after {n}: {result:?}");
            assert!(std::fs::read_dir(dir.path()).expect("read").next().is_none(), "failing after {n} left files");
        }
    }
}
