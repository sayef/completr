# asyncio

`strato.connect_async` opens the same database with awaitable storage operations, for FastAPI and other
asyncio services. Storage calls run in worker threads through `asyncio.to_thread`, so they never block the
event loop. Completions stay synchronous: they take well under a millisecond and release the GIL.

## Serving

```python
import asyncio
import tempfile

import strato


async def main():
    db = await strato.connect_async(tempfile.mkdtemp())   # or "s3://my-bucket/completions"

    txn = await db.begin()
    txn.append("products", [{"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9}])
    await db.commit(txn)

    engine = await db.engine()
    await engine.sync()                                    # call periodically
    print(engine.complete("wirel", ["products"]))


asyncio.run(main())
```

`AsyncDatabase.engine()` returns an `AsyncEngine`. Its `sync()` is awaitable; `complete` and every other
engine method (`complete_aliases`, `vector_search`, `hybrid_search`, `names`, `version`) are the
synchronous `Engine` methods.

## Keeping an engine fresh

Run the sync as a background task next to your request handlers:

```python
async def follow(engine, interval=5.0):
    while True:
        await asyncio.sleep(interval)
        try:
            await engine.sync()
        except strato.StorageError:
            pass   # keep serving the current version; try again next round
```

The [FastAPI example](https://github.com/sayef/strato/tree/main/examples/fastapi) starts this task in
the application's lifespan and serves `/complete` from the engine.

## Methods

| `AsyncDatabase` method | Wraps |
|---|---|
| `await versions()`, `await latest_version()` | `Database.versions()`, `Database.latest_version()` |
| `await index_names(version)`, `await manifest(version)` | `Database.index_names`, `Database.manifest` |
| `await begin(version)`, `await commit(txn)` | `Database.begin`, `Transaction.commit` |
| `await open_index(name, version, **options)` | `Database.open_index` |
| `await engine(**options)` | `Database.engine`, returning an `AsyncEngine` |
| `await submit(changes)`, `await pending_change_sets()` | `Database.submit`, `Database.pending_change_sets` |
| `await compact(index, **policy)`, `await cleanup(**policy)` | `Database.compact`, `Database.cleanup` |
| `await run_ingestor(ingestor)` | One round of `Ingestor.run_once()` |

`db.database` returns the underlying synchronous `Database`, for example to create an `Ingestor`:

```python
async def ingest(db):
    ingestor = strato.Ingestor(db.database, "worker-1")
    try:
        while True:
            step = await db.run_ingestor(ingestor)
            if step["step"] != "committed":
                await asyncio.sleep(1.0)
    finally:
        ingestor.release()
```

`Transaction.append` and `Transaction.overwrite` build a segment on the calling thread, which blocks the
event loop for large batches: wrap them in `asyncio.to_thread`, or run bulk loads from a separate worker.
`ChangeSet.upsert` only collects documents; `await db.submit(changes)` builds and writes them in a worker
thread.
