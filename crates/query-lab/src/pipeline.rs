//! The pipeline surface: `tag pr | current | author manager | since <cursor>`. Each stage narrows a set of
//! rows, kept in causal order. It covers the CLI's `log` filters and `show`'s referrers (questions Q1 to Q4
//! of design note `f2cf04a0a0b7`); the Datalog core comes later and this will desugar into it.
//!
//! Planning is one rewrite: row filters commute, so a run of them up to the next stage that doesn't
//! (`refs`, `limit`) starts from the narrowest index it names (`refs` from the referrers index, `tag` from
//! the tag index) instead of every row, and evaluates the cheap filters before the ones that need a
//! whole-graph pass (`current`, `since` with a cursor).

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use manyroot::{BodyKind, RelationKind};

use crate::ground::{Fact, Ground, Row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Tag(String),
    Kind(BodyKind),
    Author(String),
    /// Not retracted.
    Live,
    /// What `log --current` shows.
    Current,
    /// What a cursor hasn't seen, or what was stamped at or after a date.
    Since(String),
    /// The rows whose facts reference an id: relations, evidence and retractions about it.
    Refs(String),
    /// The last N.
    Limit(usize),
}

impl Stage {
    /// Whether the stage only keeps or drops each row on its own, so it commutes with the others.
    fn is_filter(&self) -> bool {
        !matches!(self, Stage::Refs(_) | Stage::Limit(_))
    }

    /// Filters that read a whole-graph pass go last in their run.
    fn cost(&self) -> u8 {
        match self {
            Stage::Current | Stage::Since(_) | Stage::Live => 1,
            _ => 0,
        }
    }
}

pub fn parse(text: &str) -> Result<Vec<Stage>> {
    let mut stages = Vec::new();
    for part in text.split('|').map(str::trim).filter(|p| !p.is_empty()) {
        let (word, arg) = part.split_once(char::is_whitespace).map_or((part, ""), |(w, a)| (w, a.trim()));
        let need = |what: &str| -> Result<String> {
            if arg.is_empty() { bail!("`{word}` needs {what}") } else { Ok(arg.to_owned()) }
        };
        stages.push(match word {
            "tag" => Stage::Tag(need("a tag")?),
            "kind" => Stage::Kind(match need("a kind")?.as_str() {
                "claim" => BodyKind::Claim,
                "artifact" => BodyKind::Artifact,
                "relation" => BodyKind::Relation,
                "evidence" => BodyKind::Evidence,
                "retract" => BodyKind::Retract,
                k => bail!("no kind {k}"),
            }),
            "author" => Stage::Author(need("a name or key")?),
            "live" => Stage::Live,
            "current" => Stage::Current,
            // An empty cursor is allowed: it has seen nothing.
            "since" => Stage::Since(arg.trim_matches('"').to_owned()),
            "refs" => Stage::Refs(need("an id")?),
            "limit" => Stage::Limit(need("a number")?.parse().context("`limit` needs a number")?),
            w => bail!("unknown stage `{w}`"),
        });
    }
    Ok(stages)
}

/// Lazily computed whole-graph facts, shared by the stages of one run.
#[derive(Default)]
struct Derived {
    retracted: Option<Vec<bool>>,
    hidden: Option<Vec<bool>>,
}

impl Derived {
    fn retracted(&mut self, g: &Ground) -> &[bool] {
        self.retracted.get_or_insert_with(|| g.retracted())
    }

    fn hidden(&mut self, g: &Ground) -> &[bool] {
        if self.hidden.is_none() {
            let retracted = self.retracted(g).to_vec();
            self.hidden = Some(hidden(g, &retracted));
        }
        self.hidden.as_deref().expect("just computed")
    }
}

/// What `log --current` omits: retracted rows; subjects with a supersession that counts (a live
/// `supersedes` relation by the subject's own author, from a live subject); and rows that reference an
/// omitted row. Causal order decides references first.
pub fn hidden(g: &Ground, retracted: &[bool]) -> Vec<bool> {
    let open = |r: Row| g.fact[r as usize].is_subject() && !retracted[r as usize];
    let mut hidden = vec![false; g.len()];
    for row in 0..g.len() {
        let fact = &g.fact[row];
        let superseded = fact.is_subject()
            && g.referrers[row].iter().any(|r| {
                matches!(g.fact[*r as usize], Fact::Relation { from, kind: RelationKind::Supersedes, to }
                    if to as usize == row && open(from))
                    && g.author[*r as usize] == g.author[row]
                    && !retracted[*r as usize]
            });
        hidden[row] = retracted[row] || superseded || fact.references().iter().any(|r| hidden[*r as usize]);
    }
    hidden
}

pub fn run(g: &Ground, stages: &[Stage]) -> Result<Vec<Row>> {
    let mut derived = Derived::default();
    let mut rows: Option<Vec<Row>> = None; // None: every row, not yet listed
    let mut i = 0;
    while i < stages.len() {
        if !stages[i].is_filter() {
            rows = Some(apply_step(g, &stages[i], rows)?);
            i += 1;
            continue;
        }
        let end = stages[i..].iter().position(|s| !s.is_filter()).map_or(stages.len(), |n| i + n);
        let mut run: Vec<&Stage> = stages[i..end].iter().collect();
        run.sort_by_key(|s| s.cost());
        let mut set = match rows.take() {
            Some(set) => set,
            None => source(g, &mut run)?,
        };
        for stage in run {
            set = filter(g, &mut derived, stage, set)?;
        }
        rows = Some(set);
        i = end;
    }
    Ok(rows.unwrap_or_else(|| (0..g.len() as Row).collect()))
}

/// The starting set for a run of filters over every row: the tag index if a filter names a tag, which
/// that filter then needn't check again; else every row.
fn source(g: &Ground, run: &mut Vec<&Stage>) -> Result<Vec<Row>> {
    if let Some(n) = run.iter().position(|s| matches!(s, Stage::Tag(_))) {
        let Stage::Tag(tag) = run.remove(n) else { unreachable!("matched a tag") };
        return Ok(g.tag_of.get(tag).map_or_else(Vec::new, |t| g.by_tag[*t as usize].clone()));
    }
    Ok((0..g.len() as Row).collect())
}

fn apply_step(g: &Ground, stage: &Stage, rows: Option<Vec<Row>>) -> Result<Vec<Row>> {
    match stage {
        Stage::Refs(id) => {
            let target = g.resolve(id)?;
            let refs = &g.referrers[target as usize];
            Ok(match rows {
                None => refs.clone(),
                Some(set) => {
                    let keep: HashSet<Row> = set.into_iter().collect();
                    refs.iter().copied().filter(|r| keep.contains(r)).collect()
                }
            })
        }
        Stage::Limit(n) => {
            let set = rows.unwrap_or_else(|| (0..g.len() as Row).collect());
            Ok(set[set.len().saturating_sub(*n)..].to_vec())
        }
        _ => unreachable!("filters run in apply's runs"),
    }
}

fn filter(g: &Ground, derived: &mut Derived, stage: &Stage, mut set: Vec<Row>) -> Result<Vec<Row>> {
    match stage {
        Stage::Tag(tag) => match g.tag_of.get(tag) {
            None => set.clear(),
            Some(t) => set.retain(|r| matches!(&g.fact[*r as usize], Fact::Claim { tags, .. } if tags.contains(t))),
        },
        Stage::Kind(k) => set.retain(|r| g.fact[*r as usize].kind() == *k),
        Stage::Author(name) => {
            let a = g.agent(name).with_context(|| format!("no agent {name}"))?;
            set.retain(|r| g.author[*r as usize] == a);
        }
        Stage::Live => {
            let retracted = derived.retracted(g);
            set.retain(|r| !retracted[*r as usize]);
        }
        Stage::Current => {
            let hidden = derived.hidden(g);
            set.retain(|r| !hidden[*r as usize]);
        }
        Stage::Since(s) => match since_ms(s)? {
            Some(ms) => set.retain(|r| g.timestamp_ms[*r as usize] >= ms),
            None => {
                let seen = seen(g, s)?;
                set.retain(|r| !seen[*r as usize]);
            }
        },
        Stage::Refs(_) | Stage::Limit(_) => unreachable!("not a filter"),
    }
    Ok(set)
}

/// A date (`2026-10-29`, from its start in UTC) or an RFC 3339 time, as `log --since` reads one; `None`
/// for a cursor.
fn since_ms(s: &str) -> Result<Option<u64>> {
    let dated = s.len() >= 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[..4].iter().all(u8::is_ascii_digit);
    if !dated {
        return Ok(None);
    }
    let time = if s.len() == 10 { format!("{s}T00:00:00Z") } else { s.to_owned() };
    let at = humantime::parse_rfc3339(&time).with_context(|| format!("`since`: {s} isn't a date"))?;
    let ms = at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    Ok(Some(u64::try_from(ms).unwrap_or(u64::MAX)))
}

/// Everything a cursor (comma-separated ids or prefixes) has seen: its ids and their ancestors.
pub fn seen(g: &Ground, cursor: &str) -> Result<Vec<bool>> {
    let mut seen = vec![false; g.len()];
    let mut stack = Vec::new();
    for part in cursor.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        stack.push(g.resolve(part)?);
    }
    while let Some(row) = stack.pop() {
        if !std::mem::replace(&mut seen[row as usize], true) {
            stack.extend_from_slice(&g.parents[row as usize]);
        }
    }
    Ok(seen)
}
