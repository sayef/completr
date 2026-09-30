#![cfg(feature = "store")]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use strato::{ChangeSet, Database, Document, Error, IndexConfig, IngestStep, Ingestor};

async fn databases(dir: &tempfile::TempDir, name: &str) -> Vec<Database> {
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
            Database::open(&url, Vec::<(String, String)>::new())
                .await
                .unwrap(),
        );
    }
    out
}

async fn wipe(database: &Database) {
    for key in database.store().list("").await.unwrap() {
        database.store().delete(&key).await.unwrap();
    }
}

fn doc(id: u64, text: &str) -> Document {
    Document::new(id, text, 0.5)
}

async fn text_of(ds: &Database, index: &str, id: u64) -> Option<String> {
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
    for ds in databases(&dir, "fold").await {
        let mut first = ChangeSet::new();
        first
            .upsert(
                "default/en",
                [doc(1, "python"), doc(2, "rust"), doc(3, "go")],
            )
            .upsert("acme/en", [doc(1, "python custom")]);
        ds.submit(first).await.unwrap();
        let mut second = ChangeSet::new();
        second
            .upsert("default/en", [doc(2, "rust lang")])
            .delete("default/en", [3]);
        ds.submit(second).await.unwrap();
        assert_eq!(ds.pending_change_sets().await.unwrap(), 2);

        let mut ingestor = Ingestor::new(ds.clone(), "a");
        let step = ingestor.run_once().await.unwrap();
        assert_eq!(
            step,
            IngestStep::Committed {
                version: 1,
                change_sets: 2,
                documents: 5
            }
        );
        assert_eq!(ds.pending_change_sets().await.unwrap(), 0);
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
        assert_eq!(ingestor.run_once().await.unwrap(), IngestStep::Idle);
        ingestor.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_leader_with_failover() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "leader").await {
        let mut a = Ingestor::new(ds.clone(), "a");
        let mut b = Ingestor::new(ds.clone(), "b");
        assert_eq!(a.run_once().await.unwrap(), IngestStep::Idle);
        assert_eq!(b.run_once().await.unwrap(), IngestStep::Standby);
        a.release().await.unwrap();
        assert_eq!(b.run_once().await.unwrap(), IngestStep::Idle);
        assert!(b.is_active());

        // A ingestor whose lease expired steps down at its next round.
        let mut c = Ingestor::new(ds.clone(), "c");
        c.lease_ttl = Duration::from_millis(300);
        b.release().await.unwrap();
        assert_eq!(c.run_once().await.unwrap(), IngestStep::Idle);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut d = Ingestor::new(ds.clone(), "d");
        assert_eq!(d.run_once().await.unwrap(), IngestStep::Idle);
        assert_eq!(c.run_once().await.unwrap(), IngestStep::Standby);
        d.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leftover_applied_batches_are_not_reapplied() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "leftover").await {
        let mut old = ChangeSet::new();
        old.upsert("i", [doc(7, "first")]);
        let id = ds.submit(old).await.unwrap();
        let key = format!("_inbox/{id}.batch");
        let bytes = ds.store().get(&key).await.unwrap();
        let mut ingestor = Ingestor::new(ds.clone(), "w");
        assert!(matches!(
            ingestor.run_once().await.unwrap(),
            IngestStep::Committed { .. }
        ));

        // As if the ingestor crashed after committing, before deleting the batch.
        ds.store().put(&key, bytes).await.unwrap();
        let mut newer = ChangeSet::new();
        newer.upsert("i", [doc(7, "second")]);
        ds.submit(newer).await.unwrap();
        assert!(matches!(
            ingestor.run_once().await.unwrap(),
            IngestStep::Committed { change_sets: 1, .. }
        ));
        assert_eq!(text_of(&ds, "i", 7).await.as_deref(), Some("second"));
        assert_eq!(ds.pending_change_sets().await.unwrap(), 0);
        ingestor.release().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn older_generations_are_fenced() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "fence").await {
        let mut newer = ds.begin().await.unwrap();
        newer
            .append_documents("i", [doc(1, "a")], [])
            .unwrap()
            .fence("ingestor", 5);
        newer.commit().await.unwrap();
        let mut older = ds.begin().await.unwrap();
        older
            .append_documents("i", [doc(2, "b")], [])
            .unwrap()
            .fence("ingestor", 4);
        assert!(matches!(older.commit().await, Err(Error::Conflict(_))));
        let mut same = ds.begin().await.unwrap();
        same.append_documents("i", [doc(3, "c")], [])
            .unwrap()
            .fence("ingestor", 5);
        same.commit().await.unwrap();
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn many_producers_and_competing_writers() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "stress").await {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut writers = Vec::new();
        for w in 0..4 {
            let (ds, stop) = (ds.clone(), stop.clone());
            writers.push(tokio::spawn(async move {
                let mut ingestor = Ingestor::new(ds, format!("ingestor-{w}"));
                let mut committed = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    match ingestor.run_once().await {
                        Ok(IngestStep::Committed { change_sets, .. }) => committed += change_sets,
                        Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                        Err(Error::Conflict(_)) => {}
                        Err(e) => panic!("{e}"),
                    }
                }
                ingestor.release().await.unwrap();
                committed
            }));
        }
        let mut producers = Vec::new();
        for p in 0..8u64 {
            let ds = ds.clone();
            producers.push(tokio::spawn(async move {
                for n in 0..10u64 {
                    let mut batch = ChangeSet::new();
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
        while ds.pending_change_sets().await.unwrap() > 0 {
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
    for ds in databases(&dir, "corrupt").await {
        let mut good = ChangeSet::new();
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

        let mut ingestor = Ingestor::new(ds.clone(), "w");
        assert!(matches!(
            ingestor.run_once().await.unwrap(),
            IngestStep::Committed { change_sets: 1, .. }
        ));
        assert_eq!(text_of(&ds, "i", 1).await.as_deref(), Some("kept"));
        assert_eq!(ds.pending_change_sets().await.unwrap(), 0);
        assert_eq!(ds.store().list("_rejected").await.unwrap().len(), 1);
        ingestor.release().await.unwrap();
        wipe(&ds).await;
    }
}
