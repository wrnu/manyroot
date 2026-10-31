//! `bracken-scope packages` and the library's `packages` (task de42fa5c776d): on throwaway workspaces
//! committed twice, and on this repository's own package graph at HEAD. The parity check against the old
//! map was a one-off, on the 30 merges ending at 337034cb; its output is in PR #180.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use bracken_scope::packages;
use guppy::graph::PackageGraph;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn put(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// A crate `name` under crates/, depending on the crates in `deps` by path.
fn krate(dir: &Path, name: &str, deps: &[&str], description: &str) {
    let mut manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\ndescription = \"{description}\"\n\n[dependencies]\n"
    );
    for dep in deps {
        manifest.push_str(&format!("{dep} = {{ path = \"../{dep}\" }}\n"));
    }
    put(dir, &format!("crates/{name}/Cargo.toml"), &manifest);
    put(dir, &format!("crates/{name}/src/lib.rs"), "pub fn f() {}\n");
}

/// The tool, run in `dir` with `args` and `paths` on standard input: its exit code and its lines.
fn tool(dir: &Path, args: &[&str], paths: &[&str]) -> (Option<i32>, Vec<String>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bracken-scope"))
        .arg("packages")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input: String = paths.iter().map(|p| format!("{p}\0")).collect();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let lines = String::from_utf8(out.stdout).unwrap().lines().map(str::to_owned).collect();
    (out.status.code(), lines, String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Two commits of a workspace of crates a, b (which depends on a) and c, each unpacked in a directory of
/// its own: a change to a affects a and b, its real reverse dependency, and not c; a change only to c's
/// manifest affects c alone. Packages left alone aren't marked, though the two graphs' package ids name
/// different directories. --all names every member; a bad commit is exit 2, saying why.
#[test]
fn the_tool_follows_the_real_graph_between_two_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    put(dir, "Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\"crates/a\", \"crates/b\", \"crates/c\"]\n");
    krate(dir, "a", &[], "a");
    krate(dir, "b", &["a"], "b");
    krate(dir, "c", &[], "c");
    let lock = Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(dir).output().unwrap();
    assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);
    let base = git(dir, &["rev-parse", "HEAD"]);

    put(dir, "crates/a/src/lib.rs", "pub fn f() { g() }\nfn g() {}\n");
    git(dir, &["commit", "-qam", "a changes"]);
    let (code, lines, err) = tool(dir, &[&base, "HEAD"], &["crates/a/src/lib.rs"]);
    assert_eq!((code, lines), (Some(0), vec!["a".to_owned(), "b".to_owned()]), "{err}");

    git(dir, &["reset", "-q", "--hard", &base]);
    krate(dir, "c", &[], "c, described again");
    git(dir, &["commit", "-qam", "c's manifest changes"]);
    let (code, lines, err) = tool(dir, &[&base, "HEAD"], &["crates/c/Cargo.toml"]);
    assert_eq!((code, lines), (Some(0), vec!["c".to_owned()]), "{err}");

    // No paths: nothing affected, and no graph built.
    assert_eq!(tool(dir, &[&base, "no-such-commit"], &[]).1, Vec::<String>::new());
    assert_eq!(tool(dir, &["--all", "HEAD"], &[]).1, ["a", "b", "c"]);
    let (code, _, err) = tool(dir, &[&base, "no-such-commit"], &["crates/a/src/lib.rs"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("git archive no-such-commit failed"), "{err}");
    let (code, _, err) = tool(dir, &["--bad"], &[]);
    assert!(code == Some(2) && err.contains("usage"), "{err}");
}

/// A workspace with bracken, a library it depends on, bracken-demo and bracken-experiment (which depends
/// on demo), neither depending on bracken. A change to the library changes bracken's build, so demo and
/// experiment, whose tests run its binary, come too; a doc marks bracken by its rule, for its tests, and
/// brings neither.
#[test]
fn a_change_to_brackens_build_brings_the_packages_that_run_its_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    let members = ["crates/lib", "crates/bracken", "crates/bracken-demo", "crates/bracken-experiment"];
    put(dir, "Cargo.toml", &format!("[workspace]\nresolver = \"3\"\nmembers = {members:?}\n"));
    put(dir, "README.md", "# Readme\n");
    krate(dir, "lib", &[], "lib");
    krate(dir, "bracken", &["lib"], "bracken");
    krate(dir, "bracken-demo", &[], "demo");
    krate(dir, "bracken-experiment", &["bracken-demo"], "experiment");
    let lock = Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(dir).output().unwrap();
    assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);
    let base = git(dir, &["rev-parse", "HEAD"]);

    put(dir, "crates/lib/src/lib.rs", "pub fn f() { g() }\nfn g() {}\n");
    put(dir, "README.md", "# Readme, again\n");
    git(dir, &["commit", "-qam", "lib and a doc change"]);
    let built = ["bracken", "bracken-demo", "bracken-experiment", "lib"].map(str::to_owned).to_vec();
    let (code, lines, err) = tool(dir, &[&base, "HEAD"], &["crates/lib/src/lib.rs"]);
    assert_eq!((code, lines), (Some(0), built), "{err}");
    let (code, lines, err) = tool(dir, &[&base, "HEAD"], &["README.md"]);
    assert_eq!((code, lines), (Some(0), vec!["bracken".to_owned()]), "{err}");
}

/// This repository's package graph at HEAD, built once.
fn here() -> &'static PackageGraph {
    static GRAPH: OnceLock<PackageGraph> = OnceLock::new();
    GRAPH.get_or_init(|| packages::graph_at(&repo_root(), "HEAD").expect("HEAD's package graph"))
}

fn affected(paths: &[&str]) -> Vec<String> {
    let rules = packages::rules().unwrap();
    packages::affected(here(), here(), &rules, paths.iter().copied()).unwrap()
}

/// On this repository's graph: a change in crates/tree selects bracken-tree and bracken, which depends on
/// it, and so demo and experiment, which run bracken's binary; one in crates/manyroot those and manyroot;
/// a manifest selects its crate and its dependents; crates/demo selects bracken-experiment, which depends
/// on it, and bracken by its rule. The rules file's paths map as it says, and a path outside every crate
/// and rule selects every package.
#[test]
fn this_workspaces_changes_select_their_real_reverse_dependencies_and_the_rules_paths() {
    const BRACKEN: &[&str] = &["bracken", "bracken-demo", "bracken-experiment"];
    let all = packages::all(here());
    assert_eq!(all, ["bracken", "bracken-demo", "bracken-experiment", "bracken-scope", "bracken-tree", "manyroot"]);
    let cases: &[(&[&str], &[&str])] = &[
        (&["crates/tree/tests/merge.rs"], &[BRACKEN, &["bracken-tree"]].concat()),
        (&["crates/tree/src/lines.rs"], &[BRACKEN, &["bracken-tree"]].concat()),
        (&["crates/manyroot/src/graph.rs"], &[BRACKEN, &["bracken-tree", "manyroot"]].concat()),
        (&["crates/tree/Cargo.toml"], &[BRACKEN, &["bracken-tree"]].concat()),
        (&["crates/demo/src/main.rs"], &["bracken", "bracken-demo", "bracken-experiment"]),
        (&["crates/experiment/src/lib.rs"], &["bracken", "bracken-experiment"]),
        (&["crates/bracken/src/main.rs"], &["bracken", "bracken-demo", "bracken-experiment"]),
        (&["crates/scope/src/lib.rs"], &["bracken-scope"]),
        (&["scripts/merge", ".claude/skills/agent-team-kickoff/templates/merge"], &["bracken"]),
        (&["scripts/code-scope"], &["bracken", "bracken-scope"]),
        (&[".claude/skills/agent-team-kickoff/templates/code-scope"], &["bracken", "bracken-scope"]),
        (&["docs/development.md", "AGENTS.md", ".bracken/objects/ab/cd", ".gitignore"], &["bracken", "bracken-tree"]),
        (&[".config/determinator.toml"], &["bracken", "bracken-scope", "bracken-tree"]),
        (&["rustfmt.toml"], &[]),
        (&["a-new-root-file"], &all.iter().map(String::as_str).collect::<Vec<_>>()),
        (&["docs/x.md", "tools/new/thing.rs"], &all.iter().map(String::as_str).collect::<Vec<_>>()),
    ];
    for (paths, want) in cases {
        assert_eq!(affected(paths), *want, "{paths:?}");
    }
}
