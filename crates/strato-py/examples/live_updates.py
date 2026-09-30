"""Streamed updates: python crates/strato-py/examples/live_updates.py [url]"""

import sys
import tempfile

from strato import ChangeSet, Database, Engine, Replica, Ingestor

url = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()  # or s3://bucket/prefix, gs://..., az://...
database = Database(url)

txn = database.begin()
txn.append("products", [(i, f"product {i}", 0.5, []) for i in range(10_000)])
txn.commit()

engine = Engine()
replica = Replica(database, engine)
replica.sync()

changes = ChangeSet()
changes.upsert("products", [(10_000, "wireless keyboard", 0.9, [])])
changes.delete("products", [42])
database.submit(changes)

ingestor = Ingestor(database, "ingestor-1")
print(ingestor.run_once())
ingestor.release()

replica.sync()
print([(h.id, h.kind) for h in engine.complete("wirel", ["products"])])
