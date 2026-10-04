# Layers

An `Engine` holds indexes by name. A search names a list of them, its **layers**. Later layers override
earlier ones per document id, so a tenant's edits, deletions and additions shadow the shared data without
copying it.

## Searching layers

`engine.publish` adds or replaces indexes by name, atomically. Pass the names to search, in order, to
`complete`:

=== "Python"

    ```python
    import completr
    from completr import Engine, Index

    shared = Index.from_documents([
        {"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95},
        {"id": "killer", "text": "Killer Queen – Queen", "popularity": 0.6},
        {"id": "dancing", "text": "Dancing Queen – ABBA", "popularity": 0.85},
    ])
    radio = Index.from_documents([{"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen", "popularity": 1.0}])   # overrides "bohemian"

    engine = Engine()
    engine.publish({"shared": shared, "radio": radio})
    for s in engine.complete("queen", ["shared", "radio"]):
        print(s.text, s.layer)
    ```

=== "Rust"

    ```rust
    use std::sync::Arc;
    use completr::Engine;

    let engine = Engine::new();
    engine.publish([
        ("shared".to_owned(), Some(Arc::new(shared))),
        ("radio".to_owned(), Some(Arc::new(radio))),
    ]);
    for layered in engine.complete(&["shared", "radio"], "queen", 10) {
        println!("{} from layer {}", layered.suggestion.text, layered.layer);
    }
    ```

```text
Bohemian Rhapsody (Remastered 2011) – Queen radio
Dancing Queen – ABBA shared
Killer Queen – Queen shared
```

`layer` names the index each suggestion came from. In Rust, the engine returns `LayeredSuggestion`s whose
`layer` is the position in the layer list.

- A layer name that is not published counts as an empty layer, so every tenant can use the same layer
  list whether or not it has overrides.
- `publish` replaces the named indexes atomically. A search already running keeps the versions it started
  with. `engine.publish({"radio": None})` removes a layer.
- `engine.get(name)` returns a published `Index`, and `engine.names()` lists them.
- `complete_aliases`, `vector_search` and `hybrid_search` take a list of layers too.

## How overrides apply

Each layer is searched on its own, and the results are merged by score. A later layer overrides an
earlier one for every id it holds or deletes, whether or not its own version matches the query:

- **Edits**: if the tenant renames a shared document, the shared version no longer appears, even for
  queries only the old text matched.
- **Deletions**: delete an id in the tenant's index and the shared document disappears for that tenant,
  even though the tenant's index never held it.
- **Additions**: documents that exist only in the tenant's index are completed alongside the shared ones.

```python
from completr import Segment

radio = Index([Segment.build([{"id": "bohemian", "text": "Bohemian Rhapsody (Remastered 2011) – Queen", "popularity": 1.0}], deletes=["killer"])])
engine.publish({"radio": radio})   # hides "killer" for radio only
print([(s.text, s.layer) for s in engine.complete("queen", ["shared", "radio"])])
print([(s.text, s.layer) for s in engine.complete("queen", ["shared"])])
```

```text
[('Bohemian Rhapsody (Remastered 2011) – Queen', 'radio'), ('Dancing Queen – ABBA', 'shared')]
[('Bohemian Rhapsody – Queen', 'shared'), ('Dancing Queen – ABBA', 'shared'), ('Killer Queen – Queen', 'shared')]
```

A layered search takes `limit * overfetch` candidates from each layer before overrides are applied, so
that overridden documents do not leave the result short. The default, 2, suits layers that override a
small share of the documents. Raise it for layers that shadow many results, with `Engine(overfetch=4)` or
`db.engine(overfetch=4)`.

## Layers in a database

An engine from `db.engine()` serves every index of the database by name, so layers are index names. A
tenant's overrides are an ordinary index: append its documents, and pass shared ids it hides as
`deletes`.

The library has no notion of tenants or languages. Express them as index names, such as `shared/en` and
`radio/en`, and as layer lists:

```python
db = completr.connect("./data")
txn = db.begin()
txn.append("shared/en", [{"id": "bohemian", "text": "Bohemian Rhapsody – Queen", "popularity": 0.95}])
txn.append("radio/en", [{"id": "billie", "text": "Billie Jean – Michael Jackson"}], deletes=["bohemian"])
txn.commit()

engine = db.engine(group_separator="/")
print([(s.text, s.layer) for s in engine.complete("boh", ["shared/en", "radio/en"])])
print([(s.text, s.layer) for s in engine.complete("bill", ["shared/en", "radio/en"])])
```

```text
[]
[('Billie Jean – Michael Jackson', 'radio/en')]
```

`group_separator="/"` makes the engine switch indexes group by group when it syncs, grouping by the part
after the last separator: all `en` indexes switch together. This bounds serving memory; see
[Serverless deployment](serverless.md#memory).

A sync switches indexes one group at a time (by default each index is its own group). So while a sync
runs, a query over several layers can see one index at the new version and another still at the previous
one. Put indexes that are searched together in one group when that matters.
