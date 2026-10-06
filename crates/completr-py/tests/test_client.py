import asyncio
import time

import pytest

import completr

CATALOGUE = [
    {"id": "ps5", "text": "PlayStation 5 Console", "popularity": 0.9, "abbreviations": ["PS5"]},
    {"id": "psvr", "text": "PlayStation VR2", "popularity": 0.4},
    {"id": "anc", "text": "Wireless Noise Cancelling Headphones", "popularity": 0.7, "synonyms": ["bluetooth headphones"]},
]


def ids(hits):
    return [h.id for h in hits]


def test_a_collection_adds_completes_and_deletes():
    db = completr.Client()
    products = db.get_or_create_collection("products")
    products.add(CATALOGUE)
    hits = products.complete("play")
    assert ids(hits) == ["ps5", "psvr"]
    assert hits[0].kind == "prefix" and hits[0].layer == "products"
    assert ids(products.complete("PS5")) == ["ps5"]

    assert products.complete("bluetooth") == []
    synonyms = products.complete("bluetooth", aliases=True)
    assert ids(synonyms) == ["anc"] and synonyms[0].kind == "synonym"

    products.delete(["psvr"])
    assert ids(products.complete("play")) == ["ps5"]
    stats = products.stats()
    assert stats["documents"] == 2 and stats["synced_version"] == stats["version"]
    assert stats["sync_error"] is None


def test_collections_are_named_persisted_and_layered(tmp_path):
    url = str(tmp_path / "db")
    db = completr.Client(url, sync_every=None)
    shared = db.create_collection("shared", optimize="off")
    shared.add(CATALOGUE)
    acme = db.create_collection("acme")
    acme.add([{"id": "psvr", "text": "PlayStation VR2 Bundle", "popularity": 0.4}])
    assert db.collections() == ["acme", "shared"]
    hits = shared.complete("play", layers=["acme"])
    assert [(h.text, h.layer) for h in hits] == [("PlayStation 5 Console", "shared"), ("PlayStation VR2 Bundle", "acme")]

    with pytest.raises(completr.InvalidInputError):
        db.create_collection("shared")
    with pytest.raises(completr.NotFoundError):
        db.collection("missing")
    with pytest.raises(completr.InvalidInputError):
        db.create_collection("other", optimize="sometimes")

    again = completr.Client(url, sync_every=None)
    assert ids(again["shared"].complete("play")) == ["ps5", "psvr"]
    again.drop_collection("acme")
    assert again.collections() == ["shared"]


def test_readers_follow_writers_by_themselves(tmp_path):
    url = str(tmp_path / "db")
    writer = completr.Client(url, sync_every=None).create_collection("products", min_interval=0.0, fanout=2)
    reader = completr.Client(url, sync_every=0.02)["products"]
    assert reader.complete("play") == []
    for doc in CATALOGUE:
        writer.add([doc])
    deadline = time.monotonic() + 10
    while len(reader.complete("play")) < 2:
        assert time.monotonic() < deadline, "the reader never caught up"
        time.sleep(0.02)
    while writer.stats()["segments"] > 2:
        assert time.monotonic() < deadline, "the writer never compacted"
        time.sleep(0.02)
    assert writer.optimize() is None or writer.stats()["segments"] == 1


def test_the_async_client_awaits_storage_calls(tmp_path):
    async def run():
        db = completr.AsyncClient(str(tmp_path / "db"), sync_every=None)
        products = await db.get_or_create_collection("products")
        await products.add(CATALOGUE)
        assert ids(products.complete("play")) == ["ps5", "psvr"]
        assert (await products.stats())["documents"] == 3
        assert await db.collections() == ["products"]

    asyncio.run(run())


def test_an_engine_from_a_database_syncs_itself(tmp_path):
    url = str(tmp_path / "db")
    writer = completr.connect(url)
    engine = completr.connect(url).engine(sync_every=0.02)
    assert engine.complete("play", ["products"], ignore_missing_layers=True) == []
    assert engine.sync_status is None
    txn = writer.begin()
    txn.append("products", CATALOGUE)
    txn.commit()
    deadline = time.monotonic() + 10
    while not engine.complete("play", ["products"], ignore_missing_layers=True):
        assert time.monotonic() < deadline, "the engine never caught up"
        time.sleep(0.02)
    assert ids(engine.complete("play", ["products"])) == ["ps5", "psvr"]
    status = engine.sync_status
    assert status["error"] is None and status["synced_at"] > 0
    assert completr.connect(url).engine(sync_every=None).sync_status is None


def test_collections_live_in_namespaces(tmp_path):
    client = completr.Client(str(tmp_path / "db"), sync_every=None)
    de = client.namespace("de-DE")
    shared = de.create_collection("shared")
    shared.add(CATALOGUE)
    de.create_collection("customer-a").add([{"id": "psvr", "text": "PlayStation VR2 Paket", "popularity": 0.4}])
    assert (de.collections(), client.collections()) == (["customer-a", "shared"], [])
    assert repr(shared) == "Collection(name='shared', namespace='de-DE')"
    hits = shared.complete("play", layers=["customer-a"])
    assert [(h.text, h.layer) for h in hits] == [("PlayStation 5 Console", "shared"), ("PlayStation VR2 Paket", "customer-a")]
    with pytest.raises(completr.LayerNotFoundError):
        shared.complete("play", layers=["customer-b"])
    assert len(shared.complete("play", layers=["customer-b"], ignore_missing_layers=True)) == 2
    with pytest.raises(completr.LayerNotFoundError):
        de.collection("customer-b")


def test_the_async_client_has_namespaces_too(tmp_path):
    async def run():
        client = completr.AsyncClient(str(tmp_path / "db"), sync_every=None)
        de = client.namespace("de-DE")
        products = await de.get_or_create_collection("products")
        await products.add(CATALOGUE)
        assert ids(products.complete("play")) == ["ps5", "psvr"] and products.namespace == "de-DE"
        assert await de.collections() == ["products"]
        db = completr.AsyncDatabase(str(tmp_path / "db"))
        assert await db.namespaces() == ["de-DE"]
        assert await db.namespace("de-DE").index_names() == ["products"]

    asyncio.run(run())
