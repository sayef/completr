use std::sync::{Arc, Mutex};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::search::{CachedHit, RawHit};
use crate::segment::{term_key, Keyed};
use crate::{Document, Error, Segment, SegmentConfig};

#[derive(Clone, Debug)]
pub struct IndexConfig {
    /// Raw score that normalises to 1.0. `None` estimates it from the documents; pin it when
    /// scores must stay comparable across rebuilds.
    pub max_score: Option<f64>,
    pub popularity_weight: f64,
    /// Queries up to this many chars are served from a per-index cache.
    pub short_query_chars: usize,
    /// Results computed per cached short query; requests are served by truncating them.
    pub short_query_limit: usize,
    pub short_query_cache_entries: usize,
    /// On replacing an index, the replica recomputes this many of the old index's most-served
    /// short queries on the new one, so the cache stays warm across updates.
    pub carry_short_queries: usize,
    /// Map every page of new segments before the replica publishes them.
    pub warm_on_load: bool,
    /// Threads per vector query: 1 runs inline on the caller's thread (best under concurrent
    /// load), 0 uses rayon's global pool, more uses a shared pool of that size.
    pub vector_threads: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            max_score: None,
            popularity_weight: 0.4,
            short_query_chars: 3,
            short_query_limit: 100,
            short_query_cache_entries: 10_000,
            vector_threads: 1,
            carry_short_queries: 1000,
            warm_on_load: true,
        }
    }
}

/// Cached results of one short query, and how often they were served.
pub(crate) type ShortEntry = (Arc<[CachedHit]>, u32);

/// A recursive lookup: its query and result limit.
pub(crate) type RecursionKey = (String, usize);

#[derive(Clone, Copy)]
pub(crate) enum Field {
    Title,
    Word,
    Alias,
}

/// An immutable, searchable view over segments ordered oldest to newest.
///
/// A newer segment supersedes older documents with the same id, and its deletes hide them.
pub struct Index {
    pub(crate) config: IndexConfig,
    pub(crate) max_score: f64,
    pub(crate) segment_config: SegmentConfig,
    segments: Vec<Arc<Segment>>,
    offsets: Vec<u32>,
    pub(crate) ids: Vec<u64>,
    pub(crate) weights: Vec<f32>,
    pub(crate) text_lens: Vec<u16>,
    pub(crate) single_word: Vec<u8>,
    live: Vec<bool>,
    live_count: usize,
    hidden_word_freqs: FxHashMap<String, u32>,
    pub(crate) has_words: bool,
    /// Cached short-query results with how often each was served.
    pub(crate) short_cache: Mutex<FxHashMap<String, ShortEntry>>,
    pub(crate) recursion_cache: Mutex<FxHashMap<RecursionKey, Arc<[RawHit]>>>,
    /// Bloom filters over the fuzzy variants of all but the largest segment, whose lookups
    /// mostly miss.
    variant_filters: Vec<Option<VariantFilter>>,
    /// Per segment with vectors, which of its slots are live.
    vector_masks: Vec<Option<Vec<bool>>>,
    vector_dim: Option<usize>,
}

impl Index {
    pub fn new(segments: Vec<Arc<Segment>>, config: IndexConfig) -> Result<Self, Error> {
        let segment_config = segments
            .first()
            .map_or_else(SegmentConfig::default, |s| s.config());
        if segments
            .iter()
            .any(|s| !s.config().compatible(&segment_config))
        {
            return Err(Error::input("segments were built with different configs"));
        }
        let total: usize = segments.iter().map(|s| s.len()).sum();
        if total >= u32::MAX as usize {
            return Err(Error::input("too many documents in one index"));
        }

        let mut offsets = Vec::with_capacity(segments.len());
        let (mut ids, mut weights, mut text_lens, mut single_word) = (
            Vec::with_capacity(total),
            Vec::with_capacity(total),
            Vec::with_capacity(total),
            Vec::with_capacity(total),
        );
        for seg in &segments {
            offsets.push(ids.len() as u32);
            ids.extend_from_slice(seg.ids());
            weights.extend_from_slice(seg.weights());
            text_lens.extend_from_slice(seg.text_lens());
            single_word.extend_from_slice(seg.single_word());
        }

        let mut live = vec![true; total];
        let mut superseded: FxHashSet<u64> = FxHashSet::default();
        for (i, seg) in segments.iter().enumerate().rev() {
            let base = offsets[i] as usize;
            if !superseded.is_empty() {
                for (local, id) in seg.ids().iter().enumerate() {
                    live[base + local] = !superseded.contains(id);
                }
            }
            if i > 0 {
                superseded.extend(seg.ids());
                superseded.extend(seg.deletes());
            }
        }

        let mut vector_shape: Option<(usize, u8)> = None;
        for vectors in segments.iter().filter_map(|s| s.vectors.as_ref()) {
            let shape = (vectors.dim(), vectors.bits());
            match vector_shape {
                Some(existing) if existing != shape => {
                    return Err(Error::input(format!(
                        "segments have vectors of different shapes: {existing:?} and {shape:?} (dimension, bits)"
                    )));
                }
                _ => vector_shape = Some(shape),
            }
        }
        let vector_masks = segments
            .iter()
            .zip(&offsets)
            .map(|(seg, &base)| {
                seg.vectors.as_ref().map(|v| {
                    v.locals()
                        .iter()
                        .map(|&l| live[base as usize + l as usize])
                        .collect()
                })
            })
            .collect();

        let largest = (0..segments.len()).max_by_key(|&i| segments[i].variants.map.len());
        let variant_filters = segments
            .iter()
            .enumerate()
            .map(|(i, seg)| {
                (segments.len() > 1 && Some(i) != largest).then(|| VariantFilter::build(seg))
            })
            .collect();
        let mut hidden_word_freqs: FxHashMap<String, u32> = FxHashMap::default();
        for (i, seg) in segments.iter().enumerate() {
            let base = offsets[i] as usize;
            for local in (0..seg.len()).filter(|&l| !live[base + l]) {
                for &ordinal in seg.doc_words(local) {
                    let word = seg.word_text(ordinal);
                    match hidden_word_freqs.get_mut(word) {
                        Some(count) => *count += 1,
                        None => {
                            hidden_word_freqs.insert(word.to_owned(), 1);
                        }
                    }
                }
            }
        }

        let mut index = Self {
            max_score: 0.0,
            segment_config,
            live_count: live.iter().filter(|&&l| l).count(),
            has_words: segments.iter().any(|s| !s.words.is_empty()),
            segments,
            offsets,
            ids,
            weights,
            text_lens,
            single_word,
            live,
            hidden_word_freqs,
            short_cache: Mutex::default(),
            recursion_cache: Mutex::default(),
            variant_filters,
            vector_masks,
            vector_dim: vector_shape.map(|(dim, _)| dim),
            config,
        };
        index.max_score = match index.config.max_score {
            Some(score) => score,
            None => index.estimate_max_score(),
        };
        if !(index.max_score.is_finite() && index.max_score > 0.0) {
            return Err(Error::input("max_score must be positive and finite"));
        }
        Ok(index)
    }

    pub fn empty(config: IndexConfig) -> Result<Self, Error> {
        Self::new(Vec::new(), config)
    }

    pub fn config(&self) -> &IndexConfig {
        &self.config
    }

    pub fn max_score(&self) -> f64 {
        self.max_score
    }

    pub fn segments(&self) -> &[Arc<Segment>] {
        &self.segments
    }

    /// Number of live documents.
    pub fn len(&self) -> usize {
        self.live_count
    }

    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    pub fn document(&self, id: u64) -> Option<Document> {
        self.segments.iter().enumerate().rev().find_map(|(i, seg)| {
            let local = seg.ids().binary_search(&id).ok()?;
            self.live[self.offsets[i] as usize + local].then(|| seg.document(local))
        })
    }

    pub fn documents(&self) -> impl Iterator<Item = Document> + '_ {
        self.segments.iter().enumerate().flat_map(move |(i, seg)| {
            let base = self.offsets[i] as usize;
            seg.documents()
                .enumerate()
                .filter(move |(l, _)| self.live[base + l])
                .map(|(_, doc)| doc)
        })
    }

    /// Merges the live documents into one segment without deletes.
    pub fn compact(&self) -> Result<Segment, Error> {
        self.merged_segment(Vec::new())
    }

    /// The live documents, with their vector codes carried over, as one segment with `deletes`.
    pub(crate) fn merged_segment(&self, deletes: Vec<u64>) -> Result<Segment, Error> {
        let mut codes = crate::vectors::CarriedCodes::default();
        for (i, seg) in self.segments.iter().enumerate() {
            let Some(vectors) = &seg.vectors else {
                continue;
            };
            let base = self.offsets[i] as usize;
            for &local in vectors.locals() {
                if self.live[base + local as usize] {
                    let (code, scale) = vectors.row(local).expect("slot of a listed local");
                    codes.insert(seg.ids()[local as usize], (code.to_vec(), scale));
                }
            }
        }
        let carried = self.vector_dim.map(|dim| (dim, &codes));
        let bits = self
            .segments
            .iter()
            .find_map(|s| s.vectors.as_ref().map(|v| v.bits()));
        let config = SegmentConfig {
            vector_bits: bits.unwrap_or(self.segment_config.vector_bits),
            ..self.segment_config
        };
        Segment::build_inner(config, self.documents(), deletes, carried)
    }

    /// Maps all pages of all segments in advance.
    pub fn warm(&self) {
        self.segments.iter().for_each(|s| s.warm());
    }

    /// The `n` cached short queries served most often, most often first.
    pub fn hot_short_queries(&self, n: usize) -> Vec<String> {
        let cache = self.short_cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries: Vec<(&String, u32)> =
            cache.iter().map(|(q, (_, hits))| (q, *hits)).collect();
        entries.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        entries
            .into_iter()
            .take(n)
            .map(|(q, _)| q.clone())
            .collect()
    }

    /// Computes and caches results for `queries`, e.g. those of the index this one replaces.
    pub fn prefill_short_queries(&self, queries: &[String]) {
        for query in queries {
            self.complete(query, 0);
        }
    }

    /// Dimension of the indexed embeddings, if any segment has them.
    pub fn vector_dim(&self) -> Option<usize> {
        self.vector_dim
    }

    /// The `k` live documents whose embeddings score highest against `query` (approximate inner
    /// product, cosine for normalised vectors), best first.
    pub fn vector_search(&self, query: &[f32], k: usize) -> Result<Vec<crate::Suggestion>, Error> {
        let Some(dim) = self.vector_dim else {
            return Ok(Vec::new());
        };
        if query.len() != dim {
            return Err(Error::input(format!(
                "query has {} dimensions, the index {dim}",
                query.len()
            )));
        }
        if k == 0 || query.iter().any(|x| !x.is_finite()) {
            return if k == 0 {
                Ok(Vec::new())
            } else {
                Err(Error::input("query vector is not finite"))
            };
        }
        let mut hits: Vec<crate::Suggestion> = Vec::new();
        for ((seg, mask), &base) in self
            .segments
            .iter()
            .zip(&self.vector_masks)
            .zip(&self.offsets)
        {
            let (Some(vectors), Some(mask)) = (&seg.vectors, mask) else {
                continue;
            };
            if !mask.iter().any(|&m| m) {
                continue;
            }
            for (local, score) in vectors.search(query, k, mask, self.config.vector_threads)? {
                let id = self.ids[base as usize + local as usize];
                hits.push(crate::Suggestion {
                    id,
                    score: f64::from(score),
                    kind: crate::MatchKind::Semantic,
                });
            }
        }
        hits.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
        hits.truncate(k);
        Ok(hits)
    }

    /// The 99th percentile of exact-match scores over distinct texts, the lowest id per text.
    pub fn estimate_max_score(&self) -> f64 {
        let factor = self.config.popularity_weight * 10.0;
        let mut scores = Vec::new();
        let mut last_key: Vec<u8> = Vec::new();
        self.scan(Field::Title, "", usize::MAX, |key, doc, _| {
            if key == last_key.as_slice() {
                return;
            }
            last_key = key.to_vec();
            let text = &key[..key.len() - 1];
            let mut score = 150.0 - char_count(text) as f64 * 0.1;
            if !text.contains(&b' ') {
                score += 25.0;
            }
            let weight = self.weights[doc as usize] as f64;
            score *= if weight > 0.0 {
                1.0 + weight * factor
            } else {
                1.0 + 0.33 * factor
            };
            scores.push(score);
        });
        if scores.is_empty() {
            return 750.0;
        }
        scores.sort_by(f64::total_cmp);
        scores[scores.len() * 99 / 100]
    }

    fn keyed(seg: &Segment, field: Field) -> &Keyed {
        match field {
            Field::Title => &seg.titles,
            Field::Word => &seg.words,
            Field::Alias => &seg.aliases,
        }
    }

    fn decode(field: Field, posting: u32) -> (u32, u8) {
        match field {
            Field::Alias => (posting >> 1, (posting & 1) as u8),
            _ => (posting, 0),
        }
    }

    /// Calls `f(key, doc, kind)` for up to `limit` live entries whose key starts with `prefix`,
    /// in key byte order and by id within a key.
    pub(crate) fn scan(
        &self,
        field: Field,
        prefix: &str,
        limit: usize,
        mut f: impl FnMut(&[u8], u32, u8),
    ) {
        if limit == 0 {
            return;
        }
        let prefix = prefix.as_bytes();
        let mut cursors: Vec<(usize, crate::dict::Cursor)> = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, seg)| (i, Self::keyed(seg, field).map.cursor(prefix)))
            .filter_map(|(i, mut cursor)| cursor.advance().then_some((i, cursor)))
            .collect();
        let mut key: Vec<u8> = Vec::new();
        let mut entries: Vec<(u32, u8)> = Vec::new();
        let mut emitted = 0;
        while !cursors.is_empty() {
            key.clear();
            key.extend_from_slice(cursors.iter().map(|(_, c)| c.key()).min().unwrap());
            entries.clear();
            let mut sources = 0;
            for (i, cursor) in cursors.iter_mut() {
                if cursor.key() != key.as_slice() {
                    continue;
                }
                sources += 1;
                let base = self.offsets[*i];
                for &posting in Self::keyed(&self.segments[*i], field)
                    .postings(cursor.value())
                    .as_slice()
                {
                    let (local, kind) = Self::decode(field, posting);
                    if self.live[(base + local) as usize] {
                        entries.push((base + local, kind));
                    }
                }
            }
            cursors.retain_mut(|(_, cursor)| cursor.key() != key.as_slice() || cursor.advance());
            if sources > 1 {
                entries.sort_by_key(|&(doc, kind)| (self.ids[doc as usize], kind));
            }
            for &(doc, kind) in &entries {
                f(&key, doc, kind);
                emitted += 1;
                if emitted == limit {
                    return;
                }
            }
        }
    }

    /// Live entries whose key is exactly `key`, by id.
    pub(crate) fn get(&self, field: Field, key: &str) -> Vec<(u32, u8)> {
        let key = term_key(key);
        let mut entries = Vec::new();
        let mut sources = 0;
        for (i, seg) in self.segments.iter().enumerate() {
            let Some(postings) = Self::keyed(seg, field).get(&key) else {
                continue;
            };
            sources += 1;
            for &posting in postings.as_slice() {
                let (local, kind) = Self::decode(field, posting);
                let doc = self.offsets[i] + local;
                if self.live[doc as usize] {
                    entries.push((doc, kind));
                }
            }
        }
        if sources > 1 {
            entries.sort_by_key(|&(doc, kind)| (self.ids[doc as usize], kind));
        }
        entries
    }

    pub(crate) fn has_prefix(&self, field: Field, prefix: &str) -> bool {
        let mut found = false;
        self.scan(field, prefix, 1, |_, _, _| found = true);
        found
    }

    /// Dictionary words sharing a delete variant with the query, with their live frequency.
    ///
    /// A word is found in every segment that holds it, so its per-segment frequencies come with it.
    pub(crate) fn fuzzy_candidates(&self, variants: &FxHashSet<String>) -> FxHashMap<&str, u32> {
        let mut words: FxHashMap<&str, u32> = FxHashMap::default();
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        for (seg, filter) in self.segments.iter().zip(&self.variant_filters) {
            seen.clear();
            for variant in variants {
                if filter
                    .as_ref()
                    .is_some_and(|f| !f.may_contain(variant.as_bytes()))
                {
                    continue;
                }
                let Some(ordinals) = seg.variants.get(variant.as_bytes()) else {
                    continue;
                };
                for &ordinal in ordinals.as_slice() {
                    if seen.insert(ordinal) {
                        *words.entry(seg.word_text(ordinal)).or_default() +=
                            seg.word_freq_at(ordinal);
                    }
                }
            }
        }
        if !self.hidden_word_freqs.is_empty() {
            for (word, freq) in words.iter_mut() {
                *freq =
                    freq.saturating_sub(self.hidden_word_freqs.get(*word).copied().unwrap_or(0));
            }
        }
        words
    }
}

/// Chars in UTF-8 bytes.
pub(crate) fn char_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b & 0xc0 != 0x80).count()
}

/// A Bloom filter over a segment's fuzzy variant keys, about 1 % false positives.
struct VariantFilter {
    bits: Vec<u64>,
    mask: u64,
}

impl VariantFilter {
    fn hashes(key: &[u8]) -> (u64, u64) {
        use std::hash::Hasher;
        let mut hasher = rustc_hash::FxHasher::default();
        hasher.write(key);
        let h = hasher.finish().wrapping_mul(0x9e37_79b9_7f4a_7c15);
        (h, h.rotate_left(32) | 1)
    }

    fn build(segment: &Segment) -> Self {
        let slots = (segment.variants.map.len() * 10)
            .next_power_of_two()
            .max(64);
        let mut filter = Self {
            bits: vec![0; slots / 64],
            mask: slots as u64 - 1,
        };
        let mut cursor = segment.variants.map.cursor(b"");
        while cursor.advance() {
            let (h1, h2) = Self::hashes(cursor.key());
            for i in 0..3u64 {
                let bit = h1.wrapping_add(i.wrapping_mul(h2)) & filter.mask;
                filter.bits[(bit / 64) as usize] |= 1 << (bit % 64);
            }
        }
        filter
    }

    fn may_contain(&self, key: &[u8]) -> bool {
        let (h1, h2) = Self::hashes(key);
        (0..3u64).all(|i| {
            let bit = h1.wrapping_add(i.wrapping_mul(h2)) & self.mask;
            self.bits[(bit / 64) as usize] >> (bit % 64) & 1 == 1
        })
    }
}
