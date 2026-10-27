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

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::contribution::{Body, Contribution, DecodeError};
use crate::graph::{Graph, ImportReport, RuleViolation};
use crate::id::{AgentId, ContributionId, GroundId};
use crate::identity::{self, Identity, IdentityError};
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
    #[error("cannot sync ground {0} with ground {1}")]
    DifferentGrounds(GroundId, GroundId),
    #[error("sync: {0}")]
    Sync(#[from] SyncError),
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
        fs::write(&manifest, json).map_err(io(&manifest))?;
        Ok(Store { dir: dir.to_owned(), graph: Graph::new(info.ground), info })
    }

    /// Opens a replica and verifies every contribution in it.
    pub fn open(dir: &Path) -> Result<Store, StoreError> {
        let manifest = dir.join("ground.json");
        let text = match fs::read_to_string(&manifest) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StoreError::NotAGround(dir.to_owned())),
            Err(e) => return Err(io(&manifest)(e)),
        };
        let info: GroundInfo = serde_json::from_str(&text)
            .map_err(|e| StoreError::Corrupt { path: manifest.clone(), reason: e.to_string() })?;

        let mut batch = Vec::new();
        let objects = dir.join("objects");
        let shards = match fs::read_dir(&objects) {
            Ok(shards) => shards.collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io(&objects)(e)),
        };
        for shard in shards {
            let shard = shard.map_err(io(&objects))?.path();
            if !shard.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&shard).map_err(io(&shard))? {
                let path = entry.map_err(io(&shard))?.path();
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if file_name.starts_with('.') {
                    continue; // in-flight write from another process
                }
                let bytes = fs::read(&path).map_err(io(&path))?;
                let c = Contribution::decode(&bytes)
                    .map_err(|e| StoreError::Corrupt { path: path.clone(), reason: e.to_string() })?;
                if path != object_path(dir, &c.id()) {
                    return Err(StoreError::Corrupt { path, reason: format!("content hashes to {}", c.id()) });
                }
                batch.push(c);
            }
        }

        let mut graph = Graph::new(info.ground);
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
        let identity = Identity::generate(name)?;
        let key_path = self.identities_dir().join(format!("{name}.json"));
        let entry_path = self.agents_dir().join(format!("{name}.json"));
        if key_path.exists() || entry_path.exists() {
            return Err(StoreError::NameTaken(name.to_owned()));
        }
        for dir in [self.identities_dir(), self.agents_dir()] {
            fs::create_dir_all(&dir).map_err(io(&dir))?;
        }
        identity.save(&key_path)?;
        let entry = AgentEntry { name: name.to_owned(), agent: identity.agent_id() };
        let json = serde_json::to_string_pretty(&entry).expect("agent entry serializes") + "\n";
        fs::write(&entry_path, json).map_err(io(&entry_path))?;
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
            let path = entry.map_err(io(&dir))?.path();
            if path.extension().is_some_and(|e| e == "json") {
                out.push(Identity::load(&path)?);
            }
        }
        out.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(out)
    }

    /// Every agent with a known name: the public directory plus local
    /// identities, sorted by name.
    pub fn agents(&self) -> Result<Vec<AgentEntry>, StoreError> {
        let mut by_name = std::collections::BTreeMap::new();
        let dir = self.agents_dir();
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(io(&dir))?.path();
                    if path.extension().is_none_or(|e| e != "json") {
                        continue;
                    }
                    let text = fs::read_to_string(&path).map_err(io(&path))?;
                    let entry: AgentEntry = serde_json::from_str(&text)
                        .map_err(|e| StoreError::Corrupt { path: path.clone(), reason: e.to_string() })?;
                    by_name.insert(entry.name.clone(), entry);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(&dir)(e)),
        }
        for identity in self.identities()? {
            let local = AgentEntry { name: identity.name().to_owned(), agent: identity.agent_id() };
            match by_name.get(&local.name) {
                Some(listed) if listed.agent != local.agent => {
                    return Err(StoreError::Corrupt {
                        path: self.agents_dir().join(format!("{}.json", local.name)),
                        reason: format!("lists a different key than local identity {:?}", local.name),
                    });
                }
                Some(_) => {}
                None => {
                    by_name.insert(local.name.clone(), local);
                }
            }
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

fn object_path(dir: &Path, id: &ContributionId) -> PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

fn write_object(dir: &Path, c: &Contribution) -> Result<(), StoreError> {
    let path = object_path(dir, &c.id());
    let shard = path.parent().expect("object path has a shard dir");
    fs::create_dir_all(shard).map_err(io(shard))?;
    let tmp = shard.join(format!(".tmp-{}-{}", std::process::id(), c.id().short()));
    fs::write(&tmp, c.encode()).map_err(io(&tmp))?;
    fs::rename(&tmp, &path).map_err(io(&path))
}
