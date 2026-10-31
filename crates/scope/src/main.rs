//! `bracken-scope`, run in a git repository, for `scripts/code-scope`:
//!
//! - `bracken-scope comment-only <base> <head> <file>`: whether `<file>` changed from `<base>` to `<head>` only
//!   in comments (see the library). It prints `yes` and exits 0, or `no: <why>` and exits 1; 2 on an error.
//!   Only a file changed in place can be comment-only: a regular file on both sides with its mode kept, never
//!   one added, deleted or renamed, nor one a file at `<head>` includes as text. Should an include at `<head>`
//!   not name its file plainly, no file is.
//! - `bracken-scope packages <base> <head>`, the changed paths on standard input, NUL-separated: the
//!   workspace packages the change affects, one a line and sorted, maybe none (see the library's
//!   `packages`); `bracken-scope packages --all <head>`: every workspace package at `<head>`. It exits 0
//!   having printed them, and 2 on an error.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use bracken_scope::packages;

const USAGE: &str = "usage: bracken-scope comment-only <base> <head> <file> | \
                     bracken-scope packages <base> <head> (the changed paths on stdin, NUL-separated) | \
                     bracken-scope packages --all <head>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((command, rest)) if command == "comment-only" => comment_only(rest),
        Some((command, rest)) if command == "packages" => match affected(rest) {
            Ok(names) => {
                for name in names {
                    println!("{name}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bracken-scope: {e:#}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `bracken-scope packages <args>`: the packages, by name and sorted.
fn affected(args: &[String]) -> Result<Vec<String>> {
    let repo = toplevel()?;
    match args {
        [all, head] if all == "--all" => Ok(packages::all(&packages::graph_at(&repo, head)?)),
        [base, head] if !base.starts_with('-') && !head.starts_with('-') => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).context("reading the changed paths")?;
            let text = String::from_utf8(buf).context("a changed path isn't UTF-8")?;
            let paths: Vec<&str> = text.split('\0').filter(|p| !p.is_empty()).collect();
            if paths.is_empty() {
                return Ok(Vec::new());
            }
            let old = packages::graph_at(&repo, base)?;
            let new = packages::graph_at(&repo, head)?;
            packages::affected(&old, &new, &packages::rules()?, paths)
        }
        _ => bail!("{USAGE}"),
    }
}

/// The top of the checkout this runs in.
fn toplevel() -> Result<PathBuf> {
    let out = git(&["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(String::from_utf8(out).context("the checkout's path isn't UTF-8")?.trim_end()))
}

/// `bracken-scope comment-only <base> <head> <file>`.
fn comment_only(args: &[String]) -> ExitCode {
    let [base, head, file] = args else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match why_not(base, head, file) {
        Ok(None) => {
            println!("yes");
            ExitCode::SUCCESS
        }
        Ok(Some(why)) => {
            println!("no: {why}");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("bracken-scope: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn git(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git").args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// The file `path` at `rev`, if it's UTF-8.
fn read(rev: &str, path: &str) -> Result<Option<String>> {
    Ok(String::from_utf8(git(&["cat-file", "blob", &format!("{rev}:{path}")])?).ok())
}

/// Why `file`'s change from `base` to `head` isn't comment-only, or None when it is.
fn why_not(base: &str, head: &str, file: &str) -> Result<Option<String>> {
    if !file.ends_with(".rs") {
        return Ok(Some("not a Rust file".into()));
    }
    // One raw record, -z: ':<old mode> <new mode> <old blob> <new blob> <status>', NUL, the path, NUL.
    let raw = git(&["diff", "--no-renames", "--raw", "-z", base, head, "--", file])?;
    let raw = String::from_utf8_lossy(&raw);
    let fields: Vec<&str> = raw.split('\0').filter(|f| !f.is_empty()).collect();
    let [record, path] = fields.as_slice() else {
        return Ok(Some("not changed, or more than one path".into()));
    };
    let parts: Vec<&str> = record.trim_start_matches(':').split(' ').collect();
    let [old_mode, new_mode, _, _, status] = parts.as_slice() else { bail!("a raw diff record {record:?}") };
    if *path != file || *status != "M" {
        return Ok(Some("added or deleted".into()));
    }
    if old_mode != new_mode || !matches!(*old_mode, "100644" | "100755") {
        return Ok(Some("not a regular file with its mode kept".into()));
    }

    // Whether a file at <head> includes it as text. git grep exits 1 when nothing matches.
    let out = Command::new("git")
        .args(["grep", "-lz", "-E", "include_(str|bytes)!", head, "--", "*.rs"])
        .output()
        .context("running git grep")?;
    if !matches!(out.status.code(), Some(0 | 1)) {
        bail!("git grep: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    for entry in String::from_utf8_lossy(&out.stdout).split('\0') {
        let Some(including) = entry.strip_prefix(head).and_then(|e| e.strip_prefix(':')) else { continue };
        let found = read(head, including)?.and_then(|source| bracken_scope::included(including, &source));
        match found {
            None => return Ok(Some(format!("{including} includes a file by more than a plain path"))),
            Some(found) if found.iter().any(|p| p == file) => {
                return Ok(Some(format!("{including} includes it as text")));
            }
            Some(_) => {}
        }
    }

    match (read(base, file)?, read(head, file)?) {
        (Some(b), Some(h)) if bracken_scope::comment_only(&b, &h) => Ok(None),
        (Some(_), Some(_)) => Ok(Some("its tokens changed, or a side doesn't lex".into())),
        _ => Ok(Some("not UTF-8".into())),
    }
}
