# Semantic and hybrid completion

Documents can carry embeddings from any model. completr does not include an embedding model: you compute
vectors with the model of your choice, when you add documents and when you query.

completr quantises embeddings with [TurboQuant](https://crates.io/crates/turbovec), 4 bits per dimension by
default (2 or 3 optional), and searches them with deleted and filtered-out documents masked out.

## Adding embeddings

Pass one float32 row per document as `vectors=`, or a `vector` per document. `Index.from_documents`,
`Segment.build`, `Transaction.append`, `Transaction.overwrite` and `ChangeSet.upsert` all take `vectors=`.
The examples use random vectors in place of a model:

```python
import zlib

import numpy as np

from completr import Index

docs = [
    {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95},
    {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
    {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75},
]

def embed(texts):
    """Stands in for your embedding model: one normalised float32 row per text."""
    rng = np.random.default_rng(zlib.crc32("\n".join(texts).encode()))
    rows = rng.standard_normal((len(texts), 64), dtype=np.float32)
    return rows / np.linalg.norm(rows, axis=1, keepdims=True)

vectors = embed([d["text"] for d in docs])   # shape (n, dim)
index = Index.from_documents(docs, vectors=vectors, vector_bits=4)
```

In Rust, attach the embedding to the document with `Document::with_vector`.

Every segment of an index must use the same dimension and `vector_bits`. Vector scores are inner products,
which equal cosine similarity for normalised vectors, so normalise your embeddings if your model does not.

## Vector search

`vector_search` returns the nearest documents as `Suggestion`s with `kind` set to `semantic`:

=== "Python"

    ```python
    for s in index.vector_search(vectors[1], limit=2):
        print(s.id, s.kind, round(s.score, 3))
    ```

=== "Rust"

    ```rust
    let query_vector: Vec<f32> = embed("disco classics");   // your model
    let nearest = index.vector_search(&query_vector, 2)?;    // Vec<Suggestion>, kind Semantic
    ```

```text
dancing semantic 0.998
bridge semantic -0.122
```

## Hybrid search

`hybrid_search` runs lexical completion on the text and vector search on the embedding, and fuses both
lists. Use it when users type a few characters and you also want related documents that do not share
their words. It returns `HybridSuggestion`s, which also report `lexical_score` and `semantic_score`,
either of which is `None` when the document came from one side only.

=== "Python"

    ```python
    query = "dancing q"
    for s in index.hybrid_search(query, embed([query])[0], limit=3):
        print(s.id, s.kind, round(s.score, 4), s.lexical_score and round(s.lexical_score, 3), round(s.semantic_score, 3))
    ```

=== "Rust"

    ```rust
    use completr::{Fusion, HybridOptions};

    let query_vector: Vec<f32> = embed("dancing q");   // your model
    let options = HybridOptions::default().fusion(Fusion::ReciprocalRank { k: 60.0 });
    let hits = index.hybrid_search("dancing q", &query_vector, 3, &options)?;
    ```

```text
dancing prefix 0.0328 0.547 0.023
bohemian semantic 0.0161 None 0.004
bridge semantic 0.0159 None -0.195
```

A hybrid suggestion keeps its lexical match kind when it matched the text, and is `semantic` otherwise.
`contexts` filters both sides. An engine's `vector_search` and `hybrid_search` take a list of
[layers](layers.md), as `complete` does.

## Fusion

| `fusion` | Score | Parameters |
|---|---|---|
| `rrf` (default) | `1 / (k + lexical_rank) + 1 / (k + semantic_rank)`, ranks from 1 | `rrf_k`, 60 by default; needs no calibration |
| `weighted` | `(1 - w) * lexical + w * max(semantic, 0)` | `semantic_weight` (`w`), 0.5 by default |
| `lexical_first` | lexical hits in their order, then semantic-only hits | none |

```python
query_vector = embed([query])[0]
print([(s.id, round(s.score, 4)) for s in index.hybrid_search(query, query_vector, limit=3, fusion="weighted", semantic_weight=0.3)])
print([s.id for s in index.hybrid_search(query, query_vector, limit=3, fusion="lexical_first")])
```

```text
[('dancing', 0.3896), ('bohemian', 0.0012), ('bridge', 0.0)]
['dancing', 'bohemian', 'bridge']
```

`candidates` sets how many hits are taken from each side before fusing; by default, `max(2 * limit, 20)`.

A [collection](collections.md)'s `complete(query, vector=...)` always fuses by reciprocal rank.

## Sizing and speed

| Setting | Effect |
|---|---|
| `vector_bits` | 2, 3 or 4 bits per dimension, a [build option](configuration.md#build-options). Fewer bits are smaller and faster, with lower recall. About 136 bytes per 256-d vector at 4 bits. |
| `vector_threads` | Threads per vector query, an [index option](configuration.md#index-options): 1 (default) runs on the caller's thread, which is best under concurrent load; 0 uses a global pool. |

On 200,000 documents with 256-d vectors at 4 bits, a vector search takes about 1.1 ms and a hybrid search
about 1.3 ms at p50 on one core.
