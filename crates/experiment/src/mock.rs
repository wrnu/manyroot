//! The mock backend: plays an agent's turn with scripted moves, through the
//! same MCP tool calls a model would make (design section 3, Turns), so a
//! whole run can be checked without a model.
//!
//! Each turn the mock implements one function it hasn't yet (usually with a
//! correct program, sometimes a wrong one), checks up to two implementations
//! by others and records the result (Bracken evidence, Git reviews), and keeps
//! what it has done in its private notes. In Git mode it works on a branch per
//! function, opens a PR, merges what is approved and passes CI, and brings its
//! own PRs up to date when they conflict with `main`. In CRDT mode it writes the
//! definition into its module file, checks it, and adds a line to the shared notes.
//!
//! Phase 2's mock does more ([`Moves`]), in every mode. It writes the open
//! contracts' members, and after each wave it rewrites what it wrote that the
//! released specs change: the targets, `clamp3`'s callers and the contracts'
//! members. Some agents leave an interacting pair unsettled, writing one change's
//! obvious fix. Now and then an agent breaks a function it wrote and mends it
//! the turn after. In Bracken mode a rewrite retracts the agent's earlier
//! artifact; in Git mode it goes on a branch from `main` with a PR, or onto the
//! function's own branch while that PR is open. Phase 1's moves are as they ran.
//!
//! In Weave mode, a project run, the mock reads the task, brings its tree up to the line's head, writes a
//! module of its own into the package, runs the public tests, proposes its tree, and integrates its own
//! proposal. In lockstep no two integrations meet, so the line never forks. Otherwise two agents' turns can
//! each integrate onto the same head and fork it: the next turn settles the fork by integrating one head's
//! tree with every head as a parent (losing the other's changes, as a mock may), and an integration the
//! line refuses leaves its proposal open.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bracken_demo::rng::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::candidates::phase2::UNSETTLED;
use crate::candidates::{candidates, correct_programs};
use crate::driver::{Backend, socket_error};
use crate::task::{Spec, Task, Version, change_tag};

pub struct MockBackend {
    /// The task the run gives its agents.
    pub task: Task,
    pub seed: u64,
    /// Chance that an implementation is a known-wrong program.
    pub wrong_rate: f64,
    /// Talk through `bracken-experiment shim` at this path, as Claude Code would, instead of the socket directly.
    pub shim: Option<PathBuf>,
    /// Append made-up token usage here, in the rows Claude Code's backend writes: one message per tool call,
    /// then the turn's settled total.
    pub usage: Option<PathBuf>,
    /// Phase 2's moves beyond Phase 1's.
    pub moves: Moves,
    /// Play agent-turns one at a time ([`Backend::lockstep`]), so a run is the same every time.
    pub lockstep: bool,
}

impl Default for MockBackend {
    fn default() -> MockBackend {
        MockBackend {
            task: Task::Phase1,
            seed: 0,
            wrong_rate: 0.0,
            shim: None,
            usage: None,
            moves: Moves::RUN,
            lockstep: false,
        }
    }
}

/// Phase 2's scripted moves beyond Phase 1's (Phase 1's mock ignores them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moves {
    /// Chance per turn that an agent breaks one of its correct functions, to mend it the turn after.
    pub break_rate: f64,
    /// Every n-th agent (agents n - 1, 2n - 1, ...) leaves the interacting pairs unsettled; 0 for none.
    pub unsettled_every: usize,
    /// The most of its own functions an agent rewrites in a turn.
    pub rewrites_per_turn: usize,
}

impl Moves {
    /// A full mock run's.
    pub const RUN: Moves = Moves { break_rate: 0.1, unsettled_every: 5, rewrites_per_turn: 3 };
    /// Everyone settles and nothing breaks: every function follows the waves.
    pub const PLAIN: Moves = Moves { break_rate: 0.0, unsettled_every: 0, rewrites_per_turn: 3 };

    fn unsettled(&self, agent: usize) -> bool {
        self.unsettled_every > 0 && agent % self.unsettled_every == self.unsettled_every - 1
    }
}

/// A minimal MCP client over newline-delimited JSON-RPC.
pub struct Client {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next: u64,
    /// Tool calls made so far.
    pub calls: u32,
    /// Each call's tool and the size of its arguments.
    pub asked: Vec<(String, usize)>,
    _child: Option<std::process::Child>,
}

impl Client {
    pub fn connect(socket: &Path, shim: Option<&Path>) -> Result<Client, String> {
        let (reader, writer, child): (Box<dyn Read + Send>, Box<dyn Write + Send>, _) = match shim {
            None => {
                let s = UnixStream::connect(socket).map_err(|e| socket_error("connecting to", socket, &e))?;
                (Box::new(s.try_clone().map_err(|e| e.to_string())?), Box::new(s), None)
            }
            Some(bin) => {
                let mut child = Command::new(bin)
                    .arg("shim")
                    .arg("--socket")
                    .arg(socket)
                    .env_clear()
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("starting the shim: {e}"))?;
                let r = child.stdout.take().ok_or("no shim stdout")?;
                let w = child.stdin.take().ok_or("no shim stdin")?;
                (Box::new(r), Box::new(w), Some(child))
            }
        };
        let mut c =
            Client { reader: BufReader::new(reader), writer, next: 1, calls: 0, asked: Vec::new(), _child: child };
        c.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "mock"}}),
        )?;
        c.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(c)
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        writeln!(self.writer, "{msg}").and_then(|()| self.writer.flush()).map_err(|e| e.to_string())
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let reply: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply: {e}"))?;
        if reply["id"] != id {
            return Err("reply out of order".into());
        }
        match reply.get("error") {
            Some(e) => Err(e.to_string()),
            None => Ok(reply["result"].clone()),
        }
    }

    /// Calls a tool; its result text as JSON, or the tool's error.
    pub fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        self.calls += 1;
        self.asked.push((name.to_string(), args.to_string().len()));
        let r = self.request("tools/call", json!({"name": name, "arguments": args}))?;
        let text = r["content"][0]["text"].as_str().unwrap_or_default().to_string();
        if r["isError"].as_bool().unwrap_or(true) {
            return Err(text);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

/// What a mock agent remembers between turns, in its notes.
#[derive(Default, Serialize, Deserialize)]
struct Memory {
    implemented: Vec<String>,
    mine: Vec<String>,
    checked: Vec<String>,
    /// Git mode: my PRs, by number, with their branches.
    prs: Vec<(u64, String)>,
    /// Phase 2: each function this agent wrote, with what it last wrote (absent in Phase 1's notes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    written: Vec<Written>,
}

/// A function the agent wrote, the program it last wrote, and, in Bracken mode, that artifact.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Written {
    function: String,
    program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact: Option<String>,
}

/// The next function to implement: low layers first, spread across agents.
fn next_spec(task: Task, memory: &Memory, agent: usize) -> Option<&'static Spec> {
    let todo: Vec<_> = task.specs().iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
    if !task.waves().is_empty() {
        // Phase 2's 90 functions: agent k takes the lowest of every fifth from k, so two agents share each
        // share, then anyone's.
        let share = |s: &&&Spec| task.specs().iter().position(|t| t.name == s.name).is_some_and(|i| i % 5 == agent % 5);
        return todo.iter().find(share).or(todo.first()).copied();
    }
    todo.get(agent % 3).or(todo.first()).copied()
}

fn first_line(v: &Value) -> String {
    v["output"].as_str().unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
}

impl MockBackend {
    /// What `agent` means `function` to be under `version`: the correct program, or, for an agent that
    /// leaves the interacting pairs unsettled, one change's obvious fix once both of a pair apply.
    fn intended(&self, agent: usize, function: &str, version: Version) -> String {
        if self.moves.unsettled(agent) {
            let fix = UNSETTLED.iter().filter(|u| u.pair.iter().all(|c| version.has(*c)));
            if let Some((_, p)) = fix.flat_map(|u| u.programs).find(|(f, _)| *f == function) {
                return p.to_string();
            }
        }
        correct_programs(self.task, version)[function].to_string()
    }

    /// A new function's program: usually what the agent intends, sometimes a known-wrong one.
    fn first_program(&self, agent: usize, spec: &Spec, version: Version, rng: &mut Rng) -> String {
        if rng.chance(self.wrong_rate) {
            candidates(self.task, spec.name).wrong[0].to_string()
        } else {
            self.intended(agent, spec.name, version)
        }
    }

    /// Phase 2: this turn's rewrites of the agent's own functions, lowest layer first. First those whose
    /// last program isn't what the agent now intends (a wave changed them, or it was wrong); with none,
    /// now and then a break of a right one.
    fn rewrites(&self, agent: usize, turn: u32, memory: &Memory, version: Version) -> Vec<(&'static Spec, String)> {
        let mut out: Vec<(&'static Spec, String)> = Vec::new();
        for spec in self.task.specs() {
            let Some(w) = memory.written.iter().find(|w| w.function == spec.name) else { continue };
            let want = self.intended(agent, spec.name, version);
            if w.program != want && out.len() < self.moves.rewrites_per_turn {
                out.push((spec, want));
            }
        }
        // Its own stream, so a break never shifts the other moves' draws.
        let mut rng = Rng::derive(self.seed ^ 0x6272_6561_6b73, (agent as u64) << 16 | u64::from(turn));
        if out.is_empty() && !memory.written.is_empty() && rng.chance(self.moves.break_rate) {
            let w = &memory.written[rng.below(memory.written.len())];
            let spec = self.task.spec_named(&w.function).expect("a function of the task");
            out.push((spec, candidates(self.task, spec.name).wrong[0].to_string()));
        }
        out
    }
}

/// Records that the agent wrote `program` for `function`, with its artifact in Bracken mode.
fn wrote(memory: &mut Memory, function: &str, program: &str, artifact: Option<String>) {
    match memory.written.iter_mut().find(|w| w.function == function) {
        Some(w) => (w.program, w.artifact) = (program.to_string(), artifact),
        None => memory.written.push(Written { function: function.into(), program: program.into(), artifact }),
    }
}

impl Backend for MockBackend {
    fn lockstep(&self) -> bool {
        self.lockstep
    }

    fn turn(&self, agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        let mut c = Client::connect(socket, self.shim.as_deref())?;
        let played = self.play(&mut c, agent, name, turn);
        match &self.usage {
            Some(path) => usage(path, name, turn, &c.asked).and(played),
            None => played,
        }
    }
}

/// Appends a turn's made-up usage: each call a message whose context grows with the turn, written as
/// step 6's backend writes it, a row per content block (a text block, then the call), each with the
/// message's usage.
fn usage(path: &Path, name: &str, turn: u32, asked: &[(String, usize)]) -> Result<(), String> {
    let mut rows = String::new();
    let (mut input, mut output) = (0, 0);
    for (k, (tool, bytes)) in asked.iter().enumerate() {
        let (i, o) = (2000 + 300 * k, 60 + bytes / 4);
        for tools in [vec![], vec![format!("mcp__experiment__{tool}")]] {
            let row = json!({"agent": name, "turn": turn, "message": format!("m{k}"), "model": "mock", "input": i,
                "cache_read": 8000, "cache_write": 400, "output_at_least": o, "tools": tools});
            rows.push_str(&format!("{row}\n"));
        }
        (input, output) = (input + i, output + o);
    }
    let n = asked.len();
    rows.push_str(&format!(
        "{}\n",
        json!({"agent": name, "turn": turn, "ended": "success", "input": input, "cache_read": 8000 * n,
            "cache_write": 400 * n, "output": output})
    ));
    // One append per turn, so concurrent agents' rows don't interleave.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("opening {}: {e}", path.display()))?;
    file.write_all(rows.as_bytes()).map_err(|e| format!("writing {}: {e}", path.display()))
}

impl MockBackend {
    fn play(&self, c: &mut Client, agent: usize, name: &str, turn: u32) -> Result<(), String> {
        let mut rng = Rng::derive(self.seed, (agent as u64) << 16 | u64::from(turn));
        let tools = c.request("tools/list", json!({}))?;
        let has = |tool: &str| tools["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == tool));
        let notes = c.tool("notes_read", json!({}))?;
        if has("propose") {
            return weave_turn(c, agent, turn);
        }
        let mut memory: Memory = serde_json::from_str(notes["notes"].as_str().unwrap_or_default()).unwrap_or_default();
        if has("replace") && !has("pr_open") {
            self.crdt_turn(c, agent, name, turn, &mut rng, &mut memory)?;
            let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
            c.tool("notes_write", json!({"text": text}))?;
            c.tool("sync", json!({}))?;
            return Ok(());
        }
        if has("pr_open") {
            self.git_turn(c, agent, turn, &mut rng, &mut memory)?;
            let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
            c.tool("notes_write", json!({"text": text}))?;
            c.tool("sync", json!({}))?;
            return Ok(());
        }
        if !has("log") {
            return Err(format!("{name}: no log tool"));
        }

        // Bracken's own views, where the mode has them (task `541b6f3db130`).
        if has("news") {
            c.tool("news", json!({}))?;
            c.tool("status", json!({}))?;
        }
        let changes = c.tool("log", json!({"tag": "change", "current": true}))?;
        let seen: Vec<&str> = changes
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|e| e["body"]["tags"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        let version = self
            .task
            .changes()
            .iter()
            .filter(|ch| seen.contains(&change_tag(ch.change)))
            .fold(Version::ORIGINAL, |v, ch| v.with(ch.change));
        let specs = c.tool("log", json!({"tag": "spec", "current": true}))?;
        let spec_id = |function: &str| {
            specs
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|e| e["body"]["tags"].as_array().is_some_and(|t| t.iter().any(|t| t == function)))
                .and_then(|e| e["id"].as_str())
                .map(str::to_string)
        };

        let implement = |c: &mut Client, spec: &Spec, program: &str| -> Result<String, String> {
            let content = format!("def {} {} : {program}\n", spec.name, spec.arity);
            let artifact = first_line(&c.tool("artifact", json!({"function": spec.name, "content": content}))?);
            let to = spec_id(spec.name).ok_or_else(|| format!("{name}: no spec claim for {}", spec.name))?;
            c.tool(
                "relate",
                // A short id where the tools take one: the spec claim's first 8 digits.
                json!({"from": artifact, "kind": "implements", "to": if has("news") { &to[..8] } else { &to }, "note": format!("turn {turn}")}),
            )?;
            Ok(artifact)
        };
        let phase2 = !self.task.waves().is_empty();
        if phase2 {
            // Rewrite what the specs or a mistake changed, retracting the artifact it replaces.
            for (spec, program) in self.rewrites(agent, turn, &memory, version) {
                let old = memory.written.iter().find(|w| w.function == spec.name).and_then(|w| w.artifact.clone());
                let artifact = implement(c, spec, &program)?;
                if let Some(old) = old {
                    // Already gone if another call retracted it; that refusal is expected.
                    let _ = c.tool("retract", json!({"target": old, "reason": "rewritten"}));
                }
                wrote(&mut memory, spec.name, &program, Some(artifact.clone()));
                memory.mine.push(artifact);
            }
        }

        // Implement one function not done yet.
        if let Some(spec) = next_spec(self.task, &memory, agent) {
            let program = if phase2 {
                self.first_program(agent, spec, version, &mut rng)
            } else if rng.chance(self.wrong_rate) {
                candidates(self.task, spec.name).wrong[0].to_string()
            } else {
                correct_programs(self.task, version)[spec.name].to_string()
            };
            let artifact = implement(c, spec, &program)?;
            if phase2 {
                wrote(&mut memory, spec.name, &program, Some(artifact.clone()));
            }
            memory.implemented.push(spec.name.to_string());
            memory.mine.push(artifact);
        }

        // Check up to two implementations by others.
        let artifacts = c.tool("log", json!({"kind": "artifact", "current": true}))?;
        let mut others: Vec<String> = artifacts
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str())
            .filter(|id| !memory.mine.iter().any(|m| m == id) && !memory.checked.iter().any(|m| m == id))
            .map(str::to_string)
            .collect();
        if phase2 {
            // Rewrites keep coming: check the newest first, so the backlog doesn't hold up the latest.
            others.reverse();
        }
        for id in others.iter().take(2) {
            let result = c.tool("check", json!({"id": id}))?;
            let verdict = if result["ok"].as_bool().unwrap_or(false) { "pass" } else { "fail" };
            let details = result["details"].as_str().unwrap_or_default().to_string();
            c.tool(
                "evidence",
                json!({"target": id, "verdict": verdict, "method": "public check", "details": details}),
            )?;
            memory.checked.push(id.clone());
        }

        let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
        c.tool("notes_write", json!({"text": text}))?;
        c.tool("sync", json!({}))?;
        Ok(())
    }
}

/// One Weave-mode turn of a project run (see the module's doc).
fn weave_turn(c: &mut Client, agent: usize, turn: u32) -> Result<(), String> {
    let task = c.tool("task", json!({}))?;
    if task["spec"].as_str().is_none_or(str::is_empty) {
        return Err("the task tool showed no spec".into());
    }
    let head = c.tool("head", json!({}))?;
    let heads: Vec<String> = head["heads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["decision"].as_str().map(str::to_string))
        .collect();
    if heads.len() > 1 {
        // Each agent writes its own files, so the heads' merge never conflicts.
        let merged = c.tool("merge", json!({}))?;
        if merged["conflicts"].as_array().is_some_and(|c| !c.is_empty()) {
            return Err(format!("the heads' merge conflicts: {merged}"));
        }
        c.tool("integrate", json!({"note": format!("agent {agent} settles the fork")}))?;
    } else if heads.first().map(String::as_str) != head["base"].as_str() {
        c.tool("update", json!({}))?;
    }
    let files = c.tool("list_files", json!({}))?;
    let init = files["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
        .find(|p| p.ends_with("/__init__.py") && p.matches('/').count() == 1)
        .ok_or("no package in the tree")?
        .to_string();
    c.tool("read_file", json!({"path": init, "limit": 20}))?;
    let package = init.trim_end_matches("/__init__.py");
    let module = format!("{package}/by_agent_{agent}_turn_{turn}.py");
    c.tool("write_file", json!({"path": module, "text": format!("VALUE = {}\n", turn * 10 + agent as u32)}))?;
    c.tool("run_tests", json!({}))?;
    let proposed = c.tool("propose", json!({"summary": format!("agent {agent}'s module of turn {turn}")}))?;
    let proposal = proposed["proposal"].as_str().ok_or("propose gave no proposal")?.to_string();
    // Refused if the line forked or moved on under it: the proposal stays open, for later.
    if c.tool("integrate", json!({"proposals": [proposal], "note": format!("agent {agent}'s turn {turn}")})).is_ok() {
        c.tool("update", json!({}))?;
    }
    c.tool("notes_write", json!({"text": format!("turn {turn}: wrote {module}")}))?;
    c.tool("sync", json!({}))?;
    Ok(())
}

/// Git mode, Phase 2: writes `program` as `spec`'s definition on the current branch, in place of the one
/// there or after the last line.
fn git_put(c: &mut Client, spec: &Spec, program: &str) -> Result<(), String> {
    let file = format!("{}.stk", spec.module);
    let read = c.tool("read", json!({"file": file}))?;
    let line = format!("def {} {} : {program}", spec.name, spec.arity);
    let prefix = format!("def {} ", spec.name);
    let at = file_text(&read).lines().position(|l| l.starts_with(&prefix)).map(|k| k as u64 + 1);
    let (start, end) = match at {
        Some(k) => (k, k),
        None => {
            let lines = read["lines"].as_u64().unwrap_or(0);
            (lines + 1, lines)
        }
    };
    c.tool("edit", json!({"file": file, "start": start, "end": end, "text": line}))?;
    Ok(())
}

/// A conflicted file resolved by keeping both sides, except that a function both sides define keeps this
/// branch's definition (the side before `=======`).
fn keep_ours(text: &str) -> String {
    let name = |l: &str| l.strip_prefix("def ").and_then(|r| r.split_whitespace().next()).map(str::to_string);
    let mut out = String::new();
    let (mut ours, mut theirs, mut side) = (Vec::new(), Vec::new(), 0);
    for l in text.lines() {
        if l.starts_with("<<<<<<<") {
            side = 1;
        } else if l.starts_with("=======") && side == 1 {
            side = 2;
        } else if l.starts_with(">>>>>>>") && side == 2 {
            let mine: Vec<Option<String>> = ours.iter().map(|l: &String| name(l)).collect();
            for l in ours
                .drain(..)
                .chain(theirs.drain(..).filter(|l: &String| name(l).is_none() || !mine.contains(&name(l))))
            {
                out.push_str(&format!("{l}\n"));
            }
            side = 0;
        } else {
            match side {
                1 => ours.push(l.to_string()),
                2 => theirs.push(l.to_string()),
                _ => out.push_str(&format!("{l}\n")),
            }
        }
    }
    out
}

/// A file's text from a `read` result, without its line numbers.
fn file_text(read: &Value) -> String {
    read["text"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(|l| format!("{}\n", l.split_once("| ").map_or("", |(_, t)| t)))
        .collect()
}

impl MockBackend {
    /// One CRDT-mode turn: write one function's definition into its module file (replacing an
    /// earlier definition of it), check it, and say so in the shared notes.
    fn crdt_turn(
        &self,
        c: &mut Client,
        agent: usize,
        name: &str,
        turn: u32,
        rng: &mut Rng,
        memory: &mut Memory,
    ) -> Result<(), String> {
        use crate::crdt_mode::{NOTES, TASKS};
        let version = self.task.version_of(&file_text(&c.tool("read", json!({"file": TASKS}))?));
        let todo: Vec<_> =
            self.task.specs().iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
        let phase2 = !self.task.waves().is_empty();
        if phase2 {
            // The document may no longer hold what this agent wrote: a concurrent edit of the same line, a
            // broken line someone deleted, another agent's version. Then it counts as unwritten.
            let mut texts: Vec<(&str, String)> = Vec::new();
            for w in memory.written.iter_mut() {
                let spec = self.task.spec_named(&w.function).ok_or("a function of the task")?;
                let index = self.task.modules().iter().position(|m| *m == spec.module).ok_or("a spec in no module")?;
                let file = self.task.module_files()[index];
                if !texts.iter().any(|(f, _)| *f == file) {
                    texts.push((file, file_text(&c.tool("read", json!({"file": file}))?)));
                }
                let text = &texts.iter().find(|(f, _)| *f == file).expect("read above").1;
                let line = format!("def {} {} : {}", spec.name, spec.arity, w.program);
                if !text.lines().any(|l| l == line) {
                    w.program.clear();
                }
            }
            for (spec, program) in self.rewrites(agent, turn, memory, version) {
                self.crdt_write(c, spec, &program)?;
                wrote(memory, spec.name, &program, None);
            }
        }
        let spec =
            if phase2 { next_spec(self.task, memory, agent) } else { todo.get(agent % 3).or(todo.first()).copied() };
        let Some(spec) = spec else { return Ok(()) };
        let program = if phase2 {
            self.first_program(agent, spec, version, rng)
        } else if rng.chance(self.wrong_rate) {
            candidates(self.task, spec.name).wrong[0].to_string()
        } else {
            correct_programs(self.task, version)[spec.name].to_string()
        };
        self.crdt_write(c, spec, &program)?;
        if phase2 {
            wrote(memory, spec.name, &program, None);
        }
        // Another module can still be broken by someone else's merge: then the check can't run,
        // and the mock counts that as failing, as an agent would.
        let ok = c.tool("check", json!({"function": spec.name})).is_ok_and(|v| v["ok"].as_bool().unwrap_or(false));
        let notes = file_text(&c.tool("read", json!({"file": NOTES}))?);
        let n = notes.lines().count() as i64;
        let note =
            format!("{name}, turn {turn}: wrote {} (public check {})", spec.name, if ok { "passes" } else { "fails" });
        c.tool("edit", json!({"file": NOTES, "start": n + 1, "end": n, "text": note}))?;
        memory.implemented.push(spec.name.to_string());
        Ok(())
    }

    /// CRDT mode: writes `program` as `spec`'s one definition in its module file.
    fn crdt_write(&self, c: &mut Client, spec: &Spec, program: &str) -> Result<(), String> {
        let line = format!("def {} {} : {program}", spec.name, spec.arity);
        let index = self.task.modules().iter().position(|m| *m == spec.module).ok_or("a spec in no module")?;
        let file = self.task.module_files()[index];
        // Concurrent edits of one line merge character by character, and can leave a line that
        // doesn't parse, which breaks its whole module for everyone. Like an agent reading the
        // check's error, delete such lines first, from the bottom so line numbers above stay put.
        let text = file_text(&c.tool("read", json!({"file": file}))?);
        let broken: Vec<i64> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| crate::lang::parse_module(spec.module, l).is_err())
            .map(|(i, _)| i as i64 + 1)
            .collect();
        for &k in broken.iter().rev() {
            c.tool("edit", json!({"file": file, "start": k, "end": k, "text": ""}))?;
        }
        let text = if broken.is_empty() { text } else { file_text(&c.tool("read", json!({"file": file}))?) };
        let prefix = format!("def {} ", spec.name);
        // Two agents appending the same function at once both keep their line when the CRDT
        // merges them, so there may be several. Delete all but the first, from the bottom so the
        // line numbers above stay put, then rewrite the first.
        let at: Vec<i64> =
            text.lines().enumerate().filter(|(_, l)| l.starts_with(&prefix)).map(|(i, _)| i as i64 + 1).collect();
        match at.split_first() {
            Some((&first, extra)) => {
                for &k in extra.iter().rev() {
                    c.tool("edit", json!({"file": file, "start": k, "end": k, "text": ""}))?;
                }
                c.tool("edit", json!({"file": file, "start": first, "end": first, "text": line}))?
            }
            None => {
                let n = text.lines().count() as i64;
                c.tool("edit", json!({"file": file, "start": n + 1, "end": n, "text": line}))?
            }
        };
        Ok(())
    }

    fn git_turn(
        &self,
        c: &mut Client,
        agent: usize,
        turn: u32,
        rng: &mut Rng,
        memory: &mut Memory,
    ) -> Result<(), String> {
        let version = self.task.version_of(&file_text(&c.tool("read", json!({"file": "TASKS.md"}))?));
        let phase2 = !self.task.waves().is_empty();
        let program = |spec: &Spec, rng: &mut Rng| {
            if phase2 {
                self.first_program(agent, spec, version, rng)
            } else if rng.chance(self.wrong_rate) {
                candidates(self.task, spec.name).wrong[0].to_string()
            } else {
                correct_programs(self.task, version)[spec.name].to_string()
            }
        };

        // Keep within the turn's calls, with two left for the notes and the last sync.
        let budget = crate::driver::CALLS_PER_TURN - 2;
        let list = c.tool("pr_list", json!({}))?;
        let open: Vec<Value> =
            list.as_array().into_iter().flatten().filter(|p| p["state"] == "open").cloned().collect();

        // My open PRs failing CI: bring one that conflicts up to date with main.
        for (n, branch) in memory.prs.clone() {
            let failing = open.iter().any(|p| p["number"] == n && p["ci"] == false);
            if !failing || c.calls + 8 > budget {
                continue;
            }
            let view = c.tool("pr_view", json!({"number": n}))?;
            let details = view["ci"]["details"].as_array().cloned().unwrap_or_default();
            if !details.iter().any(|d| d.as_str().is_some_and(|d| d.contains("conflicts"))) {
                continue;
            }
            c.tool("switch", json!({"branch": branch}))?;
            let merged = c.tool("merge", json!({}))?;
            if merged["merged"] == true {
                continue;
            }
            for file in merged["conflicts"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                // Both sides only add definitions: keep both. In Phase 2 both may rewrite one: keep this
                // branch's.
                let read = c.tool("read", json!({"file": file}))?;
                let kept: String = if phase2 {
                    keep_ours(&file_text(&read))
                } else {
                    file_text(&read)
                        .lines()
                        .filter(|l| !(l.starts_with("<<<<<<<") || l.starts_with("=======") || l.starts_with(">>>>>>>")))
                        .map(|l| format!("{l}\n"))
                        .collect()
                };
                let lines = read["lines"].as_u64().unwrap_or(0);
                c.tool("edit", json!({"file": file, "start": 1, "end": lines, "text": kept}))?;
            }
            c.tool("commit", json!({"message": format!("Merge main into {branch}")}))?;
        }

        // Phase 2: rewrite what the specs or a mistake changed, on the function's own branch while its PR is
        // open, else on one branch from main for the turn, with a PR.
        if phase2 {
            let mut from_main = Vec::new();
            let mut pushed = false;
            for (spec, program) in self.rewrites(agent, turn, memory, version) {
                let branch = format!("f-{}", spec.name);
                let open_pr = memory.prs.iter().any(|(n, b)| *b == branch && open.iter().any(|p| p["number"] == *n));
                if !open_pr {
                    from_main.push((spec, program));
                } else if c.calls + 5 <= budget {
                    c.tool("switch", json!({"branch": branch}))?;
                    git_put(c, spec, &program)?;
                    c.tool("commit", json!({"message": format!("Rewrite {}", spec.name)}))?;
                    wrote(memory, spec.name, &program, None);
                    pushed = true;
                }
            }
            if !from_main.is_empty() && c.calls + 4 + 2 * from_main.len() as u32 <= budget {
                let branch = format!("u{turn}-{}", from_main[0].0.name);
                c.tool("switch", json!({"branch": branch, "create": true}))?;
                for (spec, program) in &from_main {
                    git_put(c, spec, program)?;
                    wrote(memory, spec.name, program, None);
                }
                c.tool("commit", json!({"message": format!("Rewrite {}", from_main[0].0.name)}))?;
                c.tool("sync", json!({}))?;
                if let Ok(opened) = c.tool("pr_open", json!({"branch": branch, "title": "Rewrite"})) {
                    memory.prs.push((opened["number"].as_u64().unwrap_or(0), branch));
                }
            } else if pushed {
                c.tool("sync", json!({}))?;
            }
        }

        // Implement one function on a branch of its own, and open a PR.
        if let Some(spec) = next_spec(self.task, memory, agent).filter(|_| c.calls + 6 <= budget) {
            let branch = format!("f-{}", spec.name);
            memory.implemented.push(spec.name.to_string());
            c.tool("switch", json!({"branch": branch, "create": true}))?;
            let file = format!("{}.stk", spec.module);
            let lines = c.tool("read", json!({"file": file}))?["lines"].as_u64().unwrap_or(0);
            let written = program(spec, rng);
            let def = format!("def {} {} : {written}", spec.name, spec.arity);
            c.tool("edit", json!({"file": file, "start": lines + 1, "end": lines, "text": def}))?;
            if phase2 {
                wrote(memory, spec.name, &written, None);
            }
            c.tool("commit", json!({"message": format!("Implement {}", spec.name)}))?;
            c.tool("sync", json!({}))?;
            // At the open-PR cap the hub refuses; the branch waits for a later turn.
            if let Ok(opened) =
                c.tool("pr_open", json!({"branch": branch, "title": format!("Implement {}", spec.name)}))
            {
                memory.prs.push((opened["number"].as_u64().unwrap_or(0), branch));
            }
        }

        // Review up to two others' PRs at their current heads (as of the list); merge what is ready.
        let mut reviewed = 0;
        for p in &open {
            if c.calls + 2 > budget {
                break;
            }
            let n = p["number"].as_u64().unwrap_or(0);
            let key = format!("{n}@{}", p["head"].as_str().unwrap_or_default());
            let mine = memory.prs.iter().any(|(m, _)| *m == n);
            let mut approved = p["approved"] == true;
            if !mine && reviewed < 2 && !memory.checked.contains(&key) {
                let verdict = if p["ci"] == true { "approve" } else { "request_changes" };
                // Another agent may have merged it since the list; that refusal is expected.
                let done = c.tool("pr_review", json!({"number": n, "verdict": verdict})).is_ok();
                memory.checked.push(key);
                reviewed += 1;
                approved |= done && verdict == "approve";
            }
            if p["ci"] == true && approved && p["blocked_by"].as_array().is_some_and(Vec::is_empty) {
                // Another agent may merge it first; that refusal is expected.
                let _ = c.tool("pr_merge", json!({"number": n}));
            }
        }
        Ok(())
    }
}
