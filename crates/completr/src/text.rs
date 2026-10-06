//! Text normalisation, matching Python's `str.lower`, `str.strip` and `str.split`.

/// Python's `str.isspace`: Unicode White_Space plus the ASCII separators U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub(crate) fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// [`lower`] into `out`, reusing its buffer.
pub(crate) fn lower_into(s: &str, out: &mut String) {
    out.clear();
    if s.is_ascii() {
        out.push_str(s);
        out.make_ascii_lowercase();
    } else {
        out.push_str(&s.to_lowercase());
    }
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

/// The name in `names` closest to `name`, ignoring case, if it is a plausible typo of it.
pub(crate) fn closest<'a>(name: &str, names: &'a [String]) -> Option<&'a str> {
    let wanted: Vec<char> = lower(name).chars().collect();
    let budget = (wanted.len() / 4).clamp(1, 3);
    names
        .iter()
        .map(|n| {
            (
                edit_distance(&wanted, &lower(n).chars().collect::<Vec<_>>()),
                n,
            )
        })
        .filter(|&(d, _)| d <= budget)
        .min_by_key(|&(d, _)| d)
        .map(|(_, n)| n.as_str())
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diagonal + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
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
