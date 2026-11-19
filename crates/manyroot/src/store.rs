//! A Ground replica on the local filesystem.
//!
//! ```text
//! <dir>/ground.json          ground id and local name
//! <dir>/objects/ab/cdef...   one file per contribution, named by id
//! <dir>/agents/<name>.json   public directory: local names for agent keys
//! <dir>/identities/<name>    private agent keys (unencrypted in v0)
//! ```
//!
//! Everything except `identities/` is safe to commit to git. Objects are
//! immutable and named by their hash, so merging branches is a union of files
//! and never conflicts. The agent directory lets a clone show names instead
//! of bare keys. `identities/` holds private keys and must never be
//! committed, so a fresh clone has none. A Ground with no contributions also
//! has no `objects/`, because git drops empty directories. Both are treated
//! as empty.
//!
//! Contribution files are content-addressed and written by atomic rename, so
//! any number of agent processes can write to one replica without locking:
//! two processes writing concurrently just produce concurrent contributions.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::contribution::{Body, Contribution, DecodeError};
use crate::graph::{Graph, ImportReport, RuleViolation};
use crate::id::{AgentId, ContributionId, GroundId};
use crate::identity::{self, Identity, IdentityError, SignError, Signer};
use crate::sync::{Limits, Message, Session, SyncError};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("no ground at {0}")]
    NotAGround(PathBuf),
    #[error("a ground already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("{path}: {reason}")]
    Corrupt { path: PathBuf, reason: String },
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("contribution rejected")]
    Rejected(#[from] RuleViolation),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("no identity named {0:?}")]
    UnknownIdentity(String),
    #[error("the name {0:?} is already taken")]
    NameTaken(String),
    /// A contribution handed to [`Store::open_trusting`] doesn't fit the replica: open it with
    /// [`Store::open`] instead.
    #[error("trusted contribution {id}: {reason}")]
    Untrusted { id: ContributionId, reason: String },
    #[error("cannot sync ground {0} with ground {1}")]
    DifferentGrounds(GroundId, GroundId),
    #[error("sync: {0}")]
    Sync(#[from] SyncError),
    #[error("signing: {0}")]
    Sign(#[from] SignError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_owned(), source }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundInfo {
    pub ground: GroundId,
    /// Local label only; not part of the protocol.
    pub name: String,
}

/// A public directory entry, naming an agent's key. Safe to commit and share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEntry {
    pub name: String,
    pub agent: AgentId,
}

pub struct Store {
    dir: PathBuf,
    info: GroundInfo,
    graph: Graph,
}

impl Store {
    /// Creates a new Ground with a fresh random id.
    pub fn init(dir: &Path, name: &str) -> Result<Store, StoreError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(IdentityError::Random)?;
        Self::create(dir, GroundInfo { ground: GroundId(bytes), name: name.to_owned() })
    }

    /// Creates an empty replica of an existing Ground, e.g. before syncing.
    pub fn create(dir: &Path, info: GroundInfo) -> Result<Store, StoreError> {
        let manifest = dir.join("ground.json");
        if manifest.exists() {
            return Err(StoreError::AlreadyExists(dir.to_owned()));
        }
        for sub in ["objects", "identities"] {
            let p = dir.join(sub);
            fs::create_dir_all(&p).map_err(io(&p))?;
        }
        let json = serde_json::to_string_pretty(&info).expect("ground info serializes") + "\n";
        match write_new(&manifest, json.as_bytes()) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::AlreadyExists(dir.to_owned()));
            }
            other => other.map_err(io(&manifest))?,
        }
        Ok(Store { dir: dir.to_owned(), graph: Graph::new(info.ground), info })
    }

    /// Opens a replica and verifies every contribution in it.
    pub fn open(dir: &Path) -> Result<Store, StoreError> {
        Self::open_trusting(dir, Vec::new())
    }

    /// Opens a replica as [`Store::open`] does, except that it adds the `trusted` contributions without
    /// reading their objects or checking them again: what makes a cached read fast.
    ///
    /// **The caller vouches for every trusted contribution**: that it decoded it from this replica's
    /// object of the same id, and that it passed every validity rule, in an earlier open of this
    /// replica. Nothing here re-checks a trusted contribution's signature or rules. Its object must
    /// still be listed in `objects/` as a regular file (the listing's own file type, so a link, a
    /// directory or a missing file under its name doesn't count), but its bytes aren't read. So **an
    /// object overwritten on disk after the caller cached it goes unnoticed, and so does one made
    /// unreadable**, where [`Store::open`] would refuse the replica.
    ///
    /// Every other object is read, decoded and checked exactly as [`Store::open`] checks it, all of
    /// the protocol's validity rules through [`Graph::insert`]. Contributions go into the graph in the
    /// order the full open's import uses (parents first, ready ones by id), so the graph is the same
    /// to the last child list, and a replica that fails gives the same error. `trusted` must be in a causal order
    /// (each parent before its children), each of this Ground and listed once. Any contribution
    /// that doesn't fit gives [`StoreError::Untrusted`]: one whose object is gone or isn't a regular
    /// file, of another Ground, listed twice, or before one of its parents. So does a trusted parent that's missing.
    /// The caller then opens with [`Store::open`], which reports what's really wrong, if anything.
    pub fn open_trusting(dir: &Path, trusted: Vec<Contribution>) -> Result<Store, StoreError> {
        let info = read_info(dir)?;
        let files = object_files(dir)?;
        let mut graph = Graph::new(info.ground);
        let mut batch = Vec::new();
        if trusted.is_empty() {
            for (path, regular) in files {
                batch.push(read_regular_object(dir, path, regular)?);
            }
        } else {
            // Name every object by the id its file name spells, only in its canonical (lowercase)
            // spelling, as the full open's check of each path against its content does.
            let mut on_disk: HashMap<ContributionId, (PathBuf, bool)> = HashMap::new();
            for (path, regular) in files {
                let shard = path.parent().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or_default();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let id =
                    format!("{shard}{name}").parse::<ContributionId>().ok().filter(|id| object_path(dir, id) == path);
                let Some(id) = id else {
                    return Err(StoreError::Corrupt { path, reason: "not named by a contribution id".into() });
                };
                on_disk.insert(id, (path, regular));
            }
            let mut seen: HashSet<ContributionId> = HashSet::new();
            let mut pool: HashMap<ContributionId, (Contribution, bool)> = HashMap::new();
            for c in trusted {
                let id = c.id();
                let untrusted = |reason: String| StoreError::Untrusted { id, reason };
                if c.ground() != info.ground {
                    return Err(untrusted("it belongs to another Ground".into()));
                }
                if seen.contains(&id) {
                    return Err(untrusted("it is listed twice".into()));
                }
                match on_disk.remove(&id) {
                    None => return Err(untrusted("its object isn't in the replica".into())),
                    Some((_, false)) => return Err(untrusted("its object isn't a regular file".into())),
                    Some((_, true)) => {}
                }
                if let Some(p) = c.parents().iter().find(|p| !seen.contains(p)) {
                    return Err(untrusted(format!("its parent {p} isn't trusted before it")));
                }
                seen.insert(id);
                pool.insert(id, (c, true));
            }
            let mut rest: Vec<(ContributionId, PathBuf, bool)> =
                on_disk.into_iter().map(|(id, (p, regular))| (id, p, regular)).collect();
            rest.sort();
            for (_, path, regular) in rest {
                let c = read_regular_object(dir, path, regular)?;
                pool.insert(c.id(), (c, false));
            }
            // `Graph::import_with`'s walk, Kahn's algorithm with ready ids taken smallest first, over the
            // trusted and new together: the trusted are added as they are, the new through `insert`.
            let mut waiting_on: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
            let mut missing_count: HashMap<ContributionId, usize> = HashMap::new();
            let mut ready: BTreeSet<ContributionId> = BTreeSet::new();
            for (id, (c, _)) in &pool {
                let missing: HashSet<_> = c.parents().iter().collect();
                for p in &missing {
                    waiting_on.entry(**p).or_default().push(*id);
                }
                if missing.is_empty() {
                    ready.insert(*id);
                } else {
                    missing_count.insert(*id, missing.len());
                }
            }
            while let Some(id) = ready.pop_first() {
                let (c, is_trusted) = pool.remove(&id).expect("ready ids are pending");
                if is_trusted {
                    graph.insert_checked(c);
                } else if let Err(why) = graph.insert(c) {
                    return Err(StoreError::Corrupt { path: object_path(dir, &id), reason: why.to_string() });
                }
                for child in waiting_on.remove(&id).unwrap_or_default() {
                    let n = missing_count.get_mut(&child).expect("waiting child has a count");
                    *n -= 1;
                    if *n == 0 {
                        missing_count.remove(&child);
                        ready.insert(child);
                    }
                }
            }
            if let Some(id) = pool.keys().min() {
                return Err(StoreError::Corrupt {
                    path: object_path(dir, id),
                    reason: "an ancestor is missing from the replica".into(),
                });
            }
            return Ok(Store { dir: dir.to_owned(), info, graph });
        }

        let report = graph.import(batch);
        if let Some((id, why)) = report.rejected.first() {
            return Err(StoreError::Corrupt { path: object_path(dir, id), reason: why.to_string() });
        }
        if let Some(id) = report.unresolved.first() {
            return Err(StoreError::Corrupt {
                path: object_path(dir, id),
                reason: "an ancestor is missing from the replica".into(),
            });
        }
        Ok(Store { dir: dir.to_owned(), info, graph })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn info(&self) -> &GroundInfo {
        &self.info
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Writes a new contribution that builds on every current causal head.
    pub fn contribute(&mut self, identity: &Identity, body: Body) -> Result<ContributionId, StoreError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let heads: Vec<ContributionId> = self.graph.heads().collect();
        let c = Contribution::create(self.info.ground, identity, heads, now, body);
        let id = c.id();
        self.add(c)?;
        Ok(id)
    }

    /// [`Store::contribute`] through any [`Signer`], such as one whose key is held elsewhere.
    pub fn contribute_with(&mut self, signer: &dyn Signer, body: Body) -> Result<ContributionId, StoreError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let heads: Vec<ContributionId> = self.graph.heads().collect();
        let c = Contribution::create_with(self.info.ground, signer, heads, now, body)?;
        let id = c.id();
        self.add(c)?;
        Ok(id)
    }

    /// Adds one contribution whose parents are already present.
    pub fn add(&mut self, c: Contribution) -> Result<bool, StoreError> {
        if self.graph.contains(&c.id()) {
            return Ok(false);
        }
        self.graph.check(&c)?;
        let report = self.import([c])?;
        Ok(!report.added.is_empty())
    }

    /// Imports contributions in any order, persisting each one as it is added.
    pub fn import(&mut self, batch: impl IntoIterator<Item = Contribution>) -> Result<ImportReport, StoreError> {
        let dir = self.dir.clone();
        self.graph.import_with(batch, |c| write_object(&dir, c))
    }

    pub fn identities_dir(&self) -> PathBuf {
        self.dir.join("identities")
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.dir.join("agents")
    }

    /// Creates a private key and its public directory entry. Refuses a name
    /// that is already taken, here or in a directory entry from elsewhere.
    pub fn create_identity(&self, name: &str) -> Result<Identity, StoreError> {
        self.create_identity_in(name, &self.identities_dir())
    }

    /// [`Store::create_identity`], with the private key in `key_dir` instead of this replica's
    /// `identities/`: a key held by a signing server, say. The name must be free in both, and in the
    /// public directory.
    pub fn create_identity_in(&self, name: &str, key_dir: &Path) -> Result<Identity, StoreError> {
        let identity = Identity::generate(name)?;
        let file = format!("{name}.json");
        let key_path = key_dir.join(&file);
        let entry_path = self.agents_dir().join(&file);
        if key_path.exists() || entry_path.exists() || self.identities_dir().join(&file).exists() {
            return Err(StoreError::NameTaken(name.to_owned()));
        }
        for dir in [key_dir.to_owned(), self.agents_dir()] {
            fs::create_dir_all(&dir).map_err(io(&dir))?;
        }
        identity.save(&key_path)?;
        let entry = AgentEntry { name: name.to_owned(), agent: identity.agent_id() };
        let json = serde_json::to_string_pretty(&entry).expect("agent entry serializes") + "\n";
        let written = match write_new(&entry_path, json.as_bytes()) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(StoreError::NameTaken(name.to_owned())),
            other => other.map_err(io(&entry_path)),
        };
        if written.is_err() {
            // The key was made just now, for this entry alone.
            let _ = fs::remove_file(&key_path);
        }
        written?;
        Ok(identity)
    }

    pub fn identity(&self, name: &str) -> Result<Identity, StoreError> {
        identity::validate_name(name)?;
        let path = self.identities_dir().join(format!("{name}.json"));
        if !path.exists() {
            return Err(StoreError::UnknownIdentity(name.to_owned()));
        }
        Ok(Identity::load(&path)?)
    }

    /// Local identities, sorted by name.
    pub fn identities(&self) -> Result<Vec<Identity>, StoreError> {
        let dir = self.identities_dir();
        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(io(&dir)(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io(&dir))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if !entry.file_type().map_err(io(&path))?.is_file() {
                    return Err(StoreError::Corrupt { path, reason: "not a regular file (or a symlink)".into() });
                }
                // Checks the name, and that it's the file's.
                out.push(Identity::load(&path)?);
            }
        }
        out.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(out)
    }

    /// Every agent with a known name: the public directory plus local identities, sorted by name.
    ///
    /// Fails closed, as [`StoreError::Corrupt`], on a directory that could misname an agent: an entry
    /// in `agents/` that isn't a regular file named `<name>.json` for a valid name, one whose `name`
    /// isn't its file's, two entries with one name or one key, or a local identity whose key the
    /// directory lists under another name, or whose name it lists with another key. Names starting with
    /// a dot are skipped, as another process's write in flight. [`Store::open`] doesn't read `agents/`,
    /// so call this to check it.
    pub fn agents(&self) -> Result<Vec<AgentEntry>, StoreError> {
        let mut by_name: BTreeMap<String, AgentEntry> =
            self.directory()?.into_iter().map(|e| (e.name.clone(), e)).collect();
        let mut by_key: HashMap<AgentId, String> = by_name.values().map(|e| (e.agent, e.name.clone())).collect();
        for identity in self.identities()? {
            let local = AgentEntry { name: identity.name().to_owned(), agent: identity.agent_id() };
            let entry_path = self.agents_dir().join(format!("{}.json", local.name));
            match (by_name.get(&local.name), by_key.get(&local.agent)) {
                (Some(listed), _) if listed.agent != local.agent => {
                    return Err(StoreError::Corrupt {
                        path: entry_path,
                        reason: format!("lists a different key than local identity {:?}", local.name),
                    });
                }
                (_, Some(other)) if *other != local.name => {
                    return Err(StoreError::Corrupt {
                        path: self.agents_dir().join(format!("{other}.json")),
                        reason: format!("lists local identity {:?}'s key under another name", local.name),
                    });
                }
                (Some(_), _) => {}
                (None, _) => {
                    by_key.insert(local.agent, local.name.clone());
                    by_name.insert(local.name.clone(), local);
                }
            }
        }
        Ok(by_name.into_values().collect())
    }

    /// The public directory alone, `agents/`, sorted by name, never reading `identities/`: what
    /// [`Store::agents`] reads from it, with the same checks, failing closed as it does on an entry that
    /// isn't a regular file named `<name>.json` for a valid name, whose `name` isn't its file's, or a
    /// second entry for a name or a key.
    pub fn directory(&self) -> Result<Vec<AgentEntry>, StoreError> {
        let mut by_name: BTreeMap<String, AgentEntry> = BTreeMap::new();
        let mut by_key: HashMap<AgentId, String> = HashMap::new();
        let dir = self.agents_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries.collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io(&dir)(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io(&dir))?;
            let path = entry.path();
            let corrupt = |reason: String| StoreError::Corrupt { path: path.clone(), reason };
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                return Err(corrupt("not named <name>.json".into()));
            };
            if file_name.starts_with('.') {
                continue; // in-flight write from another process
            }
            let Some(stem) = file_name.strip_suffix(".json").filter(|n| identity::validate_name(n).is_ok()) else {
                return Err(corrupt("not named <name>.json for a valid name".into()));
            };
            if !entry.file_type().map_err(io(&path))?.is_file() {
                return Err(corrupt("not a regular file (or a symlink)".into()));
            }
            let text = fs::read_to_string(&path).map_err(io(&path))?;
            let listed: AgentEntry = serde_json::from_str(&text).map_err(|e| corrupt(e.to_string()))?;
            if listed.name != stem {
                return Err(corrupt(format!("names {:?}, not {stem:?}", listed.name)));
            }
            if by_name.contains_key(&listed.name) {
                return Err(corrupt(format!("a second entry named {:?}", listed.name)));
            }
            if let Some(other) = by_key.insert(listed.agent, listed.name.clone()) {
                return Err(corrupt(format!("lists the key of {other:?} under another name")));
            }
            by_name.insert(listed.name.clone(), listed);
        }
        Ok(by_name.into_values().collect())
    }
}

/// Exchanges contributions so both replicas end up holding the union, through
/// a sync session on each side (see [`crate::sync`]), so each receives only
/// what it lacked. Every message goes through its wire encoding. Returns what
/// `a` and `b` imported, respectively.
pub fn sync(a: &mut Store, b: &mut Store) -> Result<(ImportReport, ImportReport), StoreError> {
    if a.info.ground != b.info.ground {
        return Err(StoreError::DifferentGrounds(a.info.ground, b.info.ground));
    }
    let limits = Limits::default();
    let mut sides = [
        (Session::new(a.info.ground, limits), ImportReport::default()),
        (Session::new(b.info.ground, limits), ImportReport::default()),
    ];
    let stores = [a, b];
    // inbox[i] holds encoded messages for side i.
    let mut inbox: [VecDeque<Vec<u8>>; 2] = Default::default();
    for i in 0..2 {
        inbox[1 - i].extend(sides[i].0.start(stores[i].graph()).iter().map(Message::encode));
    }
    while let Some(i) = (0..2).find(|&i| !inbox[i].is_empty()) {
        let bytes = inbox[i].pop_front().expect("inbox isn't empty");
        let (session, report) = &mut sides[i];
        let message = Message::decode(&bytes, limits.max_message_bytes())?;
        let step = session.receive(stores[i].graph(), message)?;
        let mut replies = step.send;
        if let Some(batch) = step.import {
            let imported = stores[i].import(batch)?;
            replies.extend(session.imported(&imported));
            merge(report, imported);
        }
        inbox[1 - i].extend(replies.iter().map(Message::encode));
    }
    if !sides.iter().all(|(s, _)| s.is_done()) {
        return Err(StoreError::Sync(SyncError::Unexpected("end of messages before the session finished")));
    }
    let [(_, for_a), (_, for_b)] = sides;
    Ok((for_a, for_b))
}

fn merge(into: &mut ImportReport, from: ImportReport) {
    into.added.extend(from.added);
    into.already_present += from.already_present;
    into.rejected.extend(from.rejected);
    into.unresolved.extend(from.unresolved);
}

fn read_info(dir: &Path) -> Result<GroundInfo, StoreError> {
    let manifest = dir.join("ground.json");
    let text = match fs::read_to_string(&manifest) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StoreError::NotAGround(dir.to_owned())),
        Err(e) => return Err(io(&manifest)(e)),
    };
    serde_json::from_str(&text).map_err(|e| StoreError::Corrupt { path: manifest.clone(), reason: e.to_string() })
}

/// Every object file's path, and whether the listing says it's a regular file (never following a link):
/// each entry in a shard directory under `objects/`, but those whose names start with a dot (another
/// process's write in flight) or aren't UTF-8. Every entry of `objects/` not named with a dot must be a
/// real directory, by the listing's own file type: a symlinked shard, or a stray file, is
/// [`StoreError::Corrupt`].
fn object_files(dir: &Path) -> Result<Vec<(PathBuf, bool)>, StoreError> {
    let mut out = Vec::new();
    let objects = dir.join("objects");
    let shards = match fs::read_dir(&objects) {
        Ok(shards) => shards.collect::<Vec<_>>(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io(&objects)(e)),
    };
    for shard in shards {
        let shard = shard.map_err(io(&objects))?;
        let path = shard.path();
        if shard.file_name().to_str().is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        if !shard.file_type().map_err(io(&path))?.is_dir() {
            return Err(StoreError::Corrupt { path, reason: "not a shard directory (or a symlink)".into() });
        }
        let shard = path;
        for entry in fs::read_dir(&shard).map_err(io(&shard))? {
            let entry = entry.map_err(io(&shard))?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if file_name.starts_with('.') {
                continue; // in-flight write from another process
            }
            let regular = entry.file_type().map_err(io(&path))?.is_file();
            out.push((path, regular));
        }
    }
    Ok(out)
}

/// [`read_object`], refusing first an object the listing says isn't a regular file (`regular`), so a
/// symlink is never followed.
fn read_regular_object(dir: &Path, path: PathBuf, regular: bool) -> Result<Contribution, StoreError> {
    if !regular {
        return Err(StoreError::Corrupt { path, reason: "not a regular file (or a symlink)".into() });
    }
    read_object(dir, path)
}

/// Reads and decodes one object, refusing it unless its path is its id's.
fn read_object(dir: &Path, path: PathBuf) -> Result<Contribution, StoreError> {
    let bytes = fs::read(&path).map_err(io(&path))?;
    let c =
        Contribution::decode(&bytes).map_err(|e| StoreError::Corrupt { path: path.clone(), reason: e.to_string() })?;
    if path != object_path(dir, &c.id()) {
        return Err(StoreError::Corrupt { path, reason: format!("content hashes to {}", c.id()) });
    }
    Ok(c)
}

fn object_path(dir: &Path, id: &ContributionId) -> PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

/// Writes one object without following a symlink out of the Ground: `objects/` and the shard
/// must be real directories (created here if missing), and the temporary file is created new, under
/// a random name, so an existing file or symlink in its place fails the write. The file is synced
/// before the rename, which replaces whatever is at the object's path, a symlink included, rather
/// than writing through it. A failed write leaves no temporary file behind.
fn write_object(dir: &Path, c: &Contribution) -> Result<(), StoreError> {
    let path = object_path(dir, &c.id());
    let shard = path.parent().expect("object path has a shard dir");
    for d in [&dir.join("objects"), shard] {
        real_dir(d)?;
    }
    let (tmp, file) = temp_file(shard, &c.id().short()).map_err(io(shard))?;
    let written = write_synced(file, &c.encode()).map_err(io(&tmp));
    let done = written.and_then(|()| fs::rename(&tmp, &path).map_err(io(&path)));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

/// Creates a new file in `dir` named `.tmp-<pid>-<tag>-<random>`, never opening one that exists.
fn temp_file(dir: &Path, tag: &str) -> std::io::Result<(PathBuf, fs::File)> {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce)
        .map_err(|e| std::io::Error::other(format!("no randomness for a temporary name: {e}")))?;
    let tmp = dir.join(format!(".tmp-{}-{tag}-{}", std::process::id(), hex::encode(nonce)));
    let file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    Ok((tmp, file))
}

fn write_synced(mut file: fs::File, bytes: &[u8]) -> std::io::Result<()> {
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()
}

/// Writes a new file at `path` whole or not at all: the bytes go to a synced temporary file beside
/// it, which is then linked in place. The link never replaces what's at `path`, nor follows a symlink
/// there, a dangling one included: either gives [`std::io::ErrorKind::AlreadyExists`]. The temporary
/// file is always removed.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("a file has a directory");
    let (tmp, file) = temp_file(dir, "new")?;
    let done = write_synced(file, bytes).and_then(|()| fs::hard_link(&tmp, path));
    let removed = fs::remove_file(&tmp);
    done.and(removed)
}

/// Creates `d` if it doesn't exist, and refuses it if it is anything but a directory, a symlink
/// to one included.
fn real_dir(d: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(d) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(StoreError::Corrupt { path: d.to_owned(), reason: "not a directory (or a symlink)".into() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(d) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => real_dir(d),
            other => other.map_err(io(d)),
        },
        Err(e) => Err(io(d)(e)),
    }
}
