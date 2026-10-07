# Serverless deployment

A completr deployment has no completr server. It has a bucket or directory (the database), and your
processes: some commit transactions, and the ones that answer requests serve an engine that follows the
database. This page covers storage, credentials, transactions, serving, compaction, cleanup, leases and
memory.

!!! warning "Keep large writes out of serving processes"
    Building and merging segments uses far more memory and CPU than serving. Run bulk loads, frequent
    writes and compaction in separate processes (a worker, a cron job, an [ingestor](writers.md) or the
    [command-line tool](cli.md)), not inside the processes that answer completion requests.

## Storage backends

`completr.connect(url)` opens a database. Local directories are created if they do not exist.

| URL | Backend |
|---|---|
| `/path/to/db`, `file:///path/to/db` | Local file system. Segments are memory-mapped in place. |
| `s3://bucket/prefix` | Amazon S3 or an S3-compatible store (set `aws_endpoint` for the latter). |
| `gs://bucket/prefix` | Google Cloud Storage. |
| `az://container/prefix` | Azure Blob Storage. |
| `memory://`, `memory:///name` | In memory, for tests. Each `connect` gets a fresh, empty store. |

Every backend supports the create-only writes that commits rely on: `If-None-Match` on object stores,
exclusive create on a file system.

=== "Python"

    ```python
    import completr

    db = completr.connect("./data")   # or s3://bucket/prefix, gs://..., az://..., memory://
    ```

=== "Rust"

    ```rust
    use completr::Database;

    let database = Database::open("./data", Vec::<(String, String)>::new()).await?;
    ```

The database API is async in Rust and runs on Tokio. From synchronous code, `completr::block_on` runs one
of its futures on completr's internal runtime; do not call it from async code.

## Credentials

**S3** resolves credentials like `boto3`: environment variables, shared config and credentials files and
profiles, SSO, web identity, ECS and IMDS. The region is taken from the configuration, or discovered from
the bucket. Explicit `options` override all of them:

<!-- skip-test -->
```python
db = completr.connect(
    "s3://my-bucket/completions",
    options={"aws_region": "eu-central-1", "aws_access_key_id": "...", "aws_secret_access_key": "..."},
)
```

**GCS and Azure** are configured through `options`, using
[`object_store`](https://docs.rs/object_store) configuration keys such as `google_service_account`,
`google_application_credentials`, `azure_storage_account_name` and `azure_storage_account_key`.
Environment variables are not read for them. Without explicit credentials, GCS uses the gcloud application
default credentials file or the instance metadata server.

In Rust, the S3 credential chain requires the `aws-credentials` feature, and options are the key-value
pairs passed to `Database::open`.

### Object tags

`tags` puts the same tags on every object completr writes: segments, manifests, change sets and leases.
Use them where a bucket selects lifecycle rules, replication or cost allocation by tag:

<!-- skip-test -->
```python
db = completr.connect("s3://bucket/completions", tags={"LifecycleRule": "KeepForever"})
```

Pick a rule that never expires or archives objects. completr deletes what it no longer needs itself, with
[cleanup](#cleanup), while a segment still in use can be months old. S3 needs `s3:PutObjectTagging` for
tagged writes. In Rust the option is `Database::with_tags`, and on the command line `--tag KEY=VALUE`.

## Disk cache

For object stores, pass `cache_dir` to keep downloaded segments on local disk. They are then
memory-mapped rather than held in process memory, and a restarted process reuses them without
downloading them again. Segment files are immutable, so a cached copy never needs revalidation. An engine
removes cached files that no current index uses after each sync. `cache_dir` has no effect on local
databases, whose files are memory-mapped directly.

<!-- skip-test -->
```python
db = completr.connect("s3://my-bucket/completions", cache_dir="/var/cache/completr")
```

In Rust, use `Database::with_cache_dir`.

## Transactions

A transaction changes one or more indexes and commits them atomically, by writing the next manifest
create-only. `commit()` builds and uploads the segments, commits, and returns the new manifest once it is
durable.

=== "Python"

    ```python
    txn = db.begin()
    txn.append("songs", [{"id": f"s{i}", "text": f"song {i}", "popularity": 0.5} for i in range(10_000)])
    manifest = txn.commit()
    print(manifest["version"], list(manifest["namespaces"]["default"]))
    ```

=== "Rust"

    ```rust
    use completr::Document;

    let mut txn = database.begin().await?;
    txn.append_documents(
        "songs",
        (0..10_000).map(|i| Document::keyed(format!("s{i}"), format!("song {i}"), 0.5)),
        [],
    )?;
    txn.commit().await?;
    ```

```text
1 ['songs']
```

| `Transaction` method | Does |
|---|---|
| `append(index, documents, deletes=(), vectors=None)` | Adds a segment of upserts and deletes to an index, creating the index if needed. |
| `overwrite(index, documents, vectors=None)` | Replaces the whole index with one base segment. |
| `drop_index(index)` | Removes an index. |
| `set_max_score(index, max_score)` | Pins the score that normalises to 1.0. |
| `set_metadata(key, value)` | Sets or, with `None`, removes a string in the manifest. |
| `strict(True)` | Fails with `ConflictError` instead of rebasing when an index it changes was changed since `read_version`. |
| `max_retries(n)` | Retries of the create-only manifest write before giving up with `ConflictError` (32). |
| `commit()` | Builds and uploads the segments, then commits; returns the new manifest as a dict. |

Any number of processes may commit at once. A commit that loses the race for a version rebases onto the
newer one and retries, with backoff. Commits to different indexes, and to different documents of one
index, never conflict; a later commit wins per document id. In strict mode, a commit fails instead:

```python
from completr import ConflictError

txn = db.begin()
txn.strict()
other = db.begin()
other.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
other.commit()

txn.overwrite("songs", [{"id": "killer", "text": "Killer Queen – Queen"}])
try:
    txn.commit()
except ConflictError as error:
    print(error)
```

```text
commit conflict: songs in namespace default changed since version 1
```

Each commit writes one segment per changed index, so batch documents where you can: one append of 10,000
documents is much cheaper than 10,000 appends of one. For many processes writing small changes at a high
rate, use an inbox and an [ingestor](writers.md) instead.

`db.versions()`, `db.latest_version()`, `db.index_names(version)` and `db.manifest(version)` read the
history. `db.begin(version)` starts a transaction from an older version.

## Snapshots

`db.open_index(name, version=None)` returns one version of one index as an `Index`, for scripts and tests:

```python
snapshot = db.open_index("songs", version=1)
print(len(snapshot), snapshot.get("billie"))
```

```text
10000 None
```

## Serving

A serving process takes an engine from the database. `db.engine()` loads every index now. Its first query
then starts a background thread that loads the newest version every `sync_every` seconds (5 by default). A
sync lists only manifests newer than the engine's version, downloads only segments it does not hold,
rebuilds only changed indexes, and publishes them atomically. Completions never wait for a sync, and a
search in progress keeps the version it started with.

=== "Python"

    ```python
    engine = db.engine(sync_every=2.0)
    print(engine.version, engine.names())
    print([s.text for s in engine.complete("bill", ["songs"])])   # starts the background sync
    ```

=== "Rust"

    ```rust
    use std::{sync::Arc, time::Duration};
    use completr::{Engine, IndexOptions, Replica};

    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database.clone(), IndexOptions::default()));
    replica.sync(&engine).await?;
    let follower = replica.follow(&engine, Duration::from_secs(2));
    let hits = engine.complete(&["songs"], "bill", 10)?;
    ```

```text
2 ['songs']
['Billie Jean – Michael Jackson']
```

- A failed sync keeps serving the version the engine has, and tries again after `sync_every`.
  `engine.sync_status` holds `synced_at` (seconds since the Unix epoch) and `error` of the last background
  sync, or `None` before the first one. See [Observability](observability.md).
- `engine.sync()` loads the latest version now, and returns it, or `None` when nothing changed. With
  `sync_every=None`, nothing syncs in the background and `sync()` is the only way to load new versions.
- A sync switches indexes one group at a time, so a query over several [layers](layers.md) can briefly
  see one index at a newer version than another.
- An engine made with `Engine()` and filled with `publish` does not follow a database.

In Rust, a `Replica` publishes into an `Engine`, and `Replica::follow` syncs it from a background thread
until the returned `Follower` is dropped; `follower.status()` returns its `synced_at_ms` and `error`. In
Python, `db.engine()` combines the two, and `Replica(database, engine)` is also available.

### Forking servers

completr's database calls run on a runtime shared by the process, and that runtime does not survive
`fork()`. With a server that imports your app and then forks workers (gunicorn's `--preload`, or any
pre-forking server), open databases and engines in each worker after it forks, not before. Servers that
start each worker as a fresh process, such as uvicorn with `--workers`, need nothing special.

## Compaction

Every commit adds a small segment per changed index, and many small segments slow queries down.
`db.compact(index)` merges them tier by tier: `fanout` (4) same-level segments merge into one of the next
level. An index is rebuilt into a single base when more than `max_hidden_fraction` (0.25) of its
documents are superseded or deleted, or when there are more than `max_segments` (16) segments and no
tiered merge is available. It returns the new version, or `None` if nothing was due.

```python
for i in range(5):
    txn = db.begin()
    txn.append("songs", [{"id": f"extra-{i}", "text": f"Extra {i}"}])
    txn.commit()
print(len(db.manifest()["namespaces"]["default"]["songs"]["segments"]))
db.compact("songs", until_done=True)
print(len(db.manifest()["namespaces"]["default"]["songs"]["segments"]))
```

```text
7
2
```

Transactions you commit directly are not compacted; run `db.compact` after them, from a worker or a cron
job. An [ingestor](writers.md) compacts the indexes it touches, and [collections](collections.md) compact
themselves after writes.

## Cleanup

Old versions and the segments only they use stay in storage until you remove them. `db.cleanup` deletes
old manifests, keeping the newest `keep_versions` (10), and segment files that no retained version
references. It only touches objects older than `older_than_seconds` (3600), measured on the store's clock,
so engines that are still loading a recent version are not affected. Nothing runs cleanup automatically;
schedule it, for example hourly.

```python
print(db.cleanup(keep_versions=10, older_than_seconds=3600))
```

```text
{'versions_removed': 0, 'segments_removed': 0, 'bytes_removed': 0}
```

Keep `older_than_seconds` well above `sync_every` and the time an engine needs to load a version.

## Leases

A lease is an expiring lock stored in the database, for jobs that only one process should run at a time.
`acquire_lease(name, owner, ttl_seconds)` returns a `Lease`, or `None` while another owner holds it.
Its `generation` grows with every new holder, so it serves as a fencing token.

```python
lease = db.acquire_lease("nightly-rebuild", "worker-1", ttl_seconds=60.0)
print(lease.generation, db.acquire_lease("nightly-rebuild", "worker-2", ttl_seconds=60.0))
lease.renew(60.0)   # False once the lease was lost
lease.release()
```

```text
1 None
```

The [ingestor](writers.md) uses a lease named `ingestor`.

## Roles

| Role | Runs | Where |
|---|---|---|
| Serving | `engine = db.engine(sync_every=...)` once, then `engine.complete(...)` per request | Every process that answers completions |
| Writing | `txn = db.begin()`, `txn.append(...)`, `txn.commit()`; or `db.submit(changes)` with an [ingestor](writers.md) | Any process that changes data: an API handler, a queue consumer, a batch job |
| Maintenance | `db.compact(...)` after direct commits; `db.cleanup(...)` on a schedule | A cron job or a worker |

| Task | Suggested schedule | Command-line equivalent |
|---|---|---|
| Bulk load | When the source data changes | `completr <url> import <index> <file>` |
| Compaction | After bulk loads and direct commits | `completr <url> compact <index>` |
| Cleanup | Hourly or daily | `completr <url> cleanup --keep-versions 10 --older-than-seconds 3600` |

For asyncio services, see [asyncio](asyncio.md) and the
[FastAPI example](https://github.com/sayef/completr/tree/main/examples/fastapi).

## Memory

A segment is memory-mapped and costs roughly 100 bytes per short title, plus about 136 bytes per 256-d
vector at 4 bits. Hundreds of thousands to a few million documents per index fit comfortably on one
machine.

While an engine switches versions, the old and new segments of an index are both mapped. The engine
switches one [namespace](layers.md#namespaces) at a time and releases replaced segments before it loads the
next, so serving memory holds at most one namespace twice, never the whole database. Splitting a large
database into namespaces, such as one per locale, bounds that peak:

```python
with db.namespace("en").begin() as txn:
    txn.append("shared", [{"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95}])
    txn.append("radio", [{"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen", "popularity": 0.95}])

engine = db.engine()   # "shared" and "radio" of "en" switch together
print([(s.text, s.layer) for s in engine.namespace("en").complete("bohemian", ["shared", "radio"])])
```

```text
[('Bohemian Rhapsody (Remastered 2011) – Queen', 'radio')]
```

Other settings that affect memory and speed:

| Setting | Effect |
|---|---|
| `build_threads` (at `connect`) | Threads for building segments, in writing processes. 1 has the lowest peak memory; 0 uses all cores. |
| `short_query_cache_entries` (at `db.engine`) | Entries in each index's short-query cache (10,000). |
| `short_query_limit` (at `db.engine`) | Results kept per cached short query (100). |
| `vector_threads` (at `db.engine`) | Threads per vector query; 1 keeps queries on the caller's thread. |
