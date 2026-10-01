import asyncio
import subprocess
import sys

import pytest

import completr
from completr import ChangeSet, Document, Index, Ingestor

CATALOG = [
    {"id": "sku-ml", "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"], "contexts": ["books"]},
    {"id": "sku-mv", "text": "Machine Vision", "popularity": 0.4, "contexts": ["courses"]},
    {"id": "sku-ds", "text": "Data Science", "popularity": 0.7, "synonyms": ["data analytics"], "contexts": ["books"]},
    {"id": 42, "text": "Machine Translation", "popularity": 0.2},
]


class Table:
    """Quacks like a pyarrow table."""

    def __init__(self, rows):
        self.rows = rows

    def to_pylist(self):
        return self.rows


def test_documents_from_dicts_objects_and_tables():
    objects = [Document("sku-ml", "Machine Learning", 0.9, abbreviations=["ML"], contexts=["books"])]
    for source in (CATALOG, Table(CATALOG), objects):
        index = Index.from_documents(source)
        assert index.complete("mach")[0].id == "sku-ml"
    doc = Index.from_documents(CATALOG).get("sku-ds")
    assert (doc.id, doc.text, doc.synonyms, doc.contexts) == ("sku-ds", "Data Science", ["data analytics"], ["books"])
    assert doc.to_dict()["popularity"] == pytest.approx(0.7)
    assert Index.from_documents(CATALOG).get(42).id == 42
    with pytest.raises(completr.InvalidInputError, match="unknown document field"):
        Index.from_documents([{"id": 1, "text": "x", "popularty": 0.5}])
    with pytest.raises(ValueError):
        Index.from_documents([{"text": "no id"}])
    with pytest.raises(TypeError):
        Index.from_documents([(1, "tuple", 0.5)])


def test_suggestions_carry_text_and_highlights():
    index = Index.from_documents(CATALOG + [{"id": "u", "text": "Ärzte Übersicht", "popularity": 0.1}])
    top = index.complete("mach")[0]
    assert (top.id, top.text, top.kind, top.highlights) == ("sku-ml", "Machine Learning", "prefix", [(0, 4)])
    typo = index.complete("machne lerning")[0]
    assert [typo.text[a:b] for a, b in typo.highlights] == ["Machine", "Learning"]
    umlaut = index.complete("ärz üb")[0]
    assert [umlaut.text[a:b] for a, b in umlaut.highlights] == ["Ärz", "Üb"]
    assert index.complete_aliases("data ana")[0].text == "Data Science"


def test_contexts_filter_completions():
    index = Index.from_documents(CATALOG)
    assert [s.id for s in index.complete("mach", contexts=["books"])] == ["sku-ml"]
    assert {s.id for s in index.complete("mach")} == {"sku-ml", "sku-mv", 42}
    assert index.complete("mach", contexts=["music"]) == []
    assert [s.id for s in index.complete("ma", contexts=["courses"])] == ["sku-mv"]


def test_connect_engine_and_string_ids(tmp_path):
    db = completr.connect(str(tmp_path / "db"))
    txn = db.begin()
    txn.append("products", CATALOG)
    txn.commit()
    engine = db.engine()
    assert engine.version == 1 and db.index_names() == ["products"]
    assert engine.complete("data", ["products"])[0].id == "sku-ds"

    changes = ChangeSet()
    changes.upsert("products", [{"id": "sku-kb", "text": "Wireless Keyboard", "popularity": 0.8}])
    changes.delete("products", ["sku-ds"])
    db.submit(changes)
    Ingestor(db, "worker").run_once()
    assert engine.sync() == 2 and engine.sync() is None
    assert engine.complete("wirel", ["products"])[0].id == "sku-kb"
    assert engine.complete("data sc", ["products"]) == []
    with pytest.raises(completr.InvalidInputError):
        completr.Engine().sync()


def test_errors_form_a_hierarchy(tmp_path):
    for cls in (
        completr.ConflictError,
        completr.CorruptionError,
        completr.NotFoundError,
        completr.InvalidInputError,
        completr.StorageError,
    ):
        assert issubclass(cls, completr.CompletrError)
    with pytest.raises(completr.CorruptionError):
        completr.Segment.from_bytes(b"garbage")
    with pytest.raises(completr.NotFoundError):
        completr.connect(str(tmp_path / "empty")).open_index("missing")


def test_async_api(tmp_path):
    async def main():
        db = await completr.connect_async(str(tmp_path / "db"))
        txn = await db.begin()
        txn.append("products", CATALOG)
        await db.commit(txn)
        engine = await db.engine()
        changes = ChangeSet()
        changes.upsert("products", [{"id": "sku-kb", "text": "Wireless Keyboard"}])
        await db.submit(changes)
        await db.run_ingestor(Ingestor(db.database, "worker"))
        assert await engine.sync() == 2
        return engine.complete("wirel", ["products"])[0].id

    assert asyncio.run(main()) == "sku-kb"


def test_streamed_and_budgeted_builds(tmp_path):
    rows = [{"id": i, "text": f"item {i} alpha{i % 7}", "popularity": (i % 10) / 10} for i in range(3000)]
    built = completr.Segment.build(rows)
    written = completr.Segment.build(iter(rows), path=tmp_path / "one.seg")
    assert written.to_bytes() == built.to_bytes()

    writer = completr.SegmentWriter(tmp_path / "many", memory_budget=200_000)
    writer.add(CATALOG[0])
    writer.add(iter(rows))
    segments = writer.finish()
    assert len(segments) > 2
    split, one = Index(segments), Index([completr.Segment.build([CATALOG[0], *rows])])
    for q in ["item 12", "alpha3", "machine", "itme"]:
        assert [s.id for s in split.complete(q, 10)] == [s.id for s in one.complete(q, 10)]
    with pytest.raises(ValueError):
        writer.add(rows[0])


def test_threaded_writer_logs_without_deadlock(tmp_path):
    # Build events from worker threads need the GIL, which `add` must not hold while flushing.
    code = f"""
import logging, completr
logging.basicConfig(level=logging.DEBUG)
w = completr.SegmentWriter({str(tmp_path)!r}, memory_budget=200_000, build_threads=4)
w.add({{"id": i, "text": f"item {{i}} alpha{{i % 7}}"}} for i in range(5000))
print(len(w.finish()))
"""
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=60)
    assert out.returncode == 0, out.stderr
    assert int(out.stdout) > 2
    assert "built segment" in out.stderr
