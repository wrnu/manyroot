//! The team scripts in `scripts/` (and their copies in the agent-team-kickoff skill), run
//! against a throwaway Ground in a throwaway git repository.
//!
//! One module per script (task a6ecb74b4f1d), so PRs to different scripts don't meet at one file's end;
//! what they share is in `support`.

#[path = "../common/mod.rs"]
mod common;
mod support;

mod bk;
mod bk_evidence;
mod cleanup;
mod code_scope;
mod ground_checkpoint;
mod ground_guards;
mod merge;
mod owner_approve;
mod pi_prefix;
mod queue;
mod rustc_wrapper;
mod services;
mod test_table;
mod verify_seq;
mod weave;
