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
mod mode;
mod planner;
mod qdrant_client;
mod qdrant_storage;
mod query;
mod query_features;
mod ranker;
mod scip_client;
mod scip_index;
mod sidecar;
mod sidecar_search;
mod zoekt_client;
mod zoekt_index;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use cache::{RetrievalCache, RetrievalCacheKey};
pub use candidate::{CandidateHit, CandidateSource, RankFeatures};
pub use candidate::{is_phantom_sidecar_hit, phantom_sidecar_candidates_only};
pub use capabilities::SidecarCapabilities;
#[allow(deprecated)]
pub use compose::bootstrap_sidecars_without_storage_scope;
pub use compose::{
    BootstrapReport, DEFAULT_COMPOSE_REL_PATH, EmbedModelInventory, bootstrap_sidecars,
    bootstrap_sidecars_with_profile, bootstrap_sidecars_with_runtime,
    bootstrap_sidecars_with_runtime_progress, docker_available, embed_model_inventory,
    resolve_compose_file,
};
pub use config::{
    DEFAULT_AGENT_RUN_ID, DEFAULT_EMBED_HTTP_PORT, DEFAULT_QDRANT_GRPC_PORT,
    DEFAULT_QDRANT_HTTP_PORT, DEFAULT_ZOEKT_HTTP_PORT, QDRANT_IMAGE_PIN, SidecarImagePins,
    SidecarLayout, SidecarOwnership, SidecarPorts, SidecarProfile, SidecarRuntimeConfig,
    ZOEKT_REAL_VERSION_PIN, ZOEKT_WEBSERVER_IMAGE_PIN, default_sidecar_image_pins,
    sidecar_runtime_auto, sidecar_runtime_for_project, sidecar_runtime_for_project_with_run_id,
};
pub use config::{qdrant_enabled, qdrant_semantic_vectors_enabled};
pub use embeddings::qdrant_vector_dim;
pub use embeddings::{
    BGE_BASE_EN_V1_5_GGUF, BGE_QUERY_PREFIX_DEFAULT, RETRIEVAL_EMBEDDING_DIM,
    embedding_backend_label, embedding_runtime_id, ensure_product_embedding_backend,
    ensure_product_embedding_backend_for_runtime,
};
pub use executor::{QueryExecutor, QueryResult, QueryTrace, StageTrace, cancellation_flag};
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
    sidecar_inventory,
};
pub use mode::RetrievalDegradedMode;
pub use mode::derive_degraded_mode;
pub use planner::{PlannedStage, RetrievalPlan, RetrievalStageKind, plan_query};
pub use qdrant_client::{
    QDRANT_INDEX_UPSERT_BATCH_SIZE, QDRANT_VECTOR_DIM, QdrantClient, QdrantUpsertPoint,
    diagnostic_query_vector,
};
pub use qdrant_storage::{
    BootstrapStorageScope, DEFAULT_QDRANT_COLLECTION_RETENTION,
    PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR, QdrantStorageRepairReport, repair_qdrant_storage,
};
pub use query::{
    QueryBatchItem, QueryBatchRequest, QueryRequest, execute_retrieval_query,
    execute_retrieval_query_with_cache, execute_strict_retrieval_query_batch_with_cache,
};
pub use query_features::{QueryFeatures, QueryShape, classify_query};
pub use ranker::rank_candidates;
pub use scip_client::ScipClient;
pub use sidecar::{
    SidecarStateFile, sidecar_down, sidecar_down_for_project, sidecar_down_for_runtime,
    sidecar_status, sidecar_up, sidecar_up_with_runtime, strict_sidecar_status,
    strict_sidecar_status_for_profile, strict_sidecar_status_for_runtime,
};
pub use sidecar_search::{LiveSidecarSearch, SidecarSearch};
pub use zoekt_client::ZoektClient;

pub use codestory_store::RetrievalIndexManifest;
