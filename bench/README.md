# Competitor benchmark

This harness compares strato with [Typesense](https://typesense.org), [Meilisearch](https://www.meilisearch.com)
and [tantivy](https://github.com/quickwit-oss/tantivy) (through tantivy-py) as autocompletion engines. Every
engine indexes the same documents and answers the same queries. The latest results are in
[`docs/benchmarks.md`](../docs/benchmarks.md) and [`results/results.md`](results/results.md).

## What is measured, and why

Autocompletion runs one query per keystroke, so the benchmark measures both speed on the queries a user
really sends and whether the wanted suggestion appears early.

| Measurement | How |
|---|---|
| Indexing | Wall time to build the index from all documents; strato also with `build_threads=8` |
| Size | Bytes on disk of the index (strato: the segment file; others: their data directory) |
| Memory | In-process engines: RSS growth of a fresh process after opening the index, then after 5,000 prefix queries. Servers: RSS of the server process after indexing and after all queries |
| Open or restart | strato and tantivy: time to open the index. Servers: time from a restart to the first hit |
| Latency | One client, limit 10, after a warm-up. In-process time for strato and tantivy; round-trip time over localhost HTTP and the engine-reported time (`search_time_ms`, `processingTimeMs`) for the servers |
| Throughput | 8 closed-loop clients for 10 s on the prefix set: threads for in-process engines, processes for servers |
| Quality | Each target title is typed one character at a time, and the target's rank in the top 10 is recorded after every keystroke |

The latency sets, drawn from the corpus with a fixed seed:

| Set | Queries | Content |
|---|---|---|
| `prefix` | 23,292 | Every prefix of 500 uniformly drawn titles, as typed |
| `typo1` | 1,000 | The first 1 to 3 words of a title with one edit in a word of 5 or more letters |
| `typo2` | 1,000 | The same with two edits: in two words, or twice in one word of 9 or more letters |
| `multiword` | 2,000 | 2 to 4 consecutive words from a title |

Edits are substitutions, deletions, insertions or transpositions, never on the first character.

The quality targets are 500 titles drawn in proportion to their points (`popular`, what users most often
look for) and 300 drawn uniformly (`uniform`, the long tail). Each is typed as written (`clean`) and with
one edit in its first word of 5 or more letters (`typo`, 488 and 298 targets have such a word). The
metrics are:

- `mrr_over_prefixes`: the reciprocal rank of the target, averaged over all prefixes of the title, then
  over targets;
- `s@k_lenL`: the share of targets in the top k after L characters;
- `reached_topk` and `keystrokes_topk`: the share of targets that reach the top k before the title is fully
  typed, and the mean number of keystrokes they need;
- `chars_saved_top5`: the share of characters a user saves by picking the target once it is in the top 5.

## Dataset

The [Hacker News dataset](https://github.com/meilisearch/meilisearch/blob/main/workloads/hackernews.json) that Meilisearch
uses in its own benchmarks: 1,000,000 items from the official HN API, in ten NDJSON files on Meilisearch's
public bucket (`milli-benchmarks.fra1.digitaloceanspaces.com`). `fetch.py` downloads them, checks each
file's SHA-256 against Meilisearch's workload file, and keeps live stories with a title and a score: 133,561 stories. Titles that are equal
ignoring case are deduplicated, keeping the copy with the most points, which leaves **124,440 documents**.
Points are the popularity signal for every engine.

**Licence: not available.** The data has no explicit licence, so it is not redistributed here: the
harness downloads it at run time, for measurement only.
`samples.json` holds the generated queries and targets (titles and ids), so a run can be checked against
the committed sample with `python bench.py samples --check`.

## Engine settings

Each competitor uses the settings its documentation recommends for ranking by popularity, and nothing
else is tuned. All engines return 10 results.

| Engine | Version | Settings |
|---|---|---|
| strato | the installed package | One segment built with default options; `Index` with default options. Popularity is `log1p(points) / log1p(max points)` |
| tantivy | tantivy-py 0.26.2 | See below |
| Typesense | 30.2 | `title` string and `score` int32 with `default_sorting_field: score`; search with `query_by=title` and the defaults for prefix search and typo tolerance |
| Typesense, `buckets` | 30.2 | As above, plus `sort_by=_text_match(buckets: 10):desc,score:desc`, which gives points more weight |
| Meilisearch | 1.54.2 | `searchableAttributes: [title]`; the default ranking rules followed by the custom rule `score:desc` |
| Meilisearch, `popfirst` | 1.54.2 | Ranking rules `words, typo, score:desc, proximity, attributeRank, sort, wordPosition, exactness` |

Typesense and Meilisearch run as local servers on 127.0.0.1 with their default configuration, from the
official release binaries. Meilisearch checksums are the GitHub release asset digests; Typesense publishes
none, so `fetch.py` pins the SHA-256 of the archives as downloaded on 2026-09-30. strato and tantivy run in the benchmark process.

**tantivy autocomplete emulation.** tantivy is a search library without an autocomplete mode, so the adapter
builds one: the `title` field uses the default tokenizer, every complete token must match as a term and
the last token, unless followed by a space, as a prefix. Results are weighted by the `score` fast field. When
fewer than 10 documents match, a second pass adds fuzzy matches with Meilisearch's typo thresholds: 1 edit
for tokens of 5 to 8 characters and 2 edits from 9 characters, with transpositions costing one edit.

## Running

Requirements: Python 3.11 or later, about 1 GB of free disk for the Meilisearch and Typesense binaries,
and macOS (arm64 or x86_64) or Linux (x86_64 or arm64).

```sh
python -m venv .venv
.venv/bin/pip install -r bench/requirements.txt   # or install a local strato wheel instead of the PyPI one
PYTHON=.venv/bin/python bench/run.sh
```

`run.sh` downloads the dataset and binaries into `bench/.cache/` (ignored by git), checks the sample,
runs every engine, writes `results/<engine>.json` and renders `results/results.md`. A full run takes about
30 minutes on an M1 Pro, mostly for the quality sets against the servers. Single steps:

```sh
cd bench
python fetch.py                                       # dataset and binaries
python bench.py run strato --throughput               # one engine
python bench.py run typesense --variant buckets       # a variant
python report.py
```

Run it in the foreground on an idle machine: background jobs get a lower scheduling priority, which skews
timings. Servers are stopped at the end of each run; the index data stays in `bench/.cache/work/`.

## Caveats

- **In-process against network.** strato and tantivy run in the benchmark process, whereas Typesense and
  Meilisearch answer over HTTP on localhost. Round trips add about 1 ms, which favours the in-process
  engines. The engine-reported times of the servers exclude it, but Typesense and Meilisearch report whole
  milliseconds only.
- **Throughput client models differ.** In-process engines are driven by threads in one process; servers by
  8 client processes, whose own CPU use competes with the server on the same machine.
- **One machine, one run.** Timings vary between runs and machines by tens of percent; compare engines
  within one run rather than across runs.
- **One configuration each.** Engines are run with recommended settings, not tuned for this dataset or
  for these metrics. Other settings, such as different typo thresholds, change the quality results.
- **Titles only.** The corpus is short English titles with a single popularity signal. Longer texts, other
  languages, filters and aliases are not measured.
- **Synthetic queries.** Typos are random edits, not real user misspellings, and targets are typed from
  their first character. The quality metrics reward the target's rank only; other good suggestions count
  for nothing.
- **Memory measurement.** In-process figures are RSS growth, which counts the touched pages of strato's
  memory-mapped segment. Server figures are the whole server process.
- **tantivy is emulated.** Its results reflect the adapter described above as much as tantivy itself.
