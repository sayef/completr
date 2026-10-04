Measured 2026-10-04 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | completr 0.1.0 | 25.60 s | 286 MB | 338 MB | 24 MB | 68 MB | 30.1 ms |
| completr, 9 segments | completr 0.1.0 | 14.36 s | 250 MB | 430 MB | 44 MB | 50 MB | 117.2 ms |
| tantivy | tantivy-py 0.26.2 | 29.89 s | 347 MB | 357 MB | 2 MB | 25 MB | 43.8 ms |
| typesense | typesense 30.2 | 200.42 s | 1010 MB | 1224 MB | 1017 MB | 908 MB | 9.69 s |
| meilisearch | meilisearch 1.54.2 | 50.31 s | 3450 MB | 4934 MB | 2003 MB | 1966 MB | 234.4 ms |

completr with build_threads=8: 18.58 s.
completr with UUID string ids: 485 MB on disk, 51.15 s to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (10100) | completr, compacted | 0.286 | 1.955 | 5.650 | 0.750 | - | - | - |
| prefix (10100) | completr, 9 segments | 0.798 | 5.631 | 16.768 | 2.162 | - | - | - |
| prefix (10100) | tantivy | 4.360 | 36.853 | 98.369 | 12.787 | - | - | - |
| prefix (10100) | typesense | 3.619 | 40.922 | 757.188 | 35.643 | 2 | 756 | 34.40 |
| prefix (10100) | meilisearch | 4.371 | 8.134 | 16.059 | 5.137 | 3 | 15 | 3.93 |
| typo1 (1000) | completr, compacted | 1.013 | 4.890 | 8.106 | 1.894 | - | - | - |
| typo1 (1000) | completr, 9 segments | 3.269 | 15.925 | 25.823 | 5.996 | - | - | - |
| typo1 (1000) | tantivy | 4.569 | 16.903 | 30.691 | 8.484 | - | - | - |
| typo1 (1000) | typesense | 2.063 | 8.420 | 103.502 | 5.991 | 1 | 102 | 4.76 |
| typo1 (1000) | meilisearch | 3.770 | 5.644 | 7.970 | 3.988 | 3 | 7 | 2.83 |
| typo2 (1000) | completr, compacted | 0.868 | 6.110 | 9.965 | 1.942 | - | - | - |
| typo2 (1000) | completr, 9 segments | 2.883 | 20.648 | 32.586 | 6.423 | - | - | - |
| typo2 (1000) | tantivy | 12.208 | 18.260 | 29.159 | 11.301 | - | - | - |
| typo2 (1000) | typesense | 3.923 | 10.282 | 36.466 | 6.940 | 3 | 35 | 5.77 |
| typo2 (1000) | meilisearch | 3.864 | 5.625 | 8.327 | 4.045 | 3 | 7 | 2.90 |
| multiword (2000) | completr, compacted | 0.634 | 1.699 | 3.314 | 0.831 | - | - | - |
| multiword (2000) | completr, 9 segments | 1.963 | 4.868 | 7.929 | 2.420 | - | - | - |
| multiword (2000) | tantivy | 4.363 | 18.045 | 60.013 | 8.022 | - | - | - |
| multiword (2000) | typesense | 1.975 | 11.032 | 90.547 | 10.115 | 1 | 89 | 8.97 |
| multiword (2000) | meilisearch | 4.911 | 7.465 | 13.967 | 5.307 | 4 | 13 | 4.12 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr, compacted | 9536 | threads |
| completr, 9 segments | 2930 | threads |
| tantivy | 323 | threads |
| typesense | 192 | processes |
| meilisearch | 1082 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.595 | 0.046 | 0.138 | 0.230 | 0.259 | 0.463 | 0.559 | 0.597 | 0.785 | 0.842 | 1.000 | 8.3 | 1.000 | 6.3 | 0.597 |
| completr, 9 segments | 500 | 0.595 | 0.046 | 0.138 | 0.230 | 0.259 | 0.463 | 0.559 | 0.597 | 0.785 | 0.842 | 1.000 | 8.3 | 1.000 | 6.3 | 0.597 |
| tantivy | 500 | 0.567 | 0.058 | 0.142 | 0.196 | 0.232 | 0.401 | 0.477 | 0.532 | 0.703 | 0.768 | 0.968 | 8.9 | 0.998 | 7.0 | 0.563 |
| typesense | 500 | 0.477 | 0.002 | 0.024 | 0.060 | 0.088 | 0.277 | 0.337 | 0.414 | 0.603 | 0.650 | 0.998 | 9.8 | 0.998 | 7.9 | 0.490 |
| meilisearch | 500 | 0.541 | 0.002 | 0.040 | 0.086 | 0.140 | 0.403 | 0.483 | 0.511 | 0.741 | 0.797 | 1.000 | 8.9 | 1.000 | 6.8 | 0.555 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 463 | 0.527 | 0.035 | 0.102 | 0.168 | 0.167 | 0.297 | 0.380 | 0.448 | 0.625 | 0.683 | 0.965 | 9.0 | 0.976 | 6.9 | 0.560 |
| completr, 9 segments | 463 | 0.527 | 0.035 | 0.102 | 0.168 | 0.167 | 0.297 | 0.380 | 0.448 | 0.625 | 0.683 | 0.965 | 9.0 | 0.976 | 6.9 | 0.560 |
| tantivy | 463 | 0.463 | 0.041 | 0.102 | 0.149 | 0.139 | 0.256 | 0.328 | 0.370 | 0.536 | 0.593 | 0.875 | 9.6 | 0.931 | 7.6 | 0.506 |
| typesense | 463 | 0.364 | 0.002 | 0.019 | 0.048 | 0.046 | 0.150 | 0.182 | 0.255 | 0.421 | 0.471 | 0.860 | 11.1 | 0.886 | 9.1 | 0.387 |
| meilisearch | 463 | 0.479 | 0.002 | 0.037 | 0.067 | 0.108 | 0.280 | 0.347 | 0.400 | 0.630 | 0.697 | 0.896 | 9.3 | 0.935 | 7.3 | 0.508 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 300 | 0.488 | 0.003 | 0.010 | 0.013 | 0.097 | 0.171 | 0.215 | 0.292 | 0.504 | 0.585 | 0.987 | 11.7 | 1.000 | 9.4 | 0.484 |
| completr, 9 segments | 300 | 0.488 | 0.003 | 0.010 | 0.013 | 0.097 | 0.171 | 0.215 | 0.292 | 0.504 | 0.585 | 0.987 | 11.7 | 1.000 | 9.4 | 0.484 |
| tantivy | 300 | 0.444 | 0.000 | 0.003 | 0.007 | 0.050 | 0.117 | 0.171 | 0.246 | 0.433 | 0.489 | 0.913 | 12.6 | 0.983 | 10.1 | 0.448 |
| typesense | 300 | 0.440 | 0.000 | 0.003 | 0.003 | 0.044 | 0.107 | 0.161 | 0.239 | 0.408 | 0.454 | 1.000 | 12.6 | 1.000 | 10.4 | 0.433 |
| meilisearch | 300 | 0.480 | 0.003 | 0.007 | 0.013 | 0.077 | 0.164 | 0.215 | 0.278 | 0.486 | 0.570 | 0.997 | 11.6 | 1.000 | 9.5 | 0.477 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 275 | 0.436 | 0.000 | 0.000 | 0.000 | 0.044 | 0.076 | 0.098 | 0.170 | 0.375 | 0.458 | 0.935 | 12.8 | 0.975 | 10.3 | 0.450 |
| completr, 9 segments | 275 | 0.436 | 0.000 | 0.000 | 0.000 | 0.044 | 0.076 | 0.098 | 0.170 | 0.375 | 0.458 | 0.935 | 12.8 | 0.975 | 10.3 | 0.450 |
| tantivy | 275 | 0.385 | 0.000 | 0.000 | 0.000 | 0.025 | 0.055 | 0.069 | 0.133 | 0.292 | 0.356 | 0.847 | 13.4 | 0.909 | 11.0 | 0.406 |
| typesense | 275 | 0.336 | 0.000 | 0.000 | 0.000 | 0.018 | 0.047 | 0.062 | 0.110 | 0.242 | 0.284 | 0.862 | 14.6 | 0.880 | 12.1 | 0.344 |
| meilisearch | 275 | 0.430 | 0.000 | 0.000 | 0.004 | 0.033 | 0.065 | 0.095 | 0.182 | 0.364 | 0.447 | 0.895 | 12.5 | 0.927 | 10.3 | 0.443 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 1,000,000 | ok | 3.32 s | 206 MB | 61 MB | 7 MB | 0.24 / 3.77 |
| completr | 2,000,000 | ok | 6.75 s | 220 MB | 111 MB | 10 MB | 0.29 / 5.38 |
| completr | 4,000,000 | ok | 14.22 s | 252 MB | 202 MB | 16 MB | 0.43 / 8.95 |
| tantivy | 1,000,000 | ok | 5.72 s | 179 MB | 49 MB | 1 MB | 0.67 / 18.21 |
| tantivy | 2,000,000 | ok | 10.65 s | 285 MB | 93 MB | 1 MB | 0.99 / 26.79 |
| tantivy | 4,000,000 | ok | 17.08 s | 341 MB | 204 MB | 2 MB | 3.60 / 73.20 |
| typesense | 1,000,000 | ok | 25.26 s | 449 MB | 190 MB | 444 MB | 3.57 / 212.64 |
| typesense | 2,000,000 | ok | 51.18 s | 570 MB | 487 MB | 570 MB | 4.78 / 308.96 |
| typesense | 4,000,000 | ok | 108.09 s | 765 MB | 928 MB | 765 MB | 6.41 / 480.43 |
| meilisearch | 1,000,000 | ok | 7.82 s | 1986 MB | 739 MB | 1156 MB | 1.66 / 4.48 |
| meilisearch | 2,000,000 | ok | 13.40 s | 2180 MB | 1176 MB | 892 MB | 2.13 / 7.44 |
| meilisearch | 4,000,000 | ok | 26.41 s | 2776 MB | 2468 MB | 1517 MB | 3.02 / 9.81 |
