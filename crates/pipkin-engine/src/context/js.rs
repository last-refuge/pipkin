//! JavaScript string semantics the reference relies on: `String.length` (UTF-16 code units),
//! `slice(0, n)`, `trim()` and the regex class `\s`.

/// `String.prototype.length`: UTF-16 code units.
pub fn len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `text.slice(0, units)` without splitting a surrogate pair (a pair that would be split is
/// left out entirely, where JavaScript would keep a lone high surrogate).
pub fn prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        let next = used + ch.len_utf16();
        if next > units {
            return &text[..index];
        }
        used = next;
    }
    text
}

/// The ECMAScript `WhiteSpace` and `LineTerminator` sets (`\s`, and what `trim()` removes).
pub fn is_space(ch: char) -> bool {
    ('\u{2000}'..='\u{200A}').contains(&ch)
        || matches!(
            ch,
            '\u{0009}'
                | '\u{000A}'
                | '\u{000B}'
                | '\u{000C}'
                | '\u{000D}'
                | '\u{0020}'
                | '\u{00A0}'
                | '\u{1680}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
        )
}

/// `String.prototype.trim()`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `stripBom()`: drop one leading U+FEFF.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// Decode file bytes the way Node's `readFileSync(path, "utf-8")` does (invalid sequences
/// become U+FFFD; a BOM is kept).
pub fn decode_utf8(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_prefixes_count_utf16_units() {
        assert_eq!(len("abc"), 3);
        assert_eq!(len("é"), 1);
        assert_eq!(len("🎉"), 2);
        assert_eq!(prefix("a🎉b", 1), "a");
        assert_eq!(prefix("a🎉b", 2), "a");
        assert_eq!(prefix("a🎉b", 3), "a🎉");
        assert_eq!(prefix("ab", 10), "ab");
    }

    #[test]
    fn trim_matches_javascript() {
        assert_eq!(trim("\u{FEFF}\u{00A0} x \n"), "x");
        // U+0085 is Unicode White_Space but not JavaScript whitespace.
        assert_eq!(trim("\u{0085}x"), "\u{0085}x");
        assert!(is_space('\u{2005}') && is_space('\u{200A}') && !is_space('\u{200B}'));
        assert_eq!(strip_bom("\u{FEFF}\u{FEFF}x"), "\u{FEFF}x");
    }
}
