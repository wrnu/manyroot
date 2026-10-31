//! The coordination-Ground fixture test, with Weave linked in (docs/design/weave.md, section 5 *Keeping
//! the two scripts apart*; build step 8).
//!
//! `crates/manyroot/tests/fixtures/ground-v1` is the coordination format's fixture Ground, and
//! `ground-v1-acceptance.json` beside it every status and the accepted layer it reads as, under one pass
//! and under two. This crate links Weave into manyroot, as the pinned binary does: opened here, with
//! Weave's views built over it, the fixture must read exactly as recorded, and Weave must find nothing
//! of its own in it. So the binary gaining `weave` commands can't change how the coordination Ground
//! reads. If this fails, that is protocol class: don't update the recorded statuses to make it pass.

use std::path::{Path, PathBuf};

use bracken_tree::ground::Index;
use bracken_tree::lines::Lines;
use bracken_tree::results::Checks;
use bracken_tree::reviews::Reviews;
use manyroot::validation::Policy;
use manyroot::{ContributionId, Store};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../manyroot/tests/fixtures")
}

#[test]
fn the_coordination_fixture_reads_unchanged_with_weave_linked_in() {
    let store = Store::open(&fixtures().join("ground-v1")).expect("the v1 fixture Ground opens");
    let g = store.graph();

    // Weave's views over it, built first, find nothing of Weave's.
    let ix = Index::build(g);
    assert_eq!(ix.counted_marker(), None);
    assert_eq!(ix.problems(), &[]);
    // Its evidence names no Weave check, and no claim is a Weave goal. Index::reviews lists every supports
    // and contradicts relation, whatever it targets; the Reviews view below keeps only Weave targets.
    assert!(ix.check_results().is_empty() && ix.goal_links().is_empty());
    assert!(ix.file_hashes().is_empty());
    let lines = Lines::build(g, &ix);
    assert_eq!(lines.lines().count(), 0);
    assert_eq!(lines.default_line(), None);
    let checks = Checks::build(g, &ix, &lines);
    assert!(g.iter().all(|c| checks.result(&c.id()).is_none()));
    let reviews = Reviews::build(&ix, &lines);
    assert!(g.iter().all(|c| reviews.of_decision(&lines, &c.id()).is_empty()));

    // Every status and the accepted layer read as recorded.
    let recorded: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("ground-v1-acceptance.json")).expect("the recorded statuses"),
    )
    .expect("JSON");
    let mut ids: Vec<ContributionId> = g.iter().map(|c| c.id()).collect();
    ids.sort();
    for (name, p) in [("default", Policy::default()), ("two_passes", Policy { min_passes: 2, ..Policy::default() })] {
        let assessments: Vec<_> = ids.iter().filter_map(|id| p.assess(g, id)).collect();
        let now = serde_json::json!({"assessments": assessments, "evaluation": p.evaluate(g)});
        assert_eq!(now, recorded[name], "{name}");
    }
}
