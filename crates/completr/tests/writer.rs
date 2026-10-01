//! Budgeted segment writing: several segments that rank like one.

use std::sync::Arc;

use completr::{BuildOptions, Document, Index, IndexOptions, Segment, SegmentWriter};

fn docs() -> Vec<Document> {
    (0..5_000u64)
        .map(|i| {
            let text = format!("item {i} alpha{} beta{} gamma", i % 31, i % 7);
            Document::new(i, text, (i % 100) as f32 / 100.0)
        })
        .collect()
}

#[test]
fn budgeted_segments_rank_like_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SegmentWriter::new(BuildOptions::default(), dir.path())
        .unwrap()
        .memory_budget(400_000);
    for doc in docs() {
        writer.add(doc).unwrap();
    }
    // A later copy of an id wins over the earlier one, in another segment.
    writer.add(Document::new(7, "replaced title", 1.0)).unwrap();
    let segments: Vec<Arc<Segment>> = writer.finish().unwrap().into_iter().map(Arc::new).collect();
    assert!(segments.len() > 3, "{} segments", segments.len());
    let split = Index::new(segments, IndexOptions::default()).unwrap();

    let mut all = docs();
    all[7] = Document::new(7, "replaced title", 1.0);
    let one = Index::new(
        vec![Arc::new(Segment::build(all, []).unwrap())],
        IndexOptions::default(),
    )
    .unwrap();
    for q in [
        "item",
        "alpha1",
        "beta3 gam",
        "item 12",
        "alpah",
        "replaced",
        "item 7 ",
    ] {
        assert_eq!(split.complete(q, 10), one.complete(q, 10), "{q}");
    }
}

#[test]
fn empty_writer_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let writer = SegmentWriter::new(BuildOptions::default(), dir.path()).unwrap();
    assert!(writer.finish().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
