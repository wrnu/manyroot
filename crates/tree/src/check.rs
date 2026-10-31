//! The check runner (docs/design/weave.md, section 3, *The check runner*).
//!
//! A named check runs one way only:
//! - the manifest is written by [`materialize`](crate::materialize) into a fresh
//!   temporary directory (mode 0700), and the check runs in it. The sandbox may
//!   write only under its `w/`, so once the directory is removed (on every
//!   path, a panic included) nothing sandboxed can create it again;
//! - its argv runs without a shell. `argv[0]` is resolved against the fixed
//!   `PATH` first, so the tree can't pick the binary: a bare name is never
//!   looked up in the tree. A name with a `/` is a path inside the tree, so
//!   neither absolute nor with a `..` component;
//! - its environment holds only `PATH` (the toolchain's `bin`, then the
//!   system's directories), `LANG`, a fresh `HOME` and `TMPDIR`, `CARGO_HOME`
//!   (fresh, whose `config.toml` points cargo at the read-only vendor directory
//!   and sets `offline`), and `RUSTUP_HOME` and `DEVELOPER_DIR` (the Command
//!   Line Tools) when a toolchain is given. Nothing
//!   else is inherited, so no `BRACKEN_*` variable or token path;
//! - it runs under a deny-by-default `sandbox-exec` profile, modelled on the
//!   experiment's (`crates/experiment/src/sandbox.rs`): reads only from the
//!   fresh directory, the system's own directories, the vendor directory and
//!   the toolchain; writes only inside the fresh directory; exec only from the
//!   fresh directory, the toolchain and the system's directories; no network,
//!   loopback and unix sockets included, and no mach-lookup or other IPC.
//!   Metadata (`stat`) stays readable everywhere, as in the experiment, since
//!   tools resolve their paths through directories they can't read. Listing
//!   the process table is denied, and so is reading any process's arguments,
//!   environment, path or `proc_pidinfo`, whatever group it's in;
//! - it has a timeout, and its whole process group is killed when it ends or
//!   times out. The profile denies `setsid` and `setpgid`, so a check can't
//!   leave the group that way. `posix_spawn` with a new session or group still
//!   can, so on every exit path the group is stopped, the check's descendants
//!   are found by parent pid (again until no new ones appear) and stopped
//!   too, and then all are killed. If any had left the group, the run fails
//!   with [`Exit::LeftGroup`]. That holds for a child in a new group or one
//!   that joined another, the runner's included, but only while its parent
//!   chain to the check is alive: an orphan reparented to launchd before the
//!   walk can't be found (weave.md section 3). If one still holds the output
//!   open two seconds after the check ended, the run fails with
//!   [`Exit::HeldOutput`]; one that closed its output fails nothing. Killing
//!   the runner's own group instead would kill its callers' processes too;
//! - **no sandbox, no run**: off macOS, or if the profile isn't applied (a
//!   probe must run under it and be refused a read outside the fresh
//!   directory), [`Runner::run`] refuses and runs nothing.
//!
//! What a result records comes later (step 7's second part). [`recorded_tail`]
//! already prepares the output tail for it: control characters stripped, and
//! withheld as its size and hash if import's content refusals match.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::format::{CheckDefinition, Manifest};
use crate::hash::ContentHash;
use crate::materialize::MaterializeError;
use crate::secrets::{self, KnownKeys, SecretRule};

/// macOS's `sandbox-exec`.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The system's directories on a check's `PATH`, after the toolchain's.
pub const SYSTEM_PATH: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// The developer directory a check's linker uses, with a toolchain.
pub const DEVELOPER_DIR: &str = "/Library/Developer/CommandLineTools";

/// A check's `LANG`.
pub const LANG: &str = "en_US.UTF-8";

/// The deny-by-default profile. `TEMP` is the fresh directory, `VENDOR` the vendor directory.
///
/// Reading another process takes both of the last two rules (note 1301a2adcc85): without the `kern.proc`
/// prefix, `KERN_PROCARGS2` stays open, and `process-info*` must be unqualified, since `(target others)`
/// leaves out the check's own process group, which `posix_spawn`'s `POSIX_SPAWN_SETPGROUP` can make the
/// runner's. A process may still read its own: cargo dies of SIGTRAP without it.
const PROFILE: &str = r#"(version 1)
(deny default)
(allow process-fork)
(allow sysctl-read)
(allow file-read-metadata)
(allow file-read*
  (literal "/") (literal "/dev/null") (literal "/dev/zero") (literal "/dev/random") (literal "/dev/urandom")
  (subpath "/System") (subpath "/bin") (subpath "/sbin")
  (subpath "/usr/bin") (subpath "/usr/sbin") (subpath "/usr/lib") (subpath "/usr/libexec") (subpath "/usr/share")
  (subpath "/Library/Developer/CommandLineTools")
  (subpath "/private/etc") (subpath "/private/var/db/dyld") (subpath "/private/var/db/timezone")
  (subpath (param "TEMP")) (subpath (param "VENDOR")))
(allow file-write* (subpath (param "TEMP")) (literal "/dev/null"))
(allow process-exec
  (subpath (param "TEMP")) (subpath "/bin") (subpath "/sbin") (subpath "/usr/bin") (subpath "/usr/sbin")
  (subpath "/usr/libexec") (subpath "/Library/Developer/CommandLineTools"))
(allow signal (target same-sandbox))
(deny syscall-unix (syscall-number SYS_setsid SYS_setpgid))
(deny sysctl-read (sysctl-name-prefix "kern.proc"))
(deny process-info*)
(allow process-info* (target self))
"#;

/// What the profile adds for a toolchain: reading and running it, and reading `RUSTUP_HOME`.
const TOOLCHAIN: &str = r#"(allow file-read* (subpath (param "TOOLCHAIN")) (subpath (param "RUSTUP_HOME")))
(allow process-exec (subpath (param "TOOLCHAIN")))
"#;

/// The Rust toolchain a check may use.
#[derive(Debug, Clone)]
pub struct Toolchain {
    /// The toolchain's own `bin` (such as `$RUSTUP_HOME/toolchains/<name>/bin`), not rustup's proxies
    /// in `~/.cargo/bin`, which a check can't read.
    pub bin: PathBuf,
    /// The owner's rustup directory, readable and never writable.
    pub rustup_home: PathBuf,
}

/// How checks run.
#[derive(Debug, Clone)]
pub struct Runner {
    /// The read-only vendor directory cargo's offline config points at.
    pub vendor: PathBuf,
    pub toolchain: Option<Toolchain>,
    pub timeout: Duration,
    /// How many bytes of output the outcome keeps, from the end.
    pub tail_bytes: usize,
    /// Where each run's fresh directory is made: the system's temporary directory by default.
    pub base: PathBuf,
    sandbox_exec: PathBuf,
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
    /// It ran past the timeout, and its process group was killed.
    TimedOut,
    /// These processes of the check had left its process group; they were found and killed, and
    /// the run fails whatever its exit.
    LeftGroup(Vec<i32>),
    /// Two seconds after the check's processes were killed, something still held its output open: a
    /// process that escaped before the sweep could find it. The run fails whatever its exit.
    HeldOutput,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Code(code) => write!(f, "exit {code}"),
            Exit::Signal(signal) => write!(f, "killed by signal {signal}"),
            Exit::TimedOut => write!(f, "timed out"),
            Exit::LeftGroup(pids) => {
                write!(f, "failed: {} of its processes left the process group, and were killed ({pids:?})", pids.len())
            }
            Exit::HeldOutput => write!(f, "failed: a process it started still held its output open after it ended"),
        }
    }
}

impl Exit {
    pub fn passed(&self) -> bool {
        *self == Exit::Code(0)
    }
}

/// What a check run gives back.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// `<name> blake3:<definition hash>`.
    pub method: String,
    pub argv: Vec<String>,
    /// `argv[0]` as resolved.
    pub program: PathBuf,
    pub exit: Exit,
    /// The last [`Runner::tail_bytes`] of stdout and stderr, interleaved as written.
    pub tail: Vec<u8>,
    /// All the bytes it wrote.
    pub output_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("no sandbox, so the check doesn't run: {0}")]
    NoSandbox(String),
    #[error("check {0:?} has an empty argv")]
    EmptyArgv(String),
    #[error("{0:?} isn't on the check's PATH")]
    NotFound(String),
    #[error(
        "argv[0] {0:?} is outside the tree: give a bare name, found on the check's PATH, or a relative path without '..'"
    )]
    Outside(String),
    #[error("materializing the manifest: {0}")]
    Materialize(#[from] MaterializeError),
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: std::io::Error,
    },
}

fn io(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> CheckError {
    let what = what.into();
    move |source| CheckError::Io { what, source }
}

impl Runner {
    /// A runner with no toolchain, a 30-minute timeout and a 64 KiB tail.
    pub fn new(vendor: impl Into<PathBuf>) -> Runner {
        Runner {
            vendor: vendor.into(),
            toolchain: None,
            timeout: Duration::from_secs(30 * 60),
            tail_bytes: 64 * 1024,
            base: std::env::temp_dir(),
            sandbox_exec: PathBuf::from(SANDBOX_EXEC),
        }
    }

    /// Uses another `sandbox-exec`, for tests of the refusal when it can't be applied. A stand-in that
    /// doesn't apply the profile fails the probe, so this can't turn the sandbox off.
    #[doc(hidden)]
    pub fn with_sandbox_exec(mut self, path: impl Into<PathBuf>) -> Runner {
        self.sandbox_exec = path.into();
        self
    }

    /// The check's `PATH`: the toolchain's `bin`, then [`SYSTEM_PATH`].
    pub fn path(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.toolchain.iter().map(|t| t.bin.clone()).collect();
        dirs.extend(SYSTEM_PATH.iter().map(PathBuf::from));
        dirs
    }

    /// Resolves `argv[0]`: a bare name against [`Runner::path`] only, never the tree; a name with a
    /// `/` relative to `tree`, refused if absolute or with a `..` component.
    pub fn resolve(&self, program: &str, tree: &Path) -> Result<PathBuf, CheckError> {
        if program.contains('/') {
            let path = Path::new(program);
            if !path.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
                return Err(CheckError::Outside(program.to_owned()));
            }
            return Ok(tree.join(path));
        }
        self.path()
            .into_iter()
            .map(|dir| dir.join(program))
            .find(|p| is_executable_file(p))
            .ok_or_else(|| CheckError::NotFound(program.to_owned()))
    }

    /// Runs `check` on `manifest`, whose files come from `files` by content hash, in a fresh
    /// directory under the sandbox, and removes the directory afterwards.
    pub fn run(
        &self,
        check: &CheckDefinition,
        manifest: &Manifest,
        files: impl Fn(&ContentHash) -> Option<Vec<u8>>,
    ) -> Result<Outcome, CheckError> {
        let Some(first) = check.argv.first() else { return Err(CheckError::EmptyArgv(check.name.to_owned())) };
        // Removed when dropped, however this returns or unwinds.
        let fresh = Fresh::make(&self.base)?;
        self.run_in(&fresh, check, first, manifest, files)
    }

    fn run_in(
        &self,
        fresh: &Fresh,
        check: &CheckDefinition,
        first: &str,
        manifest: &Manifest,
        files: impl Fn(&ContentHash) -> Option<Vec<u8>>,
    ) -> Result<Outcome, CheckError> {
        // The sandbox's own directory is `w/` inside the fresh one, never the fresh one itself.
        let root = fresh.0.join("w");
        let (tree, home, tmp, cargo) = (root.join("tree"), root.join("home"), root.join("tmp"), root.join("cargo"));
        for dir in [&root, &tree, &home, &tmp, &cargo] {
            std::fs::create_dir(dir).map_err(io(format!("creating {}", dir.display())))?;
        }
        let profile = self.profile(&root)?;
        // The probe: if the profile isn't applied, nothing of the check runs.
        let base = fresh.0.parent().expect("the fresh directory is in the base");
        self.probe(&profile, base)?;
        crate::materialize::materialize(manifest, files, &tree)?;
        let program = self.resolve(first, &tree)?;
        let vendor = canonical(&self.vendor)?;
        let config = format!(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = {}\n\n\
             [net]\noffline = true\n",
            toml_string(&vendor)?
        );
        std::fs::write(cargo.join("config.toml"), config).map_err(io("writing the cargo config"))?;

        let mut env: Vec<(&str, std::ffi::OsString)> = vec![
            ("PATH", std::env::join_paths(self.path()).map_err(|e| io("joining PATH")(std::io::Error::other(e)))?),
            ("LANG", LANG.into()),
            ("HOME", home.into_os_string()),
            ("TMPDIR", tmp.into_os_string()),
            ("CARGO_HOME", cargo.into_os_string()),
        ];
        if let Some(t) = &self.toolchain {
            env.push(("RUSTUP_HOME", t.rustup_home.clone().into_os_string()));
            // The linker finds its SDK through xcrun: point it at the Command Line Tools, which the profile
            // allows, rather than an Xcode whose frameworks it doesn't.
            env.push(("DEVELOPER_DIR", DEVELOPER_DIR.into()));
        }
        let mut command = profile.command(&self.sandbox_exec, &program);
        command.args(&check.argv[1..]).current_dir(&tree).env_clear().envs(env);
        let (exit, tail, output_bytes) = run::run(command, self.timeout, self.tail_bytes)?;
        Ok(Outcome { method: check.method(), argv: check.argv.to_vec(), program, exit, tail, output_bytes })
    }

    fn profile(&self, root: &Path) -> Result<Profile, CheckError> {
        if !cfg!(target_os = "macos") {
            return Err(CheckError::NoSandbox("sandbox-exec exists only on macOS".to_owned()));
        }
        if !is_executable_file(&self.sandbox_exec) {
            return Err(CheckError::NoSandbox(format!("no {}", self.sandbox_exec.display())));
        }
        let mut text = PROFILE.to_owned();
        let mut params = vec![param("TEMP", root)?, param("VENDOR", &self.vendor)?];
        if let Some(t) = &self.toolchain {
            text.push_str(TOOLCHAIN);
            params.push(param("TOOLCHAIN", &t.bin)?);
            params.push(param("RUSTUP_HOME", &t.rustup_home)?);
        }
        Ok(Profile { text, params })
    }

    /// Proves the profile is applied: a program runs under it, and is refused a listing of
    /// `outside`, the base holding the fresh directory, which this process can list.
    fn probe(&self, profile: &Profile, outside: &Path) -> Result<(), CheckError> {
        std::fs::read_dir(outside).map_err(io(format!("listing {}", outside.display())))?;
        let run = |program: &str, args: &[&Path]| {
            profile
                .command(&self.sandbox_exec, Path::new(program))
                .args(args)
                .env_clear()
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| CheckError::NoSandbox(format!("running {}: {e}", self.sandbox_exec.display())))
        };
        let out = run("/usr/bin/true", &[])?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(CheckError::NoSandbox(format!("the profile can't be applied: {}", why.trim())));
        }
        if run("/bin/ls", &[outside])?.status.success() {
            return Err(CheckError::NoSandbox(format!(
                "{} ran without the profile: it listed {}",
                self.sandbox_exec.display(),
                outside.display()
            )));
        }
        Ok(())
    }
}

/// A `sandbox-exec` profile and its parameters, every path resolved: the kernel checks resolved
/// paths, so a rule on `/tmp/x` never matches `/private/tmp/x`.
struct Profile {
    text: String,
    params: Vec<(String, String)>,
}

impl Profile {
    fn command(&self, sandbox_exec: &Path, program: &Path) -> std::process::Command {
        let mut cmd = std::process::Command::new(sandbox_exec);
        for (k, v) in &self.params {
            cmd.arg("-D").arg(format!("{k}={v}"));
        }
        cmd.arg("-p").arg(&self.text).arg(program);
        cmd
    }
}

fn canonical(path: &Path) -> Result<PathBuf, CheckError> {
    std::fs::canonicalize(path).map_err(io(format!("resolving {}", path.display())))
}

fn param(name: &str, path: &Path) -> Result<(String, String), CheckError> {
    let p = canonical(path)?;
    let s = p.to_str().ok_or_else(|| io(format!("{} isn't UTF-8", p.display()))(std::io::Error::other("not UTF-8")))?;
    if s.contains('"') {
        return Err(io(format!("{s} has a double quote"))(std::io::Error::other("unusable path")));
    }
    Ok((name.to_owned(), s.to_owned()))
}

/// A TOML basic string holding `path`.
fn toml_string(path: &Path) -> Result<String, CheckError> {
    let s = path.to_str().ok_or_else(|| io("the vendor path isn't UTF-8")(std::io::Error::other("not UTF-8")))?;
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// A fresh directory, mode 0700, removed when dropped: when the run ends, however it ends.
struct Fresh(PathBuf);

impl Fresh {
    fn make(base: &Path) -> Result<Fresh, CheckError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = canonical(base)?;
        for _ in 0..100 {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("weave-check-{}-{nanos:x}-{n}", std::process::id()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&dir) {
                Ok(()) => return Ok(Fresh(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(format!("creating a directory in {}", base.display()))(e)),
            }
        }
        Err(io("creating a fresh directory")(std::io::Error::other("every name was taken")))
    }
}

impl Drop for Fresh {
    /// Best effort: a check may have left something it can't be removed through. The directory is
    /// first renamed within the base, in one step: the profile allows writes only under the old
    /// path, so a process the sweep couldn't find can neither write into the renamed tree nor
    /// create the old path again (the base isn't writable), and the removal can't be raced.
    fn drop(&mut self) {
        let name = self.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let gone = self.0.with_file_name(format!("{name}.removing"));
        let dir = if std::fs::rename(&self.0, &gone).is_ok() { gone } else { self.0.clone() };
        #[cfg(unix)]
        make_writable(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Gives the owner write and search permission on every directory under `dir`, so a check that
/// removed them can't keep its files from being cleaned up. It never follows a symlink: each
/// directory is changed through its parent's descriptor with `AT_SYMLINK_NOFOLLOW`, then opened
/// with `O_NOFOLLOW`, so a directory swapped for a symlink is left alone (finding 04592044646f).
#[cfg(unix)]
fn make_writable(dir: &Path) {
    use rustix::fs::{self as fs, Mode, OFlags};
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    if !meta.is_dir() {
        return;
    }
    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o7777;
    let _ = fs::chmod(dir, Mode::from_raw_mode(mode as _) | Mode::RWXU);
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if let Ok(fd) = fs::open(dir, flags, Mode::empty()) {
        writable_under(&fd, 0);
    }
}

#[cfg(unix)]
fn writable_under(parent: &std::os::fd::OwnedFd, depth: usize) {
    use rustix::fs::{self as fs, AtFlags, FileType, Mode, OFlags};
    // A check can nest directories deeply; past this, clean-up leaves the rest.
    if depth > 256 {
        return;
    }
    let Ok(dir) = fs::Dir::read_from(parent) else { return };
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    for entry in dir.flatten() {
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let Ok(st) = fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) else { continue };
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
            continue;
        }
        let mode = Mode::from_raw_mode(st.st_mode & 0o7777) | Mode::RWXU;
        let _ = fs::chmodat(parent, name, mode, AtFlags::SYMLINK_NOFOLLOW);
        if let Ok(fd) = fs::openat(parent, name, flags, Mode::empty()) {
            writable_under(&fd, depth + 1);
        }
    }
}

/// Running the sandboxed command: one pipe for stdout and stderr, a timeout, and the process group
/// killed at the end.
#[cfg(unix)]
mod run {
    use std::io::Read;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};

    use super::{CheckError, Exit, io};

    /// How long the output may stay open after the check's processes are killed.
    const OUTPUT_GRACE: Duration = Duration::from_secs(2);

    /// The last `cap` bytes written, and how many there were.
    #[derive(Default)]
    struct Tail {
        bytes: std::collections::VecDeque<u8>,
        total: u64,
    }

    /// A started check, leading its own process group. Whatever path leaves [`run`], an error
    /// included, dropping this kills the group before the check is reaped, so the group's id can't
    /// have been given to another process yet.
    struct Group {
        child: Child,
        reaped: bool,
    }

    impl Group {
        fn pid(&self) -> Pid {
            Pid::from_child(&self.child)
        }

        /// Whether the check has exited, without reaping it.
        fn exited(&self) -> Result<bool, CheckError> {
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            rustix::process::waitid(WaitId::Pid(self.pid()), options)
                .map(|status| status.is_some())
                .map_err(|e| io("waiting for the check")(e.into()))
        }

        /// Kills the group and every descendant found by parent pid, then reaps the check. Returns
        /// its status and the processes that had left the group.
        fn finish(&mut self) -> Result<(ExitStatus, Vec<i32>), CheckError> {
            let left = sweep(self.pid());
            let status = self.child.wait().map_err(io("reaping the check"))?;
            self.reaped = true;
            Ok((status, left?))
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            if !self.reaped {
                let _ = sweep(self.pid());
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    /// One row of the process table: pid, parent pid, process group.
    fn processes() -> Result<Vec<(i32, i32, i32)>, CheckError> {
        let out = std::process::Command::new("/bin/ps")
            .args(["-A", "-o", "pid=,ppid=,pgid="])
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .map_err(io("listing processes with ps"))?;
        if !out.status.success() {
            return Err(io("listing processes with ps")(std::io::Error::other(format!("exit {}", out.status))));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace().map(|f| f.parse::<i32>().ok());
                Some((fields.next()??, fields.next()??, fields.next()??))
            })
            .collect())
    }

    /// Stops the check's group, then finds its descendants by parent pid, starting from the leader
    /// and every process still in the group, stopping each as it is found so none can start another
    /// unseen, again until a pass finds none. Then kills them all. Returns those that had left the
    /// group. The leader's own pid is the group's id, and stays reserved until it is reaped.
    fn sweep(leader: Pid) -> Result<Vec<i32>, CheckError> {
        let group = leader.as_raw_nonzero().get();
        let _ = rustix::process::kill_process_group(leader, Signal::STOP);
        let mut found: std::collections::BTreeSet<i32> = std::collections::BTreeSet::from([group]);
        let mut left = Vec::new();
        let mut result = Ok(());
        for _ in 0..64 {
            let table = match processes() {
                Ok(table) => table,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            let before = found.len();
            found.extend(table.iter().filter(|(_, _, pgid)| *pgid == group).map(|(pid, _, _)| *pid));
            // Children of anything found, until the set stops growing within this table.
            loop {
                let n = found.len();
                for (pid, ppid, pgid) in &table {
                    if found.contains(ppid) && found.insert(*pid) && *pgid != group {
                        left.push(*pid);
                        if let Some(p) = Pid::from_raw(*pid) {
                            let _ = rustix::process::kill_process(p, Signal::STOP);
                        }
                    }
                }
                if found.len() == n {
                    break;
                }
            }
            if found.len() == before {
                break;
            }
        }
        let _ = rustix::process::kill_process_group(leader, Signal::KILL);
        for pid in &found {
            if let Some(p) = Pid::from_raw(*pid) {
                let _ = rustix::process::kill_process(p, Signal::KILL);
            }
        }
        result.map(|()| left)
    }

    pub(super) fn run(mut command: Command, timeout: Duration, cap: usize) -> Result<(Exit, Vec<u8>, u64), CheckError> {
        let (reader, writer) = std::io::pipe().map_err(io("making the output pipe"))?;
        let writer2 = writer.try_clone().map_err(io("making the output pipe"))?;
        command.stdin(Stdio::null()).stdout(writer).stderr(writer2);
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut group = Group { child: command.spawn().map_err(io("starting the check"))?, reaped: false };
        // Our copies of the write end go with the command, so the reader sees end-of-file once the
        // check's processes are gone.
        drop(command);

        // The reader never blocks, and stops when told: a process the sweep can't find (an orphan,
        // weave.md section 3) may hold the pipe open, and the reader must still end.
        let flags = rustix::fs::fcntl_getfl(&reader).map_err(|e| io("reading the output pipe's flags")(e.into()))?;
        rustix::fs::fcntl_setfl(&reader, flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|e| io("making the output pipe non-blocking")(e.into()))?;
        let tail = Arc::new(Mutex::new(Tail::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reading = {
            let (tail, stop) = (Arc::clone(&tail), Arc::clone(&stop));
            std::thread::spawn(move || {
                read_into(reader, &tail, &stop, cap);
                let _ = done_tx.send(());
            })
        };
        // On an early return too, the reader stops at its next poll.
        struct StopOnDrop(Arc<AtomicBool>);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let _stop_reader = StopOnDrop(Arc::clone(&stop));

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        while !group.exited()? {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Whether it ended or timed out, nothing it started that the sweep can find outlives it (an orphan
        // can, weave.md section 3).
        let (status, left) = group.finish()?;
        // A process the sweep couldn't find may still hold the pipe: wait for it only briefly, then stop
        // the reader, which ends before its next read, and join it. A held pipe fails the run.
        let held = done_rx.recv_timeout(OUTPUT_GRACE).is_err();
        stop.store(true, Ordering::Relaxed);
        let _ = reading.join();
        let exit = if !left.is_empty() {
            Exit::LeftGroup(left)
        } else if held {
            Exit::HeldOutput
        } else if timed_out {
            Exit::TimedOut
        } else {
            exit_of(status)
        };
        let t = tail.lock().expect("the tail's lock isn't poisoned: nothing panics holding it");
        Ok((exit, t.bytes.iter().copied().collect(), t.total))
    }

    /// Reads `reader` (non-blocking) into `tail`, keeping its last `cap` bytes, until end-of-file, an
    /// error, or `stop`. The flag is checked before every read, so a writer that keeps the pipe full
    /// can't keep the reader going (task 6b8a1b5a6108).
    fn read_into(mut reader: impl Read, tail: &Mutex<Tail>, stop: &AtomicBool, cap: usize) {
        let mut buf = [0u8; 8192];
        while !stop.load(Ordering::Relaxed) {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let mut t = tail.lock().expect("the tail's lock isn't poisoned: nothing panics holding it");
            t.total += n as u64;
            t.bytes.extend(&buf[..n]);
            let excess = t.bytes.len().saturating_sub(cap);
            t.bytes.drain(..excess);
        }
    }

    fn exit_of(status: ExitStatus) -> Exit {
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return Exit::Signal(signal);
        }
        Exit::Code(status.code().unwrap_or(-1))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A pipe that never runs dry.
        struct Full;

        impl Read for Full {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                buf.fill(b'y');
                Ok(buf.len())
            }
        }

        // Task 6b8a1b5a6108: told to stop, the reader stops, though every read finds output.
        #[test]
        fn the_reader_stops_when_told_even_while_the_pipe_stays_full() {
            let (tail, stop) = (Arc::new(Mutex::new(Tail::default())), Arc::new(AtomicBool::new(false)));
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            {
                let (tail, stop) = (Arc::clone(&tail), Arc::clone(&stop));
                std::thread::spawn(move || {
                    read_into(Full, &tail, &stop, 16);
                    let _ = done_tx.send(());
                });
            }
            std::thread::sleep(Duration::from_millis(50));
            stop.store(true, Ordering::Relaxed);
            assert!(done_rx.recv_timeout(Duration::from_secs(5)).is_ok(), "the reader kept reading after the stop");
            let t = tail.lock().expect("not poisoned");
            assert!(t.total > 0 && t.bytes.len() == 16, "{} read, {} kept", t.total, t.bytes.len());
        }
    }
}

/// Elsewhere there is no sandbox, so [`Runner::run`] refuses before it gets here.
#[cfg(not(unix))]
mod run {
    use std::process::Command;
    use std::time::Duration;

    use super::{CheckError, Exit};

    pub(super) fn run(_command: Command, _timeout: Duration, _cap: usize) -> Result<(Exit, Vec<u8>, u64), CheckError> {
        Err(CheckError::NoSandbox("checks run only on macOS".to_owned()))
    }
}

/// An output tail as a result records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedTail {
    /// The tail with control characters other than newline and tab removed.
    Text(String),
    /// A content refusal matched, so only the tail's size and hash are recorded.
    Withheld { size: u64, hash: ContentHash, rule: SecretRule },
}

/// Prepares an output tail for the record: the tail is permanent and could carry what a check read,
/// so control characters are stripped and import's content refusals run on it (weave.md section 3,
/// *Its result*).
pub fn recorded_tail(tail: &[u8], known: &KnownKeys) -> RecordedTail {
    if let Some(rule) = secrets::scan(tail, known) {
        return RecordedTail::Withheld { size: tail.len() as u64, hash: ContentHash::of(tail), rule };
    }
    let text: String =
        String::from_utf8_lossy(tail).chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
    // Stripping can join the parts of a key the scan saw apart: scan what would be recorded too.
    if let Some(rule) = secrets::scan(text.as_bytes(), known) {
        return RecordedTail::Withheld { size: tail.len() as u64, hash: ContentHash::of(tail), rule };
    }
    RecordedTail::Text(text)
}
