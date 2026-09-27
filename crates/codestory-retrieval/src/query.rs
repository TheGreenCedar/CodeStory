use crate::cache::RetrievalCache;
#[cfg(test)]
use crate::cache::RetrievalCacheKey;
use crate::candidate::CandidateHit;
use crate::config::SidecarRuntimeConfig;
use crate::embeddings::{
    EmbeddingDeviceReadiness, ProductEmbeddingResidencyLease,
    acquire_product_embedding_residency_for_runtime, embedding_device_readiness_for_runtime,
};
use crate::executor::{
    CandidatePayloadMode, QueryExecutor, QueryResult, RetrievalPublicationIdentity,
    cancellation_flag,
};
use crate::generation::manifest_unavailable_reason_for_runtime;
use crate::health::{
    probe_descriptor_sidecar_health_for_runtime, probe_sidecar_health_for_runtime,
};
use crate::index::{query_fingerprint, sidecar_project_id_for_runtime};
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode, derive_descriptor_mode};
use crate::planner::RetrievalStageKind;
use crate::query_features::{QueryLookupMode, classify_query};
use crate::ranker::rank_candidates;
use crate::retention::GenerationRetentionLease;
use crate::sidecar::validate_strict_sidecar_readiness_for_runtime;
use crate::sidecar_search::{LiveSidecarSearch, SearchExecutionContext, SidecarSearch};
use anyhow::{Context, Result, bail};
use codestory_contracts::graph::NodeId;
use codestory_store::{
    BoundRetrievalIndexManifest, CorePublicationLayout, FileRole, RetrievalIndexManifest, Store,
    core_database_exists, resolve_core_generation_database_path,
};
use parking_lot::{Mutex, MutexGuard};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// One packet can issue many subqueries while other agents share the same per-user engine. Keep
// local fan-out below the engine's global capacity so concurrent packets do not turn queue time
// into stage-deadline losses.
const STRICT_BATCH_WORKER_CAP: usize = 2;
const STRICT_BATCH_PREFETCH_MAX_MS: u64 = 100;
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

/// Numeric wall-time observation for one successful packet descriptor batch.
///
/// The observation deliberately excludes query text, candidates, paths, and
/// health details. An empty or failed batch does not produce one.
///
/// `lexical_wall_ms` / `dense_semantic_wall_ms` are the maximum per-query stage
/// elapsed times from the descriptor plan (Stage1 lexical / Stage1b semantic).
/// They attribute cost inside `query_batch_wall_ms` and are not required to
/// partition that enclosing wall.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketDescriptorBatchObservation {
    pub query_count: u64,
    pub health_resolution_wall_ms: u64,
    pub query_batch_wall_ms: u64,
    pub lexical_wall_ms: u64,
    pub dense_semantic_wall_ms: u64,
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
    storage: Arc<Mutex<Store>>,
    storage_path: PathBuf,
    core_database_path: PathBuf,
    project_root: PathBuf,
    project_id: String,
    runtime: SidecarRuntimeConfig,
    manifest: RetrievalIndexManifest,
    file_roles: RefCell<Option<Arc<HashMap<String, FileRole>>>>,
    embedding_device: EmbeddingDeviceReadiness,
    publication_identity: RetrievalPublicationIdentity,
    sidecars: Arc<dyn SidecarSearch>,
    _generation_lease: GenerationRetentionLease,
    _embedding_residency: ProductEmbeddingResidencyLease,
    full_readiness_validated: Cell<bool>,
    transaction_active: bool,
}

impl PinnedQuerySession {
    pub fn begin(
        project_root: &Path,
        storage_path: &Path,
        runtime: &SidecarRuntimeConfig,
    ) -> Result<Self> {
        Self::begin_with_scope(project_root, storage_path, runtime, true)
    }

    /// Pin the exact core/retrieval publication needed to query descriptor
    /// sidecars without loading repository file records, nodes, source, graph
    /// neighborhoods, or dense-anchor rows. Packet admission must be sealed
    /// before [`Self::validate_full_readiness`] is called.
    pub fn begin_packet_descriptor(
        project_root: &Path,
        storage_path: &Path,
        runtime: &SidecarRuntimeConfig,
    ) -> Result<Self> {
        Self::begin_with_scope(project_root, storage_path, runtime, false)
    }

    fn begin_with_scope(
        project_root: &Path,
        storage_path: &Path,
        runtime: &SidecarRuntimeConfig,
        validate_full_readiness: bool,
    ) -> Result<Self> {
        if !core_database_exists(storage_path).context("resolve core publication for query")? {
            let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
            bail!(
                "retrieval sidecar storage is missing; run retrieval index for project {project_id}"
            );
        }

        let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
        let generation_lease = GenerationRetentionLease::acquire_for_query(runtime, &project_id)?;
        let publication_storage =
            Store::open_read_only(storage_path).context("open retrieval publication pointer")?;
        let bound_manifest = publication_storage
            .get_bound_retrieval_index_manifest(&project_id)
            .context("load core-bound retrieval manifest")?
            .with_context(|| {
                format!(
                    "retrieval sidecar manifest is missing; run retrieval index for project {project_id}"
                )
            })?;
        let core_layout = CorePublicationLayout::from_storage_path(storage_path)
            .context("resolve retrieval query core layout")?;
        let has_immutable_publication = core_layout
            .read_pointer()
            .context("read retrieval query core publication pointer")?
            .is_some();
        let core_path =
            resolve_core_generation_database_path(storage_path, &bound_manifest.core.generation_id)
                .context("resolve retrieval publication core generation")?;
        drop(publication_storage);
        let storage = if has_immutable_publication {
            Store::open_immutable_generation(&core_path)
                .context("open exact immutable core generation for retrieval query")?
        } else {
            // Legacy stores have no immutable generation directory. They must
            // use a normal read-only SQLite snapshot; `immutable=1` would
            // ignore concurrent WAL changes and can surface a newer row under
            // an older retrieval identity.
            Store::open_read_only(&core_path)
                .context("open legacy core snapshot for retrieval query")?
        };
        storage
            .get_connection()
            .execute_batch("BEGIN DEFERRED TRANSACTION")
            .context("pin core publication for retrieval query")?;

        let manifest = bound_manifest.manifest.clone();
        if let Some(reason) =
            manifest_unavailable_reason_for_runtime(&project_id, &storage, &manifest, runtime)
        {
            bail!(
                "retrieval sidecar manifest is unavailable ({reason}); run retrieval index for project {project_id}"
            );
        }
        let core_publication = storage
            .get_complete_index_publication()
            .context("load pinned core publication for retrieval query")?
            .context("pinned retrieval query requires a complete core publication")?;
        if core_publication.generation_id != bound_manifest.core.generation_id
            || core_publication.run_id != bound_manifest.core.run_id
        {
            bail!("retrieval publication core binding does not match immutable core contents");
        }

        // Acquire residency before strict readiness and keep it through candidate resolution.
        let embedding_residency = acquire_product_embedding_residency_for_runtime(runtime)
            .context("pin retrieval embedding engine")?;
        let embedding_device = embedding_device_readiness_for_runtime(runtime);
        let producer_compatibility_identity =
            crate::embedded_vector::vector_producer_compatibility_identity(
                &embedding_device,
                embedding_residency.identity(),
                u32::try_from(crate::embeddings::semantic_vector_dim())
                    .context("embedding dimension exceeds evidence contract")?,
            )?;
        let publication_identity = retrieval_publication_identity_from_bound(&bound_manifest)?;
        // Share this exact read transaction with full-payload stage enrichment.
        // In particular, reopening a legacy path would observe newer WAL rows.
        let storage = Arc::new(Mutex::new(storage));
        let sidecars = Arc::new(
            LiveSidecarSearch::new_for_runtime_with_embedding_device(
                runtime,
                runtime.layout.clone(),
                project_id.clone(),
                Some(&manifest),
                Some(embedding_device.clone()),
            )?
            .with_core_candidate_context(project_root, Arc::clone(&storage)),
        );

        let session = Self {
            storage,
            storage_path: storage_path.to_path_buf(),
            core_database_path: core_path,
            project_root: project_root.to_path_buf(),
            project_id,
            runtime: runtime.clone(),
            manifest,
            file_roles: RefCell::new(None),
            embedding_device,
            publication_identity,
            sidecars,
            _generation_lease: generation_lease,
            _embedding_residency: embedding_residency,
            full_readiness_validated: Cell::new(false),
            transaction_active: true,
        };
        if validate_full_readiness {
            session
                .validate_full_readiness_with_identity(&producer_compatibility_identity, None)?;
        }
        Ok(session)
    }

    /// Complete the repository/core consistency checks after packet-wide
    /// descriptor admission has been sealed and before any admitted identity
    /// is hydrated. Ordinary query sessions perform this during `begin`.
    pub fn validate_full_readiness(&self) -> Result<()> {
        let producer_compatibility_identity =
            crate::embedded_vector::vector_producer_compatibility_identity(
                &self.embedding_device,
                self._embedding_residency.identity(),
                u32::try_from(crate::embeddings::semantic_vector_dim())
                    .context("embedding dimension exceeds evidence contract")?,
            )?;
        self.validate_full_readiness_with_identity(&producer_compatibility_identity, None)
    }

    /// Compatibility seam for carrying the retrieval request's existing
    /// deadline and cancellation state through deferred packet readiness.
    fn validate_full_readiness_with_context(&self, context: &SearchExecutionContext) -> Result<()> {
        context
            .clone()
            .with_stage_boundary("deferred_full_readiness_entry")
            .check_cancelled()?;
        let producer_compatibility_identity =
            crate::embedded_vector::vector_producer_compatibility_identity(
                &self.embedding_device,
                self._embedding_residency.identity(),
                u32::try_from(crate::embeddings::semantic_vector_dim())
                    .context("embedding dimension exceeds evidence contract")?,
            )?;
        context
            .clone()
            .with_stage_boundary("deferred_full_readiness_after_producer_identity")
            .check_cancelled()?;
        self.validate_full_readiness_with_identity(&producer_compatibility_identity, Some(context))
    }

    /// Validate deferred packet readiness under the descriptor phase's
    /// existing absolute deadline and request cancellation flag.
    pub fn validate_full_readiness_with_control(
        &self,
        deadline: Instant,
        request_cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let context = SearchExecutionContext::new(
            deadline,
            request_cancelled,
            Arc::new(AtomicBool::new(false)),
        );
        self.validate_full_readiness_with_context(&context)
    }

    fn validate_full_readiness_with_identity(
        &self,
        producer_compatibility_identity: &str,
        context: Option<&SearchExecutionContext>,
    ) -> Result<()> {
        if let Some(context) = context {
            context
                .clone()
                .with_stage_boundary(
                    "deferred_full_readiness_before_cached_success_or_strict_validation",
                )
                .check_cancelled()?;
        }
        if self.full_readiness_validated.get() {
            return Ok(());
        }
        let storage = self.storage.lock();
        if let Err(error) = validate_strict_sidecar_readiness_for_runtime(
            &self.project_root,
            &self.storage_path,
            &storage,
            &self.runtime,
            producer_compatibility_identity,
        ) {
            bail!(
                "retrieval sidecar manifest is unavailable ({error}); run retrieval index for project {}",
                self.project_id
            );
        }
        if let Some(context) = context {
            context
                .clone()
                .with_stage_boundary("deferred_full_readiness_after_strict_validation")
                .check_cancelled()?;
        }
        let core_publication = storage
            .get_complete_index_publication()
            .context("load pinned core publication for vector validation")?
            .context("pinned retrieval query requires a complete core publication")?;
        if let Some(context) = context {
            context
                .clone()
                .with_stage_boundary("deferred_full_readiness_after_core_publication")
                .check_cancelled()?;
        }
        crate::embedded_vector::validate_generation_evidence_for_publication(
            &self.runtime.layout,
            &storage,
            Some(&self.core_database_path),
            &self.manifest,
            &core_publication,
            &self.runtime,
            &self.embedding_device,
            self._embedding_residency.identity(),
        )
        .context("validate attested vector generation")?;
        if let Some(context) = context {
            context
                .clone()
                .with_stage_boundary("deferred_full_readiness_after_vector_validation")
                .check_cancelled()?;
        }
        self.full_readiness_validated.set(true);
        Ok(())
    }

    /// Borrow the pinned core for one read operation. Drop the guard before
    /// executing sidecar queries, whose enrichment borrows the same snapshot.
    pub fn storage(&self) -> MutexGuard<'_, Store> {
        self.storage.lock()
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
            file_roles: self.file_roles()?,
            cancelled,
            mode_override: None,
        };
        let mut result = executor.execute(query, budget_ms)?;
        self.enrich_and_rerank_candidates(&mut result)?;
        refresh_cached_query_result(cache, &self.manifest, &result);
        Ok(result.with_publication_identity(&self.publication_identity))
    }

    /// Execute the sidecars and return only pre-hydration packet descriptors.
    ///
    /// This path deliberately skips core enrichment. Packet admission must
    /// choose stable identities before any core node, file, source, or graph
    /// record is opened. The ordinary search path above keeps its richer
    /// post-query enrichment.
    pub fn execute_packet_descriptors_with_cache(
        &self,
        query: &str,
        budget_ms: Option<u64>,
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<QueryResult> {
        self.execute_packet_descriptors_with_cache_policy(query, budget_ms, cancelled, cache, true)
    }

    #[cfg(feature = "benchmark-support")]
    pub fn execute_packet_descriptors_without_dense_semantic_for_benchmark_with_cache(
        &self,
        query: &str,
        budget_ms: Option<u64>,
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<QueryResult> {
        self.execute_packet_descriptors_with_cache_policy(query, budget_ms, cancelled, cache, false)
    }

    fn execute_packet_descriptors_with_cache_policy(
        &self,
        query: &str,
        budget_ms: Option<u64>,
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
        include_dense_semantic: bool,
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
            file_roles: Arc::new(HashMap::new()),
            cancelled,
            mode_override: None,
        };
        let mut result = if include_dense_semantic {
            executor.execute_packet_descriptors(query, budget_ms)?
        } else {
            #[cfg(feature = "benchmark-support")]
            {
                executor.execute_packet_descriptors_without_dense_semantic_for_benchmark(
                    query, budget_ms,
                )?
            }
            #[cfg(not(feature = "benchmark-support"))]
            unreachable!("dense semantic packet control requires benchmark-support")
        };
        sanitize_packet_candidate_descriptors(&mut result.hits);
        Ok(result.with_publication_identity(&self.publication_identity))
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
            self.file_roles()?,
            cancelled,
            mode,
            queries,
            cache,
            strict_batch_worker_limit(queries.len()),
        )?;
        for result in &mut results {
            self.enrich_and_rerank_candidates(result)?;
            refresh_cached_query_result(cache, &self.manifest, result);
            result.publication_identity = Some(self.publication_identity.clone());
        }
        Ok(results)
    }

    pub fn execute_packet_descriptor_batch_with_cache(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<Vec<QueryResult>> {
        self.execute_packet_descriptor_batch_with_cache_policy(
            queries, cancelled, cache, true, false,
        )
        .map(|(results, _)| results)
    }

    #[doc(hidden)]
    pub fn execute_packet_descriptor_batch_with_observation_and_cache(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<(Vec<QueryResult>, Option<PacketDescriptorBatchObservation>)> {
        self.execute_packet_descriptor_batch_with_cache_policy(
            queries, cancelled, cache, true, true,
        )
    }

    #[cfg(feature = "benchmark-support")]
    pub fn execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_cache(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<Vec<QueryResult>> {
        self.execute_packet_descriptor_batch_with_cache_policy(
            queries, cancelled, cache, false, false,
        )
        .map(|(results, _)| results)
    }

    #[cfg(feature = "benchmark-support")]
    #[doc(hidden)]
    pub fn execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_observation_and_cache(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
    ) -> Result<(Vec<QueryResult>, Option<PacketDescriptorBatchObservation>)> {
        self.execute_packet_descriptor_batch_with_cache_policy(
            queries, cancelled, cache, false, true,
        )
    }

    fn execute_packet_descriptor_batch_with_cache_policy(
        &self,
        queries: &[QueryBatchItem<'_>],
        cancelled: Option<Arc<AtomicBool>>,
        cache: &mut RetrievalCache,
        include_dense_semantic: bool,
        observe_wall_intervals: bool,
    ) -> Result<(Vec<QueryResult>, Option<PacketDescriptorBatchObservation>)> {
        if queries.is_empty() {
            return Ok((Vec::new(), None));
        }
        let cancelled = cancelled.unwrap_or_else(cancellation_flag);
        if cancelled.load(Ordering::Acquire) {
            bail!("retrieval query batch cancelled before preflight");
        }
        cache.scope_to_publication(&self.publication_identity);
        let health_started_at = observe_wall_intervals.then(Instant::now);
        let (mode, degraded_reason) = resolve_descriptor_batch_mode(
            self.sidecars.as_ref(),
            Some(&self.manifest),
            &self.embedding_device,
            &self.runtime,
        );
        let health_resolution_wall_ms =
            health_started_at.map(|started_at| duration_millis_ceil(started_at.elapsed()));
        if mode != RetrievalDegradedMode::Full {
            bail!(
                "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason={})",
                mode.as_str(),
                degraded_reason.as_deref().unwrap_or("unknown")
            );
        }
        let query_batch_started_at = observe_wall_intervals.then(Instant::now);
        let mut results = execute_strict_retrieval_descriptor_batch_against_sidecars(
            Arc::clone(&self.sidecars),
            Some(self.manifest.clone()),
            Arc::new(HashMap::new()),
            cancelled,
            mode,
            queries,
            cache,
            strict_batch_worker_limit(queries.len()),
            include_dense_semantic,
        )?;
        let query_batch_wall_ms =
            query_batch_started_at.map(|started_at| duration_millis_ceil(started_at.elapsed()));
        let lexical_wall_ms = observe_wall_intervals
            .then(|| max_descriptor_stage_elapsed_ms(&results, RetrievalStageKind::Stage1Lexical));
        let dense_semantic_wall_ms = observe_wall_intervals.then(|| {
            max_descriptor_stage_elapsed_ms(&results, RetrievalStageKind::Stage1bSemantic)
        });
        for result in &mut results {
            sanitize_packet_candidate_descriptors(&mut result.hits);
            result.publication_identity = Some(self.publication_identity.clone());
        }
        let observation = health_resolution_wall_ms.zip(query_batch_wall_ms).map(
            |(health_resolution_wall_ms, query_batch_wall_ms)| PacketDescriptorBatchObservation {
                query_count: u64::try_from(queries.len()).unwrap_or(u64::MAX),
                health_resolution_wall_ms,
                query_batch_wall_ms,
                lexical_wall_ms: lexical_wall_ms.unwrap_or(0),
                dense_semantic_wall_ms: dense_semantic_wall_ms.unwrap_or(0),
            },
        );
        Ok((results, observation))
    }

    fn enrich_and_rerank_candidates(&self, result: &mut QueryResult) -> Result<()> {
        enrich_candidates_from_core(&self.storage.lock(), &self.project_root, &mut result.hits)?;
        result.hits = rank_candidates(&result.features, std::mem::take(&mut result.hits));
        Ok(())
    }

    /// Load repository-wide file roles only for the ordinary enriched search
    /// path. Packet descriptor queries must reach their global admission gate
    /// without opening any candidate file records.
    fn file_roles(&self) -> Result<Arc<HashMap<String, FileRole>>> {
        if let Some(file_roles) = self.file_roles.borrow().as_ref() {
            return Ok(Arc::clone(file_roles));
        }
        let file_roles = Arc::new(
            self.storage
                .lock()
                .get_files()
                .context("load file roles for enriched retrieval query")?
                .into_iter()
                .map(|file| (file.path.to_string_lossy().to_string(), file.file_role))
                .collect(),
        );
        self.file_roles.replace(Some(Arc::clone(&file_roles)));
        Ok(file_roles)
    }

    #[cfg(all(test, feature = "test-support"))]
    fn file_roles_loaded(&self) -> bool {
        self.file_roles.borrow().is_some()
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
                retrieval_publication_identity_from_storage(&storage, &self.project_id)
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

fn sanitize_packet_candidate_descriptors(candidates: &mut [CandidateHit]) {
    for candidate in candidates {
        candidate.source_excerpt = None;
        candidate.structural_kind = None;
        candidate.rank_features = None;
    }
}

pub(crate) fn enrich_candidates_from_core(
    storage: &Store,
    project_root: &Path,
    candidates: &mut [CandidateHit],
) -> Result<()> {
    let node_ids = candidates
        .iter()
        .filter_map(|candidate| candidate.node_id.as_deref())
        .filter_map(|node_id| node_id.parse::<i64>().ok())
        .map(NodeId)
        .collect::<Vec<_>>();
    let nodes = storage
        .get_nodes_by_ids(&node_ids)
        .context("load structural kinds for retrieval candidates")?;
    for candidate in candidates {
        let Some(node_id) = candidate
            .node_id
            .as_deref()
            .and_then(|node_id| node_id.parse::<i64>().ok())
            .map(NodeId)
        else {
            continue;
        };
        let Some(node) = nodes.get(&node_id) else {
            continue;
        };
        candidate.structural_kind = Some(node.kind);
        candidate.qualified_name = node.qualified_name.clone();
        candidate.start_line = candidate.start_line.or(node.start_line);
        if candidate
            .qualified_name
            .as_deref()
            .is_some_and(qualified_name_is_test_scope)
            || (requires_enclosing_test_scope(node.kind)
                && candidate.start_line.is_some_and(|line| {
                    let path = project_root.join(&candidate.file_path);
                    storage
                        .get_nodes_for_file_line(&path.to_string_lossy(), line)
                        .ok()
                        .is_some_and(|nodes| {
                            nodes.iter().any(|node| {
                                node.qualified_name
                                    .as_deref()
                                    .is_some_and(qualified_name_is_test_scope)
                            })
                        })
                }))
        {
            candidate.file_role = Some(FileRole::Test);
        }
    }
    Ok(())
}

fn qualified_name_is_test_scope(name: &str) -> bool {
    name.starts_with("tests::")
        || name.contains("::tests::")
        || name.starts_with("test::")
        || name.contains("::test::")
}

fn requires_enclosing_test_scope(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::MACRO
            | codestory_contracts::graph::NodeKind::ANNOTATION
    )
}

fn refresh_cached_query_result(
    cache: &mut RetrievalCache,
    manifest: &RetrievalIndexManifest,
    result: &QueryResult,
) {
    if result.trace.cancel_reason.is_some() {
        return;
    }
    let key = cache.key_for_manifest(manifest, query_fingerprint(&result.features.raw_query));
    if cache.get(&key).is_some() {
        cache.insert(key, result.hits.clone());
    }
}

impl Drop for PinnedQuerySession {
    fn drop(&mut self) {
        if self.transaction_active {
            let _ = self
                .storage
                .lock()
                .get_connection()
                .execute_batch("ROLLBACK");
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
    execute_strict_retrieval_query_batch_against_sidecars_with_payload(
        sidecars,
        manifest,
        file_roles,
        cancelled,
        mode,
        queries,
        cache,
        worker_limit,
        CandidatePayloadMode::Full,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_strict_retrieval_descriptor_batch_against_sidecars(
    sidecars: Arc<dyn SidecarSearch>,
    manifest: Option<RetrievalIndexManifest>,
    file_roles: Arc<HashMap<String, FileRole>>,
    cancelled: Arc<AtomicBool>,
    mode: RetrievalDegradedMode,
    queries: &[QueryBatchItem<'_>],
    cache: &mut RetrievalCache,
    worker_limit: usize,
    include_dense_semantic: bool,
) -> Result<Vec<QueryResult>> {
    execute_strict_retrieval_query_batch_against_sidecars_with_payload(
        sidecars,
        manifest,
        file_roles,
        cancelled,
        mode,
        queries,
        cache,
        worker_limit,
        CandidatePayloadMode::DescriptorOnly,
        include_dense_semantic,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_strict_retrieval_query_batch_against_sidecars_with_payload(
    sidecars: Arc<dyn SidecarSearch>,
    manifest: Option<RetrievalIndexManifest>,
    file_roles: Arc<HashMap<String, FileRole>>,
    cancelled: Arc<AtomicBool>,
    mode: RetrievalDegradedMode,
    queries: &[QueryBatchItem<'_>],
    cache: &mut RetrievalCache,
    worker_limit: usize,
    payload: CandidatePayloadMode,
    include_dense_semantic: bool,
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
        if payload == CandidatePayloadMode::Full
            && let Some(result) =
                cached_batch_result(manifest.as_ref(), cache, query.query, mode, &cancelled)
        {
            results[index] = Some(result);
        } else {
            misses.push((index, query.query.to_string(), query.budget_ms));
        }
    }

    let (sidecars, prefetch_elapsed) = if payload == CandidatePayloadMode::Full {
        prepare_batched_sidecars(sidecars, manifest.as_ref(), &misses, &cancelled)
    } else {
        (sidecars, Duration::ZERO)
    };
    let prefetch_elapsed_ms = duration_millis_ceil(prefetch_elapsed);

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
                let total_budget_ms = effective_query_budget_ms(query, *budget_ms);
                let remaining_budget_ms = total_budget_ms.saturating_sub(prefetch_elapsed_ms);
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
                    let result = match payload {
                        CandidatePayloadMode::Full => {
                            executor.execute(query, Some(remaining_budget_ms))
                        }
                        CandidatePayloadMode::DescriptorOnly if include_dense_semantic => executor
                            .execute_packet_descriptors(query, Some(remaining_budget_ms)),
                        CandidatePayloadMode::DescriptorOnly => {
                            #[cfg(feature = "benchmark-support")]
                            {
                                executor
                                    .execute_packet_descriptors_without_dense_semantic_for_benchmark(
                                        query,
                                        Some(remaining_budget_ms),
                                    )
                            }
                            #[cfg(not(feature = "benchmark-support"))]
                            unreachable!("dense semantic packet control requires benchmark-support")
                        }
                    }
                    .map(|mut result| {
                        result.trace.total_budget_ms = total_budget_ms;
                        result.trace.elapsed_ms =
                            result.trace.elapsed_ms.saturating_add(prefetch_elapsed_ms);
                        result
                    });
                    (*index, result)
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
            if payload == CandidatePayloadMode::Full {
                cache_completed_batch_result(manifest.as_ref(), cache, &result, &cancelled);
            }
            results[index] = Some(result);
        }
    }

    results
        .into_iter()
        .map(|result| result.context("strict retrieval batch dropped a query result"))
        .collect()
}

fn prepare_batched_sidecars(
    sidecars: Arc<dyn SidecarSearch>,
    manifest: Option<&RetrievalIndexManifest>,
    misses: &[(usize, String, Option<u64>)],
    cancelled: &Arc<AtomicBool>,
) -> (Arc<dyn SidecarSearch>, Duration) {
    let started = Instant::now();
    let mut seen = HashSet::new();
    let all_queries = misses
        .iter()
        .map(|(_, query, _)| query)
        .filter(|query| seen.insert((*query).clone()))
        .cloned()
        .collect::<Vec<_>>();
    if all_queries.len() < 2 || cancelled.load(Ordering::Acquire) {
        return (sidecars, Duration::ZERO);
    }
    let smallest_budget_ms = misses
        .iter()
        .map(|(_, query, budget_ms)| effective_query_budget_ms(query, *budget_ms))
        .min()
        .unwrap_or_default();
    if smallest_budget_ms == 0 {
        return (sidecars, Duration::ZERO);
    }
    let prefetch_budget_ms = smallest_budget_ms
        .saturating_div(2)
        .clamp(1, STRICT_BATCH_PREFETCH_MAX_MS);
    let prefetch_deadline = Instant::now()
        .checked_add(Duration::from_millis(prefetch_budget_ms))
        .unwrap_or_else(Instant::now);
    let prefetch_cancelled = Arc::new(AtomicBool::new(false));
    let context =
        SearchExecutionContext::new(prefetch_deadline, Arc::clone(cancelled), prefetch_cancelled);
    let lexical_requests = all_queries
        .iter()
        .cloned()
        .map(|query| (query, crate::planner::LEXICAL_FUSION_WINDOW))
        .collect::<Vec<_>>();
    let prepared_lexical = match sidecars.lexical_search_batch(&lexical_requests, &context) {
        Ok(Some(results))
            if results.len() == all_queries.len() && context.check_cancelled().is_ok() =>
        {
            all_queries
                .iter()
                .cloned()
                .zip(results)
                .collect::<HashMap<_, _>>()
        }
        _ => HashMap::new(),
    };

    seen.clear();
    let semantic_queries = misses
        .iter()
        .map(|(_, query, _)| query)
        .filter(|query| {
            let features = classify_query(query);
            !(features.intent.standalone_path
                || (features.intent.lookup_mode == QueryLookupMode::Definition
                    && features.intent.standalone_symbol
                    && !features.intent.relationship))
        })
        .filter(|query| seen.insert((*query).clone()))
        .cloned()
        .collect::<Vec<_>>();
    let dense_anchor_count = manifest
        .and_then(|manifest| {
            manifest
                .dense_projection_count
                .or(manifest.projection_count)
        })
        .unwrap_or(0);
    let prepared_semantic = if dense_anchor_count > 0
        && semantic_queries.len() >= 2
        && context.check_cancelled().is_ok()
    {
        match sidecars.semantic_search_batch(
            &semantic_queries,
            crate::planner::SEMANTIC_CALIBRATION_WINDOW,
            &context,
        ) {
            Ok(Some(results))
                if results.len() == semantic_queries.len() && context.check_cancelled().is_ok() =>
            {
                semantic_queries
                    .into_iter()
                    .zip(results)
                    .collect::<HashMap<_, _>>()
            }
            _ => HashMap::new(),
        }
    } else {
        HashMap::new()
    };
    if prepared_lexical.is_empty() && prepared_semantic.is_empty() {
        return (sidecars, started.elapsed());
    }
    (
        Arc::new(PreparedBatchSidecars {
            inner: sidecars,
            prepared_lexical,
            prepared_semantic,
        }),
        started.elapsed(),
    )
}

fn effective_query_budget_ms(query: &str, budget_ms: Option<u64>) -> u64 {
    budget_ms
        .unwrap_or_else(|| {
            crate::planner::plan_query(&classify_query(query), RetrievalDegradedMode::Full)
                .total_budget_ms
        })
        .min(crate::executor::MAX_RETRIEVAL_BUDGET_MS)
}

fn duration_millis_ceil(duration: Duration) -> u64 {
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    millis.saturating_add(u64::from(
        !duration.subsec_nanos().is_multiple_of(1_000_000),
    ))
}

fn max_descriptor_stage_elapsed_ms(results: &[QueryResult], kind: RetrievalStageKind) -> u64 {
    results
        .iter()
        .flat_map(|result| result.trace.stages.iter())
        .filter(|stage| stage.stage == kind)
        .map(|stage| stage.elapsed_ms)
        .max()
        .unwrap_or(0)
}

struct PreparedBatchSidecars {
    inner: Arc<dyn SidecarSearch>,
    prepared_lexical: HashMap<String, Vec<CandidateHit>>,
    prepared_semantic: HashMap<String, Vec<CandidateHit>>,
}

impl SidecarSearch for PreparedBatchSidecars {
    fn layout(&self) -> Option<&crate::config::SidecarLayout> {
        self.inner.layout()
    }

    fn embedding_device_readiness(&self) -> Option<&EmbeddingDeviceReadiness> {
        self.inner.embedding_device_readiness()
    }

    fn runtime_config(&self) -> Option<&SidecarRuntimeConfig> {
        self.inner.runtime_config()
    }

    fn enrich_candidates(&self, candidates: &mut [CandidateHit]) -> Result<()> {
        self.inner.enrich_candidates(candidates)
    }

    fn lexical_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        if let Some(prepared) = self.prepared_lexical.get(query) {
            let mut hits = prepared.clone();
            hits.truncate(limit);
            return Ok(hits);
        }
        self.inner.lexical_search(query, limit)
    }

    fn lexical_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.check_cancelled()?;
        let hits = if let Some(prepared) = self.prepared_lexical.get(query) {
            let mut hits = prepared.clone();
            hits.truncate(limit);
            hits
        } else {
            self.inner
                .lexical_search_with_context(query, limit, context)?
        };
        context.check_cancelled()?;
        Ok(hits)
    }

    fn semantic_search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        if let Some(prepared) = self.prepared_semantic.get(query) {
            let mut hits = prepared.clone();
            hits.truncate(limit);
            return Ok(hits);
        }
        self.inner.semantic_search(query, limit)
    }

    fn semantic_search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        context.check_cancelled()?;
        let hits = if let Some(prepared) = self.prepared_semantic.get(query) {
            let mut hits = prepared.clone();
            hits.truncate(limit);
            hits
        } else {
            self.inner
                .semantic_search_with_context(query, limit, context)?
        };
        context.check_cancelled()?;
        Ok(hits)
    }

    fn scip_anchor(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        self.inner.scip_anchor(query, limit)
    }

    fn scip_anchor_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        self.inner.scip_anchor_with_context(query, limit, context)
    }

    fn scip_expand(&self, anchors: &[CandidateHit], limit: usize) -> Result<Vec<CandidateHit>> {
        self.inner.scip_expand(anchors, limit)
    }

    fn scip_expand_with_context(
        &self,
        anchors: &[CandidateHit],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        self.inner.scip_expand_with_context(anchors, limit, context)
    }
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
    strict_batch_worker_limit_for_available_parallelism(query_count, available)
}

fn strict_batch_worker_limit_for_available_parallelism(
    query_count: usize,
    available: usize,
) -> usize {
    query_count.min(available).clamp(1, STRICT_BATCH_WORKER_CAP)
}

pub fn retrieval_publication_identity_from_storage(
    storage: &Store,
    project_id: &str,
) -> Result<RetrievalPublicationIdentity> {
    let bound = storage
        .get_bound_retrieval_index_manifest(project_id)
        .context("load core-bound retrieval manifest identity")?
        .context("retrieval manifest is missing")?;
    retrieval_publication_identity_from_bound(&bound)
}

fn retrieval_publication_identity_from_bound(
    bound: &BoundRetrievalIndexManifest,
) -> Result<RetrievalPublicationIdentity> {
    let manifest = &bound.manifest;
    Ok(RetrievalPublicationIdentity {
        core_generation_id: bound.core.generation_id.clone(),
        core_run_id: bound.core.run_id.clone(),
        sidecar_generation: manifest
            .sidecar_generation
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .context("retrieval manifest sidecar generation is missing")?,
        sidecar_input_hash: manifest
            .sidecar_input_hash
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .context("retrieval manifest input hash is missing")?,
        semantic_generation: (!manifest.semantic_generation.trim().is_empty())
            .then(|| manifest.semantic_generation.clone())
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

fn resolve_descriptor_batch_mode(
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
        let report = probe_descriptor_sidecar_health_for_runtime(
            layout,
            &manifest.project_id,
            Some(manifest.clone()),
            embedding_device,
            runtime,
        );
        return derive_descriptor_mode(&report.lexical, &report.semantic);
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tempfile::TempDir;

    #[cfg(feature = "test-support")]
    struct PinnedEnrichmentFixtureSidecars {
        live: Arc<dyn SidecarSearch>,
        enrichments: AtomicUsize,
    }

    #[cfg(feature = "test-support")]
    impl SidecarSearch for PinnedEnrichmentFixtureSidecars {
        fn layout(&self) -> Option<&crate::SidecarLayout> {
            self.live.layout()
        }

        fn embedding_device_readiness(&self) -> Option<&EmbeddingDeviceReadiness> {
            self.live.embedding_device_readiness()
        }

        fn runtime_config(&self) -> Option<&SidecarRuntimeConfig> {
            self.live.runtime_config()
        }

        fn enrich_candidates(&self, candidates: &mut [CandidateHit]) -> Result<()> {
            self.enrichments.fetch_add(1, Ordering::SeqCst);
            self.live.enrich_candidates(candidates)
        }

        fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok((1..=4)
                .map(|id| {
                    let mut hit = CandidateHit::lexical_stub("lib.rs", 1.0);
                    hit.node_id = Some(id.to_string());
                    hit.symbol_name = Some(format!("symbol_{id}"));
                    if id == 4 {
                        hit.file_role = Some(FileRole::Test);
                    }
                    hit
                })
                .collect())
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

    #[cfg(feature = "test-support")]
    fn full_payload_pin_survives_core_change(immutable: bool) {
        use crate::test_support::{
            env_lock, publish_complete_core_fixture, publish_zero_dense_pinned_query_fixture,
        };
        use codestory_contracts::core_publication::CoreGenerationIdentityV1;
        use codestory_store::{
            CorePublishTransaction, IndexPublicationMode, IndexPublicationRecord, SnapshotStore,
        };

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let cache_root = TempDir::new().expect("retrieval cache");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "pub fn symbol_1() {}\npub fn symbol_2() {}\n")
            .expect("write source");
        let storage_path = storage_dir.path().join("codestory.db");
        let publication = |generation, generation_id: &str| IndexPublicationRecord {
            generation,
            generation_id: generation_id.into(),
            run_id: format!("run-{generation}"),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: generation as i64,
        };
        let first = publication(1, "11111111-1111-4111-8111-111111111111");
        let second = publication(2, "22222222-2222-4222-8222-222222222222");
        let seed = |path: &Path, publication: &IndexPublicationRecord, replacement: bool| {
            let mut storage = Store::open(path).expect("open fixture core");
            storage
                .insert_file(&FileInfo {
                    id: 10,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: live_mtime_millis(&source_path),
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: FileRole::Source,
                })
                .expect("insert source file");
            let nodes = (1..=4)
                .map(|id| Node {
                    id: NodeId(id),
                    kind: NodeKind::FUNCTION,
                    serialized_name: format!("symbol_{id}"),
                    qualified_name: Some(if id == 3 || (replacement && id == 1) {
                        format!("tests::symbol_{id}")
                    } else {
                        format!("source::symbol_{id}")
                    }),
                    canonical_id: Some(format!("rust:symbol_{id}")),
                    file_node_id: Some(NodeId(10)),
                    start_line: Some(1),
                    start_col: Some(0),
                    end_line: Some(1),
                    end_col: Some(1),
                })
                .chain(std::iter::once(Node {
                    id: NodeId(10),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().into_owned(),
                    qualified_name: None,
                    canonical_id: None,
                    file_node_id: None,
                    start_line: Some(1),
                    start_col: Some(0),
                    end_line: Some(2),
                    end_col: Some(1),
                }))
                .collect::<Vec<_>>();
            storage
                .insert_nodes_batch(&nodes)
                .expect("insert core nodes");
            publish_complete_core_fixture(&mut storage, project.path(), publication)
                .expect("publish complete fixture core");
        };
        let identity =
            |path: &Path, publication: &IndexPublicationRecord| CoreGenerationIdentityV1 {
                generation_id: publication.generation_id.clone(),
                run_id: publication.run_id.clone(),
                logical_bytes: std::fs::metadata(path).expect("core bytes").len(),
                published_at_epoch_ms: publication.published_at_epoch_ms,
            };
        if immutable {
            let stage = SnapshotStore::staged_path(&storage_path).expect("stage A");
            seed(&stage, &first, false);
            let first_identity = identity(&stage, &first);
            CorePublishTransaction::begin_from_stage(&storage_path, stage)
                .expect("begin A publication")
                .commit_pointer(first_identity, None)
                .expect("activate A");
        } else {
            seed(&storage_path, &first, false);
        }
        let runtime = crate::config::with_test_cache_root(cache_root.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish retrieval binding A");
        let mut session = PinnedQuerySession::begin(project.path(), &storage_path, &runtime)
            .expect("pin complete A");
        let pinned_name = |session: &PinnedQuerySession| {
            session
                .storage()
                .get_connection()
                .query_row("SELECT qualified_name FROM node WHERE id=1", [], |row| {
                    row.get::<_, String>(0)
                })
                .expect("pinned raw SQLite node")
        };
        assert_eq!(pinned_name(&session), "source::symbol_1");
        let fixture = Arc::new(PinnedEnrichmentFixtureSidecars {
            live: Arc::clone(&session.sidecars),
            enrichments: AtomicUsize::new(0),
        });
        session.sidecars = fixture.clone();
        if immutable {
            let stage = SnapshotStore::staged_path(&storage_path).expect("stage B");
            seed(&stage, &second, true);
            let second_identity = identity(&stage, &second);
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let old = layout
                .read_pointer()
                .expect("old pointer")
                .expect("A pointer")
                .active;
            CorePublishTransaction::begin_from_stage(&storage_path, stage)
                .expect("begin B publication")
                .commit_pointer(second_identity, Some(old))
                .expect("activate B");
            assert_eq!(
                layout
                    .read_pointer()
                    .expect("new pointer")
                    .expect("B pointer")
                    .active
                    .generation_id,
                second.generation_id
            );
        } else {
            let writer = Store::open(&storage_path).expect("legacy WAL writer");
            writer.get_connection().execute_batch(
                "PRAGMA wal_autocheckpoint=0; UPDATE node SET qualified_name='tests::symbol_1' WHERE id=1;"
            ).expect("commit new metadata into WAL");
            assert!(
                std::fs::metadata(storage_path.with_extension("db-wal"))
                    .expect("live WAL")
                    .len()
                    > 0
            );
        }
        let current = Store::open_read_only(&storage_path).expect("current B metadata");
        assert_eq!(
            current
                .get_nodes_by_ids(&[NodeId(1)])
                .expect("current node")[&NodeId(1)]
                .qualified_name
                .as_deref(),
            Some("tests::symbol_1")
        );
        session
            .revalidate()
            .expect("retrieval binding remains A despite newer core metadata");
        // Read SQLite directly so the legacy snapshot control is not satisfied
        // by a warmed Store node cache. Live enrichment must use this read view.
        assert_eq!(pinned_name(&session), "source::symbol_1");
        let expected_identity = session.publication_identity().clone();
        let assert_hits = |result: &QueryResult| {
            let ids = result
                .hits
                .iter()
                .filter_map(|hit| hit.node_id.as_deref())
                .collect::<HashSet<_>>();
            assert_eq!(
                ids,
                HashSet::from(["1", "2"]),
                "A source hit must survive pre-fusion metadata filtering"
            );
            assert!(
                result.hits.iter().all(|hit| matches!(
                    hit.file_role,
                    Some(FileRole::Source | FileRole::Entrypoint)
                )),
                "retained A hits must remain primary source candidates: {:?}",
                result.hits
            );
            assert_eq!(
                result
                    .hits
                    .iter()
                    .find(|hit| hit.node_id.as_deref() == Some("1"))
                    .expect("source hit")
                    .qualified_name
                    .as_deref(),
                Some("source::symbol_1")
            );
            assert_eq!(
                result.publication_identity.as_ref(),
                Some(&expected_identity)
            );
        };
        let mut cache = RetrievalCache::new();
        assert_hits(
            &session
                .execute_with_cache("find service implementation", Some(500), None, &mut cache)
                .expect("execute full query against A"),
        );
        let queries = [
            QueryBatchItem {
                query: "locate service implementation",
                budget_ms: Some(500),
            },
            QueryBatchItem {
                query: "explain service implementation",
                budget_ms: Some(500),
            },
        ];
        for result in session
            .execute_batch_with_cache(&queries, None, &mut cache)
            .expect("parallel full queries")
        {
            assert_hits(&result);
        }
        assert!(
            fixture.enrichments.load(Ordering::SeqCst) >= 3,
            "full stages must invoke live enrichment"
        );
        let before_descriptors = fixture.enrichments.load(Ordering::SeqCst);
        session
            .execute_packet_descriptors_with_cache(
                "describe service implementation",
                Some(500),
                None,
                &mut cache,
            )
            .expect("descriptor query");
        assert_eq!(
            fixture.enrichments.load(Ordering::SeqCst),
            before_descriptors,
            "descriptor execution must not hydrate core candidates"
        );
        session
            .revalidate()
            .expect("old retrieval binding remains usable");
        assert!(
            crate::retention::GenerationRetentionLock::try_acquire(
                &runtime.layout.state_file,
                session.project_id(),
            )
            .expect("observe cleanup fence")
            .is_none(),
            "query session retains its cleanup lease"
        );
        let project_id = session.project_id().to_owned();
        drop(session);
        assert!(
            crate::retention::GenerationRetentionLock::try_acquire(
                &runtime.layout.state_file,
                &project_id,
            )
            .expect("cleanup fence after query")
            .is_some(),
            "cleanup can resume after the session ends"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn full_payload_enrichment_uses_pinned_core_after_pointer_swap() {
        full_payload_pin_survives_core_change(true);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn full_payload_enrichment_uses_pinned_legacy_wal_snapshot() {
        full_payload_pin_survives_core_change(false);
    }

    #[test]
    fn strict_batch_fanout_stays_bounded_when_the_host_has_many_cores() {
        assert_eq!(
            strict_batch_worker_limit_for_available_parallelism(20, 20),
            2
        );
        assert_eq!(
            strict_batch_worker_limit_for_available_parallelism(1, 20),
            1
        );
        assert_eq!(
            strict_batch_worker_limit_for_available_parallelism(20, 1),
            1
        );
    }

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

    #[cfg(feature = "test-support")]
    #[test]
    fn strict_session_accepts_fresh_generation_after_core_identity_only_change() {
        use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("retrieval cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let first_core = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let second_core = IndexPublicationRecord {
            generation: 2,
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            run_id: "run-two".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 2,
        };
        let mut store = Store::open(&storage_path).expect("open storage");
        crate::test_support::publish_complete_core_fixture(&mut store, project.path(), &first_core)
            .expect("publish first complete core fixture");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });

        let first_manifest =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish first strict generation");
        let first_session = PinnedQuerySession::begin(project.path(), &storage_path, &runtime)
            .expect("first strict query admission");
        assert!(
            !first_session.file_roles_loaded(),
            "pinning a retrieval publication must not hydrate repository file records"
        );
        assert_eq!(
            first_session.publication_identity().core_generation_id,
            first_core.generation_id
        );
        let live_context = SearchExecutionContext::new(
            Instant::now() + Duration::from_secs(30),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        first_session
            .validate_full_readiness_with_context(&live_context)
            .expect("live context must preserve an already validated session");
        drop(first_session);

        let mut store = Store::open(&storage_path).expect("open identity-only writer");
        crate::test_support::publish_complete_core_fixture(
            &mut store,
            project.path(),
            &second_core,
        )
        .expect("publish second complete core fixture");
        store
            .publish_dense_anchor_generation(
                &second_core,
                crate::generation::SEMANTIC_POLICY_VERSION,
            )
            .expect("rebind unchanged dense anchors");
        drop(store);
        assert!(
            PinnedQuerySession::begin(project.path(), &storage_path, &runtime).is_err(),
            "old publication-bound vectors must fail strict admission"
        );

        let second_manifest =
            publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
                .expect("publish fresh strict generation");
        assert_ne!(
            first_manifest.sidecar_input_hash, second_manifest.sidecar_input_hash,
            "core publication identity must bind the immutable vector generation"
        );
        assert_ne!(
            first_manifest.sidecar_generation,
            second_manifest.sidecar_generation
        );
        let second_session = PinnedQuerySession::begin(project.path(), &storage_path, &runtime)
            .expect("strict query admits rebuilt publication-bound vectors");
        assert_eq!(
            second_session.publication_identity().core_generation_id,
            second_core.generation_id
        );
        assert_eq!(
            second_session.publication_identity().core_run_id,
            second_core.run_id
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn packet_descriptor_pin_defers_file_inventory_until_after_admission() {
        use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "pub fn visible() {}\n").expect("write source");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("retrieval cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let mut store = Store::open(&storage_path).expect("open storage");
        store
            .insert_file(&FileInfo {
                id: 1,
                path: source_path,
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert indexed file");
        crate::test_support::publish_complete_core_fixture(
            &mut store,
            project.path(),
            &publication,
        )
        .expect("publish complete core fixture");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish descriptor fixture");

        let writer = Store::open(&storage_path).expect("open hostile writer");
        writer
            .get_connection()
            .execute("UPDATE file SET language = X'FF' WHERE id = 1", [])
            .expect("poison full file projection");
        drop(writer);

        let session =
            PinnedQuerySession::begin_packet_descriptor(project.path(), &storage_path, &runtime)
                .expect("descriptor pin must not decode repository file rows");
        assert!(!session.file_roles_loaded());
        let error = session
            .validate_full_readiness()
            .expect_err("post-admission full readiness must still fail closed");
        assert!(
            error
                .to_string()
                .contains("retrieval sidecar manifest is unavailable"),
            "unexpected error: {error:#}"
        );
    }

    #[cfg(all(feature = "test-support", feature = "benchmark-support"))]
    #[test]
    fn packet_descriptor_batch_wall_observation_covers_success_empty_and_error() {
        use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "pub fn visible() {}\n").expect("write source");
        let storage_dir = TempDir::new().expect("storage");
        let cache_root = TempDir::new().expect("retrieval cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let mut store = Store::open(&storage_path).expect("open storage");
        store
            .insert_file(&FileInfo {
                id: 1,
                path: source_path,
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert indexed file");
        crate::test_support::publish_complete_core_fixture(
            &mut store,
            project.path(),
            &publication,
        )
        .expect("publish complete core fixture");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache_root.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish descriptor fixture");
        let session =
            PinnedQuerySession::begin_packet_descriptor(project.path(), &storage_path, &runtime)
                .expect("begin descriptor session");
        let mut cache = RetrievalCache::new();

        let (empty_results, empty_observation) = session
            .execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_observation_and_cache(
                &[],
                None,
                &mut cache,
            )
            .expect("empty descriptor batch");
        assert!(empty_results.is_empty());
        assert_eq!(empty_observation, None);

        let queries = [QueryBatchItem {
            query: "visible",
            budget_ms: Some(500),
        }];
        let cancelled = Arc::new(AtomicBool::new(true));
        let error = session
            .execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_observation_and_cache(
                &queries,
                Some(cancelled),
                &mut cache,
            )
            .expect_err("cancelled descriptor batch must preserve its preflight error");
        assert_eq!(
            error.to_string(),
            "retrieval query batch cancelled before preflight"
        );

        let (results, observation) = session
            .execute_packet_descriptor_batch_without_dense_semantic_for_benchmark_with_observation_and_cache(
                &queries,
                None,
                &mut cache,
            )
            .expect("successful observed descriptor batch");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].query, "visible");
        assert_eq!(
            results[0].publication_identity.as_ref(),
            Some(session.publication_identity())
        );
        let observation = observation.expect("successful non-empty batch observation");
        assert_eq!(observation.query_count, 1);
        assert!(
            observation.query_batch_wall_ms >= observation.lexical_wall_ms,
            "lexical stage attribution must stay inside the enclosing query-batch wall"
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn packet_descriptor_full_readiness_refuses_expired_or_cancelled_context_before_validation() {
        use crate::test_support::{env_lock, publish_zero_dense_pinned_query_fixture};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};

        let _env = env_lock();
        let project = TempDir::new().expect("project");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "pub fn visible() {}\n").expect("write source");
        let storage_dir = TempDir::new().expect("storage");
        let cache = TempDir::new().expect("retrieval cache");
        let storage_path = storage_dir.path().join("codestory.db");
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".into(),
            run_id: "run-one".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let mut store = Store::open(&storage_path).expect("open storage");
        store
            .insert_file(&FileInfo {
                id: 1,
                path: source_path,
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert indexed file");
        crate::test_support::publish_complete_core_fixture(
            &mut store,
            project.path(),
            &publication,
        )
        .expect("publish complete core fixture");
        drop(store);
        let runtime = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                crate::SidecarProfile::Local,
            )
        });
        publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
            .expect("publish descriptor fixture");

        let writer = Store::open(&storage_path).expect("open hostile writer");
        writer
            .get_connection()
            .execute("UPDATE file SET language = X'FF' WHERE id = 1", [])
            .expect("poison full file projection");
        drop(writer);

        let session =
            PinnedQuerySession::begin_packet_descriptor(project.path(), &storage_path, &runtime)
                .expect("descriptor pin must not decode repository file rows");
        let expired = SearchExecutionContext::new(
            Instant::now()
                .checked_sub(Duration::from_millis(1))
                .expect("expired deadline"),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let expired_error = session
            .validate_full_readiness_with_context(&expired)
            .expect_err("expired readiness must stop before strict validation");
        assert_eq!(
            expired_error.to_string(),
            "retrieval stopped: reason=deadline stage=deferred_full_readiness_entry",
            "expired readiness reached work after the entry checkpoint"
        );
        assert!(!session.full_readiness_validated.get());

        let cancelled = SearchExecutionContext::new(
            Instant::now() + Duration::from_secs(30),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        );
        let cancelled_error = session
            .validate_full_readiness_with_context(&cancelled)
            .expect_err("cancelled readiness must stop before strict validation");
        assert_eq!(
            cancelled_error.to_string(),
            "retrieval stopped: reason=request_cancelled stage=deferred_full_readiness_entry",
            "cancelled readiness reached work after the entry checkpoint"
        );
        assert!(!session.full_readiness_validated.get());

        let live = SearchExecutionContext::new(
            Instant::now() + Duration::from_secs(30),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let ordinary_error = session
            .validate_full_readiness_with_context(&live)
            .expect_err("live readiness must still inspect the poisoned inventory");
        assert!(
            ordinary_error
                .to_string()
                .contains("retrieval sidecar manifest is unavailable"),
            "ordinary validation did not preserve the strict failure: {ordinary_error:#}"
        );
        assert!(!session.full_readiness_validated.get());
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

    #[cfg(feature = "benchmark-support")]
    #[test]
    fn benchmark_descriptor_batch_executes_no_dense_semantic_stage() {
        struct DenseSemanticTripwire;

        impl SidecarSearch for DenseSemanticTripwire {
            fn lexical_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/{query}.rs"),
                    1.0,
                )])
            }

            fn semantic_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                panic!("dense semantic retrieval must not execute in the descriptor batch control")
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

        let queries = [
            QueryBatchItem {
                query: "alpha flow",
                budget_ms: Some(500),
            },
            QueryBatchItem {
                query: "beta flow",
                budget_ms: Some(500),
            },
        ];
        let results = execute_strict_retrieval_descriptor_batch_against_sidecars(
            Arc::new(DenseSemanticTripwire),
            Some(manifest_for("testproj", "descriptor-control", 2)),
            Arc::new(HashMap::new()),
            cancellation_flag(),
            RetrievalDegradedMode::Full,
            &queries,
            &mut RetrievalCache::new(),
            2,
            false,
        )
        .expect("descriptor batch control");

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| {
            result
                .trace
                .stages
                .iter()
                .all(|stage| stage.stage != crate::planner::RetrievalStageKind::Stage1bSemantic)
        }));
    }

    #[test]
    fn strict_batch_slow_prefetch_falls_back_within_item_budgets() {
        struct SlowPrefetchSidecars;

        impl SidecarSearch for SlowPrefetchSidecars {
            fn lexical_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/{query}.rs"),
                    1.0,
                )])
            }

            fn lexical_search_batch(
                &self,
                _queries: &[(String, usize)],
                context: &SearchExecutionContext,
            ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
                while !context.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                anyhow::bail!("simulated slow lexical prefetch")
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

        let queries = [
            QueryBatchItem {
                query: "alpha flow",
                budget_ms: Some(100),
            },
            QueryBatchItem {
                query: "beta flow",
                budget_ms: Some(100),
            },
        ];
        let results = execute_strict_retrieval_query_batch_against_sidecars(
            Arc::new(SlowPrefetchSidecars),
            Some(manifest_for("testproj", "slow-prefetch", 2)),
            Arc::new(HashMap::new()),
            cancellation_flag(),
            RetrievalDegradedMode::Full,
            &queries,
            &mut RetrievalCache::new(),
            2,
        )
        .expect("slow prefetch falls back");

        assert!(results.iter().all(|result| {
            result.trace.total_budget_ms == 100
                && result
                    .hits
                    .iter()
                    .any(|hit| hit.provenance.iter().any(|label| label == "lexical_source"))
        }));
    }

    #[test]
    fn strict_batch_semantic_prefetch_error_preserves_prepared_lexical_evidence() {
        struct PartialPrefetchSidecars {
            ordinary_lexical_calls: AtomicUsize,
        }

        impl SidecarSearch for PartialPrefetchSidecars {
            fn lexical_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                self.ordinary_lexical_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/ordinary-{query}.rs"),
                    0.5,
                )])
            }

            fn lexical_search_batch(
                &self,
                queries: &[(String, usize)],
                _context: &SearchExecutionContext,
            ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
                Ok(Some(
                    queries
                        .iter()
                        .map(|(query, _)| {
                            vec![CandidateHit::lexical_stub(
                                format!("src/prepared-{query}.rs"),
                                1.0,
                            )]
                        })
                        .collect(),
                ))
            }

            fn semantic_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn semantic_search_batch(
                &self,
                _queries: &[String],
                _limit: usize,
                _context: &SearchExecutionContext,
            ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
                anyhow::bail!("simulated semantic prefetch failure")
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

        let sidecars = Arc::new(PartialPrefetchSidecars {
            ordinary_lexical_calls: AtomicUsize::new(0),
        });
        let queries = [
            QueryBatchItem {
                query: "alpha behavior",
                budget_ms: Some(100),
            },
            QueryBatchItem {
                query: "beta behavior",
                budget_ms: Some(100),
            },
        ];
        let results = execute_strict_retrieval_query_batch_against_sidecars(
            sidecars.clone(),
            Some(manifest_for("testproj", "semantic-prefetch-error", 2)),
            Arc::new(HashMap::new()),
            cancellation_flag(),
            RetrievalDegradedMode::Full,
            &queries,
            &mut RetrievalCache::new(),
            2,
        )
        .expect("semantic prefetch failure is opportunistic");

        assert_eq!(sidecars.ordinary_lexical_calls.load(Ordering::SeqCst), 0);
        assert!(results.iter().all(|result| {
            result
                .hits
                .iter()
                .any(|hit| hit.file_path.starts_with("src/prepared-"))
        }));
    }

    #[test]
    fn prepared_batch_sidecars_preserve_context_on_partial_prefetch_misses() {
        struct ContextRecordingSidecars {
            ordinary_lexical_calls: AtomicUsize,
            contextual_lexical_calls: AtomicUsize,
            ordinary_semantic_calls: AtomicUsize,
            contextual_semantic_calls: AtomicUsize,
            semantic_batch_calls: AtomicUsize,
            expected_context_address: AtomicUsize,
            contextual_calls: Mutex<Vec<(String, usize)>>,
            delegate_stage_cancelled: Arc<AtomicBool>,
        }

        impl SidecarSearch for ContextRecordingSidecars {
            fn lexical_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                self.ordinary_lexical_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/ordinary-lexical-{query}.rs"),
                    0.5,
                )])
            }

            fn lexical_search_with_context(
                &self,
                query: &str,
                limit: usize,
                context: &SearchExecutionContext,
            ) -> Result<Vec<CandidateHit>> {
                assert_eq!(
                    std::ptr::from_ref(context).addr(),
                    self.expected_context_address.load(Ordering::SeqCst),
                    "prepared cache miss must preserve the original context object"
                );
                context.timeout(Duration::from_secs(1))?;
                self.contextual_lexical_calls.fetch_add(1, Ordering::SeqCst);
                self.contextual_calls
                    .lock()
                    .expect("record contextual lexical call")
                    .push((format!("lexical:{query}"), limit));
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/contextual-lexical-{query}.rs"),
                    1.0,
                )])
            }

            fn lexical_search_batch(
                &self,
                queries: &[(String, usize)],
                _context: &SearchExecutionContext,
            ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
                Ok(Some(
                    queries
                        .iter()
                        .map(|(query, _)| {
                            vec![
                                CandidateHit::lexical_stub(
                                    format!("src/prepared-first-{query}.rs"),
                                    1.0,
                                ),
                                CandidateHit::lexical_stub(
                                    format!("src/prepared-second-{query}.rs"),
                                    0.9,
                                ),
                            ]
                        })
                        .collect(),
                ))
            }

            fn semantic_search(&self, query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                self.ordinary_semantic_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/ordinary-semantic-{query}.rs"),
                    0.5,
                )])
            }

            fn semantic_search_with_context(
                &self,
                query: &str,
                limit: usize,
                context: &SearchExecutionContext,
            ) -> Result<Vec<CandidateHit>> {
                assert_eq!(
                    std::ptr::from_ref(context).addr(),
                    self.expected_context_address.load(Ordering::SeqCst),
                    "prepared cache miss must preserve the original context object"
                );
                self.contextual_semantic_calls
                    .fetch_add(1, Ordering::SeqCst);
                self.contextual_calls
                    .lock()
                    .expect("record contextual semantic call")
                    .push((format!("semantic:{query}"), limit));
                if query == "cancel during semantic miss" {
                    self.delegate_stage_cancelled.store(true, Ordering::Release);
                    context.check_cancelled()?;
                }
                context.timeout(Duration::from_secs(1))?;
                Ok(vec![CandidateHit::lexical_stub(
                    format!("src/contextual-semantic-{query}.rs"),
                    1.0,
                )])
            }

            fn semantic_search_batch(
                &self,
                _queries: &[String],
                _limit: usize,
                _context: &SearchExecutionContext,
            ) -> Result<Option<Vec<Vec<CandidateHit>>>> {
                self.semantic_batch_calls.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("simulated semantic prefetch failure")
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

        let request_cancelled = Arc::new(AtomicBool::new(false));
        let delegate_stage_cancelled = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(ContextRecordingSidecars {
            ordinary_lexical_calls: AtomicUsize::new(0),
            contextual_lexical_calls: AtomicUsize::new(0),
            ordinary_semantic_calls: AtomicUsize::new(0),
            contextual_semantic_calls: AtomicUsize::new(0),
            semantic_batch_calls: AtomicUsize::new(0),
            expected_context_address: AtomicUsize::new(0),
            contextual_calls: Mutex::new(Vec::new()),
            delegate_stage_cancelled: Arc::clone(&delegate_stage_cancelled),
        });
        let misses = vec![
            (0, "alpha behavior".to_string(), Some(1_000)),
            (1, "beta behavior".to_string(), Some(1_000)),
        ];
        let (prepared, _) = prepare_batched_sidecars(
            inner.clone(),
            Some(&manifest_for("testproj", "partial-context", 2)),
            &misses,
            &request_cancelled,
        );
        let context = SearchExecutionContext::new(
            Instant::now() + Duration::from_secs(2),
            Arc::clone(&request_cancelled),
            Arc::clone(&delegate_stage_cancelled),
        );
        inner
            .expected_context_address
            .store(std::ptr::from_ref(&context).addr(), Ordering::SeqCst);
        assert_eq!(inner.semantic_batch_calls.load(Ordering::SeqCst), 1);

        let prepared_hit = prepared
            .lexical_search_with_context("alpha behavior", 1, &context)
            .expect("prepared lexical hit");
        assert_eq!(prepared_hit.len(), 1);
        assert_eq!(
            prepared_hit[0].file_path,
            "src/prepared-first-alpha behavior.rs"
        );
        assert_eq!(inner.ordinary_lexical_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inner.contextual_lexical_calls.load(Ordering::SeqCst), 0);

        let lexical_miss = prepared
            .lexical_search_with_context("gamma behavior", 7, &context)
            .expect("contextual lexical miss");
        let semantic_miss = prepared
            .semantic_search_with_context("gamma behavior", 9, &context)
            .expect("contextual semantic miss");
        assert_eq!(
            lexical_miss[0].file_path,
            "src/contextual-lexical-gamma behavior.rs"
        );
        assert_eq!(
            semantic_miss[0].file_path,
            "src/contextual-semantic-gamma behavior.rs"
        );
        assert_eq!(inner.ordinary_lexical_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inner.contextual_lexical_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.ordinary_semantic_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inner.contextual_semantic_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *inner
                .contextual_calls
                .lock()
                .expect("read contextual calls"),
            [
                ("lexical:gamma behavior".to_string(), 7),
                ("semantic:gamma behavior".to_string(), 9),
            ]
        );

        request_cancelled.store(true, Ordering::Release);
        let calls_before_cancelled_hit = inner.contextual_calls.lock().unwrap().len();
        assert!(
            prepared
                .lexical_search_with_context("alpha behavior", 1, &context)
                .is_err()
        );
        assert!(
            prepared
                .semantic_search_with_context("delta behavior", 1, &context)
                .is_err()
        );
        assert_eq!(
            inner.contextual_calls.lock().unwrap().len(),
            calls_before_cancelled_hit
        );

        request_cancelled.store(false, Ordering::Release);
        let cancellation = prepared
            .semantic_search_with_context("cancel during semantic miss", 3, &context)
            .expect_err("delegate cancellation must propagate");
        assert!(cancellation.to_string().contains("cancelled"));
        assert_eq!(inner.ordinary_semantic_calls.load(Ordering::SeqCst), 0);
        assert_eq!(inner.contextual_semantic_calls.load(Ordering::SeqCst), 2);
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
