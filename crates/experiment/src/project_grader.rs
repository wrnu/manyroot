//! The grader for evaluation projects (`docs/design/eval-suite.md`, Grading): runs a project's hidden tests on a
//! tree and counts their outcomes per item, without the tests or the code under test ever meeting.
//!
//! Each group's test file runs in its own **host**: the pinned interpreter under the Python profile, with a
//! scratch directory holding the hidden set's `tests/` as read and digested, and nothing of the code under test.
//! The host never imports or runs that code. Importing the package gives a proxy, and each use of it comes here
//! as a line, which goes on to the **server**: the interpreter under the same profile, in another scratch
//! directory, with the tree. A `python -m <package>` a test starts comes here too, and runs in a fresh child with
//! the tree and the test's working files. So agent code runs only in the profile, with no way to the hidden
//! tests (note 9891289fb39c), and its values come back as data the host compares, or as handles that equal
//! only themselves.
//!
//! What it reports is counts per item and per group, by index: no test's name, source, expected value or
//! message, and nothing the host printed.

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::project::{HiddenSet, Limits, Project};
use crate::python::{Exit, Line, POLL, Python, Running, Scratch};

const HOST: &str = include_str!("project_grader/host.py");
const SERVER: &str = include_str!("project_grader/server.py");
const CLI: &str = include_str!("project_grader/cli.py");
/// The host's CPU and wall time are the project's times this: it runs a whole file, waiting on every call.
const HOST_TIMES: u32 = 4;
/// What the host may print (its own test runner's output, discarded) and use.
const HOST_OUTPUT: u64 = 16 << 20;
const HOST_MEMORY_MB: u32 = 1024;
/// The most bytes of working files a command may be given or give back.
const MAX_WORK_BYTES: usize = 32 << 20;

/// The tree a grade judges.
pub enum Tree<'a> {
    /// An agent's tree, laid out as the seed is.
    Dir(&'a Path),
    /// One of the hidden set's own trees, as read and digested: `golden` or `golden-before`.
    Hidden(&'a str),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub pass: u32,
    pub fail: u32,
    pub error: u32,
    pub skip: u32,
}

impl Counts {
    /// One more outcome of `kind`, as the host names it.
    fn add_one(&mut self, kind: &str) {
        match kind {
            "\"pass\"" => self.pass += 1,
            "\"fail\"" => self.fail += 1,
            "\"skip\"" => self.skip += 1,
            _ => self.error += 1,
        }
    }

    fn add(&mut self, other: Counts) {
        self.pass += other.pass;
        self.fail += other.fail;
        self.error += other.error;
        self.skip += other.skip;
    }
}

/// One group's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupGrade {
    /// Its index in the hidden set's manifest.
    pub index: usize,
    pub item: String,
    pub counts: Counts,
    /// One outcome per test, in the order the tests ran: the worst of its own (and its subtests'). A test that
    /// hadn't ended when the host did is a `fail`, as in `counts`. No test names: an index is all a grade needs.
    pub outcomes: Vec<Outcome>,
    /// Why its tests didn't all run, if they didn't: how the host ended.
    pub unfinished: Option<String>,
}

/// A test's outcome, worst last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Skip,
    Fail,
    Error,
}

impl Outcome {
    /// An outcome as the host names it.
    fn named(kind: &str) -> Outcome {
        match kind {
            "\"pass\"" => Outcome::Pass,
            "\"fail\"" => Outcome::Fail,
            "\"skip\"" => Outcome::Skip,
            _ => Outcome::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemGrade {
    pub id: String,
    pub counts: Counts,
    /// Groups of the item whose tests didn't all run.
    pub unfinished: usize,
}

/// A tree's grade at one share of the run: the groups that apply then, each item's sum.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grade {
    pub share: f64,
    pub items: Vec<ItemGrade>,
    pub groups: Vec<GroupGrade>,
}

impl Grade {
    /// Whether every group ran to its end, passed at least one test, and failed none.
    pub fn all_pass(&self) -> bool {
        self.groups.iter().all(|g| g.unfinished.is_none() && g.counts.pass > 0 && g.counts.fail + g.counts.error == 0)
    }
}

/// Grades `tree` against the groups of `set` that apply at `share`.
pub fn grade(python: &Python, project: &Project, set: &HiddenSet, tree: Tree, share: f64) -> Result<Grade, String> {
    let mut groups = Vec::new();
    for (index, group) in set.groups.iter().enumerate() {
        if !set.applies(project, group, share) {
            continue;
        }
        let file = group.file.strip_prefix("tests/").expect("checked to be under tests/");
        let (counts, outcomes, unfinished) = run_group(python, project, set, &tree, file)?;
        groups.push(GroupGrade { index, item: group.item.clone(), counts, outcomes, unfinished });
    }
    let items = project
        .items
        .iter()
        .filter(|i| groups.iter().any(|g| g.item == i.id))
        .map(|i| {
            let mut counts = Counts::default();
            let mine = groups.iter().filter(|g| g.item == i.id);
            mine.clone().for_each(|g| counts.add(g.counts));
            ItemGrade { id: i.id.clone(), counts, unfinished: mine.filter(|g| g.unfinished.is_some()).count() }
        })
        .collect();
    Ok(Grade { share, items, groups })
}

/// The approval check (answer c473d137f5b7): `golden/` at the end of the run, and, for a project with a change,
/// `golden-before/` just before the first change is released. Both must pass all that applies.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoldenCheck {
    pub after: Grade,
    pub before: Option<Grade>,
}

impl GoldenCheck {
    pub fn passed(&self) -> bool {
        self.after.all_pass() && self.before.as_ref().is_none_or(Grade::all_pass)
    }
}

pub fn golden_check(python: &Python, project: &Project, set: &HiddenSet) -> Result<GoldenCheck, String> {
    let after = grade(python, project, set, Tree::Hidden("golden"), 1.0)?;
    let before = match project.first_change() {
        // The largest share below the change's: everything released before it, and not it.
        Some(at) => Some(grade(python, project, set, Tree::Hidden("golden-before"), f64::from_bits(at.to_bits() - 1))?),
        None => None,
    };
    Ok(GoldenCheck { after, before })
}

/// Writes `tree` into `to`.
fn write_tree(set: &HiddenSet, tree: &Tree, to: &Path) -> Result<(), String> {
    match tree {
        Tree::Dir(dir) => crate::python::copy_tree(dir, to),
        Tree::Hidden(name) => {
            let (dirs, files) = set.under(name);
            write_files(to, dirs.into_iter().map(|d| (d, None)).chain(files.into_iter().map(|(f, b)| (f, Some(b)))))
        }
    }
}

/// Writes directories (`None`) and files into `to`, each a plain relative path.
fn write_files<'a>(to: &Path, entries: impl Iterator<Item = (&'a str, Option<&'a [u8]>)>) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for (rel, bytes) in entries {
        if !Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            return Err("a path that isn't plain".into());
        }
        let path = to.join(rel);
        match bytes {
            None => std::fs::create_dir_all(&path),
            Some(bytes) => {
                std::fs::create_dir_all(path.parent().expect("under `to`")).and_then(|_| std::fs::write(&path, bytes))
            }
        }
        .map_err(|e| format!("writing a file to grade with: {e}"))?;
    }
    Ok(())
}

/// Runs one group's test file in its own host, serving its requests, and counts its outcomes.
fn run_group(
    python: &Python,
    project: &Project,
    set: &HiddenSet,
    tree: &Tree,
    file: &str,
) -> Result<(Counts, Vec<Outcome>, Option<String>), String> {
    let scratch = Scratch::new()?;
    let tests = scratch.dir.join("tests");
    let (dirs, files) = set.under("tests");
    write_files(&tests, dirs.into_iter().map(|d| (d, None)).chain(files.into_iter().map(|(f, b)| (f, Some(b)))))?;
    let host = scratch.dir.join("host.py");
    std::fs::write(&host, HOST).map_err(|e| format!("writing the host: {e}"))?;
    // The host's own directory, and a command's when a test gives none: empty, and never one holding the tests
    // (findings 7f3dc66a8589, f13d1ff05b5f).
    let home = scratch.dir.join("home");
    std::fs::create_dir(&home).map_err(|e| format!("making the host's directory: {e}"))?;
    let host_limits = longer(&project.limits, HOST_MEMORY_MB, HOST_OUTPUT);
    let args = [
        host.into_os_string(),
        tests.into_os_string(),
        file.into(),
        project.package.clone().into(),
        home.into_os_string(),
    ];
    let mut running = python.start(&scratch, &args, &host_limits)?;
    let deadline = Instant::now() + Duration::from_secs(host_limits.wall_seconds.into());
    let mut server = Server { python, project, set, tree, running: None, generation: 0 };
    // What the host has said as it went (finding e7d128464936): the number of tests, each outcome, each test's end.
    let (mut total, mut done, mut so_far, mut counts) = (0u32, 0u32, Counts::default(), None);
    // Each ended test's outcome, and the worst so far of the one running.
    let (mut outcomes, mut running_test) = (Vec::new(), None::<Outcome>);
    while let Some(line) = running.next(deadline) {
        let reply = match line.split_once(' ') {
            Some(("S", request)) => server.ask(request, deadline)?,
            Some(("C", request)) => command(python, project, set, tree, request)?,
            Some(("N", n)) => {
                total = n.parse().unwrap_or(0);
                continue;
            }
            Some(("O", kind)) => {
                so_far.add_one(kind);
                running_test = running_test.max(Some(Outcome::named(kind)));
                continue;
            }
            Some(("D", _)) => {
                done += 1;
                // A test that said nothing, which unittest doesn't do, counts as an error.
                outcomes.push(running_test.take().unwrap_or(Outcome::Error));
                continue;
            }
            Some(("R", result)) => {
                counts = serde_json::from_str::<Counts>(result).ok();
                break;
            }
            _ => break,
        };
        if running.send(&reply).is_err() {
            break;
        }
    }
    server.end();
    let exit = running.finish();
    let unended = usize::try_from(total.saturating_sub(done)).expect("a u32 fits a usize");
    if counts.is_none() && unended > 0 {
        // The tests that hadn't ended when the host did are failures, the one that was running among them.
        outcomes.push(running_test.take().map_or(Outcome::Fail, |o| o.max(Outcome::Fail)));
        outcomes.extend(std::iter::repeat_n(Outcome::Fail, unended - 1));
    }
    // An outcome outside any test: the file failed to load.
    outcomes.extend(running_test);
    Ok(match counts {
        Some(counts) => (counts, outcomes, None),
        // The tests that hadn't ended when the host did count as failures.
        None => (Counts { fail: so_far.fail + total.saturating_sub(done), ..so_far }, outcomes, Some(describe(&exit))),
    })
}

/// The project's `limits`, with `HOST_TIMES` its CPU and wall time, for a process that serves a whole test file,
/// and the memory and output given.
fn longer(limits: &Limits, memory_mb: u32, output_bytes: u64) -> Limits {
    Limits {
        cpu_seconds: limits.cpu_seconds * HOST_TIMES,
        memory_mb,
        output_bytes,
        wall_seconds: limits.wall_seconds * HOST_TIMES,
        scratch_mb: limits.scratch_mb,
    }
}

fn describe(exit: &Exit) -> String {
    match exit {
        Exit::Code(code) => format!("exit {code}"),
        Exit::Signal(signal) => format!("signal {signal}"),
        Exit::WallTime => "wall time".into(),
        Exit::Memory => "memory".into(),
        Exit::Output => "output".into(),
        Exit::Scratch => "scratch".into(),
    }
}

/// The code under test, started when first asked, and again after it ends: a test that crashes it leaves the
/// rest a fresh one. Each start is a new generation, whose number the server tags its handles with, so a handle
/// of one that has ended is found again by name if it was a module's attribute, and otherwise refused, as that
/// test's error, never taken for another object (findings fe2549db89ee, 5f5c3b4b25a9). It may run as long as the
/// host, and each call as long, and use as much CPU, as one run of the project's code: a budget per call, not
/// for the whole group.
struct Server<'a> {
    python: &'a Python,
    project: &'a Project,
    set: &'a HiddenSet,
    tree: &'a Tree<'a>,
    running: Option<(Scratch, Running)>,
    generation: u32,
}

impl Server<'_> {
    fn ask(&mut self, request: &str, deadline: Instant) -> Result<String, String> {
        if self.running.is_none() {
            self.generation += 1;
            let scratch = Scratch::new()?;
            let tree = scratch.dir.join("tree");
            write_tree(self.set, self.tree, &tree)?;
            let script = scratch.dir.join("server.py");
            std::fs::write(&script, SERVER).map_err(|e| format!("writing the server: {e}"))?;
            let args = [
                script.into_os_string(),
                tree.into_os_string(),
                self.project.package.clone().into(),
                self.generation.to_string().into(),
            ];
            let limits = &self.project.limits;
            // The CPU limit the operating system keeps is the group's wall time: each call's is kept below.
            let group = longer(limits, limits.memory_mb, limits.output_bytes);
            let group = Limits { cpu_seconds: group.wall_seconds, ..group };
            let running = self.python.start(&scratch, &args, &group)?;
            self.running = Some((scratch, running));
        }
        let limits = self.project.limits;
        let call = deadline.min(Instant::now() + Duration::from_secs(limits.wall_seconds.into()));
        let budget = Duration::from_secs(limits.cpu_seconds.into());
        let (_, running) = self.running.as_mut().expect("started");
        let start = running.cpu().unwrap_or_default();
        if running.send(request).is_ok() {
            loop {
                match running.next_line(call.min(Instant::now() + 5 * POLL)) {
                    Line::Got(reply) => return Ok(reply),
                    Line::Closed => break,
                    Line::Late => {
                        let used = running.cpu().map(|c| c.saturating_sub(start));
                        if Instant::now() >= call || used.is_none_or(|used| used > budget) {
                            break;
                        }
                    }
                }
            }
        }
        let (_, running) = self.running.take().expect("started");
        Ok(serde_json::json!({"gone": describe(&running.finish())}).to_string())
    }

    fn end(&mut self) {
        if let Some((_, running)) = self.running.take() {
            running.finish();
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandRequest {
    module: String,
    argv: Vec<String>,
    /// Each working file's path and its bytes in hex.
    files: Vec<(String, String)>,
    stdin: String,
}

/// Runs `python -m <module> <argv>` for a test, in a fresh child with the tree and the test's working files, and
/// gives back its exit code, its output and the working files after it.
fn command(python: &Python, project: &Project, set: &HiddenSet, tree: &Tree, request: &str) -> Result<String, String> {
    let r: CommandRequest = match serde_json::from_str(request) {
        Ok(r) => r,
        Err(_) => return Ok(serde_json::json!({"gone": "an unreadable command"}).to_string()),
    };
    let package = &project.package;
    if r.module != *package && !r.module.starts_with(&format!("{package}.")) {
        return Ok(serde_json::json!({"gone": "only the package runs as a command"}).to_string());
    }
    let scratch = Scratch::new()?;
    let (tree_dir, work) = (scratch.dir.join("tree"), scratch.dir.join("work"));
    write_tree(set, tree, &tree_dir)?;
    let mut given = Vec::new();
    for (rel, bytes) in &r.files {
        given.push((rel.as_str(), hex::decode(bytes).map_err(|_| "a working file that isn't hex")?));
    }
    if given.iter().map(|(_, b)| b.len()).sum::<usize>() > MAX_WORK_BYTES {
        return Ok(serde_json::json!({"gone": "too many working files"}).to_string());
    }
    write_files(&work, given.iter().map(|(rel, b)| (*rel, Some(b.as_slice()))))?;
    let cli = scratch.dir.join("cli.py");
    std::fs::write(&cli, CLI).map_err(|e| format!("writing the command runner: {e}"))?;
    let mut args: Vec<OsString> =
        vec![cli.into_os_string(), tree_dir.into_os_string(), work.clone().into_os_string(), r.module.into()];
    args.extend(r.argv.into_iter().map(OsString::from));
    let input = hex::decode(&r.stdin).map_err(|_| "input that isn't hex")?;
    let out = python.run_with_input(&scratch, &args, &project.limits, Some(input))?;
    let (code, mut stderr) = match &out.exit {
        Exit::Code(code) => (*code, out.stderr),
        Exit::Signal(signal) => (-signal, out.stderr),
        other => (
            -9,
            [out.stderr, format!("\n[grader: the command was ended: {}]\n", describe(other)).into_bytes()].concat(),
        ),
    };
    stderr.truncate(MAX_WORK_BYTES);
    let files: Vec<(String, String)> = work_files(&work)?.into_iter().map(|(rel, b)| (rel, hex::encode(b))).collect();
    Ok(serde_json::json!({"code": code, "stdout": hex::encode(&out.stdout), "stderr": hex::encode(&stderr), "files": files})
        .to_string())
}

/// The regular files under `work`, by path relative to it, after a command: no symlink is followed, and at most
/// [`MAX_WORK_BYTES`] in all.
fn work_files(work: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let (mut out, mut total) = (Vec::new(), 0usize);
    let mut stack = vec![(work.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
            if meta.is_dir() {
                stack.push((entry.path(), rel));
            } else if meta.is_file() {
                total += meta.len() as usize;
                if total > MAX_WORK_BYTES {
                    return Ok(out);
                }
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    out.push((rel, bytes));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
