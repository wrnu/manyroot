//! scripts/cleanup. Each test runs the script and its kickoff-template copy.

use crate::support::*;

/// Sets the modification time of `path` and everything under it, not following symlinks.
fn set_mtimes(path: &Path, when: std::time::SystemTime) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            set_mtimes(&entry.unwrap().path(), when);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(when).unwrap();
}

/// A `sleep 3600` with its working directory in `dir`, so lsof shows a process using it. It outlasts any test
/// however loaded the machine (a 467 s run outlived a `sleep 120`: evidence 16972bc5b1d9), and is killed and
/// waited for when dropped, so a failing assert never leaves it running (finding 1cc1ba335e87).
struct Sleeper(std::process::Child);

impl Sleeper {
    fn in_dir(dir: &Path) -> Self {
        Sleeper(Command::new("sleep").arg("3600").current_dir(dir).spawn().unwrap())
    }

    fn id(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// cleanup (owner process 11f1ac552f26) on stubbed worktrees, with HOME and TMPDIR throwaway directories
/// and a stub cargo that logs any call. Of this repo's worktrees, it removes only the clean, unused ones
/// idle 2 hours under T3's root (t3-<id>) or TMPDIR. A builder's worktree whose branch is squash-merged
/// into origin/main loses only its target/ while origin still has the branch; once origin hasn't, the
/// worktree and its branch go (task 268f5a72c4f2), also when main has since changed the lines it changed,
/// so that merging it conflicts but its patch reverse-applies where it landed; but only if its tip is the
/// head a current pr claim names. The reviewer's repro (finding e2741e2be8cc) stays: a local commit after
/// the squash merge that reverts part of the branch, which its net patch can't show. So does a branch no
/// pr claim names. One holding ignored files besides target/ stays, as does anything named pg-*
/// (PineGrove's). It clears only idle scratch entries. A dirty, in-use, young, unmerged,
/// key-holding or outside worktree stays, each with its reason. The dry run changes nothing, and both runs
/// end with a summary and exit 0. Each run reads the pr claims once, however many branches it judges by
/// them (finding 36e8f80a34ab): a stub jq counts the reads.
#[test]
fn cleanup_removes_only_clean_unused_idle_checkouts_in_its_places_and_the_dry_run_changes_nothing() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n*.log\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
            std::fs::write(dir.join("shared.txt"), "a\nb\nc\n").unwrap();
            std::fs::write(dir.join("f.rs"), "one();\ntwo();\n").unwrap();
            std::fs::write(dir.join("g.txt"), "x\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);

        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let t3 = home.join(".t3/worktrees/repo");
        let scratch = tmpdir.join("bracken-scratch");
        std::fs::create_dir_all(home.join("bracken-runs/run1")).unwrap();
        let add = |path: &Path, branch: Option<&str>| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let path = path.to_str().unwrap();
            match branch {
                Some(b) => git(&dir, &["worktree", "add", "-q", "-b", b, path]),
                None => git(&dir, &["worktree", "add", "-q", "--detach", path]),
            }
        };
        let commit = |path: &Path, file: &str| {
            std::fs::write(path.join(file), file).unwrap();
            git(path, &["add", file]);
            git(path, &["commit", "-qm", file]);
        };
        for name in ["t3-clean", "t3-dirty", "t3-busy", "t3-key"] {
            add(&t3.join(name), None);
        }
        std::fs::write(t3.join("t3-dirty/wip"), "work").unwrap();
        std::fs::create_dir_all(t3.join("t3-key/.bracken/identities")).unwrap();
        std::fs::write(t3.join("t3-key/.bracken/identities/k"), "key").unwrap();
        for branch in ["merged", "unmerged", "linked", "pushed", "junk", "unclaimed"] {
            add(&t3.join(branch), Some(branch));
            commit(&t3.join(branch), branch);
        }
        // The reviewer's repro: "reverted" changes two lines of f.rs and appends to g.txt.
        add(&t3.join("reverted"), Some("reverted"));
        std::fs::write(t3.join("reverted/f.rs"), "one();\ndbg();\ntwo();\nkeep();\n").unwrap();
        std::fs::write(t3.join("reverted/g.txt"), "x\ny\n").unwrap();
        git(&t3.join("reverted"), &["commit", "-qam", "c1"]);
        // "later" changes a line of a file main had, which main changes again after the squash merge.
        add(&t3.join("later"), Some("later"));
        std::fs::write(t3.join("later/shared.txt"), "a\nB\nc\n").unwrap();
        git(&t3.join("later"), &["commit", "-qam", "later"]);
        // origin keeps "pushed" after the merge.
        git(&t3.join("pushed"), &["push", "-q", "origin", "pushed"]);
        std::fs::write(t3.join("junk/notes.log"), "ignored, not target/").unwrap();
        for branch in ["merged", "unmerged", "later", "pushed"] {
            std::fs::create_dir_all(t3.join(branch).join("target/debug")).unwrap();
            std::fs::write(t3.join(branch).join("target/debug/big"), "build").unwrap();
        }
        // A shared build dir behind a symlinked target/: never followed (finding a3cb4a1ac00d).
        let shared = t.join("shared-target");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("other-build"), "build").unwrap();
        std::os::unix::fs::symlink(&shared, t3.join("linked/target")).unwrap();
        // The forge squash-merges "merged" and "linked": main gets their changes, not their commits.
        for branch in ["merged", "linked", "later", "pushed", "junk", "unclaimed", "reverted"] {
            git(&dir, &["merge", "-q", "--squash", branch]);
            git(&dir, &["commit", "-qm", &format!("{branch} (#1)")]);
        }
        // Each pushed and claimed head, as a builder's pr claim names it; "unclaimed" has none.
        bk(&dir, &["init", "--name", "cleanup"]);
        bk(&dir, &["id", "new", "builder"]);
        for (n, branch) in ["merged", "linked", "later", "pushed", "junk", "reverted"].iter().enumerate() {
            let head = String::from_utf8(run(&dir, "git", &["rev-parse", branch]).stdout).unwrap();
            let claim = format!("PR #{} {branch} @ {}: {branch}", n + 1, head.trim());
            bk(&dir, &["claim", &claim, "--tag", "pr", "--as", "builder"]);
        }
        // Then, unpushed, "reverted" drops dbg() and reverts g.txt: its net patch now shows only keep(),
        // which main has, so its content alone would call it merged.
        std::fs::write(t3.join("reverted/f.rs"), "one();\ntwo();\nkeep();\n").unwrap();
        std::fs::write(t3.join("reverted/g.txt"), "x\n").unwrap();
        git(&t3.join("reverted"), &["commit", "-qam", "c2"]);
        let c2 = String::from_utf8(run(&dir, "git", &["rev-parse", "reverted"]).stdout).unwrap();
        std::fs::write(dir.join("shared.txt"), "a\nB2\nc\n").unwrap();
        git(&dir, &["commit", "-qam", "main changes the line again"]);
        let conflict = run(&dir, "git", &["merge-tree", "--write-tree", "main", "later"]);
        assert!(!conflict.status.success(), "{scripts}: merging later should conflict");
        git(&dir, &["push", "-q", "origin", "main"]);
        add(&scratch.join("task1/tree"), None);
        add(&t.join("elsewhere/tree"), None);
        // In use, with a backslash in its name, which awk -v would have unescaped (finding 786ebd4a2970).
        add(&tmpdir.join("b\\s"), None);
        // On origin only: the dry run mustn't fetch it (finding 41df1ea1240c).
        git(&origin, &["branch", "extra", "main"]);
        std::fs::create_dir_all(scratch.join("old/deep")).unwrap();
        std::fs::write(scratch.join("old/deep/file"), "x").unwrap();
        // Another project's, though idle in the scratch root, and a worktree named like one.
        std::fs::create_dir_all(scratch.join("pg-old")).unwrap();
        std::fs::write(scratch.join("pg-old/file"), "x").unwrap();
        add(&t3.join("pg-tree"), None);

        // Everything so far was last used 3 hours ago; what follows is new.
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600));
        add(&t3.join("t3-young"), None);
        std::fs::create_dir_all(scratch.join("new")).unwrap();
        std::fs::write(scratch.join("new/file"), "x").unwrap();

        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("cargo"), "#!/bin/sh\necho \"$PWD|$*\" >>\"$LOG\"\n");
        // Never the machine's Docker.
        write_executable(&stubs.join("docker"), "#!/bin/sh\nexit 1\n");
        let log = t.join("cargo.log");
        let jq = String::from_utf8(run(&dir, "sh", &["-c", "command -v jq"]).stdout).unwrap();
        let jq_log = t.join("jq.log");
        let stub = format!("#!/bin/sh\necho read >>'{}'\nexec '{}' \"$@\"\n", jq_log.display(), jq.trim());
        write_executable(&stubs.join("jq"), &stub);
        let reads = || std::fs::read_to_string(&jq_log).unwrap_or_default().lines().count();
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let busy = t3.join("t3-busy");
        let sleeper = Sleeper::in_dir(&busy);
        let slashed = Sleeper::in_dir(&tmpdir.join("b\\s"));
        let refs = || String::from_utf8(run(&dir, "git", &["for-each-ref"]).stdout).unwrap();
        let refs_before = refs();
        let cleanup = |args: &[&str]| {
            let out = command(&dir, "scripts/cleanup", args)
                .env("HOME", &home)
                .env("TMPDIR", &tmpdir)
                .env("PATH", &path)
                .env("LOG", &log)
                // The idle targets of worktrees it keeps are another test's (task cb478ca0bd41).
                .env("CLEANUP_TARGET_IDLE_HOURS", "24")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            assert!(out.status.success(), "{scripts}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
            stdout
        };
        let line = |out: &str, path: &str| {
            out.lines()
                .find(|l| l.contains(&format!("{path}:")))
                .unwrap_or_else(|| panic!("{scripts}: no line for {path}: {out}"))
                .to_owned()
        };
        let worktrees = || String::from_utf8(run(&dir, "git", &["worktree", "list"]).stdout).unwrap();
        let before = worktrees();

        let dry = cleanup(&[]);
        assert_eq!(reads(), 1, "{scripts}: the dry run read the pr claims more than once: {dry}");
        let expect = [
            ("/t3-clean", "remove"),
            ("/task1/tree", "remove"),
            ("/merged", "remove"),
            ("/later", "remove"),
            ("/pushed", "clean"),
            ("/junk", "won't touch"),
            ("/reverted", "keep"),
            ("/unclaimed", "keep"),
            ("bracken-scratch/pg-old", "won't touch"),
            ("/pg-tree", "won't touch"),
            ("/linked", "won't touch"),
            ("bracken-scratch/old", "clear"),
            ("/t3-dirty", "won't touch"),
            ("/t3-key", "won't touch"),
            ("elsewhere/tree", "won't touch"),
            ("/t3-busy", "keep"),
            ("b\\s", "keep"),
            ("/t3-young", "keep"),
            ("/unmerged", "keep"),
            ("bracken-scratch/new", "keep"),
        ];
        for (path, verb) in expect {
            assert!(line(&dry, path).starts_with(verb), "{scripts}: {path} should be {verb}: {dry}");
        }
        assert!(line(&dry, "/t3-dirty").contains("dirty"), "{scripts}: {dry}");
        assert!(line(&dry, "/t3-busy").contains("in use by pid"), "{scripts}: {dry}");
        assert!(line(&dry, "/unmerged").contains("isn't merged"), "{scripts}: {dry}");
        assert!(line(&dry, "elsewhere/tree").contains("outside"), "{scripts}: {dry}");
        assert!(
            line(&dry, "/merged").contains("worktree and branch merged") && line(&dry, "/merged").contains("(merged)")
        );
        let squash = ["log", "--format=%h", "--fixed-strings", "--grep=later (#1)", "main"];
        let landed = String::from_utf8(run(&dir, "git", &squash).stdout).unwrap();
        assert!(line(&dry, "/later").contains(&format!("(in {})", landed.trim())), "{scripts}: {dry}");
        assert!(line(&dry, "/junk").contains("ignored files besides target/: notes.log"), "{scripts}: {dry}");
        assert!(
            line(&dry, "/reverted").contains("not its pr claim's head")
                && line(&dry, "/reverted").contains("moved since")
        );
        assert!(line(&dry, "/unclaimed").contains("no current pr claim names it"), "{scripts}: {dry}");
        assert!(dry.lines().last().unwrap().contains("6 to do with --apply"), "{scripts}: {dry}");
        assert_eq!(worktrees(), before, "{scripts}: the dry run changed the worktrees");
        assert!(!log.exists(), "{scripts}: the dry run ran cargo");
        assert_eq!(refs(), refs_before, "{scripts}: the dry run changed the refs");
        assert!(line(&dry, "b\\s").contains("in use by pid"), "{scripts}: {dry}");
        assert!(scratch.join("old/deep/file").exists() && t3.join("t3-clean").exists(), "{scripts}");

        let applied = cleanup(&["--apply"]);
        assert_eq!(reads(), 2, "{scripts}: --apply read the pr claims more than once: {applied}");
        drop((sleeper, slashed));
        assert!(refs().contains("refs/remotes/origin/extra"), "{scripts}: --apply didn't fetch");
        assert!(applied.lines().last().unwrap().contains("6 done"), "{scripts}: {applied}");
        let after = worktrees();
        for gone in ["t3-clean", "task1/tree", "/merged", "/later"] {
            assert!(!after.contains(gone), "{scripts}: {gone} is still a worktree: {after}");
        }
        let branches = String::from_utf8(run(&dir, "git", &["branch", "--format=%(refname:short)"]).stdout).unwrap();
        let branch_states = [
            ("merged", false),
            ("later", false),
            ("pushed", true),
            ("junk", true),
            ("unmerged", true),
            ("reverted", true),
            ("unclaimed", true),
        ];
        for (branch, stays) in branch_states {
            assert_eq!(branches.lines().any(|b| b == branch), stays, "{scripts}: branch {branch}: {branches}");
        }
        assert!(line(&applied, "/later").contains("worktree and branch later"), "{scripts}: {applied}");
        assert!(applied.lines().any(|l| l.starts_with("removed") && l.contains("/merged: and branch merged")));
        assert!(!t3.join("merged").exists() && !t3.join("later").exists(), "{scripts}");
        assert!(!t3.join("pushed/target").exists() && t3.join("pushed/pushed").exists(), "{scripts}: {applied}");
        assert!(t3.join("junk/notes.log").exists(), "{scripts}: {applied}");
        let still = String::from_utf8(run(&dir, "git", &["rev-parse", "reverted"]).stdout).unwrap();
        assert_eq!(still, c2, "{scripts}: the unpushed commit was lost: {applied}");
        assert!(t3.join("reverted/f.rs").exists() && t3.join("unclaimed").exists(), "{scripts}: {applied}");
        assert!(scratch.join("pg-old/file").exists() && t3.join("pg-tree").exists(), "{scripts}: {applied}");
        assert!(line(&applied, "/pg-tree").contains("named pg-*"), "{scripts}: {applied}");
        assert!(!t3.join("t3-clean").exists() && !scratch.join("old").exists(), "{scripts}: {applied}");
        for stays in
            ["t3-dirty", "t3-busy", "t3-key", "t3-young", "/pushed", "/junk", "unmerged", "/linked", "elsewhere/tree"]
        {
            assert!(after.contains(stays), "{scripts}: {stays} was removed: {applied}");
        }
        // git worktree list quotes a backslash, so look for the directory itself.
        assert!(line(&applied, "b\\s").starts_with("keep") && tmpdir.join("b\\s/Cargo.toml").exists(), "{scripts}");
        assert!(t3.join("t3-dirty/wip").exists() && t3.join("t3-key/.bracken/identities/k").exists());
        assert!(scratch.join("new/file").exists() && home.join("bracken-runs/run1").exists(), "{scripts}");
        // target/ is removed directly, never through cargo, and a symlinked one isn't followed.
        assert!(!log.exists(), "{scripts}: cargo ran: {applied}");
        assert!(t3.join("unmerged/target/debug/big").exists(), "{scripts}");
        assert!(line(&applied, "/linked").contains("symlink") && shared.join("other-build").exists(), "{scripts}");
    }
}

/// Finding 740976a60693: a copy of cleanup outside any git checkout, run from inside another repo, refuses
/// at once instead of taking the caller's directory for its main checkout.
#[test]
fn cleanup_outside_a_checkout_refuses_rather_than_cleaning_the_callers_repo() {
    for scripts in SCRIPTS {
        let (tmp, dir) = repo(&[], ".bracken/\n", |_| {});
        let loose = tmp.path().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        copy_executable(&repo_root().join(scripts).join("cleanup"), &loose.join("cleanup"));
        let out = command(&dir, loose.join("cleanup").to_str().unwrap(), &[])
            .env("GIT_CEILING_DIRECTORIES", tmp.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(out.stdout.is_empty() && stderr.contains("isn't in a git checkout"), "{scripts}: {stderr}");
    }
}

/// Task cb478ca0bd41: cleanup frees the target/ of a builder's worktree it keeps, whatever its branch's
/// state (unmerged, dirty, on no branch), when nothing has the worktree open and nothing in target/ changed
/// for the target idle time (3 h, CLEANUP_TARGET_IDLE_HOURS, --target-idle). Its sources stay. A busy
/// worktree, a target/ changed since, and a symlinked one stay. In the scratch root, a cargo target dir
/// (cargo's CACHEDIR.TAG) in an entry it keeps goes on the same rule, beside a checkout or newer files; a
/// directory with another program's CACHEDIR.TAG stays, as do target dirs in a pg-* directory or in another
/// repository's checkout at any depth (finding 2b3fe543e130). Just before each removal it looks again: a
/// process that starts using a worktree after the run's first look (here, while cleanup sizes its target/)
/// keeps it (finding 176efeffc76f). The summary reports Docker's reclaimable build cache, and cleanup never
/// asks Docker to prune.
#[test]
fn cleanup_frees_idle_targets_of_worktrees_it_keeps_and_reports_dockers_build_cache() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let (t3, scratch) = (home.join(".t3/worktrees/repo"), tmpdir.join("bracken-scratch"));
        let build = |target: &Path| {
            std::fs::create_dir_all(target.join("debug")).unwrap();
            std::fs::write(target.join("debug/big"), "build").unwrap();
        };
        let cargo_tag =
            "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

        // Builders' worktrees, none merged: one on a branch with a commit of its own, a dirty one, a busy
        // one, one whose target/ changes later, one with a symlinked target/, one on no branch, and one a
        // process starts using after cleanup's first look.
        for name in ["idle", "dirty", "busy", "fresh", "linked", "detached", "late"] {
            let path = t3.join(name);
            std::fs::create_dir_all(&t3).unwrap();
            if name == "detached" {
                git(&dir, &["worktree", "add", "-q", "--detach", path.to_str().unwrap()]);
            } else {
                git(&dir, &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
                std::fs::write(path.join(name), name).unwrap();
                git(&path, &["add", name]);
                git(&path, &["commit", "-qm", name]);
            }
            if name != "linked" {
                build(&path.join("target"));
            }
        }
        std::fs::write(t3.join("dirty/wip"), "work").unwrap();
        let shared = t.join("shared-target");
        build(&shared);
        std::os::unix::fs::symlink(&shared, t3.join("linked/target")).unwrap();
        // Scratch: a dirty checkout with a cargo target dir beside it, and an entry holding a cargo target
        // dir and another program's cache dir beside a file that changes later.
        std::fs::create_dir_all(scratch.join("task")).unwrap();
        git(&dir, &["worktree", "add", "-q", "--detach", scratch.join("task/tree").to_str().unwrap()]);
        // Dirty, so the checkout itself stays.
        std::fs::write(scratch.join("task/tree/wip"), "work").unwrap();
        let other_tag = "Signature: 8a477f597d28d172789f06886806bc55\n# made by another tool\n";
        // "deep" holds target dirs under a pg-* directory and in another repository's checkout, which stay,
        // and one in plain directories, which goes.
        std::fs::create_dir_all(scratch.join("deep/a")).unwrap();
        git(t, &["init", "-q", scratch.join("deep/a/clone").to_str().unwrap()]);
        git(&scratch.join("deep/a/clone"), &["remote", "add", "origin", "https://example.invalid/other.git"]);
        for (dir, tag) in [
            ("task/tgt", cargo_tag),
            ("notes/tgt", cargo_tag),
            ("notes/other", other_tag),
            ("deep/pg-x/b/tgt", cargo_tag),
            ("deep/a/clone/target", cargo_tag),
            ("deep/a/plain/tgt", cargo_tag),
        ] {
            build(&scratch.join(dir));
            std::fs::write(scratch.join(dir).join("CACHEDIR.TAG"), tag).unwrap();
        }
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 3600));
        std::fs::write(t3.join("fresh/target/debug/new"), "just built").unwrap();
        std::fs::write(scratch.join("notes/today"), "new").unwrap();

        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        let docker_log = t.join("docker.log");
        let docker = format!(
            "#!/bin/sh\necho \"$*\" >>'{}'\nprintf 'Images|1GB (10%%)\\nBuild Cache|2.5GB (40%%)\\n'\n",
            docker_log.display()
        );
        write_executable(&stubs.join("docker"), &docker);
        // A du that, sizing late/target with LATE set, first starts a process with its working directory in
        // that worktree: it starts after cleanup's first look, and before its removal.
        let du = String::from_utf8(run(&dir, "sh", &["-c", "command -v du"]).stdout).unwrap();
        let late_pid = t.join("late.pid");
        let stub = format!(
            "#!/bin/sh\ncase \"$*\" in *\"/late/target\"*) if [ -n \"$LATE\" ] && [ ! -e '{pid}' ]; then \
             (cd \"$LATE\" && exec sleep 120) >/dev/null 2>&1 </dev/null & echo $! >'{pid}'; sleep 1; fi ;; esac\n\
             exec '{du}' \"$@\"\n",
            pid = late_pid.display(),
            du = du.trim()
        );
        write_executable(&stubs.join("du"), &stub);
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let sleeper = Sleeper::in_dir(&t3.join("busy"));
        let cleanup = |args: &[&str], hours: Option<&str>| {
            let mut command = command(&dir, "scripts/cleanup", args);
            command.env("HOME", &home).env("TMPDIR", &tmpdir).env("PATH", &path).env_remove("LATE");
            if args.contains(&"--apply") {
                command.env("LATE", t3.join("late"));
            }
            match hours {
                Some(h) => command.env("CLEANUP_TARGET_IDLE_HOURS", h),
                None => command.env_remove("CLEANUP_TARGET_IDLE_HOURS"),
            };
            let out = command.output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            (out.status.code(), stdout + &String::from_utf8_lossy(&out.stderr))
        };
        let line = |out: &str, verb: &str, path: &str| {
            out.lines().find(|l| l.starts_with(verb) && l.contains(&format!("{path}:"))).map(str::to_owned)
        };

        let (code, dry) = cleanup(&[], None);
        assert_eq!(code, Some(0), "{scripts}: {dry}");
        for (path, why) in [
            ("/idle", "isn't merged"),
            ("/dirty", "dirty"),
            ("/detached", "on no branch"),
            ("/late", "isn't merged"),
            ("/task", "a cargo target dir"),
            ("/notes", "a cargo target dir"),
            ("/deep", "a cargo target dir"),
        ] {
            let l = line(&dry, "clean", path).unwrap_or_else(|| panic!("{scripts}: {path} isn't cleaned: {dry}"));
            assert!(l.contains(why) && l.contains(" GB)"), "{scripts}: {l}");
        }
        assert!(line(&dry, "clean", "/idle").unwrap().contains("remove target/: "), "{scripts}: {dry}");
        assert!(line(&dry, "clean", "/task").unwrap().contains("remove tgt/: "), "{scripts}: {dry}");
        assert!(line(&dry, "keep", "/busy").is_some_and(|l| l.contains("in use by pid")), "{scripts}: {dry}");
        assert!(line(&dry, "keep", "/fresh").is_some(), "{scripts}: {dry}");
        assert!(line(&dry, "keep", "/linked").is_some(), "{scripts}: {dry}");
        assert!(!dry.contains("remove other/"), "{scripts}: {dry}");
        assert!(line(&dry, "clean", "/deep").unwrap().contains("remove a/plain/tgt/: "), "{scripts}: {dry}");
        assert!(!dry.contains("pg-x/b/tgt") && !dry.contains("clone/target"), "{scripts}: {dry}");
        let last = dry.lines().last().unwrap();
        assert!(last.contains("7 to do") && last.contains("Docker build cache: 2.5GB (40%) reclaimable"), "{last}");
        assert!(t3.join("idle/target/debug/big").exists(), "{scripts}: the dry run removed something");

        // A longer idle time, from the environment or the flag, keeps them all; a bad one is refused.
        let (_, longer) = cleanup(&[], Some("5"));
        assert!(longer.lines().last().unwrap().contains("nothing to do"), "{scripts}: {longer}");
        let (_, flagged) = cleanup(&["--target-idle", "5"], None);
        assert!(flagged.lines().last().unwrap().contains("nothing to do"), "{scripts}: {flagged}");
        let (code, refused) = cleanup(&["--target-idle", "0"], None);
        assert!(code == Some(2) && refused.contains("1 to 9999 hours"), "{scripts}: {refused}");

        let (code, applied) = cleanup(&["--apply"], None);
        drop(sleeper);
        let late = std::fs::read_to_string(&late_pid).unwrap_or_default();
        let _ = Command::new("kill").arg(late.trim()).status();
        let _ = std::fs::remove_file(&late_pid);
        assert_eq!(code, Some(0), "{scripts}: {applied}");
        assert!(applied.lines().last().unwrap().contains("6 done"), "{scripts}: {applied}");
        // The late process kept late/target/, found by the look just before removing it.
        let l = line(&applied, "keep", "/late").unwrap_or_else(|| panic!("{scripts}: {applied}"));
        assert!(l.contains("now in use by pid"), "{scripts}: {l}");
        assert!(t3.join("late/target/debug/big").exists(), "{scripts}: {applied}");
        assert!(!applied.contains(".cleanup-"), "{scripts}: a renamed copy was left: {applied}");
        assert!(!scratch.join("deep/a/plain/tgt").exists(), "{scripts}: {applied}");
        for stays in ["deep/pg-x/b/tgt/debug/big", "deep/a/clone/target/debug/big"] {
            assert!(scratch.join(stays).exists(), "{scripts}: {stays} went: {applied}");
        }
        for gone in ["idle/target", "dirty/target", "detached/target"] {
            assert!(!t3.join(gone).exists(), "{scripts}: {gone} is still there: {applied}");
        }
        assert!(!scratch.join("task/tgt").exists() && !scratch.join("notes/tgt").exists(), "{scripts}: {applied}");
        for stays in ["idle/idle", "dirty/wip", "busy/target/debug/big", "fresh/target/debug/big"] {
            assert!(t3.join(stays).exists(), "{scripts}: {stays} went: {applied}");
        }
        assert!(shared.join("debug/big").exists() && t3.join("linked").exists(), "{scripts}: {applied}");
        assert!(scratch.join("task/tree/Cargo.toml").exists() && scratch.join("notes/today").exists());
        assert!(scratch.join("notes/other/debug/big").exists(), "{scripts}: another program's cache dir went");
        let asked = std::fs::read_to_string(&docker_log).unwrap();
        assert!(asked.lines().all(|l| l.starts_with("system df")), "{scripts}: docker was asked {asked}");
    }
}

/// Finding 176efeffc76f: without lsof, cleanup removes no target dir, rather than judge use from the /proc
/// fallback (which misses a program run from a target dir) or from nothing (macOS has no /proc). An
/// unmerged worktree's idle target/ stays, saying why, as does an idle scratch entry (task 02acb1d6e7e0).
#[test]
fn cleanup_without_lsof_removes_no_target_dir() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let wt = home.join(".t3/worktrees/repo/idle");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        git(&dir, &["worktree", "add", "-q", "-b", "idle", wt.to_str().unwrap()]);
        std::fs::write(wt.join("f"), "f").unwrap();
        git(&wt, &["add", "f"]);
        git(&wt, &["commit", "-qm", "f"]);
        std::fs::create_dir_all(wt.join("target/debug")).unwrap();
        std::fs::write(wt.join("target/debug/big"), "build").unwrap();
        std::fs::create_dir_all(tmpdir.join("bracken-scratch/old")).unwrap();
        std::fs::write(tmpdir.join("bracken-scratch/old/notes"), "notes").unwrap();
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 3600));
        // PATH without lsof, and with a docker that isn't the machine's. Each directory holding lsof is
        // replaced by a copy of links to everything else in it: on Linux lsof is in /usr/bin, beside git.
        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("docker"), "#!/bin/sh\nexit 1\n");
        let mut dirs = vec![stubs.clone()];
        for (i, d) in std::env::split_paths(&std::env::var_os("PATH").unwrap()).enumerate() {
            if !d.join("lsof").exists() {
                dirs.push(d);
                continue;
            }
            let shadow = t.join(format!("path{i}"));
            std::fs::create_dir_all(&shadow).unwrap();
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                if entry.file_name() != "lsof" {
                    let _ = std::os::unix::fs::symlink(entry.path(), shadow.join(entry.file_name()));
                }
            }
            dirs.push(shadow);
        }
        let path = std::env::join_paths(dirs).unwrap();
        let found = Command::new("sh").args(["-c", "command -v lsof"]).env("PATH", &path).output().unwrap();
        assert!(!found.status.success(), "{scripts}: lsof is still on the test's PATH");
        let out = command(&dir, "scripts/cleanup", &["--apply"])
            .env("HOME", &home)
            .env("TMPDIR", &tmpdir)
            .env("PATH", &path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{scripts}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
        assert!(wt.join("target/debug/big").exists(), "{scripts}: target/ went without lsof: {stdout}");
        let l = stdout.lines().find(|l| l.contains("/idle:")).unwrap_or_else(|| panic!("{scripts}: {stdout}"));
        assert!(l.starts_with("keep") && l.contains("lsof"), "{scripts}: {l}");
        let l = stdout.lines().find(|l| l.contains("/old:")).unwrap_or_else(|| panic!("{scripts}: {stdout}"));
        // Without lsof, macOS's first look gives nothing and Linux's /proc look can't clear it: kept either way.
        assert!(l.starts_with("keep") && l.contains("lsof"), "{scripts}: {l}");
        assert!(tmpdir.join("bracken-scratch/old/notes").exists(), "{scripts}: {stdout}");
    }
}

/// Task 02acb1d6e7e0 and PR #224's review: cleanup keeps a scratch entry a builder is still using.
/// - Cleared: an idle entry, and those whose .keep is a day old, dated in the future, a symlink or a
///   directory (finding f53cfe828590).
/// - Kept: one a process starts using after the run's first look (while cleanup sizes it), found by the look
///   just before its removal; one a process starts using once it's renamed aside, put back; one only read
///   for a while whose .keep was touched in the last 24 hours, whole, its idle cargo target dir included; a
///   worktree inside such an entry; and a clean scratch worktree a process starts using while cleanup sizes
///   it, found by the look after sizing (finding 075b4a4dbcfd).
/// - Put back, or kept, because of what happened while cleanup sized them, by commands that had exited by
///   its next look (finding 497ffa3fdcdc): an entry whose .keep was touched, one that was written to, one that
///   gained a git checkout, a scratch worktree that gained a commit, and a cargo target dir in an entry whose
///   .keep was touched (finding 1efc75ddf0cb).
/// - A .keep that stat can't date counts as fresh (finding df3b4dc4966f), with a stat stub: an entry whose
///   day-old .keep can't be dated is kept whole, by the check before sizing; and a cargo target dir is put
///   back when its entry's day-old .keep can't be dated once it's sized, by the check after the rename.
/// - Entries left renamed aside as <entry>.cleanup-<pid> (finding 910ae8ed0fe3): put back when that run is
///   over and the name is free, and not cleared in the same run; kept when the name exists again, or while
///   the run still runs.
/// - A run stopped by TERM while it has an entry aside puts it back.
#[test]
fn cleanup_keeps_scratch_entries_in_use_or_kept_and_clears_the_idle_ones() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        std::fs::create_dir_all(&home).unwrap();
        let scratch = tmpdir.join("bracken-scratch");
        // A pid no process has: a child that has been waited for.
        let mut gone = Command::new("true").spawn().unwrap();
        gone.wait().unwrap();
        let (dead, alive) = (gone.id(), std::process::id());
        let entries = [
            "idle", "stale", "future", "linkkeep", "dirkeep", "late", "moved", "read", "dup", "kp", "wr", "gt", "kt",
            "kn", "ks",
        ];
        let leftovers =
            [format!("orphan.cleanup-{dead}"), format!("dup.cleanup-{dead}"), format!("busy.cleanup-{alive}")];
        for name in entries.iter().map(|s| s.to_string()).chain(leftovers.iter().cloned()) {
            std::fs::create_dir_all(scratch.join(&name).join("notes")).unwrap();
            std::fs::write(scratch.join(&name).join("notes/log"), &name).unwrap();
        }
        // Idle cargo target dirs: in the entry kept by its .keep, and in one kept as changed just now.
        for entry in ["read", "kt", "ks"] {
            std::fs::create_dir_all(scratch.join(entry).join("tgt/debug")).unwrap();
            std::fs::write(scratch.join(entry).join("tgt/debug/big"), "build").unwrap();
            std::fs::write(
                scratch.join(entry).join("tgt/CACHEDIR.TAG"),
                "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n",
            )
            .unwrap();
        }
        // Clean scratch checkouts of this repo: one in an entry its builder only reads, one in use soon.
        for wt in ["task/wt", "sized/wt", "commit/wt"] {
            let wt = scratch.join(wt);
            std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
            git(&dir, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]);
        }
        let now = std::time::SystemTime::now();
        let hours = |h: i64| {
            if h >= 0 {
                now - std::time::Duration::from_secs(h as u64 * 3600)
            } else {
                now + std::time::Duration::from_secs((-h) as u64 * 3600)
            }
        };
        for (entry, h) in [("read", 3), ("task", 3), ("stale", 30), ("future", -48), ("kn", 30), ("ks", 30)] {
            std::fs::write(scratch.join(entry).join(".keep"), "").unwrap();
            set_mtimes(&scratch.join(entry).join(".keep"), hours(h));
        }
        // A fresh file outside, linked as linkkeep's .keep; a directory .keep, touched 3 h ago.
        std::fs::write(t.join("fresh"), "").unwrap();
        std::os::unix::fs::symlink(t.join("fresh"), scratch.join("linkkeep/.keep")).unwrap();
        std::fs::create_dir(scratch.join("dirkeep/.keep")).unwrap();
        let skip = [
            "read/.keep",
            "task/.keep",
            "stale/.keep",
            "future/.keep",
            "dirkeep/.keep",
            "linkkeep/.keep",
            "kn/.keep",
            "ks/.keep",
        ];
        for entry in std::fs::read_dir(t).unwrap().flatten() {
            if entry.file_name() != "fresh" {
                set_mtimes_but(&entry.path(), hours(4), &skip.map(|s| scratch.join(s)));
            }
        }
        set_mtimes(&scratch.join("dirkeep/.keep"), hours(3));
        // dup is in use again, changed just now, beside its old copy.
        set_mtimes(&scratch.join("dup/notes/log"), now);
        set_mtimes(&scratch.join("kt/notes/log"), now);
        set_mtimes(&scratch.join("ks/notes/log"), now);
        let old = Command::new("touch").args(["-h", "-t", "202001010000"]).arg(scratch.join("linkkeep/.keep")).status();
        assert!(old.unwrap().success(), "touch -h");

        // A du that, sizing late or sized/wt, and an mv that, renaming moved or trapped aside, each first
        // start a process with its working directory there, so only the looks after can find them. Renaming
        // trapped aside, mv also sends cleanup TERM.
        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("docker"), "#!/bin/sh\nexit 1\n");
        let which = |p: &str| String::from_utf8(run(&dir, "sh", &["-c", &format!("command -v {p}")]).stdout).unwrap();
        let pids = t.join("pids");
        let (du, mv, stat) = (which("du"), which("mv"), which("stat"));
        // A stat that never dates kn's .keep, nor ks's once du has sized ks/tgt (finding df3b4dc4966f).
        let sized = t.join("ks-sized");
        write_executable(
            &stubs.join("stat"),
            &format!(
                "#!/bin/sh\nfor a; do f=$a; done\ncase \"$f\" in */kn/.keep) exit 1 ;;\n\
                 */ks/.keep) [ ! -e '{sized}' ] || exit 1 ;; esac\nexec '{stat}' \"$@\"\n",
                sized = sized.display(),
                stat = stat.trim()
            ),
        );
        write_executable(
            &stubs.join("du"),
            &format!(
                "#!/bin/sh\ncase \"$*\" in *\"/late\" | *\"/sized/wt\") (cd \"$2\" && exec sleep 600) >/dev/null 2>&1 \
                 </dev/null & echo $! >>'{pids}'; sleep 1 ;;\n\
                 *\"/kp\") touch \"$2/.keep\" ;;\n*\"/kt/tgt\") touch \"${{2%/tgt}}/.keep\" ;;\n*\"/ks/tgt\") : >'{sized}' ;;\n*\"/wr\") echo new >\"$2/notes/new\" ;;\n\
                 *\"/gt\") mkdir \"$2/sub\" && : >\"$2/sub/.git\" ;;\n\
                 *\"/commit/wt\") git -C \"$2\" commit -q --allow-empty -m during ;;\nesac\nexec '{du}' \"$@\"\n",
                pids = pids.display(),
                sized = sized.display(),
                du = du.trim()
            ),
        );
        write_executable(
            &stubs.join("mv"),
            &format!(
                "#!/bin/sh\n'{mv}' \"$@\" || exit\nfor a; do to=$a; done\ncase \"$to\" in */moved.cleanup-*) \
                 (cd \"$to\" && exec sleep 600) >/dev/null 2>&1 </dev/null & echo $! >>'{pids}'; sleep 1 ;;\n\
                 */trapped.cleanup-*) kill -TERM $PPID ;; esac\n",
                pids = pids.display(),
                mv = mv.trim()
            ),
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let cleanup = || {
            let out = command(&dir, "scripts/cleanup", &["--apply"])
                .env("HOME", &home)
                .env("TMPDIR", &tmpdir)
                .env("PATH", &path)
                .output()
                .unwrap();
            for pid in std::fs::read_to_string(&pids).unwrap_or_default().split_whitespace() {
                let _ = Command::new("kill").arg(pid).status();
            }
            let _ = std::fs::remove_file(&pids);
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let (code, stdout) = cleanup();
        assert_eq!(code, Some(0), "{scripts}: {stdout}");
        let line = |name: &str| {
            stdout
                .lines()
                .find(|l| l.contains(&format!("/{name}:")))
                .unwrap_or_else(|| panic!("{scripts}: no line for {name}: {stdout}"))
                .to_owned()
        };
        for gone in ["idle", "stale", "future", "linkkeep", "dirkeep"] {
            assert!(line(gone).starts_with("clear"), "{scripts}: {gone}: {stdout}");
            assert!(!scratch.join(gone).exists(), "{scripts}: {gone} stayed: {stdout}");
        }
        for (kept, why) in [
            ("late", "now in use by pid"),
            ("moved", "once renamed aside, so put back"),
            ("read", "kept whole, target dirs included"),
            ("task/wt", "its scratch entry holds a .keep"),
            ("sized/wt", "but now in use by pid"),
            ("kp", "a .keep was touched while it was sized, so put back"),
            ("wr", "it changed while it was sized, so put back"),
            ("gt", "a git checkout appeared in it while it was sized, so put back"),
            ("commit/wt", "but it changed while it was judged"),
            (&format!("busy.cleanup-{alive}"), "still running"),
        ] {
            let l = line(kept);
            assert!(l.starts_with("keep") && l.contains(why), "{scripts}: {kept}: {l}");
        }
        // kt's tgt/, sized as kt's .keep is touched (finding 1efc75ddf0cb).
        let kt = stdout.lines().any(|l| {
            l.starts_with("keep")
                && l.contains("/kt:")
                && l.contains("tgt/ stays: a .keep was touched while it was sized")
        });
        assert!(kt, "{scripts}: kt's tgt/ wasn't put back: {stdout}");
        // A .keep that can't be dated: kn kept whole before sizing, ks's tgt/ put back after the rename.
        let l = line("kn");
        assert!(l.starts_with("keep") && l.contains("holds a .keep"), "{scripts}: kn: {l}");
        let ks = stdout.lines().any(|l| {
            l.starts_with("keep")
                && l.contains("/ks:")
                && l.contains("tgt/ stays: a .keep was touched while it was sized")
        });
        assert!(ks, "{scripts}: ks's tgt/ wasn't put back: {stdout}");
        let l = line(&format!("dup.cleanup-{dead}"));
        assert!(l.starts_with("won't touch") && l.contains("while dup existed"), "{scripts}: {l}");
        let l = stdout.lines().find(|l| l.starts_with("put back")).unwrap_or_else(|| panic!("{scripts}: {stdout}"));
        assert!(l.contains("/orphan:"), "{scripts}: {l}");
        let again = stdout.lines().any(|l| l.starts_with("keep") && l.contains("/orphan: put back this run"));
        assert!(again, "{scripts}: orphan was judged again: {stdout}");
        for kept in [
            "late",
            "moved",
            "read",
            "read/tgt/debug/big",
            "task/wt",
            "sized/wt",
            "orphan/notes/log",
            "dup",
            "kp/.keep",
            "wr/notes/new",
            "gt/sub/.git",
            "commit/wt",
            "kt/tgt/debug/big",
            "kn/notes/log",
            "ks/tgt/debug/big",
        ] {
            assert!(scratch.join(kept).exists(), "{scripts}: {kept} went: {stdout}");
        }
        for kept in [format!("dup.cleanup-{dead}"), format!("busy.cleanup-{alive}")] {
            assert!(scratch.join(&kept).exists(), "{scripts}: {kept} went: {stdout}");
        }
        assert!(!scratch.join(format!("orphan.cleanup-{dead}")).exists(), "{scripts}: {stdout}");
        assert!(scratch.join("moved/notes/log").exists(), "{scripts}: moved wasn't put back whole: {stdout}");
        assert!(stdout.lines().last().unwrap().contains("6 done"), "{scripts}: {stdout}");

        // A run stopped while it has trapped aside puts it back, and leaves no copy.
        std::fs::create_dir_all(scratch.join("trapped/notes")).unwrap();
        set_mtimes(&scratch.join("trapped"), hours(4));
        let (code, stdout) = cleanup();
        assert_eq!(code, Some(130), "{scripts}: {stdout}");
        assert!(scratch.join("trapped/notes").exists(), "{scripts}: trapped wasn't put back: {stdout}");
        let left: Vec<_> = std::fs::read_dir(&scratch).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(
            left.iter().all(|n| !n.to_string_lossy().starts_with("trapped.cleanup-")),
            "{scripts}: a copy was left: {left:?}"
        );
    }
}

/// [`set_mtimes`], but leaving the paths in `skip` (and what's under them) as they are.
fn set_mtimes_but(path: &Path, when: std::time::SystemTime, skip: &[PathBuf]) {
    if skip.iter().any(|s| s == path) {
        return;
    }
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            set_mtimes_but(&entry.unwrap().path(), when, skip);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(when).unwrap();
}

/// Task 9fa138208dac: bracken's .bracken/cache/ (its verified snapshot, since #204) is the one ignored entry
/// under .bracken/ that doesn't keep a worktree whole. A builder's worktree merged into origin/main, gone from
/// origin and at its claimed head goes with its branch though it holds .bracken/cache/; one that also holds
/// .bracken/identities/ (a key) stays, saying why. The repo ignores those two as this one does, not all of
/// .bracken/. A git status --ignored=matching that fails keeps both, at either check (finding 6dc71b5c4733):
/// a git wrapper fails it for the .bracken/ check and the whole-worktree one, then for the latter only.
/// PR #224's review: two more such worktrees stay, with their branches, found by the checks just before the
/// removal (findings 075b4a4dbcfd, 497ffa3fdcdc): one a process starts using while cleanup sizes it, and one
/// that gains a commit then, by a command that has exited by the next look.
#[test]
fn cleanup_removes_a_merged_worktree_holding_brackens_cache_but_not_one_holding_a_key() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let ignore = ".bracken/identities/\n.bracken/cache/\n/target\n";
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ignore, |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let t3 = home.join(".t3/worktrees/repo");
        std::fs::create_dir_all(&t3).unwrap();
        // Each a commit of its own, squash-merged into main, never on origin, its head claimed.
        bk(&dir, &["init", "--name", "cleanup"]);
        bk(&dir, &["id", "new", "builder"]);
        for (n, name) in ["cached", "keyed", "busy", "moving"].iter().enumerate() {
            let wt = t3.join(name);
            git(&dir, &["worktree", "add", "-q", "-b", name, wt.to_str().unwrap()]);
            std::fs::write(wt.join(name), name).unwrap();
            git(&wt, &["add", name]);
            git(&wt, &["commit", "-qm", name]);
            git(&dir, &["merge", "-q", "--squash", name]);
            git(&dir, &["commit", "-qm", &format!("{name} (#{n})")]);
            let head = String::from_utf8(run(&dir, "git", &["rev-parse", name]).stdout).unwrap();
            bk(
                &dir,
                &["claim", &format!("PR #{n} {name} @ {}: {name}", head.trim()), "--tag", "pr", "--as", "builder"],
            );
            std::fs::create_dir_all(wt.join(".bracken/cache")).unwrap();
            std::fs::write(wt.join(".bracken/cache/snapshot"), "verified").unwrap();
        }
        git(&dir, &["push", "-q", "origin", "main"]);
        std::fs::create_dir_all(t3.join("keyed/.bracken/identities")).unwrap();
        std::fs::write(t3.join("keyed/.bracken/identities/k"), "key").unwrap();
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 3600));
        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("docker"), "#!/bin/sh\nexit 1\n");
        // With --apply, a du that, sizing busy, starts a process there, and sizing moving, commits there.
        let pids = t.join("pids");
        let du = String::from_utf8(run(&dir, "sh", &["-c", "command -v du"]).stdout).unwrap();
        write_executable(
            &stubs.join("du"),
            &format!(
                "#!/bin/sh
[ -z \"$DU_ACTS\" ] || case \"$*\" in
\
                 *\"/busy\") (cd \"$2\" && exec sleep 600) >/dev/null 2>&1 </dev/null & echo $! >>'{pids}'; sleep 1 ;;
\
                 *\"/moving\") git -C \"$2\" commit -q --allow-empty -m during ;;
esac
exec '{du}' \"$@\"
",
                pids = pids.display(),
                du = du.trim()
            ),
        );
        let real_git = String::from_utf8(run(&dir, "sh", &["-c", "command -v git"]).stdout).unwrap();
        write_executable(
            &stubs.join("git"),
            &format!(
                "#!/bin/bash\ncase \" $* \" in\n\
                 *' --ignored=matching -- .bracken '*) [ \"$FAIL_GIT\" != all ] || exit 128 ;;\n\
                 *' --ignored=matching '*) [ -z \"$FAIL_GIT\" ] || exit 128 ;;\nesac\nexec '{}' \"$@\"\n",
                real_git.trim()
            ),
        );
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let cleanup = |args: &[&str], fail: &str| {
            let out = command(&dir, "scripts/cleanup", args)
                .env("HOME", &home)
                .env("TMPDIR", &tmpdir)
                .env("PATH", &path)
                .env("FAIL_GIT", fail)
                .env("DU_ACTS", if args.is_empty() { "" } else { "1" })
                .output()
                .unwrap();
            for pid in std::fs::read_to_string(&pids).unwrap_or_default().split_whitespace() {
                let _ = Command::new("kill").arg(pid).status();
            }
            let _ = std::fs::remove_file(&pids);
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let line_in = |out: &str, name: &str| -> String {
            out.lines()
                .find(|l| l.contains(&format!("/{name}:")))
                .unwrap_or_else(|| panic!("{scripts}: {out}"))
                .to_owned()
        };
        for (fail, cached, keyed) in [
            ("all", "a key under .bracken/ can't be ruled out", "a key under .bracken/ can't be ruled out"),
            ("whole", "what else is ignored is unknown", "perhaps an identity's key"),
        ] {
            let (code, dry) = cleanup(&[], fail);
            assert_eq!(code, Some(0), "{scripts}: {dry}");
            for (name, why) in [("cached", cached), ("keyed", keyed)] {
                let l = line_in(&dry, name);
                assert!(l.starts_with("won't touch") && l.contains(why), "{scripts}: git failing {fail}: {l}");
            }
        }
        let (code, stdout) = cleanup(&["--apply"], "");
        let out_code = code;
        assert_eq!(out_code, Some(0), "{scripts}: {stdout}");
        let line = |name: &str| line_in(&stdout, name);
        assert!(line("cached").starts_with("remove"), "{scripts}: {stdout}");
        assert!(!t3.join("cached").exists(), "{scripts}: {stdout}");
        let branches = String::from_utf8(run(&dir, "git", &["branch", "--list", "cached"]).stdout).unwrap();
        assert!(branches.trim().is_empty(), "{scripts}: branch cached stayed: {stdout}");
        let keyed = line("keyed");
        assert!(keyed.starts_with("won't touch") && keyed.contains("perhaps an identity's key"), "{scripts}: {keyed}");
        assert!(t3.join("keyed/.bracken/identities/k").exists(), "{scripts}: {stdout}");
        for (name, why) in [
            ("busy", "branch busy is merged, but now in use by pid"),
            ("moving", "branch moving is merged, but the worktree changed while it was judged"),
        ] {
            let l = line(name);
            assert!(l.starts_with("keep") && l.contains(why), "{scripts}: {name}: {l}");
            assert!(t3.join(name).join(".bracken/cache/snapshot").exists(), "{scripts}: {name} went: {stdout}");
            let branches = String::from_utf8(run(&dir, "git", &["branch", "--list", name]).stdout).unwrap();
            assert!(!branches.trim().is_empty(), "{scripts}: branch {name} went: {stdout}");
        }
    }
}

/// Finding 88fc809d6a05: cleanup's second look, after it renames a target/ aside. A process that starts
/// using the worktree between the rename and that look keeps target/, put back. When a build has meanwhile
/// made a new target/, the put-back fails and the copy stays as target.cleanup-<pid>. A later run, once that
/// pid is gone and nothing uses the worktree, frees the copy, and the copy doesn't make the worktree dirty,
/// in the dry run either. A copy whose pid still runs stays. The repo's .gitignore here, like an old
/// branch's, doesn't cover the copies. A target/ a build writes to while cleanup sizes it, and that has
/// exited by the next look, is put back too (finding 497ffa3fdcdc).
#[test]
fn cleanup_puts_back_a_target_dir_used_once_renamed_aside_and_later_frees_a_copy_left_behind() {
    for scripts in SCRIPTS {
        let cleanup = repo_root().join(scripts).join("cleanup");
        let (tmp, dir) = repo(&[("scripts/cleanup", &cleanup)], ".bracken/\n/target\n", |dir| {
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        });
        let t = tmp.path();
        let git = |dir: &Path, args: &[&str]| {
            let out = run(dir, "git", args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let origin = t.join("origin.git");
        git(t, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        let (home, tmpdir) = (t.join("home"), t.join("tmpdir"));
        let t3 = home.join(".t3/worktrees/repo");
        std::fs::create_dir_all(&t3).unwrap();
        // Unmerged builders' worktrees with an idle target/; "live" also holds a copy whose pid runs.
        let live = Sleeper::in_dir(t);
        let live_copy = format!("target.cleanup-{}", live.id());
        for name in ["back", "stuck", "live", "built"] {
            let wt = t3.join(name);
            git(&dir, &["worktree", "add", "-q", "-b", name, wt.to_str().unwrap()]);
            std::fs::write(wt.join("f"), name).unwrap();
            git(&wt, &["add", "f"]);
            git(&wt, &["commit", "-qm", name]);
            std::fs::create_dir_all(wt.join("target/debug")).unwrap();
            std::fs::write(wt.join("target/debug/big"), "build").unwrap();
        }
        std::fs::create_dir_all(t3.join("live").join(&live_copy).join("debug")).unwrap();
        std::fs::write(t3.join("live").join(&live_copy).join("debug/big"), "build").unwrap();
        set_mtimes(t, std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 3600));

        // An mv that, the first time it renames back/target or stuck/target aside, starts a process in that
        // worktree before returning: so the look after the rename finds it. In stuck, a build has also made a
        // new target/ by then.
        let stubs = t.join("stubs");
        std::fs::create_dir_all(&stubs).unwrap();
        write_executable(&stubs.join("docker"), "#!/bin/sh\nexit 1\n");
        let mv = String::from_utf8(run(&dir, "sh", &["-c", "command -v mv"]).stdout).unwrap();
        let state = t.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let stub = format!(
            "#!/bin/sh\n'{mv}' \"$@\" || exit\nfor a; do to=$a; done\n\
             case \"$to\" in */back/target.cleanup-* | */stuck/target.cleanup-*)\n\
             w=${{to%/target.cleanup-*}}; n=${{w##*/}}\n\
             [ ! -e '{state}'/$n ] || exit 0; : >'{state}'/$n\n\
             [ $n != stuck ] || mkdir \"$w/target\"\n\
             (cd \"$w\" && exec sleep 120) >/dev/null 2>&1 </dev/null & echo $! >>'{state}/pids'; sleep 1 ;;\n\
             esac\n",
            mv = mv.trim(),
            state = state.display()
        );
        write_executable(&stubs.join("mv"), &stub);
        // A du that, the first time it sizes built/target, writes there, as a build would.
        let du = String::from_utf8(run(&dir, "sh", &["-c", "command -v du"]).stdout).unwrap();
        let stub = format!(
            "#!/bin/sh
case \"$*\" in *\"/built/target\") [ -e '{state}'/built ] || \
             {{ : >'{state}'/built; echo new >\"$2/debug/new\"; }} ;; esac\nexec '{du}' \"$@\"\n",
            du = du.trim(),
            state = state.display()
        );
        write_executable(&stubs.join("du"), &stub);
        let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap());
        let cleanup = |args: &[&str]| {
            let out = command(&dir, "scripts/cleanup", args)
                .env("HOME", &home)
                .env("TMPDIR", &tmpdir)
                .env("PATH", &path)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            (out.status.code(), format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)))
        };
        let lines = |out: &str, wt: &str| -> Vec<String> {
            out.lines().filter(|l| l.contains(&format!("/{wt}:"))).map(str::to_owned).collect()
        };
        let copies = |wt: &str| -> Vec<String> {
            std::fs::read_dir(t3.join(wt))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("target.cleanup-"))
                .collect()
        };

        let (code, first) = cleanup(&["--apply"]);
        let pids = std::fs::read_to_string(state.join("pids")).unwrap_or_default();
        for pid in pids.split_whitespace() {
            let _ = Command::new("kill").arg(pid).status();
            // Gone before the next run looks.
            for _ in 0..50 {
                if !Command::new("kill")
                    .args(["-0", pid])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        assert_eq!(code, Some(1), "{scripts}: the stuck copy is a failure: {first}");
        let back = lines(&first, "back").join("\n");
        assert!(back.starts_with("keep") && back.contains("once renamed aside, so put back"), "{scripts}: {back}");
        assert!(t3.join("back/target/debug/big").exists(), "{scripts}: {first}");
        assert!(copies("back").is_empty(), "{scripts}: {:?}", copies("back"));
        let built = lines(&first, "built").join("\n");
        assert!(
            built.starts_with("keep") && built.contains("it changed while it was sized, so put back"),
            "{scripts}: {built}"
        );
        assert!(t3.join("built/target/debug/new").exists(), "{scripts}: {first}");
        assert!(copies("built").is_empty(), "{scripts}: {:?}", copies("built"));
        let stuck = lines(&first, "stuck").join("\n");
        assert!(stuck.starts_with("failed") && stuck.contains("couldn't be put back"), "{scripts}: {stuck}");
        let left = copies("stuck");
        assert_eq!(left.len(), 1, "{scripts}: {first}");
        assert!(stuck.contains(&left[0]), "{scripts}: the line names the copy: {stuck}");
        assert!(t3.join("stuck").join(&left[0]).join("debug/big").exists(), "{scripts}: {first}");

        // Hours later: the copy's run is over and nothing uses stuck. The dry run would free it, and judges
        // stuck as if it had: kept, as a build just made its target/, not dirty.
        set_mtimes(
            &t3.join("stuck").join(&left[0]),
            std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 3600),
        );
        let (code, dry) = cleanup(&[]);
        assert_eq!(code, Some(0), "{scripts}: {dry}");
        let stuck = lines(&dry, "stuck");
        assert!(stuck.iter().any(|l| l.starts_with("clean") && l.contains(&format!("remove {}/", left[0]))), "{dry}");
        assert!(stuck.iter().all(|l| !l.contains("dirty")), "{scripts}: the copy made it dirty: {dry}");
        assert!(stuck.iter().any(|l| l.starts_with("keep")), "{scripts}: judged as clean: {dry}");
        let live_lines = lines(&dry, "live").join("\n");
        assert!(!live_lines.contains(&live_copy) && live_lines.contains("dirty"), "{scripts}: {live_lines}");

        let (code, applied) = cleanup(&["--apply"]);
        assert_eq!(code, Some(0), "{scripts}: {applied}");
        assert!(copies("stuck").is_empty(), "{scripts}: {applied}");
        // The new target/ is a build's, made just now: it stays.
        assert!(t3.join("stuck/target").exists(), "{scripts}: {applied}");
        assert_eq!(copies("live"), std::slice::from_ref(&live_copy), "{scripts}: {applied}");
        assert!(lines(&applied, "live").join("\n").contains("dirty"), "{scripts}: {applied}");
        drop(live);
    }
}
