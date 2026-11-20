//! AGENTS.md is the one doc every agent follows, so it stays short (task 9b65c13e9d66, owner decision
//! c8bbb2886be3): one rule in, one rule out.

use std::path::Path;

/// The most AGENTS.md may hold, in bytes: 12 KB.
const LIMIT: usize = 12 * 1024;

#[test]
fn agents_md_stays_within_12_kb() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../AGENTS.md");
    let size = std::fs::read(&path).expect("read AGENTS.md").len();
    assert!(
        size <= LIMIT,
        "AGENTS.md is {size} bytes, over its {LIMIT}-byte limit: retire a rule for each one you add (AGENTS.md, section 6)"
    );
}
