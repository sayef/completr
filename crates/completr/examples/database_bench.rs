//! Database benchmarks on a real corpus. Run each phase in its own process so peak RSS is per phase:
//!   database_bench load <corpus.json> <url>
//!   database_bench open <url>
//!   database_bench rebuild <url> <index>
//!   database_bench incremental <url> <index> <batch sizes, comma separated>
//!   database_bench compact <url> <index>
//!   database_bench concurrent <url> <index> <writers> <batches> <batch size> [strict]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use completr::{
    CompactionPolicy, Database, Document, Engine, Error, Index, IndexOptions, Replica, Segment,
};

struct Usage {
    wall: Instant,
    cpu: f64,
}

fn cpu_seconds() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

fn peak_mb() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let bytes = if cfg!(target_os = "macos") {
        usage.ru_maxrss as f64
    } else {
        usage.ru_maxrss as f64 * 1024.0
    };
    bytes / 1_048_576.0
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn start() -> Usage {
    Usage {
        wall: Instant::now(),
        cpu: cpu_seconds(),
    }
}

fn report(label: &str, usage: Usage) -> Duration {
    let wall = usage.wall.elapsed();
    println!(
        "  {label:<44} wall {:>9.1} ms  cpu {:>9.1} ms  rss {:>7.1} MB  peak {:>7.1} MB",
        wall.as_secs_f64() * 1000.0,
        (cpu_seconds() - usage.cpu) * 1000.0,
        rss_mb(),
        peak_mb()
    );
    wall
}

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
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn queries(index: &Index, n: usize) -> Vec<String> {
    let docs: Vec<Document> = index.documents().collect();
    let mut rng = Rng(42);
    (0..n)
        .map(|i| {
            let text: Vec<char> = docs[rng.below(docs.len())].text.chars().collect();
            let len = (4 + rng.below(9)).min(text.len());
            let mut q: Vec<char> = text[..len].to_vec();
            if i % 3 == 0 && q.len() > 4 {
                q.remove(1 + rng.below(q.len() - 2));
            }
            q.into_iter().collect()
        })
        .collect()
}

fn latency(index: &Index, queries: &[String]) -> String {
    let mut times: Vec<f64> = queries
        .iter()
        .map(|q| {
            let t = Instant::now();
            std::hint::black_box(index.complete(q, 10));
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    format!(
        "p50 {:.3} ms, p99 {:.3} ms, max {:.2} ms",
        percentile(&times, 0.5),
        percentile(&times, 0.99),
        times[times.len() - 1]
    )
}

async fn open(url: &str) -> Database {
    let database = Database::open(url, Vec::<(String, String)>::new())
        .await
        .unwrap();
    match std::env::var("COMPLETR_CACHE_DIR") {
        Ok(dir) => database.with_cache_dir(dir).unwrap(),
        Err(_) => database,
    }
}

async fn load(corpus: &str, url: &str) {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(corpus).unwrap()).unwrap();
    let ds = open(url).await;
    let mut txn = ds.begin().await.unwrap();
    for (name, entry) in value["indexes"].as_object().unwrap() {
        let docs: Vec<Document> = entry["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let doc = Document::new(
                    d[0].as_u64().unwrap(),
                    d[1].as_str().unwrap(),
                    d[2].as_f64().unwrap() as f32,
                );
                d[3].as_array().unwrap().iter().fold(doc, |doc, a| {
                    let kind = if a[1].as_bool().unwrap() {
                        completr::AliasKind::Abbreviation
                    } else {
                        completr::AliasKind::Synonym
                    };
                    doc.with_alias(a[0].as_str().unwrap(), kind)
                })
            })
            .collect();
        let n = docs.len();
        let usage = start();
        let segment = build_segment(docs, []).unwrap();
        report(
            &format!(
                "build {name} ({n} docs, {:.1} MB)",
                segment.to_bytes().len() as f64 / 1_048_576.0
            ),
            usage,
        );
        txn.append(name, segment)
            .set_max_score(name, entry["max_score"].as_f64().unwrap());
    }
    let usage = start();
    let manifest = txn.commit().await.unwrap();
    report(&format!("upload + commit v{}", manifest.version), usage);
}

async fn open_all(url: &str) {
    let usage = start();
    let ds = open(url).await;
    report("open database (store, credentials)", usage);
    let usage = start();
    let latest = ds.latest_version().await.unwrap();
    report(&format!("list versions (latest v{latest})"), usage);
    let usage = start();
    ds.manifest(latest).await.unwrap();
    report("read manifest", usage);
    println!("  baseline rss {:.1} MB", rss_mb());
    let engine = Engine::new();
    let replica = Replica::new(ds.clone(), IndexOptions::default());
    let usage = start();
    let version = replica.sync(&engine).await.unwrap();
    report(&format!("cold load of all indexes (v{version:?})"), usage);
    for name in engine.names() {
        let index = engine.get(&name).unwrap();
        println!(
            "  {name}: {} docs, {} segments",
            index.len(),
            index.segments().len()
        );
    }
    let index = engine.get(&largest(&engine)).unwrap();
    let qs = queries(&index, 3000);
    let usage = start();
    let cold = latency(&index, &qs);
    report("3000 typed queries, cold short-query cache", usage);
    println!("  latency: {cold}; warm: {}", latency(&index, &qs));
}

fn largest(engine: &Engine) -> String {
    engine
        .names()
        .into_iter()
        .max_by_key(|n| engine.get(n).unwrap().len())
        .unwrap()
}

async fn rebuild(url: &str, name: &str) {
    let ds = open(url).await;
    let manifest = ds.latest().await.unwrap();
    let index = ds
        .open_index(&manifest, name, IndexOptions::default())
        .await
        .unwrap();
    let docs: Vec<Document> = index.documents().collect();
    drop(index);
    println!("  {} docs in memory, rss {:.1} MB", docs.len(), rss_mb());
    let usage = start();
    let segment = build_segment(docs, []).unwrap();
    report(
        &format!(
            "full segment build ({:.1} MB)",
            segment.size_bytes() as f64 / 1_048_576.0
        ),
        usage,
    );
    for (section, bytes) in segment.section_sizes() {
        println!("    {section:<18} {:>6.1} MB", bytes as f64 / 1_048_576.0);
    }
    let usage = start();
    let index = Index::new(
        vec![Arc::new(segment)],
        IndexOptions::default().max_score(1000.0),
    )
    .unwrap();
    report("compose index", usage);
    std::hint::black_box(index.len());
}

/// Half renames, a quarter new documents, a quarter deletes.
fn change_batch(
    docs: &[Document],
    next_id: &mut u64,
    size: usize,
    rng: &mut Rng,
    tag: &str,
) -> (Vec<Document>, Vec<u64>) {
    let mut upserts = Vec::new();
    let mut deletes = Vec::new();
    for i in 0..size {
        match i % 4 {
            0 | 1 => {
                let doc = &docs[rng.below(docs.len())];
                upserts.push({
                    let mut d = doc.clone();
                    d.text = format!("{} {tag}", doc.text);
                    d
                });
            }
            2 => {
                let template = &docs[rng.below(docs.len())];
                upserts.push({
                    let mut d = template.clone();
                    d.id = *next_id;
                    d.text = format!("{} {tag} new", template.text);
                    d
                });
                *next_id += 1;
            }
            _ => deletes.push(docs[rng.below(docs.len())].id),
        }
    }
    (upserts, deletes)
}

async fn incremental(url: &str, name: &str, sizes: &str) {
    let ds = open(url).await;
    let engine = Engine::new();
    let replica = Replica::new(ds.clone(), IndexOptions::default());
    replica.sync(&engine).await.unwrap();
    let base = engine.get(name).unwrap();
    let docs: Vec<Document> = base.documents().collect();
    let qs = queries(&base, 2000);
    println!(
        "  base: {} docs, {} segments; queries {}",
        base.len(),
        base.segments().len(),
        latency(&base, &qs)
    );
    let mut next_id = docs.iter().map(|d| d.id).max().unwrap() + 1_000_000;
    let mut rng = Rng(7);
    for (round, size) in sizes
        .split(',')
        .map(|s| s.parse::<usize>().unwrap())
        .enumerate()
    {
        let (upserts, deletes) =
            change_batch(&docs, &mut next_id, size, &mut rng, &format!("r{round}"));
        let usage = start();
        let segment = build_segment(upserts, deletes).unwrap();
        let bytes = segment.to_bytes().len();
        report(
            &format!(
                "batch {size}: build delta ({:.1} KB)",
                bytes as f64 / 1024.0
            ),
            usage,
        );
        let usage = start();
        let mut txn = ds.begin().await.unwrap();
        txn.append(name, segment);
        let manifest = txn.commit().await.unwrap();
        report(
            &format!("batch {size}: upload + commit v{}", manifest.version),
            usage,
        );
        let usage = start();
        replica.sync(&engine).await.unwrap();
        report(&format!("batch {size}: replica sync"), usage);
        let index = engine.get(name).unwrap();
        println!(
            "    now {} docs in {} segments; queries {}",
            index.len(),
            index.segments().len(),
            latency(&index, &qs)
        );
    }
}

/// Short-query latency right after an update, with the old index's hot queries carried over or not.
async fn carry(url: &str, name: &str) {
    let ds = open(url).await;
    let carry = std::env::var("COMPLETR_CARRY").map_or(1000, |v| v.parse().unwrap());
    let engine = Engine::new();
    let replica = Replica::new(
        ds.clone(),
        IndexOptions::default().carry_short_queries(carry),
    );
    replica.sync(&engine).await.unwrap();
    let base = engine.get(name).unwrap();
    let docs: Vec<Document> = base.documents().collect();
    let mut rng = Rng(5);
    let mut shorts: Vec<String> = (0..600)
        .map(|_| {
            docs[rng.below(docs.len())]
                .text
                .chars()
                .take(1 + rng.below(3))
                .collect()
        })
        .collect();
    shorts.sort();
    shorts.dedup();
    let time_all = |index: &Index| {
        let mut t: Vec<f64> = shorts
            .iter()
            .map(|q| {
                let s = Instant::now();
                std::hint::black_box(index.complete(q, 10));
                s.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        t.sort_by(f64::total_cmp);
        format!(
            "p50 {:.3} ms, p99 {:.3} ms, max {:.2} ms",
            percentile(&t, 0.5),
            percentile(&t, 0.99),
            t[t.len() - 1]
        )
    };
    println!(
        "  {} distinct short queries; first run on base: {}",
        shorts.len(),
        time_all(&base)
    );
    println!("  cached on base: {}", time_all(&base));
    let mut next_id = docs.iter().map(|d| d.id).max().unwrap() + 1_000_000;
    let (upserts, deletes) = change_batch(&docs, &mut next_id, 100, &mut rng, "carry");
    let mut txn = ds.begin().await.unwrap();
    txn.append_documents(name, upserts, deletes).unwrap();
    txn.commit().await.unwrap();
    let usage = start();
    replica.sync(&engine).await.unwrap();
    report("sync", usage);
    tokio::time::sleep(Duration::from_millis(3000)).await;
    println!(
        "  carry {carry}: new version, 3 s after sync: {}",
        time_all(&engine.get(name).unwrap())
    );
}

/// Producers submit batches to the inbox while `writers` compete for the ingestor lease.
async fn inbox(
    url: &str,
    name: &str,
    producers: usize,
    batches: usize,
    size: usize,
    writers: usize,
) {
    let ds = open(url).await;
    let base = ds
        .open_index(&ds.latest().await.unwrap(), name, IndexOptions::default())
        .await
        .unwrap();
    let docs = Arc::new(base.documents().collect::<Vec<_>>());
    let first_new = docs.iter().map(|d| d.id).max().unwrap() + 20_000_000;
    let start_version = ds.latest_version().await.unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut writer_tasks = Vec::new();
    for w in 0..writers {
        let (ds, stop) = (ds.clone(), stop.clone());
        writer_tasks.push(tokio::spawn(async move {
            let mut ingestor = completr::Ingestor::new(ds, format!("bench-ingestor-{w}"));
            let (mut commits, mut folded) = (0usize, 0usize);
            while !stop.load(Ordering::Relaxed) {
                match ingestor.run_once().await {
                    Ok(completr::IngestStep::Committed { change_sets, .. }) => {
                        commits += 1;
                        folded += change_sets;
                    }
                    Ok(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    Err(Error::Conflict(_)) => {}
                    Err(e) => panic!("ingestor: {e}"),
                }
            }
            ingestor.release().await.unwrap();
            (commits, folded)
        }));
    }
    let usage = start();
    let latencies = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for p in 0..producers {
        let (ds, docs, latencies, name) =
            (ds.clone(), docs.clone(), latencies.clone(), name.to_owned());
        tasks.push(tokio::spawn(async move {
            let mut rng = Rng(77 + p as u64);
            for b in 0..batches {
                let first = first_new + ((p * batches + b) * size) as u64;
                let upserts: Vec<Document> = (0..size as u64)
                    .map(|i| {
                        let mut d = docs[rng.below(docs.len())].clone();
                        d.id = first + i;
                        d
                    })
                    .collect();
                let mut batch = completr::ChangeSet::new();
                batch.upsert(&name, upserts);
                let t = Instant::now();
                ds.submit(batch).await.unwrap();
                latencies
                    .lock()
                    .unwrap()
                    .push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let submitted = usage.wall.elapsed();
    while ds.pending_change_sets().await.unwrap() > 0 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let drained = report(
        &format!("{producers} producers x {batches} batches of {size}, {writers} writers"),
        usage,
    );
    stop.store(true, Ordering::Relaxed);
    let (mut commits, mut folded) = (0, 0);
    for w in writer_tasks {
        let (c, f) = w.await.unwrap();
        commits += c;
        folded += f;
    }
    let mut lat = latencies.lock().unwrap().clone();
    lat.sort_by(f64::total_cmp);
    let total = producers * batches;
    println!(
        "    submit latency p50 {:.0} ms, p99 {:.0} ms; {total} batches submitted in {:.1} s ({:.1}/s), all committed after {:.1} s",
        percentile(&lat, 0.5),
        percentile(&lat, 0.99),
        submitted.as_secs_f64(),
        total as f64 / submitted.as_secs_f64(),
        drained.as_secs_f64()
    );
    let version = ds.latest_version().await.unwrap();
    println!(
        "    {commits} commits folded {folded} batches ({:.1} per commit); versions {start_version} -> {version}",
        folded as f64 / commits.max(1) as f64
    );
    let index = ds
        .open_index(&ds.latest().await.unwrap(), name, IndexOptions::default())
        .await
        .unwrap();
    let missing = (0..(total * size) as u64)
        .filter(|i| index.document(first_new + i).is_none())
        .count();
    println!("    missing documents: {missing} of {}", total * size);
}

async fn compact(url: &str, name: &str) {
    let ds = open(url).await;
    let before = ds.latest().await.unwrap();
    let segments = before.indexes[name].segments.len();
    let reference = ds
        .open_index(&before, name, IndexOptions::default())
        .await
        .unwrap();
    let qs = queries(&reference, 1000);
    let expected: Vec<_> = qs.iter().map(|q| reference.complete(q, 10)).collect();
    println!(
        "  {segments} segments, {} live docs, queries {}",
        reference.len(),
        latency(&reference, &qs)
    );
    drop(reference);
    println!("  rss before compaction {:.1} MB", rss_mb());

    let tiered = CompactionPolicy::default().max_hidden_fraction(1.0);
    let usage = start();
    let step = ds.compact(name, &tiered).await.unwrap();
    report(
        &format!(
            "tiered compaction -> {:?} segments",
            step.map(|m| m.indexes[name].segments.len())
        ),
        usage,
    );

    let usage = start();
    let full = ds
        .compact(name, &tiered.max_segments(1))
        .await
        .unwrap()
        .unwrap();
    report(
        "full compaction (download, merge, build, upload, commit)",
        usage,
    );

    let index = ds
        .open_index(&full, name, IndexOptions::default())
        .await
        .unwrap();
    let same = qs
        .iter()
        .zip(&expected)
        .all(|(q, e)| index.complete(q, 10) == *e);
    println!(
        "  after: {} segments, results identical: {same}, queries {}",
        full.indexes[name].segments.len(),
        latency(&index, &qs)
    );
}

async fn concurrent(
    url: &str,
    name: &str,
    writers: usize,
    batches: usize,
    size: usize,
    strict: bool,
) {
    let ds = open(url).await;
    let base_manifest = ds.latest().await.unwrap();
    let base = ds
        .open_index(&base_manifest, name, IndexOptions::default())
        .await
        .unwrap();
    let docs = Arc::new(base.documents().collect::<Vec<_>>());
    let shared: Vec<u64> = docs.iter().take(20).map(|d| d.id).collect();
    let first_new = docs.iter().map(|d| d.id).max().unwrap() + 10_000_000;
    let records = Arc::new(Mutex::new(
        Vec::<(u64, usize, usize, Vec<u64>, Vec<u64>)>::new(),
    ));
    let (latencies, attempts, conflicts) = (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicU64::new(0)),
    );
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let compactor = {
        let (ds, stop, name) = (ds.clone(), stop.clone(), name.to_owned());
        tokio::spawn(async move {
            let (mut done, mut lost) = (0, 0);
            while !stop.load(Ordering::Relaxed) {
                match ds
                    .compact(&name, &CompactionPolicy::default().fanout(4))
                    .await
                {
                    Ok(Some(_)) => done += 1,
                    Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
                    Err(Error::Conflict(_)) => lost += 1,
                    Err(e) => panic!("compactor: {e}"),
                }
            }
            (done, lost)
        })
    };
    let replica = {
        let (ds, stop) = (ds.clone(), stop.clone());
        tokio::spawn(async move {
            let engine = Engine::new();
            let replica = Replica::new(ds, IndexOptions::default());
            let mut syncs = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                if replica.sync(&engine).await.unwrap().is_some() {
                    syncs.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            syncs
        })
    };

    let usage = start();
    let mut tasks = Vec::new();
    for ingestor in 0..writers {
        let (ds, docs, shared, records, latencies, attempts, conflicts, name) = (
            ds.clone(),
            docs.clone(),
            shared.clone(),
            records.clone(),
            latencies.clone(),
            attempts.clone(),
            conflicts.clone(),
            name.to_owned(),
        );
        tasks.push(tokio::spawn(async move {
            let mut rng = Rng(1000 + ingestor as u64);
            for batch in 0..batches {
                let first = first_new + ((ingestor * batches + batch) * size) as u64;
                let mut upserts: Vec<Document> = (0..size as u64)
                    .map(|i| {
                        let mut d = docs[rng.below(docs.len())].clone();
                        d.id = first + i;
                        d
                    })
                    .collect();
                let hot = shared[rng.below(shared.len())];
                upserts.push({
                    let mut d = docs.iter().find(|d| d.id == hot).unwrap().clone();
                    d.text = format!("hot w{ingestor} b{batch}");
                    d
                });
                let deletes = vec![docs[rng.below(docs.len())].id];
                let ids: Vec<u64> = upserts.iter().map(|d| d.id).collect();
                let t = Instant::now();
                let mut txn = ds.begin().await.unwrap();
                txn.append_documents(&name, upserts, deletes.clone())
                    .unwrap()
                    .strict(strict);
                match txn.commit_with_attempts().await {
                    Ok((manifest, n)) => {
                        latencies
                            .lock()
                            .unwrap()
                            .push(t.elapsed().as_secs_f64() * 1000.0);
                        attempts.lock().unwrap().push(n);
                        records.lock().unwrap().push((
                            manifest.version,
                            ingestor,
                            batch,
                            ids,
                            deletes,
                        ));
                    }
                    Err(Error::Conflict(_)) => {
                        conflicts.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => panic!("ingestor {ingestor}: {e}"),
                }
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let wall = report(
        &format!(
            "{writers} writers x {batches} batches of {size}{}",
            if strict { " (strict)" } else { "" }
        ),
        usage,
    );
    stop.store(true, Ordering::Relaxed);
    let (compactions, compactions_lost) = compactor.await.unwrap();
    let syncs = replica.await.unwrap();

    let mut latencies = latencies.lock().unwrap().clone();
    latencies.sort_by(f64::total_cmp);
    let attempts = attempts.lock().unwrap().clone();
    let committed = latencies.len();
    println!(
        "    committed {committed}, conflicts {}, {:.1} commits/s; commit latency p50 {:.0} ms, p99 {:.0} ms, max {:.0} ms",
        conflicts.load(Ordering::Relaxed),
        committed as f64 / wall.as_secs_f64(),
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.99),
        latencies[latencies.len() - 1]
    );
    println!(
        "    manifest writes per commit: mean {:.2}, max {}; compactions {compactions} (lost to conflicts {compactions_lost}); replica syncs {} (mean {:.0} ms)",
        attempts.iter().sum::<usize>() as f64 / attempts.len() as f64,
        attempts.iter().max().unwrap(),
        syncs.len(),
        syncs.iter().sum::<f64>() / syncs.len().max(1) as f64
    );

    let manifest = ds.latest().await.unwrap();
    let index = ds
        .open_index(&manifest, name, IndexOptions::default())
        .await
        .unwrap();
    let mut records = records.lock().unwrap().clone();
    records.sort();
    let mut missing = 0;
    let mut hot_expected = std::collections::HashMap::new();
    let mut deleted = std::collections::HashSet::new();
    for (_, ingestor, batch, ids, deletes) in &records {
        for id in ids {
            deleted.remove(id);
        }
        deleted.extend(deletes.iter().copied());
        for &id in &ids[..ids.len() - 1] {
            if index.document(id).is_none() {
                missing += 1;
            }
        }
        hot_expected.insert(*ids.last().unwrap(), format!("hot w{ingestor} b{batch}"));
    }
    let hot_wrong = hot_expected
        .iter()
        .filter(|(id, text)| {
            !deleted.contains(id) && index.document(**id).map(|d| d.text) != Some(text.to_string())
        })
        .count();
    let deleted_alive = deleted
        .iter()
        .filter(|id| index.document(**id).is_some())
        .count();
    println!(
        "    final v{} with {} segments: missing inserts {missing}, wrong last-ingestor values {hot_wrong}, deleted still visible {deleted_alive}",
        manifest.version,
        manifest.indexes[name].segments.len()
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "load" => load(&args[2], &args[3]).await,
        "open" => open_all(&args[2]).await,
        "rebuild" => rebuild(&args[2], &args[3]).await,
        "incremental" => incremental(&args[2], &args[3], &args[4]).await,
        "compact" => compact(&args[2], &args[3]).await,
        "carry" => carry(&args[2], &args[3]).await,
        "inbox" => {
            let n = |i: usize| args[i].parse().unwrap();
            inbox(&args[2], &args[3], n(4), n(5), n(6), n(7)).await
        }
        "concurrent" => {
            let n = |i: usize| args[i].parse().unwrap();
            concurrent(
                &args[2],
                &args[3],
                n(4),
                n(5),
                n(6),
                args.get(7).is_some_and(|s| s == "strict"),
            )
            .await
        }
        other => panic!("unknown phase {other}"),
    }
}

fn build_segment(
    documents: impl IntoIterator<Item = Document>,
    deletes: impl IntoIterator<Item = u64>,
) -> Result<Segment, completr::Error> {
    use completr::{
        Dictionary::{Fst, Trie},
        Layout,
    };
    let layout = match std::env::var("COMPLETR_LAYOUT").as_deref() {
        Ok("fst") => Layout::uniform(Fst),
        Ok("trie") => Layout::uniform(Trie),
        Ok("compact") => Layout::uniform(completr::Dictionary::CompactTrie),
        Ok("default") | Err(_) => Layout::default(),
        Ok(other) => panic!("unknown layout {other}"),
    };
    Segment::build_with(
        completr::BuildOptions::default().layout(layout),
        documents,
        deletes,
    )
}
