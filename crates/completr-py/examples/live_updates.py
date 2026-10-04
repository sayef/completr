"""An engine that follows a writer: python crates/completr-py/examples/live_updates.py [url]"""

import sys
import tempfile
import time

import completr

url = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()  # or s3://bucket/prefix, gs://..., az://...

# A writing process: every commit is a new version of the database.
writer = completr.connect(url)
txn = writer.begin()
txn.append("songs", [{"id": f"s{i}", "text": f"song {i}", "popularity": 0.5} for i in range(10_000)])
txn.commit()

# A serving process: the engine loads new versions in the background every sync_every seconds.
engine = completr.connect(url).engine(sync_every=0.5)
print(engine.version, engine.complete("bill", ["songs"]))

txn = writer.begin()
txn.append(
    "songs",
    [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9, "contexts": ["pop"]}],
    deletes=["s42"],
)
version = txn.commit()["version"]

deadline = time.monotonic() + 10
while engine.version < version and time.monotonic() < deadline:
    time.sleep(0.1)
print(engine.version, [(s.id, s.text, s.kind) for s in engine.complete("bill", ["songs"], contexts=["pop"])])
print(engine.sync_status["error"])
