//! The scripted agents against an adversarial agent: agent-0 writes what a real
//! model could, and the honest agents must not let it retire, hide or block
//! their work.
//!
//! Like tests/demo.rs, these drive the `bracken` binary cargo builds next to the demo.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use bracken_demo::agent::{Agent, MEDIA_TYPE, ScriptedAgent, Turn};
use bracken_demo::bracken::{Bracken, Entry};
use bracken_demo::harness::{self, Behaviour, Config, Run, Schedule, StaleRule, Topology};
use bracken_demo::report::{self, Report};
use bracken_demo::task;

/// Agent-0, acting by a closure.
struct Adversary<F>(F);

impl<F: FnMut(&Turn) -> Result<()>> Agent for Adversary<F> {
    fn act(&mut self, turn: &Turn) -> Result<()> {
        (self.0)(turn)
    }
}

fn bracken_bin() -> PathBuf {
    let demo = PathBuf::from(env!("CARGO_BIN_EXE_bracken-demo"));
    demo.with_file_name(format!("bracken{}", std::env::consts::EXE_SUFFIX))
}

fn config(agents: usize, rounds: u32, seed: u64, behaviour: Behaviour) -> Config {
    Config { agents, rounds, seed, topology: Topology::Full, sample: 6, behaviour }
}

/// Runs `cfg` with agent-0 the adversary and the rest scripted.
fn run(cfg: &Config, adversary: impl FnMut(&Turn) -> Result<()> + 'static) -> (Run, Report) {
    run_with(cfg, Box::new(Adversary(adversary)))
}

/// Runs `cfg` with agent-0 `first` and the rest scripted.
fn run_with(cfg: &Config, first: Box<dyn Agent>) -> (Run, Report) {
    let bracken = Bracken::new(&bracken_bin()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut first = Some(first);
    let run = harness::run(&bracken, tmp.path(), cfg, |i, name| -> Box<dyn Agent> {
        match first.take() {
            Some(a) if i == 0 => a,
            _ => Box::new(ScriptedAgent::new(name, cfg.seed, i as u64, cfg.sample, cfg.behaviour)),
        }
    })
    .unwrap();
    let report = report::build(cfg, &run, Duration::ZERO, 0, Duration::ZERO);
    (run, report)
}

fn spec_ids(log: &[Entry]) -> Vec<(String, String)> {
    log.iter()
        .filter(|e| e.kind == "claim")
        .filter_map(|e| match e.tags()[..] {
            ["spec", name] => Some((name.to_owned(), e.id.clone())),
            _ => None,
        })
        .collect()
}

/// Others' implementations: artifact id and spec name.
fn others_impls(log: &[Entry]) -> Vec<(String, String)> {
    let specs = spec_ids(log);
    log.iter()
        .filter(|r| r.kind == "relation" && r.text("kind") == "implements")
        .filter_map(|r| {
            let spec = specs.iter().find(|(_, id)| id == r.text("to"))?;
            let artifact = log.iter().find(|a| a.id == r.text("from") && a.kind == "artifact")?;
            Some((artifact.id.clone(), spec.0.clone()))
        })
        .collect()
}

/// Whether agent-0 wrote `e`, as agent-0 tells.
fn is_mine(turn: &Turn, e: &Entry) -> bool {
    turn.roster.name(&e.author) == Some("agent-0")
}

/// Whether agent-0 wrote `id`, as the harness tells.
fn is_by_agent_0(run: &Run, id: &str) -> bool {
    run.origin.get(id).is_some_and(|o| o.agent == 0)
}

#[test]
fn an_adversary_cannot_retire_others_work_by_proposing_fixes() {
    // For every implementation by others, agent-0 records a made-up failing check and then
    // proposes "fixes", by turns: (0) a correct program for another spec; (1) a copy of
    // the target's own program; (2) every round until the target is retired, a new
    // artifact with one of the spec's buggy programs; (3) a program that's always wrong;
    // (4) a correct program for the spec that differs from the target's.
    let mut attacked: Vec<(String, usize)> = Vec::new();
    let mut written = 0;
    let adversary = move |turn: &Turn| -> Result<()> {
        let r = turn.replica;
        let log = r.log()?;
        let specs = spec_ids(&log);
        let retired =
            |id: &str| log.iter().any(|e| e.kind == "relation" && e.text("kind") == "supersedes" && e.text("to") == id);
        for (target, spec) in others_impls(&log) {
            let author_is_me = log.iter().any(|e| e.id == target && is_mine(turn, e));
            let mode = match attacked.iter().find(|(t, _)| *t == target) {
                _ if author_is_me => continue,
                Some(&(_, 2)) if !retired(&target) => 2,
                Some(_) => continue,
                None => {
                    r.evidence(&target, false, "bracken-demo check", "made up")?;
                    let mode = attacked.len() % 5;
                    attacked.push((target.clone(), mode));
                    mode
                }
            };
            let s = task::spec(&spec).expect("a known spec");
            let program = log.iter().find(|e| e.id == target).expect("the target").text("text").to_owned();
            let (fix_spec, program) = match mode {
                0 => {
                    let other = task::SPECS.iter().find(|o| o.name != spec).expect("several specs");
                    (other.name, other.correct[0].to_owned())
                }
                1 => (s.name, program),
                2 => (s.name, s.buggy[written % s.buggy.len()].to_owned()),
                3 => (s.name, "0".to_owned()),
                _ => (s.name, s.correct.iter().find(|c| **c != program).expect("two correct programs").to_string()),
            };
            written += 1;
            let name = format!("impl/{fix_spec}/agent-0/{written}");
            let fix = r.artifact(&format!("fix-{written}.rpn"), &name, MEDIA_TYPE, &program)?;
            let fix_spec_id = &specs.iter().find(|(name, _)| name == fix_spec).expect("spec claims").1;
            r.relate(&fix, "implements", fix_spec_id, "started in round 1")?;
            r.relate(&fix, "supersedes", &target, "fixes it")?;
        }
        Ok(())
    };
    let (run, _) = run(&config(4, 8, 1, Behaviour::default()), adversary);

    let by_id = |id: &str| run.log.iter().find(|e| e.id == id).expect("in the log");
    let proposals = run.log.iter().filter(|e| e.kind == "relation" && e.text("kind") == "supersedes");
    let proposals = proposals.filter(|e| is_by_agent_0(&run, &e.id)).count();
    assert!(proposals >= 20, "agent-0 proposed {proposals} fixes");
    // An honest author may accept a buggy fix of its own buggy work; never one of correct
    // work, a copy, or another spec's program.
    let spec_of = |artifact: &str| {
        let specs = spec_ids(&run.log);
        run.log
            .iter()
            .filter(|e| e.kind == "relation" && e.text("kind") == "implements" && e.text("from") == artifact)
            .find_map(|e| specs.iter().find(|(_, id)| id == e.text("to")).map(|(name, _)| name.clone()))
            .expect("implements a spec")
    };
    let bad: Vec<String> = run
        .log
        .iter()
        .filter(|e| e.kind == "relation" && e.text("kind") == "supersedes" && !is_by_agent_0(&run, &e.id))
        .filter(|e| is_by_agent_0(&run, e.text("from")))
        .filter_map(|e| {
            let (fix, old) = (by_id(e.text("from")), by_id(e.text("to")));
            let spec = task::spec(&spec_of(&old.id)).expect("a known spec");
            let why = if spec_of(&fix.id) != spec.name {
                "another spec"
            } else if fix.text("text") == old.text("text") {
                "a copy"
            } else if spec.is_correct(old.text("text")) {
                "correct work"
            } else {
                return None;
            };
            Some(format!("{} replaced {} ({why})", fix.text("name"), old.text("name")))
        })
        .collect();
    assert!(bad.is_empty(), "honest agents accepted: {bad:?}");
}

#[test]
fn an_adversary_cannot_hide_others_fresh_wip_claims() {
    // Each round, for every other open wip claim, agent-0 writes a wip claim of its own on
    // the same spec, then retracts it. With `refines`, the claim refines the other one,
    // shaped exactly like an honest takeover. A takeover must not hide a claim, so the
    // honest agents must start the same work, in the same rounds, either way.
    let adversary = |refines: bool| {
        move |turn: &Turn| -> Result<()> {
            let r = turn.replica;
            let log = r.log()?;
            let open: Vec<(&Entry, String)> = log
                .iter()
                .filter(|e| e.kind == "claim" && !e.retracted && !is_mine(turn, e))
                .filter_map(|e| match e.tags()[..] {
                    ["wip", spec] => Some((e, spec.to_owned())),
                    _ => None,
                })
                .collect();
            for (w, spec) in open {
                let statement = format!("Work in progress: agent-0 is implementing {spec} (round {})", turn.round);
                let mine = r.claim(&statement, &["wip", &spec])?;
                if refines {
                    r.relate(&mine, "refines", &w.id, "takes over stale work")?;
                }
                r.retract(&mine, &format!("delivered in round {}", turn.round))?;
            }
            Ok(())
        }
    };
    let cfg = config(5, 12, 1, Behaviour::new(4, true, Schedule::Stagger));
    // The honest agents' starts: their wip claims, by author, round and spec.
    let starts = |refines: bool| {
        let (run, _) = run(&cfg, adversary(refines));
        let hidden = run.log.iter().filter(|e| e.kind == "relation" && e.text("kind") == "refines");
        let hidden = hidden.filter(|e| is_by_agent_0(&run, &e.id)).count();
        let mut starts: Vec<(usize, u32, String)> = run
            .log
            .iter()
            .filter(|e| e.kind == "claim" && e.tags().first() == Some(&"wip") && !is_by_agent_0(&run, &e.id))
            .map(|e| (run.origin[&e.id].agent, run.origin[&e.id].round, e.tags().join(" ")))
            .collect();
        starts.sort();
        (hidden, starts)
    };
    let (hidden, with) = starts(true);
    assert!(hidden >= 10, "agent-0 took over {hidden} claims");
    let (_, without) = starts(false);
    assert!(!with.is_empty());
    assert_eq!(with, without, "agent-0's takeovers changed what the honest agents started");
}

#[test]
fn crafted_rounds_in_wip_claims_count_as_the_round_they_arrive() {
    // In round 1 agent-0 claims every spec as work in progress, never to deliver it,
    // stating a round. Claims reach the others in round 2, so a crafted round, however
    // large, must count as round 2: the honest agents do exactly what they do when the
    // claims state round 2.
    let claims = |stated: u32| {
        move |turn: &Turn| -> Result<()> {
            if turn.round == 1 {
                for s in task::SPECS {
                    let statement = format!("Work in progress: agent-0 is implementing {} (round {stated})", s.name);
                    turn.replica.claim(&statement, &["wip", s.name])?;
                }
            }
            Ok(())
        }
    };
    for rule in [StaleRule::ClaimRound, StaleRule::Liveness] {
        let cfg = config(4, 10, 1, Behaviour::new(2, true, Schedule::Lockstep).with_rule(rule));
        let honest = |stated: u32| {
            let (run, report) = run(&cfg, claims(stated));
            let mut work: Vec<(usize, u32, String, String)> = run
                .log
                .iter()
                .filter(|e| !is_by_agent_0(&run, &e.id) && run.origin.contains_key(&e.id))
                .map(|e| {
                    let o = run.origin[&e.id];
                    (o.agent, o.round, e.kind.clone(), format!("{} {}", e.text("statement"), e.text("name")))
                })
                .collect();
            work.sort();
            (work, report.wip.taken_over)
        };
        let (expected, taken_over) = honest(2);
        assert!(taken_over > 0, "{rule:?}: the honest agents take over agent-0's stale claims");
        for stated in [1_000_000, u32::MAX] {
            assert_eq!(honest(stated), (expected.clone(), taken_over), "{rule:?}, stated round {stated}");
        }
    }
}

/// Claims every spec in round 1 and never works on any; says it dropped them all
/// silently, but only after the run.
#[derive(Default)]
struct LateDropper(Vec<String>);

impl Agent for LateDropper {
    fn act(&mut self, turn: &Turn) -> Result<()> {
        if turn.round == 1 {
            for s in task::SPECS {
                let statement = format!("Work in progress: agent-0 is implementing {} (round 1)", s.name);
                self.0.push(turn.replica.claim(&statement, &["wip", s.name])?);
            }
        }
        Ok(())
    }

    fn silent_drops(&self) -> Vec<(String, u32)> {
        self.0.iter().map(|c| (c.clone(), 11)).collect()
    }
}

#[test]
fn a_takeover_before_a_silent_drop_does_not_count_as_finding_it() {
    // The claims go stale and are taken over during the run, while their work was (as far
    // as anyone knows) going on; the drop comes later, so no takeover found it.
    // The honest agents drop nothing, so every silent drop is agent-0's.
    let behaviour = Behaviour { drop_rate: 0.0, ..Behaviour::new(2, true, Schedule::Lockstep) };
    let (_, report) = run_with(&config(4, 10, 1, behaviour), Box::new(LateDropper::default()));
    let w = &report.wip;
    assert!(w.taken_over > 0, "the honest agents take over agent-0's claims");
    assert_eq!(w.silent_drops.total, task::SPECS.len());
    assert_eq!((w.silent_drops.taken_over, w.silent_drops.missed), (0, task::SPECS.len()));
    assert_eq!(w.silent_drops.mean_delay_rounds, 0.0);
}
