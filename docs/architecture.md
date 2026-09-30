# Architecture

This document describes how strato stores, searches and updates indexes. strato is serverless: the engine
runs inside each application process, and the only shared component is the store. For usage, see
[Getting started](getting-started.md) and the guides.

## Model

| Type | Role |
|---|---|
| `Document` | `id` (or a string `key` hashed to it), `text`, `popularity` in `[0, 1]`, aliases (synonyms or abbreviations), `contexts` tags and an optional embedding |
| `Segment` | Immutable, self-contained set of documents and deletions, one file |
| `Index` | Segments ordered oldest to newest. A newer copy of an id supersedes older ones, and a segment's deletes hide ids in older segments |
| `Engine` | Indexes by name, replaced atomically; searches run over a list of names as override layers |
| `Database` | Versioned manifests of named indexes in an object store |

The library has no notion of tenants, languages or domains. Callers express those as index names, for
example `tenant/language`, and as layer lists.

## Segment format

A segment is one file. Every section is 8-byte aligned and read in place from a memory map, so opening a
segment involves no parsing or copying beyond validation.

```
magic "STRATO\0\0" | version | settings | layout
ids (sorted) | weights | text lengths | single-word flags | deletes
document store     zstd blocks of 128 documents' aliases and contexts, with block offsets
titles             dictionary: normalised title -> postings
words              dictionary: title word -> postings, with word frequencies and texts
forward index      per document, the ordinals of its words
aliases            dictionary: alias -> postings (synonyms and abbreviations)
variants           dictionary: SymSpell delete variant -> word ordinals
vectors            optional: TurboQuant codes, scales and slot mapping
texts              original texts, read in place for suggestions
keys               string keys, empty for numeric ids
contexts           dictionary: context tag -> postings
xxh3 checksum of everything above
```

- **Postings.** A key with a single posting stores it inline in its dictionary value (bit 63 set).
  Otherwise the value packs `start << 24 | len` into a shared postings column.
- **Key order.** Keys are stored as `text + 0xff`, so a key sorts after all of its extensions. This lets
  a prefix scan emit shorter completions in a stable order.
- **Validation.** The checksum is verified on load, then every column length, offset table and
  dictionary structure is checked, so later reads cannot go out of bounds. Document blocks are
  decompressed lazily, only when a document is fetched.

### Dictionaries

strato fixes the dictionary per key set (`Layout`, recorded in the segment) instead of making it a user
option:

| Key set | Dictionary | Why |
|---|---|---|
| titles, aliases | LOUDS trie | scanned by prefix on every keystroke; smaller and faster to scan |
| words, variants | [`fst`](https://crates.io/crates/fst) | exact lookups only, where FSTs are fastest |

The trie is written from scratch in the style of [marisa-trie](https://github.com/s-yata/marisa-trie):
- path-compressed edges (a label plus a tail), with tails shared when one is a suffix of another;
- terminal and leaf flags;
- a `select0` index sampled every 64 zeros, with running ranks;
- zero-copy reads.

Loading validates the structure in one byte-wise pass. The optional `compact_keys` setting stores tails
in a nested trie, which is smaller but slower.

## Query path

1. **Normalise** the query: Unicode lowercase and trim; words split on Unicode whitespace.
2. **Short queries** (up to `short_query_chars`, default 3) are answered from a per-index cache of
   `short_query_limit` results, computed on first use. A replica recomputes the old index's most-served
   entries on the new index before publishing it.
3. **Candidates**, merged across segments at the primitive level:
   - exact and prefix matches from a cursor merge over the title dictionaries;
   - infix matches through the word dictionaries;
   - abbreviation aliases, which match exactly;
   - fuzzy candidates from delete variants, verified with Levenshtein distance
     ([rapidfuzz](https://github.com/rapidfuzz/rapidfuzz-rs)).

   Superseded and deleted ids are skipped through per-segment live masks. Because the merge happens below
   scoring, an index of many segments ranks exactly like one compacted segment.
4. **Score.**
   - Base score by match kind: exact 150, prefix 90, infix 30, fuzzy `20 * 0.15^(distance - 1)` plus a
     rank bonus.
   - A length penalty of 0.1 per character, and +25 when a single-word query matches a whole word of a
     multi-word title.
   - A popularity multiplier, `1 + weight * popularity_weight * 10`.
   - Normalisation by `max_score`, then fuzzy hits rescaled to rank below the weakest direct match.
5. **Order** by score, then shorter text, then id. The same inputs always give the same output.
6. **Suggestions** read each result's text in place and mark the matched ranges: word prefixes, then
   corrections within the edit distance, then words that spell out run-together input.

**Context filters.** A request with `contexts` builds a bitset of the documents tagged with any of them and
applies it next to the live masks, so scans continue past excluded documents. Filtered requests bypass the
short-query and recursion caches.

Layered searches (`Engine`) run each layer with an overfetch, let later layers override earlier ones per
id, and merge by the same order.

### Vectors and hybrid search

- **Storage.** Embeddings are quantised with TurboQuant ([turbovec](https://crates.io/crates/turbovec)) at
  2, 3 or 4 bits. The rotation seed is fixed, so codes from different segments are compatible, and
  compaction concatenates live codes without the original floats.
- **Search** runs per segment with the live mask, then merges the top k. Vector scores do not depend on the
  rest of the index, so the merge is exact.
- **Hybrid search** takes `candidates` hits from each side and fuses them with reciprocal rank fusion, a
  weighted sum, or lexical-first ordering, with ties broken by id.

## Databases

A database is a prefix in an object store. The layout follows [Lance](https://github.com/lancedb/lance):

```
_versions/00000000000000000001.json    manifest: indexes -> segments, metadata, parent version
segments/<uuid>.seg                    immutable segment files
_locks/<name>/<generation>             lease generations
_inbox/<id>.batch                      submitted change sets
_rejected/<id>.batch                   change sets that could not be decoded
```

- **Commits.** A commit writes manifest `N + 1` create-only (`If-None-Match: *` on S3, exclusive create
  elsewhere). No lock is needed for correctness.
- **Conflicts.** A commit that loses a race rebases onto the newer version. Appends, drops and metadata
  changes rebase with last-writer-wins per id. Overwrites, and everything in strict mode, fail with a
  conflict. Compactions rebase if the segments they replace are still present.
- **Compaction** is tiered: `fanout` same-level segments merge into one segment of the next level. The
  index is rebuilt into a single base when more than `max_hidden_fraction` of its documents are superseded
  or deleted, or when no tiered merge is available and there are more than `max_segments` segments.
- **Cleanup** keeps the newest `keep_versions` manifests and deletes older ones, plus segment files no
  retained manifest references. It only touches objects older than a retention window, measured on the
  store's clock.
- **Leases** are advisory, expiring locks. Every acquire or renew creates the next generation file
  create-only, so leases work on every backend, and the generation serves as a fencing token.

### Single ingestor

Many writers committing directly contend on the manifest. Instead:

1. Any process builds its changes into segments and writes them create-only to `_inbox/` as a change set.
   Change-set ids are monotonic within a process, so a process's change sets apply in submission order.
2. Every process may run an `Ingestor`, and only the one holding the `ingestor` lease acts. It folds
   pending change sets into one segment per index and commits them as one version.
3. The commit is fenced by the lease generation, and it records the applied change-set ids. An ingestor
   that lost its lease cannot commit, and a change set left over after a crash is never applied twice.
4. After committing, the ingestor compacts the indexes it touched.

### Replicas

A `Replica` polls for newer manifests. It lists only keys after its current version, downloads only
segments it does not hold, rebuilds only the changed indexes, and publishes them to an `Engine`.

With `group_separator`, it publishes one group of indexes at a time and releases replaced segments before
loading the next group. Serving memory therefore holds at most one group twice, never the whole database.

Build and compaction peaks belong to the ingestor, so run ingestors outside serving processes.

## Storage backends

Stores go through [`object_store`](https://crates.io/crates/object_store): local paths, `file://`,
`memory://`, `s3://`, `gs://` and `az://`.

- **S3 credentials.** With `aws-credentials`, they resolve through the AWS SDK chain: environment,
  shared config and credentials files, SSO, web identity, ECS and IMDS. Explicit options override them.
- **Disk cache.** An optional cache keeps downloaded segments on local disk and memory-maps them.
  Segment files are immutable, so cached copies are used without a round trip.
