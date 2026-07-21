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
mod candidate;
mod capabilities;
mod config;
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
pub use codestory_llama_sys::{
    PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS, PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS,
    PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS,
};
pub use config::{
    DEFAULT_AGENT_RUN_ID, EmbeddingRuntimeConfig, RetrievalRuntimeConfig, SidecarLayout,
    SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig, SidecarRuntimeDefaults,
    SidecarRuntimeOverrides, SummaryRuntimeConfig, sidecar_process_defaults, user_cache_root,
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
};
pub use health::{
    ComponentHealth, ComponentStatus, InfrastructureHealth, RetrievalManifestContractReport,
    RetrievalManifestLaneProvenance, RetrievalStatusReport, probe_infrastructure_health,
    probe_sidecar_health,
};
pub use index::{
    FinalizeIndexOutcome, RetrievalIndexCancelled, SidecarInputChanged, finalize_index,
    finalize_index_for_runtime, finalize_index_for_runtime_with_cancel,
    finalize_index_for_runtime_with_progress, finalize_index_for_runtime_with_progress_and_cancel,
    is_retrieval_index_cancelled, is_sidecar_input_changed, project_id_for_root,
    sidecar_project_id_for_root,
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
    AwakeMonotonicClock, EmbeddingCapacityPressureWire, EmbeddingClientBudgets,
    EmbeddingClientTransport, EmbeddingCompatibility, EmbeddingConnectIntent,
    EmbeddingConnectOutcome, EmbeddingEngineIdentity, EmbeddingEngineLeaseIdentity,
    EmbeddingExecutableIdentity, EmbeddingOperation, EmbeddingProtocolError,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingQualificationAttemptResult,
    EmbeddingQualificationOperationResult, EmbeddingQualificationParameters,
    EmbeddingQualificationRequest, EmbeddingQualificationResult,
    EmbeddingQualificationWatchdogClock, EmbeddingQualificationWatchdogMarker, EmbeddingResult,
    EmbeddingRetryStateWire, EmbeddingServerActiveRequestSnapshot,
    EmbeddingServerAuthoritySnapshot, EmbeddingServerBindOutcome, EmbeddingServerBudgets,
    EmbeddingServerClockSnapshot, EmbeddingServerEngineSnapshot, EmbeddingServerFailureSnapshot,
    EmbeddingServerListener, EmbeddingServerProcessSnapshot, EmbeddingServerProtocolSnapshot,
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
    embedding_capacity_pressure, embedding_qualification_watchdog_marker_filename,
    embedding_retry_state, install_embedding_client_transport,
    run_per_user_embedding_qualification, run_per_user_embedding_server,
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
pub use query_features::{QueryFeatures, QueryShape, classify_query};
pub use ranker::rank_candidates;
pub use retention::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport, GenerationRetentionLock,
    GenerationRetentionPlan, global_generation_gc_state_file,
};
pub use scip_client::ScipClient;
pub use sidecar::{
    sidecar_status, strict_sidecar_status, strict_sidecar_status_for_profile,
    strict_sidecar_status_for_runtime,
};
pub use sidecar_search::{LiveSidecarSearch, SidecarSearch};

pub use codestory_store::RetrievalIndexManifest;
