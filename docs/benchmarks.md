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
- **Footprint.** It builds its index fastest (0.73 s) and opens it in about a millisecond. Its 23 MB
  segment and 31 MB of resident memory after queries are far less than the servers', and about twice
  tantivy's 10 MB index and 18 MB, because completr also stores a title-prefix trie and precomputed
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
| Date | 2026-09-30; completr and tantivy re-measured 2026-10-01 on the same machine |
| Machine | Apple M1 Pro, 10 cores, 16 GiB RAM, macOS 26.7, Python 3.12.11 (3.13.7 for the re-runs) |
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

| Engine | Index time | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|
| completr | **0.73 s** (0.30 s with 8 threads) | 23 MB | 4 MB | 31 MB | 1.2 ms |
| tantivy | 1.02 s | **10 MB** | **3 MB** | **18 MB** | **0.6 ms** |
| Typesense | 3.74 s | 39 MB | 283 MB | 191 MB | 3.37 s |
| Typesense, buckets | 3.66 s | 39 MB | 268 MB | 157 MB | 3.38 s |
| Meilisearch | 2.08 s | 136 MB | 811 MB | 136 MB | 223 ms |
| Meilisearch, popfirst | 1.93 s | 136 MB | 877 MB | 145 MB | 220 ms |

For completr and tantivy, memory is the RSS growth of a fresh process after opening the index and after
5,000 prefix queries; for the servers, it is the RSS of the server process after indexing and after all
queries. completr opens a local segment without reading it whole; `Segment::verify` checks its checksum.

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
| Prefixes as typed (23,292) | completr | **0.17** | **0.49** | **0.88** | - | - |
| | tantivy | 0.43 | 1.43 | 2.75 | - | - |
| | Typesense | 1.47 | 9.16 | 41.66 | 0 | 40 |
| | Typesense, buckets | 1.56 | 9.60 | 41.93 | 0 | 41 |
| | Meilisearch | 1.89 | 5.97 | 10.00 | 1 | 5 |
| | Meilisearch, popfirst | 1.59 | 2.30 | 3.01 | 0 | 2 |
| One-edit typos (1,000) | completr | **0.16** | **0.51** | **0.85** | - | - |
| | tantivy | 0.22 | 0.70 | 2.14 | - | - |
| | Typesense | 1.09 | 2.20 | 8.75 | 0 | 8 |
| | Typesense, buckets | 1.19 | 2.38 | 9.03 | 0 | 8 |
| | Meilisearch | 1.35 | 1.87 | 4.33 | 0 | 2 |
| | Meilisearch, popfirst | 1.29 | 1.68 | 2.36 | 0 | 1 |
| Two-edit typos (1,000) | completr | **0.14** | **0.56** | **0.90** | - | - |
| | tantivy | 0.51 | 0.73 | 1.56 | - | - |
| | Typesense | 1.30 | 2.45 | 8.82 | 0 | 7 |
| | Typesense, buckets | 1.38 | 2.63 | 8.92 | 0 | 7 |
| | Meilisearch | 1.42 | 1.94 | 2.61 | 0 | 1 |
| | Meilisearch, popfirst | 1.28 | 1.59 | 2.05 | 0 | 1 |
| Multi-word (2,000) | completr | **0.13** | **0.29** | **0.46** | - | - |
| | tantivy | 0.24 | 0.75 | 1.77 | - | - |
| | Typesense | 0.95 | 2.88 | 13.55 | 0 | 12 |
| | Typesense, buckets | 1.01 | 3.03 | 13.43 | 0 | 12 |
| | Meilisearch | 1.44 | 1.89 | 2.59 | 0 | 1 |
| | Meilisearch, popfirst | 1.44 | 1.84 | 2.47 | 0 | 1 |

## Throughput

8 closed-loop clients for 10 seconds on the prefix set: threads in one process for completr and tantivy,
client processes over HTTP for the servers.

| Engine | Queries per second |
|---|---|
| completr | **30,096** |
| tantivy | 5,718 |
| Typesense | 1,727 |
| Typesense, buckets | 1,640 |
| Meilisearch | 3,955 |
| Meilisearch, popfirst | 3,757 |

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
| tantivy | 0.804 | 0.096 | 0.230 | 0.437 | 0.460 | 7.0 | 0.734 | 0.197 | 0.352 | 0.959 |
| Typesense | 0.784 | 0.066 | 0.192 | 0.375 | 0.456 | 6.9 | 0.772 | 0.158 | 0.393 | **0.998** |
| Typesense, buckets | 0.787 | 0.084 | 0.202 | 0.387 | 0.462 | 6.8 | 0.774 | 0.168 | 0.402 | 0.996 |
| Meilisearch | 0.846 | 0.152 | 0.383 | 0.627 | 0.653 | 5.2 | 0.804 | 0.348 | **0.590** | 0.969 |
| Meilisearch, popfirst | 0.789 | 0.086 | 0.210 | 0.411 | 0.418 | 7.1 | 0.746 | 0.182 | 0.355 | 0.953 |

### Uniform targets

| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| completr | **0.804** | 0.027 | **0.184** | **0.338** | **0.465** | **7.5** | **0.766** | 0.138 | 0.342 | 0.983 |
| tantivy | 0.737 | 0.007 | 0.077 | 0.137 | 0.234 | 10.2 | 0.683 | 0.070 | 0.195 | 0.943 |
| Typesense | 0.755 | 0.017 | 0.084 | 0.157 | 0.278 | 9.5 | 0.744 | 0.077 | 0.208 | **0.997** |
| Typesense, buckets | 0.754 | 0.017 | 0.087 | 0.157 | 0.268 | 9.6 | 0.743 | 0.081 | 0.201 | **0.997** |
| Meilisearch | 0.798 | **0.030** | 0.181 | 0.331 | 0.438 | 7.6 | 0.762 | **0.144** | **0.369** | 0.950 |
| Meilisearch, popfirst | 0.731 | 0.013 | 0.070 | 0.134 | 0.214 | 10.2 | 0.699 | 0.064 | 0.185 | 0.936 |

All metrics, including S@10 and characters saved, are in [`bench/results/results.md`](https://github.com/sayef/completr/blob/main/bench/results/results.md),
and the raw numbers in [`bench/results/`](https://github.com/sayef/completr/tree/main/bench/results).

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
  (see [where the bytes go](#where-the-bytes-go)). It opens in 1.2 ms against 0.6 ms for tantivy.
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
