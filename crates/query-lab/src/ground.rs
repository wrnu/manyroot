//! The tables queries read: every contribution of a Ground as a dense row, numbered in causal order, with
//! the indexes the queries need. Built once from a loaded [`Graph`], read-only afterwards.

use std::collections::HashMap;

use manyroot::{AgentId, Body, BodyKind, ContributionId, Graph, RelationKind, Verdict};

/// A contribution's place in causal order, which is also its row. A row's parents and references all
/// have smaller rows.
pub type Row = u32;

/// What a row says, with references as rows. Text is kept for output and for method matching.
#[derive(Debug, Clone)]
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

impl Ground {
    /// `names` maps public keys to local names, as `Store::agents` lists them.
    pub fn from_graph(graph: &Graph, names: &[(String, AgentId)]) -> Ground {
        let order = graph.topo_order();
        let row_of: HashMap<ContributionId, Row> =
            order.iter().enumerate().map(|(i, id)| (*id, u32::try_from(i).expect("fewer than 2^32 rows"))).collect();
        let mut g = Ground {
            ids: order.clone(),
            row_of,
            author: Vec::with_capacity(order.len()),
            timestamp_ms: Vec::with_capacity(order.len()),
            fact: Vec::with_capacity(order.len()),
            parents: Vec::with_capacity(order.len()),
            referrers: vec![Vec::new(); order.len()],
            heads: Vec::new(),
            agents: Vec::new(),
            agent_names: Vec::new(),
            tags: Vec::new(),
            tag_of: HashMap::new(),
            by_tag: Vec::new(),
        };
        let mut agent_of: HashMap<AgentId, u32> = HashMap::new();
        for (row, id) in order.iter().enumerate() {
            let c = graph.get(id).expect("ordered ids are in the graph");
            let author = *agent_of.entry(c.author()).or_insert_with(|| {
                g.agents.push(c.author());
                g.agent_names.push(names.iter().find(|(_, a)| *a == c.author()).map(|(n, _)| n.clone()));
                u32::try_from(g.agents.len() - 1).expect("fewer than 2^32 agents")
            });
            g.author.push(author);
            g.timestamp_ms.push(c.content().timestamp_ms);
            g.parents.push(c.parents().iter().map(|p| g.row_of[p]).collect());
            let fact = match c.body() {
                Body::Claim { statement, tags } => {
                    let tags = tags.iter().map(|t| g.intern_tag(t)).collect::<Vec<_>>();
                    for t in &tags {
                        g.by_tag[*t as usize].push(row as Row);
                    }
                    Fact::Claim { statement: statement.clone(), tags }
                }
                Body::Artifact { name, media_type, .. } => {
                    Fact::Artifact { name: name.clone(), media_type: media_type.clone() }
                }
                Body::Relation { from, kind, to, .. } => {
                    Fact::Relation { from: g.row_of[from], kind: *kind, to: g.row_of[to] }
                }
                Body::Evidence { target, verdict, method, .. } => {
                    Fact::Evidence { target: g.row_of[target], verdict: *verdict, method: method.clone() }
                }
                Body::Retract { target, .. } => Fact::Retract { target: g.row_of[target] },
            };
            let mut refs = fact.references();
            refs.dedup();
            for r in refs {
                g.referrers[r as usize].push(row as Row);
            }
            g.fact.push(fact);
        }
        g.heads = graph.heads().map(|h| g.row_of[&h]).collect();
        g.heads.sort_unstable();
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
