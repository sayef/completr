# Benchmarks

completr compared with tantivy, Typesense and Meilisearch as autocompletion engines, on five corpora:
124,440 Hacker News story titles ranked by points; all 7,235,024 English Wikipedia articles ranked by a month
of pageviews; all 40,393,216 MusicBrainz recordings, with their MBIDs as ids, ranked by ListenBrainz
listens; 39,588,974 Amazon search terms from AmazonQAC, ranked by searches and scored on 20,000 prefixes real
users typed; and 41,666,206 Open Library works with their first author, ranked by readers. Every engine indexes the same documents and answers the same queries. To reproduce,
see [`bench/README.md`](https://github.com/sayef/completr/blob/main/bench/README.md), which describes every
measurement and setting in detail.

## Summary

- **Ranking.** completr puts the wanted title highest of the engines tested on every corpus and target set,
  typed cleanly and with a typo, with one exception: uniformly drawn Amazon search terms typed with a typo,
  where Meilisearch leads (0.343 against 0.332). With real English misspellings on HN, its MRR is 0.854
  against 0.789 for the next engine. On the prefixes Amazon's users really typed, completr finds the term they
  then searched in its top 10 for 28.5% of them, against 25.2% for Meilisearch, 23.8% for tantivy and 17.3%
  for Typesense; 74% of those terms appear in the corpus at all.
- **Latency and throughput.** In process, completr answers at the median in 0.1 to 0.2 ms on HN, 0.3 to
  1.0 ms on Wikipedia and 1.5 to 5.3 ms on the corpora of about 40 million titles. Its p99 is the lowest of
  every query set but four: one- and two-edit typos on Wikipedia, where Meilisearch's is up to 1.6 ms lower,
  and one-edit typos and multi-word queries on AmazonQAC, where tantivy's is lower. With 8
  threads it serves from 914 queries per second on Open Library to 40,000 on HN, 7 to 30 times tantivy and 7
  to 11 times the fastest server. The servers' round trips include about 1 ms of localhost HTTP, so this
  comparison favours completr.
- **Indexing.** completr indexes Wikipedia in 25.6 s against 29.9 s for tantivy and Amazon's search terms in
  81 s against 164 s, each time writing segments under a 256 MB budget and compacting them into one inside
  the timed build. Its index is the smallest on every corpus above HN (MusicBrainz: 3.1 GB, against 5.9 GB
  for tantivy and 39.5 GB for Meilisearch), and it indexes with less memory than tantivy everywhere except
  MusicBrainz.
- **Where others lead.** tantivy indexes MusicBrainz (306 s against 317 s) and Open Library (198 s against
  350 s) faster, indexes MusicBrainz with less memory (333 MB against 547 MB), and keeps less memory
  resident after opening and after queries; on HN it opens faster and is slightly smaller. See
  [where the bytes go](#where-the-bytes-go) and the [caveats](#caveats).

## Capabilities

What each engine offers for autocompletion, as configured here and as documented by each project. The
numbers below should be read against it: an engine that does less work per query, or stores less, is
smaller and faster for that reason.

| | completr | tantivy | Typesense | Meilisearch |
|---|---|---|---|---|
| Runs as | a library in your process, with an optional database on object storage | a library in your process | a server | a server |
| Returns the suggestion's text | yes, with highlight ranges | if the field is stored (it is here); snippets on request | yes, with highlights | yes, with highlights |
| Your own ids | integers or strings such as UUIDs, returned as given | any stored field | string `id` | string or integer primary key |
| Duplicate titles | kept apart, ties broken by id | kept apart | kept apart | kept apart |
| Matching | whole-title prefix, word prefix anywhere, abbreviations, typos | terms, prefixes and fuzzy terms, combined by the application | words, the last as a prefix, typos | words, the last as a prefix, typos |
| Typo handling | delete variants precomputed at build, verified at query time | a Levenshtein automaton per query | at query time | Levenshtein automata at query time |
| Autocomplete ranking | built in: match kind, popularity, length | written by the application (emulated here) | ranking rules and sort fields | ranking rules and sort fields |
| Filters | context tags | queries and facets | `filter_by` | filters |
| Synonyms and abbreviations | per-document aliases | through custom tokenisers | synonyms | synonyms |
| Vector and hybrid search | yes, quantised | no | yes | yes |
| Updates | immutable segments, deltas and override layers | segments with deletes | live API | live API |
| Identical results however the index is segmented | yes, tested | not documented | not documented | not documented |

All engines in this benchmark store and return each result's title. Both corpora use integer ids; string
ids add their bytes to completr's key column.

## Setup

| | |
|---|---|
| Date | 2026-10-02 to 2026-10-04 |
| Machine | Apple M1 Pro, 10 cores, 16 GiB RAM, macOS 26.7, Python 3.13.7 |
| Engines | completr 0.1.0; tantivy-py 0.26.2; Typesense 30.2; Meilisearch 1.54.2 (official release binaries) |
| Hacker News | 124,440 deduplicated HN story titles, points as popularity |
| Wikipedia | 7,235,024 English articles that are not redirects, August 2026 user pageviews as popularity |
| MusicBrainz | 40,393,216 recordings as "name – artist", their MBIDs as ids, all-time ListenBrainz listens as popularity |
| AmazonQAC | 39,588,974 distinct Amazon search terms, their number of searches as popularity; 20,000 typed prefixes with the term then searched |
| Open Library | 41,666,206 works as "title – first author", their reading-log entries and ratings as popularity |
| Queries | Limit 10; the fixed-seed samples in [`bench/`](https://github.com/sayef/completr/tree/main/bench) (`samples.json` and `samples-<corpus>.json`, with `samples-<corpus>-misspelled.json` for the real misspellings) |

Each competitor uses the recommended way to rank by popularity: Typesense with `score` as
`default_sorting_field`, Meilisearch with the custom ranking rule `score:desc` after the default rules.
On HN, two variants give popularity more weight: Typesense `buckets`
(`sort_by=_text_match(buckets: 10):desc,score:desc`) and Meilisearch `popfirst` (`score:desc` right after
`words` and `typo`). tantivy has no autocomplete mode, so the harness emulates one: complete words as
terms, the last word as a prefix, and a fuzzy pass with Meilisearch's typo thresholds when fewer than 10
documents match. Its title is a stored field, read with each hit, as for the other engines. tantivy and
completr both index under a 256 MB writer budget. completr uses default options, with popularity
`log1p(score) / log1p(max score)`.

Memory is the footprint the operating system charges: for completr and tantivy, the growth of a fresh
process after opening the index and its first query, and after 5,000 prefix queries; peak memory while
indexing is the build's own peak above the loaded documents, as the kernel records it (its peak footprint
since a reset just before the build), so no short spike is missed. For the servers it is the server
process after indexing and after all queries, and its kernel-recorded peak while indexing. Open or restart to first hit
is the time from opening the index, or restarting the server, to the first answered query.

## Hacker News

Results on HN titles fetched from Meilisearch's public benchmark bucket.

!!! note "Dataset licence: not available"
    The Hacker News titles come from the official HN API and carry no explicit licence. They are not
    redistributed with completr; the harness downloads them at run time for measurement only.

### Indexing, size and memory

<!-- begin hn-index -->
| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | **292.0 ms** | **59 MB** | 11 MB | 4 MB | 10 MB | 13 ms |
| tantivy | 1.13 s | 90 MB | **10 MB** | **1 MB** | **5 MB** | **4.5 ms** |
| Typesense | 3.84 s | 265 MB | 39 MB | 237 MB | 243 MB | 3.33 s |
| Typesense, buckets | 3.82 s | 234 MB | 40 MB | 241 MB | 245 MB | 3.33 s |
| Meilisearch | 2.11 s | 1000 MB | 136 MB | 326 MB | 220 MB | 232 ms |
| Meilisearch, popfirst | 2.07 s | 983 MB | 136 MB | 316 MB | 219 MB | 232 ms |

With 8 build threads, it takes 289.5 ms. With UUID strings as ids instead of integers, its index is 13 MB and takes 660.7 ms to build.
<!-- end hn-index -->

The corpus fits in one segment under completr's writer budget, so there is nothing to compact.

### Growing the corpus

Seeded, nested subsets of the corpus.

<!-- begin hn-scale -->
| Engine | Documents | Index time | Peak memory while indexing | On disk |
|---|---|---|---|---|
| completr | 25,000 | 78.8 ms | 27 MB | 3 MB |
| completr | 50,000 | 132.8 ms | 46 MB | 5 MB |
| completr | 124,440 | 297.1 ms | 59 MB | 11 MB |
| tantivy | 25,000 | 616.8 ms | 69 MB | 2 MB |
| tantivy | 50,000 | 725.8 ms | 77 MB | 4 MB |
| tantivy | 124,440 | 1.03 s | 90 MB | 10 MB |
| Typesense | 25,000 | 761.1 ms | 128 MB | 7 MB |
| Typesense | 50,000 | 1.55 s | 168 MB | 15 MB |
| Typesense | 124,440 | 3.91 s | 241 MB | 39 MB |
| Meilisearch | 25,000 | 557.3 ms | 695 MB | 29 MB |
| Meilisearch | 50,000 | 940.6 ms | 942 MB | 54 MB |
| Meilisearch | 124,440 | 2.06 s | 987 MB | 136 MB |
<!-- end hn-scale -->

### Where the bytes go

completr's 10.6 MB segment, by section:

| Section | Size | Purpose | tantivy |
|---|---|---|---|
| Title texts | 3.7 MB | the original titles, FSST-compressed, for suggestions and for title-prefix lookups | its document store, compressed in blocks |
| Spelling variants | 3.0 MB | SymSpell delete variants, Elias-Fano coded, so a typo costs lookups instead of an automaton | none: it runs a Levenshtein automaton over its term dictionary per query |
| Words | 2.9 MB | the sorted word texts, delta-coded postings, frequencies | its term dictionary and postings |
| Columns | 0.5 MB | ids, popularity, text lengths | a bit-packed score column |
| Title order | 0.4 MB | documents sorted by lowercased title, for whole-title prefix matches such as "show hn: ru" | none: it indexes words only |

### Latency

Single client, limit 10, in milliseconds. In-process time for completr and tantivy; for the servers, the
round trip over localhost HTTP and the time the engine reports (whole milliseconds only).

<!-- begin hn-latency -->
| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (23,292) | completr | **0.15** | **0.38** | **0.62** | - | - |
|  | tantivy | 0.42 | 1.40 | 2.72 | - | - |
|  | Typesense | 1.36 | 8.91 | 40.93 | 0 | 40 |
|  | Typesense, buckets | 1.40 | 8.96 | 40.43 | 0 | 39 |
|  | Meilisearch | 1.39 | 1.92 | 2.42 | 0 | 1 |
|  | Meilisearch, popfirst | 1.48 | 2.05 | 2.62 | 0 | 1 |
| One-edit typos (1,000) | completr | **0.18** | **0.44** | **0.86** | - | - |
|  | tantivy | 0.22 | 0.68 | 2.12 | - | - |
|  | Typesense | 1.08 | 2.17 | 8.74 | 0 | 8 |
|  | Typesense, buckets | 1.12 | 2.20 | 8.74 | 0 | 8 |
|  | Meilisearch | 1.16 | 1.48 | 1.88 | 0 | 1 |
|  | Meilisearch, popfirst | 1.27 | 1.61 | 2.07 | 0 | 1 |
| Two-edit typos (1,000) | completr | **0.16** | **0.54** | **1.03** | - | - |
|  | tantivy | 0.51 | 0.71 | 1.47 | - | - |
|  | Typesense | 1.25 | 2.38 | 8.26 | 0 | 7 |
|  | Typesense, buckets | 1.29 | 2.47 | 8.28 | 0 | 7 |
|  | Meilisearch | 1.20 | 1.49 | 1.84 | 0 | 1 |
|  | Meilisearch, popfirst | 1.30 | 1.63 | 2.31 | 0 | 1 |
| Multi-word (2,000) | completr | **0.12** | **0.24** | **0.43** | - | - |
|  | tantivy | 0.23 | 0.73 | 1.75 | - | - |
|  | Typesense | 0.95 | 2.85 | 14.52 | 0 | 13 |
|  | Typesense, buckets | 0.97 | 2.90 | 13.34 | 0 | 12 |
|  | Meilisearch | 1.33 | 1.67 | 2.17 | 0 | 1 |
|  | Meilisearch, popfirst | 1.43 | 1.82 | 2.46 | 0 | 1 |
<!-- end hn-latency -->

### Throughput

8 closed-loop clients for 10 seconds on the prefix set: threads in one process for completr and tantivy,
client processes over HTTP for the servers.

<!-- begin hn-throughput -->
| Engine | Queries per second |
|---|---|
| completr | **40,016** |
| tantivy | 5,541 |
| Typesense | 1,768 |
| Typesense, buckets | 1,679 |
| Meilisearch | 4,024 |
| Meilisearch, popfirst | 3,826 |
<!-- end hn-throughput -->

### Quality

Each target title is typed one character at a time, and the target's rank in the top 10 is recorded after
every keystroke. MRR is the reciprocal rank averaged over all prefixes; S@k after L characters is the share
of targets in the top k; keystrokes to top 5 is the mean number of characters typed before the target
appears in the top 5. Popular targets are 500 titles drawn in proportion to their score; uniform targets
are 300 drawn uniformly. The typo'd variants have one edit in the first word of 5 or more letters.
Misspelled targets are 500 titles drawn by score among those holding a word from Wikipedia's list of common
English misspellings, typed with that word misspelt as people really misspell it.

#### Popular targets

<!-- begin hn-quality-popular -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.862** | **0.204** | **0.411** | **0.681** | **0.665** | **4.9** | **0.843** | **0.375** | **0.596** | **1.000** |
| tantivy | 0.804 | 0.094 | 0.230 | 0.437 | 0.460 | 7.0 | 0.734 | 0.197 | 0.352 | 0.959 |
| Typesense | 0.784 | 0.066 | 0.192 | 0.375 | 0.456 | 6.9 | 0.772 | 0.158 | 0.393 | 0.998 |
| Typesense, buckets | 0.787 | 0.084 | 0.202 | 0.387 | 0.462 | 6.8 | 0.774 | 0.168 | 0.402 | 0.996 |
| Meilisearch | 0.846 | 0.152 | 0.383 | 0.627 | 0.653 | 5.2 | 0.804 | 0.348 | 0.590 | 0.969 |
| Meilisearch, popfirst | 0.789 | 0.086 | 0.210 | 0.411 | 0.418 | 7.1 | 0.746 | 0.182 | 0.355 | 0.953 |
<!-- end hn-quality-popular -->

#### Uniform targets

<!-- begin hn-quality-uniform -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.804** | 0.027 | **0.184** | **0.338** | **0.465** | **7.5** | **0.780** | 0.138 | 0.349 | 0.993 |
| tantivy | 0.737 | 0.007 | 0.077 | 0.137 | 0.231 | 10.1 | 0.683 | 0.070 | 0.188 | 0.943 |
| Typesense | 0.755 | 0.017 | 0.084 | 0.157 | 0.278 | 9.5 | 0.744 | 0.077 | 0.208 | **0.997** |
| Typesense, buckets | 0.754 | 0.017 | 0.087 | 0.157 | 0.268 | 9.6 | 0.743 | 0.081 | 0.201 | **0.997** |
| Meilisearch | 0.798 | **0.030** | 0.181 | 0.331 | 0.438 | 7.6 | 0.762 | **0.144** | **0.369** | 0.950 |
| Meilisearch, popfirst | 0.731 | 0.013 | 0.070 | 0.134 | 0.214 | 10.2 | 0.699 | 0.064 | 0.185 | 0.936 |
<!-- end hn-quality-uniform -->

#### Misspelled targets

<!-- begin hn-quality-misspelled -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.872** | **0.156** | **0.374** | **0.610** | **0.630** | **5.4** | **0.854** | **0.352** | **0.610** | **1.000** |
| tantivy | 0.817 | 0.060 | 0.172 | 0.358 | 0.394 | 7.4 | 0.609 | 0.164 | 0.354 | 0.844 |
| Typesense | 0.807 | 0.048 | 0.168 | 0.350 | 0.396 | 7.3 | 0.789 | 0.160 | 0.384 | 0.990 |
| Typesense, buckets | 0.809 | 0.054 | 0.172 | 0.358 | 0.408 | 7.3 | 0.787 | 0.164 | 0.390 | 0.984 |
| Meilisearch | 0.811 | 0.058 | 0.160 | 0.358 | 0.364 | 7.5 | 0.688 | 0.152 | 0.328 | 0.838 |
| Meilisearch, popfirst | 0.811 | 0.058 | 0.160 | 0.358 | 0.364 | 7.5 | 0.688 | 0.152 | 0.328 | 0.838 |
<!-- end hn-quality-misspelled -->

All metrics, including S@10 and characters saved, are in
[`bench/results/hn/results.md`](https://github.com/sayef/completr/blob/main/bench/results/hn/results.md).

## Wikipedia

Every English Wikipedia article that is not a redirect, from the 2026-09-01 dump, with its views by users
in August 2026 as its popularity.

!!! note "Dataset licence"
    Article titles are © Wikipedia contributors under CC BY-SA 4.0; the pageview counts are CC0. The
    harness downloads both at run time.

### Indexing, size and memory

<!-- begin wiki-index -->
| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | 25.60 s | 286 MB | **338 MB** | 24 MB | 68 MB | **30 ms** |
| completr, 9 segments, not compacted | **14.36 s** | **250 MB** | 430 MB | 44 MB | 50 MB | 117 ms |
| tantivy | 29.89 s | 347 MB | 357 MB | **2 MB** | **25 MB** | 44 ms |
| Typesense | 200.42 s | 1010 MB | 1224 MB | 1017 MB | 908 MB | 9.69 s |
| Meilisearch | 50.31 s | 3450 MB | 4934 MB | 2003 MB | 1966 MB | 234 ms |

completr's index time covers writing 9 segments (14.96 s) and compacting them into one. With 8 build threads, it takes 18.58 s. With UUID strings as ids instead of integers, its index is 485 MB and takes 51.15 s to build.
<!-- end wiki-index -->

The second completr row keeps the writer's segments as they are, without compacting them. An index of
several segments ranks exactly like one, so its quality is the same, but each query visits every
segment.

### Growing the corpus

Seeded, nested subsets of the corpus. Above 256 MB of documents completr writes several segments, and its
index time includes compacting them.

<!-- begin wiki-scale -->
| Engine | Documents | Index time | Peak memory while indexing | On disk |
|---|---|---|---|---|
| completr | 1,000,000 | 3.32 s | 206 MB | 61 MB |
| completr | 2,000,000 | 6.75 s | 220 MB | 111 MB |
| completr | 4,000,000 | 14.22 s | 252 MB | 202 MB |
| tantivy | 1,000,000 | 5.72 s | 179 MB | 49 MB |
| tantivy | 2,000,000 | 10.65 s | 285 MB | 93 MB |
| tantivy | 4,000,000 | 17.08 s | 341 MB | 204 MB |
| Typesense | 1,000,000 | 25.26 s | 449 MB | 190 MB |
| Typesense | 2,000,000 | 51.18 s | 570 MB | 487 MB |
| Typesense | 4,000,000 | 108.09 s | 765 MB | 928 MB |
| Meilisearch | 1,000,000 | 7.82 s | 1986 MB | 739 MB |
| Meilisearch | 2,000,000 | 13.40 s | 2180 MB | 1176 MB |
| Meilisearch | 4,000,000 | 26.41 s | 2776 MB | 2468 MB |
<!-- end wiki-scale -->

### Where the bytes go

completr's 337 MB index, by section:

| Section | Size | Purpose | tantivy |
|---|---|---|---|
| Title texts | 101 MB | the original titles, FSST-compressed, also read back as title keys | its document store, compressed in blocks |
| Spelling variants | 99 MB | delete variants of 1.6M groups of words sharing their first 7 chars, Elias-Fano coded | none |
| Words | 80 MB | 2.2M word texts with sampled keys, postings in blocks of 128 gaps, frequencies | its term dictionary and postings |
| Columns | 32 MB | ids in linear-fit blocks, popularity as 16-bit codes, text lengths | a bit-packed score column |
| Title order | 24 MB | documents sorted by lowercased title, with every 64th key sampled | none |

### Latency

<!-- begin wiki-latency -->
| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (10,100) | completr | **0.29** | **1.96** | **5.65** | - | - |
|  | completr, 9 segments, not compacted | 0.80 | 5.63 | 16.77 | - | - |
|  | tantivy | 4.36 | 36.85 | 98.37 | - | - |
|  | Typesense | 3.62 | 40.92 | 757.19 | 2 | 756 |
|  | Meilisearch | 4.37 | 8.13 | 16.06 | 3 | 15 |
| One-edit typos (1,000) | completr | **1.01** | **4.89** | 8.11 | - | - |
|  | completr, 9 segments, not compacted | 3.27 | 15.92 | 25.82 | - | - |
|  | tantivy | 4.57 | 16.90 | 30.69 | - | - |
|  | Typesense | 2.06 | 8.42 | 103.50 | 1 | 102 |
|  | Meilisearch | 3.77 | 5.64 | **7.97** | 3 | 7 |
| Two-edit typos (1,000) | completr | **0.87** | 6.11 | 9.97 | - | - |
|  | completr, 9 segments, not compacted | 2.88 | 20.65 | 32.59 | - | - |
|  | tantivy | 12.21 | 18.26 | 29.16 | - | - |
|  | Typesense | 3.92 | 10.28 | 36.47 | 3 | 35 |
|  | Meilisearch | 3.86 | **5.62** | **8.33** | 3 | 7 |
| Multi-word (2,000) | completr | **0.63** | **1.70** | **3.31** | - | - |
|  | completr, 9 segments, not compacted | 1.96 | 4.87 | 7.93 | - | - |
|  | tantivy | 4.36 | 18.04 | 60.01 | - | - |
|  | Typesense | 1.98 | 11.03 | 90.55 | 1 | 89 |
|  | Meilisearch | 4.91 | 7.46 | 13.97 | 4 | 13 |
<!-- end wiki-latency -->

### Throughput

<!-- begin wiki-throughput -->
| Engine | Queries per second |
|---|---|
| completr | **9,536** |
| completr, 9 segments, not compacted | 2,930 |
| tantivy | 323 |
| Typesense | 192 |
| Meilisearch | 1,082 |
<!-- end wiki-throughput -->

### Quality

#### Popular targets

<!-- begin wiki-quality-popular -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.595** | 0.046 | **0.259** | **0.463** | **0.597** | **6.3** | **0.527** | **0.167** | **0.448** | **0.965** |
| tantivy | 0.567 | **0.058** | 0.232 | 0.401 | 0.532 | 7.0 | 0.463 | 0.139 | 0.370 | 0.875 |
| Typesense | 0.477 | 0.002 | 0.088 | 0.277 | 0.414 | 7.9 | 0.364 | 0.046 | 0.255 | 0.860 |
| Meilisearch | 0.541 | 0.002 | 0.140 | 0.403 | 0.511 | 6.8 | 0.479 | 0.108 | 0.400 | 0.896 |
<!-- end wiki-quality-popular -->

#### Uniform targets

<!-- begin wiki-quality-uniform -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.488** | **0.003** | **0.097** | **0.171** | **0.292** | **9.4** | **0.436** | **0.044** | 0.170 | **0.935** |
| tantivy | 0.444 | 0.000 | 0.050 | 0.117 | 0.246 | 10.1 | 0.385 | 0.025 | 0.133 | 0.847 |
| Typesense | 0.440 | 0.000 | 0.044 | 0.107 | 0.239 | 10.4 | 0.336 | 0.018 | 0.110 | 0.862 |
| Meilisearch | 0.480 | **0.003** | 0.077 | 0.164 | 0.278 | 9.5 | 0.430 | 0.033 | **0.182** | 0.895 |
<!-- end wiki-quality-uniform -->

All metrics are in
[`bench/results/wiki/results.md`](https://github.com/sayef/completr/blob/main/bench/results/wiki/results.md).

## MusicBrainz

Every recording in the MusicBrainz database of 2026-09-30, titled "name – artist credit", with its MBID as
its id, so every engine stores and returns a UUID string. Its popularity is the listens it received among
ListenBrainz users' all-time top recordings (the statistics dump of 2026-09-15); 4.9 million recordings have
some.

!!! note "Dataset licence"
    MusicBrainz core data and the ListenBrainz statistics are CC0. The harness downloads both at run time.

### Indexing, size and memory

<!-- begin music-index -->
| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | 317.41 s | 547 MB | **3139 MB** | 68 MB | 98 MB | **85 ms** |
| tantivy | **306.16 s** | **333 MB** | 5938 MB | **3 MB** | **13 MB** | 138 ms |
| Meilisearch | 1244.32 s | 8207 MB | 39528 MB | 2427 MB | 2434 MB | 236 ms |

completr's index time covers writing 113 segments (196.73 s) and compacting them into one. With 8 build threads, it takes 299.71 s.
<!-- end music-index -->

Typesense did not finish this corpus: after indexing for over three hours it did not answer again, twice.

### Growing the corpus

<!-- begin music-scale -->
| Engine | Documents | Index time | Peak memory while indexing | On disk |
|---|---|---|---|---|
| completr | 4,000,000 | 30.59 s | 216 MB | 368 MB |
| completr | 16,000,000 | 129.60 s | 287 MB | 1311 MB |
| tantivy | 4,000,000 | 32.04 s | 332 MB | 692 MB |
| tantivy | 16,000,000 | 150.33 s | 440 MB | 2421 MB |
| Meilisearch | 4,000,000 | 65.12 s | 3798 MB | 5627 MB |
| Meilisearch | 16,000,000 | 272.55 s | 5467 MB | 16924 MB |
<!-- end music-scale -->

### Where the bytes go

completr's 3.1 GB index, by section:

| Section | Size | Purpose |
|---|---|---|
| Title texts | 1,133 MB | the "name – artist" texts, FSST-compressed |
| Keys | 646 MB | the MBIDs, 16 bytes each |
| Words | 533 MB | word texts with sampled keys, postings, frequencies |
| Columns | 366 MB | ids (hashes of the keys), popularity, text lengths |
| Spelling variants | 287 MB | delete variants of groups of words sharing their first 7 chars |
| Title order | 165 MB | documents sorted by lowercased title, with the most popular under each block |

### Latency

<!-- begin music-latency -->
| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (17,924) | completr | **2.07** | **10.44** | **43.61** | - | - |
|  | tantivy | 54.71 | 211.69 | 429.21 | - | - |
|  | Meilisearch | 21.65 | 44.05 | 108.88 | 20 | 107 |
| One-edit typos (1,000) | completr | **2.94** | **12.56** | **48.16** | - | - |
|  | tantivy | 38.05 | 112.42 | 185.52 | - | - |
|  | Meilisearch | 16.28 | 32.48 | 55.79 | 15 | 54 |
| Two-edit typos (1,000) | completr | **2.99** | **12.33** | **27.47** | - | - |
|  | tantivy | 90.55 | 120.86 | 183.23 | - | - |
|  | Meilisearch | 16.20 | 28.67 | 53.04 | 15 | 52 |
| Multi-word (2,000) | completr | **1.55** | **7.21** | **48.74** | - | - |
|  | tantivy | 9.08 | 100.40 | 182.09 | - | - |
|  | Meilisearch | 18.85 | 36.50 | 65.52 | 17 | 64 |
<!-- end music-latency -->

### Throughput

<!-- begin music-throughput -->
| Engine | Queries per second |
|---|---|
| completr | **1,355** |
| tantivy | 97 |
| Meilisearch | 180 |
<!-- end music-throughput -->

### Quality

#### Popular targets

<!-- begin music-quality-popular -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.762** | **0.152** | **0.314** | **0.498** | **0.546** | **6.2** | **0.713** | **0.197** | **0.415** | **0.998** |
| tantivy | 0.712 | 0.130 | 0.242 | 0.418 | 0.426 | 7.3 | 0.626 | 0.137 | 0.288 | 0.956 |
| Meilisearch | 0.701 | 0.060 | 0.204 | 0.368 | 0.438 | 7.2 | 0.656 | 0.137 | 0.341 | 0.960 |
<!-- end music-quality-popular -->

#### Uniform targets

<!-- begin music-quality-uniform -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.513** | 0.007 | **0.043** | 0.077 | 0.133 | **12.7** | **0.475** | **0.018** | 0.080 | **0.757** |
| tantivy | 0.457 | 0.007 | 0.030 | 0.053 | 0.090 | 14.7 | 0.400 | 0.004 | 0.047 | 0.681 |
| Meilisearch | 0.507 | **0.010** | 0.040 | **0.100** | **0.147** | 13.0 | 0.462 | 0.007 | **0.101** | 0.717 |
<!-- end music-quality-uniform -->

All metrics are in
[`bench/results/music/results.md`](https://github.com/sayef/completr/blob/main/bench/results/music/results.md).

## AmazonQAC

The distinct search terms of [AmazonQAC](https://huggingface.co/datasets/amazon/AmazonQAC), 395 million
query autocompletion sessions on Amazon in September 2023, each term with its number of searches as its
popularity. Its test set holds 20,000 prefixes users typed in October 2023, each with the term they then
searched: real typing, with its typos, and not synthetic samples.

!!! note "Dataset licence"
    AmazonQAC is published under CDLA-Permissive-2.0. The harness reads it at run time.

### Indexing, size and memory

<!-- begin qac-index -->
| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | **80.68 s** | **315 MB** | **1117 MB** | 50 MB | **121 MB** | **34 ms** |
| tantivy | 163.86 s | 499 MB | 1319 MB | **13 MB** | 125 MB | 264 ms |
| Typesense | 1187.29 s | 3127 MB | 4750 MB | 3090 MB | 3547 MB | 431.66 s |
| Meilisearch | 231.03 s | 6899 MB | 20298 MB | 2412 MB | 2372 MB | 242 ms |

completr's index time covers writing 48 segments (49.62 s) and compacting them into one. With 8 build threads, it takes 64.62 s. With UUID strings as ids instead of integers, its index is 1998 MB and takes 229.38 s to build.
<!-- end qac-index -->

### Prefixes users typed

Each test prefix is searched once, and the term the user then searched is looked for in the top 10, as in
the AmazonQAC paper: S@k is the share of prefixes with that term in the top k, MRR@10 its mean reciprocal
rank. 26% of the searched terms never occur in training, so no engine can reach more than 0.740.

<!-- begin qac-labelled -->
| Engine | S@1 | S@10 | MRR@10 |
|---|---|---|---|
| completr | **0.132** | **0.285** | **0.180** |
| tantivy | 0.104 | 0.238 | 0.144 |
| Typesense | 0.066 | 0.173 | 0.099 |
| Meilisearch | 0.099 | 0.252 | 0.147 |
<!-- end qac-labelled -->

### Latency

<!-- begin qac-latency -->
| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (11,301) | completr | **0.82** | **6.96** | **23.06** | - | - |
|  | tantivy | 6.23 | 50.67 | 207.55 | - | - |
|  | Typesense | 11.70 | 122.78 | 596.28 | 10 | 594 |
|  | Meilisearch | 13.95 | 28.14 | 48.13 | 13 | 47 |
| One-edit typos (1,000) | completr | **2.90** | **10.85** | 34.24 | - | - |
|  | tantivy | 5.56 | 11.81 | **25.20** | - | - |
|  | Typesense | 4.38 | 27.77 | 169.10 | 3 | 168 |
|  | Meilisearch | 12.80 | 24.96 | 40.38 | 11 | 39 |
| Two-edit typos (1,000) | completr | **1.61** | **7.24** | **25.53** | - | - |
|  | tantivy | 6.44 | 11.57 | 29.37 | - | - |
|  | Typesense | 6.39 | 24.75 | 107.82 | 5 | 106 |
|  | Meilisearch | 13.08 | 25.36 | 35.15 | 12 | 34 |
| Multi-word (2,000) | completr | **2.29** | **8.34** | 34.09 | - | - |
|  | tantivy | 4.01 | 11.57 | **26.17** | - | - |
|  | Typesense | 5.57 | 25.00 | 109.16 | 4 | 108 |
|  | Meilisearch | 18.21 | 32.60 | 52.19 | 17 | 51 |
<!-- end qac-latency -->

### Throughput

<!-- begin qac-throughput -->
| Engine | Queries per second |
|---|---|
| completr | **2,623** |
| tantivy | 262 |
| Typesense | 159 |
| Meilisearch | 237 |
<!-- end qac-throughput -->

### Typing the terms

#### Popular targets

<!-- begin qac-quality-popular -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.468** | 0.034 | 0.096 | **0.272** | **0.275** | **9.0** | **0.408** | **0.064** | **0.188** | **0.914** |
| tantivy | 0.403 | **0.042** | **0.102** | 0.220 | 0.209 | 10.1 | 0.322 | 0.042 | 0.140 | 0.616 |
| Typesense | 0.297 | 0.004 | 0.035 | 0.150 | 0.097 | 11.0 | 0.205 | 0.017 | 0.055 | 0.759 |
| Meilisearch | 0.400 | 0.004 | 0.041 | 0.209 | 0.180 | 9.5 | 0.357 | 0.028 | 0.125 | 0.789 |
<!-- end qac-quality-popular -->

#### Uniform targets

<!-- begin qac-quality-uniform -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.376** | **0.000** | **0.013** | 0.043 | 0.074 | **13.1** | 0.332 | **0.007** | 0.038 | **0.914** |
| tantivy | 0.275 | **0.000** | 0.007 | 0.040 | 0.037 | 14.7 | 0.234 | 0.000 | 0.031 | 0.555 |
| Typesense | 0.299 | **0.000** | 0.010 | 0.040 | 0.051 | 14.7 | 0.221 | 0.003 | 0.028 | 0.784 |
| Meilisearch | 0.375 | **0.000** | **0.013** | **0.053** | **0.078** | **13.1** | **0.343** | **0.007** | **0.052** | 0.863 |
<!-- end qac-quality-uniform -->

All metrics are in
[`bench/results/qac/results.md`](https://github.com/sayef/completr/blob/main/bench/results/qac/results.md).

## Open Library

Every work in the Open Library dump of 2026-09-30 that has a title, titled "title – first author" where an
author is known, with the number in its key as its id. Its popularity is the number of reading-log entries
and ratings it has (the reading-log and ratings dumps of the same date); 3.3 million of the 41.7 million
works have some.

!!! note "Dataset licence"
    The Open Library dumps are CC0. The harness downloads them at run time, checked against archive.org's
    SHA-1 sums.

### Indexing, size and memory

<!-- begin books-index -->
| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | 349.71 s | **238 MB** | **3235 MB** | 48 MB | 72 MB | **122 ms** |
| completr, 93 segments, not compacted | **168.82 s** | 315 MB | 5705 MB | 78 MB | 110 MB | 3.28 s |
| tantivy | 198.22 s | 329 MB | 4033 MB | **5 MB** | **20 MB** | 241 ms |

completr's index time covers writing 93 segments (177.85 s) and compacting them into one. With 8 build threads, it takes 275.56 s. With UUID strings as ids instead of integers, its index is 4102 MB and takes 531.56 s to build.
<!-- end books-index -->

Typesense and Meilisearch have no row: Meilisearch passed the 10 GB memory limit while indexing, and
Typesense indexed in about an hour but a search answered with an error after 10 hours of queries, before the
harness counted such answers. The real misspellings are measured for completr and tantivy only.

### Growing the corpus

<!-- begin books-scale -->
| Engine | Documents | Index time | Peak memory while indexing | On disk |
|---|---|---|---|---|
| completr | 4,000,000 | 30.19 s | 316 MB | 400 MB |
| completr | 16,000,000 | 136.46 s | 377 MB | 1363 MB |
| tantivy | 4,000,000 | 20.76 s | 338 MB | 396 MB |
| tantivy | 16,000,000 | 92.23 s | 336 MB | 1512 MB |
<!-- end books-scale -->

### Latency

<!-- begin books-latency -->
| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (27,869) | completr | **3.96** | **14.87** | **43.57** | - | - |
|  | completr, 93 segments, not compacted | 51.98 | 159.53 | 351.32 | - | - |
|  | tantivy | 143.02 | 508.52 | 1084.49 | - | - |
| One-edit typos (1,000) | completr | **5.33** | **16.57** | **54.65** | - | - |
|  | completr, 93 segments, not compacted | 49.86 | 182.19 | 313.15 | - | - |
|  | tantivy | 67.52 | 167.85 | 529.91 | - | - |
| Two-edit typos (1,000) | completr | **4.47** | **19.02** | **41.91** | - | - |
|  | completr, 93 segments, not compacted | 48.82 | 223.47 | 419.05 | - | - |
|  | tantivy | 117.46 | 211.97 | 426.55 | - | - |
| Multi-word (2,000) | completr | **2.35** | **8.07** | **52.84** | - | - |
|  | completr, 93 segments, not compacted | 22.12 | 61.11 | 103.48 | - | - |
|  | tantivy | 24.64 | 169.17 | 435.56 | - | - |
<!-- end books-latency -->

### Throughput

<!-- begin books-throughput -->
| Engine | Queries per second |
|---|---|
| completr | **914** |
| completr, 93 segments, not compacted | 114 |
| tantivy | 50 |
<!-- end books-throughput -->

### Quality

#### Popular targets

<!-- begin books-quality-popular -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.759** | **0.080** | **0.196** | **0.322** | **0.422** | **8.3** | **0.736** | **0.138** | **0.364** | **0.964** |
| tantivy | 0.704 | 0.044 | 0.132 | 0.216 | 0.298 | 10.2 | 0.642 | 0.099 | 0.231 | 0.901 |
<!-- end books-quality-popular -->

#### Uniform targets

<!-- begin books-quality-uniform -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.662** | 0.007 | **0.033** | **0.063** | **0.143** | **13.5** | **0.637** | **0.010** | **0.076** | **0.913** |
| tantivy | 0.606 | **0.010** | 0.010 | 0.023 | 0.053 | 16.2 | 0.536 | 0.000 | 0.021 | 0.851 |
<!-- end books-quality-uniform -->

#### Misspelled targets

<!-- begin books-quality-misspelled -->
| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.654** | **0.012** | **0.042** | **0.084** | **0.108** | **15.8** | **0.622** | **0.040** | **0.086** | **0.908** |
| tantivy | 0.594 | 0.008 | 0.022 | 0.050 | 0.068 | 19.4 | 0.480 | 0.020 | 0.042 | 0.780 |
<!-- end books-quality-misspelled -->

All metrics are in
[`bench/results/books/results.md`](https://github.com/sayef/completr/blob/main/bench/results/books/results.md).

## Caveats

- **In-process against network.** completr and tantivy run inside the benchmark process; Typesense and
  Meilisearch answer over localhost HTTP, which adds about 1 ms per query and client-side work. This
  favours completr and tantivy in the latency and throughput tables. The engine-reported server times
  exclude the network but are rounded to whole milliseconds.
- **Typo'd typing on the long tail.** On Wikipedia Meilisearch ranks a uniformly drawn target first
  slightly more often once the misspelt word is complete (at 8 characters, 18% against 16%), though
  completr's typo MRR is higher.
- **Resident memory against tantivy.** completr keeps more of its index resident after opening than
  tantivy, and on HN its segment is slightly larger: it stores precomputed spelling variants that tantivy
  replaces with an automaton per query.
- **tantivy's merges.** tantivy merges segments in the background, so its size varies between runs.
- **Rows from different days.** The servers and tantivy were measured on 2026-10-02 and 2026-10-03 (their build
  peaks again on 2026-10-03, by the kernel); completr was measured again on 2026-10-04, after its last changes.
- **Servers at 40 million documents.** Typesense did not finish MusicBrainz on this machine; on Open Library it
  indexed, then a search answered with an error after 10 hours of queries and the run was lost (the harness
  now counts such answers). Meilisearch passed the 10 GB memory limit while indexing Open Library.
- **Searches that hang.** A server search with no answer within 30 s counts as finding nothing, and the
  results record how many there were. Typesense once stopped answering a query on Wikipedia and ignored
  shutdown, which is why the limit exists.
- **Recommended settings, not tuning.** Each engine runs with the configuration its documentation
  recommends for popularity ranking. Other settings trade clean against typo'd quality differently, as
  the HN variants show.
- **Mostly synthetic queries.** Apart from AmazonQAC's typed prefixes and the real English misspellings,
  typos are random edits, and the quality metrics count only the target's rank.
- **One laptop, not a quiet server.** Timings move by tens of percent between runs and machines, and these
  were taken on a laptop that was also doing other work, so some rows ran under contention. Index times,
  latencies and throughput are indicative; ranks, sizes and kernel-measured memory are not affected. A
  rerun on a dedicated Linux machine is planned.
- **tantivy is emulated.** Its quality reflects the harness's autocomplete adapter as much as tantivy.
