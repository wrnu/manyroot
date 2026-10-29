//! Mode 1, Git branches and pull requests (design section 2).
//!
//! The hub is a bare repository; each agent's copy is a clone of its `main`.
//! Agents edit the four module files on branches of their own, commit, and
//! open pull requests in a small PR system the hub keeps ([`prs`]). A PR
//! merges into `main` when CI passes on the merge result and someone other
//! than its author approved its current head (decision `6b13e672e7d4`).
//!
//! git runs from its absolute path with a cleared environment, no system or
//! global config, hooks off, and the fixed options of [`FIXED`]. Every value
//! an agent chooses lands after `--end-of-options` or inside one
//! `--option=value` argument, and free text goes on standard input. Data moves
//! between copies and the hub only as git bundles the driver carries, each
//! checked before it is fetched ([`GitStore::take_bundle`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::args::{Args, is_branch, is_commit};
use crate::bracken_mode::Programs;
use crate::lang::Library;
use crate::mode::{Mode, Session, Store, StoreConfig, SyncKind, ToolSpec, Transfer, is_within};
use crate::task::{Task, Version, change_tag};

pub mod prs;

/// The module files, in the repository's root.
/// The task, written only by the setter.
pub const TASKS: &str = "TASKS.md";
/// The largest module file an edit may leave.
const MAX_FILE: usize = 64 * 1024;
/// The most commits one push may bring that the hub doesn't have (finding `4bcfe41f2818`).
pub const MAX_NEW_COMMITS: usize = 500;
/// The most commit ids a copy says it has.
const MAX_HAVE: usize = 256;
/// Tools the agent's own copy serves.
const COPY_TOOLS: [&str; 11] =
    ["read", "edit", "replace", "status", "diff", "log", "switch", "commit", "merge", "rebase", "check"];
/// Tools the hub serves.
const PR_TOOLS: [&str; 7] = ["pr_open", "pr_list", "pr_view", "pr_comment", "pr_review", "pr_merge", "pr_close"];

pub struct GitMode;

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    }
}

impl Mode for GitMode {
    fn name(&self) -> &'static str {
        "git"
    }

    fn tools(&self, task: Task) -> Vec<ToolSpec> {
        let text = json!({"type": "string"});
        let files = task.module_files();
        let readable = json!({"type": "string", "enum": files.iter().chain([&TASKS]).collect::<Vec<_>>()});
        let module = json!({"type": "string", "enum": files});
        let line = json!({"type": "integer", "minimum": 1, "maximum": 100_000});
        let branch = json!({"type": "string", "description": "a-z, 0-9 and -, starting with a letter; not main"});
        let number = json!({"type": "integer", "minimum": 1, "maximum": 100_000});
        vec![
            tool(
                "read",
                "Read a file of your working copy, with line numbers",
                json!({"file": readable, "start": line, "count": line}),
                &["file"],
            ),
            tool(
                "edit",
                "Replace lines start..end (inclusive) of a module file with text; end = start - 1 inserts before start",
                json!({"file": module, "start": line, "end": {"type": "integer", "minimum": 0, "maximum": 100_000}, "text": text}),
                &["file", "start", "end", "text"],
            ),
            tool(
                "replace",
                "Replace the one occurrence of old with new in a module file",
                json!({"file": module, "old": text, "new": text}),
                &["file", "old", "new"],
            ),
            tool("status", "git status of your working copy", json!({}), &[]),
            tool(
                "diff",
                "Uncommitted changes, or with against=main everything your branch changes from the hub's main",
                json!({"against": {"type": "string", "enum": ["head", "main"]}}),
                &[],
            ),
            tool(
                "log",
                "Recent commits on your branch",
                json!({"count": {"type": "integer", "minimum": 1, "maximum": 50}}),
                &[],
            ),
            tool(
                "switch",
                "Switch to a branch of yours; with create, make it first from the hub's main or from an open PR's head",
                json!({"branch": branch, "create": {"type": "boolean"}, "from_pr": number}),
                &["branch"],
            ),
            tool(
                "commit",
                "Commit every change to the module files on your current branch",
                json!({"message": text}),
                &["message"],
            ),
            tool(
                "merge",
                "Merge the hub's main (as of your last sync) into your branch; conflicts are left to edit and commit. With abort, undo a merge in progress",
                json!({"abort": {"type": "boolean"}}),
                &[],
            ),
            tool("rebase", "Rebase your branch onto the hub's main; undone if it conflicts", json!({}), &[]),
            tool(
                "check",
                "Run the public cases of one function on your working copy, with calls resolved in it",
                json!({"function": text}),
                &["function"],
            ),
            tool(
                "pr_open",
                "Open a pull request for one of your branches, as of your last sync (sync pushes every branch)",
                json!({"branch": branch, "title": text, "body": text}),
                &["branch", "title"],
            ),
            tool("pr_list", "The hub's pull requests", json!({}), &[]),
            tool(
                "pr_view",
                "One pull request: its diff, CI, reviews and comments",
                json!({"number": number}),
                &["number"],
            ),
            tool(
                "pr_comment",
                "Comment on a pull request. Comments are where agents talk to each other: everyone can read them with pr_view",
                json!({"number": number, "text": text}),
                &["number", "text"],
            ),
            tool(
                "pr_review",
                "Approve a pull request's current head, or request changes",
                json!({"number": number, "verdict": {"type": "string", "enum": prs::VERDICTS}, "text": text}),
                &["number", "verdict"],
            ),
            tool(
                "pr_merge",
                "Merge a pull request into main, which only happens when someone calls this: CI must pass on the merge result, someone else must have approved its current head, and no one else's latest review may request changes; a conflicting merge is refused",
                json!({"number": number}),
                &["number"],
            ),
            tool("pr_close", "Close a pull request of yours without merging", json!({"number": number}), &["number"]),
        ]
    }

    fn hub_tools(&self) -> &'static [&'static str] {
        &PR_TOOLS
    }

    fn sync_kind(&self) -> SyncKind {
        SyncKind::Transfers
    }

    fn open(&self, config: StoreConfig) -> Box<dyn Store> {
        Box::new(GitStore::new(config))
    }
}

/// The arguments each tool accepts.
fn allowed(name: &str) -> &'static [&'static str] {
    match name {
        "read" => &["file", "start", "count"],
        "edit" => &["file", "start", "end", "text"],
        "replace" => &["file", "old", "new"],
        "diff" => &["against"],
        "log" => &["count"],
        "switch" => &["branch", "create", "from_pr"],
        "commit" => &["message"],
        "merge" => &["abort"],
        "check" => &["function"],
        "pr_open" => &["branch", "title", "body"],
        "pr_view" | "pr_merge" | "pr_close" => &["number"],
        "pr_comment" => &["number", "text"],
        "pr_review" => &["number", "verdict", "text"],
        _ => &[],
    }
}

/// Options on every git command: nothing in a repository, a working copy or the
/// environment can make git run a program, and every object fetched is checked.
const FIXED: [&str; 17] = [
    "core.fsmonitor=false",
    "core.symlinks=false",
    "core.attributesFile=/dev/null",
    "core.excludesFile=/dev/null",
    "core.autocrlf=false",
    "core.pager=cat",
    "core.editor=false",
    "protocol.allow=never",
    "protocol.file.allow=always",
    "transfer.fsckObjects=true",
    "fetch.fsckObjects=true",
    "gc.auto=0",
    "maintenance.auto=false",
    "commit.gpgsign=false",
    "init.defaultBranch=main",
    "merge.conflictStyle=merge",
    "advice.detachedHead=false",
];

/// What a git command printed.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    /// Standard output as bytes, for object contents.
    pub raw: Vec<u8>,
}

pub struct GitStore {
    pub(crate) config: StoreConfig,
    /// git processes started, and their argv, for tests.
    pub runs: Vec<Vec<String>>,
    files: u64,
    /// While a bundle is taken in: the object directory git writes to, apart from the store's own.
    quarantine: Option<PathBuf>,
    /// Commits whose whole history this store has checked: never checked again.
    checked: std::collections::BTreeSet<String>,
    /// CI results by (main, head): computed once per pair (finding `f582d00f535f`).
    pub(crate) ci_cache: BTreeMap<(String, String), prs::Ci>,
}

impl GitStore {
    pub fn new(config: StoreConfig) -> GitStore {
        GitStore {
            config,
            runs: Vec::new(),
            files: 0,
            quarantine: None,
            checked: Default::default(),
            ci_cache: BTreeMap::new(),
        }
    }

    /// The repository's own object directory.
    fn objects(&self) -> PathBuf {
        if self.bare().exists() { self.bare().join("objects") } else { self.work().join(".git").join("objects") }
    }

    fn work(&self) -> PathBuf {
        self.config.dir.join("work")
    }

    fn bare(&self) -> PathBuf {
        self.config.dir.join("hub.git")
    }

    /// The repository this store holds: the hub's bare one, or the copy's working one.
    fn repo(&self) -> PathBuf {
        if self.bare().exists() { self.bare() } else { self.work() }
    }

    /// Runs git in `dir` with `argv`, the fixed options and a cleared environment.
    pub(crate) fn git_in(&mut self, dir: &Path, argv: &[&str], stdin: Option<&[u8]>) -> Result<Output, String> {
        let hooks = self.config.dir.join("no-hooks");
        std::fs::create_dir_all(&hooks).map_err(|e| format!("creating {}: {e}", hooks.display()))?;
        let mut cmd = Command::new(&self.config.git);
        cmd.env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CEILING_DIRECTORIES", &self.config.dir)
            .env("HOME", &self.config.dir)
            .current_dir(dir);
        if self.config.trace {
            cmd.env("GIT_TRACE", self.config.dir.join("git-trace.log"));
        }
        if let Some(q) = &self.quarantine {
            // New objects go here; the store's own are read through it, never written.
            cmd.env("GIT_OBJECT_DIRECTORY", q).env("GIT_ALTERNATE_OBJECT_DIRECTORIES", self.objects());
        }
        let identity = &self.config.identity;
        let mut options = vec![format!("core.hooksPath={}", hooks.display())];
        options.extend(FIXED.iter().map(|o| o.to_string()));
        options.push(format!("user.name={identity}"));
        options.push(format!("user.email={identity}@experiment.invalid"));
        for o in &options {
            cmd.arg("-c").arg(o);
        }
        cmd.args(argv);
        self.runs.push(argv.iter().map(|a| a.to_string()).collect());
        let child = cmd
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting git: {e}"))?;
        let out = crate::mode::communicate(child, stdin).map_err(|e| format!("running git: {e}"))?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            raw: out.stdout,
        })
    }

    /// Runs git in the store's repository; an error if it fails.
    pub(crate) fn git(&mut self, argv: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let repo = self.repo();
        let out = self.git_in(&repo, argv, stdin)?;
        if out.ok {
            Ok(out.stdout)
        } else {
            let said = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
            Err(format!("git {}: {}", argv.first().copied().unwrap_or(""), crate::args::short(said.trim())))
        }
    }

    /// Whether `rev` names a commit here.
    #[cfg(test)]
    pub(crate) fn has_commit(&mut self, sha: &str) -> Result<bool, String> {
        let repo = self.repo();
        Ok(is_commit(sha) && self.git_in(&repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")], None)?.ok)
    }

    pub(crate) fn rev(&mut self, rev: &str) -> Result<Option<String>, String> {
        let repo = self.repo();
        let out = self.git_in(&repo, &["rev-parse", "--verify", "--quiet", "--end-of-options", rev], None)?;
        Ok(out.ok.then(|| out.stdout.trim().to_string()))
    }

    /// A file of the store's own for `bytes`, under `files/`.
    fn own_file(&mut self, bytes: &[u8]) -> Result<PathBuf, String> {
        let dir = self.config.dir.join("files");
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        self.files += 1;
        let path = dir.join(format!("{}.bundle", self.files));
        if !is_within(&path, &self.config.dir) {
            return Err("refusing a file outside the store".into());
        }
        std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
        Ok(path)
    }

    /// Checks `bytes` as a bundle and fetches its refs, each named by `map`
    /// (its ref in the bundle to the ref it becomes here, or refused). Only a
    /// file that starts with a bundle signature and passes `git bundle verify`
    /// is fetched (task `d0f9eefc612c`), so git never reads a gitfile, a
    /// directory or a repository from it, and never starts `git-upload-pack`.
    pub(crate) fn take_bundle(
        &mut self,
        bytes: &[u8],
        cap: Option<usize>,
        map: impl Fn(&str) -> Option<String>,
    ) -> Result<Vec<(String, String)>, String> {
        if !(bytes.starts_with(b"# v2 git bundle\n") || bytes.starts_with(b"# v3 git bundle\n")) {
            return Err("not a git bundle".into());
        }
        let path = self.own_file(bytes)?;
        let file = path.to_string_lossy().into_owned();
        self.git(&["bundle", "verify", "--quiet", "--end-of-options", &file], None)?;
        let heads = self.git(&["bundle", "list-heads", "--end-of-options", &file], None)?;
        let mut names = Vec::new();
        let mut taken = Vec::new();
        for line in heads.lines() {
            let (sha, name) = line.split_once(' ').ok_or("a bundle head without a ref")?;
            let to = map(name)
                .filter(|_| is_commit(sha))
                .ok_or_else(|| format!("refusing ref {}", crate::args::short(name)))?;
            names.push(name.to_string());
            taken.push((to, sha.to_string()));
        }
        // Fetched into a quarantine object directory, and written to no ref: its objects join the
        // store only once every new tree is checked, and are dropped if any isn't (finding `44769e26999c`).
        let quarantine = self.config.dir.join("quarantine");
        let _ = std::fs::remove_dir_all(&quarantine);
        std::fs::create_dir_all(quarantine.join("pack")).map_err(|e| format!("creating the quarantine: {e}"))?;
        self.quarantine = Some(quarantine.clone());
        let mut argv = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--end-of-options", &file];
        argv.extend(names.iter().map(String::as_str));
        let tips: Vec<String> = taken.iter().map(|(_, sha)| sha.clone()).collect();
        let fetched = self.git(&argv, None).and_then(|_| {
            if let Some(cap) = cap {
                // Counted before any tree is read (finding `4bcfe41f2818`).
                let mut count = vec!["rev-list", "--count"];
                count.extend(tips.iter().map(String::as_str));
                count.extend(["--not", "--all"]);
                let n: usize = self.git(&count, None)?.trim().parse().map_err(|_| "a bad count")?;
                if n > cap {
                    return Err(format!("a push may bring at most {cap} new commits, not {n}"));
                }
            }
            self.check_new_trees(&tips)?;
            // Only what the checked refs reach joins the store, in one new pack.
            let mut list = vec!["rev-list", "--objects"];
            list.extend(tips.iter().map(String::as_str));
            list.extend(["--not", "--all"]);
            let objects = self.git(&list, None)?;
            let pack = self.objects().join("pack").join("pack");
            let pack = pack.to_string_lossy().into_owned();
            self.git(&["pack-objects", "--quiet", &pack], Some(objects.as_bytes())).map(drop)
        });
        self.quarantine = None;
        let kept = fetched;
        let _ = std::fs::remove_dir_all(&quarantine);
        let _ = std::fs::remove_file(&path);
        kept?;
        self.move_refs(&taken)?;
        Ok(taken)
    }

    /// Points each name at its sha, which must be a commit here whose history new to this store
    /// holds only allowed trees: every ref update from what a peer said goes through this. A
    /// fixed number of git processes however many refs there are, and all move or none do.
    pub(crate) fn move_refs(&mut self, moves: &[(String, String)]) -> Result<(), String> {
        if moves.is_empty() {
            return Ok(());
        }
        let shas: Vec<String> = moves.iter().map(|(_, sha)| sha.clone()).collect();
        let here = self.commits_here(&shas)?;
        if let Some(missing) = shas.iter().find(|s| !here.contains(*s)) {
            return Err(format!("no commit {}", crate::args::short(missing)));
        }
        self.check_new_trees(&shas)?;
        let script: String = moves.iter().map(|(name, sha)| format!("update {name} {sha}\n")).collect();
        self.git(&["update-ref", "--stdin"], Some(script.as_bytes()))?;
        // A ref here points only at a checked history.
        self.checked.extend(shas);
        Ok(())
    }

    /// Which of `shas` name commits here, by one `git cat-file --batch-check`.
    pub(crate) fn commits_here(&mut self, shas: &[String]) -> Result<std::collections::BTreeSet<String>, String> {
        let asked: Vec<&String> = shas.iter().filter(|s| is_commit(s)).collect();
        if asked.is_empty() {
            return Ok(Default::default());
        }
        let input: String = asked.iter().map(|s| format!("{s}\n")).collect();
        let out = self.git(&["cat-file", "--batch-check=%(objectname) %(objecttype)"], Some(input.as_bytes()))?;
        Ok(out
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter(|(_, kind)| *kind == "commit")
            .map(|(sha, _)| sha.to_string())
            .collect())
    }

    /// Every agent branch at the hub, `<agent>/<branch>` to its tip, by one `git for-each-ref`.
    pub(crate) fn agent_heads(&mut self) -> Result<BTreeMap<String, String>, String> {
        let listing = self.git(&["for-each-ref", "--format=%(refname) %(objectname)", "refs/agents"], None)?;
        Ok(listing
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter_map(|(r, sha)| Some((r.strip_prefix("refs/agents/")?.to_string(), sha.to_string())))
            .collect())
    }

    /// What is wrong with any commit `tip` reaches that `base` doesn't, if anything; a tip
    /// already checked is not checked again.
    pub(crate) fn history_problem(&mut self, tip: &str, base: &str) -> Result<Option<String>, String> {
        if self.checked.contains(tip) {
            return Ok(None);
        }
        let commits: Vec<String> =
            self.git(&["rev-list", tip, "--not", base], None)?.lines().map(str::to_string).collect();
        let problem = self.trees_problem(&commits)?;
        if problem.is_none() {
            self.checked.insert(tip.to_string());
        }
        Ok(problem)
    }

    /// What is wrong with any of `commits`' trees ([`GitStore::tree_problem`]'s rules), found in
    /// three git processes however many commits there are: their objects, their trees, their blobs' sizes.
    fn trees_problem(&mut self, commits: &[String]) -> Result<Option<String>, String> {
        if commits.is_empty() {
            return Ok(None);
        }
        let short = |sha: &str| sha[..12.min(sha.len())].to_string();
        let mut trees = std::collections::BTreeMap::new();
        for (sha, kind, body) in self.batch(commits)? {
            let tree =
                String::from_utf8_lossy(&body).lines().next().and_then(|l| l.strip_prefix("tree ")).map(str::to_string);
            match (kind.as_str(), tree) {
                ("commit", Some(t)) if is_commit(&t) => trees.entry(t).or_insert(sha),
                _ => return Ok(Some(format!("{} isn't a commit", short(&sha)))),
            };
        }
        let mut blobs = std::collections::BTreeMap::new();
        let tree_list: Vec<String> = trees.keys().cloned().collect();
        for (tree, kind, body) in self.batch(&tree_list)? {
            let commit = &trees[&tree];
            if kind != "tree" {
                return Ok(Some(format!("commit {} has no tree", short(commit))));
            }
            for (mode, name, sha) in tree_entries(&body)? {
                let allowed = self.config.task.module_files().contains(&name.as_str()) || name == TASKS;
                if !allowed {
                    return Ok(Some(format!(
                        "commit {}: {} isn't a module file",
                        short(commit),
                        crate::args::short(&name)
                    )));
                }
                if mode != "100644" {
                    return Ok(Some(format!("commit {}: {name} isn't a regular file", short(commit))));
                }
                blobs.entry(sha).or_insert((commit.clone(), name));
            }
        }
        let blob_list: Vec<String> = blobs.keys().cloned().collect();
        let checked = self.git(&["cat-file", "--batch-check"], Some((blob_list.join("\n") + "\n").as_bytes()))?;
        for line in checked.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (commit, name) = blobs.get(fields.first().copied().unwrap_or_default()).cloned().unwrap_or_default();
            let size: Option<usize> = fields.get(2).and_then(|s| s.parse().ok());
            match (fields.get(1), size) {
                (Some(&"blob"), Some(n)) if n <= MAX_FILE => {}
                (Some(&"blob"), Some(n)) => {
                    return Ok(Some(format!(
                        "commit {}: {name} is {n} bytes, over the limit of {MAX_FILE}",
                        short(&commit)
                    )));
                }
                _ => return Ok(Some(format!("commit {}: {name} isn't a file", short(&commit)))),
            }
        }
        Ok(None)
    }

    /// The objects `ids` name, by one `git cat-file --batch`: id, type and content each.
    fn batch(&mut self, ids: &[String]) -> Result<Vec<(String, String, Vec<u8>)>, String> {
        let repo = self.repo();
        let out = self.git_in(&repo, &["cat-file", "--batch"], Some((ids.join("\n") + "\n").as_bytes()))?;
        if !out.ok {
            return Err(format!("git cat-file: {}", crate::args::short(out.stderr.trim())));
        }
        let raw = out.raw;
        let mut objects = Vec::new();
        let mut at = 0;
        while at < raw.len() {
            let end = raw[at..].iter().position(|&b| b == b'\n').ok_or("a cut cat-file header")? + at;
            let header = String::from_utf8_lossy(&raw[at..end]).into_owned();
            let fields: Vec<&str> = header.split(' ').collect();
            at = end + 1;
            match fields.as_slice() {
                [id, kind, size] => {
                    let size: usize = size.parse().map_err(|_| "a bad cat-file size")?;
                    let body = raw.get(at..at + size).ok_or("a cut object")?.to_vec();
                    objects.push((id.to_string(), kind.to_string(), body));
                    at += size + 1;
                }
                [id, "missing"] => objects.push((id.to_string(), "missing".into(), Vec::new())),
                _ => return Err("an unexpected cat-file header".into()),
            }
        }
        Ok(objects)
    }

    /// Refuses unless every commit `tips` reach that no ref here does holds an
    /// allowed tree ([`GitStore::tree_problem`]) (finding `f1dcf89fd948`).
    fn check_new_trees(&mut self, tips: &[String]) -> Result<(), String> {
        let tips: Vec<String> = tips.iter().filter(|t| !self.checked.contains(*t)).cloned().collect();
        if tips.is_empty() {
            return Ok(());
        }
        let mut argv = vec!["rev-list"];
        argv.extend(tips.iter().map(String::as_str));
        argv.extend(["--not", "--all"]);
        let commits: Vec<String> = self.git(&argv, None)?.lines().map(str::to_string).collect();
        match self.trees_problem(&commits)? {
            Some(problem) => Err(format!("refusing {problem}")),
            None => Ok(()),
        }
    }

    /// What is wrong with the tree of `rev`, if anything: it may hold only regular
    /// (`100644`) files at the module paths and `TASKS.md`, each at most
    /// [`MAX_FILE`] bytes. No symlink, gitlink, executable, directory or other path.
    pub(crate) fn tree_problem(&mut self, rev: &str) -> Result<Option<String>, String> {
        let listing = self.git(&["ls-tree", "-r", "-t", "-l", "--full-tree", "--end-of-options", rev], None)?;
        for line in listing.lines() {
            let (meta, path) = line.split_once('\t').ok_or("a tree entry without a path")?;
            let fields: Vec<&str> = meta.split_whitespace().collect();
            let allowed = self.config.task.module_files().contains(&path) || path == TASKS;
            if !allowed {
                return Ok(Some(format!("{} isn't a module file", crate::args::short(path))));
            }
            if fields.first() != Some(&"100644") || fields.get(1) != Some(&"blob") {
                return Ok(Some(format!("{path} isn't a regular file")));
            }
            let size: usize = fields.get(3).and_then(|s| s.parse().ok()).ok_or("a blob without a size")?;
            if size > MAX_FILE {
                return Ok(Some(format!("{path} is {size} bytes, over the limit of {MAX_FILE}")));
            }
        }
        Ok(None)
    }

    /// A bundle of `refs` without what `not` reaches; empty if that is nothing.
    pub(crate) fn make_bundle(&mut self, refs: &[String], not: &[String]) -> Result<Vec<u8>, String> {
        if refs.is_empty() {
            return Ok(Vec::new());
        }
        let mut count = vec!["rev-list", "--count"];
        count.extend(refs.iter().map(String::as_str));
        count.push("--not");
        count.extend(not.iter().map(String::as_str));
        if self.git(&count, None)?.trim() == "0" {
            return Ok(Vec::new());
        }
        let path = self.own_file(b"")?;
        let file = path.to_string_lossy().into_owned();
        let mut argv = vec!["bundle", "create", "--quiet", &file];
        argv.extend(refs.iter().map(String::as_str));
        argv.push("--not");
        argv.extend(not.iter().map(String::as_str));
        self.git(&argv, None)?;
        std::fs::read(&path).map_err(|e| format!("reading the bundle: {e}"))
    }

    /// The module files at `rev`, missing ones empty.
    pub(crate) fn modules_at(&mut self, rev: &str) -> Result<Vec<(String, String)>, String> {
        // Sizes and modes first: nothing large or strange is read (finding `32a67d979d81`).
        if let Some(problem) = self.tree_problem(rev)? {
            return Err(problem);
        }
        let mut out = Vec::new();
        let task = self.config.task;
        for (module, file) in task.modules().iter().zip(task.module_files()) {
            let repo = self.repo();
            let got = self.git_in(&repo, &["cat-file", "blob", &format!("{rev}:{file}")], None)?;
            out.push((module.to_string(), if got.ok { got.stdout } else { String::new() }));
        }
        Ok(out)
    }

    /// The version `TASKS.md` at `rev` states.
    fn version_at(&mut self, rev: &str) -> Result<Version, String> {
        let text = self.git(&["cat-file", "blob", &format!("{rev}:{TASKS}")], None)?;
        Ok(self.config.task.version_of(&text))
    }

    // The hub.

    fn setup_hub(&mut self) -> Result<Value, String> {
        std::fs::create_dir_all(self.bare()).map_err(|e| format!("creating the hub: {e}"))?;
        self.git(&["init", "--quiet", "--bare"], None)?;
        let tasks = self.config.task.tasks_md(Version::ORIGINAL);
        let commit = self.commit_files(None, &[(TASKS, tasks.as_str())], "The task")?;
        self.git(&["update-ref", "refs/heads/main", &commit], None)?;
        prs::Prs::default().save(&self.config.dir)?;
        Ok(json!({ "main": commit }))
    }

    /// A commit on `parent` (or a root) of the parent's tree with `files` replaced, and every
    /// module file present: plumbing only, no working tree.
    pub(crate) fn commit_files(
        &mut self,
        parent: Option<&str>,
        files: &[(&str, &str)],
        message: &str,
    ) -> Result<String, String> {
        let mut entries: BTreeMap<String, String> = BTreeMap::new();
        if let Some(p) = parent {
            for line in self.git(&["ls-tree", "--end-of-options", p], None)?.lines() {
                if let Some((meta, name)) = line.split_once('\t') {
                    entries.insert(name.to_string(), meta.to_string());
                }
            }
        }
        for file in self.config.task.module_files() {
            if !entries.contains_key(*file) {
                let blob = self.git(&["hash-object", "-w", "--stdin"], Some(b""))?;
                entries.insert(file.to_string(), format!("100644 blob {}", blob.trim()));
            }
        }
        for (name, text) in files {
            let blob = self.git(&["hash-object", "-w", "--stdin"], Some(text.as_bytes()))?;
            entries.insert(name.to_string(), format!("100644 blob {}", blob.trim()));
        }
        let listing: String = entries.iter().map(|(name, meta)| format!("{meta}\t{name}\n")).collect();
        let tree = self.git(&["mktree"], Some(listing.as_bytes()))?;
        let mut argv = vec!["commit-tree".to_string()];
        if let Some(p) = parent {
            argv.push(format!("-p{p}"));
        }
        argv.push(tree.trim().to_string());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        Ok(self.git(&argv, Some(message.as_bytes()))?.trim().to_string())
    }

    /// The hub's released version, from `main`'s `TASKS.md`, which only the setter writes.
    pub(crate) fn hub_version(&mut self) -> Result<Version, String> {
        self.version_at("refs/heads/main")
    }

    // A copy.

    fn setup_copy(&mut self, bundle: &[u8]) -> Result<Value, String> {
        std::fs::create_dir_all(self.work()).map_err(|e| format!("creating the copy: {e}"))?;
        self.git(&["init", "--quiet"], None)?;
        self.take_bundle(bundle, None, |r| (r == "refs/heads/main").then(|| "refs/remotes/hub/main".to_string()))?;
        self.git(&["checkout", "--quiet", "--detach", "--end-of-options", "refs/remotes/hub/main", "--"], None)?;
        Ok(json!({}))
    }

    fn current_branch(&mut self) -> Result<Option<String>, String> {
        let out = self.git_in(&self.work(), &["symbolic-ref", "--quiet", "--short", "HEAD"], None)?;
        Ok(out.ok.then(|| out.stdout.trim().to_string()))
    }

    /// The path of a working-copy file, which must be a regular file (never a link) if it exists.
    fn file_path(&self, file: &str) -> Result<PathBuf, String> {
        let path = self.work().join(file);
        if !is_within(&path, &self.work()) {
            return Err("refusing a file outside the copy".into());
        }
        match std::fs::symlink_metadata(&path) {
            Ok(m) if !m.file_type().is_file() => Err(format!("{file} isn't a regular file")),
            Ok(m) if m.len() > MAX_FILE as u64 => Err(format!("{file} is over {MAX_FILE} bytes")),
            Ok(_) => Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path),
            Err(e) => Err(format!("{file}: {e}")),
        }
    }

    /// A working-copy file's text: empty if it doesn't exist.
    fn read_file(&self, file: &str) -> Result<String, String> {
        match std::fs::read_to_string(self.file_path(file)?) {
            Ok(t) => Ok(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("reading {file}: {e}")),
        }
    }

    fn copy_tool(&mut self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "read" => {
                let file = readable(self.config.task, args)?;
                let text = self.read_file(file)?;
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
                let file = module_file(self.config.task, args)?;
                let path = self.file_path(file)?;
                let text = self.read_file(file)?;
                let new = if name == "edit" { edit_lines(&text, args)? } else { replace_once(&text, args)? };
                if new.len() > MAX_FILE {
                    return Err(format!("a module file holds at most {MAX_FILE} bytes"));
                }
                std::fs::write(&path, &new).map_err(|e| format!("writing {file}: {e}"))?;
                Ok(json!({ "file": file, "lines": new.lines().count() }))
            }
            "status" => Ok(json!({ "status": self.git(&["status", "--porcelain=v1", "--branch"], None)? })),
            "diff" => {
                let mut argv = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
                if args.choice_opt("against", &["head", "main"])? == Some("main") {
                    argv.extend(["--end-of-options", "refs/remotes/hub/main"]);
                }
                argv.push("--");
                Ok(json!({ "diff": self.git(&argv, None)? }))
            }
            "log" => {
                let n = format!("--max-count={}", args.int_opt("count", 1..=50)?.unwrap_or(10));
                Ok(json!({ "log": self.git(&["log", "--oneline", "--no-decorate", &n], None)? }))
            }
            "switch" => self.switch(args),
            "commit" => {
                let message = args.text("message")?;
                if self.current_branch()?.is_none() {
                    return Err("switch to a branch of yours first".into());
                }
                let mut add = vec!["add", "--end-of-options"];
                add.extend(self.config.task.module_files());
                self.git(&add, None)?;
                self.git(
                    &["commit", "--quiet", "--no-verify", "--allow-empty-message", "--file=-"],
                    Some(message.as_bytes()),
                )?;
                Ok(json!({ "commit": self.rev("HEAD")? }))
            }
            "merge" => {
                if args.bool_opt("abort")? {
                    self.git(&["merge", "--abort"], None)?;
                    return Ok(json!({ "aborted": true }));
                }
                if self.current_branch()?.is_none() {
                    return Err("switch to a branch of yours first".into());
                }
                let out = self.git_in(
                    &self.work(),
                    &["merge", "--quiet", "--no-edit", "--end-of-options", "refs/remotes/hub/main"],
                    None,
                )?;
                if out.ok {
                    return Ok(json!({ "merged": true }));
                }
                let conflicts = self.git(&["diff", "--name-only", "--diff-filter=U"], None)?;
                if conflicts.trim().is_empty() {
                    return Err(format!("git merge: {}", crate::args::short(out.stderr.trim())));
                }
                Ok(json!({ "merged": false, "conflicts": conflicts.lines().collect::<Vec<_>>(),
                    "next": "edit the conflicted files, then commit; or merge with abort" }))
            }
            "rebase" => {
                if self.current_branch()?.is_none() {
                    return Err("switch to a branch of yours first".into());
                }
                let out = self.git_in(
                    &self.work(),
                    &["rebase", "--quiet", "--end-of-options", "refs/remotes/hub/main"],
                    None,
                )?;
                if out.ok {
                    return Ok(json!({ "rebased": true }));
                }
                let _ = self.git_in(&self.work(), &["rebase", "--abort"], None);
                Err("the rebase stopped at a conflict and was undone; merge instead".into())
            }
            "check" => {
                let task = self.config.task;
                let function = args.spec_name("function", task)?;
                let mut modules = Vec::new();
                for (module, file) in task.modules().iter().zip(task.module_files()) {
                    let text = self.read_file(file)?;
                    modules.push((module.to_string(), text));
                }
                let library = Library::parse(modules.iter().map(|(m, t)| (m.as_str(), t.as_str())));
                let version = self.version_at("refs/remotes/hub/main")?;
                let result = task.check(&library, function, version)?;
                Ok(json!({ "function": function, "ok": result.ok(), "details": result.details() }))
            }
            other => Err(format!("no tool {}", crate::args::short(other))),
        }
    }

    fn switch(&mut self, args: &Args) -> Result<Value, String> {
        let branch = args.branch("branch")?;
        if !args.bool_opt("create")? {
            if args.int_opt("from_pr", 1..=100_000)?.is_some() {
                return Err("from_pr needs create".into());
            }
            self.git(&["switch", "--quiet", "--end-of-options", branch], None)?;
            return Ok(json!({ "branch": branch }));
        }
        let start = match args.int_opt("from_pr", 1..=100_000)? {
            Some(n) => format!("refs/remotes/hub/pr-{n}"),
            None => "refs/remotes/hub/main".to_string(),
        };
        if self.rev(&start)?.is_none() {
            return Err("no such PR as of your last sync".into());
        }
        self.git(
            &["switch", "--quiet", "--no-track", &format!("--create={branch}"), "--end-of-options", &start],
            None,
        )?;
        Ok(json!({ "branch": branch, "from": start.trim_start_matches("refs/remotes/hub/") }))
    }
}

/// The readable file an argument names, from the fixed list: never a path.
fn readable(task: Task, args: &Args) -> Result<&'static str, String> {
    let v = args.text("file")?;
    task.module_files().iter().chain([&TASKS]).find(|f| **f == v).copied().ok_or_else(|| "no such file".to_string())
}

fn module_file(task: Task, args: &Args) -> Result<&'static str, String> {
    let v = args.text("file")?;
    task.module_files()
        .iter()
        .find(|f| **f == v)
        .copied()
        .ok_or_else(|| "only the module files can be edited".to_string())
}

fn edit_lines(text: &str, args: &Args) -> Result<String, String> {
    let start = args.int("start", 1..=100_000)? as usize;
    let end = args.int("end", 0..=100_000)? as usize;
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

impl Store for GitStore {
    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String> {
        match hub_export {
            None => self.setup_hub(),
            Some(bundle) => self.setup_copy(bundle),
        }
    }

    /// The hub's `main`, whole: what a new copy starts from.
    fn export(&mut self) -> Result<Vec<u8>, String> {
        self.make_bundle(&["refs/heads/main".into()], &[])
    }

    fn import(&mut self, _bytes: &[u8]) -> Result<Value, String> {
        Err("git mode syncs by push and fetch".into())
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        if !COPY_TOOLS.contains(&name) {
            return Err(format!("no tool {}", crate::args::short(name)));
        }
        let args = Args::new(args, allowed(name))?;
        self.copy_tool(name, &args)
    }

    /// Every branch of this copy, as a bundle without what the hub already sent it, and their tips.
    fn push(&mut self) -> Result<Transfer, String> {
        let listing = self.git(&["for-each-ref", "--format=%(refname:short) %(objectname)", "refs/heads"], None)?;
        let branches: BTreeMap<String, String> = listing
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter(|(b, sha)| is_branch(b) && is_commit(sha))
            .map(|(b, sha)| (b.to_string(), sha.to_string()))
            .collect();
        let refs: Vec<String> = branches.keys().map(|b| format!("refs/heads/{b}")).collect();
        // Without what the hub sent, and what it said it holds of this copy's branches.
        let bytes = self.make_bundle(&refs, &["--remotes=hub".to_string(), "--glob=refs/hub-has/*".to_string()])?;
        let have = self.git(&["for-each-ref", "--format=%(objectname)", "refs/remotes/hub", "refs/heads"], None)?;
        let have: Vec<&str> = have.lines().take(MAX_HAVE).collect();
        Ok(Transfer { meta: json!({ "branches": branches, "have": have }), bytes })
    }

    /// One agent's branches into `refs/agents/<agent>/`, after the bundle's checks.
    fn receive(&mut self, session: &Session, push: Transfer) -> Result<Value, String> {
        let agent = agent_ref(session)?;
        // In a file of its own, so a sync never reads PR text (finding `7d119d53922f`).
        let counter = self.config.dir.join("received.json");
        let mut used: BTreeMap<String, u64> = match std::fs::read_to_string(&counter) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("the push counter: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("reading the push counter: {e}")),
        };
        let total = used.get(&session.name).copied().unwrap_or(0) + push.bytes.len() as u64;
        if total > session.quota_bytes {
            return Err("this agent's pushes have used their quota".into());
        }
        // Counted before anything is checked: a refused push uses quota too.
        used.insert(session.name.clone(), total);
        let text = serde_json::to_string(&used).map_err(|e| e.to_string())?;
        std::fs::write(&counter, text).map_err(|e| format!("writing the push counter: {e}"))?;
        let mut taken = Vec::new();
        if !push.bytes.is_empty() {
            taken = self.take_bundle(&push.bytes, Some(MAX_NEW_COMMITS), |r| {
                r.strip_prefix("refs/heads/").filter(|b| is_branch(b)).map(|b| format!("refs/agents/{agent}/{b}"))
            })?;
        }
        // Branches whose tips the hub has already (nothing new in the bundle) still move.
        let branches = push.meta["branches"].as_object().cloned().unwrap_or_default();
        let moves: Vec<(String, String)> = branches
            .iter()
            .filter(|(b, _)| is_branch(b))
            .map(|(b, sha)| (format!("refs/agents/{agent}/{b}"), sha.as_str().unwrap_or_default().to_string()))
            .collect();
        self.move_refs(&moves)?;
        Ok(json!({ "branches": branches.len(), "fetched": taken.len() }))
    }

    /// `main` and every open PR's head, without what the copy has, and their tips.
    fn send(&mut self, session: &Session, have: &Value) -> Result<Transfer, String> {
        // A fixed number of git processes, whatever the open PRs (at most each agent's cap), except
        // the first check of a head not seen before (finding `f582d00f535f`).
        let agent = agent_ref(session)?.to_string();
        let heads = self.agent_heads()?;
        let mine = format!("{agent}/");
        let yours: BTreeMap<String, String> =
            heads.iter().filter_map(|(r, sha)| Some((r.strip_prefix(&mine)?.to_string(), sha.clone()))).collect();
        let open = prs::Prs::load_open(&self.config.dir)?;
        let mut refs = BTreeMap::new();
        let main = self.rev("refs/heads/main")?.ok_or("the hub has no main")?;
        refs.insert("main".to_string(), main);
        let mut skipped = Vec::new();
        let mut script = String::new();
        for (n, author, branch) in open {
            if let Some(sha) = heads.get(&format!("{author}/{branch}")).cloned() {
                // One agent's bad PR head never fails another's sync: it is left out, and said.
                if self.history_problem(&sha, "refs/heads/main")?.is_some() {
                    skipped.push(n);
                    continue;
                }
                script.push_str(&format!("update refs/pr/{n} {sha}\n"));
                refs.insert(format!("pr-{n}"), sha);
            }
        }
        if !script.is_empty() {
            self.git(&["update-ref", "--stdin"], Some(script.as_bytes()))?;
        }
        let asked: Vec<String> = have
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(MAX_HAVE)
            .map(str::to_string)
            .collect();
        let not: Vec<String> = self.commits_here(&asked)?.into_iter().collect();
        let names: Vec<String> = refs
            .keys()
            .map(|r| if r == "main" { "refs/heads/main".into() } else { format!("refs/pr/{}", &r[3..]) })
            .collect();
        let bytes = self.make_bundle(&names, &not)?;
        Ok(Transfer { meta: json!({ "refs": refs, "yours": yours, "skipped": skipped }), bytes })
    }

    /// The hub's `main` and open PRs into `refs/remotes/hub/`, after the bundle's checks.
    fn fetch(&mut self, answer: Transfer) -> Result<Value, String> {
        if !answer.bytes.is_empty() {
            self.take_bundle(&answer.bytes, None, |r| {
                if r == "refs/heads/main" {
                    return Some("refs/remotes/hub/main".into());
                }
                let n = r.strip_prefix("refs/pr/")?;
                (!n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| format!("refs/remotes/hub/pr-{n}"))
            })?;
        }
        let refs = answer.meta["refs"].as_object().cloned().unwrap_or_default();
        let is_pr = |r: &str| {
            r.strip_prefix("pr-")
                .is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
        };
        let moves: Vec<(String, String)> = refs
            .iter()
            .filter(|(name, _)| *name == "main" || is_pr(name))
            .map(|(name, sha)| (format!("refs/remotes/hub/{name}"), sha.as_str().unwrap_or_default().to_string()))
            .collect();
        self.move_refs(&moves)?;
        // What the hub holds of this copy's branches: the next push leaves it out.
        let yours = answer.meta["yours"].as_object().cloned().unwrap_or_default();
        let asked: Vec<String> = yours.values().filter_map(Value::as_str).map(str::to_string).collect();
        let here = self.commits_here(&asked)?;
        let moves: Vec<(String, String)> = yours
            .iter()
            .filter_map(|(b, sha)| Some((b, sha.as_str()?)))
            .filter(|(b, sha)| is_branch(b) && here.contains(*sha))
            .map(|(b, sha)| (format!("refs/hub-has/{b}"), sha.to_string()))
            .collect();
        self.move_refs(&moves)?;
        // PRs no longer open go, in one update.
        let listing = self.git(&["for-each-ref", "--format=%(refname)", "refs/remotes/hub"], None)?;
        let script: String = listing
            .lines()
            .filter(|r| {
                let name = r.trim_start_matches("refs/remotes/hub/");
                is_pr(name) && !refs.contains_key(name)
            })
            .map(|r| format!("delete {r}\n"))
            .collect();
        if !script.is_empty() {
            self.git(&["update-ref", "--stdin"], Some(script.as_bytes()))?;
        }
        Ok(json!({ "refs": refs.len() }))
    }

    fn validate(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        if !PR_TOOLS.contains(&name) {
            return Err(format!("no hub tool {}", crate::args::short(name)));
        }
        prs::checked(name, &Args::new(args, allowed(name))?)
    }

    fn hub_tool(&mut self, session: &Session, name: &str, args: &Value) -> Result<Value, String> {
        if !PR_TOOLS.contains(&name) {
            return Err(format!("no hub tool {}", crate::args::short(name)));
        }
        let args = prs::checked(name, &Args::new(args, allowed(name))?)?;
        prs::run(self, session, name, &args)
    }

    fn release_changes(&mut self, wave: usize) -> Result<Value, String> {
        let main = self.rev("refs/heads/main")?.ok_or("the hub has no main")?;
        let task = self.config.task;
        let tasks = task.tasks_md(task.after_wave(wave));
        let commit = self.commit_files(Some(&main), &[(TASKS, tasks.as_str())], "Change requests")?;
        self.git(&["update-ref", "refs/heads/main", &commit, &main], None)?;
        let released: Vec<&str> = task.wave_changes(wave).map(|c| change_tag(c.change)).collect();
        Ok(json!({ "released": released }))
    }

    fn integrated(&mut self) -> Result<Vec<(String, String)>, String> {
        self.modules_at("refs/heads/main")
    }

    fn record(&mut self, tool: &str, args: &Value, result: &Value) -> Result<Value, String> {
        use crate::metrics::definitions as defs;
        match tool {
            // A commit writes each function whose definition differs from every parent's, so a merge
            // commit doesn't credit its author with what it merged.
            "commit" => {
                let line = self.git(&["rev-list", "--parents", "--max-count=1", "HEAD"], None)?;
                let ids: Vec<String> = line.split_whitespace().map(str::to_string).collect();
                let head = ids.first().ok_or("a commit without HEAD")?;
                let now = defs(self.config.task, &self.modules_at(head)?);
                let mut parents = Vec::new();
                for p in &ids[1..] {
                    parents.push(defs(self.config.task, &self.modules_at(p)?));
                }
                let task = self.config.task;
                // A task with open contracts: the branch's members at the commit, its working copy of the
                // contracts (decision `979e5085a1e7`).
                let members: Vec<_> = now.iter().filter(|(f, _)| task.contract_of(f).is_some()).collect();
                let writes: Vec<_> =
                    now.iter().filter(|(f, d)| parents.iter().all(|b| b.get(*f) != Some(*d))).collect();
                if task.contracts().is_empty() {
                    Ok(json!({ "writes": writes }))
                } else {
                    Ok(json!({ "writes": writes, "members": members }))
                }
            }
            "merge" => Ok(json!({ "conflict": result["merged"] == false })),
            "pr_review" if args["verdict"] == "request_changes" => {
                Ok(json!({ "blocks": prs::changed(self, args["number"].as_i64().unwrap_or_default())? }))
            }
            _ => Ok(Value::Null),
        }
    }
}

/// A raw tree's entries: mode, name and object id (hex).
fn tree_entries(body: &[u8]) -> Result<Vec<(String, String, String)>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let space = body[at..].iter().position(|&b| b == b' ').ok_or("a cut tree entry")? + at;
        let nul = body[space..].iter().position(|&b| b == 0).ok_or("a cut tree entry")? + space;
        let id = body.get(nul + 1..nul + 21).ok_or("a cut tree entry")?;
        out.push((
            String::from_utf8_lossy(&body[at..space]).into_owned(),
            String::from_utf8_lossy(&body[space + 1..nul]).into_owned(),
            hex::encode(id),
        ));
        at = nul + 21;
    }
    Ok(out)
}

/// The ref-safe form of the agent a session is for: its harness-set name.
fn agent_ref(session: &Session) -> Result<&str, String> {
    if is_branch(&session.name) { Ok(&session.name) } else { Err("a session's agent name isn't ref-safe".into()) }
}

/// Programs by function from module texts, for CI's comparisons.
pub(crate) fn definitions(task: Task, modules: &[(String, String)]) -> Programs {
    let mut out = Programs::new();
    for (_, text) in modules {
        for line in text.lines() {
            let code = line.split('#').next().unwrap_or_default();
            let mut words = code.split_whitespace();
            if words.next() == Some("def")
                && let Some(spec) = words.next().and_then(|w| task.spec_named(w))
            {
                out.entry(spec.name).or_default().push_str(&format!("{}\n", code.trim()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
