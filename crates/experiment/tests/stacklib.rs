//! stacklib: the language, the specs and change requests, the public check and
//! the hidden grader (docs/design/phase1-experiment.md, section 1).

use bracken_experiment::candidates::{
    CANDIDATES, Programs, UPDATES, candidates, correct_library, correct_programs, covers_every_spec, modules,
    of as candidates_of,
};
use bracken_experiment::grader::{
    EDGES, FunctionGrade, HIDDEN_DRAWS, HIDDEN_RANGE, HiddenCases, grade, grade_function,
};
use bracken_experiment::lang::{Library, MAX_DEPTH, parse_module, run};
use bracken_experiment::task::{Change, PUBLIC_RANGE, Task, Version};

const SEED: u64 = 7;
/// Phase 1's task, which most of these tests are about; those that hold for every task run on both.
const T: Task = Task::Phase1;

fn library(modules: &[(&'static str, String)]) -> Library {
    Library::parse(modules.iter().map(|(m, t)| (*m, t.as_str())))
}

fn with(programs: &Programs, changes: &[(&'static str, &'static str)]) -> Library {
    with_in(T, programs, changes)
}

fn with_in(task: Task, programs: &Programs, changes: &[(&'static str, &'static str)]) -> Library {
    let mut programs = programs.clone();
    programs.extend(changes.iter().copied());
    library(&modules(task, &programs))
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
    assert_eq!(T.specs().len(), 30);
    assert!(covers_every_spec(T));
    for module in T.modules() {
        assert!(T.specs().iter().filter(|s| s.module == *module).count() >= 6, "{module}");
    }
    for s in T.specs() {
        assert_eq!(
            usize::from(s.layer),
            T.modules().iter().position(|m| *m == s.module).expect("a module"),
            "{}",
            s.name
        );
        let c = candidates(T, s.name);
        assert!(c.correct.len() >= 2 && c.wrong.len() >= 2, "{}", s.name);
        for program in c.correct.iter().chain(c.wrong) {
            let def = parse_module(s.module, &format!("def {} {} : {program}", s.name, s.arity));
            assert!(def.is_ok(), "{}: {program}: {def:?}", s.name);
        }
        let _ = T.reference(s.name, &[1, 2, 3][..usize::from(s.arity)], T.all());
    }
    let mut seen = std::collections::BTreeSet::new();
    assert!(T.specs().iter().all(|s| seen.insert(s.name)), "names are unique");
}

#[test]
fn the_correct_library_scores_fully_under_every_change() {
    for task in Task::ALL {
        let hidden = HiddenCases::new(task, SEED);
        let mut versions = vec![Version::ORIGINAL, task.all()];
        versions.extend(task.changes().iter().map(|c| Version::ORIGINAL.with(c.change)));
        for version in versions {
            let lib = library(&correct_library(task, version));
            assert!(lib.broken.is_empty() && lib.duplicates.is_empty());
            let grades = grade(&lib, version, &hidden);
            assert_eq!(grades.len(), task.specs().len());
            assert!(failing(&grades).is_empty(), "{task:?} {version:?}: {:?}", failing(&grades));
            assert!(grades.iter().all(|g| g.present && g.note.is_none()));
        }
    }
}

#[test]
fn every_correct_candidate_passes_and_every_wrong_one_fails_on_its_own() {
    for task in Task::ALL {
        let hidden = HiddenCases::new(task, SEED);
        let base = correct_programs(task, Version::ORIGINAL);
        for c in candidates_of(task) {
            for program in c.correct {
                let g = grade_function(&with_in(task, &base, &[(c.name, program)]), c.name, Version::ORIGINAL, &hidden);
                assert!(g.in_library && g.standalone, "{task:?} {}: {program}: {:?}", c.name, g.note);
            }
            for program in c.wrong {
                let g = grade_function(&with_in(task, &base, &[(c.name, program)]), c.name, Version::ORIGINAL, &hidden);
                assert!(!g.standalone && !g.in_library, "{task:?} {}: {program} should be wrong", c.name);
            }
        }
    }
}

#[test]
fn each_spec_has_a_wrong_candidate_the_public_check_passes() {
    for task in Task::ALL {
        let base = correct_programs(task, Version::ORIGINAL);
        for c in candidates_of(task) {
            let subtle = c.wrong.last().expect("wrong candidates");
            let lib = with_in(task, &base, &[(c.name, subtle)]);
            let check = task.check(&lib, c.name, Version::ORIGINAL).expect("checkable");
            assert!(check.ok(), "{task:?} {}: {subtle}: {}", c.name, check.details());
        }
    }
}

#[test]
fn a_broken_callee_shows_as_callers_right_alone_but_wrong_in_the_library() {
    let hidden = HiddenCases::new(T, SEED);
    let lib = with(&correct_programs(T, Version::ORIGINAL), &[("max2", "a b < a b ?")]);
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
    let hidden = HiddenCases::new(T, SEED);
    let old = library(&correct_library(T, Version::ORIGINAL));
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
    let hidden = HiddenCases::new(T, SEED);
    let base = correct_programs(T, Version::ORIGINAL);
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
    let hidden = HiddenCases::new(T, SEED);
    let mut programs = correct_programs(T, Version::ORIGINAL);
    programs.remove("sq");
    let mut mods = modules(T, &programs);
    mods[1].1.push_str("def nearer 2 : a\n");
    let lib = library(&mods);
    let grades = grade(&lib, Version::ORIGINAL, &hidden);
    let by_name = |n: &str| grades.iter().find(|g| g.name == n).expect("graded");
    assert!(!by_name("sq").present);
    assert!(by_name("sumsq").broken_by_callees(), "sumsq calls the missing sq");
    assert!(!by_name("nearer").present, "defined twice");
    let mut mods = modules(T, &correct_programs(T, Version::ORIGINAL));
    mods[0].1 = mods[0].1.replace("def sq 1", "def sq 2");
    let g = grade_function(&library(&mods), "sq", Version::ORIGINAL, &hidden);
    assert!(g.present && !g.in_library && !g.standalone);
}

#[test]
fn change_requests_apply_as_specified() {
    let o = Version::ORIGINAL;
    let v = |c: Change| o.with(c);
    assert_eq!(T.reference("clamp3", &[5, 0, 3], o), 3);
    assert_eq!(T.reference("clamp3", &[5, 0, 3], v(Change::ClampOrder)), 0, "(lo, hi, x) = (5, 0, 3)");
    assert_eq!(T.reference("clamp3", &[0, 3, 5], v(Change::ClampOrder)), 3);
    assert_eq!(T.reference("sign", &[0], o), 0);
    assert_eq!(T.reference("sign", &[0], v(Change::SignOfZero)), 1);
    assert_eq!(T.reference("mulsign", &[4, 0], v(Change::SignOfZero)), 4, "defined in terms of sign, so it follows");
    assert_eq!(T.reference("nearer", &[0, 1, -1], o), 1);
    assert_eq!(T.reference("nearer", &[0, 1, -1], v(Change::NearerTies)), -1);
    assert_eq!(T.reference("between", &[3, 0, 3], o), 1);
    assert_eq!(T.reference("between", &[3, 0, 3], v(Change::BetweenExclusive)), 0);
    assert_eq!(T.reference("abs", &[i64::MIN], o), i64::MIN);
    assert_eq!(T.reference("abs", &[i64::MIN], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(T.reference("dist", &[i64::MIN, 0], v(Change::AbsSaturates)), i64::MAX);
    assert_eq!(T.reference("spread3", &[1, 2, 3], o), 2);
    assert_eq!(T.reference("spread3", &[1, 2, 3], v(Change::SpreadCounts)), 3);
    assert_eq!(T.reference("flat3", &[2, 2, 2], v(Change::SpreadCounts)), 1, "flat3's contract doesn't change");
    assert_eq!(T.reference("clampsq", &[3, 0, 5], v(Change::ClampOrder)), 5, "nor clampsq's");

    assert_eq!(T.changes().len(), 6);
    for c in T.changes() {
        let spec = T.spec_named(c.target).expect("targets a spec");
        assert_eq!(T.description_at(spec, o), spec.description);
        assert_eq!(T.description_at(spec, o.with(c.change)), c.description);
        assert!(!T.task_text(o).contains(c.text));
        assert!(T.task_text(o.with(c.change)).contains(c.text));
        assert!(T.task_text(T.all()).contains(c.description));
        assert!(UPDATES.iter().any(|u| u.change == c.change && u.programs.iter().any(|(n, _)| *n == c.target)));
    }
    for s in T.specs().iter().filter(|s| T.changes().iter().all(|c| c.target != s.name)) {
        assert_eq!(T.description_at(s, T.all()), s.description, "{} isn't a target", s.name);
    }
}

// Public and hidden cases.

#[test]
fn hidden_cases_are_the_public_ones_plus_draws_and_edges() {
    for task in Task::ALL {
        hidden_cases_of(task);
    }
}

fn hidden_cases_of(task: Task) {
    let hidden = HiddenCases::new(task, SEED);
    for s in task.specs() {
        let public = task.public_cases(s);
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
    let other = HiddenCases::new(task, SEED + 1);
    assert_ne!(hidden.of(&task.specs()[0]), other.of(&task.specs()[0]), "the seed decides the draws");
    assert_eq!(hidden.of(&task.specs()[0]), HiddenCases::new(task, SEED).of(&task.specs()[0]));
}

#[test]
fn hidden_cases_are_kept_out_of_everything_an_agent_can_read() {
    // The boundary is the module: task (agent-visible) never touches the grader.
    let files =
        [include_str!("../src/task.rs"), include_str!("../src/task/phase1.rs"), include_str!("../src/task/phase2.rs")];
    for file in files {
        let code: Vec<&str> = file.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        for word in ["grader", "HiddenCases", "HIDDEN_", "EDGES", "grade_function"] {
            assert!(!code.iter().any(|l| l.contains(word)), "the task's code uses {word}");
        }
    }
    // A check's report only ever names public inputs, and it passes programs
    // that are wrong only on hidden inputs, so it gives nothing away about them.
    let base = correct_programs(T, Version::ORIGINAL);
    for c in CANDIDATES {
        let spec = T.spec_named(c.name).expect("a spec");
        let public: Vec<String> =
            T.public_cases(spec).iter().map(|case| bracken_experiment::task::call_text(spec, case)).collect();
        for program in c.correct.iter().chain(c.wrong) {
            let lib = with(&base, &[(c.name, program)]);
            let result = T.check(&lib, c.name, Version::ORIGINAL).expect("checkable");
            if let Some(failure) = &result.first_failure {
                assert!(public.iter().any(|p| failure.starts_with(&format!("{p} "))), "{failure}");
            }
            assert_eq!(result.total, public.len());
        }
    }
    // The task text holds no hidden input either: the only numbers beyond the
    // public range in it are the stated limits and the change request's bounds.
    let allowed = ["10000", "64", "9223372036854775807", "9223372036854775808"];
    for version in [Version::ORIGINAL, T.all()] {
        let text = T.task_text(version);
        for word in text.split(|ch: char| !ch.is_ascii_digit()).filter(|w| !w.is_empty()) {
            let n: i128 = word.parse().expect("digits");
            assert!(n <= i128::from(PUBLIC_RANGE) || allowed.contains(&word), "{word} in the task text");
        }
        assert!(text.contains(T.language()));
    }
}

// Standalone scoring judges only the function's own code (finding edfe1c49ae99).

#[test]
fn standalone_never_hands_a_function_its_own_reference() {
    let hidden = HiddenCases::new(T, SEED);
    let base = correct_programs(T, Version::ORIGINAL);
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
        let mut mods = modules(T, &{
            let mut p = base.clone();
            p.extend(programs.iter().copied());
            p
        });
        mods[0].1.push_str("def h 1 : a cube@\n");
        let g = grade_function(&library(&mods), name, Version::ORIGINAL, &hidden);
        assert!(!g.standalone && !g.in_library && !g.broken_by_callees(), "{name}: {programs:?}: {g:?}");
    }
    // A helper of the agent's own that does the work is the function's own code.
    let mut mods = modules(T, &{
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
    for task in Task::ALL {
        for s in task.specs() {
            let mut seen = std::collections::BTreeSet::new();
            let mut todo: Vec<&str> = task.defined_by(s.name).to_vec();
            while let Some(d) = todo.pop() {
                assert_ne!(d, s.name, "{} depends on itself", s.name);
                let dep = task.spec_named(d).expect("a spec");
                assert!(dep.layer <= s.layer, "{} depends on {d}, a higher layer", s.name);
                assert!(task.contract_of(d).is_none(), "{} is defined by {d}, which has no reference", s.name);
                if seen.insert(d) {
                    todo.extend(task.defined_by(d));
                }
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

// Phase 1 stays reproducible.

/// The task texts and tool lists Phase 1's runs were given, as their plans recorded them (replicate 3,
/// 2026-10-28): the selector's Phase 1 is the task those runs ran, byte for byte, but for the wording of
/// Bracken mode's `accepted` tool, which changed with the acceptance rule.
#[test]
fn phase_one_gives_agents_what_its_runs_were_given() {
    use bracken_experiment::driver::core_tools;
    use bracken_experiment::mode::mode_named;
    let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    assert_eq!(
        hash(T.task_text(Version::ORIGINAL).as_bytes()),
        "13145e3a097320322b27788191ce9585e7aa93ce5f28f13fab6a2b5fdc996a9e"
    );
    assert_eq!(
        hash(T.task_text(T.all()).as_bytes()),
        "e4ec815192c7564890748ef1ca5d82fb6246bc0bd3096202890f9c9fb2eb39ba"
    );
    for (mode, want) in [
        ("git", "ac51023e70eaa804cf65da7c351ceda9972f31121957eb11a6c373fa14f2b8d2"),
        ("crdt", "fc8960a43a54e25b2508e87b630deecbe756eb922d385bba5ac075dbed75f12b"),
    ] {
        let mut tools = mode_named(mode).expect("a mode").tools(T);
        tools.extend(core_tools());
        assert_eq!(hash(&serde_json::to_vec(&tools).expect("JSON")), want, "{mode}'s tools");
    }
    // Bracken mode's too, but for the one description PR #55 rewrote with the acceptance rule (decision
    // `a4d8622c75d5` point 2), in every task.
    let mut tools = mode_named("bracken").expect("a mode").tools(T);
    tools.extend(core_tools());
    let accepted = tools.iter_mut().find(|t| t.name == "accepted").expect("the accepted tool");
    assert!(accepted.description.contains("(the earliest, if several) is accepted"), "{}", accepted.description);
    accepted.description = "What counts in your replica, per function: the implementation its author related to the \
        current spec claim that has a pass from someone else and no one else's latest verdict a fail; the one with most \
        passes wins";
    assert_eq!(
        hash(&serde_json::to_vec(&tools).expect("JSON")),
        "9d5435906a3946ee4c02998f86241d1029d1b6d83f43fada8f488b07ddbb13e0",
        "bracken's tools"
    );
    // A plan from before the selector names no task, and reads as Phase 1's.
    let mut inputs = serde_json::to_value(bracken_experiment::plan::PlanInputs::default()).expect("JSON");
    inputs.as_object_mut().expect("an object").remove("task");
    let inputs: bracken_experiment::plan::PlanInputs = serde_json::from_value(inputs).expect("an old plan");
    assert_eq!(inputs.task, Task::Phase1);
    // Phase 2's are its own.
    assert_ne!(Task::Phase2.task_text(Version::ORIGINAL), T.task_text(Version::ORIGINAL));
    assert_ne!(mode_named("git").expect("a mode").tools(Task::Phase2).len(), 0);
}

/// Phase 1's hidden cases and reference answers, under the original specs and after every change, are
/// those Phase 1's runs were graded on: hashes computed with the build before the selector (main at
/// 1a84dd1), for two seeds.
#[test]
fn phase_one_is_graded_as_its_runs_were() {
    for (seed, want) in [
        (1, "d0d1384661e33e1841a8b4d0bd0d414dae81fb3a54c286bd340029a91874e19a"),
        (7, "68abcc2b311a6a2ff4f5f949e21f55d40ee6b502a9987c6dd592144bf798b8e0"),
    ] {
        let hidden = HiddenCases::new(T, seed);
        let mut h = blake3::Hasher::new();
        for s in T.specs() {
            for c in hidden.of(s) {
                let args = &c[..usize::from(s.arity)];
                for v in c {
                    h.update(&v.to_le_bytes());
                }
                for version in [Version::ORIGINAL, T.all()] {
                    h.update(&T.reference(s.name, args, version).to_le_bytes());
                }
            }
        }
        assert_eq!(h.finalize().to_hex().to_string(), want, "seed {seed}");
    }
}
