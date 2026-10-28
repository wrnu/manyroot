//! Sync sessions: replicas converge after one session, each receiving only what it lacked,
//! and sessions refuse foreign Grounds, oversized batches and protocol violations.

use std::collections::{BTreeSet, HashSet, VecDeque};

use manyroot::sync::{Limits, MAX_IDS, Message, Session, SyncError, Tally};
use manyroot::*;

/// A small deterministic generator, so every failure has a seed to reproduce it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const GROUND: GroundId = GroundId([7; 32]);

/// A random DAG of `n` contributions: claims, plus relations and evidence about earlier ones,
/// each with one to three random earlier parents.
fn random_history(rng: &mut Rng, n: usize) -> Vec<Contribution> {
    let people = [Identity::generate("a").unwrap(), Identity::generate("b").unwrap()];
    let mut all: Vec<Contribution> = Vec::new();
    for i in 0..n {
        let who = &people[rng.below(2)];
        let parents: Vec<ContributionId> = if all.is_empty() {
            vec![]
        } else {
            (0..1 + rng.below(3)).map(|_| all[rng.below(all.len())].id()).collect()
        };
        let body = match (rng.below(3), parents.first()) {
            (1, Some(&to)) => Body::Relation {
                from: parents[parents.len() - 1],
                kind: RelationKind::Refines,
                to,
                note: String::new(),
            },
            (2, Some(&target)) => {
                Body::Evidence { target, verdict: Verdict::Pass, method: format!("check {i}"), details: String::new() }
            }
            _ => Body::Claim { statement: format!("claim {i}"), tags: vec![] },
        };
        let body = match body {
            Body::Relation { from, to, .. } if from == to => {
                Body::Claim { statement: format!("claim {i}"), tags: vec![] }
            }
            body => body,
        };
        all.push(Contribution::create(GROUND, who, parents, i as u64, body));
    }
    all
}

/// The contributions in `picks` and all their ancestors, as a graph.
fn closure(all: &[Contribution], picks: &[usize]) -> Graph {
    let mut wanted: HashSet<ContributionId> = HashSet::new();
    let mut stack: Vec<ContributionId> = picks.iter().map(|&i| all[i].id()).collect();
    while let Some(id) = stack.pop() {
        if wanted.insert(id) {
            let c = all.iter().find(|c| c.id() == id).unwrap();
            stack.extend(c.parents().iter().copied());
        }
    }
    let mut g = Graph::new(GROUND);
    let report = g.import(all.iter().filter(|c| wanted.contains(&c.id())).cloned());
    assert!(report.is_clean());
    g
}

fn ids(g: &Graph) -> BTreeSet<ContributionId> {
    g.iter().map(|c| c.id()).collect()
}

struct Outcome {
    imported: [ImportReport; 2],
    received: [Tally; 2],
    /// Message types each side sent, in order.
    sent: [Vec<&'static str>; 2],
}

fn kind(m: &Message) -> &'static str {
    match m {
        Message::Hello { .. } => "hello",
        Message::Heads { .. } => "heads",
        Message::Offer { .. } => "offer",
        Message::Need { .. } => "need",
        Message::Batch { .. } => "batch",
        Message::Result { .. } => "result",
    }
}

/// Runs one session between two in-memory replicas, every message through its encoding.
fn run(graphs: [&mut Graph; 2], limits: [Limits; 2]) -> Result<Outcome, SyncError> {
    let mut sessions = [Session::new(GROUND, limits[0]), Session::new(GROUND, limits[1])];
    let mut inbox: [VecDeque<Message>; 2] = Default::default();
    let mut out = Outcome { imported: Default::default(), received: Default::default(), sent: Default::default() };
    for i in 0..2 {
        let opening = sessions[i].start(graphs[i]);
        out.sent[i].extend(opening.iter().map(kind));
        inbox[1 - i].extend(opening);
    }
    while let Some(i) = (0..2).find(|&i| !inbox[i].is_empty()) {
        let message = inbox[i].pop_front().unwrap();
        let message = Message::decode(&message.encode(), limits[i].max_message_bytes())?;
        let step = sessions[i].receive(graphs[i], message)?;
        let mut replies = step.send;
        if let Some(batch) = step.import {
            let report = graphs[i].import(batch);
            replies.extend(sessions[i].imported(&report));
            out.imported[i].added.extend(report.added);
            out.imported[i].already_present += report.already_present;
            out.imported[i].rejected.extend(report.rejected);
            out.imported[i].unresolved.extend(report.unresolved);
        }
        out.sent[i].extend(replies.iter().map(kind));
        inbox[1 - i].extend(replies);
    }
    assert!(sessions.iter().all(Session::is_done), "the session ran to completion");
    out.received = [sessions[0].received(), sessions[1].received()];
    Ok(out)
}

#[test]
fn random_overlapping_replicas_converge_receiving_only_what_they_lacked() {
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let all = random_history(&mut rng, 40);
        let picks = |rng: &mut Rng| (0..1 + rng.below(4)).map(|_| rng.below(all.len())).collect::<Vec<_>>();
        let (pa, pb) = (picks(&mut rng), picks(&mut rng));
        let (mut a, mut b) = (closure(&all, &pa), closure(&all, &pb));
        let (before_a, before_b) = (ids(&a), ids(&b));
        let union: BTreeSet<ContributionId> = before_a.union(&before_b).copied().collect();
        // Half the seeds use a limit small enough to split the exchange into many batches.
        let limit = if seed % 2 == 0 { Limits { batch_bytes: 1500 } } else { Limits::default() };

        let out = run([&mut a, &mut b], [limit, limit]).unwrap_or_else(|e| panic!("seed {seed}: {e}"));

        assert_eq!(ids(&a), union, "seed {seed}: a holds the union");
        assert_eq!(ids(&b), union, "seed {seed}: b holds the union");
        let lacked = [&before_b - &before_a, &before_a - &before_b];
        for (side, (lacked, (imported, received))) in
            lacked.iter().zip(out.imported.iter().zip(out.received)).enumerate()
        {
            let added: BTreeSet<ContributionId> = imported.added.iter().copied().collect();
            assert_eq!(&added, lacked, "seed {seed}: side {side} added exactly what it lacked");
            assert_eq!(imported.already_present, 0, "seed {seed}: side {side} got nothing it held");
            assert!(imported.rejected.is_empty() && imported.unresolved.is_empty());
            assert_eq!(received.contributions, lacked.len(), "seed {seed}: side {side} count");
        }
        assert_eq!(a.topo_order(), b.topo_order(), "seed {seed}: same causal order");
    }
}

#[test]
fn a_replica_holding_all_the_peer_heads_skips_the_offer() {
    let mut rng = Rng(42);
    let all = random_history(&mut rng, 30);
    let mut ahead = closure(&all, &[29, 20, 10]);
    let mut behind = closure(&all, &[5]);
    assert!(ids(&behind).is_subset(&ids(&ahead)));
    let lacked = ids(&ahead).len() - ids(&behind).len();

    let out = run([&mut ahead, &mut behind], [Limits::default(); 2]).unwrap();
    assert!(!out.sent[0].contains(&"offer"), "the side ahead sends straight away: {:?}", out.sent[0]);
    assert!(out.sent[1].contains(&"offer"), "the side behind offers what it has: {:?}", out.sent[1]);
    assert_eq!(out.received[1].contributions, lacked, "the side behind receives exactly what it lacked");
    assert_eq!(out.received[0].contributions, 0);
    assert_eq!(ids(&ahead), ids(&behind));

    // Already in sync: each side sends one empty last batch and nothing else.
    let out = run([&mut ahead, &mut behind], [Limits::default(); 2]).unwrap();
    assert_eq!(out.sent, [["hello", "heads", "batch", "result"], ["hello", "heads", "batch", "result"]]);
    assert_eq!(out.received, [Tally { contributions: 0, batches: 1 }; 2]);
}

fn hello(ground: GroundId, protocol: u32) -> Message {
    Message::Hello { protocol, ground, limits: Limits::default(), features: vec![] }
}

#[test]
fn a_foreign_ground_or_protocol_ends_the_session_before_anything_is_read() {
    let g = Graph::new(GROUND);
    let mut s = Session::new(GROUND, Limits::default());
    let other = GroundId([8; 32]);
    assert_eq!(s.receive(&g, hello(other, 1)).unwrap_err(), SyncError::ForeignGround { ours: GROUND, peer: other });
    let mut s = Session::new(GROUND, Limits::default());
    assert_eq!(s.receive(&g, hello(GROUND, 2)).unwrap_err(), SyncError::UnsupportedProtocol(2));
    let mut s = Session::new(GROUND, Limits::default());
    assert!(matches!(s.receive(&g, Message::Heads { heads: vec![] }), Err(SyncError::Unexpected("heads"))));

    // A batch from another Ground is refused too.
    let mut s = Session::new(GROUND, Limits::default());
    s.receive(&g, hello(GROUND, 1)).unwrap();
    s.receive(&g, Message::Heads { heads: vec![] }).unwrap();
    let mut bundle = Vec::new();
    bundle::write(&mut bundle, other, Vec::<Contribution>::new()).unwrap();
    let foreign = Message::Batch { bundle, last: true };
    assert_eq!(s.receive(&g, foreign).unwrap_err(), SyncError::ForeignGround { ours: GROUND, peer: other });
}

#[test]
fn oversized_batches_and_messages_are_refused() {
    let mut rng = Rng(7);
    let all = random_history(&mut rng, 20);
    let g = Graph::new(GROUND);
    let small = Limits { batch_bytes: 500 };
    let mut s = Session::new(GROUND, small);
    s.receive(&g, hello(GROUND, 1)).unwrap();
    s.receive(&g, Message::Heads { heads: vec![all[19].id()] }).unwrap();
    let mut bundle = Vec::new();
    bundle::write(&mut bundle, GROUND, all.clone()).unwrap();
    let size = bundle.len();
    assert!(size > 500);
    let batch = Message::Batch { bundle, last: true };
    assert_eq!(s.receive(&g, batch).unwrap_err(), SyncError::BatchTooLarge(size, 500));

    // The message limit is the larger of a batch and a full id list, whatever batch_bytes is.
    let full_list = Message::Offer { ids: vec![ContributionId([1; 32]); MAX_IDS] }.encode().len();
    assert!(full_list <= small.max_message_bytes(), "{full_list} > {}", small.max_message_bytes());
    assert!(small.max_message_bytes() < full_list + 64);
    let bytes = Message::Batch { bundle: vec![0; small.max_message_bytes()], last: true }.encode();
    assert_eq!(
        Message::decode(&bytes, small.max_message_bytes()).unwrap_err(),
        SyncError::MessageTooLarge(bytes.len(), small.max_message_bytes())
    );

    // A sender whose contribution can't fit the peer's limit stops instead of sending a partial batch.
    let mut full = closure(&all, &[19]);
    let mut empty = Graph::new(GROUND);
    let tiny = Limits { batch_bytes: 60 };
    assert!(matches!(run([&mut full, &mut empty], [Limits::default(), tiny]), Err(SyncError::TooLargeForPeer(_))));
}

#[test]
fn a_peer_must_send_only_what_was_needed_and_ask_only_for_what_was_offered() {
    let mut rng = Rng(9);
    let all = random_history(&mut rng, 10);
    let g = closure(&all, &[3]);
    let held: Vec<ContributionId> = g.iter().map(|c| c.id()).collect();
    let mut s = Session::new(GROUND, Limits::default());
    s.receive(&g, hello(GROUND, 1)).unwrap();
    s.receive(&g, Message::Heads { heads: vec![all[9].id()] }).unwrap();
    let need = s.receive(&g, Message::Offer { ids: vec![all[9].id()] }).unwrap();
    assert!(matches!(&need.send[..], [Message::Need { ids }] if !ids.is_empty()));
    let mut bundle = Vec::new();
    bundle::write(&mut bundle, GROUND, vec![g.get(&held[0]).unwrap().clone()]).unwrap();
    assert!(matches!(s.receive(&g, Message::Batch { bundle, last: true }), Err(SyncError::Unrequested(_))));

    // Our side offered nothing it isn't asked about.
    let mut s = Session::new(GROUND, Limits::default());
    s.receive(&g, hello(GROUND, 1)).unwrap();
    s.receive(&g, Message::Heads { heads: vec![all[9].id()] }).unwrap();
    assert!(matches!(s.receive(&g, Message::Need { ids: vec![all[9].id()] }), Err(SyncError::NotOffered(_))));
}

#[test]
fn messages_must_be_canonical() {
    let message = Message::Need { ids: vec![ContributionId([1; 32])] };
    let mut bytes = message.encode();
    assert_eq!(Message::decode(&bytes, 1000).unwrap(), message);
    bytes.push(0);
    assert_eq!(Message::decode(&bytes, 1000).unwrap_err(), SyncError::NotCanonical);
}

#[test]
fn under_the_shortcut_only_what_the_receiver_held_at_the_start_is_over_sending() {
    let mut rng = Rng(11);
    let all = random_history(&mut rng, 12);
    let g = closure(&all, &[3]);
    let held = g.iter().next().unwrap().clone();
    let batch = |cs: Vec<Contribution>| {
        let mut bundle = Vec::new();
        bundle::write(&mut bundle, GROUND, cs).unwrap();
        Message::Batch { bundle, last: true }
    };
    let opened = |g: &Graph| {
        let mut s = Session::new(GROUND, Limits::default());
        s.start(g);
        s.receive(g, hello(GROUND, 1)).unwrap();
        s.receive(g, Message::Heads { heads: vec![all[11].id()] }).unwrap();
        s
    };

    // The peer took the shortcut (a batch with no offer) and sent something we held: refused.
    let mut s = opened(&g);
    assert_eq!(s.receive(&g, batch(vec![held])).unwrap_err(), SyncError::Unrequested(g.iter().next().unwrap().id()));

    // Something we gained from elsewhere after sending our heads isn't over-sending.
    let mut s = opened(&g);
    let mut grown = g.clone();
    let later = all.iter().find(|c| !g.contains(&c.id()) && c.parents().iter().all(|p| g.contains(p))).unwrap().clone();
    assert!(grown.import([later.clone()]).is_clean());
    let step = s.receive(&grown, batch(vec![later])).unwrap();
    assert_eq!(step.import.map(|cs| cs.len()), Some(1));
}

/// A session that has had the peer's hello and empty heads, so it offers what it holds.
fn opened(g: &Graph) -> Session {
    let mut s = Session::new(GROUND, Limits::default());
    s.start(g);
    s.receive(g, hello(GROUND, 1)).unwrap();
    s
}

fn batch(cs: Vec<Contribution>, last: bool) -> Message {
    let mut bundle = Vec::new();
    bundle::write(&mut bundle, GROUND, cs).unwrap();
    Message::Batch { bundle, last }
}

#[test]
fn a_need_or_offer_listing_an_id_twice_ends_the_session() {
    let mut rng = Rng(13);
    let all = random_history(&mut rng, 6);
    let g = closure(&all, &[5]);
    let unknown = ContributionId([9; 32]);

    // We offered everything we hold (the peer's head is unknown to us); a need repeating an id is refused.
    let mut s = opened(&g);
    let step = s.receive(&g, Message::Heads { heads: vec![unknown] }).unwrap();
    let [Message::Offer { ids: offered }] = &step.send[..] else { panic!("expected one offer: {:?}", step.send) };
    let twice = vec![offered[0], offered[1], offered[0]];
    assert_eq!(s.receive(&g, Message::Need { ids: twice }).unwrap_err(), SyncError::Repeated("need", offered[0]));

    // So is an offer repeating an id.
    let mut s = opened(&g);
    s.receive(&g, Message::Heads { heads: vec![unknown] }).unwrap();
    let ids = vec![unknown, ContributionId([10; 32]), unknown];
    assert_eq!(s.receive(&g, Message::Offer { ids }).unwrap_err(), SyncError::Repeated("offer", unknown));
}

#[test]
fn a_contribution_sent_twice_is_over_sending() {
    let mut rng = Rng(17);
    let all = random_history(&mut rng, 8);
    let empty = Graph::new(GROUND);
    let roots: Vec<Contribution> = all.iter().filter(|c| c.parents().is_empty()).cloned().collect();
    let c0 = roots[0].clone();

    // After a need: once a needed contribution has arrived, it isn't outstanding any more.
    let mut s = opened(&empty);
    s.receive(&empty, Message::Heads { heads: vec![c0.id()] }).unwrap();
    s.receive(&empty, Message::Offer { ids: vec![c0.id(), all[1].id()] }).unwrap();
    assert!(s.receive(&empty, batch(vec![c0.clone()], false)).unwrap().import.is_some());
    let mut g = empty.clone();
    s.imported(&g.import([c0.clone()]));
    assert_eq!(s.receive(&g, batch(vec![c0.clone()], false)).unwrap_err(), SyncError::Unrequested(c0.id()));
    // Twice in one batch, too.
    let mut s = opened(&empty);
    s.receive(&empty, Message::Heads { heads: vec![c0.id()] }).unwrap();
    s.receive(&empty, Message::Offer { ids: vec![c0.id()] }).unwrap();
    assert_eq!(
        s.receive(&empty, batch(vec![c0.clone(), c0.clone()], true)).unwrap_err(),
        SyncError::Unrequested(c0.id())
    );

    // Under the shortcut (our heads are empty, so the peer holds them all): what it already sent.
    let mut s = opened(&empty);
    s.receive(&empty, Message::Heads { heads: vec![c0.id()] }).unwrap();
    assert!(s.receive(&empty, batch(vec![c0.clone()], false)).is_ok());
    s.imported(&g.clone().import([c0.clone()]));
    assert_eq!(s.receive(&g, batch(vec![c0.clone()], true)).unwrap_err(), SyncError::Unrequested(c0.id()));

    // Only the last batch may be empty, so a peer can't keep a session open with empty batches.
    let mut s = opened(&empty);
    s.receive(&empty, Message::Heads { heads: vec![c0.id()] }).unwrap();
    assert_eq!(s.receive(&empty, batch(vec![], false)).unwrap_err(), SyncError::EmptyBatch);
}

#[test]
fn a_small_batch_limit_still_takes_a_large_offer() {
    // Two unrelated histories, so neither side holds the other's heads and both offer.
    let all = random_history(&mut Rng(19), 150);
    let other = random_history(&mut Rng(23), 20);
    let mut a = closure(&all, &(0..all.len()).collect::<Vec<_>>());
    let mut b = closure(&other, &(0..other.len()).collect::<Vec<_>>());
    let small = Limits { batch_bytes: 2000 };
    let offer = Message::Offer { ids: ids(&a).into_iter().collect() }.encode().len();
    assert!(offer > 2064, "the offer ({offer} bytes) is larger than a batch message");
    let union: BTreeSet<ContributionId> = ids(&a).union(&ids(&b)).copied().collect();

    let out = run([&mut a, &mut b], [small, small]).unwrap();
    assert!(out.sent[0].contains(&"offer") && out.sent[1].contains(&"offer"), "{:?}", out.sent);
    assert_eq!((ids(&a), ids(&b)), (union.clone(), union));
}

#[test]
fn id_lists_longer_than_max_ids_are_split_and_end_with_a_short_message() {
    let g = Graph::new(GROUND);
    let fake = |n: usize, salt: u8| -> Vec<ContributionId> {
        (0..n)
            .map(|i| {
                let mut id = [salt; 32];
                id[..8].copy_from_slice(&(i as u64).to_be_bytes());
                ContributionId(id)
            })
            .collect()
    };

    // Heads in two parts: a full one, then a short one, after which the session plans.
    let mut s = opened(&g);
    assert!(s.receive(&g, Message::Heads { heads: fake(MAX_IDS, 1) }).unwrap().send.is_empty());
    let step = s.receive(&g, Message::Heads { heads: fake(1, 2) }).unwrap();
    assert!(matches!(&step.send[..], [Message::Offer { ids }] if ids.is_empty()));

    // An offer in two parts gets a need in two parts: MAX_IDS + 1 ids, all lacked.
    assert!(s.receive(&g, Message::Offer { ids: fake(MAX_IDS, 3) }).unwrap().send.is_empty());
    let step = s.receive(&g, Message::Offer { ids: fake(1, 4) }).unwrap();
    let parts: Vec<usize> = step
        .send
        .iter()
        .map(|m| match m {
            Message::Need { ids } => ids.len(),
            m => panic!("expected need, got {m:?}"),
        })
        .collect();
    assert_eq!(parts, [MAX_IDS, 1]);

    // No message may carry more than MAX_IDS ids.
    let mut s = opened(&g);
    let too_many = Message::Heads { heads: fake(MAX_IDS + 1, 5) };
    assert_eq!(s.receive(&g, too_many).unwrap_err(), SyncError::TooManyIds("heads", MAX_IDS + 1));
}

#[test]
fn a_last_batch_while_needed_contributions_are_missing_ends_the_session() {
    let all = random_history(&mut Rng(29), 8);
    let roots: Vec<Contribution> = all.iter().filter(|c| c.parents().is_empty()).cloned().collect();
    let c0 = roots[0].clone();
    let c1 = all.iter().find(|c| c.parents() == [c0.id()]).expect("a child of the root").clone();
    let empty = Graph::new(GROUND);
    let needing = || {
        let mut s = opened(&empty);
        s.receive(&empty, Message::Heads { heads: vec![c1.id()] }).unwrap();
        let step = s.receive(&empty, Message::Offer { ids: vec![c0.id(), c1.id()] }).unwrap();
        assert!(matches!(&step.send[..], [Message::Need { ids }] if ids.len() == 2));
        s
    };

    // The last batch brings only part of what was needed.
    let mut s = needing();
    assert_eq!(s.receive(&empty, batch(vec![c0.clone()], true)).unwrap_err(), SyncError::MissingNeeded(c1.id()));

    // An empty last batch after part of it.
    let mut s = needing();
    s.receive(&empty, batch(vec![c0.clone()], false)).unwrap();
    let mut g = empty.clone();
    s.imported(&g.import([c0.clone()]));
    assert_eq!(s.receive(&g, batch(vec![], true)).unwrap_err(), SyncError::MissingNeeded(c1.id()));

    // Everything needed, then last: done.
    let mut s = needing();
    s.receive(&empty, batch(vec![c0.clone()], false)).unwrap();
    let mut g = empty.clone();
    s.imported(&g.import([c0.clone()]));
    s.receive(&g, batch(vec![c1.clone()], true)).unwrap();
    s.imported(&g.import([c1.clone()]));
}

#[test]
fn a_result_fits_the_peer_message_limit_whatever_either_batch_limit() {
    // A receiver with a large batch limit that rejects everything in a huge batch still answers
    // with a Result the smallest-limit sender can decode.
    let mut s = Session::new(GROUND, Limits::default());
    let long = RuleViolation::WrongGround { expected: GroundId([1; 32]), found: GroundId([2; 32]) };
    let report = ImportReport {
        rejected: (0..100_000u32)
            .map(|i| {
                let mut id = [0; 32];
                id[..4].copy_from_slice(&i.to_be_bytes());
                (ContributionId(id), long.clone())
            })
            .collect(),
        ..Default::default()
    };
    let [result] = &s.imported(&report)[..] else { panic!("one result") };
    let Message::Result { rejected, .. } = result else { panic!("a result") };
    assert_eq!(rejected.len(), manyroot::sync::MAX_REJECTED);
    let bytes = result.encode();
    let smallest = Limits { batch_bytes: 44 };
    assert_eq!(&Message::decode(&bytes, smallest.max_message_bytes()).unwrap(), result);

    // And the session itself with asymmetric limits, both ways round.
    let a_all = random_history(&mut Rng(31), 60);
    let b_all = random_history(&mut Rng(37), 60);
    for limits in [[Limits { batch_bytes: 1000 }, Limits::default()], [Limits::default(), Limits { batch_bytes: 1000 }]]
    {
        let mut a = closure(&a_all, &(0..60).collect::<Vec<_>>());
        let mut b = closure(&b_all, &(0..60).collect::<Vec<_>>());
        let union: BTreeSet<ContributionId> = ids(&a).union(&ids(&b)).copied().collect();
        run([&mut a, &mut b], limits).unwrap();
        assert_eq!((ids(&a), ids(&b)), (union.clone(), union));
    }
}

fn bundle_len(cs: &[Contribution]) -> usize {
    let mut bundle = Vec::new();
    bundle::write(&mut bundle, GROUND, cs.to_vec()).unwrap();
    bundle.len()
}

#[test]
fn a_bundle_of_exactly_batch_bytes_is_accepted_and_one_byte_more_refused() {
    let all = random_history(&mut Rng(41), 6);
    let roots: Vec<Contribution> = all.iter().filter(|c| c.parents().is_empty()).cloned().collect();
    let size = bundle_len(&roots);
    let empty = Graph::new(GROUND);
    let receiving = |batch_bytes: usize| {
        let mut s = Session::new(GROUND, Limits { batch_bytes: batch_bytes as u64 });
        s.start(&empty);
        s.receive(&empty, hello(GROUND, 1)).unwrap();
        s.receive(&empty, Message::Heads { heads: vec![roots[0].id()] }).unwrap();
        s
    };
    assert!(receiving(size).receive(&empty, batch(roots.clone(), true)).is_ok());
    assert_eq!(
        receiving(size - 1).receive(&empty, batch(roots.clone(), true)).unwrap_err(),
        SyncError::BatchTooLarge(size, size as u64 - 1)
    );
}

#[test]
fn a_sender_fills_a_batch_to_exactly_the_peer_limit() {
    let all = random_history(&mut Rng(43), 10);
    let g = closure(&all, &(0..all.len()).collect::<Vec<_>>());
    let order: Vec<Contribution> = g.topo_order().iter().map(|id| g.get(id).unwrap().clone()).collect();
    // The peer holds nothing, so the sender takes the shortcut and sends its first batch at once.
    let first_batch = |peer_batch_bytes: usize| -> Result<Vec<u8>, SyncError> {
        let mut s = Session::new(GROUND, Limits::default());
        s.start(&g);
        let peer = Limits { batch_bytes: peer_batch_bytes as u64 };
        s.receive(&g, Message::Hello { protocol: 1, ground: GROUND, limits: peer, features: vec![] }).unwrap();
        match &s.receive(&g, Message::Heads { heads: vec![] })?.send[..] {
            [Message::Batch { bundle, .. }] => Ok(bundle.clone()),
            other => panic!("expected one batch: {other:?}"),
        }
    };
    for k in [1, 3, 6] {
        let exact = bundle_len(&order[..k]);
        let bundle = first_batch(exact).unwrap();
        assert_eq!(bundle.len(), exact, "a limit of exactly {k} contributions' bundle takes all {k}");
        match first_batch(exact - 1) {
            Ok(bundle) => assert_eq!(bundle.len(), bundle_len(&order[..k - 1]), "one byte less takes {}", k - 1),
            Err(e) => assert_eq!((k, e), (1, SyncError::TooLargeForPeer(order[0].id()))),
        }
    }

    // The header is the magic and the Ground id: an empty last batch is 40 bytes and fits a 44-byte peer.
    let mut s = Session::new(GROUND, Limits::default());
    let empty = Graph::new(GROUND);
    s.start(&empty);
    let peer = Limits { batch_bytes: 44 };
    s.receive(&empty, Message::Hello { protocol: 1, ground: GROUND, limits: peer, features: vec![] }).unwrap();
    let step = s.receive(&empty, Message::Heads { heads: vec![] }).unwrap();
    assert!(matches!(&step.send[..], [Message::Batch { bundle, last: true }] if bundle.len() == 40));
}

#[test]
fn a_sender_caps_its_batches_at_its_own_limit_when_that_is_smaller() {
    let all = random_history(&mut Rng(47), 10);
    let g = closure(&all, &(0..all.len()).collect::<Vec<_>>());
    let order: Vec<Contribution> = g.topo_order().iter().map(|id| g.get(id).unwrap().clone()).collect();
    // The peer accepts anything and holds nothing; the sender's own limit is the smaller one.
    let first_batch = |own_batch_bytes: usize| -> Result<Vec<u8>, SyncError> {
        let mut s = Session::new(GROUND, Limits { batch_bytes: own_batch_bytes as u64 });
        s.start(&g);
        let peer = Limits { batch_bytes: u64::MAX };
        s.receive(&g, Message::Hello { protocol: 1, ground: GROUND, limits: peer, features: vec![] }).unwrap();
        match &s.receive(&g, Message::Heads { heads: vec![] })?.send[..] {
            [Message::Batch { bundle, .. }] => Ok(bundle.clone()),
            other => panic!("expected one batch: {other:?}"),
        }
    };
    for k in [1, 3, 6] {
        let exact = bundle_len(&order[..k]);
        assert_eq!(first_batch(exact).unwrap().len(), exact, "an own limit of exactly {k} contributions takes {k}");
        match first_batch(exact - 1) {
            Ok(bundle) => assert_eq!(bundle.len(), bundle_len(&order[..k - 1]), "one byte less takes {}", k - 1),
            Err(e) => assert_eq!((k, e), (1, SyncError::TooLargeForOwnLimit(order[0].id()))),
        }
    }

    // Two replicas still converge, the capped side sending many small batches.
    let small = Limits { batch_bytes: bundle_len(&order[..2]) as u64 };
    let mut empty = Graph::new(GROUND);
    let mut full = g.clone();
    let out = run([&mut full, &mut empty], [small, Limits::default()]).unwrap();
    assert_eq!(ids(&empty), ids(&g));
    assert!(out.received[1].batches >= 5, "{:?}", out.received[1]);
}

#[test]
fn a_hello_needs_batch_bytes_of_at_least_44() {
    let g = Graph::new(GROUND);
    let with =
        |batch_bytes| Message::Hello { protocol: 1, ground: GROUND, limits: Limits { batch_bytes }, features: vec![] };
    let mut s = Session::new(GROUND, Limits::default());
    assert_eq!(s.receive(&g, with(43)).unwrap_err(), SyncError::LimitTooSmall(43));
    let mut s = Session::new(GROUND, Limits::default());
    assert!(s.receive(&g, with(44)).is_ok());
}

#[test]
fn reasons_over_256_bytes_are_cut() {
    let suffix = RuleViolation::Empty("").to_string().len();
    for (total, sent) in [(256, 256), (257, 256), (255, 255)] {
        let field: &'static str = Box::leak("f".repeat(total - suffix).into_boxed_str());
        let report = ImportReport {
            rejected: vec![(ContributionId([1; 32]), RuleViolation::Empty(field))],
            ..Default::default()
        };
        assert_eq!(RuleViolation::Empty(field).to_string().len(), total);
        let mut s = Session::new(GROUND, Limits::default());
        let [Message::Result { rejected, .. }] = &s.imported(&report)[..] else { panic!("one result") };
        assert_eq!(rejected[0].reason.len(), sent, "a {total}-byte reason");
    }
}

#[test]
fn a_session_waits_for_the_import_of_each_batch() {
    // Two empty replicas: each sends one empty last batch. Deliver a's batch and b's result first,
    // so a's sending is finished when b's last batch arrives.
    let g = Graph::new(GROUND);
    let (mut a, mut b) = (Session::new(GROUND, Limits::default()), Session::new(GROUND, Limits::default()));
    let (from_a, from_b) = (a.start(&g), b.start(&g));
    let mut a_batch = vec![];
    for m in from_b {
        a_batch.extend(a.receive(&g, m).unwrap().send);
    }
    let mut b_batch = vec![];
    for m in from_a {
        b_batch.extend(b.receive(&g, m).unwrap().send);
    }
    let [a_batch] = &a_batch[..] else { panic!("a sends one batch") };
    let [b_batch] = &b_batch[..] else { panic!("b sends one batch") };
    let step = b.receive(&g, a_batch.clone()).unwrap();
    assert!(step.import.is_some());
    let result = b.imported(&ImportReport::default());
    // b has a's last batch and imported it, but its own batch is unanswered: only inbound is done.
    assert!(!b.is_done(), "done with b's own last batch unanswered");
    for m in result.clone() {
        a.receive(&g, m).unwrap();
    }
    // a's last batch is answered, but b's hasn't arrived: only outbound is done.
    assert!(!a.is_done(), "done before the peer's last batch arrived");

    // a has b's last batch but hasn't imported it: not done, and no other message is taken.
    assert!(a.receive(&g, b_batch.clone()).unwrap().import.is_some());
    assert!(!a.is_done(), "done before the last batch was imported");
    let early = a.receive(&g, result[0].clone()).unwrap_err();
    assert_eq!(early, SyncError::Unexpected("message before the last batch was imported"));
    a.imported(&ImportReport::default());
    assert!(a.is_done());

    // Nothing comes after the peer's last batch.
    assert_eq!(a.receive(&g, b_batch.clone()).unwrap_err(), SyncError::Unexpected("batch"));
}

#[test]
fn a_batch_after_the_last_one_is_refused() {
    // Under the shortcut: the peer sends a root with last, then tries an empty batch and new contributions.
    let all = random_history(&mut Rng(47), 6);
    let roots: Vec<Contribution> = all.iter().filter(|c| c.parents().is_empty()).cloned().collect();
    let children: Vec<Contribution> = all.iter().filter(|c| c.parents() == [roots[0].id()]).cloned().collect();
    assert!(!children.is_empty());
    let mut g = Graph::new(GROUND);
    let mut s = opened(&g);
    s.receive(&g, Message::Heads { heads: vec![roots[0].id()] }).unwrap();
    let step = s.receive(&g, batch(vec![roots[0].clone()], true)).unwrap();
    s.imported(&g.import(step.import.unwrap()));
    assert_eq!(s.receive(&g, batch(vec![], true)).unwrap_err(), SyncError::Unexpected("batch"));
    assert_eq!(s.receive(&g, batch(children, true)).unwrap_err(), SyncError::Unexpected("batch"));
}

#[test]
fn a_need_or_result_out_of_turn_ends_the_session() {
    let all = random_history(&mut Rng(53), 6);
    let g = closure(&all, &(0..all.len()).collect::<Vec<_>>());

    // A result with no batch outstanding: right after hello, and again once our last batch was answered.
    let mut s = opened(&g);
    let result = Message::Result { added: 0, already_present: 0, rejected: vec![], unresolved: 0 };
    assert_eq!(s.receive(&g, result.clone()).unwrap_err(), SyncError::Unexpected("result"));
    let mut s = opened(&g);
    s.receive(&g, Message::Heads { heads: vec![] }).unwrap();
    assert!(s.receive(&g, result.clone()).unwrap().send.is_empty(), "our only batch was the last");
    assert_eq!(s.receive(&g, result).unwrap_err(), SyncError::Unexpected("result"));

    // A need, even an empty one, when we didn't offer: we took the shortcut.
    let mut s = opened(&g);
    s.receive(&g, Message::Heads { heads: vec![] }).unwrap();
    assert_eq!(s.receive(&g, Message::Need { ids: vec![] }).unwrap_err(), SyncError::Unexpected("need"));
}

#[test]
fn a_sender_counts_what_it_sent() {
    let all = random_history(&mut Rng(59), 10);
    let g = closure(&all, &(0..all.len()).collect::<Vec<_>>());
    let order: Vec<Contribution> = g.topo_order().iter().map(|id| g.get(id).unwrap().clone()).collect();
    let mut s = Session::new(GROUND, Limits::default());
    s.start(&g);
    let peer = Limits { batch_bytes: bundle_len(&order[..4]) as u64 };
    s.receive(&g, Message::Hello { protocol: 1, ground: GROUND, limits: peer, features: vec![] }).unwrap();
    assert_eq!(s.sent(), Tally::default());
    s.receive(&g, Message::Heads { heads: vec![] }).unwrap();
    assert_eq!(s.sent(), Tally { contributions: 4, batches: 1 });
    let result = Message::Result { added: 4, already_present: 0, rejected: vec![], unresolved: 0 };
    s.receive(&g, result).unwrap();
    assert_eq!(s.sent().batches, 2);
    assert!(s.sent().contributions > 4);
}

#[test]
fn a_contribution_too_large_for_equal_limits_is_too_large_for_the_peer() {
    let all = random_history(&mut Rng(61), 3);
    let g = closure(&all, &[0]);
    let first = g.iter().next().unwrap().clone();
    // Both sides' limits one byte short of the contribution's bundle: the peer's limit is the reason.
    let limits = Limits { batch_bytes: bundle_len(std::slice::from_ref(&first)) as u64 - 1 };
    let mut s = Session::new(GROUND, limits);
    s.start(&g);
    s.receive(&g, Message::Hello { protocol: 1, ground: GROUND, limits, features: vec![] }).unwrap();
    assert_eq!(s.receive(&g, Message::Heads { heads: vec![] }).unwrap_err(), SyncError::TooLargeForPeer(first.id()));
}
