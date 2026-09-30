//! Byte ranges of a text that a query matched, for bold rendering in a completion list.

use std::ops::Range;

use crate::text;

/// Words of `text` with their byte ranges.
fn spans(text: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (text::is_space(c), start) {
            (true, Some(s)) => {
                out.push((s..i, text::lower(&text[s..i])));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    out
}

/// Byte length of the first `chars` chars of `word`.
fn prefix_bytes(word: &str, chars: usize) -> usize {
    word.char_indices()
        .nth(chars)
        .map_or(word.len(), |(i, _)| i)
}

/// Matched ranges of `text` for `query`: word prefixes, then corrections within `max_edits`, then
/// run-together words. Ranges are sorted and do not overlap.
pub(crate) fn highlights(text: &str, query: &str, max_edits: usize) -> Vec<Range<usize>> {
    let words = spans(text);
    let mut used = vec![false; words.len()];
    let mut out = Vec::new();
    for q in text::words(&text::lower(query)) {
        let q_chars = text::char_len(q);
        let prefix = (0..words.len()).find(|&i| !used[i] && words[i].1.starts_with(q));
        if let Some(i) = prefix {
            used[i] = true;
            let range = &words[i].0;
            out.push(range.start..range.start + prefix_bytes(&text[range.clone()], q_chars));
            continue;
        }
        // Short words would match almost anything within a few edits.
        let edits = max_edits.min(q_chars / 3);
        let corrected = (edits > 0)
            .then(|| {
                (0..words.len()).filter(|&i| !used[i]).find(|&i| {
                    let word = &words[i].1;
                    let head: String = word.chars().take(q_chars).collect();
                    let distance =
                        |w: &str| rapidfuzz::distance::osa::distance(q.chars(), w.chars());
                    distance(&head).min(distance(word)) <= edits
                })
            })
            .flatten();
        if let Some(i) = corrected {
            used[i] = true;
            out.push(words[i].0.clone());
            continue;
        }
        // Run-together input such as "datascience": consecutive words that spell it out.
        'starts: for start in 0..words.len() {
            let mut rest = q;
            let mut end = start;
            while end < words.len() && !used[end] && !rest.is_empty() {
                let w = words[end].1.as_str();
                if rest.starts_with(w) {
                    rest = &rest[w.len()..];
                } else if w.starts_with(rest) {
                    rest = "";
                } else {
                    continue 'starts;
                }
                end += 1;
            }
            if rest.is_empty() && end - start > 1 {
                for i in start..end {
                    used[i] = true;
                    out.push(words[i].0.clone());
                }
                break;
            }
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(text: &str, query: &str) -> String {
        let mut out = String::new();
        let mut last = 0;
        for r in highlights(text, query, 2) {
            out.push_str(&text[last..r.start]);
            out.push('[');
            out.push_str(&text[r.clone()]);
            out.push(']');
            last = r.end;
        }
        out.push_str(&text[last..]);
        out
    }

    #[test]
    fn marks_prefixes_corrections_and_run_together_words() {
        assert_eq!(marked("Machine Learning", "mach"), "[Mach]ine Learning");
        assert_eq!(marked("Data Science", "science"), "Data [Science]");
        assert_eq!(
            marked("Machine Learning", "machne lerning"),
            "[Machine] [Learning]"
        );
        assert_eq!(marked("Data Science", "datascience"), "[Data] [Science]");
        assert_eq!(marked("Machine Learning", "data"), "Machine Learning");
        assert_eq!(marked("Ärzte Übersicht", "ärz üb"), "[Ärz]te [Üb]ersicht");
    }
}
