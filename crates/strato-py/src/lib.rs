use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::collections::HashMap;

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyFileNotFoundError, PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

pyo3::create_exception!(
    strato,
    ConflictError,
    pyo3::exceptions::PyException,
    "A concurrent commit changed what this transaction depends on."
);

type DocumentTuple = (u64, String, f32, Vec<(String, bool)>);

fn to_py_err(e: strato_rs::Error) -> PyErr {
    match e {
        strato_rs::Error::Io(e) => PyIOError::new_err(e.to_string()),
        strato_rs::Error::NotFound(key) => PyFileNotFoundError::new_err(key),
        e @ strato_rs::Error::Store(_) => PyIOError::new_err(e.to_string()),
        e @ strato_rs::Error::Conflict(_) => ConflictError::new_err(e.to_string()),
        e => PyValueError::new_err(e.to_string()),
    }
}

fn to_document((id, text, weight, aliases): DocumentTuple) -> strato_rs::Document {
    let aliases = aliases
        .into_iter()
        .map(|(text, abbreviation)| strato_rs::Alias {
            text,
            kind: if abbreviation {
                strato_rs::AliasKind::Abbreviation
            } else {
                strato_rs::AliasKind::Synonym
            },
        })
        .collect();
    strato_rs::Document {
        id,
        text,
        weight,
        aliases,
        vector: None,
    }
}

/// Rows of a 2-D float32 buffer such as a numpy array, or a list of lists.
fn vector_rows(py: Python<'_>, vectors: &Bound<'_, PyAny>, rows: usize) -> PyResult<Vec<Vec<f32>>> {
    let out: Vec<Vec<f32>> = match PyBuffer::<f32>::get(vectors) {
        Ok(buffer) => {
            let shape = buffer.shape().to_vec();
            if shape.len() != 2 || shape[1] == 0 {
                return Err(PyValueError::new_err(format!(
                    "vectors must have shape (n, dim), got {shape:?}"
                )));
            }
            buffer
                .to_vec(py)?
                .chunks(shape[1])
                .map(<[f32]>::to_vec)
                .collect()
        }
        Err(_) => vectors.extract().map_err(|_| {
            PyValueError::new_err(
                "vectors must be a float32 array of shape (n, dim) or a list of float lists",
            )
        })?,
    };
    if out.len() != rows {
        return Err(PyValueError::new_err(format!(
            "{} vectors for {rows} documents",
            out.len()
        )));
    }
    Ok(out)
}

/// A 1-D float32 buffer such as a numpy array, or a list of floats.
fn query_vector(py: Python<'_>, vector: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    match PyBuffer::<f32>::get(vector) {
        Ok(buffer) if buffer.dimensions() == 1 => buffer.to_vec(py),
        Ok(_) => Err(PyValueError::new_err(
            "the query vector must be one-dimensional",
        )),
        Err(_) => vector.extract().map_err(|_| {
            PyValueError::new_err("the query vector must be a float32 array or a list of floats")
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn segment_config(
    min_word_chars: u8,
    max_edit_distance: u8,
    fuzzy_prefix_chars: u8,
    vector_bits: u8,
    compact_keys: bool,
    build_threads: usize,
) -> strato_rs::SegmentConfig {
    strato_rs::SegmentConfig {
        min_word_chars,
        max_edit_distance,
        fuzzy_prefix_chars,
        vector_bits,
        compact_keys,
        build_threads,
        ..strato_rs::SegmentConfig::default()
    }
}

fn build_segment(
    py: Python<'_>,
    documents: Vec<DocumentTuple>,
    deletes: Vec<u64>,
    vectors: Option<&Bound<'_, PyAny>>,
    config: strato_rs::SegmentConfig,
) -> PyResult<strato_rs::Segment> {
    let mut docs: Vec<strato_rs::Document> = documents.into_iter().map(to_document).collect();
    if let Some(vectors) = vectors {
        let rows = vector_rows(py, vectors, docs.len())?;
        for (doc, vector) in docs.iter_mut().zip(rows) {
            doc.vector = Some(vector);
        }
    }
    py.detach(|| strato_rs::Segment::build_with(config, docs, deletes))
        .map_err(to_py_err)
}

fn to_tuple(doc: strato_rs::Document) -> DocumentTuple {
    let aliases = doc
        .aliases
        .into_iter()
        .map(|a| (a.text, a.kind == strato_rs::AliasKind::Abbreviation))
        .collect();
    (doc.id, doc.text, doc.weight, aliases)
}

/// An immutable batch of documents `(id, text, weight, [(alias, is_abbreviation)])` plus deleted ids.
#[pyclass(frozen, module = "strato")]
struct Segment(Arc<strato_rs::Segment>);

#[pymethods]
impl Segment {
    /// `vectors`, optional, holds one embedding row per document, e.g. a float32 numpy array.
    #[staticmethod]
    #[pyo3(signature = (
        documents, deletes = Vec::new(), vectors = None, *,
        min_word_chars = 3, max_edit_distance = 2, fuzzy_prefix_chars = 7, vector_bits = 4, compact_keys = false, build_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn build(
        py: Python<'_>,
        documents: Vec<DocumentTuple>,
        deletes: Vec<u64>,
        vectors: Option<Bound<'_, PyAny>>,
        min_word_chars: u8,
        max_edit_distance: u8,
        fuzzy_prefix_chars: u8,
        vector_bits: u8,
        compact_keys: bool,
        build_threads: usize,
    ) -> PyResult<Self> {
        let config = segment_config(
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            compact_keys,
            build_threads,
        );
        Ok(Self(Arc::new(build_segment(
            py,
            documents,
            deletes,
            vectors.as_ref(),
            config,
        )?)))
    }

    #[getter]
    fn vector_dim(&self) -> Option<usize> {
        self.0.vector_dim()
    }

    #[getter]
    fn size_bytes(&self) -> usize {
        self.0.size_bytes()
    }

    #[staticmethod]
    fn open(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        Ok(Self(Arc::new(
            py.detach(|| strato_rs::Segment::open(path))
                .map_err(to_py_err)?,
        )))
    }

    #[staticmethod]
    fn from_bytes(py: Python<'_>, data: Vec<u8>) -> PyResult<Self> {
        Ok(Self(Arc::new(
            py.detach(|| strato_rs::Segment::from_bytes(data))
                .map_err(to_py_err)?,
        )))
    }

    fn to_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let data = py.detach(|| self.0.to_bytes());
        PyBytes::new(py, &data)
    }

    fn save(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        py.detach(|| self.0.save(path)).map_err(to_py_err)
    }

    fn ids(&self) -> Vec<u64> {
        self.0.ids().to_vec()
    }

    fn deletes(&self) -> Vec<u64> {
        self.0.deletes().to_vec()
    }

    fn documents(&self) -> Vec<DocumentTuple> {
        self.0.documents().map(to_tuple).collect()
    }

    fn __len__(&self) -> usize {
        self.0.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Segment(documents={}, deletes={})",
            self.0.len(),
            self.0.deletes().len()
        )
    }
}

#[pyclass(frozen, get_all, module = "strato")]
struct Suggestion {
    id: u64,
    score: f64,
    kind: &'static str,
    layer: Option<String>,
}

#[pymethods]
impl Suggestion {
    fn __repr__(&self) -> String {
        format!(
            "Suggestion(id={}, score={}, kind={:?}, layer={:?})",
            self.id, self.score, self.kind, self.layer
        )
    }
}

/// A fused hybrid result: `kind` is the lexical match kind when matched lexically, else "semantic".
#[pyclass(frozen, get_all, module = "strato")]
struct HybridSuggestion {
    id: u64,
    score: f64,
    kind: &'static str,
    lexical_score: Option<f64>,
    semantic_score: Option<f64>,
    layer: Option<String>,
}

#[pymethods]
impl HybridSuggestion {
    fn __repr__(&self) -> String {
        format!(
            "HybridSuggestion(id={}, score={}, kind={:?}, lexical_score={:?}, semantic_score={:?}, layer={:?})",
            self.id, self.score, self.kind, self.lexical_score, self.semantic_score, self.layer
        )
    }
}

fn hybrid_options(
    fusion: &str,
    rrf_k: f64,
    semantic_weight: f64,
    candidates: Option<usize>,
) -> PyResult<strato_rs::HybridOptions> {
    let fusion = match fusion {
        "rrf" => strato_rs::Fusion::ReciprocalRank { k: rrf_k },
        "weighted" => strato_rs::Fusion::Weighted { semantic_weight },
        "lexical_first" => strato_rs::Fusion::LexicalFirst,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown fusion {other:?}: use 'rrf', 'weighted' or 'lexical_first'"
            )))
        }
    };
    Ok(strato_rs::HybridOptions { fusion, candidates })
}

fn to_hybrid(h: strato_rs::HybridSuggestion, layer: Option<String>) -> HybridSuggestion {
    HybridSuggestion {
        id: h.id,
        score: h.score,
        kind: h.kind.as_str(),
        lexical_score: h.lexical_score,
        semantic_score: h.semantic_score,
        layer,
    }
}

#[pyclass(frozen, get_all, module = "strato")]
struct AliasSuggestion {
    id: u64,
    score: f64,
    layer: Option<String>,
}

#[pymethods]
impl AliasSuggestion {
    fn __repr__(&self) -> String {
        format!(
            "AliasSuggestion(id={}, score={}, layer={:?})",
            self.id, self.score, self.layer
        )
    }
}

/// Segments ordered oldest to newest, composed into one searchable view.
#[pyclass(frozen, module = "strato")]
struct Index(Arc<strato_rs::Index>);

#[pymethods]
impl Index {
    #[new]
    #[pyo3(signature = (
        segments, max_score = None, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
        short_query_cache_entries = 10_000, vector_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        segments: Vec<Py<Segment>>,
        max_score: Option<f64>,
        popularity_weight: f64,
        short_query_chars: usize,
        short_query_limit: usize,
        short_query_cache_entries: usize,
        vector_threads: usize,
    ) -> PyResult<Self> {
        let segments: Vec<_> = segments.iter().map(|s| s.get().0.clone()).collect();
        let config = strato_rs::IndexConfig {
            max_score,
            ..index_config(
                popularity_weight,
                short_query_chars,
                short_query_limit,
                short_query_cache_entries,
                vector_threads,
            )
        };
        Ok(Self(Arc::new(
            py.detach(|| strato_rs::Index::new(segments, config))
                .map_err(to_py_err)?,
        )))
    }

    /// Nearest documents by embedding, as hits of kind "semantic" scored by inner product.
    #[pyo3(signature = (vector, limit = 10))]
    fn vector_search(
        &self,
        py: Python<'_>,
        vector: &Bound<'_, PyAny>,
        limit: usize,
    ) -> PyResult<Vec<Suggestion>> {
        let query = query_vector(py, vector)?;
        let hits = py
            .detach(|| self.0.vector_search(&query, limit))
            .map_err(to_py_err)?;
        Ok(hits
            .into_iter()
            .map(|h| Suggestion {
                id: h.id,
                score: h.score,
                kind: h.kind.as_str(),
                layer: None,
            })
            .collect())
    }

    #[getter]
    fn vector_dim(&self) -> Option<usize> {
        self.0.vector_dim()
    }

    #[pyo3(signature = (query, limit = 10))]
    fn complete(&self, py: Python<'_>, query: &str, limit: usize) -> Vec<Suggestion> {
        let hits = py.detach(|| self.0.complete(query, limit));
        hits.into_iter()
            .map(|h| Suggestion {
                id: h.id,
                score: h.score,
                kind: h.kind.as_str(),
                layer: None,
            })
            .collect()
    }

    #[pyo3(signature = (query, limit = 10))]
    fn complete_aliases(&self, py: Python<'_>, query: &str, limit: usize) -> Vec<AliasSuggestion> {
        let hits = py.detach(|| self.0.complete_aliases(query, limit));
        hits.into_iter()
            .map(|h| AliasSuggestion {
                id: h.id,
                score: h.score,
                layer: None,
            })
            .collect()
    }

    /// Lexical and vector results fused by `fusion`: "rrf" (reciprocal rank, `rrf_k`),
    /// "weighted" (`semantic_weight` in [0, 1]) or "lexical_first".
    #[pyo3(signature = (text, vector, limit = 10, fusion = "rrf", rrf_k = 60.0, semantic_weight = 0.5, candidates = None))]
    #[allow(clippy::too_many_arguments)]
    fn hybrid_search(
        &self,
        py: Python<'_>,
        text: &str,
        vector: &Bound<'_, PyAny>,
        limit: usize,
        fusion: &str,
        rrf_k: f64,
        semantic_weight: f64,
        candidates: Option<usize>,
    ) -> PyResult<Vec<HybridSuggestion>> {
        let query = query_vector(py, vector)?;
        let options = hybrid_options(fusion, rrf_k, semantic_weight, candidates)?;
        let hits = py
            .detach(|| self.0.hybrid_search(text, &query, limit, options))
            .map_err(to_py_err)?;
        Ok(hits.into_iter().map(|h| to_hybrid(h, None)).collect())
    }

    fn get(&self, id: u64) -> Option<DocumentTuple> {
        self.0.document(id).map(to_tuple)
    }

    fn compact(&self, py: Python<'_>) -> PyResult<Segment> {
        Ok(Segment(Arc::new(
            py.detach(|| self.0.compact()).map_err(to_py_err)?,
        )))
    }

    fn segments(&self) -> Vec<Segment> {
        self.0
            .segments()
            .iter()
            .map(|s| Segment(s.clone()))
            .collect()
    }

    #[getter]
    fn max_score(&self) -> f64 {
        self.0.max_score()
    }

    fn __len__(&self) -> usize {
        self.0.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Index(documents={}, segments={}, max_score={})",
            self.0.len(),
            self.0.segments().len(),
            self.0.max_score()
        )
    }
}

/// Named indexes, published atomically; searches read one consistent set of them.
#[pyclass(frozen, module = "strato")]
struct Engine(Arc<strato_rs::Engine>);

#[pymethods]
impl Engine {
    /// With several layers, each is asked for `limit * overfetch` hits before merging.
    #[new]
    #[pyo3(signature = (overfetch = 2))]
    fn new(overfetch: usize) -> Self {
        Self(Arc::new(strato_rs::Engine::new().with_overfetch(overfetch)))
    }

    /// Replaces or, with `None`, removes the given indexes in one step.
    fn publish(&self, updates: &Bound<'_, PyDict>) -> PyResult<()> {
        let mut staged = Vec::with_capacity(updates.len());
        for (name, index) in updates.iter() {
            let index: Option<Py<Index>> = index.extract()?;
            staged.push((name.extract::<String>()?, index.map(|i| i.get().0.clone())));
        }
        self.0.publish(staged);
        Ok(())
    }

    fn get(&self, name: &str) -> Option<Index> {
        self.0.get(name).map(Index)
    }

    fn names(&self) -> Vec<String> {
        self.0.names()
    }

    /// Later layers override earlier ones per document id; missing names are empty layers.
    #[pyo3(signature = (query, layers, limit = 10))]
    fn complete(
        &self,
        py: Python<'_>,
        query: &str,
        layers: Vec<String>,
        limit: usize,
    ) -> Vec<Suggestion> {
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let hits = py.detach(|| self.0.complete(&names, query, limit));
        hits.into_iter()
            .map(|h| Suggestion {
                id: h.hit.id,
                score: h.hit.score,
                kind: h.hit.kind.as_str(),
                layer: Some(layers[h.layer].clone()),
            })
            .collect()
    }

    #[pyo3(signature = (query, layers, limit = 10))]
    fn complete_aliases(
        &self,
        py: Python<'_>,
        query: &str,
        layers: Vec<String>,
        limit: usize,
    ) -> Vec<AliasSuggestion> {
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let hits = py.detach(|| self.0.complete_aliases(&names, query, limit));
        hits.into_iter()
            .map(|h| AliasSuggestion {
                id: h.hit.id,
                score: h.hit.score,
                layer: Some(layers[h.layer].clone()),
            })
            .collect()
    }

    #[pyo3(signature = (text, vector, layers, limit = 10, fusion = "rrf", rrf_k = 60.0, semantic_weight = 0.5, candidates = None))]
    #[allow(clippy::too_many_arguments)]
    fn hybrid_search(
        &self,
        py: Python<'_>,
        text: &str,
        vector: &Bound<'_, PyAny>,
        layers: Vec<String>,
        limit: usize,
        fusion: &str,
        rrf_k: f64,
        semantic_weight: f64,
        candidates: Option<usize>,
    ) -> PyResult<Vec<HybridSuggestion>> {
        let query = query_vector(py, vector)?;
        let options = hybrid_options(fusion, rrf_k, semantic_weight, candidates)?;
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let hits = py
            .detach(|| self.0.hybrid_search(&names, text, &query, limit, options))
            .map_err(to_py_err)?;
        Ok(hits
            .into_iter()
            .map(|h| {
                let layer = Some(layers[h.layer].clone());
                to_hybrid(h, layer)
            })
            .collect())
    }

    #[pyo3(signature = (vector, layers, limit = 10))]
    fn vector_search(
        &self,
        py: Python<'_>,
        vector: &Bound<'_, PyAny>,
        layers: Vec<String>,
        limit: usize,
    ) -> PyResult<Vec<Suggestion>> {
        let query = query_vector(py, vector)?;
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let hits = py
            .detach(|| self.0.vector_search(&names, &query, limit))
            .map_err(to_py_err)?;
        Ok(hits
            .into_iter()
            .map(|h| Suggestion {
                id: h.hit.id,
                score: h.hit.score,
                kind: h.hit.kind.as_str(),
                layer: Some(layers[h.layer].clone()),
            })
            .collect())
    }
}

/// A local directory or `s3://`, `gs://`, `az://`, `file://`, `memory://` URL; keys are relative to it.
#[pyclass(frozen, module = "strato")]
struct Store(strato_rs::BlockingStore);

#[pymethods]
impl Store {
    #[new]
    #[pyo3(signature = (url, options = None, cache_dir = None))]
    fn new(
        py: Python<'_>,
        url: &str,
        options: Option<HashMap<String, String>>,
        cache_dir: Option<PathBuf>,
    ) -> PyResult<Self> {
        let options = options.unwrap_or_default();
        let store = py
            .detach(|| strato_rs::BlockingStore::open_with(url, options))
            .map_err(to_py_err)?;
        let store = match cache_dir {
            Some(dir) => store.with_cache_dir(dir).map_err(to_py_err)?,
            None => store,
        };
        Ok(Self(store))
    }

    /// Deletes cached segment files other than `keep`; returns how many.
    fn prune_cache(&self, keep: Vec<String>) -> PyResult<usize> {
        self.0
            .prune_cache(keep.iter().map(String::as_str))
            .map_err(to_py_err)
    }

    fn get<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, PyBytes>> {
        let data = py.detach(|| self.0.get(key)).map_err(to_py_err)?;
        Ok(PyBytes::new(py, &data))
    }

    fn put(&self, py: Python<'_>, key: &str, data: Vec<u8>) -> PyResult<()> {
        py.detach(|| self.0.put(key, data)).map_err(to_py_err)
    }

    /// Writes only if `key` is absent, atomically; returns whether it wrote.
    fn put_if_absent(&self, py: Python<'_>, key: &str, data: Vec<u8>) -> PyResult<bool> {
        py.detach(|| self.0.put_if_absent(key, data))
            .map_err(to_py_err)
    }

    fn delete(&self, py: Python<'_>, key: &str) -> PyResult<()> {
        py.detach(|| self.0.delete(key)).map_err(to_py_err)
    }

    #[pyo3(signature = (prefix = ""))]
    fn list(&self, py: Python<'_>, prefix: &str) -> PyResult<Vec<String>> {
        py.detach(|| self.0.list(prefix)).map_err(to_py_err)
    }

    fn put_segment(&self, py: Python<'_>, key: &str, segment: &Segment) -> PyResult<()> {
        py.detach(|| self.0.put_segment(key, &segment.0))
            .map_err(to_py_err)
    }

    fn get_segment(&self, py: Python<'_>, key: &str) -> PyResult<Segment> {
        Ok(Segment(Arc::new(
            py.detach(|| self.0.get_segment(key)).map_err(to_py_err)?,
        )))
    }
}

fn manifest_dict<'py>(
    py: Python<'py>,
    manifest: &strato_rs::Manifest,
) -> PyResult<Bound<'py, PyAny>> {
    py.import("json")?
        .call_method1("loads", (manifest.to_json(),))
}

fn index_config(
    popularity_weight: f64,
    short_query_chars: usize,
    short_query_limit: usize,
    short_query_cache_entries: usize,
    vector_threads: usize,
) -> strato_rs::IndexConfig {
    strato_rs::IndexConfig {
        popularity_weight,
        short_query_chars,
        short_query_limit,
        short_query_cache_entries,
        vector_threads,
        ..strato_rs::IndexConfig::default()
    }
}

/// Named indexes versioned under one store URL, committed optimistically.
#[pyclass(frozen, module = "strato")]
struct Database(strato_rs::Database);

#[pymethods]
impl Database {
    /// `cache_dir` keeps downloaded segments on local disk, memory-mapped. The keyword settings
    /// apply to segments this database builds from documents.
    #[new]
    #[pyo3(signature = (
        url, options = None, cache_dir = None, *,
        min_word_chars = 3, max_edit_distance = 2, fuzzy_prefix_chars = 7, vector_bits = 4, compact_keys = false, build_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        url: &str,
        options: Option<HashMap<String, String>>,
        cache_dir: Option<PathBuf>,
        min_word_chars: u8,
        max_edit_distance: u8,
        fuzzy_prefix_chars: u8,
        vector_bits: u8,
        compact_keys: bool,
        build_threads: usize,
    ) -> PyResult<Self> {
        let options = options.unwrap_or_default();
        let database = py
            .detach(|| strato_rs::block_on(strato_rs::Database::open(url, options)))
            .map_err(to_py_err)?;
        let database = match cache_dir {
            Some(dir) => database.with_cache_dir(dir).map_err(to_py_err)?,
            None => database,
        };
        let config = segment_config(
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            compact_keys,
            build_threads,
        );
        Ok(Self(database.with_segment_config(config)))
    }

    fn versions(&self, py: Python<'_>) -> PyResult<Vec<u64>> {
        py.detach(|| strato_rs::block_on(self.0.versions()))
            .map_err(to_py_err)
    }

    fn latest_version(&self, py: Python<'_>) -> PyResult<u64> {
        py.detach(|| strato_rs::block_on(self.0.latest_version()))
            .map_err(to_py_err)
    }

    /// The manifest of `version` (latest by default) as a dict.
    #[pyo3(signature = (version = None))]
    fn manifest<'py>(&self, py: Python<'py>, version: Option<u64>) -> PyResult<Bound<'py, PyAny>> {
        let manifest = py
            .detach(|| strato_rs::block_on(self.read(version)))
            .map_err(to_py_err)?;
        manifest_dict(py, &manifest)
    }

    /// A transaction against `version` (latest by default).
    #[pyo3(signature = (version = None))]
    fn begin(&self, py: Python<'_>, version: Option<u64>) -> PyResult<Transaction> {
        let manifest = py
            .detach(|| strato_rs::block_on(self.read(version)))
            .map_err(to_py_err)?;
        Ok(Transaction {
            txn: Mutex::new(Some(self.0.transaction(manifest))),
            config: self.0.segment_config(),
        })
    }

    #[pyo3(signature = (
        name, version = None, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
        short_query_cache_entries = 10_000, vector_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn load_index(
        &self,
        py: Python<'_>,
        name: &str,
        version: Option<u64>,
        popularity_weight: f64,
        short_query_chars: usize,
        short_query_limit: usize,
        short_query_cache_entries: usize,
        vector_threads: usize,
    ) -> PyResult<Index> {
        let config = index_config(
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        let index = py.detach(|| {
            strato_rs::block_on(async {
                let manifest = self.read(version).await?;
                self.0.load_index(&manifest, name, config).await
            })
        });
        Ok(Index(Arc::new(index.map_err(to_py_err)?)))
    }

    /// One compaction step, or with `until_done` as many as are due; returns the new version,
    /// or `None` if nothing was due.
    #[pyo3(signature = (index, fanout = 4, max_segments = 16, max_hidden_fraction = 0.25, until_done = false))]
    fn compact(
        &self,
        py: Python<'_>,
        index: &str,
        fanout: usize,
        max_segments: usize,
        max_hidden_fraction: f64,
        until_done: bool,
    ) -> PyResult<Option<u64>> {
        let policy = strato_rs::CompactionPolicy {
            fanout,
            max_segments,
            max_hidden_fraction,
        };
        let manifest = py
            .detach(|| {
                strato_rs::block_on(async {
                    if until_done {
                        self.0.compact_all(index, &policy).await
                    } else {
                        self.0.compact(index, &policy).await
                    }
                })
            })
            .map_err(to_py_err)?;
        Ok(manifest.map(|m| m.version))
    }

    /// Deletes manifests beyond the newest `keep_versions` and unreferenced segments, both only
    /// when older than `older_than_seconds`.
    #[pyo3(signature = (keep_versions = 10, older_than_seconds = 3600.0))]
    fn cleanup<'py>(
        &self,
        py: Python<'py>,
        keep_versions: usize,
        older_than_seconds: f64,
    ) -> PyResult<Bound<'py, PyDict>> {
        let policy = strato_rs::CleanupPolicy {
            keep_versions,
            older_than: Duration::from_secs_f64(older_than_seconds),
        };
        let stats = py
            .detach(|| strato_rs::block_on(self.0.cleanup(&policy)))
            .map_err(to_py_err)?;
        let out = PyDict::new(py);
        out.set_item("versions_removed", stats.versions_removed)?;
        out.set_item("segments_removed", stats.segments_removed)?;
        out.set_item("bytes_removed", stats.bytes_removed)?;
        Ok(out)
    }

    /// Queues `changes` for the database's ingestor; returns the change set's id.
    fn submit(&self, py: Python<'_>, changes: &mut ChangeSet) -> PyResult<String> {
        let inner = changes
            .0
            .take()
            .ok_or_else(|| PyValueError::new_err("change set already submitted"))?;
        py.detach(|| strato_rs::block_on(self.0.submit(inner)))
            .map_err(to_py_err)
    }

    fn pending_change_sets(&self, py: Python<'_>) -> PyResult<usize> {
        py.detach(|| strato_rs::block_on(self.0.pending_change_sets()))
            .map_err(to_py_err)
    }

    /// Takes the advisory lease `name`, or returns `None` while someone else holds it.
    fn acquire_lease(
        &self,
        py: Python<'_>,
        name: &str,
        owner: &str,
        ttl_seconds: f64,
    ) -> PyResult<Option<Lease>> {
        let ttl = Duration::from_secs_f64(ttl_seconds);
        let lease = py
            .detach(|| strato_rs::block_on(self.0.acquire_lease(name, owner, ttl)))
            .map_err(to_py_err)?;
        Ok(lease.map(|l| Lease(Mutex::new(Some(l)))))
    }
}

impl Database {
    async fn read(&self, version: Option<u64>) -> Result<strato_rs::Manifest, strato_rs::Error> {
        match version {
            Some(version) => self.0.manifest(version).await,
            None => self.0.latest().await,
        }
    }
}

/// Staged changes, committed together by `commit()`; usable once.
#[pyclass(frozen, module = "strato")]
struct Transaction {
    txn: Mutex<Option<strato_rs::Transaction>>,
    config: strato_rs::SegmentConfig,
}

impl Transaction {
    fn with<T>(&self, f: impl FnOnce(&mut strato_rs::Transaction) -> PyResult<T>) -> PyResult<T> {
        let mut guard = self.txn.lock().unwrap_or_else(|e| e.into_inner());
        let txn = guard
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("transaction already committed"))?;
        f(txn)
    }
}

#[pymethods]
impl Transaction {
    #[getter]
    fn read_version(&self) -> PyResult<u64> {
        self.with(|t| Ok(t.read_version()))
    }

    /// `vectors`, optional, holds one embedding row per document.
    #[pyo3(signature = (index, documents, deletes = Vec::new(), vectors = None))]
    fn append(
        &self,
        py: Python<'_>,
        index: &str,
        documents: Vec<DocumentTuple>,
        deletes: Vec<u64>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let segment = build_segment(py, documents, deletes, vectors.as_ref(), self.config)?;
        self.with(|t| {
            t.append(index, segment);
            Ok(())
        })
    }

    #[pyo3(signature = (index, documents, vectors = None))]
    fn overwrite(
        &self,
        py: Python<'_>,
        index: &str,
        documents: Vec<DocumentTuple>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let segment = build_segment(py, documents, Vec::new(), vectors.as_ref(), self.config)?;
        self.with(|t| {
            t.overwrite(index, segment);
            Ok(())
        })
    }

    fn drop_index(&self, index: &str) -> PyResult<()> {
        self.with(|t| {
            t.drop_index(index);
            Ok(())
        })
    }

    fn set_max_score(&self, index: &str, max_score: f64) -> PyResult<()> {
        self.with(|t| {
            t.set_max_score(index, max_score);
            Ok(())
        })
    }

    #[pyo3(signature = (key, value))]
    fn set_metadata(&self, key: &str, value: Option<&str>) -> PyResult<()> {
        self.with(|t| {
            t.set_metadata(key, value);
            Ok(())
        })
    }

    #[pyo3(signature = (strict = true))]
    fn strict(&self, strict: bool) -> PyResult<()> {
        self.with(|t| {
            t.strict(strict);
            Ok(())
        })
    }

    /// Manifest writes to attempt under contention before raising `ConflictError` (default 32).
    fn max_retries(&self, retries: usize) -> PyResult<()> {
        self.with(|t| {
            t.max_retries(retries);
            Ok(())
        })
    }

    /// Commits and returns the new manifest; raises `ConflictError` if it cannot be rebased.
    fn commit<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let txn = self.txn.lock().unwrap_or_else(|e| e.into_inner()).take();
        let txn = txn.ok_or_else(|| PyValueError::new_err("transaction already committed"))?;
        let manifest = py
            .detach(|| strato_rs::block_on(txn.commit()))
            .map_err(to_py_err)?;
        manifest_dict(py, &manifest)
    }
}

#[pyclass(frozen, module = "strato")]
struct Lease(Mutex<Option<strato_rs::Lease>>);

#[pymethods]
impl Lease {
    /// Fencing token; grows with every renewal.
    #[getter]
    fn generation(&self) -> PyResult<u64> {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|l| l.generation())
            .ok_or_else(|| PyValueError::new_err("lease released"))
    }

    /// Extends the lease; `False` means it was lost.
    fn renew(&self, py: Python<'_>, ttl_seconds: f64) -> PyResult<bool> {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let lease = guard
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("lease released"))?;
        py.detach(|| strato_rs::block_on(lease.renew(Duration::from_secs_f64(ttl_seconds))))
            .map_err(to_py_err)
    }

    fn release(&self, py: Python<'_>) -> PyResult<()> {
        let lease = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        match lease {
            Some(lease) => py
                .detach(|| strato_rs::block_on(lease.release()))
                .map_err(to_py_err),
            None => Ok(()),
        }
    }
}

/// Changes to one or more indexes, submitted together with `Database.submit`.
#[pyclass(module = "strato")]
struct ChangeSet(Option<strato_rs::ChangeSet>);

impl ChangeSet {
    fn inner(&mut self) -> PyResult<&mut strato_rs::ChangeSet> {
        self.0
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("change set already submitted"))
    }
}

#[pymethods]
impl ChangeSet {
    #[new]
    fn new() -> Self {
        Self(Some(strato_rs::ChangeSet::new()))
    }

    /// `vectors`, optional, holds one embedding row per document.
    #[pyo3(signature = (index, documents, vectors = None))]
    fn upsert(
        &mut self,
        py: Python<'_>,
        index: &str,
        documents: Vec<DocumentTuple>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let mut docs: Vec<strato_rs::Document> = documents.into_iter().map(to_document).collect();
        if let Some(vectors) = vectors {
            let rows = vector_rows(py, &vectors, docs.len())?;
            for (doc, vector) in docs.iter_mut().zip(rows) {
                doc.vector = Some(vector);
            }
        }
        self.inner()?.upsert(index, docs);
        Ok(())
    }

    fn delete(&mut self, index: &str, ids: Vec<u64>) -> PyResult<()> {
        self.inner()?.delete(index, ids);
        Ok(())
    }
}

/// Drains the inbox while it holds the ingestor lease; run one per process.
#[pyclass(module = "strato")]
struct Ingestor(Option<strato_rs::Ingestor>);

#[pymethods]
impl Ingestor {
    #[new]
    /// With `compact`, the ingestor compacts the indexes it committed to after each round.
    #[pyo3(signature = (database, owner, lease_ttl_seconds = 30.0, max_change_sets = 1000, compact = true))]
    fn new(
        database: &Database,
        owner: &str,
        lease_ttl_seconds: f64,
        max_change_sets: usize,
        compact: bool,
    ) -> Self {
        let mut ingestor = strato_rs::Ingestor::new(database.0.clone(), owner);
        ingestor.lease_ttl = Duration::from_secs_f64(lease_ttl_seconds);
        ingestor.max_change_sets = max_change_sets;
        if !compact {
            ingestor.compaction = None;
        }
        Self(Some(ingestor))
    }

    /// One round. Returns {"step": "standby" | "idle" | "committed", and for commits
    /// "version", "change_sets", "documents"}.
    fn run_once<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let ingestor = self
            .0
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("ingestor released"))?;
        let step = py
            .detach(|| strato_rs::block_on(ingestor.run_once()))
            .map_err(to_py_err)?;
        let out = PyDict::new(py);
        match step {
            strato_rs::IngestStep::Standby => out.set_item("step", "standby")?,
            strato_rs::IngestStep::Idle => out.set_item("step", "idle")?,
            strato_rs::IngestStep::Committed {
                version,
                change_sets,
                documents,
            } => {
                out.set_item("step", "committed")?;
                out.set_item("version", version)?;
                out.set_item("change_sets", change_sets)?;
                out.set_item("documents", documents)?;
            }
        }
        Ok(out)
    }

    #[getter]
    fn is_active(&self) -> bool {
        self.0.as_ref().is_some_and(strato_rs::Ingestor::is_active)
    }

    /// Gives up the lease so another process takes over at once.
    fn release(&mut self, py: Python<'_>) -> PyResult<()> {
        match self.0.take() {
            Some(ingestor) => py
                .detach(|| strato_rs::block_on(ingestor.release()))
                .map_err(to_py_err),
            None => Ok(()),
        }
    }
}

/// Keeps an `Engine` on a database's latest version, loading only what changed.
#[pyclass(frozen, module = "strato")]
struct Replica {
    replica: strato_rs::Replica,
    engine: Arc<strato_rs::Engine>,
}

#[pymethods]
impl Replica {
    #[new]
    #[pyo3(signature = (
        database, engine, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
        short_query_cache_entries = 10_000, vector_threads = 1, group_separator = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        database: &Database,
        engine: &Engine,
        popularity_weight: f64,
        short_query_chars: usize,
        short_query_limit: usize,
        short_query_cache_entries: usize,
        vector_threads: usize,
        group_separator: Option<&str>,
    ) -> Self {
        let config = index_config(
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        let replica = strato_rs::Replica::new(database.0.clone(), config);
        let replica = match group_separator {
            Some(separator) => replica.with_groups_by_suffix(separator),
            None => replica,
        };
        Self {
            replica,
            engine: engine.0.clone(),
        }
    }

    /// Publishes the latest version; returns it if it is new, else `None`.
    fn sync(&self, py: Python<'_>) -> PyResult<Option<u64>> {
        py.detach(|| strato_rs::block_on(self.replica.sync(&self.engine)))
            .map_err(to_py_err)
    }

    #[getter]
    fn version(&self, py: Python<'_>) -> u64 {
        py.detach(|| strato_rs::block_on(self.replica.version()))
    }
}

#[pymodule]
fn strato(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<Segment>()?;
    m.add_class::<Index>()?;
    m.add_class::<Engine>()?;
    m.add_class::<Store>()?;
    m.add_class::<Database>()?;
    m.add_class::<Transaction>()?;
    m.add_class::<Lease>()?;
    m.add_class::<Replica>()?;
    m.add_class::<ChangeSet>()?;
    m.add_class::<Ingestor>()?;
    m.add("ConflictError", m.py().get_type::<ConflictError>())?;
    m.add_class::<Suggestion>()?;
    m.add_class::<AliasSuggestion>()?;
    m.add_class::<HybridSuggestion>()?;
    Ok(())
}
