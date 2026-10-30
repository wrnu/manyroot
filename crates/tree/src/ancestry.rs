//! The ancestor rule, decided without a walk per question (design note
//! e93d47ad8a20, answer b2dbc4f5e810; task 18a4cf6d7d08).
//!
//! Weave asks, for each record, whether what it refers to is among its causal
//! ancestors. `Graph::is_ancestor` walks the history for each question, so an
//! open walked once per question. Instead, questions are answered for 64
//! *targets* (contributions asked about) at a time: one pass in causal order
//! gives each contribution a word saying which of those 64 are its strict
//! ancestors, the OR of its parents' words and bits, and each question reads
//! its answer from its asker's word ([`Passes`]).
//!
//! Two kinds of question use the passes. [`Ancestry`] answers a record's
//! references and keeps a bit for each, and all markers together count as one
//! more target, so each marker keeps one bit: is another marker a strict
//! ancestor? [`rounds`] answers askers that each ask about a whole shared list
//! (Lines asks this of a line's supersedes relations): it hands each asker's
//! answers to a callback and frees them, holding at most one word per
//! contribution at a time. A list too large for that takes more passes, so
//! memory stays linear in the history and time is the lever, never worse than a
//! walk per question.

use std::collections::HashMap;
use std::mem::size_of;

use manyroot::{ContributionId, Graph};

/// What building answers did, for tests that pin their bounds.
#[doc(hidden)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Passes over the history.
    pub passes: usize,
    /// Words held for the passes, reused by each: two per contribution.
    pub words: usize,
    /// Words combined over every pass: one per parent link and contribution per pass.
    pub ors: usize,
    /// Questions answered.
    pub questions: usize,
    /// Bytes kept once built, by the vectors' capacities and the maps' entries (not their slack).
    pub bytes: usize,
    /// The most answer words held at once by [`rounds`]; never more than one per contribution.
    pub peak_words: usize,
    /// Contributions stored in the shared lists [`rounds`] asks about.
    pub list_entries: usize,
    /// Questions nothing asked when it was built, answered by walking the graph. Always 0 unless a
    /// question was missed, which is a bug.
    pub fallbacks: usize,
}

/// The history in causal order, and the two words per contribution each pass uses.
pub(crate) struct Passes {
    position: HashMap<ContributionId, usize>,
    /// Each contribution's parents, by position: those of `at` are `parents[starts[at]..starts[at + 1]]`.
    starts: Vec<u32>,
    parents: Vec<u32>,
    own: Vec<u64>,
    below: Vec<u64>,
    passes: usize,
    ors: usize,
}

impl Passes {
    pub(crate) fn new(g: &Graph) -> Passes {
        let order = g.topo_order();
        let position: HashMap<ContributionId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut starts = Vec::with_capacity(order.len() + 1);
        let mut parents = Vec::new();
        for id in &order {
            starts.push(parents.len() as u32);
            parents.extend(g.get(id).expect("listed").parents().iter().map(|p| position[p] as u32));
        }
        starts.push(parents.len() as u32);
        let n = order.len();
        Passes { position, starts, parents, own: vec![0; n], below: vec![0; n], passes: 0, ors: 0 }
    }

    pub(crate) fn len(&self) -> usize {
        self.own.len()
    }

    pub(crate) fn position(&self, id: &ContributionId) -> Option<usize> {
        self.position.get(id).copied()
    }

    /// One pass: `owners` are the contributions asked about, each with its bit. Afterwards, until the
    /// next pass, [`Passes::below`] gives each contribution the bits of the owners that are its strict
    /// ancestors.
    pub(crate) fn run(&mut self, owners: &[(usize, u64)]) {
        for (at, b) in owners {
            self.own[*at] |= b;
        }
        for at in 0..self.len() {
            let (from, to) = (self.starts[at] as usize, self.starts[at + 1] as usize);
            let mut word = 0;
            for p in &self.parents[from..to] {
                word |= self.below[*p as usize] | self.own[*p as usize];
            }
            self.below[at] = word;
            self.ors += to - from + 1;
        }
        for (at, _) in owners {
            self.own[*at] = 0;
        }
        self.passes += 1;
    }

    pub(crate) fn below(&self, at: usize) -> u64 {
        self.below[at]
    }

    fn stats(&self) -> Stats {
        Stats { passes: self.passes, words: 2 * self.len(), ors: self.ors, ..Stats::default() }
    }
}

fn bit(words: &[u64], i: usize) -> bool {
    words[i / 64] & (1 << (i % 64)) != 0
}

fn set(words: &mut [u64], i: usize) {
    words[i / 64] |= 1 << (i % 64);
}

/// One record's references as sorted target indices, and their answers, one bit each.
#[derive(Debug, Default)]
struct Asked {
    refs: Vec<u32>,
    answers: Vec<u64>,
}

/// The answers to every record's references, and each marker's bit.
#[derive(Debug, Default)]
pub struct Ancestry {
    /// Each target's index.
    index: HashMap<ContributionId, u32>,
    asked: HashMap<ContributionId, Asked>,
    /// For each marker, whether another marker is a strict ancestor of it.
    below_a_marker: HashMap<ContributionId, bool>,
    stats: Stats,
}

impl Ancestry {
    /// Answers, for each record in `refs`, whether each contribution it names is a strict ancestor, and
    /// for each of `markers`, whether another of them is. A target the graph doesn't hold isn't kept.
    #[doc(hidden)]
    pub fn build(
        g: &Graph,
        refs: &HashMap<ContributionId, Vec<ContributionId>>,
        markers: &[ContributionId],
    ) -> Ancestry {
        let mut passes = Passes::new(g);
        let mut ancestry = Ancestry::default();
        let mut questions = 0;
        for (asker, named) in refs {
            let mut ids: Vec<u32> = named
                .iter()
                .filter(|t| passes.position(t).is_some())
                .map(|t| {
                    let next = ancestry.index.len() as u32;
                    *ancestry.index.entry(*t).or_insert(next)
                })
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids.shrink_to_fit();
            questions += ids.len();
            let answers = vec![0; ids.len().div_ceil(64)];
            ancestry.asked.insert(*asker, Asked { refs: ids, answers });
        }
        let marker_target = ancestry.index.len();
        let pass_count = (marker_target + 1).div_ceil(64);

        // Each pass's targets: their positions and bits.
        let mut owners_by_pass: Vec<Vec<(usize, u64)>> = vec![Vec::new(); pass_count];
        for (t, i) in &ancestry.index {
            let i = *i as usize;
            owners_by_pass[i / 64].push((passes.position(t).expect("kept only if held"), 1 << (i % 64)));
        }
        let marker_bit = 1 << (marker_target % 64);
        for m in markers {
            if let Some(at) = passes.position(m) {
                owners_by_pass[marker_target / 64].push((at, marker_bit));
            }
        }
        // The askers, with their positions, and a cursor into their references: the first not yet answered.
        // References are sorted by target index, so each pass answers a run of each.
        let mut askers: Vec<(usize, ContributionId, usize)> =
            ancestry.asked.keys().filter_map(|id| passes.position(id).map(|at| (at, *id, 0))).collect();
        askers.sort_unstable();
        for (pass, owners) in owners_by_pass.iter().enumerate() {
            passes.run(owners);
            for (at, asker, next) in &mut askers {
                let asked = ancestry.asked.get_mut(asker).expect("an asker");
                let word = passes.below(*at);
                while *next < asked.refs.len() && asked.refs[*next] as usize / 64 == pass {
                    if word & (1 << (asked.refs[*next] % 64)) != 0 {
                        set(&mut asked.answers, *next);
                    }
                    *next += 1;
                }
            }
            if pass == marker_target / 64 {
                for m in markers {
                    let below_one = passes.position(m).is_some_and(|at| passes.below(at) & marker_bit != 0);
                    ancestry.below_a_marker.insert(*m, below_one);
                }
            }
        }
        let entry = size_of::<ContributionId>();
        let bytes = ancestry.index.len() * (entry + size_of::<u32>())
            + ancestry
                .asked
                .values()
                .map(|a| {
                    entry
                        + size_of::<Asked>()
                        + a.refs.capacity() * size_of::<u32>()
                        + a.answers.capacity() * size_of::<u64>()
                })
                .sum::<usize>()
            + ancestry.below_a_marker.len() * (entry + 1);
        ancestry.stats = Stats { questions: questions + markers.len(), bytes, ..passes.stats() };
        ancestry
    }

    /// Whether `a` is a strict causal ancestor of `b`, if `b` named `a` when this was built; `None` if it
    /// didn't, or if the graph doesn't hold `a`, which is kept nowhere (the caller knows it is no
    /// ancestor).
    pub fn is_ancestor(&self, a: &ContributionId, b: &ContributionId) -> Option<bool> {
        let asked = self.asked.get(b)?;
        let slot = asked.refs.binary_search(self.index.get(a)?).ok()?;
        Some(bit(&asked.answers, slot))
    }

    /// Whether another of the markers it was built with is a strict ancestor of `marker`; `None` if
    /// `marker` wasn't one of them.
    pub fn below_a_marker(&self, marker: &ContributionId) -> Option<bool> {
        self.below_a_marker.get(marker).copied()
    }

    #[doc(hidden)]
    pub fn stats(&self) -> Stats {
        self.stats
    }
}

/// One asker's answers about its list, as [`rounds`] hands them over.
#[doc(hidden)]
pub struct Answers<'a> {
    list: &'a [ContributionId],
    bits: &'a [u64],
}

impl Answers<'_> {
    /// Whether `a` is a strict ancestor of the asker; `None` if `a` isn't in its list.
    pub fn get(&self, a: &ContributionId) -> Option<bool> {
        let slot = self.list.binary_search(a).ok()?;
        Some(bit(self.bits, slot))
    }
}

/// Answers, for each of `askers` (an asker and the index of its list in `lists`), whether each
/// contribution of its list is a strict ancestor, and calls `verdict` with each asker's index in `askers`
/// and its answers. Each list must be sorted and without repeats. The answers are freed in rounds: a round
/// holds at most one word per contribution, unless one asker alone needs more, so a large list asked by
/// many takes more passes rather than more memory.
#[doc(hidden)]
pub fn rounds(
    g: &Graph,
    lists: &[Vec<ContributionId>],
    askers: &[(ContributionId, usize)],
    mut verdict: impl FnMut(usize, Answers<'_>),
) -> Stats {
    let mut passes = Passes::new(g);
    let budget = passes.len();
    // The lists side by side: list `l` holds targets `offsets[l]..offsets[l + 1]`, and the target `t` is at
    // position `targets[t]` (`u32::MAX` if the graph doesn't hold it).
    let mut offsets = vec![0];
    let mut targets: Vec<u32> = Vec::new();
    for list in lists {
        debug_assert!(list.windows(2).all(|w| w[0] < w[1]), "sorted, without repeats");
        targets.extend(list.iter().map(|t| passes.position(t).map_or(u32::MAX, |at| at as u32)));
        offsets.push(targets.len());
    }
    let words_for = |l: usize| lists[l].len().div_ceil(64);
    let mut order: Vec<usize> = (0..askers.len()).collect();
    order.sort_by_key(|k| askers[*k].1);
    let mut peak = 0;
    let mut first = 0;
    while first < order.len() {
        // A round: the next askers, in list order, while their answers fit the budget.
        let mut used = 0;
        let mut end = first;
        while end < order.len() {
            let words = words_for(askers[order[end]].1);
            if end > first && used + words > budget {
                break;
            }
            used += words;
            end += 1;
        }
        let round = &order[first..end];
        peak = peak.max(used);
        let mut bits: Vec<Vec<u64>> = round.iter().map(|k| vec![0; words_for(askers[*k].1)]).collect();
        let at: Vec<Option<usize>> = round.iter().map(|k| passes.position(&askers[*k].0)).collect();
        // The chunks of 64 targets the round's lists meet, ascending; a list nobody here asks about costs no
        // pass.
        let mut chunks: Vec<usize> = Vec::new();
        for k in round {
            let l = askers[*k].1;
            for chunk in offsets[l] / 64..offsets[l + 1].div_ceil(64) {
                if chunks.last().is_none_or(|last| *last < chunk) {
                    chunks.push(chunk);
                }
            }
        }
        // Askers are in list order, so those whose lists meet a chunk are a run, starting at `low`.
        let mut low = 0;
        for chunk in chunks {
            let (start, stop) = (chunk * 64, (chunk * 64 + 64).min(targets.len()));
            let owners: Vec<(usize, u64)> = (start..stop)
                .filter(|t| targets[*t] != u32::MAX)
                .map(|t| (targets[t] as usize, 1 << (t % 64)))
                .collect();
            passes.run(&owners);
            while low < round.len() && offsets[askers[round[low]].1 + 1] <= start {
                low += 1;
            }
            for i in low..round.len() {
                let l = askers[round[i]].1;
                if offsets[l] >= stop {
                    break;
                }
                let Some(at) = at[i] else { continue };
                let word = passes.below(at);
                for t in start.max(offsets[l])..stop.min(offsets[l + 1]) {
                    if word & (1 << (t % 64)) != 0 {
                        set(&mut bits[i], t - offsets[l]);
                    }
                }
            }
        }
        for (i, k) in round.iter().enumerate() {
            verdict(*k, Answers { list: &lists[askers[*k].1], bits: &bits[i] });
        }
        first = end;
    }
    let questions = askers.iter().map(|(_, l)| lists[*l].len()).sum();
    Stats { questions, peak_words: peak, list_entries: targets.len(), ..passes.stats() }
}
