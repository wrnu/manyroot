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
    /// Bytes kept once built, by the vectors' capacities and the maps' entries (not their slack). For
    /// Lines, what its chain questions hold until its records are judged.
    pub bytes: usize,
    /// Answer vectors [`rounds`] computed: one per list and class of askers' pasts.
    pub vectors: usize,
    /// Passes [`rounds`] skipped, as no reader of a chunk could have its targets below it.
    pub skipped: usize,
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
    /// The positions the last pass swept: `below` is 0 before them, and unset after.
    swept: (usize, usize),
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
        Passes { position, starts, parents, own: vec![0; n], below: vec![0; n], swept: (n, 0), passes: 0, ors: 0 }
    }

    pub(crate) fn len(&self) -> usize {
        self.own.len()
    }

    pub(crate) fn position(&self, id: &ContributionId) -> Option<usize> {
        self.position.get(id).copied()
    }

    /// One pass: `owners` are the contributions asked about, each with its bit. Afterwards, until the
    /// next pass, [`Passes::below`] gives each contribution up to position `last` the bits of the owners
    /// that are its strict ancestors. Nothing before the first owner can have one, so the pass sweeps
    /// only from there to `last`, and not at all if that is empty.
    pub(crate) fn run(&mut self, owners: &[(usize, u64)], last: usize) -> bool {
        let first = owners.iter().map(|(at, _)| *at).min().unwrap_or(self.len());
        let last = last.min(self.len().saturating_sub(1));
        self.swept = (first, last);
        if first >= last {
            return false;
        }
        for (at, b) in owners {
            self.own[*at] |= b;
        }
        for at in first..=last {
            let (from, to) = (self.starts[at] as usize, self.starts[at + 1] as usize);
            let mut word = 0;
            for p in &self.parents[from..to] {
                let p = *p as usize;
                if p >= first {
                    word |= self.below[p] | self.own[p];
                }
            }
            self.below[at] = word;
            self.ors += to - from + 1;
        }
        for (at, _) in owners {
            self.own[*at] = 0;
        }
        self.passes += 1;
        true
    }

    pub(crate) fn below(&self, at: usize) -> u64 {
        let (first, last) = self.swept;
        debug_assert!(at <= last || first >= last, "position {at} is past the pass's last, {last}");
        if at <= first || at > last { 0 } else { self.below[at] }
    }

    /// A class for each contribution, such that two of the same class have the same `targets` among
    /// their strict ancestors. A contribution's targets below are those of its parents, and those of them
    /// that are targets; so its class is fixed by its parents' classes, leaving out class 0 (roots and
    /// whatever has no target below), and its target parents. With one such class and no target parent
    /// it is that class; otherwise the class of that combination, made when first seen, so honest merges
    /// of the same history share one (finding 6c8ea309c1e2). The combinations held while classing are
    /// linear in the parent links.
    fn classes(&self, targets: &[bool]) -> Vec<u32> {
        let mut class = vec![0u32; self.len()];
        let mut combinations: HashMap<Vec<u64>, u32> = HashMap::new();
        for at in 0..self.len() {
            let parents = &self.parents[self.starts[at] as usize..self.starts[at + 1] as usize];
            // Classes as themselves, target parents above every class.
            let mut key: Vec<u64> = parents
                .iter()
                .map(|p| match targets[*p as usize] {
                    true => (1 << 32) | u64::from(*p),
                    false => u64::from(class[*p as usize]),
                })
                .filter(|k| *k != 0)
                .collect();
            key.sort_unstable();
            key.dedup();
            class[at] = match key.as_slice() {
                [] => 0,
                [one] if *one < 1 << 32 => *one as u32,
                _ => {
                    let next = combinations.len() as u32 + 1;
                    *combinations.entry(key).or_insert(next)
                }
            };
        }
        class
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
            passes.run(owners, usize::MAX);
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
/// and its answers. Each list must be sorted and without repeats.
///
/// Askers whose pasts provably hold the same targets ([`Passes::classes`]) and who ask about the same
/// list share one answer vector, so many records written after a line's chain cost one. The vectors are
/// answered and freed in rounds: a round holds at most one word per contribution, unless one vector alone
/// needs more, so a large list asked by many distinct pasts takes more passes rather than more memory.
/// A pass over a chunk of 64 targets sweeps only from the first of them to the round's last vector that
/// reads it, and is skipped if none can have them below it.
#[doc(hidden)]
pub fn rounds(
    g: &Graph,
    lists: &[Vec<ContributionId>],
    askers: &[(ContributionId, usize)],
    mut verdict: impl FnMut(usize, Answers<'_>),
) -> Stats {
    let mut passes = Passes::new(g);
    let budget = passes.len();
    // The lists side by side: list `l` holds targets `offsets[l]..offsets[l + 1]`, each list's in causal
    // order, so a chunk of 64 starts as late as it can. The target `t` is at position `targets[t]`
    // (`u32::MAX` if the graph doesn't hold it, first), and is entry `slots[t]` of its list.
    let mut offsets = vec![0];
    let mut targets: Vec<u32> = Vec::new();
    let mut slots: Vec<u32> = Vec::new();
    for list in lists {
        debug_assert!(list.windows(2).all(|w| w[0] < w[1]), "sorted, without repeats");
        let mut by_position: Vec<(u32, u32)> = list
            .iter()
            .enumerate()
            .map(|(slot, t)| (passes.position(t).map_or(u32::MAX, |at| at as u32), slot as u32))
            .collect();
        by_position.sort_unstable_by_key(|(at, _)| at.wrapping_add(1));
        targets.extend(by_position.iter().map(|(at, _)| *at));
        slots.extend(by_position.iter().map(|(_, slot)| *slot));
        offsets.push(targets.len());
    }
    // One answer vector per list and class: its list, the earliest position among its askers (any would
    // do, their pasts being the same; `None` off the graph), and its askers.
    let mut vectors: Vec<(usize, Option<usize>, Vec<usize>)> = Vec::new();
    {
        let mut is_target = vec![false; passes.len()];
        for t in &targets {
            if *t != u32::MAX {
                is_target[*t as usize] = true;
            }
        }
        let class = passes.classes(&is_target);
        let mut vector_of: HashMap<(usize, Option<u32>), usize> = HashMap::new();
        for (k, (asker, l)) in askers.iter().enumerate() {
            let at = passes.position(asker);
            let next = vectors.len();
            let v = *vector_of.entry((*l, at.map(|at| class[at]))).or_insert(next);
            if v == next {
                vectors.push((*l, at, Vec::new()));
            }
            let vector = &mut vectors[v];
            vector.1 = vector.1.min(at).or(at);
            vector.2.push(k);
        }
    }
    vectors.sort_by_key(|(l, at, _)| (*l, *at));
    let words_for = |l: usize| lists[l].len().div_ceil(64);
    let (mut peak, mut skipped) = (0, 0);
    let mut first = 0;
    while first < vectors.len() {
        // A round: the next vectors, in list order, while they fit the budget.
        let mut used = 0;
        let mut end = first;
        while end < vectors.len() {
            let words = words_for(vectors[end].0);
            if end > first && used + words > budget {
                break;
            }
            used += words;
            end += 1;
        }
        let round = &vectors[first..end];
        let mut bits: Vec<Vec<u64>> = round.iter().map(|(l, _, _)| vec![0; words_for(*l)]).collect();
        peak = peak.max(bits.iter().map(Vec::capacity).sum());
        // The chunks of 64 targets the round's lists meet, ascending; a list nobody here asks about costs no
        // pass.
        let mut chunks: Vec<usize> = Vec::new();
        for (l, _, _) in round {
            for chunk in offsets[*l] / 64..offsets[*l + 1].div_ceil(64) {
                if chunks.last().is_none_or(|last| *last < chunk) {
                    chunks.push(chunk);
                }
            }
        }
        // Vectors are in list order, so those whose lists meet a chunk are a run, from `low` to `high`.
        let mut low = 0;
        for chunk in chunks {
            let (start, stop) = (chunk * 64, (chunk * 64 + 64).min(targets.len()));
            while low < round.len() && offsets[round[low].0 + 1] <= start {
                low += 1;
            }
            let high = low + round[low..].iter().take_while(|(l, _, _)| offsets[*l] < stop).count();
            let Some(last) = round[low..high].iter().filter_map(|(_, at, _)| *at).max() else {
                skipped += 1;
                continue;
            };
            let owners: Vec<(usize, u64)> = (start..stop)
                .filter(|t| targets[*t] != u32::MAX)
                .map(|t| (targets[t] as usize, 1 << (t % 64)))
                .collect();
            if !passes.run(&owners, last) {
                skipped += 1;
            }
            for (i, (l, at, _)) in round.iter().enumerate().take(high).skip(low) {
                let Some(at) = at else { continue };
                let word = passes.below(*at);
                let mine = start.max(offsets[*l])..stop.min(offsets[*l + 1]);
                for (t, slot) in mine.clone().zip(&slots[mine]) {
                    if word & (1 << (t % 64)) != 0 {
                        set(&mut bits[i], *slot as usize);
                    }
                }
            }
        }
        for (i, (l, _, members)) in round.iter().enumerate() {
            for k in members {
                verdict(*k, Answers { list: &lists[*l], bits: &bits[i] });
            }
        }
        first = end;
    }
    let questions = askers.iter().map(|(_, l)| lists[*l].len()).sum();
    Stats {
        questions,
        vectors: vectors.len(),
        skipped,
        peak_words: peak,
        list_entries: targets.len(),
        ..passes.stats()
    }
}
