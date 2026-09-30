# Documents

A document is one completion target: an id, one text, a popularity weight, and optional aliases, context
tags and an embedding. strato does not have multi-field schemas.

## Fields

| Field | Type | Meaning |
|---|---|---|
| `id` | `int` or `str` | Your identifier. String ids are hashed to a stable 64-bit id and returned as given. |
| `text` | `str` | What is completed and displayed. |
| `popularity` | `float` | Usually in `[0, 1]`; more popular documents rank higher. Defaults to 0. |
| `synonyms` | `list[str]` | Alternative names, searchable with `complete_aliases`. |
| `abbreviations` | `list[str]` | Codes such as `ML`, matched exactly by `complete`. |
| `contexts` | `list[str]` | Tags that requests can filter on, for example a category, tenant or language. |
| `vector` | float array | Optional embedding; or pass `vectors=` for all documents at once. |

Only `id` and `text` are required. A repeated id within one build keeps its last document.

=== "Python"

    ```python
    from strato import Index

    docs = [
        {
            "id": "ml",
            "text": "Machine Learning",
            "popularity": 0.9,
            "abbreviations": ["ML"],
            "synonyms": ["statistical learning"],
            "contexts": ["courses"],
        },
        {"id": 7, "text": "Machine Vision"},
    ]
    index = Index.from_documents(docs)
    print(index.get("ml"), index.get(7))
    ```

=== "Rust"

    ```rust
    use strato::{Document, Index};

    let index = Index::from_documents([
        Document::keyed("ml", "Machine Learning", 0.9)
            .with_abbreviation("ML")
            .with_synonym("statistical learning")
            .with_context("courses"),
        Document::new(7, "Machine Vision", 0.0),
    ])?;
    let ml = index.document_by_key("ml");
    ```

## String ids

A string id is hashed with `key_id` to a 64-bit id. The index stores the string too, and suggestions
return it, so you rarely need the number. Deletes accept the same string.

```python
import strato

print(strato.key_id("ml"))                     # the stable 64-bit id behind "ml"
print(index.complete("mach")[0].id)            # 'ml', as given
print(index.get(strato.key_id("ml")).id)       # 'ml' as well
```

In Rust, `Document::keyed(key, text, popularity)` sets a string key, and `Suggestion::key` returns it next
to the numeric `id`. `ChangeSet::delete` and `Transaction::append_documents` take numeric ids, so pass
`key_id("ml")` for keyed documents.

## Input formats

Everything that takes documents in Python (`Index.from_documents`, `Segment.build`, `Transaction.append`,
`Transaction.overwrite` and `ChangeSet.upsert`) accepts any of the following.

=== "Dicts"

    ```python
    Index.from_documents([{"id": "ds", "text": "Data Science", "popularity": 0.7}])
    ```

=== "Document"

    ```python
    from strato import Document

    Index.from_documents([Document("ds", "Data Science", 0.7, synonyms=["data analytics"])])
    ```

=== "pandas"

    ```python
    import pandas as pd

    frame = pd.DataFrame({"id": ["ds", "cs"], "text": ["Data Science", "Computer Science"], "popularity": [0.7, 0.6]})
    Index.from_documents(frame)
    ```

=== "polars"

    ```python
    import polars as pl

    frame = pl.DataFrame({"id": ["ds", "cs"], "text": ["Data Science", "Computer Science"], "popularity": [0.7, 0.6]})
    Index.from_documents(frame)
    ```

=== "Arrow"

    ```python
    import pyarrow as pa

    table = pa.table({"id": ["ds", "cs"], "text": ["Data Science", "Computer Science"], "popularity": [0.7, 0.6]})
    Index.from_documents(table)
    ```

Table columns use the field names above. List columns (`synonyms`, `abbreviations`, `contexts`) may hold
nulls or `NaN`, which count as empty. The tables are converted row by row, so pandas, polars and pyarrow
are optional dependencies that strato does not install.

`Document` objects expose the fields as read-only properties, and `to_dict()` returns the dict form:

```python
doc = Document("ds", "Data Science", 0.7, synonyms=["data analytics"])
print(doc.id, doc.text, doc.synonyms)
print(doc.to_dict()["synonyms"])   # ['data analytics']
```

## Embeddings

Give each document a `vector`, or pass one float32 row per document as `vectors=`, for example a NumPy
array of shape `(n, dim)`. All vectors of an index must have the same dimension. See
[Semantic and hybrid](semantic-hybrid.md).

```python
import numpy as np

vectors = np.random.default_rng(0).standard_normal((2, 64), dtype=np.float32)
index = Index.from_documents(
    [{"id": "ds", "text": "Data Science"}, {"id": "cs", "text": "Computer Science"}],
    vectors=vectors,
)
print(index.vector_dim)   # 64
```

## Unknown fields

Unknown fields raise `InvalidInputError` (a `ValueError`), so a typo never silently drops data. The same
applies to table columns.

```python
from strato import InvalidInputError

try:
    Index.from_documents([{"id": 1, "text": "Data Science", "popularty": 0.7}])
except InvalidInputError as error:
    print(error)   # unknown document field "popularty"; expected id, text, popularity, ...
```

## Text and languages

Text is normalised with Unicode lowercasing and split on Unicode whitespace, so any language written with
spaces works as it is. For scripts written without spaces, such as Chinese and Japanese, pass text
segmented into words to get infix matching. Words shorter than `min_word_chars` (3 by default) are not
indexed for infix or fuzzy matching.
