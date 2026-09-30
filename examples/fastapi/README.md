# FastAPI example

A completion API over a completr database. The app opens the database with `completr.connect_async`, serves
`GET /complete` from `AsyncDatabase.engine()`, and runs `await engine.sync()` every few seconds in a
background task, so new versions appear without a restart.

```sh
pip install -r requirements.txt

# Load some documents (or use `completr ./completions import products products.jsonl`).
python -c '
import completr
db = completr.connect("./completions")
txn = db.begin()
txn.append("products", [
    {"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9, "contexts": ["peripherals"]},
    {"id": "ms-1", "text": "Wireless Mouse", "popularity": 0.7, "contexts": ["peripherals"]},
])
txn.commit()
'

COMPLETR_URL=./completions uvicorn app:app
curl 'http://127.0.0.1:8000/complete?q=wirel&limit=5'
```

```json
{"version": 1, "suggestions": [
  {"id": "kb-1", "text": "Wireless Keyboard", "kind": "prefix", "score": 0.5954, "highlights": [[0, 5]]},
  {"id": "ms-1", "text": "Wireless Mouse", "kind": "prefix", "score": 0.4935, "highlights": [[0, 5]]}
]}
```

| Variable | Default | Meaning |
|---|---|---|
| `COMPLETR_URL` | `./completions` | Database URL: a local path, `s3://`, `gs://` or `az://`. |
| `COMPLETR_INDEX` | `products` | Index to complete from. |
| `COMPLETR_CACHE_DIR` | none | Local directory for downloaded segments. |
| `COMPLETR_SYNC_SECONDS` | `5` | Seconds between syncs. |

`/complete` takes `q`, `limit` (1 to 50) and repeated `contexts` parameters. Updates come from other
processes: submit change sets with `db.submit(...)` and run an ingestor outside the API processes, for
example `completr ./completions ingest`. See the
[serverless deployment guide](https://sayef.github.io/completr/guides/serverless/).
