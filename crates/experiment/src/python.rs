//! The Python execution profile (`docs/design/eval-suite.md`, Isolation; phase2-task.md's Candidate B): agent
//! code runs only here, in the pinned interpreter, under the `python` sandbox profile, with the operating
//! system's limits, in a scratch directory made fresh for each run and removed after.
//!
//! - **The interpreter** is pinned by path, version and a digest of everything it can read and run
//!   ([`Python::pin`]): the real executable, never a shim (gotcha `e5274831`), run as `python -I -S -B -X utf8`
//!   with a cleared environment. Each run first checks its files haven't changed since ([`Python::check`]).
//! - **Reads**: the interpreter's own installation and the libraries it loads, found from their Mach-O load
//!   commands when it is pinned; the system's time zone data; the scratch directory. Nothing of the owner's:
//!   no other path's metadata, and no other process's information.
//! - **Writes**: the scratch directory only. **No fork, no exec** but the interpreter's own, **no network**.
//! - **Limits**: CPU time, file size and open files as rlimits; memory as the child's footprint, which the
//!   parent reads every [`POLL`] and kills past its limit, since macOS doesn't enforce `RLIMIT_AS`; output
//!   read up to its limit, then killed; wall time, then killed; the scratch directory's size and files,
//!   checked every [`SCRATCH_POLL`], then killed. Killing ends the child's process group.
//!
//! [`run_tests`] runs a tree's public tests here: feedback for the agent, never a grade.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::project::{Limits, Project};
use crate::sandbox::{PythonPaths, Sandbox};

/// How often a running child's footprint and wall time are checked.
pub const POLL: Duration = Duration::from_millis(20);
/// The most open files a child may have.
const MAX_OPEN_FILES: u64 = 256;
/// How often the scratch directory's size is checked.
pub const SCRATCH_POLL: Duration = Duration::from_millis(100);
/// The most files and directories a child's scratch directory may hold.
pub const MAX_SCRATCH_FILES: u64 = 10_000;
/// The runner [`run_tests`] starts, as it is in this binary.
const RUN_TESTS: &str = include_str!("python/run_tests.py");
/// What the interpreter is asked when it is pinned: its version and installation.
const PROBE: &str = "import json, sys, sysconfig; print(json.dumps({'version': '%d.%d.%d' % sys.version_info[:3], \
                     'prefix': sys.base_prefix, 'stdlib': sysconfig.get_path('stdlib'), \
                     'platstdlib': sysconfig.get_path('platstdlib')}))";

/// The pinned interpreter.
#[derive(Debug, Clone, Serialize)]
pub struct Python {
    /// The real executable, resolved.
    pub executable: PathBuf,
    /// blake3 over every file of the installation but `site-packages` (the standard library, its compiled
    /// `__pycache__`, the framework's libpython, the executable) and the libraries outside it: what it can read
    /// and run. A plan records it, and a pin refuses an interpreter whose digest isn't the one expected.
    pub digest: String,
    /// The same files' paths and metadata (inode, size, mode, modification and change times), to check before
    /// each run that none has changed: a change time can't be set back.
    #[serde(skip)]
    stamp: String,
    /// Such as `3.12.16`.
    pub version: String,
    /// Its installation, which the profile lets it read.
    pub prefix: PathBuf,
    /// The libraries outside the installation it loads, which the profile lets it read one by one.
    pub libraries: Vec<PathBuf>,
    /// The names it loads them by where those aren't the resolved paths, such as Homebrew's
    /// `/opt/homebrew/opt/sqlite/lib/libsqlite3.dylib`: the profile lets it look up each link on the way.
    pub links: Vec<PathBuf>,
}

impl Python {
    /// Pins the interpreter at `path` for a project written for Python `minor` (such as `3.12`): resolves it,
    /// hashes it, asks it its version and installation, and finds the libraries it and its standard
    /// library's extension modules load. Refuses another minor version.
    ///
    /// A macOS framework build's `bin/python3.x` only starts the interpreter in its `Python.app`, a second
    /// exec the profile refuses; so that interpreter is the one pinned and run. With `expected`, refuses an
    /// interpreter whose [`Python::digest`] is another.
    pub fn pin(path: &Path, minor: &str, expected: Option<&str>) -> Result<Python, String> {
        let given = std::fs::canonicalize(path).map_err(|e| format!("python {}: {e}", path.display()))?;
        let probe = probe(&given)?;
        let app = Path::new(&probe.prefix).join("Resources/Python.app/Contents/MacOS/Python");
        let (executable, probe) = match std::fs::canonicalize(&app) {
            Ok(app) if app != given => (app.clone(), self::probe(&app)?),
            _ => (given, probe),
        };
        let Probe { version, prefix, stdlib, platstdlib } = probe;
        if version.rsplit_once('.').map(|(m, _)| m) != Some(minor) {
            return Err(format!("python {} is {version}; the project is written for {minor}", executable.display()));
        }
        let prefix = std::fs::canonicalize(prefix).map_err(|e| format!("python's prefix: {e}"))?;
        let mut objects = vec![executable.clone()];
        for dir in [stdlib, platstdlib] {
            objects.extend(extension_modules(Path::new(&dir)));
        }
        let (libraries, links) = libraries(&objects, &prefix)?;
        let digest = fingerprint(&prefix, &executable, &libraries, Fingerprint::Contents)?;
        if let Some(expected) = expected
            && expected != digest
        {
            return Err(format!("python {}'s digest is {digest}, not the expected {expected}", executable.display()));
        }
        let stamp = fingerprint(&prefix, &executable, &libraries, Fingerprint::Metadata)?;
        Ok(Python { digest, stamp, executable, version, prefix, libraries, links })
    }

    /// Refuses if any of the interpreter's files has changed since it was pinned.
    pub fn check(&self) -> Result<(), String> {
        if fingerprint(&self.prefix, &self.executable, &self.libraries, Fingerprint::Metadata)? != self.stamp {
            return Err(format!("python {}'s files have changed since it was pinned", self.executable.display()));
        }
        Ok(())
    }

    /// Runs `args` after the interpreter's own flags in `scratch`, under the profile and `limits`.
    pub fn run(&self, scratch: &Scratch, args: &[OsString], limits: &Limits) -> Result<Outcome, String> {
        if !cfg!(target_os = "macos") {
            return Err("agent code runs only under macOS's sandbox-exec; this system has none".into());
        }
        self.check()?;
        run_limited(self.command(scratch, args)?, limits, &scratch.dir)
    }

    /// The command [`Python::run`] runs: `args` in `scratch` under the profile, with nothing of this process's
    /// environment.
    fn command(&self, scratch: &Scratch, args: &[OsString]) -> Result<Command, String> {
        let sandbox = Sandbox::python(&PythonPaths {
            python: &self.executable,
            prefix: &self.prefix,
            libraries: &self.libraries,
            links: &self.links,
            scratch: &scratch.dir,
        })?;
        let mut cmd = sandbox.command(&self.executable);
        cmd.args(["-I", "-S", "-B", "-X", "utf8"])
            .args(args)
            .current_dir(&scratch.dir)
            .env_clear()
            .env("HOME", &scratch.dir)
            .env("TMPDIR", &scratch.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fingerprint {
    Contents,
    Metadata,
}

/// blake3 over every entry under `prefix` but `site-packages` directories, in path order, then `executable` if
/// it is outside, and each of `libraries`: each by its path and, as `what` says, its contents' blake3 or its
/// metadata. A symlink is taken as its target's text, never followed.
fn fingerprint(prefix: &Path, executable: &Path, libraries: &[PathBuf], what: Fingerprint) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let mut paths = Vec::new();
    let mut stack = vec![prefix.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_dir() && path.file_name().is_some_and(|n| n != "site-packages") {
                stack.push(path.clone());
            }
            if !(meta.is_dir() && path.file_name().is_some_and(|n| n == "site-packages")) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    if !executable.starts_with(prefix) {
        paths.push(executable.to_path_buf());
    }
    paths.extend(libraries.iter().cloned());
    let mut listing = blake3::Hasher::new();
    for path in paths {
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let entry = match what {
            Fingerprint::Metadata => format!(
                "{} {} {} {} {} {}",
                meta.ino(),
                meta.size(),
                meta.mode(),
                meta.mtime_nsec() + meta.mtime() * 1_000_000_000,
                meta.ctime_nsec() + meta.ctime() * 1_000_000_000,
                meta.file_type().is_symlink()
            ),
            Fingerprint::Contents if meta.is_file() => {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                blake3::hash(&bytes).to_hex().to_string()
            }
            Fingerprint::Contents if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                format!("-> {}", target.display())
            }
            Fingerprint::Contents => "dir".into(),
        };
        listing.update(format!("{}\0{entry}\n", path.display()).as_bytes());
    }
    Ok(listing.finalize().to_hex().to_string())
}

/// Opening a directory without following a symlink.
const NOFOLLOW_DIR: rustix::fs::OFlags = rustix::fs::OFlags::RDONLY
    .union(rustix::fs::OFlags::DIRECTORY)
    .union(rustix::fs::OFlags::NOFOLLOW)
    .union(rustix::fs::OFlags::CLOEXEC);

/// The bytes and the entries under `dir`, stopping once either passes its limit. It runs while the code does,
/// so it never acts on a path the code could swap for a symlink (finding b9a13da2a14c): each directory is opened
/// by name in its already open parent without following a symlink, each entry looked at the same way, and
/// nothing is changed. A directory it can't read, such as one the code has taken the owner's access to, counts
/// as past the limit (finding b4014e7dbbd8).
fn usage(dir: &Path, max_bytes: u64) -> (u64, u64) {
    use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, openat, statat};
    use rustix::io::Errno;
    let past = (max_bytes.saturating_add(1), 0);
    let Ok(top) = openat(CWD, dir, NOFOLLOW_DIR, Mode::empty()) else { return past };
    let (mut bytes, mut entries) = (0u64, 0u64);
    let mut stack = vec![top];
    while let Some(fd) = stack.pop() {
        let Ok(list) = Dir::read_from(&fd) else { return past };
        for entry in list {
            let Ok(entry) = entry else { return past };
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            entries += 1;
            match statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => {
                    bytes = bytes.saturating_add(u64::try_from(stat.st_size).unwrap_or(0));
                    if FileType::from_raw_mode(stat.st_mode as _) == FileType::Directory {
                        match openat(&fd, name, NOFOLLOW_DIR, Mode::empty()) {
                            Ok(child) => stack.push(child),
                            // Gone, or swapped for something else since: what it held is still found where it is.
                            Err(Errno::NOENT | Errno::LOOP | Errno::NOTDIR) => {}
                            Err(_) => return past,
                        }
                    }
                }
                Err(Errno::NOENT) => {}
                Err(_) => return past,
            }
            if bytes > max_bytes || entries > MAX_SCRATCH_FILES {
                return (bytes, entries);
            }
        }
    }
    (bytes, entries)
}

/// What an interpreter says of itself.
#[derive(serde::Deserialize)]
struct Probe {
    version: String,
    prefix: String,
    stdlib: String,
    platstdlib: String,
}

/// Asks the interpreter at `executable`, outside any sandbox and with a cleared environment, what it is.
fn probe(executable: &Path) -> Result<Probe, String> {
    let out = Command::new(executable)
        .args(["-I", "-S", "-B", "-c", PROBE])
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running python {}: {e}", executable.display()))?;
    serde_json::from_slice(&out.stdout).map_err(|_| format!("python {} didn't say what it is", executable.display()))
}

/// The `.so` files in `dir` and its subdirectories: the standard library's extension modules.
fn extension_modules(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() && p.file_name().is_some_and(|n| n != "site-packages") => stack.push(p),
                Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "so") => out.push(p),
                _ => {}
            }
        }
    }
    out
}

/// The libraries `objects` load, and those load in turn, resolved, outside `prefix` and the system's (which
/// every profile may read); and the names they're loaded by that aren't those.
fn libraries(objects: &[PathBuf], prefix: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut links = std::collections::BTreeSet::new();
    let mut queue: Vec<PathBuf> = objects.to_vec();
    while let Some(object) = queue.pop() {
        let bytes = std::fs::read(&object).map_err(|e| format!("{}: {e}", object.display()))?;
        for name in macho::dylibs(&bytes).map_err(|e| format!("{}: {e}", object.display()))? {
            if name.starts_with('@') || name.starts_with("/usr/lib/") || name.starts_with("/System/") {
                continue;
            }
            let lib = std::fs::canonicalize(&name).map_err(|e| format!("{} loads {name}: {e}", object.display()))?;
            if lib.starts_with(prefix) {
                continue;
            }
            if lib != Path::new(&name) {
                links.insert(PathBuf::from(&name));
            }
            if seen.insert(lib.clone()) {
                queue.push(lib);
            }
        }
    }
    Ok((seen.into_iter().collect(), links.into_iter().collect()))
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    Code(i32),
    /// Killed by a signal: `SIGXCPU` (24) is the CPU limit, `SIGXFSZ` (25) the file size limit.
    Signal(i32),
    WallTime,
    Memory,
    Output,
    /// Its scratch directory passed its size or [`MAX_SCRATCH_FILES`].
    Scratch,
}

/// A run's end and what it wrote, each stream cut at the output limit.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A directory made fresh for one run, `0700`, in the system's temporary directory, and removed when
/// dropped, whatever the run did to the permissions inside it.
pub struct Scratch {
    /// Resolved, as the sandbox's rules see it.
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new() -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        let tmp = std::fs::canonicalize(std::env::temp_dir()).map_err(|e| format!("the temporary directory: {e}"))?;
        for attempt in 0u32..16 {
            let seed = format!("{:?}{}{attempt}", Instant::now(), std::process::id());
            let dir = tmp.join(format!("bracken-python-{}", &blake3::hash(seed.as_bytes()).to_hex()[..12]));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => return Ok(Scratch { dir }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("a scratch directory in {}: {e}", tmp.display())),
            }
        }
        Err(format!("no fresh scratch directory in {}", tmp.display()))
    }

    /// Copies the tree at `from` into the scratch directory as `name`: regular files and directories only.
    pub fn copy_tree(&self, from: &Path, name: &str) -> Result<PathBuf, String> {
        let to = self.dir.join(name);
        copy_tree(from, &to)?;
        Ok(to)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unlock(&self.dir);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Gives back the owner's access to every directory under `dir`, which the run may have taken away: only once
/// the run has ended, and never through a symlink, each directory changed and opened by name in its open parent.
fn unlock(dir: &Path) {
    use rustix::fs::{AtFlags, CWD, Mode, chmodat, openat};
    let _ = chmodat(CWD, dir, Mode::RWXU, AtFlags::SYMLINK_NOFOLLOW);
    if let Ok(fd) = openat(CWD, dir, NOFOLLOW_DIR, Mode::empty()) {
        unlock_in(&fd);
    }
}

fn unlock_in(fd: &std::os::fd::OwnedFd) {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, chmodat, openat, statat};
    let Ok(list) = Dir::read_from(fd) else { return };
    for entry in list.flatten() {
        let name = entry.file_name();
        let is_dir = statat(fd, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|s| FileType::from_raw_mode(s.st_mode as _) == FileType::Directory);
        if name == c"." || name == c".." || !is_dir {
            continue;
        }
        let _ = chmodat(fd, name, Mode::RWXU, AtFlags::SYMLINK_NOFOLLOW);
        if let Ok(child) = openat(fd, name, NOFOLLOW_DIR, Mode::empty()) {
            unlock_in(&child);
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
        let target = to.join(entry.file_name());
        if meta.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if meta.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("{}: {e}", entry.path().display()))?;
        } else {
            return Err(format!("{} is a symlink or other special file", entry.path().display()));
        }
    }
    Ok(())
}

/// Starts `cmd` in a process group of its own with the rlimits, and waits for it within the rest.
fn run_limited(mut cmd: Command, limits: &Limits, scratch: &Path) -> Result<Outcome, String> {
    use std::os::unix::process::CommandExt;
    let cpu = u64::from(limits.cpu_seconds);
    // What the code may write to one file: as much as it may print, and at least 16 MiB.
    let fsize = limits.output_bytes.max(16 << 20);
    cmd.process_group(0);
    // SAFETY: between fork and exec this only makes setrlimit system calls, which allocate nothing and take
    // no lock.
    unsafe {
        cmd.pre_exec(move || {
            use rustix::process::{Resource, Rlimit, setrlimit};
            let limit = |soft: u64, hard: u64| Rlimit { current: Some(soft), maximum: Some(hard) };
            setrlimit(Resource::Cpu, limit(cpu, cpu + 1))?;
            setrlimit(Resource::Fsize, limit(fsize, fsize))?;
            setrlimit(Resource::Nofile, limit(MAX_OPEN_FILES, MAX_OPEN_FILES))?;
            setrlimit(Resource::Core, limit(0, 0))?;
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("starting python: {e}"))?;
    let cap = usize::try_from(limits.output_bytes).unwrap_or(usize::MAX);
    let full = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stdout = reader(child.stdout.take().expect("piped"), cap, full.clone());
    let stderr = reader(child.stderr.take().expect("piped"), cap, full.clone());
    let deadline = Instant::now() + Duration::from_secs(limits.wall_seconds.into());
    let memory = u64::from(limits.memory_mb) << 20;
    let scratch_bytes = u64::from(limits.scratch_mb) << 20;
    let mut next_scratch = Instant::now();
    let ended = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("waiting for python: {e}"))? {
            break exit_of(status);
        }
        let over = if full.load(std::sync::atomic::Ordering::SeqCst) {
            Some(Exit::Output)
        } else if Instant::now() > deadline {
            Some(Exit::WallTime)
        } else if footprint(child.id()).is_some_and(|bytes| bytes > memory) {
            Some(Exit::Memory)
        } else if Instant::now() >= next_scratch && {
            next_scratch = Instant::now() + SCRATCH_POLL;
            let (bytes, entries) = usage(scratch, scratch_bytes);
            bytes > scratch_bytes || entries > MAX_SCRATCH_FILES
        } {
            Some(Exit::Scratch)
        } else {
            None
        };
        if let Some(why) = over {
            kill(&mut child);
            break why;
        }
        std::thread::sleep(POLL);
    };
    // Not the group again: once reaped, its id may be another process group's, and with no fork it held only the child.
    let (out, err) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    let exit = if full.load(std::sync::atomic::Ordering::SeqCst) { Exit::Output } else { ended };
    Ok(Outcome { exit, stdout: out, stderr: err })
}

/// Reads `from` up to `cap` bytes on its own thread. Past the cap it sets `full` and stops, and the run is
/// killed.
fn reader(
    mut from: impl Read + Send + 'static,
    cap: usize,
    full: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => return out,
                Ok(n) if out.len() + n > cap => {
                    out.extend_from_slice(&buf[..cap - out.len()]);
                    full.store(true, std::sync::atomic::Ordering::SeqCst);
                    return out;
                }
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
}

fn exit_of(status: std::process::ExitStatus) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => Exit::Code(code),
        (None, Some(signal)) => Exit::Signal(signal),
        (None, None) => Exit::Code(-1),
    }
}

fn kill(child: &mut Child) {
    kill_group(child);
    let _ = child.wait();
}

/// Kills the child's process group: the child and anything that shares it.
fn kill_group(child: &Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// The child's memory footprint in bytes, as macOS counts it for memory pressure; `None` once it has gone.
#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> Option<u64> {
    /// `rusage_info_v0` from `<sys/resource.h>`.
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV0 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        proc_start_abstime: u64,
        proc_exit_abstime: u64,
    }
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
    }
    let mut info = RusageInfoV0::default();
    // SAFETY: `info` is a `rusage_info_v0`, the flavor 0 (`RUSAGE_INFO_V0`) asks for, and lives through the call.
    let ok = unsafe { proc_pid_rusage(pid as i32, 0, &mut info) } == 0;
    ok.then_some(info.phys_footprint)
}

#[cfg(not(target_os = "macos"))]
fn footprint(_pid: u32) -> Option<u64> {
    None
}

/// What [`run_tests`] found: each test by id and outcome (`pass`, `fail`, `error` or `skip`).
#[derive(Debug, Clone, Serialize)]
pub struct TestRun {
    /// Whether the runner reported at all: if not, it failed before the tests ran, and `output` says why.
    pub reported: bool,
    pub tests: Vec<(String, String)>,
    pub exit: Exit,
    /// The tests' output, cut to its last [`TAIL`] bytes.
    pub output: String,
}

/// How much of the tests' output [`TestRun`] keeps.
pub const TAIL: usize = 16 << 10;

impl TestRun {
    pub fn count(&self, outcome: &str) -> usize {
        self.tests.iter().filter(|(_, o)| o == outcome).count()
    }
}

/// Runs `project`'s public tests on a copy of `tree` (laid out as the seed is) with the pinned interpreter,
/// under the profile and the project's limits.
pub fn run_tests(python: &Python, project: &Project, tree: &Path) -> Result<TestRun, String> {
    let scratch = Scratch::new()?;
    let copy = scratch.copy_tree(tree, "tree")?;
    let runner = scratch.dir.join("run_tests.py");
    std::fs::write(&runner, RUN_TESTS).map_err(|e| format!("writing the runner: {e}"))?;
    let tests = project.public_tests.strip_prefix(&project.seed).expect("checked to be in the seed");
    let marker = format!("bracken-run-tests-{}:", blake3::hash(scratch.dir.as_os_str().as_encoded_bytes()).to_hex());
    let args = [runner.into_os_string(), copy.into_os_string(), tests.as_os_str().to_owned(), marker.clone().into()];
    let outcome = python.run(&scratch, &args, &project.limits)?;
    let stdout = String::from_utf8_lossy(&outcome.stdout);
    let reported = stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(&marker))
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|v| serde_json::from_value::<Vec<(String, String)>>(v["tests"].clone()).ok());
    let (reported, tests) = (reported.is_some(), reported.unwrap_or_default());
    let err = String::from_utf8_lossy(&outcome.stderr);
    let output = err[err.len().saturating_sub(TAIL)..].to_string();
    Ok(TestRun { reported, tests, exit: outcome.exit, output })
}

/// Reading Mach-O load commands: just enough to list the libraries a binary loads.
mod macho {
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const FAT_MAGIC: u32 = 0xcafe_babe;
    const LC_LOAD_DYLIB: u32 = 0xc;
    const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
    const LC_REEXPORT_DYLIB: u32 = 0x8000_001f;
    const LC_LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;
    /// This machine's CPU type: arm64 or x86_64.
    const CPU_TYPE: u32 = if cfg!(target_arch = "aarch64") { 0x0100_000c } else { 0x0100_0007 };

    fn u32_at(b: &[u8], at: usize, big: bool) -> Result<u32, String> {
        let s: [u8; 4] = b.get(at..at + 4).and_then(|s| s.try_into().ok()).ok_or("cut short")?;
        Ok(if big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    }

    /// The install names of the libraries a 64-bit Mach-O binary loads; for a universal binary, those of this
    /// machine's slice.
    pub fn dylibs(b: &[u8]) -> Result<Vec<String>, String> {
        if u32_at(b, 0, true)? == FAT_MAGIC {
            let n = u32_at(b, 4, true)? as usize;
            for i in 0..n.min(64) {
                let at = 8 + i * 20;
                if u32_at(b, at, true)? == CPU_TYPE {
                    let (offset, size) = (u32_at(b, at + 8, true)? as usize, u32_at(b, at + 12, true)? as usize);
                    return dylibs(b.get(offset..offset + size).ok_or("a slice past the end")?);
                }
            }
            return Err("no slice for this machine".into());
        }
        if u32_at(b, 0, false)? != MH_MAGIC_64 {
            return Err("not a 64-bit Mach-O binary".into());
        }
        let ncmds = u32_at(b, 16, false)? as usize;
        let (mut at, mut out) = (32, Vec::new());
        for _ in 0..ncmds {
            let (cmd, size) = (u32_at(b, at, false)?, u32_at(b, at + 4, false)? as usize);
            if size < 8 {
                return Err("a load command shorter than its header".into());
            }
            if [LC_LOAD_DYLIB, LC_LOAD_WEAK_DYLIB, LC_REEXPORT_DYLIB, LC_LOAD_UPWARD_DYLIB].contains(&cmd) {
                let command = b.get(at..at + size).ok_or("a load command past the end")?;
                let name = command.get(u32_at(b, at + 8, false)? as usize..).ok_or("a name past its command")?;
                let name = name.split(|&c| c == 0).next().unwrap_or_default();
                out.push(String::from_utf8(name.to_vec()).map_err(|_| "a library name that isn't UTF-8")?);
            }
            at += size;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
