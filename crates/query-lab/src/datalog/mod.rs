//! The Datalog core (design note `f2cf04a0a0b7`, section 3): base relations read from a Ground's tables, a
//! prelude of rules that defines the CLI's derived notions once, and an evaluator.
//!
//! Base relations, with contributions as `Id`s (rows, so `<` is causal order) and authors as `Agent`s:
//!
//! | relation | columns |
//! |---|---|
//! | `contribution` | C, kind (`claim`, `artifact`, `relation`, `evidence`, `retract`) |
//! | `author` | C, A |
//! | `parent` | C, P |
//! | `subject` | C: a claim or artifact |
//! | `claim` | C, statement |
//! | `tag` | C, tag |
//! | `artifact` | C, name, media type |
//! | `relation` | R, from, kind, to |
//! | `evidence` | E, target, verdict (`pass`, `fail`), method |
//! | `retract` | R, target |
//! | `retracted` | C: retracted under the protocol (computed, see below) |
//! | `agent` | A, local name (or the key's hex) |
//! | `validator` | A: whose evidence counts under the policy (every agent unless validators are named) |
//!
//! `retracted` is the one derived notion that isn't a rule. `retracted(X) :- retract(R, X), !retracted(R).`
//! negates itself through recursion, so it isn't stratified; it's sound only because a retract comes after
//! its target in causal order (local stratification). The engine doesn't stratify by causal order, so it
//! computes the relation natively, as manyroot does, in one backwards pass.

pub mod eval;
pub mod magic;
pub mod parse;

use anyhow::{Context, Result};
use manyroot::{BodyKind, Verdict};

use crate::ground::{Fact, Ground};
pub use eval::{Answer, Db, Value};
pub use parse::{Program, parse};

/// The rules every query can use: `current`, open findings, acceptance and the merge gate's checks.
pub const PRELUDE: &str = include_str!("prelude.dl");

/// What a query is evaluated against.
pub struct Options<'a> {
    /// Whose evidence triages findings, and whose `merge gate` pass says a PR merged.
    pub triager: Option<&'a str>,
    /// Whose evidence counts toward acceptance; everyone's if empty.
    pub validators: &'a [String],
    pub min_passes: i64,
    /// Further parameters: `$name` to a contribution id or prefix.
    pub ids: &'a [(String, String)],
    /// Rewrite queries that bind a recursive predicate's argument by the demand transformation.
    pub magic: bool,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options { triager: Some("manager"), validators: &[], min_passes: 1, ids: &[], magic: true }
    }
}

fn kind_name(k: BodyKind) -> &'static str {
    match k {
        BodyKind::Claim => "claim",
        BodyKind::Artifact => "artifact",
        BodyKind::Relation => "relation",
        BodyKind::Evidence => "evidence",
        BodyKind::Retract => "retract",
    }
}

/// The base relations of a Ground, with the options' parameters bound.
pub fn database(g: &Ground, opts: &Options) -> Result<Db> {
    let mut db = Db::default();
    for (name, arity) in [
        ("contribution", 2),
        ("author", 2),
        ("parent", 2),
        ("subject", 1),
        ("claim", 2),
        ("tag", 2),
        ("artifact", 3),
        ("relation", 4),
        ("evidence", 4),
        ("retract", 2),
        ("retracted", 1),
        ("agent", 2),
        ("validator", 1),
    ] {
        db.relation(name, arity);
    }
    let retracted = g.retracted();
    for (row, retracted) in retracted.iter().enumerate() {
        let c = Value::Id(row as u32);
        let kind = db.sym(kind_name(g.fact[row].kind()));
        db.add("contribution", &[c, kind]);
        db.add("author", &[c, Value::Agent(g.author[row])]);
        for p in &g.parents[row] {
            db.add("parent", &[c, Value::Id(*p)]);
        }
        if *retracted {
            db.add("retracted", &[c]);
        }
        match &g.fact[row] {
            Fact::Claim { statement, tags } => {
                db.add("subject", &[c]);
                let s = db.sym(statement);
                db.add("claim", &[c, s]);
                for t in tags {
                    let t = db.sym(&g.tags[*t as usize]);
                    db.add("tag", &[c, t]);
                }
            }
            Fact::Artifact { name, media_type } => {
                db.add("subject", &[c]);
                let (n, m) = (db.sym(name), db.sym(media_type));
                db.add("artifact", &[c, n, m]);
            }
            Fact::Relation { from, kind, to } => {
                let k = db.sym(kind.as_str());
                db.add("relation", &[c, Value::Id(*from), k, Value::Id(*to)]);
            }
            Fact::Evidence { target, verdict, method } => {
                let v = db.sym(match verdict {
                    Verdict::Pass => "pass",
                    Verdict::Fail => "fail",
                });
                let m = db.sym(method);
                db.add("evidence", &[c, Value::Id(*target), v, m]);
            }
            Fact::Retract { target } => db.add("retract", &[c, Value::Id(*target)]),
        }
    }
    for a in 0..g.agents.len() {
        let name = g.agent_names[a].clone().unwrap_or_else(|| g.agents[a].to_hex());
        let n = db.sym(&name);
        db.add("agent", &[Value::Agent(a as u32), n]);
        if opts.validators.is_empty() {
            db.add("validator", &[Value::Agent(a as u32)]);
        }
    }
    for v in opts.validators {
        let a = g.agent(v).with_context(|| format!("no validator {v}"))?;
        db.add("validator", &[Value::Agent(a)]);
    }
    if let Some(t) = opts.triager {
        // A triager who never wrote anything can't triage, but the rules still need a value for it.
        let a = g.agent(t).map_or(Value::Agent(u32::MAX), Value::Agent);
        db.params.insert("triager".into(), a);
    }
    db.params.insert("min_passes".into(), Value::Int(opts.min_passes));
    for (name, id) in opts.ids {
        db.params.insert(name.clone(), Value::Id(g.resolve(id)?));
    }
    Ok(db)
}

/// Runs `text` (rules, then queries) on top of the prelude. Returns the database, with everything the
/// queries derived, and each query's answer.
pub fn run(g: &Ground, opts: &Options, text: &str) -> Result<(Db, Vec<Answer>)> {
    let mut db = database(g, opts)?;
    let mut program = parse(PRELUDE).context("the prelude")?;
    let user = parse(text)?;
    program.rules.extend(user.rules);
    let mut answers = Vec::new();
    for (n, q) in user.queries.iter().enumerate() {
        let body = if opts.magic { magic::rewrite(&mut program, q, n) } else { q.clone() };
        answers.push(eval::query(&mut db, &program, &body)?);
    }
    Ok((db, answers))
}

/// The contributions in an answer's first column, in causal order, each once.
pub fn ids(answer: &Answer) -> Vec<u32> {
    let mut ids: Vec<u32> =
        answer.rows.iter().filter_map(|r| if let Some(Value::Id(n)) = r.first() { Some(*n) } else { None }).collect();
    ids.dedup();
    ids
}

/// A value as text: a contribution's full id, an agent's name, a symbol, a number.
pub fn show(g: &Ground, syms: &eval::Interner, v: Value) -> String {
    match v {
        Value::Id(r) => g.ids[r as usize].to_hex(),
        Value::Agent(a) => {
            g.agent_names.get(a as usize).cloned().flatten().unwrap_or_else(|| g.agents[a as usize].to_hex())
        }
        Value::Sym(s) => syms.strings[s as usize].clone(),
        Value::Int(n) => n.to_string(),
    }
}
