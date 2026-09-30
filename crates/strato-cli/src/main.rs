//! `strato`: inspect and operate strato databases from the command line.

use std::io::BufRead;
use std::time::Duration;

use clap::{Parser, Subcommand};
use strato::{
    CleanupPolicy, CompactionPolicy, Database, Document, IndexOptions, IngestStep, Ingestor,
    SearchOptions, Segment,
};

#[derive(Parser)]
#[command(
    name = "strato",
    version,
    about = "Operate strato databases: a local path or an s3://, gs://, az:// URL"
)]
struct Cli {
    /// Database URL or path.
    #[arg(env = "STRATO_URL")]
    url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Versions, indexes, segments and sizes.
    Inspect {
        /// Print the latest manifest as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Completes a query against the latest version.
    Complete {
        index: String,
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Only documents tagged with any of these contexts (comma separated).
        #[arg(long, value_delimiter = ',')]
        contexts: Vec<String>,
    },
    /// Imports JSON Lines documents (`id`, `text`, `popularity`, `synonyms`, `abbreviations`,
    /// `contexts`) from a file or `-` for stdin, in one commit.
    Import {
        index: String,
        file: String,
        /// Replace the whole index instead of adding to it.
        #[arg(long)]
        overwrite: bool,
    },
    /// Compacts an index, by default until nothing is due.
    Compact {
        index: String,
        /// Run one step only.
        #[arg(long)]
        once: bool,
    },
    /// Deletes old versions and segment files no retained version references.
    Cleanup {
        #[arg(long, default_value_t = 10)]
        keep_versions: usize,
        #[arg(long, default_value_t = 3600)]
        older_than_seconds: u64,
    },
    /// Runs an ingestor: commits submitted change sets while it holds the lease.
    Ingest {
        /// Name of this ingestor in the lease.
        #[arg(long, default_value = "strato-cli")]
        owner: String,
        /// Seconds between rounds; 0 runs one round.
        #[arg(long, default_value_t = 1.0)]
        interval: f64,
    },
}

fn document(line: &str) -> Result<Document, String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let text = value["text"].as_str().ok_or("a document needs a text")?;
    let popularity = value["popularity"].as_f64().unwrap_or(0.0) as f32;
    let mut doc = match &value["id"] {
        serde_json::Value::String(key) => Document::keyed(key.as_str(), text, popularity),
        serde_json::Value::Number(id) => Document::new(
            id.as_u64().ok_or("ids must be non-negative")?,
            text,
            popularity,
        ),
        _ => return Err("a document needs an int or string id".into()),
    };
    let strings = |field: &str| -> Vec<String> {
        value[field]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    for s in strings("synonyms") {
        doc = doc.with_synonym(s);
    }
    for a in strings("abbreviations") {
        doc = doc.with_abbreviation(a);
    }
    for c in strings("contexts") {
        doc = doc.with_context(c);
    }
    Ok(doc)
}

fn read_documents(file: &str) -> Result<Vec<Document>, Box<dyn std::error::Error>> {
    let reader: Box<dyn BufRead> = if file == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(std::io::BufReader::new(std::fs::File::open(file)?))
    };
    let mut docs = Vec::new();
    for (n, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        docs.push(document(&line).map_err(|e| format!("line {}: {e}", n + 1))?);
    }
    Ok(docs)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("STRATO_LOG")
                .unwrap_or_else(|_| "strato=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let db = Database::open(&cli.url, Vec::<(String, String)>::new()).await?;
    match cli.command {
        Command::Inspect { json } => {
            let manifest = db.latest().await?;
            if json {
                println!("{}", manifest.to_json());
                return Ok(());
            }
            let versions = db.versions().await?;
            println!(
                "{}: version {} ({} kept), {} indexes",
                cli.url,
                manifest.version,
                versions.len(),
                manifest.indexes.len()
            );
            let mut names: Vec<_> = manifest.indexes.keys().collect();
            names.sort();
            for name in names {
                let entry = &manifest.indexes[name];
                let documents: u64 = entry.segments.iter().map(|s| s.documents).sum();
                let bytes: u64 = entry.segments.iter().map(|s| s.bytes).sum();
                println!(
                    "  {name}: {} segments, {documents} documents stored, {}",
                    entry.segments.len(),
                    megabytes(bytes)
                );
            }
        }
        Command::Complete {
            index,
            query,
            limit,
            contexts,
        } => {
            let manifest = db.latest().await?;
            let index = db
                .open_index(&manifest, &index, IndexOptions::default())
                .await?;
            let options = SearchOptions::new(limit).contexts(contexts);
            for s in index.complete_with(&query, &options) {
                let id = s.key.clone().unwrap_or_else(|| s.id.to_string());
                println!("{:.4}  {:<12}  {id}  {}", s.score, s.kind.as_str(), s.text);
            }
        }
        Command::Import {
            index,
            file,
            overwrite,
        } => {
            let docs = read_documents(&file)?;
            let n = docs.len();
            let mut txn = db.begin().await?;
            if overwrite {
                txn.overwrite(&index, Segment::build_with(db.build_options(), docs, [])?);
            } else {
                txn.append_documents(&index, docs, [])?;
            }
            let manifest = txn.commit().await?;
            println!(
                "imported {n} documents into {index} as version {}",
                manifest.version
            );
        }
        Command::Compact { index, once } => {
            let policy = CompactionPolicy::default();
            let result = if once {
                db.compact(&index, &policy).await?
            } else {
                db.compact_all(&index, &policy).await?
            };
            match result {
                Some(manifest) => println!("compacted {index} into version {}", manifest.version),
                None => println!("nothing to compact in {index}"),
            }
        }
        Command::Cleanup {
            keep_versions,
            older_than_seconds,
        } => {
            let policy = CleanupPolicy::default()
                .keep_versions(keep_versions)
                .older_than(Duration::from_secs(older_than_seconds));
            let stats = db.cleanup(&policy).await?;
            println!(
                "removed {} versions and {} segments ({})",
                stats.versions_removed,
                stats.segments_removed,
                megabytes(stats.bytes_removed)
            );
        }
        Command::Ingest { owner, interval } => {
            let mut ingestor = Ingestor::new(db, owner);
            loop {
                match ingestor.run_once().await? {
                    IngestStep::Committed {
                        version,
                        change_sets,
                        documents,
                    } => println!(
                        "version {version}: {change_sets} change sets, {documents} documents"
                    ),
                    IngestStep::Standby if interval <= 0.0 => {
                        println!("another ingestor holds the lease")
                    }
                    IngestStep::Idle if interval <= 0.0 => println!("nothing to ingest"),
                    _ => {}
                }
                if interval <= 0.0 {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs_f64(interval)) => {}
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            ingestor.release().await?;
        }
    }
    Ok(())
}
