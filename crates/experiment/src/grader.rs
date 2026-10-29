//! The hidden grader: scores any version of a library exactly, on cases no
//! agent sees.
//!
//! Nothing here may reach an agent: not the cases, not the seed, not a score.
//! An agent's tools use [`crate::task::Task::check`], which runs public cases only.

use std::collections::{BTreeMap, BTreeSet};

use bracken_demo::rng::Rng;

use crate::lang::{Calls, Library, Target, run};
use crate::task::{CONTRACT_RANGE, Case, PUBLIC_RANGE, Spec, Task, Version, call_text};

/// Random draws per function, from -HIDDEN_RANGE..=HIDDEN_RANGE.
pub const HIDDEN_DRAWS: usize = 2000;
pub const HIDDEN_RANGE: i64 = 1000;
/// Edge values; a function gets every combination of them over its inputs.
pub const EDGES: [i64; 5] = [0, 1, -1, i64::MIN, i64::MAX];
/// An open contract's edge values: its specs state its inputs' range ([`CONTRACT_RANGE`]).
pub const CONTRACT_EDGES: [i64; 5] = [0, 1, -1, -CONTRACT_RANGE, CONTRACT_RANGE];

/// Every case the grader runs, per spec and per open contract's property: the
/// public cases, [`HIDDEN_DRAWS`] seeded draws and every combination of the
/// edge values.
pub struct HiddenCases {
    task: Task,
    cases: BTreeMap<&'static str, Vec<Case>>,
    properties: Vec<Vec<Case>>,
}

/// `public`, then [`HIDDEN_DRAWS`] draws of `arity` inputs from -range..=range on `stream`, then every
/// combination of `edges`.
fn cases(mut all: Vec<Case>, arity: u8, seed: u64, stream: u64, range: i64, edges: &[i64]) -> Vec<Case> {
    let span = (2 * range + 1) as usize;
    let mut rng = Rng::derive(seed, stream);
    for _ in 0..HIDDEN_DRAWS {
        let mut case = [0; 3];
        for v in case.iter_mut().take(usize::from(arity)) {
            *v = rng.below(span) as i64 - range;
        }
        all.push(case);
    }
    for n in 0..edges.len().pow(u32::from(arity)) {
        let mut case = [0; 3];
        let mut rest = n;
        for v in case.iter_mut().take(usize::from(arity)) {
            *v = edges[rest % edges.len()];
            rest /= edges.len();
        }
        all.push(case);
    }
    all
}

impl HiddenCases {
    /// The run's plan supplies `task` and `seed`; the seed is never shown to an agent.
    pub fn new(task: Task, seed: u64) -> HiddenCases {
        let specs = task.specs();
        let mut by_spec = BTreeMap::new();
        for (index, spec) in specs.iter().enumerate() {
            let all = cases(task.public_cases(spec), spec.arity, seed, index as u64, HIDDEN_RANGE, &EDGES);
            by_spec.insert(spec.name, all);
        }
        let properties = task
            .properties()
            .map(|(index, p)| {
                let stream = (specs.len() + index) as u64;
                cases(task.property_cases(index, p), p.arity, seed, stream, CONTRACT_RANGE, &CONTRACT_EDGES)
            })
            .collect();
        HiddenCases { task, cases: by_spec, properties }
    }

    /// The task the cases are for.
    pub fn task(&self) -> Task {
        self.task
    }

    pub fn of(&self, spec: &Spec) -> &[Case] {
        self.cases.get(spec.name).map_or(&[], Vec::as_slice)
    }

    /// The cases of the property with index `index` among the task's.
    pub fn of_property(&self, index: usize) -> &[Case] {
        self.properties.get(index).map_or(&[], Vec::as_slice)
    }
}

/// A reference implementation, as the interpreter calls it.
type Native = Box<dyn Fn(&[i64]) -> i64>;

/// Grading one function F: a call to spec S resolves to S's reference
/// implementation under one version only if S isn't F, S is in F's layer or a
/// lower one, S's contract isn't defined in terms of F ([`Task::defined_by`],
/// transitively), and S is no open contract's member (which has no reference).
/// Every other name resolves in the library: F itself, an agent's helper, a
/// spec of a higher layer, a spec defined in terms of F, and an open
/// contract's members. So a program that hands its work back to F (directly,
/// through a helper, through a higher layer, or through a spec defined by F)
/// recurses into its own code, never into an oracle.
struct Standalone<'a> {
    library: &'a Library,
    natives: BTreeMap<&'static str, (u8, Native)>,
}

impl<'a> Standalone<'a> {
    fn new(library: &'a Library, task: Task, version: Version, under_test: &Spec) -> Standalone<'a> {
        let natives = task
            .specs()
            .iter()
            .filter(|s| {
                s.name != under_test.name
                    && s.layer <= under_test.layer
                    && !contract_reaches(task, s.name, under_test.name)
                    && task.contract_of(s.name).is_none()
            })
            .map(|s| {
                let name = s.name;
                let eval: Native = Box::new(move |args| task.reference(name, args, version));
                (name, (s.arity, eval))
            })
            .collect();
        Standalone { library, natives }
    }
}

/// Whether `name`'s contract is defined in terms of `target`, at any depth.
fn contract_reaches(task: Task, name: &str, target: &str) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut todo: Vec<&str> = task.defined_by(name).to_vec();
    while let Some(next) = todo.pop() {
        if next == target {
            return true;
        }
        if seen.insert(next) {
            todo.extend(task.defined_by(next));
        }
    }
    false
}

impl Calls for Standalone<'_> {
    fn resolve(&self, name: &str) -> Option<Target<'_>> {
        match self.natives.get(name) {
            Some((arity, eval)) => Some(Target::Native { arity: *arity, eval: eval.as_ref() }),
            None => self.library.resolve(name),
        }
    }
}

/// One function's score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionGrade {
    pub name: &'static str,
    /// The library has exactly one definition of it, in a module that parses.
    pub present: bool,
    /// Right on every hidden case, with calls resolved in the library, as a user of the library sees it.
    pub in_library: bool,
    /// Right on every hidden case, with calls to specs resolved to the reference
    /// implementations, so only the function's own code is judged.
    pub standalone: bool,
    /// The first problem found, for the report. Never shown to an agent.
    pub note: Option<String>,
}

impl FunctionGrade {
    /// Right on its own but wrong in the library: what it calls is broken.
    pub fn broken_by_callees(&self) -> bool {
        self.standalone && !self.in_library
    }
}

/// Scores `name` in `library` under `version`. An open contract's member is right when every property
/// that calls it holds on all its hidden cases, with the other members taken from the library.
pub fn grade_function(library: &Library, name: &str, version: Version, hidden: &HiddenCases) -> FunctionGrade {
    let task = hidden.task();
    let Some(spec) = task.spec_named(name) else {
        panic!("{name} is not a spec");
    };
    let fail = |present, note: String| FunctionGrade {
        name: spec.name,
        present,
        in_library: false,
        standalone: false,
        note: Some(note),
    };
    let Some(def) = library.get(name) else {
        return fail(false, library.why_missing(name));
    };
    if def.arity != spec.arity {
        return fail(true, format!("takes {} inputs, not {}", def.arity, spec.arity));
    }
    let standalone_calls = Standalone::new(library, task, version, spec);
    let mut note = None;
    let mut score = |calls: &dyn Calls, how: &str| {
        if task.contract_of(name).is_some() {
            for (index, property) in task.properties_of(name) {
                for case in hidden.of_property(index) {
                    if let Some(failure) = task.probe(property, case, library, calls).failure() {
                        note.get_or_insert(format!("{how}: {failure}"));
                        return false;
                    }
                }
            }
            return true;
        }
        for case in hidden.of(spec) {
            let args = &case[..usize::from(spec.arity)];
            let want = task.reference(name, args, version);
            match run(def, args, calls) {
                Ok(got) if got == want => {}
                outcome => {
                    let got = outcome.map_or_else(|e| format!("failed: {e}"), |g| format!("gave {g}"));
                    note.get_or_insert(format!("{how}: {} {got}, expected {want}", call_text(spec, case)));
                    return false;
                }
            }
        }
        true
    };
    let in_library = score(library, "in library");
    let standalone = score(&standalone_calls, "standalone");
    FunctionGrade { name: spec.name, present: true, in_library, standalone, note }
}

/// Scores every spec in `library` under `version`, in the task's order.
pub fn grade(library: &Library, version: Version, hidden: &HiddenCases) -> Vec<FunctionGrade> {
    hidden.task().specs().iter().map(|s| grade_function(library, s.name, version, hidden)).collect()
}

/// Whether a hidden case holds an input no public case can: outside the public range.
pub fn beyond_public(case: &Case) -> bool {
    case.iter().any(|v| v.abs_diff(0) > PUBLIC_RANGE.unsigned_abs())
}
