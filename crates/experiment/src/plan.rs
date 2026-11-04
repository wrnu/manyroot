//! The pre-registered plan and the report (design section 4, "The analysis is
//! fixed before the run"). The driver writes `plan.json` into the run
//! directory before any agent's first turn and logs its hash in the setup
//! event; the plan is contributed to the team's Ground as an artifact before
//! the first model call (step 9). The report recomputes the hash, refuses a
//! plan that fixed other metric code than this build's, and quotes the hash.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::TURN_MESSAGE;
use crate::bracken_mode::AcceptRule;
use crate::driver::{RELEASE_AT, RunConfig, core_tools};
use crate::grader::HiddenCases;
use crate::metrics::{DEFINITIONS, Measures, Records, incomplete, measure};
use crate::mode::Mode;
use crate::task::{Task, Version};

/// The rules each mode integrates by, as fixed before the run, besides Bracken mode's
/// acceptance rule, which the run chooses ([`rules`]).
pub const RULES: &[(&str, &str)] = &[
    (
        "bracken-check",
        "Answer 88ae128c5fcd: check resolves calls in the agent's replica: the artifact under check for its own \
         spec; for every other spec the fixed rule's pick, else the agent's own latest current implementation, \
         else the latest current one by anyone.",
    ),
    (
        "git-merge",
        "Decision 6b13e672e7d4 points 1 to 4: CI passes on the merge result (modules parse, no function passing \
         on main regresses, every changed function passes, against the hub's released version); an approval of \
         the current head by someone other than the author; no reviewer's latest review requesting changes; any \
         agent merges, --no-ff, made by the hub, and a conflicting merge is refused.",
    ),
];

/// Every rule a run integrates by: Bracken mode's acceptance rule as the run chose it, then [`RULES`].
pub fn rules(accept: AcceptRule) -> Vec<(String, String)> {
    let mut out = vec![("bracken-materialization".to_string(), accept.text().to_string())];
    out.extend(RULES.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    out
}

/// What the caller chooses for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanInputs {
    /// The task agents are given. A plan from before there were two names none: Phase 1's.
    #[serde(default)]
    pub task: Task,
    pub model: String,
    pub effort: String,
    /// The hidden cases' seed: never shown to an agent.
    pub grader_seed: u64,
    pub backend_seed: u64,
    pub harness_commit: String,
    /// The backend's caps (step 6), as it states them.
    pub caps: Value,
    /// blake3 of the system prompt agents are given, the base and the mode's section; empty for the mock.
    pub system_prompt: String,
    /// The backend's usage rows: relative to the run directory, or absolute.
    pub usage: String,
    /// The stops the run may end with and still be measured: the backend's own caps.
    pub expected_stops: Vec<String>,
    /// Bracken mode's rule for which implementation counts.
    #[serde(default = "AcceptRule::before_selectable")]
    pub accept_rule: AcceptRule,
    /// The agent harness, as its backend's record says it: name, pin, profile, declared entries and
    /// what it says of itself, such as where a turn's result comes from. Null for the mock and for
    /// plans from before it was recorded.
    #[serde(default)]
    pub harness: Value,
    /// The pinned Python image, for a run of an evaluation project (`docs/design/eval-suite.md`): every
    /// public-test run and grade pins it from here ([`crate::python::Python::from_plan`]). None otherwise.
    #[serde(default)]
    pub python: Option<crate::python::PythonPin>,
    /// The evaluation project a Weave-mode run gives its agents in place of the task. None otherwise, and
    /// then not written, so earlier plans read and hash as they did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectPlan>,
    /// The configuration's integrator setting (answer `07959d6d9bc0`): who may integrate onto the line, as
    /// [`crate::weave_mode::ANY_AGENT`]. None outside Weave mode, and then not written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrator: Option<String>,
    /// A project run that holds its change back (`--hold-change`, decision `de365aae05fd`): only the items
    /// released at the start go out. Not written when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hold_change: bool,
}

/// A project as a plan records it: where it is, a digest of all of it, and what agents are given of it. No
/// path, test name or count of its hidden set: only the reference project.json already holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectPlan {
    /// The project's directory, absolute.
    pub dir: String,
    pub name: String,
    /// blake3 over every entry of the directory, as a hidden set's digest is made: the run refuses a project
    /// changed since its plan.
    pub digest: String,
    /// The Python minor version it is written for.
    pub python: String,
    /// Each item, with the share of the run's agent-turns used before it is released (0 at the start).
    pub items: Vec<(String, f64)>,
    pub hidden: crate::project::HiddenRef,
    /// blake3 of each text agents are given: the spec and each item's issue.
    pub texts: BTreeMap<String, String>,
}

impl ProjectPlan {
    pub fn of(project: &crate::project::Project) -> Result<ProjectPlan, String> {
        crate::weave_mode::check_task_outputs(project)?;
        let dir = std::fs::canonicalize(&project.dir).map_err(|e| format!("resolving the project: {e}"))?;
        let read = |p: &Path| std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()));
        let mut texts = BTreeMap::from([("spec".to_string(), hash(&read(&project.spec)?))]);
        for item in &project.items {
            texts.insert(format!("item-{}", item.id), hash(&read(&item.text)?));
        }
        let share = |r: crate::project::Release| match r {
            crate::project::Release::Start => 0.0,
            crate::project::Release::AfterShare(at) => at,
        };
        Ok(ProjectPlan {
            dir: dir.display().to_string(),
            name: project.name.clone(),
            digest: crate::project::digest(&dir)?,
            python: project.python.clone(),
            items: project.items.iter().map(|i| (i.id.clone(), share(i.release))).collect(),
            hidden: project.hidden.clone(),
            texts,
        })
    }

    /// The project, loaded again, refused if it changed since the plan.
    pub fn load(&self) -> Result<crate::project::Project, String> {
        let project = crate::project::Project::load(Path::new(&self.dir))?;
        if crate::project::digest(Path::new(&self.dir))? != self.digest {
            return Err(format!("the project at {} changed since the plan", self.dir));
        }
        Ok(project)
    }
}

impl Default for PlanInputs {
    fn default() -> PlanInputs {
        PlanInputs {
            task: Task::Phase1,
            model: "mock".into(),
            effort: "none".into(),
            grader_seed: 1,
            backend_seed: 0,
            harness_commit: "unknown".into(),
            caps: Value::Null,
            system_prompt: String::new(),
            usage: "usage.jsonl".into(),
            expected_stops: Vec::new(),
            accept_rule: AcceptRule::default(),
            harness: Value::Null,
            python: None,
            project: None,
            integrator: None,
            hold_change: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub inputs: PlanInputs,
    pub mode: String,
    pub agents: usize,
    pub turns: u32,
    pub calls_per_turn: u32,
    pub turn_limit_seconds: u64,
    /// Phase 1's one release, as a share of all agent-turns.
    pub release_at: f64,
    /// Phase 2's: the turn after which each wave of change requests goes out. Phase 1's plans have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waves: Vec<u32>,
    /// A project run with a change: the agent-turns used before it goes out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_after: Option<u32>,
    /// `git --version` of the binary the workers run.
    pub git: String,
    /// blake3 of the code that computes the measures and grades.
    pub metrics_code: String,
    /// blake3 of each text agents are given.
    pub prompts: BTreeMap<String, String>,
    pub rules: Vec<(String, String)>,
    pub definitions: Vec<(String, String)>,
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The hash of this build's measuring code: the measures, the grader, the task and language they
/// judge answers by (finding `6d14a240c355`), and the generator that draws the hidden cases.
pub fn metrics_code() -> String {
    let code = [
        include_str!("metrics.rs"),
        include_str!("grader.rs"),
        include_str!("task.rs"),
        include_str!("task/phase1.rs"),
        include_str!("task/phase2.rs"),
        include_str!("lang.rs"),
        // The hidden cases' draws.
        include_str!("../../demo/src/rng.rs"),
    ];
    hash(code.concat().as_bytes())
}

impl Plan {
    pub fn new(cfg: &RunConfig, mode: &dyn Mode) -> Plan {
        let git = std::process::Command::new(&cfg.git)
            .arg("--version")
            .env_clear()
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let task = cfg.plan.task;
        let mut tools = mode.tools(task);
        tools.extend(core_tools());
        let tools = serde_json::to_vec(&tools).expect("tool specs are plain JSON");
        let mut prompts = BTreeMap::from([
            ("tools".to_string(), hash(&tools)),
            ("turn-message".to_string(), hash(TURN_MESSAGE.as_bytes())),
        ]);
        let mut change_after = None;
        if let Some(project) = &cfg.plan.project {
            // A project's texts take the task's place: those agents are given, so a held change's not.
            let held = |key: &String| {
                cfg.plan.hold_change
                    && project.items.iter().any(|(id, share)| *share > 0.0 && *key == format!("item-{id}"))
            };
            prompts.extend(project.texts.iter().filter(|(k, _)| !held(k)).map(|(k, v)| (k.clone(), v.clone())));
            let shares: Vec<f64> = project.items.iter().map(|(_, s)| *s).collect();
            change_after = crate::weave_mode::change_share(&shares)
                .ok()
                .flatten()
                .filter(|_| !cfg.plan.hold_change)
                .map(|s| crate::weave_mode::release_after(s, cfg.turns, cfg.agents));
        } else {
            prompts.insert("task-original".into(), hash(task.task_text(Version::ORIGINAL).as_bytes()));
            prompts.insert("task-changed".into(), hash(task.task_text(task.all()).as_bytes()));
            // Between the first wave and the last, the task reads as each wave left it.
            for wave in 1..task.waves().len() {
                prompts
                    .insert(format!("task-after-wave-{wave}"), hash(task.task_text(task.after_wave(wave)).as_bytes()));
            }
        }
        let pairs = |p: &[(&str, &str)]| p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Plan {
            inputs: cfg.plan.clone(),
            mode: mode.name().into(),
            agents: cfg.agents,
            turns: cfg.turns,
            calls_per_turn: cfg.calls_per_turn,
            turn_limit_seconds: cfg.turn_limit.as_secs(),
            release_at: RELEASE_AT,
            waves: if cfg.plan.project.is_some() { Vec::new() } else { task.wave_turns(cfg.turns) },
            change_after,
            git,
            metrics_code: metrics_code(),
            prompts,
            // A project run integrates by its integrator setting alone, and has no measures yet: the task's
            // rules and definitions aren't its own.
            rules: match (&cfg.plan.project, &cfg.plan.integrator) {
                (Some(_), integrator) => vec![(
                    "weave-integrator".to_string(),
                    format!(
                        "Answer 07959d6d9bc0: {}: every agent of the run is an integrator of main, and merges proposals \
                         onto its head, or records a tree with every head as a parent.",
                        integrator.as_deref().unwrap_or(crate::weave_mode::ANY_AGENT)
                    ),
                )],
                (None, _) => rules(cfg.plan.accept_rule),
            },
            definitions: if cfg.plan.project.is_some() { Vec::new() } else { pairs(DEFINITIONS) },
        }
    }

    /// The bytes written to `plan.json`, whose blake3 is the plan's hash.
    pub fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("a plan is plain JSON")
    }
}

/// One run's report: its plan's hash and its measures.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub dir: String,
    pub plan: String,
    pub measures: Measures,
}

/// The report of the run in `dir`, with usage from `usage` (else where the plan says), which must exist.
pub fn report(dir: &Path, usage: Option<&Path>) -> Result<Report, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let plan_bytes = read(&dir.join("plan.json"))?;
    let plan: Plan = serde_json::from_slice(&plan_bytes).map_err(|e| format!("the plan: {e}"))?;
    if plan.inputs.project.is_some() {
        return Err(format!(
            "{}: a project run has no measures yet; its metrics come after the first traces",
            dir.display()
        ));
    }
    let plan_hash = hash(&plan_bytes);
    if plan.metrics_code != metrics_code() {
        return Err(format!(
            "the plan fixed metric code {}, but this build's is {}: report with the planned build, or write a new plan",
            plan.metrics_code,
            metrics_code()
        ));
    }
    let events = String::from_utf8(read(&dir.join("events.jsonl"))?).map_err(|e| e.to_string())?;
    // No usage is no token count, so no measure per token: an error, not a zero.
    let usage_path = usage.map_or_else(|| dir.join(&plan.inputs.usage), Path::to_path_buf);
    let usage = std::fs::read_to_string(&usage_path).map_err(|e| format!("reading {}: {e}", usage_path.display()))?;
    let records = Records::parse(&events, &usage)?;
    if let Some(why) = incomplete(&records, &plan.inputs.expected_stops, plan.inputs.task) {
        return Err(format!("{}: not measured: {why}", dir.display()));
    }
    let logged = records.events.iter().find(|e| e.event == "setup").and_then(|e| e.data["plan"].as_str());
    if logged != Some(plan_hash.as_str()) {
        return Err("plan.json isn't the plan the run logged at setup".into());
    }
    let measures = measure(&records, &HiddenCases::new(plan.inputs.task, plan.inputs.grader_seed))?;
    Ok(Report { dir: dir.display().to_string(), plan: plan_hash, measures })
}
