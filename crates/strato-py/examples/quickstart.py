"""The README's Python example: python crates/strato-py/examples/quickstart.py"""

import numpy as np

from strato import Engine, Index, Segment

# (id, text, weight, [(alias, is_abbreviation)])
docs = [
    (1, "Machine Learning", 0.9, [("ML", True)]),
    (2, "Machine Vision", 0.4, []),
    (3, "Data Science", 0.7, [("data analytics", False)]),
]
index = Index([Segment.build(docs)])

for query in ["mach", "ML", "vison", "science"]:
    print(f"{query:>8} ->", [(h.id, h.kind, round(h.score, 3)) for h in index.complete(query, limit=3)])

# Embeddings from any model: one float32 row per document.
vectors = np.random.default_rng(0).standard_normal((3, 64), dtype=np.float32)
vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
semantic = Index([Segment.build(docs, vectors=vectors)])
for hit in semantic.hybrid_search("machine", vectors[2], limit=3, fusion="rrf"):
    print(hit.id, hit.kind, round(hit.score, 4), hit.lexical_score, hit.semantic_score)

# Layers: a tenant's index overrides the shared one per id.
engine = Engine()
engine.publish({"shared": index, "acme": Index([Segment.build([(2, "Machine Vision Systems", 1.0, [])])])})
print([(h.id, h.layer) for h in engine.complete("machine", ["shared", "acme"])])
