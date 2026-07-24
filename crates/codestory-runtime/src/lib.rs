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
    AffectedUncoveredInputDto, AffectedUnmatchedPathDto, AgentAnswerDto, AgentAskRequest,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, ApiError, ApiErrorDetails,
    AppEventPayload, ArtifactCacheAccessTimings, ArtifactCachePolicyDto, BookmarkCategoryDto,
    BookmarkDto, CorePromotionTimings, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    DatabaseSnapshotCopyTimings, EdgeId, EdgeKind, EdgeOccurrencesRequest,
    EmbeddingProfileContractDto, FileCoverageDiagnosticDto, FrameworkRouteCoverageDto,
    FullRefreshWallTimings, GraphEdgeDto, GraphNodeDto, GraphRequest, GraphResponse,
    GroundingBudgetDto, GroundingCoverageBucketDto, GroundingFileDigestDto,
    GroundingOrientationConfidenceDto, GroundingOrientationDto, GroundingOrientationUncertaintyDto,
    GroundingSnapshotDto, GroundingSymbolDigestDto, IndexDryRunDto, IndexFreshnessChangeKindDto,
    IndexFreshnessDto, IndexFreshnessSampleDto, IndexFreshnessStatusDto, IndexMode,
    IndexPublicationDto, IndexPublicationModeDto, IndexedFileDto,
    IndexedFileIncompleteReasonCountDto, IndexedFileLanguageCountDto, IndexedFileRoleDto,
    IndexedFilesDto, IndexedFilesRequest, IndexedFilesSummaryDto, IndexingPhaseTimings,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, MemberAccess, NodeDetailsDto,
    NodeDetailsRequest, NodeId, NodeKind, NodeOccurrencesRequest, OpenContainingFolderRequest,
    OpenDefinitionRequest, OpenProjectRequest, ProjectSummary, ProjectionPersistenceFamilyTimings,
    ProjectionPersistenceTimings, ReadFileTextRequest, ReadFileTextResponse, RepoTextScanStatsDto,
    RetrievalFallbackReasonDto, RetrievalModeDto, RetrievalScoreBreakdownDto, RetrievalStateDto,
    RouteEndpointHandlerDto, RouteEndpointKindDto, RouteEndpointMetadataDto, SearchHit,
    SearchHitOrigin, SearchHybridLimitsDto, SearchMatchQualityDto, SearchPlanAnchorGroupDto,
    SearchPlanBridgeConfidenceDto, SearchPlanBridgeDto, SearchPlanBridgeEvidenceKindDto,
    SearchPlanBridgeStatusDto, SearchPlanCandidateWindowDto, SearchPlanChannelDto,
    SearchPlanDroppedTermDto, SearchPlanDto, SearchPlanNextActionDto, SearchPlanPromotionStatusDto,
    SearchPlanRejectedHitDto, SearchPlanSubqueryDto, SearchPlanTermsDto, SearchQueryAssessmentDto,
    SearchRepoTextMode, SearchRequest, SearchResultsDto, SemanticModeDto, SnippetContextDto,
    SourceOccurrenceDto, SourcePolicyExclusionDto, StartIndexingRequest, StorageStatsDto,
    StoredSemanticDocsContractDto, SummaryGenerationDto, SymbolContextDto, SymbolSummaryDto,
    SystemActionResponse, TrailConfigDto, TrailContextDto, TrailFilterOptionsDto,
    UpdateBookmarkCategoryRequest, UpdateBookmarkRequest, WorkspaceMemberIndexDto,
    WriteFileResponse, WriteFileTextRequest,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_contracts::graph::{
    AccessKind, Edge as GraphEdge, FileCoverageReason, Node as GraphNode,
};
use codestory_contracts::language_support::{
    LanguageSupportProfile, language_support_profile_for_ext,
    language_support_profile_for_language_name,
};
use codestory_indexer::{
    ArtifactCacheFamilyStats, ArtifactCachePolicies, ArtifactCachePolicy, CancellationToken,
    IncrementalIndexingStats, WorkspaceIndexer as V2WorkspaceIndexer,
};
#[cfg(test)]
use codestory_store::RetrievalIndexManifest;
use codestory_store::{
    BUILD_EDGE_SEED_BATCH_SIZE, CURRENT_SCHEMA_VERSION, DenseAnchorInput,
    DenseAnchorInputReuseMetadata, FileInfo, FileRole as StoreFileRole, GroundingEdgeKindCount,
    GroundingNodeRecord, IndexPublicationMode, IndexPublicationRecord, LlmSymbolDoc,
    LlmSymbolDocStats, SearchSymbolProjection, SnapshotStore, SourcePolicyExclusionPolicyIdentity,
    SourcePolicyExclusionRecord, StagedSnapshot, StagedSnapshotFinalizeStats,
    StagedSnapshotPublishStats, Store, StructuralTextPublicationCompatibility, SymbolSearchDoc,
    SymbolSummaryRecord,
};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
#[cfg(test)]
use codestory_workspace::{DEFAULT_SOURCE_FILE_BYTE_CAP, OVERSIZED_SOURCE_POLICY_VERSION};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, RefreshInputs, RefreshMode,
    SourceIndexPolicy, WorkspaceInventoryOutcome, WorkspaceManifest, WorkspacePathIdentity,
    project_identity_v3,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use fs4::fs_std::FileExt;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{self, BufRead};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod affected;
mod agent;
mod index_commit;
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
use index_freshness::{
    CachedIndexFreshness, index_freshness_cache_ttl_secs, index_freshness_from_storage_with_policy,
    index_freshness_observation_from_storage_with_identities, indexable_source_path_in_workspace,
    indexable_source_path_with_root, open_existing_storage_for_read, open_storage_for_read,
    storage_fingerprint, workspace_member_index_summaries, workspace_member_storage_summaries,
};
#[cfg(test)]
use index_freshness::{
    EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP, arm_after_index_freshness_fence_test_hook,
    index_freshness_from_storage, indexable_source_path, not_checked_index_freshness,
};
use index_full::*;
use index_incremental::*;
use index_timings::*;
#[cfg(test)]
use publication::{
    PUBLICATION_TEST_FAULT, PublicationTestAction, PublicationTestBoundary,
    arm_activation_search_before_revalidate_hook, arm_full_refresh_staged_store_hook,
    arm_incremental_staged_store_hook, arm_publication_test_fault,
    arm_semantic_projection_before_revalidate_hook, arm_source_policy_after_plan_hook,
    arm_source_policy_before_revalidate_hook, publication_test_checkpoint,
    run_activation_search_before_revalidate_hook, run_full_refresh_staged_store_hook,
    run_incremental_staged_store_hook, run_semantic_projection_before_revalidate_hook,
    run_source_policy_after_plan_hook, run_source_policy_before_revalidate_hook,
};
#[cfg(test)]
use route_coverage::compare_optional_confidence_desc;
use route_coverage::{
    RouteHandlerCandidate, compare_route_handler_candidates, framework_route_coverage_matrix,
    language_support_summary_for_language, route_endpoint_adjusted_search_score,
    route_endpoint_metadata_from_canonical, route_endpoint_metadata_from_openapi_label,
};
use search_intent::indexed_file_matches_language_filter;
use search_publication::{
    SearchGenerationCatalogGuard, discard_unpublished_search_generation,
    load_canonical_search_symbols, load_persisted_search_state_for_runtime,
    prune_search_generations, read_search_generation_completion,
    retrieval_state_from_storage_for_runtime, search_index_path_for_publication,
    write_search_generation_completion,
};
#[cfg(test)]
use search_publication::{
    SearchGenerationCompletion, llm_doc_embed_batch_size, load_persisted_search_state,
    search_generation_completion_path, search_index_generation_root, search_index_storage_path,
};
#[cfg(test)]
use search_scoring::HybridSearchInstrumentation;
pub(crate) use search_scoring::HybridSearchScoredHit;
use search_state_cache::*;
pub use semantic_projection::SemanticProjectionRepublishOutcome;
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use semantic_projection::stored_semantic_embeddings_for_test;
use semantic_projection::{
    CacheRefreshStats, ComponentReportRefreshScope, LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION,
    SEARCH_SYMBOL_STREAM_BATCH_SIZE, SEMANTIC_POLICY_VERSION, SearchStateBuildResult,
    SearchStateBuildStats, SemanticProjectionDocumentSource,
    SemanticProjectionSourcePolicyCompatibility, SemanticProjectionStats,
    apply_cache_refresh_stats, apply_semantic_projection_stats, edge_digest_for_node,
    finalize_staged_semantic_docs_for_runtime, load_persisted_semantic_docs_for_runtime,
    semantic_component_key_for_path, semantic_file_table_path_map,
    semantic_graph_dependent_file_ids_by_seed, semantic_projection_source_policy_compatibility,
    summarize_symbol_doc,
};
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
use semantic_republish::*;
#[cfg(test)]
pub(crate) use snippets::markdown_snippet;
pub(crate) use snippets::{
    BoundedSnippet, BoundedSnippetRangeOptions, DIRECT_SNIPPET_MAX_BYTES,
    DIRECT_SNIPPET_TRUNCATION_SUFFIX,
};
use snippets::{bounded_markdown_snippet_from_path, bounded_markdown_snippet_range_from_path};
use workspace_state::runtime_workspace_manifest;

mod browser;
mod cache_rehydrate;
mod controller_bookmarks;
mod controller_indexing;
pub mod graph_analysis;
mod graph_builders;
mod graph_canonical;
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

fn no_project_error() -> ApiError {
    ApiError::invalid_argument("No project open. Call open_project first.")
}

fn parse_db_id(raw: &str, field_name: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid_argument(format!("Invalid {field_name}: {raw}")))
}

fn edge_certainty_label(
    kind: codestory_contracts::graph::EdgeKind,
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
    confidence: Option<f32>,
) -> Option<String> {
    certainty
        .or_else(|| codestory_contracts::graph::ResolutionCertainty::from_confidence(confidence))
        .or_else(|| structural_edge_default_certainty(kind))
        .map(|value| value.as_str().to_string())
}

fn structural_edge_default_certainty(
    kind: codestory_contracts::graph::EdgeKind,
) -> Option<codestory_contracts::graph::ResolutionCertainty> {
    use codestory_contracts::graph::{EdgeKind, ResolutionCertainty};

    match kind {
        EdgeKind::MEMBER
        | EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION
        | EdgeKind::INCLUDE
        | EdgeKind::IMPORT => Some(ResolutionCertainty::Certain),
        EdgeKind::CALL
        | EdgeKind::USAGE
        | EdgeKind::TYPE_USAGE
        | EdgeKind::MACRO_USAGE
        | EdgeKind::ANNOTATION_USAGE
        | EdgeKind::UNKNOWN => None,
    }
}

fn is_structural_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::CLASS
            | codestory_contracts::graph::NodeKind::STRUCT
            | codestory_contracts::graph::NodeKind::INTERFACE
            | codestory_contracts::graph::NodeKind::UNION
            | codestory_contracts::graph::NodeKind::ENUM
            | codestory_contracts::graph::NodeKind::NAMESPACE
            | codestory_contracts::graph::NodeKind::MODULE
    )
}

fn member_access_dto(
    access: Option<codestory_contracts::graph::AccessKind>,
) -> Option<MemberAccess> {
    access.map(MemberAccess::from)
}

fn status_response(message: impl Into<String>) -> SystemActionResponse {
    SystemActionResponse {
        ok: true,
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy)]
struct AppGraphFeatureFlags {
    include_edge_certainty: bool,
    include_callsite_identity: bool,
    include_candidate_targets: bool,
}

impl AppGraphFeatureFlags {
    fn from_env() -> Self {
        Self {
            include_edge_certainty: env_flag("CODESTORY_GRAPH_INCLUDE_EDGE_CERTAINTY", true),
            include_callsite_identity: env_flag("CODESTORY_GRAPH_INCLUDE_CALLSITE_IDENTITY", true),
            include_candidate_targets: env_flag("CODESTORY_GRAPH_INCLUDE_CANDIDATE_TARGETS", true),
        }
    }
}

fn app_graph_flags() -> AppGraphFeatureFlags {
    static FLAGS: OnceLock<AppGraphFeatureFlags> = OnceLock::new();
    *FLAGS.get_or_init(AppGraphFeatureFlags::from_env)
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
}

fn graph_edge_dto(
    edge: codestory_contracts::graph::Edge,
    flags: AppGraphFeatureFlags,
) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId::from(edge.id),
        source: NodeId::from(edge.source),
        target: NodeId::from(edge.target),
        kind: EdgeKind::from(edge.kind),
        confidence: edge.confidence,
        certainty: if flags.include_edge_certainty {
            edge_certainty_label(edge.kind, edge.certainty, edge.confidence)
        } else {
            None
        },
        callsite_identity: if flags.include_callsite_identity {
            edge.callsite_identity.clone()
        } else {
            None
        },
        candidate_targets: if flags.include_candidate_targets {
            edge.candidate_targets
                .iter()
                .copied()
                .map(NodeId::from)
                .collect()
        } else {
            Vec::new()
        },
    }
}

fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn runtime_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_path_key(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn indexed_file_role(path: &Path) -> IndexedFileRoleDto {
    path_role_from_key(&normalize_path_key(&path.to_string_lossy()))
}

fn file_coverage_reason(
    file: &FileInfo,
    errors_by_file: &HashMap<i64, Vec<FileCoverageReason>>,
    has_verified_content: bool,
) -> Option<FileCoverageReason> {
    if file.complete {
        return None;
    }
    if let Some(reason) = errors_by_file
        .get(&file.id)
        .and_then(|reasons| reasons.first())
    {
        return Some(*reason);
    }
    if !file.complete && file.indexed && has_verified_content {
        Some(FileCoverageReason::ParserPartial)
    } else {
        Some(FileCoverageReason::CollectorFailure)
    }
}

fn file_coverage_retryable(reason: FileCoverageReason) -> bool {
    matches!(
        reason,
        FileCoverageReason::SourceChanged
            | FileCoverageReason::DiscoveryIncomplete
            | FileCoverageReason::CollectorFailure
    )
}

fn file_coverage_detail(reason: FileCoverageReason) -> &'static str {
    match reason {
        FileCoverageReason::ParserPartial => {
            "stable verified source published with partial parser coverage"
        }
        FileCoverageReason::SourceChanged => "source changed while its projection was collected",
        FileCoverageReason::Unreadable => "source bytes could not be read and verified",
        FileCoverageReason::Malformed => {
            "verified UTF-8 source is malformed for its structural format"
        }
        FileCoverageReason::Binary => "source is binary or is not valid UTF-8",
        FileCoverageReason::Oversized => "source exceeds the configured indexing size limit",
        FileCoverageReason::DiscoveryIncomplete => {
            "workspace discovery could not prove a complete source inventory"
        }
        FileCoverageReason::CollectorFailure => {
            "a source collector or projection write failed before verification completed"
        }
    }
}

fn full_refresh_execution_plan_with_coverage(
    root: &Path,
    workspace: &WorkspaceManifest,
    policy: &SourceIndexPolicy,
) -> Result<(RefreshExecutionPlan, Vec<OversizedSourceExclusionCandidate>), ApiError> {
    let inventory = workspace
        .source_inventory_with_policy(policy)
        .map_err(|error| {
            ApiError::source_coverage_failure(
                "source_collector_failure",
                format!("Failed to collect the full source inventory: {error}"),
                vec![FileCoverageDiagnosticDto {
                    path: ".".to_string(),
                    reason: FileCoverageReason::CollectorFailure,
                    retryable: true,
                    verified_source: false,
                    projection_available: false,
                }],
            )
        })?;
    if inventory.outcome != WorkspaceInventoryOutcome::Complete {
        let reason = if inventory.outcome == WorkspaceInventoryOutcome::Unreadable {
            FileCoverageReason::Unreadable
        } else {
            FileCoverageReason::DiscoveryIncomplete
        };
        let mut coverage_gaps = inventory
            .issues
            .iter()
            .map(|issue| FileCoverageDiagnosticDto {
                path: runtime_relative_path(root, &issue.path),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source: false,
                projection_available: false,
            })
            .collect::<Vec<_>>();
        if coverage_gaps.is_empty() {
            coverage_gaps.push(FileCoverageDiagnosticDto {
                path: ".".to_string(),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source: false,
                projection_available: false,
            });
        }
        return Err(ApiError::source_coverage_failure(
            match reason {
                FileCoverageReason::Unreadable => "source_unreadable",
                _ => "source_discovery_incomplete",
            },
            format!(
                "Effective refresh mode `full` requires a complete source inventory; discovery was {:?}.",
                inventory.outcome
            ),
            coverage_gaps,
        ));
    }
    Ok((
        RefreshExecutionPlan {
            mode: RefreshMode::FullRefresh,
            files_to_index: inventory.files,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        inventory.policy_exclusions,
    ))
}

fn publish_source_policy_exclusions(
    storage: &mut Store,
    root: &Path,
    publication: &IndexPublicationRecord,
    exclusions: &[OversizedSourceExclusionCandidate],
    policy: &SourceIndexPolicy,
) -> Result<(), ApiError> {
    let identity = project_identity_v3(root);
    storage
        .publish_source_policy_exclusion_generation(
            publication,
            &identity.project_id,
            &identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &policy.policy_version,
                policy.byte_cap,
                policy.structural_unit_cap,
            ),
            exclusions,
        )
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish complete source policy exclusions: {error}"
            ))
        })?;
    Ok(())
}

fn source_policy_exclusion_candidate(
    record: &SourcePolicyExclusionRecord,
) -> OversizedSourceExclusionCandidate {
    OversizedSourceExclusionCandidate {
        normalized_path: record.normalized_path.clone(),
        content_hash: record.content_hash.clone(),
        observed_size: record.observed_size,
        observed_unit_count: record.observed_unit_count,
        policy_version: record.policy_version.clone(),
        byte_cap: record.byte_cap,
        structural_unit_cap: record.structural_unit_cap,
    }
}

fn revalidate_source_policy_exclusions(
    workspace: &WorkspaceManifest,
    exclusions: &[OversizedSourceExclusionCandidate],
    policy: &SourceIndexPolicy,
) -> Result<Vec<OversizedSourceExclusionCandidate>, ApiError> {
    workspace
        .revalidate_source_policy_exclusions(exclusions, policy)
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!(
                    "Source policy exclusions changed before publication; the candidate core was discarded: {error}"
                ),
            )
        })
}

fn validate_source_policy_exclusions(
    storage: &Store,
    root: &Path,
    publication: &IndexPublicationRecord,
    policy: &SourceIndexPolicy,
) -> Result<(), ApiError> {
    let identity = project_identity_v3(root);
    storage
        .validate_source_policy_exclusion_publication(
            publication,
            &identity.project_id,
            &identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &policy.policy_version,
                policy.byte_cap,
                policy.structural_unit_cap,
            ),
        )
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!("Source policy exclusion publication is incomplete or stale: {error}"),
            )
        })?;
    Ok(())
}

fn validate_structural_text_units(
    storage: &Store,
    publication: &IndexPublicationRecord,
) -> Result<(), ApiError> {
    storage
        .validate_structural_text_unit_publication(publication)
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!("Structural text unit publication is incomplete or stale: {error}"),
            )
        })?;
    Ok(())
}

fn stored_file_coverage_diagnostics(
    root: &Path,
    storage: &Store,
) -> Result<Vec<FileCoverageDiagnosticDto>, ApiError> {
    let files = storage.get_files().map_err(|error| {
        ApiError::internal(format!("Failed to load staged file coverage: {error}"))
    })?;
    let verified_file_ids = storage
        .files()
        .inventory()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load staged verified source identities: {error}"
            ))
        })?
        .into_iter()
        .filter_map(|file| file.content_hash.map(|_| file.id))
        .collect::<HashSet<_>>();
    let structural_projection_file_ids = storage
        .get_structural_text_projection_file_ids()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load staged structural projection identities: {error}"
            ))
        })?
        .into_iter()
        .collect::<HashSet<_>>();
    let mut dedicated_openapi_projection_file_ids = HashSet::new();
    for file in &files {
        if file.language == "openapi"
            && verified_file_ids.contains(&file.id)
            && storage
                .has_file_owned_openapi_endpoint_projection(file.id)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to verify staged OpenAPI projection identity for {}: {error}",
                        runtime_relative_path(root, &file.path)
                    ))
                })?
        {
            dedicated_openapi_projection_file_ids.insert(file.id);
        }
    }
    let mut errors_by_file = HashMap::<i64, Vec<FileCoverageReason>>::new();
    for error in storage.get_errors(None).map_err(|error| {
        ApiError::internal(format!("Failed to load staged file errors: {error}"))
    })? {
        if let Some(file_id) = error.file_id {
            errors_by_file.entry(file_id.0).or_default().push(
                error
                    .coverage_reason
                    .unwrap_or(FileCoverageReason::CollectorFailure),
            );
        }
    }
    Ok(files
        .iter()
        .filter_map(|file| {
            let verified_source = verified_file_ids.contains(&file.id);
            let dedicated_openapi_source = file.language == "openapi"
                && verified_source
                && dedicated_openapi_projection_file_ids.contains(&file.id);
            let structural_projection_verified = dedicated_openapi_source
                || !codestory_indexer::structural::is_structural_candidate_path(&file.path)
                || (verified_source && structural_projection_file_ids.contains(&file.id));
            let reason = if file.complete && !structural_projection_verified {
                Some(FileCoverageReason::CollectorFailure)
            } else {
                file_coverage_reason(file, &errors_by_file, verified_source)
            };
            reason.map(|reason| FileCoverageDiagnosticDto {
                path: runtime_relative_path(root, &file.path),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source,
                projection_available: file.indexed
                    && verified_source
                    && structural_projection_verified,
            })
        })
        .collect())
}

fn source_coverage_failure_code(coverage_gaps: &[FileCoverageDiagnosticDto]) -> &'static str {
    let Some(first) = coverage_gaps.first().map(|entry| entry.reason) else {
        return "source_verification_failed";
    };
    if coverage_gaps.iter().any(|entry| entry.reason != first) {
        return "source_verification_failed";
    }
    match first {
        FileCoverageReason::ParserPartial => "source_verification_failed",
        FileCoverageReason::SourceChanged => "source_changed",
        FileCoverageReason::Unreadable => "source_unreadable",
        FileCoverageReason::Malformed => "source_malformed",
        FileCoverageReason::Binary => "source_binary",
        FileCoverageReason::Oversized => "source_oversized",
        FileCoverageReason::DiscoveryIncomplete => "source_discovery_incomplete",
        FileCoverageReason::CollectorFailure => "source_collector_failure",
    }
}

fn path_role_from_key(path: &str) -> IndexedFileRoleDto {
    match retrieval_file_role_from_path(path) {
        RetrievalFileRole::Test => IndexedFileRoleDto::Test,
        RetrievalFileRole::Generated => IndexedFileRoleDto::Generated,
        RetrievalFileRole::Vendor => IndexedFileRoleDto::Vendor,
        RetrievalFileRole::Source | RetrievalFileRole::Docs | RetrievalFileRole::Benchmark => {
            IndexedFileRoleDto::Source
        }
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

impl AppController {
    pub fn new() -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::local())
    }

    pub fn new_with_config(config: codestory_retrieval::SidecarRuntimeConfig) -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::new(
            config,
            SourceIndexPolicy::default(),
        ))
    }

    fn new_with_process_config(config: RuntimeProcessConfig) -> Self {
        Self::new_with_source_index_policy(config.sidecar, config.source_index_policy)
    }

    fn new_with_source_index_policy(
        config: codestory_retrieval::SidecarRuntimeConfig,
        source_index_policy: SourceIndexPolicy,
    ) -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            state: Arc::new(Mutex::new(AppState {
                project_root: None,
                storage_path: None,
                node_names: HashMap::new(),
                search_engine: None,
                search_publication: None,
                is_indexing: false,
                index_freshness_cache: None,
                #[cfg(test)]
                last_hybrid_instrumentation: None,
            })),
            sidecar_query_cache: Arc::new(Mutex::new(SidecarQueryCacheState::new())),
            events_tx,
            events_rx,
            runtime_config: Arc::new(config),
            source_index_policy: Arc::new(source_index_policy),
        }
    }

    pub fn project_service(&self) -> ProjectService {
        ProjectService::new(self.clone())
    }

    pub fn search_service(&self) -> SearchService {
        SearchService::new(self.clone())
    }

    pub fn grounding_service(&self) -> GroundingService {
        GroundingService::new(self.clone())
    }

    pub fn index_service(&self) -> IndexService {
        IndexService::new(self.clone())
    }

    pub fn trail_service(&self) -> TrailService {
        TrailService::new(self.clone())
    }

    pub fn agent_service(&self) -> AgentService {
        AgentService::new(self.clone())
    }

    pub fn bookmark_service(&self) -> BookmarkService {
        BookmarkService::new(self.clone())
    }

    pub fn browser_service(&self) -> ReadOnlyBrowserService {
        ReadOnlyBrowserService::new(self.clone(), PublicOperationService::new(self.clone()))
    }

    /// Subscribe to backend events. Intended to be consumed by a single pump
    /// that forwards to the active runtime.
    pub fn events(&self) -> Receiver<AppEventPayload> {
        self.events_rx.clone()
    }

    fn require_project_root(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .project_root
            .clone()
            .ok_or_else(no_project_error)
    }

    fn require_storage_path(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .storage_path
            .clone()
            .ok_or_else(no_project_error)
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.state) as usize
    }

    pub(crate) fn open_storage(&self) -> Result<Storage, ApiError> {
        let storage_path = self.require_storage_path()?;
        Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))
    }

    pub(crate) fn open_storage_read_only(&self) -> Result<ReadStorage, ApiError> {
        if let Some(storage) = ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| Rc::clone(&active.storage))
        }) {
            return Ok(ReadStorage::Pinned(storage));
        }
        let storage_path = self.require_storage_path()?;
        open_existing_storage_for_read(&storage_path).map(ReadStorage::Owned)
    }

    fn open_storage_for_freshness(&self) -> Result<ReadStorage, ApiError> {
        if let Some(storage) = ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| Rc::clone(&active.storage))
        }) {
            return Ok(ReadStorage::Pinned(storage));
        }
        let storage_path = self.require_storage_path()?;
        Storage::open_freshness_observational(&storage_path)
            .map(ReadStorage::Owned)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to open storage for freshness observation: {error}"
                ))
            })
    }

    fn active_core_publication(&self) -> Option<IndexPublicationRecord> {
        ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| active.publication.clone())
        })
    }

    fn active_project_summary(&self) -> Result<ProjectSummary, ApiError> {
        if self.active_core_publication().is_none() {
            return Err(ApiError::internal(
                "Active project summary requires a pinned public operation",
            ));
        }
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        let storage = self.open_storage_read_only()?;
        self.project_summary_from_storage(&root, &storage_path, &storage)
    }

    fn with_complete_core_snapshot<T>(
        &self,
        build: impl FnOnce(&IndexPublicationRecord) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        if let Some(publication) = self.active_core_publication() {
            return build(&publication);
        }
        let storage_path = self.require_storage_path()?;
        let storage = Rc::new(open_existing_storage_for_read(&storage_path)?);
        let installed_storage = Rc::clone(&storage);
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!(
                "Failed to begin public operation snapshot: {error}"
            ))
        })?;
        let publication = snapshot
            .storage()
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read public operation publication: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "project_unavailable",
                    "no complete core publication is available",
                )
            })?;
        let previous = ACTIVE_CORE_READ.with(|active| {
            active.replace(Some(ActiveCoreRead {
                controller_identity: self.identity(),
                storage: installed_storage,
                publication: publication.clone(),
            }))
        });
        let guard = ActiveCoreReadGuard { previous };
        let result = build(&publication);
        drop(guard);
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish public operation snapshot: {error}"
            ))
        })?;
        let live = Store::database_complete_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!("Failed to revalidate public operation: {error}"))
        })?;
        if live.as_ref() != Some(&publication) {
            return Err(ApiError::new(
                "publication_changed",
                "the complete core publication changed during the public operation",
            ));
        }
        result
    }

    fn index_freshness_uncached(&self) -> Result<IndexFreshnessDto, ApiError> {
        let root = self.require_project_root()?;
        let storage = self.open_storage_for_freshness()?;
        let storage_path = self.require_storage_path()?;
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
        Ok(index_freshness_from_storage_with_policy(
            &root,
            &workspace,
            &storage,
            &self.source_index_policy,
        ))
    }

    fn resolve_project_file_path(
        &self,
        path: &str,
        allow_missing_leaf: bool,
    ) -> Result<PathBuf, ApiError> {
        path_resolution::resolve_project_file_path(self, path, allow_missing_leaf)
    }

    fn open_folder_in_os(path: &Path) -> io::Result<()> {
        system_actions::open_folder_in_os(path)
    }

    fn launch_definition_in_ide(
        &self,
        path: &Path,
        line: Option<u32>,
        col: Option<u32>,
    ) -> Result<SystemActionResponse, ApiError> {
        system_actions::launch_definition_in_ide(path, line, col)
    }

    fn cached_labels<I>(&self, ids: I) -> HashMap<codestory_contracts::graph::NodeId, String>
    where
        I: IntoIterator<Item = codestory_contracts::graph::NodeId>,
    {
        let s = self.state.lock();
        ids.into_iter()
            .filter_map(|id| s.node_names.get(&id).cloned().map(|name| (id, name)))
            .collect()
    }

    fn clear_search_state(&self) {
        let mut s = self.state.lock();
        s.node_names.clear();
        clear_search_engine(&mut s);
        self.sidecar_query_cache.lock().clear();
    }

    fn ensure_consistent_read_state(&self, operation: &str) -> Result<(), ApiError> {
        if self.state.lock().is_indexing {
            return Err(ApiError::invalid_argument(format!(
                "{operation} is unavailable while indexing is in progress. Retry after indexing completes."
            )));
        }
        Ok(())
    }

    fn ensure_search_state(&self) -> Result<(), ApiError> {
        let pinned_publication = self.active_core_publication();
        if let Some(publication) = pinned_publication.as_ref() {
            let state = self.state.lock();
            if state.search_engine.is_some()
                && state.search_publication.as_ref() == Some(publication)
            {
                return Ok(());
            }
        }
        let storage_path = self.require_storage_path()?;
        let current_publication = Store::database_complete_index_publication(&storage_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read current search publication: {error}"
                ))
            })?;
        if pinned_publication.as_ref() != current_publication.as_ref()
            && pinned_publication.is_some()
        {
            return Err(ApiError::new(
                "publication_changed",
                "the pinned core publication is no longer the current lexical search generation",
            ));
        }
        {
            let s = self.state.lock();
            if s.search_engine.is_some() && s.search_publication == current_publication {
                return Ok(());
            }
        }

        let mut attempts = 0;
        let loaded = loop {
            let mut storage = open_storage_for_read(&storage_path)?;
            match load_persisted_search_state_for_runtime(
                &mut storage,
                &storage_path,
                &self.runtime_config,
            ) {
                Ok(state) => break state,
                Err(error) if error.code == "cache_busy" && attempts == 0 => attempts += 1,
                Err(error) => return Err(error),
            }
        };
        if pinned_publication.as_ref() != loaded.publication.as_ref()
            && pinned_publication.is_some()
        {
            return Err(ApiError::new(
                "publication_changed",
                "the pinned core publication does not match the loaded lexical search generation",
            ));
        }

        let mut s = self.state.lock();
        if s.search_engine.is_none() || s.search_publication != loaded.publication {
            s.node_names = loaded.node_names;
            publish_search_engine(&mut s, loaded.engine, loaded.publication);
        }

        Ok(())
    }

    pub fn retrieval_state(&self) -> Result<RetrievalStateDto, ApiError> {
        let storage = self.open_storage_read_only()?;
        retrieval_state_from_storage_for_runtime(&storage, &self.runtime_config)
    }

    pub(crate) fn file_path_for_node(
        storage: &Storage,
        node: &codestory_contracts::graph::Node,
    ) -> Result<Option<String>, ApiError> {
        let Some(file_id) = node.file_node_id else {
            return Ok(None);
        };

        let file_node = storage
            .get_node(file_id)
            .map_err(|e| ApiError::internal(format!("Failed to load file node: {e}")))?;

        Ok(file_node.map(|file| file.serialized_name))
    }

    fn occurrence_kind_label(kind: codestory_contracts::graph::OccurrenceKind) -> &'static str {
        match kind {
            codestory_contracts::graph::OccurrenceKind::DEFINITION => "definition",
            codestory_contracts::graph::OccurrenceKind::REFERENCE => "reference",
            codestory_contracts::graph::OccurrenceKind::DECLARATION => "declaration",
            codestory_contracts::graph::OccurrenceKind::MACRO_DEFINITION => "macro_definition",
            codestory_contracts::graph::OccurrenceKind::MACRO_REFERENCE => "macro_reference",
            codestory_contracts::graph::OccurrenceKind::UNKNOWN => "unknown",
        }
    }

    fn to_source_occurrence_dto(
        storage: &Storage,
        occurrence: codestory_contracts::graph::Occurrence,
    ) -> Result<Option<SourceOccurrenceDto>, ApiError> {
        let file_node = storage
            .get_node(occurrence.location.file_node_id)
            .map_err(|e| {
                ApiError::internal(format!("Failed to resolve occurrence file node: {e}"))
            })?;
        let Some(file_node) = file_node else {
            return Ok(None);
        };

        Ok(Some(SourceOccurrenceDto {
            element_id: occurrence.element_id.to_string(),
            kind: Self::occurrence_kind_label(occurrence.kind).to_string(),
            file_path: file_node.serialized_name,
            start_line: occurrence.location.start_line,
            start_col: occurrence.location.start_col,
            end_line: occurrence.location.end_line,
            end_col: occurrence.location.end_col,
        }))
    }

    fn symbol_summary_for_node(
        storage: &Storage,
        labels_by_id: &HashMap<codestory_contracts::graph::NodeId, String>,
        node: codestory_contracts::graph::Node,
    ) -> Result<SymbolSummaryDto, ApiError> {
        let has_children = !storage
            .get_children_symbols(node.id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?
            .is_empty();

        let label = labels_by_id
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| node_display_name(&node));

        Ok(SymbolSummaryDto {
            id: NodeId::from(node.id),
            label,
            kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(storage, &node)?,
            has_children,
        })
    }

    fn dedupe_symbol_nodes(
        nodes: Vec<codestory_contracts::graph::Node>,
        labels_by_id: &HashMap<codestory_contracts::graph::NodeId, String>,
    ) -> Vec<codestory_contracts::graph::Node> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(nodes.len());

        for node in nodes {
            let label = labels_by_id
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| node_display_name(&node));
            let key = (node.kind as i32, label, node.file_node_id);
            if seen.insert(key) {
                deduped.push(node);
            }
        }

        deduped
    }

    /// Resolve DB/index-backed symbol candidates for read commands.
    ///
    /// This intentionally bypasses mandatory sidecar product search so symbol,
    /// snippet, trail, and graph-query target resolution can work from an
    /// already-open indexed store. Product search and packet evidence must use
    /// the sidecar-primary search paths instead.
    pub fn resolve_indexed_symbol_candidates(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;
        let (matches, node_names) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            (
                engine.search_symbol_with_scores(query),
                s.node_names.clone(),
            )
        };

        let mut hits = matches
            .into_iter()
            .map(|(id, score)| Self::build_search_hit(&storage, &node_names, id, score))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let project_root = self.require_project_root().ok();
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root.as_deref(), query, left, right)
        });
        hits.truncate(max_results.clamp(1, 50));
        Ok(hits)
    }

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;

        let mut roots = storage
            .get_root_symbols()
            .map_err(|e| ApiError::internal(format!("Failed to load root symbols: {e}")))?;
        roots.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(roots.iter().map(|node| node.id));
        roots = Self::dedupe_symbol_nodes(roots, &labels_by_id);

        let limit = req.limit.unwrap_or(300).clamp(1, 2_000) as usize;
        if roots.len() > limit {
            roots.truncate(limit);
        }

        roots
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.ensure_search_state()?;
        let parent_id = req.parent_id.to_core()?;
        let storage = self.open_storage_read_only()?;

        let mut children = storage
            .get_children_symbols(parent_id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?;
        children.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(children.iter().map(|node| node.id));
        children = Self::dedupe_symbol_nodes(children, &labels_by_id);
        children
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    /// Build an answer from indexed source and sidecar-primary retrieval.
    ///
    /// Degraded sidecar state is reported through retrieval diagnostics or an error rather than
    /// silently substituting legacy search as answer-quality proof.
    pub fn agent_ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "agent answer", || {
            agent::agent_ask(self, req.clone())
        })
    }

    pub fn begin_packet_retrieval(&self) {
        let _ = self;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn take_hybrid_instrumentation(&self) -> Option<HybridSearchInstrumentation> {
        self.state.lock().last_hybrid_instrumentation.take()
    }

    /// Build an evidence packet with sufficiency, diagnostics, and budget metadata.
    ///
    /// Packet sufficiency is a runtime judgment over resolved evidence. Full-mode sidecar
    /// candidates that fail symbol resolution remain diagnostics and do not become supported
    /// claims merely because retrieval returned them.
    pub fn agent_packet(&self, req: AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "packet output", || {
            agent::agent_packet(self, req.clone())
        })
    }

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_neighborhood(self, req)
    }

    pub fn graph_trail(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_trail(self, req)
    }

    pub fn graph_direct_references(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_direct_references(self, req)
    }

    pub fn graph_trail_filter_options(&self) -> Result<TrailFilterOptionsDto, ApiError> {
        let storage = self.open_storage_read_only()?;
        let node_kinds = storage
            .get_present_node_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load node kinds: {e}")))?
            .into_iter()
            .map(NodeKind::from)
            .collect::<Vec<_>>();
        let edge_kinds = storage
            .get_present_edge_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load edge kinds: {e}")))?
            .into_iter()
            .map(EdgeKind::from)
            .collect::<Vec<_>>();
        Ok(TrailFilterOptionsDto {
            node_kinds,
            edge_kinds,
        })
    }

    pub fn open_definition(
        &self,
        req: OpenDefinitionRequest,
    ) -> Result<SystemActionResponse, ApiError> {
        let node_id = req.node_id.to_core()?;
        let storage = self.open_storage_read_only()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;

        let raw_path = if node.kind == codestory_contracts::graph::NodeKind::FILE {
            Some(node.serialized_name.clone())
        } else {
            Self::file_path_for_node(&storage, &node)?
        }
        .ok_or_else(|| ApiError::invalid_argument("Node has no file path for definition open."))?;

        let resolved = self.resolve_project_file_path(&raw_path, false)?;
        self.launch_definition_in_ide(&resolved, node.start_line, node.start_col)
    }

    pub fn open_containing_folder(
        &self,
        req: OpenContainingFolderRequest,
    ) -> Result<SystemActionResponse, ApiError> {
        let resolved = self.resolve_project_file_path(&req.path, false)?;
        Self::open_folder_in_os(&resolved).map_err(|e| {
            ApiError::internal(format!(
                "Failed to open containing folder for {}: {e}",
                resolved.display()
            ))
        })?;
        Ok(status_response(format!(
            "Opened containing folder for {}",
            resolved.display()
        )))
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        let id = req.id.to_core()?;

        let storage = self.open_storage_read_only()?;

        let node = storage
            .get_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to query node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {id}")))?;

        let display_name = self
            .state
            .lock()
            .node_names
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| {
                node.qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone())
            });

        let file_path = match node.file_node_id {
            Some(file_id) => match storage.get_node(file_id) {
                Ok(Some(file_node)) => Some(file_node.serialized_name),
                _ => None,
            },
            None => None,
        };

        let route_endpoint =
            self.route_endpoint_metadata(&storage, &node, file_path.as_deref(), &display_name);
        let structural_unit = storage.get_structural_text_unit(node.id).map_err(|error| {
            ApiError::internal(format!(
                "Failed to query structural evidence metadata: {error}"
            ))
        })?;
        let openapi_endpoint = node
            .canonical_id
            .as_deref()
            .is_some_and(|value| value.starts_with("openapi:endpoint:"));

        Ok(NodeDetailsDto {
            id: NodeId::from(node.id),
            kind: NodeKind::from(node.kind),
            display_name,
            serialized_name: node.serialized_name,
            qualified_name: node.qualified_name,
            canonical_id: node.canonical_id,
            file_path,
            start_line: node.start_line,
            start_col: node.start_col,
            end_line: node.end_line,
            end_col: node.end_col,
            evidence_tier: structural_unit
                .as_ref()
                .map(|_| codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
                .or_else(|| {
                    openapi_endpoint
                        .then_some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
                }),
            evidence_producer: structural_unit
                .as_ref()
                .map(|unit| unit.producer.clone())
                .or_else(|| openapi_endpoint.then(|| "openapi_endpoint_schema".to_string())),
            resolution_status: (structural_unit.is_some() || openapi_endpoint)
                .then_some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly),
            member_access: member_access_dto(storage.get_component_access(node.id).ok().flatten()),
            route_endpoint,
        })
    }

    fn route_endpoint_metadata(
        &self,
        storage: &Storage,
        node: &GraphNode,
        source_file: Option<&str>,
        display_name: &str,
    ) -> Option<RouteEndpointMetadataDto> {
        let canonical_id = node.canonical_id.as_deref()?;
        let mut metadata = if let Some(raw) = canonical_id.strip_prefix("route_endpoint:") {
            route_endpoint_metadata_from_canonical(raw, node, source_file).ok()?
        } else {
            let label = canonical_id.strip_prefix("openapi:endpoint:")?;
            route_endpoint_metadata_from_openapi_label(label, node, source_file)?
        };

        if metadata.handler.is_none() {
            metadata.handler = self.route_endpoint_handler(storage, node);
        }
        if metadata.handler.is_some()
            && !metadata
                .provenance
                .iter()
                .any(|entry| entry == "graph:handler_edge")
        {
            metadata.provenance.push("graph:handler_edge".to_string());
        }
        if metadata.source_file.is_none() {
            metadata.source_file = source_file.map(ToOwned::to_owned);
        }
        if metadata.line.is_none() {
            metadata.line = node.start_line;
        }
        if metadata.provenance.is_empty() {
            metadata.provenance.push(display_name.to_string());
        }
        Some(metadata)
    }

    fn route_endpoint_handler(
        &self,
        storage: &Storage,
        route_node: &GraphNode,
    ) -> Option<RouteEndpointHandlerDto> {
        let edges = storage.get_edges().ok()?;
        let mut candidates = edges
            .into_iter()
            .filter(|edge| {
                edge.kind == codestory_contracts::graph::EdgeKind::CALL
                    && edge.effective_source() == route_node.id
            })
            .filter_map(|edge| {
                let target = storage.get_node(edge.effective_target()).ok().flatten()?;
                let terminal = target
                    .qualified_name
                    .as_deref()
                    .unwrap_or(&target.serialized_name)
                    .rsplit([':', '.', '#'])
                    .next()
                    .unwrap_or(&target.serialized_name)
                    .to_ascii_lowercase();
                if matches!(
                    terminal.as_str(),
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "route"
                ) {
                    return None;
                }
                Some(RouteHandlerCandidate { edge, target })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(compare_route_handler_candidates);
        let RouteHandlerCandidate { edge, target } = candidates.into_iter().next()?;
        let display_name = self
            .state
            .lock()
            .node_names
            .get(&target.id)
            .cloned()
            .unwrap_or_else(|| {
                target
                    .qualified_name
                    .clone()
                    .unwrap_or_else(|| target.serialized_name.clone())
            });
        let file_path = target.file_node_id.and_then(|file_id| {
            storage
                .get_node(file_id)
                .ok()
                .flatten()
                .map(|file_node| file_node.serialized_name)
        });
        Some(RouteEndpointHandlerDto {
            node_id: NodeId::from(target.id),
            display_name,
            file_path,
            line: target.start_line,
            certainty: edge
                .certainty
                .map(|certainty| certainty.as_str().to_string()),
            confidence: edge.confidence,
        })
    }

    pub fn node_occurrences(
        &self,
        req: NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage_read_only()?;
        let mut occurrences = storage
            .get_occurrences_for_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to load node occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }

    pub fn edge_occurrences(
        &self,
        req: EdgeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage_read_only()?;
        let mut occurrences = storage
            .get_occurrences_for_element(id.0)
            .map_err(|e| ApiError::internal(format!("Failed to load edge occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }

    pub fn read_file_text(
        &self,
        req: ReadFileTextRequest,
    ) -> Result<ReadFileTextResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, false)?;

        let text = std::fs::read_to_string(&candidate).map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok(ReadFileTextResponse {
            path: candidate.to_string_lossy().to_string(),
            text,
        })
    }

    pub(crate) fn bounded_file_snippet(
        &self,
        path: &str,
        line: u32,
        context_lines: usize,
        max_bytes: usize,
        truncation_suffix: &str,
    ) -> Result<(String, BoundedSnippet), ApiError> {
        let candidate = self.resolve_project_file_path(path, false)?;
        let snippet = bounded_markdown_snippet_from_path(
            &candidate,
            line,
            context_lines,
            max_bytes,
            truncation_suffix,
        )
        .map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok((candidate.to_string_lossy().to_string(), snippet))
    }

    pub(crate) fn bounded_file_snippet_range(
        &self,
        path: &str,
        options: BoundedSnippetRangeOptions<'_>,
    ) -> Result<(String, BoundedSnippet), ApiError> {
        let candidate = self.resolve_project_file_path(path, false)?;
        let snippet = bounded_markdown_snippet_range_from_path(
            &candidate,
            options.focus_line,
            options.start_line,
            options.end_line,
            options.context_lines,
            options.max_bytes,
            options.truncation_suffix,
        )
        .map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok((candidate.to_string_lossy().to_string(), snippet))
    }

    pub fn write_file_text(
        &self,
        req: WriteFileTextRequest,
    ) -> Result<WriteFileResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, true)?;
        std::fs::write(&candidate, &req.text).map_err(|e| {
            ApiError::internal(format!("Failed to write file {}: {e}", candidate.display()))
        })?;

        Ok(WriteFileResponse {
            bytes_written: clamp_i64_to_u32(req.text.len() as i64),
        })
    }
}
