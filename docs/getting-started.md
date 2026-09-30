# Getting started

This page builds a first index in memory, reads its suggestions, and then moves the same data into a
database that serving processes follow while it changes.

## Install

=== "Python"

    ```sh
    pip install completr
    ```

    The wheel is abi3 for CPython 3.11 and newer, and includes every storage backend. It releases the GIL
    while it searches and builds.

=== "Rust"

    ```sh
    cargo add completr                                        # engine
    cargo add completr --features store                       # plus databases on local disk and in memory
    cargo add completr --features aws-credentials,gcp,azure   # plus S3, GCS and Azure
    ```

    | Cargo feature | Adds |
    |---|---|
    | `store` | `Database`, `Transaction`, `Ingestor`, `Replica`, local and in-memory stores |
    | `aws` / `gcp` / `azure` | S3, Google Cloud Storage, Azure Blob Storage |
    | `aws-credentials` | S3 credentials through the AWS SDK chain |

## A first index

A document has an id, a text and a popularity weight, and optionally synonyms, abbreviations, contexts and
an embedding. `Index.from_documents` builds one segment from them and searches it.

=== "Python"

    ```python
    from completr import Index

    docs = [
        {"id": "ml", "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"]},
        {"id": "mv", "text": "Machine Vision", "popularity": 0.4},
        {"id": "ds", "text": "Data Science", "popularity": 0.7, "synonyms": ["data analytics"]},
    ]
    index = Index.from_documents(docs)

    for query in ["mach", "ML", "vison", "science"]:
        print(query, [(s.text, s.kind, round(s.score, 3)) for s in index.complete(query, limit=3)])
    ```

=== "Rust"

    ```rust
    use completr::{Document, Index};

    let index = Index::from_documents([
        Document::keyed("ml", "Machine Learning", 0.9).with_abbreviation("ML"),
        Document::keyed("mv", "Machine Vision", 0.4),
        Document::keyed("ds", "Data Science", 0.7).with_synonym("data analytics"),
    ])?;

    for query in ["mach", "ML", "vison", "science"] {
        for s in index.complete(query, 3) {
            println!("{query}: {} {} {:.3}", s.text, s.kind.as_str(), s.score);
        }
    }
    ```

```text
mach [('Machine Learning', 'prefix', 0.596), ('Machine Vision', 'prefix', 0.337)]
ML [('Machine Learning', 'abbreviation', 0.596)]
vison [('Machine Vision', 'fuzzy', 0.09)]
science [('Data Science', 'infix', 0.299)]
```

## Reading suggestions

Each suggestion carries everything a completion box needs to render it.

| Field | Meaning |
|---|---|
| `id` | The id you gave: an `int`, or the string key. In Rust, `id` is the 64-bit id and `key` the string. |
| `text` | The document's text, to display. |
| `kind` | How it matched: `exact`, `prefix`, `abbreviation`, `infix`, `fuzzy` or `semantic`. |
| `score` | The ranking score, normalised by the index's `max_score`, so the strongest matches score around 1. |
| `highlights` | Ranges of `text` that matched the query. Python gives `(start, end)` character offsets; Rust gives byte ranges. |
| `layer` | For searches through an `Engine`, the layer the suggestion came from; otherwise `None`. |

=== "Python"

    ```python
    top = index.complete("mach")[0]
    print(top.id, top.text, top.kind, round(top.score, 3))
    print([top.text[a:b] for a, b in top.highlights])   # ['Mach']
    ```

=== "Rust"

    ```rust
    let top = &index.complete("mach", 1)[0];
    let matched: Vec<&str> = top.highlights.iter().map(|r| &top.text[r.clone()]).collect();
    println!("{:?} {} {:?}", top.key, top.text, matched);   // Some("ml") Machine Learning ["Mach"]
    ```

Synonyms are searched separately, so alternative names never crowd out direct matches:

```python
print(index.complete_aliases("data an"))   # [AliasSuggestion(id='ds', text="Data Science", ...)]
```

## A database

An index built in memory is fine for tests and scripts. In production, the data lives in a **database**: a
local directory or an object-store bucket holding versioned manifests and immutable segment files.
`completr.connect` opens one, and creates it if it does not exist.

=== "Python"

    ```python
    import tempfile

    import completr

    db = completr.connect(tempfile.mkdtemp())   # or s3://bucket/prefix, gs://..., az://..., memory:///name

    txn = db.begin()
    txn.append("products", [{"id": f"p{i}", "text": f"product {i}", "popularity": 0.5} for i in range(10_000)])
    txn.commit()
    ```

=== "Rust"

    ```rust
    use completr::{Database, Document};

    let database = Database::open("/tmp/completions", Vec::<(String, String)>::new()).await?;

    let mut txn = database.begin().await?;
    txn.append_documents(
        "products",
        (0..10_000).map(|i| Document::keyed(format!("p{i}"), format!("product {i}"), 0.5)),
        [],
    )?;
    txn.commit().await?;
    ```

### Serving: an engine that follows the database

A serving process asks the database for an engine. The engine loads every index now, and each call to
`sync()` loads the newest version, downloading only segments it does not already hold.

=== "Python"

    ```python
    engine = db.engine()
    print(engine.version, engine.names())            # 1 ['products']
    print(engine.complete("product 12", ["products"], limit=3))
    ```

=== "Rust"

    ```rust
    use completr::{Engine, IndexOptions, Replica};

    let engine = Engine::new();
    let replica = Replica::new(database.clone(), IndexOptions::default());
    replica.sync(&engine).await?;
    let hits = engine.complete(&["products"], "product 12", 3);
    ```

In Rust, the `Replica` is separate from the `Engine` it publishes to; in Python, `db.engine()` combines them.

### Updating: change sets and the ingestor

Any process may submit changes as a `ChangeSet`. Submitting writes the change set to the database's inbox;
it does not commit it. An `Ingestor` commits pending change sets in order, and only the one holding the
ingestor lease acts, so many processes can run one safely.

=== "Python"

    ```python
    changes = completr.ChangeSet()
    changes.upsert("products", [{"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9}])
    changes.delete("products", ["p42"])
    db.submit(changes)

    ingestor = completr.Ingestor(db, "ingestor-1")
    print(ingestor.run_once())   # {'step': 'committed', 'version': 2, 'change_sets': 1, 'documents': 1}
    ingestor.release()

    engine.sync()
    print([(s.id, s.text, s.kind) for s in engine.complete("wirel", ["products"])])
    ```

=== "Rust"

    ```rust
    use completr::{key_id, ChangeSet, IngestStep, Ingestor};

    let mut changes = ChangeSet::new();
    changes
        .upsert("products", [Document::keyed("kb-1", "Wireless Keyboard", 0.9)])
        .delete("products", [key_id("p42")]);
    database.submit(changes).await?;

    let mut ingestor = Ingestor::new(database.clone(), "ingestor-1");
    if let IngestStep::Committed { version, .. } = ingestor.run_once().await? {
        println!("committed version {version}");
    }
    ingestor.release().await?;

    replica.sync(&engine).await?;
    ```

```text
[('kb-1', 'Wireless Keyboard', 'prefix')]
```

In a real deployment, the ingestor runs in its own process and calls `run_once()` in a loop, and serving
processes call `engine.sync()` every few seconds. [Serverless deployment](guides/serverless.md) covers
both, and the runnable examples are
[`live_updates.py`](https://github.com/sayef/completr/blob/main/crates/completr-py/examples/live_updates.py)
and [`live_updates.rs`](https://github.com/sayef/completr/blob/main/crates/completr/examples/live_updates.rs).

## Next steps

- [Concepts](concepts.md): segments, indexes, engines, databases and how scoring works.
- [Documents](guides/documents.md): every field, string ids, and table input.
- [Completion](guides/completion.md): match kinds, highlights, contexts and limits.
- [asyncio](guides/asyncio.md): the same database in FastAPI and other asyncio services.
