//! Mode 2, CRDT shared source editing (design section 2), on Automerge.
//!
//! One Automerge document with a fixed shape, made by the setter at setup:
//! text objects for the four module files, `TASKS.md` (the setter's only) and
//! `NOTES.md` (shared notes), named as in Git mode. Each agent's copy is its
//! own document; sync is Automerge's sync protocol between a copy and the hub,
//! its messages carried by the driver, and every message checked by
//! [`wire`] before Automerge sees it. The harness sets each agent's actor; the
//! hub takes a change it lacks only from the actor of the agent it is syncing
//! with, and only text edits on the module files and `NOTES.md` (decision
//! `89568ebb896f`). No value an agent chooses names a path or reaches a
//! process: file names select one of the document's fixed text objects.

use std::collections::BTreeSet;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use automerge::{ActorId, AutoCommit, ObjId, ObjType, ROOT, ReadDoc, transaction::Transactable};
use serde_json::{Value, json};

use crate::args::Args;
use crate::lang::Library;
use crate::mode::{Mode, Session, Store, StoreConfig, ToolSpec, is_within};
use crate::task::{Task, Version, change_tag};

pub mod wire;

/// The task, written only by the setter.
pub const TASKS: &str = "TASKS.md";
/// The shared notes, which every agent may edit.
pub const NOTES: &str = "NOTES.md";
/// Every text object in the document, in the order the setter makes them: the module files (as in Git
/// mode), the task and the notes.
pub fn files(task: Task) -> Vec<&'static str> {
    task.module_files().iter().copied().chain([TASKS, NOTES]).collect()
}

/// The files an agent may edit: the module files and the notes, never `TASKS.md`.
fn editable_files(task: Task) -> Vec<&'static str> {
    files(task).into_iter().filter(|f| *f != TASKS).collect()
}
/// The largest file an edit may leave, as in Git mode.
pub const MAX_FILE: usize = 64 * 1024;
/// The document's file in a store's directory.
const DOC_FILE: &str = "doc.automerge";

pub struct CrdtMode;

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    }
}

impl Mode for CrdtMode {
    fn name(&self) -> &'static str {
        "crdt"
    }

    fn tools(&self, task: Task) -> Vec<ToolSpec> {
        let text = json!({"type": "string"});
        let readable = json!({"type": "string", "enum": files(task)});
        let editable = json!({"type": "string", "enum": editable_files(task)});
        let line = json!({"type": "integer", "minimum": 1, "maximum": 100_000});
        vec![
            tool(
                "read",
                "Read a file of your copy of the shared document, with line numbers. NOTES.md is where agents talk to each other: everyone can read it with read",
                json!({"file": readable, "start": line, "count": line}),
                &["file"],
            ),
            tool(
                "edit",
                "Replace lines start..end (inclusive) of a module file or NOTES.md with text; end = start - 1 inserts before start",
                json!({"file": editable, "start": line, "end": {"type": "integer", "minimum": 0, "maximum": 100_000}, "text": text}),
                &["file", "start", "end", "text"],
            ),
            tool(
                "replace",
                "Replace the one occurrence of old with new in a module file or NOTES.md",
                json!({"file": editable, "old": text, "new": text}),
                &["file", "old", "new"],
            ),
            tool(
                "check",
                "Run a function on the public cases, with every call resolved in your copy's module files",
                json!({"function": text}),
                &["function"],
            ),
        ]
    }

    fn open(&self, config: StoreConfig) -> Box<dyn Store> {
        Box::new(CrdtStore { config, doc: None })
    }
}

/// The arguments each tool accepts.
fn allowed(name: &str) -> &'static [&'static str] {
    match name {
        "read" => &["file", "start", "count"],
        "edit" => &["file", "start", "end", "text"],
        "replace" => &["file", "old", "new"],
        "check" => &["function"],
        _ => &[],
    }
}

/// The actor the harness gives `identity`: 32 bytes derived from its name, so agents can't choose it.
pub fn actor_of(identity: &str) -> ActorId {
    ActorId::from(blake3::derive_key("bracken-experiment crdt actor", identity.as_bytes()).to_vec())
}

pub struct CrdtStore {
    config: StoreConfig,
    doc: Option<AutoCommit>,
}

impl CrdtStore {
    fn path(&self) -> PathBuf {
        self.config.dir.join(DOC_FILE)
    }

    /// The document, loaded from this store's own file the first time.
    fn doc(&mut self) -> Result<&mut AutoCommit, String> {
        if self.doc.is_none() {
            let bytes = std::fs::read(self.path()).map_err(|_| "this store has no document yet".to_string())?;
            let mut doc =
                wire::guard(|| AutoCommit::load(&bytes))?.map_err(|e| format!("loading the document: {e}"))?;
            doc.set_actor(actor_of(&self.config.identity));
            self.doc = Some(doc);
        }
        Ok(self.doc.as_mut().expect("loaded above"))
    }

    /// Runs `f` on the document under [`wire::guard`]. After a panic the document is thrown away,
    /// never saved, and the next use reloads the last save (decision `3a3b3da7c15b`).
    fn with_doc<T>(&mut self, f: impl FnOnce(&mut AutoCommit) -> T) -> Result<T, String> {
        let doc = self.doc()?;
        wire::guard(|| f(doc)).inspect_err(|_| self.doc = None)
    }

    fn save(&mut self) -> Result<(), String> {
        let path = self.path();
        if !is_within(&path, &self.config.dir) {
            return Err("refusing a file outside the store".into());
        }
        let bytes = self.export()?;
        // Automerge can apply ops it then can't save faithfully (an insert at a text's start
        // in place of after an element of the same change saves with "mismatching heads"), so a
        // document is written only if it loads back as itself; otherwise it's thrown away.
        let heads = self.with_doc(|d| d.get_heads())?;
        if wire::guard(|| AutoCommit::load(&bytes).map(|mut d| d.get_heads()))?.ok() != Some(heads) {
            self.doc = None;
            return Err("the document doesn't load back as itself; its last save is kept".into());
        }
        std::fs::write(&path, bytes).map_err(|e| format!("writing the document: {e}"))
    }

    /// A text object by its fixed name.
    fn object(&mut self, file: &str) -> Result<ObjId, String> {
        self.with_doc(|d| match d.get(ROOT, file) {
            Ok(Some((_, id))) => Some(id),
            _ => None,
        })?
        .ok_or_else(|| format!("the document has no {file}"))
    }

    fn text(&mut self, file: &str) -> Result<String, String> {
        let obj = self.object(file)?;
        self.with_doc(|d| d.text(&obj))?.map_err(|e| format!("reading {file}: {e}"))
    }

    /// Replaces `file`'s text with `new` as the smallest splice: the common prefix and suffix stay.
    fn write_text(&mut self, file: &str, new: &str) -> Result<(), String> {
        let obj = self.object(file)?;
        let old: Vec<char> = self.text(file)?.chars().collect();
        let new: Vec<char> = new.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
        let inserted: String = new[prefix..new.len() - suffix].iter().collect();
        let deleted = (old.len() - prefix - suffix) as isize;
        self.with_doc(|d| {
            d.splice_text(&obj, prefix, deleted, &inserted)?;
            d.commit();
            Ok::<_, automerge::AutomergeError>(())
        })?
        .map_err(|e| format!("editing {file}: {e}"))
    }

    /// The text objects, as `counter@actor`, by name.
    fn object_ids(&mut self, files: &[&str]) -> Result<BTreeSet<String>, String> {
        files.iter().map(|f| self.object(f).map(|o| o.to_string())).collect()
    }

    /// The rules for a session. The hub takes edits to the module files and notes from the
    /// agent at the other end only, so every change it holds is by the setter or by the agent
    /// that synced it. A copy, which gets changes only from the hub, takes the setter's edits to
    /// `TASKS.md` and any other actor's to the module files and notes, never the setter's to
    /// those or anyone else's to `TASKS.md`.
    fn rules(&mut self, peers: &[String], hub: bool) -> Result<wire::Rules, String> {
        let editable = self.object_ids(&editable_files(self.config.task))?;
        let tasks = self.object_ids(&[TASKS])?;
        if hub {
            let peer = peers.first().ok_or("a hub session needs the agent's key")?;
            let actor = hex::decode(peer).map(ActorId::from).map_err(|e| format!("a bad peer key: {e}"))?;
            return Ok(wire::Rules { allowed: vec![(actor, editable)], others: None });
        }
        Ok(wire::Rules { allowed: vec![(actor_of("setter"), tasks)], others: Some(editable) })
    }

    fn sync(&mut self, socket: &Path, peers: &[String], hub: bool) -> Result<Value, String> {
        let rules = self.rules(peers, hub)?;
        let mut stream = UnixStream::connect(socket).map_err(|e| format!("connecting to {}: {e}", socket.display()))?;
        let result = self.session(&mut stream, &rules, !hub, wire::Bounds::default());
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let tally = result?;
        Ok(json!({ "frames": tally.frames, "received": tally.received, "new_changes": tally.new_changes }))
    }

    /// One sync session, kept only if it succeeds whole. Otherwise whatever it brought is
    /// dropped, changes held without their deps (and their dependents) included: the document
    /// reloads from its last save, which never holds them (decision `285ce6a0cad9`).
    pub(crate) fn session(
        &mut self,
        stream: &mut UnixStream,
        rules: &wire::Rules,
        first: bool,
        bounds: wire::Bounds,
    ) -> Result<wire::Tally, String> {
        match wire::session(self.doc()?, stream, rules, first, bounds) {
            Ok(tally) => {
                self.save()?;
                Ok(tally)
            }
            Err(e) => {
                self.doc = None;
                Err(e)
            }
        }
    }

    fn setup_hub(&mut self) -> Result<Value, String> {
        let mut doc = AutoCommit::new();
        doc.set_actor(actor_of("setter"));
        let task = self.config.task;
        for file in files(task) {
            let obj = doc.put_object(ROOT, file, ObjType::Text).map_err(|e| e.to_string())?;
            if file == TASKS {
                doc.splice_text(&obj, 0, 0, &task.tasks_md(Version::ORIGINAL)).map_err(|e| e.to_string())?;
            }
        }
        doc.commit();
        self.doc = Some(doc);
        self.save()?;
        Ok(json!({ "files": files(task) }))
    }

    /// An agent's copy from the hub's export: the fixed shape, written by the setter alone.
    fn setup_copy(&mut self, export: &[u8]) -> Result<Value, String> {
        if export.len() > wire::MAX_DOC_BYTES {
            return Err("the hub's export is too large".into());
        }
        let mut doc =
            wire::guard(|| AutoCommit::load(export))?.map_err(|e| format!("loading the hub's export: {e}"))?;
        let keys: Vec<String> = wire::guard(|| doc.keys(ROOT).collect())?;
        if keys.iter().map(String::as_str).collect::<BTreeSet<_>>() != files(self.config.task).into_iter().collect() {
            return Err("the hub's export doesn't have the document's shape".into());
        }
        let setter = actor_of("setter");
        if wire::guard(|| doc.get_changes(&[]).iter().any(|c| *c.actor_id() != setter))? {
            return Err("the hub's export holds changes by someone other than the setter".into());
        }
        let actor = actor_of(&self.config.identity);
        doc.set_actor(actor.clone());
        self.doc = Some(doc);
        for file in files(self.config.task) {
            let obj = self.object(file)?;
            if self.with_doc(|d| d.object_type(obj))? != Ok(ObjType::Text) {
                return Err(format!("{file} isn't text in the hub's export"));
            }
        }
        self.save()?;
        Ok(json!({ "key": actor.to_hex_string() }))
    }

    /// Runs one tool with `args`, which tests may build unchecked.
    pub(crate) fn apply(&mut self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "read" => {
                let file = readable(self.config.task, args)?;
                let text = self.text(file)?;
                let start = args.int_opt("start", 1..=100_000)?.unwrap_or(1) as usize;
                let count = args.int_opt("count", 1..=100_000)?.unwrap_or(100_000) as usize;
                let lines: String = text
                    .lines()
                    .enumerate()
                    .skip(start - 1)
                    .take(count)
                    .map(|(i, l)| format!("{:>4}| {l}\n", i + 1))
                    .collect();
                Ok(json!({ "file": file, "lines": text.lines().count(), "text": lines }))
            }
            "edit" | "replace" => {
                let file = editable(self.config.task, args)?;
                let text = self.text(file)?;
                let new = if name == "edit" { edit_lines(&text, args)? } else { replace_once(&text, args)? };
                if new.len() > MAX_FILE {
                    return Err(format!("a file holds at most {MAX_FILE} bytes"));
                }
                self.write_text(file, &new)?;
                self.save()?;
                Ok(json!({ "file": file, "lines": new.lines().count() }))
            }
            "check" => {
                let task = self.config.task;
                let function = args.spec_name("function", task)?;
                let spec = task.spec_named(function).ok_or("not a function of the task")?;
                let version = task.version_of(&self.text(TASKS)?);
                let library = Library::parse(self.modules()?.iter().map(|(m, t)| (m.as_str(), t.as_str())));
                let result = task.check(&library, spec.name, version)?;
                Ok(json!({ "function": spec.name, "ok": result.ok(), "details": result.details() }))
            }
            other => Err(format!("no tool {}", crate::args::short(other))),
        }
    }

    /// The module texts, as `(module, text)` pairs.
    fn modules(&mut self) -> Result<Vec<(String, String)>, String> {
        let task = self.config.task;
        task.modules().iter().zip(task.module_files()).map(|(m, f)| Ok((m.to_string(), self.text(f)?))).collect()
    }
}

impl Store for CrdtStore {
    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String> {
        match hub_export {
            None => self.setup_hub(),
            Some(export) => self.setup_copy(export),
        }
    }

    fn export(&mut self) -> Result<Vec<u8>, String> {
        self.with_doc(wire::save)?
    }

    fn import(&mut self, _bytes: &[u8]) -> Result<Value, String> {
        Err("CRDT mode moves data only through sync".into())
    }

    fn sync_client(&mut self, socket: &Path, _relay: &Path) -> Result<Value, String> {
        self.sync(socket, &[], false)
    }

    fn sync_server(&mut self, socket: &Path, session: &Session) -> Result<Value, String> {
        // The hub keeps changes only by the agent at the other end.
        let peer = session.peers.get(session.agent).ok_or("the session's agent has no key")?;
        self.sync(socket, std::slice::from_ref(peer), true)
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let args = Args::new(args, allowed(name))?;
        self.apply(name, &args)
    }

    fn release_changes(&mut self, wave: usize) -> Result<Value, String> {
        let task = self.config.task;
        self.write_text(TASKS, &task.tasks_md(task.after_wave(wave)))?;
        self.save()?;
        Ok(json!({ "released": task.wave_changes(wave).map(|c| change_tag(c.change)).collect::<Vec<_>>() }))
    }

    fn integrated(&mut self) -> Result<Vec<(String, String)>, String> {
        self.modules()
    }
}

/// The readable file an argument names, from the fixed list: never a path.
fn readable(task: Task, args: &Args) -> Result<&'static str, String> {
    let v = args.text("file")?;
    files(task).into_iter().find(|f| *f == v).ok_or_else(|| "no such file".to_string())
}

/// An editable file: a module file or the shared notes, never `TASKS.md`.
fn editable(task: Task, args: &Args) -> Result<&'static str, String> {
    let v = args.text("file")?;
    editable_files(task)
        .into_iter()
        .find(|f| *f == v)
        .ok_or_else(|| "only the module files and NOTES.md can be edited".to_string())
}

fn edit_lines(text: &str, args: &Args) -> Result<String, String> {
    let start = args.int_opt("start", 1..=100_000)?.ok_or("`start` is required")? as usize;
    let end = args.int_opt("end", 0..=100_000)?.ok_or("`end` is required")? as usize;
    let new = args.text("text")?;
    let lines: Vec<&str> = text.lines().collect();
    if end + 1 < start || end > lines.len() || start > lines.len() + 1 {
        return Err(format!("the file has {} lines", lines.len()));
    }
    let mut out: Vec<&str> = lines[..start - 1].to_vec();
    out.extend(new.lines());
    out.extend(&lines[end.max(start - 1)..]);
    Ok(out.iter().map(|l| format!("{l}\n")).collect())
}

fn replace_once(text: &str, args: &Args) -> Result<String, String> {
    let (old, new) = (args.text("old")?, args.text("new")?);
    if old.is_empty() {
        return Err("old is empty".into());
    }
    match text.matches(old).count() {
        1 => Ok(text.replacen(old, new, 1)),
        0 => Err("old isn't in the file".into()),
        n => Err(format!("old is in the file {n} times; give more of it")),
    }
}

#[cfg(test)]
mod tests;
