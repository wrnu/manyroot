//! Lines and decisions under qualification rules 1 to 4, heads and forks, and
//! the ancestor rule (docs/design/weave.md, sections 1 and 2; build step 5).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use bracken_tree::format::{
    Change, Decision, FILE_MEDIA_TYPE, Guardrails, Line, Manifest, Marker, Proposal, Record, VERSION,
};
use bracken_tree::ground::Index;
use bracken_tree::lines::{InvalidProposal, Lines, Unqualified};
use bracken_tree::{ContentHash, Entry, Mode};
use manyroot::{Body, Contribution, ContributionId, Graph, GroundId, Identity, RelationKind};

const GROUND: GroundId = GroundId([5; 32]);

/// A Ground built in memory. Each write lists the given parents, or every head,
/// as a real writer does, and takes its timestamp from `clock`.
struct World {
    g: Graph,
    clock: Box<dyn FnMut() -> u64>,
    alice: Rc<Identity>,
    bob: Rc<Identity>,
    line: ContributionId,
    labels: BTreeMap<ContributionId, &'static str>,
}

impl World {
    /// A marker and a `main` line by alice, its only integrator, and a first tree.
    fn new(clock: impl FnMut() -> u64 + 'static) -> World {
        let mut w = World {
            g: Graph::new(GROUND),
            clock: Box::new(clock),
            alice: Rc::new(who("alice")),
            bob: Rc::new(who("bob")),
            line: ContributionId([0; 32]),
            labels: BTreeMap::new(),
        };
        let alice = w.alice.clone();
        w.record(&alice, &Marker { v: VERSION, purpose: "test".into(), in_place: None });
        w.line = w.record(&alice, &policy(&[&alice]));
        w
    }

    fn heads(&self) -> Vec<ContributionId> {
        self.g.heads().collect()
    }

    fn add_after(&mut self, who: &Identity, parents: Vec<ContributionId>, body: Body) -> ContributionId {
        let ts = (self.clock)();
        let c = Contribution::create(GROUND, who, parents, ts, body);
        let id = c.id();
        self.g.insert(c).expect("a valid contribution");
        id
    }

    fn add(&mut self, who: &Identity, body: Body) -> ContributionId {
        let parents = self.heads();
        self.add_after(who, parents, body)
    }

    fn record<R: Record>(&mut self, who: &Identity, record: &R) -> ContributionId {
        self.add(who, artifact(record))
    }

    /// A one-file tree's file version and manifest; returns the manifest's hash.
    fn tree(&mut self, who: &Identity, bytes: &[u8]) -> ContentHash {
        self.add(who, file(bytes));
        let manifest = manifest(bytes);
        self.record(who, &manifest);
        manifest.content_hash()
    }

    fn supersede(&mut self, who: &Identity, new: ContributionId, old: ContributionId) -> ContributionId {
        self.add(who, Body::Relation { from: new, kind: RelationKind::Supersedes, to: old, note: String::new() })
    }

    /// Writes a decision on the main line after `parents` (every head if `None`),
    /// and labels it.
    fn decide(
        &mut self,
        who: &Identity,
        label: &'static str,
        manifest: ContentHash,
        decisions: &[ContributionId],
        after: Option<Vec<ContributionId>>,
    ) -> ContributionId {
        let d = artifact(&decision(self.line, manifest, decisions.to_vec(), label));
        let parents = after.unwrap_or_else(|| self.heads());
        let id = self.add_after(who, parents, d);
        self.labels.insert(id, label);
        id
    }

    fn lines(&self) -> Lines {
        Lines::build(&self.g, &Index::build(&self.g))
    }

    fn head_labels(&self) -> BTreeSet<&'static str> {
        self.lines().heads(&self.line).iter().map(|id| self.labels[id]).collect()
    }

    fn unqualified(&self, id: &ContributionId) -> Unqualified {
        match self.lines().verdict(id) {
            Some(Err(why)) => why.clone(),
            other => panic!("expected {id} not to qualify, got {other:?}"),
        }
    }
}

fn artifact<R: Record>(record: &R) -> Body {
    Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    }
}

fn file(bytes: &[u8]) -> Body {
    Body::Artifact { name: "a".into(), media_type: FILE_MEDIA_TYPE.into(), content: bytes.to_vec() }
}

fn manifest(bytes: &[u8]) -> Manifest {
    Manifest::new(vec![Entry {
        path: "a".into(),
        mode: Mode::File,
        hash: ContentHash::of(bytes),
        size: bytes.len() as u64,
    }])
}

fn policy(integrators: &[&Identity]) -> Line {
    Line {
        v: VERSION,
        name: "main".into(),
        integrators: integrators.iter().map(|i| i.agent_id()).collect(),
        validators: vec![],
        goal_setters: vec![],
        checks: BTreeMap::new(),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: Some("decision 0123456789ab".into()),
    }
}

fn decision(line: ContributionId, manifest: ContentHash, mut parents: Vec<ContributionId>, note: &str) -> Decision {
    parents.sort();
    Decision {
        v: VERSION,
        line,
        manifest,
        parents,
        proposals: vec![],
        breakage: vec![],
        answers: vec![],
        resolutions: vec![],
        goals: vec![],
        source: None,
        note: note.into(),
    }
}

fn who(name: &str) -> Identity {
    Identity::generate(name).expect("a valid name")
}

fn counter() -> impl FnMut() -> u64 {
    let mut t = 1_700_000_000_000;
    move || {
        t += 1;
        t
    }
}

/// Alice adds bob as an integrator; from `d0`, each moves the line without
/// seeing the other's move, so `d1` and `d2` fork it.
fn fork(clock: impl FnMut() -> u64 + 'static) -> (World, [ContributionId; 3]) {
    let mut w = World::new(clock);
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let both = w.record(&alice, &policy(&[&alice, &bob]));
    w.supersede(&alice, both, w.line);
    let d0 = w.decide(&alice, "d0", m0, &[], None);
    let (m1, m2) = (w.tree(&alice, b"one"), w.tree(&bob, b"two"));
    let at = w.heads();
    let d1 = w.decide(&alice, "d1", m1, &[d0], Some(at.clone()));
    let d2 = w.decide(&bob, "d2", m2, &[d0], Some(at));
    (w, [d0, d1, d2])
}

#[test]
fn two_integrators_fork_a_line_and_a_merge_settles_it() {
    let (mut w, [d0, d1, d2]) = fork(counter());
    let lines = w.lines();
    assert_eq!(lines.default_line(), Some(w.line));
    assert!(matches!(lines.verdict(&d0), Some(Ok(q)) if q.line == w.line));
    let heads = lines.heads(&w.line);
    assert_eq!(
        heads,
        {
            let mut h = vec![d1, d2];
            h.sort();
            h
        },
        "both heads, ascending"
    );
    drop(lines);

    // A decision that lists one head leaves the fork as it is: d2 is still a head.
    let alice = w.alice.clone();
    let m3 = w.tree(&alice, b"three");
    w.decide(&alice, "d3", m3, &[d1], None);
    assert_eq!(w.head_labels(), BTreeSet::from(["d2", "d3"]));

    // A merge lists every head, and settles the fork.
    let heads = w.lines().heads(&w.line);
    let m4 = w.tree(&alice, b"merged");
    w.decide(&alice, "merge", m4, &heads, None);
    assert_eq!(w.head_labels(), BTreeSet::from(["merge"]));
}

#[test]
fn shuffled_timestamps_give_the_same_heads() {
    // Rising, falling, all equal, and scrambled clocks: ids differ, heads don't.
    let falling = {
        let mut t = 2_000_000_000_000u64;
        move || {
            t -= 7;
            t
        }
    };
    let scrambled = {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % 4_000_000_000_000
        }
    };
    let clocks: Vec<Box<dyn FnMut() -> u64>> =
        vec![Box::new(counter()), Box::new(falling), Box::new(|| 0), Box::new(scrambled)];
    for clock in clocks {
        let (w, _) = fork(clock);
        assert_eq!(w.head_labels(), BTreeSet::from(["d1", "d2"]));
        let log: Vec<&str> = w.lines().log(&w.g, &w.line, None).iter().map(|d| w.labels[&d.id]).collect();
        assert_eq!(log.len(), 3);
        assert_eq!(log[2], "d0", "the oldest decision is last: {log:?}");
    }
}

#[test]
fn an_integrator_counts_only_under_a_policy_in_its_past() {
    let mut w = World::new(counter());
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let before_policy = w.heads();
    let both = w.record(&alice, &policy(&[&alice, &bob]));
    w.supersede(&alice, both, w.line);
    // Bob writes without having seen the policy that adds him.
    let early = w.decide(&bob, "early", m0, &[], Some(before_policy));
    assert!(matches!(w.unqualified(&early), Unqualified::NotAnIntegrator { .. }));
    // Having seen it, he counts.
    let late = w.decide(&bob, "late", m0, &[], None);
    assert!(matches!(w.lines().verdict(&late), Some(Ok(q)) if q.policy == both));

    // A version the line's author didn't write, or whose supersedes relation another
    // signed, changes nothing.
    let mut w = World::new(counter());
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let bobs = w.record(&bob, &policy(&[&alice, &bob]));
    w.supersede(&bob, bobs, w.line);
    let alices = w.record(&alice, &policy(&[&alice, &bob]));
    w.supersede(&bob, alices, w.line);
    let d = w.decide(&bob, "d", m0, &[], None);
    assert!(matches!(w.unqualified(&d), Unqualified::NotAnIntegrator { .. }));
    let lines = w.lines();
    assert!(lines.line(&bobs).is_some(), "bob's version is a line of its own, not a version of main");
}

#[test]
fn a_diverged_policy_qualifies_nothing_until_the_author_settles_it() {
    let mut w = World::new(counter());
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let a = w.record(&alice, &policy(&[&alice, &bob]));
    w.supersede(&alice, a, w.line);
    let mut other = policy(&[&alice]);
    other.validators.push(bob.agent_id());
    let b = w.record(&alice, &other);
    w.supersede(&alice, b, w.line);
    let d = w.decide(&alice, "d", m0, &[], None);
    assert!(matches!(w.unqualified(&d), Unqualified::PolicyDiverged(v) if v.len() == 2));
    let settled = w.record(&alice, &policy(&[&alice]));
    w.supersede(&alice, settled, a);
    w.supersede(&alice, settled, b);
    let d = w.decide(&alice, "d2", m0, &[], None);
    assert!(matches!(w.lines().verdict(&d), Some(Ok(q)) if q.policy == settled));
}

#[test]
fn a_reference_that_isnt_an_ancestor_is_ignored() {
    let (mut w, [_, d1, d2]) = fork(counter());
    let alice = w.alice.clone();
    // Written after d2 alone, so d1 isn't in its past, a decision claims d1 as its parent.
    let m = w.tree(&alice, b"x");
    let bad = w.decide(&alice, "bad", m, &[d1], Some(vec![d2]));
    assert_eq!(w.unqualified(&bad), Unqualified::ParentNotAnAncestor(d1));
    assert_eq!(w.head_labels(), BTreeSet::from(["d1", "d2"]), "the fork stands");

    // A decision built on one that doesn't qualify doesn't either.
    let child = w.decide(&alice, "child", m, &[bad], None);
    assert_eq!(w.unqualified(&child), Unqualified::ParentUnqualified(bad));

    // A line that isn't in the decision's past.
    let at = w.heads();
    let other = w.record(&alice, &Line { name: "other".into(), ..policy(&[&alice]) });
    let d = artifact(&decision(other, m, vec![], "elsewhere"));
    let elsewhere = w.add_after(&alice, at, d);
    assert_eq!(w.unqualified(&elsewhere), Unqualified::LineNotAnAncestor(other));

    // A line id that names a policy version, not the line's first artifact.
    let version = w.record(&alice, &policy(&[&alice]));
    w.supersede(&alice, version, w.line);
    let d = w.add(&alice, artifact(&decision(version, m, vec![], "version")));
    assert_eq!(w.unqualified(&d), Unqualified::NotALine(version));

    // weave check lists the references that break the rule.
    let ix = Index::build(&w.g);
    assert!(ix.problems().iter().any(|p| matches!(p,
        bracken_tree::ground::Problem::NotAnAncestor { id, reference } if *id == bad && *reference == d1)));
}

#[test]
fn a_decision_needs_its_manifest_complete_and_stays_unretracted() {
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    // A manifest whose file this replica doesn't hold.
    let lacking = manifest(b"never written");
    w.record(&alice, &lacking);
    let d = w.decide(&alice, "lacking", lacking.content_hash(), &[], None);
    assert!(matches!(w.unqualified(&d), Unqualified::ManifestIncomplete { missing: 1, .. }));
    // No manifest at all.
    let nothing = ContentHash::of(b"no manifest");
    let d = w.decide(&alice, "nothing", nothing, &[], None);
    assert_eq!(w.unqualified(&d), Unqualified::ManifestNotHeld(nothing));
    // A held file version, but not at the size the manifest names (finding 55cf2877cf33).
    w.add(&alice, file(b"one\n"));
    let mut wrong = manifest(b"one\n");
    wrong.entries[0].size = 99;
    w.record(&alice, &wrong);
    let d = w.decide(&alice, "wrong size", wrong.content_hash(), &[], None);
    assert!(matches!(w.unqualified(&d), Unqualified::ManifestIncomplete { missing: 1, .. }));
    let ix = Index::build(&w.g);
    assert_eq!(ix.missing(&wrong), ["a"]);
    assert!(ix.problems().iter().any(|p| matches!(p,
        bracken_tree::ground::Problem::Incomplete { hash, .. } if *hash == wrong.content_hash())));
    // Once the file arrives, the first qualifies: completeness converges on sync.
    w.add(&alice, file(b"never written"));
    let first = *w.labels.iter().find(|(_, l)| **l == "lacking").expect("labelled").0;
    assert!(matches!(w.lines().verdict(&first), Some(Ok(_))));
    assert_eq!(w.head_labels(), BTreeSet::from(["lacking"]));

    // Retracting it removes it from the line.
    w.add(&alice, Body::Retract { target: first, reason: "wrong".into() });
    assert_eq!(w.unqualified(&first), Unqualified::Retracted);
    assert!(w.head_labels().is_empty());
}

#[test]
fn log_is_newest_first_and_since_leaves_out_the_past() {
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let mut previous = vec![];
    let mut ids = vec![];
    for label in ["a", "b", "c", "d"] {
        let m = w.tree(&alice, label.as_bytes());
        let id = w.decide(&alice, label, m, &previous, None);
        previous = vec![id];
        ids.push(id);
    }
    let lines = w.lines();
    let log: Vec<&str> = lines.log(&w.g, &w.line, None).iter().map(|d| w.labels[&d.id]).collect();
    assert_eq!(log, ["d", "c", "b", "a"]);
    let since: Vec<&str> = lines.log(&w.g, &w.line, Some(&ids[1])).iter().map(|d| w.labels[&d.id]).collect();
    assert_eq!(since, ["d", "c"]);
}

#[test]
fn the_default_line_is_main_by_the_markers_author() {
    let mut w = World::new(counter());
    let bob = w.bob.clone();
    // Bob's line named main isn't the default; alice wrote the marker.
    let bobs = w.record(&bob, &policy(&[&bob]));
    let lines = w.lines();
    assert_eq!(lines.default_line(), Some(w.line));
    assert!(lines.line(&bobs).is_some());
    assert_eq!(lines.lines().count(), 2);
}

#[test]
fn a_line_stays_a_line_for_decisions_made_before_it_superseded_a_version() {
    // Finding cfe7d808b1d5: whether a decision's line is a first artifact is judged among
    // its ancestors, so a later supersedes relation from the line changes no old decision.
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let d0 = w.decide(&alice, "d0", m0, &[], None);
    let other = w.record(&alice, &Line { name: "other".into(), ..policy(&[&alice]) });
    w.supersede(&alice, w.line, other);
    let lines = w.lines();
    assert!(matches!(lines.verdict(&d0), Some(Ok(q)) if q.line == w.line));
    assert_eq!(lines.heads(&w.line), [d0]);
    assert!(lines.line(&w.line).is_some(), "still a line, for its decisions");
    // It now supersedes a version, so it is no longer a current line: not the default, and
    // not found by name (finding fdd17cbdc816).
    assert_eq!(lines.default_line(), None);
    assert!(lines.lines().all(|v| v.id != w.line));
    drop(lines);
    // A decision that sees the relation doesn't count the line as one.
    let d1 = w.decide(&alice, "d1", m0, &[d0], None);
    assert_eq!(w.unqualified(&d1), Unqualified::NotALine(w.line));
}

#[test]
fn a_marker_written_after_the_first_cant_take_the_default_line() {
    // Finding d8168e1a9ebb, decision 049d02449a94.
    let mut w = World::new(counter());
    let eve = Rc::new(who("eve"));
    let first = Index::build(&w.g).counted_marker().expect("alice's marker");
    // Eve, having seen alice's marker, writes markers until one has a smaller id.
    let mut n = 0;
    let eves = loop {
        n += 1;
        let id = w.record(&eve, &Marker { v: VERSION, purpose: format!("mine {n}"), in_place: None });
        if id < first {
            break id;
        }
    };
    let eves_main = w.record(&eve, &policy(&[&eve]));
    let ix = Index::build(&w.g);
    assert_eq!(ix.counted_marker(), Some(first), "the causally first marker counts, not the smallest id");
    assert!(ix.problems().iter().any(|p| matches!(p,
        bracken_tree::ground::Problem::ExtraMarker { id, counted } if *id == eves && *counted == first)));
    let lines = w.lines();
    assert_eq!(lines.default_line(), Some(w.line));
    assert!(lines.line(&eves_main).is_some());

    // Concurrent markers: neither is in the other's past, so the smallest id counts.
    let mut g = Graph::new(GROUND);
    let a = Contribution::create(
        GROUND,
        &w.alice,
        [],
        1,
        artifact(&Marker { v: VERSION, purpose: "a".into(), in_place: None }),
    );
    let b = Contribution::create(
        GROUND,
        &eve,
        [],
        2,
        artifact(&Marker { v: VERSION, purpose: "b".into(), in_place: None }),
    );
    let smaller = a.id().min(b.id());
    g.insert(a).expect("valid");
    g.insert(b).expect("valid");
    let ix = Index::build(&g);
    assert_eq!(ix.counted_marker(), Some(smaller));
    assert_eq!(
        ix.problems().iter().filter(|p| matches!(p, bracken_tree::ground::Problem::ExtraMarker { .. })).count(),
        1
    );
}

#[test]
fn a_version_used_in_the_window_before_its_relation_doesnt_stay_the_default() {
    // Finding fdd17cbdc816: alice writes a version of main and, as a separate write, the
    // relation that chains it. A decision in between names the version as its line.
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let d0 = w.decide(&alice, "d0", m0, &[], None);
    let version = w.record(&alice, &policy(&[&alice]));
    let d = artifact(&decision(version, m0, vec![], "in the window"));
    let d1 = w.add(&alice, d);
    w.supersede(&alice, version, w.line);

    let lines = w.lines();
    assert!(matches!(lines.verdict(&d1), Some(Ok(q)) if q.line == version), "its judgement stands");
    assert!(lines.line(&version).is_some(), "its decisions can still be read by its id");
    assert_eq!(lines.heads(&version), [d1]);
    assert_eq!(lines.default_line(), Some(w.line), "the default goes back to main's first artifact");
    let mains: Vec<_> = lines.lines().filter(|v| v.policy.name == "main").map(|v| v.id).collect();
    assert_eq!(mains, [w.line], "one current line is named main");
    assert_eq!(lines.heads(&w.line), [d0]);
}

#[test]
fn merge_bases_are_the_newest_common_decisions_and_a_criss_cross_has_two() {
    let (mut w, [d0, d1, d2]) = fork(counter());
    let alice = w.alice.clone();
    let lines = w.lines();
    assert_eq!(lines.merge_bases(&d1, &d0), [d0], "an ancestor is the base");
    assert_eq!(lines.merge_bases(&d1, &d2), [d0], "a fork's base is where it forked");
    assert_eq!(lines.merge_bases(&d1, &d1), [d1]);
    drop(lines);
    // Two merges of the same two heads, each listing both: a criss-cross.
    let m = w.tree(&alice, b"merged");
    let at = w.heads();
    let m1 = w.decide(&alice, "m1", m, &[d1, d2], Some(at.clone()));
    let m2 = w.decide(&alice, "m2", m, &[d1, d2], Some(at));
    let mut both = vec![d1, d2];
    both.sort();
    assert_eq!(w.lines().merge_bases(&m1, &m2), both);
    // A root of its own shares no history.
    let other = w.decide(&alice, "other root", m, &[], None);
    assert!(w.lines().merge_bases(&other, &d1).is_empty());
}

fn proposal(line: ContributionId, base: ContributionId, manifest: &Manifest, change: ContentHash) -> Proposal {
    Proposal { v: VERSION, line, base, manifest: manifest.content_hash(), change, summary: "p".into() }
}

#[test]
fn a_proposal_is_valid_only_with_its_change_recomputed_from_a_qualifying_base() {
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let d0 = w.decide(&alice, "d0", m0, &[], None);
    let proposed = manifest(b"one");
    w.tree(&alice, b"one");
    let change = Change::between(&manifest(b"zero"), &proposed).hash();
    let good = w.record(&alice, &proposal(w.line, d0, &proposed, change));
    let wrong = w.record(&alice, &proposal(w.line, d0, &proposed, ContentHash::of(b"another change")));
    let lines = w.lines();
    assert_eq!(lines.proposal(&good).expect("decoded").verdict, Ok(()));
    assert!(matches!(lines.proposal(&wrong).expect("decoded").verdict, Err(InvalidProposal::Change { .. })));
    assert_eq!(lines.proposals_on(&w.line).map(|p| p.id).collect::<Vec<_>>(), [good]);
    drop(lines);

    // A base that isn't a qualifying decision, or isn't in the proposal's past.
    let stray = w.decide(&w.bob.clone(), "bob's", m0, &[], None);
    let not_qualifying = w.record(&alice, &proposal(w.line, stray, &proposed, change));
    let at = w.heads();
    let later = w.decide(&alice, "later", m0, &[d0], None);
    let p = artifact(&proposal(w.line, later, &proposed, change));
    let not_before = w.add_after(&alice, at, p);
    let lines = w.lines();
    for id in [not_qualifying, not_before] {
        assert!(matches!(lines.proposal(&id).expect("decoded").verdict, Err(InvalidProposal::Base(_))), "{id}");
    }
}

#[test]
fn a_policy_version_without_an_owner_decision_counts_only_as_weave_inits_main() {
    // Finding 12643881ac59: a version that cites no owner decision is ignored, except a first artifact
    // named main by the counted marker's author, which is what weave init writes.
    let mut w = World::new(counter());
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let uncited = |name: &str, who: &[&Identity]| Line { owner_decision: None, name: name.into(), ..policy(who) };

    // A version adding bob, with no owner decision, changes nothing.
    let version = w.record(&alice, &uncited("main", &[&alice, &bob]));
    w.supersede(&alice, version, w.line);
    let d = w.decide(&bob, "bob's", m0, &[], None);
    assert!(matches!(w.unqualified(&d), Unqualified::NotAnIntegrator { policy, .. } if policy == w.line));
    // Nor does bob's own uncited line, or alice's uncited line by another name.
    let bobs = w.record(&bob, &uncited("main", &[&bob]));
    let other = w.record(&alice, &uncited("other", &[&alice]));
    let lines = w.lines();
    for id in [version, bobs, other] {
        assert!(lines.version(&id).is_none() && lines.ignored_versions().contains(&id), "{id}");
    }
    assert_eq!(lines.ignored_versions().len(), 3);
    drop(lines);

    // weave init's main itself, uncited, counts.
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let main = w.record(&alice, &uncited("main", &[&alice]));
    let lines = w.lines();
    assert!(lines.line(&main).is_some() && !lines.ignored_versions().contains(&main));
}
