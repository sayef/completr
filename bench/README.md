# Competitor benchmark

This harness compares completr with [Typesense](https://typesense.org), [Meilisearch](https://www.meilisearch.com)
and [tantivy](https://github.com/quickwit-oss/tantivy) (through tantivy-py) as autocompletion engines. Every
engine indexes the same documents and answers the same queries. The latest results are in
[`docs/benchmarks.md`](../docs/benchmarks.md) and [`results/hn/results.md`](results/hn/results.md).

## What is measured, and why

Autocompletion runs one query per keystroke, so the benchmark measures both speed on the queries a user
really sends and whether the wanted suggestion appears early.

| Measurement | How |
|---|---|
| Indexing | Wall time to build the index from all documents. completr writes segments under a 256 MB budget and compacts them into one inside the timed build; it is also measured with `build_threads=8`, with UUID strings as ids, and on Wikipedia with its segments left uncompacted (`--variant segments`) |
| Indexing memory | Peak memory while indexing, as the kernel records it since a reset just before the build: for in-process engines, of the build process above the loaded documents; for servers, of the server process |
| Size | Bytes on disk of the index (completr: its segment files; others: their data directory) |
| Memory | The physical footprint the OS charges. In-process engines: growth of a fresh process after opening the index and its first query, then after 5,000 prefix queries. Servers: the server process after indexing and after all queries |
| Open or restart | completr and tantivy: time to open the index and answer the first query. Servers: time from a restart to the first hit |
| Latency | One client, limit 10, after a warm-up. In-process time for completr and tantivy; round-trip time over localhost HTTP and the engine-reported time (`search_time_ms`, `processingTimeMs`) for the servers |
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

### Wikipedia (`--workload wiki`)

Every English Wikipedia article that is not a redirect, with its views by users in August 2026 as its
popularity. `python fetch.py wiki` downloads the page table of the
[2026-09-01 dump](https://dumps.wikimedia.org/enwiki/20260901/) (2.4 GB, checked against Wikimedia's
published SHA-1) and the [August 2026 pageviews](https://dumps.wikimedia.org/other/pageview_complete/monthly/2026/2026-08/)
(5.0 GB; Wikimedia publishes no checksum, so `wiki.py` pins the SHA-256 as downloaded), keeps the
main-namespace pages that are not redirects, sums each title's views over access methods, and writes
`.cache/data/wiki_titles.jsonl`. Ids are page ids. Articles without views keep a popularity of zero.

**Licence.** Wikipedia article titles are © Wikipedia contributors under
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); the pageview counts are released under
[CC0](https://creativecommons.org/publicdomain/zero/1.0/) by the Wikimedia Foundation. The harness downloads
both at run time; `samples-wiki.json` holds a few thousand titles drawn from them, under the same licence.

### MusicBrainz (`--workload music`)

Every recording in the MusicBrainz database, titled "name – artist credit", with its MBID as its id, so
every engine stores and returns a UUID string. `python fetch.py music` downloads the core dump of
2026-09-30 and the ListenBrainz statistics dump of 2026-09-15 (checked against their published SHA-256
sums), sums each recording's listens over users' all-time top recordings as its popularity, and writes
`.cache/data/music_titles.jsonl`: 40,393,216 recordings, 4.9 million of them with listens. **Licence:**
both are CC0.

### AmazonQAC (`--workload qac`)

The distinct search terms of [AmazonQAC](https://huggingface.co/datasets/amazon/AmazonQAC) (a pinned
revision), each with its number of searches as its popularity: 39,588,974 terms. `python amazonqac.py`
reads only the two columns it needs over HTTP with DuckDB. Its test set's 20,000 prefixes, each with the
term the user then searched, are scored as the dataset's paper does: S@1, S@10 and MRR@10 of that term among
the 10 results. **Licence:** CDLA-Permissive-2.0.

### Open Library (`--workload books`)

Every Open Library work with a title, as "title – first author", with its reading-log entries and ratings
as its popularity. `python fetch.py books` downloads the dumps of 2026-09-30, checked against archive.org's
SHA-1 sums. **Licence:** CC0.

### Real misspellings

`python misspellings.py WORKLOAD` draws 500 titles in proportion to their popularity among those holding a
word from Wikipedia's list of common English misspellings (revision 1199637275, CC BY-SA 4.0), and types
each with that word misspelt as people do: `samples-WORKLOAD-misspelled.json`, measured as a third set of
quality targets.

## Engine settings

Each competitor uses the settings its documentation recommends for ranking by popularity, and nothing
else is tuned. All engines return 10 results.

| Engine | Version | Settings |
|---|---|---|
| completr | the installed package | One segment built with default options; `Index` with default options. Popularity is `log1p(points) / log1p(max points)` |
| tantivy | tantivy-py 0.26.2 | See below |
| Typesense | 30.2 | `title` string and `score` int32 with `default_sorting_field: score`; search with `query_by=title` and the defaults for prefix search and typo tolerance |
| Typesense, `buckets` | 30.2 | As above, plus `sort_by=_text_match(buckets: 10):desc,score:desc`, which gives points more weight |
| Meilisearch | 1.54.2 | `searchableAttributes: [title]`; the default ranking rules followed by the custom rule `score:desc` |
| Meilisearch, `popfirst` | 1.54.2 | Ranking rules `words, typo, score:desc, proximity, attributeRank, sort, wordPosition, exactness` |

Typesense and Meilisearch run as local servers on 127.0.0.1 with their default configuration, from the
official release binaries. Meilisearch checksums are the GitHub release asset digests; Typesense publishes
none, so `fetch.py` pins the SHA-256 of the archives as downloaded on 2026-09-30. completr and tantivy run in the benchmark process.

**tantivy autocomplete emulation.** tantivy is a search library without an autocomplete mode, so the adapter
builds one: the `title` field uses the default tokenizer, every complete token must match as a term and
the last token, unless followed by a space, as a prefix. The title is a stored field and is read with every
hit, since a suggestion needs its text; the other engines store their documents too. Results are weighted by the `score` fast field. When
fewer than 10 documents match, a second pass adds fuzzy matches with Meilisearch's typo thresholds: 1 edit
for tokens of 5 to 8 characters and 2 edits from 9 characters, with transpositions costing one edit.

## Running

Requirements: Python 3.11 or later, about 1 GB of free disk for the Meilisearch and Typesense binaries,
and macOS (arm64 or x86_64) or Linux (x86_64 or arm64).

```sh
python -m venv .venv
.venv/bin/pip install -r bench/requirements.txt   # or install a local completr wheel instead of the PyPI one
PYTHON=.venv/bin/python bench/run.sh
```

`run.sh` downloads the dataset and binaries into `bench/.cache/` (ignored by git), checks the sample,
runs every engine, writes `results/hn/<engine>.json` and renders `results/hn/results.md`. A full run takes about
30 minutes on an M1 Pro, mostly for the quality sets against the servers. Single steps:

```sh
cd bench
python fetch.py                                       # dataset and binaries
python bench.py run completr --throughput               # one engine
python bench.py run typesense --variant buckets       # a variant
python bench.py scale completr --sizes 25000,50000,124440   # nested subsets of growing size
python report.py
```

**Workloads.** `--workload NAME` (default `hn`) selects the corpus; each has its own samples file, index
data under `.cache/work/` and results under `results/NAME/`. `report.py --workload NAME` renders them.

**Scale runs.** `scale` indexes seeded, nested subsets of the corpus, so each size contains the previous
one. For every size it records index time, peak memory while indexing, size on disk, memory after opening
and warm prefix latency, then deletes that index data. Results go to `results/NAME/scale-<engine>.json`.

**Limits.** Every build runs under a time and a memory limit, `BENCH_TIME_LIMIT_S` (default 7,200) and
`BENCH_MEMORY_LIMIT_GB` (default 12). A build past either limit is stopped and recorded as `timeout` or
`memory limit` rather than dropped, and a scale run stops at the first size that fails. A server search
with no answer within 30 s counts as finding nothing, and the run records how many there were.

**completr segments.** completr indexes through a `SegmentWriter` with a memory budget of
`BENCH_COMPLETR_MEMORY_BUDGET_MB` (default 256, as tantivy's writer here): rows stream in from a generator,
and the writer starts a new segment file whenever building more would pass the budget. An index of several
segments ranks exactly like one. HN fits in a single segment.

Run it in the foreground on an idle machine: background jobs get a lower scheduling priority, which skews
timings. Servers are stopped at the end of each run; the index data stays in `bench/.cache/work/`.

## Caveats

- **In-process against network.** completr and tantivy run in the benchmark process, whereas Typesense and
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
- **Mostly synthetic queries.** Apart from AmazonQAC's typed prefixes and the real misspellings, typos are
  random edits, and targets are typed from their first character. The quality metrics reward the target's rank only; other good suggestions count
  for nothing.
- **Memory measurement.** Figures are the physical footprint the operating system charges the process
  (`phys_footprint` on macOS, where these runs were made; RSS elsewhere), which on macOS leaves out the file
  pages of a memory-mapped index: those stay in the page cache, shared and evictable. Server figures are the
  whole server process.
- **tantivy is emulated.** Its results reflect the adapter described above as much as tantivy itself.
