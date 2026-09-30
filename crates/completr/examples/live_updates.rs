//! A database with streamed updates: `cargo run --example live_updates --features store -- [url]`
//! The url may be a local path, `s3://bucket/prefix`, `gs://...`, `az://...` or `memory:///name`.

use completr::{
    ChangeSet, Database, Document, Engine, IndexOptions, IngestStep, Ingestor, Replica,
    SearchOptions,
};

#[tokio::main]
async fn main() -> Result<(), completr::Error> {
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
    let replica = Replica::new(database.clone(), IndexOptions::default());
    replica.sync(&engine).await?;

    // Any process submits change sets; the one holding the ingestor lease commits them.
    let mut changes = ChangeSet::new();
    changes
        .upsert(
            "products",
            [Document::keyed("kb-1", "Wireless Keyboard", 0.9).with_context("peripherals")],
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
    let peripherals = SearchOptions::new(5).contexts(["peripherals"]);
    for layered in engine.complete_with(&["products"], "wirel", &peripherals) {
        let s = layered.suggestion;
        println!("{:?} {} {} {:.3}", s.key, s.text, s.kind.as_str(), s.score);
    }
    Ok(())
}
