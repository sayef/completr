# Configuration

completr's defaults suit most autocompletion workloads. This page lists every setting, where it is passed,
and its default.

## Database options

Passed to `completr.connect(url, ...)`, `completr.Database(url, ...)` and `completr.AsyncDatabase(url, ...)`.

| Setting | Default | Meaning |
|---|---|---|
| `url` | required | A local path, or an `s3://`, `gs://`, `az://` or `memory://` URL. |
| `options` | `None` | `object_store` configuration keys, such as `aws_region`, `aws_endpoint` or `google_service_account`. They override the environment. |
| `cache_dir` | `None` | Local directory for downloaded segments, memory-mapped. No effect on local databases. |

`connect` also takes the [build options](#build-options), which apply to every segment the database's
transactions and ingestors build.

## Build options

Build options decide what a segment stores. All segments of one index must share them.

| Setting | Default | Meaning |
|---|---|---|
| `min_word_chars` | 3 | Words shorter than this are not indexed for infix or fuzzy matching. |
| `max_edit_distance` | 2 | Maximum edits per word for spelling correction. |
| `fuzzy_prefix_chars` | 7 | Only this many leading characters of a word produce fuzzy delete variants. |
| `vector_bits` | 4 | Bits per embedding dimension: 2, 3 or 4. Fewer are smaller and faster, with lower recall. |
| `compact_keys` | `False` | Store keys in nested tries: about 25 % smaller keys, with lookups about 1.8 times and prefix scans about 4 times slower. |
| `build_threads` | 1 | Threads for building. 1 has the lowest peak memory; 0 uses all cores. |

Passed to: `connect`, `Segment.build`, `SegmentWriter`, `Index.from_documents` (all but `compact_keys`)
and `Client`.

## Index options

Index options decide how an index is searched. They do not change what is stored.

| Setting | Default | Meaning |
|---|---|---|
| `max_score` | estimated | Raw score that normalises to 1.0. Databases store it per index; pin it when scores must stay comparable across rebuilds. |
| `popularity_weight` | 0.4 | How much popularity counts in the score. 0 ignores it. |
| `short_query_chars` | 3 | Queries up to this many characters are served from a per-index cache. |
| `short_query_limit` | 100 | Results computed per cached short query. |
| `short_query_cache_entries` | 10,000 | Short queries cached per index. |
| `vector_threads` | 1 | Threads per vector query: 1 runs on the caller's thread, 0 uses a global pool, more uses a shared pool of that size. |

Passed to: `Index(...)`, `Index.from_documents` (`max_score`, `popularity_weight`), `db.open_index(...)`,
`db.engine(...)` and `Replica(...)` (all but `max_score`, which comes from the manifest).

## Engine options

| Setting | Default | Passed to | Meaning |
|---|---|---|---|
| `sync_every` | 5.0 | `db.engine(...)` | Seconds between background syncs, started by the engine's first query. `None` syncs only on `engine.sync()`. |
| `overfetch` | 2 | `Engine(...)`, `db.engine(...)` | Candidates per layer, as a multiple of `limit`, for searches over several layers. |

## Search options

| Setting | Default | Passed to | Meaning |
|---|---|---|---|
| `limit` | 10 | every search | Maximum number of suggestions. |
| `contexts` | `None` | every search | Only documents tagged with any of these contexts. |
| `ignore_missing_layers` | `False` | layered searches | Search a layer whose index does not exist as empty instead of raising `LayerNotFoundError`. |
| `fusion` | `"rrf"` | `hybrid_search` | `"rrf"`, `"weighted"` or `"lexical_first"`. |
| `rrf_k` | 60.0 | `hybrid_search` | `k` of reciprocal rank fusion. |
| `semantic_weight` | 0.5 | `hybrid_search` | Weight of the semantic score in weighted fusion. |
| `candidates` | `max(2 * limit, 20)` | `hybrid_search` | Hits taken from each side before fusing. |

## Maintenance options

| Setting | Default | Passed to | Meaning |
|---|---|---|---|
| `fanout` | 4 | `db.compact` | Same-level segments merged into one of the next level. |
| `max_segments` | 16 | `db.compact` | Above this many segments, with no tiered merge available, rebuild one base. |
| `max_hidden_fraction` | 0.25 | `db.compact` | Above this share of superseded or deleted documents, rebuild one base. |
| `until_done` | `False` | `db.compact` | Run compaction steps until none is due. |
| `keep_versions` | 10 | `db.cleanup` | Newest versions always kept. |
| `older_than_seconds` | 3600 | `db.cleanup` | Only objects older than this are deleted. |
| `lease_ttl_seconds` | 30 | `Ingestor(...)` | Lifetime of the ingestor lease; renewed after a third of it. |
| `max_change_sets` | 1000 | `Ingestor(...)` | Change sets folded into one commit at most. |
| `compact` | `True` | `Ingestor(...)` | Compact touched indexes after each commit. |

## Example

```python
import completr

db = completr.connect("./data", vector_bits=3, build_threads=0)
txn = db.begin()
txn.append("songs", [{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
txn.commit()

engine = db.engine(sync_every=2.0, popularity_weight=0.6, short_query_cache_entries=50_000, overfetch=3)
print(engine.complete("bi", ["songs"], limit=5))

db.compact("songs", fanout=8, until_done=True)
db.cleanup(keep_versions=20, older_than_seconds=6 * 3600)
```

```text
[Suggestion(id='billie', text='Billie Jean – Michael Jackson', score=0.8238, kind=MatchKind.PREFIX, layer='songs')]
```

## Rust

`BuildOptions`, `IndexOptions`, `SearchOptions`, `HybridOptions`, `CompactionPolicy` and `CleanupPolicy`
expose the same settings through chainable setters named after the fields:

```rust
use std::time::Duration;
use completr::{BuildOptions, CleanupPolicy, CompactionPolicy, IndexOptions, SearchOptions};

let build = BuildOptions::default().vector_bits(3).build_threads(0);
let index = IndexOptions::default().max_score(742.0).popularity_weight(0.6);
let search = SearchOptions::new(5).contexts(["pop"]);
let compaction = CompactionPolicy::default().fanout(8);
let cleanup = CleanupPolicy::default().keep_versions(20).older_than(Duration::from_secs(6 * 3600));
```

Build options go to `Database::with_build_options`, index options to `Replica::new`, and the sync interval
to `Replica::follow`. `Engine::with_overfetch` sets the overfetch.

`IndexOptions` also has `carry_short_queries` (1000), the number of an old index's most-served short
queries a replica recomputes on the new index before publishing it, and `warm_on_load` (`true`), which
maps every page of new segments before they are published.

## Collections

The [collections](collections.md) layer has settings of its own.

### Client options

Passed to `completr.Client(url, ...)` and `completr.AsyncClient(url, ...)`, together with the
[build options](#build-options).

| Setting | Default | Meaning |
|---|---|---|
| `url` | `"memory://"` | As for `connect`. |
| `sync_every` | 5.0 | Seconds between background syncs. `None` syncs only on `client.sync()`. |
| `cache_dir` | `None` | As for `connect`. |
| `options` | `None` | As for `connect`. |

### Collection settings

Passed to `create_collection` and `get_or_create_collection`, and stored with the collection when it is
created. Opening an existing collection keeps its stored settings.

| Setting | Default | Meaning |
|---|---|---|
| `optimize` | `"auto"` | `"auto"` merges segments in the background after writes; `"off"` only on `optimize()`. |
| `min_interval` | 30.0 | With `"auto"`, seconds between checks for a due merge, per collection and client. |
| `fanout` | 4 | Same-level segments merged into one of the next level. |
| `max_segments` | 16 | Above this many segments, with no tiered merge available, rebuild one segment. |
| `max_hidden_fraction` | 0.25 | Above this share of superseded or deleted documents, rebuild one segment. |

`fanout`, `max_segments` and `max_hidden_fraction` also apply to `optimize()`.

### Query options

Passed to `collection.complete(query, ...)`.

| Setting | Default | Meaning |
|---|---|---|
| `limit` | 10 | Maximum number of suggestions. |
| `aliases` | `False` | Also match synonyms, ranked below direct matches. |
| `layers` | `None` | Collections searched on top of this one, later ones overriding earlier ones per id. |
| `contexts` | `None` | Only documents tagged with any of these contexts. |
| `vector` | `None` | An embedding of the query, fused with the text results by reciprocal rank. |

```python
client = completr.Client("./data", sync_every=2.0)
songs = client.get_or_create_collection("songs", fanout=8, min_interval=60.0)
print(songs.complete("bi", limit=5, aliases=True))
```

```text
[Suggestion(id='billie', text='Billie Jean – Michael Jackson', score=0.5921, kind=MatchKind.PREFIX, layer='songs')]
```

In Rust, `ConnectOptions` and `Query` have chainable setters named after their fields, and `Optimize` is
an enum. `ConnectOptions` also takes `IndexOptions`, which Python's `Client` does not expose.

```rust
use std::time::Duration;
use completr::{BuildOptions, CompactionPolicy, ConnectOptions, IndexOptions, Optimize, Query};

let options = ConnectOptions::default()
    .sync_every(Some(Duration::from_secs(2)))
    .cache_dir(Some("/var/cache/completr".into()))
    .storage(vec![("aws_region".to_owned(), "eu-central-1".to_owned())])
    .build(BuildOptions::default().vector_bits(3).build_threads(0))
    .index(IndexOptions::default().popularity_weight(0.6));
let optimize = Optimize::Auto {
    policy: CompactionPolicy::default().fanout(8),
    min_interval: Duration::from_secs(60),
};
let query = Query::default().limit(5).aliases(true);
```
