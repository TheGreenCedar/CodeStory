//! Headless CodeStory runtime orchestration.
//!
//! The public controller owns project state, search, packet generation, cache rehydration, and
//! retrieval sidecar coordination. Packet/search methods are sidecar-primary: degraded retrieval
//! is surfaced as diagnostics or errors, not as product-equivalent answer evidence.

use crate::agent::packet_evidence::decorate_search_hit_evidence;
use codestory_contracts::api::{
    AffectedAnalysisBoundsDto, AffectedAnalysisCompletenessDto, AffectedAnalysisDto,
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedFollowUpDto, AffectedFollowUpInvocationDto, AffectedInputClassificationDto,
    AffectedMatchedFileDto, AffectedRouteDto, AffectedSymbolDto, AffectedTestFileDto,
    AffectedUncoveredInputDto, AffectedUnmatchedPathDto, AgentHybridWeightsDto, ApiError,
    AppEventPayload, EdgeKind, EmbeddingProfileContractDto, FrameworkRouteCoverageDto,
    GraphEdgeDto, GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto,
    GroundingCoverageBucketDto, GroundingFileDigestDto, GroundingOrientationConfidenceDto,
    GroundingOrientationDto, GroundingOrientationUncertaintyDto, GroundingSnapshotDto,
    GroundingSymbolDigestDto, IndexFreshnessChangeKindDto, IndexFreshnessDto,
    IndexFreshnessSampleDto, IndexFreshnessStatusDto, IndexedFileRoleDto, IndexingPhaseTimings,
    NodeDetailsRequest, NodeId, NodeKind, RepoTextScanStatsDto, RetrievalFallbackReasonDto,
    RetrievalModeDto, RetrievalScoreBreakdownDto, RetrievalStateDto, RouteEndpointKindDto,
    RouteEndpointMetadataDto, SearchHit, SearchHitOrigin, SearchHybridLimitsDto,
    SearchMatchQualityDto, SearchPlanAnchorGroupDto, SearchPlanBridgeConfidenceDto,
    SearchPlanBridgeDto, SearchPlanBridgeEvidenceKindDto, SearchPlanBridgeStatusDto,
    SearchPlanCandidateWindowDto, SearchPlanChannelDto, SearchPlanDroppedTermDto, SearchPlanDto,
    SearchPlanNextActionDto, SearchPlanPromotionStatusDto, SearchPlanRejectedHitDto,
    SearchPlanSubqueryDto, SearchPlanTermsDto, SearchQueryAssessmentDto, SearchRepoTextMode,
    SearchRequest, SearchResultsDto, SemanticModeDto, SnippetContextDto, StorageStatsDto,
    StoredSemanticDocsContractDto, SymbolContextDto, SystemActionResponse, TrailConfigDto,
    TrailContextDto, WorkspaceMemberIndexDto,
};
use codestory_contracts::graph::{AccessKind, Edge as GraphEdge, Node as GraphNode};
use codestory_contracts::language_support::{
    LanguageSupportProfile, language_support_profile_for_ext,
    language_support_profile_for_language_name,
};
use codestory_indexer::CancellationToken;
#[cfg(test)]
use codestory_store::RetrievalIndexManifest;
use codestory_store::{
    BUILD_EDGE_SEED_BATCH_SIZE, CURRENT_SCHEMA_VERSION, DenseAnchorInput,
    DenseAnchorInputReuseMetadata, FileInfo, FileRole as StoreFileRole, GroundingEdgeKindCount,
    GroundingNodeRecord, IndexPublicationRecord, LlmSymbolDoc, LlmSymbolDocStats,
    SearchSymbolProjection, SourcePolicyExclusionPolicyIdentity, Store, SymbolSearchDoc,
    SymbolSummaryRecord,
};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
#[cfg(test)]
use codestory_workspace::{DEFAULT_SOURCE_FILE_BYTE_CAP, OVERSIZED_SOURCE_POLICY_VERSION};
use codestory_workspace::{
    RefreshExecutionPlan, RefreshInputs, SourceIndexPolicy, WorkspaceInventoryOutcome,
    WorkspaceManifest, WorkspacePathIdentity,
};
use crossbeam_channel::{Receiver, Sender};
use fs4::fs_std::FileExt;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::BufRead;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};
use uuid::Uuid;

mod affected;
mod agent;
mod index_commit;
mod index_coverage;
mod index_freshness;
mod index_full;
mod index_incremental;
mod index_timings;
mod publication;
mod repo_text;
mod route_coverage;
mod search_intent;
mod search_plan;
mod search_publication;
mod search_scoring;
mod search_state;
mod search_state_cache;
mod search_terms;
mod semantic_projection;
mod semantic_republish;
mod snippets;
mod workspace_state;
use affected::{AffectedOperationIdentityIndex, IndexFreshnessObservation};
pub use agent::{packet_step_trace_json, plan_packet};
use index_commit::*;
pub(crate) use index_coverage::{
    current_epoch_ms, file_coverage_detail, file_coverage_reason, file_coverage_retryable,
    full_refresh_execution_plan_with_coverage, indexed_file_role, normalize_path_key,
    path_role_from_key, publish_source_policy_exclusions, revalidate_source_policy_exclusions,
    runtime_relative_path, source_coverage_failure_code, source_policy_exclusion_candidate,
    stored_file_coverage_diagnostics, validate_source_policy_exclusions,
    validate_structural_text_units,
};
use index_freshness::{
    CachedIndexFreshness, index_freshness_observation_from_storage_with_identities,
    indexable_source_path_in_workspace, indexable_source_path_with_root, open_storage_for_read,
};
#[cfg(test)]
use index_freshness::{
    EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP, arm_after_index_freshness_fence_test_hook,
    index_freshness_from_storage, indexable_source_path, not_checked_index_freshness,
};
#[cfg(test)]
use publication::{
    PUBLICATION_TEST_FAULT, PublicationTestAction, PublicationTestBoundary,
    arm_activation_search_before_revalidate_hook, arm_full_refresh_staged_store_hook,
    arm_incremental_staged_store_hook, arm_publication_test_fault,
    arm_semantic_projection_before_revalidate_hook, arm_source_policy_after_plan_hook,
    arm_source_policy_before_revalidate_hook,
};
#[cfg(test)]
use route_coverage::compare_optional_confidence_desc;
use route_coverage::route_endpoint_adjusted_search_score;
#[cfg(test)]
use search_publication::{
    SearchGenerationCompletion, llm_doc_embed_batch_size, load_persisted_search_state,
    search_generation_completion_path, search_index_generation_root, search_index_storage_path,
};
use search_publication::{load_canonical_search_symbols, retrieval_state_from_storage_for_runtime};
#[cfg(test)]
use search_scoring::HybridSearchInstrumentation;
pub(crate) use search_scoring::HybridSearchScoredHit;
use search_state_cache::*;
pub use semantic_projection::SemanticProjectionRepublishOutcome;
use semantic_projection::edge_digest_for_node;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use semantic_projection::stored_semantic_embeddings_for_test;
#[cfg(test)]
use semantic_projection::{
    DENSE_CENTRAL_RELATIONSHIP_THRESHOLD, DENSE_CENTRAL_SCORE_THRESHOLD, DenseAnchorCentrality,
    DenseAnchorReason, LEGACY_OVERSIZED_SOURCE_POLICY_VERSION, LLM_DOC_EMBED_BATCH_SIZE_ENV,
    LLM_SYMBOL_DOC_SCHEMA_VERSION, PendingLlmSymbolDoc, SEMANTIC_DOC_ALIAS_MODE_ENV,
    SEMANTIC_DOC_DEFAULT_MAX_TOKENS, SEMANTIC_DOC_MAX_TOKENS_ENV, SEMANTIC_DOC_SCOPE_ENV,
    SEMANTIC_EDGE_STREAM_BATCH_SIZE, SEMANTIC_STREAM_PENDING_DOCS_ENV,
    SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV, SYMBOL_SEARCH_DOC_PROVENANCE, SemanticDocAliasMode,
    SemanticDocGraphContext, SemanticDocScope, build_component_report_docs,
    build_llm_symbol_doc_text, build_search_state, build_semantic_file_text_cache_with_limits,
    dense_anchor_is_central, dense_anchor_reason_for_node, finalize_staged_semantic_docs,
    flush_pending_dense_anchor_inputs, llm_indexable_kind, llm_indexable_kind_for_scope,
    llm_indexable_kinds_for_scope, llm_symbol_doc_hash, semantic_doc_alias_mode_from_env,
    semantic_doc_alias_mode_from_value, semantic_doc_max_tokens_from_env,
    semantic_doc_scope_from_env, semantic_doc_scope_from_value, semantic_doc_shape_contract,
    semantic_doc_text_budget_cost, semantic_stream_sort_window_batches_from_env,
    sort_pending_dense_anchor_inputs, stream_pending_llm_symbol_docs_from_env,
    truncate_semantic_doc_text_to_token_budget,
};
#[cfg(test)]
pub(crate) use snippets::markdown_snippet;
pub(crate) use snippets::{
    BoundedSnippetRangeOptions, DIRECT_SNIPPET_MAX_BYTES, DIRECT_SNIPPET_TRUNCATION_SUFFIX,
};

mod browser;
mod cache_rehydrate;
mod controller_bookmarks;
mod controller_core;
mod controller_files;
mod controller_indexing;
mod controller_symbols;
pub(crate) use controller_core::no_project_error;
pub mod graph_analysis;
mod graph_builders;
mod graph_canonical;
mod graph_dto;
mod grounding;
mod mermaid;
mod path_identity;
mod path_resolution;
#[doc(hidden)]
pub use path_resolution::resolve_project_file_path_from_root;
mod process_config;
pub use process_config::RuntimeProcessConfig;
mod query_language;
mod repository_identity;
mod search;
mod search_runtime;
#[cfg(feature = "benchmark-support")]
pub mod benchmark_support {
    pub use crate::search::engine::{SearchEngine, SymbolIndexSession, SymbolIndexWriteStats};
}

mod semantic_doc_text;
mod services;
mod support;
mod symbol_query;
mod symbol_workflow;
mod system_actions;
mod target_resolution;
#[cfg(test)]
mod tests;
mod trail_story;

pub use browser::{BrowserQueryItem, ReadOnlyBrowserService};
pub use cache_rehydrate::{CacheRehydrateOutput, CacheRehydrateRequest, rehydrate_cache};
pub use codestory_contracts as contracts;
pub(crate) use graph_dto::{
    app_graph_flags, edge_certainty_label, graph_edge_dto, is_structural_kind, member_access_dto,
};
pub(crate) use mermaid::{fallback_mermaid, mermaid_flowchart, mermaid_gantt, mermaid_sequence};
use path_identity::{OperationPathIdentityResolver, PathIdentityUnavailable};
pub use query_language::{GraphQueryParseError, parse_graph_query};
pub use repository_identity::{
    REPOSITORY_IDENTITY_SCHEMA_VERSION, RepositoryIdentityReport, inspect_repository_identity,
};
pub(crate) use search_runtime::SearchEngine;

#[cfg(test)]
static PROCESS_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn process_env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) fn test_sidecar_runtime_from_env() -> codestory_retrieval::SidecarRuntimeConfig {
    let process_defaults = codestory_retrieval::SidecarProcessDefaults::new(
        std::env::temp_dir().join(format!("codestory-runtime-tests-{}", std::process::id())),
        codestory_retrieval::SidecarRuntimeDefaults::from_process_env(),
    );
    codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        None,
        codestory_retrieval::SidecarProfile::Local,
        None,
        &process_defaults,
        &codestory_retrieval::SidecarRuntimeOverrides::default(),
    )
}
pub use search_runtime::*;
use semantic_doc_text::{
    semantic_doc_language_from_path, semantic_path_aliases, semantic_symbol_aliases,
    semantic_symbol_role_aliases,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use services::set_before_retrieval_pin_test_hook;
pub use services::{
    ActivationCapabilities, ActivationCapabilityState, ActivationOperation, ActivationRun,
    ActivationService, ActivationSnapshot, ActivationStage, ActivationState,
    ActivePublicOperationPublication, AgentService, BookmarkService, GroundingService,
    IndexService, ProjectService, PublicOperation, PublicOperationService, SearchService,
    TrailService, embedding_api_error,
};
pub use symbol_workflow::{
    SymbolWorkflowCaps, SymbolWorkflowMode, SymbolWorkflowNode, SymbolWorkflowOutcome,
    SymbolWorkflowRequest, SymbolWorkflowResolution, SymbolWorkflowResponse, SymbolWorkflowRoute,
    SymbolWorkflowTest,
};
pub use target_resolution::{
    AmbiguousTarget, ResolvedTarget, TargetResolution, TargetSelection, TargetSelector,
    prefer_function_body_target,
};

pub(crate) use support::{
    FocusedSourceContext, HYBRID_RETRIEVAL_ENABLED_ENV, SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES,
    SEMANTIC_FILE_TEXT_MAX_BYTES, aggregate_symbol_matches, clamp_i64_to_u32, clamp_u64_to_u32,
    clamp_u128_to_u32, clamp_usize_to_u32, extract_symbol_search_terms, file_text_match_line,
    hybrid_retrieval_enabled, looks_like_repo_text_query, node_display_name, preferred_occurrence,
    query_has_symbol_or_literal_signal, read_file_text_limited, read_searchable_file_contents,
    should_expand_symbol_query,
};
#[cfg(test)]
pub(crate) use support::{apply_hybrid_limits, normalized_hybrid_weights};
#[cfg(test)]
use symbol_query::compare_search_hits;
pub use symbol_query::{
    RetrievalFileRole, SymbolNameMatchRank, compare_ranked_hits, leading_symbol_segment,
    normalize_symbol_query, retrieval_file_role_for_hit, retrieval_file_role_from_path,
    symbol_name_match_rank, symbol_query_tokens, terminal_symbol_segment,
};
pub(crate) use symbol_query::{
    architecture_query_intents, compare_search_hits_with_project_root, exact_symbol_query_terms,
    is_non_primary_source_term, looks_like_standalone_symbol_query,
    query_mentions_non_primary_source,
};
#[cfg(test)]
pub(crate) use symbol_query::{is_non_primary_source_hit, mixed_natural_language_query};

type Storage = Store;
type GraphNodeId = codestory_contracts::graph::NodeId;
type WeightedGraphMatches = Vec<(GraphNodeId, f32)>;
type GraphNodeNameMap = HashMap<GraphNodeId, String>;
type ExpandedSymbolMatches = Option<(WeightedGraphMatches, GraphNodeNameMap)>;

#[derive(Clone)]
struct ActiveCoreRead {
    controller_identity: usize,
    storage: Rc<Storage>,
    publication: IndexPublicationRecord,
}

thread_local! {
    /// One complete core snapshot for the entire public operation. Every
    /// graph/source/target adapter opened through the controller borrows this
    /// same SQLite read transaction instead of opening a second generation.
    static ACTIVE_CORE_READ: RefCell<Option<ActiveCoreRead>> = const { RefCell::new(None) };
}

pub(crate) enum ReadStorage {
    Pinned(Rc<Storage>),
    Owned(Storage),
}

impl Deref for ReadStorage {
    type Target = Storage;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Pinned(storage) => storage,
            Self::Owned(storage) => storage,
        }
    }
}

struct ActiveCoreReadGuard {
    previous: Option<ActiveCoreRead>,
}

impl Drop for ActiveCoreReadGuard {
    fn drop(&mut self) {
        ACTIVE_CORE_READ.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

#[derive(Clone)]
pub struct Runtime {
    controller: AppController,
    activation: ActivationService,
    public_operation: PublicOperationService,
}

impl Runtime {
    pub fn new() -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::local())
    }

    pub fn new_with_config(config: codestory_retrieval::SidecarRuntimeConfig) -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::new(
            config,
            SourceIndexPolicy::default(),
        ))
    }

    pub fn new_with_process_config(config: RuntimeProcessConfig) -> Self {
        let controller = AppController::new_with_process_config(config);
        let activation = ActivationService::new(controller.clone());
        Self {
            activation: activation.clone(),
            public_operation: PublicOperationService::new_with_activation(
                controller.clone(),
                activation,
            ),
            controller,
        }
    }

    pub fn project_service(&self) -> ProjectService {
        ProjectService::new(self.controller.clone())
    }

    pub fn index_service(&self) -> IndexService {
        IndexService::new(self.controller.clone())
    }

    pub fn search_service(&self) -> SearchService {
        SearchService::new(self.controller.clone())
    }

    pub fn grounding_service(&self) -> GroundingService {
        GroundingService::new(self.controller.clone())
    }

    pub fn trail_service(&self) -> TrailService {
        TrailService::new(self.controller.clone())
    }

    pub fn agent_service(&self) -> AgentService {
        AgentService::new(self.controller.clone())
    }

    pub fn bookmark_service(&self) -> BookmarkService {
        BookmarkService::new(self.controller.clone())
    }

    pub fn browser_service(&self) -> ReadOnlyBrowserService {
        ReadOnlyBrowserService::new(self.controller.clone(), self.public_operation.clone())
    }

    pub fn activation_service(&self) -> ActivationService {
        self.activation.clone()
    }

    pub fn public_operation_service(&self) -> PublicOperationService {
        self.public_operation.clone()
    }

    pub fn events(&self) -> Receiver<AppEventPayload> {
        self.controller.events()
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

struct AppState {
    project_root: Option<PathBuf>,
    storage_path: Option<PathBuf>,
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    search_engine: Option<SearchEngine>,
    search_publication: Option<IndexPublicationRecord>,
    is_indexing: bool,
    index_freshness_cache: Option<CachedIndexFreshness>,
    #[cfg(test)]
    #[allow(dead_code)]
    last_hybrid_instrumentation: Option<HybridSearchInstrumentation>,
}

fn publish_search_engine(
    state: &mut AppState,
    engine: SearchEngine,
    publication: Option<IndexPublicationRecord>,
) {
    state.index_freshness_cache = None;
    state.search_engine = Some(engine);
    state.search_publication = publication;
}

fn clear_search_engine(state: &mut AppState) {
    state.search_engine = None;
    state.search_publication = None;
}

/// GUI-agnostic orchestrator for CodeStory.
///
/// This is intentionally "headless": any app shell (CLI, desktop, IDE integration)
/// should call methods on this controller and subscribe to `AppEventPayload`.
/// The controller also owns the per-runtime sidecar query cache, so callers should reuse a
/// controller for one open project but re-open state when project or storage identity changes.
#[derive(Clone)]
pub struct AppController {
    state: Arc<Mutex<AppState>>,
    sidecar_query_cache: Arc<Mutex<SidecarQueryCacheState>>,
    events_tx: Sender<AppEventPayload>,
    events_rx: Receiver<AppEventPayload>,
    runtime_config: Arc<codestory_retrieval::SidecarRuntimeConfig>,
    source_index_policy: Arc<SourceIndexPolicy>,
}

#[derive(Debug)]
pub(crate) struct SidecarQueryCacheState {
    generation: u64,
    cache: codestory_retrieval::RetrievalCache,
}

impl SidecarQueryCacheState {
    fn new() -> Self {
        Self {
            generation: 0,
            cache: codestory_retrieval::RetrievalCache::new(),
        }
    }

    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache.clear();
    }

    pub(crate) fn snapshot(&self) -> (u64, codestory_retrieval::RetrievalCache) {
        (self.generation, self.cache.clone())
    }

    pub(crate) fn merge_if_current(
        &mut self,
        generation: u64,
        baseline: &codestory_retrieval::RetrievalCache,
        cache: codestory_retrieval::RetrievalCache,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        self.cache.merge_delta_from(baseline, cache);
        true
    }

    #[cfg(test)]
    pub(crate) fn insert(
        &mut self,
        key: codestory_retrieval::RetrievalCacheKey,
        hits: Vec<codestory_retrieval::CandidateHit>,
    ) {
        self.cache.insert(key, hits);
    }

    #[cfg(test)]
    pub(crate) fn get(
        &self,
        key: &codestory_retrieval::RetrievalCacheKey,
    ) -> Option<&[codestory_retrieval::CandidateHit]> {
        self.cache.get(key)
    }
}

impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}
