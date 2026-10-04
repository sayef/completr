"""completr: serverless autocompletion for Rust and Python."""

from ._async import AsyncClient, AsyncCollection, AsyncDatabase, AsyncEngine
from ._errors import (
    CompletrError,
    ConflictError,
    CorruptionError,
    InvalidInputError,
    NotFoundError,
    StorageError,
)
from .completr import (
    AliasSuggestion,
    ChangeSet,
    Client,
    Collection,
    Database,
    Document,
    Engine,
    HybridSuggestion,
    Index,
    Ingestor,
    Lease,
    Replica,
    Segment,
    SegmentWriter,
    Store,
    Suggestion,
    Transaction,
    __version__,
    key_id,
)


def connect(url, options=None, cache_dir=None, **build_options) -> Database:
    """Opens the database at `url`: a local path, or an s3://, gs://, az:// or memory:// URL."""
    return Database(url, options, cache_dir, **build_options)


__all__ = [
    "AliasSuggestion",
    "AsyncClient",
    "AsyncCollection",
    "AsyncDatabase",
    "AsyncEngine",
    "ChangeSet",
    "Client",
    "Collection",
    "CompletrError",
    "ConflictError",
    "CorruptionError",
    "Database",
    "Document",
    "Engine",
    "HybridSuggestion",
    "Index",
    "Ingestor",
    "InvalidInputError",
    "Lease",
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
