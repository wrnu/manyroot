//! The hub's pull requests (design section 2, Mode 1; decision `6b13e672e7d4`).
//!
//! A PR is one agent's branch, as last pushed to `refs/agents/<agent>/<branch>`.
//! It merges into `main`, as a `--no-ff` merge the hub makes, when:
//!
//! 1. CI passes on the merge result: every module parses, only module files
//!    change, every function that passes its public cases on `main` still
//!    does, and every function the PR changes does, against the cases of the
//!    hub's released version (missing functions are skipped);
//! 2. someone other than the author approved the PR's current head; and
//! 3. no reviewer's latest review requests changes. Each reviewer counts once,
//!    by their latest review, and the author never counts.
//!
//! A merge that conflicts is refused and the PR stays open.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{GitStore, definitions};
use crate::args::{Args, is_commit};
use crate::lang::Library;
use crate::mode::Session;

pub const VERDICTS: [&str; 2] = ["approve", "request_changes"];
/// The most PRs one agent may have open at once (finding `f582d00f535f`).
pub const MAX_OPEN_PRS: usize = 4;
/// The most PRs one agent may open over a run.
pub const MAX_PRS: usize = 32;
/// The longest PR title, and the longest body, comment or review text, in bytes.
pub const MAX_TITLE: usize = 200;
pub const MAX_REMARK: usize = 4000;
/// The most comments and reviews, together, one agent may add to one PR.
pub const MAX_REMARKS: usize = 8;
/// The most PR text one agent may store over a run, as the hub's record writes it (JSON-escaped)
/// (finding `7d119d53922f`).
pub const MAX_TEXT_PER_AGENT: usize = 128 * 1024;
/// The most comments, and reviews, `pr_view` shows: the latest.
const VIEW_LATEST: usize = 20;
/// The most bytes of a diff `pr_view` shows.
const MAX_DIFF: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub who: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub who: String,
    pub verdict: String,
    /// The PR head it was given on.
    pub head: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pr {
    pub number: u32,
    pub author: String,
    pub branch: String,
    pub title: String,
    pub body: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub merged_as: Option<String>,
}

impl Pr {
    /// Each reviewer's latest review, the author's left out.
    fn latest(&self) -> BTreeMap<&str, &Review> {
        let mut out = BTreeMap::new();
        for r in self.reviews.iter().filter(|r| r.who != self.author) {
            out.insert(r.who.as_str(), r);
        }
        out
    }

    /// Whether someone else's latest review approves `head`.
    pub fn approved(&self, head: &str) -> bool {
        self.latest().values().any(|r| r.verdict == "approve" && r.head == head)
    }

    /// Reviewers whose latest review requests changes.
    pub fn blocked_by(&self) -> Vec<String> {
        self.latest().values().filter(|r| r.verdict == "request_changes").map(|r| r.who.clone()).collect()
    }
}

/// The hub's PR records, and each agent's pushed bytes, in `prs.json` in the hub's store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prs {
    pub prs: Vec<Pr>,
}

/// Open PRs: number, author, branch.
pub type OpenHeads = Vec<(u32, String, String)>;

impl Prs {
    pub fn load(dir: &Path) -> Result<Prs, String> {
        let text = std::fs::read_to_string(dir.join("prs.json")).map_err(|e| format!("reading the PRs: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("the PRs file: {e}"))
    }

    /// Writes the records, and beside them the open PRs alone, which is all a sync reads.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("prs.json"), text).map_err(|e| format!("writing the PRs: {e}"))?;
        let open = serde_json::to_string(&self.open_heads()).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("open-prs.json"), open).map_err(|e| format!("writing the open PRs: {e}"))
    }

    pub fn open_heads(&self) -> OpenHeads {
        self.prs.iter().filter(|p| p.state == "open").map(|p| (p.number, p.author.clone(), p.branch.clone())).collect()
    }

    /// The open PRs, without reading any PR's text.
    pub fn load_open(dir: &Path) -> Result<OpenHeads, String> {
        let text =
            std::fs::read_to_string(dir.join("open-prs.json")).map_err(|e| format!("reading the open PRs: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("the open PRs file: {e}"))
    }

    /// The PR text `who` has stored, as the record writes it.
    fn text_of(&self, who: &str) -> usize {
        let len = |t: &str| serde_json::to_string(t).map_or(t.len(), |j| j.len());
        self.prs
            .iter()
            .map(|p| {
                let own = if p.author == who { len(&p.title) + len(&p.body) } else { 0 };
                let comments: usize = p.comments.iter().filter(|c| c.who == who).map(|c| len(&c.text)).sum();
                let reviews: usize = p.reviews.iter().filter(|r| r.who == who).map(|r| len(&r.text)).sum();
                own + comments + reviews
            })
            .sum()
    }

    /// Refuses `text` from `who` on PR `number` past the caps on remarks and stored text.
    fn may_add(&self, who: &str, number: i64, text: &str) -> Result<(), String> {
        if let Some(p) = self.prs.iter().find(|p| i64::from(p.number) == number) {
            let remarks =
                p.comments.iter().filter(|c| c.who == who).count() + p.reviews.iter().filter(|r| r.who == who).count();
            if remarks >= MAX_REMARKS {
                return Err(format!("you have added {MAX_REMARKS} comments and reviews to PR #{number}"));
            }
        }
        let len = serde_json::to_string(text).map_or(text.len(), |j| j.len());
        if self.text_of(who) + len > MAX_TEXT_PER_AGENT {
            return Err("your PR text for this run is spent".into());
        }
        Ok(())
    }

    fn get(&mut self, number: i64) -> Result<&mut Pr, String> {
        self.prs.iter_mut().find(|p| i64::from(p.number) == number).ok_or_else(|| format!("no PR #{number}"))
    }
}

/// A hub tool's arguments, checked by their rules, in the form the hub takes.
pub fn checked(name: &str, args: &Args) -> Result<Value, String> {
    let number = || args.int("number", 1..=100_000);
    Ok(match name {
        "pr_open" => {
            let (title, body) = (args.text("title")?, args.text_opt("body")?);
            limit("title", title, MAX_TITLE)?;
            limit("body", body, MAX_REMARK)?;
            json!({"branch": args.branch("branch")?, "title": title, "body": body})
        }
        "pr_list" => json!({}),
        "pr_view" | "pr_merge" | "pr_close" => json!({"number": number()?}),
        "pr_comment" => {
            let text = args.text("text")?;
            limit("text", text, MAX_REMARK)?;
            json!({"number": number()?, "text": text})
        }
        "pr_review" => {
            let text = args.text_opt("text")?;
            limit("text", text, MAX_REMARK)?;
            json!({"number": number()?, "verdict": args.choice("verdict", &VERDICTS)?, "text": text})
        }
        other => return Err(format!("no hub tool {}", crate::args::short(other))),
    })
}

fn limit(key: &str, v: &str, max: usize) -> Result<(), String> {
    if v.len() > max { Err(format!("`{key}` holds at most {max} bytes")) } else { Ok(()) }
}

/// A PR's text for a reply: its title and body, and its latest comments and reviews.
fn shown(p: &Pr) -> Value {
    let latest = |n: usize| n.saturating_sub(VIEW_LATEST);
    json!({
        "number": p.number, "author": p.author, "branch": p.branch, "title": p.title, "body": p.body,
        "state": p.state, "merged_as": p.merged_as,
        "comments": &p.comments[latest(p.comments.len())..], "comments_total": p.comments.len(),
        "reviews": &p.reviews[latest(p.reviews.len())..], "reviews_total": p.reviews.len(),
    })
}

/// CI on the merge of a PR's head into `main`.
#[derive(Debug, Clone)]
pub struct Ci {
    pub pass: bool,
    pub details: Vec<String>,
    /// The merge result's tree, if the merge doesn't conflict.
    pub tree: Option<String>,
}

/// Runs CI on the merge result of `head` into `main` (both commit ids the hub holds).
pub fn ci(store: &mut GitStore, main: &str, head: &str) -> Result<Ci, String> {
    let key = (main.to_string(), head.to_string());
    if let Some(done) = store.ci_cache.get(&key) {
        return Ok(done.clone());
    }
    let result = run_ci(store, main, head)?;
    store.ci_cache.insert(key, result.clone());
    Ok(result)
}

fn run_ci(store: &mut GitStore, main: &str, head: &str) -> Result<Ci, String> {
    let bare = store.bare();
    let out = store.git_in(&bare, &["merge-tree", "--write-tree", "--name-only", "--no-messages", main, head], None)?;
    let first = out.stdout.lines().next().unwrap_or_default().to_string();
    if !is_commit(&first) {
        return Err(format!("git merge-tree: {}", crate::args::short(out.stderr.trim())));
    }
    if !out.ok {
        let files: Vec<&str> = out.stdout.lines().skip(1).filter(|l| !l.is_empty()).collect();
        let details =
            vec![format!("conflicts with main in {}: merge main into the branch, then sync", files.join(", "))];
        return Ok(Ci { pass: false, details, tree: None });
    }
    let tree = first;
    if let Some(problem) = store.tree_problem(&tree)? {
        return Ok(Ci { pass: false, details: vec![problem], tree: None });
    }
    let mut details = Vec::new();
    let changed = store.git(&["diff-tree", "-r", "--name-only", main, &tree], None)?;
    let task = store.config.task;
    for file in changed.lines().filter(|f| !task.module_files().contains(f)) {
        details.push(format!("changes {}, which only the setter may", crate::args::short(file)));
    }
    let version = store.hub_version()?;
    let (before, after) = (store.modules_at(main)?, store.modules_at(&tree)?);
    let parse = |m: &[(String, String)]| Library::parse(m.iter().map(|(n, t)| (n.as_str(), t.as_str())));
    let (old, new) = (parse(&before), parse(&after));
    for (module, e) in &new.broken {
        details.push(format!("module {module} doesn't parse: {e}"));
    }
    for name in &new.duplicates {
        details.push(format!("`{name}` is defined more than once"));
    }
    let (old_defs, new_defs) = (definitions(task, &before), definitions(task, &after));
    let passes = |lib: &Library, name: &str| task.check(lib, name, version).is_ok_and(|c| c.ok());
    for s in task.specs() {
        let (was, is) = (passes(&old, s.name), passes(&new, s.name));
        if was && !is {
            details.push(format!("`{}` passes on main and fails after the merge", s.name));
        } else if !is && new_defs.contains_key(s.name) && new_defs.get(s.name) != old_defs.get(s.name) {
            details.push(format!("`{}` is changed and fails its public cases", s.name));
        }
    }
    Ok(Ci { pass: details.is_empty(), details, tree: Some(tree) })
}

/// Runs hub tool `name` for the agent of `session`, with checked `args`.
pub fn run(store: &mut GitStore, session: &Session, name: &str, args: &Value) -> Result<Value, String> {
    let me = super::agent_ref(session)?.to_string();
    let dir = store.config.dir.clone();
    let mut prs = Prs::load(&dir)?;
    let number = args["number"].as_i64().unwrap_or_default();
    let result = match name {
        "pr_open" => {
            let branch = args["branch"].as_str().unwrap_or_default().to_string();
            if store.rev(&format!("refs/agents/{me}/{branch}"))?.is_none() {
                return Err("the hub has no such branch of yours: sync first".into());
            }
            if prs.prs.iter().any(|p| p.state == "open" && p.author == me && p.branch == branch) {
                return Err("that branch already has an open PR".into());
            }
            if prs.prs.iter().filter(|p| p.author == me).count() >= MAX_PRS {
                return Err(format!("you have opened {MAX_PRS} PRs this run"));
            }
            let title = args["title"].as_str().unwrap_or_default();
            prs.may_add(&me, 0, &format!("{title}{}", args["body"].as_str().unwrap_or_default()))?;
            if prs.prs.iter().filter(|p| p.state == "open" && p.author == me).count() >= MAX_OPEN_PRS {
                return Err(format!("you have {MAX_OPEN_PRS} PRs open: merge or close one first"));
            }
            let head = format!("refs/agents/{me}/{branch}");
            if store.git(&["rev-list", "--count", &head, "--not", "refs/heads/main"], None)?.trim() == "0" {
                return Err("the branch has no commits that main doesn't".into());
            }
            let n = prs.prs.len() as u32 + 1;
            prs.prs.push(Pr {
                number: n,
                author: me,
                branch,
                title: args["title"].as_str().unwrap_or_default().into(),
                body: args["body"].as_str().unwrap_or_default().into(),
                state: "open".into(),
                comments: Vec::new(),
                reviews: Vec::new(),
                merged_as: None,
            });
            json!({ "number": n })
        }
        "pr_list" => {
            let main = store.rev("refs/heads/main")?.ok_or("the hub has no main")?;
            let heads = store.agent_heads()?;
            let mut out = Vec::new();
            for p in prs.prs.clone() {
                if p.state != "open" {
                    // Closed and merged PRs, summarised: their text is in pr_view.
                    out.push(json!({"number": p.number, "author": p.author, "state": p.state}));
                    continue;
                }
                let mut row = json!({"number": p.number, "author": p.author, "branch": p.branch, "title": p.title, "state": p.state});
                if p.state == "open"
                    && let Some(head) = heads.get(&format!("{}/{}", p.author, p.branch)).cloned()
                {
                    row["ci"] = json!(ci(store, &main, &head)?.pass);
                    row["head"] = json!(head);
                    row["approved"] = json!(p.approved(&head));
                    row["blocked_by"] = json!(p.blocked_by());
                }
                out.push(row);
            }
            json!(out)
        }
        "pr_view" => {
            let p = prs.get(number)?.clone();
            let main = store.rev("refs/heads/main")?.ok_or("the hub has no main")?;
            let mut view = shown(&p);
            if p.state == "open"
                && let Some(head) = head_of(store, &p)?
            {
                view["head"] = json!(head);
                // Nothing is read or diffed from a head whose trees aren't all allowed.
                if let Some(problem) = store.history_problem(&head, &main)? {
                    view["ci"] = json!({"pass": false, "details": [problem]});
                    view["approved"] = json!(p.approved(&head));
                    view["blocked_by"] = json!(p.blocked_by());
                    return Ok(view);
                }
                let run = ci(store, &main, &head)?;
                let base = store.git(&["merge-base", &main, &head], None)?;
                let mut diff = store
                    .git(&["diff", "--no-color", "--no-ext-diff", "--no-textconv", base.trim(), &head, "--"], None)?;
                if diff.len() > MAX_DIFF {
                    let cut = (0..=MAX_DIFF).rev().find(|&i| diff.is_char_boundary(i)).unwrap_or(0);
                    diff.truncate(cut);
                    diff.push_str("\n[diff cut]\n");
                }
                view["head"] = json!(head);
                view["ci"] = json!({"pass": run.pass, "details": run.details});
                view["approved"] = json!(p.approved(&head));
                view["blocked_by"] = json!(p.blocked_by());
                view["diff"] = json!(diff);
            }
            view
        }
        "pr_comment" => {
            let text = args["text"].as_str().unwrap_or_default();
            prs.get(number)?;
            prs.may_add(&me, number, text)?;
            let p = prs.get(number)?;
            p.comments.push(Comment { who: me, text: text.into() });
            json!({ "comments": p.comments.len() })
        }
        "pr_review" => {
            let p = prs.get(number)?.clone();
            if p.author == me {
                return Err("you can't review your own PR".into());
            }
            if p.state != "open" {
                return Err(format!("PR #{number} is {}", p.state));
            }
            let head = head_of(store, &p)?.ok_or("the PR's branch is gone")?;
            prs.may_add(&me, number, args["text"].as_str().unwrap_or_default())?;
            let review = Review {
                who: me,
                verdict: args["verdict"].as_str().unwrap_or_default().into(),
                head: head.clone(),
                text: args["text"].as_str().unwrap_or_default().into(),
            };
            prs.get(number)?.reviews.push(review);
            json!({ "reviewed": head })
        }
        "pr_merge" => merge(store, &mut prs, number)?,
        "pr_close" => {
            let p = prs.get(number)?;
            if p.author != me {
                return Err("only the PR's author can close it".into());
            }
            if p.state != "open" {
                return Err(format!("PR #{number} is {}", p.state));
            }
            p.state = "closed".into();
            json!({ "closed": number })
        }
        other => return Err(format!("no hub tool {}", crate::args::short(other))),
    };
    prs.save(&dir)?;
    Ok(result)
}

/// The task functions PR `number`'s head defines differently from where it left main, for the measures.
pub fn changed(store: &mut GitStore, number: i64) -> Result<Vec<(String, String)>, String> {
    let p = Prs::load(&store.config.dir.clone())?.get(number)?.clone();
    let Some(head) = head_of(store, &p)? else { return Ok(Vec::new()) };
    let base = store.git(&["merge-base", "refs/heads/main", &head], None)?.trim().to_string();
    let task = store.config.task;
    let before = crate::metrics::definitions(task, &store.modules_at(&base)?);
    let after = crate::metrics::definitions(task, &store.modules_at(&head)?);
    Ok(after.into_iter().filter(|(f, d)| before.get(f) != Some(d)).collect())
}

/// The PR's current head: its author's branch at the hub.
fn head_of(store: &mut GitStore, p: &Pr) -> Result<Option<String>, String> {
    store.rev(&format!("refs/agents/{}/{}", p.author, p.branch))
}

fn merge(store: &mut GitStore, prs: &mut Prs, number: i64) -> Result<Value, String> {
    let p = prs.get(number)?.clone();
    if p.state != "open" {
        return Err(format!("PR #{number} is {}", p.state));
    }
    let head = head_of(store, &p)?.ok_or("the PR's branch is gone")?;
    if !p.approved(&head) {
        return Err("no one else has approved the PR's current head".into());
    }
    let blocked = p.blocked_by();
    if !blocked.is_empty() {
        return Err(format!("changes requested by {}", blocked.join(", ")));
    }
    let main = store.rev("refs/heads/main")?.ok_or("the hub has no main")?;
    let run = ci(store, &main, &head)?;
    let Some(tree) = run.tree.filter(|_| run.pass) else {
        return Err(format!("CI fails: {}", run.details.join("; ")));
    };
    let message = format!("Merge PR #{number} from {}/{}\n\n{}\n", p.author, p.branch, p.title);
    let commit = store.git(&["commit-tree", "-p", &main, "-p", &head, &tree], Some(message.as_bytes()))?;
    let commit = commit.trim().to_string();
    store.git(&["update-ref", "refs/heads/main", &commit, &main], None)?;
    store.git(&["update-ref", "-d", &format!("refs/pr/{number}")], None)?;
    let pr = prs.get(number)?;
    pr.state = "merged".into();
    pr.merged_as = Some(commit.clone());
    Ok(json!({ "merged": number, "main": commit }))
}
