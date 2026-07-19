use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::tempdir;

#[derive(Debug, Serialize)]
struct RepoE2eStats {
    commit: String,
    evidence_identity: EvidenceIdentity,
    project_root: String,
    cache_dir: String,
    storage_path: String,
    search_dir: String,
    storage_bytes: u64,
    proof_tier: String,
    warnings: Vec<String>,
    stats_baseline: StatsLogBaseline,
    embed_batch_size: u32,
    search_dir_unchanged: bool,
    index_seconds: f64,
    phase_timings: Value,
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
    repeat_phase_timings: Value,
    repeat_graph_phase_seconds: f64,
    repeat_semantic_phase_seconds: f64,
    repeat_semantic_doc_build_ms: u64,
    repeat_semantic_embedding_ms: u64,
    repeat_semantic_db_upsert_ms: u64,
    repeat_semantic_reload_ms: u64,
    repeat_semantic_prune_ms: u64,
    repeat_cache_refresh_ms: u64,
    repeat_search_projection_rebuild_ms: u64,
    repeat_search_symbol_index_ms: u64,
    repeat_runtime_cache_publish_ms: u64,
    repeat_semantic_docs_reused: u64,
    repeat_semantic_docs_embedded: u64,
    repeat_semantic_docs_pending: u64,
    repeat_semantic_docs_stale: u64,
    retrieval_index_seconds: f64,
    retrieval_status_seconds: f64,
    retrieval_manifest: RetrievalManifestStats,
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
struct EvidenceIdentity {
    corpus_id: String,
    cache_id: String,
    machine_fingerprint: String,
}

#[derive(Debug, Serialize)]
struct RetrievalManifestStats {
    symbol_doc_count: u64,
    dense_projection_count: u64,
    projection_count: u64,
    semantic_policy_version: String,
    graph_artifact_hash_present: bool,
    dense_reason_counts_json: String,
    dense_reason_count_total: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct StatsLogBaseline {
    source_path: String,
    date: String,
    commit: String,
    scenario: String,
    index_seconds: f64,
    graph_phase_seconds: f64,
    semantic_phase_seconds: f64,
    repeat_full_refresh_seconds: f64,
    retrieval_index_seconds: f64,
    retrieval_status_seconds: f64,
    search_seconds: f64,
}

#[derive(Debug, Serialize)]
struct IndexStats {
    node_count: u64,
    edge_count: u64,
    file_count: u64,
    error_count: u64,
    retrieval_status_after_index: String,
    legacy_index_retrieval_mode: String,
    semantic_doc_count: u64,
}

#[derive(Debug, Serialize)]
struct GroundStats {
    retrieval_status_after_index: String,
    legacy_ground_retrieval_mode: String,
    root_symbols: usize,
    file_digests: usize,
    coverage_total_files: u64,
}

#[derive(Debug, Serialize)]
struct SearchStats {
    query: String,
    retrieval_shadow_mode: String,
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
const PROOF_TIER_FULL_RETRIEVAL: &str = "full_retrieval";
const RELEASE_WARNING_REGRESSION_FACTOR: f64 = 1.25;

fn release_readiness_proof_tier(
    retrieval_status_after_index: &str,
    search_retrieval_shadow_mode: &str,
) -> &'static str {
    if retrieval_status_after_index == "full" && search_retrieval_shadow_mode == "full" {
        PROOF_TIER_FULL_RETRIEVAL
    } else {
        PROOF_TIER_STATS_ONLY
    }
}

fn release_readiness_warnings(
    index_seconds: f64,
    semantic_phase_seconds: f64,
    retrieval_index_seconds: f64,
    retrieval_status_seconds: f64,
    search_seconds: f64,
    baseline: &StatsLogBaseline,
) -> Vec<String> {
    let mut warnings = Vec::new();
    push_release_warning(
        &mut warnings,
        "index_seconds",
        index_seconds,
        baseline.index_seconds,
        baseline,
    );
    push_release_warning(
        &mut warnings,
        "semantic_phase_seconds",
        semantic_phase_seconds,
        baseline.semantic_phase_seconds,
        baseline,
    );
    push_release_warning(
        &mut warnings,
        "retrieval_index_seconds",
        retrieval_index_seconds,
        baseline.retrieval_index_seconds,
        baseline,
    );
    push_release_warning(
        &mut warnings,
        "retrieval_status_seconds",
        retrieval_status_seconds,
        baseline.retrieval_status_seconds,
        baseline,
    );
    push_release_warning(
        &mut warnings,
        "search_seconds",
        search_seconds,
        baseline.search_seconds,
        baseline,
    );
    warnings
}

fn push_release_warning(
    warnings: &mut Vec<String>,
    metric: &str,
    current_seconds: f64,
    baseline_seconds: f64,
    baseline: &StatsLogBaseline,
) {
    let threshold = baseline_seconds * RELEASE_WARNING_REGRESSION_FACTOR;
    if current_seconds > threshold {
        warnings.push(format!(
            "{metric} exceeded latest stats-log baseline by >25%: current {current_seconds:.2}s > threshold {threshold:.2}s from {} {} ({baseline_seconds:.2}s)",
            baseline.date, baseline.commit
        ));
    }
}

fn latest_phase_stats_baseline(repo_root: &Path) -> StatsLogBaseline {
    let source_path = repo_root.join("docs/testing/codestory-e2e-stats-log.md");
    let log = fs::read_to_string(&source_path).unwrap_or_else(|error| {
        panic!(
            "failed to read release stats baseline {}: {error}",
            source_path.display()
        )
    });
    let mut baseline = latest_phase_stats_baseline_from_str(&log)
        .expect("docs/testing/codestory-e2e-stats-log.md must contain a Phase Metrics row");
    baseline.source_path = source_path.display().to_string();
    baseline
}

fn latest_phase_stats_baseline_from_str(log: &str) -> Option<StatsLogBaseline> {
    let mut in_phase_table = false;
    let mut latest = None;
    let mut latest_runtime = None;
    for line in log.lines() {
        if line.trim() == "## Phase Metrics" {
            in_phase_table = true;
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let fields = line.split('|').skip(1).map(str::trim).collect::<Vec<_>>();
        if fields.len() >= 15 && fields[0] != "Date" && !fields[0].starts_with("---") {
            let Some(search_seconds) = parse_stats_seconds(fields[5]) else {
                continue;
            };
            let Some(retrieval_index_seconds) =
                parse_named_result_seconds(fields[2], "retrieval_index_seconds")
            else {
                continue;
            };
            let Some(retrieval_status_seconds) =
                parse_named_result_seconds(fields[2], "retrieval_status_seconds")
            else {
                continue;
            };
            latest_runtime = Some((
                retrieval_index_seconds,
                retrieval_status_seconds,
                search_seconds,
            ));
            continue;
        }
        if !in_phase_table {
            continue;
        }
        if fields.len() < 9 || fields[0] == "Date" || fields[0].starts_with("---") {
            continue;
        }
        let Some(index_seconds) = parse_stats_seconds(fields[3]) else {
            continue;
        };
        let Some(graph_phase_seconds) = parse_stats_seconds(fields[4]) else {
            continue;
        };
        let Some(semantic_phase_seconds) = parse_stats_seconds(fields[5]) else {
            continue;
        };
        let Some(repeat_full_refresh_seconds) = parse_repeat_full_refresh_seconds(fields[2]) else {
            continue;
        };
        let (retrieval_index_seconds, retrieval_status_seconds, search_seconds) = latest_runtime?;
        latest = Some(StatsLogBaseline {
            source_path: String::new(),
            date: fields[0].to_string(),
            commit: fields[1].to_string(),
            scenario: fields[2].to_string(),
            index_seconds,
            graph_phase_seconds,
            semantic_phase_seconds,
            repeat_full_refresh_seconds,
            retrieval_index_seconds,
            retrieval_status_seconds,
            search_seconds,
        });
    }
    latest
}

fn parse_named_result_seconds(value: &str, marker: &str) -> Option<f64> {
    let start = value.find(marker)? + marker.len();
    let seconds = value[start..]
        .split(';')
        .next()?
        .trim()
        .trim_end_matches('s');
    parse_stats_seconds(seconds)
}

fn parse_repeat_full_refresh_seconds(value: &str) -> Option<f64> {
    let marker = "repeat full refresh ";
    let start = value.find(marker)? + marker.len();
    let seconds = value[start..].split_once('s')?.0.trim();
    parse_stats_seconds(seconds)
}

fn parse_stats_seconds(value: &str) -> Option<f64> {
    let value = value.replace(',', "");
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn retained_phase_timings(index_json: &Value) -> Value {
    index_json
        .get("phase_timings")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn retained_phase_timings_preserve_additive_diagnostics() {
    let index_json = serde_json::json!({
        "phase_timings": {
            "parse_index_ms": 12,
            "future_additive_diagnostic": {"rows": 34}
        }
    });

    let retained = retained_phase_timings(&index_json);

    assert_eq!(retained["parse_index_ms"], 12);
    assert_eq!(retained["future_additive_diagnostic"]["rows"], 34);
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
fn release_readiness_proof_tier_requires_full_retrieval_evidence() {
    assert_eq!(
        release_readiness_proof_tier("full", "full"),
        PROOF_TIER_FULL_RETRIEVAL
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

    assert_eq!(proof_tier, PROOF_TIER_FULL_RETRIEVAL);
    assert_ne!(proof_tier, "real_repo_drill");
    assert_ne!(proof_tier, "promotion_grade");
}

#[test]
fn release_readiness_warnings_only_emit_above_thresholds() {
    let baseline = StatsLogBaseline {
        source_path: "docs/testing/codestory-e2e-stats-log.md".to_string(),
        date: "2026-06-24".to_string(),
        commit: "f8ffbd15+wt".to_string(),
        scenario: "release evidence".to_string(),
        index_seconds: 80.0,
        graph_phase_seconds: 16.0,
        semantic_phase_seconds: 50.0,
        repeat_full_refresh_seconds: 30.0,
        retrieval_index_seconds: 12.0,
        retrieval_status_seconds: 0.8,
        search_seconds: 4.0,
    };

    assert!(release_readiness_warnings(100.0, 62.5, 15.0, 1.0, 5.0, &baseline).is_empty());

    assert_eq!(
        release_readiness_warnings(100.01, 62.51, 15.01, 1.01, 5.01, &baseline),
        vec![
            "index_seconds exceeded latest stats-log baseline by >25%: current 100.01s > threshold 100.00s from 2026-06-24 f8ffbd15+wt (80.00s)".to_string(),
            "semantic_phase_seconds exceeded latest stats-log baseline by >25%: current 62.51s > threshold 62.50s from 2026-06-24 f8ffbd15+wt (50.00s)".to_string(),
            "retrieval_index_seconds exceeded latest stats-log baseline by >25%: current 15.01s > threshold 15.00s from 2026-06-24 f8ffbd15+wt (12.00s)".to_string(),
            "retrieval_status_seconds exceeded latest stats-log baseline by >25%: current 1.01s > threshold 1.00s from 2026-06-24 f8ffbd15+wt (0.80s)".to_string(),
            "search_seconds exceeded latest stats-log baseline by >25%: current 5.01s > threshold 5.00s from 2026-06-24 f8ffbd15+wt (4.00s)".to_string(),
        ]
    );
}

#[test]
fn latest_phase_stats_baseline_parses_last_valid_phase_row() {
    let baseline = latest_phase_stats_baseline_from_str(
        r#"
| Date | Commit | Result | Index seconds | Ground seconds | Search seconds | Symbol seconds | Trail seconds | Snippet seconds | Nodes | Edges | Files | Index errors | Semantic docs | Search dir unchanged |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-06-23 | old+wt | pass; retrieval_index_seconds 8.00; retrieval_status_seconds 0.40 | 90.00 | 0.20 | 2.00 | 0.50 | 0.20 | 0.20 | 1 | 1 | 1 | 0 | 1 | true |
| 2026-06-24 | new+wt | pass; retrieval_index_seconds 9.50; retrieval_status_seconds 0.70 | 85.00 | 0.20 | 3.40 | 0.50 | 0.20 | 0.20 | 1 | 1 | 1 | 0 | 1 | true |

## Phase Metrics

| Date | Commit | Scenario | Index seconds | Graph phase seconds | Semantic phase seconds | Semantic docs reused | Semantic docs embedded | Semantic docs stale |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-06-23 | old+wt | old stats; repeat full refresh 30.00s with 0 embedded | 90.00 | 15.00 | 60.00 | 0 | 1 | 0 |
| 2026-06-24 | new+wt | latest stats; repeat full refresh 32.57s with 907 reused and 0 embedded | 85.15 | 17.79 | 55.34 | 0 | 907 | 0 |
"#,
    )
    .expect("phase baseline");

    assert_eq!(baseline.date, "2026-06-24");
    assert_eq!(baseline.commit, "new+wt");
    assert_eq!(
        baseline.scenario,
        "latest stats; repeat full refresh 32.57s with 907 reused and 0 embedded"
    );
    assert_eq!(baseline.index_seconds, 85.15);
    assert_eq!(baseline.graph_phase_seconds, 17.79);
    assert_eq!(baseline.semantic_phase_seconds, 55.34);
    assert_eq!(baseline.repeat_full_refresh_seconds, 32.57);
    assert_eq!(baseline.retrieval_index_seconds, 9.50);
    assert_eq!(baseline.retrieval_status_seconds, 0.70);
    assert_eq!(baseline.search_seconds, 3.40);
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
    parent.join(format!("{stem}.search-generations"))
}

fn path_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| path_bytes(&entry.path()))
        .sum()
}

fn run_cli_json(
    binary: &Path,
    project_root: &Path,
    cache_dir: &Path,
    args: &[String],
) -> (f64, Value) {
    run_cli_json_with_sidecar_cache_root(binary, project_root, cache_dir, cache_dir, args)
}

fn run_cli_json_with_sidecar_cache_root(
    binary: &Path,
    project_root: &Path,
    cache_dir: &Path,
    sidecar_cache_root: &Path,
    args: &[String],
) -> (f64, Value) {
    let (seconds, stdout) = run_cli_output_with_sidecar_cache_root(
        binary,
        project_root,
        cache_dir,
        sidecar_cache_root,
        args,
    );
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
    run_cli_output_with_sidecar_cache_root(binary, project_root, cache_dir, cache_dir, args)
}

fn run_cli_output_with_sidecar_cache_root(
    binary: &Path,
    project_root: &Path,
    cache_dir: &Path,
    sidecar_cache_root: &Path,
    args: &[String],
) -> (f64, Vec<u8>) {
    let started = Instant::now();
    let output = test_support::command(binary)
        .current_dir(project_root)
        .args(args)
        .arg("--project")
        .arg(project_root)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env_remove("CODESTORY_STDIO_CACHE_ROOT")
        .env("CODESTORY_CACHE_ROOT", sidecar_cache_root)
        .env("CODESTORY_EMBED_ALLOW_CPU", "1")
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
#[ignore = "repo-scale release e2e; set CODESTORY_EMBED_MODEL_SOURCE to the output of node scripts/prepare-embedded-model.mjs, build with cargo build --release -p codestory-cli, then run cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture"]
fn codestory_repo_release_e2e_emits_stats() {
    let project_root = repo_root();
    let binary = release_cli_binary();
    let sidecar_run_id = "release-e2e-stats";
    assert!(
        binary.is_file(),
        "missing release binary at {}. Set CODESTORY_EMBED_MODEL_SOURCE to the output of `node scripts/prepare-embedded-model.mjs`, then run `cargo build --release -p codestory-cli`.",
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

    let (_ready_repair_seconds, _ready_repair_json) = run_cli_json(
        &binary,
        project_root.as_path(),
        cache_dir.path(),
        &[
            "retrieval".to_string(),
            "index".to_string(),
            "--profile".to_string(),
            "agent".to_string(),
            "--refresh".to_string(),
            "auto".to_string(),
            "--run-id".to_string(),
            sidecar_run_id.to_string(),
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
            "--profile".to_string(),
            "agent".to_string(),
            "--run-id".to_string(),
            sidecar_run_id.to_string(),
            "--refresh".to_string(),
            "full".to_string(),
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
            "--profile".to_string(),
            "agent".to_string(),
            "--run-id".to_string(),
            sidecar_run_id.to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let sidecar_retrieval_mode =
        string_field(&retrieval_status_json, &["retrieval_mode"]).to_string();
    let retrieval_degraded_reason = retrieval_status_json
        .get("degraded_reason")
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    assert_eq!(
        sidecar_retrieval_mode,
        "full",
        "retrieval status after retrieval index should be full before trusting index/ground/search evidence; degraded_reason={}; manifest dense_projection_count={} projection_count={} symbol_doc_count={}",
        retrieval_degraded_reason,
        optional_u64_field(
            &retrieval_status_json,
            &["manifest", "dense_projection_count"]
        ),
        optional_u64_field(&retrieval_status_json, &["manifest", "projection_count"]),
        optional_u64_field(&retrieval_status_json, &["manifest", "symbol_doc_count"])
    );

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
            "--profile".to_string(),
            "agent".to_string(),
            "--run-id".to_string(),
            sidecar_run_id.to_string(),
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
    let search_retrieval_shadow_mode =
        string_field(&search_json, &["retrieval_shadow", "retrieval_mode"]).to_string();
    let dense_reason_counts_json = string_field(
        &retrieval_status_json,
        &["manifest", "dense_reason_counts_json"],
    )
    .to_string();
    let retrieval_manifest = RetrievalManifestStats {
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
        search_retrieval_shadow_mode.as_str(),
    )
    .to_string();
    let stats_baseline = latest_phase_stats_baseline(project_root.as_path());
    let warnings = release_readiness_warnings(
        index_seconds,
        semantic_phase_seconds,
        retrieval_index_seconds,
        retrieval_status_seconds,
        search_seconds,
        &stats_baseline,
    );

    let stats = RepoE2eStats {
        commit: env::var("CODESTORY_RELEASE_EVIDENCE_COMMIT")
            .unwrap_or_else(|_| "unbound-local-run".to_string()),
        evidence_identity: EvidenceIdentity {
            corpus_id: env::var("CODESTORY_RELEASE_EVIDENCE_CORPUS_ID")
                .unwrap_or_else(|_| "unbound-local-run".to_string()),
            cache_id: env::var("CODESTORY_RELEASE_EVIDENCE_CACHE_ID")
                .unwrap_or_else(|_| "unbound-local-run".to_string()),
            machine_fingerprint: env::var("CODESTORY_RELEASE_EVIDENCE_MACHINE_FINGERPRINT")
                .unwrap_or_else(|_| "unbound-local-run".to_string()),
        },
        project_root: project_root.display().to_string(),
        cache_dir: cache_dir.path().display().to_string(),
        storage_path: storage_path.display().to_string(),
        search_dir: search_dir.display().to_string(),
        storage_bytes: path_bytes(cache_dir.path()),
        proof_tier,
        warnings,
        stats_baseline,
        embed_batch_size: 128,
        search_dir_unchanged,
        index_seconds,
        phase_timings: retained_phase_timings(&index_json),
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
        repeat_phase_timings: retained_phase_timings(&repeat_index_json),
        repeat_graph_phase_seconds: repeat_graph_phase_ms as f64 / 1000.0,
        repeat_semantic_phase_seconds: repeat_semantic_phase_ms as f64 / 1000.0,
        repeat_semantic_doc_build_ms,
        repeat_semantic_embedding_ms,
        repeat_semantic_db_upsert_ms,
        repeat_semantic_reload_ms,
        repeat_semantic_prune_ms,
        repeat_cache_refresh_ms: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "cache_refresh_ms"],
        ),
        repeat_search_projection_rebuild_ms: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "search_projection_rebuild_ms"],
        ),
        repeat_search_symbol_index_ms: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "search_symbol_index_ms"],
        ),
        repeat_runtime_cache_publish_ms: optional_u64_field(
            &repeat_index_json,
            &["phase_timings", "runtime_cache_publish_ms"],
        ),
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
        retrieval_manifest,
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
            retrieval_status_after_index: sidecar_retrieval_mode.clone(),
            legacy_index_retrieval_mode: string_field(&index_json, &["retrieval", "mode"])
                .to_string(),
            semantic_doc_count: u64_field(&index_json, &["retrieval", "semantic_doc_count"]),
        },
        ground: GroundStats {
            retrieval_status_after_index: sidecar_retrieval_mode.clone(),
            legacy_ground_retrieval_mode: string_field(&ground_json, &["retrieval", "mode"])
                .to_string(),
            root_symbols: array_len(&ground_json, &["root_symbols"]),
            file_digests: array_len(&ground_json, &["files"]),
            coverage_total_files: u64_field(&ground_json, &["coverage", "total_files"]),
        },
        search: SearchStats {
            query: string_field(&search_json, &["query"]).to_string(),
            retrieval_shadow_mode: search_retrieval_shadow_mode,
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

    let stats_json = serde_json::to_string_pretty(&stats).expect("serialize stats");
    println!("{stats_json}");
    if let Ok(output_path) = env::var("CODESTORY_RELEASE_EVIDENCE_STATS_PATH") {
        let output_path = PathBuf::from(output_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).expect("create release evidence stats parent");
        }
        fs::write(output_path, format!("{stats_json}\n"))
            .expect("write release evidence stats artifact");
    }

    assert_eq!(
        stats.index.error_count, 0,
        "full repo index should finish without errors"
    );
    assert_eq!(
        stats.index.retrieval_status_after_index, "full",
        "retrieval status after retrieval index should be full before trusting index/ground/search evidence"
    );
    assert_eq!(
        stats.proof_tier, PROOF_TIER_FULL_RETRIEVAL,
        "repo e2e stats harness proves full retrieval evidence but does not run real-repo drill cases"
    );
    assert_eq!(
        stats.ground.retrieval_status_after_index, "full",
        "strict grounding should reuse the prepared full retrieval state"
    );
    assert_eq!(
        stats.search.retrieval_shadow_mode, "full",
        "search should expose full retrieval shadow"
    );
    assert!(
        stats.retrieval_manifest.symbol_doc_count > 0,
        "full retrieval manifest should record graph-native symbol docs"
    );
    assert!(
        stats.retrieval_manifest.dense_projection_count > 0,
        "CodeStory product run should select dense anchors"
    );
    assert_eq!(
        stats.retrieval_manifest.dense_projection_count, stats.retrieval_manifest.projection_count,
        "legacy projection_count should mirror dense_projection_count under graph_first_v1"
    );
    assert_eq!(
        stats.retrieval_manifest.semantic_policy_version, "graph_first_v1",
        "full retrieval manifest should record the active dense policy"
    );
    assert!(
        stats.retrieval_manifest.graph_artifact_hash_present,
        "full retrieval manifest should record a graph artifact hash"
    );
    assert_eq!(
        stats.retrieval_manifest.dense_reason_count_total,
        stats.retrieval_manifest.dense_projection_count,
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
}

#[test]
#[ignore = "real-repo drill release gate; set CODESTORY_REAL_REPO_DRILL_CASES or CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1, set CODESTORY_EMBED_MODEL_SOURCE to the output of node scripts/prepare-embedded-model.mjs, build with cargo build --release -p codestory-cli, then run the ignored test"]
fn real_repo_agent_grounding_drill_emits_verification_packets() {
    let binary = release_cli_binary();
    assert!(
        binary.is_file(),
        "missing release binary at {}. Set CODESTORY_EMBED_MODEL_SOURCE to the output of `node scripts/prepare-embedded-model.mjs`, then run `cargo build --release -p codestory-cli`.",
        binary.display()
    );

    let temporary_output = tempdir().expect("drill output dir");
    let root_output = env::var_os("CODESTORY_RELEASE_EVIDENCE_DRILL_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary_output.path().to_path_buf());
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
    let (_seconds, suite_json) = run_cli_json_with_sidecar_cache_root(
        &binary,
        repo_root().as_path(),
        cache_dir.path(),
        cache_dir.path(),
        &[
            "drill-suite".to_string(),
            "--refresh".to_string(),
            "full".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--output-dir".to_string(),
            root_output.display().to_string(),
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
        u64_field(&suite_json, &["blocked_count"]),
        0,
        "drill-suite should complete every configured repo before its evidence is evaluated: {suite_json:#}"
    );
    assert_eq!(
        array_len(&suite_json, &["repos"]),
        cases.len(),
        "suite should include exactly the manifest real-repo drill cases"
    );
    assert!(
        root_output.join("suite-report.md").is_file(),
        "drill-suite should write a markdown aggregate report"
    );
    assert!(
        root_output.join("suite-report.json").is_file(),
        "drill-suite should write a JSON aggregate report"
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
            string_field(&drill_json, &["question_search", "command"]),
            "packet",
            "{} drill should execute the packet path once",
            case.name
        );
        assert_packet_plan_names_seed_anchors(case, &drill_json);
        assert_eq!(
            string_field(&drill_json, &["question_search", "status"]),
            string_field(&drill_json, &["evidence_packet", "sufficiency", "status"]),
            "{} drill status should be the packet sufficiency decision",
            case.name
        );
        assert_compact_bridge_status_handoff(&case.name, repo_json);

        for anchor_index in 0..case.anchors.len() {
            let index = anchor_index.to_string();
            assert_eq!(
                string_field(&drill_json, &["anchors", index.as_str(), "anchor"]),
                case.anchors[anchor_index].as_str(),
                "{} drill should keep anchor order",
                case.name
            );
            assert!(
                u64_field(&drill_json, &["anchors", index.as_str(), "typed_hit_count"]) > 0,
                "{} anchor {} should retain typed search hits",
                case.name,
                case.anchors[anchor_index]
            );
            assert_eq!(
                array_len(&drill_json, &["anchors", index.as_str(), "commands"]),
                0,
                "{} drill anchors should adapt packet citations without rerunning commands",
                case.name
            );
        }

        assert_manifest_anchor_expectations(case, repo_json);
    }
}

fn allow_skip_real_repo_drill_cases() -> bool {
    env::var("CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn assert_packet_plan_names_seed_anchors(case: &DrillRepoCase, drill_json: &Value) {
    let subqueries = json_path(drill_json, &["evidence_packet", "plan", "queries"])
        .as_array()
        .expect("packet plan queries");
    for anchor in &case.anchors {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery["query"].as_str() == Some(anchor.as_str())),
            "{} packet plan should preserve explicit anchor probe {anchor}: {drill_json:#}",
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
mod test_support;
