//! Reading the log a piece at a time: cursors for `log --since`, and the compact form of `log` and `heads`,
//! with the shortest unique id prefixes.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use manyroot::graph::ResolveErrorKind;
use manyroot::{Body, ContributionId, Graph};

/// The fewest hex digits a compact id has, however few contributions there are.
pub const COMPACT_ID_DIGITS: usize = 8;

/// The longest a compact line's statement or name runs before it's cut.
const COMPACT_TEXT_CHARS: usize = 120;

/// The longest a whole compact line runs (long tags or names included) before it's cut.
pub const COMPACT_LINE_CHARS: usize = 200;

/// The cursor to pass to the next `log --since`: the causal heads, as sorted, comma-separated full ids.
pub fn cursor(g: &Graph) -> String {
    let mut heads: Vec<String> = g.heads().map(|id| id.to_hex()).collect();
    heads.sort();
    heads.join(",")
}

/// Everything a cursor has seen: its ids and all their ancestors. Each id may be a unique prefix, and an
/// empty cursor has seen nothing.
pub fn seen(g: &Graph, cursor: &str) -> Result<HashSet<ContributionId>> {
    let mut stack = Vec::new();
    for part in cursor.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match g.resolve_prefix(part) {
            Ok(id) => stack.push(id),
            Err(e) if matches!(e.kind, ResolveErrorKind::NotFound) => {
                bail!("--since: unknown id {part} in the cursor: no contribution in this replica has it")
            }
            Err(e) => bail!("--since: id {part} in the cursor: {e}"),
        }
    }
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if seen.insert(id) {
            stack.extend(g.get(&id).expect("graph ids and their parents exist").parents().iter().copied());
        }
    }
    Ok(seen)
}

/// The shortest prefix of each id, of at least `min` hex digits, that no other id in `ids` starts with.
pub fn unique_prefixes(ids: impl IntoIterator<Item = ContributionId>, min: usize) -> HashMap<ContributionId, String> {
    let mut hex: Vec<(String, ContributionId)> = ids.into_iter().map(|id| (id.to_hex(), id)).collect();
    hex.sort();
    hex.dedup();
    let common = |a: &str, b: &str| a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    (0..hex.len())
        .map(|i| {
            let before = i.checked_sub(1).map_or(0, |j| common(&hex[i].0, &hex[j].0));
            let after = hex.get(i + 1).map_or(0, |next| common(&hex[i].0, &next.0));
            let len = (before.max(after) + 1).max(min).min(hex[i].0.len());
            (hex[i].1, hex[i].0[..len].to_owned())
        })
        .collect()
}

/// The text a compact line ends with: a claim's statement, an artifact's name, or the summary of anything
/// else, cut to its first line and to `COMPACT_TEXT_CHARS`.
pub fn compact_text(body: &Body, summary: String) -> String {
    let text = match body {
        Body::Claim { statement, .. } => statement.as_str(),
        Body::Artifact { name, .. } => name.as_str(),
        _ => summary.as_str(),
    };
    let first = text.lines().next().unwrap_or_default();
    let cut = first.char_indices().nth(COMPACT_TEXT_CHARS).map(|(i, _)| i);
    match cut {
        Some(i) => format!("{}…", &first[..i]),
        None if first.len() < text.trim_end().len() => format!("{first} …"),
        None => first.to_owned(),
    }
}

/// `line`, cut to `COMPACT_LINE_CHARS` characters (chars, not display columns), ending in "…" when cut.
pub fn cap_line(line: String) -> String {
    match line.char_indices().nth(COMPACT_LINE_CHARS - 1) {
        Some((i, _)) if line[i..].chars().nth(1).is_some() => format!("{}…", &line[..i]),
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(hex: &str) -> ContributionId {
        format!("{hex:0<64}").parse().expect("a test id")
    }

    #[test]
    fn prefixes_grow_past_the_minimum_only_where_ids_share_one() {
        let (a, b, c) = (id("abcdef0123"), id("abcdef0124"), id("12"));
        let p = unique_prefixes([a, b, c], 8);
        assert_eq!((p[&a].as_str(), p[&b].as_str(), p[&c].as_str()), ("abcdef0123", "abcdef0124", "12000000"));
        // Alone, the same id needs only the minimum.
        assert_eq!(unique_prefixes([a], 8)[&a], "abcdef01");
    }

    #[test]
    fn compact_text_keeps_one_line() {
        let claim = |s: &str| Body::Claim { statement: s.to_owned(), tags: vec![] };
        assert_eq!(compact_text(&claim("one line"), String::new()), "one line");
        assert_eq!(compact_text(&claim("first\nsecond"), String::new()), "first …");
        let long = "é".repeat(200);
        assert_eq!(compact_text(&claim(&long), String::new()), format!("{}…", "é".repeat(120)));
    }
}
