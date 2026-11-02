//! What the scripts tests' modules share: running a script in a throwaway repository and Ground.

pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::process::{Command, Output};

pub(crate) use manyroot::Body;

pub(crate) use crate::common;

pub(crate) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub(crate) const SCRIPTS: [&str; 2] = ["scripts", ".claude/skills/agent-team-kickoff/templates"];
// The tests split per copy (the _in_scripts and _in_the_kickoff_templates pairs) name each copy by
// index: a third copy needs a third of each.
const _: () = assert!(SCRIPTS.len() == 2);

pub(crate) fn command(dir: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .env("BRACKEN_BIN", dir.join("../bracken"))
        .env("REAL_BRACKEN", env!("CARGO_BIN_EXE_bracken"))
        .env_remove("BRACKEN_DIR")
        .env_remove("BRACKEN_AGENT")
        // verify-seq's free-space floor would make these tests depend on the machine's disk.
        .env("VERIFY_SEQ_MIN_GB", "0")
        .env_remove("VERIFY_SEQ_WARN_GB");
    command
}

pub(crate) fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    command(dir, program, args).output().unwrap()
}

/// Writes an executable through a child `sh`, so this test process never holds it open for writing. A
/// parallel test's fork would inherit such a descriptor until its exec, and running the file meanwhile
/// fails with ETXTBSY, "Text file busy" (task aa2ac61f8dd4).
pub(crate) fn write_executable(path: &Path, contents: &str) {
    let mut sh = Command::new("sh")
        .args(["-c", "cat >\"$0\"", path.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut sh.stdin.take().unwrap(), contents.as_bytes()).unwrap();
    assert!(sh.wait().unwrap().success(), "couldn't write {path:?}");
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

/// Copies a file with `cp`, for the same reason as [`write_executable`]: `std::fs::copy` would hold the
/// copy open for writing in this process.
pub(crate) fn copy_executable(from: &Path, to: &Path) {
    assert!(Command::new("cp").arg(from).arg(to).status().unwrap().success(), "cp {from:?} {to:?}");
}

pub(crate) fn bk(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, "scripts/bk", args);
    assert!(out.status.success(), "bk {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    common::bare_ids(String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

/// A git repository holding `scripts/bk`, the given `bk-evidence` and the `code-scope` beside it, with a
/// Ground and identity `val`.
///
/// `scripts/bk` runs bracken through a pipe that fails when its reader stops early, as an
/// older bracken or a `printf | grep -q` did: the scripts must not depend on how the writer
/// takes a closed pipe.
pub(crate) fn workspace(bk_evidence: &Path) -> (tempfile::TempDir, PathBuf, String) {
    let scope = bk_evidence.with_file_name("code-scope");
    let (tmp, dir) =
        repo(&[("scripts/bk-evidence", bk_evidence), ("scripts/code-scope", &scope)], ".bracken/\n", |_| {});
    let sha = String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned();
    bk(&dir, &["init", "--name", "scripts"]);
    bk(&dir, &["id", "new", "val"]);
    (tmp, dir, sha)
}

/// A git repository on main holding `scripts/bk`, the given files, and whatever `setup` adds, all committed.
pub(crate) fn repo(
    files: &[(&str, &Path)],
    gitignore: &str,
    setup: impl FnOnce(&Path),
) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    write_executable(
        &tmp.path().join("bracken"),
        "#!/usr/bin/env bash\nset -o pipefail\n\"$REAL_BRACKEN\" \"$@\" | cat\n",
    );
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    copy_executable(&repo_root().join("scripts/bk"), &dir.join("scripts/bk"));
    for (to, from) in files {
        copy_executable(from, &dir.join(to));
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

/// Evidence on the pr claim as bk-evidence --on records it: `<command> @ <head> on <main sha>`.
#[derive(Clone, Copy)]
pub(crate) struct Tested<'a> {
    pub(crate) who: &'a str,
    /// The command in the method: `scripts/test-table` unless a case says otherwise.
    pub(crate) command: &'a str,
    /// main's rev at the end, or a 40-hex sha taken as it is.
    pub(crate) rev: &'a str,
    /// The PR's head if None.
    pub(crate) head: Option<&'a str>,
    pub(crate) verdict: &'a str,
    /// Retracted by `who` afterwards.
    pub(crate) retracted: bool,
    /// After the main sha in the method: ' tree <key> docs-only', say.
    pub(crate) suffix: &'a str,
}

pub(crate) fn tested<'a>(who: &'a str, rev: &'a str) -> Tested<'a> {
    Tested { who, rev, command: "scripts/test-table", head: None, verdict: "pass", retracted: false, suffix: "" }
}

/// The line `merge` printed for check `name`: `ok` or `FAIL`, then the check's name and detail.
pub(crate) fn check_line(out: &Output, name: &str) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find(|l| l.split_whitespace().nth(1) == Some(name));
    line.unwrap_or_else(|| panic!("no {name} check in: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr)))
        .to_owned()
}

/// Stages `contents` at `path` with `mode`, as git's index takes it, without the working tree: for a
/// path in a case a case-insensitive filesystem would fold, or a symlink.
pub(crate) fn stage(dir: &Path, mode: &str, path: &str, contents: &str) {
    let blob = dir.join("../blob");
    std::fs::write(&blob, contents).unwrap();
    let oid = run(dir, "git", &["hash-object", "-w", blob.to_str().unwrap()]).stdout;
    let entry = format!("{mode},{},{path}", String::from_utf8(oid).unwrap().trim());
    let out = run(dir, "git", &["update-index", "--add", "--cacheinfo", &entry]);
    assert!(out.status.success(), "staging {path}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Writes `text` at `path` in `dir`, making its directories.
pub(crate) fn put(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// A stand-in for crates/scope's bracken-scope, which code-scope runs through CODE_SCOPE_TOKENS: these
/// repositories have no Cargo workspace for it to read. comment-only says no to every file. packages logs
/// its arguments, and the paths it was given, to $AFFECTED_LOG if set; prints every package for --all;
/// fails with $AFFECTED_FAIL; and otherwise maps each path as the old hand-written rows did, enough for
/// these tests. The real tool is tested in crates/scope.
pub(crate) const AFFECTED_STUB: &str = r#"#!/usr/bin/env bash
[ "$1" != comment-only ] || { echo 'no: the stub'; exit 1; }
shift
if [ "$1" = --all ]; then
  echo "$*" >>"${AFFECTED_LOG:-/dev/null}"
  printf '%s\n' bracken bracken-demo bracken-experiment bracken-scope bracken-tree manyroot
  exit 0
fi
mapfile -d '' -t paths
echo "$* <${paths[*]}>" >>"${AFFECTED_LOG:-/dev/null}"
[ -z "${AFFECTED_FAIL:-}" ] || { echo 'cargo metadata failed' >&2; exit 3; }
for p in "${paths[@]}"; do
  case "$p" in
    rustfmt.toml) ;;
    crates/tree/*) printf '%s\n' bracken bracken-tree ;;
    crates/bracken/* | crates/demo/* | crates/experiment/*) printf '%s\n' bracken bracken-demo bracken-experiment ;;
    scripts/* | .claude/skills/*) echo bracken ;;
    *) printf '%s\n' bracken bracken-tree ;;
  esac
done | LC_ALL=C sort -u
"#;

/// Writes [`AFFECTED_STUB`] at `path`, returning it.
pub(crate) fn affected_stub(path: &Path) -> PathBuf {
    write_executable(path, AFFECTED_STUB);
    path.to_owned()
}

/// Commits everything in `dir` as `message`.
pub(crate) fn commit_all(dir: &Path, message: &str) {
    assert!(run(dir, "git", &["add", "-A"]).status.success());
    assert!(run(dir, "git", &["commit", "-qm", message]).status.success());
}

/// Writes an artifact with `media_type` into `dir`'s Ground with the real binary, past scripts/bk's
/// refusal, as an agent running bracken by hand could, signed by a new identity `hand`.
pub(crate) fn weave_artifact(dir: &Path, media_type: &str) {
    let real = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bracken"))
            .args(["--dir", ".bracken"])
            .args(args)
            .current_dir(dir)
            .env_remove("BRACKEN_DIR")
            .env_remove("BRACKEN_AGENT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    std::fs::write(dir.join("../marker.cbor"), b"anything").unwrap();
    real(&["id", "new", "hand"]);
    real(&["artifact", "../marker.cbor", "--name", "weave", "--media-type", media_type, "--as", "hand"]);
}

pub(crate) fn weave(dir: &Path, script: &str, home: &Path, bin: Option<&Path>, args: &[&str]) -> Output {
    let mut c = command(dir, script, args);
    c.env("HOME", home).env("BRACKEN_DIR", "/inherited/and/ignored");
    if let Some(bin) = bin {
        c.env("BRACKEN_BIN", bin);
    }
    c.output().unwrap()
}

/// A batch (decision db6f7705c0a4), end to end: a repository with the real merge, verify-seq, bk-evidence,
/// code-scope and ground-guards, a committed stub scripts/test-table that fails when the tree holds a
/// file named `broken`, and a local forge whose `curl` stub answers GET /pulls/<n> and does POST
/// /pulls/<n>/merge as a squash merge (git merge-tree onto main). PR #n adds `a<n>.rs`, and `broken` too
/// if it is in `broken`. With `$T/tamper` holding n, the forge's merge of #n leaves main's tree as it was.
/// With `$T/flaky`, the table's next run fails, once; with `$T/flaky-tree`, the next run whose a*.rs files
/// are those it lists (as `a1.rs a2.rs `) does. With `$T/slow`, each run waits 10 minutes before its
/// result; with `$T/slow-tree`, only the run of the tree it lists does. With `$T/die-tree`, that tree's run
/// kills its verify-seq job's leader (SIGKILL), so the job ends without its .done file, once, and goes on
/// in the job's process group, its pid in `$T/orphan`, for 10 minutes; a SIGINT stops it 3 s later, once
/// it has made `$T/orphan-stopped`. With
/// `$T/hook-<n>`, the forge runs it at the
/// `$T/hook-at-<n>`'th GET of /pulls/<n>. With `$T/post-502-<n>`, the POST of #n merges but answers 502,
/// as a proxy can while the forge merges anyway.
pub(crate) struct Batch {
    pub(crate) tmp: tempfile::TempDir,
    pub(crate) dir: PathBuf,
    pub(crate) forge: PathBuf,
    pub(crate) claims: Vec<String>,
    pub(crate) shas: Vec<String>,
}

impl Batch {
    pub(crate) fn new(scripts: &str, prs: usize, broken: &[usize], main_broken: bool) -> Batch {
        Batch::shaped(scripts, prs, broken, main_broken, &|_| {}, &|_| 0, &|_, _| {})
    }

    /// As `new`, with `on_main` adding to main's first commit, PR #n branching from PR #`base(n)`'s branch
    /// (0 for main), and `extra(dir, n)` adding to PR #n before its commit.
    pub(crate) fn shaped(
        scripts: &str,
        prs: usize,
        broken: &[usize],
        main_broken: bool,
        on_main: &dyn Fn(&Path),
        base: &dyn Fn(usize) -> usize,
        extra: &dyn Fn(&Path, usize),
    ) -> Batch {
        let file = |name: &str| repo_root().join(scripts).join(name);
        let names = ["merge", "ground-guards", "code-scope", "verify-seq", "bk-evidence"];
        let paths: Vec<(String, PathBuf)> = names.iter().map(|n| (format!("scripts/{n}"), file(n))).collect();
        let files: Vec<(&str, &Path)> = paths.iter().map(|(to, from)| (to.as_str(), from.as_path())).collect();
        let (tmp, dir) = repo(&files, ".bracken/identities/\n", |dir| {
            write_executable(
                &dir.join("scripts/test-table"),
                "#!/bin/sh\ntree=$(ls a*.rs 2>/dev/null | tr '\\n' ' ')\necho \"run $tree\" >>\"$LOG\"\n\
                 if [ -e \"$T/flaky\" ]; then rm \"$T/flaky\"; echo 'test result: FAILED (flaky)'; exit 1; fi\n\
                 if [ -e \"$T/flaky-tree\" ] && [ \"$(cat \"$T/flaky-tree\")\" = \"$tree\" ]; then rm \"$T/flaky-tree\"; echo 'test result: FAILED (flaky)'; exit 1; fi\n\
                 if [ -e \"$T/slow\" ]; then sleep 600; fi\n\
                 if [ -e \"$T/slow-tree\" ] && [ \"$(cat \"$T/slow-tree\")\" = \"$tree\" ]; then sleep 600; fi\n\
                 if [ -e \"$T/die-tree\" ] && [ \"$(cat \"$T/die-tree\")\" = \"$tree\" ]; then rm \"$T/die-tree\"\n\
                   echo $$ >\"$T/orphan\"; trap 'sleep 3; touch \"$T/orphan-stopped\"; exit 130' INT; kill -9 \"$(ps -o pgid= -p $$ | tr -d ' ')\"; sleep 600; fi\n\
                 if [ -e broken ]; then echo 'test result: FAILED'; exit 1; fi\necho 'test-table: all passed'\n",
            );
            if main_broken {
                put(dir, "broken", "on main\n");
            }
            on_main(dir);
        });
        let git = |args: &[&str]| assert!(run(&dir, "git", args).status.success(), "git {args:?} failed");
        bk(&dir, &["init", "--name", "batch"]);
        for who in ["builder", "val", "manager"] {
            bk(&dir, &["id", "new", who]);
        }
        bk(&dir, &["claim", "first", "--tag", "note", "--as", "val"]);
        commit_all(&dir, "ground");
        let mut shas = Vec::new();
        for n in 1..=prs {
            let from = match base(n) {
                0 => "main".to_owned(),
                b => format!("pr{b}"),
            };
            git(&["switch", "-qc", &format!("pr{n}"), &from]);
            put(&dir, &format!("a{n}.rs"), &format!("pr {n}\n"));
            if broken.contains(&n) {
                put(&dir, "broken", &format!("by pr {n}\n"));
            }
            extra(&dir, n);
            commit_all(&dir, &format!("pr {n}"));
            shas.push(String::from_utf8(run(&dir, "git", &["rev-parse", "HEAD"]).stdout).unwrap().trim().to_owned());
        }
        git(&["switch", "-q", "main"]);
        let forge = tmp.path().join("forge.git");
        git(&["init", "-q", "--bare", forge.to_str().unwrap()]);
        let mut refs = vec!["main".to_owned()];
        refs.extend((1..=prs).map(|n| format!("pr{n}:refs/pull/{n}/head")));
        let mut push = vec!["push", "-q", forge.to_str().unwrap()];
        push.extend(refs.iter().map(String::as_str));
        git(&push);
        git(&["config", "remote.origin.url", "https://forge.example/o/r.git"]);
        git(&["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]);
        git(&["config", &format!("url.{}.insteadOf", forge.display()), "https://forge.example/o/r.git"]);
        git(&[
            "config",
            "credential.https://forge.example.helper",
            "!f() { echo username=t; echo password=DUMMY; }; f",
        ]);
        git(&["fetch", "-q", "origin"]);
        let mut claims = Vec::new();
        for (n, sha) in shas.iter().enumerate() {
            let claim = bk(
                &dir,
                &[
                    "claim",
                    &format!("PR #{} pr{} @ {sha}: change {}", n + 1, n + 1, n + 1),
                    "--tag",
                    "pr",
                    "--as",
                    "builder",
                ],
            );
            bk(&dir, &["evidence", &claim, "pass", "--method", "checked", "--as", "val"]);
            claims.push(claim);
        }
        std::fs::create_dir_all(tmp.path().join("stubs")).unwrap();
        write_executable(
            &tmp.path().join("stubs/curl"),
            "#!/bin/bash\nmethod=GET body=\nwhile [ $# -gt 0 ]; do case \"$1\" in -X) method=$2; shift 2 ;; \
             --data-binary) body=${2#@}; shift 2 ;; -H) shift 2 ;; *) url=$1; shift ;; esac; done\n\
             n=$(sed -E 's|.*/pulls/([0-9]+).*|\\1|' <<<\"$url\")\n\
             g() { git --git-dir=\"$FORGE\" \"$@\"; }\n\
             if [ \"$method\" = GET ]; then\n\
               echo x >>\"$T/gets-$n\"\n\
               if [ -x \"$T/hook-$n\" ] && [ \"$(wc -l <\"$T/gets-$n\")\" -eq \"$(cat \"$T/hook-at-$n\")\" ]; then \"$T/hook-$n\"; fi\n\
               head=$(g rev-parse \"refs/pull/$n/head\"); merged=false; [ ! -e \"$T/merged-$n\" ] || merged=true\n\
               echo \"{\\\"head\\\":{\\\"sha\\\":\\\"$head\\\"},\\\"state\\\":\\\"open\\\",\\\"merged\\\":$merged,\\\"base\\\":{\\\"ref\\\":\\\"main\\\"},\\\"title\\\":\\\"t$n\\\"}\"\n\
               exit 0\n\
             fi\n\
             sha=$(jq -r .head_commit_id \"$body\")\n\
             tree=$(g merge-tree --write-tree main \"$sha\") || exit 22\n\
             [ \"$(cat \"$T/tamper\" 2>/dev/null)\" != \"$n\" ] || tree=$(g rev-parse 'main^{tree}')\n\
             new=$(g -c user.name=f -c user.email=f@invalid commit-tree \"$tree\" -p main -m \"squash $n\")\n\
             g update-ref refs/heads/main \"$new\"; touch \"$T/merged-$n\"\n\
             if [ -e \"$T/post-502-$n\" ]; then echo 502; exit 22; fi\n\
             echo 200\n",
        );
        Batch { tmp, dir, forge, claims, shas }
    }

    pub(crate) fn command(&self, program: &str, args: &[&str]) -> Command {
        let path = std::env::var("PATH").unwrap();
        let mut c = command(&self.dir, program, args);
        c.env("PATH", format!("{}:{path}", self.tmp.path().join("stubs").display()))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LOG", self.tmp.path().join("log"))
            .env("T", self.tmp.path())
            .env("FORGE", &self.forge)
            .env("TMPDIR", self.tmp.path())
            .env_remove("ON")
            .env_remove("MERGE_BATCH")
            .env_remove("MERGE_BATCH_PRECHECK");
        c
    }

    /// `verify-seq --split --speculative -j <jobs>` over these pairs, in order.
    pub(crate) fn speculative(&self, pairs: &[String], jobs: &str) -> Output {
        let mut args = vec!["--split", "--speculative", "-j", jobs];
        args.extend(pairs.iter().map(String::as_str));
        self.command("scripts/verify-seq", &args).output().unwrap()
    }

    /// PR #n's pair, `<claim>:<sha>`.
    pub(crate) fn pair(&self, n: usize) -> String {
        format!("{}:{}", &self.claims[n - 1][..12], self.shas[n - 1])
    }

    /// The table's runs, sorted: parallel runs log in any order.
    pub(crate) fn sorted_runs(&self) -> Vec<String> {
        let mut runs = self.runs();
        runs.sort();
        runs
    }

    pub(crate) fn verify(&self, members: &[usize]) -> Output {
        let pairs: Vec<String> =
            members.iter().map(|n| format!("{}:{}", &self.claims[n - 1][..12], self.shas[n - 1])).collect();
        let mut args = vec!["--batch"];
        args.extend(pairs.iter().map(String::as_str));
        self.command("scripts/verify-seq", &args).output().unwrap()
    }

    pub(crate) fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.tmp.path().join("log")).unwrap_or_default().lines().map(String::from).collect()
    }

    /// The manager's evidence methods on PR #n's claim, with their verdicts.
    pub(crate) fn evidence(&self, n: usize) -> Vec<(String, String)> {
        let shown: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["show", &self.claims[n - 1], "--json"])).unwrap();
        shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence" && r["author_name"] == "manager")
            .map(|r| {
                (r["body"]["verdict"].as_str().unwrap().to_owned(), r["body"]["method"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    pub(crate) fn batch_claim(&self) -> String {
        let log: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["log", "--tag", "batch", "--json"])).unwrap();
        log[0]["id"].as_str().expect("a batch claim").to_owned()
    }

    pub(crate) fn merge(&self, args: &[&str]) -> Output {
        let mut all = args.to_vec();
        all.extend(["--validator", "val", "--min-passes", "1", "--as", "manager"]);
        self.command("scripts/merge", &all).output().unwrap()
    }

    pub(crate) fn forge_main(&self) -> String {
        let out = run(&self.dir, "git", &["--git-dir", self.forge.to_str().unwrap(), "rev-parse", "main"]);
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    pub(crate) fn merged(&self) -> Vec<usize> {
        (1..=self.claims.len()).filter(|n| self.tmp.path().join(format!("merged-{n}")).exists()).collect()
    }
}

pub(crate) fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

impl Batch {
    /// Commits `path` with `text` on the forge's main, as another merge would, through a temporary index.
    pub(crate) fn hook(&self, n: usize, at_get: usize, path: &str, text: &str) {
        write_executable(
            &self.tmp.path().join(format!("hook-{n}")),
            &format!(
                "#!/bin/bash\nset -e\ng() {{ git --git-dir=\"$FORGE\" \"$@\"; }}\nexport GIT_INDEX_FILE=\"$T/hook-index\"\n\
                 g read-tree main\nblob=$(printf '{text}' | g hash-object -w --stdin)\n\
                 g update-index --add --cacheinfo 100644,\"$blob\",'{path}'\n\
                 new=$(g -c user.name=h -c user.email=h@invalid commit-tree \"$(g write-tree)\" -p main -m intruder)\n\
                 g update-ref refs/heads/main \"$new\"\n"
            ),
        );
        std::fs::write(self.tmp.path().join(format!("hook-at-{n}")), at_get.to_string()).unwrap();
    }

    /// main gains `path` on the branch's checkout and the forge, as a merge of other work would.
    pub(crate) fn main_moves(&self, path: &str) {
        put(&self.dir, path, "main moved\n");
        commit_all(&self.dir, "main moves");
        assert!(run(&self.dir, "git", &["push", "-q", self.forge.to_str().unwrap(), "main"]).status.success());
    }
}

impl Batch {
    /// The manager's passes on PR #n's claim, by the batch claim each names.
    pub(crate) fn passes_by_batch(&self, n: usize) -> Vec<String> {
        self.evidence(n)
            .iter()
            .filter(|(v, _)| v == "pass")
            .filter_map(|(_, m)| m.split(" batch ").nth(1).map(|r| r[..64].to_owned()))
            .collect()
    }

    /// The speculative batch claims, by their member count.
    pub(crate) fn speculative_claims(&self) -> Vec<(usize, String)> {
        let log: serde_json::Value =
            serde_json::from_str(&bk(&self.dir, &["log", "--tag", "speculative", "--json"])).unwrap();
        log.as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let statement = c["body"]["statement"].as_str().unwrap();
                let n = statement.strip_prefix("Batch of ").unwrap().split(' ').next().unwrap().parse().unwrap();
                (n, c["id"].as_str().unwrap().to_owned())
            })
            .collect()
    }
}

impl Batch {
    /// The manager's evidence on a batch claim, as (verdict, method) pairs.
    pub(crate) fn claim_evidence(&self, claim: &str) -> Vec<(String, String)> {
        let shown: serde_json::Value = serde_json::from_str(&bk(&self.dir, &["show", claim, "--json"])).unwrap();
        shown["referrers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "evidence")
            .map(|r| {
                (r["body"]["verdict"].as_str().unwrap().to_owned(), r["body"]["method"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    /// The bracken every script here runs refuses to write evidence on any claim listed in `$T/refuse`.
    pub(crate) fn refuse_evidence_on(&self, claims: &[&str]) {
        std::fs::write(self.tmp.path().join("refuse"), claims.join("\n") + "\n").unwrap();
        write_executable(
            &self.tmp.path().join("bracken"),
            "#!/usr/bin/env bash\nset -o pipefail\n\
             if [ \"$1\" = evidence ] && grep -qxF -- \"$2\" \"$T/refuse\"; then echo 'refused for the test' >&2; exit 1; fi\n\
             \"$REAL_BRACKEN\" \"$@\" | cat\n",
        );
    }

    pub(crate) fn worktrees(&self) -> usize {
        String::from_utf8(run(&self.dir, "git", &["worktree", "list"]).stdout).unwrap().lines().count()
    }
}
