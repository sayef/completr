Measured 2026-10-04 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | completr 0.1.0 | 80.68 s | 315 MB | 1117 MB | 50 MB | 121 MB | 34.2 ms |
| tantivy | tantivy-py 0.26.2 | 163.86 s | 499 MB | 1319 MB | 13 MB | 125 MB | 263.9 ms |
| typesense | typesense 30.2 | 1187.29 s | 3127 MB | 4750 MB | 3090 MB | 3547 MB | 431.66 s |
| meilisearch | meilisearch 1.54.2 | 231.03 s | 6899 MB | 20298 MB | 2412 MB | 2372 MB | 242.2 ms |

completr with build_threads=8: 64.62 s.
completr with UUID string ids: 1998 MB on disk, 229.38 s to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (11301) | completr, compacted | 0.817 | 6.958 | 23.063 | 2.548 | - | - | - |
| prefix (11301) | tantivy | 6.228 | 50.674 | 207.548 | 18.617 | - | - | - |
| prefix (11301) | typesense | 11.697 | 122.775 | 596.277 | 50.687 | 10 | 594 | 49.27 |
| prefix (11301) | meilisearch | 13.954 | 28.142 | 48.127 | 16.281 | 13 | 47 | 14.88 |
| typo1 (1000) | completr, compacted | 2.904 | 10.848 | 34.235 | 4.838 | - | - | - |
| typo1 (1000) | tantivy | 5.559 | 11.814 | 25.200 | 7.265 | - | - | - |
| typo1 (1000) | typesense | 4.378 | 27.767 | 169.103 | 14.361 | 3 | 168 | 13.06 |
| typo1 (1000) | meilisearch | 12.795 | 24.956 | 40.376 | 14.746 | 11 | 39 | 13.41 |
| typo2 (1000) | completr, compacted | 1.607 | 7.241 | 25.532 | 3.298 | - | - | - |
| typo2 (1000) | tantivy | 6.437 | 11.569 | 29.367 | 8.117 | - | - | - |
| typo2 (1000) | typesense | 6.392 | 24.751 | 107.821 | 13.382 | 5 | 106 | 12.09 |
| typo2 (1000) | meilisearch | 13.084 | 25.356 | 35.147 | 14.868 | 12 | 34 | 13.52 |
| multiword (2000) | completr, compacted | 2.290 | 8.344 | 34.088 | 4.043 | - | - | - |
| multiword (2000) | tantivy | 4.012 | 11.567 | 26.174 | 5.557 | - | - | - |
| multiword (2000) | typesense | 5.569 | 24.998 | 109.161 | 13.774 | 4 | 108 | 12.50 |
| multiword (2000) | meilisearch | 18.211 | 32.599 | 52.191 | 20.199 | 17 | 51 | 18.78 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr, compacted | 2623 | threads |
| tantivy | 262 | threads |
| typesense | 159 | processes |
| meilisearch | 237 | processes |

## Labelled: prefixes users typed, and the term they then searched

| engine | queries | in corpus | S@1 | S@10 | MRR@10 |
| --- | --- | --- | --- | --- | --- |
| completr, compacted | 20000 | 0.740 | 0.132 | 0.285 | 0.180 |
| tantivy | 20000 | 0.740 | 0.104 | 0.238 | 0.144 |
| typesense | 20000 | 0.740 | 0.066 | 0.173 | 0.099 |
| meilisearch | 20000 | 0.740 | 0.099 | 0.252 | 0.147 |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.468 | 0.034 | 0.118 | 0.166 | 0.096 | 0.272 | 0.335 | 0.275 | 0.482 | 0.575 | 1.000 | 12.2 | 1.000 | 9.0 | 0.508 |
| tantivy | 500 | 0.403 | 0.042 | 0.114 | 0.150 | 0.102 | 0.220 | 0.283 | 0.209 | 0.397 | 0.480 | 0.766 | 12.5 | 0.966 | 10.1 | 0.445 |
| typesense | 500 | 0.297 | 0.004 | 0.018 | 0.028 | 0.035 | 0.150 | 0.203 | 0.097 | 0.283 | 0.355 | 0.940 | 14.9 | 0.996 | 11.0 | 0.388 |
| meilisearch | 500 | 0.400 | 0.004 | 0.020 | 0.042 | 0.041 | 0.209 | 0.278 | 0.180 | 0.431 | 0.533 | 1.000 | 12.9 | 1.000 | 9.5 | 0.466 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 474 | 0.408 | 0.025 | 0.089 | 0.127 | 0.064 | 0.174 | 0.229 | 0.188 | 0.379 | 0.468 | 0.914 | 13.3 | 0.983 | 9.9 | 0.475 |
| tantivy | 474 | 0.322 | 0.032 | 0.084 | 0.110 | 0.042 | 0.132 | 0.191 | 0.140 | 0.295 | 0.357 | 0.616 | 13.3 | 0.863 | 10.6 | 0.394 |
| typesense | 474 | 0.205 | 0.002 | 0.015 | 0.023 | 0.017 | 0.072 | 0.100 | 0.055 | 0.153 | 0.195 | 0.759 | 17.2 | 0.846 | 12.9 | 0.290 |
| meilisearch | 474 | 0.357 | 0.002 | 0.017 | 0.036 | 0.028 | 0.134 | 0.200 | 0.125 | 0.346 | 0.433 | 0.789 | 13.5 | 0.914 | 10.1 | 0.428 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 300 | 0.376 | 0.000 | 0.000 | 0.003 | 0.013 | 0.043 | 0.053 | 0.074 | 0.172 | 0.250 | 0.983 | 16.1 | 0.993 | 13.1 | 0.396 |
| tantivy | 300 | 0.275 | 0.000 | 0.003 | 0.007 | 0.007 | 0.040 | 0.057 | 0.037 | 0.115 | 0.169 | 0.663 | 17.3 | 0.903 | 14.7 | 0.312 |
| typesense | 300 | 0.299 | 0.000 | 0.003 | 0.007 | 0.010 | 0.040 | 0.050 | 0.051 | 0.122 | 0.152 | 0.887 | 17.1 | 0.997 | 14.7 | 0.328 |
| meilisearch | 300 | 0.375 | 0.000 | 0.003 | 0.007 | 0.013 | 0.053 | 0.070 | 0.078 | 0.176 | 0.257 | 1.000 | 16.1 | 1.000 | 13.1 | 0.396 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 292 | 0.332 | 0.000 | 0.000 | 0.000 | 0.007 | 0.017 | 0.024 | 0.038 | 0.104 | 0.167 | 0.914 | 17.1 | 0.976 | 13.9 | 0.369 |
| tantivy | 292 | 0.234 | 0.000 | 0.003 | 0.003 | 0.000 | 0.014 | 0.034 | 0.031 | 0.076 | 0.111 | 0.555 | 17.4 | 0.812 | 15.4 | 0.276 |
| typesense | 292 | 0.221 | 0.000 | 0.003 | 0.003 | 0.003 | 0.017 | 0.027 | 0.028 | 0.062 | 0.073 | 0.784 | 19.0 | 0.880 | 16.3 | 0.247 |
| meilisearch | 292 | 0.343 | 0.000 | 0.003 | 0.003 | 0.007 | 0.031 | 0.045 | 0.052 | 0.111 | 0.174 | 0.863 | 16.6 | 0.932 | 13.6 | 0.370 |
