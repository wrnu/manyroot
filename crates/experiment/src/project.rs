//! A project of the evaluation suite (`docs/design/eval-suite.md`, Project format): a spec, work items, a
//! seed repository with its public tests, and a reference to the hidden tests and golden solution in a
//! private store.
//!
//! A project's directory is what agents may read. The private store is read only by the grader, at a path
//! it is given; the project names it by id and digest alone, never by path, so no hidden test's name,
//! count or content is in the project, a run, a Ground or this repository.
//!
//! Projects sit beside the stacklib [`crate::task::Task`]: nothing here changes a Phase 1 or Phase 2 run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The project format this reads.
pub const FORMAT: u32 = 1;
/// The private store's format this reads.
pub const HIDDEN_FORMAT: u32 = 1;

/// The most files a project or a store may hold, the largest one, and all of them together: far more than
/// a project needs, so a mistake (a build directory, a virtual environment) is refused, not read.
const MAX_FILES: usize = 5_000;
const MAX_FILE_BYTES: u64 = 1 << 20;
const MAX_TOTAL_BYTES: u64 = 32 << 20;

/// A project, loaded and checked.
#[derive(Debug, Clone)]
pub struct Project {
    /// The project's directory, resolved.
    pub dir: PathBuf,
    pub name: String,
    /// The Python minor version it is written for, such as `3.12`.
    pub python: String,
    /// The importable package under the seed, which the grader reaches through its proxy.
    pub package: String,
    pub spec: PathBuf,
    pub seed: PathBuf,
    pub public_tests: PathBuf,
    pub items: Vec<Item>,
    pub limits: Limits,
    pub hidden: HiddenRef,
}

/// One work item, as the issue an agent reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// Its issue text, in the project.
    pub text: PathBuf,
    pub release: Release,
    /// For a change: the items whose requirement it revises.
    pub revises: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Feature,
    Refactor,
    Fix,
    Change,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Feature => "feature",
            Kind::Refactor => "refactor",
            Kind::Fix => "fix",
            Kind::Change => "change",
        }
    }
}

/// When an item reaches the agents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Release {
    Start,
    /// Once this share of the run's agent-turns is used, as Phase 2's waves are.
    AfterShare(f64),
}

impl Release {
    /// Whether it is released once `share` of the run's agent-turns is used (1.0 at the end).
    pub fn by(self, share: f64) -> bool {
        match self {
            Release::Start => true,
            Release::AfterShare(at) => share >= at,
        }
    }
}

/// Limits on each run of agent code, per run of the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub cpu_seconds: u32,
    pub memory_mb: u32,
    pub output_bytes: u64,
    pub wall_seconds: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { cpu_seconds: 10, memory_mb: 512, output_bytes: 1 << 20, wall_seconds: 60 }
    }
}

/// The most each limit may be set to.
const MAX_LIMITS: Limits = Limits { cpu_seconds: 600, memory_mb: 8192, output_bytes: 64 << 20, wall_seconds: 3600 };

/// The hidden tests and golden solution, by reference only: which set, and the digest of its whole
/// contents, so the grader refuses any store but the approved one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiddenRef {
    pub id: String,
    pub digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    name: String,
    python: String,
    package: String,
    spec: String,
    seed: String,
    public_tests: Option<String>,
    items: Vec<ItemEntry>,
    #[serde(default)]
    limits: Limits,
    hidden: HiddenRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemEntry {
    id: String,
    kind: Kind,
    title: String,
    text: String,
    release: ReleaseEntry,
    #[serde(default)]
    revises: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseEntry {
    at: Option<String>,
    after_share: Option<f64>,
}

impl Project {
    /// Loads the project in `dir` and checks it: every path inside it and no symlink anywhere, every item's
    /// text there, unique ids, changes revising only existing items, and a seed holding the package and the
    /// public tests.
    pub fn load(dir: &Path) -> Result<Project, String> {
        let dir = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        walk(&dir, Names::Shown).map_err(|e| format!("the project: {e}"))?;
        let text = std::fs::read_to_string(dir.join("project.json")).map_err(|e| format!("project.json: {e}"))?;
        let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("project.json: {e}"))?;
        if m.format != FORMAT {
            return Err(format!("project.json is format {}; this reads format {FORMAT}", m.format));
        }
        if !is_name(&m.name) {
            return Err(format!("the name {:?} isn't lowercase letters, digits and dashes", m.name));
        }
        if !is_python_version(&m.python) {
            return Err(format!("python {:?} isn't a version such as \"3.12\"", m.python));
        }
        if !is_package(&m.package) {
            return Err(format!("the package {:?} isn't a lowercase Python identifier", m.package));
        }
        let inside = |field: &str, rel: &str| inside(&dir, rel).map_err(|e| format!("{field}: {e}"));
        let spec = inside("spec", &m.spec)?;
        let seed = inside("seed", &m.seed)?;
        let public_tests = inside("public_tests", m.public_tests.as_deref().unwrap_or(&format!("{}/tests", m.seed)))?;
        if !spec.is_file() {
            return Err(format!("spec: no file {}", m.spec));
        }
        if !seed.join(&m.package).is_dir() {
            return Err(format!("the seed has no package directory {}", m.package));
        }
        if !public_tests.starts_with(&seed) || !public_tests.is_dir() {
            return Err("public_tests isn't a directory in the seed".into());
        }
        if test_files(&public_tests)?.is_empty() {
            return Err("public_tests holds no test_*.py".into());
        }
        let limits = m.limits;
        let within = |v: u64, most: u64| v > 0 && v <= most;
        if !(within(limits.cpu_seconds.into(), MAX_LIMITS.cpu_seconds.into())
            && within(limits.memory_mb.into(), MAX_LIMITS.memory_mb.into())
            && within(limits.output_bytes, MAX_LIMITS.output_bytes)
            && within(limits.wall_seconds.into(), MAX_LIMITS.wall_seconds.into()))
        {
            return Err(format!("limits must each be above 0 and at most {MAX_LIMITS:?}"));
        }
        if !is_store_id(&m.hidden.id) {
            return Err(format!("hidden.id {:?} isn't a name such as \"ledger@1\"", m.hidden.id));
        }
        if !is_digest(&m.hidden.digest) {
            return Err("hidden.digest isn't 64 lowercase hex digits".into());
        }
        let items = m
            .items
            .into_iter()
            .map(|e| {
                let at = |what: String| format!("item {:?}: {what}", e.id);
                let release = match (e.release.at.as_deref(), e.release.after_share) {
                    (Some("start"), None) => Release::Start,
                    (None, Some(share)) if share > 0.0 && share < 1.0 => Release::AfterShare(share),
                    _ => return Err(at("release is {\"at\": \"start\"} or {\"after_share\": x}, 0 < x < 1".into())),
                };
                let text = inside("text", &e.text).map_err(at)?;
                if !text.is_file() {
                    return Err(at(format!("no file {}", e.text)));
                }
                Ok(Item { id: e.id, kind: e.kind, title: e.title, text, release, revises: e.revises })
            })
            .collect::<Result<Vec<Item>, String>>()?;
        check_items(&items)?;
        Ok(Project {
            dir,
            name: m.name,
            python: m.python,
            package: m.package,
            spec,
            seed,
            public_tests,
            items,
            limits,
            hidden: m.hidden,
        })
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The items released once `share` of the run's agent-turns is used.
    pub fn released(&self, share: f64) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.release.by(share))
    }

    /// The change that revises `item`, if one does.
    pub fn revised_by(&self, item: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.revises.iter().any(|r| r == item))
    }

    /// The seed's files, relative to it.
    pub fn seed_files(&self) -> Result<Vec<String>, String> {
        Ok(walk(&self.seed, Names::Shown)?.files.into_iter().map(|f| f.rel).collect())
    }

    /// The public tests' files, relative to the seed.
    pub fn public_test_files(&self) -> Result<Vec<String>, String> {
        let under = self.public_tests.strip_prefix(&self.seed).expect("checked to be in the seed");
        Ok(test_files(&self.public_tests)?.into_iter().map(|f| join_rel(under, &f)).collect())
    }
}

fn check_items(items: &[Item]) -> Result<(), String> {
    if items.is_empty() {
        return Err("no items".into());
    }
    let mut ids = BTreeSet::new();
    for item in items {
        if !is_name(&item.id) {
            return Err(format!("item id {:?} isn't lowercase letters, digits and dashes", item.id));
        }
        if !ids.insert(item.id.as_str()) {
            return Err(format!("item id {:?} is used twice", item.id));
        }
        if item.title.trim().is_empty() {
            return Err(format!("item {:?} has no title", item.id));
        }
    }
    let mut revised = BTreeMap::new();
    for item in items {
        if item.kind != Kind::Change {
            if !item.revises.is_empty() {
                return Err(format!("item {:?} revises others but isn't a change", item.id));
            }
            continue;
        }
        if item.release == Release::Start {
            return Err(format!("change {:?} is released at the start: it would be the spec", item.id));
        }
        for r in &item.revises {
            match items.iter().find(|i| &i.id == r) {
                None => return Err(format!("change {:?} revises {r:?}, which isn't an item", item.id)),
                Some(i) if i.kind == Kind::Change => {
                    return Err(format!("change {:?} revises {r:?}, which is a change", item.id));
                }
                Some(_) => {}
            }
            // One change per item, so its hidden tests are graded by one release: before it or after.
            if let Some(other) = revised.insert(r.as_str(), item.id.as_str()) {
                return Err(format!("{r:?} is revised by both {other:?} and {:?}", item.id));
            }
        }
    }
    Ok(())
}

/// The private store's hidden set for a project, checked against it, as read once: what was digested is
/// what is parsed and what the grader is given.
#[derive(Debug, Clone)]
pub struct HiddenSet {
    /// `<store>/<hidden.id>`, resolved.
    pub dir: PathBuf,
    pub groups: Vec<Group>,
    snapshot: Snapshot,
}

/// Hidden tests for one item, in one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub item: String,
    /// A test file, relative to the set's directory, under `tests/`.
    pub file: String,
    pub applies: Applies,
}

/// When a group's tests judge its item: always, or only before or only after the change that revises it is
/// released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applies {
    Always,
    BeforeChange,
    AfterChange,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HiddenManifest {
    format: u32,
    project: String,
    groups: Vec<Group>,
}

/// What a check of a store found, in counts and item ids only: nothing of a test's name or content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreSummary {
    pub groups: usize,
    pub test_files: usize,
    pub golden_files: usize,
    /// Items no group judges.
    pub items_without_tests: Vec<String>,
}

/// How a private store inside a repository is refused.
const STORE: bracken_demo::safety::Advice =
    bracken_demo::safety::Advice { checkout: "keep the private store outside every repository", env: "unset it" };

impl HiddenSet {
    /// Opens `project`'s set in the private store at `store` and reads it once. Refuses a set that is a
    /// symlink, that overlaps the project or is in a git checkout, or holds one (`.git`) anywhere; whose
    /// contents' digest isn't the project's; or whose manifest names an item the project hasn't, a file that
    /// isn't a `.py` under `tests/`, or a before- or after-change group for an item no change revises. Its
    /// errors name nothing inside the set: a group by its index, a problem by its kind.
    pub fn open(project: &Project, store: &Path) -> Result<HiddenSet, String> {
        let store = std::fs::canonicalize(store).map_err(|e| format!("the private store {}: {e}", store.display()))?;
        let id = &project.hidden.id;
        let meta = std::fs::symlink_metadata(store.join(id)).map_err(|e| format!("the private store's {id}: {e}"))?;
        if !meta.is_dir() {
            return Err(format!("the private store's {id} is a symlink or not a directory"));
        }
        let dir = std::fs::canonicalize(store.join(id)).map_err(|e| format!("the private store's {id}: {e}"))?;
        for path in [&store, &dir] {
            if path.starts_with(&project.dir) || project.dir.starts_with(path) {
                return Err("the private store and the project overlap".into());
            }
        }
        bracken_demo::safety::refuse_git_checkout(&dir, false, &STORE).map_err(|e| e.to_string())?;
        let snapshot = Snapshot::read(&dir)?;
        let digest = snapshot.digest();
        if digest != project.hidden.digest {
            return Err(format!(
                "the private store's {id} has digest {digest}, not the project's {}",
                project.hidden.digest
            ));
        }
        let manifest = snapshot.file("hidden.json").ok_or("the hidden set has no hidden.json")?;
        let m: HiddenManifest = serde_json::from_slice(manifest)
            .map_err(|e| format!("hidden.json isn't a valid manifest (line {}, column {})", e.line(), e.column()))?;
        if m.format != HIDDEN_FORMAT {
            return Err(format!("hidden.json is format {}; this reads format {HIDDEN_FORMAT}", m.format));
        }
        if m.project != project.name {
            return Err(format!("hidden.json is for another project than {:?}", project.name));
        }
        for (n, g) in m.groups.iter().enumerate() {
            let Some(item) = project.item(&g.item) else {
                return Err(format!("group {n} names no item of the project"));
            };
            let plain = Path::new(&g.file).components().all(|c| matches!(c, Component::Normal(_)));
            if !plain || !g.file.starts_with("tests/") || !g.file.ends_with(".py") || snapshot.file(&g.file).is_none() {
                return Err(format!("group {n}'s file isn't a .py file under tests/ in the set"));
            }
            if g.applies != Applies::Always && project.revised_by(&item.id).is_none() {
                return Err(format!("group {n} is for before or after a change, and no change revises {:?}", item.id));
            }
        }
        if !snapshot.dirs.contains(&format!("golden/{}", project.package)) {
            return Err(format!("the golden solution has no package directory {}", project.package));
        }
        Ok(HiddenSet { dir, groups: m.groups, snapshot })
    }

    /// Whether `group` judges its item once `share` of the run's agent-turns is used.
    pub fn applies(&self, project: &Project, group: &Group, share: f64) -> bool {
        let changed = || project.revised_by(&group.item).is_some_and(|c| c.release.by(share));
        match group.applies {
            Applies::Always => true,
            Applies::BeforeChange => !changed(),
            Applies::AfterChange => changed(),
        }
    }

    /// A file of the set, relative to it, as it was read and digested.
    pub fn file(&self, rel: &str) -> Option<&[u8]> {
        self.snapshot.file(rel)
    }

    pub fn summary(&self, project: &Project) -> StoreSummary {
        let judged: BTreeSet<&str> = self.groups.iter().map(|g| g.item.as_str()).collect();
        let test_files: BTreeSet<&str> = self.groups.iter().map(|g| g.file.as_str()).collect();
        StoreSummary {
            groups: self.groups.len(),
            test_files: test_files.len(),
            golden_files: self.snapshot.files.keys().filter(|f| f.starts_with("golden/")).count(),
            items_without_tests: project
                .items
                .iter()
                .map(|i| i.id.clone())
                .filter(|i| !judged.contains(i.as_str()))
                .collect(),
        }
    }
}

/// The digest of the hidden set in `dir`, read as [`HiddenSet::open`] reads it.
pub fn digest(dir: &Path) -> Result<String, String> {
    Ok(Snapshot::read(dir)?.digest())
}

/// A hidden set as read once: its directories, and each file's bytes, by path relative to it.
#[derive(Debug, Clone)]
struct Snapshot {
    dirs: BTreeSet<String>,
    files: BTreeMap<String, Vec<u8>>,
}

impl Snapshot {
    /// Walks `dir` and reads each file once, checking that what it opened is the regular file the walk found,
    /// not a symlink swapped in since. Its errors name nothing inside `dir`.
    fn read(dir: &Path) -> Result<Snapshot, String> {
        use std::os::unix::fs::MetadataExt;
        let tree = walk(dir, Names::Hidden).map_err(|e| format!("the hidden set: {e}"))?;
        let mut files = BTreeMap::new();
        for f in tree.files {
            let unread = |e: std::io::Error| format!("the hidden set: a file can't be read: {e}");
            let mut file = std::fs::File::open(&f.path).map_err(unread)?;
            let meta = file.metadata().map_err(unread)?;
            if !meta.is_file() || (meta.dev(), meta.ino()) != f.id {
                return Err("the hidden set changed while it was read".into());
            }
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, MAX_FILE_BYTES + 1), &mut bytes)
                .map_err(unread)?;
            if bytes.len() as u64 != meta.len() {
                return Err("the hidden set changed while it was read".into());
            }
            files.insert(f.rel, bytes);
        }
        Ok(Snapshot { dirs: tree.dirs, files })
    }

    /// blake3 over every entry in order of its path: a directory as `<path>/`, a file as its path and the
    /// blake3 of its bytes. So it changes with any file's content or name, and with any directory added or
    /// removed, even an empty one.
    fn digest(&self) -> String {
        let mut entries: Vec<String> = self.dirs.iter().map(|d| format!("{d}/\n")).collect();
        entries.extend(self.files.iter().map(|(rel, bytes)| format!("{rel}\0{}\n", blake3::hash(bytes).to_hex())));
        entries.sort();
        blake3::hash(entries.concat().as_bytes()).to_hex().to_string()
    }

    fn file(&self, rel: &str) -> Option<&[u8]> {
        self.files.get(rel).map(Vec::as_slice)
    }
}

/// Whether a walk's errors may name what it found: a project's may, a hidden set's never.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Names {
    Shown,
    Hidden,
}

/// A walked tree: its directories and regular files, by path relative to its root.
struct Tree {
    dirs: BTreeSet<String>,
    files: Vec<File>,
}

struct File {
    /// Relative to the walked directory, with `/` between its parts.
    rel: String,
    path: PathBuf,
    /// The device and inode the walk found.
    id: (u64, u64),
}

/// Every directory and regular file under `root`, files in order of their relative paths. Refuses a symlink or
/// any other kind of file, a `.git` (a repository inside), a name that isn't UTF-8, and more files or bytes
/// than a project needs. With [`Names::Hidden`], an error says what kind of entry, never its name.
fn walk(root: &Path, names: Names) -> Result<Tree, String> {
    use std::os::unix::fs::MetadataExt;
    let at = |rel: &str| if names == Names::Shown { format!(" at {rel}") } else { String::new() };
    let unread = |e: std::io::Error| format!("an entry can't be read: {e}");
    let (mut dirs, mut files, mut total) = (BTreeSet::new(), Vec::new(), 0u64);
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(unread)? {
            let entry = entry.map_err(unread)?;
            let name = entry.file_name().into_string().map_err(|_| format!("a name that isn't UTF-8{}", at(&rel)))?;
            let rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if name == ".git" {
                return Err(format!("a .git, a repository inside{}", at(&rel)));
            }
            let meta = std::fs::symlink_metadata(entry.path()).map_err(unread)?;
            if meta.is_dir() {
                dirs.insert(rel.clone());
                stack.push((entry.path(), rel));
            } else if meta.is_file() {
                total += meta.len();
                if meta.len() > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES || files.len() >= MAX_FILES {
                    return Err(format!(
                        "more than a project needs{}: at most {MAX_FILES} files, {MAX_FILE_BYTES} bytes each, \
                         {MAX_TOTAL_BYTES} in all",
                        at(&rel)
                    ));
                }
                files.push(File { rel, path: entry.path(), id: (meta.dev(), meta.ino()) });
            } else {
                return Err(format!("a symlink or other special file{}", at(&rel)));
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Tree { dirs, files })
}

/// The `test_*.py` files directly in `dir`, as unittest's discovery finds them.
fn test_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = walk(dir, Names::Shown)?
        .files
        .into_iter()
        .map(|f| f.rel)
        .filter(|rel| !rel.contains('/') && rel.starts_with("test_") && rel.ends_with(".py"))
        .collect();
    out.sort();
    Ok(out)
}

fn join_rel(under: &Path, name: &str) -> String {
    let under = under.to_string_lossy();
    if under.is_empty() { name.to_string() } else { format!("{under}/{name}") }
}

/// `rel` under `dir`, refused unless it is plain parts: no root, `.`, `..` or empty part.
fn inside(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = Path::new(rel);
    if rel.is_empty() || rel.ends_with('/') || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("{rel:?} isn't a plain relative path"));
    }
    Ok(dir.join(path))
}

fn is_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_package(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_python_version(s: &str) -> bool {
    s.split_once('.').is_some_and(|(major, minor)| {
        major == "3" && !minor.is_empty() && minor.len() <= 2 && minor.chars().all(|c| c.is_ascii_digit())
    })
}

/// A store id: one path part of letters, digits and `@._-`, never `.` or `..`.
fn is_store_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "@._-".contains(c))
}

fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests;
