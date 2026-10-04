//! Memory under frequent updates: soak <database url> <index> <iterations>
//!
//! Each iteration submits a batch, runs a ingestor round, syncs a replica and queries, with
//! compaction and cleanup on a schedule. Live heap is counted exactly by the allocator below.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use completr::{
    ChangeSet, CleanupPolicy, Database, Document, Engine, IndexOptions, Ingestor, Replica,
};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

static INNER: System = System;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { INNER.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { INNER.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        unsafe { INNER.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn mb(bytes: isize) -> f64 {
    bytes as f64 / 1_048_576.0
}

/// Resident MB of heap regions (malloc zones or mimalloc's anonymous VM) and of mapped files, and the number of mapped segment files.
fn vm_breakdown() -> (f64, f64, usize) {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("vmmap")
        .args(["--summary", &pid])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let resident = |prefix: &str| -> f64 {
        out.lines()
            .filter(|l| l.starts_with(prefix))
            .filter_map(|l| {
                l.split_whitespace()
                    .nth(prefix.split_whitespace().count() + 1)
            })
            .map(|v| {
                let (num, unit) = v.split_at(v.len() - 1);
                let n: f64 = num.parse().unwrap_or(0.0);
                match unit {
                    "G" => n * 1024.0,
                    "M" => n,
                    "K" => n / 1024.0,
                    _ => 0.0,
                }
            })
            .sum()
    };
    let full = std::process::Command::new("vmmap")
        .arg(&pid)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let mut segs: Vec<&str> = full
        .lines()
        .filter(|l| l.contains(".seg"))
        .filter_map(|l| l.split_whitespace().last())
        .collect();
    segs.sort_unstable();
    segs.dedup();
    let heap = resident("MALLOC_") + resident("VM_ALLOCATE");
    (heap, resident("mapped file"), segs.len())
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

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (url, name, iterations) = (&args[1], &args[2], args[3].parse::<usize>().unwrap());
    // "write": submit and write only; "serve": sync and query only; default: both in one process.
    let mode = args.get(4).map_or("both", String::as_str);
    let ds = Database::open(url, Vec::<(String, String)>::new())
        .await
        .unwrap();
    let engine = Engine::new();
    let replica = Replica::new(ds.clone(), IndexOptions::default());
    replica.sync(&engine).await.unwrap();
    let docs: Vec<Document> = engine.get(name).unwrap().documents().collect();
    let mut ingestor = Ingestor::new(ds.clone(), "soak");
    let mut state = 99u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let queries: Vec<String> = (0..50)
        .map(|i| {
            let text: Vec<char> = docs[(next() % docs.len() as u64) as usize]
                .text
                .chars()
                .collect();
            let len = if i % 2 == 0 {
                1 + (next() % 3) as usize
            } else {
                4 + (next() % 9) as usize
            };
            text[..len.min(text.len())].iter().collect()
        })
        .collect();
    let started = Instant::now();
    println!(
        "{:>6} {:>9} {:>8} {:>11} {:>11} {:>9} {:>9} {:>9}",
        "iter", "heap MB", "RSS MB", "malloc MB", "mapped MB", "seg maps", "segments", "versions"
    );
    if mode == "serve" {
        for i in 0..=iterations {
            replica.sync(&engine).await.unwrap();
            let index = engine.get(name).unwrap();
            for q in &queries {
                std::hint::black_box(index.complete(q, 10));
            }
            drop(index);
            if i % 50 == 0 {
                let (malloc, mapped, maps) = vm_breakdown();
                let segments = engine.get(name).unwrap().segments().len();
                println!(
                    "{i:>6} {:>9.1} {:>8.1} {malloc:>11.1} {mapped:>11.1} {maps:>9} {segments:>9} {:>9}",
                    mb(LIVE.load(Ordering::Relaxed)),
                    rss_mb(),
                    replica.version().await
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        return;
    }
    for i in 0..=iterations {
        let mut batch = ChangeSet::new();
        let upserts: Vec<Document> = (0..20)
            .map(|_| {
                let doc = &docs[(next() % docs.len() as u64) as usize];
                {
                    let mut d = doc.clone();
                    d.text = format!("{} r{}", doc.text, next() % 1000);
                    d
                }
            })
            .collect();
        batch.upsert(name, upserts);
        ds.submit(batch).await.unwrap();
        ingestor.run_once().await.unwrap();
        if mode == "both" {
            replica.sync(&engine).await.unwrap();
            let index = engine.get(name).unwrap();
            for q in &queries {
                std::hint::black_box(index.complete(q, 10));
            }
            drop(index);
        }
        if i % 100 == 99 {
            ds.cleanup(
                &CleanupPolicy::default()
                    .keep_versions(5)
                    .older_than(Duration::ZERO),
            )
            .await
            .unwrap();
        }
        if i % 100 == 0 {
            // Let background cache refills finish before measuring.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let segments = engine.get(name).unwrap().segments().len();
            let versions = ds.versions().await.unwrap().len();
            let (malloc, mapped, maps) = vm_breakdown();
            println!(
                "{i:>6} {:>9.1} {:>8.1} {malloc:>11.1} {mapped:>11.1} {maps:>9} {segments:>9} {versions:>9}",
                mb(LIVE.load(Ordering::Relaxed)),
                rss_mb()
            );
        }
    }
    println!(
        "{iterations} iterations in {:.1} s",
        started.elapsed().as_secs_f64()
    );
}
