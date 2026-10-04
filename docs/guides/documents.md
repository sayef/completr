# Documents

A document is one completion target: an id, one text, a popularity weight, and optional aliases, context
tags and an embedding. completr does not have multi-field schemas. Documents go into segments, through
`Index.from_documents`, a transaction, `Segment.build` or a `SegmentWriter`.

## Fields

| Field | Type | Meaning |
|---|---|---|
| `id` | `int` or `str` | Your identifier. String ids are hashed to a stable 64-bit id and returned as given. |
| `text` | `str` | What is completed and displayed. |
| `popularity` | `float` | Usually in `[0, 1]`; more popular documents rank higher. Defaults to 0. |
| `synonyms` | `list[str]` | Alternative names, matched by `complete_aliases`. |
| `abbreviations` | `list[str]` | Codes such as `RHCP`, matched exactly by `complete`. |
| `contexts` | `list[str]` | Tags that requests can filter on, for example a genre, tenant or language. |
| `vector` | float array | Optional embedding; or pass `vectors=` for all documents at once. |

Only `id` and `text` are required.

=== "Python"

    ```python
    import completr
    from completr import Index

    docs = [
        {
            "id": "bridge",
            "text": "Under the Bridge – Red Hot Chili Peppers",
            "popularity": 0.75,
            "abbreviations": ["RHCP"],
            "synonyms": ["sometimes i feel"],
            "contexts": ["rock", "alternative"],
        },
        {"id": 7, "text": "Under Pressure – Queen & David Bowie"},
    ]
    index = Index.from_documents(docs)
    print([(s.id, s.text) for s in index.complete("under")])
    ```

=== "Rust"

    ```rust
    use completr::{Document, Index};

    let index = Index::from_documents([
        Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75)
            .with_abbreviation("RHCP")
            .with_synonym("sometimes i feel")
            .with_context("rock")
            .with_context("alternative"),
        Document::new(7, "Under Pressure – Queen & David Bowie", 0.0),
    ])?;
    ```

```text
[('bridge', 'Under the Bridge – Red Hot Chili Peppers'), (7, 'Under Pressure – Queen & David Bowie')]
```

`index.get(id)` returns a stored document, by int id, string key or `key_id`. In Rust, use
`document(id)` and `document_by_key(key)`.

## Adding, replacing and deleting

In a database, a transaction's `append(index, documents, deletes=())` adds a segment of upserts and deletes
to an index: a document whose id is already in the index replaces the old one, and `deletes` hides ids,
ignoring ids that are not present. A repeated id within one call keeps its last document.
`overwrite(index, documents)` replaces the whole index.

```python
db = completr.connect("./data")
txn = db.begin()
txn.append("songs", docs)
txn.commit()
engine = db.engine()

txn = db.begin()
txn.append("songs", [{"id": 7, "text": "Under Pressure (Remastered) – Queen & David Bowie", "popularity": 0.4}])
txn.commit()
engine.sync()   # load the new version now, rather than at the next background sync
print([s.text for s in engine.complete("under", ["songs"])])

txn = db.begin()
txn.append("songs", [], deletes=[7])
txn.commit()
engine.sync()
print([s.text for s in engine.complete("under", ["songs"])])
```

```text
['Under the Bridge – Red Hot Chili Peppers', 'Under Pressure (Remastered) – Queen & David Bowie']
['Under the Bridge – Red Hot Chili Peppers']
```

Each commit writes one segment per changed index, so add documents in batches rather than one at a time
where you can. [Compaction](serverless.md#compaction) merges small segments.

## String ids

A string id is hashed with `key_id` to a 64-bit id. The segment stores the string too, and suggestions
return it, so you rarely need the number. `deletes` and `get` accept the same string.

```python
print(completr.key_id("bridge"))        # the stable 64-bit id behind "bridge"
print(index.complete("under")[0].id)    # 'bridge', as given
```

```text
17491315868598813430
bridge
```

In Rust, `Document::keyed(key, text, popularity)` sets a string key, and `Suggestion::key` returns it next
to the numeric `id`. Deletes take numeric ids, so pass `key_id("bridge")` for keyed documents.

## Input formats

`Index.from_documents`, `Segment.build`, `Transaction.append`, `Transaction.overwrite`,
`ChangeSet.upsert` and `SegmentWriter.add` accept any of the following, in Python.

=== "Dicts"

    ```python
    index = Index.from_documents([{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}])
    ```

=== "Document"

    ```python
    from completr import Document

    index = Index.from_documents([Document("bohemian", "Bohemian Rhapsody – Queen", 0.95, synonyms=["is this the real life"])])
    ```

=== "pandas"

    ```python
    import pandas as pd

    frame = pd.DataFrame({"id": ["billie", "killer"], "text": ["Billie Jean – Michael Jackson", "Killer Queen – Queen"], "popularity": [0.9, 0.6]})
    index = Index.from_documents(frame)
    ```

=== "polars"

    ```python
    import polars as pl

    frame = pl.DataFrame({"id": ["billie", "killer"], "text": ["Billie Jean – Michael Jackson", "Killer Queen – Queen"], "popularity": [0.9, 0.6]})
    index = Index.from_documents(frame)
    ```

=== "Arrow"

    ```python
    import pyarrow as pa

    table = pa.table({"id": ["billie", "killer"], "text": ["Billie Jean – Michael Jackson", "Killer Queen – Queen"], "popularity": [0.9, 0.6]})
    index = Index.from_documents(table)
    ```

Table columns use the field names above. List columns (`synonyms`, `abbreviations`, `contexts`) may hold
nulls or `NaN`, which count as empty. The tables are converted row by row, so pandas, polars and pyarrow
are optional dependencies that completr does not install.

`Document` objects expose the fields as read-only properties, and `to_dict()` returns the dict form:

```python
doc = Document("bohemian", "Bohemian Rhapsody – Queen", 0.95, synonyms=["is this the real life"])
print(doc.id, doc.text, doc.synonyms)
print(doc.to_dict()["synonyms"])
```

```text
bohemian Bohemian Rhapsody – Queen ['is this the real life']
['is this the real life']
```

## Embeddings

Give each document a `vector`, or pass one float32 row per document as `vectors=`, for example a NumPy
array of shape `(n, dim)`. All vectors of an index must have the same dimension. In Rust, use
`Document::with_vector`. See [Semantic and hybrid](semantic-hybrid.md).

```python
import numpy as np

vectors = np.random.default_rng(0).standard_normal((2, 64), dtype=np.float32)
songs = Index.from_documents(
    [{"id": "bohemian", "text": "Bohemian Rhapsody – Queen"}, {"id": "dancing", "text": "Dancing Queen – ABBA"}],
    vectors=vectors,
)
print(songs.vector_dim)
```

```text
64
```

## Unknown fields

Unknown fields raise `InvalidInputError` (a `ValueError`), so a typo never silently drops data. The same
applies to table columns.

```python
from completr import InvalidInputError

try:
    Index.from_documents([{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularty": 0.9}])
except InvalidInputError as error:
    print(error)
```

```text
unknown document field "popularty"; expected id, text, popularity, synonyms, abbreviations, contexts, vector
```

## Segments

`Segment.build` builds one segment from documents, and `deletes` lists ids it hides in older segments.
With `path=`, it streams the build into a file and memory-maps it. An `Index` takes segments oldest first;
a newer copy of an id supersedes older ones, and an index of many segments ranks exactly like one
compacted segment.

=== "Python"

    ```python
    from completr import Segment

    catalogue = [
        {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95},
        {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
        {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7},
    ]
    base = Segment.build(catalogue, path="songs.seg")
    delta = Segment.build([{"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9}], deletes=["dark"])

    index = Index([Segment.open("songs.seg"), delta])
    print([s.text for s in index.complete("danc")], [s.text for s in index.complete("bill")])

    merged = index.compact()   # one segment with the same documents
    print(len(merged), merged.deletes())
    ```

=== "Rust"

    ```rust
    use std::sync::Arc;
    use completr::{key_id, Document, Index, IndexOptions, Segment};

    let base = Segment::build(catalogue, [])?;
    let delta = Segment::build(
        [Document::keyed("billie", "Billie Jean – Michael Jackson", 0.9)],
        [key_id("dark")],
    )?;
    let index = Index::new(vec![Arc::new(base), Arc::new(delta)], IndexOptions::default())?;
    let merged = index.compact()?;
    ```

```text
['Dancing Queen – ABBA'] ['Billie Jean – Michael Jackson']
3 []
```

| Method | Does |
|---|---|
| `Segment.open(path)` | Memory-maps a segment file and checks its structure; reads only its headers. |
| `segment.verify()` | Checks the checksum and every section. |
| `Segment.from_bytes(data)`, `segment.to_bytes()`, `segment.save(path)` | Converts and stores segments; `from_bytes` verifies. |
| `segment.documents()`, `ids()`, `deletes()`, `len(segment)` | The segment's contents. |
| `index.compact(path=None)` | Merges an index's segments into one, byte for byte as a rebuild would. |
| `index.segments()` | The index's segments, oldest first. |

## Offline builds

A `SegmentWriter` writes segment files into a directory, starting a new file whenever building the
current one would pass its `memory_budget` (256 MB by default). So any corpus indexes in bounded memory,
and the segments rank exactly like one. Of several documents with one id, the last added wins.

=== "Python"

    ```python
    from completr import SegmentWriter

    writer = SegmentWriter("segments", memory_budget=1_000_000)
    writer.add([{"id": f"s{i}", "text": f"song {i}"} for i in range(20_000)])
    segments = writer.finish()   # segments/000000.seg, 000001.seg, ...

    index = Index(segments)
    print(len(segments), len(index))
    ```

=== "Rust"

    ```rust
    use std::sync::Arc;
    use completr::{BuildOptions, Document, Index, IndexOptions, SegmentWriter};

    let mut writer = SegmentWriter::new(BuildOptions::default(), "segments")?.memory_budget(1 << 20);
    for i in 0..20_000 {
        writer.add(Document::keyed(format!("s{i}"), format!("song {i}"), 0.0))?;
    }
    let segments = writer.finish()?.into_iter().map(Arc::new).collect();
    let index = Index::new(segments, IndexOptions::default())?;
    ```

```text
6 20000
```

Indexing the 124,440 HN titles takes 0.29 s and peaks at 59 MB; all of English Wikipedia, 7.2 million
titles, is written and compacted into one segment in 25.6 s. Segments can be shipped as files and opened
with `Segment.open` wherever they are served.

## Text and languages

Text is normalised with Unicode lowercasing and split on Unicode whitespace, so any language written with
spaces works as it is. For scripts written without spaces, such as Chinese and Japanese, pass text
segmented into words to get infix matching. Words shorter than `min_word_chars` (3 by default) are not
indexed for infix or fuzzy matching.
