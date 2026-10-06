# Collections

`completr.Client` is a convenience layer over a [database](serverless.md). It manages named
**collections**: each is an index of the same name, so a client and the building blocks work on the same
database. A client adds:

- **Named collections with stored settings**: each collection's optimize settings are kept in the
  database when it is created.
- **Writes that look after themselves**: `add` and `delete` commit directly, and a collection with
  `optimize="auto"` merges its small segments in the background after writes.
- **Reads that sync themselves**: the client loads new versions every `sync_every` seconds from a
  background thread, as an engine from `db.engine()` does.
- **`stats()`**: a collection's size and how fresh the client's view of it is.
- **One `complete()`**: direct matches, synonyms with `aliases=True` merged below them, layers, contexts,
  and a query vector fused by reciprocal rank.

It does not expose [index options](configuration.md#index-options) such as `popularity_weight`, the
choice of [fusion method](semantic-hybrid.md#fusion), `vector_search` and `hybrid_search` with both source
scores, or getting a document by id. Use the building blocks for those: `client.database` returns the
`Database` under a client.

## A first collection

=== "Python"

    ```python
    import completr

    client = completr.Client("./data")   # in memory when omitted; or s3://bucket/prefix, gs://..., az://...
    songs = client.get_or_create_collection("songs")
    songs.add([
        {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95, "synonyms": ["is this the real life"]},
        {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
        {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
        {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75, "abbreviations": ["RHCP"]},
    ])

    for query in ["danc", "RHCP"]:
        print(query, [(s.text, s.kind, round(s.score, 3), s.layer) for s in songs.complete(query, limit=3)])
    ```

=== "Rust"

    ```rust
    use completr::{connect, ConnectOptions, Document, Optimize, Query};

    #[tokio::main]
    async fn main() -> Result<(), completr::Error> {
        let client = connect("./data", ConnectOptions::default()).await?;
        let songs = client.get_or_create_collection("songs", Optimize::default()).await?;
        songs
            .add([
                Document::keyed("bohemian", "Bohemian Rhapsody – Queen", 0.95)
                    .with_synonym("is this the real life"),
                Document::keyed("dancing", "Dancing Queen – ABBA", 0.85),
                Document::keyed("dark", "Dancing in the Dark – Bruce Springsteen", 0.7),
                Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75)
                    .with_abbreviation("RHCP"),
            ])
            .await?;

        for c in songs.complete("danc", &Query::default().limit(3))? {
            println!("{} {} {:.3} {}", c.text, c.kind.as_str(), c.score, c.collection);
        }
        Ok(())
    }
    ```

```text
danc [('Dancing Queen – ABBA', MatchKind.PREFIX, 0.547, 'songs'), ('Dancing in the Dark – Bruce Springsteen', MatchKind.PREFIX, 0.462, 'songs')]
RHCP [('Under the Bridge – Red Hot Chili Peppers', MatchKind.ABBREVIATION, 0.486, 'songs')]
```

In Python, `completr.connect` returns a `Database` and `completr.Client` a collections client. In Rust,
`completr::connect` returns the collections `Client`, and `Database::open` a database.

`add` replaces documents with the same id, and returns the new version of the database once the write is
durable. `delete(ids)` removes documents. Both are visible to this client at once. `Client` takes the same
URLs, `cache_dir`, `options` and [build options](configuration.md#build-options) as `connect`, plus
`sync_every`.

| `Client` method | Does |
|---|---|
| `collections()` | Lists the collections. |
| `collection(name)`, `client[name]` | Opens an existing collection; raises `NotFoundError` when there is none. |
| `create_collection(name, **settings)` | Creates a collection with its [settings](configuration.md#collection-settings). |
| `get_or_create_collection(name, **settings)` | Opens a collection, or creates it; an existing collection keeps its stored settings. |
| `drop_collection(name)` | Deletes a collection. |
| `sync()` | Loads the latest version now; returns it, or `None` when nothing changed. |
| `database` | The `Database` under the client. |

## Completing

<!-- skip-test -->
```python
collection.complete(query, limit=10, *, aliases=False, layers=None, contexts=None, vector=None, ignore_missing_layers=False)
```

| Option | Default | Effect |
|---|---|---|
| `limit` | 10 | Maximum number of suggestions. |
| `aliases` | `False` | Also match synonyms, ranked below every direct match. |
| `layers` | none | Collections searched on top of this one, later ones overriding earlier ones per id. |
| `ignore_missing_layers` | `False` | Search a layer that does not exist as empty instead of raising `LayerNotFoundError`. |
| `contexts` | none | Only documents tagged with any of these contexts. |
| `vector` | none | An embedding of the query, fused with the text results by reciprocal rank; with an empty query, a vector search. |

Each suggestion's `layer` is the collection it came from. In Rust, the options are fields of `Query`, with
chainable setters, and a `Completion` names its collection in `collection`.

Synonyms are only matched with `aliases=True`. They rank below every direct match, keeping their own
order, and fill the result only up to `limit`; a document that matches directly is not repeated:

```python
print(songs.complete("is this the real life"))
print(songs.complete("is this the real life", aliases=True))
```

```text
[]
[Suggestion(id='bohemian', text='Bohemian Rhapsody – Queen', score=0.6637, kind=MatchKind.SYNONYM, layer='songs')]
```

Layers are other collections:

```python
radio = client.get_or_create_collection("radio")
radio.add([{"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen", "popularity": 1.0}])   # overrides "bohemian"
print([(s.text, s.layer) for s in songs.complete("queen", layers=["radio"])])
```

```text
[('Bohemian Rhapsody (Remastered 2011) – Queen', 'radio'), ('Dancing Queen – ABBA', 'songs')]
```

A layer that does not exist raises `LayerNotFoundError` unless `ignore_missing_layers=True`. See
[Layers](layers.md) for how overrides apply.

Collections live in namespaces; those above are in `default`. `client.namespace(name)` has the same
collection methods, and the layers of a query come from the collection's namespace. A sync switches a whole
namespace at once, so a query over several of its collections sees one version of all of them:

```python
de = client.namespace("de")
charts = de.get_or_create_collection("charts")
charts.add([{"id": "luftballons", "text": "99 Luftballons – Nena", "popularity": 0.98}])
print(de.collections(), client.collections())
print(charts)
```

```text
['charts'] ['radio', 'songs']
Collection(name='charts', namespace='de')
```

## Reading from another process

A client loads every collection when it opens. Its first `complete` starts a background thread that loads
new versions every `sync_every` seconds (5 by default). With `sync_every=None`, call `client.sync()`
yourself.

```python
import time

reader = completr.Client("./data", sync_every=1.0)   # in a serving process
served = reader["songs"]

songs.add([{"id": "blinding", "text": "Blinding Lights – The Weeknd", "popularity": 0.9}])   # in another process

print(served.complete("blind"))   # not synced yet
time.sleep(1.5)
print(served.complete("blind"))
```

```text
[]
[Suggestion(id='blinding', text='Blinding Lights – The Weeknd', score=0.5666, kind=MatchKind.PREFIX, layer='songs')]
```

As with engines, open clients in each worker after a pre-forking server forks; see
[Serverless deployment](serverless.md#forking-servers).

## Stats

`stats()` reports a collection's size and how fresh this client's view of it is:

```python
stats = served.stats()
print({key: stats[key] for key in ("documents", "segments", "version", "synced_version", "sync_error")})
```

```text
{'documents': 5, 'segments': 2, 'version': 7, 'synced_version': 7, 'sync_error': None}
```

| Field | Meaning |
|---|---|
| `documents` | Live documents in the version this client serves. |
| `segments` | Segment files of the collection in the latest version. |
| `bytes` | Size of those segment files. |
| `version` | The latest version of the database in storage. |
| `committed_at` | When `version` was committed, in seconds since the Unix epoch (UTC). |
| `synced_version` | The version this client serves. |
| `synced_at` | When this client last checked for a newer version, or `None` if it never did. |
| `sync_error` | The error of the last sync, or `None` if it succeeded. |
| `optimize_error` | The error of this client's last background merge of the collection, or `None`. |

`stats()` reads the latest manifest from storage, so call it from health checks and metrics, not per
request. In Rust, `Collection::stats` returns `CollectionStats`, with the times as `committed_at_ms` and
`synced_at_ms` in milliseconds.

## Optimizing

Every `add` and `delete` writes a small segment. By default (`optimize="auto"`), the writing client merges
a collection's segments in a background thread after writes, once a merge is due, and at most every
`min_interval` seconds (30). With `optimize="off"`, segments are only merged when you call `optimize()`:

```python
drafts = client.create_collection("drafts", optimize="off")
for i in range(6):
    drafts.add([{"id": f"draft-{i}", "text": f"Draft {i}"}])
print(drafts.stats()["segments"])
drafts.optimize()
print(drafts.stats()["segments"])
```

```text
6
2
```

Merges follow the same policy as [`db.compact`](serverless.md#compaction): `fanout`, `max_segments` and
`max_hidden_fraction`, stored with the collection. A failed background merge shows in
`stats()["optimize_error"]`. [Configuration](configuration.md#collection-settings) lists every setting.

## With the building blocks

`client.database` is an ordinary `Database`, so the building blocks work on a client's collections:
transactions, snapshots, engines with index options, `hybrid_search` with other fusion methods, and
cleanup, which a client does not run by itself.

```python
database = client.database
print(database.index_names())
engine = database.engine(popularity_weight=0.0)
print([s.text for s in engine.complete("queen", ["songs"])])
print(database.open_index("songs").get("dancing"))
database.cleanup(keep_versions=10, older_than_seconds=3600)
```

```text
['drafts', 'radio', 'songs']
['Dancing Queen – ABBA', 'Bohemian Rhapsody – Queen']
Document(id='dancing', text='Dancing Queen – ABBA', popularity=0.85)
```

For asyncio, `completr.AsyncClient` has the same methods, awaited; see [asyncio](asyncio.md#collections).
