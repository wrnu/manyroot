//! Lines and decisions under qualification rules 1 to 4, heads and forks, and
//! the ancestor rule (docs/design/weave.md, sections 1 and 2; build step 5).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use bracken_tree::format::{
    Change, DECISION_MEDIA_TYPE, Decision, FILE_MEDIA_TYPE, Guardrails, LINE_MEDIA_TYPE, Line, Manifest, Marker,
    Proposal, Record, VERSION,
};
use bracken_tree::ground::Index;
use bracken_tree::lines::{DIVERGED_SHOWN, InvalidProposal, Lines, Unqualified};
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
        checked(&self.g)
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

/// Lines built from the index, checked against Lines built by walking the graph for every question: the
/// same verdict for every decision and proposal, and no question missed (answer b2dbc4f5e810).
fn checked(g: &Graph) -> Lines {
    let ix = Index::build(g);
    let lines = Lines::build(g, &ix);
    let walked = Lines::build_by_walking(g, &ix);
    assert!(lines.objections().differ_from_walking(&ix, &lines).is_empty(), "windows and a walk disagree");
    for c in g.iter() {
        let id = c.id();
        assert_eq!(lines.verdict(&id), walked.verdict(&id), "decision {id}");
        let verdict = |l: &Lines| l.proposal(&id).map(|p| p.verdict.clone());
        assert_eq!(verdict(&lines), verdict(&walked), "proposal {id}");
    }
    assert_eq!(ix.ancestry_stats().fallbacks, 0, "{:?}", ix.ancestry_stats());
    assert_eq!(lines.stats().fallbacks, 0, "{:?}", lines.stats());
    assert!(lines.stats().peak_words <= g.len().max(1), "{:?}", lines.stats());
    lines
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
    assert!(matches!(w.unqualified(&d), Unqualified::PolicyDiverged { versions, count: 2 } if versions.len() == 2));
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

// The ancestor index's shapes (design note e93d47ad8a20): each is checked against walking the graph, with no
// question missed, by `checked`.

/// A deterministic random number source.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

impl World {
    /// `n` more versions of the main line, each superseding the one before; returns them, the line first.
    fn chain(&mut self, who: &Identity, n: usize) -> Vec<ContributionId> {
        let mut versions = vec![self.line];
        for _ in 0..n {
            let next = self.record(who, &policy(&[who]));
            self.supersede(who, next, *versions.last().expect("a version"));
            versions.push(next);
        }
        versions
    }

    /// `n` decisions naming `line`, each after every head.
    fn decide_on(
        &mut self,
        who: &Identity,
        line: ContributionId,
        manifest: ContentHash,
        n: usize,
    ) -> Vec<ContributionId> {
        (0..n).map(|_| self.record(who, &decision(line, manifest, vec![], "d"))).collect()
    }
}

/// Finding 83f757301c59: supersedes relations that someone other than the author signs never become
/// questions, as Lines never counts them.
#[test]
fn only_the_authors_supersedes_relations_are_asked_about() {
    let mut w = World::new(counter());
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&alice, b"zero");
    let versions = w.chain(&alice, 1);
    w.decide(&alice, "d", m0, &[], None);
    let asked = w.lines().stats();
    for _ in 0..50 {
        w.supersede(&bob, versions[1], w.line);
    }
    w.decide(&alice, "e", m0, &[], None);
    let with_bobs = w.lines().stats();
    assert_eq!(with_bobs.list_entries, asked.list_entries, "bob's 50 relations aren't stored");
    // The second decision asks what the first does: alice's one relation.
    assert_eq!(with_bobs.questions, asked.questions + 1, "{asked:?} then {with_bobs:?}");
    assert_kept(&w.lines(), 1, 2);
}

/// What Lines' chain questions hold until its records are judged (findings 1cdb8b4dcac8 and 09c403532fc9):
/// 32 bytes a relation in the lists, 128 for the vectors themselves (these shapes have at most two lists),
/// and 184 bytes an asker for its two entries and its verdict, by their types' sizes on a 64-bit target.
/// Keeping an asker's answers after its verdict would add K / 8 bytes an asker, 38 at K = 300, and a list
/// per asker 32 K: either breaks it.
fn assert_kept(lines: &Lines, relations: usize, askers: usize) {
    let stats = lines.stats();
    let bound = 32 * relations + 184 * askers + 128;
    assert!(stats.bytes > 0 && stats.bytes <= bound, "{relations} relations, {askers} askers: {stats:?}");
}

/// Builds the lines without `checked`, whose walk per question takes most of a minute on the larger
/// shapes; still with no question missed.
fn unchecked(g: &Graph) -> Lines {
    let ix = Index::build(g);
    let lines = Lines::build(g, &ix);
    assert_eq!((lines.stats().fallbacks, ix.ancestry_stats().fallbacks), (0, 0));
    assert!(lines.objections().differ_from_walking(&ix, &lines).is_empty(), "windows and a walk disagree");
    lines
}

/// Finding 98ce07a5e421: a line's author makes a long chain and many decisions on it. Written after the
/// chain, the decisions share one past, so one answer vector: one round of ceil(K / 64) passes.
#[test]
fn many_decisions_after_a_long_chain_share_one_answer_vector() {
    const K: usize = 300;
    const D: usize = 300;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let versions = w.chain(&alice, K);
    let decisions = w.decide_on(&alice, w.line, m0, D);
    let lines = unchecked(&w.g);
    let stats = lines.stats();
    assert_eq!((stats.list_entries, stats.questions), (K, D * K), "{stats:?}");
    // Each decision's past adds the one before, which no relation is: the same class.
    assert_eq!((stats.vectors, stats.peak_words, stats.passes), (1, K.div_ceil(64), K.div_ceil(64)), "{stats:?}");
    assert_kept(&lines, K, D);
    for d in &decisions {
        assert!(matches!(lines.verdict(d), Some(Ok(q)) if q.policy == versions[K]), "{d}");
    }
}

/// The batch budget (answer b2dbc4f5e810): a decision after each new version of a long chain, so every
/// past differs. The answers are held a round at a time, within a word per contribution, and a round's
/// passes reach only the relations written before its last decision. Checked against the walk (finding
/// 09c403532fc9), so a little smaller than the other long chains.
#[test]
fn a_decision_after_each_version_is_answered_in_rounds_within_a_word_per_contribution() {
    const K: usize = 200;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let (mut newest, mut decisions) = (w.line, vec![]);
    for _ in 0..K {
        let next = w.record(&alice, &policy(&[&alice]));
        w.supersede(&alice, next, newest);
        newest = next;
        decisions.push(w.record(&alice, &decision(w.line, m0, vec![], "d")));
    }
    let lines = w.lines();
    let stats = lines.stats();
    let n = w.g.len();
    assert!(K * K.div_ceil(64) > n, "more than one round");
    assert_eq!((stats.vectors, stats.questions), (K, K * K), "{stats:?}");
    // ceil(K / 64) words a vector and n words a round, each round reaching the chunks
    // its last decision has in its past.
    let per_round = n / K.div_ceil(64);
    assert_eq!(stats.peak_words, per_round * K.div_ceil(64), "{stats:?}");
    let reached: usize = (0..K.div_ceil(per_round)).map(|r| ((r + 1) * per_round).min(K).div_ceil(64)).sum();
    assert_eq!(stats.passes, reached, "{stats:?}");
    assert_kept(&lines, K, K);
    let qualified = decisions.iter().filter(|d| matches!(lines.verdict(d), Some(Ok(_)))).count();
    assert_eq!(qualified, K);
}

/// Finding 7c1c3b1fea29: a decision naming each version of a chain. Only the first artifact's qualify; each
/// relation is stored once, however many versions are named.
#[test]
fn decisions_naming_each_version_of_a_chain_store_it_once() {
    const L: usize = 300;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let versions = w.chain(&alice, L);
    let decisions: Vec<ContributionId> =
        versions.iter().map(|v| w.record(&alice, &decision(*v, m0, vec![], "d"))).collect();
    let lines = w.lines();
    assert_eq!(lines.stats().list_entries, L, "{:?}", lines.stats());
    assert_kept(&lines, L, L + 1);
    assert!(matches!(lines.verdict(&decisions[0]), Some(Ok(q)) if q.policy == versions[L]));
    for (d, v) in decisions[1..].iter().zip(&versions[1..]) {
        assert_eq!(lines.verdict(d), Some(&Err(Unqualified::NotALine(*v))), "{d}");
    }
}

/// Finding a5ac20689b4b, case 2: decisions made before their line superseded a version still find their
/// policy from the whole component, with nothing walked; one made after doesn't count the line.
#[test]
fn decisions_before_their_line_superseded_a_version_ask_the_whole_chain() {
    const L: usize = 200;
    const D: usize = 50;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let versions = w.chain(&alice, L);
    let decisions = w.decide_on(&alice, w.line, m0, D);
    let y = w.record(&alice, &Line { name: "y".into(), ..policy(&[&alice]) });
    w.supersede(&alice, w.line, y);
    let after = w.record(&alice, &decision(w.line, m0, vec![], "after"));
    let lines = w.lines();
    for d in &decisions {
        assert!(matches!(lines.verdict(d), Some(Ok(q)) if q.policy == versions[L]), "{d}");
    }
    assert_eq!(lines.verdict(&after), Some(&Err(Unqualified::NotALine(w.line))));
    assert_kept(&lines, L + 1, D + 1);
}

/// Finding a5ac20689b4b, case 1: the line supersedes a version Lines drops (uncited, retracted, or
/// undecodable). Lines builds its questions after dropping it, so the line is a first artifact and its
/// decisions ask about its chain, with nothing walked.
#[test]
fn a_line_superseding_a_dropped_version_asks_the_whole_chain() {
    const L: usize = 100;
    const D: usize = 30;
    for how in ["uncited", "retracted", "undecodable"] {
        let mut w = World::new(counter());
        let alice = w.alice.clone();
        let m0 = w.tree(&alice, b"zero");
        let y = match how {
            "uncited" => w.record(&alice, &Line { owner_decision: None, name: "y".into(), ..policy(&[&alice]) }),
            "retracted" => {
                let y = w.record(&alice, &Line { name: "y".into(), ..policy(&[&alice]) });
                w.add(&alice, Body::Retract { target: y, reason: "wrong".into() });
                y
            }
            _ => w.add(
                &alice,
                Body::Artifact { name: "y".into(), media_type: LINE_MEDIA_TYPE.into(), content: b"no cbor".to_vec() },
            ),
        };
        w.supersede(&alice, w.line, y);
        let versions = w.chain(&alice, L);
        let decisions = w.decide_on(&alice, w.line, m0, D);
        let lines = w.lines();
        assert_eq!(lines.stats().questions, D * L, "{how}: {:?}", lines.stats());
        assert_kept(&lines, L, D);
        for d in &decisions {
            assert!(matches!(lines.verdict(d), Some(Ok(q)) if q.policy == versions[L]), "{how}: {d}");
        }
    }
}

/// Many markers: one target, a bit each, and the first counts.
#[test]
fn many_markers_are_answered_without_a_walk() {
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let first = Index::build(&w.g).counted_marker().expect("alice's marker");
    for _ in 0..200 {
        w.record(&alice, &Marker { v: VERSION, purpose: "test".into(), in_place: None });
    }
    let ix = Index::build(&w.g);
    assert_eq!(ix.counted_marker(), Some(first));
    assert_eq!(ix.ancestry_stats().fallbacks, 0);
    assert_eq!(ix.problems().len(), 200, "the rest are extra");
    w.lines();
}

/// Random Grounds of lines, versions by two authors, cited and not, relations by either, retractions,
/// concurrent decisions and proposals: every verdict is the one walking the graph gives, with nothing
/// walked (answer b2dbc4f5e810).
#[test]
fn lines_agree_with_walking_the_graph() {
    for seed in 0..4 {
        let mut rng = Lcg(seed);
        let mut w = World::new(counter());
        let (alice, bob) = (w.alice.clone(), w.bob.clone());
        let m0 = w.tree(&alice, b"zero");
        let proposed = manifest(b"one");
        w.tree(&alice, b"one");
        let change = Change::between(&manifest(b"zero"), &proposed).hash();
        let mut all: Vec<ContributionId> = w.g.iter().map(|c| c.id()).collect();
        let mut versions = vec![w.line];
        let mut decisions = vec![];
        let mut relations = vec![];
        let mut authors: std::collections::HashMap<ContributionId, Rc<Identity>> =
            all.iter().map(|id| (*id, alice.clone())).collect();
        for _ in 0..300 {
            let mut who = if rng.below(4) == 0 { bob.clone() } else { alice.clone() };
            // After a few random contributions, so writes are often concurrent.
            let after: Vec<ContributionId> =
                (0..1 + rng.below(3)).map(|_| all[all.len() - 1 - rng.below(all.len().min(12))]).collect();
            let body = match rng.below(10) {
                0..=1 => {
                    let mut line = policy(&[&alice, &bob]);
                    if rng.below(5) == 0 {
                        line.owner_decision = None;
                    }
                    artifact(&line)
                }
                2..=3 if versions.len() > 1 => {
                    let (new, old) = (versions[rng.below(versions.len())], versions[rng.below(versions.len())]);
                    if new == old {
                        continue;
                    }
                    Body::Relation { from: new, kind: RelationKind::Supersedes, to: old, note: String::new() }
                }
                4 if rng.below(3) == 0 => {
                    let pool = [&versions, &relations, &decisions][rng.below(3)];
                    match pool.is_empty() {
                        true => continue,
                        false => {
                            let target = pool[rng.below(pool.len())];
                            who = authors[&target].clone();
                            Body::Retract { target, reason: "r".into() }
                        }
                    }
                }
                5 if !decisions.is_empty() => {
                    let base = decisions[rng.below(decisions.len())];
                    artifact(&proposal(versions[rng.below(versions.len())], base, &proposed, change))
                }
                _ => {
                    // Often on the main line with no parents, so that some qualify.
                    let parents = match decisions.is_empty() || rng.below(2) == 0 {
                        true => vec![],
                        false => vec![decisions[rng.below(decisions.len())]],
                    };
                    let line = if rng.below(2) == 0 { w.line } else { versions[rng.below(versions.len())] };
                    artifact(&decision(line, m0, parents, "d"))
                }
            };
            let kind = match &body {
                Body::Artifact { media_type, .. } if media_type == LINE_MEDIA_TYPE => 0,
                Body::Relation { .. } => 1,
                Body::Artifact { media_type, .. } if media_type == DECISION_MEDIA_TYPE => 2,
                _ => 3,
            };
            let id = w.add_after(&who, after, body);
            authors.insert(id, who);
            match kind {
                0 => versions.push(id),
                1 => relations.push(id),
                2 => decisions.push(id),
                _ => {}
            }
            all.push(id);
        }
        let lines = w.lines();
        let qualified = decisions.iter().filter(|d| matches!(lines.verdict(d), Some(Ok(_)))).count();
        assert!(qualified > 10 && qualified < decisions.len(), "seed {seed}: {qualified} of {}", decisions.len());
    }
}

/// Task a90865cb60db: K versions superseding the line, none another, so its policy diverges into K; D
/// decisions written after them all find the same K. They share one list of the first DIVERGED_SHOWN,
/// with the count, so what Lines keeps is linear in D + K, not D x K; the line's author still sees all K.
#[test]
fn a_diverged_policy_is_kept_once_however_many_decisions_find_it() {
    const K: usize = 1_000;
    const D: usize = 1_000;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    for _ in 0..K {
        let version = w.record(&alice, &policy(&[&alice]));
        w.supersede(&alice, version, w.line);
    }
    let decisions = w.decide_on(&alice, w.line, m0, D);
    let lines = unchecked(&w.g);
    assert_eq!(lines.current_versions(&w.line).len(), K);
    let shown = |d: &ContributionId| match lines.verdict(d) {
        Some(Err(Unqualified::PolicyDiverged { versions, count })) => (versions.clone(), *count),
        other => panic!("{d}: {other:?}"),
    };
    let (first, count) = shown(&decisions[0]);
    assert_eq!((first.len(), count), (DIVERGED_SHOWN, K));
    assert_eq!(&first[..], &lines.current_versions(&w.line)[..DIVERGED_SHOWN]);
    for d in &decisions[1..] {
        assert!(Arc::ptr_eq(&shown(d).0, &first), "{d} shares the first decision's list");
    }
    // The one shared list: 8 versions, a pointer and two counts.
    assert_kept(&lines, K, D);
    assert!(lines.stats().bytes <= 32 * K + 184 * D + 128 + 8 * 32 + 32, "{:?}", lines.stats());
}

/// Task a90865cb60db: a version superseding the line, then a decision, repeated, so each decision finds one
/// more diverged version than the one before, a different list each time. Each keeps at most
/// DIVERGED_SHOWN, so D decisions keep at most D x DIVERGED_SHOWN versions, not D x K / 2.
#[test]
fn every_decision_finding_a_different_divergence_keeps_a_bounded_list() {
    const K: usize = 150;
    let mut w = World::new(counter());
    let alice = w.alice.clone();
    let m0 = w.tree(&alice, b"zero");
    let mut decisions = vec![];
    for _ in 0..K {
        let version = w.record(&alice, &policy(&[&alice]));
        w.supersede(&alice, version, w.line);
        decisions.push(w.record(&alice, &decision(w.line, m0, vec![], "d")));
    }
    let lines = w.lines();
    for (i, d) in decisions.iter().enumerate() {
        match lines.verdict(d) {
            Some(Ok(q)) => assert_eq!(i, 0, "only the first, after one version, qualifies: {q:?}"),
            Some(Err(Unqualified::PolicyDiverged { versions, count })) => {
                assert_eq!((versions.len(), *count), ((i + 1).min(DIVERGED_SHOWN), i + 1), "{d}")
            }
            other => panic!("{d}: {other:?}"),
        }
    }
    let lists = K - 1;
    assert!(lines.stats().bytes <= 32 * K + 184 * K + 128 + lists * (DIVERGED_SHOWN * 32 + 32), "{:?}", lines.stats());
}
