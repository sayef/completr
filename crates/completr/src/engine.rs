use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;

use crate::hybrid::fuse;
use crate::{
    AliasSuggestion, Error, HybridOptions, HybridSuggestion, Index, SearchOptions, Suggestion,
};

/// The namespace of indexes published or written without naming one.
pub const DEFAULT_NAMESPACE: &str = "default";

/// One version of a namespace: its indexes by name.
#[derive(Default)]
struct Snapshot {
    name: String,
    version: u64,
    indexes: FxHashMap<String, Arc<Index>>,
}

/// Named namespaces of named indexes, each published atomically and searched lock-free.
pub struct Engine {
    namespaces: ArcSwap<FxHashMap<String, Arc<Snapshot>>>,
    overfetch: usize,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            namespaces: ArcSwap::default(),
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

/// One version of one namespace, searched in layers. Every search through it sees that version,
/// however the engine changes meanwhile.
#[derive(Clone)]
pub struct Namespace {
    snapshot: Arc<Snapshot>,
    overfetch: usize,
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

    pub fn overfetch(&self) -> usize {
        self.overfetch
    }

    /// The current version of namespace `name`.
    pub fn namespace(&self, name: &str) -> Result<Namespace, Error> {
        match self.namespaces.load().get(name) {
            Some(snapshot) => Ok(self.handle(snapshot.clone())),
            None => Err(Error::NamespaceNotFound {
                name: name.to_owned(),
                available: self.namespaces(),
            }),
        }
    }

    pub fn namespaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self.namespaces.load().keys().cloned().collect();
        names.sort();
        names
    }

    /// The default namespace, empty if nothing was published to it.
    pub fn default_namespace(&self) -> Namespace {
        let snapshot = self.namespaces.load().get(DEFAULT_NAMESPACE).cloned();
        self.handle(snapshot.unwrap_or_else(|| {
            Arc::new(Snapshot {
                name: DEFAULT_NAMESPACE.to_owned(),
                ..Snapshot::default()
            })
        }))
    }

    fn handle(&self, snapshot: Arc<Snapshot>) -> Namespace {
        Namespace {
            snapshot,
            overfetch: self.overfetch,
        }
    }

    /// Applies all updates to the default namespace in one step; `None` removes an index.
    pub fn publish(&self, updates: impl IntoIterator<Item = (String, Option<Arc<Index>>)>) {
        self.publish_to(DEFAULT_NAMESPACE, updates);
    }

    /// Applies all updates to `namespace` in one step, as its next version. A namespace left
    /// without indexes is removed.
    pub fn publish_to(
        &self,
        namespace: &str,
        updates: impl IntoIterator<Item = (String, Option<Arc<Index>>)>,
    ) {
        self.update(namespace, None, updates.into_iter().collect());
    }

    pub(crate) fn update(
        &self,
        namespace: &str,
        version: Option<u64>,
        updates: Vec<(String, Option<Arc<Index>>)>,
    ) {
        self.namespaces.rcu(|current| {
            let mut next = FxHashMap::clone(current);
            let previous = current.get(namespace);
            let mut indexes = previous.map(|s| s.indexes.clone()).unwrap_or_default();
            for (name, index) in &updates {
                match index {
                    Some(index) => indexes.insert(name.clone(), index.clone()),
                    None => indexes.remove(name),
                };
            }
            if indexes.is_empty() {
                next.remove(namespace);
            } else {
                let version = version.unwrap_or_else(|| previous.map_or(0, |s| s.version) + 1);
                let snapshot = Snapshot {
                    name: namespace.to_owned(),
                    version,
                    indexes,
                };
                next.insert(namespace.to_owned(), Arc::new(snapshot));
            }
            next
        });
    }

    /// An index of the default namespace.
    pub fn get(&self, name: &str) -> Option<Arc<Index>> {
        self.default_namespace().get(name)
    }

    /// Indexes of the default namespace.
    pub fn names(&self) -> Vec<String> {
        self.default_namespace().names()
    }

    /// [`Namespace::complete`] in the default namespace.
    pub fn complete(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.default_namespace().complete(layers, query, limit)
    }

    pub fn complete_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.default_namespace()
            .complete_with(layers, query, options)
    }

    pub fn complete_aliases(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<AliasSuggestion>>, Error> {
        self.default_namespace()
            .complete_aliases(layers, query, limit)
    }

    pub fn complete_aliases_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<AliasSuggestion>>, Error> {
        self.default_namespace()
            .complete_aliases_with(layers, query, options)
    }

    pub fn hybrid_search(
        &self,
        layers: &[impl AsRef<str>],
        text: &str,
        vector: &[f32],
        limit: usize,
        options: &HybridOptions,
    ) -> Result<Vec<HybridSuggestion>, Error> {
        self.default_namespace()
            .hybrid_search(layers, text, vector, limit, options)
    }

    pub fn vector_search(
        &self,
        layers: &[impl AsRef<str>],
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.default_namespace().vector_search(layers, query, limit)
    }

    pub fn vector_search_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.default_namespace()
            .vector_search_with(layers, query, options)
    }
}

impl Namespace {
    pub fn name(&self) -> &str {
        &self.snapshot.name
    }

    /// The database version this namespace was synced at, or the count of its publishes.
    pub fn version(&self) -> u64 {
        self.snapshot.version
    }

    pub fn get(&self, name: &str) -> Option<Arc<Index>> {
        self.snapshot.indexes.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.snapshot.indexes.keys().cloned().collect();
        names.sort();
        names
    }

    fn resolve(
        &self,
        layers: &[impl AsRef<str>],
        options: &SearchOptions,
    ) -> Result<Vec<Option<&Index>>, Error> {
        layers
            .iter()
            .map(|layer| match self.snapshot.indexes.get(layer.as_ref()) {
                Some(index) => Ok(Some(&**index)),
                None if options.ignore_missing_layers => Ok(None),
                None => Err(Error::LayerNotFound {
                    namespace: self.snapshot.name.clone(),
                    version: self.snapshot.version,
                    name: layer.as_ref().to_owned(),
                    available: self.names(),
                }),
            })
            .collect()
    }

    /// Completes over `layers` in order, later layers overriding earlier ones per document id. A
    /// missing layer is an error unless [`SearchOptions::ignore_missing_layers`] is set.
    pub fn complete(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.complete_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn complete_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        let resolved = self.resolve(layers, options)?;
        Ok(layered_complete(&resolved, query, options, self.overfetch))
    }

    pub fn complete_aliases(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<AliasSuggestion>>, Error> {
        self.complete_aliases_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn complete_aliases_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<AliasSuggestion>>, Error> {
        let resolved = self.resolve(layers, options)?;
        Ok(layered_complete_aliases(
            &resolved,
            query,
            options,
            self.overfetch,
        ))
    }

    /// Layered autocomplete on `text` fused with layered vector search on `vector`.
    pub fn hybrid_search(
        &self,
        layers: &[impl AsRef<str>],
        text: &str,
        vector: &[f32],
        limit: usize,
        options: &HybridOptions,
    ) -> Result<Vec<HybridSuggestion>, Error> {
        let resolved = self.resolve(layers, &options.search_options(limit))?;
        layered_hybrid_search(&resolved, text, vector, limit, options, self.overfetch)
    }

    pub fn vector_search(
        &self,
        layers: &[impl AsRef<str>],
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        self.vector_search_with(layers, query, &SearchOptions::new(limit))
    }

    pub fn vector_search_with(
        &self,
        layers: &[impl AsRef<str>],
        query: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<LayeredSuggestion<Suggestion>>, Error> {
        let resolved = self.resolve(layers, options)?;
        layered_vector_search(&resolved, query, options, self.overfetch)
    }
}

impl std::fmt::Debug for Namespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Namespace")
            .field("name", &self.snapshot.name)
            .field("version", &self.snapshot.version)
            .field("indexes", &self.names())
            .finish()
    }
}

/// Layered autocomplete fused with layered vector search.
pub(crate) fn layered_hybrid_search(
    layers: &[Option<&Index>],
    text: &str,
    vector: &[f32],
    limit: usize,
    options: &HybridOptions,
    overfetch: usize,
) -> Result<Vec<HybridSuggestion>, Error> {
    options.validate()?;
    let search = options.search_options(limit);
    let lexical: Vec<(Suggestion, usize)> = layered_complete(layers, text, &search, overfetch)
        .into_iter()
        .map(|h| (h.suggestion, h.layer))
        .collect();
    let semantic: Vec<(Suggestion, usize)> =
        layered_vector_search(layers, vector, &search, overfetch)?
            .into_iter()
            .map(|h| (h.suggestion, h.layer))
            .collect();
    Ok(fuse(&lexical, &semantic, limit, options.fusion))
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
            l.map(|index| {
                let mut hits = index.complete_with(query, &fetch);
                let scale = common_scale(layers, index);
                hits.iter_mut().for_each(|s| s.score *= scale);
                hits
            })
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
            l.map(|index| {
                let mut hits = index.complete_aliases_with(query, &fetch);
                let scale = common_scale(layers, index);
                hits.iter_mut().for_each(|s| s.score *= scale);
                hits
            })
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

/// The factor that puts `index`'s scores on the first layer's scale. Each index normalises by its own
/// `max_score`, so a small layer's scores would not be comparable with a large one's.
fn common_scale(layers: &[Option<&Index>], index: &Index) -> f64 {
    match layers.iter().flatten().next() {
        Some(first) if first.max_score() > 0.0 => index.max_score() / first.max_score(),
        _ => 1.0,
    }
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
