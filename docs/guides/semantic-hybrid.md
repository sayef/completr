# Semantic and hybrid completion

Documents can carry embeddings from any model. strato does not include an embedding model: you compute
vectors with the model of your choice, at build time for documents and at query time for queries.

strato quantises embeddings with [TurboQuant](https://crates.io/crates/turbovec), 4 bits per dimension by
default (2 or 3 optional), and searches them with deleted and filtered-out documents masked out.

## Adding embeddings

Pass one float32 row per document as `vectors=`, or a `vector` per document. The examples use random
vectors in place of a model:

```python
import zlib

import numpy as np
from strato import Index

docs = [
    {"id": "ml", "text": "Machine Learning", "popularity": 0.9},
    {"id": "dl", "text": "Deep Learning", "popularity": 0.6},
    {"id": "ds", "text": "Data Science", "popularity": 0.7},
]

def embed(texts):
    """Stands in for your embedding model: one normalised float32 row per text."""
    rng = np.random.default_rng(zlib.crc32("\n".join(texts).encode()))
    rows = rng.standard_normal((len(texts), 64), dtype=np.float32)
    return rows / np.linalg.norm(rows, axis=1, keepdims=True)

vectors = embed([d["text"] for d in docs])        # shape (n, dim)
index = Index.from_documents(docs, vectors=vectors, vector_bits=4)
print(index.vector_dim)                           # 64
```

Every segment of an index must use the same dimension and `vector_bits`. Vector scores are inner
products, which equal cosine similarity for normalised vectors, so normalise your embeddings if your model
does not.

## Vector search

`vector_search` returns the nearest documents to a query vector, as `Suggestion`s with `kind` set to
`semantic`.

=== "Python"

    ```python
    for s in index.vector_search(vectors[1], limit=2):
        print(s.id, s.kind, round(s.score, 3))
    ```

=== "Rust"

    ```rust
    let query_vector: Vec<f32> = embed("deep learning");   // your model
    let hits = index.vector_search(&query_vector, 10)?;   // Vec<Suggestion>, kind Semantic
    ```

## Hybrid search

`hybrid_search` runs lexical completion on the text and vector search on the embedding, then fuses both
lists. Use it when users type a few characters and you also want related documents that do not share
their words.

=== "Python"

    ```python
    query = "deep lea"
    for s in index.hybrid_search(query, embed([query])[0], limit=3, fusion="rrf"):
        print(s.id, s.kind, round(s.score, 4), s.lexical_score, s.semantic_score)
    ```

=== "Rust"

    ```rust
    use strato::{Fusion, HybridOptions};

    let options = HybridOptions::default().fusion(Fusion::ReciprocalRank { k: 60.0 });
    let hits = index.hybrid_search("deep lea", &query_vector, 10, &options)?;
    ```

A `HybridSuggestion` keeps its lexical match kind when it matched lexically, and `semantic` otherwise. It
reports both source scores in `lexical_score` and `semantic_score`, either of which is `None` when the
document came from one side only.

### Fusion

| `fusion` | Score | Parameters |
|---|---|---|
| `rrf` (default) | `1 / (k + lexical_rank) + 1 / (k + semantic_rank)`, ranks from 1 | `rrf_k`, 60 by default; needs no calibration |
| `weighted` | `(1 - w) * lexical + w * max(semantic, 0)` | `semantic_weight` (`w`), 0.5 by default |
| `lexical_first` | lexical hits in their order, then semantic-only hits | none |

```python
index.hybrid_search("deep lea", embed(["deep lea"])[0], limit=3, fusion="weighted", semantic_weight=0.3)
index.hybrid_search("deep lea", embed(["deep lea"])[0], limit=3, fusion="lexical_first")
```

`candidates` sets how many hits are taken from each side before fusing; by default,
`max(2 * limit, 20)`. `contexts` filters both sides.

## Engines and databases

`Engine.vector_search` and `Engine.hybrid_search` take a list of layers, as `complete` does. In a database,
pass `vectors=` to `Transaction.append`, `Transaction.overwrite` and `ChangeSet.upsert`:

```python
import tempfile
import strato

db = strato.connect(tempfile.mkdtemp(), vector_bits=4)
txn = db.begin()
txn.append("topics", docs, vectors=vectors)
txn.commit()

engine = db.engine()
print([s.id for s in engine.hybrid_search("deep lea", embed(["deep lea"])[0], ["topics"], limit=3)])
```

## Sizing and speed

| Setting | Effect |
|---|---|
| `vector_bits` | 2, 3 or 4 bits per dimension. Fewer bits are smaller and faster, with lower recall. About 136 bytes per 256-d vector at 4 bits. |
| `vector_threads` | Threads per vector query: 1 (default) runs on the caller's thread, which is best under concurrent load; 0 uses a global pool. |

On 200,000 documents with 256-d vectors at 4 bits, a vector search takes about 1.1 ms and a hybrid search
about 1.3 ms at p50 on one core.
