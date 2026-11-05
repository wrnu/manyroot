//! README.md's quick start, run as a newcomer would run it (task 95213eda73e8, process idea dc515d716ce6).
//!
//! The test reads the `sh` blocks under `## Quick start`, drops the clone and build lines (this test's
//! own `bracken` stands in for the release build), and runs the rest in one shell, as the README says,
//! inside a temp dir. Any change to the section's shape fails the test rather than skipping a line.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

/// The lines the test drops, in order, as the first two lines of the first block. The test supplies
/// what they do: a checkout isn't needed, and its own `bracken` is first on `PATH`.
const DROPPED: [&str; 2] = [
    "git clone https://github.com/wrnu/manyroot && cd manyroot",
    r#"cargo build --release && export PATH="$PWD/target/release:$PATH""#,
];

/// The comment that ends the Weave block and names what its last command prints, one word a line.
const EXPECTED_MARK: &str = "# A b C";

/// The `sh` blocks under `## Quick start`, each a list of lines. Panics on any shape the test doesn't
/// know, so a change to the README can't make it skip something silently.
fn quick_start_blocks(readme: &str) -> Vec<Vec<&str>> {
    let starts: Vec<usize> =
        readme.lines().enumerate().filter(|(_, line)| *line == "## Quick start").map(|(i, _)| i).collect();
    assert_eq!(starts.len(), 1, "README.md must have exactly one '## Quick start' heading, found {}", starts.len());
    let section = readme.lines().skip(starts[0] + 1).take_while(|line| !line.starts_with("## "));

    let mut blocks = Vec::new();
    let mut open: Option<Vec<&str>> = None;
    for line in section {
        match (&mut open, line.starts_with("```")) {
            (None, true) => {
                assert_eq!(
                    line, "```sh",
                    "every code block in README's quick start must be ```sh; this test runs them all"
                );
                open = Some(Vec::new());
            }
            (Some(_), true) => {
                assert_eq!(line, "```", "a quick start block must close with a bare ```");
                blocks.push(open.take().expect("a block is open"));
            }
            (Some(block), false) => block.push(line),
            (None, false) => {}
        }
    }
    assert!(open.is_none(), "README's quick start has an unclosed code block");
    blocks
}

/// The quick start as one shell script: the blocks without the dropped lines, each followed by a
/// marker line on stdout so the test can tell which block failed and which output is whose.
fn script(blocks: &[Vec<&str>]) -> String {
    assert_eq!(
        blocks.len(),
        2,
        "README's quick start must have two sh blocks, the Ground one and the Weave one; this test needs updating"
    );
    let first = &blocks[0];
    assert!(
        first.len() > DROPPED.len() && first[..DROPPED.len()] == DROPPED,
        "the quick start's first block must start with these lines, which the test drops:\n{}\nfound:\n{}",
        DROPPED.join("\n"),
        first.iter().take(DROPPED.len()).copied().collect::<Vec<_>>().join("\n")
    );
    let dropped_elsewhere = blocks.iter().flatten().filter(|line| DROPPED.contains(line)).count();
    assert_eq!(dropped_elsewhere, DROPPED.len(), "a dropped line appears twice in the quick start");

    let mut script = String::from("set -euo pipefail\n");
    for (i, block) in blocks.iter().enumerate() {
        let skip = if i == 0 { DROPPED.len() } else { 0 };
        for line in &block[skip..] {
            script.push_str(line);
            script.push('\n');
        }
        script.push_str(&format!("echo {}\n", marker(i)));
    }
    script
}

fn marker(block: usize) -> String {
    format!("__quick_start_block_{block}_done__")
}

/// What the Weave block's last command must print, read from its closing comment.
fn expected_weave_output(blocks: &[Vec<&str>]) -> String {
    let last = blocks[1].last().expect("the Weave block has lines");
    assert!(
        last.trim_end().ends_with(EXPECTED_MARK),
        "the Weave block's last line must end with '{EXPECTED_MARK}', the output this test checks; found: {last}"
    );
    EXPECTED_MARK.trim_start_matches('#').split_whitespace().map(|word| format!("{word}\n")).collect()
}

/// A `mktemp` that honours `TMPDIR` for a bare `mktemp -d`, as GNU's does. macOS's ignores it, which
/// would put the README's throwaway Grounds and repositories outside the test's temp dir.
fn write_mktemp_shim(bin: &Path, real: &Path) {
    let shim = bin.join("mktemp");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\n\
             [ \"$#\" -eq 1 ] && [ \"$1\" = -d ] || {{ echo \"quick start test: only 'mktemp -d' is shimmed, not: mktemp $*\" >&2; exit 2; }}\n\
             exec '{}' -d \"$TMPDIR/tmp.XXXXXXXXXX\"\n",
            real.display()
        ),
    )
    .expect("write the mktemp shim");
    let mut perms = std::fs::metadata(&shim).expect("the shim exists").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&shim, perms).expect("make the shim executable");
}

#[test]
fn readme_quick_start_runs() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    let readme = std::fs::read_to_string(&readme_path).expect("read README.md");
    let blocks = quick_start_blocks(&readme);
    let script = script(&blocks);
    let expected = expected_weave_output(&blocks);

    let tmp = tempfile::tempdir().expect("a temp dir");
    let (bin, home, scratch, work) =
        (tmp.path().join("bin"), tmp.path().join("home"), tmp.path().join("tmp"), tmp.path().join("work"));
    for dir in [&bin, &home, &scratch, &work] {
        std::fs::create_dir_all(dir).expect("make the test's dirs");
    }
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_bracken"), bin.join("bracken")).expect("link bracken");
    let system_path = std::env::var("PATH").expect("PATH is set");
    // The real mktemp, for the shim: the first one on the inherited PATH.
    let real_mktemp = std::env::split_paths(&system_path)
        .map(|dir| dir.join("mktemp"))
        .find(|path| path.is_file())
        .expect("mktemp is on PATH");
    write_mktemp_shim(&bin, &real_mktemp);

    // A clean environment: nothing from the caller's Ground, identity or team server leaks in.
    let out = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(&work)
        .env_clear()
        .env("PATH", format!("{}:{system_path}", bin.display()))
        .env("HOME", &home)
        .env("TMPDIR", &scratch)
        .env("USER", "newcomer")
        .env("LOGNAME", "newcomer")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "newcomer")
        .env("GIT_AUTHOR_EMAIL", "newcomer@example.invalid")
        .env("GIT_COMMITTER_NAME", "newcomer")
        .env("GIT_COMMITTER_EMAIL", "newcomer@example.invalid")
        .output()
        .expect("bash runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    for i in 0..blocks.len() {
        assert!(
            stdout.lines().any(|line| line == marker(i)),
            "README's quick start block {} failed (exit {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
            i + 1,
            out.status.code()
        );
    }
    assert!(out.status.success(), "the quick start exited {:?}\nstderr:\n{stderr}", out.status.code());

    let weave = stdout
        .split_once(&format!("{}\n", marker(0)))
        .and_then(|(_, rest)| rest.split_once(&format!("{}\n", marker(1))))
        .map(|(weave, _)| weave)
        .expect("both markers are in stdout");
    assert!(
        weave.ends_with(&expected),
        "the Weave block must end by printing {expected:?}\nits stdout:\n{weave}\nstderr:\n{stderr}"
    );
}
