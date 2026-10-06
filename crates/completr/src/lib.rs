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
    connect, Client, ClientNamespace, Collection, CollectionStats, Completion, ConnectOptions,
    Optimize, Query,
};
#[cfg(feature = "store")]
#[doc(hidden)]
pub use database::validate_name;
#[cfg(feature = "store")]
pub use database::{
    CleanupPolicy, CleanupStats, CompactionPolicy, Database, DatabaseNamespace, FollowStatus,
    Follower, IndexEntry, Lease, Manifest, NamespaceTransaction, Replica, SegmentRef, Transaction,
    BASE_LEVEL,
};
#[doc(hidden)]
pub use dict::Dictionary;
pub use document::{key_id, Alias, AliasKind, Document};
pub use engine::{
    layered_complete, layered_complete_aliases, Engine, LayeredSuggestion, Namespace,
    DEFAULT_NAMESPACE,
};
pub use hybrid::{Fusion, HybridOptions, HybridSuggestion};
#[cfg(feature = "store")]
pub use inbox::{ChangeSet, IngestStep, Ingestor, NamespaceChanges};
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
    /// [`Engine::namespace`] named a namespace the engine does not hold.
    NamespaceNotFound {
        name: String,
        available: Vec<String>,
    },
    /// A required layer named an index its namespace does not hold.
    LayerNotFound {
        namespace: String,
        version: u64,
        name: String,
        available: Vec<String>,
    },
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
            Self::NamespaceNotFound { name, available } => {
                write!(f, "no namespace '{name}'")?;
                write_choices(f, name, available, "namespaces")
            }
            Self::LayerNotFound {
                namespace,
                version,
                name,
                available,
            } => {
                write!(
                    f,
                    "no index '{name}' in namespace '{namespace}' at version {version}"
                )?;
                write_choices(f, name, available, "indexes")?;
                write!(f, "; set ignore_missing_layers if it may not exist")
            }
            Self::Conflict(m) => write!(f, "commit conflict: {m}"),
            #[cfg(feature = "store")]
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

/// Appends a did-you-mean and the names that exist.
fn write_choices(
    f: &mut std::fmt::Formatter<'_>,
    name: &str,
    available: &[String],
    what: &str,
) -> std::fmt::Result {
    if let Some(close) = text::closest(name, available) {
        write!(f, " (did you mean '{close}'?)")?;
    }
    if available.is_empty() {
        return write!(f, "; there are no {what}");
    }
    const SHOWN: usize = 20;
    let shown = available[..available.len().min(SHOWN)].join(", ");
    write!(f, "; {what}: {shown}")?;
    if available.len() > SHOWN {
        write!(f, " and {} more", available.len() - SHOWN)?;
    }
    Ok(())
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
