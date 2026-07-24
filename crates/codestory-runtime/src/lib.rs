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
    SourcePolicyExclusionRecord, Store, StructuralTextPublicationCompatibility, SymbolSearchDoc,
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
use serde::Deserialize;
use std::cell::RefCell;
use std::cmp::Ordering;
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
mod publication;
mod repo_text;
mod search_intent;
mod search_plan;
mod search_publication;
mod search_scoring;
mod search_state;
mod search_terms;
mod semantic_projection;
mod snippets;
mod workspace_state;
use affected::*;
pub use agent::{packet_step_trace_json, plan_packet};
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

enum ReadStorage {
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

#[derive(Debug, Deserialize)]
struct RouteEndpointCanonicalMetadata {
    kind: String,
    #[serde(default)]
    framework: Option<String>,
    method: String,
    path: String,
    #[serde(default)]
    raw_path: Option<String>,
    #[serde(default)]
    params: Vec<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    source_convention: Option<String>,
    #[serde(default)]
    provenance: Vec<String>,
}

fn route_endpoint_metadata_from_canonical(
    raw: &str,
    node: &GraphNode,
    source_file: Option<&str>,
) -> serde_json::Result<RouteEndpointMetadataDto> {
    let canonical = serde_json::from_str::<RouteEndpointCanonicalMetadata>(raw)?;
    let kind = match canonical.kind.as_str() {
        "openapi_endpoint" => RouteEndpointKindDto::OpenapiEndpoint,
        _ => RouteEndpointKindDto::FrameworkRoute,
    };
    Ok(RouteEndpointMetadataDto {
        kind,
        framework: canonical.framework,
        method: canonical.method,
        path: canonical.path,
        raw_path: canonical.raw_path,
        params: canonical.params,
        source_file: source_file.map(ToOwned::to_owned),
        line: node.start_line,
        confidence: canonical.confidence,
        source_convention: canonical.source_convention,
        handler: None,
        provenance: canonical.provenance,
    })
}

fn route_endpoint_extraction_score_adjustment(canonical_id: Option<&str>) -> f32 {
    let Some(raw) = canonical_id.and_then(|value| value.strip_prefix("route_endpoint:")) else {
        return 0.0;
    };
    let Ok(canonical) = serde_json::from_str::<RouteEndpointCanonicalMetadata>(raw) else {
        return 0.0;
    };

    if canonical.provenance.iter().any(|entry| {
        matches!(
            entry.as_str(),
            "extraction:ast_indexed" | "extraction:tree_sitter_query"
        )
    }) {
        return 0.025;
    }
    if canonical.provenance.iter().any(|entry| {
        matches!(
            entry.as_str(),
            "extraction:text_only" | "extraction:lexical_fallback"
        )
    }) {
        return -0.025;
    }
    0.0
}

fn route_endpoint_adjusted_search_score(score: f32, canonical_id: Option<&str>) -> f32 {
    (score + route_endpoint_extraction_score_adjustment(canonical_id)).max(0.0)
}

fn route_endpoint_metadata_from_openapi_label(
    label: &str,
    node: &GraphNode,
    source_file: Option<&str>,
) -> Option<RouteEndpointMetadataDto> {
    let (method, path) = label.split_once(' ')?;
    Some(RouteEndpointMetadataDto {
        kind: RouteEndpointKindDto::OpenapiEndpoint,
        framework: None,
        method: method.to_ascii_uppercase(),
        path: path.to_string(),
        raw_path: Some(path.to_string()),
        params: route_endpoint_params(path),
        source_file: source_file.map(ToOwned::to_owned),
        line: node.start_line,
        confidence: Some("schema".to_string()),
        source_convention: Some("openapi".to_string()),
        handler: None,
        provenance: vec!["openapi".to_string()],
    })
}

fn route_endpoint_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| {
            let segment = segment.trim();
            segment
                .strip_prefix(':')
                .or_else(|| {
                    segment
                        .strip_prefix('{')
                        .and_then(|value| value.strip_suffix('}'))
                })
                .map(str::to_string)
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn route_handler_certainty_rank(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
) -> u8 {
    match certainty {
        Some(codestory_contracts::graph::ResolutionCertainty::Certain) => 3,
        Some(codestory_contracts::graph::ResolutionCertainty::Probable) => 2,
        Some(codestory_contracts::graph::ResolutionCertainty::Uncertain) => 1,
        None => 0,
    }
}

#[derive(Debug, Clone)]
struct RouteHandlerCandidate {
    edge: GraphEdge,
    target: GraphNode,
}

fn compare_optional_confidence_desc(left: Option<f32>, right: Option<f32>) -> Ordering {
    let left = left.filter(|value| value.is_finite());
    let right = right.filter(|value| value.is_finite());
    match (left, right) {
        (Some(left), Some(right)) => right.total_cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_route_handler_candidates(
    left: &RouteHandlerCandidate,
    right: &RouteHandlerCandidate,
) -> Ordering {
    // Persisted graph edges have unique IDs, so the edge ID tie-breaker makes
    // this a total order for every valid route-handler candidate set.
    compare_optional_confidence_desc(left.edge.confidence, right.edge.confidence)
        .then_with(|| {
            route_handler_certainty_rank(right.edge.certainty)
                .cmp(&route_handler_certainty_rank(left.edge.certainty))
        })
        .then(left.target.canonical_id.cmp(&right.target.canonical_id))
        .then(left.target.qualified_name.cmp(&right.target.qualified_name))
        .then(
            left.target
                .serialized_name
                .cmp(&right.target.serialized_name),
        )
        .then((left.target.kind as i32).cmp(&(right.target.kind as i32)))
        .then(left.target.file_node_id.cmp(&right.target.file_node_id))
        .then(left.target.start_line.cmp(&right.target.start_line))
        .then(left.target.start_col.cmp(&right.target.start_col))
        .then(left.target.end_line.cmp(&right.target.end_line))
        .then(left.target.end_col.cmp(&right.target.end_col))
        .then(left.target.id.cmp(&right.target.id))
        .then(left.edge.id.cmp(&right.edge.id))
        .then(left.edge.source.cmp(&right.edge.source))
        .then(left.edge.target.cmp(&right.edge.target))
        .then(left.edge.resolved_source.cmp(&right.edge.resolved_source))
        .then(left.edge.resolved_target.cmp(&right.edge.resolved_target))
        .then(left.edge.line.cmp(&right.edge.line))
        .then(
            left.edge
                .callsite_identity
                .cmp(&right.edge.callsite_identity),
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrameworkRouteCoverageEntry {
    framework: &'static str,
    language: &'static str,
    status: &'static str,
    coverage_evidence: &'static str,
    confidence_floor: &'static str,
    handler_link_support: &'static str,
    unsupported_patterns: &'static [&'static str],
    known_gaps: &'static [&'static str],
    promotable: bool,
}

const FRAMEWORK_ROUTE_COVERAGE_ENTRIES: &[FrameworkRouteCoverageEntry] = &[
    FrameworkRouteCoverageEntry {
        framework: "express",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_direct_handler_names_when_graph_resolution_succeeds",
        unsupported_patterns: &[
            "nested, injected, factory-returned, chained, and multi-target receivers are not promoted",
            "dynamic paths and middleware arrays do not produce handler claims",
            "inline and nested-call handlers do not produce name-based handler links",
        ],
        known_gaps: &[
            "mounted app and router prefixes are not globally propagated",
            "direct require('express')() construction is not promoted",
        ],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "react-router",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &[
            "loader/action route objects and nested config composition are partial",
        ],
        known_gaps: &["generated routes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "sveltekit",
        language: "svelte/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_server_method_exports",
        unsupported_patterns: &[
            "route groups and advanced matcher params are normalized conservatively",
        ],
        known_gaps: &["layout load propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nextjs",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_route_method_exports",
        unsupported_patterns: &["middleware rewrites and route groups require source review"],
        known_gaps: &["parallel routes and intercepting routes are not fully modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "remix",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_loader_action_exports",
        unsupported_patterns: &["route config composition and resource routes are partial"],
        known_gaps: &["flat-route edge cases need more real-repo probes"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "astro",
        language: "astro/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_endpoint_method_exports",
        unsupported_patterns: &["redirects and integration-generated routes are not expanded"],
        known_gaps: &["content collections do not create route nodes"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nuxt",
        language: "vue/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_server_handlers",
        unsupported_patterns: &["route middleware and generated module routes are partial"],
        known_gaps: &["custom router options are not evaluated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fastify",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_direct_handler_names_when_graph_resolution_succeeds",
        unsupported_patterns: &[
            "nested, injected, factory-returned, chained, and multi-target receivers are not promoted",
            "dynamic paths, method arrays, and nested route builders are not promoted",
            "inline and wrapped handlers do not produce name-based handler links",
        ],
        known_gaps: &[
            "register() prefix propagation is not modeled",
            "schema and runtime middleware semantics are not evaluated",
        ],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "koa",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["router prefixes and middleware arrays are partial"],
        known_gaps: &["mounted router prefixes are not globally propagated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "hono",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["basePath/grouped routes are partial"],
        known_gaps: &["OpenAPI helper generated routes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nestjs",
        language: "typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "decorator",
        handler_link_support: "probable_for_controller_method",
        unsupported_patterns: &["global prefixes and dynamic decorator expressions are partial"],
        known_gaps: &["module graph prefix propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "django",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["include() trees and namespaced URLConfs are not fully expanded"],
        known_gaps: &["path converters beyond parameter names are not typed"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "flask",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "decorator",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["blueprint prefixes and dynamic method declarations are partial"],
        known_gaps: &["method lists are not fully enumerated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fastapi",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_decorated_handler",
        unsupported_patterns: &[
            "path= keyword arguments and escaped non-raw string literals are not exact routes",
            "head/options/api_route/websocket decorators are not indexed",
            "chained or multi-target FastAPI/APIRouter construction is not promoted",
            "factory-returned or injected router receivers without module-scope construction are not claimed",
        ],
        known_gaps: &["include_router prefix propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "rails",
        language: "ruby",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["resource expansion is not fully enumerated"],
        known_gaps: &["constraints/scopes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "laravel",
        language: "php",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["controller arrays and route groups are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "spring",
        language: "java",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "annotation",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["class-level prefixes are not fully combined in every case"],
        known_gaps: &["composed annotations are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "aspnet",
        language: "csharp",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "attribute",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["controller-level route templates are partial"],
        known_gaps: &["minimal API grouping is not fully modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "axum",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["nested routers and stateful route composition are partial"],
        known_gaps: &["Router::nest prefix propagation is limited"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "actix",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["scoped services and macros are partial"],
        known_gaps: &["web::scope prefix propagation is limited"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "rocket",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "attribute",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["mount prefixes are not fully combined"],
        known_gaps: &["rank/format route attributes are not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "gin",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["router groups and middleware chains are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "chi",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["route groups and mounted subrouters are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "echo",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["group prefixes are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fiber",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["group prefixes and mounted apps are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "vue-router",
        language: "vue",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["imported route arrays and generated routes are partial"],
        known_gaps: &["Nuxt file routes are reported separately as nuxt"],
        promotable: true,
    },
];

fn framework_route_coverage_matrix() -> Vec<FrameworkRouteCoverageDto> {
    FRAMEWORK_ROUTE_COVERAGE_ENTRIES
        .iter()
        .map(framework_route_coverage_dto)
        .collect()
}

fn framework_route_coverage_dto(entry: &FrameworkRouteCoverageEntry) -> FrameworkRouteCoverageDto {
    FrameworkRouteCoverageDto {
        framework: entry.framework.to_string(),
        language: entry.language.to_string(),
        status: entry.status.to_string(),
        coverage_evidence: entry.coverage_evidence.to_string(),
        confidence_floor: entry.confidence_floor.to_string(),
        handler_link_support: entry.handler_link_support.to_string(),
        unsupported_patterns: entry
            .unsupported_patterns
            .iter()
            .map(|value| value.to_string())
            .collect(),
        known_gaps: entry
            .known_gaps
            .iter()
            .map(|value| value.to_string())
            .collect(),
        promotable: entry.promotable,
    }
}

struct LanguageSupportSummary {
    support_mode: String,
    evidence_tier: String,
    claim_label: String,
}

fn language_support_summary_for_language(language: &str) -> LanguageSupportSummary {
    language_support_profile_for_language_name(language)
        .map(|profile| LanguageSupportSummary {
            support_mode: profile.support_mode.as_str().to_string(),
            evidence_tier: profile.evidence_tier.as_str().to_string(),
            claim_label: profile.claim_label.to_string(),
        })
        .unwrap_or_else(|| LanguageSupportSummary {
            support_mode: "unknown".to_string(),
            evidence_tier: "unknown".to_string(),
            claim_label: "no support claim recorded".to_string(),
        })
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

#[derive(Debug, Clone, Default)]
struct OptionalResolutionTelemetry {
    setup_existing_projection_ids_ms: Option<u32>,
    setup_seed_symbol_table_ms: Option<u32>,
    flush_files_ms: Option<u32>,
    flush_nodes_ms: Option<u32>,
    flush_edges_ms: Option<u32>,
    flush_occurrences_ms: Option<u32>,
    flush_component_access_ms: Option<u32>,
    flush_callable_projection_ms: Option<u32>,
    resolution_override_count_ms: Option<u32>,
    resolution_unresolved_counts_ms: Option<u32>,
    resolution_calls_ms: Option<u32>,
    resolution_imports_ms: Option<u32>,
    resolution_cleanup_ms: Option<u32>,
    resolution_call_candidate_index_ms: Option<u32>,
    resolution_import_candidate_index_ms: Option<u32>,
    resolution_call_semantic_index_ms: Option<u32>,
    resolution_import_semantic_index_ms: Option<u32>,
    resolution_support_snapshot_limit_bytes: Option<u64>,
    resolution_support_snapshot_stored: Option<bool>,
    resolution_support_snapshot_skipped_oversize: Option<bool>,
    resolution_call_semantic_candidates_ms: Option<u32>,
    resolution_import_semantic_candidates_ms: Option<u32>,
    resolution_call_semantic_requests: Option<u32>,
    resolution_call_semantic_unique_requests: Option<u32>,
    resolution_call_semantic_skipped_requests: Option<u32>,
    resolution_import_semantic_requests: Option<u32>,
    resolution_import_semantic_unique_requests: Option<u32>,
    resolution_import_semantic_skipped_requests: Option<u32>,
    resolution_call_compute_ms: Option<u32>,
    resolution_import_compute_ms: Option<u32>,
    resolution_call_apply_ms: Option<u32>,
    resolution_import_apply_ms: Option<u32>,
    resolution_override_resolution_ms: Option<u32>,
    resolved_calls_same_file: Option<u32>,
    resolved_calls_same_module: Option<u32>,
    resolved_calls_global_unique: Option<u32>,
    resolved_calls_semantic: Option<u32>,
    resolved_imports_same_file: Option<u32>,
    resolved_imports_same_module: Option<u32>,
    resolved_imports_global_unique: Option<u32>,
    resolved_imports_fuzzy: Option<u32>,
    resolved_imports_semantic: Option<u32>,
}

fn artifact_cache_access_timings(stats: &ArtifactCacheFamilyStats) -> ArtifactCacheAccessTimings {
    ArtifactCacheAccessTimings {
        policy: match stats.policy {
            ArtifactCachePolicy::KnownEmpty => ArtifactCachePolicyDto::KnownEmpty,
            ArtifactCachePolicy::ReadThrough => ArtifactCachePolicyDto::ReadThrough,
        },
        logical_lookups: clamp_usize_to_u32(stats.logical_lookups),
        physical_queries: clamp_usize_to_u32(stats.physical_queries),
        hits: clamp_usize_to_u32(stats.hits),
        misses: clamp_usize_to_u32(stats.misses),
        reader_opens: clamp_usize_to_u32(stats.reader_opens),
        lookup_wall_ms: clamp_u64_to_u32(stats.lookup_wall_ns / 1_000_000),
    }
}

fn projection_persistence_family_timings(
    stats: codestory_store::ProjectionPersistenceFamilyStats,
) -> ProjectionPersistenceFamilyTimings {
    ProjectionPersistenceFamilyTimings {
        row_attempts: stats.row_attempts,
        bound_bytes: stats.bound_bytes,
        statement_executions: stats.statement_executions,
        wall_ms: stats.wall_ms,
    }
}

fn projection_persistence_timings(
    stats: &codestory_store::ProjectionPersistenceStats,
) -> ProjectionPersistenceTimings {
    ProjectionPersistenceTimings {
        transactions: stats.transactions.min(u32::MAX as u64) as u32,
        row_attempts: stats.row_attempts(),
        bound_bytes: stats.bound_bytes(),
        statement_executions: stats.statement_executions(),
        transaction_wall_ms: stats.transaction_wall_ms,
        transaction_setup_ms: stats.transaction_setup_ms,
        commit_ms: stats.commit_ms,
        files: projection_persistence_family_timings(stats.files),
        nodes: projection_persistence_family_timings(stats.nodes),
        structural_text: projection_persistence_family_timings(stats.structural_text),
        edges: projection_persistence_family_timings(stats.edges),
        occurrences: projection_persistence_family_timings(stats.occurrences),
        component_access: projection_persistence_family_timings(stats.component_access),
        callable_projection: projection_persistence_family_timings(stats.callable_projection),
        file_errors: projection_persistence_family_timings(stats.file_errors),
        dirty_state: projection_persistence_family_timings(stats.dirty_state),
    }
}

fn database_snapshot_copy_timings(
    stats: codestory_store::DatabaseSnapshotCopyStats,
) -> DatabaseSnapshotCopyTimings {
    DatabaseSnapshotCopyTimings {
        copy_ms: stats.copy_ms,
        source_bytes: stats.source_bytes,
        target_bytes: stats.target_bytes,
    }
}

fn core_promotion_timings(stats: codestory_store::CorePromotionStats) -> CorePromotionTimings {
    CorePromotionTimings {
        total_ms: stats.total_ms,
        lock_recovery_ms: stats.lock_recovery_ms,
        candidate_validation_ms: stats.candidate_validation_ms,
        previous_validation_ms: stats.previous_validation_ms,
        rollback_backup_copy_ms: stats.rollback_backup_copy_ms,
        backup_validation_ms: stats.backup_validation_ms,
        prepared_journal_write_ms: stats.prepared_journal_write_ms,
        prepared_journal_file_sync_ms: stats.prepared_journal_file_sync_ms,
        prepared_journal_directory_sync_ms: stats.prepared_journal_directory_sync_ms,
        staged_to_live_restore_ms: stats.staged_to_live_restore_ms,
        promoted_validation_ms: stats.promoted_validation_ms,
        committed_journal_ms: stats.committed_journal_ms,
        cleanup_ms: stats.cleanup_ms,
        unattributed_ms: stats.unattributed_ms,
        candidate_bytes: stats.candidate_bytes,
        previous_live_bytes: stats.previous_live_bytes,
        rollback_backup_bytes: stats.rollback_backup_bytes,
    }
}

impl OptionalResolutionTelemetry {
    fn from_incremental_stats(index_stats: &IncrementalIndexingStats) -> Self {
        let mut telemetry = Self::from_flush_stats(index_stats);
        if index_stats.resolution_ran {
            telemetry.apply_resolution_stats(index_stats);
        }
        telemetry
    }

    fn from_flush_stats(index_stats: &IncrementalIndexingStats) -> Self {
        Self {
            setup_existing_projection_ids_ms: Some(clamp_u64_to_u32(
                index_stats.setup_existing_projection_ids_ms,
            )),
            setup_seed_symbol_table_ms: Some(clamp_u64_to_u32(
                index_stats.setup_seed_symbol_table_ms,
            )),
            flush_files_ms: Some(clamp_u64_to_u32(index_stats.flush_files_ms)),
            flush_nodes_ms: Some(clamp_u64_to_u32(index_stats.flush_nodes_ms)),
            flush_edges_ms: Some(clamp_u64_to_u32(index_stats.flush_edges_ms)),
            flush_occurrences_ms: Some(clamp_u64_to_u32(index_stats.flush_occurrences_ms)),
            flush_component_access_ms: Some(clamp_u64_to_u32(
                index_stats.flush_component_access_ms,
            )),
            flush_callable_projection_ms: Some(clamp_u64_to_u32(
                index_stats.flush_callable_projection_ms,
            )),
            ..Self::default()
        }
    }

    fn apply_resolution_stats(&mut self, index_stats: &IncrementalIndexingStats) {
        self.resolution_override_count_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_override_count_ms));
        self.resolution_unresolved_counts_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_unresolved_counts_ms,
        ));
        self.resolution_calls_ms = Some(clamp_u64_to_u32(index_stats.resolution_calls_ms));
        self.resolution_imports_ms = Some(clamp_u64_to_u32(index_stats.resolution_imports_ms));
        self.resolution_cleanup_ms = Some(clamp_u64_to_u32(index_stats.resolution_cleanup_ms));
        self.resolution_call_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_candidate_index_ms,
        ));
        self.resolution_import_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_candidate_index_ms,
        ));
        self.resolution_call_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_index_ms,
        ));
        self.resolution_import_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_index_ms,
        ));
        self.resolution_support_snapshot_limit_bytes =
            Some(index_stats.resolution_support_snapshot_limit_bytes);
        self.resolution_support_snapshot_stored =
            Some(index_stats.resolution_support_snapshot_stored);
        self.resolution_support_snapshot_skipped_oversize =
            Some(index_stats.resolution_support_snapshot_skipped_oversize);
        self.resolution_call_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_candidates_ms,
        ));
        self.resolution_import_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_candidates_ms,
        ));
        self.resolution_call_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_requests,
        ));
        self.resolution_call_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_unique_requests,
        ));
        self.resolution_call_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_skipped_requests,
        ));
        self.resolution_import_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_requests,
        ));
        self.resolution_import_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_unique_requests,
        ));
        self.resolution_import_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_skipped_requests,
        ));
        self.resolution_call_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_compute_ms));
        self.resolution_import_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_compute_ms));
        self.resolution_call_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_apply_ms));
        self.resolution_import_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_apply_ms));
        self.resolution_override_resolution_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_override_resolution_ms,
        ));
        self.resolved_calls_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_file));
        self.resolved_calls_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_module));
        self.resolved_calls_global_unique =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_global_unique));
        self.resolved_calls_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_semantic));
        self.resolved_imports_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_file));
        self.resolved_imports_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_module));
        self.resolved_imports_global_unique = Some(clamp_usize_to_u32(
            index_stats.resolved_imports_global_unique,
        ));
        self.resolved_imports_fuzzy = Some(clamp_usize_to_u32(index_stats.resolved_imports_fuzzy));
        self.resolved_imports_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_semantic));
    }
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

const INDEX_FRESHNESS_INDEXED_FILE_CAP: usize = 25_000;
const INDEX_FRESHNESS_CURRENT_FILE_CAP: usize = 25_000;
const INDEX_FRESHNESS_SAMPLE_LIMIT: usize = 8;
const INDEX_FRESHNESS_CACHE_DEFAULT_TTL_SECS: u64 = 60;
#[cfg(test)]
const EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP: usize = 80;

#[cfg(test)]
thread_local! {
    static AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK: RefCell<Option<Box<dyn FnOnce()>>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
fn arm_after_index_freshness_fence_test_hook(hook: impl FnOnce() + 'static) {
    AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_index_freshness_fence_test_hook() {
    let hook = AFTER_INDEX_FRESHNESS_FENCE_TEST_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

fn not_checked_index_freshness(
    reason: impl Into<String>,
    indexed_file_count: u32,
    started_at: Instant,
) -> IndexFreshnessDto {
    IndexFreshnessDto {
        status: IndexFreshnessStatusDto::NotChecked,
        changed_file_count: 0,
        new_file_count: 0,
        removed_file_count: 0,
        checked_file_count: 0,
        indexed_file_count,
        duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
        reason: Some(reason.into()),
        samples: Vec::new(),
    }
}

fn indexable_source_path(path: &Path) -> bool {
    if path.to_str().is_some_and(|path| {
        !Path::new(path).is_absolute()
            && codestory_contracts::language_support::is_structural_source_path(path)
            && codestory_contracts::language_support::structural_source_path_exclusion(path)
                .is_some()
    }) {
        return false;
    }
    let tree_sitter_supported = path
        .extension()
        .and_then(|value| value.to_str())
        .and_then(codestory_indexer::get_language_for_ext)
        .is_some();
    tree_sitter_supported
        || codestory_indexer::template_pipeline::template_kind_for_path(path).is_some()
        || codestory_indexer::structural::is_structural_candidate_path(path)
        || codestory_indexer::is_text_only_candidate_path(path)
        || looks_like_openapi_source_path(path)
}

fn indexable_source_path_in_workspace(root: &Path, path: &Path) -> bool {
    let Some(relative) = codestory_workspace::workspace_relative_path(root, path) else {
        return false;
    };
    indexable_source_path(&relative)
}

fn indexable_source_path_with_root(root: Option<&Path>, path: &Path) -> bool {
    root.map_or_else(
        || indexable_source_path(path),
        |root| indexable_source_path_in_workspace(root, path),
    )
}

fn looks_like_openapi_source_path(path: &Path) -> bool {
    if !codestory_indexer::is_openapi_candidate_path(path) {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return true;
    };
    codestory_indexer::looks_like_openapi_schema(&source)
}

fn index_freshness_observation_from_storage(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
) -> IndexFreshnessObservation {
    let mut identities = AffectedOperationIdentityIndex::native();
    index_freshness_observation_from_storage_with_identities(
        root,
        workspace,
        storage,
        policy,
        &mut identities,
    )
}

fn index_freshness_observation_from_storage_with_identities<R>(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
    identities: &mut AffectedOperationIdentityIndex<R>,
) -> IndexFreshnessObservation
where
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    let started_at = Instant::now();
    match storage.has_incomplete_incremental_run() {
        Ok(true) => {
            return IndexFreshnessObservation::incomplete(IndexFreshnessDto {
                status: IndexFreshnessStatusDto::Stale,
                changed_file_count: 0,
                new_file_count: 0,
                removed_file_count: 0,
                checked_file_count: 0,
                indexed_file_count: 0,
                duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
                reason: Some(
                    "previous_incremental_run_incomplete_full_refresh_required".to_string(),
                ),
                samples: Vec::new(),
            });
        }
        Ok(false) => {}
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to inspect incomplete index marker: {error}"),
                0,
                started_at,
            ));
        }
    }
    match storage.get_complete_index_publication() {
        Ok(Some(publication)) => {
            if let Err(error) = validate_structural_text_units(storage, &publication) {
                return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                    format!(
                        "structural text unit publication is incomplete: {}",
                        error.message
                    ),
                    0,
                    started_at,
                ));
            }
            if let Err(error) =
                validate_source_policy_exclusions(storage, root, &publication, policy)
            {
                return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                    format!(
                        "source policy exclusion publication is incomplete: {}",
                        error.message
                    ),
                    0,
                    started_at,
                ));
            }
        }
        Ok(None) => {}
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to read complete core publication: {error}"),
                0,
                started_at,
            ));
        }
    }
    #[cfg(test)]
    run_after_index_freshness_fence_test_hook();
    let files = match storage.get_files() {
        Ok(files) => files,
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to read indexed file inventory: {error}"),
                0,
                started_at,
            ));
        }
    };
    let indexed_file_count = clamp_usize_to_u32(files.len());
    if files.is_empty() {
        return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
            "no indexed file inventory is available yet",
            indexed_file_count,
            started_at,
        ));
    }
    if files.len() > INDEX_FRESHNESS_INDEXED_FILE_CAP {
        return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
            format!(
                "indexed file inventory exceeds bounded freshness cap ({} > {})",
                files.len(),
                INDEX_FRESHNESS_INDEXED_FILE_CAP
            ),
            indexed_file_count,
            started_at,
        ));
    }

    let stored_files = match storage.files().inventory() {
        Ok(files) => files,
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to read refresh inventory: {error}"),
                indexed_file_count,
                started_at,
            ));
        }
    };
    let removed_paths = files
        .iter()
        .map(|file| (file.id, file.path.clone()))
        .collect::<HashMap<_, _>>();
    let stored_policy_exclusions = match storage.get_source_policy_exclusions() {
        Ok(exclusions) => exclusions,
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to read source policy exclusions: {error}"),
                indexed_file_count,
                started_at,
            ));
        }
    };
    let refresh_inputs = RefreshInputs {
        stored_files,
        policy_exclusions: stored_policy_exclusions
            .iter()
            .map(source_policy_exclusion_candidate)
            .collect(),
        inventory: Default::default(),
    };
    let refresh = match workspace.build_execution_outcome_bounded_with_policy(
        &refresh_inputs,
        INDEX_FRESHNESS_CURRENT_FILE_CAP,
        policy,
    ) {
        Ok(refresh) => refresh,
        Err(error) => {
            return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
                format!("failed to check workspace inventory: {error}"),
                indexed_file_count,
                started_at,
            ));
        }
    };
    if refresh.refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
        let detail = refresh
            .refresh
            .inventory_issues
            .first()
            .map(|issue| format!("{}: {}", issue.path.display(), issue.message));
        return IndexFreshnessObservation::incomplete(not_checked_index_freshness(
            match detail {
                Some(detail) => format!(
                    "current workspace inventory is {:?}: {detail}",
                    refresh.refresh.inventory_outcome
                ),
                None => format!(
                    "current workspace inventory is {:?} (>{})",
                    refresh.refresh.inventory_outcome, INDEX_FRESHNESS_CURRENT_FILE_CAP
                ),
            },
            indexed_file_count,
            started_at,
        ));
    }
    let current_policy_exclusions = refresh.policy_exclusions;
    let plan = refresh.refresh.plan;

    let mut changed_file_count = 0u32;
    let mut new_file_count = 0u32;
    let mut samples = Vec::new();
    for path in &plan.files_to_index {
        let existing_indexed_file = plan.existing_file_ids.contains_key(path);
        if !existing_indexed_file && !indexable_source_path_in_workspace(root, path) {
            continue;
        }
        let kind = if existing_indexed_file {
            changed_file_count = changed_file_count.saturating_add(1);
            IndexFreshnessChangeKindDto::Changed
        } else {
            new_file_count = new_file_count.saturating_add(1);
            IndexFreshnessChangeKindDto::New
        };
        if samples.len() < INDEX_FRESHNESS_SAMPLE_LIMIT {
            samples.push(IndexFreshnessSampleDto {
                kind,
                path: runtime_relative_path(root, path),
            });
        }
    }

    let previous_policy_by_path = stored_policy_exclusions
        .iter()
        .map(|entry| (entry.normalized_path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let current_policy_paths = current_policy_exclusions
        .iter()
        .map(|entry| entry.normalized_path.as_str())
        .collect::<HashSet<_>>();
    let planned_paths = plan
        .files_to_index
        .iter()
        .map(|path| runtime_relative_path(root, path))
        .collect::<HashSet<_>>();
    for exclusion in &current_policy_exclusions {
        let kind = match previous_policy_by_path.get(exclusion.normalized_path.as_str()) {
            Some(previous)
                if previous.content_hash == exclusion.content_hash
                    && previous.observed_size == exclusion.observed_size
                    && previous.observed_unit_count == exclusion.observed_unit_count
                    && previous.policy_version == exclusion.policy_version
                    && previous.byte_cap == exclusion.byte_cap
                    && previous.structural_unit_cap == exclusion.structural_unit_cap =>
            {
                continue;
            }
            Some(_) => {
                changed_file_count = changed_file_count.saturating_add(1);
                IndexFreshnessChangeKindDto::Changed
            }
            None => {
                new_file_count = new_file_count.saturating_add(1);
                IndexFreshnessChangeKindDto::New
            }
        };
        if samples.len() < INDEX_FRESHNESS_SAMPLE_LIMIT {
            samples.push(IndexFreshnessSampleDto {
                kind,
                path: exclusion.normalized_path.clone(),
            });
        }
    }

    let removed_policy_exclusions = stored_policy_exclusions
        .iter()
        .filter(|entry| {
            !current_policy_paths.contains(entry.normalized_path.as_str())
                && !planned_paths.contains(&entry.normalized_path)
        })
        .collect::<Vec<_>>();
    let removed_file_count = clamp_usize_to_u32(
        plan.files_to_remove
            .len()
            .saturating_add(removed_policy_exclusions.len()),
    );
    for removed_id in &plan.files_to_remove {
        if samples.len() >= INDEX_FRESHNESS_SAMPLE_LIMIT {
            break;
        }
        if let Some(path) = removed_paths.get(removed_id) {
            samples.push(IndexFreshnessSampleDto {
                kind: IndexFreshnessChangeKindDto::Removed,
                path: runtime_relative_path(root, path),
            });
        }
    }
    for removed in removed_policy_exclusions {
        if samples.len() >= INDEX_FRESHNESS_SAMPLE_LIMIT {
            break;
        }
        samples.push(IndexFreshnessSampleDto {
            kind: IndexFreshnessChangeKindDto::Removed,
            path: removed.normalized_path.clone(),
        });
    }

    let status = if changed_file_count == 0 && new_file_count == 0 && removed_file_count == 0 {
        IndexFreshnessStatusDto::Fresh
    } else {
        IndexFreshnessStatusDto::Stale
    };
    let checked_file_count = indexed_file_count
        .saturating_sub(removed_file_count)
        .saturating_add(new_file_count);

    for path in plan
        .existing_file_ids
        .keys()
        .chain(plan.files_to_index.iter())
    {
        identities.record_admitted(path);
    }
    for path in &plan.files_to_index {
        identities.record_stale(path);
    }
    for removed_id in &plan.files_to_remove {
        let Some(path) = removed_paths.get(removed_id) else {
            continue;
        };
        identities.record_stale(path);
    }

    let identity_gap_count = identities.freshness_identity_gap_count();
    let identity_gap_sample = identities.freshness_identity_gap_sample();

    IndexFreshnessObservation {
        freshness: IndexFreshnessDto {
            status,
            changed_file_count,
            new_file_count,
            removed_file_count,
            checked_file_count,
            indexed_file_count,
            duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
            reason: None,
            samples,
        },
        inventory_complete: identity_gap_count == 0,
        admitted_identities: identities.admitted_identities.clone(),
        stale_identities: identities.stale_identities.clone(),
        identity_gap_count,
        identity_gap_sample,
    }
}

fn index_freshness_from_storage_with_policy(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
    policy: &SourceIndexPolicy,
) -> IndexFreshnessDto {
    index_freshness_observation_from_storage(root, workspace, storage, policy).freshness
}

#[cfg(test)]
fn index_freshness_from_storage(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
) -> IndexFreshnessDto {
    index_freshness_from_storage_with_policy(
        root,
        workspace,
        storage,
        &SourceIndexPolicy::default(),
    )
}

fn workspace_member_index_summaries(
    root: &Path,
    workspace: &WorkspaceManifest,
    refresh_inputs: &RefreshInputs,
    execution_plan: &RefreshExecutionPlan,
) -> Vec<WorkspaceMemberIndexDto> {
    workspace
        .members()
        .iter()
        .map(|member| {
            let absolute = if member.is_absolute() {
                member.clone()
            } else {
                root.join(member)
            };
            let files_to_index = execution_plan
                .files_to_index
                .iter()
                .filter(|path| path.starts_with(&absolute))
                .count()
                .min(u32::MAX as usize) as u32;
            let indexed_files = refresh_inputs
                .stored_files
                .iter()
                .filter(|file| file.path.starts_with(&absolute))
                .count()
                .min(u32::MAX as usize) as u32;
            WorkspaceMemberIndexDto {
                path: runtime_relative_path(root, &absolute),
                files_to_index,
                indexed_files,
                file_count: None,
                node_count: None,
                edge_count: None,
            }
        })
        .collect()
}

fn workspace_member_storage_summaries(
    root: &Path,
    workspace: &WorkspaceManifest,
    storage: &Storage,
) -> Result<Vec<WorkspaceMemberIndexDto>, ApiError> {
    if workspace.members().is_empty() {
        return Ok(Vec::new());
    }
    let files = storage
        .get_files()
        .map_err(|e| ApiError::internal(format!("Failed to query member files: {e}")))?;
    let nodes = storage
        .get_nodes()
        .map_err(|e| ApiError::internal(format!("Failed to query member nodes: {e}")))?;
    let edges = storage
        .get_edges()
        .map_err(|e| ApiError::internal(format!("Failed to query member edges: {e}")))?;

    let node_file_ids = nodes
        .iter()
        .map(|node| (node.id, node.file_node_id))
        .collect::<HashMap<_, _>>();

    Ok(workspace
        .members()
        .iter()
        .map(|member| {
            let absolute = if member.is_absolute() {
                member.clone()
            } else {
                root.join(member)
            };
            let file_ids = files
                .iter()
                .filter(|file| file.path.starts_with(&absolute))
                .map(|file| codestory_contracts::graph::NodeId(file.id))
                .collect::<HashSet<_>>();
            let file_count = file_ids.len().min(u32::MAX as usize) as u32;
            let node_count = nodes
                .iter()
                .filter(|node| {
                    file_ids.contains(&node.id)
                        || node
                            .file_node_id
                            .is_some_and(|file_id| file_ids.contains(&file_id))
                })
                .count()
                .min(u32::MAX as usize) as u32;
            let edge_count = edges
                .iter()
                .filter(|edge| {
                    edge.file_node_id
                        .is_some_and(|file_id| file_ids.contains(&file_id))
                        || node_file_ids
                            .get(&edge.effective_source())
                            .and_then(|file_id| *file_id)
                            .is_some_and(|file_id| file_ids.contains(&file_id))
                })
                .count()
                .min(u32::MAX as usize) as u32;
            WorkspaceMemberIndexDto {
                path: runtime_relative_path(root, &absolute),
                files_to_index: 0,
                indexed_files: file_count,
                file_count: Some(file_count),
                node_count: Some(node_count),
                edge_count: Some(edge_count),
            }
        })
        .collect())
}

#[derive(Debug, Clone)]
struct CachedIndexFreshness {
    root: PathBuf,
    storage_path: PathBuf,
    storage_fingerprint: String,
    value: IndexFreshnessDto,
    cached_at: Instant,
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

fn index_freshness_cache_ttl_secs() -> u64 {
    std::env::var("CODESTORY_INDEX_FRESHNESS_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ttl| *ttl > 0)
        .unwrap_or(INDEX_FRESHNESS_CACHE_DEFAULT_TTL_SECS)
}

fn storage_fingerprint(path: &Path) -> String {
    [
        storage_path_fingerprint(path),
        storage_path_fingerprint(&path.with_extension("db-wal")),
        storage_path_fingerprint(&path.with_extension("db-shm")),
    ]
    .join("|")
}

fn storage_path_fingerprint(path: &Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return "missing".to_string();
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("len:{}:mtime_ms:{modified_ms}", metadata.len())
}

fn open_storage_for_read(path: &Path) -> Result<Storage, ApiError> {
    let requires_initialization = !path.exists()
        || Storage::database_schema_version(path)
            .map(|version| version != CURRENT_SCHEMA_VERSION)
            .map_err(|error| {
                ApiError::internal(format!("Failed to inspect storage schema: {error}"))
            })?;
    let storage = if requires_initialization {
        Storage::open(path)
    } else {
        Storage::open_read_only(path)
    };
    storage.map_err(|error| ApiError::internal(format!("Failed to open storage: {error}")))
}

fn open_existing_storage_for_read(path: &Path) -> Result<Storage, ApiError> {
    if !path.is_file() {
        return Err(ApiError::new(
            "project_unavailable",
            "no complete project storage is available",
        ));
    }
    let schema = Storage::database_schema_version(path).map_err(|error| {
        ApiError::internal(format!("Failed to inspect storage schema: {error}"))
    })?;
    if schema != CURRENT_SCHEMA_VERSION {
        return Err(ApiError::new(
            "project_unavailable",
            format!(
                "project storage schema {schema} is not readable by runtime schema {CURRENT_SCHEMA_VERSION}"
            ),
        ));
    }
    Storage::open_read_only(path)
        .map_err(|error| ApiError::internal(format!("Failed to open storage: {error}")))
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

    fn open_storage(&self) -> Result<Storage, ApiError> {
        let storage_path = self.require_storage_path()?;
        Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))
    }

    fn open_storage_read_only(&self) -> Result<ReadStorage, ApiError> {
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

    fn file_path_for_node(
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

    fn project_summary_from_storage(
        &self,
        root: &Path,
        storage_path: &Path,
        storage: &Storage,
    ) -> Result<ProjectSummary, ApiError> {
        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;
        let derived_file_count = if stats.file_count > 0 {
            stats.file_count
        } else {
            storage
                .get_file_node_count()
                .map_err(|e| ApiError::internal(format!("Failed to query file nodes: {e}")))?
        };
        let dto_stats = StorageStatsDto {
            node_count: clamp_i64_to_u32(stats.node_count),
            edge_count: clamp_i64_to_u32(stats.edge_count),
            file_count: clamp_i64_to_u32(derived_file_count),
            error_count: clamp_i64_to_u32(stats.error_count),
            fatal_error_count: clamp_i64_to_u32(stats.fatal_error_count),
        };
        let workspace = runtime_workspace_manifest(root, storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let members = workspace_member_storage_summaries(root, &workspace, storage)?;
        let freshness =
            self.cached_index_freshness_from_storage(root, storage_path, &workspace, storage);
        let publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read complete index publication: {error}"
                ))
            })?
            .map(index_publication_dto);

        Ok(ProjectSummary {
            root: root.to_string_lossy().to_string(),
            stats: dto_stats,
            members,
            retrieval: Some(retrieval_state_from_storage_for_runtime(
                storage,
                &self.runtime_config,
            )?),
            freshness: Some(freshness),
            publication,
        })
    }

    pub fn complete_index_publication_at(
        &self,
        storage_path: &Path,
    ) -> Result<Option<IndexPublicationDto>, ApiError> {
        if !storage_path.is_file() {
            return Ok(None);
        }
        Store::open_observational(storage_path)
            .and_then(|storage| storage.get_complete_index_publication())
            .map(|publication| publication.map(index_publication_dto))
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to observe complete index publication: {error}"
                ))
            })
    }

    fn open_project_summary_with_storage_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        let storage = open_storage_for_read(&storage_path)?;
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!("Failed to begin project summary snapshot: {error}"))
        })?;
        let summary =
            self.project_summary_from_storage(&root, &storage_path, snapshot.storage())?;
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish project summary snapshot: {error}"
            ))
        })?;

        {
            let mut s = self.state.lock();
            s.project_root = Some(root);
            s.storage_path = Some(storage_path);
            s.node_names.clear();
            clear_search_engine(&mut s);
        }
        self.sidecar_query_cache.lock().clear();

        Ok(summary)
    }

    fn open_project_with_storage_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        let mut storage = open_storage_for_read(&storage_path)?;
        let loaded = load_persisted_search_state_for_runtime(
            &mut storage,
            &storage_path,
            &self.runtime_config,
        )?;
        let mut summary = self.project_summary_from_storage(&root, &storage_path, &storage)?;
        summary.retrieval = Some(retrieval_state_from_storage_for_runtime(
            &storage,
            &self.runtime_config,
        )?);

        {
            let mut s = self.state.lock();
            s.project_root = Some(root);
            s.storage_path = Some(storage_path);
            s.node_names = loaded.node_names;
            publish_search_engine(&mut s, loaded.engine, loaded.publication);
        }
        self.sidecar_query_cache.lock().clear();

        let _ = self.events_tx.send(AppEventPayload::StatusUpdate {
            message: "Project opened.".to_string(),
        });

        Ok(summary)
    }

    pub fn open_project(&self, req: OpenProjectRequest) -> Result<ProjectSummary, ApiError> {
        let root = PathBuf::from(req.path);
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }

        let storage_path = root.join("codestory.db");
        self.open_project_with_storage_path(root, storage_path)
    }

    pub fn open_project_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if let Some(parent) = storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to create storage directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        self.open_project_with_storage_inner(root, storage_path)
    }

    pub fn open_project_summary_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if let Some(parent) = storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to create storage directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        self.open_project_summary_with_storage_inner(root, storage_path)
    }

    pub fn inspect_project_summary_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<Option<ProjectSummary>, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if !storage_path.is_file() {
            return Ok(None);
        }
        let storage = Storage::open_observational(&storage_path).map_err(|error| {
            ApiError::internal(format!("Failed to open storage observationally: {error}"))
        })?;
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!("Failed to begin project summary snapshot: {error}"))
        })?;
        let summary =
            self.project_summary_from_storage(&root, &storage_path, snapshot.storage())?;
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish project summary snapshot: {error}"
            ))
        })?;
        let changed = {
            let mut state = self.state.lock();
            let changed =
                state.project_root.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &root)
                }) || state.storage_path.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &storage_path)
                });
            if changed {
                state.node_names.clear();
                clear_search_engine(&mut state);
            }
            state.project_root = Some(root);
            state.storage_path = Some(storage_path);
            changed
        };
        if changed {
            self.sidecar_query_cache.lock().clear();
        }
        Ok(Some(summary))
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        let (root, storage_path) = {
            let s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let root = s.project_root.clone().ok_or_else(|| {
                ApiError::invalid_argument("No project open. Call open_project first.")
            })?;
            let storage_path = s
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        if req.mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        {
            let mut s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            s.is_indexing = true;
            s.index_freshness_cache = None;
        }

        let events_tx = self.events_tx.clone();
        let controller = self.clone();

        // Use a dedicated thread so callers can keep their runtime responsive.
        std::thread::spawn(move || {
            let indexing_started = std::time::Instant::now();
            let result = match IndexWriterGuard::try_acquire(&storage_path) {
                Ok(_writer_guard) => {
                    let result = match req.mode {
                        IndexMode::Full => index_full_for_runtime(
                            &root,
                            &storage_path,
                            &events_tx,
                            None,
                            &controller.runtime_config,
                            &controller.source_index_policy,
                        ),
                        IndexMode::Incremental => index_incremental_for_runtime(
                            &root,
                            &storage_path,
                            &events_tx,
                            None,
                            &controller.runtime_config,
                            &controller.source_index_policy,
                        ),
                    };
                    result.and_then(|summary| {
                        controller.finish_successful_indexing(summary, &storage_path, true, None)
                    })
                }
                Err(error) => Err(error),
            };

            match result {
                Ok(phase_timings) => {
                    controller.state.lock().is_indexing = false;
                    let _ = events_tx.send(AppEventPayload::IndexingComplete {
                        duration_ms: clamp_u128_to_u32(indexing_started.elapsed().as_millis()),
                        phase_timings,
                    });
                }
                Err(err) => {
                    let _ = events_tx.send(AppEventPayload::IndexingFailed { error: err.message });
                    controller.recover_failed_indexing(&storage_path, true);
                }
            }
        });

        Ok(())
    }

    fn run_indexing_blocking_inner(
        &self,
        mode: IndexMode,
        refresh_runtime_caches: bool,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        let (root, storage_path) = {
            let s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let root = s.project_root.clone().ok_or_else(no_project_error)?;
            let storage_path = s
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        if mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        {
            let mut s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            s.is_indexing = true;
            s.index_freshness_cache = None;
        }

        let _writer_guard = match IndexWriterGuard::try_acquire(&storage_path) {
            Ok(guard) => guard,
            Err(error) => {
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };

        let result = match mode {
            IndexMode::Full => index_full_for_runtime(
                &root,
                &storage_path,
                &self.events_tx,
                cancel_token,
                &self.runtime_config,
                &self.source_index_policy,
            ),
            IndexMode::Incremental => index_incremental_for_runtime(
                &root,
                &storage_path,
                &self.events_tx,
                cancel_token,
                &self.runtime_config,
                &self.source_index_policy,
            ),
        };

        match result {
            Ok(summary) => self.finish_successful_indexing(
                summary,
                &storage_path,
                refresh_runtime_caches,
                cancel_token,
            ),
            Err(error) => {
                self.recover_failed_indexing(&storage_path, refresh_runtime_caches);
                Err(error)
            }
        }
    }

    fn finish_successful_indexing(
        &self,
        mut summary: IndexingRunSummary,
        storage_path: &Path,
        refresh_runtime_caches: bool,
        _cancel_token: Option<&CancellationToken>,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        if refresh_runtime_caches {
            #[cfg(test)]
            let boundary_result =
                publication_test_checkpoint(PublicationTestBoundary::RuntimeCache, _cancel_token);
            #[cfg(not(test))]
            let boundary_result: Result<(), ApiError> = Ok(());
            if let Err(error) = boundary_result {
                tracing::warn!(
                    error = %error.message,
                    "Runtime cache publication fault occurred after durable database commit; completing from the prepared generation"
                );
            }
        }
        let cache_refresh_started = Instant::now();
        let cache_stats_result = if let Some(prepared) = summary.prepared_search_state.take() {
            if refresh_runtime_caches {
                Ok(publish_prepared_search_state(self, prepared))
            } else {
                self.clear_search_state();
                self.state.lock().is_indexing = false;
                Ok(CacheRefreshStats {
                    search_stats: prepared.search_stats,
                    semantic_stats: prepared.semantic_stats,
                    runtime_cache_publish_ms: None,
                })
            }
        } else if refresh_runtime_caches {
            (|| {
                let mut storage = Storage::open(storage_path)
                    .map_err(|e| ApiError::internal(format!("Failed to reopen storage: {e}")))?;
                refresh_caches(
                    self,
                    &mut storage,
                    storage_path,
                    summary.llm_refresh_scope.as_ref(),
                )
            })()
        } else {
            self.finalize_indexing_without_runtime_refresh_with(
                storage_path,
                summary.llm_refresh_scope.as_ref(),
                |storage, llm_refresh_scope| {
                    rebuild_search_state_from_storage_for_runtime(
                        storage,
                        storage_path,
                        llm_refresh_scope,
                        false,
                        &self.runtime_config,
                        None,
                        None,
                    )
                    .map(|result| CacheRefreshStats {
                        search_stats: result.search_stats,
                        semantic_stats: result.semantic_stats,
                        runtime_cache_publish_ms: None,
                    })
                },
            )
        };
        let mut cache_stats = match cache_stats_result {
            Ok(cache_stats) => cache_stats,
            Err(error) => {
                self.clear_search_state();
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };
        summary.phase_timings.cache_refresh_ms = Some(clamp_u128_to_u32(
            cache_refresh_started.elapsed().as_millis(),
        ));
        if summary.staged_semantic_stats.reported {
            summary.staged_semantic_stats.reload_ms = cache_stats.semantic_stats.reload_ms;
            cache_stats.semantic_stats = summary.staged_semantic_stats;
        }
        apply_cache_refresh_stats(&mut summary.phase_timings, cache_stats);
        Ok(summary.phase_timings)
    }

    fn recover_failed_indexing(&self, storage_path: &Path, refresh_runtime_caches: bool) {
        if refresh_runtime_caches && let Ok(mut storage) = Storage::open(storage_path) {
            let incomplete = storage.has_incomplete_incremental_run().unwrap_or(true);
            if !incomplete {
                self.clear_search_state();
                let _ = refresh_caches(self, &mut storage, storage_path, None);
                return;
            }
        }
        self.clear_search_state();
        let mut state = self.state.lock();
        state.index_freshness_cache = None;
        state.is_indexing = false;
    }

    pub(crate) fn prepare_search_state_for_activation(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<(), ApiError> {
        let storage_path = self
            .state
            .lock()
            .storage_path
            .clone()
            .ok_or_else(no_project_error)?;
        let _writer_guard = IndexWriterGuard::try_acquire(&storage_path)?;
        if cancel_token.is_cancelled() {
            return Err(indexing_cancelled_error());
        }

        let mut storage = Storage::open(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to open core storage for search preparation: {error}"
            ))
        })?;
        let expected_publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read the complete core publication before search preparation: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "publication_changed",
                    "The complete core publication disappeared before search preparation.",
                )
            })?;
        let mut validate_before_completion =
            |prepared_publication: &IndexPublicationRecord| -> Result<(), ApiError> {
                if cancel_token.is_cancelled() {
                    return Err(indexing_cancelled_error());
                }

                #[cfg(test)]
                run_activation_search_before_revalidate_hook(&storage_path);

                let live_publication = Store::database_index_publication(&storage_path).map_err(
                    |error| {
                        ApiError::internal(format!(
                            "Failed to revalidate the core publication before search promotion: {error}"
                        ))
                    },
                )?;
                if prepared_publication != &expected_publication
                    || live_publication.as_ref() != Some(&expected_publication)
                {
                    return Err(ApiError::new(
                        "publication_changed",
                        "The core publication changed while its search generation was being prepared.",
                    ));
                }
                Ok(())
            };
        let prepared = rebuild_search_state_from_storage_for_runtime(
            &mut storage,
            &storage_path,
            None,
            false,
            &self.runtime_config,
            Some(cancel_token),
            Some(&mut validate_before_completion),
        )?;
        if cancel_token.is_cancelled() {
            return Err(indexing_cancelled_error());
        }

        let live_publication =
            Store::database_index_publication(&storage_path).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to revalidate the core publication after search preparation: {error}"
                ))
            })?;
        if prepared.publication.as_ref() != Some(&expected_publication)
            || live_publication.as_ref() != Some(&expected_publication)
        {
            drop(prepared);
            return Err(ApiError::new(
                "publication_changed",
                "The core publication changed while its search generation was being prepared.",
            ));
        }

        publish_prepared_search_state(self, prepared);
        Ok(())
    }

    pub(crate) fn complete_core_requires_publication_repair(
        &self,
        storage_path: &Path,
    ) -> Result<bool, ApiError> {
        if !storage_path.is_file() {
            return Ok(false);
        }
        let storage = Store::open_read_only(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect dense-anchor publication readiness: {error}"
            ))
        })?;
        let Some(publication) = storage.get_complete_index_publication().map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect dense-anchor core publication: {error}"
            ))
        })?
        else {
            return Ok(false);
        };
        if storage
            .validate_dense_anchor_publication(&publication)
            .is_err()
            || storage
                .validate_structural_text_unit_publication(&publication)
                .is_err()
        {
            return Ok(true);
        }
        let root = self.require_project_root()?;
        Ok(validate_source_policy_exclusions(
            &storage,
            &root,
            &publication,
            &self.source_index_policy,
        )
        .is_err())
    }

    pub fn ensure_incremental_refresh_compatible(&self) -> Result<(), ApiError> {
        let state = self.state.lock();
        let root = state.project_root.as_deref().ok_or_else(no_project_error)?;
        let storage_path = state.storage_path.as_deref().ok_or_else(no_project_error)?;
        ensure_incremental_refresh_compatible(root, storage_path)
    }

    pub fn ensure_incremental_refresh_compatible_at(
        &self,
        root: &Path,
        storage_path: &Path,
    ) -> Result<(), ApiError> {
        ensure_incremental_refresh_compatible(root, storage_path)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, true, None)
    }

    pub fn run_indexing_blocking_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, true, Some(cancel_token))
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, false, None)
    }

    pub fn run_indexing_blocking_without_runtime_refresh_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, false, Some(cancel_token))
    }

    pub fn republish_semantic_projections_blocking(
        &self,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_blocking_inner(None)
    }

    pub fn republish_semantic_projections_blocking_with_cancel(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_blocking_inner(Some(cancel_token))
    }

    fn republish_semantic_projections_blocking_inner(
        &self,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        let (root, storage_path) = {
            let state = self.state.lock();
            let root = state.project_root.clone().ok_or_else(no_project_error)?;
            let storage_path = state
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        self.republish_semantic_projections_at_blocking_inner(root, storage_path, cancel_token)
    }

    pub fn republish_semantic_projections_at_blocking(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_at_blocking_inner(root, storage_path, None)
    }

    fn republish_semantic_projections_at_blocking_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        if !root.is_dir() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist or is not a directory: {}",
                root.display()
            )));
        }
        {
            let mut state = self.state.lock();
            if state.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let changed =
                state.project_root.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &root)
                }) || state.storage_path.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &storage_path)
                });
            if changed {
                state.node_names.clear();
                clear_search_engine(&mut state);
            }
            state.project_root = Some(root.clone());
            state.storage_path = Some(storage_path.clone());
            state.is_indexing = true;
            state.index_freshness_cache = None;
        }
        let _writer_guard = match IndexWriterGuard::try_acquire(&storage_path) {
            Ok(guard) => guard,
            Err(error) => {
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };
        let result = semantic_projection_republish_for_runtime(
            &root,
            &storage_path,
            cancel_token,
            &self.runtime_config,
            &self.source_index_policy,
        );
        match result {
            Ok((
                summary,
                previous_publication,
                publication,
                symbol_document_count,
                dense_anchor_count,
            )) => {
                let phase_timings =
                    self.finish_successful_indexing(summary, &storage_path, true, cancel_token)?;
                Ok(SemanticProjectionRepublishOutcome {
                    previous_publication,
                    publication,
                    semantic_policy_version: SEMANTIC_POLICY_VERSION.to_string(),
                    symbol_document_count,
                    dense_anchor_count,
                    phase_timings,
                })
            }
            Err(error) => {
                let mut state = self.state.lock();
                state.is_indexing = false;
                state.index_freshness_cache = None;
                Err(error)
            }
        }
    }

    pub fn dry_run_index(&self, mode: IndexMode) -> Result<IndexDryRunDto, ApiError> {
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        if mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let refresh_inputs = if storage_path.exists() {
            let schema_version = Store::database_schema_version_observational(&storage_path)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to inspect dry-run storage without recovery: {error}"
                    ))
                })?;
            if schema_version < CURRENT_SCHEMA_VERSION {
                RefreshInputs::default()
            } else {
                let store =
                    Store::open_freshness_observational(&storage_path).map_err(|error| {
                        ApiError::internal(format!(
                            "Failed to inspect dry-run storage without mutation: {error}"
                        ))
                    })?;
                workspace_refresh_inputs(&store)?
            }
        } else {
            RefreshInputs::default()
        };
        let execution_plan = match mode {
            IndexMode::Full => {
                full_refresh_execution_plan_with_coverage(
                    &root,
                    &workspace,
                    &self.source_index_policy,
                )?
                .0
            }
            IndexMode::Incremental => {
                workspace
                    .build_execution_outcome_with_policy(&refresh_inputs, &self.source_index_policy)
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "Failed to generate incremental refresh plan: {e}"
                        ))
                    })?
                    .refresh
                    .plan
            }
        };
        let members =
            workspace_member_index_summaries(&root, &workspace, &refresh_inputs, &execution_plan);
        Ok(IndexDryRunDto {
            root: root.to_string_lossy().to_string(),
            storage_path: storage_path.to_string_lossy().to_string(),
            refresh: mode,
            files_to_index: execution_plan.files_to_index.len().min(u32::MAX as usize) as u32,
            files_to_remove: execution_plan.files_to_remove.len().min(u32::MAX as usize) as u32,
            sample_files_to_index: execution_plan
                .files_to_index
                .iter()
                .take(12)
                .map(|path| runtime_relative_path(&root, path))
                .collect(),
            sample_file_ids_to_remove: execution_plan
                .files_to_remove
                .iter()
                .take(12)
                .copied()
                .collect(),
            members,
        })
    }

    pub fn summarize_symbols_blocking(&self) -> Result<SummaryGenerationDto, ApiError> {
        let endpoint = self
            .runtime_config
            .summary
            .endpoint
            .clone()
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "--summarize requires CODESTORY_SUMMARY_ENDPOINT to be configured.",
                )
            })?;
        let model = self.runtime_config.summary.model.clone();
        let storage_path = self.require_storage_path()?;
        let mut storage = Store::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
        let docs = storage
            .get_all_llm_symbol_docs()
            .map_err(|e| ApiError::internal(format!("Failed to load symbol docs: {e}")))?;
        let current_summaries = storage
            .get_all_current_symbol_summaries()
            .map_err(|e| ApiError::internal(format!("Failed to load symbol summaries: {e}")))?;

        let mut generated = 0u32;
        let mut reused = 0u32;
        let mut skipped = 0u32;
        let mut pending = Vec::new();
        for doc in docs {
            if current_summaries.contains_key(&doc.node_id) {
                reused = reused.saturating_add(1);
                continue;
            }
            if doc.doc_text.trim().is_empty() {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let summary =
                summarize_symbol_doc(&endpoint, &model, &doc, &self.runtime_config.summary)?;
            pending.push(SymbolSummaryRecord {
                node_id: doc.node_id,
                content_hash: doc.doc_hash,
                summary,
                model: model.clone(),
                updated_at_epoch_ms: current_epoch_ms(),
            });
            generated = generated.saturating_add(1);

            if pending.len() >= 32 {
                storage
                    .upsert_symbol_summaries_batch(&pending)
                    .map_err(|e| {
                        ApiError::internal(format!("Failed to store symbol summaries: {e}"))
                    })?;
                pending.clear();
            }
        }
        storage
            .upsert_symbol_summaries_batch(&pending)
            .map_err(|e| ApiError::internal(format!("Failed to store symbol summaries: {e}")))?;

        Ok(SummaryGenerationDto {
            generated,
            reused,
            skipped,
            endpoint,
        })
    }

    fn finalize_indexing_without_runtime_refresh_with<F>(
        &self,
        storage_path: &Path,
        llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
        rebuild: F,
    ) -> Result<CacheRefreshStats, ApiError>
    where
        F: FnOnce(
            &mut Storage,
            Option<&HashSet<codestory_contracts::graph::NodeId>>,
        ) -> Result<CacheRefreshStats, ApiError>,
    {
        let result = (|| {
            let mut storage = Storage::open(storage_path)
                .map_err(|e| ApiError::internal(format!("Failed to reopen storage: {e}")))?;
            rebuild(&mut storage, llm_refresh_scope)
        })();

        self.clear_search_state();
        self.state.lock().is_indexing = false;

        result
    }

    pub fn indexed_files(&self, req: IndexedFilesRequest) -> Result<IndexedFilesDto, ApiError> {
        self.ensure_consistent_read_state("Files")?;
        let root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        let publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read source policy exclusion publication identity: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "source_verification_failed",
                    "Indexed-file coverage requires a complete core publication.",
                )
            })?;
        validate_source_policy_exclusions(
            &storage,
            &root,
            &publication,
            &self.source_index_policy,
        )?;
        validate_structural_text_units(&storage, &publication)?;
        let source_policy_exclusions = storage.get_source_policy_exclusions().map_err(|error| {
            ApiError::internal(format!("Failed to load source policy exclusions: {error}"))
        })?;
        let mut files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load indexed files: {e}")))?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let errors = storage
            .get_errors(None)
            .map_err(|e| ApiError::internal(format!("Failed to load index errors: {e}")))?;
        let verified_file_ids = storage
            .files()
            .inventory()
            .map_err(|e| ApiError::internal(format!("Failed to load file inventory: {e}")))?
            .into_iter()
            .filter_map(|file| file.content_hash.map(|_| file.id))
            .collect::<HashSet<_>>();
        let mut errors_by_file = HashMap::<i64, u32>::new();
        let mut coverage_reasons_by_file = HashMap::<i64, Vec<FileCoverageReason>>::new();
        for error in errors {
            if let Some(file_id) = error.file_id {
                *errors_by_file.entry(file_id.0).or_default() += 1;
                coverage_reasons_by_file.entry(file_id.0).or_default().push(
                    error
                        .coverage_reason
                        .unwrap_or(FileCoverageReason::CollectorFailure),
                );
            }
        }

        let mut language_counts = BTreeMap::<String, u32>::new();
        let mut incomplete_reason_counts = BTreeMap::<String, (u32, String)>::new();
        let mut indexed_file_count = 0_u32;
        let mut incomplete_file_count = 0_u32;
        let mut error_file_count = 0_u32;
        for file in &files {
            *language_counts.entry(file.language.clone()).or_default() += 1;
            indexed_file_count += u32::from(file.indexed);
            incomplete_file_count += u32::from(!file.complete);
            error_file_count += u32::from(errors_by_file.contains_key(&file.id));
            if let Some(reason) = file_coverage_reason(
                file,
                &coverage_reasons_by_file,
                verified_file_ids.contains(&file.id),
            ) {
                let entry = incomplete_reason_counts
                    .entry(reason.as_str().to_string())
                    .or_insert_with(|| (0, file_coverage_detail(reason).to_string()));
                entry.0 += 1;
            }
        }
        let coverage_gaps = files
            .iter()
            .filter_map(|file| {
                let verified_source = verified_file_ids.contains(&file.id);
                file_coverage_reason(file, &coverage_reasons_by_file, verified_source).map(
                    |reason| FileCoverageDiagnosticDto {
                        path: runtime_relative_path(&root, &file.path),
                        reason,
                        retryable: file_coverage_retryable(reason),
                        verified_source,
                        projection_available: file.indexed && verified_source,
                    },
                )
            })
            .collect::<Vec<_>>();

        let path_filter = req.path_contains.as_deref().map(normalize_path_key);
        let language_filter = req.language.as_deref().map(str::to_ascii_lowercase);
        let policy_exclusion_count = source_policy_exclusions.len().min(u32::MAX as usize) as u32;
        let mut policy_exclusions = source_policy_exclusions
            .into_iter()
            .filter(|entry| {
                let role = path_role_from_key(&normalize_path_key(&entry.normalized_path));
                req.role.is_none_or(|requested| requested == role)
                    && path_filter.as_deref().is_none_or(|needle| {
                        normalize_path_key(&entry.normalized_path).contains(needle)
                    })
                    && language_filter.as_deref().is_none_or(|language| {
                        indexed_file_matches_language_filter(
                            "unknown",
                            Path::new(&entry.normalized_path),
                            language,
                        )
                    })
            })
            .map(|entry| SourcePolicyExclusionDto {
                role: path_role_from_key(&normalize_path_key(&entry.normalized_path)),
                path: entry.normalized_path,
                content_hash: entry.content_hash,
                observed_size: entry.observed_size,
                observed_unit_count: entry.observed_unit_count,
                policy_version: entry.policy_version,
                byte_cap: entry.byte_cap,
                structural_unit_cap: entry.structural_unit_cap,
                project_id: entry.project_id,
                workspace_id: entry.workspace_id,
                core_generation_id: entry.core_generation_id,
                core_run_id: entry.core_run_id,
                graph_coverage: false,
                semantic_coverage: false,
            })
            .collect::<Vec<_>>();
        policy_exclusions.truncate(5_000);
        let mut visible = files
            .into_iter()
            .filter(|file| {
                let role = indexed_file_role(&file.path);
                req.role.is_none_or(|requested| requested == role)
                    && path_filter.as_deref().is_none_or(|needle| {
                        normalize_path_key(&runtime_relative_path(&root, &file.path))
                            .contains(needle)
                    })
                    && language_filter.as_deref().is_none_or(|language| {
                        indexed_file_matches_language_filter(&file.language, &file.path, language)
                    })
            })
            .map(|file| IndexedFileDto {
                path: runtime_relative_path(&root, &file.path),
                language: file.language,
                indexed: file.indexed,
                complete: file.complete,
                line_count: file.line_count,
                role: indexed_file_role(&file.path),
                error_count: errors_by_file.get(&file.id).copied().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let limit = req.limit.unwrap_or(500).clamp(1, 5000) as usize;
        let filtered_file_count = visible.len().min(u32::MAX as usize) as u32;
        let truncated = visible.len() > limit;
        visible.truncate(limit);
        let visible_file_count = visible.len().min(u32::MAX as usize) as u32;

        let mut coverage_notes = Vec::new();
        if incomplete_file_count > 0 || error_file_count > 0 {
            coverage_notes.push(format!(
                "index usable with {incomplete_file_count} incomplete files and {error_file_count} files carrying index errors"
            ));
        } else {
            coverage_notes.push("index usable; no file-level index errors recorded".to_string());
        }
        if policy_exclusion_count > 0 {
            coverage_notes.push(format!(
                "{policy_exclusion_count} verified source policy exclusions have no parser-backed graph or semantic coverage"
            ));
        }
        let language_counts = language_counts
            .into_iter()
            .map(|(language, file_count)| {
                let support = language_support_summary_for_language(&language);
                IndexedFileLanguageCountDto {
                    language,
                    file_count,
                    support_mode: support.support_mode,
                    evidence_tier: support.evidence_tier,
                    claim_label: support.claim_label,
                }
            })
            .collect::<Vec<_>>();
        let incomplete_reason_counts = incomplete_reason_counts
            .into_iter()
            .map(
                |(reason, (file_count, detail))| IndexedFileIncompleteReasonCountDto {
                    reason,
                    file_count,
                    detail,
                },
            )
            .collect::<Vec<_>>();
        let file_count = language_counts
            .iter()
            .map(|entry| entry.file_count)
            .sum::<u32>();

        Ok(IndexedFilesDto {
            project_root: root.to_string_lossy().to_string(),
            usable: indexed_file_count > 0,
            summary: IndexedFilesSummaryDto {
                file_count,
                indexed_file_count,
                filtered_file_count,
                visible_file_count,
                incomplete_file_count,
                error_file_count,
                policy_exclusion_count,
                incomplete_reason_counts,
                truncated,
                language_counts,
                framework_route_coverage: framework_route_coverage_matrix(),
                coverage_notes,
            },
            coverage_gaps,
            policy_exclusions,
            files: visible,
        })
    }

    pub(crate) fn index_freshness(&self) -> Result<IndexFreshnessDto, ApiError> {
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        let storage = self.open_storage_for_freshness()?;
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let freshness =
            self.cached_index_freshness_from_storage(&root, &storage_path, &workspace, &storage);
        Ok(freshness)
    }

    /// Return the durable identity of the core database generation at the live path.
    pub fn index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        let storage_path = self.require_storage_path()?;
        Store::database_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read index publication identity: {error}"
            ))
        })
    }

    /// Return the durable publication only when the live database is not fenced
    /// by an incomplete legacy incremental run.
    pub fn complete_index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        let storage_path = self.require_storage_path()?;
        Store::database_complete_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read complete index publication: {error}"
            ))
        })
    }

    fn cached_index_freshness_from_storage(
        &self,
        root: &Path,
        storage_path: &Path,
        workspace: &WorkspaceManifest,
        storage: &Storage,
    ) -> IndexFreshnessDto {
        if !matches!(storage.has_incomplete_incremental_run(), Ok(false)) {
            self.state.lock().index_freshness_cache = None;
            return index_freshness_from_storage_with_policy(
                root,
                workspace,
                storage,
                &self.source_index_policy,
            );
        }
        let ttl = Duration::from_secs(index_freshness_cache_ttl_secs());
        let storage_fingerprint = storage_fingerprint(storage_path);
        {
            let state = self.state.lock();
            if let Some(cached) = state.index_freshness_cache.as_ref()
                && cached.root == root
                && cached.storage_path == storage_path
                && cached.storage_fingerprint == storage_fingerprint
                && cached.cached_at.elapsed() < ttl
            {
                return cached.value.clone();
            }
        }

        let freshness = index_freshness_from_storage_with_policy(
            root,
            workspace,
            storage,
            &self.source_index_policy,
        );
        let mut state = self.state.lock();
        state.index_freshness_cache = Some(CachedIndexFreshness {
            root: root.to_path_buf(),
            storage_path: storage_path.to_path_buf(),
            storage_fingerprint,
            value: freshness.clone(),
            cached_at: Instant::now(),
        });
        freshness
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

    pub fn list_bookmark_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        let categories = storage
            .get_bookmark_categories()
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark categories: {e}")))?;
        Ok(categories
            .into_iter()
            .map(|category| BookmarkCategoryDto {
                id: category.id.to_string(),
                name: category.name,
            })
            .collect())
    }

    pub fn create_bookmark_category(
        &self,
        req: CreateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }

        let storage = self.open_storage()?;
        let id = storage
            .create_bookmark_category(name)
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark category: {e}")))?;
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub fn update_bookmark_category(
        &self,
        id: i64,
        req: UpdateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }
        let storage = self.open_storage()?;
        let updated = storage
            .rename_bookmark_category(id, name)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark category: {e}")))?;
        if !updated {
            return Err(ApiError::not_found(format!(
                "Bookmark category not found: {id}"
            )));
        }
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark_category(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark category: {e}")))?;
        Ok(())
    }

    pub fn list_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        let bookmarks = storage
            .get_bookmarks(category_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmarks: {e}")))?;

        let mut response = Vec::with_capacity(bookmarks.len());
        for bookmark in bookmarks {
            let node = storage
                .get_node(bookmark.node_id)
                .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;
            let (node_label, node_kind, file_path) = match node {
                Some(node) => (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    Self::file_path_for_node(&storage, &node)?,
                ),
                None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
            };
            response.push(BookmarkDto {
                id: bookmark.id.to_string(),
                category_id: bookmark.category_id.to_string(),
                node_id: NodeId::from(bookmark.node_id),
                comment: bookmark.comment,
                node_label,
                node_kind,
                file_path,
            });
        }
        Ok(response)
    }

    pub fn create_bookmark(&self, req: CreateBookmarkRequest) -> Result<BookmarkDto, ApiError> {
        let node_id = req.node_id.to_core()?;
        let category_id = parse_db_id(&req.category_id, "category_id")?;
        let storage = self.open_storage()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;
        let bookmark_id = storage
            .add_bookmark(category_id, node_id, req.comment.as_deref())
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark: {e}")))?;

        Ok(BookmarkDto {
            id: bookmark_id.to_string(),
            category_id: category_id.to_string(),
            node_id: NodeId::from(node_id),
            comment: req.comment,
            node_label: node_display_name(&node),
            node_kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(&storage, &node)?,
        })
    }

    pub fn update_bookmark(
        &self,
        id: i64,
        req: UpdateBookmarkRequest,
    ) -> Result<BookmarkDto, ApiError> {
        let storage = self.open_storage()?;
        let category_id = req
            .category_id
            .as_deref()
            .map(|raw| parse_db_id(raw, "category_id"))
            .transpose()?;
        let comment_patch = req.comment.as_ref().map(|value| value.as_deref());
        storage
            .update_bookmark(id, category_id, comment_patch)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark: {e}")))?;
        let bookmark = storage
            .get_bookmarks(None)
            .map_err(|e| ApiError::internal(format!("Failed to reload bookmarks: {e}")))?
            .into_iter()
            .find(|bookmark| bookmark.id == id)
            .ok_or_else(|| ApiError::not_found(format!("Bookmark not found: {id}")))?;
        let node = storage
            .get_node(bookmark.node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;

        let (node_label, node_kind, file_path) = match node {
            Some(node) => (
                node_display_name(&node),
                NodeKind::from(node.kind),
                Self::file_path_for_node(&storage, &node)?,
            ),
            None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
        };

        Ok(BookmarkDto {
            id: bookmark.id.to_string(),
            category_id: bookmark.category_id.to_string(),
            node_id: NodeId::from(bookmark.node_id),
            comment: bookmark.comment,
            node_label,
            node_kind,
            file_path,
        })
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark: {e}")))?;
        Ok(())
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

fn is_low_confidence_search_plan_bridge(bridge: &SearchPlanBridgeDto) -> bool {
    bridge.confidence == SearchPlanBridgeConfidenceDto::Low
}

struct IndexingRunSummary {
    phase_timings: IndexingPhaseTimings,
    staged_semantic_stats: SemanticProjectionStats,
    llm_refresh_scope: Option<HashSet<codestory_contracts::graph::NodeId>>,
    #[cfg(test)]
    publication: IndexPublicationRecord,
    prepared_search_state: Option<SearchStateBuildResult>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FullRefreshWallDurations {
    live_inspection: Duration,
    source_discovery: Duration,
    stage_open: Duration,
    indexer_execution: Duration,
    coverage_validation: Duration,
    copy_forward: Duration,
    semantic_stage: Duration,
    snapshot_stage: Duration,
    publication_prepare: Duration,
    search_generation: Duration,
    catalog_publication: Duration,
}

impl FullRefreshWallDurations {
    fn finish(self, core_refresh: Duration) -> FullRefreshWallTimings {
        let accounted = [
            self.live_inspection,
            self.source_discovery,
            self.stage_open,
            self.indexer_execution,
            self.coverage_validation,
            self.copy_forward,
            self.semantic_stage,
            self.snapshot_stage,
            self.publication_prepare,
            self.search_generation,
            self.catalog_publication,
        ]
        .into_iter()
        .fold(Duration::ZERO, Duration::saturating_add);
        let unattributed = core_refresh.saturating_sub(accounted);

        FullRefreshWallTimings {
            core_refresh_ms: clamp_u128_to_u32(core_refresh.as_millis()),
            live_inspection_ms: clamp_u128_to_u32(self.live_inspection.as_millis()),
            source_discovery_ms: clamp_u128_to_u32(self.source_discovery.as_millis()),
            stage_open_ms: clamp_u128_to_u32(self.stage_open.as_millis()),
            indexer_execution_ms: clamp_u128_to_u32(self.indexer_execution.as_millis()),
            coverage_validation_ms: clamp_u128_to_u32(self.coverage_validation.as_millis()),
            copy_forward_ms: clamp_u128_to_u32(self.copy_forward.as_millis()),
            semantic_stage_ms: clamp_u128_to_u32(self.semantic_stage.as_millis()),
            snapshot_stage_ms: clamp_u128_to_u32(self.snapshot_stage.as_millis()),
            publication_prepare_ms: clamp_u128_to_u32(self.publication_prepare.as_millis()),
            search_generation_ms: clamp_u128_to_u32(self.search_generation.as_millis()),
            catalog_publication_ms: clamp_u128_to_u32(self.catalog_publication.as_millis()),
            unattributed_ms: clamp_u128_to_u32(unattributed.as_millis()),
        }
    }
}

fn next_index_publication(
    previous: Option<&IndexPublicationRecord>,
    mode: IndexPublicationMode,
    run_id: &str,
) -> Result<IndexPublicationRecord, ApiError> {
    let generation = previous
        .map(|publication| publication.generation)
        .unwrap_or_default()
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("Index publication generation overflow"))?;
    Ok(IndexPublicationRecord {
        generation,
        generation_id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        mode,
        published_at_epoch_ms: current_epoch_ms(),
    })
}

fn index_publication_dto(publication: IndexPublicationRecord) -> IndexPublicationDto {
    IndexPublicationDto {
        generation: publication.generation,
        generation_id: publication.generation_id,
        run_id: publication.run_id,
        mode: match publication.mode {
            IndexPublicationMode::Full => IndexPublicationModeDto::Full,
            IndexPublicationMode::Incremental => IndexPublicationModeDto::Incremental,
            IndexPublicationMode::SemanticProjection => IndexPublicationModeDto::SemanticProjection,
        },
        published_at_epoch_ms: publication.published_at_epoch_ms,
    }
}

struct IndexWriterGuard {
    file: std::fs::File,
    path: PathBuf,
}

impl IndexWriterGuard {
    fn try_acquire(storage_path: &Path) -> Result<Self, ApiError> {
        let path = storage_path.with_extension("index-writer.lock");
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create index writer lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to open index writer lock {}: {error}",
                    path.display()
                ))
            })?;
        if !FileExt::try_lock_exclusive(&file).map_err(|error| {
            ApiError::internal(format!(
                "Failed to acquire index writer lock {}: {error}",
                path.display()
            ))
        })? {
            return Err(ApiError::new(
                "cache_busy",
                format!(
                    "Another indexing run owns the writer lock at {}. Wait for it to finish and retry.",
                    path.display()
                ),
            ));
        }
        Ok(Self { file, path })
    }
}

impl Drop for IndexWriterGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock index writer lock: {error}"
            );
        }
    }
}

fn semantic_projection_republish_for_runtime(
    root: &Path,
    storage_path: &Path,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<
    (
        IndexingRunSummary,
        IndexPublicationRecord,
        IndexPublicationRecord,
        u32,
        u64,
    ),
    ApiError,
> {
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    if !storage_path.is_file() {
        return Err(ApiError::new(
            "semantic_projection_core_missing",
            "Semantic projection republish requires an existing complete core publication.",
        ));
    }

    let expected_schema_version =
        Store::database_schema_version(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to pin the stored core schema version: {error}"
            ))
        })?;

    let expected_publication = Store::database_complete_index_publication(storage_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to pin the complete core publication: {error}"
            ))
        })?
        .ok_or_else(|| {
            ApiError::new(
                "semantic_projection_core_incomplete",
                "Semantic projection republish requires a complete core publication.",
            )
        })?;
    let mut staged = SnapshotStore::clone_live_to_staged(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to clone the pinned core for semantic projection republish: {error}"
        ))
    })?;
    let cleanup_staged_path = staged.path().to_path_buf();
    let result = (|| {
        let staged_publication = staged
            .store_mut()
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to validate the staged core publication: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "semantic_projection_core_incomplete",
                    "The staged core publication is incomplete.",
                )
            })?;
        if staged_publication != expected_publication {
            return Err(ApiError::new(
                "publication_changed",
                "The cloned core does not match the pinned live publication.",
            ));
        }
        staged
            .store_mut()
            .validate_dense_anchor_publication(&expected_publication)
            .map_err(|error| {
                ApiError::new(
                    "semantic_projection_migration_required",
                    format!("Pinned dense-anchor publication is not complete: {error}"),
                )
            })?;
        let structural_compatibility = staged
            .store_mut()
            .validate_structural_text_unit_publication_or_legacy_empty(&expected_publication)
            .map_err(|error| {
                ApiError::new(
                    "semantic_projection_migration_required",
                    format!("Pinned structural state is not compatible: {error}"),
                )
            })?;
        if structural_compatibility == StructuralTextPublicationCompatibility::LegacyEmpty
            && expected_schema_version != LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION
        {
            return Err(ApiError::new(
                "semantic_projection_migration_required",
                "A missing structural publication is compatible only with a schema-29 retained core whose structural stores are empty.",
            ));
        }
        let source_manifest = staged
            .store_mut()
            .get_source_policy_exclusion_manifest()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to load the pinned source-policy manifest: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "semantic_projection_migration_required",
                    "Pinned source-policy publication is missing.",
                )
            })?;
        let recorded_source_policy = SourcePolicyExclusionPolicyIdentity::new(
            &source_manifest.policy_version,
            source_manifest.byte_cap,
            source_manifest.structural_unit_cap,
        );
        let selected_identity = project_identity_v3(root);
        if source_manifest.project_id != selected_identity.project_id
            || source_manifest.workspace_id != selected_identity.workspace_id
        {
            return Err(ApiError::new(
                "semantic_projection_project_mismatch",
                "The selected project root does not own the cached core publication.",
            ));
        }
        let source_policy_compatibility = semantic_projection_source_policy_compatibility(
            recorded_source_policy,
            source_index_policy,
            expected_schema_version,
            structural_compatibility == StructuralTextPublicationCompatibility::LegacyEmpty,
        )
        .ok_or_else(|| {
            ApiError::new(
                "semantic_projection_migration_required",
                "Pinned source-policy identity differs from the current runtime policy; run a source refresh before republishing semantic projections.",
            )
        })?;
        let source_policy_validation = match source_policy_compatibility {
            SemanticProjectionSourcePolicyCompatibility::Exact => staged
                .store_mut()
                .validate_source_policy_exclusion_publication(
                    &expected_publication,
                    &source_manifest.project_id,
                    &source_manifest.workspace_id,
                    recorded_source_policy,
                ),
            SemanticProjectionSourcePolicyCompatibility::LegacyPredecessor => staged
                .store_mut()
                .validate_legacy_v1_source_policy_exclusion_publication(
                    &expected_publication,
                    &source_manifest.project_id,
                    &source_manifest.workspace_id,
                    recorded_source_policy,
                ),
        };
        source_policy_validation.map_err(|error| {
            ApiError::new(
                "semantic_projection_migration_required",
                format!("Pinned source-policy publication is not complete: {error}"),
            )
        })?;
        let source_exclusions =
            staged
                .store_mut()
                .get_source_policy_exclusions()
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to load pinned source-policy exclusions: {error}"
                    ))
                })?;

        let publication = next_index_publication(
            Some(&expected_publication),
            IndexPublicationMode::SemanticProjection,
            &Uuid::new_v4().to_string(),
        )?;
        let source_identity = format!("core:{}:{}", publication.generation_id, publication.run_id);
        staged
            .store_mut()
            .begin_incremental_run()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to fence the staged semantic projection writer: {error}"
                ))
            })?;
        staged
            .store_mut()
            .invalidate_grounding_snapshots()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to invalidate staged derived snapshots: {error}"
                ))
            })?;

        let staged_semantic_stats = finalize_staged_semantic_docs_for_runtime(
            staged.store_mut(),
            None,
            None,
            &source_identity,
            cancel_token,
            runtime,
            SemanticProjectionDocumentSource::StoredCore,
        )?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let staged_finalize_stats = staged.snapshots().finalize_staged().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finalize staged semantic projection snapshots: {error}"
            ))
        })?;
        #[cfg(test)]
        publication_test_checkpoint(
            PublicationTestBoundary::ProjectionSnapshotFinalize,
            cancel_token,
        )?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let detail_started = Instant::now();
        staged.snapshots().refresh_detail().map_err(|error| {
            ApiError::internal(format!(
                "Failed to refresh staged grounding detail snapshot: {error}"
            ))
        })?;
        #[cfg(test)]
        publication_test_checkpoint(
            PublicationTestBoundary::ProjectionSnapshotDetail,
            cancel_token,
        )?;
        let detail_snapshot_ms = clamp_u128_to_u32(detail_started.elapsed().as_millis());
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }

        #[cfg(test)]
        publication_test_checkpoint(
            PublicationTestBoundary::ProjectionManifestIdentity,
            cancel_token,
        )?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let dense_manifest = staged
            .store_mut()
            .publish_dense_anchor_generation(&publication, SEMANTIC_POLICY_VERSION)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to publish semantic dense-anchor inputs: {error}"
                ))
            })?;
        let current_source_policy = SourcePolicyExclusionPolicyIdentity::new(
            &source_index_policy.policy_version,
            source_index_policy.byte_cap,
            source_index_policy.structural_unit_cap,
        );
        let source_candidates = source_exclusions
            .iter()
            .map(|record| {
                let mut candidate = source_policy_exclusion_candidate(record);
                candidate.policy_version = source_index_policy.policy_version.clone();
                candidate.byte_cap = source_index_policy.byte_cap;
                candidate.structural_unit_cap = source_index_policy.structural_unit_cap;
                candidate
            })
            .collect::<Vec<_>>();
        staged
            .store_mut()
            .publish_source_policy_exclusion_generation(
                &publication,
                &selected_identity.project_id,
                &selected_identity.workspace_id,
                current_source_policy,
                &source_candidates,
            )
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to rebind pinned source-policy exclusions: {error}"
                ))
            })?;
        staged
            .store_mut()
            .publish_structural_text_unit_generation(&publication)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to rebind pinned structural publication: {error}"
                ))
            })?;
        staged
            .store_mut()
            .put_index_publication(&publication)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to persist staged semantic projection identity: {error}"
                ))
            })?;

        let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
            staged.store_mut(),
            storage_path,
            None,
            false,
            runtime,
            cancel_token,
            None,
        ) {
            Ok(state) => state,
            Err(error) => {
                discard_unpublished_search_generation(storage_path, &publication);
                return Err(error);
            }
        };
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        let staged_path = staged.path().to_path_buf();
        #[cfg(test)]
        if let Err(error) =
            publication_test_checkpoint(PublicationTestBoundary::CatalogLock, cancel_token)
        {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        #[cfg(test)]
        run_semantic_projection_before_revalidate_hook(storage_path);
        let _catalog_guard = match SearchGenerationCatalogGuard::acquire(storage_path) {
            Ok(guard) => guard,
            Err(error) => {
                drop(prepared_search_state);
                discard_unpublished_search_generation(storage_path, &publication);
                return Err(error);
            }
        };
        let live_publication = match Store::database_complete_index_publication(storage_path) {
            Ok(publication) => publication,
            Err(error) => {
                drop(prepared_search_state);
                discard_unpublished_search_generation(storage_path, &publication);
                return Err(ApiError::internal(format!(
                    "Failed to revalidate the pinned core before promotion: {error}"
                )));
            }
        };
        if live_publication.as_ref() != Some(&expected_publication) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(ApiError::new(
                "publication_changed",
                "The live core changed while semantic projections were being rebuilt.",
            ));
        }
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        #[cfg(test)]
        if let Err(error) =
            publication_test_checkpoint(PublicationTestBoundary::MarkerCompletion, cancel_token)
        {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        staged
            .store_mut()
            .finish_incremental_run()
            .map_err(|error| {
                discard_unpublished_search_generation(storage_path, &publication);
                ApiError::internal(format!(
                    "Failed to complete the staged semantic projection marker: {error}"
                ))
            })?;

        let publish_started = Instant::now();
        #[cfg(test)]
        if let Err(error) =
            publication_test_checkpoint(PublicationTestBoundary::DatabaseReplacement, cancel_token)
        {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        let staged_publish_stats = staged.publish_with_stats(storage_path).map_err(|error| {
            discard_unpublished_search_generation(storage_path, &publication);
            ApiError::internal(format!(
                "Failed to publish staged semantic projections: {error}. Preserved staged snapshot at {}",
                staged_path.display()
            ))
        })?;
        let mut phase_timings = IndexingPhaseTimings {
            deferred_indexes_ms: Some(
                staged_finalize_stats
                    .deferred_indexes_ms
                    .saturating_add(staged_semantic_stats.semantic_context_index_ms),
            ),
            summary_snapshot_ms: Some(staged_finalize_stats.summary_snapshot_ms),
            detail_snapshot_ms: Some(detail_snapshot_ms),
            publish_ms: Some(clamp_u128_to_u32(publish_started.elapsed().as_millis())),
            staged_sqlite_wal_autocheckpoint_bytes: staged_publish_stats
                .sqlite_wal_autocheckpoint_bytes,
            staged_sqlite_checkpoint_ms: staged_publish_stats.sqlite_checkpoint_ms,
            staged_sqlite_sync_ms: staged_publish_stats.sqlite_sync_ms,
            staged_snapshot_copy: staged_publish_stats
                .snapshot_copy
                .map(database_snapshot_copy_timings),
            core_promotion: Some(core_promotion_timings(staged_publish_stats.core_promotion)),
            ..Default::default()
        };
        apply_semantic_projection_stats(&mut phase_timings, staged_semantic_stats);
        Ok((
            IndexingRunSummary {
                phase_timings,
                staged_semantic_stats,
                llm_refresh_scope: None,
                #[cfg(test)]
                publication: publication.clone(),
                prepared_search_state: Some(prepared_search_state),
            },
            publication,
            staged_semantic_stats.symbol_search_docs_written,
            dense_manifest.anchor_count,
        ))
    })();

    match result {
        Ok((summary, publication, symbol_document_count, dense_anchor_count)) => Ok((
            summary,
            expected_publication,
            publication,
            symbol_document_count,
            dense_anchor_count,
        )),
        Err(error) => {
            let _ = SnapshotStore::discard_staged(&cleanup_staged_path);
            Err(error)
        }
    }
}

fn index_full_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    let core_refresh_started = Instant::now();
    let mut wall_durations = FullRefreshWallDurations::default();
    let mut wall_stage_started = Instant::now();
    let previous_publication = if storage_path.exists() {
        Store::database_index_publication(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect live publication identity: {error}"
            ))
        })?
    } else {
        None
    };
    let publication_run_id = Uuid::new_v4().to_string();
    let publication = next_index_publication(
        previous_publication.as_ref(),
        IndexPublicationMode::Full,
        &publication_run_id,
    )?;
    let dense_anchor_source_identity =
        format!("core:{}:{}", publication.generation_id, publication.run_id);
    let recovering_incomplete_run = if storage_path.exists() {
        match Storage::database_schema_version(storage_path) {
            Ok(version) if version > codestory_store::CURRENT_SCHEMA_VERSION => {
                Storage::database_has_incomplete_incremental_run(storage_path).map_err(|error| {
                    ApiError::internal(format!("Failed to inspect live storage: {error}"))
                })?
            }
            Ok(_) => match Storage::database_has_incomplete_incremental_run(storage_path) {
                Ok(marked) => marked,
                Err(error) => {
                    tracing::warn!(
                        path = %storage_path.display(),
                        "Live storage could not be inspected; rebuilding without copying derived state: {error}"
                    );
                    true
                }
            },
            Err(error) => {
                tracing::warn!(
                    path = %storage_path.display(),
                    "Live storage schema could not be read; rebuilding without copying derived state: {error}"
                );
                true
            }
        }
    } else {
        false
    };
    let has_verified_live_publication = !recovering_incomplete_run
        && previous_publication.as_ref().is_some_and(|expected| {
            let live = match Store::open_read_only(storage_path) {
                Ok(storage) => storage,
                Err(error) => {
                    tracing::debug!(
                        path = %storage_path.display(),
                        "Live publication could not be opened for verification: {error}"
                    );
                    return false;
                }
            };
            match live.get_complete_index_publication() {
                Ok(Some(publication)) if publication == *expected => {
                    match live.validate_dense_anchor_publication(&publication) {
                        Ok(_) => {
                            validate_structural_text_units(&live, &publication).is_ok()
                                && validate_source_policy_exclusions(
                                    &live,
                                    root,
                                    &publication,
                                    source_index_policy,
                                )
                                .is_ok()
                        }
                        Err(error) => {
                            tracing::debug!(
                                path = %storage_path.display(),
                                "Live dense anchor publication could not be verified: {error}"
                            );
                            false
                        }
                    }
                }
                Ok(_) => false,
                Err(error) => {
                    tracing::debug!(
                        path = %storage_path.display(),
                        "Live core publication could not be verified: {error}"
                    );
                    false
                }
            }
        });
    wall_durations.live_inspection = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let workspace = runtime_workspace_manifest(root, storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
    let (execution_plan, mut policy_exclusions) =
        full_refresh_execution_plan_with_coverage(root, &workspace, source_index_policy)?;

    wall_durations.source_discovery = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });

    #[cfg(test)]
    run_source_policy_after_plan_hook();

    let mut staged = SnapshotStore::open_disposable_full_refresh(storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to open staged storage: {e}")))?;
    #[cfg(test)]
    run_full_refresh_staged_store_hook(staged.store_mut());
    let can_copy_forward = !recovering_incomplete_run && storage_path.exists();
    let copied_structural_artifacts = if has_verified_live_publication {
        match staged
            .store_mut()
            .copy_structural_text_artifact_cache_from(storage_path)
        {
            Ok(copied) => {
                tracing::debug!(
                    copied,
                    "Copied verified structural artifacts into staged storage"
                );
                copied
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to copy verified structural artifacts into staged storage; recollecting: {error}"
                );
                0
            }
        }
    } else {
        0
    };

    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());
    let indexer = V2WorkspaceIndexer::new(root.to_path_buf())
        .with_source_index_policy(source_index_policy.clone())
        .with_artifact_cache_policies(ArtifactCachePolicies {
            parser: ArtifactCachePolicy::KnownEmpty,
            structural: if copied_structural_artifacts > 0 {
                ArtifactCachePolicy::ReadThrough
            } else {
                ArtifactCachePolicy::KnownEmpty
            },
        });
    wall_durations.stage_open = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let result =
        indexer.run_with_policy_exclusions(staged.store_mut(), &execution_plan, &bus, cancel_token);

    drop(bus);
    let _ = forwarder.join();

    let index_stats = match result {
        Ok(_) if is_indexing_cancelled(cancel_token) => {
            let _ = staged.discard();
            return Err(indexing_cancelled_error());
        }
        Ok(outcome) => {
            policy_exclusions.extend(outcome.policy_exclusions);
            outcome.stats
        }
        Err(_) if is_indexing_cancelled(cancel_token) => {
            let _ = staged.discard();
            return Err(indexing_cancelled_error());
        }
        Err(err) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!("Indexing failed: {err}")));
        }
    };
    wall_durations.indexer_execution = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let coverage_gaps = match stored_file_coverage_diagnostics(root, staged.store_mut()) {
        Ok(coverage_gaps) => coverage_gaps,
        Err(error) => {
            let _ = staged.discard();
            return Err(error);
        }
    };
    let blocking_gaps = coverage_gaps
        .iter()
        .filter(|entry| entry.reason != FileCoverageReason::ParserPartial)
        .cloned()
        .collect::<Vec<_>>();
    if !blocking_gaps.is_empty() {
        let sample = blocking_gaps
            .iter()
            .take(3)
            .map(|entry| format!("{} ({})", entry.path, entry.reason.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let remainder = blocking_gaps.len().saturating_sub(3);
        let sample = if remainder > 0 {
            format!("{sample}, and {remainder} more")
        } else {
            sample
        };
        let preserved_state = if has_verified_live_publication {
            "The previous complete publication was preserved"
        } else if recovering_incomplete_run {
            "The existing live index and its incomplete-run recovery fence were preserved"
        } else if previous_publication.is_some() {
            "The existing live index was preserved and no replacement publication was created"
        } else {
            "No core publication was created"
        };
        let count = blocking_gaps.len();
        let code = source_coverage_failure_code(&blocking_gaps);
        let _ = staged.discard();
        return Err(ApiError::source_coverage_failure(
            code,
            format!(
                "Effective refresh mode `full` could not verify {count} scheduled file(s): {sample}. {preserved_state}."
            ),
            blocking_gaps,
        ));
    }
    wall_durations.coverage_validation = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    if can_copy_forward {
        match staged
            .store_mut()
            .copy_retrieval_artifact_nodes_from(storage_path)
        {
            Ok(copied) => {
                tracing::debug!(
                    copied,
                    "Copied retrieval artifact nodes into staged storage"
                )
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to copy retrieval artifact nodes into staged storage: {error}"
                )
            }
        }
        match staged
            .store_mut()
            .copy_symbol_search_docs_from(storage_path)
        {
            Ok(copied) => tracing::debug!(copied, "Copied symbol docs into staged storage"),
            Err(error) => {
                tracing::warn!("Failed to copy symbol docs into staged storage: {error}")
            }
        }
        match staged
            .store_mut()
            .copy_dense_anchor_inputs_from(storage_path)
        {
            Ok(copied) => tracing::debug!(copied, "Copied dense anchor inputs into staged storage"),
            Err(error) => {
                tracing::warn!("Failed to copy dense anchor inputs into staged storage: {error}")
            }
        }
    }
    wall_durations.copy_forward = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let staged_semantic_stats = match finalize_staged_semantic_docs_for_runtime(
        staged.store_mut(),
        None,
        None,
        &dense_anchor_source_identity,
        cancel_token,
        runtime,
        SemanticProjectionDocumentSource::SourceFiles,
    ) {
        Ok(stats) => stats,
        Err(error) => {
            let _ = staged.discard();
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        let _ = staged.discard();
        return Err(indexing_cancelled_error());
    }
    wall_durations.semantic_stage = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let staged_finalize_stats = match staged.snapshots().finalize_staged() {
        Ok(stats) => stats,
        Err(err) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!(
                "Failed to finalize staged snapshot lifecycle: {err}"
            )));
        }
    };
    let deferred_indexes_ms = staged_finalize_stats
        .deferred_indexes_ms
        .saturating_add(staged_semantic_stats.semantic_context_index_ms);
    let summary_snapshot_ms = staged_finalize_stats.summary_snapshot_ms;
    let detail_started = Instant::now();
    if let Err(err) = staged.snapshots().refresh_detail() {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to finalize staged detail snapshots: {err}"
        )));
    }
    let detail_snapshot_ms = Some(clamp_u128_to_u32(detail_started.elapsed().as_millis()));
    if is_indexing_cancelled(cancel_token) {
        let _ = staged.discard();
        return Err(indexing_cancelled_error());
    }
    wall_durations.snapshot_stage = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    if recovering_incomplete_run && let Err(err) = staged.store_mut().begin_incremental_run() {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to preserve incomplete marker through staged recovery: {err}"
        )));
    }
    #[cfg(test)]
    if let Err(error) = publication_test_checkpoint(PublicationTestBoundary::Identity, cancel_token)
    {
        let _ = staged.discard();
        return Err(error);
    }
    if let Err(error) = staged
        .store_mut()
        .publish_dense_anchor_generation(&publication, SEMANTIC_POLICY_VERSION)
    {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to publish complete dense anchor inputs: {error}"
        )));
    }
    #[cfg(test)]
    run_source_policy_before_revalidate_hook();
    let policy_exclusions = match revalidate_source_policy_exclusions(
        &workspace,
        &policy_exclusions,
        source_index_policy,
    ) {
        Ok(exclusions) => exclusions,
        Err(error) => {
            let _ = staged.discard();
            return Err(error);
        }
    };
    if let Err(error) = publish_source_policy_exclusions(
        staged.store_mut(),
        root,
        &publication,
        &policy_exclusions,
        source_index_policy,
    ) {
        let _ = staged.discard();
        return Err(error);
    }
    if let Err(error) = staged
        .store_mut()
        .publish_structural_text_unit_generation(&publication)
    {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to publish complete structural text units: {error}"
        )));
    }
    if let Err(error) = staged.store_mut().put_index_publication(&publication) {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to persist staged full publication identity: {error}"
        )));
    }
    wall_durations.publication_prepare = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
        staged.store_mut(),
        storage_path,
        None,
        false,
        runtime,
        cancel_token,
        None,
    ) {
        Ok(state) => state,
        Err(error) => {
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    wall_durations.search_generation = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let staged_path = staged.path().to_path_buf();
    #[cfg(test)]
    if let Err(error) =
        publication_test_checkpoint(PublicationTestBoundary::CatalogLock, cancel_token)
    {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(error);
    }
    let _catalog_guard = match SearchGenerationCatalogGuard::acquire(storage_path) {
        Ok(guard) => guard,
        Err(error) => {
            drop(prepared_search_state);
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    if recovering_incomplete_run {
        #[cfg(test)]
        if let Err(error) =
            publication_test_checkpoint(PublicationTestBoundary::MarkerCompletion, cancel_token)
        {
            drop(prepared_search_state);
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
        if is_indexing_cancelled(cancel_token) {
            drop(prepared_search_state);
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(indexing_cancelled_error());
        }
        if let Err(error) = staged.store_mut().finish_incremental_run() {
            drop(prepared_search_state);
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(ApiError::internal(format!(
                "Failed to complete staged full-recovery marker: {error}"
            )));
        }
    }
    let publish_started = std::time::Instant::now();
    #[cfg(test)]
    if let Err(error) =
        publication_test_checkpoint(PublicationTestBoundary::DatabaseReplacement, cancel_token)
    {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(error);
    }
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    let staged_publish_stats = match staged.publish_with_stats(storage_path) {
        Ok(stats) => stats,
        Err(err) => {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(ApiError::internal(format!(
                "Failed to publish staged storage: {err}. Preserved staged snapshot at {}",
                staged_path.display()
            )));
        }
    };
    let publish_ms = clamp_u128_to_u32(publish_started.elapsed().as_millis());
    wall_durations.catalog_publication = wall_stage_started.elapsed();
    let full_refresh_wall = wall_durations.finish(core_refresh_started.elapsed());
    let resolution_telemetry = OptionalResolutionTelemetry::from_incremental_stats(&index_stats);
    let full_refresh_pipeline_enabled = index_stats.full_refresh_queue_capacity > 0;
    let full_refresh_chunking_enabled = index_stats.full_refresh_chunk_target_bytes > 0;
    Ok(IndexingRunSummary {
        phase_timings: IndexingPhaseTimings {
            full_refresh_wall: Some(full_refresh_wall),
            parse_index_ms: clamp_u64_to_u32(index_stats.parse_index_ms),
            projection_flush_ms: clamp_u64_to_u32(index_stats.projection_flush_ms),
            edge_resolution_ms: clamp_u64_to_u32(index_stats.edge_resolution_ms),
            error_flush_ms: clamp_u64_to_u32(index_stats.error_flush_ms),
            cleanup_ms: clamp_u64_to_u32(index_stats.cleanup_ms),
            artifact_cache_write_ms: Some(clamp_u64_to_u32(index_stats.artifact_cache_write_ms)),
            artifact_cache_writes: Some(clamp_usize_to_u32(index_stats.artifact_cache_writes)),
            artifact_cache_write_transactions: Some(clamp_usize_to_u32(
                index_stats.artifact_cache_write_transactions,
            )),
            parser_artifact_cache: Some(artifact_cache_access_timings(
                &index_stats.parser_artifact_cache,
            )),
            structural_artifact_cache: Some(artifact_cache_access_timings(
                &index_stats.structural_artifact_cache,
            )),
            full_refresh_chunks_produced: full_refresh_pipeline_enabled
                .then_some(clamp_usize_to_u32(index_stats.full_refresh_chunks_produced)),
            full_refresh_chunks_persisted: full_refresh_pipeline_enabled.then_some(
                clamp_usize_to_u32(index_stats.full_refresh_chunks_persisted),
            ),
            full_refresh_queue_capacity: full_refresh_pipeline_enabled
                .then_some(clamp_usize_to_u32(index_stats.full_refresh_queue_capacity)),
            full_refresh_queue_high_water: full_refresh_pipeline_enabled.then_some(
                clamp_usize_to_u32(index_stats.full_refresh_queue_high_water),
            ),
            full_refresh_producer_blocked_ms: full_refresh_pipeline_enabled.then_some(
                clamp_u64_to_u32(index_stats.full_refresh_producer_blocked_ms),
            ),
            full_refresh_writer_idle_ms: full_refresh_pipeline_enabled
                .then_some(clamp_u64_to_u32(index_stats.full_refresh_writer_idle_ms)),
            full_refresh_chunk_target_bytes: full_refresh_chunking_enabled
                .then_some(index_stats.full_refresh_chunk_target_bytes),
            full_refresh_chunk_target_nodes: full_refresh_chunking_enabled.then_some(
                clamp_usize_to_u32(index_stats.full_refresh_chunk_target_nodes),
            ),
            full_refresh_chunk_file_ceiling: full_refresh_chunking_enabled.then_some(
                clamp_usize_to_u32(index_stats.full_refresh_chunk_file_ceiling),
            ),
            full_refresh_chunk_max_files: full_refresh_chunking_enabled
                .then_some(clamp_usize_to_u32(index_stats.full_refresh_chunk_max_files)),
            full_refresh_chunk_max_planned_bytes: full_refresh_chunking_enabled
                .then_some(index_stats.full_refresh_chunk_max_planned_bytes),
            full_refresh_chunk_max_nodes: full_refresh_chunking_enabled
                .then_some(clamp_usize_to_u32(index_stats.full_refresh_chunk_max_nodes)),
            full_refresh_chunk_budget_overruns: full_refresh_chunking_enabled.then_some(
                clamp_usize_to_u32(index_stats.full_refresh_chunk_budget_overruns),
            ),
            full_refresh_chunk_planning_ms: full_refresh_chunking_enabled
                .then_some(clamp_u64_to_u32(index_stats.full_refresh_chunk_planning_ms)),
            source_prepare_ms: Some(clamp_u64_to_u32(index_stats.source_prepare_ms)),
            projection_batch_wall_ms: Some(clamp_u64_to_u32(index_stats.projection_batch_wall_ms)),
            projection_batch_transactions: Some(clamp_usize_to_u32(
                index_stats.projection_batch_transactions,
            )),
            projection_persistence: Some(projection_persistence_timings(
                &index_stats.projection_persistence,
            )),
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_stream_ms: None,
            search_symbol_stream_rows: None,
            search_symbol_stream_batches: None,
            search_symbol_index_ms: None,
            search_symbol_index_docs_written: None,
            search_symbol_index_writer_count: None,
            search_symbol_index_commit_count: None,
            search_symbol_index_reload_count: None,
            search_symbol_index_commit_ms: None,
            search_symbol_index_reload_ms: None,
            runtime_cache_publish_ms: None,
            semantic_context_index_ms: None,
            semantic_node_load_ms: None,
            semantic_node_load_rows: None,
            semantic_node_stream_batches: None,
            semantic_endpoint_load_ms: None,
            semantic_endpoint_load_rows: None,
            semantic_endpoint_load_batches: None,
            semantic_selected_nodes: None,
            semantic_context_file_count: None,
            semantic_context_path_bytes: None,
            semantic_node_lookup_entries: None,
            semantic_context_ms: None,
            semantic_doc_build_ms: None,
            semantic_embedding_ms: None,
            semantic_db_upsert_ms: None,
            semantic_reload_ms: None,
            semantic_prune_ms: None,
            semantic_docs_reused: None,
            semantic_docs_embedded: None,
            semantic_docs_pending: None,
            semantic_docs_stale: None,
            symbol_search_docs_written: None,
            semantic_dense_docs_skipped: None,
            semantic_dense_public_api: None,
            semantic_dense_entrypoint: None,
            semantic_dense_documented_nontrivial: None,
            semantic_dense_central_graph_node: None,
            semantic_dense_component_report: None,
            semantic_dense_unstructured_doc: None,
            deferred_indexes_ms: Some(deferred_indexes_ms),
            summary_snapshot_ms: Some(summary_snapshot_ms),
            detail_snapshot_ms,
            publish_ms: Some(publish_ms),
            staged_sqlite_wal_autocheckpoint_bytes: staged_publish_stats
                .sqlite_wal_autocheckpoint_bytes,
            staged_sqlite_checkpoint_ms: staged_publish_stats.sqlite_checkpoint_ms,
            staged_sqlite_sync_ms: staged_publish_stats.sqlite_sync_ms,
            staged_snapshot_copy: staged_publish_stats
                .snapshot_copy
                .map(database_snapshot_copy_timings),
            core_promotion: Some(core_promotion_timings(staged_publish_stats.core_promotion)),
            setup_existing_projection_ids_ms: resolution_telemetry.setup_existing_projection_ids_ms,
            setup_seed_symbol_table_ms: resolution_telemetry.setup_seed_symbol_table_ms,
            flush_files_ms: resolution_telemetry.flush_files_ms,
            flush_nodes_ms: resolution_telemetry.flush_nodes_ms,
            flush_edges_ms: resolution_telemetry.flush_edges_ms,
            flush_occurrences_ms: resolution_telemetry.flush_occurrences_ms,
            flush_component_access_ms: resolution_telemetry.flush_component_access_ms,
            flush_callable_projection_ms: resolution_telemetry.flush_callable_projection_ms,
            unresolved_calls_start: clamp_usize_to_u32(index_stats.unresolved_calls_start),
            unresolved_imports_start: clamp_usize_to_u32(index_stats.unresolved_imports_start),
            resolved_calls: clamp_usize_to_u32(index_stats.resolved_calls),
            resolved_imports: clamp_usize_to_u32(index_stats.resolved_imports),
            unresolved_calls_end: clamp_usize_to_u32(index_stats.unresolved_calls_end),
            unresolved_imports_end: clamp_usize_to_u32(index_stats.unresolved_imports_end),
            resolution_override_count_ms: resolution_telemetry.resolution_override_count_ms,
            resolution_unresolved_counts_ms: resolution_telemetry.resolution_unresolved_counts_ms,
            resolution_calls_ms: resolution_telemetry.resolution_calls_ms,
            resolution_imports_ms: resolution_telemetry.resolution_imports_ms,
            resolution_cleanup_ms: resolution_telemetry.resolution_cleanup_ms,
            resolution_call_candidate_index_ms: resolution_telemetry
                .resolution_call_candidate_index_ms,
            resolution_import_candidate_index_ms: resolution_telemetry
                .resolution_import_candidate_index_ms,
            resolution_call_semantic_index_ms: resolution_telemetry
                .resolution_call_semantic_index_ms,
            resolution_import_semantic_index_ms: resolution_telemetry
                .resolution_import_semantic_index_ms,
            resolution_support_snapshot_limit_bytes: resolution_telemetry
                .resolution_support_snapshot_limit_bytes,
            resolution_support_snapshot_stored: resolution_telemetry
                .resolution_support_snapshot_stored,
            resolution_support_snapshot_skipped_oversize: resolution_telemetry
                .resolution_support_snapshot_skipped_oversize,
            resolution_call_semantic_candidates_ms: resolution_telemetry
                .resolution_call_semantic_candidates_ms,
            resolution_import_semantic_candidates_ms: resolution_telemetry
                .resolution_import_semantic_candidates_ms,
            resolution_call_semantic_requests: resolution_telemetry
                .resolution_call_semantic_requests,
            resolution_call_semantic_unique_requests: resolution_telemetry
                .resolution_call_semantic_unique_requests,
            resolution_call_semantic_skipped_requests: resolution_telemetry
                .resolution_call_semantic_skipped_requests,
            resolution_import_semantic_requests: resolution_telemetry
                .resolution_import_semantic_requests,
            resolution_import_semantic_unique_requests: resolution_telemetry
                .resolution_import_semantic_unique_requests,
            resolution_import_semantic_skipped_requests: resolution_telemetry
                .resolution_import_semantic_skipped_requests,
            resolution_call_compute_ms: resolution_telemetry.resolution_call_compute_ms,
            resolution_import_compute_ms: resolution_telemetry.resolution_import_compute_ms,
            resolution_call_apply_ms: resolution_telemetry.resolution_call_apply_ms,
            resolution_import_apply_ms: resolution_telemetry.resolution_import_apply_ms,
            resolution_override_resolution_ms: resolution_telemetry
                .resolution_override_resolution_ms,
            resolved_calls_same_file: resolution_telemetry.resolved_calls_same_file,
            resolved_calls_same_module: resolution_telemetry.resolved_calls_same_module,
            resolved_calls_global_unique: resolution_telemetry.resolved_calls_global_unique,
            resolved_calls_semantic: resolution_telemetry.resolved_calls_semantic,
            resolved_imports_same_file: resolution_telemetry.resolved_imports_same_file,
            resolved_imports_same_module: resolution_telemetry.resolved_imports_same_module,
            resolved_imports_global_unique: resolution_telemetry.resolved_imports_global_unique,
            resolved_imports_fuzzy: resolution_telemetry.resolved_imports_fuzzy,
            resolved_imports_semantic: resolution_telemetry.resolved_imports_semantic,
        },
        staged_semantic_stats,
        llm_refresh_scope: None,
        #[cfg(test)]
        publication,
        prepared_search_state: Some(prepared_search_state),
    })
}

#[cfg(test)]
fn index_incremental(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
) -> Result<IndexingRunSummary, ApiError> {
    index_incremental_for_runtime(
        root,
        storage_path,
        events_tx,
        cancel_token,
        &test_sidecar_runtime_from_env(),
        &SourceIndexPolicy::default(),
    )
}

fn index_incremental_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    run_incremental_indexing_common(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
    )
}

fn spawn_progress_forwarder(
    rx: Receiver<Event>,
    progress_tx: Sender<AppEventPayload>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                Event::IndexingProgress { current, total } => {
                    let _ = progress_tx.send(AppEventPayload::IndexingProgress {
                        current: current.min(u32::MAX as usize) as u32,
                        total: total.min(u32::MAX as usize) as u32,
                    });
                }
                Event::StatusUpdate { message } => {
                    let _ = progress_tx.send(AppEventPayload::StatusUpdate { message });
                }
                _ => {}
            }
        }
    })
}

const FULL_REFRESH_REQUIRED_ERROR_CODE: &str = "full_refresh_required";

fn full_refresh_required_error(
    root: &Path,
    reason_code: &str,
    reason: impl AsRef<str>,
) -> ApiError {
    let project = root.to_string_lossy().to_string();
    let next_command = format!(
        "codestory-cli index --project {} --refresh full",
        quote_refresh_command_argument(&project)
    );
    ApiError::with_details(
        FULL_REFRESH_REQUIRED_ERROR_CODE,
        format!(
            "Refresh compatibility rejected the request before workspace reads: requested=incremental effective=none required=full reason={}",
            reason.as_ref()
        ),
        ApiErrorDetails {
            cause_code: Some(reason_code.to_string()),
            failed_layer: Some("core_publication_compatibility".to_string()),
            project: Some(project),
            next_commands: vec![next_command.clone()],
            minimum_next: vec![next_command.clone()],
            full_repair: vec![next_command],
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps: Vec::new(),
        },
    )
}

#[cfg(windows)]
fn quote_refresh_command_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_refresh_command_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ensure_incremental_refresh_compatible(root: &Path, storage_path: &Path) -> Result<(), ApiError> {
    if !storage_path.is_file() {
        return Err(full_refresh_required_error(
            root,
            "complete_core_publication_missing",
            "complete_core_publication_missing",
        ));
    }
    let schema_version = Store::database_schema_version_observational(storage_path).map_err(
        |error| {
            ApiError::internal(format!(
                "Failed to inspect incremental refresh schema compatibility without recovery: {error}"
            ))
        },
    )?;
    if schema_version < CURRENT_SCHEMA_VERSION {
        let (reason_code, reason) = if schema_version == 0 {
            (
                "complete_core_publication_missing",
                "complete_core_publication_missing".to_string(),
            )
        } else {
            (
                "core_schema_upgrade_required",
                format!(
                    "core_schema_upgrade_required:observed={schema_version}:required={CURRENT_SCHEMA_VERSION}"
                ),
            )
        };
        return Err(full_refresh_required_error(root, reason_code, reason));
    }
    let storage = Store::open_freshness_observational(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect incremental refresh compatibility: {error}"
        ))
    })?;
    if storage.has_incomplete_incremental_run().map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect incomplete incremental marker: {error}"
        ))
    })? {
        return Err(full_refresh_required_error(
            root,
            "incomplete_incremental_publication",
            "incomplete_incremental_publication",
        ));
    }
    let Some(publication) = storage.get_complete_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect complete core publication: {error}"
        ))
    })?
    else {
        return Err(full_refresh_required_error(
            root,
            "complete_core_publication_missing",
            "complete_core_publication_missing",
        ));
    };
    if let Err(error) = storage.validate_structural_text_unit_publication(&publication) {
        return Err(full_refresh_required_error(
            root,
            "structural_publication_incompatible",
            format!("structural_publication_incompatible:{error}"),
        ));
    }
    Ok(())
}

fn run_incremental_indexing_common(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    ensure_incremental_refresh_compatible(root, storage_path)?;
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }

    let mut staged = SnapshotStore::clone_live_to_staged(storage_path).map_err(|e| {
        ApiError::internal(format!(
            "Failed to clone live storage for incremental build: {e}"
        ))
    })?;
    let previous_publication = match staged.store_mut().get_index_publication() {
        Ok(publication) => publication,
        Err(error) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!(
                "Failed to read staged publication identity: {error}"
            )));
        }
    };
    let rebuild_complete_dense_anchor_set = staged
        .store_mut()
        .get_dense_anchor_publication_manifest()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to read staged dense anchor publication identity: {error}"
            ))
        })?
        .is_none();
    let publication_run_id = Uuid::new_v4().to_string();
    let publication = next_index_publication(
        previous_publication.as_ref(),
        IndexPublicationMode::Incremental,
        &publication_run_id,
    )?;
    let dense_anchor_source_identity =
        format!("core:{}:{}", publication.generation_id, publication.run_id);
    let staged_result = (|| {
        staged.store_mut().begin_incremental_run().map_err(|e| {
            ApiError::internal(format!(
                "Failed to persist staged incomplete index marker: {e}"
            ))
        })?;
        staged
            .store_mut()
            .invalidate_grounding_snapshots()
            .map_err(|e| {
                ApiError::internal(format!(
                    "Failed to invalidate staged derived index snapshots: {e}"
                ))
            })?;

        let workspace = runtime_workspace_manifest(root, storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let refresh_inputs = workspace_refresh_inputs(staged.store_mut())?;
        let policy_refresh = workspace
            .build_execution_outcome_with_policy(&refresh_inputs, source_index_policy)
            .map_err(|e| ApiError::internal(format!("Failed to generate refresh info: {e}")))?;
        if policy_refresh.refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
            let reason = if policy_refresh.refresh.inventory_outcome
                == WorkspaceInventoryOutcome::Unreadable
            {
                FileCoverageReason::Unreadable
            } else {
                FileCoverageReason::DiscoveryIncomplete
            };
            let mut gaps = policy_refresh
                .refresh
                .inventory_issues
                .iter()
                .map(|issue| FileCoverageDiagnosticDto {
                    path: runtime_relative_path(root, &issue.path),
                    reason,
                    retryable: file_coverage_retryable(reason),
                    verified_source: false,
                    projection_available: false,
                })
                .collect::<Vec<_>>();
            if gaps.is_empty() {
                gaps.push(FileCoverageDiagnosticDto {
                    path: ".".into(),
                    reason,
                    retryable: file_coverage_retryable(reason),
                    verified_source: false,
                    projection_available: false,
                });
            }
            return Err(ApiError::source_coverage_failure(
                source_coverage_failure_code(&gaps),
                format!(
                    "Incremental refresh requires a complete source inventory; discovery was {:?}.",
                    policy_refresh.refresh.inventory_outcome
                ),
                gaps,
            ));
        }
        let execution_plan = policy_refresh.refresh.plan;
        let mut policy_exclusions = policy_refresh.policy_exclusions;
        let mut planned_semantic_seed_file_ids = execution_plan
            .files_to_remove
            .iter()
            .copied()
            .map(codestory_contracts::graph::NodeId)
            .collect::<HashSet<_>>();
        let mut previous_indexed_file_ids_by_path = HashMap::new();
        let mut policy_excluded_semantic_seed_file_ids = HashSet::new();
        for path in &execution_plan.files_to_index {
            let normalized_path = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };
            if let Some(file_info) = staged
                .store_mut()
                .get_file_by_path(&normalized_path)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to resolve previous semantic scope for {}: {error}",
                        normalized_path.display()
                    ))
                })?
            {
                let file_id = codestory_contracts::graph::NodeId(file_info.id);
                planned_semantic_seed_file_ids.insert(file_id);
                previous_indexed_file_ids_by_path
                    .insert(runtime_relative_path(root, &normalized_path), file_id);
            }
        }
        let previous_semantic_dependents_by_seed = semantic_graph_dependent_file_ids_by_seed(
            staged.store_mut(),
            &planned_semantic_seed_file_ids,
        )?;
        let existing_file_paths = semantic_file_table_path_map(
            staged
                .store_mut()
                .get_files()
                .map_err(|error| ApiError::internal(format!("Failed to load files: {error}")))?,
        );
        let mut removed_component_keys = HashSet::new();
        for file_id in &execution_plan.files_to_remove {
            let path = existing_file_paths
                .get(&codestory_contracts::graph::NodeId(*file_id))
                .ok_or_else(|| {
                    ApiError::internal(format!(
                        "Removed file is missing from staged component scope: {file_id}"
                    ))
                })?;
            if let Some(component_key) = semantic_component_key_for_path(Some(path)) {
                removed_component_keys.insert(component_key);
            }
        }
        let mut component_report_refresh = ComponentReportRefreshScope {
            previous_file_paths: existing_file_paths,
            removed_component_keys,
        };

        let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
        let _ = events_tx.send(AppEventPayload::IndexingStarted {
            file_count: total_files,
        });

        #[cfg(test)]
        run_incremental_staged_store_hook(staged.store_mut());
        let bus = EventBus::new();
        let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());
        if is_indexing_cancelled(cancel_token) {
            drop(bus);
            let _ = forwarder.join();
            return Err(indexing_cancelled_error());
        }
        let indexer = V2WorkspaceIndexer::new(root.to_path_buf())
            .with_source_index_policy(source_index_policy.clone());
        let result = indexer.run_with_policy_exclusions(
            staged.store_mut(),
            &execution_plan,
            &bus,
            cancel_token,
        );

        // Drop bus so forwarder unblocks.
        drop(bus);
        let _ = forwarder.join();

        let index_stats = match result {
            Ok(_) if is_indexing_cancelled(cancel_token) => {
                return Err(indexing_cancelled_error());
            }
            Ok(outcome) => {
                for exclusion in &outcome.policy_exclusions {
                    if let Some(file_id) =
                        previous_indexed_file_ids_by_path.get(&exclusion.normalized_path)
                    {
                        policy_excluded_semantic_seed_file_ids.insert(*file_id);
                        if let Some(component_key) = component_report_refresh
                            .previous_file_paths
                            .get(file_id)
                            .and_then(|path| semantic_component_key_for_path(Some(path)))
                        {
                            component_report_refresh
                                .removed_component_keys
                                .insert(component_key);
                        }
                    }
                }
                policy_exclusions.extend(outcome.policy_exclusions);
                outcome.stats
            }
            Err(_) if is_indexing_cancelled(cancel_token) => {
                return Err(indexing_cancelled_error());
            }
            Err(e) => return Err(ApiError::internal(format!("Indexing failed: {e}"))),
        };
        let blocking_gaps = stored_file_coverage_diagnostics(root, staged.store_mut())?
            .into_iter()
            .filter(|entry| entry.reason != FileCoverageReason::ParserPartial)
            .collect::<Vec<_>>();
        if !blocking_gaps.is_empty() {
            let count = blocking_gaps.len();
            let sample = blocking_gaps
                .iter()
                .take(3)
                .map(|entry| format!("{} ({})", entry.path, entry.reason.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ApiError::source_coverage_failure(
                source_coverage_failure_code(&blocking_gaps),
                format!(
                    "Incremental refresh could not verify {count} scheduled file(s): {sample}. The previous complete publication was preserved."
                ),
                blocking_gaps,
            ));
        }
        let mut semantic_refresh_seed_file_ids = HashSet::new();
        for path in &execution_plan.files_to_index {
            let normalized_path = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };
            let file_info = staged
                .store_mut()
                .get_file_by_path(&normalized_path)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to resolve indexed semantic scope for {}: {error}",
                        normalized_path.display()
                    ))
                })?;
            // Workspace refresh plans include discovered files that have no graph
            // collector, while incomplete reads preserve the last verified graph.
            // Only complete files can prove that their semantic projection changed.
            if let Some(file_info) = file_info
                && file_info.complete
            {
                semantic_refresh_seed_file_ids
                    .insert(codestory_contracts::graph::NodeId(file_info.id));
            }
        }
        semantic_refresh_seed_file_ids.extend(
            execution_plan
                .files_to_remove
                .iter()
                .copied()
                .map(codestory_contracts::graph::NodeId),
        );
        semantic_refresh_seed_file_ids.extend(policy_excluded_semantic_seed_file_ids);
        let current_semantic_dependents_by_seed = semantic_graph_dependent_file_ids_by_seed(
            staged.store_mut(),
            &semantic_refresh_seed_file_ids,
        )?;
        let mut llm_refresh_scope = semantic_refresh_seed_file_ids.clone();
        for seed_file_id in &semantic_refresh_seed_file_ids {
            if let Some(file_ids) = previous_semantic_dependents_by_seed.get(seed_file_id) {
                llm_refresh_scope.extend(file_ids.iter().copied());
            }
            if let Some(file_ids) = current_semantic_dependents_by_seed.get(seed_file_id) {
                llm_refresh_scope.extend(file_ids.iter().copied());
            }
        }
        let staged_semantic_stats = finalize_staged_semantic_docs_for_runtime(
            staged.store_mut(),
            (!rebuild_complete_dense_anchor_set).then_some(&llm_refresh_scope),
            (!rebuild_complete_dense_anchor_set).then_some(&component_report_refresh),
            &dense_anchor_source_identity,
            cancel_token,
            runtime,
            SemanticProjectionDocumentSource::SourceFiles,
        )?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let staged_finalize_stats = staged.snapshots().finalize_staged().map_err(|e| {
            ApiError::internal(format!(
                "Failed to finalize staged incremental storage: {e}"
            ))
        })?;
        let detail_started = Instant::now();
        staged.snapshots().refresh_detail().map_err(|e| {
            ApiError::internal(format!(
                "Failed to refresh staged grounding detail snapshot: {e}"
            ))
        })?;
        let detail_snapshot_ms = clamp_u128_to_u32(detail_started.elapsed().as_millis());
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        Ok((
            index_stats,
            staged_finalize_stats,
            detail_snapshot_ms,
            llm_refresh_scope,
            staged_semantic_stats,
            policy_exclusions,
        ))
    })();
    let (
        index_stats,
        staged_finalize_stats,
        detail_snapshot_ms,
        llm_refresh_scope,
        staged_semantic_stats,
        policy_exclusions,
    ) = match staged_result {
        Ok(result) => result,
        Err(error) => {
            let _ = staged.discard();
            return Err(error);
        }
    };
    let deferred_indexes_ms = staged_finalize_stats
        .deferred_indexes_ms
        .saturating_add(staged_semantic_stats.semantic_context_index_ms);
    let summary_snapshot_ms = staged_finalize_stats.summary_snapshot_ms;
    let resolution_telemetry = OptionalResolutionTelemetry::from_incremental_stats(&index_stats);

    if is_indexing_cancelled(cancel_token) {
        let _ = staged.discard();
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    if let Err(error) = publication_test_checkpoint(PublicationTestBoundary::Identity, cancel_token)
    {
        let _ = staged.discard();
        return Err(error);
    }
    if let Err(error) = staged
        .store_mut()
        .publish_dense_anchor_generation(&publication, SEMANTIC_POLICY_VERSION)
    {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to publish complete dense anchor inputs: {error}"
        )));
    }
    #[cfg(test)]
    run_source_policy_before_revalidate_hook();
    let workspace = runtime_workspace_manifest(root, storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to reopen project: {error}")))?;
    let policy_exclusions = match revalidate_source_policy_exclusions(
        &workspace,
        &policy_exclusions,
        source_index_policy,
    ) {
        Ok(exclusions) => exclusions,
        Err(error) => {
            let _ = staged.discard();
            return Err(error);
        }
    };
    if let Err(error) = publish_source_policy_exclusions(
        staged.store_mut(),
        root,
        &publication,
        &policy_exclusions,
        source_index_policy,
    ) {
        let _ = staged.discard();
        return Err(error);
    }
    if let Err(error) = staged
        .store_mut()
        .publish_structural_text_unit_generation(&publication)
    {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to publish complete structural text units: {error}"
        )));
    }
    if let Err(error) = staged.store_mut().put_index_publication(&publication) {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to persist staged incremental publication identity: {error}"
        )));
    }
    let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
        staged.store_mut(),
        storage_path,
        Some(&llm_refresh_scope),
        false,
        runtime,
        cancel_token,
        None,
    ) {
        Ok(state) => state,
        Err(error) => {
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    let staged_path = staged.path().to_path_buf();
    #[cfg(test)]
    if let Err(error) =
        publication_test_checkpoint(PublicationTestBoundary::CatalogLock, cancel_token)
    {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(error);
    }
    let _catalog_guard = match SearchGenerationCatalogGuard::acquire(storage_path) {
        Ok(guard) => guard,
        Err(error) => {
            drop(prepared_search_state);
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    if let Err(error) =
        publication_test_checkpoint(PublicationTestBoundary::MarkerCompletion, cancel_token)
    {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(error);
    }
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    if let Err(error) = staged.store_mut().finish_incremental_run() {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(ApiError::internal(format!(
            "Failed to complete staged incremental marker: {error}"
        )));
    }
    let publish_started = Instant::now();
    #[cfg(test)]
    if let Err(error) =
        publication_test_checkpoint(PublicationTestBoundary::DatabaseReplacement, cancel_token)
    {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(error);
    }
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    let staged_publish_stats = match staged.publish_with_stats(storage_path) {
        Ok(stats) => stats,
        Err(error) => {
            drop(prepared_search_state);
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(ApiError::internal(format!(
                "Failed to publish staged incremental storage: {error}. Preserved staged snapshot at {}",
                staged_path.display()
            )));
        }
    };
    let publish_ms = clamp_u128_to_u32(publish_started.elapsed().as_millis());

    Ok(IndexingRunSummary {
        phase_timings: IndexingPhaseTimings {
            full_refresh_wall: None,
            parse_index_ms: clamp_u64_to_u32(index_stats.parse_index_ms),
            projection_flush_ms: clamp_u64_to_u32(index_stats.projection_flush_ms),
            edge_resolution_ms: clamp_u64_to_u32(index_stats.edge_resolution_ms),
            error_flush_ms: clamp_u64_to_u32(index_stats.error_flush_ms),
            cleanup_ms: clamp_u64_to_u32(index_stats.cleanup_ms),
            artifact_cache_write_ms: Some(clamp_u64_to_u32(index_stats.artifact_cache_write_ms)),
            artifact_cache_writes: Some(clamp_usize_to_u32(index_stats.artifact_cache_writes)),
            artifact_cache_write_transactions: Some(clamp_usize_to_u32(
                index_stats.artifact_cache_write_transactions,
            )),
            parser_artifact_cache: Some(artifact_cache_access_timings(
                &index_stats.parser_artifact_cache,
            )),
            structural_artifact_cache: Some(artifact_cache_access_timings(
                &index_stats.structural_artifact_cache,
            )),
            full_refresh_chunks_produced: None,
            full_refresh_chunks_persisted: None,
            full_refresh_queue_capacity: None,
            full_refresh_queue_high_water: None,
            full_refresh_producer_blocked_ms: None,
            full_refresh_writer_idle_ms: None,
            full_refresh_chunk_target_bytes: None,
            full_refresh_chunk_target_nodes: None,
            full_refresh_chunk_file_ceiling: None,
            full_refresh_chunk_max_files: None,
            full_refresh_chunk_max_planned_bytes: None,
            full_refresh_chunk_max_nodes: None,
            full_refresh_chunk_budget_overruns: None,
            full_refresh_chunk_planning_ms: None,
            source_prepare_ms: Some(clamp_u64_to_u32(index_stats.source_prepare_ms)),
            projection_batch_wall_ms: Some(clamp_u64_to_u32(index_stats.projection_batch_wall_ms)),
            projection_batch_transactions: Some(clamp_usize_to_u32(
                index_stats.projection_batch_transactions,
            )),
            projection_persistence: Some(projection_persistence_timings(
                &index_stats.projection_persistence,
            )),
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_stream_ms: None,
            search_symbol_stream_rows: None,
            search_symbol_stream_batches: None,
            search_symbol_index_ms: None,
            search_symbol_index_docs_written: None,
            search_symbol_index_writer_count: None,
            search_symbol_index_commit_count: None,
            search_symbol_index_reload_count: None,
            search_symbol_index_commit_ms: None,
            search_symbol_index_reload_ms: None,
            runtime_cache_publish_ms: None,
            semantic_context_index_ms: None,
            semantic_node_load_ms: None,
            semantic_node_load_rows: None,
            semantic_node_stream_batches: None,
            semantic_endpoint_load_ms: None,
            semantic_endpoint_load_rows: None,
            semantic_endpoint_load_batches: None,
            semantic_selected_nodes: None,
            semantic_context_file_count: None,
            semantic_context_path_bytes: None,
            semantic_node_lookup_entries: None,
            semantic_context_ms: None,
            semantic_doc_build_ms: None,
            semantic_embedding_ms: None,
            semantic_db_upsert_ms: None,
            semantic_reload_ms: None,
            semantic_prune_ms: None,
            semantic_docs_reused: None,
            semantic_docs_embedded: None,
            semantic_docs_pending: None,
            semantic_docs_stale: None,
            symbol_search_docs_written: None,
            semantic_dense_docs_skipped: None,
            semantic_dense_public_api: None,
            semantic_dense_entrypoint: None,
            semantic_dense_documented_nontrivial: None,
            semantic_dense_central_graph_node: None,
            semantic_dense_component_report: None,
            semantic_dense_unstructured_doc: None,
            deferred_indexes_ms: Some(deferred_indexes_ms),
            summary_snapshot_ms: Some(summary_snapshot_ms),
            detail_snapshot_ms: Some(detail_snapshot_ms),
            publish_ms: Some(publish_ms),
            staged_sqlite_wal_autocheckpoint_bytes: staged_publish_stats
                .sqlite_wal_autocheckpoint_bytes,
            staged_sqlite_checkpoint_ms: staged_publish_stats.sqlite_checkpoint_ms,
            staged_sqlite_sync_ms: staged_publish_stats.sqlite_sync_ms,
            staged_snapshot_copy: staged_publish_stats
                .snapshot_copy
                .map(database_snapshot_copy_timings),
            core_promotion: Some(core_promotion_timings(staged_publish_stats.core_promotion)),
            setup_existing_projection_ids_ms: resolution_telemetry.setup_existing_projection_ids_ms,
            setup_seed_symbol_table_ms: resolution_telemetry.setup_seed_symbol_table_ms,
            flush_files_ms: resolution_telemetry.flush_files_ms,
            flush_nodes_ms: resolution_telemetry.flush_nodes_ms,
            flush_edges_ms: resolution_telemetry.flush_edges_ms,
            flush_occurrences_ms: resolution_telemetry.flush_occurrences_ms,
            flush_component_access_ms: resolution_telemetry.flush_component_access_ms,
            flush_callable_projection_ms: resolution_telemetry.flush_callable_projection_ms,
            unresolved_calls_start: clamp_usize_to_u32(index_stats.unresolved_calls_start),
            unresolved_imports_start: clamp_usize_to_u32(index_stats.unresolved_imports_start),
            resolved_calls: clamp_usize_to_u32(index_stats.resolved_calls),
            resolved_imports: clamp_usize_to_u32(index_stats.resolved_imports),
            unresolved_calls_end: clamp_usize_to_u32(index_stats.unresolved_calls_end),
            unresolved_imports_end: clamp_usize_to_u32(index_stats.unresolved_imports_end),
            resolution_override_count_ms: resolution_telemetry.resolution_override_count_ms,
            resolution_unresolved_counts_ms: resolution_telemetry.resolution_unresolved_counts_ms,
            resolution_calls_ms: resolution_telemetry.resolution_calls_ms,
            resolution_imports_ms: resolution_telemetry.resolution_imports_ms,
            resolution_cleanup_ms: resolution_telemetry.resolution_cleanup_ms,
            resolution_call_candidate_index_ms: resolution_telemetry
                .resolution_call_candidate_index_ms,
            resolution_import_candidate_index_ms: resolution_telemetry
                .resolution_import_candidate_index_ms,
            resolution_call_semantic_index_ms: resolution_telemetry
                .resolution_call_semantic_index_ms,
            resolution_import_semantic_index_ms: resolution_telemetry
                .resolution_import_semantic_index_ms,
            resolution_support_snapshot_limit_bytes: resolution_telemetry
                .resolution_support_snapshot_limit_bytes,
            resolution_support_snapshot_stored: resolution_telemetry
                .resolution_support_snapshot_stored,
            resolution_support_snapshot_skipped_oversize: resolution_telemetry
                .resolution_support_snapshot_skipped_oversize,
            resolution_call_semantic_candidates_ms: resolution_telemetry
                .resolution_call_semantic_candidates_ms,
            resolution_import_semantic_candidates_ms: resolution_telemetry
                .resolution_import_semantic_candidates_ms,
            resolution_call_semantic_requests: resolution_telemetry
                .resolution_call_semantic_requests,
            resolution_call_semantic_unique_requests: resolution_telemetry
                .resolution_call_semantic_unique_requests,
            resolution_call_semantic_skipped_requests: resolution_telemetry
                .resolution_call_semantic_skipped_requests,
            resolution_import_semantic_requests: resolution_telemetry
                .resolution_import_semantic_requests,
            resolution_import_semantic_unique_requests: resolution_telemetry
                .resolution_import_semantic_unique_requests,
            resolution_import_semantic_skipped_requests: resolution_telemetry
                .resolution_import_semantic_skipped_requests,
            resolution_call_compute_ms: resolution_telemetry.resolution_call_compute_ms,
            resolution_import_compute_ms: resolution_telemetry.resolution_import_compute_ms,
            resolution_call_apply_ms: resolution_telemetry.resolution_call_apply_ms,
            resolution_import_apply_ms: resolution_telemetry.resolution_import_apply_ms,
            resolution_override_resolution_ms: resolution_telemetry
                .resolution_override_resolution_ms,
            resolved_calls_same_file: resolution_telemetry.resolved_calls_same_file,
            resolved_calls_same_module: resolution_telemetry.resolved_calls_same_module,
            resolved_calls_global_unique: resolution_telemetry.resolved_calls_global_unique,
            resolved_calls_semantic: resolution_telemetry.resolved_calls_semantic,
            resolved_imports_same_file: resolution_telemetry.resolved_imports_same_file,
            resolved_imports_same_module: resolution_telemetry.resolved_imports_same_module,
            resolved_imports_global_unique: resolution_telemetry.resolved_imports_global_unique,
            resolved_imports_fuzzy: resolution_telemetry.resolved_imports_fuzzy,
            resolved_imports_semantic: resolution_telemetry.resolved_imports_semantic,
        },
        staged_semantic_stats,
        llm_refresh_scope: Some(llm_refresh_scope),
        #[cfg(test)]
        publication,
        prepared_search_state: Some(prepared_search_state),
    })
}

fn is_indexing_cancelled(cancel_token: Option<&CancellationToken>) -> bool {
    cancel_token
        .map(CancellationToken::is_cancelled)
        .unwrap_or(false)
}

fn indexing_cancelled_error() -> ApiError {
    ApiError::new("cancelled", "Indexing cancelled.")
}

fn workspace_refresh_inputs(store: &Store) -> Result<RefreshInputs, ApiError> {
    Ok(RefreshInputs {
        stored_files: store
            .files()
            .inventory()
            .map_err(|e| ApiError::internal(format!("Failed to read workspace inventory: {e}")))?,
        policy_exclusions: store
            .get_source_policy_exclusions()
            .map_err(|e| {
                ApiError::internal(format!("Failed to read source policy exclusions: {e}"))
            })?
            .iter()
            .map(source_policy_exclusion_candidate)
            .collect(),
        inventory: Default::default(),
    })
}

fn reuse_completed_search_state(
    storage: &mut Storage,
    search_storage_path: &Path,
    publication: &IndexPublicationRecord,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<Option<SearchStateBuildResult>, ApiError> {
    let generation_id = Uuid::parse_str(&publication.generation_id)
        .map_err(|error| {
            ApiError::internal(format!(
                "Invalid index publication generation id {}: {error}",
                publication.generation_id
            ))
        })?
        .to_string();
    let Some(marker) = read_search_generation_completion(search_storage_path, &generation_id)
    else {
        return Ok(None);
    };

    let search_index_started = Instant::now();
    let mut engine = match SearchEngine::open_existing(search_storage_path) {
        Ok(engine) => engine,
        Err(error) => {
            tracing::warn!(
                path = %search_storage_path.display(),
                "Completed persisted search generation could not be reopened and will be rebuilt: {error}"
            );
            return Ok(None);
        }
    };
    engine.load_symbol_projection(std::iter::empty());
    let (node_names, mut search_stats) = load_canonical_search_symbols(
        storage,
        SEARCH_SYMBOL_STREAM_BATCH_SIZE,
        cancel_token,
        |batch| {
            engine.extend_symbol_projection(
                batch
                    .into_iter()
                    .map(|entry| (entry.node_id, entry.display_name)),
            );
            Ok(())
        },
    )?;
    if marker.symbol_count != search_stats.search_symbol_stream_rows as u64
        || engine.full_text_doc_count() != search_stats.search_symbol_stream_rows as usize
        || engine.tantivy_doc_count() as u64 != marker.tantivy_doc_count
    {
        tracing::warn!(
            path = %search_storage_path.display(),
            searchable_docs = engine.full_text_doc_count(),
            stored_docs = engine.tantivy_doc_count(),
            expected_symbols = search_stats.search_symbol_stream_rows,
            expected_stored_docs = marker.tantivy_doc_count,
            "Completed persisted search generation count validation failed and will be rebuilt"
        );
        return Ok(None);
    }
    search_stats.search_symbol_index_ms =
        clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let semantic_stats = load_persisted_semantic_docs_for_runtime(
        storage,
        &mut engine,
        hydrate_semantic_docs,
        runtime,
    )?;
    Ok(Some(SearchStateBuildResult {
        publication: Some(publication.clone()),
        node_names,
        engine,
        search_stats,
        semantic_stats,
    }))
}

fn build_persisted_search_state_from_canonical_symbols(
    storage: &mut Storage,
    search_storage_path: &Path,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<SearchStateBuildResult, ApiError> {
    let search_index_started = Instant::now();
    let count_started = Instant::now();
    let expected_rows = storage
        .get_canonical_search_symbol_count()
        .map_err(|error| {
            ApiError::internal(format!("Failed to count canonical search symbols: {error}"))
        })?;
    let mut stream_duration = count_started.elapsed();
    let mut engine = SearchEngine::new(Some(search_storage_path)).map_err(|error| {
        if search::engine::is_persisted_search_index_busy(&error) {
            ApiError::new(
                "cache_busy",
                format!("Failed to init search engine: {error}"),
            )
        } else {
            ApiError::internal(format!("Failed to init search engine: {error}"))
        }
    })?;
    let mut node_names = HashMap::with_capacity(expected_rows as usize);
    let mut symbol_session = engine.begin_symbol_index().map_err(|error| {
        ApiError::internal(format!("Failed to start symbol index writer: {error}"))
    })?;
    let mut after_node_id = None;
    let mut stream_rows = 0_usize;
    let mut stream_batches = 0_usize;
    loop {
        let batch_started = Instant::now();
        let batch = storage
            .get_canonical_search_symbol_batch_after(after_node_id, SEARCH_SYMBOL_STREAM_BATCH_SIZE)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to stream canonical search symbols: {error}"
                ))
            })?;
        stream_duration = stream_duration.saturating_add(batch_started.elapsed());
        if batch.is_empty() {
            break;
        }
        after_node_id = batch.last().map(|entry| entry.node_id);
        stream_rows = stream_rows.saturating_add(batch.len());
        stream_batches = stream_batches.saturating_add(1);
        let symbols = batch
            .into_iter()
            .map(|entry| {
                node_names.insert(entry.node_id, entry.display_name.clone());
                (entry.node_id, entry.display_name)
            })
            .collect::<Vec<_>>();
        symbol_session.add_nodes(symbols).map_err(|error| {
            ApiError::internal(format!("Failed to index search nodes: {error}"))
        })?;
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SearchSymbolPage, cancel_token)?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
    }
    if stream_rows != expected_rows as usize {
        return Err(ApiError::internal(format!(
            "Canonical search symbol stream count changed: expected {expected_rows}, loaded {stream_rows}"
        )));
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchIndexWrite, cancel_token)?;
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let symbol_write_stats = symbol_session
        .finish()
        .map_err(|error| ApiError::internal(format!("Failed to commit symbol index: {error}")))?;
    if engine.full_text_doc_count() != stream_rows {
        return Err(ApiError::internal(format!(
            "Persisted search generation validation failed: indexed {} docs for {stream_rows} canonical symbols",
            engine.full_text_doc_count()
        )));
    }
    let search_symbol_index_ms = clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let semantic_stats = load_persisted_semantic_docs_for_runtime(
        storage,
        &mut engine,
        hydrate_semantic_docs,
        runtime,
    )?;
    Ok(SearchStateBuildResult {
        publication: None,
        node_names,
        engine,
        search_stats: SearchStateBuildStats {
            search_projection_rebuild_ms: 0,
            search_symbol_stream_ms: clamp_u128_to_u32(stream_duration.as_millis()),
            search_symbol_stream_rows: clamp_usize_to_u32(stream_rows),
            search_symbol_stream_batches: clamp_usize_to_u32(stream_batches),
            search_symbol_index_ms,
            search_symbol_index_docs_written: clamp_usize_to_u32(symbol_write_stats.docs_written),
            search_symbol_index_writer_count: clamp_usize_to_u32(symbol_write_stats.writer_count),
            search_symbol_index_commit_count: clamp_usize_to_u32(symbol_write_stats.commit_count),
            search_symbol_index_reload_count: clamp_usize_to_u32(symbol_write_stats.reload_count),
            search_symbol_index_commit_ms: clamp_u128_to_u32(
                symbol_write_stats.commit_duration.as_millis(),
            ),
            search_symbol_index_reload_ms: clamp_u128_to_u32(
                symbol_write_stats.reload_duration.as_millis(),
            ),
        },
        semantic_stats,
    })
}

#[cfg(test)]
fn rebuild_search_state_from_storage(
    storage: &mut Storage,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
) -> Result<SearchStateBuildResult, ApiError> {
    rebuild_search_state_from_storage_for_runtime(
        storage,
        storage_path,
        llm_refresh_scope,
        hydrate_semantic_docs,
        &test_sidecar_runtime_from_env(),
        None,
        None,
    )
}

type SearchCompletionValidator<'a> =
    &'a mut dyn FnMut(&IndexPublicationRecord) -> Result<(), ApiError>;

fn rebuild_search_state_from_storage_for_runtime(
    storage: &mut Storage,
    storage_path: &Path,
    _llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
    mut validate_before_completion: Option<SearchCompletionValidator<'_>>,
) -> Result<SearchStateBuildResult, ApiError> {
    let publication = storage.get_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to read search publication identity: {error}"
        ))
    })?;
    let _catalog_guard = publication
        .as_ref()
        .map(|_| SearchGenerationCatalogGuard::acquire(storage_path))
        .transpose()?;
    let search_storage_path =
        search_index_path_for_publication(storage_path, publication.as_ref())?;
    let reused = match publication.as_ref() {
        Some(publication) => reuse_completed_search_state(
            storage,
            &search_storage_path,
            publication,
            hydrate_semantic_docs,
            runtime,
            cancel_token,
        )?,
        None => None,
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchBuild, cancel_token)?;
    let built_new = reused.is_none();
    let mut result = match reused {
        Some(result) => result,
        None => build_persisted_search_state_from_canonical_symbols(
            storage,
            search_storage_path.as_path(),
            hydrate_semantic_docs,
            runtime,
            cancel_token,
        )
        .map_err(|mut error| {
            error.message = format!("Failed to rebuild search state: {}", error.message);
            error
        })?,
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchValidation, cancel_token)?;
    if result.engine.full_text_doc_count() != result.node_names.len() {
        return Err(ApiError::internal(format!(
            "Prepared search generation contains {} searchable symbols for {} core symbols",
            result.engine.full_text_doc_count(),
            result.node_names.len()
        )));
    }
    if built_new && let Some(publication) = publication.as_ref() {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        if let Some(validate) = validate_before_completion.as_mut()
            && let Err(error) = validate(publication)
        {
            drop(result);
            discard_unpublished_search_generation(storage_path, publication);
            return Err(error);
        }
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SearchCompletion, cancel_token)?;
        write_search_generation_completion(
            &search_storage_path,
            publication,
            result.node_names.len(),
            result.engine.tantivy_doc_count(),
        )?;
    }
    if publication.is_some() {
        result
            .engine
            .downgrade_persisted_lock_to_shared()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to share completed search generation {}: {error}",
                    search_storage_path.display()
                ))
            })?;
    }
    if publication.is_some()
        && let Some(active_generation_id) =
            search_storage_path.file_name().and_then(|id| id.to_str())
        && let Err(error) = prune_search_generations(storage_path, active_generation_id)
    {
        tracing::warn!(
            generation_id = %active_generation_id,
            "Failed to prune persisted search generations after publication: {}",
            error.message
        );
    }
    result.publication = publication;
    Ok(result)
}

fn refresh_caches(
    controller: &AppController,
    storage: &mut Storage,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
) -> Result<CacheRefreshStats, ApiError> {
    let refreshed = rebuild_search_state_from_storage_for_runtime(
        storage,
        storage_path,
        llm_refresh_scope,
        true,
        &controller.runtime_config,
        None,
        None,
    );

    match refreshed {
        Ok(result) => Ok(publish_prepared_search_state(controller, result)),
        Err(error) => {
            tracing::warn!(
                "Failed to rebuild search caches from storage: {}",
                error.message
            );
            let mut state = controller.state.lock();
            state.node_names.clear();
            clear_search_engine(&mut state);
            controller.sidecar_query_cache.lock().clear();
            state.is_indexing = false;
            Err(error)
        }
    }
}

fn publish_prepared_search_state(
    controller: &AppController,
    result: SearchStateBuildResult,
) -> CacheRefreshStats {
    let publish_started = Instant::now();
    let mut state = controller.state.lock();
    state.node_names = result.node_names;
    publish_search_engine(&mut state, result.engine, result.publication);
    controller.sidecar_query_cache.lock().clear();
    state.is_indexing = false;
    CacheRefreshStats {
        search_stats: result.search_stats,
        semantic_stats: result.semantic_stats,
        runtime_cache_publish_ms: Some(clamp_u128_to_u32(publish_started.elapsed().as_millis())),
    }
}

#[cfg(test)]
mod tests;
