# Layers

An `Engine` holds indexes by name and publishes new versions atomically. A search names a list of
indexes, its **layers**. Later layers override earlier ones per document id, so a tenant's edits,
deletions and additions shadow the shared data without copying it.

## Publishing and searching

=== "Python"

    ```python
    from completr import Engine, Index

    shared = Index.from_documents([
        {"id": "ml", "text": "Machine Learning", "popularity": 0.9},
        {"id": "mv", "text": "Machine Vision", "popularity": 0.4},
    ])
    acme = Index.from_documents([
        {"id": "mv", "text": "Machine Vision Systems", "popularity": 1.0},   # overrides "mv"
        {"id": "mt", "text": "Machine Translation", "popularity": 0.5},      # adds a document
    ])

    engine = Engine()
    engine.publish({"shared": shared, "acme": acme})
    for s in engine.complete("machine", ["shared", "acme"], limit=10):
        print(s.text, s.layer)
    ```

=== "Rust"

    ```rust
    use std::sync::Arc;
    use completr::{Document, Engine, Index};

    let engine = Engine::new();
    engine.publish([
        ("shared".to_owned(), Some(Arc::new(shared))),
        ("acme".to_owned(), Some(Arc::new(acme))),
    ]);
    for layered in engine.complete(&["shared", "acme"], "machine", 10) {
        println!("{} from layer {}", layered.suggestion.text, layered.layer);
    }
    ```

```text
Machine Learning shared
Machine Vision Systems acme
Machine Translation acme
```

`suggestion.layer` names the layer each suggestion came from. In Rust, the engine returns
`LayeredSuggestion`s whose `layer` is the position in the layer list.

- A layer name that is not published counts as an empty layer, so every tenant can use the same layer
  list whether or not it has overrides.
- `publish` replaces the named indexes atomically. A search already running keeps the versions it started
  with.
- `engine.publish({"acme": None})` removes a layer.
- `engine.get(name)` returns a published `Index`, and `engine.names()` lists them.

## How overrides apply

Each layer is searched on its own, and the results are merged by score. A later layer overrides an
earlier one for every id it holds or deletes, whether or not its own version matches the query:

- **Edits**: if the tenant renames a shared document, the shared version no longer appears, even for
  queries only the old text matched.
- **Deletions**: delete an id in the tenant's index (`ChangeSet.delete` or `deletes=` on a segment) and the
  shared document disappears for that tenant.
- **Additions**: documents that exist only in the tenant's index are completed alongside the shared ones.

```python
changes = completr.ChangeSet()
changes.upsert("acme", [{"id": "mv", "text": "Computer Vision"}])   # rename for acme only
changes.delete("acme", ["mt"])                                      # hide for acme only
db.submit(changes)
```

## Overfetch

A layered search takes `limit * overfetch` candidates from each layer before overrides are applied, so
that overridden documents do not leave the result short. The default, 2, suits layers that override a
small share of the documents. Raise it for layers that shadow many results.

```python
engine = Engine(overfetch=4)
```

## Naming layers in a database

The library has no notion of tenants or languages. Express them as index names, such as `shared/en` and
`acme/en`, and as layer lists. A database engine serves every index of the database by name:

```python
import tempfile
import completr

db = completr.connect(tempfile.mkdtemp())
txn = db.begin()
txn.append("shared/en", [{"id": "ml", "text": "Machine Learning", "popularity": 0.9}])
txn.append("acme/en", [{"id": "ml", "text": "Machine Learning at Acme", "popularity": 0.9}])
txn.commit()

engine = db.engine(group_separator="/")
print([(s.text, s.layer) for s in engine.complete("mach", ["shared/en", "acme/en"])])
```

With `group_separator="/"`, the replica behind the engine switches indexes group by group, grouping by the
part after the last separator (here the language), and releases replaced segments before it loads the next
group. See [Serverless deployment](serverless.md#memory).
