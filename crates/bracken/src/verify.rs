//! `bracken evidence <target> --run -- <command> [args]`: verify and record in one step (owner decision
//! f3405c284180, change B). The command runs without a shell. Its exit code is the verdict, 0 a pass and
//! anything else a fail. Its command line is the method, and its exit and the tail of its output are the
//! details, as `scripts/bk-evidence` records them for this team.

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use manyroot::Verdict;

/// Lines of output the details keep, from the end.
pub const TAIL_LINES: usize = 40;
/// Bytes of output the details keep, at most.
pub const TAIL_BYTES: usize = 8000;
/// Output held while the command runs: enough for the tail, whatever its lines' lengths.
const HELD: usize = 64 * 1024;

/// What a run gives the evidence, and the exit status to pass on.
pub struct Outcome {
    pub verdict: Verdict,
    pub method: String,
    pub details: String,
    /// The command's exit code, or 128 plus the signal that ended it.
    pub status: u8,
}

/// Runs `command` (program and arguments, no shell) in the current directory, with no input, its
/// output streamed to stderr as it comes. Refuses, so that nothing is recorded, when it can't start.
///
/// Its stdout and stderr are one pipe, as `2>&1` makes them, so the details hold the output in the
/// order the command wrote it (note 4a2c53c065a9: two pipes read on two threads interleave by chance).
pub fn run(command: &[String]) -> Result<Outcome> {
    let Some((program, args)) = command.split_first() else { bail!("--run needs a command after --") };
    let (mut output, into) = io::pipe()?;
    let mut child = {
        // The command holds the pipe's write ends until it's dropped here, so reading ends when the
        // command (and anything it left holding them) closes them.
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(into.try_clone()?).stderr(into);
        match cmd.spawn() {
            Ok(child) => child,
            Err(e) => bail!("couldn't start {program}: {e}; nothing recorded"),
        }
    };
    let mut held = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match output.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("reading {program}'s output: {e}; nothing recorded");
            }
        };
        let _ = io::stderr().write_all(&chunk[..n]);
        held.extend_from_slice(&chunk[..n]);
        if held.len() > 2 * HELD {
            held.drain(..held.len() - HELD);
        }
    }
    let exit = child.wait()?;
    let (ended, status) = match exit.code() {
        Some(code) => (format!("exit {code}"), u8::try_from(code).unwrap_or(1)),
        None => {
            #[cfg(unix)]
            let signal = std::os::unix::process::ExitStatusExt::signal(&exit).unwrap_or(0);
            #[cfg(not(unix))]
            let signal = 0;
            (format!("signal {signal}"), u8::try_from(128 + signal).unwrap_or(255))
        }
    };
    let verdict = if exit.success() { Verdict::Pass } else { Verdict::Fail };
    let details = format!("{ended}\n{}", tail(&printable(&held)));
    Ok(Outcome { verdict, method: command_line(command), details, status })
}

/// The command as one line a shell would read back the same: each argument as it is when that's
/// safe, else single-quoted.
pub fn command_line(command: &[String]) -> String {
    let quote = |a: &String| {
        let plain = !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./=:,+@%^".contains(&b));
        if plain { a.clone() } else { format!("'{}'", a.replace('\'', r"'\''")) }
    };
    printable(command.iter().map(quote).collect::<Vec<_>>().join(" ").as_bytes()).replace('\n', " ")
}

/// Output as text that can't drive a terminal: invalid UTF-8 replaced, and C0 controls except tab and
/// newline, DEL and C1 controls dropped.
pub fn printable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().filter(|&c| c == '\t' || c == '\n' || !(c.is_control())).collect()
}

/// The last [`TAIL_LINES`] lines of `text`, cut to its last [`TAIL_BYTES`] bytes; a line the cut
/// starts inside is dropped, unless it's the only one.
pub fn tail(text: &str) -> String {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let kept = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    if kept.len() <= TAIL_BYTES {
        return kept;
    }
    let mut start = kept.len() - TAIL_BYTES;
    while !kept.is_char_boundary(start) {
        start += 1;
    }
    let cut = &kept[start..];
    match cut.split_once('\n') {
        Some((_, rest)) if !rest.is_empty() => rest.to_string(),
        _ => cut.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_last_lines_and_bytes() {
        let many: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let t = tail(&many);
        assert_eq!(
            (t.lines().count(), t.lines().next(), t.lines().last()),
            (TAIL_LINES, Some("line 60"), Some("line 99"))
        );
        let long = format!("{}\nlast", "x".repeat(20_000));
        assert_eq!(tail(&long), "last", "a line the cut starts inside is dropped");
        let one = "é".repeat(9_000);
        let t = tail(&one);
        assert!(t.len() <= TAIL_BYTES && t.chars().all(|c| c == 'é'), "one line is cut on a character boundary");
    }

    #[test]
    fn output_and_command_lines_are_printable() {
        assert_eq!(printable(b"ok\x1b[2K\r\n\x07bell\t\xff\xc2\x9b"), "ok[2K\nbell\t\u{fffd}");
        let line = command_line(&["cargo".into(), "test".into(), "--".into(), "a b".into(), "it's".into(), "".into()]);
        assert_eq!(line, r"cargo test -- 'a b' 'it'\''s' ''");
    }
}
