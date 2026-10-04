//! SymSpell primitives: delete variants and bounded Levenshtein distance.

use rustc_hash::FxHashSet;

/// All strings reachable from the first `prefix_length` chars of `word` by up to `max_distance`
/// deletions, including the truncated word itself.
pub(crate) fn delete_variants(
    word: &str,
    max_distance: u8,
    prefix_length: usize,
) -> FxHashSet<String> {
    let root: Vec<char> = word.chars().take(prefix_length).collect();
    let mut seen: FxHashSet<Vec<char>> = FxHashSet::default();
    seen.insert(root.clone());
    let mut queue = vec![root];
    for _ in 0..max_distance {
        let mut next = Vec::new();
        for item in &queue {
            if item.len() <= 1 {
                continue;
            }
            for i in 0..item.len() {
                let mut variant = item.clone();
                variant.remove(i);
                if seen.insert(variant.clone()) {
                    next.push(variant);
                }
            }
        }
        queue = next;
    }
    seen.into_iter()
        .map(|chars| chars.into_iter().collect())
        .collect()
}

/// Whether `variant` is one of `word`'s [`delete_variants`]: a subsequence of its first
/// `prefix_length` chars missing at most `max_distance` of them, and not empty unless they are.
pub(crate) fn is_variant(
    variant: &str,
    word: &str,
    max_distance: u8,
    prefix_length: usize,
) -> bool {
    let mut rest = variant.chars().peekable();
    let (mut len, mut kept) = (0usize, 0usize);
    for c in word.chars().take(prefix_length) {
        len += 1;
        if rest.peek() == Some(&c) {
            rest.next();
            kept += 1;
        }
    }
    rest.peek().is_none() && len - kept <= max_distance as usize && (kept > 0 || len == 0)
}

/// Calls `f` with each of `word`'s [`delete_variants`], without allocating; a variant reachable
/// in several ways comes several times.
pub(crate) fn for_each_delete_variant(
    word: &str,
    max_distance: u8,
    prefix_length: usize,
    mut f: impl FnMut(&str),
) {
    let mut chars = [char::default(); 64];
    let mut n = 0;
    for c in word.chars().take(prefix_length) {
        if n == chars.len() {
            return delete_variants(word, max_distance, prefix_length)
                .iter()
                .for_each(|v| f(v));
        }
        chars[n] = c;
        n += 1;
    }
    let mut buf = String::with_capacity(n * 4);
    // Deleting from a single char is not allowed, so at least one char stays.
    let most = (max_distance as usize).min(n.saturating_sub(1));
    let mut removed = [0usize; 8];
    fn visit(
        chars: &[char],
        removed: &mut [usize],
        depth: usize,
        start: usize,
        most: usize,
        buf: &mut String,
        f: &mut impl FnMut(&str),
    ) {
        buf.clear();
        let mut skip = removed[..depth].iter().peekable();
        for (i, &c) in chars.iter().enumerate() {
            if skip.peek() == Some(&&i) {
                skip.next();
            } else {
                buf.push(c);
            }
        }
        f(buf);
        if depth < most {
            for i in start..chars.len() {
                removed[depth] = i;
                visit(chars, removed, depth + 1, i + 1, most, buf, f);
            }
        }
    }
    if most > removed.len() {
        return delete_variants(word, max_distance, prefix_length)
            .iter()
            .for_each(|v| f(v));
    }
    visit(&chars[..n], &mut removed, 0, 0, most, &mut buf, &mut f);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn variants_match_symspell() {
        let v = delete_variants("abc", 2, 7);
        let mut got: Vec<_> = v.into_iter().collect();
        got.sort();
        assert_eq!(got, ["a", "ab", "abc", "ac", "b", "bc", "c"]);
        assert!(delete_variants("machinery", 1, 7).contains("machine"));
        assert_eq!(delete_variants("a", 2, 7).len(), 1);
    }

    #[test]
    fn is_variant_matches_variant_sets() {
        let words = [
            "",
            "a",
            "ab",
            "ba",
            "abc",
            "cab",
            "kitten",
            "sitting",
            "mitten",
            "ärzte",
            "arzt",
            "aab",
            "日本語",
        ];
        for q in words {
            for d in 0..3 {
                for pc in [2, 7] {
                    let variants = delete_variants(q, d, pc);
                    for v in words {
                        let expected = variants.contains(v);
                        assert_eq!(is_variant(v, q, d, pc), expected, "{v:?} {q:?} {d} {pc}");
                    }
                    for v in &variants {
                        assert!(is_variant(v, q, d, pc), "{v:?} {q:?} {d} {pc}");
                    }
                }
            }
        }
    }

    #[test]
    fn variant_generator_matches_variant_sets() {
        for w in [
            "",
            "a",
            "ab",
            "aab",
            "abc",
            "kitten",
            "machinery",
            "ärzte",
            "日本語",
        ] {
            for d in 0..4 {
                for pc in [2, 7] {
                    let mut got = FxHashSet::default();
                    for_each_delete_variant(w, d, pc, |v| {
                        got.insert(v.to_owned());
                    });
                    assert_eq!(got, delete_variants(w, d, pc), "{w:?} {d} {pc}");
                }
            }
        }
    }

    #[test]
    fn rapidfuzz_matches_full_distance() {
        fn full(a: &[char], b: &[char]) -> usize {
            let mut prev: Vec<usize> = (0..=b.len()).collect();
            for (i, &ca) in a.iter().enumerate() {
                let mut curr = vec![i + 1; b.len() + 1];
                for (j, &cb) in b.iter().enumerate() {
                    curr[j + 1] = (prev[j] + usize::from(ca != cb))
                        .min(prev[j + 1] + 1)
                        .min(curr[j] + 1);
                }
                prev = curr;
            }
            prev[b.len()]
        }
        let words = [
            "",
            "a",
            "ab",
            "ba",
            "abc",
            "acb",
            "kitten",
            "sitting",
            "sittin",
            "datadog",
            "data dog",
            "ärzte",
            "arzt",
            "日本語",
            "日本",
        ];
        for a in words {
            let comparator = rapidfuzz::distance::levenshtein::BatchComparator::new(a.chars());
            for b in words {
                for max in 0..4 {
                    let args = rapidfuzz::distance::levenshtein::Args::default().score_cutoff(max);
                    let expected = Some(full(&chars(a), &chars(b))).filter(|&d| d <= max);
                    assert_eq!(
                        comparator.distance_with_args(b.chars(), &args),
                        expected,
                        "{a:?} {b:?} {max}"
                    );
                }
            }
        }
    }
}
