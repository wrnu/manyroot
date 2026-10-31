//! The pipeline against manyroot, the oracle, on a small Ground holding the hard cases: a retracted
//! retraction, a supersession by the subject's author and one by someone else, a retracted supersession,
//! and relations and evidence about hidden contributions. The comparison with the CLI itself is
//! `query-lab bench`, which needs a bracken binary.

use std::collections::HashSet;

use bracken_query_lab::ground::{Ground, Row};
use bracken_query_lab::pipeline::{self, parse};
use manyroot::validation::open_supersessors;
use manyroot::{Body, ContributionId, RelationKind, Store, Verdict};

fn claim(s: &mut Store, who: &manyroot::Identity, text: &str, tags: &[&str]) -> ContributionId {
    let tags = tags.iter().map(|t| (*t).to_owned()).collect();
    s.contribute(who, Body::Claim { statement: text.to_owned(), tags }).expect("contributes")
}

fn relate(
    s: &mut Store,
    who: &manyroot::Identity,
    from: ContributionId,
    kind: RelationKind,
    to: ContributionId,
) -> ContributionId {
    s.contribute(who, Body::Relation { from, kind, to, note: String::new() }).expect("contributes")
}

fn retract(s: &mut Store, who: &manyroot::Identity, target: ContributionId) -> ContributionId {
    s.contribute(who, Body::Retract { target, reason: "test".into() }).expect("contributes")
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    cursor: ContributionId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut s = Store::init(dir.path(), "lab").expect("a Ground");
    let alice = s.create_identity("alice").expect("alice");
    let bob = s.create_identity("bob").expect("bob");

    let a1 = claim(&mut s, &alice, "a1", &["pr"]);
    let a2 = claim(&mut s, &alice, "a2 replaces a1", &["pr"]);
    relate(&mut s, &alice, a2, RelationKind::Supersedes, a1); // counts: a1 hidden
    let b1 = claim(&mut s, &bob, "b1", &["pr", "finding"]);
    relate(&mut s, &bob, b1, RelationKind::Contradicts, a2);
    relate(&mut s, &bob, b1, RelationKind::Supersedes, a2); // another author's: doesn't count
    let cursor = s
        .contribute(
            &bob,
            Body::Evidence { target: a2, verdict: Verdict::Pass, method: "read it".into(), details: String::new() },
        )
        .expect("contributes");

    let c1 = claim(&mut s, &alice, "c1", &["decision"]);
    let c2 = claim(&mut s, &alice, "c2", &["decision"]);
    let sup = relate(&mut s, &alice, c2, RelationKind::Supersedes, c1);
    retract(&mut s, &alice, sup); // the supersession is withdrawn: c1 current again
    let d1 = claim(&mut s, &bob, "d1", &["finding"]);
    let r1 = retract(&mut s, &bob, d1);
    let r2 = retract(&mut s, &bob, r1); // d1 restored
    retract(&mut s, &bob, r2); // and withdrawn again
    let e1 = claim(&mut s, &alice, "e1", &[]);
    let e2 = claim(&mut s, &alice, "e2", &[]);
    relate(&mut s, &alice, e2, RelationKind::Supersedes, e1);
    retract(&mut s, &alice, e2); // the replacement isn't live: e1 current again
    relate(&mut s, &bob, d1, RelationKind::Refines, e1); // refers to the withdrawn d1: hidden
    Fixture { _dir: dir, store: s, cursor }
}

fn ground(f: &Fixture) -> Ground {
    let names: Vec<_> = f.store.agents().expect("agents").into_iter().map(|e| (e.name, e.agent)).collect();
    Ground::from_graph(f.store.graph(), &names)
}

fn ids(g: &Ground, rows: &[Row]) -> Vec<ContributionId> {
    rows.iter().map(|r| g.ids[*r as usize]).collect()
}

#[test]
fn rows_are_causal_order() {
    let f = fixture();
    let g = ground(&f);
    assert_eq!(g.ids, f.store.graph().topo_order());
}

#[test]
fn retracted_agrees_with_manyroot() {
    let f = fixture();
    let g = ground(&f);
    let ours = g.retracted();
    for (row, id) in g.ids.iter().enumerate() {
        assert_eq!(ours[row], f.store.graph().is_retracted(id), "row {row}");
    }
}

/// `current` against the CLI's rule, written here from manyroot's own pieces.
#[test]
fn current_agrees_with_manyroot() {
    let f = fixture();
    let graph = f.store.graph();
    let g = ground(&f);
    let mut hidden: HashSet<ContributionId> = HashSet::new();
    for id in graph.topo_order() {
        let c = graph.get(&id).expect("present");
        let superseded = c.body().is_subject() && !open_supersessors(graph, &id).is_empty();
        if graph.is_retracted(&id) || superseded || c.body().references().iter().any(|r| hidden.contains(r)) {
            hidden.insert(id);
        }
    }
    let expected: Vec<ContributionId> = graph.topo_order().into_iter().filter(|id| !hidden.contains(id)).collect();
    let ours = pipeline::run(&g, &parse("current").expect("parses")).expect("runs");
    assert_eq!(ids(&g, &ours), expected);
    // The cases the fixture was built for.
    let text = |row: &Row| match &g.fact[*row as usize] {
        bracken_query_lab::ground::Fact::Claim { statement, .. } => statement.clone(),
        _ => String::new(),
    };
    let claims: Vec<String> = ours.iter().map(text).filter(|t| !t.is_empty()).collect();
    assert_eq!(claims, ["a2 replaces a1", "b1", "c1", "c2", "e1"]);
}

#[test]
fn since_a_cursor_is_what_it_has_not_seen() {
    let f = fixture();
    let graph = f.store.graph();
    let g = ground(&f);
    let ours = pipeline::run(&g, &parse(&format!("since {}", f.cursor)).expect("parses")).expect("runs");
    let expected: Vec<ContributionId> =
        graph.topo_order().into_iter().filter(|id| *id != f.cursor && !graph.is_ancestor(id, &f.cursor)).collect();
    assert_eq!(ids(&g, &ours), expected);
    let everything = pipeline::run(&g, &parse("since \"\"").expect("parses")).expect("runs");
    assert_eq!(everything.len(), g.len());
}

#[test]
fn filters_commute_with_the_planner() {
    let f = fixture();
    let g = ground(&f);
    let run = |q: &str| pipeline::run(&g, &parse(q).expect("parses")).expect("runs");
    assert_eq!(run("tag finding | current"), run("current | tag finding"));
    assert_eq!(run("author bob | kind claim | tag pr"), run("tag pr | author bob | kind claim"));
    let naive: Vec<Row> = (0..g.len() as Row)
        .filter(|r| {
            matches!(&g.fact[*r as usize], bracken_query_lab::ground::Fact::Claim { tags, .. }
                if tags.contains(&g.tag_of["pr"]))
        })
        .collect();
    assert_eq!(run("tag pr"), naive);
    assert!(run("tag nothing-has-this").is_empty());
    assert_eq!(run("tag pr | limit 1"), run("tag pr")[2..].to_vec());
}

#[test]
fn refs_agrees_with_manyroot() {
    let f = fixture();
    let graph = f.store.graph();
    let g = ground(&f);
    for id in graph.topo_order() {
        let mut expected: Vec<ContributionId> = graph.referrers(&id).map(|c| c.id()).collect();
        let order: Vec<ContributionId> = graph.topo_order();
        expected.sort_by_key(|x| order.iter().position(|o| o == x));
        expected.dedup();
        let ours = pipeline::run(&g, &parse(&format!("refs {id}")).expect("parses")).expect("runs");
        assert_eq!(ids(&g, &ours), expected);
    }
}

#[test]
fn bad_queries_are_refused() {
    let f = fixture();
    let g = ground(&f);
    assert!(parse("tagg pr").is_err());
    assert!(parse("tag").is_err());
    assert!(parse("limit x").is_err());
    assert!(pipeline::run(&g, &parse("author nobody").expect("parses")).is_err());
    // Ids that can't be there. A short prefix such as "00" won't do: the fixture's ids are random, so about
    // one run in 30 has exactly one id starting with it, and it resolves.
    assert!(pipeline::run(&g, &parse(&format!("refs {}", "0".repeat(64))).expect("parses")).is_err());
    assert!(pipeline::run(&g, &parse("refs zz").expect("parses")).is_err(), "not hex");
}

#[test]
fn since_a_date_is_what_was_stamped_then_or_later() {
    let f = fixture();
    let g = ground(&f);
    let run = |q: &str| pipeline::run(&g, &parse(q).expect("parses")).expect("runs");
    let mid = g.len() / 2;
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(g.timestamp_ms[mid]);
    let time = humantime::format_rfc3339_millis(at).to_string();
    let expected: Vec<Row> =
        (0..g.len() as Row).filter(|r| g.timestamp_ms[*r as usize] >= g.timestamp_ms[mid]).collect();
    assert!(expected.contains(&(mid as Row)));
    assert_eq!(run(&format!("since {time}")), expected);
    assert_eq!(run("since 1970-01-01").len(), g.len(), "a date: from its start, in UTC");
    assert!(run("since 2999-01-01").is_empty());
    assert!(pipeline::run(&g, &parse("since 2026-13-45").expect("parses")).is_err(), "not a date");
}
