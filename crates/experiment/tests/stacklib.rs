//! stacklib: the language, the specs and change requests, the public check and
//! the hidden grader (docs/design/phase1-experiment.md, section 1).

use bracken_experiment::candidates::{
    CANDIDATES, Programs, UPDATES, candidates, correct_library, correct_programs, covers_every_spec, modules,
};
use bracken_experiment::grader::{
    EDGES, FunctionGrade, HIDDEN_DRAWS, HIDDEN_RANGE, HiddenCases, grade, grade_function,
};
use bracken_experiment::lang::{Library, MAX_DEPTH, parse_module, run};
use bracken_experiment::task::{
    CHANGES, Change, LANGUAGE, MODULES, PUBLIC_RANGE, SPECS, Version, check, public_cases, reference, spec_named,
    task_text,
};

const SEED: u64 = 7;

fn library(modules: &[(&'static str, String)]) -> Library {
    Library::parse(modules.iter().map(|(m, t)| (*m, t.as_str())))
}

fn with(programs: &Programs, changes: &[(&'static str, &'static str)]) -> Library {
    let mut programs = programs.clone();
    programs.extend(changes.iter().copied());
    library(&modules(&programs))
}

fn failing(grades: &[FunctionGrade]) -> Vec<&FunctionGrade> {
    grades.iter().filter(|g| !(g.in_library && g.standalone)).collect()
}

// The language.

#[test]
fn calls_pop_their_arity_with_the_deepest_as_a() {
    let lib = Library::parse([
        ("base", "def sub2 2 : a b -\n# a comment\n\ndef rev 2 : b a sub2@  # trailing comment"),
        ("order", "def pick 3 : a b c ?"),
    ]);
    assert!(lib.broken.is_empty(), "{:?}", lib.broken);
    let rev = lib.get("rev").expect("rev is defined");
    assert_eq!(run(rev, &[10, 3], &lib), Ok(-7));
    let pick = lib.get("pick").expect("pick is defined");
    assert_eq!(run(pick, &[0, 1, 2], &lib), Ok(2));
    assert_eq!(rev.module, "base");
}

#[test]
fn arithmetic_wraps_and_errors_are_failures_not_panics() {
    let lib = Library::parse([(
        "base",
        "def w 1 : a 1 +\ndef under 1 : +\ndef two 1 : a a\ndef call 1 : a nope@\ndef few 2 : a few2@\ndef few2 2 : a b +",
    )]);
    let get = |n| lib.get(n).expect("defined");
    assert_eq!(run(get("w"), &[i64::MAX], &lib), Ok(i64::MIN));
    assert!(run(get("under"), &[1], &lib).is_err_and(|e| e.contains("underflow")));
    assert!(run(get("two"), &[1], &lib).is_err_and(|e| e.contains("left 2 values")));
    assert!(run(get("call"), &[1], &lib).is_err_and(|e| e.contains("isn't defined")));
    assert!(run(get("few"), &[1, 2], &lib).is_err_and(|e| e.contains("underflow")));
    assert!(run(get("w"), &[1, 2], &lib).is_err(), "wrong number of inputs");
}

#[test]
fn step_and_depth_limits_stop_any_program() {
    // 40 calls of a 301-token function: about 12,000 steps, in a module well under the size limit.
    let long = format!("def g 1 : 1{}\ndef long 1 : 0{}", " 1 +".repeat(150), " 1 g@ +".repeat(40));
    let lib = Library::parse([("base", "def loop 1 : a loop@\ndef deep 1 : a 1 - deep@"), ("order", long.as_str())]);
    let get = |n| lib.get(n).expect("defined");
    assert!(run(get("loop"), &[1], &lib).is_err_and(|e| e.contains(&MAX_DEPTH.to_string())));
    assert!(run(get("deep"), &[1], &lib).is_err());
    assert!(run(get("long"), &[1], &lib).is_err_and(|e| e.contains("10000 steps")));
}

#[test]
fn a_bad_line_breaks_its_whole_module_and_duplicates_count_as_undefined() {
    for (text, why) in [
        ("def f 1 : a x", "unknown token"),
        ("def f 2 : a b c", "input `c`"),
        ("def F 1 : a", "can't name"),
        ("def neg 1 : a", "can't name"),
        ("def f 0 : 1", "arity"),
        ("def f 4 : a", "arity"),
        ("def f 1 :", "empty program"),
        ("f 1 : a", "expected"),
        ("def f 1 a", "expected"),
        ("def f 1 : a G@", "valid function name"),
    ] {
        let e = parse_module("base", &format!("def ok 1 : a\n{text}")).expect_err(text);
        assert_eq!(e.line, 2, "{text}");
        assert!(e.message.contains(why), "{text}: {}", e.message);
    }
    let lib = Library::parse([
        ("base", "def ok 1 : a\ndef f 1 : a x"),
        ("order", "def twice 1 : a\ndef g 1 : a"),
        ("arith", "def twice 1 : a a +\ndef twice 1 : 0"),
    ]);
    assert!(lib.get("ok").is_none(), "base doesn't parse, so nothing in it counts");
    assert_eq!(lib.broken.len(), 1);
    assert!(lib.why_missing("ok").contains("base (line 2"));
    assert!(lib.get("twice").is_none());
    assert!(lib.why_missing("twice").contains("more than once"));
    assert!(lib.get("g").is_some());
}

// The specs, candidates and change requests.

#[test]
fn about_thirty_specs_in_four_layered_modules_each_with_candidates() {
    assert_eq!(SPECS.len(), 30);
    assert!(covers_every_spec());
    for module in MODULES {
        assert!(SPECS.iter().filter(|s| s.module == module).count() >= 6, "{module}");
    }
    for s in SPECS {
        assert_eq!(usize::from(s.layer), MODULES.iter().position(|m| *m == s.module).expect("a module"), "{}", s.name);
        let c = candidates(s.name);
        assert!(c.correct.len() >= 2 && c.wrong.len() >= 2, "{}", s.name);
        for program in c.correct.iter().chain(c.wrong) {
            let def = parse_module(s.module, &format!("def {} {} : {program}", s.name, s.arity));
            assert!(def.is_ok(), "{}: {program}: {def:?}", s.name);
        }
        let _ = reference(s.name, &[1, 2, 3][..usize::from(s.arity)], Version::all());
    }
    let mut seen = std::collections::BTreeSet::new();
    assert!(SPECS.iter().all(|s| seen.insert(s.name)), "names are unique");
}

#[test]
fn the_correct_library_scores_fully_under_every_change() {
    let hidden = HiddenCases::new(SEED);
    let mut versions = vec![Version::ORIGINAL, Version::all()];
    versions.extend(CHANGES.iter().map(|c| Version::ORIGINAL.with(c.change)));
    for version in versions {
        let lib = library(&correct_library(version));
        assert!(lib.broken.is_empty() && lib.duplicates.is_empty());
        let grades = grade(&lib, version, &hidden);
        assert_eq!(grades.len(), SPECS.len());
        assert!(failing(&grades).is_empty(), "{version:?}: {:?}", failing(&grades));
        assert!(grades.iter().all(|g| g.present && g.note.is_none()));
    }
}

#[test]
fn every_correct_candidate_passes_and_every_wrong_one_fails_on_its_own() {
    let hidden = HiddenCases::new(SEED);
    let base = correct_programs(Version::ORIGINAL);
    for c in CANDIDATES {
        for program in c.correct {
            let g = grade_function(&with(&base, &[(c.name, program)]), c.name, Version::ORIGINAL, &hidden);
            assert!(g.in_library && g.standalone, "{}: {program}: {:?}", c.name, g.note);
        }
        for program in c.wrong {
            let g = grade_function(&with(&base, &[(c.name, program)]), c.name, Version::ORIGINAL, &hidden);
            assert!(!g.standalone && !g.in_library, "{}: {program} should be wrong", c.name);
        }
    }
}

#[test]
fn each_spec_has_a_wrong_candidate_the_public_check_passes() {
    let base = correct_programs(Version::ORIGINAL);
    for c in CANDIDATES {
        let subtle = c.wrong.last().expect("wrong candidates");
        let check = check(&with(&base, &[(c.name, subtle)]), c.name, Version::ORIGINAL).expect("checkable");
        assert!(check.ok(), "{}: {subtle}: {}", c.name, check.details());
    }
}

#[test]
fn a_broken_callee_shows_as_callers_right_alone_but_wrong_in_the_library() {
    let hidden = HiddenCases::new(SEED);
    let lib = with(&correct_programs(Version::ORIGINAL), &[("max2", "a b < a b ?")]);
    let grades = grade(&lib, Version::ORIGINAL, &hidden);
    let by_name = |n: &str| grades.iter().find(|g| g.name == n).expect("graded");
    assert!(!by_name("max2").standalone);
    for caller in ["clamp3", "median3", "max3", "relu", "spread3", "clampsq"] {
        assert!(by_name(caller).broken_by_callees(), "{caller}: {:?}", by_name(caller));
    }
    assert!(by_name("sq").in_library && by_name("sq").standalone);
    for g in failing(&grades).into_iter().filter(|g| g.name != "max2") {
        assert!(g.broken_by_callees(), "{} is only wrong through what it calls", g.name);
    }
}

#[test]
fn a_change_left_undone_fails_its_target_and_the_callers_that_must_change_on_their_own() {
    let hidden = HiddenCases::new(SEED);
    let old = library(&correct_library(Version::ORIGINAL));
    for update in UPDATES {
        let version = Version::ORIGINAL.with(update.change);
        let grades = grade(&old, version, &hidden);
        for (name, _) in update.programs {
            let g = grades.iter().find(|g| g.name == *name).expect("graded");
            assert!(!g.standalone, "{:?}: {name}'s own code is out of date", update.change);
        }
        for g in failing(&grades).into_iter().filter(|g| !update.programs.iter().any(|(n, _)| *n == g.name)) {
            assert!(
                g.broken_by_callees(),
                "{:?}: {} follows the change, so only its callee is wrong",
                update.change,
                g.name
            );
        }
    }
    // Followers exist, and are caught: dist follows abs.
    let grades = grade(&old, Version::ORIGINAL.with(Change::AbsSaturates), &hidden);
    let dist = grades.iter().find(|g| g.name == "dist").expect("graded");
    assert!(dist.broken_by_callees());
}

#[test]
fn a_caller_updated_before_its_callee_is_a_semantic_conflict() {
    let hidden = HiddenCases::new(SEED);
    let base = correct_programs(Version::ORIGINAL);
    let version = Version::ORIGINAL.with(Change::ClampOrder);
    let update = UPDATES.iter().find(|u| u.change == Change::ClampOrder).expect("an update");
    let callers: Vec<_> = update.programs.iter().copied().filter(|(n, _)| *n != "clamp3").collect();
    let grades = grade(&with(&base, &callers), version, &hidden);
    for (name, _) in &callers {
        let g = grades.iter().find(|g| g.name == *name).expect("graded");
        assert!(g.broken_by_callees(), "{name}: right for the new clamp3, wrong with the library's old one");
    }
    let version = Version::ORIGINAL.with(Change::SpreadCounts);
    let lib = with(&base, &[("flat3", "a b c spread3@ 1 =")]);
    assert!(grade_function(&lib, "flat3", version, &hidden).broken_by_callees());
}

#[test]
fn missing_and_wrong_arity_functions_score_nothing() {
    let hidden = HiddenCases::new(SEED);
    let mut programs = correct_programs(Version::ORIGINAL);
    programs.remove("sq");
    let mut mods = modules(&programs);
    mods[1].1.push_str("def nearer 2 : a\n");
    let lib = library(&mods);
    let grades = grade(&lib, Version::ORIGINAL, &hidden);
    let by_name = |n: &str| grades.iter().find(|g| g.name == n).expect("graded");
    assert!(!by_name("sq").present);
    assert!(by_name("sumsq").broken_by_callees(), "sumsq calls the missing sq");
    assert!(!by_name("nearer").present, "defined twice");
    let mut mods = modules(&correct_programs(Version::ORIGINAL));
    mods[0].1 = mods[0].1.replace("def sq 1", "def sq 2");
    let g = grade_function(&library(&mods), "sq", Version::ORIGINAL, &hidden);
    assert!(g.present && !g.in_library && !g.standalone);
}

#[test]
fn change_requests_apply_as_specified() {
    let o = Version::ORIGINAL;
    let v = |c: Change| o.with(c);
    assert_eq!(reference("clamp3", &[5, 0, 3], o), 3);
    assert_eq!(reference("clamp3", &[5, 0, 3], v(Change::ClampOrder)), 0, "(lo, hi, x) = (5, 0, 3)");
    assert_eq!(reference("clamp3", &[0, 3, 5], v(Change::ClampOrder)), 3);
    assert_eq!(reference("sign", &[0], o), 0);
    assert_eq!(reference("sign", &[0], v(Change::SignOfZero)), 1);
    assert_eq!(reference("mulsign", &[4, 0], v(Change::SignOfZero)), 4, "defined in terms of sign, so it follows");
    assert_eq!(reference("nearer", &[0, 1, -1], o), 1);
    assert_eq!(reference("nearer", &[0, 1, -1], v(Change::NearerTies)), -1);
    assert_eq!(reference("between", &[3, 0, 3], o), 1);
    assert_eq!(reference("between", &[3, 0, 3], v(Change::BetweenExclusive)), 0);
    assert_eq!(reference("abs", &[i64::MIN], o), i64::MIN);
    assert_eq!(reference("abs", &[i64::MIN], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(reference("dist", &[i64::MIN, 0], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(reference("spread3", &[1, 2, 3], o), 2);
    assert_eq!(reference("spread3", &[1, 2, 3], v(Change::SpreadCounts)), 3);
    assert_eq!(reference("flat3", &[2, 2, 2], v(Change::SpreadCounts)), 1, "flat3's contract doesn't change");
    assert_eq!(reference("clampsq", &[3, 0, 5], v(Change::ClampOrder)), 5, "nor clampsq's");

    assert_eq!(CHANGES.len(), 6);
    for c in CHANGES {
        let spec = spec_named(c.target).expect("targets a spec");
        assert_eq!(spec.description_at(o), spec.description);
        assert_eq!(spec.description_at(o.with(c.change)), c.description);
        assert!(!task_text(o).contains(c.text));
        assert!(task_text(o.with(c.change)).contains(c.text));
        assert!(task_text(Version::all()).contains(c.description));
        assert!(UPDATES.iter().any(|u| u.change == c.change && u.programs.iter().any(|(n, _)| *n == c.target)));
    }
    for s in SPECS.iter().filter(|s| CHANGES.iter().all(|c| c.target != s.name)) {
        assert_eq!(s.description_at(Version::all()), s.description, "{} isn't a target", s.name);
    }
}

// Public and hidden cases.

#[test]
fn hidden_cases_are_the_public_ones_plus_draws_and_edges() {
    let hidden = HiddenCases::new(SEED);
    for s in SPECS {
        let public = public_cases(s);
        let all = hidden.of(s);
        let arity = usize::from(s.arity);
        assert_eq!(public.len(), if s.arity == 1 { 13 } else { 60 }, "{}", s.name);
        assert_eq!(all.len(), public.len() + HIDDEN_DRAWS + EDGES.len().pow(u32::from(s.arity)));
        assert_eq!(&all[..public.len()], public.as_slice());
        assert!(
            public
                .iter()
                .all(|c| c[..arity].iter().all(|v| v.abs() <= PUBLIC_RANGE) && c[arity..].iter().all(|v| *v == 0))
        );
        let draws = &all[public.len()..public.len() + HIDDEN_DRAWS];
        assert!(draws.iter().all(|c| c[..arity].iter().all(|v| v.abs() <= HIDDEN_RANGE)));
        assert!(draws.iter().any(|c| c[0].abs() > PUBLIC_RANGE), "{}: draws go beyond the public range", s.name);
        if arity >= 2 {
            assert!(all.iter().any(|c| c[0] == i64::MIN && c[1] == i64::MAX), "{}: edge combinations", s.name);
        }
        assert!(all.iter().any(|c| c[0] == i64::MIN) && all.iter().any(|c| c[0] == i64::MAX));
    }
    let other = HiddenCases::new(SEED + 1);
    assert_ne!(hidden.of(&SPECS[0]), other.of(&SPECS[0]), "the seed decides the draws");
    assert_eq!(hidden.of(&SPECS[0]), HiddenCases::new(SEED).of(&SPECS[0]));
}

#[test]
fn hidden_cases_are_kept_out_of_everything_an_agent_can_read() {
    // The boundary is the module: task (agent-visible) never touches the grader.
    let code: Vec<&str> =
        include_str!("../src/task.rs").lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    for word in ["grader", "HiddenCases", "HIDDEN_", "EDGES", "grade"] {
        assert!(!code.iter().any(|l| l.contains(word)), "task.rs uses {word}");
    }
    // A check's report only ever names public inputs, and it passes programs
    // that are wrong only on hidden inputs, so it gives nothing away about them.
    let base = correct_programs(Version::ORIGINAL);
    for c in CANDIDATES {
        let spec = spec_named(c.name).expect("a spec");
        let public: Vec<String> =
            public_cases(spec).iter().map(|case| bracken_experiment::task::call_text(spec, case)).collect();
        for program in c.correct.iter().chain(c.wrong) {
            let lib = with(&base, &[(c.name, program)]);
            let result = check(&lib, c.name, Version::ORIGINAL).expect("checkable");
            if let Some(failure) = &result.first_failure {
                assert!(public.iter().any(|p| failure.starts_with(&format!("{p} "))), "{failure}");
            }
            assert_eq!(result.total, public.len());
        }
    }
    // The task text holds no hidden input either: the only numbers beyond the
    // public range in it are the stated limits and the change request's bounds.
    let allowed = ["10000", "64", "9223372036854775807", "9223372036854775808"];
    for version in [Version::ORIGINAL, Version::all()] {
        let text = task_text(version);
        for word in text.split(|ch: char| !ch.is_ascii_digit()).filter(|w| !w.is_empty()) {
            let n: i128 = word.parse().expect("digits");
            assert!(n <= i128::from(PUBLIC_RANGE) || allowed.contains(&word), "{word} in the task text");
        }
        assert!(text.contains(LANGUAGE));
    }
}

// Standalone scoring judges only the function's own code (finding edfe1c49ae99).

#[test]
fn standalone_never_hands_a_function_its_own_reference() {
    let hidden = HiddenCases::new(SEED);
    let base = correct_programs(Version::ORIGINAL);
    // Each hands its work back to the function under test: directly, through
    // the agent's own helper, or through a spec defined in terms of it.
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("abs", &[("abs", "a abs@")]),
        ("median3", &[("median3", "a b c median3@")]),
        ("cube", &[("cube", "a h@")]),
        ("abs", &[("abs", "a 0 dist@")]),
        ("sq", &[("sq", "a 0 sumsq@")]),
        ("max2", &[("max2", "a b b max3@")]),
        ("sign", &[("sign", "1 a mulsign@")]),
        ("dist", &[("dist", "a b a nearer@ a b - abs@ swap -")]),
    ];
    for (name, programs) in cases {
        let mut mods = modules(&{
            let mut p = base.clone();
            p.extend(programs.iter().copied());
            p
        });
        mods[0].1.push_str("def h 1 : a cube@\n");
        let g = grade_function(&library(&mods), name, Version::ORIGINAL, &hidden);
        assert!(!g.standalone && !g.in_library && !g.broken_by_callees(), "{name}: {programs:?}: {g:?}");
    }
    // A helper of the agent's own that does the work is the function's own code.
    let mut mods = modules(&{
        let mut p = base.clone();
        p.insert("abs", "a myabs@");
        p
    });
    mods[0].1.push_str("def myabs 1 : a 0 < a neg a ?\n");
    let g = grade_function(&library(&mods), "abs", Version::ORIGINAL, &hidden);
    assert!(g.standalone && g.in_library, "{g:?}");
}

#[test]
fn contracts_depend_only_on_their_own_layer_or_lower_without_cycles() {
    use bracken_experiment::task::defined_by;
    for s in SPECS {
        let mut seen = std::collections::BTreeSet::new();
        let mut todo: Vec<&str> = defined_by(s.name).to_vec();
        while let Some(d) = todo.pop() {
            assert_ne!(d, s.name, "{} depends on itself", s.name);
            let dep = spec_named(d).expect("a spec");
            assert!(dep.layer <= s.layer, "{} depends on {d}, a higher layer", s.name);
            if seen.insert(d) {
                todo.extend(defined_by(d));
            }
        }
    }
}

// Parser limits (finding 19dbb1de64fb).

#[test]
fn huge_modules_and_words_give_short_errors() {
    use bracken_experiment::lang::{MAX_MODULE_BYTES, MAX_NAME_LEN};
    let big = format!("def f 1 : a\n{}", "# padding\n".repeat(MAX_MODULE_BYTES / 10));
    let e = parse_module("base", &big).expect_err("too big");
    assert!(e.message.contains("over the limit"), "{e}");
    let lib = Library::parse([("base", big.as_str())]);
    assert!(lib.get("f").is_none() && lib.why_missing("f").len() < 200);

    let word = "x".repeat(10_000);
    for text in [
        format!("def f 1 : {word}"),
        format!("def f 1 : {word}@"),
        format!("def {word} 1 : a"),
        format!("def f {word} : a"),
        format!("def f 1 : 1{word}"),
    ] {
        let e = parse_module("base", &text).expect_err("a bad word");
        assert!(e.message.len() < 120, "{} bytes: {}", e.message.len(), &e.message[..120.min(e.message.len())]);
    }
    let long_name = "f".repeat(MAX_NAME_LEN + 1);
    assert!(parse_module("base", &format!("def {long_name} 1 : a")).is_err());
    assert!(parse_module("base", &format!("def {} 1 : a", "f".repeat(MAX_NAME_LEN))).is_ok());
}
