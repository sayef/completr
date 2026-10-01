Measured 2026-10-01 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | completr 0.1.0 | 400.0 ms | 73 MB | 23 MB | 5 MB | 32 MB | 1.4 ms |
| tantivy | tantivy-py 0.26.2 | 1.06 s | 89 MB | 10 MB | 3 MB | 19 MB | 0.5 ms |
| typesense | typesense 30.2 | 3.62 s | 280 MB | 40 MB | 281 MB | 222 MB | 3.28 s |
| typesense-buckets | typesense 30.2 | 3.63 s | 267 MB | 39 MB | 266 MB | 285 MB | 3.28 s |
| meilisearch | meilisearch 1.54.2 | 1.94 s | 1034 MB | 136 MB | 1023 MB | 854 MB | 219.2 ms |
| meilisearch-popfirst | meilisearch 1.54.2 | 1.99 s | 1122 MB | 136 MB | 1117 MB | 662 MB | 222.0 ms |

completr with build_threads=8: 271.4 ms.
completr with UUID string ids: 26 MB on disk, 842.3 ms to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (23292) | completr | 0.179 | 0.514 | 0.984 | 0.241 | - | - | - |
| prefix (23292) | tantivy | 0.431 | 1.417 | 2.730 | 0.618 | - | - | - |
| prefix (23292) | typesense | 1.379 | 9.080 | 41.300 | 4.030 | 0 | 40 | 2.91 |
| prefix (23292) | typesense-buckets | 1.456 | 9.188 | 41.755 | 4.093 | 0 | 40 | 2.93 |
| prefix (23292) | meilisearch | 1.459 | 2.068 | 2.782 | 1.512 | 0 | 1 | 0.34 |
| prefix (23292) | meilisearch-popfirst | 1.513 | 2.099 | 2.719 | 1.556 | 0 | 2 | 0.41 |
| typo1 (1000) | completr | 0.189 | 0.569 | 0.950 | 0.263 | - | - | - |
| typo1 (1000) | tantivy | 0.220 | 0.690 | 2.166 | 0.394 | - | - | - |
| typo1 (1000) | typesense | 1.085 | 2.181 | 8.705 | 1.573 | 0 | 8 | 0.50 |
| typo1 (1000) | typesense-buckets | 1.121 | 2.272 | 8.778 | 1.631 | 0 | 8 | 0.51 |
| typo1 (1000) | meilisearch | 1.260 | 1.714 | 2.344 | 1.307 | 0 | 1 | 0.09 |
| typo1 (1000) | meilisearch-popfirst | 1.294 | 1.704 | 2.279 | 1.325 | 0 | 1 | 0.12 |
| typo2 (1000) | completr | 0.148 | 0.599 | 0.971 | 0.248 | - | - | - |
| typo2 (1000) | tantivy | 0.511 | 0.734 | 1.475 | 0.489 | - | - | - |
| typo2 (1000) | typesense | 1.257 | 2.394 | 8.837 | 1.712 | 0 | 8 | 0.57 |
| typo2 (1000) | typesense-buckets | 1.325 | 2.497 | 8.859 | 1.775 | 0 | 8 | 0.59 |
| typo2 (1000) | meilisearch | 1.298 | 1.749 | 2.277 | 1.339 | 0 | 1 | 0.10 |
| typo2 (1000) | meilisearch-popfirst | 1.327 | 1.688 | 2.288 | 1.344 | 0 | 1 | 0.10 |
| multiword (2000) | completr | 0.128 | 0.293 | 0.469 | 0.154 | - | - | - |
| multiword (2000) | tantivy | 0.237 | 0.736 | 1.766 | 0.388 | - | - | - |
| multiword (2000) | typesense | 0.958 | 2.874 | 15.267 | 1.824 | 0 | 14 | 0.82 |
| multiword (2000) | typesense-buckets | 0.991 | 2.920 | 13.297 | 1.837 | 0 | 12 | 0.81 |
| multiword (2000) | meilisearch | 1.389 | 1.827 | 2.417 | 1.443 | 0 | 1 | 0.19 |
| multiword (2000) | meilisearch-popfirst | 1.456 | 1.892 | 2.498 | 1.505 | 0 | 1 | 0.26 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr | 28372 | threads |
| tantivy | 5655 | threads |
| typesense | 1725 | processes |
| typesense-buckets | 1680 | processes |
| meilisearch | 3791 | processes |
| meilisearch-popfirst | 3762 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 500 | 0.861 | 0.204 | 0.434 | 0.530 | 0.413 | 0.681 | 0.764 | 0.663 | 0.869 | 0.902 | 1.000 | 7.4 | 1.000 | 4.8 | 0.874 |
| tantivy | 500 | 0.804 | 0.094 | 0.208 | 0.294 | 0.230 | 0.437 | 0.523 | 0.460 | 0.683 | 0.777 | 0.998 | 9.9 | 1.000 | 7.0 | 0.816 |
| typesense | 500 | 0.784 | 0.066 | 0.178 | 0.256 | 0.192 | 0.375 | 0.461 | 0.456 | 0.667 | 0.737 | 0.998 | 9.7 | 0.998 | 6.9 | 0.814 |
| typesense-buckets | 500 | 0.787 | 0.084 | 0.190 | 0.266 | 0.202 | 0.387 | 0.469 | 0.462 | 0.671 | 0.737 | 0.996 | 9.6 | 0.998 | 6.8 | 0.815 |
| meilisearch | 500 | 0.846 | 0.152 | 0.386 | 0.508 | 0.383 | 0.627 | 0.719 | 0.653 | 0.849 | 0.896 | 0.998 | 7.7 | 0.998 | 5.2 | 0.860 |
| meilisearch-popfirst | 500 | 0.789 | 0.086 | 0.206 | 0.292 | 0.210 | 0.411 | 0.517 | 0.418 | 0.673 | 0.769 | 0.990 | 10.3 | 0.998 | 7.1 | 0.808 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 488 | 0.827 | 0.172 | 0.346 | 0.422 | 0.367 | 0.592 | 0.668 | 0.590 | 0.803 | 0.848 | 0.996 | 7.8 | 0.996 | 5.2 | 0.863 |
| tantivy | 488 | 0.734 | 0.080 | 0.174 | 0.242 | 0.197 | 0.385 | 0.467 | 0.352 | 0.572 | 0.672 | 0.959 | 10.3 | 0.969 | 7.3 | 0.787 |
| typesense | 488 | 0.772 | 0.061 | 0.152 | 0.217 | 0.158 | 0.305 | 0.381 | 0.393 | 0.594 | 0.662 | 0.998 | 10.2 | 0.998 | 7.3 | 0.807 |
| typesense-buckets | 488 | 0.774 | 0.076 | 0.158 | 0.223 | 0.168 | 0.318 | 0.385 | 0.402 | 0.600 | 0.664 | 0.996 | 10.0 | 0.998 | 7.3 | 0.807 |
| meilisearch | 488 | 0.804 | 0.131 | 0.309 | 0.395 | 0.348 | 0.590 | 0.682 | 0.590 | 0.785 | 0.838 | 0.969 | 7.9 | 0.977 | 5.5 | 0.838 |
| meilisearch-popfirst | 488 | 0.746 | 0.074 | 0.172 | 0.242 | 0.182 | 0.367 | 0.465 | 0.355 | 0.588 | 0.693 | 0.953 | 10.5 | 0.969 | 7.4 | 0.783 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 300 | 0.804 | 0.027 | 0.127 | 0.203 | 0.184 | 0.338 | 0.418 | 0.465 | 0.656 | 0.722 | 0.997 | 9.9 | 1.000 | 7.5 | 0.813 |
| tantivy | 300 | 0.738 | 0.007 | 0.020 | 0.040 | 0.080 | 0.137 | 0.191 | 0.234 | 0.381 | 0.468 | 0.987 | 12.8 | 0.997 | 10.2 | 0.747 |
| typesense | 300 | 0.755 | 0.017 | 0.023 | 0.037 | 0.084 | 0.157 | 0.197 | 0.278 | 0.455 | 0.522 | 1.000 | 11.6 | 1.000 | 9.5 | 0.761 |
| typesense-buckets | 300 | 0.754 | 0.017 | 0.020 | 0.033 | 0.087 | 0.157 | 0.197 | 0.268 | 0.445 | 0.512 | 1.000 | 11.7 | 1.000 | 9.6 | 0.759 |
| meilisearch | 300 | 0.798 | 0.030 | 0.107 | 0.180 | 0.181 | 0.331 | 0.428 | 0.438 | 0.652 | 0.719 | 1.000 | 10.1 | 1.000 | 7.6 | 0.809 |
| meilisearch-popfirst | 300 | 0.731 | 0.013 | 0.023 | 0.047 | 0.070 | 0.134 | 0.194 | 0.214 | 0.378 | 0.472 | 0.983 | 13.0 | 0.997 | 10.2 | 0.745 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 298 | 0.766 | 0.023 | 0.104 | 0.174 | 0.138 | 0.272 | 0.352 | 0.342 | 0.534 | 0.607 | 0.983 | 10.8 | 0.990 | 8.2 | 0.795 |
| tantivy | 298 | 0.683 | 0.003 | 0.017 | 0.034 | 0.074 | 0.124 | 0.161 | 0.191 | 0.319 | 0.386 | 0.943 | 13.1 | 0.953 | 10.4 | 0.716 |
| typesense | 298 | 0.744 | 0.013 | 0.020 | 0.030 | 0.077 | 0.138 | 0.168 | 0.208 | 0.369 | 0.426 | 0.997 | 12.1 | 0.997 | 10.0 | 0.755 |
| typesense-buckets | 298 | 0.743 | 0.013 | 0.017 | 0.027 | 0.081 | 0.138 | 0.168 | 0.201 | 0.362 | 0.409 | 0.997 | 12.2 | 0.997 | 10.1 | 0.753 |
| meilisearch | 298 | 0.762 | 0.027 | 0.084 | 0.144 | 0.144 | 0.295 | 0.389 | 0.369 | 0.570 | 0.651 | 0.950 | 10.2 | 0.966 | 7.9 | 0.785 |
| meilisearch-popfirst | 298 | 0.699 | 0.010 | 0.017 | 0.040 | 0.064 | 0.121 | 0.168 | 0.185 | 0.336 | 0.403 | 0.936 | 13.1 | 0.953 | 10.4 | 0.718 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 25,000 | ok | 108.4 ms | 23 MB | 6 MB | 3 MB | 0.11 / 0.45 |
| completr | 50,000 | ok | 176.7 ms | 32 MB | 11 MB | 4 MB | 0.16 / 0.94 |
| completr | 124,440 | ok | 424.3 ms | 75 MB | 23 MB | 5 MB | 0.18 / 1.59 |
| tantivy | 25,000 | ok | 624.2 ms | 72 MB | 2 MB | 3 MB | 0.22 / 1.17 |
| tantivy | 50,000 | ok | 667.5 ms | 81 MB | 4 MB | 3 MB | 0.27 / 1.66 |
| tantivy | 124,440 | ok | 977.4 ms | 88 MB | 10 MB | 3 MB | 0.36 / 2.63 |
| typesense | 25,000 | ok | 709.1 ms | 175 MB | 7 MB | 176 MB | 3.82 / 35.29 |
| typesense | 50,000 | ok | 1.43 s | 235 MB | 16 MB | 218 MB | 4.96 / 46.79 |
| typesense | 124,440 | ok | 3.66 s | 273 MB | 39 MB | 274 MB | 1.50 / 40.27 |
| meilisearch | 25,000 | ok | 557.0 ms | 669 MB | 29 MB | 669 MB | 0.98 / 1.68 |
| meilisearch | 50,000 | ok | 852.8 ms | 819 MB | 53 MB | 819 MB | 1.08 / 1.93 |
| meilisearch | 124,440 | ok | 1.86 s | 1137 MB | 136 MB | 1131 MB | 1.47 / 2.47 |
