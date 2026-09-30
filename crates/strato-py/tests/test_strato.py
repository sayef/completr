import os
import time

import pytest

from strato import CorruptionError, Engine, Index, NotFoundError, Segment, Store

DOCS = [
    {"id": 1, "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"], "synonyms": ["statistical learning"]},
    {"id": 2, "text": "Machine Vision", "popularity": 0.2},
    {"id": 3, "text": "Data Science", "popularity": 0.5, "synonyms": ["data analytics"]},
    {"id": 4, "text": "Datadog", "popularity": 0.1},
]


def test_autocomplete_kinds():
    index = Index([Segment.build(DOCS)], max_score=1000.0)
    hits = index.complete("machine", 10)
    assert [h.id for h in hits] == [1, 2]
    assert index.complete("machine learning")[0].kind == "exact"
    assert index.complete("ml")[0].id == 1
    assert index.complete("ml")[0].kind == "abbreviation"
    assert index.complete("machne lerning")[0].kind == "fuzzy"


def test_search_aliases_skips_abbreviations():
    index = Index([Segment.build(DOCS)], max_score=1000.0)
    assert [h.id for h in index.complete_aliases("data")] == [3]
    assert index.complete_aliases("ml") == []


def test_newer_segments_supersede_and_delete():
    base = Segment.build(DOCS)
    delta = Segment.build([{"id": 2, "text": "Computer Vision", "popularity": 0.2}], deletes=[4])
    index = Index([base, delta], max_score=1000.0)
    assert len(index) == 3
    assert [h.id for h in index.complete("machine")] == [1]
    assert 4 not in [h.id for h in index.complete("datadog")]
    assert index.get(2).text == "Computer Vision"
    compacted = index.compact()
    assert sorted(compacted.ids()) == [1, 2, 3] and compacted.deletes() == []


def test_round_trip(tmp_path):
    segment = Segment.build(DOCS, deletes=[9])
    path = tmp_path / "base.seg"
    segment.save(path)
    for copy in (Segment.open(path), Segment.from_bytes(segment.to_bytes())):
        assert copy.documents() == segment.documents()
        assert copy.deletes() == [9]
    with pytest.raises(CorruptionError):
        Segment.from_bytes(b"not a segment")


def test_engine_layers_override_by_id():
    engine = Engine()
    engine.publish(
        {
            "default": Index([Segment.build(DOCS)], max_score=1000.0),
            "acme": Index([Segment.build([{"id": 2, "text": "Machine Vision Systems", "popularity": 1.0}])], max_score=1000.0),
        }
    )
    hits = engine.complete("machine", ["default", "acme"])
    assert {(h.id, h.layer) for h in hits} == {(1, "default"), (2, "acme")}
    assert [h.id for h in engine.complete("machine", ["default", "missing"])] == [1, 2]
    engine.publish({"acme": None})
    assert engine.names() == ["default"]


def _round_trip(store: Store):
    store.put_segment("segments/base.seg", Segment.build(DOCS))
    store.put_segment("segments/delta.seg", Segment.build([{"id": 2, "text": "Computer Vision", "popularity": 0.2}], deletes=[4]))
    index = Index([store.get_segment("segments/base.seg"), store.get_segment("segments/delta.seg")], max_score=1000.0)
    assert len(index) == 3
    assert store.put_if_absent("_versions/000000000001.json", b"{}")
    assert not store.put_if_absent("_versions/000000000001.json", b"{}")
    assert store.list() == ["_versions/000000000001.json", "segments/base.seg", "segments/delta.seg"]
    for key in store.list():
        store.delete(key)
    with pytest.raises(NotFoundError):
        store.get("segments/base.seg")


def test_local_store(tmp_path):
    _round_trip(Store(str(tmp_path / "index")))


@pytest.mark.skipif("STRATO_TEST_S3_URL" not in os.environ, reason="STRATO_TEST_S3_URL not set")
def test_s3_store():
    _round_trip(Store(f"{os.environ['STRATO_TEST_S3_URL'].rstrip('/')}/py-{time.time_ns()}"))


def test_compact_keys_give_the_same_results():
    regular = Index([Segment.build(DOCS)], max_score=1000.0)
    compact_segment = Segment.build(DOCS, compact_keys=True)
    compact = Index([compact_segment], max_score=1000.0)
    for query in ["machine", "mach", "ml", "data", "machne lerning", "vision"]:
        assert [(h.id, h.score, h.kind) for h in compact.complete(query)] == [
            (h.id, h.score, h.kind) for h in regular.complete(query)
        ]
        assert [(h.id, h.score) for h in compact.complete_aliases(query)] == [
            (h.id, h.score) for h in regular.complete_aliases(query)
        ]
