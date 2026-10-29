use manyroot::graph::{MIN_PREFIX_LEN, ResolveErrorKind};
use manyroot::validation::{open_conflicts, open_supersessors};
use manyroot::*;

fn ground() -> GroundId {
    GroundId([7; 32])
}

fn agent(name: &str) -> Identity {
    Identity::generate(name).unwrap()
}

fn claim(text: &str) -> Body {
    Body::Claim { statement: text.into(), tags: vec![] }
}

fn relate(from: ContributionId, kind: RelationKind, to: ContributionId) -> Body {
    Body::Relation { from, kind, to, note: String::new() }
}

fn evidence(target: ContributionId, verdict: Verdict) -> Body {
    Body::Evidence { target, verdict, method: "test".into(), details: String::new() }
}

/// Adds a contribution on top of all current heads.
fn add(g: &mut Graph, who: &Identity, body: Body) -> ContributionId {
    let heads: Vec<_> = g.heads().collect();
    let c = Contribution::create(g.ground(), who, heads, 0, body);
    let id = c.id();
    assert!(g.insert(c).unwrap());
    id
}

/// Adds a contribution that only depends on what its body references.
fn add_detached(g: &mut Graph, who: &Identity, body: Body) -> ContributionId {
    let c = Contribution::create(g.ground(), who, [], 0, body);
    let id = c.id();
    g.insert(c).unwrap();
    id
}

#[test]
fn encoding_round_trips_and_ids_are_content_addresses() {
    let alice = agent("alice");
    let c = Contribution::create(ground(), &alice, [], 42, claim("tokens expire"));
    let decoded = Contribution::decode(&c.encode()).unwrap();
    assert_eq!(decoded, c);
    assert!(decoded.signature_is_valid());

    let mut trailing = c.encode();
    trailing.push(0);
    assert!(matches!(Contribution::decode(&trailing), Err(manyroot::contribution::DecodeError::NotCanonical)));

    let other = Contribution::create(ground(), &alice, [], 43, claim("tokens expire"));
    assert_ne!(other.id(), c.id(), "any change to content changes the id");
}

#[test]
fn tampered_content_fails_signature_check() {
    let alice = agent("alice");
    let c = Contribution::create(ground(), &alice, [], 0, claim("original statement"));
    let mut bytes = c.encode();
    let pos = bytes.windows(8).position(|w| w == b"original").unwrap();
    bytes[pos..pos + 8].copy_from_slice(b"modified");
    let forged = Contribution::decode(&bytes).unwrap();
    assert_ne!(forged.id(), c.id());
    assert!(!forged.signature_is_valid());
    assert_eq!(Graph::new(ground()).check(&forged), Err(RuleViolation::BadSignature));
}

#[test]
fn rules_reject_invalid_contributions() {
    let (alice, bob) = (agent("alice"), agent("bob"));
    let mut g = Graph::new(ground());
    let a = add(&mut g, &alice, claim("a"));

    let elsewhere = Contribution::create(GroundId([9; 32]), &alice, [], 0, claim("x"));
    assert!(matches!(g.check(&elsewhere), Err(RuleViolation::WrongGround { .. })));

    let unknown = Contribution::create(ground(), &alice, [], 0, claim("unknown"));
    let dangling = Contribution::create(ground(), &alice, [], 0, evidence(unknown.id(), Verdict::Pass));
    assert_eq!(g.check(&dangling), Err(RuleViolation::MissingParent(unknown.id())));

    let retract = Contribution::create(ground(), &bob, [], 0, Body::Retract { target: a, reason: String::new() });
    assert_eq!(g.check(&retract), Err(RuleViolation::RetractByOther(a)));

    let selfrel = Contribution::create(ground(), &alice, [], 0, relate(a, RelationKind::Refines, a));
    assert_eq!(g.check(&selfrel), Err(RuleViolation::SelfRelation));

    let empty = Contribution::create(ground(), &alice, [], 0, claim("  "));
    assert_eq!(g.check(&empty), Err(RuleViolation::Empty("claim statement")));
}

#[test]
fn replicas_converge_regardless_of_order() {
    let (alice, bob, carol) = (agent("alice"), agent("bob"), agent("carol"));
    let mut base = Graph::new(ground());
    let root = add(&mut base, &alice, claim("we need auth"));

    // Three replicas diverge from the same starting point.
    let (mut r1, mut r2, mut r3) = (base.clone(), base.clone(), base);
    add(&mut r1, &alice, claim("token validation"));
    add(&mut r1, &alice, claim("token refresh"));
    add(&mut r2, &bob, claim("rate limiting"));
    let t = add(&mut r3, &carol, claim("security tests"));
    add(&mut r3, &carol, relate(t, RelationKind::Tests, root));

    // r1 <- r2 <- r3 in one order; r3 <- r1 <- r2 with reversed batches.
    let mut x = r1.clone();
    x.import(r2.missing_from(&x));
    x.import(r3.missing_from(&x));

    let mut y = r3.clone();
    let mut batch = r2.missing_from(&y);
    batch.extend(r1.missing_from(&y));
    batch.reverse();
    let report = y.import(batch);
    assert!(report.is_clean());

    assert_eq!(x.len(), 6);
    assert_eq!(x.topo_order(), y.topo_order(), "same set gives the same causal order");
    assert_eq!(x.heads().collect::<Vec<_>>(), y.heads().collect::<Vec<_>>());
    assert_eq!(x.heads().count(), 3, "three concurrent lines of work");

    // A new contribution on top of all heads merges them.
    add(&mut x, &alice, claim("integrated"));
    assert_eq!(x.heads().count(), 1);
    assert!(x.is_ancestor(&root, &x.heads().next().unwrap()));
}

#[test]
fn import_reports_unresolvable_contributions() {
    let alice = agent("alice");
    let mut source = Graph::new(ground());
    let a = add(&mut source, &alice, claim("a"));
    let b = add(&mut source, &alice, claim("b"));

    let mut target = Graph::new(ground());
    let only_child = source.missing_from(&target).into_iter().filter(|c| c.id() == b);
    let report = target.import(only_child);
    assert!(report.added.is_empty());
    assert_eq!(report.unresolved, vec![b]);
    assert!(!target.contains(&a));
}

#[test]
fn retraction_and_retracting_a_retraction() {
    let alice = agent("alice");
    let mut g = Graph::new(ground());
    let a = add(&mut g, &alice, claim("a"));
    let r = add(&mut g, &alice, Body::Retract { target: a, reason: "wrong".into() });
    assert!(g.is_retracted(&a));
    add(&mut g, &alice, Body::Retract { target: r, reason: "it was right".into() });
    assert!(!g.is_retracted(&a));
}

#[test]
fn a_long_chain_of_retractions_does_not_overflow_the_stack() {
    // Each retraction retracts the one before it. The last one stands, so
    // the claim stands when the chain's length is even.
    const CHAIN: usize = 10_000;
    let alice = agent("alice");
    let mut g = Graph::new(ground());
    let a = add(&mut g, &alice, claim("a"));
    let mut chain = vec![a];
    for _ in 0..CHAIN {
        let last = *chain.last().unwrap();
        chain.push(add(&mut g, &alice, Body::Retract { target: last, reason: String::new() }));
    }
    // A small stack, so the test fails the same way in debug and release
    // builds if the walk ever recurses per link again. Every link is asked
    // about, as `log` does: link i stands when the chain after it is even.
    let walk = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || chain.iter().map(|id| g.is_retracted(id)).collect::<Vec<_>>())
        .unwrap();
    let statuses = walk.join().unwrap();
    for (i, retracted) in statuses.iter().enumerate() {
        assert_eq!(*retracted, (CHAIN - i) % 2 == 1, "link {i}");
    }
}

/// The cost finding 613f0e33fced measured: asking about every link of a
/// 100,000-link chain of self-retractions. Walking each link's retractions
/// costs the square of the chain's length (about 15-20 minutes here); one
/// pass costs its length. Run in release:
/// `cargo test --release -p manyroot --test protocol -- --ignored --nocapture every_link`.
#[test]
#[ignore = "builds 100,000 signed contributions; a timing check, run in release"]
fn every_link_of_a_100k_chain_is_decided_in_linear_time() {
    const CHAIN: usize = 100_000;
    let alice = agent("alice");
    let mut g = Graph::new(ground());
    let mut chain = vec![add(&mut g, &alice, claim("a"))];
    let build = std::time::Instant::now();
    for _ in 0..CHAIN {
        let last = *chain.last().unwrap();
        chain.push(add(&mut g, &alice, Body::Retract { target: last, reason: String::new() }));
    }
    println!("built {CHAIN} links in {:.2?}", build.elapsed());
    let read = std::time::Instant::now();
    let retracted = chain.iter().filter(|id| g.is_retracted(id)).count();
    let elapsed = read.elapsed();
    println!("decided every link in {elapsed:.2?}: {retracted} retracted");
    assert_eq!(retracted, CHAIN / 2);
    assert!(elapsed < std::time::Duration::from_secs(10), "took {elapsed:.2?}");
}

/// The definition, written as directly as possible: retracted if any
/// retraction of it is not itself retracted.
fn retracted_by_definition(g: &Graph, id: &ContributionId) -> bool {
    g.referrers(id).any(|r| {
        matches!(r.body(), Body::Retract { target, .. } if target == id) && !retracted_by_definition(g, &r.id())
    })
}

#[test]
fn retraction_matches_its_definition_on_branching_retractions() {
    let alice = agent("alice");
    // A fixed xorshift sequence, so a failure reproduces.
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    for _ in 0..20 {
        let mut g = Graph::new(ground());
        let mut ids = vec![add(&mut g, &alice, claim("a")), add(&mut g, &alice, claim("b"))];
        // Retractions of random earlier contributions: several retractions of
        // one target, retractions of retractions, and retractions of those.
        for i in 0..40 {
            let target = ids[next(ids.len())];
            ids.push(add_detached(&mut g, &alice, Body::Retract { target, reason: i.to_string() }));
        }
        for id in &ids {
            assert_eq!(g.is_retracted(id), retracted_by_definition(&g, id));
        }
    }
}

#[test]
fn acceptance_requires_independent_evidence() {
    let (alice, bob, carol) = (agent("alice"), agent("bob"), agent("carol"));
    let mut g = Graph::new(ground());
    let work = add(&mut g, &alice, claim("tokens expire after 15 minutes"));
    let policy = Policy::default();
    let status = |g: &Graph, p: &Policy| p.assess(g, &work).unwrap().status;

    add(&mut g, &alice, evidence(work, Verdict::Pass));
    assert_eq!(status(&g, &policy), Status::Pending, "self-validation does not count");

    add(&mut g, &bob, evidence(work, Verdict::Pass));
    assert_eq!(status(&g, &policy), Status::Accepted);

    let strict = Policy { min_passes: 2, ..Policy::default() };
    assert_eq!(status(&g, &strict), Status::Pending);
    add(&mut g, &bob, evidence(work, Verdict::Pass));
    assert_eq!(status(&g, &strict), Status::Pending, "validators count once each");

    let fail = add(&mut g, &carol, evidence(work, Verdict::Fail));
    assert_eq!(status(&g, &policy), Status::Rejected);

    let only_bob = Policy { min_passes: 1, validators: Some([bob.agent_id()].into()), ..Policy::default() };
    assert_eq!(status(&g, &only_bob), Status::Accepted, "carol's evidence is not counted under this policy");

    add(&mut g, &carol, Body::Retract { target: fail, reason: "flaky runner".into() });
    assert_eq!(status(&g, &policy), Status::Accepted);

    assert!(policy.assess(&g, &fail).is_none(), "evidence is not itself assessed");
}

#[test]
fn concurrent_supersessions_are_preserved_as_divergence() {
    let (alice, bob, carol, val) = (agent("alice"), agent("bob"), agent("carol"), agent("validator"));
    let mut g = Graph::new(ground());
    let contract = add(&mut g, &alice, claim("tokens last 30 minutes"));
    add(&mut g, &val, evidence(contract, Verdict::Pass));

    // Bob and Carol each propose a replacement without seeing the other.
    // Their own supersessions don't count: only the contract's author can
    // retire it.
    let short = add_detached(&mut g, &bob, claim("tokens last 15 minutes"));
    let long = add_detached(&mut g, &carol, claim("tokens last 60 minutes"));
    add_detached(&mut g, &bob, relate(short, RelationKind::Supersedes, contract));
    add_detached(&mut g, &carol, relate(long, RelationKind::Supersedes, contract));
    assert!(!g.is_ancestor(&short, &long) && !g.is_ancestor(&long, &short));
    assert!(open_conflicts(&g).is_empty());

    // Alice accepts each, on replicas that haven't seen the other acceptance.
    add_detached(&mut g, &alice, relate(short, RelationKind::Supersedes, contract));
    add_detached(&mut g, &alice, relate(long, RelationKind::Supersedes, contract));

    // Before validation: open conflicts see the fork, accepted view does not.
    let policy = Policy::default();
    let divergence = Conflict::Divergence { target: contract, successors: sorted([short, long]) };
    assert_eq!(open_conflicts(&g), vec![divergence.clone()]);
    assert!(policy.evaluate(&g).conflicts.is_empty());
    assert_eq!(ids(&policy.evaluate(&g)), vec![contract]);

    add(&mut g, &val, evidence(short, Verdict::Pass));
    let e = policy.evaluate(&g);
    assert_eq!(ids(&e), vec![short], "accepted successor replaces the original");
    assert!(e.conflicts.is_empty());

    add(&mut g, &val, evidence(long, Verdict::Pass));
    let e = policy.evaluate(&g);
    assert_eq!(policy.assess(&g, &contract).unwrap().status, Status::Superseded);
    assert_eq!(e.conflicts, vec![divergence.clone()]);

    // A later decision that supersedes both resolves it, once each
    // replacement's author accepts it.
    let decision = add(&mut g, &alice, claim("tokens last 15 minutes; refresh tokens last 60"));
    add(&mut g, &alice, relate(decision, RelationKind::Supersedes, short));
    add(&mut g, &alice, relate(decision, RelationKind::Supersedes, long));
    add(&mut g, &val, evidence(decision, Verdict::Pass));
    assert_eq!(policy.evaluate(&g).conflicts, vec![divergence], "Alice can't retire Bob's and Carol's claims");
    add(&mut g, &bob, relate(decision, RelationKind::Supersedes, short));
    add(&mut g, &carol, relate(decision, RelationKind::Supersedes, long));
    let e = policy.evaluate(&g);
    assert_eq!(ids(&e), vec![decision]);
    assert!(e.conflicts.is_empty());
}

#[test]
fn only_the_targets_author_can_supersede() {
    let (alice, mallory, val) = (agent("alice"), agent("mallory"), agent("validator"));
    let mut g = Graph::new(ground());
    let policy = Policy::default();
    let a = add(&mut g, &alice, claim("a"));
    let b = add(&mut g, &alice, claim("b"));
    let m = add(&mut g, &mallory, claim("m"));
    for x in [a, b, m] {
        add(&mut g, &val, evidence(x, Verdict::Pass));
    }

    // Mallory can't retire Alice's claim, with Alice's replacement or her own.
    add(&mut g, &mallory, relate(b, RelationKind::Supersedes, a));
    add(&mut g, &mallory, relate(m, RelationKind::Supersedes, a));
    assert_eq!(policy.assess(&g, &a).unwrap().status, Status::Accepted);
    assert!(open_supersessors(&g, &a).is_empty());

    // Alice can, with a replacement anyone wrote.
    add(&mut g, &alice, relate(m, RelationKind::Supersedes, a));
    let assessment = policy.assess(&g, &a).unwrap();
    assert_eq!(assessment.status, Status::Superseded);
    assert_eq!(assessment.superseded_by, vec![m]);
    assert_eq!(open_supersessors(&g, &a), vec![m]);
}

#[test]
fn another_authors_supersession_does_not_silence_a_contradiction() {
    // Deferred task 780f658d4b84: a builder "supersedes" the finding against
    // their PR with a claim of their own. The contradiction stays open.
    let (builder, security) = (agent("builder"), agent("security"));
    let mut g = Graph::new(ground());
    let pr = add(&mut g, &builder, claim("PR #1 head"));
    let finding = add(&mut g, &security, claim("main.rs:1 leaks a key"));
    let rel = add(&mut g, &security, relate(finding, RelationKind::Contradicts, pr));
    let contradiction = Conflict::Contradiction { relation: rel, a: finding, b: pr };
    assert_eq!(open_conflicts(&g), vec![contradiction.clone()]);

    let addressed = add(&mut g, &builder, claim("Addressed"));
    add(&mut g, &builder, relate(addressed, RelationKind::Supersedes, finding));
    assert_eq!(open_conflicts(&g), vec![contradiction]);

    // The finding's author withdrawing it in favour of the fix does close it.
    add(&mut g, &security, relate(addressed, RelationKind::Supersedes, finding));
    assert!(open_conflicts(&g).is_empty());
}

#[test]
fn contradictions_between_current_contributions_are_reported() {
    let (alice, bob) = (agent("alice"), agent("bob"));
    let mut g = Graph::new(ground());
    let a = add(&mut g, &alice, claim("clients cache tokens for 60 minutes"));
    let b = add(&mut g, &bob, claim("tokens expire after 15 minutes"));
    let rel = add(&mut g, &bob, relate(b, RelationKind::Contradicts, a));
    assert_eq!(open_conflicts(&g), vec![Conflict::Contradiction { relation: rel, a: b, b: a }]);

    add(&mut g, &alice, Body::Retract { target: a, reason: "fixed client".into() });
    assert!(open_conflicts(&g).is_empty());
}

#[test]
fn store_persists_syncs_and_bundles() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Store::init(&tmp.path().join("a"), "auth").unwrap();
    let mut b = Store::create(&tmp.path().join("b"), a.info().clone()).unwrap();
    let alice = a.create_identity("alice").unwrap();
    let bob = b.create_identity("bob").unwrap();
    assert!(a.create_identity("alice").is_err(), "identities are never overwritten");

    let x = a.contribute(&alice, claim("token validation")).unwrap();
    b.contribute(&bob, claim("rate limiting")).unwrap();
    let artifact =
        Body::Artifact { name: "auth.rs".into(), media_type: "text/x-rust".into(), content: b"fn main() {}".to_vec() };
    b.contribute(&bob, artifact).unwrap();

    let (ra, rb) = store::sync(&mut a, &mut b).unwrap();
    assert_eq!((ra.added.len(), rb.added.len()), (2, 1));
    assert_eq!(a.graph().topo_order(), b.graph().topo_order());

    let reopened = Store::open(&tmp.path().join("a")).unwrap();
    assert_eq!(reopened.graph().topo_order(), a.graph().topo_order());
    assert_eq!(reopened.identity("alice").unwrap().agent_id(), alice.agent_id());

    let mut buf = Vec::new();
    let all = a.graph().missing_from(&Graph::new(a.info().ground));
    bundle::write(&mut buf, a.info().ground, all).unwrap();
    let (gid, contributions) = bundle::read(buf.as_slice()).unwrap();
    assert_eq!(gid, a.info().ground);
    let mut c = Store::create(&tmp.path().join("c"), a.info().clone()).unwrap();
    assert_eq!(c.import(contributions).unwrap().added.len(), 3);
    assert!(c.graph().contains(&x));

    let other = Store::init(&tmp.path().join("other"), "elsewhere").unwrap();
    let mut other = other;
    assert!(matches!(store::sync(&mut a, &mut other), Err(StoreError::DifferentGrounds(..))));
}

#[test]
fn store_detects_tampered_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("g");
    let mut s = Store::init(&dir, "g").unwrap();
    let alice = s.create_identity("alice").unwrap();
    let id = s.contribute(&alice, claim("original statement")).unwrap();
    let hex = id.to_hex();
    let path = dir.join("objects").join(&hex[..2]).join(&hex[2..]);
    let mut bytes = std::fs::read(&path).unwrap();
    let pos = bytes.windows(8).position(|w| w == b"original").unwrap();
    bytes[pos] = b'O';
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(Store::open(&dir), Err(StoreError::Corrupt { .. })));
}

fn ids(e: &Evaluation) -> Vec<ContributionId> {
    e.accepted.iter().map(|a| a.id).collect()
}

fn sorted<const N: usize>(mut ids: [ContributionId; N]) -> Vec<ContributionId> {
    ids.sort();
    ids.to_vec()
}

#[test]
fn a_git_clone_of_a_ground_opens_without_keys_or_empty_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, clone) = (tmp.path().join("origin"), tmp.path().join("clone"));
    let mut s = Store::init(&origin, "g").unwrap();

    // What git would carry for a brand-new Ground: the manifest only.
    std::fs::create_dir_all(&clone).unwrap();
    std::fs::copy(origin.join("ground.json"), clone.join("ground.json")).unwrap();
    let empty = Store::open(&clone).unwrap();
    assert!(empty.graph().is_empty());
    assert!(empty.identities().unwrap().is_empty());

    // After contributions: everything except the private keys.
    let alice = s.create_identity("alice").unwrap();
    let id = s.contribute(&alice, claim("kept in git")).unwrap();
    copy_dir(&origin.join("objects"), &clone.join("objects"));
    copy_dir(&origin.join("agents"), &clone.join("agents"));
    let cloned = Store::open(&clone).unwrap();
    assert!(cloned.graph().contains(&id));
    assert!(matches!(cloned.identity("alice"), Err(StoreError::UnknownIdentity(_))));
    let directory = cloned.agents().unwrap();
    assert_eq!(
        directory,
        vec![AgentEntry { name: "alice".into(), agent: alice.agent_id() }],
        "names survive the clone"
    );
    assert!(matches!(cloned.create_identity("alice"), Err(StoreError::NameTaken(_))));

    // The clone can mint its own identity and keep contributing.
    let mut cloned = cloned;
    let bob = cloned.create_identity("bob").unwrap();
    cloned.contribute(&bob, claim("from the clone")).unwrap();
    assert_eq!(cloned.graph().len(), 2);
    assert_eq!(cloned.agents().unwrap().len(), 2);
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() { copy_dir(&path, &dest) } else { std::fs::copy(&path, &dest).map(|_| ()).unwrap() }
    }
}

#[test]
fn import_reports_and_graph_queries_say_what_they_hold() {
    let alice = agent("alice");
    let mut source = Graph::new(ground());
    let a = add(&mut source, &alice, claim("a"));
    let b = add_detached(&mut source, &alice, claim("b"));
    let c = add(&mut source, &alice, claim("c"));

    // A report is clean only with nothing rejected and nothing unresolved.
    let rejected = ImportReport { rejected: vec![(a, RuleViolation::SelfRelation)], ..Default::default() };
    let unresolved = ImportReport { unresolved: vec![a], ..Default::default() };
    assert!(ImportReport::default().is_clean());
    assert!(!rejected.is_clean() && !unresolved.is_clean());

    // Importing what's already there counts it.
    let mut g = Graph::new(ground());
    assert!(g.is_empty());
    let all = source.missing_from(&g);
    assert_eq!(g.import(all.clone()).added.len(), 3);
    assert!(!g.is_empty());
    let again = g.import(all);
    assert_eq!((again.added.len(), again.already_present), (0, 3));

    // An unrelated contribution with parents is not an ancestor.
    assert!(g.is_ancestor(&a, &c) && !g.is_ancestor(&c, &a));
    let d = add_detached(&mut g, &alice, relate(b, RelationKind::Supports, a));
    assert!(!g.is_ancestor(&c, &d), "d's parents are a and b, not c");
}

#[test]
fn ids_resolve_from_a_prefix_of_at_least_four_hex_characters() {
    let alice = agent("alice");
    let mut g = Graph::new(ground());
    let id = add(&mut g, &alice, claim("a"));
    let hex = id.to_hex();
    for len in [MIN_PREFIX_LEN, 12, 64] {
        assert_eq!(g.resolve_prefix(&hex[..len]).unwrap(), id, "{len} characters");
    }
    assert_eq!(g.resolve_prefix(&hex[..12].to_uppercase()).unwrap(), id, "case doesn't matter");
    let short = g.resolve_prefix(&hex[..MIN_PREFIX_LEN - 1]).unwrap_err();
    assert!(matches!(short.kind, ResolveErrorKind::TooShort), "{short}");
    let not_hex = g.resolve_prefix("zzzzzz").unwrap_err();
    assert!(matches!(not_hex.kind, ResolveErrorKind::NotHex), "{not_hex}");
}
