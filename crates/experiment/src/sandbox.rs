//! The `sandbox-exec` profiles (design section 3, Isolation): each process
//! runs under one that allows only its row of the process table and denies
//! everything else.
//!
//! - **agent**: the harness and the shim it starts, under one of a closed set
//!   of [`Profile`]s the harness names, never its own text. Reads the system,
//!   the installation, its own directory and the turn configuration; writes
//!   only its own directory; starts only the pinned program, `/usr/bin/env` and
//!   the shim; connects only to the forwarding proxy's port and its one socket.
//!   No mach service at all, so no keychain.
//! - **tools**: one agent's worker. Reads and writes only that agent's copy;
//!   starts only the worker binary, `bracken` and git (with its exec path),
//!   never a shell; connects only to its own sync socket.
//! - **hub**: the hub worker. The same, for the hub, never a copy.
//! - **python**: agent code in the pinned interpreter ([`crate::python`]). Reads only the interpreter's
//!   installation, the libraries it loads and the system's time zone data; reads and writes only its scratch
//!   directory; starts only the interpreter, and never forks; no network.
//!
//! No profile can read another process's arguments, environment, path or
//! information, or list processes through the process table ([`BASE`]).
//! Which pids are live can still be learnt, one pid at a time.
//!
//! Every path a rule names is resolved first: the kernel checks resolved
//! paths, so a rule on `/tmp/x` never matches `/private/tmp/x`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// macOS's `sandbox-exec`.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// What every profile allows: loading the system's libraries and nothing of the owner's, and no other
/// process's information (task e23f87035615). Reading another process takes both denies (note 116ae2144d9d):
/// without the `kern.proc` prefix, `KERN_PROCARGS2` still reads its arguments and environment, a parent's
/// secrets included, and `process-info*` must be unqualified, since `(target others)` leaves out the process
/// group. A process may still read its own, as the check runner's profile found cargo needs (#160).
/// `getpgid` and `getsid` are denied too, which would otherwise tell any pid's group and session (task
/// 72fd057d4b33): for any pid, its own included, since the rule can't tell them apart. `getpgrp` stays.
/// Which pids are live stays open (finding f55c629069a9): `getpriority` answers for a live pid or group, and
/// a denied call fails differently for a live pid and a dead one. That gives no arguments, environment, name
/// or path.
const BASE: &str = r#"(version 1)
(deny default)
(allow process-fork)
(allow sysctl-read)
(deny sysctl-read (sysctl-name-prefix "kern.proc"))
(deny process-info*)
(allow process-info* (target self))
(deny syscall-unix (syscall-number SYS_getpgid SYS_getsid))
(allow file-read-metadata)
(allow file-read*
  (literal "/") (literal "/dev/null") (literal "/dev/urandom") (literal "/dev/random")
  (subpath "/System") (subpath "/usr/lib") (subpath "/private/var/db/dyld"))
(allow file-write-data (literal "/dev/null"))
(allow signal (target same-sandbox))
"#;

const AGENT: &str = r#"(allow file-read*
  (subpath "/usr/share") (subpath "/private/etc/ssl") (subpath "/private/var/db/timezone")
  (literal "/private/etc/localtime")
  (subpath (param "INSTALL")) (subpath (param "TURN")) (literal (param "SHIM")))
(allow file-read* file-write* (subpath (param "OWN")))
(allow process-exec (literal (param "PROGRAM")) (literal "/usr/bin/env") (literal (param "SHIM")))
(allow network-outbound
  (remote ip (string-append "localhost:" (param "PORT")))
  (remote unix-socket (path-literal (param "SOCKET"))))
"#;

/// What agent code in Python may touch.
pub struct PythonPaths<'a> {
    /// The pinned interpreter, resolved.
    pub python: &'a Path,
    /// Its installation.
    pub prefix: &'a Path,
    /// The libraries outside it that it loads.
    pub libraries: &'a [PathBuf],
    /// The names it loads some of them by, through symlinks.
    pub links: &'a [PathBuf],
    /// The run's own directory: the tree, the runner and anything the code writes.
    pub scratch: &'a Path,
}

/// The tools and hub profiles differ only in their store and sockets.
const WORKER: &str = r#"(allow file-read* (literal (param "WORKER")) (literal (param "BRACKEN")))
(allow file-read* file-write* (subpath (param "STORE")))
(allow process-exec (literal (param "WORKER")) (literal (param "BRACKEN")))
"#;

/// git, when the mode uses it: the real binary and the helpers in its exec path.
const GIT: &str = r#"(allow file-read* (literal (param "GIT")) (subpath (param "GIT_EXEC_PATH")))
(allow process-exec (literal (param "GIT")) (subpath (param "GIT_EXEC_PATH")))
"#;

/// Agent code in Python: the system's libraries as every profile has, but no fork, so no process but the
/// interpreter; its installation, the time zone data and the scratch directory, which holds the tree.
const PYTHON: &str = r#"(allow file-read*
  (subpath "/usr/share/zoneinfo") (subpath "/private/var/db/timezone") (literal "/private/etc/localtime")
  (subpath (param "PREFIX")) (literal (param "PYTHON")))
(allow file-read* file-write* (subpath (param "SCRATCH")))
(allow process-exec (literal (param "PYTHON")))
"#;

/// The agent profiles a harness may run under. The backend owns their text: a harness names one, so no
/// harness writes its own rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// A native program, such as Claude Code's binary: the agent rules alone.
    Agent,
    /// A program Node.js runs, such as Pi: the agent rules, plus only what a run under it shows Node needs,
    /// each with why. None is shown yet, so its text is the agent rules' (security note `b8bbcf81bc17` (e)).
    Node,
}

impl Profile {
    /// Its name, in errors and the plan.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Agent => "agent",
            Profile::Node => "node",
        }
    }

    fn text(self) -> String {
        match self {
            Profile::Agent | Profile::Node => format!("{BASE}{AGENT}"),
        }
    }
}

/// A profile and the values of its parameters.
#[derive(Debug, Clone)]
pub struct Sandbox {
    text: String,
    params: Vec<(String, String)>,
}

/// git's binary and its exec path, for a mode that runs git.
#[derive(Debug, Clone)]
pub struct Git {
    pub binary: PathBuf,
    pub exec_path: PathBuf,
}

impl Git {
    /// `binary` (by absolute path) and the directory it runs its helpers from, as `git --exec-path`
    /// says, resolved. Not the folder of the resolved binary: with Xcode's git, that is a `bin` of
    /// over a hundred tools (note `bb68a9908903`).
    pub fn of(binary: &Path) -> Result<Git, String> {
        let out = Command::new(binary)
            .arg("--exec-path")
            .env_clear()
            .output()
            .map_err(|e| format!("asking git for its exec path: {e}"))?;
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || dir.is_empty() {
            return Err("git gave no exec path".into());
        }
        let exec_path = std::fs::canonicalize(&dir).map_err(|e| format!("resolving git's exec path {dir}: {e}"))?;
        Ok(Git { binary: binary.to_path_buf(), exec_path })
    }
}

/// What an agent's harness may touch.
pub struct AgentPaths<'a> {
    /// The harness's pinned program, the one it starts.
    pub program: &'a Path,
    /// What it was installed in ([`crate::backend::Pin::install`]): it reads its own files there.
    pub install: &'a Path,
    /// Its own directory: `HOME`, the temporary directory and the harness's own are inside.
    pub own: &'a Path,
    /// The turn configuration, read-only.
    pub turn: &'a Path,
    /// The `bracken-experiment` binary, started as the shim.
    pub shim: &'a Path,
    /// The agent's socket to the driver.
    pub socket: &'a Path,
    /// The forwarding proxy's port on localhost.
    pub proxy_port: u16,
}

/// What a worker may touch.
pub struct WorkerPaths<'a> {
    /// Its store: an agent's copy (tools profile) or the hub (hub profile).
    pub store: &'a Path,
    /// The `bracken-experiment` binary it runs as, and runs again as the sync relay.
    pub worker: &'a Path,
    pub bracken: &'a Path,
    pub git: Option<&'a Git>,
    /// The sync sockets it may connect to: its own (tools), or every agent's hub end (hub).
    pub sockets: &'a [PathBuf],
}

/// `path` resolved, as the kernel will see it. A socket that doesn't exist yet is
/// resolved through its directory.
pub fn resolved(path: &Path) -> Result<PathBuf, String> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(_) => {
            let (dir, name) = (path.parent().unwrap_or(Path::new("/")), path.file_name());
            let name = name.ok_or_else(|| format!("{} names no file", path.display()))?;
            let dir = std::fs::canonicalize(dir).map_err(|e| format!("resolving {}: {e}", dir.display()))?;
            Ok(dir.join(name))
        }
    }
}

/// Adds to `out` every path the kernel looks up to resolve the absolute `path`: each directory on the way,
/// each symlink, and each path a symlink's target leads through.
fn lookups(path: &Path, out: &mut std::collections::BTreeSet<PathBuf>, depth: u32) -> Result<(), String> {
    use std::path::Component;
    if depth > 32 {
        return Err(format!("resolving {}: too many symlinks", path.display()));
    }
    let mut at = PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => {
                at.pop();
                continue;
            }
            Component::Normal(name) => at.push(name),
            Component::Prefix(_) => return Err(format!("{} isn't a Unix path", path.display())),
        }
        out.insert(at.clone());
        if let Ok(target) = std::fs::read_link(&at) {
            let next = at.parent().unwrap_or(Path::new("/")).join(target);
            lookups(&next, out, depth + 1)?;
            at = resolved(&at)?;
        }
    }
    Ok(())
}

fn param(name: &str, path: &Path) -> Result<(String, String), String> {
    unresolved(name, &resolved(path)?)
}

/// A parameter for `path` as it is: a symlink's own path, which the kernel checks when it looks the link up.
fn unresolved(name: &str, path: &Path) -> Result<(String, String), String> {
    let s = path.to_str().ok_or_else(|| format!("{} is not UTF-8", path.display()))?;
    Ok((name.to_string(), s.to_string()))
}

impl Sandbox {
    /// An agent profile.
    pub fn agent(profile: Profile, p: &AgentPaths) -> Result<Sandbox, String> {
        let params = vec![
            param("PROGRAM", p.program)?,
            param("INSTALL", p.install)?,
            param("OWN", p.own)?,
            param("TURN", p.turn)?,
            param("SHIM", p.shim)?,
            param("SOCKET", p.socket)?,
            ("PORT".to_string(), p.proxy_port.to_string()),
        ];
        Ok(Sandbox { text: profile.text(), params })
    }

    /// The tools profile: one agent's copy.
    pub fn tools(p: &WorkerPaths) -> Result<Sandbox, String> {
        Self::worker(p)
    }

    /// The hub profile: the mode's hub.
    pub fn hub(p: &WorkerPaths) -> Result<Sandbox, String> {
        Self::worker(p)
    }

    fn worker(p: &WorkerPaths) -> Result<Sandbox, String> {
        let mut text = format!("{BASE}{WORKER}");
        let mut params = vec![param("STORE", p.store)?, param("WORKER", p.worker)?, param("BRACKEN", p.bracken)?];
        if let Some(git) = p.git {
            text.push_str(GIT);
            params.push(param("GIT", &git.binary)?);
            params.push(param("GIT_EXEC_PATH", &git.exec_path)?);
        }
        if !p.sockets.is_empty() {
            text.push_str("(allow network-outbound");
            for (n, socket) in p.sockets.iter().enumerate() {
                let name = format!("SOCKET{n}");
                text.push_str(&format!("\n  (remote unix-socket (path-literal (param \"{name}\")))"));
                params.push(param(&name, socket)?);
            }
            text.push_str(")\n");
        }
        Ok(Sandbox { text, params })
    }

    /// The python profile: one run of agent code. BASE without fork, and without its unscoped
    /// `file-read-metadata`: only the paths looked up on the way to what it may read can be stat'd (finding
    /// 8b414a06f294). Whether a path exists still shows, and `sandbox-exec` can't hide it: a refused lookup fails
    /// with `EPERM`, a missing one with `ENOENT`, and `access` answers either way (finding c932586417301). So
    /// nothing else of a path shows, but no agent may ever be given the private store's path. And no other process's
    /// information at all (finding c44e0191291a, note 116ae2144d9d): each of the two denies alone still lets
    /// `KERN_PROCARGS2` read another process's arguments and environment, and both together refuse it, while
    /// `os.uname`, which needs `sysctl-read`, still works. BASE has them too; repeated last, they take back its
    /// allowing a process its own information, which the interpreter doesn't need.
    pub fn python(p: &PythonPaths) -> Result<Sandbox, String> {
        let base = BASE.replace("(allow process-fork)\n", "").replace("(allow file-read-metadata)\n", "");
        assert_eq!(base.len(), BASE.len() - "(allow process-fork)\n(allow file-read-metadata)\n".len());
        let mut text = format!("{base}{PYTHON}");
        let mut params = vec![param("PYTHON", p.python)?, param("PREFIX", p.prefix)?, param("SCRATCH", p.scratch)?];
        for (n, lib) in p.libraries.iter().enumerate() {
            let name = format!("LIB{n}");
            text.push_str(&format!("(allow file-read* (literal (param \"{name}\")))\n"));
            params.push(param(&name, lib)?);
        }
        let mut above = std::collections::BTreeSet::new();
        let fixed = ["/usr/share/zoneinfo", "/private/var/db/timezone", "/private/etc/localtime"].map(Path::new);
        let read = [p.python, p.prefix, p.scratch]
            .into_iter()
            .chain(p.libraries.iter().chain(p.links).map(PathBuf::as_path))
            .chain(fixed);
        for path in read {
            lookups(path, &mut above, 0)?;
        }
        text.push_str("(allow file-read-metadata");
        for (n, dir) in above.iter().enumerate() {
            let name = format!("ABOVE{n}");
            text.push_str(&format!("\n  (literal (param \"{name}\"))"));
            params.push(unresolved(&name, dir)?);
        }
        text.push_str(")\n(deny sysctl-read (sysctl-name-prefix \"kern.proc\"))\n(deny process-info*)\n");
        Ok(Sandbox { text, params })
    }

    /// The profile's text, for the run's plan.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// A command that runs `program` under this profile.
    pub fn command(&self, program: &Path) -> Command {
        let mut cmd = Command::new(SANDBOX_EXEC);
        for (k, v) in &self.params {
            cmd.arg("-D").arg(format!("{k}={v}"));
        }
        cmd.arg("-p").arg(&self.text).arg(program);
        cmd
    }
}
