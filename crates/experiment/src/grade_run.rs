//! Grading a project run after it ends (answer `07959d6d9bc0`): each checkpoint's integrated tree, and the tree
//! at the end, checked out of the hub's Ground by the manifest id its event names (finding `01e0ce2648ad`) and
//! graded with the project's hidden set by [`crate::project_grader`]. The private store is named
//! only on the command line; the grades go to a file of their own, outside the run, the project, the store, any
//! Ground and any git checkout, and nothing reads them back into a run. A grade names groups and tests by index
//! alone.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::plan::Plan;
use crate::project::HiddenSet;
use crate::project_grader::{Grade, Tree, grade};
use crate::python::{Python, Scratch};

/// One graded point of a run, a line of the grades file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    /// The checkpoint's turn, or `"end"`.
    pub at: Value,
    /// The share of the run's turns done by then: the checkpoint's turn over the run's, 1 at the end.
    pub turn_share: f64,
    /// Whether the project's change had gone out by then, and so its groups apply (the grade's share is 1),
    /// or not (0).
    pub change_released: bool,
    /// The tree graded: the line's decision and its manifest id, the first head's on a forked line.
    pub decision: String,
    pub manifest: String,
    /// Whether the line was forked then: other heads weren't graded.
    pub forked: bool,
    pub grade: Grade,
}

/// What grading a run did, with nothing of the hidden set but counts.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub project: String,
    pub hidden: String,
    pub points: usize,
    /// Points whose tree passes all that applies to it.
    pub all_pass: usize,
}

/// Grades the project run in `dir` with the hidden set in `store`, writing one [`Point`] a line to `grades`, a
/// new file. `bracken` checks each tree out of the run's hub.
pub fn grade_run(dir: &Path, store: &Path, grades: &Path, bracken: &Path) -> Result<Summary, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let plan_bytes = read(&dir.join("plan.json"))?;
    let plan: Plan = serde_json::from_slice(&plan_bytes).map_err(|e| format!("the plan: {e}"))?;
    let project_plan = plan.inputs.project.as_ref().ok_or("not a project run: its plan names no project")?;
    let pin = plan.inputs.python.as_ref().ok_or("the plan has no Python pin")?;
    let project = project_plan.load()?;
    check_grades_path(grades, &[("the run", dir), ("the project", &project.dir), ("the private store", store)])?;
    let set = HiddenSet::open(&project, store)?;
    let python = Python::from_plan(pin, &project.python)?;
    let events = String::from_utf8(read(&dir.join("events.jsonl"))?).map_err(|e| format!("the events: {e}"))?;
    let events: Vec<Value> = events
        .lines()
        .map(|l| serde_json::from_str(l).map_err(|e| format!("an event that isn't JSON: {e}")))
        .collect::<Result<_, _>>()?;
    let logged = events.iter().find(|e| e["event"] == "setup").and_then(|e| e["data"]["plan"].as_str());
    if logged != Some(blake3::hash(&plan_bytes).to_hex().as_str()) {
        return Err("plan.json isn't the plan the run logged at setup".into());
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(grades)
        .map_err(|e| format!("creating {}: {e}", grades.display()))?;
    let hub = dir.join("hub").join(".bracken");
    let (mut points, mut all_pass, mut graded) = (0, 0, BTreeMap::<(String, bool), Grade>::new());
    for logged in trees(&events, plan.turns)? {
        let key = (logged.manifest.clone(), logged.change_released);
        let g = match graded.get(&key) {
            // The same tree under the same groups: the grade it already has.
            Some(g) => g.clone(),
            None => {
                let scratch = Scratch::new()?;
                let tree = scratch.dir.join("tree");
                checkout(bracken, &hub, &logged.manifest, &tree)?;
                let share = if logged.change_released { 1.0 } else { 0.0 };
                let g = grade(&python, &project, &set, Tree::Dir(&tree), share)?;
                graded.insert(key, g.clone());
                g
            }
        };
        let turn_share = match &logged.at {
            Value::Number(n) => n.as_f64().unwrap_or(0.0) / f64::from(plan.turns.max(1)),
            _ => 1.0,
        };
        all_pass += usize::from(g.all_pass());
        let point = Point {
            at: logged.at,
            turn_share,
            change_released: logged.change_released,
            decision: logged.decision,
            manifest: logged.manifest,
            forked: logged.forked,
            grade: g,
        };
        writeln!(out, "{}", serde_json::to_string(&point).expect("a point is plain JSON"))
            .map_err(|e| format!("writing {}: {e}", grades.display()))?;
        points += 1;
    }
    out.sync_all().map_err(|e| format!("writing {}: {e}", grades.display()))?;
    Ok(Summary { project: project.name.clone(), hidden: project.hidden.id.clone(), points, all_pass })
}

/// A tree a run logged: where (a checkpoint's turn, or `"end"`), whether the change had gone out before it, and
/// the line's first head then, and whether it was forked.
struct Logged {
    at: Value,
    change_released: bool,
    decision: String,
    manifest: String,
    forked: bool,
}

/// The trees a run logged to grade, in order: each checkpoint's, then the line at the end, as the final
/// `integrated` event names it (or, for a run that stopped early, the last tree logged).
fn trees(events: &[Value], turns: u32) -> Result<Vec<Logged>, String> {
    let logged = |at: Value, released: bool, e: &Value| -> Result<Logged, String> {
        let line = &e["data"]["line"];
        let head = &line["heads"][0];
        let (Some(decision), Some(manifest)) = (head["decision"].as_str(), head["manifest"].as_str()) else {
            return Err(format!("a {} event names no head of the line", e["event"].as_str().unwrap_or("?")));
        };
        let hex = manifest.strip_prefix("blake3:").unwrap_or_default();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("an event's manifest isn't blake3:<64 hex digits>".into());
        }
        crate::args::check_id(decision)?;
        Ok(Logged {
            at,
            change_released: released,
            decision: decision.to_string(),
            manifest: manifest.to_string(),
            forked: line["forked"] == true,
        })
    };
    let (mut out, mut released, mut last) = (Vec::new(), false, None);
    for e in events {
        match e["event"].as_str() {
            Some("changes_released") => released = true,
            Some("checkpoint") => {
                let turn = e["data"]["turn"].as_u64().filter(|t| *t >= 1 && *t <= u64::from(turns));
                let turn = turn.ok_or("a checkpoint with no turn of the run")?;
                out.push(logged(json!(turn), released, e)?);
                last = Some((released, e));
            }
            Some("snapshot" | "integrated") => last = Some((released, e)),
            _ => {}
        }
    }
    let (released, e) = last.ok_or("the run logged no tree to grade")?;
    out.push(logged(json!("end"), released, e)?);
    Ok(out)
}

/// Writes the manifest `manifest` of the hub's Ground `hub` into `to`, a new directory, with `bracken`.
fn checkout(bracken: &Path, hub: &Path, manifest: &str, to: &Path) -> Result<(), String> {
    std::fs::create_dir(to).map_err(|e| format!("making the tree to grade: {e}"))?;
    let out = std::process::Command::new(bracken)
        .arg("--dir")
        .arg(hub)
        .args(["weave", "checkout", &format!("--manifest={manifest}")])
        .arg(to)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("running bracken: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("checking out {manifest} from the run's hub: {}", crate::args::short(err.trim())));
    }
    Ok(())
}

/// Refuses a grades file that exists, or would be inside any of `inside` (the run, the project, the store, each
/// named by what it is, so no error says the store's path), a Ground or a git checkout: grades are kept apart
/// from everything a run or an agent reads.
pub fn check_grades_path(grades: &Path, inside: &[(&str, &Path)]) -> Result<(), String> {
    if std::fs::symlink_metadata(grades).is_ok() {
        return Err(format!("{} already exists; grades go to a new file", grades.display()));
    }
    let name = grades.file_name().ok_or_else(|| format!("{} names no file", grades.display()))?;
    let parent = grades.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let real: PathBuf = parent.join(name);
    for (what, dir) in inside {
        let dir = std::fs::canonicalize(dir).map_err(|e| format!("{what}: {e}"))?;
        if real.starts_with(&dir) {
            return Err(format!(
                "the grades file is inside {what}; grades go outside the run, its project and the store"
            ));
        }
    }
    let ground = |p: &Path| {
        p.file_name().is_some_and(|n| n == ".bracken")
            || p.join("ground.json").exists()
            || p.join(".bracken").join("ground.json").exists()
    };
    if let Some(g) = parent.ancestors().find(|p| ground(p)) {
        return Err(format!(
            "{} is inside the Ground at {}; grades go outside every Ground",
            real.display(),
            g.display()
        ));
    }
    let advice = bracken_demo::safety::Advice { checkout: "write grades outside any repository", env: "unset it" };
    bracken_demo::safety::refuse_git_checkout(&parent, false, &advice).map_err(|e| e.to_string())
}
