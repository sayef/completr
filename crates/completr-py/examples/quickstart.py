"""The README's Python example: python crates/completr-py/examples/quickstart.py"""

import numpy as np

import completr
from completr import Engine, Index

docs = [
    {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95, "synonyms": ["is this the real life"]},
    {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
    {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
    {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75, "abbreviations": ["RHCP"]},
]

# An index in memory.
index = Index.from_documents(docs)
for query in ["danc", "RHCP", "bohemain rapsody", "queen"]:
    print(f"{query:>16} ->", [(s.text, s.kind, round(s.score, 3)) for s in index.complete(query, limit=3)])

# Highlights are character ranges of the text that matched.
top = index.complete("danc")[0]
print("matched:", [top.text[a:b] for a, b in top.highlights])

# Synonyms are searched on their own.
print(index.complete_aliases("is this the real"))

# Embeddings from any model: one float32 row per document.
vectors = np.random.default_rng(0).standard_normal((len(docs), 64), dtype=np.float32)
vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
semantic = Index.from_documents(docs, vectors=vectors)
for s in semantic.hybrid_search("danc", vectors[3], limit=3):
    print(s.id, s.kind, round(s.score, 4))

# Layers: a tenant's index overrides the shared one per id.
engine = Engine()
engine.publish({"shared": index, "radio": Index.from_documents([{"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen"}])})
print([(s.text, s.layer) for s in engine.complete("queen", ["shared", "radio"])])

# A database: a transaction commits a version, and an engine from the database keeps itself on the latest one.
db = completr.connect("memory://")  # or "./data", "s3://bucket/prefix", gs://..., az://...
txn = db.begin()
txn.append("songs", docs)
print("committed version", txn.commit()["version"])
engine = db.engine()
print([(s.text, s.kind) for s in engine.complete("danc", ["songs"])])
