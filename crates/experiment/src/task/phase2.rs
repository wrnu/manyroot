//! Phase 2's task (design `docs/design/phase2-task.md`, Candidate A with
//! Candidate C's open contracts): 90 functions in eight modules and five
//! layers. Every function above layer 1 calls two or three functions from at
//! least two modules of lower layers, so a change in one module reaches
//! several, and twelve functions call `clamp3`. Two open contracts, `rank` and
//! `none`, leave a convention to the library: their members are right when
//! they agree with each other, whatever the library chose.

use super::{Caller, Contract, Probe, Property, Spec, Version, spec};

/// The language, as the task states it to agents.
pub(super) const LANGUAGE: &str = "A library is eight modules of definitions, one per line: \
    `def <name> <arity> : <program>`, where arity is 1 to 3; `#` starts a comment. \
    A program is whitespace-separated tokens run on a stack of integers: \
    an integer pushes itself; `a`, `b` and `c` push the inputs; `+ - *` pop two values and push the result; \
    `< > =` pop two values and push 1 or 0; `neg` negates the top; `dup` copies it; `swap` swaps the top two; \
    `c x y ?` pushes x if c is not 0, else y; `name@` calls the library's function `name`, \
    popping as many values as it takes (the deepest is its `a`) and pushing its result. \
    A program must leave exactly one value. Arithmetic wraps around on 64-bit overflow. \
    A program stops with an error after 10000 steps or 64 nested calls. \
    A module with a line that doesn't parse contributes nothing, and a name defined twice counts as undefined. \
    An open contract is a convention the task leaves to the library: its functions are right when they agree \
    with each other, whatever the convention, and they are checked together.";

pub(super) const MODULES: &[&str] = &["base", "logic", "order", "step", "arith", "range", "shape", "score"];

pub(super) const MODULE_FILES: &[&str] =
    &["base.stk", "logic.stk", "order.stk", "step.stk", "arith.stk", "range.stk", "shape.stk", "score.stk"];

pub(super) const SPECS: &[Spec] = &[
    // Layer 0.
    spec("abs", "base", 0, 1, "abs(a): the absolute value of a (abs of the most negative integer is itself)"),
    spec("sign", "base", 0, 1, "sign(a): -1 if a < 0, 0 if a = 0, 1 if a > 0"),
    spec("max2", "base", 0, 2, "max2(a, b): the larger of a and b"),
    spec("min2", "base", 0, 2, "min2(a, b): the smaller of a and b"),
    spec("inc", "base", 0, 1, "inc(a): a + 1"),
    spec("dec", "base", 0, 1, "dec(a): a - 1"),
    spec("sq", "base", 0, 1, "sq(a): a * a"),
    spec("double", "base", 0, 1, "double(a): a + a"),
    spec("triple", "base", 0, 1, "triple(a): 3 * a"),
    spec("opp", "base", 0, 1, "opp(a): -a (opp of the most negative integer is itself)"),
    spec("diff", "base", 0, 2, "diff(a, b): a - b"),
    spec("add3", "base", 0, 3, "add3(a, b, c): a + b + c"),
    spec("iszero", "logic", 0, 1, "iszero(a): 1 if a = 0, else 0"),
    spec("eq", "logic", 0, 2, "eq(a, b): 1 if a = b, else 0"),
    spec("neq", "logic", 0, 2, "neq(a, b): 1 if a and b differ, else 0"),
    spec("lt", "logic", 0, 2, "lt(a, b): 1 if a < b, else 0"),
    spec("le", "logic", 0, 2, "le(a, b): 1 if a <= b, else 0"),
    spec("gt", "logic", 0, 2, "gt(a, b): 1 if a > b, else 0"),
    spec("ge", "logic", 0, 2, "ge(a, b): 1 if a >= b, else 0"),
    spec("and", "logic", 0, 2, "and(a, b): 1 if a and b are both not 0, else 0"),
    spec("or", "logic", 0, 2, "or(a, b): 1 if a or b is not 0, else 0"),
    spec("xor", "logic", 0, 2, "xor(a, b): 1 if exactly one of a and b is not 0, else 0"),
    spec("truth", "logic", 0, 1, "truth(a): 1 if a is not 0, else 0"),
    // Layer 1.
    spec("clamp3", "order", 1, 3, "clamp3(x, lo, hi): min2(max2(x, lo), hi)"),
    spec("median3", "order", 1, 3, "median3(a, b, c): the middle one of a, b and c"),
    spec("max3", "order", 1, 3, "max3(a, b, c): the largest of a, b and c"),
    spec("min3", "order", 1, 3, "min3(a, b, c): the smallest of a, b and c"),
    spec("between", "order", 1, 3, "between(x, lo, hi): 1 if lo <= x <= hi, else 0"),
    spec("cmp", "order", 1, 2, "cmp(a, b): -1 if a < b, 0 if a = b, 1 if a > b"),
    spec("dist", "order", 1, 2, "dist(a, b): abs(a - b)"),
    spec("nearer", "order", 1, 3, "nearer(x, p, q): p if dist(x, p) <= dist(x, q), else q"),
    spec("spread3", "order", 1, 3, "spread3(a, b, c): max3(a, b, c) - min3(a, b, c)"),
    spec("outside", "order", 1, 3, "outside(x, lo, hi): 1 if x < lo or x > hi, else 0"),
    spec("sorted3", "order", 1, 3, "sorted3(a, b, c): 1 if a <= b <= c, else 0"),
    spec(
        "rank",
        "order",
        1,
        2,
        "rank(a, b): a value that says how a compares with b, for a and b from -1000 to 1000. Open contract rank: \
         the library chooses how the value says it, and isless, issame, ismore and flip read it that way",
    ),
    spec("toward", "step", 1, 2, "toward(a, t): a + sign(t - a), one step from a toward t"),
    spec("away", "step", 1, 2, "away(a, t): a - sign(t - a), one step from a away from t"),
    spec("stepn", "step", 1, 3, "stepn(a, s, n): a + s * n"),
    spec("relu", "step", 1, 1, "relu(a): max2(a, 0)"),
    spec("deadzone", "step", 1, 2, "deadzone(a, r): 0 if abs(a) <= r, else a"),
    spec("grow", "step", 1, 1, "grow(a): double(a) + 1"),
    spec("shrink", "step", 1, 1, "shrink(a): abs(a) - 1"),
    spec(
        "flip",
        "step",
        1,
        1,
        "flip(t): for t = rank(a, b), a value that isless, issame and ismore read as they read rank(b, a) \
         (open contract rank)",
    ),
    spec("isless", "step", 1, 1, "isless(t): for t = rank(a, b), 1 if a < b, else 0 (open contract rank)"),
    spec("issame", "step", 1, 1, "issame(t): for t = rank(a, b), 1 if a = b, else 0 (open contract rank)"),
    spec("ismore", "step", 1, 1, "ismore(t): for t = rank(a, b), 1 if a > b, else 0 (open contract rank)"),
    // Layer 2.
    spec("energy", "arith", 2, 2, "energy(a, b): sq(a) + dist(a, b)"),
    spec("mulsign", "arith", 2, 2, "mulsign(a, b): abs(a) * cmp(b, 0)"),
    spec("clampsq", "arith", 2, 3, "clampsq(a, lo, hi): clamp3(sq(a), lo, hi)"),
    spec("path3", "arith", 2, 3, "path3(a, b, c): 0 if eq(a, c) = 1, else dist(a, b) + dist(b, c)"),
    spec("pull", "arith", 2, 3, "pull(a, t, m): clamp3(toward(a, t), opp(m), m)"),
    spec("band", "arith", 2, 3, "band(x, lo, hi): between(x, lo, hi) * sign(x)"),
    spec("clip", "arith", 2, 2, "clip(a, m): clamp3(a, 0, max2(m, 0))"),
    spec("relax", "arith", 2, 2, "relax(a, b): max2(relu(a), relu(b))"),
    spec("gap", "arith", 2, 3, "gap(a, b, c): spread3(a, b, c) * sign(b)"),
    spec("rise", "arith", 2, 2, "rise(a, b): relu(diff(b, a))"),
    spec(
        "cut",
        "arith",
        2,
        2,
        "cut(a, b): a - b if a >= b, else none, for a and b from -1000 to 1000. Open contract none: none is one \
         value of the library's choice outside 0 to 4000, the same everywhere, that isnone, either and plus \
         recognise",
    ),
    spec("pos", "arith", 2, 1, "pos(a): a if a > 0, else none, for a from -1000 to 1000 (open contract none)"),
    spec("fit", "range", 2, 3, "fit(x, lo, hi): clamp3(x, min2(lo, hi), max2(lo, hi))"),
    spec("inband", "range", 2, 3, "inband(x, lo, hi): and(between(x, lo, hi), neq(lo, hi))"),
    spec("pinned", "range", 2, 3, "pinned(x, lo, hi): neq(clamp3(x, lo, hi), x)"),
    spec("span", "range", 2, 2, "span(lo, hi): inc(dist(lo, hi))"),
    spec("within", "range", 2, 3, "within(x, p, r): le(dist(x, p), r)"),
    spec("snap", "range", 2, 3, "snap(x, lo, hi): nearer(relu(x), lo, hi)"),
    spec("count3", "range", 2, 3, "count3(a, b, c): add3(truth(a), truth(b), truth(c))"),
    spec("safe", "range", 2, 3, "safe(x, lo, hi): clamp3(x, lo, hi) * ge(hi, lo)"),
    spec("trim", "range", 2, 2, "trim(a, m): sign(a) * clamp3(abs(a), 0, m)"),
    spec("isnone", "range", 2, 1, "isnone(x): 1 if x is none, else 0 (open contract none)"),
    spec("either", "range", 2, 2, "either(x, d): d if x is none, else x (open contract none)"),
    spec("plus", "range", 2, 2, "plus(x, y): none if x or y is none, else x + y (open contract none)"),
    // Layer 3.
    spec("spot", "shape", 3, 3, "spot(a, b, r): within(a, b, r) * energy(a, b)"),
    spec("cap", "shape", 3, 3, "cap(a, lo, hi): clampsq(a, lo, hi) - clamp3(a, lo, hi)"),
    spec("tilt", "shape", 3, 2, "tilt(a, b): mulsign(a, b) + cmp(a, b)"),
    spec("route", "shape", 3, 3, "route(a, b, c): path3(a, b, c) + spread3(a, b, c)"),
    spec("box", "shape", 3, 3, "box(x, lo, hi): fit(x, lo, hi) - clamp3(x, lo, hi)"),
    spec("lift", "shape", 3, 2, "lift(a, b): relax(a, b) + span(a, b)"),
    spec("press", "shape", 3, 3, "press(x, t, m): trim(pull(x, t, m), m)"),
    spec("weigh", "shape", 3, 3, "weigh(a, b, c): gap(a, b, c) + count3(a, b, c)"),
    spec("hold", "shape", 3, 3, "hold(x, lo, hi): clamp3(snap(x, lo, hi), lo, hi)"),
    spec("flag", "shape", 3, 3, "flag(x, lo, hi): or(pinned(x, lo, hi), band(x, lo, hi))"),
    // Layer 4.
    spec("total", "score", 4, 2, "total(a, b): lift(a, b) + energy(a, b)"),
    spec("grade", "score", 4, 3, "grade(a, lo, hi): clamp3(route(a, lo, hi), lo, hi)"),
    spec("judge", "score", 4, 3, "judge(a, b, c): max2(weigh(a, b, c), flag(a, b, c))"),
    spec("rate", "score", 4, 3, "rate(a, b, r): spot(a, b, r) + dist(a, b)"),
    spec("tune", "score", 4, 3, "tune(x, t, m): press(x, t, m) + clamp3(t, opp(m), m)"),
    spec("settle", "score", 4, 3, "settle(x, lo, hi): hold(x, lo, hi) + pinned(x, lo, hi)"),
    spec("peak", "score", 4, 3, "peak(a, b, c): max3(cap(a, b, c), box(a, b, c), c)"),
    spec("mix", "score", 4, 2, "mix(a, b): tilt(a, b) + clip(a, b)"),
    spec("index", "score", 4, 3, "index(x, lo, hi): box(x, lo, hi) + safe(x, lo, hi)"),
    spec("verdict", "score", 4, 3, "verdict(x, p, r): and(flag(x, p, r), within(x, p, r))"),
];

/// The specs each spec's description is written in terms of.
pub(super) fn defined_by(name: &str) -> &'static [&'static str] {
    match name {
        "clamp3" => &["max2", "min2"],
        "dist" | "deadzone" | "shrink" => &["abs"],
        "nearer" => &["dist"],
        "spread3" => &["max3", "min3"],
        "toward" | "away" => &["sign"],
        "relu" => &["max2"],
        "grow" => &["double"],
        "energy" => &["sq", "dist"],
        "mulsign" => &["abs", "cmp"],
        "clampsq" => &["sq", "clamp3"],
        "path3" => &["dist", "eq"],
        "pull" => &["toward", "opp", "clamp3"],
        "band" => &["between", "sign"],
        "clip" => &["clamp3", "max2"],
        "relax" => &["relu", "max2"],
        "gap" => &["spread3", "sign"],
        "rise" => &["relu", "diff"],
        "fit" => &["clamp3", "min2", "max2"],
        "inband" => &["between", "neq", "and"],
        "pinned" => &["clamp3", "neq"],
        "span" => &["inc", "dist"],
        "within" => &["dist", "le"],
        "snap" => &["relu", "nearer"],
        "count3" => &["add3", "truth"],
        "safe" => &["clamp3", "ge"],
        "trim" => &["sign", "abs", "clamp3"],
        "spot" => &["within", "energy"],
        "cap" => &["clampsq", "clamp3"],
        "tilt" => &["mulsign", "cmp"],
        "route" => &["path3", "spread3"],
        "box" => &["fit", "clamp3"],
        "lift" => &["relax", "span"],
        "press" => &["trim", "pull"],
        "weigh" => &["gap", "count3"],
        "hold" => &["snap", "clamp3"],
        "flag" => &["pinned", "band", "or"],
        "total" => &["lift", "energy"],
        "grade" => &["route", "clamp3"],
        "judge" => &["weigh", "flag", "max2"],
        "rate" => &["spot", "dist"],
        "tune" => &["press", "clamp3", "opp"],
        "settle" => &["hold", "pinned"],
        "peak" => &["cap", "box", "max3"],
        "mix" => &["tilt", "clip"],
        "index" => &["box", "safe"],
        "verdict" => &["flag", "within", "and"],
        _ => &[],
    }
}

/// The reference implementation of `name` under `version`, if it is a spec
/// with that many inputs and not an open contract's member; `r` evaluates a
/// callee's reference.
pub(super) fn reference(name: &str, args: &[i64], _version: Version, r: &dyn Fn(&str, &[i64]) -> i64) -> Option<i64> {
    let bit = |b: bool| i64::from(b);
    let a = || args[0];
    let b = || args[1];
    let c = || args[2];
    Some(match (name, args.len()) {
        ("abs", 1) => a().wrapping_abs(),
        ("sign", 1) => a().signum(),
        ("max2", 2) => a().max(b()),
        ("min2", 2) => a().min(b()),
        ("inc", 1) => a().wrapping_add(1),
        ("dec", 1) => a().wrapping_sub(1),
        ("sq", 1) => a().wrapping_mul(a()),
        ("double", 1) => a().wrapping_add(a()),
        ("triple", 1) => a().wrapping_mul(3),
        ("opp", 1) => a().wrapping_neg(),
        ("diff", 2) => a().wrapping_sub(b()),
        ("add3", 3) => a().wrapping_add(b()).wrapping_add(c()),
        ("iszero", 1) => bit(a() == 0),
        ("eq", 2) => bit(a() == b()),
        ("neq", 2) => bit(a() != b()),
        ("lt", 2) => bit(a() < b()),
        ("le", 2) => bit(a() <= b()),
        ("gt", 2) => bit(a() > b()),
        ("ge", 2) => bit(a() >= b()),
        ("and", 2) => bit(a() != 0 && b() != 0),
        ("or", 2) => bit(a() != 0 || b() != 0),
        ("xor", 2) => bit((a() != 0) != (b() != 0)),
        ("truth", 1) => bit(a() != 0),
        ("clamp3", 3) => r("min2", &[r("max2", &[a(), b()]), c()]),
        ("median3", 3) => {
            let mut v = [a(), b(), c()];
            v.sort_unstable();
            v[1]
        }
        ("max3", 3) => a().max(b()).max(c()),
        ("min3", 3) => a().min(b()).min(c()),
        ("between", 3) => bit(b() <= a() && a() <= c()),
        ("cmp", 2) => match a().cmp(&b()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
        ("dist", 2) => r("abs", &[a().wrapping_sub(b())]),
        ("nearer", 3) => {
            if r("dist", &[a(), b()]) <= r("dist", &[a(), c()]) {
                b()
            } else {
                c()
            }
        }
        ("spread3", 3) => r("max3", args).wrapping_sub(r("min3", args)),
        ("outside", 3) => bit(a() < b() || a() > c()),
        ("sorted3", 3) => bit(a() <= b() && b() <= c()),
        ("toward", 2) => a().wrapping_add(r("sign", &[b().wrapping_sub(a())])),
        ("away", 2) => a().wrapping_sub(r("sign", &[b().wrapping_sub(a())])),
        ("stepn", 3) => a().wrapping_add(b().wrapping_mul(c())),
        ("relu", 1) => r("max2", &[a(), 0]),
        ("deadzone", 2) => {
            if r("abs", &[a()]) <= b() {
                0
            } else {
                a()
            }
        }
        ("grow", 1) => r("double", &[a()]).wrapping_add(1),
        ("shrink", 1) => r("abs", &[a()]).wrapping_sub(1),
        ("energy", 2) => r("sq", &[a()]).wrapping_add(r("dist", args)),
        ("mulsign", 2) => r("abs", &[a()]).wrapping_mul(r("cmp", &[b(), 0])),
        ("clampsq", 3) => r("clamp3", &[r("sq", &[a()]), b(), c()]),
        ("path3", 3) => {
            if r("eq", &[a(), c()]) == 1 {
                0
            } else {
                r("dist", &[a(), b()]).wrapping_add(r("dist", &[b(), c()]))
            }
        }
        ("pull", 3) => r("clamp3", &[r("toward", &[a(), b()]), r("opp", &[c()]), c()]),
        ("band", 3) => r("between", args).wrapping_mul(r("sign", &[a()])),
        ("clip", 2) => r("clamp3", &[a(), 0, r("max2", &[b(), 0])]),
        ("relax", 2) => r("max2", &[r("relu", &[a()]), r("relu", &[b()])]),
        ("gap", 3) => r("spread3", args).wrapping_mul(r("sign", &[b()])),
        ("rise", 2) => r("relu", &[r("diff", &[b(), a()])]),
        ("fit", 3) => r("clamp3", &[a(), r("min2", &[b(), c()]), r("max2", &[b(), c()])]),
        ("inband", 3) => r("and", &[r("between", args), r("neq", &[b(), c()])]),
        ("pinned", 3) => r("neq", &[r("clamp3", args), a()]),
        ("span", 2) => r("inc", &[r("dist", args)]),
        ("within", 3) => r("le", &[r("dist", &[a(), b()]), c()]),
        ("snap", 3) => r("nearer", &[r("relu", &[a()]), b(), c()]),
        ("count3", 3) => r("add3", &[r("truth", &[a()]), r("truth", &[b()]), r("truth", &[c()])]),
        ("safe", 3) => r("clamp3", args).wrapping_mul(r("ge", &[c(), b()])),
        ("trim", 2) => r("sign", &[a()]).wrapping_mul(r("clamp3", &[r("abs", &[a()]), 0, b()])),
        ("spot", 3) => r("within", args).wrapping_mul(r("energy", &[a(), b()])),
        ("cap", 3) => r("clampsq", args).wrapping_sub(r("clamp3", args)),
        ("tilt", 2) => r("mulsign", args).wrapping_add(r("cmp", args)),
        ("route", 3) => r("path3", args).wrapping_add(r("spread3", args)),
        ("box", 3) => r("fit", args).wrapping_sub(r("clamp3", args)),
        ("lift", 2) => r("relax", args).wrapping_add(r("span", args)),
        ("press", 3) => r("trim", &[r("pull", args), c()]),
        ("weigh", 3) => r("gap", args).wrapping_add(r("count3", args)),
        ("hold", 3) => r("clamp3", &[r("snap", args), b(), c()]),
        ("flag", 3) => r("or", &[r("pinned", args), r("band", args)]),
        ("total", 2) => r("lift", args).wrapping_add(r("energy", args)),
        ("grade", 3) => r("clamp3", &[r("route", args), b(), c()]),
        ("judge", 3) => r("max2", &[r("weigh", args), r("flag", args)]),
        ("rate", 3) => r("spot", args).wrapping_add(r("dist", &[a(), b()])),
        ("tune", 3) => r("press", args).wrapping_add(r("clamp3", &[b(), r("opp", &[c()]), c()])),
        ("settle", 3) => r("hold", args).wrapping_add(r("pinned", args)),
        ("peak", 3) => r("max3", &[r("cap", args), r("box", args), c()]),
        ("mix", 2) => r("tilt", args).wrapping_add(r("clip", args)),
        ("index", 3) => r("box", args).wrapping_add(r("safe", args)),
        ("verdict", 3) => r("and", &[r("flag", args), r("within", args)]),
        _ => return None,
    })
}

/// A property whose answer is 1 or 0.
fn holds(expr: String, want: bool, got: Result<i64, String>) -> Probe {
    Probe { expr, got, want: i64::from(want) }
}

/// `reader(rank(a, b))`, or `reader(flip(rank(a, b)))`, with the library's functions.
fn read_rank(call: &mut Caller<'_>, x: &[i64], flipped: bool, reader: &str) -> Probe {
    let inner = format!("rank({}, {})", x[0], x[1]);
    let expr = if flipped { format!("{reader}(flip({inner}))") } else { format!("{reader}({inner})") };
    let got = (|| {
        let t = call("rank", &x[..2])?;
        let t = if flipped { call("flip", &[t])? } else { t };
        call(reader, &[t])
    })();
    // Flipped, rank(a, b) reads as rank(b, a).
    let (a, b) = if flipped { (x[1], x[0]) } else { (x[0], x[1]) };
    let want = match reader {
        "isless" => a < b,
        "issame" => a == b,
        _ => a > b,
    };
    holds(expr, want, got)
}

/// What `either(x, d)` must give, where `x` is none unless `valid`.
fn either_want(valid: bool, value: i64, d: i64) -> i64 {
    if valid { value } else { d }
}

pub(super) const CONTRACTS: &[Contract] = &[
    Contract {
        name: "rank",
        members: &["rank", "flip", "isless", "issame", "ismore"],
        properties: &[
            Property { arity: 2, calls: &["rank", "isless"], eval: |call, x| read_rank(call, x, false, "isless") },
            Property { arity: 2, calls: &["rank", "issame"], eval: |call, x| read_rank(call, x, false, "issame") },
            Property { arity: 2, calls: &["rank", "ismore"], eval: |call, x| read_rank(call, x, false, "ismore") },
            Property {
                arity: 2,
                calls: &["rank", "flip", "isless"],
                eval: |call, x| read_rank(call, x, true, "isless"),
            },
            Property {
                arity: 2,
                calls: &["rank", "flip", "issame"],
                eval: |call, x| read_rank(call, x, true, "issame"),
            },
            Property {
                arity: 2,
                calls: &["rank", "flip", "ismore"],
                eval: |call, x| read_rank(call, x, true, "ismore"),
            },
        ],
    },
    Contract {
        name: "none",
        members: &["cut", "pos", "isnone", "either", "plus"],
        properties: &[
            // One none, the same everywhere and outside 0 to 4000: both producers' nones are one value.
            Property {
                arity: 2,
                calls: &["cut", "pos"],
                eval: |call, x| {
                    let (lo, hi) = if x[1] == 0 { (-1, 0) } else { (-x[1].abs(), x[1].abs()) };
                    let p = -x[0].abs();
                    let got = (|| {
                        let (u, v) = (call("cut", &[lo, hi])?, call("pos", &[p])?);
                        Ok(i64::from(u == v && !(0..=4000).contains(&u)))
                    })();
                    let expr = format!("cut({lo}, {hi}) and pos({p}) being one none outside 0 to 4000");
                    holds(expr, true, got)
                },
            },
            Property {
                arity: 2,
                calls: &["cut", "isnone"],
                eval: |call, x| {
                    let (a, b) = (x[0], x[1]);
                    let got = call("cut", &[a, b]).and_then(|v| call("isnone", &[v]));
                    holds(format!("isnone(cut({a}, {b}))"), a < b, got)
                },
            },
            Property {
                arity: 3,
                calls: &["cut", "either"],
                eval: |call, x| {
                    let (a, b, d) = (x[0], x[1], x[2]);
                    let got = call("cut", &[a, b]).and_then(|v| call("either", &[v, d]));
                    Probe { expr: format!("either(cut({a}, {b}), {d})"), got, want: either_want(a >= b, a - b, d) }
                },
            },
            Property {
                arity: 1,
                calls: &["pos", "isnone"],
                eval: |call, x| {
                    let a = x[0];
                    let got = call("pos", &[a]).and_then(|v| call("isnone", &[v]));
                    holds(format!("isnone(pos({a}))"), a <= 0, got)
                },
            },
            Property {
                arity: 2,
                calls: &["pos", "either"],
                eval: |call, x| {
                    let (a, d) = (x[0], x[1]);
                    let got = call("pos", &[a]).and_then(|v| call("either", &[v, d]));
                    Probe { expr: format!("either(pos({a}), {d})"), got, want: either_want(a > 0, a, d) }
                },
            },
            Property {
                arity: 2,
                calls: &["pos", "cut", "plus", "isnone"],
                eval: |call, x| {
                    let (a, b) = (x[0], x[1]);
                    let got = (|| {
                        let (p, q) = (call("pos", &[a])?, call("cut", &[a, b])?);
                        let sum = call("plus", &[p, q])?;
                        call("isnone", &[sum])
                    })();
                    holds(format!("isnone(plus(pos({a}), cut({a}, {b})))"), !(a > 0 && a >= b), got)
                },
            },
            Property {
                arity: 3,
                calls: &["cut", "pos", "plus", "either"],
                eval: |call, x| {
                    let (a, b, d) = (x[0], x[1], x[2]);
                    let got = (|| {
                        let (p, q) = (call("cut", &[a, b])?, call("pos", &[b])?);
                        let sum = call("plus", &[p, q])?;
                        call("either", &[sum, d])
                    })();
                    let want = either_want(a >= b && b > 0, a, d);
                    Probe { expr: format!("either(plus(cut({a}, {b}), pos({b})), {d})"), got, want }
                },
            },
        ],
    },
];
