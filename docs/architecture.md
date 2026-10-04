# Architecture

This document describes how completr stores, searches and updates indexes. completr is serverless: the engine
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
segment involves no parsing or copying beyond a few structural checks.

```
magic "COMPLETR\0\0" | version | settings | layout | max score
ids (sorted, blocked) | weights | text shapes (length, single word; bit-packed) | deletes
document store     zstd blocks of 128 documents' aliases and contexts, with block offsets
texts              original texts, read in place for suggestions
keys               string keys, empty for numeric ids
vectors            optional: TurboQuant codes, scales and slot mapping
titles             documents sorted by lowercased title, with every 64th key sampled
aliases            dictionary: alias -> postings (synonyms and abbreviations)
contexts           dictionary: context tag -> postings
words              title words in key order, with every 64th key sampled, postings and frequencies
variants           SymSpell delete variants hashed into buckets of word groups
xxh3 checksum of everything above
```

- **Postings.** A key with a single posting stores it inline in its dictionary value (bit 63 set).
  Otherwise the value is the first bit of its list: the count, then gaps in blocks of 128, each block
  bit-packed at its own width, as in tantivy and Lucene.
- **Titles.** A prefix search over titles is two binary searches over the documents in title order: the
  sampled keys narrow each to 64 positions, whose keys are decoded from the texts.
- **Variants.** Delete variants come from a word's first 7 chars, so the words sharing those chars,
  adjacent in key order, form one group with one set of variants (on Wikipedia, 1.6M groups for 2.2M
  words). Each group's variants are hashed into about three buckets per group; the entries
  `bucket * words + first ordinal` form one sorted sequence, stored with Elias-Fano coding (as in Lucene
  and PISA) at about 20 bits per entry, and a bit per word marks where groups start. No variant is
  stored, so a lookup checks that the query variant really is a subsequence of the group's words.
- **Blocked columns.** Ids and text offsets are stored in blocks of 128, each bit-packed as offsets from
  a line through the block, as in tantivy's blockwise linear codec; any value reads in constant time.
- **Section order.** The sections that need the documents' texts come first, so a build or merge frees
  the texts before it computes the variants, its largest step.
- **Max score.** Each segment stores the 99th-percentile exact-match score of its documents, so an index
  of one segment with every document live opens without reading its columns.
- **Key order.** Keys are stored as `text + 0xff`, so a key sorts after all of its extensions. This lets
  a prefix scan emit shorter completions in a stable order.
- **Validation.** `Segment::open` checks the structure only (magic, version, section lengths and
  dictionary headers) and reads nothing else, so opening is about a millisecond and touches few pages.
  `Segment::verify`, `Segment::from_bytes` and segments downloaded into a store's cache also check the
  checksum and every offset and posting. Document blocks are decompressed lazily, only when a document is
  fetched.

### Dictionaries

completr fixes the dictionary per key set (`Layout`, recorded in the segment) instead of making it a user
option:

| Key set | Dictionary | Why |
|---|---|---|
| titles | none: documents sorted by lowercased text | keys are read back from the stored texts, so titles are not stored twice |
| aliases | LOUDS trie | scanned by prefix on every keystroke; smaller and faster to scan |
| words | none: the sorted word texts | binary search over sampled keys and then the texts, as in tantivy's sstable dictionary; the texts are needed for spelling corrections anyway |

The trie is written from scratch in the style of [marisa-trie](https://github.com/s-yata/marisa-trie):
- path-compressed edges (a label plus a tail), with tails shared when one is a suffix of another;
- terminal and leaf flags;
- a `select0` index sampled every 64 zeros, with running ranks;
- zero-copy reads.

Loading validates the structure in one byte-wise pass. The optional `compact_keys` setting stores tails
in a nested trie, which is smaller but slower.

## Building a segment

`SegmentBuilder` keeps what it is given compactly: every text in one byte arena and a 32-byte entry per
document, with keys, aliases, contexts and vectors boxed only for documents that have them. Building then
runs in a fixed order, each step freeing what the next does not need:

1. **Order** the entries by id, keeping the last added of each id.
2. **Scan** each text once: lowercase it for the title key, and intern its words into a table of postings
   and occurrences, without allocating per occurrence.
3. **Sort** the words into term-key order, in flat arrays for keys and postings.
4. **Write** the sections in file order. Each is produced only when it is written: the spelling variants,
   for instance, are generated twice per word, once to count each bucket and once to place the entries, so
   no intermediate list exists. A section's bytes pass to the output a megabyte at a time.

With `SegmentBuilder::write`, the output is the file itself, hashed as it is written and then mapped, so the
encoded segment is never held in memory. The bytes are the same as those of an in-memory build.

`SegmentWriter` bounds memory for any corpus size. `SegmentBuilder::memory_bytes` estimates the peak of
building what is staged, from its text bytes and document count with factors measured on short names,
titles and paragraphs, erring high; the writer starts a new segment file once the estimate reaches its
budget. tantivy bounds its indexing the same way, flushing a segment when its writer's memory budget fills,
and Meilisearch by spilling sorted chunks to disk. The segments rank exactly like one.

## Merging segments

Compaction merges segments without rebuilding them from documents, the way tantivy's merger works:

1. **Map** the live documents of every segment to their places in the merged one, in id order. A
   segment's places ascend, so they are kept in a blocked column of a few bits per document; the
   merged order is kept only until the sections in document order are written.
2. **Union** each key set (words, aliases, contexts) across the segments in key order, with
   each key's postings remapped and those of hidden documents dropped; the title orders merge the same
   way, by keys read from the texts, each written out as it is merged. Word occurrences in hidden
   documents are subtracted, and words left without a live document are dropped.
3. **Copy** the per-document columns, the stored aliases and contexts, and the vector codes.
4. **Recompute** only what depends on the whole: spelling variants from the merged words, and the FSST
   table, trained on a sample of the merged texts read one at a time.

The merged segment is byte for byte what building from the live documents gives, which the tests check,
with less memory and time because no text is tokenised, lowercased or interned again.

## Query path

1. **Normalise** the query: Unicode lowercase and trim; words split on Unicode whitespace.
2. **Short queries** (up to `short_query_chars`, default 3) are answered from a per-index cache of
   `short_query_limit` results, computed on first use. A replica recomputes the old index's most-served
   entries on the new index before publishing it.
3. **Candidates**, merged across segments at the primitive level:
   - exact and prefix matches from a cursor merge over the title dictionaries;
   - infix matches through the word dictionaries;
   - abbreviation aliases, which match exactly;
   - fuzzy candidates from delete variants, verified with optimal string alignment distance
     ([rapidfuzz](https://github.com/rapidfuzz/rapidfuzz-rs)).

   Superseded and deleted ids are skipped through per-segment live masks. Because the merge happens below
   scoring, an index of many segments ranks exactly like one compacted segment.
4. **Score.**
   - Base score by match kind: exact 150, prefix 90, infix 30, fuzzy `20 * 0.15^(distance - 1)` plus a
     rank bonus and a quarter of the corrected query's own score for the document, without popularity.
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
