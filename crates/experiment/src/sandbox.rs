//! The `sandbox-exec` profiles (design section 3, Isolation): each process
//! runs under one that allows only its row of the process table and denies
//! everything else.
//!
//! - **agent**: Claude Code and the shim it starts. Reads the system, the
//!   installation, its own directory and the turn configuration; writes only
//!   its own directory; starts only the pinned binary, `/usr/bin/env` and the
//!   shim; connects only to the forwarding proxy's port and its one socket.
//!   No mach service at all, so no keychain.
//! - **tools**: one agent's worker. Reads and writes only that agent's copy;
//!   starts only the worker binary, `bracken` and git (with its exec path),
//!   never a shell; connects only to its own sync socket.
//! - **hub**: the hub worker. The same, for the hub, never a copy.
//!
//! Every path a rule names is resolved first: the kernel checks resolved
//! paths, so a rule on `/tmp/x` never matches `/private/tmp/x`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// macOS's `sandbox-exec`.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// What every profile allows: loading the system's libraries and nothing of the owner's.
const BASE: &str = r#"(version 1)
(deny default)
(allow process-fork)
(allow sysctl-read)
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
(allow process-exec (literal (param "CLAUDE")) (literal "/usr/bin/env") (literal (param "SHIM")))
(allow network-outbound
  (remote ip (string-append "localhost:" (param "PORT")))
  (remote unix-socket (path-literal (param "SOCKET"))))
"#;

/// The tools and hub profiles differ only in their store and sockets.
const WORKER: &str = r#"(allow file-read* (literal (param "WORKER")) (literal (param "BRACKEN")))
(allow file-read* file-write* (subpath (param "STORE")))
(allow process-exec (literal (param "WORKER")) (literal (param "BRACKEN")))
"#;

/// git, when the mode uses it: the real binary and the helpers in its exec path.
const GIT: &str = r#"(allow file-read* (literal (param "GIT")) (subpath (param "GIT_EXEC_PATH")))
(allow process-exec (literal (param "GIT")) (subpath (param "GIT_EXEC_PATH")))
"#;

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

/// What an agent's Claude Code may touch.
pub struct AgentPaths<'a> {
    /// The pinned binary.
    pub claude: &'a Path,
    /// The directory it was installed in: it reads its own files there.
    pub install: &'a Path,
    /// Its own directory: `HOME`, `CLAUDE_CONFIG_DIR` and the temporary directories are inside.
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

fn param(name: &str, path: &Path) -> Result<(String, String), String> {
    let p = resolved(path)?;
    let s = p.to_str().ok_or_else(|| format!("{} is not UTF-8", p.display()))?;
    Ok((name.to_string(), s.to_string()))
}

impl Sandbox {
    /// The agent profile.
    pub fn agent(p: &AgentPaths) -> Result<Sandbox, String> {
        let params = vec![
            param("CLAUDE", p.claude)?,
            param("INSTALL", p.install)?,
            param("OWN", p.own)?,
            param("TURN", p.turn)?,
            param("SHIM", p.shim)?,
            param("SOCKET", p.socket)?,
            ("PORT".to_string(), p.proxy_port.to_string()),
        ];
        Ok(Sandbox { text: format!("{BASE}{AGENT}"), params })
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
