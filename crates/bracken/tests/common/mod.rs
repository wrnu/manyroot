//! Shared by the CLI's integration tests.

/// What a write command printed, as bare ids, one per line: each labelled line (`claim <id>`,
/// `relation <id> (supersedes)`) becomes its id, so a test can pass it on as an argument. Any other
/// output comes back unchanged. The labelled form itself is pinned by
/// `cli.rs`'s `write_commands_print_one_labelled_line_per_record`.
pub fn bare_ids(out: String) -> String {
    let id = |line: &str| -> Option<String> {
        let mut words = line.splitn(3, ' ');
        let kind = words.next()?;
        let id = words.next()?;
        let known = ["claim", "artifact", "relation", "evidence", "retract"].contains(&kind);
        let hex = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
        let rest_ok = words.next().is_none_or(|r| kind == "relation" && r.starts_with('(') && r.ends_with(')'));
        (known && hex && rest_ok).then(|| id.to_owned())
    };
    match out.lines().map(id).collect::<Option<Vec<String>>>() {
        Some(ids) if !ids.is_empty() => ids.join("\n"),
        _ => out,
    }
}
