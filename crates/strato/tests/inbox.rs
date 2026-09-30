#![cfg(feature = "store")]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use strato::{Batch, Dataset, Document, Error, IndexConfig, Writer, WriterStep};

async fn datasets(dir: &tempfile::TempDir, name: &str) -> Vec<Dataset> {
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut urls = vec![
        format!("memory:///{name}"),
        dir.path().join(name).to_string_lossy().into_owned(),
    ];
    if let Ok(base) = std::env::var("STRATO_TEST_S3_URL") {
        urls.push(format!("{}/{name}-{run}", base.trim_end_matches('/')));
    }
    let mut out = Vec::new();
    for url in urls {
        out.push(
            Dataset::open(&url, Vec::<(String, String)>::new())
                .await
                .unwrap(),
        );
    }
    out
}

async fn wipe(dataset: &Dataset) {
    for key in dataset.store().list("").await.unwrap() {
        dataset.store().delete(&key).await.unwrap();
    }
}

fn doc(id: u64, text: &str) -> Document {
    Document::new(id, text, 0.5)
}

async fn text_of(ds: &Dataset, index: &str, id: u64) -> Option<String> {
    let manifest = ds.latest().await.unwrap();
    let index = ds
        .load_index(&manifest, index, IndexConfig::default())
        .await
        .unwrap();
    index.document(id).map(|d| d.text)
}

#[tokio::test(flavor = "multi_thread")]
async fn batches_fold_into_one_commit() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "fold").await {
        let mut first = Batch::new();
        first
            .upsert(
                "default/en",
                [doc(1, "python"), doc(2, "rust"), doc(3, "go")],
            )
            .upsert("acme/en", [doc(1, "python custom")]);
        ds.submit(first).await.unwrap();
        let mut second = Batch::new();
        second
            .upsert("default/en", [doc(2, "rust lang")])
            .delete("default/en", [3]);
        ds.submit(second).await.unwrap();
        assert_eq!(ds.pending_batches().await.unwrap(), 2);

        let mut writer = Writer::new(ds.clone(), "a");
        let step = writer.run_once().await.unwrap();
        assert_eq!(
            step,
            WriterStep::Committed {
                version: 1,
                batches: 2,
                documents: 5
            }
        );
        assert_eq!(ds.pending_batches().await.unwrap(), 0);
        assert_eq!(
            text_of(&ds, "default/en", 2).await.as_deref(),
            Some("rust lang")
        );
        assert_eq!(text_of(&ds, "default/en", 3).await, None);
        assert_eq!(
            text_of(&ds, "acme/en", 1).await.as_deref(),
            Some("python custom")
        );
        assert_eq!(
            ds.latest().await.unwrap().indexes["default/en"]
                .segments
                .len(),
            1
        );
        assert_eq!(writer.run_once().await.unwrap(), WriterStep::Idle);
        writer.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_leader_with_failover() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "leader").await {
        let mut a = Writer::new(ds.clone(), "a");
        let mut b = Writer::new(ds.clone(), "b");
        assert_eq!(a.run_once().await.unwrap(), WriterStep::Idle);
        assert_eq!(b.run_once().await.unwrap(), WriterStep::NotLeader);
        a.release().await.unwrap();
        assert_eq!(b.run_once().await.unwrap(), WriterStep::Idle);
        assert!(b.is_leader());

        // A writer whose lease expired steps down at its next round.
        let mut c = Writer::new(ds.clone(), "c");
        c.lease_ttl = Duration::from_millis(300);
        b.release().await.unwrap();
        assert_eq!(c.run_once().await.unwrap(), WriterStep::Idle);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut d = Writer::new(ds.clone(), "d");
        assert_eq!(d.run_once().await.unwrap(), WriterStep::Idle);
        assert_eq!(c.run_once().await.unwrap(), WriterStep::NotLeader);
        d.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leftover_applied_batches_are_not_reapplied() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "leftover").await {
        let mut old = Batch::new();
        old.upsert("i", [doc(7, "first")]);
        let id = ds.submit(old).await.unwrap();
        let key = format!("_inbox/{id}.batch");
        let bytes = ds.store().get(&key).await.unwrap();
        let mut writer = Writer::new(ds.clone(), "w");
        assert!(matches!(
            writer.run_once().await.unwrap(),
            WriterStep::Committed { .. }
        ));

        // As if the writer crashed after committing, before deleting the batch.
        ds.store().put(&key, bytes).await.unwrap();
        let mut newer = Batch::new();
        newer.upsert("i", [doc(7, "second")]);
        ds.submit(newer).await.unwrap();
        assert!(matches!(
            writer.run_once().await.unwrap(),
            WriterStep::Committed { batches: 1, .. }
        ));
        assert_eq!(text_of(&ds, "i", 7).await.as_deref(), Some("second"));
        assert_eq!(ds.pending_batches().await.unwrap(), 0);
        writer.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn older_generations_are_fenced() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "fence").await {
        let mut newer = ds.begin().await.unwrap();
        newer
            .append_documents("i", [doc(1, "a")], [])
            .unwrap()
            .fence("writer", 5);
        newer.commit().await.unwrap();
        let mut older = ds.begin().await.unwrap();
        older
            .append_documents("i", [doc(2, "b")], [])
            .unwrap()
            .fence("writer", 4);
        assert!(matches!(older.commit().await, Err(Error::Conflict(_))));
        let mut same = ds.begin().await.unwrap();
        same.append_documents("i", [doc(3, "c")], [])
            .unwrap()
            .fence("writer", 5);
        same.commit().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn many_producers_and_competing_writers() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "stress").await {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut writers = Vec::new();
        for w in 0..4 {
            let (ds, stop) = (ds.clone(), stop.clone());
            writers.push(tokio::spawn(async move {
                let mut writer = Writer::new(ds, format!("writer-{w}"));
                let mut committed = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    match writer.run_once().await {
                        Ok(WriterStep::Committed { batches, .. }) => committed += batches,
                        Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                        Err(Error::Conflict(_)) => {}
                        Err(e) => panic!("{e}"),
                    }
                }
                writer.release().await.unwrap();
                committed
            }));
        }
        let mut producers = Vec::new();
        for p in 0..8u64 {
            let ds = ds.clone();
            producers.push(tokio::spawn(async move {
                for n in 0..10u64 {
                    let mut batch = Batch::new();
                    batch.upsert(
                        "i",
                        [
                            doc(1000 + p * 100 + n, "item"),
                            doc(p, &format!("hot {p} {n}")),
                        ],
                    );
                    ds.submit(batch).await.unwrap();
                }
            }));
        }
        for p in producers {
            p.await.unwrap();
        }
        while ds.pending_batches().await.unwrap() > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut total = 0;
        for w in writers {
            total += w.await.unwrap();
        }
        assert_eq!(total, 80);
        let manifest = ds.latest().await.unwrap();
        let index = ds
            .load_index(&manifest, "i", IndexConfig::default())
            .await
            .unwrap();
        assert_eq!(index.len(), 88);
        for p in 0..8u64 {
            assert_eq!(index.document(p).unwrap().text, format!("hot {p} 9"));
        }
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_batches_are_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    for ds in datasets(&dir, "corrupt").await {
        let mut good = Batch::new();
        good.upsert("i", [doc(1, "kept")]);
        let id = ds.submit(good).await.unwrap();
        let key = format!("_inbox/{id}.batch");
        let mut bytes = ds.store().get(&key).await.unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        ds.store()
            .put("_inbox/0000000000000000-corrupt.batch", bytes)
            .await
            .unwrap();

        let mut writer = Writer::new(ds.clone(), "w");
        assert!(matches!(
            writer.run_once().await.unwrap(),
            WriterStep::Committed { batches: 1, .. }
        ));
        assert_eq!(text_of(&ds, "i", 1).await.as_deref(), Some("kept"));
        assert_eq!(ds.pending_batches().await.unwrap(), 0);
        assert_eq!(ds.store().list("_rejected").await.unwrap().len(), 1);
        writer.release().await.unwrap();
        wipe(&ds).await;
    }
}
