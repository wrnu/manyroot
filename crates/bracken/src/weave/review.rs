//! `weave green`, `weave review`, and what `integrate` records of answers, breakage and goals
//! (docs/design/weave.md, sections 2 and 3; build step 7).
//!
//! Every view here reads the library's: green and the counted results from
//! [`Checks`], reviews from [`Reviews`], and open objections from [`Lines`], so
//! the commands judge exactly as qualification does.

use std::collections::HashSet;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use bracken_tree::format::{Answer, Breakage, Decision, Disposition, GOAL_TAG};
use bracken_tree::ground::{Index, Review};
use bracken_tree::lines::Lines;
use bracken_tree::objections::{Basis, Open};
use bracken_tree::results::{Checks, Counted, Green};
use bracken_tree::reviews::{Reviews, Target};
use clap::Args;
use manyroot::{Body, ContributionId, Store};
use serde_json::{Value, json};

use super::merge::require_marker;
use super::resolve_line;
use crate::terminal::printable;
use crate::{Names, Output, print_json, write_stdout};

/// What a decision `integrate` writes says of open objections, known breakage and goals.
#[derive(Args, Debug, Default, Clone)]
pub(crate) struct DecideArgs {
    /// Answer an objection: OBJECTION=DISPOSITION:REASON, the disposition one of fixed, fell_back,
    /// accepted_breakage or overridden. OBJECTION may instead name its target (a decision, a manifest
    /// artifact or a proposal), answering every objection to it that the disposition is allowed for, or
    /// the decision's own line, answering every objection in its window that the disposition is allowed
    /// for. Repeat for several.
    #[arg(long = "answer", value_name = "OBJECTION=DISPOSITION:REASON")]
    answers: Vec<String>,
    /// Known breakage the decision accepts: CHECK, or CHECK:PATH,PATH... naming the paths it affects.
    /// The line's guardrails govern it. Repeat for several.
    #[arg(long = "breakage", value_name = "CHECK[:PATHS]")]
    breakage: Vec<String>,
    /// A goal the decision serves: a claim tagged weave-goal, by id. Repeat for several.
    #[arg(long = "goal", value_name = "GOAL")]
    goals: Vec<String>,
}

fn disposition(given: &str) -> Result<Disposition> {
    use Disposition::*;
    [Fixed, FellBack, AcceptedBreakage, Overridden].into_iter().find(|d| d.as_str() == given).with_context(|| {
        format!("{given:?} isn't a disposition: one of fixed, fell_back, accepted_breakage, overridden")
    })
}

impl DecideArgs {
    /// Fills in a decision's answers, breakage and goals from the flags, resolving each id. An answer must
    /// name an objection (a `contradicts` relation), what objections target (a decision, a manifest
    /// artifact or a proposal; finding 889d899a57b8), or the decision's own line. Nothing is written
    /// before this.
    pub(crate) fn apply(&self, s: &Store, ix: &Index, lines: &Lines, d: &mut Decision) -> Result<()> {
        for given in &self.answers {
            let (objection, rest) = given
                .split_once('=')
                .with_context(|| format!("--answer {given:?}: expected OBJECTION=DISPOSITION:REASON"))?;
            let (how, reason) = rest
                .split_once(':')
                .with_context(|| format!("--answer {given:?}: expected OBJECTION=DISPOSITION:REASON"))?;
            let objection = s.graph().resolve_prefix(objection).with_context(|| format!("--answer: {objection:?}"))?;
            if reason.trim().is_empty() {
                bail!("--answer {given:?}: the reason can't be blank");
            }
            let is_objection = ix.reviews().iter().any(|r| r.id == objection && r.objection);
            // The decision's own line answers every objection in its window (decision 4afee4855944).
            if !is_objection && objection != d.line && Target::of(ix, lines, &objection).is_none() {
                let what = s.graph().get(&objection).map(bracken_tree::ground::kind_of).unwrap_or_default();
                bail!(
                    "--answer: {} is a {what}, neither an objection nor a decision, manifest artifact or proposal \
                     objections target; nothing was written",
                    objection.short()
                );
            }
            d.answers.push(Answer { objection, disposition: disposition(how)?, reason: reason.to_owned() });
        }
        for given in &self.breakage {
            let (check, paths) = match given.split_once(':') {
                Some((check, paths)) => {
                    (check, paths.split(',').filter(|p| !p.is_empty()).map(str::to_owned).collect())
                }
                None => (given.as_str(), vec![]),
            };
            if check.trim().is_empty() {
                bail!("--breakage {given:?}: name the check");
            }
            d.breakage.push(Breakage { check: check.to_owned(), detail: String::new(), paths });
        }
        for given in &self.goals {
            let id = s.graph().resolve_prefix(given).with_context(|| format!("--goal: {given:?}"))?;
            let goal = matches!(s.graph().get(&id).map(|c| c.body()), Some(Body::Claim { tags, .. }) if tags.iter().any(|t| t == GOAL_TAG));
            if !goal {
                bail!("--goal: {} isn't a claim tagged {GOAL_TAG}", id.short());
            }
            d.goals.push(id);
        }
        Ok(())
    }
}

#[derive(Args)]
pub(crate) struct GreenArgs {
    /// A line's id or name. Defaults to the default line.
    #[arg(long, value_name = "LINE")]
    line: Option<String>,
    #[command(flatten)]
    out: Output,
}

#[derive(Args)]
pub(crate) struct ReviewArgs {
    /// A line's id or name. Defaults to the default line.
    #[arg(long, value_name = "LINE")]
    line: Option<String>,
    /// Only this proposal: its reviews, carried over by change, and what a decision using it unchanged
    /// would have to answer.
    #[arg(long, value_name = "PROPOSAL")]
    proposal: Option<String>,
    #[command(flatten)]
    out: Output,
}

fn counted_json(c: &Counted, checks: &Checks, names: &Names) -> Value {
    let result = |id: &ContributionId| {
        let r = checks.result(id).expect("counted from the results");
        json!({"result": id, "author": r.author.to_string(), "author_name": names.display(&r.author),
               "attempt": r.attempt, "passed": r.passed})
    };
    json!({"check": c.check, "method": c.method, "green": c.green(),
           "passes": c.passes.iter().map(result).collect::<Vec<_>>(),
           "fails": c.fails.iter().map(result).collect::<Vec<_>>(),
           "replaced": c.replaced.iter().map(result).collect::<Vec<_>>()})
}

fn counted_text(c: &Counted, checks: &Checks, names: &Names) -> String {
    let list = |ids: &[ContributionId]| -> String {
        ids.iter()
            .map(|id| {
                let r = checks.result(id).expect("counted from the results");
                let attempt = r.attempt.map_or(String::new(), |a| format!(", attempt {a}"));
                format!("{} ({}{attempt})", id.short(), printable(&names.display(&r.author)))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!("  check {}: {}", printable(&c.check), if c.green() { "green" } else { "not green" });
    if !c.passes.is_empty() {
        text.push_str(&format!("; passes {}", list(&c.passes)));
    }
    if !c.fails.is_empty() {
        text.push_str(&format!("; fails {}", list(&c.fails)));
    }
    if !c.replaced.is_empty() {
        text.push_str(&format!("; fails a later attempt replaced {}", list(&c.replaced)));
    }
    text
}

/// `weave green`: the line's green under its current policy, and per head, each check's counted
/// results, with the fails a later attempt replaced.
pub(crate) fn green(s: &Store, args: GreenArgs) -> Result<ExitCode> {
    let g = s.graph();
    let ix = Index::build(g);
    require_marker(&ix)?;
    let lines = Lines::build(g, &ix);
    let line = resolve_line(s, &lines, args.line.as_deref(), "--line")?;
    let checks = Checks::build(g, &ix, &lines);
    let names = Names::load(s)?;
    let green = checks.green(g, &lines, &line);
    let (state, decisions): (&str, Vec<ContributionId>) = match &green {
        Green::Undefined(versions) => ("undefined", versions.clone()),
        Green::None => ("none", vec![]),
        Green::One(d) => ("one", vec![*d]),
        Green::Ambiguous(ds) => ("ambiguous", ds.clone()),
    };
    let heads = lines.heads(&line);
    let per_head: Vec<(ContributionId, Option<Vec<Counted>>)> =
        heads.iter().map(|h| (*h, checks.counted(g, &lines, h))).collect();
    if args.out.json {
        let heads: Vec<Value> = per_head
            .iter()
            .map(|(h, counted)| {
                let manifest = lines.decision(h).expect("a head").decision.manifest.to_string();
                let checks: Option<Vec<Value>> =
                    counted.as_ref().map(|cs| cs.iter().map(|c| counted_json(c, &checks, &names)).collect());
                json!({"decision": h, "manifest": manifest, "checks": checks})
            })
            .collect();
        let (green, versions) = match green {
            Green::Undefined(_) => (vec![], decisions),
            _ => (decisions, vec![]),
        };
        print_json(
            &json!({"line": line, "state": state, "green": green, "diverged_versions": versions, "heads": heads}),
        );
        return Ok(ExitCode::SUCCESS);
    }
    let ids = |ds: &[ContributionId]| ds.iter().map(ContributionId::short).collect::<Vec<_>>().join(" ");
    match &green {
        Green::Undefined(versions) => println!(
            "green undefined: the line's policy has diverged into versions {}; the line's author settles them",
            ids(versions)
        ),
        Green::None => println!("no green decision yet"),
        Green::One(d) => println!("green {d}"),
        Green::Ambiguous(ds) => println!("green ambiguous: {}", ids(ds)),
    }
    for (h, counted) in &per_head {
        println!("head {h} manifest {}", lines.decision(h).expect("a head").decision.manifest);
        for c in counted.iter().flatten() {
            println!("{}", counted_text(c, &checks, &names));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn target_json(t: &Target) -> Value {
    match t {
        Target::Decision(d) => json!({"decision": d}),
        Target::Manifest(id, hash) => json!({"manifest_artifact": id, "manifest": hash.to_string()}),
        Target::Proposal { id, line, change } => json!({"proposal": id, "line": line, "change": change.to_string()}),
    }
}

fn target_text(t: &Target) -> String {
    match t {
        Target::Decision(d) => format!("decision {}", d.short()),
        Target::Manifest(id, hash) => format!("manifest {hash} (artifact {})", id.short()),
        Target::Proposal { id, .. } => format!("proposal {}", id.short()),
    }
}

fn review_json(r: &Review, t: &Target, names: &Names) -> Value {
    json!({"review": r.id, "kind": if r.objection { "objection" } else { "agreement" },
           "author": r.author.to_string(), "author_name": names.display(&r.author), "from": r.from,
           "target": target_json(t), "note": r.note})
}

fn review_text(r: &Review, t: &Target, names: &Names) -> String {
    let kind = if r.objection { "objection" } else { "agreement" };
    format!(
        "  review {} {kind} by {} to {}: {}",
        r.id,
        printable(&names.display(&r.author)),
        target_text(t),
        printable(&r.note)
    )
}

fn basis_json(b: &Basis) -> Value {
    match b {
        Basis::Evidence { result, check } => json!({"kind": b.kind(), "result": result, "check": check}),
        Basis::Goal(goal) => json!({"kind": b.kind(), "goal": goal}),
        Basis::Discussion => json!({"kind": b.kind()}),
    }
}

fn open_json(o: &Open, names: &Names) -> Value {
    json!({"objection": o.objection.id, "author": o.objection.author.to_string(),
           "author_name": names.display(&o.objection.author), "from": o.objection.from,
           "target": target_json(&o.objection.target), "basis": basis_json(&o.basis),
           "allowed": o.allowed.iter().map(Disposition::as_str).collect::<Vec<_>>()})
}

fn open_text(o: &Open, names: &Names) -> String {
    let basis = match &o.basis {
        Basis::Evidence { result, check } => format!("evidence-backed: {} fails {}", result.short(), printable(check)),
        Basis::Goal(goal) => format!("goal-backed: goal {}", goal.short()),
        Basis::Discussion => "discussion".to_owned(),
    };
    let allowed: Vec<&str> = o.allowed.iter().map(Disposition::as_str).collect();
    format!(
        "  open {} by {} to {} ({basis}); allowed: {}; answer with --answer {}=<disposition>:<reason>",
        o.objection.id,
        printable(&names.display(&o.objection.author)),
        target_text(&o.objection.target),
        allowed.join(", "),
        o.objection.id.short()
    )
}

/// `weave review`: the reviews of the line's heads and the objections the next move must answer; and the
/// reviews of each open proposal on the line, with what a decision using it unchanged would have to
/// answer. With --proposal, that proposal alone.
pub(crate) fn review(s: &Store, args: ReviewArgs) -> Result<ExitCode> {
    let g = s.graph();
    let ix = Index::build(g);
    require_marker(&ix)?;
    let lines = Lines::build(g, &ix);
    let reviews = Reviews::build(&ix, &lines);
    let names = Names::load(s)?;
    let proposal_view = |p: &ContributionId| -> (Vec<(&Review, Target)>, Vec<Open>) {
        let open = lines.proposal_objections(g, &ix, p).unwrap_or_default();
        (reviews.of_proposal(&lines, p), open)
    };
    let proposals: Vec<ContributionId>;
    let mut heads: Vec<(ContributionId, Vec<(&Review, Target)>)> = Vec::new();
    let mut open: Option<Vec<Open>> = None;
    let line;
    if let Some(given) = &args.proposal {
        let id = g.resolve_prefix(given).with_context(|| format!("--proposal: {given:?}"))?;
        if lines.proposal_named(&id).is_none() {
            bail!("--proposal: {} isn't a proposal", id.short());
        }
        line = lines.proposal_named(&id).expect("a proposal").line;
        proposals = vec![id];
    } else {
        line = resolve_line(s, &lines, args.line.as_deref(), "--line")?;
        heads = lines.heads(&line).into_iter().map(|h| (h, reviews.of_decision(&lines, &h))).collect();
        open = lines.open_objections(g, &ix, &line);
        // Open proposals: valid ones on the line whose change no qualifying decision on it lists.
        let integrated: HashSet<_> = lines
            .log(g, &line, None)
            .iter()
            .flat_map(|d| d.decision.proposals.iter().filter_map(|p| lines.proposal_named(p)).map(|p| p.change))
            .collect();
        proposals =
            lines.proposals_on(&line).filter(|p| !integrated.contains(&p.proposal.change)).map(|p| p.id).collect();
    }
    type ProposalView<'r> = (ContributionId, Vec<(&'r Review, Target)>, Vec<Open>);
    let per_proposal: Vec<ProposalView> = proposals
        .iter()
        .map(|p| {
            let (r, o) = proposal_view(p);
            (*p, r, o)
        })
        .collect();
    if args.out.json {
        let heads: Vec<Value> = heads
            .iter()
            .map(|(h, rs)| json!({"decision": h, "reviews": rs.iter().map(|(r, t)| review_json(r, t, &names)).collect::<Vec<_>>()}))
            .collect();
        let proposals: Vec<Value> = per_proposal
            .iter()
            .map(|(p, rs, os)| {
                json!({"proposal": p,
                       "reviews": rs.iter().map(|(r, t)| review_json(r, t, &names)).collect::<Vec<_>>(),
                       "must_answer": os.iter().map(|o| open_json(o, &names)).collect::<Vec<_>>()})
            })
            .collect();
        let open: Option<Vec<Value>> = open.as_ref().map(|os| os.iter().map(|o| open_json(o, &names)).collect());
        print_json(&json!({"line": line, "heads": heads, "open": open, "proposals": proposals}));
        return Ok(ExitCode::SUCCESS);
    }
    for (h, rs) in &heads {
        println!("head {h}");
        for (r, t) in rs {
            println!("{}", review_text(r, t, &names));
        }
    }
    if args.proposal.is_none() {
        match &open {
            None => println!("open objections undefined: the line's policy has diverged"),
            Some(os) if os.is_empty() => println!("no open objections"),
            Some(os) => {
                println!("open objections the next move must answer: {}", os.len());
                for o in os {
                    println!("{}", open_text(o, &names));
                }
            }
        }
    }
    for (p, rs, os) in &per_proposal {
        println!("proposal {p}");
        for (r, t) in rs {
            println!("{}", review_text(r, t, &names));
        }
        for o in os {
            println!("{}", open_text(o, &names));
        }
    }
    Ok(ExitCode::SUCCESS)
}
