//! CRDT mode's sync on the wire: framing, the session loop, and the checks
//! every peer message passes before Automerge sees it (decision `89568ebb896f`).
//!
//! Automerge's own `receive_sync_message` must never see unchecked peer bytes
//! (note `7f9a154018f5`): it inflates compressed change chunks and V2 messages'
//! documents without a bound, sizes a bloom filter's probe list from the wire,
//! and divides by a wire-chosen length. So [`check_message`] parses each
//! message itself first, with bounds: V1 only; bounded hash lists; a bloom
//! filter with Automerge's own constants and exact length; and only
//! uncompressed change chunks, which Automerge never inflates, and at most
//! [`MAX_MESSAGE_OPS`] ops. Each change is then decoded and its operations
//! held to a whitelist by [`Rules`]. A refused message is refused whole,
//! before any of it is applied, and the session ends with an error.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use automerge::legacy::{Key, ObjectId, OpType};
use automerge::sync::{self, SyncDoc};
use automerge::{ActorId, AutoCommit, Change, ScalarValue};

/// The largest frame (one sync message) a side reads: a whole document's changes and a little
/// more, since Automerge sends a peer everything it lacks in one message.
pub const MAX_MESSAGE: usize = MAX_DOC_BYTES + 1024 * 1024;
/// Bytes one side of a session may receive.
pub const SESSION_BUDGET: u64 = 32 * 1024 * 1024;
/// How long one session may take.
pub const SESSION_DEADLINE: Duration = Duration::from_secs(60);
/// Frames one side may receive in a session.
pub const MAX_FRAMES: usize = 200;
/// The most hashes in a message's `heads`, `need`, or one `have`'s `last_sync`.
pub const MAX_HASHES: u64 = 1024;
/// The most `have` entries in a message.
pub const MAX_HAVES: u64 = 4;
/// The most entries a bloom filter may summarise: a document's change cap.
pub const MAX_BLOOM_ENTRIES: u32 = MAX_CHANGES as u32;
/// The most changes one message may carry.
pub const MAX_MESSAGE_CHANGES: u64 = 10_000;
/// The largest change chunk.
pub const MAX_CHANGE_BYTES: usize = 2 * 1024 * 1024;
/// The most ops one message may carry. Automerge sends a peer everything it lacks in one
/// message, so this is also about the most a copy can fall behind and still catch up.
pub const MAX_MESSAGE_OPS: usize = 250_000;
/// The most changes a document may hold.
pub const MAX_CHANGES: usize = 20_000;
/// The largest a document may be, saved.
pub const MAX_DOC_BYTES: usize = 16 * 1024 * 1024;

/// Automerge's sync message type byte for V1, the only one accepted.
const MESSAGE_V1: u8 = 0x42;
/// Every storage chunk starts with this, then a 4-byte checksum, then its type.
const CHUNK_MAGIC: [u8; 4] = [0x85, 0x6f, 0x4a, 0x83];
/// The chunk type of an uncompressed change. Documents (0) and compressed changes (2) are refused.
const CHUNK_CHANGE: u8 = 1;
/// Automerge's bloom filter constants (`sync/bloom.rs`).
const BLOOM_BITS_PER_ENTRY: u64 = 10;
const BLOOM_PROBES: u64 = 7;
/// The longest change message an honest side writes (it writes none).
const MAX_CHANGE_MESSAGE: usize = 256;

/// A session's bounds on what it receives.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub deadline: Duration,
    /// Bytes received, frame prefixes included.
    pub budget: u64,
    pub frames: usize,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds { deadline: SESSION_DEADLINE, budget: SESSION_BUDGET, frames: MAX_FRAMES }
    }
}

/// Who may change what, as one side sees the other.
pub struct Rules {
    /// Actors whose changes this side accepts, each with the text objects they may change
    /// (as `counter@actor`).
    pub allowed: Vec<(ActorId, BTreeSet<String>)>,
    /// What any other actor may change, if any other actor is accepted at all.
    pub others: Option<BTreeSet<String>>,
}

/// A cursor over a message's bytes. Every read is bounded by what's left.
struct Input<'a> {
    bytes: &'a [u8],
}

impl<'a> Input<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let (&b, rest) = self.bytes.split_first().ok_or("the message ends early")?;
        self.bytes = rest;
        Ok(b)
    }

    fn take(&mut self, n: u64) -> Result<&'a [u8], String> {
        let n = usize::try_from(n).ok().filter(|&n| n <= self.bytes.len()).ok_or("the message ends early")?;
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(taken)
    }

    /// An unsigned LEB128 of at most 10 bytes.
    fn uleb(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for i in 0..10 {
            let b = self.byte()?;
            let bits = u64::from(b & 0x7f);
            if i == 9 && bits > 1 {
                return Err("a length overflows".into());
            }
            value |= bits << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("a length is too long".into())
    }

    /// A count of at most `max`.
    fn count(&mut self, what: &str, max: u64) -> Result<u64, String> {
        let n = self.uleb()?;
        if n > max { Err(format!("{n} {what}, over the limit of {max}")) } else { Ok(n) }
    }

    fn hashes(&mut self, what: &str) -> Result<(), String> {
        let n = self.count(what, MAX_HASHES)?;
        self.take(n * 32).map(drop)
    }
}

/// The changes in a message, after its framing passed every bound.
pub struct Checked {
    pub changes: Vec<Change>,
}

/// Parses a V1 sync message as Automerge would, refusing anything outside the bounds above, and
/// decodes its changes. Nothing here inflates, allocates from a wire count, or panics.
pub fn check_message(bytes: &[u8]) -> Result<Checked, String> {
    if bytes.len() > MAX_MESSAGE {
        return Err(format!("a sync message of {} bytes, over the limit of {MAX_MESSAGE}", bytes.len()));
    }
    let mut i = Input { bytes };
    if i.byte()? != MESSAGE_V1 {
        return Err("only V1 sync messages are accepted".into());
    }
    i.hashes("heads")?;
    i.hashes("needed hashes")?;
    for _ in 0..i.count("have entries", MAX_HAVES)? {
        i.hashes("last-sync hashes")?;
        let len = i.uleb()?;
        let bloom = i.take(len)?;
        check_bloom(bloom).map_err(|e| format!("a bloom filter: {e}"))?;
    }
    let mut changes = Vec::new();
    let mut ops = 0;
    for _ in 0..i.count("changes", MAX_MESSAGE_CHANGES)? {
        let len = i.uleb()?;
        if len > MAX_CHANGE_BYTES as u64 {
            return Err(format!("a change of {len} bytes, over the limit of {MAX_CHANGE_BYTES}"));
        }
        let change = check_chunk(i.take(len)?)?;
        ops += change.len();
        if ops > MAX_MESSAGE_OPS {
            return Err(format!("a message of over {MAX_MESSAGE_OPS} ops"));
        }
        changes.push(change);
    }
    if !i.bytes.is_empty() {
        // The optional flags: one length-prefixed byte string, then nothing.
        let len = i.count("flag bytes", 8)?;
        i.take(len)?;
        if !i.bytes.is_empty() {
            return Err("bytes after the message".into());
        }
    }
    Ok(Checked { changes })
}

/// A bloom filter is empty, or uses Automerge's constants with exactly the bits they imply.
fn check_bloom(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut i = Input { bytes };
    let entries = i.uleb()?;
    let (bits, probes) = (i.uleb()?, i.uleb()?);
    if entries > u64::from(MAX_BLOOM_ENTRIES) {
        return Err(format!("{entries} entries, over the limit of {MAX_BLOOM_ENTRIES}"));
    }
    if bits != BLOOM_BITS_PER_ENTRY || probes != BLOOM_PROBES {
        return Err("parameters other than Automerge's own".into());
    }
    if entries == 0 || i.bytes.len() as u64 != (entries * bits).div_ceil(8) {
        return Err("the wrong length".into());
    }
    Ok(())
}

/// One chunk: an uncompressed change, decoded.
fn check_chunk(chunk: &[u8]) -> Result<Change, String> {
    if chunk.len() < 9 || chunk[..4] != CHUNK_MAGIC {
        return Err("a change that isn't an Automerge chunk".into());
    }
    if chunk[8] != CHUNK_CHANGE {
        return Err("only uncompressed change chunks are accepted".into());
    }
    let mut i = Input { bytes: &chunk[9..] };
    let len = i.uleb()?;
    if len != i.bytes.len() as u64 {
        return Err("a change chunk of the wrong length".into());
    }
    guard(|| Change::from_bytes(chunk.to_vec()).map_err(|e| format!("a malformed change: {e}")))?
}

/// Runs `f`, turning a panic inside Automerge into an error, so it ends one sync and not the worker.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| "Automerge failed on this input".to_string())
}

/// Checks the changes `doc` doesn't hold yet against `rules`: an allowed actor, and only
/// character inserts and deletes on that actor's text objects. Also holds the document to its
/// caps. Deps are checked at the end of the session ([`session`]), not here: Automerge's bloom
/// filter has false positives, so an honest sender sometimes leaves out a change the receiver
/// lacks, sends its children, and sends the change itself only when the receiver asks for it.
pub fn check_changes(doc: &mut AutoCommit, checked: &Checked, rules: &Rules) -> Result<usize, String> {
    let mut new = 0;
    for change in &checked.changes {
        if guard(|| doc.get_change_by_hash(&change.hash()).is_some())? {
            continue;
        }
        new += 1;
        let objects = rules
            .allowed
            .iter()
            .find(|(actor, _)| actor == change.actor_id())
            .map(|(_, objects)| objects)
            .or(rules.others.as_ref())
            .ok_or("a change by an actor this side doesn't accept")?;
        let expanded = guard(|| change.decode())?;
        if expanded.message.as_ref().is_some_and(|m| m.len() > MAX_CHANGE_MESSAGE) || !expanded.extra_bytes.is_empty() {
            return Err("a change with a long message or extra bytes".into());
        }
        for op in &expanded.operations {
            let ObjectId::Id(obj) = &op.obj else { return Err("a change to the document's root".into()) };
            if !objects.contains(&format!("{}@{}", obj.0, obj.1)) {
                return Err("a change to an object this actor may not change".into());
            }
            let ok = match (&op.action, op.insert, &op.key) {
                (OpType::Put(ScalarValue::Str(s)), true, Key::Seq(_)) => s.chars().count() == 1,
                (OpType::Delete, false, Key::Seq(_)) => true,
                _ => false,
            };
            if !ok {
                return Err("a change that isn't a text insert or delete".into());
            }
        }
    }
    if guard(|| doc.get_changes(&[]).len())? + new > MAX_CHANGES {
        return Err(format!("the document would hold over {MAX_CHANGES} changes"));
    }
    let size: usize = checked.changes.iter().map(|c| c.raw_bytes().len()).sum();
    if save(doc)?.len() + size > MAX_DOC_BYTES {
        return Err(format!("the document would be over {MAX_DOC_BYTES} bytes"));
    }
    Ok(new)
}

/// The document's bytes, without changes whose deps it doesn't hold (Automerge keeps those by
/// default).
pub fn save(doc: &mut AutoCommit) -> Result<Vec<u8>, String> {
    guard(|| doc.save_with_options(automerge::SaveOptions { deflate: true, retain_orphans: false }))
}

/// What one side of a session saw.
#[derive(Debug, Default)]
pub struct Tally {
    pub frames: usize,
    pub received: u64,
    pub new_changes: usize,
}

/// Runs one side of a sync session on `stream` to its end. Each side sends in turn one frame (a
/// u32 big-endian length, then a V1 message, or length 0 for nothing to send) until both have sent
/// 0 in a row. `first` sends first. Every received message is checked whole before it's applied.
///
/// A session succeeds only if it ends normally with every change's deps held (decision
/// `285ce6a0cad9`). Otherwise, however it ended (an error, the deadline, a cap, the peer gone, a
/// caught panic), the result is an error, and `doc` may be half-applied or still hold changes
/// without their deps: the caller must discard it, never save it, and reload its last save
/// (decision `3a3b3da7c15b`).
pub fn session(
    doc: &mut AutoCommit,
    stream: &mut UnixStream,
    rules: &Rules,
    first: bool,
    bounds: Bounds,
) -> Result<Tally, String> {
    let tally = guard(|| exchange(doc, stream, rules, first, bounds))??;
    if !guard(|| doc.get_missing_deps(&[]).is_empty())? {
        return Err("the peer sent changes whose deps never arrived".into());
    }
    Ok(tally)
}

fn exchange(
    doc: &mut AutoCommit,
    stream: &mut UnixStream,
    rules: &Rules,
    first: bool,
    bounds: Bounds,
) -> Result<Tally, String> {
    let deadline = Instant::now() + bounds.deadline;
    let mut state = sync::State::new();
    let mut tally = Tally::default();
    let mut sent_empty = false;
    if first {
        sent_empty = send(doc, &mut state, stream, deadline)?;
    }
    loop {
        let frame = read_frame(stream, deadline, bounds, &mut tally)?;
        if frame.is_empty() && sent_empty {
            return Ok(tally);
        }
        if !frame.is_empty() {
            tally.new_changes += receive(doc, &mut state, &frame, rules)?;
        }
        let none = send(doc, &mut state, stream, deadline)?;
        if frame.is_empty() && none {
            return Ok(tally);
        }
        sent_empty = none;
    }
}

/// Checks one peer message whole, then applies it; the number of changes new to `doc`. A refused
/// message changes nothing.
pub fn receive(doc: &mut AutoCommit, state: &mut sync::State, frame: &[u8], rules: &Rules) -> Result<usize, String> {
    let checked = check_message(frame)?;
    let new = check_changes(doc, &checked, rules)?;
    let message = guard(|| sync::Message::decode(frame))?.map_err(|e| format!("a malformed sync message: {e}"))?;
    guard(|| doc.sync().receive_sync_message(state, message))?.map_err(|e| format!("applying a sync message: {e}"))?;
    // Never offer the peer V2 messages, which carry whole documents.
    state.their_capabilities = None;
    Ok(new)
}

/// Sends this side's next message, or an empty frame; whether it was empty.
fn send(
    doc: &mut AutoCommit,
    state: &mut sync::State,
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<bool, String> {
    let message = guard(|| doc.sync().generate_sync_message(state))?;
    let bytes = message.map(sync::Message::encode).unwrap_or_default();
    if bytes.len() > MAX_MESSAGE {
        return Err("this side's sync message is over the limit".into());
    }
    stream.set_write_timeout(Some(left(deadline)?)).map_err(|e| e.to_string())?;
    let len = u32::try_from(bytes.len()).map_err(|_| "a message over 4 GiB")?;
    stream
        .write_all(&len.to_be_bytes())
        .and_then(|()| stream.write_all(&bytes))
        .and_then(|()| stream.flush())
        .map_err(|e| format!("sending a sync message: {e}"))?;
    Ok(bytes.is_empty())
}

fn left(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() { Err("the sync session passed its deadline".into()) } else { Ok(left) }
}

/// Reads one frame, refusing it before reading its body if it's over the message limit or the
/// session's budget, or past the frame count or deadline.
fn read_frame(
    stream: &mut UnixStream,
    deadline: Instant,
    bounds: Bounds,
    tally: &mut Tally,
) -> Result<Vec<u8>, String> {
    tally.frames += 1;
    if tally.frames > bounds.frames {
        return Err(format!("more than {} sync messages in one session", bounds.frames));
    }
    let mut prefix = [0u8; 4];
    read_exact(stream, &mut prefix, deadline)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len > MAX_MESSAGE {
        return Err(format!("a sync message of {len} bytes, over the limit of {MAX_MESSAGE}"));
    }
    tally.received += 4 + len as u64;
    if tally.received > bounds.budget {
        return Err(format!("the sync session's {}-byte budget is spent", bounds.budget));
    }
    let mut body = vec![0u8; len];
    read_exact(stream, &mut body, deadline)?;
    Ok(body)
}

/// Reads exactly `buf.len()` bytes, every read bounded by what's left of the deadline.
fn read_exact(stream: &mut UnixStream, buf: &mut [u8], deadline: Instant) -> Result<(), String> {
    let mut filled = 0;
    while filled < buf.len() {
        stream.set_read_timeout(Some(left(deadline)?)).map_err(|e| e.to_string())?;
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return Err("the peer closed the sync session early".into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err("the sync session passed its deadline".into());
            }
            Err(e) => return Err(format!("reading a sync message: {e}")),
        }
    }
    Ok(())
}
