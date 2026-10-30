//! Qualification rules 5 and 6, and open objections (docs/design/weave.md, section 1
//! *Goals, checks and objections*, and section 2; build step 7). Every judgement is
//! also made by walking the graph, and must agree; and no ancestor question falls
//! back to a walk (decision on split note dc467163233c).

use std::collections::BTreeMap;
use std::rc::Rc;

use bracken_tree::check::{Exit, RecordedTail};
use bracken_tree::format::{
    Answer, Breakage, Change, CheckDefinition, Decision, Disposition, FILE_MEDIA_TYPE, GOAL_TAG, Guardrails, Line,
    Manifest, Marker, Proposal, Record, VERSION,
};
use bracken_tree::ground::Index;
use bracken_tree::lines::{Lines, Unqualified};
use bracken_tree::objections::Basis;
use bracken_tree::results::details;
use bracken_tree::{ContentHash, Entry, Mode};
use manyroot::{Body, Contribution, ContributionId, Graph, GroundId, Identity, RelationKind, Verdict};

const GROUND: GroundId = GroundId([7; 32]);
const ARGV: &[&str] = &["cargo", "test"];

type Files<'a> = &'a [(&'a str, &'a [u8])];

/// A Weave Ground in memory: alice's marker and `main` line. alice integrates and sets goals; bob and
/// carol validate; `test` is the one check; breakage allows two entries, two moves old.
struct World {
    g: Graph,
    clock: u64,
    alice: Rc<Identity>,
    bob: Rc<Identity>,
    carol: Rc<Identity>,
    line: ContributionId,
}

fn policy(w: &World, goal_setters: Vec<manyroot::AgentId>, guardrails: Guardrails) -> Line {
    Line {
        v: VERSION,
        name: "main".into(),
        integrators: vec![w.alice.agent_id()],
        validators: vec![w.bob.agent_id(), w.carol.agent_id()],
        goal_setters,
        checks: BTreeMap::from([("test".to_owned(), ARGV.iter().map(|a| a.to_string()).collect())]),
        guardrails,
        owner_decision: Some("decision 0123456789ab".into()),
    }
}

fn rails(max_known: u64, max_age: u64, protected: &[&str]) -> Guardrails {
    Guardrails { max_known, max_age, protected: protected.iter().map(|p| p.to_string()).collect() }
}

fn artifact<R: Record>(record: &R) -> Body {
    Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    }
}

fn manifest(files: Files) -> Manifest {
    Manifest::new(
        files
            .iter()
            .map(|(path, bytes)| Entry {
                path: (*path).into(),
                mode: Mode::File,
                hash: ContentHash::of(bytes),
                size: bytes.len() as u64,
            })
            .collect(),
    )
}

fn breakage(check: &str, paths: &[&str]) -> Breakage {
    Breakage { check: check.into(), detail: "known".into(), paths: paths.iter().map(|p| p.to_string()).collect() }
}

fn answer(objection: ContributionId, disposition: Disposition) -> Answer {
    Answer { objection, disposition, reason: "because".into() }
}

impl World {
    fn new() -> World {
        let who = |name: &str| Rc::new(Identity::generate(name).expect("a valid name"));
        let mut w = World {
            g: Graph::new(GROUND),
            clock: 1_700_000_000_000,
            alice: who("alice"),
            bob: who("bob"),
            carol: who("carol"),
            line: ContributionId([0; 32]),
        };
        let alice = w.alice.clone();
        w.add(&alice, artifact(&Marker { v: VERSION, purpose: "test".into(), in_place: None }));
        let line = policy(&w, vec![alice.agent_id()], rails(2, 2, &["secrets"]));
        w.line = w.add(&alice, artifact(&line));
        w
    }

    fn add_after(&mut self, who: &Identity, parents: Vec<ContributionId>, body: Body) -> ContributionId {
        self.clock += 1;
        let c = Contribution::create(GROUND, who, parents, self.clock, body);
        let id = c.id();
        self.g.insert(c).expect("a valid contribution");
        id
    }

    fn add(&mut self, who: &Identity, body: Body) -> ContributionId {
        let heads = self.g.heads().collect();
        self.add_after(who, heads, body)
    }

    /// A tree's file versions and manifest artifact: the manifest's hash and its artifact.
    fn tree(&mut self, files: Files) -> (ContentHash, ContributionId) {
        let alice = self.alice.clone();
        for (_, bytes) in files {
            let body = Body::Artifact { name: "f".into(), media_type: FILE_MEDIA_TYPE.into(), content: bytes.to_vec() };
            self.add(&alice, body);
        }
        let m = manifest(files);
        (m.content_hash(), self.add(&alice, artifact(&m)))
    }

    /// A decision by alice on the line, shaped by `with`.
    fn decide(
        &mut self,
        manifest: ContentHash,
        parents: &[ContributionId],
        with: impl FnOnce(&mut Decision),
    ) -> ContributionId {
        let alice = self.alice.clone();
        let d = self.draft(manifest, parents, with);
        self.add(&alice, artifact(&d))
    }

    fn draft(&self, manifest: ContentHash, parents: &[ContributionId], with: impl FnOnce(&mut Decision)) -> Decision {
        let mut parents = parents.to_vec();
        parents.sort();
        let mut d = Decision {
            v: VERSION,
            line: self.line,
            manifest,
            parents,
            proposals: vec![],
            breakage: vec![],
            answers: vec![],
            resolutions: vec![],
            goals: vec![],
            source: None,
            note: String::new(),
        };
        with(&mut d);
        d
    }

    /// A result for `test` on a manifest artifact by `who`.
    fn result(&mut self, who: &Identity, manifest: (ContentHash, ContributionId), passed: bool) -> ContributionId {
        let argv: Vec<String> = ARGV.iter().map(|a| a.to_string()).collect();
        let method = CheckDefinition { name: "test", argv: &argv }.method();
        let exit = Exit::Code(if passed { 0 } else { 1 });
        let details = details(&manifest.0, 1, &argv, &exit, &RecordedTail::Text("out".into()));
        let verdict = if passed { Verdict::Pass } else { Verdict::Fail };
        self.add(who, Body::Evidence { target: manifest.1, verdict, method, details })
    }

    fn claim(&mut self, who: &Identity, statement: &str, tags: &[&str]) -> ContributionId {
        self.add(who, Body::Claim { statement: statement.into(), tags: tags.iter().map(|t| t.to_string()).collect() })
    }

    fn relate(
        &mut self,
        who: &Identity,
        from: ContributionId,
        kind: RelationKind,
        to: ContributionId,
    ) -> ContributionId {
        self.add(who, Body::Relation { from, kind, to, note: "why".into() })
    }

    fn object(&mut self, who: &Identity, from: ContributionId, to: ContributionId) -> ContributionId {
        self.relate(who, from, RelationKind::Contradicts, to)
    }

    fn retract(&mut self, who: &Identity, target: ContributionId) -> ContributionId {
        self.add(who, Body::Retract { target, reason: "r".into() })
    }

    /// A goal claim by `setter`, linked to the line by `linker`.
    fn goal(&mut self, setter: &Identity, linker: &Identity) -> (ContributionId, ContributionId) {
        let goal = self.claim(setter, "fast", &[GOAL_TAG]);
        let line = self.line;
        (goal, self.relate(linker, goal, RelationKind::Refines, line))
    }

    /// A proposal by alice on the line from `base`, whose change takes `from` to `to`.
    fn propose(&mut self, base: ContributionId, from: Files, to: Files) -> ContributionId {
        let new = self.tree(to);
        let change = Change::between(&manifest(from), &manifest(to)).hash();
        let p = Proposal { v: VERSION, line: self.line, base, manifest: new.0, change, summary: "p".into() };
        let alice = self.alice.clone();
        self.add(&alice, artifact(&p))
    }

    /// A new policy version by alice superseding `old`, with these goal setters and guardrails.
    fn change_policy(
        &mut self,
        old: ContributionId,
        goal_setters: Vec<manyroot::AgentId>,
        guardrails: Guardrails,
    ) -> ContributionId {
        let alice = self.alice.clone();
        let next = policy(self, goal_setters, guardrails);
        let next = self.add(&alice, artifact(&next));
        self.relate(&alice, next, RelationKind::Supersedes, old);
        next
    }

    /// Lines, built by the index and by walking, which must agree, with no question walked.
    fn lines(&self) -> (Index, Lines) {
        let ix = Index::build(&self.g);
        let lines = Lines::build(&self.g, &ix);
        let walked = Lines::build_by_walking(&self.g, &ix);
        for id in self.g.iter().map(|c| c.id()) {
            assert_eq!(lines.verdict(&id), walked.verdict(&id), "{id}: the index and a walk disagree");
        }
        assert_eq!(lines.obligation_stats().fallbacks, 0, "{:?}", lines.obligation_stats());
        assert_eq!(lines.stats().fallbacks, 0, "{:?}", lines.stats());
        (ix, lines)
    }

    fn verdict(&self, d: &ContributionId) -> Result<(), Unqualified> {
        let (_, lines) = self.lines();
        lines.verdict(d).expect("a decision").clone().map(|_| ())
    }

    /// Whether a decision alice would write now with this payload qualifies.
    fn judge_new(&self, d: &Decision) -> Result<(), Unqualified> {
        let (ix, lines) = self.lines();
        lines.judge_new(&self.g, &ix, self.alice.agent_id(), d).map(|_| ())
    }
}

fn unanswered(r: Result<(), Unqualified>, objection: ContributionId) -> bool {
    matches!(r, Err(Unqualified::Unanswered { objection: o, .. }) if o == objection)
}

fn not_allowed(r: Result<(), Unqualified>, objection: ContributionId) -> bool {
    matches!(r, Err(Unqualified::AnswerNotAllowed { objection: o, .. }) if o == objection)
}

/// The decisions every disposition test starts from: d0 on m0, then d1 on m1, which an objection
/// contradicts. Returns (m0, m1, d0, d1).
fn two_moves(
    w: &mut World,
) -> ((ContentHash, ContributionId), (ContentHash, ContributionId), ContributionId, ContributionId) {
    let m0 = w.tree(&[("a", b"0")]);
    let d0 = w.decide(m0.0, &[], |_| {});
    let m1 = w.tree(&[("a", b"1")]);
    let d1 = w.decide(m1.0, &[d0], |_| {});
    (m0, m1, d0, d1)
}

/// Each disposition against each basis (rule 5's table), for an objection to the line's head.
#[test]
fn each_disposition_is_allowed_only_for_its_bases() {
    use Disposition::*;
    // basis, then for fixed, fell_back, accepted_breakage, overridden: whether it qualifies.
    let table = [
        ("evidence", [true, true, true, false]),
        ("goal", [true, true, false, false]),
        ("discussion", [true, true, false, true]),
    ];
    for (basis, expected) in table {
        for (disposition, ok) in [Fixed, FellBack, AcceptedBreakage, Overridden].into_iter().zip(expected) {
            let mut w = World::new();
            let (alice, bob) = (w.alice.clone(), w.bob.clone());
            let (m0, m1, d0, d1) = two_moves(&mut w);
            let from = match basis {
                "evidence" => w.result(&bob, m1, false),
                "goal" => w.goal(&alice, &alice).0,
                _ => w.claim(&bob, "I don't like it", &[]),
            };
            let o = w.object(&bob, from, d1);
            let next = match disposition {
                FellBack => m0.0,
                _ => w.tree(&[("a", b"2")]).0,
            };
            let e = w.decide(next, &[d1], |d| {
                d.answers.push(answer(o, disposition));
                if disposition == AcceptedBreakage {
                    d.breakage.push(breakage("test", &["a"]));
                }
            });
            let verdict = w.verdict(&e);
            assert_eq!(verdict.is_ok(), ok, "{basis} answered {}: {verdict:?}", disposition.as_str());
            if !ok {
                assert!(not_allowed(verdict, o), "{basis} {}", disposition.as_str());
            }
            assert!(w.verdict(&d0).is_ok() && w.verdict(&d1).is_ok());
        }
    }
}

/// An objection in the window must be answered: by the decision itself, or by a decision between the
/// objected one and it. One made before a move binds that move; one made later binds the next.
#[test]
fn an_objection_binds_every_move_until_answered() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let (_, _, _, d1) = two_moves(&mut w);
    let why = w.claim(&bob, "no", &[]);
    let o = w.object(&bob, why, d1);
    let m2 = w.tree(&[("a", b"2")]);
    let e = w.decide(m2.0, &[d1], |_| {});
    assert!(unanswered(w.verdict(&e), o));
    let e = w.decide(m2.0, &[d1], |d| d.answers.push(answer(o, Disposition::Overridden)));
    assert_eq!(w.verdict(&e), Ok(()));
    // A later move needn't answer it again: e answered it, between d1 and it.
    let m3 = w.tree(&[("a", b"3")]);
    let f = w.decide(m3.0, &[e], |_| {});
    assert_eq!(w.verdict(&f), Ok(()));
    // An answer by a decision the objection's target isn't below doesn't count: a sibling of d1.
    assert!(w.judge_new(&w.draft(m3.0, &[d1], |_| {})).is_err(), "e's answer isn't below a new child of d1");
}

/// Finding b2098df8e4be: the basis is classified under the policy in force for the decision judged.
/// A later policy that makes the goal's author a goal setter doesn't change an earlier decision, but
/// binds the decisions under it.
#[test]
fn a_basis_is_judged_under_the_policy_in_force() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let p0 = w.line;
    let (_, _, _, d1) = two_moves(&mut w);
    let (goal, _) = w.goal(&bob, &bob);
    let o = w.object(&bob, goal, d1);
    let m2 = w.tree(&[("a", b"2")]);
    let e = w.decide(m2.0, &[d1], |d| d.answers.push(answer(o, Disposition::Overridden)));
    assert_eq!(w.verdict(&e), Ok(()), "bob isn't a goal setter under p0: discussion");
    w.change_policy(p0, vec![alice.agent_id(), bob.agent_id()], rails(2, 2, &["secrets"]));
    assert_eq!(w.verdict(&e), Ok(()), "a later policy doesn't change e");
    let m3 = w.tree(&[("a", b"3")]);
    let f = w.decide(m3.0, &[e], |_| {});
    assert!(not_allowed(w.verdict(&f), o), "under p1 the goal binds, and overridden isn't allowed");
}

/// Task 8ff95e0b756f, the reviewer's case: a goal binds only through a link a goal setter wrote. alice
/// links her goal, then withdraws it by retracting the link; bob, no goal setter, links it again. The
/// goal doesn't bind, so an objection from it is discussion.
#[test]
fn a_goal_binds_only_through_a_goal_setters_link() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let (_, _, _, d1) = two_moves(&mut w);
    let (goal, link) = w.goal(&alice, &alice);
    w.retract(&alice, link);
    let line = w.line;
    w.relate(&bob, goal, RelationKind::Refines, line);
    let o = w.object(&bob, goal, d1);
    let m2 = w.tree(&[("a", b"2")]);
    let e = w.decide(m2.0, &[d1], |d| d.answers.push(answer(o, Disposition::Overridden)));
    assert_eq!(w.verdict(&e), Ok(()), "bob's link doesn't bind alice's goal");
    // With alice's link standing, the same answer isn't allowed.
    let mut w2 = World::new();
    let (alice, bob) = (w2.alice.clone(), w2.bob.clone());
    let (_, _, _, d1) = two_moves(&mut w2);
    let (goal, _) = w2.goal(&alice, &alice);
    let o = w2.object(&bob, goal, d1);
    let m2 = w2.tree(&[("a", b"2")]);
    let e = w2.decide(m2.0, &[d1], |d| d.answers.push(answer(o, Disposition::Overridden)));
    assert!(not_allowed(w2.verdict(&e), o));
}

/// Retraction within the decision's past (gotcha b4d45c7352d4): an objection retracted there binds
/// nothing; one retracted later still bound the decision; a retracted retraction restores it.
#[test]
fn an_objections_retraction_counts_within_the_decisions_past() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let (_, _, _, d1) = two_moves(&mut w);
    let why = w.claim(&bob, "no", &[]);
    let o = w.object(&bob, why, d1);
    let m2 = w.tree(&[("a", b"2")]);
    let before = w.decide(m2.0, &[d1], |_| {});
    let unobject = w.retract(&bob, o);
    let after = w.decide(m2.0, &[d1], |_| {});
    assert!(unanswered(w.verdict(&before), o), "the retraction came later");
    assert_eq!(w.verdict(&after), Ok(()), "retracted in its past");
    w.retract(&bob, unobject);
    let restored = w.decide(m2.0, &[d1], |_| {});
    assert!(unanswered(w.verdict(&restored), o), "a retracted retraction counts for nothing");
    assert_eq!(w.verdict(&after), Ok(()), "nor does it change an earlier decision");
}

/// An evidence-backed basis is a failing result eligible under the policy in force within the
/// decision's past: retracting the result after the decision doesn't make its accepted_breakage
/// answer disallowed, and a result retracted before it makes the basis discussion.
#[test]
fn an_evidence_basis_is_judged_within_the_decisions_past() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let (_, m1, _, d1) = two_moves(&mut w);
    let fail = w.result(&bob, m1, false);
    let o = w.object(&bob, fail, d1);
    let m2 = w.tree(&[("a", b"2")]);
    let with = |d: &mut Decision| {
        d.answers.push(answer(o, Disposition::AcceptedBreakage));
        d.breakage.push(breakage("test", &["a"]));
    };
    let e = w.decide(m2.0, &[d1], with);
    w.retract(&bob, fail);
    assert_eq!(w.verdict(&e), Ok(()));
    let late = w.decide(m2.0, &[d1], with);
    assert!(not_allowed(w.verdict(&late), o), "with the fail retracted in its past, the basis is discussion");
}

/// Objections to what a decision uses unchanged: a manifest artifact with its manifest hash, a
/// proposal it lists or one with the same change, or a proposal whose change is in its own (security
/// note 17906a29698f). Only accepted_breakage and overridden can answer them.
#[test]
fn objections_to_a_manifest_or_proposal_bind_the_decisions_that_use_them() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let base0: Files = &[("a", b"0")];
    let m0 = w.tree(base0);
    let d0 = w.decide(m0.0, &[], |_| {});
    let with_b: Files = &[("a", b"0"), ("b", b"1")];
    let p1 = w.propose(d0, base0, with_b);
    let why = w.claim(&bob, "b is wrong", &[]);
    let o = w.object(&bob, why, p1);
    let hash = manifest(with_b).content_hash();
    // Listing the proposal, or integrating its tree as given, both use it.
    let listing = w.decide(hash, &[d0], |d| d.proposals.push(p1));
    let given = w.decide(hash, &[d0], |_| {});
    for e in [listing, given] {
        assert!(unanswered(w.verdict(&e), o), "{e}");
    }
    let fixed = w.decide(hash, &[d0], |d| d.answers.push(answer(o, Disposition::Fixed)));
    assert!(not_allowed(w.verdict(&fixed), o), "fixed can't answer an objection to what it uses unchanged");
    let overridden = w.decide(hash, &[d0], |d| d.answers.push(answer(o, Disposition::Overridden)));
    assert_eq!(w.verdict(&overridden), Ok(()));
    // A different change isn't reached.
    let other = w.tree(&[("a", b"0"), ("b", b"9")]);
    let e = w.decide(other.0, &[d0], |_| {});
    assert_eq!(w.verdict(&e), Ok(()));
    // An objection to a manifest artifact binds a decision on that manifest.
    let m9 = w.tree(&[("z", b"9")]);
    let o9 = w.object(&bob, why, m9.1);
    let e = w.decide(m9.0, &[d0], |_| {});
    assert!(unanswered(w.verdict(&e), o9));
}

/// A review of a proposal carries over a rebase with the same change, and binds a decision listing the
/// rebased proposal; after one that changes it, it doesn't. Retracting the proposal the objection
/// targets clears nothing (finding 06bbc6a34636).
#[test]
fn a_proposals_objection_carries_over_a_rebase_with_the_same_change() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let base0: Files = &[("a", b"0")];
    let m0 = w.tree(base0);
    let d0 = w.decide(m0.0, &[], |_| {});
    let p1 = w.propose(d0, base0, &[("a", b"0"), ("b", b"1")]);
    let why = w.claim(&bob, "b is wrong", &[]);
    let o = w.object(&bob, why, p1);
    let base1: Files = &[("a", b"0"), ("c", b"2")];
    let m1 = w.tree(base1);
    let d1 = w.decide(m1.0, &[d0], |_| {});
    let same = w.propose(d1, base1, &[("a", b"0"), ("b", b"1"), ("c", b"2")]);
    let other = w.propose(d1, base1, &[("a", b"0"), ("b", b"9"), ("c", b"2")]);
    w.retract(&alice, p1);
    let hash = |files: Files| manifest(files).content_hash();
    let e = w.decide(hash(&[("a", b"0"), ("b", b"1"), ("c", b"2")]), &[d1], |d| d.proposals.push(same));
    assert!(unanswered(w.verdict(&e), o), "carried over the rebase, through the retracted proposal");
    let e = w.decide(hash(&[("a", b"0"), ("b", b"9"), ("c", b"2")]), &[d1], |d| d.proposals.push(other));
    assert_eq!(w.verdict(&e), Ok(()), "not carried over a different change");
    let (ix, lines) = w.lines();
    let open = lines.proposal_objections(&w.g, &ix, &same).expect("a current policy");
    assert_eq!(open.iter().map(|o| o.objection.id).collect::<Vec<_>>(), [o]);
    assert_eq!(open[0].basis, Basis::Discussion);
    assert_eq!(open[0].allowed, [Disposition::Overridden]);
}

/// Finding 06bbc6a34636 at qualification: an objection to a manifest artifact still binds after its
/// author writes a second artifact of the same manifest and retracts the first.
#[test]
fn a_manifest_objection_binds_after_its_artifact_is_retracted() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&[("a", b"0")]);
    let d0 = w.decide(m0.0, &[], |_| {});
    let files: Files = &[("a", b"5")];
    let first = w.tree(files);
    let why = w.claim(&bob, "no", &[]);
    let o = w.object(&bob, why, first.1);
    w.add(&alice, artifact(&manifest(files)));
    w.retract(&alice, first.1);
    let e = w.decide(first.0, &[d0], |_| {});
    assert!(unanswered(w.verdict(&e), o));
}

/// Answer 2c28892e9701: rule 4 resolves the manifest and its files by hash, retracted artifacts
/// included, so retracting a manifest artifact or a file version after a decision leaves it qualified.
#[test]
fn retracting_a_manifest_or_file_version_after_a_decision_leaves_it_qualified() {
    for what in ["manifest", "file"] {
        let mut w = World::new();
        let alice = w.alice.clone();
        let file = w.add(
            &alice,
            Body::Artifact { name: "f".into(), media_type: FILE_MEDIA_TYPE.into(), content: b"0".to_vec() },
        );
        let m = manifest(&[("a", b"0")]);
        let artifact_id = w.add(&alice, artifact(&m));
        let d = w.decide(m.content_hash(), &[], |_| {});
        assert_eq!(w.verdict(&d), Ok(()), "{what}");
        w.retract(&alice, if what == "manifest" { artifact_id } else { file });
        assert_eq!(w.verdict(&d), Ok(()), "{what}: a later retraction changes nothing");
        let (ix, _) = w.lines();
        assert!(ix.manifest(&m.content_hash()).is_some() && ix.missing(&m).is_empty(), "{what}");
        // A proposal from it still recomputes its change.
        let p = w.propose(d, &[("a", b"0")], &[("a", b"0"), ("b", b"1")]);
        let (_, lines) = w.lines();
        assert_eq!(lines.proposal(&p).expect("a proposal").verdict, Ok(()), "{what}");
    }
}

/// Rule 6: no breakage entry names a protected path; at most `max_known` entries, none more than
/// `max_age` line moves old, unless the decision reduces every parent's breakage on the line. A root
/// gets no exception.
#[test]
fn guardrails_hold_unless_the_breakage_reduces() {
    let mut w = World::new();
    let m: Vec<ContentHash> = (0..6u8).map(|i| w.tree(&[("a", &[i][..])]).0).collect();
    let protected = w.decide(m[0], &[], |d| d.breakage.push(breakage("test", &["secrets/key"])));
    assert!(matches!(w.verdict(&protected), Err(Unqualified::ProtectedPath { .. })));
    let three = |d: &mut Decision| d.breakage.extend(["a", "b", "c"].map(|c| breakage(c, &[])));
    let root = w.decide(m[0], &[], three);
    assert!(
        matches!(w.verdict(&root), Err(Unqualified::TooMuchBreakage { count: 3, max: 2 })),
        "a root gets no exception"
    );
    let d0 = w.decide(m[0], &[], |d| d.breakage.push(breakage("test", &[])));
    let d1 = w.decide(m[1], &[d0], |d| d.breakage.push(breakage("test", &[])));
    let d2 = w.decide(m[2], &[d1], |d| d.breakage.push(breakage("test", &[])));
    assert_eq!((w.verdict(&d0), w.verdict(&d1)), (Ok(()), Ok(())), "one and two moves old");
    assert!(matches!(w.verdict(&d2), Err(Unqualified::BreakageTooOld { age: 3, max: 2, .. })));
    // Reducing: a parent with two entries, then one, past the age limit, qualifies.
    let both = |d: &mut Decision| d.breakage.extend(["test", "lint"].map(|c| breakage(c, &[])));
    let e0 = w.decide(m[3], &[], both);
    let e1 = w.decide(m[4], &[e0], both);
    let e2 = w.decide(m[5], &[e1], |d| d.breakage.push(breakage("test", &[])));
    assert_eq!(w.verdict(&e2), Ok(()), "three moves old, but fewer than its parent's");
    let e3 = w.decide(m[5], &[e1], both);
    assert!(matches!(w.verdict(&e3), Err(Unqualified::BreakageTooOld { .. })), "the same set isn't a reduction");
}

/// Open objections: what the next move must answer, with the dispositions each basis allows; once a
/// move answers one as allowed, it isn't open. integrate's check (judge_new) agrees.
#[test]
fn open_objections_are_what_the_next_move_must_answer() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let (_, m1, _, d1) = two_moves(&mut w);
    let fail = w.result(&bob, m1, false);
    let o = w.object(&bob, fail, d1);
    let (ix, lines) = w.lines();
    let open = lines.open_objections(&w.g, &ix, &w.line).expect("a current policy");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].objection.id, o);
    assert!(matches!(&open[0].basis, Basis::Evidence { result, check } if *result == fail && check == "test"));
    use Disposition::*;
    assert_eq!(open[0].allowed, [Fixed, FellBack, AcceptedBreakage]);
    let m2 = w.tree(&[("a", b"2")]);
    assert!(unanswered(w.judge_new(&w.draft(m2.0, &[d1], |_| {})), o));
    let overridden = w.draft(m2.0, &[d1], |d| d.answers.push(answer(o, Overridden)));
    assert!(not_allowed(w.judge_new(&overridden), o));
    let fixed = w.draft(m2.0, &[d1], |d| d.answers.push(answer(o, Fixed)));
    assert_eq!(w.judge_new(&fixed), Ok(()));
    let alice = w.alice.clone();
    w.add(&alice, artifact(&fixed));
    let (ix, lines) = w.lines();
    assert!(lines.open_objections(&w.g, &ix, &w.line).expect("a current policy").is_empty());
}

/// Many decisions, each with objections in its window and results and goals behind them, are judged
/// with every question answered by the index, and agree with walking (World::lines).
#[test]
fn many_objections_are_judged_without_a_walk() {
    let mut w = World::new();
    let (alice, bob, carol) = (w.alice.clone(), w.bob.clone(), w.carol.clone());
    let (goal, _) = w.goal(&alice, &alice);
    let start = w.tree(&[("a", b"start")]);
    let mut parent = w.decide(start.0, &[], |_| {});
    for i in 0..40u32 {
        let m = w.tree(&[("a", &i.to_le_bytes()[..])]);
        let fail = w.result(if i % 2 == 0 { &bob } else { &carol }, m, false);
        let d = w.decide(m.0, &[parent], |_| {});
        let from = match i % 3 {
            0 => fail,
            1 => goal,
            _ => w.claim(&bob, "hm", &[]),
        };
        let o = w.object(&bob, from, d);
        if i % 5 == 0 {
            w.retract(&bob, o);
        }
        let next = w.tree(&[("a", &(1000 + i).to_le_bytes()[..])]);
        parent = w.decide(next.0, &[d], |x| x.answers.push(answer(o, Disposition::Fixed)));
    }
    let (_, lines) = w.lines();
    assert_eq!(lines.verdict(&parent).expect("judged").as_ref().map(|_| ()), Ok(()));
    assert!(lines.obligation_stats().questions > 0, "{:?}", lines.obligation_stats());
}
