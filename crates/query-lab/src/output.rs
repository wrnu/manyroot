//! Printing a query's rows: full ids, one compact line each, or JSON.

use serde_json::{Value, json};

use crate::ground::{Fact, Ground, Row};

/// One line each: a 12-digit id, author, kind and tags, and the text cut to one line of at most 120
/// characters. Control characters print as escapes, as in the CLI's log lines.
pub fn compact(g: &Ground, row: Row) -> String {
    let r = row as usize;
    let id = g.ids[r].short();
    let author = g.author_name(row);
    let text = match &g.fact[r] {
        Fact::Claim { statement, tags } => {
            let tags: Vec<&str> = tags.iter().map(|t| g.tags[*t as usize].as_str()).collect();
            format!("claim [{}] {}", tags.join(","), statement)
        }
        Fact::Artifact { name, media_type } => format!("artifact {name} ({media_type})"),
        Fact::Relation { from, kind, to } => {
            format!("relation {} {} {}", g.ids[*from as usize].short(), kind.as_str(), g.ids[*to as usize].short())
        }
        Fact::Evidence { target, verdict, method } => {
            format!("evidence {} {} via {}", verdict.as_str(), g.ids[*target as usize].short(), method)
        }
        Fact::Retract { target } => format!("retract {}", g.ids[*target as usize].short()),
    };
    let text: String = text.escape_debug().take(120).collect();
    format!("{id}  {author}  {text}")
}

pub fn json(g: &Ground, row: Row) -> Value {
    let r = row as usize;
    let a = g.author[r] as usize;
    let mut v = json!({
        "id": g.ids[r].to_hex(),
        "author": g.agents[a].to_hex(),
        "author_name": g.agent_names[a],
        "kind": g.fact[r].kind(),
        "timestamp_ms": g.timestamp_ms[r],
    });
    let id = |row: &Row| g.ids[*row as usize].to_hex();
    v["body"] = match &g.fact[r] {
        Fact::Claim { statement, tags } => {
            json!({"statement": statement, "tags": tags.iter().map(|t| &g.tags[*t as usize]).collect::<Vec<_>>()})
        }
        Fact::Artifact { name, media_type } => json!({"name": name, "media_type": media_type}),
        Fact::Relation { from, kind, to } => json!({"from": id(from), "kind": kind, "to": id(to)}),
        Fact::Evidence { target, verdict, method } => {
            json!({"target": id(target), "verdict": verdict, "method": method})
        }
        Fact::Retract { target } => json!({"target": id(target)}),
    };
    v
}
