//! The Phase 1 experiment: the same task under three collaboration modes (Git
//! branches and pull requests, CRDT shared editing, Bracken contributions),
//! measured the same way.
//!
//! This is build step 1: `stacklib`, the task. See
//! `docs/design/phase1-experiment.md`, section 1.
//!
//! What an agent may see goes through [`task`] only: the language, the specs,
//! the change requests, the public cases and the public check. [`grader`]
//! holds the hidden cases and the scores, which no agent-facing tool may
//! return.

pub mod candidates;
pub mod grader;
pub mod lang;
pub mod task;
