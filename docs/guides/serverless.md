# Serverless deployment

A completr deployment has no completr server. It has a bucket or directory (the database), your serving
processes with an engine in each, and one or more processes that run an ingestor. This page covers
storage, credentials, the roles each process plays, maintenance schedules and memory.

!!! warning "Run ingestors outside serving processes"
    Building segments and compacting indexes use far more memory and CPU than serving. Build and
    compaction peaks belong to the ingestor, so run ingestors in separate processes (a worker, a cron job
    or `completr ingest`), not inside the processes that answer completion requests.

## Storage backends

`completr.connect(url)` opens a database. Local directories are created if they do not exist.

| URL | Backend |
|---|---|
| `/path/to/db`, `file:///path/to/db` | Local file system. Segments are memory-mapped in place. |
| `s3://bucket/prefix` | Amazon S3 or an S3-compatible store (set `aws_endpoint` for the latter). |
| `gs://bucket/prefix` | Google Cloud Storage. |
| `az://container/prefix` | Azure Blob Storage. |
| `memory:///name` | In memory, for tests. Each `connect` gets a fresh, empty store. |

Every backend supports the create-only writes that commits and leases rely on: `If-None-Match` on object
stores, exclusive create on a file system.

## Credentials

**S3** resolves credentials like `boto3`: environment variables, shared config and credentials files and
profiles, SSO, web identity, ECS and IMDS. The region is taken from the configuration, or discovered from
the bucket. Explicit `options` override all of them:

```python
import completr

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

In Rust, the S3 credential chain requires the `aws-credentials` feature, and options are passed as
key-value pairs to `Database::open(url, options)`.

## Disk cache

For object stores, pass `cache_dir` to keep downloaded segments on local disk. They are then
memory-mapped rather than held in process memory, and a restarted process reuses them without
downloading them again. Segment files are immutable, so a cached copy never needs revalidation. Replicas
remove cached files that no current index uses after each sync. `cache_dir` has no effect on local
databases, whose files are memory-mapped directly.

```python
db = completr.connect("s3://my-bucket/completions", cache_dir="/var/cache/completr")
```

## Roles

| Role | Runs | Where |
|---|---|---|
| Serving | `engine = db.engine()`, then `engine.sync()` every few seconds; `engine.complete(...)` per request | Every process that answers completions |
| Submitting | `db.submit(changes)` | Any process that changes data: an API handler, a queue consumer, a batch job |
| Ingesting | `Ingestor(db, owner).run_once()` in a loop | One or more worker processes, outside serving processes |
| Maintenance | `db.cleanup(...)` on a schedule | A cron job or the ingestor's worker |

### Serving processes

A serving process opens the database, creates an engine, and syncs it periodically in the background.
`sync()` lists only manifests newer than the current version, downloads only segments it does not hold,
and publishes the changed indexes atomically; it returns the new version, or `None` when nothing changed.
Completions never wait for a sync.

```python
import threading
import time

db = completr.connect("s3://my-bucket/completions", cache_dir="/var/cache/completr")
engine = db.engine()

def follow(interval=5.0):
    while True:
        time.sleep(interval)
        engine.sync()

threading.Thread(target=follow, daemon=True).start()
```

For asyncio services, see [asyncio](asyncio.md) and the
[FastAPI example](https://github.com/sayef/completr/tree/main/examples/fastapi).

### Submitting changes

`db.submit(changes)` writes a change set to the database's `_inbox/` and returns its id. It does not wait
for a commit. Change sets from one process apply in submission order, and later change sets win per
document id.

```python
changes = completr.ChangeSet()
changes.upsert("products", [{"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9}])
changes.delete("products", ["p42"])
change_set_id = db.submit(changes)
print(db.pending_change_sets())
```

For bulk loads and full rebuilds, commit a transaction directly instead:

```python
txn = db.begin()
txn.overwrite("products", [{"id": f"p{i}", "text": f"product {i}"} for i in range(10_000)])
txn.commit()
```

### Running ingestors

An `Ingestor` commits pending change sets while it holds the database's `ingestor` lease. Every round
takes or renews the lease, folds up to `max_change_sets` change sets into one commit, deletes them from
the inbox, and compacts the indexes it touched. You may run several ingestors for availability: only the
lease holder acts, and another takes over within `lease_ttl_seconds` (30 by default) when it stops.

<!-- skip-test -->
```python
import time

ingestor = completr.Ingestor(db, "worker-1", lease_ttl_seconds=30.0)
try:
    while True:
        step = ingestor.run_once()   # {'step': 'standby' | 'idle' | 'committed', ...}
        if step["step"] != "committed":
            time.sleep(1.0)
finally:
    ingestor.release()
```

| `step` | Meaning |
|---|---|
| `standby` | Another ingestor holds the lease. |
| `idle` | The inbox was empty. |
| `committed` | Change sets were committed; the result also has `version`, `change_sets` and `documents`. |

Commits carry the lease generation as a fencing token, so an ingestor that lost its lease cannot commit,
and a change set left over after a crash is never applied twice. Call `run_once()` well within
`lease_ttl_seconds`: it renews the lease once a third of the TTL has passed, and loses it after the TTL. The command-line tool runs the same loop:
`completr s3://my-bucket/completions ingest --interval 1`.

## Compaction and cleanup

**Compaction** merges small delta segments tier by tier: `fanout` (4) same-level segments merge into one
of the next level. An index is rebuilt into a single base when more than `max_hidden_fraction` (0.25) of
its documents are superseded or deleted, or when there are more than `max_segments` (16) segments and no
tiered merge is available. The ingestor compacts automatically after each commit (disable with
`compact=False`). Transactions you commit directly are not compacted; run `db.compact` after them:

```python
db.compact("products", until_done=True)   # returns the new version, or None if nothing was due
```

**Cleanup** deletes old manifests, keeping the newest `keep_versions`, and segment files that no retained
version references. It only touches objects older than `older_than_seconds`, measured on the store's
clock, so replicas that are still loading a recent version are not affected. Nothing runs cleanup
automatically; schedule it, for example hourly:

```python
print(db.cleanup(keep_versions=10, older_than_seconds=3600))
# {'versions_removed': ..., 'segments_removed': ..., 'bytes_removed': ...}
```

| Task | Suggested schedule | Command-line equivalent |
|---|---|---|
| Ingest | Continuously, one-second rounds | `completr <url> ingest --interval 1` |
| Compact after direct transactions | After each bulk load | `completr <url> compact <index>` |
| Cleanup | Hourly or daily | `completr <url> cleanup --keep-versions 10 --older-than-seconds 3600` |

Keep `older_than_seconds` well above your sync interval and the time a replica needs to load a version.

## Memory

A segment is memory-mapped and costs roughly 100 bytes per short title, plus about 136 bytes per 256-d
vector at 4 bits. Hundreds of thousands to a few million documents per index fit comfortably on one
machine. While a replica switches versions, the old and new segments of an index are both mapped.

When a database holds many indexes, pass `group_separator` to `db.engine()`. The replica then switches
indexes group by group, grouping by the part after the last separator, and releases replaced segments
before it loads the next group. Serving memory holds at most one group twice, never the whole database.

```python
engine = db.engine(group_separator="/")   # "acme/en", "shared/en": all "en" indexes switch together
```

Other settings that affect serving memory and speed:

| Setting | Effect |
|---|---|
| `short_query_cache_entries` | Entries in each index's short-query cache (10,000). |
| `short_query_limit` | Results kept per cached short query (100). |
| `vector_threads` | Threads per vector query; 1 keeps queries on the caller's thread. |
| `build_threads` (at `connect`) | Threads for building segments, on the ingestor. 1 has the lowest peak memory; 0 uses all cores. |
