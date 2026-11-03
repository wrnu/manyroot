//! Check results, counted, and green (docs/design/weave.md, section 1 *Goals, checks
//! and objections*, and section 2 *Counted check results* and *Green*).
//!
//! A check result is evidence on a manifest artifact whose method names a check
//! with its definition hash, as Weave's runner writes it. Results attach to the
//! manifest, not to a decision. Counting is done under one policy version `P`:
//! a result is eligible if its method is one of `P`'s checks (tested first), it
//! isn't retracted (retractions of retractions included), its author is one of `P`'s validators but not the decision's
//! author, and it descends from `P`'s artifact (the policy epoch). Of those, each
//! validator's latest per manifest and method counts, and several concurrent
//! latest results count as one fail.
//!
//! Counting is over one set of contributions: the whole replica, for green under
//! a line's current policy ([`Checks::counted`], [`Checks::green`]), or one
//! decision's own causal past, under the policy in force for it
//! ([`Checks::count_in_pasts`]), so that a later re-run, retraction or policy
//! change never changes what that decision saw (finding `b2098df8e4be`).
//!
//! Every ancestor question here is answered by the ancestor index, never by a
//! walk per question (decision on split note dc467163233c): over the whole
//! replica, the latest results by [`ancestry::latest`] and the epoch by an
//! [`Ancestry`] built over the results; within a decision's past, which results
//! and retractions are in it and the epoch by an [`Ancestry`], and which of those
//! results lie below one another by [`ancestry::rounds`].

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};

use manyroot::{AgentId, ContributionId, Graph};

use crate::ancestry::{self, Ancestry, Stats};
use crate::check::{Exit, RecordedTail};
use crate::format::Line;
use crate::ground::Index;
use crate::hash::ContentHash;
use crate::lines::Lines;

/// A check result: evidence on a valid manifest artifact whose method names a check and its
/// definition hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub id: ContributionId,
    pub author: AgentId,
    /// The content hash of the manifest it targets.
    pub manifest: ContentHash,
    /// `<name> blake3:<definition hash>`.
    pub method: String,
    pub passed: bool,
    /// The attempt number its details give, if they give one.
    pub attempt: Option<u64>,
    /// Retracted now, by the protocol's rule. Within a causal past, [`crate::ground::Retractions::within`]
    /// judges it.
    pub retracted: bool,
}

/// The details a result records: the manifest's content hash, the attempt number, the argv and how
/// it ended, then the output tail as [`crate::check::recorded_tail`] prepared it (weave.md section 3,
/// *Its result*). Each on its own line, the tail last.
pub fn details(manifest: &ContentHash, attempt: u64, argv: &[String], exit: &Exit, tail: &RecordedTail) -> String {
    let tail = match tail {
        RecordedTail::Text(text) => text.clone(),
        RecordedTail::Withheld { size, hash, rule } => {
            format!("output withheld: {size} bytes, {hash}, as it matched {rule}")
        }
    };
    format!("{manifest}\nattempt {attempt}\nargv {argv:?}\n{exit}\n\n{tail}")
}

/// [`details`] for a check that ran outside the sandbox: `run`'s lines (the commit, tree, bundle, box
/// and log) come after the exit, before the blank line and the tail, so [`parse_details`] reads it as
/// it reads any result.
pub fn details_with_run(
    manifest: &ContentHash,
    attempt: u64,
    argv: &[String],
    exit: &Exit,
    run: &[String],
    tail: &RecordedTail,
) -> String {
    let tail = match tail {
        RecordedTail::Text(text) => text.clone(),
        RecordedTail::Withheld { size, hash, rule } => {
            format!("output withheld: {size} bytes, {hash}, as it matched {rule}")
        }
    };
    let run: String = run.iter().map(|l| format!("{l}\n")).collect();
    format!("{manifest}\nattempt {attempt}\nargv {argv:?}\n{exit}\n{run}\n{tail}")
}

/// The manifest hash and the attempt number a result's details start with, if they are in
/// [`details`]'s form.
pub fn parse_details(details: &str) -> Option<(ContentHash, u64)> {
    let mut lines = details.lines();
    let manifest = lines.next()?.parse().ok()?;
    let attempt = lines.next()?.strip_prefix("attempt ")?.parse().ok()?;
    Some((manifest, attempt))
}

/// One validator's results for one check on one manifest.
type Key = (AgentId, ContentHash, String);

/// What each check's counted results are, for one decision under one policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counted {
    pub check: String,
    pub method: String,
    /// The counted passes, one per validator whose latest result passed.
    pub passes: Vec<ContributionId>,
    /// The counted fails: a validator's latest result that failed, or each of several concurrent
    /// latest results, which count together as one fail.
    pub fails: Vec<ContributionId>,
    /// How many validators' latest results count as a fail.
    pub failing_validators: usize,
    /// Earlier fails by the counted validators that a later attempt replaced, so re-running until a
    /// check passes shows (weave.md section 2).
    pub replaced: Vec<ContributionId>,
}

impl Counted {
    /// At least one pass and no fail.
    pub fn green(&self) -> bool {
        !self.passes.is_empty() && self.failing_validators == 0
    }
}

/// A line's green (weave.md section 2, *Green*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Green {
    /// The line has no current policy: its chain diverged into these versions, so green is undefined
    /// until the line's author settles them.
    Undefined(Vec<ContributionId>),
    /// No green decision is an ancestor of, or equal to, every head.
    None,
    /// The green decision.
    One(ContributionId),
    /// Several green decisions, none a descendant of another: ambiguous, smallest id first.
    Ambiguous(Vec<ContributionId>),
}

/// One count over a decision's own causal past: of the results on `manifest`, under the policy in force
/// for `decision`, leaving out its author's, over its strict causal ancestors. `manifest` is its own, or
/// the manifest an objection to it objects to. The policy and author come from [`Lines`], so a query
/// can't pair a decision with another's policy (task 3117629ebf94).
#[derive(Debug, Clone, Copy)]
pub struct PastQuery {
    pub decision: ContributionId,
    pub manifest: ContentHash,
}

/// Every check result in a replica by a validator under some policy version, with each validator's
/// latest per manifest and method over the whole replica, and the policy epoch of each result for each
/// line's current policy.
#[derive(Debug, Default)]
pub struct Checks {
    results: HashMap<ContributionId, CheckResult>,
    /// Each key's results, retracted or not, in id order.
    by_key: HashMap<Key, Vec<ContributionId>>,
    /// Those of each key's results not retracted that no other such descends from.
    latest: HashMap<Key, Vec<ContributionId>>,
    /// Whether each current policy version is an ancestor of each result that names one of its checks.
    epochs: Ancestry,
    stats: Stats,
    epoch_misses: Cell<usize>,
}

/// An ancestor answer from `ancestry`, or a walk, counted in `misses`, for a question nothing asked: a
/// bug, which fails debug builds (answer b2dbc4f5e810).
pub(crate) fn answer(
    ancestry: &Ancestry,
    misses: &Cell<usize>,
    g: &Graph,
    a: &ContributionId,
    b: &ContributionId,
) -> bool {
    if !g.contains(a) {
        return false;
    }
    let answer = ancestry.is_ancestor(a, b);
    debug_assert!(answer.is_some(), "{b} didn't ask about {a}");
    answer.unwrap_or_else(|| {
        misses.set(misses.get() + 1);
        g.is_ancestor(a, b)
    })
}

impl Checks {
    pub fn build(g: &Graph, ix: &Index, lines: &Lines) -> Checks {
        let mut checks = Checks::default();
        // Only results by a validator under some policy version of a line judged can count, so only
        // theirs are asked about: anyone else's writes cost nothing, as do the validators of a line
        // nobody asked about (task a4d1a06a1072).
        let validators: HashSet<&AgentId> = lines.judged_versions().flat_map(|v| &v.policy.validators).collect();
        for r in ix.check_results().iter().filter(|r| validators.contains(&r.author)) {
            checks.results.insert(r.id, r.clone());
            checks.by_key.entry((r.author, r.manifest, r.method.clone())).or_default().push(r.id);
        }
        let mut keys: Vec<&Key> = checks.by_key.keys().collect();
        keys.sort();
        let groups: Vec<Vec<ContributionId>> = keys
            .iter()
            .map(|k| checks.by_key[*k].iter().filter(|r| !checks.results[*r].retracted).copied().collect())
            .collect();
        let (latest, stats) = ancestry::latest(g, &groups);
        checks.latest = keys
            .iter()
            .zip(&groups)
            .zip(latest)
            .map(|((key, group), latest)| {
                let mine = group.iter().zip(latest).filter(|(_, l)| *l).map(|(id, _)| *id).collect();
                ((*key).clone(), mine)
            })
            .collect();
        // The epoch: each result that names a check of a line's current policy asks whether that
        // version is its ancestor. The current versions by method, then one pass over the results
        // (finding bf859c9a069d).
        let mut by_method: HashMap<String, Vec<ContributionId>> = HashMap::new();
        for line in lines.lines().filter(|l| lines.judges(&l.id)) {
            if let [current] = lines.current_versions(&line.id).as_slice() {
                let policy = &lines.version(current).expect("a current version").policy;
                for name in policy.checks.keys() {
                    let method = policy.check_method(name).expect("one of its checks");
                    by_method.entry(method).or_default().push(*current);
                }
            }
        }
        let refs: HashMap<ContributionId, Vec<ContributionId>> = checks
            .results
            .values()
            .filter_map(|r| by_method.get(&r.method).map(|versions| (r.id, versions.clone())))
            .collect();
        checks.epochs = Ancestry::build(g, &refs, &[]);
        let epoch = checks.epochs.stats();
        checks.stats = Stats {
            passes: stats.passes + epoch.passes,
            ors: stats.ors + epoch.ors,
            questions: stats.questions + epoch.questions,
            bytes: epoch.bytes,
            ..stats
        };
        checks
    }

    pub fn result(&self, id: &ContributionId) -> Option<&CheckResult> {
        self.results.get(id)
    }

    /// Every result on a manifest by a validator under some policy version, retracted or not, in id
    /// order.
    pub fn on_manifest(&self, manifest: &ContentHash) -> Vec<&CheckResult> {
        let mut on: Vec<&CheckResult> = self.results.values().filter(|r| r.manifest == *manifest).collect();
        on.sort_by_key(|r| r.id);
        on
    }

    /// A validator's latest results for a check on a manifest over the whole replica, not retracted:
    /// none, one, or several concurrent ones. `weave validate` refuses to run a check that has one
    /// unless asked again.
    pub fn latest(&self, author: &AgentId, manifest: &ContentHash, method: &str) -> &[ContributionId] {
        self.latest.get(&(*author, *manifest, method.to_owned())).map_or(&[], Vec::as_slice)
    }

    /// The attempt number a validator's next result for a check on a manifest takes: one past the
    /// highest it has recorded, retracted or not, or 1. A forged attempt of `u64::MAX` gives `u64::MAX`
    /// again, rather than overflowing (finding 5d6f605b2f15).
    pub fn next_attempt(&self, author: &AgentId, manifest: &ContentHash, method: &str) -> u64 {
        let key = (*author, *manifest, method.to_owned());
        let highest = self.by_key.get(&key).into_iter().flatten().filter_map(|r| self.results[r].attempt).max();
        highest.unwrap_or(0).saturating_add(1)
    }

    /// Counts one check's results for a decision, given each validator's latest results and the
    /// eligible results they might replace.
    fn tally(
        &self,
        check: &str,
        method: String,
        per_validator: impl Iterator<Item = (Vec<ContributionId>, Vec<ContributionId>)>,
    ) -> Counted {
        let mut c = Counted {
            check: check.to_owned(),
            method,
            passes: vec![],
            fails: vec![],
            failing_validators: 0,
            replaced: vec![],
        };
        for (latest, eligible) in per_validator {
            match latest.as_slice() {
                [] => continue,
                [one] if self.results[one].passed => c.passes.push(*one),
                _ => {
                    c.fails.extend(&latest);
                    c.failing_validators += 1;
                }
            }
            c.replaced.extend(eligible.iter().filter(|r| !latest.contains(r) && !self.results[*r].passed));
        }
        c
    }

    /// The counted results for each check of the current policy of a decision's line, over the whole
    /// replica: what green reads. `None` if `decision` isn't a decision, or its line has no current
    /// policy (a diverged chain, or not a line).
    pub fn counted(&self, g: &Graph, lines: &Lines, decision: &ContributionId) -> Option<Vec<Counted>> {
        let d = lines.decision(decision)?;
        let [version] = lines.current_versions(&d.decision.line)[..] else { return None };
        let policy = &lines.version(&version)?.policy;
        Some(self.counted_now(g, &version, policy, std::slice::from_ref(&d.author), &d.decision.manifest))
    }

    /// The counted results for each check of `policy`, current version `version`, on any manifest: what
    /// green would read for a decision with that manifest, by any of the policy's integrators, whose own
    /// results are left out as a decision's integrator's are (`weave green --manifest`, plan cc496dcee0e3
    /// step 1b: scripts/merge reads a candidate's results through it before git merges).
    pub fn counted_on_manifest(
        &self,
        g: &Graph,
        version: &ContributionId,
        policy: &Line,
        manifest: &ContentHash,
    ) -> Vec<Counted> {
        self.counted_now(g, version, policy, &policy.integrators, manifest)
    }

    /// [`Checks::counted`], given the current version.
    fn counted_now(
        &self,
        g: &Graph,
        version: &ContributionId,
        policy: &Line,
        excluded: &[AgentId],
        manifest: &ContentHash,
    ) -> Vec<Counted> {
        let in_epoch = |r: &ContributionId| answer(&self.epochs, &self.epoch_misses, g, version, r);
        let validators: BTreeSet<&AgentId> = policy.validators.iter().filter(|v| !excluded.contains(v)).collect();
        policy
            .definitions()
            .map(|definition| {
                let (name, method) = (definition.name, definition.method());
                let per_validator = validators.iter().map(|v| {
                    let key = (**v, *manifest, method.clone());
                    let eligible = |list: Option<&Vec<ContributionId>>| -> Vec<ContributionId> {
                        list.into_iter()
                            .flatten()
                            .filter(|r| !self.results[*r].retracted && in_epoch(r))
                            .copied()
                            .collect()
                    };
                    (eligible(self.latest.get(&key)), eligible(self.by_key.get(&key)))
                });
                self.tally(name, method.clone(), per_validator.collect::<Vec<_>>().into_iter())
            })
            .collect()
    }

    /// The counted results for each check of the policy in force for each query's decision, over that
    /// decision's own causal past (weave.md section 2), or `None` if its line's chain gives no policy in
    /// force. A result counts only if it, and the policy version, are ancestors of the decision, and it
    /// isn't retracted within that past by the protocol's rule; and of those, each validator's latest
    /// within that past counts. So a later re-run, retraction or policy change, outside the past, never
    /// changes the count (finding c85337dfad09). Every ancestor question is answered by the ancestor
    /// index: which results, the retractions below them and policy versions lie in each past by one
    /// [`Ancestry`], and which results lie below one another by one [`ancestry::below_in_groups`] over
    /// each key's results, each eligible result reading its word once for every query it is eligible in
    /// (finding eb8b992ae58f).
    pub fn count_in_pasts(
        &self,
        g: &Graph,
        ix: &Index,
        lines: &Lines,
        queries: &[PastQuery],
    ) -> (Vec<Option<Vec<Counted>>>, Stats) {
        let retractions = ix.retractions();
        // Each query's decision, policy version and payload, author and manifest, from Lines.
        type Resolved<'l> = Option<(ContributionId, ContributionId, &'l Line, AgentId, ContentHash)>;
        let resolved: Vec<Resolved> = queries
            .iter()
            .map(|q| {
                let d = lines.decision(&q.decision)?;
                let version = lines.policy_in_force_for(&q.decision)?;
                Some((q.decision, version.id, &version.policy, d.author, q.manifest))
            })
            .collect();
        // Each query's checks, each with the validators' keys that have results.
        let plans: Vec<Vec<(String, String, Vec<Key>)>> = resolved
            .iter()
            .map(|q| {
                let Some((_, _, policy, author, manifest)) = q else { return vec![] };
                let validators: BTreeSet<&AgentId> = policy.validators.iter().filter(|v| *v != author).collect();
                policy
                    .definitions()
                    .map(|definition| {
                        let (name, method) = (definition.name, definition.method());
                        let keys = validators
                            .iter()
                            .map(|v| (**v, *manifest, method.clone()))
                            .filter(|k| self.by_key.contains_key(k))
                            .collect();
                        (name.to_owned(), method, keys)
                    })
                    .collect()
            })
            .collect();
        // Stage one: whether each result, and each retraction below it, is in the asker's past, and
        // whether the policy version is in the result's.
        let mut refs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for (q, plan) in resolved.iter().zip(&plans) {
            let Some((asker, version, ..)) = q else { continue };
            for key in plan.iter().flat_map(|(_, _, keys)| keys) {
                for r in &self.by_key[key] {
                    let asked = refs.entry(*asker).or_default();
                    asked.push(*r);
                    asked.extend(retractions.below(r));
                    refs.entry(*r).or_default().push(*version);
                }
            }
        }
        let past = Ancestry::build(g, &refs, &[]);
        let misses = Cell::new(0);
        let ask = |a: &ContributionId, b: &ContributionId| answer(&past, &misses, g, a, b);
        // Stage two: the keys asked about, each a group of its results; and per query, check and key, a
        // sink that its eligible results read into: in its past, not retracted there by the protocol's rule
        // (a retraction in the past that isn't itself retracted there), in the epoch.
        let mut group_of: HashMap<&Key, usize> = HashMap::new();
        let mut groups: Vec<Vec<ContributionId>> = Vec::new();
        let mut sinks: Vec<Vec<Vec<usize>>> = Vec::new();
        // Each sink's group, and its eligible results by slot in that group.
        let mut sink_slots: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut sink_at: Vec<Vec<Vec<usize>>> = Vec::new();
        for (q, plan) in resolved.iter().zip(&plans) {
            let mut mine = Vec::new();
            let Some((asker, version, ..)) = q else {
                sink_at.push(mine);
                continue;
            };
            for (_, _, keys) in plan {
                let mut per_key = Vec::new();
                for key in keys {
                    let next = groups.len();
                    let k = *group_of.entry(key).or_insert(next);
                    if k == next {
                        groups.push(self.by_key[key].clone());
                        sinks.push(vec![Vec::new(); self.by_key[key].len()]);
                    }
                    let sink = sink_slots.len();
                    let eligible: Vec<usize> = groups[k]
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| ask(r, asker) && !retractions.within(r, |x| ask(x, asker)) && ask(version, r))
                        .map(|(slot, _)| slot)
                        .collect();
                    for slot in &eligible {
                        sinks[k][*slot].push(sink);
                    }
                    sink_slots.push((k, eligible));
                    per_key.push(sink);
                }
                mine.push(per_key);
            }
            sink_at.push(mine);
        }
        let (below, latest_stats) = ancestry::below_in_groups(g, &groups, &sinks, sink_slots.len());
        let counted = resolved
            .iter()
            .zip(&plans)
            .zip(&sink_at)
            .map(|((q, plan), sink_at)| {
                q.as_ref()?;
                let per_check = plan
                    .iter()
                    .zip(sink_at)
                    .map(|((name, method, _), sinks)| {
                        let per_validator = sinks.iter().map(|sink| {
                            let (k, eligible) = &sink_slots[*sink];
                            let ids = |slots: &mut dyn Iterator<Item = &usize>| -> Vec<ContributionId> {
                                slots.map(|slot| groups[*k][*slot]).collect()
                            };
                            let latest = ids(&mut eligible.iter().filter(|slot| !below[*sink][**slot]));
                            (latest, ids(&mut eligible.iter()))
                        });
                        self.tally(name, method.clone(), per_validator.collect::<Vec<_>>().into_iter())
                    })
                    .collect();
                Some(per_check)
            })
            .collect();
        let first = past.stats();
        let stats = Stats {
            passes: first.passes + latest_stats.passes,
            ors: first.ors + latest_stats.ors,
            questions: first.questions + latest_stats.questions,
            bytes: first.bytes,
            fallbacks: misses.get() + latest_stats.fallbacks,
            ..latest_stats
        };
        (counted, stats)
    }

    /// A line's green: the green decisions on the line that are ancestors of, or equal to, every head,
    /// and among them those with no green descendant there. Judged under the line's current policy,
    /// counting over the whole replica. A decision on another line, below a head through a merge or a
    /// line's first decision, is never the line's green (finding 3aa37cc9ef0d).
    pub fn green(&self, g: &Graph, lines: &Lines, line: &ContributionId) -> Green {
        let current = lines.current_versions(line);
        let [version] = current.as_slice() else { return Green::Undefined(current) };
        let policy = &lines.version(version).expect("a current version").policy;
        let heads = lines.heads(line);
        let Some((first, rest)) = heads.split_first() else { return Green::None };
        let mut below_every_head: HashSet<ContributionId> = lines.decision_ancestry(first);
        below_every_head.insert(*first);
        for head in rest {
            let mut mine = lines.decision_ancestry(head);
            mine.insert(*head);
            below_every_head.retain(|d| mine.contains(d));
        }
        let on_line = |d: &ContributionId| matches!(lines.verdict(d), Some(Ok(q)) if q.line == *line);
        let green: BTreeSet<ContributionId> = below_every_head
            .into_iter()
            .filter(on_line)
            .filter(|d| {
                let record = lines.decision(d).expect("a decision on the line");
                record.decision.breakage.is_empty()
                    && self
                        .counted_now(
                            g,
                            version,
                            policy,
                            std::slice::from_ref(&record.author),
                            &record.decision.manifest,
                        )
                        .iter()
                        .all(Counted::green)
            })
            .collect();
        // The green decisions' strict ancestors, in one walk of the decision graph.
        let mut below_green: HashSet<ContributionId> = HashSet::new();
        let parents = |d: &ContributionId| lines.decision(d).map(|r| r.decision.parents.clone()).unwrap_or_default();
        let mut stack: Vec<ContributionId> = green.iter().flat_map(parents).collect();
        while let Some(d) = stack.pop() {
            if below_green.insert(d) {
                stack.extend(parents(&d));
            }
        }
        let newest: Vec<ContributionId> = green.iter().filter(|d| !below_green.contains(d)).copied().collect();
        match newest.as_slice() {
            [] => Green::None,
            [one] => Green::One(*one),
            _ => Green::Ambiguous(newest),
        }
    }

    /// What answering the replica-wide latest-result and epoch questions did, and how many fell back to
    /// a walk.
    #[doc(hidden)]
    pub fn stats(&self) -> Stats {
        Stats { fallbacks: self.epoch_misses.get(), ..self.stats }
    }
}
