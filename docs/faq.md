# FAQ

## What does "serverless" mean here?

The same as for [LanceDB](https://github.com/lancedb/lancedb): there is no strato server. The engine is a
library in your process, and the database is a set of files in a bucket or directory. Your application is
still deployed however you like, on containers, VMs or functions; strato adds no extra service to it.

## How does strato differ from Elasticsearch, OpenSearch, Typesense or Meilisearch?

Those are search servers for documents with many fields, filters and facets: you run and scale a cluster,
and every query crosses the network. strato is serverless and does autocompletion only: it runs inside
your process, and ranks the match kinds a completion box needs (abbreviations, spelling correction, word
decomposition) together with popularity. Many applications use both: a search server for the results
page, strato for the box.

## How do several writers avoid conflicts without a server?

Through the store. Commits create the next manifest create-only, so exactly one writer wins each version
and the others rebase. For high write rates, processes submit change sets to an inbox, and a single
`Ingestor` elected by an expiring lease commits them in order. Every commit carries the lease generation
as a fencing token, so an ingestor that lost its lease cannot commit.

## How does it differ from an FST or trie library?

[`fst`](https://crates.io/crates/fst) and [marisa-trie](https://github.com/s-yata/marisa-trie) are
excellent key dictionaries, and strato uses the same ideas internally. On top of them it adds scoring,
typo tolerance, abbreviations, semantic search, updates without rebuilding, and storage.

## Which languages does it support?

Text is normalised with Unicode lowercasing and split on Unicode whitespace, so any language written with
spaces works as it is. For scripts written without spaces, such as Chinese and Japanese, pass text
segmented into words to get infix matching.

## How large can an index be?

A segment is memory-mapped and costs roughly 100 bytes per short title, plus about 136 bytes per 256-d
vector at 4 bits. Hundreds of thousands to a few million documents per index fit comfortably on one
machine.

## How fresh are completions after an update?

A change is visible once an ingestor has committed it and a serving process has synced. With an ingestor
running one-second rounds and engines syncing every few seconds, updates typically appear within a few
seconds. Transactions committed directly are visible at the next sync.

## Is the on-disk format stable?

strato is young. The format is versioned and checked on load, but it and the API may still change before
1.0. Segments of an older format are rejected rather than misread, and must be rebuilt.

## Why the name?

*Strato* is Italian for *layer*, from the Latin *stratum*. Indexes are stacks of immutable segments in
storage, and searches stack override layers: completions are served from strata.

## Non-features

strato does one job. It deliberately does not include:

- a server or HTTP API; it is serverless by design, so expose it through your own API if you need one (see
  the [FastAPI example](https://github.com/sayef/strato/tree/main/examples/fastapi));
- full-text search over long documents, filtering beyond [contexts](guides/completion.md#contexts), or
  faceting;
- multi-field schemas; each document has one text, plus aliases and an optional embedding;
- built-in embedding models; bring vectors from any model;
- sharding across machines; one index is expected to fit on one machine, memory-mapped.
