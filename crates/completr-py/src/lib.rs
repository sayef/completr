use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBytes, PyDict, PyString, PyType};

fn error_class(py: Python<'_>, name: &str) -> PyResult<Py<PyType>> {
    static MODULE: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
    let module = MODULE.get_or_try_init(py, || py.import("completr._errors").map(Bound::unbind))?;
    Ok(module
        .bind(py)
        .getattr(name)?
        .cast_into::<PyType>()?
        .unbind())
}

fn raise(name: &str, message: String) -> PyErr {
    Python::attach(|py| match error_class(py, name) {
        Ok(class) => PyErr::from_type(class.into_bound(py), message),
        Err(e) => e,
    })
}

fn invalid(message: impl Into<String>) -> PyErr {
    raise("InvalidInputError", message.into())
}

fn to_py_err(e: completr_rs::Error) -> PyErr {
    let name = match &e {
        completr_rs::Error::Io(_) | completr_rs::Error::Store(_) => "StorageError",
        completr_rs::Error::NotFound(_) => "NotFoundError",
        completr_rs::Error::Conflict(_) => "ConflictError",
        completr_rs::Error::Corrupt(_) | completr_rs::Error::Fst(_) => "CorruptionError",
        completr_rs::Error::InvalidInput(_) => "InvalidInputError",
        _ => "CompletrError",
    };
    raise(name, e.to_string())
}

/// A document id as Python sees it: an int, or a string key.
#[derive(FromPyObject)]
enum Id {
    Int(u64),
    Key(String),
}

impl Id {
    fn numeric(&self) -> u64 {
        match self {
            Self::Int(id) => *id,
            Self::Key(key) => completr_rs::key_id(key),
        }
    }
}

fn py_id(py: Python<'_>, id: u64, key: Option<&str>) -> PyResult<Py<PyAny>> {
    Ok(match key {
        Some(key) => key.into_pyobject(py)?.into_any().unbind(),
        None => id.into_pyobject(py)?.into_any().unbind(),
    })
}

/// Rows of a 2-D float32 buffer such as a numpy array, or a list of lists.
fn vector_rows(py: Python<'_>, vectors: &Bound<'_, PyAny>, rows: usize) -> PyResult<Vec<Vec<f32>>> {
    let out: Vec<Vec<f32>> = match PyBuffer::<f32>::get(vectors) {
        Ok(buffer) => {
            let shape = buffer.shape().to_vec();
            if shape.len() != 2 || shape[1] == 0 {
                return Err(invalid(format!(
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
            invalid("vectors must be a float32 array of shape (n, dim) or a list of float lists")
        })?,
    };
    if out.len() != rows {
        return Err(invalid(format!(
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
        Ok(_) => Err(invalid("a vector must be one-dimensional")),
        Err(_) => vector
            .extract()
            .map_err(|_| invalid("a vector must be a float32 array or a list of floats")),
    }
}

const FIELDS: [&str; 7] = [
    "id",
    "text",
    "popularity",
    "synonyms",
    "abbreviations",
    "contexts",
    "vector",
];

/// Strings from a list-like value; `None` and pandas' NaN mean none.
fn strings(value: &Bound<'_, PyAny>, field: &str) -> PyResult<Vec<String>> {
    if value.is_none() || value.extract::<f64>().is_ok_and(f64::is_nan) {
        return Ok(Vec::new());
    }
    if value.is_instance_of::<PyString>() {
        return Err(invalid(format!(
            "{field} must be a list of strings, not a string"
        )));
    }
    value
        .try_iter()?
        .map(|v| v?.extract::<String>())
        .collect::<PyResult<_>>()
        .map_err(|_| invalid(format!("{field} must be a list of strings")))
}

fn document_from_dict(py: Python<'_>, dict: &Bound<'_, PyDict>) -> PyResult<completr_rs::Document> {
    for key in dict.keys() {
        let key: String = key.extract()?;
        if !FIELDS.contains(&key.as_str()) {
            return Err(invalid(format!(
                "unknown document field {key:?}; expected {}",
                FIELDS.join(", ")
            )));
        }
    }
    let get = |name: &str| dict.get_item(name).ok().flatten().filter(|v| !v.is_none());
    let id: Id = get("id")
        .ok_or_else(|| invalid("a document needs an id"))?
        .extract()
        .map_err(|_| invalid("a document id must be an int or a string"))?;
    let text: String = get("text")
        .ok_or_else(|| invalid("a document needs a text"))?
        .extract()?;
    let popularity: f32 = get("popularity").map_or(Ok(0.0), |p| p.extract())?;
    let mut doc = match id {
        Id::Int(id) => completr_rs::Document::new(id, text, popularity),
        Id::Key(key) => completr_rs::Document::keyed(key, text, popularity),
    };
    if let Some(v) = get("synonyms") {
        for s in strings(&v, "synonyms")? {
            doc = doc.with_synonym(s);
        }
    }
    if let Some(v) = get("abbreviations") {
        for s in strings(&v, "abbreviations")? {
            doc = doc.with_abbreviation(s);
        }
    }
    if let Some(v) = get("contexts") {
        for s in strings(&v, "contexts")? {
            doc = doc.with_context(s);
        }
    }
    if let Some(v) = get("vector") {
        doc = doc.with_vector(query_vector(py, &v)?);
    }
    Ok(doc)
}

/// Documents from dicts, `Document`s, or a pandas, polars or pyarrow table.
/// Documents from dicts, `completr.Document`s or a table, each passed to `add` as it is read.
fn for_each_document(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    mut add: impl FnMut(completr_rs::Document) -> PyResult<()>,
) -> PyResult<()> {
    let rows = if source.hasattr("to_pylist")? {
        source.call_method0("to_pylist")?
    } else if source.hasattr("to_dicts")? {
        source.call_method0("to_dicts")?
    } else if source.hasattr("to_dict")? && source.hasattr("columns")? {
        source.call_method1("to_dict", ("records",))?
    } else {
        source.clone()
    };
    for item in rows.try_iter()? {
        let item = item?;
        if let Ok(doc) = item.cast::<Document>() {
            add(doc.get().0.clone())?;
        } else if let Ok(dict) = item.cast::<PyDict>() {
            add(document_from_dict(py, dict)?)?;
        } else {
            return Err(PyTypeError::new_err(format!(
                "documents must be dicts or completr.Document, not {}",
                item.get_type().name()?
            )));
        }
    }
    Ok(())
}

fn documents(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    vectors: Option<&Bound<'_, PyAny>>,
) -> PyResult<Vec<completr_rs::Document>> {
    let mut docs = Vec::new();
    for_each_document(py, source, |doc| {
        docs.push(doc);
        Ok(())
    })?;
    if let Some(vectors) = vectors {
        let rows = vector_rows(py, vectors, docs.len())?;
        for (doc, vector) in docs.iter_mut().zip(rows) {
            doc.vector = Some(vector);
        }
    }
    Ok(docs)
}

/// A builder holding `source`'s documents; without `vectors` they stream in, never all held as
/// documents at once.
fn builder(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    vectors: Option<&Bound<'_, PyAny>>,
    options: completr_rs::BuildOptions,
) -> PyResult<completr_rs::SegmentBuilder> {
    let mut builder = completr_rs::SegmentBuilder::new(options);
    match vectors {
        Some(_) => {
            for doc in documents(py, source, vectors)? {
                builder.add(doc).map_err(to_py_err)?;
            }
        }
        None => for_each_document(py, source, |doc| builder.add(doc).map_err(to_py_err))?,
    }
    Ok(builder)
}

fn numeric_ids(values: Vec<Id>) -> Vec<u64> {
    values.iter().map(Id::numeric).collect()
}

/// Byte ranges of `text` as `(start, end)` char offsets.
fn char_ranges(text: &str, ranges: &[Range<usize>]) -> Vec<(usize, usize)> {
    let offset = |byte: usize| text[..byte].chars().count();
    ranges
        .iter()
        .map(|r| (offset(r.start), offset(r.end)))
        .collect()
}

fn build_options(
    min_word_chars: u8,
    max_edit_distance: u8,
    fuzzy_prefix_chars: u8,
    vector_bits: u8,
    compact_keys: bool,
    build_threads: usize,
) -> completr_rs::BuildOptions {
    completr_rs::BuildOptions::default()
        .min_word_chars(min_word_chars)
        .max_edit_distance(max_edit_distance)
        .fuzzy_prefix_chars(fuzzy_prefix_chars)
        .vector_bits(vector_bits)
        .compact_keys(compact_keys)
        .build_threads(build_threads)
}

fn index_options(
    max_score: Option<f64>,
    popularity_weight: f64,
    short_query_chars: usize,
    short_query_limit: usize,
    short_query_cache_entries: usize,
    vector_threads: usize,
) -> completr_rs::IndexOptions {
    let options = completr_rs::IndexOptions::default()
        .popularity_weight(popularity_weight)
        .short_query_chars(short_query_chars)
        .short_query_limit(short_query_limit)
        .short_query_cache_entries(short_query_cache_entries)
        .vector_threads(vector_threads);
    match max_score {
        Some(score) => options.max_score(score),
        None => options,
    }
}

fn search_options(limit: usize, contexts: Option<Vec<String>>) -> completr_rs::SearchOptions {
    completr_rs::SearchOptions::new(limit).contexts(contexts.unwrap_or_default())
}

fn hybrid_options(
    fusion: &str,
    rrf_k: f64,
    semantic_weight: f64,
    candidates: Option<usize>,
    contexts: Option<Vec<String>>,
) -> PyResult<completr_rs::HybridOptions> {
    let fusion = match fusion {
        "rrf" => completr_rs::Fusion::ReciprocalRank { k: rrf_k },
        "weighted" => completr_rs::Fusion::Weighted { semantic_weight },
        "lexical_first" => completr_rs::Fusion::LexicalFirst,
        other => {
            return Err(invalid(format!(
                "unknown fusion {other:?}: use 'rrf', 'weighted' or 'lexical_first'"
            )))
        }
    };
    let options = completr_rs::HybridOptions::default()
        .fusion(fusion)
        .contexts(contexts.unwrap_or_default());
    Ok(match candidates {
        Some(n) => options.candidates(n),
        None => options,
    })
}

/// One completable entry. `id` is an int or a string key.
#[pyclass(frozen, module = "completr")]
struct Document(completr_rs::Document);

#[pymethods]
impl Document {
    #[new]
    #[pyo3(signature = (id, text, popularity = 0.0, *, synonyms = Vec::new(), abbreviations = Vec::new(), contexts = Vec::new(), vector = None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        id: Id,
        text: String,
        popularity: f32,
        synonyms: Vec<String>,
        abbreviations: Vec<String>,
        contexts: Vec<String>,
        vector: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let mut doc = match id {
            Id::Int(id) => completr_rs::Document::new(id, text, popularity),
            Id::Key(key) => completr_rs::Document::keyed(key, text, popularity),
        };
        for s in synonyms {
            doc = doc.with_synonym(s);
        }
        for a in abbreviations {
            doc = doc.with_abbreviation(a);
        }
        for c in contexts {
            doc = doc.with_context(c);
        }
        if let Some(v) = vector {
            doc = doc.with_vector(query_vector(py, &v)?);
        }
        Ok(Self(doc))
    }

    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py_id(py, self.0.id, self.0.key.as_deref())
    }

    #[getter]
    fn text(&self) -> &str {
        &self.0.text
    }

    #[getter]
    fn popularity(&self) -> f32 {
        self.0.popularity
    }

    #[getter]
    fn synonyms(&self) -> Vec<String> {
        self.aliases(completr_rs::AliasKind::Synonym)
    }

    #[getter]
    fn abbreviations(&self) -> Vec<String> {
        self.aliases(completr_rs::AliasKind::Abbreviation)
    }

    #[getter]
    fn contexts(&self) -> Vec<String> {
        self.0.contexts.clone()
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        out.set_item("id", self.id(py)?)?;
        out.set_item("text", &self.0.text)?;
        out.set_item("popularity", self.0.popularity)?;
        out.set_item("synonyms", self.synonyms())?;
        out.set_item("abbreviations", self.abbreviations())?;
        out.set_item("contexts", self.contexts())?;
        Ok(out)
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other.cast::<Document>().is_ok_and(|o| o.get().0 == self.0)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Document(id={}, text={:?}, popularity={})",
            self.id(py)?.bind(py).repr()?,
            self.0.text,
            self.0.popularity
        ))
    }
}

impl Document {
    fn aliases(&self, kind: completr_rs::AliasKind) -> Vec<String> {
        self.0
            .aliases
            .iter()
            .filter(|a| a.kind == kind)
            .map(|a| a.text.clone())
            .collect()
    }
}

/// An immutable batch of documents plus deleted ids.
#[pyclass(frozen, module = "completr")]
struct Segment(Arc<completr_rs::Segment>);

#[pymethods]
impl Segment {
    /// `vectors`, optional, holds one embedding row per document, e.g. a float32 numpy array.
    /// With `path`, the segment is written there as it is built and opened from it.
    #[staticmethod]
    #[pyo3(signature = (
        documents, deletes = Vec::new(), vectors = None, *,
        min_word_chars = 3, max_edit_distance = 2, fuzzy_prefix_chars = 7, vector_bits = 4, compact_keys = false, build_threads = 1,
        path = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn build(
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
        deletes: Vec<Id>,
        vectors: Option<Bound<'_, PyAny>>,
        min_word_chars: u8,
        max_edit_distance: u8,
        fuzzy_prefix_chars: u8,
        vector_bits: u8,
        compact_keys: bool,
        build_threads: usize,
        path: Option<PathBuf>,
    ) -> PyResult<Self> {
        let options = build_options(
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            compact_keys,
            build_threads,
        );
        let mut builder = builder(py, documents, vectors.as_ref(), options)?;
        for id in numeric_ids(deletes) {
            builder.delete(id);
        }
        let segment = py
            .detach(|| match path {
                Some(path) => builder.write(path),
                None => builder.build(),
            })
            .map_err(to_py_err)?;
        Ok(Self(Arc::new(segment)))
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
            py.detach(|| completr_rs::Segment::open(path))
                .map_err(to_py_err)?,
        )))
    }

    /// Checks the checksum and every section; `open` checks only the structure.
    fn verify(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.0.verify()).map_err(to_py_err)
    }

    #[staticmethod]
    fn from_bytes(py: Python<'_>, data: Vec<u8>) -> PyResult<Self> {
        Ok(Self(Arc::new(
            py.detach(|| completr_rs::Segment::from_bytes(data))
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

    fn documents(&self) -> Vec<Document> {
        self.0.documents().map(Document).collect()
    }

    /// Numeric ids of the documents, ascending.
    fn ids(&self) -> Vec<u64> {
        self.0.ids().to_vec()
    }

    /// Numeric ids this segment deletes from older segments.
    fn deletes(&self) -> Vec<u64> {
        self.0.deletes().to_vec()
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

/// One completion. `highlights` are `(start, end)` character offsets into `text`.
#[pyclass(frozen, get_all, module = "completr")]
struct Suggestion {
    id: Py<PyAny>,
    text: String,
    score: f64,
    kind: &'static str,
    highlights: Vec<(usize, usize)>,
    layer: Option<String>,
}

impl Suggestion {
    fn from(py: Python<'_>, s: completr_rs::Suggestion, layer: Option<String>) -> PyResult<Self> {
        Ok(Self {
            id: py_id(py, s.id, s.key.as_deref())?,
            highlights: char_ranges(&s.text, &s.highlights),
            text: s.text,
            score: s.score,
            kind: s.kind.as_str(),
            layer,
        })
    }
}

#[pymethods]
impl Suggestion {
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Suggestion(id={}, text={:?}, score={:.4}, kind={:?})",
            self.id.bind(py).repr()?,
            self.text,
            self.score,
            self.kind
        ))
    }
}

/// A fused hybrid result: `kind` is the lexical match kind when matched lexically, else "semantic".
#[pyclass(frozen, get_all, module = "completr")]
struct HybridSuggestion {
    id: Py<PyAny>,
    text: String,
    score: f64,
    kind: &'static str,
    lexical_score: Option<f64>,
    semantic_score: Option<f64>,
    highlights: Vec<(usize, usize)>,
    layer: Option<String>,
}

impl HybridSuggestion {
    fn from(
        py: Python<'_>,
        h: completr_rs::HybridSuggestion,
        layer: Option<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            id: py_id(py, h.id, h.key.as_deref())?,
            highlights: char_ranges(&h.text, &h.highlights),
            text: h.text,
            score: h.score,
            kind: h.kind.as_str(),
            lexical_score: h.lexical_score,
            semantic_score: h.semantic_score,
            layer,
        })
    }
}

#[pymethods]
impl HybridSuggestion {
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "HybridSuggestion(id={}, text={:?}, score={:.4}, kind={:?})",
            self.id.bind(py).repr()?,
            self.text,
            self.score,
            self.kind
        ))
    }
}

/// A document found through one of its synonyms.
#[pyclass(frozen, get_all, module = "completr")]
struct AliasSuggestion {
    id: Py<PyAny>,
    text: String,
    score: f64,
    layer: Option<String>,
}

impl AliasSuggestion {
    fn from(
        py: Python<'_>,
        a: completr_rs::AliasSuggestion,
        layer: Option<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            id: py_id(py, a.id, a.key.as_deref())?,
            text: a.text,
            score: a.score,
            layer,
        })
    }
}

#[pymethods]
impl AliasSuggestion {
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "AliasSuggestion(id={}, text={:?}, score={:.4})",
            self.id.bind(py).repr()?,
            self.text,
            self.score
        ))
    }
}

fn unlayered<T, U>(
    py: Python<'_>,
    items: Vec<T>,
    f: impl Fn(Python<'_>, T, Option<String>) -> PyResult<U>,
) -> PyResult<Vec<U>> {
    items.into_iter().map(|item| f(py, item, None)).collect()
}

/// Segments ordered oldest to newest, completed as one.
#[pyclass(frozen, module = "completr")]
struct Index(Arc<completr_rs::Index>);

#[pymethods]
impl Index {
    #[new]
    #[pyo3(signature = (
        segments, *, max_score = None, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
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
        let options = index_options(
            max_score,
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        Ok(Self(Arc::new(
            py.detach(|| completr_rs::Index::new(segments, options))
                .map_err(to_py_err)?,
        )))
    }

    /// Builds one segment from `documents` (dicts, `Document`s or a table) and completes over it.
    #[staticmethod]
    #[pyo3(signature = (
        documents, vectors = None, *, max_score = None, popularity_weight = 0.4,
        min_word_chars = 3, max_edit_distance = 2, fuzzy_prefix_chars = 7, vector_bits = 4, build_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn from_documents(
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
        vectors: Option<Bound<'_, PyAny>>,
        max_score: Option<f64>,
        popularity_weight: f64,
        min_word_chars: u8,
        max_edit_distance: u8,
        fuzzy_prefix_chars: u8,
        vector_bits: u8,
        build_threads: usize,
    ) -> PyResult<Self> {
        let build = build_options(
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            false,
            build_threads,
        );
        let builder = builder(py, documents, vectors.as_ref(), build)?;
        let options = index_options(max_score, popularity_weight, 3, 100, 10_000, 1);
        let index = py
            .detach(|| {
                let segment = builder.build()?;
                completr_rs::Index::new(vec![Arc::new(segment)], options)
            })
            .map_err(to_py_err)?;
        Ok(Self(Arc::new(index)))
    }

    /// Ranked completions: exact, prefix, abbreviation, infix and typo-tolerant matches.
    #[pyo3(signature = (query, limit = 10, *, contexts = None))]
    fn complete(
        &self,
        py: Python<'_>,
        query: &str,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<Suggestion>> {
        let options = search_options(limit, contexts);
        let hits = py.detach(|| self.0.complete_with(query, &options));
        unlayered(py, hits, Suggestion::from)
    }

    #[pyo3(signature = (query, limit = 10, *, contexts = None))]
    fn complete_aliases(
        &self,
        py: Python<'_>,
        query: &str,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<AliasSuggestion>> {
        let options = search_options(limit, contexts);
        let hits = py.detach(|| self.0.complete_aliases_with(query, &options));
        unlayered(py, hits, AliasSuggestion::from)
    }

    /// Nearest documents by embedding, of kind "semantic", scored by inner product.
    #[pyo3(signature = (vector, limit = 10, *, contexts = None))]
    fn vector_search(
        &self,
        py: Python<'_>,
        vector: &Bound<'_, PyAny>,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<Suggestion>> {
        let query = query_vector(py, vector)?;
        let options = search_options(limit, contexts);
        let hits = py
            .detach(|| self.0.vector_search_with(&query, &options))
            .map_err(to_py_err)?;
        unlayered(py, hits, Suggestion::from)
    }

    /// Lexical and vector results fused by `fusion`: "rrf" (reciprocal rank, `rrf_k`),
    /// "weighted" (`semantic_weight` in [0, 1]) or "lexical_first".
    #[pyo3(signature = (text, vector, limit = 10, *, fusion = "rrf", rrf_k = 60.0, semantic_weight = 0.5, candidates = None, contexts = None))]
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
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<HybridSuggestion>> {
        let query = query_vector(py, vector)?;
        let options = hybrid_options(fusion, rrf_k, semantic_weight, candidates, contexts)?;
        let hits = py
            .detach(|| self.0.hybrid_search(text, &query, limit, &options))
            .map_err(to_py_err)?;
        unlayered(py, hits, HybridSuggestion::from)
    }

    /// The live document with this id or key.
    fn get(&self, id: Id) -> Option<Document> {
        let doc = match &id {
            Id::Int(id) => self.0.document(*id),
            Id::Key(key) => self.0.document_by_key(key),
        };
        doc.map(Document)
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
    fn vector_dim(&self) -> Option<usize> {
        self.0.vector_dim()
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
            "Index(documents={}, segments={})",
            self.0.len(),
            self.0.segments().len()
        )
    }
}

/// Named indexes, published atomically and completed over as layers. An engine from
/// `Database.engine()` also follows the database: call `sync()` to load new versions.
#[pyclass(frozen, module = "completr")]
struct Engine {
    engine: Arc<completr_rs::Engine>,
    replica: Option<completr_rs::Replica>,
}

fn layered<T, U>(
    py: Python<'_>,
    layers: &[String],
    hits: Vec<completr_rs::LayeredSuggestion<T>>,
    f: impl Fn(Python<'_>, T, Option<String>) -> PyResult<U>,
) -> PyResult<Vec<U>> {
    hits.into_iter()
        .map(|h| f(py, h.suggestion, Some(layers[h.layer].clone())))
        .collect()
}

#[pymethods]
impl Engine {
    /// With several layers, each is asked for `limit * overfetch` suggestions before merging.
    #[new]
    #[pyo3(signature = (overfetch = 2))]
    fn new(overfetch: usize) -> Self {
        Self {
            engine: Arc::new(completr_rs::Engine::new().with_overfetch(overfetch)),
            replica: None,
        }
    }

    /// Replaces or, with `None`, removes the given indexes in one step.
    fn publish(&self, updates: &Bound<'_, PyDict>) -> PyResult<()> {
        let mut staged = Vec::with_capacity(updates.len());
        for (name, index) in updates.iter() {
            let index: Option<Py<Index>> = index.extract()?;
            staged.push((name.extract::<String>()?, index.map(|i| i.get().0.clone())));
        }
        self.engine.publish(staged);
        Ok(())
    }

    /// Loads and publishes the database's latest version; returns it if new, else `None`.
    fn sync(&self, py: Python<'_>) -> PyResult<Option<u64>> {
        let replica = self.replica.as_ref().ok_or_else(|| {
            invalid("this engine does not follow a database; create it with Database.engine()")
        })?;
        py.detach(|| completr_rs::block_on(replica.sync(&self.engine)))
            .map_err(to_py_err)
    }

    /// The database version served, for an engine from `Database.engine()`.
    #[getter]
    fn version(&self, py: Python<'_>) -> Option<u64> {
        let replica = self.replica.as_ref()?;
        Some(py.detach(|| completr_rs::block_on(replica.version())))
    }

    fn get(&self, name: &str) -> Option<Index> {
        self.engine.get(name).map(Index)
    }

    fn names(&self) -> Vec<String> {
        self.engine.names()
    }

    /// Later layers override earlier ones per document id; missing names are empty layers.
    #[pyo3(signature = (query, layers, limit = 10, *, contexts = None))]
    fn complete(
        &self,
        py: Python<'_>,
        query: &str,
        layers: Vec<String>,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<Suggestion>> {
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let options = search_options(limit, contexts);
        let hits = py.detach(|| self.engine.complete_with(&names, query, &options));
        layered(py, &layers, hits, Suggestion::from)
    }

    #[pyo3(signature = (query, layers, limit = 10, *, contexts = None))]
    fn complete_aliases(
        &self,
        py: Python<'_>,
        query: &str,
        layers: Vec<String>,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<AliasSuggestion>> {
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let options = search_options(limit, contexts);
        let hits = py.detach(|| self.engine.complete_aliases_with(&names, query, &options));
        layered(py, &layers, hits, AliasSuggestion::from)
    }

    #[pyo3(signature = (vector, layers, limit = 10, *, contexts = None))]
    fn vector_search(
        &self,
        py: Python<'_>,
        vector: &Bound<'_, PyAny>,
        layers: Vec<String>,
        limit: usize,
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<Suggestion>> {
        let query = query_vector(py, vector)?;
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let options = search_options(limit, contexts);
        let hits = py
            .detach(|| self.engine.vector_search_with(&names, &query, &options))
            .map_err(to_py_err)?;
        layered(py, &layers, hits, Suggestion::from)
    }

    #[pyo3(signature = (text, vector, layers, limit = 10, *, fusion = "rrf", rrf_k = 60.0, semantic_weight = 0.5, candidates = None, contexts = None))]
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
        contexts: Option<Vec<String>>,
    ) -> PyResult<Vec<HybridSuggestion>> {
        let query = query_vector(py, vector)?;
        let options = hybrid_options(fusion, rrf_k, semantic_weight, candidates, contexts)?;
        let names: Vec<&str> = layers.iter().map(String::as_str).collect();
        let hits = py
            .detach(|| {
                self.engine
                    .hybrid_search(&names, text, &query, limit, &options)
            })
            .map_err(to_py_err)?;
        hits.into_iter()
            .map(|h| {
                let layer = Some(layers[h.layer].clone());
                HybridSuggestion::from(py, h, layer)
            })
            .collect()
    }
}

/// A local directory or `s3://`, `gs://`, `az://`, `file://`, `memory://` URL; keys are relative to it.
#[pyclass(frozen, module = "completr")]
struct Store(completr_rs::BlockingStore);

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
            .detach(|| completr_rs::BlockingStore::open_with(url, options))
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
    manifest: &completr_rs::Manifest,
) -> PyResult<Bound<'py, PyAny>> {
    py.import("json")?
        .call_method1("loads", (manifest.to_json(),))
}

/// Named indexes versioned in a bucket or directory: the database is the storage itself.
#[pyclass(frozen, module = "completr")]
struct Database(completr_rs::Database);

impl Database {
    async fn read(
        &self,
        version: Option<u64>,
    ) -> Result<completr_rs::Manifest, completr_rs::Error> {
        match version {
            Some(version) => self.0.manifest(version).await,
            None => self.0.latest().await,
        }
    }
}

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
            .detach(|| completr_rs::block_on(completr_rs::Database::open(url, options)))
            .map_err(to_py_err)?;
        let database = match cache_dir {
            Some(dir) => database.with_cache_dir(dir).map_err(to_py_err)?,
            None => database,
        };
        let build = build_options(
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            compact_keys,
            build_threads,
        );
        Ok(Self(database.with_build_options(build)))
    }

    fn versions(&self, py: Python<'_>) -> PyResult<Vec<u64>> {
        py.detach(|| completr_rs::block_on(self.0.versions()))
            .map_err(to_py_err)
    }

    fn latest_version(&self, py: Python<'_>) -> PyResult<u64> {
        py.detach(|| completr_rs::block_on(self.0.latest_version()))
            .map_err(to_py_err)
    }

    /// Names of the indexes in `version` (latest by default).
    #[pyo3(signature = (version = None))]
    fn index_names(&self, py: Python<'_>, version: Option<u64>) -> PyResult<Vec<String>> {
        let manifest = py
            .detach(|| completr_rs::block_on(self.read(version)))
            .map_err(to_py_err)?;
        let mut names: Vec<String> = manifest.indexes.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    /// The manifest of `version` (latest by default) as a dict.
    #[pyo3(signature = (version = None))]
    fn manifest<'py>(&self, py: Python<'py>, version: Option<u64>) -> PyResult<Bound<'py, PyAny>> {
        let manifest = py
            .detach(|| completr_rs::block_on(self.read(version)))
            .map_err(to_py_err)?;
        manifest_dict(py, &manifest)
    }

    /// A transaction against `version` (latest by default).
    #[pyo3(signature = (version = None))]
    fn begin(&self, py: Python<'_>, version: Option<u64>) -> PyResult<Transaction> {
        let manifest = py
            .detach(|| completr_rs::block_on(self.read(version)))
            .map_err(to_py_err)?;
        Ok(Transaction {
            txn: Mutex::new(Some(self.0.transaction(manifest))),
            options: self.0.build_options(),
        })
    }

    /// A snapshot of index `name` at `version` (latest by default).
    #[pyo3(signature = (
        name, version = None, *, max_score = None, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
        short_query_cache_entries = 10_000, vector_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn open_index(
        &self,
        py: Python<'_>,
        name: &str,
        version: Option<u64>,
        max_score: Option<f64>,
        popularity_weight: f64,
        short_query_chars: usize,
        short_query_limit: usize,
        short_query_cache_entries: usize,
        vector_threads: usize,
    ) -> PyResult<Index> {
        let options = index_options(
            max_score,
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        let index = py.detach(|| {
            completr_rs::block_on(async {
                let manifest = self.read(version).await?;
                self.0.open_index(&manifest, name, options).await
            })
        });
        Ok(Index(Arc::new(index.map_err(to_py_err)?)))
    }

    /// An engine serving every index of this database, loaded now; `engine.sync()` loads newer
    /// versions. With `group_separator`, indexes switch group by group to bound memory.
    #[pyo3(signature = (
        *, group_separator = None, overfetch = 2, popularity_weight = 0.4, short_query_chars = 3,
        short_query_limit = 100, short_query_cache_entries = 10_000, vector_threads = 1,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn engine(
        &self,
        py: Python<'_>,
        group_separator: Option<&str>,
        overfetch: usize,
        popularity_weight: f64,
        short_query_chars: usize,
        short_query_limit: usize,
        short_query_cache_entries: usize,
        vector_threads: usize,
    ) -> PyResult<Engine> {
        let options = index_options(
            None,
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        let replica = completr_rs::Replica::new(self.0.clone(), options);
        let replica = match group_separator {
            Some(separator) => replica.with_groups_by_suffix(separator),
            None => replica,
        };
        let engine = Engine {
            engine: Arc::new(completr_rs::Engine::new().with_overfetch(overfetch)),
            replica: Some(replica),
        };
        engine.sync(py)?;
        Ok(engine)
    }

    /// One compaction step, or with `until_done` as many as are due; returns the new version,
    /// or `None` if nothing was due.
    #[pyo3(signature = (index, *, fanout = 4, max_segments = 16, max_hidden_fraction = 0.25, until_done = false))]
    fn compact(
        &self,
        py: Python<'_>,
        index: &str,
        fanout: usize,
        max_segments: usize,
        max_hidden_fraction: f64,
        until_done: bool,
    ) -> PyResult<Option<u64>> {
        let policy = completr_rs::CompactionPolicy::default()
            .fanout(fanout)
            .max_segments(max_segments)
            .max_hidden_fraction(max_hidden_fraction);
        let manifest = py
            .detach(|| {
                completr_rs::block_on(async {
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
    #[pyo3(signature = (*, keep_versions = 10, older_than_seconds = 3600.0))]
    fn cleanup<'py>(
        &self,
        py: Python<'py>,
        keep_versions: usize,
        older_than_seconds: f64,
    ) -> PyResult<Bound<'py, PyDict>> {
        let policy = completr_rs::CleanupPolicy::default()
            .keep_versions(keep_versions)
            .older_than(Duration::from_secs_f64(older_than_seconds));
        let stats = py
            .detach(|| completr_rs::block_on(self.0.cleanup(&policy)))
            .map_err(to_py_err)?;
        let out = PyDict::new(py);
        out.set_item("versions_removed", stats.versions_removed)?;
        out.set_item("segments_removed", stats.segments_removed)?;
        out.set_item("bytes_removed", stats.bytes_removed)?;
        Ok(out)
    }

    /// Queues `changes` for the database's ingestor; returns the change set's id.
    fn submit(&self, py: Python<'_>, changes: &Bound<'_, ChangeSet>) -> PyResult<String> {
        let inner = changes
            .borrow_mut()
            .0
            .take()
            .ok_or_else(|| invalid("change set already submitted"))?;
        py.detach(|| completr_rs::block_on(self.0.submit(inner)))
            .map_err(to_py_err)
    }

    fn pending_change_sets(&self, py: Python<'_>) -> PyResult<usize> {
        py.detach(|| completr_rs::block_on(self.0.pending_change_sets()))
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
            .detach(|| completr_rs::block_on(self.0.acquire_lease(name, owner, ttl)))
            .map_err(to_py_err)?;
        Ok(lease.map(|l| Lease(Mutex::new(Some(l)))))
    }
}

/// Staged changes, committed together by `commit()`; usable once.
#[pyclass(frozen, module = "completr")]
struct Transaction {
    txn: Mutex<Option<completr_rs::Transaction>>,
    options: completr_rs::BuildOptions,
}

impl Transaction {
    fn with<T>(&self, f: impl FnOnce(&mut completr_rs::Transaction) -> PyResult<T>) -> PyResult<T> {
        let mut guard = self.txn.lock().unwrap_or_else(|e| e.into_inner());
        let txn = guard
            .as_mut()
            .ok_or_else(|| invalid("transaction already committed"))?;
        f(txn)
    }

    fn segment(
        &self,
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
        deletes: Vec<u64>,
        vectors: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<completr_rs::Segment> {
        let docs = self::documents(py, documents, vectors)?;
        let options = self.options;
        py.detach(|| completr_rs::Segment::build_with(options, docs, deletes))
            .map_err(to_py_err)
    }
}

#[pymethods]
impl Transaction {
    #[getter]
    fn read_version(&self) -> PyResult<u64> {
        self.with(|t| Ok(t.read_version()))
    }

    /// Adds or replaces `documents` and deletes `deletes` (ids or keys) in `index`.
    #[pyo3(signature = (index, documents, deletes = Vec::new(), vectors = None))]
    fn append(
        &self,
        py: Python<'_>,
        index: &str,
        documents: &Bound<'_, PyAny>,
        deletes: Vec<Id>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let segment = self.segment(py, documents, numeric_ids(deletes), vectors.as_ref())?;
        self.with(|t| {
            t.append(index, segment);
            Ok(())
        })
    }

    /// Replaces the whole of `index` with `documents`.
    #[pyo3(signature = (index, documents, vectors = None))]
    fn overwrite(
        &self,
        py: Python<'_>,
        index: &str,
        documents: &Bound<'_, PyAny>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let segment = self.segment(py, documents, Vec::new(), vectors.as_ref())?;
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
        let txn = txn.ok_or_else(|| invalid("transaction already committed"))?;
        let manifest = py
            .detach(|| completr_rs::block_on(txn.commit()))
            .map_err(to_py_err)?;
        manifest_dict(py, &manifest)
    }
}

#[pyclass(frozen, module = "completr")]
struct Lease(Mutex<Option<completr_rs::Lease>>);

#[pymethods]
impl Lease {
    /// Fencing token; grows with every renewal.
    #[getter]
    fn generation(&self) -> PyResult<u64> {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|l| l.generation())
            .ok_or_else(|| invalid("lease released"))
    }

    /// Extends the lease; `False` means it was lost.
    fn renew(&self, py: Python<'_>, ttl_seconds: f64) -> PyResult<bool> {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let lease = guard.as_mut().ok_or_else(|| invalid("lease released"))?;
        py.detach(|| completr_rs::block_on(lease.renew(Duration::from_secs_f64(ttl_seconds))))
            .map_err(to_py_err)
    }

    fn release(&self, py: Python<'_>) -> PyResult<()> {
        let lease = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        match lease {
            Some(lease) => py
                .detach(|| completr_rs::block_on(lease.release()))
                .map_err(to_py_err),
            None => Ok(()),
        }
    }
}

/// Changes to one or more indexes, submitted together with `Database.submit`.
#[pyclass(module = "completr")]
struct ChangeSet(Option<completr_rs::ChangeSet>);

impl ChangeSet {
    fn inner(&mut self) -> PyResult<&mut completr_rs::ChangeSet> {
        self.0
            .as_mut()
            .ok_or_else(|| invalid("change set already submitted"))
    }
}

#[pymethods]
impl ChangeSet {
    #[new]
    fn new() -> Self {
        Self(Some(completr_rs::ChangeSet::new()))
    }

    /// Adds or replaces `documents` in `index`.
    #[pyo3(signature = (index, documents, vectors = None))]
    fn upsert(
        &mut self,
        py: Python<'_>,
        index: &str,
        documents: &Bound<'_, PyAny>,
        vectors: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let docs = self::documents(py, documents, vectors.as_ref())?;
        self.inner()?.upsert(index, docs);
        Ok(())
    }

    /// Deletes documents by id or key.
    fn delete(&mut self, index: &str, ids: Vec<Id>) -> PyResult<()> {
        self.inner()?.delete(index, numeric_ids(ids));
        Ok(())
    }
}

/// Commits submitted change sets while it holds the ingestor lease; run one per process.
#[pyclass(module = "completr")]
struct Ingestor(Option<completr_rs::Ingestor>);

#[pymethods]
impl Ingestor {
    /// With `compact`, the ingestor compacts the indexes it committed to after each round.
    #[new]
    #[pyo3(signature = (database, owner, *, lease_ttl_seconds = 30.0, max_change_sets = 1000, compact = true))]
    fn new(
        database: &Database,
        owner: &str,
        lease_ttl_seconds: f64,
        max_change_sets: usize,
        compact: bool,
    ) -> Self {
        let mut ingestor = completr_rs::Ingestor::new(database.0.clone(), owner);
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
            .ok_or_else(|| invalid("ingestor released"))?;
        let step = py
            .detach(|| completr_rs::block_on(ingestor.run_once()))
            .map_err(to_py_err)?;
        let out = PyDict::new(py);
        match step {
            completr_rs::IngestStep::Standby => out.set_item("step", "standby")?,
            completr_rs::IngestStep::Idle => out.set_item("step", "idle")?,
            completr_rs::IngestStep::Committed {
                version,
                change_sets,
                documents,
            } => {
                out.set_item("step", "committed")?;
                out.set_item("version", version)?;
                out.set_item("change_sets", change_sets)?;
                out.set_item("documents", documents)?;
            }
            _ => out.set_item("step", "unknown")?,
        }
        Ok(out)
    }

    #[getter]
    fn is_active(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(completr_rs::Ingestor::is_active)
    }

    /// Gives up the lease so another process takes over at once.
    fn release(&mut self, py: Python<'_>) -> PyResult<()> {
        match self.0.take() {
            Some(ingestor) => py
                .detach(|| completr_rs::block_on(ingestor.release()))
                .map_err(to_py_err),
            None => Ok(()),
        }
    }
}

/// Keeps an existing `Engine` on a database's latest version, loading only what changed.
/// `Database.engine()` is simpler when the engine serves one database.
#[pyclass(frozen, module = "completr")]
struct Replica {
    replica: completr_rs::Replica,
    engine: Arc<completr_rs::Engine>,
}

#[pymethods]
impl Replica {
    #[new]
    #[pyo3(signature = (
        database, engine, *, popularity_weight = 0.4, short_query_chars = 3, short_query_limit = 100,
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
        let options = index_options(
            None,
            popularity_weight,
            short_query_chars,
            short_query_limit,
            short_query_cache_entries,
            vector_threads,
        );
        let replica = completr_rs::Replica::new(database.0.clone(), options);
        let replica = match group_separator {
            Some(separator) => replica.with_groups_by_suffix(separator),
            None => replica,
        };
        Self {
            replica,
            engine: engine.engine.clone(),
        }
    }

    /// Publishes the latest version; returns it if it is new, else `None`.
    fn sync(&self, py: Python<'_>) -> PyResult<Option<u64>> {
        py.detach(|| completr_rs::block_on(self.replica.sync(&self.engine)))
            .map_err(to_py_err)
    }

    #[getter]
    fn version(&self, py: Python<'_>) -> u64 {
        py.detach(|| completr_rs::block_on(self.replica.version()))
    }
}

/// The numeric id completr derives from a string key.
#[pyfunction]
fn key_id(key: &str) -> u64 {
    completr_rs::key_id(key)
}

#[pymodule]
fn completr(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Engine events go to Python's `logging`, under loggers named `completr.*`.
    pyo3_log::init();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<Document>()?;
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
    m.add_class::<Suggestion>()?;
    m.add_class::<AliasSuggestion>()?;
    m.add_class::<HybridSuggestion>()?;
    m.add_function(wrap_pyfunction!(key_id, m)?)?;
    Ok(())
}
