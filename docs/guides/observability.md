# Observability

strato reports what it does through [`tracing`](https://docs.rs/tracing) events: commits, replica syncs,
ingest rounds, compaction, cleanup and segment builds. Searches emit no events, so logging never costs
anything on the hot path.

## Events

| Target | Level | Message | Fields |
|---|---|---|---|
| `strato::database` | info | `committed` | `version`, `attempts`, `segments` |
| `strato::database` | warn | `commit gave up under contention` | `attempts` |
| `strato::database` | info | `replica synced` | `version`, `segments_loaded`, `ms` |
| `strato::database` | info | `compacting` | `index`, `merged`, `level`, `full`, `documents`, `ms` |
| `strato::database` | info | `cleaned up` | `versions`, `segments`, `bytes` |
| `strato::inbox` | info | `ingested` | `version`, `change_sets`, `documents` |
| `strato::segment` | debug | `built segment` | `documents`, `bytes`, `ms` |

`replica synced` is the event to watch on serving processes: `ms` is the time a sync took, and
`segments_loaded` how many segments it downloaded or mapped. On ingestors, `ingested` and `compacting`
show throughput and compaction cost.

## Python

In Python, the events reach the standard `logging` module under loggers named `strato.*`
(`strato.database`, `strato.inbox`, `strato.segment`), with the fields in the message. Configure them like
any other logger:

```python
import logging
import tempfile

import strato

logging.basicConfig(format="%(asctime)s %(name)s %(levelname)s %(message)s")
logging.getLogger("strato").setLevel(logging.INFO)

db = strato.connect(tempfile.mkdtemp())
txn = db.begin()
txn.append("products", [{"id": "kb-1", "text": "Wireless Keyboard"}])
txn.commit()
```

```text
2026-09-30 12:00:00,000 strato.database INFO committed version=1 attempts=1 segments=1
```

Set the `strato` logger to `DEBUG` to include segment builds.

!!! note "Configure logging before the first strato call"
    The bridge to `logging` caches each logger's level the first time that logger is used. Set levels at
    start-up, before strato logs anything; a level changed later may not take effect for loggers already
    used.

## Rust

In Rust, install any `tracing` subscriber. For example, with `tracing-subscriber` and its `env-filter`
feature:

```rust
tracing_subscriber::fmt()
    .with_env_filter("strato=info")
    .init();
```

## Command line

The `strato` tool logs to stderr at `strato=info`. Set `STRATO_LOG` to change the filter:

```sh
STRATO_LOG=strato=debug strato s3://my-bucket/completions ingest
STRATO_LOG=strato=warn strato s3://my-bucket/completions cleanup
```
