//! scripts/code-scope. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// scripts/code-scope full names each path a head changed that needs the whole table, a rename or a
/// deletion of one too (a rename counts as a deletion and an addition), and any build.rs. code-scope
/// packages asks bracken-scope packages (here a stub, through CODE_SCOPE_TOKENS; task de42fa5c776d) for
/// every package when full names a path, and otherwise for the packages the changed paths affect, given
/// NUL-separated, from the merge base to the head; it asks nothing for no change, and its failure is exit 2,
/// saying so. code-scope light-passes says which of the five builder passes a light run needs are missing
/// on the sha.
#[test]
fn code_scope_names_what_needs_the_full_table_and_the_light_scope() {
    for scripts in SCRIPTS {
        let scope = repo_root().join(scripts).join("code-scope");
        let files = [("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            put(dir, "crates/tree/src/old.rs", "tree\n");
            put(dir, "crates/manyroot/src/gone.rs", "protocol\n");
            put(dir, "notes.txt", "notes\n");
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?}");
        let stub = affected_stub(&tmp.path().join("bracken-scope"));
        let log = tmp.path().join("affected.log");
        let scope = |args: &[&str]| {
            let out = command(&dir, "scripts/code-scope", args)
                .env("CODE_SCOPE_TOKENS", &stub)
                .env("AFFECTED_LOG", &log)
                .output()
                .unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let branch = |name: &str, change: &dyn Fn()| {
            git(&["switch", "-qc", name, "main"]);
            change();
            commit_all(&dir, name);
            git(&["switch", "-q", "main"]);
        };
        branch("rename", &|| git(&["mv", "crates/tree/src/old.rs", "elsewhere.rs"]));
        branch("delete", &|| git(&["rm", "-q", "crates/manyroot/src/gone.rs"]));
        branch("docs", &|| put(&dir, "notes.txt", "more\n"));
        branch("demo", &|| {
            put(&dir, "crates/demo/src/main.rs", "demo\n");
            put(&dir, "scripts/x", "x\n");
        });
        branch("fmt", &|| put(&dir, "rustfmt.toml", "max_width = 120\n"));
        branch("build", &|| {
            put(&dir, "crates/tree/build.rs", "fn main() {}\n");
            put(&dir, "build.rs", "fn main() {}\n");
            put(&dir, "crates/demo/sub/build.rs", "fn main() {}\n");
        });

        let (code, out) = scope(&["full", "main", "rename"]);
        assert_eq!(code, Some(0), "{scripts}: {out}");
        assert!(out.contains("code-scope: full: crates/tree/src/old.rs ("), "{scripts}: {out}");
        let (code, out) = scope(&["full", "main", "delete"]);
        assert_eq!(code, Some(0), "{scripts}: {out}");
        assert!(out.contains("code-scope: full: crates/manyroot/src/gone.rs ("), "{scripts}: {out}");
        assert_eq!(scope(&["full", "main", "docs"]).0, Some(1), "{scripts}");
        // A build script anywhere, with no manifest change (finding 398c794c1707).
        let (code, out) = scope(&["full", "main", "build"]);
        assert_eq!(code, Some(0), "{scripts}: {out}");
        for path in ["build.rs", "crates/tree/build.rs", "crates/demo/sub/build.rs"] {
            assert!(out.contains(&format!("code-scope: full: {path} (a build script)")), "{scripts}: {path}: {out}");
        }
        assert_eq!(scope(&["full", "main", "no-such-branch"]).0, Some(2), "{scripts}");

        // What code-scope packages asked the tool, and what it printed.
        let asked = |branch: &str| {
            let _ = std::fs::remove_file(&log);
            let out = scope(&["packages", "main", branch]);
            (out, std::fs::read_to_string(&log).unwrap_or_default())
        };
        let rev = |r: &str| String::from_utf8(run(&dir, "git", &["rev-parse", r]).stdout).unwrap().trim().to_owned();
        let main = rev("main");
        let all = "bracken\nbracken-demo\nbracken-experiment\nbracken-scope\nbracken-tree\nmanyroot\n";
        // A path full names: every package, the tool asked only for the head's.
        for branch in ["rename", "delete", "build"] {
            assert_eq!(asked(branch), ((Some(0), all.to_owned()), format!("--all {branch}\n")), "{scripts}");
        }
        assert_eq!(
            asked("docs"),
            ((Some(0), "bracken\nbracken-tree\n".to_owned()), format!("{main} docs <notes.txt>\n")),
            "{scripts}"
        );
        assert_eq!(
            asked("demo"),
            (
                (Some(0), "bracken\nbracken-demo\nbracken-experiment\n".to_owned()),
                format!("{main} demo <crates/demo/src/main.rs scripts/x>\n")
            ),
            "{scripts}"
        );
        assert_eq!(asked("fmt"), ((Some(0), String::new()), format!("{main} fmt <rustfmt.toml>\n")), "{scripts}");
        // No change: nothing asked.
        assert_eq!(asked("main"), ((Some(0), String::new()), String::new()), "{scripts}");
        let failed = command(&dir, "scripts/code-scope", &["packages", "main", "docs"])
            .env("CODE_SCOPE_TOKENS", &stub)
            .env("AFFECTED_FAIL", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&failed.stderr);
        assert_eq!(failed.status.code(), Some(2), "{scripts}: {stderr}");
        assert!(stderr.contains("bracken-scope packages failed (exit 3)") && stderr.contains("cargo metadata failed"));
        assert!(failed.stdout.is_empty(), "{scripts}");

        let h = "a".repeat(40);
        let passes = |methods: &[&str], packages: &[&str]| {
            let mut args = vec!["light-passes", h.as_str()];
            args.extend_from_slice(packages);
            let mut c = command(&dir, "scripts/code-scope", &args);
            let mut child = c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
            let text: String = methods.iter().map(|m| format!("{m} @ {h}\n")).collect();
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), text.as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let all = [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings",
            "cargo nextest run -P fast -p bracken-tree -p bracken",
            "scripts/test-table --remote -p bracken -p bracken-tree -p bracken-demo",
        ];
        assert_eq!(passes(&all, &["bracken", "bracken-tree"]), (Some(0), String::new()), "{scripts}");
        // bracken-demo is in the --remote run but not the fast run; bracken-tre isn't bracken-tree.
        let (code, out) = passes(&all, &["bracken", "bracken-demo"]);
        assert_eq!(code, Some(1), "{scripts}: {out}");
        assert_eq!(
            out,
            format!("no pass on {h} of: cargo nextest run -P fast -p bracken -p bracken-demo\n"),
            "{scripts}"
        );
        let (code, out) = passes(&all[1..], &["bracken-tre"]);
        assert_eq!(code, Some(1), "{scripts}: {out}");
        assert!(
            out.contains("cargo fmt --check; cargo nextest run -P fast -p bracken-tre; scripts/test-table --remote -p bracken-tre"),
            "{scripts}: {out}"
        );
        // The fast pass may carry -j <n>, --no-fail-fast and --test-threads <n> anywhere among its -p
        // arguments (task b1156a629af9), but nothing else, and must still name every package; the --remote
        // pass takes none of them.
        let with = |fast: &str, remote: &str| -> [String; 5] {
            let mut five = all.map(str::to_owned);
            five[3] = fast.to_owned();
            five[4] = remote.to_owned();
            five
        };
        let remote = "scripts/test-table --remote -p bracken -p bracken-tree";
        for fast in [
            "cargo nextest run -P fast -j 4 -p bracken -p bracken-tree",
            "cargo nextest run -P fast --no-fail-fast -p bracken -p bracken-tree",
            "cargo nextest run -P fast --test-threads 4 -p bracken -p bracken-tree",
            "cargo nextest run -P fast -p bracken -j 4 -p bracken-tree --no-fail-fast",
            "cargo nextest run -P fast -j 4 --no-fail-fast -p bracken-tree --test-threads 2 -p bracken",
        ] {
            let five = with(fast, remote);
            let five: Vec<&str> = five.iter().map(String::as_str).collect();
            assert_eq!(passes(&five, &["bracken", "bracken-tree"]), (Some(0), String::new()), "{scripts}: {fast}");
        }
        let missing_fast = format!("no pass on {h} of: cargo nextest run -P fast -p bracken -p bracken-tree\n");
        for fast in [
            // Fewer packages.
            "cargo nextest run -P fast -j 4 -p bracken",
            // Another flag, a flag with no number, a zero, and a flag that isn't one of them.
            "cargo nextest run -P fast --release -p bracken -p bracken-tree",
            "cargo nextest run -P fast -j -p bracken -p bracken-tree",
            "cargo nextest run -P fast --test-threads 0 -p bracken -p bracken-tree",
            "cargo nextest run -P fast -E all() -p bracken -p bracken-tree",
            // No package at all.
            "cargo nextest run -P fast -j 4 --no-fail-fast",
        ] {
            let five = with(fast, remote);
            let five: Vec<&str> = five.iter().map(String::as_str).collect();
            assert_eq!(
                passes(&five, &["bracken", "bracken-tree"]),
                (Some(1), missing_fast.clone()),
                "{scripts}: {fast}"
            );
        }
        let five = with("cargo nextest run -P fast -p bracken -p bracken-tree", &format!("{remote} -j 4"));
        let five: Vec<&str> = five.iter().map(String::as_str).collect();
        let (code, out) = passes(&five, &["bracken", "bracken-tree"]);
        assert!(code == Some(1) && out.contains("scripts/test-table --remote -p bracken"), "{scripts}: {out}");
    }
}
