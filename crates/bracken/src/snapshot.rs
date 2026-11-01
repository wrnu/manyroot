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
//! rewrites it. A command that writes rewrites the snapshot as it ends ([`Refreshing`]), so a read
//! writes it only when objects arrived another way, such as a git pull. `bracken view` never writes it.
//!
//! `BRACKEN_FULL_CHECK=1` makes every read a full `Store::open` that neither reads nor writes the
//! snapshot, for the merge gate and the Ground's guards; `bracken verify` re-reads and checks everything
//! once, then rewrites the snapshot.

use std::fs;
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use manyroot::{Contribution, GroundId, Store};

const MAGIC: &[u8; 8] = b"BKSNAP1\0";

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

/// Opens the replica in `dir`, through its snapshot unless `BRACKEN_FULL_CHECK` asks for a full check.
pub fn open(dir: &Path, refresh: Refresh) -> Result<Store> {
    if full_check() {
        return Ok(Store::open(dir)?);
    }
    let path = snapshot_path(dir);
    let cached = fs::read(&path).ok().and_then(|bytes| decode(&bytes).ok());
    if let Some((_, trusted)) = cached {
        let n = trusted.len();
        if let Ok(store) = Store::open_trusting(dir, trusted) {
            if refresh == Refresh::Yes && store.graph().len() != n {
                // Best effort: a cache that can't be written is only slower next time.
                let _ = write(dir, &store);
            }
            return Ok(store);
        }
    }
    let store = Store::open(dir)?;
    if refresh == Refresh::Yes {
        let _ = write(dir, &store);
    }
    Ok(store)
}

/// A store opened through the snapshot that rewrites it when dropped, if the store gained contributions
/// since: a write leaves the snapshot current, so the reads after it find nothing new and write nothing.
/// Only objects that arrive another way (a git pull, another tool) make a read write the snapshot.
pub struct Refreshing {
    store: Option<Store>,
    known: usize,
    refresh: Refresh,
}

impl Refreshing {
    pub fn open(dir: &Path, refresh: Refresh) -> Result<Refreshing> {
        let store = open(dir, refresh)?;
        Ok(Refreshing { known: store.graph().len(), store: Some(store), refresh })
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
        if let Some(store) = &self.store
            && self.refresh == Refresh::Yes
            && store.graph().len() != self.known
            && !full_check()
        {
            let _ = write(store.dir(), store);
        }
    }
}

/// `bracken verify`: a full `Store::open`, then the snapshot rewritten from it.
pub fn verify(dir: &Path) -> Result<Store> {
    let store = Store::open(dir)?;
    if !full_check() {
        write(dir, &store).context("writing the snapshot")?;
    }
    Ok(store)
}

pub fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("cache").join("snapshot")
}

fn checksum(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

fn encode(ground: GroundId, contributions: &[&Contribution]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(ground.as_bytes());
    out.extend_from_slice(&u64::try_from(contributions.len()).expect("fits").to_le_bytes());
    for c in contributions {
        let bytes = c.encode();
        out.extend_from_slice(&u32::try_from(bytes.len()).expect("a contribution is at most 16 MiB").to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    let sum = checksum(&out);
    out.extend_from_slice(&sum);
    out
}

/// The snapshot's Ground and contributions, in the causal order they were written in. Refuses anything
/// that isn't exactly what [`encode`] wrote.
fn decode(bytes: &[u8]) -> Result<(GroundId, Vec<Contribution>)> {
    ensure!(bytes.len() >= MAGIC.len() + 32 + 8 + 32, "too short");
    let (body, sum) = bytes.split_at(bytes.len() - 32);
    ensure!(checksum(body) == sum, "the checksum doesn't match");
    ensure!(&body[..8] == MAGIC, "not a snapshot of this version");
    let mut at = 8;
    let mut take = |n: usize| -> Result<&[u8]> {
        let Some(slice) = body.get(at..at + n) else { bail!("truncated") };
        at += n;
        Ok(slice)
    };
    let ground = GroundId(take(32)?.try_into().expect("32 bytes"));
    let count = u64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
    let mut out = Vec::new();
    for _ in 0..count {
        let len = u32::from_le_bytes(take(4)?.try_into().expect("4 bytes")) as usize;
        out.push(Contribution::decode(take(len)?)?);
    }
    ensure!(at == body.len(), "trailing bytes");
    Ok((ground, out))
}

/// Writes the snapshot of `store` atomically: to a temporary file of its own in `cache/`, created fresh
/// (`create_new`, which never follows a link), then renamed over `cache/snapshot`, which replaces a link
/// there rather than writing through it. Refuses a `cache` that isn't a real directory.
fn write(dir: &Path, store: &Store) -> Result<()> {
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
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_refuses_what_encode_did_not_write() {
        let ground = GroundId([7; 32]);
        let bytes = encode(ground, &[]);
        assert_eq!(decode(&bytes).expect("round trip").0, ground);
        for cut in [0, 1, 40, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut flipped = bytes.clone();
        flipped[10] ^= 1;
        assert!(decode(&flipped).is_err());
    }
}
