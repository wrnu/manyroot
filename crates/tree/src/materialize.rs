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
//! - symlinks are created last, after every file and directory.
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
    write(manifest, &contents, target)
}

#[cfg(not(unix))]
fn write(_manifest: &Manifest, _contents: &[Vec<u8>], _target: &Path) -> Result<(), MaterializeError> {
    Err(MaterializeError::Unsupported)
}

#[cfg(unix)]
fn write(manifest: &Manifest, contents: &[Vec<u8>], target: &Path) -> Result<(), MaterializeError> {
    use std::io::Write;
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
    let basename = |path: &str| path.rsplit('/').next().expect("split always yields one part").to_owned();

    let file_flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    for (entry, bytes) in manifest.entries.iter().zip(contents) {
        let mode = match entry.mode {
            Mode::File => 0o644,
            Mode::Executable => 0o755,
            Mode::Symlink => continue,
        };
        let parent = open_parent(&mut dirs, &entry.path)?;
        let fd = fs::openat(&dirs[&parent], basename(&entry.path), file_flags, fs::Mode::from_raw_mode(0o600))
            .map_err(io(&entry.path))?;
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
        let parent = open_parent(&mut dirs, &entry.path)?;
        let link_target = std::str::from_utf8(target_bytes).expect("targets were checked to be UTF-8");
        fs::symlinkat(link_target, &dirs[&parent], basename(&entry.path)).map_err(io(&entry.path))?;
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
/// key in `dirs`.
#[cfg(unix)]
fn open_parent(dirs: &mut BTreeMap<String, std::os::fd::OwnedFd>, path: &str) -> Result<String, MaterializeError> {
    use rustix::fs::{self as fs, OFlags};

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut at = String::new();
    let components: Vec<&str> = path.split('/').collect();
    for name in &components[..components.len() - 1] {
        let next = if at.is_empty() { (*name).to_owned() } else { format!("{at}/{name}") };
        if !dirs.contains_key(&next) {
            let parent = &dirs[&at];
            let created = match fs::mkdirat(parent, *name, fs::Mode::from_raw_mode(0o755)) {
                Ok(()) => true,
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
