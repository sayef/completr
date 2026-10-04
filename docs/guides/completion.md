# Completion

`complete` returns ranked suggestions for what a user has typed so far. Every match kind is collected and
ranked in one request; you do not choose between prefix, infix or fuzzy search.

<!-- skip-test -->
```python
index.complete(query, limit=10, *, contexts=None)
engine.complete(query, layers, limit=10, *, contexts=None)
```

| Method | Returns |
|---|---|
| `complete` | Direct matches: exact, prefix, abbreviation, infix and fuzzy, as `Suggestion`s. |
| `complete_aliases` | Documents whose synonyms match, as `AliasSuggestion`s. See [Synonyms](#synonyms). |
| `vector_search`, `hybrid_search` | Nearest documents by embedding, alone or fused with `complete`. See [Semantic and hybrid](semantic-hybrid.md). |

An `Index` searches one index. An `Engine`, including one from `db.engine()`, takes a list of index names
to search as [layers](layers.md). Both have the same methods and options. In Rust, the `_with` variants
take `SearchOptions`:

```rust
use completr::SearchOptions;

let pop = SearchOptions::new(5).contexts(["pop"]);
let hits = index.complete_with("danc", &pop);
let layered = engine.complete_with(&["songs"], "danc", &pop);
```

The examples on this page use this index:

```python
from completr import Index

index = Index.from_documents([
    {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95, "synonyms": ["is this the real life"], "contexts": ["rock"]},
    {"id": "killer", "text": "Killer Queen – Queen", "popularity": 0.6, "contexts": ["rock"]},
    {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85, "contexts": ["pop", "disco"]},
    {"id": "dark", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 0.7, "contexts": ["rock"]},
    {"id": "ownown", "text": "Dancing On My Own – Robyn", "popularity": 0.5, "contexts": ["pop"]},
    {"id": "bridge", "text": "Under the Bridge – Red Hot Chili Peppers", "popularity": 0.75, "abbreviations": ["RHCP"], "contexts": ["rock"]},
    {"id": "teen", "text": "Smells Like Teen Spirit – Nirvana", "popularity": 0.8, "abbreviations": ["SLTS"], "contexts": ["rock"]},
    {"id": "billie", "text": "Billie Jean – Michael Jackson", "popularity": 0.9, "abbreviations": ["MJ"], "contexts": ["pop"]},
])
```

## Match kinds

| Query | Top suggestion | `kind` | Highlights |
|---|---|---|---|
| `killer queen – queen` | Killer Queen – Queen | `exact` | **Killer** **Queen** **–** **Queen** |
| `danc` | Dancing Queen – ABBA, Dancing in the Dark – Bruce Springsteen | `prefix` | **Danc**ing Queen – ABBA |
| `Dancing Qu` | Dancing Queen – ABBA | `prefix` | **Dancing** **Qu**een – ABBA |
| `rhcp` | Under the Bridge – Red Hot Chili Peppers | `abbreviation` | none |
| `queen` | Bohemian Rhapsody – Queen, Dancing Queen – ABBA | `infix` | Bohemian Rhapsody – **Queen** |
| `rapsody` | Bohemian Rhapsody – Queen | `fuzzy` | Bohemian **Rhapsody** – Queen |
| `bohemain rapsody` | Bohemian Rhapsody – Queen | `fuzzy` | **Bohemian** **Rhapsody** – Queen |
| `dancingqueen` | Dancing Queen – ABBA | `prefix` | **Dancing** **Queen** – ABBA |
| `is this the real`, through `complete_aliases` | Bohemian Rhapsody – Queen | `synonym` | none |

```python
for query in ["killer queen – queen", "danc", "rhcp", "queen", "bohemain rapsody", "dancingqueen"]:
    print(f"{query!r:22}", [(s.text, s.kind) for s in index.complete(query, limit=2)])
```

```text
'killer queen – queen' [('Killer Queen – Queen', 'exact')]
'danc'                 [('Dancing Queen – ABBA', 'prefix'), ('Dancing in the Dark – Bruce Springsteen', 'prefix')]
'rhcp'                 [('Under the Bridge – Red Hot Chili Peppers', 'abbreviation')]
'queen'                [('Bohemian Rhapsody – Queen', 'infix'), ('Dancing Queen – ABBA', 'infix')]
'bohemain rapsody'     [('Bohemian Rhapsody – Queen', 'fuzzy')]
'dancingqueen'         [('Dancing Queen – ABBA', 'prefix')]
```

Notes on each kind:

- **Exact and prefix** matching works on the whole text and word by word, so `Dancing Qu` completes
  *Dancing Queen – ABBA*. Every query word must start a word of the text.
- **Abbreviations** match exactly and case-insensitively: `rhcp` finds a document with the abbreviation
  `RHCP`, but `rhc` does not match it as an abbreviation, so short codes do not flood the results.
- **Infix** matches a word inside the text. Words shorter than `min_word_chars` (3) are not indexed for
  infix matching.
- **Fuzzy** matching corrects up to `max_edit_distance` (2) edits per word, SymSpell-style, and verifies
  candidates with Levenshtein distance. Fuzzy hits always rank below the weakest direct match.
- **Word decomposition** splits run-together input, such as `dancingqueen`, into dictionary words.
- **Synonyms** are matched by `complete_aliases`; see [Synonyms](#synonyms).
- **Semantic** hits come from embeddings; see [Semantic and hybrid](semantic-hybrid.md).

## Highlights

`highlights` lists the `(start, end)` character ranges of `text` that matched, sorted, ready to render in
bold. Abbreviation and synonym matches have no highlights, since the query does not appear in the text.

=== "Python"

    ```python
    def render(s):
        out, last = [], 0
        for start, end in s.highlights:
            out += [s.text[last:start], "<b>", s.text[start:end], "</b>"]
            last = end
        return "".join(out + [s.text[last:]])

    print([render(s) for s in index.complete("dancing qu")])
    ```

=== "Rust"

    ```rust
    // Rust highlights are byte ranges of `text`.
    let top = &index.complete("dancing qu", 10)[0];
    let parts: Vec<&str> = top.highlights.iter().map(|r| &top.text[r.clone()]).collect();
    ```

```text
['<b>Dancing</b> <b>Qu</b>een – ABBA']
```

## Contexts

Tag documents with `contexts` and pass `contexts=` to any search. A document qualifies when it has **any**
of the requested contexts. Filtering happens while candidates are collected, so a filtered request still
returns up to `limit` results.

=== "Python"

    ```python
    print([s.text for s in index.complete("danc", contexts=["pop"])])
    print([s.text for s in index.complete("danc", contexts=["disco", "rock"])])
    ```

=== "Rust"

    ```rust
    let pop = SearchOptions::new(10).contexts(["pop"]);
    let hits = index.complete_with("danc", &pop);
    ```

```text
['Dancing Queen – ABBA', 'Dancing On My Own – Robyn']
['Dancing Queen – ABBA', 'Dancing in the Dark – Bruce Springsteen']
```

Contexts are the only filter completr has. Use them for genres, tenants or languages, or use separate
indexes and [layers](layers.md) when the partitions are large.

## Synonyms

`complete_aliases` matches synonyms by prefix and returns the documents they belong to, as
`AliasSuggestion`s with `id`, `text` (the document's, not the synonym), `score` and `layer`. It is a
separate call, so an alternative name never displaces something the user typed:

```python
print(index.complete_aliases("is this the real"))
print(index.complete_aliases("bohemian"))
```

```text
[AliasSuggestion(id='bohemian', text="Bohemian Rhapsody – Queen", score=0.6637)]
[]
```

To show synonyms below the direct matches, append the alias hits that are not already there:

```python
def complete_with_synonyms(index, query, limit=10):
    direct = index.complete(query, limit)
    seen = {s.id for s in direct}
    return direct + [a for a in index.complete_aliases(query, limit) if a.id not in seen][: limit - len(direct)]

print([s.text for s in complete_with_synonyms(index, "is this the real life")])
```

```text
['Bohemian Rhapsody – Queen']
```

A [collection](collections.md)'s `complete(..., aliases=True)` does this in one call.

## Limits

`limit` (10 by default) caps the number of suggestions. Queries of up to `short_query_chars` characters
(3 by default) are answered from a per-index cache of `short_query_limit` (100) results, so one- to
three-character prefixes, which match large parts of an index, stay fast. Requests with `contexts` bypass
this cache.

```python
print(len(index.complete("d", limit=1)))
```

```text
1
```

## Ranking

Suggestions are ordered by score, then shorter text, then id. The score combines the match kind, the text
length, whole-word bonuses and popularity; [Concepts](../concepts.md#scoring-overview) lists the formula.
`popularity_weight` (0.4 by default) sets how much popularity counts, and `max_score` the raw score that
normalises to 1.0:

```python
flat = Index.from_documents(
    [{"id": "a", "text": "Dancing Queen – ABBA", "popularity": 0.0}, {"id": "b", "text": "Dancing in the Dark – Bruce Springsteen", "popularity": 1.0}],
    popularity_weight=0.0,
)
print([s.text for s in flat.complete("danc")])   # shorter text first
```

```text
['Dancing Queen – ABBA', 'Dancing in the Dark – Bruce Springsteen']
```

An index estimates `max_score` from its documents unless you pass one. A database stores it per index.
Pin it, with `max_score=` or `Transaction.set_max_score`, when scores must stay comparable across
rebuilds. `popularity_weight` is also an option of `db.engine()`, `db.open_index()` and `Index(...)`;
[Configuration](configuration.md#index-options) lists every index option.
