"""completr: serverless autocompletion for Rust and Python."""

from ._async import (
    AsyncClient,
    AsyncClientNamespace,
    AsyncCollection,
    AsyncDatabase,
    AsyncDatabaseNamespace,
    AsyncEngine,
)
from ._errors import (
    CompletrError,
    ConflictError,
    CorruptionError,
    InvalidInputError,
    LayerNotFoundError,
    NamespaceNotFoundError,
    NotFoundError,
    StorageError,
)
from ._types import MatchKind
from .completr import (
    AliasSuggestion,
    ChangeSet,
    Client,
    ClientNamespace,
    Collection,
    Database,
    DatabaseNamespace,
    Document,
    Engine,
    HybridSuggestion,
    Index,
    Ingestor,
    Lease,
    Namespace,
    NamespaceChanges,
    NamespaceTransaction,
    Replica,
    Segment,
    SegmentWriter,
    Store,
    Suggestion,
    Transaction,
    __version__,
    key_id,
)

DEFAULT_NAMESPACE = "default"


def connect(url, options=None, cache_dir=None, **build_options) -> Database:
    """Opens the database at `url`: a local path, or an s3://, gs://, az:// or memory:// URL."""
    return Database(url, options, cache_dir, **build_options)


__all__ = [
    "DEFAULT_NAMESPACE",
    "AliasSuggestion",
    "AsyncClient",
    "AsyncClientNamespace",
    "AsyncCollection",
    "AsyncDatabase",
    "AsyncDatabaseNamespace",
    "AsyncEngine",
    "ChangeSet",
    "Client",
    "ClientNamespace",
    "Collection",
    "CompletrError",
    "ConflictError",
    "CorruptionError",
    "Database",
    "DatabaseNamespace",
    "Document",
    "Engine",
    "HybridSuggestion",
    "Index",
    "Ingestor",
    "InvalidInputError",
    "LayerNotFoundError",
    "Lease",
    "MatchKind",
    "Namespace",
    "NamespaceChanges",
    "NamespaceNotFoundError",
    "NamespaceTransaction",
    "NotFoundError",
    "Replica",
    "Segment",
    "SegmentWriter",
    "StorageError",
    "Store",
    "Suggestion",
    "Transaction",
    "__version__",
    "connect",
    "key_id",
]
