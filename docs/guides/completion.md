# Completion

`complete` returns ranked suggestions for what a user has typed so far. Every match kind is collected and
ranked in one request; you do not choose between prefix, infix or fuzzy search.

The examples on this page use this index:

```python
from completr import Index

index = Index.from_documents([
    {"id": "ml", "text": "Machine Learning", "popularity": 0.9, "abbreviations": ["ML"], "contexts": ["courses"]},
    {"id": "mv", "text": "Machine Vision", "popularity": 0.4, "contexts": ["books"]},
    {"id": "ds", "text": "Data Science", "popularity": 0.7, "synonyms": ["data analytics"], "contexts": ["courses", "books"]},
    {"id": "cs", "text": "Computer Science", "popularity": 0.6},
    {"id": "nlp", "text": "Natural Language Processing", "popularity": 0.5, "abbreviations": ["NLP"]},
])
```

## Match kinds

| Query | Top suggestion | `kind` | Highlights |
|---|---|---|---|
| `machine learning` | Machine Learning | `exact` | **Machine** **Learning** |
| `mach` | Machine Learning, Machine Vision | `prefix` | **Mach**ine Learning |
| `Data Sc` | Data Science | `prefix` | **Data** **Sc**ience |
| `nlp` | Natural Language Processing | `abbreviation` | none |
| `science` | Data Science, Computer Science | `infix` | Data **Science** |
| `vison` | Machine Vision | `fuzzy` | Machine **Vision** |
| `machne lerning` | Machine Learning | `fuzzy` | **Machine** **Learning** |
| `datascience` | Data Science | `exact` | **Data** **Science** |

```python
for query in ["machine learning", "mach", "nlp", "science", "machne lerning", "datascience"]:
    print(f"{query!r:18}", [(s.text, s.kind) for s in index.complete(query, limit=2)])
```

```text
'machine learning' [('Machine Learning', 'exact')]
'mach'             [('Machine Learning', 'prefix'), ('Machine Vision', 'prefix')]
'nlp'              [('Natural Language Processing', 'abbreviation')]
'science'          [('Data Science', 'infix'), ('Computer Science', 'infix')]
'machne lerning'   [('Machine Learning', 'fuzzy')]
'datascience'      [('Data Science', 'exact')]
```

Notes on each kind:

- **Exact and prefix** matching works on the whole text and word by word, so `Data Sc` completes
  *Data Science*. Every query word must start a word of the text.
- **Abbreviations** match exactly and case-insensitively: `k8s` finds a document with the abbreviation
  `K8S`, but `k8` does not, so short codes do not flood the results.
- **Infix** matches a word inside the text. Words shorter than `min_word_chars` (3) are not indexed for
  infix matching.
- **Fuzzy** matching corrects up to `max_edit_distance` (2) edits per word, SymSpell-style, and verifies
  candidates with Levenshtein distance. Fuzzy hits always rank below the weakest direct match.
- **Word decomposition** splits run-together input, such as `datascience`, into dictionary words.

## Highlights

`highlights` lists the `(start, end)` character ranges of `text` that matched, sorted, ready to render in
bold. Abbreviation matches have no highlights, since the query does not appear in the text.

=== "Python"

    ```python
    def render(s):
        out, last = [], 0
        for start, end in s.highlights:
            out += [s.text[last:start], "<b>", s.text[start:end], "</b>"]
            last = end
        return "".join(out + [s.text[last:]])

    print([render(s) for s in index.complete("data sc")])   # ['<b>Data</b> <b>Sc</b>ience']
    ```

=== "Rust"

    ```rust
    // Rust highlights are byte ranges of `text`.
    let top = &index.complete("data sc", 1)[0];
    let parts: Vec<&str> = top.highlights.iter().map(|r| &top.text[r.clone()]).collect();
    ```

## Contexts

Tag documents with `contexts` and pass `contexts=` to any request (`complete`, `complete_aliases`,
`vector_search` and `hybrid_search`). A document qualifies when it has **any** of the requested contexts.
Filtering happens while candidates are collected, so a filtered request still returns up to `limit`
results.

=== "Python"

    ```python
    print([s.text for s in index.complete("mach", contexts=["books"])])              # ['Machine Vision']
    print([s.text for s in index.complete("sci", contexts=["courses", "books"])])    # ['Data Science']
    ```

=== "Rust"

    ```rust
    use completr::SearchOptions;

    let books = SearchOptions::new(10).contexts(["books"]);
    let hits = index.complete_with("mach", &books);
    ```

Contexts are the only filter completr has. Use them for categories, tenants or languages, or use separate
indexes and [layers](layers.md) when the partitions are large.

## Synonyms

Synonyms are searched with `complete_aliases`, which prefix-matches the synonyms and returns the documents
they belong to. Keeping them separate from `complete` means an alternative name never displaces a direct
match; call both if your box shows both.

```python
print(index.complete_aliases("data an"))   # [AliasSuggestion(id='ds', text="Data Science", score=...)]
```

An `AliasSuggestion` has `id`, `text` (the document's text, not the synonym), `score` and `layer`.

## Limits

`limit` (10 by default) caps the number of suggestions. Queries of up to `short_query_chars` characters
(3 by default) are answered from a per-index cache of `short_query_limit` (100) results, so one- to
three-character prefixes, which match large parts of an index, stay fast. Requests with `contexts` bypass
this cache.

```python
print(len(index.complete("m", limit=1)))   # 1
```

## Ranking

Suggestions are ordered by score, then shorter text, then id. The score combines the match kind, the text
length, whole-word bonuses and popularity; [Concepts](../concepts.md#scoring-overview) lists the formula.
`popularity_weight` (0.4 by default) sets how much popularity counts:

```python
flat = Index.from_documents(
    [{"id": "a", "text": "Machine Vision", "popularity": 0.0}, {"id": "b", "text": "Machine Learning", "popularity": 1.0}],
    popularity_weight=0.0,
)
print([s.text for s in flat.complete("mach")])   # shorter text first: ['Machine Vision', 'Machine Learning']
```
