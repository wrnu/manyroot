//! Driving the real `bracken` binary as a subprocess.
//!
//! Every call names its replica with `--dir` and runs in the agent's own work
//! directory with `BRACKEN_DIR` and `BRACKEN_AGENT` removed, so nothing a
//! caller inherits can point a call at another Ground.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

/// The `bracken` binary, with counters for the report.
pub struct Bracken {
    bin: PathBuf,
    calls: Cell<u64>,
    elapsed: Cell<Duration>,
}

impl Bracken {
    /// The binary at `bin`, resolved to an absolute path now, so no later call can
    /// find a different one through `PATH` or a child's working directory.
    pub fn new(bin: &Path) -> Result<Bracken> {
        let missing =
            || anyhow!("no bracken binary at {}; run `cargo build -p bracken` or pass --bracken <path>", bin.display());
        let bin = std::fs::canonicalize(bin).map_err(|_| missing())?;
        if !is_executable_file(&bin) {
            return Err(missing());
        }
        Ok(Bracken { bin, calls: Cell::new(0), elapsed: Cell::new(Duration::ZERO) })
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    pub fn calls(&self) -> u64 {
        self.calls.get()
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed.get()
    }

    /// Runs `bracken --dir <dir> <args>` in `cwd` and returns its stdout.
    fn run(&self, dir: &Path, cwd: &Path, args: &[&OsStr]) -> Result<String> {
        let start = Instant::now();
        let out = Command::new(&self.bin)
            .arg("--dir")
            .arg(dir)
            .args(args)
            .current_dir(cwd)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .with_context(|| format!("running {}", self.bin.display()))?;
        self.calls.set(self.calls.get() + 1);
        self.elapsed.set(self.elapsed.get() + start.elapsed());
        if !out.status.success() {
            let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            bail!("bracken {} failed: {}", shown.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        String::from_utf8(out.stdout).context("bracken printed non-UTF-8 output")
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// One agent's replica of the Ground, and the identity it signs with.
pub struct Replica<'b> {
    bracken: &'b Bracken,
    /// The agent's work directory: commands run here, files are written here.
    workdir: PathBuf,
    /// The replica itself, `<workdir>/.bracken`.
    dir: PathBuf,
    identity: String,
}

/// One row of `bracken log --json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub retracted: bool,
    pub body: Value,
}

impl Entry {
    pub fn text(&self, field: &str) -> &str {
        self.body[field].as_str().unwrap_or_default()
    }

    pub fn tags(&self) -> Vec<&str> {
        self.body["tags"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
    }
}

impl<'b> Replica<'b> {
    /// A replica in `<workdir>/.bracken`, signing as `identity`. Creates the work directory.
    pub fn new(bracken: &'b Bracken, workdir: PathBuf, identity: &str) -> Result<Replica<'b>> {
        // Bracken runs inside the work directory, so a relative path would resolve twice.
        let workdir = std::path::absolute(&workdir).with_context(|| format!("resolving {}", workdir.display()))?;
        std::fs::create_dir_all(&workdir).with_context(|| format!("creating {}", workdir.display()))?;
        let dir = workdir.join(".bracken");
        Ok(Replica { bracken, workdir, dir, identity: identity.to_owned() })
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The absolute path of the bracken binary.
    pub fn bracken_bin(&self) -> &Path {
        self.bracken.bin()
    }

    fn run(&self, args: &[&OsStr]) -> Result<String> {
        self.bracken.run(&self.dir, &self.workdir, args)
    }

    fn run_str(&self, args: &[&str]) -> Result<String> {
        self.run(&args.iter().map(OsStr::new).collect::<Vec<_>>())
    }

    /// Runs a write command signed by this replica's identity, and returns the new id.
    fn write(&self, args: &[&OsStr]) -> Result<String> {
        let mut all = args.to_vec();
        all.extend([OsStr::new("--as"), OsStr::new(&self.identity)]);
        let out = self.run(&all)?;
        // The new contribution's id is the first line; later lines may list related ids.
        Ok(out.lines().next().unwrap_or_default().trim().to_owned())
    }

    fn json(&self, args: &[&str]) -> Result<Value> {
        let out = self.run_str(args)?;
        serde_json::from_str(&out).with_context(|| format!("parsing the output of bracken {}", args.join(" ")))
    }

    pub fn init(&self, name: &str) -> Result<()> {
        self.run_str(&["init", "--name", name]).map(drop)
    }

    pub fn join(&self, bundle: &Path) -> Result<()> {
        self.run(&[OsStr::new("join"), bundle.as_os_str()]).map(drop)
    }

    /// Creates this replica's identity and returns its public key.
    pub fn create_identity(&self) -> Result<String> {
        Ok(self.run_str(&["id", "new", &self.identity])?.trim().to_owned())
    }

    pub fn claim(&self, statement: &str, tags: &[&str]) -> Result<String> {
        let mut args: Vec<&OsStr> = vec![OsStr::new("claim"), OsStr::new(statement)];
        for t in tags {
            args.extend([OsStr::new("--tag"), OsStr::new(t)]);
        }
        self.write(&args)
    }

    /// Writes `content` to `file` in the work directory and contributes it as an artifact.
    pub fn artifact(&self, file: &str, name: &str, media_type: &str, content: &str) -> Result<String> {
        let path = self.workdir.join(file);
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        let args: [&OsStr; 6] = [
            "artifact".as_ref(),
            path.as_os_str(),
            "--name".as_ref(),
            name.as_ref(),
            "--media-type".as_ref(),
            media_type.as_ref(),
        ];
        self.write(&args)
    }

    pub fn relate(&self, from: &str, kind: &str, to: &str, note: &str) -> Result<String> {
        let args: [&OsStr; 6] =
            ["relate".as_ref(), from.as_ref(), kind.as_ref(), to.as_ref(), "--note".as_ref(), note.as_ref()];
        self.write(&args)
    }

    pub fn evidence(&self, target: &str, pass: bool, method: &str, details: &str) -> Result<String> {
        let verdict = if pass { "pass" } else { "fail" };
        let args: [&OsStr; 7] = [
            "evidence".as_ref(),
            target.as_ref(),
            verdict.as_ref(),
            "--method".as_ref(),
            method.as_ref(),
            "--details".as_ref(),
            details.as_ref(),
        ];
        self.write(&args)
    }

    /// Every contribution, in causal order.
    pub fn log(&self) -> Result<Vec<Entry>> {
        let v = self.json(&["log", "--json"])?;
        serde_json::from_value(v).context("reading bracken log --json")
    }

    pub fn conflicts(&self) -> Result<Value> {
        self.json(&["conflicts", "--json"])
    }

    pub fn accepted(&self, min_passes: usize) -> Result<Value> {
        self.json(&["accepted", "--min-passes", &min_passes.to_string(), "--json"])
    }

    pub fn export(&self, bundle: &Path) -> Result<()> {
        self.run(&[OsStr::new("export"), bundle.as_os_str()]).map(drop)
    }

    pub fn import(&self, bundle: &Path) -> Result<()> {
        self.run(&[OsStr::new("import"), bundle.as_os_str()]).map(drop)
    }
}
