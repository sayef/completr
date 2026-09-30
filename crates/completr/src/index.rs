use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::search::{CachedHit, RawHit};
use crate::segment::{term_key, Keyed};
use crate::{BuildOptions, Document, Error, Segment};

/// Options for searching segments as one index. Set them with the chainable methods of the same
/// names, e.g. `IndexOptions::default().max_score(742.0)`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct IndexOptions {
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

impl Default for IndexOptions {
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

crate::setters!(IndexOptions {
    popularity_weight: f64,
    short_query_chars: usize,
    short_query_limit: usize,
    short_query_cache_entries: usize,
    carry_short_queries: usize,
    warm_on_load: bool,
    vector_threads: usize,
});

impl IndexOptions {
    /// Pins the raw score that normalises to 1.0, so scores stay comparable across rebuilds.
    pub fn max_score(mut self, max_score: f64) -> Self {
        self.max_score = Some(max_score);
        self
    }
}

/// Documents a request may return, as a bitset over an index's document numbers.
pub(crate) struct Allowed(Vec<u64>);

impl Allowed {
    pub(crate) fn contains(&self, doc: u32) -> bool {
        self.0[doc as usize / 64] >> (doc % 64) & 1 == 1
    }
}

thread_local! {
    static ALLOWED: RefCell<Option<Rc<Allowed>>> = const { RefCell::new(None) };
}

/// The filter of the request running on this thread, if any.
pub(crate) fn current_filter() -> Option<Rc<Allowed>> {
    ALLOWED.with(|a| a.borrow().clone())
}

/// Runs `f` with `allowed` as the filter of this thread's request, restoring the previous one.
pub(crate) fn with_filter<R>(allowed: Option<Allowed>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<Allowed>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ALLOWED.with(|a| *a.borrow_mut() = self.0.take());
        }
    }
    let previous = ALLOWED.with(|a| a.replace(allowed.map(Rc::new)));
    let _restore = Restore(previous);
    f()
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
    pub(crate) config: IndexOptions,
    pub(crate) max_score: f64,
    pub(crate) segment_config: BuildOptions,
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
    /// Per segment with vectors, which of its slots are live.
    vector_masks: Vec<Option<Vec<bool>>>,
    vector_dim: Option<usize>,
}

impl Index {
    pub fn new(segments: Vec<Arc<Segment>>, config: IndexOptions) -> Result<Self, Error> {
        let segment_config = segments
            .first()
            .map_or_else(BuildOptions::default, |s| s.config());
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
        // Superseding ids with their key; deletes carry none.
        let mut superseded: FxHashMap<u64, Option<String>> = FxHashMap::default();
        for (i, seg) in segments.iter().enumerate().rev() {
            let base = offsets[i] as usize;
            if !superseded.is_empty() {
                for (local, id) in seg.ids().iter().enumerate() {
                    if let Some(newer) = superseded.get(id) {
                        live[base + local] = false;
                        if let (Some(newer), Some(older)) = (newer, seg.key(local)) {
                            if *newer != older {
                                return Err(Error::input(format!(
                                    "keys {older:?} and {newer:?} map to the same id {id}"
                                )));
                            }
                        }
                    }
                }
            }
            if i > 0 {
                for local in 0..seg.len() {
                    superseded.insert(seg.ids()[local], seg.key(local));
                }
                for &id in seg.deletes() {
                    superseded.entry(id).or_insert(None);
                }
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

        let mut hidden_word_freqs: FxHashMap<String, u32> = FxHashMap::default();
        for (i, seg) in segments.iter().enumerate() {
            let base = offsets[i] as usize;
            for local in (0..seg.len()).filter(|&l| !live[base + l]) {
                for word in seg.doc_words(local) {
                    *hidden_word_freqs.entry(word).or_default() += 1;
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

    pub fn empty(config: IndexOptions) -> Result<Self, Error> {
        Self::new(Vec::new(), config)
    }

    /// Builds one segment from `documents` with default options and searches it.
    pub fn from_documents(documents: impl IntoIterator<Item = Document>) -> Result<Self, Error> {
        Self::from_documents_with(documents, BuildOptions::default(), IndexOptions::default())
    }

    pub fn from_documents_with(
        documents: impl IntoIterator<Item = Document>,
        build: BuildOptions,
        options: IndexOptions,
    ) -> Result<Self, Error> {
        let segment = Segment::build_with(build, documents, [])?;
        Self::new(vec![Arc::new(segment)], options)
    }

    pub fn config(&self) -> &IndexOptions {
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

    /// Whether this index holds or deletes `id`, so that it overrides `id` in earlier layers.
    pub(crate) fn covers(&self, id: u64) -> bool {
        self.segments
            .iter()
            .any(|s| s.ids().binary_search(&id).is_ok() || s.deletes().binary_search(&id).is_ok())
    }

    /// The live document with string key `key`.
    pub fn document_by_key(&self, key: &str) -> Option<Document> {
        self.document(crate::key_id(key))
            .filter(|d| d.key.as_deref() == Some(key))
    }

    /// Segment and local position of document number `doc`.
    fn locate(&self, doc: u32) -> (usize, usize) {
        let i = self.offsets.partition_point(|&o| o <= doc) - 1;
        (i, (doc - self.offsets[i]) as usize)
    }

    pub(crate) fn doc_text(&self, doc: u32) -> String {
        let (i, local) = self.locate(doc);
        self.segments[i].text(local)
    }

    pub(crate) fn doc_key(&self, doc: u32) -> Option<String> {
        let (i, local) = self.locate(doc);
        self.segments[i].key(local)
    }

    /// The live documents tagged with any of `contexts`, or `None` for no filter.
    pub(crate) fn allowed(&self, contexts: &[String]) -> Option<Allowed> {
        if contexts.is_empty() {
            return None;
        }
        let mut bits = vec![0u64; self.ids.len().div_ceil(64)];
        for (seg, &base) in self.segments.iter().zip(&self.offsets) {
            for context in contexts {
                if let Some(postings) = seg.contexts.get(&term_key(context)) {
                    for local in postings {
                        let doc = (base + local) as usize;
                        bits[doc / 64] |= 1 << (doc % 64);
                    }
                }
            }
        }
        Some(Allowed(bits))
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
        let config = BuildOptions {
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
        let allowed = current_filter();
        let mut hits: Vec<(u32, f64)> = Vec::new();
        for ((seg, mask), &base) in self
            .segments
            .iter()
            .zip(&self.vector_masks)
            .zip(&self.offsets)
        {
            let (Some(vectors), Some(mask)) = (&seg.vectors, mask) else {
                continue;
            };
            let filtered: Vec<bool>;
            let mask = match &allowed {
                Some(allowed) => {
                    filtered = mask
                        .iter()
                        .zip(vectors.locals())
                        .map(|(&m, &local)| m && allowed.contains(base + local))
                        .collect();
                    &filtered
                }
                None => mask,
            };
            if !mask.iter().any(|&m| m) {
                continue;
            }
            for (local, score) in vectors.search(query, k, mask, self.config.vector_threads)? {
                hits.push((base + local, f64::from(score)));
            }
        }
        hits.sort_unstable_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then(self.ids[a.0 as usize].cmp(&self.ids[b.0 as usize]))
        });
        hits.truncate(k);
        Ok(hits
            .into_iter()
            .map(|(doc, score)| self.suggestion(doc, score, crate::MatchKind::Semantic, None))
            .collect())
    }

    /// The 99th percentile of exact-match scores over live documents.
    pub fn estimate_max_score(&self) -> f64 {
        let factor = self.config.popularity_weight * 10.0;
        let mut scores: Vec<f64> = (0..self.ids.len())
            .filter(|&d| self.live[d])
            .map(|d| {
                let mut score = 150.0 - f64::from(self.text_lens[d]) * 0.1;
                if self.single_word[d] == 1 {
                    score += 25.0;
                }
                let weight = f64::from(self.weights[d]);
                score
                    * if weight > 0.0 {
                        1.0 + weight * factor
                    } else {
                        1.0 + 0.33 * factor
                    }
            })
            .collect();
        if scores.is_empty() {
            return 750.0;
        }
        let at = scores.len() * 99 / 100;
        *scores.select_nth_unstable_by(at, f64::total_cmp).1
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

    /// Calls `f(doc)` for every visible document under a key starting with `prefix`, in no order.
    pub(crate) fn scan_docs(&self, field: Field, prefix: &str, mut f: impl FnMut(u32)) {
        let allowed = current_filter();
        for (seg, &base) in self.segments.iter().zip(&self.offsets) {
            let keyed = Self::keyed(seg, field);
            let mut cursor = keyed.map.cursor(prefix.as_bytes());
            while cursor.advance() {
                for posting in keyed.postings(cursor.value()) {
                    let doc = base + Self::decode(field, posting).0;
                    if self.live[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc)) {
                        f(doc);
                    }
                }
            }
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
        let allowed = current_filter();
        let visible =
            |doc: u32| self.live[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc));
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
                for posting in Self::keyed(&self.segments[*i], field).postings(cursor.value()) {
                    let (local, kind) = Self::decode(field, posting);
                    if visible(base + local) {
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
        let allowed = current_filter();
        let mut entries = Vec::new();
        let mut sources = 0;
        for (i, seg) in self.segments.iter().enumerate() {
            let Some(postings) = Self::keyed(seg, field).get(&key) else {
                continue;
            };
            sources += 1;
            for posting in postings {
                let (local, kind) = Self::decode(field, posting);
                let doc = self.offsets[i] + local;
                if self.live[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc)) {
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
    pub(crate) fn fuzzy_candidates(&self, variants: &FxHashSet<String>) -> Vec<(&str, u32)> {
        let config = self.segment_config;
        let (d, pc) = (config.max_edit_distance, config.fuzzy_prefix_chars as usize);
        let mut found: Vec<(&str, u32)> = Vec::new();
        let mut hits: Vec<u32> = Vec::new();
        for seg in &self.segments {
            hits.clear();
            let word_count = seg.word_count();
            for variant in variants {
                // A bucket also holds the words of other variants.
                hits.extend(seg.variants.get(variant.as_bytes()).filter(|&ordinal| {
                    ordinal < word_count
                        && crate::fuzzy::is_variant(variant, seg.word_text(ordinal), d, pc)
                }));
            }
            hits.sort_unstable();
            hits.dedup();
            found.extend(
                hits.iter()
                    .map(|&o| (seg.word_text(o), seg.word_freq_at(o))),
            );
        }
        if self.segments.len() > 1 {
            let mut merged: FxHashMap<&str, u32> = FxHashMap::default();
            for (word, freq) in found.drain(..) {
                *merged.entry(word).or_default() += freq;
            }
            found.extend(merged);
        }
        if !self.hidden_word_freqs.is_empty() {
            for (word, freq) in found.iter_mut() {
                *freq =
                    freq.saturating_sub(self.hidden_word_freqs.get(*word).copied().unwrap_or(0));
            }
        }
        found
    }
}

/// Chars in UTF-8 bytes.
pub(crate) fn char_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b & 0xc0 != 0x80).count()
}
