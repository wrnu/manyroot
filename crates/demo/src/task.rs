//! The toy task: small integer functions written in a tiny stack language.
//!
//! Answers are checkable by running a program on a grid of inputs, so agents
//! can produce real evidence without a compiler or sandbox. Some candidate
//! programs are wrong only on a few inputs, so a check on a random sample can
//! miss the bug: that is what makes a validation policy matter.

use crate::rng::Rng;

/// The language, as the task claim states it to agents.
pub const LANGUAGE: &str = "Programs are whitespace-separated tokens run on a stack of integers: \
    an integer pushes itself; `a` and `b` push the inputs; `+ - *` pop two values and push the result; \
    `< > =` pop two values and push 1 or 0; `neg` negates the top; `dup` copies it; `swap` swaps the top two; \
    `c x y ?` pushes x if c is not 0, else y. A program must leave exactly one value.";

/// One function to implement.
pub struct Spec {
    pub name: &'static str,
    pub arity: u8,
    pub description: &'static str,
    pub reference: fn(i64, i64) -> i64,
    /// Candidate programs that are correct on every input.
    pub correct: &'static [&'static str],
    /// Candidate programs with a bug, some of which fail only on a few inputs.
    pub buggy: &'static [&'static str],
}

pub const SPECS: &[Spec] = &[
    Spec {
        name: "abs",
        arity: 1,
        description: "abs(a): the absolute value of a",
        reference: |a, _| a.abs(),
        correct: &["a 0 < a neg a ?", "a 0 > a a neg ?"],
        buggy: &["a", "a -1 < a neg a ?"],
    },
    Spec {
        name: "max2",
        arity: 2,
        description: "max2(a, b): the larger of a and b",
        reference: |a, b| a.max(b),
        correct: &["a b > a b ?", "a b < b a ?"],
        buggy: &["a b < a b ?", "a b = 0 a b > a b ? ?"],
    },
    Spec {
        name: "min2",
        arity: 2,
        description: "min2(a, b): the smaller of a and b",
        reference: |a, b| a.min(b),
        correct: &["a b < a b ?", "a b > b a ?"],
        buggy: &["a b > a b ?", "a b - 6 = a a b < a b ? ?"],
    },
    Spec {
        name: "sign",
        arity: 1,
        description: "sign(a): 1 if a is positive, -1 if negative, 0 if zero",
        reference: |a, _| a.signum(),
        correct: &["a 0 > a 0 < -", "a 0 > 1 a 0 < -1 0 ? ?"],
        buggy: &["a 0 > 1 -1 ?", "a 0 < 1 -1 ?"],
    },
    Spec {
        name: "clamp3",
        arity: 1,
        description: "clamp3(a): a limited to the range -3 to 3",
        reference: |a, _| a.clamp(-3, 3),
        correct: &["a -3 < -3 a 3 > 3 a ? ?", "a 3 > 3 a -3 < -3 a ? ?"],
        buggy: &["a 3 > 3 a ?", "a -3 < -3 a 4 > 3 a ? ?"],
    },
    Spec {
        name: "dist",
        arity: 2,
        description: "dist(a, b): the distance between a and b, |a - b|",
        reference: |a, b| (a - b).abs(),
        correct: &["a b - dup 0 < swap dup neg swap ?", "a b > a b - b a - ?"],
        buggy: &["a b -", "a b > a b - a b - ?"],
    },
];

/// Inputs range over -6..=6. One-argument functions use b = 0.
const RANGE: std::ops::RangeInclusive<i64> = -6..=6;

pub fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

impl Spec {
    /// Every input the full check covers.
    pub fn grid(&self) -> Vec<(i64, i64)> {
        let bs: Vec<i64> = if self.arity == 1 { vec![0] } else { RANGE.collect() };
        RANGE.flat_map(|a| bs.iter().map(move |&b| (a, b))).collect()
    }

    /// `n` distinct inputs from the grid, chosen by `rng`.
    pub fn sample(&self, n: usize, rng: &mut Rng) -> Vec<(i64, i64)> {
        let mut grid = self.grid();
        rng.shuffle(&mut grid);
        grid.truncate(n);
        grid
    }

    /// Runs `program` on `cases` and compares it with the reference.
    pub fn check(&self, program: &str, cases: &[(i64, i64)]) -> Check {
        let mut passed = 0;
        let mut failure = None;
        for &(a, b) in cases {
            let want = (self.reference)(a, b);
            match eval(program, a, b) {
                Ok(got) if got == want => passed += 1,
                outcome => {
                    let call = if self.arity == 1 {
                        format!("{}({a})", self.name)
                    } else {
                        format!("{}({a}, {b})", self.name)
                    };
                    let got = match outcome {
                        Ok(got) => format!("gave {got}"),
                        Err(e) => format!("failed: {e}"),
                    };
                    failure.get_or_insert(format!("{call} {got}, expected {want}"));
                }
            }
        }
        Check { passed, total: cases.len(), first_failure: failure }
    }

    /// Whether `program` is right on every input.
    pub fn is_correct(&self, program: &str) -> bool {
        self.check(program, &self.grid()).ok()
    }

    /// The inputs `program` gets wrong, in grid order: what it does, whatever its text.
    pub fn wrong_on(&self, program: &str) -> Vec<(i64, i64)> {
        self.grid().into_iter().filter(|&(a, b)| eval(program, a, b) != Ok((self.reference)(a, b))).collect()
    }
}

/// The outcome of running a program on some inputs.
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

    /// One line for evidence details.
    pub fn details(&self) -> String {
        match &self.first_failure {
            None => format!("{}/{} cases passed", self.passed, self.total),
            Some(f) => format!("{f} ({}/{} cases passed)", self.passed, self.total),
        }
    }
}

/// Runs a program with inputs `a` and `b`. Arithmetic wraps, so no input panics.
pub fn eval(program: &str, a: i64, b: i64) -> Result<i64, String> {
    let mut stack: Vec<i64> = Vec::new();
    for token in program.split_whitespace() {
        let pop = |stack: &mut Vec<i64>| stack.pop().ok_or_else(|| format!("stack underflow at `{token}`"));
        let value = match token {
            "a" => a,
            "b" => b,
            "neg" => pop(&mut stack)?.wrapping_neg(),
            "dup" => {
                let x = pop(&mut stack)?;
                stack.push(x);
                x
            }
            "swap" => {
                let (y, x) = (pop(&mut stack)?, pop(&mut stack)?);
                stack.push(y);
                x
            }
            "+" | "-" | "*" | "<" | ">" | "=" => {
                let (y, x) = (pop(&mut stack)?, pop(&mut stack)?);
                match token {
                    "+" => x.wrapping_add(y),
                    "-" => x.wrapping_sub(y),
                    "*" => x.wrapping_mul(y),
                    "<" => i64::from(x < y),
                    ">" => i64::from(x > y),
                    _ => i64::from(x == y),
                }
            }
            "?" => {
                let (y, x, c) = (pop(&mut stack)?, pop(&mut stack)?, pop(&mut stack)?);
                if c != 0 { x } else { y }
            }
            literal => literal.parse().map_err(|_| format!("unknown token `{literal}`"))?,
        };
        stack.push(value);
    }
    match stack.as_slice() {
        [v] => Ok(*v),
        s => Err(format!("program left {} values", s.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_what_they_claim() {
        for s in SPECS {
            for p in s.correct {
                assert!(s.is_correct(p), "{} should be correct: {p}", s.name);
            }
            for p in s.buggy {
                assert!(!s.is_correct(p), "{} should be buggy: {p}", s.name);
            }
        }
    }

    #[test]
    fn errors_are_failures_not_panics() {
        assert_eq!(eval("a +", 1, 2), Err("stack underflow at `+`".into()));
        assert_eq!(eval("a b", 1, 2), Err("program left 2 values".into()));
        assert!(eval("a x", 1, 2).is_err());
        assert_eq!(eval("a 1 -", i64::MIN, 0), Ok(i64::MAX));
        let check = spec("max2").expect("max2 is a spec").check("a b < a b ?", &[(1, 1), (3, 5)]);
        assert_eq!(check.details(), "max2(3, 5) gave 3, expected 5 (1/2 cases passed)");
    }
}
