//! Reproducible benchmark on a synthetic corpus: `cargo run --release --example bench -- [documents] [--vectors]`

use std::sync::Arc;
use std::time::Instant;

use strato::{AliasKind, Document, HybridOptions, Index, IndexConfig, Segment};

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

    /// Zipf-like: small indexes are much more likely.
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

fn latency(label: &str, queries: &[String], run: impl Fn(&str)) {
    let mut times: Vec<f64> = queries
        .iter()
        .map(|q| {
            let t = Instant::now();
            run(q);
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let at = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
    println!(
        "| {label} | {} | {:.3} ms | {:.3} ms | {:.3} ms |",
        queries.len(),
        at(0.5),
        at(0.99),
        times[times.len() - 1]
    );
}

fn unit_vector(rng: &mut Rng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim)
        .map(|_| (rng.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5)
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / norm).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.iter().find_map(|a| a.parse().ok()).unwrap_or(200_000);
    let with_vectors = args.iter().any(|a| a == "--vectors");
    let dim = 256;
    let mut rng = Rng(0x5eed);
    let mut docs = corpus(n, &mut rng);
    if with_vectors {
        docs = docs
            .into_iter()
            .map(|d| d.with_vector(unit_vector(&mut rng, dim)))
            .collect();
    }
    let (typed, typos) = queries(&docs, &mut rng, 2000);

    let t = Instant::now();
    let segment = Segment::build(docs.clone(), []).unwrap();
    let build = t.elapsed();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("base.seg");
    segment.save(&path).unwrap();
    drop(segment);
    let t = Instant::now();
    let base = Arc::new(Segment::open(&path).unwrap());
    let open = t.elapsed();
    println!(
        "{n} documents{}\n",
        if with_vectors {
            format!(", {dim}-d vectors at 4 bits")
        } else {
            String::new()
        }
    );
    println!("| Step | Result |\n|---|---|");
    println!(
        "| Build segment | {:.0} ms, {:.1} MB |",
        build.as_secs_f64() * 1e3,
        base.size_bytes() as f64 / 1e6
    );
    println!(
        "| Open (memory-mapped, checksum verified) | {:.1} ms |",
        open.as_secs_f64() * 1e3
    );

    let updates: Vec<Document> = docs
        .iter()
        .step_by(n / 1000)
        .map(|d| Document {
            weight: 1.0,
            ..d.clone()
        })
        .collect();
    let t = Instant::now();
    let delta = Arc::new(Segment::build(updates, [1, 2, 3]).unwrap());
    println!(
        "| Delta segment, 1,000 upserts | {:.1} ms |\n",
        t.elapsed().as_secs_f64() * 1e3
    );

    let config = IndexConfig {
        short_query_cache_entries: 0,
        ..IndexConfig::default()
    };
    let one = Index::new(vec![base.clone()], config.clone()).unwrap();
    let layered = Index::new(vec![base.clone(), delta], config).unwrap();
    let cached = Index::new(vec![base], IndexConfig::default()).unwrap();
    println!("| Query | Count | p50 | p99 | max |\n|---|---|---|---|---|");
    latency("Typed prefixes", &typed, |q| drop(one.autocomplete(q, 10)));
    latency("Typed prefixes, short-query cache", &typed, |q| {
        drop(cached.autocomplete(q, 10))
    });
    latency("Typed prefixes, base + delta", &typed, |q| {
        drop(layered.autocomplete(q, 10))
    });
    latency("One-edit typos", &typos, |q| drop(one.autocomplete(q, 10)));
    latency("Synonym aliases", &typed, |q| {
        drop(one.search_aliases(q, 10))
    });
    if with_vectors {
        let vectors: Vec<Vec<f32>> = (0..1000).map(|_| unit_vector(&mut rng, dim)).collect();
        let keys: Vec<String> = (0..vectors.len()).map(|i| i.to_string()).collect();
        let vector = |q: &str| &vectors[q.parse::<usize>().unwrap()];
        latency("Vector search", &keys, |q| {
            drop(one.vector_search(vector(q), 10).unwrap())
        });
        let hybrid: Vec<String> = (0..vectors.len())
            .map(|i| format!("{i} {}", typed[i * 7 % typed.len()]))
            .collect();
        latency("Hybrid (RRF)", &hybrid, |q| {
            let (i, text) = q.split_once(' ').unwrap();
            drop(
                one.hybrid_search(text, vector(i), 10, HybridOptions::default())
                    .unwrap(),
            );
        });
    }
}
