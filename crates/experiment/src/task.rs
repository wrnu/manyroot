//! The task as agents see it: the language, about 30 specs in four modules,
//! the change requests, the public cases and the public check.
//!
//! Everything here may reach an agent. The hidden cases and the scores are in
//! [`crate::grader`], and nothing in this module reads them.

use bracken_demo::rng::Rng;

use crate::lang::{Library, run};

/// The language, as the task states it to agents.
pub const LANGUAGE: &str = "A library is four modules of definitions, one per line: \
    `def <name> <arity> : <program>`, where arity is 1 to 3; `#` starts a comment. \
    A program is whitespace-separated tokens run on a stack of integers: \
    an integer pushes itself; `a`, `b` and `c` push the inputs; `+ - *` pop two values and push the result; \
    `< > =` pop two values and push 1 or 0; `neg` negates the top; `dup` copies it; `swap` swaps the top two; \
    `c x y ?` pushes x if c is not 0, else y; `name@` calls the library's function `name`, \
    popping as many values as it takes (the deepest is its `a`) and pushing its result. \
    A program must leave exactly one value. Arithmetic wraps around on 64-bit overflow. \
    A program stops with an error after 10000 steps or 64 nested calls. \
    A module with a line that doesn't parse contributes nothing, and a name defined twice counts as undefined.";

/// The library's modules, in the order the task lists them.
pub const MODULES: [&str; 4] = ["base", "order", "arith", "shape"];

/// One function to implement.
pub struct Spec {
    pub name: &'static str,
    pub module: &'static str,
    /// 0 needs only the language; layer n calls functions of layers below it.
    pub layer: u8,
    pub arity: u8,
    pub description: &'static str,
}

const fn spec(name: &'static str, module: &'static str, layer: u8, arity: u8, description: &'static str) -> Spec {
    Spec { name, module, layer, arity, description }
}

pub const SPECS: &[Spec] = &[
    spec("abs", "base", 0, 1, "abs(a): the absolute value of a (abs of the most negative integer is itself)"),
    spec("sign", "base", 0, 1, "sign(a): -1 if a < 0, 0 if a = 0, 1 if a > 0"),
    spec("max2", "base", 0, 2, "max2(a, b): the larger of a and b"),
    spec("min2", "base", 0, 2, "min2(a, b): the smaller of a and b"),
    spec("inc", "base", 0, 1, "inc(a): a + 1"),
    spec("dec", "base", 0, 1, "dec(a): a - 1"),
    spec("sq", "base", 0, 1, "sq(a): a * a"),
    spec("iszero", "base", 0, 1, "iszero(a): 1 if a = 0, else 0"),
    spec("clamp3", "order", 1, 3, "clamp3(x, lo, hi): min2(max2(x, lo), hi)"),
    spec("median3", "order", 1, 3, "median3(a, b, c): the middle one of a, b and c"),
    spec("max3", "order", 1, 3, "max3(a, b, c): the largest of a, b and c"),
    spec("min3", "order", 1, 3, "min3(a, b, c): the smallest of a, b and c"),
    spec("between", "order", 1, 3, "between(x, lo, hi): 1 if lo <= x <= hi, else 0"),
    spec("cmp", "order", 1, 2, "cmp(a, b): -1 if a < b, 0 if a = b, 1 if a > b"),
    spec("dist", "order", 1, 2, "dist(a, b): abs(a - b)"),
    spec("nearer", "order", 1, 3, "nearer(x, p, q): p if dist(x, p) <= dist(x, q), else q"),
    spec("sumsq", "arith", 2, 2, "sumsq(a, b): sq(a) + sq(b)"),
    spec("cube", "arith", 2, 1, "cube(a): a * sq(a)"),
    spec("poly", "arith", 2, 1, "poly(a): 2 * sq(a) - 3 * a + 1"),
    spec("mulsign", "arith", 2, 2, "mulsign(a, b): abs(a) * sign(b)"),
    spec("clampsq", "arith", 2, 3, "clampsq(a, lo, hi): sq(a) limited to lo..hi, that is min2(max2(sq(a), lo), hi)"),
    spec("manhattan", "arith", 2, 2, "manhattan(a, b): abs(a) + abs(b)"),
    spec("path3", "arith", 2, 3, "path3(a, b, c): dist(a, b) + dist(b, c)"),
    spec("relu", "arith", 2, 1, "relu(a): max2(a, 0)"),
    spec("near2", "shape", 3, 3, "near2(a, b, r): 1 if manhattan(a, b) <= r, else 0"),
    spec(
        "snap",
        "shape",
        3,
        3,
        "snap(x, lo, hi): whichever of lo and hi is nearer to x limited to lo..hi: nearer(min2(max2(x, lo), hi), lo, hi)",
    ),
    spec("spread3", "shape", 3, 3, "spread3(a, b, c): max3(a, b, c) - min3(a, b, c)"),
    spec("energy", "shape", 3, 2, "energy(a, b): sumsq(a, b) - dist(a, b)"),
    spec("bucket", "shape", 3, 1, "bucket(a): sign(a) * min2(max2(abs(a), 0), 3)"),
    spec("flat3", "shape", 3, 3, "flat3(a, b, c): 1 if a, b and c are all equal, else 0"),
];

/// The specs `name`'s contract is defined in terms of: the ones its
/// reference calls, so it follows their change requests. Each is in the same
/// layer or a lower one, and there are no cycles.
pub fn defined_by(name: &str) -> &'static [&'static str] {
    match name {
        "dist" | "manhattan" => &["abs"],
        "nearer" | "path3" => &["dist"],
        "sumsq" | "cube" | "poly" | "clampsq" => &["sq"],
        "mulsign" | "bucket" => &["abs", "sign"],
        "near2" => &["manhattan"],
        "snap" => &["nearer"],
        "spread3" => &["max3", "min3"],
        "energy" => &["sumsq", "dist"],
        _ => &[],
    }
}

/// The spec named `name`.
pub fn spec_named(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// A change request: the experiment releases all of them at once, partway through a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    ClampOrder,
    SignOfZero,
    NearerTies,
    BetweenExclusive,
    AbsSaturates,
    SpreadCounts,
}

pub struct ChangeRequest {
    pub change: Change,
    /// The spec whose contract changes.
    pub target: &'static str,
    /// The spec's description once the change applies.
    pub description: &'static str,
    /// The request as agents read it.
    pub text: &'static str,
}

pub const CHANGES: &[ChangeRequest] = &[
    ChangeRequest {
        change: Change::ClampOrder,
        target: "clamp3",
        description: "clamp3(lo, hi, x): min2(max2(x, lo), hi)",
        text: "clamp3 now takes its inputs as (lo, hi, x) instead of (x, lo, hi). Every function that calls it must pass them in the new order.",
    },
    ChangeRequest {
        change: Change::SignOfZero,
        target: "sign",
        description: "sign(a): -1 if a < 0, else 1",
        text: "sign(0) is now 1: sign returns -1 for negative inputs and 1 for all others. Functions defined in terms of sign follow it.",
    },
    ChangeRequest {
        change: Change::NearerTies,
        target: "nearer",
        description: "nearer(x, p, q): p if dist(x, p) < dist(x, q), else q",
        text: "nearer now breaks ties toward q: it returns p only if p is strictly nearer to x. Functions defined in terms of nearer follow it.",
    },
    ChangeRequest {
        change: Change::BetweenExclusive,
        target: "between",
        description: "between(x, lo, hi): 1 if lo <= x < hi, else 0",
        text: "between now excludes hi: it is 1 if lo <= x < hi.",
    },
    ChangeRequest {
        change: Change::AbsSaturates,
        target: "abs",
        description: "abs(a): the absolute value of a, except that abs of the most negative integer is the most positive one",
        text: "abs no longer wraps: abs(-9223372036854775808) is now 9223372036854775807. Functions defined in terms of abs follow it.",
    },
    ChangeRequest {
        change: Change::SpreadCounts,
        target: "spread3",
        description: "spread3(a, b, c): max3(a, b, c) - min3(a, b, c) + 1, the number of integers from the smallest to the largest",
        text: "spread3 now counts the integers from the smallest input to the largest, both included: max3 - min3 + 1. Code that relies on spread3 must allow for it.",
    },
];

/// Which change requests apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Version(u8);

impl Version {
    /// Before any change request.
    pub const ORIGINAL: Version = Version(0);

    /// After every change request: the contracts the experiment grades the end of a run against.
    pub fn all() -> Version {
        CHANGES.iter().fold(Version::ORIGINAL, |v, c| v.with(c.change))
    }

    pub fn with(self, change: Change) -> Version {
        Version(self.0 | (1 << change as u8))
    }

    pub fn has(self, change: Change) -> bool {
        self.0 & (1 << change as u8) != 0
    }

    /// The change requests that apply, in the order the task lists them.
    pub fn changes(self) -> impl Iterator<Item = &'static ChangeRequest> {
        CHANGES.iter().filter(move |c| self.has(c.change))
    }
}

impl Spec {
    /// The description that holds under `version`.
    pub fn description_at(&self, version: Version) -> &'static str {
        version.changes().filter(|c| c.target == self.name).last().map_or(self.description, |c| c.description)
    }
}

/// The reference implementation of `name` under `version`, on one input per
/// argument. A spec defined in terms of other functions calls their
/// references, so it follows their change requests. Panics on a name that
/// isn't a spec or the wrong number of inputs: callers resolve those first.
pub fn reference(name: &str, args: &[i64], version: Version) -> i64 {
    let r = |callee: &str, args: &[i64]| {
        assert!(defined_by(name).contains(&callee), "{name}'s reference calls {callee}, outside defined_by");
        reference(callee, args, version)
    };
    let arg = |i: usize| args[i];
    let limit = |x: i64, lo: i64, hi: i64| x.max(lo).min(hi);
    match (name, args.len()) {
        ("abs", 1) if version.has(Change::AbsSaturates) => arg(0).saturating_abs(),
        ("abs", 1) => arg(0).wrapping_abs(),
        ("sign", 1) if version.has(Change::SignOfZero) => {
            if arg(0) < 0 {
                -1
            } else {
                1
            }
        }
        ("sign", 1) => arg(0).signum(),
        ("max2", 2) => arg(0).max(arg(1)),
        ("min2", 2) => arg(0).min(arg(1)),
        ("inc", 1) => arg(0).wrapping_add(1),
        ("dec", 1) => arg(0).wrapping_sub(1),
        ("sq", 1) => arg(0).wrapping_mul(arg(0)),
        ("iszero", 1) => i64::from(arg(0) == 0),
        ("clamp3", 3) if version.has(Change::ClampOrder) => limit(arg(2), arg(0), arg(1)),
        ("clamp3", 3) => limit(arg(0), arg(1), arg(2)),
        ("median3", 3) => {
            let mut v = [arg(0), arg(1), arg(2)];
            v.sort_unstable();
            v[1]
        }
        ("max3", 3) => arg(0).max(arg(1)).max(arg(2)),
        ("min3", 3) => arg(0).min(arg(1)).min(arg(2)),
        ("between", 3) if version.has(Change::BetweenExclusive) => i64::from(arg(1) <= arg(0) && arg(0) < arg(2)),
        ("between", 3) => i64::from(arg(1) <= arg(0) && arg(0) <= arg(2)),
        ("cmp", 2) => match arg(0).cmp(&arg(1)) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
        ("dist", 2) => r("abs", &[arg(0).wrapping_sub(arg(1))]),
        ("nearer", 3) => {
            let (dp, dq) = (r("dist", &[arg(0), arg(1)]), r("dist", &[arg(0), arg(2)]));
            let p_wins = if version.has(Change::NearerTies) { dp < dq } else { dp <= dq };
            if p_wins { arg(1) } else { arg(2) }
        }
        ("sumsq", 2) => r("sq", &[arg(0)]).wrapping_add(r("sq", &[arg(1)])),
        ("cube", 1) => arg(0).wrapping_mul(r("sq", &[arg(0)])),
        ("poly", 1) => r("sq", &[arg(0)]).wrapping_mul(2).wrapping_sub(arg(0).wrapping_mul(3)).wrapping_add(1),
        ("mulsign", 2) => r("abs", &[arg(0)]).wrapping_mul(r("sign", &[arg(1)])),
        ("clampsq", 3) => limit(r("sq", &[arg(0)]), arg(1), arg(2)),
        ("manhattan", 2) => r("abs", &[arg(0)]).wrapping_add(r("abs", &[arg(1)])),
        ("path3", 3) => r("dist", &[arg(0), arg(1)]).wrapping_add(r("dist", &[arg(1), arg(2)])),
        ("relu", 1) => arg(0).max(0),
        ("near2", 3) => i64::from(r("manhattan", &[arg(0), arg(1)]) <= arg(2)),
        ("snap", 3) => r("nearer", &[limit(arg(0), arg(1), arg(2)), arg(1), arg(2)]),
        ("spread3", 3) => {
            let spread = r("max3", args).wrapping_sub(r("min3", args));
            if version.has(Change::SpreadCounts) { spread.wrapping_add(1) } else { spread }
        }
        ("energy", 2) => r("sumsq", args).wrapping_sub(r("dist", args)),
        ("bucket", 1) => r("sign", &[arg(0)]).wrapping_mul(limit(r("abs", &[arg(0)]), 0, 3)),
        ("flat3", 3) => i64::from(arg(0) == arg(1) && arg(1) == arg(2)),
        _ => panic!("no reference for {name} with {} inputs", args.len()),
    }
}

/// Inputs per public case set, for functions of 2 or 3 inputs. A function of
/// one input gets all 13 inputs from -6 to 6.
pub const PUBLIC_CASES: usize = 60;
/// Public inputs are drawn from -PUBLIC_RANGE..=PUBLIC_RANGE.
pub const PUBLIC_RANGE: i64 = 6;
/// Public cases are the same in every run; they are no secret.
const PUBLIC_SEED: u64 = 0x5747_4c49_4250_5542;

/// One set of inputs, `a`, `b` and `c`; a function uses the first `arity`.
pub type Case = [i64; 3];

/// The public cases of `spec`: the only inputs an agent's check ever runs.
pub fn public_cases(spec: &Spec) -> Vec<Case> {
    let range = -PUBLIC_RANGE..=PUBLIC_RANGE;
    let mut grid: Vec<Case> = match spec.arity {
        1 => range.map(|a| [a, 0, 0]).collect(),
        2 => range.clone().flat_map(|a| range.clone().map(move |b| [a, b, 0])).collect(),
        _ => range
            .clone()
            .flat_map(|a| range.clone().flat_map(move |b| (-PUBLIC_RANGE..=PUBLIC_RANGE).map(move |c| [a, b, c])))
            .collect(),
    };
    let index = SPECS.iter().position(|s| s.name == spec.name).unwrap_or(SPECS.len());
    Rng::derive(PUBLIC_SEED, index as u64).shuffle(&mut grid);
    grid.truncate(PUBLIC_CASES);
    grid
}

/// The call `spec(case)` as text, for messages.
pub fn call_text(spec: &Spec, case: &Case) -> String {
    let args: Vec<String> = case[..usize::from(spec.arity)].iter().map(i64::to_string).collect();
    format!("{}({})", spec.name, args.join(", "))
}

/// The outcome of a public check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub passed: usize,
    pub total: usize,
    pub first_failure: Option<String>,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.first_failure.is_none()
    }

    /// One line for a tool result or evidence details.
    pub fn details(&self) -> String {
        match &self.first_failure {
            None => format!("{}/{} cases passed", self.passed, self.total),
            Some(f) => format!("{f} ({}/{} cases passed)", self.passed, self.total),
        }
    }
}

/// The agents' check: runs `name`'s public cases in `library` (an agent's own
/// working copy), with every call resolved in that library, against the specs
/// under `version`. Errs when there's nothing to run.
pub fn check(library: &Library, name: &str, version: Version) -> Result<Check, String> {
    let spec = spec_named(name).ok_or_else(|| format!("`{name}` is not a function of the task"))?;
    let def = library.get(name).ok_or_else(|| library.why_missing(name))?;
    if def.arity != spec.arity {
        return Err(format!("`{name}` must take {} inputs, not {}", spec.arity, def.arity));
    }
    let cases = public_cases(spec);
    let mut passed = 0;
    let mut first_failure = None;
    for case in &cases {
        let args = &case[..usize::from(spec.arity)];
        let want = reference(name, args, version);
        match run(def, args, library) {
            Ok(got) if got == want => passed += 1,
            outcome => {
                let got = match outcome {
                    Ok(got) => format!("gave {got}"),
                    Err(e) => format!("failed: {e}"),
                };
                first_failure.get_or_insert(format!("{} {got}, expected {want}", call_text(spec, case)));
            }
        }
    }
    Ok(Check { passed, total: cases.len(), first_failure })
}

/// The task as agents read it under `version`: the language, the modules and
/// their specs, and the change requests that apply.
pub fn task_text(version: Version) -> String {
    let mut text = format!("Write this library. {LANGUAGE}\n");
    for module in MODULES {
        text.push_str(&format!("\nModule {module}:\n"));
        for s in SPECS.iter().filter(|s| s.module == module) {
            text.push_str(&format!("- {}\n", s.description_at(version)));
        }
    }
    let changes: Vec<&ChangeRequest> = version.changes().collect();
    if !changes.is_empty() {
        text.push_str("\nChange requests (the descriptions above already include them):\n");
        for c in changes {
            text.push_str(&format!("- {}\n", c.text));
        }
    }
    text
}
