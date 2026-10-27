//! Agents, and the scripted agent the demo ships with.
//!
//! An agent only touches the Ground through its own replica, with the bracken
//! commands. The harness measures everything from the replicas afterwards,
//! never from what an agent says it did, so any kind of agent can be swapped in.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};

use crate::bracken::{Entry, Replica};
use crate::rng::Rng;
use crate::task::{self, Spec};

/// Media type of an implementation artifact.
pub const MEDIA_TYPE: &str = "text/x-demo-rpn";

/// What an agent gets for one round.
pub struct Turn<'a, 'b> {
    /// 1-based.
    pub round: u32,
    /// The agent's replica: its directory, work directory and identity.
    pub replica: &'a Replica<'b>,
    /// Every agent's public key and name. Replicas only know their own
    /// identity's name, so this is how an agent tells who wrote what.
    pub roster: &'a Roster,
}

/// One participant.
///
/// A driver for a real model would implement this by starting a process in
/// `turn.replica.workdir()` with a prompt holding the round and the roster.
/// It acts with `bracken … --json` and checks programs with `bracken-demo check`.
/// Such a driver must give the process a cleared environment holding only
/// `BRACKEN_DIR` (`turn.replica.dir()`), `BRACKEN_AGENT`
/// (`turn.replica.identity()`) and what the model needs to run; the absolute
/// path of the bracken binary (`turn.replica.bracken_bin()`), not a `PATH`
/// lookup; and no access to any git checkout or to a team's `.bracken/`. The
/// harness already refuses to run inside a git checkout.
pub trait Agent {
    fn act(&mut self, turn: &Turn) -> Result<()>;
}

#[derive(Default)]
pub struct Roster(BTreeMap<String, String>);

impl Roster {
    pub fn add(&mut self, key: String, name: String) {
        self.0.insert(key, name);
    }

    /// An agent's name, or `None` for a key outside the team.
    pub fn name(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// An agent that follows a fixed, seeded script: it implements specs, checks
/// others' implementations on a sample of inputs, reports what fails, fixes
/// failing implementations, and sometimes duplicates work it can see.
pub struct ScriptedAgent {
    name: String,
    rng: Rng,
    /// Chance that a new implementation is correct.
    skill: f64,
    /// Chance of implementing a spec someone else already has a live implementation of.
    duplicate_rate: f64,
    /// Chance of fixing someone else's failing implementation instead of starting new work.
    fix_others_rate: f64,
    /// Chance of recording a contradicting finding when a check fails.
    contradict_rate: f64,
    checks_per_round: usize,
    sample: usize,
    written: usize,
}

impl ScriptedAgent {
    pub fn new(name: &str, seed: u64, index: u64, sample: usize) -> ScriptedAgent {
        let mut rng = Rng::derive(seed, index + 1);
        ScriptedAgent {
            name: name.to_owned(),
            skill: rng.between(0.4, 0.8),
            duplicate_rate: rng.between(0.05, 0.25),
            fix_others_rate: rng.between(0.1, 0.4),
            contradict_rate: rng.between(0.3, 0.8),
            checks_per_round: 2,
            sample,
            written: 0,
            rng,
        }
    }

    /// Contributes a new implementation of `spec`, replacing `replaces` if given.
    fn implement(&mut self, turn: &Turn, view: &View, spec: &Spec, replaces: Option<&Impl>) -> Result<()> {
        // A fix is more often right: the failing check says what went wrong.
        let skill = if replaces.is_some() { (self.skill + 0.2).min(0.95) } else { self.skill };
        let pool = if self.rng.chance(skill) { spec.correct } else { spec.buggy };
        let program = self.rng.pick(pool).expect("every spec has candidates");
        self.written += 1;
        let name = format!("impl/{}/{}/{}", spec.name, self.name, self.written);
        let file = format!("{}-{}.rpn", spec.name, self.written);
        let artifact = turn.replica.artifact(&file, &name, MEDIA_TYPE, program)?;
        let spec_id = &view.specs[spec.name];
        turn.replica.relate(&artifact, "implements", spec_id, "")?;
        if let Some(old) = replaces {
            turn.replica.relate(&artifact, "supersedes", &old.id, &format!("fixes {}", old.name))?;
        }
        Ok(())
    }

    fn check(&mut self, turn: &Turn, target: &Impl) -> Result<()> {
        let spec = task::spec(&target.spec).expect("views only hold known specs");
        let cases = spec.sample(self.sample, &mut self.rng);
        let result = spec.check(&target.program, &cases);
        let method = format!("bracken-demo check {} --sample {}", spec.name, self.sample);
        let evidence = turn.replica.evidence(&target.id, result.ok(), &method, &result.details())?;
        if let Some(failure) = &result.first_failure
            && self.rng.chance(self.contradict_rate)
        {
            let statement = format!("{} is wrong: {failure}", target.name);
            let finding = turn.replica.claim(&statement, &["finding", spec.name])?;
            turn.replica.relate(&finding, "contradicts", &target.id, "")?;
            turn.replica.relate(&finding, "depends_on", &evidence, "")?;
        }
        Ok(())
    }
}

impl Agent for ScriptedAgent {
    fn act(&mut self, turn: &Turn) -> Result<()> {
        let view = View::read(turn)?;
        let me_owned = self.name.clone();
        let me = me_owned.as_str();
        let live: Vec<&Impl> = view.impls.values().filter(|i| i.live()).collect();

        // 1. Fix my own failing work first, sometimes someone else's, else start something.
        let mine_failing = live.iter().find(|i| i.author == me && !i.fails.is_empty());
        let others_failing: Vec<&Impl> =
            live.iter().copied().filter(|i| i.author != me && !i.fails.is_empty()).collect();
        if let Some(old) = mine_failing {
            let spec = task::spec(&old.spec).expect("views only hold known specs");
            self.implement(turn, &view, spec, Some(old))?;
        } else if !others_failing.is_empty() && self.rng.chance(self.fix_others_rate) {
            let old = *self.rng.pick(&others_failing).expect("not empty");
            let spec = task::spec(&old.spec).expect("views only hold known specs");
            self.implement(turn, &view, spec, Some(old))?;
        } else {
            // Specs with no live implementation that hasn't failed.
            let covered: BTreeSet<&str> = live.iter().filter(|i| i.fails.is_empty()).map(|i| i.spec.as_str()).collect();
            let open: Vec<&Spec> =
                task::SPECS.iter().filter(|s| view.specs.contains_key(s.name) && !covered.contains(s.name)).collect();
            let mine: BTreeSet<&str> = live.iter().filter(|i| i.author == me).map(|i| i.spec.as_str()).collect();
            let theirs: Vec<&Spec> =
                task::SPECS.iter().filter(|s| covered.contains(s.name) && !mine.contains(s.name)).collect();
            // With every spec covered, only an agent inclined to duplicate starts more work.
            let choice = if self.rng.chance(self.duplicate_rate) {
                self.rng.pick(&theirs).or_else(|| self.rng.pick(&open))
            } else {
                self.rng.pick(&open)
            };
            if let Some(spec) = choice {
                self.implement(turn, &view, spec, None)?;
            }
        }

        // 2. Check others' live work that I haven't checked, least-checked first.
        let mut unchecked: Vec<&Impl> =
            live.iter().copied().filter(|i| i.author != me && !i.checked_by.contains(me)).collect();
        unchecked.sort_by_key(|i| (i.checked_by.len(), i.name.clone()));
        for target in unchecked.into_iter().take(self.checks_per_round) {
            self.check(turn, target)?;
        }
        Ok(())
    }
}

/// An implementation, as one replica sees it.
struct Impl {
    id: String,
    /// The artifact's name, `impl/<spec>/<agent>/<n>`.
    name: String,
    author: String,
    spec: String,
    program: String,
    retracted: bool,
    superseded: bool,
    checked_by: BTreeSet<String>,
    fails: BTreeSet<String>,
}

impl Impl {
    fn live(&self) -> bool {
        !self.retracted && !self.superseded
    }
}

/// What the scripted agent reads from its replica. It is keyed and ordered by
/// names, never by contribution ids: ids include timestamps, so ordering by
/// them would make a seed's run depend on the clock.
struct View {
    /// Spec name to its claim's id.
    specs: HashMap<String, String>,
    /// Implementations by artifact name.
    impls: BTreeMap<String, Impl>,
}

impl View {
    fn read(turn: &Turn) -> Result<View> {
        let log = turn.replica.log().context("reading my replica")?;
        let by_id: HashMap<&str, &Entry> = log.iter().map(|e| (e.id.as_str(), e)).collect();
        let name = |key: &str| turn.roster.name(key).unwrap_or("?").to_owned();

        let mut specs = HashMap::new();
        let mut spec_of_claim = HashMap::new();
        for e in log.iter().filter(|e| e.kind == "claim" && !e.retracted) {
            if let ["spec", spec] = e.tags()[..] {
                specs.insert(spec.to_owned(), e.id.clone());
                spec_of_claim.insert(e.id.as_str(), spec);
            }
        }

        let mut impls = BTreeMap::new();
        let mut name_of = HashMap::new();
        for e in log.iter().filter(|e| e.kind == "relation" && !e.retracted && e.text("kind") == "implements") {
            let (Some(artifact), Some(spec)) = (by_id.get(e.text("from")), spec_of_claim.get(e.text("to"))) else {
                continue;
            };
            if artifact.kind != "artifact" || artifact.author != e.author || task::spec(spec).is_none() {
                continue;
            }
            let artifact_name = artifact.text("name").to_owned();
            name_of.insert(artifact.id.as_str(), artifact_name.clone());
            impls.entry(artifact_name.clone()).or_insert_with(|| Impl {
                id: artifact.id.clone(),
                name: artifact_name,
                author: name(&artifact.author),
                spec: (*spec).to_owned(),
                program: artifact.text("text").to_owned(),
                retracted: artifact.retracted,
                superseded: false,
                checked_by: BTreeSet::new(),
                fails: BTreeSet::new(),
            });
        }

        for e in log.iter().filter(|e| !e.retracted) {
            match e.kind.as_str() {
                "evidence" => {
                    let Some(target) = name_of.get(e.text("target")) else { continue };
                    let i = impls.get_mut(target).expect("named impls exist");
                    let who = name(&e.author);
                    if e.text("verdict") == "fail" && e.author != by_id[i.id.as_str()].author {
                        i.fails.insert(who.clone());
                    }
                    i.checked_by.insert(who);
                }
                // Supersession counts only when the relation's author wrote the replacement.
                "relation" if e.text("kind") == "supersedes" => {
                    let from_author = by_id.get(e.text("from")).map(|f| f.author.as_str());
                    if from_author == Some(e.author.as_str())
                        && let Some(target) = name_of.get(e.text("to"))
                    {
                        impls.get_mut(target).expect("named impls exist").superseded = true;
                    }
                }
                _ => {}
            }
        }
        Ok(View { specs, impls })
    }
}
