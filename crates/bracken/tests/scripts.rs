//! The team scripts in `scripts/` (and their copies in the agent-team-kickoff skill), run
//! against a throwaway Ground in a throwaway git repository.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use manyroot::Body;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const SCRIPTS: [&str; 2] = ["scripts", ".claude/skills/agent-team-kickoff/templates"];

fn command(dir: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env("BRACKEN_BIN", dir.join("../bracken"))
        .env("REAL_BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT");
    command
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    command(dir, program, args).output().unwrap()
}

fn write_executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn bk(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, "scripts/bk", args);
    assert!(out.status.success(), "bk {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A git repository holding `scripts/bk` and the given `bk-evidence`, with a Ground and identity `val`.
///
/// `scripts/bk` runs bracken through a pipe that fails when its reader stops early, as an
/// older bracken or a `printf | grep -q` did: the scripts must not depend on how the writer
/// takes a closed pipe.
fn workspace(bk_evidence: &Path) -> (tempfile::TempDir, PathBuf, String) {
    let (tmp, dir) = repo(&[("scripts/bk-evidence", bk_evidence)], ".bracken/\n", |_| {});
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    bk(&dir, &["init", "--name", "scripts"]);
    bk(&dir, &["id", "new", "val"]);
    (tmp, dir, sha)
}

/// A git repository on main holding `scripts/bk`, the given files, and whatever `setup` adds, all committed.
fn repo(files: &[(&str, &Path)], gitignore: &str, setup: impl FnOnce(&Path)) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    write_executable(
        &tmp.path().join("bracken"),
        "#!/usr/bin/env bash\nset -o pipefail\n\"$REAL_BRACKEN\" \"$@\" | cat\n",
    );
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::copy(repo_root().join("scripts/bk"), dir.join("scripts/bk")).unwrap();
    for (to, from) in files {
        std::fs::copy(from, dir.join(to)).unwrap();
    }
    std::fs::write(dir.join(".gitignore"), gitignore).unwrap();
    setup(&dir);
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@example.invalid"],
        &["add", "-A"],
        &["commit", "-qm", "scripts"],
    ] {
        assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    }
    (tmp, dir)
}

/// --any-head must refuse a pr claim even when `log --tag pr --json` is over the 64 KiB pipe
/// buffer: a search that stops at the first match must not turn the writer's SIGPIPE into a pass.
#[test]
fn bk_evidence_any_head_refuses_a_pr_claim_in_a_large_pr_log() {
    for scripts in SCRIPTS {
        let script = format!("{scripts}/bk-evidence");
        let (_tmp, dir, sha) = workspace(&repo_root().join(&script));
        let padding = "x".repeat(4000);
        let mut prs = Vec::new();
        for n in 1..=20 {
            prs.push(bk(
                &dir,
                &["claim", &format!("PR #{n} branch-{n} @ {sha}: {padding}"), "--tag", "pr", "--as", "val"],
            ));
        }
        let log = bk(&dir, &["log", "--tag", "pr", "--json"]);
        assert!(log.len() > 64 * 1024, "the pr log is only {} bytes", log.len());

        // The first and last pr claims, wherever they fall in the log, are refused.
        for pr in [&prs[0], &prs[19]] {
            let out =
                run(&dir, "scripts/bk-evidence", &[&pr[..12], "--as", "val", "--any-head", "--", "/usr/bin/true"]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{script} accepted pr claim {pr}: {stderr}");
            assert!(stderr.contains("is tagged pr"), "{script}: {stderr}");
        }
        assert_eq!(bk(&dir, &["log", "--kind", "evidence", "--json"]), "[]", "{script} recorded evidence");

        // A claim that isn't a pr claim still takes --any-head.
        let note = bk(&dir, &["claim", "a note", "--tag", "note", "--as", "val"]);
        let out = run(&dir, "scripts/bk-evidence", &[&note[..12], "--as", "val", "--any-head", "--", "/usr/bin/true"]);
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// ground-checkpoint must refuse when what got staged isn't exactly the new Ground files, even when the
/// unexpected lines are over the 64 KiB pipe buffer: a search that stops at the first match must not turn
/// the writer's SIGPIPE into a pass. A `git` shim stages more along with the Ground's new files: new files
/// outside .bracken/, changes to tracked ones, or the removal of committed Ground files, whose paths are
/// allowed and are caught only because they aren't additions.
#[test]
fn ground_checkpoint_refuses_more_than_a_pipe_of_unexpected_staged_lines() {
    let real_git = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    let names: Vec<String> = (0..2000).map(|i| format!("extra/{i:04}-{}", "y".repeat(150))).collect();
    for scripts in SCRIPTS {
        for (case, also) in [
            ("new files", "add extra"),
            ("changed tracked files", "add extra"),
            ("unstaged Ground files", "rm -r --cached .bracken/objects"),
        ] {
            let tracked = case == "changed tracked files";
            let checkpoint = repo_root().join(scripts).join("ground-checkpoint");
            let (tmp, dir) = repo(&[("scripts/ground-checkpoint", &checkpoint)], ".bracken/identities/\n", |dir| {
                std::fs::create_dir_all(dir.join("extra")).unwrap();
                for name in names.iter().filter(|_| tracked) {
                    std::fs::write(dir.join(name), "old\n").unwrap();
                }
            });
            let case = format!("{scripts}, {case}");
            let checkpoint = |shim: bool| {
                let mut command = command(&dir, "scripts/ground-checkpoint", &[]);
                if shim {
                    let path = std::env::var("PATH").unwrap();
                    command.env("PATH", format!("{}:{path}", tmp.path().join("shim").display()));
                }
                command.env("REAL_GIT", &real_git).env("STAGE_ALSO", also).output().unwrap()
            };
            let head = || String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap();

            bk(&dir, &["init", "--name", "checkpoint"]);
            bk(&dir, &["id", "new", "val"]);
            bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
            if also.starts_with("rm") {
                // Enough committed objects that their removal is over 64 KiB of lines; written in
                // process, as a thousand bracken runs would be slow.
                let mut store = manyroot::Store::open(&dir.join(".bracken")).unwrap();
                let val = store.identity("val").unwrap();
                for i in 0..1000 {
                    store.contribute(&val, Body::Claim { statement: format!("filler {i}"), tags: vec![] }).unwrap();
                }
            }
            let out = checkpoint(false);
            assert!(out.status.success(), "{case}: {}", String::from_utf8_lossy(&out.stderr));

            bk(&dir, &["claim", "second", "--tag", "note", "--as", "val"]);
            for name in &names {
                std::fs::write(dir.join(name), "new\n").unwrap();
            }
            std::fs::create_dir_all(tmp.path().join("shim")).unwrap();
            write_executable(
                &tmp.path().join("shim/git"),
                "#!/usr/bin/env bash\n\"$REAL_GIT\" \"$@\" || exit\n\
                 [ \"$*\" != \"add --pathspec-from-file=-\" ] || \"$REAL_GIT\" $STAGE_ALSO\n",
            );
            let also: Vec<&str> = also.split(' ').collect();
            let dry_run = [&also[..1], &["--dry-run"], &also[1..]].concat();
            let staged = run(&dir, "git", &dry_run).stdout;
            assert!(staged.len() > 64 * 1024, "{case}: only {} bytes of extra staging", staged.len());
            let before = head();
            let out = checkpoint(true);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{case}: checkpoint committed {}", head());
            assert!(
                stderr.contains("staging didn't match what was checked"),
                "{case}: {}",
                &stderr[..500.min(stderr.len())]
            );
            assert_eq!(head(), before, "{case}: a commit was made");
            assert!(run(&dir, "git", &["diff", "--cached", "--quiet"]).status.success(), "{case}: left staged");
        }
    }
}

/// merge must not hand the forge token to its children when the caller exports a variable named `token`:
/// an assignment keeps an inherited export. The fake bracken reports every exported variable holding the
/// (dummy) token, by name; it runs only once the token is set.
#[test]
fn merge_keeps_the_token_out_of_the_environment_when_the_caller_exports_token() {
    for scripts in SCRIPTS {
        let merge = repo_root().join(scripts).join("merge");
        let (tmp, dir) = repo(&[("scripts/merge", &merge)], ".bracken/\n", |_| {});
        for args in [
            &["config", "remote.origin.url", "https://forge.example/o/r.git"][..],
            &[
                "config",
                "credential.https://forge.example.helper",
                "!f() { echo username=t; echo password=DUMMY-TOKEN-0123; }; f",
            ],
        ] {
            assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
        }
        let probe = tmp.path().join("probe.log");
        write_executable(
            &tmp.path().join("bracken"),
            "#!/usr/bin/env bash\necho ran >>\"$PROBE_LOG\"\n\
             for v in $(compgen -e); do [[ ${!v} != *DUMMY-TOKEN* ]] || echo \"exported: $v\"; done >>\"$PROBE_LOG\"\n\
             exit 1\n",
        );
        // merge only checks that curl and jq exist before the token; nothing reaches them here.
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        for tool in ["curl", "jq"] {
            write_executable(&tmp.path().join("stubs").join(tool), "#!/bin/sh\nexit 1\n");
        }
        let path = std::env::var("PATH").unwrap();
        let out = command(
            &dir,
            "scripts/merge",
            &["1", "0123456789ab", "--validator", "v", "--min-passes", "1", "--dry-run"],
        )
        .env("PATH", format!("{}:{path}", tmp.path().join("stubs").display()))
        .env("PROBE_LOG", &probe)
        .env("token", "unrelated")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("DUMMY-TOKEN") && !String::from_utf8_lossy(&out.stdout).contains("DUMMY-TOKEN"));
        let log = std::fs::read_to_string(&probe).unwrap_or_else(|_| panic!("{scripts}: bracken never ran: {stderr}"));
        assert_eq!(log, "ran\n", "{scripts}: the token reached bracken's environment");
    }
}

/// Runs `scripts/merge --dry-run` for PR #1 against a local forge: a bare repository standing in for
/// origin (through `insteadOf`) and a `curl` stub that answers with the PR. main holds a committed Ground;
/// `on_branch` changes the PR's head and `on_main` what main gains after the branch point, each committed
/// (`git add -A`, so a caller forces anything ignored in itself). The pr claim is by `builder` and passed
/// by `val`, so the gate accepts it.
fn merge_dry_run(scripts: &str, on_branch: impl FnOnce(&Path), on_main: impl FnOnce(&Path)) -> Output {
    let file = |name: &str| repo_root().join(scripts).join(name);
    let (merge, guards) = (file("merge"), file("ground-guards"));
    let (tmp, dir) =
        repo(&[("scripts/merge", &merge), ("scripts/ground-guards", &guards)], ".bracken/identities/\n", |_| {});
    let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
    let commit = |message: &str| {
        git(&["add", "-A"]);
        git(&["commit", "-q", "--allow-empty", "-m", message]);
    };
    bk(&dir, &["init", "--name", "merge"]);
    bk(&dir, &["id", "new", "builder"]);
    bk(&dir, &["id", "new", "val"]);
    bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
    commit("ground");
    git(&["switch", "-qc", "pr"]);
    on_branch(&dir);
    commit("the change");
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    git(&["switch", "-q", "main"]);
    on_main(&dir);
    commit("main moves on");

    let forge = tmp.path().join("forge.git");
    git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
    git(&["push", "-q", forge.to_str().unwrap(), "main", "pr:refs/pull/1/head"]);
    git(&["config", "remote.origin.url", "https://forge.example/o/r.git"]);
    git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
    git(&["config", &format!("url.{}.insteadOf", forge.display()), "https://forge.example/o/r.git"]);
    git(&["config", "credential.https://forge.example.helper", "!f() { echo username=t; echo password=DUMMY; }; f"]);
    let claim = bk(&dir, &["claim", &format!("PR #1 pr @ {sha}: a change"), "--tag", "pr", "--as", "builder"]);
    bk(&dir, &["evidence", &claim, "pass", "--method", "checked", "--as", "val"]);

    std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
    let pr =
        format!(r#"{{"head":{{"sha":"{sha}"}},"state":"open","merged":false,"base":{{"ref":"main"}},"title":"t"}}"#);
    write_executable(&tmp.path().join("stubs/curl"), &format!("#!/bin/sh\necho '{pr}'\n"));
    let path = std::env::var("PATH").unwrap();
    command(&dir, "scripts/merge", &["1", &claim[..12], "--validator", "val", "--min-passes", "1", "--dry-run"])
        .env("PATH", format!("{}:{path}", tmp.path().join("stubs").display()))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

/// The line `merge` printed for check `name`: `ok` or `FAIL`, then the check's name and detail.
fn check_line(out: &Output, name: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find(|l| l.split_whitespace().nth(1) == Some(name));
    line.unwrap_or_else(|| panic!("no {name} check in: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr)))
        .to_owned()
}

/// merge runs scripts/ground-guards on the PR's head: a clean head passes, and a head that commits a key
/// under .bracken/identities/ or changes a committed Ground object is refused, saying which guard.
#[test]
fn merge_refuses_a_head_that_fails_the_ground_guards() {
    for scripts in SCRIPTS {
        let clean = merge_dry_run(scripts, |dir| std::fs::write(dir.join("code.txt"), "a change\n").unwrap(), |_| {});
        let stderr = String::from_utf8_lossy(&clean.stderr);
        assert!(clean.status.success(), "{scripts}: {}{stderr}", String::from_utf8_lossy(&clean.stdout));
        assert!(check_line(&clean, "guards").starts_with("ok "), "{scripts}: {}", check_line(&clean, "guards"));

        let key = merge_dry_run(
            scripts,
            |dir| {
                std::fs::write(dir.join(".bracken/identities/leak.json"), "{\"secret_key\": \"00\"}\n").unwrap();
                assert!(run(dir, "git", &["add", "-f", ".bracken/identities/leak.json"]).status.success());
            },
            |_| {},
        );
        let line = check_line(&key, "guards");
        assert_eq!(key.status.code(), Some(1), "{scripts}: merged a head with a key");
        assert!(line.starts_with("FAIL") && line.contains("only Ground files under .bracken"), "{scripts}: {line}");

        let object = merge_dry_run(
            scripts,
            |dir| {
                let objects = dir.join(".bracken/objects");
                let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
                let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
                let mut bytes = std::fs::read(&file).unwrap();
                bytes.push(b'x');
                std::fs::write(&file, bytes).unwrap();
            },
            |_| {},
        );
        let line = check_line(&object, "guards");
        assert_eq!(object.status.code(), Some(1), "{scripts}: merged a head that edits the Ground");
        assert!(line.starts_with("FAIL") && line.contains("Ground is append-only"), "{scripts}: {line}");

        // Guard 3 passes and says so before guard 4 fails: merge names guard 4.
        let vision = merge_dry_run(
            scripts,
            |dir| {
                std::fs::create_dir_all(dir.join("docs")).unwrap();
                std::fs::write(dir.join("docs/vision.md"), "a vision with no artifact\n").unwrap();
            },
            |_| {},
        );
        let line = check_line(&vision, "guards");
        assert_eq!(vision.status.code(), Some(1), "{scripts}: merged a vision document the Ground doesn't hold");
        assert!(line.starts_with("FAIL") && line.contains("vision document"), "{scripts}: {line}");
    }
}

/// main may be ahead of the verified head in .md files under .claude/skills/ that the branch doesn't
/// touch, as in docs/: they can't change what builds or runs. Not in a templates/ directory (those are the
/// scripts and files a kickoff copies into a repo), not in other files, and not in a file the branch changed.
#[test]
fn merge_lets_main_be_ahead_only_in_skill_docs_the_branch_leaves_alone() {
    let write = |path: &'static str, text: &'static str| {
        move |dir: &Path| {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    };
    let code = write("code.txt", "a change\n");
    for scripts in SCRIPTS {
        for (main_file, branch_also, ok) in [
            (".claude/skills/kick/process.md", None, true),
            (".claude/skills/kick/deep/notes.md", None, true),
            ("docs/guide.md", None, true),
            (".claude/skills/kick/templates/AGENTS.md", None, false),
            (".claude/skills/kick/templates/merge", None, false),
            (".claude/skills/templates/x.md", None, false),
            (".claude/skills/kick/run.sh", None, false),
            (".claude/skills/kick/process.md", Some(".claude/skills/kick/process.md"), false),
        ] {
            let out = merge_dry_run(
                scripts,
                |dir| {
                    code(dir);
                    if let Some(f) = branch_also {
                        write(f, "the branch's version\n")(dir);
                    }
                },
                |dir| {
                    let file = dir.join(main_file);
                    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                    std::fs::write(file, "main's version\n").unwrap();
                },
            );
            let line = check_line(&out, "main");
            let case = format!("{scripts}: main ahead in {main_file}, branch also {branch_also:?}");
            assert_eq!(line.starts_with("ok "), ok, "{case}: {line}");
            assert_eq!(out.status.success(), ok, "{case}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

/// Stages `contents` at `path` with `mode`, as git's index takes it, without the working tree: for a
/// path in a case a case-insensitive filesystem would fold, or a symlink.
fn stage(dir: &Path, mode: &str, path: &str, contents: &str) {
    let blob = dir.join("../blob");
    std::fs::write(&blob, contents).unwrap();
    let oid = run(dir, "git", &["hash-object", "-w", blob.to_str().unwrap()]).stdout;
    let entry = format!("{mode},{},{path}", String::from_utf8(oid).unwrap().trim());
    let out = run(dir, "git", &["update-index", "--add", "--cacheinfo", &entry]);
    assert!(out.status.success(), "staging {path}: {}", String::from_utf8_lossy(&out.stderr));
}

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

/// ground-checkpoint runs ground-guards on what origin's main gained before anything is pulled: it names a
/// clean commit as checked, and stops on one that edits a committed Ground object. Offline, it still commits.
#[test]
fn ground_checkpoint_runs_the_ground_guards_on_what_it_would_pull() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (checkpoint, guards) = (file("ground-checkpoint"), file("ground-guards"));
        let files = [("scripts/ground-checkpoint", &checkpoint), ("scripts/ground-guards", &guards)];
        let (tmp, dir) = repo(&files.map(|(to, from)| (to, from.as_path())), ".bracken/identities/\n", |_| {});
        let git = |args: &[&str]| {
            let out = run(&dir, "git", args);
            assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        bk(&dir, &["init", "--name", "checkpoint"]);
        bk(&dir, &["id", "new", "val"]);
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "ground"]);
        let forge = tmp.path().join("forge.git");
        git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
        git(&["remote", "add", "origin", forge.to_str().unwrap()]);
        git(&["push", "-q", "origin", "main"]);
        // Origin's main gains a commit made elsewhere: `change` on a side branch, pushed as main.
        let push_ahead = |change: &dyn Fn(&Path)| {
            git(&["switch", "-qc", "side", "origin/main"]);
            change(&dir);
            git(&["add", "-A"]);
            git(&["commit", "-qm", "elsewhere"]);
            let sha = git(&["rev-parse", "HEAD"]);
            git(&["push", "-q", "origin", "side:main"]);
            git(&["switch", "-q", "main"]);
            git(&["branch", "-qD", "side"]);
            sha
        };
        let case = |what: &str| format!("{scripts}, {what}");

        let clean = push_ahead(&|dir| std::fs::write(dir.join("code.txt"), "a change\n").unwrap());
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}: {stdout}{}", case("clean"), String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains(&format!("git merge --no-edit {clean}")), "{}: {stdout}", case("clean"));
        git(&["merge", "-q", "--no-edit", &clean]);

        push_ahead(&|dir| {
            let objects = dir.join(".bracken/objects");
            let shard = std::fs::read_dir(&objects).unwrap().next().unwrap().unwrap().path();
            let file = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
            let mut bytes = std::fs::read(&file).unwrap();
            bytes.push(b'x');
            std::fs::write(&file, bytes).unwrap();
        });
        let before = git(&["rev-parse", "HEAD"]);
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}: {stderr}", case("edited object"));
        assert!(
            stderr.contains("Ground is append-only") && stderr.contains("fails the Ground guards"),
            "{}: {stderr}",
            case("edited object")
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("git merge"), "{}", case("edited object"));
        assert_eq!(git(&["rev-parse", "HEAD"]), before, "{}: a commit was made", case("edited object"));

        // With origin out of reach, the local Ground's new files are still committed, and nothing is named to pull.
        git(&["remote", "set-url", "origin", tmp.path().join("gone.git").to_str().unwrap()]);
        bk(&dir, &["claim", "offline", "--tag", "note", "--as", "val"]);
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{}: {stdout}{stderr}", case("offline"));
        assert!(stderr.contains("checked nothing there"), "{}: {stderr}", case("offline"));
        assert!(stdout.contains("committed") && !stdout.contains("git merge"), "{}: {stdout}", case("offline"));
        assert_ne!(git(&["rev-parse", "HEAD"]), before, "{}: nothing was committed", case("offline"));
    }
}
