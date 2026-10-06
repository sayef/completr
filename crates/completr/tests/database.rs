#![cfg(feature = "store")]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use completr::{
    CleanupPolicy, CompactionPolicy, Database, Document, Engine, Error, Index, IndexOptions,
    Manifest, Replica, Segment, BASE_LEVEL,
};

/// A fresh database per backend: memory, a temp dir, and S3 when `COMPLETR_TEST_S3_URL` is set.
async fn databases(dir: &tempfile::TempDir, name: &str) -> Vec<Database> {
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut urls = vec![
        format!("memory:///{name}"),
        dir.path().join(name).to_string_lossy().into_owned(),
    ];
    if let Ok(base) = std::env::var("COMPLETR_TEST_S3_URL") {
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

fn docs(ids: impl IntoIterator<Item = u64>, tag: &str) -> Vec<Document> {
    const WORDS: [&str; 8] = [
        "data", "science", "rust", "python", "cloud", "design", "machine", "learning",
    ];
    ids.into_iter()
        .map(|i| {
            let text = format!(
                "{} {} {tag}{}",
                WORDS[i as usize % 8],
                WORDS[(i as usize / 8) % 8],
                i % 5
            );
            Document::new(i, text, (i % 10) as f32 / 10.0)
        })
        .collect()
}

fn queries() -> Vec<&'static str> {
    vec![
        "d", "da", "dat", "data", "data sci", "rust p", "pyhton", "machne", "clou", "learning",
        "design", "x",
    ]
}

fn results(index: &Index) -> Vec<Vec<(u64, u64, &'static str)>> {
    queries()
        .into_iter()
        .map(|q| {
            index
                .complete(q, 20)
                .into_iter()
                .map(|h| (h.id, h.score.to_bits(), h.kind.as_str()))
                .collect()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_rebase_and_conflict() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "commits").await {
        let mut t = ds.begin().await.unwrap();
        t.append_documents("a", docs(0..100, "v"), [])
            .unwrap()
            .set_max_score("a", 700.0)
            .set_metadata("cursor", Some("1"));
        let v1 = t.commit().await.unwrap();
        assert_eq!(
            (
                v1.version,
                v1.parent,
                v1.namespaces["default"]["a"].max_score
            ),
            (1, None, 700.0)
        );

        let mut first = ds.transaction(v1.clone());
        let mut second = ds.transaction(v1.clone());
        let mut strict = ds.transaction(v1.clone());
        let mut overwrite = ds.transaction(v1.clone());
        first
            .append_documents("a", docs(100..110, "w"), [3])
            .unwrap();
        second
            .append_documents("a", docs(110..120, "w"), [])
            .unwrap()
            .append_documents("b", docs(0..5, "b"), [])
            .unwrap();
        strict
            .append_documents("a", docs(0..1, "s"), [])
            .unwrap()
            .strict(true);
        overwrite.overwrite("a", Segment::build(docs(0..10, "o"), []).unwrap());

        assert_eq!(first.commit().await.unwrap().version, 2);
        let v3 = second.commit().await.unwrap();
        assert_eq!((v3.version, v3.parent), (3, Some(2)));
        assert_eq!(v3.namespaces["default"]["a"].segments.len(), 3);
        assert_eq!(v3.metadata["cursor"], "1");
        assert!(matches!(strict.commit().await, Err(Error::Conflict(_))));
        assert!(matches!(overwrite.commit().await, Err(Error::Conflict(_))));

        let index = ds
            .open_index(&v3, "a", IndexOptions::default())
            .await
            .unwrap();
        assert_eq!(index.len(), 119);
        assert_eq!(ds.versions().await.unwrap(), [1, 2, 3]);
        assert_eq!(ds.latest_version_after(1).await.unwrap(), Some(3));
        assert_eq!(ds.latest_version_after(3).await.unwrap(), None);
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn compaction_preserves_results() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "compaction").await {
        let mut t = ds.begin().await.unwrap();
        t.append_documents("a", docs(0..400, "base"), [])
            .unwrap()
            .set_max_score("a", 700.0);
        t.commit().await.unwrap();
        for round in 0..5u64 {
            let mut t = ds.begin().await.unwrap();
            let ids = (round * 37..round * 37 + 30).chain(400 + round * 10..410 + round * 10);
            t.append_documents("a", docs(ids, &format!("r{round}")), [round * 50 + 7, 405])
                .unwrap();
            t.commit().await.unwrap();
        }
        let before = ds.latest().await.unwrap();
        let expected = results(
            &ds.open_index(&before, "a", IndexOptions::default())
                .await
                .unwrap(),
        );

        let policy = CompactionPolicy::default()
            .fanout(4)
            .max_segments(16)
            .max_hidden_fraction(1.0);
        let tiered = ds
            .compact("a", &policy)
            .await
            .unwrap()
            .expect("five level-0 segments are due");
        let levels: Vec<u32> = tiered.namespaces["default"]["a"]
            .segments
            .iter()
            .map(|s| s.level)
            .collect();
        assert_eq!(levels, [BASE_LEVEL, 1]);
        assert_eq!(
            results(
                &ds.open_index(&tiered, "a", IndexOptions::default())
                    .await
                    .unwrap()
            ),
            expected
        );
        assert!(ds.compact("a", &policy).await.unwrap().is_none());

        let full = ds
            .compact("a", &policy.max_segments(1))
            .await
            .unwrap()
            .unwrap();
        let entry = &full.namespaces["default"]["a"];
        assert_eq!(
            (
                entry.segments.len(),
                entry.segments[0].level,
                entry.segments[0].deletes
            ),
            (1, BASE_LEVEL, 0)
        );
        assert_eq!(
            results(
                &ds.open_index(&full, "a", IndexOptions::default())
                    .await
                    .unwrap()
            ),
            expected
        );

        let stats = ds
            .cleanup(
                &CleanupPolicy::default()
                    .keep_versions(1)
                    .older_than(Duration::ZERO),
            )
            .await
            .unwrap();
        assert_eq!(
            (stats.versions_removed, stats.segments_removed),
            (full.version as usize - 1, 7)
        );
        assert_eq!(ds.versions().await.unwrap(), [full.version]);
        assert_eq!(
            results(
                &ds.open_index(&ds.latest().await.unwrap(), "a", IndexOptions::default())
                    .await
                    .unwrap()
            ),
            expected
        );
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_writers_and_compactor_converge() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "concurrent").await {
        let mut t = ds.begin().await.unwrap();
        t.append_documents("a", docs(0..200, "base"), [])
            .unwrap()
            .set_max_score("a", 700.0);
        t.commit().await.unwrap();

        let mut tasks = Vec::new();
        for writer in 0..4u64 {
            let ds = ds.clone();
            tasks.push(tokio::spawn(async move {
                for batch in 0..4u64 {
                    let first = 1000 + writer * 100 + batch * 10;
                    let mut t = ds.begin().await.unwrap();
                    t.append_documents("a", docs(first..first + 10, "w"), [writer * 20 + batch])
                        .unwrap();
                    t.commit().await.unwrap();
                }
            }));
        }
        let compactor = {
            let ds = ds.clone();
            tokio::spawn(async move {
                let policy = CompactionPolicy::default().fanout(2);
                let mut done = 0;
                for _ in 0..12 {
                    match ds.compact("a", &policy).await {
                        Ok(Some(_)) => done += 1,
                        Ok(None) | Err(Error::Conflict(_)) => {}
                        Err(e) => panic!("{e}"),
                    }
                }
                done
            })
        };
        for task in tasks {
            task.await.unwrap();
        }
        let compactions = compactor.await.unwrap();

        let manifest = ds.latest().await.unwrap();
        let index = ds
            .open_index(&manifest, "a", IndexOptions::default())
            .await
            .unwrap();
        let mut expected: Vec<Document> = docs(0..200, "base");
        for writer in 0..4u64 {
            for batch in 0..4u64 {
                let first = 1000 + writer * 100 + batch * 10;
                expected.retain(|d| d.id != writer * 20 + batch);
                expected.extend(docs(first..first + 10, "w"));
            }
        }
        let truth = Index::new(
            vec![Arc::new(Segment::build(expected, []).unwrap())],
            IndexOptions::default().max_score(700.0),
        )
        .unwrap();
        assert_eq!(index.len(), truth.len());
        assert_eq!(results(&index), results(&truth));
        assert_eq!(manifest.version as usize, 1 + 16 + compactions);
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leases_exclude_and_expire() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "leases").await {
        let hour = Duration::from_secs(3600);
        let mut a = ds
            .acquire_lease("writer", "a", hour)
            .await
            .unwrap()
            .expect("free");
        assert_eq!(a.generation(), 1);
        assert!(ds
            .acquire_lease("writer", "b", hour)
            .await
            .unwrap()
            .is_none());
        assert!(a.renew(hour).await.unwrap());
        assert_eq!(a.generation(), 2);
        a.release().await.unwrap();

        let b = ds
            .acquire_lease("writer", "b", Duration::ZERO)
            .await
            .unwrap()
            .expect("released");
        assert_eq!(b.generation(), 4);
        let mut c = ds
            .acquire_lease("writer", "c", hour)
            .await
            .unwrap()
            .expect("expired");
        assert_eq!(c.generation(), 5);
        assert!(c.renew(hour).await.unwrap());
        drop(b);
        wipe(&ds).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn replica_loads_only_changes() {
    let dir = tempfile::tempdir().unwrap();
    for ds in databases(&dir, "replica").await {
        let engine = Engine::new();
        let replica = Replica::new(ds.clone(), IndexOptions::default());
        assert_eq!(replica.sync(&engine).await.unwrap(), None);

        let en = ds.namespace("en").unwrap();
        let mut t = en.begin().await.unwrap();
        t.append_documents("base", docs(0..50, "d"), []).unwrap();
        t.append_documents("acme", docs(0..5, "acme"), []).unwrap();
        t.namespace("de")
            .append_documents("base", docs(0..3, "d"), [])
            .unwrap();
        t.commit().await.unwrap();
        assert_eq!(replica.sync(&engine).await.unwrap(), Some(1));
        assert_eq!(engine.namespaces(), ["de", "en"]);
        let held = engine.namespace("en").unwrap();
        assert_eq!(
            (held.names(), held.version()),
            (vec!["acme".to_owned(), "base".to_owned()], 1)
        );
        let base_before = held.get("base").unwrap();

        let mut t = en.begin().await.unwrap();
        t.append_documents("acme", docs(5..8, "acme"), []).unwrap();
        t.commit().await.unwrap();
        assert_eq!(replica.sync(&engine).await.unwrap(), Some(2));
        let now = engine.namespace("en").unwrap();
        assert!(Arc::ptr_eq(&base_before, &now.get("base").unwrap()));
        assert_eq!(now.get("acme").unwrap().len(), 8);
        assert_eq!(
            (now.version(), engine.namespace("de").unwrap().version()),
            (2, 2)
        );
        // A held namespace keeps serving the version it was taken at.
        assert_eq!((held.version(), held.get("acme").unwrap().len()), (1, 5));

        let mut t = en.begin().await.unwrap();
        t.drop_index("acme");
        t.namespace("de").drop_index("base");
        t.commit().await.unwrap();
        assert_eq!(replica.sync(&engine).await.unwrap(), Some(3));
        assert_eq!(engine.namespaces(), ["en"]);
        assert_eq!(engine.namespace("en").unwrap().names(), ["base"]);
        assert_eq!(replica.sync(&engine).await.unwrap(), None);
        wipe(&ds).await;
    }
}

#[tokio::test]
async fn names_are_validated_and_old_manifests_load_into_the_default_namespace() {
    let ds = Database::open("memory://", Vec::<(String, String)>::new())
        .await
        .unwrap();
    assert!(matches!(ds.namespace("de/DE"), Err(Error::InvalidInput(_))));
    let mut t = ds.begin().await.unwrap();
    t.append_documents("radio/en", docs(0..2, "d"), []).unwrap();
    assert!(matches!(t.commit().await, Err(Error::InvalidInput(_))));

    let old = br#"{"version": 4, "parent": 3, "timestamp_ms": 1,
        "indexes": {"songs": {"max_score": 2.0, "segments": []}}, "metadata": {}}"#;
    let manifest = Manifest::from_json(old).unwrap();
    assert_eq!(manifest.index_names(completr::DEFAULT_NAMESPACE), ["songs"]);
    assert_eq!(manifest.index("default", "songs").unwrap().max_score, 2.0);
}
