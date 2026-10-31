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
use crate::shared::Set;

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

/// What an answer names: one objection, or every objection to a target (decision 8cf20fe7669a, B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Objection(ContributionId),
    Decision(ContributionId),
    Manifest(ContentHash),
    Change(ContributionId, ContentHash),
    /// The answering decision's own line: every objection in its window (decision 4afee4855944, 2).
    Line,
}

/// The objections settled by answers at a decision and its ancestors in the decision graph (decision
/// 8cf20fe7669a, D), by the line of the decision whose answer settled each: a settle reaches only that
/// line (decision 4afee4855944, 1).
type Settled = Arc<BTreeMap<ContributionId, Set>>;

/// What rules 5 and 6 read of one decision: its window and the objections to what it uses, both less
/// those settled below it, its own answers, and its breakage entries' ages.
#[derive(Debug, Clone)]
pub(crate) struct Reads {
    pub(crate) window: Arc<[At]>,
    pub(crate) uses: Vec<At>,
    answers: Vec<(Key, Disposition)>,
    pub(crate) ages: BTreeMap<String, u64>,
}

/// Every objection in a replica, what rule 5 asks of each decision, and what rule 6 counts: each
/// decision's window (objections to its strict decision-ancestors) and the objections to what it uses,
/// both less those settled below it, its answers, and its breakage entries' ages.
#[derive(Debug, Default)]
pub struct Obligations {
    list: Vec<Objection>,
    /// Each objection's index in `list`, by id.
    by_id: HashMap<ContributionId, At>,
    to_decision: HashMap<ContributionId, Vec<At>>,
    to_manifest: HashMap<ContentHash, Vec<At>>,
    to_proposal: Vec<At>,
    /// Each objected proposal's change entries, if its base's manifest and its own resolve.
    changes: HashMap<ContributionId, Option<Vec<ChangeEntry>>>,
    /// The results an objection comes from, by id.
    results: HashMap<ContributionId, CheckResult>,
    /// The goal links of each goal claim an objection comes from.
    links: HashMap<ContributionId, Vec<GoalLink>>,
    /// The decisions that may qualify: not retracted, on a line, by one of its integrators under the
    /// policy in force for it, and with parents that may. No other is judged by rules 5 and 6, and none
    /// is the ancestor of one that is (rule 3), so none of what follows is kept for the rest.
    may_qualify: HashSet<ContributionId>,
    /// Each decision's window, ascending: objections to its strict ancestors in the decision graph that
    /// no answer below it settled.
    window: HashMap<ContributionId, Arc<[At]>>,
    /// The objections to each decision and its ancestors in the decision graph, settled or not: what a
    /// child's window starts from, shared along runs of decisions no one objects to.
    history: HashMap<ContributionId, Set>,
    /// What answers at each decision and its ancestors settled, shared along runs that settle nothing.
    settled: HashMap<ContributionId, Settled>,
    /// The objections each decision's own answers settle, ascending.
    settles: HashMap<ContributionId, Vec<At>>,
    /// Each decision's answers, resolved.
    answers: HashMap<ContributionId, Vec<(Key, Disposition)>>,
    /// The objections to what each decision uses, its manifest and the proposals it uses, that no answer
    /// on its line below it settled.
    uses: HashMap<ContributionId, Vec<At>>,
    /// Each decision's breakage entries' ages, by check, where it has any.
    ages: HashMap<ContributionId, BTreeMap<String, u64>>,
    /// Each manifest a judged decision names, numbered, for `bases`.
    manifests: HashMap<ContentHash, u32>,
    /// For each judged decision, the manifests of the qualifying decisions among it and its ancestors in
    /// the decision graph: what a `fell_back` answer to an objection to a child may return to (finding
    /// fa578bd1da48), shared along runs that add none.
    bases: HashMap<ContributionId, Set>,
}

/// Whether `path` is `prefix` or below it.
fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    !prefix.is_empty() && (path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/')))
}

impl Obligations {
    /// Every objection whose target resolves, with the results and goal links its basis may need, and
    /// the answers of each decision in `may_qualify` resolved. [`Obligations::propagate`] then works out
    /// what each of those reads.
    pub(crate) fn new(
        ix: &Index,
        lines: &Lines,
        order: &[ContributionId],
        may_qualify: HashSet<ContributionId>,
    ) -> Obligations {
        let mut ob = Obligations { may_qualify, ..Obligations::default() };
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
        ob.by_id = ob.list.iter().enumerate().map(|(at, o)| (o.id, at as At)).collect();
        for id in order.iter().filter(|id| ob.may_qualify.contains(*id)) {
            let d = &lines.decision(id).expect("in order").decision;
            let answers = ob.resolve(ix, lines, d);
            if !answers.is_empty() {
                ob.answers.insert(*id, answers);
            }
        }
        ob
    }

    /// What an answer names, if anything: an objection, or a target whose objections it covers.
    fn resolve(&self, ix: &Index, lines: &Lines, d: &Decision) -> Vec<(Key, Disposition)> {
        d.answers
            .iter()
            .filter_map(|a| {
                let key = if self.by_id.contains_key(&a.objection) {
                    Key::Objection(a.objection)
                } else if a.objection == d.line {
                    Key::Line
                } else {
                    match Target::of(ix, lines, &a.objection)? {
                        Target::Decision(d) => Key::Decision(d),
                        Target::Manifest(_, hash) => Key::Manifest(hash),
                        Target::Proposal { line, change, .. } => Key::Change(line, change),
                    }
                };
                Some((key, a.disposition))
            })
            .collect()
    }

    /// Whether an answer naming `key` covers objection `o`, judged in the answering decision's window
    /// (`in_window`) or as an objection to what it uses.
    fn covers(key: &Key, o: &Objection, in_window: bool) -> bool {
        match (key, o.target) {
            (Key::Objection(id), _) => *id == o.id,
            (Key::Line, _) => in_window,
            (Key::Decision(d), Target::Decision(t)) => *d == t,
            (Key::Manifest(h), Target::Manifest(_, t)) => *h == t,
            (Key::Change(l, c), Target::Proposal { line, change, .. }) => *l == line && *c == change,
            _ => false,
        }
    }

    /// The objections an answer naming a target covers, for [`Obligations::settle_asks`].
    fn covered(&self, key: &Key) -> Vec<At> {
        match key {
            Key::Objection(_) | Key::Line => vec![],
            Key::Decision(d) => self.to_decision.get(d).cloned().unwrap_or_default(),
            Key::Manifest(h) => self.to_manifest.get(h).cloned().unwrap_or_default(),
            Key::Change(..) => self
                .to_proposal
                .iter()
                .filter(|at| Self::covers(key, &self.list[**at as usize], false))
                .copied()
                .collect(),
        }
    }

    /// The objections an answer names: the one it names, those to the target it names, or for its line,
    /// those in its window.
    fn named(&self, key: &Key, window: &[At]) -> Vec<At> {
        match key {
            Key::Objection(o) => self.by_id.get(o).copied().into_iter().collect(),
            Key::Line => window.to_vec(),
            _ => self.covered(key),
        }
    }

    /// Whether some decision answers its line, blanket: then settling takes two passes.
    pub(crate) fn has_line_answers(&self) -> bool {
        self.answers.values().flatten().any(|(key, _)| *key == Key::Line)
    }

    /// The questions settling asks: for each decision with answers, whether each objection they name is
    /// in its past, and each retraction below it. With `line_answers` false, the first pass: answers
    /// naming the line are left out, as what they name is the window, which settling decides. With it
    /// true, the second: only those, each naming the window the first pass found, which holds the one
    /// the second will (decision 4afee4855944).
    pub(crate) fn settle_asks(
        &self,
        retractions: &Retractions,
        line_answers: bool,
    ) -> HashMap<ContributionId, Vec<ContributionId>> {
        let mut refs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for (id, answers) in &self.answers {
            let window = self.window.get(id).map(|w| &w[..]).unwrap_or(&[]);
            for (key, _) in answers.iter().filter(|(key, _)| (*key == Key::Line) == line_answers) {
                for at in self.named(key, window) {
                    let o = self.list[at as usize].id;
                    let asked = refs.entry(*id).or_default();
                    asked.push(o);
                    asked.extend(retractions.below(&o));
                }
            }
        }
        refs
    }

    /// Each decision's window, settled objections, used objections and breakage ages, in `lines`' causal
    /// order, from payloads and the decision graph; `in_past(x, a)` answers [`Obligations::settle_asks`].
    /// With `line_answers` false, answers naming the line settle nothing.
    pub(crate) fn propagate(
        &mut self,
        ix: &Index,
        lines: &Lines,
        order: &[ContributionId],
        retractions: &Retractions,
        line_answers: bool,
        in_past: &mut dyn FnMut(&ContributionId, &ContributionId) -> bool,
    ) {
        self.window.clear();
        self.history.clear();
        self.settled.clear();
        self.settles.clear();
        self.uses.clear();
        self.ages.clear();
        for id in order.iter().filter(|id| self.may_qualify.contains(*id)) {
            let d = &lines.decision(id).expect("in order").decision;
            let settled = self.settled_for(&d.parents);
            let on_line = settled.get(&d.line).cloned().unwrap_or_default();
            let window = self.window_for(lines, &d.line, &d.parents, &on_line);
            let uses = self.uses_for(ix, lines, d, &on_line);
            let ages = self.ages_for(lines, d);
            // What its answers settle for its descendants (finding 0bf37b345567): exactly the objections rule
            // 5 judges here and that an answer here names, so an allowed one, as this decision qualifies
            // only if each judged objection has one. Rule 5 judges an objection in its window, or to what
            // it uses, that is in its past and not retracted there. One it doesn't judge (to a manifest or
            // proposal it doesn't use, outside its past, or retracted there, even if later restored) isn't
            // settled. Whether it qualifies needn't be asked: if not, no descendant does (rule 3).
            let mut settles: Vec<At> = Vec::new();
            for (key, _) in self.answers.get(id).into_iter().flatten() {
                if *key == Key::Line && !line_answers {
                    continue;
                }
                for at in self.named(key, &window) {
                    let in_window = window.binary_search(&at).is_ok();
                    if !in_window && uses.binary_search(&at).is_err() {
                        continue;
                    }
                    let o = self.list[at as usize].id;
                    if !in_past(&o, id) || retractions.within(&o, |x| in_past(x, id)) {
                        continue;
                    }
                    settles.push(at);
                }
            }
            settles.sort_unstable();
            settles.dedup();
            // What its children read: the objections to it and below, and what it and below settled.
            let mut history = self.history_for(&d.parents);
            for at in self.to_decision.get(id).into_iter().flatten() {
                history = history.with(*at);
            }
            let settled = match settles.is_empty() {
                true => settled,
                false => {
                    let mut by_line = (*settled).clone();
                    let mine = by_line.entry(d.line).or_default();
                    *mine = settles.iter().fold(mine.clone(), |all, at| all.with(*at));
                    Arc::new(by_line)
                }
            };
            self.window.insert(*id, window);
            self.history.insert(*id, history);
            self.settled.insert(*id, settled);
            if !settles.is_empty() {
                self.settles.insert(*id, settles);
            }
            if !uses.is_empty() {
                self.uses.insert(*id, uses);
            }
            if !ages.is_empty() {
                self.ages.insert(*id, ages);
            }
        }
    }

    /// The objections settled at strict ancestors of a decision with `parents`, by line. A decision with
    /// one parent shares its parent's.
    fn settled_for(&self, parents: &[ContributionId]) -> Settled {
        let below = |p: &ContributionId| self.settled.get(p).cloned().unwrap_or_default();
        match parents {
            [] => Settled::default(),
            [p] => below(p),
            _ => {
                let mut all: BTreeMap<ContributionId, Set> = BTreeMap::new();
                for p in parents {
                    for (line, theirs) in below(p).iter() {
                        let mine = all.entry(*line).or_default();
                        *mine = mine.union(theirs);
                    }
                }
                Arc::new(all)
            }
        }
    }

    /// The objections to the strict ancestors of a decision with `parents`, settled or not. A decision
    /// with one parent shares its parent's.
    fn history_for(&self, parents: &[ContributionId]) -> Set {
        parents.iter().filter_map(|p| self.history.get(p)).fold(Set::default(), |all, theirs| all.union(theirs))
    }

    /// Records that `id` has been judged, after its parents: the manifests of the qualifying decisions
    /// among it and its decision-ancestors are its parents', with its own if it qualifies.
    pub(crate) fn judged(&mut self, lines: &Lines, id: &ContributionId) {
        let d = &lines.decision(id).expect("judged").decision;
        let mut bases = self.bases_below(&d.parents);
        if matches!(lines.verdict(id), Some(Ok(_))) {
            let next = self.manifests.len() as u32;
            bases = bases.with(*self.manifests.entry(d.manifest).or_insert(next));
        }
        self.bases.insert(*id, bases);
    }

    /// The manifests of the qualifying strict decision-ancestors of a decision with `parents`.
    fn bases_below(&self, parents: &[ContributionId]) -> Set {
        parents.iter().filter_map(|p| self.bases.get(p)).fold(Set::default(), |all, theirs| all.union(theirs))
    }

    /// The decisions whose window or used objections differ from what walking each one's decision
    /// ancestry gives, given what each settled: the reference the shared histories are tested against.
    #[doc(hidden)]
    pub fn differ_from_walking(&self, ix: &Index, lines: &Lines) -> Vec<ContributionId> {
        let mut differ = Vec::new();
        for id in &self.may_qualify {
            let d = &lines.decision(id).expect("may qualify").decision;
            let (mut objected, mut settled) = (BTreeSet::new(), HashSet::new());
            for a in lines.decision_ancestry(id).iter().filter(|a| *a != id) {
                objected.extend(self.to_decision.get(a).into_iter().flatten().copied());
                if lines.decision(a).is_some_and(|r| r.decision.line == d.line) {
                    settled.extend(self.settles.get(a).into_iter().flatten().copied());
                }
            }
            let window: Vec<At> = objected.into_iter().filter(|at| !settled.contains(at)).collect();
            let as_set = settled.iter().fold(Set::default(), |all, at| all.with(*at));
            let uses = self.uses_for(ix, lines, d, &as_set);
            if self.window.get(id).map(|w| &w[..]).unwrap_or(&[]) != &window[..]
                || self.uses.get(id).map(Vec::as_slice).unwrap_or(&[]) != &uses[..]
            {
                differ.push(*id);
            }
        }
        differ.sort();
        differ
    }

    /// Every objection whose target resolves, in id order.
    pub fn list(&self) -> &[Objection] {
        &self.list
    }

    /// The window of a decision on `line` with `parents`: the objections to its strict decision-ancestors
    /// that no answer settled for it, ascending, given `settled`, what answers at those ancestors on its
    /// line settled. An objection is settled for it only by an answer at an ancestor on its own line
    /// (decision 4afee4855944, 1). With one parent on its line, that is the parent's window less what the
    /// parent settled, with the objections to the parent; shared if there are none of either. Otherwise,
    /// as the window of a parent on another line was settled for that line, and a merge's parents may
    /// each have settled what the other's window holds, it is the parents' histories less `settled`,
    /// read a word per 64 objections where the two differ.
    fn window_for(&self, lines: &Lines, line: &ContributionId, parents: &[ContributionId], settled: &Set) -> Arc<[At]> {
        match parents {
            [] => Arc::from([]),
            [p] if lines.decision(p).is_some_and(|r| r.decision.line == *line) => {
                let below = self.window.get(p).cloned().unwrap_or_else(|| Arc::from([]));
                let (objected, settles) = (self.to_decision.get(p), self.settles.get(p));
                if objected.is_none() && settles.is_none() {
                    return below;
                }
                let settles = settles.map(Vec::as_slice).unwrap_or(&[]);
                let mut all: Vec<At> = below.iter().filter(|at| settles.binary_search(at).is_err()).copied().collect();
                all.extend(objected.into_iter().flatten());
                all.sort_unstable();
                all.into()
            }
            _ => {
                let mut all = Vec::new();
                self.history_for(parents).difference(settled, &mut all);
                all.into()
            }
        }
    }

    /// The objections to what a decision uses unchanged (rule 5): to a manifest artifact with its
    /// manifest hash, or to a proposal `X` it uses, by listing `X` or a proposal with `X`'s line and
    /// change, or because every entry of `X`'s change is in its own change against one of its parents on
    /// its line (security note 17906a29698f). Ascending.
    fn uses_for(&self, ix: &Index, lines: &Lines, d: &Decision, settled: &Set) -> Vec<At> {
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
        // Settled by an answer at an ancestor on its line.
        uses.retain(|at| !settled.contains(*at));
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
                let Some(d) = lines.decision(&d) else { return false };
                self.manifests.get(&a.manifest).is_some_and(|m| self.bases_below(&d.decision.parents).contains(*m))
            }
            _ => false,
        }
    }

    /// Rule 5 for decision `e` by `author` on its line, under `policy`, over the past `in_past` holds:
    /// every objection in its window to a qualifying decision, and every objection to what it uses, in
    /// that past, not retracted there and not settled below it, has an allowed answer in `e` itself,
    /// naming it or its target (decision 8cf20fe7669a).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rule_5(
        &self,
        lines: &Lines,
        retractions: &Retractions,
        author: &AgentId,
        e: &Decision,
        policy: &Line,
        reads: &Reads,
        in_past: &mut dyn FnMut(&ContributionId) -> bool,
        in_epoch: &mut dyn FnMut(&ContributionId) -> bool,
    ) -> Result<(), Unqualified> {
        let window = reads.window.iter().map(|at| (false, at));
        let uses = reads.uses.iter().filter(|at| reads.window.binary_search(at).is_err()).map(|at| (true, at));
        for (uses_it, at) in window.chain(uses) {
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
            let mut answers =
                reads.answers.iter().filter(|(key, _)| Self::covers(key, o, !uses_it)).map(|(_, d)| *d).peekable();
            let Some(first) = answers.peek().copied() else {
                return Err(Unqualified::Unanswered { objection: o.id, basis: basis.kind() });
            };
            if !answers.any(|disposition| self.allowed(lines, o, &basis, uses_it, e, disposition)) {
                return Err(Unqualified::AnswerNotAllowed { objection: o.id, disposition: first, basis: basis.kind() });
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
            answers: self.answers.get(id).cloned().unwrap_or_default(),
            ages: self.ages.get(id).cloned().unwrap_or_default(),
        }
    }

    /// The same for a decision not yet written, whose past is the whole replica.
    pub(crate) fn of_new(&self, ix: &Index, lines: &Lines, d: &Decision) -> Reads {
        let settled = self.settled_for(&d.parents).get(&d.line).cloned().unwrap_or_default();
        Reads {
            window: self.window_for(lines, &d.line, &d.parents, &settled),
            uses: self.uses_for(ix, lines, d, &settled),
            answers: self.resolve(ix, lines, d),
            ages: self.ages_for(lines, d),
        }
    }

    /// The objections among `candidates` that bind now, unanswered: under `policy`, over the whole
    /// replica, with retraction read now, each with the dispositions its basis allows. For
    /// [`Lines::open_objections`] and [`Lines::proposal_objections`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &self,
        lines: &Lines,
        retractions: &Retractions,
        line: &ContributionId,
        policy: &Line,
        candidates: &[(bool, At)],
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
            let allowed = basis.allows(*uses_it);
            open.push(Open { objection: o.clone(), basis, allowed });
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

    /// The window of the next move on a line whose heads are `heads`.
    pub(crate) fn next_move(&self, lines: &Lines, line: &ContributionId, heads: &[ContributionId]) -> Vec<(bool, At)> {
        let settled = self.settled_for(heads).get(line).cloned().unwrap_or_default();
        self.window_for(lines, line, heads, &settled).iter().map(|at| (false, *at)).collect()
    }

    /// The objections to a manifest or proposal that a next move on its line, from `heads`, using
    /// `proposal` unchanged would have to answer: to any proposal with its line and change, and to its
    /// manifest, less those an answer on the line settled.
    pub(crate) fn of_proposal(
        &self,
        lines: &Lines,
        proposal: &ContributionId,
        heads: &[ContributionId],
    ) -> Vec<(bool, At)> {
        let Some(p) = lines.proposal_named(proposal) else { return vec![] };
        let settled = self.settled_for(heads).get(&p.line).cloned().unwrap_or_default();
        let mut ats: Vec<At> = self.to_manifest.get(&p.manifest).cloned().unwrap_or_default();
        ats.extend(self.to_proposal.iter().filter(|at| {
            matches!(self.list[**at as usize].target, Target::Proposal { line, change, .. } if line == p.line && change == p.change)
        }));
        ats.sort_unstable();
        ats.dedup();
        ats.retain(|at| !settled.contains(*at));
        ats.into_iter().map(|at| (true, at)).collect()
    }
}
