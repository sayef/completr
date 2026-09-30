//! Text normalisation, matching Python's `str.lower`, `str.strip` and `str.split`.

/// Python's `str.isspace`: Unicode White_Space plus the ASCII separators U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub(crate) fn lower(s: &str) -> String {
    s.to_lowercase()
}

pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub(crate) fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|w| !w.is_empty())
}

pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_python() {
        let got: Vec<_> = words("  a\u{1f}b\u{a0}c\t d ").collect();
        assert_eq!(got, ["a", "b", "c", "d"]);
        assert_eq!(strip("\u{1c} x y \n"), "x y");
    }

    #[test]
    fn lowercases_unicode() {
        assert_eq!(lower("ÄRZTE İ ΟΔΟΣ"), "ärzte i̇ οδος");
        assert_eq!(char_len("ärzte"), 5);
    }
}
