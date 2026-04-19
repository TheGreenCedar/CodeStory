use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;

#[derive(Debug, Serialize)]
struct RepoE2eStats {
    project_root: String,
    cache_dir: String,
    storage_path: String,
    search_dir: String,
    embed_batch_size: u32,
    search_dir_unchanged: bool,
    index_seconds: f64,
    graph_phase_seconds: f64,
    semantic_phase_seconds: f64,
    semantic_docs_reused: u64,
    semantic_docs_embedded: u64,
    semantic_docs_pending: u64,
    semantic_docs_stale: u64,
    ground_seconds: f64,
    search_seconds: f64,
    symbol_seconds: f64,
    trail_seconds: f64,
    snippet_seconds: f64,
    index: IndexStats,
    ground: GroundStats,
    search: SearchStats,
    symbol: SymbolStats,
    trail: TrailStats,
    snippet: SnippetStats,
}

#[derive(Debug, Serialize)]
struct IndexStats {
    node_count: u64,
    edge_count: u64,
    file_count: u64,
    error_count: u64,
    retrieval_mode: String,
    semantic_doc_count: u64,
}

#[derive(Debug, Serialize)]
struct GroundStats {
    retrieval_mode: String,
    root_symbols: usize,
    file_digests: usize,
    coverage_total_files: u64,
}

#[derive(Debug, Serialize)]
struct SearchStats {
    query: String,
    retrieval_mode: String,
    semantic_doc_count: u64,
    indexed_symbol_hits: usize,
    repo_text_hits: usize,
    top_symbol_id: String,
    top_symbol_name: String,
}

#[derive(Debug, Serialize)]
struct SymbolStats {
    display_name: String,
    related_hits: usize,
    edge_digest_entries: usize,
}

#[derive(Debug, Serialize)]
struct TrailStats {
    focus_display_name: String,
    node_count: usize,
    edge_count: usize,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct SnippetStats {
    path: String,
    line: u64,
    snippet_lines: usize,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn release_cli_binary() -> PathBuf {
    repo_root()
        .join("target")
        .join("release")
        .join(format!("codestory-cli{}", std::env::consts::EXE_SUFFIX))
}

fn search_dir_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().expect("storage parent");
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("storage file stem");
    parent.join(format!("{stem}.search"))
}

fn run_cli_json(
    binary: &Path,
    project_root: &Path,
    cache_dir: &Path,
    args: &[String],
) -> (f64, Value) {
    let started = Instant::now();
    let output = Command::new(binary)
        .current_dir(project_root)
        .args(args)
        .arg("--project")
        .arg(project_root)
        .arg("--cache-dir")
        .arg(cache_dir)
        .output()
        .expect("run codestory-cli");
    let seconds = started.elapsed().as_secs_f64();
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        seconds,
        serde_json::from_slice(&output.stdout).expect("parse json output"),
    )
}

fn json_path<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields
                .get(*key)
                .unwrap_or_else(|| panic!("missing object key {key:?} at path {path:?}")),
            Value::Array(items) => {
                let index = key.parse::<usize>().unwrap_or_else(|_| {
                    panic!("expected array index at path {path:?}, got {key:?}")
                });
                items
                    .get(index)
                    .unwrap_or_else(|| panic!("missing array index {index} at path {path:?}"))
            }
            _ => panic!("cannot descend into non-container at path {path:?}, key {key:?}"),
        };
    }
    current
}

fn string_field<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    let current = json_path(value, path);
    current
        .as_str()
        .unwrap_or_else(|| panic!("expected string at path {:?}", path))
}

fn u64_field(value: &Value, path: &[&str]) -> u64 {
    let current = json_path(value, path);
    current
        .as_u64()
        .unwrap_or_else(|| panic!("expected u64 at path {:?}", path))
}

fn optional_u64_field(value: &Value, path: &[&str]) -> u64 {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => match fields.get(*key) {
                Some(value) => value,
                None => return 0,
            },
            Value::Array(items) => {
                let Ok(index) = key.parse::<usize>() else {
                    return 0;
                };
                match items.get(index) {
                    Some(value) => value,
                    None => return 0,
                }
            }
            _ => return 0,
        };
    }
    current.as_u64().unwrap_or(0)
}

fn bool_field(value: &Value, path: &[&str]) -> bool {
    let current = json_path(value, path);
    current
        .as_bool()
        .unwrap_or_else(|| panic!("expected bool at path {:?}", path))
}

fn array_len(value: &Value, path: &[&str]) -> usize {
    let current = json_path(value, path);
    current
        .as_array()
        .unwrap_or_else(|| panic!("expected array at path {:?}", path))
        .len()
}

#[test]
#[ignore = "repo-scale release e2e; run with cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture after cargo build --release -p codestory-cli"]
fn codestory_repo_release_e2e_emits_stats() {
    let project_root = repo_root();
    let binary = release_cli_binary();
    assert!(
        binary.is_file(),
        "missing release binary at {}. Run `cargo build --release -p codestory-cli` first.",
        binary.display()
    );

    let cache_dir = tempdir().expect("cache dir");

    let (index_seconds, index_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "index".to_string(),
            "--refresh".to_string(),
            "full".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let storage_path = PathBuf::from(string_field(&index_json, &["storage_path"]));
    let search_dir = search_dir_for_storage(storage_path.as_path());
    let search_dir_before = fs::metadata(&search_dir)
        .expect("search dir metadata before reads")
        .modified()
        .expect("search dir modified time before reads");

    let (ground_seconds, ground_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "ground".to_string(),
            "--budget".to_string(),
            "strict".to_string(),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let (search_seconds, search_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "search".to_string(),
            "--query".to_string(),
            "AppController".to_string(),
            "--repo-text".to_string(),
            "off".to_string(),
            "--limit".to_string(),
            "10".to_string(),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let top_symbol_id = string_field(&search_json, &["indexed_symbol_hits", "0", "node_id"]);
    let top_symbol_name = string_field(&search_json, &["indexed_symbol_hits", "0", "display_name"]);

    let (symbol_seconds, symbol_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "symbol".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let (trail_seconds, trail_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "trail".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let (snippet_seconds, snippet_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "snippet".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let search_dir_after = fs::metadata(&search_dir)
        .expect("search dir metadata after reads")
        .modified()
        .expect("search dir modified time after reads");
    let search_dir_unchanged = search_dir_before == search_dir_after;

    let graph_phase_ms = optional_u64_field(&index_json, &["phase_timings", "parse_index_ms"])
        + optional_u64_field(&index_json, &["phase_timings", "projection_flush_ms"])
        + optional_u64_field(&index_json, &["phase_timings", "edge_resolution_ms"])
        + optional_u64_field(&index_json, &["phase_timings", "error_flush_ms"])
        + optional_u64_field(&index_json, &["phase_timings", "cleanup_ms"]);
    let semantic_phase_ms =
        optional_u64_field(&index_json, &["phase_timings", "semantic_doc_build_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_embedding_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_db_upsert_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_reload_ms"]);

    let stats = RepoE2eStats {
        project_root: project_root.display().to_string(),
        cache_dir: cache_dir.path().display().to_string(),
        storage_path: storage_path.display().to_string(),
        search_dir: search_dir.display().to_string(),
        embed_batch_size: 128,
        search_dir_unchanged,
        index_seconds,
        graph_phase_seconds: graph_phase_ms as f64 / 1000.0,
        semantic_phase_seconds: semantic_phase_ms as f64 / 1000.0,
        semantic_docs_reused: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_docs_reused"],
        ),
        semantic_docs_embedded: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_docs_embedded"],
        ),
        semantic_docs_pending: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_docs_pending"],
        ),
        semantic_docs_stale: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_docs_stale"],
        ),
        ground_seconds,
        search_seconds,
        symbol_seconds,
        trail_seconds,
        snippet_seconds,
        index: IndexStats {
            node_count: u64_field(&index_json, &["summary", "stats", "node_count"]),
            edge_count: u64_field(&index_json, &["summary", "stats", "edge_count"]),
            file_count: u64_field(&index_json, &["summary", "stats", "file_count"]),
            error_count: u64_field(&index_json, &["summary", "stats", "error_count"]),
            retrieval_mode: string_field(&index_json, &["retrieval", "mode"]).to_string(),
            semantic_doc_count: u64_field(&index_json, &["retrieval", "semantic_doc_count"]),
        },
        ground: GroundStats {
            retrieval_mode: string_field(&ground_json, &["retrieval", "mode"]).to_string(),
            root_symbols: array_len(&ground_json, &["root_symbols"]),
            file_digests: array_len(&ground_json, &["files"]),
            coverage_total_files: u64_field(&ground_json, &["coverage", "total_files"]),
        },
        search: SearchStats {
            query: string_field(&search_json, &["query"]).to_string(),
            retrieval_mode: string_field(&search_json, &["retrieval", "mode"]).to_string(),
            semantic_doc_count: u64_field(&search_json, &["retrieval", "semantic_doc_count"]),
            indexed_symbol_hits: array_len(&search_json, &["indexed_symbol_hits"]),
            repo_text_hits: array_len(&search_json, &["repo_text_hits"]),
            top_symbol_id: top_symbol_id.to_string(),
            top_symbol_name: top_symbol_name.to_string(),
        },
        symbol: SymbolStats {
            display_name: string_field(&symbol_json, &["symbol", "node", "display_name"])
                .to_string(),
            related_hits: array_len(&symbol_json, &["symbol", "related_hits"]),
            edge_digest_entries: array_len(&symbol_json, &["symbol", "edge_digest"]),
        },
        trail: TrailStats {
            focus_display_name: string_field(&trail_json, &["trail", "focus", "display_name"])
                .to_string(),
            node_count: array_len(&trail_json, &["trail", "trail", "nodes"]),
            edge_count: array_len(&trail_json, &["trail", "trail", "edges"]),
            truncated: bool_field(&trail_json, &["trail", "trail", "truncated"]),
        },
        snippet: SnippetStats {
            path: string_field(&snippet_json, &["snippet", "path"]).to_string(),
            line: u64_field(&snippet_json, &["snippet", "line"]),
            snippet_lines: string_field(&snippet_json, &["snippet", "snippet"])
                .lines()
                .count(),
        },
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&stats).expect("serialize stats")
    );

    assert_eq!(
        stats.index.error_count, 0,
        "full repo index should finish without errors"
    );
    assert!(
        stats.index.semantic_doc_count > 0,
        "full repo index should populate semantic docs"
    );
    assert!(
        stats.semantic_docs_embedded > 0,
        "full repo index should report embedded semantic docs"
    );
    assert_eq!(
        stats.search.top_symbol_name, "AppController",
        "exact symbol query should prefer the exact AppController symbol"
    );
    assert_eq!(
        stats.symbol.display_name, "AppController",
        "symbol lookup should resolve the same top hit returned by search"
    );
    assert!(
        stats.search.indexed_symbol_hits > 0,
        "search should return indexed hits"
    );
    assert!(
        stats.ground.file_digests > 0,
        "ground should emit file digests"
    );
    assert!(stats.trail.node_count > 0, "trail should emit graph nodes");
    assert!(
        stats.snippet.snippet_lines > 0,
        "snippet should include source lines"
    );
    assert!(
        stats.search_dir_unchanged,
        "plain read commands should not recreate the persisted search dir"
    );
    assert!(
        stats.search_seconds < 10.0,
        "cold search should stay under 10 seconds on the codestory repo, got {:.2}s",
        stats.search_seconds
    );
    assert!(
        stats.symbol_seconds < 10.0,
        "cold symbol lookup should stay under 10 seconds on the codestory repo, got {:.2}s",
        stats.symbol_seconds
    );
}
