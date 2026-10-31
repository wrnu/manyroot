//! A persistent set of `u32`s: a fixed-shape trie of 64-bit words, 16 ways at each of seven levels,
//! whose versions share every subtree they have in common. Adding a value copies one path; a union or
//! difference of two versions skips the subtrees they share, and otherwise reads a word per 64 values.
//! [`SetMap`] maps `u32`s to such sets the same way, with a set at each leaf.

use std::sync::Arc;

/// Branch levels above the words: 6 bits in a word and 4 per level cover a `u32`.
const LEVELS: u32 = 7;

#[derive(Debug, Clone, Default)]
pub(crate) struct Set(Option<Arc<Node>>);

#[derive(Debug)]
enum Node {
    Word(u64),
    Branch([Set; 16]),
}

/// The branch `x` takes at `level`, the top being 0.
fn slot(x: u32, level: u32) -> usize {
    ((x >> (6 + 4 * (LEVELS - 1 - level))) & 15) as usize
}

impl Set {
    fn node(node: Node) -> Set {
        Set(Some(Arc::new(node)))
    }

    fn same(&self, other: &Set) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    pub(crate) fn contains(&self, x: u32) -> bool {
        let (mut set, mut level) = (self, 0);
        loop {
            match set.0.as_deref() {
                None => return false,
                Some(Node::Word(bits)) => return bits >> (x & 63) & 1 == 1,
                Some(Node::Branch(children)) => {
                    set = &children[slot(x, level)];
                    level += 1;
                }
            }
        }
    }

    /// This set with `x` in it.
    pub(crate) fn with(&self, x: u32) -> Set {
        self.with_at(x, 0)
    }

    fn with_at(&self, x: u32, level: u32) -> Set {
        if level == LEVELS {
            let bits = match self.0.as_deref() {
                Some(Node::Word(bits)) => *bits,
                _ => 0,
            };
            let more = bits | 1 << (x & 63);
            return if more == bits { self.clone() } else { Set::node(Node::Word(more)) };
        }
        let mut children: [Set; 16] = match self.0.as_deref() {
            Some(Node::Branch(children)) => children.clone(),
            _ => Default::default(),
        };
        let at = slot(x, level);
        let child = children[at].with_at(x, level + 1);
        if child.same(&children[at]) {
            return self.clone();
        }
        children[at] = child;
        Set::node(Node::Branch(children))
    }

    /// Every value in either set.
    pub(crate) fn union(&self, other: &Set) -> Set {
        match (self.0.as_deref(), other.0.as_deref()) {
            _ if self.same(other) => self.clone(),
            (None, _) => other.clone(),
            (_, None) => self.clone(),
            (Some(Node::Word(a)), Some(Node::Word(b))) => match a | b {
                both if both == *a => self.clone(),
                both if both == *b => other.clone(),
                both => Set::node(Node::Word(both)),
            },
            (Some(Node::Branch(a)), Some(Node::Branch(b))) => {
                let children: [Set; 16] = std::array::from_fn(|i| a[i].union(&b[i]));
                if children.iter().zip(a).all(|(c, a)| c.same(a)) {
                    self.clone()
                } else if children.iter().zip(b).all(|(c, b)| c.same(b)) {
                    other.clone()
                } else {
                    Set::node(Node::Branch(children))
                }
            }
            _ => unreachable!("a trie of fixed shape"),
        }
    }

    /// Pushes the values in this set and not in `other` to `out`, ascending.
    pub(crate) fn difference(&self, other: &Set, out: &mut Vec<u32>) {
        self.difference_at(other, 0, 0, out);
    }

    fn difference_at(&self, other: &Set, level: u32, base: u32, out: &mut Vec<u32>) {
        if self.same(other) {
            return;
        }
        match (self.0.as_deref(), other.0.as_deref()) {
            (None, _) => {}
            (Some(Node::Word(a)), b) => {
                let mut bits = match b {
                    Some(Node::Word(b)) => a & !b,
                    _ => *a,
                };
                while bits != 0 {
                    out.push(base + bits.trailing_zeros());
                    bits &= bits - 1;
                }
            }
            (Some(Node::Branch(a)), b) => {
                let empty = Set::default();
                for (i, child) in a.iter().enumerate() {
                    let theirs = match b {
                        Some(Node::Branch(b)) => &b[i],
                        _ => &empty,
                    };
                    let base = base + ((i as u32) << (6 + 4 * (LEVELS - 1 - level)));
                    child.difference_at(theirs, level + 1, base, out);
                }
            }
        }
    }
}

/// Key levels: 4 bits per level cover a `u32`.
const KEY_LEVELS: u32 = 8;

/// A persistent map from `u32` to [`Set`], shaped as [`Set`] is, so that adding to one key copies one
/// path, and a union of two versions skips the subtrees they share or that one of them lacks.
#[derive(Debug, Clone, Default)]
pub(crate) struct SetMap(Option<Arc<MapNode>>);

#[derive(Debug)]
enum MapNode {
    Leaf(Set),
    Branch([SetMap; 16]),
}

fn key_slot(k: u32, level: u32) -> usize {
    ((k >> (4 * (KEY_LEVELS - 1 - level))) & 15) as usize
}

impl SetMap {
    fn node(node: MapNode) -> SetMap {
        SetMap(Some(Arc::new(node)))
    }

    fn same(&self, other: &SetMap) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// The set at `k`, empty if there is none.
    pub(crate) fn get(&self, k: u32) -> Set {
        let (mut map, mut level) = (self, 0);
        loop {
            match map.0.as_deref() {
                None => return Set::default(),
                Some(MapNode::Leaf(set)) => return set.clone(),
                Some(MapNode::Branch(children)) => {
                    map = &children[key_slot(k, level)];
                    level += 1;
                }
            }
        }
    }

    /// This map with `set` at `k`.
    pub(crate) fn with(&self, k: u32, set: Set) -> SetMap {
        self.with_at(k, set, 0)
    }

    fn with_at(&self, k: u32, set: Set, level: u32) -> SetMap {
        if level == KEY_LEVELS {
            return match self.0.as_deref() {
                Some(MapNode::Leaf(old)) if old.same(&set) => self.clone(),
                _ => SetMap::node(MapNode::Leaf(set)),
            };
        }
        let mut children: [SetMap; 16] = match self.0.as_deref() {
            Some(MapNode::Branch(children)) => children.clone(),
            _ => Default::default(),
        };
        let at = key_slot(k, level);
        let child = children[at].with_at(k, set, level + 1);
        if child.same(&children[at]) {
            return self.clone();
        }
        children[at] = child;
        SetMap::node(MapNode::Branch(children))
    }

    /// At each key, the union of the two maps' sets.
    pub(crate) fn union(&self, other: &SetMap) -> SetMap {
        match (self.0.as_deref(), other.0.as_deref()) {
            _ if self.same(other) => self.clone(),
            (None, _) => other.clone(),
            (_, None) => self.clone(),
            (Some(MapNode::Leaf(a)), Some(MapNode::Leaf(b))) => {
                let both = a.union(b);
                if both.same(a) {
                    self.clone()
                } else if both.same(b) {
                    other.clone()
                } else {
                    SetMap::node(MapNode::Leaf(both))
                }
            }
            (Some(MapNode::Branch(a)), Some(MapNode::Branch(b))) => {
                let children: [SetMap; 16] = std::array::from_fn(|i| a[i].union(&b[i]));
                if children.iter().zip(a).all(|(c, a)| c.same(a)) {
                    self.clone()
                } else if children.iter().zip(b).all(|(c, b)| c.same(b)) {
                    other.clone()
                } else {
                    SetMap::node(MapNode::Branch(children))
                }
            }
            _ => unreachable!("a trie of fixed shape"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::Set;

    /// Versions built by adding and joining, each against a `BTreeSet`, over values near and far apart.
    #[test]
    fn versions_agree_with_a_btreeset() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut versions: Vec<(Set, BTreeSet<u32>)> = vec![(Set::default(), BTreeSet::new())];
        for _ in 0..2_000 {
            let r = next();
            let (a, b) = (r as usize % versions.len(), (r >> 20) as usize % versions.len());
            let made = match r >> 40 & 3 {
                0 => {
                    let x = match r >> 42 & 1 {
                        0 => (r >> 43) as u32 % 300,
                        _ => (r >> 12) as u32,
                    };
                    let (set, reference) = &versions[a];
                    let mut more = reference.clone();
                    more.insert(x);
                    (set.with(x), more)
                }
                1 => {
                    let union = versions[a].0.union(&versions[b].0);
                    (union, versions[a].1.union(&versions[b].1).copied().collect())
                }
                _ => versions[a].clone(),
            };
            versions.push(made);
        }
        for (i, (set, reference)) in versions.iter().enumerate() {
            let mut all = Vec::new();
            set.difference(&Set::default(), &mut all);
            assert_eq!(all, reference.iter().copied().collect::<Vec<_>>(), "version {i}");
            let other = &versions[(i * 7919) % versions.len()];
            let mut less = Vec::new();
            set.difference(&other.0, &mut less);
            assert_eq!(less, reference.difference(&other.1).copied().collect::<Vec<_>>(), "version {i}");
            assert!(reference.iter().all(|x| set.contains(*x)));
            assert!(!set.contains(u32::MAX) || reference.contains(&u32::MAX));
        }
    }

    /// Maps built by setting keys and joining, each against a `BTreeMap` of `BTreeSet`s.
    #[test]
    fn maps_agree_with_a_btreemap() {
        use std::collections::BTreeMap;

        use super::SetMap;
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let as_set = |r: &BTreeSet<u32>| r.iter().fold(Set::default(), |all, x| all.with(*x));
        let mut versions: Vec<(SetMap, BTreeMap<u32, BTreeSet<u32>>)> = vec![(SetMap::default(), BTreeMap::new())];
        for _ in 0..1_000 {
            let r = next();
            let (a, b) = (r as usize % versions.len(), (r >> 20) as usize % versions.len());
            let made = match r >> 40 & 1 {
                0 => {
                    let k = match r >> 41 & 1 {
                        0 => (r >> 42) as u32 % 40,
                        _ => (r >> 9) as u32,
                    };
                    let (map, reference) = &versions[a];
                    let mut more = reference.clone();
                    more.entry(k).or_default().insert((r >> 50) as u32 % 200);
                    (map.with(k, as_set(&more[&k])), more)
                }
                _ => {
                    let mut both = versions[a].1.clone();
                    for (k, v) in &versions[b].1 {
                        both.entry(*k).or_default().extend(v);
                    }
                    (versions[a].0.union(&versions[b].0), both)
                }
            };
            versions.push(made);
        }
        for (i, (map, reference)) in versions.iter().enumerate() {
            for (k, v) in reference {
                let mut all = Vec::new();
                map.get(*k).difference(&Set::default(), &mut all);
                assert_eq!(all, v.iter().copied().collect::<Vec<_>>(), "version {i}, key {k}");
            }
            let missing = (0..u32::MAX).step_by(1 << 28).find(|k| !reference.contains_key(k)).expect("a free key");
            let mut none = Vec::new();
            map.get(missing).difference(&Set::default(), &mut none);
            assert!(none.is_empty(), "version {i}");
        }
    }
}
