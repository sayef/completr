use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;

use crate::hybrid::fuse;
use crate::{AliasSuggestion, Error, HybridOptions, HybridSuggestion, Index, Suggestion};

type Indexes = FxHashMap<String, Arc<Index>>;

/// Named indexes, published atomically and searched lock-free.
pub struct Engine {
    indexes: ArcSwap<Indexes>,
    overfetch: usize,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            indexes: ArcSwap::default(),
            overfetch: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayeredSuggestion<H> {
    pub hit: H,
    /// Position in the requested layers of the index that produced the hit.
    pub layer: usize,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// With several layers, each is asked for `limit * overfetch` hits before merging (default 2).
    pub fn with_overfetch(mut self, overfetch: usize) -> Self {
        self.overfetch = overfetch.max(1);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<Index>> {
        self.indexes.load().get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.indexes.load().keys().cloned().collect();
        names.sort();
        names
    }

    /// Applies all updates in one step; `None` removes an index.
    pub fn publish(&self, updates: impl IntoIterator<Item = (String, Option<Arc<Index>>)>) {
        let updates: Vec<_> = updates.into_iter().collect();
        self.indexes.rcu(|current| {
            let mut next = Indexes::clone(current);
            for (name, index) in &updates {
                match index {
                    Some(index) => next.insert(name.clone(), index.clone()),
                    None => next.remove(name),
                };
            }
            next
        });
    }

    /// Searches `layers` in order, later layers overriding earlier ones per document id.
    /// Missing names are empty layers.
    pub fn complete(
        &self,
        layers: &[&str],
        query: &str,
        limit: usize,
    ) -> Vec<LayeredSuggestion<Suggestion>> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_autocomplete(&resolved, query, limit, self.overfetch)
    }

    pub fn complete_aliases(
        &self,
        layers: &[&str],
        query: &str,
        limit: usize,
    ) -> Vec<LayeredSuggestion<AliasSuggestion>> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_search_aliases(&resolved, query, limit, self.overfetch)
    }

    /// Layered autocomplete on `text` fused with layered vector search on `vector`.
    pub fn hybrid_search(
        &self,
        layers: &[&str],
        text: &str,
        vector: &[f32],
        limit: usize,
        options: HybridOptions,
    ) -> Result<Vec<HybridSuggestion>, Error> {
        options.validate()?;
        let n = options.candidates(limit);
        let lexical: Vec<(Suggestion, usize)> = self
            .complete(layers, text, n)
            .into_iter()
            .map(|h| (h.hit, h.layer))
            .collect();
        let semantic: Vec<(Suggestion, usize)> = self
            .vector_search(layers, vector, n)?
            .into_iter()
            .map(|h| (h.hit, h.layer))
            .collect();
        Ok(fuse(&lexical, &semantic, limit, options.fusion))
    }

    pub fn vector_search(
        &self,
        layers: &[&str],
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_vector_search(&resolved, query, limit, self.overfetch)
    }
}

fn fetch(layers: usize, limit: usize, overfetch: usize) -> usize {
    if layers > 1 {
        limit * overfetch
    } else {
        limit
    }
}

/// With more than one layer, each is asked for `limit * overfetch` hits before merging.
pub fn layered_autocomplete(
    layers: &[Option<&Index>],
    query: &str,
    limit: usize,
    overfetch: usize,
) -> Vec<LayeredSuggestion<Suggestion>> {
    let fetch = fetch(layers.len(), limit, overfetch);
    let per_layer = layers
        .iter()
        .map(|l| {
            l.map(|index| index.complete(query, fetch))
                .unwrap_or_default()
        })
        .collect();
    merge(per_layer, limit, |h: &Suggestion| (h.id, h.score))
}

pub fn layered_search_aliases(
    layers: &[Option<&Index>],
    query: &str,
    limit: usize,
    overfetch: usize,
) -> Vec<LayeredSuggestion<AliasSuggestion>> {
    let fetch = fetch(layers.len(), limit, overfetch);
    let per_layer = layers
        .iter()
        .map(|l| {
            l.map(|index| index.complete_aliases(query, fetch))
                .unwrap_or_default()
        })
        .collect();
    merge(per_layer, limit, |h: &AliasSuggestion| (h.id, h.score))
}

pub fn layered_vector_search(
    layers: &[Option<&Index>],
    query: &[f32],
    limit: usize,
    overfetch: usize,
) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
    let fetch = fetch(layers.len(), limit, overfetch);
    let mut per_layer = Vec::with_capacity(layers.len());
    for index in layers {
        per_layer.push(match index {
            Some(index) => index.vector_search(query, fetch)?,
            None => Vec::new(),
        });
    }
    Ok(merge(per_layer, limit, |h: &Suggestion| (h.id, h.score)))
}

/// Later layers override earlier ones per id, keeping the earlier position; then sorts by score.
fn merge<H: Copy>(
    per_layer: Vec<Vec<H>>,
    limit: usize,
    key: impl Fn(&H) -> (u64, f64),
) -> Vec<LayeredSuggestion<H>> {
    let mut hits: Vec<LayeredSuggestion<H>> = Vec::new();
    let mut position: FxHashMap<u64, usize> = FxHashMap::default();
    for (layer, results) in per_layer.into_iter().enumerate() {
        for hit in results {
            let entry = LayeredSuggestion { hit, layer };
            match position.get(&key(&hit).0) {
                Some(&i) => hits[i] = entry,
                None => {
                    position.insert(key(&hit).0, hits.len());
                    hits.push(entry);
                }
            }
        }
    }
    hits.sort_by(|a, b| {
        key(&b.hit)
            .1
            .partial_cmp(&key(&a.hit).1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    hits
}
