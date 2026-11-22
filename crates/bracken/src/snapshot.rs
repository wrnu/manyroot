//! A verified snapshot (owner decision `96e282819607`, task `ac538d60e8f9`): every contribution this
//! replica's readers have already verified, kept in one file, `cache/snapshot`, beside `objects/`. A
//! read opens the replica with [`Store::open_trusting`], which adds the snapshot's contributions without
//! reading or checking their objects again, and reads and checks only the objects the snapshot lacks,
//! all of the protocol's validity rules as [`Store::open`] checks them. At this repository's size that
//! skips about 89% of a read: the signature checks and the reading of one file per contribution.
//!
//! Trust: the snapshot is this replica's own cache of its own verification, never committed or synced.
//! `cache/` holds a `.gitignore` of `*`, so git never sees it, and nothing outside it is written. Anyone
//! who can write it could hide or alter contributions from this replica's readers, but anyone who can
//! write the replica can already do that (its keys are there). **An object the snapshot holds isn't read
//! again: if its bytes are overwritten on disk later, a cached read doesn't notice**, where `Store::open`
//! refuses the replica. Nor does it notice one made unreadable. A deleted object is noticed, since every
//! read lists `objects/`, and so is one replaced by a link, a directory or anything but a regular file. The checksum
//! catches a damaged snapshot, not a forged one. Whatever doesn't fit, a damaged, foreign or stale
//! snapshot, or one naming an object that's gone, falls back to a full `Store::open`, which then
//! rewrites it. `bracken view` never writes it.
//!
//! **Append-only segments** (task `7a3d17f979c3`): the file is a header and then segments, each a batch
//! of contributions with its own checksum. A command that added contributions (a write, or a read that
//! found objects the snapshot lacked, say after a git pull) appends one segment holding just those, in
//! causal order, so a write costs about its own object's bytes, not the whole Ground's. Once the segments
//! after the first add up to more than the first (and at least [`MIN_COMPACT_BYTES`]), the whole snapshot
//! is rewritten as one segment, so the file stays at most about twice the Ground and the rewrites cost
//! a constant number of bytes per byte added, amortised. Reading takes every segment whose checksum holds,
//! up to the first that doesn't (a torn append), drops repeats (two processes appending the same new
//! contribution) and puts the rest in causal order; a snapshot that had a bad segment is rewritten whole
//! by the next command that writes the snapshot. Two processes appending at once each append with one
//! `O_APPEND` write; a rewrite renames a new file into place, so an append racing it can land on the old
//! file and be lost, which costs only a re-read of those objects next time. The full rebuild, what
//! [`verify`] and every fallback write, stays the oracle the tests compare with.
//!
//! `BRACKEN_FULL_CHECK=1` makes every read a full `Store::open` that neither reads nor writes the
//! snapshot, for the merge gate and the Ground's guards; `bracken verify` re-reads and checks everything
//! once, then rewrites the snapshot.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use manyroot::{Contribution, ContributionId, GroundId, Store};

const MAGIC: &[u8; 8] = b"BKSNAP2\0";
const HEADER_BYTES: usize = MAGIC.len() + 32;
const CHECKSUM_BYTES: usize = 32;

/// The tail of segments after the first is compacted once it is larger than the first and than this.
pub const MIN_COMPACT_BYTES: u64 = 1 << 20;

/// Whether a read may write the snapshot. The view never writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    Yes,
    No,
}

/// `BRACKEN_FULL_CHECK` set to anything but empty or `0`.
pub fn full_check() -> bool {
    std::env::var_os("BRACKEN_FULL_CHECK").is_some_and(|v| !v.is_empty() && v != "0")
}

/// What the snapshot file holds, as far as this process knows: which contributions, how many bytes its
/// first segment and the rest take, and whether every segment read back whole.
#[derive(Clone, Debug, Default)]
struct Held {
    ids: HashSet<ContributionId>,
    base_bytes: u64,
    tail_bytes: u64,
    clean: bool,
}

/// Opens the replica in `dir`, through its snapshot unless `BRACKEN_FULL_CHECK` asks for a full check.
pub fn open(dir: &Path, refresh: Refresh) -> Result<Store> {
    Ok(open_held(dir, refresh)?.0)
}

fn open_held(dir: &Path, refresh: Refresh) -> Result<(Store, Option<Held>)> {
    if full_check() {
        return Ok((Store::open(dir)?, None));
    }
    let path = snapshot_path(dir);
    let cached = fs::read(&path).ok().and_then(|bytes| decode(&bytes).ok());
    if let Some(decoded) = cached {
        let mut held = Held {
            ids: decoded.contributions.iter().map(Contribution::id).collect(),
            base_bytes: decoded.base_bytes,
            tail_bytes: decoded.tail_bytes,
            clean: decoded.clean,
        };
        let ground = decoded.ground;
        if let Ok(store) = Store::open_trusting(dir, decoded.contributions)
            && store.info().ground == ground
        {
            if refresh == Refresh::Yes && (store.graph().len() != held.ids.len() || !held.clean) {
                // Best effort: a cache that can't be written is only slower next time.
                let _ = save(dir, &store, &mut held);
            }
            return Ok((store, Some(held)));
        }
    }
    let store = Store::open(dir)?;
    let mut held = Held::default();
    if refresh == Refresh::Yes {
        let _ = save(dir, &store, &mut held);
    }
    Ok((store, Some(held)))
}

/// A store opened through the snapshot that adds to it when dropped, if the store gained contributions
/// since: a write leaves the snapshot current, so the reads after it find nothing new and write nothing.
/// Only objects that arrive another way (a git pull, another tool) make a read write the snapshot.
pub struct Refreshing {
    store: Option<Store>,
    held: Option<Held>,
    refresh: Refresh,
}

impl Refreshing {
    pub fn open(dir: &Path, refresh: Refresh) -> Result<Refreshing> {
        let (store, held) = open_held(dir, refresh)?;
        Ok(Refreshing { store: Some(store), held, refresh })
    }

    /// The store, for a command that keeps it (`watch`, `serve`): it reads through the snapshot itself.
    pub fn into_inner(mut self) -> Store {
        self.store.take().expect("taken only here, once")
    }
}

impl Deref for Refreshing {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.store.as_ref().expect("present until into_inner")
    }
}

impl DerefMut for Refreshing {
    fn deref_mut(&mut self) -> &mut Store {
        self.store.as_mut().expect("present until into_inner")
    }
}

impl Drop for Refreshing {
    fn drop(&mut self) {
        if let (Some(store), Some(held)) = (&self.store, &mut self.held)
            && self.refresh == Refresh::Yes
            && store.graph().len() != held.ids.len()
            && !full_check()
        {
            let _ = save(store.dir(), store, held);
        }
    }
}

/// `bracken verify`: a full `Store::open`, then the snapshot rewritten whole from it.
pub fn verify(dir: &Path) -> Result<Store> {
    let store = Store::open(dir)?;
    if !full_check() {
        rewrite(dir, &store, &mut Held::default()).context("writing the snapshot")?;
    }
    Ok(store)
}

pub fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("cache").join("snapshot")
}

fn checksum(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

fn header(ground: GroundId) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(ground.as_bytes());
    out
}

/// One segment: its length (u64), then the number of contributions (u64) and each one's length (u32) and
/// encoding, then a checksum of everything before it in the segment.
fn segment(contributions: &[&Contribution]) -> Vec<u8> {
    let mut body = u64::try_from(contributions.len()).expect("fits").to_le_bytes().to_vec();
    for c in contributions {
        let bytes = c.encode();
        body.extend_from_slice(&u32::try_from(bytes.len()).expect("a contribution is at most 16 MiB").to_le_bytes());
        body.extend_from_slice(&bytes);
    }
    let mut out = u64::try_from(body.len()).expect("fits").to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    let sum = checksum(&out);
    out.extend_from_slice(&sum);
    out
}

/// A whole snapshot of `contributions`, which must be in causal order: the header and one segment.
fn encode(ground: GroundId, contributions: &[&Contribution]) -> Vec<u8> {
    let mut out = header(ground);
    out.extend_from_slice(&segment(contributions));
    out
}

#[derive(Debug)]
struct Decoded {
    ground: GroundId,
    /// Each contribution once, in causal order (parents in the snapshot before their children).
    contributions: Vec<Contribution>,
    base_bytes: u64,
    tail_bytes: u64,
    /// Every byte after the header was a whole segment.
    clean: bool,
}

/// Reads one segment at the start of `bytes`: its contributions and its length, or why not.
fn read_segment(bytes: &[u8]) -> Result<(Vec<Contribution>, usize)> {
    ensure!(bytes.len() >= 8, "truncated");
    let len = usize::try_from(u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes")))?;
    let end = 8usize.checked_add(len).context("too long")?;
    let Some(sum) = bytes.get(end..end + CHECKSUM_BYTES) else { bail!("truncated") };
    ensure!(checksum(&bytes[..end]) == sum, "the checksum doesn't match");
    let body = &bytes[8..end];
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8]> {
        let Some(slice) = body.get(at..at + n) else { bail!("truncated") };
        at += n;
        Ok(slice)
    };
    let count = u64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
    let mut out = Vec::new();
    for _ in 0..count {
        let n = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes")) as usize;
        out.push(Contribution::decode(take(n)?)?);
    }
    ensure!(at == body.len(), "trailing bytes in a segment");
    Ok((out, end + CHECKSUM_BYTES))
}

/// The snapshot's Ground and contributions. Refuses a file without this version's header or whose first
/// segment doesn't read back; takes the segments after it up to the first that doesn't.
fn decode(bytes: &[u8]) -> Result<Decoded> {
    ensure!(bytes.len() >= HEADER_BYTES, "too short");
    ensure!(&bytes[..MAGIC.len()] == MAGIC, "not a snapshot of this version");
    let ground = GroundId(bytes[MAGIC.len()..HEADER_BYTES].try_into().expect("32 bytes"));
    let mut at = HEADER_BYTES;
    let (mut all, base) = read_segment(&bytes[at..]).context("the first segment")?;
    at += base;
    let mut clean = true;
    while at < bytes.len() {
        match read_segment(&bytes[at..]) {
            Ok((more, n)) => {
                all.extend(more);
                at += n;
            }
            Err(_) => {
                clean = false;
                break;
            }
        }
    }
    Ok(Decoded {
        ground,
        contributions: causal(all),
        base_bytes: base as u64,
        tail_bytes: (at - HEADER_BYTES - base) as u64,
        clean,
    })
}

/// `contributions` with repeats dropped (the first kept) and in causal order: as they are if they already
/// are (the usual case), else parents moved before children, the rest keeping their order. A parent that
/// isn't among them is left for [`Store::open_trusting`] to refuse.
fn causal(contributions: Vec<Contribution>) -> Vec<Contribution> {
    let mut seen = HashSet::new();
    let mut ordered = true;
    let mut unique = Vec::with_capacity(contributions.len());
    for c in contributions {
        if !seen.insert(c.id()) {
            continue;
        }
        unique.push(c);
    }
    let mut before = HashSet::new();
    for c in &unique {
        if c.parents().iter().any(|p| seen.contains(p) && !before.contains(p)) {
            ordered = false;
            break;
        }
        before.insert(c.id());
    }
    if ordered {
        return unique;
    }
    // Depth-first, parents first, in the file's order.
    let index: HashMap<ContributionId, usize> = unique.iter().enumerate().map(|(i, c)| (c.id(), i)).collect();
    let mut slots: Vec<Option<Contribution>> = unique.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    let mut done = vec![false; slots.len()];
    for start in 0..slots.len() {
        let mut stack = vec![(start, false)];
        while let Some((i, expanded)) = stack.pop() {
            if done[i] {
                continue;
            }
            if expanded {
                done[i] = true;
                out.push(slots[i].take().expect("emitted once"));
                continue;
            }
            stack.push((i, true));
            let parents = slots[i].as_ref().expect("not emitted yet").parents();
            for p in parents.iter().rev() {
                if let Some(&j) = index.get(p).filter(|&&j| !done[j]) {
                    stack.push((j, false));
                }
            }
        }
    }
    out
}

/// Brings the snapshot up to `store`: appends a segment of what it lacks, or rewrites it whole when it
/// isn't clean (or isn't there yet: a fresh [`Held`] isn't clean), can't be appended to, or its tail would outgrow its first segment. Updates `held` to match.
fn save(dir: &Path, store: &Store, held: &mut Held) -> Result<()> {
    if !held.clean {
        return rewrite(dir, store, held);
    }
    let g = store.graph();
    let new: Vec<&Contribution> = g
        .topo_order()
        .iter()
        .filter(|id| !held.ids.contains(id))
        .map(|id| g.get(id).expect("the order lists the graph's own ids"))
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let bytes = segment(&new);
    let tail = held.tail_bytes + bytes.len() as u64;
    if tail > held.base_bytes.max(MIN_COMPACT_BYTES) {
        return rewrite(dir, store, held);
    }
    if append(dir, &bytes).is_err() {
        // Gone, a link, or no append here: the whole snapshot instead, renamed into place.
        return rewrite(dir, store, held);
    }
    held.ids.extend(new.iter().map(|c| c.id()));
    held.tail_bytes = tail;
    Ok(())
}

/// `cache/`, created with its `.gitignore` if missing; refuses one that isn't a real directory.
fn cache_dir(dir: &Path) -> Result<PathBuf> {
    let cache = dir.join("cache");
    match fs::symlink_metadata(&cache) {
        Ok(m) => ensure!(m.is_dir(), "{} isn't a directory", cache.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&cache).with_context(|| format!("creating {}", cache.display()))?;
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", cache.display())),
    }
    let ignore = cache.join(".gitignore");
    if fs::symlink_metadata(&ignore).is_err() {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&ignore)?;
        f.write_all(b"# bracken's local cache of its own verification: never committed\n*\n")?;
    }
    Ok(cache)
}

/// Appends one segment to the snapshot with a single `O_APPEND` write, opening it without following a
/// link and refusing anything but a regular file. Only on Unix; elsewhere the caller rewrites instead.
#[cfg(unix)]
fn append(dir: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    cache_dir(dir)?;
    let path = snapshot_path(dir);
    let mut f = fs::OpenOptions::new()
        .append(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    ensure!(f.metadata()?.is_file(), "{} isn't a regular file", path.display());
    f.write_all(bytes).with_context(|| format!("appending to {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn append(_: &Path, _: &[u8]) -> Result<()> {
    bail!("appending needs a no-follow open")
}

/// Writes the snapshot of `store` whole and atomically: to a temporary file of its own in `cache/`,
/// created fresh (`create_new`, which never follows a link), then renamed over `cache/snapshot`, which
/// replaces a link there rather than writing through it. Refuses a `cache` that isn't a real directory.
fn rewrite(dir: &Path, store: &Store, held: &mut Held) -> Result<()> {
    let cache = cache_dir(dir)?;
    let g = store.graph();
    let order: Vec<&Contribution> =
        g.topo_order().iter().map(|id| g.get(id).expect("the order lists the graph's own ids")).collect();
    let bytes = encode(store.info().ground, &order);
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("no randomness for a temporary name: {e}"))?;
    let tmp = cache.join(format!(".snapshot-{}-{}.tmp", std::process::id(), u64::from_le_bytes(nonce)));
    let result = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&bytes)?;
        fs::rename(&tmp, snapshot_path(dir))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    *held = Held {
        ids: order.iter().map(|c| c.id()).collect(),
        base_bytes: (bytes.len() - HEADER_BYTES) as u64,
        tail_bytes: 0,
        clean: true,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manyroot::{Body, Identity};

    fn chain(n: usize) -> Vec<Contribution> {
        let who = Identity::generate("t").expect("a key");
        let mut out: Vec<Contribution> = Vec::new();
        for i in 0..n {
            let parents: Vec<ContributionId> = out.last().map(Contribution::id).into_iter().collect();
            let body = Body::Claim { statement: format!("c{i}"), tags: vec![] };
            out.push(Contribution::create(GroundId([7; 32]), &who, parents, i as u64, body));
        }
        out
    }

    fn refs(cs: &[Contribution]) -> Vec<&Contribution> {
        cs.iter().collect()
    }

    #[test]
    fn decode_refuses_what_encode_did_not_write() {
        let ground = GroundId([7; 32]);
        let bytes = encode(ground, &[]);
        assert_eq!(decode(&bytes).expect("round trip").ground, ground);
        for cut in [0, 1, 40, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut flipped = bytes.clone();
        flipped[50] ^= 1;
        assert!(decode(&flipped).is_err());
        let mut old = bytes.clone();
        old[..8].copy_from_slice(b"BKSNAP1\0");
        assert!(decode(&old).is_err(), "the whole-file version isn't read");
    }

    #[test]
    fn appended_segments_read_back_and_a_torn_one_ends_the_read() {
        let cs = chain(6);
        let mut bytes = encode(GroundId([7; 32]), &refs(&cs[..2]));
        bytes.extend(segment(&refs(&cs[2..4])));
        let whole = decode(&bytes).expect("reads");
        assert!(whole.clean);
        assert_eq!(
            whole.contributions.iter().map(Contribution::id).collect::<Vec<_>>(),
            cs[..4].iter().map(Contribution::id).collect::<Vec<_>>()
        );
        let torn = segment(&refs(&cs[4..]));
        bytes.extend_from_slice(&torn[..torn.len() - 3]);
        let d = decode(&bytes).expect("the prefix reads");
        assert!(!d.clean, "a torn segment is noticed");
        assert_eq!(d.contributions.len(), 4);
    }

    /// The oracle: whatever the snapshot holds after appends, races, outside arrivals and compactions is
    /// exactly the replica a full open reads, and opening through it gives the full open's graph.
    fn agrees_with_a_full_rebuild(dir: &Path, when: &str) -> Decoded {
        let full = Store::open(dir).expect("valid");
        let d = decode(&fs::read(snapshot_path(dir)).expect("written")).expect("decodes");
        assert!(d.clean, "{when}: clean");
        assert_eq!(d.ground, full.info().ground, "{when}");
        let ids: HashSet<ContributionId> = d.contributions.iter().map(Contribution::id).collect();
        assert_eq!(ids.len(), d.contributions.len(), "{when}: no repeats");
        let all: HashSet<ContributionId> = full.graph().iter().map(Contribution::id).collect();
        assert_eq!(ids, all, "{when}: the snapshot holds exactly the replica");
        let cached = Store::open_trusting(dir, d.contributions.clone()).expect("the snapshot fits");
        assert_eq!(cached.graph().topo_order(), full.graph().topo_order(), "{when}");
        let heads = |s: &Store| s.graph().heads().collect::<HashSet<_>>();
        assert_eq!(heads(&cached), heads(&full), "{when}");
        d
    }

    fn write(dir: &Path, who: &Identity, body: Body) {
        let mut s = Refreshing::open(dir, Refresh::Yes).expect("opens");
        s.contribute(who, body).expect("adds");
    }

    fn claim(n: usize) -> Body {
        Body::Claim { statement: format!("claim {n}"), tags: vec!["note".into()] }
    }

    #[test]
    fn appends_agree_with_a_full_rebuild() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("g");
        let store = Store::init(&dir, "t").expect("a Ground");
        let who = store.create_identity("w").expect("a key");
        drop(store);
        write(&dir, &who, claim(0));
        // Opening the empty Ground wrote an empty snapshot whole; the write appended to it.
        agrees_with_a_full_rebuild(&dir, "first write");

        // Each later write appends one segment of about its object's size, leaving the bytes before alone.
        for i in 1..20 {
            let before = fs::read(snapshot_path(&dir)).expect("written");
            write(&dir, &who, claim(i));
            let after = fs::read(snapshot_path(&dir)).expect("written");
            assert_eq!(&after[..before.len()], &before[..], "write {i} appended");
            assert!(after.len() - before.len() < 600, "write {i} added {} bytes", after.len() - before.len());
            agrees_with_a_full_rebuild(&dir, &format!("write {i}"));
        }

        // Two processes at once: both opened before either wrote, each appends its own.
        let mut a = Refreshing::open(&dir, Refresh::Yes).expect("opens");
        let mut b = Refreshing::open(&dir, Refresh::Yes).expect("opens");
        a.contribute(&who, claim(100)).expect("adds");
        b.contribute(&who, claim(101)).expect("adds");
        drop(a);
        drop(b);
        agrees_with_a_full_rebuild(&dir, "two at once");

        // Objects that arrive another way (a pull): the next read appends them.
        let mut other = Store::open(&dir).expect("valid");
        for i in 200..205 {
            other.contribute(&who, claim(i)).expect("adds");
        }
        let n = open(&dir, Refresh::Yes).expect("reads").graph().len();
        assert_eq!(agrees_with_a_full_rebuild(&dir, "after a pull").contributions.len(), n);

        // A read that never writes leaves an outside arrival for the next one.
        let mut other = Store::open(&dir).expect("valid");
        other.contribute(&who, claim(300)).expect("adds");
        let before = fs::read(snapshot_path(&dir)).expect("written");
        open(&dir, Refresh::No).expect("reads");
        assert_eq!(fs::read(snapshot_path(&dir)).expect("kept"), before);
        open(&dir, Refresh::Yes).expect("reads");
        agrees_with_a_full_rebuild(&dir, "after the view");

        // A torn append: the next command rewrites the snapshot whole as it opens, then appends its write.
        let mut torn = fs::read(snapshot_path(&dir)).expect("written");
        torn.extend_from_slice(&[1, 2, 3]);
        fs::write(snapshot_path(&dir), &torn).expect("written");
        write(&dir, &who, claim(400));
        let d = agrees_with_a_full_rebuild(&dir, "after a torn append");
        assert!(d.tail_bytes < 600, "rewritten whole, then one write appended: {}", d.tail_bytes);
    }

    #[test]
    fn a_tail_larger_than_the_first_segment_is_compacted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("g");
        let store = Store::init(&dir, "t").expect("a Ground");
        let who = store.create_identity("w").expect("a key");
        drop(store);
        write(&dir, &who, claim(0));
        let big = |i: u8| Body::Artifact { name: format!("a{i}"), media_type: "x".into(), content: vec![i; 400 << 10] };
        let mut tails = Vec::new();
        for i in 0..6 {
            write(&dir, &who, big(i));
            let d = agrees_with_a_full_rebuild(&dir, &format!("artifact {i}"));
            assert!(d.tail_bytes <= d.base_bytes.max(MIN_COMPACT_BYTES), "artifact {i}: the tail stays bounded");
            tails.push(d.tail_bytes);
        }
        assert!(tails.contains(&0) && tails.iter().any(|t| *t > 0), "both appends and a compaction: {tails:?}");
    }

    #[test]
    fn repeats_are_dropped_and_order_restored() {
        let cs = chain(5);
        // Two appenders: one wrote 3 and 4, the other 2, 3 and 4 again, after it.
        let mut bytes = encode(GroundId([7; 32]), &refs(&cs[..2]));
        bytes.extend(segment(&[&cs[4], &cs[3]]));
        bytes.extend(segment(&refs(&cs[2..])));
        let d = decode(&bytes).expect("reads");
        assert!(d.clean);
        let ids: Vec<_> = d.contributions.iter().map(Contribution::id).collect();
        assert_eq!(ids, cs.iter().map(Contribution::id).collect::<Vec<_>>());
    }
}
