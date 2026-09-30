<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/sayef/strato/main/assets/banner-dark.svg">
    <img alt="strato: layered autocompletion for Rust and Python" src="https://raw.githubusercontent.com/sayef/strato/main/assets/banner-light.svg" width="600">
  </picture>
</p>

<p align="center">
  <b>Typeahead search that stays fast while your data changes.</b><br>
  Prefix, infix, fuzzy, abbreviation, semantic and hybrid matching over immutable, memory-mapped
  segments, with incremental updates on local disk, S3, GCS or Azure.
</p>

<p align="center">
  <a href="https://github.com/sayef/strato/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/sayef/strato/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://crates.io/crates/strato"><img alt="crates.io" src="https://img.shields.io/crates/v/strato.svg"></a>
  <a href="https://docs.rs/strato"><img alt="docs.rs" src="https://img.shields.io/docsrs/strato"></a>
  <a href="https://pypi.org/project/strato/"><img alt="PyPI" src="https://img.shields.io/pypi/v/strato.svg"></a>
  <img alt="Python 3.11+" src="https://img.shields.io/badge/python-3.11%2B-blue">
  <a href="https://github.com/sayef/strato/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-green.svg"></a>
</p>

---

strato is an embeddable autocompletion engine written in Rust, with first-class Python bindings. You give it
documents (an id, a text, a popularity weight and optional aliases or embeddings) and it answers
search-as-you-type queries in well under a millisecond. It ranks exact, prefix, abbreviation, infix and
typo-tolerant matches, and optionally fuses them with vector search.

Indexes are built from **immutable segments**, the way Lucene and LanceDB build theirs: updates land as small delta
segments, compaction merges them in the background, and readers switch to new versions atomically. Several
indexes can be searched as **override layers**, so a tenant, a user or an experiment can shadow shared
data per document without copying it.

## Highlights

- **Fast.** p50 around 0.1 ms and p99 around 1 ms for typed queries on 200k documents with the short-query
  cache, from a single core. See [Performance](#performance).
- **Every match kind you need.** `exact`, `prefix`, `abbreviation`, `infix`, `fuzzy` (up to two edits,
  SymSpell-style), and `semantic` from embeddings. Each hit reports its kind.
- **Hybrid search, your rules.** Reciprocal rank fusion, weighted blending or lexical-first, chosen per
  request.
- **Zero-copy segments.** An aligned, checksummed binary format read in place through `mmap`: opening a
  19 MB segment takes about 6 ms, and memory is shared with the page cache.
- **Compact dictionaries.** A purpose-built LOUDS trie with shared tails for scan-heavy keys and
  finite-state transducers ([`fst`](https://crates.io/crates/fst)) for lookups. strato chooses the layout per
  key set.
- **Incremental and versioned.** Datasets are versioned manifests over object storage, with
  optimistic transactions, conflict detection, tiered compaction and cleanup of old versions.
- **One writer, many readers.** Any process can submit batches to an inbox, and a lease-elected writer
  commits them in order. Followers load only the changed segments and publish them without doubling memory.
- **Deterministic.** Identical inputs always give bit-identical results and order, with ties broken by id,
  independent of segmentation and thread count.
- **Storage anywhere.** Local disk, S3, GCS, Azure and in-memory through
  [`object_store`](https://crates.io/crates/object_store). S3 credentials resolve like `boto3`: environment,
  profiles, SSO, web identity, ECS and IMDS. An optional disk cache is available.
- **Rust and Python.** The same engine from both. The Python wheel is abi3 for CPython 3.11+, and releases
  the GIL during searches and builds.

## Installation

```sh
pip install strato
```

```sh
cargo add strato                          # engine only
cargo add strato --features store         # datasets on local disk and in memory
cargo add strato --features aws-credentials,gcp,azure   # plus cloud object stores
```

| Cargo feature | Adds |
|---|---|
| `store` | `Dataset`, `Transaction`, `Writer`, `Follower`, local and in-memory stores |
| `aws` / `gcp` / `azure` | S3, Google Cloud Storage, Azure Blob Storage |
| `aws-credentials` | S3 credentials through the AWS SDK chain (profiles, SSO, web identity, ECS, IMDS) |

The Python wheel includes all of them.

## Quick start

### Python

```python
from strato import Engine, Index, Segment

# (id, text, weight, [(alias, is_abbreviation)])
docs = [
    (1, "Machine Learning", 0.9, [("ML", True)]),
    (2, "Machine Vision", 0.4, []),
    (3, "Data Science", 0.7, [("data analytics", False)]),
]
index = Index([Segment.build(docs)])

for query in ["mach", "ML", "vison", "science"]:
    print(query, [(h.id, h.kind, round(h.score, 3)) for h in index.autocomplete(query, limit=3)])
```

```text
mach    [(1, 'prefix', 0.596), (2, 'prefix', 0.337)]
ML      [(1, 'abbreviation', 0.596)]
vison   [(2, 'fuzzy', 0.09)]
science [(3, 'infix', 0.299)]
```

### Rust

```rust
use std::sync::Arc;
use strato::{AliasKind, Document, Index, IndexConfig, Segment};

let docs = vec![
    Document::new(1, "Machine Learning", 0.9).with_alias("ML", AliasKind::Abbreviation),
    Document::new(2, "Machine Vision", 0.4),
    Document::new(3, "Data Science", 0.7).with_alias("data analytics", AliasKind::Synonym),
];
let index = Index::new(vec![Arc::new(Segment::build(docs, [])?)], IndexConfig::default())?;

for hit in index.autocomplete("mach", 10) {
    println!("{} {} {:.3}", hit.id, hit.kind.as_str(), hit.score);
}
```

Runnable versions: [`examples/quickstart.rs`](https://github.com/sayef/strato/blob/main/crates/strato/examples/quickstart.rs) and
[`examples/quickstart.py`](https://github.com/sayef/strato/blob/main/crates/strato-py/examples/quickstart.py).

## Usage

### Layers

An `Engine` holds indexes by name and publishes new versions atomically. A search over several layers
lets later layers override earlier ones per document id, so a tenant's edits, deletions or additions
shadow the shared data.

```python
from strato import Engine

engine = Engine()
engine.publish({"shared": shared_index, "acme": acme_index})
hits = engine.autocomplete("machine", ["shared", "acme"], limit=10)   # hit.layer tells where it came from
engine.publish({"acme": None})                                         # remove a layer
```

### Semantic and hybrid search

Documents can carry embeddings from any model. strato stores them quantised with
[TurboQuant](https://crates.io/crates/turbovec) (4 bits by default, 2 or 3 optional) and searches them with
deleted documents masked out.

```python
import numpy as np

vectors = embed([text for _, text, _, _ in docs]).astype(np.float32)   # shape (n, dim)
index = Index([Segment.build(docs, vectors=vectors, vector_bits=4)])

index.vector_search(embed(["deep learning"])[0], limit=10)             # kind == "semantic"
index.hybrid_search("deep lea", embed(["deep lea"])[0], limit=10,
                    fusion="rrf")                                      # or "weighted", "lexical_first"
```

| Fusion | Score |
|---|---|
| `rrf` (default) | `1 / (k + lexical_rank) + 1 / (k + semantic_rank)`, `k = 60`, no calibration needed |
| `weighted` | `(1 - w) * lexical + w * max(semantic, 0)`, set `semantic_weight` |
| `lexical_first` | lexical hits in their order, then semantic-only hits |

A hybrid hit keeps its lexical match kind when it matched lexically, and reports both source scores.

### Datasets and live updates

A `Dataset` is a versioned collection of named indexes in a directory or bucket. Transactions commit
atomically, and concurrent commits rebase or fail with `ConflictError` in strict mode.

```python
from strato import Batch, Dataset, Engine, Follower, Writer

dataset = Dataset("s3://my-bucket/search")          # or a local path, gs://..., az://..., memory:///...

txn = dataset.begin()
txn.append("products", [(i, f"product {i}", 0.5, []) for i in range(10_000)])
txn.commit()

# Serving processes: load changed segments only, publish atomically.
engine = Engine()
follower = Follower(dataset, engine)
follower.sync()                                     # call periodically

# Any process: submit changes to the inbox.
batch = Batch()
batch.upsert("products", [(10_000, "wireless keyboard", 0.9, [])])
batch.delete("products", [42])
dataset.submit(batch)

# One process at a time commits them (lease-elected), then compacts.
writer = Writer(dataset, "writer-1")
writer.run_once()                                   # call in a loop
```

Runnable: [`live_updates.py`](https://github.com/sayef/strato/blob/main/crates/strato-py/examples/live_updates.py) and
[`live_updates.rs`](https://github.com/sayef/strato/blob/main/crates/strato/examples/live_updates.rs).

- **Compaction**: `dataset.compact(index)` merges delta segments tier by tier and rebuilds the base when
  too many documents are superseded. The writer does this automatically after committing.
- **Cleanup**: `dataset.cleanup(keep_versions=10, older_than_seconds=3600)` deletes old manifests and
  segment files that no retained version references.
- **Credentials**: S3 uses the standard AWS chain. Pass `options={"aws_access_key_id": ..., "aws_region": ...}`
  to override it, and `cache_dir=...` to keep downloaded segments on local disk.

### Match kinds

| Kind | Example: query → document |
|---|---|
| `exact` | `data science` → *Data Science* |
| `prefix` | `data sc` → *Data Science* |
| `abbreviation` | `ML` → *Machine Learning* (alias marked as abbreviation, exact only) |
| `infix` | `science` → *Data Science* |
| `fuzzy` | `data sceince` → *Data Science* (up to 2 edits, configurable) |
| `semantic` | from vector search |

Synonym aliases are searched with `search_aliases(query)`, which returns the documents they belong to.

### Configuration

| Where | Settings |
|---|---|
| `Segment.build` / `Dataset(...)` | `min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`, `compact_keys`, `build_threads` |
| `Index(...)` / `Follower(...)` | `max_score`, `popularity_weight`, `short_query_chars`, `short_query_limit`, `short_query_cache_entries`, `vector_threads` |
| `Engine(...)` | `overfetch` for layered searches |
| `hybrid_search(...)` | `fusion`, `rrf_k`, `semantic_weight`, `candidates` |
| `Dataset.compact(...)` | `fanout`, `max_segments`, `max_hidden_fraction` |
| `Writer(...)` | `lease_ttl_seconds`, `max_batches`, `compact` |

The Rust structs `SegmentConfig`, `IndexConfig`, `HybridOptions`, `CompactionPolicy` and `CleanupPolicy`
expose the same settings.

## Performance

Synthetic corpus of 200,000 documents (1 to 4 words from a 30,000-word vocabulary, Zipf-distributed),
one core of an Apple M1 Pro. Reproduce with
`cargo run --release -p strato --example bench -- 200000 --vectors`.

| Step | Result |
|---|---|
| Build a segment | 452 ms, 18.8 MB (1.1 s, 46 MB with 256-d vectors) |
| Open a segment (memory-mapped, checksum verified) | 5.6 ms |
| Build a delta segment of 1,000 upserts | 12.5 ms |

| Query, limit 10 | p50 | p99 |
|---|---|---|
| Every prefix of 2,000 titles, as typed (33,744 queries) | 0.13 ms | 3.9 ms |
| The same with the short-query cache (default) | 0.08 ms | 0.99 ms |
| The same over a base plus a delta segment | 0.18 ms | 4.2 ms |
| One-edit typos | 0.28 ms | 1.5 ms |
| Vector search, 256-d, 4-bit | 1.1 ms | 1.2 ms |
| Hybrid search (RRF) | 1.3 ms | 5.2 ms |

Latency is dominated by one- and two-character prefixes, which match large parts of the corpus. The
short-query cache serves those, and followers carry its hottest entries across index versions.

## Guarantees and testing

- **Determinism**: results depend only on the documents and the settings, not on how the index was
  segmented, compacted or queried concurrently. The test suite re-runs queries sampled under concurrent
  updates on the same snapshot and on a freshly built index, and requires bit-identical output.
- **Concurrency**: readers never block. New index versions are published atomically, and a reader keeps
  the version it started with.
- **Integrity**: every segment ends with an xxh3 checksum verified on load, and every section is validated
  before use. Stable-Rust fuzz tests feed truncated, bit-flipped and garbage segments, random unicode
  queries and corrupted manifests.
- **Memory**: followers replace indexes group by group and release old segments before loading the next
  group. In a soak test of thousands of versions, serving memory stayed flat.

```sh
cargo test --release -p strato --features store          # unit, dataset, concurrency and fuzz tests
STRATO_FUZZ_ITERS=1000000 cargo test --release -p strato --features store --test fuzz
```

## How it works

A segment stores its documents in zstd-compressed blocks and its keys in four dictionaries: titles,
title words, aliases, and SymSpell delete variants for typo tolerance. Popularity, title length and
per-document word ordinals are kept in aligned columns. Queries run on primitives merged across
segments (cursor merges over the sorted dictionaries), so a layered or segmented index ranks exactly
like a single segment. See [docs/architecture.md](https://github.com/sayef/strato/blob/main/docs/architecture.md) for the format, the ranking and the
dataset protocol.

## Status

strato is young. The on-disk format is versioned and checked on load, but it may still change before 1.0,
as may the API. Feedback and issues are very welcome.

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](https://github.com/sayef/strato/blob/main/CONTRIBUTING.md) and our
[Code of Conduct](https://github.com/sayef/strato/blob/main/CODE_OF_CONDUCT.md). To report a vulnerability, see [SECURITY.md](https://github.com/sayef/strato/blob/main/SECURITY.md).

## Acknowledgements

strato stands on excellent work: [`fst`](https://github.com/BurntSushi/fst) by Andrew Gallant,
[`rapidfuzz`](https://github.com/rapidfuzz/rapidfuzz-rs), [`turbovec`](https://crates.io/crates/turbovec),
[`object_store`](https://github.com/apache/arrow-rs-object-store), [`zstd`](https://github.com/gyscos/zstd-rs),
and [PyO3](https://github.com/PyO3/pyo3). The trie design is inspired by
[marisa-trie](https://github.com/s-yata/marisa-trie), fuzzy matching by
[SymSpell](https://github.com/wolfgarbe/SymSpell), and the dataset protocol by
[Lance](https://github.com/lancedb/lance).

## License

[MIT](https://github.com/sayef/strato/blob/main/LICENSE). Developed and maintained by
[Saiful Islam](https://github.com/sayef).
