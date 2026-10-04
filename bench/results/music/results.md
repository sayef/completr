Measured 2026-10-04 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | completr 0.1.0 | 317.41 s | 547 MB | 3139 MB | 68 MB | 98 MB | 84.7 ms |
| tantivy | tantivy-py 0.26.2 | 306.16 s | 333 MB | 5938 MB | 3 MB | 13 MB | 138.2 ms |
| meilisearch | meilisearch 1.54.2 | 1244.32 s | 8207 MB | 39528 MB | 2427 MB | 2434 MB | 236.4 ms |

completr with build_threads=8: 299.71 s.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (17924) | completr, compacted | 2.071 | 10.437 | 43.607 | 4.485 | - | - | - |
| prefix (17924) | tantivy | 54.712 | 211.691 | 429.209 | 82.894 | - | - | - |
| prefix (17924) | meilisearch | 21.649 | 44.053 | 108.882 | 26.060 | 20 | 107 | 24.58 |
| typo1 (1000) | completr, compacted | 2.943 | 12.561 | 48.162 | 5.966 | - | - | - |
| typo1 (1000) | tantivy | 38.053 | 112.422 | 185.515 | 56.457 | - | - | - |
| typo1 (1000) | meilisearch | 16.279 | 32.484 | 55.787 | 19.191 | 15 | 54 | 17.81 |
| typo2 (1000) | completr, compacted | 2.987 | 12.326 | 27.474 | 5.238 | - | - | - |
| typo2 (1000) | tantivy | 90.546 | 120.856 | 183.231 | 82.154 | - | - | - |
| typo2 (1000) | meilisearch | 16.204 | 28.673 | 53.038 | 18.302 | 15 | 52 | 16.92 |
| multiword (2000) | completr, compacted | 1.551 | 7.213 | 48.745 | 3.616 | - | - | - |
| multiword (2000) | tantivy | 9.077 | 100.399 | 182.090 | 34.081 | - | - | - |
| multiword (2000) | meilisearch | 18.846 | 36.501 | 65.519 | 21.807 | 17 | 64 | 20.38 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr, compacted | 1355 | threads |
| tantivy | 97 | threads |
| meilisearch | 180 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.762 | 0.152 | 0.298 | 0.356 | 0.314 | 0.498 | 0.570 | 0.546 | 0.762 | 0.828 | 0.994 | 8.7 | 1.000 | 6.2 | 0.777 |
| tantivy | 500 | 0.712 | 0.130 | 0.228 | 0.282 | 0.242 | 0.418 | 0.484 | 0.426 | 0.626 | 0.706 | 0.988 | 9.9 | 0.998 | 7.3 | 0.733 |
| meilisearch | 500 | 0.701 | 0.060 | 0.104 | 0.120 | 0.204 | 0.368 | 0.446 | 0.438 | 0.670 | 0.764 | 0.996 | 9.7 | 1.000 | 7.2 | 0.729 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 451 | 0.713 | 0.091 | 0.180 | 0.237 | 0.197 | 0.361 | 0.408 | 0.415 | 0.641 | 0.710 | 0.998 | 9.6 | 1.000 | 6.9 | 0.755 |
| tantivy | 451 | 0.626 | 0.075 | 0.133 | 0.173 | 0.137 | 0.264 | 0.319 | 0.288 | 0.483 | 0.576 | 0.956 | 11.0 | 0.971 | 8.2 | 0.686 |
| meilisearch | 451 | 0.656 | 0.038 | 0.060 | 0.071 | 0.137 | 0.242 | 0.313 | 0.341 | 0.579 | 0.665 | 0.960 | 10.3 | 0.971 | 7.9 | 0.691 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 300 | 0.513 | 0.007 | 0.020 | 0.030 | 0.043 | 0.077 | 0.107 | 0.133 | 0.260 | 0.333 | 0.767 | 15.5 | 0.947 | 12.7 | 0.579 |
| tantivy | 300 | 0.457 | 0.007 | 0.013 | 0.017 | 0.030 | 0.053 | 0.067 | 0.090 | 0.163 | 0.200 | 0.750 | 17.4 | 0.947 | 14.7 | 0.519 |
| meilisearch | 300 | 0.507 | 0.010 | 0.017 | 0.023 | 0.040 | 0.100 | 0.130 | 0.147 | 0.250 | 0.297 | 0.770 | 14.9 | 0.953 | 13.0 | 0.568 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 276 | 0.475 | 0.000 | 0.004 | 0.011 | 0.018 | 0.036 | 0.062 | 0.080 | 0.174 | 0.221 | 0.757 | 16.7 | 0.938 | 13.7 | 0.554 |
| tantivy | 276 | 0.400 | 0.000 | 0.000 | 0.004 | 0.004 | 0.022 | 0.025 | 0.047 | 0.094 | 0.127 | 0.681 | 18.3 | 0.884 | 15.7 | 0.467 |
| meilisearch | 276 | 0.462 | 0.004 | 0.004 | 0.007 | 0.007 | 0.043 | 0.062 | 0.101 | 0.196 | 0.221 | 0.717 | 15.6 | 0.884 | 13.4 | 0.526 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 4,000,000 | ok | 30.59 s | 216 MB | 368 MB | 18 MB | 1.48 / 20.25 |
| completr | 16,000,000 | ok | 129.60 s | 287 MB | 1311 MB | 40 MB | 2.73 / 60.74 |
| tantivy | 4,000,000 | ok | 32.04 s | 332 MB | 692 MB | 2 MB | 20.82 / 111.16 |
| tantivy | 16,000,000 | ok | 150.33 s | 440 MB | 2421 MB | 2 MB | 30.54 / 182.28 |
| typesense | 4,000,000 | ok | 178.05 s | 849 MB | 1331 MB | 840 MB | 13.96 / 357.35 |
| typesense | 16,000,000 | ok | 1181.92 s | 1833 MB | 4480 MB | 1830 MB | 25.05 / 852.07 |
| meilisearch | 4,000,000 | ok | 65.12 s | 3798 MB | 5627 MB | 2338 MB | 3.57 / 10.49 |
| meilisearch | 16,000,000 | ok | 272.55 s | 5467 MB | 16924 MB | 2411 MB | 11.55 / 82.58 |
