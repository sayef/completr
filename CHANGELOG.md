# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

The first public release.

### Added

- **Completion**: exact, prefix, abbreviation, infix, spelling-tolerant and word-decomposing matches ranked
  together with popularity, with highlights, context filters and synonyms.
- **Semantic and hybrid search**: embeddings quantised to 2 to 4 bits, fused with lexical results by
  reciprocal rank, weighted blending or lexical-first ordering.
- **Segments**: an aligned, checksummed format read in place through `mmap`, built by streaming
  (`SegmentBuilder`), in bounded memory across files (`SegmentWriter`), and merged through their sorted
  structures, byte for byte as a rebuild would.
- **Databases** on local disk, S3, GCS, Azure or memory: versioned manifests, optimistic transactions with
  automatic retries, tiered compaction and cleanup.
- **Engines that follow a database**: `Database.engine()` picks up new versions in a background thread,
  loading only changed segments and switching versions atomically; `Replica::follow` in Rust.
- **Override layers**: a tenant's, a user's or an experiment's index searched on top of shared data, per
  document id.
- **Many writers**: change sets submitted to an inbox and committed in order by a lease-elected ingestor.
- **Collections** (`completr.Client` in Python, `completr::connect` in Rust): named collections with stored
  settings, automatic optimisation, `stats()`, and one `complete()` with synonyms, layers, contexts and
  vectors.
- **Python**: documents as dicts, `Document` objects, or pandas, polars and Arrow tables; typed stubs; an
  asyncio API (`AsyncDatabase`, `AsyncClient`); errors under `CompletrError`; logs through `logging`.
- **Command line** (`completr-cli`): inspect, import, complete, compact, cleanup and ingest.
- **Benchmarks** against tantivy, Typesense and Meilisearch on five corpora, with the harness in `bench/`.
