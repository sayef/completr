from collections.abc import Buffer, Iterable, Mapping, Sequence
from os import PathLike
from typing import Any, Literal, TypedDict

__version__: str

Id = int | str
"""A document id: an int, or a string key."""
MatchKind = Literal["exact", "prefix", "abbreviation", "infix", "fuzzy", "semantic"]
FusionKind = Literal["rrf", "weighted", "lexical_first"]
Vector = Buffer | Sequence[float]
"""An embedding: a 1-D float32 array or a list of floats."""
Vectors = Buffer | Sequence[Sequence[float]]
"""One embedding row per document: a float32 array of shape (n, dim), or lists."""

class DocumentDict(TypedDict, total=False):
    id: Id
    text: str
    popularity: float
    synonyms: Sequence[str]
    abbreviations: Sequence[str]
    contexts: Sequence[str]
    vector: Vector

Documents = Iterable[DocumentDict | Document] | Any
"""Dicts, `Document`s, or a pandas, polars or pyarrow table with those columns."""

class CompletrError(Exception): ...
class ConflictError(CompletrError): ...
class CorruptionError(CompletrError): ...
class NotFoundError(CompletrError, LookupError): ...
class InvalidInputError(CompletrError, ValueError): ...
class StorageError(CompletrError, OSError): ...

def key_id(key: str) -> int: ...
def connect(
    url: str,
    options: Mapping[str, str] | None = None,
    cache_dir: str | PathLike[str] | None = None,
    *,
    min_word_chars: int = 3,
    max_edit_distance: int = 2,
    fuzzy_prefix_chars: int = 7,
    vector_bits: int = 4,
    compact_keys: bool = False,
    build_threads: int = 1,
) -> Database: ...
async def connect_async(
    url: str,
    options: Mapping[str, str] | None = None,
    cache_dir: str | PathLike[str] | None = None,
    **build_options: Any,
) -> AsyncDatabase: ...

class Document:
    def __init__(
        self,
        id: Id,
        text: str,
        popularity: float = 0.0,
        *,
        synonyms: Sequence[str] = ...,
        abbreviations: Sequence[str] = ...,
        contexts: Sequence[str] = ...,
        vector: Vector | None = None,
    ) -> None: ...
    @property
    def id(self) -> Id: ...
    @property
    def text(self) -> str: ...
    @property
    def popularity(self) -> float: ...
    @property
    def synonyms(self) -> list[str]: ...
    @property
    def abbreviations(self) -> list[str]: ...
    @property
    def contexts(self) -> list[str]: ...
    def to_dict(self) -> DocumentDict: ...

class Suggestion:
    id: Id
    text: str
    score: float
    kind: MatchKind
    highlights: list[tuple[int, int]]
    """`(start, end)` character offsets of `text` that matched the query."""
    layer: str | None

class HybridSuggestion:
    id: Id
    text: str
    score: float
    kind: MatchKind
    lexical_score: float | None
    semantic_score: float | None
    highlights: list[tuple[int, int]]
    layer: str | None

class AliasSuggestion:
    id: Id
    text: str
    score: float
    layer: str | None

class SegmentWriter:
    """Writes segment files into `directory`, a new one whenever building would exceed `memory_budget` bytes."""
    def __init__(
        self,
        directory: str | PathLike[str],
        *,
        memory_budget: int = ...,
        min_word_chars: int = 3,
        max_edit_distance: int = 2,
        fuzzy_prefix_chars: int = 7,
        vector_bits: int = 4,
        compact_keys: bool = False,
        build_threads: int = 1,
    ) -> None: ...
    def add(self, documents: DocumentDict | Document | Documents) -> None: ...
    def finish(self) -> list[Segment]: ...

class Segment:
    @staticmethod
    def build(
        documents: Documents,
        deletes: Sequence[Id] = ...,
        vectors: Vectors | None = None,
        *,
        min_word_chars: int = 3,
        max_edit_distance: int = 2,
        fuzzy_prefix_chars: int = 7,
        vector_bits: int = 4,
        compact_keys: bool = False,
        build_threads: int = 1,
        path: str | PathLike[str] | None = None,
    ) -> Segment: ...
    @property
    def vector_dim(self) -> int | None: ...
    @property
    def size_bytes(self) -> int: ...
    @staticmethod
    def open(path: str | PathLike[str]) -> Segment: ...
    @staticmethod
    def from_bytes(data: bytes) -> Segment: ...
    def verify(self) -> None: ...
    def to_bytes(self) -> bytes: ...
    def save(self, path: str | PathLike[str]) -> None: ...
    def documents(self) -> list[Document]: ...
    def ids(self) -> list[int]: ...
    def deletes(self) -> list[int]: ...
    def __len__(self) -> int: ...

class Index:
    def __init__(
        self,
        segments: Sequence[Segment],
        *,
        max_score: float | None = None,
        popularity_weight: float = 0.4,
        short_query_chars: int = 3,
        short_query_limit: int = 100,
        short_query_cache_entries: int = 10_000,
        vector_threads: int = 1,
    ) -> None: ...
    @staticmethod
    def from_documents(
        documents: Documents,
        vectors: Vectors | None = None,
        *,
        max_score: float | None = None,
        popularity_weight: float = 0.4,
        min_word_chars: int = 3,
        max_edit_distance: int = 2,
        fuzzy_prefix_chars: int = 7,
        vector_bits: int = 4,
        build_threads: int = 1,
    ) -> Index: ...
    def complete(self, query: str, limit: int = 10, *, contexts: Sequence[str] | None = None) -> list[Suggestion]: ...
    def complete_aliases(
        self, query: str, limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[AliasSuggestion]: ...
    def vector_search(
        self, vector: Vector, limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[Suggestion]: ...
    def hybrid_search(
        self,
        text: str,
        vector: Vector,
        limit: int = 10,
        *,
        fusion: FusionKind = "rrf",
        rrf_k: float = 60.0,
        semantic_weight: float = 0.5,
        candidates: int | None = None,
        contexts: Sequence[str] | None = None,
    ) -> list[HybridSuggestion]: ...
    def get(self, id: Id) -> Document | None: ...
    def compact(self, path: str | PathLike[str] | None = None) -> Segment: ...
    def segments(self) -> list[Segment]: ...
    @property
    def vector_dim(self) -> int | None: ...
    @property
    def max_score(self) -> float: ...
    def __len__(self) -> int: ...

class Engine:
    def __init__(self, overfetch: int = 2) -> None: ...
    def publish(self, updates: Mapping[str, Index | None]) -> None: ...
    def sync(self) -> int | None:
        """Loads the database's latest version; only for engines from `Database.engine()`."""
    @property
    def version(self) -> int | None: ...
    def get(self, name: str) -> Index | None: ...
    def names(self) -> list[str]: ...
    def complete(
        self, query: str, layers: Sequence[str], limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[Suggestion]: ...
    def complete_aliases(
        self, query: str, layers: Sequence[str], limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[AliasSuggestion]: ...
    def vector_search(
        self, vector: Vector, layers: Sequence[str], limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[Suggestion]: ...
    def hybrid_search(
        self,
        text: str,
        vector: Vector,
        layers: Sequence[str],
        limit: int = 10,
        *,
        fusion: FusionKind = "rrf",
        rrf_k: float = 60.0,
        semantic_weight: float = 0.5,
        candidates: int | None = None,
        contexts: Sequence[str] | None = None,
    ) -> list[HybridSuggestion]: ...

class Store:
    def __init__(
        self, url: str, options: Mapping[str, str] | None = None, cache_dir: str | PathLike[str] | None = None
    ) -> None: ...
    def prune_cache(self, keep: Sequence[str]) -> int: ...
    def get(self, key: str) -> bytes: ...
    def put(self, key: str, data: bytes) -> None: ...
    def put_if_absent(self, key: str, data: bytes) -> bool: ...
    def delete(self, key: str) -> None: ...
    def list(self, prefix: str = "") -> list[str]: ...
    def put_segment(self, key: str, segment: Segment) -> None: ...
    def get_segment(self, key: str) -> Segment: ...

class Transaction:
    @property
    def read_version(self) -> int: ...
    def append(
        self, index: str, documents: Documents, deletes: Sequence[Id] = ..., vectors: Vectors | None = None
    ) -> None: ...
    def overwrite(self, index: str, documents: Documents, vectors: Vectors | None = None) -> None: ...
    def drop_index(self, index: str) -> None: ...
    def set_max_score(self, index: str, max_score: float) -> None: ...
    def set_metadata(self, key: str, value: str | None) -> None: ...
    def strict(self, strict: bool = True) -> None: ...
    def max_retries(self, retries: int) -> None: ...
    def commit(self) -> dict[str, Any]: ...

class Lease:
    @property
    def generation(self) -> int: ...
    def renew(self, ttl_seconds: float) -> bool: ...
    def release(self) -> None: ...

class Database:
    def __init__(
        self,
        url: str,
        options: Mapping[str, str] | None = None,
        cache_dir: str | PathLike[str] | None = None,
        *,
        min_word_chars: int = 3,
        max_edit_distance: int = 2,
        fuzzy_prefix_chars: int = 7,
        vector_bits: int = 4,
        compact_keys: bool = False,
        build_threads: int = 1,
    ) -> None: ...
    def versions(self) -> list[int]: ...
    def latest_version(self) -> int: ...
    def index_names(self, version: int | None = None) -> list[str]: ...
    def manifest(self, version: int | None = None) -> dict[str, Any]: ...
    def begin(self, version: int | None = None) -> Transaction: ...
    def open_index(
        self,
        name: str,
        version: int | None = None,
        *,
        max_score: float | None = None,
        popularity_weight: float = 0.4,
        short_query_chars: int = 3,
        short_query_limit: int = 100,
        short_query_cache_entries: int = 10_000,
        vector_threads: int = 1,
    ) -> Index: ...
    def engine(
        self,
        *,
        group_separator: str | None = None,
        overfetch: int = 2,
        popularity_weight: float = 0.4,
        short_query_chars: int = 3,
        short_query_limit: int = 100,
        short_query_cache_entries: int = 10_000,
        vector_threads: int = 1,
    ) -> Engine: ...
    def compact(
        self,
        index: str,
        *,
        fanout: int = 4,
        max_segments: int = 16,
        max_hidden_fraction: float = 0.25,
        until_done: bool = False,
    ) -> int | None: ...
    def cleanup(self, *, keep_versions: int = 10, older_than_seconds: float = 3600.0) -> dict[str, int]: ...
    def acquire_lease(self, name: str, owner: str, ttl_seconds: float) -> Lease | None: ...
    def submit(self, changes: ChangeSet) -> str: ...
    def pending_change_sets(self) -> int: ...

class ChangeSet:
    def __init__(self) -> None: ...
    def upsert(self, index: str, documents: Documents, vectors: Vectors | None = None) -> None: ...
    def delete(self, index: str, ids: Sequence[Id]) -> None: ...

class Ingestor:
    def __init__(
        self,
        database: Database,
        owner: str,
        *,
        lease_ttl_seconds: float = 30.0,
        max_change_sets: int = 1000,
        compact: bool = True,
    ) -> None: ...
    @property
    def is_active(self) -> bool: ...
    def run_once(self) -> dict[str, Any]: ...
    def release(self) -> None: ...

class Replica:
    def __init__(
        self,
        database: Database,
        engine: Engine,
        *,
        popularity_weight: float = 0.4,
        short_query_chars: int = 3,
        short_query_limit: int = 100,
        short_query_cache_entries: int = 10_000,
        vector_threads: int = 1,
        group_separator: str | None = None,
    ) -> None: ...
    @property
    def version(self) -> int: ...
    def sync(self) -> int | None: ...

class AsyncEngine:
    async def sync(self) -> int | None: ...
    def complete(
        self, query: str, layers: Sequence[str], limit: int = 10, *, contexts: Sequence[str] | None = None
    ) -> list[Suggestion]: ...
    def __getattr__(self, name: str) -> Any: ...

class AsyncDatabase:
    @property
    def database(self) -> Database: ...
    async def versions(self) -> list[int]: ...
    async def latest_version(self) -> int: ...
    async def index_names(self, version: int | None = None) -> list[str]: ...
    async def manifest(self, version: int | None = None) -> dict[str, Any]: ...
    async def begin(self, version: int | None = None) -> Transaction: ...
    async def commit(self, transaction: Transaction) -> dict[str, Any]: ...
    async def open_index(self, name: str, version: int | None = None, **options: Any) -> Index: ...
    async def engine(self, **options: Any) -> AsyncEngine: ...
    async def submit(self, changes: ChangeSet) -> str: ...
    async def pending_change_sets(self) -> int: ...
    async def compact(self, index: str, **policy: Any) -> int | None: ...
    async def cleanup(self, **policy: Any) -> dict[str, int]: ...
    async def run_ingestor(self, ingestor: Ingestor) -> dict[str, Any]: ...
