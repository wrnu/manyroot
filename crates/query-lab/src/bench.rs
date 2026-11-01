//! The benchmark: each representative query (Q1 to Q9 of design note `f2cf04a0a0b7`) the CLI's way, as
//! the processes an agent or script runs, and this crate's way, in this process, with the answers compared
//! id for id. The CLI is the oracle; a mismatch is reported, never hidden.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::datalog::{self, Options};
use crate::ground::{Fact, Ground};
use crate::pipeline;

/// Runs the CLI against one replica, counting the processes.
pub struct Cli {
    pub bin: PathBuf,
    pub dir: PathBuf,
    pub calls: usize,
}

impl Cli {
    pub fn json(&mut self, args: &[&str]) -> Result<Value> {
        self.calls += 1;
        let out = Command::new(&self.bin)
            .args(args)
            .env("BRACKEN_DIR", &self.dir)
            .env_remove("BRACKEN_AGENT")
            .output()
            .with_context(|| format!("running {}", self.bin.display()))?;
        if !out.status.success() {
            bail!("{} {}: {}", self.bin.display(), args.join(" "), String::from_utf8_lossy(&out.stderr));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }
}

pub enum Ours {
    Pipeline(String),
    /// A program whose last query's first column is the answer.
    Datalog {
        program: String,
        triager: String,
        min_passes: i64,
        ids: Vec<(String, String)>,
    },
}

type CliWay = Box<dyn Fn(&mut Cli) -> Result<Vec<String>>>;

pub struct Case {
    pub name: String,
    pub cli: CliWay,
    pub ours: Ours,
    /// Whether the CLI's way answers in causal order; if not, the answers are compared as sets.
    pub ordered: bool,
}

fn ids(v: &Value) -> Result<Vec<String>> {
    let list = v
        .as_array()
        .or_else(|| ["contributions", "referrers", "accepted"].iter().find_map(|k| v.get(*k).and_then(Value::as_array)))
        .context("not a list of contributions")?;
    list.iter().map(|c| c["id"].as_str().map(str::to_owned).context("an entry without an id")).collect()
}

fn one(args: &str) -> CliWay {
    let args: Vec<String> = args.split_whitespace().map(str::to_owned).collect();
    Box::new(move |cli| ids(&cli.json(&args.iter().map(String::as_str).collect::<Vec<_>>())?))
}

fn dl(program: &str, triager: &str) -> Ours {
    Ours::Datalog { program: program.to_owned(), triager: triager.to_owned(), min_passes: 1, ids: vec![] }
}

/// The current pr claim with the most contradicts links: what scripts/merge's findings check reads most.
fn busiest_pr(g: &Ground) -> Option<u32> {
    let hidden = pipeline::hidden(g, &g.retracted());
    let pr = *g.tag_of.get("pr")?;
    g.by_tag[pr as usize].iter().copied().filter(|r| !hidden[*r as usize]).max_by_key(|r| {
        g.referrers[*r as usize]
            .iter()
            .filter(|x| matches!(g.fact[**x as usize], Fact::Relation { kind: manyroot::RelationKind::Contradicts, to, .. } if to == *r))
            .count()
    })
}

/// scripts/merge's findings check (section 3), its reads and its jq, for one claim: the findings that
/// block it. Mirrors the script line for line, through the same CLI calls.
fn merge_findings(claim: String, triager: String) -> CliWay {
    Box::new(move |cli| {
        let shown = cli.json(&["show", &claim, "--json"])?;
        let full = shown["id"].as_str().context("an id")?.to_owned();
        let _current_prs = cli.json(&["log", "--tag", "pr", "--current", "--json"])?; // section 1's read
        let contradicts: Vec<&Value> = shown["referrers"]
            .as_array()
            .context("referrers")?
            .iter()
            .filter(|r| {
                r["kind"] == "relation" && r["body"]["kind"] == "contradicts" && r["body"]["to"] == full.as_str()
            })
            .collect();
        let mut findings: Vec<String> =
            contradicts.iter().filter_map(|r| r["body"]["from"].as_str().map(str::to_owned)).collect();
        findings.sort();
        findings.dedup();
        let tasks = cli.json(&["log", "--tag", "task", "--current", "--json"])?;
        let mut blocking = Vec::new();
        for f in findings {
            let about = cli.json(&["show", &f, "--json"])?;
            if about["retracted"] != false {
                continue;
            }
            let by = about["author"].as_str().unwrap_or_default();
            let counts = contradicts.iter().any(|r| {
                r["body"]["from"] == f.as_str()
                    && (r["retracted"] == false || (r["author"] != by && r["author_name"] != triager.as_str()))
            });
            if !counts {
                continue;
            }
            let refs = about["referrers"].as_array().context("referrers")?;
            let mine = |r: &&Value| r["retracted"] == false && r["author_name"] == triager.as_str();
            let verdicts: Vec<&Value> = refs
                .iter()
                .filter(mine)
                .filter(|r| r["kind"] == "evidence" && r["body"]["target"] == f.as_str())
                .map(|r| &r["body"]["verdict"])
                .collect();
            if verdicts.is_empty() {
                blocking.push(f);
                continue;
            }
            if !verdicts.iter().any(|v| *v == "pass") {
                continue; // rejected: doesn't block
            }
            let deferred = refs
                .iter()
                .filter(mine)
                .filter(|r| {
                    r["kind"] == "relation" && r["body"]["kind"] == "depends_on" && r["body"]["to"] == f.as_str()
                })
                .any(|r| {
                    tasks.as_array().is_some_and(|ts| {
                        ts.iter().any(|t| {
                            t["id"] == r["body"]["from"]
                                && t["author_name"] == triager.as_str()
                                && t["body"]["tags"].as_array().is_some_and(|x| x.iter().any(|y| y == "deferred"))
                        })
                    })
                });
            if !deferred {
                blocking.push(f);
            }
        }
        Ok(blocking)
    })
}

/// Q1 to Q9. `back` places the cursor that many rows before the end; `about` is the id `show` reads.
pub fn cases(g: &Ground, triager: &str, back: usize, about: &str) -> Result<Vec<Case>> {
    anyhow::ensure!(!g.is_empty(), "the Ground has no contributions to query");
    let cursor = g.ids[g.len().saturating_sub(back + 1)].to_hex();
    let case = |name: &str, cli: CliWay, ours: Ours| Case { name: name.to_owned(), cli, ours, ordered: true };
    let pipe = |s: &str| Ours::Pipeline(s.to_owned());
    let t = triager.to_owned();
    let mut out = vec![
        case("Q1 decisions, current", one("log --tag decision --current --json"), pipe("tag decision | current")),
        case("Q1 pr claims, current", one("log --tag pr --current --json"), pipe("tag pr | current")),
        case(
            "Q1 evidence by the triager",
            one(&format!("log --kind evidence --author {triager} --json")),
            pipe(&format!("kind evidence | author {triager}")),
        ),
        case("Q2 current, pipeline", one("log --current --json"), pipe("current")),
        case("Q2 current, datalog", one("log --current --json"), dl("?- current(X).", triager)),
        case(
            "Q3 findings since a cursor",
            one(&format!("log --since {cursor} --tag finding --json")),
            pipe(&format!("since {cursor} | tag finding")),
        ),
        case(
            "Q3 since a date",
            one("log --since 2026-10-30 --kind claim --json"),
            pipe("since 2026-10-30 | kind claim"),
        ),
        case("Q4 referrers of an id", one(&format!("show {about} --json")), pipe(&format!("refs {about}"))),
        case("Q5 accepted, default policy", one("accepted --json"), dl("?- accepted(X).", triager)),
        case(
            "Q5 accepted, two passes",
            one("accepted --min-passes 2 --json"),
            Ours::Datalog { program: "?- accepted(X).".into(), triager: t.clone(), min_passes: 2, ids: vec![] },
        ),
        case(
            "Q6 open findings",
            one(&format!("log --open --triager {triager} --json")),
            dl("?- open_finding(F).", triager),
        ),
    ];
    // Q7: the current pr claim for the latest PR, as scripts/merge's section 1 finds it.
    if let Some(prefix) =
        g.tag_of.get("pr").and_then(|t| g.by_tag[*t as usize].last()).and_then(|r| match &g.fact[*r as usize] {
            Fact::Claim { statement, .. } => {
                statement.split_once(' ').and_then(|(_, rest)| rest.split_once(' ')).map(|(n, _)| format!("PR {n} "))
            }
            _ => None,
        })
    {
        let p = prefix.clone();
        let cli: CliWay = Box::new(move |cli| {
            let all = cli.json(&["log", "--tag", "pr", "--current", "--json"])?;
            Ok(all
                .as_array()
                .context("an array")?
                .iter()
                .filter(|c| c["body"]["statement"].as_str().is_some_and(|s| s.starts_with(&p)))
                .filter(|c| c["superseded_by_others"].as_array().is_none_or(Vec::is_empty))
                .filter_map(|c| c["id"].as_str().map(str::to_owned))
                .collect())
        });
        let program =
            format!("?- claim(C, S), tag(C, \"pr\"), starts_with(S, \"{prefix}\"), current(C), !others_supersede(C).");
        out.push(case(&format!("Q7 current claim for {}", prefix.trim()), cli, dl(&program, triager)));
    }
    // Q8: a contribution's causal ancestors. The CLI has no ancestry read; its cursor gives the complement.
    let d = g.ids[g.len() / 2].to_hex();
    let dd = d.clone();
    let cli: CliWay = Box::new(move |cli| {
        let all = ids(&cli.json(&["log", "--json"])?)?;
        let after: std::collections::HashSet<String> =
            ids(&cli.json(&["log", "--since", &dd, "--json"])?)?.into_iter().collect();
        Ok(all.into_iter().filter(|i| !after.contains(i) && *i != dd).collect())
    });
    out.push(case(
        "Q8 ancestors of an id",
        cli,
        Ours::Datalog {
            program: "?- ancestor(A, $d).".into(),
            triager: t.clone(),
            min_passes: 1,
            ids: vec![("d".into(), d)],
        },
    ));
    // Q9: scripts/merge's findings check for the busiest current pr claim.
    if let Some(row) = busiest_pr(g) {
        let claim = g.ids[row as usize].to_hex();
        out.push(Case {
            ordered: false, // the script sorts the findings by id
            ..case(
                "Q9 merge's findings check",
                merge_findings(claim.clone(), t.clone()),
                Ours::Datalog {
                    program: "?- gate_blocking(F).".into(),
                    triager: t.clone(),
                    min_passes: 1,
                    ids: vec![("claim".into(), claim)],
                },
            )
        });
    }
    Ok(out)
}

pub struct Outcome {
    pub name: String,
    pub cli: Duration,
    pub calls: usize,
    pub warm: Duration,
    pub rows: usize,
    pub same: bool,
    pub note: String,
}

/// Runs each case `runs` times both ways and keeps the minimum of each.
pub fn run(dir: &Path, bracken: &Path, g: &Ground, cases: &[Case], runs: usize) -> Result<Vec<Outcome>> {
    let mut out = Vec::new();
    for case in cases {
        let mut cli_time = Duration::MAX;
        let mut expected = Vec::new();
        let mut calls = 0;
        for _ in 0..runs {
            let mut cli = Cli { bin: bracken.to_owned(), dir: dir.to_owned(), calls: 0 };
            let t = Instant::now();
            expected = (case.cli)(&mut cli).with_context(|| case.name.clone())?;
            cli_time = cli_time.min(t.elapsed());
            calls = cli.calls;
        }
        let mut warm = Duration::MAX;
        let mut ours = Vec::new();
        for _ in 0..runs.max(10) {
            let t = Instant::now();
            ours = answer(g, &case.ours).with_context(|| case.name.clone())?;
            warm = warm.min(t.elapsed());
        }
        let mut ours: Vec<String> = ours.iter().map(|r| g.ids[*r as usize].to_hex()).collect();
        if !case.ordered {
            ours.sort();
            expected.sort();
        }
        let same = ours == expected;
        let note = if same {
            String::new()
        } else {
            let missing = expected.iter().filter(|i| !ours.contains(i)).count();
            let extra = ours.iter().filter(|i| !expected.contains(i)).count();
            format!("cli {} rows, ours {}: {missing} missing, {extra} extra", expected.len(), ours.len())
        };
        out.push(Outcome { name: case.name.clone(), cli: cli_time, calls, warm, rows: ours.len(), same, note });
    }
    Ok(out)
}

/// As [`answer`], with the Datalog base relations already built.
pub fn answer_in(session: &mut datalog::Session, g: &Ground, ours: &Ours) -> Result<Vec<u32>> {
    match ours {
        Ours::Pipeline(text) => pipeline::run(g, &pipeline::parse(text)?),
        Ours::Datalog { program, triager, min_passes, ids } => {
            let opts = Options { triager: Some(triager), validators: &[], min_passes: *min_passes, ids, magic: true };
            let answers = session.run(&opts, program)?;
            Ok(datalog::ids(answers.last().context("a program with a query")?))
        }
    }
}

pub fn answer(g: &Ground, ours: &Ours) -> Result<Vec<u32>> {
    match ours {
        Ours::Pipeline(text) => pipeline::run(g, &pipeline::parse(text)?),
        Ours::Datalog { program, triager, min_passes, ids } => {
            let opts = Options { triager: Some(triager), validators: &[], min_passes: *min_passes, ids, magic: true };
            let (_, answers) = datalog::run(g, &opts, program)?;
            Ok(datalog::ids(answers.last().context("a program with a query")?))
        }
    }
}
