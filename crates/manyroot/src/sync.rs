//! Weave: exchanging exactly the missing contributions between two replicas.
//!
//! A [`Session`] is one side of a sync. It does no I/O: the caller feeds it the
//! peer's [`Message`]s and the local [`Graph`], sends the messages it returns,
//! and imports the contributions it hands back. Both sides run the same
//! machine. The wire format is specified in `docs/protocol.md`, section "Sync".
//!
//! Because a replica only holds a contribution once it holds its parents, a
//! replica's set is closed under ancestry and its causal heads describe it.
//! Each side sends `Hello` and `Heads`. A side that holds all the peer's heads
//! knows exactly what the peer lacks and sends it; otherwise it offers the ids
//! the peer may lack, and the peer answers with the ones it needs.

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::bundle;
use crate::contribution::Contribution;
use crate::graph::{Graph, ImportReport};
use crate::id::{ContributionId, GroundId};

/// Sync protocol version sent in `Hello`.
pub const SYNC_PROTOCOL: u32 = 1;

/// The most ids one `Heads`, `Offer` or `Need` message carries. A longer list is split across
/// messages of the same type: every one but the last holds exactly this many, and the last holds
/// fewer, possibly none, which ends the list.
pub const MAX_IDS: usize = 65_536;

/// Bytes a bundle adds around its contributions: the magic and the Ground id.
const BUNDLE_HEADER: usize = 8 + 32;
/// Bytes each contribution adds to a bundle: its length prefix.
const BUNDLE_ENTRY: usize = 4;
/// The most entries a `Result` lists in `rejected`, and the longest `reason` it gives, in bytes,
/// so a `Result` always fits the peer's message limit, whatever either side's `batch_bytes`.
pub const MAX_REJECTED: usize = 1024;
pub const MAX_REASON_BYTES: usize = 256;

/// Room a message needs beyond its batch or id list, for the envelope.
const MESSAGE_OVERHEAD: usize = 64;
/// Bytes an id takes in an id list: a byte-string header and 32 bytes.
const ID_BYTES: usize = 2 + 32;

/// What a replica will accept from its peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The largest bundle a `Batch` may carry.
    pub batch_bytes: u64,
}

impl Default for Limits {
    /// 64 MiB, room for several contributions of the maximum size.
    fn default() -> Self {
        Limits { batch_bytes: 64 * 1024 * 1024 }
    }
}

/// A contribution the receiver refused, and why. The reason is advisory text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub id: ContributionId,
    pub reason: String,
}

/// One sync message. On the wire, a canonical CBOR map whose `type` comes first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        protocol: u32,
        ground: GroundId,
        limits: Limits,
        features: Vec<String>,
    },
    /// The sender's causal heads.
    Heads {
        heads: Vec<ContributionId>,
    },
    /// Contributions the sender holds and the receiver may lack, in causal order.
    Offer {
        ids: Vec<ContributionId>,
    },
    /// The offered contributions the sender lacks.
    Need {
        ids: Vec<ContributionId>,
    },
    /// Contributions in bundle format. Each side's last batch says so.
    Batch {
        #[serde(with = "serde_bytes")]
        bundle: Vec<u8>,
        last: bool,
    },
    /// The receiver's import report for the batch before it.
    Result {
        added: u64,
        already_present: u64,
        rejected: Vec<Rejected>,
        unresolved: u64,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("message is {0} bytes, over the {1}-byte limit")]
    MessageTooLarge(usize, usize),
    #[error("malformed message: {0}")]
    Malformed(String),
    #[error("message is not canonically encoded")]
    NotCanonical,
    #[error("peer speaks sync protocol {0}; this replica speaks {SYNC_PROTOCOL}")]
    UnsupportedProtocol(u32),
    #[error("peer is in ground {peer}, this replica is in {ours}")]
    ForeignGround { ours: GroundId, peer: GroundId },
    #[error("peer's batch limit of {0} bytes is too small to be useful")]
    LimitTooSmall(u64),
    #[error("batch of {0} bytes is over this replica's {1}-byte limit")]
    BatchTooLarge(usize, u64),
    #[error("contribution {0} is too large for the peer's batch limit")]
    TooLargeForPeer(ContributionId),
    #[error("bad batch: {0}")]
    BadBatch(String),
    #[error("peer sent {0}, which this replica didn't need")]
    Unrequested(ContributionId),
    #[error("peer asked for {0}, which this replica didn't offer")]
    NotOffered(ContributionId),
    #[error("peer's {0} lists {1} twice")]
    Repeated(&'static str, ContributionId),
    #[error("{0} message with {1} ids, over the {MAX_IDS}-id limit")]
    TooManyIds(&'static str, usize),
    #[error("batch that isn't the last carries no contributions")]
    EmptyBatch,
    #[error("peer's last batch came before {0}, which this replica needed")]
    MissingNeeded(ContributionId),
    #[error("unexpected {0} message")]
    Unexpected(&'static str),
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes).expect("writing to a Vec cannot fail");
        bytes
    }

    /// Decodes a message of at most `max_bytes`, refusing anything that doesn't
    /// re-encode to the same bytes.
    pub fn decode(bytes: &[u8], max_bytes: usize) -> Result<Message, SyncError> {
        if bytes.len() > max_bytes {
            return Err(SyncError::MessageTooLarge(bytes.len(), max_bytes));
        }
        let message: Message = ciborium::from_reader(bytes).map_err(|e| SyncError::Malformed(e.to_string()))?;
        if message.encode() != bytes {
            return Err(SyncError::NotCanonical);
        }
        Ok(message)
    }

    fn name(&self) -> &'static str {
        match self {
            Message::Hello { .. } => "hello",
            Message::Heads { .. } => "heads",
            Message::Offer { .. } => "offer",
            Message::Need { .. } => "need",
            Message::Batch { .. } => "batch",
            Message::Result { .. } => "result",
        }
    }
}

impl Limits {
    /// The largest message this replica will decode from its peer: a batch within `batch_bytes`,
    /// or an id list of [`MAX_IDS`] ids, whichever is larger.
    pub fn max_message_bytes(&self) -> usize {
        let batch = usize::try_from(self.batch_bytes).unwrap_or(usize::MAX);
        batch.max(MAX_IDS * ID_BYTES).saturating_add(MESSAGE_OVERHEAD)
    }
}

/// What a session asks the caller to do after a message.
#[derive(Debug, Default)]
pub struct Step {
    /// Messages to send to the peer, in order.
    pub send: Vec<Message>,
    /// After a batch: contributions to import, parents first. Report the outcome
    /// with [`Session::imported`], even for an empty batch, before the next message.
    pub import: Option<Vec<Contribution>>,
}

/// Counts for one direction of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Contributions carried in batches.
    pub contributions: usize,
    pub batches: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inbound {
    /// Waiting for the peer's Hello, then its Heads (one or more messages).
    Hello,
    Heads,
    /// The peer's Offer or its first Batch.
    Open,
    /// The rest of the peer's Offer, after a full first message.
    Offer,
    /// The peer's batches, holding only `needed` if we sent a Need.
    Batches,
    /// The peer's last batch has arrived (and, while `awaiting_import`, isn't imported yet).
    Done,
}

#[derive(Debug, PartialEq, Eq)]
enum Outbound {
    /// Waiting for the peer's Heads to know what to send.
    Heads,
    /// We offered `offered` and wait for the peer's Need (one or more messages).
    Offered,
    /// Sending `queue`; `awaiting_result` while a batch is unanswered.
    Sending,
    Done,
}

/// One side of a sync session.
#[derive(Debug)]
pub struct Session {
    ground: GroundId,
    limits: Limits,
    peer_limits: Option<Limits>,
    inbound: Inbound,
    outbound: Outbound,
    offered: HashSet<ContributionId>,
    /// The peer's id list being received in parts: its heads, then the offered ids we lack.
    peer_ids: Vec<ContributionId>,
    /// Every id the peer has offered so far.
    peer_offer: HashSet<ContributionId>,
    /// The peer's Need so far, in order and as a set.
    peer_need: Vec<ContributionId>,
    peer_need_set: HashSet<ContributionId>,
    queue: VecDeque<ContributionId>,
    awaiting_result: bool,
    sent_last: bool,
    /// After our Need: the needed ids that haven't arrived yet.
    needed: Option<HashSet<ContributionId>>,
    /// The heads this side sent, and, once a shortcut batch arrives, everything under them:
    /// what the peer can't have meant to send.
    own_heads: Vec<ContributionId>,
    held_at_start: Option<HashSet<ContributionId>>,
    /// Under the shortcut: what the peer has sent so far, which it mustn't send again.
    received_ids: HashSet<ContributionId>,
    awaiting_import: bool,
    sent: Tally,
    received: Tally,
}

impl Session {
    pub fn new(ground: GroundId, limits: Limits) -> Session {
        Session {
            ground,
            limits,
            peer_limits: None,
            inbound: Inbound::Hello,
            outbound: Outbound::Heads,
            offered: HashSet::new(),
            peer_ids: Vec::new(),
            peer_offer: HashSet::new(),
            peer_need: Vec::new(),
            peer_need_set: HashSet::new(),
            queue: VecDeque::new(),
            awaiting_result: false,
            sent_last: false,
            needed: None,
            own_heads: Vec::new(),
            held_at_start: None,
            received_ids: HashSet::new(),
            awaiting_import: false,
            sent: Tally::default(),
            received: Tally::default(),
        }
    }

    /// The opening messages: Hello, then this replica's heads.
    pub fn start(&mut self, graph: &Graph) -> Vec<Message> {
        let mut heads: Vec<ContributionId> = graph.heads().collect();
        heads.sort();
        self.own_heads = heads.clone();
        let mut send = vec![Message::Hello {
            protocol: SYNC_PROTOCOL,
            ground: self.ground,
            limits: self.limits,
            features: vec![],
        }];
        send.extend(split(&heads, MAX_IDS, |heads| Message::Heads { heads }));
        send
    }

    /// Both directions are finished: each side's last batch was sent and answered.
    pub fn is_done(&self) -> bool {
        self.inbound == Inbound::Done && self.outbound == Outbound::Done && !self.awaiting_import
    }

    /// What this side sent and received.
    pub fn sent(&self) -> Tally {
        self.sent
    }

    pub fn received(&self) -> Tally {
        self.received
    }

    /// Handles one message from the peer. `graph` is this replica's graph as it is now.
    pub fn receive(&mut self, graph: &Graph, message: Message) -> Result<Step, SyncError> {
        if self.awaiting_import {
            return Err(SyncError::Unexpected("message before the last batch was imported"));
        }
        let unexpected = SyncError::Unexpected(message.name());
        if let Message::Heads { heads: ids } | Message::Offer { ids } | Message::Need { ids } = &message
            && ids.len() > MAX_IDS
        {
            return Err(SyncError::TooManyIds(message.name(), ids.len()));
        }
        match (message, self.inbound) {
            (Message::Hello { protocol, ground, limits, .. }, Inbound::Hello) => {
                if protocol != SYNC_PROTOCOL {
                    return Err(SyncError::UnsupportedProtocol(protocol));
                }
                if ground != self.ground {
                    return Err(SyncError::ForeignGround { ours: self.ground, peer: ground });
                }
                if limits.batch_bytes < (BUNDLE_HEADER + BUNDLE_ENTRY) as u64 {
                    return Err(SyncError::LimitTooSmall(limits.batch_bytes));
                }
                self.peer_limits = Some(limits);
                self.inbound = Inbound::Heads;
                Ok(Step::default())
            }
            (Message::Heads { heads }, Inbound::Heads) => {
                let complete = heads.len() < MAX_IDS;
                self.peer_ids.extend(heads);
                if !complete {
                    return Ok(Step::default());
                }
                self.inbound = Inbound::Open;
                let heads = std::mem::take(&mut self.peer_ids);
                self.plan(graph, &heads)
            }
            (Message::Offer { ids }, Inbound::Open | Inbound::Offer) => {
                let complete = ids.len() < MAX_IDS;
                for id in ids {
                    if !self.peer_offer.insert(id) {
                        return Err(SyncError::Repeated("offer", id));
                    }
                    if !graph.contains(&id) {
                        self.peer_ids.push(id);
                    }
                }
                if !complete {
                    self.inbound = Inbound::Offer;
                    return Ok(Step::default());
                }
                let needed = std::mem::take(&mut self.peer_ids);
                self.needed = Some(needed.iter().copied().collect());
                self.inbound = Inbound::Batches;
                Ok(Step { send: split(&needed, MAX_IDS, |ids| Message::Need { ids }), import: None })
            }
            (Message::Batch { bundle, last }, Inbound::Open | Inbound::Batches) => self.batch(graph, &bundle, last),
            (Message::Need { ids }, _) if self.outbound == Outbound::Offered => {
                let complete = ids.len() < MAX_IDS;
                for id in ids {
                    if !self.offered.contains(&id) {
                        return Err(SyncError::NotOffered(id));
                    }
                    if !self.peer_need_set.insert(id) {
                        return Err(SyncError::Repeated("need", id));
                    }
                    self.peer_need.push(id);
                }
                if !complete {
                    return Ok(Step::default());
                }
                self.queue = std::mem::take(&mut self.peer_need).into();
                self.outbound = Outbound::Sending;
                self.next_batch(graph)
            }
            (Message::Result { .. }, _) if self.awaiting_result => {
                self.awaiting_result = false;
                if self.sent_last {
                    self.outbound = Outbound::Done;
                    return Ok(Step::default());
                }
                self.next_batch(graph)
            }
            _ => Err(unexpected),
        }
    }

    /// The outcome of importing the contributions from the last batch. Returns the Result to send.
    pub fn imported(&mut self, report: &ImportReport) -> Vec<Message> {
        self.imported_refusing(report, &[])
    }

    /// Like [`Session::imported`], for a receiver that refused some of the batch's contributions
    /// before importing the rest, by local policy (an allowlist, a quota). The refusals are listed
    /// first in `rejected`.
    pub fn imported_refusing(&mut self, report: &ImportReport, refused: &[Rejected]) -> Vec<Message> {
        self.awaiting_import = false;
        let rejected = refused
            .iter()
            .map(|r| (r.id, r.reason.clone()))
            .chain(report.rejected.iter().map(|(id, why)| (*id, why.to_string())))
            .take(MAX_REJECTED)
            .map(|(id, why)| Rejected { id, reason: truncate(why, MAX_REASON_BYTES) })
            .collect();
        vec![Message::Result {
            added: report.added.len() as u64,
            already_present: report.already_present as u64,
            rejected,
            unresolved: report.unresolved.len() as u64,
        }]
    }

    /// After the peer's heads: send what it lacks if we hold all its heads, or offer what it may lack.
    fn plan(&mut self, graph: &Graph, peer_heads: &[ContributionId]) -> Result<Step, SyncError> {
        let held: Vec<ContributionId> = peer_heads.iter().filter(|h| graph.contains(h)).copied().collect();
        let known = ancestry(graph, &held);
        let candidates: Vec<ContributionId> = graph.topo_order().into_iter().filter(|id| !known.contains(id)).collect();
        if held.len() == peer_heads.len() {
            self.queue = candidates.into();
            self.outbound = Outbound::Sending;
            return self.next_batch(graph);
        }
        self.offered = candidates.iter().copied().collect();
        self.outbound = Outbound::Offered;
        Ok(Step { send: split(&candidates, MAX_IDS, |ids| Message::Offer { ids }), import: None })
    }

    /// The next batch from the queue, as large as both sides' limits allow, so a peer can't make
    /// this side build a batch larger than it would accept itself. The last one says so.
    fn next_batch(&mut self, graph: &Graph) -> Result<Step, SyncError> {
        let peer = self.peer_limits.expect("hello comes before anything is sent").batch_bytes;
        let limit = peer.min(self.limits.batch_bytes);
        let mut size = BUNDLE_HEADER as u64;
        let mut batch = Vec::new();
        while let Some(id) = self.queue.front() {
            let c = graph.get(id).expect("offered and planned contributions are in the graph");
            let entry = (BUNDLE_ENTRY + c.encode().len()) as u64;
            if size + entry > limit {
                if batch.is_empty() {
                    return Err(SyncError::TooLargeForPeer(*id));
                }
                break;
            }
            size += entry;
            batch.push(c.clone());
            self.queue.pop_front();
        }
        let last = self.queue.is_empty();
        self.sent.contributions += batch.len();
        self.sent.batches += 1;
        self.sent_last = last;
        self.awaiting_result = true;
        let mut bundle = Vec::new();
        bundle::write(&mut bundle, self.ground, batch).expect("writing to a Vec cannot fail");
        Ok(Step { send: vec![Message::Batch { bundle, last }], import: None })
    }

    /// A batch from the peer: checked against the limit, the Ground, and what the peer may send.
    /// That's what we asked for with Need and haven't received yet, or, if the peer took the
    /// subset shortcut, anything we didn't hold when we sent our heads (the graph may have grown
    /// since, from elsewhere) and the peer hasn't sent yet. Only the last batch may be empty, and
    /// after a Need, the last batch must bring the last needed contribution.
    fn batch(&mut self, graph: &Graph, bundle: &[u8], last: bool) -> Result<Step, SyncError> {
        if bundle.len() as u64 > self.limits.batch_bytes {
            return Err(SyncError::BatchTooLarge(bundle.len(), self.limits.batch_bytes));
        }
        let (ground, contributions) = bundle::read(bundle).map_err(|e| SyncError::BadBatch(e.to_string()))?;
        if ground != self.ground {
            return Err(SyncError::ForeignGround { ours: self.ground, peer: ground });
        }
        if contributions.is_empty() && !last {
            return Err(SyncError::EmptyBatch);
        }
        let unrequested = match &mut self.needed {
            Some(needed) => contributions.iter().find(|c| !needed.remove(&c.id())),
            None => {
                let held = self.held_at_start.get_or_insert_with(|| ancestry(graph, &self.own_heads));
                contributions.iter().find(|c| held.contains(&c.id()) || !self.received_ids.insert(c.id()))
            }
        };
        if let Some(c) = unrequested {
            return Err(SyncError::Unrequested(c.id()));
        }
        if let Some(missing) = self.needed.as_ref().filter(|_| last).and_then(|needed| needed.iter().min()) {
            return Err(SyncError::MissingNeeded(*missing));
        }
        self.inbound = if last { Inbound::Done } else { Inbound::Batches };
        self.received.contributions += contributions.len();
        self.received.batches += 1;
        self.awaiting_import = true;
        Ok(Step { send: vec![], import: Some(contributions) })
    }
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn truncate(mut text: String, max: usize) -> String {
    if text.len() > max {
        let end = (0..=max).rev().find(|&i| text.is_char_boundary(i)).expect("0 is a char boundary");
        text.truncate(end);
    }
    text
}

/// `ids` as messages of at most `chunk` ids each: every one but the last full, the last short
/// (empty if `ids` fills the others exactly), so the receiver knows where the list ends.
fn split(ids: &[ContributionId], chunk: usize, message: impl Fn(Vec<ContributionId>) -> Message) -> Vec<Message> {
    let mut messages: Vec<Message> = ids.chunks(chunk).map(|part| message(part.to_vec())).collect();
    if ids.len().is_multiple_of(chunk) {
        messages.push(message(vec![]));
    }
    messages
}

/// `roots` and all their ancestors, all of which `graph` holds.
fn ancestry(graph: &Graph, roots: &[ContributionId]) -> HashSet<ContributionId> {
    let mut seen: HashSet<ContributionId> = HashSet::new();
    let mut stack: Vec<ContributionId> = roots.to_vec();
    while let Some(id) = stack.pop() {
        if seen.insert(id) {
            let c = graph.get(&id).expect("held contributions have their ancestors");
            stack.extend(c.parents().iter().filter(|p| !seen.contains(p)));
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u8) -> Vec<ContributionId> {
        (0..n).map(|i| ContributionId([i; 32])).collect()
    }

    fn lens(messages: &[Message]) -> Vec<usize> {
        messages
            .iter()
            .map(|m| match m {
                Message::Offer { ids } => ids.len(),
                _ => panic!("not an offer"),
            })
            .collect()
    }

    #[test]
    fn reasons_are_cut_at_a_character_boundary() {
        assert_eq!(truncate("abc".into(), 5), "abc");
        assert_eq!(truncate("abcdef".into(), 3), "abc");
        assert_eq!(truncate("aé".into(), 2), "a");
    }

    #[test]
    fn a_list_is_split_into_full_parts_and_a_short_last_one() {
        let offer = |ids| Message::Offer { ids };
        assert_eq!(lens(&split(&ids(0), 3, offer)), [0]);
        assert_eq!(lens(&split(&ids(2), 3, offer)), [2]);
        assert_eq!(lens(&split(&ids(3), 3, offer)), [3, 0]);
        assert_eq!(lens(&split(&ids(7), 3, offer)), [3, 3, 1]);
        let parts = split(&ids(7), 3, offer);
        let joined: Vec<ContributionId> =
            parts.into_iter().flat_map(|m| if let Message::Offer { ids } = m { ids } else { vec![] }).collect();
        assert_eq!(joined, ids(7));
    }
}
