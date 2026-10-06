import pytest

import completr

pyarrow = pytest.importorskip("pyarrow")


def test_completes_alongside_pyarrow_allocations(tmp_path):
    pool = pyarrow.default_memory_pool()
    table = pyarrow.table({"text": [f"skill {i}" for i in range(10_000)]})
    assert pool.bytes_allocated() > 0
    db = completr.connect(str(tmp_path / "db"))
    with db.namespace("en-US").begin() as txn:
        txn.overwrite("owner", [{"id": f"s{i}", "text": f"Skill {i}", "popularity": 0.5} for i in range(2_000)])
    namespace = completr.connect(str(tmp_path / "db")).engine(sync_every=None).namespace("en-US")
    for _ in range(200):
        hits = namespace.complete("skill 1", ["owner"])
        assert [str(h.id) for h in hits]
        table = table.slice(1)
    assert len(table) > 0
