//! A dataset with streamed updates: `cargo run --example live_updates --features store -- [url]`
//! The url may be a local path, `s3://bucket/prefix`, `gs://...`, `az://...` or `memory:///name`.

use strato::{Batch, Dataset, Document, Engine, Follower, IndexConfig, Writer, WriterStep};

#[tokio::main]
async fn main() -> Result<(), strato::Error> {
    let dir = tempfile::tempdir()?;
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| dir.path().to_string_lossy().into_owned());
    let dataset = Dataset::open(&url, Vec::<(String, String)>::new()).await?;

    // Bulk load in one transaction.
    let mut txn = dataset.begin().await?;
    txn.append_documents(
        "products",
        (0..10_000).map(|i| Document::new(i, format!("product {i}"), 0.5)),
        [],
    )?;
    txn.commit().await?;

    // Serving processes follow the dataset and publish new versions atomically.
    let engine = Engine::new();
    let follower = Follower::new(dataset.clone(), IndexConfig::default());
    follower.sync(&engine).await?;

    // Any process submits batches; the one holding the writer lease commits them.
    let mut batch = Batch::new();
    batch
        .upsert(
            "products",
            [Document::new(10_000, "wireless keyboard", 0.9)],
        )
        .delete("products", [42]);
    dataset.submit(batch).await?;
    let mut writer = Writer::new(dataset.clone(), "writer-1");
    if let WriterStep::Committed {
        version, batches, ..
    } = writer.run_once().await?
    {
        println!("committed {batches} batch(es) as version {version}");
    }
    writer.release().await?;

    follower.sync(&engine).await?;
    for hit in engine.autocomplete(&["products"], "wirel", 5) {
        println!(
            "{} {} {:.3}",
            hit.hit.id,
            hit.hit.kind.as_str(),
            hit.hit.score
        );
    }
    Ok(())
}
