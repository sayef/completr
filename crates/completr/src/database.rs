//! Versioned databases: named indexes over segment files, committed optimistically through
//! create-only manifest writes, with rebase on conflict, compaction, cleanup and leases.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::{
    BuildOptions, Document, Engine, Error, Index, IndexOptions, Segment, Store, DEFAULT_NAMESPACE,
};

const VERSIONS: &str = "_versions";
const SEGMENTS: &str = "segments";
const LOCKS: &str = "_locks";

/// Level of a segment written by an overwrite or a full compaction; tiered compaction skips it.
pub const BASE_LEVEL: u32 = 255;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SegmentRef {
    pub id: String,
    pub key: String,
    pub level: u32,
    pub documents: u64,
    pub deletes: u64,
    /// Size of the segment file; 0 in manifests written before it was recorded.
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IndexEntry {
    pub max_score: f64,
    /// Oldest first.
    pub segments: Vec<SegmentRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl IndexEntry {
    fn new(max_score: f64) -> Self {
        Self {
            max_score,
            segments: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

/// Indexes of one namespace by name.
pub type Indexes = BTreeMap<String, IndexEntry>;

/// The full state of a database at one version. Version 0 is the empty database and has no file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredManifest")]
#[non_exhaustive]
pub struct Manifest {
    pub version: u64,
    pub parent: Option<u64>,
    pub timestamp_ms: u64,
    /// Namespaces by name; none is empty.
    pub namespaces: BTreeMap<String, Indexes>,
    pub metadata: BTreeMap<String, String>,
}

/// A manifest as stored; those written before namespaces keep their indexes in `indexes`.
#[derive(Deserialize)]
struct StoredManifest {
    version: u64,
    parent: Option<u64>,
    timestamp_ms: u64,
    #[serde(default)]
    namespaces: BTreeMap<String, Indexes>,
    #[serde(default)]
    indexes: Indexes,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

impl From<StoredManifest> for Manifest {
    fn from(stored: StoredManifest) -> Self {
        let mut namespaces = stored.namespaces;
        if !stored.indexes.is_empty() {
            namespaces
                .entry(DEFAULT_NAMESPACE.to_owned())
                .or_default()
                .extend(stored.indexes);
        }
        Self {
            version: stored.version,
            parent: stored.parent,
            timestamp_ms: stored.timestamp_ms,
            namespaces,
            metadata: stored.metadata,
        }
    }
}

impl Manifest {
    pub fn index(&self, namespace: &str, name: &str) -> Option<&IndexEntry> {
        self.namespaces.get(namespace)?.get(name)
    }

    /// Index names of `namespace`, sorted; empty if it does not exist.
    pub fn index_names(&self, namespace: &str) -> Vec<String> {
        self.namespaces
            .get(namespace)
            .map(|n| n.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Every index as `(namespace, name, entry)`.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, &IndexEntry)> {
        self.namespaces.iter().flat_map(|(namespace, indexes)| {
            indexes
                .iter()
                .map(move |(name, entry)| (namespace.as_str(), name.as_str(), entry))
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest serialises")
    }

    pub fn from_json(data: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(data).map_err(|e| Error::Corrupt(format!("invalid manifest: {e}")))
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn version_key(version: u64) -> String {
    format!("{VERSIONS}/{version:020}.json")
}

fn parse_version(key: &str) -> Option<u64> {
    key.strip_prefix(VERSIONS)?
        .strip_prefix('/')?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// Namespace and index names: 1 to 128 ASCII letters, digits, `.`, `_` or `-`.
pub fn validate_name(what: &str, name: &str) -> Result<(), Error> {
    let valid = (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !valid {
        return Err(Error::input(format!(
            "{what} name {name:?} must be 1 to 128 letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}

/// Named indexes in named namespaces, stored under one store prefix.
#[derive(Clone, Debug)]
pub struct Database {
    store: Store,
    segment_config: BuildOptions,
}

impl Database {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            segment_config: BuildOptions::default(),
        }
    }

    /// Build settings for segments this database writes from documents.
    pub fn with_build_options(mut self, config: BuildOptions) -> Self {
        self.segment_config = config;
        self
    }

    pub fn build_options(&self) -> BuildOptions {
        self.segment_config
    }

    pub async fn open<K: Into<String>, V: Into<String>>(
        url: &str,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, Error> {
        Ok(Self::new(Store::open_with(url, options).await?))
    }

    /// Caches downloaded segments in `dir` and memory-maps them.
    pub fn with_cache_dir(self, dir: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        Ok(Self {
            store: self.store.with_cache_dir(dir)?,
            ..self
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Committed versions, ascending.
    pub async fn versions(&self) -> Result<Vec<u64>, Error> {
        let objects = self.store.list_objects(VERSIONS).await?;
        let mut versions: Vec<u64> = objects
            .iter()
            .filter_map(|o| parse_version(&o.key))
            .collect();
        versions.sort_unstable();
        Ok(versions)
    }

    /// The newest version above `after`, if any. Lists only keys after it, so polling stays cheap.
    pub async fn latest_version_after(&self, after: u64) -> Result<Option<u64>, Error> {
        let objects = self
            .store
            .list_objects_after(VERSIONS, &version_key(after))
            .await?;
        Ok(objects
            .iter()
            .filter_map(|o| parse_version(&o.key))
            .filter(|&v| v > after)
            .max())
    }

    pub async fn latest_version(&self) -> Result<u64, Error> {
        Ok(self.latest_version_after(0).await?.unwrap_or(0))
    }

    pub async fn manifest(&self, version: u64) -> Result<Manifest, Error> {
        if version == 0 {
            return Ok(Manifest::default());
        }
        let manifest = Manifest::from_json(&self.store.get(&version_key(version)).await?)?;
        if manifest.version != version {
            return Err(Error::Corrupt(format!(
                "manifest {version} claims version {}",
                manifest.version
            )));
        }
        Ok(manifest)
    }

    pub async fn latest(&self) -> Result<Manifest, Error> {
        self.manifest(self.latest_version().await?).await
    }

    /// Namespace `name`, for writing, opening and compacting its indexes. Does no I/O.
    pub fn namespace(&self, name: &str) -> Result<DatabaseNamespace, Error> {
        validate_name("namespace", name)?;
        Ok(DatabaseNamespace {
            database: self.clone(),
            name: name.to_owned(),
        })
    }

    fn default_namespace(&self) -> DatabaseNamespace {
        DatabaseNamespace {
            database: self.clone(),
            name: DEFAULT_NAMESPACE.to_owned(),
        }
    }

    /// A transaction against the latest version, writing to the default namespace.
    pub async fn begin(&self) -> Result<Transaction, Error> {
        Ok(self.transaction(self.latest().await?))
    }

    /// A transaction against `read`; commits made since are rebased over where that is safe.
    pub fn transaction(&self, read: Manifest) -> Transaction {
        Transaction {
            database: self.clone(),
            namespace: DEFAULT_NAMESPACE.to_owned(),
            read,
            ops: Vec::new(),
            strict: false,
            max_retries: 32,
            invalid: None,
        }
    }

    /// Segment files are never overwritten, so cached copies are used without a round trip.
    pub async fn load_segment(&self, segment: &SegmentRef) -> Result<Segment, Error> {
        self.store
            .get_immutable_segment(&segment.key, (segment.bytes > 0).then_some(segment.bytes))
            .await
    }

    async fn load_segments(&self, refs: &[SegmentRef]) -> Result<Vec<Arc<Segment>>, Error> {
        let loads = refs
            .iter()
            .map(|r| async move { self.load_segment(r).await.map(Arc::new) });
        futures::future::try_join_all(loads).await
    }

    /// [`DatabaseNamespace::open_index`] in the default namespace.
    pub async fn open_index(
        &self,
        manifest: &Manifest,
        name: &str,
        config: IndexOptions,
    ) -> Result<Index, Error> {
        self.default_namespace()
            .open_index(manifest, name, config)
            .await
    }

    /// [`DatabaseNamespace::compact`] in the default namespace.
    pub async fn compact(
        &self,
        index: &str,
        policy: &CompactionPolicy,
    ) -> Result<Option<Manifest>, Error> {
        self.default_namespace().compact(index, policy).await
    }

    /// [`DatabaseNamespace::compact_all`] in the default namespace.
    pub async fn compact_all(
        &self,
        index: &str,
        policy: &CompactionPolicy,
    ) -> Result<Option<Manifest>, Error> {
        self.default_namespace().compact_all(index, policy).await
    }

    /// Deletes old manifests and segment files no retained manifest references.
    ///
    /// Only objects older than `policy.older_than` by the store's clock are touched, which protects
    /// lagging readers and uploads of transactions that have not committed yet.
    pub async fn cleanup(&self, policy: &CleanupPolicy) -> Result<CleanupStats, Error> {
        let cutoff = self.store.now_ms().await? - policy.older_than.as_millis() as i64;
        let mut versions: Vec<(u64, i64)> = self
            .store
            .list_objects(VERSIONS)
            .await?
            .into_iter()
            .filter_map(|o| parse_version(&o.key).map(|v| (v, o.modified_ms)))
            .collect();
        versions.sort_unstable();
        let keep_from = versions.len().saturating_sub(policy.keep_versions.max(1));
        let (old, recent) = versions.split_at(keep_from);
        let (removable, kept_old): (Vec<_>, Vec<_>) =
            old.iter().partition(|(_, modified)| *modified <= cutoff);

        let mut referenced = std::collections::HashSet::new();
        for &(version, _) in kept_old.iter().chain(recent) {
            let manifest = self.manifest(version).await?;
            referenced.extend(
                manifest
                    .entries()
                    .flat_map(|(_, _, e)| e.segments.iter().map(|s| s.key.clone())),
            );
        }
        let mut stats = CleanupStats::default();
        for &(version, _) in &removable {
            self.store.delete(&version_key(version)).await?;
            stats.versions_removed += 1;
        }
        for object in self.store.list_objects(SEGMENTS).await? {
            if object.modified_ms <= cutoff && !referenced.contains(&object.key) {
                self.store.delete(&object.key).await?;
                stats.segments_removed += 1;
                stats.bytes_removed += object.size;
            }
        }
        tracing::info!(
            versions = stats.versions_removed,
            segments = stats.segments_removed,
            bytes = stats.bytes_removed,
            "cleaned up"
        );
        Ok(stats)
    }

    /// Takes the lease `name` if it is free, released or expired. Returns `None` while someone
    /// else holds it.
    pub async fn acquire_lease(
        &self,
        name: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<Lease>, Error> {
        let prefix = format!("{LOCKS}/{name}");
        let current = self
            .store
            .list(&prefix)
            .await?
            .iter()
            .filter_map(|k| parse_generation(&prefix, k))
            .max();
        if let Some(generation) = current {
            match self.store.get(&lease_key(&prefix, generation)).await {
                Ok(data) => {
                    let record: LeaseRecord = serde_json::from_slice(&data)
                        .map_err(|e| Error::Corrupt(format!("invalid lease: {e}")))?;
                    if !record.released && record.expires_at_ms > now_ms() {
                        return Ok(None);
                    }
                }
                // A newer generation replaced it meanwhile; let the caller retry later.
                Err(Error::NotFound(_)) => return Ok(None),
                Err(e) => return Err(e),
            }
        }
        let mut lease = Lease {
            store: self.store.clone(),
            prefix,
            owner: owner.to_owned(),
            generation: current.unwrap_or(0),
            expires_at_ms: 0,
        };
        Ok(lease.advance(ttl, false).await?.then_some(lease))
    }
}

/// One namespace of a [`Database`]: its transactions, indexes and compaction.
#[derive(Clone, Debug)]
pub struct DatabaseNamespace {
    database: Database,
    name: String,
}

impl DatabaseNamespace {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    /// A transaction against the latest version, writing to this namespace.
    pub async fn begin(&self) -> Result<Transaction, Error> {
        let mut txn = self.database.begin().await?;
        txn.namespace = self.name.clone();
        Ok(txn)
    }

    /// Index names at the latest version, sorted.
    pub async fn index_names(&self) -> Result<Vec<String>, Error> {
        Ok(self.database.latest().await?.index_names(&self.name))
    }

    /// Loads one index of `manifest`; its `max_score` overrides the one in `config`.
    pub async fn open_index(
        &self,
        manifest: &Manifest,
        name: &str,
        config: IndexOptions,
    ) -> Result<Index, Error> {
        let entry = manifest
            .index(&self.name, name)
            .ok_or_else(|| Error::NotFound(format!("index {name} in namespace {}", self.name)))?;
        let segments = self.database.load_segments(&entry.segments).await?;
        Index::new(
            segments,
            IndexOptions {
                max_score: (entry.max_score > 0.0).then_some(entry.max_score),
                ..config
            },
        )
    }

    /// Runs one compaction step on `index` of the latest version; `None` if nothing is due.
    ///
    /// Builds without holding any lock and commits by rebasing over newer segments.
    pub async fn compact(
        &self,
        index: &str,
        policy: &CompactionPolicy,
    ) -> Result<Option<Manifest>, Error> {
        let read = self.database.latest().await?;
        let Some(entry) = read.index(&self.name, index) else {
            return Ok(None);
        };
        if entry.segments.len() < 2 {
            return Ok(None);
        }
        // Each newer document or delete hides at most one older document, so this bounds the
        // hidden fraction without loading anything.
        let refs = &entry.segments;
        let total: u64 = refs.iter().map(|s| s.documents).sum();
        let newer: u64 = refs[1..].iter().map(|s| s.documents + s.deletes).sum();
        let mut full = false;
        if total > 0 && newer as f64 / total as f64 > policy.max_hidden_fraction {
            let all = self.database.load_segments(refs).await?;
            let config = IndexOptions {
                max_score: Some(1.0),
                ..IndexOptions::default()
            };
            let live = Index::new(all, config)?.len();
            full = 1.0 - live as f64 / total as f64 > policy.max_hidden_fraction;
        }
        let tiered = policy.tiered_run(refs);
        full |= tiered.is_none() && refs.len() > policy.max_segments;
        let (start, end, level) = match (full, tiered) {
            (true, _) => (0, refs.len(), BASE_LEVEL),
            (false, Some(run)) => run,
            (false, None) => return Ok(None),
        };
        let started = std::time::Instant::now();
        let segments = self.database.load_segments(&refs[start..end]).await?;
        let merged = merge(&segments, start == 0)?;
        tracing::info!(
            namespace = self.name.as_str(),
            index,
            merged = end - start,
            level,
            full,
            documents = merged.len(),
            ms = started.elapsed().as_millis() as u64,
            "compacting"
        );
        let remove = entry.segments[start..end]
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let mut txn = self.database.transaction(read);
        txn.ops.push(Op::Replace {
            target: Target {
                namespace: self.name.clone(),
                index: index.to_owned(),
            },
            remove,
            segment: Arc::new(merged),
            level,
        });
        txn.commit().await.map(Some)
    }

    /// Runs compaction steps on `index` until none is due; returns the last new manifest.
    pub async fn compact_all(
        &self,
        index: &str,
        policy: &CompactionPolicy,
    ) -> Result<Option<Manifest>, Error> {
        let mut last = None;
        while let Some(manifest) = self.compact(index, policy).await? {
            last = Some(manifest);
        }
        Ok(last)
    }
}

/// Merges a contiguous run of segments; deletes are kept unless the run starts at the oldest.
fn merge(run: &[Arc<Segment>], oldest: bool) -> Result<Segment, Error> {
    let view = Index::new(
        run.to_vec(),
        IndexOptions {
            max_score: Some(1.0),
            ..IndexOptions::default()
        },
    )?;
    let deletes: Vec<u64> = if oldest {
        Vec::new()
    } else {
        run.iter()
            .flat_map(|s| s.deletes().iter().copied())
            .collect()
    };
    view.merged_segment(deletes, None, 1)
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CompactionPolicy {
    /// Merge this many consecutive segments of one level into one of the next level.
    pub fanout: usize,
    /// Above this many segments, and with no tiered merge available, merge everything into one base.
    pub max_segments: usize,
    /// Above this fraction of superseded or deleted documents, merge everything into one base.
    pub max_hidden_fraction: f64,
}

crate::setters!(CompactionPolicy {
    fanout: usize,
    max_segments: usize,
    max_hidden_fraction: f64,
});

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            fanout: 4,
            max_segments: 16,
            max_hidden_fraction: 0.25,
        }
    }
}

impl CompactionPolicy {
    /// The lowest-level run of at least `fanout` consecutive same-level segments.
    fn tiered_run(&self, segments: &[SegmentRef]) -> Option<(usize, usize, u32)> {
        let mut best: Option<(usize, usize, u32)> = None;
        let mut start = 0;
        while start < segments.len() {
            let level = segments[start].level;
            let end = start
                + segments[start..]
                    .iter()
                    .take_while(|s| s.level == level)
                    .count();
            if level < BASE_LEVEL
                && end - start >= self.fanout.max(2)
                && best.is_none_or(|b| level < b.2)
            {
                best = Some((start, end, level + 1));
            }
            start = end;
        }
        best
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CleanupPolicy {
    /// Newest versions always kept, at least one.
    pub keep_versions: usize,
    pub older_than: Duration,
}

crate::setters!(CleanupPolicy {
    keep_versions: usize,
    older_than: std::time::Duration,
});

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            keep_versions: 10,
            older_than: Duration::from_secs(3600),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanupStats {
    pub versions_removed: usize,
    pub segments_removed: usize,
    pub bytes_removed: u64,
}

/// An index of a namespace that a transaction operation touches.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    namespace: String,
    index: String,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} in namespace {}", self.index, self.namespace)
    }
}

enum Op {
    Append {
        target: Target,
        segment: Arc<Segment>,
    },
    Overwrite {
        target: Target,
        segment: Arc<Segment>,
    },
    Replace {
        target: Target,
        remove: Vec<String>,
        segment: Arc<Segment>,
        level: u32,
    },
    Drop {
        target: Target,
    },
    MaxScore {
        target: Target,
        max_score: f64,
    },
    IndexMetadata {
        target: Target,
        key: String,
        value: Option<String>,
    },
    /// Refuses to commit if metadata `key` holds a higher generation, then records ours.
    Fence {
        key: String,
        generation: u64,
    },
    /// Refuses to commit if metadata `key` (a JSON list) already holds one of `ids`, then sets it
    /// to `keep` plus `ids`.
    Claim {
        key: String,
        ids: Vec<String>,
        keep: Vec<String>,
    },
    Metadata {
        key: String,
        value: Option<String>,
    },
}

impl Op {
    fn target(&self) -> Option<&Target> {
        match self {
            Op::Append { target, .. }
            | Op::Overwrite { target, .. }
            | Op::Replace { target, .. }
            | Op::Drop { target }
            | Op::MaxScore { target, .. }
            | Op::IndexMetadata { target, .. } => Some(target),
            Op::Metadata { .. } | Op::Fence { .. } | Op::Claim { .. } => None,
        }
    }

    fn segment(&self) -> Option<&Arc<Segment>> {
        match self {
            Op::Append { segment, .. }
            | Op::Overwrite { segment, .. }
            | Op::Replace { segment, .. } => Some(segment),
            _ => None,
        }
    }
}

/// Changes staged against one manifest and committed together.
///
/// Index operations write to the transaction's namespace, or to another through
/// [`Transaction::namespace`]. On a concurrent commit, appends, drops and metadata rebase onto the
/// newer version (last writer wins per document id); overwrites, and everything in strict mode,
/// fail with [`Error::Conflict`] if an index they touch changed.
pub struct Transaction {
    database: Database,
    namespace: String,
    read: Manifest,
    ops: Vec<Op>,
    strict: bool,
    max_retries: usize,
    invalid: Option<Error>,
}

/// Index operations of a [`Transaction`] in one namespace.
pub struct NamespaceTransaction<'a> {
    txn: &'a mut Transaction,
    namespace: String,
}

impl NamespaceTransaction<'_> {
    fn target(&mut self, index: &str) -> Target {
        if self.txn.invalid.is_none() {
            self.txn.invalid = validate_name("index", index).err();
        }
        Target {
            namespace: self.namespace.clone(),
            index: index.to_owned(),
        }
    }

    /// Adds a segment on top of `index`, creating the index if needed.
    pub fn append(&mut self, index: &str, segment: Segment) -> &mut Self {
        let target = self.target(index);
        self.txn.ops.push(Op::Append {
            target,
            segment: Arc::new(segment),
        });
        self
    }

    pub fn append_documents(
        &mut self,
        index: &str,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
    ) -> Result<&mut Self, Error> {
        let config = self.txn.database.segment_config;
        Ok(self.append(index, Segment::build_with(config, documents, deletes)?))
    }

    /// Replaces all segments of `index` with one.
    pub fn overwrite(&mut self, index: &str, segment: Segment) -> &mut Self {
        let target = self.target(index);
        self.txn.ops.push(Op::Overwrite {
            target,
            segment: Arc::new(segment),
        });
        self
    }

    pub fn drop_index(&mut self, index: &str) -> &mut Self {
        let target = self.target(index);
        self.txn.ops.push(Op::Drop { target });
        self
    }

    /// Pins `index`'s score normalisation. New indexes otherwise estimate it from their first segment.
    pub fn set_max_score(&mut self, index: &str, max_score: f64) -> &mut Self {
        let target = self.target(index);
        self.txn.ops.push(Op::MaxScore { target, max_score });
        self
    }

    /// Sets or, with `None`, removes metadata `key` of `index`.
    pub fn set_index_metadata(&mut self, index: &str, key: &str, value: Option<&str>) -> &mut Self {
        let target = self.target(index);
        self.txn.ops.push(Op::IndexMetadata {
            target,
            key: key.to_owned(),
            value: value.map(str::to_owned),
        });
        self
    }
}

impl Transaction {
    fn here(&mut self) -> NamespaceTransaction<'_> {
        let namespace = self.namespace.clone();
        NamespaceTransaction {
            txn: self,
            namespace,
        }
    }

    /// Adds a segment on top of `index` in the transaction's namespace, creating it if needed.
    pub fn append(&mut self, index: &str, segment: Segment) -> &mut Self {
        self.here().append(index, segment);
        self
    }

    pub fn append_documents(
        &mut self,
        index: &str,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
    ) -> Result<&mut Self, Error> {
        self.here().append_documents(index, documents, deletes)?;
        Ok(self)
    }

    /// Replaces all segments of `index` with one.
    pub fn overwrite(&mut self, index: &str, segment: Segment) -> &mut Self {
        self.here().overwrite(index, segment);
        self
    }

    pub fn drop_index(&mut self, index: &str) -> &mut Self {
        self.here().drop_index(index);
        self
    }

    pub fn set_max_score(&mut self, index: &str, max_score: f64) -> &mut Self {
        self.here().set_max_score(index, max_score);
        self
    }

    pub fn set_index_metadata(&mut self, index: &str, key: &str, value: Option<&str>) -> &mut Self {
        self.here().set_index_metadata(index, key, value);
        self
    }
}

impl Transaction {
    pub fn read_version(&self) -> u64 {
        self.read.version
    }

    /// The namespace index operations on the transaction itself write to.
    pub fn default_namespace(&self) -> &str {
        &self.namespace
    }

    /// Index operations in namespace `name`, committed with the rest of this transaction.
    pub fn namespace(&mut self, name: &str) -> NamespaceTransaction<'_> {
        if self.invalid.is_none() {
            self.invalid = validate_name("namespace", name).err();
        }
        NamespaceTransaction {
            txn: self,
            namespace: name.to_owned(),
        }
    }

    /// Fences this commit with a lease `generation`: it fails with [`Error::Conflict`] once a
    /// commit with a higher generation for `key` has landed.
    pub fn fence(&mut self, key: &str, generation: u64) -> &mut Self {
        self.ops.push(Op::Fence {
            key: key.to_owned(),
            generation,
        });
        self
    }

    pub(crate) fn claim(&mut self, key: &str, ids: Vec<String>, keep: Vec<String>) -> &mut Self {
        self.ops.push(Op::Claim {
            key: key.to_owned(),
            ids,
            keep,
        });
        self
    }

    pub fn set_metadata(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        self.ops.push(Op::Metadata {
            key: key.to_owned(),
            value: value.map(str::to_owned),
        });
        self
    }

    /// Fail instead of rebasing when any touched index changed since the read version.
    pub fn strict(&mut self, strict: bool) -> &mut Self {
        self.strict = strict;
        self
    }

    /// Retries of the create-only manifest write before giving up with [`Error::Conflict`].
    pub fn max_retries(&mut self, retries: usize) -> &mut Self {
        self.max_retries = retries;
        self
    }

    /// Uploads the new segments, then commits the next version with a create-only write.
    pub async fn commit(self) -> Result<Manifest, Error> {
        self.commit_with_attempts()
            .await
            .map(|(manifest, _)| manifest)
    }

    /// Like [`Transaction::commit`], also returning how many manifest writes it took.
    pub async fn commit_with_attempts(mut self) -> Result<(Manifest, usize), Error> {
        if let Some(error) = self.invalid.take() {
            return Err(error);
        }
        for op in &self.ops {
            if let Op::MaxScore { max_score, .. } = op {
                if !(max_score.is_finite() && *max_score > 0.0) {
                    return Err(Error::input("max_score must be positive and finite"));
                }
            }
        }
        let store = &self.database.store;
        let mut refs = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            let Some(segment) = op.segment() else {
                refs.push(None);
                continue;
            };
            let id = uuid::Uuid::new_v4().simple().to_string();
            let key = format!("{SEGMENTS}/{id}.seg");
            store.put_segment(&key, segment).await?;
            let level = match op {
                Op::Replace { level, .. } => *level,
                Op::Overwrite { .. } => BASE_LEVEL,
                _ => 0,
            };
            let reference = SegmentRef {
                id,
                key,
                level,
                documents: segment.len() as u64,
                deletes: segment.deletes().len() as u64,
                bytes: segment.size_bytes() as u64,
            };
            refs.push(Some(reference));
        }

        let mut latest = self.read.clone();
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                backoff(attempt).await;
            }
            if let Some(version) = self.database.latest_version_after(latest.version).await? {
                latest = self.database.manifest(version).await?;
            }
            self.check_conflicts(&latest)?;
            let next = self.apply(&latest, &refs)?;
            if store
                .put_if_absent(&version_key(next.version), next.to_json().into_bytes())
                .await?
            {
                tracing::info!(
                    version = next.version,
                    attempts = attempt + 1,
                    segments = refs.iter().flatten().count(),
                    "committed"
                );
                return Ok((next, attempt + 1));
            }
        }
        tracing::warn!(
            attempts = self.max_retries + 1,
            "commit gave up under contention"
        );
        Err(Error::Conflict(format!(
            "gave up after {} concurrent commits",
            self.max_retries + 1
        )))
    }

    fn check_conflicts(&self, latest: &Manifest) -> Result<(), Error> {
        for op in &self.ops {
            match op {
                Op::Fence { key, generation } => {
                    let current = latest
                        .metadata
                        .get(key)
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    if current > *generation {
                        return Err(Error::Conflict(format!(
                            "{key} is held by a newer generation {current}"
                        )));
                    }
                }
                Op::Claim { key, ids, .. }
                    if claimed(latest, key).iter().any(|c| ids.contains(c)) =>
                {
                    return Err(Error::Conflict(format!("{key} already applied")));
                }
                _ => {}
            }
            let Some(index) = op.target() else { continue };
            let entry = |m: &Manifest| m.index(&index.namespace, &index.index).cloned();
            let changed = entry(&self.read) != entry(latest);
            match op {
                Op::Replace { remove, .. } => {
                    let current = latest
                        .index(&index.namespace, &index.index)
                        .map_or(&[][..], |e| &e.segments[..]);
                    if find_run(current, remove).is_none() {
                        return Err(Error::Conflict(format!(
                            "segments of {index} being replaced changed"
                        )));
                    }
                }
                _ if changed && (self.strict || matches!(op, Op::Overwrite { .. })) => {
                    return Err(Error::Conflict(format!(
                        "{index} changed since version {}",
                        self.read.version
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn apply(&self, latest: &Manifest, refs: &[Option<SegmentRef>]) -> Result<Manifest, Error> {
        let mut next = latest.clone();
        next.version = latest.version + 1;
        next.parent = (latest.version > 0).then_some(latest.version);
        next.timestamp_ms = now_ms();
        for (op, staged) in self.ops.iter().zip(refs) {
            let staged = staged.as_ref();
            match op {
                Op::Append { target, segment } => {
                    let reference = staged.unwrap();
                    let entry = entry_mut(&mut next, target);
                    // An index without segments and a pinned max_score takes it from its first.
                    if entry.segments.is_empty() && entry.max_score == 0.0 {
                        entry.max_score = estimate_max_score(segment)?;
                    }
                    // The first segment of an index is its base.
                    let level = if entry.segments.is_empty() {
                        BASE_LEVEL
                    } else {
                        reference.level
                    };
                    entry.segments.push(SegmentRef {
                        level,
                        ..reference.clone()
                    });
                }
                Op::Overwrite { target, segment } => {
                    let reference = staged.unwrap();
                    let entry = entry_mut(&mut next, target);
                    if entry.max_score == 0.0 {
                        entry.max_score = estimate_max_score(segment)?;
                    }
                    entry.segments = vec![reference.clone()];
                }
                Op::Replace { target, remove, .. } => {
                    let reference = staged.unwrap();
                    let entry = next
                        .namespaces
                        .get_mut(&target.namespace)
                        .and_then(|n| n.get_mut(&target.index))
                        .ok_or_else(|| Error::Conflict(format!("{target} was dropped")))?;
                    let start = find_run(&entry.segments, remove).ok_or_else(|| {
                        Error::Conflict(format!("segments of {target} being replaced changed"))
                    })?;
                    entry
                        .segments
                        .splice(start..start + remove.len(), [reference.clone()]);
                }
                Op::Drop { target } => {
                    if let Some(indexes) = next.namespaces.get_mut(&target.namespace) {
                        indexes.remove(&target.index);
                        if indexes.is_empty() {
                            next.namespaces.remove(&target.namespace);
                        }
                    }
                }
                Op::MaxScore { target, max_score } => {
                    entry_mut(&mut next, target).max_score = *max_score;
                }
                Op::IndexMetadata { target, key, value } => {
                    let metadata = &mut entry_mut(&mut next, target).metadata;
                    match value {
                        Some(value) => metadata.insert(key.clone(), value.clone()),
                        None => metadata.remove(key),
                    };
                }
                Op::Fence { key, generation } => {
                    next.metadata.insert(key.clone(), generation.to_string());
                }
                Op::Claim { key, ids, keep } => {
                    let all: Vec<&String> = keep.iter().chain(ids).collect();
                    next.metadata.insert(
                        key.clone(),
                        serde_json::to_string(&all).expect("ids serialise"),
                    );
                }
                Op::Metadata { key, value } => match value {
                    Some(value) => {
                        next.metadata.insert(key.clone(), value.clone());
                    }
                    None => {
                        next.metadata.remove(key);
                    }
                },
            }
        }
        Ok(next)
    }
}

/// The entry of `target`, created empty, with its max_score unset, if missing.
fn entry_mut<'a>(manifest: &'a mut Manifest, target: &Target) -> &'a mut IndexEntry {
    manifest
        .namespaces
        .entry(target.namespace.clone())
        .or_default()
        .entry(target.index.clone())
        .or_insert_with(|| IndexEntry::new(0.0))
}

fn estimate_max_score(segment: &Arc<Segment>) -> Result<f64, Error> {
    Ok(Index::new(vec![segment.clone()], IndexOptions::default())?.max_score())
}

/// Exponential backoff with full jitter, from 10 ms up to 1 s.
async fn backoff(attempt: usize) {
    let cap = (10u64 << attempt.min(7)).min(1000);
    let jitter = (uuid::Uuid::new_v4().as_u128() % u128::from(cap + 1)) as u64;
    tokio::time::sleep(Duration::from_millis(jitter)).await;
}

/// The ids a JSON-list metadata entry records.
pub(crate) fn claimed(manifest: &Manifest, key: &str) -> Vec<String> {
    manifest
        .metadata
        .get(key)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
}

fn find_run(segments: &[SegmentRef], ids: &[String]) -> Option<usize> {
    if ids.is_empty() {
        return None;
    }
    segments
        .windows(ids.len())
        .position(|w| w.iter().zip(ids).all(|(s, id)| s.id == *id))
}

#[derive(Serialize, Deserialize)]
struct LeaseRecord {
    owner: String,
    expires_at_ms: u64,
    released: bool,
}

fn lease_key(prefix: &str, generation: u64) -> String {
    format!("{prefix}/{generation:020}.json")
}

fn parse_generation(prefix: &str, key: &str) -> Option<u64> {
    key.strip_prefix(prefix)?
        .strip_prefix('/')?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// An advisory, expiring lock, e.g. to run one writer or compactor at a time.
///
/// Every acquire, renew and release creates the next generation file create-only, so exactly
/// one contender wins it on any store. The generation is a fencing token. Expiry relies on
/// roughly synchronised clocks; commits stay safe without the lease.
#[derive(Debug)]
pub struct Lease {
    store: Store,
    prefix: String,
    owner: String,
    generation: u64,
    expires_at_ms: u64,
}

impl Lease {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    async fn advance(&mut self, ttl: Duration, released: bool) -> Result<bool, Error> {
        let expires_at_ms = if released {
            0
        } else {
            now_ms() + ttl.as_millis() as u64
        };
        let record = LeaseRecord {
            owner: self.owner.clone(),
            expires_at_ms,
            released,
        };
        let data = serde_json::to_vec(&record).expect("lease serialises");
        let next = self.generation + 1;
        if !self
            .store
            .put_if_absent(&lease_key(&self.prefix, next), data)
            .await?
        {
            return Ok(false);
        }
        if self.generation > 0 {
            let _ = self
                .store
                .delete(&lease_key(&self.prefix, self.generation))
                .await;
        }
        self.generation = next;
        self.expires_at_ms = expires_at_ms;
        Ok(true)
    }

    /// Extends the lease; `false` means it was lost to someone else.
    pub async fn renew(&mut self, ttl: Duration) -> Result<bool, Error> {
        self.advance(ttl, false).await
    }

    pub async fn release(mut self) -> Result<(), Error> {
        self.advance(Duration::ZERO, true).await.map(|_| ())
    }
}

/// Keeps an engine on its replica's latest version from a background thread, until dropped.
pub struct Follower {
    stop: Arc<std::sync::atomic::AtomicBool>,
    status: Arc<std::sync::Mutex<FollowStatus>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct FollowStatus {
    /// When the last background sync finished, in milliseconds since the Unix epoch.
    pub synced_at_ms: Option<u64>,
    /// Its error; the engine keeps serving the version it has.
    pub error: Option<String>,
}

impl Follower {
    pub fn status(&self) -> FollowStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A change to one index of an engine namespace; `None` removes it.
type Update = (String, Option<Arc<Index>>);

/// Keeps an [`Engine`] on the latest version of a database, loading only segments it lacks and
/// switching each namespace to its new indexes in one step, one namespace at a time.
pub struct Replica {
    database: Database,
    config: IndexOptions,
    state: tokio::sync::Mutex<ReplicaState>,
}

#[derive(Default)]
struct ReplicaState {
    version: u64,
    /// Segment ids and max_score of each loaded index, by namespace and name.
    loaded: FxHashMap<(String, String), (Vec<String>, f64)>,
    segments: FxHashMap<String, Arc<Segment>>,
}

impl Replica {
    /// `config` applies to every index; each index's `max_score` comes from the manifest.
    pub fn new(database: Database, config: IndexOptions) -> Self {
        Self {
            database,
            config,
            state: tokio::sync::Mutex::default(),
        }
    }

    pub async fn version(&self) -> u64 {
        self.state.lock().await.version
    }

    /// Syncs `engine` every `every` from a background thread, until the returned follower is dropped
    /// or the replica or engine is.
    pub fn follow(self: &Arc<Self>, engine: &Arc<Engine>, every: Duration) -> Follower {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let status = Arc::new(std::sync::Mutex::new(FollowStatus::default()));
        let (replica, engine) = (Arc::downgrade(self), Arc::downgrade(engine));
        let (stopped, recorded) = (stop.clone(), status.clone());
        std::thread::Builder::new()
            .name("completr-sync".into())
            .spawn(move || loop {
                std::thread::sleep(every);
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let (Some(replica), Some(engine)) = (replica.upgrade(), engine.upgrade()) else {
                    break;
                };
                let result = crate::store::block_on(replica.sync(&engine));
                let mut status = recorded.lock().unwrap_or_else(|e| e.into_inner());
                status.synced_at_ms = Some(now_ms());
                status.error = result.err().map(|e| e.to_string());
            })
            .ok();
        Follower { stop, status }
    }

    /// Publishes the latest version into `engine`; returns it if it is new.
    pub async fn sync(&self, engine: &Engine) -> Result<Option<u64>, Error> {
        let mut state = self.state.lock().await;
        let Some(version) = self.database.latest_version_after(state.version).await? else {
            return Ok(None);
        };
        let manifest = self.database.manifest(version).await?;
        let started = std::time::Instant::now();
        let mut downloaded = 0usize;

        // Removals first, so their memory is free before anything new loads.
        let mut removed: BTreeMap<String, Vec<Update>> = BTreeMap::new();
        for (namespace, name) in state.loaded.keys() {
            if manifest.index(namespace, name).is_none() {
                removed
                    .entry(namespace.clone())
                    .or_default()
                    .push((name.clone(), None));
            }
        }
        for (namespace, updates) in removed {
            for (name, _) in &updates {
                state.loaded.remove(&(namespace.clone(), name.clone()));
            }
            engine.update(&namespace, Some(version), updates);
        }
        release_unreferenced(&mut state);

        // Then one namespace at a time: load, publish, release the replaced segments, so memory
        // never holds more than one namespace twice.
        for (namespace, indexes) in &manifest.namespaces {
            let changed: Vec<(&String, &IndexEntry)> = indexes
                .iter()
                .filter(|(name, entry)| {
                    let key = (namespace.clone(), (*name).clone());
                    !state.loaded.get(&key).is_some_and(|(ids, max_score)| {
                        ids.iter().eq(entry.segments.iter().map(|s| &s.id))
                            && *max_score == entry.max_score
                    })
                })
                .collect();
            if changed.is_empty() {
                engine.update(namespace, Some(version), Vec::new());
                continue;
            }
            let mut missing: Vec<SegmentRef> = Vec::new();
            for segment in changed.iter().flat_map(|(_, e)| &e.segments) {
                if !state.segments.contains_key(&segment.id)
                    && !missing.iter().any(|m| m.id == segment.id)
                {
                    missing.push(segment.clone());
                }
            }
            downloaded += missing.len();
            for (reference, segment) in missing
                .iter()
                .zip(self.database.load_segments(&missing).await?)
            {
                state.segments.insert(reference.id.clone(), segment);
            }
            let current = engine.namespace(namespace).ok();
            let mut updates = Vec::with_capacity(changed.len());
            let mut carried = Vec::new();
            for (name, entry) in &changed {
                let segments = entry
                    .segments
                    .iter()
                    .map(|s| state.segments[&s.id].clone())
                    .collect();
                // An index created without documents has no score scale yet.
                let config = IndexOptions {
                    max_score: (entry.max_score > 0.0).then_some(entry.max_score),
                    ..self.config.clone()
                };
                let index = Arc::new(Index::new(segments, config)?);
                if self.config.warm_on_load {
                    index.warm();
                }
                if let Some(old) = current.as_ref().and_then(|n| n.get(name)) {
                    carried.push((
                        old.hot_short_queries(self.config.carry_short_queries),
                        index.clone(),
                    ));
                }
                updates.push(((*name).clone(), Some(index)));
            }
            drop(current);
            engine.update(namespace, Some(version), updates);
            for (name, entry) in &changed {
                let ids = entry.segments.iter().map(|s| s.id.clone()).collect();
                state
                    .loaded
                    .insert((namespace.clone(), (*name).clone()), (ids, entry.max_score));
            }
            release_unreferenced(&mut state);
            // Published first, so readers see the new version while its cache fills.
            if !carried.is_empty() {
                tokio::task::spawn_blocking(move || {
                    for (queries, index) in carried {
                        index.prefill_short_queries(&queries);
                    }
                });
            }
        }

        let keys: Vec<&str> = manifest
            .entries()
            .flat_map(|(_, _, e)| e.segments.iter().map(|s| s.key.as_str()))
            .collect();
        self.database.store.prune_cache(keys)?;
        state.version = version;
        tracing::info!(
            version,
            segments_loaded = downloaded,
            ms = started.elapsed().as_millis() as u64,
            "replica synced"
        );
        Ok(Some(version))
    }
}

/// Drops loaded segments that no current index uses.
fn release_unreferenced(state: &mut ReplicaState) {
    let referenced: std::collections::HashSet<&String> =
        state.loaded.values().flat_map(|(ids, _)| ids).collect();
    state.segments.retain(|id, _| referenced.contains(id));
}
