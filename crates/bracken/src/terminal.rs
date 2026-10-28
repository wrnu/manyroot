//! Text from the Ground, made safe to print on a terminal.
//!
//! Statements, tags and names come from other agents. Printed as they are, a carriage return, an
//! escape sequence or a line separator in one could overwrite, forge or hide lines of a listing
//! (finding 252f052a283d). `--json` output keeps the text as it is: JSON escapes it.

/// `text` with every character that could move the cursor, break the line or reorder what a terminal
/// shows written as a visible escape (`\u{1b}`): control characters (C0, DEL and C1, so CR, ESC, VT, FF,
/// NEL, tab and newline too), the line and paragraph separators, and the bidirectional overrides and
/// isolates.
pub fn printable(text: &str) -> String {
    let unsafe_char = |c: char| {
        c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    if !text.chars().any(unsafe_char) {
        return text.to_owned();
    }
    text.chars().map(|c| if unsafe_char(c) { format!("\\u{{{:x}}}", c as u32) } else { c.to_string() }).collect()
}

#[cfg(test)]
mod tests {
    use super::printable;

    #[test]
    fn printable_escapes_what_could_forge_a_line() {
        assert_eq!(printable("plain é 文"), "plain é 文");
        assert_eq!(
            printable("a\rb\x1b[2Kc\u{85}d\u{2028}e\u{202e}f\n"),
            "a\\u{d}b\\u{1b}[2Kc\\u{85}d\\u{2028}e\\u{202e}f\\u{a}"
        );
    }

    #[test]
    fn control_characters_and_separators_become_visible() {
        let forged = "ok\r\u{1b}[2Kfake\u{b}\u{c}\u{85}\u{2028}\u{2029}\u{202e}\u{7f}\u{0}\tend\nnext";
        assert_eq!(
            printable(forged),
            "ok\\u{d}\\u{1b}[2Kfake\\u{b}\\u{c}\\u{85}\\u{2028}\\u{2029}\\u{202e}\\u{7f}\\u{0}\\u{9}end\\u{a}next"
        );
        assert_eq!(printable("plain text, émoji 🌱 and CJK 漢字"), "plain text, émoji 🌱 and CJK 漢字");
    }
}
