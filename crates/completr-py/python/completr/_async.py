"""asyncio API: the same classes and method names, awaited. Storage calls run in worker threads; completions
stay synchronous (sub-millisecond). Constructors do not block: storage opens on the first awaited call."""

import asyncio

from .completr import Client, Collection, Database, Engine


class _Lazy:
    """Opens its synchronous counterpart in a worker thread, once, on first use."""

    def __init__(self, opener, *args, **kwargs):
        self._opener, self._args, self._kwargs = opener, args, kwargs
        self._opened = None
        self._lock = None

    async def _get(self):
        if self._opened is None:
            self._lock = self._lock or asyncio.Lock()
            async with self._lock:
                if self._opened is None:
                    self._opened = await asyncio.to_thread(self._opener, *self._args, **self._kwargs)
        return self._opened

    async def _call(self, name, *args, **kwargs):
        target = await self._get()
        return await asyncio.to_thread(getattr(target, name), *args, **kwargs)

    async def open(self):
        """Opens storage now rather than on the first call."""
        await self._get()
        return self

    async def __aenter__(self):
        return await self.open()

    async def __aexit__(self, *exc):
        return None


class AsyncEngine:
    """An `Engine` whose `sync()` is awaitable; completions stay synchronous."""

    def __init__(self, engine: Engine):
        self._engine = engine

    async def sync(self):
        return await asyncio.to_thread(self._engine.sync)

    def __getattr__(self, name):
        return getattr(self._engine, name)


class AsyncDatabase(_Lazy):
    """A `Database` whose storage operations are awaitable: `AsyncDatabase(url, ...)` takes `connect`'s arguments."""

    def __init__(self, url, options=None, cache_dir=None, **build_options):
        super().__init__(Database, url, options, cache_dir, **build_options)

    @property
    def database(self) -> Database:
        """The synchronous `Database`, once opened."""
        if self._opened is None:
            raise RuntimeError("the database is not open yet; await one of its methods or open() first")
        return self._opened

    async def versions(self):
        return await self._call("versions")

    async def latest_version(self):
        return await self._call("latest_version")

    async def index_names(self, version=None):
        return await self._call("index_names", version)

    async def manifest(self, version=None):
        return await self._call("manifest", version)

    async def begin(self, version=None):
        return await self._call("begin", version)

    async def commit(self, transaction):
        return await asyncio.to_thread(transaction.commit)

    async def open_index(self, name, version=None, **options):
        return await self._call("open_index", name, version, **options)

    async def engine(self, **options):
        return AsyncEngine(await self._call("engine", **options))

    async def submit(self, changes):
        return await self._call("submit", changes)

    async def pending_change_sets(self):
        return await self._call("pending_change_sets")

    async def compact(self, index, **policy):
        return await self._call("compact", index, **policy)

    async def cleanup(self, **policy):
        return await self._call("cleanup", **policy)

    async def run_ingestor(self, ingestor):
        """One round of `ingestor.run_once()`."""
        return await asyncio.to_thread(ingestor.run_once)


class AsyncCollection:
    """A `Collection` whose writes and storage calls are awaitable; `complete` stays synchronous."""

    def __init__(self, collection: Collection):
        self._collection = collection

    @property
    def name(self) -> str:
        return self._collection.name

    async def add(self, documents, vectors=None):
        return await asyncio.to_thread(self._collection.add, documents, vectors)

    async def delete(self, ids):
        return await asyncio.to_thread(self._collection.delete, ids)

    async def optimize(self):
        return await asyncio.to_thread(self._collection.optimize)

    async def stats(self):
        return await asyncio.to_thread(self._collection.stats)

    def complete(self, query, limit=10, **options):
        return self._collection.complete(query, limit, **options)

    def __repr__(self):
        return f"Async{self._collection!r}"


class AsyncClient(_Lazy):
    """A `Client` whose storage calls are awaitable: `AsyncClient(url, ...)` takes `Client`'s arguments."""

    def __init__(self, url="memory://", **options):
        super().__init__(Client, url, **options)

    @property
    def client(self) -> Client:
        """The synchronous `Client`, once opened."""
        if self._opened is None:
            raise RuntimeError("the client is not open yet; await one of its methods or open() first")
        return self._opened

    async def collections(self):
        return await self._call("collections")

    async def collection(self, name):
        return AsyncCollection(await self._call("collection", name))

    async def create_collection(self, name, **settings):
        return AsyncCollection(await self._call("create_collection", name, **settings))

    async def get_or_create_collection(self, name, **settings):
        return AsyncCollection(await self._call("get_or_create_collection", name, **settings))

    async def drop_collection(self, name):
        return await self._call("drop_collection", name)

    async def sync(self):
        return await self._call("sync")
