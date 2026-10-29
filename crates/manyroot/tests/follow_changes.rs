//! Acceptance follows changes (decision `2b91a9541aba`, docs/design/acceptance-follows-changes.md): under
//! `Policy::follow_changes`, an accepted subject whose dependencies were superseded or retracted after its
//! passes is `needs_recheck`, and out of the accepted layer.

use std::path::Path;

use manyroot::validation::dependencies;
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

fn artifact(name: &str) -> Body {
    Body::Artifact { name: name.into(), media_type: "text/plain".into(), content: name.as_bytes().to_vec() }
}

fn relate(from: ContributionId, kind: RelationKind, to: ContributionId) -> Body {
    Body::Relation { from, kind, to, note: String::new() }
}

fn pass(target: ContributionId) -> Body {
    Body::Evidence { target, verdict: Verdict::Pass, method: "test".into(), details: String::new() }
}

/// Adds a contribution on top of all current heads: its author has seen everything.
fn add(g: &mut Graph, who: &Identity, body: Body) -> ContributionId {
    let heads: Vec<_> = g.heads().collect();
    let c = Contribution::create(g.ground(), who, heads, 0, body);
    let id = c.id();
    assert!(g.insert(c).unwrap());
    id
}

/// Adds a contribution that has seen only what its body references.
fn add_detached(g: &mut Graph, who: &Identity, body: Body) -> ContributionId {
    let c = Contribution::create(g.ground(), who, [], 0, body);
    let id = c.id();
    g.insert(c).unwrap();
    id
}

fn follow() -> Policy {
    Policy { follow_changes: true, ..Policy::default() }
}

fn status(g: &Graph, p: &Policy, id: &ContributionId) -> Status {
    p.assess(g, id).expect("a subject").status
}

/// A spec claim by the setter, an implementation of it by the writer, a pass by the validator, and the spec
/// superseded by the setter after the pass.
struct Scene {
    g: Graph,
    setter: Identity,
    writer: Identity,
    validator: Identity,
    spec: ContributionId,
    work: ContributionId,
}

fn scene() -> Scene {
    let (setter, writer, validator) = (agent("setter"), agent("writer"), agent("validator"));
    let mut g = Graph::new(ground());
    let spec = add(&mut g, &setter, claim("abs returns the magnitude"));
    let work = add(&mut g, &writer, artifact("abs"));
    add(&mut g, &writer, relate(work, RelationKind::Implements, spec));
    add(&mut g, &validator, pass(work));
    Scene { g, setter, writer, validator, spec, work }
}

#[test]
fn a_superseded_dependency_after_the_pass_asks_for_a_recheck_only_when_opted_in() {
    let mut s = scene();
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Accepted, "nothing changed yet");
    let new = add(&mut s.g, &s.setter, claim("abs saturates at the largest value"));
    let change = add(&mut s.g, &s.setter, relate(new, RelationKind::Supersedes, s.spec));

    let off = Policy::default();
    assert!(!off.follow_changes, "off by default");
    assert_eq!(status(&s.g, &off, &s.work), Status::Accepted);
    assert!(off.assess(&s.g, &s.work).unwrap().changes.is_empty());
    assert!(off.evaluate(&s.g).accepted.iter().any(|a| a.id == s.work));

    let a = follow().assess(&s.g, &s.work).unwrap();
    assert_eq!(a.status, Status::NeedsRecheck);
    assert_eq!(a.changes, [change], "the supersession its pass hadn't seen");
    assert!(!follow().evaluate(&s.g).accepted.iter().any(|a| a.id == s.work), "out of the accepted layer");
    assert_eq!(serde_json::to_value(a.status).unwrap(), "needs_recheck");
}

#[test]
fn only_a_change_after_the_evidence_counts_by_causal_ancestry() {
    let (setter, writer, validator) = (agent("setter"), agent("writer"), agent("validator"));
    let mut g = Graph::new(ground());
    let spec = add(&mut g, &setter, claim("abs returns the magnitude"));
    let work = add(&mut g, &writer, artifact("abs"));
    add(&mut g, &writer, relate(work, RelationKind::Implements, spec));
    let new = add(&mut g, &setter, claim("abs saturates"));
    add(&mut g, &setter, relate(new, RelationKind::Supersedes, spec));
    // The validator had seen the supersession when passing: the pass speaks to it.
    add(&mut g, &validator, pass(work));
    assert_eq!(status(&g, &follow(), &work), Status::Accepted);

    // Work written and passed without seeing it doesn't, however late it arrives.
    let other = add_detached(&mut g, &writer, artifact("abs, again"));
    add_detached(&mut g, &writer, relate(other, RelationKind::Implements, spec));
    let late = add_detached(&mut g, &validator, pass(other));
    assert!(!g.is_ancestor(&new, &late));
    assert_eq!(status(&g, &follow(), &other), Status::NeedsRecheck);
}

#[test]
fn a_pass_that_has_seen_the_change_clears_it_and_with_two_needed_both_must() {
    let mut s = scene();
    let new = add(&mut s.g, &s.setter, claim("abs saturates"));
    add(&mut s.g, &s.setter, relate(new, RelationKind::Supersedes, s.spec));
    let two = Policy { min_passes: 2, ..follow() };
    let second = agent("second");
    add(&mut s.g, &second, pass(s.work));
    assert_eq!(status(&s.g, &two, &s.work), Status::NeedsRecheck, "one fresh pass of two needed");
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Accepted, "the fresh pass clears it");
    add(&mut s.g, &s.validator, pass(s.work));
    assert_eq!(status(&s.g, &two, &s.work), Status::Accepted);
    assert!(two.assess(&s.g, &s.work).unwrap().changes.is_empty());
}

#[test]
fn retracting_the_supersession_undoes_the_change_and_a_retracted_dependency_is_one() {
    let mut s = scene();
    let new = add(&mut s.g, &s.setter, claim("abs saturates"));
    let change = add(&mut s.g, &s.setter, relate(new, RelationKind::Supersedes, s.spec));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::NeedsRecheck);
    add(&mut s.g, &s.setter, Body::Retract { target: change, reason: "not yet".into() });
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Accepted, "accepted on its old evidence again");

    // Retracting the replacement undoes it too: the supersession needs a live replacement.
    add(&mut s.g, &s.setter, relate(new, RelationKind::Supersedes, s.spec));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::NeedsRecheck);
    add(&mut s.g, &s.setter, Body::Retract { target: new, reason: "withdrawn".into() });
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Accepted);

    // Withdrawing a dependency is a change.
    let withdrawn = add(&mut s.g, &s.setter, Body::Retract { target: s.spec, reason: "dropped".into() });
    let a = follow().assess(&s.g, &s.work).unwrap();
    assert_eq!((a.status, a.changes), (Status::NeedsRecheck, vec![withdrawn]));
}

#[test]
fn the_closure_is_transitive_over_depends_on_implements_and_refines() {
    let mut s = scene();
    // The spec depends on another spec, which refines the task: a change two or three steps away counts.
    let task = add(&mut s.g, &s.setter, claim("the library"));
    let sign = add(&mut s.g, &s.setter, claim("sign returns -1, 0 or 1"));
    add(&mut s.g, &s.setter, relate(s.spec, RelationKind::DependsOn, sign));
    add(&mut s.g, &s.setter, relate(sign, RelationKind::Refines, task));
    let tests = add(&mut s.g, &s.setter, claim("a test of abs"));
    add(&mut s.g, &s.setter, relate(s.spec, RelationKind::Tests, tests));
    let mut want = vec![s.spec, sign, task];
    want.sort();
    assert_eq!(dependencies(&s.g, &s.work), want, "not through tests");
    add(&mut s.g, &s.validator, pass(s.work));

    let new_task = add(&mut s.g, &s.setter, claim("the library, revised"));
    add(&mut s.g, &s.setter, relate(new_task, RelationKind::Supersedes, task));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::NeedsRecheck, "three steps away");
    // Supersessions out of the closure don't count, and cycles are walked once.
    add(&mut s.g, &s.setter, relate(sign, RelationKind::DependsOn, s.spec));
    assert_eq!(dependencies(&s.g, &s.work), want);
}

/// Security note `9769ab37127d`, case 1: a stranger can't make a node implement their own claim, and so
/// can't hang dependencies of theirs on it.
#[test]
fn a_strangers_implements_onto_their_own_claim_doesnt_count() {
    let mut s = scene();
    let mallory = agent("mallory");
    let bait = add(&mut s.g, &mallory, claim("mallory's spec"));
    let hook = add(&mut s.g, &mallory, claim("mallory's dependency"));
    add(&mut s.g, &mallory, relate(s.work, RelationKind::Implements, bait));
    add(&mut s.g, &mallory, relate(bait, RelationKind::DependsOn, hook));
    // And straight onto the setter's spec, which isn't hers to extend.
    add(&mut s.g, &mallory, relate(s.spec, RelationKind::DependsOn, hook));
    add(&mut s.g, &mallory, relate(s.work, RelationKind::DependsOn, hook));
    assert_eq!(dependencies(&s.g, &s.work), [s.spec]);
    let swap = add(&mut s.g, &mallory, claim("mallory's dependency, changed"));
    add(&mut s.g, &mallory, relate(swap, RelationKind::Supersedes, hook));
    add(&mut s.g, &mallory, relate(swap, RelationKind::Supersedes, bait));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Accepted, "her changes force no recheck");
}

/// Security note `9769ab37127d`, case 2: the rule applies at every step. Past the setter's spec, only
/// edges by that node's author (or by the author of a claim it implements) count, so a third spec author's
/// edge from the setter's claim drops out, while their own claim's edges count.
#[test]
fn a_third_authors_edge_mid_chain_counts_only_from_their_own_node() {
    let mut s = scene();
    let carol = agent("carol");
    let sign = add(&mut s.g, &s.setter, claim("sign, the setter's"));
    add(&mut s.g, &s.setter, relate(s.spec, RelationKind::DependsOn, sign));
    let theirs = add(&mut s.g, &carol, claim("carol's helper spec"));
    let deeper = add(&mut s.g, &carol, claim("carol's deeper spec"));
    // Carol extends the setter's sign: not hers, so it doesn't count.
    add(&mut s.g, &carol, relate(sign, RelationKind::DependsOn, theirs));
    let mut want = vec![s.spec, sign];
    want.sort();
    assert_eq!(dependencies(&s.g, &s.work), want);
    // The setter makes sign depend on carol's spec: that edge counts, and so do carol's from her own claim.
    add(&mut s.g, &s.setter, relate(sign, RelationKind::DependsOn, theirs));
    add(&mut s.g, &carol, relate(theirs, RelationKind::DependsOn, deeper));
    let mut want = vec![s.spec, sign, theirs, deeper];
    want.sort();
    assert_eq!(dependencies(&s.g, &s.work), want);
    add(&mut s.g, &s.validator, pass(s.work));
    let changed = add(&mut s.g, &carol, claim("carol's deeper spec, changed"));
    add(&mut s.g, &carol, relate(changed, RelationKind::Supersedes, deeper));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::NeedsRecheck);
    // The writer's own depends_on counts at the root, as the claim's author's does.
    let w = &s.writer;
    let own = add(&mut s.g, w, claim("the writer's note"));
    add(&mut s.g, w, relate(s.work, RelationKind::DependsOn, own));
    assert!(dependencies(&s.g, &s.work).contains(&own));
}

/// Reviewer's note on PR #79 (`433005735918`): supersession comes first, so superseded work is
/// `superseded`, never recheck work, and lists no changes.
#[test]
fn superseded_work_is_superseded_not_recheck_work() {
    let mut s = scene();
    let new = add(&mut s.g, &s.setter, claim("abs saturates"));
    add(&mut s.g, &s.setter, relate(new, RelationKind::Supersedes, s.spec));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::NeedsRecheck);
    let redone = add(&mut s.g, &s.writer, artifact("abs, saturating"));
    add(&mut s.g, &s.writer, relate(redone, RelationKind::Implements, new));
    add(&mut s.g, &s.validator, pass(redone));
    add(&mut s.g, &s.writer, relate(redone, RelationKind::Supersedes, s.work));
    let a = follow().assess(&s.g, &s.work).unwrap();
    assert_eq!((a.status, a.superseded_by, a.changes), (Status::Superseded, vec![redone], vec![]));
    assert_eq!(status(&s.g, &follow(), &redone), Status::Accepted);
}

/// Security's note on PR #79 (`5c1e7b364f8a`): a replacement that needs a recheck still replaces. The old
/// version it superseded stays superseded, and doesn't come back into the accepted layer.
#[test]
fn a_replacement_needing_a_recheck_still_supersedes_the_old_version() {
    let mut s = scene();
    let redone = add(&mut s.g, &s.writer, artifact("abs, faster"));
    add(&mut s.g, &s.writer, relate(redone, RelationKind::Implements, s.spec));
    add(&mut s.g, &s.writer, relate(redone, RelationKind::Supersedes, s.work));
    let helper = add(&mut s.g, &s.writer, claim("the writer's helper"));
    add(&mut s.g, &s.writer, relate(redone, RelationKind::DependsOn, helper));
    add(&mut s.g, &s.validator, pass(redone));
    assert_eq!(status(&s.g, &follow(), &s.work), Status::Superseded);
    // The helper changes after the replacement's pass; the old version doesn't depend on it.
    let helper2 = add(&mut s.g, &s.writer, claim("the writer's helper, changed"));
    add(&mut s.g, &s.writer, relate(helper2, RelationKind::Supersedes, helper));
    assert!(!dependencies(&s.g, &s.work).contains(&helper));
    assert_eq!(status(&s.g, &follow(), &redone), Status::NeedsRecheck);
    let old = follow().assess(&s.g, &s.work).unwrap();
    assert_eq!((old.status, old.superseded_by), (Status::Superseded, vec![redone]));
    let layer = follow().evaluate(&s.g).accepted;
    assert!(!layer.iter().any(|a| a.id == s.work || a.id == redone), "neither is established");
}

/// Finding `73252cef945e`: a stranger's subject with many passes and many changes once cost passes ×
/// changes × graph size (11.7 s at 1,200 contributions). One sweep over the graph per 64 changes keeps it
/// well inside the bound; the pairwise walk took minutes here.
#[test]
fn many_passes_and_changes_cost_one_sweep_not_a_walk_per_pair() {
    let n = 400;
    let mallory = agent("mallory");
    let puppets: Vec<Identity> = (0..8).map(|i| agent(&format!("puppet{i}"))).collect();
    let mut g = Graph::new(ground());
    let d = add(&mut g, &mallory, claim("d"));
    let w = add(&mut g, &mallory, claim("w"));
    add(&mut g, &mallory, relate(w, RelationKind::DependsOn, d));
    for i in 0..n {
        add(&mut g, &puppets[i % puppets.len()], pass(w));
    }
    for i in 0..n {
        let r = add(&mut g, &mallory, claim(&format!("d, version {i}")));
        add(&mut g, &mallory, relate(r, RelationKind::Supersedes, d));
    }
    assert!(g.len() > 3 * n);
    let started = std::time::Instant::now();
    let a = follow().assess(&g, &w).unwrap();
    let evaluated = follow().evaluate(&g);
    let took = started.elapsed();
    assert_eq!((a.status, a.changes.len()), (Status::NeedsRecheck, n));
    assert!(!evaluated.accepted.iter().any(|a| a.id == w));
    assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
}

/// The same finding, across subjects: many subjects resting on one long chain whose far end changed once
/// cost each subject a walk of the chain. One assessor works out what each node reaches once.
#[test]
fn many_subjects_on_a_long_chain_cost_one_walk_not_one_each() {
    let (k, m) = (1000, 1000);
    let (mallory, puppet) = (agent("mallory"), agent("puppet"));
    let mut g = Graph::new(ground());
    let head = add(&mut g, &mallory, claim("c0"));
    let mut tail = head;
    for i in 1..m {
        let next = add(&mut g, &mallory, claim(&format!("c{i}")));
        add(&mut g, &mallory, relate(tail, RelationKind::DependsOn, next));
        tail = next;
    }
    let mut subjects = Vec::new();
    for i in 0..k {
        let w = add(&mut g, &mallory, claim(&format!("w{i}")));
        add(&mut g, &mallory, relate(w, RelationKind::DependsOn, head));
        add(&mut g, &puppet, pass(w));
        subjects.push(w);
    }
    let r = add(&mut g, &mallory, claim("the far end, changed"));
    add(&mut g, &mallory, relate(r, RelationKind::Supersedes, tail));
    let started = std::time::Instant::now();
    let p = follow();
    let evaluated = p.evaluate(&g);
    let assessor = p.assessor(&g);
    let rechecks = subjects.iter().filter(|w| assessor.assess(w).unwrap().status == Status::NeedsRecheck).count();
    let took = started.elapsed();
    assert_eq!(rechecks, k);
    assert!(!evaluated.accepted.iter().any(|a| subjects.contains(&a.id)));
    assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
}

/// On a dependency cycle, a subject's own changes aren't in its closure, as `dependencies` leaves it out;
/// the others' are.
#[test]
fn on_a_cycle_a_subjects_own_change_isnt_one_of_its_dependencies() {
    let (setter, validator) = (agent("setter"), agent("validator"));
    let mut g = Graph::new(ground());
    let a = add(&mut g, &setter, claim("a, a contract partner of b"));
    let b = add(&mut g, &setter, claim("b, a contract partner of a"));
    add(&mut g, &setter, relate(a, RelationKind::DependsOn, b));
    add(&mut g, &setter, relate(b, RelationKind::DependsOn, a));
    add(&mut g, &validator, pass(a));
    add(&mut g, &validator, pass(b));
    assert_eq!(dependencies(&g, &a), [b]);
    let a2 = add(&mut g, &setter, claim("a, revised"));
    let change = add(&mut g, &setter, relate(a2, RelationKind::Supersedes, a));
    assert_eq!(status(&g, &follow(), &a2), Status::Pending, "the replacement doesn't stand yet");
    assert_eq!(status(&g, &follow(), &a), Status::Accepted, "its own change isn't a dependency's");
    let other = follow().assess(&g, &b).unwrap();
    assert_eq!((other.status, other.changes), (Status::NeedsRecheck, vec![change]));
}

/// Off by default, every status and the accepted layer of the version 0.1.0 fixture Ground read as they
/// did before this option existed: the same JSON as `tests/fixtures/ground-v1-acceptance.json`, which was
/// recorded with the code before it (at `44da9245`), under one pass and under two.
#[test]
fn off_by_default_the_fixture_ground_reads_as_before() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let store = Store::open(&dir.join("ground-v1")).expect("the v1 fixture Ground opens");
    let before: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("ground-v1-acceptance.json")).unwrap()).unwrap();
    let g = store.graph();
    let mut ids: Vec<ContributionId> = g.iter().map(|c| c.id()).collect();
    ids.sort();
    for (name, p) in [("default", Policy::default()), ("two_passes", Policy { min_passes: 2, ..Policy::default() })] {
        assert!(!p.follow_changes);
        let assessments: Vec<_> = ids.iter().filter_map(|id| p.assess(g, id)).collect();
        let now = serde_json::json!({"assessments": assessments, "evaluation": p.evaluate(g)});
        assert_eq!(now, before[name], "{name}");
    }
}
