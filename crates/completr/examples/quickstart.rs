//! The README's first example: `cargo run --example quickstart --features store`

use std::sync::Arc;
use std::time::Duration;

use completr::{Database, Document, Engine, Index, IndexOptions, Replica};

fn catalogue() -> Vec<Document> {
    vec![
        Document::keyed("bohemian", "Bohemian Rhapsody – Queen", 0.95)
            .with_synonym("is this the real life"),
        Document::keyed("dancing", "Dancing Queen – ABBA", 0.85),
        Document::keyed("dark", "Dancing in the Dark – Bruce Springsteen", 0.7),
        Document::keyed("bridge", "Under the Bridge – Red Hot Chili Peppers", 0.75)
            .with_abbreviation("RHCP"),
    ]
}

#[tokio::main]
async fn main() -> Result<(), completr::Error> {
    // An index in memory.
    let index = Index::from_documents(catalogue())?;
    for query in ["danc", "RHCP", "bohemain rapsody", "queen"] {
        let hits: Vec<_> = index
            .complete(query, 3)
            .iter()
            .map(|s| format!("{} {} {:.3}", s.text, s.kind.as_str(), s.score))
            .collect();
        println!("{query:>16} -> {hits:?}");
    }
    let top = &index.complete("danc", 10)[0];
    let matched: Vec<&str> = top
        .highlights
        .iter()
        .map(|r| &top.text[r.clone()])
        .collect();
    println!("matched: {matched:?}");

    // Synonyms are searched on their own.
    for s in index.complete_aliases("is this the real", 10) {
        println!("{:?} {} {:.3}", s.key, s.text, s.score);
    }

    // A database: a transaction commits a version, and an engine follows the database.
    let database = Database::open("memory://", Vec::<(String, String)>::new()).await?;
    let mut txn = database.begin().await?;
    txn.append_documents("songs", catalogue(), [])?;
    let manifest = txn.commit().await?;
    println!("committed version {}", manifest.version);

    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database.clone(), IndexOptions::default()));
    replica.sync(&engine).await?;
    let _follower = replica.follow(&engine, Duration::from_secs(5));
    for layered in engine.complete(&["songs"], "danc", 10) {
        let s = layered.suggestion;
        println!("{} {} {:.3}", s.text, s.kind.as_str(), s.score);
    }
    Ok(())
}
