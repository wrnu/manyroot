use super::shown;

/// The grader's own pass escapes the Khmer inherent vowels and U+FFF0-FFF8 by itself, not only after the host's
/// (finding 3c11f5c52e8e): none of them is a control character to `char::is_control`, so only `is_format`'s ranges
/// catch them, and the host never sees this text.
#[test]
fn the_grader_escapes_the_khmer_vowels_and_fff0_to_fff8_by_itself() {
    for c in ['\u{17b4}', '\u{17b5}'].into_iter().chain('\u{fff0}'..='\u{fff8}') {
        assert!(!c.is_control(), "{c:?}");
        assert_eq!(shown(&format!("a{c}b"), 50), format!("a{}b", c.escape_unicode()), "{c:?}");
    }
}
