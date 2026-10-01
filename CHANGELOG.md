# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- The project is renamed from strato to completr: crates `completr`, `completr-cli` and `completr-py`, the
  Python package `completr`, the `completr` command, `COMPLETR_*` environment variables, and new segment
  and inbox file markers, so files written under the old name must be rebuilt.
- Segment format 10: spelling variants are hashed into buckets instead of a dictionary, postings and text
  offsets are bit-packed, and the per-document forward index is gone. Segments are about 40% smaller and
  build about a third faster; older segments must be rebuilt.
- `Segment::open` checks a local file's structure only and opens in about a millisecond; the new
  `Segment::verify` (and `Segment.verify()` in Python) checks the checksum and every section, as
  `from_bytes` and downloads into a store's cache do.
- The vector index is built on the first vector search or on `warm`, not when a segment is opened.
- Corrected results are ranked by how well the corrected query matches them, and the word being typed is
  corrected when the query as typed matches nothing.
- Multi-word queries intersect postings through a bitset, roughly halving tail latency.
- Segment builds stream: `SegmentBuilder` keeps documents in a text arena with a small entry each,
  sections are written to the file as they are produced (`SegmentBuilder::write`, `Segment.build(path=...)`
  in Python), and spelling variants are generated without allocating, counted, then placed. Building the
  same segment takes about half the time and half the peak memory, with identical bytes.

### Added

- Suggestions carry the document's text, string key and highlight ranges.
- String ids: `Document::keyed` and `key_id` in Rust; `id` may be a `str` in Python.
- Context filters: tag documents with `contexts` and pass `contexts` to any request.
- Python: documents as dicts, `completr.Document`, or pandas, polars and Arrow tables; `completr.connect`,
  `Database.engine()` with `Engine.sync()`, `Database.open_index`, `Index.from_documents`, an asyncio API
  (`connect_async`) and an exception hierarchy under `CompletrError`.
- The `completr` command-line tool (`completr-cli`): inspect, import, complete, compact, cleanup and ingest.
- `tracing` events for commits, replica syncs, ingest rounds, compaction, cleanup and segment builds; in
  Python they reach the `logging` module under loggers named `completr.*`.
- Rust: `SearchOptions` and `complete_with`, `complete_aliases_with`, `vector_search_with`;
  `Index::from_documents` and `Index::document_by_key`.

### Fixed

- Typo'd queries with a word shorter than `min_word_chars` (such as "welcoem to") found nothing: short
  words and the word being typed are no longer "corrected" into other words. On Hacker News titles, MRR on
  typo'd typing rose from 0.38 to 0.70, and prefix p99 fell from 2.8 to 1.7 ms.
- Spelling correction counts a transposition as one edit (optimal string alignment, as SymSpell intends),
  corrects the word being typed against word prefixes, prefers frequent corrections, and tries the
  runner-up corrections and common neighbours of rare real words. On Hacker News titles, MRR on typo'd
  typing rose from 0.70 to 0.81 (popular targets) and from 0.64 to 0.75 (uniform targets).
- Direct matches are no longer reported as `fuzzy` when a correction also reaches them.
- A later layer that renames or deletes a document now hides the earlier layer's version even when its own
  version does not match the query.
- Short query words must start a word of the text, so `data sc` no longer completes every "data" title.

### Changed

- Segment format v9: texts, keys and contexts are stored; v8 segments must be rebuilt. Texts and keys are
  FSST-compressed and decompressed one at a time, so a segment of Hacker News titles is 3 % larger than
  in v8 while every suggestion carries its text.
- Public structs and enums are `#[non_exhaustive]`; options are set with chainable setters.
- `SegmentConfig` is now `BuildOptions`, `IndexConfig` is `IndexOptions`, `Error::Format` is
  `Error::Corrupt`, `Document::weight` is `popularity`, `LayeredSuggestion::hit` is `suggestion`, and
  `Database::load_index` is `open_index`. `hybrid_search` takes `&HybridOptions`.
- Python documents are no longer tuples, and `Index.get` returns a `Document`.
- Presented as a serverless autocompletion engine: new logo, animated demo and README.
- Renamed the API to say what each part does: `Dataset` is now `Database`, `Batch` is `ChangeSet`,
  `Writer` is `Ingestor` (`WriterStep` is `IngestStep`, with `NotLeader` now `Standby` and `is_leader`
  now `is_active`), and `Follower` is `Replica`. `Hit`, `AliasHit`, `HybridHit` and `LayeredHit` are now
  `Suggestion`, `AliasSuggestion`, `HybridSuggestion` and `LayeredSuggestion`. `autocomplete` and
  `search_aliases` are now `complete` and `complete_aliases`.

## [0.1.0]

First public release.

### Added

- Immutable, memory-mapped segments (format v8) with an xxh3 checksum and structural validation on
  load.
- Search: exact, prefix, abbreviation, infix and fuzzy matching, synonym alias search, and a
  short-query cache.
- LOUDS trie and FST key dictionaries, with the layout chosen per key set.
- Indexes over many segments, with results identical to a compacted index.
- `Engine` with atomic publishing and override layers.
- Vector search with TurboQuant (2, 3 or 4 bits), and hybrid search with reciprocal rank, weighted or
  lexical-first fusion.
- Databases on local disk, S3, GCS, Azure and memory: versioned manifests, optimistic transactions,
  tiered compaction, cleanup and leases.
- Inbox with a lease-elected single `Ingestor`, and a `Replica` that loads only changed segments and
  switches group by group.
- Python bindings (abi3, CPython 3.11+) with type stubs.
