# asyncio

`completr.AsyncDatabase` is a `Database` whose storage calls are awaitable, for FastAPI and other asyncio
services. It has the same methods as `Database`, awaited. Storage calls run in worker threads through
`asyncio.to_thread`, so they never block the event loop. Completions stay synchronous: they take well
under a millisecond and release the GIL.

## Serving

`AsyncDatabase(url, ...)` takes the same arguments as `completr.connect`. Constructing it does not block:
storage opens on the first awaited call, or on `await db.open()`.

```python
import asyncio

import completr


async def main():
    db = completr.AsyncDatabase("./data")   # or "s3://my-bucket/completions"
    txn = await db.begin()
    txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
    await db.commit(txn)

    engine = await db.engine()   # syncs itself in the background
    print([s.text for s in engine.complete("billie", ["songs"])])


asyncio.run(main())
```

```text
['Billie Jean – Michael Jackson']
```

The engine follows the database from a background thread, as the synchronous one does, so there is no
sync loop to write. `await engine.sync()` loads the latest version now; `engine.complete` and every other
engine method are the synchronous `Engine` methods.

## Methods

| `AsyncDatabase` method | Wraps |
|---|---|
| `await open()`, `async with AsyncDatabase(...) as db` | Opens storage now rather than on the first call. |
| `await versions()`, `await latest_version()` | `Database.versions()`, `Database.latest_version()` |
| `await index_names(version)`, `await manifest(version)` | `Database.index_names`, `Database.manifest` |
| `await begin(version)`, `await commit(txn)` | `Database.begin`, `Transaction.commit` |
| `await open_index(name, version, **options)` | `Database.open_index` |
| `await engine(**options)` | `Database.engine`, returning an `AsyncEngine` |
| `await submit(changes)`, `await pending_change_sets()` | `Database.submit`, `Database.pending_change_sets` |
| `await compact(index, **policy)`, `await cleanup(**policy)` | `Database.compact`, `Database.cleanup` |
| `await run_ingestor(ingestor)` | One round of `Ingestor.run_once()` |
| `database` | The underlying synchronous `Database`, once open |

`Transaction.append` and `Transaction.overwrite` build a segment on the calling thread, which blocks the
event loop for large batches: wrap them in `asyncio.to_thread`, or run bulk loads from a separate worker.
`ChangeSet.upsert` only collects documents; `await db.submit(changes)` builds and writes them in a worker
thread. To run an ingestor under asyncio, see [Many writers](writers.md#under-asyncio).

The [FastAPI example](https://github.com/sayef/completr/tree/main/examples/fastapi) opens a database and
an engine once at import time, and serves `/complete` from the engine with synchronous handlers.

## Collections

`completr.AsyncClient(url, ...)` is the asyncio form of a [collections](collections.md) client. It takes
the same arguments as `completr.Client`, and also opens storage on the first awaited call.

```python
async def collections():
    client = completr.AsyncClient("./data")
    songs = await client.get_or_create_collection("songs")
    await songs.add([{"id": "bad", "text": "Bad Guy – Billie Eilish", "popularity": 0.75}])
    print([s.text for s in songs.complete("billie")])
    print((await songs.stats())["documents"])


asyncio.run(collections())
```

```text
['Billie Jean – Michael Jackson', 'Bad Guy – Billie Eilish']
2
```

| `AsyncClient` | Wraps |
|---|---|
| `await collections()` | `Client.collections()` |
| `await collection(name)`, `await create_collection(name, **settings)`, `await get_or_create_collection(name, **settings)` | The `Client` methods, returning an `AsyncCollection` |
| `await drop_collection(name)` | `Client.drop_collection` |
| `await sync()` | `Client.sync()` |
| `await open()` | Opens storage now. |
| `client` | The underlying synchronous `Client`, once open |

| `AsyncCollection` | Wraps |
|---|---|
| `await add(documents, vectors)`, `await delete(ids)` | `Collection.add`, `Collection.delete` |
| `await optimize()`, `await stats()` | `Collection.optimize`, `Collection.stats` |
| `complete(query, limit, **options)` | `Collection.complete`, synchronous |
| `name` | The collection's name |
