//! A verified snapshot (owner decision `96e282819607`, task `ac538d60e8f9`): every contribution this
//! replica's readers have already verified, kept in one file, `cache/snapshot`, beside `objects/`. A
//! read opens the replica with [`Store::open_trusting_with`], which adds the snapshot's contributions
//! without reading or checking their objects again, and reads and checks only the objects the snapshot
//! lacks, all of the protocol's validity rules as [`Store::open`] checks them. At this repository's size
//! that skips about 89% of a read: the signature checks and the reading of one file per contribution.
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
//! after the first add up to more than the first (and at least [`MIN_COMPACT_BYTES`]), the next command
//! that writes the snapshot, a read included, rewrites it as one segment, so the file stays at most about
//! twice the Ground and the rewrites cost a constant number of bytes per byte added, amortised.
//!
//! **Reading is bounded by `objects/`** (finding `ed533a35b961`): the snapshot is read as a stream of
//! entries, each framed with its id, length and own hash (`BKSNAP3`). Each id is checked before its
//! encoding is read, an encoding is at most a contribution's largest size, and encodings are hashed and
//! decoded in parallel in bounded batches, each by [`Contribution::decode`] (one canonical encoding) to a
//! contribution with its framed id. The read stops at the first
//! contribution the replica holds no object for (the whole snapshot is then set aside for a full open),
//! at a repeat or an empty segment, or at a segment whose framing or checksum doesn't hold (a torn
//! append). So every contribution read, kept or buffered in a segment, is a distinct object of the
//! replica, and what a read decodes is at most what `objects/` holds plus one contribution. A read that stopped early keeps what came
//! before, reads the rest from the objects, and rewrites the snapshot whole. Two processes appending at
//! once each append with one `O_APPEND` write; a rewrite renames a new file into place, so an append
//! racing it can land on the old file and be lost, which costs only a re-read of those objects next time.
//!
//! **Nothing is followed through a link** (finding `4c97cc23ad23`): `cache/` is opened once, with
//! `O_DIRECTORY | O_NOFOLLOW`, and everything in it is opened relative to that descriptor, with
//! `O_NOFOLLOW | O_NONBLOCK`, its type checked on the open descriptor before a byte is read or written;
//! the temporary file is created new and renamed, and removed, relative to it. So a `cache/` or a
//! `snapshot` swapped for a link, or a FIFO, is never read, written or waited on: the read is a full one
//! and the snapshot is replaced (or, for a `cache/` that isn't a real directory, never written).
//!
//! The full rebuild, what [`verify`] and every fallback write, stays the oracle the tests compare with.
//! `BRACKEN_FULL_CHECK=1` makes every read a full `Store::open` that neither reads nor writes the
//! snapshot, for the merge gate and the Ground's guards; `bracken verify` re-reads and checks everything
//! once, then rewrites the snapshot.

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read};
use std::ops::{Deref, DerefMut};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use manyroot::contribution::MAX_CONTRIBUTION_BYTES;
use manyroot::{Contribution, ContributionId, GroundId, Store};

use cache::Cache;

const MAGIC: &[u8; 8] = b"BKSNAP3\0";
const HEADER_BYTES: u64 = MAGIC.len() as u64 + 32;
const CHECKSUM_BYTES: u64 = 32;
const SNAPSHOT: &str = "snapshot";

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

impl Held {
    /// Whether the tail has outgrown the first segment, and the snapshot should be rewritten whole.
    fn over(&self) -> bool {
        self.tail_bytes > self.base_bytes.max(MIN_COMPACT_BYTES)
    }
}

/// Opens the replica in `dir`, through its snapshot unless `BRACKEN_FULL_CHECK` asks for a full check.
pub fn open(dir: &Path, refresh: Refresh) -> Result<Store> {
    Ok(open_held(dir, refresh)?.0)
}

fn open_held(dir: &Path, refresh: Refresh) -> Result<(Store, Option<Held>)> {
    // The index's stamps are taken before anything lists objects/ (docs/design/index-store.md section 5),
    // and only by an open that may write: the view never touches the index.
    let index = refresh == Refresh::Yes && crate::index::usable(dir);
    if full_check() {
        let before = crate::index::Before::stats(dir);
        let store = Store::open(dir)?;
        if index {
            crate::index::compare(dir, &store, &before)?;
        }
        return Ok((store, None));
    }
    let before = index.then(|| crate::index::Before::take(dir));
    let mut decoded: Option<Decoded> = None;
    let opened = Store::open_trusting_with(dir, |ground, on_disk| {
        decoded = read_snapshot(dir, ground, on_disk);
        decoded.as_mut().map(|d| std::mem::take(&mut d.contributions)).unwrap_or_default()
    });
    let (store, mut held) = match (opened, decoded) {
        (Ok(store), Some(d)) => {
            (store, Held { ids: d.ids, base_bytes: d.base_bytes, tail_bytes: d.tail_bytes, clean: d.clean })
        }
        // No snapshot to trust: that was a full open.
        (Ok(store), None) => (store, Held::default()),
        // The snapshot didn't fit the replica: a full open says what's really wrong, if anything.
        (Err(_), Some(_)) => (Store::open(dir)?, Held::default()),
        (Err(e), None) => return Err(e.into()),
    };
    if refresh == Refresh::Yes && (store.graph().len() != held.ids.len() || !held.clean || held.over()) {
        // Best effort: a cache that can't be written is only slower next time.
        let _ = save(dir, &store, &mut held);
    }
    if let Some(before) = &before {
        // Best effort too: a lock not granted in time, or a failure, leaves the index for a later command.
        let _ = crate::index::refresh(dir, &store, before, crate::index::Record::Yes);
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
    /// How many contributions the store held when opened: more when dropped means this command wrote.
    opened: usize,
}

impl Refreshing {
    pub fn open(dir: &Path, refresh: Refresh) -> Result<Refreshing> {
        let (store, held) = open_held(dir, refresh)?;
        let opened = store.graph().len();
        Ok(Refreshing { store: Some(store), held, refresh, opened })
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
        // A test hook, in debug builds only: the process dies after its objects were written and before
        // the snapshot or the index records them (docs/design/index-store.md section 8, Process crash).
        #[cfg(debug_assertions)]
        if std::env::var_os("BRACKEN_TEST_ABORT_BEFORE_INDEX").is_some() {
            std::process::abort();
        }
        // The index learns this command's writes (section 6): no stamps, the written shards left dirty.
        if let Some(store) = &self.store
            && self.refresh == Refresh::Yes
            && store.graph().len() != self.opened
            && !full_check()
            && crate::index::usable(store.dir())
        {
            let before = crate::index::Before::stats(store.dir());
            let _ = crate::index::refresh(store.dir(), store, &before, crate::index::Record::No);
        }
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
    // Under BRACKEN_FULL_CHECK nothing is written, cache/clock included.
    let before = if full_check() { crate::index::Before::stats(dir) } else { crate::index::Before::take(dir) };
    let store = Store::open(dir)?;
    if !full_check() {
        let cache = Cache::open(dir, true).context("opening cache/")?.context("no cache/ was made")?;
        rewrite(&cache, &store, &mut Held::default()).context("writing the snapshot")?;
        if crate::index::usable(dir) {
            crate::index::rebuild(dir, &store, &before).context("rebuilding the index")?;
        }
    }
    Ok(store)
}

fn header(ground: GroundId) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(ground.as_bytes());
    out
}

/// One segment: its length (u64), then the number of contributions (u64) and for each its id, its
/// encoding's length (u32) and blake3 hash, and the encoding; then a blake3 checksum of everything in the
/// segment before it except the encodings, which their own hashes cover. So a read can check an entry's
/// id against the replica before reading its encoding, and hash and decode encodings in parallel.
fn segment(contributions: &[&Contribution]) -> Vec<u8> {
    let mut body = u64::try_from(contributions.len()).expect("fits").to_le_bytes().to_vec();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&body);
    for c in contributions {
        let bytes = c.encode();
        let mut head = c.id().as_bytes().to_vec();
        head.extend_from_slice(&u32::try_from(bytes.len()).expect("a contribution is at most 16 MiB").to_le_bytes());
        head.extend_from_slice(blake3::hash(&bytes).as_bytes());
        hasher.update(&head);
        body.extend_from_slice(&head);
        body.extend_from_slice(&bytes);
    }
    let len = u64::try_from(body.len()).expect("fits").to_le_bytes();
    let mut sums = blake3::Hasher::new();
    sums.update(&len);
    sums.update(hasher.finalize().as_bytes());
    let mut out = len.to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(sums.finalize().as_bytes());
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
    /// Each contribution once, in causal order (parents in the snapshot before their children).
    contributions: Vec<Contribution>,
    ids: HashSet<ContributionId>,
    base_bytes: u64,
    tail_bytes: u64,
    /// Every byte after the header was a whole segment, with no repeat and no empty segment.
    clean: bool,
}

/// The snapshot of the replica in `dir`, if there is one to trust: `None` for none, one that can't be
/// opened as a regular file in a real `cache/`, or one [`decode`] sets aside.
fn read_snapshot(dir: &Path, ground: GroundId, on_disk: &dyn Fn(&ContributionId) -> bool) -> Option<Decoded> {
    let cache = Cache::open(dir, false).ok()??;
    let (file, size) = cache.read().ok()??;
    decode(BufReader::new(file), size, ground, on_disk).ok()
}

/// Why a segment wasn't taken.
enum Stop {
    /// Torn, damaged, empty or repeating: the read ends before it.
    Torn,
    /// Names a contribution the replica holds no object for: the read stops there, and the snapshot is
    /// set aside. (A damaged id lands here too, before its segment's checksum is read: it costs only the
    /// full open a torn segment would have cost from that point.)
    Missing,
}

fn read_u64(r: &mut impl Read) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

/// The bytes of one entry's framing: its id, length and hash.
const ENTRY_HEAD_BYTES: u64 = 32 + 4 + 32;

/// Encodings are decoded in batches of about this many bytes, in parallel, so what a read buffers before
/// checking it is bounded. Smaller under test, so the tests cross batches.
const BATCH_BYTES: usize = if cfg!(test) { 1 << 20 } else { 64 << 20 };

/// An entry read but not yet checked: its id and hash as framed, and its encoding.
type Raw = (ContributionId, [u8; 32], Vec<u8>);

/// Hashes and decodes `raw` across threads: each encoding must hash to its framed hash and decode, as
/// [`Contribution::decode`] decodes (one canonical encoding), to a contribution with its framed id.
fn decode_batch(raw: Vec<Raw>) -> Result<Vec<Contribution>, Stop> {
    let one = |(id, sum, bytes): &Raw| -> Result<Contribution, Stop> {
        if blake3::hash(bytes).as_bytes() != sum {
            return Err(Stop::Torn);
        }
        let c = Contribution::decode(bytes).map_err(|_| Stop::Torn)?;
        if c.id() != *id {
            return Err(Stop::Torn);
        }
        Ok(c)
    };
    let total: usize = raw.iter().map(|(_, _, b)| b.len()).sum();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
    if threads == 1 || total < (1 << 20) {
        return raw.iter().map(one).collect();
    }
    let chunk = raw.len().div_ceil(threads).max(1);
    let parts: Vec<Result<Vec<Contribution>, Stop>> = std::thread::scope(|scope| {
        let handles: Vec<_> =
            raw.chunks(chunk).map(|part| scope.spawn(move || part.iter().map(one).collect())).collect();
        handles.into_iter().map(|h| h.join().unwrap_or(Err(Stop::Torn))).collect()
    });
    let mut out = Vec::with_capacity(raw.len());
    for part in parts {
        out.extend(part?);
    }
    Ok(out)
}

/// Reads the segment at the start of `r`, at most `remaining` bytes, refusing a contribution already in
/// `seen`, and an empty segment unless it is the `first`; returns its contributions and its length. Each
/// entry's id is checked (a repeat, or one the replica holds no object for) before its encoding is read.
fn read_segment(
    r: &mut impl Read,
    remaining: u64,
    seen: &HashSet<ContributionId>,
    on_disk: &dyn Fn(&ContributionId) -> bool,
    first: bool,
) -> Result<(Vec<Contribution>, u64), Stop> {
    let len = read_u64(r).ok_or(Stop::Torn)?;
    let total = len.checked_add(8 + CHECKSUM_BYTES).filter(|t| *t <= remaining).ok_or(Stop::Torn)?;
    let mut hasher = blake3::Hasher::new();
    let count = read_u64(r).ok_or(Stop::Torn)?;
    hasher.update(&count.to_le_bytes());
    let mut left = len.checked_sub(8).ok_or(Stop::Torn)?;
    // Only an empty Ground's snapshot has an empty segment, its first; nor can more entries than their
    // framing fit.
    if (count == 0 && !first) || count > left / ENTRY_HEAD_BYTES {
        return Err(Stop::Torn);
    }
    let mut out = Vec::new();
    let mut ids = HashSet::new();
    let mut batch: Vec<Raw> = Vec::new();
    let mut batched = 0usize;
    for _ in 0..count {
        let mut head = [0u8; ENTRY_HEAD_BYTES as usize];
        r.read_exact(&mut head).map_err(|_| Stop::Torn)?;
        hasher.update(&head);
        left = left.checked_sub(ENTRY_HEAD_BYTES).ok_or(Stop::Torn)?;
        let id = ContributionId(head[..32].try_into().expect("32 bytes"));
        let n = u32::from_le_bytes(head[32..36].try_into().expect("4 bytes"));
        let sum: [u8; 32] = head[36..].try_into().expect("32 bytes");
        if n as usize > MAX_CONTRIBUTION_BYTES || u64::from(n) > left {
            return Err(Stop::Torn);
        }
        left -= u64::from(n);
        if seen.contains(&id) || !ids.insert(id) {
            return Err(Stop::Torn);
        }
        // At the first contribution the replica holds no object for, stop: its encoding and nothing after
        // it is read (findings 937b52d0164e, 4dc0418ed33c), so what a segment buffers is bounded by objects/.
        if !on_disk(&id) {
            return Err(Stop::Missing);
        }
        let mut bytes = Vec::with_capacity(n as usize);
        r.by_ref().take(u64::from(n)).read_to_end(&mut bytes).map_err(|_| Stop::Torn)?;
        if bytes.len() != n as usize {
            return Err(Stop::Torn);
        }
        batched += bytes.len();
        batch.push((id, sum, bytes));
        if batched >= BATCH_BYTES {
            out.extend(decode_batch(std::mem::take(&mut batch))?);
            batched = 0;
        }
    }
    out.extend(decode_batch(batch)?);
    if left != 0 {
        return Err(Stop::Torn);
    }
    let mut sum = [0u8; CHECKSUM_BYTES as usize];
    r.read_exact(&mut sum).map_err(|_| Stop::Torn)?;
    let mut sums = blake3::Hasher::new();
    sums.update(&len.to_le_bytes());
    sums.update(hasher.finalize().as_bytes());
    if *sums.finalize().as_bytes() != sum {
        return Err(Stop::Torn);
    }
    Ok((out, total))
}

/// The snapshot of `size` bytes read from `r`, for the replica of Ground `ground` whose objects
/// `on_disk` names. Refuses one without this version's header or this Ground's id, one whose first
/// segment doesn't read back, and one that names a contribution `on_disk` doesn't; takes the segments
/// after the first up to the first that doesn't read back, repeats a contribution or is empty.
fn decode(mut r: impl Read, size: u64, ground: GroundId, on_disk: &dyn Fn(&ContributionId) -> bool) -> Result<Decoded> {
    ensure!(size >= HEADER_BYTES, "too short");
    let mut head = [0u8; HEADER_BYTES as usize];
    r.read_exact(&mut head)?;
    ensure!(head == header(ground)[..], "not a snapshot of this version and Ground");
    let mut remaining = size - HEADER_BYTES;
    let mut ids = HashSet::new();
    let mut all = Vec::new();
    let mut base = None;
    let mut tail = 0;
    let mut clean = true;
    while remaining > 0 {
        match read_segment(&mut r, remaining, &ids, on_disk, base.is_none()) {
            Ok((more, n)) => {
                ids.extend(more.iter().map(Contribution::id));
                all.extend(more);
                if base.is_none() {
                    base = Some(n);
                } else {
                    tail += n;
                }
                remaining -= n;
            }
            Err(Stop::Missing) => bail!("names a contribution the replica holds no object for"),
            Err(Stop::Torn) => {
                clean = false;
                break;
            }
        }
    }
    let base_bytes = base.context("no whole first segment")?;
    Ok(Decoded { contributions: causal(all), ids, base_bytes, tail_bytes: tail, clean })
}

/// `contributions` (each once) in causal order: as they are if they already are (the usual case), else
/// parents moved before children, the rest keeping their order. A parent that isn't among them is left
/// for [`Store::open_trusting_with`] to refuse.
fn causal(contributions: Vec<Contribution>) -> Vec<Contribution> {
    let all: HashSet<ContributionId> = contributions.iter().map(Contribution::id).collect();
    let mut before = HashSet::new();
    let ordered = contributions.iter().all(|c| {
        let fine = !c.parents().iter().any(|p| all.contains(p) && !before.contains(p));
        before.insert(c.id());
        fine
    });
    if ordered {
        return contributions;
    }
    // Depth-first, parents first, in the file's order.
    let index: HashMap<ContributionId, usize> = contributions.iter().enumerate().map(|(i, c)| (c.id(), i)).collect();
    let mut slots: Vec<Option<Contribution>> = contributions.into_iter().map(Some).collect();
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
/// isn't clean (or isn't there yet: a fresh [`Held`] isn't clean), can't be appended to, or its tail
/// has outgrown, or would outgrow, its first segment. Updates `held` to match.
fn save(dir: &Path, store: &Store, held: &mut Held) -> Result<()> {
    let cache = Cache::open(dir, true)?.context("no cache/ was made")?;
    if !held.clean || held.over() {
        return rewrite(&cache, store, held);
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
    let mut after = held.clone();
    after.tail_bytes += bytes.len() as u64;
    if after.over() {
        return rewrite(&cache, store, held);
    }
    if cache.append(&bytes).is_err() {
        // Gone, a link, a FIFO, or not a regular file: the whole snapshot instead, renamed into place.
        return rewrite(&cache, store, held);
    }
    held.ids.extend(new.iter().map(|c| c.id()));
    held.tail_bytes = after.tail_bytes;
    Ok(())
}

/// Writes the snapshot of `store` whole and atomically, through [`Cache::replace`].
fn rewrite(cache: &Cache, store: &Store, held: &mut Held) -> Result<()> {
    let g = store.graph();
    let order: Vec<&Contribution> =
        g.topo_order().iter().map(|id| g.get(id).expect("the order lists the graph's own ids")).collect();
    let bytes = encode(store.info().ground, &order);
    cache.replace(&bytes).context("writing the snapshot")?;
    *held = Held {
        ids: order.iter().map(|c| c.id()).collect(),
        base_bytes: bytes.len() as u64 - HEADER_BYTES,
        tail_bytes: 0,
        clean: true,
    };
    Ok(())
}

/// `cache/` and the files in it, never through a link.
#[cfg(unix)]
pub(crate) mod cache {
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::fd::OwnedFd;
    use std::path::Path;

    use rustix::fs::{self as fs, AtFlags, CWD, FileType, Mode, OFlags};
    use rustix::io::Errno;

    use super::SNAPSHOT;

    const IGNORE: &str = ".gitignore";

    /// `cache/`, held open: everything below is relative to this descriptor.
    pub struct Cache {
        fd: OwnedFd,
    }

    fn refused(what: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what} isn't a regular file (or is a symlink)"))
    }

    impl Cache {
        /// `<dir>/cache`, opened with `O_DIRECTORY | O_NOFOLLOW` (`dir` itself is followed: it's the replica
        /// the caller named). With `create`, it's made first if missing, with its `.gitignore`. `None` if it
        /// doesn't exist; an error if it is a link or not a directory.
        pub fn open(dir: &Path, create: bool) -> io::Result<Option<Cache>> {
            let root = fs::openat(CWD, dir, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?;
            if create {
                match fs::mkdirat(&root, "cache", Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let cache = match fs::openat(&root, "cache", flags, Mode::empty()) {
                Ok(fd) => Cache { fd },
                Err(Errno::NOENT) if !create => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if create {
                cache.ignore()?;
            }
            Ok(Some(cache))
        }

        /// Opens `name` here with `flags` and `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, so neither a link nor a
        /// FIFO is followed or waited on, and refuses it unless the open descriptor is a regular file.
        fn regular(&self, name: &str, flags: OFlags) -> io::Result<File> {
            let flags = flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = fs::openat(&self.fd, name, flags, Mode::empty()).map_err(|e| match e {
                Errno::LOOP | Errno::NOTDIR => refused(name),
                e => e.into(),
            })?;
            if FileType::from_raw_mode(fs::fstat(&fd)?.st_mode) != FileType::RegularFile {
                return Err(refused(name));
            }
            Ok(File::from(fd))
        }

        /// The snapshot opened to read, and its size from the open descriptor; `None` if there's none.
        pub fn read(&self) -> io::Result<Option<(File, u64)>> {
            match self.regular(SNAPSHOT, OFlags::RDONLY) {
                Ok(f) => {
                    let size = f.metadata()?.len();
                    Ok(Some((f, size)))
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }

        /// Appends `bytes` to the snapshot with one `O_APPEND` write. Fails, writing nothing, if the
        /// snapshot is missing or isn't a regular file.
        pub fn append(&self, bytes: &[u8]) -> io::Result<()> {
            self.regular(SNAPSHOT, OFlags::WRONLY | OFlags::APPEND)?.write_all(bytes)
        }

        /// Writes `bytes` to a new temporary file here (`O_CREAT | O_EXCL | O_NOFOLLOW`), then renames it over
        /// the snapshot, replacing whatever is there (a link or a FIFO included) rather than writing through
        /// it. A failed write removes the temporary file.
        pub fn replace(&self, bytes: &[u8]) -> io::Result<()> {
            let mut nonce = [0u8; 8];
            getrandom::fill(&mut nonce).map_err(|e| io::Error::other(format!("no randomness: {e}")))?;
            let tmp = format!(".snapshot-{}-{}.tmp", std::process::id(), u64::from_le_bytes(nonce));
            let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let fd = fs::openat(&self.fd, tmp.as_str(), flags, Mode::from_raw_mode(0o666))?;
            let written = File::from(fd)
                .write_all(bytes)
                .and_then(|()| fs::renameat(&self.fd, tmp.as_str(), &self.fd, SNAPSHOT).map_err(io::Error::from));
            if written.is_err() {
                let _ = fs::unlinkat(&self.fd, tmp.as_str(), AtFlags::empty());
            }
            written
        }

        /// The filesystem's own clock, as the index's freshness reads it (docs/design/index-store.md
        /// section 5 step 1): `clock` here is rewritten (opened with `O_NOFOLLOW | O_NONBLOCK`, refused
        /// unless it's a regular file) and its mtime in nanoseconds returned, with the device it's on.
        pub fn clock(&self) -> io::Result<(i128, u64)> {
            let flags =
                OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = fs::openat(&self.fd, "clock", flags, Mode::from_raw_mode(0o666)).map_err(|e| match e {
                Errno::LOOP | Errno::NOTDIR => refused("clock"),
                e => e.into(),
            })?;
            let st = fs::fstat(&fd)?;
            if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
                return Err(refused("clock"));
            }
            File::from(fd.try_clone()?).write_all(&std::process::id().to_le_bytes())?;
            let st = fs::fstat(&fd)?;
            Ok((i128::from(st.st_mtime) * 1_000_000_000 + i128::from(st.st_mtime_nsec), st.st_dev as u64))
        }

        /// Creates `.gitignore` (`*`) if nothing is at its name; never writes through a link there.
        fn ignore(&self) -> io::Result<()> {
            match fs::statat(&self.fd, IGNORE, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => return Ok(()),
                Err(Errno::NOENT) => {}
                Err(e) => return Err(e.into()),
            }
            let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let fd = match fs::openat(&self.fd, IGNORE, flags, Mode::from_raw_mode(0o666)) {
                Ok(fd) => fd,
                Err(Errno::EXIST) => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            File::from(fd).write_all(b"# bracken's local cache of its own verification: never committed\n*\n")
        }
    }
}

/// Elsewhere, the same checks by path, each just before the step it guards.
#[cfg(not(unix))]
pub(crate) mod cache {
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    use super::SNAPSHOT;

    pub struct Cache {
        path: PathBuf,
    }

    fn refused(what: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("{what} isn't a regular file (or is a symlink)"))
    }

    impl Cache {
        pub fn open(dir: &Path, create: bool) -> io::Result<Option<Cache>> {
            let path = dir.join("cache");
            if create {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(refused("cache")),
                Err(e) if e.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
                Err(e) => return Err(e),
            }
            let ignore = path.join(".gitignore");
            if create && fs::symlink_metadata(&ignore).is_err() {
                let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&ignore)?;
                f.write_all(b"# bracken's local cache of its own verification: never committed\n*\n")?;
            }
            Ok(Some(Cache { path }))
        }

        fn regular(&self, options: &fs::OpenOptions) -> io::Result<File> {
            let path = self.path.join(SNAPSHOT);
            if !fs::symlink_metadata(&path)?.is_file() {
                return Err(refused(SNAPSHOT));
            }
            let f = options.open(&path)?;
            if !f.metadata()?.is_file() {
                return Err(refused(SNAPSHOT));
            }
            Ok(f)
        }

        pub fn read(&self) -> io::Result<Option<(File, u64)>> {
            match self.regular(fs::OpenOptions::new().read(true)) {
                Ok(f) => {
                    let size = f.metadata()?.len();
                    Ok(Some((f, size)))
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }

        pub fn append(&self, bytes: &[u8]) -> io::Result<()> {
            self.regular(fs::OpenOptions::new().append(true))?.write_all(bytes)
        }

        pub fn replace(&self, bytes: &[u8]) -> io::Result<()> {
            let mut nonce = [0u8; 8];
            getrandom::fill(&mut nonce).map_err(|e| io::Error::other(format!("no randomness: {e}")))?;
            let tmp = self.path.join(format!(".snapshot-{}-{}.tmp", std::process::id(), u64::from_le_bytes(nonce)));
            let written = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .and_then(|mut f| f.write_all(bytes))
                .and_then(|()| fs::rename(&tmp, self.path.join(SNAPSHOT)));
            if written.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            written
        }

        /// No filesystem clock the index trusts here: freshness records no stamps.
        pub fn clock(&self) -> io::Result<(i128, u64)> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no filesystem clock on this platform"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manyroot::{Body, Identity};
    use std::fs;
    use std::path::PathBuf;

    const G: GroundId = GroundId([7; 32]);

    fn snapshot_path(dir: &Path) -> PathBuf {
        dir.join("cache").join(SNAPSHOT)
    }

    fn chain(n: usize) -> Vec<Contribution> {
        let who = Identity::generate("t").expect("a key");
        let mut out: Vec<Contribution> = Vec::new();
        for i in 0..n {
            let parents: Vec<ContributionId> = out.last().map(Contribution::id).into_iter().collect();
            let body = Body::Claim { statement: format!("c{i}"), tags: vec![] };
            out.push(Contribution::create(G, &who, parents, i as u64, body));
        }
        out
    }

    fn refs(cs: &[Contribution]) -> Vec<&Contribution> {
        cs.iter().collect()
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Decoded> {
        decode(bytes, bytes.len() as u64, G, &|_| true)
    }

    fn ids(cs: &[Contribution]) -> Vec<ContributionId> {
        cs.iter().map(Contribution::id).collect()
    }

    #[test]
    fn decode_refuses_what_encode_did_not_write() {
        let cs = chain(1);
        let bytes = encode(G, &refs(&cs));
        assert_eq!(ids(&decode_bytes(&bytes).expect("round trip").contributions), ids(&cs));
        for cut in [0, 1, 40, bytes.len() - 1] {
            assert!(decode_bytes(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut flipped = bytes.clone();
        flipped[60] ^= 1;
        assert!(decode_bytes(&flipped).is_err());
        let mut old = bytes.clone();
        for version in [b"BKSNAP1\0", b"BKSNAP2\0"] {
            old[..8].copy_from_slice(version);
            assert!(decode_bytes(&old).is_err(), "an earlier version isn't read");
        }
        assert!(decode(&bytes[..], bytes.len() as u64, GroundId([8; 32]), &|_| true).is_err(), "another Ground's");
        assert!(decode(&bytes[..], bytes.len() as u64, G, &|_| false).is_err(), "names an object that's gone");
        let empty = decode_bytes(&encode(G, &[])).expect("an empty Ground's snapshot reads");
        assert!(empty.clean && empty.contributions.is_empty());
    }

    #[test]
    fn appended_segments_read_back_and_a_torn_one_ends_the_read() {
        let cs = chain(6);
        let mut bytes = encode(G, &refs(&cs[..2]));
        bytes.extend(segment(&refs(&cs[2..4])));
        let whole = decode_bytes(&bytes).expect("reads");
        assert!(whole.clean);
        assert_eq!(ids(&whole.contributions), ids(&cs[..4]));
        let torn = segment(&refs(&cs[4..]));
        bytes.extend_from_slice(&torn[..torn.len() - 3]);
        let d = decode_bytes(&bytes).expect("the prefix reads");
        assert!(!d.clean, "a torn segment is noticed");
        assert_eq!(d.contributions.len(), 4);
    }

    /// Findings `cfac5d60761e` and `5c7f2f20ac90`: a segment length near `u64::MAX`, first or later,
    /// ends the read as torn, never an overflow.
    #[test]
    fn a_segment_length_near_the_limit_is_torn_not_an_overflow() {
        let cs = chain(2);
        for len in [u64::MAX, u64::MAX - 8, u64::MAX - 20, u64::MAX - 39, u64::MAX - 40, 1 << 62] {
            let mut first = header(G);
            first.extend_from_slice(&len.to_le_bytes());
            first.extend_from_slice(&[0u8; 40]);
            assert!(decode_bytes(&first).is_err(), "first segment of length {len}");
            let mut later = encode(G, &refs(&cs));
            later.extend_from_slice(&len.to_le_bytes());
            later.extend_from_slice(&[0u8; 40]);
            let d = decode_bytes(&later).expect("the first segment reads");
            assert!(!d.clean, "length {len}");
            assert_eq!(d.contributions.len(), 2);
        }
        // An entry length past the segment, and past the largest contribution.
        let mut entry = header(G);
        let body_len = 8u64 + ENTRY_HEAD_BYTES;
        entry.extend_from_slice(&body_len.to_le_bytes());
        entry.extend_from_slice(&1u64.to_le_bytes());
        entry.extend_from_slice(cs[0].id().as_bytes());
        entry.extend_from_slice(&u32::MAX.to_le_bytes());
        entry.extend_from_slice(&[0u8; 64]);
        assert!(decode_bytes(&entry).is_err());
    }

    /// The v3 framing: every byte of an encoding is covered by its own hash (a damaged signature is caught,
    /// though it leaves the id alone), a framed id must be the encoding's, and big segments decode in
    /// parallel batches to exactly what was written.
    #[test]
    fn each_entry_is_checked_against_its_hash_and_id() {
        let who = Identity::generate("t").expect("a key");
        let cs: Vec<Contribution> = (0..12u8)
            .map(|i| {
                let body =
                    Body::Artifact { name: format!("a{i}"), media_type: "x".into(), content: vec![i; 300 << 10] };
                Contribution::create(G, &who, [], u64::from(i), body)
            })
            .collect();
        let bytes = encode(G, &refs(&cs));
        let d = decode_bytes(&bytes).expect("reads, across batches");
        assert!(d.clean);
        assert_eq!(ids(&d.contributions), ids(&cs), "the same contributions, in the same order");
        assert!(d.contributions.iter().zip(&cs).all(|(a, b)| a.encode() == b.encode()));

        // The last byte of the first encoding is in its signature: the id doesn't change, its hash does.
        let first_end = HEADER_BYTES as usize + 16 + ENTRY_HEAD_BYTES as usize + cs[0].encode().len();
        let mut damaged = bytes.clone();
        damaged[first_end - 1] ^= 1;
        assert!(decode_bytes(&damaged).is_err(), "a damaged signature is caught");

        // A framed id that isn't its encoding's: one the replica holds (the membership check passes) and
        // the segment doesn't, so only the decoded id can tell.
        let other = Contribution::create(G, &who, [], 99, Body::Claim { statement: "x".into(), tags: vec![] });
        let mut swapped = bytes.clone();
        let at = HEADER_BYTES as usize + 16;
        swapped[at..at + 32].copy_from_slice(other.id().as_bytes());
        assert!(decode_bytes(&swapped).is_err(), "a framed id must be the encoding's");
    }

    /// Findings `937b52d0164e` and `4dc0418ed33c`: the read stops at the first contribution the replica
    /// holds no object for, reading and checking nothing after it, even within its segment.
    #[test]
    fn the_read_stops_at_the_first_contribution_without_an_object() {
        struct Counted<'a>(&'a [u8], std::rc::Rc<std::cell::Cell<usize>>);
        impl Read for Counted<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buf)?;
                self.1.set(self.1.get() + n);
                Ok(n)
            }
        }
        let cs = chain(1);
        let who = Identity::generate("t").expect("a key");
        let big: Vec<Contribution> = (0..8u8)
            .map(|i| {
                let body = Body::Artifact { name: format!("a{i}"), media_type: "x".into(), content: vec![i; 1 << 20] };
                Contribution::create(G, &who, [], u64::from(i), body)
            })
            .collect();
        // A planted segment: one contribution the replica lacks, then 8 MiB the read must never reach.
        let mut planted: Vec<&Contribution> = vec![&cs[0]];
        planted.extend(big.iter());
        let bytes = encode(G, &planted);
        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let checks = std::cell::Cell::new(0);
        let on_disk = |_: &ContributionId| {
            checks.set(checks.get() + 1);
            false
        };
        let reader = BufReader::with_capacity(4096, Counted(&bytes, read.clone()));
        assert!(decode(reader, bytes.len() as u64, G, &on_disk).is_err(), "set aside");
        assert_eq!(checks.get(), 1, "one membership check, then the read stops");
        assert!(read.get() < 8192, "read {} of {} bytes", read.get(), bytes.len());
    }

    /// Finding `ed533a35b961`: repeats and empty segments end the read, so they can't inflate it or the
    /// first segment's size, and make the snapshot unclean, so the next writer rewrites it.
    #[test]
    fn repeats_and_empty_segments_end_the_read() {
        let cs = chain(5);
        // A first segment that repeats one contribution: only what comes before the repeat is whole.
        let mut repeated = header(G);
        repeated.extend(segment(&[&cs[0], &cs[0], &cs[0]]));
        assert!(decode_bytes(&repeated).is_err(), "no whole first segment");

        // Two appenders: one wrote 4 and 3 (out of order), the other 2, 3 and 4 again, after it.
        let mut bytes = encode(G, &refs(&cs[..2]));
        bytes.extend(segment(&[&cs[4], &cs[3]]));
        bytes.extend(segment(&refs(&cs[2..])));
        let d = decode_bytes(&bytes).expect("reads");
        assert!(!d.clean, "a repeat ends the read");
        assert_eq!(ids(&d.contributions), vec![cs[0].id(), cs[1].id(), cs[3].id(), cs[4].id()], "order restored");

        // Empty segments: the read ends at the first.
        let mut empty = encode(G, &refs(&cs[..2]));
        for _ in 0..1000 {
            empty.extend(segment(&[]));
        }
        let d = decode_bytes(&empty).expect("reads");
        assert!(!d.clean);
        assert_eq!(d.tail_bytes, 0);
        assert_eq!(d.contributions.len(), 2);
    }

    /// The oracle: whatever the snapshot holds after appends, races, outside arrivals and compactions is
    /// exactly the replica a full open reads, and opening through it gives the full open's graph.
    fn agrees_with_a_full_rebuild(dir: &Path, when: &str) -> Decoded {
        let full = Store::open(dir).expect("valid");
        let bytes = fs::read(snapshot_path(dir)).expect("written");
        let d = decode(&bytes[..], bytes.len() as u64, full.info().ground, &|_| true).expect("decodes");
        assert!(d.clean, "{when}: clean");
        assert_eq!(d.ids.len(), d.contributions.len(), "{when}: no repeats");
        let all: HashSet<ContributionId> = full.graph().iter().map(Contribution::id).collect();
        assert_eq!(d.ids, all, "{when}: the snapshot holds exactly the replica");
        let cached = Store::open_trusting(dir, d.contributions.clone()).expect("the snapshot fits");
        assert_eq!(cached.graph().topo_order(), full.graph().topo_order(), "{when}");
        let heads = |s: &Store| s.graph().heads().collect::<HashSet<_>>();
        assert_eq!(heads(&cached), heads(&full), "{when}");
        let through = open(dir, Refresh::No).expect("opens through the snapshot");
        assert_eq!(through.graph().topo_order(), full.graph().topo_order(), "{when}");
        d
    }

    fn write(dir: &Path, who: &Identity, body: Body) {
        let mut s = Refreshing::open(dir, Refresh::Yes).expect("opens");
        s.contribute(who, body).expect("adds");
    }

    fn claim(n: usize) -> Body {
        Body::Claim { statement: format!("claim {n}"), tags: vec!["note".into()] }
    }

    fn ground(tmp: &Path) -> (PathBuf, Identity) {
        let dir = tmp.join("g");
        let store = Store::init(&dir, "t").expect("a Ground");
        let who = store.create_identity("w").expect("a key");
        drop(store);
        (dir, who)
    }

    #[test]
    fn appends_agree_with_a_full_rebuild() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (dir, who) = ground(tmp.path());
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

    fn big(i: u8) -> Body {
        Body::Artifact { name: format!("a{i}"), media_type: "x".into(), content: vec![i; 400 << 10] }
    }

    #[test]
    fn a_tail_larger_than_the_first_segment_is_compacted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (dir, who) = ground(tmp.path());
        write(&dir, &who, claim(0));
        let mut tails = Vec::new();
        for i in 0..6 {
            write(&dir, &who, big(i));
            let d = agrees_with_a_full_rebuild(&dir, &format!("artifact {i}"));
            assert!(d.tail_bytes <= d.base_bytes.max(MIN_COMPACT_BYTES), "artifact {i}: the tail stays bounded");
            tails.push(d.tail_bytes);
        }
        assert!(tails.contains(&0) && tails.iter().any(|t| *t > 0), "both appends and a compaction: {tails:?}");
    }

    /// Finding `ed533a35b961` (c): a read compacts an over-long tail too, not only a write.
    #[test]
    fn a_read_compacts_an_over_long_tail() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (dir, who) = ground(tmp.path());
        let mut s = Store::open(&dir).expect("opens");
        s.contribute(&who, claim(0)).expect("adds");
        for i in 0..4 {
            s.contribute(&who, big(i)).expect("adds");
        }
        let order: Vec<Contribution> =
            s.graph().topo_order().iter().map(|id| s.graph().get(id).expect("listed").clone()).collect();
        // A small first segment and a tail of 1.6 MiB, appended one at a time.
        let mut bytes = encode(s.info().ground, &refs(&order[..1]));
        for c in &order[1..] {
            bytes.extend(segment(&[c]));
        }
        fs::create_dir_all(dir.join("cache")).expect("cache");
        fs::write(snapshot_path(&dir), &bytes).expect("written");
        open(&dir, Refresh::No).expect("reads");
        assert_eq!(fs::read(snapshot_path(&dir)).expect("kept"), bytes, "the view never writes");
        open(&dir, Refresh::Yes).expect("reads");
        let d = agrees_with_a_full_rebuild(&dir, "a read after an over-long tail");
        assert_eq!(d.tail_bytes, 0, "compacted by the read");
    }

    /// Finding `ed533a35b961` (a): a huge file is read only as far as its framing holds, and bounded by
    /// the largest contribution, never whole; the read falls back and the snapshot is rebuilt.
    #[test]
    fn a_huge_sparse_snapshot_is_never_read_whole() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (dir, who) = ground(tmp.path());
        write(&dir, &who, claim(0));
        let ground_id = Store::open(&dir).expect("opens").info().ground;
        let mut bytes = header(ground_id);
        bytes.extend_from_slice(&((4u64 << 30) - 100).to_le_bytes());
        bytes.extend_from_slice(&1_000_000u64.to_le_bytes());
        bytes.extend_from_slice(&(MAX_CONTRIBUTION_BYTES as u32).to_le_bytes());
        let f = fs::File::create(snapshot_path(&dir)).expect("created");
        std::io::Write::write_all(&mut &f, &bytes).expect("written");
        f.set_len(4 << 30).expect("sparse");
        drop(f);
        let started = std::time::Instant::now();
        open(&dir, Refresh::Yes).expect("reads");
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
        agrees_with_a_full_rebuild(&dir, "after a huge snapshot");
        assert!(fs::metadata(snapshot_path(&dir)).expect("rewritten").len() < 4096);
    }

    /// Finding `4c97cc23ad23`: a snapshot or `cache/` swapped for a link, or a FIFO, is never read,
    /// written or waited on.
    #[cfg(unix)]
    mod swaps {
        use super::*;
        use std::os::unix::fs::symlink;
        use std::sync::mpsc;
        use std::time::Duration;

        /// Runs `f` on a thread and fails if it takes more than 30 seconds (a blocked open).
        fn within_30s<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(f());
            });
            rx.recv_timeout(Duration::from_secs(30)).expect("didn't block")
        }

        fn mkfifo(path: &Path) {
            let made = std::process::Command::new("mkfifo").arg(path).status().expect("mkfifo runs");
            assert!(made.success(), "a FIFO at {}", path.display());
        }

        #[test]
        fn a_fifo_snapshot_is_neither_read_nor_waited_on() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let (dir, who) = ground(tmp.path());
            write(&dir, &who, claim(0));
            fs::remove_file(snapshot_path(&dir)).expect("removed");
            mkfifo(&snapshot_path(&dir));
            let d = dir.clone();
            within_30s(move || open(&d, Refresh::Yes).map(|_| ()).expect("reads"));
            agrees_with_a_full_rebuild(&dir, "after a FIFO at the snapshot");

            // A FIFO swapped in between the open and the write's append: replaced, not waited on.
            let mut s = Refreshing::open(&dir, Refresh::Yes).expect("opens");
            s.contribute(&who, claim(1)).expect("adds");
            fs::remove_file(snapshot_path(&dir)).expect("removed");
            mkfifo(&snapshot_path(&dir));
            within_30s(move || drop(s));
            assert!(fs::symlink_metadata(snapshot_path(&dir)).expect("there").is_file());
            agrees_with_a_full_rebuild(&dir, "after a FIFO swapped in before an append");
        }

        #[test]
        fn a_linked_snapshot_is_neither_trusted_nor_written_through() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let (dir, who) = ground(tmp.path());
            write(&dir, &who, claim(0));
            // A valid snapshot outside the replica, linked in: not read (a full open), then replaced.
            let outside = tmp.path().join("outside");
            fs::rename(snapshot_path(&dir), &outside).expect("moved");
            let kept = fs::read(&outside).expect("there");
            symlink(&outside, snapshot_path(&dir)).expect("linked");
            open(&dir, Refresh::Yes).expect("reads");
            assert!(fs::symlink_metadata(snapshot_path(&dir)).expect("there").is_file(), "replaced");
            assert_eq!(fs::read(&outside).expect("there"), kept, "never written through");

            // Linked in between the open and the write's append: replaced, never appended to.
            let mut s = Refreshing::open(&dir, Refresh::Yes).expect("opens");
            s.contribute(&who, claim(1)).expect("adds");
            fs::remove_file(snapshot_path(&dir)).expect("removed");
            symlink(&outside, snapshot_path(&dir)).expect("linked");
            drop(s);
            assert_eq!(fs::read(&outside).expect("there"), kept, "never appended through a link");
            assert!(fs::symlink_metadata(snapshot_path(&dir)).expect("there").is_file());
            agrees_with_a_full_rebuild(&dir, "after a linked snapshot");

            // /dev/zero linked in: not read.
            fs::remove_file(snapshot_path(&dir)).expect("removed");
            symlink("/dev/zero", snapshot_path(&dir)).expect("linked");
            let d = dir.clone();
            within_30s(move || open(&d, Refresh::Yes).map(|_| ()).expect("reads"));
            agrees_with_a_full_rebuild(&dir, "after /dev/zero at the snapshot");
        }

        #[test]
        fn a_linked_cache_is_neither_read_nor_written() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let (dir, who) = ground(tmp.path());
            write(&dir, &who, claim(0));
            // cache/ swapped for a link to a directory holding a snapshot, between the open and the
            // write's append: nothing there is read or written, .gitignore included.
            let mut s = Refreshing::open(&dir, Refresh::Yes).expect("opens");
            s.contribute(&who, claim(1)).expect("adds");
            let victim = tmp.path().join("victim");
            fs::rename(dir.join("cache"), &victim).expect("moved");
            fs::remove_file(victim.join(".gitignore")).expect("removed");
            let kept = fs::read(victim.join(SNAPSHOT)).expect("there");
            // Every file there, with its bytes: the snapshot, and the index and its clock, which the open
            // made before the swap (docs/design/index-store.md).
            let contents = |d: &Path| -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
                fs::read_dir(d)
                    .expect("listed")
                    .map(|e| e.expect("entry"))
                    .map(|e| (e.file_name(), fs::read(e.path()).expect("read")))
                    .collect()
            };
            let was = contents(&victim);
            assert!(was.contains_key(std::ffi::OsStr::new(SNAPSHOT)), "{:?}", was.keys());
            symlink(&victim, dir.join("cache")).expect("linked");
            drop(s);
            assert_eq!(fs::read(victim.join(SNAPSHOT)).expect("there"), kept, "never appended to");
            assert!(!victim.join(".gitignore").exists(), "no .gitignore written through the link");
            assert_eq!(contents(&victim), was, "nothing written there, and no temporary file left");

            // Reads through a linked cache/ are full ones and write nothing there.
            let full = Store::open(&dir).expect("valid");
            let through = open(&dir, Refresh::Yes).expect("reads");
            assert_eq!(through.graph().topo_order(), full.graph().topo_order());
            assert_eq!(fs::read(victim.join(SNAPSHOT)).expect("there"), kept);
            assert!(verify(&dir).is_err(), "verify refuses to write the snapshot through a link");
            assert_eq!(fs::read(victim.join(SNAPSHOT)).expect("there"), kept);
            assert_eq!(contents(&victim), was, "neither the reads nor verify wrote anything there");
        }
    }
}
