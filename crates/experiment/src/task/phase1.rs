//! Phase 1's task, `stacklib`: about 30 specs in four modules and four layers,
//! and six change requests released together. Kept as Phase 1 ran it, so its
//! results stay reproducible: a test pins the task text's hashes to the runs'.

use super::{Change, ChangeRequest, Spec, Version, spec};

/// The language, as the task states it to agents.
pub(super) const LANGUAGE: &str = "A library is four modules of definitions, one per line: \
    `def <name> <arity> : <program>`, where arity is 1 to 3; `#` starts a comment. \
    A program is whitespace-separated tokens run on a stack of integers: \
    an integer pushes itself; `a`, `b` and `c` push the inputs; `+ - *` pop two values and push the result; \
    `< > =` pop two values and push 1 or 0; `neg` negates the top; `dup` copies it; `swap` swaps the top two; \
    `c x y ?` pushes x if c is not 0, else y; `name@` calls the library's function `name`, \
    popping as many values as it takes (the deepest is its `a`) and pushing its result. \
    A program must leave exactly one value. Arithmetic wraps around on 64-bit overflow. \
    A program stops with an error after 10000 steps or 64 nested calls. \
    A module with a line that doesn't parse contributes nothing, and a name defined twice counts as undefined.";

pub(super) const MODULES: &[&str] = &["base", "order", "arith", "shape"];

pub(super) const SPECS: &[Spec] = &[
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

pub(super) const CHANGES: &[ChangeRequest] = &[
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

/// The specs `name`'s contract is defined in terms of: the ones its
/// reference calls, so it follows their change requests. Each is in the same
/// layer or a lower one, and there are no cycles.
pub(super) fn defined_by(name: &str) -> &'static [&'static str] {
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

/// The reference implementation of `name` under `version`, if it is a spec
/// with that many inputs; `r` evaluates a callee's reference.
pub(super) fn reference(name: &str, args: &[i64], version: Version, r: &dyn Fn(&str, &[i64]) -> i64) -> Option<i64> {
    let arg = |i: usize| args[i];
    let limit = |x: i64, lo: i64, hi: i64| x.max(lo).min(hi);
    Some(match (name, args.len()) {
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
        _ => return None,
    })
}
