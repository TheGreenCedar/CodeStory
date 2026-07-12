use crate::cache::RetrievalCache;
use crate::cache::RetrievalCacheKey;
use crate::config::{SidecarLayout, SidecarRuntimeConfig};
use crate::embeddings::{EmbeddingDeviceReadiness, embedding_device_readiness_for_runtime};
use crate::executor::{QueryExecutor, QueryResult, cancellation_flag};
use crate::generation::manifest_unavailable_reason_for_runtime;
use crate::health::probe_sidecar_health_for_runtime;
use crate::index::{query_fingerprint, sidecar_project_id_for_root};
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode};
use crate::query_features::classify_query;
use crate::sidecar::validate_strict_sidecar_readiness;
use crate::sidecar_search::LiveSidecarSearch;
use crate::sidecar_search::SidecarSearch;
use anyhow::{Context, Result, bail};
use codestory_store::{FileRole, RetrievalIndexManifest, Store};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const STRICT_BATCH_WORKER_CAP: usize = 4;

#[derive(Debug, Clone)]
pub struct QueryRequest<'a> {
    pub project_root: &'a Path,
    pub storage_path: &'a Path,
    pub query: &'a str,
    pub budget_ms: Option<u64>,
    pub cancelled: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone)]
pub struct QueryBatchItem<'a> {
    pub query: &'a str,
    pub budget_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct QueryBatchRequest<'a> {
    pub project_root: &'a Path,
    pub storage_path: &'a Path,
    pub queries: &'a [QueryBatchItem<'a>],
    pub cancelled: Option<Arc<AtomicBool>>,
}

pub fn execute_retrieval_query(request: QueryRequest<'_>) -> Result<QueryResult> {
    let mut cache = RetrievalCache::new();
    execute_retrieval_query_with_cache(request, &mut cache)
}

pub fn execute_retrieval_query_with_cache(
    request: QueryRequest<'_>,
    cache: &mut RetrievalCache,
) -> Result<QueryResult> {
    let runtime = SidecarRuntimeConfig::for_project_auto(request.project_root);
    execute_retrieval_query_with_cache_for_runtime(request, cache, &runtime)
}

pub fn execute_retrieval_query_with_cache_for_runtime(
    request: QueryRequest<'_>,
    cache: &mut RetrievalCache,
    runtime: &SidecarRuntimeConfig,
) -> Result<QueryResult> {
    let cancelled = request.cancelled.unwrap_or_else(cancellation_flag);
    if cancelled.load(Ordering::Acquire) {
        bail!("retrieval query cancelled before preflight");
    }
    let QueryContext {
        layout,
        project_id,
        manifest,
        file_roles,
        embedding_device,
    } = load_query_context(request.project_root, request.storage_path, runtime)?;
    let sidecars = Arc::new(LiveSidecarSearch::new_for_runtime_with_embedding_device(
        runtime,
        layout,
        project_id,
        manifest.as_ref(),
        Some(embedding_device),
    )?);
    let mut executor = QueryExecutor {
        sidecars,
        cache,
        manifest,
        file_roles,
        cancelled,
        mode_override: None,
    };
    executor.execute(request.query, request.budget_ms)
}

pub fn execute_strict_retrieval_query_batch_with_cache(
    request: QueryBatchRequest<'_>,
    cache: &mut RetrievalCache,
) -> Result<Vec<QueryResult>> {
    let runtime = SidecarRuntimeConfig::for_project_auto(request.project_root);
    execute_strict_retrieval_query_batch_with_cache_for_runtime(request, cache, &runtime)
}

pub fn execute_strict_retrieval_query_batch_with_cache_for_runtime(
    request: QueryBatchRequest<'_>,
    cache: &mut RetrievalCache,
    runtime: &SidecarRuntimeConfig,
) -> Result<Vec<QueryResult>> {
    if request.queries.is_empty() {
        return Ok(Vec::new());
    }
    let cancelled = request.cancelled.unwrap_or_else(cancellation_flag);
    if cancelled.load(Ordering::Acquire) {
        bail!("retrieval query batch cancelled before preflight");
    }
    let QueryContext {
        layout,
        project_id,
        manifest,
        file_roles,
        embedding_device,
    } = load_query_context(request.project_root, request.storage_path, runtime)?;
    let sidecars = Arc::new(LiveSidecarSearch::new_for_runtime_with_embedding_device(
        runtime,
        layout,
        project_id,
        manifest.as_ref(),
        Some(embedding_device.clone()),
    )?);
    let (mode, degraded_reason) = resolve_batch_mode(
        sidecars.as_ref(),
        manifest.as_ref(),
        &embedding_device,
        runtime,
    );
    if mode != RetrievalDegradedMode::Full {
        bail!(
            "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason={})",
            mode.as_str(),
            degraded_reason.as_deref().unwrap_or("unknown")
        );
    }
    execute_strict_retrieval_query_batch_against_sidecars(
        sidecars,
        manifest,
        file_roles,
        cancelled,
        mode,
        request.queries,
        cache,
        strict_batch_worker_limit(request.queries.len()),
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_strict_retrieval_query_batch_against_sidecars(
    sidecars: Arc<dyn SidecarSearch>,
    manifest: Option<RetrievalIndexManifest>,
    file_roles: Arc<HashMap<String, FileRole>>,
    cancelled: Arc<AtomicBool>,
    mode: RetrievalDegradedMode,
    queries: &[QueryBatchItem<'_>],
    cache: &mut RetrievalCache,
    worker_limit: usize,
) -> Result<Vec<QueryResult>> {
    if mode != RetrievalDegradedMode::Full {
        bail!(
            "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason=unknown)",
            mode.as_str()
        );
    }
    if cancelled.load(Ordering::Acquire) {
        bail!("retrieval query batch cancelled before cache lookup");
    }

    let mut results = vec![None; queries.len()];
    let mut misses = Vec::new();
    for (index, query) in queries.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            bail!("retrieval query batch cancelled during cache lookup");
        }
        if let Some(result) =
            cached_batch_result(manifest.as_ref(), cache, query.query, mode, &cancelled)
        {
            results[index] = Some(result);
        } else {
            misses.push((index, query.query.to_string(), query.budget_ms));
        }
    }

    for wave in misses.chunks(worker_limit.max(1)) {
        if cancelled.load(Ordering::Acquire) {
            bail!("retrieval query batch cancelled before worker wave");
        }
        let wave_results = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(wave.len());
            for (index, query, budget_ms) in wave {
                let manifest = manifest.clone();
                let file_roles = Arc::clone(&file_roles);
                let cancelled = Arc::clone(&cancelled);
                let sidecars = Arc::clone(&sidecars);
                handles.push(scope.spawn(move || {
                    let mut worker_cache = RetrievalCache::new();
                    let mut executor = QueryExecutor {
                        sidecars,
                        cache: &mut worker_cache,
                        manifest,
                        file_roles,
                        cancelled,
                        mode_override: Some(mode),
                    };
                    (*index, executor.execute(query, *budget_ms))
                }));
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .expect("strict retrieval batch worker panicked")
                })
                .collect::<Vec<_>>()
        });

        for (index, result) in wave_results {
            let result = result?;
            if cancelled.load(Ordering::Acquire) {
                bail!("retrieval query batch cancelled after worker wave");
            }
            cache_completed_batch_result(manifest.as_ref(), cache, &result, &cancelled);
            results[index] = Some(result);
        }
    }

    results
        .into_iter()
        .map(|result| result.context("strict retrieval batch dropped a query result"))
        .collect()
}

fn cached_batch_result(
    manifest: Option<&RetrievalIndexManifest>,
    cache: &RetrievalCache,
    query: &str,
    mode: RetrievalDegradedMode,
    cancelled: &AtomicBool,
) -> Option<QueryResult> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let manifest = manifest?;
    let features = classify_query(query);
    let key = RetrievalCacheKey::from_manifest(manifest, query_fingerprint(&features.raw_query));
    let hits = cache.get(&key)?.to_vec();
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    Some(QueryResult {
        query: features.raw_query.clone(),
        features,
        hits,
        trace: crate::executor::QueryTrace {
            retrieval_mode: mode.as_str().into(),
            degraded_reason: None,
            total_budget_ms: 0,
            elapsed_ms: 0,
            cancel_reason: None,
            cache_hit: true,
            stages: Vec::new(),
        },
    })
}

fn cache_completed_batch_result(
    manifest: Option<&RetrievalIndexManifest>,
    cache: &mut RetrievalCache,
    result: &QueryResult,
    cancelled: &AtomicBool,
) {
    if result.trace.cancel_reason.is_some() || cancelled.load(Ordering::Acquire) {
        return;
    }
    if let Some(manifest) = manifest {
        let key = RetrievalCacheKey::from_manifest(
            manifest,
            query_fingerprint(&result.features.raw_query),
        );
        if !cancelled.load(Ordering::Acquire) {
            cache.insert(key.clone(), result.hits.clone());
            if cancelled.load(Ordering::Acquire) {
                cache.remove(&key);
            }
        }
    }
}

fn strict_batch_worker_limit(query_count: usize) -> usize {
    if query_count <= 1 {
        return 1;
    }
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    // Cap sidecar fan-out; make this configurable only if telemetry needs it.
    query_count.min(available).clamp(1, STRICT_BATCH_WORKER_CAP)
}

struct QueryContext {
    layout: SidecarLayout,
    project_id: String,
    manifest: Option<RetrievalIndexManifest>,
    file_roles: Arc<HashMap<String, FileRole>>,
    embedding_device: EmbeddingDeviceReadiness,
}

fn load_query_context(
    project_root: &Path,
    storage_path: &Path,
    runtime: &SidecarRuntimeConfig,
) -> Result<QueryContext> {
    let embedding_device = embedding_device_readiness_for_runtime(runtime);
    let layout = runtime.layout.clone();
    let project_id = sidecar_project_id_for_root(project_root);
    let (manifest, file_roles) = if storage_path.exists() {
        let storage = Store::open(storage_path).context("open storage for query")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if let Some(manifest) = manifest.as_ref() {
            if let Err(error) =
                validate_strict_sidecar_readiness(project_root, storage_path, &storage)
            {
                bail!(
                    "retrieval sidecar manifest is unavailable ({error}); run retrieval index for project {project_id}"
                );
            }
            if let Some(reason) =
                manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, runtime)
            {
                bail!(
                    "retrieval sidecar manifest is unavailable ({reason}); run retrieval index for project {project_id}"
                );
            }
        } else {
            bail!(
                "retrieval sidecar manifest is missing; run retrieval index for project {project_id}"
            );
        }
        let roles = storage
            .get_files()
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| (file.path.to_string_lossy().to_string(), file.file_role))
                    .collect()
            })
            .unwrap_or_default();
        (manifest, roles)
    } else {
        bail!("retrieval sidecar storage is missing; run retrieval index for project {project_id}");
    };

    Ok(QueryContext {
        layout,
        project_id,
        manifest,
        file_roles: Arc::new(file_roles),
        embedding_device,
    })
}

fn resolve_batch_mode(
    sidecars: &dyn SidecarSearch,
    manifest: Option<&RetrievalIndexManifest>,
    embedding_device: &EmbeddingDeviceReadiness,
    runtime: &SidecarRuntimeConfig,
) -> (RetrievalDegradedMode, Option<String>) {
    if let Some(manifest) = manifest {
        let Some(layout) = sidecars.layout() else {
            return (
                RetrievalDegradedMode::Unavailable,
                Some("sidecar_layout_missing".into()),
            );
        };
        let report = probe_sidecar_health_for_runtime(
            layout,
            &manifest.project_id,
            Some(manifest.clone()),
            embedding_device,
            runtime,
        );
        return derive_degraded_mode(&report.lexical, &report.qdrant, &report.scip);
    }
    (
        RetrievalDegradedMode::LexicalOnly,
        Some("manifest_missing".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CandidateHit;
    use crate::index::finalize_index;
    use crate::sidecar_search::SidecarSearch;
    use crate::test_support::retrieval_manifest_fixture;
    use crate::{QdrantClient, SidecarLayout};
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole, LlmSymbolDoc, SearchSymbolProjection};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tempfile::TempDir;

    fn manifest_for(
        project_id: &str,
        hash: &str,
        projection_count: i64,
    ) -> codestory_store::RetrievalIndexManifest {
        let mut manifest = retrieval_manifest_fixture(project_id, hash);
        manifest.projection_count = Some(projection_count);
        manifest.symbol_doc_count = Some(projection_count);
        manifest.dense_projection_count = Some(projection_count);
        manifest.dense_reason_counts_json = Some(format!("{{\"public_api\":{projection_count}}}"));
        manifest
    }

    #[test]
    fn empty_batch_query_does_not_require_storage() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("missing").join("codestory.db");
        let mut cache = RetrievalCache::new();

        let results = execute_strict_retrieval_query_batch_with_cache(
            QueryBatchRequest {
                project_root: project.path(),
                storage_path: &storage_path,
                queries: &[],
                cancelled: None,
            },
            &mut cache,
        )
        .expect("empty batch should short-circuit before storage setup");

        assert!(results.is_empty());
    }

    #[test]
    fn strict_batch_runs_cache_misses_bounded_and_keeps_order() {
        struct CountingSidecars {
            active: AtomicUsize,
            max_active: AtomicUsize,
            first_wave: std::sync::Barrier,
        }

        impl CountingSidecars {
            fn record(&self, query: &str) -> Vec<CandidateHit> {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                if matches!(query, "slow" | "fast") {
                    self.first_wave.wait();
                }
                if query == "slow" {
                    std::thread::sleep(Duration::from_millis(30));
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
                self.active.fetch_sub(1, Ordering::SeqCst);
                vec![CandidateHit::lexical_stub(format!("src/{query}.rs"), 1.0)]
            }
        }

        impl SidecarSearch for CountingSidecars {
            fn lexical_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(self.record(query))
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let sidecars = Arc::new(CountingSidecars {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            first_wave: std::sync::Barrier::new(2),
        });
        let mut cache = RetrievalCache::new();
        let manifest = manifest_for("testproj", "cafebabedeadbeef", 3);
        let queries = [
            QueryBatchItem {
                query: "slow",
                budget_ms: Some(500),
            },
            QueryBatchItem {
                query: "fast",
                budget_ms: Some(500),
            },
            QueryBatchItem {
                query: "last",
                budget_ms: Some(500),
            },
        ];
        let file_roles = Arc::new(
            (0..10_000)
                .map(|index| (format!("src/unrelated_{index}.rs"), FileRole::Source))
                .chain([
                    ("src/slow.rs".to_string(), FileRole::Source),
                    ("src/fast.rs".to_string(), FileRole::Test),
                    ("src/last.rs".to_string(), FileRole::Generated),
                ])
                .collect::<HashMap<_, _>>(),
        );

        let results = execute_strict_retrieval_query_batch_against_sidecars(
            sidecars.clone(),
            Some(manifest),
            Arc::clone(&file_roles),
            cancellation_flag(),
            RetrievalDegradedMode::Full,
            &queries,
            &mut cache,
            2,
        )
        .expect("batch");

        assert_eq!(
            results
                .iter()
                .map(|result| result.query.as_str())
                .collect::<Vec<_>>(),
            ["slow", "fast", "last"]
        );
        assert_eq!(file_roles.len(), 10_003);
        assert_eq!(
            results
                .iter()
                .flat_map(|result| result.hits.iter())
                .map(|hit| hit.file_role)
                .collect::<Vec<_>>(),
            [
                Some(FileRole::Source),
                Some(FileRole::Test),
                Some(FileRole::Generated)
            ]
        );
        assert_eq!(sidecars.max_active.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn strict_batch_rejects_non_full_mode_before_cache_hits() {
        let sidecars = crate::sidecar_search::mock::MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = manifest_for("testproj", "cafebabedeadbeef", 1);
        cache.insert(
            RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached")),
            vec![CandidateHit::lexical_stub("src/cached.rs", 1.0)],
        );
        let queries = [QueryBatchItem {
            query: "cached",
            budget_ms: Some(100),
        }];

        let error = execute_strict_retrieval_query_batch_against_sidecars(
            Arc::new(sidecars),
            Some(manifest),
            Arc::new(HashMap::new()),
            cancellation_flag(),
            RetrievalDegradedMode::NoSemantic,
            &queries,
            &mut cache,
            1,
        )
        .expect_err("non-full mode must fail before cache use");

        assert!(error.to_string().contains("retrieval sidecar is mandatory"));
    }

    #[test]
    fn strict_batch_cancellation_preflight_rejects_cache_hits() {
        let mut cache = RetrievalCache::new();
        let manifest = manifest_for("testproj", "cafebabedeadbeef", 1);
        cache.insert(
            RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached")),
            vec![CandidateHit::lexical_stub("src/cached.rs", 1.0)],
        );
        let queries = [QueryBatchItem {
            query: "cached",
            budget_ms: Some(100),
        }];
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Release);

        let error = execute_strict_retrieval_query_batch_against_sidecars(
            Arc::new(crate::sidecar_search::mock::MockSidecarSearch::default()),
            Some(manifest),
            Arc::new(HashMap::new()),
            cancelled,
            RetrievalDegradedMode::Full,
            &queries,
            &mut cache,
            1,
        )
        .expect_err("cancelled batch must not serve cache");

        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    #[ignore = "requires live Qdrant and embedding sidecars; run explicitly with cargo test -p codestory-retrieval integration_query_against_fixture_manifest -- --ignored --nocapture"]
    fn integration_query_against_fixture_manifest() {
        let layout = SidecarLayout::from_env();
        if !QdrantClient::new(&layout)
            .list_collections_probe()
            .reachable
        {
            return;
        }
        if crate::embeddings::embed_query("function").is_err() {
            return;
        }

        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("lib.rs"),
            "pub fn extension_service() {}",
        )
        .expect("write");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            let file_id = 10_i64;
            let source_path = project.path().join("lib.rs");
            storage
                .insert_file(&FileInfo {
                    id: file_id,
                    path: source_path.clone(),
                    language: "rust".to_string(),
                    modification_time: live_mtime_millis(&source_path),
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Entrypoint,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: NodeId(file_id),
                        kind: NodeKind::FILE,
                        serialized_name: "lib.rs".to_string(),
                        qualified_name: None,
                        canonical_id: None,
                        file_node_id: None,
                        start_line: Some(1),
                        start_col: Some(0),
                        end_line: Some(1),
                        end_col: Some(0),
                    },
                    Node {
                        id: NodeId(11),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "extension_service".to_string(),
                        qualified_name: Some("extension_service".to_string()),
                        canonical_id: None,
                        file_node_id: Some(NodeId(file_id)),
                        start_line: Some(1),
                        start_col: Some(0),
                        end_line: Some(1),
                        end_col: Some(30),
                    },
                ])
                .expect("insert nodes");
            storage
                .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
                    node_id: NodeId(11),
                    display_name: "extension_service".to_string(),
                }])
                .expect("projection");
            storage
                .upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
                    node_id: NodeId(11),
                    file_node_id: Some(NodeId(file_id)),
                    kind: NodeKind::FUNCTION,
                    display_name: "extension_service".to_string(),
                    qualified_name: Some("extension_service".to_string()),
                    file_path: Some(project.path().join("lib.rs").display().to_string()),
                    start_line: Some(1),
                    doc_text:
                        "semantic_doc_version: 4\nsymbol_kind: FUNCTION\nname: extension_service"
                            .to_string(),
                    doc_version: 4,
                    doc_hash: "extension-service-doc".to_string(),
                    embedding_profile: Some("bge-base-en-v1.5".to_string()),
                    embedding_model: "BAAI/bge-base-en-v1.5-local|backend=onnx".to_string(),
                    embedding_backend: Some("onnx".to_string()),
                    embedding_dim: 768,
                    doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".to_string()),
                    semantic_policy_version: Some(
                        crate::generation::SEMANTIC_POLICY_VERSION.into(),
                    ),
                    dense_reason: Some("public_api".into()),
                    embedding: vec![0.01; 768],
                    updated_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
                }])
                .expect("semantic doc");
        }
        if let Err(error) = finalize_index(project.path(), &storage_path) {
            eprintln!(
                "skipping live retrieval query fixture because sidecar indexing failed: {error:#}"
            );
            return;
        }

        let result = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "extension",
            budget_ms: Some(500),
            cancelled: None,
        })
        .expect("query");

        assert_eq!(result.trace.retrieval_mode, "full");
        assert!(!result.hits.is_empty() || !result.trace.stages.is_empty());
    }

    #[test]
    fn query_rejects_legacy_manifest_before_sidecar_access() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id,
                    lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
                    qdrant_collection: "codestory_legacy".into(),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: 1,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some("hash-projection:768".into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: None,
                    sidecar_input_hash: None,
                    sidecar_generation: None,
                    projection_count: None,
                    symbol_doc_count: None,
                    dense_projection_count: None,
                    semantic_policy_version: None,
                    graph_artifact_hash: None,
                    dense_reason_counts_json: None,
                    precise_semantic_import_status: None,
                    precise_semantic_import_reason: None,
                    precise_semantic_import_revision: None,
                    precise_semantic_import_producer: None,
                })
                .expect("manifest");
        }

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "ExtensionHostManager",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("legacy manifests must fail closed");

        assert!(error.to_string().contains("generation_contract_missing"));
    }

    #[test]
    fn query_rejects_manifest_with_stale_projection_count() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(&project_id, "deadbeefcafebabe", 10))
                .expect("manifest");
        }

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "ExtensionHostManager",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("stale manifests must fail closed");

        assert!(error.to_string().contains("sidecar_manifest_stale"));
    }

    #[test]
    fn query_rejects_manifest_when_indexed_file_changes_or_is_removed() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(
                    &project_id,
                    "changedfeedcafebeef",
                    0,
                ))
                .expect("manifest");
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&source_path, "pub fn indexed() -> usize { 1 }\n").expect("mutate source");
        let changed_error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "indexed",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("changed indexed file must fail closed");
        assert!(changed_error.to_string().contains("sidecar_manifest_stale"));

        std::fs::remove_file(&source_path).expect("remove source");
        let removed_error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "indexed",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("removed indexed file must fail closed");
        assert!(removed_error.to_string().contains("sidecar_manifest_stale"));
    }

    #[test]
    fn query_rejects_manifest_when_new_indexable_file_is_added() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(
                    &project_id,
                    "newfilefeedcafebeef",
                    0,
                ))
                .expect("manifest");
        }
        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "newly_added",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("new indexable file must fail closed");

        assert!(error.to_string().contains("sidecar_manifest_stale"));
    }

    fn live_mtime_millis(path: &Path) -> i64 {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("modified")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime since epoch")
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}
