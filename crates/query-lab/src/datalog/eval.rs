//! Evaluation: rules are compiled against a database of relations, split into strata by their negations
//! and counts, and each stratum is run to a fixpoint by semi-naive iteration. Only the rules a query can
//! reach are compiled, so the prelude costs nothing for what a query doesn't use.
//!
//! Joins run left to right in an order the planner picks per rule: the most-bound atom next, ties to the
//! smaller relation, every filter as soon as its variables are bound, and in a semi-naive pass the changed
//! atom first. An atom with bound columns reads a hash index on exactly those columns, built on first use
//! and extended as the relation grows.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::parse::{Atom, CmpOp, Literal, Program, Rule, Term};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Value {
    /// A contribution, by its row: comparing two compares their causal order.
    Id(u32),
    Agent(u32),
    Sym(u32),
    Int(i64),
}

#[derive(Default)]
pub struct Interner {
    pub strings: Vec<String>,
    of: HashMap<String, u32>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(n) = self.of.get(s) {
            return *n;
        }
        let n = u32::try_from(self.strings.len()).expect("fewer than 2^32 symbols");
        self.strings.push(s.to_owned());
        self.of.insert(s.to_owned(), n);
        n
    }

    pub fn get(&self, s: &str) -> Option<u32> {
        self.of.get(s).copied()
    }
}

type Tuple = Box<[Value]>;

struct Index {
    upto: usize,
    map: HashMap<Tuple, Vec<u32>>,
}

pub struct Relation {
    pub arity: usize,
    pub tuples: Vec<Tuple>,
    set: HashSet<Tuple>,
    indexes: RefCell<HashMap<u64, Index>>,
}

impl Relation {
    pub fn new(arity: usize) -> Relation {
        Relation { arity, tuples: Vec::new(), set: HashSet::new(), indexes: RefCell::new(HashMap::new()) }
    }

    /// Adds a tuple; false if it was there.
    pub fn insert(&mut self, t: Tuple) -> bool {
        debug_assert_eq!(t.len(), self.arity);
        if self.set.contains(&t) {
            return false;
        }
        self.set.insert(t.clone());
        self.tuples.push(t);
        true
    }

    fn ensure_index(&self, mask: u64) {
        let mut indexes = self.indexes.borrow_mut();
        let index = indexes.entry(mask).or_insert_with(|| Index { upto: 0, map: HashMap::new() });
        for (n, t) in self.tuples.iter().enumerate().skip(index.upto) {
            index.map.entry(key(t, mask)).or_default().push(n as u32);
        }
        index.upto = self.tuples.len();
    }
}

fn key(t: &[Value], mask: u64) -> Tuple {
    t.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, v)| *v).collect()
}

#[derive(Default)]
pub struct Db {
    pub syms: Interner,
    pub rels: HashMap<String, Relation>,
    pub params: HashMap<String, Value>,
}

impl Db {
    pub fn relation(&mut self, name: &str, arity: usize) -> &mut Relation {
        self.rels.entry(name.to_owned()).or_insert_with(|| Relation::new(arity))
    }

    pub fn add(&mut self, name: &str, t: &[Value]) {
        self.relation(name, t.len()).insert(t.into());
    }

    pub fn sym(&mut self, s: &str) -> Value {
        Value::Sym(self.syms.intern(s))
    }
}

#[derive(Debug, Clone, Copy)]
enum CTerm {
    Var(usize),
    Wild,
    Const(Value),
}

#[derive(Debug, Clone)]
enum CLit {
    Pos { pred: String, args: Vec<CTerm> },
    Neg { pred: String, args: Vec<CTerm> },
    Cmp(CTerm, CmpOp, CTerm),
    StartsWith(CTerm, CTerm),
    Count { pred: String, args: Vec<CTerm>, var: usize, op: CmpOp, rhs: CTerm },
}

fn vars_of(args: &[CTerm]) -> Vec<usize> {
    args.iter().filter_map(|a| if let CTerm::Var(v) = a { Some(*v) } else { None }).collect()
}

impl CLit {
    /// The variables a filter needs bound before it can run.
    fn needs(&self) -> Vec<usize> {
        match self {
            CLit::Pos { .. } => vec![],
            CLit::Neg { args, .. } => vars_of(args),
            CLit::Cmp(a, _, b) | CLit::StartsWith(a, b) => vars_of(&[*a, *b]),
            CLit::Count { args, var, rhs, .. } => {
                let mut v = vars_of(args);
                v.retain(|x| x != var);
                v.extend(vars_of(&[*rhs]));
                v
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CRule {
    head_pred: String,
    head: Vec<CTerm>,
    nvars: usize,
    body: Vec<CLit>,
    names: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    /// Read a positive atom: every tuple, the index on `mask`'s columns, or with `delta` only the tuples
    /// the last pass added.
    Scan {
        lit: usize,
        mask: u64,
        delta: bool,
    },
    Filter {
        lit: usize,
    },
    /// `Var = term` with the variable unbound: binds it.
    Assign {
        var: usize,
        from: CTerm,
    },
}

/// Built-in predicates, which aren't relations.
const BUILTINS: &[&str] = &["starts_with"];

fn compile_rule(db: &mut Db, rule: &Rule) -> Result<CRule> {
    let mut names: Vec<String> = Vec::new();
    fn var(name: &str, names: &mut Vec<String>) -> usize {
        match names.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                names.push(name.to_owned());
                names.len() - 1
            }
        }
    }
    // Every symbol is interned: one in a head is a value the rule derives, and one only in a body matches
    // nothing, as no fact holds it.
    fn term(db: &mut Db, t: &Term, names: &mut Vec<String>) -> Result<CTerm> {
        Ok(match t {
            Term::Var(v) => CTerm::Var(var(v, names)),
            Term::Wild => CTerm::Wild,
            Term::Int(n) => CTerm::Const(Value::Int(*n)),
            Term::Sym(s) => CTerm::Const(db.sym(s)),
            Term::Param(p) => CTerm::Const(*db.params.get(p).with_context(|| format!("no value for ${p}"))?),
        })
    }
    fn args(db: &mut Db, a: &Atom, names: &mut Vec<String>) -> Result<Vec<CTerm>> {
        a.args.iter().map(|t| term(db, t, names)).collect()
    }
    let mut body = Vec::new();
    for lit in &rule.body {
        body.push(match lit {
            Literal::Pos(a) if a.pred == "starts_with" => {
                let args = args(db, a, &mut names)?;
                if args.len() != 2 {
                    bail!("starts_with takes two arguments");
                }
                CLit::StartsWith(args[0], args[1])
            }
            Literal::Pos(a) => CLit::Pos { pred: a.pred.clone(), args: args(db, a, &mut names)? },
            Literal::Neg(a) => CLit::Neg { pred: a.pred.clone(), args: args(db, a, &mut names)? },
            Literal::Cmp(a, op, b) => CLit::Cmp(term(db, a, &mut names)?, *op, term(db, b, &mut names)?),
            Literal::Count { var: v, atom, op, rhs } => {
                let v = var(v, &mut names);
                let atom_args = args(db, atom, &mut names)?;
                CLit::Count {
                    pred: atom.pred.clone(),
                    args: atom_args,
                    var: v,
                    op: *op,
                    rhs: term(db, rhs, &mut names)?,
                }
            }
        });
    }
    let head = rule.head.args.iter().map(|t| term(db, t, &mut names)).collect::<Result<Vec<_>>>()?;
    Ok(CRule { head_pred: rule.head.pred.clone(), head, nvars: names.len(), body, names })
}

/// The order a rule's literals run in, with `delta` (a body position) read first.
fn plan(db: &Db, rule: &CRule, delta: Option<usize>) -> Result<Vec<Step>> {
    let mut bound = vec![false; rule.nvars];
    let mut done = vec![false; rule.body.len()];
    let mut steps = Vec::new();
    let scan = |lit: usize, bound: &mut Vec<bool>, done: &mut Vec<bool>, steps: &mut Vec<Step>, delta: bool| {
        let CLit::Pos { args, .. } = &rule.body[lit] else { unreachable!("scans read atoms") };
        let mut mask = 0u64;
        for (i, a) in args.iter().enumerate() {
            match a {
                CTerm::Const(_) => mask |= 1 << i,
                CTerm::Var(v) if bound[*v] => mask |= 1 << i,
                _ => {}
            }
        }
        for v in vars_of(args) {
            bound[v] = true;
        }
        done[lit] = true;
        steps.push(Step::Scan { lit, mask: if delta { 0 } else { mask }, delta });
    };
    if let Some(d) = delta {
        scan(d, &mut bound, &mut done, &mut steps, true);
    }
    loop {
        // Every filter whose variables are bound, and every assignment that can run.
        let mut progress = true;
        while progress {
            progress = false;
            for (i, lit) in rule.body.iter().enumerate() {
                if done[i] || matches!(lit, CLit::Pos { .. }) {
                    continue;
                }
                if let CLit::Cmp(a, CmpOp::Eq, b) = lit {
                    let unbound = |t: &CTerm| matches!(t, CTerm::Var(v) if !bound[*v]);
                    let ready = |t: &CTerm| !unbound(t) && !matches!(t, CTerm::Wild);
                    let assign = match (a, b) {
                        (CTerm::Var(v), from) if unbound(a) && ready(from) => Some((*v, *from)),
                        (from, CTerm::Var(v)) if unbound(b) && ready(from) => Some((*v, *from)),
                        _ => None,
                    };
                    if let Some((v, from)) = assign {
                        bound[v] = true;
                        done[i] = true;
                        steps.push(Step::Assign { var: v, from });
                        progress = true;
                        continue;
                    }
                }
                if lit.needs().iter().all(|v| bound[*v]) {
                    done[i] = true;
                    steps.push(Step::Filter { lit: i });
                    progress = true;
                }
            }
        }
        let next = rule
            .body
            .iter()
            .enumerate()
            .filter(|(i, l)| !done[*i] && matches!(l, CLit::Pos { .. }))
            .max_by_key(|(_, l)| {
                let CLit::Pos { pred, args } = l else { unreachable!("filtered to atoms") };
                let bound_args = args
                    .iter()
                    .filter(|a| matches!(a, CTerm::Const(_)) || matches!(a, CTerm::Var(v) if bound[*v]))
                    .count();
                let size = db.rels.get(pred).map_or(0, |r| r.tuples.len());
                (bound_args, std::cmp::Reverse(size))
            })
            .map(|(i, _)| i);
        match next {
            Some(i) => scan(i, &mut bound, &mut done, &mut steps, false),
            None => break,
        }
    }
    if let Some(i) = done.iter().position(|d| !d) {
        let unbound: Vec<&str> =
            rule.body[i].needs().iter().filter(|v| !bound[**v]).map(|v| rule.names[*v].as_str()).collect();
        bail!("in a rule for {}: {} never bound by a positive atom", rule.head_pred, unbound.join(", "));
    }
    for t in &rule.head {
        match t {
            CTerm::Var(v) if !bound[*v] => {
                bail!("in a rule for {}: head variable {} is unbound", rule.head_pred, rule.names[*v])
            }
            CTerm::Wild => bail!("in a rule for {}: `_` in a head", rule.head_pred),
            _ => {}
        }
    }
    Ok(steps)
}

struct Exec<'a> {
    db: &'a Db,
    rule: &'a CRule,
    steps: &'a [Step],
    /// The range a delta scan reads.
    delta: (usize, usize),
    out: Vec<Tuple>,
}

impl Exec<'_> {
    fn value(t: &CTerm, b: &[Option<Value>]) -> Option<Value> {
        match t {
            CTerm::Var(v) => b[*v],
            CTerm::Const(c) => Some(*c),
            CTerm::Wild => None,
        }
    }

    fn compare(a: Value, op: CmpOp, b: Value) -> bool {
        let same_kind = std::mem::discriminant(&a) == std::mem::discriminant(&b);
        match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => same_kind && a < b,
            CmpOp::Le => same_kind && a <= b,
            CmpOp::Gt => same_kind && a > b,
            CmpOp::Ge => same_kind && a >= b,
        }
    }

    /// Unifies a tuple with an atom's arguments, binding what's unbound and noting it in `newly`.
    fn unify(args: &[CTerm], t: &[Value], b: &mut [Option<Value>], newly: &mut Vec<usize>) -> bool {
        for (a, v) in args.iter().zip(t) {
            match a {
                CTerm::Wild => {}
                CTerm::Const(c) => {
                    if c != v {
                        return false;
                    }
                }
                CTerm::Var(x) => match b[*x] {
                    Some(bv) if bv != *v => return false,
                    Some(_) => {}
                    None => {
                        b[*x] = Some(*v);
                        newly.push(*x);
                    }
                },
            }
        }
        true
    }

    /// The tuples of `pred` matching the bound columns `mask` names.
    fn matching(&self, pred: &str, args: &[CTerm], mask: u64, b: &[Option<Value>]) -> Vec<Tuple> {
        let Some(rel) = self.db.rels.get(pred) else { return Vec::new() };
        if mask == 0 {
            return rel.tuples.clone();
        }
        let k: Tuple = args
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, a)| Self::value(a, b).expect("masked columns are bound"))
            .collect();
        rel.ensure_index(mask);
        let indexes = rel.indexes.borrow();
        indexes[&mask]
            .map
            .get(&k)
            .map_or_else(Vec::new, |rows| rows.iter().map(|n| rel.tuples[*n as usize].clone()).collect())
    }

    fn mask_of(args: &[CTerm], b: &[Option<Value>]) -> u64 {
        let mut mask = 0;
        for (i, a) in args.iter().enumerate() {
            if matches!(a, CTerm::Const(_)) || matches!(a, CTerm::Var(v) if b[*v].is_some()) {
                mask |= 1 << i;
            }
        }
        mask
    }

    fn run(&mut self, at: usize, b: &mut Vec<Option<Value>>) {
        let Some(step) = self.steps.get(at) else {
            let t: Tuple = self.rule.head.iter().map(|h| Self::value(h, b).expect("heads are bound")).collect();
            self.out.push(t);
            return;
        };
        match *step {
            Step::Scan { lit, mask, delta } => {
                let CLit::Pos { pred, args } = &self.rule.body[lit] else { unreachable!("scans read atoms") };
                let found = if delta {
                    self.db.rels[pred].tuples[self.delta.0..self.delta.1].to_vec()
                } else {
                    self.matching(pred, args, mask, b)
                };
                let mut newly = Vec::new();
                for t in &found {
                    if Self::unify(args, t, b, &mut newly) {
                        self.run(at + 1, b);
                    }
                    for v in newly.drain(..) {
                        b[v] = None;
                    }
                }
            }
            Step::Assign { var, from } => {
                b[var] = Self::value(&from, b);
                self.run(at + 1, b);
                b[var] = None;
            }
            Step::Filter { lit } => {
                if self.filter(&self.rule.body[lit], b) {
                    self.run(at + 1, b);
                }
            }
        }
    }

    fn filter(&self, lit: &CLit, b: &[Option<Value>]) -> bool {
        let unifies = |args: &[CTerm], t: &[Value]| Self::unify(args, t, &mut b.to_vec(), &mut Vec::new());
        match lit {
            CLit::Neg { pred, args } => {
                !self.matching(pred, args, Self::mask_of(args, b), b).iter().any(|t| unifies(args, t))
            }
            CLit::Cmp(x, op, y) => match (Self::value(x, b), Self::value(y, b)) {
                (Some(x), Some(y)) => Self::compare(x, *op, y),
                _ => false,
            },
            CLit::StartsWith(s, p) => match (Self::value(s, b), Self::value(p, b)) {
                (Some(Value::Sym(s)), Some(Value::Sym(p))) => {
                    self.db.syms.strings[s as usize].starts_with(&self.db.syms.strings[p as usize])
                }
                _ => false,
            },
            CLit::Count { pred, args, var, op, rhs } => {
                let col = args.iter().position(|a| matches!(a, CTerm::Var(v) if v == var));
                let distinct: HashSet<Value> = self
                    .matching(pred, args, Self::mask_of(args, b), b)
                    .iter()
                    .filter(|t| unifies(args, t))
                    .filter_map(|t| col.map(|c| t[c]))
                    .collect();
                let n = Value::Int(i64::try_from(distinct.len()).expect("counts fit"));
                Self::value(rhs, b).is_some_and(|r| Self::compare(n, *op, r))
            }
            CLit::Pos { .. } => unreachable!("atoms are scans"),
        }
    }
}

type Deps = HashMap<String, Vec<(String, bool)>>;

/// The rules `roots` need, in strata, lowest first: each stratum's predicates and rules. A stratum reads
/// only its own predicates and lower strata's, and negates or counts only lower strata's.
fn strata(rules: &[Rule], roots: &[String]) -> Result<Vec<(Vec<String>, Vec<usize>)>> {
    let mut deps: Deps = HashMap::new();
    for r in rules {
        let entry = deps.entry(r.head.pred.clone()).or_default();
        for lit in &r.body {
            match lit {
                Literal::Pos(a) => entry.push((a.pred.clone(), false)),
                Literal::Neg(a) | Literal::Count { atom: a, .. } => entry.push((a.pred.clone(), true)),
                Literal::Cmp(..) => {}
            }
        }
    }
    let defined: HashSet<String> = deps.keys().cloned().collect();
    for list in deps.values_mut() {
        list.retain(|(p, _)| defined.contains(p));
    }
    // Tarjan's strongly connected components: each is emitted after everything it depends on.
    struct Tarjan<'a> {
        deps: &'a Deps,
        index: HashMap<String, usize>,
        low: HashMap<String, usize>,
        stack: Vec<String>,
        on: HashSet<String>,
        out: Vec<Vec<String>>,
    }
    fn visit(t: &mut Tarjan, p: &str) {
        let n = t.index.len();
        t.index.insert(p.to_owned(), n);
        t.low.insert(p.to_owned(), n);
        t.stack.push(p.to_owned());
        t.on.insert(p.to_owned());
        for (q, _) in t.deps[p].clone() {
            if !t.index.contains_key(&q) {
                visit(t, &q);
                let l = t.low[&q].min(t.low[p]);
                t.low.insert(p.to_owned(), l);
            } else if t.on.contains(&q) {
                let l = t.index[&q].min(t.low[p]);
                t.low.insert(p.to_owned(), l);
            }
        }
        if t.low[p] == t.index[p] {
            let mut comp = Vec::new();
            loop {
                let q = t.stack.pop().expect("p is on the stack");
                t.on.remove(&q);
                let done = q == p;
                comp.push(q);
                if done {
                    break;
                }
            }
            t.out.push(comp);
        }
    }
    let mut t = Tarjan {
        deps: &deps,
        index: HashMap::new(),
        low: HashMap::new(),
        stack: Vec::new(),
        on: HashSet::new(),
        out: Vec::new(),
    };
    for r in roots {
        if defined.contains(r) && !t.index.contains_key(r) {
            visit(&mut t, r);
        }
    }
    let mut out = Vec::new();
    for comp in t.out {
        for p in &comp {
            if let Some((q, _)) = deps[p].iter().find(|(q, neg)| *neg && comp.contains(q)) {
                bail!("{p} negates or counts {q} through recursion, so the program isn't stratified");
            }
        }
        let ids = rules.iter().enumerate().filter(|(_, r)| comp.contains(&r.head.pred)).map(|(i, _)| i).collect();
        out.push((comp, ids));
    }
    Ok(out)
}

/// What one evaluation did, for the benchmarks.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub rules: usize,
    pub strata: usize,
    pub passes: usize,
    pub derived: usize,
}

pub struct Answer {
    pub vars: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub stats: Stats,
}

/// Evaluates the rules a query needs, adding what they derive to `db`, then answers the query: one row per
/// distinct binding of its variables (in order of first use), sorted. Derived relations stay in `db`, so a
/// later query reuses them.
pub fn query(db: &mut Db, program: &Program, body: &[Literal]) -> Result<Answer> {
    let roots: Vec<String> = body
        .iter()
        .filter_map(|lit| match lit {
            Literal::Pos(a) | Literal::Neg(a) | Literal::Count { atom: a, .. } => Some(a.pred.clone()),
            Literal::Cmp(..) => None,
        })
        .collect();
    let mut stats = Stats::default();
    let defined: HashSet<&str> = program.rules.iter().map(|r| r.head.pred.as_str()).collect();
    for (preds, ids) in strata(&program.rules, &roots)? {
        if preds.iter().all(|p| db.rels.contains_key(p)) {
            continue; // derived by an earlier query on this database
        }
        stats.strata += 1;
        let rules: Vec<CRule> = ids.iter().map(|i| compile_rule(db, &program.rules[*i])).collect::<Result<_>>()?;
        stats.rules += rules.len();
        for r in &rules {
            check_preds(db, &r.body, &defined)?;
            let arity = db.relation(&r.head_pred, r.head.len()).arity;
            if arity != r.head.len() {
                bail!("{} is used with {arity} and {} arguments", r.head_pred, r.head.len());
            }
        }
        stats.passes += fixpoint(db, &rules, &preds, &mut stats.derived)?;
    }
    // The query is a rule whose head lists its variables.
    let mut vars: Vec<String> = Vec::new();
    let mut note = |t: &Term| {
        if let Term::Var(v) = t
            && !vars.contains(v)
        {
            vars.push(v.clone());
        }
    };
    for lit in body {
        match lit {
            Literal::Pos(a) => a.args.iter().for_each(&mut note),
            Literal::Cmp(a, CmpOp::Eq, b) => {
                note(a);
                note(b);
            }
            _ => {}
        }
    }
    let rule = Rule {
        head: Atom { pred: "?query".into(), args: vars.iter().map(|v| Term::Var(v.clone())).collect() },
        body: body.to_vec(),
    };
    let c = compile_rule(db, &rule)?;
    check_preds(db, &c.body, &defined)?;
    let steps = plan(db, &c, None)?;
    let mut exec = Exec { db, rule: &c, steps: &steps, delta: (0, 0), out: Vec::new() };
    exec.run(0, &mut vec![None; c.nvars]);
    let mut rows: Vec<Vec<Value>> = exec.out.into_iter().map(|t| t.into_vec()).collect();
    rows.sort();
    rows.dedup();
    Ok(Answer { vars, rows, stats })
}

/// Runs one stratum to its fixpoint; returns the passes it took. Pass 0 reads everything; each later
/// pass reads, at one recursive atom at a time, only what the pass before added.
fn fixpoint(db: &mut Db, rules: &[CRule], preds: &[String], derived: &mut usize) -> Result<usize> {
    let recursive = |l: &CLit| matches!(l, CLit::Pos { pred, .. } if preds.contains(pred));
    let any_recursive = rules.iter().any(|r| r.body.iter().any(recursive));
    let mut delta: HashMap<String, (usize, usize)> = HashMap::new();
    let mut passes = 0;
    loop {
        let mut new: Vec<(String, Tuple)> = Vec::new();
        for r in rules {
            let positions: Vec<Option<usize>> = if passes == 0 {
                vec![None]
            } else {
                r.body
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| match l {
                        CLit::Pos { pred, .. } if preds.contains(pred) => delta.get(pred).is_some_and(|(a, b)| a < b),
                        _ => false,
                    })
                    .map(|(i, _)| Some(i))
                    .collect()
            };
            for d in positions {
                let steps = plan(db, r, d)?;
                let range = d.map_or((0, 0), |i| {
                    let CLit::Pos { pred, .. } = &r.body[i] else { unreachable!("deltas are atoms") };
                    delta[pred]
                });
                let mut exec = Exec { db, rule: r, steps: &steps, delta: range, out: Vec::new() };
                exec.run(0, &mut vec![None; r.nvars]);
                new.extend(exec.out.into_iter().map(|t| (r.head_pred.clone(), t)));
            }
        }
        passes += 1;
        let before: HashMap<&String, usize> = preds.iter().map(|p| (p, db.rels[p].tuples.len())).collect();
        for (p, t) in new {
            if db.relation(&p, t.len()).insert(t) {
                *derived += 1;
            }
        }
        delta = preds.iter().map(|p| (p.clone(), (before[p], db.rels[p].tuples.len()))).collect();
        if !any_recursive || delta.values().all(|(a, b)| a == b) {
            return Ok(passes);
        }
    }
}

fn check_preds(db: &Db, body: &[CLit], defined: &HashSet<&str>) -> Result<()> {
    for lit in body {
        if let CLit::Pos { pred, args } | CLit::Neg { pred, args } | CLit::Count { pred, args, .. } = lit {
            match db.rels.get(pred) {
                Some(r) if r.arity != args.len() => bail!("{pred} has {} arguments, not {}", r.arity, args.len()),
                Some(_) => {}
                None if defined.contains(pred.as_str()) || BUILTINS.contains(&pred.as_str()) => {}
                None => bail!("no relation {pred}"),
            }
        }
    }
    Ok(())
}
