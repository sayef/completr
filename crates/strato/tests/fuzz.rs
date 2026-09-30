//! Stable-Rust fuzzing: corrupted segments and manifests must fail with an error, never panic,
//! and arbitrary unicode queries must not panic and must be deterministic.
//! `STRATO_FUZZ_ITERS` scales the run, e.g. to 1_000_000 for a long session.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use strato::{AliasKind, BuildOptions, Document, Index, IndexOptions, Segment};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn iterations(default: usize) -> usize {
    std::env::var("STRATO_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const ALPHABET: &[&str] = &[
    "a", "e", "s", "t", "r", "n", "ä", "ß", "é", "ł", "ő", "ñ", " ", "-", "/", "+", ".", "日",
    "本", "語", "カ", "ー", "\u{301}", "\u{200d}", "İ", "ﬁ", "😀", "\0", "\u{fffd}", "Σ",
];

fn random_text(rng: &mut Rng, max: usize) -> String {
    (0..rng.below(max + 1))
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

fn corpus(rng: &mut Rng) -> Vec<Document> {
    let base = [
        "data science",
        "rust programming",
        "machine learning",
        "Kundenservice",
        "gestion de projet",
        "日本語 教育",
        "C++",
        "UX/UI design",
    ];
    (0..400u64)
        .map(|id| {
            let text = if id % 3 == 0 {
                random_text(rng, 24)
            } else {
                format!("{} {}", base[rng.below(base.len())], id % 37)
            };
            let mut doc = Document::new(id, text, rng.below(10) as f32 / 10.0)
                .with_vector((0..8).map(|i| ((id + i) % 7) as f32 - 3.0).collect());
            if id % 5 == 0 {
                doc = doc
                    .with_alias(random_text(rng, 10), AliasKind::Synonym)
                    .with_alias("DS", AliasKind::Abbreviation);
            }
            doc
        })
        .collect()
}

fn exercise(index: &Index, rng: &mut Rng) {
    for _ in 0..8 {
        let q = random_text(rng, 12);
        index.complete(&q, 10);
        index.complete_aliases(&q, 10);
    }
    let _ = index.vector_search(&[0.5; 8], 5);
    for id in 0..20 {
        index.document(rng.below(400) as u64 + id);
    }
    let _ = index.compact();
}

#[test]
fn corrupted_segments_never_panic() {
    let mut rng = Rng(0x5eed);
    let docs = corpus(&mut rng);
    let segments: Vec<Vec<u8>> = [false, true]
        .into_iter()
        .map(|compact_keys| {
            Segment::build_with(
                BuildOptions::default().compact_keys(compact_keys),
                docs.clone(),
                [3, 9],
            )
            .unwrap()
            .to_bytes()
        })
        .collect();
    let location: Arc<std::sync::Mutex<String>> = Arc::default();
    let hook_location = location.clone();
    std::panic::set_hook(Box::new(move |info| {
        *hook_location.lock().unwrap() = info.to_string();
    }));
    let (mut rejected, mut loaded, mut crafted) = (0, 0, 0);
    for i in 0..iterations(3000) {
        let mut data = segments[i % segments.len()].clone();
        match rng.below(4) {
            0 => data.truncate(rng.below(data.len())),
            1 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(data.len());
                    data[at] ^= 1 << rng.below(8);
                }
            }
            2 => {
                let at = rng.below(data.len());
                let end = (at + 1 + rng.below(64)).min(data.len());
                data[at..end].iter_mut().for_each(|b| *b = rng.next() as u8);
            }
            _ => {
                let at = rng.below(data.len()) & !7;
                let end = (at + 8).min(data.len());
                data[at..end].copy_from_slice(&u64::MAX.to_le_bytes()[..end - at]);
            }
        }
        let unchanged = data == segments[i % segments.len()];
        // Re-sealing half the inputs gets past the checksum to the structural validation.
        let sealed = rng.below(2) == 0 && data.len() >= 8 && data.len().is_multiple_of(8);
        if sealed {
            let body = data.len() - 8;
            let sum = xxhash_rust::xxh3::xxh3_64(&data[..body]);
            data[body..].copy_from_slice(&sum.to_le_bytes());
        }
        let seed = rng.next();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            match Segment::from_bytes(data.clone()) {
                Err(_) => false,
                Ok(segment) => {
                    if let Ok(index) = Index::new(vec![Arc::new(segment)], IndexOptions::default())
                    {
                        exercise(&index, &mut Rng(seed | 1));
                    }
                    true
                }
            }
        }));
        match outcome {
            Ok(true) => {
                if !sealed && !unchanged {
                    let _ = std::panic::take_hook();
                    panic!("iteration {i}: corrupted segment passed the checksum");
                }
                loaded += 1;
            }
            Ok(false) => rejected += 1,
            // fst and lazily read document blocks trust checksummed content; only crafted input gets here.
            Err(_)
                if ["/fst-", "validated segment"]
                    .iter()
                    .any(|p| location.lock().unwrap().contains(p)) =>
            {
                crafted += 1
            }
            Err(_) => {
                let path = std::env::temp_dir().join(format!("strato-fuzz-{i}.seg"));
                std::fs::write(&path, &data).unwrap();
                let message = location.lock().unwrap().clone();
                let _ = std::panic::take_hook();
                panic!(
                    "iteration {i} panicked: {message}; input saved to {}",
                    path.display()
                );
            }
        }
    }
    let _ = std::panic::take_hook();
    eprintln!("{rejected} rejected, {loaded} crafted inputs loaded and queried, {crafted} panics in fst or document blocks");
}

#[test]
fn random_queries_are_deterministic() {
    let mut rng = Rng(42);
    let docs = corpus(&mut rng);
    let whole = Index::new(
        vec![Arc::new(Segment::build(docs.clone(), []).unwrap())],
        IndexOptions::default(),
    )
    .unwrap();
    let parts: Vec<Arc<Segment>> = docs
        .chunks(97)
        .map(|c| Arc::new(Segment::build(c.to_vec(), []).unwrap()))
        .collect();
    let segmented = Index::new(parts, IndexOptions::default()).unwrap();
    let bits = |index: &Index, q: &str| -> Vec<(u64, u64, &'static str)> {
        let mut out: Vec<_> = index
            .complete(q, 10)
            .into_iter()
            .map(|h| (h.id, h.score.to_bits(), h.kind.as_str()))
            .collect();
        out.extend(
            index
                .complete_aliases(q, 10)
                .into_iter()
                .map(|h| (h.id, h.score.to_bits(), "alias")),
        );
        out
    };
    for _ in 0..iterations(20_000) {
        let q = if rng.below(4) == 0 {
            let doc = &docs[rng.below(docs.len())].text;
            doc.chars()
                .take(rng.below(doc.chars().count() + 1))
                .collect()
        } else {
            random_text(&mut rng, 16)
        };
        let first = bits(&whole, &q);
        assert_eq!(bits(&whole, &q), first, "repeat differs for {q:?}");
        assert_eq!(bits(&segmented, &q), first, "segmented differs for {q:?}");
    }
}

#[cfg(feature = "store")]
#[test]
fn corrupted_manifests_never_panic() {
    let manifest = br#"{"version":3,"parent":2,"timestamp_ms":1700000000000,"indexes":{"i":{"segments":[{"path":"s/1.seg","id":1,"level":0,"documents":10,"deletes":0,"bytes":100}],"max_score":1000.0}},"metadata":{"strato.inbox.applied":"[\"a\"]"}}"#;
    let mut rng = Rng(7);
    for _ in 0..iterations(20_000) {
        let mut data = manifest.to_vec();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(data.len());
            match rng.below(3) {
                0 => data[at] = rng.next() as u8,
                1 => {
                    data.remove(at);
                }
                _ => data.insert(at, b"{}[]\",:0-e9"[rng.below(11)]),
            }
        }
        let _ = strato::Manifest::from_json(&data);
    }
}
