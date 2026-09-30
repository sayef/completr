# Benchmarks

strato compared with Typesense, Meilisearch and tantivy as autocompletion engines, on 124,440 Hacker News
story titles ranked by points. Every engine indexes the same documents and answers the same queries. To
reproduce, see [`bench/README.md`](../bench/README.md), which also describes every measurement and setting
in detail.

Results are measured on HN titles fetched from Meilisearch's public benchmark bucket.

!!! note "Dataset licence: not available"
    The Hacker News titles come from the official HN API and carry no explicit licence. They are not
    redistributed with strato; the harness downloads them at run time for measurement only.

## Summary

- **Clean typing.** strato ranks the wanted title highest of the engines tested: a mean reciprocal rank
  over prefixes of 0.861 for popular titles and 0.804 for uniformly drawn ones, ahead of Meilisearch (0.846
  and 0.798), tantivy, and Typesense. After 5 characters, the popular target is first 41% of the time.
- **Typing with a typo.** strato is on par with the best: 0.806 and 0.747, against 0.804 and 0.762 for
  Meilisearch and 0.772 and 0.744 for Typesense. It leads on popular targets and is second on uniform
  ones, where Meilisearch ranks the target first more often once the misspelt word is complete.
- **Latency and throughput.** In process, strato answers prefix queries in 0.26 ms at the median and
  2.2 ms at p99, and serves 16,000 queries per second with 8 threads, about 3 times tantivy and 4 times the
  fastest server. The servers' round trips include about 1 ms of localhost HTTP, so this comparison
  favours strato.
- **Footprint.** A 39 MB segment file and 49 MB of resident memory, far less than the servers but about 8
  times tantivy's 5 MB index.

## Setup

| | |
|---|---|
| Date | 2026-09-30 |
| Machine | Apple M1 Pro, 10 cores, 16 GiB RAM, macOS 26.7, Python 3.12.11 |
| Engines | strato 0.1.0; tantivy-py 0.26.2; Typesense 30.2; Meilisearch 1.54.2 (official release binaries) |
| Corpus | 124,440 deduplicated HN story titles, points as popularity |
| Queries | Limit 10; the fixed-seed sample in [`bench/samples.json`](../bench/samples.json) |

Each competitor uses the recommended way to rank by popularity: Typesense with `score` as
`default_sorting_field`, Meilisearch with the custom ranking rule `score:desc` after the default rules.
Two variants give popularity more weight: Typesense `buckets` (`sort_by=_text_match(buckets: 10):desc,score:desc`)
and Meilisearch `popfirst` (`score:desc` right after `words` and `typo`). tantivy has no autocomplete mode,
so the harness emulates one: complete words as terms, the last word as a prefix, and a fuzzy pass with
Meilisearch's typo thresholds when fewer than 10 documents match. strato uses default options, with
popularity `log1p(points) / log1p(max points)`.

## Indexing, size and memory

| Engine | Index time | On disk | Memory after open or index | Memory after queries | Open or restart to first hit |
|---|---|---|---|---|---|
| strato | 1.11 s (0.45 s with 8 threads) | 39 MB | 45 MB | 49 MB | 33 ms |
| tantivy | 1.01 s | 5 MB | 3 MB | 13 MB | 0.5 ms |
| Typesense | 3.74 s | 39 MB | 283 MB | 191 MB | 3.37 s |
| Typesense, buckets | 3.66 s | 39 MB | 268 MB | 157 MB | 3.38 s |
| Meilisearch | 2.08 s | 136 MB | 811 MB | 136 MB | 223 ms |
| Meilisearch, popfirst | 1.93 s | 136 MB | 877 MB | 145 MB | 220 ms |

For strato and tantivy, memory is the RSS growth of a fresh process after opening the index and after
5,000 prefix queries; for the servers, it is the RSS of the server process after indexing and after all
queries. strato's open time includes verifying the segment's checksum.

## Latency

Single client, limit 10, in milliseconds. In-process time for strato and tantivy; for the servers, the
round trip over localhost HTTP and the time the engine reports (whole milliseconds only).

| Set | Engine | p50 | p90 | p99 | Engine-reported p50 | Engine-reported p99 |
|---|---|---|---|---|---|---|
| Prefixes as typed (23,292) | strato | 0.26 | 1.03 | 2.20 | - | - |
| | tantivy | 0.44 | 1.45 | 2.82 | - | - |
| | Typesense | 1.47 | 9.16 | 41.66 | 0 | 40 |
| | Typesense, buckets | 1.56 | 9.60 | 41.93 | 0 | 41 |
| | Meilisearch | 1.89 | 5.97 | 10.00 | 1 | 5 |
| | Meilisearch, popfirst | 1.59 | 2.30 | 3.01 | 0 | 2 |
| One-edit typos (1,000) | strato | 0.21 | 1.30 | 2.76 | - | - |
| | tantivy | 0.22 | 0.71 | 2.15 | - | - |
| | Typesense | 1.09 | 2.20 | 8.75 | 0 | 8 |
| | Typesense, buckets | 1.19 | 2.38 | 9.03 | 0 | 8 |
| | Meilisearch | 1.35 | 1.87 | 4.33 | 0 | 2 |
| | Meilisearch, popfirst | 1.29 | 1.68 | 2.36 | 0 | 1 |
| Two-edit typos (1,000) | strato | 0.17 | 1.09 | 2.57 | - | - |
| | tantivy | 0.51 | 0.74 | 1.51 | - | - |
| | Typesense | 1.30 | 2.45 | 8.82 | 0 | 7 |
| | Typesense, buckets | 1.38 | 2.63 | 8.92 | 0 | 7 |
| | Meilisearch | 1.42 | 1.94 | 2.61 | 0 | 1 |
| | Meilisearch, popfirst | 1.28 | 1.59 | 2.05 | 0 | 1 |
| Multi-word (2,000) | strato | 0.17 | 0.65 | 1.21 | - | - |
| | tantivy | 0.23 | 0.73 | 1.75 | - | - |
| | Typesense | 0.95 | 2.88 | 13.55 | 0 | 12 |
| | Typesense, buckets | 1.01 | 3.03 | 13.43 | 0 | 12 |
| | Meilisearch | 1.44 | 1.89 | 2.59 | 0 | 1 |
| | Meilisearch, popfirst | 1.44 | 1.84 | 2.47 | 0 | 1 |

## Throughput

8 closed-loop clients for 10 seconds on the prefix set: threads in one process for strato and tantivy,
client processes over HTTP for the servers.

| Engine | Queries per second |
|---|---|
| strato | 15,964 |
| tantivy | 5,329 |
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
| strato | **0.861** | **0.204** | **0.413** | **0.681** | **0.663** | **4.8** | **0.806** | 0.320 | 0.484 | 0.992 |
| tantivy | 0.804 | 0.096 | 0.230 | 0.437 | 0.460 | 7.0 | 0.735 | 0.197 | 0.352 | 0.959 |
| Typesense | 0.784 | 0.066 | 0.192 | 0.375 | 0.456 | 6.9 | 0.772 | 0.158 | 0.393 | **0.998** |
| Typesense, buckets | 0.787 | 0.084 | 0.202 | 0.387 | 0.462 | 6.8 | 0.774 | 0.168 | 0.402 | 0.996 |
| Meilisearch | 0.846 | 0.152 | 0.383 | 0.627 | 0.653 | 5.2 | 0.804 | **0.348** | **0.590** | 0.969 |
| Meilisearch, popfirst | 0.789 | 0.086 | 0.210 | 0.411 | 0.418 | 7.1 | 0.746 | 0.182 | 0.355 | 0.953 |

### Uniform targets

| Engine | MRR | S@1, 3 chars | S@1, 5 chars | S@5, 5 chars | S@1, 8 chars | Keystrokes to top 5 | MRR, typo | S@1, 5 chars, typo | S@1, 8 chars, typo | Reached top 1, typo |
|---|---|---|---|---|---|---|---|---|---|---|
| strato | **0.804** | 0.027 | **0.191** | **0.351** | **0.465** | **7.5** | 0.747 | **0.148** | 0.302 | 0.980 |
| tantivy | 0.738 | 0.010 | 0.080 | 0.137 | 0.231 | 10.1 | 0.683 | 0.074 | 0.188 | 0.943 |
| Typesense | 0.755 | 0.017 | 0.084 | 0.157 | 0.278 | 9.5 | 0.744 | 0.077 | 0.208 | **0.997** |
| Typesense, buckets | 0.754 | 0.017 | 0.087 | 0.157 | 0.268 | 9.6 | 0.743 | 0.081 | 0.201 | **0.997** |
| Meilisearch | 0.798 | **0.030** | 0.181 | 0.331 | 0.438 | 7.6 | **0.762** | 0.144 | **0.369** | 0.950 |
| Meilisearch, popfirst | 0.731 | 0.013 | 0.070 | 0.134 | 0.214 | 10.2 | 0.699 | 0.064 | 0.185 | 0.936 |

All metrics, including S@10 and characters saved, are in [`bench/results/results.md`](../bench/results/results.md),
and the raw numbers in [`bench/results/`](../bench/results/).

## Caveats

- **In-process against network.** strato and tantivy run inside the benchmark process; Typesense and
  Meilisearch answer over localhost HTTP, which adds about 1 ms per query and client-side work. This
  favours strato and tantivy in the latency and throughput tables. The engine-reported server times
  exclude the network but are rounded to whole milliseconds.
- **Typo'd typing is close, not won.** strato leads on popular targets, but Meilisearch ranks a typo'd
  target first more often once the misspelt word is complete (at 8 characters, 59% of popular targets
  against 48% for strato), and is ahead on uniform targets (0.762 against 0.747). Typesense reaches the top
  result for almost every typo'd target, strato for 99% of popular and 98% of uniform ones.
- **Footprint against tantivy.** strato's segment is about 8 times larger than tantivy's index and uses
  more memory, because it stores precomputed spelling variants and the title texts (FSST-compressed) for
  suggestions.
  Opening it takes 33 ms, against 0.5 ms for tantivy, because the checksum is verified.
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
