//! scripts/ground-checkpoint. Each test runs the script and its kickoff-template copy.

use crate::support::*;

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

/// ground-checkpoint runs ground-guards on what origin's main gained before anything is pulled: it names a
/// clean commit as checked, and stops on one that edits a committed Ground object. Offline, it still commits,
/// and says to rerun it before pushing. A root .gitattributes change lands only with --attributes-checked.
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
        bk(&dir, &["id", "new", "manager"]);
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
        assert!(
            stdout.contains("rerun ground-checkpoint") && !stdout.contains("push it"),
            "{}: {stdout}",
            case("offline")
        );
        assert_ne!(git(&["rev-parse", "HEAD"]), before, "{}: nothing was committed", case("offline"));

        // A change to the root .gitattributes stops every checkpoint until a manager lands it with
        // --attributes-checked and origin's main's full sha, which records a note.
        git(&["remote", "set-url", "origin", forge.to_str().unwrap()]);
        git(&["push", "-qf", "origin", &format!("{clean}:main")]);
        let attributes = push_ahead(&|dir| std::fs::write(dir.join(".gitattributes"), "*.png binary\n").unwrap());
        let checkpoint = |args: &[&str]| {
            let out = run(&dir, "scripts/ground-checkpoint", args);
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            (out.status.code(), text)
        };
        let (code, text) = checkpoint(&[]);
        assert_eq!(code, Some(1), "{}: {text}", case("attributes"));
        assert!(text.contains("root .gitattributes changed") && text.contains("--attributes-checked"), "{text}");
        let (code, text) = checkpoint(&["--attributes-checked", &clean]);
        assert_eq!(code, Some(1), "{}: took another sha: {text}", case("attributes"));
        assert!(text.contains("isn't origin's main"), "{}: {text}", case("attributes"));
        assert!(
            !bk(&dir, &["log", "--tag", "note", "--full"]).contains("--attributes-checked"),
            "{}",
            case("attributes")
        );
        let (code, text) = checkpoint(&["--attributes-checked", &attributes]);
        assert_eq!(code, Some(0), "{}: {text}", case("attributes"));
        assert!(text.contains(&format!("git merge --no-edit {attributes}")), "{}: {text}", case("attributes"));
        let notes = bk(&dir, &["log", "--tag", "note", "--full"]);
        assert!(notes.contains("manager") && notes.contains("--attributes-checked"), "{}: {notes}", case("attributes"));
        assert!(
            git(&["status", "--porcelain", "--", ".bracken"]).is_empty(),
            "{}: the note wasn't committed",
            case("attributes")
        );
    }
}

/// The Weave tripwire in ground-checkpoint: a Weave record in the local Ground stops the commit, naming
/// it, whatever the media type's case.
#[test]
fn ground_checkpoint_refuses_a_ground_holding_a_weave_record() {
    for scripts in SCRIPTS {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let (checkpoint, guards) = (file("ground-checkpoint"), file("ground-guards"));
        let files = [("scripts/ground-checkpoint", &checkpoint), ("scripts/ground-guards", &guards)];
        let (_tmp, dir) = repo(&files.map(|(to, from)| (to, from.as_path())), ".bracken/identities/\n", |_| {});
        bk(&dir, &["init", "--name", "checkpoint"]);
        bk(&dir, &["id", "new", "val"]);
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        weave_artifact(&dir, "application/VND.bracken.weave.manifest+cbor");
        let head = run(&dir, "git", &["rev-parse", "HEAD"]).stdout;
        let out = run(&dir, "scripts/ground-checkpoint", &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{scripts}: {stderr}");
        assert!(stderr.contains("Weave media type") && stderr.contains("Tell the owner"), "{scripts}: {stderr}");
        assert_eq!(run(&dir, "git", &["rev-parse", "HEAD"]).stdout, head, "{scripts}: a commit was made");
        assert!(run(&dir, "git", &["diff", "--cached", "--quiet"]).status.success(), "{scripts}: left staged");
    }
}

/// With a team server that holds keys, each checkpoint records the signing log's length and hash, and
/// scopes.toml's hash, in a signing-log claim whenever either changed, says when scopes.toml did, and stops
/// when the log no longer extends the one recorded (docs/design/agent-keys.md, 4.1; verdict 4386857e6cdb).
#[test]
fn ground_checkpoint_records_the_signing_log_and_stops_on_a_rewrite() {
    let real = env!("CARGO_BIN_EXE_bracken");
    for scripts in SCRIPTS {
        let copy = |name: &str| repo_root().join(scripts).join(name);
        let files = [
            ("scripts/bk", copy("bk")),
            ("scripts/team-server", copy("team-server")),
            ("scripts/ground-checkpoint", copy("ground-checkpoint")),
        ];
        let files: Vec<(&str, &Path)> = files.iter().map(|(to, from)| (*to, from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/identities/\n.bracken-team\n", |_| {});
        let team = tmp.path().join("team");
        let sh = |program: &str, args: &[&str], agent: Option<&str>| {
            let mut c = command(&dir, program, args);
            c.env("BRACKEN_BIN", real);
            if let Some(agent) = agent {
                c.env("BRACKEN_AGENT", agent);
            }
            c.output().unwrap()
        };
        let ok = |out: Output, what: &str| {
            assert!(out.status.success(), "{scripts}: {what} failed: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        };
        struct Stop<'a>(&'a dyn Fn());
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                (self.0)()
            }
        }
        let stop = || {
            sh("scripts/team-server", &["stop"], None);
        };
        let _stop = Stop(&stop);
        let records = || -> Vec<String> {
            let log = ok(sh("scripts/bk", &["log", "--tag", "signing-log", "--json"], Some("manager")), "log");
            let log: serde_json::Value = serde_json::from_str(&log).unwrap();
            log.as_array().unwrap().iter().map(|c| c["body"]["statement"].as_str().unwrap().to_owned()).collect()
        };
        let checkpoint = || sh("scripts/ground-checkpoint", &[], Some("manager"));

        ok(sh("scripts/bk", &["init", "--name", "team"], None), "init");
        ok(sh("scripts/bk", &["id", "new", "manager"], None), "id new");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
        ok(sh("scripts/team-server", &["setup", team.to_str().unwrap(), &port], None), "setup");
        ok(sh("scripts/team-server", &["token", "manager"], None), "token");
        ok(sh("scripts/team-server", &["start"], None), "start");
        ok(sh("scripts/team-server", &["key", "builder-x"], None), "key");

        let out = ok(checkpoint(), "the first checkpoint");
        assert!(out.contains("recorded the signing log (0 bytes)"), "{scripts}: {out}");
        let first = records();
        assert!(first.len() == 1 && first[0].contains("signed.jsonl 0 bytes, blake3 "), "{scripts}: {first:?}");
        let tracked = String::from_utf8(run(&dir, "git", &["ls-files", ".bracken"]).stdout).unwrap();
        assert!(tracked.contains(".bracken/agents/builder-x.json"), "{scripts}: {tracked}");

        // Nothing changed: nothing recorded.
        let out = ok(checkpoint(), "a checkpoint with nothing new");
        assert!(!out.contains("recorded the signing log"), "{scripts}: {out}");
        assert_eq!(records().len(), 1, "{scripts}");

        // A signature, and a scopes edit: both recorded, the edit named.
        ok(sh("scripts/bk", &["claim", "signed by the server", "--tag", "note"], Some("builder-x")), "claim");
        let scopes = team.join("scopes.toml");
        let text = std::fs::read_to_string(&scopes).unwrap();
        std::fs::write(&scopes, text.replace("\"tests\"]", "\"tests\"]\n# edited")).unwrap();
        let out = ok(checkpoint(), "a checkpoint after a signature");
        assert!(out.contains("scopes.toml changed since claim"), "{scripts}: {out}");
        let length = std::fs::metadata(team.join("signed.jsonl")).unwrap().len();
        assert!(out.contains(&format!("recorded the signing log ({length} bytes)")), "{scripts}: {out}");
        assert_eq!(records().len(), 2, "{scripts}");

        // An old entry rewritten: the checkpoint stops, committing nothing.
        let log = std::fs::read_to_string(team.join("signed.jsonl")).unwrap();
        std::fs::write(
            team.join("signed.jsonl"),
            log.replacen("\"credential\":\"builder-x\"", "\"credential\":\"builder-y\"", 1),
        )
        .unwrap();
        ok(sh("scripts/bk", &["claim", "something new", "--tag", "note"], Some("manager")), "claim");
        let head = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap();
        let out = checkpoint();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && stderr.contains("no longer extends"), "{scripts}: {stderr}");
        assert_eq!(String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap(), head, "{scripts}");
    }
}
