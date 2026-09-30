# Command line

The `strato` tool operates a database from a terminal or a cron job: import documents, try completions,
inspect versions, compact, clean up and run an ingestor.

```sh
cargo install strato-cli
```

Every command takes the database URL or path first, or reads it from `STRATO_URL`. The tool supports
local paths, `s3://`, `gs://` and `az://` URLs. S3 credentials resolve through the standard AWS chain; the
tool takes no storage options on the command line.

```sh
strato s3://my-bucket/completions import products products.jsonl   # JSON Lines documents
strato s3://my-bucket/completions complete products "wirel" --contexts peripherals
strato s3://my-bucket/completions inspect                          # versions, indexes, sizes
strato s3://my-bucket/completions compact products
strato s3://my-bucket/completions cleanup --keep-versions 10
strato s3://my-bucket/completions ingest --interval 1               # run an ingestor
```

## Commands

| Command | Does | Options |
|---|---|---|
| `inspect` | Prints the latest version, the number of retained versions, and per index its segments, stored documents and size. | `--json` prints the latest manifest as JSON. |
| `complete <index> <query>` | Completes a query against the latest version and prints score, kind, id and text. | `--limit` (10), `--contexts a,b` |
| `import <index> <file>` | Imports JSON Lines documents from a file, or `-` for stdin, in one commit. | `--overwrite` replaces the whole index. |
| `compact <index>` | Compacts an index until nothing is due. | `--once` runs one step. |
| `cleanup` | Deletes old versions, and segment files no retained version references. | `--keep-versions` (10), `--older-than-seconds` (3600) |
| `ingest` | Runs an ingestor: commits submitted change sets while it holds the lease, until interrupted. | `--owner` (`strato-cli`), `--interval` seconds between rounds (1; 0 runs one round) |

## Importing documents

Each line is one JSON document with the fields `id` (a non-negative integer or a string), `text`,
`popularity`, `synonyms`, `abbreviations` and `contexts`. Blank lines are skipped. Embeddings cannot be
imported from the command line.

```json
{"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9, "contexts": ["peripherals"]}
{"id": "ms-1", "text": "Wireless Mouse", "popularity": 0.7, "synonyms": ["cordless mouse"], "contexts": ["peripherals"]}
```

```console
$ strato ./completions import products products.jsonl
imported 2 documents into products as version 1
$ strato ./completions complete products wirel
0.5954  prefix        kb-1  Wireless Keyboard
0.4935  prefix        ms-1  Wireless Mouse
```

Imports commit directly and are not compacted; run `compact` after large or repeated imports.

## Scheduling

The tool fits cron jobs and container sidecars. For example, one worker running the ingestor, and an
hourly cleanup:

```sh
# A long-running worker
STRATO_URL=s3://my-bucket/completions strato ingest --owner worker-1 --interval 1

# crontab: clean up every hour
0 * * * *  STRATO_URL=s3://my-bucket/completions strato cleanup --keep-versions 10
```

## Logging

Engine events are written to stderr, at `info` level by default. Set `STRATO_LOG` to an
[`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
directive for more or less:

```sh
STRATO_LOG=strato=debug strato ./completions import products products.jsonl
```

See [Observability](observability.md) for the events.
