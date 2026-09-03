//! Retrieval generations, managed per-user embeddings, health probes, and index manifests.
//!
//! A result with
//! `retrieval_mode=full` means the manifest, lexical index, graph artifacts, and dense-anchor
//! collection agreed at query time; other modes are degraded diagnostics and must not be treated
//! as product-equivalent answer evidence.
//!
//! Cache keys and status reports intentionally carry manifest generation, input-hash, schema, and
//! projection counts. Callers that copy caches or reuse worktrees must preserve those identity
//! checks and revalidate generations before serving cached retrieval results.

mod cache;
mod cache_clean;
mod cache_inventory;
mod candidate;
mod capabilities;
mod config;
mod content_addressed_vector_cache;
mod copy_on_write;
mod embedded_vector;
mod embedding_contract;
mod embedding_server_compat;
mod embeddings;
mod executor;
mod generation;
mod health;
mod index;
mod inventory;
mod lexical_client;
mod lexical_index;
mod mode;
pub mod outbound_http;
mod per_user_embedding;
mod planner;
mod process_identity;
mod query;
mod query_features;
mod ranker;
mod retention;
mod rollback;
mod scip_client;
mod scip_index;
#[cfg(feature = "semantic-calibration-support")]
pub mod semantic_calibration_support;
mod sidecar;
mod sidecar_search;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

/// Narrow measurement surface for the offline vector-backend bake-off.
///
/// The bake-off has to compare candidate backends against *the shipped dense
/// lane*, not against a second copy of it. Everything here therefore forwards
/// into the same publication and scan functions product queries use, so a
/// measured incumbent number cannot drift from what the product actually runs.
/// The module is feature-gated because it is measurement infrastructure: no
/// product binary enables `benchmark-support`.
#[cfg(feature = "benchmark-support")]
pub mod benchmark_support {
    use crate::config::SidecarLayout;
    use crate::embedded_vector::{
        AttestedSemanticPoint, AttestedVectorPublication, EmbeddedVectorIndex,
        ExpectedVectorAnchor, SemanticPoint, VectorEvidenceContract,
    };
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    /// Prepare the existing lexical implementation without semantic or graph
    /// work. Only the frozen witness experiment consumes this isolated shard.
    pub fn prepare_witness_lexical_shard(
        project_root: &Path,
        core: &codestory_store::CoreReadSession,
        lexical_root: &Path,
    ) -> Result<String> {
        let source =
            crate::lexical_index::lexical_source_input(project_root, core.generation_path())?;
        let expected = crate::lexical_index::prepare_lexical_input_for_store(
            source,
            project_root,
            core.storage(),
        )?;
        anyhow::ensure!(
            expected.fingerprint.coverage.complete(),
            "incomplete lexical source discovery"
        );
        let input_hash = expected.fingerprint.hash.clone();
        crate::lexical_index::build_prepared_lexical_shard(
            lexical_root,
            &core.identity().generation_id,
            &expected,
            &input_hash,
            None,
            || expected.revalidate_source_seals(project_root, core.generation_path()),
        )?;
        Ok(input_hash)
    }

    /// One vector the bake-off publishes and later scores against.
    #[derive(Debug, Clone, PartialEq)]
    pub struct BenchmarkVector {
        pub node_id: String,
        pub vector: Vec<f32>,
    }

    /// A published generation the bake-off can query.
    #[derive(Debug, Clone)]
    pub struct PublishedVectorGeneration {
        layout: SidecarLayout,
        collection: String,
        generation: String,
        input_hash: String,
        point_count: u64,
    }

    impl PublishedVectorGeneration {
        pub fn point_count(&self) -> u64 {
            self.point_count
        }

        /// Bytes the published SQLite database occupies on disk.
        pub fn database_bytes(&self) -> Result<u64> {
            Ok(std::fs::metadata(self.database_path())?.len())
        }

        pub fn database_path(&self) -> PathBuf {
            crate::embedded_vector::index_path(&self.layout, &self.collection)
        }
    }

    fn layout_for_root(root: &Path) -> SidecarLayout {
        SidecarLayout {
            lexical_data_dir: root.join("lexical"),
            semantic_data_dir: root.join("semantic"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("state.json"),
        }
    }

    /// Publish `vectors` through the product's attested publication path.
    ///
    /// This is the real `build_attested_with_points_with_cancel`: anchor
    /// coverage, per-vector validation, the canonical digest, and the atomic
    /// old-or-new publication all run exactly as they do in a product refresh.
    pub fn publish_vector_generation(
        root: &Path,
        collection: &str,
        generation: &str,
        input_hash: &str,
        embedding_dim: usize,
        vectors: &[BenchmarkVector],
    ) -> Result<PublishedVectorGeneration> {
        let layout = layout_for_root(root);
        layout.ensure_data_dirs()?;
        let contract = VectorEvidenceContract::new(
            "bakeoff-embedding-backend",
            embedding_dim,
            "bakeoff-producer-identity",
            "bakeoff-evidence-contract-v1",
        );
        let anchors = vectors
            .iter()
            .map(|vector| ExpectedVectorAnchor {
                node_id: vector.node_id.clone(),
                document_hash: document_hash_for(&vector.node_id),
            })
            .collect::<Vec<_>>();
        let attestation = EmbeddedVectorIndex::build_attested_with_points_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection,
                generation,
                input_hash,
                contract: &contract,
                expected_anchors: &anchors,
            },
            || Ok(()),
            |visit| {
                for vector in vectors {
                    visit(AttestedSemanticPoint {
                        point: SemanticPoint {
                            display_name: vector.node_id.clone(),
                            node_id: vector.node_id.clone(),
                            file_path: Some(format!("{}.rs", vector.node_id)),
                            file_role: None,
                            dense_reason: None,
                            vector: vector.vector.clone(),
                        },
                        document_hash: document_hash_for(&vector.node_id),
                    })?;
                }
                Ok(())
            },
        )
        .context("publish bake-off vector generation")?;
        Ok(PublishedVectorGeneration {
            layout,
            collection: collection.to_string(),
            generation: generation.to_string(),
            input_hash: input_hash.to_string(),
            point_count: attestation.point_count,
        })
    }

    fn document_hash_for(node_id: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(node_id.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Run the product dense scan against a published generation.
    ///
    /// Returns `(node_id, score)` in the order the product lane reports them,
    /// after the product's own dense-abstention filter.
    pub fn scan_published_vectors(
        published: &PublishedVectorGeneration,
        query: &[f32],
        limit: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<(String, f32)>> {
        let hits = crate::embedded_vector::search_database(
            &published.database_path(),
            &published.generation,
            &published.input_hash,
            query,
            limit,
            cancelled,
        )?;
        Ok(hits
            .into_iter()
            .map(|hit| {
                (
                    hit.node_id.clone().unwrap_or_else(|| hit.file_path.clone()),
                    hit.score,
                )
            })
            .collect())
    }

    /// Read every published vector back out, so a candidate backend builds
    /// from the same bytes the incumbent scans.
    pub fn read_published_vectors(
        published: &PublishedVectorGeneration,
        embedding_dim: usize,
    ) -> Result<Vec<BenchmarkVector>> {
        crate::embedded_vector::read_published_vectors_for_benchmark(
            &published.database_path(),
            embedding_dim,
        )
        .map(|vectors| {
            vectors
                .into_iter()
                .map(|(node_id, vector)| BenchmarkVector { node_id, vector })
                .collect()
        })
    }

    /// The product's own semantic-stage budget for `query`, in milliseconds.
    ///
    /// Read out of `plan_query` rather than restated, so the bake-off gate
    /// moves when the shipped budget moves instead of measuring a number the
    /// product no longer uses.
    pub fn semantic_stage_budget_ms(query: &str) -> Option<u64> {
        crate::planner::plan_query(
            &crate::query_features::classify_query(query),
            crate::mode::RetrievalDegradedMode::Full,
        )
        .stages
        .into_iter()
        .find(|stage| stage.kind == crate::planner::RetrievalStageKind::Stage1bSemantic)
        .map(|stage| stage.budget_ms)
    }
}

pub use cache::{RetrievalCache, RetrievalCacheKey};
pub use cache_clean::{
    CACHE_CLEAN_SCHEMA_VERSION, CacheCleanCandidate, CacheCleanKind, CacheCleanPlan,
    CacheCleanRefusal, CacheCleanRemoval, CacheCleanReport, CacheCleanRetained, apply_cache_clean,
    plan_cache_clean,
};
pub use cache_inventory::{
    CACHE_INVENTORY_SCHEMA_VERSION, CacheCloneSharing, CacheConsumer, CacheHardlinkGroup,
    CacheInventoryEntry, CacheInventoryKind, CacheInventoryReport, cache_inventory,
};
pub use candidate::{
    CandidateGraphDirection, CandidateGraphEvidence, CandidateHit, CandidateLane,
    CandidateLaneEvidence, CandidateLaneScores, CandidateSource, RankFeatures,
};
pub use candidate::{is_phantom_sidecar_hit, phantom_sidecar_candidates_only};
pub use capabilities::SidecarCapabilities;
pub use codestory_llama_sys::{
    PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS, PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS,
    PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS,
};
pub use config::{
    DEFAULT_AGENT_RUN_ID, EmbeddingRuntimeConfig, RetrievalRuntimeConfig, SidecarLayout,
    SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig, SidecarRuntimeDefaults,
    SidecarRuntimeOverrides, SummaryRuntimeConfig, hybrid_retrieval_enabled_from_process_env,
    retrieval_runtime_config_from_process_env, sidecar_process_defaults, user_cache_root,
};
#[cfg(feature = "test-support")]
pub use config::{
    active_test_cache_root, enable_automatic_test_cache_root_for_process, with_test_cache_root,
};
#[cfg(feature = "test-support")]
pub use embeddings::TEST_EMBEDDING_UNAVAILABLE_MARKER;
pub use embeddings::{
    CODERANK_EMBED_Q8_GGUF, CODERANK_QUERY_PREFIX_DEFAULT, EmbeddingAcceleratorSmoke,
    EmbeddingDeviceReadiness, EmbeddingRuntimeProbe, PRODUCT_EMBEDDING_RUNTIME_ID,
    ProductEmbeddingClient, RETRIEVAL_EMBEDDING_DIM, embed_documents_for_runtime,
    embed_query_for_runtime, embedding_backend_label, embedding_backend_label_for_runtime,
    embedding_runtime_id, embedding_runtime_id_for_runtime,
    ensure_embedding_accelerator_smoke_for_runtime, ensure_product_embedding_backend,
    ensure_product_embedding_backend_for_runtime, probe_product_embedding_runtime,
    probe_product_embedding_runtime_for_runtime, semantic_vector_dim,
};
pub use executor::{
    QueryExecutor, QueryResult, QueryTrace, RetrievalPublicationIdentity, StageCompletionStatus,
    StageTrace, cancellation_flag,
};
pub use generation::{
    SEMANTIC_POLICY_VERSION, SIDECAR_SCHEMA_VERSION, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED,
    manifest_unavailable_reason_for_runtime, storage_admission_refusal_reason_for_runtime,
};
pub use health::{
    ComponentHealth, ComponentStatus, InfrastructureHealth, RetrievalManifestContractReport,
    RetrievalManifestLaneProvenance, RetrievalStatusReport, manifest_classifies_full,
    probe_infrastructure_health, probe_sidecar_health,
};
pub use index::{
    FinalizeComponentWork, FinalizeIndexOutcome, FinalizePhaseTiming,
    IncrementalRetrievalRefreshReceipt, RetrievalIndexCancelled, SidecarInputChanged,
    clear_incremental_retrieval_refresh_receipt, finalize_index, finalize_index_for_runtime,
    finalize_index_for_runtime_with_cancel, finalize_index_for_runtime_with_progress,
    finalize_index_for_runtime_with_progress_and_cancel,
    install_incremental_retrieval_refresh_receipt, is_retrieval_index_cancelled,
    is_sidecar_input_changed, project_id_for_root, sidecar_project_id_for_root,
};
pub use inventory::{
    SidecarGcReport, SidecarInventoryReport, sidecar_gc_apply_with_storage,
    sidecar_inventory_with_storage,
};
pub use lexical_client::LexicalClient;
pub use lexical_index::LEXICAL_INDEX_VERSION;
pub use mode::RetrievalDegradedMode;
pub use mode::derive_degraded_mode;
pub use per_user_embedding::{
    AwakeMonotonicClock, EMBEDDING_BUSY_RETRY_QUEUE_CLASS,
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingCapacityPressureWire,
    EmbeddingClientBudgets, EmbeddingClientTransport, EmbeddingCompatibility,
    EmbeddingConnectIntent, EmbeddingConnectOutcome, EmbeddingEngineIdentity,
    EmbeddingEngineLeaseIdentity, EmbeddingExecutableIdentity, EmbeddingOperation,
    EmbeddingProtocolError, EmbeddingProtocolRequest, EmbeddingProtocolResponse,
    EmbeddingQualificationAttemptResult, EmbeddingQualificationOperationResult,
    EmbeddingQualificationParameters, EmbeddingQualificationRequest, EmbeddingQualificationResult,
    EmbeddingQualificationWatchdogClock, EmbeddingQualificationWatchdogMarker,
    EmbeddingQualificationWorkerError, EmbeddingQualificationWorkerMeasurement,
    EmbeddingQualificationWorkerMeasurementSpan, EmbeddingQualificationWorkerOutput,
    EmbeddingQualificationWorkerProtocolExchange, EmbeddingQualificationWorkerQueueOperation,
    EmbeddingQualificationWorkerRequest, EmbeddingResult, EmbeddingRetryStateWire,
    EmbeddingServerActiveRequestSnapshot, EmbeddingServerAuthoritySnapshot,
    EmbeddingServerBindOutcome, EmbeddingServerBudgets, EmbeddingServerClockSnapshot,
    EmbeddingServerEngineSnapshot, EmbeddingServerFailureSnapshot, EmbeddingServerListener,
    EmbeddingServerProcessSnapshot, EmbeddingServerProtocolSnapshot,
    EmbeddingServerSchedulerSnapshot, EmbeddingServerSnapshot, EmbeddingServerStream,
    EmbeddingServerTransport, EmbeddingSpawnAttempt, EmbeddingTransportFailure,
    EmbeddingTransportIdentity, PER_USER_EMBEDDING_BOOTSTRAP_VERSION,
    PER_USER_EMBEDDING_CONSTANT_SET_FROZEN, PER_USER_EMBEDDING_CONSTANT_SET_SHA256,
    PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT, PER_USER_EMBEDDING_MAX_INPUT_BYTES,
    PER_USER_EMBEDDING_MAX_METADATA_BYTES, PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES,
    PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256, PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
    PER_USER_EMBEDDING_PROTOCOL_SHA256, PER_USER_EMBEDDING_PROTOCOL_V1,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, PER_USER_EMBEDDING_SERVER_PROOF_MARKER,
    PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION, PerUserEmbeddingClient,
    PerUserEmbeddingError, PerUserEmbeddingResidencyLease, PerUserEmbeddingServerConfig,
    QualificationGateEnvironment, embedding_capacity_pressure,
    embedding_qualification_watchdog_marker_filename, embedding_retry_state,
    install_embedding_client_transport, install_embedding_client_transport_factory,
    qualification_gate_environment, run_per_user_embedding_qualification,
    run_per_user_embedding_server,
};
pub use planner::{PlannedStage, RetrievalPlan, RetrievalStageKind, plan_query};
pub use process_identity::{
    ProcessOwnerState, ProcessStartProbe, probe_process_start_identity, process_owner_state,
};
pub use query::{
    PinnedQuerySession, QueryBatchItem, QueryBatchRequest, QueryRequest,
    RETRIEVAL_PUBLICATION_CHANGED_CODE, RetrievalPublicationChanged, execute_retrieval_query,
    execute_retrieval_query_with_cache, execute_retrieval_query_with_cache_for_runtime,
    execute_strict_retrieval_query_batch_with_cache,
    execute_strict_retrieval_query_batch_with_cache_for_runtime, is_retrieval_publication_changed,
    retrieval_publication_identity_from_storage,
};
pub use query_features::{
    QUERY_INTENT_POLICY_VERSION, QueryFeatures, QueryIntent, QueryLookupMode, QueryShape,
    classify_query,
};
pub use ranker::{RANKING_POLICY_VERSION, rank_candidates};
pub use retention::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport, GenerationRetentionLock,
    GenerationRetentionPlan, MarkerRetirement, ObservedRetentionLock, RETENTION_MARKER_SCHEMA_V1,
    RETENTION_MARKER_SCHEMA_V2, global_generation_gc_state_file,
};
pub use rollback::{
    RetainedRollbackObservation, RollbackActivationError, RollbackActivationOutcome,
    RollbackActivationRefusal, activate_retained_rollback_generation,
    observe_retained_rollback_generation,
};
pub use scip_client::ScipClient;
pub use sidecar::{
    ReadyEmbeddingEngineIdentity, ReadyRetrievalIdentity,
    observe_ready_retrieval_identity_for_project_id, ready_retrieval_identity_for_runtime,
    sidecar_status, strict_descriptor_sidecar_status_for_runtime, strict_sidecar_status,
    strict_sidecar_status_for_profile, strict_sidecar_status_for_runtime,
};
pub use sidecar_search::{LiveSidecarSearch, SidecarSearch};

pub use codestory_store::RetrievalIndexManifest;
