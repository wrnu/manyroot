//! Phase 2's task (docs/design/phase2-task.md, Candidate A with Candidate C's
//! open contracts): its shape, its open contracts and their grading. The
//! tests that hold for every task, Phase 2's included, are in `stacklib.rs`.

use std::collections::BTreeSet;

use bracken_experiment::candidates::phase2::OTHER_CHOICE;
use bracken_experiment::candidates::{Programs, correct_programs, modules, of as candidates_of};
use bracken_experiment::grader::{CONTRACT_EDGES, HIDDEN_DRAWS, HiddenCases, grade, grade_function};
use bracken_experiment::lang::{Library, parse_module};
use bracken_experiment::task::{CONTRACT_RANGE, PUBLIC_RANGE, Task, Version};

const T: Task = Task::Phase2;
const SEED: u64 = 7;

fn library(programs: &Programs) -> Library {
    let mods = modules(T, programs);
    Library::parse(mods.iter().map(|(m, t)| (*m, t.as_str())))
}

fn with(changes: &[(&'static str, &'static str)]) -> Library {
    let mut programs = correct_programs(T, Version::ORIGINAL);
    programs.extend(changes.iter().copied());
    library(&programs)
}

fn members() -> Vec<&'static str> {
    T.contracts().iter().flat_map(|c| c.members.iter().copied()).collect()
}

// The shape.

#[test]
fn ninety_specs_in_eight_modules_and_five_layers() {
    let specs = T.specs();
    assert_eq!(specs.len(), 90);
    assert_eq!(T.modules().len(), 8);
    assert_eq!(T.module_files().len(), 8);
    for (module, file) in T.modules().iter().zip(T.module_files()) {
        assert_eq!(*file, format!("{module}.stk"));
        let layers: BTreeSet<u8> = specs.iter().filter(|s| s.module == *module).map(|s| s.layer).collect();
        assert_eq!(layers.len(), 1, "{module}: one layer per module");
        assert!(specs.iter().filter(|s| s.module == *module).count() >= 8, "{module}");
    }
    let layers: Vec<u8> =
        T.modules().iter().map(|m| specs.iter().find(|s| s.module == *m).expect("a spec").layer).collect();
    assert!(layers.windows(2).all(|w| w[0] <= w[1]), "modules are listed from the lowest layer: {layers:?}");
    assert_eq!(layers.iter().copied().collect::<BTreeSet<_>>(), (0..5).collect(), "five layers");
    let mut seen = BTreeSet::new();
    assert!(specs.iter().all(|s| seen.insert(s.name)), "names are unique");
    assert!(T.changes().is_empty(), "the change waves are build step 2");
    for s in specs {
        assert!(T.modules().contains(&s.module), "{}", s.name);
        let c = bracken_experiment::candidates::candidates(T, s.name);
        assert!(c.correct.len() >= 2 && c.wrong.len() >= 2, "{}", s.name);
        for program in c.correct.iter().chain(c.wrong) {
            let def = parse_module(s.module, &format!("def {} {} : {program}", s.name, s.arity));
            assert!(def.is_ok(), "{}: {program}: {def:?}", s.name);
        }
        if T.contract_of(s.name).is_none() {
            let _ = T.reference(s.name, &[1, 2, 3][..usize::from(s.arity)], T.all());
        }
    }
    assert_eq!(candidates_of(T).len(), specs.len());
}

#[test]
fn every_function_above_layer_one_calls_two_or_three_from_two_lower_modules() {
    for s in T.specs().iter().filter(|s| T.contract_of(s.name).is_none()) {
        let callees: Vec<_> = T.defined_by(s.name).iter().map(|d| T.spec_named(d).expect("a spec")).collect();
        let distinct: BTreeSet<&str> = callees.iter().map(|c| c.name).collect();
        assert_eq!(distinct.len(), callees.len(), "{}: no callee twice", s.name);
        match s.layer {
            0 => assert!(callees.is_empty(), "{}: layer 0 needs only the language", s.name),
            1 => assert!(callees.iter().all(|c| c.layer <= 1), "{}", s.name),
            _ => {
                assert!((2..=3).contains(&callees.len()), "{}: {} callees", s.name, callees.len());
                assert!(callees.iter().all(|c| c.layer < s.layer), "{}: only lower layers", s.name);
                let modules: BTreeSet<&str> = callees.iter().map(|c| c.module).collect();
                assert!(modules.len() >= 2, "{}: callees from {modules:?}", s.name);
            }
        }
    }
}

/// Step 2 changes the signature of a function many others call, so many callers must follow.
#[test]
fn clamp3_has_ten_to_fifteen_callers() {
    let callers = T.specs().iter().filter(|s| T.defined_by(s.name).contains(&"clamp3")).count();
    assert!((10..=15).contains(&callers), "{callers} callers");
}

/// Bracken mode's helper rule needs it: a helper is named starting with its function's name, so no
/// spec name may start another (`bracken_mode::artifact_problem`).
#[test]
fn no_spec_name_starts_another() {
    for task in Task::ALL {
        for a in task.specs() {
            for b in task.specs().iter().filter(|b| b.name != a.name) {
                assert!(!b.name.starts_with(a.name), "{task:?}: {} starts {}", a.name, b.name);
            }
        }
    }
}

// The open contracts.

#[test]
fn open_contracts_span_modules_and_their_members_are_graded_by_properties() {
    assert!((2..=3).contains(&T.contracts().len()));
    for c in T.contracts() {
        let modules: BTreeSet<&str> =
            c.members.iter().map(|m| T.spec_named(m).expect("a member is a spec").module).collect();
        assert!(modules.len() >= 2, "{}: producers and consumers in different modules", c.name);
        for m in c.members {
            assert_eq!(T.contract_of(m).map(|k| k.name), Some(c.name));
            assert!(T.defined_by(m).is_empty(), "{m}: a member has no reference to follow");
            assert!(T.spec_named(m).expect("a spec").description.to_lowercase().contains("open contract"), "{m}");
            assert!(!T.properties_of(m).is_empty(), "{m} is graded by some property");
        }
        for p in c.properties {
            assert!(p.calls.iter().all(|f| c.members.contains(f)), "{}: a property calls members only", c.name);
            assert!(p.calls.len() >= 2, "{}: a property relates members", c.name);
        }
    }
    // No other function's contract is defined in terms of a member.
    for s in T.specs() {
        assert!(T.defined_by(s.name).iter().all(|d| !members().contains(d)), "{}", s.name);
    }
}

#[test]
#[should_panic(expected = "no reference for rank")]
fn a_member_has_no_reference() {
    T.reference("rank", &[1, 2], Version::ORIGINAL);
}

#[test]
fn any_consistent_choice_passes_and_a_mixed_one_fails() {
    let hidden = HiddenCases::new(T, SEED);
    let failing = |lib: &Library| -> Vec<&'static str> {
        grade(lib, Version::ORIGINAL, &hidden)
            .into_iter()
            .filter(|g| !(g.in_library && g.standalone))
            .map(|g| g.name)
            .collect()
    };
    assert_eq!(failing(&with(&[])), Vec::<&str>::new(), "the first correct programs' convention");
    assert_eq!(failing(&with(OTHER_CHOICE)), Vec::<&str>::new(), "another convention");
    assert_eq!(OTHER_CHOICE.len(), members().len());
    // Producers of one convention, consumers of the other: each looks right alone, and the library is wrong.
    let producers = ["rank", "cut", "pos"];
    let consumers: Vec<_> = OTHER_CHOICE.iter().copied().filter(|(n, _)| !producers.contains(n)).collect();
    let mixed = with(&consumers);
    let broken: BTreeSet<&str> = failing(&mixed).into_iter().collect();
    assert_eq!(broken, members().into_iter().collect(), "every member of both contracts");
    // Two producers that each recognise their own none, but with different nones, break the rule.
    let lib = with(&[
        ("pos", "a 0 > a 99999 ?"),
        ("isnone", "a -1 = a 99999 = +"),
        ("either", "a isnone@ b a ?"),
        ("plus", "a isnone@ b isnone@ + 0 > -1 a b + ?"),
    ]);
    let grades = grade(&lib, Version::ORIGINAL, &hidden);
    let wrong: BTreeSet<&str> = grades.iter().filter(|g| !g.in_library).map(|g| g.name).collect();
    assert_eq!(wrong, BTreeSet::from(["cut", "pos"]), "only the producers break the one-none rule");
    let pos = grades.iter().find(|g| g.name == "pos").expect("graded");
    assert!(pos.note.as_deref().is_some_and(|n| n.contains("one none")), "{pos:?}");
    let check = T.check(&mixed, "isless", Version::ORIGINAL).expect("checkable");
    assert!(!check.ok(), "the public check shows the disagreement");
    assert!(check.details().starts_with("isless(rank("), "{}", check.details());
    // Only the members break: no function outside the contracts calls them.
    assert!(failing(&mixed).iter().all(|f| members().contains(f)));
}

#[test]
fn a_member_with_a_partner_missing_fails_and_others_stand() {
    let hidden = HiddenCases::new(T, SEED);
    let mut programs = correct_programs(T, Version::ORIGINAL);
    programs.remove("isless");
    let lib = library(&programs);
    let rank = grade_function(&lib, "rank", Version::ORIGINAL, &hidden);
    assert!(rank.present && !rank.in_library && !rank.standalone, "{rank:?}");
    assert!(rank.note.as_deref().is_some_and(|n| n.contains("`isless` is not defined")), "{rank:?}");
    let isless = grade_function(&lib, "isless", Version::ORIGINAL, &hidden);
    assert!(!isless.present);
    let none = T.contracts().iter().find(|c| c.name == "none").expect("the none contract");
    for name in none.members {
        let g = grade_function(&lib, name, Version::ORIGINAL, &hidden);
        assert!(g.in_library && g.standalone, "{name}: the other contract stands");
    }
}

#[test]
fn property_cases_are_public_ones_plus_draws_and_edges_within_the_stated_range() {
    let hidden = HiddenCases::new(T, SEED);
    let canonical = with(&[]);
    for (index, p) in T.properties() {
        let arity = usize::from(p.arity);
        let public = T.property_cases(index, p);
        let all = hidden.of_property(index);
        assert_eq!(public.len(), if p.arity == 1 { 13 } else { 60 });
        assert_eq!(&all[..public.len()], public.as_slice());
        assert_eq!(all.len(), public.len() + HIDDEN_DRAWS + CONTRACT_EDGES.len().pow(u32::from(p.arity)));
        assert!(public.iter().all(|c| c[..arity].iter().all(|v| v.abs() <= PUBLIC_RANGE)));
        assert!(all.iter().all(|c| c[..arity].iter().all(|v| v.abs() <= CONTRACT_RANGE)));
        assert!(all.iter().any(|c| c[0] == CONTRACT_RANGE) && all.iter().any(|c| c[0] == -CONTRACT_RANGE));
        assert!(all.iter().any(|c| c[0].abs() > PUBLIC_RANGE), "draws go beyond the public range");
    }
    // Each property's public cases show more than one answer, so a constant can't pass them, but for the
    // one-none rule, which only relates the producers' nones; each member's properties together still do.
    let mut fixed = Vec::new();
    for (index, p) in T.properties() {
        let probes: Vec<_> = T.property_cases(index, p).iter().map(|c| T.probe(p, c, &canonical, &canonical)).collect();
        let wants: BTreeSet<i64> = probes.iter().map(|probe| probe.want).collect();
        if wants.len() < 2 {
            fixed.push(probes[0].expr.clone());
        }
    }
    assert_eq!(fixed.len(), 1, "{fixed:?}");
    assert!(fixed[0].contains("one none"), "{fixed:?}");
    for name in members() {
        let wants: BTreeSet<i64> = T
            .properties_of(name)
            .into_iter()
            .flat_map(|(i, p)| T.property_cases(i, p).into_iter().map(move |c| (p, c)))
            .map(|(p, c)| T.probe(p, &c, &canonical, &canonical).want)
            .collect();
        assert!(wants.len() >= 2, "{name}: public answers {wants:?}");
    }
    let other = HiddenCases::new(T, SEED + 1);
    assert_ne!(hidden.of_property(0), other.of_property(0), "the seed decides the draws");
}

#[test]
fn a_members_check_reports_only_public_inputs() {
    let canonical = with(&[]);
    for name in members() {
        let mut public = Vec::new();
        for (index, p) in T.properties_of(name) {
            public.extend(T.property_cases(index, p).iter().map(|c| T.probe(p, c, &canonical, &canonical).expr));
        }
        let c = bracken_experiment::candidates::candidates(T, name);
        for program in c.correct.iter().chain(c.wrong) {
            let result = T.check(&with(&[(name, program)]), name, Version::ORIGINAL).expect("checkable");
            assert_eq!(result.total, public.len(), "{name}");
            if let Some(failure) = &result.first_failure {
                assert!(public.iter().any(|e| failure.starts_with(&format!("{e} "))), "{failure}");
            }
        }
    }
}

#[test]
fn the_task_text_states_every_spec_and_no_hidden_input() {
    let text = T.task_text(Version::ORIGINAL);
    assert!(text.contains(T.language()) && T.language().contains("eight modules"));
    for s in T.specs() {
        assert!(text.contains(&format!("- {}\n", s.description)), "{}", s.name);
    }
    for m in T.modules() {
        assert!(text.contains(&format!("\nModule {m}:\n")));
    }
    // The only numbers beyond the public range are the stated limits and the open contracts' ranges.
    let allowed = ["10000", "64", "1000", "4000"];
    for word in text.split(|ch: char| !ch.is_ascii_digit()).filter(|w| !w.is_empty()) {
        let n: i128 = word.parse().expect("digits");
        assert!(n <= i128::from(PUBLIC_RANGE) || allowed.contains(&word), "{word} in the task text");
    }
    assert_eq!(T.tasks_md(Version::ORIGINAL), format!("{text}\n\nChange requests released: \n"));
    assert_eq!(T.version_of(&T.tasks_md(Version::ORIGINAL)), Version::ORIGINAL);
}

/// The interpreter's limits hold for the deepest layer: a correct layer-4 function stays well within
/// them on its hidden cases, so a correct program is never cut off.
#[test]
fn the_top_layer_runs_within_the_step_and_depth_limits() {
    use bracken_experiment::lang::run;
    let lib = with(&[]);
    let hidden = HiddenCases::new(T, SEED);
    for s in T.specs().iter().filter(|s| s.layer == 4) {
        let def = lib.get(s.name).expect("defined");
        for case in hidden.of(s) {
            let args = &case[..usize::from(s.arity)];
            assert_eq!(run(def, args, &lib), Ok(T.reference(s.name, args, Version::ORIGINAL)), "{}{args:?}", s.name);
        }
    }
}

/// Reading the task whole fits in one tool result, as every one must (`driver::MAX_RESULT_CHARS`),
/// even as `read` numbers its lines, and so does the correct library's largest module.
#[test]
fn the_task_and_a_module_fit_in_one_tool_result() {
    use bracken_experiment::driver::MAX_RESULT_CHARS;
    let tasks = T.tasks_md(T.all());
    let numbered = tasks.chars().count() + 6 * tasks.lines().count();
    assert!(numbered * 2 < MAX_RESULT_CHARS, "{numbered} characters numbered");
    let largest = modules(T, &correct_programs(T, Version::ORIGINAL)).into_iter().map(|(_, t)| t.len()).max();
    assert!(largest.is_some_and(|n| n * 4 < bracken_experiment::lang::MAX_MODULE_BYTES), "{largest:?}");
}
