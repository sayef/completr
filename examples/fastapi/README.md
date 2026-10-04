# FastAPI example

A completion API over a completr database. The app opens the database once with `completr.connect`, takes
an engine from it with `db.engine(sync_every=...)`, and serves `GET /complete` from one index. The engine
loads new versions by itself in a background thread every `COMPLETR_SYNC_SECONDS`, so updates appear
without a restart and without a sync loop.

```sh
pip install -r requirements.txt

# Load some documents (or use `completr ./completions import songs songs.jsonl`).
python -c '
import completr
db = completr.connect("./completions")
txn = db.begin()
txn.append("songs", [
    {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85, "contexts": ["pop", "disco"]},
    {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7, "contexts": ["rock"]},
])
txn.commit()
'

COMPLETR_URL=./completions uvicorn app:app
curl 'http://127.0.0.1:8000/complete?q=danc&limit=5'
```

```json
[
  {"id": "dancing", "text": "Dancing Queen – ABBA", "kind": "prefix", "score": 0.5946, "highlights": [[0, 4]]},
  {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "kind": "prefix", "score": 0.5024, "highlights": [[0, 4]]}
]
```

| Variable | Default | Meaning |
|---|---|---|
| `COMPLETR_URL` | `./completions` | Database URL: a local path, `s3://`, `gs://` or `az://`. |
| `COMPLETR_INDEX` | `songs` | Index to complete from. |
| `COMPLETR_CACHE_DIR` | none | Local directory for downloaded segments. |
| `COMPLETR_SYNC_SECONDS` | `5` | Seconds between background syncs. |

`/complete` takes `q`, `limit` (1 to 50) and repeated `contexts` parameters. `/health` returns the version
the engine serves and its last background sync (`synced_at`, `error`).

Updates come from other processes: any process can commit a transaction to the same database, for example
a worker that runs `txn.append("songs", ...)` when the catalogue changes. Keep large writes out of the
API processes.

The engine is opened at import time. Each uvicorn worker imports the app on its own, so each has its own
engine. With a server that imports the app and then forks workers (gunicorn's `--preload`), open the
database in each worker instead, because completr's runtime does not survive `fork()`. See the
[serverless deployment guide](https://completr.pages.dev/guides/serverless/).
