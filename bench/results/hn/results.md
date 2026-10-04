Measured 2026-10-04 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | peak memory while indexing | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | completr 0.1.0 | 292.0 ms | 59 MB | 11 MB | 4 MB | 10 MB | 13.2 ms |
| tantivy | tantivy-py 0.26.2 | 1.13 s | 90 MB | 10 MB | 1 MB | 5 MB | 4.5 ms |
| typesense | typesense 30.2 | 3.84 s | 265 MB | 39 MB | 237 MB | 243 MB | 3.33 s |
| typesense-buckets | typesense 30.2 | 3.82 s | 234 MB | 40 MB | 241 MB | 245 MB | 3.33 s |
| meilisearch | meilisearch 1.54.2 | 2.11 s | 1000 MB | 136 MB | 326 MB | 220 MB | 232.3 ms |
| meilisearch-popfirst | meilisearch 1.54.2 | 2.07 s | 983 MB | 136 MB | 316 MB | 219 MB | 231.9 ms |

completr with build_threads=8: 289.5 ms.
completr with UUID string ids: 13 MB on disk, 660.7 ms to index.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries; peak memory while indexing is the build process's peak RSS above the loaded documents. Servers: RSS of the server process after indexing, then after all queries; peak memory is the server's peak RSS while indexing.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (23292) | completr, compacted | 0.149 | 0.380 | 0.621 | 0.182 | - | - | - |
| prefix (23292) | tantivy | 0.423 | 1.404 | 2.719 | 0.612 | - | - | - |
| prefix (23292) | typesense | 1.355 | 8.913 | 40.928 | 3.964 | 0 | 40 | 2.87 |
| prefix (23292) | typesense-buckets | 1.402 | 8.956 | 40.425 | 3.994 | 0 | 39 | 2.88 |
| prefix (23292) | meilisearch | 1.386 | 1.916 | 2.421 | 1.421 | 0 | 1 | 0.29 |
| prefix (23292) | meilisearch-popfirst | 1.481 | 2.047 | 2.620 | 1.524 | 0 | 1 | 0.39 |
| typo1 (1000) | completr, compacted | 0.175 | 0.439 | 0.857 | 0.227 | - | - | - |
| typo1 (1000) | tantivy | 0.218 | 0.682 | 2.123 | 0.392 | - | - | - |
| typo1 (1000) | typesense | 1.085 | 2.171 | 8.738 | 1.570 | 0 | 8 | 0.49 |
| typo1 (1000) | typesense-buckets | 1.115 | 2.198 | 8.738 | 1.611 | 0 | 8 | 0.51 |
| typo1 (1000) | meilisearch | 1.158 | 1.481 | 1.880 | 1.180 | 0 | 1 | 0.05 |
| typo1 (1000) | meilisearch-popfirst | 1.268 | 1.605 | 2.072 | 1.280 | 0 | 1 | 0.10 |
| typo2 (1000) | completr, compacted | 0.159 | 0.538 | 1.028 | 0.236 | - | - | - |
| typo2 (1000) | tantivy | 0.506 | 0.715 | 1.472 | 0.482 | - | - | - |
| typo2 (1000) | typesense | 1.251 | 2.382 | 8.258 | 1.714 | 0 | 7 | 0.58 |
| typo2 (1000) | typesense-buckets | 1.291 | 2.466 | 8.275 | 1.749 | 0 | 7 | 0.58 |
| typo2 (1000) | meilisearch | 1.197 | 1.494 | 1.836 | 1.210 | 0 | 1 | 0.06 |
| typo2 (1000) | meilisearch-popfirst | 1.297 | 1.629 | 2.305 | 1.314 | 0 | 1 | 0.10 |
| multiword (2000) | completr, compacted | 0.117 | 0.239 | 0.433 | 0.135 | - | - | - |
| multiword (2000) | tantivy | 0.230 | 0.726 | 1.747 | 0.380 | - | - | - |
| multiword (2000) | typesense | 0.951 | 2.851 | 14.521 | 1.791 | 0 | 13 | 0.79 |
| multiword (2000) | typesense-buckets | 0.974 | 2.900 | 13.338 | 1.819 | 0 | 12 | 0.80 |
| multiword (2000) | meilisearch | 1.326 | 1.667 | 2.172 | 1.358 | 0 | 1 | 0.14 |
| multiword (2000) | meilisearch-popfirst | 1.425 | 1.821 | 2.464 | 1.465 | 0 | 1 | 0.24 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr, compacted | 40016 | threads |
| tantivy | 5541 | threads |
| typesense | 1768 | processes |
| typesense-buckets | 1679 | processes |
| meilisearch | 4024 | processes |
| meilisearch-popfirst | 3826 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.862 | 0.204 | 0.434 | 0.530 | 0.411 | 0.681 | 0.764 | 0.665 | 0.876 | 0.910 | 1.000 | 7.4 | 1.000 | 4.9 | 0.873 |
| tantivy | 500 | 0.804 | 0.094 | 0.208 | 0.294 | 0.230 | 0.437 | 0.525 | 0.460 | 0.683 | 0.777 | 0.998 | 9.9 | 1.000 | 7.0 | 0.816 |
| typesense | 500 | 0.784 | 0.066 | 0.178 | 0.256 | 0.192 | 0.375 | 0.461 | 0.456 | 0.667 | 0.737 | 0.998 | 9.7 | 0.998 | 6.9 | 0.814 |
| typesense-buckets | 500 | 0.787 | 0.084 | 0.190 | 0.266 | 0.202 | 0.387 | 0.469 | 0.462 | 0.671 | 0.737 | 0.996 | 9.6 | 0.998 | 6.8 | 0.815 |
| meilisearch | 500 | 0.846 | 0.152 | 0.386 | 0.508 | 0.383 | 0.627 | 0.719 | 0.653 | 0.849 | 0.896 | 0.998 | 7.7 | 0.998 | 5.2 | 0.860 |
| meilisearch-popfirst | 500 | 0.789 | 0.086 | 0.206 | 0.292 | 0.210 | 0.411 | 0.517 | 0.418 | 0.673 | 0.769 | 0.990 | 10.3 | 0.998 | 7.1 | 0.808 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 488 | 0.843 | 0.174 | 0.350 | 0.428 | 0.375 | 0.611 | 0.695 | 0.596 | 0.818 | 0.863 | 1.000 | 7.8 | 1.000 | 5.3 | 0.865 |
| tantivy | 488 | 0.734 | 0.080 | 0.174 | 0.242 | 0.197 | 0.385 | 0.469 | 0.352 | 0.572 | 0.672 | 0.959 | 10.3 | 0.969 | 7.3 | 0.787 |
| typesense | 488 | 0.772 | 0.061 | 0.152 | 0.217 | 0.158 | 0.305 | 0.381 | 0.393 | 0.594 | 0.662 | 0.998 | 10.2 | 0.998 | 7.3 | 0.807 |
| typesense-buckets | 488 | 0.774 | 0.076 | 0.158 | 0.223 | 0.168 | 0.318 | 0.385 | 0.402 | 0.600 | 0.664 | 0.996 | 10.0 | 0.998 | 7.3 | 0.807 |
| meilisearch | 488 | 0.804 | 0.131 | 0.309 | 0.395 | 0.348 | 0.590 | 0.682 | 0.590 | 0.785 | 0.838 | 0.969 | 7.9 | 0.977 | 5.5 | 0.838 |
| meilisearch-popfirst | 488 | 0.746 | 0.074 | 0.172 | 0.242 | 0.182 | 0.367 | 0.465 | 0.355 | 0.588 | 0.693 | 0.953 | 10.5 | 0.969 | 7.4 | 0.783 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 300 | 0.804 | 0.027 | 0.127 | 0.203 | 0.184 | 0.338 | 0.418 | 0.465 | 0.652 | 0.719 | 0.997 | 9.9 | 1.000 | 7.5 | 0.813 |
| tantivy | 300 | 0.737 | 0.007 | 0.020 | 0.043 | 0.077 | 0.137 | 0.197 | 0.231 | 0.381 | 0.472 | 0.987 | 12.8 | 0.997 | 10.1 | 0.748 |
| typesense | 300 | 0.755 | 0.017 | 0.023 | 0.037 | 0.084 | 0.157 | 0.197 | 0.278 | 0.455 | 0.522 | 1.000 | 11.6 | 1.000 | 9.5 | 0.761 |
| typesense-buckets | 300 | 0.754 | 0.017 | 0.020 | 0.033 | 0.087 | 0.157 | 0.197 | 0.268 | 0.445 | 0.512 | 1.000 | 11.7 | 1.000 | 9.6 | 0.759 |
| meilisearch | 300 | 0.798 | 0.030 | 0.107 | 0.180 | 0.181 | 0.331 | 0.428 | 0.438 | 0.652 | 0.719 | 1.000 | 10.1 | 1.000 | 7.6 | 0.809 |
| meilisearch-popfirst | 300 | 0.731 | 0.013 | 0.023 | 0.047 | 0.070 | 0.134 | 0.194 | 0.214 | 0.378 | 0.472 | 0.983 | 13.0 | 0.997 | 10.2 | 0.745 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 298 | 0.780 | 0.023 | 0.104 | 0.171 | 0.138 | 0.272 | 0.349 | 0.349 | 0.550 | 0.617 | 0.993 | 10.7 | 0.997 | 8.2 | 0.800 |
| tantivy | 298 | 0.683 | 0.003 | 0.017 | 0.037 | 0.070 | 0.124 | 0.168 | 0.188 | 0.315 | 0.393 | 0.943 | 13.2 | 0.953 | 10.4 | 0.717 |
| typesense | 298 | 0.744 | 0.013 | 0.020 | 0.030 | 0.077 | 0.138 | 0.168 | 0.208 | 0.369 | 0.426 | 0.997 | 12.1 | 0.997 | 10.0 | 0.755 |
| typesense-buckets | 298 | 0.743 | 0.013 | 0.017 | 0.027 | 0.081 | 0.138 | 0.168 | 0.201 | 0.362 | 0.409 | 0.997 | 12.2 | 0.997 | 10.1 | 0.753 |
| meilisearch | 298 | 0.762 | 0.027 | 0.084 | 0.144 | 0.144 | 0.295 | 0.389 | 0.369 | 0.570 | 0.651 | 0.950 | 10.2 | 0.966 | 7.9 | 0.785 |
| meilisearch-popfirst | 298 | 0.699 | 0.010 | 0.017 | 0.040 | 0.064 | 0.121 | 0.168 | 0.185 | 0.336 | 0.403 | 0.936 | 13.1 | 0.953 | 10.4 | 0.718 |

### misspelled targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.872 | 0.156 | 0.394 | 0.482 | 0.374 | 0.610 | 0.700 | 0.630 | 0.814 | 0.870 | 1.000 | 7.7 | 1.000 | 5.4 | 0.880 |
| tantivy | 500 | 0.817 | 0.060 | 0.176 | 0.258 | 0.172 | 0.358 | 0.446 | 0.394 | 0.626 | 0.712 | 0.998 | 10.5 | 1.000 | 7.4 | 0.834 |
| typesense | 500 | 0.807 | 0.048 | 0.146 | 0.220 | 0.168 | 0.350 | 0.438 | 0.396 | 0.594 | 0.666 | 0.996 | 9.8 | 1.000 | 7.3 | 0.832 |
| typesense-buckets | 500 | 0.809 | 0.054 | 0.166 | 0.230 | 0.172 | 0.358 | 0.444 | 0.408 | 0.600 | 0.670 | 0.996 | 9.8 | 1.000 | 7.3 | 0.834 |
| meilisearch | 500 | 0.811 | 0.058 | 0.172 | 0.252 | 0.160 | 0.358 | 0.454 | 0.364 | 0.638 | 0.734 | 0.994 | 10.5 | 1.000 | 7.5 | 0.829 |
| meilisearch-popfirst | 500 | 0.811 | 0.058 | 0.172 | 0.252 | 0.160 | 0.358 | 0.454 | 0.364 | 0.638 | 0.734 | 0.994 | 10.5 | 1.000 | 7.5 | 0.829 |

### misspelled targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr, compacted | 500 | 0.854 | 0.146 | 0.366 | 0.448 | 0.352 | 0.588 | 0.668 | 0.610 | 0.790 | 0.844 | 1.000 | 7.9 | 1.000 | 5.5 | 0.877 |
| tantivy | 500 | 0.609 | 0.058 | 0.164 | 0.238 | 0.164 | 0.338 | 0.424 | 0.354 | 0.570 | 0.642 | 0.844 | 10.1 | 0.866 | 7.0 | 0.729 |
| typesense | 500 | 0.789 | 0.046 | 0.138 | 0.202 | 0.160 | 0.326 | 0.416 | 0.384 | 0.562 | 0.626 | 0.990 | 10.1 | 0.996 | 7.6 | 0.821 |
| typesense-buckets | 500 | 0.787 | 0.052 | 0.156 | 0.212 | 0.164 | 0.334 | 0.422 | 0.390 | 0.568 | 0.630 | 0.984 | 10.1 | 0.996 | 7.6 | 0.821 |
| meilisearch | 500 | 0.688 | 0.056 | 0.160 | 0.232 | 0.152 | 0.340 | 0.430 | 0.328 | 0.582 | 0.672 | 0.838 | 10.1 | 0.862 | 7.0 | 0.724 |
| meilisearch-popfirst | 500 | 0.688 | 0.056 | 0.160 | 0.232 | 0.152 | 0.340 | 0.430 | 0.328 | 0.582 | 0.672 | 0.838 | 10.1 | 0.862 | 7.0 | 0.724 |

## Scale: nested subsets of growing size

| engine | documents | status | index time | peak memory while indexing | on disk | memory after open or index | warm prefix p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 25,000 | ok | 78.8 ms | 27 MB | 3 MB | 3 MB | 0.20 / 1.02 |
| completr | 50,000 | ok | 132.8 ms | 46 MB | 5 MB | 3 MB | 0.24 / 1.44 |
| completr | 124,440 | ok | 297.1 ms | 59 MB | 11 MB | 4 MB | 0.17 / 0.87 |
| tantivy | 25,000 | ok | 616.8 ms | 69 MB | 2 MB | 1 MB | 0.22 / 1.17 |
| tantivy | 50,000 | ok | 725.8 ms | 77 MB | 4 MB | 1 MB | 0.27 / 1.62 |
| tantivy | 124,440 | ok | 1.03 s | 90 MB | 10 MB | 1 MB | 0.36 / 2.64 |
| typesense | 25,000 | ok | 761.1 ms | 128 MB | 7 MB | 121 MB | 3.76 / 35.39 |
| typesense | 50,000 | ok | 1.55 s | 168 MB | 15 MB | 168 MB | 4.83 / 46.40 |
| typesense | 124,440 | ok | 3.91 s | 241 MB | 39 MB | 228 MB | 1.44 / 39.95 |
| meilisearch | 25,000 | ok | 557.3 ms | 695 MB | 29 MB | 695 MB | 0.99 / 1.70 |
| meilisearch | 50,000 | ok | 940.6 ms | 942 MB | 54 MB | 828 MB | 1.07 / 1.87 |
| meilisearch | 124,440 | ok | 2.06 s | 987 MB | 136 MB | 331 MB | 1.45 / 2.41 |
