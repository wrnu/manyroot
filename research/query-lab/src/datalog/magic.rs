//! The demand transformation (magic sets): a query that binds an argument of a recursive predicate, such as
//! `ancestor(A, $d)`, shouldn't derive the predicate for every contribution. The rewrite adorns each
//! predicate with which arguments arrive bound (`b`) or free (`f`), passing bindings left to right through
//! each rule body, and guards every adorned rule with a magic predicate that holds just the bindings
//! demanded. Bottom-up evaluation of the result derives only what the query can reach.
//!
//! It applies only where the predicates a query atom reaches use no negation and no counts; elsewhere the
//! query runs unchanged, which is always correct. How much it saves depends on the rules' shape: for
//! `ancestor(A, D) :- ancestor(M, D), parent(M, A).` and a bound `D`, the demand stays one contribution,
//! but the left-recursive form `ancestor(A, D) :- parent(D, M), ancestor(A, M).` demands every ancestor.

use std::collections::{HashMap, HashSet};

use super::parse::{Atom, Literal, Program, Rule, Term};

fn bound(t: &Term, vars: &HashSet<String>) -> bool {
    match t {
        Term::Var(v) => vars.contains(v),
        Term::Wild => false,
        Term::Sym(_) | Term::Int(_) | Term::Param(_) => true,
    }
}

fn adornment(args: &[Term], vars: &HashSet<String>) -> String {
    args.iter().map(|t| if bound(t, vars) { 'b' } else { 'f' }).collect()
}

fn bound_args(args: &[Term], adorn: &str) -> Vec<Term> {
    args.iter().zip(adorn.chars()).filter(|(_, a)| *a == 'b').map(|(t, _)| t.clone()).collect()
}

fn add_vars(args: &[Term], vars: &mut HashSet<String>) {
    for t in args {
        if let Term::Var(v) = t {
            vars.insert(v.clone());
        }
    }
}

/// Whether every predicate `pred` reaches is defined without negation or counts.
fn positive(program: &Program, pred: &str) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![pred.to_owned()];
    while let Some(p) = stack.pop() {
        if !seen.insert(p.clone()) {
            continue;
        }
        for r in program.rules.iter().filter(|r| r.head.pred == p) {
            for lit in &r.body {
                match lit {
                    Literal::Pos(a) => stack.push(a.pred.clone()),
                    Literal::Neg(_) | Literal::Count { .. } => return false,
                    Literal::Cmp(..) => {}
                }
            }
        }
    }
    true
}

/// Rewrites `body`'s atoms that bind an argument of a positive, defined predicate, adding the adorned and
/// magic rules to `program`. `tag` keeps this query's rules apart from another's on the same database, as
/// derived relations stay there. Returns the new body.
pub fn rewrite(program: &mut Program, body: &[Literal], tag: usize) -> Vec<Literal> {
    let defined: HashSet<String> = program.rules.iter().map(|r| r.head.pred.clone()).collect();
    let mut out = Vec::new();
    let mut added: Vec<Rule> = Vec::new();
    let mut done: HashMap<(String, String), String> = HashMap::new();
    let mut vars = HashSet::new();
    for lit in body {
        let Literal::Pos(a) = lit else {
            out.push(lit.clone());
            continue;
        };
        let adorn = adornment(&a.args, &vars);
        if !defined.contains(&a.pred) || !adorn.contains('b') || !positive(program, &a.pred) {
            add_vars(&a.args, &mut vars);
            out.push(lit.clone());
            continue;
        }
        let name = |p: &str, ad: &str| format!("{p}@{ad}#{tag}");
        let magic = |p: &str, ad: &str| format!("magic@{p}@{ad}#{tag}");
        // The seed: this atom's bound arguments are demanded.
        added.push(Rule {
            head: Atom { pred: magic(&a.pred, &adorn), args: bound_args(&a.args, &adorn) },
            body: vars_guard(&a.args, &adorn, body, lit),
        });
        let mut work = vec![(a.pred.clone(), adorn.clone())];
        while let Some((p, ad)) = work.pop() {
            if done.contains_key(&(p.clone(), ad.clone())) {
                continue;
            }
            done.insert((p.clone(), ad.clone()), name(&p, &ad));
            for r in program.rules.iter().filter(|r| r.head.pred == p) {
                let guard = Literal::Pos(Atom { pred: magic(&p, &ad), args: bound_args(&r.head.args, &ad) });
                let mut bound_vars = HashSet::new();
                for (t, c) in r.head.args.iter().zip(ad.chars()) {
                    if c == 'b' {
                        add_vars(std::slice::from_ref(t), &mut bound_vars);
                    }
                }
                let mut new_body = vec![guard.clone()];
                for l in &r.body {
                    match l {
                        Literal::Pos(b) if defined.contains(&b.pred) => {
                            let bad = adornment(&b.args, &bound_vars);
                            // Demand flows to the callee: what's bound when the body reaches it.
                            added.push(Rule {
                                head: Atom { pred: magic(&b.pred, &bad), args: bound_args(&b.args, &bad) },
                                body: new_body.clone(),
                            });
                            work.push((b.pred.clone(), bad.clone()));
                            new_body.push(Literal::Pos(Atom { pred: name(&b.pred, &bad), args: b.args.clone() }));
                            add_vars(&b.args, &mut bound_vars);
                        }
                        Literal::Pos(b) => {
                            new_body.push(l.clone());
                            add_vars(&b.args, &mut bound_vars);
                        }
                        _ => new_body.push(l.clone()),
                    }
                }
                added.push(Rule { head: Atom { pred: name(&p, &ad), args: r.head.args.clone() }, body: new_body });
            }
        }
        out.push(Literal::Pos(Atom { pred: name(&a.pred, &adorn), args: a.args.clone() }));
        add_vars(&a.args, &mut vars);
    }
    program.rules.extend(added);
    out
}

/// The body that seeds a magic predicate: the literals before `lit` that bind its variables. A query's
/// atoms bound only by constants and parameters seed with an empty body, a fact.
fn vars_guard(args: &[Term], adorn: &str, body: &[Literal], lit: &Literal) -> Vec<Literal> {
    let needs_vars = bound_args(args, adorn).iter().any(|t| matches!(t, Term::Var(_)));
    if !needs_vars {
        return vec![];
    }
    body.iter().take_while(|l| *l != lit).cloned().collect()
}
