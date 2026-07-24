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
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, BufRead};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod affected;
mod agent;
use affected::*;
pub use agent::{packet_step_trace_json, plan_packet};

/// Result of explicitly republishing semantic projections from one pinned core.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticProjectionRepublishOutcome {
    pub previous_publication: IndexPublicationRecord,
    pub publication: IndexPublicationRecord,
    pub semantic_policy_version: String,
    pub symbol_document_count: u32,
    pub dense_anchor_count: u64,
    pub phase_timings: IndexingPhaseTimings,
}
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

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn stored_semantic_embeddings_for_test(storage_path: &Path) -> anyhow::Result<Vec<Vec<f32>>> {
    Ok(Store::open_read_only(storage_path)?
        .get_all_llm_symbol_docs()?
        .into_iter()
        .map(|document| document.embedding)
        .collect())
}
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

const REPO_TEXT_SCAN_FILE_CAP: usize = 2_000;
const REPO_TEXT_SCAN_BYTE_CAP: usize = 32 * 1024 * 1024;
const REPO_TEXT_SCAN_TIME_CAP_MS: u128 = 500;
const REPO_TEXT_MAX_FILE_BYTES: u64 = 1_000_000;
const DIRECT_SNIPPET_CONTEXT_LINE_CAP: usize = 50;
pub(crate) const DIRECT_SNIPPET_MAX_BYTES: usize = 64 * 1024;
const DIRECT_SNIPPET_TRUNCATION_SUFFIX: &str = "\n... snippet truncated by byte cap\n```";

#[derive(Debug, Clone)]
struct RepoTextScan {
    hits: Vec<SearchHit>,
    #[cfg(test)]
    stats: RepoTextScanStatsDto,
}

#[derive(Debug, Clone, Default)]
struct SearchPlanExecutedEvidence {
    indexed_symbol_hits: Vec<SearchHit>,
    repo_text_hits: Vec<SearchHit>,
    suggestions: Vec<SearchHit>,
    candidate_windows: Vec<SearchPlanCandidateWindowDto>,
}

#[derive(Debug, Clone, Copy, Default)]
struct SearchPlanActivePathEvidence {
    caller_count: u32,
}

#[derive(Debug, Clone)]
struct SearchPlanBuild {
    plan: SearchPlanDto,
    indexed_symbol_hits: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
struct SearchIntentQuery {
    effective_query: String,
    filters: Vec<SearchIntentFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchIntentFilter {
    Kind(String),
    Path(String),
    Name(String),
    Language(String),
}

#[derive(Debug, Clone)]
pub(crate) struct BoundedSnippet {
    pub(crate) markdown: String,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BoundedSnippetRangeOptions<'a> {
    pub(crate) focus_line: u32,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) context_lines: usize,
    pub(crate) max_bytes: usize,
    pub(crate) truncation_suffix: &'a str,
}

fn parse_search_intent_query(query: &str) -> SearchIntentQuery {
    let mut free_terms = Vec::new();
    let mut name_terms = Vec::new();
    let mut fallback_terms = Vec::new();
    let mut filters = Vec::new();

    for token in query.split_whitespace() {
        let Some((field, raw_value)) = token.split_once(':') else {
            free_terms.push(token.to_string());
            continue;
        };
        let value = strip_query_value_quotes(raw_value);
        if value.is_empty() {
            free_terms.push(token.to_string());
            continue;
        }
        match field.to_ascii_lowercase().as_str() {
            "kind" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Kind(value));
            }
            "path" | "file" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Path(value));
            }
            "name" | "symbol" => {
                name_terms.push(value.clone());
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Name(value));
            }
            "lang" | "language" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Language(value));
            }
            _ => free_terms.push(token.to_string()),
        }
    }

    let effective_query = if !free_terms.is_empty() {
        free_terms.join(" ")
    } else if !name_terms.is_empty() {
        name_terms.join(" ")
    } else if !fallback_terms.is_empty() {
        fallback_terms.join(" ")
    } else {
        query.trim().to_string()
    };

    SearchIntentQuery {
        effective_query,
        filters,
    }
}

fn strip_query_value_quotes(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if matches!(first, b'"' | b'\'' | b'`') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn apply_search_intent_filters(hits: &mut Vec<SearchHit>, filters: &[SearchIntentFilter]) {
    if filters.is_empty() {
        return;
    }
    hits.retain(|hit| {
        filters
            .iter()
            .all(|filter| search_hit_matches_intent_filter(hit, filter))
    });
}

fn search_hit_matches_intent_filter(hit: &SearchHit, filter: &SearchIntentFilter) -> bool {
    match filter {
        SearchIntentFilter::Kind(kind) => search_hit_kind_matches(hit.kind, kind),
        SearchIntentFilter::Path(fragment) => hit.file_path.as_deref().is_some_and(|path| {
            path.to_ascii_lowercase()
                .contains(&fragment.to_ascii_lowercase())
        }),
        SearchIntentFilter::Name(name) => search_hit_name_matches(&hit.display_name, name),
        SearchIntentFilter::Language(language) => hit
            .file_path
            .as_deref()
            .is_some_and(|path| language_filter_matches_path(language, path)),
    }
}

fn search_hit_kind_matches(kind: NodeKind, requested: &str) -> bool {
    let normalized = normalize_filter_token(requested);
    let actual = normalize_filter_token(&format!("{kind:?}"));
    if normalized == actual {
        return true;
    }
    match normalized.as_str() {
        "fn" | "func" | "function" => kind == NodeKind::FUNCTION,
        "method" => kind == NodeKind::METHOD,
        "callable" => matches!(
            kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        ),
        "type" => matches!(
            kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        ),
        "var" | "variable" => matches!(kind, NodeKind::VARIABLE | NodeKind::GLOBAL_VARIABLE),
        "global" | "globalvar" | "globalvariable" => kind == NodeKind::GLOBAL_VARIABLE,
        "const" | "constant" => kind == NodeKind::CONSTANT,
        "field" | "member" => kind == NodeKind::FIELD,
        "file" => kind == NodeKind::FILE,
        _ => false,
    }
}

fn search_hit_name_matches(display_name: &str, requested: &str) -> bool {
    let requested = requested.trim();
    if requested.is_empty() {
        return true;
    }
    let display_lower = display_name.to_ascii_lowercase();
    let requested_lower = requested.to_ascii_lowercase();
    display_lower.contains(&requested_lower)
        || normalize_symbol_query(display_name).contains(&normalize_symbol_query(requested))
}

fn language_filter_matches_path(requested: &str, path: &str) -> bool {
    let requested_lower = requested
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if requested_lower.is_empty() {
        return false;
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension.is_empty() {
        return false;
    }

    if let Some(language_name) = language_family_alias(&requested_lower) {
        return language_profile_matches_extension_name(language_name, &extension);
    }

    if let Some(profile) = language_support_profile_for_language_name(&requested_lower) {
        return language_profile_matches_extension(profile, &extension);
    }

    if language_support_profile_for_ext(&requested_lower).is_some() {
        return requested_lower == extension;
    }

    let requested_normalized = normalize_filter_token(&requested_lower);
    match requested_normalized.as_str() {
        "markdown" => matches!(extension.as_str(), "md" | "mdx"),
        _ => requested_normalized == normalize_filter_token(&extension),
    }
}

fn indexed_file_matches_language_filter(
    stored_language: &str,
    path: &Path,
    requested: &str,
) -> bool {
    stored_language.eq_ignore_ascii_case(requested)
        || language_filter_matches_path(requested, &path.to_string_lossy())
}

fn language_family_alias(requested: &str) -> Option<&'static str> {
    match requested {
        "ts" => Some("typescript"),
        "js" => Some("javascript"),
        "kt" => Some("kotlin"),
        "c++" | "cplusplus" => Some("cpp"),
        "c#" | "cs" => Some("csharp"),
        _ => None,
    }
}

fn language_profile_matches_extension(profile: &LanguageSupportProfile, extension: &str) -> bool {
    profile.extensions.contains(&extension)
}

fn language_profile_matches_extension_name(language_name: &str, extension: &str) -> bool {
    language_support_profile_for_language_name(language_name)
        .is_some_and(|profile| language_profile_matches_extension(profile, extension))
}

fn normalize_filter_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn exact_symbol_hit_count(query: &str, hits: &[SearchHit]) -> u32 {
    hits.iter()
        .filter(|hit| {
            let rank = symbol_name_match_rank(query, &hit.display_name);
            rank.exact_display > 0 || rank.exact_terminal > 0 || rank.exact_leading > 0
        })
        .count()
        .min(u32::MAX as usize) as u32
}

fn search_hit_match_quality(query: &str, hit: &SearchHit) -> SearchMatchQualityDto {
    if hit.is_text_match() {
        return SearchMatchQualityDto::RepoText;
    }
    let query = query.trim();
    if query.is_empty() {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    let query_normalized = normalize_symbol_query(query);
    let display_normalized = normalize_symbol_query(&hit.display_name);
    let terminal = terminal_symbol_segment(&hit.display_name);
    let leading = leading_symbol_segment(&hit.display_name);
    let exact_terms = exact_symbol_query_terms(query);
    if hit.display_name == query || exact_terms.contains(&hit.display_name) {
        return SearchMatchQualityDto::Exact;
    }
    if exact_terms.iter().any(|term| {
        let normalized = normalize_symbol_query(term);
        display_normalized == normalized || terminal == normalized || leading == normalized
    }) {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized == query_normalized
        || terminal == query_normalized
        || leading == query_normalized
    {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized.starts_with(&query_normalized)
        || terminal.starts_with(&query_normalized)
        || leading.starts_with(&query_normalized)
    {
        return SearchMatchQualityDto::Prefix;
    }
    if hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.semantic > 0.0 && breakdown.lexical <= f32::EPSILON)
    {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    SearchMatchQualityDto::Fuzzy
}

fn annotate_search_hit_match_quality(query: &str, hits: &mut [SearchHit]) {
    for hit in hits {
        hit.match_quality = Some(search_hit_match_quality(query, hit));
    }
}

fn text_contains_query_term(text: &str, term: &str) -> bool {
    if text.contains(term) {
        return true;
    }
    let Some(singular) = term.strip_suffix('s') else {
        return false;
    };
    singular.len() >= 3 && text.contains(singular)
}

fn weak_search_top_hit(query: &str, hits: &[SearchHit]) -> bool {
    hits.first().is_none_or(|hit| {
        hit.score < 0.5
            || matches!(
                search_hit_match_quality(query, hit),
                SearchMatchQualityDto::Fuzzy | SearchMatchQualityDto::SemanticSuggestion
            )
    })
}

#[cfg(test)]
fn repo_text_auto_fallback_reason(query: &str, indexed_hits: &[SearchHit]) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    if indexed_hits.is_empty() {
        return Some("auto fallback: no indexed symbol hits matched the query".to_string());
    }
    if exact_symbol_hit_count(query, indexed_hits) == 0 {
        if query_has_symbol_or_literal_signal(query) {
            return Some(
                "auto fallback: query looks like a concrete anchor but no exact indexed symbol matched"
                    .to_string(),
            );
        }
        if weak_search_top_hit(query, indexed_hits) {
            return Some(
                "auto fallback: top indexed symbol hit was weak and non-exact".to_string(),
            );
        }
    }
    None
}

fn search_query_assessment(
    query: &str,
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    repo_text_mode: SearchRepoTextMode,
    repo_text_enabled: bool,
    repo_text_fallback_reason: Option<String>,
) -> SearchQueryAssessmentDto {
    let exact_symbol_hit_count = exact_symbol_hit_count(query, indexed_hits);
    let weak_top_hit = exact_symbol_hit_count == 0 && weak_search_top_hit(query, indexed_hits);
    let stale_or_missing_anchor =
        exact_symbol_hit_count == 0 && query_has_symbol_or_literal_signal(query);
    let architecture_intents = architecture_query_intents(query);

    SearchQueryAssessmentDto {
        exact_symbol_hit_count,
        weak_top_hit,
        stale_or_missing_anchor,
        repo_text_fallback_reason,
        recommended_next_action: Some(search_query_recommended_next_action(
            exact_symbol_hit_count,
            &architecture_intents,
            indexed_hits,
            repo_text_hits,
            repo_text_mode,
            repo_text_enabled,
        )),
    }
}

fn search_query_recommended_next_action(
    exact_symbol_hit_count: u32,
    architecture_intents: &[symbol_query::ArchitectureQueryIntent],
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    repo_text_mode: SearchRepoTextMode,
    repo_text_enabled: bool,
) -> String {
    if exact_symbol_hit_count > 0 && !architecture_intents.is_empty() {
        return format!(
            "Architecture intent detected ({}); open the strongest production entrypoint/orchestrator with symbol, trail, and function-body snippet before answering.",
            architecture_intent_labels(architecture_intents)
        );
    }
    if exact_symbol_hit_count > 0 {
        return "Open the exact indexed hit with symbol, trail, and snippet before answering."
            .to_string();
    }
    if !architecture_intents.is_empty() && !indexed_hits.is_empty() {
        return format!(
            "Architecture intent detected ({}) with no exact anchor; run drill with concrete anchors from ground/search, then inspect symbol, trail, and function-body snippets before answering. Treat broad search hits as leads only.",
            architecture_intent_labels(architecture_intents)
        );
    }
    if !repo_text_hits.is_empty() {
        return "Use repo-text hits to choose a concrete identifier, then rerun symbol/trail/snippet."
            .to_string();
    }
    if repo_text_mode == SearchRepoTextMode::Off || !repo_text_enabled {
        return "Run retrieval index to restore full sidecar mode, then rerun search --why with a shorter concrete symbol.".to_string();
    }
    "Try a shorter symbol, file name, or literal from ground output.".to_string()
}

fn architecture_intent_labels(intents: &[symbol_query::ArchitectureQueryIntent]) -> String {
    intents
        .iter()
        .map(|intent| intent.label())
        .collect::<Vec<_>>()
        .join(", ")
}

const SEARCH_PLAN_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "anchor",
    "answer",
    "are",
    "around",
    "as",
    "at",
    "be",
    "by",
    "can",
    "cite",
    "cited",
    "cites",
    "code",
    "codestory",
    "does",
    "explain",
    "for",
    "from",
    "how",
    "in",
    "into",
    "is",
    "it",
    "later",
    "of",
    "on",
    "or",
    "repo",
    "repository",
    "show",
    "that",
    "the",
    "then",
    "this",
    "through",
    "to",
    "turns",
    "what",
    "where",
    "which",
    "why",
    "with",
];
const SEARCH_PLAN_SYMBOL_TERMS: &[&str] = &[
    "indexer",
    "service",
    "storage",
    "store",
    "posts",
    "feed",
    "auth",
    "trail",
    "snippet",
    "workspace",
    "persistence",
    "snapshot",
];
const SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT: usize = 8;
const SEARCH_PLAN_MAX_SEED_ANCHORS: usize = 32;
const SEARCH_PLAN_SEED_ANCHOR_MARKER: &str = "Seed anchors:";
const SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER: &str = "Anchor the answer around";
const SEARCH_PLAN_ROLE_SPECS: &[(&str, &[&str])] = &[
    (
        "indexing_pipeline",
        &["full", "index", "indexing", "indexer", "workspace", "store"],
    ),
    (
        "build_index_entrypoint",
        &["project", "indexing", "build", "index"],
    ),
    (
        "source_group_configuration",
        &[
            "project",
            "source-group",
            "source",
            "group",
            "configuration",
        ],
    ),
    (
        "indexing_work",
        &["indexing", "indexed", "indexer", "command", "work"],
    ),
    (
        "storage_access_surface",
        &[
            "storage",
            "access",
            "accessed",
            "data",
            "application",
            "persistence",
        ],
    ),
    (
        "workspace_discovery",
        &["workspace", "file", "discovery", "source"],
    ),
    (
        "symbol_extraction",
        &["symbol", "extraction", "indexer", "indexing"],
    ),
    (
        "runtime_boundary",
        &["cli", "runtime", "command", "service"],
    ),
    (
        "exec_cli_surface",
        &["exec", "cli", "json", "subcommand", "runtime"],
    ),
    (
        "exec_event_output_surface",
        &[
            "exec",
            "event",
            "events",
            "json",
            "jsonl",
            "output",
            "event processor",
        ],
    ),
    (
        "read_surface",
        &["search", "trail", "snippet", "context", "explore"],
    ),
    (
        "collection_config_surface",
        &[
            "payload",
            "collection",
            "collections",
            "schema",
            "hooks",
            "access",
            "config",
        ],
    ),
    (
        "comment_submission_surface",
        &["comments", "comment", "auth", "submission", "guard"],
    ),
    (
        "public_feed_surface",
        &["feed", "rss", "elsewhere", "social", "entries"],
    ),
    (
        "content_surface",
        &["posts", "comments", "auth", "feed", "elsewhere"],
    ),
    (
        "persistence_surface",
        &[
            "storage",
            "store",
            "persistence",
            "payload",
            "collection",
            "snapshot",
            "refresh",
        ],
    ),
];
const SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS: &[&str] = &[
    "Draft the CodeStory-only answer from selected anchors, bridge status, symbol, trail, and snippet evidence before opening source.",
    "Open the cited source files after the CodeStory-only draft and classify each claim as correct, partial, misleading, or unsupported.",
];
const SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK: &str = "Repo-text-only or ambiguous groups require direct source reads before they can support architecture claims.";

fn search_plan_terms(query: &str) -> SearchPlanTermsDto {
    let mut extracted = Vec::new();
    let mut dropped = Vec::new();
    let mut seen = HashSet::new();
    let mut dropped_seen = HashSet::new();

    for raw in query.split_whitespace() {
        let token = raw
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']'
                )
            })
            .trim_end_matches("'s");
        if token.is_empty() {
            continue;
        }
        let fragments = token
            .split('/')
            .flat_map(|part| part.split('.'))
            .flat_map(|part| part.split(':'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        for fragment in fragments {
            let normalized = fragment
                .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));
            if normalized.is_empty() {
                continue;
            }
            add_search_plan_term(
                normalized,
                &mut extracted,
                &mut seen,
                &mut dropped,
                &mut dropped_seen,
            );
            if normalized.contains('-') {
                for part in normalized.split('-').filter(|part| !part.is_empty()) {
                    add_search_plan_term(
                        part,
                        &mut extracted,
                        &mut seen,
                        &mut dropped,
                        &mut dropped_seen,
                    );
                }
            }
            for camel_part in split_camel_identifier(normalized) {
                add_search_plan_term(
                    &camel_part,
                    &mut extracted,
                    &mut seen,
                    &mut dropped,
                    &mut dropped_seen,
                );
            }
        }
    }
    drop_search_plan_brand_terms_for_content_flow(query, &mut extracted, &mut dropped);
    add_search_plan_inferred_architecture_terms(
        query,
        &mut extracted,
        &mut seen,
        &mut dropped,
        &mut dropped_seen,
    );

    SearchPlanTermsDto { extracted, dropped }
}

fn add_search_plan_inferred_architecture_terms(
    query: &str,
    extracted: &mut Vec<String>,
    seen: &mut HashSet<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
    dropped_seen: &mut HashSet<String>,
) {
    let lower = query.to_ascii_lowercase();
    let has_source_group = lower.contains("source-group")
        || (search_plan_query_has_token(&lower, "source")
            && search_plan_query_has_token(&lower, "group"));
    if has_source_group {
        add_search_plan_term("SourceGroup", extracted, seen, dropped, dropped_seen);
    }

    let has_indexing_work = search_plan_query_has_token(&lower, "indexing")
        && (search_plan_query_has_token(&lower, "work")
            || search_plan_query_has_token(&lower, "command")
            || has_source_group);
    if has_indexing_work {
        add_search_plan_term("build", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("index", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("BuildIndex", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("indexer", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("IndexerCommand", extracted, seen, dropped, dropped_seen);
    }

    let has_data_access = search_plan_query_has_token(&lower, "data")
        && (search_plan_query_has_token(&lower, "access")
            || search_plan_query_has_token(&lower, "accessed"))
        && search_plan_query_has_token(&lower, "application");
    if has_data_access {
        add_search_plan_term("access", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("storage", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("persistence", extracted, seen, dropped, dropped_seen);
    }

    let has_event_output = search_plan_query_has_token(&lower, "event")
        && (search_plan_query_has_token(&lower, "output")
            || search_plan_query_has_token(&lower, "notification")
            || search_plan_query_has_token(&lower, "notifications")
            || search_plan_query_has_token(&lower, "jsonl"));
    if has_event_output {
        add_search_plan_term("EventProcessor", extracted, seen, dropped, dropped_seen);
    }

    if search_plan_query_has_exec_json_flow(&lower) {
        for term in [
            "exec cli",
            "exec runtime",
            "exec session",
            "event processor",
            "event output",
            "thread start",
            "turn start",
        ] {
            add_search_plan_term(term, extracted, seen, dropped, dropped_seen);
        }
    }

    if search_plan_query_has_payload_content_flow(&lower) {
        for term in [
            "content config",
            "collection config",
            "Posts",
            "Comments",
            "social entries",
            "post page",
            "content client",
            "comment submission",
            "comment auth",
            "feed",
        ] {
            add_search_plan_term(term, extracted, seen, dropped, dropped_seen);
        }
    }
}

fn search_plan_query_has_exec_json_flow(lower_query: &str) -> bool {
    search_plan_query_has_token(lower_query, "exec")
        && (search_plan_query_has_token(lower_query, "json")
            || search_plan_query_has_token(lower_query, "jsonl"))
        && (search_plan_query_has_token(lower_query, "event")
            || search_plan_query_has_token(lower_query, "events")
            || search_plan_query_has_token(lower_query, "output"))
}

fn search_plan_query_has_token(lower_query: &str, token: &str) -> bool {
    lower_query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == token)
}

fn search_plan_query_has_payload_content_flow(lower_query: &str) -> bool {
    search_plan_query_has_token(lower_query, "payload")
        && (search_plan_query_has_token(lower_query, "posts")
            || search_plan_query_has_token(lower_query, "post")
            || search_plan_query_has_token(lower_query, "writing"))
        && (search_plan_query_has_token(lower_query, "comments")
            || search_plan_query_has_token(lower_query, "comment")
            || search_plan_query_has_token(lower_query, "feed")
            || search_plan_query_has_token(lower_query, "rss")
            || search_plan_query_has_token(lower_query, "elsewhere")
            || search_plan_query_has_token(lower_query, "social"))
}

fn drop_search_plan_brand_terms_for_content_flow(
    query: &str,
    extracted: &mut Vec<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
) {
    let lower = query.to_ascii_lowercase();
    if !(search_plan_query_has_payload_content_flow(&lower)
        && search_plan_query_has_token(&lower, "root")
        && search_plan_query_has_token(&lower, "runtime"))
    {
        return;
    }

    extracted.retain(|term| {
        let is_brand = term.eq_ignore_ascii_case("root") || term.eq_ignore_ascii_case("runtime");
        if is_brand {
            dropped.push(SearchPlanDroppedTermDto {
                term: term.clone(),
                reason: "brand_phrase_in_content_flow".to_string(),
            });
        }
        !is_brand
    });
}

fn add_search_plan_term(
    raw: &str,
    extracted: &mut Vec<String>,
    seen: &mut HashSet<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
    dropped_seen: &mut HashSet<String>,
) {
    let clean = raw
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .to_string();
    if clean.is_empty() {
        return;
    }
    let lower = clean.to_ascii_lowercase();
    if lower.len() < 3 {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "too_short".to_string(),
            });
        }
        return;
    }
    if SEARCH_PLAN_STOPWORDS.contains(&lower.as_str()) {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "natural_language_filler".to_string(),
            });
        }
        return;
    }
    let value = if clean.chars().any(|ch| ch.is_ascii_uppercase()) && clean.len() > 3 {
        clean
    } else {
        lower.clone()
    };
    if seen.insert(value.to_ascii_lowercase()) {
        extracted.push(value);
    }
}

fn split_camel_identifier(value: &str) -> Vec<String> {
    if !value.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
            continue;
        }
        if ch.is_ascii_uppercase() && !current.is_empty() {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
        }
        current.push(ch.to_ascii_lowercase());
    }
    if current.len() >= 3 {
        parts.push(current);
    }
    parts
}

fn search_plan_eligible(query: &str, exact_symbol_hit_count: u32, intents: &[String]) -> bool {
    let broad_query = looks_like_repo_text_query(query) || query.split_whitespace().count() >= 4;
    let has_seed_anchors = query.contains(SEARCH_PLAN_SEED_ANCHOR_MARKER);
    let broad_explanation_prompt =
        search_plan_broad_explanation_prompt_with_architecture_terms(query);
    !intents.is_empty()
        && broad_query
        && (exact_symbol_hit_count == 0 || has_seed_anchors || broad_explanation_prompt)
}

fn search_plan_broad_explanation_prompt_with_architecture_terms(query: &str) -> bool {
    let lower = query.to_ascii_lowercase();
    let asks_for_flow = lower.contains("explain how")
        || lower.contains("trace how")
        || lower.starts_with("how ")
        || lower.contains(" how ");
    if !asks_for_flow {
        return false;
    }
    let tokens = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<HashSet<_>>();
    [
        "cli",
        "command",
        "runtime",
        "workspace",
        "indexer",
        "indexing",
        "store",
        "storage",
        "persistence",
        "snapshot",
        "search",
        "trail",
        "snippet",
        "configuration",
        "source",
        "activation",
        "host",
        "execution",
    ]
    .iter()
    .filter(|term| tokens.contains(**term))
    .count()
        >= 3
}

fn search_plan_subqueries(
    query: &str,
    terms: &SearchPlanTermsDto,
    intents: &[String],
) -> Vec<SearchPlanSubqueryDto> {
    if intents.is_empty() {
        return Vec::new();
    }
    let mut subqueries = Vec::new();
    let mut seen = HashSet::new();

    push_search_plan_subquery(
        &mut subqueries,
        &mut seen,
        query.trim().to_string(),
        "original_question",
        vec![
            SearchPlanChannelDto::Semantic,
            SearchPlanChannelDto::Lexical,
        ],
    );
    push_search_plan_seed_anchor_subqueries(&mut subqueries, &mut seen, query);
    push_search_plan_explicit_anchor_subqueries(&mut subqueries, &mut seen, query);
    push_search_plan_symbol_term_subquery(&mut subqueries, &mut seen, terms);
    push_search_plan_role_subqueries(&mut subqueries, &mut seen, terms);
    push_search_plan_named_anchor_subqueries(&mut subqueries, &mut seen, terms);
    push_search_plan_fallback_subquery(&mut subqueries, &mut seen, terms);
    subqueries
}

fn push_search_plan_seed_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: &str,
) {
    for anchor in search_plan_seed_anchor_terms(query)
        .into_iter()
        .take(SEARCH_PLAN_MAX_SEED_ANCHORS)
    {
        push_required_search_plan_subquery(
            subqueries,
            seen,
            anchor,
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

fn search_plan_seed_anchor_terms(query: &str) -> Vec<String> {
    query
        .split(SEARCH_PLAN_SEED_ANCHOR_MARKER)
        .skip(1)
        .map(|rest| rest.lines().next().unwrap_or(rest))
        .flat_map(|anchors| anchors.split(','))
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn push_search_plan_explicit_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: &str,
) {
    for anchor in search_plan_explicit_anchor_terms(query) {
        push_required_search_plan_subquery(
            subqueries,
            seen,
            anchor,
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

fn search_plan_explicit_anchor_terms(query: &str) -> Vec<String> {
    let Some(rest) = query.split(SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER).nth(1) else {
        return Vec::new();
    };
    let anchor_text = rest.split('.').next().unwrap_or(rest).trim();
    anchor_text
        .split(',')
        .map(|part| part.trim().trim_start_matches("and ").trim())
        .filter(|anchor| !anchor.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn push_search_plan_symbol_term_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    let symbol_terms = sorted_search_plan_symbol_terms(terms);
    if symbol_terms.is_empty() {
        return;
    }
    push_search_plan_subquery(
        subqueries,
        seen,
        symbol_terms
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        "typed_anchor_terms",
        vec![
            SearchPlanChannelDto::TypedSymbol,
            SearchPlanChannelDto::Lexical,
        ],
    );
}

fn push_search_plan_named_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    let symbol_terms = sorted_search_plan_symbol_terms(terms);
    for term in symbol_terms
        .iter()
        .filter(|term| search_plan_named_anchor_term(term))
        .take(5)
    {
        push_search_plan_subquery(
            subqueries,
            seen,
            term.clone(),
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

fn sorted_search_plan_symbol_terms(terms: &SearchPlanTermsDto) -> Vec<String> {
    let mut symbol_terms = terms
        .extracted
        .iter()
        .filter(|term| search_plan_symbol_term(term))
        .cloned()
        .collect::<Vec<_>>();
    symbol_terms.sort_by(|left, right| {
        search_plan_symbol_subquery_term_score(right)
            .cmp(&search_plan_symbol_subquery_term_score(left))
            .then_with(|| left.cmp(right))
    });
    symbol_terms
}

fn search_plan_symbol_term(term: &str) -> bool {
    term.chars().any(|ch| ch.is_ascii_uppercase())
        || SEARCH_PLAN_SYMBOL_TERMS
            .iter()
            .any(|symbol_term| term.eq_ignore_ascii_case(symbol_term))
}

fn search_plan_symbol_subquery_term_score(term: &str) -> u32 {
    let uppercase_count = term.chars().filter(|ch| ch.is_ascii_uppercase()).count() as u32;
    let lowercase_count = term.chars().filter(|ch| ch.is_ascii_lowercase()).count() as u32;
    let mut score = term.len().min(40) as u32;
    if uppercase_count >= 2 && lowercase_count > 0 {
        score += 120;
    } else if uppercase_count > 0 && lowercase_count > 0 {
        score += 40;
    }
    if term.contains('_') || term.contains('-') {
        score += 35;
    }
    if SEARCH_PLAN_SYMBOL_TERMS
        .iter()
        .any(|symbol_term| term.eq_ignore_ascii_case(symbol_term))
    {
        score += 20;
    }
    score
}

fn search_plan_named_anchor_term(term: &str) -> bool {
    let uppercase_count = term.chars().filter(|ch| ch.is_ascii_uppercase()).count();
    let lowercase_count = term.chars().filter(|ch| ch.is_ascii_lowercase()).count();
    uppercase_count >= 1 && lowercase_count > 0 && term.len() >= 4
}

fn push_search_plan_role_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    for (role, needles) in SEARCH_PLAN_ROLE_SPECS {
        let role_terms = search_plan_matching_terms(terms, needles);
        if role_terms.len() >= 2 {
            push_search_plan_subquery(
                subqueries,
                seen,
                role_terms.join(" "),
                role,
                vec![
                    SearchPlanChannelDto::TypedSymbol,
                    SearchPlanChannelDto::Lexical,
                    SearchPlanChannelDto::RepoText,
                ],
            );
        }
    }
}

fn search_plan_matching_terms(terms: &SearchPlanTermsDto, needles: &[&str]) -> Vec<String> {
    terms
        .extracted
        .iter()
        .filter(|term| {
            needles
                .iter()
                .any(|needle| term.eq_ignore_ascii_case(needle))
        })
        .cloned()
        .collect()
}

fn push_search_plan_fallback_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    if subqueries.len() >= 3 {
        return;
    }
    let fallback = terms
        .extracted
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    push_search_plan_subquery(
        subqueries,
        seen,
        fallback,
        "repo_text_terms",
        vec![
            SearchPlanChannelDto::RepoText,
            SearchPlanChannelDto::Lexical,
        ],
    );
}

fn push_search_plan_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: String,
    role: &str,
    channels: Vec<SearchPlanChannelDto>,
) {
    let key = query.to_ascii_lowercase();
    if !query.trim().is_empty()
        && seen.insert(key)
        && subqueries.len() < SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT
    {
        subqueries.push(SearchPlanSubqueryDto {
            query,
            role: role.to_string(),
            channels,
        });
    }
}

fn push_required_search_plan_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: String,
    role: &str,
    channels: Vec<SearchPlanChannelDto>,
) {
    let key = query.to_ascii_lowercase();
    if !query.trim().is_empty() && seen.insert(key) {
        subqueries.push(SearchPlanSubqueryDto {
            query,
            role: role.to_string(),
            channels,
        });
    }
}

fn search_plan_subqueries_for_repo_text_mode(
    subqueries: Vec<SearchPlanSubqueryDto>,
    allow_repo_text: bool,
) -> Vec<SearchPlanSubqueryDto> {
    if allow_repo_text {
        return subqueries;
    }

    subqueries
        .into_iter()
        .filter_map(|mut subquery| {
            subquery
                .channels
                .retain(|channel| *channel != SearchPlanChannelDto::RepoText);
            (!subquery.channels.is_empty()).then_some(subquery)
        })
        .collect()
}

fn search_plan_candidate_window(
    channel: SearchPlanChannelDto,
    subquery: &SearchPlanSubqueryDto,
    limit_per_source: u32,
    returned_count: usize,
    truncated: bool,
    reason: &str,
) -> SearchPlanCandidateWindowDto {
    SearchPlanCandidateWindowDto {
        channel,
        subquery: subquery.query.clone(),
        limit: limit_per_source,
        returned_count: clamp_usize_to_u32(returned_count),
        truncated,
        score_reasons: vec![reason.to_string()],
    }
}

fn search_plan_symbol_windows(
    subquery: &SearchPlanSubqueryDto,
    hits: &[SearchHit],
    suggestions: &[SearchHit],
    limit_per_source: u32,
    semantic_ready: bool,
) -> Vec<SearchPlanCandidateWindowDto> {
    let mut windows = Vec::new();
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::TypedSymbol) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::TypedSymbol,
            subquery,
            limit_per_source,
            hits.iter().filter(|hit| hit.resolvable).count(),
            hits.len() >= limit_per_source as usize,
            "executed typed-symbol retrieval for this planned subquery",
        ));
    }
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::Lexical) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::Lexical,
            subquery,
            limit_per_source,
            search_plan_lexical_hit_count(hits),
            hits.len() >= limit_per_source as usize,
            "executed lexical/name/path retrieval for this planned subquery",
        ));
    }
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::Semantic) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::Semantic,
            subquery,
            limit_per_source,
            search_plan_semantic_hit_count(hits, suggestions),
            suggestions.len() >= limit_per_source as usize,
            if semantic_ready {
                "executed semantic retrieval for this planned subquery"
            } else {
                "semantic retrieval unavailable; this subquery relied on lexical indexed evidence"
            },
        ));
    }
    windows
}

fn search_plan_uses_channel(
    subquery: &SearchPlanSubqueryDto,
    channel: SearchPlanChannelDto,
) -> bool {
    subquery.channels.contains(&channel)
}

fn search_plan_lexical_hit_count(hits: &[SearchHit]) -> usize {
    hits.iter()
        .filter(|hit| {
            hit.score_breakdown
                .as_ref()
                .is_none_or(|breakdown| breakdown.lexical > 0.0)
        })
        .count()
}

fn search_plan_semantic_hit_count(hits: &[SearchHit], suggestions: &[SearchHit]) -> usize {
    hits.iter()
        .chain(suggestions.iter())
        .filter(|hit| {
            hit.score_breakdown
                .as_ref()
                .is_some_and(|breakdown| breakdown.semantic > 0.0)
        })
        .count()
}

fn same_search_file(left: &SearchHit, right: &SearchHit) -> bool {
    let Some(left_path) = left.file_path.as_deref() else {
        return false;
    };
    let Some(right_path) = right.file_path.as_deref() else {
        return false;
    };
    normalize_path_key(left_path) == normalize_path_key(right_path)
}

fn hit_matches_identifier(hit: &SearchHit, identifier: &str) -> bool {
    hit_exactly_matches_identifier(hit, identifier) || {
        let normalized_identifier = normalize_symbol_query(identifier);
        !normalized_identifier.is_empty()
            && leading_symbol_segment(&hit.display_name) == normalized_identifier
    }
}

fn hit_exactly_matches_identifier(hit: &SearchHit, identifier: &str) -> bool {
    let normalized_identifier = normalize_symbol_query(identifier);
    if normalized_identifier.is_empty() {
        return false;
    }
    normalize_symbol_query(&hit.display_name) == normalized_identifier
        || terminal_symbol_segment(&hit.display_name) == normalized_identifier
}

fn repo_text_line_identifiers(hit: &SearchHit) -> Vec<String> {
    let Some(path) = hit.file_path.as_deref() else {
        return Vec::new();
    };
    let Some(line) = hit.line else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let start = line.saturating_sub(3) as usize;
    let window = contents
        .lines()
        .skip(start)
        .take(5)
        .collect::<Vec<_>>()
        .join("\n");
    let mut identifiers = Vec::new();
    let mut seen = HashSet::new();
    for token in window.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        if token.len() < 3 {
            continue;
        }
        let looks_symbolic = token.chars().any(|ch| ch.is_ascii_uppercase())
            || token.contains('_')
            || matches!(
                token,
                "auth" | "feed" | "posts" | "storage" | "indexer" | "service" | "trail" | "snippet"
            );
        if looks_symbolic && seen.insert(token.to_ascii_lowercase()) {
            identifiers.push(token.to_string());
        }
    }
    identifiers
}

fn repo_text_identifiers_at(identifiers: &[Vec<String>], index: usize) -> &[String] {
    identifiers
        .get(index)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn repo_text_mentions_hit(identifiers: &[String], hit: &SearchHit) -> bool {
    identifiers
        .iter()
        .any(|identifier| hit_matches_identifier(hit, identifier))
}

fn search_plan_group_confidence(query: &str, hit: &SearchHit) -> String {
    let exact = search_hit_match_quality(query, hit);
    if matches!(
        exact,
        SearchMatchQualityDto::Exact | SearchMatchQualityDto::NormalizedExact
    ) {
        "high".to_string()
    } else {
        "medium".to_string()
    }
}

fn search_plan_typed_anchor_group(
    query: &str,
    hit: &SearchHit,
    repo_text_hits: &[SearchHit],
    repo_text_identifiers: &[Vec<String>],
    used_repo_text: &mut HashSet<usize>,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanAnchorGroupDto {
    let mut supporting_hits = vec![hit.clone()];
    let mut reasons = vec!["typed indexed symbol selected as an anchor candidate".to_string()];
    let active_path = search_plan_active_path_for_hit(hit, active_path_evidence);
    reasons.extend(search_plan_active_path_reasons(hit, active_path));

    for (repo_index, repo_hit) in repo_text_hits.iter().enumerate() {
        if !same_search_file(hit, repo_hit) {
            continue;
        }
        let identifiers = repo_text_identifiers_at(repo_text_identifiers, repo_index);
        if identifiers.is_empty() {
            supporting_hits.push(repo_hit.clone());
            used_repo_text.insert(repo_index);
            reasons.push("repo-text hit appears in the same file as the anchor".to_string());
        } else if repo_text_mentions_hit(identifiers, hit) {
            supporting_hits.push(repo_hit.clone());
            used_repo_text.insert(repo_index);
            reasons.push("repo-text hit names this anchor in the same file".to_string());
        }
    }

    SearchPlanAnchorGroupDto {
        anchor: hit.display_name.clone(),
        chosen_symbol: Some(hit.clone()),
        supporting_hits,
        promotion_status: SearchPlanPromotionStatusDto::TypedAnchor,
        promotion_method: Some("indexed_symbol".to_string()),
        caller_count: active_path.caller_count,
        definition_only: search_plan_definition_only(hit, active_path),
        no_visible_callers: search_plan_no_visible_callers(hit, active_path),
        confidence: search_plan_group_confidence(query, hit),
        reasons,
    }
}

fn search_plan_promoted_anchor_group(
    symbol: SearchHit,
    repo_hit: &SearchHit,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanAnchorGroupDto {
    let active_path = search_plan_active_path_for_hit(&symbol, active_path_evidence);
    let mut reasons =
        vec!["repo-text lead was promoted to an indexed symbol in the same file".to_string()];
    reasons.extend(search_plan_active_path_reasons(&symbol, active_path));
    let definition_only = search_plan_definition_only(&symbol, active_path);
    let no_visible_callers = search_plan_no_visible_callers(&symbol, active_path);

    SearchPlanAnchorGroupDto {
        anchor: symbol.display_name.clone(),
        chosen_symbol: Some(symbol.clone()),
        supporting_hits: vec![symbol, repo_hit.clone()],
        promotion_status: SearchPlanPromotionStatusDto::Promoted,
        promotion_method: Some("same_file_exact_identifier".to_string()),
        caller_count: active_path.caller_count,
        definition_only,
        no_visible_callers,
        confidence: "medium".to_string(),
        reasons,
    }
}

fn search_plan_unbound_repo_text_group(
    repo_hit: &SearchHit,
    identifiers: &[String],
) -> SearchPlanAnchorGroupDto {
    SearchPlanAnchorGroupDto {
        anchor: repo_hit.display_name.clone(),
        chosen_symbol: None,
        supporting_hits: vec![repo_hit.clone()],
        promotion_status: if identifiers.is_empty() {
            SearchPlanPromotionStatusDto::NeedsSourceRead
        } else {
            SearchPlanPromotionStatusDto::Ambiguous
        },
        promotion_method: None,
        caller_count: 0,
        definition_only: false,
        no_visible_callers: false,
        confidence: "low".to_string(),
        reasons: vec![
            "repo-text lead could not be bound to one indexed symbol before source reads"
                .to_string(),
        ],
    }
}

fn search_plan_active_path_for_hit(
    hit: &SearchHit,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanActivePathEvidence {
    active_path_evidence
        .get(&hit.node_id)
        .copied()
        .unwrap_or_default()
}

fn search_plan_active_path_reasons(
    hit: &SearchHit,
    active_path: SearchPlanActivePathEvidence,
) -> Vec<String> {
    if !search_plan_callable_hit(hit) {
        return Vec::new();
    }
    if active_path.caller_count > 0 {
        vec![format!(
            "call graph shows {} visible production caller(s); rank as active-path evidence",
            active_path.caller_count
        )]
    } else {
        vec![
            "call graph shows no visible production callers; treat as definition-only evidence unless a framework/runtime entry path is source-verified"
                .to_string(),
        ]
    }
}

fn search_plan_definition_only(hit: &SearchHit, active_path: SearchPlanActivePathEvidence) -> bool {
    search_plan_callable_hit(hit) && active_path.caller_count == 0
}

fn search_plan_no_visible_callers(
    hit: &SearchHit,
    active_path: SearchPlanActivePathEvidence,
) -> bool {
    search_plan_callable_hit(hit) && active_path.caller_count == 0
}

fn search_plan_callable_hit(hit: &SearchHit) -> bool {
    matches!(
        hit.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

fn search_plan_runtime_call_is_speculative(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
    confidence: Option<f32>,
) -> bool {
    matches!(
        certainty.or_else(|| {
            codestory_contracts::graph::ResolutionCertainty::from_confidence(confidence)
        }),
        Some(
            codestory_contracts::graph::ResolutionCertainty::Uncertain
                | codestory_contracts::graph::ResolutionCertainty::Probable
        )
    ) || confidence.is_some_and(|confidence| {
        confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
    })
}

fn search_plan_caller_is_test_or_bench(storage: &Storage, caller_id: GraphNodeId) -> bool {
    let Ok(Some(caller)) = storage.get_node(caller_id) else {
        return false;
    };
    let Ok(Some(path)) = AppController::file_path_for_node(storage, &caller) else {
        return false;
    };
    search_plan_path_is_test_or_bench(&path)
}

fn search_plan_path_is_test_or_bench(path: &str) -> bool {
    matches!(
        retrieval_file_role_from_path(path),
        RetrievalFileRole::Test | RetrievalFileRole::Benchmark
    )
}

fn search_plan_anchor_groups(
    query: &str,
    terms: &SearchPlanTermsDto,
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    suggestions: &[SearchHit],
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> Vec<SearchPlanAnchorGroupDto> {
    let mut groups = Vec::new();
    let mut grouped_ids = HashSet::new();
    let mut used_repo_text = HashSet::new();
    let mut all_symbol_hits = indexed_hits
        .iter()
        .chain(suggestions.iter())
        .cloned()
        .collect::<Vec<_>>();
    all_symbol_hits
        .sort_by(|left, right| compare_search_hits_with_project_root(None, query, left, right));
    let repo_text_identifiers = repo_text_hits
        .iter()
        .map(repo_text_line_identifiers)
        .collect::<Vec<_>>();

    let mut grouped_anchor_names = HashSet::new();
    for hit in all_symbol_hits.iter().filter(|hit| hit.resolvable).take(16) {
        if !grouped_ids.insert(hit.node_id.clone()) {
            continue;
        }
        let anchor_name = hit.display_name.to_ascii_lowercase();
        if !grouped_anchor_names.insert(anchor_name) {
            continue;
        }
        groups.push(search_plan_typed_anchor_group(
            query,
            hit,
            repo_text_hits,
            &repo_text_identifiers,
            &mut used_repo_text,
            active_path_evidence,
        ));
    }

    for (repo_index, repo_hit) in repo_text_hits.iter().enumerate() {
        if used_repo_text.contains(&repo_index) {
            continue;
        }
        let identifiers = repo_text_identifiers_at(&repo_text_identifiers, repo_index);
        let promoted = identifiers.iter().find_map(|identifier| {
            all_symbol_hits
                .iter()
                .find(|hit| {
                    same_search_file(hit, repo_hit)
                        && hit_exactly_matches_identifier(hit, identifier)
                })
                .cloned()
        });
        if let Some(symbol) = promoted {
            if !grouped_ids.insert(symbol.node_id.clone()) {
                continue;
            }
            groups.push(search_plan_promoted_anchor_group(
                symbol,
                repo_hit,
                active_path_evidence,
            ));
        } else {
            groups.push(search_plan_unbound_repo_text_group(repo_hit, identifiers));
        }
    }

    let term_set = terms
        .extracted
        .iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    groups.sort_by(|left, right| {
        search_plan_group_score(right, &term_set)
            .cmp(&search_plan_group_score(left, &term_set))
            .then_with(|| left.anchor.cmp(&right.anchor))
    });
    groups.truncate(8);
    groups
}

fn search_plan_group_score(group: &SearchPlanAnchorGroupDto, terms: &HashSet<String>) -> u32 {
    let mut score = 0;
    if group.chosen_symbol.is_some() {
        score += 100;
    }
    if group.caller_count > 0 {
        score += 90 + group.caller_count.min(5) * 6;
    }
    score += match group.promotion_status {
        SearchPlanPromotionStatusDto::TypedAnchor => 60,
        SearchPlanPromotionStatusDto::Promoted => 45,
        SearchPlanPromotionStatusDto::Ambiguous => 10,
        SearchPlanPromotionStatusDto::NeedsSourceRead => 0,
    };
    if group
        .supporting_hits
        .iter()
        .any(|hit| hit.origin == SearchHitOrigin::TextMatch)
    {
        score += 12;
    }
    if group
        .promotion_method
        .as_deref()
        .is_some_and(|method| method == "same_file_exact_identifier")
        || group
            .reasons
            .iter()
            .any(|reason| reason.contains("names this anchor"))
    {
        score += 35;
    }
    let text = group
        .chosen_symbol
        .as_ref()
        .map(|hit| {
            format!(
                "{} {}",
                hit.display_name,
                hit.file_path.as_deref().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| group.anchor.clone())
        .to_ascii_lowercase();
    score
        + terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count() as u32
            * 8
}

fn search_plan_rejected_hits(
    anchor_groups: &[SearchPlanAnchorGroupDto],
    suggestions: &[SearchHit],
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> Vec<SearchPlanRejectedHitDto> {
    let chosen = anchor_groups
        .iter()
        .filter_map(|group| group.chosen_symbol.as_ref().map(|hit| hit.node_id.clone()))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut rejected = suggestions
        .iter()
        .chain(indexed_hits.iter())
        .chain(repo_text_hits.iter())
        .filter(|hit| !chosen.contains(&hit.node_id) && seen.insert(hit.node_id.clone()))
        .map(|hit| {
            let coverage = architecture_coverage_for_hit(hit);
            let coverage_score = coverage
                .as_ref()
                .map(|coverage| coverage.score)
                .unwrap_or(0);
            (hit, coverage, coverage_score)
        })
        .collect::<Vec<_>>();

    rejected.sort_by(
        |(left, left_coverage, left_score), (right, right_coverage, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| right_coverage.is_some().cmp(&left_coverage.is_some()))
                .then_with(|| {
                    (right.origin == SearchHitOrigin::TextMatch)
                        .cmp(&(left.origin == SearchHitOrigin::TextMatch))
                })
        },
    );

    rejected
        .into_iter()
        .take(8)
        .map(|(hit, coverage, _)| SearchPlanRejectedHitDto {
            display_name: hit.display_name.clone(),
            reason: search_plan_rejected_hit_reason(hit, coverage.as_ref()),
            origin: hit.origin,
            file_path: hit.file_path.clone(),
            line: hit.line,
        })
        .collect()
}

fn search_plan_rejected_hit_reason(
    hit: &SearchHit,
    coverage: Option<&ArchitectureCoverage>,
) -> String {
    let source = match hit.origin {
        SearchHitOrigin::IndexedSymbol => "indexed_symbol",
        SearchHitOrigin::TextMatch => "repo_text",
    };
    if let Some(coverage) = coverage {
        format!(
            "not selected after anchor grouping and final coverage ranking; source={source}; coverage_key={}; coverage_score={}",
            coverage.key, coverage.score
        )
    } else {
        format!("not selected after anchor grouping and evidence ranking; source={source}")
    }
}

fn search_plan_bridge_request(from: &NodeId, to: &NodeId) -> TrailConfigDto {
    TrailConfigDto {
        root_id: from.clone(),
        mode: codestory_contracts::api::TrailMode::ToTargetSymbol,
        target_id: Some(to.clone()),
        depth: 0,
        direction: codestory_contracts::api::TrailDirection::Outgoing,
        caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: true,
        story: false,
        node_filter: Vec::new(),
        max_nodes: 80,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }
}

fn graph_response_has_bridge(
    graph: &codestory_contracts::api::GraphResponse,
    from: &NodeId,
    to: &NodeId,
) -> bool {
    if from == to {
        return true;
    }
    graph.nodes.iter().any(|node| node.id == *from)
        && graph.nodes.iter().any(|node| node.id == *to)
        && !graph.edges.is_empty()
}

fn graph_bridge_evidence_kind(graph: &GraphResponse) -> SearchPlanBridgeEvidenceKindDto {
    if graph.edges.iter().any(|edge| {
        edge.callsite_identity
            .as_deref()
            .is_some_and(|identity| identity.starts_with("payload:"))
    }) || graph
        .nodes
        .iter()
        .any(|node| node.label.contains("payload collection "))
    {
        return SearchPlanBridgeEvidenceKindDto::DataCollectionUsage;
    }
    if graph.nodes.iter().any(|node| {
        node.label.contains(" route; confidence=")
            || node
                .qualified_name
                .as_deref()
                .is_some_and(|name| name.starts_with("framework::"))
    }) {
        return SearchPlanBridgeEvidenceKindDto::FrameworkRoute;
    }
    if graph.edges.iter().any(|edge| edge.kind == EdgeKind::CALL)
        && graph.nodes.iter().any(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.file_path.as_deref().is_some_and(|path| {
                    let path = path.to_ascii_lowercase();
                    path.ends_with(".tsx") || path.ends_with(".jsx")
                })
                && node
                    .label
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
        })
    {
        return SearchPlanBridgeEvidenceKindDto::ComponentUsage;
    }
    SearchPlanBridgeEvidenceKindDto::GraphPath
}

fn shared_file_bridge(from: &SearchHit, to: &SearchHit) -> bool {
    same_search_file(from, to)
}

fn search_plan_next_actions(groups: &[SearchPlanAnchorGroupDto]) -> Vec<SearchPlanNextActionDto> {
    groups
        .iter()
        .filter_map(|group| group.chosen_symbol.as_ref())
        .take(4)
        .flat_map(|hit| {
            [
                SearchPlanNextActionDto {
                    action: "symbol".to_string(),
                    node_id: hit.node_id.clone(),
                    options: Vec::new(),
                },
                SearchPlanNextActionDto {
                    action: "trail".to_string(),
                    node_id: hit.node_id.clone(),
                    options: vec!["story".to_string(), "hide_speculative".to_string()],
                },
                SearchPlanNextActionDto {
                    action: "snippet".to_string(),
                    node_id: hit.node_id.clone(),
                    options: vec!["function_body".to_string(), "context=40".to_string()],
                },
            ]
        })
        .collect()
}

fn search_plan_source_truth_checks(groups: &[SearchPlanAnchorGroupDto]) -> Vec<String> {
    let mut checks = SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS
        .iter()
        .map(|check| (*check).to_string())
        .collect::<Vec<_>>();
    if search_plan_has_unbound_repo_text_group(groups) {
        checks.push(SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK.to_string());
    }
    checks
}

fn search_plan_has_unbound_repo_text_group(groups: &[SearchPlanAnchorGroupDto]) -> bool {
    groups.iter().any(|group| {
        matches!(
            group.promotion_status,
            SearchPlanPromotionStatusDto::NeedsSourceRead | SearchPlanPromotionStatusDto::Ambiguous
        )
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

#[cfg(test)]
fn markdown_snippet(text: &str, focus_line: Option<u32>, context: usize) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    if all_lines.is_empty() {
        return String::new();
    }

    let line_index = focus_line
        .and_then(|line| line.checked_sub(1))
        .map(|line| line as usize)
        .unwrap_or(0)
        .min(all_lines.len().saturating_sub(1));

    let start = line_index.saturating_sub(context);
    let end = (line_index + context + 1).min(all_lines.len());

    let mut out = String::new();
    out.push_str("```text\n");
    for (idx, line) in all_lines[start..end].iter().enumerate() {
        let source_line = start + idx + 1;
        let marker = if source_line == line_index + 1 {
            ">"
        } else {
            " "
        };
        let _ = writeln!(out, "{marker}{source_line:>5} | {line}");
    }
    out.push_str("```");
    out
}

fn truncate_to_byte_cap(mut text: String, max_bytes: usize, suffix: &str) -> BoundedSnippet {
    if text.len() <= max_bytes {
        return BoundedSnippet {
            markdown: text,
            truncated: false,
        };
    }

    let mut keep = max_bytes.saturating_sub(suffix.len());
    while keep > 0 && !text.is_char_boundary(keep) {
        keep -= 1;
    }
    text.truncate(keep);
    text.push_str(suffix);
    if text.len() > max_bytes {
        let mut hard_keep = max_bytes;
        while hard_keep > 0 && !text.is_char_boundary(hard_keep) {
            hard_keep -= 1;
        }
        text.truncate(hard_keep);
    }

    BoundedSnippet {
        markdown: text,
        truncated: true,
    }
}

#[cfg(test)]
pub(crate) fn bounded_direct_markdown_snippet(
    text: &str,
    focus_line: Option<u32>,
    context: usize,
) -> BoundedSnippet {
    let markdown = markdown_snippet(
        text,
        focus_line,
        context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP),
    );
    truncate_to_byte_cap(
        markdown,
        DIRECT_SNIPPET_MAX_BYTES,
        DIRECT_SNIPPET_TRUNCATION_SUFFIX,
    )
}

fn bounded_markdown_snippet_from_path(
    path: &Path,
    focus_line: u32,
    context: usize,
    max_bytes: usize,
    truncation_suffix: &str,
) -> io::Result<BoundedSnippet> {
    let file = std::fs::File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let context = context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP);
    let focus = focus_line.max(1) as usize;
    let start = focus.saturating_sub(context).max(1);
    let end = focus.saturating_add(context);
    let mut line_no = 0usize;
    let mut line = String::new();
    let mut out = String::from("```text\n");
    let mut truncated = false;

    loop {
        let (read, line_truncated) = read_line_capped(&mut reader, &mut line, max_bytes)?;
        if read == 0 {
            break;
        }
        line_no = line_no.saturating_add(1);
        if line_no > end {
            break;
        }
        if line_no >= start {
            truncated |= line_truncated;
            let marker = if line_no == focus { ">" } else { " " };
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let _ = writeln!(out, "{marker}{line_no:>5} | {trimmed}");
        }
    }

    Ok(finish_bounded_file_snippet(
        out,
        truncated,
        max_bytes,
        truncation_suffix,
    ))
}

fn bounded_markdown_snippet_range_from_path(
    path: &Path,
    focus_line: u32,
    start_line: u32,
    end_line: u32,
    context: usize,
    max_bytes: usize,
    truncation_suffix: &str,
) -> io::Result<BoundedSnippet> {
    let file = std::fs::File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let context = context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP) as u32;
    let focus = focus_line.max(1);
    let start = start_line.saturating_sub(context).max(1);
    let end = end_line.max(start_line).saturating_add(context);
    let mut line_no = 0u32;
    let mut line = String::new();
    let mut out = String::from("```text\n");
    let mut truncated = false;

    loop {
        let (read, line_truncated) = read_line_capped(&mut reader, &mut line, max_bytes)?;
        if read == 0 {
            break;
        }
        line_no = line_no.saturating_add(1);
        if line_no > end {
            break;
        }
        if line_no >= start {
            truncated |= line_truncated;
            let marker = if line_no == focus { ">" } else { " " };
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let _ = writeln!(out, "{marker}{line_no:>5} | {trimmed}");
        }
    }

    Ok(finish_bounded_file_snippet(
        out,
        truncated,
        max_bytes,
        truncation_suffix,
    ))
}

fn finish_bounded_file_snippet(
    mut out: String,
    truncated: bool,
    max_bytes: usize,
    truncation_suffix: &str,
) -> BoundedSnippet {
    if out == "```text\n" {
        return BoundedSnippet {
            markdown: String::new(),
            truncated: false,
        };
    }
    out.push_str("```");
    if out.len() > max_bytes {
        return truncate_to_byte_cap(out, max_bytes, truncation_suffix);
    }
    if truncated {
        if out.ends_with("```") {
            out.truncate(out.len().saturating_sub(3));
        }
        if out.len().saturating_add(truncation_suffix.len()) <= max_bytes {
            out.push_str(truncation_suffix);
            return BoundedSnippet {
                markdown: out,
                truncated: true,
            };
        }
        return truncate_to_byte_cap(out, max_bytes, truncation_suffix);
    }
    BoundedSnippet {
        markdown: out,
        truncated: false,
    }
}

fn read_line_capped<R: BufRead>(
    reader: &mut R,
    out: &mut String,
    max_line_bytes: usize,
) -> io::Result<(usize, bool)> {
    out.clear();
    let mut total = 0usize;
    let mut truncated = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((total, truncated));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take_len = newline.map(|pos| pos + 1).unwrap_or(available.len());
        let chunk = &available[..take_len];
        total = total.saturating_add(chunk.len());

        if out.len() < max_line_bytes {
            let remaining = max_line_bytes - out.len();
            let copy_len = chunk.len().min(remaining);
            out.push_str(&String::from_utf8_lossy(&chunk[..copy_len]));
            truncated |= copy_len < chunk.len();
        } else if !chunk.is_empty() {
            truncated = true;
        }

        reader.consume(take_len);
        if newline.is_some() {
            return Ok((total, truncated));
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SemanticProjectionStats {
    reported: bool,
    semantic_context_index_ms: u32,
    node_load_ms: u32,
    node_load_rows: u32,
    node_stream_batches: u32,
    endpoint_load_ms: u32,
    endpoint_load_rows: u32,
    endpoint_load_batches: u32,
    selected_nodes: u32,
    context_file_count: u32,
    context_path_bytes: u32,
    node_lookup_entries: u32,
    context_ms: u32,
    doc_build_ms: u32,
    embedding_ms: u32,
    db_upsert_ms: u32,
    reload_ms: u32,
    prune_ms: u32,
    docs_reused: u32,
    docs_embedded: u32,
    docs_pending: u32,
    docs_stale: u32,
    symbol_search_docs_written: u32,
    dense_docs_skipped: u32,
    dense_public_api: u32,
    dense_entrypoint: u32,
    dense_documented_nontrivial: u32,
    dense_central_graph_node: u32,
    dense_component_report: u32,
    dense_unstructured_doc: u32,
}

struct ComponentReportRefreshScope {
    previous_file_paths: HashMap<codestory_contracts::graph::NodeId, String>,
    removed_component_keys: HashSet<String>,
}

#[derive(Clone, Copy)]
struct SemanticRefreshScope<'a> {
    file_ids: Option<&'a HashSet<codestory_contracts::graph::NodeId>>,
    component_reports: Option<&'a ComponentReportRefreshScope>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SearchStateBuildStats {
    search_projection_rebuild_ms: u32,
    search_symbol_stream_ms: u32,
    search_symbol_stream_rows: u32,
    search_symbol_stream_batches: u32,
    search_symbol_index_ms: u32,
    search_symbol_index_docs_written: u32,
    search_symbol_index_writer_count: u32,
    search_symbol_index_commit_count: u32,
    search_symbol_index_reload_count: u32,
    search_symbol_index_commit_ms: u32,
    search_symbol_index_reload_ms: u32,
}

struct SearchStateBuildResult {
    publication: Option<IndexPublicationRecord>,
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    engine: SearchEngine,
    search_stats: SearchStateBuildStats,
    semantic_stats: SemanticProjectionStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CacheRefreshStats {
    search_stats: SearchStateBuildStats,
    semantic_stats: SemanticProjectionStats,
    runtime_cache_publish_ms: Option<u32>,
}

fn apply_semantic_projection_stats(
    timings: &mut IndexingPhaseTimings,
    stats: SemanticProjectionStats,
) {
    if !stats.reported {
        return;
    }
    timings.semantic_context_index_ms = Some(stats.semantic_context_index_ms);
    timings.semantic_node_load_ms = Some(stats.node_load_ms);
    timings.semantic_node_load_rows = Some(stats.node_load_rows);
    timings.semantic_node_stream_batches = Some(stats.node_stream_batches);
    timings.semantic_endpoint_load_ms = Some(stats.endpoint_load_ms);
    timings.semantic_endpoint_load_rows = Some(stats.endpoint_load_rows);
    timings.semantic_endpoint_load_batches = Some(stats.endpoint_load_batches);
    timings.semantic_selected_nodes = Some(stats.selected_nodes);
    timings.semantic_context_file_count = Some(stats.context_file_count);
    timings.semantic_context_path_bytes = Some(stats.context_path_bytes);
    timings.semantic_node_lookup_entries = Some(stats.node_lookup_entries);
    timings.semantic_context_ms = Some(stats.context_ms);
    timings.semantic_doc_build_ms = Some(stats.doc_build_ms);
    timings.semantic_embedding_ms = Some(stats.embedding_ms);
    timings.semantic_db_upsert_ms = Some(stats.db_upsert_ms);
    timings.semantic_reload_ms = Some(stats.reload_ms);
    timings.semantic_prune_ms = Some(stats.prune_ms);
    timings.semantic_docs_reused = Some(stats.docs_reused);
    timings.semantic_docs_embedded = Some(stats.docs_embedded);
    timings.semantic_docs_pending = Some(stats.docs_pending);
    timings.semantic_docs_stale = Some(stats.docs_stale);
    timings.symbol_search_docs_written = Some(stats.symbol_search_docs_written);
    timings.semantic_dense_docs_skipped = Some(stats.dense_docs_skipped);
    timings.semantic_dense_public_api = Some(stats.dense_public_api);
    timings.semantic_dense_entrypoint = Some(stats.dense_entrypoint);
    timings.semantic_dense_documented_nontrivial = Some(stats.dense_documented_nontrivial);
    timings.semantic_dense_central_graph_node = Some(stats.dense_central_graph_node);
    timings.semantic_dense_component_report = Some(stats.dense_component_report);
    timings.semantic_dense_unstructured_doc = Some(stats.dense_unstructured_doc);
}

fn apply_cache_refresh_stats(timings: &mut IndexingPhaseTimings, stats: CacheRefreshStats) {
    timings.search_projection_rebuild_ms = Some(stats.search_stats.search_projection_rebuild_ms);
    timings.search_symbol_stream_ms = Some(stats.search_stats.search_symbol_stream_ms);
    timings.search_symbol_stream_rows = Some(stats.search_stats.search_symbol_stream_rows);
    timings.search_symbol_stream_batches = Some(stats.search_stats.search_symbol_stream_batches);
    timings.search_symbol_index_ms = Some(stats.search_stats.search_symbol_index_ms);
    timings.search_symbol_index_docs_written =
        Some(stats.search_stats.search_symbol_index_docs_written);
    timings.search_symbol_index_writer_count =
        Some(stats.search_stats.search_symbol_index_writer_count);
    timings.search_symbol_index_commit_count =
        Some(stats.search_stats.search_symbol_index_commit_count);
    timings.search_symbol_index_reload_count =
        Some(stats.search_stats.search_symbol_index_reload_count);
    timings.search_symbol_index_commit_ms = Some(stats.search_stats.search_symbol_index_commit_ms);
    timings.search_symbol_index_reload_ms = Some(stats.search_stats.search_symbol_index_reload_ms);
    timings.runtime_cache_publish_ms = stats.runtime_cache_publish_ms;
    apply_semantic_projection_stats(timings, stats.semantic_stats);
}

#[cfg(test)]
fn build_search_state(
    search_storage_path: Option<&Path>,
    nodes: Vec<codestory_contracts::graph::Node>,
) -> Result<SearchStateBuildResult, ApiError> {
    build_search_state_for_nodes(search_storage_path, nodes, None)
}

#[cfg(test)]
fn build_search_state_for_nodes(
    search_storage_path: Option<&Path>,
    nodes: Vec<codestory_contracts::graph::Node>,
    cancel_token: Option<&CancellationToken>,
) -> Result<SearchStateBuildResult, ApiError> {
    let search_index_started = Instant::now();
    let mut node_names = HashMap::with_capacity(nodes.len());
    let mut engine = SearchEngine::new(search_storage_path).map_err(|error| {
        if search::engine::is_persisted_search_index_busy(&error) {
            ApiError::new(
                "cache_busy",
                format!("Failed to init search engine: {error}"),
            )
        } else {
            ApiError::internal(format!("Failed to init search engine: {error}"))
        }
    })?;
    let mut symbol_session = engine.begin_symbol_index().map_err(|error| {
        ApiError::internal(format!("Failed to start symbol index writer: {error}"))
    })?;
    let mut search_nodes = Vec::with_capacity(nodes.len().min(SEARCH_NODE_BATCH_SIZE));
    for node in &nodes {
        let display_name = node_display_name(node);
        node_names.insert(node.id, display_name.clone());
        search_nodes.push((node.id, display_name));
        if search_nodes.len() >= SEARCH_NODE_BATCH_SIZE {
            symbol_session
                .add_nodes(std::mem::take(&mut search_nodes))
                .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
            if is_indexing_cancelled(cancel_token) {
                return Err(indexing_cancelled_error());
            }
        }
    }
    if !search_nodes.is_empty() {
        symbol_session
            .add_nodes(search_nodes)
            .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchIndexWrite, cancel_token)?;
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let symbol_write_stats = symbol_session
        .finish()
        .map_err(|e| ApiError::internal(format!("Failed to commit symbol index: {e}")))?;
    if search_storage_path.is_some() && engine.full_text_doc_count() != nodes.len() {
        return Err(ApiError::internal(format!(
            "Persisted search generation validation failed: indexed {} docs for {} nodes",
            engine.full_text_doc_count(),
            nodes.len()
        )));
    }
    let search_symbol_index_ms = clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let search_stats = SearchStateBuildStats {
        search_projection_rebuild_ms: 0,
        search_symbol_stream_ms: 0,
        search_symbol_stream_rows: clamp_usize_to_u32(nodes.len()),
        search_symbol_stream_batches: clamp_usize_to_u32(
            nodes.len().div_ceil(SEARCH_NODE_BATCH_SIZE),
        ),
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
    };
    engine.index_llm_symbol_docs(Vec::new());
    Ok(SearchStateBuildResult {
        publication: None,
        node_names,
        engine,
        search_stats,
        semantic_stats: SemanticProjectionStats::default(),
    })
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
type ActivationSearchRevalidateHook = Box<dyn FnOnce(&Path)>;
#[cfg(test)]
type SemanticProjectionRevalidateHook = Box<dyn FnOnce(&Path)>;
#[cfg(test)]
type FullRefreshStagedStoreHook = Box<dyn FnOnce(&mut Storage)>;
#[cfg(test)]
type IncrementalStagedStoreHook = Box<dyn FnOnce(&mut Storage)>;

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

fn summarize_symbol_doc(
    endpoint: &str,
    model: &str,
    doc: &LlmSymbolDoc,
    config: &codestory_retrieval::SummaryRuntimeConfig,
) -> Result<String, ApiError> {
    if endpoint.eq_ignore_ascii_case("local") || endpoint.eq_ignore_ascii_case("mock") {
        return Ok(local_symbol_summary(doc));
    }

    let mut request = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "Write one concise sentence explaining what this code symbol does. Do not mention that you are summarizing metadata."
            },
            {
                "role": "user",
                "content": doc.doc_text
            }
        ],
        "temperature": 0
    });
    if let Some(object) = request.as_object_mut()
        && let Some(max_tokens) = config.max_tokens
    {
        object.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
    }

    let body = serde_json::to_string(&request)
        .map_err(|e| ApiError::internal(format!("Failed to build summary request: {e}")))?;
    let mut request = ureq::post(endpoint)
        .timeout(config.timeout)
        .set("Content-Type", "application/json");
    if let Some(api_key) = config.api_key.as_deref() {
        request = request.set("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    let response_body = codestory_retrieval::outbound_http::read_text(request.send_string(&body))
        .map_err(summary_endpoint_http_error)?
        .body;
    let response: serde_json::Value = serde_json::from_str(&response_body)
        .map_err(|e| ApiError::internal(format!("Summary endpoint returned invalid JSON: {e}")))?;
    let summary = response
        .pointer("/choices/0/message/content")
        .and_then(|value| value.as_str())
        .or_else(|| {
            response
                .pointer("/choices/0/text")
                .and_then(|value| value.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::internal(
                "Summary endpoint response did not include choices[0].message.content.",
            )
        })?;
    Ok(summary.lines().next().unwrap_or(summary).trim().to_string())
}

fn summary_endpoint_http_error(
    error: codestory_retrieval::outbound_http::OutboundHttpError,
) -> ApiError {
    if let Some(status) = error.status() {
        return ApiError::internal(format!(
            "Summary endpoint failed with status {status}: {}",
            codestory_retrieval::outbound_http::truncate_http_body_to(
                error.body().unwrap_or_default(),
                2_048
            )
        ));
    }
    ApiError::internal(format!("Summary endpoint request failed: {error}"))
}

fn local_symbol_summary(doc: &LlmSymbolDoc) -> String {
    let kind = format!("{:?}", doc.kind).to_ascii_lowercase();
    let location = doc
        .file_path
        .as_deref()
        .map(|path| format!(" in {path}"))
        .unwrap_or_default();
    format!(
        "{} is a {kind}{location} that participates in the indexed code graph.",
        doc.display_name
    )
}

const LLM_SYMBOL_DOC_SCHEMA_VERSION: u32 = 6;
const LLM_SYMBOL_DOC_VERSION_PREFIX: &str = "semantic_doc_version:";
#[cfg(test)]
const SEARCH_NODE_BATCH_SIZE: usize = 8_192;
const SEARCH_SYMBOL_STREAM_BATCH_SIZE: usize = 4_096;
const SEMANTIC_NODE_STREAM_BATCH_SIZE: usize = 4_096;
const SEMANTIC_EDGE_STREAM_BATCH_SIZE: usize = 4_096;
const LLM_DOC_RELOAD_BATCH_SIZE: usize = 512;
#[cfg(test)]
const LLM_DOC_EMBED_BATCH_SIZE: usize = 128;
#[cfg(test)]
const LLM_DOC_EMBED_BATCH_SIZE_ENV: &str = "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE";
#[cfg(test)]
const SEMANTIC_DOC_SCOPE_ENV: &str = "CODESTORY_SEMANTIC_DOC_SCOPE";
#[cfg(test)]
const SEMANTIC_DOC_ALIAS_MODE_ENV: &str = "CODESTORY_SEMANTIC_DOC_ALIAS_MODE";
#[cfg(test)]
const SEMANTIC_DOC_MAX_TOKENS_ENV: &str = "CODESTORY_SEMANTIC_DOC_MAX_TOKENS";
#[cfg(test)]
const SEMANTIC_DOC_DEFAULT_MAX_TOKENS: usize = 128;
#[cfg(test)]
const SEMANTIC_STREAM_PENDING_DOCS_ENV: &str = "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS";
#[cfg(test)]
const SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV: &str =
    "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES";
#[cfg(test)]
const SEMANTIC_STREAM_SORT_WINDOW_BATCHES: usize = 1;
const SEMANTIC_POLICY_VERSION: &str = codestory_retrieval::SEMANTIC_POLICY_VERSION;
const LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION: u32 = 29;
const LEGACY_OVERSIZED_SOURCE_POLICY_VERSION: &str = "oversized-source-v1";
const SYMBOL_SEARCH_DOC_PROVENANCE: &str = "extracted";
const DENSE_CENTRAL_RELATIONSHIP_THRESHOLD: usize = 12;
const DENSE_CENTRAL_SCORE_THRESHOLD: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DenseAnchorReason {
    PublicApi,
    Entrypoint,
    DocumentedNontrivial,
    CentralGraphNode,
    ComponentReport,
    UnstructuredDoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticProjectionSourcePolicyCompatibility {
    Exact,
    LegacyPredecessor,
}

fn semantic_projection_source_policy_compatibility(
    recorded: SourcePolicyExclusionPolicyIdentity<'_>,
    current: &SourceIndexPolicy,
    schema_version: u32,
    legacy_structural_empty: bool,
) -> Option<SemanticProjectionSourcePolicyCompatibility> {
    if recorded.byte_cap != current.byte_cap
        || recorded.structural_unit_cap != current.structural_unit_cap
    {
        return None;
    }
    if recorded.policy_version == current.policy_version {
        return Some(SemanticProjectionSourcePolicyCompatibility::Exact);
    }
    (recorded.policy_version == LEGACY_OVERSIZED_SOURCE_POLICY_VERSION
        && current.policy_version
            == codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION
        && schema_version == LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION
        && legacy_structural_empty)
        .then_some(SemanticProjectionSourcePolicyCompatibility::LegacyPredecessor)
}

impl DenseAnchorReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::PublicApi => "public_api",
            Self::Entrypoint => "entrypoint",
            Self::DocumentedNontrivial => "documented_nontrivial",
            Self::CentralGraphNode => "central_graph_node",
            Self::ComponentReport => "component_report",
            Self::UnstructuredDoc => "unstructured_doc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticDocScope {
    DurableSymbols,
    AllSymbols,
}

impl SemanticDocScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::DurableSymbols => "durable_symbols",
            Self::AllSymbols => "all_symbols",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticDocAliasMode {
    NoAlias,
    CurrentAlias,
    AliasVariant,
}

impl SemanticDocAliasMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoAlias => "no_alias",
            Self::CurrentAlias => "current_alias",
            Self::AliasVariant => "alias_variant",
        }
    }
}

#[cfg(test)]
fn semantic_doc_shape_contract() -> String {
    let max_tokens = semantic_doc_max_tokens_from_env();
    format!(
        "semantic_doc_version={};scope={};alias_mode={};max_tokens={}",
        LLM_SYMBOL_DOC_SCHEMA_VERSION,
        semantic_doc_scope_from_env().as_str(),
        semantic_doc_alias_mode_from_env().as_str(),
        max_tokens
    )
}

fn semantic_doc_shape_contract_for_runtime(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> String {
    format!(
        "semantic_doc_version={};scope={};alias_mode={};max_tokens={}",
        LLM_SYMBOL_DOC_SCHEMA_VERSION,
        semantic_doc_scope_from_value(&runtime.retrieval.semantic_doc_scope).as_str(),
        semantic_doc_alias_mode_from_value(&runtime.retrieval.semantic_doc_alias_mode).as_str(),
        runtime.retrieval.semantic_doc_max_tokens,
    )
}

#[cfg(test)]
fn current_embedding_contract_from_env() -> Option<EmbeddingProfileContractDto> {
    let doc_shape = semantic_doc_shape_contract();
    embedding_profile_contract_from_env()
        .ok()
        .map(|contract| EmbeddingProfileContractDto {
            profile: contract.profile,
            backend: contract.backend,
            model_id: contract.model_id,
            cache_key: contract.cache_key,
            dimension: contract.dimension,
            doc_shape,
        })
}

fn current_embedding_contract_for_runtime(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Option<EmbeddingProfileContractDto> {
    let doc_shape = semantic_doc_shape_contract_for_runtime(runtime);
    crate::search_runtime::embedding_profile_contract_from_config(&runtime.embedding)
        .ok()
        .map(|contract| EmbeddingProfileContractDto {
            profile: contract.profile,
            backend: contract.backend,
            model_id: contract.model_id,
            cache_key: contract.cache_key,
            dimension: contract.dimension,
            doc_shape,
        })
}

fn search_index_storage_path(storage_path: &Path) -> PathBuf {
    codestory_workspace::legacy_search_directory_for_storage(storage_path)
}

fn search_index_generation_root(storage_path: &Path) -> PathBuf {
    codestory_workspace::search_generation_directory_for_storage(storage_path)
}

fn runtime_workspace_manifest(
    root: &Path,
    storage_path: &Path,
) -> anyhow::Result<WorkspaceManifest> {
    WorkspaceManifest::open_with_storage_owned_exclusions(root.to_path_buf(), storage_path)
}

fn search_index_path_for_publication(
    storage_path: &Path,
    publication: Option<&IndexPublicationRecord>,
) -> Result<PathBuf, ApiError> {
    match publication {
        Some(publication) => Uuid::parse_str(&publication.generation_id)
            .map(|generation_id| {
                search_index_generation_root(storage_path).join(generation_id.to_string())
            })
            .map_err(|error| {
                ApiError::internal(format!(
                    "Invalid index publication generation id {}: {error}",
                    publication.generation_id
                ))
            }),
        None => Ok(search_index_storage_path(storage_path)),
    }
}

const SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION: u32 = 1;
const SEARCH_GENERATION_COMPLETION_FILE: &str = ".codestory-complete.json";
const SEARCH_GENERATION_COMPLETION_MAX_BYTES: u64 = 4 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct SearchGenerationCompletion {
    schema_version: u32,
    generation_id: String,
    symbol_count: u64,
    tantivy_doc_count: u64,
}

fn search_generation_completion_path(search_path: &Path) -> PathBuf {
    search_path.join(SEARCH_GENERATION_COMPLETION_FILE)
}

fn read_search_generation_completion(
    search_path: &Path,
    expected_generation_id: &str,
) -> Option<SearchGenerationCompletion> {
    let marker_path = search_generation_completion_path(search_path);
    let metadata = std::fs::metadata(&marker_path).ok()?;
    if !metadata.is_file() || metadata.len() > SEARCH_GENERATION_COMPLETION_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(&marker_path).ok()?;
    let marker = serde_json::from_slice::<SearchGenerationCompletion>(&bytes).ok()?;
    (marker.schema_version == SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION
        && marker.generation_id == expected_generation_id)
        .then_some(marker)
}

fn write_search_generation_completion(
    search_path: &Path,
    publication: &IndexPublicationRecord,
    symbol_count: usize,
    tantivy_doc_count: usize,
) -> Result<(), ApiError> {
    let generation_id = Uuid::parse_str(&publication.generation_id)
        .map_err(|error| {
            ApiError::internal(format!(
                "Invalid index publication generation id {}: {error}",
                publication.generation_id
            ))
        })?
        .to_string();
    let marker = SearchGenerationCompletion {
        schema_version: SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION,
        generation_id,
        symbol_count: symbol_count as u64,
        tantivy_doc_count: tantivy_doc_count as u64,
    };
    let bytes = serde_json::to_vec(&marker).map_err(|error| {
        ApiError::internal(format!(
            "Failed to encode persisted search generation completion marker: {error}"
        ))
    })?;
    let marker_path = search_generation_completion_path(search_path);
    let temp_path = search_path.join(format!(".codestory-complete.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<(), ApiError> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create persisted search completion temp file {}: {error}",
                    temp_path.display()
                ))
            })?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|error| {
            ApiError::internal(format!(
                "Failed to write persisted search completion temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            ApiError::internal(format!(
                "Failed to sync persisted search completion temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        std::fs::rename(&temp_path, &marker_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish persisted search completion marker {}: {error}",
                marker_path.display()
            ))
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

struct SearchGenerationCatalogGuard {
    file: std::fs::File,
    path: PathBuf,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationTestBoundary {
    SemanticContextIndexes,
    SemanticNodePage,
    SemanticStoredDocumentPage,
    SemanticEndpointRead,
    ProjectionSnapshotFinalize,
    ProjectionSnapshotDetail,
    ProjectionManifestIdentity,
    Identity,
    SearchBuild,
    SearchSymbolPage,
    SearchIndexWrite,
    SearchValidation,
    SearchCompletion,
    CatalogLock,
    DatabaseReplacement,
    MarkerCompletion,
    RuntimeCache,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationTestAction {
    Fail,
    Cancel,
}

#[cfg(test)]
thread_local! {
    static PUBLICATION_TEST_FAULT: std::cell::RefCell<Option<(PublicationTestBoundary, PublicationTestAction)>> =
        const { std::cell::RefCell::new(None) };
    static ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<ActivationSearchRevalidateHook>> =
        const { std::cell::RefCell::new(None) };
    static SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<SemanticProjectionRevalidateHook>> =
        const { std::cell::RefCell::new(None) };
    static SOURCE_POLICY_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static SOURCE_POLICY_AFTER_PLAN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FULL_REFRESH_STAGED_STORE_HOOK: std::cell::RefCell<Option<FullRefreshStagedStoreHook>> =
        const { std::cell::RefCell::new(None) };
    static INCREMENTAL_STAGED_STORE_HOOK: std::cell::RefCell<Option<IncrementalStagedStoreHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn arm_publication_test_fault(boundary: PublicationTestBoundary, action: PublicationTestAction) {
    PUBLICATION_TEST_FAULT.with(|fault| *fault.borrow_mut() = Some((boundary, action)));
}

#[cfg(test)]
fn arm_activation_search_before_revalidate_hook(hook: impl FnOnce(&Path) + 'static) {
    ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_activation_search_before_revalidate_hook(storage_path: &Path) {
    ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage_path);
        }
    });
}

#[cfg(test)]
fn arm_semantic_projection_before_revalidate_hook(hook: impl FnOnce(&Path) + 'static) {
    SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_semantic_projection_before_revalidate_hook(storage_path: &Path) {
    SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK.with(|slot| {
        let hook = slot.borrow_mut().take();
        if let Some(hook) = hook {
            hook(storage_path);
        }
    });
}

#[cfg(test)]
fn arm_source_policy_before_revalidate_hook(hook: impl FnOnce() + 'static) {
    SOURCE_POLICY_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_source_policy_before_revalidate_hook() {
    SOURCE_POLICY_BEFORE_REVALIDATE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn arm_source_policy_after_plan_hook(hook: impl FnOnce() + 'static) {
    SOURCE_POLICY_AFTER_PLAN_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_source_policy_after_plan_hook() {
    SOURCE_POLICY_AFTER_PLAN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn arm_full_refresh_staged_store_hook(hook: impl FnOnce(&mut Storage) + 'static) {
    FULL_REFRESH_STAGED_STORE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_full_refresh_staged_store_hook(storage: &mut Storage) {
    FULL_REFRESH_STAGED_STORE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage);
        }
    });
}

#[cfg(test)]
fn arm_incremental_staged_store_hook(hook: impl FnOnce(&mut Storage) + 'static) {
    INCREMENTAL_STAGED_STORE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_incremental_staged_store_hook(storage: &mut Storage) {
    INCREMENTAL_STAGED_STORE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage);
        }
    });
}

#[cfg(test)]
fn publication_test_checkpoint(
    boundary: PublicationTestBoundary,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), ApiError> {
    let action = PUBLICATION_TEST_FAULT.with(|fault| {
        let armed = *fault.borrow();
        matches!(armed, Some((armed_boundary, _)) if armed_boundary == boundary).then(|| {
            fault
                .borrow_mut()
                .take()
                .expect("armed publication fault")
                .1
        })
    });
    match action {
        Some(PublicationTestAction::Fail) => Err(ApiError::internal(format!(
            "Injected publication failure at {boundary:?}"
        ))),
        Some(PublicationTestAction::Cancel) => {
            if let Some(token) = cancel_token {
                token.cancel();
            }
            Ok(())
        }
        None => Ok(()),
    }
}

impl SearchGenerationCatalogGuard {
    fn acquire(storage_path: &Path) -> Result<Self, ApiError> {
        let mut path = search_index_generation_root(storage_path).into_os_string();
        path.push(".lock");
        let path = PathBuf::from(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create search generation catalog lock directory {}: {error}",
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
                    "Failed to open search generation catalog lock {}: {error}",
                    path.display()
                ))
            })?;
        FileExt::lock_exclusive(&file).map_err(|error| {
            ApiError::internal(format!(
                "Failed to acquire search generation catalog lock {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self { file, path })
    }
}

impl Drop for SearchGenerationCatalogGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock search generation catalog: {error}"
            );
        }
    }
}

fn inspect_search_generation(path: &Path) -> Result<Option<bool>, ApiError> {
    let lock_path = crate::search::engine::persisted_search_index_lock_path(path);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to open persisted search generation lock {}: {error}",
                lock_path.display()
            ))
        })?;
    if !FileExt::try_lock_shared(&lock).map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect persisted search generation lock {}: {error}",
            lock_path.display()
        ))
    })? {
        return Ok(None);
    }
    let generation_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let marker = read_search_generation_completion(path, generation_id);
    let valid = marker.is_some_and(|marker| {
        SearchEngine::open_existing(path)
            .is_ok_and(|engine| engine.tantivy_doc_count() as u64 == marker.tantivy_doc_count)
    });
    let _ = FileExt::unlock(&lock);
    Ok(Some(valid))
}

fn try_remove_search_generation(
    deletion: &OwnedDeletionRoot,
    relative: &Path,
    path: &Path,
) -> Result<bool, ApiError> {
    let lock_path = crate::search::engine::persisted_search_index_lock_path(path);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to open persisted search generation lock {}: {error}",
                lock_path.display()
            ))
        })?;
    if !FileExt::try_lock_exclusive(&lock).map_err(|error| {
        ApiError::internal(format!(
            "Failed to lock persisted search generation {} for removal: {error}",
            path.display()
        ))
    })? {
        return Ok(false);
    }
    let removal = deletion.remove(relative);
    let _ = FileExt::unlock(&lock);
    let removed = removal.map_err(|error| {
        ApiError::internal(format!(
            "Failed to remove persisted search generation {}: {error}",
            path.display()
        ))
    })?;
    Ok(removed)
}

fn prune_search_generations(
    storage_path: &Path,
    active_generation_id: &str,
) -> Result<(), ApiError> {
    let root = search_index_generation_root(storage_path);
    if !root.is_dir() {
        return Ok(());
    }
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    let deletion = OwnedDeletionRoot::open(parent).map_err(|error| {
        ApiError::internal(format!(
            "Failed to open persisted search generation deletion root {}: {error}",
            parent.display()
        ))
    })?;
    let relative_root = root.file_name().ok_or_else(|| {
        ApiError::internal(format!(
            "Persisted search generation root has no owned relative name: {}",
            root.display()
        ))
    })?;
    let mut generations = std::fs::read_dir(&root)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to list persisted search generations {}: {error}",
                root.display()
            ))
        })?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".lock"))
        .collect::<Vec<_>>();
    generations.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });

    // During staged publication the prepared search identity is newer than
    // the still-live core. Keep the search generation bound to that live core
    // as the rollback; a concurrent prepared generation must not consume the
    // sole rollback slot merely because its completion marker was written.
    let pinned_rollback_generation_id = Store::database_complete_index_publication(storage_path)
        .ok()
        .flatten()
        .map(|publication| publication.generation_id)
        .filter(|generation_id| generation_id != active_generation_id);

    let mut rollback_retained = false;
    for entry in generations {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == active_generation_id {
            continue;
        }
        let well_formed = Uuid::parse_str(&name).is_ok();
        let inspection = if well_formed {
            inspect_search_generation(&path)?
        } else {
            Some(false)
        };
        match inspection {
            Some(true)
                if !rollback_retained
                    && pinned_rollback_generation_id
                        .as_ref()
                        .is_none_or(|generation_id| generation_id == &name) =>
            {
                rollback_retained = true
            }
            Some(_) => {
                let relative = Path::new(relative_root).join(&name);
                let _ = try_remove_search_generation(&deletion, &relative, &path)?;
            }
            None => {
                tracing::debug!(
                    path = %path.display(),
                    "Skipping locked persisted search generation during retention"
                );
            }
        }
    }
    Ok(())
}

fn discard_unpublished_search_generation(
    storage_path: &Path,
    publication: &IndexPublicationRecord,
) {
    if matches!(
        Store::database_index_publication(storage_path),
        Ok(Some(live)) if live == *publication
    ) {
        return;
    }
    if let Ok(path) = search_index_path_for_publication(storage_path, Some(publication)) {
        let root = search_index_generation_root(storage_path);
        let parent = root.parent().unwrap_or_else(|| Path::new("."));
        if let (Ok(deletion), Some(relative_root), Some(generation_name)) = (
            OwnedDeletionRoot::open(parent),
            root.file_name(),
            path.file_name(),
        ) {
            let relative = Path::new(relative_root).join(generation_name);
            let _ = try_remove_search_generation(&deletion, &relative, &path);
        }
    }
}

fn load_canonical_search_symbols(
    storage: &Storage,
    batch_size: usize,
    cancel_token: Option<&CancellationToken>,
    mut consume_batch: impl FnMut(Vec<SearchSymbolProjection>) -> Result<(), ApiError>,
) -> Result<
    (
        HashMap<codestory_contracts::graph::NodeId, String>,
        SearchStateBuildStats,
    ),
    ApiError,
> {
    let count_started = Instant::now();
    let expected_rows = storage
        .get_canonical_search_symbol_count()
        .map_err(|error| {
            ApiError::internal(format!("Failed to count canonical search symbols: {error}"))
        })?;
    let mut node_names = HashMap::with_capacity(expected_rows as usize);
    let mut after_node_id = None;
    let batch_size = batch_size.max(1);
    let mut stream_duration = count_started.elapsed();
    let mut stream_rows = 0_usize;
    let mut stream_batches = 0_usize;
    loop {
        let batch_started = Instant::now();
        let batch = storage
            .get_canonical_search_symbol_batch_after(after_node_id, batch_size)
            .map_err(|e| {
                ApiError::internal(format!("Failed to stream canonical search symbols: {e}"))
            })?;
        stream_duration = stream_duration.saturating_add(batch_started.elapsed());
        if batch.is_empty() {
            break;
        }
        after_node_id = batch.last().map(|entry| entry.node_id);
        stream_rows = stream_rows.saturating_add(batch.len());
        stream_batches = stream_batches.saturating_add(1);
        for entry in &batch {
            node_names.insert(entry.node_id, entry.display_name.clone());
        }
        consume_batch(batch)?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
    }
    if stream_rows != expected_rows as usize {
        return Err(ApiError::internal(format!(
            "Canonical search symbol stream count changed: expected {expected_rows}, loaded {stream_rows}"
        )));
    }
    Ok((
        node_names,
        SearchStateBuildStats {
            search_projection_rebuild_ms: 0,
            search_symbol_stream_ms: clamp_u128_to_u32(stream_duration.as_millis()),
            search_symbol_stream_rows: clamp_usize_to_u32(stream_rows),
            search_symbol_stream_batches: clamp_usize_to_u32(stream_batches),
            ..SearchStateBuildStats::default()
        },
    ))
}

struct LoadedSearchState {
    publication: Option<IndexPublicationRecord>,
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    engine: SearchEngine,
}

#[cfg(test)]
fn load_persisted_search_state(
    storage: &mut Storage,
    storage_path: &Path,
) -> Result<LoadedSearchState, ApiError> {
    load_persisted_search_state_for_runtime(storage, storage_path, &test_sidecar_runtime_from_env())
}

fn load_persisted_search_state_for_runtime(
    storage: &mut Storage,
    storage_path: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<LoadedSearchState, ApiError> {
    let _catalog_guard = SearchGenerationCatalogGuard::acquire(storage_path)?;
    *storage = open_storage_for_read(storage_path)?;
    let publication = storage.get_complete_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to read complete search publication identity: {error}"
        ))
    })?;
    if publication.is_none() {
        let mut engine = SearchEngine::new(None).map_err(|error| {
            ApiError::internal(format!("Failed to init search engine: {error}"))
        })?;
        let mut symbol_session = engine.begin_symbol_index().map_err(|error| {
            ApiError::internal(format!(
                "Failed to start legacy symbol index writer: {error}"
            ))
        })?;
        let (node_names, _) = load_canonical_search_symbols(storage, 10_000, None, |batch| {
            symbol_session
                .add_nodes(
                    batch
                        .into_iter()
                        .map(|entry| (entry.node_id, entry.display_name)),
                )
                .map(|_| ())
                .map_err(|error| {
                    ApiError::internal(format!("Failed to index legacy search nodes: {error}"))
                })
        })?;
        symbol_session.finish().map_err(|error| {
            ApiError::internal(format!("Failed to finish legacy symbol index: {error}"))
        })?;
        load_persisted_semantic_docs_for_runtime(storage, &mut engine, false, runtime)?;
        return Ok(LoadedSearchState {
            publication: None,
            node_names,
            engine,
        });
    }
    let search_storage_path =
        search_index_path_for_publication(storage_path, publication.as_ref())?;
    let completion = publication.as_ref().and_then(|publication| {
        let generation_id = Uuid::parse_str(&publication.generation_id)
            .ok()?
            .to_string();
        read_search_generation_completion(&search_storage_path, &generation_id)
    });
    if publication.is_some() && completion.is_none() {
        return Err(ApiError::new(
            "cache_busy",
            "The complete core publication does not yet have a completed search generation.",
        ));
    }
    let mut engine =
        SearchEngine::open_existing(search_storage_path.as_path()).map_err(|error| {
            ApiError::new(
                "cache_busy",
                format!(
                    "Failed to open completed search generation {}: {error}",
                    search_storage_path.display()
                ),
            )
        })?;
    engine.load_symbol_projection(std::iter::empty());
    let (node_names, stream_stats) =
        load_canonical_search_symbols(storage, SEARCH_SYMBOL_STREAM_BATCH_SIZE, None, |batch| {
            engine.extend_symbol_projection(
                batch
                    .into_iter()
                    .map(|entry| (entry.node_id, entry.display_name)),
            );
            Ok(())
        })?;
    let completion_count_mismatch = completion.as_ref().is_some_and(|marker| {
        marker.symbol_count != stream_stats.search_symbol_stream_rows as u64
            || marker.tantivy_doc_count != engine.tantivy_doc_count() as u64
    });
    if engine.full_text_doc_count() != stream_stats.search_symbol_stream_rows as usize
        || completion_count_mismatch
    {
        return Err(ApiError::new(
            "cache_busy",
            format!(
                "Completed search generation {} does not match its core symbols: streamed={}, searchable={}, marker_symbols={}, stored_docs={}, marker_docs={}.",
                search_storage_path.display(),
                stream_stats.search_symbol_stream_rows,
                engine.full_text_doc_count(),
                completion.as_ref().map_or(0, |marker| marker.symbol_count),
                engine.tantivy_doc_count(),
                completion
                    .as_ref()
                    .map_or(0, |marker| marker.tantivy_doc_count),
            ),
        ));
    }
    if publication.is_some() {
        engine
            .downgrade_persisted_lock_to_shared()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to share completed search generation {}: {error}",
                    search_storage_path.display()
                ))
            })?;
    }
    let live_publication =
        Store::database_complete_index_publication(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to revalidate live publication after loading persisted search: {error}"
            ))
        })?;
    if live_publication != publication {
        return Err(ApiError::new(
            "cache_busy",
            "Core publication changed while persisted search state was loading. Retry against the new generation.",
        ));
    }
    Ok(LoadedSearchState {
        publication,
        node_names,
        engine,
    })
}

fn reload_llm_docs_from_storage(
    storage: &Storage,
    engine: &mut SearchEngine,
    batch_size: usize,
) -> Result<(), ApiError> {
    engine.clear_llm_symbol_docs();
    let mut after_node_id = None;
    let batch_size = batch_size.max(1);
    loop {
        let docs = storage
            .get_llm_symbol_docs_batch_after(after_node_id, batch_size)
            .map_err(|e| ApiError::internal(format!("Failed to load LLM symbol docs: {e}")))?;
        if docs.is_empty() {
            break;
        }
        after_node_id = docs.last().map(|doc| doc.node_id);
        engine.extend_llm_symbol_docs(docs.into_iter().map(map_llm_doc_to_search));
    }
    Ok(())
}

#[cfg(test)]
fn llm_doc_embed_batch_size() -> usize {
    std::env::var(LLM_DOC_EMBED_BATCH_SIZE_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 2_048))
        .unwrap_or(LLM_DOC_EMBED_BATCH_SIZE)
}

#[cfg(test)]
fn retrieval_state_from_parts(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
    current_embedding: Option<EmbeddingProfileContractDto>,
    stored_embedding: Option<StoredSemanticDocsContractDto>,
    runtime_degraded: bool,
) -> RetrievalStateDto {
    retrieval_state_from_parts_with_hybrid(
        semantic_doc_count,
        embedding_model,
        embedding_runtime_available,
        fallback_message,
        current_embedding,
        stored_embedding,
        runtime_degraded,
        hybrid_retrieval_enabled(),
    )
}

#[allow(clippy::too_many_arguments)]
fn retrieval_state_from_parts_with_hybrid(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
    current_embedding: Option<EmbeddingProfileContractDto>,
    stored_embedding: Option<StoredSemanticDocsContractDto>,
    runtime_degraded: bool,
    hybrid_configured: bool,
) -> RetrievalStateDto {
    let fallback_reason = if !hybrid_configured {
        Some(RetrievalFallbackReasonDto::DisabledByConfig)
    } else if runtime_degraded {
        Some(RetrievalFallbackReasonDto::DegradedRuntime)
    } else if !embedding_runtime_available {
        Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    } else if semantic_doc_count == 0 {
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs)
    } else {
        None
    };
    let semantic_mode = if !hybrid_configured {
        SemanticModeDto::DisabledByConfig
    } else if runtime_degraded || !embedding_runtime_available || semantic_doc_count == 0 {
        SemanticModeDto::DegradedRuntime
    } else {
        SemanticModeDto::Enabled
    };
    let semantic_ready = semantic_mode == SemanticModeDto::Enabled;
    let mode = if semantic_ready {
        RetrievalModeDto::Hybrid
    } else {
        RetrievalModeDto::Symbolic
    };
    let fallback_message = fallback_message.or_else(|| match fallback_reason {
        Some(RetrievalFallbackReasonDto::DisabledByConfig) => Some(format!(
            "Hybrid retrieval disabled by {HYBRID_RETRIEVAL_ENABLED_ENV}=false; agent-facing retrieval is not full."
        )),
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs) => Some(
            "Semantic assets are available, but semantic symbol docs have not been built yet. Run `retrieval index --refresh full` to repair full sidecar readiness."
                .to_string(),
        ),
        Some(RetrievalFallbackReasonDto::DegradedRuntime) => Some(
            "Hybrid retrieval is configured but degraded at runtime; agent-facing retrieval is not full."
                .to_string(),
        ),
        _ => None,
    });

    RetrievalStateDto {
        mode,
        hybrid_configured,
        semantic_ready,
        semantic_mode,
        semantic_doc_count,
        embedding_model,
        current_embedding,
        stored_embedding,
        fallback_reason,
        fallback_message,
    }
}

#[cfg(test)]
fn retrieval_state_from_engine(engine: &SearchEngine) -> RetrievalStateDto {
    let probe = embedding_runtime_availability_from_env();
    let current_embedding = current_embedding_contract_from_env();
    retrieval_state_from_parts(
        engine.semantic_doc_count(),
        engine
            .embedding_model_id()
            .map(str::to_string)
            .or_else(|| {
                current_embedding
                    .as_ref()
                    .map(|contract| contract.cache_key.clone())
            })
            .or(probe.model_id),
        engine.embedding_runtime_configured(),
        if engine.embedding_runtime_configured() {
            None
        } else {
            probe.fallback_message
        },
        current_embedding,
        None,
        false,
    )
}

#[cfg(test)]
fn retrieval_state_from_engine_with_storage_contract(
    engine: &SearchEngine,
    storage_retrieval: &RetrievalStateDto,
) -> RetrievalStateDto {
    let mut retrieval = retrieval_state_from_engine(engine);
    retrieval.stored_embedding = storage_retrieval.stored_embedding.clone();
    retrieval
}

#[cfg(test)]
fn retrieval_state_from_storage(storage: &Storage) -> Result<RetrievalStateDto, ApiError> {
    retrieval_state_from_storage_for_runtime(storage, &test_sidecar_runtime_from_env())
}

fn retrieval_state_from_storage_for_runtime(
    storage: &Storage,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<RetrievalStateDto, ApiError> {
    let stats = storage
        .get_llm_symbol_doc_stats()
        .map_err(|e| ApiError::internal(format!("Failed to query LLM symbol doc stats: {e}")))?;
    let probe = embedding_runtime_availability_from_config(runtime);
    let current_embedding = current_embedding_contract_for_runtime(runtime);
    let stored_embedding = stored_semantic_docs_contract_from_stats(&stats);
    let contract_mismatch = stats.doc_count > 0
        && probe.available
        && !current_embedding
            .as_ref()
            .is_some_and(|contract| semantic_doc_stats_match_contract(&stats, contract));
    let fallback_message = probe.fallback_message.or_else(|| {
        contract_mismatch.then(|| {
            "Stored semantic docs do not match the current embedding contract. Run `retrieval index --refresh full` before trusting hybrid retrieval."
                .to_string()
        })
    });
    Ok(retrieval_state_from_parts_with_hybrid(
        stats.doc_count,
        stats
            .embedding_model
            .clone()
            .or_else(|| {
                current_embedding
                    .as_ref()
                    .map(|contract| contract.cache_key.clone())
            })
            .or(probe.model_id),
        probe.available,
        fallback_message,
        current_embedding,
        Some(stored_embedding),
        contract_mismatch,
        runtime.retrieval.hybrid_enabled,
    ))
}

fn semantic_doc_stats_match_contract(
    stats: &LlmSymbolDocStats,
    contract: &EmbeddingProfileContractDto,
) -> bool {
    !stats.mixed_embedding_profiles
        && !stats.mixed_embedding_models
        && !stats.mixed_embedding_backends
        && !stats.mixed_dimensions
        && !stats.mixed_doc_versions
        && !stats.mixed_doc_shapes
        && !stats.mixed_semantic_policy_versions
        && stats.embedding_profile.as_deref() == Some(contract.profile.as_str())
        && stats.embedding_model.as_deref() == Some(contract.cache_key.as_str())
        && stats.embedding_backend.as_deref() == Some(contract.backend.as_str())
        && stats.embedding_dim.is_some_and(|dimension| {
            dimension > 0
                && contract
                    .dimension
                    .is_none_or(|expected| expected == dimension)
        })
        && stats.doc_version == Some(LLM_SYMBOL_DOC_SCHEMA_VERSION)
        && stats.doc_shape.as_deref() == Some(contract.doc_shape.as_str())
        && stats.semantic_policy_version.as_deref() == Some(SEMANTIC_POLICY_VERSION)
}

fn stored_semantic_docs_contract_from_stats(
    stats: &LlmSymbolDocStats,
) -> StoredSemanticDocsContractDto {
    StoredSemanticDocsContractDto {
        doc_count: stats.doc_count,
        embedding_profile: stats.embedding_profile.clone(),
        embedding_backend: stats.embedding_backend.clone(),
        cache_key: stats.embedding_model.clone(),
        dimension: stats.embedding_dim,
        doc_version: stats.doc_version,
        mixed_embedding_profiles: stats.mixed_embedding_profiles,
        mixed_embedding_models: stats.mixed_embedding_models,
        mixed_embedding_backends: stats.mixed_embedding_backends,
        mixed_dimensions: stats.mixed_dimensions,
        mixed_doc_versions: stats.mixed_doc_versions,
        mixed_doc_shapes: stats.mixed_doc_shapes,
        doc_shape: stats.doc_shape.clone(),
        semantic_policy_version: stats.semantic_policy_version.clone(),
        mixed_semantic_policy_versions: stats.mixed_semantic_policy_versions,
    }
}

#[cfg(test)]
fn semantic_doc_scope_from_env() -> SemanticDocScope {
    semantic_doc_scope_from_value(&std::env::var(SEMANTIC_DOC_SCOPE_ENV).unwrap_or_default())
}

fn semantic_doc_scope_from_value(value: &str) -> SemanticDocScope {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" | "full" | "all-symbols" | "all_symbols" => SemanticDocScope::AllSymbols,
        _ => SemanticDocScope::DurableSymbols,
    }
}

#[cfg(test)]
fn semantic_doc_alias_mode_from_env() -> SemanticDocAliasMode {
    semantic_doc_alias_mode_from_value(
        &std::env::var(SEMANTIC_DOC_ALIAS_MODE_ENV).unwrap_or_default(),
    )
}

fn semantic_doc_alias_mode_from_value(value: &str) -> SemanticDocAliasMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "default" | "auto" => SemanticDocAliasMode::AliasVariant,
        "none" | "no_alias" | "no-alias" | "off" | "false" | "0" => SemanticDocAliasMode::NoAlias,
        "current_alias" | "current-alias" | "full" | "full_alias" | "full-alias" | "on"
        | "true" | "1" => SemanticDocAliasMode::CurrentAlias,
        "variant" | "alias_variant" | "alias-variant" | "compact" | "compact_alias"
        | "compact-alias" => SemanticDocAliasMode::AliasVariant,
        _ => SemanticDocAliasMode::AliasVariant,
    }
}

#[cfg(test)]
fn semantic_doc_max_tokens_from_env() -> usize {
    std::env::var(SEMANTIC_DOC_MAX_TOKENS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.clamp(16, 8_192))
        .unwrap_or(SEMANTIC_DOC_DEFAULT_MAX_TOKENS)
}

#[cfg(test)]
fn stream_pending_llm_symbol_docs_from_env() -> bool {
    !matches!(
        std::env::var(SEMANTIC_STREAM_PENDING_DOCS_ENV)
            .unwrap_or_else(|_| "true".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
fn semantic_stream_sort_window_batches_from_env() -> usize {
    std::env::var(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 16))
        .unwrap_or(SEMANTIC_STREAM_SORT_WINDOW_BATCHES)
}

fn llm_indexable_kind_for_scope(
    kind: codestory_contracts::graph::NodeKind,
    scope: SemanticDocScope,
) -> bool {
    match scope {
        SemanticDocScope::AllSymbols => !matches!(
            kind,
            codestory_contracts::graph::NodeKind::FILE
                | codestory_contracts::graph::NodeKind::UNKNOWN
                | codestory_contracts::graph::NodeKind::BUILTIN_TYPE
        ),
        SemanticDocScope::DurableSymbols => matches!(
            kind,
            codestory_contracts::graph::NodeKind::STRUCT
                | codestory_contracts::graph::NodeKind::CLASS
                | codestory_contracts::graph::NodeKind::INTERFACE
                | codestory_contracts::graph::NodeKind::ANNOTATION
                | codestory_contracts::graph::NodeKind::UNION
                | codestory_contracts::graph::NodeKind::ENUM
                | codestory_contracts::graph::NodeKind::TYPEDEF
                | codestory_contracts::graph::NodeKind::FUNCTION
                | codestory_contracts::graph::NodeKind::METHOD
                | codestory_contracts::graph::NodeKind::MACRO
                | codestory_contracts::graph::NodeKind::GLOBAL_VARIABLE
                | codestory_contracts::graph::NodeKind::CONSTANT
                | codestory_contracts::graph::NodeKind::ENUM_CONSTANT
        ),
    }
}

fn llm_indexable_kinds_for_scope(
    scope: SemanticDocScope,
) -> &'static [codestory_contracts::graph::NodeKind] {
    use codestory_contracts::graph::NodeKind;

    const DURABLE_SYMBOLS: &[NodeKind] = &[
        NodeKind::STRUCT,
        NodeKind::CLASS,
        NodeKind::INTERFACE,
        NodeKind::ANNOTATION,
        NodeKind::UNION,
        NodeKind::ENUM,
        NodeKind::TYPEDEF,
        NodeKind::FUNCTION,
        NodeKind::METHOD,
        NodeKind::MACRO,
        NodeKind::GLOBAL_VARIABLE,
        NodeKind::CONSTANT,
        NodeKind::ENUM_CONSTANT,
    ];
    const ALL_SYMBOLS: &[NodeKind] = &[
        NodeKind::MODULE,
        NodeKind::NAMESPACE,
        NodeKind::PACKAGE,
        NodeKind::STRUCT,
        NodeKind::CLASS,
        NodeKind::INTERFACE,
        NodeKind::ANNOTATION,
        NodeKind::UNION,
        NodeKind::ENUM,
        NodeKind::TYPEDEF,
        NodeKind::TYPE_PARAMETER,
        NodeKind::FUNCTION,
        NodeKind::METHOD,
        NodeKind::MACRO,
        NodeKind::GLOBAL_VARIABLE,
        NodeKind::FIELD,
        NodeKind::VARIABLE,
        NodeKind::CONSTANT,
        NodeKind::ENUM_CONSTANT,
    ];

    match scope {
        SemanticDocScope::DurableSymbols => DURABLE_SYMBOLS,
        SemanticDocScope::AllSymbols => ALL_SYMBOLS,
    }
}

#[cfg(test)]
fn llm_indexable_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    llm_indexable_kind_for_scope(kind, semantic_doc_scope_from_env())
}

fn normalize_semantic_store_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        return rest.to_string();
    }
    path
}

fn semantic_path_is_absolute_like(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() > 2
            && bytes[1] == b':'
            && bytes[2] == b'/'
            && bytes[0].is_ascii_alphabetic())
}

fn semantic_path_parent(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
}

fn common_semantic_path_prefix(left: &str, right: &str) -> String {
    let left_parts = left.split('/').collect::<Vec<_>>();
    let right_parts = right.split('/').collect::<Vec<_>>();
    let mut common = Vec::new();
    for (left, right) in left_parts.iter().zip(right_parts.iter()) {
        if left != right {
            break;
        }
        common.push(*left);
    }
    common.join("/")
}

fn common_absolute_semantic_parent(paths: &[(GraphNodeId, String)]) -> Option<String> {
    let mut parents = paths
        .iter()
        .map(|(_, path)| path.as_str())
        .filter(|path| semantic_path_is_absolute_like(path))
        .filter_map(semantic_path_parent);
    let mut common = parents.next()?.to_string();
    for parent in parents {
        common = common_semantic_path_prefix(&common, parent);
        if common.is_empty() {
            return None;
        }
    }
    Some(common).filter(|common| !common.is_empty())
}

fn strip_semantic_common_parent(path: &str, common_parent: &str) -> Option<String> {
    let rest = path.strip_prefix(common_parent)?;
    let rest = rest.strip_prefix('/')?;
    (!rest.is_empty()).then(|| rest.to_string())
}

fn semantic_file_table_path_maps(
    files: Vec<FileInfo>,
) -> (HashMap<GraphNodeId, String>, HashMap<GraphNodeId, String>) {
    let rows = files
        .into_iter()
        .map(|file| {
            (
                codestory_contracts::graph::NodeId(file.id),
                normalize_semantic_store_path(&file.path),
            )
        })
        .collect::<Vec<_>>();
    let common_parent = common_absolute_semantic_parent(&rows);
    let mut display_paths = HashMap::new();
    let mut read_paths = HashMap::new();
    for (id, path) in rows {
        let normalized = common_parent
            .as_deref()
            .and_then(|common_parent| strip_semantic_common_parent(&path, common_parent))
            .unwrap_or_else(|| path.clone());
        display_paths.insert(id, normalized);
        read_paths.insert(id, path);
    }
    (display_paths, read_paths)
}

fn semantic_file_table_path_map(files: Vec<FileInfo>) -> HashMap<GraphNodeId, String> {
    let (display_paths, _) = semantic_file_table_path_maps(files);
    display_paths
}

#[derive(Default)]
struct SemanticDocGraphContext {
    child_labels: HashMap<GraphNodeId, Vec<String>>,
    referenced_labels: HashMap<GraphNodeId, Vec<String>>,
    edge_digests: HashMap<GraphNodeId, Vec<String>>,
    centrality: HashMap<GraphNodeId, DenseAnchorCentrality>,
    file_paths: HashMap<GraphNodeId, String>,
    file_read_paths: HashMap<GraphNodeId, String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SemanticDocGraphPageStats {
    endpoint_load_ms: u32,
    endpoint_rows: u32,
    endpoint_query_batches: u32,
    lookup_entries: u32,
}

#[derive(Debug, Default)]
struct SemanticNodeGraphSummary {
    child_labels: Vec<String>,
    referenced_labels: Vec<String>,
    edge_kind_counts: HashMap<String, usize>,
    centrality: DenseAnchorCentrality,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DenseAnchorCentrality {
    child_count: usize,
    related_count: usize,
    edge_count: usize,
}

impl SemanticNodeGraphSummary {
    fn observe_edge(
        &mut self,
        node: &GraphNode,
        edge: &GraphEdge,
        page_nodes: &HashMap<GraphNodeId, &GraphNode>,
        endpoint_nodes: &HashMap<GraphNodeId, GraphNode>,
        scope: SemanticDocScope,
    ) {
        let kind = format!("{:?}", edge.kind);
        *self.edge_kind_counts.entry(kind).or_insert(0) += 1;
        self.centrality.edge_count = self.centrality.edge_count.saturating_add(1);

        if edge.kind == codestory_contracts::graph::EdgeKind::MEMBER
            && edge.source == node.id
            && let Some(child) = semantic_graph_node(edge.target, page_nodes, endpoint_nodes)
            && llm_indexable_kind_for_scope(child.kind, scope)
        {
            let label = node_display_name(child);
            if !label.is_empty() {
                self.centrality.child_count = self.centrality.child_count.saturating_add(1);
                if self.child_labels.len() < 6 {
                    self.child_labels.push(label);
                }
            }
        }

        let (source, target) = edge.effective_endpoints();
        let other = if source == node.id {
            target
        } else if target == node.id {
            source
        } else {
            return;
        };
        let Some(other_node) = semantic_graph_node(other, page_nodes, endpoint_nodes) else {
            return;
        };
        if !llm_indexable_kind_for_scope(other_node.kind, scope) {
            return;
        }
        let label = node_display_name(other_node);
        if label.is_empty() {
            return;
        }
        self.centrality.related_count = self.centrality.related_count.saturating_add(1);
        if self.referenced_labels.len() < 6 && !self.referenced_labels.contains(&label) {
            self.referenced_labels.push(label);
        }
    }

    fn finish(
        self,
        limit: usize,
    ) -> (Vec<String>, Vec<String>, Vec<String>, DenseAnchorCentrality) {
        let mut counts = self.edge_kind_counts.into_iter().collect::<Vec<_>>();
        counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        let edge_digest = counts
            .into_iter()
            .take(limit)
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect();
        (
            self.child_labels,
            self.referenced_labels,
            edge_digest,
            self.centrality,
        )
    }
}

fn semantic_graph_node<'a>(
    node_id: GraphNodeId,
    page_nodes: &'a HashMap<GraphNodeId, &'a GraphNode>,
    endpoint_nodes: &'a HashMap<GraphNodeId, GraphNode>,
) -> Option<&'a GraphNode> {
    page_nodes
        .get(&node_id)
        .copied()
        .or_else(|| endpoint_nodes.get(&node_id))
}

impl SemanticDocGraphContext {
    #[cfg(test)]
    fn build(
        storage: &Storage,
        semantic_nodes: &[&GraphNode],
        all_nodes: &[GraphNode],
    ) -> Result<Self, ApiError> {
        let files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load semantic doc files: {e}")))?;
        let (file_paths, file_read_paths) = semantic_file_table_path_maps(files);
        Self::build_for_scope(
            storage,
            semantic_nodes,
            all_nodes,
            semantic_doc_scope_from_env(),
            file_paths,
            file_read_paths,
        )
    }

    fn build_for_scope(
        storage: &Storage,
        semantic_nodes: &[&GraphNode],
        all_nodes: &[GraphNode],
        scope: SemanticDocScope,
        mut file_paths: HashMap<GraphNodeId, String>,
        mut file_read_paths: HashMap<GraphNodeId, String>,
    ) -> Result<Self, ApiError> {
        let nodes_by_id = all_nodes
            .iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let node_ids = semantic_nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let edges_by_node = storage.get_edges_for_node_ids(&node_ids).map_err(|e| {
            ApiError::internal(format!("Failed to load semantic doc graph context: {e}"))
        })?;
        let context_file_ids = semantic_nodes
            .iter()
            .filter_map(|node| node.file_node_id)
            .collect::<HashSet<_>>();
        file_paths.retain(|file_id, _| context_file_ids.contains(file_id));
        file_read_paths.retain(|file_id, _| context_file_ids.contains(file_id));
        for file_id in context_file_ids {
            if let Some(file_node) = nodes_by_id.get(&file_id) {
                file_paths
                    .entry(file_id)
                    .or_insert_with(|| file_node.serialized_name.clone());
                file_read_paths
                    .entry(file_id)
                    .or_insert_with(|| file_node.serialized_name.clone());
            }
        }

        let mut context = Self {
            file_paths,
            file_read_paths,
            ..Default::default()
        };
        let endpoint_nodes = HashMap::new();
        for node in semantic_nodes {
            let edges = edges_by_node
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut summary = SemanticNodeGraphSummary::default();
            for edge in edges {
                summary.observe_edge(node, edge, &nodes_by_id, &endpoint_nodes, scope);
            }
            let (child_labels, referenced_labels, edge_digest, centrality) = summary.finish(6);
            context.child_labels.insert(node.id, child_labels);
            context.referenced_labels.insert(node.id, referenced_labels);
            context.edge_digests.insert(node.id, edge_digest);
            context.centrality.insert(node.id, centrality);
        }

        Ok(context)
    }

    fn build_for_full_page(
        storage: &Storage,
        semantic_nodes: &[GraphNode],
        scope: SemanticDocScope,
        all_file_paths: &HashMap<GraphNodeId, String>,
        all_file_read_paths: &HashMap<GraphNodeId, String>,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<(Self, SemanticDocGraphPageStats), ApiError> {
        let semantic_node_ids = semantic_nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let page_nodes = semantic_nodes
            .iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let context_file_ids = semantic_nodes
            .iter()
            .filter_map(|node| node.file_node_id)
            .collect::<HashSet<_>>();
        let mut stats = SemanticDocGraphPageStats {
            lookup_entries: clamp_usize_to_u32(semantic_nodes.len()),
            ..Default::default()
        };
        let mut summaries = semantic_node_ids
            .iter()
            .copied()
            .map(|node_id| (node_id, SemanticNodeGraphSummary::default()))
            .collect::<HashMap<_, _>>();
        let mut file_paths = context_file_ids
            .iter()
            .filter_map(|file_id| {
                all_file_paths
                    .get(file_id)
                    .cloned()
                    .map(|path| (*file_id, path))
            })
            .collect::<HashMap<_, _>>();
        let mut file_read_paths = context_file_ids
            .iter()
            .filter_map(|file_id| {
                all_file_read_paths
                    .get(file_id)
                    .cloned()
                    .map(|path| (*file_id, path))
            })
            .collect::<HashMap<_, _>>();

        let mut missing_file_ids = context_file_ids
            .iter()
            .filter(|file_id| !all_file_paths.contains_key(file_id))
            .copied()
            .collect::<Vec<_>>();
        missing_file_ids.sort_unstable_by_key(|node_id| node_id.0);
        if !missing_file_ids.is_empty() {
            let endpoint_load_started = Instant::now();
            let file_lookup = storage
                .get_nodes_by_ids_no_cache_for_build(&missing_file_ids)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to load semantic file-node fallbacks: {e}"))
                })?;
            stats.endpoint_load_ms = stats.endpoint_load_ms.saturating_add(clamp_u128_to_u32(
                endpoint_load_started.elapsed().as_millis(),
            ));
            stats.endpoint_rows = stats
                .endpoint_rows
                .saturating_add(clamp_usize_to_u32(file_lookup.nodes.len()));
            stats.endpoint_query_batches = stats
                .endpoint_query_batches
                .saturating_add(clamp_usize_to_u32(file_lookup.query_batches));
            stats.lookup_entries = stats.lookup_entries.max(clamp_usize_to_u32(
                semantic_nodes.len().saturating_add(file_lookup.nodes.len()),
            ));
            for (file_id, file_node) in file_lookup.nodes {
                file_paths
                    .entry(file_id)
                    .or_insert_with(|| file_node.serialized_name.clone());
                file_read_paths
                    .entry(file_id)
                    .or_insert_with(|| file_node.serialized_name.clone());
            }
        }

        for seed_node_ids in semantic_node_ids.chunks(BUILD_EDGE_SEED_BATCH_SIZE) {
            let seed_node_id_set = seed_node_ids.iter().copied().collect::<HashSet<_>>();
            let mut after_edge_id = None;
            loop {
                if is_indexing_cancelled(cancel_token) {
                    return Err(indexing_cancelled_error());
                }
                let edges = storage
                    .get_edges_for_node_ids_batch_after_for_build(
                        seed_node_ids,
                        after_edge_id,
                        SEMANTIC_EDGE_STREAM_BATCH_SIZE,
                    )
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "Failed to stream semantic doc graph context: {e}"
                        ))
                    })?;
                if edges.is_empty() {
                    break;
                }
                after_edge_id = edges.last().map(|edge| edge.id);

                let mut endpoint_ids = HashSet::new();
                for edge in &edges {
                    let (source, target) = edge.effective_endpoints();
                    let mut assigned_node_ids = [None, None];
                    if seed_node_id_set.contains(&source) {
                        assigned_node_ids[0] = Some(source);
                    }
                    if target != source && seed_node_id_set.contains(&target) {
                        assigned_node_ids[1] = Some(target);
                    }
                    if assigned_node_ids.iter().all(Option::is_none) {
                        continue;
                    }
                    endpoint_ids.insert(source);
                    endpoint_ids.insert(target);
                    if edge.kind == codestory_contracts::graph::EdgeKind::MEMBER
                        && assigned_node_ids.contains(&Some(edge.source))
                    {
                        endpoint_ids.insert(edge.target);
                    }
                }
                endpoint_ids.retain(|node_id| !page_nodes.contains_key(node_id));
                let mut endpoint_ids = endpoint_ids.into_iter().collect::<Vec<_>>();
                endpoint_ids.sort_unstable_by_key(|node_id| node_id.0);

                let endpoint_load_started = Instant::now();
                let endpoint_lookup = storage
                    .get_nodes_by_ids_no_cache_for_build(&endpoint_ids)
                    .map_err(|e| {
                        ApiError::internal(format!("Failed to load semantic endpoint nodes: {e}"))
                    })?;
                stats.endpoint_load_ms = stats.endpoint_load_ms.saturating_add(clamp_u128_to_u32(
                    endpoint_load_started.elapsed().as_millis(),
                ));
                stats.endpoint_rows = stats
                    .endpoint_rows
                    .saturating_add(clamp_usize_to_u32(endpoint_lookup.nodes.len()));
                stats.endpoint_query_batches = stats
                    .endpoint_query_batches
                    .saturating_add(clamp_usize_to_u32(endpoint_lookup.query_batches));
                stats.lookup_entries = stats.lookup_entries.max(clamp_usize_to_u32(
                    semantic_nodes
                        .len()
                        .saturating_add(endpoint_lookup.nodes.len()),
                ));

                for edge in &edges {
                    let (source, target) = edge.effective_endpoints();
                    if seed_node_id_set.contains(&source)
                        && let Some(node) = page_nodes.get(&source).copied()
                    {
                        summaries.entry(source).or_default().observe_edge(
                            node,
                            edge,
                            &page_nodes,
                            &endpoint_lookup.nodes,
                            scope,
                        );
                    }
                    if target != source
                        && seed_node_id_set.contains(&target)
                        && let Some(node) = page_nodes.get(&target).copied()
                    {
                        summaries.entry(target).or_default().observe_edge(
                            node,
                            edge,
                            &page_nodes,
                            &endpoint_lookup.nodes,
                            scope,
                        );
                    }
                }
            }
        }

        let mut context = Self {
            file_paths,
            file_read_paths,
            ..Default::default()
        };
        for node in semantic_nodes {
            let summary = summaries.remove(&node.id).unwrap_or_default();
            let (child_labels, referenced_labels, edge_digest, centrality) = summary.finish(6);
            context.child_labels.insert(node.id, child_labels);
            context.referenced_labels.insert(node.id, referenced_labels);
            context.edge_digests.insert(node.id, edge_digest);
            context.centrality.insert(node.id, centrality);
        }

        Ok((context, stats))
    }

    fn file_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        node.file_node_id
            .and_then(|file_id| self.file_paths.get(&file_id))
            .map(String::as_str)
    }

    fn file_read_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        node.file_node_id.and_then(|file_id| {
            self.file_read_paths
                .get(&file_id)
                .or_else(|| self.file_paths.get(&file_id))
                .map(String::as_str)
        })
    }
}

fn semantic_graph_dependent_file_ids_by_seed(
    storage: &Storage,
    seed_file_ids: &HashSet<GraphNodeId>,
) -> Result<HashMap<GraphNodeId, HashSet<GraphNodeId>>, ApiError> {
    let mut dependent_file_ids = seed_file_ids
        .iter()
        .copied()
        .map(|file_id| (file_id, HashSet::from([file_id])))
        .collect::<HashMap<_, _>>();
    if seed_file_ids.is_empty() {
        return Ok(dependent_file_ids);
    }

    let nodes = storage.get_nodes().map_err(|error| {
        ApiError::internal(format!("Failed to load semantic dependency nodes: {error}"))
    })?;
    let file_id_by_node = nodes
        .iter()
        .filter_map(|node| {
            node.file_node_id
                .or_else(|| {
                    (node.kind == codestory_contracts::graph::NodeKind::FILE).then_some(node.id)
                })
                .map(|file_id| (node.id, file_id))
        })
        .collect::<HashMap<_, _>>();
    let seed_node_ids = file_id_by_node
        .iter()
        .filter_map(|(node_id, file_id)| seed_file_ids.contains(file_id).then_some(*node_id))
        .collect::<Vec<_>>();
    if seed_node_ids.is_empty() {
        return Ok(dependent_file_ids);
    }

    let edges_by_node = storage
        .get_edges_for_node_ids(&seed_node_ids)
        .map_err(|error| {
            ApiError::internal(format!("Failed to load semantic dependency edges: {error}"))
        })?;
    let mut seen_edge_ids = HashSet::new();
    for edge in edges_by_node.into_values().flatten() {
        if !seen_edge_ids.insert(edge.id) {
            continue;
        }
        let endpoint_file_ids = [
            Some(edge.source),
            Some(edge.target),
            edge.resolved_source,
            edge.resolved_target,
        ]
        .into_iter()
        .flatten()
        .filter_map(|node_id| file_id_by_node.get(&node_id).copied())
        .collect::<HashSet<_>>();
        for seed_file_id in endpoint_file_ids
            .iter()
            .filter(|file_id| seed_file_ids.contains(file_id))
        {
            dependent_file_ids
                .entry(*seed_file_id)
                .or_default()
                .extend(endpoint_file_ids.iter().copied());
        }
    }
    Ok(dependent_file_ids)
}

fn build_semantic_file_text_cache(
    graph_context: &SemanticDocGraphContext,
    semantic_nodes: &[&GraphNode],
) -> HashMap<String, Option<String>> {
    build_semantic_file_text_cache_with_limits(
        graph_context,
        semantic_nodes,
        SEMANTIC_FILE_TEXT_MAX_BYTES,
        SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES,
    )
}

fn build_semantic_file_text_cache_with_limits(
    graph_context: &SemanticDocGraphContext,
    semantic_nodes: &[&GraphNode],
    max_file_bytes: u64,
    max_cache_bytes: usize,
) -> HashMap<String, Option<String>> {
    let file_paths = semantic_nodes
        .iter()
        .filter_map(|node| {
            let display_path = graph_context.file_path_for_node(node)?.to_string();
            let read_path = graph_context
                .file_read_path_for_node(node)
                .unwrap_or(display_path.as_str())
                .to_string();
            Some((display_path, read_path))
        })
        .collect::<HashMap<_, _>>();
    build_semantic_file_text_cache_from_paths_with_limits(
        &file_paths,
        max_file_bytes,
        max_cache_bytes,
    )
}

fn build_semantic_file_text_cache_from_paths(
    file_paths: &HashMap<String, String>,
) -> HashMap<String, Option<String>> {
    build_semantic_file_text_cache_from_paths_with_limits(
        file_paths,
        SEMANTIC_FILE_TEXT_MAX_BYTES,
        SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES,
    )
}

fn build_semantic_file_text_cache_from_paths_with_limits(
    file_paths: &HashMap<String, String>,
    max_file_bytes: u64,
    max_cache_bytes: usize,
) -> HashMap<String, Option<String>> {
    let mut file_paths = file_paths
        .iter()
        .map(|(display_path, read_path)| (display_path.clone(), read_path.clone()))
        .collect::<Vec<_>>();
    file_paths.sort_by(|left, right| left.0.cmp(&right.0));

    let mut cached_bytes = 0usize;
    let mut cache_exhausted = false;
    let mut cache = HashMap::with_capacity(file_paths.len());
    for (display_path, read_path) in file_paths {
        if cache_exhausted {
            cache.insert(display_path, None);
            continue;
        }

        let contents = read_file_text_limited(Path::new(&read_path), max_file_bytes)
            .ok()
            .flatten();
        let Some(contents) = contents else {
            cache.insert(display_path, None);
            continue;
        };

        let body_bytes = contents.len();
        if cached_bytes.saturating_add(body_bytes) > max_cache_bytes {
            cache_exhausted = true;
            cache.insert(display_path, None);
            continue;
        }

        cached_bytes = cached_bytes.saturating_add(body_bytes);
        cache.insert(display_path, Some(contents));
    }
    cache
}

fn edge_digest_for_edges(edges: &[GraphEdge], limit: usize) -> Vec<String> {
    let mut by_kind = HashMap::<String, usize>::new();
    for edge in edges {
        let key = format!("{:?}", edge.kind);
        *by_kind.entry(key).or_insert(0) += 1;
    }

    let mut counts = by_kind.into_iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    counts
        .into_iter()
        .take(limit)
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect()
}

fn edge_digest_for_node(storage: &Storage, node_id: GraphNodeId, limit: usize) -> Vec<String> {
    storage
        .get_edges_for_node_ids(&[node_id])
        .ok()
        .and_then(|edges_by_node| edges_by_node.get(&node_id).cloned())
        .map(|edges| edge_digest_for_edges(&edges, limit))
        .unwrap_or_default()
}

fn compact_doc_lines(lines: impl Iterator<Item = String>, limit: usize) -> Vec<String> {
    lines
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect()
}

fn semantic_doc_budget_cost(token: &str) -> usize {
    token.chars().count().div_ceil(3).max(1)
}

#[cfg(test)]
fn semantic_doc_text_budget_cost(doc_text: &str) -> usize {
    doc_text
        .split_whitespace()
        .map(semantic_doc_budget_cost)
        .sum()
}

fn truncate_semantic_doc_text_to_token_budget(doc_text: &str, max_tokens: usize) -> String {
    let mut remaining = max_tokens;
    let mut out = String::new();

    'lines: for line in doc_text.lines() {
        if remaining == 0 {
            break;
        }
        let mut selected = Vec::new();
        for token in line.split_whitespace() {
            let cost = semantic_doc_budget_cost(token);
            if cost > remaining {
                break 'lines;
            }
            selected.push(token);
            remaining -= cost;
        }
        if selected.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&selected.join(" "));
    }

    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn comment_block_before(lines: &[&str], start_idx: usize, limit: usize) -> Vec<String> {
    if start_idx == 0 {
        return Vec::new();
    }

    let mut block = Vec::new();
    for idx in (0..start_idx).rev() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            if block.is_empty() {
                continue;
            }
            break;
        }
        if trimmed.starts_with("//")
            || trimmed.starts_with("///")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            block.push(trimmed.to_string());
            if block.len() >= limit {
                break;
            }
            continue;
        }
        break;
    }
    block.reverse();
    block
}

fn symbol_excerpt(
    node: &codestory_contracts::graph::Node,
    file_path: Option<&str>,
    file_text_cache: &HashMap<String, Option<String>>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let Some(path) = file_path else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let Some(contents) = file_text_cache
        .get(path)
        .and_then(|contents| contents.as_deref())
    else {
        return (Vec::new(), Vec::new(), Vec::new());
    };

    let lines = contents.lines().collect::<Vec<_>>();
    let start_idx = node.start_line.unwrap_or(1).saturating_sub(1) as usize;
    let mut signature = Vec::new();
    if let Some(line) = lines.get(start_idx) {
        signature.push(line.trim().to_string());
    }

    let end_idx = node
        .end_line
        .map(|value| value as usize)
        .unwrap_or_else(|| start_idx.saturating_add(8).saturating_add(1))
        .min(lines.len());
    let body_start = start_idx.saturating_add(1).min(lines.len());
    let body = compact_doc_lines(
        lines[body_start..end_idx]
            .iter()
            .map(|line| (*line).to_string()),
        6,
    );
    let comments = comment_block_before(&lines, start_idx.min(lines.len()), 4);
    (signature, comments, body)
}

#[cfg(test)]
fn build_llm_symbol_doc_text(
    graph_context: &SemanticDocGraphContext,
    node: &GraphNode,
    display_name: &str,
    file_path: Option<&str>,
    file_text_cache: &HashMap<String, Option<String>>,
) -> String {
    build_llm_symbol_doc_text_with_policy(
        graph_context,
        node,
        display_name,
        file_path,
        file_text_cache,
        semantic_doc_alias_mode_from_env(),
        semantic_doc_max_tokens_from_env(),
    )
}

fn build_llm_symbol_doc_text_with_policy(
    graph_context: &SemanticDocGraphContext,
    node: &GraphNode,
    display_name: &str,
    file_path: Option<&str>,
    file_text_cache: &HashMap<String, Option<String>>,
    alias_mode: SemanticDocAliasMode,
    max_tokens: usize,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{LLM_SYMBOL_DOC_VERSION_PREFIX} {LLM_SYMBOL_DOC_SCHEMA_VERSION}"
    );
    let _ = writeln!(out, "symbol: {display_name}");
    let _ = writeln!(out, "kind: {:?}", node.kind);
    if let Some(line) = node.start_line {
        let _ = writeln!(out, "line: {line}");
    }
    if let Some(qualified_name) = node.qualified_name.as_deref() {
        let _ = writeln!(out, "qualified_name: {qualified_name}");
    }
    let (signature, comments, body) = symbol_excerpt(node, file_path, file_text_cache);
    if !comments.is_empty() {
        let _ = writeln!(out, "comments: {}", comments.join(" "));
    }
    if alias_mode != SemanticDocAliasMode::NoAlias {
        if let Some(language) = semantic_doc_language_from_path(file_path) {
            let _ = writeln!(out, "language: {language}");
        }

        let aliases = semantic_symbol_aliases(display_name, node.qualified_name.as_deref());
        if alias_mode == SemanticDocAliasMode::CurrentAlias && !aliases.name_aliases.is_empty() {
            let _ = writeln!(out, "name_aliases: {}", aliases.name_aliases.join(", "));
        }
        if let Some(terminal_alias) = aliases.terminal_alias {
            let _ = writeln!(out, "terminal_alias: {terminal_alias}");
        }
        if !aliases.owner_aliases.is_empty() {
            let _ = writeln!(out, "owner_aliases: {}", aliases.owner_aliases.join(", "));
        }
        if alias_mode == SemanticDocAliasMode::CurrentAlias {
            let path_aliases = semantic_path_aliases(file_path, 8);
            if !path_aliases.is_empty() {
                let _ = writeln!(out, "path_aliases: {}", path_aliases.join(", "));
            }
        }
        let _ = writeln!(
            out,
            "symbol_role: {}",
            semantic_symbol_role_aliases(node.kind)
        );
    }
    if !signature.is_empty() {
        let _ = writeln!(out, "signature: {}", signature.join(" "));
    }
    if !body.is_empty() {
        let _ = writeln!(out, "body_summary: {}", body.join(" "));
    }
    if let Some(path) = file_path {
        let _ = writeln!(out, "file: {path}");
        let path_lower = path.to_ascii_lowercase();
        if path_lower.contains("/tests/") || path_lower.contains("\\tests\\") {
            let _ = writeln!(out, "file_role: test");
        } else if path_lower.contains("/docs/")
            || path_lower.contains("\\docs\\")
            || path_lower.ends_with(".md")
        {
            let _ = writeln!(out, "file_role: docs");
        }
    }

    let children = graph_context
        .child_labels
        .get(&node.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !children.is_empty() {
        let _ = writeln!(out, "members: {}", children.join(", "));
    }

    let related = graph_context
        .referenced_labels
        .get(&node.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !related.is_empty() {
        let _ = writeln!(out, "related_symbols: {}", related.join(", "));
    }

    let edge_digest = graph_context
        .edge_digests
        .get(&node.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !edge_digest.is_empty() {
        out.push_str("edge_digest:");
        for digest in edge_digest {
            let _ = write!(out, " {digest};");
        }
        out.push('\n');
    }

    out = truncate_semantic_doc_text_to_token_budget(&out, max_tokens);

    out
}

fn map_llm_doc_to_search(doc: LlmSymbolDoc) -> LlmSearchDoc {
    let file_role = doc
        .file_path
        .as_deref()
        .map(retrieval_file_role_from_path)
        .unwrap_or(RetrievalFileRole::Source);
    LlmSearchDoc {
        node_id: doc.node_id,
        file_role,
        doc_text: doc.doc_text,
        embedding: doc.embedding,
    }
}

#[derive(Debug, Clone)]
struct PendingLlmSymbolDoc {
    node_id: codestory_contracts::graph::NodeId,
    file_node_id: Option<codestory_contracts::graph::NodeId>,
    kind: codestory_contracts::graph::NodeKind,
    display_name: String,
    qualified_name: Option<String>,
    file_path: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    doc_text: String,
    doc_hash: String,
    dense_reason: DenseAnchorReason,
}

#[derive(Debug)]
struct BuiltLlmSymbolDoc {
    symbol_doc: SymbolSearchDoc,
    pending: Option<PendingLlmSymbolDoc>,
    reusable: bool,
}

#[cfg(test)]
fn llm_symbol_doc_hash(doc_text: &str) -> String {
    llm_symbol_doc_hash_with_alias(doc_text, semantic_doc_alias_mode_from_env())
}

fn llm_symbol_doc_hash_with_alias(doc_text: &str, alias_mode: SemanticDocAliasMode) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in LLM_SYMBOL_DOC_SCHEMA_VERSION
        .to_le_bytes()
        .into_iter()
        .chain(alias_mode.as_str().as_bytes().iter().copied())
        .chain(std::iter::once(0))
        .chain(doc_text.as_bytes().iter().copied())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn observe_dense_anchor_reason(stats: &mut SemanticProjectionStats, reason: DenseAnchorReason) {
    match reason {
        DenseAnchorReason::PublicApi => {
            stats.dense_public_api = stats.dense_public_api.saturating_add(1);
        }
        DenseAnchorReason::Entrypoint => {
            stats.dense_entrypoint = stats.dense_entrypoint.saturating_add(1);
        }
        DenseAnchorReason::DocumentedNontrivial => {
            stats.dense_documented_nontrivial = stats.dense_documented_nontrivial.saturating_add(1);
        }
        DenseAnchorReason::CentralGraphNode => {
            stats.dense_central_graph_node = stats.dense_central_graph_node.saturating_add(1);
        }
        DenseAnchorReason::ComponentReport => {
            stats.dense_component_report = stats.dense_component_report.saturating_add(1);
        }
        DenseAnchorReason::UnstructuredDoc => {
            stats.dense_unstructured_doc = stats.dense_unstructured_doc.saturating_add(1);
        }
    }
}

fn dense_anchor_score(graph_context: &SemanticDocGraphContext, node_id: GraphNodeId) -> usize {
    let centrality = graph_context
        .centrality
        .get(&node_id)
        .copied()
        .unwrap_or_default();
    centrality
        .child_count
        .saturating_add(centrality.related_count)
        .saturating_add(centrality.edge_count)
}

fn dense_anchor_is_central(graph_context: &SemanticDocGraphContext, node_id: GraphNodeId) -> bool {
    let centrality = graph_context
        .centrality
        .get(&node_id)
        .copied()
        .unwrap_or_default();
    centrality
        .child_count
        .saturating_add(centrality.related_count)
        >= DENSE_CENTRAL_RELATIONSHIP_THRESHOLD
        && dense_anchor_score(graph_context, node_id) >= DENSE_CENTRAL_SCORE_THRESHOLD
}

fn semantic_component_key_for_path(path: Option<&str>) -> Option<String> {
    let path = path?.replace('\\', "/");
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let parts = parent
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Some("dir:.".into());
    }
    if let Some(index) = parts.iter().position(|part| *part == "crates")
        && let Some(crate_name) = parts.get(index.saturating_add(1))
    {
        return Some(format!("crate:{crate_name}"));
    }
    if let Some(index) = parts.iter().position(|part| *part == "src") {
        if let Some(module) = parts.get(index.saturating_add(1)) {
            return Some(format!("module:src/{module}"));
        }
        return Some("module:src".into());
    }
    Some(format!(
        "dir:{}",
        parts.iter().take(2).copied().collect::<Vec<_>>().join("/")
    ))
}

fn virtual_component_report_node_id(component_key: &str) -> GraphNodeId {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in component_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let value = ((hash & 0x3fff_ffff_ffff_ffff) as i64).max(1);
    codestory_contracts::graph::NodeId(-value)
}

fn semantic_file_is_entrypoint(path: Option<&str>, display_name: &str) -> bool {
    let name = display_name
        .rsplit("::")
        .next()
        .unwrap_or(display_name)
        .to_ascii_lowercase();
    if name == "main" {
        return true;
    }
    semantic_path_is_entrypoint_file(path)
        && matches!(
            name.as_str(),
            "__main__"
                | "app"
                | "application"
                | "asgi"
                | "function"
                | "handler"
                | "index"
                | "program"
                | "route"
                | "routes"
                | "run"
                | "server"
                | "start"
                | "startup"
                | "wsgi"
        )
}

fn semantic_path_is_entrypoint_file(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    [
        "/main.rs",
        "/main.c",
        "/main.cc",
        "/main.cpp",
        "/main.cxx",
        "/main.go",
        "/main.java",
        "/main.py",
        "/app.js",
        "/app.jsx",
        "/app.py",
        "/app.rb",
        "/app.ts",
        "/app.tsx",
        "/application.java",
        "/asgi.py",
        "/config.ru",
        "/index.js",
        "/index.jsx",
        "/index.php",
        "/index.rb",
        "/index.ts",
        "/index.tsx",
        "/program.cs",
        "/route.js",
        "/route.jsx",
        "/route.ts",
        "/route.tsx",
        "/server.js",
        "/server.jsx",
        "/server.py",
        "/server.rb",
        "/server.ts",
        "/server.tsx",
        "/startup.cs",
        "/wsgi.py",
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
        || (normalized.contains("/cmd/") && normalized.ends_with("/main.go"))
        || (normalized.contains("/src/main/java/") && normalized.ends_with("application.java"))
        || (normalized.contains("/src/main/kotlin/") && normalized.ends_with("application.kt"))
}

fn semantic_file_is_public_surface(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.ends_with("/lib.rs")
        || normalized.ends_with("/mod.rs")
        || normalized.ends_with("/public.rs")
        || normalized.ends_with("/__init__.py")
        || normalized.ends_with("/index.js")
        || normalized.ends_with("/index.jsx")
        || normalized.ends_with("/index.php")
        || normalized.ends_with("/index.rb")
        || normalized.ends_with("/index.ts")
        || normalized.ends_with("/index.tsx")
        || normalized.ends_with("/package.json")
        || normalized.starts_with("api/")
        || normalized.contains("/api/")
        || normalized.starts_with("apps/")
        || normalized.contains("/apps/")
        || normalized.starts_with("include/")
        || normalized.contains("/include/")
        || normalized.starts_with("pkg/")
        || normalized.contains("/pkg/")
        || normalized.starts_with("public/")
        || normalized.contains("/public/")
        || normalized.starts_with("routes/")
        || normalized.contains("/routes/")
        || normalized.starts_with("controllers/")
        || normalized.contains("/controllers/")
        || normalized.starts_with("components/")
        || normalized.contains("/components/")
        || normalized.contains("/src/main/java/")
        || normalized.contains("/src/main/kotlin/")
}

fn dense_anchor_public_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::STRUCT
            | codestory_contracts::graph::NodeKind::CLASS
            | codestory_contracts::graph::NodeKind::INTERFACE
            | codestory_contracts::graph::NodeKind::ANNOTATION
            | codestory_contracts::graph::NodeKind::UNION
            | codestory_contracts::graph::NodeKind::ENUM
            | codestory_contracts::graph::NodeKind::TYPEDEF
            | codestory_contracts::graph::NodeKind::GLOBAL_VARIABLE
            | codestory_contracts::graph::NodeKind::CONSTANT
    )
}

fn dense_anchor_callable_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::FUNCTION
            | codestory_contracts::graph::NodeKind::METHOD
            | codestory_contracts::graph::NodeKind::MACRO
    )
}

fn semantic_file_is_package_callable_surface(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let source_extension = [
        ".bash", ".c", ".cc", ".cjs", ".cpp", ".cs", ".dart", ".fish", ".go", ".h", ".hpp",
        ".java", ".js", ".jsx", ".kt", ".kts", ".mjs", ".php", ".py", ".rb", ".sh", ".swift",
        ".ts", ".tsx", ".zsh",
    ]
    .iter()
    .any(|suffix| file_name.ends_with(suffix));
    if !source_extension {
        return false;
    }
    normalized.contains("/lib/")
        || normalized.contains("/src/")
        || normalized.contains("/pkg/")
        || normalized.contains("/packages/")
        || normalized.contains("/routes/")
        || normalized.contains("/router/")
        || normalized.contains("/controllers/")
        || normalized.contains("/middleware/")
        || normalized.contains("/sources/")
        || matches!(
            file_name,
            "application.js"
                | "context.go"
                | "gin.go"
                | "http.dart"
                | "nvm.sh"
                | "request.js"
                | "response.js"
                | "routergroup.go"
                | "sessions.py"
                | "tree.go"
        )
}

fn semantic_doc_is_documented_nontrivial(doc_text: &str) -> bool {
    if !doc_text.contains("comments:") {
        return false;
    }
    doc_text
        .lines()
        .find_map(|line| line.strip_prefix("body_summary:"))
        .is_some_and(|body| body.split_whitespace().count() >= 8)
}

fn dense_anchor_reason_for_node(
    graph_context: &SemanticDocGraphContext,
    node: &GraphNode,
    display_name: &str,
    file_path: Option<&str>,
    doc_text: &str,
    access: Option<AccessKind>,
) -> Option<DenseAnchorReason> {
    let file_role = file_path
        .map(retrieval_file_role_from_path)
        .unwrap_or(RetrievalFileRole::Source);
    let central = dense_anchor_is_central(graph_context, node.id);

    if file_role == RetrievalFileRole::Docs {
        return Some(DenseAnchorReason::UnstructuredDoc);
    }
    if file_role.is_non_primary() && !central {
        return None;
    }
    if semantic_file_is_entrypoint(file_path, display_name) {
        return Some(DenseAnchorReason::Entrypoint);
    }
    if central {
        return Some(DenseAnchorReason::CentralGraphNode);
    }
    if dense_anchor_public_kind(node.kind)
        && (matches!(access, Some(AccessKind::Public | AccessKind::Protected))
            || semantic_file_is_public_surface(file_path))
    {
        return Some(DenseAnchorReason::PublicApi);
    }
    if dense_anchor_callable_kind(node.kind) && semantic_file_is_package_callable_surface(file_path)
    {
        return Some(DenseAnchorReason::PublicApi);
    }
    if semantic_doc_is_documented_nontrivial(doc_text) {
        return Some(DenseAnchorReason::DocumentedNontrivial);
    }
    None
}

fn is_retrieval_artifact_node(node: &GraphNode) -> bool {
    node.serialized_name.starts_with("component_report:")
        || node
            .canonical_id
            .as_deref()
            .is_some_and(|canonical_id| canonical_id.starts_with("codestory:component_report:"))
}

#[cfg(test)]
fn build_component_report_docs(
    graph_context: &SemanticDocGraphContext,
    semantic_nodes: &[&GraphNode],
    existing_docs: &HashMap<GraphNodeId, DenseAnchorInputReuseMetadata>,
    updated_at_epoch_ms: i64,
) -> Vec<BuiltLlmSymbolDoc> {
    build_component_report_docs_with_policy(
        graph_context,
        semantic_nodes,
        existing_docs,
        updated_at_epoch_ms,
        semantic_doc_alias_mode_from_env(),
        semantic_doc_max_tokens_from_env(),
    )
}

#[derive(Debug)]
struct ComponentReportNode {
    node: GraphNode,
    file_path: String,
    centrality: usize,
}

#[derive(Debug, Default)]
struct ComponentReportSummary {
    symbol_count: usize,
    files: BTreeSet<String>,
    top_nodes: Vec<ComponentReportNode>,
}

#[derive(Debug, Default)]
struct ComponentReportAccumulator {
    components: BTreeMap<String, ComponentReportSummary>,
}

impl ComponentReportAccumulator {
    fn observe(&mut self, graph_context: &SemanticDocGraphContext, semantic_nodes: &[&GraphNode]) {
        for node in semantic_nodes {
            let Some(file_path) = graph_context.file_path_for_node(node) else {
                continue;
            };
            let Some(component_key) = semantic_component_key_for_path(Some(file_path)) else {
                continue;
            };
            let summary = self.components.entry(component_key).or_default();
            summary.symbol_count = summary.symbol_count.saturating_add(1);
            summary.files.insert(file_path.to_string());
            if summary.files.len() > 12 {
                let last = summary.files.iter().next_back().cloned();
                if let Some(last) = last {
                    summary.files.remove(&last);
                }
            }
            summary.top_nodes.push(ComponentReportNode {
                node: (*node).clone(),
                file_path: file_path.to_string(),
                centrality: dense_anchor_score(graph_context, node.id),
            });
            summary.top_nodes.sort_by(|left, right| {
                right
                    .centrality
                    .cmp(&left.centrality)
                    .then_with(|| {
                        node_display_name(&left.node).cmp(&node_display_name(&right.node))
                    })
                    .then_with(|| left.node.id.0.cmp(&right.node.id.0))
            });
            summary.top_nodes.truncate(8);
        }
    }

    fn build_docs(
        self,
        existing_docs: &HashMap<GraphNodeId, DenseAnchorInputReuseMetadata>,
        updated_at_epoch_ms: i64,
        alias_mode: SemanticDocAliasMode,
        max_tokens: usize,
    ) -> Vec<BuiltLlmSymbolDoc> {
        self.components
            .into_iter()
            .filter_map(|(component_key, summary)| {
                let god_nodes = summary
                    .top_nodes
                    .iter()
                    .map(|entry| {
                        format!(
                            "- {} kind={:?} file={} centrality={}",
                            node_display_name(&entry.node),
                            entry.node.kind,
                            entry.file_path,
                            entry.centrality
                        )
                    })
                    .collect::<Vec<_>>();
                if god_nodes.is_empty() {
                    return None;
                }
                let files = summary.files.into_iter().collect::<Vec<_>>();
                let representative_file_path = files.first().cloned();

                let mut doc_text = String::new();
                let _ = writeln!(
                    doc_text,
                    "{LLM_SYMBOL_DOC_VERSION_PREFIX} {LLM_SYMBOL_DOC_SCHEMA_VERSION}"
                );
                let _ = writeln!(doc_text, "component_report: {component_key}");
                let _ = writeln!(
                    doc_text,
                    "source_provenance: {SYMBOL_SEARCH_DOC_PROVENANCE}"
                );
                let _ = writeln!(doc_text, "policy_version: {SEMANTIC_POLICY_VERSION}");
                if let Some(path) = representative_file_path.as_deref() {
                    let _ = writeln!(doc_text, "representative_file: {path}");
                }
                let _ = writeln!(doc_text, "symbol_count: {}", summary.symbol_count);
                let _ = writeln!(doc_text, "file_count: {}", files.len());
                if !files.is_empty() {
                    let _ = writeln!(doc_text, "files: {}", files.join("; "));
                }
                let _ = writeln!(doc_text, "god_nodes:");
                for line in god_nodes {
                    let _ = writeln!(doc_text, "{line}");
                }
                doc_text = truncate_semantic_doc_text_to_token_budget(&doc_text, max_tokens);
                let doc_hash = llm_symbol_doc_hash_with_alias(&doc_text, alias_mode);
                let node_id = virtual_component_report_node_id(&component_key);
                let display_name = format!("component_report:{component_key}");
                let qualified_name = Some(format!("codestory::component_report::{component_key}"));
                let kind = codestory_contracts::graph::NodeKind::MODULE;
                let symbol_doc = SymbolSearchDoc {
                    node_id,
                    file_node_id: None,
                    kind,
                    display_name: display_name.clone(),
                    qualified_name: qualified_name.clone(),
                    file_path: representative_file_path.clone(),
                    start_line: None,
                    doc_text: doc_text.clone(),
                    doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
                    doc_hash: doc_hash.clone(),
                    policy_version: SEMANTIC_POLICY_VERSION.to_string(),
                    source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
                    updated_at_epoch_ms,
                };
                let dense_reason = DenseAnchorReason::ComponentReport;
                let reusable = existing_docs.get(&node_id).is_some_and(|existing_doc| {
                    existing_doc.document_hash == doc_hash
                        && existing_doc.selection_reason == dense_reason.as_str()
                        && existing_doc.policy_version == SEMANTIC_POLICY_VERSION
                });
                let pending = Some(PendingLlmSymbolDoc {
                    node_id,
                    file_node_id: None,
                    kind,
                    display_name,
                    qualified_name,
                    file_path: representative_file_path,
                    start_line: None,
                    end_line: None,
                    doc_text,
                    doc_hash,
                    dense_reason,
                });
                Some(BuiltLlmSymbolDoc {
                    symbol_doc,
                    pending,
                    reusable,
                })
            })
            .collect()
    }
}

fn build_component_report_docs_with_policy(
    graph_context: &SemanticDocGraphContext,
    semantic_nodes: &[&GraphNode],
    existing_docs: &HashMap<GraphNodeId, DenseAnchorInputReuseMetadata>,
    updated_at_epoch_ms: i64,
    alias_mode: SemanticDocAliasMode,
    max_tokens: usize,
) -> Vec<BuiltLlmSymbolDoc> {
    let mut accumulator = ComponentReportAccumulator::default();
    accumulator.observe(graph_context, semantic_nodes);
    accumulator.build_docs(existing_docs, updated_at_epoch_ms, alias_mode, max_tokens)
}

fn sort_pending_dense_anchor_inputs(docs: &mut [PendingLlmSymbolDoc]) {
    docs.sort_by_key(|doc| doc.node_id.0);
}

fn flush_pending_dense_anchor_inputs(
    storage: &mut Storage,
    batch: &[PendingLlmSymbolDoc],
    source_identity: &str,
    updated_at_epoch_ms: i64,
    stats: &mut SemanticProjectionStats,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), ApiError> {
    if batch.is_empty() {
        return Ok(());
    }
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }

    let docs = batch
        .iter()
        .map(|doc| DenseAnchorInput {
            node_id: doc.node_id,
            file_node_id: doc.file_node_id,
            kind: doc.kind,
            display_name: doc.display_name.clone(),
            qualified_name: doc.qualified_name.clone(),
            file_path: doc.file_path.clone(),
            start_line: doc.start_line,
            end_line: doc.end_line,
            file_role: doc
                .file_path
                .as_deref()
                .map(Path::new)
                .map(StoreFileRole::classify_path)
                .unwrap_or(StoreFileRole::Source),
            source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
            text: doc.doc_text.clone(),
            document_hash: doc.doc_hash.clone(),
            selection_reason: doc.dense_reason.as_str().to_string(),
            policy_version: SEMANTIC_POLICY_VERSION.to_string(),
            source_identity: source_identity.to_string(),
            updated_at_epoch_ms,
        })
        .collect::<Vec<_>>();

    let upsert_started = Instant::now();
    storage
        .upsert_dense_anchor_inputs_batch(&docs)
        .map_err(|e| ApiError::internal(format!("Failed to upsert dense anchor inputs: {e}")))?;
    stats.db_upsert_ms = stats
        .db_upsert_ms
        .saturating_add(clamp_u128_to_u32(upsert_started.elapsed().as_millis()));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_semantic_symbol_nodes(
    storage: &mut Storage,
    semantic_nodes: &[&GraphNode],
    graph_context: &SemanticDocGraphContext,
    file_text_cache: &HashMap<String, Option<String>>,
    stored_docs: Option<&HashMap<GraphNodeId, SymbolSearchDoc>>,
    component_access: &HashMap<GraphNodeId, AccessKind>,
    existing_docs: &HashMap<GraphNodeId, DenseAnchorInputReuseMetadata>,
    updated_at_epoch_ms: i64,
    semantic_alias_mode: SemanticDocAliasMode,
    semantic_max_tokens: usize,
    stream_sort_window_size: usize,
    anchor_batch_size: usize,
    source_identity: &str,
    cancel_token: Option<&CancellationToken>,
    stats: &mut SemanticProjectionStats,
    doc_build_ns: &mut u128,
    pending_docs: &mut Vec<PendingLlmSymbolDoc>,
    seen_symbol_node_ids: &mut Vec<GraphNodeId>,
    seen_dense_node_ids: &mut Vec<GraphNodeId>,
) -> Result<(), ApiError> {
    for semantic_window in semantic_nodes.chunks(stream_sort_window_size.max(1)) {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let doc_build_started = Instant::now();
        let built_docs = semantic_window
            .par_iter()
            .map(|node| {
                let display_name = node_display_name(node);
                let file_path = graph_context
                    .file_path_for_node(node)
                    .map(ToString::to_string);
                let doc_text = if let Some(stored_docs) = stored_docs {
                    let stored = stored_docs.get(&node.id).ok_or_else(|| {
                        ApiError::new(
                            "semantic_projection_migration_required",
                            format!(
                                "Stored semantic document {} is missing from the pinned core",
                                node.id.0
                            ),
                        )
                    })?;
                    if stored.file_node_id != node.file_node_id
                        || stored.kind != node.kind
                        || stored.display_name != display_name
                        || stored.qualified_name != node.qualified_name
                        || stored.file_path != file_path
                        || stored.start_line != node.start_line
                        || stored.doc_version != LLM_SYMBOL_DOC_SCHEMA_VERSION
                        || stored.source_provenance != SYMBOL_SEARCH_DOC_PROVENANCE
                        || stored.doc_text.trim().is_empty()
                        || stored.doc_hash
                            != llm_symbol_doc_hash_with_alias(
                                &stored.doc_text,
                                semantic_alias_mode,
                            )
                    {
                        return Err(ApiError::new(
                            "semantic_projection_migration_required",
                            format!(
                                "Stored semantic document {} does not match the pinned graph and current document contract",
                                node.id.0
                            ),
                        ));
                    }
                    stored.doc_text.clone()
                } else {
                    build_llm_symbol_doc_text_with_policy(
                        graph_context,
                        node,
                        &display_name,
                        file_path.as_deref(),
                        file_text_cache,
                        semantic_alias_mode,
                        semantic_max_tokens,
                    )
                };
                let doc_hash = llm_symbol_doc_hash_with_alias(&doc_text, semantic_alias_mode);
                let dense_reason = dense_anchor_reason_for_node(
                    graph_context,
                    node,
                    &display_name,
                    file_path.as_deref(),
                    &doc_text,
                    component_access.get(&node.id).copied(),
                );
                let symbol_doc = SymbolSearchDoc {
                    node_id: node.id,
                    file_node_id: node.file_node_id,
                    kind: node.kind,
                    display_name: display_name.clone(),
                    qualified_name: node.qualified_name.clone(),
                    file_path: file_path.clone(),
                    start_line: node.start_line,
                    doc_text: doc_text.clone(),
                    doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
                    doc_hash: doc_hash.clone(),
                    policy_version: SEMANTIC_POLICY_VERSION.to_string(),
                    source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
                    updated_at_epoch_ms,
                };
                let pending_with_reuse = dense_reason.map(|dense_reason| {
                    let reusable = existing_docs.get(&node.id).is_some_and(|existing_doc| {
                        existing_doc.document_hash == doc_hash
                            && existing_doc.selection_reason == dense_reason.as_str()
                            && existing_doc.policy_version == SEMANTIC_POLICY_VERSION
                    });
                    (
                        PendingLlmSymbolDoc {
                            node_id: node.id,
                            file_node_id: node.file_node_id,
                            kind: node.kind,
                            display_name,
                            qualified_name: node.qualified_name.clone(),
                            file_path,
                            start_line: node.start_line,
                            end_line: node.end_line,
                            doc_text,
                            doc_hash,
                            dense_reason,
                        },
                        reusable,
                    )
                });
                let (pending, reusable) = pending_with_reuse
                    .map(|(pending, reusable)| (Some(pending), reusable))
                    .unwrap_or((None, false));

                Ok(BuiltLlmSymbolDoc {
                    symbol_doc,
                    pending,
                    reusable,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        *doc_build_ns = doc_build_ns.saturating_add(doc_build_started.elapsed().as_nanos());
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }

        let symbol_docs = built_docs
            .iter()
            .map(|built_doc| built_doc.symbol_doc.clone())
            .collect::<Vec<_>>();
        let symbol_upsert_started = Instant::now();
        storage
            .upsert_symbol_search_docs_batch(&symbol_docs)
            .map_err(|e| ApiError::internal(format!("Failed to upsert symbol search docs: {e}")))?;
        stats.db_upsert_ms = stats.db_upsert_ms.saturating_add(clamp_u128_to_u32(
            symbol_upsert_started.elapsed().as_millis(),
        ));
        stats.symbol_search_docs_written = stats
            .symbol_search_docs_written
            .saturating_add(clamp_usize_to_u32(symbol_docs.len()));

        for built_doc in built_docs {
            seen_symbol_node_ids.push(built_doc.symbol_doc.node_id);
            let Some(pending_doc) = built_doc.pending else {
                stats.dense_docs_skipped = stats.dense_docs_skipped.saturating_add(1);
                continue;
            };
            seen_dense_node_ids.push(pending_doc.node_id);
            observe_dense_anchor_reason(stats, pending_doc.dense_reason);
            if built_doc.reusable {
                stats.docs_reused = stats.docs_reused.saturating_add(1);
            } else {
                stats.docs_pending = stats.docs_pending.saturating_add(1);
            }
            pending_docs.push(pending_doc);
        }

        while pending_docs.len() >= anchor_batch_size {
            if is_indexing_cancelled(cancel_token) {
                return Err(indexing_cancelled_error());
            }
            sort_pending_dense_anchor_inputs(pending_docs);
            flush_pending_dense_anchor_inputs(
                storage,
                &pending_docs[..anchor_batch_size],
                source_identity,
                updated_at_epoch_ms,
                stats,
                cancel_token,
            )?;
            pending_docs.drain(..anchor_batch_size);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_component_report_docs(
    storage: &mut Storage,
    built_reports: Vec<BuiltLlmSymbolDoc>,
    source_identity: &str,
    updated_at_epoch_ms: i64,
    anchor_batch_size: usize,
    cancel_token: Option<&CancellationToken>,
    stats: &mut SemanticProjectionStats,
    pending_docs: &mut Vec<PendingLlmSymbolDoc>,
    seen_symbol_node_ids: &mut Vec<GraphNodeId>,
    seen_dense_node_ids: &mut Vec<GraphNodeId>,
    component_report_node_ids: &mut Vec<GraphNodeId>,
    dense_component_report_node_ids: &mut Vec<GraphNodeId>,
) -> Result<(), ApiError> {
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    if built_reports.is_empty() {
        return Ok(());
    }

    let report_symbol_docs = built_reports
        .iter()
        .map(|built_doc| built_doc.symbol_doc.clone())
        .collect::<Vec<_>>();
    let report_nodes = report_symbol_docs
        .iter()
        .map(|doc| GraphNode {
            id: doc.node_id,
            kind: doc.kind,
            serialized_name: doc.display_name.clone(),
            qualified_name: doc.qualified_name.clone(),
            canonical_id: Some(format!("codestory:{}", doc.display_name)),
            file_node_id: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
        })
        .collect::<Vec<_>>();
    storage
        .upsert_retrieval_artifact_nodes_batch(&report_nodes)
        .map_err(|e| ApiError::internal(format!("Failed to upsert component report nodes: {e}")))?;
    let symbol_upsert_started = Instant::now();
    storage
        .upsert_symbol_search_docs_batch(&report_symbol_docs)
        .map_err(|e| ApiError::internal(format!("Failed to upsert component report docs: {e}")))?;
    stats.db_upsert_ms = stats.db_upsert_ms.saturating_add(clamp_u128_to_u32(
        symbol_upsert_started.elapsed().as_millis(),
    ));
    stats.symbol_search_docs_written = stats
        .symbol_search_docs_written
        .saturating_add(clamp_usize_to_u32(report_symbol_docs.len()));

    for built_doc in built_reports {
        seen_symbol_node_ids.push(built_doc.symbol_doc.node_id);
        component_report_node_ids.push(built_doc.symbol_doc.node_id);
        let Some(pending_doc) = built_doc.pending else {
            stats.dense_docs_skipped = stats.dense_docs_skipped.saturating_add(1);
            continue;
        };
        seen_dense_node_ids.push(pending_doc.node_id);
        dense_component_report_node_ids.push(pending_doc.node_id);
        observe_dense_anchor_reason(stats, pending_doc.dense_reason);
        if built_doc.reusable {
            stats.docs_reused = stats.docs_reused.saturating_add(1);
        } else {
            stats.docs_pending = stats.docs_pending.saturating_add(1);
        }
        pending_docs.push(pending_doc);
    }

    while pending_docs.len() >= anchor_batch_size {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        sort_pending_dense_anchor_inputs(pending_docs);
        flush_pending_dense_anchor_inputs(
            storage,
            &pending_docs[..anchor_batch_size],
            source_identity,
            updated_at_epoch_ms,
            stats,
            cancel_token,
        )?;
        pending_docs.drain(..anchor_batch_size);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sync_llm_symbol_projection_for_runtime(
    storage: &mut Storage,
    nodes: &[codestory_contracts::graph::Node],
    engine: &mut SearchEngine,
    refresh_scope: SemanticRefreshScope<'_>,
    hydrate_semantic_docs: bool,
    source_identity: &str,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<SemanticProjectionStats, ApiError> {
    let mut stats = SemanticProjectionStats {
        reported: true,
        ..Default::default()
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }

    let updated_at_epoch_ms = current_epoch_ms();

    let existing_docs = storage
        .get_dense_anchor_input_reuse_metadata()
        .map_err(|e| ApiError::internal(format!("Failed to load dense anchor metadata: {e}")))?
        .into_iter()
        .map(|doc| (doc.node_id, doc))
        .collect::<HashMap<_, _>>();

    let graph_doc_contract_mismatch = refresh_scope.file_ids.is_some()
        && storage
            .has_symbol_search_doc_contract_mismatch(
                LLM_SYMBOL_DOC_SCHEMA_VERSION,
                SEMANTIC_POLICY_VERSION,
            )
            .map_err(|e| {
                ApiError::internal(format!(
                    "Failed to inspect graph-native semantic doc contract: {e}"
                ))
            })?;
    let dense_doc_contract_mismatch = refresh_scope.file_ids.is_some()
        && existing_docs
            .values()
            .any(|existing_doc| existing_doc.policy_version != SEMANTIC_POLICY_VERSION);
    let expand_semantic_scope_for_contract_repair =
        graph_doc_contract_mismatch || dense_doc_contract_mismatch;
    if expand_semantic_scope_for_contract_repair {
        tracing::warn!(
            graph_doc_contract_mismatch,
            dense_doc_contract_mismatch,
            "Stored semantic-doc contract differs from the current schema or embedding contract; expanding incremental semantic sync to rebuild all semantic docs"
        );
    }
    let effective_llm_refresh_file_scope = if expand_semantic_scope_for_contract_repair {
        None
    } else {
        refresh_scope.file_ids
    };
    let anchor_batch_size = runtime.retrieval.llm_doc_embed_batch_size;
    let semantic_alias_mode =
        semantic_doc_alias_mode_from_value(&runtime.retrieval.semantic_doc_alias_mode);
    let semantic_max_tokens = runtime.retrieval.semantic_doc_max_tokens;
    let stream_sort_window_batches = runtime.retrieval.stream_sort_window_batches;
    let stream_sort_window_size = anchor_batch_size.saturating_mul(stream_sort_window_batches);
    tracing::debug!(
        anchor_batch_size,
        "Using dense anchor input publication batch size"
    );
    let mut pending_docs = Vec::<PendingLlmSymbolDoc>::new();
    let mut seen_symbol_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut seen_dense_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut component_report_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut dense_component_report_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut doc_build_ns = 0_u128;
    let semantic_scope = semantic_doc_scope_from_value(&runtime.retrieval.semantic_doc_scope);
    let semantic_nodes = nodes
        .iter()
        .filter(|node| llm_indexable_kind_for_scope(node.kind, semantic_scope))
        .filter(|node| !is_retrieval_artifact_node(node))
        .filter(|node| {
            effective_llm_refresh_file_scope
                .map(|scope| {
                    node.file_node_id
                        .map(|file_node_id| scope.contains(&file_node_id))
                        .unwrap_or(false)
                })
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let files = storage
        .get_files()
        .map_err(|e| ApiError::internal(format!("Failed to load semantic doc files: {e}")))?;
    let (file_paths, file_read_paths) = semantic_file_table_path_maps(files);
    let effective_component_report_scope = if expand_semantic_scope_for_contract_repair {
        None
    } else if let Some(refresh) = refresh_scope.component_reports {
        let normalization_changed =
            refresh
                .previous_file_paths
                .iter()
                .any(|(file_id, old_path)| {
                    file_paths
                        .get(file_id)
                        .is_some_and(|new_path| new_path != old_path)
                });
        if normalization_changed {
            tracing::warn!(
                "Semantic file-path normalization changed; rebuilding all component reports"
            );
            None
        } else {
            let mut scope = refresh.removed_component_keys.clone();
            if let Some(file_scope) = effective_llm_refresh_file_scope {
                for file_id in file_scope {
                    if let Some(component_key) = file_paths
                        .get(file_id)
                        .and_then(|path| semantic_component_key_for_path(Some(path)))
                    {
                        scope.insert(component_key);
                    }
                }
            }
            Some(scope)
        }
    } else {
        None
    };
    if let Some(scope) = effective_component_report_scope.as_ref() {
        for node in nodes.iter().filter(|node| is_retrieval_artifact_node(node)) {
            let component_key = node
                .serialized_name
                .strip_prefix("component_report:")
                .unwrap_or(&node.serialized_name);
            if !scope.contains(component_key) {
                component_report_node_ids.push(node.id);
                dense_component_report_node_ids.push(node.id);
            }
        }
    }
    let report_semantic_nodes = nodes
        .iter()
        .filter(|node| llm_indexable_kind_for_scope(node.kind, semantic_scope))
        .filter(|node| !is_retrieval_artifact_node(node))
        .filter(|node| {
            effective_component_report_scope
                .as_ref()
                .map(|scope| {
                    node.file_node_id
                        .and_then(|file_node_id| file_paths.get(&file_node_id))
                        .and_then(|path| semantic_component_key_for_path(Some(path)))
                        .is_some_and(|component_key| scope.contains(&component_key))
                })
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let mut context_nodes = semantic_nodes.clone();
    let mut context_node_ids = semantic_nodes
        .iter()
        .map(|node| node.id)
        .collect::<HashSet<_>>();
    for node in &report_semantic_nodes {
        if context_node_ids.insert(node.id) {
            context_nodes.push(*node);
        }
    }
    let semantic_node_ids = semantic_nodes
        .iter()
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let component_access = storage
        .get_component_access_map_for_nodes(&semantic_node_ids)
        .map_err(|e| ApiError::internal(format!("Failed to load symbol access metadata: {e}")))?;
    let context_started = Instant::now();
    let graph_context = SemanticDocGraphContext::build_for_scope(
        storage,
        &context_nodes,
        nodes,
        semantic_scope,
        file_paths,
        file_read_paths,
    )?;
    stats.context_ms = clamp_u128_to_u32(context_started.elapsed().as_millis());
    stats.selected_nodes = clamp_usize_to_u32(semantic_nodes.len());
    stats.context_file_count = clamp_usize_to_u32(graph_context.file_paths.len());
    stats.context_path_bytes = clamp_usize_to_u32(
        graph_context
            .file_paths
            .values()
            .chain(graph_context.file_read_paths.values())
            .map(String::len)
            .sum(),
    );
    stats.node_lookup_entries = clamp_usize_to_u32(nodes.len());
    let file_cache_started = Instant::now();
    let file_text_cache = build_semantic_file_text_cache(&graph_context, &semantic_nodes);
    doc_build_ns = doc_build_ns.saturating_add(file_cache_started.elapsed().as_nanos());

    process_semantic_symbol_nodes(
        storage,
        &semantic_nodes,
        &graph_context,
        &file_text_cache,
        None,
        &component_access,
        &existing_docs,
        updated_at_epoch_ms,
        semantic_alias_mode,
        semantic_max_tokens,
        stream_sort_window_size,
        anchor_batch_size,
        source_identity,
        cancel_token,
        &mut stats,
        &mut doc_build_ns,
        &mut pending_docs,
        &mut seen_symbol_node_ids,
        &mut seen_dense_node_ids,
    )?;

    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let report_build_started = Instant::now();
    let built_reports = build_component_report_docs_with_policy(
        &graph_context,
        &report_semantic_nodes,
        &existing_docs,
        updated_at_epoch_ms,
        semantic_alias_mode,
        semantic_max_tokens,
    );
    doc_build_ns = doc_build_ns.saturating_add(report_build_started.elapsed().as_nanos());
    publish_component_report_docs(
        storage,
        built_reports,
        source_identity,
        updated_at_epoch_ms,
        anchor_batch_size,
        cancel_token,
        &mut stats,
        &mut pending_docs,
        &mut seen_symbol_node_ids,
        &mut seen_dense_node_ids,
        &mut component_report_node_ids,
        &mut dense_component_report_node_ids,
    )?;
    stats.doc_build_ms = clamp_u128_to_u32(doc_build_ns / 1_000_000);

    sort_pending_dense_anchor_inputs(&mut pending_docs);
    for batch in pending_docs.chunks(anchor_batch_size) {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        flush_pending_dense_anchor_inputs(
            storage,
            batch,
            source_identity,
            updated_at_epoch_ms,
            &mut stats,
            cancel_token,
        )?;
    }

    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let prune_started = Instant::now();
    let stale_symbol_docs = if let Some(scope) = effective_llm_refresh_file_scope {
        let file_node_ids = scope.iter().copied().collect::<Vec<_>>();
        storage
            .delete_symbol_search_docs_for_files_except_node_ids(
                &file_node_ids,
                &seen_symbol_node_ids,
            )
            .map_err(|e| ApiError::internal(format!("Failed to prune stale symbol docs: {e}")))?
    } else {
        storage
            .prune_symbol_search_docs_to_node_ids(&seen_symbol_node_ids)
            .map_err(|e| ApiError::internal(format!("Failed to prune stale symbol docs: {e}")))?
    };
    let stale_dense_docs = if let Some(scope) = effective_llm_refresh_file_scope {
        let file_node_ids = scope.iter().copied().collect::<Vec<_>>();
        storage
            .delete_dense_anchor_inputs_for_files_except_node_ids(
                &file_node_ids,
                &seen_dense_node_ids,
            )
            .map_err(|e| ApiError::internal(format!("Failed to prune dense anchor inputs: {e}")))?
    } else {
        storage
            .prune_dense_anchor_inputs_to_node_ids(&seen_dense_node_ids)
            .map_err(|e| ApiError::internal(format!("Failed to prune dense anchor inputs: {e}")))?
    };
    let removed_legacy_vectors = storage
        .clear_llm_symbol_docs()
        .map_err(|e| ApiError::internal(format!("Failed to remove legacy core vectors: {e}")))?;
    let stale_component_docs = storage
        .prune_retrieval_artifacts_to_node_ids(
            &component_report_node_ids,
            &dense_component_report_node_ids,
        )
        .map_err(|e| ApiError::internal(format!("Failed to prune component reports: {e}")))?;
    stats.prune_ms = clamp_u128_to_u32(prune_started.elapsed().as_millis());
    stats.docs_stale = clamp_usize_to_u32(
        stale_dense_docs
            .saturating_add(removed_legacy_vectors)
            .saturating_add(stale_symbol_docs)
            .saturating_add(stale_component_docs),
    );

    if hydrate_semantic_docs {
        engine.index_llm_symbol_docs(Vec::new());
    }

    Ok(stats)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticProjectionDocumentSource {
    SourceFiles,
    StoredCore,
}

fn sync_full_llm_symbol_projection_streaming_for_runtime(
    storage: &mut Storage,
    source_identity: &str,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    document_source: SemanticProjectionDocumentSource,
) -> Result<SemanticProjectionStats, ApiError> {
    let mut stats = SemanticProjectionStats {
        reported: true,
        ..Default::default()
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }

    let updated_at_epoch_ms = current_epoch_ms();
    let existing_docs = storage
        .get_dense_anchor_input_reuse_metadata()
        .map_err(|e| ApiError::internal(format!("Failed to load dense anchor metadata: {e}")))?
        .into_iter()
        .map(|doc| (doc.node_id, doc))
        .collect::<HashMap<_, _>>();
    let anchor_batch_size = runtime.retrieval.llm_doc_embed_batch_size;
    let semantic_alias_mode =
        semantic_doc_alias_mode_from_value(&runtime.retrieval.semantic_doc_alias_mode);
    let semantic_max_tokens = runtime.retrieval.semantic_doc_max_tokens;
    let stream_sort_window_size = anchor_batch_size
        .saturating_mul(runtime.retrieval.stream_sort_window_batches)
        .max(1);
    let semantic_scope = semantic_doc_scope_from_value(&runtime.retrieval.semantic_doc_scope);
    let semantic_kinds = llm_indexable_kinds_for_scope(semantic_scope);

    let node_load_started = Instant::now();
    let semantic_file_ids = storage
        .get_node_file_ids_by_kinds_for_build(semantic_kinds)
        .map_err(|e| ApiError::internal(format!("Failed to load semantic file ids: {e}")))?;
    stats.node_load_ms = stats
        .node_load_ms
        .saturating_add(clamp_u128_to_u32(node_load_started.elapsed().as_millis()));
    let files = storage
        .get_files()
        .map_err(|e| ApiError::internal(format!("Failed to load semantic doc files: {e}")))?;
    let (mut file_paths, mut file_read_paths) = semantic_file_table_path_maps(files);
    let semantic_file_id_set = semantic_file_ids.iter().copied().collect::<HashSet<_>>();
    file_paths.retain(|file_id, _| semantic_file_id_set.contains(file_id));
    file_read_paths.retain(|file_id, _| semantic_file_id_set.contains(file_id));

    let mut missing_file_ids = semantic_file_ids
        .iter()
        .filter(|file_id| !file_paths.contains_key(file_id))
        .copied()
        .collect::<Vec<_>>();
    missing_file_ids.sort_unstable_by_key(|file_id| file_id.0);
    if !missing_file_ids.is_empty() {
        let fallback_started = Instant::now();
        let fallback_lookup = storage
            .get_nodes_by_ids_no_cache_for_build(&missing_file_ids)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load semantic file-node fallbacks: {e}"))
            })?;
        stats.endpoint_load_ms = stats
            .endpoint_load_ms
            .saturating_add(clamp_u128_to_u32(fallback_started.elapsed().as_millis()));
        stats.endpoint_load_rows = stats
            .endpoint_load_rows
            .saturating_add(clamp_usize_to_u32(fallback_lookup.nodes.len()));
        stats.endpoint_load_batches = stats
            .endpoint_load_batches
            .saturating_add(clamp_usize_to_u32(fallback_lookup.query_batches));
        for (file_id, node) in fallback_lookup.nodes {
            file_paths
                .entry(file_id)
                .or_insert_with(|| node.serialized_name.clone());
            file_read_paths
                .entry(file_id)
                .or_insert(node.serialized_name);
        }
    }
    stats.context_file_count = clamp_usize_to_u32(file_paths.len());
    stats.context_path_bytes = clamp_usize_to_u32(
        file_paths
            .values()
            .chain(file_read_paths.values())
            .map(String::len)
            .sum(),
    );
    let mut doc_build_ns = 0_u128;
    let file_text_cache = if document_source == SemanticProjectionDocumentSource::SourceFiles {
        let mut file_text_paths = HashMap::new();
        for file_id in &semantic_file_ids {
            let Some(display_path) = file_paths.get(file_id) else {
                continue;
            };
            let read_path = file_read_paths.get(file_id).unwrap_or(display_path).clone();
            file_text_paths.insert(display_path.clone(), read_path);
        }
        let file_cache_started = Instant::now();
        let cache = build_semantic_file_text_cache_from_paths(&file_text_paths);
        doc_build_ns = doc_build_ns.saturating_add(file_cache_started.elapsed().as_nanos());
        cache
    } else {
        HashMap::new()
    };

    let mut pending_docs = Vec::<PendingLlmSymbolDoc>::new();
    let mut seen_symbol_node_ids = Vec::<GraphNodeId>::new();
    let mut seen_dense_node_ids = Vec::<GraphNodeId>::new();
    let mut component_report_node_ids = Vec::<GraphNodeId>::new();
    let mut dense_component_report_node_ids = Vec::<GraphNodeId>::new();
    let mut component_reports = ComponentReportAccumulator::default();
    let mut after_node_id = None;
    loop {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        let page_load_started = Instant::now();
        let mut semantic_nodes = storage
            .get_nodes_by_kinds_batch_after_for_build(
                semantic_kinds,
                after_node_id,
                SEMANTIC_NODE_STREAM_BATCH_SIZE,
            )
            .map_err(|e| ApiError::internal(format!("Failed to stream semantic nodes: {e}")))?;
        stats.node_load_ms = stats
            .node_load_ms
            .saturating_add(clamp_u128_to_u32(page_load_started.elapsed().as_millis()));
        if semantic_nodes.is_empty() {
            break;
        }
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SemanticNodePage, cancel_token)?;
        after_node_id = semantic_nodes.last().map(|node| node.id);
        stats.node_stream_batches = stats.node_stream_batches.saturating_add(1);
        stats.node_load_rows = stats
            .node_load_rows
            .saturating_add(clamp_usize_to_u32(semantic_nodes.len()));
        semantic_nodes.retain(|node| !is_retrieval_artifact_node(node));
        stats.selected_nodes = stats
            .selected_nodes
            .saturating_add(clamp_usize_to_u32(semantic_nodes.len()));
        if semantic_nodes.is_empty() {
            continue;
        }

        let semantic_node_ids = semantic_nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let stored_docs = if document_source == SemanticProjectionDocumentSource::StoredCore {
            let docs = storage
                .get_symbol_search_docs_for_node_ids(&semantic_node_ids)
                .map_err(|error| {
                    ApiError::internal(format!("Failed to load pinned semantic documents: {error}"))
                })?;
            if docs.len() != semantic_node_ids.len() {
                return Err(ApiError::new(
                    "semantic_projection_migration_required",
                    format!(
                        "Pinned core contains {} semantic nodes but only {} stored documents in this page",
                        semantic_node_ids.len(),
                        docs.len()
                    ),
                ));
            }
            Some(
                docs.into_iter()
                    .map(|doc| (doc.node_id, doc))
                    .collect::<HashMap<_, _>>(),
            )
        } else {
            None
        };
        #[cfg(test)]
        if document_source == SemanticProjectionDocumentSource::StoredCore {
            publication_test_checkpoint(
                PublicationTestBoundary::SemanticStoredDocumentPage,
                cancel_token,
            )?;
            if is_indexing_cancelled(cancel_token) {
                return Err(indexing_cancelled_error());
            }
        }
        let component_access = storage
            .get_component_access_map_for_nodes(&semantic_node_ids)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load symbol access metadata: {e}"))
            })?;
        let context_started = Instant::now();
        let (graph_context, page_stats) = SemanticDocGraphContext::build_for_full_page(
            storage,
            &semantic_nodes,
            semantic_scope,
            &file_paths,
            &file_read_paths,
            cancel_token,
        )?;
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SemanticEndpointRead, cancel_token)?;
        stats.context_ms = stats
            .context_ms
            .saturating_add(clamp_u128_to_u32(context_started.elapsed().as_millis()));
        stats.endpoint_load_ms = stats
            .endpoint_load_ms
            .saturating_add(page_stats.endpoint_load_ms);
        stats.endpoint_load_rows = stats
            .endpoint_load_rows
            .saturating_add(page_stats.endpoint_rows);
        stats.endpoint_load_batches = stats
            .endpoint_load_batches
            .saturating_add(page_stats.endpoint_query_batches);
        stats.node_lookup_entries = stats.node_lookup_entries.max(page_stats.lookup_entries);

        let semantic_node_refs = semantic_nodes.iter().collect::<Vec<_>>();
        component_reports.observe(&graph_context, &semantic_node_refs);
        process_semantic_symbol_nodes(
            storage,
            &semantic_node_refs,
            &graph_context,
            &file_text_cache,
            stored_docs.as_ref(),
            &component_access,
            &existing_docs,
            updated_at_epoch_ms,
            semantic_alias_mode,
            semantic_max_tokens,
            stream_sort_window_size,
            anchor_batch_size,
            source_identity,
            cancel_token,
            &mut stats,
            &mut doc_build_ns,
            &mut pending_docs,
            &mut seen_symbol_node_ids,
            &mut seen_dense_node_ids,
        )?;
    }

    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let report_build_started = Instant::now();
    let built_reports = component_reports.build_docs(
        &existing_docs,
        updated_at_epoch_ms,
        semantic_alias_mode,
        semantic_max_tokens,
    );
    doc_build_ns = doc_build_ns.saturating_add(report_build_started.elapsed().as_nanos());
    publish_component_report_docs(
        storage,
        built_reports,
        source_identity,
        updated_at_epoch_ms,
        anchor_batch_size,
        cancel_token,
        &mut stats,
        &mut pending_docs,
        &mut seen_symbol_node_ids,
        &mut seen_dense_node_ids,
        &mut component_report_node_ids,
        &mut dense_component_report_node_ids,
    )?;
    stats.doc_build_ms = clamp_u128_to_u32(doc_build_ns / 1_000_000);

    sort_pending_dense_anchor_inputs(&mut pending_docs);
    for batch in pending_docs.chunks(anchor_batch_size) {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        flush_pending_dense_anchor_inputs(
            storage,
            batch,
            source_identity,
            updated_at_epoch_ms,
            &mut stats,
            cancel_token,
        )?;
    }

    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let prune_started = Instant::now();
    let stale_symbol_docs = storage
        .prune_symbol_search_docs_to_node_ids(&seen_symbol_node_ids)
        .map_err(|e| ApiError::internal(format!("Failed to prune stale symbol docs: {e}")))?;
    let stale_dense_docs = storage
        .prune_dense_anchor_inputs_to_node_ids(&seen_dense_node_ids)
        .map_err(|e| ApiError::internal(format!("Failed to prune dense anchor inputs: {e}")))?;
    let removed_legacy_vectors = storage
        .clear_llm_symbol_docs()
        .map_err(|e| ApiError::internal(format!("Failed to remove legacy core vectors: {e}")))?;
    let stale_component_docs = storage
        .prune_retrieval_artifacts_to_node_ids(
            &component_report_node_ids,
            &dense_component_report_node_ids,
        )
        .map_err(|e| ApiError::internal(format!("Failed to prune component reports: {e}")))?;
    stats.prune_ms = clamp_u128_to_u32(prune_started.elapsed().as_millis());
    stats.docs_stale = clamp_usize_to_u32(
        stale_dense_docs
            .saturating_add(removed_legacy_vectors)
            .saturating_add(stale_symbol_docs)
            .saturating_add(stale_component_docs),
    );
    Ok(stats)
}

#[cfg(test)]
fn finalize_staged_semantic_docs(
    storage: &mut Storage,
    llm_refresh_file_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    component_report_refresh: Option<&ComponentReportRefreshScope>,
    cancel_token: Option<&CancellationToken>,
) -> Result<SemanticProjectionStats, ApiError> {
    finalize_staged_semantic_docs_for_runtime(
        storage,
        llm_refresh_file_scope,
        component_report_refresh,
        "core:test-publication",
        cancel_token,
        &test_sidecar_runtime_from_env(),
        SemanticProjectionDocumentSource::SourceFiles,
    )
}

fn finalize_staged_semantic_docs_for_runtime(
    storage: &mut Storage,
    llm_refresh_file_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    component_report_refresh: Option<&ComponentReportRefreshScope>,
    source_identity: &str,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    document_source: SemanticProjectionDocumentSource,
) -> Result<SemanticProjectionStats, ApiError> {
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let semantic_context_index_started = Instant::now();
    storage
        .create_semantic_context_endpoint_indexes_for_build()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to create staged semantic context endpoint indexes: {error}"
            ))
        })?;
    let semantic_context_index_ms =
        clamp_u128_to_u32(semantic_context_index_started.elapsed().as_millis());
    #[cfg(test)]
    publication_test_checkpoint(
        PublicationTestBoundary::SemanticContextIndexes,
        cancel_token,
    )?;
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let mut stats = if storage.is_staged_build()
        && llm_refresh_file_scope.is_none()
        && component_report_refresh.is_none()
    {
        sync_full_llm_symbol_projection_streaming_for_runtime(
            storage,
            source_identity,
            cancel_token,
            runtime,
            document_source,
        )?
    } else {
        let node_load_started = Instant::now();
        let nodes = storage
            .get_nodes()
            .map_err(|error| ApiError::internal(format!("Failed to load staged nodes: {error}")))?;
        let node_load_ms = clamp_u128_to_u32(node_load_started.elapsed().as_millis());
        let node_load_rows = clamp_usize_to_u32(nodes.len());
        let mut engine = SearchEngine::new(None).map_err(|error| {
            ApiError::internal(format!("Failed to init semantic engine: {error}"))
        })?;
        let mut stats = sync_llm_symbol_projection_for_runtime(
            storage,
            &nodes,
            &mut engine,
            SemanticRefreshScope {
                file_ids: llm_refresh_file_scope,
                component_reports: component_report_refresh,
            },
            false,
            source_identity,
            cancel_token,
            runtime,
        )?;
        stats.node_load_ms = node_load_ms;
        stats.node_load_rows = node_load_rows;
        stats
    };
    stats.semantic_context_index_ms = semantic_context_index_ms;
    Ok(stats)
}

fn load_persisted_semantic_docs_for_runtime(
    storage: &Storage,
    engine: &mut SearchEngine,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<SemanticProjectionStats, ApiError> {
    let mut stats = SemanticProjectionStats {
        reported: true,
        ..Default::default()
    };
    if !hydrate_semantic_docs || !runtime.retrieval.hybrid_enabled {
        return Ok(stats);
    }
    if let Err(error) = engine.set_embedding_runtime_for_runtime(runtime) {
        tracing::warn!(
            "embedding runtime unavailable while hydrating completed semantic docs: {error}"
        );
        return Ok(stats);
    }
    let current_contract = current_embedding_contract_for_runtime(runtime).ok_or_else(|| {
        ApiError::internal(
            "Failed to resolve current embedding profile contract after configuring runtime",
        )
    })?;
    let stored_stats = storage
        .get_llm_symbol_doc_stats()
        .map_err(|error| ApiError::internal(format!("Failed to inspect semantic docs: {error}")))?;
    if !semantic_doc_stats_match_contract(&stored_stats, &current_contract) {
        tracing::warn!(
            "Stored semantic docs do not match the current embedding contract; skipping runtime hydration until a staged reindex publishes matching docs"
        );
        return Ok(stats);
    }
    let reload_started = Instant::now();
    reload_llm_docs_from_storage(storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
    stats.reload_ms = clamp_u128_to_u32(reload_started.elapsed().as_millis());
    Ok(stats)
}

#[derive(Debug, Clone)]
pub(crate) struct HybridSearchScoredHit {
    pub hit: SearchHit,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub total_score: f32,
}

#[cfg(test)]
fn exact_symbol_merged_lexical_hybrid_hits(
    engine: &SearchEngine,
    query: &str,
    graph_boosts: &HashMap<codestory_contracts::graph::NodeId, f32>,
) -> Vec<HybridSearchHit> {
    crate::search::lexical::exact_symbol_merged_lexical_hybrid_hits_for_symbols(
        engine.symbols(),
        query,
        graph_boosts,
    )
}

#[cfg(test)]
struct HybridHitsContext<'a> {
    req: &'a SearchRequest,
    graph_boosts: &'a HashMap<codestory_contracts::graph::NodeId, f32>,
    requested_max_results: usize,
    request_weights: Option<AgentHybridWeightsDto>,
    prefer_primary_sources: bool,
    storage_retrieval: &'a RetrievalStateDto,
    use_exact_symbol_lexical_fast_path: bool,
}

#[cfg(test)]
fn hybrid_hits_for_retrieval_state(
    engine: &mut SearchEngine,
    context: HybridHitsContext<'_>,
    retrieval: &mut RetrievalStateDto,
) -> Vec<HybridSearchHit> {
    let uses_hybrid = !semantic_disabled_by_request_weights(context.request_weights.as_ref())
        && !context.use_exact_symbol_lexical_fast_path
        && retrieval.mode == RetrievalModeDto::Hybrid;
    let mut hits = if !uses_hybrid {
        exact_symbol_merged_lexical_hybrid_hits(engine, &context.req.query, context.graph_boosts)
    } else {
        let config = hybrid_search_config_for_request(
            context.req,
            context.requested_max_results,
            context.request_weights.clone(),
            context.prefer_primary_sources,
        );
        match engine.search_hybrid_with_scores(&context.req.query, context.graph_boosts, config) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    "Hybrid retrieval failed for query {:?}; falling back to symbolic ranking: {}",
                    context.req.query,
                    error
                );
                *retrieval = retrieval_state_from_parts(
                    engine.semantic_doc_count(),
                    engine.embedding_model_id().map(str::to_string),
                    engine.embedding_runtime_configured(),
                    Some(format!(
                        "Semantic query fallback engaged after runtime error: {error}"
                    )),
                    context.storage_retrieval.current_embedding.clone(),
                    context.storage_retrieval.stored_embedding.clone(),
                    true,
                );
                exact_symbol_merged_lexical_hybrid_hits(
                    engine,
                    &context.req.query,
                    context.graph_boosts,
                )
            }
        }
    };
    if uses_hybrid
        && context.request_weights.is_none()
        && !mixed_natural_language_query(&context.req.query)
    {
        let additional = exact_symbol_merged_lexical_hybrid_hits(
            engine,
            &context.req.query,
            context.graph_boosts,
        );
        merge_hybrid_hits_by_node_id(&mut hits, additional);
    }
    hits
}

#[cfg(test)]
fn exact_symbol_lexical_fast_path(
    req: &SearchRequest,
    request_weights: Option<&AgentHybridWeightsDto>,
) -> bool {
    request_weights.is_none()
        && req.hybrid_weights.is_none()
        && req
            .hybrid_limits
            .as_ref()
            .and_then(|limits| limits.semantic)
            .is_none()
        && !exact_symbol_query_terms(&req.query).is_empty()
        && has_fast_path_symbol_signal(&req.query)
}

#[cfg(test)]
fn semantic_disabled_by_request_weights(request_weights: Option<&AgentHybridWeightsDto>) -> bool {
    request_weights
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
}

#[cfg(test)]
fn has_fast_path_symbol_signal(query: &str) -> bool {
    let trimmed = query.trim();
    looks_like_standalone_symbol_query(trimmed)
        && (trimmed.contains('_')
            || trimmed.contains("::")
            || trimmed.contains('.')
            || trimmed.contains('$')
            || trimmed
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
            || trimmed.chars().skip(1).any(|ch| ch.is_ascii_uppercase()))
}

#[cfg(test)]
fn merge_hybrid_hits_by_node_id(hits: &mut Vec<HybridSearchHit>, additional: Vec<HybridSearchHit>) {
    let mut existing = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.node_id, index))
        .collect::<HashMap<_, _>>();

    for hit in additional {
        if let Some(index) = existing.get(&hit.node_id).copied() {
            let current = &mut hits[index];
            current.lexical_score = current.lexical_score.max(hit.lexical_score);
            current.semantic_score = current.semantic_score.max(hit.semantic_score);
            current.graph_score = current.graph_score.max(hit.graph_score);
            current.total_score = current.total_score.max(hit.total_score);
            continue;
        }

        existing.insert(hit.node_id, hits.len());
        hits.push(hit);
    }
}

#[cfg(test)]
fn hybrid_search_config_for_request(
    req: &SearchRequest,
    requested_max_results: usize,
    request_weights: Option<AgentHybridWeightsDto>,
    prefer_primary_sources: bool,
) -> HybridSearchConfig {
    let mut config = HybridSearchConfig {
        max_results: requested_max_results,
        ..HybridSearchConfig::default()
    };
    let has_request_weights = request_weights.is_some();
    let (lexical_weight, semantic_weight, graph_weight) =
        normalized_hybrid_weights(request_weights, &config);
    config.lexical_weight = lexical_weight;
    config.semantic_weight = semantic_weight;
    config.graph_weight = graph_weight;
    let has_exact_symbol_terms = !exact_symbol_query_terms(&req.query).is_empty();
    let mixed_nl = mixed_natural_language_query(&req.query);
    if !has_request_weights && has_exact_symbol_terms && !mixed_nl {
        config.lexical_weight = 0.85;
        config.semantic_weight = 0.15;
        config.graph_weight = 0.0;
        config.max_results = requested_max_results
            .saturating_mul(5)
            .clamp(requested_max_results, EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP);
        config.lexical_limit = config.lexical_limit.max(80);
        config.semantic_limit = config.semantic_limit.max(20);
    }
    apply_hybrid_limits(req.hybrid_limits.clone(), &mut config);
    if prefer_primary_sources && !has_exact_symbol_terms {
        config.max_results = requested_max_results.saturating_mul(5).min(80);
    }
    config
}

#[cfg(test)]
fn should_pretruncate_primary_source_window(
    query: &str,
    prefer_primary_sources: bool,
    candidate_count: usize,
    requested_max_results: usize,
) -> bool {
    prefer_primary_sources
        && exact_symbol_query_terms(query).is_empty()
        && candidate_count > requested_max_results
}

#[cfg(test)]
fn primary_source_retention_threshold(requested_max_results: usize) -> usize {
    requested_max_results.clamp(1, 3)
}

fn merge_search_hits_by_node_id(hits: &mut Vec<SearchHit>, additional: Vec<SearchHit>) {
    let mut existing = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.node_id.clone(), index))
        .collect::<HashMap<_, _>>();

    for hit in additional {
        if let Some(index) = existing.get(&hit.node_id).copied() {
            if hit.score > hits[index].score {
                hits[index] = hit;
            }
            continue;
        }

        existing.insert(hit.node_id.clone(), hits.len());
        hits.push(hit);
    }
}

fn search_plan_subquery_candidate_limit(subquery: &SearchPlanSubqueryDto, limit: usize) -> usize {
    if subquery.role == "original_question"
        && architecture_query_intents(&subquery.query).is_empty()
    {
        limit
    } else {
        limit.saturating_mul(5).clamp(limit, 50)
    }
}

#[cfg(test)]
fn truncate_repo_text_hits_for_query(query: &str, hits: &mut Vec<SearchHit>, limit: usize) {
    if limit == 0 {
        hits.clear();
        return;
    }
    if hits.len() <= limit {
        return;
    }
    if architecture_query_intents(query).is_empty() {
        hits.truncate(limit);
        return;
    }
    diversify_architecture_repo_text_hits(query, hits, limit);
}

#[cfg(test)]
fn diversify_architecture_repo_text_hits(query: &str, hits: &mut Vec<SearchHit>, limit: usize) {
    let query_terms = search_plan_terms(query)
        .extracted
        .into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut selected = hits
        .iter()
        .take(limit)
        .cloned()
        .enumerate()
        .collect::<Vec<_>>();

    for (candidate_rank, candidate) in hits.iter().enumerate().skip(limit) {
        let candidate_score = architecture_repo_text_surface_score(&query_terms, candidate);
        if candidate_score == 0 {
            continue;
        }
        let Some(candidate_key) = architecture_repo_text_surface_key(candidate) else {
            continue;
        };
        if selected.iter().any(|(_, hit)| {
            architecture_repo_text_surface_key(hit).as_ref() == Some(&candidate_key)
        }) {
            continue;
        }
        let Some(replace_index) =
            architecture_repo_text_replacement_index(&query_terms, &selected, candidate_score)
        else {
            continue;
        };
        selected[replace_index] = (candidate_rank, candidate.clone());
    }

    selected.sort_by_key(|(rank, _)| *rank);
    *hits = selected.into_iter().map(|(_, hit)| hit).collect();
}

#[cfg(test)]
fn architecture_repo_text_replacement_index(
    query_terms: &HashSet<String>,
    selected: &[(usize, SearchHit)],
    candidate_score: u32,
) -> Option<usize> {
    let mut bucket_counts = HashMap::<String, usize>::new();
    for (_, hit) in selected {
        if let Some(bucket) = architecture_repo_text_bucket_key(hit) {
            *bucket_counts.entry(bucket).or_default() += 1;
        }
    }

    selected
        .iter()
        .enumerate()
        .filter_map(|(index, (rank, hit))| {
            let bucket = architecture_repo_text_bucket_key(hit)?;
            if bucket_counts.get(&bucket).copied().unwrap_or_default() <= 1 {
                return None;
            }
            let score = architecture_repo_text_surface_score(query_terms, hit);
            let strong_distinct_surface = candidate_score >= 3 && score <= candidate_score + 1;
            (candidate_score >= score || strong_distinct_surface).then_some((index, score, *rank))
        })
        .min_by_key(|(_, score, rank)| (*score, std::cmp::Reverse(*rank)))
        .map(|(index, _, _)| index)
}

#[cfg(test)]
fn architecture_repo_text_surface_score(query_terms: &HashSet<String>, hit: &SearchHit) -> u32 {
    let Some(path) = hit.file_path.as_deref() else {
        return 0;
    };
    if retrieval_file_role_from_path(path).is_non_primary() {
        return 0;
    }
    architecture_repo_text_surface_terms(path)
        .into_iter()
        .filter(|term| query_terms.contains(*term))
        .count()
        .min(u32::MAX as usize) as u32
}

#[cfg(test)]
fn architecture_repo_text_surface_terms(path: &str) -> Vec<&'static str> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let path_terms = architecture_coverage_terms(path, "");
    let mut terms = Vec::new();
    if normalized.contains("sourcegroup")
        || normalized.contains("source_group")
        || normalized.contains("source-group")
        || architecture_has_all_terms(&path_terms, &["source", "group"])
    {
        terms.extend(["source", "group", "source-group", "configuration"]);
    }
    if architecture_has_all_terms(&path_terms, &["source", "group"])
        && architecture_has_any_term(&path_terms, &["cxx", "cdb", "compile", "database"])
    {
        terms.extend(["source", "group", "source-group", "cxx", "cdb", "indexing"]);
    }
    if normalized.contains("lib_cxx") || normalized.contains("/cxx/") {
        terms.extend(["cxx", "indexing", "indexer"]);
    }
    if normalized.contains("lib_java") || normalized.contains("/java/") {
        terms.extend(["java", "indexing", "indexer"]);
    }
    if normalized.contains("/data/indexer/") || normalized.contains("indexercommand") {
        terms.extend(["data", "indexer", "indexing", "command", "work"]);
    }
    if normalized.contains("/data/storage/")
        || architecture_has_all_terms(&path_terms, &["storage", "access"])
        || architecture_has_all_terms(&path_terms, &["persistent", "storage"])
    {
        terms.extend(["data", "storage", "access", "persistence"]);
    }
    if normalized.contains("/project/") {
        terms.extend(["project", "source", "group", "configuration"]);
    }
    if normalized.contains("codestory-cli") {
        terms.extend(["cli", "command", "entrypoint"]);
    }
    if normalized.contains("codestory-workspace") {
        terms.extend(["workspace", "file", "discovery", "source"]);
    }
    if normalized.contains("codestory-indexer") {
        terms.extend(["indexer", "indexing", "symbol", "extraction", "extract"]);
    }
    if normalized.contains("codestory-store") {
        terms.extend(["store", "storage", "persistence", "persist"]);
    }
    if normalized.contains("snapshot") {
        terms.extend(["snapshot", "refresh"]);
    }
    if normalized.contains("storage_impl") || normalized.contains("/storage_impl/") {
        terms.extend(["storage", "persistence", "projection", "sqlite"]);
    }
    if normalized.contains("/collections/") {
        terms.extend(["payload", "collection", "collections"]);
        if architecture_has_any_term(&path_terms, &["post", "posts"]) {
            terms.extend(["posts", "post", "writing"]);
        }
        if architecture_has_any_term(&path_terms, &["comment", "comments"]) {
            terms.extend(["comments", "comment"]);
        }
        if architecture_has_all_terms(&path_terms, &["social", "entries"]) {
            terms.extend(["social", "elsewhere", "feed"]);
        }
    }
    if normalized.ends_with("/lib/payload.ts") {
        terms.extend(["payload", "client"]);
    }
    if normalized.contains("/lib/content-data/") {
        terms.extend(["content"]);
        if normalized.ends_with("/post-content.ts") {
            terms.extend(["posts", "post", "writing"]);
        }
        if normalized.ends_with("/comment-content.ts") {
            terms.extend(["comments", "comment"]);
        }
        if normalized.ends_with("/social-entry-content.ts") {
            terms.extend(["social", "elsewhere", "feed"]);
        }
    }
    if normalized.ends_with("/app/feed.xml/route.ts") {
        terms.extend(["feed", "rss"]);
    }
    if normalized.ends_with("/posts/[slug]/comments/route.ts") {
        terms.extend(["posts", "comments", "comment", "submission"]);
    }
    if normalized.contains("codestory-runtime") {
        if normalized.ends_with("/lib.rs") {
            terms.extend(["runtime", "orchestration", "indexing"]);
        }
        if normalized.contains("/services.rs") {
            terms.extend(["runtime", "service", "orchestration", "indexing"]);
        }
        if normalized.contains("/search") || normalized.contains("search_runtime") {
            terms.extend(["search"]);
        }
        if normalized.contains("symbol_query") {
            terms.extend(["symbol", "search"]);
        }
        if normalized.contains("/agent/") || normalized.contains("orchestrator") {
            terms.extend(["orchestration"]);
        }
        if normalized.contains("graph") {
            terms.extend(["graph"]);
        }
    }
    terms
}

#[cfg(test)]
fn architecture_repo_text_surface_key(hit: &SearchHit) -> Option<String> {
    architecture_repo_text_path_key(hit, true)
}

#[cfg(test)]
fn architecture_repo_text_bucket_key(hit: &SearchHit) -> Option<String> {
    architecture_repo_text_path_key(hit, false)
}

#[cfg(test)]
fn architecture_repo_text_path_key(hit: &SearchHit, include_surface: bool) -> Option<String> {
    let path = normalize_repo_text_path(hit.file_path.as_deref()?);
    let parts = path.split('/').collect::<Vec<_>>();
    if let Some(crate_index) = parts.iter().position(|part| *part == "crates") {
        let crate_name = parts.get(crate_index + 1)?;
        if !include_surface {
            return Some(format!("crates/{crate_name}"));
        }
        let surface = parts
            .iter()
            .skip(crate_index + 2)
            .position(|part| *part == "src")
            .and_then(|src_offset| parts.get(crate_index + 3 + src_offset))
            .map(|part| part.trim_end_matches(".rs"))
            .unwrap_or("root");
        return Some(format!("crates/{crate_name}/{surface}"));
    }
    let src_index = parts.iter().position(|part| *part == "src")?;
    let root = parts.get(src_index + 1)?;
    let domain = parts.get(src_index + 2).copied().unwrap_or("root");
    if !include_surface {
        return Some(format!("src/{root}/{domain}"));
    }
    let stem = parts
        .last()
        .map(|part| part.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(part))
        .unwrap_or("root");
    Some(format!("src/{root}/{domain}/{stem}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchitectureCoverage {
    key: String,
    score: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchitectureCoverageLane {
    Indexed,
    RepoText,
}

#[derive(Debug, Clone)]
struct ArchitectureCoverageCandidate {
    lane: ArchitectureCoverageLane,
    source_rank: usize,
    hit: SearchHit,
    coverage: ArchitectureCoverage,
}

fn apply_architecture_cross_source_coverage(
    query: &str,
    indexed_symbol_hits: &mut Vec<SearchHit>,
    repo_text_hits: &mut Vec<SearchHit>,
    indexed_candidates: &[SearchHit],
    repo_text_candidates: &[SearchHit],
    limit: usize,
) {
    if limit == 0 || architecture_query_intents(query).is_empty() {
        return;
    }
    indexed_symbol_hits.truncate(limit);
    repo_text_hits.truncate(limit);

    let mut selected_ids = indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .map(|hit| hit.node_id.clone())
        .collect::<HashSet<_>>();
    let mut selected_paths = indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(|hit| hit.file_path.as_deref())
        .map(normalize_repo_text_path)
        .collect::<HashSet<_>>();
    let mut selected_keys =
        architecture_selected_coverage_keys(indexed_symbol_hits, repo_text_hits);

    let mut candidates = indexed_candidates
        .iter()
        .enumerate()
        .skip(limit)
        .filter_map(|(rank, hit)| {
            architecture_coverage_candidate(
                ArchitectureCoverageLane::Indexed,
                rank,
                hit,
                &selected_ids,
                &selected_paths,
                &selected_keys,
            )
        })
        .chain(
            repo_text_candidates
                .iter()
                .enumerate()
                .skip(limit)
                .filter_map(|(rank, hit)| {
                    architecture_coverage_candidate(
                        ArchitectureCoverageLane::RepoText,
                        rank,
                        hit,
                        &selected_ids,
                        &selected_paths,
                        &selected_keys,
                    )
                }),
        )
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .coverage
            .score
            .cmp(&left.coverage.score)
            .then_with(|| left.source_rank.cmp(&right.source_rank))
            .then_with(|| left.coverage.key.cmp(&right.coverage.key))
    });

    let mut replacement_count = 0usize;
    let replacement_limit = limit.min(8);
    for candidate in candidates {
        if selected_keys.contains(&candidate.coverage.key)
            || selected_ids.contains(&candidate.hit.node_id)
            || candidate
                .hit
                .file_path
                .as_deref()
                .map(normalize_repo_text_path)
                .is_some_and(|path| selected_paths.contains(&path))
        {
            continue;
        }
        let Some(replace_index) = architecture_coverage_replacement_index(
            indexed_symbol_hits,
            repo_text_hits,
            candidate.lane,
            candidate.coverage.score,
        ) else {
            continue;
        };

        match candidate.lane {
            ArchitectureCoverageLane::Indexed => {
                selected_ids.remove(&indexed_symbol_hits[replace_index].node_id);
                if let Some(path) = indexed_symbol_hits[replace_index].file_path.as_deref() {
                    selected_paths.remove(&normalize_repo_text_path(path));
                }
                indexed_symbol_hits[replace_index] = candidate.hit.clone();
            }
            ArchitectureCoverageLane::RepoText => {
                selected_ids.remove(&repo_text_hits[replace_index].node_id);
                if let Some(path) = repo_text_hits[replace_index].file_path.as_deref() {
                    selected_paths.remove(&normalize_repo_text_path(path));
                }
                repo_text_hits[replace_index] = candidate.hit.clone();
            }
        }
        selected_ids.insert(candidate.hit.node_id.clone());
        if let Some(path) = candidate.hit.file_path.as_deref() {
            selected_paths.insert(normalize_repo_text_path(path));
        }
        selected_keys.insert(candidate.coverage.key);
        replacement_count += 1;
        if replacement_count >= replacement_limit {
            break;
        }
    }
}

fn architecture_coverage_candidate(
    lane: ArchitectureCoverageLane,
    source_rank: usize,
    hit: &SearchHit,
    selected_ids: &HashSet<NodeId>,
    selected_paths: &HashSet<String>,
    selected_keys: &HashSet<String>,
) -> Option<ArchitectureCoverageCandidate> {
    if selected_ids.contains(&hit.node_id) {
        return None;
    }
    if hit
        .file_path
        .as_deref()
        .map(normalize_repo_text_path)
        .is_some_and(|path| selected_paths.contains(&path))
    {
        return None;
    }
    let coverage = architecture_coverage_for_hit(hit)?;
    (!selected_keys.contains(&coverage.key)).then(|| ArchitectureCoverageCandidate {
        lane,
        source_rank,
        hit: hit.clone(),
        coverage,
    })
}

fn architecture_selected_coverage_keys(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> HashSet<String> {
    indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(architecture_coverage_for_hit)
        .map(|coverage| coverage.key)
        .collect()
}

fn architecture_coverage_replacement_index(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    lane: ArchitectureCoverageLane,
    candidate_score: u32,
) -> Option<usize> {
    let key_counts = architecture_coverage_key_counts(indexed_symbol_hits, repo_text_hits);
    let hits = match lane {
        ArchitectureCoverageLane::Indexed => indexed_symbol_hits,
        ArchitectureCoverageLane::RepoText => repo_text_hits,
    };
    hits.iter()
        .enumerate()
        .filter_map(|(index, hit)| {
            let coverage = architecture_coverage_for_hit(hit);
            let score = coverage
                .as_ref()
                .map(|coverage| coverage.score)
                .unwrap_or(0);
            let protected = coverage.as_ref().is_some_and(|coverage| {
                coverage.score >= 8 && key_counts.get(&coverage.key).copied().unwrap_or(0) <= 1
            });
            if protected || score > candidate_score {
                return None;
            }
            Some((index, score))
        })
        .min_by_key(|(_, score)| *score)
        .map(|(index, _)| index)
}

fn architecture_coverage_key_counts(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for coverage in indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(architecture_coverage_for_hit)
    {
        *counts.entry(coverage.key).or_default() += 1;
    }
    counts
}

fn architecture_coverage_for_hit(hit: &SearchHit) -> Option<ArchitectureCoverage> {
    let path = hit.file_path.as_deref()?;
    if retrieval_file_role_from_path(path).is_non_primary() {
        return None;
    }
    let normalized = normalize_repo_text_path(path);
    let terms = architecture_coverage_terms(path, &hit.display_name);
    let source_kind = architecture_source_kind(&normalized);
    let coverage = if normalized.contains("/cli/src/main.rs") {
        ArchitectureCoverage {
            key: format!("cli:top_level_entrypoint:{source_kind}"),
            score: 8,
        }
    } else if normalized.contains("/exec/src/main.rs") {
        ArchitectureCoverage {
            key: format!("exec:binary_entrypoint:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/cli.rs") {
        ArchitectureCoverage {
            key: format!("exec:cli_options:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/exec/src/lib.rs") {
        ArchitectureCoverage {
            key: format!("exec:runtime:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["exec", "events"])
        && architecture_path_stem(&normalized).contains("events")
    {
        ArchitectureCoverage {
            key: format!("exec:events:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["event", "processor", "jsonl", "output"])
    {
        ArchitectureCoverage {
            key: format!("exec:jsonl_event_processor:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["event", "processor"])
    {
        ArchitectureCoverage {
            key: format!("exec:event_processor:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["source", "group"])
        && architecture_has_any_term(&terms, &["config", "cdb", "compile", "database", "cxx"])
    {
        ArchitectureCoverage {
            key: format!("source_group:configuration:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "command", "cxx"]) {
        ArchitectureCoverage {
            key: format!("indexing:cxx_command:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "java"]) {
        ArchitectureCoverage {
            key: format!("indexing:java:{source_kind}"),
            score: if source_kind == "impl" { 10 } else { 8 },
        }
    } else if architecture_has_all_terms(&terms, &["storage", "access", "proxy"]) {
        ArchitectureCoverage {
            key: format!("storage:access_proxy:{source_kind}"),
            score: if source_kind == "impl" { 10 } else { 8 },
        }
    } else if architecture_has_all_terms(&terms, &["persistent", "storage"]) {
        ArchitectureCoverage {
            key: format!("storage:persistent:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["storage", "access"]) {
        ArchitectureCoverage {
            key: format!("storage:access:{source_kind}"),
            score: 9,
        }
    } else if normalized.ends_with("/project.cpp")
        || architecture_has_all_terms(&terms, &["project", "build", "index"])
    {
        ArchitectureCoverage {
            key: format!("project:build_index:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "command"]) {
        ArchitectureCoverage {
            key: format!(
                "indexing:{}:{source_kind}",
                architecture_path_stem(&normalized)
            ),
            score: 4,
        }
    } else if normalized.contains("sourcegroup") && normalized.contains("/project/") {
        ArchitectureCoverage {
            key: format!(
                "source_group:{}:{source_kind}",
                architecture_path_stem(&normalized)
            ),
            score: 4,
        }
    } else if architecture_has_all_terms(&terms, &["payload", "config"])
        && normalized.ends_with(".ts")
    {
        ArchitectureCoverage {
            key: format!("payload:config:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_any_term(&terms, &["post", "posts"])
    {
        ArchitectureCoverage {
            key: format!("payload:posts_collection:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_any_term(&terms, &["comment", "comments"])
    {
        ArchitectureCoverage {
            key: format!("payload:comments_collection:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_all_terms(&terms, &["social", "entries"])
    {
        ArchitectureCoverage {
            key: format!("payload:social_entries_collection:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/posts/")
        && normalized.contains("/comments/")
        && architecture_has_all_terms(&terms, &["comments", "route"])
    {
        ArchitectureCoverage {
            key: format!("comments:submission_route:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["feed", "route"]) {
        ArchitectureCoverage {
            key: format!("feed:rss_route:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/lib/")
        && architecture_has_all_terms(&terms, &["payload"])
        && architecture_has_any_term(&terms, &["client", "lib"])
    {
        ArchitectureCoverage {
            key: format!("payload:client:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["post", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:post_data:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["comment", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:comment_data:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["social", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:social_data:{source_kind}"),
            score: 9,
        }
    } else {
        return None;
    };
    Some(coverage)
}

fn architecture_coverage_terms(path: &str, display_name: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    for raw in [path, display_name] {
        for fragment in raw.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if fragment.is_empty() {
                continue;
            }
            terms.insert(fragment.to_ascii_lowercase());
            for camel_part in split_camel_identifier(fragment) {
                terms.insert(camel_part);
            }
        }
    }
    terms
}

fn architecture_has_all_terms(terms: &HashSet<String>, required: &[&str]) -> bool {
    required.iter().all(|term| terms.contains(*term))
}

fn architecture_has_any_term(terms: &HashSet<String>, required: &[&str]) -> bool {
    required.iter().any(|term| terms.contains(*term))
}

fn architecture_source_kind(path: &str) -> &'static str {
    if path.ends_with(".h") || path.ends_with(".hpp") || path.ends_with(".hh") {
        "decl"
    } else if path.ends_with(".cpp")
        || path.ends_with(".cxx")
        || path.ends_with(".cc")
        || path.ends_with(".c")
        || path.ends_with(".rs")
        || path.ends_with(".ts")
        || path.ends_with(".tsx")
        || path.ends_with(".js")
        || path.ends_with(".jsx")
    {
        "impl"
    } else {
        "file"
    }
}

fn architecture_path_stem(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.').map(|(stem, _)| stem))
        .unwrap_or(path)
}

fn normalize_repo_text_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn dedupe_inexact_search_hits_by_display_key(query: &str, hits: &mut Vec<SearchHit>) {
    let mut seen = HashSet::<(String, NodeKind, Option<String>)>::new();
    hits.retain(|hit| {
        let rank = symbol_name_match_rank(query, &hit.display_name);
        let is_exact_match =
            rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0;
        if is_exact_match {
            return true;
        }

        seen.insert((hit.display_name.clone(), hit.kind, hit.file_path.clone()))
    });
}

fn did_you_mean_suggestions(scored_hits: &[HybridSearchScoredHit]) -> Vec<SearchHit> {
    const MIN_SEMANTIC_SCORE: f32 = 0.18;
    const MAX_SUGGESTIONS: usize = 5;

    if scored_hits.is_empty()
        || scored_hits
            .iter()
            .any(|hit| hit.lexical_score > 0.01 || hit.graph_score > 0.25)
    {
        return Vec::new();
    }

    scored_hits
        .iter()
        .filter(|hit| hit.semantic_score >= MIN_SEMANTIC_SCORE)
        .take(MAX_SUGGESTIONS)
        .map(|hit| hit.hit.clone())
        .collect()
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
struct HybridSearchInstrumentation {
    symbol_table_size: usize,
    exact_symbol_merge_queries: usize,
    hybrid_max_results: usize,
    hybrid_lexical_limit: usize,
    hybrid_semantic_limit: usize,
    mixed_natural_language: bool,
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

    fn build_search_hit(
        storage: &Storage,
        node_names: &HashMap<codestory_contracts::graph::NodeId, String>,
        id: codestory_contracts::graph::NodeId,
        score: f32,
    ) -> Result<Option<SearchHit>, ApiError> {
        let node = match storage.get_node(id) {
            Ok(Some(node)) if node.kind != codestory_contracts::graph::NodeKind::UNKNOWN => node,
            _ => return Ok(None),
        };

        let display_name = node_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| node_display_name(&node));

        let mut file_path = Self::file_path_for_node(storage, &node).ok().flatten();
        let mut line = node.start_line;
        if let Ok(occs) = storage.get_occurrences_for_node(id)
            && let Some(occ) = preferred_occurrence(&occs)
        {
            if file_path.is_none()
                && let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id)
            {
                file_path = Some(file_node.serialized_name);
            }
            if line.is_none() {
                line = Some(occ.location.start_line);
            }
        }

        let openapi_endpoint = node
            .canonical_id
            .as_deref()
            .is_some_and(|value| value.starts_with("openapi:endpoint:"));
        let structural_unit = storage.get_structural_text_unit(id).map_err(|error| {
            ApiError::internal(format!(
                "Failed to load structural provenance for node {}: {error}",
                id.0
            ))
        })?;

        let mut hit = SearchHit {
            node_id: NodeId::from(id),
            display_name,
            kind: NodeKind::from(node.kind),
            file_path,
            line,
            score: route_endpoint_adjusted_search_score(score, node.canonical_id.as_deref()),
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: Some(if structural_unit.is_some() {
                codestory_contracts::api::PacketEvidenceTierDto::StructuralText
            } else if openapi_endpoint {
                codestory_contracts::api::PacketEvidenceTierDto::ExactSource
            } else {
                codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph
            }),
            evidence_producer: Some(if let Some(unit) = structural_unit.as_ref() {
                unit.producer.clone()
            } else {
                if openapi_endpoint {
                    "openapi_endpoint_schema"
                } else {
                    "route_endpoint"
                }
                .to_string()
            }),
            resolution_status: Some(if structural_unit.is_some() || openapi_endpoint {
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly
            } else {
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved
            }),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(structural_unit.is_none() && !openapi_endpoint),
            score_breakdown: None,
        };
        decorate_search_hit_evidence(&mut hit);
        Ok(Some(hit))
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn repo_text_enabled_for_mode(
        mode: SearchRepoTextMode,
        query: &str,
        indexed_hits: &[SearchHit],
    ) -> bool {
        match mode {
            SearchRepoTextMode::Auto => {
                looks_like_repo_text_query(query)
                    || repo_text_auto_fallback_reason(query, indexed_hits).is_some()
            }
            SearchRepoTextMode::On => true,
            SearchRepoTextMode::Off => false,
        }
    }

    fn collect_repo_text_hits(
        storage: &Storage,
        project_root: Option<&Path>,
        query: &str,
        limit: usize,
        indexed_hit_ids: &HashSet<NodeId>,
    ) -> Result<RepoTextScan, ApiError> {
        let started_at = Instant::now();
        let mut stats = RepoTextScanStatsDto {
            scanned_file_count: 0,
            scanned_byte_count: 0,
            skipped_large_file_count: 0,
            file_cap: REPO_TEXT_SCAN_FILE_CAP as u32,
            byte_cap: REPO_TEXT_SCAN_BYTE_CAP as u32,
            time_cap_ms: REPO_TEXT_SCAN_TIME_CAP_MS as u32,
            duration_ms: 0,
            truncated: false,
            reason: None,
            action: None,
        };
        if query.trim().is_empty() || limit == 0 {
            return Ok(RepoTextScan {
                hits: Vec::new(),
                #[cfg(test)]
                stats,
            });
        }

        let mut hits = Vec::new();
        let mut seen = indexed_hit_ids.clone();
        let terms = extract_symbol_search_terms(query);
        let normalized_query = query.trim().to_ascii_lowercase();
        for file in storage
            .get_files_ordered_limit(REPO_TEXT_SCAN_FILE_CAP.saturating_add(1))
            .map_err(|e| ApiError::internal(format!("Failed to load files for text search: {e}")))?
        {
            if Self::repo_text_scan_should_stop(&mut stats, &started_at) {
                break;
            }

            let path_string = file.path.to_string_lossy().to_string();
            stats.scanned_file_count = stats.scanned_file_count.saturating_add(1);
            let Ok(metadata) = std::fs::metadata(&file.path) else {
                continue;
            };
            if metadata.len() > REPO_TEXT_MAX_FILE_BYTES {
                stats.skipped_large_file_count = stats.skipped_large_file_count.saturating_add(1);
                continue;
            }
            let projected_bytes =
                u64::from(stats.scanned_byte_count).saturating_add(metadata.len());
            if projected_bytes > REPO_TEXT_SCAN_BYTE_CAP as u64 {
                Self::mark_repo_text_scan_truncated(
                    &mut stats,
                    format!(
                        "repo-text scan stopped before reading more than {} bytes",
                        REPO_TEXT_SCAN_BYTE_CAP
                    ),
                );
                break;
            }
            let Some(contents) = read_searchable_file_contents(&path_string) else {
                continue;
            };
            if contents.len() as u64 > REPO_TEXT_MAX_FILE_BYTES {
                stats.skipped_large_file_count = stats.skipped_large_file_count.saturating_add(1);
                continue;
            }
            stats.scanned_byte_count = stats
                .scanned_byte_count
                .saturating_add(clamp_usize_to_u32(contents.len()));
            let Some(line) = Self::repo_text_match_line(&contents, &path_string, query, &terms)
            else {
                continue;
            };
            let node_id = NodeId::from(codestory_contracts::graph::NodeId(file.id));
            if !seen.insert(node_id.clone()) {
                continue;
            }

            let display_name =
                Self::repo_text_display_name(project_root, &file.path, path_string.as_str());
            let score = Self::repo_text_score(
                &contents,
                &path_string,
                &normalized_query,
                &terms,
                line,
                hits.len(),
            );
            hits.push(Self::repo_text_search_hit(
                node_id,
                display_name,
                path_string,
                line,
                score,
            ));
        }

        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root, query, left, right)
        });
        hits.truncate(limit);
        stats.duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
        Ok(RepoTextScan {
            hits,
            #[cfg(test)]
            stats,
        })
    }

    fn repo_text_scan_should_stop(stats: &mut RepoTextScanStatsDto, started_at: &Instant) -> bool {
        if (stats.scanned_file_count as usize) >= REPO_TEXT_SCAN_FILE_CAP {
            Self::mark_repo_text_scan_truncated(
                stats,
                format!(
                    "repo-text scan stopped after scanning {} files",
                    REPO_TEXT_SCAN_FILE_CAP
                ),
            );
            return true;
        }
        if started_at.elapsed().as_millis() > REPO_TEXT_SCAN_TIME_CAP_MS {
            Self::mark_repo_text_scan_truncated(
                stats,
                format!(
                    "repo-text scan stopped after {} ms",
                    REPO_TEXT_SCAN_TIME_CAP_MS
                ),
            );
            return true;
        }
        false
    }

    fn repo_text_display_name(
        project_root: Option<&Path>,
        file_path: &Path,
        fallback: &str,
    ) -> String {
        project_root
            .and_then(|root| file_path.strip_prefix(root).ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .or_else(|| {
                file_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| fallback.to_string())
    }

    fn repo_text_match_line(
        contents: &str,
        path: &str,
        query: &str,
        terms: &[String],
    ) -> Option<u32> {
        if let Some(line) = file_text_match_line(contents, query, terms) {
            return Some(line);
        }
        if terms.is_empty() {
            return None;
        }

        let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
        let mut distinct_hit_terms = HashSet::new();
        for (term_index, term) in terms.iter().enumerate() {
            if text_contains_query_term(&normalized_path, term) {
                distinct_hit_terms.insert(term_index);
            }
        }
        let path_has_term = !distinct_hit_terms.is_empty();

        let mut best_line = None;
        let mut best_score = 0usize;
        for (index, line) in contents.lines().enumerate() {
            let normalized_line = line.to_ascii_lowercase();
            let mut line_score = 0usize;
            for (term_index, term) in terms.iter().enumerate() {
                if text_contains_query_term(&normalized_line, term) {
                    distinct_hit_terms.insert(term_index);
                    line_score += 1;
                }
            }
            if line_score > best_score {
                best_score = line_score;
                best_line = Some((index + 1).min(u32::MAX as usize) as u32);
            }
        }

        let required_hits = if path_has_term { 2 } else { 3.min(terms.len()) };
        if distinct_hit_terms.len() < required_hits {
            return None;
        }

        best_line.or(Some(1))
    }

    fn repo_text_term_hits(text: &str, terms: &[String]) -> f32 {
        terms
            .iter()
            .filter(|term| text_contains_query_term(text, term))
            .count() as f32
    }

    fn repo_text_score(
        contents: &str,
        path: &str,
        normalized_query: &str,
        terms: &[String],
        line: u32,
        hit_index: usize,
    ) -> f32 {
        let normalized_contents = contents.to_ascii_lowercase();
        let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
        let line_text = contents
            .lines()
            .nth(line.saturating_sub(1) as usize)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let exact_line_match = !normalized_query.is_empty() && line_text.contains(normalized_query);
        let exact_file_match =
            !normalized_query.is_empty() && normalized_contents.contains(normalized_query);
        let path_term_hits = Self::repo_text_term_hits(&normalized_path, terms);
        let line_term_hits = Self::repo_text_term_hits(&line_text, terms);
        let file_term_hits = Self::repo_text_term_hits(&normalized_contents, terms);

        let mut score = 100.0;
        if exact_line_match {
            score += 220.0;
        } else if exact_file_match {
            score += 140.0;
        }
        score += path_term_hits * 35.0;
        score += line_term_hits * 28.0;
        score += file_term_hits * 6.0;
        score - (hit_index as f32 * 0.01)
    }

    fn repo_text_search_hit(
        node_id: NodeId,
        display_name: String,
        path_string: String,
        line: u32,
        score: f32,
    ) -> SearchHit {
        SearchHit {
            node_id,
            display_name,
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some(path_string),
            line: Some(line),
            score,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            match_quality: Some(SearchMatchQualityDto::RepoText),
            resolvable: false,
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::LexicalSource),
            evidence_producer: Some("repo_text_fallback".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            score_breakdown: None,
        }
    }

    fn mark_repo_text_scan_truncated(stats: &mut RepoTextScanStatsDto, reason: String) {
        stats.truncated = true;
        stats.reason = Some(reason);
        stats.action = Some(
            "Narrow the query or use indexed symbol search with repo_text=off for deterministic results."
                .to_string(),
        );
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

    #[allow(clippy::too_many_arguments)]
    fn execute_search_plan_subqueries(
        &self,
        storage: &Storage,
        subqueries: &[SearchPlanSubqueryDto],
        filters: &[SearchIntentFilter],
        indexed_hit_ids: &HashSet<NodeId>,
        limit_per_source: usize,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
        semantic_ready: bool,
    ) -> Result<SearchPlanExecutedEvidence, ApiError> {
        let mut evidence = SearchPlanExecutedEvidence::default();
        let project_root = self.require_project_root().ok();
        let mut seen_repo_text_ids = indexed_hit_ids.clone();

        for subquery in subqueries {
            let uses_indexed = subquery.channels.iter().any(|channel| {
                matches!(
                    channel,
                    SearchPlanChannelDto::TypedSymbol
                        | SearchPlanChannelDto::Lexical
                        | SearchPlanChannelDto::Semantic
                )
            });
            if uses_indexed {
                let subquery_limit =
                    search_plan_subquery_candidate_limit(subquery, limit_per_source);
                let mut scored_hits = self.search_hybrid_scored(
                    SearchRequest {
                        query: subquery.query.clone(),
                        repo_text: SearchRepoTextMode::Off,
                        limit_per_source: subquery_limit as u32,
                        expand_search_plan: false,
                        hybrid_weights: hybrid_weights.clone(),
                        hybrid_limits: hybrid_limits.clone(),
                    },
                    None,
                    subquery_limit,
                    hybrid_weights.clone(),
                )?;
                let mut suggestions = did_you_mean_suggestions(&scored_hits);
                let mut hits = scored_hits
                    .drain(..)
                    .map(|scored| scored.hit)
                    .collect::<Vec<_>>();
                if should_expand_symbol_query(&subquery.query, hits.len()) {
                    let expanded_hits = self.expanded_symbol_hits(storage, &subquery.query)?;
                    merge_search_hits_by_node_id(&mut hits, expanded_hits);
                }
                apply_search_intent_filters(&mut hits, filters);
                apply_search_intent_filters(&mut suggestions, filters);
                hits.sort_by(|left, right| {
                    compare_search_hits_with_project_root(
                        project_root.as_deref(),
                        &subquery.query,
                        left,
                        right,
                    )
                });
                suggestions.sort_by(|left, right| {
                    compare_search_hits_with_project_root(
                        project_root.as_deref(),
                        &subquery.query,
                        left,
                        right,
                    )
                });
                dedupe_inexact_search_hits_by_display_key(&subquery.query, &mut hits);
                hits.truncate(subquery_limit);
                suggestions.truncate(subquery_limit);
                annotate_search_hit_match_quality(&subquery.query, &mut hits);
                annotate_search_hit_match_quality(&subquery.query, &mut suggestions);
                evidence
                    .candidate_windows
                    .extend(search_plan_symbol_windows(
                        subquery,
                        &hits,
                        &suggestions,
                        subquery_limit as u32,
                        semantic_ready,
                    ));
                merge_search_hits_by_node_id(&mut evidence.indexed_symbol_hits, hits);
                merge_search_hits_by_node_id(&mut evidence.suggestions, suggestions);
            }

            if subquery.channels.contains(&SearchPlanChannelDto::RepoText) {
                let repo_text_limit =
                    search_plan_subquery_candidate_limit(subquery, limit_per_source);
                let scan = Self::collect_repo_text_hits(
                    storage,
                    project_root.as_deref(),
                    &subquery.query,
                    repo_text_limit,
                    &seen_repo_text_ids,
                )?;
                let mut hits = scan.hits;
                apply_search_intent_filters(&mut hits, filters);
                annotate_search_hit_match_quality(&subquery.query, &mut hits);
                for hit in &hits {
                    seen_repo_text_ids.insert(hit.node_id.clone());
                }
                evidence
                    .candidate_windows
                    .push(SearchPlanCandidateWindowDto {
                        channel: SearchPlanChannelDto::RepoText,
                        subquery: subquery.query.clone(),
                        limit: repo_text_limit as u32,
                        returned_count: clamp_usize_to_u32(hits.len()),
                        truncated: hits.len() >= repo_text_limit,
                        score_reasons: vec![
                            "executed repo-text retrieval for this planned subquery; hits require promotion or source reads"
                                .to_string(),
                        ],
                    });
                merge_search_hits_by_node_id(&mut evidence.repo_text_hits, hits);
            }
        }

        Ok(evidence)
    }

    fn search_plan_active_path_evidence<'a, I>(
        &self,
        storage: &Storage,
        hits: I,
    ) -> HashMap<NodeId, SearchPlanActivePathEvidence>
    where
        I: IntoIterator<Item = &'a SearchHit>,
    {
        let mut evidence = HashMap::new();
        for hit in hits {
            if evidence.contains_key(&hit.node_id) {
                continue;
            }
            if let Some(active_path) = self.search_plan_active_path_evidence_for_hit(storage, hit) {
                evidence.insert(hit.node_id.clone(), active_path);
            }
        }
        evidence
    }

    fn search_plan_active_path_evidence_for_hit(
        &self,
        storage: &Storage,
        hit: &SearchHit,
    ) -> Option<SearchPlanActivePathEvidence> {
        if !search_plan_callable_hit(hit) {
            return None;
        }
        let node_id = hit.node_id.to_core().ok()?;
        let edges = storage.get_edges_for_node_id(node_id).ok()?;
        let mut callers = HashSet::new();
        for edge in edges {
            if edge.kind != codestory_contracts::graph::EdgeKind::CALL {
                continue;
            }
            if search_plan_runtime_call_is_speculative(edge.certainty, edge.confidence) {
                continue;
            }
            let (source, target) = edge.effective_endpoints();
            if target != node_id || source == node_id {
                continue;
            }
            if search_plan_caller_is_test_or_bench(storage, source) {
                continue;
            }
            callers.insert(source);
        }

        Some(SearchPlanActivePathEvidence {
            caller_count: callers.len().min(u32::MAX as usize) as u32,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_search_plan(
        &self,
        storage: &Storage,
        original_query: &str,
        effective_query: &str,
        query_assessment: &SearchQueryAssessmentDto,
        indexed_symbol_hits: &[SearchHit],
        repo_text_hits: &[SearchHit],
        suggestions: &[SearchHit],
        retrieval: &RetrievalStateDto,
        limit_per_source: u32,
        filters: &[SearchIntentFilter],
        indexed_hit_ids: &HashSet<NodeId>,
        allow_repo_text: bool,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
    ) -> Result<Option<SearchPlanBuild>, ApiError> {
        let intents = architecture_query_intents(effective_query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        let eligible = search_plan_eligible(
            effective_query,
            query_assessment.exact_symbol_hit_count,
            &intents,
        );
        if !eligible {
            return Ok(None);
        }
        let terms = search_plan_terms(effective_query);
        let subqueries = search_plan_subqueries_for_repo_text_mode(
            search_plan_subqueries(effective_query, &terms, &intents),
            allow_repo_text,
        );
        let mut executed = self.execute_search_plan_subqueries(
            storage,
            &subqueries,
            filters,
            indexed_hit_ids,
            limit_per_source as usize,
            hybrid_weights,
            hybrid_limits,
            retrieval.semantic_ready,
        )?;
        let mut plan_indexed_hits = indexed_symbol_hits.to_vec();
        merge_search_hits_by_node_id(&mut plan_indexed_hits, executed.indexed_symbol_hits.clone());
        let mut plan_repo_text_hits = repo_text_hits.to_vec();
        merge_search_hits_by_node_id(&mut plan_repo_text_hits, executed.repo_text_hits.clone());
        let mut plan_suggestions = suggestions.to_vec();
        merge_search_hits_by_node_id(&mut plan_suggestions, executed.suggestions.clone());
        let active_path_evidence = self.search_plan_active_path_evidence(
            storage,
            plan_indexed_hits.iter().chain(plan_suggestions.iter()),
        );
        let anchor_groups = search_plan_anchor_groups(
            effective_query,
            &terms,
            &plan_indexed_hits,
            &plan_repo_text_hits,
            &plan_suggestions,
            &active_path_evidence,
        );
        let mut bridges = self.search_plan_bridges(&anchor_groups);
        let original_bridge_count = bridges.len();
        bridges.retain(|bridge| !is_low_confidence_search_plan_bridge(bridge));
        let suppressed_low_confidence_bridges = original_bridge_count.saturating_sub(bridges.len());
        if !bridges.is_empty() {
            executed
                .candidate_windows
                .push(SearchPlanCandidateWindowDto {
                    channel: SearchPlanChannelDto::Bridge,
                    subquery: "selected_anchor_bridges".to_string(),
                    limit: anchor_groups.len().saturating_sub(1).min(u32::MAX as usize) as u32,
                    returned_count: clamp_usize_to_u32(bridges.len()),
                    truncated: false,
                    score_reasons: vec![
                        "bridge evidence expands only after anchor grouping".to_string(),
                    ],
                });
        }
        let mut source_truth_checks = search_plan_source_truth_checks(&anchor_groups);
        if suppressed_low_confidence_bridges > 0 {
            source_truth_checks.push(format!(
                "Suppressed {suppressed_low_confidence_bridges} low-confidence bridge candidate(s); treat missing bridge rows as a source-truth prompt, not proof of isolation."
            ));
        }
        let next_actions = search_plan_next_actions(&anchor_groups);
        let plan = SearchPlanDto {
            original_query: original_query.to_string(),
            eligible,
            intents,
            terms,
            subqueries,
            candidate_windows: executed.candidate_windows,
            anchor_groups: anchor_groups.clone(),
            bridges,
            rejected_hits: search_plan_rejected_hits(
                &anchor_groups,
                &plan_suggestions,
                &plan_indexed_hits,
                &plan_repo_text_hits,
            ),
            next_actions,
            source_truth_checks,
        };
        Ok(Some(SearchPlanBuild {
            plan,
            indexed_symbol_hits: executed.indexed_symbol_hits,
        }))
    }

    fn search_plan_bridges(&self, groups: &[SearchPlanAnchorGroupDto]) -> Vec<SearchPlanBridgeDto> {
        let selected = groups
            .iter()
            .filter_map(|group| group.chosen_symbol.as_ref().map(|hit| (group, hit)))
            .take(5)
            .collect::<Vec<_>>();
        selected
            .windows(2)
            .map(|pair| {
                let (from_group, from_hit) = pair[0];
                let (to_group, to_hit) = pair[1];
                let mut notes = Vec::new();
                if from_hit.node_id == to_hit.node_id {
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Supported,
                        confidence: SearchPlanBridgeConfidenceDto::High,
                        evidence_kind: SearchPlanBridgeEvidenceKindDto::SameAnchor,
                        direction: Some("self".to_string()),
                        node_count: 1,
                        edge_count: 0,
                        truncated: false,
                        notes,
                    };
                }
                let forward = self.graph_trail(search_plan_bridge_request(
                    &from_hit.node_id,
                    &to_hit.node_id,
                ));
                if let Ok(graph) = forward
                    && graph_response_has_bridge(&graph, &from_hit.node_id, &to_hit.node_id)
                {
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Supported,
                        confidence: if graph.truncated {
                            SearchPlanBridgeConfidenceDto::Medium
                        } else {
                            SearchPlanBridgeConfidenceDto::High
                        },
                        evidence_kind: graph_bridge_evidence_kind(&graph),
                        direction: Some("forward".to_string()),
                        node_count: clamp_usize_to_u32(graph.nodes.len()),
                        edge_count: clamp_usize_to_u32(graph.edges.len()),
                        truncated: graph.truncated,
                        notes,
                    };
                }
                let reverse = self.graph_trail(search_plan_bridge_request(
                    &to_hit.node_id,
                    &from_hit.node_id,
                ));
                if let Ok(graph) = reverse
                    && graph_response_has_bridge(&graph, &to_hit.node_id, &from_hit.node_id)
                {
                    notes.push(
                        "reverse graph path found; direction is partial evidence for the original flow"
                            .to_string(),
                    );
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Partial,
                        confidence: if graph.truncated {
                            SearchPlanBridgeConfidenceDto::Low
                        } else {
                            SearchPlanBridgeConfidenceDto::Medium
                        },
                        evidence_kind: graph_bridge_evidence_kind(&graph),
                        direction: Some("reverse".to_string()),
                        node_count: clamp_usize_to_u32(graph.nodes.len()),
                        edge_count: clamp_usize_to_u32(graph.edges.len()),
                        truncated: graph.truncated,
                        notes,
                    };
                }
                if shared_file_bridge(from_hit, to_hit) {
                    notes.push(
                        "anchors share a source file; this is low-confidence bridge evidence only"
                            .to_string(),
                    );
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Partial,
                        confidence: SearchPlanBridgeConfidenceDto::Low,
                        evidence_kind: SearchPlanBridgeEvidenceKindDto::SharedFile,
                        direction: None,
                        node_count: 2,
                        edge_count: 0,
                        truncated: false,
                        notes,
                    };
                }
                notes.push(
                    "no graph path or shared-file bridge found inside the bounded evidence budget"
                        .to_string(),
                );
                SearchPlanBridgeDto {
                    from_anchor: from_group.anchor.clone(),
                    to_anchor: to_group.anchor.clone(),
                    status: SearchPlanBridgeStatusDto::Unsupported,
                    confidence: SearchPlanBridgeConfidenceDto::Low,
                    evidence_kind: SearchPlanBridgeEvidenceKindDto::IsolatedAnchors,
                    direction: None,
                    node_count: 2,
                    edge_count: 0,
                    truncated: false,
                    notes,
                }
            })
            .collect()
    }

    /// Run sidecar-primary search and return only the hit list.
    ///
    /// Use [`AppController::search_results`] when the caller needs retrieval mode, diagnostics,
    /// or search-plan metadata.
    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        Ok(self.search_results(req)?.hits)
    }

    /// Run sidecar-primary search with retrieval state metadata.
    ///
    /// The returned retrieval state distinguishes full sidecar evidence from degraded or
    /// diagnostic-only paths. Callers should not collapse those states into a generic success.
    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "search output", || {
            self.search_results_once(req.clone())
        })
    }

    fn search_results_once(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.ensure_consistent_read_state("Search")?;
        let original_query = req.query.clone();
        let intent_query = parse_search_intent_query(&original_query);
        let limit_per_source = req.limit_per_source.clamp(1, 50) as usize;
        let repo_text_mode = req.repo_text;
        self.search_results_sidecar_primary(
            original_query,
            intent_query,
            limit_per_source,
            repo_text_mode,
            req.expand_search_plan,
            req.hybrid_weights.clone(),
            req.hybrid_limits.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn search_results_sidecar_primary(
        &self,
        original_query: String,
        intent_query: SearchIntentQuery,
        limit_per_source: usize,
        repo_text_mode: SearchRepoTextMode,
        expand_search_plan: bool,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
    ) -> Result<SearchResultsDto, ApiError> {
        if !agent::retrieval_primary::sidecar_retrieval_primary_enabled(self) {
            let reason = agent::retrieval_primary::sidecar_retrieval_unavailable_reason(self)
                .unwrap_or_else(|| {
                    "full retrieval is mandatory; legacy search is disabled".to_string()
                });
            return Err(
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(self, reason),
            );
        }

        let query = intent_query.effective_query.clone();
        let (query_result, resolution) = agent::retrieval_primary::run_and_resolve_sidecar_query(
            self,
            &query,
            limit_per_source,
            None,
        )?;
        let mut indexed_symbol_hits = resolution.resolved_hits.clone();
        if let Some(reason) = agent::retrieval_primary::sidecar_primary_result_rejection_reason(
            &query_result,
            &indexed_symbol_hits,
        ) {
            let diagnostic = agent::retrieval_primary::sidecar_rejection_diagnostic(
                self,
                &query_result,
                &indexed_symbol_hits,
                5,
            );
            return Err(
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(
                    self,
                    format!("sidecar search rejected query: {reason}; {diagnostic}"),
                ),
            );
        }
        let initial_sidecar_hits = indexed_symbol_hits.clone();

        apply_search_intent_filters(&mut indexed_symbol_hits, &intent_query.filters);
        let project_root = self.require_project_root().ok();
        indexed_symbol_hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root.as_deref(), &query, left, right)
        });
        dedupe_inexact_search_hits_by_display_key(&query, &mut indexed_symbol_hits);
        indexed_symbol_hits.truncate(limit_per_source);
        annotate_search_hit_match_quality(&query, &mut indexed_symbol_hits);

        let storage = self.open_storage_read_only()?;
        let retrieval = retrieval_state_from_storage_for_runtime(&storage, &self.runtime_config)?;
        let freshness = self.index_freshness().ok();
        let mut repo_text_hits = Vec::new();
        let suggestions = Vec::new();
        let query_assessment = search_query_assessment(
            &query,
            &indexed_symbol_hits,
            &repo_text_hits,
            repo_text_mode,
            false,
            None,
        );
        let indexed_hit_ids = indexed_symbol_hits
            .iter()
            .map(|hit| hit.node_id.clone())
            .collect::<HashSet<_>>();
        let mut search_plan_anchor_rank = HashMap::<NodeId, usize>::new();
        let search_plan = if expand_search_plan {
            match self.build_search_plan(
                &storage,
                &original_query,
                &query,
                &query_assessment,
                &indexed_symbol_hits,
                &repo_text_hits,
                &suggestions,
                &retrieval,
                limit_per_source as u32,
                &intent_query.filters,
                &indexed_hit_ids,
                false,
                hybrid_weights,
                hybrid_limits,
            )? {
                Some(plan_build) => {
                    for (rank, group) in plan_build.plan.anchor_groups.iter().enumerate() {
                        if let Some(symbol) = &group.chosen_symbol {
                            search_plan_anchor_rank
                                .entry(symbol.node_id.clone())
                                .or_insert(rank);
                            merge_search_hits_by_node_id(
                                &mut indexed_symbol_hits,
                                vec![symbol.clone()],
                            );
                        }
                    }
                    merge_search_hits_by_node_id(
                        &mut indexed_symbol_hits,
                        plan_build.indexed_symbol_hits,
                    );
                    Some(plan_build.plan)
                }
                None => None,
            }
        } else {
            None
        };
        indexed_symbol_hits.sort_by(|left, right| {
            let anchor_order = match (
                search_plan_anchor_rank.get(&left.node_id),
                search_plan_anchor_rank.get(&right.node_id),
            ) {
                (Some(left_rank), Some(right_rank)) => left_rank.cmp(right_rank),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            anchor_order.then_with(|| {
                compare_search_hits_with_project_root(project_root.as_deref(), &query, left, right)
            })
        });
        dedupe_inexact_search_hits_by_display_key(&query, &mut indexed_symbol_hits);
        let indexed_symbol_candidates = indexed_symbol_hits.clone();
        apply_architecture_cross_source_coverage(
            &query,
            &mut indexed_symbol_hits,
            &mut repo_text_hits,
            &indexed_symbol_candidates,
            &[],
            limit_per_source,
        );
        indexed_symbol_hits.truncate(limit_per_source);
        annotate_search_hit_match_quality(&query, &mut indexed_symbol_hits);
        let hits = indexed_symbol_hits.clone();
        let retrieval_shadow = Some(
            agent::retrieval_primary::shadow_from_query_result_with_candidate_admission_diagnostics(
                self,
                query_result.clone(),
                &resolution,
                &initial_sidecar_hits,
                &hits,
            ),
        );

        Ok(SearchResultsDto {
            query: original_query,
            retrieval_publication: None,
            retrieval,
            retrieval_shadow,
            freshness,
            limit_per_source: limit_per_source as u32,
            repo_text_mode,
            repo_text_enabled: false,
            query_assessment: Some(query_assessment),
            search_plan,
            repo_text_stats: None,
            suggestions,
            indexed_symbol_hits,
            repo_text_hits,
            hits,
        })
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

    #[cfg(test)]
    fn is_repo_explanation_search_query(query: &str) -> bool {
        let lower = query.to_ascii_lowercase();
        let subject =
            lower.contains("repo") || lower.contains("repository") || lower.contains("codebase");
        let intent = lower.contains("fit together")
            || lower.contains("how does")
            || lower.contains("explain")
            || lower.contains("overview")
            || lower.contains("architecture");
        subject && intent
    }

    fn expanded_symbol_hits(
        &self,
        storage: &Storage,
        query: &str,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let Some((expanded_matches, node_names)) = self.expanded_symbol_matches(query)? else {
            return Ok(Vec::new());
        };
        Ok(expanded_matches
            .into_iter()
            .map(|(id, score)| Self::build_search_hit(storage, &node_names, id, score))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    fn expanded_symbol_matches(&self, query: &str) -> Result<ExpandedSymbolMatches, ApiError> {
        let mut s = self.state.lock();
        let engine = s.search_engine.as_mut().ok_or_else(|| {
            ApiError::invalid_argument("Search engine not initialized. Open a project first.")
        })?;
        let direct_matches = engine.search_symbol_with_scores(query);
        let terms = extract_symbol_search_terms(query);
        if terms.is_empty() {
            return Ok(None);
        }

        let mut expanded = Vec::<(codestory_contracts::graph::NodeId, f32)>::new();
        for term in terms {
            expanded.extend(engine.search_symbol_with_scores(&term));
            if let Ok(ids) = engine.search_full_text(&term) {
                expanded.extend(ids.into_iter().enumerate().map(|(rank, id)| {
                    let text_score = 40.0_f32 - (rank as f32 * 1.5);
                    (id, text_score)
                }));
            }
        }

        Ok(Some((
            aggregate_symbol_matches(direct_matches, expanded),
            s.node_names.clone(),
        )))
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

    fn search_hybrid_results(
        &self,
        mut req: SearchRequest,
        _focus_node_id: Option<NodeId>,
        max_results: usize,
        _request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<SearchHit>, RetrievalStateDto), ApiError> {
        req.limit_per_source = max_results.clamp(1, 50) as u32;
        req.expand_search_plan = false;
        let results = self.search_results(req)?;
        Ok((results.hits, results.retrieval))
    }

    /// Run hybrid search through the same sidecar-primary contract as `search_results`.
    ///
    /// `max_results` limits returned hits; it is not a retrieval budget and does not prove packet
    /// sufficiency.
    pub fn search_hybrid(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: Option<u32>,
        hybrid_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let (hits, _) = self.search_hybrid_results(
            req,
            focus_node_id,
            max_results.unwrap_or(20).clamp(1, 50) as usize,
            hybrid_weights,
        )?;
        Ok(hits)
    }

    pub(crate) fn search_hybrid_scored(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<HybridSearchScoredHit>, ApiError> {
        let (hits, _) =
            self.search_hybrid_results(req, focus_node_id, max_results, request_weights)?;
        Ok(hits
            .into_iter()
            .map(|hit| HybridSearchScoredHit {
                lexical_score: hit.score,
                semantic_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.semantic)
                    .unwrap_or(0.0),
                graph_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.graph)
                    .unwrap_or(0.0),
                total_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.total)
                    .unwrap_or(hit.score),
                hit,
            })
            .collect())
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn search_hybrid_scored_inner(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<HybridSearchScoredHit>, RetrievalStateDto), ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;
        let semantic_disabled = semantic_disabled_by_request_weights(request_weights.as_ref());
        let storage_retrieval = if semantic_disabled {
            None
        } else {
            Some(retrieval_state_from_storage(&storage)?)
        };
        let mut graph_boosts = HashMap::<codestory_contracts::graph::NodeId, f32>::new();
        let requested_max_results = max_results.clamp(1, 50);
        let prefer_primary_sources = !query_mentions_non_primary_source(&req.query);
        let use_exact_symbol_lexical_fast_path =
            exact_symbol_lexical_fast_path(&req, request_weights.as_ref());
        let hybrid_config = hybrid_search_config_for_request(
            &req,
            requested_max_results,
            request_weights.clone(),
            prefer_primary_sources,
        );
        let exact_symbol_merge_queries =
            crate::search::lexical::exact_symbol_merged_lexical_queries(&req.query).len();

        let focus_core_id = match focus_node_id {
            Some(value) => Some(value.to_core()?),
            None => None,
        };
        if let Some(center) = focus_core_id {
            graph_boosts.insert(center, 1.0);
            if let Ok(edges) = storage.get_edges_for_node_id(center) {
                for edge in edges.into_iter().take(240) {
                    let (source, target) = edge.effective_endpoints();
                    if source != center {
                        graph_boosts.entry(source).or_insert(0.55);
                    }
                    if target != center {
                        graph_boosts.entry(target).or_insert(0.55);
                    }
                }
            }
        }

        let (hybrid, node_names, retrieval) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            let mut retrieval = storage_retrieval
                .clone()
                .unwrap_or_else(|| retrieval_state_from_engine(engine));

            if !semantic_disabled
                && !use_exact_symbol_lexical_fast_path
                && retrieval.mode == RetrievalModeDto::Hybrid
                && engine.semantic_doc_count() == 0
            {
                if !engine.embedding_runtime_configured()
                    && let Err(error) =
                        engine.set_embedding_runtime_for_runtime(&self.runtime_config)
                {
                    tracing::warn!(
                        "Search embedding runtime unavailable during hybrid load: {error}"
                    );
                }
                if engine.embedding_runtime_configured() && engine.semantic_doc_count() == 0 {
                    reload_llm_docs_from_storage(&storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
                }
                if let Some(storage_retrieval) = storage_retrieval.as_ref() {
                    retrieval = retrieval_state_from_engine_with_storage_contract(
                        engine,
                        storage_retrieval,
                    );
                } else {
                    retrieval = retrieval_state_from_engine(engine);
                }
            } else if !semantic_disabled
                && (engine.semantic_doc_count() > 0 || engine.embedding_runtime_configured())
                && let Some(storage_retrieval) = storage_retrieval.as_ref()
            {
                retrieval =
                    retrieval_state_from_engine_with_storage_contract(engine, storage_retrieval);
            }

            let context_storage_retrieval = storage_retrieval
                .clone()
                .unwrap_or_else(|| retrieval.clone());
            let symbol_table_size = engine.symbols().len();
            let hits = hybrid_hits_for_retrieval_state(
                engine,
                HybridHitsContext {
                    req: &req,
                    graph_boosts: &graph_boosts,
                    requested_max_results,
                    request_weights,
                    prefer_primary_sources,
                    storage_retrieval: &context_storage_retrieval,
                    use_exact_symbol_lexical_fast_path,
                },
                &mut retrieval,
            );
            s.last_hybrid_instrumentation = Some(HybridSearchInstrumentation {
                symbol_table_size,
                exact_symbol_merge_queries,
                hybrid_max_results: hybrid_config.max_results,
                hybrid_lexical_limit: hybrid_config.lexical_limit,
                hybrid_semantic_limit: hybrid_config.semantic_limit,
                mixed_natural_language: mixed_natural_language_query(&req.query),
            });
            tracing::info!(
                symbol_table_size,
                exact_symbol_merge_queries,
                hybrid_max_results = hybrid_config.max_results,
                hybrid_lexical_limit = hybrid_config.lexical_limit,
                hybrid_semantic_limit = hybrid_config.semantic_limit,
                mixed_nl = mixed_natural_language_query(&req.query),
                "hybrid_search_instrumentation"
            );

            (hits, s.node_names.clone(), retrieval)
        };

        let mut out = Vec::with_capacity(hybrid.len());
        for scored in hybrid {
            if let Some(mut hit) =
                Self::build_search_hit(&storage, &node_names, scored.node_id, scored.total_score)?
            {
                hit.score_breakdown = Some(RetrievalScoreBreakdownDto {
                    lexical: scored.lexical_score,
                    semantic: scored.semantic_score,
                    graph: scored.graph_score,
                    total: scored.total_score,
                    tier_cap: None,
                    boosts: Vec::new(),
                    dampening: Vec::new(),
                    final_rank_reason: None,
                    provenance: Vec::new(),
                });
                out.push(HybridSearchScoredHit {
                    hit,
                    lexical_score: scored.lexical_score,
                    semantic_score: scored.semantic_score,
                    graph_score: scored.graph_score,
                    total_score: scored.total_score,
                });
            }
        }
        if should_pretruncate_primary_source_window(
            &req.query,
            prefer_primary_sources,
            out.len(),
            requested_max_results,
        ) {
            let top_window_has_non_primary = out
                .iter()
                .take(requested_max_results)
                .any(|scored| is_non_primary_source_hit(&scored.hit));
            if top_window_has_non_primary {
                let primary_count = out
                    .iter()
                    .filter(|scored| !is_non_primary_source_hit(&scored.hit))
                    .count();
                if primary_count >= primary_source_retention_threshold(requested_max_results) {
                    out.retain(|scored| !is_non_primary_source_hit(&scored.hit));
                }
            } else {
                out.truncate(requested_max_results);
            }
        }
        let project_root = self.require_project_root().ok();
        out.sort_by(|left, right| {
            compare_search_hits_with_project_root(
                project_root.as_deref(),
                &req.query,
                &left.hit,
                &right.hit,
            )
        });
        out.truncate(requested_max_results);

        Ok((out, retrieval))
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
