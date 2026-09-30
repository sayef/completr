use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;

use crate::hybrid::fuse;
use crate::{
    AliasSuggestion, Error, HybridOptions, HybridSuggestion, Index, SearchOptions, Suggestion,
};

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

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LayeredSuggestion<S> {
    pub suggestion: S,
    /// Position in the requested layers of the index that produced the suggestion.
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

    /// Completes over `layers` in order, later layers overriding earlier ones per document id.
    /// Missing names are empty layers.
    pub fn complete(
        &self,
        layers: &[&str],
        query: &str,
        limit: usize,
    ) -> Vec<LayeredSuggestion<Suggestion>> {
        self.complete_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn complete_with(
        &self,
        layers: &[&str],
        query: &str,
        options: &SearchOptions,
    ) -> Vec<LayeredSuggestion<Suggestion>> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_complete(&resolved, query, options, self.overfetch)
    }

    pub fn complete_aliases(
        &self,
        layers: &[&str],
        query: &str,
        limit: usize,
    ) -> Vec<LayeredSuggestion<AliasSuggestion>> {
        self.complete_aliases_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn complete_aliases_with(
        &self,
        layers: &[&str],
        query: &str,
        options: &SearchOptions,
    ) -> Vec<LayeredSuggestion<AliasSuggestion>> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_complete_aliases(&resolved, query, options, self.overfetch)
    }

    /// Layered autocomplete on `text` fused with layered vector search on `vector`.
    pub fn hybrid_search(
        &self,
        layers: &[&str],
        text: &str,
        vector: &[f32],
        limit: usize,
        options: &HybridOptions,
    ) -> Result<Vec<HybridSuggestion>, Error> {
        options.validate()?;
        let search = options.search_options(limit);
        let lexical: Vec<(Suggestion, usize)> = self
            .complete_with(layers, text, &search)
            .into_iter()
            .map(|h| (h.suggestion, h.layer))
            .collect();
        let semantic: Vec<(Suggestion, usize)> = self
            .vector_search_with(layers, vector, &search)?
            .into_iter()
            .map(|h| (h.suggestion, h.layer))
            .collect();
        Ok(fuse(&lexical, &semantic, limit, options.fusion))
    }

    pub fn vector_search(
        &self,
        layers: &[&str],
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.vector_search_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn vector_search_with(
        &self,
        layers: &[&str],
        query: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        let indexes = self.indexes.load();
        let resolved: Vec<Option<&Index>> = layers
            .iter()
            .map(|n| indexes.get(*n).map(|i| &**i))
            .collect();
        layered_vector_search(&resolved, query, options, self.overfetch)
    }
}

fn fetch(layers: usize, options: &SearchOptions, overfetch: usize) -> SearchOptions {
    let limit = if layers > 1 {
        options.limit * overfetch
    } else {
        options.limit
    };
    options.clone().limit(limit)
}

/// With more than one layer, each is asked for `limit * overfetch` suggestions before merging.
pub fn layered_complete(
    layers: &[Option<&Index>],
    query: &str,
    options: &SearchOptions,
    overfetch: usize,
) -> Vec<LayeredSuggestion<Suggestion>> {
    let fetch = fetch(layers.len(), options, overfetch);
    let per_layer = layers
        .iter()
        .map(|l| {
            l.map(|index| index.complete_with(query, &fetch))
                .unwrap_or_default()
        })
        .collect();
    merge(layers, per_layer, options.limit, |s: &Suggestion| {
        (s.id, s.score)
    })
}

pub fn layered_complete_aliases(
    layers: &[Option<&Index>],
    query: &str,
    options: &SearchOptions,
    overfetch: usize,
) -> Vec<LayeredSuggestion<AliasSuggestion>> {
    let fetch = fetch(layers.len(), options, overfetch);
    let per_layer = layers
        .iter()
        .map(|l| {
            l.map(|index| index.complete_aliases_with(query, &fetch))
                .unwrap_or_default()
        })
        .collect();
    merge(layers, per_layer, options.limit, |s: &AliasSuggestion| {
        (s.id, s.score)
    })
}

pub fn layered_vector_search(
    layers: &[Option<&Index>],
    query: &[f32],
    options: &SearchOptions,
    overfetch: usize,
) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
    let fetch = fetch(layers.len(), options, overfetch);
    let mut per_layer = Vec::with_capacity(layers.len());
    for index in layers {
        per_layer.push(match index {
            Some(index) => index.vector_search_with(query, &fetch)?,
            None => Vec::new(),
        });
    }
    Ok(merge(layers, per_layer, options.limit, |s: &Suggestion| {
        (s.id, s.score)
    }))
}

/// Later layers override earlier ones per id, keeping the earlier position; then sorts by score.
/// A layer that holds or deletes an id hides it from earlier layers even when it does not match.
fn merge<S>(
    layers: &[Option<&Index>],
    per_layer: Vec<Vec<S>>,
    limit: usize,
    key: impl Fn(&S) -> (u64, f64),
) -> Vec<LayeredSuggestion<S>> {
    let mut merged: Vec<LayeredSuggestion<S>> = Vec::new();
    let mut position: FxHashMap<u64, usize> = FxHashMap::default();
    for (layer, results) in per_layer.into_iter().enumerate() {
        for suggestion in results {
            let id = key(&suggestion).0;
            let entry = LayeredSuggestion { suggestion, layer };
            match position.get(&id) {
                Some(&i) => merged[i] = entry,
                None => {
                    position.insert(id, merged.len());
                    merged.push(entry);
                }
            }
        }
    }
    merged.retain(|entry| {
        let id = key(&entry.suggestion).0;
        !layers[entry.layer + 1..]
            .iter()
            .flatten()
            .any(|index| index.covers(id))
    });
    merged.sort_by(|a, b| {
        key(&b.suggestion)
            .1
            .partial_cmp(&key(&a.suggestion).1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged.truncate(limit);
    merged
}
