//! The Datalog surface:
//!
//! ```text
//! % a comment
//! current(X) :- contribution(X, _), !hidden(X).
//! accepted(X) :- subject(X), #count { A : pass_by(X, A) } >= $min_passes, A != B.
//! ?- current(X), tag(X, "pr").
//! ```
//!
//! Variables start with an upper-case letter or `_` (`_` alone is a fresh wildcard). Bare lower-case words
//! and `"strings"` are symbols; integers are numbers; `$name` is a parameter, bound before evaluation. A
//! query is a rule body; its answer is one row per binding of its variables, in order of first use.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Var(String),
    Wild,
    Sym(String),
    Int(i64),
    Param(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom {
    pub pred: String,
    pub args: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Pos(Atom),
    Neg(Atom),
    Cmp(Term, CmpOp, Term),
    /// `#count { V : atom } op rhs`: the number of distinct values of `V` among the atom's matches, the
    /// atom's other variables bound by the literals before it.
    Count {
        var: String,
        atom: Atom,
        op: CmpOp,
        rhs: Term,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub head: Atom,
    pub body: Vec<Literal>,
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub rules: Vec<Rule>,
    pub queries: Vec<Vec<Literal>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Var(String),
    Str(String),
    Int(i64),
    Param(String),
    Punct(&'static str),
}

const PUNCT: &[&str] = &["?-", ":-", "!=", "<=", ">=", "(", ")", ",", ".", "!", "=", "<", ">", "{", "}", ":", "#"];

fn lex(text: &str) -> Result<Vec<(Tok, usize)>> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'%' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'"' {
            let mut s = String::new();
            let mut chars = text[i + 1..].char_indices();
            loop {
                match chars.next() {
                    None => bail!("line {line}: unterminated string"),
                    Some((n, '"')) => {
                        i += 1 + n + 1;
                        break;
                    }
                    Some((_, '\\')) => match chars.next() {
                        Some((_, 'n')) => s.push('\n'),
                        Some((_, ch)) => s.push(ch),
                        None => bail!("line {line}: unterminated string"),
                    },
                    Some((_, ch)) => s.push(ch),
                }
            }
            out.push((Tok::Str(s), line));
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push((Tok::Int(text[start..i].parse()?), line));
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            out.push((
                if let Some(p) = word.strip_prefix('$') {
                    if p.is_empty() {
                        bail!("line {line}: `$` needs a name");
                    }
                    Tok::Param(p.to_owned())
                } else if c.is_ascii_uppercase() || c == b'_' {
                    Tok::Var(word.to_owned())
                } else {
                    Tok::Ident(word.to_owned())
                },
                line,
            ));
        } else if let Some(p) = PUNCT.iter().find(|p| text[i..].starts_with(**p)) {
            out.push((Tok::Punct(p), line));
            i += p.len();
        } else {
            bail!("line {line}: unexpected {:?}", text[i..].chars().next().expect("not at the end"));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(t, _)| t)
    }

    fn line(&self) -> usize {
        self.toks.get(self.at).or(self.toks.last()).map_or(1, |(_, l)| *l)
    }

    fn next(&mut self) -> Result<Tok> {
        let t = self.toks.get(self.at).map(|(t, _)| t.clone());
        self.at += 1;
        t.ok_or_else(|| anyhow::anyhow!("unexpected end of the program"))
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.peek() == Some(&Tok::Punct(PUNCT.iter().find(|x| **x == p).expect("a known punctuation"))) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<()> {
        if !self.eat(p) {
            bail!("line {}: expected `{p}`, found {:?}", self.line(), self.peek());
        }
        Ok(())
    }

    fn term(&mut self) -> Result<Term> {
        Ok(match self.next()? {
            Tok::Var(v) if v == "_" => Term::Wild,
            Tok::Var(v) => Term::Var(v),
            Tok::Ident(s) | Tok::Str(s) => Term::Sym(s),
            Tok::Int(n) => Term::Int(n),
            Tok::Param(p) => Term::Param(p),
            t => bail!("line {}: expected a term, found {t:?}", self.line()),
        })
    }

    fn atom_after(&mut self, pred: String) -> Result<Atom> {
        self.expect("(")?;
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                args.push(self.term()?);
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        Ok(Atom { pred, args })
    }

    fn atom(&mut self) -> Result<Atom> {
        match self.next()? {
            Tok::Ident(pred) => self.atom_after(pred),
            t => bail!("line {}: expected a predicate, found {t:?}", self.line()),
        }
    }

    fn op(&mut self) -> Result<CmpOp> {
        Ok(match self.next()? {
            Tok::Punct("=") => CmpOp::Eq,
            Tok::Punct("!=") => CmpOp::Ne,
            Tok::Punct("<") => CmpOp::Lt,
            Tok::Punct("<=") => CmpOp::Le,
            Tok::Punct(">") => CmpOp::Gt,
            Tok::Punct(">=") => CmpOp::Ge,
            t => bail!("line {}: expected a comparison, found {t:?}", self.line()),
        })
    }

    fn literal(&mut self) -> Result<Literal> {
        if self.eat("!") {
            return Ok(Literal::Neg(self.atom()?));
        }
        if self.eat("#") {
            match self.next()? {
                Tok::Ident(w) if w == "count" => {}
                t => bail!("line {}: expected `count` after `#`, found {t:?}", self.line()),
            }
            self.expect("{")?;
            let var = match self.next()? {
                Tok::Var(v) if v != "_" => v,
                t => bail!("line {}: #count counts a variable, found {t:?}", self.line()),
            };
            self.expect(":")?;
            let atom = self.atom()?;
            self.expect("}")?;
            let op = self.op()?;
            let rhs = self.term()?;
            return Ok(Literal::Count { var, atom, op, rhs });
        }
        // An atom, or a comparison whose left side is a term.
        if let Some(Tok::Ident(pred)) = self.peek().cloned()
            && matches!(self.toks.get(self.at + 1), Some((Tok::Punct("("), _)))
        {
            self.at += 1;
            return Ok(Literal::Pos(self.atom_after(pred)?));
        }
        let left = self.term()?;
        let op = self.op()?;
        Ok(Literal::Cmp(left, op, self.term()?))
    }

    fn body(&mut self) -> Result<Vec<Literal>> {
        let mut body = vec![self.literal()?];
        while self.eat(",") {
            body.push(self.literal()?);
        }
        self.expect(".")?;
        Ok(body)
    }
}

pub fn parse(text: &str) -> Result<Program> {
    let mut p = Parser { toks: lex(text)?, at: 0 };
    let mut program = Program::default();
    while p.peek().is_some() {
        if p.eat("?-") {
            program.queries.push(p.body()?);
            continue;
        }
        let head = p.atom()?;
        if p.eat(".") {
            program.rules.push(Rule { head, body: vec![] });
            continue;
        }
        p.expect(":-")?;
        program.rules.push(Rule { head, body: p.body()? });
    }
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_negation_counts_and_queries() {
        let p = parse(
            "% comment\nok(X) :- a(X, \"pr\"), !b(X, _), #count { A : c(X, A) } >= $min, X != 3.\nfact(one).\n?- ok(X).",
        )
        .expect("parses");
        assert_eq!(p.rules.len(), 2);
        assert_eq!(p.queries.len(), 1);
        let body = &p.rules[0].body;
        assert!(matches!(&body[0], Literal::Pos(a) if a.args[1] == Term::Sym("pr".into())));
        assert!(matches!(&body[1], Literal::Neg(a) if a.args[1] == Term::Wild));
        assert!(
            matches!(&body[2], Literal::Count { var, op: CmpOp::Ge, rhs: Term::Param(m), .. } if var == "A" && m == "min")
        );
        assert!(matches!(&body[3], Literal::Cmp(Term::Var(x), CmpOp::Ne, Term::Int(3)) if x == "X"));
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(parse("a(X) :- b(X)").is_err());
        assert!(parse("a(X) :- b(X.").is_err());
        assert!(parse("a(X) :- \"open").is_err());
        assert!(parse("a(X) :- #sum { A : b(A) } > 1.").is_err());
    }
}
