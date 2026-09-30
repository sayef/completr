//! The README's first example: `cargo run --example quickstart`

use std::sync::Arc;

use completr::{Document, Engine, Index};

fn main() -> Result<(), completr::Error> {
    let index = Index::from_documents([
        Document::keyed("ml", "Machine Learning", 0.9).with_abbreviation("ML"),
        Document::keyed("mv", "Machine Vision", 0.4),
        Document::keyed("ds", "Data Science", 0.7).with_synonym("data analytics"),
    ])?;

    for query in ["mach", "ML", "vison", "science"] {
        let hits: Vec<_> = index
            .complete(query, 3)
            .iter()
            .map(|s| format!("{} {} {:.3}", s.text, s.kind.as_str(), s.score))
            .collect();
        println!("{query:>8} -> {hits:?}");
    }
    let top = &index.complete("mach", 1)[0];
    println!(
        "matched: {:?}",
        top.highlights
            .iter()
            .map(|r| &top.text[r.clone()])
            .collect::<Vec<_>>()
    );

    // Layers: a tenant's index overrides the shared one per id.
    let engine = Engine::new();
    let tenant = Index::from_documents([Document::keyed("mv", "Machine Vision Systems", 1.0)])?;
    engine.publish([
        ("shared".to_owned(), Some(Arc::new(index))),
        ("acme".to_owned(), Some(Arc::new(tenant))),
    ]);
    for layered in engine.complete(&["shared", "acme"], "machine", 3) {
        println!("{} from layer {}", layered.suggestion.text, layered.layer);
    }
    Ok(())
}
