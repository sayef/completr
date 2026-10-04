//! Criterion benchmarks on the synthetic corpus of `examples/bench.rs`: `cargo bench -p completr`

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};

/// Every call into the completr API goes through here.
mod api {
    use std::path::Path;
    use std::sync::Arc;

    pub use completr::{AliasKind, Document, Index};
    use completr::{HybridOptions, IndexOptions, Segment};

    pub fn build(docs: Vec<Document>, deletes: Vec<u64>) -> Segment {
        Segment::build(docs, deletes).unwrap()
    }

    pub fn save_and_open(segment: &Segment, path: &Path) -> Arc<Segment> {
        segment.save(path).unwrap();
        Arc::new(Segment::open(path).unwrap())
    }

    pub fn index(segments: Vec<Arc<Segment>>) -> Index {
        let options = IndexOptions::default().short_query_cache_entries(0);
        Index::new(segments, options).unwrap()
    }

    pub fn complete(index: &Index, query: &str, limit: usize) -> usize {
        index.complete(query, limit).len()
    }

    pub fn vector_search(index: &Index, vector: &[f32], limit: usize) -> usize {
        index.vector_search(vector, limit).unwrap().len()
    }

    pub fn hybrid_search(index: &Index, text: &str, vector: &[f32], limit: usize) -> usize {
        index
            .hybrid_search(text, vector, limit, &HybridOptions::default())
            .unwrap()
            .len()
    }
}

use api::{AliasKind, Document, Index};

const DOCS: usize = 50_000;
const DIM: usize = 128;
const LIMIT: usize = 10;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn skewed(&mut self, n: usize) -> usize {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        ((n as f64).powf(u) - 1.0) as usize
    }
}

fn vocabulary(rng: &mut Rng, size: usize) -> Vec<String> {
    const ONSETS: [&str; 18] = [
        "b", "c", "d", "f", "g", "k", "l", "m", "n", "p", "r", "s", "t", "v", "st", "tr", "pl",
        "ch",
    ];
    const VOWELS: [&str; 7] = ["a", "e", "i", "o", "u", "ai", "ou"];
    const CODAS: [&str; 8] = ["", "", "n", "r", "s", "t", "l", "x"];
    let mut words = std::collections::BTreeSet::new();
    while words.len() < size {
        let syllables = 1 + rng.below(3);
        let word: String = (0..syllables)
            .map(|_| {
                format!(
                    "{}{}{}",
                    ONSETS[rng.below(18)],
                    VOWELS[rng.below(7)],
                    CODAS[rng.below(8)]
                )
            })
            .collect();
        if word.len() >= 3 {
            words.insert(word);
        }
    }
    let mut words: Vec<String> = words.into_iter().collect();
    for i in (1..words.len()).rev() {
        words.swap(i, rng.below(i + 1));
    }
    words
}

fn corpus(n: usize, rng: &mut Rng) -> Vec<Document> {
    let words = vocabulary(rng, 30_000);
    (0..n as u64)
        .map(|id| {
            let len = 1 + rng.skewed(5).min(3);
            let text: Vec<&str> = (0..len)
                .map(|_| words[rng.skewed(words.len())].as_str())
                .collect();
            let mut doc = Document::new(id, text.join(" "), rng.below(1000) as f32 / 1000.0);
            if id % 10 == 0 {
                doc = doc.with_alias(words[rng.below(words.len())].clone(), AliasKind::Synonym);
            }
            if len > 1 && id % 25 == 0 {
                let initials: String = text.iter().map(|w| w[..1].to_uppercase()).collect();
                doc = doc.with_alias(initials, AliasKind::Abbreviation);
            }
            doc
        })
        .collect()
}

/// Every prefix of sampled titles, as typed, plus one-edit typos.
fn queries(docs: &[Document], rng: &mut Rng, titles: usize) -> (Vec<String>, Vec<String>) {
    let mut typed = Vec::new();
    let mut typos = Vec::new();
    for _ in 0..titles {
        let text: Vec<char> = docs[rng.below(docs.len())].text.chars().collect();
        typed.extend((1..=text.len()).map(|n| text[..n].iter().collect::<String>()));
        let mut typo = text.clone();
        let at = rng.below(typo.len());
        match rng.below(3) {
            0 => typo[at] = (b'a' + rng.below(26) as u8) as char,
            1 => {
                typo.remove(at);
            }
            _ => typo.insert(at, (b'a' + rng.below(26) as u8) as char),
        }
        typos.push(typo.into_iter().collect());
    }
    (typed, typos)
}

fn unit_vector(rng: &mut Rng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim)
        .map(|_| (rng.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5)
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / norm).collect()
}

struct Fixture {
    docs: Vec<Document>,
    typed: Vec<String>,
    typos: Vec<String>,
    vectors: Vec<Vec<f32>>,
    base: Index,
    layered: Index,
    semantic: Index,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let mut rng = Rng(0x5eed);
    let docs = corpus(DOCS, &mut rng);
    let (mut typed, typos) = queries(&docs, &mut rng, 100);
    typed.truncate(400);
    let dir = tempfile::tempdir().unwrap();

    let base = api::save_and_open(
        &api::build(docs.clone(), vec![]),
        &dir.path().join("base.seg"),
    );
    let updates: Vec<Document> = docs
        .iter()
        .step_by(DOCS / 1000)
        .map(|d| {
            let mut d = d.clone();
            d.popularity = 1.0;
            d
        })
        .collect();
    let delta = api::save_and_open(
        &api::build(updates, vec![1, 2, 3]),
        &dir.path().join("delta.seg"),
    );

    let with_vectors: Vec<Document> = docs
        .iter()
        .map(|d| d.clone().with_vector(unit_vector(&mut rng, DIM)))
        .collect();
    let semantic = api::save_and_open(
        &api::build(with_vectors, vec![]),
        &dir.path().join("vectors.seg"),
    );
    let vectors = (0..100).map(|_| unit_vector(&mut rng, DIM)).collect();

    Fixture {
        typed,
        typos,
        vectors,
        base: api::index(vec![base.clone()]),
        layered: api::index(vec![base, delta]),
        semantic: api::index(vec![semantic]),
        docs,
        _dir: dir,
    }
}

fn run_queries(c: &mut Criterion, name: &str, index: &Index, queries: &[String]) {
    let mut group = c.benchmark_group("complete");
    group.throughput(Throughput::Elements(queries.len() as u64));
    group.bench_function(name, |b| {
        b.iter(|| {
            for q in queries {
                black_box(api::complete(index, black_box(q), LIMIT));
            }
        })
    });
    group.finish();
}

fn benches(c: &mut Criterion) {
    let f = fixture();

    let mut group = c.benchmark_group("build");
    group.throughput(Throughput::Elements(DOCS as u64));
    group.bench_function("segment_50k", |b| {
        b.iter_batched(
            || f.docs.clone(),
            |docs| black_box(api::build(docs, vec![])),
            BatchSize::LargeInput,
        )
    });
    group.finish();

    run_queries(c, "typed_prefixes", &f.base, &f.typed);
    run_queries(c, "typos", &f.base, &f.typos);
    run_queries(c, "typed_prefixes_base_delta", &f.layered, &f.typed);

    let mut group = c.benchmark_group("semantic");
    group.throughput(Throughput::Elements(f.vectors.len() as u64));
    group.bench_function("vector_search", |b| {
        b.iter(|| {
            for v in &f.vectors {
                black_box(api::vector_search(&f.semantic, black_box(v), LIMIT));
            }
        })
    });
    group.bench_function("hybrid_rrf", |b| {
        b.iter(|| {
            for (i, v) in f.vectors.iter().enumerate() {
                let text = &f.typed[i * 7 % f.typed.len()];
                black_box(api::hybrid_search(&f.semantic, text, black_box(v), LIMIT));
            }
        })
    });
    group.finish();
}

fn config() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
}

criterion_group! {
    name = search;
    config = config();
    targets = benches
}
criterion_main!(search);
