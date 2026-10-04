//! Serverless autocompletion: exact, prefix, infix, abbreviation, spelling-tolerant, word-decomposing
//! and semantic completions over immutable, memory-mapped segments. With the `store` feature,
//! [`Database`] adds versioned storage on local disk, S3, GCS or Azure.
//!
//! ```
//! use std::sync::Arc;
//! use completr::{AliasKind, Document, Index, IndexOptions, MatchKind, Segment};
//!
//! let docs = [
//!     Document::new(1, "Under the Bridge – Red Hot Chili Peppers", 0.75)
//!         .with_alias("RHCP", AliasKind::Abbreviation),
//!     Document::new(2, "Bohemian Rhapsody – Queen", 0.95),
//! ];
//! let index = Index::new(vec![Arc::new(Segment::build(docs, [])?)], IndexOptions::default())?;
//! let hits = index.complete("rhcp", 10);
//! assert_eq!((hits[0].id, hits[0].kind), (1, MatchKind::Abbreviation));
//! # Ok::<(), completr::Error>(())
//! ```

mod blocked;
mod codec;
/// Chainable setters named after the fields of a non-exhaustive options struct.
macro_rules! setters {
    ($ty:ident { $($field:ident: $t:ty),* $(,)? }) => {
        impl $ty {
            $(
                pub fn $field(mut self, value: $t) -> Self {
                    self.$field = value;
                    self
                }
            )*
        }
    };
}
pub(crate) use setters;

#[cfg(feature = "store")]
mod client;
#[cfg(feature = "store")]
mod database;
mod dict;
mod document;
mod elias_fano;
mod engine;
mod fuzzy;
mod highlight;
mod hybrid;
#[cfg(feature = "store")]
mod inbox;
mod index;
mod postings;
mod search;
mod segment;
mod staged;
#[cfg(feature = "store")]
mod store;
mod text;
mod trie;
mod vectors;

#[cfg(feature = "store")]
pub use client::{
    connect, Client, Collection, CollectionStats, Completion, ConnectOptions, Optimize, Query,
};
#[cfg(feature = "store")]
pub use database::{
    CleanupPolicy, CleanupStats, CompactionPolicy, Database, FollowStatus, Follower, IndexEntry,
    Lease, Manifest, Replica, SegmentRef, Transaction, BASE_LEVEL,
};
#[doc(hidden)]
pub use dict::Dictionary;
pub use document::{key_id, Alias, AliasKind, Document};
pub use engine::{layered_complete, layered_complete_aliases, Engine, LayeredSuggestion};
pub use hybrid::{Fusion, HybridOptions, HybridSuggestion};
#[cfg(feature = "store")]
pub use inbox::{ChangeSet, IngestStep, Ingestor};
pub use index::{Index, IndexOptions};
pub use search::{AliasSuggestion, MatchKind, SearchOptions, Suggestion};
#[doc(hidden)]
pub use segment::Layout;
pub use segment::{BuildOptions, Segment, SegmentBuilder, SegmentWriter};
#[cfg(feature = "store")]
pub use store::{block_on, BlockingStore, ObjectInfo, Store};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(std::io::Error),
    Fst(fst::Error),
    /// Stored data failed its checksum or structural validation.
    Corrupt(String),
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
            Self::Corrupt(m) => write!(f, "corrupt data: {m}"),
            Self::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Self::NotFound(key) => write!(f, "not found: {key}"),
            Self::Conflict(m) => write!(f, "commit conflict: {m}"),
            #[cfg(feature = "store")]
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Fst(e) => Some(e),
            #[cfg(feature = "store")]
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

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
