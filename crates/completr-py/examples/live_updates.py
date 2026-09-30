"""Serverless updates: python crates/completr-py/examples/live_updates.py [url]"""

import sys
import tempfile

import completr

url = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()  # or s3://bucket/prefix, gs://..., az://...
db = completr.connect(url)

txn = db.begin()
txn.append("products", [{"id": f"p{i}", "text": f"product {i}", "popularity": 0.5} for i in range(10_000)])
txn.commit()

# Serving processes: an engine that follows the database.
engine = db.engine()

# Any process: submit changes.
changes = completr.ChangeSet()
changes.upsert("products", [{"id": "kb-1", "text": "Wireless Keyboard", "popularity": 0.9, "contexts": ["peripherals"]}])
changes.delete("products", ["p42"])
db.submit(changes)

# One process at a time commits them (lease-elected).
ingestor = completr.Ingestor(db, "ingestor-1")
print(ingestor.run_once())
ingestor.release()

engine.sync()
print([(s.id, s.text, s.kind) for s in engine.complete("wirel", ["products"], contexts=["peripherals"])])
