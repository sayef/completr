import os
import time

import pytest

from strato import ChangeSet, ConflictError, Database, Engine, Replica, Store, Ingestor

DOCS = [(i, f"item {name} {i}", (i % 10) / 10, []) for i, name in enumerate(["rust", "python", "data", "cloud"] * 25)]

URLS = ["memory:///py"]
if "STRATO_TEST_S3_URL" in os.environ:
    URLS.append(f"{os.environ['STRATO_TEST_S3_URL'].rstrip('/')}/py-database")


@pytest.fixture(params=["local", *URLS])
def database(request, tmp_path):
    url = str(tmp_path / "ds") if request.param == "local" else f"{request.param}-{time.time_ns()}"
    yield Database(url)
    store = Store(url)
    for key in store.list():
        store.delete(key)


def test_commit_rebase_conflict(database):
    t = database.begin()
    t.append("catalog", DOCS)
    t.set_max_score("catalog", 1000.0)
    t.set_metadata("cursor", "42")
    assert t.commit()["version"] == 1

    first, second, strict = database.begin(), database.begin(), database.begin()
    first.append("catalog", [(1000, "rust async", 0.9, [])], deletes=[0])
    second.append("catalog", [(1001, "rust macros", 0.8, [])])
    strict.append("catalog", [(1002, "rust unsafe", 0.1, [])])
    strict.strict()
    first.commit()
    manifest = second.commit()
    assert manifest["version"] == 3 and manifest["metadata"] == {"cursor": "42"}
    with pytest.raises(ConflictError):
        strict.commit()

    index = database.load_index("catalog")
    assert len(index) == 101
    assert {h.id for h in index.complete("rust a")} >= {1000}


def test_compact_cleanup_and_follow(database):
    t = database.begin()
    t.append("catalog", DOCS)
    t.commit()
    for i in range(4):
        t = database.begin()
        t.append("catalog", [(2000 + i, f"cloud native {i}", 0.5, [])], deletes=[i])
        t.commit()

    engine = Engine()
    replica = Replica(database, engine)
    assert replica.sync() == 5
    before = [(h.id, h.score) for h in engine.complete("cloud", ["catalog"], 20)]

    assert database.compact("catalog") == 6
    assert replica.sync() == 6 and replica.sync() is None
    assert [(h.id, h.score) for h in engine.complete("cloud", ["catalog"], 20)] == before

    stats = database.cleanup(keep_versions=1, older_than_seconds=0)
    assert stats["versions_removed"] == 5 and stats["segments_removed"] == 4
    assert database.versions() == [6]


def test_lease(database):
    lease = database.acquire_lease("compactor", "worker-a", 60)
    assert lease is not None and database.acquire_lease("compactor", "worker-b", 60) is None
    assert lease.renew(60) and lease.generation == 2
    lease.release()
    other = database.acquire_lease("compactor", "worker-b", 60)
    assert other is not None and other.generation == 4
    other.release()


def test_inbox_and_single_writer(database):
    for n in range(3):
        batch = ChangeSet()
        batch.upsert("catalog", [(5000 + n, f"streamed item {n}", 0.4, [])])
        if n == 2:
            batch.delete("catalog", [5000])
        database.submit(batch)
    assert database.pending_change_sets() == 3
    with pytest.raises(ValueError):
        database.submit(batch)

    leader, standby = Ingestor(database, "a"), Ingestor(database, "b")
    step = leader.run_once()
    assert step["step"] == "committed" and step["change_sets"] == 3 and leader.is_active
    assert standby.run_once()["step"] == "standby"
    assert leader.run_once()["step"] == "idle"
    index = database.load_index("catalog")
    assert index.get(5000) is None and index.get(5002)[1] == "streamed item 2"
    leader.release()
    assert standby.run_once()["step"] == "idle" and standby.is_active
    standby.release()


def test_grouped_replica_and_compacting_writer(database):
    t = database.begin()
    for tenant in ("base", "acme"):
        for language in ("en", "de"):
            t.append(f"{tenant}/{language}", DOCS[:20])
    t.commit()
    engine = Engine()
    replica = Replica(database, engine, group_separator="/")
    replica.sync()
    assert engine.names() == ["acme/de", "acme/en", "base/de", "base/en"]
    ingestor = Ingestor(database, "w")
    for n in range(6):
        batch = ChangeSet()
        batch.upsert("base/en", [(9000 + n, f"extra {n}", 0.2, [])])
        database.submit(batch)
        ingestor.run_once()
    replica.sync()
    assert len(engine.get("base/en")) == 26
    assert len(engine.get("base/en").segments()) < 7
    ingestor.release()


def test_threaded_queries_during_updates(database):
    import random
    import threading

    t = database.begin()
    t.append("catalog", [(i, f"item {w} {i}", (i % 10) / 10, []) for i, w in enumerate(["rust", "python", "data", "cloud"] * 500)])
    t.set_max_score("catalog", 1000.0)
    t.commit()
    engine = Engine()
    replica = Replica(database, engine)
    replica.sync()
    stop, samples, errors = threading.Event(), [], []

    def read(seed):
        rng = random.Random(seed)
        try:
            while not stop.is_set():
                query = rng.choice(["ru", "pyth", "data it", "cloud 1", "ite", "rsut"])
                index = engine.get("catalog")
                result = [(h.id, h.score, h.kind) for h in index.complete(query, 10)]
                if rng.random() < 0.05:
                    samples.append((index, query, result))
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    readers = [threading.Thread(target=read, args=(n,)) for n in range(6)]
    for r in readers:
        r.start()
    ingestor = Ingestor(database, "w")
    for n in range(30):
        batch = ChangeSet()
        batch.upsert("catalog", [(10_000 + n, f"rust streamed {n}", 0.3, []), (n, f"python renamed {n}", 0.9, [])])
        database.submit(batch)
        ingestor.run_once()
        replica.sync()
    stop.set()
    for r in readers:
        r.join()
    ingestor.release()

    assert not errors
    assert len({id(index) for index, _, _ in samples}) > 1
    for index, query, result in samples:
        assert [(h.id, h.score, h.kind) for h in index.complete(query, 10)] == result
