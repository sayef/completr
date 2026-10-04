//! A database with streamed updates: `cargo run --example live_updates --features store -- [url]`
//! The url may be a local path, `s3://bucket/prefix`, `gs://...`, `az://...` or `memory:///name`.

use std::sync::Arc;
use std::time::Duration;

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
        "songs",
        (0..10_000).map(|i| Document::new(i, format!("song {i}"), 0.5)),
        [],
    )?;
    txn.commit().await?;

    // A serving process: the follower keeps the engine on the latest version until it is dropped.
    let engine = Arc::new(Engine::new());
    let replica = Arc::new(Replica::new(database.clone(), IndexOptions::default()));
    replica.sync(&engine).await?;
    let follower = replica.follow(&engine, Duration::from_millis(200));

    // Any process submits change sets; the one holding the ingestor lease commits them.
    let mut changes = ChangeSet::new();
    changes
        .upsert(
            "songs",
            [Document::keyed("billie", "Billie Jean – Michael Jackson", 0.9).with_context("pop")],
        )
        .delete("songs", [42]);
    database.submit(changes).await?;
    let mut ingestor = Ingestor::new(database.clone(), "ingestor-1");
    let mut committed = 0;
    if let IngestStep::Committed {
        version,
        change_sets,
        ..
    } = ingestor.run_once().await?
    {
        println!("committed {change_sets} change set(s) as version {version}");
        committed = version;
    }
    ingestor.release().await?;

    while replica.version().await < committed {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!(
        "served version {}, sync error: {:?}",
        replica.version().await,
        follower.status().error
    );
    let pop = SearchOptions::new(5).contexts(["pop"]);
    for layered in engine.complete_with(&["songs"], "bill", &pop) {
        let s = layered.suggestion;
        println!("{:?} {} {} {:.3}", s.key, s.text, s.kind.as_str(), s.score);
    }
    Ok(())
}
