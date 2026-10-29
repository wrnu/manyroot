//! Phase 2's known programs: two correct and two subtly wrong ones per spec.
//!
//! The open contracts' first correct programs share one convention: `rank(a, b)` is `a - b`, read by its
//! sign, and none is -1. [`OTHER_CHOICE`] is another convention for both, which a test shows is graded
//! right too, and a library mixing the two wrong.
//!
//! The last wrong program of a spec is usually its first correct one for `a` below 7 and one more from 7 on,
//! so it passes every public case (inputs from -6 to 6) and fails hidden ones. A member of an open contract
//! that reads another member's value is wrong only where that value is beyond what public inputs give.

use super::{Candidates, Revision, Update, c};
use crate::task::Change;

pub(super) const CANDIDATES: &[Candidates] = &[
    c("abs", &["a 0 < a neg a ?", "a 0 > a a neg ?"], &["a", "a 7 < a 0 < a neg a ? a 0 < a neg a ? 1 + ?"]),
    c(
        "sign",
        &["a 0 > 1 a 0 < -1 0 ? ?", "a 0 < -1 a 0 = 0 1 ? ?"],
        &["a 0 > 1 -1 ?", "a 7 < a 0 > 1 a 0 < -1 0 ? ? a 0 > 1 a 0 < -1 0 ? ? 1 + ?"],
    ),
    c("max2", &["a b > a b ?", "a b < b a ?"], &["a b < a b ?", "a 7 < a b > a b ? a b > a b ? 1 + ?"]),
    c("min2", &["a b < a b ?", "a b > b a ?"], &["a b > a b ?", "a 7 < a b < a b ? a b < a b ? 1 + ?"]),
    c("inc", &["a 1 +", "1 a +"], &["a 1 -", "a 7 < a 1 + a 1 + 1 + ?"]),
    c("dec", &["a 1 -", "a -1 +"], &["1 a -", "a 7 < a 1 - a 1 - 1 + ?"]),
    c("sq", &["a a *", "a dup *"], &["a a +", "a 7 < a a * a a * 1 + ?"]),
    c("double", &["a a +", "a 2 *"], &["a a *", "a 7 < a a + a a + 1 + ?"]),
    c("triple", &["a 3 *", "a a + a +"], &["a 3 +", "a 7 < a 3 * a 3 * 1 + ?"]),
    c("opp", &["a neg", "0 a -"], &["a neg 1 +", "a 7 < a neg a neg 1 + ?"]),
    c("diff", &["a b -", "b neg a +"], &["b a -", "a 7 < a b - a b - 1 + ?"]),
    c("add3", &["a b + c +", "c b a + +"], &["a b + c -", "a 7 < a b + c + a b + c + 1 + ?"]),
    c("iszero", &["a 0 =", "0 a ="], &["a 0 < 0 1 ?", "a 7 < a 0 = a 0 = 1 + ?"]),
    c("eq", &["a b =", "b a ="], &["a b -", "a 7 < a b = a b = 1 + ?"]),
    c("neq", &["a b = 0 1 ?", "1 a b = -"], &["a b -", "a 7 < a b = 0 1 ? a b = 0 1 ? 1 + ?"]),
    c("lt", &["a b <", "b a >"], &["a b >", "a 7 < a b < a b < 1 + ?"]),
    c("le", &["a b > 0 1 ?", "1 a b > -"], &["a b <", "a 7 < a b > 0 1 ? a b > 0 1 ? 1 + ?"]),
    c("gt", &["a b >", "b a <"], &["a b <", "a 7 < a b > a b > 1 + ?"]),
    c("ge", &["a b < 0 1 ?", "1 a b < -"], &["a b >", "a 7 < a b < 0 1 ? a b < 0 1 ? 1 + ?"]),
    c("and", &["a b 1 0 ? 0 ?", "a 0 = b 0 = + 0 ="], &["a b *", "a 7 < a b 1 0 ? 0 ? a b 1 0 ? 0 ? 1 + ?"]),
    c("or", &["a 1 b 1 0 ? ?", "a 0 = b 0 = * 0 ="], &["a b +", "a 7 < a 1 b 1 0 ? ? a 1 b 1 0 ? ? 1 + ?"]),
    c(
        "xor",
        &["a 0 = b 0 = = 0 1 ?", "a 0 = b 0 = - dup *"],
        &["a b = 0 1 ?", "a 7 < a 0 = b 0 = = 0 1 ? a 0 = b 0 = = 0 1 ? 1 + ?"],
    ),
    c("truth", &["a 1 0 ?", "a 0 = 0 1 ?"], &["a 0 >", "a 7 < a 1 0 ? a 1 0 ? 1 + ?"]),
    c(
        "clamp3",
        &["a b max2@ c min2@", "a b max2@ dup c < swap c ?"],
        &["a b min2@ c max2@", "a 7 < a b max2@ c min2@ a b max2@ c min2@ 1 + ?"],
    ),
    c(
        "median3",
        &["a b min2@ a b max2@ c min2@ max2@", "a b max2@ a c max2@ min2@ b c max2@ min2@"],
        &["a b max2@ c min2@", "a 7 < a b min2@ a b max2@ c min2@ max2@ a b min2@ a b max2@ c min2@ max2@ 1 + ?"],
    ),
    c(
        "max3",
        &["a b max2@ c max2@", "a b c max2@ max2@"],
        &["a b max2@", "a 7 < a b max2@ c max2@ a b max2@ c max2@ 1 + ?"],
    ),
    c(
        "min3",
        &["a b min2@ c min2@", "a b c min2@ min2@"],
        &["a b min2@", "a 7 < a b min2@ c min2@ a b min2@ c min2@ 1 + ?"],
    ),
    c(
        "between",
        &["a b < 0 a c > 0 1 ? ?", "a b < a c > + 0 ="],
        &["a b > a c < *", "a 7 < a b < 0 a c > 0 1 ? ? a b < 0 a c > 0 1 ? ? 1 + ?"],
    ),
    c(
        "cmp",
        &["a b < -1 a b > 1 0 ? ?", "a b > a b < -"],
        &["b a - sign@", "a 7 < a b < -1 a b > 1 0 ? ? a b < -1 a b > 1 0 ? ? 1 + ?"],
    ),
    c("dist", &["a b - abs@", "b a - abs@"], &["a b -", "a 7 < a b - abs@ a b - abs@ 1 + ?"]),
    c(
        "nearer",
        &["a b dist@ a c dist@ > c b ?", "a c dist@ a b dist@ < c b ?"],
        &["a b dist@ a c dist@ < b c ?", "a 7 < a b dist@ a c dist@ > c b ? a b dist@ a c dist@ > c b ? 1 + ?"],
    ),
    c(
        "spread3",
        &["a b c max3@ a b c min3@ -", "a b max2@ c max2@ a b min2@ c min2@ -"],
        &["a b c max3@ a b c median3@ -", "a 7 < a b c max3@ a b c min3@ - a b c max3@ a b c min3@ - 1 + ?"],
    ),
    c(
        "outside",
        &["a b < 1 a c > 1 0 ? ?", "a b c between@ 0 ="],
        &["a b < a c > *", "a 7 < a b < 1 a c > 1 0 ? ? a b < 1 a c > 1 0 ? ? 1 + ?"],
    ),
    c(
        "sorted3",
        &["a b > 0 b c > 0 1 ? ?", "a b > b c > + 0 ="],
        &["a c > 0 1 ?", "a 7 < a b > 0 b c > 0 1 ? ? a b > 0 b c > 0 1 ? ? 1 + ?"],
    ),
    c("rank", &["a b -", "b neg a +"], &["b a -", "a 7 < a b - a b - 1 + ?"]),
    c(
        "toward",
        &["b a - sign@ a +", "a b a - sign@ +"],
        &["a b - sign@ a +", "a 7 < b a - sign@ a + b a - sign@ a + 1 + ?"],
    ),
    c(
        "away",
        &["a b a - sign@ -", "0 b a - sign@ - a +"],
        &["a b a - sign@ +", "a 7 < a b a - sign@ - a b a - sign@ - 1 + ?"],
    ),
    c("stepn", &["b c * a +", "a c b * +"], &["a b + c *", "a 7 < b c * a + b c * a + 1 + ?"]),
    c("relu", &["a 0 max2@", "a 0 < 0 a ?"], &["a abs@", "a 7 < a 0 max2@ a 0 max2@ 1 + ?"]),
    c(
        "deadzone",
        &["a abs@ b > a 0 ?", "b a abs@ < a 0 ?"],
        &["a abs@ b < a 0 ?", "a 7 < a abs@ b > a 0 ? a abs@ b > a 0 ? 1 + ?"],
    ),
    c("grow", &["a double@ 1 +", "a a + 1 +"], &["a double@ 1 -", "a 7 < a double@ 1 + a double@ 1 + 1 + ?"]),
    c("shrink", &["a abs@ 1 -", "a abs@ dec@"], &["a 1 - abs@", "a 7 < a abs@ 1 - a abs@ 1 - 1 + ?"]),
    c("flip", &["a neg", "0 a -"], &["a", "a 12 > a a neg ?"]),
    c("isless", &["a 0 <", "0 a >"], &["a 0 >", "a -12 < 0 a 0 < ?"]),
    c("issame", &["a 0 =", "0 a ="], &["a 0 <", "a 12 > 1 a 0 = ?"]),
    c("ismore", &["a 0 >", "0 a <"], &["a 0 <", "a 12 > 0 a 0 > ?"]),
    c(
        "energy",
        &["a sq@ a b dist@ +", "a b dist@ a sq@ +"],
        &["a sq@ a b dist@ -", "a 7 < a sq@ a b dist@ + a sq@ a b dist@ + 1 + ?"],
    ),
    c(
        "mulsign",
        &["a abs@ b 0 cmp@ *", "b 0 cmp@ a abs@ *"],
        &["a b 0 cmp@ *", "a 7 < a abs@ b 0 cmp@ * a abs@ b 0 cmp@ * 1 + ?"],
    ),
    c(
        "clampsq",
        &["a sq@ b c clamp3@", "a sq@ b max2@ c min2@"],
        &["a b c clamp3@ sq@", "a 7 < a sq@ b c clamp3@ a sq@ b c clamp3@ 1 + ?"],
    ),
    c(
        "path3",
        &["a c eq@ 0 a b dist@ b c dist@ + ?", "a c = 0 b c dist@ a b dist@ + ?"],
        &["a b dist@ b c dist@ +", "a 7 < a c eq@ 0 a b dist@ b c dist@ + ? a c eq@ 0 a b dist@ b c dist@ + ? 1 + ?"],
    ),
    c(
        "pull",
        &["a b toward@ c opp@ c clamp3@", "a b toward@ c neg c clamp3@"],
        &["a b toward@ c c opp@ clamp3@", "a 7 < a b toward@ c opp@ c clamp3@ a b toward@ c opp@ c clamp3@ 1 + ?"],
    ),
    c(
        "band",
        &["a b c between@ a sign@ *", "a sign@ a b c between@ *"],
        &["a b c between@ a *", "a 7 < a b c between@ a sign@ * a b c between@ a sign@ * 1 + ?"],
    ),
    c(
        "clip",
        &["a 0 b 0 max2@ clamp3@", "a 0 max2@ b 0 max2@ min2@"],
        &["a 0 b clamp3@", "a 7 < a 0 b 0 max2@ clamp3@ a 0 b 0 max2@ clamp3@ 1 + ?"],
    ),
    c(
        "relax",
        &["a relu@ b relu@ max2@", "a b max2@ relu@"],
        &["a b min2@ relu@", "a 7 < a relu@ b relu@ max2@ a relu@ b relu@ max2@ 1 + ?"],
    ),
    c(
        "gap",
        &["a b c spread3@ b sign@ *", "b sign@ a b c spread3@ *"],
        &["a b c spread3@ b *", "a 7 < a b c spread3@ b sign@ * a b c spread3@ b sign@ * 1 + ?"],
    ),
    c(
        "rise",
        &["b a diff@ relu@", "b a - 0 max2@"],
        &["a b diff@ relu@", "a 7 < b a diff@ relu@ b a diff@ relu@ 1 + ?"],
    ),
    c("cut", &["a b < -1 a b - ?", "b a > -1 a b - ?"], &["a b -", "a 7 < a b < -1 a b - ? a b < -1 a b - ? 1 + ?"]),
    c("pos", &["a 0 > a -1 ?", "0 a < a -1 ?"], &["a 0 < -1 a ?", "a 7 < a 0 > a -1 ? a 0 > a -1 ? 1 + ?"]),
    c(
        "fit",
        &["a b c min2@ b c max2@ clamp3@", "a b c min2@ max2@ b c max2@ min2@"],
        &["a b c clamp3@", "a 7 < a b c min2@ b c max2@ clamp3@ a b c min2@ b c max2@ clamp3@ 1 + ?"],
    ),
    c(
        "inband",
        &["a b c between@ b c neq@ and@", "b c neq@ a b c between@ and@"],
        &["a b c between@", "a 7 < a b c between@ b c neq@ and@ a b c between@ b c neq@ and@ 1 + ?"],
    ),
    c(
        "pinned",
        &["a b c clamp3@ a neq@", "a a b c clamp3@ = 0 1 ?"],
        &["a b c clamp3@ a eq@", "a 7 < a b c clamp3@ a neq@ a b c clamp3@ a neq@ 1 + ?"],
    ),
    c("span", &["a b dist@ inc@", "a b dist@ 1 +"], &["a b dist@", "a 7 < a b dist@ inc@ a b dist@ inc@ 1 + ?"]),
    c(
        "within",
        &["a b dist@ c le@", "c a b dist@ ge@"],
        &["a b dist@ c lt@", "a 7 < a b dist@ c le@ a b dist@ c le@ 1 + ?"],
    ),
    c(
        "snap",
        &["a relu@ b c nearer@", "a 0 max2@ b c nearer@"],
        &["a b c nearer@", "a 7 < a relu@ b c nearer@ a relu@ b c nearer@ 1 + ?"],
    ),
    c(
        "count3",
        &["a truth@ b truth@ c truth@ add3@", "a truth@ b truth@ + c truth@ +"],
        &["a b c add3@ truth@", "a 7 < a truth@ b truth@ c truth@ add3@ a truth@ b truth@ c truth@ add3@ 1 + ?"],
    ),
    c(
        "safe",
        &["a b c clamp3@ c b ge@ *", "c b < 0 a b c clamp3@ ?"],
        &["a b c clamp3@", "a 7 < a b c clamp3@ c b ge@ * a b c clamp3@ c b ge@ * 1 + ?"],
    ),
    c(
        "trim",
        &["a sign@ a abs@ 0 b clamp3@ *", "a abs@ 0 b clamp3@ a sign@ *"],
        &["a 0 b clamp3@", "a 7 < a sign@ a abs@ 0 b clamp3@ * a sign@ a abs@ 0 b clamp3@ * 1 + ?"],
    ),
    c("isnone", &["a -1 =", "-1 a ="], &["a 0 =", "a 18 > 1 a -1 = ?"]),
    c("either", &["a -1 = b a ?", "-1 a = b a ?"], &["a -1 = a b ?", "a 12 > b a -1 = b a ? ?"]),
    c(
        "plus",
        &["a -1 = -1 b -1 = -1 a b + ? ?", "a -1 = b -1 = + 0 > -1 a b + ?"],
        &["a b +", "a 12 > a -1 = -1 b -1 = -1 a b + ? ? 1 + a -1 = -1 b -1 = -1 a b + ? ? ?"],
    ),
    c(
        "spot",
        &["a b c within@ a b energy@ *", "a b energy@ a b c within@ *"],
        &["a b c within@ a b energy@ +", "a 7 < a b c within@ a b energy@ * a b c within@ a b energy@ * 1 + ?"],
    ),
    c(
        "cap",
        &["a b c clampsq@ a b c clamp3@ -", "a b c clamp3@ neg a b c clampsq@ +"],
        &["a b c clampsq@", "a 7 < a b c clampsq@ a b c clamp3@ - a b c clampsq@ a b c clamp3@ - 1 + ?"],
    ),
    c(
        "tilt",
        &["a b mulsign@ a b cmp@ +", "a b cmp@ a b mulsign@ +"],
        &["a b mulsign@ b a cmp@ +", "a 7 < a b mulsign@ a b cmp@ + a b mulsign@ a b cmp@ + 1 + ?"],
    ),
    c(
        "route",
        &["a b c path3@ a b c spread3@ +", "a b c spread3@ a b c path3@ +"],
        &["a b c path3@ a b c spread3@ -", "a 7 < a b c path3@ a b c spread3@ + a b c path3@ a b c spread3@ + 1 + ?"],
    ),
    c(
        "box",
        &["a b c fit@ a b c clamp3@ -", "a b c clamp3@ neg a b c fit@ +"],
        &["a b c fit@", "a 7 < a b c fit@ a b c clamp3@ - a b c fit@ a b c clamp3@ - 1 + ?"],
    ),
    c(
        "lift",
        &["a b relax@ a b span@ +", "a b span@ a b relax@ +"],
        &["a b relax@ a b span@ -", "a 7 < a b relax@ a b span@ + a b relax@ a b span@ + 1 + ?"],
    ),
    c(
        "press",
        &["a b c pull@ c trim@", "a b c pull@ dup sign@ swap abs@ 0 c clamp3@ *"],
        &["a b c pull@", "a 7 < a b c pull@ c trim@ a b c pull@ c trim@ 1 + ?"],
    ),
    c(
        "weigh",
        &["a b c gap@ a b c count3@ +", "a b c count3@ a b c gap@ +"],
        &["a b c gap@ a b c count3@ -", "a 7 < a b c gap@ a b c count3@ + a b c gap@ a b c count3@ + 1 + ?"],
    ),
    c(
        "hold",
        &["a b c snap@ b c clamp3@", "a b c snap@ b max2@ c min2@"],
        &["a b c snap@", "a 7 < a b c snap@ b c clamp3@ a b c snap@ b c clamp3@ 1 + ?"],
    ),
    c(
        "flag",
        &["a b c pinned@ a b c band@ or@", "a b c band@ a b c pinned@ or@"],
        &["a b c pinned@ a b c band@ and@", "a 7 < a b c pinned@ a b c band@ or@ a b c pinned@ a b c band@ or@ 1 + ?"],
    ),
    c(
        "total",
        &["a b lift@ a b energy@ +", "a b energy@ a b lift@ +"],
        &["a b lift@ a b energy@ -", "a 7 < a b lift@ a b energy@ + a b lift@ a b energy@ + 1 + ?"],
    ),
    c(
        "grade",
        &["a b c route@ b c clamp3@", "a b c route@ b max2@ c min2@"],
        &["a b c route@ c b clamp3@", "a 7 < a b c route@ b c clamp3@ a b c route@ b c clamp3@ 1 + ?"],
    ),
    c(
        "judge",
        &["a b c weigh@ a b c flag@ max2@", "a b c flag@ a b c weigh@ max2@"],
        &[
            "a b c weigh@ a b c flag@ min2@",
            "a 7 < a b c weigh@ a b c flag@ max2@ a b c weigh@ a b c flag@ max2@ 1 + ?",
        ],
    ),
    c(
        "rate",
        &["a b c spot@ a b dist@ +", "a b dist@ a b c spot@ +"],
        &["a b c spot@ a b dist@ -", "a 7 < a b c spot@ a b dist@ + a b c spot@ a b dist@ + 1 + ?"],
    ),
    c(
        "tune",
        &["a b c press@ b c opp@ c clamp3@ +", "b c opp@ c clamp3@ a b c press@ +"],
        &[
            "a b c press@ b c c opp@ clamp3@ +",
            "a 7 < a b c press@ b c opp@ c clamp3@ + a b c press@ b c opp@ c clamp3@ + 1 + ?",
        ],
    ),
    c(
        "settle",
        &["a b c hold@ a b c pinned@ +", "a b c pinned@ a b c hold@ +"],
        &["a b c hold@ a b c pinned@ -", "a 7 < a b c hold@ a b c pinned@ + a b c hold@ a b c pinned@ + 1 + ?"],
    ),
    c(
        "peak",
        &["a b c cap@ a b c box@ c max3@", "a b c cap@ a b c box@ max2@ c max2@"],
        &["a b c cap@ a b c box@ c min3@", "a 7 < a b c cap@ a b c box@ c max3@ a b c cap@ a b c box@ c max3@ 1 + ?"],
    ),
    c(
        "mix",
        &["a b tilt@ a b clip@ +", "a b clip@ a b tilt@ +"],
        &["a b tilt@ a b clip@ -", "a 7 < a b tilt@ a b clip@ + a b tilt@ a b clip@ + 1 + ?"],
    ),
    c(
        "index",
        &["a b c box@ a b c safe@ +", "a b c safe@ a b c box@ +"],
        &["a b c box@ a b c safe@ *", "a 7 < a b c box@ a b c safe@ + a b c box@ a b c safe@ + 1 + ?"],
    ),
    c(
        "verdict",
        &["a b c flag@ a b c within@ and@", "a b c within@ a b c flag@ and@"],
        &["a b c flag@ a b c within@ or@", "a 7 < a b c flag@ a b c within@ and@ a b c flag@ a b c within@ and@ 1 + ?"],
    ),
];

/// Every open contract's members under another convention: `rank` gives 5, 7 or 9 for less, the same or
/// more, and none is 99999.
pub const OTHER_CHOICE: &[(&str, &str)] = &[
    ("rank", "a b < 5 a b > 9 7 ? ?"),
    ("flip", "a 5 = 9 a 9 = 5 7 ? ?"),
    ("isless", "a 5 ="),
    ("issame", "a 7 ="),
    ("ismore", "a 9 ="),
    ("cut", "a b < 99999 a b - ?"),
    ("pos", "a 0 > a 99999 ?"),
    ("isnone", "a 99999 ="),
    ("either", "a 99999 = b a ?"),
    ("plus", "a 99999 = 99999 b 99999 = 99999 a b + ? ?"),
];

/// The programs each change request needs, in the order they apply: a later one replaces an earlier one's
/// program. Both of open contract none's changes move none to -5000, and `cut-signed`'s comes last so its
/// `cut` wins; `rank-wide`'s `rank` says how far apart a and b are up to 3, so `ismore-far`'s `ismore` reads
/// it as it reads `a - b`.
pub(super) const UPDATES: &[Update] = &[
    Update {
        change: Change::ClampOrder,
        programs: &[
            ("clamp3", "c a max2@ b min2@"),
            ("clampsq", "b c a sq@ clamp3@"),
            ("pull", "c opp@ c a b toward@ clamp3@"),
            ("clip", "0 b 0 max2@ a clamp3@"),
            ("fit", "b c min2@ b c max2@ a clamp3@"),
            ("pinned", "b c a clamp3@ a neq@"),
            ("safe", "b c a clamp3@ c b ge@ *"),
            ("trim", "a sign@ 0 b a abs@ clamp3@ *"),
            ("cap", "a b c clampsq@ b c a clamp3@ -"),
            ("box", "a b c fit@ b c a clamp3@ -"),
            ("hold", "b c a b c snap@ clamp3@"),
            ("grade", "b c a b c route@ clamp3@"),
            ("tune", "a b c press@ c opp@ c b clamp3@ +"),
        ],
    },
    Update {
        change: Change::ClampHiFirst,
        programs: &[
            ("clamp3", "c b max2@ a min2@"),
            ("clampsq", "c b a sq@ clamp3@"),
            ("pull", "c c opp@ a b toward@ clamp3@"),
            ("clip", "b 0 max2@ 0 a clamp3@"),
            ("fit", "b c max2@ b c min2@ a clamp3@"),
            ("pinned", "c b a clamp3@ a neq@"),
            ("safe", "c b a clamp3@ c b ge@ *"),
            ("trim", "a sign@ b 0 a abs@ clamp3@ *"),
            ("cap", "a b c clampsq@ c b a clamp3@ -"),
            ("box", "a b c fit@ c b a clamp3@ -"),
            ("hold", "c b a b c snap@ clamp3@"),
            ("grade", "c b a b c route@ clamp3@"),
            ("tune", "a b c press@ c c opp@ b clamp3@ +"),
        ],
    },
    Update { change: Change::SignOfZero, programs: &[("sign", "a 0 < -1 1 ?")] },
    Update { change: Change::BetweenExclusive, programs: &[("between", "a b < 0 a c < 1 0 ? ?")] },
    Update {
        change: Change::NoneWide,
        programs: &[
            ("cut", "a b < -5000 a b - ?"),
            ("pos", "a 0 > a -5000 ?"),
            ("isnone", "a -5000 ="),
            ("either", "a -5000 = b a ?"),
            ("plus", "a -5000 = -5000 b -5000 = -5000 a b + ? ?"),
        ],
    },
    Update {
        change: Change::CutSigned,
        programs: &[
            ("cut", "a b - -1000 < -5000 a b - 1000 > -5000 a b - ? ?"),
            ("pos", "a 0 > a -5000 ?"),
            ("isnone", "a -5000 ="),
            ("either", "a -5000 = b a ?"),
            ("plus", "a -5000 = -5000 b -5000 = -5000 a b + ? ?"),
        ],
    },
    Update { change: Change::WeighSubtracts, programs: &[("weigh", "a b c gap@ a b c count3@ -")] },
    Update { change: Change::IsMoreFar, programs: &[("ismore", "a 2 >")] },
    Update { change: Change::RankWide, programs: &[("rank", "a b 2 + > 3 a b > 1 a b = 0 a 2 + b < -3 -1 ? ? ? ?")] },
    Update { change: Change::NearerTies, programs: &[("nearer", "a b dist@ a c dist@ < b c ?")] },
    Update {
        change: Change::AbsSaturates,
        programs: &[("abs", "a 0 < a neg a ? dup -1 > swap 9223372036854775807 ?")],
    },
    Update { change: Change::MixSubtracts, programs: &[("mix", "a b tilt@ a b clip@ -")] },
];

/// Each change request's target under it, in the correct library with the change's updates.
pub(super) const REVISIONS: &[Revision] = &[
    Revision {
        change: Change::ClampOrder,
        candidates: c(
            "clamp3",
            &["c a max2@ b min2@", "a c max2@ b min2@"],
            &["c b min2@ a max2@", "a 7 < c a max2@ b min2@ c a max2@ b min2@ 1 + ?"],
        ),
    },
    Revision {
        change: Change::ClampHiFirst,
        candidates: c(
            "clamp3",
            &["c b max2@ a min2@", "b c max2@ a min2@"],
            &["c a min2@ b max2@", "a 7 < c b max2@ a min2@ c b max2@ a min2@ 1 + ?"],
        ),
    },
    Revision {
        change: Change::SignOfZero,
        candidates: c(
            "sign",
            &["a 0 < -1 1 ?", "0 a > -1 1 ?"],
            &["a 0 > 1 -1 ?", "a 7 < a 0 < -1 1 ? a 0 < -1 1 ? 1 + ?"],
        ),
    },
    Revision {
        change: Change::BetweenExclusive,
        candidates: c(
            "between",
            &["a b < 0 a c < 1 0 ? ?", "a b < 0 = a c < *"],
            &["a b < 0 a c > 0 1 ? ?", "a 7 < a b < 0 a c < 1 0 ? ? a b < 0 a c < 1 0 ? ? 1 + ?"],
        ),
    },
    Revision {
        change: Change::CutSigned,
        candidates: c(
            "cut",
            &["a b - -1000 < -5000 a b - 1000 > -5000 a b - ? ?", "a b - dup -1000 < swap 1000 > + -5000 a b - ?"],
            &["a b < -5000 a b - ?", "a b - -1000 < -5000 a b - ?"],
        ),
    },
    Revision {
        change: Change::NoneWide,
        candidates: c("isnone", &["a -5000 =", "-5000 a ="], &["a -1 =", "a -5000 = a 18 > +"]),
    },
    Revision {
        change: Change::WeighSubtracts,
        candidates: c(
            "weigh",
            &["a b c gap@ a b c count3@ -", "a b c count3@ neg a b c gap@ +"],
            &["a b c gap@ a b c count3@ +", "a 7 < a b c gap@ a b c count3@ - a b c gap@ a b c count3@ - 1 + ?"],
        ),
    },
    Revision {
        change: Change::RankWide,
        candidates: c(
            "rank",
            &["a b 2 + > 3 a b > 1 a b = 0 a 2 + b < -3 -1 ? ? ? ?", "a b < -1 a b > 1 0 ? ?"],
            &["b a -", "a b -"],
        ),
    },
    Revision {
        change: Change::IsMoreFar,
        candidates: c("ismore", &["a 2 >", "2 a <"], &["a 0 >", "a 12 > 0 a 2 > ?"]),
    },
    Revision {
        change: Change::NearerTies,
        candidates: c(
            "nearer",
            &["a b dist@ a c dist@ < b c ?", "a c dist@ a b dist@ > b c ?"],
            &["a b dist@ a c dist@ > c b ?", "a 7 < a b dist@ a c dist@ < b c ? a b dist@ a c dist@ < b c ? 1 + ?"],
        ),
    },
    Revision {
        change: Change::AbsSaturates,
        candidates: c(
            "abs",
            &[
                "a 0 < a neg a ? dup -1 > swap 9223372036854775807 ?",
                "a -9223372036854775807 < 9223372036854775807 a 0 < a neg a ? ?",
            ],
            &[
                "a 0 < a neg a ?",
                "a 7 < a 0 < a neg a ? dup -1 > swap 9223372036854775807 ? a 0 < a neg a ? dup -1 > swap 9223372036854775807 ? 1 + ?",
            ],
        ),
    },
    Revision {
        change: Change::MixSubtracts,
        candidates: c(
            "mix",
            &["a b tilt@ a b clip@ -", "a b clip@ neg a b tilt@ +"],
            &["a b tilt@ a b clip@ +", "a 7 < a b tilt@ a b clip@ - a b tilt@ a b clip@ - 1 + ?"],
        ),
    },
];
