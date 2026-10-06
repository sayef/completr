//! One entry point: a [`Client`] on a database, and named [`Collection`]s in it.
//!
//! Reads follow the database by themselves, writes commit straight to storage, and collections compact
//! themselves as their settings say. The building blocks underneath stay public for finer control.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;
use serde_json::{json, Value};

use crate::database::{now_ms, validate_name, CompactionPolicy, Database, Manifest, Replica};
use crate::engine::{layered_complete, layered_complete_aliases};
use crate::hybrid::HybridOptions;
use crate::search::{MatchKind, SearchOptions, Suggestion};
use crate::segment::BuildOptions;
use crate::store::block_on;
use crate::{Document, Engine, Error, Index, IndexOptions, DEFAULT_NAMESPACE};

/// How a client connects and follows its database.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ConnectOptions {
    /// How often reads pick up new versions in the background; `None` syncs only on [`Client::sync`].
    pub sync_every: Option<Duration>,
    /// Caches downloaded segments here and memory-maps them.
    pub cache_dir: Option<PathBuf>,
    /// Storage options, such as credentials or a region, passed to the object store.
    pub storage: Vec<(String, String)>,
    pub build: BuildOptions,
    pub index: IndexOptions,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            sync_every: Some(Duration::from_secs(5)),
            cache_dir: None,
            storage: Vec::new(),
            build: BuildOptions::default(),
            index: IndexOptions::default(),
        }
    }
}

crate::setters!(ConnectOptions {
    sync_every: Option<Duration>,
    cache_dir: Option<PathBuf>,
    storage: Vec<(String, String)>,
    build: BuildOptions,
    index: IndexOptions,
});

/// When a collection compacts its segments.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Optimize {
    /// After a write, once `policy` finds a merge due, at most once every `min_interval`.
    Auto {
        policy: CompactionPolicy,
        min_interval: Duration,
    },
    /// Only on [`Collection::optimize`].
    Off,
}

impl Default for Optimize {
    fn default() -> Self {
        Self::Auto {
            policy: CompactionPolicy::default(),
            min_interval: Duration::from_secs(30),
        }
    }
}

impl Optimize {
    fn to_json(&self) -> Value {
        match self {
            Self::Off => json!({"optimize": "off"}),
            Self::Auto {
                policy,
                min_interval,
            } => json!({
                "optimize": "auto",
                "fanout": policy.fanout,
                "max_segments": policy.max_segments,
                "max_hidden_fraction": policy.max_hidden_fraction,
                "min_interval_s": min_interval.as_secs_f64(),
            }),
        }
    }

    fn from_json(value: &Value) -> Self {
        if value["optimize"] == "off" {
            return Self::Off;
        }
        let default = CompactionPolicy::default();
        let number = |key: &str| value[key].as_f64();
        Self::Auto {
            policy: default
                .clone()
                .fanout(number("fanout").map_or(default.fanout, |v| v as usize))
                .max_segments(number("max_segments").map_or(default.max_segments, |v| v as usize))
                .max_hidden_fraction(
                    number("max_hidden_fraction").unwrap_or(default.max_hidden_fraction),
                ),
            min_interval: Duration::from_secs_f64(
                number("min_interval_s").unwrap_or(30.0).max(0.0),
            ),
        }
    }
}

/// What a query asks for, beyond its text.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Query {
    pub limit: usize,
    /// Also match synonyms, ranked below direct matches.
    pub aliases: bool,
    /// Collections searched on top of this one, later ones overriding earlier ones per document id.
    pub layers: Vec<String>,
    /// Only documents tagged with any of these contexts; empty means all.
    pub contexts: Vec<String>,
    /// Search a layer that does not exist as empty instead of failing.
    pub ignore_missing_layers: bool,
    /// An embedding of the query, for results ranked by meaning as well as by text.
    pub vector: Option<Vec<f32>>,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            limit: 10,
            aliases: false,
            layers: Vec::new(),
            contexts: Vec::new(),
            ignore_missing_layers: false,
            vector: None,
        }
    }
}

crate::setters!(Query {
    limit: usize,
    aliases: bool,
    layers: Vec<String>,
    contexts: Vec<String>,
    ignore_missing_layers: bool,
    vector: Option<Vec<f32>>,
});

/// One completion, from whichever collection of the query's layers decided it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Completion {
    pub id: u64,
    pub key: Option<String>,
    pub text: String,
    pub score: f64,
    pub kind: MatchKind,
    /// Byte ranges of `text` that matched the query.
    pub highlights: Vec<std::ops::Range<usize>>,
    /// The collection the completion came from.
    pub collection: String,
}

/// A collection's size and freshness.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CollectionStats {
    /// Live documents in the version this client serves.
    pub documents: usize,
    pub segments: usize,
    /// Size of its segment files.
    pub bytes: u64,
    /// The latest committed version, and when it was committed.
    pub version: u64,
    pub committed_at_ms: u64,
    /// The version this client serves, and when it last checked for a newer one.
    pub synced_version: u64,
    pub synced_at_ms: Option<u64>,
    pub sync_error: Option<String>,
    pub optimize_error: Option<String>,
}

/// A database and the collections in it.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    database: Database,
    replica: Replica,
    engine: Engine,
    sync_every: Option<Duration>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The process whose background thread follows the database; a fork starts its own.
    follower: Option<u32>,
    synced_at_ms: Option<u64>,
    sync_error: Option<String>,
    optimizing: FxHashMap<(String, String), Instant>,
    optimize_errors: FxHashMap<(String, String), String>,
}

/// Index metadata key of a collection's settings.
const SETTINGS: &str = "collection";
/// Database metadata prefix of settings written before namespaces, for the default namespace.
const LEGACY_SETTINGS: &str = "collection/";

/// Opens the database at `url`: a local directory, `memory://`, `s3://`, `gs://` or `az://`.
pub async fn connect(url: &str, options: ConnectOptions) -> Result<Client, Error> {
    let mut database = Database::open(url, options.storage)
        .await?
        .with_build_options(options.build);
    if let Some(dir) = &options.cache_dir {
        database = database.with_cache_dir(dir)?;
    }
    let inner = Inner {
        replica: Replica::new(database.clone(), options.index),
        database,
        engine: Engine::new(),
        sync_every: options.sync_every,
        state: Mutex::default(),
    };
    let client = Client {
        inner: Arc::new(inner),
    };
    client.sync().await?;
    Ok(client)
}

impl Inner {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn sync(&self) -> Result<Option<u64>, Error> {
        let result = self.replica.sync(&self.engine).await;
        let mut state = self.state();
        state.synced_at_ms = Some(now_ms());
        state.sync_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    /// Starts this process's background sync, once.
    fn follow(self: &Arc<Self>) {
        let Some(every) = self.sync_every else {
            return;
        };
        let pid = std::process::id();
        {
            let mut state = self.state();
            if state.follower == Some(pid) {
                return;
            }
            state.follower = Some(pid);
        }
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("completr-sync".into())
            .spawn(move || loop {
                std::thread::sleep(every);
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                // A failed sync keeps serving the version it has; the error shows in `stats`.
                let _ = block_on(inner.sync());
            })
            .ok();
    }

    async fn settings(&self, namespace: &str, name: &str) -> Result<Option<Optimize>, Error> {
        Ok(settings(&self.database.latest().await?, namespace, name))
    }
}

/// A collection's settings, if it exists: an index, or settings written before its first document.
fn settings(manifest: &Manifest, namespace: &str, name: &str) -> Option<Optimize> {
    let parse = |raw: &str| Optimize::from_json(&serde_json::from_str(raw).unwrap_or(Value::Null));
    if let Some(entry) = manifest.index(namespace, name) {
        return Some(
            entry
                .metadata
                .get(SETTINGS)
                .map_or_else(Optimize::default, |raw| parse(raw)),
        );
    }
    (namespace == DEFAULT_NAMESPACE)
        .then(|| manifest.metadata.get(&format!("{LEGACY_SETTINGS}{name}")))
        .flatten()
        .map(|raw| parse(raw))
}

impl Client {
    /// Loads the latest version now; returns it if it is new.
    pub async fn sync(&self) -> Result<Option<u64>, Error> {
        self.inner.sync().await
    }

    /// The database underneath, for transactions, leases and other lower-level work.
    pub fn database(&self) -> &Database {
        &self.inner.database
    }

    /// Collections of namespace `name`. Does no I/O.
    pub fn namespace(&self, name: &str) -> Result<ClientNamespace, Error> {
        validate_name("namespace", name)?;
        Ok(ClientNamespace {
            inner: self.inner.clone(),
            name: name.to_owned(),
        })
    }

    fn default_namespace(&self) -> ClientNamespace {
        ClientNamespace {
            inner: self.inner.clone(),
            name: DEFAULT_NAMESPACE.to_owned(),
        }
    }

    /// Names of the collections in the default namespace, sorted.
    pub async fn collections(&self) -> Result<Vec<String>, Error> {
        self.default_namespace().collections().await
    }

    /// An existing collection of the default namespace.
    pub async fn collection(&self, name: &str) -> Result<Collection, Error> {
        self.default_namespace().collection(name).await
    }

    /// A new collection in the default namespace; an error if one of that name exists.
    pub async fn create_collection(
        &self,
        name: &str,
        optimize: Optimize,
    ) -> Result<Collection, Error> {
        self.default_namespace()
            .create_collection(name, optimize)
            .await
    }

    /// The collection of that name, created with `optimize` if it does not exist yet.
    pub async fn get_or_create_collection(
        &self,
        name: &str,
        optimize: Optimize,
    ) -> Result<Collection, Error> {
        self.default_namespace()
            .get_or_create_collection(name, optimize)
            .await
    }

    /// Deletes a collection of the default namespace and its settings.
    pub async fn drop_collection(&self, name: &str) -> Result<(), Error> {
        self.default_namespace().drop_collection(name).await
    }
}

/// The collections of one namespace.
#[derive(Clone)]
pub struct ClientNamespace {
    inner: Arc<Inner>,
    name: String,
}

impl ClientNamespace {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Names of the collections, sorted.
    pub async fn collections(&self) -> Result<Vec<String>, Error> {
        let manifest = self.inner.database.latest().await?;
        let mut names = manifest.index_names(&self.name);
        if self.name == DEFAULT_NAMESPACE {
            names.extend(
                manifest
                    .metadata
                    .keys()
                    .filter_map(|k| k.strip_prefix(LEGACY_SETTINGS))
                    .map(str::to_owned),
            );
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// An existing collection.
    pub async fn collection(&self, name: &str) -> Result<Collection, Error> {
        let manifest = self.inner.database.latest().await?;
        match settings(&manifest, &self.name, name) {
            Some(optimize) => Ok(self.handle(name, optimize)),
            None => Err(Error::LayerNotFound {
                namespace: self.name.clone(),
                version: manifest.version,
                name: name.to_owned(),
                available: manifest.index_names(&self.name),
            }),
        }
    }

    /// A new collection; an error if one of that name exists.
    pub async fn create_collection(
        &self,
        name: &str,
        optimize: Optimize,
    ) -> Result<Collection, Error> {
        if self.inner.settings(&self.name, name).await?.is_some() {
            return Err(Error::input(format!(
                "collection {name} already exists in namespace {}",
                self.name
            )));
        }
        self.save_settings(name, &optimize).await?;
        Ok(self.handle(name, optimize))
    }

    /// The collection of that name, created with `optimize` if it does not exist yet.
    pub async fn get_or_create_collection(
        &self,
        name: &str,
        optimize: Optimize,
    ) -> Result<Collection, Error> {
        match self.inner.settings(&self.name, name).await? {
            Some(existing) => Ok(self.handle(name, existing)),
            None => {
                self.save_settings(name, &optimize).await?;
                Ok(self.handle(name, optimize))
            }
        }
    }

    /// Deletes a collection and its settings.
    pub async fn drop_collection(&self, name: &str) -> Result<(), Error> {
        let mut txn = self.inner.database.namespace(&self.name)?.begin().await?;
        txn.drop_index(name);
        if self.name == DEFAULT_NAMESPACE {
            txn.set_metadata(&format!("{LEGACY_SETTINGS}{name}"), None);
        }
        txn.commit().await?;
        self.inner.sync().await?;
        Ok(())
    }

    async fn save_settings(&self, name: &str, optimize: &Optimize) -> Result<(), Error> {
        let mut txn = self.inner.database.namespace(&self.name)?.begin().await?;
        txn.set_index_metadata(name, SETTINGS, Some(&optimize.to_json().to_string()));
        txn.commit().await?;
        self.inner.sync().await?;
        Ok(())
    }

    fn handle(&self, name: &str, optimize: Optimize) -> Collection {
        Collection {
            inner: self.inner.clone(),
            namespace: self.name.clone(),
            name: name.to_owned(),
            optimize,
        }
    }
}

/// Named documents to complete: add, delete and query them.
#[derive(Clone)]
pub struct Collection {
    inner: Arc<Inner>,
    namespace: String,
    name: String,
    optimize: Optimize,
}

impl Collection {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    fn key(&self) -> (String, String) {
        (self.namespace.clone(), self.name.clone())
    }

    pub fn optimize_setting(&self) -> &Optimize {
        &self.optimize
    }

    /// Adds documents, replacing any with the same id; durable on return, and visible to this client.
    pub async fn add(&self, documents: impl IntoIterator<Item = Document>) -> Result<u64, Error> {
        self.write(documents, std::iter::empty()).await
    }

    /// Deletes documents by id; ids that are not present are ignored.
    pub async fn delete(&self, ids: impl IntoIterator<Item = u64>) -> Result<u64, Error> {
        self.write(std::iter::empty(), ids).await
    }

    async fn write(
        &self,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
    ) -> Result<u64, Error> {
        let mut txn = self
            .inner
            .database
            .namespace(&self.namespace)?
            .begin()
            .await?;
        txn.append_documents(&self.name, documents, deletes)?;
        let version = txn.commit().await?.version;
        self.inner.sync().await?;
        self.optimize_soon();
        Ok(version)
    }

    /// Compacts in the background when the setting says one is due.
    fn optimize_soon(&self) {
        let Optimize::Auto {
            policy,
            min_interval,
        } = &self.optimize
        else {
            return;
        };
        {
            let mut state = self.inner.state();
            if state
                .optimizing
                .get(&self.key())
                .is_some_and(|at| at.elapsed() < *min_interval)
            {
                return;
            }
            state.optimizing.insert(self.key(), Instant::now());
        }
        let (inner, key, policy) = (self.inner.clone(), self.key(), policy.clone());
        std::thread::Builder::new()
            .name("completr-optimize".into())
            .spawn(move || {
                let result = block_on(async {
                    let namespace = inner.database.namespace(&key.0)?;
                    let compacted = namespace.compact_all(&key.1, &policy).await?;
                    if compacted.is_some() {
                        inner.sync().await?;
                    }
                    Ok::<_, Error>(())
                });
                let mut state = inner.state();
                match result {
                    Ok(()) => state.optimize_errors.remove(&key),
                    Err(e) => state.optimize_errors.insert(key, e.to_string()),
                };
            })
            .ok();
    }

    /// Compacts now, whatever the setting; returns the new version if anything was merged.
    pub async fn optimize(&self) -> Result<Option<u64>, Error> {
        let policy = match &self.optimize {
            Optimize::Auto { policy, .. } => policy.clone(),
            Optimize::Off => CompactionPolicy::default(),
        };
        let namespace = self.inner.database.namespace(&self.namespace)?;
        let compacted = namespace.compact_all(&self.name, &policy).await?;
        if compacted.is_some() {
            self.inner.sync().await?;
        }
        Ok(compacted.map(|m| m.version))
    }

    /// Completions for `text`, best first.
    pub fn complete(&self, text: &str, query: &Query) -> Result<Vec<Completion>, Error> {
        self.inner.follow();
        let names: Vec<&str> = std::iter::once(self.name.as_str())
            .chain(query.layers.iter().map(String::as_str))
            .collect();
        let name = |layer: usize| names.get(layer).copied().unwrap_or_default().to_owned();
        let namespace = self.inner.engine.namespace(&self.namespace).ok();
        let held: Vec<Option<Arc<Index>>> = names
            .iter()
            .map(|n| namespace.as_ref().and_then(|ns| ns.get(n)))
            .collect();
        // The collection itself is empty until its first write is synced; other layers must exist.
        if !query.ignore_missing_layers {
            if let Some(missing) = (1..names.len()).find(|&i| held[i].is_none()) {
                return Err(Error::LayerNotFound {
                    namespace: self.namespace.clone(),
                    version: namespace.as_ref().map_or(0, |n| n.version()),
                    name: names[missing].to_owned(),
                    available: namespace.as_ref().map(|n| n.names()).unwrap_or_default(),
                });
            }
        }
        let layers: Vec<Option<&Index>> = held.iter().map(|i| i.as_deref()).collect();
        let search = SearchOptions::new(query.limit).contexts(query.contexts.iter().cloned());
        let overfetch = self.inner.engine.overfetch();
        let mut out: Vec<Completion> = match &query.vector {
            Some(vector) => {
                let options = HybridOptions::default().contexts(query.contexts.iter().cloned());
                crate::engine::layered_hybrid_search(
                    &layers,
                    text,
                    vector,
                    query.limit,
                    &options,
                    overfetch,
                )?
                .into_iter()
                .map(|h| Completion {
                    id: h.id,
                    key: h.key,
                    text: h.text,
                    score: h.score,
                    kind: h.kind,
                    highlights: h.highlights,
                    collection: name(h.layer),
                })
                .collect()
            }
            None => layered_complete(&layers, text, &search, overfetch)
                .into_iter()
                .map(|h| completion(h.suggestion, name(h.layer)))
                .collect(),
        };
        if query.aliases && out.len() < query.limit {
            let floor = out.iter().map(|c| c.score).fold(f64::INFINITY, f64::min);
            let aliases = layered_complete_aliases(&layers, text, &search, overfetch);
            let top = aliases.first().map_or(0.0, |a| a.suggestion.score);
            // Synonyms rank below every direct match, keeping their order, as typo corrections do.
            let scale = if floor.is_finite() && top > 0.0 {
                (floor * 0.95 / top).min(1.0)
            } else {
                1.0
            };
            for a in aliases {
                if out.len() >= query.limit {
                    break;
                }
                if out.iter().any(|c| c.id == a.suggestion.id) {
                    continue;
                }
                out.push(Completion {
                    id: a.suggestion.id,
                    key: a.suggestion.key,
                    text: a.suggestion.text,
                    score: a.suggestion.score * scale,
                    kind: MatchKind::Synonym,
                    highlights: Vec::new(),
                    collection: name(a.layer),
                });
            }
        }
        Ok(out)
    }

    /// Size and freshness, from the latest version and this client's view of it.
    pub async fn stats(&self) -> Result<CollectionStats, Error> {
        let manifest = self.inner.database.latest().await?;
        let segments = manifest
            .index(&self.namespace, &self.name)
            .map_or(&[][..], |e| e.segments.as_slice());
        let synced_version = self.inner.replica.version().await;
        let documents = self
            .inner
            .engine
            .namespace(&self.namespace)
            .ok()
            .and_then(|n| n.get(&self.name))
            .map_or(0, |i| i.len());
        let state = self.inner.state();
        Ok(CollectionStats {
            documents,
            segments: segments.len(),
            bytes: segments.iter().map(|s| s.bytes).sum(),
            version: manifest.version,
            committed_at_ms: manifest.timestamp_ms,
            synced_version,
            synced_at_ms: state.synced_at_ms,
            sync_error: state.sync_error.clone(),
            optimize_error: state.optimize_errors.get(&self.key()).cloned(),
        })
    }
}

fn completion(s: Suggestion, collection: String) -> Completion {
    Completion {
        id: s.id,
        key: s.key,
        text: s.text,
        score: s.score,
        kind: s.kind,
        highlights: s.highlights,
        collection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs() -> Vec<Document> {
        vec![
            Document::keyed("ps5", "PlayStation 5 Console", 0.9).with_abbreviation("PS5"),
            Document::keyed("psvr", "PlayStation VR2", 0.4),
            Document::keyed("anc", "Wireless Noise Cancelling Headphones", 0.7)
                .with_synonym("bluetooth headphones"),
        ]
    }

    fn keys(hits: &[Completion]) -> Vec<&str> {
        hits.iter()
            .map(|c| c.key.as_deref().unwrap_or_default())
            .collect()
    }

    #[tokio::test]
    async fn collections_add_complete_delete_and_stats() {
        let client = connect("memory://", ConnectOptions::default().sync_every(None))
            .await
            .unwrap();
        let products = client
            .get_or_create_collection("products", Optimize::Off)
            .await
            .unwrap();
        assert_eq!(client.collections().await.unwrap(), ["products"]);
        products.add(docs()).await.unwrap();
        let hits = products.complete("play", &Query::default()).unwrap();
        assert_eq!(keys(&hits), ["ps5", "psvr"]);
        assert_eq!(hits[0].collection, "products");

        products.delete([crate::key_id("psvr")]).await.unwrap();
        assert_eq!(
            keys(&products.complete("play", &Query::default()).unwrap()),
            ["ps5"]
        );

        let plain = products.complete("bluetooth", &Query::default()).unwrap();
        assert!(plain.is_empty());
        let synonyms = products
            .complete("bluetooth", &Query::default().aliases(true))
            .unwrap();
        assert_eq!(keys(&synonyms), ["anc"]);
        assert_eq!(synonyms[0].kind, MatchKind::Synonym);

        let stats = products.stats().await.unwrap();
        assert_eq!(stats.documents, 2);
        assert_eq!(stats.segments, 2);
        assert_eq!(stats.synced_version, stats.version);
        products.optimize().await.unwrap();
        assert_eq!(products.stats().await.unwrap().segments, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn readers_follow_and_writers_optimize_by_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let url = dir.path().to_str().unwrap();
        let writer = connect(url, ConnectOptions::default().sync_every(None))
            .await
            .unwrap();
        let auto = Optimize::Auto {
            policy: CompactionPolicy::default().fanout(2),
            min_interval: Duration::ZERO,
        };
        let written = writer.create_collection("products", auto).await.unwrap();
        let reader = connect(
            url,
            ConnectOptions::default().sync_every(Some(Duration::from_millis(20))),
        )
        .await
        .unwrap();
        let read = reader.collection("products").await.unwrap();
        assert!(read.complete("play", &Query::default()).unwrap().is_empty());

        for doc in docs() {
            written.add([doc]).await.unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while read.complete("play", &Query::default()).unwrap().len() < 2 {
            assert!(Instant::now() < deadline, "the reader never caught up");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        while written.stats().await.unwrap().segments > 2 {
            assert!(Instant::now() < deadline, "the writer never compacted");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn layers_override_and_settings_persist() {
        let client = connect("memory://", ConnectOptions::default().sync_every(None))
            .await
            .unwrap();
        let shared = client
            .create_collection("shared", Optimize::default())
            .await
            .unwrap();
        shared.add(docs()).await.unwrap();
        let acme = client
            .create_collection("acme", Optimize::Off)
            .await
            .unwrap();
        acme.add([Document::keyed("psvr", "PlayStation VR2 Bundle", 0.4)])
            .await
            .unwrap();
        let hits = shared
            .complete("play", &Query::default().layers(vec!["acme".into()]))
            .unwrap();
        assert_eq!(hits[1].text, "PlayStation VR2 Bundle");
        assert_eq!(hits[1].collection, "acme");

        assert!(client
            .create_collection("acme", Optimize::default())
            .await
            .is_err());
        assert!(matches!(
            client.collection("acme").await.unwrap().optimize_setting(),
            Optimize::Off
        ));
        assert!(client.collection("missing").await.is_err());
        client.drop_collection("acme").await.unwrap();
        assert_eq!(client.collections().await.unwrap(), ["shared"]);
    }
}
