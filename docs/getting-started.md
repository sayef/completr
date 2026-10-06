# Getting started

This page builds an index in memory and completes queries with it. It then stores the same documents in a
database on disk, serves them from an engine that keeps itself up to date, and moves the database to
object storage.

## Install

=== "Python"

    ```sh
    pip install completr
    ```

    The wheel is abi3 for CPython 3.11 and newer, and includes every storage backend. It releases the GIL
    while it searches and builds.

=== "Rust"

    ```sh
    cargo add completr --features store                       # databases on local disk and in memory
    cargo add completr --features aws-credentials,gcp,azure   # plus S3, GCS and Azure
    cargo add tokio --features macros,rt-multi-thread         # the database API is async
    ```

    | Cargo feature | Adds |
    |---|---|
    | none | The engine: `Segment`, `SegmentWriter`, `Index`, `Engine` |
    | `store` | `Database`, `Transaction`, `Replica`, `Lease`, `ChangeSet`, `Ingestor`, local and in-memory stores, and the [collections](guides/collections.md) layer: `connect`, `Client`, `Collection` |
    | `aws` / `gcp` / `azure` | S3, Google Cloud Storage, Azure Blob Storage |
    | `aws-credentials` | S3 credentials through the AWS SDK chain |

## An index in memory

A document has an id, a text and a popularity weight, and optionally synonyms, abbreviations, contexts and
an embedding. `Index.from_documents` builds an index from documents in memory, with no database.

=== "Python"

    ```python
    from completr import Index

    docs = [
        {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95, "synonyms": ["is this the real life"]},
        {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
        {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
        {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75, "abbreviations": ["RHCP"]},
    ]
    index = Index.from_documents(docs)

    for query in ["danc", "RHCP", "bohemain rapsody", "queen"]:
        print(query, [(s.text, s.kind, round(s.score, 3)) for s in index.complete(query, limit=3)])
    ```

=== "Rust"

    ```rust
    use completr::{Document, Index};

    let index = Index::from_documents([
        Document::keyed("bohemian", "Bohemian Rhapsody – Queen", 0.95)
            .with_synonym("is this the real life"),
        Document::keyed("dancing", "Dancing Queen – ABBA", 0.85),
        Document::keyed("dark", "Dancing in the Dark – Bruce Springsteen", 0.7),
        Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75)
            .with_abbreviation("RHCP"),
    ])?;

    for query in ["danc", "RHCP", "bohemain rapsody", "queen"] {
        for s in index.complete(query, 3) {
            println!("{query}: {} {} {:.3}", s.text, s.kind.as_str(), s.score);
        }
    }
    ```

```text
danc [('Dancing Queen – ABBA', MatchKind.PREFIX, 0.547), ('Dancing in the Dark – Bruce Springsteen', MatchKind.PREFIX, 0.462)]
RHCP [('Under the Bridge – Red Hot Chili Peppers', MatchKind.ABBREVIATION, 0.486)]
bohemain rapsody [('Bohemian Rhapsody – Queen', MatchKind.FUZZY, 0.186)]
queen [('Bohemian Rhapsody – Queen', MatchKind.INFIX, 0.356), ('Dancing Queen – ABBA', MatchKind.INFIX, 0.329)]
```

## Reading suggestions

Each suggestion carries everything a completion box needs to render it.

| Field | Meaning |
|---|---|
| `id` | The id you gave: an `int`, or the string key. In Rust, `id` is the 64-bit id and `key` the string. |
| `text` | The document's text, to display. |
| `kind` | How it matched: `exact`, `prefix`, `abbreviation`, `infix`, `fuzzy`, `synonym` or `semantic`. |
| `score` | The ranking score, normalised so the strongest matches score around 1. |
| `highlights` | Ranges of `text` that matched the query. Python gives `(start, end)` character offsets; Rust gives byte ranges. |
| `layer` | The index the suggestion came from, in a search over [layers](guides/layers.md); `None` for a single index. |

=== "Python"

    ```python
    top = index.complete("danc")[0]
    print(top.id, top.text, top.kind, round(top.score, 3), top.layer)
    print([top.text[a:b] for a, b in top.highlights])
    ```

=== "Rust"

    ```rust
    let top = &index.complete("danc", 10)[0];
    let matched: Vec<&str> = top.highlights.iter().map(|r| &top.text[r.clone()]).collect();
    println!("{:?} {} {:?}", top.key, top.text, matched);
    ```

```text
dancing Dancing Queen – ABBA prefix 0.547 None
['Danc']
```

Synonyms are searched separately, with `complete_aliases`. Its suggestions carry the document's text, not
the synonym:

```python
print(index.complete("is this the real life"))
print(index.complete_aliases("is this the real life"))
```

```text
[]
[AliasSuggestion(id='bohemian', text='Bohemian Rhapsody – Queen', score=0.6637)]
```

## A database

A database keeps named indexes in a directory or a bucket. `completr.connect(url)` opens one, and creates a
local one if it does not exist. A transaction changes one or more indexes and commits a new **version** of
the database. An engine from the database serves its indexes.

=== "Python"

    ```python
    import completr

    db = completr.connect("./data")
    txn = db.begin()
    txn.append("songs", docs)   # creates the index "songs"
    print(txn.commit()["version"])

    engine = db.engine()
    print([(s.text, s.kind) for s in engine.complete("danc", ["songs"])])
    ```

=== "Rust"

    ```rust
    use std::{sync::Arc, time::Duration};
    use completr::{Database, Engine, IndexOptions, Replica};

    let database = Database::open("./data", Vec::<(String, String)>::new()).await?;
    let mut txn = database.begin().await?;
    txn.append_documents("songs", docs, [])?;
    println!("{}", txn.commit().await?.version);

    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database.clone(), IndexOptions::default()));
    replica.sync(&engine).await?;
    let follower = replica.follow(&engine, Duration::from_secs(5));   // syncs until dropped
    for layered in engine.complete(&["songs"], "danc", 10) {
        println!("{} {}", layered.suggestion.text, layered.suggestion.kind.as_str());
    }
    ```

```text
1
[('Dancing Queen – ABBA', MatchKind.PREFIX), ('Dancing in the Dark – Bruce Springsteen', MatchKind.PREFIX)]
```

`engine.complete` takes the query and a list of index names to search. With more than one name, later
indexes override earlier ones per document id; see [Layers](guides/layers.md).

## Staying up to date

Any number of processes can open the same database, and any of them can commit. An engine from
`db.engine()` loads every index when it is created, and then keeps itself on the latest version: its
first query starts a background thread that syncs every `sync_every` seconds (5 by default). A sync
downloads only segments the engine does not hold, and queries never wait for it.

```python
import time

engine = completr.connect("./data").engine(sync_every=1.0)   # in a serving process
print(engine.complete("blind", ["songs"]))                    # starts the background sync

txn = db.begin()                                               # in another process
txn.append("songs", [{"id": "blinding", "text": "Blinding Lights – The Weeknd", "popularity": 0.9}])
txn.commit()

time.sleep(1.5)
print(engine.complete("blind", ["songs"]), engine.version)
print(engine.sync_status["error"])
```

```text
[]
[Suggestion(id='blinding', text='Blinding Lights – The Weeknd', score=0.5666, kind=MatchKind.PREFIX, layer='songs')] 2
None
```

- `engine.version` is the version the engine serves.
- `engine.sync_status` is `None` until the first background sync, and then holds `synced_at` (seconds
  since the Unix epoch) and `error`. A failed sync keeps serving the version the engine has.
- `engine.sync()` syncs now, and returns the new version or `None`. With `sync_every=None`, nothing syncs
  in the background, and `sync()` is the only way to load new versions.

In Rust, `Replica::follow` starts the same thread, and stops it when the returned `Follower` is dropped.

Commits are create-only writes to storage, so concurrent writers do not corrupt each other: a commit that
loses a race retries on top of the newer version.

## A database in object storage

The same code works on S3, Google Cloud Storage and Azure Blob Storage: only the URL changes. Pass
`cache_dir` to keep downloaded segments on local disk, so a restarted process does not download them again.

<!-- skip-test -->
```python
db = completr.connect("s3://my-bucket/completions", cache_dir="/var/cache/completr")
# or "gs://bucket/prefix", "az://container/prefix"
```

S3 credentials resolve like `boto3`'s. [Serverless deployment](guides/serverless.md) covers credentials,
compaction, cleanup and what each process does.

## Or use collections

`completr.Client` is a convenience layer over a database: named collections that keep their settings,
compact themselves after writes, report `stats()`, and complete synonyms in the same call. A collection is
an index of the same name, so both work on the same database.

```python
client = completr.Client("./data")
print(client.collections(), [s.text for s in client["songs"].complete("blind")])
```

```text
['songs'] ['Blinding Lights – The Weeknd']
```

See [Collections](guides/collections.md).

## Next steps

- [Concepts](concepts.md): indexes, segments, engines, databases and versions.
- [Documents](guides/documents.md): every field, string ids, table input, segments and offline builds.
- [Completion](guides/completion.md): match kinds, highlights, contexts, synonyms and ranking.
- [Serverless deployment](guides/serverless.md): storage, credentials, transactions, engines, compaction
  and cleanup.
- [Many writers](guides/writers.md): change sets and a lease-elected ingestor.
- [asyncio](guides/asyncio.md): the same API in FastAPI and other asyncio services.
