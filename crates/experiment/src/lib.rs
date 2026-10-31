//! The Phase 1 experiment: the same task under three collaboration modes (Git
//! branches and pull requests, CRDT shared editing, Bracken contributions),
//! measured the same way.
//!
//! Build step 1 is `stacklib`, the task (section 1 of
//! `docs/design/phase1-experiment.md`); step 2 is the core (the [`driver`],
//! its [`worker`]s, the [`shim`] and the [`mock`] backend) and Bracken mode
//! ([`bracken_mode`]); step 3 is Git mode ([`git_mode`]); step 4 is CRDT mode
//! ([`crdt_mode`]), on Automerge; all behind the same [`mode`] interface. Step 6
//! is the agent [`backend`], with its forwarding [`proxy`], and Claude Code as its
//! first harness ([`claude`]), with its [`sandbox`] profiles.
//! Step 5 measures a run: the [`plan`] fixed before it, and the six [`metrics`]
//! over its records.
//!
//! A run chooses its task ([`task::Task`]): Phase 1's, or Phase 2's larger
//! one with open contracts (`docs/design/phase2-task.md`).
//!
//! What an agent may see goes through [`task`] only: the language, the specs,
//! the change requests, the public cases and the public check. [`grader`]
//! holds the hidden cases and the scores, which no agent-facing tool may
//! return.

pub mod args;
pub mod backend;
pub mod bracken_mode;
pub mod candidates;
pub mod claude;
pub mod crdt_mode;
pub mod driver;
pub mod git_mode;
pub mod grader;
pub mod lang;
pub mod metrics;
pub mod mock;
pub mod mode;
pub mod pi;
pub mod plan;
pub mod project;
pub mod prompts;
pub mod proxy;
pub mod python;
pub mod sandbox;
pub mod shim;
pub mod task;
pub mod worker;
