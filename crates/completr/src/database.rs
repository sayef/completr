//! Versioned databases: named indexes over segment files, committed optimistically through
//! create-only manifest writes, with rebase on conflict, compaction, cleanup and leases.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::{BuildOptions, Document, Engine, Error, Index, IndexOptions, Segment, Store};

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
}

/// The full state of a database at one version. Version 0 is the empty database and has no file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Manifest {
    pub version: u64,
    pub parent: Option<u64>,
    pub timestamp_ms: u64,
    pub indexes: BTreeMap<String, IndexEntry>,
    pub metadata: BTreeMap<String, String>,
}

impl Manifest {
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

/// Named indexes stored under one store prefix.
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

    /// A transaction against the latest version.
    pub async fn begin(&self) -> Result<Transaction, Error> {
        Ok(self.transaction(self.latest().await?))
    }

    /// A transaction against `read`; commits made since are rebased over where that is safe.
    pub fn transaction(&self, read: Manifest) -> Transaction {
        Transaction {
            database: self.clone(),
            read,
            ops: Vec::new(),
            strict: false,
            max_retries: 32,
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

    /// Loads one index of `manifest`; its `max_score` overrides the one in `config`.
    pub async fn open_index(
        &self,
        manifest: &Manifest,
        name: &str,
        config: IndexOptions,
    ) -> Result<Index, Error> {
        let entry = manifest
            .indexes
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("index {name}")))?;
        let segments = self.load_segments(&entry.segments).await?;
        Index::new(
            segments,
            IndexOptions {
                max_score: Some(entry.max_score),
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
        let read = self.latest().await?;
        let Some(entry) = read.indexes.get(index) else {
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
            let all = self.load_segments(refs).await?;
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
        let segments = self.load_segments(&refs[start..end]).await?;
        let merged = merge(&segments, start == 0)?;
        tracing::info!(
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
        let mut txn = self.transaction(read);
        txn.ops.push(Op::Replace {
            index: index.to_owned(),
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
                    .indexes
                    .values()
                    .flat_map(|e| e.segments.iter().map(|s| s.key.clone())),
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
    view.merged_segment(deletes)
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

enum Op {
    Append {
        index: String,
        segment: Arc<Segment>,
    },
    Overwrite {
        index: String,
        segment: Arc<Segment>,
    },
    Replace {
        index: String,
        remove: Vec<String>,
        segment: Arc<Segment>,
        level: u32,
    },
    Drop {
        index: String,
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
    MaxScore {
        index: String,
        max_score: f64,
    },
    Metadata {
        key: String,
        value: Option<String>,
    },
}

impl Op {
    fn index(&self) -> Option<&str> {
        match self {
            Op::Append { index, .. }
            | Op::Overwrite { index, .. }
            | Op::Replace { index, .. }
            | Op::Drop { index }
            | Op::MaxScore { index, .. } => Some(index),
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
/// On a concurrent commit, appends, drops and metadata rebase onto the newer version (last
/// writer wins per document id); overwrites, and everything in strict mode, fail with
/// [`Error::Conflict`] if an index they touch changed.
pub struct Transaction {
    database: Database,
    read: Manifest,
    ops: Vec<Op>,
    strict: bool,
    max_retries: usize,
}

impl Transaction {
    pub fn read_version(&self) -> u64 {
        self.read.version
    }

    /// Adds a segment on top of `index`, creating the index if needed.
    pub fn append(&mut self, index: &str, segment: Segment) -> &mut Self {
        self.ops.push(Op::Append {
            index: index.to_owned(),
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
        let config = self.database.segment_config;
        Ok(self.append(index, Segment::build_with(config, documents, deletes)?))
    }

    /// Replaces all segments of `index` with one.
    pub fn overwrite(&mut self, index: &str, segment: Segment) -> &mut Self {
        self.ops.push(Op::Overwrite {
            index: index.to_owned(),
            segment: Arc::new(segment),
        });
        self
    }

    pub fn drop_index(&mut self, index: &str) -> &mut Self {
        self.ops.push(Op::Drop {
            index: index.to_owned(),
        });
        self
    }

    /// Pins `index`'s score normalisation. New indexes otherwise estimate it from their first segment.
    pub fn set_max_score(&mut self, index: &str, max_score: f64) -> &mut Self {
        self.ops.push(Op::MaxScore {
            index: index.to_owned(),
            max_score,
        });
        self
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
    pub async fn commit_with_attempts(self) -> Result<(Manifest, usize), Error> {
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
            let Some(index) = op.index() else { continue };
            let changed = self.read.indexes.get(index) != latest.indexes.get(index);
            match op {
                Op::Replace { remove, .. } => {
                    let current = latest
                        .indexes
                        .get(index)
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
                Op::Append { index, segment } => {
                    let reference = staged.unwrap();
                    if !next.indexes.contains_key(index) {
                        let max_score = estimate_max_score(segment)?;
                        next.indexes.insert(
                            index.clone(),
                            IndexEntry {
                                max_score,
                                segments: Vec::new(),
                            },
                        );
                    }
                    let entry = next.indexes.get_mut(index).unwrap();
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
                Op::Overwrite { index, segment } => {
                    let reference = staged.unwrap();
                    let max_score = match next.indexes.get(index) {
                        Some(entry) => entry.max_score,
                        None => estimate_max_score(segment)?,
                    };
                    next.indexes.insert(
                        index.clone(),
                        IndexEntry {
                            max_score,
                            segments: vec![reference.clone()],
                        },
                    );
                }
                Op::Replace { index, remove, .. } => {
                    let reference = staged.unwrap();
                    let entry = next
                        .indexes
                        .get_mut(index)
                        .ok_or_else(|| Error::Conflict(format!("{index} was dropped")))?;
                    let start = find_run(&entry.segments, remove).ok_or_else(|| {
                        Error::Conflict(format!("segments of {index} being replaced changed"))
                    })?;
                    entry
                        .segments
                        .splice(start..start + remove.len(), [reference.clone()]);
                }
                Op::Drop { index } => {
                    next.indexes.remove(index);
                }
                Op::MaxScore { index, max_score } => {
                    next.indexes
                        .entry(index.clone())
                        .or_insert_with(|| IndexEntry {
                            max_score: *max_score,
                            segments: Vec::new(),
                        })
                        .max_score = *max_score;
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

/// Keeps an [`Engine`] on the latest version of a database, loading only segments it lacks and
/// republishing only indexes that changed.
pub struct Replica {
    database: Database,
    config: IndexOptions,
    group: Arc<dyn Fn(&str) -> String + Send + Sync>,
    state: tokio::sync::Mutex<ReplicaState>,
}

#[derive(Default)]
struct ReplicaState {
    version: u64,
    loaded: FxHashMap<String, (Vec<String>, f64)>,
    segments: FxHashMap<String, Arc<Segment>>,
}

impl Replica {
    /// `config` applies to every index; each index's `max_score` comes from the manifest.
    pub fn new(database: Database, config: IndexOptions) -> Self {
        Self {
            database,
            config,
            group: Arc::new(str::to_owned),
            state: tokio::sync::Mutex::default(),
        }
    }

    /// Indexes with the same group key are switched together; groups one after another. By
    /// default each index is its own group.
    pub fn with_groups(mut self, group: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        self.group = Arc::new(group);
        self
    }

    /// Groups by the part after the last `separator`, e.g. `"/"` switches all `tenant/language`
    /// indexes of one language together.
    pub fn with_groups_by_suffix(self, separator: &str) -> Self {
        let separator = separator.to_owned();
        self.with_groups(move |name| {
            name.rsplit_once(separator.as_str())
                .map_or(name, |(_, s)| s)
                .to_owned()
        })
    }

    pub async fn version(&self) -> u64 {
        self.state.lock().await.version
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
        let removed: Vec<String> = state
            .loaded
            .keys()
            .filter(|n| !manifest.indexes.contains_key(*n))
            .cloned()
            .collect();
        if !removed.is_empty() {
            engine.publish(removed.iter().map(|n| (n.clone(), None)));
            for name in &removed {
                state.loaded.remove(name);
            }
            release_unreferenced(&mut state);
        }

        // Then one group at a time: load, publish, release the replaced segments, so memory
        // never holds more than one group twice.
        let mut groups: std::collections::BTreeMap<String, Vec<(&String, &IndexEntry)>> =
            Default::default();
        for (name, entry) in &manifest.indexes {
            let ids: Vec<&String> = entry.segments.iter().map(|s| &s.id).collect();
            let unchanged = state
                .loaded
                .get(name)
                .is_some_and(|(i, m)| i.iter().eq(ids.iter().copied()) && *m == entry.max_score);
            if !unchanged {
                groups
                    .entry((self.group)(name))
                    .or_default()
                    .push((name, entry));
            }
        }
        for members in groups.into_values() {
            let mut missing: Vec<SegmentRef> = Vec::new();
            for segment in members.iter().flat_map(|(_, e)| &e.segments) {
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
            let mut updates = Vec::with_capacity(members.len());
            let mut carried = Vec::new();
            for (name, entry) in &members {
                let segments = entry
                    .segments
                    .iter()
                    .map(|s| state.segments[&s.id].clone())
                    .collect();
                let config = IndexOptions {
                    max_score: Some(entry.max_score),
                    ..self.config.clone()
                };
                let index = Arc::new(Index::new(segments, config)?);
                if self.config.warm_on_load {
                    index.warm();
                }
                if let Some(old) = engine.get(name) {
                    carried.push((
                        old.hot_short_queries(self.config.carry_short_queries),
                        index.clone(),
                    ));
                }
                updates.push(((*name).clone(), Some(index)));
            }
            engine.publish(updates);
            for (name, entry) in &members {
                let ids = entry.segments.iter().map(|s| s.id.clone()).collect();
                state.loaded.insert((*name).clone(), (ids, entry.max_score));
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
            .indexes
            .values()
            .flat_map(|e| e.segments.iter().map(|s| s.key.as_str()))
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
