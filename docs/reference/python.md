# Python API

The reference for the `strato` package, generated from its typed stubs
([`__init__.pyi`](https://github.com/sayef/strato/blob/main/crates/strato-py/python/strato/__init__.pyi)).
Every name on this page is importable from `strato`.

## Connecting

::: strato.connect
    options:
      heading_level: 3

`connect` takes the same arguments as [`Database`][strato.Database]: `url`, `options`, `cache_dir`, and
the keyword-only build options `min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`,
`compact_keys` and `build_threads`.

::: strato.connect_async
    options:
      heading_level: 3

## Documents

::: strato.Document
    options:
      heading_level: 3

::: strato.DocumentDict
    options:
      heading_level: 3

::: strato.Id
    options:
      heading_level: 3

::: strato.Documents
    options:
      heading_level: 3

::: strato.Vector
    options:
      heading_level: 3

::: strato.Vectors
    options:
      heading_level: 3

::: strato.key_id
    options:
      heading_level: 3

## Suggestions

::: strato.Suggestion
    options:
      heading_level: 3

::: strato.HybridSuggestion
    options:
      heading_level: 3

::: strato.AliasSuggestion
    options:
      heading_level: 3

::: strato.MatchKind
    options:
      heading_level: 3

::: strato.FusionKind
    options:
      heading_level: 3

## Segments, indexes and engines

::: strato.Segment
    options:
      heading_level: 3

::: strato.Index
    options:
      heading_level: 3

::: strato.Engine
    options:
      heading_level: 3

## Databases

::: strato.Database
    options:
      heading_level: 3

::: strato.Transaction
    options:
      heading_level: 3

::: strato.ChangeSet
    options:
      heading_level: 3

::: strato.Ingestor
    options:
      heading_level: 3

::: strato.Replica
    options:
      heading_level: 3

::: strato.Lease
    options:
      heading_level: 3

::: strato.Store
    options:
      heading_level: 3

## asyncio

::: strato.AsyncDatabase
    options:
      heading_level: 3

::: strato.AsyncEngine
    options:
      heading_level: 3

## Errors

Every error derives from `StratoError`. `NotFoundError` is also a `LookupError`, `InvalidInputError` a
`ValueError`, and `StorageError` an `OSError`.

::: strato.StratoError
    options:
      heading_level: 3

::: strato.ConflictError
    options:
      heading_level: 3

::: strato.CorruptionError
    options:
      heading_level: 3

::: strato.NotFoundError
    options:
      heading_level: 3

::: strato.InvalidInputError
    options:
      heading_level: 3

::: strato.StorageError
    options:
      heading_level: 3
