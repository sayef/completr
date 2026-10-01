Measured 2026-10-01 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | completr 0.1.0 | 24.65 s | 1698 MB | 801 MB | 827 MB | 939 MB | 424.5 ms |
| tantivy | tantivy-py 0.26.2 | 29.72 s | 1760 MB | 358 MB | 5 MB | 284 MB | 2.1 ms |
| typesense | typesense 30.2 | 202.74 s | 880 MB | 1329 MB | 874 MB | 836 MB | 14.48 s |
| meilisearch | meilisearch 1.54.2 | 48.81 s | 3452 MB | 4951 MB | 2244 MB | 1901 MB | 234.9 ms |

completr with build_threads=8: 16.23 s.
completr with UUID string ids: 1034 MB on disk, 55.44 s to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (10100) | completr | 0.709 | 6.312 | 16.594 | 2.156 | - | - | - |
| prefix (10100) | tantivy | 4.326 | 36.711 | 98.538 | 12.762 | - | - | - |
| prefix (10100) | typesense | 3.684 | 40.972 | 762.229 | 35.814 | 2 | 761 | 34.56 |
| prefix (10100) | meilisearch | 4.386 | 8.200 | 16.098 | 5.167 | 3 | 15 | 3.97 |
| typo1 (1000) | completr | 2.352 | 13.964 | 24.177 | 5.101 | - | - | - |
| typo1 (1000) | tantivy | 4.508 | 16.879 | 29.694 | 8.434 | - | - | - |
| typo1 (1000) | typesense | 2.113 | 8.293 | 103.374 | 6.019 | 1 | 102 | 4.80 |
| typo1 (1000) | meilisearch | 4.044 | 6.126 | 9.848 | 4.304 | 3 | 9 | 3.13 |
| typo2 (1000) | completr | 2.028 | 17.718 | 29.188 | 5.265 | - | - | - |
| typo2 (1000) | tantivy | 12.083 | 18.198 | 29.138 | 11.269 | - | - | - |
| typo2 (1000) | typesense | 4.015 | 10.441 | 36.534 | 6.998 | 3 | 35 | 5.81 |
| typo2 (1000) | meilisearch | 4.125 | 6.083 | 9.607 | 4.284 | 3 | 8 | 3.15 |
| multiword (2000) | completr | 1.617 | 5.124 | 9.033 | 2.211 | - | - | - |
| multiword (2000) | tantivy | 4.413 | 18.240 | 60.279 | 8.064 | - | - | - |
| multiword (2000) | typesense | 2.072 | 11.113 | 90.163 | 10.219 | 1 | 89 | 9.05 |
| multiword (2000) | meilisearch | 4.944 | 7.636 | 14.018 | 5.369 | 4 | 13 | 4.21 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr | 3065 | threads |
| tantivy | 326 | threads |
| typesense | 190 | processes |
| meilisearch | 1079 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 500 | 0.573 | 0.046 | 0.138 | 0.224 | 0.220 | 0.387 | 0.443 | 0.584 | 0.749 | 0.791 | 0.986 | 8.4 | 0.992 | 6.6 | 0.575 |
| tantivy | 500 | 0.567 | 0.058 | 0.142 | 0.196 | 0.232 | 0.401 | 0.477 | 0.532 | 0.703 | 0.768 | 0.968 | 8.9 | 0.998 | 7.0 | 0.563 |
| typesense | 500 | 0.477 | 0.002 | 0.024 | 0.060 | 0.088 | 0.277 | 0.337 | 0.414 | 0.603 | 0.650 | 0.998 | 9.8 | 0.998 | 7.9 | 0.490 |
| meilisearch | 500 | 0.541 | 0.002 | 0.040 | 0.086 | 0.140 | 0.403 | 0.483 | 0.511 | 0.741 | 0.797 | 1.000 | 8.9 | 1.000 | 6.8 | 0.555 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 463 | 0.478 | 0.035 | 0.102 | 0.162 | 0.126 | 0.221 | 0.273 | 0.425 | 0.572 | 0.621 | 0.914 | 9.1 | 0.944 | 7.2 | 0.523 |
| tantivy | 463 | 0.463 | 0.041 | 0.102 | 0.149 | 0.139 | 0.256 | 0.328 | 0.370 | 0.536 | 0.593 | 0.875 | 9.6 | 0.931 | 7.6 | 0.506 |
| typesense | 463 | 0.364 | 0.002 | 0.019 | 0.048 | 0.046 | 0.150 | 0.182 | 0.255 | 0.421 | 0.471 | 0.860 | 11.1 | 0.886 | 9.1 | 0.387 |
| meilisearch | 463 | 0.479 | 0.002 | 0.037 | 0.067 | 0.108 | 0.280 | 0.347 | 0.400 | 0.630 | 0.697 | 0.896 | 9.3 | 0.935 | 7.3 | 0.508 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 300 | 0.488 | 0.003 | 0.010 | 0.013 | 0.097 | 0.174 | 0.218 | 0.292 | 0.507 | 0.585 | 0.987 | 11.8 | 1.000 | 9.3 | 0.485 |
| tantivy | 300 | 0.443 | 0.000 | 0.003 | 0.007 | 0.050 | 0.117 | 0.171 | 0.246 | 0.430 | 0.486 | 0.913 | 12.6 | 0.983 | 10.2 | 0.446 |
| typesense | 300 | 0.440 | 0.000 | 0.003 | 0.003 | 0.044 | 0.107 | 0.161 | 0.239 | 0.408 | 0.454 | 1.000 | 12.6 | 1.000 | 10.4 | 0.433 |
| meilisearch | 300 | 0.480 | 0.003 | 0.007 | 0.013 | 0.077 | 0.164 | 0.215 | 0.278 | 0.486 | 0.570 | 0.997 | 11.6 | 1.000 | 9.5 | 0.477 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 275 | 0.408 | 0.000 | 0.000 | 0.000 | 0.044 | 0.076 | 0.102 | 0.163 | 0.364 | 0.436 | 0.887 | 12.8 | 0.920 | 10.1 | 0.430 |
| tantivy | 275 | 0.385 | 0.000 | 0.000 | 0.000 | 0.025 | 0.055 | 0.069 | 0.133 | 0.292 | 0.356 | 0.847 | 13.4 | 0.909 | 11.2 | 0.404 |
| typesense | 275 | 0.336 | 0.000 | 0.000 | 0.000 | 0.018 | 0.047 | 0.062 | 0.110 | 0.242 | 0.284 | 0.862 | 14.6 | 0.880 | 12.1 | 0.344 |
| meilisearch | 275 | 0.430 | 0.000 | 0.000 | 0.004 | 0.033 | 0.065 | 0.095 | 0.182 | 0.364 | 0.447 | 0.895 | 12.5 | 0.927 | 10.3 | 0.443 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 1,000,000 | ok | 3.70 s | 248 MB | 119 MB | 32 MB | 0.27 / 4.63 |
| completr | 2,000,000 | ok | 6.81 s | 276 MB | 232 MB | 179 MB | 0.40 / 8.22 |
| completr | 4,000,000 | ok | 13.01 s | 279 MB | 452 MB | 339 MB | 0.68 / 15.96 |
| tantivy | 1,000,000 | ok | 5.49 s | 179 MB | 48 MB | 1 MB | 0.64 / 17.65 |
| tantivy | 2,000,000 | ok | 10.18 s | 274 MB | 93 MB | 1 MB | 0.89 / 25.82 |
| tantivy | 4,000,000 | ok | 15.90 s | 341 MB | 204 MB | 1 MB | 3.20 / 65.59 |
| typesense | 1,000,000 | ok | 24.79 s | 444 MB | 262 MB | 444 MB | 3.73 / 212.53 |
| typesense | 2,000,000 | ok | 50.65 s | 586 MB | 501 MB | 586 MB | 5.14 / 321.95 |
| typesense | 4,000,000 | ok | 106.22 s | 761 MB | 782 MB | 761 MB | 6.78 / 477.55 |
| meilisearch | 1,000,000 | ok | 7.90 s | 1975 MB | 739 MB | 1143 MB | 1.50 / 4.17 |
| meilisearch | 2,000,000 | ok | 12.95 s | 2222 MB | 1176 MB | 1562 MB | 1.92 / 7.02 |
| meilisearch | 4,000,000 | ok | 26.19 s | 2598 MB | 2552 MB | 1585 MB | 2.91 / 9.35 |
