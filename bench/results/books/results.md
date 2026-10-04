Measured 2026-10-04 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | completr 0.1.0 | 349.71 s | 238 MB | 3235 MB | 48 MB | 72 MB | 122.4 ms |
| completr, 93 segments | completr 0.1.0 | 168.82 s | 315 MB | 5705 MB | 78 MB | 110 MB | 3.28 s |
| tantivy | tantivy-py 0.26.2 | 198.22 s | 329 MB | 4033 MB | 5 MB | 20 MB | 240.7 ms |
| meilisearch | - | memory limit | - | - | - | - | - |

completr with build_threads=8: 275.56 s.
completr with UUID string ids: 4102 MB on disk, 531.56 s to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (27869) | completr, compacted | 3.962 | 14.873 | 43.572 | 6.550 | - | - | - |
| prefix (27869) | completr, 93 segments | 51.977 | 159.526 | 351.323 | 71.487 | - | - | - |
| prefix (27869) | tantivy | 143.021 | 508.518 | 1084.494 | 210.303 | - | - | - |
| typo1 (1000) | completr, compacted | 5.332 | 16.574 | 54.646 | 8.145 | - | - | - |
| typo1 (1000) | completr, 93 segments | 49.865 | 182.187 | 313.153 | 74.742 | - | - | - |
| typo1 (1000) | tantivy | 67.516 | 167.847 | 529.906 | 93.169 | - | - | - |
| typo2 (1000) | completr, compacted | 4.467 | 19.016 | 41.907 | 7.784 | - | - | - |
| typo2 (1000) | completr, 93 segments | 48.817 | 223.472 | 419.049 | 84.478 | - | - | - |
| typo2 (1000) | tantivy | 117.464 | 211.966 | 426.546 | 127.313 | - | - | - |
| multiword (2000) | completr, compacted | 2.346 | 8.073 | 52.843 | 4.493 | - | - | - |
| multiword (2000) | completr, 93 segments | 22.124 | 61.112 | 103.480 | 28.912 | - | - | - |
| multiword (2000) | tantivy | 24.644 | 169.174 | 435.561 | 66.450 | - | - | - |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr, compacted | 914 | threads |
| completr, 93 segments | 114 | threads |
| tantivy | 50 | threads |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.759 | 0.080 | 0.170 | 0.212 | 0.196 | 0.322 | 0.388 | 0.422 | 0.594 | 0.688 | 0.966 | 11.3 | 1.000 | 8.3 | 0.791 |
| completr, 93 segments | 500 | 0.759 | 0.080 | 0.170 | 0.212 | 0.196 | 0.322 | 0.388 | 0.422 | 0.594 | 0.688 | 0.966 | 11.3 | 1.000 | 8.3 | 0.791 |
| tantivy | 500 | 0.704 | 0.044 | 0.106 | 0.124 | 0.132 | 0.216 | 0.264 | 0.298 | 0.420 | 0.490 | 0.944 | 13.2 | 1.000 | 10.2 | 0.740 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 494 | 0.736 | 0.065 | 0.138 | 0.178 | 0.138 | 0.241 | 0.300 | 0.364 | 0.502 | 0.585 | 0.964 | 11.7 | 1.000 | 8.6 | 0.783 |
| completr, 93 segments | 494 | 0.736 | 0.065 | 0.138 | 0.178 | 0.138 | 0.241 | 0.300 | 0.364 | 0.502 | 0.585 | 0.964 | 11.7 | 1.000 | 8.6 | 0.783 |
| tantivy | 494 | 0.642 | 0.038 | 0.089 | 0.105 | 0.099 | 0.160 | 0.206 | 0.231 | 0.356 | 0.403 | 0.901 | 13.8 | 0.970 | 10.8 | 0.704 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 300 | 0.662 | 0.007 | 0.010 | 0.010 | 0.033 | 0.063 | 0.083 | 0.143 | 0.230 | 0.280 | 0.917 | 18.0 | 0.983 | 13.5 | 0.708 |
| completr, 93 segments | 300 | 0.662 | 0.007 | 0.010 | 0.010 | 0.033 | 0.063 | 0.083 | 0.143 | 0.230 | 0.280 | 0.917 | 18.0 | 0.983 | 13.5 | 0.708 |
| tantivy | 300 | 0.606 | 0.010 | 0.013 | 0.013 | 0.010 | 0.023 | 0.040 | 0.053 | 0.110 | 0.140 | 0.903 | 21.2 | 0.987 | 16.2 | 0.661 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 288 | 0.637 | 0.000 | 0.000 | 0.000 | 0.010 | 0.031 | 0.042 | 0.076 | 0.149 | 0.205 | 0.913 | 18.9 | 0.983 | 14.2 | 0.695 |
| completr, 93 segments | 288 | 0.637 | 0.000 | 0.000 | 0.000 | 0.010 | 0.031 | 0.042 | 0.076 | 0.149 | 0.205 | 0.913 | 18.9 | 0.983 | 14.2 | 0.695 |
| tantivy | 288 | 0.536 | 0.003 | 0.003 | 0.003 | 0.000 | 0.007 | 0.017 | 0.021 | 0.062 | 0.076 | 0.851 | 22.8 | 0.948 | 17.9 | 0.603 |

### misspelled targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.654 | 0.012 | 0.038 | 0.052 | 0.042 | 0.084 | 0.106 | 0.108 | 0.208 | 0.246 | 0.914 | 22.6 | 0.998 | 15.8 | 0.724 |
| completr, 93 segments | 500 | 0.654 | 0.012 | 0.038 | 0.052 | 0.042 | 0.084 | 0.106 | 0.108 | 0.208 | 0.246 | 0.914 | 22.6 | 0.998 | 15.8 | 0.724 |
| tantivy | 500 | 0.594 | 0.008 | 0.022 | 0.032 | 0.022 | 0.050 | 0.060 | 0.068 | 0.124 | 0.160 | 0.894 | 25.6 | 0.986 | 19.4 | 0.656 |

### misspelled targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.622 | 0.012 | 0.036 | 0.044 | 0.040 | 0.078 | 0.090 | 0.086 | 0.182 | 0.220 | 0.908 | 23.1 | 0.996 | 16.2 | 0.713 |
| completr, 93 segments | 500 | 0.622 | 0.012 | 0.036 | 0.044 | 0.040 | 0.078 | 0.090 | 0.086 | 0.182 | 0.220 | 0.908 | 23.1 | 0.996 | 16.2 | 0.713 |
| tantivy | 500 | 0.480 | 0.008 | 0.022 | 0.032 | 0.020 | 0.044 | 0.052 | 0.042 | 0.084 | 0.114 | 0.780 | 26.5 | 0.886 | 20.1 | 0.583 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 4,000,000 | ok | 30.19 s | 316 MB | 400 MB | 12 MB | 2.41 / 26.23 |
| completr | 16,000,000 | ok | 136.46 s | 377 MB | 1363 MB | 29 MB | 4.87 / 81.36 |
| tantivy | 4,000,000 | ok | 20.76 s | 338 MB | 396 MB | 1 MB | 23.32 / 132.94 |
| tantivy | 16,000,000 | ok | 92.23 s | 336 MB | 1512 MB | 4 MB | 68.86 / 378.59 |
