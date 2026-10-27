//! Roots: the causal graph of a Ground's contributions.
//!
//! The graph is a grow-only set of contributions, so merging two replicas is
//! set union and is commutative, associative and idempotent: a CRDT without
//! any merge logic. What keeps replicas in agreement about *validity* is that
//! every rule in [`Graph::check`] depends only on a contribution and its
//! parents, which must already be present before it can be added.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::contribution::{Body, Contribution, MAX_CONTRIBUTION_BYTES, PROTOCOL_VERSION};
use crate::id::{ContributionId, GroundId};

/// Shortest id prefix accepted by [`Graph::resolve_prefix`].
pub const MIN_PREFIX_LEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleViolation {
    #[error("unsupported protocol version {0}")]
    UnsupportedProtocol(u32),
    #[error("belongs to ground {found}, not {expected}")]
    WrongGround { expected: GroundId, found: GroundId },
    #[error("signature is not valid for the author")]
    BadSignature,
    #[error("parents are not strictly ascending")]
    ParentsNotCanonical,
    #[error("parent {0} is not in the graph")]
    MissingParent(ContributionId),
    #[error("references {0} without listing it as a parent")]
    UnlistedReference(ContributionId),
    #[error("a relation cannot point from a contribution to itself")]
    SelfRelation,
    #[error("only the author of {0} can retract it")]
    RetractByOther(ContributionId),
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("encoded size {0} exceeds {MAX_CONTRIBUTION_BYTES} bytes")]
    TooLarge(usize),
}

#[derive(Debug, thiserror::Error)]
#[error("prefix {prefix:?}: {kind}")]
pub struct ResolveError {
    pub prefix: String,
    pub kind: ResolveErrorKind,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveErrorKind {
    #[error("use at least {MIN_PREFIX_LEN} hex characters")]
    TooShort,
    #[error("not a hex string")]
    NotHex,
    #[error("no matching contribution")]
    NotFound,
    #[error("matches {0} contributions")]
    Ambiguous(usize),
}

/// Result of importing a batch of contributions that may arrive in any order.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Newly added, in the order they were added (parents before children).
    pub added: Vec<ContributionId>,
    pub already_present: usize,
    pub rejected: Vec<(ContributionId, RuleViolation)>,
    /// Never became addable because an ancestor is missing or was rejected.
    pub unresolved: Vec<ContributionId>,
}

impl ImportReport {
    pub fn is_clean(&self) -> bool {
        self.rejected.is_empty() && self.unresolved.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Graph {
    ground: GroundId,
    contributions: HashMap<ContributionId, Contribution>,
    children: HashMap<ContributionId, Vec<ContributionId>>,
    /// Relations, evidence and retractions that reference a contribution.
    referrers: HashMap<ContributionId, Vec<ContributionId>>,
    heads: BTreeSet<ContributionId>,
}

impl Graph {
    pub fn new(ground: GroundId) -> Self {
        Self {
            ground,
            contributions: HashMap::new(),
            children: HashMap::new(),
            referrers: HashMap::new(),
            heads: BTreeSet::new(),
        }
    }

    pub fn ground(&self) -> GroundId {
        self.ground
    }

    pub fn len(&self) -> usize {
        self.contributions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contributions.is_empty()
    }

    pub fn contains(&self, id: &ContributionId) -> bool {
        self.contributions.contains_key(id)
    }

    pub fn get(&self, id: &ContributionId) -> Option<&Contribution> {
        self.contributions.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Contribution> {
        self.contributions.values()
    }

    /// Causal heads: contributions nothing else builds on yet. A new
    /// contribution normally lists all of them as parents.
    pub fn heads(&self) -> impl Iterator<Item = ContributionId> + '_ {
        self.heads.iter().copied()
    }

    pub fn children(&self, id: &ContributionId) -> &[ContributionId] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    /// Relations, evidence and retractions that reference `id`.
    pub fn referrers(&self, id: &ContributionId) -> impl Iterator<Item = &Contribution> {
        self.referrers.get(id).into_iter().flatten().map(|r| &self.contributions[r])
    }

    /// Checks every protocol rule. Rules only look at `c` and its parents, so
    /// any two replicas holding those parents reach the same answer.
    pub fn check(&self, c: &Contribution) -> Result<(), RuleViolation> {
        let content = c.content();
        if content.protocol != PROTOCOL_VERSION {
            return Err(RuleViolation::UnsupportedProtocol(content.protocol));
        }
        if content.ground != self.ground {
            return Err(RuleViolation::WrongGround { expected: self.ground, found: content.ground });
        }
        let size = c.encode().len();
        if size > MAX_CONTRIBUTION_BYTES {
            return Err(RuleViolation::TooLarge(size));
        }
        if !c.signature_is_valid() {
            return Err(RuleViolation::BadSignature);
        }
        if !content.parents.windows(2).all(|w| w[0] < w[1]) {
            return Err(RuleViolation::ParentsNotCanonical);
        }
        if let Some(missing) = content.parents.iter().find(|p| !self.contains(p)) {
            return Err(RuleViolation::MissingParent(*missing));
        }
        for r in content.body.references() {
            if content.parents.binary_search(&r).is_err() {
                return Err(RuleViolation::UnlistedReference(r));
            }
        }
        match &content.body {
            Body::Claim { statement, .. } if statement.trim().is_empty() => {
                Err(RuleViolation::Empty("claim statement"))
            }
            Body::Artifact { name, .. } if name.trim().is_empty() => Err(RuleViolation::Empty("artifact name")),
            Body::Evidence { method, .. } if method.trim().is_empty() => Err(RuleViolation::Empty("evidence method")),
            Body::Relation { from, to, .. } if from == to => Err(RuleViolation::SelfRelation),
            Body::Retract { target, .. } if self.contributions[target].author() != content.author => {
                Err(RuleViolation::RetractByOther(*target))
            }
            _ => Ok(()),
        }
    }

    /// Adds `c` if it is valid. Returns `Ok(false)` if it was already present.
    pub fn insert(&mut self, c: Contribution) -> Result<bool, RuleViolation> {
        if self.contains(&c.id()) {
            return Ok(false);
        }
        self.check(&c)?;
        self.insert_checked(c);
        Ok(true)
    }

    fn insert_checked(&mut self, c: Contribution) {
        let id = c.id();
        for p in c.parents() {
            self.children.entry(*p).or_default().push(id);
            self.heads.remove(p);
        }
        for r in c.body().references() {
            self.referrers.entry(r).or_default().push(id);
        }
        self.heads.insert(id);
        self.contributions.insert(id, c);
    }

    /// Imports contributions in any order, adding each once its parents are
    /// present. `on_add` runs just before each insertion (used to persist);
    /// if it fails, the import stops and the error is returned.
    pub fn import_with<E>(
        &mut self,
        batch: impl IntoIterator<Item = Contribution>,
        mut on_add: impl FnMut(&Contribution) -> Result<(), E>,
    ) -> Result<ImportReport, E> {
        let mut report = ImportReport::default();
        let mut pending: HashMap<ContributionId, Contribution> = HashMap::new();
        for c in batch {
            if self.contains(&c.id()) {
                report.already_present += 1;
            } else {
                pending.insert(c.id(), c);
            }
        }

        // Kahn's algorithm over the pending set.
        let mut waiting_on: HashMap<ContributionId, Vec<ContributionId>> = HashMap::new();
        let mut missing_count: HashMap<ContributionId, usize> = HashMap::new();
        let mut ready: BTreeSet<ContributionId> = BTreeSet::new();
        for (id, c) in &pending {
            let missing: HashSet<_> = c.parents().iter().filter(|p| !self.contains(p)).collect();
            for p in &missing {
                waiting_on.entry(**p).or_default().push(*id);
            }
            if missing.is_empty() {
                ready.insert(*id);
            } else {
                missing_count.insert(*id, missing.len());
            }
        }

        while let Some(id) = ready.pop_first() {
            let c = pending.remove(&id).expect("ready ids are pending");
            if let Err(violation) = self.check(&c) {
                report.rejected.push((id, violation));
                continue;
            }
            on_add(&c)?;
            self.insert_checked(c);
            report.added.push(id);
            for child in waiting_on.remove(&id).unwrap_or_default() {
                let n = missing_count.get_mut(&child).expect("waiting child has a count");
                *n -= 1;
                if *n == 0 {
                    missing_count.remove(&child);
                    ready.insert(child);
                }
            }
        }

        report.unresolved = pending.into_keys().collect();
        report.unresolved.sort();
        Ok(report)
    }

    pub fn import(&mut self, batch: impl IntoIterator<Item = Contribution>) -> ImportReport {
        let Ok(report) = self.import_with(batch, |_| Ok::<(), std::convert::Infallible>(()));
        report
    }

    /// All contributions in a causal order (parents first). Ties are broken by
    /// id, so every replica holding the same set produces the same order.
    pub fn topo_order(&self) -> Vec<ContributionId> {
        let mut remaining: HashMap<ContributionId, usize> =
            self.contributions.iter().map(|(id, c)| (*id, c.parents().len())).collect();
        let mut ready: BTreeSet<ContributionId> =
            remaining.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
        let mut order = Vec::with_capacity(self.contributions.len());
        while let Some(id) = ready.pop_first() {
            order.push(id);
            for child in self.children(&id) {
                let n = remaining.get_mut(child).expect("child is in graph");
                *n -= 1;
                if *n == 0 {
                    ready.insert(*child);
                }
            }
        }
        order
    }

    /// Whether `a` is a strict causal ancestor of `b`.
    pub fn is_ancestor(&self, a: &ContributionId, b: &ContributionId) -> bool {
        let mut stack: Vec<ContributionId> = self.get(b).map(|c| c.parents().to_vec()).unwrap_or_default();
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == *a {
                return true;
            }
            if seen.insert(id) {
                stack.extend_from_slice(self.contributions[&id].parents());
            }
        }
        false
    }

    /// Whether `id` has been withdrawn by its author. Retracting a retraction
    /// restores the original.
    pub fn is_retracted(&self, id: &ContributionId) -> bool {
        self.referrers(id)
            .any(|r| matches!(r.body(), Body::Retract { target, .. } if target == id) && !self.is_retracted(&r.id()))
    }

    /// Contributions this graph has that `other` lacks, parents first.
    pub fn missing_from(&self, other: &Graph) -> Vec<Contribution> {
        self.topo_order()
            .into_iter()
            .filter(|id| !other.contains(id))
            .map(|id| self.contributions[&id].clone())
            .collect()
    }

    /// Finds the unique contribution whose hex id starts with `prefix`.
    pub fn resolve_prefix(&self, prefix: &str) -> Result<ContributionId, ResolveError> {
        let err = |kind| ResolveError { prefix: prefix.to_owned(), kind };
        let lower = prefix.to_ascii_lowercase();
        if lower.len() < MIN_PREFIX_LEN {
            return Err(err(ResolveErrorKind::TooShort));
        }
        if !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err(ResolveErrorKind::NotHex));
        }
        let matches: Vec<_> = self.contributions.keys().filter(|id| id.to_hex().starts_with(&lower)).collect();
        match matches.as_slice() {
            [one] => Ok(**one),
            [] => Err(err(ResolveErrorKind::NotFound)),
            many => Err(err(ResolveErrorKind::Ambiguous(many.len()))),
        }
    }
}
