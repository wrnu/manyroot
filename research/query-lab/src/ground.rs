//! The tables queries read: every contribution of a Ground as a dense row, numbered in causal order, with
//! the indexes the queries need. Built once, from a loaded [`Graph`] or a snapshot, read-only afterwards.

use std::collections::{BTreeSet, HashMap};

use manyroot::{AgentId, Body, BodyKind, Contribution, ContributionId, Graph, RelationKind, Verdict};

/// A contribution's place in causal order, which is also its row. A row's parents and references all
/// have smaller rows.
pub type Row = u32;

/// What a row says, with references as rows. Text is kept for output and for method matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Claim { statement: String, tags: Vec<u32> },
    Artifact { name: String, media_type: String },
    Relation { from: Row, kind: RelationKind, to: Row },
    Evidence { target: Row, verdict: Verdict, method: String },
    Retract { target: Row },
}

impl Fact {
    pub fn kind(&self) -> BodyKind {
        match self {
            Fact::Claim { .. } => BodyKind::Claim,
            Fact::Artifact { .. } => BodyKind::Artifact,
            Fact::Relation { .. } => BodyKind::Relation,
            Fact::Evidence { .. } => BodyKind::Evidence,
            Fact::Retract { .. } => BodyKind::Retract,
        }
    }

    pub fn is_subject(&self) -> bool {
        matches!(self, Fact::Claim { .. } | Fact::Artifact { .. })
    }

    /// The rows this one talks about.
    pub fn references(&self) -> Vec<Row> {
        match self {
            Fact::Claim { .. } | Fact::Artifact { .. } => vec![],
            Fact::Relation { from, to, .. } => vec![*from, *to],
            Fact::Evidence { target, .. } | Fact::Retract { target } => vec![*target],
        }
    }
}

pub struct Ground {
    pub ids: Vec<ContributionId>,
    pub row_of: HashMap<ContributionId, Row>,
    pub author: Vec<u32>,
    pub timestamp_ms: Vec<u64>,
    pub fact: Vec<Fact>,
    pub parents: Vec<Vec<Row>>,
    /// Rows whose fact references the row, in causal order.
    pub referrers: Vec<Vec<Row>>,
    pub heads: Vec<Row>,
    pub agents: Vec<AgentId>,
    /// Local names from the replica's public name map, where it has one.
    pub agent_names: Vec<Option<String>>,
    pub tags: Vec<String>,
    pub tag_of: HashMap<String, u32>,
    /// Claims carrying each tag, in causal order.
    pub by_tag: Vec<Vec<Row>>,
}

/// One contribution as the tables need it: no signature, details or artifact content. A snapshot stores
/// these, so loading one needs no decoding, hashing or verifying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: ContributionId,
    pub author: AgentId,
    pub timestamp_ms: u64,
    pub parents: Vec<ContributionId>,
    pub body: EntryBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryBody {
    Claim { statement: String, tags: Vec<String> },
    Artifact { name: String, media_type: String },
    Relation { from: ContributionId, kind: RelationKind, to: ContributionId },
    Evidence { target: ContributionId, verdict: Verdict, method: String },
    Retract { target: ContributionId },
}

impl Entry {
    pub fn of(c: &Contribution) -> Entry {
        let body = match c.body() {
            Body::Claim { statement, tags } => EntryBody::Claim { statement: statement.clone(), tags: tags.clone() },
            Body::Artifact { name, media_type, .. } => {
                EntryBody::Artifact { name: name.clone(), media_type: media_type.clone() }
            }
            Body::Relation { from, kind, to, .. } => EntryBody::Relation { from: *from, kind: *kind, to: *to },
            Body::Evidence { target, verdict, method, .. } => {
                EntryBody::Evidence { target: *target, verdict: *verdict, method: method.clone() }
            }
            Body::Retract { target, .. } => EntryBody::Retract { target: *target },
        };
        Entry {
            id: c.id(),
            author: c.author(),
            timestamp_ms: c.content().timestamp_ms,
            parents: c.parents().to_vec(),
            body,
        }
    }

    /// The ids the body talks about.
    pub fn references(&self) -> Vec<ContributionId> {
        match &self.body {
            EntryBody::Claim { .. } | EntryBody::Artifact { .. } => vec![],
            EntryBody::Relation { from, to, .. } => vec![*from, *to],
            EntryBody::Evidence { target, .. } | EntryBody::Retract { target } => vec![*target],
        }
    }
}

/// Entries in the protocol's causal order (protocol.md, Derived state): a topological order breaking ties
/// by ascending id, the order `Graph::topo_order` gives, so rows are the same however the entries arrived.
/// Every parent must be among the entries.
pub fn causal_order(mut entries: Vec<Entry>) -> Vec<Entry> {
    let index: HashMap<ContributionId, usize> = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
    let mut waiting: Vec<usize> = entries.iter().map(|e| e.parents.len()).collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); entries.len()];
    for (i, e) in entries.iter().enumerate() {
        for p in &e.parents {
            children[index[p]].push(i);
        }
    }
    let mut ready: BTreeSet<(ContributionId, usize)> =
        entries.iter().enumerate().filter(|(i, _)| waiting[*i] == 0).map(|(i, e)| (e.id, i)).collect();
    let mut order = Vec::with_capacity(entries.len());
    while let Some((_, i)) = ready.pop_first() {
        order.push(i);
        for c in &children[i] {
            waiting[*c] -= 1;
            if waiting[*c] == 0 {
                ready.insert((entries[*c].id, *c));
            }
        }
    }
    assert_eq!(order.len(), entries.len(), "every parent is among the entries, so nothing waits forever");
    let mut slots: Vec<Option<Entry>> = entries.drain(..).map(Some).collect();
    order.into_iter().map(|i| slots[i].take().expect("each entry once")).collect()
}

impl Ground {
    /// `names` maps public keys to local names, as `Store::agents` lists them.
    pub fn from_graph(graph: &Graph, names: &[(String, AgentId)]) -> Ground {
        let entries =
            graph.topo_order().iter().map(|id| Entry::of(graph.get(id).expect("ordered ids exist"))).collect();
        Ground::from_ordered(entries, names)
    }

    /// From entries already in causal order (see [`causal_order`]).
    pub fn from_ordered(entries: Vec<Entry>, names: &[(String, AgentId)]) -> Ground {
        let row_of: HashMap<ContributionId, Row> =
            entries.iter().enumerate().map(|(i, e)| (e.id, u32::try_from(i).expect("fewer than 2^32 rows"))).collect();
        let n = entries.len();
        let mut g = Ground {
            ids: entries.iter().map(|e| e.id).collect(),
            row_of,
            author: Vec::with_capacity(n),
            timestamp_ms: Vec::with_capacity(n),
            fact: Vec::with_capacity(n),
            parents: Vec::with_capacity(n),
            referrers: vec![Vec::new(); n],
            heads: Vec::new(),
            agents: Vec::new(),
            agent_names: Vec::new(),
            tags: Vec::new(),
            tag_of: HashMap::new(),
            by_tag: Vec::new(),
        };
        let mut agent_of: HashMap<AgentId, u32> = HashMap::new();
        let mut has_child = vec![false; n];
        for (row, e) in entries.into_iter().enumerate() {
            let author = *agent_of.entry(e.author).or_insert_with(|| {
                g.agents.push(e.author);
                g.agent_names.push(names.iter().find(|(_, a)| *a == e.author).map(|(n, _)| n.clone()));
                u32::try_from(g.agents.len() - 1).expect("fewer than 2^32 agents")
            });
            g.author.push(author);
            g.timestamp_ms.push(e.timestamp_ms);
            let parents: Vec<Row> = e.parents.iter().map(|p| g.row_of[p]).collect();
            for p in &parents {
                has_child[*p as usize] = true;
            }
            g.parents.push(parents);
            let fact = match e.body {
                EntryBody::Claim { statement, tags } => {
                    let tags = tags.iter().map(|t| g.intern_tag(t)).collect::<Vec<_>>();
                    for t in &tags {
                        g.by_tag[*t as usize].push(row as Row);
                    }
                    Fact::Claim { statement, tags }
                }
                EntryBody::Artifact { name, media_type } => Fact::Artifact { name, media_type },
                EntryBody::Relation { from, kind, to } => {
                    Fact::Relation { from: g.row_of[&from], kind, to: g.row_of[&to] }
                }
                EntryBody::Evidence { target, verdict, method } => {
                    Fact::Evidence { target: g.row_of[&target], verdict, method }
                }
                EntryBody::Retract { target } => Fact::Retract { target: g.row_of[&target] },
            };
            let mut refs = fact.references();
            refs.dedup();
            for r in refs {
                g.referrers[r as usize].push(row as Row);
            }
            g.fact.push(fact);
        }
        g.heads = (0..n as Row).filter(|r| !has_child[*r as usize]).collect();
        g
    }

    fn intern_tag(&mut self, tag: &str) -> u32 {
        if let Some(t) = self.tag_of.get(tag) {
            return *t;
        }
        let t = u32::try_from(self.tags.len()).expect("fewer than 2^32 tags");
        self.tags.push(tag.to_owned());
        self.tag_of.insert(tag.to_owned(), t);
        self.by_tag.push(Vec::new());
        t
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// A full id, or a unique hex prefix of one.
    pub fn resolve(&self, prefix: &str) -> anyhow::Result<Row> {
        let prefix = prefix.to_ascii_lowercase();
        if let Ok(id) = prefix.parse::<ContributionId>() {
            return self.row_of.get(&id).copied().ok_or_else(|| anyhow::anyhow!("no contribution {prefix}"));
        }
        let mut found = self.ids.iter().enumerate().filter(|(_, id)| id.to_hex().starts_with(&prefix));
        match (found.next(), found.next()) {
            (Some((row, _)), None) => Ok(row as Row),
            (None, _) => anyhow::bail!("no contribution starts with {prefix}"),
            (Some(_), Some(_)) => anyhow::bail!("{prefix} is ambiguous"),
        }
    }

    /// An agent by local name or public key.
    pub fn agent(&self, name: &str) -> Option<u32> {
        let by_name = self.agent_names.iter().position(|n| n.as_deref() == Some(name));
        let by_key = || self.agents.iter().position(|a| a.to_hex() == name);
        by_name.or_else(by_key).map(|a| a as u32)
    }

    pub fn author_name(&self, row: Row) -> String {
        let a = self.author[row as usize] as usize;
        self.agent_names[a].clone().unwrap_or_else(|| self.agents[a].to_hex())
    }

    /// Every row retracted under the protocol: the target of a retract that isn't itself retracted. A retract
    /// comes after its target, so walking causal order backwards decides each retract first.
    pub fn retracted(&self) -> Vec<bool> {
        let mut retracted = vec![false; self.len()];
        for row in (0..self.len()).rev() {
            retracted[row] = self.referrers[row].iter().any(|r| {
                matches!(self.fact[*r as usize], Fact::Retract { target } if target as usize == row)
                    && !retracted[*r as usize]
            });
        }
        retracted
    }
}
