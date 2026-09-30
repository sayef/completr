# Rust API

The Rust API reference is on [docs.rs/strato](https://docs.rs/strato). Enable the `store` feature for
databases, and `aws` (or `aws-credentials`), `gcp` or `azure` for cloud storage.

```toml
[dependencies]
strato = { version = "0.1", features = ["store"] }
```

## Main types

| Type | Feature | Role |
|---|---|---|
| [`Document`](https://docs.rs/strato/latest/strato/struct.Document.html) | | A document: `Document::new(id, text, popularity)` or `Document::keyed(key, text, popularity)`, with `with_synonym`, `with_abbreviation`, `with_context` and `with_vector`. |
| [`Segment`](https://docs.rs/strato/latest/strato/struct.Segment.html) | | An immutable, memory-mapped set of documents and deletes. |
| [`Index`](https://docs.rs/strato/latest/strato/struct.Index.html) | | Segments searched as one: `complete`, `complete_aliases`, `vector_search`, `hybrid_search`, and the `_with` variants taking options. |
| [`Engine`](https://docs.rs/strato/latest/strato/struct.Engine.html) | | Indexes by name, published atomically, searched as layers. Returns `LayeredSuggestion`s. |
| [`Suggestion`](https://docs.rs/strato/latest/strato/struct.Suggestion.html), [`AliasSuggestion`](https://docs.rs/strato/latest/strato/struct.AliasSuggestion.html), [`HybridSuggestion`](https://docs.rs/strato/latest/strato/struct.HybridSuggestion.html) | | Results, with `id`, `key`, `text`, `score` and, for completions, `kind` and byte-range `highlights`. |
| [`MatchKind`](https://docs.rs/strato/latest/strato/enum.MatchKind.html), [`Fusion`](https://docs.rs/strato/latest/strato/enum.Fusion.html) | | Match kinds, and hybrid fusion methods. |
| [`BuildOptions`](https://docs.rs/strato/latest/strato/struct.BuildOptions.html), [`IndexOptions`](https://docs.rs/strato/latest/strato/struct.IndexOptions.html), [`SearchOptions`](https://docs.rs/strato/latest/strato/struct.SearchOptions.html), [`HybridOptions`](https://docs.rs/strato/latest/strato/struct.HybridOptions.html) | | Settings, with chainable setters. |
| [`Database`](https://docs.rs/strato/latest/strato/struct.Database.html) | `store` | Versioned manifests of named indexes in a store. |
| [`Transaction`](https://docs.rs/strato/latest/strato/struct.Transaction.html) | `store` | An optimistic, atomic commit. |
| [`ChangeSet`](https://docs.rs/strato/latest/strato/struct.ChangeSet.html) | `store` | Upserts and deletes submitted to the inbox. |
| [`Ingestor`](https://docs.rs/strato/latest/strato/struct.Ingestor.html), [`IngestStep`](https://docs.rs/strato/latest/strato/enum.IngestStep.html) | `store` | The lease-elected committer of change sets, and the result of a round. |
| [`Replica`](https://docs.rs/strato/latest/strato/struct.Replica.html) | `store` | Follows a database and publishes changed indexes to an `Engine`. |
| [`CompactionPolicy`](https://docs.rs/strato/latest/strato/struct.CompactionPolicy.html), [`CleanupPolicy`](https://docs.rs/strato/latest/strato/struct.CleanupPolicy.html) | `store` | Maintenance settings. |
| [`Store`](https://docs.rs/strato/latest/strato/struct.Store.html), [`BlockingStore`](https://docs.rs/strato/latest/strato/struct.BlockingStore.html) | `store` | Object-store access, async and blocking. |
| [`Error`](https://docs.rs/strato/latest/strato/enum.Error.html) | | `Io`, `Corrupt`, `InvalidInput`, `NotFound`, `Conflict` and `Store`, among others. |

The database API is async and runs on Tokio. From synchronous code, `strato::block_on` runs one of its
futures on strato's internal runtime; do not call it from async code.

## Example

```rust
use std::sync::Arc;
use strato::{AliasKind, Document, Index, IndexOptions, MatchKind, Segment};

let docs = [
    Document::new(1, "Machine Learning", 0.9).with_alias("ML", AliasKind::Abbreviation),
    Document::new(2, "Machine Vision", 0.4),
];
let index = Index::new(vec![Arc::new(Segment::build(docs, [])?)], IndexOptions::default())?;
let hits = index.complete("ml", 10);
assert_eq!((hits[0].id, hits[0].kind), (1, MatchKind::Abbreviation));
```

Runnable examples:
[`quickstart.rs`](https://github.com/sayef/strato/blob/main/crates/strato/examples/quickstart.rs) and
[`live_updates.rs`](https://github.com/sayef/strato/blob/main/crates/strato/examples/live_updates.rs).
