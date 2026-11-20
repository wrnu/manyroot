//! Git project mode's rules on names and trees.

use super::*;

#[test]
fn a_branch_name_is_plain_and_never_the_hubs() {
    for good in ["fix", "fix/parse-dates", "a.b_c-1", "agent-0-turn-2", "x/y/z"] {
        assert_eq!(branch_name(good), Ok(()), "{good}");
    }
    let long = "a".repeat(MAX_BRANCH + 1);
    for bad in [
        "", "Fix", "a b", "-a", ".a", "a/.b", "a/-b", "a..b", "a.lock", "a/", "/a", "a//b", "origin", "origin/x",
        "head", "a@{1}", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "é", &long,
    ] {
        assert!(branch_name(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn only_the_named_integrator_or_any_agent_updates_main() {
    assert!(may_update_main(ANY_AGENT, "agent-0") && may_update_main(ANY_AGENT, "agent-7"));
    assert!(may_update_main("agent-3", "agent-3"));
    assert!(!may_update_main("agent-3", "agent-0") && !may_update_main("agent-3", "agent-30"));
}

/// `git ls-tree -r -l -z` entries: only regular files at plain paths, within the limits, are taken.
#[test]
fn a_tree_holds_only_regular_files_at_plain_paths() {
    let sha = "a".repeat(40);
    let entry = |mode: &str, kind: &str, size: usize, path: &[u8]| {
        let mut e = format!("{mode} {kind} {sha} {size:>7}\t").into_bytes();
        e.extend_from_slice(path);
        e.push(0);
        e
    };
    let ok = [entry("100644", "blob", 10, b"pkg/a.py"), entry("100755", "blob", 0, b"run.sh")].concat();
    assert_eq!(entries(&ok).expect("taken").len(), 2);
    for (bad, why) in [
        (entry("120000", "blob", 3, b"link"), "isn't a regular file"),
        (entry("160000", "commit", 0, b"sub"), "isn't a regular file"),
        (entry("100644", "blob", 1, b".git/config"), "start with .git"),
        (entry("100644", "blob", 1, b"a/.gitattributes"), "start with .git"),
        (entry("100644", "blob", 1, b"a/../b"), ".."),
        (entry("100644", "blob", 1, b"bad\xff.py"), "UTF-8"),
        (entry("100644", "blob", MAX_FILE_BYTES + 1, b"big.py"), "over the limit"),
    ] {
        let err = entries(&bad).expect_err("refused");
        assert!(err.contains(why), "{why}: {err}");
    }
    let many: Vec<u8> = (0..=MAX_FILES).flat_map(|i| entry("100644", "blob", 1, format!("f{i}").as_bytes())).collect();
    assert!(entries(&many).is_err_and(|e| e.contains("files, over the limit")));
}

#[test]
fn tree_at_takes_only_main_or_an_id() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let git = crate::driver::git_binary().expect("git");
    for rev in ["--output=/tmp/x", "HEAD", "main~1", "refs/heads/x"] {
        assert!(tree_at(&git, dir.path(), rev).is_err_and(|e| e.contains("neither")), "{rev}");
    }
}
