import numpy as np
import pytest

from strato import Dataset, Engine, Follower, Index, Segment

rng = np.random.default_rng(0)
N, DIM = 500, 64
VECTORS = rng.standard_normal((N, DIM)).astype(np.float32)
VECTORS /= np.linalg.norm(VECTORS, axis=1, keepdims=True)
DOCS = [(i, f"entry {i}", 0.5, []) for i in range(N)]


def test_vector_search_returns_semantic_hits():
    index = Index([Segment.build(DOCS, vectors=VECTORS)], max_score=1000.0)
    assert index.vector_dim == DIM
    hits = index.vector_search(VECTORS[42], limit=5)
    assert hits[0].id == 42 and {h.kind for h in hits} == {"semantic"}
    assert [h.id for h in index.vector_search(VECTORS[42].tolist(), 5)] == [h.id for h in hits]
    assert index.autocomplete("entry 42")[0].kind == "exact"


def test_updates_deletes_and_compaction():
    base = Segment.build(DOCS, vectors=VECTORS)
    delta = Segment.build([(7, "moved", 0.5, [])], deletes=[42], vectors=VECTORS[100:101])
    index = Index([base, delta], max_score=1000.0)
    assert 42 not in [h.id for h in index.vector_search(VECTORS[42], 10)]
    assert index.vector_search(VECTORS[100], 2)[0].id in (7, 100)
    before = [(h.id, h.score) for h in index.vector_search(VECTORS[3], 10)]
    compacted = Index([index.compact()], max_score=1000.0)
    assert [(h.id, h.score) for h in compacted.vector_search(VECTORS[3], 10)] == before


def test_configuration_and_errors():
    for bits in (2, 3, 4):
        index = Index([Segment.build(DOCS, vectors=VECTORS, vector_bits=bits)], max_score=1000.0, vector_threads=0)
        assert index.vector_search(VECTORS[9], 1)[0].id == 9
    with pytest.raises(ValueError):
        Segment.build(DOCS, vectors=VECTORS, vector_bits=5)
    with pytest.raises(ValueError):
        Segment.build(DOCS, vectors=VECTORS[:10])
    with pytest.raises(ValueError):
        Index([Segment.build(DOCS, vectors=VECTORS)], max_score=1000.0).vector_search(np.zeros(8, dtype=np.float32))
    strict = Segment.build(DOCS, min_word_chars=5, max_edit_distance=1)
    assert Index([strict], max_score=1000.0).autocomplete("entri 4")


def test_layers_and_dataset(tmp_path):
    engine = Engine(overfetch=3)
    ds = Dataset(str(tmp_path / "ds"), vector_bits=2)
    t = ds.begin()
    t.append("default", DOCS, vectors=VECTORS)
    t.append("acme", DOCS[:20], vectors=VECTORS[:20])
    t.commit()
    Follower(ds, engine).sync()
    hits = engine.vector_search(VECTORS[5], ["default", "acme"], 5)
    assert hits[0].id == 5 and hits[0].layer == "acme"
    for i in range(5):
        t = ds.begin()
        t.append("default", [(1000 + i, f"new {i}", 0.1, [])], vectors=VECTORS[i : i + 1])
        t.commit()
    before = [h.id for h in ds.load_index("default").vector_search(VECTORS[1], 10)]
    ds.compact("default", max_segments=1)
    assert [h.id for h in ds.load_index("default").vector_search(VECTORS[1], 10)] == before


def test_hybrid_fusions():
    docs = [(i, "python programming" if i == 7 else f"entry {i}", 0.5, []) for i in range(N)]
    index = Index([Segment.build(docs, vectors=VECTORS)], max_score=1000.0)
    for fusion in ("rrf", "weighted", "lexical_first"):
        hits = index.hybrid_search("python", VECTORS[42], limit=5, fusion=fusion)
        kinds = {h.id: h.kind for h in hits}
        assert kinds[7] in ("exact", "prefix") and kinds[42] == "semantic"
        again = index.hybrid_search("python", VECTORS[42], limit=5, fusion=fusion)
        assert [(h.id, h.score, h.kind) for h in again] == [(h.id, h.score, h.kind) for h in hits]
    first = index.hybrid_search("python", VECTORS[42], limit=5, fusion="lexical_first")
    assert first[0].id == 7 and first[0].lexical_score is not None
    engine = Engine()
    engine.publish({"default": index})
    assert all(h.layer == "default" for h in engine.hybrid_search("python", VECTORS[42], ["default"], limit=5))
    with pytest.raises(ValueError):
        index.hybrid_search("python", VECTORS[42], fusion="magic")
    with pytest.raises(ValueError):
        index.hybrid_search("python", VECTORS[42], fusion="weighted", semantic_weight=1.5)
