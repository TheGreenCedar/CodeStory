use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
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
    proof_tier: String,
    warnings: Vec<String>,
    embed_batch_size: u32,
    search_dir_unchanged: bool,
    index_seconds: f64,
    graph_phase_seconds: f64,
    semantic_phase_seconds: f64,
    semantic_embedding_ms: u64,
    symbol_search_docs_written: u64,
    semantic_dense_docs_skipped: u64,
    semantic_dense_public_api: u64,
    semantic_dense_entrypoint: u64,
    semantic_dense_documented_nontrivial: u64,
    semantic_dense_central_graph_node: u64,
    semantic_dense_component_report: u64,
    semantic_dense_unstructured_doc: u64,
    semantic_docs_reused: u64,
    semantic_docs_embedded: u64,
    semantic_docs_pending: u64,
    semantic_docs_stale: u64,
    repeat_full_refresh_seconds: f64,
    repeat_graph_phase_seconds: f64,
    repeat_semantic_phase_seconds: f64,
    repeat_semantic_doc_build_ms: u64,
    repeat_semantic_embedding_ms: u64,
    repeat_semantic_db_upsert_ms: u64,
    repeat_semantic_reload_ms: u64,
    repeat_semantic_prune_ms: u64,
    repeat_semantic_docs_reused: u64,
    repeat_semantic_docs_embedded: u64,
    repeat_semantic_docs_pending: u64,
    repeat_semantic_docs_stale: u64,
    retrieval_index_seconds: f64,
    retrieval_status_seconds: f64,
    sidecar_manifest: SidecarManifestStats,
    ground_seconds: f64,
    search_seconds: f64,
    symbol_seconds: f64,
    trail_seconds: f64,
    snippet_seconds: f64,
    report_seconds: f64,
    index: IndexStats,
    ground: GroundStats,
    search: SearchStats,
    symbol: SymbolStats,
    trail: TrailStats,
    snippet: SnippetStats,
    report: ReportStats,
}

#[derive(Debug, Serialize)]
struct SidecarManifestStats {
    symbol_doc_count: u64,
    dense_projection_count: u64,
    projection_count: u64,
    semantic_policy_version: String,
    graph_artifact_hash_present: bool,
    dense_reason_counts_json: String,
    dense_reason_count_total: u64,
}

#[derive(Debug, Serialize)]
struct IndexStats {
    node_count: u64,
    edge_count: u64,
    file_count: u64,
    error_count: u64,
    sidecar_status_after_retrieval_index: String,
    legacy_index_retrieval_mode: String,
    semantic_doc_count: u64,
}

#[derive(Debug, Serialize)]
struct GroundStats {
    sidecar_status_after_retrieval_index: String,
    legacy_ground_retrieval_mode: String,
    root_symbols: usize,
    file_digests: usize,
    coverage_total_files: u64,
}

#[derive(Debug, Serialize)]
struct SearchStats {
    query: String,
    sidecar_shadow_retrieval_mode: String,
    legacy_search_retrieval_mode: String,
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

#[derive(Debug, Serialize)]
struct ReportStats {
    markdown_seconds: f64,
    json_seconds: f64,
    markdown_bytes: u64,
    json_graph_nodes: usize,
    json_graph_edges: usize,
}

const PROOF_TIER_STATS_ONLY: &str = "stats_only";
const PROOF_TIER_FULL_SIDECAR: &str = "full_sidecar";
const INDEX_SECONDS_WARNING_THRESHOLD: f64 = 600.0;
const SEMANTIC_PHASE_SECONDS_WARNING_THRESHOLD: f64 = 500.0;

fn release_readiness_proof_tier(
    sidecar_status_after_retrieval_index: &str,
    search_shadow_sidecar_mode: &str,
) -> &'static str {
    if sidecar_status_after_retrieval_index == "full" && search_shadow_sidecar_mode == "full" {
        PROOF_TIER_FULL_SIDECAR
    } else {
        PROOF_TIER_STATS_ONLY
    }
}

fn release_readiness_warnings(index_seconds: f64, semantic_phase_seconds: f64) -> Vec<String> {
    let mut warnings = Vec::new();
    if index_seconds > INDEX_SECONDS_WARNING_THRESHOLD {
        warnings.push(format!(
            "index_seconds exceeded {INDEX_SECONDS_WARNING_THRESHOLD:.0}s release-readiness warning threshold: {index_seconds:.2}s"
        ));
    }
    if semantic_phase_seconds > SEMANTIC_PHASE_SECONDS_WARNING_THRESHOLD {
        warnings.push(format!(
            "semantic_phase_seconds exceeded {SEMANTIC_PHASE_SECONDS_WARNING_THRESHOLD:.0}s release-readiness warning threshold: {semantic_phase_seconds:.2}s"
        ));
    }
    warnings
}

#[derive(Debug)]
struct DrillRepoCase {
    name: String,
    project_root: PathBuf,
    question: String,
    anchors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DrillRepoCaseManifest {
    cases: Vec<DrillRepoCaseConfig>,
}

#[derive(Debug, Deserialize)]
struct DrillRepoCaseConfig {
    slug: String,
    project: PathBuf,
    question: String,
    anchors: Vec<String>,
}

#[test]
fn release_readiness_proof_tier_requires_full_sidecar_evidence() {
    assert_eq!(
        release_readiness_proof_tier("full", "full"),
        PROOF_TIER_FULL_SIDECAR
    );
    assert_eq!(
        release_readiness_proof_tier("full", "degraded"),
        PROOF_TIER_STATS_ONLY
    );
    assert_eq!(
        release_readiness_proof_tier("unavailable", "full"),
        PROOF_TIER_STATS_ONLY
    );
}

#[test]
fn release_readiness_proof_tier_does_not_claim_drill_or_promotion() {
    let proof_tier = release_readiness_proof_tier("full", "full");

    assert_eq!(proof_tier, PROOF_TIER_FULL_SIDECAR);
    assert_ne!(proof_tier, "real_repo_drill");
    assert_ne!(proof_tier, "promotion_grade");
}

#[test]
fn release_readiness_warnings_only_emit_above_thresholds() {
    assert!(release_readiness_warnings(600.0, 500.0).is_empty());

    assert_eq!(
        release_readiness_warnings(600.01, 500.01),
        vec![
            "index_seconds exceeded 600s release-readiness warning threshold: 600.01s".to_string(),
            "semantic_phase_seconds exceeded 500s release-readiness warning threshold: 500.01s"
                .to_string(),
        ]
    );
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
    let (seconds, stdout) = run_cli_output(binary, project_root, cache_dir, args);
    (
        seconds,
        serde_json::from_slice(&stdout).expect("parse json output"),
    )
}

fn run_cli_output(
    binary: &Path,
    project_root: &Path,
    cache_dir: &Path,
    args: &[String],
) -> (f64, Vec<u8>) {
    let started = Instant::now();
    let output = Command::new(binary)
        .current_dir(project_root)
        .args(args)
        .arg("--project")
        .arg(project_root)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env_remove("CODESTORY_EMBED_RUNTIME_MODE")
        .env("CODESTORY_EMBED_BACKEND", "llamacpp")
        .env("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1")
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
    (seconds, output.stdout)
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

fn dense_reason_count_total(reason_counts_json: &str) -> u64 {
    let value: Value =
        serde_json::from_str(reason_counts_json).expect("dense reason counts should be json");
    value
        .as_object()
        .expect("dense reason counts should be a json object")
        .values()
        .map(|value| value.as_u64().expect("dense reason count should be u64"))
        .sum()
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

fn array_contains_command(value: &Value, path: &[&str], expected: &str) -> bool {
    json_path(value, path)
        .as_array()
        .unwrap_or_else(|| panic!("expected array at path {:?}", path))
        .iter()
        .any(|item| item["command"].as_str() == Some(expected))
}

fn array_item_by_field<'a>(
    value: &'a Value,
    path: &[&str],
    field: &str,
    expected: &str,
) -> &'a Value {
    json_path(value, path)
        .as_array()
        .unwrap_or_else(|| panic!("expected array at path {:?}", path))
        .iter()
        .find(|item| item[field].as_str() == Some(expected))
        .unwrap_or_else(|| panic!("missing {field}={expected:?} at path {path:?}"))
}

fn drill_repo_cases_from_manifest(manifest_path: &Path) -> Vec<DrillRepoCase> {
    let manifest_text = fs::read_to_string(manifest_path).unwrap_or_else(|error| {
        panic!(
            "failed to read CODESTORY_REAL_REPO_DRILL_CASES manifest {}: {error}",
            manifest_path.display()
        )
    });
    let manifest: DrillRepoCaseManifest =
        serde_json::from_str(&manifest_text).unwrap_or_else(|error| {
            panic!(
                "failed to parse CODESTORY_REAL_REPO_DRILL_CASES manifest {}: {error}",
                manifest_path.display()
            )
        });
    let manifest_dir = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    manifest
        .cases
        .into_iter()
        .map(|case| {
            let project_root = if case.project.is_absolute() {
                case.project
            } else {
                manifest_dir.join(case.project)
            };
            DrillRepoCase {
                name: case.slug,
                project_root,
                question: case.question,
                anchors: case.anchors,
            }
        })
        .collect()
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

    let (repeat_full_refresh_seconds, repeat_index_json) = run_cli_json(
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

    let (retrieval_index_seconds, _retrieval_index_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "retrieval".to_string(),
            "index".to_string(),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let (retrieval_status_seconds, retrieval_status_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "retrieval".to_string(),
            "status".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let sidecar_retrieval_mode =
        string_field(&retrieval_status_json, &["retrieval_mode"]).to_string();

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

    let (report_markdown_seconds, report_markdown_stdout) = run_cli_output(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "report".to_string(),
            "--limit".to_string(),
            "8".to_string(),
            "--format".to_string(),
            "markdown".to_string(),
        ],
    );
    let (report_json_seconds, report_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "report".to_string(),
            "--limit".to_string(),
            "8".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let report_seconds = report_markdown_seconds + report_json_seconds;

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
    let repeat_graph_phase_ms =
        optional_u64_field(&repeat_index_json, &["phase_timings", "parse_index_ms"])
            + optional_u64_field(
                &repeat_index_json,
                &["phase_timings", "projection_flush_ms"],
            )
            + optional_u64_field(&repeat_index_json, &["phase_timings", "edge_resolution_ms"])
            + optional_u64_field(&repeat_index_json, &["phase_timings", "error_flush_ms"])
            + optional_u64_field(&repeat_index_json, &["phase_timings", "cleanup_ms"]);
    let semantic_phase_ms =
        optional_u64_field(&index_json, &["phase_timings", "semantic_doc_build_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_embedding_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_db_upsert_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_reload_ms"])
            + optional_u64_field(&index_json, &["phase_timings", "semantic_prune_ms"]);
    let repeat_semantic_doc_build_ms = optional_u64_field(
        &repeat_index_json,
        &["phase_timings", "semantic_doc_build_ms"],
    );
    let repeat_semantic_embedding_ms = optional_u64_field(
        &repeat_index_json,
        &["phase_timings", "semantic_embedding_ms"],
    );
    let repeat_semantic_db_upsert_ms = optional_u64_field(
        &repeat_index_json,
        &["phase_timings", "semantic_db_upsert_ms"],
    );
    let repeat_semantic_reload_ms =
        optional_u64_field(&repeat_index_json, &["phase_timings", "semantic_reload_ms"]);
    let repeat_semantic_prune_ms =
        optional_u64_field(&repeat_index_json, &["phase_timings", "semantic_prune_ms"]);
    let repeat_semantic_phase_ms = repeat_semantic_doc_build_ms
        + repeat_semantic_embedding_ms
        + repeat_semantic_db_upsert_ms
        + repeat_semantic_reload_ms
        + repeat_semantic_prune_ms;
    let semantic_phase_seconds = semantic_phase_ms as f64 / 1000.0;
    let search_sidecar_shadow_retrieval_mode =
        string_field(&search_json, &["retrieval_shadow", "retrieval_mode"]).to_string();
    let dense_reason_counts_json = string_field(
        &retrieval_status_json,
        &["manifest", "dense_reason_counts_json"],
    )
    .to_string();
    let sidecar_manifest = SidecarManifestStats {
        symbol_doc_count: u64_field(&retrieval_status_json, &["manifest", "symbol_doc_count"]),
        dense_projection_count: u64_field(
            &retrieval_status_json,
            &["manifest", "dense_projection_count"],
        ),
        projection_count: u64_field(&retrieval_status_json, &["manifest", "projection_count"]),
        semantic_policy_version: string_field(
            &retrieval_status_json,
            &["manifest", "semantic_policy_version"],
        )
        .to_string(),
        graph_artifact_hash_present: !string_field(
            &retrieval_status_json,
            &["manifest", "graph_artifact_hash"],
        )
        .trim()
        .is_empty(),
        dense_reason_count_total: dense_reason_count_total(&dense_reason_counts_json),
        dense_reason_counts_json,
    };
    let proof_tier = release_readiness_proof_tier(
        sidecar_retrieval_mode.as_str(),
        search_sidecar_shadow_retrieval_mode.as_str(),
    )
    .to_string();
    let warnings = release_readiness_warnings(index_seconds, semantic_phase_seconds);

    let stats = RepoE2eStats {
        project_root: project_root.display().to_string(),
        cache_dir: cache_dir.path().display().to_string(),
        storage_path: storage_path.display().to_string(),
        search_dir: search_dir.display().to_string(),
        proof_tier,
        warnings,
        embed_batch_size: 128,
        search_dir_unchanged,
        index_seconds,
        graph_phase_seconds: graph_phase_ms as f64 / 1000.0,
        semantic_phase_seconds,
        semantic_embedding_ms: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_embedding_ms"],
        ),
        symbol_search_docs_written: optional_u64_field(
            &index_json,
            &["phase_timings", "symbol_search_docs_written"],
        ),
        semantic_dense_docs_skipped: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_docs_skipped"],
        ),
        semantic_dense_public_api: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_public_api"],
        ),
        semantic_dense_entrypoint: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_entrypoint"],
        ),
        semantic_dense_documented_nontrivial: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_documented_nontrivial"],
        ),
        semantic_dense_central_graph_node: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_central_graph_node"],
        ),
        semantic_dense_component_report: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_component_report"],
        ),
        semantic_dense_unstructured_doc: optional_u64_field(
            &index_json,
            &["phase_timings", "semantic_dense_unstructured_doc"],
        ),
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
        repeat_full_refresh_seconds,
        repeat_graph_phase_seconds: repeat_graph_phase_ms as f64 / 1000.0,
        repeat_semantic_phase_seconds: repeat_semantic_phase_ms as f64 / 1000.0,
        repeat_semantic_doc_build_ms,
        repeat_semantic_embedding_ms,
        repeat_semantic_db_upsert_ms,
        repeat_semantic_reload_ms,
        repeat_semantic_prune_ms,
        repeat_semantic_docs_reused: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "semantic_docs_reused"],
        ),
        repeat_semantic_docs_embedded: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "semantic_docs_embedded"],
        ),
        repeat_semantic_docs_pending: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "semantic_docs_pending"],
        ),
        repeat_semantic_docs_stale: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "semantic_docs_stale"],
        ),
        retrieval_index_seconds,
        retrieval_status_seconds,
        sidecar_manifest,
        ground_seconds,
        search_seconds,
        symbol_seconds,
        trail_seconds,
        snippet_seconds,
        report_seconds,
        index: IndexStats {
            node_count: u64_field(&index_json, &["summary", "stats", "node_count"]),
            edge_count: u64_field(&index_json, &["summary", "stats", "edge_count"]),
            file_count: u64_field(&index_json, &["summary", "stats", "file_count"]),
            error_count: u64_field(&index_json, &["summary", "stats", "error_count"]),
            sidecar_status_after_retrieval_index: sidecar_retrieval_mode.clone(),
            legacy_index_retrieval_mode: string_field(&index_json, &["retrieval", "mode"])
                .to_string(),
            semantic_doc_count: u64_field(&index_json, &["retrieval", "semantic_doc_count"]),
        },
        ground: GroundStats {
            sidecar_status_after_retrieval_index: sidecar_retrieval_mode.clone(),
            legacy_ground_retrieval_mode: string_field(&ground_json, &["retrieval", "mode"])
                .to_string(),
            root_symbols: array_len(&ground_json, &["root_symbols"]),
            file_digests: array_len(&ground_json, &["files"]),
            coverage_total_files: u64_field(&ground_json, &["coverage", "total_files"]),
        },
        search: SearchStats {
            query: string_field(&search_json, &["query"]).to_string(),
            sidecar_shadow_retrieval_mode: search_sidecar_shadow_retrieval_mode,
            legacy_search_retrieval_mode: string_field(&search_json, &["retrieval", "mode"])
                .to_string(),
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
        report: ReportStats {
            markdown_seconds: report_markdown_seconds,
            json_seconds: report_json_seconds,
            markdown_bytes: report_markdown_stdout.len() as u64,
            json_graph_nodes: array_len(&report_json, &["graph", "nodes"]),
            json_graph_edges: array_len(&report_json, &["graph", "edges"]),
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
    assert_eq!(
        stats.index.sidecar_status_after_retrieval_index, "full",
        "retrieval status after retrieval index should be full before trusting index/ground/search evidence"
    );
    assert_eq!(
        stats.proof_tier, PROOF_TIER_FULL_SIDECAR,
        "repo e2e stats harness proves full sidecar evidence but does not run real-repo drill cases"
    );
    assert_eq!(
        stats.ground.sidecar_status_after_retrieval_index, "full",
        "strict grounding should reuse the prepared full sidecar retrieval state"
    );
    assert_eq!(
        stats.search.sidecar_shadow_retrieval_mode, "full",
        "search should expose full sidecar retrieval shadow"
    );
    assert!(
        stats.sidecar_manifest.symbol_doc_count > 0,
        "full sidecar manifest should record graph-native symbol docs"
    );
    assert!(
        stats.sidecar_manifest.dense_projection_count > 0,
        "CodeStory product run should select dense anchors"
    );
    assert_eq!(
        stats.sidecar_manifest.dense_projection_count, stats.sidecar_manifest.projection_count,
        "legacy projection_count should mirror dense_projection_count under graph_first_v1"
    );
    assert_eq!(
        stats.sidecar_manifest.semantic_policy_version, "graph_first_v1",
        "full sidecar manifest should record the active dense policy"
    );
    assert!(
        stats.sidecar_manifest.graph_artifact_hash_present,
        "full sidecar manifest should record a graph artifact hash"
    );
    assert_eq!(
        stats.sidecar_manifest.dense_reason_count_total,
        stats.sidecar_manifest.dense_projection_count,
        "dense reason counts should account for every dense anchor"
    );
    assert!(
        stats.symbol_search_docs_written > 0,
        "index should report graph-native symbol docs written"
    );
    assert!(
        stats.semantic_dense_docs_skipped > 0,
        "AST-first policy should skip dense embeddings for recoverable code symbols"
    );
    assert_eq!(
        stats.repeat_semantic_docs_embedded, 0,
        "repeat full refresh should embed zero unchanged dense docs"
    );
    assert!(
        stats.repeat_full_refresh_seconds < 25.0,
        "repeat full refresh should stay under 25 seconds, got {:.2}s",
        stats.repeat_full_refresh_seconds
    );
    assert!(
        stats.index.semantic_doc_count > 0,
        "full repo index should populate dense anchors"
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

#[test]
#[ignore = "real-repo drill release gate; set CODESTORY_REAL_REPO_DRILL_CASES or CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1 and run after cargo build --release -p codestory-cli"]
fn real_repo_agent_grounding_drill_emits_verification_packets() {
    let binary = release_cli_binary();
    assert!(
        binary.is_file(),
        "missing release binary at {}. Run `cargo build --release -p codestory-cli` first.",
        binary.display()
    );

    let root_output = tempdir().expect("drill output dir");
    let cache_dir = tempdir().expect("drill cache dir");
    let Some(manifest_path) = env::var_os("CODESTORY_REAL_REPO_DRILL_CASES").map(PathBuf::from)
    else {
        if allow_skip_real_repo_drill_cases() {
            eprintln!(
                "intentionally skipping manifest real-repo drill suite because CODESTORY_REAL_REPO_DRILL_CASES is not set and CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1"
            );
            return;
        }
        panic!(
            "real-repo drill suite cannot run because CODESTORY_REAL_REPO_DRILL_CASES is not set. Set CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1 only for an intentional local skip before invoking the ignored test."
        );
    };
    let cases = drill_repo_cases_from_manifest(&manifest_path);
    if cases.is_empty() {
        panic!(
            "real-repo drill suite manifest contains no cases: {}",
            manifest_path.display()
        );
    }
    let missing = cases
        .iter()
        .filter(|case| !case.project_root.is_dir())
        .map(|case| format!("{} ({})", case.name, case.project_root.display()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        if allow_skip_real_repo_drill_cases() {
            eprintln!(
                "intentionally skipping manifest real-repo drill suite because configured repos are missing and CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1: {}",
                missing.join(", ")
            );
            return;
        }
        panic!(
            "real-repo drill suite cannot run because configured repos are missing: {}. Set CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1 only for an intentional local skip before invoking the ignored test.",
            missing.join(", ")
        );
    }

    let (_seconds, suite_json) = run_cli_json(
        &binary,
        repo_root().as_path(),
        cache_dir.path(),
        &[
            "drill-suite".to_string(),
            "--refresh".to_string(),
            "full".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--output-dir".to_string(),
            root_output.path().display().to_string(),
            "--case-file".to_string(),
            manifest_path.display().to_string(),
        ],
    );

    let reported_case_file = PathBuf::from(string_field(&suite_json, &["case_file"]));
    assert_eq!(
        reported_case_file, manifest_path,
        "suite should report the configured case file path"
    );
    assert_eq!(u64_field(&suite_json, &["repo_count"]), cases.len() as u64);
    assert_eq!(
        array_len(&suite_json, &["repos"]),
        cases.len(),
        "suite should include exactly the manifest real-repo drill cases"
    );
    assert!(
        root_output.path().join("suite-report.md").is_file(),
        "drill-suite should write a markdown aggregate report"
    );
    assert!(
        root_output.path().join("suite-report.json").is_file(),
        "drill-suite should write a JSON aggregate report"
    );
    let suite_markdown = fs::read_to_string(root_output.path().join("suite-report.md"))
        .expect("read suite markdown");
    assert!(
        suite_markdown.contains("targets / 0 verified /") && suite_markdown.contains("pending"),
        "suite markdown should make pending source-truth verification visible instead of implying CodeStory-only proof"
    );
    for case in &cases {
        assert!(
            cache_dir.path().join(case.name.as_str()).is_dir(),
            "drill-suite should isolate the explicit cache root for {}",
            case.name
        );
    }

    for (case_index, case) in cases.iter().enumerate() {
        let repo_index = case_index.to_string();
        let repo_json = json_path(&suite_json, &["repos", repo_index.as_str()]);
        assert_eq!(
            string_field(repo_json, &["slug"]),
            case.name,
            "suite should preserve manifest repo order"
        );
        assert_eq!(
            string_field(repo_json, &["question"]),
            case.question,
            "{} suite entry should preserve the exact natural-language question",
            case.name
        );
        assert_eq!(array_len(repo_json, &["anchors"]), case.anchors.len());
        assert_eq!(
            u64_field(repo_json, &["summary", "anchors", "resolved"]),
            case.anchors.len() as u64,
            "{} suite summary should resolve every seed anchor",
            case.name
        );
        assert_eq!(
            u64_field(repo_json, &["summary", "anchors", "unresolved"]),
            0,
            "{} suite summary should not leave seed anchors unresolved",
            case.name
        );
        assert_ne!(
            string_field(repo_json, &["summary", "verdict", "status"]),
            "blocked",
            "{} suite summary should remain usable even when degraded",
            case.name
        );
        assert!(
            bool_field(repo_json, &["summary", "source_truth", "required"]),
            "{} suite summary should require source-truth verification",
            case.name
        );
        assert!(
            u64_field(repo_json, &["summary", "source_truth", "check_count"])
                >= case.anchors.len() as u64,
            "{} suite summary should name at least one source-truth check per anchor",
            case.name
        );
        let check_count = u64_field(repo_json, &["summary", "source_truth", "check_count"]);
        assert_eq!(
            u64_field(
                repo_json,
                &["summary", "source_truth", "pending_check_count"]
            ),
            check_count,
            "{} suite summary should keep all generated source-truth checks pending until source reads happen",
            case.name
        );
        assert_eq!(
            u64_field(
                repo_json,
                &["summary", "source_truth", "verified_check_count"]
            ),
            0,
            "{} suite summary should not count generated checks as verified before source reads",
            case.name
        );
        assert_eq!(
            u64_field(
                repo_json,
                &["summary", "open_gaps", "pending_source_truth_check_count"]
            ),
            check_count,
            "{} suite open-gaps summary should preserve pending source-truth checks",
            case.name
        );
        assert_eq!(
            string_field(
                repo_json,
                &["summary", "open_gaps", "answer_quality_status"]
            ),
            "pending_source_verification",
            "{} suite open-gaps status should not imply CodeStory-only answer quality is final",
            case.name
        );
        assert!(
            u64_field(
                repo_json,
                &["summary", "source_truth", "pending_claim_count"]
            ) > 0,
            "{} suite summary should keep claim-ledger entries pending",
            case.name
        );
        assert!(
            u64_field(repo_json, &["summary", "open_gaps", "pending_claim_count"]) > 0,
            "{} suite open-gaps summary should count pending claims",
            case.name
        );

        let output_dir = PathBuf::from(string_field(repo_json, &["output_dir"]));
        let drill_report_path = output_dir.join("drill-report.json");
        assert!(
            drill_report_path.is_file(),
            "{} suite should write the per-repo full drill report at {}",
            case.name,
            drill_report_path.display()
        );
        let drill_json: Value =
            serde_json::from_slice(&fs::read(&drill_report_path).expect("read drill report"))
                .expect("parse drill report json");

        assert_eq!(
            string_field(&drill_json, &["question"]),
            case.question,
            "{} drill report should preserve the natural-language question",
            case.name
        );
        assert_eq!(
            string_field(&drill_json, &["question_search", "status"]),
            "ok",
            "{} drill should collect natural-language repo-text evidence",
            case.name
        );
        assert_question_search_names_seed_anchors(case, &drill_json);
        assert!(
            array_len(&drill_json, &["verification_checklist"]) >= 4,
            "{} drill should force source-truth verification structure",
            case.name
        );
        assert!(
            bool_field(
                &drill_json,
                &["answer_quality_contract", "code_story_only_draft_required"]
            ),
            "{} drill should require a CodeStory-only draft before source reads",
            case.name
        );
        assert!(
            bool_field(
                &drill_json,
                &[
                    "answer_quality_contract",
                    "source_truth_verification_required",
                ]
            ),
            "{} drill should require source-truth verification after the draft",
            case.name
        );
        assert!(
            array_len(&drill_json, &["answer_quality_contract", "score_inputs"]) >= 5,
            "{} drill should expose score inputs for answer-quality reporting",
            case.name
        );
        assert!(
            array_len(&drill_json, &["claim_ledger_template", "claims"]) >= case.anchors.len(),
            "{} drill should emit a fillable claim ledger template",
            case.name
        );
        assert_eq!(
            array_len(&drill_json, &["bridges"]),
            case.anchors.len().saturating_sub(1) * case.anchors.len() / 2,
            "{} drill should emit pairwise cross-anchor bridge evidence",
            case.name
        );
        assert_compact_bridge_status_handoff(&case.name, repo_json);
        assert!(array_len(&drill_json, &["execution_boundaries"]) >= 3);

        for anchor_index in 0..case.anchors.len() {
            let index = anchor_index.to_string();
            assert_eq!(
                string_field(&drill_json, &["anchors", index.as_str(), "anchor"]),
                case.anchors[anchor_index].as_str(),
                "{} drill should keep anchor order",
                case.name
            );
            assert!(
                array_len(&drill_json, &["anchors", index.as_str(), "commands"]) >= 1,
                "{} drill anchor should record evidence command artifacts",
                case.name
            );
            assert!(
                u64_field(&drill_json, &["anchors", index.as_str(), "typed_hit_count"]) > 0,
                "{} anchor {} should retain typed search hits",
                case.name,
                case.anchors[anchor_index]
            );
            if !json_path(&drill_json, &["anchors", index.as_str(), "chosen_anchor"]).is_null() {
                assert!(
                    array_contains_command(
                        &drill_json,
                        &["anchors", index.as_str(), "commands"],
                        "symbol"
                    ),
                    "{} drill should include symbol evidence for resolved anchors",
                    case.name
                );
                assert!(
                    array_contains_command(
                        &drill_json,
                        &["anchors", index.as_str(), "commands"],
                        "trail"
                    ),
                    "{} drill should include trail evidence for resolved anchors",
                    case.name
                );
                assert!(
                    array_contains_command(
                        &drill_json,
                        &["anchors", index.as_str(), "commands"],
                        "snippet"
                    ),
                    "{} drill should include snippet evidence for resolved anchors",
                    case.name
                );
                assert!(
                    array_contains_command(
                        &drill_json,
                        &["anchors", index.as_str(), "commands"],
                        "explore"
                    ),
                    "{} drill should include an explore source-packet artifact for resolved anchors",
                    case.name
                );
            }
        }

        assert_manifest_anchor_expectations(case, repo_json);
    }
}

fn allow_skip_real_repo_drill_cases() -> bool {
    env::var("CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn assert_question_search_names_seed_anchors(case: &DrillRepoCase, drill_json: &Value) {
    let artifact = PathBuf::from(string_field(drill_json, &["question_search", "artifact"]));
    assert!(
        artifact.is_file(),
        "{} drill should write question-search artifact at {}",
        case.name,
        artifact.display()
    );
    let question_search: Value =
        serde_json::from_slice(&fs::read(&artifact).expect("read question-search artifact"))
            .expect("parse question-search artifact");
    let subqueries = json_path(&question_search, &["search_plan", "subqueries"])
        .as_array()
        .expect("question search plan subqueries");
    for anchor in &case.anchors {
        assert!(
            subqueries.iter().any(|subquery| {
                subquery["role"].as_str() == Some("named_anchor")
                    && subquery["query"].as_str() == Some(anchor.as_str())
            }),
            "{} broad question Search Plan should preserve named-anchor subquery for {anchor}: {question_search:#}",
            case.name
        );
    }
}

fn assert_compact_bridge_status_handoff(repo_name: &str, repo_json: &Value) {
    let total = u64_field(repo_json, &["summary", "bridges", "total"]);
    let statuses = json_path(repo_json, &["summary", "bridges", "statuses"])
        .as_array()
        .expect("compact bridge statuses");
    assert_eq!(
        statuses.len() as u64,
        total,
        "{repo_name} compact bridge statuses should cover every bridge pair"
    );
    let blocked = string_field(repo_json, &["summary", "verdict", "status"]) == "blocked";
    for status in statuses {
        for field in ["from_anchor", "to_anchor", "status", "strategy"] {
            assert!(
                status[field]
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty()),
                "{repo_name} compact bridge status should preserve {field}: {status:#}"
            );
        }
        if !blocked {
            assert_eq!(
                status["command_status"].as_str(),
                Some("ok"),
                "{repo_name} compact bridge status should preserve command health for usable suite entries: {status:#}"
            );
        }
    }
}

fn assert_manifest_anchor_expectations(case: &DrillRepoCase, repo_json: &Value) {
    for anchor in &case.anchors {
        assert_anchor_summary_usable(repo_json, anchor);
    }
}

fn assert_anchor_summary_usable(repo_json: &Value, anchor: &str) {
    let summary = array_item_by_field(
        repo_json,
        &["summary", "anchors", "statuses"],
        "anchor",
        anchor,
    );
    assert!(
        u64_field(summary, &["source_truth_target_count"]) > 0,
        "{anchor} should retain source-truth target pointers"
    );
}
