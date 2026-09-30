"""Streamed updates: python crates/strato-py/examples/live_updates.py [url]"""

import sys
import tempfile

from strato import Batch, Dataset, Engine, Follower, Writer

url = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()  # or s3://bucket/prefix, gs://..., az://...
dataset = Dataset(url)

txn = dataset.begin()
txn.append("products", [(i, f"product {i}", 0.5, []) for i in range(10_000)])
txn.commit()

engine = Engine()
follower = Follower(dataset, engine)
follower.sync()

batch = Batch()
batch.upsert("products", [(10_000, "wireless keyboard", 0.9, [])])
batch.delete("products", [42])
dataset.submit(batch)

writer = Writer(dataset, "writer-1")
print(writer.run_once())
writer.release()

follower.sync()
print([(h.id, h.kind) for h in engine.autocomplete("wirel", ["products"])])
