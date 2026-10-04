# Python API

The reference for the `completr` package, generated from its typed stubs:
[`__init__.pyi`](https://github.com/sayef/completr/blob/main/crates/completr-py/python/completr/__init__.pyi),
[`completr.pyi`](https://github.com/sayef/completr/blob/main/crates/completr-py/python/completr/completr.pyi)
and [`_async.pyi`](https://github.com/sayef/completr/blob/main/crates/completr-py/python/completr/_async.pyi).
Every name on this page is importable from `completr`.

## Databases

::: completr.connect
    options:
      heading_level: 3

`connect(url, ...)` is `Database(url, ...)`: it takes `url`, `options`, `cache_dir` and the build options
`min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`, `compact_keys` and
`build_threads`. See [Configuration](../guides/configuration.md#database-options).

::: completr.Database
    options:
      heading_level: 3

::: completr.Transaction
    options:
      heading_level: 3

::: completr.Lease
    options:
      heading_level: 3

::: completr.Store
    options:
      heading_level: 3

## Indexes and engines

::: completr.Index
    options:
      heading_level: 3

::: completr.Segment
    options:
      heading_level: 3

::: completr.SegmentWriter
    options:
      heading_level: 3

::: completr.Engine
    options:
      heading_level: 3

::: completr.Replica
    options:
      heading_level: 3

## Many writers

::: completr.ChangeSet
    options:
      heading_level: 3

::: completr.Ingestor
    options:
      heading_level: 3

## Documents

::: completr.Document
    options:
      heading_level: 3

::: completr.DocumentDict
    options:
      heading_level: 3

::: completr.Id
    options:
      heading_level: 3

::: completr.Documents
    options:
      heading_level: 3

::: completr.Vector
    options:
      heading_level: 3

::: completr.Vectors
    options:
      heading_level: 3

::: completr.key_id
    options:
      heading_level: 3

## Suggestions

::: completr.Suggestion
    options:
      heading_level: 3

::: completr.AliasSuggestion
    options:
      heading_level: 3

::: completr.HybridSuggestion
    options:
      heading_level: 3

::: completr.MatchKind
    options:
      heading_level: 3

::: completr.FusionKind
    options:
      heading_level: 3

## asyncio

::: completr.AsyncDatabase
    options:
      heading_level: 3

::: completr.AsyncEngine
    options:
      heading_level: 3

## Errors

Every error derives from `CompletrError`. `NotFoundError` is also a `LookupError`, `InvalidInputError` a
`ValueError`, and `StorageError` an `OSError`.

::: completr.CompletrError
    options:
      heading_level: 3

::: completr.ConflictError
    options:
      heading_level: 3

::: completr.CorruptionError
    options:
      heading_level: 3

::: completr.NotFoundError
    options:
      heading_level: 3

::: completr.InvalidInputError
    options:
      heading_level: 3

::: completr.StorageError
    options:
      heading_level: 3

## Collections

The [collections](../guides/collections.md) layer.

::: completr.Client
    options:
      heading_level: 3

::: completr.Collection
    options:
      heading_level: 3

::: completr.CollectionStats
    options:
      heading_level: 3

::: completr.OptimizeKind
    options:
      heading_level: 3

::: completr.AsyncClient
    options:
      heading_level: 3

::: completr.AsyncCollection
    options:
      heading_level: 3
