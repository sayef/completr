//! Runs against a real bucket when `COMPLETR_TEST_S3_URL` is set, e.g. `s3://bucket/prefix/`.
#![cfg(feature = "aws")]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use completr::{BlockingStore, Document, Error, Index, IndexOptions, Segment};

#[test]
fn s3_round_trip() {
    let Ok(base) = std::env::var("COMPLETR_TEST_S3_URL") else {
        eprintln!("COMPLETR_TEST_S3_URL not set, skipping");
        return;
    };
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let url = format!("{}/test-{run}", base.trim_end_matches('/'));
    let store = BlockingStore::open(&url).unwrap();

    let base_segment = Segment::build(
        (0..2000).map(|i| Document::new(i, format!("item number {i}"), (i % 7) as f32 / 7.0)),
        [],
    )
    .unwrap();
    let delta = Segment::build([Document::new(5, "Rust Programming", 1.0)], [6]).unwrap();
    store
        .put_segment("segments/base.seg", &base_segment)
        .unwrap();
    store.put_segment("segments/delta.seg", &delta).unwrap();

    let segments = ["segments/base.seg", "segments/delta.seg"]
        .map(|key| Arc::new(store.get_segment(key).unwrap()));
    let index = Index::new(segments.to_vec(), IndexOptions::default().max_score(1000.0)).unwrap();
    assert_eq!(index.len(), 1999);
    assert_eq!(index.complete("rust prog", 5)[0].id, 5);

    assert!(store
        .put_if_absent("_versions/000000000001.json", b"{\"version\":1}".to_vec())
        .unwrap());
    assert!(!store
        .put_if_absent("_versions/000000000001.json", b"{\"version\":1}".to_vec())
        .unwrap());
    assert_eq!(
        store.list("").unwrap(),
        [
            "_versions/000000000001.json",
            "segments/base.seg",
            "segments/delta.seg"
        ]
    );

    for key in store.list("").unwrap() {
        store.delete(&key).unwrap();
    }
    assert!(store.list("").unwrap().is_empty());
    assert!(matches!(
        store.get_segment("segments/base.seg"),
        Err(Error::NotFound(_))
    ));
}
