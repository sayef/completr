//! Ranking: exact, prefix, abbreviation, infix and fuzzy scores, with deterministic tie-breaks by id.

use std::cmp::Ordering;
use std::ops::Range;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::index::{char_count, current_filter, with_filter, Field};
use crate::{fuzzy, text, Index};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MatchKind {
    Exact,
    Prefix,
    Abbreviation,
    Infix,
    Fuzzy,
    /// Found by `vector_search`.
    Semantic,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Prefix => "prefix",
            Self::Abbreviation => "abbreviation",
            Self::Infix => "infix",
            Self::Fuzzy => "fuzzy",
            Self::Semantic => "semantic",
        }
    }
}

/// One completion: the document, how it matched, and which parts of its text matched.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Suggestion {
    pub id: u64,
    pub key: Option<String>,
    pub text: String,
    pub score: f64,
    pub kind: MatchKind,
    /// Byte ranges of `text` that matched the query, sorted, for highlighting.
    pub highlights: Vec<Range<usize>>,
}

/// A document found through one of its synonyms.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AliasSuggestion {
    pub id: u64,
    pub key: Option<String>,
    pub text: String,
    pub score: f64,
}

/// Per-request options: how many results, and which contexts they must be tagged with.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchOptions {
    pub limit: usize,
    /// Only documents tagged with any of these contexts; empty means all documents.
    pub contexts: Vec<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self::new(10)
    }
}

impl SearchOptions {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            contexts: Vec::new(),
        }
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn contexts<S: Into<String>>(mut self, contexts: impl IntoIterator<Item = S>) -> Self {
        self.contexts = contexts.into_iter().map(Into::into).collect();
        self
    }
}

/// Cached short-query results keep `f32` scores, as the precomputed tables they replace did.
#[derive(Clone, Copy)]
pub(crate) struct CachedHit {
    doc: u32,
    score: f32,
    kind: MatchKind,
}

/// Corrected phrases with their edit distance.
/// Corrected words or phrases with their edit distance and dictionary frequency.
type Corrections = Vec<(String, usize, u32)>;

/// Fuzzy lookups repeated within one request.
#[derive(Default)]
struct Memo {
    fuzzy: std::cell::RefCell<FxHashMap<(String, usize, bool), Corrections>>,
}

#[derive(Clone, Copy)]
pub(crate) struct RawHit {
    doc: u32,
    score: f64,
    kind: MatchKind,
}

#[derive(Clone, Copy, Default)]
struct Merged {
    from_prefix: bool,
    fuzzy_distance: usize,
    fuzzy_rank: Option<usize>,
    is_abbreviation: bool,
    is_exact: bool,
}

impl Index {
    /// Ranked completions for `query`: exact, prefix, abbreviation, infix and typo-tolerant matches.
    pub fn complete(&self, query: &str, limit: usize) -> Vec<Suggestion> {
        self.complete_with(query, &SearchOptions::new(limit))
    }

    pub fn complete_with(&self, query: &str, options: &SearchOptions) -> Vec<Suggestion> {
        let hits = with_filter(self.allowed(&options.contexts), || {
            self.ranked(query, options.limit)
        });
        hits.into_iter()
            .map(|(doc, score, kind)| self.suggestion(doc, score, kind, Some(query)))
            .collect()
    }

    /// Document numbers, scores and kinds of the best `limit` completions.
    fn ranked(&self, query: &str, limit: usize) -> Vec<(u32, f64, MatchKind)> {
        let lower = text::lower(query);
        let stripped = text::strip(&lower);
        if let Some(cached) = self.short_query(stripped) {
            return cached
                .iter()
                .take(limit)
                .map(|h| (h.doc, f64::from(h.score), h.kind))
                .collect();
        }
        let mut hits = self.raw_autocomplete(&Memo::default(), query, limit, 0);
        hits.truncate(limit);
        hits.into_iter().map(|h| (h.doc, h.score, h.kind)).collect()
    }

    pub(crate) fn suggestion(
        &self,
        doc: u32,
        score: f64,
        kind: MatchKind,
        query: Option<&str>,
    ) -> Suggestion {
        let text = self.doc_text(doc);
        let highlights = match (query, kind) {
            (
                Some(q),
                MatchKind::Exact | MatchKind::Prefix | MatchKind::Infix | MatchKind::Fuzzy,
            ) => crate::highlight::highlights(
                &text,
                q,
                self.segment_config.max_edit_distance as usize,
            ),
            _ => Vec::new(),
        };
        Suggestion {
            id: self.ids[doc as usize],
            key: self.doc_key(doc),
            text,
            score,
            kind,
            highlights,
        }
    }

    /// Documents with a synonym alias starting with `query`, ranked by alias length and weight.
    pub fn complete_aliases(&self, query: &str, limit: usize) -> Vec<AliasSuggestion> {
        self.complete_aliases_with(query, &SearchOptions::new(limit))
    }

    pub fn complete_aliases_with(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Vec<AliasSuggestion> {
        let limit = options.limit;
        let hits = with_filter(self.allowed(&options.contexts), || {
            let lower = text::lower(query);
            let stripped = text::strip(&lower);
            if self.is_short(stripped) {
                let hits = self.raw_aliases(stripped, self.config.short_query_limit);
                return hits
                    .into_iter()
                    .take(limit)
                    .map(|(doc, score)| (doc, f64::from(score as f32)))
                    .collect::<Vec<_>>();
            }
            self.raw_aliases(query, limit)
        });
        hits.into_iter()
            .map(|(doc, score)| AliasSuggestion {
                id: self.ids[doc as usize],
                key: self.doc_key(doc),
                text: self.doc_text(doc),
                score,
            })
            .collect()
    }

    /// Vector search restricted to `options.contexts`.
    pub fn vector_search_with(
        &self,
        query: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<Suggestion>, crate::Error> {
        with_filter(self.allowed(&options.contexts), || {
            self.vector_search(query, options.limit)
        })
    }

    fn is_short(&self, query: &str) -> bool {
        !query.is_empty()
            && text::char_len(query) <= self.config.short_query_chars
            && (self.has_prefix(Field::Title, query) || self.has_prefix(Field::Alias, query))
    }

    fn short_query(&self, query: &str) -> Option<Arc<[CachedHit]>> {
        if current_filter().is_some() || !self.is_short(query) {
            return None;
        }
        let cache = || self.short_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((hits, served)) = cache().get_mut(query) {
            *served = served.saturating_add(1);
            return Some(hits.clone());
        }
        let hits: Arc<[CachedHit]> = self
            .raw_autocomplete(&Memo::default(), query, self.config.short_query_limit, 0)
            .into_iter()
            .map(|h| CachedHit {
                doc: h.doc,
                score: h.score as f32,
                kind: h.kind,
            })
            .collect();
        let mut cache = cache();
        if cache.len() < self.config.short_query_cache_entries {
            cache
                .entry(query.to_owned())
                .or_insert_with(|| (hits.clone(), 1));
        }
        Some(hits)
    }

    fn by_weight(&self, a: u32, b: u32) -> Ordering {
        let (a, b) = (a as usize, b as usize);
        self.weights[b]
            .partial_cmp(&self.weights[a])
            .unwrap_or(Ordering::Equal)
            .then(self.ids[a].cmp(&self.ids[b]))
    }

    fn top_by_weight(&self, mut docs: Vec<u32>, k: usize) -> Vec<u32> {
        if k == 0 {
            return Vec::new();
        }
        if docs.len() > k {
            docs.select_nth_unstable_by(k - 1, |&a, &b| self.by_weight(a, b));
            docs.truncate(k);
        }
        docs.sort_unstable_by(|&a, &b| self.by_weight(a, b));
        docs
    }

    fn prefix_search(&self, query: &str, max_results: usize) -> Vec<u32> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = text::lower(query);
        let mut seen = FxHashSet::default();
        let mut docs = Vec::new();
        self.scan(Field::Title, &lower, max_results * 5, |_, doc, _| {
            if seen.insert(doc) {
                docs.push(doc);
            }
        });
        self.top_by_weight(docs, max_results)
    }

    fn infix_search(&self, query: &str, max_results: usize) -> Vec<u32> {
        let lower = text::lower(query);
        let min_chars = self.segment_config.min_word_chars as usize;
        let (words, short): (Vec<&str>, Vec<&str>) =
            text::words(&lower).partition(|w| text::char_len(w) >= min_chars);
        let docs = match words.as_slice() {
            [] => return Vec::new(),
            [word] => {
                let mut seen = FxHashSet::default();
                let mut docs = Vec::new();
                self.scan(Field::Word, word, max_results * 5, |_, doc, _| {
                    if seen.insert(doc) {
                        docs.push(doc);
                    }
                });
                docs
            }
            _ => {
                let mut sets: Vec<FxHashSet<u32>> = words
                    .iter()
                    .map(|word| {
                        let mut set = FxHashSet::default();
                        self.scan(Field::Word, word, usize::MAX, |_, doc, _| {
                            set.insert(doc);
                        });
                        set
                    })
                    .collect();
                sets.sort_by_key(|s| s.len());
                let (first, rest) = sets.split_first().unwrap();
                first
                    .iter()
                    .copied()
                    .filter(|doc| rest.iter().all(|s| s.contains(doc)))
                    .collect()
            }
        };
        if short.is_empty() {
            return self.top_by_weight(docs, max_results);
        }
        // Words too short to index must still start a word of the text.
        let mut docs = docs;
        docs.sort_unstable_by(|&a, &b| self.by_weight(a, b));
        docs.into_iter()
            .filter(|&doc| {
                let text = text::lower(&self.doc_text(doc));
                short
                    .iter()
                    .all(|s| text::words(&text).any(|w| w.starts_with(s)))
            })
            .take(max_results)
            .collect()
    }

    /// Dictionary words within the edit distance of `query`, closest and most frequent first.
    /// With `prefix`, words are compared by their first `query` chars, and those prefixes are
    /// returned: corrections of a word still being typed.
    fn fuzzy_search(
        &self,
        memo: &Memo,
        query: &str,
        max_results: usize,
        prefix: bool,
    ) -> Corrections {
        if !self.has_words || query.is_empty() {
            return Vec::new();
        }
        let lower = text::lower(query);
        let key = (lower, max_results, prefix);
        if let Some(hit) = memo.fuzzy.borrow().get(&key) {
            return hit.clone();
        }
        let lower = &key.0;
        let config = self.segment_config;
        let max_distance = config.max_edit_distance as usize;
        let variants = fuzzy::delete_variants(
            lower,
            config.max_edit_distance,
            config.fuzzy_prefix_chars as usize,
        );
        let query_len = lower.chars().count();
        // Optimal string alignment: a transposition is one edit, as in SymSpell.
        let comparator = rapidfuzz::distance::osa::BatchComparator::new(lower.chars());
        let args = rapidfuzz::distance::osa::Args::default().score_cutoff(max_distance);
        let mut prefixes: FxHashMap<&str, u32> = FxHashMap::default();
        let mut matches: Vec<(&str, usize, u32)> = Vec::new();
        for (word, freq) in self.fuzzy_candidates(&variants) {
            let word_len = char_count(word.as_bytes());
            if freq == 0 {
                continue;
            }
            let compared = if prefix && word_len > query_len {
                let end = word
                    .char_indices()
                    .nth(query_len)
                    .map_or(word.len(), |(i, _)| i);
                &word[..end]
            } else {
                word
            };
            if char_count(compared.as_bytes()).abs_diff(query_len) > max_distance {
                continue;
            }
            if prefix {
                // Words sharing a prefix pool their frequency under it.
                *prefixes.entry(compared).or_default() += freq;
                continue;
            }
            if let Some(distance) = comparator.distance_with_args(compared.chars(), &args) {
                matches.push((compared, distance, freq));
            }
        }
        for (compared, freq) in prefixes {
            if let Some(distance) = comparator.distance_with_args(compared.chars(), &args) {
                matches.push((compared, distance, freq));
            }
        }
        matches.sort_unstable_by(|a, b| (a.1, b.2, a.0).cmp(&(b.1, a.2, b.0)));
        matches.truncate(max_results);
        let result: Corrections = matches
            .into_iter()
            .map(|(word, distance, freq)| (word.to_owned(), distance, freq))
            .collect();
        memo.fuzzy.borrow_mut().insert(key.clone(), result.clone());
        result
    }

    /// Phrase corrections: per-word fixes, and splits of a run-together word.
    fn lookup_compound(
        &self,
        memo: &Memo,
        phrase: &str,
        max_results: usize,
    ) -> Vec<(String, usize)> {
        if !self.has_words || phrase.is_empty() {
            return Vec::new();
        }
        let phrase = text::lower(phrase);
        let words: Vec<&str> = text::words(&phrase).collect();
        if words.is_empty() {
            return Vec::new();
        }

        let mut corrected = Vec::with_capacity(words.len());
        // Runner-up corrections per word position, tried one at a time.
        let mut alternatives: Vec<(usize, String, usize)> = Vec::new();
        let mut total_distance = 0;
        let mut any_correction = false;
        let min_chars = (self.segment_config.min_word_chars as usize).max(2);
        for (i, &word) in words.iter().enumerate() {
            let last = i + 1 == words.len();
            // Short words are not indexed, so a correction would replace a real word; the word
            // being typed stays while an indexed word starts with it.
            let typing = last && self.has_prefix(Field::Word, word);
            if text::char_len(word) < min_chars || typing {
                corrected.push(word.to_owned());
                continue;
            }
            let prefix = last && text::char_len(word) >= 3;
            let found = self.fuzzy_search(memo, word, 3, prefix);
            match found.first() {
                Some((best, distance, _)) if *distance > 0 => {
                    corrected.push(best.clone());
                    total_distance += distance;
                    any_correction = true;
                    alternatives.extend(
                        found[1..]
                            .iter()
                            .filter(|(_, d, _)| *d > 0)
                            .map(|(alt, d, _)| (i, alt.clone(), d.saturating_sub(*distance))),
                    );
                }
                Some((_, _, freq)) => {
                    // A real word can still be a typo of a much more common one.
                    let common = found[1..]
                        .iter()
                        .find(|(_, d, f)| *d == 1 && *f >= freq.saturating_mul(20).max(20));
                    if let Some((alt, _, _)) = common {
                        alternatives.push((i, alt.clone(), 1));
                    }
                    corrected.push(word.to_owned());
                }
                None => corrected.push(word.to_owned()),
            }
        }

        let mut results = Vec::new();
        if any_correction {
            let joined = corrected.join(" ");
            if joined != phrase {
                results.push((joined, total_distance.max(1)));
            }
        }
        for (i, alt, extra) in alternatives {
            let mut phrase_words = corrected.clone();
            phrase_words[i] = alt;
            results.push((phrase_words.join(" "), (total_distance + extra).max(1)));
        }

        if let [word] = words.as_slice() {
            let chars: Vec<char> = word.chars().collect();
            if chars.len() > 6 {
                for split in 3..chars.len() - 2 {
                    let left: String = chars[..split].iter().collect();
                    let right: String = chars[split..].iter().collect();
                    let left = self.fuzzy_search(memo, &left, 1, false).into_iter().next();
                    let right = self.fuzzy_search(memo, &right, 1, false).into_iter().next();
                    if let (Some((lw, ld, _)), Some((rw, rd, _))) = (left, right) {
                        if ld <= 1 && rd <= 1 {
                            let compound = format!("{lw} {rw}");
                            if compound != phrase {
                                results.push((compound, ld + rd + 1));
                            }
                        }
                    }
                }
            }
        }

        results.sort_by_key(|r| r.1);
        let mut seen = FxHashSet::default();
        results.retain(|r| seen.insert(r.0.clone()));
        results.truncate(max_results);
        results
    }

    fn fix_spell(&self, memo: &Memo, query: &str, max_results: usize) -> Vec<(String, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = text::lower(query);
        let mut phrases: Vec<(String, usize)> = self
            .lookup_compound(memo, &lower, 3)
            .into_iter()
            .filter(|(p, _)| *p != lower)
            .collect();
        let mut seen: FxHashSet<String> = phrases.iter().map(|p| p.0.clone()).collect();
        for (word, distance, _) in self.fuzzy_search(memo, &lower, 5, false) {
            if word != lower && distance > 0 && seen.insert(word.clone()) {
                phrases.push((word, distance));
            }
        }
        phrases.sort_by_key(|p| p.1);
        phrases.truncate(max_results);
        phrases
    }

    fn raw_aliases(&self, query: &str, max_results: usize) -> Vec<(u32, f64)> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = text::lower(query);
        let mut shortest: FxHashMap<u32, usize> = FxHashMap::default();
        self.scan(
            Field::Alias,
            &lower,
            (max_results * 100).min(1000),
            |key, doc, kind| {
                if kind == 0 {
                    let len = char_count(&key[..key.len() - 1]);
                    shortest
                        .entry(doc)
                        .and_modify(|l| *l = (*l).min(len))
                        .or_insert(len);
                }
            },
        );
        let weight = self.config.popularity_weight as f32;
        let mut hits: Vec<(u32, f64)> = shortest
            .into_iter()
            .map(|(doc, len)| {
                let boost = 1.0f32 + self.weights[doc as usize] * weight * 10.0f32;
                let score = (100.0 - len as f64 * 0.1) * f64::from(boost);
                (doc, (score / self.max_score).min(1.0))
            })
            .collect();
        hits.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then(self.ids[a.0 as usize].cmp(&self.ids[b.0 as usize]))
        });
        hits.truncate(max_results);
        hits
    }

    fn raw_autocomplete(
        &self,
        memo: &Memo,
        query: &str,
        max_results: usize,
        depth: u8,
    ) -> Vec<RawHit> {
        if query.is_empty() {
            return Vec::new();
        }
        // Recursive lookups of short corrections are expensive and depend only on the index.
        if depth == 2
            && current_filter().is_none()
            && text::char_len(text::strip(&text::lower(query))) <= self.config.short_query_chars
        {
            let key = (query.to_owned(), max_results);
            let cache = || {
                self.recursion_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
            };
            if let Some(hits) = cache().get(&key) {
                return hits.to_vec();
            }
            let hits = self.search_uncached(memo, query, max_results, depth);
            let mut cache = cache();
            if cache.len() < self.config.short_query_cache_entries {
                cache.insert(key, hits.clone().into());
            }
            return hits;
        }
        self.search_uncached(memo, query, max_results, depth)
    }

    fn search_uncached(
        &self,
        memo: &Memo,
        query: &str,
        max_results: usize,
        depth: u8,
    ) -> Vec<RawHit> {
        let lower = text::lower(query);
        let q = text::strip(&lower);
        let (fetch_prefix, fetch_infix) = match text::char_len(q) {
            0..=3 => (10_000, 10_000),
            4..=5 => (50.max(max_results * 3), 100.max(max_results * 5)),
            _ => (30.max(max_results * 2), 50.max(max_results * 3)),
        };

        let prefix_matches = self.prefix_search(query, fetch_prefix);
        let infix_matches = self.infix_search(query, fetch_infix);
        let abbreviation_matches: Vec<u32> = self
            .get(Field::Alias, q)
            .into_iter()
            .filter(|&(_, kind)| kind == 1)
            .map(|(doc, _)| doc)
            .collect();

        let mut fuzzy_matches: FxHashMap<u32, (usize, usize)> = FxHashMap::default();
        if prefix_matches.len() < max_results && depth < 2 {
            let mut last_distance = 0;
            for (rank, (phrase, distance)) in self
                .fix_spell(memo, query, fetch_prefix)
                .into_iter()
                .enumerate()
            {
                // Phrases come closest first; further ones only fill up a short list.
                if distance > last_distance && fuzzy_matches.len() >= max_results * 2 {
                    break;
                }
                last_distance = distance;
                for hit in self.raw_autocomplete(memo, &phrase, max_results * 2, 2) {
                    let better = fuzzy_matches
                        .get(&hit.doc)
                        .is_none_or(|&(d, r)| distance < d || (distance == d && rank < r));
                    if better {
                        fuzzy_matches.insert(hit.doc, (distance, rank));
                    }
                }
            }
        }

        let exact = self.get(Field::Title, q).first().map(|&(doc, _)| doc);
        let mut merged: FxHashMap<u32, Merged> = FxHashMap::default();
        for (&doc, &(distance, rank)) in &fuzzy_matches {
            merged.insert(
                doc,
                Merged {
                    fuzzy_distance: distance,
                    fuzzy_rank: Some(rank),
                    ..Merged::default()
                },
            );
        }
        // Direct matches override corrections that also reach the document.
        for doc in infix_matches {
            merged.insert(doc, Merged::default());
        }
        for doc in prefix_matches {
            merged.insert(
                doc,
                Merged {
                    from_prefix: true,
                    is_exact: Some(doc) == exact,
                    ..Merged::default()
                },
            );
        }
        for doc in abbreviation_matches {
            let prev = merged.get(&doc).copied().filter(|p| p.fuzzy_distance == 0);
            merged.insert(
                doc,
                Merged {
                    from_prefix: prev.is_none_or(|p| p.from_prefix),
                    is_abbreviation: true,
                    is_exact: prev.is_some_and(|p| p.is_exact),
                    ..Merged::default()
                },
            );
        }

        // The f32/f64 mix is part of the scoring contract; changing it changes scores.
        let factor = self.config.popularity_weight * 10.0;
        let default_multiplier = 1.0 + 0.33 * factor;
        let single_word_query = !q.contains(' ');
        let word_bonus: FxHashSet<u32> = if single_word_query {
            self.get(Field::Word, q)
                .into_iter()
                .map(|(doc, _)| doc)
                .collect()
        } else {
            FxHashSet::default()
        };
        let mut scored: Vec<(f64, u32, Merged)> = merged
            .into_iter()
            .map(|(doc, m)| {
                let d = doc as usize;
                let mut score = if m.fuzzy_distance > 0 {
                    let base = 20.0 * 0.15f64.powf((m.fuzzy_distance - 1) as f64);
                    base + m
                        .fuzzy_rank
                        .map_or(0.0, |rank| (5.0 - rank as f64).max(0.0))
                } else if m.is_exact {
                    150.0
                } else if m.from_prefix {
                    90.0
                } else {
                    30.0
                };
                score -= f64::from(self.text_lens[d]) * 0.1;
                if single_word_query && self.single_word[d] == 0 && word_bonus.contains(&doc) {
                    score += 25.0;
                }
                let weight = self.weights[d];
                score *= if weight > 0.0 {
                    f64::from(1.0f32 + weight * factor as f32)
                } else {
                    default_multiplier
                };
                (score, doc, m)
            })
            .collect();
        scored.sort_unstable_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(self.text_lens[a.1 as usize].cmp(&self.text_lens[b.1 as usize]))
                .then(self.ids[a.1 as usize].cmp(&self.ids[b.1 as usize]))
        });
        scored.truncate(max_results);

        let mut results = Vec::with_capacity(scored.len());
        let mut fuzzy_results = Vec::new();
        for (score, doc, m) in scored {
            let score = (score / self.max_score).min(1.0);
            if m.fuzzy_distance > 0 {
                fuzzy_results.push(RawHit {
                    doc,
                    score,
                    kind: MatchKind::Fuzzy,
                });
                continue;
            }
            let kind = if m.is_exact {
                MatchKind::Exact
            } else if m.is_abbreviation {
                MatchKind::Abbreviation
            } else if m.from_prefix {
                MatchKind::Prefix
            } else {
                MatchKind::Infix
            };
            results.push(RawHit { doc, score, kind });
        }
        // Fuzzy matches rank below the weakest direct match.
        if let (Some(top_fuzzy), Some(min_direct)) = (
            fuzzy_results.first().map(|h| h.score),
            results.iter().map(|h| h.score).reduce(f64::min),
        ) {
            if top_fuzzy > 0.0 {
                let scale = (min_direct * 0.95) / top_fuzzy;
                fuzzy_results.iter_mut().for_each(|h| h.score *= scale);
            }
        }
        results.extend(fuzzy_results);

        if depth == 0 && single_word_query && text::char_len(q) > 8 {
            let has_direct = results.iter().any(|h| {
                matches!(
                    h.kind,
                    MatchKind::Exact | MatchKind::Prefix | MatchKind::Abbreviation
                )
            });
            if !has_direct {
                if let Some((phrase, _)) = self.lookup_compound(memo, q, 3).into_iter().next() {
                    if phrase != q {
                        return self.raw_autocomplete(memo, &phrase, max_results, 1);
                    }
                }
            }
        }
        results
    }
}
