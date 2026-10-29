//! Phase 2's task (docs/design/phase2-task.md, Candidate A with Candidate C's
//! open contracts): its shape, its open contracts and their grading. The
//! tests that hold for every task, Phase 2's included, are in `stacklib.rs`.

use std::collections::BTreeSet;

use bracken_experiment::candidates::phase2::OTHER_CHOICE;
use bracken_experiment::candidates::{Programs, correct_programs, modules, of as candidates_of, revisions, updates};
use bracken_experiment::grader::{CONTRACT_EDGES, HIDDEN_DRAWS, HiddenCases, grade, grade_function};
use bracken_experiment::lang::{Library, parse_module};
use bracken_experiment::task::{CONTRACT_RANGE, Change, PUBLIC_RANGE, Task, Version, change_tag};

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

/// The correct library under `version`, with `changes` on top.
fn with_at(version: Version, changes: &[(&'static str, &'static str)]) -> Library {
    let mut programs = correct_programs(T, version);
    programs.extend(changes.iter().copied());
    library(&programs)
}

/// The functions `lib` gets wrong under `version`, alone or in the library.
fn failing_at(lib: &Library, version: Version, hidden: &HiddenCases) -> BTreeSet<&'static str> {
    grade(lib, version, hidden).into_iter().filter(|g| !(g.in_library && g.standalone)).map(|g| g.name).collect()
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
    assert_eq!(T.changes().len(), 12, "two waves of six");
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
        let probes: Vec<_> = T
            .property_cases(index, p)
            .iter()
            .map(|c| T.probe(p, c, &canonical, &canonical, Version::ORIGINAL))
            .collect();
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
            .map(|(p, c)| T.probe(p, &c, &canonical, &canonical, Version::ORIGINAL).want)
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
            public.extend(
                T.property_cases(index, p)
                    .iter()
                    .map(|c| T.probe(p, c, &canonical, &canonical, Version::ORIGINAL).expr),
            );
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

// The change waves.

#[test]
fn two_waves_of_six_at_thirty_and_sixty_percent_of_the_turns() {
    let waves = T.waves();
    assert_eq!(waves.len(), 2);
    assert_eq!(waves.iter().map(|w| w.share).collect::<Vec<_>>(), [0.3, 0.6]);
    assert_eq!(T.wave_turns(8), [2, 5], "after turns 2 and 5 of 8");
    assert_eq!(T.wave_turns(2), [1, 1]);
    assert_eq!(T.wave_turns(10), [3, 6]);
    // Every change request is in exactly one wave, the task lists them wave by wave, and each wave has six.
    let listed: Vec<Change> = waves.iter().flat_map(|w| w.changes.iter().copied()).collect();
    assert_eq!(listed, T.changes().iter().map(|c| c.change).collect::<Vec<_>>());
    assert!(waves.iter().all(|w| w.changes.len() == 6));
    assert_eq!(T.after_wave(0), Version::ORIGINAL);
    assert_eq!(T.after_wave(2), T.all());
    let tags: BTreeSet<&str> = T.changes().iter().map(|c| change_tag(c.change)).collect();
    assert_eq!(tags.len(), 12, "tags are unique");
    // Phase 1 keeps its single release.
    assert!(Task::Phase1.waves().is_empty() && Task::Phase1.wave_turns(8).is_empty());
}

/// clamp3 is the only function with 10 to 15 callers, so both signature changes are its: wave 2 changes
/// what wave 1 changed.
#[test]
fn clamp3_changes_its_signature_in_each_wave_and_its_callers_must_follow_both_times() {
    let hidden = HiddenCases::new(T, SEED);
    let callers: BTreeSet<&str> =
        T.specs().iter().filter(|s| T.defined_by(s.name).contains(&"clamp3")).map(|s| s.name).collect();
    for (wave, change) in [(1, Change::ClampOrder), (2, Change::ClampHiFirst)] {
        assert!(T.waves()[wave - 1].changes.contains(&change));
        let update = updates(T).iter().find(|u| u.change == change).expect("an update");
        let updated: BTreeSet<&str> = update.programs.iter().map(|(n, _)| *n).collect();
        let mut want = callers.clone();
        want.insert("clamp3");
        assert_eq!(updated, want, "{change:?}: clamp3 and every caller");
        // Callers left as they were are wrong on their own; with them updated but not clamp3, wrong through it.
        let before = T.after_wave(wave - 1);
        let after = T.after_wave(wave);
        let old = library(&correct_programs(T, before));
        let broken = failing_at(&old, after, &hidden);
        for name in &want {
            let g = grade_function(&old, name, after, &hidden);
            assert!(!g.standalone, "{change:?}: {name}'s own code is out of date");
        }
        assert!(broken.is_superset(&want));
        let mut programs = correct_programs(T, after);
        programs.insert("clamp3", correct_programs(T, before)["clamp3"]);
        let lib = library(&programs);
        for name in &callers {
            assert!(grade_function(&lib, name, after, &hidden).broken_by_callees(), "{change:?}: {name}");
        }
    }
    let o = Version::ORIGINAL;
    assert_eq!(T.reference("clamp3", &[5, 0, 3], o), 3);
    assert_eq!(T.reference("clamp3", &[5, 0, 3], T.after_wave(1)), 0, "(lo, hi, x) = (5, 0, 3)");
    assert_eq!(T.reference("clamp3", &[5, 0, 3], T.after_wave(2)), 3, "(hi, lo, x) = (5, 0, 3)");
    assert_eq!(T.reference("clamp3", &[3, 0, 5], o.with(Change::ClampHiFirst)), 3, "alone too");
    for version in [o, T.after_wave(1), T.after_wave(2)] {
        assert_eq!(T.reference("clampsq", &[3, 0, 5], version), 5, "a caller's contract doesn't change");
        assert_eq!(T.reference("tune", &[1, 9, 4], version), T.reference("tune", &[1, 9, 4], o));
    }
    let clamp3 = T.spec_named("clamp3").expect("a spec");
    assert!(T.description_at(clamp3, T.after_wave(1)).starts_with("clamp3(lo, hi, x)"));
    assert!(T.description_at(clamp3, T.after_wave(2)).starts_with("clamp3(hi, lo, x)"));
}

#[test]
fn each_change_has_a_spec_version_a_reference_and_candidates() {
    let hidden = HiddenCases::new(T, SEED);
    let o = Version::ORIGINAL;
    assert_eq!(revisions(T).len(), T.changes().len());
    for c in T.changes() {
        let v = o.with(c.change);
        let spec = T.spec_named(c.target).expect("targets a spec");
        assert_eq!(T.description_at(spec, o), spec.description);
        assert_eq!(T.description_at(spec, v), c.description);
        assert_ne!(c.description, spec.description);
        assert!(!T.task_text(o).contains(c.text) && T.task_text(v).contains(c.text));
        assert!(updates(T).iter().any(|u| u.change == c.change && u.programs.iter().any(|(n, _)| *n == c.target)));
        // The target's own candidates under the change, in the library that has it.
        let r = revisions(T).iter().find(|r| r.change == c.change).expect("a revision");
        assert_eq!(r.candidates.name, c.target);
        assert!(r.candidates.correct.len() == 2 && r.candidates.wrong.len() == 2);
        for program in r.candidates.correct {
            let g = grade_function(&with_at(v, &[(c.target, program)]), c.target, v, &hidden);
            assert!(g.in_library && g.standalone, "{:?}: {program}: {:?}", c.change, g.note);
        }
        for program in r.candidates.wrong {
            let g = grade_function(&with_at(v, &[(c.target, program)]), c.target, v, &hidden);
            assert!(!g.in_library && !g.standalone, "{:?}: {program} should be wrong", c.change);
        }
        let subtle = r.candidates.wrong.last().expect("wrong candidates");
        let check = T.check(&with_at(v, &[(c.target, subtle)]), c.target, v).expect("checkable");
        assert!(check.ok(), "{:?}: {subtle}: {}", c.change, check.details());
        // The original library is wrong under the change. For none-wide that is cut and pos, the producers
        // of the old none, not isnone, where the rule is stated.
        let old = failing_at(&with(&[]), v, &hidden);
        let first = if c.change == Change::NoneWide { "cut" } else { c.target };
        assert!(old.contains(first), "{:?}: {old:?}", c.change);
    }
    let v = |c: Change| o.with(c);
    assert_eq!(T.reference("sign", &[0], v(Change::SignOfZero)), 1);
    assert_eq!(T.reference("toward", &[3, 3], v(Change::SignOfZero)), 4, "defined in terms of sign, so it follows");
    assert_eq!(T.reference("between", &[3, 0, 3], v(Change::BetweenExclusive)), 0);
    assert_eq!(T.reference("band", &[3, 0, 3], v(Change::BetweenExclusive)), 0);
    assert_eq!(T.reference("weigh", &[1, 2, 3], o), 5);
    assert_eq!(T.reference("weigh", &[1, 2, 3], v(Change::WeighSubtracts)), -1);
    assert_eq!(T.reference("nearer", &[0, 1, -1], v(Change::NearerTies)), -1);
    assert_eq!(T.reference("abs", &[i64::MIN], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(T.reference("dist", &[i64::MIN, 0], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(T.reference("mix", &[2, 5], v(Change::MixSubtracts)), T.reference("tilt", &[2, 5], o) - 2);
}

/// Each pair's obvious fixes, one per change, conflict: a library must settle a new convention.
#[test]
fn the_interacting_pairs_need_a_shared_convention() {
    let hidden = HiddenCases::new(T, SEED);
    let o = Version::ORIGINAL;
    let none: BTreeSet<&str> = ["cut", "pos", "isnone", "either", "plus"].into();
    let rank: BTreeSet<&str> = ["rank", "flip", "isless", "issame", "ismore"].into();
    let both_none = o.with(Change::CutSigned).with(Change::NoneWide);
    assert!(T.waves()[0].changes.contains(&Change::CutSigned) && T.waves()[0].changes.contains(&Change::NoneWide));
    // cut turning signed, with none still -1: -1 is now a difference too.
    let signed = [("cut", "a b - -1000 < -1 a b - 1000 > -1 a b - ? ?")];
    assert!(!failing_at(&with(&signed), o.with(Change::CutSigned), &hidden).is_empty());
    // Its obvious fix moves none just past cut's range, which the other change rules out.
    let just_past = [
        ("cut", "a b - -1000 < -1001 a b - 1000 > -1001 a b - ? ?"),
        ("pos", "a 0 > a -1001 ?"),
        ("isnone", "a -1001 ="),
        ("either", "a -1001 = b a ?"),
        ("plus", "a -1001 = -1001 b -1001 = -1001 a b + ? ?"),
    ];
    assert!(failing_at(&with(&just_past), o.with(Change::CutSigned), &hidden).is_empty());
    assert_eq!(failing_at(&with(&just_past), both_none, &hidden), ["cut", "pos"].into());
    // The other's obvious fix moves none far away, but leaves cut unsigned.
    let far: Vec<_> =
        correct_programs(T, o.with(Change::NoneWide)).into_iter().filter(|(n, _)| none.contains(n)).collect();
    assert!(failing_at(&with(&far), o.with(Change::NoneWide), &hidden).is_empty());
    assert!(failing_at(&with(&far), both_none, &hidden).is_subset(&none));
    assert!(!failing_at(&with(&far), both_none, &hidden).is_empty());
    assert!(failing_at(&with_at(both_none, &[]), both_none, &hidden).is_empty(), "the settled convention");

    let both_rank = o.with(Change::RankWide).with(Change::IsMoreFar);
    assert!(T.waves()[1].changes.contains(&Change::RankWide) && T.waves()[1].changes.contains(&Change::IsMoreFar));
    // rank's obvious fix for the wide range says only which is larger; ismore's for its margin reads a - b.
    let by_sign = [("rank", "a b < -1 a b > 1 0 ? ?")];
    let margin = [("ismore", "a 2 >")];
    assert!(failing_at(&with(&by_sign), o.with(Change::RankWide), &hidden).is_empty());
    assert!(failing_at(&with(&margin), o.with(Change::IsMoreFar), &hidden).is_empty());
    assert!(!failing_at(&with(&margin), both_rank, &hidden).is_empty(), "a - b overflows on wide inputs");
    let together = with(&[by_sign[0], margin[0]]);
    let broken = failing_at(&together, both_rank, &hidden);
    assert!(broken.contains("ismore") && broken.is_subset(&rank), "{broken:?}");
    assert!(failing_at(&with_at(both_rank, &[]), both_rank, &hidden).is_empty(), "the settled convention");
}

/// Grading uses the specs in force: each wave's correct library is right under its own version and not
/// under the next, and the task text and `TASKS.md` state each wave's version.
#[test]
fn each_wave_is_graded_and_stated_under_its_own_version() {
    let hidden = HiddenCases::new(T, SEED);
    for wave in 0..=2 {
        let version = T.after_wave(wave);
        let lib = with_at(version, &[]);
        assert!(failing_at(&lib, version, &hidden).is_empty(), "wave {wave}");
        if wave < 2 {
            let next = T.after_wave(wave + 1);
            let broken = failing_at(&lib, next, &hidden);
            for c in T.waves()[wave].changes {
                let target = T.changes().iter().find(|r| r.change == *c).expect("a request").target;
                assert!(broken.contains(target), "wave {}: {target} is out of date", wave + 1);
            }
        }
        let tasks = T.tasks_md(version);
        assert_eq!(T.version_of(&tasks), version);
        let released: Vec<&str> = T.changes_in(version).map(|c| change_tag(c.change)).collect();
        assert_eq!(released.len(), wave * 6);
        assert!(tasks.ends_with(&format!("Change requests released: {}\n", released.join(", "))));
    }
}

/// Under `rank-wide`, a rank property's inputs span the range the spec states, and some pairs are close
/// enough to test `ismore`'s margin.
#[test]
fn rank_properties_reach_the_wide_range_once_it_applies() {
    let hidden = HiddenCases::new(T, SEED);
    let canonical = with_at(T.all(), &[]);
    let numbers = |expr: &str| -> Vec<i128> {
        expr.split(|ch: char| !(ch.is_ascii_digit() || ch == '-')).filter_map(|w| w.parse().ok()).collect()
    };
    let rank = T.contracts().iter().find(|c| c.name == "rank").expect("the rank contract");
    let mut widest = 0;
    let mut close = 0;
    for (index, p) in T.properties().filter(|(_, p)| p.calls.contains(&"rank")) {
        assert!(rank.properties.iter().any(|q| std::ptr::eq(q, p)));
        for case in hidden.of_property(index) {
            let probe = T.probe(p, case, &canonical, &canonical, T.all());
            let n = numbers(&probe.expr);
            assert_eq!(n.len(), 2, "{}", probe.expr);
            assert!(n.iter().all(|v| v.abs() <= 9_000_000_000_000_000_000), "{}", probe.expr);
            widest = widest.max(n[0].abs());
            close += usize::from((n[0] - n[1]).abs() <= 3 && n[0] != n[1]);
            assert_eq!(probe.failure(), None);
        }
    }
    assert_eq!(widest, 9_000_000_000_000_000_000);
    assert!(close > 100, "{close} close pairs");
}
