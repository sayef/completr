//! Queries from many threads while a writer commits and compacts and a follower swaps indexes.
//! Every sampled result must be reproduced exactly on the same snapshot and on a fresh index.
#![cfg(feature = "store")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use strato::{
    Batch, Dataset, Document, Engine, Follower, Fusion, HybridOptions, Index, IndexConfig, Writer,
};

const DIM: usize = 16;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    }

    fn vector(&mut self) -> Vec<f32> {
        let v: Vec<f32> = (0..DIM).map(|_| self.unit()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        v.into_iter().map(|x| x / norm).collect()
    }
}

const WORDS: [&str; 12] = [
    "data", "science", "rust", "python", "cloud", "design", "machine", "learning", "sales",
    "finance", "java", "vision",
];

fn doc(id: u64, rng: &mut Rng) -> Document {
    let text = format!(
        "{} {} {}",
        WORDS[(rng.next() % 12) as usize],
        WORDS[(rng.next() % 12) as usize],
        id % 97
    );
    Document::new(id, text, (rng.next() % 10) as f32 / 10.0).with_vector(rng.vector())
}

#[derive(Clone, Debug)]
enum Query {
    Complete(String),
    Aliases(String),
    Vector(Vec<f32>),
    Hybrid(String, Vec<f32>),
}

type Bits = Vec<(u64, u64, &'static str)>;
type Sample = (Arc<Index>, Query, Bits);

/// Results as comparable bits.
fn run(index: &Index, query: &Query) -> Bits {
    match query {
        Query::Complete(q) => index
            .autocomplete(q, 10)
            .into_iter()
            .map(|h| (h.id, h.score.to_bits(), h.kind.as_str()))
            .collect(),
        Query::Aliases(q) => index
            .search_aliases(q, 10)
            .into_iter()
            .map(|h| (h.id, h.score.to_bits(), "alias"))
            .collect(),
        Query::Vector(v) => index
            .vector_search(v, 10)
            .unwrap()
            .into_iter()
            .map(|h| (h.id, h.score.to_bits(), h.kind.as_str()))
            .collect(),
        Query::Hybrid(q, v) => index
            .hybrid_search(
                q,
                v,
                10,
                HybridOptions {
                    fusion: Fusion::ReciprocalRank { k: 60.0 },
                    candidates: None,
                },
            )
            .unwrap()
            .into_iter()
            .map(|h| (h.id, h.score.to_bits(), h.kind.as_str()))
            .collect(),
    }
}

#[test]
fn queries_stay_deterministic_under_concurrent_updates() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ds = runtime
        .block_on(Dataset::open(
            "memory:///concurrency",
            Vec::<(String, String)>::new(),
        ))
        .unwrap();
    let mut rng = Rng(1);
    let base: Vec<Document> = (0..5000).map(|i| doc(i, &mut rng)).collect();
    runtime.block_on(async {
        let mut t = ds.begin().await.unwrap();
        t.append_documents("i", base, [])
            .unwrap()
            .set_max_score("i", 1000.0);
        t.commit().await.unwrap();
    });
    let engine = Arc::new(Engine::new());
    let follower = Follower::new(ds.clone(), IndexConfig::default());
    runtime.block_on(follower.sync(&engine)).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let queries = Arc::new(AtomicU64::new(0));
    let samples: Arc<Mutex<Vec<Sample>>> = Arc::default();
    let latencies: Arc<Mutex<Vec<f64>>> = Arc::default();
    let readers: Vec<_> = (0..8)
        .map(|t| {
            let (engine, stop, queries, samples, latencies) = (
                engine.clone(),
                stop.clone(),
                queries.clone(),
                samples.clone(),
                latencies.clone(),
            );
            std::thread::spawn(move || {
                let mut rng = Rng(100 + t);
                let mut local = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let word = WORDS[(rng.next() % 12) as usize];
                    let text: String = word.chars().take(1 + (rng.next() % 6) as usize).collect();
                    let query = match rng.next() % 4 {
                        0 => Query::Complete(text),
                        1 => Query::Aliases(text),
                        2 => Query::Vector(rng.vector()),
                        _ => Query::Hybrid(text, rng.vector()),
                    };
                    let snapshot = engine.get("i").unwrap();
                    let started = Instant::now();
                    let result = run(&snapshot, &query);
                    local.push(started.elapsed().as_secs_f64() * 1000.0);
                    // Layered reads go through the engine's own snapshot and must not fail either.
                    let _ = engine.autocomplete(&["i", "missing"], "da", 5);
                    if rng.next().is_multiple_of(20) {
                        samples.lock().unwrap().push((snapshot, query, result));
                    }
                    queries.fetch_add(1, Ordering::Relaxed);
                }
                latencies.lock().unwrap().extend(local);
            })
        })
        .collect();

    let started = Instant::now();
    let versions = runtime.block_on(async {
        let mut writer = Writer::new(ds.clone(), "writer");
        let mut rng = Rng(7);
        let mut next_id = 10_000;
        for round in 0..150u64 {
            let mut batch = Batch::new();
            let upserts: Vec<Document> = (0..10)
                .map(|i| {
                    if i % 2 == 0 {
                        doc(rng.next() % 5000, &mut rng)
                    } else {
                        next_id += 1;
                        doc(next_id, &mut rng)
                    }
                })
                .collect();
            batch.upsert("i", upserts).delete("i", [rng.next() % 5000]);
            ds.submit(batch).await.unwrap();
            writer.run_once().await.unwrap();
            if round % 2 == 0 {
                follower.sync(&engine).await.unwrap();
            }
        }
        follower.sync(&engine).await.unwrap();
        follower.version().await
    });
    std::thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    for reader in readers {
        reader.join().unwrap();
    }
    let elapsed = started.elapsed();

    let samples = std::mem::take(&mut *samples.lock().unwrap());
    let snapshots: std::collections::HashSet<*const Index> =
        samples.iter().map(|(s, _, _)| Arc::as_ptr(s)).collect();
    assert!(
        snapshots.len() > 10,
        "readers saw only {} index versions",
        snapshots.len()
    );
    for (snapshot, query, result) in &samples {
        assert_eq!(
            &run(snapshot, query),
            result,
            "same snapshot, different result for {query:?}"
        );
        let fresh = Index::new(snapshot.segments().to_vec(), snapshot.config().clone()).unwrap();
        assert_eq!(
            &run(&fresh, query),
            result,
            "fresh index differs for {query:?}"
        );
    }
    let mut latencies = std::mem::take(&mut *latencies.lock().unwrap());
    latencies.sort_by(f64::total_cmp);
    let total = queries.load(Ordering::Relaxed);
    eprintln!(
        "{total} queries in {:.1} s across {} snapshots and {versions} versions ({:.0}/s); p50 {:.3} ms, p99 {:.3} ms; {} samples verified",
        elapsed.as_secs_f64(),
        snapshots.len(),
        total as f64 / elapsed.as_secs_f64(),
        latencies[latencies.len() / 2],
        latencies[latencies.len() * 99 / 100],
        samples.len()
    );
}
