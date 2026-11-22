//! Typed conventions on protocol v1 claims (task `8c9ad9bd4518`; research synthesis 2.2 C1 and 3.1, owner
//! decision `ea6f5419a31e`). Three tag forms, each mapping one-to-one onto a field protocol v2 is to hold:
//!
//! | Tag | v2 field | Rule here |
//! |---|---|---|
//! | `type:<name>` | `type` | at most one; `name` is lowercase words joined by `.` |
//! | `<key>:<value>` | one entry of `data` | each key at most once; `type` and `ref` are not keys |
//! | `ref:<role>=<id>` | one `[role, id]` of `refs` | the full id; the CLI also makes it a parent |
//!
//! Any other tag is a plain tag (`finding`, `note`), as before. A type may have a *shape*: keys it needs,
//! the kind of each key's value, and relations it must be written with. `bracken claim` checks the syntax of
//! every tag and the shape of a typed claim before writing anything, and refuses a claim that breaks them.
//! That is a client check only: validity never looks at tags (decision `ea6f5419a31e`, point 1), so a
//! contribution written some other way, by an older client or another Ground, is as valid as ever, and a
//! read only reports what doesn't conform ([`read`], [`problems`]), never refuses it.
//!
//! Claims written before these conventions keep working: a plain `finding` tag means what it meant, and a
//! typed claim also carries its type's plain tag ([`Shape::legacy`]), so every reader of plain tags finds it.

use std::collections::BTreeMap;

use manyroot::RelationKind;
use serde_json::{Value, json};

/// A tag's longest name or key, and its longest value.
const MAX_NAME: usize = 64;
const MAX_VALUE: usize = 256;

/// One tag, by its form.
#[derive(Debug, PartialEq, Eq)]
pub enum Tag<'a> {
    Plain(&'a str),
    Type(&'a str),
    Attr(&'a str, &'a str),
    Ref(&'a str, &'a str),
}

/// Parses one tag, or says why it isn't one.
pub fn parse(tag: &str) -> Result<Tag<'_>, String> {
    if tag.is_empty() {
        return Err("a tag can't be empty".into());
    }
    if tag.contains(',') {
        return Err(format!("{tag:?} holds a comma: give each tag its own --tag"));
    }
    if tag.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{tag:?} holds a space or a control character"));
    }
    let Some((key, value)) = tag.split_once(':') else {
        return plain(tag).then_some(Tag::Plain(tag)).ok_or_else(|| {
            format!(
                "{tag:?} isn't a tag: a plain tag is lowercase letters, digits, '-', '_' and '.', starting with a \
                 letter or digit, at most {MAX_NAME} characters"
            )
        });
    };
    match key {
        "type" => type_name(value).then_some(Tag::Type(value)).ok_or_else(|| {
            format!("{tag:?}: a type is lowercase words of letters, digits, '-' and '_', joined by '.'")
        }),
        "ref" => {
            let (role, id) = value.split_once('=').ok_or_else(|| format!("{tag:?}: a reference is ref:<role>=<id>"))?;
            if !name(role) {
                return Err(format!("{tag:?}: a role is a lowercase name"));
            }
            if !(id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
                return Err(format!("{tag:?}: a reference holds the full 64-digit id (--ref resolves a prefix)"));
            }
            Ok(Tag::Ref(role, id))
        }
        _ if !name(key) => Err(format!(
            "{tag:?}: a key is a lowercase letter, then lowercase letters, digits, '-', '_' and '.', at most \
             {MAX_NAME} characters"
        )),
        _ if value.is_empty() || value.len() > MAX_VALUE => Err(format!("{tag:?}: a value is 1 to {MAX_VALUE} bytes")),
        _ => Ok(Tag::Attr(key, value)),
    }
}

/// A key, a role, or one word of a type: `[a-z][a-z0-9_.-]*`, at most [`MAX_NAME`] bytes.
fn name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(c))
}

/// A plain tag: as a name, but it may start with a digit.
fn plain(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(c))
}

/// A type: words of `[a-z][a-z0-9_-]*` joined by `.`, so `finding` or `bracken.finding`.
fn type_name(s: &str) -> bool {
    s.len() <= MAX_NAME && s.split('.').all(|w| name(w) && !w.contains('.'))
}

/// A claim's tags, read by form.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub ty: Option<String>,
    pub data: BTreeMap<String, String>,
    pub refs: Vec<(String, String)>,
    pub plain: Vec<String>,
}

impl Typed {
    /// Whether any tag is typed, so the claim is more than plain tags.
    pub fn any(&self) -> bool {
        self.ty.is_some() || !self.data.is_empty() || !self.refs.is_empty()
    }
}

/// Reads tags by form, never refusing: what breaks a rule is left out and listed.
pub fn read(tags: &[String]) -> (Typed, Vec<String>) {
    let mut t = Typed::default();
    let mut problems = Vec::new();
    for tag in tags {
        match parse(tag) {
            Err(e) => problems.push(e),
            Ok(Tag::Plain(p)) if t.plain.iter().any(|q| q == p) => problems.push(format!("{p:?} is given twice")),
            Ok(Tag::Plain(p)) => t.plain.push(p.to_owned()),
            Ok(Tag::Type(ty)) => match &t.ty {
                Some(first) if first == ty => problems.push(format!("type:{ty} is given twice")),
                Some(first) => problems.push(format!("two types, {first} and {ty}: a claim has one")),
                None => t.ty = Some(ty.to_owned()),
            },
            Ok(Tag::Attr(k, v)) => match t.data.get(k) {
                Some(first) => problems.push(format!("the key {k} is given twice ({first}, {v}): a key has one value")),
                None => {
                    t.data.insert(k.to_owned(), v.to_owned());
                }
            },
            Ok(Tag::Ref(role, id)) if t.refs.iter().any(|(r, i)| r == role && i == id) => {
                problems.push(format!("ref:{role}={id} is given twice"))
            }
            Ok(Tag::Ref(role, id)) => t.refs.push((role.to_owned(), id.to_owned())),
        }
    }
    (t, problems)
}

/// A value's kind.
#[derive(Clone, Copy)]
enum Kind {
    /// A positive whole number, no leading zero.
    Count,
    /// A decimal number: `-12`, `3.5`.
    Number,
    /// A calendar date, `YYYY-MM-DD`.
    Date,
    /// A git commit: 40 lowercase hex digits.
    Sha,
    /// A contribution id: 64 lowercase hex digits.
    Id,
    /// Letters, digits and `._/-`: a branch, a name, a metric key.
    Word,
    /// Anything a tag's value may hold.
    Text,
    OneOf(&'static [&'static str]),
}

impl Kind {
    fn holds(self, v: &str) -> bool {
        let hex = |n: usize| v.len() == n && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        match self {
            Kind::Count => {
                !v.starts_with('0') && !v.is_empty() && v.len() <= 9 && v.bytes().all(|b| b.is_ascii_digit())
            }
            Kind::Number => {
                let v = v.strip_prefix('-').unwrap_or(v);
                let (whole, frac) = v.split_once('.').unwrap_or((v, "0"));
                let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
                digits(whole) && digits(frac)
            }
            Kind::Date => date(v),
            Kind::Sha => hex(40),
            Kind::Id => hex(64),
            Kind::Word => !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)),
            Kind::Text => true,
            Kind::OneOf(values) => values.contains(&v),
        }
    }

    fn describe(self) -> String {
        match self {
            Kind::Count => "a positive whole number".into(),
            Kind::Number => "a number, such as 15 or 2.5".into(),
            Kind::Date => "a date, YYYY-MM-DD".into(),
            Kind::Sha => "a full commit sha, 40 lowercase hex digits".into(),
            Kind::Id => "a full contribution id, 64 lowercase hex digits".into(),
            Kind::Word => "letters, digits and ._/-".into(),
            Kind::Text => "text".into(),
            Kind::OneOf(values) => format!("one of {}", values.join(", ")),
        }
    }
}

/// `YYYY-MM-DD`, a day that exists.
fn date(v: &str) -> bool {
    let b = v.as_bytes();
    if !v.is_ascii() || b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| {
        let s = &v[r];
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse::<u32>().ok()).flatten()
    };
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else { return false };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// One key a shape knows.
struct Key {
    name: &'static str,
    kind: Kind,
    required: bool,
}

const fn need(name: &'static str, kind: Kind) -> Key {
    Key { name, kind, required: true }
}

const fn may(name: &'static str, kind: Kind) -> Key {
    Key { name, kind, required: false }
}

/// What a type's claims hold. Keys a shape doesn't name are allowed, in any kind.
pub struct Shape {
    pub name: &'static str,
    /// The plain tag a claim of this type also carries, so readers of plain tags find it.
    pub legacy: Option<&'static str>,
    keys: &'static [Key],
    /// Roles of references it needs.
    refs: &'static [&'static str],
    /// Relations it must be written with, in the same call.
    relations: &'static [RelationKind],
    /// What else it needs, from the statement and the tags together.
    extra: fn(&Typed, &str) -> Vec<String>,
}

fn nothing(_: &Typed, _: &str) -> Vec<String> {
    Vec::new()
}

/// A pr claim's statement is `PR #<pr> <branch> @ <sha>: <title>`, which scripts/merge parses, and names
/// the Weave proposal it gives, so the text and the tags can't disagree.
fn pr_statement(t: &Typed, statement: &str) -> Vec<String> {
    let (Some(pr), Some(branch), Some(sha)) = (t.data.get("pr"), t.data.get("branch"), t.data.get("sha")) else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    let start = format!("PR #{pr} {branch} @ {sha}: ");
    if !statement.starts_with(&start) {
        problems.push(format!("a pr claim's statement starts {start:?}, as its tags say"));
    }
    if let Some(w) = t.data.get("weave")
        && !statement.contains(&format!("Weave proposal {w}"))
    {
        problems.push(format!("the statement names the Weave proposal its weave key gives: \"Weave proposal {w}\""));
    }
    problems
}

/// An experiment states its rule in a `rule:` line. Any `metric:`, `baseline:`, `target:` or `expires:`
/// line agrees with the key of that name, since scripts/merge and scripts/metrics read the lines.
fn experiment_statement(t: &Typed, statement: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let line = |key: &str| statement.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':').map(str::trim));
    if line("rule").is_none_or(str::is_empty) {
        problems.push("an experiment states its rule in a line 'rule: <the rule>'".into());
    }
    for key in ["metric", "baseline", "target", "expires"] {
        if let (Some(l), Some(v)) = (line(key), t.data.get(key)) {
            // expires is matched whole, as scripts/merge reads it; the others by their first word.
            let given = if key == "expires" { l } else { l.split_whitespace().next().unwrap_or("") };
            if given != v {
                problems.push(format!("the statement's '{key}:' line says {given:?}, its {key} key {v:?}"));
            }
        }
    }
    problems
}

/// A triage deferring a finding names the task it's deferred to.
fn triage_refs(t: &Typed, _: &str) -> Vec<String> {
    if t.data.get("disposition").is_some_and(|d| d == "deferred") && !t.refs.iter().any(|(r, _)| r == "task") {
        return vec!["a deferred triage names its task: --ref task=<id>".into()];
    }
    Vec::new()
}

const SEVERITY: Kind = Kind::OneOf(&["low", "medium", "high", "critical"]);

/// Every type with a shape. A type not listed here is refused at write, to catch a misspelt one; add it
/// here (with no keys, if it needs none) to use it.
pub const SHAPES: &[Shape] = &[
    Shape {
        name: "pr",
        legacy: Some("pr"),
        keys: &[
            need("pr", Kind::Count),
            need("branch", Kind::Word),
            need("sha", Kind::Sha),
            may("weave", Kind::Id),
            may("class", Kind::Word),
        ],
        refs: &[],
        relations: &[],
        extra: pr_statement,
    },
    Shape {
        name: "finding",
        legacy: Some("finding"),
        keys: &[
            need("severity", SEVERITY),
            need("label", Kind::OneOf(&["confirmed", "possible"])),
            may("at", Kind::Text),
        ],
        refs: &[],
        relations: &[RelationKind::Contradicts],
        extra: nothing,
    },
    Shape {
        name: "triage",
        legacy: None,
        keys: &[need("disposition", Kind::OneOf(&["confirmed", "rejected", "deferred"]))],
        refs: &["finding"],
        relations: &[],
        extra: triage_refs,
    },
    Shape {
        name: "task",
        legacy: Some("task"),
        keys: &[may("agent", Kind::Word), may("class", Kind::Word), may("branch", Kind::Word)],
        refs: &[],
        relations: &[],
        extra: nothing,
    },
    Shape {
        name: "decision",
        legacy: Some("decision"),
        keys: &[need("source", Kind::OneOf(&["owner", "manager"])), need("date", Kind::Date)],
        refs: &[],
        relations: &[],
        extra: nothing,
    },
    Shape {
        name: "experiment",
        legacy: Some("experiment"),
        keys: &[
            need("metric", Kind::Word),
            need("baseline", Kind::Number),
            need("target", Kind::Number),
            need("expires", Kind::Date),
        ],
        refs: &[],
        relations: &[],
        extra: experiment_statement,
    },
    Shape {
        name: "metric",
        legacy: Some("metrics"),
        keys: &[need("week", Kind::Date)],
        refs: &[],
        relations: &[],
        extra: nothing,
    },
    plain_type("note"),
    plain_type("question"),
    plain_type("answer"),
    plain_type("report"),
    plain_type("gotcha"),
    plain_type("preference"),
    plain_type("reference"),
    plain_type("process"),
    plain_type("misfire"),
    plain_type("plan"),
];

/// A type with no keys of its own, which carries its plain tag.
const fn plain_type(name: &'static str) -> Shape {
    Shape { name, legacy: Some(name), keys: &[], refs: &[], relations: &[], extra: nothing }
}

pub fn shape(ty: &str) -> Option<&'static Shape> {
    SHAPES.iter().find(|s| s.name == ty)
}

/// What a claim's statement and tags break, read as a reader reads them: relations aren't looked at, since
/// they're separate contributions. Empty if it conforms.
pub fn problems(statement: &str, tags: &[String]) -> Vec<String> {
    let (t, mut problems) = read(tags);
    if let Some(ty) = &t.ty {
        match shape(ty) {
            None => problems.push(format!("no shape for type {ty}")),
            Some(s) => problems.extend(shape_problems(s, &t, statement)),
        }
    }
    problems
}

fn shape_problems(s: &Shape, t: &Typed, statement: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for k in s.keys {
        match t.data.get(k.name) {
            None if k.required => problems.push(format!(
                "a {} needs the key {} ({}): --attr {}=…",
                s.name,
                k.name,
                k.kind.describe(),
                k.name
            )),
            Some(v) if !k.kind.holds(v) => {
                problems.push(format!("{}:{v}: {} is {}", k.name, k.name, k.kind.describe()))
            }
            _ => {}
        }
    }
    for role in s.refs {
        if !t.refs.iter().any(|(r, _)| r == role) {
            problems.push(format!("a {} needs a {role} reference: --ref {role}=<id>", s.name));
        }
    }
    problems.extend((s.extra)(t, statement));
    problems
}

/// The write-time check: every tag's syntax, one-to-one, and a typed claim's shape, including the relations
/// written with it in the same call. `relations` are the kinds this call writes from the claim.
pub fn check_write(statement: &str, tags: &[String], relations: &[RelationKind]) -> Result<Typed, String> {
    let (t, mut problems) = read(tags);
    if let Some(ty) = &t.ty {
        match shape(ty) {
            None => {
                let known: Vec<&str> = SHAPES.iter().map(|s| s.name).collect();
                problems.push(format!("there is no type {ty}; the types are {}", known.join(", ")));
            }
            Some(s) => {
                problems.extend(shape_problems(s, &t, statement));
                for r in s.relations {
                    if !relations.contains(r) {
                        let flag = r.as_str().replace('_', "-");
                        problems.push(format!("a {} is written with its {r} relation: --{flag} <id>", s.name));
                    }
                }
            }
        }
    }
    // A deferral is a task tagged deferred that depends on the finding it defers (scripts/merge, findings).
    if t.plain.iter().any(|p| p == "deferred")
        && t.ty.as_deref() == Some("task")
        && !relations.contains(&RelationKind::DependsOn)
    {
        problems.push("a deferred task is written with the finding it defers: --depends-on <finding>".into());
    }
    if problems.is_empty() { Ok(t) } else { Err(problems.join("; ")) }
}

/// The names a signing scope judges a tag by: the tag itself, and for `type:<name>` the type's name and its
/// plain tag, so a scope that forbids `decision` forbids `type:decision` too.
pub fn scope_names(tag: &str) -> Vec<&str> {
    let mut names = vec![tag];
    if let Ok(Tag::Type(ty)) = parse(tag) {
        names.push(ty);
        if let Some(legacy) = shape(ty).and_then(|s| s.legacy) {
            names.push(legacy);
        }
    }
    names
}

/// The typed reading of a claim for `--json`, or `None` when its tags are all plain: `{"type", "data",
/// "refs": [{"role", "id"}], "problems"}`, where problems also names a reference that isn't a parent.
pub fn json(statement: &str, tags: &[String], parents: &[manyroot::ContributionId]) -> Option<Value> {
    let (t, _) = read(tags);
    let malformed = tags.iter().any(|tag| tag.contains(':') && parse(tag).is_err());
    if !t.any() && !malformed {
        return None;
    }
    let mut problems = problems(statement, tags);
    for (role, id) in &t.refs {
        if !parents.iter().any(|p| p.to_hex() == *id) {
            problems.push(format!("ref:{role}={id} isn't a parent"));
        }
    }
    let refs: Vec<Value> = t.refs.iter().map(|(role, id)| json!({"role": role, "id": id})).collect();
    Some(json!({"type": t.ty, "data": t.data, "refs": refs, "problems": problems}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn each_form_parses_and_malformed_tags_are_named() {
        assert_eq!(parse("finding"), Ok(Tag::Plain("finding")));
        assert_eq!(parse("type:bracken.pr"), Ok(Tag::Type("bracken.pr")));
        assert_eq!(parse("until:2026-01-01T00:00:00Z"), Ok(Tag::Attr("until", "2026-01-01T00:00:00Z")));
        let id = "a".repeat(64);
        assert_eq!(parse(&format!("ref:task={id}")), Ok(Tag::Ref("task", &id)));
        for bad in [
            "",
            "task,deferred",
            "two words",
            "Finding",
            "-x",
            "type:",
            "type:Pr",
            "type:a..b",
            "ref:task",
            "ref:task=abc",
            "Key:v",
            "k:",
            "t\u{1b}[31m",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
        assert!(parse("task,deferred").unwrap_err().contains("comma"));
    }

    #[test]
    fn one_type_one_value_per_key() {
        let (_, p) = read(&tags(&["type:pr", "type:finding"]));
        assert_eq!(p.len(), 1, "{p:?}");
        let (_, p) = read(&tags(&["severity:low", "severity:high"]));
        assert_eq!(p.len(), 1, "{p:?}");
        let id = "b".repeat(64);
        let (t, p) = read(&tags(&[&format!("ref:x={id}"), &format!("ref:y={id}")]));
        assert!(p.is_empty() && t.refs.len() == 2, "{p:?}");
    }

    #[test]
    fn value_kinds() {
        assert!(Kind::Date.holds("2024-02-29") && !Kind::Date.holds("2026-02-29") && !Kind::Date.holds("2026-13-01"));
        assert!(Kind::Count.holds("12") && !Kind::Count.holds("012") && !Kind::Count.holds("0"));
        assert!(
            Kind::Number.holds("-2.5")
                && Kind::Number.holds("15")
                && !Kind::Number.holds("1.")
                && !Kind::Number.holds("x")
        );
        assert!(Kind::Sha.holds(&"0".repeat(40)) && !Kind::Sha.holds(&"0".repeat(39)));
    }

    #[test]
    fn shapes_check_keys_kinds_relations_and_statements() {
        let sha = "c".repeat(40);
        let pr = tags(&["type:pr", "pr", "pr:7", "branch:typed-x", &format!("sha:{sha}")]);
        assert!(check_write(&format!("PR #7 typed-x @ {sha}: Title"), &pr, &[]).is_ok());
        let e = check_write(&format!("PR #8 typed-x @ {sha}: Title"), &pr, &[]).unwrap_err();
        assert!(e.contains("PR #7 typed-x"), "{e}");
        let e = check_write("x", &tags(&["type:finding", "severity:huge"]), &[]).unwrap_err();
        assert!(e.contains("severity") && e.contains("label") && e.contains("--contradicts"), "{e}");
        let f = tags(&["type:finding", "severity:low", "label:possible"]);
        assert!(check_write("x", &f, &[RelationKind::Contradicts]).is_ok());
        assert!(check_write("x", &tags(&["type:fidning"]), &[]).unwrap_err().contains("no type fidning"));
        let exp = tags(&["type:experiment", "metric:m", "baseline:67", "target:15", "expires:2026-12-03"]);
        assert!(check_write("rule: r\nexpires: 2026-12-03", &exp, &[]).is_ok());
        assert!(check_write("rule: r\nexpires: 2026-12-04", &exp, &[]).unwrap_err().contains("expires"));
        assert!(check_write("no rule", &exp, &[]).unwrap_err().contains("rule"));
        let t = tags(&["type:triage", "disposition:deferred", &format!("ref:finding={}", "d".repeat(64))]);
        assert!(check_write("x", &t, &[]).unwrap_err().contains("--ref task"));
        assert!(check_write("x", &tags(&["type:task", "deferred"]), &[]).is_err());
        assert!(check_write("x", &tags(&["type:task", "deferred"]), &[RelationKind::DependsOn]).is_ok());
        // Untyped claims are checked for syntax only.
        assert!(check_write("x", &tags(&["finding", "task"]), &[]).is_ok());
    }

    #[test]
    fn a_scope_judges_a_type_by_its_name_and_plain_tag() {
        assert_eq!(scope_names("type:decision"), vec!["type:decision", "decision", "decision"]);
        assert_eq!(scope_names("type:metric"), vec!["type:metric", "metric", "metrics"]);
        assert_eq!(scope_names("note"), vec!["note"]);
    }
}
