# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
- Datasets on local disk, S3, GCS, Azure and memory: versioned manifests, optimistic transactions,
  tiered compaction, cleanup and leases.
- Inbox with a lease-elected single `Writer`, and a `Follower` that loads only changed segments and
  switches group by group.
- Python bindings (abi3, CPython 3.11+) with type stubs.
