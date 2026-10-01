# Benchmarks

completr compared with Typesense, Meilisearch and tantivy as autocompletion engines, on 124,440 Hacker News
story titles ranked by points. Every engine indexes the same documents and answers the same queries. To
reproduce, see [`bench/README.md`](https://github.com/sayef/completr/blob/main/bench/README.md), which also describes every measurement and setting
in detail.

Results are measured on HN titles fetched from Meilisearch's public benchmark bucket.

!!! note "Dataset licence: not available"
    The Hacker News titles come from the official HN API and carry no explicit licence. They are not
    redistributed with completr; the harness downloads them at run time for measurement only.

## Summary

- **Clean typing.** completr ranks the wanted title highest of the engines tested: a mean reciprocal rank
  over prefixes of 0.861 for popular titles and 0.804 for uniformly drawn ones, ahead of Meilisearch (0.846
  and 0.798), tantivy, and Typesense. After 5 characters, the popular target is first 41% of the time.
- **Typing with a typo.** completr leads here too: 0.827 and 0.766, against 0.804 and 0.762 for
  Meilisearch and 0.772 and 0.744 for Typesense. Meilisearch still ranks a uniformly drawn target first
  slightly more often once the misspelt word is complete.
- **Latency and throughput.** In process, completr answers every query set in under 0.2 ms at the median
  and under 1 ms at p99, and serves 30,000 queries per second with 8 threads, about 5 times tantivy and 8
  times the fastest server. The servers' round trips include about 1 ms of localhost HTTP, so this
  comparison favours completr.
- **Indexing and footprint.** It builds its index fastest (0.72 s, 0.33 s with 8 threads), with a peak of
  152 MB against 89 MB for tantivy, 280 MB for Typesense and about 1 GB for Meilisearch, and opens it in
  about a millisecond. Its 23 MB segment and 31 MB of resident memory after queries are far less than the
  servers', and about twice tantivy's 10 MB index and 19 MB, because completr also stores a title-prefix trie and precomputed
  spelling variants (see [where the bytes go](#where-the-bytes-go)).

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

All engines in this benchmark store and return each result's title. The HN corpus uses integer ids; string
ids add their bytes to completr's key column.

## Setup

| | |
|---|---|
| Date | 2026-10-01 |
| Machine | Apple M1 Pro, 10 cores, 16 GiB RAM, macOS 26.7, Python 3.13.7 |
| Engines | completr 0.1.0; tantivy-py 0.26.2; Typesense 30.2; Meilisearch 1.54.2 (official release binaries) |
| Corpus | 124,440 deduplicated HN story titles, points as popularity |
| Queries | Limit 10; the fixed-seed sample in [`bench/samples.json`](https://github.com/sayef/completr/blob/main/bench/samples.json) |

Each competitor uses the recommended way to rank by popularity: Typesense with `score` as
`default_sorting_field`, Meilisearch with the custom ranking rule `score:desc` after the default rules.
Two variants give popularity more weight: Typesense `buckets` (`sort_by=_text_match(buckets: 10):desc,score:desc`)
and Meilisearch `popfirst` (`score:desc` right after `words` and `typo`). tantivy has no autocomplete mode,
so the harness emulates one: complete words as terms, the last word as a prefix, and a fuzzy pass with
Meilisearch's typo thresholds when fewer than 10 documents match. Its title is a stored field, read with
each hit, as for the other engines. completr uses default options, with
popularity `log1p(points) / log1p(max points)`.

## Indexing, size and memory

| Engine | Index time | Peak memory while indexing | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|---|
| completr | **717.3 ms** (325.2 ms with 8 threads) | 152 MB | 23 MB | 5 MB | 31 MB | 1.5 ms |
| tantivy | 1.06 s | **89 MB** | **10 MB** | **3 MB** | **19 MB** | **0.5 ms** |
| Typesense | 3.62 s | 280 MB | 40 MB | 281 MB | 222 MB | 3.28 s |
| Typesense, buckets | 3.63 s | 267 MB | 39 MB | 266 MB | 285 MB | 3.28 s |
| Meilisearch | 1.94 s | 1034 MB | 136 MB | 1023 MB | 854 MB | 219 ms |
| Meilisearch, popfirst | 1.99 s | 1122 MB | 136 MB | 1117 MB | 662 MB | 222 ms |

With UUID strings as ids instead of integers, completr's segment is 26 MB and takes 1.12 s to build.

With UUID strings as ids instead of integers, completr's segment is 26 MB and takes 1.12 s to build.

For completr and tantivy, memory is the RSS growth of a fresh process after opening the index and after
5,000 prefix queries, and peak memory while indexing is the build process's peak RSS above the loaded
documents; for the servers, it is the RSS of the server process after indexing and after all queries, and
its peak RSS while indexing. completr opens a local segment without reading it whole; `Segment::verify` checks its checksum.

### Growing the corpus

The same measurements on seeded, nested subsets of the corpus show how each engine scales. completr builds
one segment per million documents, so its build memory stays bounded on larger corpora; tantivy flushes a
segment whenever its 256 MB writer budget fills. Million-document workloads follow in later runs.

| Engine | Documents | Index time | Peak memory while indexing | On disk |
|---|---|---|---|---|
| completr | 25,000 | 221 ms | 31 MB | 6 MB |
| completr | 50,000 | 357 ms | 57 MB | 11 MB |
| completr | 124,440 | 747 ms | 151 MB | 23 MB |
| tantivy | 25,000 | 624 ms | 72 MB | 2 MB |
| tantivy | 50,000 | 667 ms | 81 MB | 4 MB |
| tantivy | 124,440 | 977 ms | 88 MB | 10 MB |
| Typesense | 25,000 | 709 ms | 175 MB | 7 MB |
| Typesense | 50,000 | 1.43 s | 235 MB | 16 MB |
| Typesense | 124,440 | 3.66 s | 273 MB | 39 MB |
| Meilisearch | 25,000 | 557 ms | 669 MB | 29 MB |
| Meilisearch | 50,000 | 853 ms | 819 MB | 53 MB |
| Meilisearch | 124,440 | 1.86 s | 1137 MB | 136 MB |

### Where the bytes go

completr's 23.4 MB segment, by section, with what tantivy keeps for the same purpose:

| Section | Size | Purpose | tantivy |
|---|---|---|---|
| Spelling variants | 7.1 MB | SymSpell delete variants hashed into buckets of word ordinals, so a typo costs lookups instead of an automaton | none: it runs a Levenshtein automaton over its term dictionary per query |
| Title trie | 6.2 MB | every title as a key, for whole-title prefix matches such as "show hn: ru" | none: it indexes words only |
| Title texts | 3.9 MB | the original titles, FSST-compressed, for suggestions | its document store, compressed in blocks |
| Words | 3.9 MB | word dictionary, postings, frequencies | its term dictionary and postings |
| Columns | 1.9 MB | ids, popularity, text lengths | a bit-packed score column |

Words and columns together, about 6 MB, are what tantivy's term index and score column cover. The variants
and the title trie are what make typos and title prefixes fast and well ranked.

## Latency

Single client, limit 10, in milliseconds. In-process time for completr and tantivy; for the servers, the
round trip over localhost HTTP and the time the engine reports (whole milliseconds only).

| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (23,292) | completr | **0.17** | **0.49** | **0.89** | - | - |
|  | tantivy | 0.43 | 1.42 | 2.73 | - | - |
|  | Typesense | 1.38 | 9.08 | 41.30 | 0 | 40 |
|  | Typesense, buckets | 1.46 | 9.19 | 41.76 | 0 | 40 |
|  | Meilisearch | 1.46 | 2.07 | 2.78 | 0 | 1 |
|  | Meilisearch, popfirst | 1.51 | 2.10 | 2.72 | 0 | 2 |
| One-edit typos (1,000) | completr | **0.16** | **0.52** | **0.84** | - | - |
|  | tantivy | 0.22 | 0.69 | 2.17 | - | - |
|  | Typesense | 1.09 | 2.18 | 8.71 | 0 | 8 |
|  | Typesense, buckets | 1.12 | 2.27 | 8.78 | 0 | 8 |
|  | Meilisearch | 1.26 | 1.71 | 2.34 | 0 | 1 |
|  | Meilisearch, popfirst | 1.29 | 1.70 | 2.28 | 0 | 1 |
| Two-edit typos (1,000) | completr | **0.14** | **0.56** | **0.90** | - | - |
|  | tantivy | 0.51 | 0.73 | 1.47 | - | - |
|  | Typesense | 1.26 | 2.39 | 8.84 | 0 | 8 |
|  | Typesense, buckets | 1.32 | 2.50 | 8.86 | 0 | 8 |
|  | Meilisearch | 1.30 | 1.75 | 2.28 | 0 | 1 |
|  | Meilisearch, popfirst | 1.33 | 1.69 | 2.29 | 0 | 1 |
| Multi-word (2,000) | completr | **0.12** | **0.28** | **0.46** | - | - |
|  | tantivy | 0.24 | 0.74 | 1.77 | - | - |
|  | Typesense | 0.96 | 2.87 | 15.27 | 0 | 14 |
|  | Typesense, buckets | 0.99 | 2.92 | 13.30 | 0 | 12 |
|  | Meilisearch | 1.39 | 1.83 | 2.42 | 0 | 1 |
|  | Meilisearch, popfirst | 1.46 | 1.89 | 2.50 | 0 | 1 |

## Throughput

8 closed-loop clients for 10 seconds on the prefix set: threads in one process for completr and tantivy,
client processes over HTTP for the servers.

| Engine | Queries per second |
|---|---|
| completr | **30,213** |
| tantivy | 5,655 |
| Typesense | 1,725 |
| Typesense, buckets | 1,680 |
| Meilisearch | 3,791 |
| Meilisearch, popfirst | 3,762 |

## Quality

Each target title is typed one character at a time, and the target's rank in the top 10 is recorded after
every keystroke. MRR is the reciprocal rank averaged over all prefixes; S@k after L characters is the share
of targets in the top k; keystrokes to top 5 is the mean number of characters typed before the target
appears in the top 5. Popular targets are 500 titles drawn in proportion to their points; uniform targets
are 300 drawn uniformly. The typo'd variants have one edit in the first word of 5 or more letters.

### Popular targets

| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.861** | **0.204** | **0.413** | **0.681** | **0.663** | **4.8** | **0.827** | **0.367** | **0.590** | 0.996 |
| tantivy | 0.804 | 0.094 | 0.230 | 0.437 | 0.460 | 7.0 | 0.734 | 0.197 | 0.352 | 0.959 |
| Typesense | 0.784 | 0.066 | 0.192 | 0.375 | 0.456 | 6.9 | 0.772 | 0.158 | 0.393 | **0.998** |
| Typesense, buckets | 0.787 | 0.084 | 0.202 | 0.387 | 0.462 | 6.8 | 0.774 | 0.168 | 0.402 | 0.996 |
| Meilisearch | 0.846 | 0.152 | 0.383 | 0.627 | 0.653 | 5.2 | 0.804 | 0.348 | **0.590** | 0.969 |
| Meilisearch, popfirst | 0.789 | 0.086 | 0.210 | 0.411 | 0.418 | 7.1 | 0.746 | 0.182 | 0.355 | 0.953 |

### Uniform targets

| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.804** | 0.027 | **0.184** | **0.338** | **0.465** | **7.5** | **0.766** | 0.138 | 0.342 | 0.983 |
| tantivy | 0.738 | 0.007 | 0.080 | 0.137 | 0.234 | 10.2 | 0.683 | 0.074 | 0.191 | 0.943 |
| Typesense | 0.755 | 0.017 | 0.084 | 0.157 | 0.278 | 9.5 | 0.744 | 0.077 | 0.208 | **0.997** |
| Typesense, buckets | 0.754 | 0.017 | 0.087 | 0.157 | 0.268 | 9.6 | 0.743 | 0.081 | 0.201 | **0.997** |
| Meilisearch | 0.798 | **0.030** | 0.181 | 0.331 | 0.438 | 7.6 | 0.762 | **0.144** | **0.369** | 0.950 |
| Meilisearch, popfirst | 0.731 | 0.013 | 0.070 | 0.134 | 0.214 | 10.2 | 0.699 | 0.064 | 0.185 | 0.936 |

All metrics, including S@10 and characters saved, are in [`bench/results/hn/results.md`](https://github.com/sayef/completr/blob/main/bench/results/hn/results.md),
and the raw numbers in [`bench/results/hn/`](https://github.com/sayef/completr/tree/main/bench/results/hn).

## Caveats

- **In-process against network.** completr and tantivy run inside the benchmark process; Typesense and
  Meilisearch answer over localhost HTTP, which adds about 1 ms per query and client-side work. This
  favours completr and tantivy in the latency and throughput tables. The engine-reported server times
  exclude the network but are rounded to whole milliseconds.
- **Typo'd typing on the long tail.** completr has the best typo MRR on both target sets, but for
  uniformly drawn targets Meilisearch ranks the target first more often once the misspelt word is complete
  (at 8 characters, 37% against 34%). Typesense reaches the top result for almost every typo'd target,
  completr for over 98%.
- **Footprint against tantivy.** completr's segment is about twice the size of tantivy's index and uses
  more memory after queries, because it stores hashed spelling variants and a title trie for prefix scans
  (see [where the bytes go](#where-the-bytes-go)). It opens in 1.5 ms against 0.5 ms for tantivy, and its
  build memory grows with the segment, about 1.2 KB per document, where tantivy's is capped by its writer
  budget.
- **tantivy's merge.** tantivy merges segments in the background, so its size varies between runs (10 to
  11 MB here).
- **Recommended settings, not tuning.** Each engine runs with the configuration its documentation
  recommends for popularity ranking. Other settings trade clean against typo'd quality differently, as
  the variants show.
- **One dataset, synthetic queries.** Short English titles with one popularity signal; typos are random
  edits, not real misspellings. The quality metrics count only the target's rank.
- **One machine, one run.** Timings move by tens of percent between runs and machines. Compare engines
  within a run.
- **tantivy is emulated.** Its quality reflects the harness's autocomplete adapter as much as tantivy.
- **The dataset is not redistributed.** The HN data comes from the official HN API and its licence is not
  available; the harness downloads it at run time.
