//! The Python execution profile (`docs/design/eval-suite.md`, Isolation; decision ac22cedd8fee): agent code
//! runs only here, in a container of the pinned Python image on Docker, under the kernel's limits, in a scratch
//! directory made fresh for each run and gone after it.
//!
//! - **The image** is pinned by digest ([`Python::pin`]): a run's plan records it, and a run refuses an image
//!   that isn't here by that digest, or whose Python isn't the project's version.
//! - **In and out**: a run's inputs, such as the tree and the runner, are written to a directory of the host's
//!   ([`Scratch`]), mounted read-only at `/in` and copied into the scratch directory, `/scratch`, before the code
//!   starts. Nothing else of the host's is mounted. Results come out as the code's output, its exit, and the
//!   files of a directory a run names ([`Outcome::files`]).
//! - **The container**: no network, a read-only root, no new privileges, every capability dropped but the three
//!   the supervisor needs to start the code as another user, which are gone before any code runs. The code
//!   runs as `nobody`, as `python -I -S -B -X utf8` with an environment of its own.
//! - **Limits**: CPU time, file size and open files as rlimits; memory as the container's, the run's memory
//!   limit plus its scratch size, since the scratch directory is held in memory, plus [`SUPERVISOR_MB`];
//!   processes as the container's; the scratch directory as its size and [`MAX_SCRATCH_FILES`]; output read up
//!   to its limit, then the container killed; wall time from when the code starts, then killed. Killing a
//!   container ends everything in it.
//! - **The supervisor** (`python/supervisor.py`), the container's first process under its init, starts the code
//!   and relays its output in frames the code can't forge. It runs as a user of its own, which the code can't
//!   signal or read. It says only whether the scratch directory was full, which is how a run that ended of
//!   itself counts as [`Exit::Scratch`], and the CPU time used so far ([`Running::cpu`]). Its word never
//!   widens a limit or marks a pass.
//!
//! [`run_tests`] runs a tree's public tests here: feedback for the agent, never a grade.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::project::{Limits, Project};

/// The image a project run pins by default: Python 3.12 (decision c008b959c572), by digest.
pub const IMAGE: &str = "python:3.12@sha256:17b9724493d916f56565c131f389faff2597f46dec6ce09516f95125bc9f519f";
/// How often a running container's output and wall time are checked.
pub const POLL: Duration = Duration::from_millis(20);
/// The most open files a process may have.
const MAX_OPEN_FILES: u64 = 256;
/// The most processes a container may hold: its init, the supervisor and the code's.
const MAX_PROCESSES: u32 = 64;
/// The most files and directories a run's scratch directory may hold.
pub const MAX_SCRATCH_FILES: u64 = 10_000;
/// The memory the container's init and the supervisor may use, on top of the run's.
pub const SUPERVISOR_MB: u32 = 16;
/// How long a container may take to start the code, which the wall time doesn't count.
const START_GRACE: Duration = Duration::from_secs(60);
/// Where the scratch directory is, in the container.
pub const SCRATCH: &str = "/scratch";
/// The label each container is given: the pid of the process that started it, for [`sweep`].
const LABEL: &str = "bracken.python-runner";
/// The most bytes of one frame from the supervisor: a collected file is the longest.
const MAX_FRAME: usize = 64 << 20;
/// The runner [`run_tests`] starts, as it is in this binary.
const RUN_TESTS: &str = include_str!("python/run_tests.py");
const SUPERVISOR: &str = include_str!("python/supervisor.py");
/// What the image is asked when it is pinned: its Python's version.
const PROBE: &str = "import sys; print('%d.%d.%d' % sys.version_info[:3])";

/// The image as a run's plan records it, and pins it from again (decisions c008b959c572, ac22cedd8fee): a run
/// refuses an image that isn't here by the plan's digest, or whose version isn't the plan's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonPin {
    /// Pinned by digest, as `name@sha256:<hex>`.
    pub image: String,
    pub version: String,
}

/// The pinned image.
#[derive(Debug, Clone, Serialize)]
pub struct Python {
    /// Pinned by digest, as `name@sha256:<hex>`: every container runs it by this name, so by this digest.
    pub image: String,
    /// The digest, `sha256:<hex>`.
    pub digest: String,
    /// Its Python's, such as `3.12.13`.
    pub version: String,
}

impl Python {
    /// Pins `image` for a project written for Python `minor` (such as `3.12`): refuses an image not named by
    /// its digest, one that isn't here by that digest (it isn't pulled: `docker pull` it first), and one whose
    /// Python is another minor version.
    pub fn pin(image: &str, minor: &str) -> Result<Python, String> {
        let digest = image
            .rsplit_once('@')
            .map(|(_, d)| d)
            .filter(|d| {
                d.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            .ok_or_else(|| format!("python image {image}: pin it by digest, as name@sha256:<64 hex digits>"))?;
        let out = docker(&["image", "inspect", "--format", "{{json .RepoDigests}}", image])?;
        if !out.status.success() {
            return Err(format!(
                "python image {image} isn't here (docker pull it first): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let digests: Vec<String> =
            serde_json::from_slice(&out.stdout).map_err(|e| format!("docker image inspect: {e}"))?;
        if !digests.iter().any(|d| d.rsplit_once('@').is_some_and(|(_, d)| d == digest)) {
            return Err(format!("python image {image}: the image here has digests {digests:?}, not {digest}"));
        }
        let probe = docker(&[
            "run",
            "--rm",
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--user",
            "65534:65534",
            image,
            "python",
            "-I",
            "-S",
            "-c",
            PROBE,
        ])?;
        let version = String::from_utf8_lossy(&probe.stdout).trim().to_string();
        if !probe.status.success() || version.is_empty() {
            return Err(format!(
                "python image {image} didn't say its version: {}",
                String::from_utf8_lossy(&probe.stderr)
            ));
        }
        if version.rsplit_once('.').map(|(m, _)| m) != Some(minor) {
            return Err(format!("python image {image} has Python {version}; the project is written for {minor}"));
        }
        Ok(Python { image: image.to_string(), digest: digest.to_string(), version })
    }

    /// What a plan records of it.
    pub fn plan_pin(&self) -> PythonPin {
        PythonPin { image: self.image.clone(), version: self.version.clone() }
    }

    /// The image `pin` names, pinned again: refused unless it is here by the pin's digest, with its version.
    pub fn from_plan(pin: &PythonPin, minor: &str) -> Result<Python, String> {
        let python = Python::pin(&pin.image, minor)?;
        if python.version != pin.version {
            return Err(format!(
                "python image {} has Python {}, not the plan's {}",
                pin.image, python.version, pin.version
            ));
        }
        Ok(python)
    }

    /// Runs `args` after the interpreter's own flags in `scratch`, under the profile and `limits`.
    pub fn run(&self, scratch: &Scratch, args: &[OsString], limits: &Limits) -> Result<Outcome, String> {
        self.run_with_input(scratch, args, limits, None, None)
    }

    /// [`Python::run`], with `input` written to its standard input, and with `collect`, the regular files under
    /// that directory of the scratch directory given back once the code has ended ([`Outcome::files`]).
    pub fn run_with_input(
        &self,
        scratch: &Scratch,
        args: &[OsString],
        limits: &Limits,
        input: Option<Vec<u8>>,
        collect: Option<&str>,
    ) -> Result<Outcome, String> {
        let cap = usize::try_from(limits.output_bytes).unwrap_or(usize::MAX);
        let mut container = Container::start(self, scratch, args, limits, collect, None)?;
        if let (Some(input), Some(mut stdin)) = (input, container.client.stdin.take()) {
            std::thread::spawn(move || {
                let _ = std::io::Write::write_all(&mut stdin, &input);
            });
        }
        drop(container.client.stdin.take());
        let killed = loop {
            if container.client.try_wait().map_err(|e| format!("waiting for docker: {e}"))?.is_some() {
                break None;
            }
            if let Some(why) = container.over(limits) {
                container.kill();
                break Some(why);
            }
            std::thread::sleep(POLL);
        };
        let ended = Instant::now();
        let (exit, shared) = container.end(killed)?;
        let take = |m: &Mutex<Vec<u8>>| std::mem::take(&mut *m.lock().unwrap_or_else(|e| e.into_inner()));
        let (mut stdout, mut stderr) = (take(&shared.stdout), take(&shared.stderr));
        stdout.truncate(cap);
        stderr.truncate(cap);
        let files = std::mem::take(&mut *shared.files.lock().unwrap_or_else(|e| e.into_inner()));
        let started = *shared.started.lock().unwrap_or_else(|e| e.into_inner());
        let ran = started.map_or(Duration::ZERO, |at| ended.saturating_duration_since(at));
        Ok(Outcome { exit, stdout, stderr, files, ran })
    }

    /// Starts `args` in `scratch` under the profile and `limits`, to talk to a line at a time ([`Running`]).
    pub fn start(&self, scratch: &Scratch, args: &[OsString], limits: &Limits) -> Result<Running, String> {
        let (send, lines) = std::sync::mpsc::channel();
        let mut container = Container::start(self, scratch, args, limits, None, Some(send))?;
        let stdin = container.client.stdin.take();
        let (stop, why) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        let (name, shared, limits, spawned) =
            (container.name.clone(), container.shared.clone(), *limits, container.spawned);
        let (stopping, reason) = (stop.clone(), why.clone());
        let watch = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if let Some(over) = over(&shared, &limits, spawned) {
                    kill(&name);
                    *reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(over);
                    return;
                }
                std::thread::sleep(POLL);
            }
        });
        Ok(Running { container: Some(container), stdin, lines, stop, why, watch: Some(watch) })
    }
}

/// Runs `docker` with `args`, and waits for it.
fn docker(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("docker").args(args).stdin(Stdio::null()).output().map_err(|e| format!("running docker: {e}"))
}

/// Whether Docker is running here: tests that need it are skipped, and say so, where it isn't.
pub fn docker_running() -> bool {
    static RUNNING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RUNNING.get_or_init(|| docker(&["info", "--format", "{{.ServerVersion}}"]).is_ok_and(|o| o.status.success()))
}

/// The image tests pin: [`IMAGE`], or the one `BRACKEN_TEST_PYTHON_IMAGE` names. None, having said why, where
/// Docker isn't running, or, elsewhere than macOS, where the image isn't pulled, as in the test boxes. On macOS,
/// where the counted run runs these tests (docs/development.md), an image not pulled fails them.
#[doc(hidden)]
pub fn for_tests() -> Option<Python> {
    if !docker_running() {
        eprintln!("skipped: Docker isn't running here, and this test runs agent code in a container");
        return None;
    }
    let image = std::env::var("BRACKEN_TEST_PYTHON_IMAGE").unwrap_or_else(|_| IMAGE.into());
    match Python::pin(&image, "3.12") {
        Ok(python) => Some(python),
        Err(e) if !cfg!(target_os = "macos") && e.contains("isn't here") => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e} (docker pull {image})"),
    }
}

/// Ends the container `name`, and everything in it. It may have ended already.
fn kill(name: &str) {
    let _ = docker(&["kill", name]);
}

/// Removes the containers of runners that have ended, which a runner killed before it removed its own leaves
/// behind: once a process, before its first container.
fn sweep() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(sweep_now);
}

/// [`sweep`], whether or not this process has swept before.
fn sweep_now() {
    let format = format!("{{{{.Names}}}} {{{{.Label \"{LABEL}\"}}}}");
    let Ok(out) = docker(&["ps", "-a", "--filter", &format!("label={LABEL}"), "--format", &format]) else { return };
    let dead: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, pid)| {
            let pid = pid.parse::<i32>().ok().and_then(rustix::process::Pid::from_raw);
            pid.is_none_or(|pid| rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH))
        })
        .map(|(name, _)| name.to_string())
        .collect();
    if !dead.is_empty() {
        let mut args = vec!["rm", "-f"];
        args.extend(dead.iter().map(String::as_str));
        let _ = docker(&args);
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    Code(i32),
    /// Killed by a signal: `SIGXCPU` (24) is the CPU limit, `SIGXFSZ` (25) the file size limit.
    Signal(i32),
    WallTime,
    /// Killed for passing the container's memory limit.
    Memory,
    Output,
    /// Ended, of itself or by an error or a signal, with its scratch directory full, of bytes or of files, at a
    /// check: the scratch directory can't hold more, so the code's next write failed.
    Scratch,
}

/// A run's end and what it wrote, each stream cut at the output limit.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub exit: Exit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The regular files of the directory the run named to collect, by path relative to it: those the
    /// supervisor could read, and 32 MiB at most. None otherwise.
    pub files: Vec<(String, Vec<u8>)>,
    /// How long the code ran: from when it started to when its container ended, without the container's start
    /// and removal.
    pub ran: Duration,
}

/// A run's inputs: a directory made fresh for it, `0700`, in the system's temporary directory, and removed when
/// dropped. The container sees it at `/in`, read-only, and copies it into the scratch directory before the code
/// starts: what's at `dir/x` here is at `/scratch/x` there ([`Scratch::inside`]).
pub struct Scratch {
    /// Resolved.
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

    /// Copies the tree at `from` into the directory as `name`: regular files and directories only.
    pub fn copy_tree(&self, from: &Path, name: &str) -> Result<PathBuf, String> {
        let to = self.dir.join(name);
        copy_tree(from, &to)?;
        Ok(to)
    }

    /// Where `path`, in [`Scratch::dir`], is in the container once copied: in the scratch directory.
    pub fn inside(&self, path: &Path) -> OsString {
        let rel = path.strip_prefix(&self.dir).expect("a path in the scratch directory");
        Path::new(SCRATCH).join(rel).into_os_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
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

/// What the supervisor says last: the code's exit status (negative for a signal; none if the inputs weren't
/// copied), and whether the scratch directory was full at any check.
#[derive(serde::Deserialize)]
struct End {
    status: Option<i32>,
    scratch: bool,
}

/// What a container's frames have said so far, as the reader on its output thread keeps it.
#[derive(Default)]
struct Shared {
    /// A stream passed the output limit, or a line was longer than [`MAX_LINE`].
    full: AtomicBool,
    /// When the first frame came: the code had started.
    started: Mutex<Option<Instant>>,
    /// The frames have ended: the container has.
    done: AtomicBool,
    /// Milliseconds more of wall time the caller has given it ([`Running::resume`]).
    extra: AtomicU64,
    /// Since when its wall time has stood still, if it has ([`Running::pause`]).
    paused: Mutex<Option<Instant>>,
    /// The container's CPU time, in microseconds.
    cpu: AtomicU64,
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
    files: Mutex<Vec<(String, Vec<u8>)>>,
    end: Mutex<Option<End>>,
}

/// The limit a container has passed, if it has passed one: its output, or its wall time from when the code
/// started (or, until it has, from when the container was asked for, with [`START_GRACE`] more).
fn over(shared: &Shared, limits: &Limits, spawned: Instant) -> Option<Exit> {
    let started = *shared.started.lock().unwrap_or_else(|e| e.into_inner());
    let extra = Duration::from_millis(shared.extra.load(Ordering::SeqCst));
    let deadline = started.unwrap_or(spawned + START_GRACE) + Duration::from_secs(limits.wall_seconds.into()) + extra;
    let now = shared.paused.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(Instant::now);
    if shared.full.load(Ordering::SeqCst) {
        Some(Exit::Output)
    } else if now > deadline {
        Some(Exit::WallTime)
    } else {
        None
    }
}

/// One run's container: the `docker run` client attached to it, and the threads reading its output.
struct Container {
    name: String,
    client: Child,
    spawned: Instant,
    shared: Arc<Shared>,
    reader: Option<std::thread::JoinHandle<()>>,
    errors: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl Container {
    /// Starts `args` in a container of `python`'s image, with `scratch` as its inputs, under `limits`. With
    /// `lines`, the code's standard output goes there a line at a time, and only its standard error counts
    /// toward the output limit.
    fn start(
        python: &Python,
        scratch: &Scratch,
        args: &[OsString],
        limits: &Limits,
        collect: Option<&str>,
        lines: Option<std::sync::mpsc::Sender<String>>,
    ) -> Result<Container, String> {
        sweep();
        let argv: Vec<&str> = args
            .iter()
            .map(|a| a.to_str().ok_or_else(|| format!("an argument that isn't UTF-8: {a:?}")))
            .collect::<Result<_, _>>()?;
        let spec = serde_json::json!({"argv": argv, "cwd": SCRATCH, "collect": collect}).to_string();
        let source =
            scratch.dir.to_str().filter(|s| !s.contains(',')).ok_or("a scratch directory docker can't mount")?;
        let seed = format!("{:?}{}{}", Instant::now(), std::process::id(), source);
        let name = format!("bracken-python-{}", &blake3::hash(seed.as_bytes()).to_hex()[..16]);
        let memory = u64::from(limits.memory_mb) + u64::from(limits.scratch_mb) + u64::from(SUPERVISOR_MB);
        let cpu = u64::from(limits.cpu_seconds);
        // What the code may write to one file: as much as it may print, and at least 16 MiB.
        let fsize = limits.output_bytes.max(16 << 20);
        let mut cmd = Command::new("docker");
        cmd.args(["run", "-i", "--name", &name, "--label", &format!("{LABEL}={}", std::process::id())])
            .args(["--log-driver", "none", "--init", "--network", "none", "--read-only"])
            .arg("--tmpfs")
            .arg(format!("{SCRATCH}:rw,size={}m,nr_inodes={MAX_SCRATCH_FILES},mode=1777", limits.scratch_mb))
            .args(["--memory", &format!("{memory}m"), "--memory-swap", &format!("{memory}m")])
            .args(["--cpus", "1", "--pids-limit", &MAX_PROCESSES.to_string()])
            .args(["--cap-drop", "ALL", "--cap-add", "SETUID", "--cap-add", "SETGID", "--cap-add", "SETPCAP"])
            .args(["--security-opt", "no-new-privileges", "--user", "0:0"])
            .args(["--ulimit", &format!("cpu={cpu}:{}", cpu + 1), "--ulimit", &format!("fsize={fsize}:{fsize}")])
            .args(["--ulimit", &format!("nofile={MAX_OPEN_FILES}:{MAX_OPEN_FILES}"), "--ulimit", "core=0:0"])
            .args(["-w", SCRATCH, "--mount", &format!("type=bind,source={source},target=/in,readonly")])
            .args([python.image.as_str(), "python", "-I", "-S", "-B", "-c", SUPERVISOR, &spec])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let spawned = Instant::now();
        let mut client = cmd.spawn().map_err(|e| format!("running docker: {e}"))?;
        let shared = Arc::new(Shared::default());
        let cap = usize::try_from(limits.output_bytes).unwrap_or(usize::MAX);
        let (out, keep) = (client.stdout.take().expect("piped"), shared.clone());
        let reader = std::thread::spawn(move || read_frames(out, &keep, cap, lines));
        let mut err = client.stderr.take().expect("piped");
        let errors = std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = (&mut err).take(64 << 10).read_to_end(&mut text);
            let _ = std::io::copy(&mut err, &mut std::io::sink());
            text
        });
        Ok(Container { name, client, spawned, shared, reader: Some(reader), errors: Some(errors) })
    }

    fn over(&self, limits: &Limits) -> Option<Exit> {
        over(&self.shared, limits, self.spawned)
    }

    /// Ends the container, and everything in it: once it is there, if it isn't yet (a kill asked for before
    /// docker has made it would miss it), unless the client ends first, or after [`START_GRACE`].
    fn kill(&mut self) {
        let deadline = Instant::now() + START_GRACE;
        while !docker(&["kill", &self.name]).is_ok_and(|o| o.status.success())
            && self.client.try_wait().is_ok_and(|s| s.is_none())
            && Instant::now() < deadline
        {
            std::thread::sleep(5 * POLL);
        }
    }

    /// Waits for the container to end, removes it, and says how the run ended: `killed` if the runner killed it
    /// for a limit; otherwise as the kernel or the supervisor says.
    fn end(mut self, killed: Option<Exit>) -> Result<(Exit, Arc<Shared>), String> {
        let status = self.client.wait().map_err(|e| format!("waiting for docker: {e}"))?;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let errors = self.errors.take().and_then(|e| e.join().ok()).unwrap_or_default();
        let state = docker(&["inspect", "--format", "{{.State.OOMKilled}}", &self.name]);
        let _ = docker(&["rm", "-f", &self.name]);
        let oom = state.is_ok_and(|s| String::from_utf8_lossy(&s.stdout).trim() == "true");
        let shared = self.shared.clone();
        let end = shared.end.lock().unwrap_or_else(|e| e.into_inner()).take();
        let started = shared.started.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let errors = String::from_utf8_lossy(&errors);
        let exit = if shared.full.load(Ordering::SeqCst) {
            Exit::Output
        } else if let Some(why) = killed {
            why
        } else if oom {
            Exit::Memory
        } else {
            match end {
                Some(End { scratch: true, .. }) => Exit::Scratch,
                Some(End { status: Some(s), .. }) if s < 0 => Exit::Signal(-s),
                Some(End { status: Some(s), .. }) => Exit::Code(s),
                Some(End { status: None, .. }) => return Err(format!("the run's inputs weren't copied: {errors}")),
                // Docker's own failures to run it.
                None if !started && matches!(status.code(), Some(125..=127)) => {
                    return Err(format!("docker run: {}", errors.trim()));
                }
                // Ended before the supervisor said how: as docker says, a signal as 128 more.
                None => match status.code() {
                    Some(code) if code > 128 => Exit::Signal(code - 128),
                    Some(code) => Exit::Code(code),
                    None => Exit::Code(-1),
                },
            }
        };
        Ok((exit, shared))
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.reader.is_some() {
            self.kill();
            let _ = self.client.wait();
            let _ = docker(&["rm", "-f", &self.name]);
        }
    }
}

/// Reads the supervisor's frames from `from` into `shared` until it ends: the code's output up to `cap` bytes a
/// stream (with `lines`, its standard output a line at a time instead), past which `full` is set and the rest is
/// read and dropped.
fn read_frames(from: impl Read, shared: &Shared, cap: usize, lines: Option<std::sync::mpsc::Sender<String>>) {
    let mut from = std::io::BufReader::new(from);
    let mut line = Vec::new();
    let mut header = [0u8; 5];
    while from.read_exact(&mut header).is_ok() {
        let len = u32::from_be_bytes(header[1..].try_into().expect("four bytes")) as usize;
        if len > MAX_FRAME {
            break;
        }
        let mut payload = vec![0; len];
        if from.read_exact(&mut payload).is_err() {
            break;
        }
        shared.started.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Instant::now);
        match (header[0], &lines) {
            (b'o', Some(lines)) => {
                for &b in &payload {
                    if b == b'\n' {
                        let _ = lines.send(String::from_utf8_lossy(&line).into_owned());
                        line.clear();
                    } else if line.len() >= MAX_LINE {
                        shared.full.store(true, Ordering::SeqCst);
                    } else {
                        line.push(b);
                    }
                }
            }
            (b'o', None) => keep(&shared.stdout, &payload, cap, &shared.full),
            (b'e', _) => keep(&shared.stderr, &payload, cap, &shared.full),
            (b'c', _) => {
                let usec = std::str::from_utf8(&payload).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
                shared.cpu.store(usec, Ordering::SeqCst);
            }
            (b'f', _) => {
                if let Some(at) = payload.iter().position(|&b| b == 0) {
                    let path = String::from_utf8_lossy(&payload[..at]).into_owned();
                    let bytes = payload[at + 1..].to_vec();
                    shared.files.lock().unwrap_or_else(|e| e.into_inner()).push((path, bytes));
                }
            }
            (b'x', _) => *shared.end.lock().unwrap_or_else(|e| e.into_inner()) = serde_json::from_slice(&payload).ok(),
            _ => break,
        }
    }
    // Drained, so the client can end.
    let _ = std::io::copy(&mut from, &mut std::io::sink());
    shared.done.store(true, Ordering::SeqCst);
}

/// Adds `bytes` to `to`, up to `cap`; past it, sets `full`.
fn keep(to: &Mutex<Vec<u8>>, bytes: &[u8], cap: usize, full: &AtomicBool) {
    let mut to = to.lock().unwrap_or_else(|e| e.into_inner());
    if to.len() + bytes.len() > cap {
        let room = cap.saturating_sub(to.len());
        to.extend_from_slice(&bytes[..room]);
        full.store(true, Ordering::SeqCst);
    } else {
        to.extend_from_slice(bytes);
    }
}

/// A child started with [`Python::start`]: lines go to its standard input, and come from its standard output, at
/// most [`MAX_LINE`] bytes each. A watchdog ends it past a limit, when its standard error passes the output
/// limit, or when a line is longer than [`MAX_LINE`]: the container is killed, and [`Running::finish`] says why.
pub struct Running {
    container: Option<Container>,
    stdin: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    why: Arc<Mutex<Option<Exit>>>,
    watch: Option<std::thread::JoinHandle<()>>,
}

/// The longest line a [`Running`] child may send.
pub const MAX_LINE: usize = 64 << 20;

impl Running {
    /// Sends one line.
    pub fn send(&mut self, line: &str) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("the child's input is closed")?;
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        std::io::Write::write_all(stdin, &bytes).map_err(|e| format!("writing to the child: {e}"))
    }

    /// The next line, waiting until `deadline` at most; `None` if the child ended, was killed, or sent nothing in
    /// time.
    pub fn next(&self, deadline: Instant) -> Option<String> {
        match self.next_line(deadline) {
            Line::Got(line) => Some(line),
            Line::Late | Line::Closed => None,
        }
    }

    /// The next line, waiting until `deadline` at most, or why there is none.
    pub fn next_line(&self, deadline: Instant) -> Line {
        use std::sync::mpsc::RecvTimeoutError;
        match self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => Line::Got(line),
            Err(RecvTimeoutError::Timeout) => Line::Late,
            Err(RecvTimeoutError::Disconnected) => Line::Closed,
        }
    }

    /// When the code started, waiting for it to, up to [`START_GRACE`]; now if it ended or didn't start in that
    /// time. A container takes a while to start, which a caller's own deadlines shouldn't count, as its wall
    /// time doesn't.
    pub fn started(&self) -> Instant {
        let Some(container) = self.container.as_ref() else { return Instant::now() };
        let shared = &container.shared;
        loop {
            if let Some(at) = *shared.started.lock().unwrap_or_else(|e| e.into_inner()) {
                return at;
            }
            if shared.done.load(Ordering::SeqCst) || container.spawned.elapsed() > START_GRACE {
                return Instant::now();
            }
            std::thread::sleep(POLL);
        }
    }

    /// Stops its wall time, while it waits on something that may not count toward it, such as another container
    /// starting: its watchdog can't end it for wall time until [`Running::resume`].
    pub fn pause(&self) {
        if let Some(container) = self.container.as_ref() {
            container.shared.paused.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Instant::now);
        }
    }

    /// Starts its wall time again, with `skip`, at most the time it stood still, not counted.
    pub fn resume(&self, skip: Duration) {
        if let Some(container) = self.container.as_ref() {
            let since = container.shared.paused.lock().unwrap_or_else(|e| e.into_inner()).take();
            let skip = since.map_or(Duration::ZERO, |at| skip.min(at.elapsed()));
            let ms = u64::try_from(skip.as_millis()).unwrap_or(u64::MAX);
            container.shared.extra.fetch_add(ms, Ordering::SeqCst);
        }
    }

    /// The CPU time the container has used so far, as the supervisor last said it: every 0.1 s.
    pub fn cpu(&self) -> Option<Duration> {
        let container = self.container.as_ref()?;
        Some(Duration::from_micros(container.shared.cpu.load(Ordering::SeqCst)))
    }

    /// Ends the child, and says how it ended: the limit it passed, if one did.
    pub fn finish(mut self) -> Exit {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(watch) = self.watch.take() {
            let _ = watch.join();
        }
        drop(self.stdin.take());
        let mut container = self.container.take().expect("not yet finished");
        let why = self.why.lock().unwrap_or_else(|e| e.into_inner()).take();
        // Ended now if it hasn't ended itself: what the code does after its input closes is never waited for.
        if container.shared.end.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            container.kill();
        }
        container.end(why).map(|(exit, _)| exit).unwrap_or(Exit::Code(-1))
    }
}

impl Drop for Running {
    /// The watchdog stops, and the container, if [`Running::finish`] didn't end it, is ended and removed.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// What [`Running::next_line`] found.
pub enum Line {
    Got(String),
    /// Nothing by the deadline.
    Late,
    /// The child's output has ended.
    Closed,
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

/// Runs `project`'s public tests on a copy of `tree` (laid out as the seed is) in the pinned image, under the
/// profile and the project's limits.
pub fn run_tests(python: &Python, project: &Project, tree: &Path) -> Result<TestRun, String> {
    let scratch = Scratch::new()?;
    let copy = scratch.copy_tree(tree, "tree")?;
    let runner = scratch.dir.join("run_tests.py");
    std::fs::write(&runner, RUN_TESTS).map_err(|e| format!("writing the runner: {e}"))?;
    let tests = project.public_tests.strip_prefix(&project.seed).expect("checked to be in the seed");
    let marker = format!("bracken-run-tests-{}:", blake3::hash(scratch.dir.as_os_str().as_encoded_bytes()).to_hex());
    let args = [scratch.inside(&runner), scratch.inside(&copy), tests.as_os_str().to_owned(), marker.clone().into()];
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

#[cfg(test)]
pub(crate) mod tests;
