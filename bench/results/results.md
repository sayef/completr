Measured 2026-10-01 on Apple M1 Pro, 10 cores, 17180 MB RAM, macOS 26.7, Python 3.13.7.

## Indexing, size, memory

| engine | version | index time | on disk | memory after open or index | memory after queries | open / restart to first hit |
| --- | --- | --- | --- | --- | --- | --- |
| completr | completr 0.1.0 | 728.7 ms | 23 MB | 4 MB | 31 MB | 1.2 ms |
| tantivy | tantivy-py 0.26.2 | 1.02 s | 10 MB | 3 MB | 18 MB | 0.6 ms |
| typesense | typesense 30.2 | 3.74 s | 39 MB | 283 MB | 191 MB | 3.37 s |
| typesense-buckets | typesense 30.2 | 3.66 s | 39 MB | 268 MB | 157 MB | 3.38 s |
| meilisearch | meilisearch 1.54.2 | 2.08 s | 136 MB | 811 MB | 136 MB | 222.8 ms |
| meilisearch-popfirst | meilisearch 1.54.2 | 1.93 s | 136 MB | 877 MB | 145 MB | 219.9 ms |

completr with build_threads=8: 295.8 ms.

In-process engines: RSS growth of a fresh process after opening, then after 5,000 prefix queries. Servers: RSS of the server process after indexing, then after all queries.

## Latency, limit 10, single client (ms)

| set | engine | in-process or round-trip p50 | p90 | p99 | mean | engine-reported p50 | p99 | mean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| prefix (23292) | completr | 0.171 | 0.486 | 0.881 | 0.226 | - | - | - |
| prefix (23292) | tantivy | 0.434 | 1.428 | 2.752 | 0.622 | - | - | - |
| prefix (23292) | typesense | 1.470 | 9.159 | 41.659 | 4.114 | 0 | 40 | 2.96 |
| prefix (23292) | typesense-buckets | 1.560 | 9.595 | 41.934 | 4.266 | 0 | 41 | 3.04 |
| prefix (23292) | meilisearch | 1.894 | 5.971 | 10.000 | 2.814 | 1 | 5 | 0.88 |
| prefix (23292) | meilisearch-popfirst | 1.591 | 2.299 | 3.011 | 1.666 | 0 | 2 | 0.45 |
| typo1 (1000) | completr | 0.161 | 0.510 | 0.854 | 0.235 | - | - | - |
| typo1 (1000) | tantivy | 0.223 | 0.700 | 2.138 | 0.399 | - | - | - |
| typo1 (1000) | typesense | 1.086 | 2.204 | 8.753 | 1.580 | 0 | 8 | 0.52 |
| typo1 (1000) | typesense-buckets | 1.186 | 2.376 | 9.034 | 1.686 | 0 | 8 | 0.54 |
| typo1 (1000) | meilisearch | 1.347 | 1.874 | 4.326 | 1.448 | 0 | 2 | 0.17 |
| typo1 (1000) | meilisearch-popfirst | 1.286 | 1.681 | 2.357 | 1.321 | 0 | 1 | 0.13 |
| typo2 (1000) | completr | 0.136 | 0.561 | 0.895 | 0.231 | - | - | - |
| typo2 (1000) | tantivy | 0.507 | 0.728 | 1.558 | 0.487 | - | - | - |
| typo2 (1000) | typesense | 1.303 | 2.447 | 8.821 | 1.742 | 0 | 7 | 0.60 |
| typo2 (1000) | typesense-buckets | 1.376 | 2.626 | 8.916 | 1.838 | 0 | 7 | 0.63 |
| typo2 (1000) | meilisearch | 1.419 | 1.939 | 2.609 | 1.618 | 0 | 1 | 0.18 |
| typo2 (1000) | meilisearch-popfirst | 1.275 | 1.589 | 2.053 | 1.288 | 0 | 1 | 0.09 |
| multiword (2000) | completr | 0.126 | 0.291 | 0.463 | 0.151 | - | - | - |
| multiword (2000) | tantivy | 0.240 | 0.751 | 1.771 | 0.391 | - | - | - |
| multiword (2000) | typesense | 0.954 | 2.881 | 13.552 | 1.795 | 0 | 12 | 0.80 |
| multiword (2000) | typesense-buckets | 1.006 | 3.026 | 13.430 | 1.864 | 0 | 12 | 0.82 |
| multiword (2000) | meilisearch | 1.441 | 1.890 | 2.586 | 1.498 | 0 | 1 | 0.23 |
| multiword (2000) | meilisearch-popfirst | 1.435 | 1.841 | 2.469 | 1.479 | 0 | 1 | 0.25 |

## Throughput, 8 clients, prefix set

| engine | QPS | clients |
| --- | --- | --- |
| completr | 30096 | threads |
| tantivy | 5718 | threads |
| typesense | 1727 | processes |
| typesense-buckets | 1640 | processes |
| meilisearch | 3955 | processes |
| meilisearch-popfirst | 3757 | processes |

## Quality: typing the target title one character at a time


### popular targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 500 | 0.861 | 0.204 | 0.434 | 0.530 | 0.413 | 0.681 | 0.764 | 0.663 | 0.869 | 0.902 | 1.000 | 7.4 | 1.000 | 4.8 | 0.874 |
| tantivy | 500 | 0.804 | 0.096 | 0.210 | 0.294 | 0.230 | 0.437 | 0.523 | 0.460 | 0.683 | 0.777 | 0.998 | 9.9 | 1.000 | 7.0 | 0.816 |
| typesense | 500 | 0.784 | 0.066 | 0.178 | 0.258 | 0.192 | 0.375 | 0.461 | 0.456 | 0.667 | 0.737 | 0.998 | 9.7 | 0.998 | 6.9 | 0.814 |
| typesense-buckets | 500 | 0.787 | 0.084 | 0.192 | 0.268 | 0.202 | 0.387 | 0.469 | 0.462 | 0.671 | 0.737 | 0.996 | 9.6 | 0.998 | 6.8 | 0.815 |
| meilisearch | 500 | 0.846 | 0.152 | 0.386 | 0.508 | 0.383 | 0.627 | 0.719 | 0.653 | 0.849 | 0.896 | 0.998 | 7.7 | 0.998 | 5.2 | 0.860 |
| meilisearch-popfirst | 500 | 0.789 | 0.086 | 0.206 | 0.292 | 0.210 | 0.411 | 0.517 | 0.418 | 0.673 | 0.769 | 0.990 | 10.3 | 0.998 | 7.1 | 0.808 |

### popular targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 488 | 0.827 | 0.172 | 0.346 | 0.422 | 0.367 | 0.592 | 0.668 | 0.590 | 0.803 | 0.848 | 0.996 | 7.8 | 0.996 | 5.2 | 0.863 |
| tantivy | 488 | 0.734 | 0.082 | 0.176 | 0.242 | 0.197 | 0.385 | 0.467 | 0.352 | 0.572 | 0.672 | 0.959 | 10.3 | 0.969 | 7.3 | 0.787 |
| typesense | 488 | 0.772 | 0.061 | 0.152 | 0.219 | 0.158 | 0.305 | 0.381 | 0.393 | 0.594 | 0.662 | 0.998 | 10.2 | 0.998 | 7.3 | 0.807 |
| typesense-buckets | 488 | 0.774 | 0.076 | 0.160 | 0.225 | 0.168 | 0.318 | 0.385 | 0.402 | 0.600 | 0.664 | 0.996 | 10.0 | 0.998 | 7.3 | 0.807 |
| meilisearch | 488 | 0.804 | 0.131 | 0.309 | 0.395 | 0.348 | 0.590 | 0.682 | 0.590 | 0.785 | 0.838 | 0.969 | 7.9 | 0.977 | 5.5 | 0.838 |
| meilisearch-popfirst | 488 | 0.746 | 0.074 | 0.172 | 0.242 | 0.182 | 0.367 | 0.465 | 0.355 | 0.588 | 0.693 | 0.953 | 10.5 | 0.969 | 7.4 | 0.783 |

### uniform targets, clean

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 300 | 0.804 | 0.027 | 0.127 | 0.203 | 0.184 | 0.338 | 0.418 | 0.465 | 0.656 | 0.722 | 0.997 | 9.9 | 1.000 | 7.5 | 0.813 |
| tantivy | 300 | 0.737 | 0.007 | 0.023 | 0.040 | 0.077 | 0.137 | 0.191 | 0.234 | 0.378 | 0.468 | 0.987 | 12.8 | 0.997 | 10.2 | 0.747 |
| typesense | 300 | 0.755 | 0.017 | 0.023 | 0.037 | 0.084 | 0.157 | 0.197 | 0.278 | 0.455 | 0.522 | 1.000 | 11.6 | 1.000 | 9.5 | 0.761 |
| typesense-buckets | 300 | 0.754 | 0.017 | 0.020 | 0.033 | 0.087 | 0.157 | 0.197 | 0.268 | 0.445 | 0.512 | 1.000 | 11.7 | 1.000 | 9.6 | 0.759 |
| meilisearch | 300 | 0.798 | 0.030 | 0.107 | 0.180 | 0.181 | 0.331 | 0.428 | 0.438 | 0.652 | 0.719 | 1.000 | 10.1 | 1.000 | 7.6 | 0.809 |
| meilisearch-popfirst | 300 | 0.731 | 0.013 | 0.023 | 0.047 | 0.070 | 0.134 | 0.194 | 0.214 | 0.378 | 0.472 | 0.983 | 13.0 | 0.997 | 10.2 | 0.745 |

### uniform targets, typo

| engine | n | mrr_over_prefixes | s@1_len3 | s@5_len3 | s@10_len3 | s@1_len5 | s@5_len5 | s@10_len5 | s@1_len8 | s@5_len8 | s@10_len8 | reached_top1 | keystrokes_top1 | reached_top5 | keystrokes_top5 | chars_saved_top5 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completr | 298 | 0.766 | 0.023 | 0.104 | 0.174 | 0.138 | 0.272 | 0.352 | 0.342 | 0.534 | 0.607 | 0.983 | 10.8 | 0.990 | 8.2 | 0.795 |
| tantivy | 298 | 0.683 | 0.003 | 0.017 | 0.034 | 0.070 | 0.124 | 0.164 | 0.195 | 0.319 | 0.389 | 0.943 | 13.1 | 0.953 | 10.4 | 0.717 |
| typesense | 298 | 0.744 | 0.013 | 0.020 | 0.030 | 0.077 | 0.138 | 0.168 | 0.208 | 0.369 | 0.426 | 0.997 | 12.1 | 0.997 | 10.0 | 0.755 |
| typesense-buckets | 298 | 0.743 | 0.013 | 0.017 | 0.027 | 0.081 | 0.138 | 0.168 | 0.201 | 0.362 | 0.409 | 0.997 | 12.2 | 0.997 | 10.1 | 0.753 |
| meilisearch | 298 | 0.762 | 0.027 | 0.084 | 0.144 | 0.144 | 0.295 | 0.389 | 0.369 | 0.570 | 0.651 | 0.950 | 10.2 | 0.966 | 7.9 | 0.785 |
| meilisearch-popfirst | 298 | 0.699 | 0.010 | 0.017 | 0.040 | 0.064 | 0.121 | 0.168 | 0.185 | 0.336 | 0.403 | 0.936 | 13.1 | 0.953 | 10.4 | 0.718 |
