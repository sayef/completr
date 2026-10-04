# Observability

An engine reports the version it serves and how its background sync went. Beyond that, completr reports
what it does through [`tracing`](https://docs.rs/tracing) events: commits, syncs, ingest rounds,
compaction, cleanup and segment builds. Searches emit no events, so logging never costs anything on the
hot path.

## Engine status

```python
import time

import completr

db = completr.connect("./data")
txn = db.begin()
txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
txn.commit()

engine = db.engine(sync_every=0.5)
print(engine.sync_status)
engine.complete("bill", ["songs"])   # starts the background sync
time.sleep(1)
print(engine.sync_status)
print(engine.version, db.latest_version())
```

```text
None
{'synced_at': 1791145251.237, 'error': None}
1 1
```

| Value | Meaning |
|---|---|
| `engine.version` | The database version the engine serves. |
| `engine.sync_status` | `None` before the first background sync; then `synced_at`, when the last one finished in seconds since the Unix epoch, and `error`, its error or `None`. |
| `db.latest_version()` | The latest version in storage. It reads storage, so call it from health checks, not per request. |

A serving process is healthy when `sync_status["error"]` is `None` and `synced_at` is recent. A failed
sync keeps serving the version the engine has; a growing gap between `db.latest_version()` and
`engine.version` means it is falling behind.

In Rust, `Follower::status` returns `FollowStatus { synced_at_ms, error }`, and `Replica::version` the
version served.

A [collection](collections.md#stats) reports the same and more, including segment counts and the last
merge error, through `stats()`.

## Events

| Target | Level | Message | Fields |
|---|---|---|---|
| `completr::database` | info | `committed` | `version`, `attempts`, `segments` |
| `completr::database` | warn | `commit gave up under contention` | `attempts` |
| `completr::database` | info | `replica synced` | `version`, `segments_loaded`, `ms` |
| `completr::database` | info | `compacting` | `index`, `merged`, `level`, `full`, `documents`, `ms` |
| `completr::database` | info | `cleaned up` | `versions`, `segments`, `bytes` |
| `completr::inbox` | info | `ingested` | `version`, `change_sets`, `documents` |
| `completr::segment` | debug | `built segment` | `documents`, `bytes`, `ms` |

`replica synced` is the event to watch on serving processes: `ms` is the time a sync took, and
`segments_loaded` how many segments it downloaded or mapped. On writing processes, `committed` and
`compacting` show write and merge cost; on [ingestors](writers.md), `ingested` shows throughput.

## Python

In Python, the events reach the standard `logging` module under loggers named `completr.*`
(`completr.database`, `completr.inbox`, `completr.segment`), with the fields in the message. Configure
them like any other logger:

```python
import logging

import completr

logging.basicConfig(format="%(asctime)s %(name)s %(levelname)s %(message)s")
logging.getLogger("completr").setLevel(logging.INFO)

db = completr.connect("./logged")
txn = db.begin()
txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson"}])
txn.commit()
engine = db.engine()
```

```text
2026-10-04 22:20:50,660 completr.database INFO committed version=1 attempts=1 segments=1
2026-10-04 22:20:50,663 completr.database INFO replica synced version=1 segments_loaded=1 ms=0
```

Set the `completr` logger to `DEBUG` to include segment builds.

!!! note "Configure logging before the first completr call"
    The bridge to `logging` caches each logger's level the first time that logger is used. Set levels at
    start-up, before completr logs anything; a level changed later may not take effect for loggers already
    used.

## Rust

In Rust, install any `tracing` subscriber. For example, with `tracing-subscriber` and its `env-filter`
feature:

```rust
tracing_subscriber::fmt()
    .with_env_filter("completr=info")
    .init();
```

## Command line

The `completr` tool logs to stderr at `completr=info`. Set `COMPLETR_LOG` to change the filter:

```sh
COMPLETR_LOG=completr=debug completr s3://my-bucket/completions ingest
COMPLETR_LOG=completr=warn completr s3://my-bucket/completions cleanup
```
