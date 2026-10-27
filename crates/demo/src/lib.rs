//! A demo harness for Bracken: several agents work on a toy task through the
//! real `bracken` binary, each with its own identity and replica, syncing with
//! bundles between rounds, and the run ends with a measured report.
//!
//! See `docs/demo.md` for how to run it and what the numbers mean.

pub mod agent;
pub mod bracken;
pub mod compare;
pub mod harness;
pub mod report;
pub mod rng;
pub mod safety;
pub mod task;
