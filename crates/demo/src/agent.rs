//! Agents, and the scripted agent the demo ships with.
//!
//! An agent only touches the Ground through its own replica, with the bracken
//! commands. The harness measures everything from the replicas afterwards,
//! never from what an agent says it did, so any kind of agent can be swapped in.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};

use crate::bracken::{Entry, Replica};
use crate::harness::{Behaviour, Schedule, StaleRule};
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

    /// Ground truth the Ground can't hold: the wip claims whose work this agent dropped
    /// without saying so, and the round it dropped each. Only a scripted agent can know
    /// this; the harness uses it to judge takeovers, as it judges correctness on every input.
    fn silent_drops(&self) -> Vec<(String, u32)> {
        Vec::new()
    }
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
/// implementation itself, but only a fix that implements the same spec, of an
/// implementation another agent's check has failed, and that passes the
/// author's own check. Any other proposal stays a proposal.
///
/// With [`Behaviour::work_rounds`] above 0, an implementation is delivered
/// that many rounds after the agent starts it, and the agent sometimes drops
/// work it started. With [`Behaviour::wip`], it announces each start with a
/// `wip` claim, retracts the claim when it delivers or abandons the work, and
/// doesn't start a spec that another agent's open, fresh wip claim covers. A
/// stale wip claim by another agent stops counting by age alone; an agent
/// that starts the same spec records that it takes the work over with a
/// `refines` relation, since only the claim's author could supersede it.
/// Whether a claim is fresh is each agent's own judgement, from when the claim
/// reached it: another agent's takeover doesn't end a claim, and the round a
/// claim states counts only if it isn't later than that.
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
    /// For other agents' wip claims: the heartbeats I've seen, and the round I first saw that many.
    news: HashMap<String, (usize, u32)>,
    /// For other agents' wip claims: the round I first saw each.
    first_seen: HashMap<String, u32>,
    /// For checking others' proposed fixes of my work, apart from `rng` so that
    /// proposals don't change the rest of the script.
    review_rng: Rng,
    /// Programs proposed as fixes of my work that failed my check, with their spec: each
    /// is checked once, so a wrong fix can't pass by being re-sampled, under the same id
    /// or as a new artifact.
    rejected: HashSet<(String, String)>,
    silent_drops: Vec<(String, u32)>,
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
            news: HashMap::new(),
            first_seen: HashMap::new(),
            silent_drops: Vec::new(),
            review_rng: Rng::derive(seed, (index + 1) | 1 << 32),
            rejected: HashSet::new(),
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
            match (&p.wip, says_so) {
                (Some(wip), true) => {
                    turn.replica.retract(wip, &format!("abandoned in round {}", turn.round))?;
                }
                (Some(wip), false) => self.silent_drops.push((wip.clone(), turn.round)),
                (None, _) => {}
            }
        } else if turn.round >= p.due {
            self.implement(turn, view, &p.start, p.started)?;
            if let Some(wip) = &p.wip {
                turn.replica.retract(wip, &format!("delivered in round {}", turn.round))?;
            }
        } else {
            if let (Some(wip), StaleRule::Liveness) = (&p.wip, self.behaviour.stale_rule) {
                let round = format!("round {}", turn.round);
                turn.replica.evidence(wip, true, HEARTBEAT, &round)?;
            }
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
            .filter(|w| w.author != me && w.spec == spec.name && !w.taken_over && self.is_stale(w, round))
            .map(|w| w.id.clone())
            .collect();
        Some(Start { spec, replaces, takes_over })
    }

    /// Whether an open wip claim is stale, by the behaviour's rule. Under claim-round,
    /// the round the claim states counts only if it isn't later than the round the claim
    /// reached me: claim text can't keep a claim fresh, or overflow the sum.
    fn is_stale(&self, w: &Wip, round: u32) -> bool {
        let since = match self.behaviour.stale_rule {
            StaleRule::ClaimRound => {
                let seen = self.first_seen.get(&w.id).copied().unwrap_or(round);
                w.round.map_or(seen, |stated| stated.min(seen))
            }
            StaleRule::Liveness => self.news.get(&w.id).map_or(round, |&(_, seen)| seen),
        };
        round >= since.saturating_add(self.behaviour.stale_after)
    }

    /// Notes, for each open wip claim, the round it first reached me and the round its
    /// latest heartbeat did.
    fn observe(&mut self, view: &View, round: u32) {
        for w in &view.wips {
            self.first_seen.entry(w.id.clone()).or_insert(round);
            let news = self.news.entry(w.id.clone()).or_insert((w.heartbeats, round));
            if news.0 != w.heartbeats {
                *news = (w.heartbeats, round);
            }
        }
    }

    /// Whether to accept `fix`, another agent's proposed replacement for my `mine`: it
    /// implements the same spec with a different program, another agent's check has
    /// failed `mine`, my own check
    /// fails `mine` too (anyone can record a fail), and `fix` passes my own check. The
    /// checks aren't recorded. A program that fails my check once stays rejected; a
    /// proposal turned down because `mine` passed is checked again next time.
    fn accepts(&mut self, mine: &Impl, fix: &Impl) -> bool {
        let Some(spec) = task::spec(&mine.spec) else { return false };
        let program = (fix.spec.clone(), fix.program.clone());
        if fix.spec != mine.spec
            || fix.program == mine.program
            || mine.fails.is_empty()
            || self.rejected.contains(&program)
        {
            return false;
        }
        if spec.check(&mine.program, &spec.sample(self.sample, &mut self.review_rng)).ok() {
            return false;
        }
        let passes = spec.check(&fix.program, &spec.sample(self.sample, &mut self.review_rng)).ok();
        if !passes {
            self.rejected.insert(program);
        }
        passes
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
        let me = self.name.clone();
        for mine in view.impls.values().filter(|i| i.live() && i.author == me) {
            for fix in &mine.proposed_fixes {
                let Some(fix) = view.impls.get(fix) else { continue };
                if self.accepts(mine, fix) {
                    turn.replica.relate(&fix.id, "supersedes", &mine.id, &format!("accepts {}", fix.name))?;
                    accepted = true;
                }
            }
        }
        if accepted {
            view = View::read(turn)?;
        }
        self.observe(&view, turn.round);

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

    fn silent_drops(&self) -> Vec<(String, u32)> {
        self.silent_drops.clone()
    }
}

/// The evidence method of a heartbeat on a wip claim.
pub const HEARTBEAT: &str = "heartbeat";

/// An open wip claim (not retracted or superseded by its author), as one replica sees it.
struct Wip {
    id: String,
    author: String,
    spec: String,
    /// The round the claim states it was made in, if it states one. Its author's word only.
    round: Option<u32>,
    /// Heartbeats its author has recorded on it.
    heartbeats: usize,
    /// Whether another author's wip claim on the same spec has taken it over (`refines`
    /// it). That doesn't end the claim, since anyone can write such a relation; it only
    /// stops a second takeover.
    taken_over: bool,
}

/// The round a wip claim states, from its trailing `(round N)`. The author's word:
/// see [`ScriptedAgent::is_stale`] for how much it counts.
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
    /// Live implementations by other agents that claim to supersede it, which its
    /// author hasn't accepted: their names and ids.
    proposed_fixes: BTreeSet<(String, String)>,
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
    /// Implementations by artifact name, then id: a name is its author's choice, so
    /// two artifacts may share one.
    impls: BTreeMap<(String, String), Impl>,
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
            let key = (artifact_name.clone(), artifact.id.clone());
            name_of.insert(artifact.id.as_str(), key.clone());
            impls.entry(key).or_insert_with(|| Impl {
                id: artifact.id.clone(),
                name: artifact_name,
                author: name(&artifact.author),
                spec: (*spec).to_owned(),
                program: artifact.text("text").to_owned(),
                retracted: artifact.retracted,
                superseded: false,
                proposed_fixes: BTreeSet::new(),
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
                        i.proposed_fixes.insert(fix.clone());
                    }
                }
                _ => {}
            }
        }
        // Open wip claims: not retracted, and not superseded by their own author. A takeover
        // (another wip claim refining one) is noted but doesn't close it: it's stale or not
        // by each agent's own judgement.
        let relations = || log.iter().filter(|e| e.kind == "relation" && !e.retracted);
        let superseded: BTreeSet<&str> = relations()
            .filter(|e| e.text("kind") == "supersedes" && by_id.get(e.text("to")).is_some_and(|t| t.author == e.author))
            .map(|e| e.text("to"))
            .collect();
        // A takeover is a refines from another author's wip claim on the same spec.
        let wip_spec = |id: &str| match by_id.get(id) {
            Some(c) if c.kind == "claim" => match c.tags()[..] {
                ["wip", spec] => Some((spec, c.author.as_str())),
                _ => None,
            },
            _ => None,
        };
        let taken_over: BTreeSet<&str> = relations()
            .filter(|e| e.text("kind") == "refines")
            .filter(|e| match (wip_spec(e.text("from")), wip_spec(e.text("to"))) {
                (Some((s1, a1)), Some((s2, a2))) => s1 == s2 && a1 != a2,
                _ => false,
            })
            .map(|e| e.text("to"))
            .collect();
        // Heartbeats by their target, counting only those the claim's own author recorded.
        let mut heartbeats: HashMap<&str, usize> = HashMap::new();
        for h in log.iter().filter(|h| h.kind == "evidence" && !h.retracted && h.text("method") == HEARTBEAT) {
            if by_id.get(h.text("target")).is_some_and(|t| t.author == h.author) {
                *heartbeats.entry(h.text("target")).or_default() += 1;
            }
        }
        let mut wips: Vec<Wip> = log
            .iter()
            .filter(|e| e.kind == "claim" && !e.retracted && !superseded.contains(e.id.as_str()))
            .filter_map(|e| match &e.tags()[..] {
                &["wip", spec] if task::spec(spec).is_some() => Some(Wip {
                    id: e.id.clone(),
                    author: name(&e.author),
                    spec: spec.to_owned(),
                    round: wip_round(e.text("statement")),
                    heartbeats: heartbeats.get(e.id.as_str()).copied().unwrap_or(0),
                    taken_over: taken_over.contains(e.id.as_str()),
                }),
                _ => None,
            })
            .collect();
        wips.sort_by(|a, b| (&a.author, a.round, &a.spec).cmp(&(&b.author, b.round, &b.spec)));
        Ok(View { specs, impls, wips })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs_impl(id: &str, author: &str, program: &str, failed_by: &[&str]) -> Impl {
        Impl {
            id: id.to_owned(),
            name: format!("impl/abs/{author}/{id}"),
            author: author.to_owned(),
            spec: "abs".to_owned(),
            program: program.to_owned(),
            retracted: false,
            superseded: false,
            proposed_fixes: BTreeSet::new(),
            checked_by: BTreeSet::new(),
            fails: failed_by.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn a_proposed_fix_that_fails_my_check_once_stays_rejected() {
        // Wrong only at a = -1, so a sample of 6 of the 13 inputs misses the bug about
        // half the time: checked afresh each time, it would be accepted sooner or later.
        // Each proposal is a new artifact with the same program.
        let mut agent = ScriptedAgent::new("agent-1", 1, 1, 6, Behaviour::default());
        let mine = abs_impl("mine", "agent-1", "a", &["agent-2"]);
        let verdicts: Vec<bool> = (0..50)
            .map(|n| agent.accepts(&mine, &abs_impl(&format!("fix{n}"), "agent-0", "a -1 < a neg a ?", &[])))
            .collect();
        let first_no = verdicts.iter().position(|v| !v).expect("some sample finds the bug");
        assert!(verdicts[first_no..].iter().all(|v| !v), "accepted after rejecting: {verdicts:?}");
    }

    #[test]
    fn a_proposed_fix_is_only_accepted_for_failed_work_of_the_same_spec() {
        let mut agent = ScriptedAgent::new("agent-1", 1, 1, 6, Behaviour::default());
        let correct = task::spec("abs").expect("abs").correct[0];
        let fix = abs_impl("fix", "agent-0", correct, &[]);
        assert!(!agent.accepts(&abs_impl("ok", "agent-1", "a", &[]), &fix), "nobody failed it");
        let mut other_spec = abs_impl("fix2", "agent-0", correct, &[]);
        other_spec.spec = "sign".to_owned();
        assert!(!agent.accepts(&abs_impl("bad", "agent-1", "a", &["agent-2"]), &other_spec), "another spec");
        assert!(agent.accepts(&abs_impl("bad", "agent-1", "a", &["agent-2"]), &fix));
    }

    #[test]
    fn a_made_up_fail_on_correct_work_never_gets_a_fix_accepted() {
        // agent-0 records a fail on agent-1's correct implementation, then proposes a copy.
        let mut agent = ScriptedAgent::new("agent-1", 1, 1, 6, Behaviour::default());
        let correct = task::spec("abs").expect("abs").correct[0];
        let mine = abs_impl("mine", "agent-1", correct, &["agent-0"]);
        for n in 0..50 {
            assert!(!agent.accepts(&mine, &abs_impl(&format!("copy{n}"), "agent-0", correct, &[])));
        }
    }
}
