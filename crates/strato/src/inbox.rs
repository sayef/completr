//! Single-writer ingestion: any process submits batches to an inbox in the store, and the one
//! process holding the writer lease folds them into a single commit per round.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dataset::claimed;
use crate::{CompactionPolicy, Dataset, Document, Error, Index, IndexConfig, Lease, Segment};

const INBOX: &str = "_inbox";
const REJECTED: &str = "_rejected";
const MAGIC: &[u8; 8] = b"STRATOIB";
const WRITER_LEASE: &str = "writer";
const APPLIED: &str = "strato.inbox.applied";
const GENERATION: &str = "strato.writer.generation";

/// Changes to one or more indexes, applied together and in submission order relative to
/// other batches. Later batches win per document id.
#[derive(Default)]
pub struct Batch {
    changes: BTreeMap<String, (Vec<Document>, Vec<u64>)>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(
        &mut self,
        index: &str,
        documents: impl IntoIterator<Item = Document>,
    ) -> &mut Self {
        self.changes
            .entry(index.to_owned())
            .or_default()
            .0
            .extend(documents);
        self
    }

    pub fn delete(&mut self, index: &str, ids: impl IntoIterator<Item = u64>) -> &mut Self {
        self.changes
            .entry(index.to_owned())
            .or_default()
            .1
            .extend(ids);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.changes
            .values()
            .all(|(docs, deletes)| docs.is_empty() && deletes.is_empty())
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    id: String,
    /// Index name and byte length of its segment, in file order.
    parts: Vec<(String, u64)>,
}

fn encode(id: &str, parts: &[(String, Segment)]) -> Vec<u8> {
    let header = Header {
        id: id.to_owned(),
        parts: parts
            .iter()
            .map(|(n, s)| (n.clone(), s.size_bytes() as u64))
            .collect(),
    };
    let json = serde_json::to_vec(&header).expect("header serialises");
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&(json.len() as u64).to_le_bytes());
    out.extend_from_slice(&json);
    for (_, segment) in parts {
        out.extend_from_slice(&segment.to_bytes());
    }
    out
}

fn decode(data: &[u8]) -> Result<(String, Vec<(String, Segment)>), Error> {
    let bad = || Error::Format("invalid inbox batch".into());
    if data.len() < 16 || &data[..8] != MAGIC {
        return Err(bad());
    }
    let len =
        usize::try_from(u64::from_le_bytes(data[8..16].try_into().unwrap())).map_err(|_| bad())?;
    let header: Header =
        serde_json::from_slice(data.get(16..16 + len).ok_or_else(bad)?).map_err(|_| bad())?;
    let mut pos = 16 + len;
    let mut parts = Vec::with_capacity(header.parts.len());
    for (name, size) in header.parts {
        let end = pos
            .checked_add(usize::try_from(size).map_err(|_| bad())?)
            .ok_or_else(bad)?;
        parts.push((
            name,
            Segment::from_bytes(data.get(pos..end).ok_or_else(bad)?.to_vec())?,
        ));
        pos = end;
    }
    Ok((header.id, parts))
}

/// Ids sort in submission order within a process: monotonic milliseconds, then a sequence
/// number, then a random part for uniqueness across processes.
fn batch_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_MS: AtomicU64 = AtomicU64::new(0);
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = crate::dataset::now_ms();
    let ms = LAST_MS.fetch_max(now, Ordering::SeqCst).max(now);
    let sequence = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    format!("{ms:016}-{sequence:016}-{}", uuid::Uuid::new_v4().simple())
}

impl Dataset {
    /// Queues `batch` for the writer; returns its id. Any process may submit. A process's
    /// batches apply in the order it submitted them; across processes, in order of arrival.
    pub async fn submit(&self, batch: Batch) -> Result<String, Error> {
        if batch.is_empty() {
            return Err(Error::input("empty batch"));
        }
        let id = batch_id();
        let config = self.segment_config();
        let mut parts = Vec::with_capacity(batch.changes.len());
        for (index, (documents, deletes)) in batch.changes {
            parts.push((index, Segment::build_with(config, documents, deletes)?));
        }
        let key = format!("{INBOX}/{id}.batch");
        if !self
            .store()
            .put_if_absent(&key, encode(&id, &parts))
            .await?
        {
            return Err(Error::Conflict(format!("inbox batch {id} exists")));
        }
        Ok(id)
    }

    /// Batches waiting in the inbox.
    pub async fn pending_batches(&self) -> Result<usize, Error> {
        Ok(self.store().list_objects(INBOX).await?.len())
    }
}

/// What one writer round did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriterStep {
    /// Another process holds the writer lease.
    NotLeader,
    /// The inbox was empty.
    Idle,
    /// Batches were committed as one version.
    Committed {
        version: u64,
        batches: usize,
        documents: usize,
    },
}

/// Drains the inbox while holding the writer lease; run one per process, and whichever holds
/// the lease writes. Commits are fenced by the lease generation.
pub struct Writer {
    dataset: Dataset,
    owner: String,
    lease: Option<Lease>,
    renewed: Instant,
    /// Lease lifetime; the lease is renewed after a third of it.
    pub lease_ttl: Duration,
    /// Batches folded into one commit at most.
    pub max_batches: usize,
    /// After each commit, compact the indexes it touched until nothing is due; `None` leaves
    /// compaction to the caller. Keeps compaction on the writer, off serving processes.
    pub compaction: Option<CompactionPolicy>,
}

impl Writer {
    pub fn new(dataset: Dataset, owner: impl Into<String>) -> Self {
        Self {
            dataset,
            owner: owner.into(),
            lease: None,
            renewed: Instant::now(),
            lease_ttl: Duration::from_secs(30),
            max_batches: 1000,
            compaction: Some(CompactionPolicy::default()),
        }
    }

    pub fn is_leader(&self) -> bool {
        self.lease.is_some()
    }

    async fn ensure_lease(&mut self) -> Result<Option<u64>, Error> {
        match &mut self.lease {
            Some(lease) if self.renewed.elapsed() >= self.lease_ttl / 3 => {
                if !lease.renew(self.lease_ttl).await? {
                    self.lease = None;
                    return Ok(None);
                }
                self.renewed = Instant::now();
            }
            Some(_) => {}
            None => {
                self.lease = self
                    .dataset
                    .acquire_lease(WRITER_LEASE, &self.owner, self.lease_ttl)
                    .await?;
                self.renewed = Instant::now();
            }
        }
        Ok(self.lease.as_ref().map(Lease::generation))
    }

    /// One round: take or keep the lease, fold pending batches into one commit, and delete them.
    pub async fn run_once(&mut self) -> Result<WriterStep, Error> {
        let Some(generation) = self.ensure_lease().await? else {
            return Ok(WriterStep::NotLeader);
        };
        let store = self.dataset.store().clone();
        let read = self.dataset.latest().await?;
        let applied = claimed(&read, APPLIED);

        let mut objects = store.list_objects(INBOX).await?;
        objects.sort_by(|a, b| (a.modified_ms, &a.key).cmp(&(b.modified_ms, &b.key)));
        let batch_id = |key: &str| {
            key.strip_prefix(INBOX)
                .and_then(|k| k.strip_prefix('/')?.strip_suffix(".batch"))
                .map(str::to_owned)
        };
        let (done, pending): (Vec<_>, Vec<_>) = objects
            .into_iter()
            .filter_map(|o| batch_id(&o.key).map(|id| (id, o.key)))
            .partition(|(id, _)| applied.contains(id));
        store
            .delete_many(done.iter().map(|(_, key)| key.as_str()))
            .await?;
        let pending: Vec<(String, String)> = pending.into_iter().take(self.max_batches).collect();
        if pending.is_empty() {
            return Ok(WriterStep::Idle);
        }

        let loads = pending
            .iter()
            .map(|(_, key)| async { store.get(key).await.map(|data| decode(&data)) });
        let loaded = futures::future::try_join_all(loads).await?;
        let mut batches = Vec::with_capacity(pending.len());
        let mut accepted = Vec::with_capacity(pending.len());
        for ((id, key), loaded) in pending.into_iter().zip(loaded) {
            match loaded {
                Ok(batch) => {
                    batches.push(batch);
                    accepted.push((id, key));
                }
                // An undecodable batch would block the inbox forever; it is set aside instead.
                Err(Error::Format(_)) => {
                    store
                        .put(&format!("{REJECTED}/{id}.batch"), store.get(&key).await?)
                        .await?;
                    store.delete(&key).await?;
                }
                Err(e) => return Err(e),
            }
        }
        let pending = accepted;
        if pending.is_empty() {
            return Ok(WriterStep::Idle);
        }
        let mut per_index: BTreeMap<String, Vec<Arc<Segment>>> = BTreeMap::new();
        let mut documents = 0;
        for (_, parts) in batches {
            for (index, segment) in parts {
                documents += segment.len();
                per_index.entry(index).or_default().push(Arc::new(segment));
            }
        }
        let touched: Vec<String> = per_index.keys().cloned().collect();
        let mut txn = self.dataset.transaction(read);
        for (index, segments) in per_index {
            let segment = if segments.len() == 1 {
                Arc::try_unwrap(segments.into_iter().next().unwrap())
                    .unwrap_or_else(|s| Segment::from_bytes(s.to_bytes()).unwrap())
            } else {
                let deletes = segments
                    .iter()
                    .flat_map(|s| s.deletes().iter().copied())
                    .collect();
                let view = Index::new(
                    segments,
                    IndexConfig {
                        max_score: Some(1.0),
                        ..IndexConfig::default()
                    },
                )?;
                view.merged_segment(deletes)?
            };
            txn.append(&index, segment);
        }
        // Applied ids stay recorded until their batch objects are gone.
        let keep: Vec<String> = done.iter().map(|(id, _)| id.clone()).collect();
        txn.claim(
            APPLIED,
            pending.iter().map(|(id, _)| id.clone()).collect(),
            keep,
        )
        .fence(GENERATION, generation);
        let manifest = txn.commit().await?;
        store
            .delete_many(pending.iter().map(|(_, key)| key.as_str()))
            .await?;
        let mut version = manifest.version;
        if let Some(policy) = &self.compaction {
            for index in &touched {
                match self.dataset.compact_all(index, policy).await {
                    Ok(Some(compacted)) => version = compacted.version,
                    Ok(None) | Err(Error::Conflict(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(WriterStep::Committed {
            version,
            batches: pending.len(),
            documents,
        })
    }

    /// Releases the lease so another process can take over at once.
    pub async fn release(mut self) -> Result<(), Error> {
        match self.lease.take() {
            Some(lease) => lease.release().await,
            None => Ok(()),
        }
    }
}
