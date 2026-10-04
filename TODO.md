# To do

Open items: measurements still to make, and gaps the benchmarks show. Remove an item once it is done.

## Benchmarks to run

- **Timings on a dedicated machine.** The published runs are from one laptop that was also doing other work.
  Rerun the suite on a dedicated Linux machine, one engine at a time, and measure Linux peaks the same way as
  on macOS (a peak since a reset, via `/proc/PID/clear_refs`).
- **Typesense on Open Library.** It indexed, then a search answered with an error after 10 hours of queries
  and the run was lost; the harness now counts error answers instead of stopping. Rerun it.
- **Uncompacted row on MusicBrainz.** Left out: querying its 113 segments made the run take hours. Rerun on
  the dedicated machine.
- **Real misspellings on every corpus.** The misspelled set is complete on HN only; the other engines' rows on
  Wikipedia, MusicBrainz, AmazonQAC and Open Library are still to measure.
- **Repeatable build peaks.** Two identical compactions peaked 120 MB apart (allocator purge timing and
  memory compression). Pin the allocator's purging or report the median of several runs.

## Gaps to close

- **Index time on MusicBrainz and Open Library**, where tantivy is faster. Re-encoding every text with the
  merged FSST table is the largest cost of compaction; page faults while reading the parts add about 10%.
- **Build memory at 40 million documents**, where tantivy peaks lower: the merged words and the places of
  each part's documents dominate.
- **Memory after open**, where tantivy keeps less resident.
- **AmazonQAC**: one-typo and multi-word p99 against tantivy, and uniform typo'd targets against Meilisearch.
- **Match kinds and scores on typos.** `headphnes` is labelled `infix` rather than `fuzzy`, and the typo'd
  `playstaton` scores above the clean `play`; check how a corrected word's kind and score are carried.

## API

- **Queued writes for collections.** `completr.Client(..., writes="queued")` routing `add` and `delete`
  through the inbox and a lease-elected ingestor, for fleets of writers whose commits keep colliding.
- **Surviving `fork()`.** The shared tokio runtime does not survive a fork, so a database or engine opened
  before a pre-forking server forks (gunicorn `--preload`) cannot be used in the children; restart the
  runtime in a child instead of asking users to open after the fork.

## Before the first release

- Changelog.
- Review the open Dependabot pull requests.
- Check that CI passes once the repository is public, and deploy the docs from CI instead of by hand.
