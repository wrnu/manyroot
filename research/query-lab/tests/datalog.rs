//! The Datalog prelude against its oracles, on generated Grounds: manyroot's `Policy` for acceptance, the
//! pipeline (itself checked against manyroot in agree.rs) for `current`, manyroot's `is_ancestor` for
//! ancestry, and a copy of the CLI's `open_finding` for open findings. `query-lab bench` compares the same
//! queries with the real CLI on a real Ground.

use std::collections::{BTreeSet, HashSet};

use bracken_query_lab::datalog::{self, Options, Value};
use bracken_query_lab::generate::{self, Generated};
use bracken_query_lab::ground::Ground;
use bracken_query_lab::pipeline;
use manyroot::validation::open_supersessors;
use manyroot::{AgentId, Body, Contribution, ContributionId, Graph, Policy, RelationKind, Status, Verdict};

fn generated(seed: u64) -> (Generated, Ground) {
    let g = generate::ground(seed, 700, 5);
    let ground = Ground::from_graph(&g.graph, &g.names);
    (g, ground)
}

fn one_column(ground: &Ground, opts: &Options, query: &str) -> Vec<ContributionId> {
    let (_, answers) = datalog::run(ground, opts, query).expect("runs");
    datalog::ids(&answers[0]).iter().map(|r| ground.ids[*r as usize]).collect()
}

fn id_of(ground: &Ground, v: Value) -> ContributionId {
    let Value::Id(row) = v else { panic!("{v:?} isn't a contribution") };
    ground.ids[row as usize]
}

#[test]
fn the_generator_makes_every_case() {
    let (g, ground) = generated(1);
    let kinds: HashSet<_> = g.graph.iter().map(|c| c.body().kind()).collect();
    assert_eq!(kinds.len(), 5, "every body kind");
    let others_supersede = g
        .graph
        .iter()
        .filter(|c| {
            matches!(c.body(), Body::Relation { kind: RelationKind::Supersedes, to, .. }
            if g.graph.get(to).is_some_and(|t| t.author() != c.author()))
        })
        .count();
    assert!(others_supersede > 0, "a supersession by someone else");
    let branches = g.graph.iter().filter(|c| c.parents().len() > 1).count();
    assert!(branches > 0, "concurrent branches merged");
    let (_, answers) = datalog::run(&ground, &Options::default(), "?- open_finding(F).").expect("runs");
    assert!(!answers[0].rows.is_empty(), "some open findings");
}

#[test]
fn current_agrees_with_the_pipeline() {
    for seed in 1..4 {
        let (_, ground) = generated(seed);
        let ours = one_column(&ground, &Options::default(), "?- current(X).");
        let pipe: Vec<ContributionId> = pipeline::run(&ground, &pipeline::parse("current").expect("parses"))
            .expect("runs")
            .iter()
            .map(|r| ground.ids[*r as usize])
            .collect();
        assert_eq!(ours, pipe, "seed {seed}");
    }
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Accepted => "accepted",
        Status::NeedsRecheck => "needs_recheck",
        Status::Superseded => "superseded",
        Status::Pending => "pending",
        Status::Rejected => "rejected",
        Status::Retracted => "retracted",
    }
}

#[test]
fn statuses_and_the_accepted_layer_agree_with_manyroot() {
    for seed in 1..4 {
        let (g, ground) = generated(seed);
        let key = |name: &str| g.names.iter().find(|(n, _)| n == name).map(|(_, a)| *a).expect("named");
        let validators = vec!["agent-1".to_owned(), "agent-2".to_owned()];
        let policies: Vec<(Policy, Options)> = vec![
            (Policy::default(), Options::default()),
            (Policy { min_passes: 2, ..Policy::default() }, Options { min_passes: 2, ..Options::default() }),
            (
                Policy {
                    validators: Some(validators.iter().map(|v| key(v)).collect::<BTreeSet<AgentId>>()),
                    ..Policy::default()
                },
                Options { validators: &validators, ..Options::default() },
            ),
        ];
        for (n, (policy, opts)) in policies.iter().enumerate() {
            let (db, answers) = datalog::run(&ground, opts, "?- status(X, S).").expect("runs");
            let ours: Vec<(ContributionId, String)> = answers[0]
                .rows
                .iter()
                .map(|r| (id_of(&ground, r[0]), datalog::show(&ground, &db.syms, r[1])))
                .collect();
            let assessor = policy.assessor(&g.graph);
            let mut expected: Vec<(ContributionId, String)> = ground
                .ids
                .iter()
                .filter_map(|id| assessor.assess(id).map(|a| (*id, status_name(a.status).to_owned())))
                .collect();
            let row = |id: &ContributionId| ground.row_of[id];
            expected.sort_by_key(|(id, s)| (row(id), s.clone()));
            let mut ours = ours;
            ours.sort_by_key(|(id, s)| (row(id), s.clone()));
            assert_eq!(ours, expected, "seed {seed}, policy {n}");
            let accepted = one_column(&ground, opts, "?- accepted(X).");
            let layer: Vec<ContributionId> = assessor.evaluate().accepted.iter().map(|a| a.id).collect();
            assert_eq!(accepted, layer, "seed {seed}, policy {n}");
        }
    }
}

#[test]
fn ancestry_agrees_with_manyroot() {
    let g = generate::ground(7, 300, 4);
    let ground = Ground::from_graph(&g.graph, &g.names);
    for row in [10usize, 150, 299] {
        let d = ground.ids[row.min(ground.len() - 1)];
        let ids = [("d".to_owned(), d.to_hex())];
        let opts = Options { ids: &ids, ..Options::default() };
        let ours: HashSet<ContributionId> = one_column(&ground, &opts, "?- ancestor(A, $d).").into_iter().collect();
        let expected: HashSet<ContributionId> =
            ground.ids.iter().filter(|a| g.graph.is_ancestor(a, &d)).copied().collect();
        assert_eq!(ours, expected, "row {row}");
    }
}

// ---- The CLI's open_finding, copied from crates/bracken/src/main.rs as the oracle ----

struct CurrentView {
    hidden: HashSet<ContributionId>,
}

impl CurrentView {
    fn new(g: &Graph) -> CurrentView {
        let mut hidden = HashSet::new();
        for id in g.topo_order() {
            let c = g.get(&id).expect("ordered ids exist");
            let superseded = c.body().is_subject() && !open_supersessors(g, &id).is_empty();
            if g.is_retracted(&id) || superseded || c.body().references().iter().any(|r| hidden.contains(r)) {
                hidden.insert(id);
            }
        }
        CurrentView { hidden }
    }
}

fn merged(g: &Graph, triager: AgentId, id: &ContributionId) -> bool {
    g.referrers(id).any(|r| {
        r.author() == triager
            && !g.is_retracted(&r.id())
            && matches!(r.body(), Body::Evidence { target, verdict: Verdict::Pass, method, .. }
                if target == id && method.starts_with("merge gate"))
    })
}

fn open_finding(g: &Graph, view: &CurrentView, triager: AgentId, c: &Contribution) -> bool {
    if g.is_retracted(&c.id()) {
        return false;
    }
    let (mut against_current, mut triaged, mut confirmed, mut deferred) = (false, false, false, false);
    for r in g.referrers(&c.id()) {
        let live = !g.is_retracted(&r.id());
        match r.body() {
            Body::Relation { from, kind: RelationKind::Contradicts, to, .. } if *from == c.id() => {
                let counts = live || (r.author() != c.author() && r.author() != triager);
                against_current |=
                    counts && g.get(to).is_some() && !view.hidden.contains(to) && !merged(g, triager, to);
            }
            Body::Evidence { target, verdict, .. } if *target == c.id() && live && r.author() == triager => {
                triaged = true;
                confirmed |= *verdict == Verdict::Pass;
            }
            Body::Relation { from, kind: RelationKind::DependsOn, to, .. }
                if *to == c.id() && live && r.author() == triager =>
            {
                deferred |= g.get(from).is_some_and(|t| {
                    t.author() == triager
                        && !view.hidden.contains(from)
                        && matches!(t.body(), Body::Claim { tags, .. } if tags.iter().any(|x| x == "task")
                            && tags.iter().any(|x| x == "deferred"))
                });
            }
            _ => {}
        }
    }
    against_current && (!triaged || (confirmed && !deferred))
}

#[test]
fn open_findings_agree_with_the_clis_rule() {
    for seed in 1..6 {
        let (g, ground) = generated(seed);
        let triager = g.names[0].1;
        let view = CurrentView::new(&g.graph);
        let expected: Vec<ContributionId> = g
            .graph
            .topo_order()
            .into_iter()
            .filter(|id| open_finding(&g.graph, &view, triager, g.graph.get(id).expect("present")))
            .collect();
        let ours = one_column(&ground, &Options::default(), "?- open_finding(F).");
        assert_eq!(ours, expected, "seed {seed}");
        assert!(!expected.is_empty(), "seed {seed} has open findings to compare");
    }
}

#[test]
fn queries_compose_with_the_prelude() {
    let (_, ground) = generated(2);
    let (db, answers) = datalog::run(
        &ground,
        &Options::default(),
        "mine(X) :- current(X), tag(X, \"pr\"), author(X, A), agent(A, manager).\n?- mine(X).\n?- current(X), tag(X, \"pr\").",
    )
    .expect("runs");
    let mine: HashSet<Value> = answers[0].rows.iter().map(|r| r[0]).collect();
    let all: HashSet<Value> = answers[1].rows.iter().map(|r| r[0]).collect();
    assert!(mine.is_subset(&all) && !mine.is_empty());
    assert!(db.rels.contains_key("hidden"), "derived relations stay for later queries");
}

#[test]
fn bad_programs_are_refused() {
    let (_, ground) = generated(1);
    let run = |q: &str| datalog::run(&ground, &Options::default(), q).map(|_| ());
    assert!(
        run("p(X) :- contribution(X, _), !q(X).\nq(X) :- contribution(X, _), !p(X).\n?- p(X).").is_err(),
        "unstratified"
    );
    assert!(run("p(X, Y) :- contribution(X, _).\n?- p(X, Y).").is_err(), "unbound head variable");
    assert!(run("p(X) :- !retracted(X).\n?- p(X).").is_err(), "negation binds nothing");
    assert!(run("?- nothing(X).").is_err(), "unknown relation");
    assert!(run("?- contribution(X).").is_err(), "wrong arity");
    assert!(run("?- gate_current(C).").is_err(), "a parameter without a value");
}

#[test]
fn the_demand_transformation_changes_no_answer_and_derives_less() {
    let (_, ground) = generated(3);
    let d = ground.ids[ground.len() / 2].to_hex();
    let ids = [("d".to_owned(), d)];
    for query in ["?- ancestor(A, $d).", "?- later(T, S), T = $d.", "?- contribution($d, K), ancestor(A, $d)."] {
        let with = Options { ids: &ids, ..Options::default() };
        let without = Options { ids: &ids, magic: false, ..Options::default() };
        let (_, a) = datalog::run(&ground, &with, query).expect("runs");
        let (_, b) = datalog::run(&ground, &without, query).expect("runs");
        assert_eq!(a[0].rows, b[0].rows, "{query}");
        if query.starts_with("?- ancestor") {
            assert!(
                a[0].stats.derived * 10 < b[0].stats.derived,
                "{query}: {} vs {}",
                a[0].stats.derived,
                b[0].stats.derived
            );
        }
    }
}

#[test]
fn a_session_answers_as_fresh_runs_do_whatever_ran_before() {
    let (_, ground) = generated(4);
    let mut session = datalog::Session::new(&ground).expect("builds");
    let a = ground.ids[ground.len() / 3].to_hex();
    let b = ground.ids[ground.len() / 2].to_hex();
    // Each: its id parameters, its min passes, and the program.
    type Program<'a> = (Vec<(String, String)>, i64, &'a str);
    let programs: Vec<Program> = vec![
        (vec![("d".into(), a.clone())], 1, "?- ancestor(X, $d)."),
        (vec![("d".into(), b.clone())], 1, "?- ancestor(X, $d)."),
        (vec![], 2, "?- accepted(X)."),
        (vec![], 1, "?- accepted(X)."),
        (vec![("claim".into(), a)], 1, "?- gate_blocking(F).\n?- gate_current(C)."),
        (vec![("claim".into(), b)], 1, "?- gate_blocking(F).\n?- gate_current(C)."),
    ];
    for (ids, min_passes, text) in &programs {
        let opts = Options { ids, min_passes: *min_passes, ..Options::default() };
        let ours = session.run(&opts, text).expect("runs");
        let (_, fresh) = datalog::run(&ground, &opts, text).expect("runs");
        assert_eq!(ours.len(), fresh.len(), "{text}: as many answers");
        for (x, y) in ours.iter().zip(&fresh) {
            assert_eq!(x.rows, y.rows, "{text} with {ids:?}, min passes {min_passes}");
        }
    }
}
