//! A verified snapshot (design note `f2cf04a0a0b7`, section 5(b)): the query tables' entries for every
//! contribution this reader has already verified, in one file outside the Ground. Opening a replica with it
//! lists `objects/`, reads only the objects the snapshot lacks, and checks those alone, instead of reading,
//! decoding, hashing and verifying every object as `Store::open` does.
//!
//! Trust: the snapshot is a local cache of this reader's own verification, never replicated. Anyone who can
//! write it could hide or alter contributions from this reader, but anyone who can write the replica
//! directory can already do worse (its identities are there). A checksum catches a damaged file, not a
//! forged one; either way a snapshot that fails to read is rebuilt from a full `Store::open`. An object the
//! snapshot already holds is never read again: if its bytes are overwritten later, this reader doesn't
//! notice, where `Store::open` would refuse the replica. That is what the snapshot trades for its speed.
//!
//! New objects get all eight validity rules (protocol.md, Validity), checked as `Graph::check` checks them:
//! the protocol version and the Ground, canonical encoding and size, the strict signature, parents strictly
//! ascending and present, every referenced id among the parents, a relation's ends differing, a retract
//! being its target's author's, and no blank statement, name or method (`str::trim`, Unicode's
//! White_Space). Plus the id matching the object's path. `Graph::check` itself can't be called: it needs
//! every parent as a `Contribution` in a `Graph`, and the snapshot holds only entries.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hasher};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use manyroot::{AgentEntry, AgentId, Contribution, ContributionId, GroundId, GroundInfo, RelationKind, Store, Verdict};

use crate::ground::{Entry, EntryBody, Ground, causal_order};

const MAGIC: &[u8; 8] = b"QLSNAP1\0";

/// How a load went.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Entries read from the snapshot.
    pub cached: usize,
    /// Objects read and checked because the snapshot lacked them.
    pub new: usize,
    /// Why the snapshot wasn't used, if it wasn't: missing, unreadable, another Ground, or stale.
    pub rebuilt: Option<String>,
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: usize) {
        self.0.extend_from_slice(&u32::try_from(v).expect("fits the snapshot format").to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes32(&mut self, v: &[u8; 32]) {
        self.0.extend_from_slice(v);
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).context("the snapshot ends early")?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")) as usize)
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }
    fn bytes32(&mut self) -> Result<[u8; 32]> {
        Ok(self.take(32)?.try_into().expect("32 bytes"))
    }
    fn id(&mut self) -> Result<ContributionId> {
        Ok(ContributionId(self.bytes32()?))
    }
    fn str(&mut self) -> Result<String> {
        let n = self.u32()?;
        Ok(String::from_utf8(self.take(n)?.to_vec())?)
    }
}

fn kind_index(k: RelationKind) -> u8 {
    RelationKind::ALL.iter().position(|x| *x == k).expect("every kind is in ALL") as u8
}

/// The snapshot's bytes for entries in causal order.
pub fn encode(ground: GroundId, entries: &[Entry]) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.bytes32(&ground.0);
    w.u64(entries.len() as u64);
    for e in entries {
        w.bytes32(&e.id.0);
        w.bytes32(&e.author.0);
        w.u64(e.timestamp_ms);
        w.u32(e.parents.len());
        for p in &e.parents {
            w.bytes32(&p.0);
        }
        match &e.body {
            EntryBody::Claim { statement, tags } => {
                w.u8(0);
                w.str(statement);
                w.u32(tags.len());
                tags.iter().for_each(|t| w.str(t));
            }
            EntryBody::Artifact { name, media_type } => {
                w.u8(1);
                w.str(name);
                w.str(media_type);
            }
            EntryBody::Relation { from, kind, to } => {
                w.u8(2);
                w.bytes32(&from.0);
                w.u8(kind_index(*kind));
                w.bytes32(&to.0);
            }
            EntryBody::Evidence { target, verdict, method } => {
                w.u8(3);
                w.bytes32(&target.0);
                w.u8(u8::from(*verdict == Verdict::Pass));
                w.str(method);
            }
            EntryBody::Retract { target } => {
                w.u8(4);
                w.bytes32(&target.0);
            }
        }
    }
    let sum = checksum(&w.0);
    w.u64(sum);
    w.0
}

/// Reads a snapshot: its Ground and its entries, in the causal order they were written in.
pub fn decode(bytes: &[u8]) -> Result<(GroundId, Vec<Entry>)> {
    ensure!(bytes.len() >= MAGIC.len() + 32 + 16, "too short for a snapshot");
    let (body, sum) = bytes.split_at(bytes.len() - 8);
    ensure!(checksum(body) == u64::from_le_bytes(sum.try_into().expect("eight bytes")), "the checksum doesn't match");
    let mut r = Reader { bytes: body, at: 0 };
    ensure!(r.take(MAGIC.len())? == MAGIC, "not a snapshot");
    let ground = GroundId(r.bytes32()?);
    let n = r.u64()?;
    let mut entries = Vec::new();
    for _ in 0..n {
        let id = r.id()?;
        let author = AgentId(r.bytes32()?);
        let timestamp_ms = r.u64()?;
        let parents = (0..r.u32()?).map(|_| r.id()).collect::<Result<Vec<_>>>()?;
        let body = match r.u8()? {
            0 => {
                let statement = r.str()?;
                let tags = (0..r.u32()?).map(|_| r.str()).collect::<Result<Vec<_>>>()?;
                EntryBody::Claim { statement, tags }
            }
            1 => EntryBody::Artifact { name: r.str()?, media_type: r.str()? },
            2 => {
                let from = r.id()?;
                let kind = *RelationKind::ALL.get(r.u8()? as usize).context("an unknown relation kind")?;
                EntryBody::Relation { from, kind, to: r.id()? }
            }
            3 => {
                let target = r.id()?;
                let verdict = if r.u8()? == 1 { Verdict::Pass } else { Verdict::Fail };
                EntryBody::Evidence { target, verdict, method: r.str()? }
            }
            4 => EntryBody::Retract { target: r.id()? },
            k => bail!("an unknown body kind {k}"),
        };
        entries.push(Entry { id, author, timestamp_ms, parents, body });
    }
    ensure!(r.at == body.len(), "trailing bytes in the snapshot");
    Ok((ground, entries))
}

/// The ids `objects/` holds, by file name, without reading any object.
fn listed(dir: &Path) -> Result<HashSet<ContributionId>> {
    let mut out = HashSet::new();
    let objects = dir.join("objects");
    let shards = match fs::read_dir(&objects) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).context("reading objects/"),
    };
    for shard in shards {
        let shard = shard?.path();
        let Some(prefix) = shard.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if !shard.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&shard)? {
            let name = entry?.file_name();
            let Some(rest) = name.to_str() else { continue };
            if rest.starts_with('.') {
                continue; // in-flight write from another process
            }
            let id: ContributionId =
                format!("{prefix}{rest}").parse().with_context(|| format!("objects/{prefix}/{rest} isn't an id"))?;
            out.insert(id);
        }
    }
    Ok(out)
}

fn object_path(dir: &Path, id: &ContributionId) -> std::path::PathBuf {
    let hex = id.to_hex();
    dir.join("objects").join(&hex[..2]).join(&hex[2..])
}

/// Reads and checks the objects `ids` names, given the entries already verified.
fn check_new(
    dir: &Path,
    ground: GroundId,
    known: &HashMap<ContributionId, AgentId>,
    ids: &[ContributionId],
) -> Result<Vec<Entry>> {
    let mut new: HashMap<ContributionId, Contribution> = HashMap::new();
    for id in ids {
        let path = object_path(dir, id);
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let c = Contribution::decode(&bytes).with_context(|| format!("{} doesn't decode", path.display()))?;
        ensure!(c.id() == *id, "{} hashes to {}", path.display(), c.id());
        let protocol = c.content().protocol;
        ensure!(
            protocol == manyroot::contribution::PROTOCOL_VERSION,
            "{id} is protocol {protocol}, not {}",
            manyroot::contribution::PROTOCOL_VERSION
        );
        ensure!(c.ground() == ground, "{id} belongs to another Ground");
        ensure!(c.signature_is_valid(), "{id}'s signature doesn't verify");
        new.insert(*id, c);
    }
    let author_of = |id: &ContributionId| known.get(id).copied().or_else(|| new.get(id).map(Contribution::author));
    for c in new.values() {
        let id = c.id();
        for p in c.parents() {
            ensure!(author_of(p).is_some(), "{id}'s parent {p} is missing from the replica");
        }
        ensure!(c.parents().windows(2).all(|w| w[0] < w[1]), "{id}'s parents aren't strictly ascending");
        let refs = c.body().references();
        for r in &refs {
            ensure!(c.parents().binary_search(r).is_ok(), "{id} refers to {r}, which isn't among its parents");
        }
        let blank = |what: &str, text: &str| -> Result<()> {
            ensure!(!text.trim().is_empty(), "{id}'s {what} is blank");
            Ok(())
        };
        match c.body() {
            manyroot::Body::Claim { statement, .. } => blank("claim statement", statement)?,
            manyroot::Body::Artifact { name, .. } => blank("artifact name", name)?,
            manyroot::Body::Evidence { method, .. } => blank("evidence method", method)?,
            manyroot::Body::Relation { from, to, .. } => ensure!(from != to, "{id} relates {from} to itself"),
            manyroot::Body::Retract { target, .. } => {
                ensure!(author_of(target) == Some(c.author()), "{id} retracts another author's {target}")
            }
        }
    }
    Ok(new.values().map(Entry::of).collect())
}

/// Refuses a snapshot path inside a Ground: in a directory holding `ground.json`, or one of its
/// subdirectories, or anywhere under a `.bracken`. The snapshot lives outside every replica.
fn ensure_outside_grounds(path: &Path) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = parent.canonicalize().with_context(|| format!("{} has no directory", path.display()))?;
    for dir in parent.ancestors() {
        ensure!(
            dir.file_name() != Some(".bracken".as_ref()) && !dir.join("ground.json").exists(),
            "the snapshot {} would be inside the Ground at {}; put it outside every replica",
            path.display(),
            dir.display()
        );
    }
    Ok(())
}

/// Opens a replica with the snapshot at `path`, refreshing the snapshot if anything changed. Refuses a
/// `path` inside a Ground.
pub fn open(dir: &Path, path: &Path) -> Result<(Ground, Report)> {
    ensure_outside_grounds(path)?;
    let info: GroundInfo = serde_json::from_str(
        &fs::read_to_string(dir.join("ground.json")).with_context(|| format!("{} isn't a replica", dir.display()))?,
    )?;
    let names = read_names(dir)?;
    let mut report = Report::default();
    let snapshot = match fs::read(path) {
        Ok(bytes) => decode(&bytes).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let on_disk = listed(dir)?;
    let usable = match snapshot {
        Ok((ground, _)) if ground != info.ground => Err("it's for another Ground".to_owned()),
        Ok((_, entries)) => {
            let ids: HashSet<ContributionId> = entries.iter().map(|e| e.id).collect();
            if ids.is_subset(&on_disk) { Ok(entries) } else { Err("it lists contributions the replica lacks".into()) }
        }
        Err(why) => Err(why),
    };
    let entries = match usable {
        Ok(cached) => {
            report.cached = cached.len();
            let known: HashMap<ContributionId, AgentId> = cached.iter().map(|e| (e.id, e.author)).collect();
            let mut ids: Vec<ContributionId> = on_disk.iter().filter(|id| !known.contains_key(id)).copied().collect();
            ids.sort();
            report.new = ids.len();
            if ids.is_empty() {
                cached
            } else {
                let mut all = cached;
                all.extend(check_new(dir, info.ground, &known, &ids)?);
                let all = causal_order(all);
                write(path, info.ground, &all)?;
                all
            }
        }
        Err(why) => {
            report.rebuilt = Some(why);
            let store = Store::open(dir)?;
            let g = store.graph();
            let entries: Vec<Entry> =
                g.topo_order().iter().map(|id| Entry::of(g.get(id).expect("ordered ids exist"))).collect();
            write(path, info.ground, &entries)?;
            entries
        }
    };
    Ok((Ground::from_ordered(entries, &names), report))
}

/// The replica's public name map, `agents/`, as `Store::agents` reads it, without opening the store (which
/// would verify every object). Local identities aren't read: a reader of a copy has none.
pub fn read_names(dir: &Path) -> Result<Vec<(String, AgentId)>> {
    let mut out = Vec::new();
    let agents = dir.join("agents");
    let entries = match fs::read_dir(&agents) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).context("reading agents/"),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let a: AgentEntry =
            serde_json::from_str(&fs::read_to_string(&path)?).with_context(|| format!("{}", path.display()))?;
        out.push((a.name, a.agent));
    }
    out.sort();
    Ok(out)
}

/// Writes a snapshot atomically: to a temporary file beside it, then renamed over it.
///
/// The bytes go to `<path>.tmp` first, created fresh (`create_new`, which never follows a symlink), then
/// renamed over `path`, which replaces a link at `path` rather than writing through it.
pub fn write(path: &Path, ground: GroundId, entries: &[Entry]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    match fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing a stale {}", tmp.display()));
        }
        _ => {}
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(&encode(ground, entries))?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}
