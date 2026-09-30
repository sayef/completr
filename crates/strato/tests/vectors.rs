use std::sync::Arc;

use strato::{BuildOptions, Document, Engine, Error, Index, IndexOptions, MatchKind, Segment};

const DIM: usize = 64;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    }

    /// A unit vector near one of 20 cluster centres.
    fn vector(&mut self, centres: &[Vec<f32>], i: usize) -> Vec<f32> {
        let centre = &centres[i % centres.len()];
        let v: Vec<f32> = centre.iter().map(|c| c + 0.3 * self.next()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / norm).collect()
    }
}

fn corpus(n: u64, seed: u64) -> (Vec<Document>, Vec<Vec<f32>>) {
    let mut rng = Rng(seed);
    let centres: Vec<Vec<f32>> = (0..20)
        .map(|_| (0..DIM).map(|_| rng.next()).collect())
        .collect();
    let docs = (0..n)
        .map(|i| {
            Document::new(i, format!("item {i}"), 0.5).with_vector(rng.vector(&centres, i as usize))
        })
        .collect();
    (docs, centres)
}

fn index(segments: Vec<Segment>) -> Index {
    let config = IndexOptions::default().max_score(1000.0);
    Index::new(segments.into_iter().map(Arc::new).collect(), config).unwrap()
}

fn exact(docs: &[Document], query: &[f32], k: usize) -> Vec<u64> {
    let mut scored: Vec<(f32, u64)> = docs
        .iter()
        .map(|d| {
            (
                d.vector
                    .as_ref()
                    .unwrap()
                    .iter()
                    .zip(query)
                    .map(|(a, b)| a * b)
                    .sum(),
                d.id,
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(k).map(|(_, id)| id).collect()
}

fn results(index: &Index, queries: &[Vec<f32>]) -> Vec<Vec<(u64, u64)>> {
    queries
        .iter()
        .map(|q| {
            index
                .vector_search(q, 10)
                .unwrap()
                .into_iter()
                .map(|h| (h.id, h.score.to_bits()))
                .collect()
        })
        .collect()
}

/// Recall on real embeddings is measured by the model2vec benchmark; this checks the basics:
/// ordering, match kind, and that a stored vector finds its own document first.
#[test]
fn finds_nearest_neighbours() {
    let (docs, _) = corpus(3000, 1);
    let idx = index(vec![Segment::build(docs.clone(), []).unwrap()]);
    assert_eq!(idx.vector_dim(), Some(DIM));
    let mut self_hits = 0;
    let mut in_exact_top10 = 0;
    for doc in docs.iter().step_by(30) {
        let query = doc.vector.as_ref().unwrap();
        let hits = idx.vector_search(query, 10).unwrap();
        assert!(hits.iter().all(|h| h.kind == MatchKind::Semantic));
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
        self_hits += usize::from(hits[0].id == doc.id);
        let want = exact(&docs, query, 10);
        in_exact_top10 += usize::from(want.contains(&hits[0].id));
    }
    assert!(self_hits >= 95, "self hits {self_hits} of 100");
    assert_eq!(in_exact_top10, 100);
}

#[test]
fn matches_turbovec_directly() {
    let (docs, _) = corpus(3000, 1);
    let idx = index(vec![Segment::build(docs.clone(), []).unwrap()]);
    let flat: Vec<f32> = docs
        .iter()
        .flat_map(|d| d.vector.clone().unwrap())
        .collect();
    let mut direct = turbovec::TurboQuantIndex::new(DIM, 4).unwrap();
    direct.add(&flat);
    for doc in docs.iter().step_by(30) {
        let query = doc.vector.as_ref().unwrap();
        let want = direct.search(query, 10);
        let want: Vec<(u64, f32)> = want
            .indices_for_query(0)
            .iter()
            .zip(want.scores_for_query(0))
            .map(|(&i, &s)| (i as u64, s))
            .collect();
        let got: Vec<(u64, f32)> = idx
            .vector_search(query, 10)
            .unwrap()
            .iter()
            .map(|h| (h.id, h.score as f32))
            .collect();
        assert_eq!(got, want);
    }
}

#[test]
fn hides_superseded_and_deleted_vectors_and_compacts_exactly() {
    let (docs, centres) = corpus(1200, 2);
    let mut rng = Rng(9);
    let moved: Vec<Document> = (0..50)
        .map(|i| {
            Document::new(i, format!("moved {i}"), 0.5)
                .with_vector(rng.vector(&centres, i as usize + 7))
        })
        .collect();
    let deleted: Vec<u64> = (100..150).collect();
    let added: Vec<Document> = (5000..5100)
        .map(|i| {
            Document::new(i, format!("new {i}"), 0.5).with_vector(rng.vector(&centres, i as usize))
        })
        .collect();
    let layered = index(vec![
        Segment::build(docs.clone(), []).unwrap(),
        Segment::build(moved.clone(), deleted.clone()).unwrap(),
        Segment::build(added, []).unwrap(),
    ]);
    let queries: Vec<Vec<f32>> = docs
        .iter()
        .step_by(40)
        .map(|d| d.vector.clone().unwrap())
        .collect();
    let before = results(&layered, &queries);
    for hits in &before {
        assert!(hits.iter().all(|(id, _)| !deleted.contains(id)));
    }
    // The old vector of a moved document no longer finds it first.
    let old = docs[3].vector.as_ref().unwrap();
    assert_ne!(layered.vector_search(old, 1).unwrap()[0].id, 3);

    let compacted = index(vec![layered.compact().unwrap()]);
    assert_eq!(results(&compacted, &queries), before);
    let copy = Segment::from_bytes(layered.compact().unwrap().to_bytes()).unwrap();
    assert_eq!(results(&index(vec![copy]), &queries), before);
}

#[test]
fn handles_missing_vectors_bits_and_shapes() {
    let (docs, _) = corpus(200, 3);
    let mut mixed = docs.clone();
    for doc in mixed.iter_mut().step_by(2) {
        doc.vector = None;
    }
    let idx = index(vec![Segment::build(mixed, []).unwrap()]);
    let hits = idx
        .vector_search(docs[0].vector.as_ref().unwrap(), 200)
        .unwrap();
    assert_eq!(hits.len(), 100);
    assert!(hits.iter().all(|h| h.id % 2 == 1));
    assert!(matches!(
        idx.vector_search(&[0.0; 8], 5),
        Err(Error::InvalidInput(_))
    ));

    for bits in [2, 3, 4] {
        let config = BuildOptions::default().vector_bits(bits);
        let seg = Segment::build_with(config, docs.clone(), []).unwrap();
        assert_eq!(
            index(vec![seg])
                .vector_search(docs[5].vector.as_ref().unwrap(), 1)
                .unwrap()[0]
                .id,
            5
        );
    }
    let two =
        Segment::build_with(BuildOptions::default().vector_bits(2), docs.clone(), []).unwrap();
    let four = Segment::build(docs.clone(), []).unwrap();
    let config = IndexOptions::default().max_score(1000.0);
    assert!(Index::new(vec![Arc::new(two), Arc::new(four)], config).is_err());

    let odd = vec![Document::new(1, "a", 0.1).with_vector(vec![0.5; 12])];
    assert!(Segment::build(odd, []).is_err());
    let bad_bits = BuildOptions::default().vector_bits(5);
    assert!(Segment::build_with(bad_bits, docs, []).is_err());
    assert!(index(vec![Segment::build(
        [Document::new(1, "no vectors", 0.1)],
        []
    )
    .unwrap()])
    .vector_search(&[0.0; 8], 3)
    .unwrap()
    .is_empty());
}

#[test]
fn layers_override_by_id() {
    let (docs, _) = corpus(500, 4);
    let engine = Engine::new();
    let overlay: Vec<Document> = docs[..20]
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d.text = format!("{} custom", d.text);
            d
        })
        .collect();
    engine.publish([
        (
            "default".to_owned(),
            Some(Arc::new(index(vec![
                Segment::build(docs.clone(), []).unwrap()
            ]))),
        ),
        (
            "acme".to_owned(),
            Some(Arc::new(index(vec![Segment::build(overlay, []).unwrap()]))),
        ),
    ]);
    let query = docs[3].vector.as_ref().unwrap();
    let hits = engine
        .vector_search(&["default", "acme"], query, 10)
        .unwrap();
    assert_eq!(hits.len(), 10);
    let top = hits
        .iter()
        .find(|h| h.suggestion.id == 3)
        .expect("the query document itself");
    assert_eq!(top.layer, 1);
    let mut ids: Vec<u64> = hits.iter().map(|h| h.suggestion.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 10);
}

#[cfg(feature = "store")]
#[tokio::test(flavor = "multi_thread")]
async fn database_compaction_keeps_vectors() {
    let (docs, centres) = corpus(800, 5);
    let ds = strato::Database::open("memory:///vectors", Vec::<(String, String)>::new())
        .await
        .unwrap();
    let mut t = ds.begin().await.unwrap();
    t.append_documents("a", docs.clone(), [])
        .unwrap()
        .set_max_score("a", 1000.0);
    t.commit().await.unwrap();
    let mut rng = Rng(11);
    for round in 0..5u64 {
        let mut t = ds.begin().await.unwrap();
        let batch: Vec<Document> = (0..20)
            .map(|i| {
                let id = 10_000 + round * 20 + i;
                Document::new(id, format!("r{round}"), 0.2)
                    .with_vector(rng.vector(&centres, id as usize))
            })
            .collect();
        t.append_documents("a", batch, [round * 3]).unwrap();
        t.commit().await.unwrap();
    }
    let queries: Vec<Vec<f32>> = docs
        .iter()
        .step_by(50)
        .map(|d| d.vector.clone().unwrap())
        .collect();
    let before = results(
        &ds.open_index(&ds.latest().await.unwrap(), "a", IndexOptions::default())
            .await
            .unwrap(),
        &queries,
    );
    // Any hidden document forces a full merge.
    let policy = strato::CompactionPolicy::default().max_hidden_fraction(0.0);
    let full = ds.compact("a", &policy).await.unwrap().unwrap();
    assert_eq!(full.indexes["a"].segments.len(), 1);
    let after = results(
        &ds.open_index(&full, "a", IndexOptions::default())
            .await
            .unwrap(),
        &queries,
    );
    assert_eq!(after, before);

    let engine = Engine::new();
    strato::Replica::new(ds.clone(), IndexOptions::default())
        .sync(&engine)
        .await
        .unwrap();
    assert_eq!(results(&engine.get("a").unwrap(), &queries), before);
}
