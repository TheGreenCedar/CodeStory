use crate::cache::RetrievalCache;
#[cfg(test)]
use crate::cache::RetrievalCacheKey;
use crate::config::SidecarRuntimeConfig;
use crate::embeddings::{
    EmbeddingDeviceReadiness, ProductEmbeddingResidencyLease,
    acquire_product_embedding_residency_for_runtime, embedding_device_readiness_for_runtime,
};
use crate::executor::{
    QueryExecutor, QueryResult, RetrievalPublicationIdentity, cancellation_flag,
};
use crate::generation::manifest_unavailable_reason_for_runtime;
use crate::health::probe_sidecar_health_for_runtime;
use crate::index::{query_fingerprint, sidecar_project_id_for_runtime};
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode};
use crate::query_features::classify_query;
use crate::retention::GenerationRetentionLease;
use crate::sidecar::validate_strict_sidecar_readiness_for_runtime;
use crate::sidecar_search::LiveSidecarSearch;
use crate::sidecar_search::SidecarSearch;
use anyhow::{Context, Result, bail};
use codestory_store::{FileRole, RetrievalIndexManifest, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const STRICT_BATCH_WORKER_CAP: usize = 4;
pub const RETRIEVAL_PUBLICATION_CHANGED_CODE: &str = "publication_changed";

/// Typed signal that the complete query session must be discarded and retried by its caller.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "publication_changed: retrieval publication changed while {operation}; retry the complete query session"
)]
pub struct RetrievalPublicationChanged {
    operation: String,
    expected: RetrievalPublicationIdentity,
    observed: Option<RetrievalPublicationIdentity>,
    detail: Option<String>,
}

impl RetrievalPublicationChanged {
    pub fn code(&self) -> &'static str {
        RETRIEVAL_PUBLICATION_CHANGED_CODE
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn expected(&self) -> &RetrievalPublicationIdentity {
        &self.expected
    }

    pub fn observed(&self) -> Option<&RetrievalPublicationIdentity> {
        self.observed.as_ref()
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    fn changed(
        operation: impl Into<String>,
        expected: &RetrievalPublicationIdentity,
        observed: Option<RetrievalPublicationIdentity>,
    ) -> Self {
        Self {
            operation: operation.into(),
            expected: expected.clone(),
            observed,
            detail: None,
        }
    }

    fn unreadable(
        operation: impl Into<String>,
        expected: &RetrievalPublicationIdentity,
        error: impl std::fmt::Display,
    ) -> Self {
        Self {
            operation: operation.into(),
            expected: expected.clone(),
            observed: None,
            detail: Some(error.to_string()),
        }
    }
}

pub fn is_retrieval_publication_changed(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<RetrievalPublicationChanged>()
        .is_some()
}

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

/// One coherent retrieval read, from core SQLite evidence through sidecar execution and candidate
/// resolution. The caller may retry this whole session once when `revalidate` returns
/// [`RetrievalPublicationChanged`]; the session itself never retries.
pub struct PinnedQuerySession {
    storage: Store,
    storage_path: PathBuf,
    project_root: PathBuf,
    project_id: String,
    runtime: SidecarRuntimeConfig,
    manifest: RetrievalIndexManifest,
    file_roles: Arc<HashMap<String, FileRole>>,
    embedding_device: EmbeddingDeviceReadiness,
    publication_identity: RetrievalPublicationIdentity,
    sidecars: Arc<dyn SidecarSearch>,
    _generation_lease: GenerationRetentionLease,
    _embedding_residency: ProductEmbeddingResidencyLease,
    transaction_active: bool,
}

impl PinnedQuerySession {
    pub fn begin(
        project_root: &Path,
        storage_path: &Path,
        runtime: &SidecarRuntimeConfig,
    ) -> Result<Self> {
        if !storage_path.exists() {
            let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
            bail!(
                "retrieval sidecar storage is missing; run retrieval index for project {project_id}"
            );
        }

        let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
        let generation_lease = GenerationRetentionLease::acquire_for_query(runtime, &project_id)?;
        let storage = Store::open_read_only(storage_path).context("open storage for query")?;
        storage
            .get_connection()
            .execute_batch("BEGIN DEFERRED TRANSACTION")
            .context("pin core publication for retrieval query")?;

        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?
            .with_context(|| {
                format!(
                    "retrieval sidecar manifest is missing; run retrieval index for project {project_id}"
                )
            })?;
        if let Some(reason) =
            manifest_unavailable_reason_for_runtime(&project_id, &storage, &manifest, runtime)
        {
            bail!(
                "retrieval sidecar manifest is unavailable ({reason}); run retrieval index for project {project_id}"
            );
        }

        // Acquire residency before strict readiness and keep it through candidate resolution.
        let embedding_residency = acquire_product_embedding_residency_for_runtime(runtime)
            .context("pin retrieval embedding engine")?;
        if let Err(error) =
            validate_strict_sidecar_readiness_for_runtime(project_root, &storage, runtime)
        {
            bail!(
                "retrieval sidecar manifest is unavailable ({error}); run retrieval index for project {project_id}"
            );
        }
        let embedding_device = embedding_device_readiness_for_runtime(runtime);
        let file_roles = storage
            .get_files()
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| (file.path.to_string_lossy().to_string(), file.file_role))
                    .collect()
            })
            .unwrap_or_default();
        let publication_identity =
            retrieval_publication_identity_from_storage(&storage, &project_id)?;
        let core_publication = storage
            .get_complete_index_publication()
            .context("load pinned core publication for vector evidence")?
            .context("pinned retrieval query requires a complete core publication")?;
        crate::embedded_vector::validate_generation_evidence_for_publication(
            &runtime.layout,
            &manifest,
            &core_publication,
            embedding_residency.identity(),
        )
        .context("validate attested vector generation")?;
        let sidecars = Arc::new(LiveSidecarSearch::new_for_runtime_with_embedding_device(
            runtime,
            runtime.layout.clone(),
            project_id.clone(),
            Some(&manifest),
            Some(embedding_device.clone()),
        )?);

        Ok(Self {
            storage,
            storage_path: storage_path.to_path_buf(),
            project_root: project_root.to_path_buf(),
            project_id,
            runtime: runtime.clone(),
            manifest,
            file_roles: Arc::new(file_roles),
            embedding_device,
            publication_identity,
            sidecars,
            _generation_lease: generation_lease,
            _embedding_residency: embedding_residency,
            transaction_active: true,
        })
    }

    pub fn storage(&self) -> &Store {
        &self.storage
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn manifest(&self) -> &RetrievalIndexManifest {
        &self.manifest
    }

    pub fn publication_identity(&self) -> &RetrievalPublicationIdentity {
        &self.publication_identity
    }

    pub fn execute_with_cache(
        &self,
        query: &str,
        budget_ms: Option<u64>,
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<QueryResult> {
        let cancelled = cancelled.unwrap_or_else(cancellation_flag);
        if cancelled.load(Ordering::Acquire) {
            bail!("retrieval query cancelled before preflight");
        }
        cache.scope_to_publication(&self.publication_identity);
        let mut executor = QueryExecutor {
            sidecars: Arc::clone(&self.sidecars),
            cache,
            manifest: Some(self.manifest.clone()),
            file_roles: Arc::clone(&self.file_roles),
            cancelled,
            mode_override: None,
        };
        executor
            .execute(query, budget_ms)
            .map(|result| result.with_publication_identity(&self.publication_identity))
    }

    pub fn execute_batch_with_cache(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<Vec<QueryResult>> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let cancelled = cancelled.unwrap_or_else(cancellation_flag);
        if cancelled.load(Ordering::Acquire) {
            bail!("retrieval query batch cancelled before preflight");
        }
        cache.scope_to_publication(&self.publication_identity);
        let (mode, degraded_reason) = resolve_batch_mode(
            self.sidecars.as_ref(),
            Some(&self.manifest),
            &self.embedding_device,
            &self.runtime,
        );
        if mode != RetrievalDegradedMode::Full {
            bail!(
                "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason={})",
                mode.as_str(),
                degraded_reason.as_deref().unwrap_or("unknown")
            );
        }
        let mut results = execute_strict_retrieval_query_batch_against_sidecars(
            Arc::clone(&self.sidecars),
            Some(self.manifest.clone()),
            Arc::clone(&self.file_roles),
            cancelled,
            mode,
            queries,
            cache,
            strict_batch_worker_limit(queries.len()),
        )?;
        for result in &mut results {
            result.publication_identity = Some(self.publication_identity.clone());
        }
        Ok(results)
    }

    pub fn ensure_result_identity(
        &self,
        result: &QueryResult,
        operation: impl Into<String>,
    ) -> Result<()> {
        if result.publication_identity.as_ref() != Some(&self.publication_identity) {
            return Err(RetrievalPublicationChanged::changed(
                operation,
                &self.publication_identity,
                result.publication_identity.clone(),
            )
            .into());
        }
        Ok(())
    }

    /// Compare against a fresh publication after all candidate resolution and response assembly.
    pub fn revalidate(&self) -> Result<()> {
        let current = Store::open_read_only(&self.storage_path)
            .context("open current retrieval publication")
            .and_then(|storage| {
                let snapshot = storage
                    .read_snapshot()
                    .context("pin current retrieval publication")?;
                let identity = retrieval_publication_identity_from_storage(
                    snapshot.storage(),
                    &self.project_id,
                );
                snapshot
                    .finish()
                    .context("finish current retrieval publication")?;
                identity
            });
        let current = current.map_err(|error| {
            RetrievalPublicationChanged::unreadable(
                "revalidating the query session",
                &self.publication_identity,
                error,
            )
        })?;
        if current != self.publication_identity {
            return Err(RetrievalPublicationChanged::changed(
                "revalidating the query session",
                &self.publication_identity,
                Some(current),
            )
            .into());
        }
        Ok(())
    }
}

impl Drop for PinnedQuerySession {
    fn drop(&mut self) {
        if self.transaction_active {
            let _ = self.storage.get_connection().execute_batch("ROLLBACK");
            self.transaction_active = false;
        }
    }
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
    let session = PinnedQuerySession::begin(request.project_root, request.storage_path, runtime)?;
    let result =
        session.execute_with_cache(request.query, request.budget_ms, request.cancelled, cache)?;
    session.revalidate()?;
    Ok(result)
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
    let session = PinnedQuerySession::begin(request.project_root, request.storage_path, runtime)?;
    let results = session.execute_batch_with_cache(request.queries, request.cancelled, cache)?;
    session.revalidate()?;
    Ok(results)
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
    let key = cache.key_for_manifest(manifest, query_fingerprint(&features.raw_query));
    let hits = cache.get(&key)?.to_vec();
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    Some(QueryResult {
        publication_identity: None,
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
        let key = cache.key_for_manifest(manifest, query_fingerprint(&result.features.raw_query));
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

pub fn retrieval_publication_identity_from_storage(
    storage: &Store,
    project_id: &str,
) -> Result<RetrievalPublicationIdentity> {
    let publication = storage
        .get_complete_index_publication()
        .context("load complete core publication")?
        .context("complete core publication is missing")?;
    let manifest = storage
        .get_retrieval_index_manifest(project_id)
        .context("load retrieval manifest identity")?
        .context("retrieval manifest is missing")?;
    Ok(RetrievalPublicationIdentity {
        core_generation_id: publication.generation_id,
        core_run_id: publication.run_id,
        sidecar_generation: manifest
            .sidecar_generation
            .filter(|value| !value.trim().is_empty())
            .context("retrieval manifest sidecar generation is missing")?,
        sidecar_input_hash: manifest
            .sidecar_input_hash
            .filter(|value| !value.trim().is_empty())
            .context("retrieval manifest input hash is missing")?,
        semantic_generation: (!manifest.semantic_generation.trim().is_empty())
            .then_some(manifest.semantic_generation)
            .context("retrieval manifest semantic generation is missing")?,
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
        return derive_degraded_mode(&report.lexical, &report.semantic, &report.scip);
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

    fn publication_identity(label: &str) -> RetrievalPublicationIdentity {
        RetrievalPublicationIdentity {
            core_generation_id: format!("core-{label}"),
            core_run_id: format!("run-{label}"),
            sidecar_generation: format!("sidecar-{label}"),
            sidecar_input_hash: format!("hash-{label}"),
            semantic_generation: format!("semantic-{label}"),
        }
    }

    #[test]
    fn publication_change_is_typed_for_one_complete_session_retry() {
        let expected = publication_identity("old");
        let observed = publication_identity("new");
        let error = anyhow::Error::from(RetrievalPublicationChanged::changed(
            "resolving candidates",
            &expected,
            Some(observed.clone()),
        ))
        .context("assemble packet response");

        assert!(is_retrieval_publication_changed(&error));
        let changed = error
            .downcast_ref::<RetrievalPublicationChanged>()
            .expect("typed publication change");
        assert_eq!(changed.code(), RETRIEVAL_PUBLICATION_CHANGED_CODE);
        assert_eq!(changed.operation(), "resolving candidates");
        assert_eq!(changed.expected(), &expected);
        assert_eq!(changed.observed(), Some(&observed));
        assert_eq!(changed.detail(), None);
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

            fn semantic_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
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
    #[ignore = "requires a live embedding runtime; run explicitly with cargo test -p codestory-retrieval integration_query_against_fixture_manifest -- --ignored --nocapture"]
    fn integration_query_against_fixture_manifest() {
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
                    embedding_profile: Some("coderank-embed".to_string()),
                    embedding_model: "legacy-producer".to_string(),
                    embedding_backend: Some("legacy".to_string()),
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
                    semantic_generation: "codestory_legacy".into(),
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

        assert!(error.to_string().contains("retrieval_manifest_stale"));
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
        assert!(
            changed_error
                .to_string()
                .contains("retrieval_manifest_stale")
        );

        std::fs::remove_file(&source_path).expect("remove source");
        let removed_error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "indexed",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("removed indexed file must fail closed");
        assert!(
            removed_error
                .to_string()
                .contains("retrieval_manifest_stale")
        );
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

        assert!(error.to_string().contains("retrieval_manifest_stale"));
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
