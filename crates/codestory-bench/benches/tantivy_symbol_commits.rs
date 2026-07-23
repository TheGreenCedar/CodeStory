use codestory_contracts::graph::NodeId;
use codestory_runtime::benchmark_support::{SearchEngine, SymbolIndexWriteStats};
use serde::Serialize;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

const DEFAULT_DOCUMENTS: usize = 262_144;
const COMMIT_WINDOW: usize = 8_192;
const DEFAULT_REPEATS: usize = 3;

#[derive(Debug, Clone, Serialize)]
struct Measurement {
    strategy: &'static str,
    repeat: usize,
    elapsed_ms: u128,
    docs_written: usize,
    writers: usize,
    commits: usize,
    reloads: usize,
    segments: usize,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    documents: usize,
    commit_window: usize,
    repeats: usize,
    filesystem_root: String,
    legacy_median_ms: u128,
    generation_median_ms: u128,
    measurements: Vec<Measurement>,
}

fn main() {
    let document_count = env_usize("CODESTORY_TANTIVY_BENCH_DOCUMENTS", DEFAULT_DOCUMENTS);
    let repeats = env_usize("CODESTORY_TANTIVY_BENCH_REPEATS", DEFAULT_REPEATS).max(1);
    let root = std::env::var_os("CODESTORY_TANTIVY_BENCH_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).expect("create Tantivy benchmark root");
    let nodes = (0..document_count)
        .map(|index| {
            (
                NodeId(index as i64 + 1),
                format!("module_{:06}::Symbol_{:09}", index % 4096, index),
            )
        })
        .collect::<Vec<_>>();

    let mut measurements = Vec::with_capacity(repeats.saturating_mul(2));
    for repeat in 0..repeats {
        if repeat % 2 == 0 {
            measurements.push(run_legacy(&root, &nodes, repeat));
            measurements.push(run_generation(&root, &nodes, repeat));
        } else {
            measurements.push(run_generation(&root, &nodes, repeat));
            measurements.push(run_legacy(&root, &nodes, repeat));
        }
    }

    let report = BenchmarkReport {
        documents: document_count,
        commit_window: COMMIT_WINDOW,
        repeats,
        filesystem_root: root.display().to_string(),
        legacy_median_ms: median_ms(&measurements, "legacy_window_commits"),
        generation_median_ms: median_ms(&measurements, "generation_single_commit"),
        measurements,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize Tantivy benchmark report")
    );
}

fn run_legacy(root: &Path, nodes: &[(NodeId, String)], repeat: usize) -> Measurement {
    let directory = tempfile::Builder::new()
        .prefix("codestory-tantivy-legacy-")
        .tempdir_in(root)
        .expect("legacy Tantivy benchmark directory");
    let started = Instant::now();
    let mut engine = SearchEngine::new(Some(directory.path())).expect("create legacy engine");
    let mut stats = SymbolIndexWriteStats::default();
    for chunk in nodes.chunks(COMMIT_WINDOW) {
        engine
            .index_nodes(chunk.to_vec())
            .expect("legacy symbol-index window");
        stats.docs_written = stats.docs_written.saturating_add(chunk.len());
        stats.writer_count = stats.writer_count.saturating_add(1);
        stats.commit_count = stats.commit_count.saturating_add(1);
        stats.reload_count = stats.reload_count.saturating_add(1);
    }
    let elapsed_ms = started.elapsed().as_millis();
    let measurement = measurement(
        "legacy_window_commits",
        repeat,
        elapsed_ms,
        stats,
        &engine,
        directory.path(),
    );
    black_box(engine);
    measurement
}

fn run_generation(root: &Path, nodes: &[(NodeId, String)], repeat: usize) -> Measurement {
    let directory = tempfile::Builder::new()
        .prefix("codestory-tantivy-generation-")
        .tempdir_in(root)
        .expect("generation Tantivy benchmark directory");
    let started = Instant::now();
    let mut engine = SearchEngine::new(Some(directory.path())).expect("create generation engine");
    let stats = {
        let mut session = engine
            .begin_symbol_index()
            .expect("start generation writer");
        for chunk in nodes.chunks(COMMIT_WINDOW) {
            session
                .add_nodes(chunk.iter().cloned())
                .expect("generation symbol-index window");
        }
        session.finish().expect("commit generation writer")
    };
    let elapsed_ms = started.elapsed().as_millis();
    let measurement = measurement(
        "generation_single_commit",
        repeat,
        elapsed_ms,
        stats,
        &engine,
        directory.path(),
    );
    black_box(engine);
    measurement
}

fn measurement(
    strategy: &'static str,
    repeat: usize,
    elapsed_ms: u128,
    stats: SymbolIndexWriteStats,
    engine: &SearchEngine,
    directory: &Path,
) -> Measurement {
    Measurement {
        strategy,
        repeat,
        elapsed_ms,
        docs_written: stats.docs_written,
        writers: stats.writer_count,
        commits: stats.commit_count,
        reloads: stats.reload_count,
        segments: engine.tantivy_segment_count(),
        bytes: directory_bytes(directory),
    }
}

fn directory_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .expect("read Tantivy benchmark directory")
        .map(|entry| {
            let entry = entry.expect("read Tantivy benchmark entry");
            let metadata = entry.metadata().expect("read Tantivy benchmark metadata");
            if metadata.is_dir() {
                directory_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

fn median_ms(measurements: &[Measurement], strategy: &str) -> u128 {
    let mut values = measurements
        .iter()
        .filter(|measurement| measurement.strategy == strategy)
        .map(|measurement| measurement.elapsed_ms)
        .collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
