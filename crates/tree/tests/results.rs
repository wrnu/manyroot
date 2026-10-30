//! Counted check results, green, goals and reviews (docs/design/weave.md, section 1
//! *Goals, checks and objections*, section 2 *Counted check results* and *Green*;
//! build step 7). Every test also asserts that no ancestor question fell back to a
//! walk (decision on split note dc467163233c).

use std::collections::BTreeMap;
use std::rc::Rc;

use bracken_tree::check::{Exit, RecordedTail};
use bracken_tree::format::{
    Change, CheckDefinition, Decision, FILE_MEDIA_TYPE, GOAL_TAG, Guardrails, Line, Manifest, Marker, Proposal, Record,
    VERSION,
};
use bracken_tree::ground::{Index, Review};
use bracken_tree::lines::Lines;
use bracken_tree::results::{Checks, Counted, Green, PastQuery, details, parse_details};
use bracken_tree::reviews::{Reviews, Target, goals};
use bracken_tree::{ContentHash, Entry, Mode};
use manyroot::{Body, Contribution, ContributionId, Graph, GroundId, Identity, RelationKind, Verdict};

const GROUND: GroundId = GroundId([6; 32]);
const ARGV: &[&str] = &["cargo", "test"];

/// A Weave Ground in memory: alice's marker and `main` line, whose policy has one check, `test`,
/// and validators alice, bob and carol (alice, to show the integrator's own results never count).
struct World {
    g: Graph,
    clock: u64,
    alice: Rc<Identity>,
    bob: Rc<Identity>,
    carol: Rc<Identity>,
    line: ContributionId,
}

fn policy(w: &World, argv: &[&str]) -> Line {
    Line {
        v: VERSION,
        name: "main".into(),
        integrators: vec![w.alice.agent_id()],
        validators: vec![w.alice.agent_id(), w.bob.agent_id(), w.carol.agent_id()],
        goal_setters: vec![w.alice.agent_id()],
        checks: BTreeMap::from([("test".to_owned(), argv.iter().map(|a| a.to_string()).collect())]),
        guardrails: Guardrails { max_known: 0, max_age: 0, protected: vec![] },
        owner_decision: Some("decision 0123456789ab".into()),
    }
}

fn artifact<R: Record>(record: &R) -> Body {
    Body::Artifact {
        name: record.artifact_name().to_owned(),
        media_type: R::MEDIA_TYPE.to_owned(),
        content: record.encode(),
    }
}

fn manifest(files: &[(&str, &[u8])]) -> Manifest {
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

fn draft(line: ContributionId, manifest: ContentHash, mut parents: Vec<ContributionId>) -> Decision {
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
        note: String::new(),
    }
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
        let line = policy(&w, ARGV);
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
    fn tree(&mut self, files: &[(&str, &[u8])]) -> (ContentHash, ContributionId) {
        let alice = self.alice.clone();
        for (_, bytes) in files {
            let body = Body::Artifact { name: "f".into(), media_type: FILE_MEDIA_TYPE.into(), content: bytes.to_vec() };
            self.add(&alice, body);
        }
        let m = manifest(files);
        (m.content_hash(), self.add(&alice, artifact(&m)))
    }

    fn decide(&mut self, manifest: ContentHash, parents: &[ContributionId]) -> ContributionId {
        let alice = self.alice.clone();
        let d = draft(self.line, manifest, parents.to_vec());
        self.add(&alice, artifact(&d))
    }

    /// A new policy version by alice superseding `old`, with `test` running `argv`.
    fn change_policy(&mut self, old: ContributionId, argv: &[&str]) -> ContributionId {
        let alice = self.alice.clone();
        let next = policy(self, argv);
        let next = self.add(&alice, artifact(&next));
        self.add(&alice, Body::Relation { from: next, kind: RelationKind::Supersedes, to: old, note: String::new() });
        next
    }

    /// A result for `test` running `argv` on a manifest artifact, at an attempt, after `after` (every head
    /// if `None`).
    fn result(
        &mut self,
        who: &Identity,
        manifest: (ContentHash, ContributionId),
        argv: &[&str],
        passed: bool,
        attempt: u64,
        after: Option<Vec<ContributionId>>,
    ) -> ContributionId {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let method = CheckDefinition { name: "test", argv: &argv }.method();
        let exit = Exit::Code(if passed { 0 } else { 1 });
        let details = details(&manifest.0, attempt, &argv, &exit, &RecordedTail::Text("ok".into()));
        let verdict = if passed { Verdict::Pass } else { Verdict::Fail };
        let body = Body::Evidence { target: manifest.1, verdict, method, details };
        let parents = after.unwrap_or_else(|| self.g.heads().collect());
        self.add_after(who, parents, body)
    }

    fn checks(&self) -> (Index, Lines, Checks) {
        let ix = Index::build(&self.g);
        let lines = Lines::build(&self.g, &ix);
        let checks = Checks::build(&self.g, &ix, &lines);
        (ix, lines, checks)
    }

    /// The line's green, with no ancestor question walked.
    fn green(&self) -> Green {
        let (_, lines, checks) = self.checks();
        let green = checks.green(&self.g, &lines, &self.line);
        assert_eq!(checks.stats().fallbacks, 0, "{:?}", checks.stats());
        green
    }

    /// The counted results of `test` for a decision under the line's current policy, with no ancestor
    /// question walked.
    fn counted(&self, decision: &ContributionId) -> Counted {
        let (_, lines, checks) = self.checks();
        let mut counted = checks.counted(&self.g, &lines, decision).expect("one current policy");
        assert_eq!(checks.stats().fallbacks, 0, "{:?}", checks.stats());
        counted.remove(0)
    }

    /// The counted results of `test` for a decision on its own manifest, over its own past, under the
    /// policy in force for it, which must be `version`, with no ancestor question walked.
    fn past(&self, decision: &ContributionId, version: &ContributionId) -> Counted {
        let (ix, lines, checks) = self.checks();
        let d = lines.decision(decision).expect("a decision");
        assert_eq!(lines.policy_in_force_for(decision).map(|v| v.id), Some(*version), "Lines gives the policy");
        let query = PastQuery { decision: *decision, manifest: d.decision.manifest };
        let (mut counted, stats) = checks.count_in_pasts(&self.g, &ix, &lines, &[query]);
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        counted.remove(0).expect("a policy in force").remove(0)
    }
}

fn sorted(mut ids: Vec<ContributionId>) -> Vec<ContributionId> {
    ids.sort();
    ids
}

#[test]
fn details_carry_the_manifest_and_the_attempt() {
    let hash = ContentHash::of(b"m");
    let text = details(&hash, 3, &["cargo".into()], &Exit::Code(1), &RecordedTail::Text("boom".into()));
    assert!(text.starts_with(&format!("{hash}\nattempt 3\n")), "{text}");
    assert_eq!(parse_details(&text), Some((hash, 3)));
    assert_eq!(parse_details("blake3:zz\nattempt 1"), None);
}

#[test]
fn a_validators_pass_makes_a_decision_green_and_the_integrators_doesnt() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    assert_eq!(w.green(), Green::None, "no results yet");
    let alice = w.alice.clone();
    w.result(&alice, m, ARGV, true, 1, None);
    assert_eq!(w.green(), Green::None, "the integrator's own pass doesn't count");
    let bob = w.bob.clone();
    let pass = w.result(&bob, m, ARGV, true, 1, None);
    assert_eq!(w.green(), Green::One(d));
    assert_eq!(w.counted(&d).passes, [pass]);
}

/// Finding 98dd56b8170a: a policy change makes green start over. Passes and fails recorded before the
/// new version aren't counted; the manifest goes green on fresh results alone.
#[test]
fn green_after_a_policy_change_counts_only_fresh_results() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let (bob, carol) = (w.bob.clone(), w.carol.clone());
    w.result(&bob, m, ARGV, true, 1, None);
    w.result(&carol, m, ARGV, false, 1, None);
    assert_eq!(w.green(), Green::None, "carol's fail blocks it under the first policy");
    let line = w.line;
    w.change_policy(line, ARGV);
    assert_eq!(w.green(), Green::None, "the old pass doesn't count under the new policy");
    assert!(w.counted(&d).fails.is_empty(), "nor does the old fail");
    w.result(&bob, m, ARGV, true, 2, None);
    assert_eq!(w.green(), Green::One(d), "a fresh pass, and the old fail ignored");
}

/// A validator's re-run replaces its fail, and every earlier fail stays listed as replaced.
#[test]
fn a_validators_rerun_replaces_its_fail_and_every_attempt_shows() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let bob = w.bob.clone();
    let first = w.result(&bob, m, ARGV, false, 1, None);
    let second = w.result(&bob, m, ARGV, false, 2, None);
    assert_eq!(w.green(), Green::None);
    w.result(&bob, m, ARGV, true, 3, None);
    assert_eq!(w.green(), Green::One(d));
    let counted = w.counted(&d);
    assert_eq!(sorted(counted.replaced.clone()), sorted(vec![first, second]));
    let (_, _, checks) = w.checks();
    assert_eq!(checks.next_attempt(&bob.agent_id(), &m.0, &counted.method), 4);
    assert_eq!(checks.latest(&bob.agent_id(), &m.0, &counted.method).len(), 1);
    let attempts: Vec<u64> = checks.on_manifest(&m.0).iter().filter_map(|r| r.attempt).collect();
    assert_eq!(sorted_u64(attempts), [1, 2, 3]);
}

fn sorted_u64(mut v: Vec<u64>) -> Vec<u64> {
    v.sort();
    v
}

/// Several concurrent latest results from one validator count together as one fail, until a later
/// result follows them both.
#[test]
fn concurrent_latest_results_count_as_a_fail() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let bob = w.bob.clone();
    let heads: Vec<ContributionId> = w.g.heads().collect();
    let one = w.result(&bob, m, ARGV, true, 1, Some(heads.clone()));
    let other = w.result(&bob, m, ARGV, true, 2, Some(heads));
    let counted = w.counted(&d);
    assert_eq!((counted.failing_validators, counted.passes.len()), (1, 0), "{counted:?}");
    assert_eq!(sorted(counted.fails.clone()), sorted(vec![one, other]));
    assert_eq!(w.green(), Green::None);
    w.result(&bob, m, ARGV, true, 3, None);
    assert_eq!(w.green(), Green::One(d));
}

/// Finding 4189811f3cdd: a result for another line's check of the same name and a different argv
/// neither counts nor replaces a fail.
#[test]
fn a_result_for_another_definition_neither_counts_nor_replaces_a_fail() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let bob = w.bob.clone();
    let fail = w.result(&bob, m, ARGV, false, 1, None);
    w.result(&bob, m, &["cargo", "test", "--other"], true, 2, None);
    let counted = w.counted(&d);
    assert_eq!((counted.fails.clone(), counted.passes.len()), (vec![fail], 0), "{counted:?}");
    assert_eq!(w.green(), Green::None);
}

/// A retracted result isn't counted, and the one before it is latest again.
#[test]
fn a_retracted_result_isnt_counted() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let bob = w.bob.clone();
    let fail = w.result(&bob, m, ARGV, false, 1, None);
    let pass = w.result(&bob, m, ARGV, true, 2, None);
    assert_eq!(w.green(), Green::One(d));
    w.add(&bob, Body::Retract { target: pass, reason: "wrong manifest".into() });
    assert_eq!(w.counted(&d).fails, [fail]);
    assert_eq!(w.green(), Green::None);
}

/// Green needs empty breakage. On a fork it is the last green below every head; once a merge settles
/// the fork, its two green sides are ambiguous until the merge's own manifest passes.
#[test]
fn green_is_the_last_green_below_every_head() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let m0 = w.tree(&[("a", b"0")]);
    let d0 = w.decide(m0.0, &[]);
    w.result(&bob, m0, ARGV, true, 1, None);
    let (m1, m2) = (w.tree(&[("a", b"1")]), w.tree(&[("a", b"2")]));
    let at: Vec<ContributionId> = w.g.heads().collect();
    let alice = w.alice.clone();
    let d1 = w.add_after(&alice, at.clone(), artifact(&draft(w.line, m1.0, vec![d0])));
    let d2 = w.add_after(&alice, at, artifact(&draft(w.line, m2.0, vec![d0])));
    w.result(&bob, m1, ARGV, true, 1, None);
    w.result(&bob, m2, ARGV, true, 1, None);
    let (_, lines, _) = w.checks();
    assert_eq!(lines.heads(&w.line).len(), 2, "forked");
    assert_eq!(w.green(), Green::One(d0), "the last green below both heads");
    let m3 = w.tree(&[("a", b"3")]);
    let d3 = w.decide(m3.0, &[d1, d2]);
    // Below the merge, the only head, both sides are green and neither is below the other.
    assert_eq!(w.green(), Green::Ambiguous(sorted(vec![d1, d2])), "the merge has no result yet");
    w.result(&bob, m3, ARGV, true, 1, None);
    assert_eq!(w.green(), Green::One(d3));
    // A decision with breakage is never green, whatever its results.
    let m4 = w.tree(&[("a", b"4")]);
    let mut broken = draft(w.line, m4.0, vec![d3]);
    broken.breakage.push(bracken_tree::format::Breakage {
        check: "test".into(),
        detail: "flaky".into(),
        paths: vec![],
    });
    w.add(&alice, artifact(&broken));
    w.result(&bob, m4, ARGV, true, 1, None);
    assert_eq!(w.green(), Green::One(d3));
}

/// A diverged chain has no current policy, so green is undefined, listing the versions.
#[test]
fn green_is_undefined_on_a_diverged_policy() {
    let mut w = World::new();
    let line = w.line;
    let a = w.change_policy(line, ARGV);
    let b = w.change_policy(line, &["cargo", "test", "-q"]);
    assert_eq!(w.green(), Green::Undefined(sorted(vec![a, b])));
}

#[test]
fn goals_bind_under_a_policy_whose_goal_setters_include_their_author() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let mut ids = vec![];
    for who in [&alice, &bob] {
        let goal = w.add(who, Body::Claim { statement: "fast".into(), tags: vec![GOAL_TAG.into()] });
        w.add(who, Body::Relation { from: goal, kind: RelationKind::Refines, to: w.line, note: String::new() });
        ids.push(goal);
    }
    // An untagged claim refining the line isn't a goal.
    let plain = w.add(&bob, Body::Claim { statement: "x".into(), tags: vec![] });
    w.add(&bob, Body::Relation { from: plain, kind: RelationKind::Refines, to: w.line, note: String::new() });
    let (ix, lines, _) = w.checks();
    let set = goals(&ix, &w.line);
    assert_eq!(sorted(set.iter().map(|g| g.id).collect()), sorted(ids.clone()));
    let policy = &lines.version(&w.line).expect("the line").policy;
    let binding: Vec<ContributionId> = set.iter().filter(|g| g.binds(policy)).map(|g| g.id).collect();
    assert_eq!(binding, [ids[0]], "alice is the only goal setter");
}

/// A review of a proposal carries over a rebase that leaves its change as it was, and not one that
/// changes it; a review of a manifest artifact reaches every decision with that manifest.
#[test]
fn reviews_carry_by_change_and_by_manifest() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let base0: [(&str, &[u8]); 1] = [("a", b"0")];
    let m0 = w.tree(&base0);
    let d0 = w.decide(m0.0, &[]);
    let proposal = |w: &mut World, base: ContributionId, from: &[(&str, &[u8])], to: &[(&str, &[u8])]| {
        let new = w.tree(to);
        let change = Change::between(&manifest(from), &manifest(to)).hash();
        let p = Proposal { v: VERSION, line: w.line, base, manifest: new.0, change, summary: "p".into() };
        let alice = w.alice.clone();
        w.add(&alice, artifact(&p))
    };
    let p1 = proposal(&mut w, d0, &base0, &[("a", b"0"), ("b", b"1")]);
    let objection =
        w.add(&bob, Body::Relation { from: m0.1, kind: RelationKind::Contradicts, to: p1, note: "no".into() });
    // The line moves, adding c. A rebase adding the same b keeps the change; one adding another b doesn't.
    let base1: [(&str, &[u8]); 2] = [("a", b"0"), ("c", b"2")];
    let m1 = w.tree(&base1);
    let d1 = w.decide(m1.0, &[d0]);
    let same = proposal(&mut w, d1, &base1, &[("a", b"0"), ("b", b"1"), ("c", b"2")]);
    let other = proposal(&mut w, d1, &base1, &[("a", b"0"), ("b", b"9"), ("c", b"2")]);
    let agreement =
        w.add(&alice, Body::Relation { from: m0.1, kind: RelationKind::Supports, to: m1.1, note: String::new() });
    let (ix, lines, _) = w.checks();
    for p in [p1, same, other] {
        assert!(lines.proposal(&p).expect("a proposal").verdict.is_ok(), "{p} is valid");
    }
    let reviews = Reviews::build(&ix, &lines);
    let ids = |list: Vec<(&Review, Target)>| list.iter().map(|(r, _)| r.id).collect::<Vec<_>>();
    assert_eq!(ids(reviews.of_proposal(&lines, &same)), [objection], "carried over the rebase");
    assert!(ids(reviews.of_proposal(&lines, &other)).is_empty(), "a different change isn't reviewed");
    assert_eq!(ids(reviews.of_decision(&lines, &d1)), [agreement], "the manifest's review reaches its decision");
    let (review, target) = reviews.of_decision(&lines, &d1)[0];
    assert!(!review.objection && matches!(target, Target::Manifest(id, hash) if id == m1.1 && hash == m1.0));
}

/// The latest-result rule over many results is answered by passes, never by a walk per question.
#[test]
fn many_results_are_answered_without_a_walk() {
    let mut w = World::new();
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let (bob, carol) = (w.bob.clone(), w.carol.clone());
    for attempt in 1..=100 {
        w.result(&bob, m, ARGV, attempt % 2 == 0, attempt, None);
        w.result(&carol, m, ARGV, attempt % 3 == 0, attempt, None);
    }
    let (_, _, checks) = w.checks();
    let stats = checks.stats();
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
    // Two groups of 100 side by side: 200 targets, a pass per 64, and one epoch pass.
    assert_eq!(stats.passes, 200usize.div_ceil(64) + 1, "{stats:?}");
    let counted = w.counted(&d);
    // bob's last attempt, 100, passed; carol's, 100 (not a multiple of 3), failed. Every earlier fail by
    // either is replaced: bob's 50 odd attempts, carol's 67 attempts that aren't multiples of 3, less her
    // last, which is the one counted.
    assert_eq!((counted.passes.len(), counted.failing_validators, counted.replaced.len()), (1, 1, 50 + 66));
}

/// Finding 3aa37cc9ef0d: a line's green is one of its own decisions. A new line q whose only decision
/// has a passed main decision as its parent has no green of its own yet.
#[test]
fn a_lines_green_is_never_another_lines_decision() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let m0 = w.tree(&[("a", b"0")]);
    let main = w.decide(m0.0, &[]);
    let q = w.add(&alice, artifact(&Line { name: "q".into(), ..policy(&w, ARGV) }));
    w.result(&bob, m0, ARGV, true, 1, None);
    let m1 = w.tree(&[("a", b"1")]);
    let dq = w.add(&alice, artifact(&draft(q, m1.0, vec![main])));
    let (_, lines, checks) = w.checks();
    assert_eq!(lines.heads(&q), [dq]);
    assert_eq!(checks.green(&w.g, &lines, &q), Green::None);
    assert_eq!(checks.green(&w.g, &lines, &w.line), Green::One(main));
    assert_eq!(checks.stats().fallbacks, 0);
}

/// Finding c85337dfad09: counted over a decision's own past, under the policy in force for it. A later
/// re-run, a later retraction and a later policy change leave what it saw unchanged; within its past,
/// each validator's latest counts, and a retraction there counts.
#[test]
fn a_decisions_count_reads_only_its_own_past() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let p0 = w.line;
    let m = w.tree(&[("a", b"0")]);
    let first = w.result(&bob, m, ARGV, false, 1, None);
    let d0 = w.decide(m.0, &[]);
    assert_eq!(w.past(&d0, &p0).fails, [first]);
    // After d0: a re-run that passes, and a retraction of the fail.
    let rerun = w.result(&bob, m, ARGV, true, 2, None);
    w.add(&bob, Body::Retract { target: first, reason: "flaky".into() });
    let seen = w.past(&d0, &p0);
    assert_eq!((seen.fails.clone(), seen.passes.len()), (vec![first], 0), "d0 still saw its fail: {seen:?}");
    // A decision after that sees the re-run as latest, and the fail both replaced and retracted.
    let d1 = w.decide(m.0, &[d0]);
    let later = w.past(&d1, &p0);
    assert_eq!((later.passes.clone(), later.fails.len(), later.replaced.len()), (vec![rerun], 0, 0), "{later:?}");
    // A new policy changes neither count, and a decision under it counts afresh (the epoch).
    let p1 = w.change_policy(p0, ARGV);
    assert_eq!(w.past(&d0, &p0).fails, [first]);
    assert_eq!(w.past(&d1, &p0).passes, [rerun]);
    let d2 = w.decide(m.0, &[d1]);
    assert!(w.past(&d2, &p1).passes.is_empty(), "the re-run is older than p1");
}

/// Within a decision's past, a validator's concurrent latest results count as one fail, and an earlier
/// fail a later attempt there replaced is listed.
#[test]
fn concurrent_results_in_a_decisions_past_count_as_a_fail() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let p0 = w.line;
    let m = w.tree(&[("a", b"0")]);
    let fail = w.result(&bob, m, ARGV, false, 1, None);
    let heads: Vec<ContributionId> = w.g.heads().collect();
    let one = w.result(&bob, m, ARGV, true, 2, Some(heads.clone()));
    let other = w.result(&bob, m, ARGV, true, 3, Some(heads));
    let d = w.decide(m.0, &[]);
    let counted = w.past(&d, &p0);
    assert_eq!(counted.failing_validators, 1);
    assert_eq!(sorted(counted.fails.clone()), sorted(vec![one, other]));
    assert_eq!(counted.replaced, [fail]);
}

/// Finding 5d6f605b2f15: a forged attempt of u64::MAX doesn't overflow the next attempt.
#[test]
fn a_forged_highest_attempt_saturates() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let m = w.tree(&[("a", b"0")]);
    w.result(&bob, m, ARGV, false, u64::MAX, None);
    let (_, _, checks) = w.checks();
    let method =
        CheckDefinition { name: "test", argv: &ARGV.iter().map(|a| a.to_string()).collect::<Vec<_>>() }.method();
    assert_eq!(checks.next_attempt(&bob.agent_id(), &m.0, &method), u64::MAX);
}

/// Many decisions, each after one more of a validator's attempts, counted over their own pasts in one
/// batch: each sees exactly the attempt before it as latest, and nothing is walked.
#[test]
fn many_decisions_are_counted_over_their_pasts_without_a_walk() {
    const N: u64 = 60;
    let mut w = World::new();
    let bob = w.bob.clone();
    let p0 = w.line;
    let m = w.tree(&[("a", b"0")]);
    let mut seen = vec![];
    let mut previous: Vec<ContributionId> = vec![];
    for attempt in 1..=N {
        let r = w.result(&bob, m, ARGV, attempt % 2 == 0, attempt, None);
        let d = w.decide(m.0, &previous);
        previous = vec![d];
        seen.push((d, r, attempt % 2 == 0));
    }
    let (ix, lines, checks) = w.checks();
    let queries: Vec<PastQuery> = seen.iter().map(|(d, _, _)| PastQuery { decision: *d, manifest: m.0 }).collect();
    let (counted, stats) = checks.count_in_pasts(&w.g, &ix, &lines, &queries);
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
    for ((d, r, passed), counted) in seen.iter().zip(&counted) {
        assert_eq!(lines.policy_in_force_for(d).map(|v| v.id), Some(p0));
        let c = &counted.as_ref().expect("a policy in force")[0];
        let latest = if *passed { &c.passes } else { &c.fails };
        assert_eq!(latest, &[*r], "{d}: {c:?}");
    }
}

/// Finding 348e506fce4c: results attach to the manifest, not to one of its artifacts. Retracting the
/// artifact a validator's fail targets, while another artifact of the same manifest carries a pass,
/// clears nothing: neither green now nor the count of a decision made before the retraction.
#[test]
fn retracting_the_manifest_artifact_a_fail_targets_clears_nothing() {
    let mut w = World::new();
    let (alice, bob, carol) = (w.alice.clone(), w.bob.clone(), w.carol.clone());
    let p0 = w.line;
    let files: [(&str, &[u8]); 1] = [("a", b"0")];
    let first = w.tree(&files);
    let fail = w.result(&bob, first, ARGV, false, 1, None);
    let second = (first.0, w.add(&alice, artifact(&manifest(&files))));
    assert_ne!(first.1, second.1, "two artifacts of one manifest");
    w.result(&carol, second, ARGV, true, 1, None);
    let d = w.decide(first.0, &[]);
    assert_eq!(w.green(), Green::None);
    assert_eq!(w.past(&d, &p0).fails, [fail]);
    w.add(&alice, Body::Retract { target: first.1, reason: "duplicate".into() });
    assert_eq!(w.green(), Green::None, "green now still sees bob's fail");
    assert_eq!(w.counted(&d).fails, [fail]);
    assert_eq!(w.past(&d, &p0).fails, [fail], "nor does d's own past change");
    // With every artifact of the manifest retracted, the results still name it.
    w.add(&alice, Body::Retract { target: second.1, reason: "duplicate".into() });
    let (ix, _, _) = w.checks();
    assert_eq!(ix.check_results().iter().filter(|r| r.manifest == first.0).count(), 2);
}

/// Finding 140fde48d768: retracted goals, goal links and reviews stay in the index, so judging a past
/// decision can read their retractions within its own past; the views of now leave them out.
#[test]
fn retracted_goals_and_reviews_are_kept_with_their_retractions() {
    let mut w = World::new();
    let alice = w.alice.clone();
    let goal = w.add(&alice, Body::Claim { statement: "fast".into(), tags: vec![GOAL_TAG.into()] });
    let link =
        w.add(&alice, Body::Relation { from: goal, kind: RelationKind::Refines, to: w.line, note: String::new() });
    let m = w.tree(&[("a", b"0")]);
    let d = w.decide(m.0, &[]);
    let objection =
        w.add(&alice, Body::Relation { from: goal, kind: RelationKind::Contradicts, to: d, note: "slow".into() });
    let (ix, lines, _) = w.checks();
    assert_eq!(goals(&ix, &w.line).len(), 1);
    assert_eq!(Reviews::build(&ix, &lines).of_decision(&lines, &d).len(), 1);
    let unclaim = w.add(&alice, Body::Retract { target: goal, reason: "done".into() });
    let unobject = w.add(&alice, Body::Retract { target: objection, reason: "answered".into() });
    let (ix, lines, _) = w.checks();
    assert!(goals(&ix, &w.line).is_empty(), "the view of now leaves the retracted goal out");
    assert!(Reviews::build(&ix, &lines).of_decision(&lines, &d).is_empty());
    let later = w.add(&alice, Body::Claim { statement: "later".into(), tags: vec![] });
    let g = &w.g;
    let within = |asker: ContributionId| move |x: &ContributionId| g.is_ancestor(x, &asker);
    let kept = ix.goal_links().iter().find(|l| l.id == link).expect("kept");
    assert!(kept.retracted);
    assert!(!kept.retracted_within(ix.retractions(), within(d)), "d's past holds no retraction");
    assert!(kept.retracted_within(ix.retractions(), within(later)), "{unclaim} is in a later past");
    let kept = ix.reviews().iter().find(|r| r.id == objection).expect("kept");
    assert!(kept.retracted);
    assert!(!ix.retractions().within(&objection, within(d)));
    assert!(ix.retractions().within(&objection, within(later)), "{unobject} is in a later past");
}

impl World {
    /// A proposal by alice on the line from `base`, whose change takes `from` to `to`.
    fn propose(&mut self, base: ContributionId, from: &[(&str, &[u8])], to: &[(&str, &[u8])]) -> ContributionId {
        let new = self.tree(to);
        let change = Change::between(&manifest(from), &manifest(to)).hash();
        let p = Proposal { v: VERSION, line: self.line, base, manifest: new.0, change, summary: "p".into() };
        let alice = self.alice.clone();
        self.add(&alice, artifact(&p))
    }

    fn retract(&mut self, who: &Identity, target: ContributionId) -> ContributionId {
        self.add(who, Body::Retract { target, reason: "r".into() })
    }
}

fn review_ids(list: Vec<(&Review, Target)>) -> Vec<ContributionId> {
    list.iter().map(|(r, _)| r.id).collect()
}

/// Finding 06bbc6a34636, the reviewer's case: an objection to a manifest artifact attaches to the
/// manifest. Its author writing a second artifact of the same manifest, deciding on it and retracting
/// the first clears nothing: the objection still reaches the decision, and its target still resolves.
#[test]
fn retracting_the_manifest_artifact_an_objection_targets_clears_nothing() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let files: [(&str, &[u8]); 1] = [("a", b"0")];
    let first = w.tree(&files);
    let why = w.add(&bob, Body::Claim { statement: "it breaks b".into(), tags: vec![] });
    let objection =
        w.add(&bob, Body::Relation { from: why, kind: RelationKind::Contradicts, to: first.1, note: "no".into() });
    let second = w.add(&alice, artifact(&manifest(&files)));
    let d = w.decide(first.0, &[]);
    w.retract(&alice, first.1);
    let (ix, lines, _) = w.checks();
    assert_eq!(ix.manifest_named(&first.1), Some(first.0), "a retracted artifact still names its manifest");
    assert_eq!(ix.manifest_named(&second), Some(first.0));
    assert_eq!(Target::of(&ix, &lines, &first.1), Some(Target::Manifest(first.1, first.0)));
    let reviews = Reviews::build(&ix, &lines);
    assert_eq!(review_ids(reviews.of_decision(&lines, &d)), [objection]);
    assert_eq!(review_ids(reviews.of_manifest(&first.0)), [objection]);
    // With every artifact of the manifest retracted too.
    w.retract(&alice, second);
    let (ix, lines, _) = w.checks();
    assert_eq!(review_ids(Reviews::build(&ix, &lines).of_decision(&lines, &d)), [objection]);
}

/// Finding 06bbc6a34636, the proposal case: an objection to a proposal carries over a rebase with the
/// same change, and retracting the proposal it targets clears it from neither.
#[test]
fn retracting_the_proposal_an_objection_targets_clears_nothing() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let base0: [(&str, &[u8]); 1] = [("a", b"0")];
    let m0 = w.tree(&base0);
    let d0 = w.decide(m0.0, &[]);
    let p1 = w.propose(d0, &base0, &[("a", b"0"), ("b", b"1")]);
    let objection =
        w.add(&bob, Body::Relation { from: m0.1, kind: RelationKind::Contradicts, to: p1, note: "no".into() });
    let base1: [(&str, &[u8]); 2] = [("a", b"0"), ("c", b"2")];
    let m1 = w.tree(&base1);
    let d1 = w.decide(m1.0, &[d0]);
    let p2 = w.propose(d1, &base1, &[("a", b"0"), ("b", b"1"), ("c", b"2")]);
    w.retract(&alice, p1);
    let (ix, lines, _) = w.checks();
    assert!(lines.proposal(&p1).is_none(), "views of now leave the retracted proposal out");
    let named = lines.proposal_named(&p1).expect("its payload is kept");
    let (line, change) = (named.line, named.change);
    assert_eq!(lines.proposal(&p2).expect("the rebase").proposal.change, change);
    assert_eq!(Target::of(&ix, &lines, &p1), Some(Target::Proposal { id: p1, line, change }));
    let reviews = Reviews::build(&ix, &lines);
    assert_eq!(review_ids(reviews.of_proposal(&lines, &p2)), [objection], "carried over the rebase");
    assert_eq!(review_ids(reviews.of_proposal(&lines, &p1)), [objection]);
}

/// Finding 1c584afefc50: a retraction that is itself retracted counts for nothing (docs/protocol.md,
/// *Retraction*), for reviews, goals and results alike, now and within a decision's past; and
/// retracting that again retracts the contribution again.
#[test]
fn a_retracted_retraction_restores_reviews_goals_and_results() {
    let mut w = World::new();
    let (alice, bob) = (w.alice.clone(), w.bob.clone());
    let p0 = w.line;
    let goal = w.add(&alice, Body::Claim { statement: "fast".into(), tags: vec![GOAL_TAG.into()] });
    w.add(&alice, Body::Relation { from: goal, kind: RelationKind::Refines, to: w.line, note: String::new() });
    let m = w.tree(&[("a", b"0")]);
    let fail = w.result(&bob, m, ARGV, false, 1, None);
    let d = w.decide(m.0, &[]);
    let objection =
        w.add(&bob, Body::Relation { from: fail, kind: RelationKind::Contradicts, to: d, note: "fails".into() });
    let gone = [w.retract(&alice, goal), w.retract(&bob, objection), w.retract(&bob, fail)];
    let d_gone = w.decide(m.0, &[d]);
    let state = |w: &World, asker: &ContributionId| {
        let (ix, lines, _) = w.checks();
        let objected = !Reviews::build(&ix, &lines).of_decision(&lines, &d).is_empty();
        (goals(&ix, &w.line).len(), objected, w.counted(&d).fails.len(), w.past(asker, &p0).fails.len())
    };
    assert_eq!(state(&w, &d_gone), (0, false, 0, 0), "all three retracted");
    let restored: Vec<ContributionId> =
        [(&alice, gone[0]), (&bob, gone[1]), (&bob, gone[2])].into_iter().map(|(who, r)| w.retract(who, r)).collect();
    let d_back = w.decide(m.0, &[d_gone]);
    assert_eq!(state(&w, &d_back), (1, true, 1, 1), "all three restored");
    assert_eq!(w.past(&d_gone, &p0).fails.len(), 0, "a decision before the restoring still saw the fail retracted");
    for (who, r) in [(&alice, restored[0]), (&bob, restored[1]), (&bob, restored[2])] {
        w.retract(who, r);
    }
    let d_again = w.decide(m.0, &[d_back]);
    assert_eq!(state(&w, &d_again), (0, false, 0, 0), "retracted again");
    assert_eq!(w.past(&d_back, &p0).fails.len(), 1, "d_back's past is as it was");
    let (ix, _, _) = w.checks();
    for id in [goal, objection, fail] {
        assert_eq!(ix.retractions().within(&id, |_| true), w.g.is_retracted(&id), "{id}");
    }
}

/// A long chain of retractions is judged as the graph judges it, and without recursion.
#[test]
fn a_long_chain_of_retractions_is_judged_as_the_graph_judges_it() {
    let mut w = World::new();
    let bob = w.bob.clone();
    let m = w.tree(&[("a", b"0")]);
    let fail = w.result(&bob, m, ARGV, false, 1, None);
    let mut chain = vec![fail];
    for _ in 0..2_001 {
        let r = w.retract(&bob, *chain.last().expect("one"));
        chain.push(r);
    }
    let (ix, _, _) = w.checks();
    assert_eq!(ix.retractions().below(&fail).len(), 2_001);
    for id in [fail, chain[1], chain[1_000], chain[2_000]] {
        assert_eq!(ix.retractions().within(&id, |_| true), w.g.is_retracted(&id), "{id}");
    }
    assert!(w.g.is_retracted(&fail), "an odd chain retracts it");
    // Halfway along the chain, as a past that holds only its first 1,000 retractions sees it.
    let first: std::collections::HashSet<ContributionId> = chain[1..=1_000].iter().copied().collect();
    assert!(!ix.retractions().within(&fail, |x| first.contains(x)));
}

/// Finding eb8b992ae58f: the latest rule within decisions' pasts asks which of a key's results lie below
/// which once per key, however many decisions are counted: ten decisions take the same passes as one.
/// (Measured in a release build: 8,000 results on one manifest, ten decisions, about 0.3 s; weave.md
/// section 4.)
#[test]
fn the_latest_rule_costs_the_same_passes_for_many_decisions_as_for_one() {
    const N: u64 = 300;
    let stats = |q: usize| {
        let mut w = World::new();
        let bob = w.bob.clone();
        let m = w.tree(&[("a", b"0")]);
        for attempt in 1..=N {
            w.result(&bob, m, ARGV, false, attempt, None);
        }
        let mut previous = vec![];
        let mut queries = vec![];
        for _ in 0..q {
            let d = w.decide(m.0, &previous);
            previous = vec![d];
            queries.push(PastQuery { decision: d, manifest: m.0 });
        }
        let (ix, lines, checks) = w.checks();
        let (counted, stats) = checks.count_in_pasts(&w.g, &ix, &lines, &queries);
        assert!(counted.iter().all(|c| c.as_ref().expect("a policy")[0].fails.len() == 1));
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
        stats
    };
    let (one, ten) = (stats(1), stats(10));
    assert_eq!(one.passes, ten.passes, "{one:?} {ten:?}");
    assert_eq!(one.passes as u64, 2 * N.div_ceil(64), "a pass per 64 results for each stage: {one:?}");
}
