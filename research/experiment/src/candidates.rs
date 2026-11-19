//! Known programs for every spec of each task: correct ones, subtly wrong
//! ones, and the updates each change request needs. The scripted agents and
//! the tests use them; no agent is shown them.
//!
//! Each spec's first correct program, with the others' first correct
//! programs, makes a library that is right everywhere under the original
//! specs. A wrong program fails at least one hidden case; the last wrong one of
//! each spec passes every public case, so a public check can't tell.

use crate::task::{Change, Task, Version};

pub mod phase2;

pub struct Candidates {
    pub name: &'static str,
    pub correct: &'static [&'static str],
    pub wrong: &'static [&'static str],
}

const fn c(name: &'static str, correct: &'static [&'static str], wrong: &'static [&'static str]) -> Candidates {
    Candidates { name, correct, wrong }
}

/// Phase 1's.
pub const CANDIDATES: &[Candidates] = &[
    c("abs", &["a 0 < a neg a ?", "a 0 > a a neg ?"], &["a", "a -7 > a 0 < a neg a ? a ?"]),
    c(
        "sign",
        &["a 0 > 1 a 0 < -1 0 ? ?", "a 0 < -1 a 0 = 0 1 ? ?"],
        &["a 0 > 1 -1 ?", "a 7 < a 0 > 1 a 0 < -1 0 ? ? 0 ?"],
    ),
    c("max2", &["a b > a b ?", "a b < b a ?"], &["a b < a b ?", "a b - 0 > a b ?"]),
    c("min2", &["a b < a b ?", "a b > b a ?"], &["a b > a b ?", "a b - 0 < a b ?"]),
    c("inc", &["a 1 +", "1 a +"], &["a 1 -", "a 6 > a a 1 + ?"]),
    c("dec", &["a 1 -", "a -1 +"], &["1 a -", "a -6 < a a 1 - ?"]),
    c("sq", &["a a *", "a dup *"], &["a a +", "a 6 > 36 a a * ?"]),
    c("iszero", &["a 0 =", "0 a ="], &["a 0 < 0 1 ?", "a a * 0 ="]),
    c(
        "clamp3",
        &["a b max2@ c min2@", "a b max2@ dup c < swap c ?"],
        &["a b min2@ c max2@", "a 7 < a b max2@ c min2@ a ?"],
    ),
    c(
        "median3",
        &["a b min2@ a b max2@ c min2@ max2@", "a b max2@ a c max2@ min2@ b c max2@ min2@"],
        &["a b max2@ c min2@", "c 7 < a b min2@ a b max2@ c min2@ max2@ c ?"],
    ),
    c("max3", &["a b max2@ c max2@", "a b c max2@ max2@"], &["a b max2@", "a -7 > a b max2@ c max2@ a ?"]),
    c("min3", &["a b min2@ c min2@", "a b c min2@ min2@"], &["a b min2@", "a 7 < a b min2@ c min2@ a ?"]),
    c("between", &["a b < 0 a c > 0 1 ? ?", "a b < a c > + 0 ="], &["a b > a c < *", "a 7 < a b < a c > + 0 = 0 ?"]),
    c("cmp", &["a b < -1 a b > 1 0 ? ?", "a b > a b < -"], &["b a - sign@", "a b - sign@"]),
    c("dist", &["a b - abs@", "b a - abs@"], &["a b -", "a b max2@ a b min2@ -"]),
    c(
        "nearer",
        &["a b dist@ a c dist@ > c b ?", "a c dist@ a b dist@ < c b ?"],
        &["a b dist@ a c dist@ < b c ?", "a 7 < a b dist@ a c dist@ > c b ? b ?"],
    ),
    c("sumsq", &["a sq@ b sq@ +", "b sq@ a sq@ +"], &["a b + sq@", "a 7 < a sq@ b sq@ + 0 ?"]),
    c("cube", &["a sq@ a *", "a a a * *"], &["a sq@ a +", "a 7 < a sq@ a * 0 ?"]),
    c(
        "poly",
        &["a sq@ 2 * a 3 * - 1 +", "a sq@ dup + a -3 * + 1 +"],
        &["a sq@ 2 * a 3 * + 1 +", "a 7 < a sq@ 2 * a 3 * - 1 + 0 ?"],
    ),
    c("mulsign", &["a abs@ b sign@ *", "b sign@ a abs@ *"], &["a b sign@ *", "b 7 < a abs@ b sign@ * 0 ?"]),
    c(
        "clampsq",
        &["a sq@ b c clamp3@", "a sq@ b max2@ c min2@"],
        &["a b c clamp3@ sq@", "a 7 < a sq@ b c clamp3@ 0 ?"],
    ),
    c("manhattan", &["a abs@ b abs@ +", "b abs@ a abs@ +"], &["a b + abs@", "a 7 < a abs@ b abs@ + 0 ?"]),
    c("path3", &["a b dist@ b c dist@ +", "b c dist@ a b dist@ +"], &["a c dist@", "a 7 < a b dist@ b c dist@ + 0 ?"]),
    c("relu", &["a 0 max2@", "a 0 < 0 a ?"], &["a abs@", "a 7 < a 0 max2@ 0 ?"]),
    c(
        "near2",
        &["a b manhattan@ c > 0 1 ?", "c a b manhattan@ < 0 1 ?"],
        &["a b manhattan@ c <", "c 7 < a b manhattan@ c > 0 1 ? 1 ?"],
    ),
    c(
        "snap",
        &["a b c clamp3@ b c nearer@", "a b max2@ c min2@ b c nearer@"],
        &["a b c nearer@", "a 7 < a b c clamp3@ b c nearer@ 0 ?"],
    ),
    c(
        "spread3",
        &["a b c max3@ a b c min3@ -", "a b max2@ c max2@ a b min2@ c min2@ -"],
        &["a b c max3@ a b c median3@ -", "a 7 < a b c max3@ a b c min3@ - 0 ?"],
    ),
    c(
        "energy",
        &["a b sumsq@ a b dist@ -", "a sq@ b sq@ + a b dist@ -"],
        &["a b sumsq@ a b dist@ +", "a 7 < a b sumsq@ a b dist@ - 0 ?"],
    ),
    c(
        "bucket",
        &["a sign@ a abs@ 0 3 clamp3@ *", "a abs@ 0 3 clamp3@ a sign@ *"],
        &["a 0 3 clamp3@", "a 7 < a sign@ a abs@ 0 3 clamp3@ * 0 ?"],
    ),
    c("flat3", &["a b c spread3@ iszero@", "a b c spread3@ 0 ="], &["a b =", "a 7 < a b c spread3@ iszero@ 1 ?"]),
];

/// The programs a change request needs in the [`correct_library`], beyond
/// those that follow it unchanged: its target, and every caller whose own code
/// must change.
pub struct Update {
    pub change: Change,
    pub programs: &'static [(&'static str, &'static str)],
}

/// Phase 1's.
pub const UPDATES: &[Update] = &[
    Update {
        change: Change::ClampOrder,
        programs: &[
            ("clamp3", "c a max2@ b min2@"),
            ("clampsq", "b c a sq@ clamp3@"),
            ("snap", "b c a clamp3@ b c nearer@"),
            ("bucket", "a sign@ 0 3 a abs@ clamp3@ *"),
        ],
    },
    Update { change: Change::SignOfZero, programs: &[("sign", "a 0 < -1 1 ?")] },
    Update { change: Change::NearerTies, programs: &[("nearer", "a b dist@ a c dist@ < b c ?")] },
    Update { change: Change::BetweenExclusive, programs: &[("between", "a b < 0 a c < 1 0 ? ?")] },
    Update {
        change: Change::AbsSaturates,
        programs: &[("abs", "a 0 < a neg a ? dup -1 > swap 9223372036854775807 ?")],
    },
    Update {
        change: Change::SpreadCounts,
        programs: &[("spread3", "a b c max3@ a b c min3@ - 1 +"), ("flat3", "a b c spread3@ 1 =")],
    },
];

/// A change request's target as the change has it: two correct and two subtly wrong programs, judged in
/// the [`correct_library`] of the original specs with that change alone.
pub struct Revision {
    pub change: Change,
    pub candidates: Candidates,
}

/// `task`'s candidates, one entry per spec.
pub fn of(task: Task) -> &'static [Candidates] {
    match task {
        Task::Phase1 => CANDIDATES,
        Task::Phase2 => phase2::CANDIDATES,
    }
}

/// `task`'s updates, at least one per change request, in the order they apply.
pub fn updates(task: Task) -> &'static [Update] {
    match task {
        Task::Phase1 => UPDATES,
        Task::Phase2 => phase2::UPDATES,
    }
}

/// `task`'s revisions, one per change request. Phase 1's runs had none, so it has none.
pub fn revisions(task: Task) -> &'static [Revision] {
    match task {
        Task::Phase1 => &[],
        Task::Phase2 => phase2::REVISIONS,
    }
}

pub fn candidates(task: Task, name: &str) -> &'static Candidates {
    of(task).iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no candidates for {name}"))
}

/// One program per spec, by name.
pub type Programs = std::collections::BTreeMap<&'static str, &'static str>;

/// The library of first correct programs, with the updates for every change in `version`.
pub fn correct_programs(task: Task, version: Version) -> Programs {
    let mut programs: Programs = of(task).iter().map(|c| (c.name, c.correct[0])).collect();
    for update in updates(task).iter().filter(|u| version.has(u.change)) {
        programs.extend(update.programs.iter().copied());
    }
    programs
}

/// Module texts for `programs`, each function in its spec's module.
pub fn modules(task: Task, programs: &Programs) -> Vec<(&'static str, String)> {
    task.modules()
        .iter()
        .map(|&module| {
            let text: String = task
                .specs()
                .iter()
                .filter(|s| s.module == module)
                .filter_map(|s| programs.get(s.name).map(|p| format!("def {} {} : {p}\n", s.name, s.arity)))
                .collect();
            (module, text)
        })
        .collect()
}

/// [`modules`] of [`correct_programs`].
pub fn correct_library(task: Task, version: Version) -> Vec<(&'static str, String)> {
    modules(task, &correct_programs(task, version))
}

/// Whether every candidate names a spec, and every spec has candidates.
pub fn covers_every_spec(task: Task) -> bool {
    of(task).len() == task.specs().len() && of(task).iter().all(|c| task.spec_named(c.name).is_some())
}
