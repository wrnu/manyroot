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

/// A Cargo.lock with an entry for each of `entries`, (name, version, checksum), as cargo writes one.
fn lock(entries: &[(&str, &str, &str)]) -> String {
    let mut text = String::from(
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n",
    );
    for (name, version, sum) in entries {
        text += &format!(
            "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{sum}\"\ndependencies = [\n \"blake3\",\n]\n"
        );
    }
    text
}

/// Weave's format freeze, G6 (docs/design/weave-freeze.md 2.4, task e1d2cb38d478): code-scope frozen names
/// every changed path under crates/tree/src/ (a new file and a rename away included), crates/tree's
/// manifest, the format guard, paths.rs, lines.rs, the fixture test and tests/fixtures/, and a Cargo.lock
/// change to the entry of a crate the formats rest on, each with its reason; an 'owner:' line for a golden
/// vector or pinned fixture value; and a 'security:' line for every path in the directories of the
/// freeze's refusals, Weave's CLI included. It exits 3, with its one line, for a head that changes none of
/// these, crates/tree's other tests and another crate's Cargo.lock entry included, and 2 on an error. A
/// changed path holding a newline or a tab is an error, named escaped, with nothing on standard output
/// (findings db2dc1c6120e, e3f9ae2b1820). full names the new paths in its list too.
#[test]
fn code_scope_frozen_names_the_freezes_paths_each_with_what_it_needs() {
    let base_lock = lock(&[("blake3", "1.8.7", "aa"), ("serde", "1.0.0", "bb"), ("icu_casemap", "2.4.0", "cc")]);
    for scripts in SCRIPTS {
        let scope = repo_root().join(scripts).join("code-scope");
        let files = [("scripts/code-scope", scope.as_path())];
        let (_tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            put(dir, "crates/tree/src/old.rs", "tree\n");
            put(dir, "crates/tree/src/format.rs", "format\n");
            put(dir, "Cargo.lock", &base_lock);
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?}");
        let branch = |name: &str, change: &dyn Fn()| {
            git(&["switch", "-qc", name, "main"]);
            change();
            commit_all(&dir, name);
            git(&["switch", "-q", "main"]);
        };
        let frozen = |head: &str| {
            let out = run(&dir, "scripts/code-scope", &["frozen", "main", head]);
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let src = "crates/tree's source, which holds Weave's frozen formats and their rules";
        let refusals = "crates/tree's source, where the freeze's refusals at write and use live";
        let cli = "Weave's CLI, where the freeze's refusals at use live";

        branch("source", &|| {
            put(&dir, "crates/tree/src/format.rs", "format, changed\n");
            put(&dir, "crates/tree/src/check/new.rs", "a new module\n");
        });
        assert_eq!(
            frozen("source"),
            (
                Some(0),
                format!(
                    "code-scope: frozen: crates/tree/src/check/new.rs ({src})\n\
                     code-scope: security: crates/tree/src/check/new.rs ({refusals})\n\
                     code-scope: frozen: crates/tree/src/format.rs ({src})\n\
                     code-scope: security: crates/tree/src/format.rs ({refusals})\n"
                )
            ),
            "{scripts}"
        );
        branch("moved", &|| git(&["mv", "crates/tree/src/old.rs", "elsewhere.rs"]));
        let (code, out) = frozen("moved");
        assert_eq!(code, Some(0), "{scripts}: {out}");
        assert!(out.contains(&format!("code-scope: frozen: crates/tree/src/old.rs ({src})")), "{scripts}: {out}");
        assert!(out.contains("code-scope: security: crates/tree/src/old.rs ("), "{scripts}: {out}");

        branch("guards", &|| {
            put(&dir, "crates/tree/Cargo.toml", "[package]\n");
            put(&dir, "crates/tree/tests/format_guard.rs", "// a golden vector\n");
            put(&dir, "crates/tree/tests/paths.rs", "// paths\n");
            put(&dir, "crates/tree/tests/lines.rs", "// lines\n");
            put(&dir, "crates/tree/tests/weave_fixture.rs", "// the fixture\n");
            put(&dir, "crates/tree/tests/fixtures/weave-v1/objects/ab", "bytes\n");
        });
        assert_eq!(
            frozen("guards"),
            (
                Some(0),
                "code-scope: frozen: crates/tree/Cargo.toml (crates/tree's manifest: what its formats build from)\n\
                 code-scope: frozen: crates/tree/tests/fixtures/weave-v1/objects/ab (a Weave fixture)\n\
                 code-scope: owner: crates/tree/tests/fixtures/weave-v1/objects/ab (a Weave fixture: the owner's own go)\n\
                 code-scope: frozen: crates/tree/tests/format_guard.rs (the format guard: golden vectors and pins)\n\
                 code-scope: owner: crates/tree/tests/format_guard.rs (golden vectors and pinned values: the owner's own go)\n\
                 code-scope: frozen: crates/tree/tests/lines.rs (the ancestor rule's guard)\n\
                 code-scope: frozen: crates/tree/tests/paths.rs (the path, folding and collision rules' guard)\n\
                 code-scope: frozen: crates/tree/tests/weave_fixture.rs (the weave-v1 fixture's test and its pinned values)\n\
                 code-scope: owner: crates/tree/tests/weave_fixture.rs (the weave-v1 fixture's pinned values: the owner's own go)\n"
                    .to_owned()
            ),
            "{scripts}"
        );

        branch("cli", &|| {
            put(&dir, "crates/bracken/src/weave.rs", "cli\n");
            put(&dir, "crates/bracken/src/weave/merge.rs", "cli\n");
        });
        assert_eq!(
            frozen("cli"),
            (
                Some(0),
                format!(
                    "code-scope: security: crates/bracken/src/weave.rs ({cli})\n\
                     code-scope: security: crates/bracken/src/weave/merge.rs ({cli})\n"
                )
            ),
            "{scripts}"
        );

        // Cargo.lock: a version, a checksum alone, an entry removed, and another crate's change.
        let lockfile = |text: String| {
            let dir = dir.as_path();
            move || put(dir, "Cargo.lock", &text)
        };
        branch(
            "bump",
            &lockfile(lock(&[("blake3", "1.9.0", "dd"), ("serde", "1.0.0", "bb"), ("icu_casemap", "2.4.0", "cc")])),
        );
        branch(
            "checksum",
            &lockfile(lock(&[("blake3", "1.8.7", "ee"), ("serde", "1.0.0", "bb"), ("icu_casemap", "2.4.0", "cc")])),
        );
        branch("dropped", &lockfile(lock(&[("blake3", "1.8.7", "aa"), ("serde", "1.0.0", "bb")])));
        branch(
            "other",
            &lockfile(lock(&[("blake3", "1.8.7", "aa"), ("serde", "1.0.1", "ff"), ("icu_casemap", "2.4.0", "cc")])),
        );
        let rests = "and Weave's frozen formats rest on it";
        let expect = [
            ("bump", format!("code-scope: frozen: Cargo.lock (blake3 1.8.7 -> 1.9.0, {rests})\n")),
            ("checksum", format!("code-scope: frozen: Cargo.lock (blake3 1.8.7: its entry changed, {rests})\n")),
            ("dropped", format!("code-scope: frozen: Cargo.lock (icu_casemap 2.4.0 -> none, {rests})\n")),
        ];
        for (head, out) in expect {
            assert_eq!(frozen(head), (Some(0), out), "{scripts}: {head}");
        }

        // Nothing the freeze guards: exit 3, saying so.
        branch("elsewhere", &|| {
            put(&dir, "crates/tree/tests/merge.rs", "another test\n");
            put(&dir, "crates/bracken/src/main.rs", "the rest of the CLI\n");
            put(&dir, "crates/bracken/src/weaver.rs", "not weave.rs\n");
        });
        for head in ["other", "elsewhere"] {
            let (code, out) = frozen(head);
            assert_eq!(code, Some(3), "{scripts}: {head}: {out}");
            assert!(out.starts_with("code-scope: nothing since ") && out.lines().count() == 1, "{scripts}: {out}");
        }
        assert_eq!(frozen("no-such-branch").0, Some(2), "{scripts}");

        // A path with a newline or a tab, frozen or not, anywhere among the changes: refused, named escaped.
        for (name, file, escaped) in [
            ("newline", "crates/tree/src/a\nb.rs", "$'crates/tree/src/a\\nb.rs'"),
            ("tab", "notes/x\ty.txt", "$'notes/x\\ty.txt'"),
        ] {
            branch(name, &|| {
                put(&dir, "crates/tree/src/format.rs", "format, changed again\n");
                put(&dir, file, "a name with a control character\n");
            });
            let out = run(&dir, "scripts/code-scope", &["frozen", "main", name]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{scripts}: {name}: {stderr}");
            assert!(out.stdout.is_empty(), "{scripts}: {name}: {:?}", out.stdout);
            assert!(stderr.contains("control character") && stderr.contains(escaped), "{scripts}: {name}: {stderr}");
        }
        assert_eq!(run(&dir, "scripts/code-scope", &["frozen", "main"]).status.code(), Some(2), "{scripts}");

        // full's list holds the freeze's guards and Weave's CLI (G6).
        let out = run(&dir, "scripts/code-scope", &["full", "main", "guards"]);
        let said = String::from_utf8_lossy(&out.stdout);
        for path in ["format_guard.rs", "paths.rs", "lines.rs"] {
            let line = format!("code-scope: full: crates/tree/tests/{path} (a guard of Weave's frozen formats)");
            assert!(said.contains(&line), "{scripts}: {path}: {said}");
        }
        let out = run(&dir, "scripts/code-scope", &["full", "main", "cli"]);
        assert_eq!(out.status.code(), Some(0), "{scripts}");
        let said = String::from_utf8_lossy(&out.stdout);
        for path in ["crates/bracken/src/weave.rs", "crates/bracken/src/weave/merge.rs"] {
            assert!(said.contains(&format!("code-scope: full: {path} (Weave's CLI")), "{scripts}: {path}: {said}");
        }
        assert_eq!(run(&dir, "scripts/code-scope", &["full", "main", "elsewhere"]).status.code(), Some(1), "{scripts}");
    }
}

/// Makes a commit in `dir` with git's plumbing, through an index of its own, never the working tree: the
/// tree of `from` with `files` (path, contents) written over it, and `parents` as its parents. So a path in
/// a case a case-insensitive filesystem would fold is committed as given. Returns its sha.
fn plumb(dir: &Path, from: &str, parents: &[&str], files: &[(&str, &str)]) -> String {
    let index = dir.join(format!("../index-{}", files.len() + parents.len() * 100 + from.len() * 10_000));
    let _ = std::fs::remove_file(&index);
    let git = |args: &[&str]| {
        let out = command(dir, "git", args).env("GIT_INDEX_FILE", &index).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["read-tree", from]);
    let blob = dir.join("../plumb-blob");
    for (path, text) in files {
        std::fs::write(&blob, text).unwrap();
        let oid = git(&["hash-object", "-w", blob.to_str().unwrap()]);
        git(&["update-index", "--add", "--cacheinfo", &format!("100644,{oid},{path}")]);
    }
    let tree = git(&["write-tree"]);
    let mut args = vec!["commit-tree", tree.as_str(), "-m", "plumbed"];
    for p in parents {
        args.extend(["-p", p]);
    }
    git(&args)
}

/// Findings 89119f47ba5c and bb7107f27076. Paths are matched with ASCII case folded, since a
/// case-insensitive checkout (macOS's default) writes Crates/Tree/src/x.rs over crates/tree/src/x.rs: frozen
/// and full name a case variant of their paths. frozen refuses, exit 2 naming both, a changed path that
/// differs only in case from another path in the merged tree. With several merge bases (a criss-cross
/// history, which a diff from one can miss), frozen refuses, exit 2, and full says every package is
/// needed, as packages then gives.
#[test]
fn code_scope_folds_case_and_fails_closed_on_a_collision_or_a_criss_cross() {
    for scripts in SCRIPTS {
        let scope = repo_root().join(scripts).join("code-scope");
        let files = [("scripts/code-scope", scope.as_path())];
        let (tmp, dir) = repo(&files, ".bracken/\n", |dir| {
            put(dir, "crates/tree/src/format.rs", "format\n");
            put(dir, "notes.txt", "notes\n");
        });
        let stub = affected_stub(&tmp.path().join("bracken-scope"));
        let scope = |args: &[&str]| {
            let out = command(&dir, "scripts/code-scope", args).env("CODE_SCOPE_TOKENS", &stub).output().unwrap();
            let said =
                (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned());
            (out.status.code(), said.0, said.1)
        };
        let main = String::from_utf8(run(&dir, "git", &["rev-parse", "main"]).stdout).unwrap().trim().to_owned();
        let head = |files: &[(&str, &str)]| plumb(&dir, &main, &[&main], files);

        // Case variants of each kind, none colliding with a path that exists: named, as their kind.
        let variants = head(&[
            ("Crates/Tree/src/New.rs", "new\n"),
            ("crates/tree/CARGO.toml", "[package]\n"),
            ("crates/tree/tests/FIXTURES/x", "fixture\n"),
            ("crates/bracken/src/WEAVE.rs", "cli\n"),
        ]);
        let (code, out, err) = scope(&["frozen", &main, &variants]);
        assert_eq!(code, Some(0), "{scripts}: {out}{err}");
        for line in [
            "code-scope: frozen: Crates/Tree/src/New.rs (",
            "code-scope: security: Crates/Tree/src/New.rs (",
            "code-scope: frozen: crates/tree/CARGO.toml (",
            "code-scope: frozen: crates/tree/tests/FIXTURES/x (",
            "code-scope: owner: crates/tree/tests/FIXTURES/x (",
            "code-scope: security: crates/bracken/src/WEAVE.rs (",
        ] {
            assert!(out.contains(line), "{scripts}: {line}: {out}");
        }
        let (code, out, _) = scope(&["full", &main, &variants]);
        assert_eq!(code, Some(0), "{scripts}: {out}");
        for path in ["Crates/Tree/src/New.rs", "crates/tree/CARGO.toml", "crates/bracken/src/WEAVE.rs"] {
            assert!(out.contains(&format!("code-scope: full: {path} (")), "{scripts}: {path}: {out}");
        }
        let (code, out, _) = scope(&["full", &main, &head(&[("CARGO.toml", "[workspace]\n")])]);
        assert!(code == Some(0) && out.contains("code-scope: full: CARGO.toml (the workspace's CARGO.toml)"), "{out}");

        // A case collision in the merged tree: refused, naming both, whether the path is frozen or not, and
        // whichever side came first.
        for (path, other) in [
            ("Crates/Tree/src/format.rs", "crates/tree/src/format.rs"),
            ("crates/tree/src/FORMAT.rs", "crates/tree/src/format.rs"),
            ("NOTES.txt", "notes.txt"),
        ] {
            let (code, out, err) = scope(&["frozen", &main, &head(&[(path, "over it\n")])]);
            assert_eq!(code, Some(2), "{scripts}: {path}: {out}{err}");
            assert!(out.is_empty(), "{scripts}: {path}: {out}");
            assert!(err.contains("differing only in case") && err.contains(path) && err.contains(other), "{err}");
        }
        // Two new paths that collide with each other.
        let (code, _, err) = scope(&["frozen", &main, &head(&[("docs/a.md", "a\n"), ("docs/A.md", "A\n")])]);
        assert!(code == Some(2) && err.contains("docs/A.md and docs/a.md"), "{scripts}: {err}");

        // A head that doesn't merge cleanly: refused.
        let tip = plumb(&dir, &main, &[&main], &[("notes.txt", "main's\n")]);
        let (code, _, err) = scope(&["frozen", &tip, &head(&[("notes.txt", "the head's\n")])]);
        assert!(code == Some(2) && err.contains("doesn't merge cleanly"), "{scripts}: {err}");

        // A criss-cross (the finding's counterexample): B2 changes a frozen file, B1 something else; the tip
        // merges B2 into B1, and the head merges them too, putting the frozen file back.
        let b2 = head(&[("crates/tree/src/f.rs", "y\n")]);
        let b1 = head(&[("notes.txt", "b1\n")]);
        let tip = plumb(&dir, &b2, &[&b1, &b2], &[("notes.txt", "b1\n")]);
        let crossed = plumb(&dir, &b1, &[&b2, &b1], &[]);
        let (code, out, err) = scope(&["frozen", &tip, &crossed]);
        assert_eq!(code, Some(2), "{scripts}: {out}{err}");
        assert!(out.is_empty() && err.contains("are all merge bases") && err.contains("criss-cross"), "{err}");
        let (code, out, _) = scope(&["full", &tip, &crossed]);
        assert!(
            code == Some(0) && out.starts_with("code-scope: full: every package (") && out.contains("criss-cross"),
            "{out}"
        );
        let (code, out, _) = scope(&["packages", &tip, &crossed]);
        let all = "bracken\nbracken-demo\nbracken-experiment\nbracken-scope\nbracken-tree\nmanyroot\n";
        assert_eq!((code, out.as_str()), (Some(0), all), "{scripts}");
    }
}
