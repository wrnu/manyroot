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
use crate::task::{CHANGES, SPECS, Version, change_tag, version_of};

pub struct MockBackend {
    pub seed: u64,
    /// Chance that an implementation is a known-wrong program.
    pub wrong_rate: f64,
    /// Talk through `bracken-experiment shim` at this path, as Claude Code would, instead of the socket directly.
    pub shim: Option<PathBuf>,
}

/// A minimal MCP client over newline-delimited JSON-RPC.
pub struct Client {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next: u64,
    /// Tool calls made so far.
    pub calls: u32,
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
        let mut c = Client { reader: BufReader::new(reader), writer, next: 1, calls: 0, _child: child };
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
fn next_spec(memory: &Memory, agent: usize) -> Option<&'static crate::task::Spec> {
    let todo: Vec<_> = SPECS.iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
    todo.get(agent % 3).or(todo.first()).copied()
}

fn first_line(v: &Value) -> String {
    v["output"].as_str().unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
}

impl Backend for MockBackend {
    fn turn(&self, agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        let mut rng = Rng::derive(self.seed, (agent as u64) << 16 | u64::from(turn));
        let mut c = Client::connect(socket, self.shim.as_deref())?;
        let tools = c.request("tools/list", json!({}))?;
        let has = |tool: &str| tools["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == tool));
        let notes = c.tool("notes_read", json!({}))?;
        let mut memory: Memory = serde_json::from_str(notes["notes"].as_str().unwrap_or_default()).unwrap_or_default();
        if has("replace") && !has("pr_open") {
            self.crdt_turn(&mut c, agent, name, turn, &mut rng, &mut memory)?;
            let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
            c.tool("notes_write", json!({"text": text}))?;
            c.tool("sync", json!({}))?;
            return Ok(());
        }
        if has("pr_open") {
            self.git_turn(&mut c, agent, &mut rng, &mut memory)?;
            let text = serde_json::to_string(&memory).map_err(|e| e.to_string())?;
            c.tool("notes_write", json!({"text": text}))?;
            c.tool("sync", json!({}))?;
            return Ok(());
        }
        if !has("log") {
            return Err(format!("{name}: no log tool"));
        }

        let changes = c.tool("log", json!({"tag": "change", "current": true}))?;
        let seen: Vec<&str> = changes
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|e| e["body"]["tags"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        let version = CHANGES
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
        if let Some(spec) = next_spec(&memory, agent) {
            let program = if rng.chance(self.wrong_rate) {
                candidates(spec.name).wrong[0].to_string()
            } else {
                correct_programs(version)[spec.name].to_string()
            };
            let content = format!("def {} {} : {program}\n", spec.name, spec.arity);
            let artifact = first_line(&c.tool("artifact", json!({"function": spec.name, "content": content}))?);
            let to = spec_id(spec.name).ok_or_else(|| format!("{name}: no spec claim for {}", spec.name))?;
            c.tool(
                "relate",
                json!({"from": artifact, "kind": "implements", "to": to, "note": format!("turn {turn}")}),
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
        use crate::crdt_mode::{MODULE_FILES, NOTES, TASKS};
        let version = version_of(&file_text(&c.tool("read", json!({"file": TASKS}))?));
        let todo: Vec<_> = SPECS.iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
        let Some(spec) = todo.get(agent % 3).or(todo.first()).copied() else { return Ok(()) };
        let program = if rng.chance(self.wrong_rate) {
            candidates(spec.name).wrong[0].to_string()
        } else {
            correct_programs(version)[spec.name].to_string()
        };
        let line = format!("def {} {} : {program}", spec.name, spec.arity);
        let index = crate::task::MODULES.iter().position(|m| *m == spec.module).ok_or("a spec in no module")?;
        let file = MODULE_FILES[index];
        let text = file_text(&c.tool("read", json!({"file": file}))?);
        let prefix = format!("def {} ", spec.name);
        match text.lines().find(|l| l.starts_with(&prefix)) {
            Some(old) => c.tool("replace", json!({"file": file, "old": old, "new": line}))?,
            None => {
                let n = text.lines().count() as i64;
                c.tool("edit", json!({"file": file, "start": n + 1, "end": n, "text": line}))?
            }
        };
        let checked = c.tool("check", json!({"function": spec.name}))?;
        let notes = file_text(&c.tool("read", json!({"file": NOTES}))?);
        let n = notes.lines().count() as i64;
        let ok = checked["ok"].as_bool().unwrap_or(false);
        let note =
            format!("{name}, turn {turn}: wrote {} (public check {})", spec.name, if ok { "passes" } else { "fails" });
        c.tool("edit", json!({"file": NOTES, "start": n + 1, "end": n, "text": note}))?;
        memory.implemented.push(spec.name.to_string());
        Ok(())
    }

    fn git_turn(&self, c: &mut Client, agent: usize, rng: &mut Rng, memory: &mut Memory) -> Result<(), String> {
        let version = version_of(&file_text(&c.tool("read", json!({"file": "TASKS.md"}))?));
        let program = |spec: &crate::task::Spec, rng: &mut Rng| {
            if rng.chance(self.wrong_rate) {
                candidates(spec.name).wrong[0].to_string()
            } else {
                correct_programs(version)[spec.name].to_string()
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
        if let Some(spec) = next_spec(memory, agent).filter(|_| c.calls + 6 <= budget) {
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
