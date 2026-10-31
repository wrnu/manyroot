//! Whether a change to a Rust file is comment-only, for `scripts/code-scope` (task `0837da3df7a4`): a file
//! whose tokens are the same on both sides, comments left out, compiles as before, so it never needs the
//! full table. Clippy still reads comments (finding `c40ed7b8d3f8`: one starting an inner `if` keeps
//! `collapsible_if` quiet), so the caller still lints it.
//!
//! Doc comments are tokens (`#[doc = "…"]`): they reach rustdoc and doctests, so changing one changes code.
//! So does anything else the lexer keeps: a string literal, an attribute, a `cfg`. A side that doesn't lex is
//! never comment-only. Two things outside the tokens are checked too:
//! - whether a blank line follows each outer attribute, doc comments included, since clippy's
//!   `empty_line_after_outer_attr` and `empty_line_after_doc_comments` read it;
//! - whether the file is included as text (`include_str!`, `include_bytes!`), where every byte is data. The
//!   caller asks [`included`] for that.
//!
//! A file read as text at run time, by path, isn't seen here.

use std::str::FromStr;

use proc_macro2::{Delimiter, LineColumn, Spacing, TokenStream, TokenTree};

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Open(Delimiter),
    Close(Delimiter),
    Ident(String),
    Punct(char, Spacing),
    Literal(String),
}

/// A file's tokens, groups flattened into their delimiters, each with where it starts and ends.
struct Lexed {
    tokens: Vec<Token>,
    spans: Vec<(LineColumn, LineColumn)>,
}

impl Lexed {
    fn new(source: &str) -> Option<Lexed> {
        let stream = TokenStream::from_str(source).ok()?;
        let mut lexed = Lexed { tokens: Vec::new(), spans: Vec::new() };
        lexed.flatten(stream);
        Some(lexed)
    }

    fn flatten(&mut self, stream: TokenStream) {
        for tree in stream {
            match tree {
                TokenTree::Group(g) => {
                    self.push(Token::Open(g.delimiter()), g.span_open());
                    self.flatten(g.stream());
                    self.push(Token::Close(g.delimiter()), g.span_close());
                }
                TokenTree::Ident(i) => self.push(Token::Ident(i.to_string()), i.span()),
                TokenTree::Punct(p) => self.push(Token::Punct(p.as_char(), p.spacing()), p.span()),
                TokenTree::Literal(l) => self.push(Token::Literal(l.to_string()), l.span()),
            }
        }
    }

    fn push(&mut self, token: Token, span: proc_macro2::Span) {
        self.tokens.push(token);
        self.spans.push((span.start(), span.end()));
    }

    /// The index of the delimiter that closes the one opened at `open`.
    fn close_of(&self, open: usize) -> usize {
        let mut depth = 0usize;
        for (i, token) in self.tokens.iter().enumerate().skip(open) {
            match token {
                Token::Open(_) => depth += 1,
                Token::Close(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        self.tokens.len() - 1
    }

    /// For each outer attribute, in order, whether a blank line separates it from the token after it.
    fn blank_after_attributes(&self, source: &str) -> Vec<bool> {
        let starts: Vec<usize> = std::iter::once(0).chain(source.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let offset = |at: LineColumn| {
            let line = starts.get(at.line.wrapping_sub(1)).copied().unwrap_or(source.len());
            let rest = &source[line..];
            line + rest.char_indices().nth(at.column).map_or(rest.len(), |(i, _)| i)
        };
        let mut blanks = Vec::new();
        for i in 0..self.tokens.len() {
            let outer = matches!(self.tokens[i], Token::Punct('#', _))
                && self.tokens.get(i + 1) == Some(&Token::Open(Delimiter::Bracket));
            if !outer {
                continue;
            }
            let close = self.close_of(i + 1);
            let Some(next) = self.spans.get(close + 1) else { continue };
            let gap = source.get(offset(self.spans[close].1)..offset(next.0)).unwrap_or("");
            let lines: Vec<&str> = gap.split('\n').collect();
            blanks.push(lines.len() > 2 && lines[1..lines.len() - 1].iter().any(|l| l.trim().is_empty()));
        }
        blanks
    }
}

/// Whether `head` differs from `base` only in comments and in whitespace nothing reads: the same tokens, and a
/// blank line after the same outer attributes. False when either doesn't lex.
pub fn comment_only(base: &str, head: &str) -> bool {
    let (Some(b), Some(h)) = (Lexed::new(base), Lexed::new(head)) else {
        return false;
    };
    b.tokens == h.tokens && b.blank_after_attributes(base) == h.blank_after_attributes(head)
}

/// The paths that `source`, the file at `path` (relative to the repository's root), includes as text with
/// `include_str!` or `include_bytes!`, resolved from its directory. None when it doesn't lex, or an include
/// names its file by anything but a plain string literal, since then any file might be the one.
pub fn included(path: &str, source: &str) -> Option<Vec<String>> {
    let lexed = Lexed::new(source)?;
    let t = &lexed.tokens;
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut paths = Vec::new();
    for i in 0..t.len() {
        let Token::Ident(name) = &t[i] else { continue };
        if !matches!(name.as_str(), "include_str" | "include_bytes")
            || !matches!(t.get(i + 1), Some(Token::Punct('!', _)))
        {
            continue;
        }
        let (Some(Token::Open(_)), Some(Token::Literal(literal)), Some(Token::Close(_))) =
            (t.get(i + 2), t.get(i + 3), t.get(i + 4))
        else {
            return None;
        };
        paths.push(resolve(dir, &plain_string(literal)?));
    }
    Some(paths)
}

/// The text of a string literal without escapes, plain or raw; None for any other literal.
fn plain_string(literal: &str) -> Option<String> {
    if let Some(inner) = literal.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
        return (!inner.contains('\\')).then(|| inner.to_owned());
    }
    let raw = literal.strip_prefix('r')?;
    let hashes = raw.len() - raw.trim_start_matches('#').len();
    let fence = "#".repeat(hashes);
    raw.strip_prefix(&format!("{fence}\""))?.strip_suffix(&format!("\"{fence}")).map(str::to_owned)
}

/// `relative` from the directory `dir`, `.` and `..` taken away.
fn resolve(dir: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = if relative.starts_with('/') { Vec::new() } else { dir.split('/').collect() };
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/");
    if relative.starts_with('/') { format!("/{joined}") } else { joined }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "/// Adds.\n#[inline]\nfn add(a: u8, b: u8) -> u8 {\n    // the sum\n    a + b\n}\n";

    #[test]
    fn comments_and_layout_are_comment_only() {
        let head = "/// Adds.\n#[inline]\nfn add(a: u8, b: u8) -> u8 {\n    /* the sum,\n       wrapping */ a +\n    b // here\n}\n";
        assert!(comment_only(BASE, head));
        assert!(comment_only(BASE, &format!("// A new first line.\n{BASE}")));
    }

    #[test]
    fn a_doc_comment_is_code() {
        assert!(!comment_only(BASE, &BASE.replace("/// Adds.", "/// Adds them.")));
        assert!(!comment_only(BASE, &BASE.replace("/// Adds.", "/** Adds. */")));
        assert!(!comment_only(BASE, &format!("//! The crate.\n{BASE}")));
        assert!(!comment_only(BASE, &BASE.replace("/// Adds.\n", "")));
    }

    #[test]
    fn a_literal_or_an_attribute_is_code() {
        let base = "#[cfg(unix)]\nconst S: &str = \"a\";\n";
        assert!(!comment_only(base, &base.replace("\"a\"", "\"b\"")));
        assert!(!comment_only(base, &base.replace("\"a\"", "r\"a\"")));
        assert!(!comment_only(base, &base.replace("unix", "windows")));
        assert!(!comment_only(base, &base.replace("#[cfg(unix)]\n", "")));
    }

    #[test]
    fn spacing_between_puncts_is_code() {
        assert!(!comment_only("fn f(mut a: u8) { a += 1; }", "fn f(mut a: u8) { a + /**/ = 1; }"));
    }

    #[test]
    fn a_side_that_doesnt_lex_is_code() {
        assert!(!comment_only("fn f() {", "fn f() { // x"));
        assert!(!comment_only("fn f() {}", "fn f() { \"}"));
        assert!(!comment_only("fn f() {} /* open", "fn f() {} /* open */"));
    }

    #[test]
    fn a_blank_line_after_an_outer_attribute_is_code() {
        let base = "/// Doc.\n#[derive(Debug)]\nstruct S;\n";
        assert!(!comment_only(base, &base.replace("/// Doc.\n", "/// Doc.\n\n")));
        assert!(!comment_only(base, &base.replace("]\nstruct", "]\n\nstruct")));
        assert!(comment_only(base, &base.replace("]\nstruct", "]\n// a note\nstruct")));
        let inner = "//! Crate.\n\nfn f() {}\n";
        assert!(comment_only(inner, &inner.replace("\n\n", "\n\n// More.\n\n")));
    }

    #[test]
    fn included_files_are_found_from_the_including_files_directory() {
        let source =
            "const A: &str = include_str!(\"a.rs\");\nconst B: &[u8] = include_bytes!(r#\"../../demo/src/b.rs\"#);\n";
        assert_eq!(
            included("crates/x/src/lib.rs", source),
            Some(vec!["crates/x/src/a.rs".to_owned(), "crates/demo/src/b.rs".to_owned()])
        );
        assert_eq!(included("lib.rs", "include_str!(\"./a.rs\")"), Some(vec!["a.rs".to_owned()]));
        assert_eq!(included("src/lib.rs", "include_str!(concat!(\"a\", \".rs\"))"), None);
        assert_eq!(included("src/lib.rs", "include_str!(\"a\\x2e.rs\")"), None);
        assert_eq!(included("src/lib.rs", "fn f() {"), None);
    }
}
