//! Agents, and the scripted agent the demo ships with.
//!
//! An agent only touches the Ground through its own replica, with the bracken
//! commands. The harness measures everything from the replicas afterwards,
//! never from what an agent says it did, so any kind of agent can be swapped in.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};

use crate::bracken::{Entry, Replica};
use crate::harness::{Behaviour, Schedule};
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
///
/// A fix `supersedes` the implementation it replaces, but under the protocol
/// that only counts when the replaced implementation's author writes the
/// relation. So an agent that finds others' fixes of its own work accepts
/// them on its next turn, by relating each fix as superseding its
/// implementation itself.
///
/// With [`Behaviour::work_rounds`] above 0, an implementation is delivered
/// that many rounds after the agent starts it, and the agent sometimes drops
/// work it started. With [`Behaviour::wip`], it announces each start with a
/// `wip` claim, retracts the claim when it delivers or abandons the work, and
/// doesn't start a spec that another agent's open, fresh wip claim covers. A
/// stale wip claim by another agent stops counting by age alone; an agent
/// that starts the same spec records that it takes the work over with a
/// `refines` relation, since only the claim's author could supersede it.
pub struct ScriptedAgent {
    name: String,
    rng: Rng,
    behaviour: Behaviour,
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
    /// Work started and not yet delivered or dropped.
    pending: Option<Pending>,
    /// The first round in which this agent starts work.
    first_round: u32,
}

/// What an agent decided to work on.
struct Start {
    spec: &'static Spec,
    /// The failing implementation this replaces: its id and name.
    replaces: Option<(String, String)>,
    /// Stale wip claims by others on the same spec, which this start takes over (`refines`).
    takes_over: Vec<String>,
}

struct Pending {
    start: Start,
    started: u32,
    due: u32,
    /// The wip claim announcing it.
    wip: Option<String>,
}

impl ScriptedAgent {
    pub fn new(name: &str, seed: u64, index: u64, sample: usize, behaviour: Behaviour) -> ScriptedAgent {
        let mut rng = Rng::derive(seed, index + 1);
        ScriptedAgent {
            name: name.to_owned(),
            behaviour,
            skill: rng.between(0.4, 0.8),
            duplicate_rate: rng.between(0.05, 0.25),
            fix_others_rate: rng.between(0.1, 0.4),
            contradict_rate: rng.between(0.3, 0.8),
            checks_per_round: 2,
            sample,
            written: 0,
            pending: None,
            first_round: behaviour.first_round(index),
            rng,
        }
    }

    /// Contributes the implementation for `start`, begun in round `started`.
    fn implement(&mut self, turn: &Turn, view: &View, start: &Start, started: u32) -> Result<()> {
        let spec = start.spec;
        // A fix is more often right: the failing check says what went wrong.
        let skill = if start.replaces.is_some() { (self.skill + 0.2).min(0.95) } else { self.skill };
        let pool = if self.rng.chance(skill) { spec.correct } else { spec.buggy };
        let program = self.rng.pick(pool).expect("every spec has candidates");
        self.written += 1;
        let name = format!("impl/{}/{}/{}", spec.name, self.name, self.written);
        let file = format!("{}-{}.rpn", spec.name, self.written);
        let artifact = turn.replica.artifact(&file, &name, MEDIA_TYPE, program)?;
        let spec_id = &view.specs[spec.name];
        turn.replica.relate(&artifact, "implements", spec_id, &format!("started in round {started}"))?;
        if let Some((old, old_name)) = &start.replaces {
            turn.replica.relate(&artifact, "supersedes", old, &format!("fixes {old_name}"))?;
        }
        Ok(())
    }

    /// Starts work that will be delivered later, announcing it if wip is on.
    fn begin(&mut self, turn: &Turn, view: &View, start: Start) -> Result<()> {
        let mut wip = None;
        if self.behaviour.wip {
            let what = match &start.replaces {
                Some((_, old_name)) => format!("fixing {old_name}"),
                None => format!("implementing {}", start.spec.name),
            };
            let statement = format!("Work in progress: {} is {what} (round {})", self.name, turn.round);
            let claim = turn.replica.claim(&statement, &["wip", start.spec.name])?;
            turn.replica.relate(&claim, "refines", &view.specs[start.spec.name], "")?;
            for stale in &start.takes_over {
                turn.replica.relate(&claim, "refines", stale, "takes over stale work")?;
            }
            wip = Some(claim);
        }
        let jitter = if self.behaviour.schedule == Schedule::Jitter { self.rng.below(2) as u32 } else { 0 };
        let due = turn.round + self.behaviour.work_rounds + jitter;
        self.pending = Some(Pending { start, started: turn.round, due, wip });
        Ok(())
    }

    /// Continues started work: drops it, delivers it when due, or keeps at it.
    fn work(&mut self, turn: &Turn, view: &View, p: Pending) -> Result<()> {
        if self.rng.chance(self.behaviour.drop_rate) {
            // Half the time the agent says so; otherwise its claim goes stale.
            let says_so = self.rng.chance(0.5);
            if let (Some(wip), true) = (&p.wip, says_so) {
                turn.replica.retract(wip, &format!("abandoned in round {}", turn.round))?;
            }
        } else if turn.round >= p.due {
            self.implement(turn, view, &p.start, p.started)?;
            if let Some(wip) = &p.wip {
                turn.replica.retract(wip, &format!("delivered in round {}", turn.round))?;
            }
        } else {
            self.pending = Some(p);
        }
        Ok(())
    }

    /// Picks what to work on next: my failing work first, sometimes someone
    /// else's, else a spec nobody covers, and sometimes one somebody does.
    fn choose(&mut self, view: &View, round: u32) -> Option<Start> {
        let me = self.name.as_str();
        let live: Vec<&Impl> = view.impls.values().filter(|i| i.live()).collect();
        // Specs another agent has announced and is still within time on.
        let claimed: BTreeSet<&str> = if self.behaviour.wip {
            view.wips.iter().filter(|w| w.author != me && !self.is_stale(w, round)).map(|w| w.spec.as_str()).collect()
        } else {
            BTreeSet::new()
        };
        let free = |spec: &str| !claimed.contains(spec);
        let fix = |i: &Impl| Some((i.id.clone(), i.name.clone()));

        let mine_failing = live.iter().find(|i| i.author == me && !i.fails.is_empty() && free(&i.spec));
        let others_failing: Vec<&Impl> =
            live.iter().copied().filter(|i| i.author != me && !i.fails.is_empty() && free(&i.spec)).collect();
        let (spec, replaces) = if let Some(old) = mine_failing {
            (task::spec(&old.spec).expect("views only hold known specs"), fix(old))
        } else if !others_failing.is_empty() && self.rng.chance(self.fix_others_rate) {
            let old = *self.rng.pick(&others_failing).expect("not empty");
            (task::spec(&old.spec).expect("views only hold known specs"), fix(old))
        } else {
            // Specs with no live implementation that hasn't failed.
            let covered: BTreeSet<&str> = live.iter().filter(|i| i.fails.is_empty()).map(|i| i.spec.as_str()).collect();
            let open: Vec<&Spec> = task::SPECS
                .iter()
                .filter(|s| view.specs.contains_key(s.name) && !covered.contains(s.name) && free(s.name))
                .collect();
            let mine: BTreeSet<&str> = live.iter().filter(|i| i.author == me).map(|i| i.spec.as_str()).collect();
            let theirs: Vec<&Spec> = task::SPECS
                .iter()
                .filter(|s| covered.contains(s.name) && !mine.contains(s.name) && free(s.name))
                .collect();
            // With every spec covered, only an agent inclined to duplicate starts more work.
            let choice = if self.rng.chance(self.duplicate_rate) {
                self.rng.pick(&theirs).or_else(|| self.rng.pick(&open))
            } else {
                self.rng.pick(&open)
            };
            (*choice?, None)
        };
        let takes_over = view
            .wips
            .iter()
            .filter(|w| w.author != me && w.spec == spec.name && self.is_stale(w, round))
            .map(|w| w.id.clone())
            .collect();
        Some(Start { spec, replaces, takes_over })
    }

    /// An open wip claim is stale once `stale_after` rounds have passed since the round it states.
    fn is_stale(&self, w: &Wip, round: u32) -> bool {
        round >= w.round + self.behaviour.stale_after
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
        let mut view = View::read(turn)?;

        // 0. Accept others' fixes of my live implementations: only my relation retires mine.
        let mut accepted = false;
        for mine in view.impls.values().filter(|i| i.live() && i.author == self.name) {
            for (fix_name, fix_id) in &mine.proposed_fixes {
                turn.replica.relate(fix_id, "supersedes", &mine.id, &format!("accepts {fix_name}"))?;
                accepted = true;
            }
        }
        if accepted {
            view = View::read(turn)?;
        }

        // 1. Carry on with started work, or start something.
        if let Some(p) = self.pending.take() {
            self.work(turn, &view, p)?;
        } else if turn.round < self.first_round {
            // Not started yet: this agent joins later.
        } else if let Some(start) = self.choose(&view, turn.round) {
            if self.behaviour.work_rounds == 0 && !self.behaviour.wip {
                self.implement(turn, &view, &start, turn.round)?;
            } else {
                self.begin(turn, &view, start)?;
                if self.behaviour.work_rounds == 0 {
                    let p = self.pending.take().expect("just started");
                    self.work(turn, &view, p)?;
                }
            }
        }

        // 2. Check others' live work that I haven't checked, least-checked first.
        let me = self.name.as_str();
        let mut unchecked: Vec<&Impl> =
            view.impls.values().filter(|i| i.live() && i.author != me && !i.checked_by.contains(me)).collect();
        unchecked.sort_by_key(|i| (i.checked_by.len(), i.name.clone()));
        for target in unchecked.into_iter().take(self.checks_per_round) {
            self.check(turn, target)?;
        }
        Ok(())
    }
}

/// An open wip claim (not retracted, superseded by its author, or taken over), as one replica sees it.
struct Wip {
    id: String,
    author: String,
    spec: String,
    /// The round the claim states it was made in.
    round: u32,
}

/// The round a wip claim states, from its trailing `(round N)`.
pub fn wip_round(statement: &str) -> Option<u32> {
    statement.strip_suffix(')')?.rsplit_once("(round ")?.1.parse().ok()
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
    /// A supersession that counts: written by this implementation's author.
    superseded: bool,
    /// Live fixes by other agents that claim to supersede it, which its author
    /// hasn't accepted: fix name to fix id.
    proposed_fixes: BTreeMap<String, String>,
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
    /// Open wip claims, ordered by author and round.
    wips: Vec<Wip>,
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
                proposed_fixes: BTreeMap::new(),
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
                // Supersession counts only when the relation's author wrote the replaced
                // implementation. Another agent's fix is only a proposal until then.
                "relation" if e.text("kind") == "supersedes" => {
                    let Some(target) = name_of.get(e.text("to")) else { continue };
                    let target_author = by_id[e.text("to")].author.as_str();
                    let i = impls.get_mut(target).expect("named impls exist");
                    if e.author == target_author {
                        i.superseded = true;
                    } else if let Some(fix) = name_of.get(e.text("from"))
                        && by_id[e.text("from")].author == e.author
                        && !by_id[e.text("from")].retracted
                    {
                        i.proposed_fixes.insert(fix.clone(), e.text("from").to_owned());
                    }
                }
                _ => {}
            }
        }
        // Open wip claims: not retracted, not superseded by their own author, and not taken
        // over (refined by another wip claim) as stale.
        let is_wip = |id: &str| by_id.get(id).is_some_and(|c| c.kind == "claim" && c.tags().first() == Some(&"wip"));
        let superseded: BTreeSet<&str> = log
            .iter()
            .filter(|e| e.kind == "relation" && !e.retracted)
            .filter(|e| match e.text("kind") {
                "supersedes" => by_id.get(e.text("to")).is_some_and(|t| t.author == e.author),
                "refines" => is_wip(e.text("from")) && is_wip(e.text("to")),
                _ => false,
            })
            .map(|e| e.text("to"))
            .collect();
        let mut wips: Vec<Wip> = log
            .iter()
            .filter(|e| e.kind == "claim" && !e.retracted && !superseded.contains(e.id.as_str()))
            .filter_map(|e| match (&e.tags()[..], wip_round(e.text("statement"))) {
                (&["wip", spec], Some(round)) if task::spec(spec).is_some() => {
                    Some(Wip { id: e.id.clone(), author: name(&e.author), spec: spec.to_owned(), round })
                }
                _ => None,
            })
            .collect();
        wips.sort_by(|a, b| (&a.author, a.round, &a.spec).cmp(&(&b.author, b.round, &b.spec)));
        Ok(View { specs, impls, wips })
    }
}
