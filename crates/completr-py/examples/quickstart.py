"""The README's Python example: python crates/completr-py/examples/quickstart.py"""

import numpy as np

from completr import Engine, Index

docs = [
    {"id": "ml", "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"]},
    {"id": "mv", "text": "Machine Vision", "popularity": 0.4},
    {"id": "ds", "text": "Data Science", "popularity": 0.7, "synonyms": ["data analytics"]},
]
index = Index.from_documents(docs)

for query in ["mach", "ML", "vison", "science"]:
    print(f"{query:>8} ->", [(s.text, s.kind, round(s.score, 3)) for s in index.complete(query, limit=3)])

# Highlights are character ranges of the text that matched.
top = index.complete("mach")[0]
print("matched:", [top.text[a:b] for a, b in top.highlights])

# Embeddings from any model: one float32 row per document.
vectors = np.random.default_rng(0).standard_normal((3, 64), dtype=np.float32)
vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
semantic = Index.from_documents(docs, vectors=vectors)
for s in semantic.hybrid_search("machine", vectors[2], limit=3, fusion="rrf"):
    print(s.id, s.kind, round(s.score, 4))

# Layers: a tenant's index overrides the shared one per id.
engine = Engine()
engine.publish({"shared": index, "acme": Index.from_documents([{"id": "mv", "text": "Machine Vision Systems"}])})
print([(s.text, s.layer) for s in engine.complete("machine", ["shared", "acme"])])
