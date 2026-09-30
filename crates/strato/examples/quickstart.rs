//! The README's first example: `cargo run --example quickstart`

use std::sync::Arc;

use strato::{AliasKind, Document, Engine, Index, IndexConfig, Segment};

fn main() -> Result<(), strato::Error> {
    let docs = vec![
        Document::new(1, "Machine Learning", 0.9).with_alias("ML", AliasKind::Abbreviation),
        Document::new(2, "Machine Vision", 0.4),
        Document::new(3, "Data Science", 0.7).with_alias("data analytics", AliasKind::Synonym),
    ];
    let index = Index::new(
        vec![Arc::new(Segment::build(docs, [])?)],
        IndexConfig::default(),
    )?;

    for query in ["mach", "ML", "vison", "science"] {
        let hits: Vec<_> = index
            .autocomplete(query, 3)
            .iter()
            .map(|h| format!("{} {} {:.3}", h.id, h.kind.as_str(), h.score))
            .collect();
        println!("{query:>8} -> {hits:?}");
    }

    // Layers: a tenant's index overrides the shared one per id.
    let engine = Engine::new();
    let tenant = Segment::build([Document::new(2, "Machine Vision Systems", 1.0)], [])?;
    engine.publish([
        ("shared".to_owned(), Some(Arc::new(index))),
        (
            "acme".to_owned(),
            Some(Arc::new(Index::new(
                vec![Arc::new(tenant)],
                IndexConfig::default(),
            )?)),
        ),
    ]);
    for hit in engine.autocomplete(&["shared", "acme"], "machine", 3) {
        println!("{} from layer {}", hit.hit.id, hit.layer);
    }
    Ok(())
}
