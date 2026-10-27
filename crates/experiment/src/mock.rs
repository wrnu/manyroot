//! The mock backend: plays an agent's turn with scripted moves, through the
//! same MCP tool calls a model would make (design section 3, Turns), so a
//! whole run can be checked without a model.
//!
//! Each turn the mock implements one function it hasn't yet (usually with a
//! correct program, sometimes a wrong one), checks up to two implementations
//! by others and records the result as evidence, and keeps what it has done in
//! its private notes.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bracken_demo::rng::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bracken_mode::change_tag;
use crate::candidates::{candidates, correct_programs};
use crate::driver::Backend;
use crate::task::{CHANGES, SPECS, Version};

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
        let mut c = Client { reader: BufReader::new(reader), writer, next: 1, _child: child };
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
}

fn first_line(v: &Value) -> String {
    v["output"].as_str().unwrap_or_default().lines().next().unwrap_or_default().trim().to_string()
}

impl Backend for MockBackend {
    fn turn(&self, agent: usize, name: &str, turn: u32, socket: &Path) -> Result<(), String> {
        let mut rng = Rng::derive(self.seed, (agent as u64) << 16 | u64::from(turn));
        let mut c = Client::connect(socket, self.shim.as_deref())?;
        let tools = c.request("tools/list", json!({}))?;
        if !tools["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == "log")) {
            return Err(format!("{name}: no log tool"));
        }
        let notes = c.tool("notes_read", json!({}))?;
        let mut memory: Memory = serde_json::from_str(notes["notes"].as_str().unwrap_or_default()).unwrap_or_default();

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

        // Implement one function not done yet, low layers first, spread across agents.
        let todo: Vec<_> = SPECS.iter().filter(|s| !memory.implemented.iter().any(|d| d == s.name)).collect();
        if let Some(spec) = todo.get(agent % 3).or(todo.first()) {
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
