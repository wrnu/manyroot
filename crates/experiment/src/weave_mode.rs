//! Weave mode (`docs/design/eval-suite.md`, task `e4970250b3e4`): agents build an evaluation project's tree
//! together through Weave, Bracken's source trees.
//!
//! The hub is a throwaway Weave Ground under the run directory, never `~/bracken-weave`. The setter imports
//! the project's seed as `main`'s first decision, so the seed is the run's first tree, states the spec and the
//! items released at the start as claims, and names every agent an integrator: the configuration's
//! integrator setting, any agent (answer `07959d6d9bc0`), recorded as a decision claim the line's policy cites.
//! Each agent's copy is a replica of the hub's Ground, synced as Bracken mode syncs, beside a git working
//! tree that only the store touches: Weave reads git's view of a tree. An agent edits files there, runs the
//! public tests (the driver runs them, under the Python profile), proposes its tree against the decision it
//! started from, and integrates proposals onto the line.
//!
//! The project's files reach the hub from the driver ([`hub_data`]), so no worker reads the project directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::args::{Args, check_id, short};
use crate::mode::{Mode, Session, Store, StoreConfig, ToolSpec, is_within};
use crate::project::{Project, Release};
use crate::task::Task;

/// The integrator setting of this first configuration: any agent may integrate.
pub const ANY_AGENT: &str = "any-agent";

/// The largest file an agent may write, in bytes.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// The most files an agent's tree may hold.
pub const MAX_FILES: usize = 2000;

/// How long a sync session may run, in seconds, as Bracken mode's.
const SYNC_DEADLINE: u32 = 60;

/// The most bytes one sync session may move: a tree's every version, with room.
const SYNC_BUDGET: u64 = 64 * 1024 * 1024;

pub struct WeaveMode;

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    }
}

impl Mode for WeaveMode {
    fn name(&self) -> &'static str {
        "weave"
    }

    fn tools(&self, _task: Task) -> Vec<ToolSpec> {
        let path =
            json!({"type": "string", "description": "A path in the tree, relative to its root, such as pkg/mod.py"});
        let id =
            json!({"type": "string", "description": "An id, in full or by a unique prefix of at least 8 hex digits"});
        vec![
            tool("task", "The project's spec and the work items released so far", json!({}), &[]),
            tool("list_files", "Every file in your working tree, with its size in bytes", json!({}), &[]),
            tool(
                "read_file",
                "A file of your working tree, with line numbers; from line `offset` (1 is the first), at most `limit` lines",
                json!({"path": path, "offset": {"type": "integer", "minimum": 1}, "limit": {"type": "integer", "minimum": 1}}),
                &["path"],
            ),
            tool(
                "write_file",
                "Write a whole file in your working tree, creating it and its directories if need be",
                json!({"path": path, "text": {"type": "string"}}),
                &["path", "text"],
            ),
            tool(
                "edit_file",
                "Replace the one occurrence of `old` in a file of your working tree with `new`; refused unless `old` occurs exactly once",
                json!({"path": path, "old": {"type": "string"}, "new": {"type": "string"}}),
                &["path", "old", "new"],
            ),
            tool("delete_file", "Delete a file of your working tree", json!({"path": path}), &["path"]),
            tool(
                "run_tests",
                "Run the project's public tests on your working tree: each test's outcome, and the output's end",
                json!({}),
                &[],
            ),
            tool(
                "changes",
                "Your working tree against the decision it started from (proposed or not): each file changed, and the diff",
                json!({}),
                &[],
            ),
            tool(
                "head",
                "The line's head (every head, if two integrations forked it), the decision your tree started from, and whether you have edits not yet proposed",
                json!({}),
                &[],
            ),
            tool(
                "update",
                "Replace your working tree with the line's head, or with `decision`, one of its heads; refused while you have edits not yet proposed",
                json!({"decision": id}),
                &[],
            ),
            tool(
                "propose",
                "Propose your working tree to the line, against the decision it started from, with a summary of what it does",
                json!({"summary": {"type": "string"}}),
                &["summary"],
            ),
            tool("proposals", "The line's open proposals, with their authors", json!({}), &[]),
            tool(
                "diff",
                "The per-file difference between two decisions or manifests, with a unified diff of each changed file",
                json!({"a": id, "b": id}),
                &["a", "b"],
            ),
            tool(
                "merge",
                "On a forked line: make your working tree every head merged, file by file against where they parted. Each file the heads changed differently is listed as a conflict, left with conflict markers (<<<<<<< and >>>>>>>) where it is text; settle each with resolve, giving no proposals, then integrate with no proposals. Refused while you have edits not yet proposed",
                json!({}),
                &[],
            ),
            tool(
                "integrate",
                "Write the line's next decision: merge `proposals`, in order, onto its head (refused if one conflicts where no resolution settles it, and the conflicts listed); or, with none given, record your working tree: only if it started from the line's one head, or, on a forked line, is your merge of every head with no conflict markers left, which settles the fork",
                json!({"proposals": {"type": "array", "items": id}, "note": {"type": "string"}}),
                &["note"],
            ),
            tool(
                "resolve",
                "Settle a conflict at `path`. With `proposals`, one that integrating them (in that order) stopped at: `take` the line's side (into), the proposal's (from), your working tree's file at that path (tree), or delete it (delete); then integrate the same proposals. With none, one your merge reported: keep your working tree's file as you've settled it (tree, refused while it holds conflict markers) or delete it (delete)",
                json!({
                    "proposals": {"type": "array", "items": id},
                    "path": path,
                    "take": {"type": "string", "enum": ["into", "from", "tree", "delete"]},
                }),
                &["path", "take"],
            ),
        ]
    }

    fn open(&self, config: StoreConfig) -> Box<dyn Store> {
        Box::new(WeaveStore { config, runs: 0 })
    }
}

/// The share of the run's agent-turns after which the items not released at the start go out, given each
/// item's share (0 at the start): a project releases at the start, and at most once more, a change (owner
/// decision `de365aae05fd`). None if every item goes out at the start.
pub fn change_share(shares: &[f64]) -> Result<Option<f64>, String> {
    let mut later: Vec<f64> = shares.iter().copied().filter(|s| *s > 0.0).collect();
    later.sort_by(f64::total_cmp);
    later.dedup();
    match later.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err("a project releases its items at the start and at most once more, at one share".into()),
    }
}

/// The agent-turns used before the change goes out, at `share` of all of a run's agent-turns (`agents`
/// agents playing `turns` turns each), rounded up so it never goes out early. The product is first rounded to
/// 1e-9, so binary rounding never makes a whole product, such as 0.1 x 3 x 10, go out an agent-turn late
/// (finding `d8167e497c9d`): project.json's shares are short decimals.
pub fn release_after(share: f64, turns: u32, agents: usize) -> u32 {
    let product = share * f64::from(turns) * agents as f64;
    ((product * 1e9).round() / 1e9).ceil() as u32
}

fn item_shares(project: &Project) -> Vec<f64> {
    project
        .items
        .iter()
        .map(|i| match i.release {
            Release::Start => 0.0,
            Release::AfterShare(at) => at,
        })
        .collect()
}

/// What the hub is given of `project`, so its worker never reads the project directory: the spec, the seed's
/// files (hex), each item with its text and whether it goes out later than the start, and the integrator
/// setting. A run that holds the change back (`hold`) gives the hub no later item at all.
pub fn hub_data(project: &Project, integrator: &str, hold: bool) -> Result<Value, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let text = |p: &Path| String::from_utf8(read(p)?).map_err(|_| format!("{} isn't UTF-8", p.display()));
    let change = change_share(&item_shares(project))?.filter(|_| !hold);
    let mut seed = Vec::new();
    for rel in project.seed_files()? {
        seed.push(json!([rel, hex::encode(read(&project.seed.join(&rel))?)]));
    }
    let mut items = Vec::new();
    for item in project.items.iter().filter(|i| !hold || i.release == Release::Start) {
        items.push(json!({
            "id": item.id,
            "kind": item.kind.name(),
            "title": item.title,
            "text": text(&item.text)?,
            "later": item.release != Release::Start,
            "revises": item.revises,
        }));
    }
    Ok(json!({
        "name": project.name,
        "spec": text(&project.spec)?,
        "seed": seed,
        "items": items,
        "change": change.is_some(),
        "integrator": integrator,
    }))
}

/// Whether `path` is one an agent may name: a plain relative path of UTF-8 components, none empty, `.` or
/// `..`, none starting with `.git` or `.bracken`, at most 255 bytes a component and 1024 in all.
pub fn plain_path(path: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("path {}: {why}", short(path)));
    if path.is_empty() || path.len() > 1024 {
        return bad("give 1 to 1024 bytes");
    }
    if path.starts_with('/') || path.contains('\\') || path.chars().any(char::is_control) {
        return bad("give a plain relative path, with / between its parts");
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.len() > 255 {
            return bad("no empty, . or .. part, and none over 255 bytes");
        }
        let lower = part.to_ascii_lowercase();
        if lower.starts_with(".git") || lower.starts_with(".bracken") {
            return bad("no part may start with .git or .bracken");
        }
    }
    Ok(())
}

/// A decision, by id or a unique prefix of at least 8 hex digits, or a manifest, as `blake3:<64 hex digits>`.
fn reference(v: &str) -> Result<&str, String> {
    match v.strip_prefix("blake3:") {
        Some(hex) if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) => Ok(v),
        Some(_) => Err(format!("{} is not a manifest (blake3: and 64 lowercase hex digits)", short(v))),
        None => check_id(v).map(|()| v),
    }
}

pub struct WeaveStore {
    config: StoreConfig,
    /// bracken and git processes started.
    pub runs: u64,
}

impl WeaveStore {
    fn replica(&self) -> PathBuf {
        self.config.dir.join(".bracken")
    }

    fn tree(&self) -> PathBuf {
        self.config.dir.join("tree")
    }

    /// `bracken --dir <own replica> <argv>` in the store's directory, with only `PATH` set, to the real git's
    /// directory: Weave reads a tree through git.
    fn bracken(&mut self, argv: &[String]) -> Result<Vec<u8>, String> {
        self.runs += 1;
        let path = self.config.git.parent().map(Path::to_path_buf).unwrap_or_default();
        let out = Command::new(&self.config.bracken)
            .arg("--dir")
            .arg(self.replica())
            .args(argv)
            .env_clear()
            .env("PATH", path)
            .current_dir(&self.config.dir)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("running bracken: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bracken {}: {}", argv.first().map_or("", String::as_str), short(err.trim())))
        }
    }

    fn bracken_json(&mut self, argv: &[&str]) -> Result<Value, String> {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let out = self.bracken(&argv)?;
        serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))
    }

    /// git on the working tree, with no configuration but its own, no hooks and no fsmonitor.
    fn git(&mut self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = self.git_output(args)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(format!("git {}: {}", args.first().unwrap_or(&""), short(String::from_utf8_lossy(&out.stderr).trim())))
        }
    }

    /// [`WeaveStore::git`]'s process, whatever its exit.
    fn git_output(&mut self, args: &[&str]) -> Result<std::process::Output, String> {
        self.runs += 1;
        Command::new(&self.config.git)
            .args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", "-c", "user.name=weave"])
            .args(["-c", "user.email=weave@invalid", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .arg("-C")
            .arg(self.tree())
            .args(args)
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("running git: {e}"))
    }

    /// git's one-line answer, trimmed.
    fn git_line(&mut self, args: &[&str]) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.git(args)?).trim().to_string())
    }

    /// The tree now starts from decision `id`: `base.txt` names it, and git's `base` tag marks its commit, which
    /// `changes` diffs against.
    fn set_base(&mut self, id: &str) -> Result<(), String> {
        self.git(&["tag", "-f", "base"])?;
        self.save("base.txt", id)?;
        self.clear_merge()
    }

    /// Forgets a merge of the heads: its heads and its conflicted paths.
    fn clear_merge(&mut self) -> Result<(), String> {
        for file in ["merge.txt", "conflicts.txt"] {
            let path = self.config.dir.join(file);
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| format!("removing {file}: {e}"))?;
            }
        }
        Ok(())
    }

    /// Makes the working tree a git repository holding what it holds now, as one commit.
    fn git_start(&mut self, message: &str) -> Result<(), String> {
        self.git(&["init", "-q", "-b", "main"])?;
        self.git_commit_all(message)
    }

    fn git_commit_all(&mut self, message: &str) -> Result<(), String> {
        self.git(&["add", "-A"])?;
        self.git(&["commit", "-q", "--allow-empty", "--no-verify", "-m", message])?;
        Ok(())
    }

    /// Whether the working tree has edits since its last commit: since its start, or its last proposal.
    fn dirty(&mut self) -> Result<bool, String> {
        Ok(!self.git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty())
    }

    fn saved(&self, file: &str) -> Result<String, String> {
        std::fs::read_to_string(self.config.dir.join(file))
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("no {file}: {e}"))
    }

    fn save(&self, file: &str, value: &str) -> Result<(), String> {
        std::fs::write(self.config.dir.join(file), value).map_err(|e| format!("writing {file}: {e}"))
    }

    /// A claim by this store's identity, by its id.
    fn claim(&mut self, tag: &str, text: &str) -> Result<String, String> {
        let me = self.config.identity.clone();
        let argv = vec![
            "claim".into(),
            format!("--as={me}"),
            format!("--tag={tag}"),
            "--json".into(),
            "--".into(),
            text.into(),
        ];
        let out = self.bracken(&argv)?;
        let v: Value = serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))?;
        v["id"].as_str().map(str::to_string).ok_or_else(|| "bracken printed no id".into())
    }

    /// The line's heads, as `weave head --json` gives them.
    fn heads(&mut self) -> Result<Value, String> {
        self.bracken_json(&["weave", "head", "--json"])
    }

    /// Writes the manifest `manifest` as the working tree, keeping its git repository, and commits it.
    fn checkout(&mut self, manifest: &str, message: &str) -> Result<(), String> {
        let dir = self.config.dir.clone();
        let fresh = dir.join("tree.new");
        // Nothing below is cleared while it holds the only copy of the agent's files or repository. After this,
        // a missing tree means none was ever placed (the copy's setup), so tree.new holds nothing of the agent's.
        self.recover_tree()?;
        if fresh.exists() {
            std::fs::remove_dir_all(&fresh).map_err(|e| format!("clearing {}: {e}", fresh.display()))?;
        }
        // Earlier checkouts' old trees, those that couldn't be removed then: each has a name of its own, so one
        // that still can't be is no hindrance.
        self.sweep_old_trees();
        let old = (0u32..).map(|k| dir.join(format!("tree.old.{k}"))).find(|p| !p.exists()).expect("a free name");
        // Checkout writes into an empty directory.
        std::fs::create_dir(&fresh).map_err(|e| format!("creating {}: {e}", fresh.display()))?;
        let target = fresh.to_string_lossy().into_owned();
        self.bracken(&["weave".into(), "checkout".into(), format!("--manifest={manifest}"), target])?;
        let tree = self.tree();
        let kept = tree.join(".git").exists();
        if kept {
            std::fs::rename(tree.join(".git"), fresh.join(".git"))
                .map_err(|e| format!("keeping git's repository: {e}"))?;
        }
        // The old tree is set aside whole, not cleared in place: something can write into it as it's cleared, and
        // the clear then fails (os error 66, task 61c34b3c2704). A rename doesn't.
        let swapped = if tree.exists() {
            std::fs::rename(&tree, &old).map_err(|e| format!("setting the old tree aside: {e}"))
        } else {
            Ok(())
        }
        .and_then(|()| self.place(&fresh, &tree));
        // A swap that fails puts the old tree and git's repository back, so the tree is still one, and git can
        // restore its files: else every later git call there fails as not a repository (task 898fff65aaef).
        if let Err(e) = swapped {
            let back = (if tree.exists() || !old.exists() { Ok(()) } else { std::fs::rename(&old, &tree) })
                .and_then(|()| std::fs::create_dir_all(&tree))
                .and_then(|()| if kept { std::fs::rename(fresh.join(".git"), tree.join(".git")) } else { Ok(()) });
            if let Err(b) = back {
                return Err(format!("{e}; and putting the tree back: {b}"));
            }
            return Err(e);
        }
        // What can't be removed now, the next checkout tries again.
        let _ = std::fs::remove_dir_all(&old);
        if tree.join(".git").exists() { self.git_commit_all(message) } else { self.git_start(message) }
    }

    /// Renames the checked-out `fresh` to be the tree. A debug build fails it while the store's directory holds
    /// a file `fail-placing-the-tree`, so a test can reach the put-back after it (task 43ca0a00d4b9): nothing
    /// else makes this one rename fail, a root user's included. A release build never reads that file.
    ///
    /// A debug build likewise makes a tree directory, with a file in it, before the rename while it holds a file
    /// `recreate-the-tree`: what an unknown writer may do between the set-aside and the placing (task
    /// af9ef395181b), and the rename then fails as it would.
    ///
    /// These hooks are live in debug builds only, and not behind a cargo feature: a feature only the tests turn
    /// on needs the crate as its own dev-dependency, which changes Cargo.lock and so makes every head that
    /// touches this need the full table (decision on task af9ef395181b). Only the harness or an operator can make
    /// the files, in the store's directory, which no agent's tool reaches.
    fn place(&self, fresh: &Path, tree: &Path) -> Result<(), String> {
        if cfg!(debug_assertions) && self.config.dir.join("fail-placing-the-tree").exists() {
            return Err("placing the tree: failed for a test (fail-placing-the-tree)".into());
        }
        if cfg!(debug_assertions) && self.config.dir.join("recreate-the-tree").exists() {
            std::fs::create_dir_all(tree)
                .and_then(|()| std::fs::write(tree.join("recreated"), "a writer's"))
                .map_err(|e| format!("recreating the tree for a test: {e}"))?;
        }
        std::fs::rename(fresh, tree).map_err(|e| format!("placing the tree: {e}"))
    }

    /// Puts back what a checkout whose swap and put-back both failed left elsewhere (task 43ca0a00d4b9):
    /// the agent's files in the one tree.old.<n> while the tree is missing, and git's repository in tree.new
    /// while the tree has none. Where that isn't one clear move, it refuses, naming what it found, and removes
    /// nothing: no later clear of tree.new or tree.old.* may take the only copy.
    fn recover_tree(&self) -> Result<(), String> {
        let dir = &self.config.dir;
        let (tree, fresh) = (self.tree(), dir.join("tree.new"));
        if tree.exists() && !tree.join(".git").exists() {
            // Something made the tree again after it was set aside (task af9ef395181b): the repository in an old
            // tree is the agent's, and which files are its is unclear.
            if let Some(held) = self.old_trees().into_iter().find(|o| o.join(".git").exists()) {
                return Err(format!(
                    "{} has no git repository, while {} holds one, left by a failed checkout: neither is moved or \
                     removed",
                    tree.display(),
                    held.display()
                ));
            }
        }
        if !tree.exists() {
            let old = self.old_trees();
            match old.as_slice() {
                [] => {}
                [one] => std::fs::rename(one, &tree)
                    .map_err(|e| format!("the tree is missing and {} holds it: putting it back: {e}", one.display()))?,
                many => {
                    let names: Vec<String> = many.iter().map(|p| p.display().to_string()).collect();
                    return Err(format!(
                        "the tree is missing, and {} old trees are left ({}): which was the tree is unclear, so \
                         none is moved or removed",
                        many.len(),
                        names.join(", ")
                    ));
                }
            }
        }
        if fresh.join(".git").exists() {
            if tree.join(".git").exists() {
                return Err(format!(
                    "both {} and {} hold a git repository, left by a failed checkout: neither is removed",
                    tree.join(".git").display(),
                    fresh.join(".git").display()
                ));
            }
            std::fs::create_dir_all(&tree)
                .and_then(|()| std::fs::rename(fresh.join(".git"), tree.join(".git")))
                .map_err(|e| format!("{} holds git's repository: putting it back: {e}", fresh.display()))?;
        }
        Ok(())
    }

    /// Earlier checkouts' old trees, tree.old.<n>, in name order.
    fn old_trees(&self) -> Vec<PathBuf> {
        let mut old: Vec<PathBuf> = std::fs::read_dir(&self.config.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("tree.old."))
            .map(|e| e.path())
            .collect();
        old.sort();
        old
    }

    /// Removes earlier checkouts' old trees, as far as it can, but only while the tree is a working checkout, its
    /// repository's files exactly: only then is an old tree surely no copy of the agent's (task af9ef395181b).
    fn sweep_old_trees(&mut self) {
        if self.working_checkout() {
            for old in self.old_trees() {
                let _ = std::fs::remove_dir_all(old);
            }
        }
    }

    /// Whether the tree is a working checkout: it has its repository, and git status runs there and finds the
    /// files exactly the repository's, so nothing of the agent's is anywhere else.
    fn working_checkout(&mut self) -> bool {
        self.tree().join(".git").is_dir() && self.dirty().is_ok_and(|d| !d)
    }

    /// A path in the working tree, checked: plain, and with no symlink on the way.
    fn file(&self, path: &str) -> Result<PathBuf, String> {
        plain_path(path)?;
        let tree = self.tree();
        let mut at = tree.clone();
        for part in path.split('/') {
            at.push(part);
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(format!("path {}: a symlink is on the way", short(path)));
            }
        }
        Ok(at)
    }

    /// Every file of the working tree, outside `.git`, as `(path, bytes)`, sorted.
    fn files(&self) -> Result<Vec<(String, u64)>, String> {
        let mut out = Vec::new();
        let mut todo = vec![(self.tree(), String::new())];
        while let Some((dir, rel)) = todo.pop() {
            for entry in std::fs::read_dir(&dir).map_err(|e| format!("listing {}: {e}", dir.display()))? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if rel.is_empty() && name == ".git" {
                    continue;
                }
                let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                let meta = entry.metadata().map_err(|e| e.to_string())?;
                if meta.is_dir() {
                    todo.push((entry.path(), path));
                } else if meta.is_file() {
                    out.push((path, meta.len()));
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn setup_hub(&mut self) -> Result<Value, String> {
        let data: Value = serde_json::from_str(&self.saved("project.json")?).map_err(|e| e.to_string())?;
        let me = self.config.identity.clone();
        let init =
            self.bracken_json(&["weave", "init", "--purpose=evaluation run", &format!("--as={me}"), "--json"])?;
        let setter = self.bracken_json(&["id", "list", "--json"])?;
        let key = setter
            .as_array()
            .and_then(|ids| ids.iter().find(|i| i["name"] == me.as_str()))
            .and_then(|i| i["agent"].as_str())
            .ok_or("the setter has no key")?
            .to_string();
        self.save("key.txt", &key)?;
        self.save("setter.txt", &key)?;
        let tree = self.tree();
        std::fs::create_dir_all(&tree).map_err(|e| format!("creating the tree: {e}"))?;
        for pair in data["seed"].as_array().ok_or("the project data has no seed")? {
            let rel = pair[0].as_str().ok_or("a seed path isn't text")?;
            plain_path(rel)?;
            let bytes = hex::decode(pair[1].as_str().unwrap_or_default()).map_err(|e| e.to_string())?;
            let path = tree.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, bytes).map_err(|e| format!("writing {rel}: {e}"))?;
        }
        self.git_start("the seed")?;
        let repo = tree.to_string_lossy().into_owned();
        let seed = self.bracken_json(&[
            "weave",
            "integrate",
            "--from-git=HEAD",
            &format!("--repo={repo}"),
            "--note=the project's seed",
            &format!("--as={me}"),
            "--json",
        ])?;
        let spec = self.claim("spec", data["spec"].as_str().ok_or("the project data has no spec")?)?;
        let integrator = data["integrator"].as_str().unwrap_or(ANY_AGENT).to_string();
        let decision =
            self.claim("decision", &format!("Run configuration: the integrator setting is {integrator}: every agent of the run is an integrator of main."))?;
        self.save("configuration.txt", &decision)?;
        let released = self.state_items(&data, false)?;
        Ok(
            json!({"line": init["line"], "seed": seed["decision"], "spec": spec, "configuration": decision, "released": released}),
        )
    }

    /// The hub states the items released at the start, or `later`, as claims tagged `item`, one each.
    fn state_items(&mut self, data: &Value, later: bool) -> Result<Vec<String>, String> {
        let mut released = Vec::new();
        for item in data["items"].as_array().into_iter().flatten().filter(|i| i["later"] == later) {
            let id = item["id"].as_str().unwrap_or_default();
            let revises: Vec<&str> =
                item["revises"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let mut text = format!(
                "Item {id} ({}): {}\n\n{}",
                item["kind"].as_str().unwrap_or_default(),
                item["title"].as_str().unwrap_or_default(),
                item["text"].as_str().unwrap_or_default()
            );
            if !revises.is_empty() {
                text.push_str(&format!("\n\nThis changes the requirement of: {}.", revises.join(", ")));
            }
            self.claim("item", &text)?;
            released.push(id.to_string());
        }
        Ok(released)
    }

    fn setup_copy(&mut self, bundle: &[u8]) -> Result<Value, String> {
        let path = self.config.dir.join("join.mrb");
        std::fs::write(&path, bundle).map_err(|e| format!("writing the bundle: {e}"))?;
        self.bracken(&["join".into(), "--".into(), path.to_string_lossy().into_owned()])?;
        let key = self.bracken(&["id".into(), "new".into(), "--".into(), self.config.identity.clone()])?;
        let key = String::from_utf8_lossy(&key).trim().to_string();
        self.save("key.txt", &key)?;
        // The setter's key, from the hub's own Ground as joined, before any agent has written to it: the seed,
        // the line's first decision, is the setter's.
        let log = self.bracken_json(&["weave", "log", "--json", "--limit=100000"])?;
        let seed = log["decisions"]
            .as_array()
            .and_then(|d| d.iter().find(|d| d["parents"].as_array().is_some_and(Vec::is_empty)));
        let setter = seed.and_then(|d| d["author"].as_str()).ok_or("the hub's line has no seed decision")?;
        self.save("setter.txt", setter)?;
        let heads = self.heads()?;
        let head = &heads["heads"][0];
        let manifest = head["manifest"].as_str().ok_or("the hub's line has no head")?.to_string();
        self.checkout(&manifest, "the line's head")?;
        self.set_base(head["decision"].as_str().unwrap_or_default())?;
        Ok(json!({"key": key}))
    }

    /// The spec and the items released so far: only claims the setter's key signed (finding `8c2713b55062`),
    /// so no claim another author tags `spec` or `item` reaches an agent as its task.
    fn task(&mut self) -> Result<Value, String> {
        let setter = self.saved("setter.txt")?;
        let log = |s: &mut WeaveStore, tag: &str| s.bracken_json(&["log", "--json", &format!("--tag={tag}")]);
        let texts = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["author"] == setter.as_str())
                .filter_map(|e| e["body"]["statement"].as_str().map(str::to_string))
                .collect()
        };
        let spec = texts(&log(self, "spec")?);
        let items = texts(&log(self, "item")?);
        Ok(json!({"spec": spec.first(), "items": items}))
    }

    fn read_file(&mut self, args: &Args) -> Result<Value, String> {
        let path = args.text("path")?;
        let file = self.file(path)?;
        let bytes = std::fs::read(&file).map_err(|e| format!("reading {}: {}", short(path), e.kind()))?;
        let text = String::from_utf8(bytes).map_err(|_| format!("{} isn't UTF-8 text", short(path)))?;
        let from = args.int_opt("offset", 1..=i64::from(u32::MAX))?.unwrap_or(1) as usize;
        let limit = args.int_opt("limit", 1..=i64::from(u32::MAX))?.map_or(usize::MAX, |l| l as usize);
        let lines: Vec<&str> = text.lines().collect();
        let shown: Vec<String> =
            lines.iter().enumerate().skip(from - 1).take(limit).map(|(i, l)| format!("{:>5}  {l}", i + 1)).collect();
        Ok(json!({"path": path, "lines": lines.len(), "from": from, "text": shown.join("\n")}))
    }

    fn write(&mut self, path: &str, text: &str) -> Result<Value, String> {
        if text.len() > MAX_FILE_BYTES {
            return Err(format!("a file holds at most {MAX_FILE_BYTES} bytes"));
        }
        let file = self.file(path)?;
        if !file.exists() && self.files()?.len() >= MAX_FILES {
            return Err(format!("a tree holds at most {MAX_FILES} files"));
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}'s directory: {}", short(path), e.kind()))?;
        }
        if !is_within(&file, &self.tree()) {
            return Err(format!("path {}: outside the tree", short(path)));
        }
        if std::fs::symlink_metadata(&file).is_ok_and(|m| !m.is_file()) {
            return Err(format!("path {}: not a file", short(path)));
        }
        std::fs::write(&file, text).map_err(|e| format!("writing {}: {}", short(path), e.kind()))?;
        Ok(json!({"path": path, "bytes": text.len()}))
    }

    fn edit_file(&mut self, args: &Args) -> Result<Value, String> {
        let (path, old, new) =
            (args.text("path")?, args.text_up_to("old", MAX_FILE_BYTES)?, args.text_up_to("new", MAX_FILE_BYTES)?);
        if old.is_empty() {
            return Err("`old` is empty: give the text to replace".into());
        }
        let file = self.file(path)?;
        let text = std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {}", short(path), e.kind()))?;
        match text.matches(old).count() {
            1 => {}
            0 => return Err(format!("{} doesn't hold that text", short(path))),
            n => return Err(format!("{} holds that text {n} times: give more of it, so it occurs once", short(path))),
        }
        self.write(path, &text.replacen(old, new, 1))
    }

    fn delete_file(&mut self, path: &str) -> Result<Value, String> {
        let file = self.file(path)?;
        let meta = std::fs::symlink_metadata(&file).map_err(|_| format!("no file {}", short(path)))?;
        if !meta.is_file() {
            return Err(format!("{} isn't a file", short(path)));
        }
        std::fs::remove_file(&file).map_err(|e| format!("deleting {}: {}", short(path), e.kind()))?;
        Ok(json!({"deleted": path}))
    }

    /// The working tree against its base's commit, so a proposal doesn't hide what it proposed (finding
    /// `24bae8fdb48a`).
    fn changes(&mut self) -> Result<Value, String> {
        self.git(&["add", "-A"])?;
        let files = self
            .git(&["diff", "--cached", "--name-status", "refs/tags/base", "--"])
            .map(|o| String::from_utf8_lossy(&o).into_owned());
        let diff = self
            .git(&["diff", "--cached", "--no-color", "--no-ext-diff", "refs/tags/base", "--"])
            .map(|o| String::from_utf8_lossy(&o).into_owned());
        self.git(&["reset", "-q"])?;
        let (files, diff) = (files?, diff?);
        Ok(json!({"base": self.saved("base.txt")?, "files": files.lines().collect::<Vec<_>>(), "diff": diff}))
    }

    fn head(&mut self) -> Result<Value, String> {
        let heads = self.heads()?;
        Ok(json!({
            "forked": heads["forked"],
            "heads": heads["heads"],
            "base": self.saved("base.txt")?,
            "unproposed_edits": self.dirty()?,
        }))
    }

    fn update(&mut self, decision: Option<&str>) -> Result<Value, String> {
        if self.dirty()? {
            return Err("you have edits not yet proposed: propose them first, or undo them".into());
        }
        let heads = self.heads()?;
        let list = heads["heads"].as_array().cloned().unwrap_or_default();
        let head = match decision {
            Some(d) => list.iter().find(|h| h["decision"].as_str().is_some_and(|id| id.starts_with(d))).cloned(),
            None if list.len() == 1 => list.first().cloned(),
            None => return Err(format!("the line is forked, with {} heads: name one as decision", list.len())),
        }
        .ok_or("that decision isn't one of the line's heads")?;
        let (id, manifest) =
            (head["decision"].as_str().unwrap_or_default(), head["manifest"].as_str().unwrap_or_default());
        let (id, manifest) = (id.to_string(), manifest.to_string());
        self.checkout(&manifest, &format!("decision {id}"))?;
        self.set_base(&id)?;
        Ok(json!({"base": id, "manifest": manifest}))
    }

    fn propose(&mut self, summary: &str) -> Result<Value, String> {
        if !self.dirty()? {
            return Err("nothing to propose: your tree is as it was at its start or your last proposal".into());
        }
        self.git(&["add", "-A"])?;
        let (base, me, repo) =
            (self.saved("base.txt")?, self.config.identity.clone(), self.tree().to_string_lossy().into_owned());
        let out = self.bracken_json(&[
            "weave",
            "propose",
            "--index",
            &format!("--repo={repo}"),
            &format!("--base={base}"),
            &format!("--summary={summary}"),
            &format!("--as={me}"),
            "--json",
        ]);
        let out = match out {
            Ok(out) => out,
            Err(e) => {
                self.git(&["reset", "-q"])?;
                return Err(e);
            }
        };
        self.git(&["commit", "-q", "--no-verify", "-m", "proposed"])?;
        Ok(json!({"proposal": out["proposal"], "base": out["base"], "files": out["files"]}))
    }

    fn proposals(&mut self) -> Result<Value, String> {
        let review = self.bracken_json(&["weave", "review", "--json"])?;
        let log = self.bracken_json(&["log", "--json", "--kind=artifact"])?;
        let author = |id: &str| {
            log.as_array()
                .into_iter()
                .flatten()
                .find(|e| e["id"] == id)
                .map(|e| e["author_name"].as_str().map_or_else(|| e["author"].clone(), |n| json!(n)))
                .unwrap_or(Value::Null)
        };
        let open: Vec<Value> = review["proposals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["proposal"].as_str())
            .map(|id| json!({"proposal": id, "author": author(id)}))
            .collect();
        Ok(json!({"proposals": open}))
    }

    fn integrate(&mut self, proposals: Vec<String>, note: &str) -> Result<Value, String> {
        let me = self.config.identity.clone();
        let mut argv = vec![
            "weave".to_string(),
            "integrate".into(),
            format!("--note={note}"),
            format!("--as={me}"),
            "--json".into(),
        ];
        if proposals.is_empty() {
            self.may_record_tree()?;
            self.git(&["add", "-A"])?;
            argv.extend(["--index".into(), format!("--repo={}", self.tree().display())]);
            let out = self.bracken(&argv);
            self.git(&["reset", "-q"])?;
            let v: Value = serde_json::from_slice(&out?).map_err(|e| format!("bracken printed bad JSON: {e}"))?;
            // The tree is now the decision's: the agent starts from it, as after an update.
            self.git_commit_all("integrated")?;
            if let Some(d) = v["decision"].as_str() {
                self.set_base(d)?;
            }
            return Ok(json!({"decision": v["decision"], "parents": v["parents"]}));
        }
        argv.push("--proposals".into());
        argv.extend(proposals);
        let out = self.bracken(&argv)?;
        let v: Value = serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))?;
        Ok(json!({"decision": v["decision"], "parents": v["parents"], "proposals": v["proposals"]}))
    }

    /// Whether integrate may record the working tree as the line's next decision, with every head a parent
    /// (finding `9f4e48c4c547`): only a tree that started from the line's one head, so it undoes nothing that
    /// came since; or, on a forked line, the agent's merge of exactly its heads, with no conflict markers left
    /// in the files that conflicted, so it keeps every head's work.
    fn may_record_tree(&mut self) -> Result<(), String> {
        let ids = head_ids(&self.heads()?);
        let base = self.saved("base.txt")?;
        match ids.as_slice() {
            [] => Err("the line has no head".into()),
            [one] if *one == base => Ok(()),
            [one] => Err(format!(
                "your tree started from {}, not the line's head {}: recording it would undo what came since. \
                 Propose it and integrate the proposal, which merges it onto the head",
                short_id(&base),
                short_id(one)
            )),
            _ => {
                if self.saved("merge.txt").ok().as_deref() != Some(ids.join("\n").as_str()) {
                    return Err(format!(
                        "the line is forked, with {} heads: merge them into your tree first (merge), settle any \
                         conflict, then integrate",
                        ids.len()
                    ));
                }
                // Each conflict is settled by resolve, so one with no markers to find, such as a binary file's,
                // never passes unsettled (finding e26a1c74e007).
                let conflicts = self.saved("conflicts.txt").unwrap_or_default();
                match conflicts.lines().find(|l| !l.is_empty()) {
                    Some(path) => Err(format!(
                        "your merge's conflict at {} isn't settled: settle it, then resolve it with no proposals",
                        short(path)
                    )),
                    None => Ok(()),
                }
            }
        }
    }

    /// The `merge` tool: every head of a forked line merged into the working tree by git's three-way merge,
    /// file by file against where the heads parted, through any earlier merge (so two agents settling the same
    /// fork is no harder). Conflicts stay in the files, marked.
    fn merge(&mut self) -> Result<Value, String> {
        if self.dirty()? {
            return Err("you have edits not yet proposed: propose them first, or undo them".into());
        }
        let heads = self.heads()?;
        let ids = head_ids(&heads);
        if ids.len() < 2 {
            return Err("the line isn't forked: there is nothing to merge; update to its head".into());
        }
        let decisions = self.decisions()?;
        let keep = self.git_line(&["rev-parse", "HEAD"])?;
        // Whatever happens after the mirror, the tree goes back to its own commit (finding e26a1c74e007).
        let merged = self.merge_heads(&decisions, &ids);
        let restored = self.git(&["reset", "-q", "--hard", &keep]);
        // The merge's own failure is the one to tell, with the restore's if that failed too: the restore's alone
        // would hide what went wrong (task 898fff65aaef).
        let (tree, conflicts) = match (merged, restored) {
            (Err(e), Err(r)) => return Err(format!("{e}; and restoring the tree: {r}")),
            (Err(e), Ok(_)) => return Err(e),
            (Ok(_), Err(r)) => return Err(r),
            (Ok(merged), Ok(_)) => merged,
        };
        let count = self
            .git(&["ls-tree", "-r", "--name-only", &tree])?
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count();
        if count > MAX_FILES {
            return Err(format!("the heads merged hold {count} files, more than a tree may ({MAX_FILES})"));
        }
        // The merged files over the tree's own commit, unstaged: edits not yet proposed.
        let applied = self.git(&["read-tree", "--reset", "-u", &tree]).and_then(|_| self.git(&["reset", "-q"]));
        if let Err(e) = applied {
            self.git(&["reset", "-q", "--hard", &keep])?;
            return Err(e);
        }
        self.save("merge.txt", &ids.join("\n"))?;
        self.save("conflicts.txt", &conflicts.join("\n"))?;
        Ok(json!({"heads": ids, "conflicts": conflicts}))
    }

    /// The heads `ids` merged in order, as git's tree id, and the paths that conflicted. Leaves the working
    /// tree wherever the mirror left it; [`WeaveStore::merge`] restores it.
    fn merge_heads(
        &mut self,
        decisions: &std::collections::BTreeMap<String, (String, Vec<String>)>,
        ids: &[String],
    ) -> Result<(String, Vec<String>), String> {
        let commits = self.mirror(decisions, ids)?;
        let mut merged = commits[0].clone();
        let mut conflicts = std::collections::BTreeSet::new();
        let mut tree = String::new();
        for other in &commits[1..] {
            let out =
                self.git_output(&["merge-tree", "--write-tree", "--name-only", "--no-messages", &merged, other])?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let mut lines = text.lines();
            tree = lines.next().unwrap_or_default().trim().to_string();
            match out.status.code() {
                Some(0) => {}
                Some(1) => conflicts.extend(lines.take_while(|l| !l.is_empty()).map(str::to_string)),
                _ => return Err(format!("git merge-tree: {}", short(String::from_utf8_lossy(&out.stderr).trim()))),
            }
            merged = self.git_line(&["commit-tree", &tree, "-p", &merged, "-p", other, "-m", "merged"])?;
        }
        Ok((tree, conflicts.into_iter().collect()))
    }

    /// Each of `heads` as a git commit whose history is the line's: every decision in their past becomes a
    /// commit of its tree with its parents' commits as parents, once (a ref `refs/weave/<decision>` keeps it),
    /// so git's merge finds the heads' merge bases itself, through any earlier merge. Leaves the working tree
    /// as the last decision mirrored; the caller restores it.
    fn mirror(
        &mut self,
        decisions: &std::collections::BTreeMap<String, (String, Vec<String>)>,
        heads: &[String],
    ) -> Result<Vec<String>, String> {
        let mut commits = std::collections::BTreeMap::new();
        for id in ancestry_order(decisions, heads)? {
            let name = format!("refs/weave/{id}");
            if let Ok(c) = self.git_line(&["rev-parse", "--verify", "--quiet", &name]) {
                commits.insert(id, c);
                continue;
            }
            let (manifest, parents) = decisions.get(&id).cloned().ok_or("a decision the log doesn't hold")?;
            self.checkout(&manifest, &format!("decision {id}"))?;
            let tree = self.git_line(&["rev-parse", "HEAD^{tree}"])?;
            let mut argv = vec!["commit-tree".to_string(), tree];
            for p in &parents {
                argv.extend(["-p".into(), commits.get(p).cloned().ok_or("a parent not mirrored before its child")?]);
            }
            argv.extend(["-m".into(), format!("decision {id}")]);
            let commit = self.git_line(&argv.iter().map(String::as_str).collect::<Vec<_>>())?;
            self.git(&["update-ref", &name, &commit])?;
            commits.insert(id, commit);
        }
        heads.iter().map(|h| commits.get(h).cloned().ok_or_else(|| "a head not mirrored".to_string())).collect()
    }

    /// Every decision of the line: its manifest and parents, by id.
    fn decisions(&mut self) -> Result<std::collections::BTreeMap<String, (String, Vec<String>)>, String> {
        let log = self.bracken_json(&["weave", "log", "--json", "--limit=100000"])?;
        if log["more"] == true {
            return Err("the line has more decisions than a merge reads".into());
        }
        let mut out = std::collections::BTreeMap::new();
        for d in log["decisions"].as_array().into_iter().flatten() {
            let (Some(id), Some(manifest)) = (d["decision"].as_str(), d["manifest"].as_str()) else { continue };
            let parents = d["parents"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string);
            out.insert(id.to_string(), (manifest.to_string(), parents.collect()));
        }
        Ok(out)
    }

    /// The `resolve` tool: a resolution of the conflict at `path` that integrating `proposals` stopped at.
    fn resolve(&mut self, proposals: Vec<String>, path: &str, take: &str) -> Result<Value, String> {
        plain_path(path)?;
        if proposals.is_empty() {
            return self.settle(path, take);
        }
        let me = self.config.identity.clone();
        let mut argv = vec!["weave".to_string(), "resolve".into(), format!("--as={me}"), "--json".into()];
        match take {
            "into" | "from" => argv.push(format!("--take={take}")),
            "delete" => argv.push("--delete".into()),
            "tree" => {
                let file = self.file(path)?;
                if !std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file()) {
                    return Err(format!("no file {} in your tree", short(path)));
                }
                argv.extend([format!("--file={}", file.display()), format!("--repo={}", self.tree().display())]);
            }
            other => return Err(format!("take {}: give into, from, tree or delete", short(other))),
        }
        argv.push("--proposals".into());
        argv.extend(proposals);
        argv.extend(["--".into(), path.into()]);
        let out = self.bracken(&argv)?;
        serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))
    }

    /// Settles a conflict the agent's merge reported at `path`: its working tree's file as it stands (`tree`),
    /// with no conflict marker left, or the file deleted (`delete`).
    fn settle(&mut self, path: &str, take: &str) -> Result<Value, String> {
        let conflicts = self.saved("conflicts.txt").unwrap_or_default();
        if !conflicts.lines().any(|l| l == path) {
            return Err(format!("your merge reported no conflict at {} to settle", short(path)));
        }
        let file = self.file(path)?;
        match take {
            "tree" => {
                let bytes = std::fs::read(&file).map_err(|_| format!("no file {} in your tree", short(path)))?;
                let text = String::from_utf8_lossy(&bytes);
                if text.lines().any(|l| l.starts_with("<<<<<<<") || l.starts_with(">>>>>>>")) {
                    return Err(format!("{} still holds conflict markers: settle them first", short(path)));
                }
            }
            "delete" => {
                if std::fs::symlink_metadata(&file).is_ok() {
                    self.delete_file(path)?;
                }
            }
            other => return Err(format!("take {}: a merge's conflict takes tree or delete", short(other))),
        }
        let left: Vec<&str> = conflicts.lines().filter(|l| !l.is_empty() && *l != path).collect();
        self.save("conflicts.txt", &left.join("\n"))?;
        Ok(json!({"settled": path, "conflicts": left}))
    }
}

/// The decisions of `weave head --json`'s heads, sorted.
fn head_ids(heads: &Value) -> Vec<String> {
    let mut ids: Vec<String> = heads["heads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["decision"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// A decision id's first 12 digits.
fn short_id(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// Every decision in the past of `heads`, themselves included, each after all its parents.
pub fn ancestry_order(
    decisions: &std::collections::BTreeMap<String, (String, Vec<String>)>,
    heads: &[String],
) -> Result<Vec<String>, String> {
    use std::collections::BTreeSet;
    let (mut order, mut done, mut todo) =
        (Vec::new(), BTreeSet::new(), heads.iter().rev().map(|h| (h.clone(), false)).collect::<Vec<_>>());
    // Depth first: a decision is placed once all its parents are.
    while let Some((id, parents_placed)) = todo.pop() {
        if done.contains(&id) {
            continue;
        }
        let parents =
            &decisions.get(&id).ok_or_else(|| format!("decision {} isn't in the line's log", short_id(&id)))?.1;
        if parents_placed {
            done.insert(id.clone());
            order.push(id);
            continue;
        }
        if todo.len() > 4 * decisions.len() + heads.len() {
            return Err("the line's history has a cycle".into());
        }
        todo.push((id, true));
        todo.extend(parents.iter().filter(|p| !done.contains(*p)).map(|p| (p.clone(), false)));
    }
    Ok(order)
}

impl Store for WeaveStore {
    fn project(&mut self, data: &Value) -> Result<Value, String> {
        self.save("project.json", &data.to_string())?;
        Ok(json!({"items": data["items"].as_array().map_or(0, Vec::len)}))
    }

    fn peers(&mut self, keys: &[String]) -> Result<Value, String> {
        let me = self.config.identity.clone();
        let decision = self.saved("configuration.txt")?;
        let mut argv = vec!["weave".to_string(), "line".into(), "policy".into(), "main".into()];
        argv.push(format!("--integrator={}", self.saved("key.txt")?));
        argv.extend(keys.iter().map(|k| format!("--integrator={k}")));
        argv.extend([format!("--owner-decision={decision}"), format!("--as={me}"), "--json".into()]);
        let out = self.bracken(&argv)?;
        let v: Value = serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))?;
        Ok(json!({"policy": v["line"], "integrators": keys.len() + 1}))
    }

    /// One release after the start if the project has a change, else none.
    fn releases(&self) -> Option<usize> {
        let data: Value = serde_json::from_str(&self.saved("project.json").ok()?).ok()?;
        Some(usize::from(data["change"] == true))
    }

    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String> {
        match hub_export {
            None => self.setup_hub(),
            Some(bundle) => self.setup_copy(bundle),
        }
    }

    fn export(&mut self) -> Result<Vec<u8>, String> {
        self.bracken(&["export".into(), "-".into()])
    }

    fn import(&mut self, _bytes: &[u8]) -> Result<Value, String> {
        Err("weave mode syncs by sessions".into())
    }

    fn sync_client(&mut self, socket: &Path, relay: &Path) -> Result<Value, String> {
        self.runs += 1;
        crate::bracken_mode::sync_client(
            &self.config.bracken,
            &self.replica(),
            &self.config.dir,
            socket,
            relay,
            SYNC_DEADLINE,
            SYNC_BUDGET,
        )
    }

    fn sync_server(&mut self, socket: &Path, session: &Session) -> Result<Value, String> {
        self.runs += 1;
        crate::bracken_mode::sync_server(
            &self.config.bracken,
            &self.replica(),
            &self.config.dir,
            socket,
            session,
            SYNC_DEADLINE,
            SYNC_BUDGET,
        )
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        // What a failed checkout left aside goes back first, so the tool sees the tree and its repository, not
        // a tree that isn't one (task 43ca0a00d4b9).
        self.recover_tree()?;
        match name {
            "task" => {
                Args::new(args, &[])?;
                self.task()
            }
            "list_files" => {
                Args::new(args, &[])?;
                let files = self.files()?;
                Ok(json!({"files": files.iter().map(|(p, n)| json!({"path": p, "bytes": n})).collect::<Vec<_>>()}))
            }
            "read_file" => self.read_file(&Args::new(args, &["path", "offset", "limit"])?),
            "write_file" => {
                let a = Args::new(args, &["path", "text"])?;
                self.write(a.text("path")?, a.text_up_to("text", MAX_FILE_BYTES)?)
            }
            "edit_file" => self.edit_file(&Args::new(args, &["path", "old", "new"])?),
            "delete_file" => self.delete_file(Args::new(args, &["path"])?.text("path")?),
            "changes" => {
                Args::new(args, &[])?;
                self.changes()
            }
            "head" => {
                Args::new(args, &[])?;
                self.head()
            }
            "update" => {
                let a = Args::new(args, &["decision"])?;
                let decision = a.id_opt("decision")?.map(str::to_string);
                self.update(decision.as_deref())
            }
            "propose" => {
                let a = Args::new(args, &["summary"])?;
                self.propose(a.text("summary")?)
            }
            "proposals" => {
                Args::new(args, &[])?;
                self.proposals()
            }
            "diff" => {
                let a = Args::new(args, &["a", "b"])?;
                let (x, y) = (reference(a.text("a")?)?.to_string(), reference(a.text("b")?)?.to_string());
                let out = self.bracken(&["weave".into(), "diff".into(), "--json".into(), x, y])?;
                serde_json::from_slice(&out).map_err(|e| format!("bracken printed bad JSON: {e}"))
            }
            "integrate" => {
                let a = Args::new(args, &["proposals", "note"])?;
                let proposals = a.ids("proposals")?.into_iter().map(str::to_string).collect();
                self.integrate(proposals, a.text("note")?)
            }
            "merge" => {
                Args::new(args, &[])?;
                self.merge()
            }
            "resolve" => {
                let a = Args::new(args, &["proposals", "path", "take"])?;
                let proposals: Vec<String> = a.ids("proposals")?.into_iter().map(str::to_string).collect();
                self.resolve(proposals, a.text("path")?, a.text("take")?)
            }
            other => Err(format!("no tool {}", short(other))),
        }
    }

    fn tree_files(&mut self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let mut out = Vec::new();
        for (path, _) in self.files()? {
            let file = self.file(&path)?;
            out.push((path, std::fs::read(&file).map_err(|e| e.to_string())?));
        }
        Ok(out)
    }

    /// The one release after the start: the change, and any item going out with it.
    fn release_changes(&mut self, _release: usize) -> Result<Value, String> {
        let data: Value = serde_json::from_str(&self.saved("project.json")?).map_err(|e| e.to_string())?;
        Ok(json!({"released": self.state_items(&data, true)?}))
    }

    /// The line's heads, as decision and manifest ids, and whether it is forked: what a checkpoint's tree is.
    fn line_head(&mut self) -> Result<Value, String> {
        let heads = self.heads()?;
        Ok(json!({"forked": heads["forked"], "heads": heads["heads"]}))
    }

    /// The line's head as `(path, text)` pairs: on a forked line, its first head in `weave head`'s order.
    fn integrated(&mut self) -> Result<Vec<(String, String)>, String> {
        let heads = self.heads()?;
        let Some(manifest) = heads["heads"][0]["manifest"].as_str().map(str::to_string) else {
            return Ok(Vec::new());
        };
        let view = self.config.dir.join("view");
        if view.exists() {
            std::fs::remove_dir_all(&view).map_err(|e| format!("clearing the view: {e}"))?;
        }
        std::fs::create_dir(&view).map_err(|e| format!("creating the view: {e}"))?;
        self.bracken(&[
            "weave".into(),
            "checkout".into(),
            format!("--manifest={manifest}"),
            view.to_string_lossy().into_owned(),
        ])?;
        let mut out = Vec::new();
        let mut todo = vec![(view.clone(), String::new())];
        while let Some((dir, rel)) = todo.pop() {
            for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
                if meta.is_dir() {
                    todo.push((entry.path(), path));
                } else if meta.is_file() {
                    let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
                    out.push((path, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        std::fs::remove_dir_all(&view).map_err(|e| format!("clearing the view: {e}"))?;
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
