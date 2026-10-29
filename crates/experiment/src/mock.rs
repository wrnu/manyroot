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

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bracken_demo::rng::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::candidates::{candidates, correct_programs};
use crate::driver::Backend;
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
                let s = UnixStream::connect(socket).map_err(|e| format!("connecting: {e}"))?;
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
}

/// The next function to implement: low layers first, spread across agents.
fn next_spec(task: Task, memory: &Memory, agent: usize) -> Option<&'static Spec> {
    let todo: Vec<_> = task.specs().iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
    todo.get(agent % 3).or(todo.first()).copied()
}

fn first_line(v: &Value) -> String {
    v["output"].as_str().unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
}

impl Backend for MockBackend {
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
        let mut memory: Memory = serde_json::from_str(notes["notes"].as_str().unwrap_or_default()).unwrap_or_default();
        if has("replace") && !has("pr_open") {
            self.crdt_turn(c, agent, name, turn, &mut rng, &mut memory)?;
            let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
            c.tool("notes_write", json!({"text": text}))?;
            c.tool("sync", json!({}))?;
            return Ok(());
        }
        if has("pr_open") {
            self.git_turn(c, agent, &mut rng, &mut memory)?;
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

        // Implement one function not done yet.
        if let Some(spec) = next_spec(self.task, &memory, agent) {
            let program = if rng.chance(self.wrong_rate) {
                candidates(self.task, spec.name).wrong[0].to_string()
            } else {
                correct_programs(self.task, version)[spec.name].to_string()
            };
            let content = format!("def {} {} : {program}\n", spec.name, spec.arity);
            let artifact = first_line(&c.tool("artifact", json!({"function": spec.name, "content": content}))?);
            let to = spec_id(spec.name).ok_or_else(|| format!("{name}: no spec claim for {}", spec.name))?;
            c.tool(
                "relate",
                // A short id where the tools take one: the spec claim's first 8 digits.
                json!({"from": artifact, "kind": "implements", "to": if has("news") { &to[..8] } else { &to }, "note": format!("turn {turn}")}),
            )?;
            memory.implemented.push(spec.name.to_string());
            memory.mine.push(artifact);
        }

        // Check up to two implementations by others.
        let artifacts = c.tool("log", json!({"kind": "artifact", "current": true}))?;
        let others: Vec<String> = artifacts
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str())
            .filter(|id| !memory.mine.iter().any(|m| m == id) && !memory.checked.iter().any(|m| m == id))
            .map(str::to_string)
            .collect();
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
        let Some(spec) = todo.get(agent % 3).or(todo.first()).copied() else { return Ok(()) };
        let program = if rng.chance(self.wrong_rate) {
            candidates(self.task, spec.name).wrong[0].to_string()
        } else {
            correct_programs(self.task, version)[spec.name].to_string()
        };
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

    fn git_turn(&self, c: &mut Client, agent: usize, rng: &mut Rng, memory: &mut Memory) -> Result<(), String> {
        let version = self.task.version_of(&file_text(&c.tool("read", json!({"file": "TASKS.md"}))?));
        let program = |spec: &Spec, rng: &mut Rng| {
            if rng.chance(self.wrong_rate) {
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
                // Both sides only add definitions: keep both.
                let read = c.tool("read", json!({"file": file}))?;
                let kept: String = file_text(&read)
                    .lines()
                    .filter(|l| !(l.starts_with("<<<<<<<") || l.starts_with("=======") || l.starts_with(">>>>>>>")))
                    .map(|l| format!("{l}\n"))
                    .collect();
                let lines = read["lines"].as_u64().unwrap_or(0);
                c.tool("edit", json!({"file": file, "start": 1, "end": lines, "text": kept}))?;
            }
            c.tool("commit", json!({"message": format!("Merge main into {branch}")}))?;
        }

        // Implement one function on a branch of its own, and open a PR.
        if let Some(spec) = next_spec(self.task, memory, agent).filter(|_| c.calls + 6 <= budget) {
            let branch = format!("f-{}", spec.name);
            memory.implemented.push(spec.name.to_string());
            c.tool("switch", json!({"branch": branch, "create": true}))?;
            let file = format!("{}.stk", spec.module);
            let lines = c.tool("read", json!({"file": file}))?["lines"].as_u64().unwrap_or(0);
            let def = format!("def {} {} : {}", spec.name, spec.arity, program(spec, rng));
            c.tool("edit", json!({"file": file, "start": lines + 1, "end": lines, "text": def}))?;
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
