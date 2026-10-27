//! The stacklib language: modules of definitions that call each other, and an
//! interpreter with step and depth limits.
//!
//! It is the demo's stack language (`bracken_demo::task`) with three inputs
//! and calls. A module is text, one definition per line:
//!
//! ```text
//! # comments run to the end of the line
//! def clamp3 3 : a b max2@ c min2@
//! ```
//!
//! `name@` calls a function defined in any module of the library: it pops the
//! callee's arity in arguments (the deepest is the callee's `a`) and pushes
//! its result. Programs only ever run here, on integers with wrapping
//! arithmetic, within [`MAX_STEPS`] and [`MAX_DEPTH`].

use std::collections::{BTreeMap, BTreeSet};

/// Tokens one top-level evaluation may run, counting those of every call it makes.
pub const MAX_STEPS: u32 = 10_000;
/// Nested calls one evaluation may make.
pub const MAX_DEPTH: u32 = 64;
/// The most inputs a function takes: `a`, `b` and `c`.
pub const MAX_ARITY: u8 = 3;
/// The largest module that parses, in bytes: about 16 KiB, far more than the task needs.
pub const MAX_MODULE_BYTES: usize = 16 * 1024;
/// The longest function name.
pub const MAX_NAME_LEN: usize = 32;
/// How much of a bad word an error quotes.
const QUOTE_LEN: usize = 40;

/// Words that can't name a function.
const RESERVED: [&str; 7] = ["a", "b", "c", "neg", "dup", "swap", "def"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Int(i64),
    /// `a`, `b` or `c`: 0, 1 or 2.
    Input(u8),
    Add,
    Sub,
    Mul,
    Lt,
    Gt,
    Eq,
    Neg,
    Dup,
    Swap,
    /// `c x y ?`: x if c is not 0, else y.
    Select,
    Call(String),
}

/// One `def` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    pub arity: u8,
    pub body: Vec<Token>,
    /// The module it was defined in.
    pub module: String,
}

/// Why a module doesn't parse: the first bad line, 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 { write!(f, "{}", self.message) } else { write!(f, "line {}: {}", self.line, self.message) }
    }
}

/// Whether `word` can name a function: a lowercase letter, then lowercase
/// letters and digits, at most [`MAX_NAME_LEN`] in all.
pub fn is_function_name(word: &str) -> bool {
    let mut chars = word.chars();
    word.len() <= MAX_NAME_LEN
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !RESERVED.contains(&word)
}

/// `word` as an error message quotes it: at most [`QUOTE_LEN`] characters, so an
/// error never repeats a huge input.
fn quote(word: &str) -> String {
    match word.char_indices().nth(QUOTE_LEN) {
        Some((cut, _)) => format!("`{}...` ({} bytes)", &word[..cut], word.len()),
        None => format!("`{word}`"),
    }
}

/// Parses one module. A module with any bad line doesn't parse at all, as a
/// source file with a syntax error doesn't compile, and neither does one over
/// [`MAX_MODULE_BYTES`].
pub fn parse_module(module: &str, text: &str) -> Result<Vec<Def>, ParseError> {
    if text.len() > MAX_MODULE_BYTES {
        return Err(ParseError {
            line: 0,
            message: format!("module is {} bytes, over the limit of {MAX_MODULE_BYTES}", text.len()),
        });
    }
    let mut defs = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default();
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let def = parse_def(module, &words).map_err(|message| ParseError { line: i + 1, message })?;
        defs.push(def);
    }
    Ok(defs)
}

fn parse_def(module: &str, words: &[&str]) -> Result<Def, String> {
    let [def, name, arity, colon, body @ ..] = words else {
        return Err("expected `def <name> <arity> : <program>`".into());
    };
    if *def != "def" || *colon != ":" {
        return Err("expected `def <name> <arity> : <program>`".into());
    }
    if !is_function_name(name) {
        return Err(format!("{} can't name a function", quote(name)));
    }
    let arity: u8 = match arity.parse() {
        Ok(n) if (1..=MAX_ARITY).contains(&n) => n,
        _ => return Err(format!("arity must be 1 to {MAX_ARITY}, not {}", quote(arity))),
    };
    if body.is_empty() {
        return Err(format!("`{name}` has an empty program"));
    }
    let body = body.iter().map(|w| parse_token(w, arity)).collect::<Result<_, _>>()?;
    Ok(Def { name: name.to_string(), arity, body, module: module.to_string() })
}

fn parse_token(word: &str, arity: u8) -> Result<Token, String> {
    let token = match word {
        "a" => Token::Input(0),
        "b" => Token::Input(1),
        "c" => Token::Input(2),
        "+" => Token::Add,
        "-" => Token::Sub,
        "*" => Token::Mul,
        "<" => Token::Lt,
        ">" => Token::Gt,
        "=" => Token::Eq,
        "neg" => Token::Neg,
        "dup" => Token::Dup,
        "swap" => Token::Swap,
        "?" => Token::Select,
        _ => match word.strip_suffix('@') {
            Some(name) if is_function_name(name) => Token::Call(name.to_string()),
            Some(_) => return Err(format!("{} doesn't call a valid function name", quote(word))),
            None => word.parse().map(Token::Int).map_err(|_| format!("unknown token {}", quote(word)))?,
        },
    };
    if let Token::Input(i) = token
        && i >= arity
    {
        return Err(format!("input {} used in a function of arity {arity}", quote(word)));
    }
    Ok(token)
}

/// What a call resolves to.
pub enum Target<'a> {
    /// A definition, run by the interpreter.
    Code(&'a Def),
    /// A function given in Rust, such as a reference implementation. One call is one step.
    Native { arity: u8, eval: &'a dyn Fn(&[i64]) -> i64 },
}

/// How `name@` is resolved.
pub trait Calls {
    fn resolve(&self, name: &str) -> Option<Target<'_>>;
}

/// The modules of one library, parsed.
#[derive(Debug, Clone, Default)]
pub struct Library {
    defs: BTreeMap<String, Def>,
    /// Modules that don't parse, so contribute nothing.
    pub broken: Vec<(String, ParseError)>,
    /// Names defined more than once across the library. None of their definitions counts.
    pub duplicates: BTreeSet<String>,
}

impl Library {
    /// Parses `(module name, text)` pairs. Calls are resolved by name across all of them.
    pub fn parse<'a>(modules: impl IntoIterator<Item = (&'a str, &'a str)>) -> Library {
        let mut library = Library::default();
        for (module, text) in modules {
            match parse_module(module, text) {
                Ok(defs) => {
                    for def in defs {
                        if library.duplicates.contains(&def.name) {
                            continue;
                        }
                        if library.defs.remove(&def.name).is_some() {
                            library.duplicates.insert(def.name);
                        } else {
                            library.defs.insert(def.name.clone(), def);
                        }
                    }
                }
                Err(e) => library.broken.push((module.to_string(), e)),
            }
        }
        library
    }

    /// The one definition of `name`, if there is exactly one in a module that parses.
    pub fn get(&self, name: &str) -> Option<&Def> {
        self.defs.get(name)
    }

    /// Why `name` has no usable definition.
    pub fn why_missing(&self, name: &str) -> String {
        if self.duplicates.contains(name) {
            format!("`{name}` is defined more than once")
        } else if self.broken.is_empty() {
            format!("`{name}` is not defined")
        } else {
            let broken: Vec<String> = self.broken.iter().map(|(m, e)| format!("{m} ({e})")).collect();
            format!("`{name}` is not defined; modules that don't parse: {}", broken.join(", "))
        }
    }
}

impl Calls for Library {
    fn resolve(&self, name: &str) -> Option<Target<'_>> {
        self.get(name).map(Target::Code)
    }
}

/// Runs `def` on `args` (one per input), resolving calls with `calls`.
pub fn run(def: &Def, args: &[i64], calls: &dyn Calls) -> Result<i64, String> {
    if args.len() != usize::from(def.arity) {
        return Err(format!("`{}` takes {} inputs, not {}", def.name, def.arity, args.len()));
    }
    let mut steps = 0;
    exec(def, args, calls, &mut steps, 0)
}

fn exec(def: &Def, args: &[i64], calls: &dyn Calls, steps: &mut u32, depth: u32) -> Result<i64, String> {
    let mut stack: Vec<i64> = Vec::new();
    for token in &def.body {
        *steps += 1;
        if *steps > MAX_STEPS {
            return Err(format!("stopped after {MAX_STEPS} steps"));
        }
        let pop = |stack: &mut Vec<i64>| stack.pop().ok_or_else(|| format!("stack underflow in `{}`", def.name));
        let value = match token {
            Token::Int(n) => *n,
            Token::Input(i) => args[usize::from(*i)],
            Token::Neg => pop(&mut stack)?.wrapping_neg(),
            Token::Dup => {
                let x = pop(&mut stack)?;
                stack.push(x);
                x
            }
            Token::Swap => {
                let (y, x) = (pop(&mut stack)?, pop(&mut stack)?);
                stack.push(y);
                x
            }
            Token::Add | Token::Sub | Token::Mul | Token::Lt | Token::Gt | Token::Eq => {
                let (y, x) = (pop(&mut stack)?, pop(&mut stack)?);
                match token {
                    Token::Add => x.wrapping_add(y),
                    Token::Sub => x.wrapping_sub(y),
                    Token::Mul => x.wrapping_mul(y),
                    Token::Lt => i64::from(x < y),
                    Token::Gt => i64::from(x > y),
                    _ => i64::from(x == y),
                }
            }
            Token::Select => {
                let (y, x, c) = (pop(&mut stack)?, pop(&mut stack)?, pop(&mut stack)?);
                if c != 0 { x } else { y }
            }
            Token::Call(name) => {
                let target =
                    calls.resolve(name).ok_or_else(|| format!("`{}` calls `{name}`, which isn't defined", def.name))?;
                let arity = match &target {
                    Target::Code(d) => d.arity,
                    Target::Native { arity, .. } => *arity,
                };
                let n = usize::from(arity);
                if stack.len() < n {
                    return Err(format!("stack underflow calling `{name}` in `{}`", def.name));
                }
                let call_args = stack.split_off(stack.len() - n);
                match target {
                    Target::Code(callee) => {
                        if depth + 1 >= MAX_DEPTH {
                            return Err(format!("calls nested deeper than {MAX_DEPTH}"));
                        }
                        exec(callee, &call_args, calls, steps, depth + 1)?
                    }
                    Target::Native { eval, .. } => eval(&call_args),
                }
            }
        };
        stack.push(value);
    }
    match stack.as_slice() {
        [v] => Ok(*v),
        s => Err(format!("`{}` left {} values", def.name, s.len())),
    }
}
