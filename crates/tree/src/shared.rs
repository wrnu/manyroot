//! A persistent set of `u32`s: a fixed-shape trie of 64-bit words, 16 ways at each of seven levels,
//! whose versions share every subtree they have in common. Adding a value copies one path; a union or
//! difference of two versions skips the subtrees they share, and otherwise reads a word per 64 values.

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
}
