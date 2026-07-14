//! Retrieval v2 sidecar orchestration: health probes, local-dev clients, and index manifests.
//!
//! Public query APIs in this crate are sidecar-first. A result with
//! `retrieval_mode=full` means the manifest, lexical index, graph artifacts, and dense-anchor
//! collection agreed at query time; other modes are degraded diagnostics and must not be treated
//! as product-equivalent answer evidence.
//!
//! Cache keys and status reports intentionally carry manifest generation, input-hash, schema, and
//! projection counts. Callers that copy caches or reuse worktrees must preserve those identity
//! checks and revalidate sidecars before serving cached retrieval results.

mod cache;
mod candidate;
mod capabilities;
mod compose;
mod config;
mod embeddings;
mod executor;
mod generation;
mod health;
mod index;
mod inventory;
mod lexical_client;
mod lexical_index;
mod managed_assets;
mod mode;
pub mod outbound_http;
mod planner;
mod port_registry;
mod process_identity;
mod qdrant_client;
mod qdrant_storage;
mod query;
mod query_features;
mod ranker;
mod retention;
mod scip_client;
mod scip_index;
mod sidecar;
mod sidecar_search;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use cache::{RetrievalCache, RetrievalCacheKey};
pub use candidate::{CandidateHit, CandidateSource, RankFeatures};
pub use candidate::{is_phantom_sidecar_hit, phantom_sidecar_candidates_only};
pub use capabilities::SidecarCapabilities;
pub use compose::{
    BootstrapReport, BootstrapSidecarsOptions, DEFAULT_COMPOSE_REL_PATH, EmbedModelInventory,
    ManagedAssetPrewarmReport, NATIVE_EMBEDDING_DARWIN_EXEC_GATE_PROTOCOL,
    NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON, NativeEmbeddingStartupCleanupFailure,
    bootstrap_sidecars, bootstrap_sidecars_with_profile, bootstrap_sidecars_with_runtime,
    bootstrap_sidecars_with_runtime_progress,
    bootstrap_sidecars_with_runtime_progress_and_native_launch_observer, docker_available,
    embed_model_inventory, expected_native_embedding_launch_metadata,
    native_embedding_launch_contract_from_paths, native_embedding_launch_matches_runtime_for_reuse,
    native_embedding_startup_cleanup_failure, prewarm_managed_assets, resolve_compose_file,
};
pub use config::{
    DEFAULT_AGENT_RUN_ID, DEFAULT_EMBED_HTTP_PORT, DEFAULT_QDRANT_GRPC_PORT,
    DEFAULT_QDRANT_HTTP_PORT, EmbeddingEndpointOrigin, EmbeddingRuntimeConfig,
    EmbeddingServerLaunchMode, QDRANT_IMAGE_PIN, RetrievalRuntimeConfig, SidecarImagePins,
    SidecarLayout, SidecarOwnership, SidecarPorts, SidecarProcessDefaults, SidecarProfile,
    SidecarRuntimeConfig, SidecarRuntimeDefaults, SidecarRuntimeOverrides, SummaryRuntimeConfig,
    default_sidecar_image_pins, embedding_server_launch_mode,
    embedding_server_launch_mode_for_runtime, sidecar_process_defaults, user_cache_root,
};
#[cfg(feature = "test-support")]
pub use config::{
    active_test_cache_root, enable_automatic_test_cache_root_for_process, with_test_cache_root,
};
pub use embeddings::{
    BGE_BASE_EN_V1_5_GGUF, BGE_QUERY_PREFIX_DEFAULT, EmbeddingAcceleratorSmoke,
    EmbeddingDeviceReadiness, EmbeddingRuntimeProbe, LlamaCppEmbeddingClient,
    RETRIEVAL_EMBEDDING_DIM, embed_documents_for_runtime, embed_query_for_runtime,
    embedding_backend_label, embedding_backend_label_for_runtime, embedding_runtime_id,
    embedding_runtime_id_for_runtime, ensure_embedding_accelerator_smoke_for_runtime,
    ensure_product_embedding_backend, ensure_product_embedding_backend_for_runtime,
    probe_product_embedding_runtime, probe_product_embedding_runtime_for_runtime,
    qdrant_vector_dim,
};
pub use executor::{
    QueryExecutor, QueryResult, QueryTrace, RetrievalPublicationIdentity, StageCompletionStatus,
    StageTrace, cancellation_flag,
};
pub use generation::{SIDECAR_SCHEMA_VERSION, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED};
pub use health::{
    ComponentHealth, ComponentStatus, EmbeddingLaunchMetadata, InfrastructureHealth,
    RetrievalManifestContractReport, RetrievalManifestLaneProvenance, RetrievalRepairHint,
    RetrievalStatusReport, probe_infrastructure_health, probe_sidecar_health,
};
pub use index::{
    FinalizeIndexOutcome, ProjectQdrantRepairOutcome, finalize_index, finalize_index_for_runtime,
    finalize_index_for_runtime_with_progress, project_id_for_root,
    repair_project_qdrant_collection, repair_project_qdrant_collection_for_runtime,
    sidecar_project_id_for_root,
};
pub use inventory::{
    SidecarDockerResource, SidecarDockerResourceKind, SidecarGcNamespaceResult, SidecarGcReport,
    SidecarInventoryEntry, SidecarInventoryReport, SidecarInventoryState, sidecar_gc_apply,
    sidecar_gc_apply_with_storage, sidecar_inventory, sidecar_inventory_with_storage,
};
pub use lexical_client::LexicalClient;
pub use lexical_index::LEXICAL_INDEX_VERSION;
pub use mode::RetrievalDegradedMode;
pub use mode::derive_degraded_mode;
pub use planner::{PlannedStage, RetrievalPlan, RetrievalStageKind, plan_query};
pub use process_identity::{
    ProcessOwnerState, ProcessStartProbe, native_embedding_process_start_identity,
    probe_process_start_identity, process_owner_state,
};
pub use qdrant_client::{
    QDRANT_INDEX_UPSERT_BATCH_SIZE, QDRANT_VECTOR_DIM, QdrantClient, QdrantUpsertPoint,
    diagnostic_query_vector,
};
pub use qdrant_storage::{
    BootstrapStorageScope, DEFAULT_QDRANT_COLLECTION_RETENTION,
    PRUNE_SUPPRESSED_POST_PUBLICATION_RETENTION, PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR,
    QdrantStorageRepairReport, repair_qdrant_storage,
};
pub use query::{
    QueryBatchItem, QueryBatchRequest, QueryRequest, execute_retrieval_query,
    execute_retrieval_query_with_cache, execute_retrieval_query_with_cache_for_runtime,
    execute_strict_retrieval_query_batch_with_cache,
    execute_strict_retrieval_query_batch_with_cache_for_runtime,
    retrieval_publication_identity_from_storage,
};
pub use query_features::{QueryFeatures, QueryShape, classify_query};
pub use ranker::rank_candidates;
pub use retention::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport, GenerationRetentionLock,
    GenerationRetentionPlan, global_generation_gc_state_file,
};
pub use scip_client::ScipClient;
pub use sidecar::{
    EmbeddingLaunchOwnership, NativeEmbeddingLaunchIdentityStatus, SidecarStateFile,
    attached_native_embedding_state_paths, ensure_native_embedding_launch_identity,
    native_embedding_launch_identity_status, sidecar_down,
    sidecar_down_after_failed_bootstrap_for_runtime, sidecar_down_for_project,
    sidecar_down_for_runtime, sidecar_state_matches_runtime, sidecar_status, sidecar_up,
    sidecar_up_with_runtime, sidecar_up_with_runtime_preserving_launch,
    stop_native_embedding_process_for_launch, strict_sidecar_status,
    strict_sidecar_status_for_profile, strict_sidecar_status_for_runtime,
    validate_sidecar_state_matches_runtime,
};
pub use sidecar_search::{LiveSidecarSearch, SidecarSearch};

pub use codestory_store::RetrievalIndexManifest;
