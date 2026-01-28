use anyhow::Result;
use clap::Parser;
use codestory_index::{IndexResult, get_language_for_ext, index_file};
use codestory_storage::Storage;
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::path::PathBuf;
use std::thread;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the directory or file to index
    #[arg(short, long, default_value = ".")]
    path: PathBuf,

    /// Path to the SQLite database
    #[arg(short, long)]
    db: Option<PathBuf>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // 1. Initialize Storage (Single Writer)
    let mut storage = if let Some(db_path) = args.db {
        Storage::open(db_path)?
    } else {
        Storage::open("codestory.db")?
    };

    println!("Indexing workspace: {:?}", args.path);

    // 2. Discover Files
    let walker = ignore::WalkBuilder::new(&args.path)
        .standard_filters(true)
        .build();

    let mut files_to_index = Vec::new();
    for result in walker {
        match result {
            Ok(entry) => {
                if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    let path = entry.path().to_path_buf();
                    files_to_index.push(path);
                }
            }
            Err(err) => {
                eprintln!("Error walking directory: {}", err);
            }
        }
    }

    println!("Found {} files to process.", files_to_index.len());

    // 3. Setup Channels and Writer Thread
    // We use a bounded channel to provide backpressure if the writer is slow
    let (tx, rx) = bounded::<IndexResult>(100);

    let writer_handle = thread::spawn(move || -> Result<usize> {
        let mut count = 0;
        for result in rx {
            // Write nodes, edges, occurrences
            if !result.nodes.is_empty() {
                storage.insert_nodes_batch(&result.nodes)?;
            }
            if !result.occurrences.is_empty() {
                storage.insert_occurrences_batch(&result.occurrences)?;
            }
            if !result.edges.is_empty() {
                storage.insert_edges_batch(&result.edges)?;
            }
            count += 1;
        }
        Ok(count)
    });

    // 4. Parallel Indexing
    let files_processed = files_to_index
        .par_iter()
        .filter_map(|path| {
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if let Some((lang, lang_name, graph_query)) = get_language_for_ext(ext) {
                if let Ok(source) = std::fs::read_to_string(path) {
                    // println!("Indexing: {:?}", path); // Commented out to reduce noise in parallel output
                    match index_file(path, &source, lang, lang_name, graph_query, None, None) {
                        Ok(result) => Some(result),
                        Err(e) => {
                            eprintln!("Error indexing {:?}: {}", path, e);
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        })
        .map(|result| {
            tx.send(result).unwrap(); // Send to writer
        })
        .count();

    // Drop the sender to close the channel so the writer thread exits
    drop(tx);

    // 5. Wait for Writer
    let files_stored = writer_handle.join().unwrap()?;

    println!("Processed {} files.", files_processed);
    println!("Stored {} files in database.", files_stored);

    // Re-open storage to verify counts (since storage was moved to thread)
    // Actually, we can't easily query the same storage struct since it was moved.
    // We can open a NEW connection or just rely on the logs.

    Ok(())
}
