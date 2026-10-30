//! Objections and the guardrails: qualification rules 5 and 6, and open objections
//! (docs/design/weave.md, section 1 *Goals, checks and objections*, and section 2).
//!
//! An objection is a `contradicts` relation whose target resolves ([`Target::of`])
//! to a decision, a manifest or a proposal, retracted or not. Its basis comes from
//! its `from` end, classified under one policy over one causal past: a failing
//! check result eligible there (evidence-backed), a goal that binds the line there
//! (goal-backed), or anything else (discussion).
//!
//! Every rule judging a decision `E` reads only `E`'s causal past, under the policy
//! in force for it (finding b2098df8e4be). Which objections, retractions, results
//! and goal links lie in that past is asked of the ancestor index, once per decision,
//! before any is judged; the decision graph, which needs no
//! Ground walk, gives each decision's window and the answers below it. Retraction
//! follows the protocol's rule within the past ([`Retractions::within`]), and a
//! target resolves to what it names whether or not it is retracted (gotcha
//! b4d45c7352d4).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use manyroot::{AgentId, ContributionId};

use crate::format::{Change, ChangeEntry, Decision, Disposition, Line, parse_method};
use crate::ground::{GoalLink, Index, Retractions};
use crate::hash::ContentHash;
use crate::lines::{Lines, Unqualified};
use crate::results::CheckResult;
use crate::reviews::Target;

/// What backs an objection, under one policy over one causal past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    /// `from` is a failing check result eligible there, for `check`.
    Evidence {
        result: ContributionId,
        check: String,
    },
    /// `from` is a goal that binds the line there.
    Goal(ContributionId),
    Discussion,
}

impl Basis {
    pub fn kind(&self) -> &'static str {
        match self {
            Basis::Evidence { .. } => "evidence-backed",
            Basis::Goal(_) => "goal-backed",
            Basis::Discussion => "discussion",
        }
    }

    /// The dispositions this basis allows, before checking them against the answering decision: for an
    /// objection to a decision in the window, or (with `uses`) to a manifest or proposal the decision
    /// uses unchanged, which only `accepted_breakage` and `overridden` can answer.
    pub fn allows(&self, uses: bool) -> Vec<Disposition> {
        let mut allowed = if uses { vec![] } else { vec![Disposition::Fixed, Disposition::FellBack] };
        match self {
            Basis::Evidence { .. } => allowed.push(Disposition::AcceptedBreakage),
            Basis::Goal(_) => {}
            Basis::Discussion => allowed.push(Disposition::Overridden),
        }
        allowed
    }
}

/// An objection: a `contradicts` relation to a decision, a manifest or a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Objection {
    pub id: ContributionId,
    pub author: AgentId,
    pub from: ContributionId,
    pub target: Target,
    /// The objected manifest: the decision's, the manifest artifact's, or the proposal's.
    pub objected: ContentHash,
}

/// An objection the next move on a line, or a decision using a proposal unchanged, has to answer, under
/// the line's current policy over the whole replica, with the dispositions its basis allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub objection: Objection,
    pub basis: Basis,
    pub allowed: Vec<Disposition>,
}

/// One objection's index in [`Obligations::list`].
type At = u32;

/// Answers at or below a decision: (objection, answering decision), ascending.
type Answered = Arc<[(ContributionId, ContributionId)]>;

/// What rules 5 and 6 read of one decision: its window, the objections to what it uses, the answers at
/// or below it, and its breakage entries' ages.
#[derive(Debug, Clone)]
pub(crate) struct Reads {
    pub(crate) window: Arc<[At]>,
    pub(crate) uses: Vec<At>,
    pub(crate) answered: Answered,
    pub(crate) ages: BTreeMap<String, u64>,
}

/// Every objection in a replica, what rule 5 asks of each decision, and what rule 6 counts: each
/// decision's window (objections to its strict decision-ancestors), the answers at or below it, the
/// objections to what it uses, and its breakage entries' ages.
#[derive(Debug, Default)]
pub struct Obligations {
    list: Vec<Objection>,
    to_decision: HashMap<ContributionId, Vec<At>>,
    to_manifest: HashMap<ContentHash, Vec<At>>,
    to_proposal: Vec<At>,
    /// Each objected proposal's change entries, if its base's manifest and its own resolve.
    changes: HashMap<ContributionId, Option<Vec<ChangeEntry>>>,
    /// The results an objection comes from, by id.
    results: HashMap<ContributionId, CheckResult>,
    /// The goal links of each goal claim an objection comes from.
    links: HashMap<ContributionId, Vec<GoalLink>>,
    /// Each decision's window, ascending: objections to its strict ancestors in the decision graph.
    window: HashMap<ContributionId, Arc<[At]>>,
    /// Each decision's answers at or below it in the decision graph: (objection, answering decision),
    /// ascending.
    answered: HashMap<ContributionId, Answered>,
    /// The objections to what each decision uses: its manifest, and the proposals it uses.
    uses: HashMap<ContributionId, Vec<At>>,
    /// Each decision's breakage entries' ages, by check, where it has any.
    ages: HashMap<ContributionId, BTreeMap<String, u64>>,
}

/// Whether `path` is `prefix` or below it.
fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    !prefix.is_empty() && (path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/')))
}

impl Obligations {
    /// Every objection whose target resolves, with the results and goal links its basis may need, then
    /// each decision's window, answers, used objections and breakage ages, in `lines`' causal order.
    /// Reads only payloads and the decision graph: no ancestor question.
    pub(crate) fn build(ix: &Index, lines: &Lines, order: &[ContributionId]) -> Obligations {
        let mut ob = Obligations::default();
        let results: HashMap<ContributionId, &CheckResult> = ix.check_results().iter().map(|r| (r.id, r)).collect();
        let mut links: HashMap<ContributionId, Vec<GoalLink>> = HashMap::new();
        for link in ix.goal_links() {
            links.entry(link.goal).or_default().push(link.clone());
        }
        for review in ix.reviews().iter().filter(|r| r.objection) {
            let Some(target) = Target::of(ix, lines, &review.to) else { continue };
            let objected = match target {
                Target::Decision(d) => lines.decision(&d).expect("resolved").decision.manifest,
                Target::Manifest(_, hash) => hash,
                Target::Proposal { id, .. } => lines.proposal_named(&id).expect("resolved").manifest,
            };
            let at = ob.list.len() as At;
            match target {
                Target::Decision(d) => ob.to_decision.entry(d).or_default().push(at),
                Target::Manifest(_, hash) => ob.to_manifest.entry(hash).or_default().push(at),
                Target::Proposal { id, .. } => {
                    ob.to_proposal.push(at);
                    ob.changes.entry(id).or_insert_with(|| {
                        let p = lines.proposal_named(&id).expect("resolved");
                        let base = lines.decision(&p.base).and_then(|b| ix.manifest(&b.decision.manifest));
                        match (base, ix.manifest(&p.manifest)) {
                            (Some((_, base)), Some((_, new))) => Some(Change::between(base, new).entries().to_vec()),
                            _ => None,
                        }
                    });
                }
            }
            if let Some(r) = results.get(&review.from) {
                ob.results.insert(r.id, (*r).clone());
            }
            if let Some(l) = links.get(&review.from) {
                ob.links.insert(review.from, l.clone());
            }
            ob.list.push(Objection { id: review.id, author: review.author, from: review.from, target, objected });
        }
        for id in order {
            let d = &lines.decision(id).expect("in order").decision;
            let window = ob.window_for(&d.parents);
            let answered = ob.answered_for(&d.parents, Some((id, d)));
            let uses = ob.uses_for(ix, lines, d);
            let ages = ob.ages_for(lines, d);
            ob.window.insert(*id, window);
            ob.answered.insert(*id, answered);
            if !uses.is_empty() {
                ob.uses.insert(*id, uses);
            }
            if !ages.is_empty() {
                ob.ages.insert(*id, ages);
            }
        }
        ob
    }

    /// Every objection whose target resolves, in id order.
    pub fn list(&self) -> &[Objection] {
        &self.list
    }

    /// A decision's window with `parents`: the objections to its strict decision-ancestors, ascending.
    /// A decision with one parent and no objection to it shares its parent's.
    fn window_for(&self, parents: &[ContributionId]) -> Arc<[At]> {
        let empty = Arc::<[At]>::from([]);
        let parts = |p: &ContributionId| {
            (self.window.get(p).cloned().unwrap_or_else(|| empty.clone()), self.to_decision.get(p))
        };
        match parents {
            [] => empty,
            [p] if !self.to_decision.contains_key(p) => parts(p).0,
            _ => {
                let mut all: BTreeSet<At> = BTreeSet::new();
                for p in parents {
                    let (below, to) = parts(p);
                    all.extend(below.iter().copied());
                    all.extend(to.into_iter().flatten().copied());
                }
                all.into_iter().collect::<Vec<_>>().into()
            }
        }
    }

    /// A decision's answers at or below it, given its parents, and itself if it is in the graph.
    fn answered_for(&self, parents: &[ContributionId], own: Option<(&ContributionId, &Decision)>) -> Answered {
        let empty = Answered::from([]);
        let mine: Vec<(ContributionId, ContributionId)> =
            own.map(|(id, d)| d.answers.iter().map(|a| (a.objection, *id)).collect()).unwrap_or_default();
        match parents {
            [] if mine.is_empty() => empty,
            [p] if mine.is_empty() => self.answered.get(p).cloned().unwrap_or(empty),
            _ => {
                let mut all: BTreeSet<(ContributionId, ContributionId)> = mine.into_iter().collect();
                for p in parents {
                    all.extend(self.answered.get(p).into_iter().flat_map(|a| a.iter().copied()));
                }
                all.into_iter().collect::<Vec<_>>().into()
            }
        }
    }

    /// The objections to what a decision uses unchanged (rule 5): to a manifest artifact with its
    /// manifest hash, or to a proposal `X` it uses, by listing `X` or a proposal with `X`'s line and
    /// change, or because every entry of `X`'s change is in its own change against one of its parents on
    /// its line (security note 17906a29698f). Ascending.
    fn uses_for(&self, ix: &Index, lines: &Lines, d: &Decision) -> Vec<At> {
        let mut uses: Vec<At> = self.to_manifest.get(&d.manifest).cloned().unwrap_or_default();
        if !self.to_proposal.is_empty() {
            let listed: HashSet<(ContributionId, ContentHash)> =
                d.proposals.iter().filter_map(|p| lines.proposal_named(p)).map(|p| (p.line, p.change)).collect();
            // Its own change against each of its parents on its line, in path order.
            let own: Vec<Change> = match ix.manifest(&d.manifest) {
                Some((_, mine)) => d
                    .parents
                    .iter()
                    .filter_map(|p| lines.decision(p))
                    .filter(|p| p.decision.line == d.line)
                    .filter_map(|p| ix.manifest(&p.decision.manifest))
                    .map(|(_, theirs)| Change::between(theirs, mine))
                    .collect(),
                None => vec![],
            };
            for at in &self.to_proposal {
                let Target::Proposal { id, line, change } = self.list[*at as usize].target else { continue };
                let covers = |mine: &Change, e: &ChangeEntry| {
                    let at = mine.entries().binary_search_by(|m| m.path.as_bytes().cmp(e.path.as_bytes()));
                    at.is_ok_and(|at| mine.entries()[at] == *e)
                };
                let used = listed.contains(&(line, change))
                    || self
                        .changes
                        .get(&id)
                        .and_then(Option::as_ref)
                        .is_some_and(|x| !x.is_empty() && own.iter().any(|mine| x.iter().all(|e| covers(mine, e))));
                if used {
                    uses.push(*at);
                }
            }
            uses.sort_unstable();
            uses.dedup();
        }
        uses
    }

    /// Each breakage entry's age (rule 6): the number of decisions in the longest unbroken chain of
    /// ancestors on the line, through parents on it, ending at this one, that list the same check.
    fn ages_for(&self, lines: &Lines, d: &Decision) -> BTreeMap<String, u64> {
        d.breakage
            .iter()
            .map(|b| {
                let below = d
                    .parents
                    .iter()
                    .filter(|p| lines.decision(p).is_some_and(|p| p.decision.line == d.line))
                    .filter_map(|p| self.ages.get(p).and_then(|ages| ages.get(&b.check)))
                    .max()
                    .copied()
                    .unwrap_or(0);
                (b.check.clone(), below.saturating_add(1))
            })
            .collect()
    }

    /// The contributions rule 5 asks about for decision `id` on `line`: whether each lies in its causal
    /// past (pushed to `past`), and the results whose epoch it needs under its policy (pushed to
    /// `epoch`): each objection in its window or to what it uses, the retractions below it, and its
    /// basis's result or goal links, with the retractions below them.
    pub(crate) fn asks(
        &self,
        id: &ContributionId,
        line: &ContributionId,
        retractions: &Retractions,
        past: &mut Vec<ContributionId>,
        epoch: &mut Vec<ContributionId>,
    ) {
        let window = self.window.get(id).map(|w| &w[..]).unwrap_or(&[]);
        let uses = self.uses.get(id).map(Vec::as_slice).unwrap_or(&[]);
        for at in window.iter().chain(uses) {
            self.ask_one(*at, line, retractions, past, epoch);
        }
    }

    fn ask_one(
        &self,
        at: At,
        line: &ContributionId,
        retractions: &Retractions,
        past: &mut Vec<ContributionId>,
        epoch: &mut Vec<ContributionId>,
    ) {
        let o = &self.list[at as usize];
        past.push(o.id);
        past.extend(retractions.below(&o.id));
        if self.results.contains_key(&o.from) {
            past.push(o.from);
            past.extend(retractions.below(&o.from));
            epoch.push(o.from);
        }
        if let Some(links) = self.links.get(&o.from) {
            past.extend(retractions.below(&o.from));
            for link in links.iter().filter(|l| l.to == *line) {
                past.push(link.id);
                past.extend(retractions.below(&link.id));
            }
        }
    }

    /// An objection's basis under `policy` for decisions on `line`, over the past `in_past` holds, with
    /// results eligible for a decision by `eligible_for` (none excluded if `None`). `in_epoch` says
    /// whether the policy version is an ancestor of a result.
    #[allow(clippy::too_many_arguments)]
    fn basis(
        &self,
        o: &Objection,
        policy: &Line,
        line: &ContributionId,
        eligible_for: Option<&AgentId>,
        retractions: &Retractions,
        in_past: &mut dyn FnMut(&ContributionId) -> bool,
        in_epoch: &mut dyn FnMut(&ContributionId) -> bool,
    ) -> Basis {
        if let Some(r) = self.results.get(&o.from) {
            let check = parse_method(&r.method).map(|(name, _)| name.to_owned());
            let eligible = !r.passed
                && check.as_ref().is_some_and(|name| policy.check_method(name).as_deref() == Some(&r.method))
                && policy.validators.contains(&r.author)
                && eligible_for != Some(&r.author)
                && r.manifest == o.objected
                && in_past(&r.id)
                && !retractions.within(&r.id, &mut *in_past)
                && in_epoch(&r.id);
            if eligible {
                return Basis::Evidence { result: r.id, check: check.expect("tested above") };
            }
        }
        if let Some(links) = self.links.get(&o.from) {
            let binds = |link: &GoalLink| {
                policy.goal_setters.contains(&link.author) && policy.goal_setters.contains(&link.linker)
            };
            let mut candidates = links.iter().filter(|l| l.to == *line && binds(l));
            if candidates.any(|link| in_past(&link.id) && !link.retracted_within(retractions, &mut *in_past)) {
                return Basis::Goal(o.from);
            }
        }
        Basis::Discussion
    }

    /// Whether `a`, a decision answering `o` with `disposition`, is allowed for `basis` (rule 5's table).
    /// For an objection to a decision `D`, `fixed` needs `a`'s manifest to differ from `D`'s and, if
    /// evidence-backed, the failing check not in `a`'s breakage; `fell_back` needs `a`'s manifest to be a
    /// qualifying strict ancestor of `D`'s.
    fn allowed(
        &self,
        lines: &Lines,
        o: &Objection,
        basis: &Basis,
        uses: bool,
        a: &Decision,
        disposition: Disposition,
    ) -> bool {
        if !basis.allows(uses).contains(&disposition) {
            return false;
        }
        let lists = |check: &str| a.breakage.iter().any(|b| b.check == check);
        match (disposition, basis) {
            (Disposition::AcceptedBreakage, Basis::Evidence { check, .. }) => lists(check),
            (Disposition::Overridden, _) => true,
            (Disposition::Fixed, _) => {
                a.manifest != o.objected && !matches!(basis, Basis::Evidence { check, .. } if lists(check))
            }
            (Disposition::FellBack, _) => {
                let Target::Decision(d) = o.target else { return false };
                let mut below = lines.decision_ancestry(&d);
                below.remove(&d);
                below.iter().any(|b| {
                    matches!(lines.verdict(b), Some(Ok(_)))
                        && lines.decision(b).is_some_and(|b| b.decision.manifest == a.manifest)
                })
            }
            _ => false,
        }
    }

    /// Rule 5 for decision `e` (with id `id`, `None` if not yet written) by `author` on its line, under
    /// `policy`, over the past `in_past` holds: every objection in its window to a qualifying decision,
    /// and every objection to what it uses, in that past and not retracted there, has an allowed answer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rule_5(
        &self,
        lines: &Lines,
        retractions: &Retractions,
        id: Option<&ContributionId>,
        author: &AgentId,
        e: &Decision,
        policy: &Line,
        window: &[At],
        uses: &[At],
        answered: &[(ContributionId, ContributionId)],
        in_past: &mut dyn FnMut(&ContributionId) -> bool,
        in_epoch: &mut dyn FnMut(&ContributionId) -> bool,
    ) -> Result<(), Unqualified> {
        let answers_to = |o: &ContributionId| -> Vec<(Option<ContributionId>, &Decision)> {
            let mine = e.answers.iter().filter(|a| a.objection == *o).map(|_| (id.copied(), e));
            let start = answered.partition_point(|(x, _)| x < o);
            let below = answered[start..]
                .iter()
                .take_while(|(x, _)| x == o)
                .filter(|(_, a)| Some(a) != id)
                .filter_map(|(_, a)| lines.decision(a).map(|r| (Some(*a), &r.decision)));
            mine.chain(below).collect()
        };
        let disposition_of = |a: &Decision, o: &ContributionId| -> Vec<Disposition> {
            a.answers.iter().filter(|x| x.objection == *o).map(|x| x.disposition).collect()
        };
        let mut seen: HashSet<At> = HashSet::new();
        for (uses_it, at) in window.iter().map(|at| (false, at)).chain(uses.iter().map(|at| (true, at))) {
            if !seen.insert(*at) && uses_it {
                // An objection both in the window and to what `e` uses was judged in the window.
                continue;
            }
            let o = &self.list[*at as usize];
            if !in_past(&o.id) || retractions.within(&o.id, &mut *in_past) {
                continue;
            }
            let eligible_for = match (uses_it, o.target) {
                (false, Target::Decision(d)) => match (lines.verdict(&d), lines.decision(&d)) {
                    (Some(Ok(_)), Some(d)) => d.author,
                    _ => continue,
                },
                _ => *author,
            };
            let basis = self.basis(o, policy, &e.line, Some(&eligible_for), retractions, in_past, in_epoch);
            // Who may answer: in the window, `e` or a decision that the objection's target is a strict
            // ancestor of; for what `e` uses, `e` or an ancestor of it on its line.
            let may_answer = |a: &Option<ContributionId>, d: &Decision| match a {
                None => true,
                Some(a) if Some(a) == id => true,
                Some(a) => match uses_it {
                    false => self.window.get(a).is_some_and(|w| w.binary_search(at).is_ok()),
                    true => d.line == e.line,
                },
            };
            let answers: Vec<(Option<ContributionId>, &Decision)> =
                answers_to(&o.id).into_iter().filter(|(a, d)| may_answer(a, d)).collect();
            let Some((_, first)) = answers.first() else {
                return Err(Unqualified::Unanswered { objection: o.id, basis: basis.kind() });
            };
            let ok = answers.iter().any(|(_, a)| {
                disposition_of(a, &o.id).into_iter().any(|disp| self.allowed(lines, o, &basis, uses_it, a, disp))
            });
            if !ok {
                let disposition = disposition_of(first, &o.id)[0];
                return Err(Unqualified::AnswerNotAllowed { objection: o.id, disposition, basis: basis.kind() });
            }
        }
        Ok(())
    }

    /// Rule 6 for decision `e` under `policy`, given its breakage entries' ages: no entry names a
    /// protected path; and at most `max_known` entries, none older than `max_age` line moves, unless it
    /// has a parent on its line and its breakage is a strict subset of every such parent's.
    pub(crate) fn rule_6(
        &self,
        lines: &Lines,
        e: &Decision,
        policy: &Line,
        ages: &BTreeMap<String, u64>,
    ) -> Result<(), Unqualified> {
        let rails = &policy.guardrails;
        for b in &e.breakage {
            if let Some(path) = b.paths.iter().find(|p| rails.protected.iter().any(|pre| under(p, pre))) {
                return Err(Unqualified::ProtectedPath { check: b.check.clone(), path: path.clone() });
            }
        }
        let over = if e.breakage.len() as u64 > rails.max_known {
            Some(Unqualified::TooMuchBreakage { count: e.breakage.len(), max: rails.max_known })
        } else {
            ages.iter().find(|(_, age)| **age > rails.max_age).map(|(check, age)| Unqualified::BreakageTooOld {
                check: check.clone(),
                age: *age,
                max: rails.max_age,
            })
        };
        let Some(over) = over else { return Ok(()) };
        let mine: BTreeSet<&str> = e.breakage.iter().map(|b| b.check.as_str()).collect();
        let on_line: Vec<&Decision> = e
            .parents
            .iter()
            .filter_map(|p| lines.decision(p))
            .map(|p| &p.decision)
            .filter(|p| p.line == e.line)
            .collect();
        let reducing = !on_line.is_empty()
            && on_line.iter().all(|p| {
                let theirs: BTreeSet<&str> = p.breakage.iter().map(|b| b.check.as_str()).collect();
                mine.is_subset(&theirs) && mine.len() < theirs.len()
            });
        if reducing { Ok(()) } else { Err(over) }
    }

    /// What [`Obligations::rule_5`] and [`Obligations::rule_6`] read for decision `id`.
    pub(crate) fn of(&self, id: &ContributionId) -> Reads {
        Reads {
            window: self.window.get(id).cloned().unwrap_or_else(|| Arc::from([])),
            uses: self.uses.get(id).cloned().unwrap_or_default(),
            answered: self.answered.get(id).cloned().unwrap_or_else(|| Answered::from([])),
            ages: self.ages.get(id).cloned().unwrap_or_default(),
        }
    }

    /// The same for a decision not yet written.
    pub(crate) fn of_new(&self, ix: &Index, lines: &Lines, d: &Decision) -> Reads {
        let window = self.window_for(&d.parents);
        let answered = self.answered_for(&d.parents, None);
        Reads { window, uses: self.uses_for(ix, lines, d), answered, ages: self.ages_for(lines, d) }
    }

    /// The objections in a window, or to what a decision uses, that no answer allows, judged now: under
    /// `policy`, over the whole replica, with retraction read now. For [`Lines::open_objections`] and
    /// [`Lines::proposal_objections`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &self,
        lines: &Lines,
        retractions: &Retractions,
        line: &ContributionId,
        policy: &Line,
        candidates: &[(bool, At)],
        answered: &[(ContributionId, ContributionId)],
        eligible_for: Option<&AgentId>,
        in_epoch: &mut dyn FnMut(&ContributionId) -> bool,
    ) -> Vec<Open> {
        let mut open = Vec::new();
        for (uses_it, at) in candidates {
            let o = &self.list[*at as usize];
            let mut now = |_: &ContributionId| true;
            if retractions.within(&o.id, &mut now) {
                continue;
            }
            let eligible = match (uses_it, o.target) {
                (false, Target::Decision(d)) => match (lines.verdict(&d), lines.decision(&d)) {
                    (Some(Ok(_)), Some(d)) => Some(d.author),
                    _ => continue,
                },
                _ => eligible_for.copied(),
            };
            let basis = self.basis(o, policy, line, eligible.as_ref(), retractions, &mut now, in_epoch);
            let start = answered.partition_point(|(x, _)| *x < o.id);
            let answered_ok = answered[start..].iter().take_while(|(x, _)| *x == o.id).any(|(_, a)| {
                let may = match uses_it {
                    false => self.window.get(a).is_some_and(|w| w.binary_search(at).is_ok()),
                    true => lines.decision(a).is_some_and(|d| d.decision.line == *line),
                };
                let Some(d) = lines.decision(a) else { return false };
                may && d
                    .decision
                    .answers
                    .iter()
                    .filter(|x| x.objection == o.id)
                    .any(|x| self.allowed(lines, o, &basis, *uses_it, &d.decision, x.disposition))
            });
            if !answered_ok {
                let allowed = basis.allows(*uses_it);
                open.push(Open { objection: o.clone(), basis, allowed });
            }
        }
        open.sort_by_key(|o| o.objection.id);
        open
    }

    /// The results an objection in `candidates` comes from: what [`Obligations::open`] asks the epoch of.
    pub(crate) fn basis_results(&self, candidates: &[(bool, At)]) -> Vec<ContributionId> {
        let mut rs: Vec<ContributionId> = candidates
            .iter()
            .map(|(_, at)| self.list[*at as usize].from)
            .filter(|f| self.results.contains_key(f))
            .collect();
        rs.sort_unstable();
        rs.dedup();
        rs
    }

    /// The window and answers of the next move on a line whose heads are `heads`.
    pub(crate) fn next_move(&self, heads: &[ContributionId]) -> (Vec<(bool, At)>, Answered) {
        let window = self.window_for(heads);
        let answered = self.answered_for(heads, None);
        (window.iter().map(|at| (false, *at)).collect(), answered)
    }

    /// The objections to a manifest or proposal that a decision using `proposal` unchanged would have
    /// to answer: to any proposal with its line and change, and to its manifest.
    pub(crate) fn of_proposal(&self, lines: &Lines, proposal: &ContributionId) -> Vec<(bool, At)> {
        let Some(p) = lines.proposal_named(proposal) else { return vec![] };
        let mut ats: Vec<At> = self.to_manifest.get(&p.manifest).cloned().unwrap_or_default();
        ats.extend(self.to_proposal.iter().filter(|at| {
            matches!(self.list[**at as usize].target, Target::Proposal { line, change, .. } if line == p.line && change == p.change)
        }));
        ats.sort_unstable();
        ats.dedup();
        ats.into_iter().map(|at| (true, at)).collect()
    }
}
