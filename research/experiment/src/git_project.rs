//! Git project mode (task `6bd13d994a03`): agents build an evaluation project's tree with git and branches in
//! place of Bracken and Weave, the eval pilot's git baseline (owner decision `b1b56d75221b`). Everything else is
//! Weave mode's: the project, the task tool, the file tools, `run_tests`, the change's release and the
//! checkpoints.
//!
//! The hub is a bare repository under the run directory, whose `main` starts as one commit of the project's
//! seed. Each agent's copy is a clone of it: a working tree with its own repository. An agent edits files,
//! commits on branches of its own, merges, and syncs: its branches go to the hub as `<agent>/<branch>`, and it
//! fetches `main` and every agent's branches, as `origin/main` and `origin/<agent>/<branch>`. A builder asks for
//! its branch to be merged with `request_integration`, which its next sync sends; `requests` lists the open ones.
//! An agent's own `main` goes with its sync too, and the hub makes it `main` only if the integrator setting lets
//! that agent update main ([`may_update_main`]) and it is a fast-forward of the hub's: the hub enforces both.
//!
//! As in Phase 1's git mode ([`crate::git_mode`]), whose store this one drives, data moves between copies and
//! the hub only as git bundles the driver carries, each checked by its receiver before any of it is kept, so no
//! agent's process reaches the hub's repository or another copy. git runs from its absolute path with a cleared
//! environment, no system or global configuration, no hooks, aliases, attributes, credential helper or
//! protocol but a bundle's file, and every object fetched checked. Every ref an agent names is checked
//! ([`branch_name`], then `git check-ref-format`) and given to git whole, after `--end-of-options`; free text
//! goes on standard input. Every tree the hub or a copy takes in holds only regular files at plain paths
//! ([`crate::weave_mode::plain_path`]), within Weave's limits ([`trees_problem`]).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::args::{Args, is_commit, short};
use crate::git_mode::{GitStore, MAX_NEW_COMMITS, TreeRule, fsck_on_bundle_fetch};
use crate::mode::{Mode, Session, Store, StoreConfig, SyncKind, ToolSpec, Transfer};
use crate::task::Task;
use crate::weave_mode::{ANY_AGENT, MAX_FILE_BYTES, MAX_FILES, plain_path};

/// The mode's name, as the plan and the workers know it; `run --mode git --project` picks it.
pub const NAME: &str = "git-project";

const MAIN: &str = "refs/heads/main";
const ORIGIN_MAIN: &str = "refs/remotes/origin/main";
/// The longest branch name an agent may give.
const MAX_BRANCH: usize = 100;
/// The most local branches a copy pushes, and the most requests one agent has open.
const MAX_BRANCHES: usize = 100;
/// The longest summary of an integration request, and commit message.
const MAX_SUMMARY: usize = 2000;
const MAX_MESSAGE: usize = 4000;
/// The most commit ids a copy says it has.
const MAX_HAVE: usize = 1024;

/// The tools that are Weave mode's, word for word.
pub(crate) const SHARED: [&str; 7] =
    ["task", "list_files", "read_file", "write_file", "edit_file", "delete_file", "run_tests"];

pub struct GitProjectMode;

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
    }
}

impl Mode for GitProjectMode {
    fn name(&self) -> &'static str {
        NAME
    }

    fn tools(&self, task: Task) -> Vec<ToolSpec> {
        let mut tools: Vec<ToolSpec> =
            crate::weave_mode::WeaveMode.tools(task).into_iter().filter(|t| SHARED.contains(&t.name)).collect();
        let reference =
            json!({"type": "string", "description": "origin/main, origin/<agent>/<branch>, or a branch of yours"});
        let branch = json!({"type": "string", "description": "a-z, 0-9, ., _, - and /, such as fix/parse-dates"});
        let path =
            json!({"type": "string", "description": "A path in the tree, relative to its root, such as pkg/mod.py"});
        let page = json!({"type": "integer", "minimum": 1});
        tools.extend([
            tool(
                "status",
                "Your current branch, whether a merge is in progress and which files conflict, your uncommitted changes, how far your branch is ahead of and behind origin/main, your branches and the hub's (origin/...) as of your last sync, and who may update main",
                json!({}),
                &[],
            ),
            tool(
                "switch",
                "Switch your working tree to `branch`, a branch of yours; with create, make it first, from `start` (origin/main if not given). Refused while you have uncommitted changes or a merge in progress",
                json!({"branch": branch, "create": {"type": "boolean"}, "start": reference}),
                &["branch"],
            ),
            tool(
                "commit",
                "Commit every change in your working tree (new, changed and deleted files) on your current branch. During a merge this concludes it, refused while a conflicted file still holds conflict markers",
                json!({"message": {"type": "string"}}),
                &["message"],
            ),
            tool(
                "merge",
                "Merge `ref` into your current branch, as git merge does: a fast-forward, or a merge commit. A merge that conflicts leaves conflict markers (<<<<<<< and >>>>>>>) in each conflicted file it lists: settle them, then commit; or merge with abort to undo it. Refused while you have uncommitted changes",
                json!({"ref": reference, "abort": {"type": "boolean"}}),
                &[],
            ),
            tool(
                "diff",
                "Your working tree, committed or not, against `against` (your branch's last commit if not given); or, with `to`, what branch `to` changed since it parted from `against`, as git diff against...to does: give against origin/main and to a requested branch to see what it brings to main. Each file changed, and the diff; with `path`, that file's alone. Changes nothing. A diff too large for one result comes in pages: files says the page each file's diff is on, and `page` (from 1) picks one",
                json!({"against": reference, "to": reference, "path": path, "page": page}),
                &[],
            ),
            tool(
                "log",
                "Recent commits of `ref` (your current branch if not given), newest first, each with its author and message",
                json!({"ref": reference, "count": {"type": "integer", "minimum": 1, "maximum": 50}}),
                &[],
            ),
            tool(
                "request_integration",
                "Ask for `branch`, a branch of yours, to be merged into main, with a summary of what it does: your next sync pushes the branch and sends the request, which requests then shows everyone",
                json!({"branch": branch, "summary": {"type": "string"}}),
                &["branch", "summary"],
            ),
            tool(
                "requests",
                "The open integration requests as of your last sync: each one's author, its branch as origin/<agent>/<branch>, that branch's commit and its summary. A request closes once its branch is in main",
                json!({}),
                &[],
            ),
        ]);
        tools
    }

    fn sync_kind(&self) -> SyncKind {
        SyncKind::Transfers
    }

    fn open(&self, config: StoreConfig) -> Box<dyn Store> {
        Box::new(GitProjectStore { git: GitStore::with_rule(config, TreeRule::Project) })
    }
}

/// The mode a run of `mode` is, with a project or not: git with a project is git project mode, git and branches
/// on a project's tree; every other mode is itself.
pub fn project_mode(mode: &str, project: bool) -> &str {
    if mode == "git" && project { NAME } else { mode }
}

/// The run's caps from `--mode-token-cap`, for `mode` with a project or not. A git project run takes a cap given
/// for git or for git-project alike, but not both (review note 1 of `f218ba241bb4`): neither is ignored.
pub fn caps_for(
    mode: &str,
    project: bool,
    given: &[crate::backend::ModeTokenCap],
) -> Result<crate::backend::Caps, String> {
    let run_mode = project_mode(mode, project);
    let named = |c: &crate::backend::ModeTokenCap| crate::backend::ModeTokenCap {
        mode: if c.mode == mode { run_mode.to_string() } else { c.mode.clone() },
        tokens: c.tokens,
    };
    crate::backend::Caps::for_mode(run_mode, &given.iter().map(named).collect::<Vec<_>>())
}

/// Whether agent `name` may update main under the integrator setting `setting`: every agent under
/// [`ANY_AGENT`], else only the agent it names.
pub fn may_update_main(setting: &str, name: &str) -> bool {
    setting == ANY_AGENT || setting == name
}

/// A branch name an agent may give: 1 to [`MAX_BRANCH`] bytes of a-z, 0-9, `.`, `_`, `-` and `/`, in parts
/// none empty or starting with `.` or `-`, none `..` and none ending `.lock`; never `HEAD`, nor starting
/// `origin/`, which names the hub's. git's own `check-ref-format` checks it too ([`GitProjectStore::checked`]).
pub fn branch_name(name: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("branch {}: {why}", short(name)));
    if name.is_empty() || name.len() > MAX_BRANCH {
        return bad(&format!("give 1 to {MAX_BRANCH} characters"));
    }
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-/".contains(&b)) {
        return bad("use only a-z, 0-9, ., _, - and /");
    }
    for part in name.split('/') {
        if part.is_empty() || part.starts_with(['.', '-']) || part.ends_with(".lock") || part.contains("..") {
            return bad("no empty part, none starting with . or -, none ending .lock, and no ..");
        }
    }
    if name == "head" || name.starts_with("origin/") || name == "origin" {
        return bad("that name is taken: origin/... names the hub's branches");
    }
    Ok(())
}

/// An agent's name as the harness sets it: `agent-<n>`.
fn agent_name(name: &str) -> bool {
    name.strip_prefix("agent-").is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// What is wrong with any of `commits`' trees, if anything: each may hold only regular files (mode 100644 or
/// 100755) at plain paths ([`plain_path`]), at most [`MAX_FILE_BYTES`] each and [`MAX_FILES`] in all. No
/// symlink, submodule or path into `.git`. Read as raw bytes, so a path that isn't UTF-8 is refused, not
/// mended.
pub(crate) fn trees_problem(store: &mut GitStore, commits: &[String]) -> Result<Option<String>, String> {
    let mut trees = std::collections::BTreeSet::new();
    for (sha, kind, body) in store.batch(commits)? {
        let tree =
            String::from_utf8_lossy(&body).lines().next().and_then(|l| l.strip_prefix("tree ")).map(str::to_string);
        match (kind.as_str(), tree) {
            ("commit", Some(t)) if is_commit(&t) => trees.insert(t),
            _ => return Ok(Some(format!("{} isn't a commit", short(&sha)))),
        };
    }
    for tree in trees {
        let repo = store.config.dir.join(if store.config.dir.join("hub.git").exists() { "hub.git" } else { "work" });
        let out =
            store.git_in(&repo, &["ls-tree", "-r", "-l", "-z", "--full-tree", "--end-of-options", &tree], None)?;
        if !out.ok {
            return Err(format!("git ls-tree: {}", short(out.stderr.trim())));
        }
        if let Err(problem) = entries(&out.raw) {
            return Ok(Some(format!("tree {}: {problem}", &tree[..12])));
        }
    }
    Ok(None)
}

/// The entries of `git ls-tree -r -l -z`'s output, as `(path, blob)`, each checked as [`trees_problem`] says.
fn entries(listing: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let tab = entry.iter().position(|b| *b == b'\t').ok_or("an entry without a path")?;
        let path = std::str::from_utf8(&entry[tab + 1..]).map_err(|_| "a path that isn't UTF-8")?;
        let meta = String::from_utf8_lossy(&entry[..tab]);
        let fields: Vec<&str> = meta.split_whitespace().collect();
        let [mode, kind, sha, size] = fields[..] else { return Err(format!("{}: an odd entry", short(path))) };
        if kind != "blob" || !(mode == "100644" || mode == "100755") {
            return Err(format!("{} isn't a regular file", short(path)));
        }
        plain_path(path)?;
        let size: usize = size.parse().map_err(|_| format!("{}: no size", short(path)))?;
        if size > MAX_FILE_BYTES {
            return Err(format!("{} is {size} bytes, over the limit of {MAX_FILE_BYTES}", short(path)));
        }
        out.push((path.to_string(), sha.to_string()));
    }
    if out.len() > MAX_FILES {
        return Err(format!("{} files, over the limit of {MAX_FILES}", out.len()));
    }
    Ok(out)
}

/// The files of commit or tree `rev` in the repository `repo`, each checked as [`trees_problem`] says, read
/// with the git binary `git` under the same fixed options as every store's: the hub's integrated tree, and a
/// run's trees for `grade-run`. Nothing is checked out: git only lists and prints objects. git runs as every
/// store runs it ([`crate::git_mode::FIXED`], no hooks, cleared environment), as security note 1 of
/// `fad734d6b26e` asked.
pub fn tree_at(git: &Path, repo: &Path, rev: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    if !is_commit(rev) && rev != MAIN {
        return Err(format!("{} is neither main nor a commit or tree id", short(rev)));
    }
    let run = |argv: &[&str], stdin: Option<&[u8]>| -> Result<Vec<u8>, String> {
        let mut cmd = std::process::Command::new(git);
        cmd.env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(crate::git_mode::FIXED.iter().flat_map(|o| ["-c", o]))
            .arg(format!("--git-dir={}", repo.display()))
            .args(argv)
            .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd.spawn().map_err(|e| format!("starting git: {e}"))?;
        let out = crate::mode::communicate(child, stdin).map_err(|e| format!("running git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", argv[0], short(String::from_utf8_lossy(&out.stderr).trim())));
        }
        Ok(out.stdout)
    };
    let files = entries(&run(&["ls-tree", "-r", "-l", "-z", "--full-tree", "--end-of-options", rev], None)?)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let ids: String = files.iter().map(|(_, sha)| format!("{sha}\n")).collect();
    let raw = run(&["cat-file", "--batch"], Some(ids.as_bytes()))?;
    let mut out = Vec::new();
    let mut at = 0;
    for (path, sha) in files {
        let rest = raw.get(at..).ok_or("a cut cat-file output")?;
        let end = rest.iter().position(|&b| b == b'\n').ok_or("a cut cat-file header")? + at;
        let header = String::from_utf8_lossy(&raw[at..end]).into_owned();
        let size: usize = match header.split(' ').collect::<Vec<_>>()[..] {
            [id, "blob", size] if id == sha => size.parse().map_err(|_| "a bad cat-file size")?,
            _ => return Err(format!("{}: not the blob its tree names", short(&path))),
        };
        at = end + 1;
        out.push((path, raw.get(at..at + size).ok_or("a cut object")?.to_vec()));
        at += size + 1;
    }
    Ok(out)
}

pub struct GitProjectStore {
    git: GitStore,
}

impl GitProjectStore {
    fn dir(&self) -> PathBuf {
        self.git.config.dir.clone()
    }

    fn work(&self) -> PathBuf {
        self.dir().join("work")
    }

    fn hub_repo(&self) -> PathBuf {
        self.dir().join("hub.git")
    }

    fn is_hub(&self) -> bool {
        self.hub_repo().exists()
    }

    /// A JSON file of the store's own, or `default` if there's none.
    fn load(&self, file: &str, default: Value) -> Result<Value, String> {
        match std::fs::read_to_string(self.dir().join(file)) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{file}: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
            Err(e) => Err(format!("reading {file}: {e}")),
        }
    }

    /// Writes `file` whole or not at all, as Weave mode's store does.
    fn save(&self, file: &str, value: &Value) -> Result<(), String> {
        let (path, temp) = (self.dir().join(file), self.dir().join(format!("{file}.new")));
        std::fs::write(&temp, value.to_string())
            .and_then(|()| std::fs::rename(&temp, &path))
            .map_err(|e| format!("writing {file}: {e}"))
    }

    /// git in the copy's working tree; an error if it fails.
    fn wt(&mut self, argv: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let out = self.wt_out(argv, stdin)?;
        if out.ok {
            return Ok(out.stdout);
        }
        let said = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
        Err(format!("git {}: {}", argv.first().copied().unwrap_or(""), short(said.trim())))
    }

    fn wt_out(&mut self, argv: &[&str], stdin: Option<&[u8]>) -> Result<crate::git_mode::Output, String> {
        let work = self.work();
        self.git.git_in(&work, argv, stdin)
    }

    /// `refs/heads/<name>` or `refs/agents/<agent>/<name>` as git's `check-ref-format` takes it.
    fn checked(&mut self, full: &str) -> Result<(), String> {
        let repo = if self.is_hub() { self.hub_repo() } else { self.work() };
        // Always a full name under refs/, so never read as an option (check-ref-format takes no --end-of-options).
        if !full.starts_with("refs/") {
            return Err(format!("{} isn't a full ref", short(full)));
        }
        let out = self.git.git_in(&repo, &["check-ref-format", full], None)?;
        if out.ok { Ok(()) } else { Err(format!("{} isn't a name git takes for a branch", short(full))) }
    }

    /// A local branch an agent names, as a full ref: checked, whether or not it exists yet.
    fn local(&mut self, name: &str) -> Result<String, String> {
        if name != "main" {
            branch_name(name)?;
        }
        let full = format!("refs/heads/{name}");
        self.checked(&full)?;
        Ok(full)
    }

    /// The ref an agent names, as a full ref to a commit that exists here: `origin/main`,
    /// `origin/<agent>/<branch>`, or a branch of its own.
    fn resolve(&mut self, name: &str) -> Result<String, String> {
        let full = match name.strip_prefix("origin/") {
            Some("main") => ORIGIN_MAIN.to_string(),
            Some(rest) => {
                let (agent, branch) = rest.split_once('/').unwrap_or((rest, ""));
                if !agent_name(agent) {
                    return Err(format!("{}: give origin/main or origin/<agent>/<branch>", short(name)));
                }
                branch_name(branch)?;
                let full = format!("refs/remotes/origin/{agent}/{branch}");
                self.checked(&full)?;
                full
            }
            None => self.local(name)?,
        };
        match self.git.rev(&format!("{full}^{{commit}}"))? {
            Some(_) => Ok(full),
            None => Err(format!("no {} here: see status for the branches as of your last sync", short(name))),
        }
    }

    fn current_branch(&mut self) -> Result<Option<String>, String> {
        let out = self.wt_out(&["symbolic-ref", "--quiet", "HEAD"], None)?;
        Ok(out.ok.then(|| out.stdout.trim().trim_start_matches("refs/heads/").to_string()))
    }

    fn merging(&self) -> bool {
        self.work().join(".git/MERGE_HEAD").exists()
    }

    /// The paths a merge left in conflict.
    fn unmerged(&mut self) -> Result<Vec<String>, String> {
        let out = self.wt(&["diff", "--name-only", "--diff-filter=U", "-z"], None)?;
        Ok(out.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
    }

    /// Uncommitted changes, as `git status --porcelain` lists them.
    fn changes(&mut self) -> Result<Vec<String>, String> {
        let out = self.wt(&["status", "--porcelain=v1", "-z", "--untracked-files=all"], None)?;
        Ok(out.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
    }

    /// Branches under `prefix`, each name (less the prefix) to its commit.
    fn refs_under(&mut self, prefix: &str) -> Result<Vec<(String, String)>, String> {
        let listing = self.git.git(&["for-each-ref", "--format=%(objectname) %(refname)", prefix], None)?;
        Ok(listing
            .lines()
            .filter_map(|l| l.split_once(' '))
            .filter_map(|(sha, r)| Some((r.strip_prefix(prefix)?.trim_start_matches('/').to_string(), sha.to_string())))
            .collect())
    }

    fn is_ancestor(&mut self, a: &str, b: &str) -> Result<bool, String> {
        let repo = if self.is_hub() { self.hub_repo() } else { self.work() };
        Ok(self.git.git_in(&repo, &["merge-base", "--is-ancestor", "--end-of-options", a, b], None)?.ok)
    }

    fn data(&self) -> Result<Value, String> {
        self.load("project.json", Value::Null)
    }

    fn setting(&self) -> Result<String, String> {
        Ok(self.data()?["integrator"].as_str().unwrap_or(ANY_AGENT).to_string())
    }

    // The hub.

    /// The bare hub, its `main` one commit of the project's seed.
    fn setup_hub(&mut self) -> Result<Value, String> {
        let data = self.data()?;
        std::fs::create_dir_all(self.hub_repo()).map_err(|e| format!("creating the hub: {e}"))?;
        self.git.git(&["init", "--quiet", "--bare"], None)?;
        fsck_on_bundle_fetch(&self.git.git(&["version"], None)?)?;
        let seed = self.dir().join("seed");
        for pair in data["seed"].as_array().ok_or("the project data has no seed")? {
            let rel = pair[0].as_str().ok_or("a seed path isn't text")?;
            plain_path(rel)?;
            let bytes = hex::decode(pair[1].as_str().unwrap_or_default()).map_err(|e| e.to_string())?;
            let path = seed.join(rel);
            std::fs::create_dir_all(path.parent().expect("a path in the seed has a parent"))
                .map_err(|e| format!("creating the seed: {e}"))?;
            std::fs::write(&path, bytes).map_err(|e| format!("writing {rel}: {e}"))?;
        }
        let work = format!("--work-tree={}", seed.display());
        self.git.git(&[&work, "add", "--all", "--", "."], None)?;
        let tree = self.git.git(&["write-tree"], None)?;
        let commit = self.git.git(&["commit-tree", tree.trim()], Some(b"The project's seed"))?;
        self.git.git(&["update-ref", MAIN, commit.trim()], None)?;
        std::fs::remove_dir_all(&seed).map_err(|e| format!("clearing the seed: {e}"))?;
        std::fs::remove_file(self.hub_repo().join("index")).map_err(|e| format!("clearing the index: {e}"))?;
        self.save("released.json", &json!(false))?;
        let released: Vec<&Value> =
            data["items"].as_array().into_iter().flatten().filter(|i| i["later"] != true).map(|i| &i["id"]).collect();
        Ok(json!({"main": commit.trim(), "released": released}))
    }

    /// The spec and the items released so far, as statements ([`crate::weave_mode::item_statement`]).
    fn task_data(&self) -> Result<Value, String> {
        let data = self.data()?;
        let released = self.load("released.json", json!(false))? == true;
        let items: Vec<String> = data["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| released || i["later"] != true)
            .map(crate::weave_mode::item_statement)
            .collect();
        Ok(json!({"spec": data["spec"], "items": items}))
    }

    /// Makes `sha`, agent `agent`'s own main as it pushed it, the hub's main where it may: what the agent is told.
    fn update_main(&mut self, agent: &str, sha: &str) -> Result<Value, String> {
        let main = self.git.rev(MAIN)?.ok_or("the hub has no main")?;
        if sha == main {
            return Ok(json!({"main": "up to date"}));
        }
        if self.is_ancestor(sha, &main)? {
            return Ok(json!({"main": "not pushed: your main is behind origin/main"}));
        }
        let setting = self.setting()?;
        if !may_update_main(&setting, agent) {
            return Ok(json!({"main": format!(
                "refused: only {setting} updates main. Push your work on a branch of your own and ask for its integration"
            )}));
        }
        if !self.is_ancestor(&main, sha)? {
            return Ok(
                json!({"main": "refused: not a fast-forward of the hub's main, which has moved. Merge origin/main into your main, then sync again"}),
            );
        }
        self.git.git(&["update-ref", MAIN, sha, &main], None)?;
        Ok(json!({"main": "updated", "commit": sha}))
    }

    /// Requests that are still open: their branch at the hub isn't in main.
    fn open_requests(&mut self) -> Result<Vec<Value>, String> {
        let all = self.load("requests.json", json!([]))?;
        let heads: std::collections::BTreeMap<String, String> = self.refs_under("refs/agents")?.into_iter().collect();
        let merged =
            self.git.git(&["for-each-ref", "--merged=refs/heads/main", "--format=%(refname)", "refs/agents"], None)?;
        let merged: std::collections::BTreeSet<&str> = merged.lines().collect();
        let (mut open, mut kept) = (Vec::new(), Vec::new());
        for r in all.as_array().into_iter().flatten() {
            let (agent, branch) = (r["agent"].as_str().unwrap_or_default(), r["branch"].as_str().unwrap_or_default());
            let key = format!("{agent}/{branch}");
            let Some(commit) = heads.get(&key) else { continue };
            if merged.contains(format!("refs/agents/{key}").as_str()) {
                // Closed for good: later commits on the branch don't reopen it, only a new request does.
                continue;
            }
            kept.push(r.clone());
            open.push(
                json!({"from": agent, "branch": format!("origin/{key}"), "commit": commit, "summary": r["summary"]}),
            );
        }
        self.save("requests.json", &json!(kept))?;
        Ok(open)
    }

    // A copy.

    fn setup_copy(&mut self, bundle: &[u8]) -> Result<Value, String> {
        std::fs::create_dir_all(self.work()).map_err(|e| format!("creating the copy: {e}"))?;
        self.git.git(&["init", "--quiet"], None)?;
        self.git.take_bundle(bundle, None, |r| (r == MAIN).then(|| ORIGIN_MAIN.to_string()))?;
        self.wt(&["checkout", "--quiet", "-B", "main", "--no-track", ORIGIN_MAIN], None)?;
        Ok(json!({}))
    }

    fn status(&mut self) -> Result<Value, String> {
        let branch = self.current_branch()?;
        let counts = self.wt(&["rev-list", "--left-right", "--count", "HEAD...refs/remotes/origin/main"], None)?;
        let mut counts = counts.split_whitespace().map(|n| n.parse::<u64>().unwrap_or(0));
        let (ahead, behind) = (counts.next().unwrap_or(0), counts.next().unwrap_or(0));
        let mut changes = self.changes()?;
        let more = changes.len().saturating_sub(200);
        changes.truncate(200);
        let short_sha = |(n, s): (String, String)| json!({"branch": n, "commit": s[..12].to_string()});
        let mine: Vec<Value> = self.refs_under("refs/heads")?.into_iter().map(short_sha).collect();
        let hubs: Vec<Value> = self
            .refs_under("refs/remotes/origin")?
            .into_iter()
            .map(|(n, s)| short_sha((format!("origin/{n}"), s)))
            .collect();
        let setting = self.load("setting.json", json!(ANY_AGENT))?;
        let mut out = json!({
            "branch": branch,
            "merging": self.merging(),
            "conflicts": self.unmerged()?,
            "uncommitted": changes,
            "ahead_of_origin_main": ahead,
            "behind_origin_main": behind,
            "branches": mine,
            "origin": hubs,
            "main_updated_by": if setting == ANY_AGENT { json!("any agent") } else { setting },
        });
        if more > 0 {
            out["uncommitted_not_listed"] = json!(more);
        }
        Ok(out)
    }

    fn switch(&mut self, args: &Args) -> Result<Value, String> {
        let name = args.text("branch")?;
        let full = self.local(name)?;
        if self.merging() {
            return Err("a merge is in progress: commit it, or merge with abort".into());
        }
        if !self.changes()?.is_empty() {
            return Err("you have uncommitted changes: commit them first".into());
        }
        if args.bool_opt("create")? {
            let start = match args.text_opt("start")? {
                "" => self.resolve("origin/main")?,
                given => self.resolve(given)?,
            };
            if self.git.rev(&full)?.is_some() {
                return Err(format!("you already have a branch {}: switch to it without create", short(name)));
            }
            if self.refs_under("refs/heads")?.len() >= MAX_BRANCHES {
                return Err(format!("you have {MAX_BRANCHES} branches, the most a copy may have"));
            }
            self.wt(
                &["switch", "--quiet", "--no-track", &format!("--create={name}"), "--end-of-options", &start],
                None,
            )?;
            return Ok(
                json!({"branch": name, "from": start.trim_start_matches("refs/remotes/").trim_start_matches("refs/heads/")}),
            );
        }
        if !args.text_opt("start")?.is_empty() {
            return Err("start needs create".into());
        }
        if self.git.rev(&full)?.is_none() {
            return Err(format!("no branch {} of yours: give create to make it", short(name)));
        }
        self.wt(&["switch", "--quiet", "--end-of-options", name], None)?;
        Ok(json!({"branch": name}))
    }

    fn commit(&mut self, message: &str) -> Result<Value, String> {
        crate::args::check_text(message, MAX_MESSAGE)?;
        let branch = self.current_branch()?.ok_or("you are on no branch: switch to one first")?;
        let work = self.work();
        for path in self.unmerged()? {
            let text = std::fs::read(crate::weave_mode::tree_file(&work, &path)?).unwrap_or_default();
            let text = String::from_utf8_lossy(&text);
            if text.lines().any(|l| l.starts_with("<<<<<<<") || l.starts_with(">>>>>>>")) {
                return Err(format!("{} still holds conflict markers: settle them first", short(&path)));
            }
        }
        self.wt(&["add", "--all"], None)?;
        if !self.merging() && self.wt_out(&["diff", "--cached", "--quiet"], None)?.ok {
            return Err("nothing to commit: your working tree is as your branch's last commit".into());
        }
        self.wt(&["commit", "--quiet", "--no-verify", "--allow-empty-message", "--file=-"], Some(message.as_bytes()))?;
        Ok(json!({"branch": branch, "commit": self.git.rev("HEAD")?}))
    }

    fn merge(&mut self, args: &Args) -> Result<Value, String> {
        if args.bool_opt("abort")? {
            if !self.merging() {
                return Err("no merge is in progress".into());
            }
            self.wt(&["merge", "--abort"], None)?;
            return Ok(json!({"aborted": true}));
        }
        let name = args.text("ref")?;
        let full = self.resolve(name)?;
        self.current_branch()?.ok_or("you are on no branch: switch to one first")?;
        if self.merging() {
            return Err("a merge is in progress: commit it, or merge with abort".into());
        }
        if !self.changes()?.is_empty() {
            return Err("you have uncommitted changes: commit them first".into());
        }
        let out = self.wt_out(&["merge", "--quiet", "--no-edit", "--end-of-options", &full], None)?;
        if out.ok {
            return Ok(json!({"merged": true, "commit": self.git.rev("HEAD")?}));
        }
        let conflicts = self.unmerged()?;
        if conflicts.is_empty() {
            return Err(format!("git merge: {}", short(out.stderr.trim())));
        }
        Ok(json!({"merged": false, "conflicts": conflicts,
            "next": "settle each conflicted file (its conflict markers), then commit; or merge with abort"}))
    }

    /// The working tree, committed or not, against `against`, through an index of the store's own, so the
    /// agent's index and any merge in progress are untouched.
    /// The working tree against `against`; or, given `to`, what `to` changed since it parted from `against`
    /// (`against...to`), so an integrator reviews a requested branch's own changes without switching to it, not
    /// mixed with main's since (finding `a236e7206344`). Nothing of the working tree, its index or a merge changes.
    fn diff(&mut self, against: &str, to: &str, path: &str, page: usize) -> Result<Value, String> {
        let base = if against.is_empty() { "HEAD".to_string() } else { self.resolve(against)? };
        let target = if to.is_empty() { None } else { Some(self.resolve(to)?) };
        if !path.is_empty() {
            plain_path(path)?;
        }
        let shown = if against.is_empty() { "your branch's last commit" } else { against };
        let to_shown = if to.is_empty() { "your working tree" } else { to };
        let extra = json!({"against": shown, "to": to_shown});
        if let Some(target) = target {
            let files = self.diff_refs(None, &[&format!("{base}...{target}")], path)?;
            return crate::weave_mode::diff_page(&files, page, extra);
        }
        let index = self.dir().join("diff.index");
        let _ = std::fs::remove_file(&index);
        let files = self.diff_files(&index, &base, path);
        let _ = std::fs::remove_file(&index);
        crate::weave_mode::diff_page(&files?, page, extra)
    }

    /// `git diff <revs> -- <path>` per file, with `--cached` and the index at `index` if given.
    fn diff_refs(
        &mut self,
        index: Option<&Path>,
        revs: &[&str],
        path: &str,
    ) -> Result<Vec<crate::weave_mode::FileDiff>, String> {
        let work = self.work();
        let vars: Vec<(&str, &Path)> = index.map(|i| ("GIT_INDEX_FILE", i)).into_iter().collect();
        let mut run = |argv: &[String]| -> Result<String, String> {
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.git.git_in_env(&work, &argv, None, &vars)?;
            if out.ok { Ok(out.stdout) } else { Err(format!("git diff: {}", short(out.stderr.trim()))) }
        };
        let argv = |how: &[&str]| -> Vec<String> {
            let mut a: Vec<String> = ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            if index.is_some() {
                a.push("--cached".into());
            }
            a.extend(how.iter().map(|s| s.to_string()));
            a.push("--end-of-options".into());
            a.extend(revs.iter().map(|r| r.to_string()));
            a.push("--".into());
            a.extend([path].into_iter().filter(|p| !p.is_empty()).map(str::to_string));
            a
        };
        let status = run(&argv(&["--name-status", "-z"]))?;
        let patch = run(&argv(&[]))?;
        Ok(crate::weave_mode::split_diff(&status, &patch))
    }

    /// [`GitProjectStore::diff`]'s files, its index at `index`.
    fn diff_files(&mut self, index: &Path, base: &str, path: &str) -> Result<Vec<crate::weave_mode::FileDiff>, String> {
        let (work, vars) = (self.work(), [("GIT_INDEX_FILE", index)]);
        let mut run = |argv: &[&str]| -> Result<String, String> {
            let out = self.git.git_in_env(&work, argv, None, &vars)?;
            if out.ok { Ok(out.stdout) } else { Err(format!("git {}: {}", argv[0], short(out.stderr.trim()))) }
        };
        run(&["read-tree", "HEAD"])?;
        run(&["add", "--all"])?;
        self.diff_refs(Some(index), &[base], path)
    }

    fn log(&mut self, args: &Args) -> Result<Value, String> {
        let name = args.text_opt("ref")?;
        let full = if name.is_empty() { "HEAD".to_string() } else { self.resolve(name)? };
        let count = format!("--max-count={}", args.int_opt("count", 1..=50)?.unwrap_or(10));
        let log = self.wt(
            &["log", "--no-decorate", "--no-color", &count, "--format=%h %an: %s", "--end-of-options", &full],
            None,
        )?;
        Ok(json!({"log": log}))
    }

    fn request_integration(&mut self, branch: &str, summary: &str) -> Result<Value, String> {
        if branch == "main" {
            return Err("main is what integration merges into: put your work on a branch of your own".into());
        }
        let full = self.local(branch)?;
        let commit = self.git.rev(&full)?.ok_or_else(|| format!("no branch {} of yours", short(branch)))?;
        crate::args::check_text(summary, MAX_SUMMARY)?;
        if summary.trim().is_empty() {
            return Err("give a summary of what the branch does".into());
        }
        let mut pending: Vec<Value> = self.load("pending.json", json!([]))?.as_array().cloned().unwrap_or_default();
        pending.retain(|r| r["branch"] != branch);
        if pending.len() >= MAX_BRANCHES {
            return Err(format!("at most {MAX_BRANCHES} requests wait for a sync"));
        }
        pending.push(json!({"branch": branch, "summary": summary}));
        self.save("pending.json", &json!(pending))?;
        Ok(
            json!({"branch": branch, "commit": commit, "note": "Your next sync pushes the branch and sends the request."}),
        )
    }

    fn requests(&self) -> Result<Value, String> {
        let all: Vec<Value> = self.load("requests.json", json!([]))?.as_array().cloned().unwrap_or_default();
        let (mut shown, mut size) = (Vec::new(), 0);
        for r in &all {
            size += r.to_string().len() + 1;
            if size > crate::driver::MAX_RESULT_BYTES - 2048 {
                break;
            }
            shown.push(r.clone());
        }
        let mut out = json!({"requests": shown});
        if shown.len() < all.len() {
            out["not_shown"] = json!(all.len() - shown.len());
        }
        Ok(out)
    }

    fn task(&self, item: &str, page: usize) -> Result<Value, String> {
        let data = self.load("task.json", Value::Null)?;
        if data.is_null() {
            return Err("no task yet: sync first".into());
        }
        let items: Vec<String> =
            data["items"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        crate::weave_mode::task_view(data["spec"].as_str(), &items, (!item.is_empty()).then_some(item), page)
    }
}

/// Whether `v`, from the hub, is a task as [`GitProjectStore::task_data`] makes one.
fn task_shaped(v: &Value) -> bool {
    v["spec"].is_string() && v["items"].as_array().is_some_and(|i| i.iter().all(Value::is_string))
}

impl Store for GitProjectStore {
    fn project(&mut self, data: &Value) -> Result<Value, String> {
        std::fs::create_dir_all(self.dir()).map_err(|e| format!("creating the hub: {e}"))?;
        self.save("project.json", data)?;
        Ok(json!({"items": data["items"].as_array().map_or(0, Vec::len)}))
    }

    /// The hub needs no agent's key: its own rules name agents by the names the harness gives them.
    fn peers(&mut self, _keys: &[String]) -> Result<Value, String> {
        Ok(json!({"integrator": self.setting()?}))
    }

    fn releases(&self) -> Option<usize> {
        Some(usize::from(self.data().ok()?["change"] == true))
    }

    fn setup(&mut self, hub_export: Option<&[u8]>) -> Result<Value, String> {
        match hub_export {
            None => self.setup_hub(),
            Some(bundle) => self.setup_copy(bundle),
        }
    }

    /// The hub's main, whole: what a new copy starts from.
    fn export(&mut self) -> Result<Vec<u8>, String> {
        self.git.make_bundle(&[MAIN.into()], &[])
    }

    fn import(&mut self, _bytes: &[u8]) -> Result<Value, String> {
        Err("this store syncs by push and fetch".into())
    }

    /// Every local branch of this copy, as a bundle without what the hub has sent it, their tips, and the
    /// integration requests made since the last sync.
    fn push(&mut self) -> Result<Transfer, String> {
        // switch makes at most MAX_BRANCHES; main is always among those pushed, whatever else is (review note 4).
        let mut all = self.refs_under("refs/heads")?;
        all.sort_by_key(|(b, _)| b != "main");
        let branches: serde_json::Map<String, Value> =
            all.into_iter().take(MAX_BRANCHES).map(|(b, s)| (b, json!(s))).collect();
        let refs: Vec<String> = branches.keys().map(|b| format!("refs/heads/{b}")).collect();
        let bytes = self.git.make_bundle(&refs, &["--remotes=origin".to_string()])?;
        let have = self.git.git(&["for-each-ref", "--format=%(objectname)", "refs/remotes/origin"], None)?;
        let have: Vec<&str> = have.lines().take(MAX_HAVE).collect();
        let requests = self.load("pending.json", json!([]))?;
        Ok(Transfer { meta: json!({"branches": branches, "have": have, "requests": requests}), bytes })
    }

    /// One agent's branches into `refs/agents/<agent>/`, after the bundle's checks; its main, where it may
    /// update main; and its requests.
    fn receive(&mut self, session: &Session, push: Transfer) -> Result<Value, String> {
        let agent = session.name.clone();
        if !agent_name(&agent) {
            return Err("a session's agent name isn't agent-<n>".into());
        }
        self.git.charge(session, push.bytes.len())?;
        let branches = push.meta["branches"].as_object().cloned().unwrap_or_default();
        if branches.len() > MAX_BRANCHES {
            return Err(format!("at most {MAX_BRANCHES} branches a push"));
        }
        let mut requested = Vec::new();
        for r in push.meta["requests"].as_array().into_iter().flatten().take(MAX_BRANCHES) {
            let branch = r["branch"].as_str().filter(|b| *b != "main" && branches.contains_key(*b));
            let branch = branch.ok_or("a request for a branch the push doesn't hold")?;
            let summary = r["summary"].as_str().ok_or("a request without a summary")?;
            crate::args::check_text(summary, MAX_SUMMARY)?;
            requested.push((branch.to_string(), summary.to_string()));
        }
        let mut moves = Vec::new();
        for (b, sha) in &branches {
            let sha = sha.as_str().filter(|s| is_commit(s)).ok_or("a branch's commit isn't a commit id")?;
            let to = if b == "main" {
                format!("refs/incoming/{agent}/main")
            } else {
                branch_name(b)?;
                format!("refs/agents/{agent}/{b}")
            };
            self.checked(&to)?;
            moves.push((to, sha.to_string()));
        }
        if !push.bytes.is_empty() {
            // Each branch the bundle holds to where its push said it goes; any other is refused.
            let to: std::collections::BTreeMap<String, String> =
                branches.keys().zip(&moves).map(|(b, (to, _))| (format!("refs/heads/{b}"), to.clone())).collect();
            self.git.take_bundle(&push.bytes, Some(MAX_NEW_COMMITS), |r| to.get(r).cloned())?;
        }
        self.git.move_refs(&moves)?;
        let mut reports = self.load("pushed.json", json!({}))?;
        reports.as_object_mut().ok_or("pushed.json isn't an object")?.remove(&agent);
        if let Some(sha) = branches.get("main").and_then(Value::as_str) {
            let report = self.update_main(&agent, sha);
            self.git.git(&["update-ref", "-d", &format!("refs/incoming/{agent}/main")], None)?;
            reports[agent.as_str()] = report?;
        }
        self.save("pushed.json", &reports)?;
        let mut requests: Vec<Value> = self.load("requests.json", json!([]))?.as_array().cloned().unwrap_or_default();
        for (branch, summary) in requested {
            requests.retain(|o| !(o["agent"] == agent.as_str() && o["branch"] == branch.as_str()));
            requests.push(json!({"agent": agent, "branch": branch, "summary": summary}));
        }
        if requests.iter().filter(|r| r["agent"] == agent.as_str()).count() > MAX_BRANCHES {
            return Err(format!("at most {MAX_BRANCHES} requests an agent"));
        }
        self.save("requests.json", &json!(requests))?;
        Ok(json!({"branches": branches.len()}))
    }

    /// main and every agent's branches, without what the copy has; the task as released so far; the open
    /// requests; and what became of the agent's main, if it pushed one.
    fn send(&mut self, session: &Session, have: &Value) -> Result<Transfer, String> {
        let main = self.git.rev(MAIN)?.ok_or("the hub has no main")?;
        let mut refs = serde_json::Map::new();
        refs.insert("main".into(), json!(main));
        let mut names = vec![MAIN.to_string()];
        for (name, sha) in self.refs_under("refs/agents")? {
            names.push(format!("refs/agents/{name}"));
            refs.insert(name, json!(sha));
        }
        let asked: Vec<String> = have
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(MAX_HAVE)
            .map(str::to_string)
            .collect();
        let not: Vec<String> = self.git.commits_here(&asked)?.into_iter().collect();
        let bytes = self.git.make_bundle(&names, &not)?;
        let reports = self.load("pushed.json", json!({}))?;
        let meta = json!({
            "refs": refs,
            "task": self.task_data()?,
            "requests": self.open_requests()?,
            "integrator": self.setting()?,
            "main": reports[&session.name]["main"],
        });
        Ok(Transfer { meta, bytes })
    }

    /// The hub's main and branches into `refs/remotes/origin/`, after the bundle's checks, and the task and
    /// requests as the hub sent them.
    fn fetch(&mut self, answer: Transfer) -> Result<Value, String> {
        let origin = |name: &str| -> Option<String> {
            if name == "main" {
                return Some(ORIGIN_MAIN.into());
            }
            let (agent, branch) = name.split_once('/')?;
            (agent_name(agent) && branch_name(branch).is_ok()).then(|| format!("refs/remotes/origin/{name}"))
        };
        if !answer.bytes.is_empty() {
            self.git.take_bundle(&answer.bytes, None, |r| {
                if r == MAIN {
                    return origin("main");
                }
                origin(r.strip_prefix("refs/agents/")?)
            })?;
        }
        let refs = answer.meta["refs"].as_object().cloned().unwrap_or_default();
        let mut moves = Vec::new();
        for (name, sha) in &refs {
            let to = origin(name).ok_or_else(|| format!("refusing ref {}", short(name)))?;
            self.checked(&to)?;
            moves.push((to, sha.as_str().unwrap_or_default().to_string()));
        }
        self.git.move_refs(&moves)?;
        let task = &answer.meta["task"];
        if !task_shaped(task) {
            return Err("the hub sent no task".into());
        }
        self.save("task.json", task)?;
        let requests = answer.meta["requests"].as_array().cloned().unwrap_or_default();
        self.save("requests.json", &json!(requests))?;
        self.save("setting.json", &json!(answer.meta["integrator"].as_str().unwrap_or(ANY_AGENT)))?;
        self.save("pending.json", &json!([]))?;
        Ok(json!({"refs": refs.len(), "main": answer.meta["main"]}))
    }

    fn tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let work = self.work();
        match name {
            "task" => {
                let a = Args::new(args, &["item", "page"])?;
                let page = a.int_opt("page", 1..=100_000)?.unwrap_or(1) as usize;
                self.task(a.text_opt("item")?, page)
            }
            "list_files" => {
                Args::new(args, &[])?;
                let files = crate::weave_mode::tree_listing(&work)?;
                Ok(json!({"files": files.iter().map(|(p, n)| json!({"path": p, "bytes": n})).collect::<Vec<_>>()}))
            }
            "read_file" => crate::weave_mode::read_tree_file(&work, &Args::new(args, &["path", "offset", "limit"])?),
            "write_file" => {
                let a = Args::new(args, &["path", "text"])?;
                crate::weave_mode::write_tree_file(&work, a.text("path")?, a.text_up_to("text", MAX_FILE_BYTES)?)
            }
            "edit_file" => crate::weave_mode::edit_tree_file(&work, &Args::new(args, &["path", "old", "new"])?),
            "delete_file" => crate::weave_mode::delete_tree_file(&work, Args::new(args, &["path"])?.text("path")?),
            "status" => {
                Args::new(args, &[])?;
                self.status()
            }
            "switch" => self.switch(&Args::new(args, &["branch", "create", "start"])?),
            "commit" => self.commit(Args::new(args, &["message"])?.text_up_to("message", MAX_MESSAGE)?),
            "merge" => self.merge(&Args::new(args, &["ref", "abort"])?),
            "diff" => {
                let a = Args::new(args, &["against", "to", "path", "page"])?;
                let page = a.int_opt("page", 1..=100_000)?.unwrap_or(1) as usize;
                self.diff(a.text_opt("against")?, a.text_opt("to")?, a.text_opt("path")?, page)
            }
            "log" => self.log(&Args::new(args, &["ref", "count"])?),
            "request_integration" => {
                let a = Args::new(args, &["branch", "summary"])?;
                self.request_integration(a.text("branch")?, a.text_up_to("summary", MAX_SUMMARY)?)
            }
            "requests" => {
                Args::new(args, &[])?;
                self.requests()
            }
            other => Err(format!("no tool {}", short(other))),
        }
    }

    fn tree_files(&mut self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let work = self.work();
        let mut out = Vec::new();
        for (path, _) in crate::weave_mode::tree_listing(&work)? {
            let file = crate::weave_mode::tree_file(&work, &path)?;
            out.push((path, std::fs::read(&file).map_err(|e| e.to_string())?));
        }
        Ok(out)
    }

    /// The one release after the start: the change. Agents see it at their next sync.
    fn release_changes(&mut self, _release: usize) -> Result<Value, String> {
        self.save("released.json", &json!(true))?;
        let data = self.data()?;
        let later: Vec<&Value> =
            data["items"].as_array().into_iter().flatten().filter(|i| i["later"] == true).map(|i| &i["id"]).collect();
        Ok(json!({"released": later}))
    }

    /// main's commit and tree: what a checkpoint's tree is, as `decision` and `manifest`, the names Weave's
    /// line uses, so grades read alike. A hub has one main, so it is never forked.
    fn line_head(&mut self) -> Result<Value, String> {
        let commit = self.git.rev(MAIN)?.ok_or("the hub has no main")?;
        let tree = self.git.rev(&format!("{commit}^{{tree}}"))?.ok_or("main has no tree")?;
        Ok(json!({"forked": false, "heads": [{"decision": commit, "manifest": tree}]}))
    }

    /// main's files as `(path, text)` pairs.
    fn integrated(&mut self) -> Result<Vec<(String, String)>, String> {
        let files = tree_at(&self.git.config.git.clone(), &self.hub_repo(), MAIN)?;
        Ok(files.into_iter().map(|(p, b)| (p, String::from_utf8_lossy(&b).into_owned())).collect())
    }
}

#[cfg(test)]
mod tests;
