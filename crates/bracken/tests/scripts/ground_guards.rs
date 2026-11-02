//! scripts/ground-guards. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// The object path main's .gitattributes export-ignores, for a head to hide a file at.
const HIDDEN: &str = ".bracken/objects/ab/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// Runs `scripts/ground-guards <args>` from `cwd` inside the repository, in the C locale. main holds a
/// committed Ground with docs/vision.md as its artifact, and a .gitattributes that keeps the Ground's
/// files from conversion and export-ignores `HIDDEN`. Branch `pr` commits what `on_branch` stages.
fn ground_guards(scripts: &str, cwd: &str, args: [&str; 2], on_branch: impl FnOnce(&Path)) -> Output {
    let guards = repo_root().join(scripts).join("ground-guards");
    let (_tmp, dir) = repo(&[("scripts/ground-guards", &guards)], ".bracken/identities/\n", |dir| {
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/vision.md"), "the vision\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), format!(".bracken/** -text\n{HIDDEN} export-ignore\n")).unwrap();
    });
    let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    bk(&dir, &["init", "--name", "guards"]);
    bk(&dir, &["id", "new", "val"]);
    bk(&dir, &["artifact", "docs/vision.md", "--as", "val"]);
    git(&["config", "core.ignorecase", "false"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "ground"]);
    git(&["switch", "-qc", "pr"]);
    on_branch(&dir);
    git(&["commit", "-q", "--allow-empty", "-m", "the change"]);
    command(&dir, dir.join("scripts/ground-guards").to_str().unwrap(), &args)
        .current_dir(dir.join(cwd))
        .env("LC_ALL", "C")
        .env_remove("LANG")
        .output()
        .unwrap()
}

/// What a case stages on the branch.
type Stager<'a> = Box<dyn FnOnce(&Path) + 'a>;

/// ground-guards refuses each way a head can slip a key or a broken Ground past it, saying which guard: a
/// file hidden from the copy it opens by export-ignore, attributes that would convert the Ground on
/// checkout, a path with a non-ASCII byte (a Kelvin sign folds to k), a malformed object, a symlink or an
/// executable, a .bracken/ path in another case, an agent entry holding a key, a vision document the Ground
/// doesn't match, a head that is its own base, and an edited object when it's run from a subdirectory.
#[test]
fn ground_guards_refuse_each_way_a_head_can_hide_or_break_the_ground() {
    let object = |name: char| format!(".bracken/objects/ab/{}", name.to_string().repeat(62));
    let key = format!("{{\"secret_key\": \"{}\"}}\n", "0".repeat(64));
    for scripts in SCRIPTS {
        let cases: Vec<(&str, &str, [&str; 2], Stager, &str)> = vec![
            ("clean", ".", ["main", "pr"], Box::new(|dir| stage(dir, "100644", "code.txt", "a change\n")), ""),
            ("export-ignore", ".", ["main", "pr"], Box::new(|dir| stage(dir, "100644", HIDDEN, &key)), "Ground opens"),
            (
                "a nested .gitattributes",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", "crates/manyroot/tests/fixtures/.gitattributes", "* -text\n")),
                "",
            ),
            (
                "attributes converting the Ground",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    stage(dir, "100644", ".gitattributes", ".bracken/** -text\n.bracken/objects/** text eol=crlf\n")
                }),
                ".gitattributes changed",
            ),
            (
                "a Kelvin sign for k",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    stage(dir, "100644", &format!(".brac\u{212A}en/objects/ee/{}", "d".repeat(62)), "garbage\n")
                }),
                "a non-ASCII byte in the path",
            ),
            (
                "malformed object",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", &object('d'), "not a contribution\n")),
                "Ground opens: the committed Ground doesn't open",
            ),
            (
                "symlink",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "120000", &object('e'), "/etc/passwd")),
                "mode 120000",
            ),
            (
                "executable",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100755", &object('f'), "#!/bin/sh\n")),
                "mode 100755",
            ),
            (
                "other case",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", ".BRACKEN/identities/k.json", &key)),
                ".BRACKEN/identities/k.json (not a file under .bracken/",
            ),
            (
                "keyed agent entry",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", ".bracken/agents/x.json", &key)),
                "only Ground files under .bracken",
            ),
            (
                "vision mismatch",
                ".",
                ["main", "pr"],
                Box::new(|dir| stage(dir, "100644", "docs/vision.md", "another vision\n")),
                "vision document: docs/vision.md differs",
            ),
            (
                "a Weave record",
                ".",
                ["main", "pr"],
                Box::new(|dir| {
                    weave_artifact(dir, "Application/VND.Bracken.Weave.ground+cbor");
                    assert!(run(dir, "git", &["add", "-A"]).status.success());
                }),
                "no Weave records",
            ),
            (
                "head is the base",
                ".",
                ["pr", "pr"],
                Box::new(|dir| stage(dir, "100644", "code.txt", "a change\n")),
                "nothing to compare it with; can't check",
            ),
            (
                "from a subdirectory",
                "docs",
                ["main", "pr"],
                Box::new(|dir| {
                    let objects = dir.join(".bracken/objects");
                    let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
                    let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
                    let mut bytes = std::fs::read(&file).unwrap();
                    bytes.push(b'x');
                    std::fs::write(&file, bytes).unwrap();
                    assert!(run(dir, "git", &["add", "-A"]).status.success());
                }),
                "Ground is append-only",
            ),
        ];
        for (case, cwd, args, on_branch, expected) in cases {
            let out = ground_guards(scripts, cwd, args, on_branch);
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let case = format!("{scripts}, {case}");
            if expected.is_empty() {
                assert!(out.status.success(), "{case}: {stdout}{stderr}");
                assert!(stdout.contains("all guards pass"), "{case}: {stdout}");
                continue;
            }
            assert_eq!(out.status.code(), Some(1), "{case} passed: {stdout}{stderr}");
            let last = stderr.lines().rfind(|l| l.starts_with("ground-guards:")).unwrap_or_default();
            let failure = &stderr[stderr.find(last).unwrap_or_default()..];
            assert!(failure.contains(expected), "{case}: expected {expected:?} in: {stderr}");
        }
    }
}
