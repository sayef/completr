//! A database with streamed updates: `cargo run --example live_updates --features store -- [url]`
//! The url may be a local path, `s3://bucket/prefix`, `gs://...`, `az://...` or `memory:///name`.

use strato::{ChangeSet, Database, Document, Engine, IndexConfig, IngestStep, Ingestor, Replica};

#[tokio::main]
async fn main() -> Result<(), strato::Error> {
    let dir = tempfile::tempdir()?;
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| dir.path().to_string_lossy().into_owned());
    let database = Database::open(&url, Vec::<(String, String)>::new()).await?;

    // Bulk load in one transaction.
    let mut txn = database.begin().await?;
    txn.append_documents(
        "products",
        (0..10_000).map(|i| Document::new(i, format!("product {i}"), 0.5)),
        [],
    )?;
    txn.commit().await?;

    // Serving processes follow the database and publish new versions atomically.
    let engine = Engine::new();
    let replica = Replica::new(database.clone(), IndexConfig::default());
    replica.sync(&engine).await?;

    // Any process submits change sets; the one holding the ingestor lease commits them.
    let mut changes = ChangeSet::new();
    changes
        .upsert(
            "products",
            [Document::new(10_000, "wireless keyboard", 0.9)],
        )
        .delete("products", [42]);
    database.submit(changes).await?;
    let mut ingestor = Ingestor::new(database.clone(), "ingestor-1");
    if let IngestStep::Committed {
        version,
        change_sets,
        ..
    } = ingestor.run_once().await?
    {
        println!("committed {change_sets} change set(s) as version {version}");
    }
    ingestor.release().await?;

    replica.sync(&engine).await?;
    for hit in engine.complete(&["products"], "wirel", 5) {
        println!(
            "{} {} {:.3}",
            hit.hit.id,
            hit.hit.kind.as_str(),
            hit.hit.score
        );
    }
    Ok(())
}
