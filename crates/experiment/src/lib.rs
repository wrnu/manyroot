//! The Phase 1 experiment: the same task under three collaboration modes (Git
//! branches and pull requests, CRDT shared editing, Bracken contributions),
//! measured the same way.
//!
//! Build step 1 is `stacklib`, the task (section 1 of
//! `docs/design/phase1-experiment.md`); step 2 is the core (the [`driver`],
//! its [`worker`]s, the [`shim`] and the [`mock`] backend) and Bracken mode
//! ([`bracken_mode`]), behind the [`mode`] interface.
//!
//! What an agent may see goes through [`task`] only: the language, the specs,
//! the change requests, the public cases and the public check. [`grader`]
//! holds the hidden cases and the scores, which no agent-facing tool may
//! return.

pub mod args;
pub mod bracken_mode;
pub mod candidates;
pub mod driver;
pub mod grader;
pub mod lang;
pub mod mock;
pub mod mode;
pub mod shim;
pub mod task;
pub mod worker;
