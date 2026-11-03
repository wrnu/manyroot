//! The agent backend, harness-neutral (docs/design/agents.md, Evaluating
//! configurations): plays each agent turn with one process of some agent
//! harness, and reads its event stream as it arrives. A [`Harness`] declares
//! its pinned program, its turn configuration and the environment entries it
//! may add, and gives each turn's arguments and how to read its events;
//! [`HarnessBackend`] does the rest, the same for every harness, so no
//! harness can leave a guarantee out. Claude Code ([`crate::claude`]) is the
//! first harness.
//!
//! - **The process**: the backend builds it. The pinned program (path and
//!   blake3, of the binary or of its whole install, checked before every turn)
//!   under the agent sandbox profile the harness names; the
//!   turn configuration written read-only and checked before every turn; an
//!   environment cleared, then [`BASE_ENV`] (a home and temporary directory
//!   inside the agent's own, a minimal `PATH`, `HTTPS_PROXY` at the forwarding
//!   proxy) and only the entries the harness declared; standard input null.
//! - **The gate**: the first event must be the harness's start event, showing
//!   exactly what the plan expects; anything else ends the turn and the run.
//! - **Token accounting**: every model call's usage and the tools it asked
//!   for, and the turn's result usage, go to `usage.jsonl`.
//! - **Caps, failing closed**: tool calls, context and time per turn end the
//!   turn; an agent's tokens end its turns; the run's tokens, usage that can't
//!   be read, or a stream and result that disagree on input by more than 10%
//!   stop the run. Output is known only from the result (the stream's figure
//!   is a lower bound), so a turn without one is counted at [`Caps::turn_output`].
//! - **Process groups**: the harness runs in a group led by a guard process,
//!   killed on every exit path; if the driver dies, the guard sees its pipe
//!   close and kills the group itself.
//! - **Transcripts**: each turn's event stream, checked for the harness's
//!   secret before it is kept.
//! - **The forwarding proxy**: the agent's only way out, to the harness's route.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::driver::Backend;
use crate::proxy::{Attempt, Proxy, Route};
use crate::sandbox::{AgentPaths, Profile, Sandbox, resolved};
use crate::worker::Lines;

/// The environment every turn's process gets from the backend, the same for every harness: `HOME` and
/// `TMPDIR` inside the agent's own directory, `PATH` of the system's directories only, and `HTTPS_PROXY`
/// at the forwarding proxy. A harness may add only names it declares ([`Harness::env_names`]), never these.
pub const BASE_ENV: [&str; 4] = ["HOME", "TMPDIR", "PATH", "HTTPS_PROXY"];

/// The shortest secret a harness may give ([`Harness::secret`]): a shorter one would match too much to
/// check transcripts for.
pub const MIN_SECRET: usize = 8;

/// The run's own caps (design section 5): stops a plan expects, which the driver logs by name.
pub const RUN_TOKEN_CAP: &str = "the run's token cap is reached";
pub const RUN_TIME_CAP: &str = "the run's time cap is reached";

/// The message that starts each turn, the same in every mode ([`crate::prompts::turn_message`]); the plan hashes it.
pub const TURN_MESSAGE: &str = "This is your turn {turn} of {turns}. You are {name}. Start by reading your notes.";

/// The longest event line read from a harness.
const MAX_EVENT: usize = 8 * 1024 * 1024;
/// How long standard error may take to close once the group is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// How often a waiting turn looks at whether the run has stopped.
const POLL: Duration = Duration::from_millis(100);

/// Section 5's caps.
#[derive(Debug, Clone)]
pub struct Caps {
    /// Tool calls asked for in one turn. Past it the driver refuses each call and the turn has
    /// [`Caps::cap_grace`] and [`Caps::cap_grace_calls`] to end itself (decision `147a36d5d623`).
    pub calls_per_turn: u32,
    /// How long a turn past its tool-call cap may take to write its result; then the process ends.
    pub cap_grace: Duration,
    /// How many more model calls a turn past its tool-call cap may make; at the next, the process ends.
    pub cap_grace_calls: usize,
    /// Context of one model call (input, cache reads and cache writes).
    pub context: u64,
    pub turn_time: Duration,
    /// The output a turn without a readable result is counted at.
    pub turn_output: u64,
    /// `--max-budget-usd`: Claude Code's own stop, on its API-equivalent estimate, which sees output as it grows.
    pub turn_budget_usd: Option<f64>,
    /// One agent's tokens over the run; past them it takes no more turns (`--agent-token-cap`).
    pub agent_tokens: u64,
    /// One agent's output tokens over the run; past them it takes no more turns (`--agent-output-cap`).
    pub agent_output: u64,
    pub run_tokens: u64,
    pub run_output: u64,
    /// Wall-clock time from the backend's start; past it the run stops.
    pub run_time: Duration,
}

/// The tokens of a whole replicate (the owner's go, approval `1316798b18f9`): no one mode's cap may pass it.
pub const REPLICATE_TOKENS: u64 = 75_000_000;

/// One mode's token cap for a run, given as `--mode-token-cap <mode>=<tokens>` (the manager's note
/// `2018c9beceff`, decision `c90a34c1ca38`): a run of that mode stops at `tokens` instead of the pilot's
/// [`Caps::run_tokens`], higher or lower; runs of other modes are unchanged. At most [`REPLICATE_TOKENS`].
/// The replicate's total is kept by the run order: its last mode runs at what is left of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeTokenCap {
    pub mode: String,
    pub tokens: u64,
}

impl std::str::FromStr for ModeTokenCap {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (mode, tokens) = s.split_once('=').ok_or_else(|| format!("{s:?}: expected <mode>=<tokens>"))?;
        if crate::mode::mode_named(mode).is_none() {
            return Err(format!("{mode:?}: not a mode (bracken, git or crdt)"));
        }
        if tokens.is_empty() || !tokens.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("{tokens:?}: expected a whole number of tokens above 0"));
        }
        let tokens = match tokens.parse::<u64>() {
            Ok(0) => return Err("0: expected a whole number of tokens above 0".to_string()),
            Ok(n) if n <= REPLICATE_TOKENS => n,
            _ => return Err(format!("{tokens}: above {REPLICATE_TOKENS}, the whole replicate's tokens")),
        };
        Ok(ModeTokenCap { mode: mode.to_string(), tokens })
    }
}

impl Caps {
    /// The caps of one run of `mode`: the pilot's, with the run's token cap from the entry of `given` naming
    /// `mode`, if there is one. Without one they are exactly [`Caps::pilot`]. A mode named twice is refused.
    pub fn for_mode(mode: &str, given: &[ModeTokenCap]) -> Result<Caps, String> {
        let mut caps = Caps::pilot();
        for (i, c) in given.iter().enumerate() {
            if given[..i].iter().any(|d| d.mode == c.mode) {
                return Err(format!("--mode-token-cap names {} twice", c.mode));
            }
            if c.mode == mode {
                caps.run_tokens = c.tokens;
            }
        }
        Ok(caps)
    }

    /// These caps with one agent's token and output caps in place of the pilot's, where given (`run
    /// --agent-token-cap`, `--agent-output-cap`, task `ba049eeaff56`). Neither may pass the run's cap of the
    /// same kind, [`Caps::run_tokens`] and [`Caps::run_output`]: one agent can't use more than the whole run.
    pub fn with_agent_caps(self, tokens: Option<u64>, output: Option<u64>) -> Result<Caps, String> {
        let caps = Caps {
            agent_tokens: tokens.unwrap_or(self.agent_tokens),
            agent_output: output.unwrap_or(self.agent_output),
            ..self
        };
        if caps.agent_tokens > caps.run_tokens {
            return Err(format!(
                "--agent-token-cap {} is above the run's token cap, {}: raise that with --mode-token-cap",
                caps.agent_tokens, caps.run_tokens
            ));
        }
        if caps.agent_output > caps.run_output {
            return Err(format!(
                "--agent-output-cap {} is above the run's output cap, {}",
                caps.agent_output, caps.run_output
            ));
        }
        Ok(caps)
    }

    /// The caps as the plan records them, and so as its hash covers them.
    pub fn plan_value(&self) -> serde_json::Value {
        serde_json::Value::String(format!("{self:?}"))
    }

    /// The pilot's caps (design section 5), with the per-turn call cap at 50 since owner decision
    /// `63f4fd5cab66`, as the driver's [`crate::driver::CALLS_PER_TURN`].
    pub fn pilot() -> Caps {
        Caps {
            calls_per_turn: crate::driver::CALLS_PER_TURN,
            cap_grace: Duration::from_secs(60),
            cap_grace_calls: 3,
            context: 150_000,
            turn_time: Duration::from_secs(600),
            turn_output: 30_000,
            // About twice a turn's expected use at the pinned model's price (design section 5).
            turn_budget_usd: Some(1.0),
            agent_tokens: 3_000_000,
            agent_output: 180_000,
            // One run is one mode, and the modes run one after another: each run's caps are a third
            // of the owner's go for the whole pilot, 75M tokens, 5M output and 3 h (decisions
            // `3418fe14b8ed`, `ea053c2c4d48`), equal in every mode. A run may take its mode's token cap in
            // place of this (`--mode-token-cap`, [`Caps::for_mode`]).
            run_tokens: 25_000_000,
            run_output: 1_666_666,
            run_time: Duration::from_secs(3600),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Tokens {
    // Sums saturate: a figure too large to add pins the total at the top, so the caps trip.
    fn input_side(&self) -> u64 {
        self.input.saturating_add(self.cache_read).saturating_add(self.cache_write)
    }

    pub fn total(&self) -> u64 {
        self.input_side().saturating_add(self.output)
    }

    pub fn add(&mut self, o: &Tokens) {
        self.input = self.input.saturating_add(o.input);
        self.cache_read = self.cache_read.saturating_add(o.cache_read);
        self.cache_write = self.cache_write.saturating_add(o.cache_write);
        self.output = self.output.saturating_add(o.output);
    }

    /// What `self` has beyond `o`, field by field.
    fn beyond(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
            output: self.output.saturating_sub(o.output),
        }
    }

    /// The larger of the two, field by field.
    pub fn max(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.max(o.input),
            cache_read: self.cache_read.max(o.cache_read),
            cache_write: self.cache_write.max(o.cache_write),
            output: self.output.max(o.output),
        }
    }
}

/// A harness's pinned program: one binary ([`Pin::of`]), or a program and the whole tree it runs from
/// ([`Pin::tree`]). Its fields are private, so only those two make one, and [`Pin::check`] re-asserts
/// what they established (finding ab0192deac8f).
#[derive(Debug, Clone)]
pub struct Pin {
    /// Its real path: the versioned binary, not a package manager's `latest` link.
    path: PathBuf,
    /// What it reads its own files from, resolved: the tree a tree pin covers, else the binary's
    /// directory, which the sandbox lets it read but isn't hashed.
    install: PathBuf,
    /// blake3, hex: of the binary's bytes, or of the tree's manifest ([`tree_hash`]).
    blake3: String,
    /// The version its start event must show.
    version: String,
    tree: bool,
}

impl Pin {
    /// Pins the binary at `path` as it is now.
    pub fn of(path: &Path, version: &str) -> Result<Pin, String> {
        let path = std::fs::canonicalize(path).map_err(|e| format!("resolving {}: {e}", path.display()))?;
        let install = path.parent().ok_or("the pinned binary has no directory")?.to_path_buf();
        Ok(Pin { blake3: file_hash(&path)?, path, install, version: version.into(), tree: false })
    }

    /// Pins `program` and everything under `root`, which must hold it, as they are now: for a program an
    /// interpreter runs from its own packages, such as Node.js and Pi's (security note `b8bbcf81bc17` (d)).
    pub fn tree(program: &Path, root: &Path, version: &str) -> Result<Pin, String> {
        let install = std::fs::canonicalize(root).map_err(|e| format!("resolving {}: {e}", root.display()))?;
        let path = std::fs::canonicalize(program).map_err(|e| format!("resolving {}: {e}", program.display()))?;
        if !path.starts_with(&install) || !path.is_file() {
            return Err(format!("{} isn't a file inside {}", program.display(), install.display()));
        }
        Ok(Pin { blake3: tree_hash(&install)?, path, install, version: version.into(), tree: true })
    }

    /// blake3, hex: of the binary's bytes, or of the tree's manifest.
    pub fn blake3(&self) -> &str {
        &self.blake3
    }

    /// Whether the program, and a tree pin's whole tree, is still the pinned one; `harness` names it in
    /// the error.
    fn check(&self, harness: &str) -> Result<(), String> {
        // What the constructors established, asserted again before every turn: a tree pin's program is
        // inside its tree, and a binary's install is its own directory.
        let held = match self.tree {
            true => self.path.starts_with(&self.install),
            false => self.path.parent() == Some(self.install.as_path()),
        };
        if !held {
            return Err(format!("the pinned {harness} program isn't inside its install"));
        }
        let real = std::fs::canonicalize(&self.path).map_err(|e| format!("the pinned binary: {e}"))?;
        if !self.tree {
            if real != self.path || file_hash(&real)? != self.blake3 {
                return Err(format!("the pinned {harness} binary changed"));
            }
            return Ok(());
        }
        // The program is inside the tree, so a tree that now resolves elsewhere moves it too.
        let changed = |why: String| format!("the pinned {harness} install changed: {why}");
        if real != self.path {
            return Err(changed("its program resolves elsewhere".into()));
        }
        if tree_hash(&self.install).map_err(changed)? != self.blake3 {
            return Err(changed("its manifest's hash differs".into()));
        }
        Ok(())
    }
}

/// blake3, hex, of the manifest of every entry under `root`, `root` included, in path order. An entry
/// is its kind and path relative to `root`, then a directory's mode, a file's mode and contents' blake3,
/// or a symbolic link's target as written, never followed; each field ends in NUL, which no path or
/// target holds. Refuses a tree an entry could reach outside of: a symbolic link that leads anywhere but
/// through the tree to something in it ([`beneath`]), a file with another hard link, or anything but a
/// file, directory or link.
fn tree_hash(root: &Path) -> Result<String, String> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let mut h = blake3::Hasher::new();
    let mut field = |b: &[u8]| {
        h.update(b);
        h.update(b"\0");
    };
    let mut todo = vec![PathBuf::new()];
    while let Some(rel) = todo.pop() {
        let path = root.join(&rel);
        let shown = || format!("{}", rel.display());
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let mode = format!("{:o}", meta.mode() & 0o7777);
        let kind = meta.file_type();
        if kind.is_dir() {
            field(b"d");
            field(rel.as_os_str().as_bytes());
            field(mode.as_bytes());
            let mut names = Vec::new();
            for e in std::fs::read_dir(&path).map_err(|e| format!("reading {}: {e}", path.display()))? {
                names.push(e.map_err(|e| format!("reading {}: {e}", path.display()))?.file_name());
            }
            // Popped in reverse, so the tree is read in name order, each directory before what it holds:
            // read_dir's own order is the file system's, which on some (tmpfs) is the order of making.
            names.sort_by(|a, b| b.as_bytes().cmp(a.as_bytes()));
            todo.extend(names.into_iter().map(|n| rel.join(n)));
        } else if kind.is_file() {
            if meta.nlink() > 1 {
                return Err(format!("{} has another hard link", shown()));
            }
            field(b"f");
            field(rel.as_os_str().as_bytes());
            field(mode.as_bytes());
            field(file_hash(&path)?.as_bytes());
        } else if kind.is_symlink() {
            let to = std::fs::read_link(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
            if let Err(why) = beneath(root, &rel) {
                return Err(format!("{} links to {}, {why}", shown(), to.display()));
            }
            field(b"l");
            field(rel.as_os_str().as_bytes());
            field(to.as_os_str().as_bytes());
        } else {
            return Err(format!("{} isn't a file, directory or symbolic link", shown()));
        }
    }
    Ok(h.finalize().to_hex().to_string())
}

fn file_hash(path: &Path) -> Result<String, String> {
    let mut h = blake3::Hasher::new();
    let mut f = File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    std::io::copy(&mut f, &mut h).map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(h.finalize().to_hex().to_string())
}

/// One agent turn, as the backend starts it.
pub struct Turn<'a> {
    pub name: &'a str,
    pub turn: u32,
    /// The message that starts it ([`crate::prompts::turn_message`]).
    pub message: &'a str,
    /// The agent's socket to the driver, which the harness's MCP client reaches through the shim.
    pub socket: &'a Path,
    /// The agent's own directory under the backend's, the only one it may write. The harness may make
    /// more directories inside it.
    pub own: &'a Path,
    /// `HOME` and `TMPDIR`, inside `own`, made and resolved.
    pub home: &'a Path,
    pub tmp: &'a Path,
    /// The turn configuration ([`Harness::config`]), read-only and checked, resolved.
    pub config: &'a Path,
    /// The forwarding proxy's port on 127.0.0.1: the agent's only way out.
    pub proxy_port: u16,
    /// The pinned version, as the start took it: what the harness's start event must show.
    pub version: &'a str,
}

/// What a harness adds to the process the backend builds for a turn: its arguments, and environment
/// entries, each named in [`Harness::env_names`].
#[derive(Debug, Default)]
pub struct Launch {
    args: Vec<OsString>,
    env: Vec<(String, OsString)>,
}

impl Launch {
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Launch {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&mut self, args: I) -> &mut Launch {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub fn env(&mut self, name: &str, value: impl AsRef<OsStr>) -> &mut Launch {
        self.env.push((name.to_string(), value.as_ref().to_os_string()));
        self
    }
}

/// What the backend reads from a harness's event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The first event, checked against the plan: why it doesn't match, if it doesn't.
    Init(Result<(), String>),
    /// A model call as far as it has gone. Every event about one call repeats its usage so far.
    Message {
        id: String,
        /// The model the harness says answered, recorded as given.
        model: Value,
        usage: Tokens,
        /// The tool calls it asks for: (call id, tool name).
        tools: Vec<(String, String)>,
    },
    /// The turn's end, with the harness's own figures for it.
    Result {
        /// Its own total, or none if it has none apart from the stream's (Pi): the turn then counts the
        /// stream's sum, recorded as such, and there is no total to check the stream against (answer
        /// cc34f6275877 (2)).
        usage: Option<Tokens>,
        /// How it ended, the cost the harness estimates and how long it took: recorded as given.
        ended: Value,
        cost_usd: Value,
        duration_ms: Value,
    },
    /// Anything else: kept in the transcript only.
    Other,
}

/// An agent harness, as the backend drives it. Every error that a method marks as stopping the run
/// stops it; the others end only the turn.
pub trait Harness: Sync {
    /// Its name, in errors.
    fn name(&self) -> &str;

    /// Where its model calls go: the forwarding proxy lets the agent reach this and nothing else.
    fn route(&self) -> Route;

    /// What no transcript or standard error kept may hold: its credential. At least [`MIN_SECRET`]
    /// bytes, or the backend doesn't start. Read once, at the start: the run checks what it gave then.
    fn secret(&self) -> &str;

    /// The program that plays every turn, and for a tree pin all it runs from, checked before each; the
    /// sandbox lets it start nothing else but the shim (and `/usr/bin/env`, which the shim is started
    /// through). Read once, at the start.
    fn pin(&self) -> &Pin;

    /// Which of the backend's sandbox profiles its turns run under. Read once, at the start.
    fn profile(&self) -> Profile;

    /// What the run's record should say about it beyond its pin and profile, such as where a turn's
    /// result comes from: the first usage row and the plan hold it ([`HarnessBackend::harness_record`]).
    fn about(&self) -> Value {
        json!({})
    }

    /// The environment entries it may add beyond [`BASE_ENV`], by name, fixed for the run: read once, at
    /// the start. Each must be a plain name, and none may be one of [`BASE_ENV`] or a proxy setting, in
    /// any case ([`declared_name`]), or the backend doesn't start.
    fn env_names(&self) -> &[&str];

    /// An agent's turn configuration: (file name, bytes), each name one plain path component of printable
    /// ASCII, none twice in any case.
    /// The backend writes it read-only into [`Turn::config`] before the agent's first turn, and before
    /// every turn checks it is unchanged and still what this gives; any of these failing stops the run.
    fn config(&self, name: &str, socket: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String>;

    /// The turn's arguments and environment entries. Its standard output is the event stream, a line an
    /// event. An entry not named in [`Harness::env_names`] stops the run.
    fn launch(&self, turn: &Turn) -> Result<Launch, String>;

    /// Reads one line of the event stream, the turn's first if `first`, which must be [`Event::Init`].
    /// An error stops the run.
    fn event(&self, line: &str, first: bool, turn: &Turn) -> Result<Event, String>;
}

/// What the backend needs beside its harness, fixed before the run.
pub struct BackendConfig {
    pub caps: Caps,
    /// The backend's own directory, outside every store: agents' directories, turn configurations,
    /// transcripts and `usage.jsonl`. Must not exist yet.
    pub dir: PathBuf,
    /// The `bracken-experiment` binary: each turn's guard, and the shim the sandbox lets the agent start.
    pub shim: PathBuf,
    /// Each agent's turns in the run, for the turn message.
    pub turns: u32,
}

#[derive(Default)]
struct Totals {
    agents: BTreeMap<String, Tokens>,
    run: Tokens,
}

/// How a turn ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Result,
    /// A turn or agent cap: the process was ended.
    Cap(&'static str),
    /// The stream ended without a result.
    NoResult,
    /// The run must stop.
    Halt(String),
}

/// The driver's [`Backend`] for any [`Harness`].
pub struct HarnessBackend<H> {
    harness: H,
    /// What the harness declared, as the start checked it: the run uses these, never asking again.
    secret: String,
    pin: Pin,
    profile: Profile,
    env_names: Vec<String>,
    /// What the run's record says of the harness, as the start took it.
    record: Value,
    cfg: BackendConfig,
    proxy: Proxy,
    totals: Mutex<Totals>,
    halt: Mutex<Option<String>>,
    usage: Mutex<File>,
    started: Instant,
}

impl<H: Harness> HarnessBackend<H> {
    /// Checks what the harness declares, makes the backend's directory, checks the pin and starts the
    /// forwarding proxy.
    pub fn start(harness: H, cfg: BackendConfig) -> Result<HarnessBackend<H>, String> {
        let name = harness.name();
        let (secret, pin, profile) = (harness.secret().to_string(), harness.pin().clone(), harness.profile());
        let env_names: Vec<String> = harness.env_names().iter().map(|n| n.to_string()).collect();
        if secret.len() < MIN_SECRET {
            return Err(format!(
                "{name}'s secret is {} bytes, under {MIN_SECRET}: too short to check transcripts for",
                secret.len()
            ));
        }
        for n in &env_names {
            declared_name(name, n)?;
        }
        if cfg.dir.exists() {
            return Err(format!("{} already exists", cfg.dir.display()));
        }
        for sub in ["", "agents", "turn", "transcripts"] {
            make_dir(&cfg.dir.join(sub))?;
        }
        pin.check(name)?;
        let proxy = Proxy::start(harness.route(), cfg.caps.turn_time)?;
        let usage = private_file(&cfg.dir.join("usage.jsonl"))?;
        let record = json!({
            "harness": name,
            "pin": {"path": pin.path, "blake3": pin.blake3, "version": pin.version, "tree": pin.tree},
            "profile": profile.name(),
            "declared": env_names,
            "about": harness.about(),
        });
        let backend = HarnessBackend {
            harness,
            secret,
            pin,
            profile,
            env_names,
            record,
            cfg,
            proxy,
            totals: Mutex::new(Totals::default()),
            halt: Mutex::new(None),
            usage: Mutex::new(usage),
            started: Instant::now(),
        };
        backend.record(backend.record.clone());
        Ok(backend)
    }

    /// What the run's record says of the harness: its name, pin, profile, declared entries and
    /// [`Harness::about`]. The first usage row holds it, and a run's plan carries it.
    pub fn harness_record(&self) -> &Value {
        &self.record
    }

    pub fn harness(&self) -> &H {
        &self.harness
    }

    /// Everything the forwarding proxy was asked so far.
    pub fn proxy_attempts(&self) -> Vec<Attempt> {
        self.proxy.attempts()
    }

    pub fn proxy_port(&self) -> u16 {
        self.proxy.port()
    }

    /// An agent's counted tokens, and the run's.
    pub fn totals(&self, agent: &str) -> (Tokens, Tokens) {
        let t = self.totals.lock().unwrap_or_else(|e| e.into_inner());
        (t.agents.get(agent).copied().unwrap_or_default(), t.run)
    }

    fn stop(&self, why: &str) {
        self.halt.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| why.to_string());
    }

    fn record(&self, row: Value) {
        let failed = writeln!(self.usage.lock().unwrap_or_else(|e| e.into_inner()), "{row}").is_err();
        if failed {
            self.stop("writing usage.jsonl failed");
        }
    }

    /// Adds `t` to the agent's and the run's totals, and says whether a cap is reached.
    fn count(&self, agent: &str, t: &Tokens) -> Option<End> {
        let caps = &self.cfg.caps;
        let mut totals = self.totals.lock().unwrap_or_else(|e| e.into_inner());
        totals.run.add(t);
        let run = totals.run;
        let mine = totals.agents.entry(agent.to_string()).or_default();
        mine.add(t);
        if run.total() >= caps.run_tokens || run.output >= caps.run_output {
            return Some(End::Halt(RUN_TOKEN_CAP.into()));
        }
        (mine.total() >= caps.agent_tokens || mine.output >= caps.agent_output).then_some(End::Cap("agent tokens"))
    }

    /// Plays one turn and says how it ended.
    fn play(&self, name: &str, turn: u32, socket: &Path) -> Result<End, String> {
        let config = self.cfg.dir.join("turn").join(name);
        let h = &self.harness;
        self.pin.check(h.name()).and_then(|()| self.turn_config(&config, name, socket)).map_err(|e| self.halting(e))?;
        let message = crate::prompts::turn_message(turn, self.cfg.turns, name);
        let own = self.cfg.dir.join("agents").join(name);
        let (home, tmp) = (own.join("home"), own.join("tmp"));
        for d in [&own, &home, &tmp] {
            make_dir(d)?;
        }
        let (home, tmp, config) = (resolved(&home)?, resolved(&tmp)?, resolved(&config)?);
        let t = Turn {
            name,
            turn,
            message: &message,
            socket,
            own: &own,
            home: &home,
            tmp: &tmp,
            config: &config,
            proxy_port: self.proxy.port(),
            version: &self.pin.version,
        };
        let launch = h.launch(&t)?;
        let mut cmd = self.command(&t, launch)?;
        let path = self.cfg.dir.join("transcripts").join(format!("{name}-{turn}.jsonl"));
        let mut transcript = Transcript::create(&path, &self.secret)?;
        let group = Group::start(&self.cfg.shim, &mut cmd, h.name())?;
        let (lines, stderr) = (read_lines(group.stdout), read_stderr(group.stderr));
        let end = self.watch(&t, &lines, &mut transcript);
        drop(group.guard); // kills the group
        let err = stderr.recv_timeout(EXIT_GRACE).unwrap_or_default();
        let end = match transcript.finish(&err) {
            Err(e) => End::Halt(e),
            Ok(()) => end,
        };
        if let End::Halt(why) = &end {
            self.stop(why);
        }
        Ok(end)
    }

    fn halting(&self, why: String) -> String {
        self.stop(&why);
        why
    }

    /// Writes the agent's turn configuration into `dir` if it isn't there yet, and checks it is exactly
    /// what the harness gives.
    fn turn_config(&self, dir: &Path, name: &str, socket: &Path) -> Result<(), String> {
        let files = self.harness.config(name, socket)?;
        // Every name checked before anything is written: each is one plain component of `dir`, so no file
        // lands outside it (finding 85d103a6132a), and none comes twice in any case, since macOS's volumes
        // fold case; ASCII only, so no two spellings of one Unicode name pass either (finding 2c8cbade851b).
        for (k, (file, _)) in files.iter().enumerate() {
            if !plain_file_name(file) {
                let file = crate::args::short(&file.escape_debug().to_string());
                return Err(format!("the turn configuration's file name {file} isn't one plain name"));
            }
            if let Some((f, _)) = files[..k].iter().find(|(f, _)| f.eq_ignore_ascii_case(file)) {
                let same = if f == file { String::new() } else { format!(" (as {f})") };
                return Err(format!("the turn configuration names {file} twice{same}"));
            }
        }
        if !dir.exists() {
            make_dir(dir)?;
            for (file, bytes) in &files {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o400)
                    .open(dir.join(file))
                    .map_err(|e| format!("writing {file}: {e}"))?;
                f.write_all(bytes).map_err(|e| format!("writing {file}: {e}"))?;
            }
        }
        for (file, bytes) in &files {
            let now = std::fs::read(dir.join(file)).map_err(|e| format!("reading {file}: {e}"))?;
            if blake3::hash(&now) != blake3::hash(bytes) {
                return Err(format!("the turn configuration's {file} changed"));
            }
        }
        Ok(())
    }

    /// The turn's process: the pinned program under the harness's profile, with its arguments, an
    /// environment of [`BASE_ENV`] and the harness's declared entries only, and standard input null.
    fn command(&self, t: &Turn, launch: Launch) -> Result<Command, String> {
        let h = &self.harness;
        if let Some((k, _)) = launch.env.iter().find(|(k, _)| !self.env_names.contains(k)) {
            let why = format!("{} set {}, an environment entry it didn't declare", h.name(), crate::args::short(k));
            return Err(self.halting(why));
        }
        let pin = &self.pin;
        let sandbox = Sandbox::agent(
            self.profile,
            &AgentPaths {
                program: &pin.path,
                install: &pin.install,
                own: t.own,
                turn: t.config,
                shim: &self.cfg.shim,
                socket: t.socket,
                proxy_port: t.proxy_port,
            },
        )?;
        let mut cmd = sandbox.command(&pin.path);
        cmd.args(&launch.args)
            .env_clear()
            .env("HOME", t.home)
            .env("TMPDIR", t.tmp)
            .env("PATH", "/usr/bin:/bin")
            .env("HTTPS_PROXY", format!("http://127.0.0.1:{}", t.proxy_port))
            .envs(launch.env)
            .current_dir(t.home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }

    /// Reads the event stream until the turn ends.
    fn watch(&self, t: &Turn, lines: &Receiver<Result<String, String>>, out: &mut Transcript) -> End {
        let (name, turn) = (t.name, t.turn);
        let caps = &self.cfg.caps;
        let deadline = Instant::now() + caps.turn_time;
        let mut gated = false;
        // Per message id, what has been counted: each event about a message repeats its usage.
        let mut messages: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut tool_uses: BTreeSet<String> = BTreeSet::new();
        // Whether a result's figures were counted.
        let mut settled = false;
        // Once past the tool-call cap: until when, and after how many model calls, the turn may end itself.
        let mut grace: Option<(Instant, usize)> = None;
        let ended = loop {
            if let Some(why) = self.halted() {
                break End::Halt(why);
            }
            let now = Instant::now();
            if now >= self.started + caps.run_time {
                break End::Halt(RUN_TIME_CAP.into());
            }
            if now >= deadline {
                break End::Cap("turn time");
            }
            if grace.is_some_and(|(until, _)| now >= until) {
                break End::Cap("tool calls");
            }
            let line = match lines.recv_timeout(POLL.min(deadline - now)) {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => break End::Halt(format!("reading the event stream: {e}")),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break End::NoResult,
            };
            if let Err(e) = out.line(&line) {
                break End::Halt(e);
            }
            let event = match self.harness.event(&line, !gated, t) {
                Ok(e) => e,
                Err(e) => break End::Halt(e),
            };
            if !gated {
                let verdict = match event {
                    Event::Init(v) => v,
                    _ => Err("the first event isn't init".into()),
                };
                self.record(json!({"agent": name, "turn": turn, "gate": verdict.is_ok()}));
                if let Err(e) = verdict {
                    break End::Halt(format!("the gate refused {name}'s turn {turn}: {e}"));
                }
                gated = true;
                continue;
            }
            match event {
                Event::Message { id, model, usage, tools } => {
                    // A model call beyond the grace's: counted, then the process ends.
                    let beyond_grace = grace.is_some_and(|(_, at)| {
                        !messages.contains_key(&id) && messages.len() >= at + caps.cap_grace_calls
                    });
                    let seen = messages.entry(id.clone()).or_default();
                    let grown = usage.beyond(seen);
                    seen.add(&grown);
                    self.record(json!({
                        "agent": name, "turn": turn, "message": crate::args::short(&id), "model": model,
                        "input": usage.input, "cache_read": usage.cache_read, "cache_write": usage.cache_write,
                        "output_at_least": usage.output, "tools": tools.iter().map(|(_, n)| n).collect::<Vec<_>>(),
                    }));
                    tool_uses.extend(tools.into_iter().map(|(id, _)| id));
                    if let Some(end) = self.count(name, &grown) {
                        break end;
                    }
                    if usage.input_side() >= caps.context {
                        break End::Cap("context");
                    }
                    if beyond_grace {
                        break End::Cap("tool calls");
                    }
                    if grace.is_none() && tool_uses.len() > caps.calls_per_turn as usize {
                        // The driver refuses this call and every later one; the turn may still end itself.
                        self.record(json!({"agent": name, "turn": turn, "past_tool_call_cap": tool_uses.len()}));
                        grace = Some((now + caps.cap_grace, messages.len()));
                    }
                }
                Event::Result { usage, ended, cost_usd, duration_ms } => {
                    let streamed = messages.values().fold(Tokens::default(), |mut a, t| {
                        a.add(t);
                        a
                    });
                    settled = true;
                    let about = json!({"ended": ended, "cost_usd": cost_usd, "duration_ms": duration_ms});
                    break self.settle(name, turn, usage.as_ref(), &streamed, about);
                }
                Event::Init(_) | Event::Other => {}
            }
        };
        if !gated && !matches!(ended, End::Halt(_)) {
            return End::Halt(format!("{name}'s turn {turn} ended before its init event"));
        }
        if ended != End::Result && !settled {
            // No result counted, however the turn ended: its output counts at the larger of its cap
            // and what the stream already counted, and the row says what was counted.
            let streamed: u64 = messages.values().map(|t| t.output).fold(0, u64::saturating_add);
            let rest = Tokens { output: caps.turn_output.saturating_sub(streamed), ..Tokens::default() };
            let at = caps.turn_output.max(streamed);
            self.record(json!({"agent": name, "turn": turn, "ended": format!("{ended:?}"), "output_counted_at": at}));
            if let (Some(End::Halt(why)), false) = (self.count(name, &rest), matches!(ended, End::Halt(_))) {
                return End::Halt(why);
            }
        }
        ended
    }

    /// The result: counts the larger of it and the stream, and stops the run if the two disagree on
    /// input by more than 10%. With no result total of the harness's own, the stream's sum is the result,
    /// the row says so, and there's nothing to check. `about` holds the result's `ended`, `cost_usd` and
    /// `duration_ms`.
    fn settle(&self, name: &str, turn: u32, result: Option<&Tokens>, streamed: &Tokens, about: Value) -> End {
        let gap = result.is_some_and(|result| {
            let (a, b) = (result.input_side(), streamed.input_side());
            let (low, high) = (a.min(b), a.max(b));
            high > 0 && (high - low).saturating_mul(10) > high
        });
        let counted = result.map_or(*streamed, |r| r.max(streamed));
        let source = if result.is_some() { "the harness's" } else { "the stream's sum" };
        self.record(json!({
            "agent": name, "turn": turn, "ended": about["ended"], "input": counted.input,
            "cache_read": counted.cache_read, "cache_write": counted.cache_write, "output": counted.output,
            "cost_usd": about["cost_usd"], "duration_ms": about["duration_ms"], "result": source,
        }));
        // The stream's figures are already in the totals: add only the rest. A gap is counted, then stops the run.
        let capped = self.count(name, &counted.beyond(streamed));
        if gap {
            return End::Halt(format!("{name}'s turn {turn}: the stream and the result disagree on input by over 10%"));
        }
        match capped {
            Some(End::Halt(why)) => End::Halt(why),
            _ => End::Result,
        }
    }
}

impl<H: Harness> Backend for HarnessBackend<H> {
    fn turn(&self, _agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        if let Some(why) = self.halted() {
            return Err(why);
        }
        let (mine, run) = self.totals(name);
        let caps = &self.cfg.caps;
        if run.total() >= caps.run_tokens || run.output >= caps.run_output {
            return Err(self.halting(RUN_TOKEN_CAP.into()));
        }
        if self.started.elapsed() >= caps.run_time {
            return Err(self.halting(RUN_TIME_CAP.into()));
        }
        if mine.total() >= caps.agent_tokens || mine.output >= caps.agent_output {
            return Err(format!("{name} has spent its tokens"));
        }
        match self.play(name, turn, socket)? {
            End::Result => Ok(()),
            End::Cap(cap) => Err(format!("{name}'s turn {turn} ended at its {cap} cap")),
            End::NoResult => Err(format!("{name}'s turn {turn} ended without a result")),
            End::Halt(why) => Err(why),
        }
    }

    fn halted(&self) -> Option<String> {
        self.halt.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Checks a name a harness declares ([`Harness::env_names`]): a plain environment name
/// (`[A-Za-z_][A-Za-z0-9_]*`), not one of [`BASE_ENV`] in any case, and no proxy setting in any case:
/// none with `PROXY` in it (`https_proxy`, `NO_PROXY`, `ALL_PROXY`, `npm_config_proxy`...), since
/// programs read those beside or before `HTTPS_PROXY`, and only the backend's says where the agent
/// connects (finding 85d103a6132a). `harness` names it in the error.
pub fn declared_name(harness: &str, n: &str) -> Result<(), String> {
    let mut chars = n.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !plain {
        let n = crate::args::short(&n.escape_debug().to_string());
        return Err(format!("{harness} declares {n}, which isn't an environment variable name"));
    }
    let upper = n.to_ascii_uppercase();
    if BASE_ENV.contains(&upper.as_str()) {
        return Err(format!("{harness} declares {n}, which the backend sets"));
    }
    if upper.contains("PROXY") {
        return Err(format!(
            "{harness} declares {n}, a proxy setting: only the backend's HTTPS_PROXY says where the agent connects"
        ));
    }
    Ok(())
}

/// Whether `file` is one plain path component: printable ASCII without a space or `/`, and not empty, `.`
/// or `..`.
fn plain_file_name(file: &str) -> bool {
    !file.is_empty() && file != "." && file != ".." && file.bytes().all(|b| b.is_ascii_graphic() && b != b'/')
}

/// Follows the link at `rel`, under `root`, one component at a time as the kernel would, each link's own
/// target in turn, and refuses any step that leaves `root`: an absolute target anywhere on the way, a
/// `..` above the root, a component that doesn't exist, or over 40 links. Its final destination alone
/// isn't enough: a link out of the tree and back in leads through what nothing hashes (finding
/// 79e50adb4032), and a `..` after a link inside the tree can climb above the root though the target's
/// text doesn't.
fn beneath(root: &Path, rel: &Path) -> Result<(), String> {
    use std::ffi::OsString;
    use std::path::Component;
    enum Step {
        Up,
        Name(OsString),
    }
    // A target's steps, last first, to be popped.
    let steps = |to: &Path| -> Result<Vec<Step>, String> {
        let mut out = Vec::new();
        for c in to.components() {
            match c {
                Component::RootDir | Component::Prefix(_) => return Err("which leads to an absolute path".into()),
                Component::CurDir => {}
                Component::ParentDir => out.push(Step::Up),
                Component::Normal(n) => out.push(Step::Name(n.to_os_string())),
            }
        }
        out.reverse();
        Ok(out)
    };
    let outside = || "which isn't inside the tree".to_string();
    let link = |at: &Path| std::fs::read_link(root.join(at)).map_err(|_| outside());
    // Where the walk is, as components under `root`: it starts in the link's own directory, which the
    // tree's walk reached without following anything.
    let mut at: Vec<OsString> = rel.parent().into_iter().flatten().map(|c| c.to_os_string()).collect();
    let mut todo = steps(&link(rel)?)?;
    let mut links = 1;
    while let Some(step) = todo.pop() {
        match step {
            Step::Up => {
                at.pop().ok_or_else(outside)?;
            }
            Step::Name(n) => {
                at.push(n);
                let here: PathBuf = at.iter().collect();
                let meta = std::fs::symlink_metadata(root.join(&here)).map_err(|_| outside())?;
                if meta.file_type().is_symlink() {
                    links += 1;
                    if links > 40 {
                        return Err(outside());
                    }
                    at.pop();
                    todo.extend(steps(&link(&here)?)?);
                }
            }
        }
    }
    Ok(())
}

/// Makes `path` and its parents, private to this user.
pub fn make_dir(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("creating {}: {e}", path.display()))
}

fn private_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("creating {}: {e}", path.display()))
}

/// The harness's process group, led by a guard.
struct Group {
    guard: Guard,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
}

/// Dropping it kills the whole group.
struct Guard {
    leader: Child,
    agent: Child,
}

impl Group {
    /// Starts `bin guard` as a new group's leader, then `cmd`, the harness called `name`, in that group.
    fn start(bin: &Path, cmd: &mut Command, name: &str) -> Result<Group, String> {
        let leader = Command::new(bin)
            .arg("guard")
            .env_clear()
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting the guard: {e}"))?;
        let pgid = leader.id() as i32;
        let mut agent = match cmd.process_group(pgid).spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = kill_group(pgid);
                return Err(format!("starting {name}: {e}"));
            }
        };
        let (Some(stdout), Some(stderr)) = (agent.stdout.take(), agent.stderr.take()) else {
            let _ = kill_group(pgid);
            return Err(format!("{name} has no pipes"));
        };
        Ok(Group { guard: Guard { leader, agent }, stdout, stderr })
    }
}

fn kill_group(pgid: i32) -> Result<(), String> {
    let pid = rustix::process::Pid::from_raw(pgid).ok_or("no group")?;
    rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).map_err(|e| e.to_string())
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = kill_group(self.leader.id() as i32);
        let _ = self.agent.kill();
        let _ = self.agent.wait();
        let _ = self.leader.kill();
        let _ = self.leader.wait();
    }
}

/// `bracken-experiment guard`: waits for its standard input to close (the driver ended or died),
/// then kills its own process group, which holds the harness and everything it started.
pub fn guard() -> Result<(), String> {
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
    rustix::process::kill_process_group(rustix::process::getpgrp(), rustix::process::Signal::KILL)
        .map_err(|e| e.to_string())
}

/// The event stream's lines, as they arrive.
fn read_lines(stdout: std::process::ChildStdout) -> Receiver<Result<String, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut lines = Lines::new(BufReader::new(stdout), MAX_EVENT);
        let mut line = String::new();
        loop {
            let next = lines.next_line(&mut line);
            let more = matches!(next, Ok(true));
            let sent = match next {
                Ok(true) => tx.send(Ok(line.clone())),
                Ok(false) => Ok(()),
                Err(e) => tx.send(Err(e)),
            };
            if !more || sent.is_err() {
                return;
            }
        }
    });
    rx
}

/// At most 64 KiB of standard error, once it closes.
fn read_stderr(stderr: std::process::ChildStderr) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// A turn's transcript: written as the stream arrives, refusing any line holding the secret,
/// and checked again whole before it is kept.
struct Transcript<'a> {
    path: PathBuf,
    file: File,
    secret: &'a str,
}

impl<'a> Transcript<'a> {
    fn create(path: &Path, secret: &'a str) -> Result<Transcript<'a>, String> {
        Ok(Transcript { path: path.to_path_buf(), file: private_file(path)?, secret })
    }

    fn line(&mut self, line: &str) -> Result<(), String> {
        if line.contains(self.secret) {
            return Err("the event stream held the auth value".into());
        }
        writeln!(self.file, "{line}").map_err(|e| format!("writing the transcript: {e}"))
    }

    /// Keeps the transcript, with `stderr` beside it, only if neither holds the secret; else removes it.
    fn finish(mut self, stderr: &[u8]) -> Result<(), String> {
        let written = self.file.flush().and_then(|()| std::fs::read(&self.path));
        // An empty secret can't be looked for: counted as leaked, never a panic on windows(0).
        let leaked =
            |b: &[u8]| self.secret.is_empty() || b.windows(self.secret.len()).any(|w| w == self.secret.as_bytes());
        if !written.is_ok_and(|b| !leaked(&b) && !leaked(stderr)) {
            let _ = std::fs::remove_file(&self.path);
            return Err("a transcript held the auth value, or couldn't be read back; it was removed".into());
        }
        let err_path = self.path.with_extension("stderr");
        private_file(&err_path)?.write_all(stderr).map_err(|e| format!("writing {}: {e}", err_path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a harness could do if Pin's fields were its to set (finding ab0192deac8f): the check
    /// refuses either, though each pin's own file and hash still match.
    #[test]
    fn a_pin_whose_program_left_its_install_fails_its_check() {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).expect("a tree");
        std::fs::write(root.join("prog"), "the program").expect("a program");
        let mut tree = Pin::tree(&root.join("prog"), &root, "1").expect("a pin");
        tree.check("mock").expect("as pinned");
        tree.path = std::fs::canonicalize("/bin/sh").expect("/bin/sh");
        tree.blake3 = tree_hash(&tree.install).expect("the tree's hash, unchanged");
        assert_eq!(tree.check("mock"), Err("the pinned mock program isn't inside its install".into()));

        let mut binary = Pin::of(&root.join("prog"), "1").expect("a pin");
        binary.check("mock").expect("as pinned");
        binary.install = PathBuf::from("/");
        assert_eq!(binary.check("mock"), Err("the pinned mock program isn't inside its install".into()));
    }
}
