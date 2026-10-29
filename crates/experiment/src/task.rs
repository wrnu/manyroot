//! The task as agents see it: the language, the specs in their modules, the
//! open contracts, the change requests, the public cases and the public check.
//!
//! There are two tasks, and a run's plan names which one it ran ([`Task`]):
//! Phase 1's `stacklib` (`task/phase1.rs`, kept as it ran so its results stay
//! reproducible) and Phase 2's larger one (`task/phase2.rs`, design
//! `docs/design/phase2-task.md`, Candidate A with Candidate C's open contracts).
//!
//! Everything here may reach an agent. The hidden cases and the scores are in
//! [`crate::grader`], and nothing in this module reads them.

use bracken_demo::rng::Rng;
use serde::{Deserialize, Serialize};

use crate::lang::{Calls, Library, run};

mod phase1;
mod phase2;

/// Which task a run gives its agents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    /// Phase 1's `stacklib`: 30 functions in four modules. A plan from before the selector named none, so
    /// it reads as this one.
    #[default]
    Phase1,
    /// Phase 2's: about 90 functions in eight modules and five layers, with open contracts.
    Phase2,
}

/// One function to implement.
pub struct Spec {
    pub name: &'static str,
    pub module: &'static str,
    /// 0 needs only the language; layer n calls functions of layers below it.
    pub layer: u8,
    pub arity: u8,
    pub description: &'static str,
}

const fn spec(name: &'static str, module: &'static str, layer: u8, arity: u8, description: &'static str) -> Spec {
    Spec { name, module, layer, arity, description }
}

/// An open contract: functions whose results the task leaves to the library, as long as they agree with
/// each other, such as how a value says that one number is less than another. No reference implementation
/// exists for its members, so none is graded alone: each is right when every property that calls it holds,
/// whatever convention the library chose. A property holds for every consistent choice, so no single
/// answer can leak.
pub struct Contract {
    pub name: &'static str,
    pub members: &'static [&'static str],
    pub properties: &'static [Property],
}

/// The library's functions, as a property calls them: by name, with one value per input.
pub type Caller<'a> = dyn FnMut(&str, &[i64]) -> Result<i64, String> + 'a;

/// One relation between an open contract's members, on inputs drawn from [`CONTRACT_RANGE`]: a round trip,
/// or a producer against a consumer.
pub struct Property {
    /// Inputs per case.
    pub arity: u8,
    /// The members it calls. A member is graded by every property that calls it.
    pub calls: &'static [&'static str],
    /// The property on one case.
    pub eval: fn(&mut Caller<'_>, &[i64]) -> Probe,
}

/// A property on one case: the expression, what it gave and what it must give.
pub struct Probe {
    pub expr: String,
    pub got: Result<i64, String>,
    pub want: i64,
}

/// An open contract's inputs are drawn from -CONTRACT_RANGE..=CONTRACT_RANGE, as its specs state.
pub const CONTRACT_RANGE: i64 = 1000;

/// A change request: the experiment releases all of them at once, partway through a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    ClampOrder,
    SignOfZero,
    NearerTies,
    BetweenExclusive,
    AbsSaturates,
    SpreadCounts,
}

pub struct ChangeRequest {
    pub change: Change,
    /// The spec whose contract changes.
    pub target: &'static str,
    /// The spec's description once the change applies.
    pub description: &'static str,
    /// The request as agents read it.
    pub text: &'static str,
}

/// Which change requests apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32);

impl Version {
    /// Before any change request.
    pub const ORIGINAL: Version = Version(0);

    pub fn with(self, change: Change) -> Version {
        Version(self.0 | (1 << change as u32))
    }

    pub fn has(self, change: Change) -> bool {
        self.0 & (1 << change as u32) != 0
    }
}

impl Task {
    pub const ALL: [Task; 2] = [Task::Phase1, Task::Phase2];

    /// The name a plan and the command line use.
    pub fn name(self) -> &'static str {
        match self {
            Task::Phase1 => "phase1",
            Task::Phase2 => "phase2",
        }
    }

    pub fn named(name: &str) -> Option<Task> {
        Task::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The language, as the task states it to agents.
    pub fn language(self) -> &'static str {
        match self {
            Task::Phase1 => phase1::LANGUAGE,
            Task::Phase2 => phase2::LANGUAGE,
        }
    }

    /// The library's modules, in the order the task lists them.
    pub fn modules(self) -> &'static [&'static str] {
        match self {
            Task::Phase1 => phase1::MODULES,
            Task::Phase2 => phase2::MODULES,
        }
    }

    /// The file each module is in, in a mode that keeps modules as files: `<module>.stk`.
    pub fn module_files(self) -> &'static [&'static str] {
        match self {
            Task::Phase1 => &["base.stk", "order.stk", "arith.stk", "shape.stk"],
            Task::Phase2 => phase2::MODULE_FILES,
        }
    }

    pub fn specs(self) -> &'static [Spec] {
        match self {
            Task::Phase1 => phase1::SPECS,
            Task::Phase2 => phase2::SPECS,
        }
    }

    pub fn changes(self) -> &'static [ChangeRequest] {
        match self {
            Task::Phase1 => phase1::CHANGES,
            Task::Phase2 => &[],
        }
    }

    pub fn contracts(self) -> &'static [Contract] {
        match self {
            Task::Phase1 => &[],
            Task::Phase2 => phase2::CONTRACTS,
        }
    }

    /// The spec named `name`.
    pub fn spec_named(self, name: &str) -> Option<&'static Spec> {
        self.specs().iter().find(|s| s.name == name)
    }

    /// The open contract `name` is a member of, if any.
    pub fn contract_of(self, name: &str) -> Option<&'static Contract> {
        self.contracts().iter().find(|c| c.members.contains(&name))
    }

    /// The specs `name`'s contract is defined in terms of: the ones its
    /// reference calls, so it follows their change requests. Each is in the same
    /// layer or a lower one, and there are no cycles.
    pub fn defined_by(self, name: &str) -> &'static [&'static str] {
        match self {
            Task::Phase1 => phase1::defined_by(name),
            Task::Phase2 => phase2::defined_by(name),
        }
    }

    /// After every change request: the contracts the experiment grades the end of a run against.
    pub fn all(self) -> Version {
        self.changes().iter().fold(Version::ORIGINAL, |v, c| v.with(c.change))
    }

    /// The change requests that apply under `version`, in the order the task lists them.
    pub fn changes_in(self, version: Version) -> impl Iterator<Item = &'static ChangeRequest> {
        self.changes().iter().filter(move |c| version.has(c.change))
    }

    /// The description of `spec` that holds under `version`.
    pub fn description_at(self, spec: &Spec, version: Version) -> &'static str {
        self.changes_in(version).filter(|c| c.target == spec.name).last().map_or(spec.description, |c| c.description)
    }

    /// The reference implementation of `name` under `version`, on one input per
    /// argument. A spec defined in terms of other functions calls their
    /// references, so it follows their change requests. Panics on a name that
    /// isn't a spec with a reference (an open contract's members have none) or
    /// on the wrong number of inputs: callers resolve those first.
    pub fn reference(self, name: &str, args: &[i64], version: Version) -> i64 {
        let r = |callee: &str, args: &[i64]| {
            assert!(self.defined_by(name).contains(&callee), "{name}'s reference calls {callee}, outside defined_by");
            self.reference(callee, args, version)
        };
        let value = match self {
            Task::Phase1 => phase1::reference(name, args, version, &r),
            Task::Phase2 => phase2::reference(name, args, version, &r),
        };
        value.unwrap_or_else(|| panic!("no reference for {name} with {} inputs", args.len()))
    }

    /// The properties that grade `name`, an open contract's member, each with its index among all the
    /// task's properties (which seeds its cases).
    pub fn properties_of(self, name: &str) -> Vec<(usize, &'static Property)> {
        self.properties().filter(|(_, p)| p.calls.contains(&name)).collect()
    }

    /// Every property of every open contract, with its index.
    pub fn properties(self) -> impl Iterator<Item = (usize, &'static Property)> {
        self.contracts().iter().flat_map(|c| c.properties).enumerate()
    }

    /// The public cases of `spec`: the only inputs an agent's check ever runs.
    pub fn public_cases(self, spec: &Spec) -> Vec<Case> {
        let index = self.specs().iter().position(|s| s.name == spec.name).unwrap_or(self.specs().len());
        public_grid(spec.arity, index as u64)
    }

    /// The public cases of the property with index `index`.
    pub fn property_cases(self, index: usize, property: &Property) -> Vec<Case> {
        public_grid(property.arity, (self.specs().len() + index) as u64)
    }

    /// The agents' check: runs `name`'s public cases in `library` (an agent's own
    /// working copy), with every call resolved in that library, against the specs
    /// under `version`. An open contract's member runs the public cases of every
    /// property that calls it. Errs when there's nothing to run.
    pub fn check(self, library: &Library, name: &str, version: Version) -> Result<Check, String> {
        let spec = self.spec_named(name).ok_or_else(|| format!("`{name}` is not a function of the task"))?;
        let def = library.get(name).ok_or_else(|| library.why_missing(name))?;
        if def.arity != spec.arity {
            return Err(format!("`{name}` must take {} inputs, not {}", spec.arity, def.arity));
        }
        let mut passed = 0;
        let mut total = 0;
        let mut first_failure = None;
        if self.contract_of(name).is_some() {
            for (index, property) in self.properties_of(name) {
                for case in self.property_cases(index, property) {
                    let probe = self.probe(property, &case, library, library);
                    total += 1;
                    match probe.failure() {
                        None => passed += 1,
                        Some(f) => {
                            first_failure.get_or_insert(f);
                        }
                    }
                }
            }
            return Ok(Check { passed, total, first_failure });
        }
        for case in &self.public_cases(spec) {
            let args = &case[..usize::from(spec.arity)];
            let want = self.reference(name, args, version);
            total += 1;
            match run(def, args, library) {
                Ok(got) if got == want => passed += 1,
                outcome => {
                    let got = match outcome {
                        Ok(got) => format!("gave {got}"),
                        Err(e) => format!("failed: {e}"),
                    };
                    first_failure.get_or_insert(format!("{} {got}, expected {want}", call_text(spec, case)));
                }
            }
        }
        Ok(Check { passed, total, first_failure })
    }

    /// `property` on `case`, each member's definition taken from `library` and its calls resolved by `calls`.
    pub fn probe(self, property: &Property, case: &Case, library: &Library, calls: &dyn Calls) -> Probe {
        let mut call = |name: &str, args: &[i64]| -> Result<i64, String> {
            let def = library.get(name).ok_or_else(|| library.why_missing(name))?;
            match self.spec_named(name) {
                Some(spec) if spec.arity != def.arity => {
                    Err(format!("`{name}` must take {} inputs, not {}", spec.arity, def.arity))
                }
                _ => run(def, args, calls),
            }
        };
        (property.eval)(&mut call, &case[..usize::from(property.arity)])
    }

    /// The task as agents read it under `version`: the language, the modules and
    /// their specs, and the change requests that apply.
    pub fn task_text(self, version: Version) -> String {
        let mut text = format!("Write this library. {}\n", self.language());
        for module in self.modules() {
            text.push_str(&format!("\nModule {module}:\n"));
            for s in self.specs().iter().filter(|s| s.module == *module) {
                text.push_str(&format!("- {}\n", self.description_at(s, version)));
            }
        }
        let changes: Vec<&ChangeRequest> = self.changes_in(version).collect();
        if !changes.is_empty() {
            text.push_str("\nChange requests (the descriptions above already include them):\n");
            for c in changes {
                text.push_str(&format!("- {}\n", c.text));
            }
        }
        text
    }

    /// The version a `TASKS.md` states on its released line.
    pub fn version_of(self, tasks: &str) -> Version {
        let released: Vec<&str> =
            tasks.lines().filter_map(|l| l.strip_prefix(RELEASED)).flat_map(|l| l.split(',').map(str::trim)).collect();
        self.changes()
            .iter()
            .filter(|c| released.contains(&change_tag(c.change)))
            .fold(Version::ORIGINAL, |v, c| v.with(c.change))
    }

    /// `TASKS.md` at `version`: the task text, and the released line.
    pub fn tasks_md(self, version: Version) -> String {
        let tags: Vec<&str> = self.changes_in(version).map(|c| change_tag(c.change)).collect();
        format!("{}\n\n{RELEASED} {}\n", self.task_text(version), tags.join(", "))
    }
}

impl Probe {
    /// What went wrong, as the check reports it, if anything.
    pub fn failure(&self) -> Option<String> {
        match &self.got {
            Ok(got) if *got == self.want => None,
            Ok(got) => Some(format!("{} gave {got}, expected {}", self.expr, self.want)),
            Err(e) => Some(format!("{} failed: {e}, expected {}", self.expr, self.want)),
        }
    }
}

/// Inputs per public case set, for functions of 2 or 3 inputs. A function of
/// one input gets all 13 inputs from -6 to 6.
pub const PUBLIC_CASES: usize = 60;
/// Public inputs are drawn from -PUBLIC_RANGE..=PUBLIC_RANGE.
pub const PUBLIC_RANGE: i64 = 6;
/// Public cases are the same in every run; they are no secret.
const PUBLIC_SEED: u64 = 0x5747_4c49_4250_5542;

/// One set of inputs, `a`, `b` and `c`; a function uses the first `arity`.
pub type Case = [i64; 3];

/// [`PUBLIC_CASES`] inputs of `arity` from the public range, shuffled by `stream`.
fn public_grid(arity: u8, stream: u64) -> Vec<Case> {
    let range = -PUBLIC_RANGE..=PUBLIC_RANGE;
    let mut grid: Vec<Case> = match arity {
        1 => range.map(|a| [a, 0, 0]).collect(),
        2 => range.clone().flat_map(|a| range.clone().map(move |b| [a, b, 0])).collect(),
        _ => range
            .clone()
            .flat_map(|a| range.clone().flat_map(move |b| (-PUBLIC_RANGE..=PUBLIC_RANGE).map(move |c| [a, b, c])))
            .collect(),
    };
    Rng::derive(PUBLIC_SEED, stream).shuffle(&mut grid);
    grid.truncate(PUBLIC_CASES);
    grid
}

/// The call `spec(case)` as text, for messages.
pub fn call_text(spec: &Spec, case: &Case) -> String {
    let args: Vec<String> = case[..usize::from(spec.arity)].iter().map(i64::to_string).collect();
    format!("{}({})", spec.name, args.join(", "))
}

/// The outcome of a public check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub passed: usize,
    pub total: usize,
    pub first_failure: Option<String>,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.first_failure.is_none()
    }

    /// One line for a tool result or evidence details.
    pub fn details(&self) -> String {
        match &self.first_failure {
            None => format!("{}/{} cases passed", self.passed, self.total),
            Some(f) => format!("{f} ({}/{} cases passed)", self.passed, self.total),
        }
    }
}

/// A change request's tag: on the setter's claim in Bracken mode, and on `TASKS.md`'s released line.
pub fn change_tag(change: Change) -> &'static str {
    match change {
        Change::ClampOrder => "clamp-order",
        Change::SignOfZero => "sign-of-zero",
        Change::NearerTies => "nearer-ties",
        Change::BetweenExclusive => "between-exclusive",
        Change::AbsSaturates => "abs-saturates",
        Change::SpreadCounts => "spread-counts",
    }
}

/// The line of `TASKS.md` that lists the change requests released so far.
pub const RELEASED: &str = "Change requests released:";
