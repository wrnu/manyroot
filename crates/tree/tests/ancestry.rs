//! The ancestor rule answered without a walk per question (design note e93d47ad8a20, answer b2dbc4f5e810):
//! the same answers as walking the graph, with memory linear in the history whatever its shape, and time
//! pinned by passes and words combined. Lines' own shapes, and the fallback count, are in tests/lines.rs.

use std::collections::HashMap;

use bracken_tree::ancestry::{Ancestry, Stats, latest, rounds};
use bracken_tree::ground::Index;
use bracken_tree::lines::Lines;
use manyroot::{Body, Contribution, ContributionId, Graph, GroundId, Identity};

const GROUND: GroundId = GroundId([7; 32]);

/// A deterministic random number source.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

/// A graph built in memory from claims, each after the given parents.
struct Built {
    g: Graph,
    who: Identity,
    ids: Vec<ContributionId>,
    links: usize,
}

impl Built {
    fn new() -> Built {
        Built { g: Graph::new(GROUND), who: Identity::generate("a").expect("a valid name"), ids: vec![], links: 0 }
    }

    fn add(&mut self, mut parents: Vec<ContributionId>) -> ContributionId {
        parents.sort();
        parents.dedup();
        self.links += parents.len();
        let i = self.ids.len() as u64;
        let body = Body::Claim { statement: format!("c{i}"), tags: vec![] };
        let c = Contribution::create(GROUND, &self.who, parents, i + 1, body);
        self.ids.push(c.id());
        self.g.insert(c).expect("a valid contribution");
        *self.ids.last().expect("added")
    }

    /// A chain of `n` contributions.
    fn chain(n: usize) -> Built {
        let mut b = Built::new();
        for _ in 0..n {
            let parents = b.ids.last().copied().into_iter().collect();
            b.add(parents);
        }
        b
    }

    /// The bounds every build keeps: two words per contribution, and one word combined per parent link and
    /// contribution in each pass.
    fn assert_bounded(&self, stats: Stats) {
        assert_eq!(stats.words, 2 * self.ids.len(), "{stats:?}");
        assert!(stats.ors <= stats.passes * (self.ids.len() + self.links), "{stats:?}");
        assert!(stats.peak_words <= self.ids.len().max(1), "{stats:?}");
        assert_eq!(stats.fallbacks, 0, "{stats:?}");
    }

    /// Answers every asker's list with [`rounds`], as `(asker, answers in list order)`.
    fn answer(&self, lists: &[Vec<ContributionId>], askers: &[(ContributionId, usize)]) -> (Vec<Vec<bool>>, Stats) {
        let mut answers = vec![Vec::new(); askers.len()];
        let stats = rounds(&self.g, lists, askers, |k, a| {
            answers[k] = lists[askers[k].1].iter().map(|t| a.get(t).expect("in its list")).collect();
        });
        self.assert_bounded(stats);
        (answers, stats)
    }
}

#[test]
fn ancestry_agrees_with_walking_the_graph() {
    for seed in 0..4 {
        let mut rng = Lcg(seed);
        let mut b = Built::new();
        for _ in 0..300 {
            // Mostly one parent, sometimes a merge of up to three, sometimes a new root.
            let n = b.ids.len();
            let parents: Vec<ContributionId> = match (n == 0, rng.below(10)) {
                (true, _) | (_, 0) => vec![],
                (false, 1..=2) => (0..3).map(|_| b.ids[rng.below(n)]).collect(),
                _ => vec![b.ids[n - 1 - rng.below(n.min(5))]],
            };
            b.add(parents);
        }
        let ids = b.ids.clone();
        // Random references, an id the graph doesn't hold among them, and random markers.
        let mut refs: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        for _ in 0..3000 {
            refs.entry(ids[rng.below(ids.len())]).or_default().push(ids[rng.below(ids.len())]);
        }
        refs.entry(ids[0]).or_default().push(ContributionId([9; 32]));
        let markers: Vec<ContributionId> = ids.iter().filter(|_| rng.below(20) == 0).copied().collect();
        let ancestry = Ancestry::build(&b.g, &refs, &markers);
        b.assert_bounded(ancestry.stats());
        let mut yes = 0;
        for (asker, named) in &refs {
            for a in named {
                let answer = ancestry.is_ancestor(a, asker);
                if !b.g.contains(a) {
                    assert_eq!(answer, None, "an id the graph lacks is kept nowhere");
                    continue;
                }
                assert_eq!(answer, Some(b.g.is_ancestor(a, asker)), "seed {seed}: {a} before {asker}");
                yes += usize::from(answer == Some(true));
            }
        }
        assert!(yes > 100, "seed {seed}: the graph is deep enough to test ({yes})");
        for m in &markers {
            let walked = markers.iter().any(|other| b.g.is_ancestor(other, m));
            assert_eq!(ancestry.below_a_marker(m), Some(walked), "seed {seed}: marker {m}");
        }
        assert_eq!(ancestry.below_a_marker(&ContributionId([9; 32])), None);

        // Seven random lists of different sizes, an id the graph doesn't hold in one, and a sparse one of
        // early contributions (finding 09c403532fc9). Each is asked by random askers.
        let n = ids.len();
        let mut lists: Vec<Vec<ContributionId>> =
            (0..7).map(|l| (0..l * 30).map(|_| ids[rng.below(n)]).collect()).collect();
        let order = b.g.topo_order();
        lists.push((0..8).map(|_| order[rng.below(n / 10)]).collect());
        lists[3].push(ContributionId([9; 32]));
        for list in &mut lists {
            list.sort();
            list.dedup();
        }
        let askers: Vec<(ContributionId, usize)> =
            (0..400).map(|_| (ids[rng.below(n)], rng.below(lists.len()))).collect();
        let (answers, stats) = b.answer(&lists, &askers);
        // Askers that share a past share a vector, and passes start at their first target.
        assert!(stats.vectors < askers.len(), "seed {seed}: {stats:?}");
        assert!(stats.ors < stats.passes * (n + b.links), "seed {seed}: {stats:?}");
        // A list of late contributions, alone in its chunk, asked only by early askers: its pass is skipped.
        lists.push((0..8).map(|_| order[n - 1 - rng.below(n / 10)]).collect());
        lists.last_mut().expect("pushed").sort();
        lists.last_mut().expect("pushed").dedup();
        let early: Vec<(ContributionId, usize)> = (0..20).map(|_| (order[rng.below(n / 2)], 0)).collect();
        let late = &lists[lists.len() - 1..];
        let (early_answers, skipping) = b.answer(late, &early);
        assert!(skipping.skipped > 0 && skipping.passes == 0, "seed {seed}: {skipping:?}");
        // Each compared against the walk.
        let all = askers.iter().zip(&answers).map(|((a, l), x)| (a, &lists[*l], x));
        for (asker, list, answer) in all.chain(early.iter().zip(&early_answers).map(|((a, _), x)| (a, &late[0], x))) {
            let walked: Vec<bool> = list.iter().map(|t| b.g.is_ancestor(t, asker)).collect();
            assert_eq!(answer, &walked, "seed {seed}: asker {asker}");
        }
    }
}

/// Finding 83f757301c59: many markers are one target, with a bit each, not a question per pair.
#[test]
fn many_markers_cost_one_pass_and_a_bit_each() {
    const M: usize = 3_000;
    let b = Built::chain(M);
    let markers = b.ids.clone();
    let ancestry = Ancestry::build(&b.g, &HashMap::new(), &markers);
    let stats = ancestry.stats();
    b.assert_bounded(stats);
    assert_eq!((stats.passes, stats.questions), (1, M));
    assert!(stats.bytes <= M * 64, "a bit and an entry per marker: {stats:?}");
    assert_eq!(ancestry.below_a_marker(&markers[0]), Some(false));
    assert!(markers[1..].iter().all(|m| ancestry.below_a_marker(m) == Some(true)));
}

/// Finding 54f6e7658aee: a merge of K roots, each root asking about the one before, holds two words per
/// contribution, not K live sets; and K roots are no more than K contributions.
#[test]
fn a_wide_merge_of_many_roots_holds_two_words_per_contribution() {
    const K: usize = 10_000;
    let mut b = Built::new();
    let roots: Vec<ContributionId> = (0..K).map(|_| b.add(vec![])).collect();
    let merge = b.add(roots.clone());
    let mut refs: HashMap<ContributionId, Vec<ContributionId>> = roots.windows(2).map(|w| (w[1], vec![w[0]])).collect();
    refs.insert(merge, vec![roots[0], roots[K - 1]]);
    let ancestry = Ancestry::build(&b.g, &refs, &[]);
    b.assert_bounded(ancestry.stats());
    assert_eq!(ancestry.stats().passes, (K + 1).div_ceil(64));
    assert_eq!(ancestry.is_ancestor(&roots[0], &roots[1]), Some(false));
    assert_eq!(ancestry.is_ancestor(&roots[K - 1], &merge), Some(true));
    assert_eq!(ancestry.is_ancestor(&roots[0], &merge), Some(true));
    // The roots as one list, asked by the merge and by 64 roots: the merge has every root below it, a root
    // none.
    let mut list = roots.clone();
    list.sort();
    let askers: Vec<(ContributionId, usize)> = roots[..64].iter().chain([&merge]).map(|id| (*id, 0)).collect();
    let (answers, stats) = b.answer(&[list], &askers);
    assert!(answers[64].iter().all(|a| *a) && answers[..64].iter().all(|a| a.iter().all(|a| !*a)));
    // The roots share an empty past, so one vector for them and one for the merge, in one round.
    assert_eq!((stats.vectors, stats.passes), (2, K.div_ceil(64)), "{stats:?}");
}

/// Finding 0a799e7393e9: a long chain where each contribution asks about its parent and the root, as
/// decisions ask about their parents and their line.
#[test]
fn a_long_chain_holds_two_words_per_contribution() {
    const N: usize = 10_000;
    let b = Built::chain(N);
    let ids = b.ids.clone();
    let refs: HashMap<ContributionId, Vec<ContributionId>> =
        ids.windows(2).map(|w| (w[1], vec![w[0], ids[0]])).collect();
    let ancestry = Ancestry::build(&b.g, &refs, &[]);
    b.assert_bounded(ancestry.stats());
    assert_eq!(ancestry.stats().passes, N.div_ceil(64));
    assert_eq!(ancestry.is_ancestor(&ids[0], &ids[N - 1]), Some(true));
    assert_eq!(ancestry.is_ancestor(&ids[6], &ids[5]), None, "never asked");
}

/// Answer ca694f06dc91: many distinct targets cost ceil((T + 1) / 64) passes, the time lever accepted.
#[test]
fn many_distinct_targets_cost_a_pass_per_64() {
    let mut rng = Lcg(42);
    let mut b = Built::new();
    for _ in 0..2_000 {
        let n = b.ids.len();
        let parents = if n == 0 { vec![] } else { (0..2).map(|_| b.ids[rng.below(n)]).collect() };
        b.add(parents);
    }
    let ids = b.ids.clone();
    let refs: HashMap<ContributionId, Vec<ContributionId>> = ids.windows(2).map(|w| (w[1], vec![w[0]])).collect();
    let ancestry = Ancestry::build(&b.g, &refs, &[]);
    b.assert_bounded(ancestry.stats());
    assert_eq!(ancestry.stats().passes, 32);
    for i in (1..ids.len()).step_by(37) {
        assert_eq!(ancestry.is_ancestor(&ids[i - 1], &ids[i]), Some(b.g.is_ancestor(&ids[i - 1], &ids[i])));
    }
}

/// References cost 4 bytes and a bit each: R records naming 1,024 contributions each.
#[test]
fn references_cost_four_bytes_and_a_bit() {
    const N: usize = 5_000;
    const R: usize = 200;
    let b = Built::chain(N);
    let ids = b.ids.clone();
    let refs: HashMap<ContributionId, Vec<ContributionId>> =
        (0..R).map(|r| (ids[N - 1 - r], (0..1024).map(|k| ids[(r * 7 + k * 3) % (N - R)]).collect())).collect();
    let ancestry = Ancestry::build(&b.g, &refs, &[]);
    let stats = ancestry.stats();
    let targets = refs.values().flatten().collect::<std::collections::HashSet<_>>().len();
    b.assert_bounded(stats);
    println!("{R} records of 1,024 references: {} bytes kept, {} questions", stats.bytes, stats.questions);
    assert!(stats.bytes <= stats.questions * 5 + R * 160 + targets * 64, "{stats:?}");
    assert_eq!(ancestry.is_ancestor(&ids[0], &ids[N - 1]), Some(true));
}

/// Finding 98ce07a5e421: D records written after a line's chain of L relations have the same past, so
/// they share one answer vector: one round of ceil(L / 64) passes, not D * L / (64 n) rounds of them.
#[test]
fn askers_with_the_same_past_share_one_answer_vector() {
    const L: usize = 2_000;
    const D: usize = 2_000;
    let b = Built::chain(L + D);
    let (relations, askers) = b.ids.split_at(L);
    let mut list = relations.to_vec();
    list.sort();
    let askers: Vec<(ContributionId, usize)> = askers.iter().map(|id| (*id, 0)).collect();
    let (answers, stats) = b.answer(&[list], &askers);
    assert!(answers.iter().all(|a| a.iter().all(|a| *a)));
    assert_eq!(stats.questions, D * L);
    assert_eq!(stats.list_entries, L, "the list once, however many ask about it");
    assert_eq!((stats.vectors, stats.peak_words, stats.passes), (1, L / 64 + 1, L.div_ceil(64)), "{stats:?}");
}

/// Finding 6c8ea309c1e2: askers whose parents are the one before and a side root, which holds no target,
/// have the same past, and share a vector; so do merges of two branches of the same history.
#[test]
fn merges_of_the_same_targets_share_one_answer_vector() {
    const L: usize = 500;
    const D: usize = 500;
    let mut b = Built::chain(L);
    let relations = b.ids.clone();
    let side = b.add(vec![]);
    let mut askers: Vec<ContributionId> = vec![];
    for _ in 0..D {
        let previous = *askers.last().unwrap_or(&relations[L - 1]);
        askers.push(b.add(vec![previous, side]));
    }
    // Two branches from the last asker, each a claim, then their merge.
    let last = *askers.last().expect("an asker");
    let (left, right) = (b.add(vec![last]), b.add(vec![last]));
    askers.push(b.add(vec![left, right]));
    let mut list = relations;
    list.sort();
    let askers: Vec<(ContributionId, usize)> = askers.iter().map(|id| (*id, 0)).collect();
    let (answers, stats) = b.answer(&[list], &askers);
    assert!(answers.iter().all(|a| a.iter().all(|a| *a)));
    assert_eq!((stats.vectors, stats.passes), (1, L.div_ceil(64)), "{stats:?}");
}

/// The batch budget (answer b2dbc4f5e810): when the D askers' pasts all differ (each written after one
/// more of the L relations), they hold at most a word per contribution at a time, so the list is answered
/// in rounds, more passes rather than more memory. A round's pass over a chunk sweeps only to its last
/// asker, and is skipped for chunks written after it (finding 98ce07a5e421).
#[test]
fn distinct_pasts_are_answered_in_rounds_within_a_word_per_contribution() {
    const L: usize = 2_000;
    const D: usize = 2_000;
    let b = Built::chain(L + D);
    // Relation, asker, relation, asker...
    let mut list: Vec<ContributionId> = b.ids.iter().step_by(2).copied().collect();
    list.sort();
    let askers: Vec<(ContributionId, usize)> = b.ids.iter().skip(1).step_by(2).map(|id| (*id, 0)).collect();
    let (answers, stats) = b.answer(&[list.clone()], &askers);
    for (k, (asker, _)) in askers.iter().enumerate() {
        assert_eq!(answers[k].iter().filter(|a| **a).count(), k + 1, "{asker}");
    }
    assert_eq!(stats.vectors, D);
    // 32 words a vector, so 125 vectors a round within 4,000 words: 16 rounds. Without pruning, each would
    // pass over all 32 chunks; round r reaches only the chunks before its last asker, about 2(r + 1).
    let rounds = D.div_ceil(125);
    assert_eq!(stats.peak_words, 125 * 32, "{stats:?}");
    let reached: usize = (0..rounds).map(|r| (2 * 125 * (r + 1) - 1).div_ceil(128).min(L.div_ceil(64))).sum();
    assert_eq!(stats.passes, reached, "{stats:?}");
    assert!(stats.passes < rounds * L.div_ceil(64) * 6 / 10, "{stats:?}");
}

/// Many small lists, each asked once, share passes: a pass per 64 relations, not one per list.
#[test]
fn many_small_lists_share_passes() {
    const N: usize = 1_000;
    let b = Built::chain(2 * N);
    let lists: Vec<Vec<ContributionId>> = b.ids[..N].iter().map(|id| vec![*id]).collect();
    let askers: Vec<(ContributionId, usize)> = (0..N).map(|l| (b.ids[N + l], l)).collect();
    let (answers, stats) = b.answer(&lists, &askers);
    assert!(answers.iter().all(|a| a == &[true]));
    assert_eq!(stats.passes, N.div_ceil(64), "{stats:?}");
}

/// The index and the lines can move between threads, as they could before the ancestor index.
#[test]
fn the_index_is_send() {
    fn send<T: Send>() {}
    send::<Index>();
    send::<Ancestry>();
    send::<Lines>();
}

/// The latest-result rule's engine: for random groups on random graphs, a member is latest exactly when
/// no member of its group descends from it, as the walk says; an id the graph lacks is never latest,
/// and an empty group costs nothing.
#[test]
fn latest_agrees_with_walking_the_graph() {
    for seed in 0..4 {
        let mut rng = Lcg(seed + 10);
        let mut b = Built::new();
        for _ in 0..300 {
            let n = b.ids.len();
            let parents: Vec<ContributionId> = match (n == 0, rng.below(10)) {
                (true, _) | (_, 0) => vec![],
                (false, 1..=2) => (0..3).map(|_| b.ids[rng.below(n)]).collect(),
                _ => vec![b.ids[n - 1 - rng.below(n.min(5))]],
            };
            b.add(parents);
        }
        let ids = b.ids.clone();
        let mut groups: Vec<Vec<ContributionId>> = (0..12)
            .map(|k| {
                let mut group: Vec<ContributionId> = (0..k * 9).map(|_| ids[rng.below(ids.len())]).collect();
                group.sort();
                group.dedup();
                group
            })
            .collect();
        groups[5].push(ContributionId([9; 32]));
        let (answers, stats) = latest(&b.g, &groups);
        assert_eq!(stats.fallbacks, 0);
        let mut some_not_latest = false;
        for (group, answer) in groups.iter().zip(&answers) {
            for (member, is_latest) in group.iter().zip(answer) {
                let walked = b.g.contains(member) && !group.iter().any(|other| b.g.is_ancestor(member, other));
                assert_eq!(*is_latest, walked, "seed {seed}: {member}");
                some_not_latest |= b.g.contains(member) && !walked;
            }
        }
        assert!(some_not_latest, "seed {seed}: the groups are deep enough to test");
        assert!(stats.passes <= stats.list_entries.div_ceil(64), "{stats:?}");
    }
}
