use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::search::{CachedHit, RawHit};
use crate::segment::{term_key, weight_order, Keyed, TitleCursor, WordCursor, Words};
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

/// The default popularity weight, for which each segment stores its own `max_score`.
pub(crate) const POPULARITY_WEIGHT: f64 = 0.4;

/// A document's score for an exact match, before normalising.
pub(crate) fn exact_score(len: u16, single: bool, weight: f32, factor: f64) -> f64 {
    let base = 150.0 - f64::from(len) * 0.1 + if single { 25.0 } else { 0.0 };
    base * (1.0 + f64::from(weight.max(0.0)) * factor)
}

/// The 99th percentile of `scores()`, exactly; `scores` is called three times.
pub(crate) fn max_score_of<I: ParallelIterator<Item = f64>>(scores: impl Fn() -> I) -> f64 {
    let scores = || scores().filter(|s| s.is_finite());
    let (count, lo, hi) = scores()
        .fold(
            || (0, f64::MAX, f64::MIN),
            |(n, lo, hi), s| (n + 1, lo.min(s), hi.max(s)),
        )
        .reduce(
            || (0, f64::MAX, f64::MIN),
            |a, b| (a.0 + b.0, a.1.min(b.1), a.2.max(b.2)),
        );
    if count == 0 {
        return 750.0;
    }
    // Exact, without sorting: count scores into bins, then select within the one holding it.
    const BINS: usize = 4096;
    let bin = |s: f64| ((s - lo) / (hi - lo).max(f64::MIN_POSITIVE) * (BINS - 1) as f64) as usize;
    let counts = scores()
        .fold(
            || vec![0usize; BINS],
            |mut counts, s| {
                counts[bin(s)] += 1;
                counts
            },
        )
        .reduce(
            || vec![0usize; BINS],
            |mut a, b| {
                a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                a
            },
        );
    let mut rank = count * 99 / 100;
    let target = counts
        .iter()
        .position(|&c| {
            let inside = rank < c;
            if !inside {
                rank -= c;
            }
            inside
        })
        .unwrap_or(BINS - 1);
    let mut within: Vec<f64> = scores().filter(|&s| bin(s) == target).collect();
    *within.select_nth_unstable_by(rank, f64::total_cmp).1
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_score: None,
            popularity_weight: POPULARITY_WEIGHT,
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
    /// Per document, whether a newer segment supersedes or deletes it; zeroed, so it costs nothing
    /// until a document is hidden.
    hidden: Vec<bool>,
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
        let mut base = 0;
        for seg in &segments {
            offsets.push(base);
            base += seg.len() as u32;
        }

        let mut hidden = vec![false; total];
        let hidden_docs = hide_superseded(&segments, &offsets, &mut hidden)?;

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
                        .map(|&l| !hidden[base as usize + l as usize])
                        .collect()
                })
            })
            .collect();

        let mut hidden_word_freqs: FxHashMap<String, u32> = FxHashMap::default();
        for &doc in &hidden_docs {
            let i = offsets.partition_point(|&o| o <= doc) - 1;
            for word in segments[i].doc_words((doc - offsets[i]) as usize) {
                *hidden_word_freqs.entry(word).or_default() += 1;
            }
        }

        let mut index = Self {
            max_score: 0.0,
            segment_config,
            live_count: total - hidden_docs.len(),
            has_words: segments.iter().any(|s| !s.words.is_empty()),
            segments,
            offsets,
            hidden,
            hidden_word_freqs,
            short_cache: Mutex::default(),
            recursion_cache: Mutex::default(),
            vector_masks,
            vector_dim: vector_shape.map(|(dim, _)| dim),
            config,
        };
        index.max_score = match (index.config.max_score, index.segments.as_slice()) {
            (Some(score), _) => score,
            // A lone segment with every document live already knows it.
            (None, [seg])
                if index.live_count == seg.len()
                    && index.config.popularity_weight == POPULARITY_WEIGHT =>
            {
                seg.max_score()
            }
            (None, _) => index.estimate_max_score(),
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
    pub(crate) fn id(&self, doc: u32) -> u64 {
        let (i, local) = self.locate(doc);
        self.segments[i].id(local)
    }

    /// What [`Index::id_cmp`] orders by.
    pub(crate) fn id_key(&self, doc: u32) -> u64 {
        match self.segments.len() {
            1 => u64::from(doc),
            _ => self.id(doc),
        }
    }

    /// Orders documents by id; a lone segment holds its documents in id order.
    pub(crate) fn id_cmp(&self, a: u32, b: u32) -> std::cmp::Ordering {
        match self.segments.len() {
            1 => a.cmp(&b),
            _ => self.id(a).cmp(&self.id(b)),
        }
    }

    pub(crate) fn weight(&self, doc: u32) -> f32 {
        let (i, local) = self.locate(doc);
        self.segments[i].weight(local)
    }

    pub(crate) fn text_len(&self, doc: u32) -> u16 {
        let (i, local) = self.locate(doc);
        self.segments[i].text_len(local)
    }

    pub(crate) fn is_single_word(&self, doc: u32) -> bool {
        let (i, local) = self.locate(doc);
        self.segments[i].is_single_word(local)
    }

    /// Documents in all segments, live or not.
    pub(crate) fn doc_count(&self) -> usize {
        self.hidden.len()
    }

    pub fn len(&self) -> usize {
        self.live_count
    }

    pub fn is_empty(&self) -> bool {
        self.live_count == 0
    }

    pub fn document(&self, id: u64) -> Option<Document> {
        self.segments.iter().enumerate().rev().find_map(|(i, seg)| {
            let local = seg.local_of(id)?;
            (!self.hidden[self.offsets[i] as usize + local]).then(|| seg.document(local))
        })
    }

    /// Whether this index holds or deletes `id`, so that it overrides `id` in earlier layers.
    pub(crate) fn covers(&self, id: u64) -> bool {
        self.segments
            .iter()
            .any(|s| s.local_of(id).is_some() || s.deletes().binary_search(&id).is_ok())
    }

    /// The live document with string key `key`.
    pub fn document_by_key(&self, key: &str) -> Option<Document> {
        self.document(crate::key_id(key))
            .filter(|d| d.key.as_deref() == Some(key))
    }

    /// Segment and local position of document number `doc`.
    fn locate(&self, doc: u32) -> (usize, usize) {
        let i = match self.segments.len() {
            1 => 0,
            _ => self.offsets.partition_point(|&o| o <= doc) - 1,
        };
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
        let mut bits = vec![0u64; self.doc_count().div_ceil(64)];
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
                .filter(move |(l, _)| !self.hidden[base + l])
                .map(|(_, doc)| doc)
        })
    }

    /// Merges the live documents into one segment without deletes.
    pub fn compact(&self) -> Result<Segment, Error> {
        self.merged_segment(Vec::new(), None, 1)
    }

    /// Like [`Index::compact`], writing the segment to `path` as it is produced and mapping it.
    pub fn compact_to(&self, path: impl AsRef<std::path::Path>) -> Result<Segment, Error> {
        self.merged_segment(Vec::new(), Some(path.as_ref()), 1)
    }

    /// Like [`Index::compact`] on `build_threads` threads, written to `path` if given. Sections are
    /// still written one at a time, so memory does not grow with the threads.
    pub fn compact_with(
        &self,
        path: Option<&std::path::Path>,
        build_threads: usize,
    ) -> Result<Segment, Error> {
        self.merged_segment(Vec::new(), path, build_threads.max(1))
    }

    /// The live documents as one segment with `deletes`, merged from the segments' sorted
    /// structures, in memory or into the file at `path`.
    pub(crate) fn merged_segment(
        &self,
        deletes: Vec<u64>,
        path: Option<&std::path::Path>,
        build_threads: usize,
    ) -> Result<Segment, Error> {
        let bits = self
            .segments
            .iter()
            .find_map(|s| s.vectors.as_ref().map(|v| v.bits()));
        let config = BuildOptions {
            vector_bits: bits.unwrap_or(self.segment_config.vector_bits),
            build_threads,
            ..self.segment_config
        };
        let parts: Vec<crate::segment::Part> = (0..self.segments.len())
            .map(|i| crate::segment::Part {
                segment: &self.segments[i],
                hidden: self.segment_hidden(i),
            })
            .collect();
        Segment::merge(config, &parts, deletes, path)
    }

    /// Which documents of the `i`th segment are hidden.
    pub(crate) fn segment_hidden(&self, i: usize) -> &[bool] {
        let base = self.offsets[i] as usize;
        &self.hidden[base..base + self.segments[i].len()]
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
        hits.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(self.id_cmp(a.0, b.0)));
        hits.truncate(k);
        Ok(hits
            .into_iter()
            .map(|(doc, score)| self.suggestion(doc, score, crate::MatchKind::Semantic, None))
            .collect())
    }

    /// The 99th percentile of exact-match scores over live documents.
    pub fn estimate_max_score(&self) -> f64 {
        let factor = self.config.popularity_weight * 10.0;
        max_score_of(|| {
            (0..self.hidden.len())
                .into_par_iter()
                .filter(|&d| !self.hidden[d])
                .map(move |d| {
                    let doc = d as u32;
                    let (len, single) = (self.text_len(doc), self.is_single_word(doc));
                    exact_score(len, single, self.weight(doc), factor)
                })
        })
    }

    fn cursor<'s>(seg: &'s Segment, field: Field, prefix: &[u8]) -> KeyCursor<'s> {
        match field {
            Field::Title => KeyCursor::Titles(seg.title_cursor(prefix)),
            Field::Word => KeyCursor::Words(&seg.words, WordCursor::new(&seg.words, prefix)),
            Field::Alias => KeyCursor::Keyed(&seg.aliases, seg.aliases.map.cursor(prefix)),
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
            let mut cursor = Self::cursor(seg, field, prefix.as_bytes());
            while cursor.advance() {
                cursor.postings(|posting| {
                    let doc = base + Self::decode(field, posting).0;
                    if !self.hidden[doc as usize]
                        && allowed.as_ref().is_none_or(|a| a.contains(doc))
                    {
                        f(doc);
                    }
                });
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
        let mut cursors: Vec<(usize, KeyCursor)> = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, seg)| (i, Self::cursor(seg, field, prefix)))
            .filter_map(|(i, mut cursor)| cursor.advance().then_some((i, cursor)))
            .collect();
        let allowed = current_filter();
        let visible = |doc: u32| {
            !self.hidden[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc))
        };
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
                cursor.postings(|posting| {
                    let (local, kind) = Self::decode(field, posting);
                    if visible(base + local) {
                        entries.push((base + local, kind));
                    }
                });
            }
            cursors.retain_mut(|(_, cursor)| cursor.key() != key.as_slice() || cursor.advance());
            if sources > 1 {
                entries.sort_by_cached_key(|&(doc, kind)| (self.id_key(doc), kind));
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
            let push = |posting| {
                let (local, kind) = Self::decode(field, posting);
                let doc = self.offsets[i] + local;
                if !self.hidden[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc)) {
                    entries.push((doc, kind));
                }
            };
            let postings = match field {
                Field::Title => {
                    let titled = seg.titled(&key);
                    sources += usize::from(!titled.is_empty());
                    titled.into_iter().for_each(push);
                    continue;
                }
                Field::Word => seg.words.ordinal(&key).map(|o| seg.words.postings(o)),
                Field::Alias => seg.aliases.get(&key),
            };
            let Some(postings) = postings else {
                continue;
            };
            sources += 1;
            postings.for_each(push);
        }
        if sources > 1 {
            entries.sort_by_cached_key(|&(doc, kind)| (self.id_key(doc), kind));
        }
        entries
    }

    /// Calls `f(doc)` for up to `limit` live documents whose title starts with `prefix`, in title
    /// order and by id within a title.
    pub(crate) fn scan_titles(&self, prefix: &str, limit: usize, f: impl FnMut(u32)) {
        let allowed = current_filter();
        let visible = |doc: u32| {
            !self.hidden[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc))
        };
        let [seg] = self.segments.as_slice() else {
            return self.merge_titles(prefix, limit, visible, f);
        };
        let docs = seg
            .title_range(prefix.as_bytes())
            .filter_map(|i| seg.title_local(i))
            .filter(|&doc| visible(doc));
        docs.take(limit).for_each(f);
    }

    /// Up to `limit` visible documents whose title starts with `prefix`, heaviest first, then by
    /// title, then by id, however the index is segmented.
    pub(crate) fn top_titles(&self, prefix: &str, limit: usize) -> Vec<u32> {
        let allowed = current_filter();
        let visible = |doc: u32| {
            !self.hidden[doc as usize] && allowed.as_ref().is_none_or(|a| a.contains(doc))
        };
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        for (seg, &base) in self.segments.iter().zip(&self.offsets) {
            let mut taken = 0;
            seg.titles_by_weight(seg.title_range(prefix.as_bytes()), |local| {
                let doc = base + local;
                if visible(doc) {
                    out.push(doc);
                    taken += 1;
                }
                taken == limit
            });
        }
        if self.segments.len() > 1 {
            let mut keyed: Vec<(u32, Vec<u8>, u64, u32)> = out
                .iter()
                .map(|&doc| {
                    let (i, local) = self.locate(doc);
                    let mut key = Vec::new();
                    self.segments[i].title_key(local, &mut key);
                    (weight_order(self.weight(doc)), key, self.id(doc), doc)
                })
                .collect();
            keyed.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then(a.2.cmp(&b.2))
            });
            keyed.truncate(limit);
            out = keyed.into_iter().map(|e| e.3).collect();
        }
        out
    }

    /// As [`Index::scan_titles`] over several segments, merged in key order. A segment's titles
    /// below every other segment's next title go out as one run found by binary search, so few
    /// keys are read; equal titles across segments go out by id.
    fn merge_titles(
        &self,
        prefix: &str,
        limit: usize,
        visible: impl Fn(u32) -> bool,
        mut f: impl FnMut(u32),
    ) {
        // Per segment: its next position, its end, and the key there.
        let mut heads: Vec<(usize, usize, usize, Vec<u8>)> = Vec::new();
        for (i, seg) in self.segments.iter().enumerate() {
            let range = seg.title_range(prefix.as_bytes());
            if range.start < range.end {
                let mut key = Vec::new();
                seg.title_key_at(range.start, &mut key);
                heads.push((i, range.start, range.end, key));
            }
        }
        let mut emitted = 0;
        let mut group: Vec<u32> = Vec::new();
        while !heads.is_empty() {
            let m = (0..heads.len())
                .min_by(|&a, &b| heads[a].3.cmp(&heads[b].3))
                .expect("a head");
            let tied: Vec<usize> = (0..heads.len())
                .filter(|&h| heads[h].3 == heads[m].3)
                .collect();
            group.clear();
            if tied.len() > 1 {
                // One title in several segments: all its entries, by id.
                let mut after = heads[m].3.clone();
                after.push(0);
                for &h in &tied {
                    let (i, pos, end, _) = heads[h];
                    let (seg, base) = (&self.segments[i], self.offsets[i]);
                    let stop = seg.title_lower_bound_in(&after, pos, end);
                    group.extend(
                        (pos..stop)
                            .filter_map(|p| seg.title_local(p))
                            .map(|l| base + l),
                    );
                    heads[h].1 = stop;
                }
                group.sort_by_cached_key(|&doc| self.id_key(doc));
            } else {
                let next = (0..heads.len())
                    .filter(|&h| h != m)
                    .map(|h| heads[h].3.clone())
                    .min();
                let (i, pos, end, _) = heads[m];
                let (seg, base) = (&self.segments[i], self.offsets[i]);
                let stop = next.map_or(end, |k| seg.title_lower_bound_in(&k, pos, end));
                group.extend(
                    (pos..stop)
                        .filter_map(|p| seg.title_local(p))
                        .map(|l| base + l),
                );
                heads[m].1 = stop;
            }
            for &doc in group.iter().filter(|&&d| visible(d)) {
                f(doc);
                emitted += 1;
                if emitted == limit {
                    return;
                }
            }
            heads.retain(|h| h.1 < h.2);
            for h in heads.iter_mut() {
                self.segments[h.0].title_key_at(h.1, &mut h.3);
            }
        }
    }

    pub(crate) fn has_prefix(&self, field: Field, prefix: &str) -> bool {
        let mut found = false;
        match field {
            Field::Title => self.scan_titles(prefix, 1, |_| found = true),
            _ => self.scan(field, prefix, 1, |_, _, _| found = true),
        }
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
                // A bucket also holds the groups of other variants.
                for first in seg.variants.get(variant.as_bytes()) {
                    let words = seg.variants.group_at(first);
                    if words.start < words.end
                        && words.end <= word_count
                        && crate::fuzzy::is_variant(variant, seg.word_text(words.start), d, pc)
                    {
                        hits.extend(words);
                    }
                }
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

/// The keys of one field of one segment, in byte order.
enum KeyCursor<'a> {
    Keyed(&'a Keyed, crate::dict::Cursor<'a>),
    Words(&'a Words, WordCursor<'a>),
    Titles(TitleCursor<'a>),
}

impl KeyCursor<'_> {
    fn advance(&mut self) -> bool {
        match self {
            Self::Keyed(_, cursor) => cursor.advance(),
            Self::Words(_, cursor) => cursor.advance(),
            Self::Titles(cursor) => cursor.advance(),
        }
    }

    fn key(&self) -> &[u8] {
        match self {
            Self::Keyed(_, cursor) => cursor.key(),
            Self::Words(_, cursor) => cursor.key(),
            Self::Titles(cursor) => cursor.key(),
        }
    }

    fn postings(&self, f: impl FnMut(u32)) {
        match self {
            Self::Keyed(keyed, cursor) => keyed.postings(cursor.value()).for_each(f),
            Self::Words(words, cursor) => words.postings(cursor.value() as u32).for_each(f),
            Self::Titles(cursor) => cursor.locals().iter().copied().for_each(f),
        }
    }
}

/// Hides each document whose id a newer segment holds or deletes, by merging the segments' sorted
/// id and delete columns; ids held twice must carry the same key.
fn hide_superseded(
    segments: &[Arc<Segment>],
    offsets: &[u32],
    hidden: &mut [bool],
) -> Result<Vec<u32>, Error> {
    use std::cmp::Reverse;
    let mut hidden_docs = Vec::new();
    // (id, segment, is a delete, position)
    let at = |seg: usize, delete: bool, pos: usize| -> Option<u64> {
        let seg = &segments[seg];
        if delete {
            seg.deletes().get(pos).copied()
        } else {
            (pos < seg.len()).then(|| seg.id(pos))
        }
    };
    // Only segments whose id range meets a newer segment's ids or deletes can hide anything.
    let range = |seg: usize, delete: bool| {
        let len = if delete {
            segments[seg].deletes().len()
        } else {
            segments[seg].len()
        };
        at(seg, delete, 0).zip(at(seg, delete, len.wrapping_sub(1)))
    };
    let meets = |a: Option<(u64, u64)>, b: Option<(u64, u64)>| {
        a.zip(b)
            .is_some_and(|((a0, a1), (b0, b1))| a0 <= b1 && b0 <= a1)
    };
    let mut involved = vec![false; segments.len()];
    for older in 0..segments.len() {
        for newer in older + 1..segments.len() {
            let ids = range(older, false);
            if meets(ids, range(newer, false)) || meets(ids, range(newer, true)) {
                involved[older] = true;
                involved[newer] = true;
            }
        }
    }
    let mut heap = std::collections::BinaryHeap::new();
    for seg in (0..segments.len()).filter(|&s| involved[s]) {
        for delete in [false, true] {
            if let Some(id) = at(seg, delete, 0) {
                heap.push(Reverse((id, seg, delete, 0usize)));
            }
        }
    }
    let mut group: Vec<(usize, bool, usize)> = Vec::new();
    while let Some(Reverse((id, seg, delete, pos))) = heap.pop() {
        group.clear();
        group.push((seg, delete, pos));
        while heap.peek().is_some_and(|Reverse(top)| top.0 == id) {
            let Reverse((_, s, d, p)) = heap.pop().expect("peeked");
            group.push((s, d, p));
        }
        for &(s, d, p) in &group {
            if let Some(next) = at(s, d, p + 1) {
                heap.push(Reverse((next, s, d, p + 1)));
            }
        }
        if group.len() == 1 {
            continue;
        }
        // Newest first: every holder but the newest is hidden, as is one with a newer delete.
        group.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut newer: Option<(usize, Option<String>)> = None;
        let mut newest_delete: Option<usize> = None;
        for &(s, d, p) in &group {
            if d {
                newest_delete = newest_delete.max(Some(s));
                continue;
            }
            let key = segments[s].key(p);
            if newer.is_some() || newest_delete.is_some_and(|n| n > s) {
                let doc = offsets[s] + p as u32;
                if !std::mem::replace(&mut hidden[doc as usize], true) {
                    hidden_docs.push(doc);
                }
            }
            if let Some((_, Some(newer_key))) = &newer {
                if let Some(older) = &key {
                    if newer_key != older {
                        return Err(Error::input(format!(
                            "keys {older:?} and {newer_key:?} map to the same id {id}"
                        )));
                    }
                }
            }
            newer = Some((s, key));
        }
    }
    Ok(hidden_docs)
}
