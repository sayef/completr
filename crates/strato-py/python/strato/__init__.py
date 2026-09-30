"""strato: serverless autocompletion for Rust and Python."""

from ._async import AsyncDatabase, AsyncEngine, connect_async
from ._errors import (
    ConflictError,
    CorruptionError,
    InvalidInputError,
    NotFoundError,
    StorageError,
    StratoError,
)
from .strato import (
    AliasSuggestion,
    ChangeSet,
    Database,
    Document,
    Engine,
    HybridSuggestion,
    Index,
    Ingestor,
    Lease,
    Replica,
    Segment,
    Store,
    Suggestion,
    Transaction,
    __version__,
    key_id,
)


def connect(url, options=None, cache_dir=None, **build_options):
    """Opens the database at `url`: a local path, or an s3://, gs://, az:// or memory:// URL."""
    return Database(url, options, cache_dir, **build_options)


__all__ = [
    "AliasSuggestion",
    "AsyncDatabase",
    "AsyncEngine",
    "ChangeSet",
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
    "StorageError",
    "Store",
    "StratoError",
    "Suggestion",
    "Transaction",
    "__version__",
    "connect",
    "connect_async",
    "key_id",
]
