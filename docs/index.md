---
title: completr
hide:
  - navigation
  - toc
---

<section class="cp-hero">
  <div class="cp-hero__text">
    <p class="cp-eyebrow">Rust · Python · serverless</p>
    <h1 class="cp-title" aria-label="Autocompletion that ships as a library.">
      <span class="cp-rotate" aria-hidden="true">
        <span>Autocompletion that ships as a library<span class="cp-dot">.</span></span>
        <span>Your database is a bucket<span class="cp-dot">.</span></span>
        <span>0.15 ms at the median<span class="cp-dot">.</span></span>
        <span>40 million titles in 3.1 GB<span class="cp-dot">.</span></span>
        <span>Typos forgiven, ranks kept<span class="cp-dot">.</span></span>
      </span>
    </h1>
    <p class="cp-lede">An embedded engine whose database is a bucket. No cluster and no server: every process
    reads the index straight from storage, ranks exact, prefix, abbreviation, infix and misspelt matches
    together, and answers in a fraction of a millisecond.</p>
    <div class="cp-actions">
      <a class="md-button md-button--primary" href="getting-started/">Get started</a>
      <a class="md-button" href="https://github.com/sayef/completr">GitHub</a>
    </div>
    <code class="cp-install">pip install completr</code>
  </div>
  <div class="cp-hero__demo">
    <img src="assets/demo.svg" alt="completr completing music queries: prefix, abbreviation, spelling correction, word decomposition and infix" width="720" height="380">
  </div>
</section>

<div class="cp-stats">
  <div><strong>0.15 ms</strong><span>median query, Hacker News titles</span></div>
  <div><strong>40,000</strong><span>queries a second on 8 threads</span></div>
  <div><strong>40 M</strong><span>MusicBrainz recordings in 3.1 GB</span></div>
  <div><strong>5 corpora</strong><span><a href="benchmarks/">benchmarked</a> against tantivy, Typesense and Meilisearch</span></div>
</div>

<div class="grid cards" markdown>

-   **Every match kind**

    ---

    Exact, prefix, infix, abbreviation, spelling-tolerant and word-decomposing completion, ranked together
    with popularity.

    [:octicons-arrow-right-24: Completion](guides/completion.md)

-   **Serverless by design**

    ---

    Storage is the only shared component. Commits are create-only writes, and every process syncs
    itself from the bucket.

    [:octicons-arrow-right-24: Serverless deployment](guides/serverless.md)

-   **Semantic and hybrid**

    ---

    Embeddings from any model, quantised to 2 to 4 bits, fused with lexical results by reciprocal rank,
    weighted blending or lexical-first ordering.

    [:octicons-arrow-right-24: Semantic and hybrid](guides/semantic-hybrid.md)

-   **Override layers**

    ---

    One catalogue, many views: tenants, languages, listeners, promotions and takedowns as small layers
    over the shared data, overriding it per document id, without a copy.

    [:octicons-arrow-right-24: Layers](guides/layers.md)

-   **Fast and deterministic**

    ---

    Memory-mapped, checksummed segments. Typed queries take 0.1 to 0.2 ms at the median on short titles,
    and identical inputs give bit-identical results.

    [:octicons-arrow-right-24: Architecture](architecture.md)

-   **Rust and Python**

    ---

    Documents as dicts, `Document` objects, or pandas, polars and Arrow tables. Typed stubs, an asyncio API
    and a command-line tool.

    [:octicons-arrow-right-24: Python API](reference/python.md)

</div>

## Install

=== "Python"

    ```sh
    pip install completr
    ```

=== "Rust"

    ```sh
    cargo add completr --features store    # databases on local disk and in memory
    cargo add tokio --features macros,rt-multi-thread
    ```

=== "Command line"

    ```sh
    cargo install completr-cli
    ```

## Quick start

=== "Python"

    ```python
    import completr
    from completr import Index

    docs = [
        {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95},
        {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
        {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
        {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75, "abbreviations": ["RHCP"]},
    ]
    index = Index.from_documents(docs)   # in memory
    for query in ["danc", "RHCP", "bohemain rapsody", "queen"]:
        print(query, [(s.text, s.kind) for s in index.complete(query, limit=3)])

    db = completr.connect("./data")   # or "s3://bucket/prefix", "gs://...", "az://..."
    txn = db.begin()
    txn.append("songs", docs)
    txn.commit()
    engine = db.engine()   # loads new versions by itself, every 5 seconds
    print([s.text for s in engine.complete("danc", ["songs"])])
    ```

=== "Rust"

    ```rust
    use std::{sync::Arc, time::Duration};
    use completr::{Database, Document, Engine, Index, IndexOptions, Replica};

    let docs = || [
        Document::keyed("bohemian", "Bohemian Rhapsody – Queen", 0.95),
        Document::keyed("dancing", "Dancing Queen – ABBA", 0.85),
        Document::keyed("dark", "Dancing in the Dark – Bruce Springsteen", 0.7),
        Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75).with_abbreviation("RHCP"),
    ];
    let index = Index::from_documents(docs())?;   // in memory
    for s in index.complete("danc", 3) {
        println!("{} {}", s.text, s.kind.as_str());
    }

    let database = Database::open("./data", Vec::<(String, String)>::new()).await?;
    let mut txn = database.begin().await?;
    txn.append_documents("songs", docs(), [])?;
    txn.commit().await?;
    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database, IndexOptions::default()));
    replica.sync(&engine).await?;
    let _follower = replica.follow(&engine, Duration::from_secs(5));
    let hits = engine.complete(&["songs"], "danc", 10);
    ```

```text
danc [('Dancing Queen – ABBA', 'prefix'), ('Dancing in the Dark – Bruce Springsteen', 'prefix')]
RHCP [('Under the Bridge – Red Hot Chili Peppers', 'abbreviation')]
bohemain rapsody [('Bohemian Rhapsody – Queen', 'fuzzy')]
queen [('Bohemian Rhapsody – Queen', 'infix'), ('Dancing Queen – ABBA', 'infix')]
['Dancing Queen – ABBA', 'Dancing in the Dark – Bruce Springsteen']
```

Next, [Getting started](getting-started.md) walks from an index in memory to a database in a bucket that
several processes share, [Concepts](concepts.md) explains the model behind it, and
[Collections](guides/collections.md) covers the convenience layer on top.
