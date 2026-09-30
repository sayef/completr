# Observability

completr reports what it does through [`tracing`](https://docs.rs/tracing) events: commits, replica syncs,
ingest rounds, compaction, cleanup and segment builds. Searches emit no events, so logging never costs
anything on the hot path.

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
`segments_loaded` how many segments it downloaded or mapped. On ingestors, `ingested` and `compacting`
show throughput and compaction cost.

## Python

In Python, the events reach the standard `logging` module under loggers named `completr.*`
(`completr.database`, `completr.inbox`, `completr.segment`), with the fields in the message. Configure them like
any other logger:

```python
import logging
import tempfile

import completr

logging.basicConfig(format="%(asctime)s %(name)s %(levelname)s %(message)s")
logging.getLogger("completr").setLevel(logging.INFO)

db = completr.connect(tempfile.mkdtemp())
txn = db.begin()
txn.append("products", [{"id": "kb-1", "text": "Wireless Keyboard"}])
txn.commit()
```

```text
2026-09-30 12:00:00,000 completr.database INFO committed version=1 attempts=1 segments=1
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
