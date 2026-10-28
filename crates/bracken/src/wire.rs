//! What every sync transport shares: message framing, a session's bounds, and the loop that
//! feeds one side of a [`Session`] its peer's messages.
//!
//! On a stream, and in an HTTP body, each message is `u32 big-endian length || message`
//! (network-sync.md section 2). A session ends with an error once it passes its wall-clock
//! deadline, or once the peer has sent more bytes than its budget, counting each frame before
//! it's read, with id lists (heads, offer, need) also held to a smaller budget of their own.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::Result;
use manyroot::sync::{Limits, Message, Rejected, Session};
use manyroot::{Contribution, ContributionId, Graph, GroundId, ImportReport};

/// Bytes a frame adds before its message.
pub const FRAME_PREFIX: u64 = 4;

/// A session's bounds on what it receives from its peer.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    /// Wall-clock time from the session's start.
    pub deadline: Duration,
    /// Bytes received, frame prefixes included.
    pub budget: u64,
    /// Bytes of those that may be id lists.
    pub id_budget: u64,
    /// The largest batch this side accepts, and sends.
    pub batch_bytes: u64,
}

impl Default for Bounds {
    /// Ten minutes, 1 GiB, of which 64 MiB of id lists (about 1.9 M ids).
    fn default() -> Self {
        let batch_bytes = Limits::default().batch_bytes;
        Bounds { deadline: Duration::from_secs(600), budget: 1 << 30, id_budget: 64 << 20, batch_bytes }
    }
}

/// Why a session ended early, with the HTTP status that reports it.
#[derive(Debug)]
pub struct Refused {
    pub status: u16,
    pub reason: String,
}

impl Refused {
    pub fn new(status: u16, reason: impl Into<String>) -> Refused {
        Refused { status, reason: reason.into() }
    }

    pub fn protocol(e: impl fmt::Display) -> Refused {
        Refused::new(400, format!("sync protocol: {e}"))
    }

    pub fn deadline() -> Refused {
        Refused::new(408, "the session passed its deadline")
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for Refused {}

/// A replica a session reads from and imports into.
pub trait Replica {
    fn graph(&self) -> &Graph;
    /// Imports a batch, after refusing whatever local policy refuses.
    fn import(&mut self, batch: Vec<Contribution>) -> Result<(ImportReport, Vec<Rejected>)>;
}

/// One side of a session over some transport, with its bounds.
pub struct Side {
    pub session: Session,
    max_message: usize,
    started: Instant,
    bounds: Bounds,
    received: u64,
    id_bytes: u64,
    /// What this side imported, over the whole session.
    pub report: ImportReport,
    /// The peer's results, summed: what it added and refused of what this side sent.
    pub peer: PeerTally,
    /// The heads the peer sent when the session opened.
    pub peer_heads: Vec<ContributionId>,
}

#[derive(Debug, Default)]
pub struct PeerTally {
    pub added: u64,
    pub rejected: Vec<Rejected>,
    pub unresolved: u64,
}

impl Side {
    pub fn new(session: Session, max_message: usize, bounds: Bounds) -> Side {
        Side {
            session,
            max_message,
            started: Instant::now(),
            bounds,
            received: 0,
            id_bytes: 0,
            report: ImportReport::default(),
            peer: PeerTally::default(),
            peer_heads: Vec::new(),
        }
    }

    /// A new session for `ground`, with `bounds.batch_bytes` as its limit.
    pub fn open(ground: GroundId, bounds: Bounds) -> Side {
        let limits = Limits { batch_bytes: bounds.batch_bytes };
        Side::new(Session::new(ground, limits), limits.max_message_bytes(), bounds)
    }

    /// When the session's deadline passes.
    pub fn deadline(&self) -> Instant {
        self.started + self.bounds.deadline
    }

    /// Time left before the deadline, or the deadline refusal.
    pub fn time_left(&self) -> Result<Duration, Refused> {
        self.bounds.deadline.checked_sub(self.started.elapsed()).filter(|d| !d.is_zero()).ok_or_else(Refused::deadline)
    }

    /// Bytes the peer may still send.
    pub fn budget_left(&self) -> u64 {
        self.bounds.budget.saturating_sub(self.received)
    }

    /// Reads one frame, refusing it before reading its message if it's over the message limit or
    /// what's left of the budget. `None` at a clean end of input, before any byte of a frame.
    pub fn read_frame(&mut self, r: &mut impl Read) -> Result<Option<Vec<u8>>> {
        let mut prefix = [0u8; FRAME_PREFIX as usize];
        let mut filled = 0;
        while filled < prefix.len() {
            match r.read(&mut prefix[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => return Err(Refused::protocol("input ended inside a frame").into()),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(self.read_error(e)),
            }
        }
        let len = u32::from_be_bytes(prefix) as usize;
        if len > self.max_message {
            return Err(
                Refused::new(413, format!("message of {len} bytes, over the {}-byte limit", self.max_message)).into()
            );
        }
        if FRAME_PREFIX + len as u64 > self.budget_left() {
            return Err(Refused::new(413, format!("the session's {}-byte budget is spent", self.bounds.budget)).into());
        }
        self.received += FRAME_PREFIX + len as u64;
        let mut message = vec![0u8; len];
        r.read_exact(&mut message).map_err(|e| self.read_error(e))?;
        Ok(Some(message))
    }

    fn read_error(&self, e: io::Error) -> anyhow::Error {
        match e.kind() {
            io::ErrorKind::UnexpectedEof => Refused::protocol("input ended inside a frame").into(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => match self.time_left() {
                Err(deadline) => deadline.into(),
                Ok(_) => Refused::new(408, "the peer stopped sending").into(),
            },
            _ => anyhow::Error::new(e).context("reading from the peer"),
        }
    }

    /// Handles one frame from the peer: decodes it, runs it through the session, imports a batch,
    /// and returns the messages to send back.
    pub fn handle(&mut self, replica: &mut dyn Replica, frame: &[u8]) -> Result<Vec<Message>> {
        self.time_left()?;
        let message = Message::decode(frame, self.max_message).map_err(Refused::protocol)?;
        match &message {
            Message::Heads { .. } | Message::Offer { .. } | Message::Need { .. } => {
                self.id_bytes += frame.len() as u64;
                if self.id_bytes > self.bounds.id_budget {
                    let reason = format!("the session's {}-byte budget for id lists is spent", self.bounds.id_budget);
                    return Err(Refused::new(413, reason).into());
                }
                if let Message::Heads { heads } = &message {
                    self.peer_heads.extend(heads.iter().copied());
                }
            }
            Message::Result { added, rejected, unresolved, .. } => {
                self.peer.added += added;
                self.peer.unresolved += unresolved;
                self.peer.rejected.extend(rejected.iter().cloned());
            }
            _ => {}
        }
        let step = self.session.receive(replica.graph(), message).map_err(Refused::protocol)?;
        let mut send = step.send;
        if let Some(batch) = step.import {
            let (report, refused) = replica.import(batch)?;
            send.extend(self.session.imported_refusing(&report, &refused));
            self.report.added.extend(report.added);
            self.report.already_present += report.already_present;
            self.report.rejected.extend(report.rejected);
            self.report.unresolved.extend(report.unresolved);
            // The import is done and stored either way; a session past its deadline ends here.
            self.time_left()?;
        }
        Ok(send)
    }
}

/// A TCP stream whose reads and writes fail once `until` passes, however slowly the peer trickles
/// bytes: each call waits at most the time left, and at most `each`, if set.
pub struct Timed<'a> {
    stream: &'a TcpStream,
    until: Instant,
    each: Option<Duration>,
}

impl<'a> Timed<'a> {
    pub fn new(stream: &'a TcpStream, until: Instant, each: Option<Duration>) -> Timed<'a> {
        Timed { stream, until, each }
    }

    /// The timeout for the next call, or an error once `until` has passed.
    fn timeout(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(self.each.map_or(left, |each| each.min(left)))
    }
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.timeout()?))?;
        (&mut &*self.stream).read(buf)
    }
}

impl Write for Timed<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.timeout()?))?;
        (&mut &*self.stream).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&mut &*self.stream).flush()
    }
}

/// Writes messages as frames, without flushing.
pub fn write_frames(w: &mut impl Write, messages: &[Message]) -> io::Result<()> {
    for m in messages {
        let bytes = m.encode();
        let len = u32::try_from(bytes.len()).map_err(|_| io::Error::other("message over 4 GiB"))?;
        w.write_all(&len.to_be_bytes())?;
        w.write_all(&bytes)?;
    }
    Ok(())
}

/// Runs one side of a session over a stream to its end: sends `opening`, then answers each frame
/// from `input`, through `handle` (usually [`Side::handle`]), until the session is done.
pub fn run_stream(
    side: &mut Side,
    opening: Vec<Message>,
    input: &mut impl Read,
    output: &mut impl Write,
    mut handle: impl FnMut(&mut Side, &[u8]) -> Result<Vec<Message>>,
) -> Result<()> {
    write_frames(output, &opening)?;
    output.flush()?;
    while !side.session.is_done() {
        let Some(frame) = side.read_frame(input)? else {
            return Err(Refused::protocol("the peer closed the session before it finished").into());
        };
        let reply = handle(side, &frame)?;
        write_frames(output, &reply)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manyroot::sync::SYNC_PROTOCOL;

    /// An empty replica whose imports take `delay`.
    struct Slow {
        graph: Graph,
        delay: Duration,
        imports: usize,
    }

    impl Replica for Slow {
        fn graph(&self) -> &Graph {
            &self.graph
        }

        fn import(&mut self, _: Vec<Contribution>) -> Result<(ImportReport, Vec<Rejected>)> {
            std::thread::sleep(self.delay);
            self.imports += 1;
            Ok((ImportReport::default(), Vec::new()))
        }
    }

    const GROUND: GroundId = GroundId([3; 32]);

    fn side(deadline: Duration) -> Side {
        let mut side = Side::open(GROUND, Bounds { deadline, ..Bounds::default() });
        side.session.start(&Graph::new(GROUND));
        side
    }

    fn hello() -> Vec<u8> {
        Message::Hello { protocol: SYNC_PROTOCOL, ground: GROUND, limits: Limits::default(), features: vec![] }.encode()
    }

    fn heads() -> Vec<u8> {
        Message::Heads { heads: vec![] }.encode()
    }

    fn last_batch() -> Vec<u8> {
        let mut bundle = Vec::new();
        manyroot::bundle::write(&mut bundle, GROUND, []).expect("writing to a Vec cannot fail");
        Message::Batch { bundle, last: true }.encode()
    }

    fn status(e: anyhow::Error) -> u16 {
        e.downcast::<Refused>().expect("a refusal").status
    }

    #[test]
    fn a_message_after_the_deadline_is_refused_before_it_is_handled() {
        let mut replica = Slow { graph: Graph::new(GROUND), delay: Duration::ZERO, imports: 0 };
        let mut side = side(Duration::from_millis(100));
        side.handle(&mut replica, &hello()).expect("in time");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(status(side.handle(&mut replica, &heads()).unwrap_err()), 408);
    }

    #[test]
    fn a_session_whose_import_runs_past_the_deadline_ends_right_after_it() {
        let mut replica = Slow { graph: Graph::new(GROUND), delay: Duration::from_millis(300), imports: 0 };
        let mut side = side(Duration::from_millis(200));
        side.handle(&mut replica, &hello()).expect("in time");
        side.handle(&mut replica, &heads()).expect("in time");
        assert_eq!(status(side.handle(&mut replica, &last_batch()).unwrap_err()), 408);
        assert_eq!(replica.imports, 1, "the import under way finished");

        // The same session with a fast import completes.
        let mut fast = Slow { graph: Graph::new(GROUND), delay: Duration::ZERO, imports: 0 };
        let mut side = self::side(Duration::from_millis(200));
        for m in [hello(), heads(), last_batch()] {
            side.handle(&mut fast, &m).expect("in time");
        }
    }
}
