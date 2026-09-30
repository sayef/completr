"""asyncio API: storage calls run in worker threads, completions stay synchronous (sub-millisecond)."""

import asyncio

from .completr import Database, Engine


class AsyncEngine:
    """An `Engine` that follows a database, with `sync()` awaitable."""

    def __init__(self, engine: Engine):
        self._engine = engine

    async def sync(self):
        return await asyncio.to_thread(self._engine.sync)

    def __getattr__(self, name):
        return getattr(self._engine, name)


class AsyncDatabase:
    """A `Database` whose storage operations are awaitable."""

    def __init__(self, database: Database):
        self._database = database

    @property
    def database(self) -> Database:
        return self._database

    async def versions(self):
        return await asyncio.to_thread(self._database.versions)

    async def latest_version(self):
        return await asyncio.to_thread(self._database.latest_version)

    async def index_names(self, version=None):
        return await asyncio.to_thread(self._database.index_names, version)

    async def manifest(self, version=None):
        return await asyncio.to_thread(self._database.manifest, version)

    async def begin(self, version=None):
        return await asyncio.to_thread(self._database.begin, version)

    async def commit(self, transaction):
        return await asyncio.to_thread(transaction.commit)

    async def open_index(self, name, version=None, **options):
        return await asyncio.to_thread(self._database.open_index, name, version, **options)

    async def engine(self, **options):
        return AsyncEngine(await asyncio.to_thread(self._database.engine, **options))

    async def submit(self, changes):
        return await asyncio.to_thread(self._database.submit, changes)

    async def pending_change_sets(self):
        return await asyncio.to_thread(self._database.pending_change_sets)

    async def compact(self, index, **policy):
        return await asyncio.to_thread(self._database.compact, index, **policy)

    async def cleanup(self, **policy):
        return await asyncio.to_thread(self._database.cleanup, **policy)

    async def run_ingestor(self, ingestor):
        """One round of `ingestor.run_once()`."""
        return await asyncio.to_thread(ingestor.run_once)


async def connect_async(url, options=None, cache_dir=None, **build_options) -> AsyncDatabase:
    """Opens the database at `url` without blocking the event loop."""
    database = await asyncio.to_thread(Database, url, options, cache_dir, **build_options)
    return AsyncDatabase(database)
