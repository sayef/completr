# Python API

The reference for the `completr` package, generated from its typed stubs
([`__init__.pyi`](https://github.com/sayef/completr/blob/main/crates/completr-py/python/completr/__init__.pyi)).
Every name on this page is importable from `completr`.

## Connecting

::: completr.connect
    options:
      heading_level: 3

`connect` takes the same arguments as [`Database`][completr.Database]: `url`, `options`, `cache_dir`, and
the keyword-only build options `min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`,
`compact_keys` and `build_threads`.

::: completr.connect_async
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

::: completr.HybridSuggestion
    options:
      heading_level: 3

::: completr.AliasSuggestion
    options:
      heading_level: 3

::: completr.MatchKind
    options:
      heading_level: 3

::: completr.FusionKind
    options:
      heading_level: 3

## Segments, indexes and engines

::: completr.Segment
    options:
      heading_level: 3

::: completr.Index
    options:
      heading_level: 3

::: completr.Engine
    options:
      heading_level: 3

## Databases

::: completr.Database
    options:
      heading_level: 3

::: completr.Transaction
    options:
      heading_level: 3

::: completr.ChangeSet
    options:
      heading_level: 3

::: completr.Ingestor
    options:
      heading_level: 3

::: completr.Replica
    options:
      heading_level: 3

::: completr.Lease
    options:
      heading_level: 3

::: completr.Store
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
