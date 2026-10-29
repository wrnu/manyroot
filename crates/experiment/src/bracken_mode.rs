//! Mode 3, Bracken semantic contributions (design section 2).
//!
//! Each agent's copy and the hub are Bracken replicas of one Ground. Tools run
//! the real `bracken` binary; every value an agent chooses passes the rules in
//! [`crate::args`], lands after `--` or inside one `--option=value` argument,
//! and free text that bracken can read from standard input goes there. An
//! implementation is an artifact holding the function's definition (and any
//! helpers named after it) that `implements` the function's spec claim. Only
//! the setter's claims state the task, its specs and its change requests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::args::Args;
use crate::lang::{Library, parse_module};
use crate::mode::{Mode, Session, Store, StoreConfig, ToolSpec, is_within};
use crate::task::{Task, Version, change_tag};

/// The media type of an implementation artifact.
pub const MEDIA_TYPE: &str = "text/x-stacklib";
/// Kinds a `log` may filter on.
const KINDS: [&str; 5] = ["claim", "artifact", "relation", "evidence", "retraction"];
/// Relation kinds an agent may record.
const RELATIONS: [&str; 7] = ["supports", "contradicts", "depends_on", "refines", "supersedes", "implements", "tests"];
const VERDICTS: [&str; 2] = ["pass", "fail"];
/// The largest implementation artifact: a module of the most specs any module has, each this size, still fits.
pub const MAX_ARTIFACT_BYTES: usize = 2000;
/// How long one sync session may take, in seconds, on either side.
const SYNC_DEADLINE: u32 = 60;
/// Bytes one side of a sync session may receive from the other.
const SYNC_BUDGET: u64 = 32 * 1024 * 1024;

pub struct BrackenMode;

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    }
}

impl Mode for BrackenMode {
    fn name(&self) -> &'static str {
        "bracken"
    }

    fn tools(&self, _task: Task) -> Vec<ToolSpec> {
        let id = json!({"type": "string", "description": "A contribution id: 12 to 64 hex digits"});
        let text = json!({"type": "string"});
        vec![
            tool(
                "log",
                "Contributions in your replica (your copy of the shared record), oldest first, as JSON; filter by kind or tag. With current, leave out what was retracted and what its own author superseded",
                json!({"kind": {"type": "string", "enum": KINDS}, "tag": text, "current": {"type": "boolean"}}),
                &[],
            ),
            tool(
                "show",
                "One contribution, with the evidence on it and the relations that point to it",
                json!({"id": id}),
                &["id"],
            ),
            tool("heads", "The newest contributions in your replica: those nothing later builds on", json!({}), &[]),
            tool(
                "conflicts",
                "Contributions that disagree: claims that contradict each other, and rival successors to one claim",
                json!({}),
                &[],
            ),
            tool(
                "accepted",
                "What counts in your replica, per function: the implementation its author related to the current spec claim that has a pass from someone else and no one else's latest verdict a fail (the earliest, if several) is accepted; the function's other implementations are waiting",
                json!({}),
                &[],
            ),
            tool("cat", "The text of an artifact", json!({"id": id}), &["id"]),
            tool(
                "claim",
                "Contribute a claim: a statement, with optional tags and claims of yours it replaces. Claims are where agents talk to each other: everyone can read them with log",
                json!({"statement": text, "tags": {"type": "array", "items": text}, "supersedes": {"type": "array", "items": id}}),
                &["statement"],
            ),
            tool(
                "artifact",
                "Contribute an implementation: the definition of one task function, as stacklib text of at most 2000 bytes. It may define helpers, each named starting with the function's name (e.g. `median3lo`), but no other task function. Relate it with `implements` to its spec claim",
                json!({"function": text, "content": text}),
                &["function", "content"],
            ),
            tool(
                "relate",
                "Link two contributions: from is the subject, kind how it relates to to, for example an artifact implements a spec claim",
                json!({"from": id, "kind": {"type": "string", "enum": RELATIONS}, "to": id, "note": text}),
                &["from", "kind", "to"],
            ),
            tool(
                "evidence",
                "Record your pass or fail verdict on a contribution; method says how you judged it, details what you found. A later verdict of yours replaces your earlier one",
                json!({"target": id, "verdict": {"type": "string", "enum": VERDICTS}, "method": text, "details": text}),
                &["target", "verdict", "method"],
            ),
            tool(
                "retract",
                "Withdraw one of your contributions, with a reason; it then no longer counts",
                json!({"target": id, "reason": text}),
                &["target"],
            ),
            tool(
                "check",
                "Run an implementation artifact on its function's public cases; calls to other functions use what counts in your replica, else your own latest implementation, else anyone's",
                json!({"id": id}),
                &["id"],
            ),
        ]
    }

    fn open(&self, config: StoreConfig) -> Box<dyn Store> {
        Box::new(BrackenStore { config, runs: 0, files: 0 })
    }
}

/// A command to run: `bracken --dir <replica> <argv>`, with `stdin` if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    /// Free text the store writes to a file of its own before running; the file's path ends `argv`.
    pub file: Option<String>,
    /// Whether the output is JSON.
    pub json: bool,
}

fn opt(name: &str, value: impl std::fmt::Display) -> String {
    format!("--{name}={value}")
}

/// The command for tool `name`, signing as `identity`. Every value from `args`
/// is checked by its rule (unless the test-only unchecked args are given), and
/// lands after `--` or inside one `--option=value` argument, never anywhere it
/// could be read as an option.
pub fn plan(name: &str, args: &Args, identity: &str, task: Task) -> Result<Plan, String> {
    let read = |argv: Vec<String>| Plan { argv, stdin: None, file: None, json: true };
    let write = |mut head: Vec<String>, tail: Vec<String>| {
        head.insert(1, opt("as", identity));
        head.push("--".into());
        head.extend(tail);
        Plan { argv: head, stdin: None, file: None, json: false }
    };
    let s = |v: &str| v.to_string();
    Ok(match name {
        "log" => {
            let mut argv = vec![s("log"), s("--json")];
            if let Some(k) = args.choice_opt("kind", &KINDS)? {
                argv.push(opt("kind", k));
            }
            if let Some(t) = args.tag_opt("tag")? {
                argv.push(opt("tag", t));
            }
            if args.bool_opt("current")? {
                argv.push(s("--current"));
            }
            read(argv)
        }
        "show" => read(vec![s("show"), s("--json"), s("--"), s(args.id("id")?)]),
        "heads" => read(vec![s("heads"), s("--json")]),
        "conflicts" => read(vec![s("conflicts"), s("--json")]),
        "cat" => Plan { json: false, ..read(vec![s("cat"), s("--"), s(args.id("id")?)]) },
        "claim" => {
            let mut head = vec![s("claim")];
            head.extend(args.tags("tags")?.into_iter().map(|t| opt("tag", t)));
            head.extend(args.ids("supersedes")?.into_iter().map(|i| opt("supersedes", i)));
            write(head, vec![s(args.text("statement")?)])
        }
        "artifact" => {
            let function = args.spec_name("function", task)?;
            let content = args.text("content")?;
            let head = vec![s("artifact"), opt("name", function), opt("media-type", MEDIA_TYPE)];
            Plan { file: Some(s(content)), ..write(head, vec![]) }
        }
        "relate" => {
            let head = vec![s("relate"), opt("note", args.text_opt("note")?)];
            write(head, vec![s(args.id("from")?), s(args.choice("kind", &RELATIONS)?), s(args.id("to")?)])
        }
        "evidence" => {
            let head = vec![s("evidence"), opt("method", args.text("method")?), s("--details-file=-")];
            let details = args.text_opt("details")?.as_bytes().to_vec();
            let plan = write(head, vec![s(args.id("target")?), s(args.choice("verdict", &VERDICTS)?)]);
            Plan { stdin: Some(details), ..plan }
        }
        "retract" => write(vec![s("retract"), opt("reason", args.text_opt("reason")?)], vec![s(args.id("target")?)]),
        other => return Err(format!("no tool {}", crate::args::short(other))),
    })
}

/// The arguments each tool accepts.
fn allowed(name: &str) -> &'static [&'static str] {
    match name {
        "log" => &["kind", "tag", "current"],
        "show" | "cat" | "check" => &["id"],
        "claim" => &["statement", "tags", "supersedes"],
        "artifact" => &["function", "content"],
        "relate" => &["from", "kind", "to", "note"],
        "evidence" => &["target", "verdict", "method", "details"],
        "retract" => &["target", "reason"],
        _ => &[],
    }
}

pub struct BrackenStore {
    config: StoreConfig,
    /// bracken processes started, for tests that show a refused call starts none.
    pub runs: u64,
    files: u64,
}

impl BrackenStore {
    fn replica(&self) -> PathBuf {
        self.config.dir.join(".bracken")
    }

    /// Runs `bracken --dir <own replica> <argv>` in the store's own directory, with an empty environment.
    fn run(&mut self, argv: &[String], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        self.runs += 1;
        let child = Command::new(&self.config.bracken)
            .arg("--dir")
            .arg(self.replica())
            .args(argv)
            .env_clear()
            .current_dir(&self.config.dir)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting bracken: {e}"))?;
        let out = crate::mode::communicate(child, stdin).map_err(|e| format!("running bracken: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bracken {}: {}", argv.first().map_or("", String::as_str), crate::args::short(err.trim())))
        }
    }

    fn json(&mut self, argv: &[&str]) -> Result<Value, String> {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let out = self.run(&argv, None)?;
        serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))
    }

    fn write(&mut self, argv: Vec<String>) -> Result<String, String> {
        let out = self.run(&argv, None)?;
        Ok(String::from_utf8_lossy(&out).lines().next().unwrap_or_default().trim().to_string())
    }

    /// A file of the store's own for `content`, under `files/`.
    fn own_file(&mut self, content: &str) -> Result<PathBuf, String> {
        let dir = self.config.dir.join("files");
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        self.files += 1;
        let path = dir.join(format!("{}.stk", self.files));
        if !is_within(&path, &self.config.dir) {
            return Err("refusing a file outside the store".into());
        }
        std::fs::write(&path, content).map_err(|e| format!("writing {}: {e}", path.display()))?;
        Ok(path)
    }

    fn state(&mut self) -> Result<Snapshot, String> {
        let current = self.json(&["log", "--json", "--current"])?;
        Snapshot::new(&current, &self.saved("setter.txt")?, self.config.accept_rule, self.config.task)
    }

    /// A key this store saved at setup.
    fn saved(&self, file: &str) -> Result<String, String> {
        std::fs::read_to_string(self.config.dir.join(file))
            .map(|k| k.trim().to_string())
            .map_err(|e| format!("no {file}: {e}"))
    }

    fn save(&self, file: &str, key: &str) -> Result<(), String> {
        std::fs::write(self.config.dir.join(file), key).map_err(|e| format!("writing {file}: {e}"))
    }

    /// A new identity for this store; its public key, saved as `key.txt`.
    fn new_identity(&mut self) -> Result<String, String> {
        let key = self.run(&["id".into(), "new".into(), "--".into(), self.config.identity.clone()], None)?;
        let key = String::from_utf8_lossy(&key).trim().to_string();
        self.save("key.txt", &key)?;
        Ok(key)
    }

    /// The setter states the task: a task claim, and one spec claim per function refining it.
    fn setup_hub(&mut self) -> Result<Value, String> {
        self.run(&["init".into(), "--name=hub".into()], None)?;
        let key = self.new_identity()?;
        self.save("setter.txt", &key)?;
        let me = self.config.identity.clone();
        let task = self.write(vec![
            "claim".into(),
            opt("as", &me),
            "--tag=task".into(),
            "--".into(),
            self.config.task.task_text(Version::ORIGINAL),
        ])?;
        for s in self.config.task.specs() {
            let statement = format!("Implement {}.", s.description);
            let spec = self.write(vec![
                "claim".into(),
                opt("as", &me),
                "--tag=spec".into(),
                opt("tag", s.name),
                "--".into(),
                statement,
            ])?;
            self.write(vec![
                "relate".into(),
                opt("as", &me),
                "--note=".into(),
                "--".into(),
                spec,
                "refines".into(),
                task.clone(),
            ])?;
        }
        Ok(json!({ "task": task }))
    }
}

impl Store for BrackenStore {
    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String> {
        let Some(bundle) = hub_export else { return self.setup_hub() };
        let path = self.config.dir.join("join.mrb");
        std::fs::write(&path, bundle).map_err(|e| format!("writing the bundle: {e}"))?;
        self.run(&["join".into(), "--".into(), path.to_string_lossy().into_owned()], None)?;
        // Before any agent has written, the hub's export holds only the setter's work: its task claim names the setter.
        let log = self.json(&["log", "--json", "--tag=task"])?;
        let setter = match log.as_array().map(Vec::as_slice) {
            Some([task]) => task["author"].as_str().unwrap_or_default().to_string(),
            _ => return Err("the hub's export must hold exactly one task claim".into()),
        };
        self.save("setter.txt", &setter)?;
        let key = self.new_identity()?;
        Ok(json!({ "key": key }))
    }

    fn export(&mut self) -> Result<Vec<u8>, String> {
        self.run(&["export".into(), "-".into()], None)
    }

    fn import(&mut self, bytes: &[u8]) -> Result<Value, String> {
        let out = self.run(&["import".into(), "-".into()], Some(bytes))?;
        Ok(json!({ "imported": String::from_utf8_lossy(&out).trim() }))
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        if name == "accepted" {
            // The harness's own count, not real Bracken's `accepted`, so the tool shows what counts
            // (decision `5e8df6996a2e`; finding `6792b444895d`).
            Args::new(args, allowed("accepted"))?;
            return Ok(self.state()?.counted());
        }
        if name == "check" {
            let id = Args::new(args, allowed("check"))?.id("id")?.to_string();
            return self.check(&id);
        }
        if !self.tools_known(name) {
            return Err(format!("no tool {}", crate::args::short(name)));
        }
        let args = Args::new(args, allowed(name))?;
        if name == "artifact"
            && let Some(problem) =
                artifact_problem(self.config.task, args.spec_name("function", self.config.task)?, args.text("content")?)
        {
            return Err(problem);
        }
        let plan = plan(name, &args, &self.config.identity, self.config.task)?;
        self.execute(plan)
    }

    fn sync_client(&mut self, socket: &Path, relay: &Path) -> Result<Value, String> {
        let mut argv = vec!["sync".into(), opt("deadline", SYNC_DEADLINE), opt("budget", SYNC_BUDGET), "--exec".into()];
        argv.extend([relay.to_string_lossy().into_owned(), "shim".into(), "--socket".into()]);
        argv.push(socket.to_string_lossy().into_owned());
        let out = self.run(&argv, None)?;
        Ok(json!({ "synced": crate::args::short(String::from_utf8_lossy(&out).trim()) }))
    }

    fn sync_server(&mut self, socket: &Path, session: &Session) -> Result<Value, String> {
        let allow = self.config.dir.join("allow.txt");
        std::fs::write(&allow, session.peers.join("\n") + "\n").map_err(|e| format!("writing the allow list: {e}"))?;
        // Over stdio every session has the same credential, and serve counts quotas from the log it is
        // given: one log per agent gives each agent its own quota (finding 57acf0d9ce57).
        let first_seen = self.config.dir.join(format!("first-seen-{}.jsonl", session.agent));
        let stream = std::os::unix::net::UnixStream::connect(socket)
            .map_err(|e| format!("connecting to {}: {e}", socket.display()))?;
        let seconds = if session.sync_seconds > 0 { session.sync_seconds } else { SYNC_DEADLINE };
        self.runs += 1;
        let mut child = Command::new(&self.config.bracken)
            .arg("--dir")
            .arg(self.replica())
            .args(["serve", "--stdio"])
            .arg(format!("--allow={}", allow.display()))
            .arg(format!("--first-seen={}", first_seen.display()))
            .args([opt("deadline", seconds), opt("budget", SYNC_BUDGET)])
            .args([opt("quota-contributions", session.quota_contributions), opt("quota-bytes", session.quota_bytes)])
            .env_clear()
            .current_dir(&self.config.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting bracken: {e}"))?;
        let (mut to_bracken, mut from_bracken) =
            (child.stdin.take().ok_or("no bracken stdin")?, child.stdout.take().ok_or("no bracken stdout")?);
        let up = stream.try_clone().map_err(|e| e.to_string())?;
        let control = stream.try_clone().map_err(|e| e.to_string())?;
        let mut down = stream;
        let limit = std::time::Duration::from_secs(u64::from(seconds) + 5);
        let deadline = std::time::Instant::now() + limit;
        // A peer that stops reading can hold a write for at most the session's time.
        let _ = down.set_write_timeout(Some(limit));
        let done = std::sync::atomic::AtomicBool::new(false);
        let mut killed = false;
        let relayed = std::thread::scope(|scope| {
            // Both halves on threads, so this one can stop bracken at the deadline whatever they wait
            // on (finding `a8cd5b213d6f`). The incoming half polls, and stops once bracken has
            // finished: on macOS a shutdown of the socket doesn't wake a read blocked on its clone
            // (finding `05bee0cd4fc4`).
            let incoming = scope.spawn(|| crate::shim::relay_until(&up, &mut to_bracken, &done, deadline));
            let outgoing = scope.spawn(|| crate::shim::relay(&mut from_bracken, &mut down));
            while !outgoing.is_finished() {
                if !killed && std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    killed = true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = control.shutdown(std::net::Shutdown::Both);
            let outgoing = outgoing.join().unwrap_or_else(|_| Err("the relay panicked".into()));
            let incoming = incoming.join().unwrap_or_else(|_| Err("the relay panicked".into()));
            outgoing.and(incoming)
        });
        // The relay can end (a write timing out, say) with bracken still running: it gets until
        // the deadline to finish, and is stopped then.
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() > deadline || (relayed.is_err() && !killed) => {
                    let _ = child.kill();
                    killed = true;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(e) => return Err(format!("waiting for bracken: {e}")),
            }
        }
        if killed {
            return Err("the sync session passed its deadline or failed; bracken was stopped".to_string());
        }
        let out = child.wait_with_output().map_err(|e| format!("running bracken: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bracken serve: {}", crate::args::short(err.trim())));
        }
        relayed?;
        Ok(json!({ "served": true }))
    }

    fn release_changes(&mut self) -> Result<Value, String> {
        let state = self.state()?;
        let me = self.config.identity.clone();
        let task = state.tagged("task").first().map(|e| e.id.clone()).ok_or("no task claim")?;
        let mut released = Vec::new();
        for c in self.config.task.changes() {
            let old = state.spec_claim(c.target).ok_or_else(|| format!("no spec claim for {}", c.target))?;
            self.write(vec![
                "claim".into(),
                opt("as", &me),
                "--tag=change".into(),
                opt("tag", change_tag(c.change)),
                "--".into(),
                format!("Change request: {}", c.text),
            ])?;
            let spec = self.write(vec![
                "claim".into(),
                opt("as", &me),
                "--tag=spec".into(),
                opt("tag", c.target),
                opt("supersedes", &old),
                "--".into(),
                format!("Implement {}.", c.description),
            ])?;
            self.write(vec![
                "relate".into(),
                opt("as", &me),
                "--note=".into(),
                "--".into(),
                spec,
                "refines".into(),
                task.clone(),
            ])?;
            released.push(change_tag(c.change));
        }
        Ok(json!({ "released": released }))
    }

    fn integrated(&mut self) -> Result<Vec<(String, String)>, String> {
        let state = self.state()?;
        Ok(modules_of(self.config.task, &state.integrated()))
    }

    fn record(&mut self, tool: &str, args: &Value, _result: &Value) -> Result<Value, String> {
        let task = self.config.task;
        let of = |function: &str, text: &str| {
            let defs = crate::metrics::definitions(task, &[("artifact".into(), text.into())]);
            defs.get(function).map(|d| vec![(function.to_string(), d.clone())]).unwrap_or_default()
        };
        // A task with open contracts: the calls that can change which of this agent's own implementations of a
        // member are current also record them, the agent's working copy of the contracts (decision `979e5085a1e7`).
        let members = |me: &mut BrackenStore| -> Result<Option<Value>, String> {
            if task.contracts().is_empty() || !["artifact", "relate", "claim", "retract"].contains(&tool) {
                return Ok(None);
            }
            let (state, key) = (me.state()?, me.saved("key.txt")?);
            let own: Vec<_> = state
                .own_implementations(&key)
                .into_iter()
                .filter(|(f, _)| task.contract_of(f).is_some())
                .flat_map(|(f, text)| of(f, &text))
                .collect();
            Ok(Some(json!(own)))
        };
        let mut record = match tool {
            "artifact" => {
                let function = args["function"].as_str().unwrap_or_default();
                json!({ "writes": of(function, args["content"].as_str().unwrap_or_default()) })
            }
            // A non-author's fail on an implementation: a counted fail, which keeps it out (decision `5e8df6996a2e`).
            "evidence" if args["verdict"] == "fail" => {
                let (state, me) = (self.state()?, self.saved("key.txt")?);
                let target = args["target"].as_str().unwrap_or_default();
                let blocks = match state.find(target) {
                    Some(e) if e.kind == "artifact" && e.author != me => {
                        of(e.body["name"].as_str().unwrap_or_default(), e.body["text"].as_str().unwrap_or_default())
                    }
                    _ => Vec::new(),
                };
                json!({ "blocks": blocks })
            }
            _ => Value::Null,
        };
        if let Some(own) = members(self)? {
            if record.is_null() {
                record = json!({});
            }
            record["members"] = own;
        }
        Ok(record)
    }
}

impl BrackenStore {
    fn tools_known(&self, name: &str) -> bool {
        BrackenMode.tools(self.config.task).iter().any(|t| t.name == name)
    }

    fn execute(&mut self, mut plan: Plan) -> Result<Value, String> {
        if let Some(content) = plan.file.take() {
            let path = self.own_file(&content)?;
            plan.argv.push(path.to_string_lossy().into_owned());
        }
        let out = self.run(&plan.argv, plan.stdin.as_deref())?;
        if plan.json {
            serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))
        } else {
            let text = String::from_utf8_lossy(&out).into_owned();
            Ok(json!({ "output": text }))
        }
    }

    /// The public check of artifact `id`, against the working library of this replica.
    fn check(&mut self, id: &str) -> Result<Value, String> {
        let state = self.state()?;
        let artifact = state.find(id).ok_or("no such artifact in your replica")?;
        if artifact.kind != "artifact" {
            return Err("check takes an artifact".into());
        }
        let function = artifact.body["name"].as_str().unwrap_or_default().to_string();
        let task = self.config.task;
        let spec = task.spec_named(&function).ok_or("the artifact doesn't name a task function")?;
        let text = artifact.body["text"].as_str().unwrap_or_default().to_string();
        if let Some(problem) = artifact_problem(task, spec.name, &text) {
            return Ok(json!({ "function": spec.name, "ok": false, "details": problem }));
        }
        let me = self.saved("key.txt")?;
        let mut programs = state.working(&me);
        programs.insert(spec.name, text);
        let modules = modules_of(task, &programs);
        let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
        let result = task.check(&library, spec.name, state.version())?;
        Ok(json!({ "function": spec.name, "ok": result.ok(), "details": result.details() }))
    }
}

/// Programs by function, as artifact texts.
pub type Programs = BTreeMap<&'static str, String>;

/// Module texts for `programs`: each artifact's text in its function's module.
pub fn modules_of(task: Task, programs: &Programs) -> Vec<(String, String)> {
    task.modules()
        .iter()
        .map(|&m| {
            let text: String = task
                .specs()
                .iter()
                .filter(|s| s.module == m)
                .filter_map(|s| programs.get(s.name))
                .map(|t| if t.ends_with('\n') { t.clone() } else { format!("{t}\n") })
                .collect();
            (m.to_string(), text)
        })
        .collect()
}

/// Why `text` can't implement `function`, if it can't: it must parse alone,
/// fit [`MAX_ARTIFACT_BYTES`], define `function` once, and name every helper
/// starting with `function` (no spec name starts another, so a helper is never
/// a task function and never collides with another function's helpers).
pub fn artifact_problem(task: Task, function: &str, text: &str) -> Option<String> {
    if text.len() > MAX_ARTIFACT_BYTES {
        return Some(format!("an implementation holds at most {MAX_ARTIFACT_BYTES} bytes, not {}", text.len()));
    }
    let defs = match parse_module("artifact", text) {
        Ok(defs) => defs,
        Err(e) => return Some(format!("it doesn't parse: {e}")),
    };
    let mut names = std::collections::BTreeSet::new();
    for d in &defs {
        if !names.insert(d.name.as_str()) {
            return Some(format!("`{}` is defined more than once", d.name));
        }
        if d.name != function && (task.spec_named(&d.name).is_some() || !d.name.starts_with(function)) {
            return Some(format!(
                "helper `{}` must be named starting with `{function}`, and not be a task function",
                d.name
            ));
        }
    }
    (!names.contains(function)).then(|| format!("it doesn't define `{function}`"))
}

/// Which implementation counts for a function, among those with an independent pass and no
/// independent latest fail. The plan records the rule a run used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AcceptRule {
    /// Decision `a4d8622c75d5` point 2, as Bracken's default: one independent pass accepts, and
    /// among several the earliest in causal order counts.
    #[default]
    #[serde(rename = "one-pass")]
    OnePass,
    /// Phase 1's rule (decisions `b29d5db02da8` point 7, `5e8df6996a2e`): the one with the most
    /// passes counts, then the earliest. Kept to reproduce Phase 1.
    #[serde(rename = "phase1-most-passes")]
    MostPasses,
}

impl AcceptRule {
    pub const NAMES: [&str; 2] = ["one-pass", "phase1-most-passes"];

    pub fn name(self) -> &'static str {
        match self {
            AcceptRule::OnePass => "one-pass",
            AcceptRule::MostPasses => "phase1-most-passes",
        }
    }

    pub fn named(name: &str) -> Option<AcceptRule> {
        [AcceptRule::OnePass, AcceptRule::MostPasses].into_iter().find(|r| r.name() == name)
    }

    /// Plans written before the rule was selectable used Phase 1's.
    pub fn before_selectable() -> AcceptRule {
        AcceptRule::MostPasses
    }

    /// The rule as the plan states it.
    pub fn text(self) -> &'static str {
        match self {
            AcceptRule::OnePass => {
                "Decisions a4d8622c75d5 point 2 and 5e8df6996a2e (rule one-pass): for each current spec, the \
                 current implementations its author related to it with at least one independent pass and no \
                 counted fail, counting each validator once (its latest current verdict) and never the author; \
                 the earliest in causal order."
            }
            AcceptRule::MostPasses => {
                "Decisions b29d5db02da8 point 7 and 5e8df6996a2e (rule phase1-most-passes): for each current \
                 spec, the current implementations with at least one independent pass and no counted fail, \
                 counting each validator once (its latest current verdict) and never the author; the most \
                 passes first, then causal order."
            }
        }
    }
}

/// One row of `bracken log --json`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    /// The author's public key.
    pub author: String,
    pub body: Value,
    /// Position in causal order.
    pub index: usize,
}

impl Entry {
    fn tags(&self) -> Vec<&str> {
        self.body["tags"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
    }
}

/// What a replica holds that the materializer and the check need.
pub struct Snapshot {
    /// `log --current`, in causal order: nothing retracted, nothing its own author superseded.
    current: Vec<Entry>,
    /// The setter's public key: only its claims state the task, the specs and the change requests.
    setter: String,
    /// For each target id, each validator's latest current verdict.
    verdicts: BTreeMap<String, BTreeMap<String, String>>,
    rule: AcceptRule,
    task: Task,
}

/// One implementation of a current spec.
struct Implementation<'a> {
    spec: &'static str,
    entry: &'a Entry,
}

impl Snapshot {
    pub fn new(current: &Value, setter: &str, rule: AcceptRule, task: Task) -> Result<Snapshot, String> {
        let rows = current.as_array().ok_or("log --json is not a list")?;
        let current: Vec<Entry> = rows
            .iter()
            .enumerate()
            .map(|(index, r)| Entry {
                id: r["id"].as_str().unwrap_or_default().to_string(),
                kind: r["kind"].as_str().unwrap_or_default().to_string(),
                author: r["author"].as_str().unwrap_or_default().to_string(),
                body: r["body"].clone(),
                index,
            })
            .collect();
        let mut verdicts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for e in current.iter().filter(|e| e.kind == "evidence") {
            let (Some(target), Some(verdict)) = (e.body["target"].as_str(), e.body["verdict"].as_str()) else {
                continue;
            };
            // Causal order: a later verdict by the same validator replaces its earlier one.
            verdicts.entry(target.to_string()).or_default().insert(e.author.clone(), verdict.to_string());
        }
        Ok(Snapshot { current, setter: setter.to_string(), verdicts, rule, task })
    }

    /// The setter's current claims tagged `tag`.
    fn tagged(&self, tag: &str) -> Vec<&Entry> {
        self.current
            .iter()
            .filter(|e| e.kind == "claim" && e.author == self.setter && e.tags().contains(&tag))
            .collect()
    }

    /// The current spec claim for `function`.
    fn spec_claim(&self, function: &str) -> Option<String> {
        self.tagged("spec").into_iter().rev().find(|e| e.tags().contains(&function)).map(|e| e.id.clone())
    }

    fn find(&self, id: &str) -> Option<&Entry> {
        let mut matches = self.current.iter().filter(|e| e.id.starts_with(id));
        let first = matches.next()?;
        if matches.next().is_some() { None } else { Some(first) }
    }

    /// The change requests this replica has seen.
    pub fn version(&self) -> Version {
        let tags: Vec<&str> = self.tagged("change").iter().flat_map(|e| e.tags()).collect();
        let task = self.task;
        task.changes()
            .iter()
            .filter(|c| tags.contains(&change_tag(c.change)))
            .fold(Version::ORIGINAL, |v, c| v.with(c.change))
    }

    /// Current artifacts that implement a current spec, related by their own
    /// author, and hold that function's definition by the artifact rules.
    fn implementations(&self) -> Vec<Implementation<'_>> {
        let specs: BTreeMap<String, &'static str> =
            self.task.specs().iter().filter_map(|s| self.spec_claim(s.name).map(|id| (id, s.name))).collect();
        let by_id: BTreeMap<&str, &Entry> = self.current.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut out = Vec::new();
        for r in self.current.iter().filter(|e| e.kind == "relation" && e.body["kind"] == "implements") {
            let (Some(from), Some(to)) = (r.body["from"].as_str(), r.body["to"].as_str()) else { continue };
            let (Some(artifact), Some(&spec)) = (by_id.get(from), specs.get(to)) else { continue };
            if artifact.kind == "artifact"
                && artifact.author == r.author
                && artifact.body["name"] == spec
                && artifact_problem(self.task, spec, artifact.body["text"].as_str().unwrap_or_default()).is_none()
                && !out.iter().any(|i: &Implementation| i.entry.id == artifact.id)
            {
                out.push(Implementation { spec, entry: artifact });
            }
        }
        out
    }

    /// Decision `5e8df6996a2e`: the independent passes on `entry`, counting
    /// each validator once (its latest current verdict) and never the author;
    /// none when there is no pass or a counted fail.
    fn passes(&self, entry: &Entry) -> Option<usize> {
        let verdicts = self.verdicts.get(&entry.id)?;
        let counted = || verdicts.iter().filter(|(validator, _)| **validator != entry.author).map(|(_, v)| v.as_str());
        if counted().any(|v| v != "pass") {
            return None;
        }
        Some(counted().count()).filter(|&n| n > 0)
    }

    /// The fixed rule ([`AcceptRule`], counted per `5e8df6996a2e`): for each
    /// current spec, the current implementations with at least one independent
    /// pass and no counted fail; the earliest in causal order, or under Phase 1's
    /// rule the one with the most passes, then the earliest. None: missing.
    fn pick(&self) -> BTreeMap<&'static str, &Entry> {
        let mut best: BTreeMap<&'static str, (usize, usize, &Entry)> = BTreeMap::new();
        for i in self.implementations() {
            let Some(passes) = self.passes(i.entry) else { continue };
            let earlier = |index: usize| i.entry.index < index;
            let better = match (best.get(i.spec), self.rule) {
                (None, _) => true,
                (Some(&(_, index, _)), AcceptRule::OnePass) => earlier(index),
                (Some(&(p, index, _)), AcceptRule::MostPasses) => passes > p || (passes == p && earlier(index)),
            };
            if better {
                best.insert(i.spec, (passes, i.entry.index, i.entry));
            }
        }
        best.into_iter().map(|(s, (_, _, e))| (s, e)).collect()
    }

    /// What the `accepted` tool shows: per function, the fixed rule's pick as accepted and the
    /// function's other current implementations as waiting, with no pass counts (decision
    /// `a4d8622c75d5` point 2).
    pub fn counted(&self) -> Value {
        let picked = self.pick();
        let mut implementations = self.implementations();
        implementations.sort_by_key(|i| (i.spec, i.entry.index));
        let rows: Vec<Value> = implementations
            .into_iter()
            .map(|i| {
                let status =
                    if picked.get(i.spec).is_some_and(|e| e.id == i.entry.id) { "accepted" } else { "waiting" };
                json!({"function": i.spec, "id": i.entry.id, "author": i.entry.author, "status": status})
            })
            .collect();
        Value::Array(rows)
    }

    /// The integrated library: the fixed rule's pick, and nothing else.
    pub fn integrated(&self) -> Programs {
        self.pick().into_iter().map(|(s, e)| (s, e.body["text"].as_str().unwrap_or_default().to_string())).collect()
    }

    /// The working library `check` resolves calls in (answer `88ae128c5fcd`):
    /// for each spec, the fixed rule's pick; else the latest current
    /// implementation by `me` (a public key); else the latest current one by anyone.
    pub fn working(&self, me: &str) -> Programs {
        self.working_of(me)
    }

    /// The current implementations by `me` (a public key), in causal order, as `(function, text)`.
    pub fn own_implementations(&self, me: &str) -> Vec<(&'static str, String)> {
        let mut own: Vec<_> = self.implementations().into_iter().filter(|i| i.entry.author == me).collect();
        own.sort_by_key(|i| i.entry.index);
        own.into_iter().map(|i| (i.spec, i.entry.body["text"].as_str().unwrap_or_default().to_string())).collect()
    }

    fn working_of(&self, me: &str) -> Programs {
        let picked = self.pick();
        let implementations = self.implementations();
        let mut out = Programs::new();
        for s in self.task.specs() {
            let latest = |mine: bool| {
                implementations
                    .iter()
                    .filter(|i| i.spec == s.name && (!mine || i.entry.author == me))
                    .max_by_key(|i| i.entry.index)
                    .map(|i| i.entry)
            };
            if let Some(e) = picked.get(s.name).copied().or_else(|| latest(true)).or_else(|| latest(false)) {
                out.insert(s.name, e.body["text"].as_str().unwrap_or_default().to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
