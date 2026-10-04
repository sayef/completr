# Rust API

The Rust API reference is on [docs.rs/completr](https://docs.rs/completr). Enable the `store` feature for
databases, and `aws` (or `aws-credentials`), `gcp` or `azure` for cloud storage. The database API is async
and runs on Tokio. From synchronous code, `completr::block_on` runs one of its futures on completr's
internal runtime; do not call it from async code.

```toml
[dependencies]
completr = { version = "0.1", features = ["store"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Indexes and engines

| Item | Feature | Role |
|---|---|---|
| [`Document`](https://docs.rs/completr/latest/completr/struct.Document.html) | | A document: `Document::new(id, text, popularity)` or `Document::keyed(key, text, popularity)`, with `with_synonym`, `with_abbreviation`, `with_context` and `with_vector`. |
| [`Index`](https://docs.rs/completr/latest/completr/struct.Index.html) | | Segments searched as one: `from_documents`, `complete`, `complete_aliases`, `vector_search`, `hybrid_search`, and the `_with` variants taking options. |
| [`Segment`](https://docs.rs/completr/latest/completr/struct.Segment.html), [`SegmentWriter`](https://docs.rs/completr/latest/completr/struct.SegmentWriter.html) | | An immutable, memory-mapped set of documents and deletes; and a writer of segment files in bounded memory. |
| [`Engine`](https://docs.rs/completr/latest/completr/struct.Engine.html) | | Indexes by name, published atomically, searched as layers. Returns `LayeredSuggestion`s. |
| [`Suggestion`](https://docs.rs/completr/latest/completr/struct.Suggestion.html), [`AliasSuggestion`](https://docs.rs/completr/latest/completr/struct.AliasSuggestion.html), [`HybridSuggestion`](https://docs.rs/completr/latest/completr/struct.HybridSuggestion.html) | | Results, with `id`, `key`, `text`, `score` and, for completions, `kind` and byte-range `highlights`. |
| [`MatchKind`](https://docs.rs/completr/latest/completr/enum.MatchKind.html), [`Fusion`](https://docs.rs/completr/latest/completr/enum.Fusion.html) | | Match kinds, and hybrid fusion methods. |
| [`BuildOptions`](https://docs.rs/completr/latest/completr/struct.BuildOptions.html), [`IndexOptions`](https://docs.rs/completr/latest/completr/struct.IndexOptions.html), [`SearchOptions`](https://docs.rs/completr/latest/completr/struct.SearchOptions.html), [`HybridOptions`](https://docs.rs/completr/latest/completr/struct.HybridOptions.html) | | Settings, with chainable setters. |
| [`Error`](https://docs.rs/completr/latest/completr/enum.Error.html) | | `Io`, `Corrupt`, `InvalidInput`, `NotFound`, `Conflict` and `Store`, among others. |

```rust
use std::sync::Arc;
use completr::{AliasKind, Document, Index, IndexOptions, MatchKind, Segment};

let docs = [
    Document::new(1, "Under the Bridge – Red Hot Chili Peppers", 0.75)
        .with_alias("RHCP", AliasKind::Abbreviation),
    Document::new(2, "Bohemian Rhapsody – Queen", 0.95),
];
let index = Index::new(vec![Arc::new(Segment::build(docs, [])?)], IndexOptions::default())?;
let hits = index.complete("rhcp", 10);
assert_eq!((hits[0].id, hits[0].kind), (1, MatchKind::Abbreviation));
```

## Databases

| Item | Feature | Role |
|---|---|---|
| [`Database`](https://docs.rs/completr/latest/completr/struct.Database.html) | `store` | Versioned manifests of named indexes in a store: `Database::open(url, options)`, `begin`, `open_index`, `compact`, `cleanup`, `acquire_lease`, `submit`. |
| [`Transaction`](https://docs.rs/completr/latest/completr/struct.Transaction.html) | `store` | An optimistic, atomic commit. |
| [`Replica`](https://docs.rs/completr/latest/completr/struct.Replica.html) | `store` | Publishes a database's latest version into an `Engine`: `sync`, and [`follow`](https://docs.rs/completr/latest/completr/struct.Replica.html#method.follow), which syncs from a background thread. |
| [`Follower`](https://docs.rs/completr/latest/completr/struct.Follower.html), [`FollowStatus`](https://docs.rs/completr/latest/completr/struct.FollowStatus.html) | `store` | The handle `Replica::follow` returns; dropping it stops the thread. `status()` returns `synced_at_ms` and `error`. |
| [`Lease`](https://docs.rs/completr/latest/completr/struct.Lease.html) | `store` | An expiring, fenced lock in the database. |
| [`ChangeSet`](https://docs.rs/completr/latest/completr/struct.ChangeSet.html) | `store` | Upserts and deletes submitted to the inbox. |
| [`Ingestor`](https://docs.rs/completr/latest/completr/struct.Ingestor.html), [`IngestStep`](https://docs.rs/completr/latest/completr/enum.IngestStep.html) | `store` | The lease-elected committer of change sets, and the result of a round. |
| [`CompactionPolicy`](https://docs.rs/completr/latest/completr/struct.CompactionPolicy.html), [`CleanupPolicy`](https://docs.rs/completr/latest/completr/struct.CleanupPolicy.html) | `store` | Maintenance settings. |
| [`Store`](https://docs.rs/completr/latest/completr/struct.Store.html), [`BlockingStore`](https://docs.rs/completr/latest/completr/struct.BlockingStore.html) | `store` | Object-store access, async and blocking. |

```rust
use std::{sync::Arc, time::Duration};
use completr::{Database, Document, Engine, IndexOptions, Replica};

#[tokio::main]
async fn main() -> Result<(), completr::Error> {
    let database = Database::open("./data", Vec::<(String, String)>::new()).await?;
    let mut txn = database.begin().await?;
    txn.append_documents("songs", [Document::keyed("bohemian", "Bohemian Rhapsody – Queen", 0.95)], [])?;
    txn.commit().await?;

    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database, IndexOptions::default()));
    replica.sync(&engine).await?;
    let _follower = replica.follow(&engine, Duration::from_secs(5));   // syncs until dropped
    let hits = engine.complete(&["songs"], "boh", 10);
    Ok(())
}
```

Runnable examples:
[`quickstart.rs`](https://github.com/sayef/completr/blob/main/crates/completr/examples/quickstart.rs) for an
index and a database, and
[`live_updates.rs`](https://github.com/sayef/completr/blob/main/crates/completr/examples/live_updates.rs)
for a followed database with an ingestor.

## Collections

The [collections](../guides/collections.md) layer. In Rust, `completr::connect` returns this client; a
database is opened with `Database::open`.

| Item | Feature | Role |
|---|---|---|
| [`connect`](https://docs.rs/completr/latest/completr/fn.connect.html), [`ConnectOptions`](https://docs.rs/completr/latest/completr/struct.ConnectOptions.html) | `store` | Opens a client on the database at a URL: `sync_every`, `cache_dir`, `storage` options, `build` and `index` options. |
| [`Client`](https://docs.rs/completr/latest/completr/struct.Client.html) | `store` | A database and its collections: `collections`, `collection`, `create_collection`, `get_or_create_collection`, `drop_collection`, `sync`, and `database()`. |
| [`Collection`](https://docs.rs/completr/latest/completr/struct.Collection.html) | `store` | Named documents: `add`, `delete`, `complete`, `optimize` and `stats`. |
| [`Query`](https://docs.rs/completr/latest/completr/struct.Query.html) | `store` | `limit`, `aliases`, `layers`, `contexts` and `vector`, with chainable setters. |
| [`Completion`](https://docs.rs/completr/latest/completr/struct.Completion.html) | `store` | A result: `id`, `key`, `text`, `score`, `kind`, byte-range `highlights` and `collection`. |
| [`CollectionStats`](https://docs.rs/completr/latest/completr/struct.CollectionStats.html) | `store` | Size and freshness, from `Collection::stats`. |
| [`Optimize`](https://docs.rs/completr/latest/completr/enum.Optimize.html) | `store` | `Auto { policy, min_interval }` or `Off`. |

```rust
use completr::{connect, ConnectOptions, Document, MatchKind, Optimize, Query};

#[tokio::main]
async fn main() -> Result<(), completr::Error> {
    let client = connect("memory://", ConnectOptions::default()).await?;
    let songs = client.get_or_create_collection("songs", Optimize::default()).await?;
    songs
        .add([
            Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75)
                .with_abbreviation("RHCP"),
        ])
        .await?;
    let hits = songs.complete("rhcp", &Query::default())?;
    assert_eq!((hits[0].key.as_deref(), hits[0].kind), (Some("bridge"), MatchKind::Abbreviation));
    Ok(())
}
```
