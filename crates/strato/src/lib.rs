//! Layered autocompletion over immutable, memory-mapped segments. With the `store` feature,
//! [`Dataset`] adds versioned storage on local disk, S3, GCS or Azure.
//!
//! ```
//! use std::sync::Arc;
//! use strato::{AliasKind, Document, Index, IndexConfig, MatchKind, Segment};
//!
//! let docs = [
//!     Document::new(1, "Machine Learning", 0.9).with_alias("ML", AliasKind::Abbreviation),
//!     Document::new(2, "Machine Vision", 0.4),
//! ];
//! let index = Index::new(vec![Arc::new(Segment::build(docs, [])?)], IndexConfig::default())?;
//! let hits = index.autocomplete("ml", 10);
//! assert_eq!((hits[0].id, hits[0].kind), (1, MatchKind::Abbreviation));
//! # Ok::<(), strato::Error>(())
//! ```

mod codec;
#[cfg(feature = "store")]
mod dataset;
mod dict;
mod document;
mod engine;
mod fuzzy;
mod hybrid;
#[cfg(feature = "store")]
mod inbox;
mod index;
mod search;
mod segment;
#[cfg(feature = "store")]
mod store;
mod text;
mod trie;
mod vectors;

#[cfg(feature = "store")]
pub use dataset::{
    CleanupPolicy, CleanupStats, CompactionPolicy, Dataset, Follower, IndexEntry, Lease, Manifest,
    SegmentRef, Transaction, BASE_LEVEL,
};
#[doc(hidden)]
pub use dict::Dictionary;
pub use document::{Alias, AliasKind, Document};
pub use engine::{layered_autocomplete, layered_search_aliases, Engine, LayeredHit};
pub use hybrid::{Fusion, HybridHit, HybridOptions};
#[cfg(feature = "store")]
pub use inbox::{Batch, Writer, WriterStep};
pub use index::{Index, IndexConfig};
pub use search::{AliasHit, Hit, MatchKind};
#[doc(hidden)]
pub use segment::Layout;
pub use segment::{Segment, SegmentConfig};
#[cfg(feature = "store")]
pub use store::{block_on, BlockingStore, ObjectInfo, Store};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Fst(fst::Error),
    Format(String),
    InvalidInput(String),
    NotFound(String),
    /// A concurrent commit changed what this transaction depends on.
    Conflict(String),
    #[cfg(feature = "store")]
    Store(object_store::Error),
}

impl Error {
    pub(crate) fn input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Fst(e) => write!(f, "fst error: {e}"),
            Self::Format(m) => write!(f, "invalid segment: {m}"),
            Self::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Self::NotFound(key) => write!(f, "not found: {key}"),
            Self::Conflict(m) => write!(f, "commit conflict: {m}"),
            #[cfg(feature = "store")]
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<fst::Error> for Error {
    fn from(e: fst::Error) -> Self {
        Self::Fst(e)
    }
}

#[cfg(feature = "store")]
impl From<object_store::Error> for Error {
    fn from(e: object_store::Error) -> Self {
        match e {
            object_store::Error::NotFound { path, .. } => Self::NotFound(path),
            e => Self::Store(e),
        }
    }
}
