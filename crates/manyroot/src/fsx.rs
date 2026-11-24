//! Reading and writing inside a replica without following a symlink: every directory below the one
//! given is opened with `O_DIRECTORY | O_NOFOLLOW` and every file with `O_NOFOLLOW | O_NONBLOCK`,
//! relative to its directory's descriptor, and a file's type is checked on the open descriptor, so a
//! link or a FIFO swapped in after a listing is refused rather than followed (findings
//! `58b9903c1820`, `99f862b78057`). The directory a [`Dir`] is first opened at is followed: it's the
//! replica the caller named.
//!
//! A refused entry gives an [`io::Error`] of kind [`io::ErrorKind::InvalidData`], whose message says
//! why: [`NOT_A_DIR`] or [`NOT_A_FILE`].

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const NOT_A_DIR: &str = "not a directory (or a symlink)";
pub(crate) const NOT_A_FILE: &str = "not a regular file (or a symlink)";

/// What a directory listing says an entry is, never following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Other,
}

fn refused(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// Whether `e` is a refusal from this module, and which.
pub(crate) fn refusal(e: &io::Error) -> Option<String> {
    (e.kind() == io::ErrorKind::InvalidData).then(|| e.to_string())
}

/// A directory, held open on Unix, and its path, for messages only.
pub(crate) struct Dir {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

/// A new file name, `.tmp-<pid>-<tag>-<random>`: a dot name, so listings skip it, and no `.json`.
fn temp_name(tag: &str) -> io::Result<String> {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(format!("no randomness for a temporary name: {e}")))?;
    Ok(format!(".tmp-{}-{tag}-{}", std::process::id(), hex::encode(nonce)))
}

fn write_synced(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

impl Dir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Writes `bytes` to a new file in this directory, synced, under a random temporary name, which
    /// it returns. `mode` is the file's permissions on Unix.
    pub(crate) fn write_temp(&self, tag: &str, bytes: &[u8], mode: u32) -> io::Result<String> {
        let tmp = temp_name(tag)?;
        let file = self.create_new(&tmp, mode)?;
        if let Err(e) = write_synced(file, bytes) {
            let _ = self.remove(&tmp);
            return Err(e);
        }
        Ok(tmp)
    }

    /// Writes a new file `name` whole or not at all: the bytes go to a synced temporary file here,
    /// which is then linked in place. The link never replaces what's at `name`, nor follows a symlink
    /// there, a dangling one included: either gives [`io::ErrorKind::AlreadyExists`]. The temporary
    /// file is always removed.
    pub(crate) fn write_new(&self, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
        let tmp = self.write_temp("new", bytes, mode)?;
        let done = self.link(&tmp, name);
        let removed = self.remove(&tmp);
        done.and(removed)
    }

    /// Writes `name` by a synced temporary file renamed over it, which replaces whatever is there,
    /// a symlink included, rather than writing through it. A failed write leaves no temporary file.
    pub(crate) fn replace(&self, name: &str, tag: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
        let tmp = self.write_temp(tag, bytes, mode)?;
        let done = self.rename(&tmp, name);
        if done.is_err() {
            let _ = self.remove(&tmp);
        }
        done
    }

    /// The bytes of the regular file `name`, at most `limit` and one more (so a caller sees it's over).
    pub(crate) fn read(&self, name: &str, limit: usize) -> io::Result<Vec<u8>> {
        let file = self.open_file(name)?;
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use rustix::fs::{self as fs, AtFlags, CWD, FileType, Mode, OFlags};

    use super::{Dir, Kind, NOT_A_DIR, NOT_A_FILE, refused};

    const DIR: OFlags = OFlags::RDONLY.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

    /// `ELOOP` (a symlink under `O_NOFOLLOW`) and `ENOTDIR` (not a directory under `O_DIRECTORY`) are
    /// refusals; every other error is passed on as it is.
    fn classify(e: rustix::io::Errno, why: &str) -> io::Error {
        if e == rustix::io::Errno::LOOP || e == rustix::io::Errno::NOTDIR { refused(why) } else { e.into() }
    }

    fn kind(t: FileType) -> Kind {
        match t {
            FileType::RegularFile => Kind::File,
            FileType::Directory => Kind::Dir,
            _ => Kind::Other,
        }
    }

    impl Dir {
        /// Opens `path` as a directory, following it if it is a link: the replica the caller named.
        pub(crate) fn open(path: &Path) -> io::Result<Dir> {
            let fd = fs::openat(CWD, path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?;
            Ok(Dir { path: path.to_owned(), fd })
        }

        /// The directory `name` in this one, refusing a symlink or anything but a directory.
        pub(crate) fn sub(&self, name: &str) -> io::Result<Dir> {
            let fd = fs::openat(&self.fd, name, DIR, Mode::empty()).map_err(|e| classify(e, NOT_A_DIR))?;
            Ok(Dir { path: self.path.join(name), fd })
        }

        /// [`Dir::sub`], creating it first if it doesn't exist.
        pub(crate) fn sub_or_create(&self, name: &str, mode: u32) -> io::Result<Dir> {
            match fs::mkdirat(&self.fd, name, Mode::from_raw_mode(mode as _)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => self.sub(name),
                Err(e) => Err(e.into()),
            }
        }

        /// Every entry but `.` and `..`, with its type as the listing says, never following a link.
        pub(crate) fn entries(&self) -> io::Result<Vec<(std::ffi::OsString, Kind)>> {
            let mut out = Vec::new();
            for entry in fs::Dir::read_from(&self.fd)? {
                let entry = entry?;
                let name = OsStr::from_bytes(entry.file_name().to_bytes());
                if name == "." || name == ".." {
                    continue;
                }
                let t = match entry.file_type() {
                    FileType::Unknown => {
                        FileType::from_raw_mode(fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)?.st_mode)
                    }
                    t => t,
                };
                out.push((name.to_owned(), kind(t)));
            }
            Ok(out)
        }

        /// Opens the regular file `name` to read: `O_NOFOLLOW`, and `O_NONBLOCK` so a FIFO can't block
        /// the open, then checks the open descriptor's type.
        pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = fs::openat(&self.fd, name, flags, Mode::empty()).map_err(|e| classify(e, NOT_A_FILE))?;
            if FileType::from_raw_mode(fs::fstat(&fd)?.st_mode) != FileType::RegularFile {
                return Err(refused(NOT_A_FILE));
            }
            Ok(File::from(fd))
        }

        pub(crate) fn create_new(&self, name: &str, mode: u32) -> io::Result<File> {
            let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            Ok(File::from(fs::openat(&self.fd, name, flags, Mode::from_raw_mode(mode as _))?))
        }

        pub(crate) fn link(&self, from: &str, to: &str) -> io::Result<()> {
            Ok(fs::linkat(&self.fd, from, &self.fd, to, AtFlags::empty())?)
        }

        pub(crate) fn rename(&self, from: &str, to: &str) -> io::Result<()> {
            Ok(fs::renameat(&self.fd, from, &self.fd, to)?)
        }

        pub(crate) fn remove(&self, name: &str) -> io::Result<()> {
            Ok(fs::unlinkat(&self.fd, name, AtFlags::empty())?)
        }
    }
}

/// Elsewhere, the same checks by path: each is made just before the step it guards.
#[cfg(not(unix))]
mod imp {
    use std::fs::{self, File};
    use std::io;
    use std::path::Path;

    use super::{Dir, Kind, NOT_A_DIR, NOT_A_FILE, refused};

    fn kind(t: fs::FileType) -> Kind {
        if t.is_file() {
            Kind::File
        } else if t.is_dir() {
            Kind::Dir
        } else {
            Kind::Other
        }
    }

    impl Dir {
        pub(crate) fn open(path: &Path) -> io::Result<Dir> {
            if !fs::metadata(path)?.is_dir() {
                return Err(refused(NOT_A_DIR));
            }
            Ok(Dir { path: path.to_owned() })
        }

        pub(crate) fn sub(&self, name: &str) -> io::Result<Dir> {
            let path = self.path.join(name);
            if !fs::symlink_metadata(&path)?.is_dir() {
                return Err(refused(NOT_A_DIR));
            }
            Ok(Dir { path })
        }

        pub(crate) fn sub_or_create(&self, name: &str, _mode: u32) -> io::Result<Dir> {
            match fs::create_dir(self.path.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            self.sub(name)
        }

        pub(crate) fn entries(&self) -> io::Result<Vec<(std::ffi::OsString, Kind)>> {
            let mut out = Vec::new();
            for entry in fs::read_dir(&self.path)? {
                let entry = entry?;
                out.push((entry.file_name(), kind(entry.file_type()?)));
            }
            Ok(out)
        }

        pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
            let path = self.path.join(name);
            if !fs::symlink_metadata(&path)?.is_file() {
                return Err(refused(NOT_A_FILE));
            }
            let file = File::open(&path)?;
            if !file.metadata()?.is_file() {
                return Err(refused(NOT_A_FILE));
            }
            Ok(file)
        }

        pub(crate) fn create_new(&self, name: &str, _mode: u32) -> io::Result<File> {
            fs::OpenOptions::new().write(true).create_new(true).open(self.path.join(name))
        }

        pub(crate) fn link(&self, from: &str, to: &str) -> io::Result<()> {
            fs::hard_link(self.path.join(from), self.path.join(to))
        }

        pub(crate) fn rename(&self, from: &str, to: &str) -> io::Result<()> {
            fs::rename(self.path.join(from), self.path.join(to))
        }

        pub(crate) fn remove(&self, name: &str) -> io::Result<()> {
            fs::remove_file(self.path.join(name))
        }
    }
}
