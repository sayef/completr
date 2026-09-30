---
title: strato
hide:
  - navigation
  - toc
---

<div class="strato-hero" markdown>

![strato: serverless autocompletion for Rust and Python](assets/banner-light.svg#only-light){ width="620" }
![strato: serverless autocompletion for Rust and Python](assets/banner-dark.svg#only-dark){ width="620" }

</div>

<p class="strato-pitch"><strong>Serverless autocompletion: an embedded engine whose database is a bucket.</strong></p>

strato is a library, not a service. Your application embeds it (in Rust, or in Python through first-class
bindings), and an object-store bucket or a local directory is the database. There is no cluster to deploy,
scale or keep alive: every process that serves completions reads the index straight from storage, and
writers coordinate through the storage itself.

<div class="strato-hero" markdown>

![strato completing queries: prefix, abbreviation, spelling correction, word decomposition and infix](assets/demo.svg){ width="600" }

</div>

<div class="grid cards" markdown>

-   **Every match kind**

    ---

    Exact, prefix, infix, abbreviation, spelling-tolerant and word-decomposing completion, ranked together
    with popularity.

    [:octicons-arrow-right-24: Completion](guides/completion.md)

-   **Serverless by design**

    ---

    Storage is the only shared component. Commits are create-only writes, and ingestion is a role any
    process can take.

    [:octicons-arrow-right-24: Serverless deployment](guides/serverless.md)

-   **Semantic and hybrid**

    ---

    Embeddings from any model, quantised to 2 to 4 bits, fused with lexical results by reciprocal rank,
    weighted blending or lexical-first ordering.

    [:octicons-arrow-right-24: Semantic and hybrid](guides/semantic-hybrid.md)

-   **Override layers**

    ---

    Search a tenant's, a user's or an experiment's index on top of shared data, per document id, without
    copying it.

    [:octicons-arrow-right-24: Layers](guides/layers.md)

-   **Fast and deterministic**

    ---

    Memory-mapped, checksummed segments. Typed queries take around 0.1 ms at p50, and identical inputs
    give bit-identical results.

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
    pip install strato
    ```

=== "Rust"

    ```sh
    cargo add strato                     # engine
    cargo add strato --features store    # plus databases on local disk and in memory
    ```

=== "Command line"

    ```sh
    cargo install strato-cli
    ```

## Quick start

=== "Python"

    ```python
    from strato import Index

    index = Index.from_documents([
        {"id": "ml", "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"]},
        {"id": "mv", "text": "Machine Vision", "popularity": 0.4},
        {"id": "ds", "text": "Data Science", "popularity": 0.7},
    ])

    for query in ["mach", "ML", "vison", "science"]:
        print(query, [(s.text, s.kind) for s in index.complete(query, limit=3)])
    ```

=== "Rust"

    ```rust
    use strato::{Document, Index};

    let index = Index::from_documents([
        Document::keyed("ml", "Machine Learning", 0.9).with_abbreviation("ML"),
        Document::keyed("mv", "Machine Vision", 0.4),
        Document::keyed("ds", "Data Science", 0.7),
    ])?;

    for s in index.complete("mach", 10) {
        println!("{} {} {:.3}", s.text, s.kind.as_str(), s.score);
    }
    ```

```text
mach [('Machine Learning', 'prefix'), ('Machine Vision', 'prefix')]
ML [('Machine Learning', 'abbreviation')]
vison [('Machine Vision', 'fuzzy')]
science [('Data Science', 'infix')]
```

Next, [Getting started](getting-started.md) walks from a first index to a live database, and
[Concepts](concepts.md) explains the model behind it.
