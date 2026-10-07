//! Ranking: exact, prefix, abbreviation, infix and fuzzy scores, with deterministic tie-breaks by id.

use std::cmp::Ordering;
use std::ops::Range;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::index::{char_count, current_filter, with_filter, Field};
use crate::{fuzzy, text, Index};

/// Shorter queries are not corrected: one edit away from two characters is almost anything.
const MIN_CORRECTED_CHARS: usize = 3;

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
    /// Found through one of the document's synonyms.
    Synonym,
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
            Self::Synonym => "synonym",
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
    /// In layered searches, search a layer whose index does not exist as empty instead of failing.
    pub ignore_missing_layers: bool,
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
            ignore_missing_layers: false,
        }
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn ignore_missing_layers(mut self, ignore: bool) -> Self {
        self.ignore_missing_layers = ignore;
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
    /// The corrected query's own score for the document.
    fuzzy_score: f64,
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
        if hits.is_empty() {
            // Words too short to index, such as "HR" in "HR budgeting", give way when nothing holds them.
            let min_chars = self.segment_config.min_word_chars as usize;
            let words: Vec<&str> = text::words(stripped).collect();
            let kept: Vec<&str> = words
                .iter()
                .copied()
                .filter(|w| text::char_len(w) >= min_chars)
                .collect();
            if !kept.is_empty() && kept.len() < words.len() {
                return self.ranked(&kept.join(" "), limit);
            }
        }
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
            id: self.id(doc),
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
                id: self.id(doc),
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

    /// The `k` heaviest of `docs`, heaviest first, ties by id; each document's weight and id are
    /// read once.
    fn top_by_weight(&self, docs: Vec<u32>, k: usize) -> Vec<u32> {
        if k == 0 {
            return Vec::new();
        }
        let mut keyed: Vec<(f32, u64, u32)> = docs
            .into_iter()
            .map(|d| (self.weight(d), self.id_key(d), d))
            .collect();
        let order = |a: &(f32, u64, u32), b: &(f32, u64, u32)| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(a.1.cmp(&b.1))
        };
        if keyed.len() > k {
            keyed.select_nth_unstable_by(k - 1, order);
            keyed.truncate(k);
        }
        keyed.sort_unstable_by(order);
        keyed.into_iter().map(|(_, _, d)| d).collect()
    }

    fn prefix_search(&self, query: &str, max_results: usize) -> Vec<u32> {
        if query.is_empty() {
            return Vec::new();
        }
        self.top_titles(&text::lower(query), max_results)
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
            // Without short words to filter by, only the heaviest matches are needed.
            _ => self.docs_with_words(&words, short.is_empty().then_some(max_results)),
        };
        if short.is_empty() {
            return self.top_by_weight(docs, max_results);
        }
        // Words too short to index must still start a word of the text.
        let n = docs.len();
        self.top_by_weight(docs, n)
            .into_iter()
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
        let mut prefixes: FxHashMap<&str, (usize, u32)> = FxHashMap::default();
        let mut matches: Vec<(&str, usize, u32)> = Vec::new();
        for (word, freq) in self.fuzzy_candidates(&variants) {
            let word_len = char_count(word.as_bytes());
            if freq == 0 {
                continue;
            }
            if prefix && word_len > query_len {
                // The closest prefix within the distance in length, so a dropped or added letter
                // does not shift the comparison; the query's own length is tried first.
                let closest = (0..=2 * max_distance)
                    .map(|i| {
                        if i % 2 == 0 {
                            query_len + i / 2
                        } else {
                            query_len.wrapping_sub(i / 2 + 1)
                        }
                    })
                    .filter(|&n| n >= 1 && n <= word_len)
                    .filter_map(|n| {
                        let end = word.char_indices().nth(n).map_or(word.len(), |(i, _)| i);
                        let compared = &word[..end];
                        Some((
                            comparator.distance_with_args(compared.chars(), &args)?,
                            compared,
                        ))
                    })
                    .min_by_key(|&(distance, _)| distance);
                if let Some((distance, compared)) = closest {
                    // Words sharing a prefix pool their frequency under it.
                    prefixes.entry(compared).or_insert((distance, 0)).1 += freq;
                }
                continue;
            }
            if word_len.abs_diff(query_len) > max_distance {
                continue;
            }
            let Some(distance) = comparator.distance_with_args(word.chars(), &args) else {
                continue;
            };
            if prefix {
                prefixes.entry(word).or_insert((distance, 0)).1 += freq;
            } else {
                matches.push((word, distance, freq));
            }
        }
        matches.extend(
            prefixes
                .into_iter()
                .map(|(compared, (distance, freq))| (compared, distance, freq)),
        );
        matches.sort_unstable_by(|a, b| (a.1, b.2, a.0).cmp(&(b.1, a.2, b.0)));
        matches.truncate(max_results);
        let result: Corrections = matches
            .into_iter()
            .map(|(word, distance, freq)| (word.to_owned(), distance, freq))
            .collect();
        memo.fuzzy.borrow_mut().insert(key.clone(), result.clone());
        result
    }

    /// Whether checking `docs` documents' texts costs less than gathering postings up to `bound`.
    fn cheaper_by_text(&self, docs: usize, bound: u64) -> bool {
        // Reading a document's words costs about this many postings; past this many documents,
        // gathering postings is the safer bound.
        const TEXT_COST: u64 = 32;
        const TEXT_DOCS: usize = 100_000;
        docs <= TEXT_DOCS && (docs as u64).saturating_mul(TEXT_COST) < bound
    }

    /// Whether `doc` has an indexed word starting with each of `words`.
    /// Each of `words` is long enough to be indexed, so any word starting with it is too.
    fn holds_words(&self, doc: u32, words: &[&str]) -> bool {
        let mut doc_text = self.doc_text(doc);
        let lower = if doc_text.is_ascii() {
            doc_text.make_ascii_lowercase();
            doc_text
        } else {
            text::lower(&doc_text)
        };
        words
            .iter()
            .all(|w| text::words(&lower).any(|h| h.starts_with(w)))
    }

    /// At most how many postings the words starting with `prefix` hold: their occurrences, or a
    /// large number when too many words start with it to add up.
    fn postings_bound(&self, prefix: &str) -> u64 {
        const MOST_WORDS: u32 = 1 << 20;
        let mut total = 0u64;
        for seg in self.segments() {
            let range = seg.words.range(prefix.as_bytes());
            if range.len() as u32 > MOST_WORDS {
                return u64::MAX / 4;
            }
            total += range.map(|o| u64::from(seg.words.freq(o))).sum::<u64>();
        }
        total
    }

    /// Visible documents, ascending, with an indexed word starting with each of `words`, all long
    /// enough to be indexed. The rarest word's documents are gathered; the other words are looked
    /// for in their texts when that costs less than gathering theirs.
    /// With `heaviest`, at least that many of the heaviest such documents, when there are that many.
    fn docs_with_words(&self, words: &[&str], heaviest: Option<usize>) -> Vec<u32> {
        let gather = |word: &str| {
            let mut docs = Vec::new();
            self.scan_docs(Field::Word, word, |doc| docs.push(doc));
            docs
        };
        let rarest = (0..words.len())
            .min_by_key(|&i| self.postings_bound(words[i]))
            .expect("a word");
        let mut first = gather(words[rarest]);
        first.sort_unstable();
        first.dedup();
        let others: u64 = (0..words.len())
            .filter(|&i| i != rarest)
            .map(|i| self.postings_bound(words[i]))
            .fold(0, u64::saturating_add);
        if self.cheaper_by_text(first.len(), others) {
            let rest: Vec<&str> = (0..words.len())
                .filter(|&i| i != rarest)
                .map(|i| words[i])
                .collect();
            let Some(k) = heaviest else {
                first.retain(|&doc| self.holds_words(doc, &rest));
                return first;
            };
            // Heaviest first, so the checks stop once enough documents hold every word.
            let n = first.len();
            let mut docs: Vec<u32> = self
                .top_by_weight(first, n)
                .into_iter()
                .filter(|&doc| self.holds_words(doc, &rest))
                .take(k)
                .collect();
            docs.sort_unstable();
            return docs;
        }
        // Otherwise every list is gathered, and the shortest is narrowed by the others.
        let mut lists: Vec<Vec<u32>> = (0..words.len())
            .map(|i| {
                if i == rarest {
                    std::mem::take(&mut first)
                } else {
                    gather(words[i])
                }
            })
            .collect();
        lists.sort_by_key(Vec::len);
        let mut docs = std::mem::take(&mut lists[0]);
        docs.sort_unstable();
        docs.dedup();
        let mut marks = vec![0u64; self.doc_count().div_ceil(64)];
        for list in &lists[1..] {
            if docs.is_empty() {
                break;
            }
            for &doc in list {
                marks[doc as usize / 64] |= 1 << (doc % 64);
            }
            docs.retain(|&doc| marks[doc as usize / 64] >> (doc % 64) & 1 == 1);
            for &doc in list {
                marks[doc as usize / 64] = 0;
            }
        }
        docs
    }

    /// `corrections` of the `i`th word that some document holds beside the other words, closest
    /// first, then by how many documents hold both.
    fn fitting(&self, words: &[&str], i: usize, corrections: &Corrections) -> Vec<(String, usize)> {
        let min_chars = self.segment_config.min_word_chars as usize;
        // A word in one document of twenty or more says little about which correction fits.
        let common = self.doc_count() as u64 / 20;
        let others: Vec<&str> = words
            .iter()
            .enumerate()
            .filter(|&(j, w)| j != i && text::char_len(w) >= min_chars)
            .map(|(_, &w)| w)
            .filter(|w| self.postings_bound(w) <= common)
            .collect();
        if others.is_empty() {
            return Vec::new();
        }
        let bound = others
            .iter()
            .map(|w| self.postings_bound(w))
            .fold(0, u64::saturating_add);
        // The candidates' texts are read when they hold fewer documents, all told, than gathering
        // the others' documents once would cost.
        let held: u64 = corrections
            .iter()
            .map(|(c, _, _)| {
                let key = crate::segment::term_key(c);
                self.segments()
                    .iter()
                    .filter_map(|seg| {
                        seg.words
                            .ordinal(&key)
                            .map(|o| u64::from(seg.words.freq(o)))
                    })
                    .sum::<u64>()
            })
            .fold(0, u64::saturating_add);
        let by_text = self.cheaper_by_text(usize::try_from(held).unwrap_or(usize::MAX), bound);
        let context: FxHashSet<u32> = if by_text {
            FxHashSet::default()
        } else {
            self.docs_with_words(&others, None).into_iter().collect()
        };
        if !by_text && context.is_empty() {
            return Vec::new();
        }
        let mut fitting: Vec<(&str, usize, usize)> = Vec::new();
        for (candidate, distance, _) in corrections {
            // Corrections come closest first; further ones only when no closer one fits.
            if fitting.last().is_some_and(|f| f.1 < *distance) {
                break;
            }
            let docs = self.get(Field::Word, candidate);
            let together = if by_text {
                docs.iter()
                    .filter(|&&(doc, _)| self.holds_words(doc, &others))
                    .count()
            } else {
                docs.iter().filter(|(doc, _)| context.contains(doc)).count()
            };
            if together > 0 {
                fitting.push((candidate, *distance, together));
            }
        }
        fitting.sort_by(|a, b| (a.1, b.2).cmp(&(b.1, a.2)));
        fitting
            .into_iter()
            .map(|(w, d, _)| (w.to_owned(), d))
            .collect()
    }

    /// Phrase corrections: per-word fixes, and splits of a run-together word.
    /// With `stuck`, the word being typed is corrected too: the query as typed matched nothing.
    fn lookup_compound(
        &self,
        memo: &Memo,
        phrase: &str,
        max_results: usize,
        stuck: bool,
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
            let prefix = last && text::char_len(word) >= 3;
            if text::char_len(word) < min_chars || typing && !stuck {
                corrected.push(word.to_owned());
                continue;
            }
            if typing {
                // Its corrections are tried beside it, as short prefixes find few candidates.
                corrected.push(word.to_owned());
                alternatives.extend(
                    self.fuzzy_search(memo, word, 3, prefix)
                        .into_iter()
                        .filter(|(_, d, _)| *d > 0)
                        .map(|(alt, d, _)| (i, alt, d)),
                );
                continue;
            }
            let found = self.fuzzy_search(memo, word, 64, prefix);
            // Beside other words, a correction that some document holds with them comes first,
            // when the word needs one or the query as typed matched nothing.
            let needed = stuck || found.first().is_some_and(|(_, d, _)| *d > 0);
            let fitting = if words.len() > 1 && !last && needed {
                self.fitting(&words, i, &found)
            } else {
                Vec::new()
            };
            match fitting.first() {
                Some((_, 0)) => {
                    corrected.push(word.to_owned());
                    continue;
                }
                Some((best, distance)) => {
                    corrected.push(best.clone());
                    total_distance += distance;
                    any_correction = true;
                    alternatives.extend(
                        fitting[1..]
                            .iter()
                            .take(2)
                            .filter(|(_, d)| *d > 0)
                            .map(|(alt, d)| (i, alt.clone(), d.saturating_sub(*distance))),
                    );
                    continue;
                }
                None => {}
            }
            let found = &found[..found.len().min(3)];
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

    fn fix_spell(
        &self,
        memo: &Memo,
        query: &str,
        max_results: usize,
        stuck: bool,
    ) -> Vec<(String, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = text::lower(query);
        let mut phrases: Vec<(String, usize)> = self
            .lookup_compound(memo, &lower, 3, stuck)
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
        let hits: Vec<(u32, f64)> = shortest
            .into_iter()
            .map(|(doc, len)| {
                let boost = 1.0f32 + self.weight(doc) * weight * 10.0f32;
                let score = (100.0 - len as f64 * 0.1) * f64::from(boost);
                (doc, (score / self.max_score).min(1.0))
            })
            .collect();
        let mut keyed: Vec<(u32, f64, u64)> = hits
            .into_iter()
            .map(|(doc, score)| (doc, score, self.id_key(doc)))
            .collect();
        keyed.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then(a.2.cmp(&b.2))
        });
        keyed.truncate(max_results);
        keyed
            .into_iter()
            .map(|(doc, score, _)| (doc, score))
            .collect()
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

        let mut prefix_matches = self.prefix_search(query, fetch_prefix);
        let infix_matches = self.infix_search(query, fetch_infix);
        let abbreviation_matches: Vec<u32> = self
            .get(Field::Alias, q)
            .into_iter()
            .filter(|&(_, kind)| kind == 1)
            .map(|(doc, _)| doc)
            .collect();

        let mut fuzzy_matches: FxHashMap<u32, (usize, usize, f64)> = FxHashMap::default();
        if prefix_matches.len() < max_results
            && depth < 2
            && text::char_len(q) >= MIN_CORRECTED_CHARS
        {
            let mut last_distance = 0;
            for (rank, (phrase, distance)) in self
                .fix_spell(
                    memo,
                    query,
                    fetch_prefix,
                    prefix_matches.is_empty() && infix_matches.is_empty(),
                )
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
                        .is_none_or(|&(d, r, _)| distance < d || (distance == d && rank < r));
                    if better {
                        fuzzy_matches.insert(hit.doc, (distance, rank, hit.score));
                    }
                }
            }
        }

        // The prefix candidates are the most popular titles only; an exact title always joins them.
        let exact: Vec<u32> = self
            .get(Field::Title, q)
            .into_iter()
            .map(|(doc, _)| doc)
            .collect();
        for &doc in &exact {
            if !prefix_matches.contains(&doc) {
                prefix_matches.push(doc);
            }
        }
        let mut merged: FxHashMap<u32, Merged> = FxHashMap::default();
        for (&doc, &(distance, rank, score)) in &fuzzy_matches {
            merged.insert(
                doc,
                Merged {
                    fuzzy_distance: distance,
                    fuzzy_rank: Some(rank),
                    fuzzy_score: score,
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
                    is_exact: exact.contains(&doc),
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
        let single_word_query = !q.contains(' ');
        let word_bonus: FxHashSet<u32> = if single_word_query {
            self.get(Field::Word, q)
                .into_iter()
                .map(|(doc, _)| doc)
                .collect()
        } else {
            FxHashSet::default()
        };
        // Score, text length and id are read once per document, for the sort below.
        let mut scored: Vec<(f64, u16, u64, u32, Merged)> = merged
            .into_iter()
            .map(|(doc, m)| {
                let weight = self.weight(doc);
                let multiplier = f64::from(1.0f32 + weight.max(0.0) * factor as f32);
                let mut score = if m.fuzzy_distance > 0 {
                    let base = 20.0 * 0.15f64.powf((m.fuzzy_distance - 1) as f64);
                    // The corrected query's own score, without the popularity applied below.
                    let own = m.fuzzy_score * self.max_score / multiplier;
                    base + m
                        .fuzzy_rank
                        .map_or(0.0, |rank| (5.0 - rank as f64).max(0.0))
                        + 0.25 * own
                } else if m.is_exact {
                    150.0
                } else if m.from_prefix {
                    90.0
                } else {
                    30.0
                };
                let len = self.text_len(doc);
                score -= f64::from(len) * 0.1;
                if single_word_query && !self.is_single_word(doc) && word_bonus.contains(&doc) {
                    score += 25.0;
                }
                score *= multiplier;
                (score, len, self.id_key(doc), doc, m)
            })
            .collect();
        // Corrections rank below every direct match before the cut, so a longer limit only appends.
        scored.sort_unstable_by(|a, b| {
            (a.4.fuzzy_distance > 0)
                .cmp(&(b.4.fuzzy_distance > 0))
                .then(b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal))
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        scored.truncate(max_results);

        let mut results = Vec::with_capacity(scored.len());
        let mut fuzzy_results = Vec::new();
        for (score, _, _, doc, m) in scored {
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
                if let Some((phrase, _)) =
                    self.lookup_compound(memo, q, 3, false).into_iter().next()
                {
                    if phrase != q {
                        return self.raw_autocomplete(memo, &phrase, max_results, 1);
                    }
                }
            }
        }
        results
    }
}
